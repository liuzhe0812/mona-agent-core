use crate::{
    support::{clip_head_tail, error, resolve_path},
    ToolConfig,
};
use api::{
    async_trait, CancellationToken, ErrorCode, Tool, ToolConcurrency, ToolContext, ToolOutput,
    ToolSpec,
};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    future::Future,
    path::{Path, PathBuf},
};
use tokio::{
    fs,
    fs::File,
    io::AsyncReadExt,
    sync::mpsc::{self, Receiver},
};

#[derive(Clone)]
pub struct GrepTool {
    config: ToolConfig,
}

#[derive(Clone)]
pub struct FindTool {
    config: ToolConfig,
}

#[derive(Clone)]
pub struct LsTool {
    config: ToolConfig,
}

impl GrepTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

impl FindTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

impl LsTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepArguments {
    query: String,
    path: Option<String>,
    glob: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindArguments {
    pattern: String,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LsArguments {
    path: Option<String>,
}

fn cancelled(message: &str) -> api::AgentError {
    error(ErrorCode::Cancelled, message)
}

fn check_cancel(ctx: &ToolContext) -> api::Result<()> {
    ctx.run.task.check()?;
    if ctx.run.cancel.is_cancelled() {
        return Err(cancelled("search operation cancelled"));
    }
    Ok(())
}

async fn io_with_cancel<T, F>(
    cancel: &CancellationToken,
    future: F,
    operation: &str,
) -> api::Result<T>
where
    F: Future<Output = std::io::Result<T>>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(cancelled("search operation cancelled")),
        result = future => result.map_err(|value| error(ErrorCode::Tool, format!("{operation}: {value}"))),
    }
}

fn normalize_slashes(value: &str) -> String {
    value.replace('\\', "/").trim_start_matches("./").to_owned()
}

struct PathMatcher {
    full_path: GlobSet,
    basename: Option<GlobSet>,
}

impl PathMatcher {
    fn new(pattern: &str) -> api::Result<Self> {
        let pattern = normalize_slashes(pattern);
        let mut full_builder = GlobSetBuilder::new();
        full_builder.add(
            GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|value| {
                    error(ErrorCode::Schema, format!("invalid glob pattern: {value}"))
                })?,
        );
        let full_path = full_builder
            .build()
            .map_err(|value| error(ErrorCode::Schema, format!("invalid glob pattern: {value}")))?;
        let basename = if pattern.contains('/') {
            None
        } else {
            let mut basename_builder = GlobSetBuilder::new();
            basename_builder.add(
                GlobBuilder::new(&pattern)
                    .literal_separator(true)
                    .build()
                    .map_err(|value| {
                        error(ErrorCode::Schema, format!("invalid glob pattern: {value}"))
                    })?,
            );
            Some(basename_builder.build().map_err(|value| {
                error(ErrorCode::Schema, format!("invalid glob pattern: {value}"))
            })?)
        };
        Ok(Self {
            full_path,
            basename,
        })
    }

    fn is_match(&self, relative_path: &str) -> bool {
        let relative_path = normalize_slashes(relative_path);
        self.full_path.is_match(Path::new(&relative_path))
            || self.basename.as_ref().is_some_and(|matcher| {
                relative_path
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| matcher.is_match(Path::new(name)))
            })
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let value = normalize_slashes(&relative.to_string_lossy());
    if value.is_empty() {
        path.file_name()
            .map(|name| normalize_slashes(&name.to_string_lossy()))
            .unwrap_or_default()
    } else {
        value
    }
}

fn display_path(search_root: &Path, path: &Path, root_is_directory: bool) -> String {
    if root_is_directory {
        relative_path(search_root, path)
    } else {
        path.file_name()
            .map(|name| normalize_slashes(&name.to_string_lossy()))
            .unwrap_or_else(|| normalize_slashes(&path.to_string_lossy()))
    }
}

fn bounded_output(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    if max_bytes == 0 {
        return String::new();
    }
    let (clipped, _) = clip_head_tail(&text, max_bytes);
    if clipped.len() <= max_bytes {
        clipped
    } else {
        api::clip_utf8(&clipped, max_bytes).to_owned()
    }
}

async fn walk_files(
    root: &Path,
    cancel: &CancellationToken,
) -> api::Result<Receiver<api::Result<PathBuf>>> {
    let (sender, receiver) = mpsc::channel(64);
    let root = root.to_path_buf();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let walker = WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .follow_links(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .sort_by_file_name(|left, right| left.cmp(right))
            .build();
        for entry in walker {
            if cancel.is_cancelled() {
                break;
            }
            match entry {
                Ok(entry) => {
                    if entry
                        .file_type()
                        .map_or(false, |file_type| file_type.is_file())
                    {
                        if sender
                            .blocking_send(Ok(entry.path().to_path_buf()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(value) => {
                    let _ = sender.blocking_send(Err(error(
                        ErrorCode::Tool,
                        format!("cannot walk search path: {value}"),
                    )));
                    break;
                }
            }
        }
    });
    Ok(receiver)
}

async fn next_walk_file(
    receiver: &mut Receiver<api::Result<PathBuf>>,
    cancel: &CancellationToken,
) -> api::Result<Option<PathBuf>> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(cancelled("search operation cancelled")),
        item = receiver.recv() => match item {
            Some(result) => result.map(Some),
            None => Ok(None),
        },
    }
}

fn prefix_table(pattern: &[u8]) -> Vec<usize> {
    let mut table = vec![0; pattern.len()];
    let mut matched = 0;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = table[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        table[index] = matched;
    }
    table
}

fn line_text(line: &[u8], truncated: bool) -> String {
    let mut text = String::from_utf8_lossy(line).into_owned();
    if text.ends_with('\r') {
        text.pop();
    }
    if truncated {
        text.push_str(" [line truncated]");
    }
    text
}

async fn grep_file(
    path: &Path,
    display: &str,
    query: &[u8],
    prefix: &[usize],
    cancel: &CancellationToken,
    max_matches: usize,
    max_line_bytes: usize,
) -> api::Result<(Vec<String>, bool)> {
    let mut file = io_with_cancel(cancel, File::open(path), "cannot open search file").await?;
    let mut chunk = vec![0_u8; 8 * 1024];
    let mut line = Vec::with_capacity(max_line_bytes.min(8 * 1024));
    let mut line_number = 1usize;
    let mut query_index = 0usize;
    let mut line_matches = false;
    let mut line_truncated = false;
    let mut matches = Vec::new();
    let mut more_matches = false;

    loop {
        let read = io_with_cancel(cancel, file.read(&mut chunk), "cannot read search file").await?;
        if read == 0 {
            break;
        }
        for &byte in &chunk[..read] {
            if byte == 0 {
                return Ok((Vec::new(), false));
            }
            if byte == b'\n' {
                if line_matches {
                    if matches.len() < max_matches {
                        matches.push(format!(
                            "{display}:{line_number}: {}",
                            line_text(&line, line_truncated)
                        ));
                    } else {
                        more_matches = true;
                    }
                }
                line.clear();
                line_truncated = false;
                line_number += 1;
                query_index = 0;
                line_matches = false;
                continue;
            }
            if line.len() < max_line_bytes {
                line.push(byte);
            } else {
                line_truncated = true;
            }
            while query_index > 0 && query[query_index] != byte {
                query_index = prefix[query_index - 1];
            }
            if query[query_index] == byte {
                query_index += 1;
            }
            if query_index == query.len() {
                line_matches = true;
                query_index = prefix[query_index - 1];
            }
        }
    }

    if !line.is_empty() || line_number == 1 {
        if line_matches {
            if matches.len() < max_matches {
                matches.push(format!(
                    "{display}:{line_number}: {}",
                    line_text(&line, line_truncated)
                ));
            } else {
                more_matches = true;
            }
        }
    }
    Ok((matches, more_matches))
}

async fn resolve_search_root(
    ctx: &ToolContext,
    value: Option<&str>,
    cwd: &Path,
) -> api::Result<(PathBuf, bool)> {
    let root = resolve_path(cwd, value.unwrap_or("."))?;
    let metadata = io_with_cancel(
        &ctx.run.cancel,
        fs::metadata(&root),
        "cannot inspect search path",
    )
    .await?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(error(
            ErrorCode::Tool,
            "search path is not a file or directory",
        ));
    }
    Ok((root, metadata.is_dir()))
}

async fn find_matching_paths(
    root: &Path,
    matcher: &PathMatcher,
    cancel: &CancellationToken,
    max_results: usize,
) -> api::Result<(Vec<PathBuf>, bool)> {
    let mut receiver = walk_files(root, cancel).await?;
    let mut matches = Vec::new();
    let mut truncated = false;
    while let Some(path) = next_walk_file(&mut receiver, cancel).await? {
        let relative = relative_path(root, &path);
        if !matcher.is_match(&relative) {
            continue;
        }
        if matches.len() < max_results {
            matches.push(path);
        } else {
            truncated = true;
            break;
        }
    }
    Ok((matches, truncated))
}

async fn grep_output(
    ctx: &ToolContext,
    root: &Path,
    root_is_directory: bool,
    query: &str,
    glob: Option<&str>,
    config: &ToolConfig,
) -> api::Result<String> {
    let matcher = glob.map(PathMatcher::new).transpose()?;
    let query = query.as_bytes();
    let prefix = prefix_table(query);
    let mut receiver = walk_files(root, &ctx.run.cancel).await?;
    let mut matches = Vec::new();
    let mut truncated_by_results = false;
    while let Some(path) = next_walk_file(&mut receiver, &ctx.run.cancel).await? {
        check_cancel(ctx)?;
        let relative = relative_path(root, &path);
        if matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.is_match(&relative))
        {
            continue;
        }

        let remaining = config.max_search_results.saturating_sub(matches.len());
        let collect_limit = remaining.max(1);
        let (file_matches, file_has_more) = grep_file(
            &path,
            &display_path(root, &path, root_is_directory),
            query,
            &prefix,
            &ctx.run.cancel,
            collect_limit,
            config.max_read_bytes,
        )
        .await?;
        let file_had_match = !file_matches.is_empty();
        if remaining > 0 {
            matches.extend(file_matches.into_iter().take(remaining));
        }
        if file_has_more || (remaining == 0 && file_had_match) {
            truncated_by_results = true;
            break;
        }
    }

    let mut output = if matches.is_empty() {
        "No matches found".to_owned()
    } else {
        matches.join("\n")
    };
    if truncated_by_results {
        output.push_str(&format!(
            "\n\n[{} result limit reached; increase max_search_results to see more.]",
            config.max_search_results
        ));
    }
    Ok(bounded_output(output, config.max_read_bytes))
}

#[async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Recursively search complete UTF-8 text files for a literal query, respecting gitignore rules. Binary files are skipped; results are path:line:text.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "minLength": 1},
                    "path": {"type": "string", "minLength": 1},
                    "glob": {"type": "string", "minLength": 1}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        check_cancel(&ctx)?;
        let args: GrepArguments = serde_json::from_value(arguments)
            .map_err(|_| error(ErrorCode::Schema, "invalid grep arguments"))?;
        if args.query.is_empty() {
            return Err(error(ErrorCode::Schema, "grep query must not be empty"));
        }
        let (root, root_is_directory) =
            resolve_search_root(&ctx, args.path.as_deref(), &self.config.cwd).await?;
        let text = grep_output(
            &ctx,
            &root,
            root_is_directory,
            &args.query,
            args.glob.as_deref(),
            &self.config,
        )
        .await?;
        Ok(ToolOutput::new(text))
    }
}

#[async_trait]
impl Tool for FindTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find".into(),
            description: "Recursively find files whose relative paths match a simple glob pattern. Results respect gitignore rules.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "minLength": 1},
                    "path": {"type": "string", "minLength": 1}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        check_cancel(&ctx)?;
        let args: FindArguments = serde_json::from_value(arguments)
            .map_err(|_| error(ErrorCode::Schema, "invalid find arguments"))?;
        if args.pattern.is_empty() {
            return Err(error(ErrorCode::Schema, "find pattern must not be empty"));
        }
        let matcher = PathMatcher::new(&args.pattern)?;
        let (root, root_is_directory) =
            resolve_search_root(&ctx, args.path.as_deref(), &self.config.cwd).await?;
        let (matches, truncated_by_results) = find_matching_paths(
            &root,
            &matcher,
            &ctx.run.cancel,
            self.config.max_search_results,
        )
        .await?;
        let mut output = matches
            .iter()
            .map(|path| display_path(&root, path, root_is_directory))
            .collect::<Vec<_>>()
            .join("\n");
        if output.is_empty() {
            output = "No files found matching pattern".to_owned();
        }
        if truncated_by_results {
            output.push_str(&format!(
                "\n\n[{} result limit reached; increase max_search_results to see more.]",
                self.config.max_search_results
            ));
        }
        Ok(ToolOutput::new(bounded_output(
            output,
            self.config.max_read_bytes,
        )))
    }
}

#[async_trait]
impl Tool for LsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ls".into(),
            description: "List one directory, sorted alphabetically; directory entries have a trailing slash.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "minLength": 1}},
                "additionalProperties": false
            }),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        check_cancel(&ctx)?;
        let args: LsArguments = serde_json::from_value(arguments)
            .map_err(|_| error(ErrorCode::Schema, "invalid ls arguments"))?;
        let (root, root_is_directory) =
            resolve_search_root(&ctx, args.path.as_deref(), &self.config.cwd).await?;
        if !root_is_directory {
            return Err(error(ErrorCode::Tool, "ls path is not a directory"));
        }
        let mut reader = io_with_cancel(
            &ctx.run.cancel,
            fs::read_dir(&root),
            "cannot read directory",
        )
        .await?;
        let mut entries = Vec::new();
        let mut truncated_by_results = false;
        while let Some(entry) = io_with_cancel(
            &ctx.run.cancel,
            reader.next_entry(),
            "cannot read directory",
        )
        .await?
        {
            check_cancel(&ctx)?;
            let file_type = io_with_cancel(
                &ctx.run.cancel,
                entry.file_type(),
                "cannot inspect directory entry",
            )
            .await?;
            if !file_type.is_dir() && !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.push((name, file_type.is_dir()));
            if entries.len() > self.config.max_search_results {
                entries.sort_by(|left, right| {
                    left.0
                        .to_lowercase()
                        .cmp(&right.0.to_lowercase())
                        .then_with(|| left.0.cmp(&right.0))
                });
                entries.truncate(self.config.max_search_results);
                truncated_by_results = true;
            }
        }
        entries.sort_by(|left, right| {
            left.0
                .to_lowercase()
                .cmp(&right.0.to_lowercase())
                .then_with(|| left.0.cmp(&right.0))
        });
        let output = entries
            .into_iter()
            .take(self.config.max_search_results)
            .map(|(name, is_directory)| {
                if is_directory {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut output = if output.is_empty() && !truncated_by_results {
            "(empty directory)".to_owned()
        } else {
            output
        };
        if truncated_by_results {
            output.push_str(&format!(
                "\n\n[{} result limit reached; increase max_search_results to see more.]",
                self.config.max_search_results
            ));
        }
        Ok(ToolOutput::new(bounded_output(
            output,
            self.config.max_read_bytes,
        )))
    }
}
