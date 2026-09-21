use agent_api::*;
use agent_application::{AgentApplication, ApplicationConfig};
use agent_core::HostBuilder;
use agent_providers::{ChatConfig, ChatModel};
use std::{sync::Arc, time::Duration};

/// Deterministic, explicitly labelled offline demonstration. Not a real language model.
struct DemoModel;
#[async_trait]
impl Model for DemoModel {
    async fn stream(&self, request: ModelRequest, cancel: CancellationToken) -> Result<ModelStream> {
        let events = if request.messages.iter().any(|m| matches!(m, Message::Tool { .. })) {
            let mut events = "演示完成：1 + 2 = 3。这里的文字逐字流式发送。".chars()
                .map(|c| ModelEvent::Text(c.to_string())).collect::<Vec<_>>();
            events.extend([ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]); events
        } else { agent_demo::call("demo-add", "add", serde_json::json!({"a":1,"b":2})) };
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
    let model: Arc<dyn Model> = if demo {
        eprintln!("OFFLINE DEMO: deterministic model; no paid API requests"); Arc::new(DemoModel)
    } else {
        let mut config = ChatConfig::new(std::env::var("AGENT_MODEL_ENDPOINT")?, std::env::var("AGENT_MODEL_NAME")?);
        config.api_key = std::env::var("AGENT_MODEL_KEY").ok();
        Arc::new(ChatModel::new(config)?)
    };
    let mut host = HostBuilder::new().model(model).tool(Arc::new(agent_demo::Add)).build().await?;
    let application = AgentApplication::new(Arc::new(host.engine()), ApplicationConfig::default())?;
    let mut config = agent_bridge_http::HttpConfig::new(token);
    if let Ok(origin) = std::env::var("AGENT_UI_ORIGIN") { config.allowed_origins.push(origin); }
    let router = agent_bridge_http::router(application.clone(), config)?;
    // Deliberately loopback-only. Deploy behind an authenticated TLS ingress for remote use.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8787").await?;
    eprintln!("Agent HTTP/SSE listening on 127.0.0.1:8787 (token not logged)");
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
