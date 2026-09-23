mod support;
use api::*;
use runtime::HostBuilder;
use std::{sync::{Arc, Mutex, atomic::Ordering}, time::Duration};
use support::*;
use tokio::sync::Notify;

fn phase_name(phase: &CheckpointPhase) -> &'static str {
    match phase {
        CheckpointPhase::BeforeModel => "before", CheckpointPhase::AfterModel => "after_model",
        CheckpointPhase::InputApplied => "input", CheckpointPhase::ToolIntent { .. } => "intent",
        CheckpointPhase::ToolSettled { .. } => "settled", CheckpointPhase::AfterTools => "after_tools",
        CheckpointPhase::RunFinished => "finish",
    }
}
#[derive(Default)]
struct Recorder {
    records: Mutex<Vec<Arc<RunCheckpoint>>>,
    fail_at: Option<&'static str>,
    hang_at: Option<&'static str>,
}
#[async_trait]
impl CheckpointSink for Recorder {
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, cancel: CancellationToken) -> Result<()> {
        // Terminal settlement must get a fresh, usable cancellation token.
        assert!(!cancel.is_cancelled());
        let phase = phase_name(&checkpoint.phase);
        if self.hang_at == Some(phase) {
            cancel.cancelled().await;
            return Err(AgentError::new(ErrorCode::Cancelled, "sink stopped"));
        }
        if self.fail_at == Some(phase) {
            return Err(AgentError::new(ErrorCode::Tool, "backend password=PRIVATE_SECRET"));
        }
        self.records.lock().unwrap().push(checkpoint);
        Ok(())
    }
}

#[tokio::test]
async fn no_sink_keeps_checkpointing_optional() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![answer("ok")])).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert!(!report.checkpoint.configured);
    assert_eq!(report.checkpoint.last_acknowledged_revision, None);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn checkpoint_order_is_monotonic_and_prior_snapshots_are_immutable() {
    let store = Arc::new(Recorder::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a"), answer("ok")]))
        .tool(Arc::new(CountTool::default())).checkpoint_sink(store.clone()).unwrap().build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    let records = store.records.lock().unwrap().clone();
    assert_eq!(records.iter().map(|c| phase_name(&c.phase)).collect::<Vec<_>>(),
        vec!["before", "after_model", "intent", "settled", "after_tools", "before", "after_model", "finish"]);
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record.revision, index as u64 + 1);
        assert_eq!(record.run_id, report.run_id);
        assert_eq!(record.schema_version, CHECKPOINT_VERSION);
    }
    assert!(matches!(records[1].pending_tools.get("a"), Some(CheckpointToolState::Pending)));
    assert!(matches!(records[2].pending_tools.get("a"), Some(CheckpointToolState::IntentRecorded)));
    assert!(matches!(records[3].pending_tools.get("a"), Some(CheckpointToolState::Settled { result }) if result.status == ToolStatus::Success));
    assert!(records[4].pending_tools.is_empty());
    assert_eq!(records.last().unwrap().transcript, report.transcript);
    assert_eq!(records.last().unwrap().status, Some(RunStatus::Completed));
    assert_eq!(report.checkpoint.last_acknowledged_revision, Some(records.len() as u64));
    host.shutdown().await.unwrap();
}

struct WitnessTool { store: Arc<Recorder> }
#[async_trait]
impl Tool for WitnessTool {
    fn spec(&self) -> ToolSpec { CountTool::default().spec() }
    async fn execute(&self, ctx: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        let records = self.store.records.lock().unwrap();
        assert!(records.iter().any(|c| matches!(&c.phase, CheckpointPhase::ToolIntent { call_id } if call_id == &ctx.call_id)));
        Ok("intent was acknowledged before dispatch".into())
    }
}
#[tokio::test]
async fn tool_body_cannot_run_before_its_intent_is_acknowledged() {
    let store = Arc::new(Recorder::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a"), answer("ok")]))
        .tool(Arc::new(WitnessTool { store: store.clone() })).checkpoint_sink(store).unwrap().build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(results(&report)[0].status, ToolStatus::Success);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_before_model_commit_stops_without_calling_a_model() {
    let store = Arc::new(Recorder { fail_at: Some("before"), ..Default::default() });
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).checkpoint_sink(store).unwrap().build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Checkpoint);
    assert!(!report.error.as_ref().unwrap().message.contains("PRIVATE_SECRET"));
    assert!(model.requests.lock().unwrap().is_empty());
    assert_eq!(report.checkpoint.last_acknowledged_revision, None);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_after_model_or_intent_commit_never_dispatches_the_tool() {
    for phase in ["after_model", "intent"] {
        let store = Arc::new(Recorder { fail_at: Some(phase), ..Default::default() });
        let tool = Arc::new(CountTool::default());
        let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a")])).tool(tool.clone())
            .checkpoint_sink(store).unwrap().build().await.unwrap();
        let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Checkpoint);
        assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
        assert_eq!(results(&report)[0].status, ToolStatus::Skipped);
        api::validate_messages(&report.transcript).unwrap();
        host.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn failed_result_commit_preserves_known_effect_and_does_not_retry() {
    let store = Arc::new(Recorder { fail_at: Some("settled"), ..Default::default() });
    let tool = Arc::new(CountTool::default());
    let model = ScriptModel::new(vec![calls(&[("a", "count", serde_json::json!({"value":1})),
        ("b", "count", serde_json::json!({"value":2}))])]);
    let mut host = HostBuilder::new().model(model.clone()).tool(tool.clone()).checkpoint_sink(store).unwrap().build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.checkpoint.error.as_ref().unwrap().code, ErrorCode::Checkpoint);
    assert_eq!(results(&report)[0].status, ToolStatus::Success);
    assert_eq!(results(&report)[1].status, ToolStatus::Skipped);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_success_requires_terminal_commit_acknowledgement() {
    let store = Arc::new(Recorder { fail_at: Some("finish"), ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![answer("provisional answer")]))
        .checkpoint_sink(store).unwrap().build().await.unwrap();
    let handle = host.engine().start(RunRequest::new("go")).unwrap();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.output, None);
    assert_eq!(handle.snapshot().outcome.unwrap().status, RunStatus::Failed);
    assert_eq!(report.checkpoint.last_acknowledged_revision, Some(2));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_run_still_records_unknown_tool_result_and_final_status() {
    let store = Arc::new(Recorder::default());
    let tool = Arc::new(CountTool { wait_for_cancel: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a")])).tool(tool.clone())
        .checkpoint_sink(store.clone()).unwrap().build().await.unwrap();
    let handle = host.engine().start(RunRequest::new("go")).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tool.started.notified()).await.unwrap();
    handle.cancel();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Cancelled);
    assert_eq!(results(&report)[0].status, ToolStatus::Unknown);
    let records = store.records.lock().unwrap().clone();
    assert!(records.iter().any(|c| matches!(c.pending_tools.get("a"), Some(CheckpointToolState::Settled { result }) if result.status == ToolStatus::Unknown)));
    assert_eq!(records.last().unwrap().status, Some(RunStatus::Cancelled));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn checkpoint_timeout_is_bounded_and_does_not_leak_backend_errors() {
    let store = Arc::new(Recorder { hang_at: Some("before"), ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![])).checkpoint_sink(store).unwrap().build().await.unwrap();
    let mut request = RunRequest::new("go");
    request.limits.checkpoint_timeout = Duration::from_millis(10);
    request.limits.cancellation_grace = Duration::from_millis(10);
    let report = tokio::time::timeout(Duration::from_secs(2), host.engine().execute(request)).await.unwrap().unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Checkpoint);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversize_checkpoint_fails_before_model_or_storage_io() {
    let store = Arc::new(Recorder::default());
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).checkpoint_sink(store.clone()).unwrap().build().await.unwrap();
    let mut request = RunRequest::new("go"); request.limits.max_checkpoint_bytes = 16;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Checkpoint);
    assert!(model.requests.lock().unwrap().is_empty());
    assert!(store.records.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}

#[test]
fn duplicate_checkpoint_sink_registration_is_not_a_silent_override() {
    let builder = HostBuilder::new().checkpoint_sink(Arc::new(Recorder::default())).unwrap();
    assert!(builder.checkpoint_sink(Arc::new(Recorder::default())).is_err());
}

#[tokio::test]
async fn parallel_tool_commits_serialize_per_run_without_losing_partial_results() {
    let store = Arc::new(Recorder::default());
    let tool = Arc::new(CountTool { concurrency: ToolConcurrency::ParallelSafe, delay: Duration::from_millis(2), ..Default::default() });
    let model = ScriptModel::new(vec![calls(&[("a", "count", serde_json::json!({"value":1})),
        ("b", "count", serde_json::json!({"value":2}))]), answer("ok")]);
    let mut host = HostBuilder::new().model(model).tool(tool).checkpoint_sink(store.clone()).unwrap().build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    let records = store.records.lock().unwrap().clone();
    for pair in records.windows(2) { assert_eq!(pair[0].revision + 1, pair[1].revision); }
    let last_settled = records.iter().rev().find(|c| matches!(&c.phase, CheckpointPhase::ToolSettled { .. })).unwrap();
    assert!(last_settled.pending_tools.values().all(|v| matches!(v, CheckpointToolState::Settled { .. })));
    assert_eq!(last_settled.pending_tools.len(), 2);
    host.shutdown().await.unwrap();
}

struct GateInput { entered: Notify, release: Notify }
#[async_trait]
impl CheckpointSink for GateInput {
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, cancel: CancellationToken) -> Result<()> {
        if checkpoint.phase == CheckpointPhase::InputApplied {
            self.entered.notify_one();
            tokio::select! {
                _ = self.release.notified() => {},
                _ = cancel.cancelled() => return Err(AgentError::new(ErrorCode::Cancelled, "stopped")),
            }
        }
        Ok(())
    }
}
#[tokio::test]
async fn input_receipt_waits_for_checkpoint_ack() {
    let sink = Arc::new(GateInput { entered: Notify::new(), release: Notify::new() });
    let release = Arc::new(Notify::new());
    let tool = Arc::new(CountTool { release: Some(release.clone()), ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a"), answer("ok")])).tool(tool.clone())
        .checkpoint_sink(sink.clone()).unwrap().build().await.unwrap();
    let handle = host.engine().start(RunRequest::new("go")).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tool.started.notified()).await.unwrap();
    let session = handle.session();
    let steering = session.steer("extra input".into()); tokio::pin!(steering);
    // Poll the future once to enqueue, while the current tool is still blocked.
    tokio::select! { biased; _ = &mut steering => panic!("input applied too early"), _ = tokio::task::yield_now() => {} }
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), sink.entered.notified()).await.unwrap();
    tokio::select! { biased; _ = &mut steering => panic!("receipt preceded checkpoint ack"), _ = tokio::task::yield_now() => {} }
    sink.release.notify_one();
    steering.await.unwrap();
    assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
    host.shutdown().await.unwrap();
}
