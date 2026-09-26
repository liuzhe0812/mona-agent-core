//! Owned child-agent orchestration over public Runtime and Sessions contracts.
//! No model provider, application server, Web dependency, or second execution loop.
#![forbid(unsafe_code)]
mod binding;
mod config;
mod execution;
mod service;
mod state;
mod tools;
use api::{AgentError, ErrorCode, Result};
pub use binding::{Binding, SubagentPlugin};
pub use config::{Config, ModelRoute, Role};
pub use service::{Driver, Launch, Service};
pub use state::{ChildView, ContextMode, Spawn, WaitResult, OWNER_KEY, ROOT_KEY};
use std::sync::{Mutex, MutexGuard};
pub use tools::TOOL_NAMES;
fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    value
        .lock()
        .map_err(|_| AgentError::new(ErrorCode::Plugin, "subagent state unavailable"))
}
fn invalid(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Tool, message)
}
fn storage(error: sessions::SessionError) -> AgentError {
    AgentError::new(
        match error.code {
            sessions::SessionErrorCode::Capacity => ErrorCode::Limit,
            sessions::SessionErrorCode::Internal => ErrorCode::Checkpoint,
            _ => ErrorCode::Tool,
        },
        error.message,
    )
}
async fn disk<T: Send + 'static>(action: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(action).await.map_err(|_| {
        invalid("subagent storage worker failed; inspect saved state before continuing")
    })?
}
