use api::*;
use compaction::{CompactionState, Compactor};
use std::sync::Arc;

struct Summarizer;
#[async_trait]
impl ModelCaller for Summarizer {
    async fn complete(&self, _: ModelRequest, _: Option<Arc<dyn ModelSink>>) -> Result<ModelReply> {
        Ok(ModelReply {
            content: serde_json::to_string(&compaction::TaskSummary { goal: "decision 31415".into(), ..Default::default() }).unwrap(),
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
            finish: FinishReason::Stop,
            usage: None,
        })
    }
}
#[tokio::test]
async fn state_roundtrips_validates_coverage_and_reuses_summary_with_new_tail() {
    let compactor = Compactor::default();
    let ctx = RunContext {
        run_id: "persisted".into(),
        task: TaskControl::default(),
        cancel: CancellationToken::new(),
        model: Arc::new(Summarizer),
        services: Services::default(),
        metadata: Arc::default(),
        model_options: ModelOptions::default(),
        limits: RunLimits {
            max_context_bytes: 4096,
            max_output_tokens: 64,
            ..Default::default()
        },
        request_overhead_bytes: 100,
        request_tools: Arc::default(),
        context_sources: Arc::default(),
        model_context_window_tokens: None,
        tools_enabled: true,
        allowed_tools: None,
    };
    let original = vec![
        Message::user("earlier ".repeat(200)),
        Message::user("earlier ".repeat(200)),
        Message::user("current"),
    ];
    let projected = compactor.transform(&ctx, original.clone()).await.unwrap();
    let state = compactor.state(&ctx.run_id).unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    compactor.finish(&ctx.run_id).await.unwrap();
    assert!(compactor.state(&ctx.run_id).is_none());
    let restored: CompactionState = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored.project(&original).unwrap(), projected);
    let mut appended = original.clone();
    appended.push(Message::user("new run after restart"));
    let continued = restored.project(&appended).unwrap();
    assert_eq!(continued.last().unwrap().text(), "new run after restart");
    assert_eq!(continued.len(), projected.len() + 1);
    let mut changed = original.clone();
    changed[0] = Message::user("different source");
    assert!(restored.project(&changed).is_err());
    let mut forged = restored;
    forged.ranges[0].start += 1;
    assert!(forged.project(&original).is_err());
}
