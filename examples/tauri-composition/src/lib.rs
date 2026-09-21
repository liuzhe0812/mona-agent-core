//! An embeddable composition root, not a full desktop UI application.
//! Does NOT depend on the HTTP bridge or Axum.
#![forbid(unsafe_code)]
#[cfg(feature = "tauri")]
pub async fn assemble<R: tauri::Runtime>(model: std::sync::Arc<dyn api::Model>)
    -> Result<(runtime::Host, application::AgentApplication, tauri::plugin::TauriPlugin<R>), Box<dyn std::error::Error + Send + Sync>> {
    let host = runtime::HostBuilder::new().model(model).build().await?;
    let application = application::AgentApplication::new(std::sync::Arc::new(host.engine()), Default::default())?;
    let plugin = tauri_bridge::init(application.clone(), ["main".to_owned()])?;
    // Keep Host alive in the Tauri application. On exit: application.shutdown(), then host.shutdown().
    Ok((host, application, plugin))
}
