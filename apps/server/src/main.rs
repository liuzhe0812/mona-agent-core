use api::*;
use application::{AgentApplication, ApplicationConfig};
use providers::{ChatConfig, ChatModel};
use runtime::HostBuilder;
use std::{sync::Arc, time::Duration};

mod capabilities;
mod session_routes;
#[cfg(feature = "model-management")]
mod model_settings;
#[cfg(feature = "skills")]
mod skill_setup;
#[cfg(feature = "spill")]
mod spill_setup;
mod tool_setup;

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
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("AGENT_SERVER_TOKEN")?;
    let args: Vec<_> = std::env::args().collect();
    let demo = args.iter().any(|arg| arg == "--demo");
    let capability_manager = capabilities::from_environment(&args)?;
    #[cfg(feature = "model-management")]
    let manager = if !demo && capability_manager.active("model-management") {
        Some(model_settings::from_environment()?)
    } else {
        None
    };
    #[cfg(feature = "spill")]
    let spill_host = if !demo && capability_manager.active(capabilities::SPILL) {
        let host = spill_setup::SpillHost::from_environment()?;
        host.cleanup_startup().await?;
        Some(host)
    } else {
        None
    };

    #[cfg(feature = "spill")]
    let spill_plugin = spill_host.as_ref().map(spill_setup::SpillHost::plugin);
    let tool_config = tool_setup::from_environment(Vec::new())?;
    let session_store = session_routes::from_environment(&tool_config.cwd, demo)?;
    #[cfg(feature = "spill")]
    let tool_config = {
        let mut tools = tool_config;
        if let Some(spill_host) = &spill_host {
            tools.read_extensions.push(Arc::new(spill_host.read_extension().with_sessions(session_store.clone())));
            tools.output_archive = Some(Arc::new(spill_setup::SpillOutputArchive::new(
                spill_plugin.as_ref().expect("spill host owns its plugin").archive(),
            )));
        }
        tools
    };
    let mut builder = HostBuilder::new()
        .checkpoint_sink(Arc::new(sessions::SessionSink(session_store.clone())))?;
    #[cfg(feature = "spill")]
    if spill_host.is_some() {
        builder = builder.context_transform(Arc::new(sessions::references::SessionReferences::new(session_store.clone())));
    }
    if !demo && capability_manager.active(capabilities::INSTRUCTIONS) {
        let store = session_store.clone();
        let rules = instructions::ProjectInstructions::new(&tool_config.cwd)?.with_history(Arc::new(
            move |ctx: &RunContext| match ctx.metadata.get(sessions::SESSION_KEY) {
                Some(id) => store.source_history(id).map_err(|_| AgentError::new(ErrorCode::Checkpoint, "saved instruction scopes could not be loaded")),
                None => Ok(Vec::new()),
            },
        ));
        builder = builder.plugin(Arc::new(rules.plugin()));
    }
    for tool in tools::core_tools(&tool_config) {
        builder = builder.tool(tool);
    }
    for name in [capabilities::GREP, capabilities::FIND, capabilities::LS] {
        if capability_manager.active(name) {
            builder = builder
                .tool(tools::optional_tool(name, &tool_config).expect("known optional tool"));
        }
    }
    for name in ["shell", "edit", "write"] {
        builder = builder.allow_side_effect_tool(name);
    }
    #[cfg(feature = "compaction")]
    if !demo && capability_manager.active(capabilities::COMPACTION) {
        let plugin = compaction::CompactionPlugin::default();
        session_store.attach_compactor(plugin.compactor())?;
        builder = builder.plugin(Arc::new(plugin));
    }
    #[cfg(feature = "spill")]
    if let Some(plugin) = spill_plugin {
        builder = builder.plugin(Arc::new(plugin));
    }
    #[cfg(feature = "skills")]
    if !demo && capability_manager.active("skills") {
        if let Some(plugin) = skill_setup::from_environment(&tool_config.cwd)? {
            builder = builder.plugin(Arc::new(plugin));
        }
    }
    #[cfg(feature = "model-management")]
    if let Some(manager) = &manager {
        builder = builder.plugin(manager.plugin());
    }
    #[cfg(feature = "model-management")]
    let fixed_model = manager.is_none();
    #[cfg(not(feature = "model-management"))]
    let fixed_model = true;
    if fixed_model {
        let model: Arc<dyn Model> = if demo {
            eprintln!("OFFLINE DEMO: deterministic model; no paid API requests");
            Arc::new(DemoModel)
        } else {
            let mut config = ChatConfig::new(
                std::env::var("AGENT_MODEL_ENDPOINT")?,
                std::env::var("AGENT_MODEL_NAME")?,
            );
            config.api_key = std::env::var("AGENT_MODEL_KEY")
                .ok()
                .filter(|value| !value.is_empty());
            config.context_window_tokens = std::env::var("AGENT_MODEL_CONTEXT_TOKENS")
                .ok()
                .map(|value| value.parse::<u64>())
                .transpose()
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "AGENT_MODEL_CONTEXT_TOKENS must be a positive integer",
                    )
                })?;
            config.allow_http_loopback =
                std::env::var("AGENT_ALLOW_HTTP_LOOPBACK").as_deref() == Ok("1");
            if let Ok(extra) = std::env::var("AGENT_MODEL_EXTRA_JSON") {
                config.extra_body = serde_json::from_str(&extra).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "AGENT_MODEL_EXTRA_JSON must be a JSON object",
                    )
                })?;
            }
            Arc::new(ChatModel::new(config)?)
        };
        builder = builder.model(model);
    }

    let mut host = builder.build().await?;
    let runtime: Arc<dyn AgentRuntime> = Arc::new(host.engine());
    #[cfg(feature = "model-management")]
    let runtime: Arc<dyn AgentRuntime> = if let Some(manager) = &manager {
        manager.runtime(runtime)
    } else {
        runtime
    };
    let runtime = sessions::runtime(runtime, session_store.clone());
    let application = AgentApplication::new(runtime, ApplicationConfig::default())?;
    let mut config = http_bridge::HttpConfig::new(token.clone());
    if let Ok(origin) = std::env::var("AGENT_UI_ORIGIN") {
        config.allowed_origins.push(origin);
    }
    let router = http_bridge::router(application.clone(), config)?;
    let router = router.merge(session_routes::router(
        session_store, application.clone(), token.clone(),
        std::env::var("AGENT_UI_ORIGIN").ok(),
    )?);
    let router = if demo {
        router
    } else {
        router.merge(capabilities::router(
            capability_manager,
            token.clone(),
            std::env::var("AGENT_UI_ORIGIN").ok(),
        )?)
    };
    #[cfg(feature = "spill")]
    let router = if let Some(spill_host) = spill_host {
        router.merge(spill_setup::router(
            spill_host,
            token.clone(),
            std::env::var("AGENT_UI_ORIGIN").ok(),
        )?)
    } else {
        router
    };
    #[cfg(feature = "model-management")]
    let router = if let Some(manager) = manager {
        router.merge(model_settings::router(
            manager,
            token,
            std::env::var("AGENT_UI_ORIGIN").ok(),
        )?)
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
    host.shutdown().await?;
    serve_result?;
    Ok(())
}
