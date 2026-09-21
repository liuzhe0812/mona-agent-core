mod support;
use agent_api::*;
use agent_core::{validate_messages, HostBuilder};
use serde_json::json;
use std::{sync::{Arc, atomic::Ordering}, time::Duration};
use support::*;
use tokio::sync::Notify;

#[tokio::test]
async fn normal_react_roundtrip_has_paired_results_and_audits() {
    let model = ScriptModel::new(vec![call("one"), answer("done")]);
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(model.clone()).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.output.as_deref(), Some("done"));
    assert_eq!(report.steps, 2);
    assert_eq!(report.model_requests.len(), 2);
    assert_eq!(report.transcript.len(), 4);
    assert_eq!(results(&report)[0].status, ToolStatus::Success);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    validate_messages(&report.transcript).unwrap();
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn complete_json_is_required_before_tool_execution() {
    let events = vec![ModelEvent::ToolDelta { index: 0, id: Some("bad".into()), name: Some("count".into()), arguments: "{\"value\":".into() },
        ModelEvent::Finish(FinishReason::ToolCalls), ModelEvent::End];
    let model = ScriptModel::new(vec![events]);
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(model).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::ModelProtocol);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    assert!(report.model_requests[0].settled);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn truncated_output_never_executes_even_valid_tool_arguments() {
    let mut events = call("one");
    events[1] = ModelEvent::Finish(FinishReason::Length);
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![events])).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::ModelTruncated);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_transport_end_never_executes_a_tool() {
    let mut events = call("one"); events.pop();
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![events])).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::ModelTransport);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn schema_error_is_a_result_not_a_tool_invocation() {
    let tool = Arc::new(CountTool::default());
    let model = ScriptModel::new(vec![calls(&[("bad", "count", json!({"value":"wrong"}))]), answer("correct it")]);
    let mut host = HostBuilder::new().model(model).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(results(&report)[0].status, ToolStatus::Error);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

struct Allow;
#[async_trait]
impl ToolPolicy for Allow {
    async fn check(&self, _: &RunContext, _: &ToolCall, _: &ToolSpec) -> Result<PolicyDecision> { Ok(PolicyDecision::Allow) }
}
#[tokio::test]
async fn plugin_allow_cannot_bypass_host_side_effect_gate() {
    let tool = Arc::new(CountTool { effects: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one"), answer("denied")]))
        .tool(tool.clone()).policy(Arc::new(Allow)).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(results(&report)[0].status, ToolStatus::Denied);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}
struct BrokenPolicy;
#[async_trait]
impl ToolPolicy for BrokenPolicy {
    async fn check(&self, _: &RunContext, _: &ToolCall, _: &ToolSpec) -> Result<PolicyDecision> {
        Err(AgentError::new(ErrorCode::Policy, "authorization backend unavailable"))
    }
}
#[tokio::test]
async fn failed_policy_is_fail_closed() {
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")]))
        .tool(tool.clone()).policy(Arc::new(BrokenPolicy)).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(results(&report)[0].status, ToolStatus::Skipped);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn repeated_call_id_is_not_replayed() {
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("same"), call("same")]))
        .tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    validate_messages(&report.transcript).unwrap();
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn step_limit_stops_after_settling_tool_results() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")]))
        .tool(Arc::new(CountTool::default())).build().await.unwrap();
    let mut request = RunRequest::new("run"); request.limits.max_steps = 1;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(results(&report).len(), 1);
    validate_messages(&report.transcript).unwrap();
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn reported_token_breaker_blocks_tool_execution_after_model() {
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")])).tool(tool.clone()).build().await.unwrap();
    let mut request = RunRequest::new("run");
    request.task = TaskControl::new(TaskLimits { max_reported_tokens: Some(1), ..Default::default() });
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(report.task_usage.reported_tokens, 15);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn usage_omission_is_not_reported_as_zero_cost_certainty() {
    let model = ScriptModel::new(vec![vec![ModelEvent::Text("ok".into()), ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]]);
    let mut host = HostBuilder::new().model(model).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert!(!report.task_usage.usage_complete);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_unicode_result_is_marked_and_bounded() {
    let tool = Arc::new(CountTool { payload: Some("中".repeat(1000)), ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one"), answer("ok")])).tool(tool).build().await.unwrap();
    let mut request = RunRequest::new("run"); request.limits.max_tool_result_bytes = 256;
    let report = host.engine().execute(request).await.unwrap();
    let found = results(&report); let result = found[0];
    assert!(result.content.byte_len() <= 256);
    assert!(result.truncated);
    assert_eq!(result.original_bytes, 3000);
    host.shutdown().await.unwrap();
}
struct LyingTransform;
#[async_trait]
impl ResultTransform for LyingTransform {
    async fn transform(&self, _: &RunContext, _: &ToolCall, mut result: ToolResult) -> Result<ToolResult> {
        result.status = ToolStatus::Denied; Ok(result)
    }
}
#[tokio::test]
async fn result_transform_cannot_rewrite_execution_status() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")]))
        .tool(Arc::new(CountTool::default())).result_transform(Arc::new(LyingTransform)).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(results(&report)[0].status, ToolStatus::Success);
    host.shutdown().await.unwrap();
}
struct OrphanTransform;
#[async_trait]
impl ContextTransform for OrphanTransform {
    async fn transform(&self, _: &RunContext, mut messages: Vec<Message>) -> Result<Vec<Message>> {
        messages.push(Message::Tool { result: ToolResult::new("missing", ToolStatus::Success, "bad") }); Ok(messages)
    }
}
#[tokio::test]
async fn context_transform_must_preserve_valid_call_result_pairing() {
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).context_transform(Arc::new(OrphanTransform)).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert!(model.requests.lock().unwrap().is_empty());
    assert_eq!(report.transcript.len(), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_stops_started_tool_and_preserves_unknown_outcome() {
    let tool = Arc::new(CountTool { wait_for_cancel: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")])).tool(tool.clone()).build().await.unwrap();
    let handle = host.engine().start(RunRequest::new("run")).unwrap();
    tool.started.notified().await;
    handle.cancel();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Cancelled);
    assert_eq!(results(&report)[0].status, ToolStatus::Unknown);
    assert_eq!(tool.probe.active.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn tool_timeout_does_not_retry() {
    let tool = Arc::new(CountTool { wait_for_cancel: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")])).tool(tool.clone()).build().await.unwrap();
    let mut request = RunRequest::new("run"); request.limits.tool_timeout = Duration::from_millis(20);
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::TimedOut);
    assert_eq!(results(&report)[0].status, ToolStatus::Unknown);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn tool_panic_is_captured_without_claiming_rollback() {
    let tool = Arc::new(CountTool { panic: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")])).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Panic);
    assert_eq!(results(&report)[0].status, ToolStatus::Unknown);
    assert_eq!(tool.probe.active.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_drains_before_revoking_and_old_engine_cannot_start() {
    let tool = Arc::new(CountTool { wait_for_cancel: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")])).tool(tool.clone()).build().await.unwrap();
    let engine = host.engine();
    let handle = engine.start(RunRequest::new("run")).unwrap();
    tool.started.notified().await;
    host.shutdown().await.unwrap();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Cancelled);
    assert!(engine.start(RunRequest::new("late")).is_err());
    assert!(host.services().is_err());
}

#[tokio::test]
async fn slow_event_reader_does_not_lose_final_result() {
    let mut events = (0..100).map(|_| ModelEvent::Text("x".into())).collect::<Vec<_>>();
    events.extend([ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]);
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![events])).event_capacity(2).build().await.unwrap();
    let mut handle = host.engine().start(RunRequest::new("run")).unwrap();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.output.as_ref().unwrap().len(), 100);
    assert!(matches!(handle.events.recv().await, Err(tokio::sync::broadcast::error::RecvError::Lagged(_))));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn parallel_tools_are_bounded_exclusive_is_a_barrier_results_stay_ordered() {
    let probe = Arc::new(Probe::default());
    let read = Arc::new(CountTool { name: "read", probe: probe.clone(), concurrency: ToolConcurrency::ParallelSafe,
        delay: Duration::from_millis(20), ..Default::default() });
    let write = Arc::new(CountTool { name: "write", probe: probe.clone(), effects: true, ..Default::default() });
    let request_calls = calls(&[("a","read",json!({"value":1})),("b","read",json!({"value":2})),
        ("c","write",json!({"value":3})),("d","read",json!({"value":4})),("e","read",json!({"value":5}))]);
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![request_calls, answer("done")]))
        .tool(read).tool(write).allow_side_effect_tool("write").build().await.unwrap();
    let mut request = RunRequest::new("run"); request.limits.max_parallel_tools = 2;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    assert_eq!(probe.exclusive_overlap.load(Ordering::SeqCst), 0);
    assert_eq!(results(&report).iter().map(|r| r.call_id.as_str()).collect::<Vec<_>>(), vec!["a","b","c","d","e"]);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_never_starts_queued_tools_after_the_first() {
    let tool = Arc::new(CountTool { wait_for_cancel: true, concurrency: ToolConcurrency::ParallelSafe, ..Default::default() });
    let events = calls(&[("a","count",json!({"value":1})),("b","count",json!({"value":2})),("c","count",json!({"value":3}))]);
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![events])).tool(tool.clone()).build().await.unwrap();
    let mut request = RunRequest::new("run"); request.limits.max_parallel_tools = 1;
    let handle = host.engine().start(request).unwrap();
    tool.started.notified().await; handle.cancel();
    let report = handle.wait().await.unwrap();
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    assert_eq!(results(&report).len(), 3);
    assert_eq!(results(&report)[1].status, ToolStatus::Skipped);
    assert_eq!(results(&report)[2].status, ToolStatus::Skipped);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn steering_is_applied_only_after_tool_batch_settlement() {
    let release = Arc::new(Notify::new());
    let tool = Arc::new(CountTool { release: Some(release.clone()), ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one"), answer("done")])).tool(tool.clone()).build().await.unwrap();
    let handle = host.engine().start(RunRequest::new("original")).unwrap();
    tool.started.notified().await;
    let (applied, _) = tokio::join!(biased; handle.steer("new input"), async { release.notify_one(); });
    applied.unwrap();
    let report = handle.wait().await.unwrap();
    let index = report.transcript.iter().position(|m| matches!(m, Message::User { content } if content.text() == "new input")).unwrap();
    assert!(matches!(&report.transcript[index-1], Message::Tool { .. }));
    assert!(report.model_requests[1].request.messages.iter().any(|m| m.text() == "new input"));
    host.shutdown().await.unwrap();
}

struct Echo;
#[async_trait]
impl Model for Echo {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let text = request.messages.last().unwrap().text().to_owned();
        Ok(Box::pin(futures_util::stream::iter(answer(&text).into_iter().map(Ok))))
    }
}
#[tokio::test]
async fn concurrent_runs_do_not_share_transcripts_or_ids() {
    let mut host = HostBuilder::new().model(Arc::new(Echo)).build().await.unwrap();
    let engine = host.engine();
    let a = engine.start(RunRequest::new("A")).unwrap();
    let b = engine.start(RunRequest::new("B")).unwrap();
    let (a, b) = tokio::join!(a.wait(), b.wait());
    let a = a.unwrap(); let b = b.unwrap();
    assert_ne!(a.run_id, b.run_id);
    assert_eq!(a.output.as_deref(), Some("A"));
    assert_eq!(b.output.as_deref(), Some("B"));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn task_call_budget_is_atomic_across_concurrent_users() {
    let task = TaskControl::new(TaskLimits { max_model_calls: 3, ..Default::default() });
    let mut joins = vec![];
    for _ in 0..20 { let task = task.clone(); joins.push(tokio::spawn(async move { task.reserve_model_call().is_ok() })); }
    let mut admitted = 0;
    for join in joins { if join.await.unwrap() { admitted += 1; } }
    assert_eq!(admitted, 3);
    assert_eq!(task.usage().model_calls, 3);
}


struct AuxiliaryModelCall;
#[async_trait]
impl ContextTransform for AuxiliaryModelCall {
    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        ctx.model.complete(ModelRequest { messages: vec![Message::user("auxiliary work")], tools: vec![], max_output_tokens: 32, options: ModelOptions::default() }, None).await?;
        Ok(messages)
    }
}
#[tokio::test]
async fn plugin_auxiliary_model_calls_are_counted_against_the_same_budget() {
    let model = ScriptModel::new(vec![answer("auxiliary result")]);
    let mut host = HostBuilder::new().model(model.clone()).context_transform(Arc::new(AuxiliaryModelCall)).build().await.unwrap();
    let mut request = RunRequest::new("main work");
    request.task = TaskControl::new(TaskLimits { max_model_calls: 1, ..Default::default() });
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(report.task_usage.model_calls, 1);
    assert_eq!(report.model_requests[0].request.messages[0].text(), "auxiliary work");
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn fragmented_arguments_assemble_before_execution() {
    let mut events = vec![
        ModelEvent::ToolDelta { index: 0, id: Some("one".into()), name: Some("count".into()), arguments: "{\"val".into() },
        ModelEvent::ToolDelta { index: 0, id: None, name: None, arguments: "ue\":1}".into() },
        ModelEvent::Finish(FinishReason::ToolCalls), ModelEvent::End,
    ];
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![std::mem::take(&mut events), answer("done")]))
        .tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("run")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_audit_limit_rejects_before_network_call() {
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut request = RunRequest::new("run"); request.limits.max_audit_bytes = 1;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert!(model.requests.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}

#[test]
fn orphaned_history_is_rejected() {
    let messages = vec![Message::user("x"), Message::Tool { result: ToolResult::new("missing", ToolStatus::Success, "x") }];
    assert!(validate_messages(&messages).is_err());
}

struct RemoteSchemaTool;
#[async_trait]
impl Tool for RemoteSchemaTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "remote".into(), description: "bad schema".into(),
            parameters: json!({"type":"object","properties":{"x":{"$ref":"https://example.invalid/schema.json"}}}),
            concurrency: ToolConcurrency::Exclusive, side_effects: false }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> { Ok("never".into()) }
}
#[tokio::test]
async fn external_schema_references_are_rejected_at_registration() {
    assert!(HostBuilder::new().model(ScriptModel::new(vec![])).tool(Arc::new(RemoteSchemaTool)).build().await.is_err());
}

#[tokio::test]
async fn dropping_execute_future_cancels_child_but_not_shared_task() {
    let tool = Arc::new(CountTool { wait_for_cancel: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("one")]))
        .tool(tool.clone()).build().await.unwrap();
    let request = RunRequest::new("run");
    let task_control = request.task.clone();
    let engine = host.engine();
    let waiter = tokio::spawn(async move { engine.execute(request).await });
    tool.started.notified().await;
    waiter.abort();
    let _ = waiter.await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while tool.probe.active.load(Ordering::SeqCst) != 0 { tokio::task::yield_now().await; }
    }).await.unwrap();
    assert!(!task_control.cancellation().is_cancelled());
    host.shutdown().await.unwrap();
}
