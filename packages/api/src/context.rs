use crate::{Message, ModelCaller, Result, Services, TaskControl, ToolCall, ToolResult, ToolSpec, ModelOptions};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct RunContext {
    pub run_id: String,
    pub task: TaskControl,
    pub cancel: CancellationToken,
    /// All auxiliary model calls must use this budgeted gateway.
    pub model: Arc<dyn ModelCaller>,
    pub services: Services,
    pub metadata: Arc<BTreeMap<String, String>>,
    pub model_options: ModelOptions,
}

#[async_trait]
pub trait ContextTransform: Send + Sync {
    /// Owned projection, not the canonical transcript. The core validates the result.
    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>>;
}

#[async_trait]
pub trait ResultTransform: Send + Sync {
    /// May archive/reduce output, but must not change call_id or execution status.
    async fn transform(&self, ctx: &RunContext, call: &ToolCall, result: ToolResult) -> Result<ToolResult>;
}

#[derive(Clone, Debug)]
pub enum PolicyDecision { Allow, Deny(String) }
#[async_trait]
pub trait ToolPolicy: Send + Sync {
    /// Veto-only. The host's final safety gate cannot be relaxed by a plugin.
    async fn check(&self, ctx: &RunContext, call: &ToolCall, spec: &ToolSpec) -> Result<PolicyDecision>;
}

/// Per-round tool view. Registered implementations remain frozen during a Host lifetime.
/// Selectors compose in order by narrowing; never re-add a previously removed tool.
#[async_trait]
pub trait ToolSelector: Send + Sync {
    async fn select(&self, ctx: &RunContext, step: usize, messages: &[Message], available: &[ToolSpec]) -> Result<Vec<String>>;
}
