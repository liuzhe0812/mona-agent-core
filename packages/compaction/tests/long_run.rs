use api::*;
use compaction::CompactionPlugin;
use runtime::HostBuilder;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct LoopModel {
    main_calls: AtomicUsize,
    summaries: AtomicUsize,
    bad_summary: bool,
}
#[async_trait]
impl Model for LoopModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let summary = request
            .messages
            .first()
            .is_some_and(|m| m.text().starts_with("Summarize the earlier"));
        let events = if summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            vec![
                ModelEvent::Text(if self.bad_summary {
                    serde_json::to_string(&compaction::TaskSummary {goal:"S".repeat(2800),..Default::default()}).unwrap()
                } else {
                    serde_json::to_string(&compaction::TaskSummary { goal: "Earlier tool work completed; keep going.".into(), ..Default::default() }).unwrap()
                }),
                ModelEvent::Finish(FinishReason::Stop),
                ModelEvent::End,
            ]
        } else {
            let round = self.main_calls.fetch_add(1, Ordering::SeqCst);
            assert!(request
                .messages
                .iter()
                .any(|m| m.text() == "Complete seven tool calls."));
            if round == 7 {
                vec![
                    ModelEvent::Text("done".into()),
                    ModelEvent::Finish(FinishReason::Stop),
                    ModelEvent::End,
                ]
            } else {
                vec![
                    ModelEvent::ToolDelta {
                        index: 0,
                        id: Some(format!("call-{round}")),
                        name: Some("work".into()),
                        arguments: "{}".into(),
                    },
                    ModelEvent::Finish(FinishReason::ToolCalls),
                    ModelEvent::End,
                ]
            }
        };
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
struct Work(AtomicUsize);
#[async_trait]
impl Tool for Work {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "work".into(),
            description: "Local test work".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("x".repeat(3500).into())
    }
}

#[tokio::test]
async fn one_user_request_keeps_running_through_seven_tool_rounds() {
    let model = Arc::new(LoopModel {
        main_calls: AtomicUsize::new(0),
        summaries: AtomicUsize::new(0),
        bad_summary: false,
    });
    let tool = Arc::new(Work(AtomicUsize::new(0)));
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(tool.clone())
        .allow_side_effect_tool("work")
        .plugin(Arc::new(CompactionPlugin::default()))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("Complete seven tool calls.");
    request.limits.max_context_bytes = 6000;
    request.limits.max_tool_result_bytes = 10000;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(tool.0.load(Ordering::SeqCst), 7);
    assert_eq!(model.main_calls.load(Ordering::SeqCst), 8);
    assert!(model.summaries.load(Ordering::SeqCst) > 0);
    assert_eq!(
        report
            .transcript
            .iter()
            .filter(|m| matches!(m, Message::Tool { .. }))
            .count(),
        7
    );
    assert!(report
        .model_requests
        .iter()
        .all(|r| serde_json::to_vec(r.request.as_ref().unwrap()).unwrap().len() <= 6000));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_summary_is_rejected_before_the_next_primary_call() {
    let model = Arc::new(LoopModel {
        main_calls: AtomicUsize::new(0),
        summaries: AtomicUsize::new(0),
        bad_summary: true,
    });
    let mut host = HostBuilder::new()
        .model(model.clone())
        .plugin(Arc::new(CompactionPlugin::default()))
        .build()
        .await
        .unwrap();
    let assistant = || Message::Assistant {
        content: "a".repeat(100),
        tool_calls: vec![],
        reasoning_content: None,
        provider_data: None,
    };
    let mut request = RunRequest::new("unused");
    request.messages = vec![
        Message::user("u".repeat(2500)),
        assistant(),
        Message::user("v".repeat(2200)),
        assistant(),
    ];
    request.limits.max_context_bytes = 6000;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(model.main_calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.summaries.load(Ordering::SeqCst), 1);
    assert!(report
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("summary exceeds"));
    host.shutdown().await.unwrap();
}
