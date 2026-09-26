mod support;
use api::*;
use runtime::HostBuilder;
use std::{sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}, time::Duration};
use support::*;

#[derive(Default)]
struct AfterToolFailure { requests: Mutex<Vec<ModelRequest>>, calls: AtomicUsize }
#[async_trait]
impl Model for AfterToolFailure {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        self.requests.lock().unwrap().push(request);
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => call("once"),
            1 => return Err(AgentError::new(ErrorCode::ModelServer, "temporary")),
            2 => answer("finished"),
            _ => panic!("unbounded retry"),
        };
        Ok(Box::pin(futures_util::stream::iter(events.into_iter().map(Ok))))
    }
}
#[tokio::test]
async fn retry_reuses_only_the_failed_model_request_not_executed_tools() {
    let model = Arc::new(AfterToolFailure::default());
    let tool = Arc::new(CountTool { effects: true, ..Default::default() });
    let mut host = HostBuilder::new().model(model.clone()).tool(tool.clone())
        .allow_side_effect_tool("count").build().await.unwrap();
    let mut request = RunRequest::new("go");
    request.limits.model_retry = ModelRetryPolicy {
        max_retries: 1, initial_delay: Duration::from_millis(1), max_delay: Duration::from_millis(1),
    };
    request.task = TaskControl::new(TaskLimits { max_model_calls: 3, ..Default::default() });
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(report.model_requests.len(), 3);
    assert_eq!(report.task_usage.model_calls, 3);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    assert_eq!(results(&report).len(), 1);
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(serde_json::to_value(&requests[1]).unwrap(), serde_json::to_value(&requests[2]).unwrap());
    }
    assert_eq!(report.model_requests.iter().map(|record| record.call_number).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert!(report.model_requests.iter().all(|record| record.settled));
    host.shutdown().await.unwrap();
}

struct FailCheckpoint { at_settlement: bool }
#[async_trait]
impl CheckpointSink for FailCheckpoint {
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, _: CancellationToken) -> Result<()> {
        if (!self.at_settlement && matches!(checkpoint.phase, CheckpointPhase::ToolIntent { .. }))
            || (self.at_settlement && matches!(checkpoint.phase, CheckpointPhase::ToolSettled { .. })) {
            return Err(AgentError::new(ErrorCode::Checkpoint, "storage failed"));
        }
        Ok(())
    }
}
#[tokio::test]
async fn metadata_audit_does_not_weaken_intent_or_settlement_checkpoint_barriers() {
    for at_settlement in [false, true] {
        let tool = Arc::new(CountTool::default());
        let model = ScriptModel::new(vec![calls(&[
            ("a", "count", serde_json::json!({"value":1})),
            ("b", "count", serde_json::json!({"value":2})),
        ])]);
        let mut host = HostBuilder::new().model(model.clone()).tool(tool.clone())
            .checkpoint_sink(Arc::new(FailCheckpoint { at_settlement })).unwrap().build().await.unwrap();
        let mut request = RunRequest::new("go");
        request.limits.audit_mode = AuditMode::Metadata;
        let report = host.engine().execute(request).await.unwrap();
        assert_eq!(report.status, RunStatus::Failed);
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Checkpoint);
        assert_eq!(tool.probe.count.load(Ordering::SeqCst), usize::from(at_settlement));
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        assert!(report.model_requests.iter().all(|record| record.request.is_none()));
        let results = results(&report);
        assert_eq!(results[0].status, if at_settlement { ToolStatus::Success } else { ToolStatus::Skipped });
        assert_eq!(results[1].status, ToolStatus::Skipped);
        api::validate_messages(&report.transcript).unwrap();
        host.shutdown().await.unwrap();
    }
}

struct UsageFailure;
#[async_trait]
impl Model for UsageFailure {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Usage(Usage { input_tokens: 10, output_tokens: 0, cache_read_tokens: None, cache_write_tokens: None })),
            Err(AgentError::new(ErrorCode::ModelServer, "temporary after usage")),
        ])))
    }
}
#[tokio::test]
async fn reported_token_breaker_stops_retry_before_waiting() {
    let mut host = HostBuilder::new().model(Arc::new(UsageFailure)).build().await.unwrap();
    let mut request = RunRequest::new("go");
    request.task = TaskControl::new(TaskLimits { max_reported_tokens: Some(10), ..Default::default() });
    request.limits.model_retry = ModelRetryPolicy {
        max_retries: 1, initial_delay: Duration::from_secs(10), max_delay: Duration::from_secs(10),
    };
    let report = tokio::time::timeout(Duration::from_secs(1), host.engine().execute(request)).await.unwrap().unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(report.task_usage.model_calls, 1);
    assert_eq!(report.task_usage.reported_tokens, 10);
    assert_eq!(report.model_requests.len(), 1);
    assert_eq!(report.model_requests[0].error.as_ref().unwrap().code, ErrorCode::ModelServer);
    host.shutdown().await.unwrap();
}
