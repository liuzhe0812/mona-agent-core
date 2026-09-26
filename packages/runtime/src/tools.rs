use crate::{checkpoint::Checkpoints, events::{EventBus, ProgressSink}, gate::{bounded, CancelOnDrop}, host::Registry, validation::valid_name};
use crate::history::{history_limit, ToolBudget};
use api::*;
use futures_util::{stream::FuturesUnordered, StreamExt};
use jsonschema::{error::{TypeKind, ValidationError, ValidationErrorKind}, Draft, ErrorIterator, JSONSchema};
use std::{collections::{BTreeSet, VecDeque}, sync::Arc};
use tokio::time::Instant;

const MAX_SCHEMA_ERRORS: usize = 4;
const MAX_SCHEMA_ERROR_BYTES: usize = 1024;
const MAX_SCHEMA_PATH_BYTES: usize = 160;
const MAX_SCHEMA_REASON_BYTES: usize = 256;

pub(crate) struct CompiledTool {
    pub spec: ToolSpec,
    implementation: Arc<dyn Tool>,
    validator: JSONSchema,
}
impl CompiledTool {
    pub fn new(implementation: Arc<dyn Tool>) -> Result<Self> {
        let spec = implementation.spec();
        if !valid_name(&spec.name) || spec.parameters.get("type").and_then(|v| v.as_str()) != Some("object") {
            return Err(AgentError::new(ErrorCode::Schema, "tool requires a valid name and object parameter schema"));
        }
        if spec.description.len() > 16 * 1024 || serde_json::to_vec(&spec.parameters).map_or(true, |v| v.len() > 64 * 1024) {
            return Err(AgentError::new(ErrorCode::Schema, "tool description/schema too large"));
        }
        reject_external_refs(&spec.parameters)?;
        let draft = match spec.parameters.get("$schema").and_then(|v|v.as_str()).map(|s|s.trim_end_matches('#')) {
            None if spec.parameters.get("$schema").is_none() => Draft::Draft7,
            Some("http://json-schema.org/draft-07/schema"|"https://json-schema.org/draft-07/schema") => Draft::Draft7,
            Some("https://json-schema.org/draft/2019-09/schema") => Draft::Draft201909,
            Some("https://json-schema.org/draft/2020-12/schema") => Draft::Draft202012,
            _ => return Err(AgentError::new(ErrorCode::Schema,"unsupported tool JSON Schema dialect")),
        };
        let validator = JSONSchema::options().with_draft(draft).compile(&spec.parameters)
            .map_err(|e| AgentError::new(ErrorCode::Schema, format!("invalid tool schema: {e}")))?;
        Ok(Self { spec, implementation, validator })
    }
}
fn reject_external_refs(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            for key in ["$ref","$dynamicRef","$recursiveRef"] {
                if map.get(key).is_some_and(|r|!r.as_str().is_some_and(|s|s.starts_with('#'))) {
                    return Err(AgentError::new(ErrorCode::Schema, "only local JSON Schema references are allowed"));
                }
            }
            for value in map.values() { reject_external_refs(value)?; }
        }
        serde_json::Value::Array(values) => for value in values { reject_external_refs(value)?; },
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod schema_dialect_tests {
    use super::*;
    use serde_json::{json,Value};
    struct Declared(Value);
    #[async_trait]
    impl Tool for Declared {
        fn spec(&self)->ToolSpec { ToolSpec {name:"declared_schema".into(),description:"schema test".into(),parameters:self.0.clone(),concurrency:ToolConcurrency::Exclusive,side_effects:false} }
        async fn execute(&self,_ctx:ToolContext,_args:Value)->Result<ToolOutput>{Ok(ToolOutput::new("unused"))}
    }
    #[test]
    fn declared_2020_tuple_is_not_misinterpreted_as_draft7() {
        let schema=json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"tuple":{"type":"array","prefixItems":[{"type":"string"}],"items":false}},"required":["tuple"]});
        let tool=CompiledTool::new(Arc::new(Declared(schema))).unwrap();
        assert!(tool.validator.is_valid(&json!({"tuple":["valid"]})));
        assert!(!tool.validator.is_valid(&json!({"tuple":[1]})));
        assert!(!tool.validator.is_valid(&json!({"tuple":["valid","extra"]})));
    }
    #[test]
    fn unknown_dialect_and_external_dynamic_reference_are_rejected() {
        for schema in [json!({"$schema":"https://unknown.invalid/schema","type":"object"}),json!({"type":"object","$dynamicRef":"https://outside.invalid/schema"})] {
            assert!(CompiledTool::new(Arc::new(Declared(schema))).is_err());
        }
    }
}

struct Ready { call: ToolCall, tool: Arc<CompiledTool> }

pub(crate) async fn execute_batch(
    context: RunContext, registry: Arc<Registry>, limits: RunLimits,
    allowed_effects: &BTreeSet<String>, offered: &BTreeSet<String>, calls: &[ToolCall], bus: EventBus, step: usize, checkpoints: Checkpoints,
    mut result_budget: ToolBudget,
) -> (Vec<ToolResult>, Option<AgentError>) {
    let mut settled: Vec<Option<ToolResult>> = vec![None; calls.len()];
    let mut prepared = vec![];
    // Preflight only immutable schemas/offered tools/host authority. Dynamic policies
    // run once immediately before each dispatch, after earlier exclusive tools settle.
    for (index, call) in calls.iter().enumerate() {
        let check = async {
            context.task.check()?;
            checkpoints.check().await?;
            if !offered.contains(&call.name) {
                return Ok(Err(ToolResult::new(&call.id, ToolStatus::Denied, "tool was not offered in this model round")));
            }
            if context.cancel.is_cancelled() { return Err(AgentError::new(ErrorCode::Cancelled, "run cancelled")); }
            let Some(tool) = registry.tools.get(&call.name) else {
                return Ok(Err(ToolResult::new(&call.id, ToolStatus::Error, "unknown tool")));
            };
            if !tool.validator.is_valid(&call.arguments) {
                let message = match tool.validator.validate(&call.arguments) {
                    Ok(()) => "invalid tool arguments".to_owned(),
                    Err(errors) => format_schema_errors(errors),
                };
                return Ok(Err(ToolResult::new(&call.id, ToolStatus::Error, message)));
            }
            // Final, non-relaxable host gate. Plugin metadata is trusted, not a sandbox.
            if tool.spec.side_effects && !allowed_effects.contains(&tool.spec.name) {
                return Ok(Err(ToolResult::new(&call.id, ToolStatus::Denied, "side-effect tool not authorized by host")));
            }
            Ok(Ok(Ready { call: call.clone(), tool: tool.clone() }))
        }.await;
        match check {
            Ok(Ok(ready)) => prepared.push((index, ready)),
            Ok(Err(mut result)) => {
                cap_result(&mut result, limits.max_tool_result_bytes);
                if let Err(error) = result_budget.settle(index, &result) {
                    // No body has started. Fall back to the pre-reserved pair-closing records.
                    let results: Vec<_> = calls.iter().map(|c| ToolResult::new(&c.id, ToolStatus::Skipped,
                        "history limit reached during preflight; not executed")).collect();
                    for (i, result) in results.iter().enumerate() { bus.complete_tool(step, i, result); }
                    return (results, Some(error));
                }
                settled[index] = Some(result);
            }
            Err(error) => {
                // Nothing in this batch has started yet; preserve every call/result pair.
                let results: Vec<_> = calls.iter().map(|c| ToolResult::new(&c.id, ToolStatus::Skipped, "batch preflight failed; not executed")).collect();
                for (i, result) in results.iter().enumerate() { bus.complete_tool(step, i, result); }
                return (results, Some(error));
            }
        }
    }
    let mut groups: Vec<Vec<(usize, Ready)>> = vec![];
    let mut parallel = vec![];
    for item in prepared {
        if item.1.tool.spec.concurrency == ToolConcurrency::Exclusive {
            if !parallel.is_empty() { groups.push(std::mem::take(&mut parallel)); }
            groups.push(vec![item]);
        } else { parallel.push(item); }
    }
    if !parallel.is_empty() { groups.push(parallel); }
    let mut fatal: Option<AgentError> = None;
    for group in groups {
        if fatal.is_some() {
            for (index, ready) in group { settled[index] = Some(ToolResult::new(&ready.call.id, ToolStatus::Skipped, "earlier operation stopped the batch")); }
            continue;
        }
        let group_cancel = context.cancel.child_token();
        let mut group_ctx = context.clone(); group_ctx.cancel = group_cancel.clone();
        let mut queue: VecDeque<_> = group.into();
        let mut running = FuturesUnordered::new();
        loop {
            while fatal.is_none() && running.len() < limits.max_parallel_tools {
                let Some((index, ready)) = queue.front() else { break; };
                if result_budget.reserve(*index, &ready.call.id, limits.max_tool_result_bytes).is_err() {
                    // In-flight reservations may soon shrink to actual results. Drain first;
                    // never cancel an already-started tool just to reclaim history capacity.
                    if running.is_empty() { fatal = Some(history_limit()); }
                    break;
                }
                let (index, ready) = queue.pop_front().expect("front was present");
                running.push(run_indexed(index, ready, group_ctx.clone(), registry.clone(), limits.clone(), bus.clone(), step, checkpoints.clone()));
            }
            let Some((index, result, error)) = running.next().await else { break; };
            if let Err(accounting_error) = result_budget.settle(index, &result) {
                fatal.get_or_insert(accounting_error);
                group_cancel.cancel();
            }
            settled[index] = Some(result);
            if let Some(error) = error {
                if fatal.is_none() { fatal = Some(error); group_cancel.cancel(); }
            }
        }
        for (index, ready) in queue {
            settled[index] = Some(ToolResult::new(&ready.call.id, ToolStatus::Skipped, "batch stopped before dispatch; tool not executed"));
        }
    }
    let results: Vec<_> = calls.iter().enumerate().map(|(index, call)| {
        let mut result = settled[index].take().unwrap_or_else(|| ToolResult::new(&call.id, ToolStatus::Skipped, "not executed"));
        if let Some(error) = cap_result(&mut result, limits.max_tool_result_bytes) {
            if fatal.is_none() { fatal = Some(error); }
        }
        result
    }).collect();
    for (i, result) in results.iter().enumerate() { bus.complete_tool(step, i, result); }
    (results, fatal)
}

fn format_schema_errors(errors: ErrorIterator<'_>) -> String {
    let mut message = String::from("invalid tool arguments: ");
    let mut included = 0;
    let mut omitted = false;
    for error in errors {
        if included >= MAX_SCHEMA_ERRORS {
            omitted = true;
            break;
        }
        let path = sanitize_text(&schema_error_path(&error), MAX_SCHEMA_PATH_BYTES);
        let reason = sanitize_text(&schema_error_reason(&error.kind), MAX_SCHEMA_REASON_BYTES);
        let entry = format!("{path}: {reason}");
        let separator = if included == 0 { "" } else { "; " };
        if message.len().saturating_add(separator.len()).saturating_add(entry.len()) > MAX_SCHEMA_ERROR_BYTES {
            omitted = true;
            break;
        }
        message.push_str(separator);
        message.push_str(&entry);
        included += 1;
    }
    if omitted {
        let suffix = if included == 0 { "details omitted" } else { "; additional details omitted" };
        let available = MAX_SCHEMA_ERROR_BYTES.saturating_sub(message.len());
        message.push_str(clip_utf8(suffix, available));
    }
    message
}

fn schema_error_path(error: &ValidationError<'_>) -> String {
    let path = error.instance_path.to_string();
    if let ValidationErrorKind::Required { property } = &error.kind {
        if let Some(property) = property.as_str() {
            let property = property.replace('~', "~0").replace('/', "~1");
            return format!("{path}/{property}");
        }
    }
    if path.is_empty() { "$".to_owned() } else { path }
}

fn schema_error_reason(kind: &ValidationErrorKind) -> String {
    match kind {
        ValidationErrorKind::AdditionalItems { limit } => format!("too many items (maximum {limit})"),
        ValidationErrorKind::AdditionalProperties { unexpected } => {
            let names = unexpected.iter().take(MAX_SCHEMA_ERRORS).map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
            format!("unexpected properties: {names}")
        }
        ValidationErrorKind::AnyOf => "does not match any allowed schema".to_owned(),
        ValidationErrorKind::BacktrackLimitExceeded { .. } => "pattern validation exceeded its limit".to_owned(),
        ValidationErrorKind::Constant { .. } => "must equal the configured value".to_owned(),
        ValidationErrorKind::Contains => "no item matches the required schema".to_owned(),
        ValidationErrorKind::ContentEncoding { .. } => "has an invalid content encoding".to_owned(),
        ValidationErrorKind::ContentMediaType { .. } => "has an invalid content media type".to_owned(),
        ValidationErrorKind::Custom { message } => format!("validation failed: {message}"),
        ValidationErrorKind::Enum { .. } => "must be one of the allowed values".to_owned(),
        ValidationErrorKind::ExclusiveMaximum { limit } => format!("must be < {limit}"),
        ValidationErrorKind::ExclusiveMinimum { limit } => format!("must be > {limit}"),
        ValidationErrorKind::FalseSchema => "is not allowed".to_owned(),
        ValidationErrorKind::FileNotFound { .. }
        | ValidationErrorKind::FromUtf8 { .. }
        | ValidationErrorKind::InvalidReference { .. }
        | ValidationErrorKind::InvalidURL { .. }
        | ValidationErrorKind::JSONParse { .. }
        | ValidationErrorKind::Resolver { .. }
        | ValidationErrorKind::Schema
        | ValidationErrorKind::UnknownReferenceScheme { .. }
        | ValidationErrorKind::Utf8 { .. } => "failed schema validation".to_owned(),
        ValidationErrorKind::Format { format } => format!("must match format `{format}`"),
        ValidationErrorKind::MaxItems { limit } => format!("has too many items (maximum {limit})"),
        ValidationErrorKind::Maximum { limit } => format!("must be <= {limit}"),
        ValidationErrorKind::MaxLength { limit } => format!("is too long (maximum {limit} characters)"),
        ValidationErrorKind::MaxProperties { limit } => format!("has too many properties (maximum {limit})"),
        ValidationErrorKind::MinItems { limit } => format!("has too few items (minimum {limit})"),
        ValidationErrorKind::Minimum { limit } => format!("must be >= {limit}"),
        ValidationErrorKind::MinLength { limit } => format!("is too short (minimum {limit} characters)"),
        ValidationErrorKind::MinProperties { limit } => format!("has too few properties (minimum {limit})"),
        ValidationErrorKind::MultipleOf { multiple_of } => format!("must be a multiple of {multiple_of}"),
        ValidationErrorKind::Not { .. } => "matches a forbidden schema".to_owned(),
        ValidationErrorKind::OneOfMultipleValid => "matches more than one allowed schema".to_owned(),
        ValidationErrorKind::OneOfNotValid => "does not match exactly one allowed schema".to_owned(),
        ValidationErrorKind::Pattern { .. } => "does not match the required pattern".to_owned(),
        ValidationErrorKind::PropertyNames { error } => schema_error_reason(&error.kind),
        ValidationErrorKind::Required { .. } => "is required".to_owned(),
        ValidationErrorKind::Type { kind } => match kind {
            TypeKind::Single(expected) => format!("expected {expected}"),
            TypeKind::Multiple(expected) => {
                let expected = expected.into_iter().map(|kind| kind.to_string()).collect::<Vec<_>>().join(" or ");
                format!("expected {expected}")
            }
        },
        ValidationErrorKind::UnevaluatedProperties { unexpected } => {
            let names = unexpected.iter().take(MAX_SCHEMA_ERRORS).map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
            format!("unexpected properties: {names}")
        }
        ValidationErrorKind::UniqueItems => "contains duplicate items".to_owned(),
    }
}

fn sanitize_text(text: &str, max_bytes: usize) -> String {
    let sanitized = text.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect::<String>();
    clip_utf8(&sanitized, max_bytes).to_owned()
}

async fn run_indexed(index: usize, ready: Ready, ctx: RunContext, registry: Arc<Registry>, limits: RunLimits, bus: EventBus, step: usize, checkpoints: Checkpoints)
    -> (usize, ToolResult, Option<AgentError>) {
    let (result, error) = run_one(ready, ctx, registry, limits, bus, step, index, checkpoints).await;
    (index, result, error)
}

async fn run_one(ready: Ready, ctx: RunContext, registry: Arc<Registry>, limits: RunLimits, bus: EventBus, step: usize, index: usize, checkpoints: Checkpoints)
    -> (ToolResult, Option<AgentError>) {
    let call = ready.call;
    if let Err(error) = ctx.task.check() {
        return (ToolResult::new(&call.id, ToolStatus::Skipped, "task stopped before tool started"), Some(error));
    }
    if ctx.cancel.is_cancelled() {
        return (ToolResult::new(&call.id, ToolStatus::Skipped, "cancelled before tool started"), Some(AgentError::new(ErrorCode::Cancelled, "run cancelled")));
    }
    // Persist intent before entering the tool body. Recheck stop conditions after I/O.
    if let Err(error) = checkpoints.intent(&call.id).await {
        return (ToolResult::new(&call.id, ToolStatus::Skipped, "checkpoint failed before tool dispatch"), Some(error));
    }
    if let Err(error) = ctx.task.check() {
        return (ToolResult::new(&call.id, ToolStatus::Skipped, "task stopped after intent, before dispatch"), Some(error));
    }
    if ctx.cancel.is_cancelled() {
        return (ToolResult::new(&call.id, ToolStatus::Skipped, "cancelled after intent, before dispatch"), Some(AgentError::new(ErrorCode::Cancelled, "run cancelled")));
    }
    // Intent is not dispatch. A slow checkpoint must not leave a stale policy decision.
    // Denials and failed checks are settled durably without entering the tool body.
    for policy in &registry.policies {
        let operation = ctx.cancel.child_token();
        let _drop_cancel = CancelOnDrop(operation.clone());
        let mut policy_ctx = ctx.clone(); policy_ctx.cancel = operation.clone();
        let decision = bounded(&ctx.cancel, &operation,
            ctx.task.deadline().min(Instant::now() + limits.hook_timeout), limits.cancellation_grace,
            policy.check(&policy_ctx, &call, &ready.tool.spec)).await;
        let (mut result, mut fatal) = match decision {
            Ok(PolicyDecision::Allow) => continue,
            Ok(PolicyDecision::Deny(reason)) =>
                (ToolResult::new(&call.id, ToolStatus::Denied, clip_utf8(&reason, 4096)), None),
            Err(error) => (ToolResult::new(&call.id, ToolStatus::Skipped,
                "policy check failed before dispatch; not executed"), Some(error)),
        };
        if let Some(error) = cap_result(&mut result, limits.max_tool_result_bytes) { fatal.get_or_insert(error); }
        if let Err(error) = checkpoints.settled(&result).await { fatal.get_or_insert(error); }
        bus.complete_tool(step, index, &result);
        return (result, fatal);
    }
    if let Err(error) = ctx.task.check().and_then(|_| {
        if ctx.cancel.is_cancelled() { Err(AgentError::new(ErrorCode::Cancelled, "cancelled before dispatch")) }
        else { Ok(()) }
    }) {
        let result = ToolResult::new(&call.id, ToolStatus::Skipped, "stopped after policy; not executed");
        let _ = checkpoints.settled(&result).await;
        bus.complete_tool(step, index, &result);
        return (result, Some(error));
    }
    if let Err(error) = checkpoints.check().await {
        return (ToolResult::new(&call.id, ToolStatus::Skipped, "checkpoint failed before dispatch"), Some(error));
    }
    bus.start_tool(step, index);
    let operation = ctx.cancel.child_token();
    let _drop_cancel = CancelOnDrop(operation.clone());
    let mut tool_run = ctx.clone(); tool_run.cancel = operation.clone();
    let tool_ctx = ToolContext { run: tool_run, call_id: call.id.clone(),
        progress: Arc::new(ProgressSink { bus: bus.clone(), item_id: tool_item_id(step, index) }) };
    let started = Instant::now();
    let execution = bounded(&ctx.cancel, &operation,
        ctx.task.deadline().min(Instant::now() + limits.tool_timeout), limits.cancellation_grace,
        ready.tool.implementation.execute(tool_ctx, call.arguments.clone())).await;
    ctx.task.record_tool_timing(started.elapsed());
    let (mut result, mut fatal) = match execution {
        Ok(output) => (ToolResult::from_output(&call.id, output), None),
        Err(error) => {
            let interrupted = matches!(error.code, ErrorCode::Cancelled | ErrorCode::Deadline | ErrorCode::Panic);
            let status = if interrupted { ToolStatus::Unknown } else { ToolStatus::Error };
            let message = if interrupted { "tool was started; final side-effect state is unknown".to_owned() } else { error.to_string() };
            (ToolResult::new(&call.id, status, message), if interrupted { Some(error) } else { None })
        }
    };
    if fatal.is_none() {
        for transform in &registry.results {
            let operation = ctx.cancel.child_token();
            let _drop_cancel = CancelOnDrop(operation.clone());
            let mut transform_ctx = ctx.clone(); transform_ctx.cancel = operation.clone();
            let transformed = bounded(&ctx.cancel, &operation,
                ctx.task.deadline().min(Instant::now() + limits.hook_timeout), limits.cancellation_grace,
                transform.transform(&transform_ctx, &call, result.clone())).await;
            match transformed {
                Ok(mut next) if next.call_id == result.call_id && next.status == result.status => {
                    next.original_bytes = next.original_bytes.max(result.original_bytes);
                    next.truncated |= result.truncated;
                    result = next;
                }
                Ok(_) => { fatal = Some(AgentError::new(ErrorCode::Plugin, "result transform changed immutable execution identity/status")); break; }
                Err(error) => { fatal = Some(error); break; }
            }
        }
    }
    if let Err(error) = result.validate() {
        result.content = "[invalid tool content omitted]".into();
        result.structured = None; result.artifact = None; result.truncated = true;
        if fatal.is_none() { fatal = Some(error); }
    }
    if let Some(error) = cap_result(&mut result, limits.max_tool_result_bytes) {
        if fatal.is_none() { fatal = Some(error); }
    }
    if let Err(error) = checkpoints.settled(&result).await {
        if fatal.is_none() { fatal = Some(error); }
    }
    bus.complete_tool(step, index, &result);
    (result, fatal)
}

/// Truncate only text. Never cut image encodings/JSON into malformed payloads.
/// Oversized rich output stops the Run after retaining an explicitly marked preview.
fn cap_result(result: &mut ToolResult, max: usize) -> Option<AgentError> {
    let bytes = result.payload_bytes();
    let rich = result.content.has_media() || result.structured.is_some() || result.artifact.is_some();
    if bytes <= max { return None; }
    result.original_bytes = result.original_bytes.max(bytes);
    let suffix = format!("\n[truncated: {} original bytes; omitted content is not retained by core]", result.original_bytes);
    let preview = result.content.preview();
    let prefix = clip_utf8(&preview, max.saturating_sub(suffix.len()));
    result.content = format!("{prefix}{suffix}").into();
    result.structured = None;
    result.artifact = None;
    result.truncated = true;
    if rich { Some(AgentError::new(ErrorCode::Limit, "rich tool output exceeds limit; transform/archive it or increase the run limit")) }
    else { None }
}
