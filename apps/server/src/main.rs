use api::*;
use application::{AgentApplication, ApplicationConfig};
use runtime::HostBuilder;
use providers::{ChatConfig, ChatModel};
use std::{sync::Arc, time::Duration};

#[cfg(feature = "model-management")]
mod model_settings;

/// Deterministic, explicitly labelled offline demonstration. Not a real language model.
struct DemoModel;
#[async_trait]
impl Model for DemoModel {
    async fn stream(&self, request: ModelRequest, cancel: CancellationToken) -> Result<ModelStream> {
        let events = if request.messages.iter().any(|m| matches!(m, Message::Tool { .. })) {
            let mut events = "演示完成：1 + 2 = 3。这里的文字逐字流式发送。".chars()
                .map(|c| ModelEvent::Text(c.to_string())).collect::<Vec<_>>();
            events.extend([ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]); events
        } else { demo::call("demo-add", "add", serde_json::json!({"a":1,"b":2})) };
        Ok(Box::pin(futures_util::stream::unfold((events.into_iter(), cancel), |(mut events, cancel)| async move {
            let event = events.next()?;
            tokio::select! {
                _ = cancel.cancelled() => None,
                _ = tokio::time::sleep(Duration::from_millis(40)) => Some((Ok(event), (events, cancel))),
            }
        })))
    }
}
#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // No default password. Supply a randomly generated token of at least 32 characters.
    let token = std::env::var("AGENT_SERVER_TOKEN")?;
    let demo = std::env::args().any(|arg| arg == "--demo");
    #[cfg(feature = "model-management")]
    let manager = if !demo && std::env::var("AGENT_MODEL_MANAGEMENT").as_deref() != Ok("0") {
        Some(model_settings::from_environment()?)
    } else { None };
    let mut builder = HostBuilder::new().tool(Arc::new(demo::Add));
    #[cfg(feature = "model-management")]
    if let Some(manager) = &manager { builder = builder.plugin(manager.plugin()); }
    #[cfg(feature = "model-management")]
    let fixed_model = manager.is_none();
    #[cfg(not(feature = "model-management"))]
    let fixed_model = true;
    if fixed_model {
    let model: Arc<dyn Model> = if demo {
        eprintln!("OFFLINE DEMO: deterministic model; no paid API requests"); Arc::new(DemoModel)
    } else {
        let mut config = ChatConfig::new(std::env::var("AGENT_MODEL_ENDPOINT")?, std::env::var("AGENT_MODEL_NAME")?);
        config.api_key = std::env::var("AGENT_MODEL_KEY").ok().filter(|value| !value.is_empty());
        config.allow_http_loopback = std::env::var("AGENT_ALLOW_HTTP_LOOPBACK").as_deref() == Ok("1");
        if let Ok(extra) = std::env::var("AGENT_MODEL_EXTRA_JSON") {
            config.extra_body = serde_json::from_str(&extra).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "AGENT_MODEL_EXTRA_JSON must be a JSON object")
            })?;
        }
        Arc::new(ChatModel::new(config)?)
    };
    builder = builder.model(model);
    }
    let mut host = builder.build().await?;
    let runtime: Arc<dyn AgentRuntime> = Arc::new(host.engine());
    #[cfg(feature = "model-management")]
    let runtime: Arc<dyn AgentRuntime> = if let Some(manager) = &manager { manager.runtime(runtime) } else { runtime };
    let application = AgentApplication::new(runtime, ApplicationConfig::default())?;
    let mut config = http_bridge::HttpConfig::new(token.clone());
    if let Ok(origin) = std::env::var("AGENT_UI_ORIGIN") { config.allowed_origins.push(origin); }
    let router = http_bridge::router(application.clone(), config)?;
    #[cfg(feature = "model-management")]
    let router = if let Some(manager) = manager {
        router.merge(model_settings::router(manager, token, std::env::var("AGENT_UI_ORIGIN").ok())?)
    } else { router };
    // The development launcher only supplies a loopback address. Production deployments
    // should still put this service behind an authenticated TLS ingress.
    let bind_addr = std::env::var("AGENT_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    eprintln!("Agent HTTP/SSE listening on {bind_addr} (token not logged)");
    let stop_app = application.clone();
    let serve_result = axum::serve(listener, router).with_graceful_shutdown(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_app.shutdown(Duration::from_secs(10)).await;
    }).await;
    application.shutdown(Duration::from_secs(10)).await?;
    host.shutdown().await?;
    serve_result?;
    Ok(())
}
