use api::*;
use compaction::CompactionPlugin;
use runtime::HostBuilder;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

struct Source {
    bytes: usize,
}
#[async_trait]
impl ContextTransform for Source {
    async fn transform(&self, _: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        Ok(messages)
    }
    async fn sources(&self, _: &RunContext, _: &[Message]) -> Result<Vec<ContextBlock>> {
        Ok(vec![ContextBlock::new(
            "test.pinned",
            format!("PINNED_GUIDANCE {}", "R".repeat(self.bytes)),
        )])
    }
}
#[derive(Default)]
struct CapturingModel {
    summaries: AtomicUsize,
    primary: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl Model for CapturingModel {
    fn context_window_tokens(&self, _: &ModelOptions) -> Option<u64> {
        Some(1500)
    }
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let summary = request
            .messages
            .first()
            .is_some_and(|m| m.text().starts_with("Summarize the earlier"));
        if summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
        } else {
            self.primary.lock().unwrap().push(request);
        }
        let events = vec![
            ModelEvent::Text(
                if summary {
                    "preserved decisions"
                } else {
                    "done"
                }
                .into(),
            ),
            ModelEvent::Usage(Usage {
                input_tokens: if summary { 1_000_000 } else { 40 },
                output_tokens: 5,
            }),
            ModelEvent::Finish(FinishReason::Stop),
            ModelEvent::End,
        ];
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
#[tokio::test]
async fn pinned_source_budget_is_reserved_in_either_registration_order_without_replacing_user_anchor(
) {
    for source_first in [true, false] {
        let model = Arc::new(CapturingModel::default());
        let source = Arc::new(Source { bytes: 1000 });
        let compactor = Arc::new(CompactionPlugin::default().compactor());
        let mut builder = HostBuilder::new().model(model.clone());
        builder = if source_first {
            builder
                .context_transform(source)
                .context_transform(compactor)
        } else {
            builder
                .context_transform(compactor)
                .context_transform(source)
        };
        let mut host = builder.build().await.unwrap();
        let mut run = RunRequest::new("unused");
        run.limits.max_context_bytes = 4096;
        run.limits.max_output_tokens = 64;
        run.messages = vec![
            Message::system("HOST_POLICY"),
            Message::user("old ".repeat(580)),
            Message::user("LATEST_REAL_USER_REQUEST"),
        ];
        let original = run.messages.clone();
        let report = host.engine().execute(run).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
        assert!(model.summaries.load(Ordering::SeqCst) > 0);
        let requests = model.primary.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(serde_json::to_vec(request).unwrap().len() <= 4096);
        assert_eq!(
            request.messages.last().unwrap().text(),
            "LATEST_REAL_USER_REQUEST"
        );
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|m| m.text().contains("PINNED_GUIDANCE"))
                .count(),
            1
        );
        assert_eq!(&report.transcript[..original.len()], original.as_slice());
        assert!(!report
            .transcript
            .iter()
            .any(|m| m.text().contains("PINNED_GUIDANCE")));
        assert!(report.task_usage.reported_tokens >= 1_000_000);
        drop(requests);
        host.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn pinned_sources_too_large_are_rejected_before_any_model_call() {
    let model = Arc::new(CapturingModel::default());
    let mut host = HostBuilder::new()
        .model(model.clone())
        .context_transform(Arc::new(Source { bytes: 5000 }))
        .plugin(Arc::new(CompactionPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("real");
    run.limits.max_context_bytes = 4096;
    let report = host.engine().execute(run).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Limit);
    assert_eq!(model.summaries.load(Ordering::SeqCst), 0);
    assert!(model.primary.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}
