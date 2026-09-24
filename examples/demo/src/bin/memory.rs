use api::{CancellationToken, RunRequest};
use runtime::HostBuilder;
use demo::{display_run, text, ScriptedModel};
use memory::{Backend, Binding, Change, InMemoryStore, MemoryPlugin, Operation, Origin};
use std::sync::Arc;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let memory = Arc::new(InMemoryStore::default());
    let cancel = CancellationToken::new();
    memory.apply(&Change { revision: memory.read(&cancel)?.revision, operations: vec![Operation::Add { text: "使用中文，回答简洁".into() }] }, Origin::host(), &cancel)?;
    let model = Arc::new(ScriptedModel::new(vec![text("记忆已作为参考上下文注入。本例使用确定性模型，不做语义推理。") ]));
    let mut host = HostBuilder::new().model(model).plugin(Arc::new(MemoryPlugin::new(vec![Binding::new("personal", memory, false)])?)).build().await?;
    let report = display_run(&host.engine(), RunRequest::new("我的 language 偏好是什么？")).await?;
    assert!(report.model_requests[0].request.as_ref().unwrap().messages.iter().any(|m| m.text().contains("Curated long-term")));
    assert!(!report.transcript.iter().any(|m| m.text().contains("Curated long-term")));
    host.shutdown().await?;
    Ok(())
}
