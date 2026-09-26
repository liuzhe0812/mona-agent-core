use api::*;
use application::{AgentApplication, ApplicationConfig};
use std::{future::Future, time::Duration};

mod capabilities;
#[cfg(feature = "mcp")]
mod mcp_setup;
mod sandbox_setup;
#[cfg(feature = "subagent")]
mod subagent_setup;
#[cfg(feature = "planner")]
mod planning;
mod environment;
mod conversation_metrics;
#[cfg(any(feature = "memory", feature = "history-search"))]
mod memory_routes;
#[cfg(feature = "model-management")]
mod model_settings;
#[cfg(all(test, feature = "model-management"))]
mod model_settings_tests;
mod session_routes;
mod side_routes;
#[cfg(feature = "skills")]
mod skill_setup;
#[cfg(feature = "spill")]
mod spill_setup;
mod tool_setup;
mod workspace_routes;
mod workspace_setup;
mod workbench_routes;
mod terminal;
mod review;

pub(crate) fn ui_origin_allowed(origin: &str) -> bool {
    origin == "tauri://localhost" || origin.starts_with("https://") || origin.starts_with("http://")
}

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

/// Must run before any Tokio or GUI threads so embedded sandbox helpers exit immediately.
pub fn dispatch_helper() -> Option<i32> {
    #[cfg(feature = "sandbox")]
    { sandbox::runner::dispatch() }
    #[cfg(not(feature = "sandbox"))]
    { None }
}

/// The same product host serves the browser and the desktop shell. Both shutdown paths drain it.
pub async fn serve_until(shutdown: impl Future<Output = ()> + Send + 'static)
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_on(None, None, None, shutdown).await
}

/// Desktop owns a prebound loopback listener, so no other process can take its chosen port.
pub async fn serve_with_listener(listener: std::net::TcpListener, token: String, model_store_key: String, shutdown: impl Future<Output = ()> + Send + 'static)
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_on(Some(listener), Some(token), Some(model_store_key), shutdown).await
}

async fn serve_on(listener: Option<std::net::TcpListener>, token: Option<String>, model_store_key: Option<String>, shutdown: impl Future<Output = ()> + Send + 'static)
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = match token { Some(token) => token, None => std::env::var("AGENT_SERVER_TOKEN")? };
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
        model_store_key,
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
    let side = side_routes::Service::new(
        application.clone(),
        environments.clone(),
        workspaces.settings.clone(),
        session_store.clone(),
    );
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
            Some(side.clone()),
        )
        .map_err(|e| AgentError::new(ErrorCode::Configuration, e.to_string()))?,
    );
    let terminals = std::sync::Arc::new(terminal::Terminals::default());
    let sweep = terminals.clone();
    let terminal_sweep = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop { interval.tick().await; sweep.sweep(); }
    });
    let router = router.merge(workbench_routes::router(workspaces.clone(), terminals.clone(), token.clone(), std::env::var("AGENT_UI_ORIGIN").ok())?);
    let router = router.merge(workspace_routes::router(
        workspaces,
        token.clone(),
        std::env::var("AGENT_UI_ORIGIN").ok(),
    )?);
    let router = router.merge(side_routes::router(
        side,
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
    #[cfg(any(feature = "memory", feature = "history-search"))]
    let router = router.merge(memory_routes::router(&environments.factory, token.clone(), std::env::var("AGENT_UI_ORIGIN").ok())?);
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

    let listener = match listener {
        Some(listener) => tokio::net::TcpListener::from_std(listener)?,
        None => {
            let bind_addr = std::env::var("AGENT_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
            tokio::net::TcpListener::bind(&bind_addr).await?
        }
    };
    eprintln!("Agent HTTP/SSE listening on {} (token not logged)", listener.local_addr()?);
    let stop_app = application.clone();
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = shutdown => {} }
            let _ = stop_app.shutdown(Duration::from_secs(10)).await;
        })
        .await;
    terminal_sweep.abort(); terminals.shutdown();
    application.shutdown(Duration::from_secs(10)).await?;
    environments.shutdown().await?;
    serve_result?;
    Ok(())
}
