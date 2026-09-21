use crate::{checkpoint::Checkpoints, events::{EventBus, ProgressSink}, gate::{bounded, CancelOnDrop}, host::Registry, validation::valid_name};
use agent_api::*;
use futures_util::{stream::FuturesUnordered, StreamExt};
use jsonschema::{Draft, JSONSchema};
use std::{collections::{BTreeSet, VecDeque}, sync::Arc};
use tokio::time::Instant;

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
        let validator = JSONSchema::options().with_draft(Draft::Draft7).compile(&spec.parameters)
            .map_err(|e| AgentError::new(ErrorCode::Schema, format!("invalid tool schema: {e}")))?;
        Ok(Self { spec, implementation, validator })
    }
}
fn reject_external_refs(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(reference) = map.get("$ref") {
                if !reference.as_str().is_some_and(|s| s.starts_with('#')) {
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

struct Ready { call: ToolCall, tool: Arc<CompiledTool> }

pub(crate) async fn execute_batch(
    context: RunContext, registry: Arc<Registry>, limits: RunLimits,
    allowed_effects: &BTreeSet<String>, offered: &BTreeSet<String>, calls: &[ToolCall], bus: EventBus, step: usize, checkpoints: Checkpoints,
) -> (Vec<ToolResult>, Option<AgentError>) {
    let mut settled: Vec<Option<ToolResult>> = vec![None; calls.len()];
    let mut prepared = vec![];
    // All policies run before this batch starts causing side effects.
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
                return Ok(Err(ToolResult::new(&call.id, ToolStatus::Error, "arguments do not match the tool's JSON Schema")));
            }
            for policy in &registry.policies {
                let operation = context.cancel.child_token();
                let _drop_cancel = CancelOnDrop(operation.clone());
                let mut policy_ctx = context.clone(); policy_ctx.cancel = operation.clone();
                let decision = bounded(&context.cancel, &operation,
                    context.task.deadline().min(Instant::now() + limits.hook_timeout), limits.cancellation_grace,
                    policy.check(&policy_ctx, call, &tool.spec)).await?;
                if let PolicyDecision::Deny(reason) = decision {
                    return Ok(Err(ToolResult::new(&call.id, ToolStatus::Denied, clip_utf8(&reason, 4096))));
                }
            }
            // Final, non-relaxable host gate. Plugin metadata is trusted, not a sandbox.
            if tool.spec.side_effects && !allowed_effects.contains(&tool.spec.name) {
                return Ok(Err(ToolResult::new(&call.id, ToolStatus::Denied, "side-effect tool not authorized by host")));
            }
            Ok(Ok(Ready { call: call.clone(), tool: tool.clone() }))
        }.await;
        match check {
            Ok(Ok(ready)) => prepared.push((index, ready)),
            Ok(Err(result)) => settled[index] = Some(result),
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
                let Some((index, ready)) = queue.pop_front() else { break; };
                running.push(run_indexed(index, ready, group_ctx.clone(), registry.clone(), limits.clone(), bus.clone(), step, checkpoints.clone()));
            }
            let Some((index, result, error)) = running.next().await else { break; };
            settled[index] = Some(result);
            if let Some(error) = error {
                if fatal.is_none() { fatal = Some(error); group_cancel.cancel(); }
            }
        }
        for (index, ready) in queue {
            settled[index] = Some(ToolResult::new(&ready.call.id, ToolStatus::Skipped, "parallel group stopped before execution"));
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
    bus.start_tool(step, index);
    let operation = ctx.cancel.child_token();
    let _drop_cancel = CancelOnDrop(operation.clone());
    let mut tool_run = ctx.clone(); tool_run.cancel = operation.clone();
    let tool_ctx = ToolContext { run: tool_run, call_id: call.id.clone(),
        progress: Arc::new(ProgressSink { bus: bus.clone(), item_id: tool_item_id(step, index) }) };
    let execution = bounded(&ctx.cancel, &operation,
        ctx.task.deadline().min(Instant::now() + limits.tool_timeout), limits.cancellation_grace,
        ready.tool.implementation.execute(tool_ctx, call.arguments.clone())).await;
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
