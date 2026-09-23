mod support;

use api::*;
use runtime::HostBuilder;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{atomic::{AtomicUsize, Ordering}, Arc},
    time::Duration,
};
use support::*;

struct FailingModel {
    calls: AtomicUsize,
    code: ErrorCode,
    partial: bool,
}
#[async_trait]
impl Model for FailingModel {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let error = AgentError::new(self.code, "original failure");
        if self.partial {
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::Text("PRIVATE_PARTIAL_ANSWER".into())),
                Err(error),
            ])))
        } else {
            Err(error)
        }
    }
}
fn failing(code: ErrorCode, partial: bool) -> Arc<FailingModel> {
    Arc::new(FailingModel { calls: AtomicUsize::new(0), code, partial })
}
fn retry_request() -> RunRequest {
    let mut request = RunRequest::new("go");
    request.limits.model_retry = ModelRetryPolicy {
        max_retries: 2,
        initial_delay: Duration::from_secs(10),
        max_delay: Duration::from_secs(10),
    };
    request
}

#[tokio::test]
async fn exhausted_call_budget_does_not_wait_for_retry() {
    let model = failing(ErrorCode::ModelServer, false);
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut request = retry_request();
    request.task = TaskControl::new(TaskLimits { max_model_calls: 1, ..Default::default() });
    let result = tokio::time::timeout(Duration::from_secs(1), host.engine().execute(request)).await;
    host.shutdown().await.unwrap();
    let report = result.expect("exhausted budget must fail before the ten-second retry delay").unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.model_requests.len(), 1);
    assert_eq!(report.model_requests[0].error.as_ref().unwrap().message, "original failure");
}

#[tokio::test]
async fn exhausted_audit_budget_does_not_wait_for_retry() {
    let model = failing(ErrorCode::ModelServer, false);
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut request = retry_request();
    request.limits.audit_mode = AuditMode::Metadata;
    request.limits.max_audit_bytes = 8192;
    let result = tokio::time::timeout(Duration::from_secs(1), host.engine().execute(request)).await;
    host.shutdown().await.unwrap();
    let report = result.expect("audit capacity must be checked before retry delay").unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(report.task_usage.model_calls, 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_wait_obeys_total_deadline() {
    let model = failing(ErrorCode::ModelRateLimit, false);
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut request = retry_request();
    request.task = TaskControl::new(TaskLimits { wall_time: Duration::from_millis(50), ..Default::default() });
    let report = tokio::time::timeout(Duration::from_secs(1), host.engine().execute(request)).await.unwrap().unwrap();
    assert_eq!(report.status, RunStatus::TimedOut);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn default_retry_and_quota_failure_do_not_retry() {
    for (code, opt_in) in [(ErrorCode::ModelServer, false), (ErrorCode::ModelQuota, true)] {
        let model = failing(code, false);
        let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
        let request = if opt_in { retry_request() } else { RunRequest::new("go") };
        let report = tokio::time::timeout(Duration::from_secs(1), host.engine().execute(request)).await.unwrap().unwrap();
        assert_eq!(report.error.as_ref().unwrap().code, code);
        assert_eq!(model.calls.load(Ordering::SeqCst), 1);
        host.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn metadata_audit_has_explicit_body_omission_and_no_partial_answer() {
    for mode in [AuditMode::Full, AuditMode::Metadata] {
        let mut host = HostBuilder::new().model(failing(ErrorCode::ModelTransport, true)).build().await.unwrap();
        let mut request = retry_request();
        request.limits.audit_mode = mode;
        let report = host.engine().execute(request).await.unwrap();
        assert_eq!(report.task_usage.model_calls, 1);
        assert!(report.error.as_ref().unwrap().model_output_started);
        let record = &report.model_requests[0];
        assert!(record.settled);
        if mode == AuditMode::Full {
            assert!(record.request.is_some());
            assert_eq!(record.partial_text.as_deref(), Some("PRIVATE_PARTIAL_ANSWER"));
        } else {
            assert!(record.request.is_none());
            assert!(record.partial_text.is_none(), "metadata must not retain model content");
            let json = serde_json::to_value(record).unwrap();
            assert_eq!(json.get("request"), Some(&Value::Null));
            assert!(!json.to_string().contains("PRIVATE_PARTIAL_ANSWER"));
        }
        host.shutdown().await.unwrap();
    }
}

struct NestedTool(AtomicUsize);
#[async_trait]
impl Tool for NestedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "nested".into(), description: "Schema diagnostic fixture".into(),
            parameters: json!({"type":"object","required":["path","limit","edits"],
                "properties":{"path":{"type":"string"},"limit":{"type":"integer","exclusiveMinimum":0},
                    "edits":{"type":"array","items":{"type":"object","required":["newText"],
                        "properties":{"newText":{"type":"string"}}}}}}),
            concurrency: ToolConcurrency::Exclusive, side_effects: false,
        }
    }
    async fn execute(&self, _: ToolContext, _: Value) -> Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("valid arguments".into())
    }
}
#[tokio::test]
async fn nested_schema_diagnostics_allow_model_correction_without_dispatching_invalid_args() {
    let tool = Arc::new(NestedTool(AtomicUsize::new(0)));
    let model = ScriptModel::new(vec![
        calls(&[("bad", "nested", json!({"limit":0,"edits":[{"newText":17}]}))]),
        calls(&[("good", "nested", json!({"path":"a.txt","limit":1,"edits":[{"newText":"ok"}]}))]),
        answer("done"),
    ]);
    let mut host = HostBuilder::new().model(model).tool(tool.clone()).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    let results = results(&report);
    let diagnostic = results[0].content.text();
    assert!(diagnostic.contains("/path: is required"), "{diagnostic}");
    assert!(diagnostic.contains("/limit: must be > 0"), "{diagnostic}");
    assert!(diagnostic.contains("/edits/0/newText: expected string"), "{diagnostic}");
    assert!(diagnostic.len() <= 1024);
    assert_eq!(results[0].status, ToolStatus::Error);
    assert_eq!(results[1].status, ToolStatus::Success);
    assert_eq!(tool.0.load(Ordering::SeqCst), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_history_without_compaction_is_rejected_before_model_call() {
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut request = RunRequest::new("x".repeat(3000));
    request.limits.max_context_bytes = 1024;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert!(model.requests.lock().unwrap().is_empty());
    let mut beyond_admission = RunRequest::new("x".repeat(5000));
    beyond_admission.limits.max_context_bytes = 1024;
    beyond_admission.limits.max_initial_history_bytes = 4096;
    assert!(host.engine().start(beyond_admission).is_err());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn every_tool_emits_exactly_one_terminal_event_after_snapshot_eviction() {
    let count = UI_RETAINED_ITEMS + 33;
    let ids = (0..count).map(|index| format!("call-{index}")).collect::<Vec<_>>();
    let inputs = ids.iter().map(|id| (id.as_str(), "count", json!({"value":1}))).collect::<Vec<_>>();
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![calls(&inputs), answer("done")]))
        .tool(tool.clone()).event_capacity(4096).build().await.unwrap();
    let mut request = RunRequest::new("go");
    request.limits.max_tools_per_step = count;
    let mut handle = host.engine().start(request).unwrap();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    let mut completed = BTreeSet::new();
    while let Ok(envelope) = handle.events.try_recv() {
        if let RunEvent::ItemCompleted { item } = envelope.event {
            if let ItemContent::ToolCall { call_id: Some(id), result: Some(result), .. } = item.content {
                assert!(completed.insert(id), "duplicate tool completion");
                assert_eq!(result.status, ToolStatus::Success);
            }
        }
    }
    assert_eq!(completed.len(), count);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), count);
    assert_eq!(results(&report).len(), count);
    assert_eq!(handle.snapshot().items.len(), UI_RETAINED_ITEMS);
    assert!(handle.snapshot().pruned_items > 0);
    host.shutdown().await.unwrap();
}
