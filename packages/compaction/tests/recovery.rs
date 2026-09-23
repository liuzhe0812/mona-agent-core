use api::*;
use compaction::{CompactionConfig, CompactionPlugin};
use runtime::HostBuilder;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};

#[derive(Default)]
struct RecoveryModel {
    summaries: AtomicUsize,
    primary: AtomicUsize,
    primary_bytes: Mutex<Vec<usize>>,
    no_progress: bool,
    repeated_overflow: bool,
}
#[async_trait]
impl Model for RecoveryModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let summary = request.messages[0].text().starts_with("Summarize the earlier");
        let text = if summary {
            let index = self.summaries.fetch_add(1, Ordering::SeqCst);
            if index == 0 || self.no_progress { "S".repeat(600) } else { "preserved requirements".into() }
        } else {
            self.primary_bytes.lock().unwrap().push(serde_json::to_vec(&request).unwrap().len());
            if self.primary.fetch_add(1, Ordering::SeqCst) == 0 || self.repeated_overflow {
                return Err(AgentError::new(ErrorCode::ModelContextWindow, "explicit context overflow"));
            }
            "finished".into()
        };
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(text)),
            Ok(ModelEvent::Usage(Usage { input_tokens: if summary { 100_000 } else { 10 }, output_tokens: 5 })),
            Ok(ModelEvent::Finish(FinishReason::Stop)), Ok(ModelEvent::End),
        ])))
    }
}
async fn execute(model: Arc<RecoveryModel>) -> Arc<RunReport> {
    let config = CompactionConfig {
        trigger_percent: 50, target_percent: 40, recent_groups: 1, ..Default::default()
    };
    let mut host = HostBuilder::new().model(model)
        .plugin(Arc::new(CompactionPlugin::new(config).unwrap())).build().await.unwrap();
    let mut request = RunRequest::new("unused");
    request.messages = vec![Message::user("old ".repeat(400)), Message::user("current request")];
    let original = request.messages.clone();
    request.limits.max_context_bytes = 3000;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.transcript[..original.len()], original);
    host.shutdown().await.unwrap();
    report
}

#[tokio::test]
async fn already_compacted_history_gets_one_smaller_cached_summary_on_overflow() {
    let model = Arc::new(RecoveryModel::default());
    let report = execute(model.clone()).await;
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(model.summaries.load(Ordering::SeqCst), 2);
    assert_eq!(model.primary.load(Ordering::SeqCst), 2);
    let sizes = model.primary_bytes.lock().unwrap();
    assert!(sizes[1] < sizes[0]);
    assert_eq!(report.model_requests.len(), 4);
    assert!(report.task_usage.reported_tokens > 200_000, "summary usage is accounted, not fed into the main context-size estimate");
}

#[tokio::test]
async fn unchanged_summary_stops_without_resending_the_primary_request() {
    let model = Arc::new(RecoveryModel { no_progress: true, ..Default::default() });
    let report = execute(model.clone()).await;
    assert_ne!(report.status, RunStatus::Completed);
    assert_eq!(model.summaries.load(Ordering::SeqCst), 2);
    assert_eq!(model.primary.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_second_context_overflow_does_not_start_another_recovery_cycle() {
    let model = Arc::new(RecoveryModel { repeated_overflow: true, ..Default::default() });
    let report = execute(model.clone()).await;
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::ModelContextWindow);
    assert_eq!(model.summaries.load(Ordering::SeqCst), 2);
    assert_eq!(model.primary.load(Ordering::SeqCst), 2);
}
