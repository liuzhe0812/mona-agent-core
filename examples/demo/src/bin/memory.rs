use api::{Result, RunRequest};
use runtime::HostBuilder;
use demo::{display_run, text, ScriptedModel};
use memory::{InMemoryStore, MemoryBackend, MemoryPlugin};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let memory = Arc::new(InMemoryStore::default());
    memory.remember("language".into(), "使用中文，回答简洁".into()).await?;
    let model = Arc::new(ScriptedModel::new(vec![text("记忆已作为参考上下文注入。本例使用确定性模型，不做语义推理。") ]));
    let mut host = HostBuilder::new().model(model).plugin(Arc::new(MemoryPlugin::new(memory))).build().await?;
    let report = display_run(&host.engine(), RunRequest::new("我的 language 偏好是什么？")).await?;
    assert!(report.model_requests[0].request.as_ref().unwrap().messages.iter().any(|m| m.text().contains("Retrieved reference")));
    assert!(!report.transcript.iter().any(|m| m.text().contains("Retrieved reference")));
    host.shutdown().await
}
