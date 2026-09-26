use api::{AgentError, ErrorCode, RunContext};
use sandbox::{LocalSandbox, Policy};
use std::sync::Arc;

/// Host-resolved immutable run policy. Model tool arguments never select modes or roots.
#[derive(Clone)]
pub struct SandboxBinding {
    pub provider: Arc<LocalSandbox>,
    resolver: Arc<dyn Fn(&RunContext) -> api::Result<Policy> + Send + Sync>,
}
impl SandboxBinding {
    pub fn new(
        provider: Arc<LocalSandbox>,
        resolver: impl Fn(&RunContext) -> api::Result<Policy> + Send + Sync + 'static,
    ) -> Self {
        Self {
            provider,
            resolver: Arc::new(resolver),
        }
    }
    pub fn resolve(&self, run: &RunContext) -> api::Result<Policy> {
        (self.resolver)(run)
    }
}
pub(crate) fn failure(error: sandbox::Error) -> AgentError {
    let code = match error.code {
        sandbox::ErrorCode::FsSandboxDenied => ErrorCode::Policy,
        sandbox::ErrorCode::SandboxCancelled => ErrorCode::Cancelled,
        sandbox::ErrorCode::SandboxCapacity => ErrorCode::Limit,
        sandbox::ErrorCode::SandboxConfiguration => ErrorCode::Configuration,
        sandbox::ErrorCode::SandboxUnavailable => ErrorCode::Tool,
    };
    AgentError::new(code, error.to_string())
}
