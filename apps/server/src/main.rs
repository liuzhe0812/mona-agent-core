use api::*;
use application::{AgentApplication, ApplicationConfig};
use std::time::Duration;

mod capabilities;
mod environment;
#[cfg(feature = "model-management")]
mod model_settings;
mod session_routes;
#[cfg(feature = "skills")]
mod skill_setup;
#[cfg(feature = "spill")]
mod spill_setup;
mod tool_setup;
mod workspace_routes;
mod workspace_setup;

/// Deterministic, explicitly labelled offline demonstration. Not a real language model.
struct DemoModel;
#[async_trait]
impl Model for DemoModel {
    async fn stream(
        &self,
        _request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream> {
        let mut events = "离线演示完成：这里的文字逐字流式发送。"
            .chars()
            .map(|c| ModelEvent::Text(c.to_string()))
            .collect::<Vec<_>>();
        events.extend([ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]);
        Ok(Box::pin(futures_util::stream::unfold(
            (events.into_iter(), cancel),
            |(mut events, cancel)| async move {
                let event = events.next()?;
                tokio::select! {
                    _ = cancel.cancelled() => None,
                    _ = tokio::time::sleep(Duration::from_millis(40)) => Some((Ok(event), (events, cancel))),
                }
            },
        )))
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = std::env::var("AGENT_SERVER_TOKEN")?;
    let args: Vec<_> = std::env::args().collect();
    let demo = args.iter().any(|arg| arg == "--demo");
    let capability_manager = capabilities::from_environment(&args)
        .map_err(|e| AgentError::new(ErrorCode::Configuration, e.to_string()))?;
    let settings = workspace_setup::WorkspaceSettings::from_environment(demo)?;
    let root = std::path::PathBuf::from(settings.view(false)?.default_root);
    let session_store = session_routes::from_environment(&root, demo)?;
    let factory = environment::Factory::from_environment(
        session_store.clone(),
        capability_manager.clone(),
        demo,
    )
    .await?;
    let environments = environment::Environments::new(factory, &root).await?;
    let application =
        AgentApplication::new(environments.runtime.clone(), ApplicationConfig::default())?;
    let workspaces = workspace_routes::Service::new(
        settings,
        session_store.clone(),
        environments.clone(),
        demo,
    )?;
    let mut config = http_bridge::HttpConfig::new(token.clone());
    if let Ok(origin) = std::env::var("AGENT_UI_ORIGIN") {
        config.allowed_origins.push(origin);
    }
    let router = http_bridge::router(application.clone(), config)?;
    let router = router.merge(
        session_routes::router(
            session_store,
            application.clone(),
            token.clone(),
            std::env::var("AGENT_UI_ORIGIN").ok(),
            Some(workspaces.clone()),
        )
        .map_err(|e| AgentError::new(ErrorCode::Configuration, e.to_string()))?,
    );
    let router = router.merge(workspace_routes::router(
        workspaces,
        token.clone(),
        std::env::var("AGENT_UI_ORIGIN").ok(),
    )?);
    let router = if demo {
        router
    } else {
        router.merge(
            capabilities::router(
                capability_manager,
                token.clone(),
                std::env::var("AGENT_UI_ORIGIN").ok(),
            )
            .map_err(|e| AgentError::new(ErrorCode::Configuration, e.to_string()))?,
        )
    };
    #[cfg(feature = "spill")]
    let router = if let Some(spill_host) = &environments.factory.spill {
        router.merge(
            spill_setup::router(
                spill_host.clone(),
                token.clone(),
                std::env::var("AGENT_UI_ORIGIN").ok(),
            )
            .map_err(|e| AgentError::new(ErrorCode::Configuration, e.to_string()))?,
        )
    } else {
        router
    };
    #[cfg(feature = "model-management")]
    let router = if let Some(manager) = &environments.factory.manager {
        router.merge(
            model_settings::router(
                manager.clone(),
                token,
                std::env::var("AGENT_UI_ORIGIN").ok(),
            )
            .map_err(|e| AgentError::new(ErrorCode::Configuration, e.to_string()))?,
        )
    } else {
        router
    };

    let bind_addr = std::env::var("AGENT_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    eprintln!("Agent HTTP/SSE listening on {bind_addr} (token not logged)");
    let stop_app = application.clone();
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop_app.shutdown(Duration::from_secs(10)).await;
        })
        .await;
    application.shutdown(Duration::from_secs(10)).await?;
    environments.shutdown().await?;
    serve_result?;
    Ok(())
}
