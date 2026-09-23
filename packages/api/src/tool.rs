use crate::{ArtifactRef, Content, Result, RunContext};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolConcurrency {
    Exclusive,
    ParallelSafe,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema Draft 7; remote $ref is rejected at registration.
    pub parameters: Value,
    pub concurrency: ToolConcurrency,
    pub side_effects: bool,
}

pub trait ToolProgress: Send + Sync {
    /// Nonblocking telemetry. Must not be used as the final result channel.
    fn report(&self, text: &str);
    /// Optional namespaced UI snapshot (plan, diff, etc.). Cannot alter execution status.
    fn set_detail(&self, _key: &str, _value: Value) -> Result<()> {
        Err(crate::AgentError::new(
            crate::ErrorCode::Configuration,
            "structured tool progress is unsupported by this sink",
        ))
    }
}
#[derive(Clone)]
pub struct ToolContext {
    pub run: RunContext,
    pub call_id: String,
    pub progress: Arc<dyn ToolProgress>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Implementations must yield, honor cancellation, and own spawned child processes.
    async fn execute(&self, ctx: ToolContext, arguments: Value) -> Result<ToolOutput>;
}

/// A tool may report a known business error without losing its structured payload.
/// Denied/Skipped/Unknown are assigned by the executor, not by this output type.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToolOutput {
    pub content: Content,
    pub structured: Option<Value>,
    pub artifact: Option<ArtifactRef>,
    pub is_error: bool,
}
impl ToolOutput {
    pub fn new(content: impl Into<Content>) -> Self {
        Self {
            content: content.into(),
            ..Self::default()
        }
    }
    pub fn error(content: impl Into<Content>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            ..Self::default()
        }
    }
}
impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}
impl From<&str> for ToolOutput {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}
impl From<Content> for ToolOutput {
    fn from(content: Content) -> Self {
        Self::new(content)
    }
}
