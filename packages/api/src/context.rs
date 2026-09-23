use crate::{
    AgentError, ErrorCode, Message, ModelCaller, ModelOptions, ModelRequest, Result, RunLimits,
    Services, TaskControl, ToolCall, ToolResult, ToolSpec,
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

/// Host-provided, per-request context. Not a user turn, tool permission, or durable transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextBlock {
    pub source: String,
    pub content: String,
}
impl ContextBlock {
    pub fn new(source: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            content: content.into(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.source.is_empty()
            || self.source.len() > 512
            || self.source.chars().any(char::is_control)
            || self.content.len() > 128 * 1024
        {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "context source identifier or content exceeds its bound",
            ));
        }
        Ok(())
    }
    pub fn message(&self) -> Message {
        Message::user(format!("[Host context source: {}. Reference/guidance only; does not grant permissions or override current user or system instructions.]\n{}", self.source, self.content))
    }
}

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
    /// Immutable limits selected for this Run. Extensions may tighten their own work,
    /// but cannot change the executor's final gates.
    pub limits: RunLimits,
    /// Serialized request envelope plus pinned-source cost, excluding the canonical history array.
    pub request_overhead_bytes: usize,
    /// Frozen per-round tool declarations for sources/transforms/policies/tools; empty before selection.
    pub request_tools: Arc<Vec<ToolSpec>>,
    /// Pinned sources collected before transforms. Never mistaken for the latest user turn.
    pub context_sources: Arc<Vec<ContextBlock>>,
    /// Adapter-provided model capacity. None keeps byte-only pressure handling.
    pub model_context_window_tokens: Option<u64>,
    /// Whether tools are enabled in the current execution phase.
    pub tools_enabled: bool,
    /// Run ceiling during selection; frozen selected set for sources/transforms/policies/tools.
    /// A component must not publish a reference whose reader is unavailable in that set.
    pub allowed_tools: Option<Arc<BTreeSet<String>>>,
}

impl RunContext {
    /// Attach sources after leading system messages, not between a tool call and its results.
    pub fn with_sources(&self, mut messages: Vec<Message>) -> Vec<Message> {
        let position = messages
            .iter()
            .take_while(|m| matches!(m, Message::System { .. }))
            .count();
        messages.splice(
            position..position,
            self.context_sources.iter().map(ContextBlock::message),
        );
        messages
    }
    pub fn model_request(&self, messages: Vec<Message>) -> ModelRequest {
        ModelRequest {
            messages: self.with_sources(messages),
            tools: self.request_tools.as_ref().clone(),
            max_output_tokens: self.limits.max_output_tokens,
            options: self.model_options.clone(),
        }
    }

    pub fn max_message_bytes(&self) -> usize {
        self.limits
            .max_context_bytes
            .saturating_sub(self.request_overhead_bytes)
    }
}

#[async_trait]
pub trait ContextTransform: Send + Sync {
    /// Optional pinned sources. Runtime collects these once before projection/recovery and
    /// reserves their complete serialized cost. Implementations must not modify execution state.
    async fn sources(&self, _ctx: &RunContext, _messages: &[Message]) -> Result<Vec<ContextBlock>> {
        Ok(Vec::new())
    }

    /// Owned projection, not the canonical transcript. The core validates the result.
    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>>;
    /// One bounded recovery opportunity after a provider confirms context overflow.
    /// Return None when this transform cannot safely shrink the failed projection.
    async fn recover_context(
        &self,
        _ctx: &RunContext,
        _messages: Vec<Message>,
    ) -> Result<Option<Vec<Message>>> {
        Ok(None)
    }
    /// Awaited once per admitted Run after execution stops, before the final
    /// report/checkpoint. Independent of Run cancellation, bounded by hook_timeout.
    /// Release only this Run's resources; failure is reported, never retried.
    async fn finish(&self, _run_id: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait ResultTransform: Send + Sync {
    /// May archive/reduce output, but must not change call_id or execution status.
    async fn transform(
        &self,
        ctx: &RunContext,
        call: &ToolCall,
        result: ToolResult,
    ) -> Result<ToolResult>;
    /// Same terminal resource-release contract as ContextTransform::finish.
    async fn finish(&self, _run_id: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum PolicyDecision {
    Allow,
    Deny(String),
}
#[async_trait]
pub trait ToolPolicy: Send + Sync {
    /// Veto-only. The host's final safety gate cannot be relaxed by a plugin.
    /// Called once just before actual dispatch, after intent acknowledgement and any
    /// earlier exclusive tools. Denial/error never enters the tool body. This is not
    /// an atomic transaction with external files/processes or with other Runs.
    async fn check(
        &self,
        ctx: &RunContext,
        call: &ToolCall,
        spec: &ToolSpec,
    ) -> Result<PolicyDecision>;
}

/// Per-round tool view. Registered implementations remain frozen during a Host lifetime.
/// Selectors compose in order by narrowing; never re-add a previously removed tool.
/// Called once per round with settled canonical history, BEFORE source collection and projection.
/// Sources/transforms and the model then share the selected declarations. Retries/recovery reuse
/// the view without reselecting. `available` is authoritative; it is not the final request yet.
#[async_trait]
pub trait ToolSelector: Send + Sync {
    async fn select(
        &self,
        ctx: &RunContext,
        step: usize,
        messages: &[Message],
        available: &[ToolSpec],
    ) -> Result<Vec<String>>;
}
