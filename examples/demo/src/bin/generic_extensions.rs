//! Generic extension seams; no database, Mona types, Python runtime or bridge.
//! The model below is scripted and intentionally does not interpret the sample image.
use agent_api::*;
use agent_core::HostBuilder;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct ExampleSink { checkpoints: Mutex<Vec<Arc<RunCheckpoint>>> }
#[async_trait]
impl CheckpointSink for ExampleSink {
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, cancel: CancellationToken) -> Result<()> {
        if cancel.is_cancelled() { return Err(AgentError::new(ErrorCode::Cancelled, "sink cancelled")); }
        let mut stored = self.checkpoints.lock().unwrap();
        if stored.len() >= 64 { return Err(AgentError::new(ErrorCode::Limit, "example sink capacity reached")); }
        if stored.iter().any(|old| old.run_id == checkpoint.run_id && old.revision == checkpoint.revision) { return Ok(()); }
        stored.push(checkpoint);
        // This only acknowledges an in-memory copy, not disk durability!
        Ok(())
    }
}
struct NoTools;
#[async_trait]
impl ToolSelector for NoTools {
    async fn select(&self, _: &RunContext, _: usize, _: &[Message], _: &[ToolSpec]) -> Result<Vec<String>> { Ok(vec![]) }
}
struct Extensions { sink: Arc<ExampleSink> }
#[async_trait]
impl Plugin for Extensions {
    fn manifest(&self) -> PluginManifest { PluginManifest::new("generic.example") }
    async fn install(&self, registry: &mut dyn Registrar) -> Result<()> {
        registry.tool_selector(Arc::new(NoTools));
        registry.checkpoint_sink(self.sink.clone())
    }
}
struct ExampleModel;
#[async_trait]
impl Model for ExampleModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        assert!(request.tools.is_empty());
        assert!(request.messages.iter().any(|m| matches!(m, Message::User { content } if content.has_media())));
        assert_eq!(request.options.temperature, Some(0.2));
        let events = vec![ModelEvent::Text("Scripted example: rich input and options reached the adapter.".into()),
            ModelEvent::ProviderData { target: ProtocolTarget::Assistant,
                data: ProviderData { namespace: "example".into(), value: json!({"opaque_ticket":"example-only"}) } },
            ModelEvent::Finish(FinishReason::Stop), ModelEvent::End];
        Ok(Box::pin(futures_util::stream::iter(events.into_iter().map(Ok))))
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    let sink = Arc::new(ExampleSink::default());
    let mut host = HostBuilder::new().model(Arc::new(ExampleModel))
        .plugin(Arc::new(Extensions { sink: sink.clone() })).build().await?;
    let mut request = RunRequest::new(Content::Blocks(vec![ContentBlock::Text { text: "Inspect this input".into() },
        ContentBlock::Image { media_type: "image/png".into(), source: ImageSource::Base64 { data: "AQ==".into() } }]));
    // AQ== only demonstrates encoding validity, not a real PNG or a vision test.
    request.model_options.temperature = Some(0.2);
    let report = host.engine().execute(request).await?;
    println!("status={:?}; checkpoint_ack={:?}; text={}", report.status,
        report.checkpoint.last_acknowledged_revision, report.output.as_deref().unwrap_or(""));
    println!("in-memory checkpoints={} (not durable)", sink.checkpoints.lock().unwrap().len());
    host.shutdown().await
}
