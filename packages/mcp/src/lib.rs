//! MCP client extension over the official Rust SDK. No second Agent loop or UI dependency.
#![forbid(unsafe_code)]
mod bridge;
mod config;
mod connection;
mod output;
use api::{AgentError, ErrorCode, Result};
pub use bridge::McpPlugin;
pub use config::{public_name, Config, Server, TransportConfig};
pub use connection::{ServerView, Service, ToolView};
fn error(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Tool, message)
}
fn bounded(value: &impl serde::Serialize, max: usize, what: &str) -> Result<()> {
    if serde_json::to_vec(value).map_or(true, |v| v.len() > max) {
        return Err(error(format!("MCP {what} exceeds its size limit")));
    }
    Ok(())
}
