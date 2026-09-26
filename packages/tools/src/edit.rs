use crate::{
    support::{error, mutate_file, resolve_path},
    ToolConfig,
};
use api::{
    async_trait, ErrorCode, Tool, ToolConcurrency, ToolContext, ToolOutput, ToolResult, ToolSpec,
};
use serde::Deserialize;
use serde_json::{json, Value};
use similar::{DiffTag, TextDiff};

pub struct EditTool {
    config: ToolConfig,
}

impl EditTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: String,
    edits: Vec<Replacement>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Replacement {
    old_text: String,
    new_text: String,
}

#[derive(Clone)]
struct Match {
    start: usize,
    end: usize,
    replacement: String,
    edit_index: usize,
}

#[derive(Clone, Copy)]
struct FuzzySpan {
    fuzzy_start: usize,
    fuzzy_end: usize,
    original_start: usize,
    original_end: usize,
}

struct FuzzyText {
    text: String,
    spans: Vec<FuzzySpan>,
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: "Edit one UTF-8 text file with one or more exact replacements. Every edits[].oldText must identify one unique, non-overlapping region in the original file. Nearby changes should be combined into one replacement.".into(),
            parameters: json!({"type":"object","properties":{
                "path":{"type":"string","minLength":1},
                "edits":{"type":"array","minItems":1,"maxItems":128,"items":{"type":"object","properties":{
                    "oldText":{"type":"string","minLength":1},"newText":{"type":"string"}
                },"required":["oldText","newText"],"additionalProperties":false}}
            },"required":["path","edits"],"additionalProperties":false}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        ctx.run.task.check()?;
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|_| error(ErrorCode::Schema, "invalid edit arguments"))?;
        if args.edits.is_empty()
            || args.edits.len() > 128
            || args.edits.iter().any(|edit| edit.old_text.is_empty())
        {
            return Err(error(
                ErrorCode::Schema,
                "edits must contain 1..128 nonempty oldText values",
            ));
        }

        let path = resolve_path(&self.config.cwd, &args.path)?;
        let path_text = path.display().to_string();
        let path_text_for_closure = path_text.clone();
        let edit_count = args.edits.len();
        let mutation = mutate_file(path.clone(), &ctx, &self.config, move |before| {
            let Some(bytes) = before else {
                return Err(error(
                    ErrorCode::Tool,
                    format!("cannot edit missing file {path_text_for_closure}"),
                ));
            };
            apply_edits(bytes, args.edits, &path_text_for_closure)
        })
        .await?;
        ctx.run.task.check()?;

        let before = mutation.before.unwrap_or_default();
        let after = mutation.after;
        let after_len = after.len();
        let diff_path = path_text.clone();
        let diff_task =
            tokio::task::spawn_blocking(move || build_diff(&before, &after, &diff_path));
        let (diff_text, first_changed_line) = diff_task
            .await
            .map_err(|_| error(ErrorCode::Tool, "edit diff worker failed"))??;
        let summary = format!(
            "Successfully replaced {} block{} in {}.",
            edit_count,
            if edit_count == 1 { "" } else { "s" },
            path.display()
        );
        let result_budget = ctx.run.limits.max_tool_result_bytes;
        let short_summary = format!(
            "Successfully replaced {} block{}.",
            edit_count,
            if edit_count == 1 { "" } else { "s" }
        );
        let mut result = ToolOutput::new(summary.clone());
        let mut detail = bounded_detail(
            &path_text,
            edit_count,
            after_len,
            &summary,
            diff_text.clone(),
            first_changed_line,
            result_budget.saturating_sub(summary.len() + 128),
        );
        result.structured = Some(detail.clone());
        if !output_fits(&result, result_budget) {
            result.content = short_summary.clone().into();
            detail = bounded_detail(
                "",
                edit_count,
                after_len,
                &short_summary,
                diff_text,
                first_changed_line,
                result_budget.saturating_sub(short_summary.len() + 128),
            );
            result.structured = Some(detail.clone());
            if !output_fits(&result, result_budget) {
                result.structured = None;
            }
        }
        if let Some(detail) = &result.structured {
            let _ = ctx.progress.set_detail("coding.diff", detail.clone());
        }
        Ok(result)
    }
}

fn apply_edits(bytes: Vec<u8>, edits: Vec<Replacement>, path: &str) -> api::Result<Vec<u8>> {
    if bytes.contains(&0) {
        return Err(error(
            ErrorCode::Unsupported,
            "edit supports UTF-8 text files without NUL bytes",
        ));
    }
    let had_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let text_bytes = if had_bom { &bytes[3..] } else { &bytes[..] };
    let original = std::str::from_utf8(text_bytes).map_err(|_| {
        error(
            ErrorCode::Unsupported,
            "edit supports valid UTF-8 text files",
        )
    })?;
    let line_ending = detect_line_ending(original);
    let normalized_original = normalize_lf(original);
    let normalized_edits: Vec<_> = edits
        .into_iter()
        .map(|edit| Replacement {
            old_text: normalize_lf(&edit.old_text),
            new_text: normalize_lf(&edit.new_text),
        })
        .collect();

    let exact_matches: Vec<_> = normalized_edits
        .iter()
        .map(|edit| occurrences(&normalized_original, &edit.old_text))
        .collect();
    let use_fuzzy = exact_matches.iter().any(Vec::is_empty);
    let fuzzy = use_fuzzy.then(|| fuzzy_text(&normalized_original));
    let replacement_base = fuzzy
        .as_ref()
        .map(|value| value.text.as_str())
        .unwrap_or(&normalized_original);
    let mut matches = Vec::with_capacity(normalized_edits.len());
    for (index, edit) in normalized_edits.iter().enumerate() {
        let old_text = if use_fuzzy {
            normalize_for_fuzzy_match(&edit.old_text)
        } else {
            edit.old_text.clone()
        };
        if old_text.is_empty() {
            return Err(error(
                ErrorCode::Tool,
                format!("edits[{index}].oldText is empty after normalization in {path}"),
            ));
        }
        let positions = occurrences(&replacement_base, &old_text);
        match positions.as_slice() {
            [] => {
                return Err(error(
                    ErrorCode::Tool,
                    format!("could not find edits[{index}] in {path}"),
                ));
            }
            [start] => {
                let (source_start, source_end) = if let Some(fuzzy) = &fuzzy {
                    map_fuzzy_range(&fuzzy.spans, *start, start + old_text.len()).ok_or_else(
                        || {
                            error(
                                ErrorCode::Tool,
                                format!("could not map edits[{index}] in {path}"),
                            )
                        },
                    )?
                } else {
                    (*start, start + old_text.len())
                };
                matches.push(Match {
                    start: source_start,
                    end: source_end,
                    replacement: edit.new_text.clone(),
                    edit_index: index,
                });
            }
            _ => {
                return Err(error(
                    ErrorCode::Tool,
                    format!(
                        "edits[{index}].oldText matched {} regions in {path}; include more context so it is unique",
                        positions.len()
                    ),
                ));
            }
        }
    }
    matches.sort_by_key(|item| item.start);
    for pair in matches.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(error(
                ErrorCode::Tool,
                format!(
                    "edits[{}] and edits[{}] overlap in {path}",
                    pair[0].edit_index, pair[1].edit_index
                ),
            ));
        }
    }

    let output = apply_replacements(&normalized_original, &matches, 0);
    if normalized_original == output {
        return Err(error(
            ErrorCode::Tool,
            format!("replacement made no changes to {path}"),
        ));
    }
    let output = restore_line_endings(output, line_ending);
    let mut output_bytes = Vec::with_capacity(output.len() + usize::from(had_bom) * 3);
    if had_bom {
        output_bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    output_bytes.extend_from_slice(output.as_bytes());
    Ok(output_bytes)
}

fn text_for_diff(bytes: &[u8]) -> api::Result<String> {
    let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    let text = std::str::from_utf8(bytes).map_err(|_| {
        error(
            ErrorCode::Unsupported,
            "edit supports valid UTF-8 text files",
        )
    })?;
    Ok(normalize_lf(text))
}

fn build_diff(before: &[u8], after: &[u8], path: &str) -> api::Result<(String, Option<usize>)> {
    let before_text = text_for_diff(before)?;
    let after_text = text_for_diff(after)?;
    let diff = TextDiff::configure()
        .timeout(std::time::Duration::from_millis(200))
        .diff_lines(&before_text, &after_text);
    let first_changed_line = diff
        .ops()
        .iter()
        .find(|operation| operation.tag() != DiffTag::Equal)
        .map(|operation| operation.new_range().start + 1);
    let diff_text = diff
        .unified_diff()
        .context_radius(4)
        .header(path, path)
        .to_string();
    Ok((diff_text, first_changed_line))
}

fn output_fits(output: &ToolOutput, budget: usize) -> bool {
    ToolResult::from_output("edit", output.clone()).payload_bytes() <= budget
}

fn detect_line_ending(value: &str) -> &'static str {
    let Some(newline) = value.find('\n') else {
        return "\n";
    };
    if newline > 0 && value.as_bytes()[newline - 1] == b'\r' {
        "\r\n"
    } else {
        "\n"
    }
}

fn normalize_lf(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(value: String, ending: &str) -> String {
    if ending == "\r\n" {
        value.replace('\n', "\r\n")
    } else {
        value
    }
}

fn normalize_for_fuzzy_match(value: &str) -> String {
    fuzzy_text(value).text
}

fn map_fuzzy_range(spans: &[FuzzySpan], start: usize, end: usize) -> Option<(usize, usize)> {
    let first = spans
        .iter()
        .find(|span| start >= span.fuzzy_start && start < span.fuzzy_end)?;
    let last = spans
        .iter()
        .find(|span| end > span.fuzzy_start && end <= span.fuzzy_end)?;
    Some((first.original_start, last.original_end))
}

fn fuzzy_text(value: &str) -> FuzzyText {
    let source = normalize_lf(value);
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut line_start = 0;

    for (index, character) in source.char_indices() {
        if character != '\n' {
            continue;
        }
        append_fuzzy_line(&source, line_start, index, &mut text, &mut spans);
        let fuzzy_start = text.len();
        text.push('\n');
        spans.push(FuzzySpan {
            fuzzy_start,
            fuzzy_end: text.len(),
            original_start: index,
            original_end: index + 1,
        });
        line_start = index + 1;
    }
    append_fuzzy_line(&source, line_start, source.len(), &mut text, &mut spans);
    FuzzyText { text, spans }
}

fn append_fuzzy_line(
    source: &str,
    start: usize,
    end: usize,
    output: &mut String,
    spans: &mut Vec<FuzzySpan>,
) {
    let line = &source[start..end];
    let trimmed_len = line.trim_end().len();
    for (relative, character) in line[..trimmed_len].char_indices() {
        let original_start = start + relative;
        let original_end = original_start + character.len_utf8();
        let mapped = match character {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        };
        let fuzzy_start = output.len();
        output.push(mapped);
        spans.push(FuzzySpan {
            fuzzy_start,
            fuzzy_end: output.len(),
            original_start,
            original_end,
        });
    }
}

fn occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut offset = 0;
    while offset <= haystack.len() {
        let Some(found) = haystack[offset..].find(needle) else {
            break;
        };
        let position = offset + found;
        result.push(position);
        // Overlapping occurrences are also ambiguous ("aa" in "aaa").
        offset = position
            + haystack[position..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
    }
    result
}

fn apply_replacements(content: &str, replacements: &[Match], offset: usize) -> String {
    let mut result = content.to_owned();
    for replacement in replacements.iter().rev() {
        let start = replacement.start - offset;
        let end = replacement.end - offset;
        result.replace_range(start..end, &replacement.replacement);
    }
    result
}

fn bounded_detail(
    path: &str,
    edit_count: usize,
    bytes: usize,
    summary: &str,
    full_diff: String,
    first_changed_line: Option<usize>,
    budget: usize,
) -> Value {
    let mut diff = full_diff;
    let mut diff_truncated = false;
    loop {
        let detail = json!({
            "path": path,
            "edits": edit_count,
            "bytes": bytes,
            "summary": summary,
            "diff": diff,
            "diffTruncated": diff_truncated,
            "firstChangedLine": first_changed_line,
        });
        if serde_json::to_vec(&detail).map_or(false, |encoded| encoded.len() <= budget) {
            return detail;
        }
        if diff.is_empty() {
            let minimal = json!({
                "changed": true,
                "edits": edit_count,
                "bytes": bytes,
                "diffTruncated": true,
                "firstChangedLine": first_changed_line,
            });
            if serde_json::to_vec(&minimal).map_or(false, |encoded| encoded.len() <= budget) {
                return minimal;
            }
            return json!({"changed": true, "diffTruncated": true});
        }
        let target = (diff.len() / 2).max(1);
        let (clipped, _) = crate::support::clip_head_tail(&diff, target);
        if clipped.len() >= diff.len() {
            diff.clear();
        } else {
            diff = clipped;
        }
        diff_truncated = true;
    }
}
