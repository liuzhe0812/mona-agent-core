use api::*;
use runtime::HostBuilder;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Fixture {
    window: Option<u64>,
    summaries: AtomicUsize,
}
#[async_trait]
impl Model for Fixture {
    fn context_window_tokens(&self, _: &ModelOptions) -> Option<u64> {
        self.window
    }
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let summary = request.messages[0]
            .text()
            .starts_with("Summarize the earlier");
        if summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
        } else {
            assert_eq!(request.tools.len(), 1);
            assert_eq!(request.tools[0].name, "lookup");
            assert!(serde_json::to_vec(&request).unwrap().len() < 2048);
        }
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(
                if summary {
                    serde_json::to_string(&compaction::TaskSummary { goal: "retained decisions".into(), ..Default::default() }).unwrap()
                } else {
                    "done".into()
                }
                .into(),
            )),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}
struct Declared(&'static str, usize);
#[async_trait]
impl Tool for Declared {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.0.into(),
            description: "x".repeat(self.1),
            parameters: serde_json::json!({"type":"object"}),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        panic!("not requested")
    }
}
struct Lookup(AtomicUsize);
#[async_trait]
impl ToolSelector for Lookup {
    async fn select(
        &self,
        _: &RunContext,
        _: usize,
        _: &[Message],
        _: &[ToolSpec],
    ) -> Result<Vec<String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec!["lookup".into()])
    }
}
#[tokio::test]
async fn real_compaction_uses_selected_schemas_for_known_windows_and_byte_only_models() {
    for window in [None, Some(1500)] {
        for long_history in [false, true] {
            let model = Arc::new(Fixture {
                window,
                summaries: AtomicUsize::new(0),
            });
            let selector = Arc::new(Lookup(AtomicUsize::new(0)));
            let mut host = HostBuilder::new()
                .model(model.clone())
                .tool(Arc::new(Declared("lookup", 30)))
                .tool(Arc::new(Declared("unused", 16000)))
                .tool_selector(selector.clone())
                .plugin(Arc::new(compaction::CompactionPlugin::default()))
                .build()
                .await
                .unwrap();
            let mut run = RunRequest::new("current request");
            run.limits.max_context_bytes = 2048;
            run.limits.max_output_tokens = 64;
            if long_history {
                run.messages.splice(0..0, [Message::user("old context ".repeat(55)),
                    Message::user("old context ".repeat(55)), Message::user("old decisions ".repeat(40))]);
            }
            let original = run.messages.clone();
            let report = host.engine().execute(run).await.unwrap();
            assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
            assert_eq!(selector.0.load(Ordering::SeqCst), 1);
            assert_eq!(model.summaries.load(Ordering::SeqCst) > 0, long_history);
            assert_eq!(&report.transcript[..original.len()], original.as_slice());
            host.shutdown().await.unwrap();
        }
    }
}
