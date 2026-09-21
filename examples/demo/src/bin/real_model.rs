use agent_api::{AgentError, ErrorCode, Result, RunRequest};
use agent_core::HostBuilder;
use agent_demo::{display_run, Add};
use agent_providers::{ChatConfig, ChatModel};
use std::{env, sync::Arc};

fn required(key: &str) -> Result<String> {
    env::var(key).map_err(|_| AgentError::new(ErrorCode::Configuration, format!("set {key} first")))
}
#[tokio::main]
async fn main() -> Result<()> {
    let mut config = ChatConfig::new(required("AGENT_MODEL_ENDPOINT")?, required("AGENT_MODEL_NAME")?);
    config.api_key = env::var("AGENT_API_KEY").ok();
    config.allow_http_loopback = env::var("AGENT_ALLOW_HTTP_LOOPBACK").as_deref() == Ok("1");
    if let Ok(extra) = env::var("AGENT_MODEL_EXTRA_JSON") {
        config.extra_body = serde_json::from_str(&extra).map_err(|_| AgentError::new(ErrorCode::Configuration, "AGENT_MODEL_EXTRA_JSON must be an object"))?;
    }
    let model = Arc::new(ChatModel::new(config)?);
    let mut host = HostBuilder::new().model(model).tool(Arc::new(Add)).build().await?;
    let prompt = env::args().nth(1).unwrap_or_else(|| "请调用 add 工具计算 17 + 25，再回答结果。".into());
    let outcome = display_run(&host.engine(), RunRequest::new(prompt)).await;
    let shutdown = host.shutdown().await;
    outcome?;
    shutdown
}
