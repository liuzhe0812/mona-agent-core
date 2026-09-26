use crate::{Message, ModelOptions, ProtocolTarget, ProviderData, Result, ToolCall, ToolSpec};
use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::{pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Provider-reported prompt total, including tokens read from or written to cache.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Prompt-side subsets. None means the provider did not report that bucket.
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}
impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: u32,
    #[serde(default)]
    pub options: ModelOptions,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Filtered,
}

#[derive(Clone, Debug)]
pub enum ModelEvent {
    Text(String),
    Reasoning(String),
    ToolDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// Complete opaque snapshot for a message/call, not displayable reasoning.
    ProviderData {
        target: ProtocolTarget,
        data: ProviderData,
    },
    /// A cumulative snapshot for this request, not a token delta.
    Usage(Usage),
    Finish(FinishReason),
    /// Transport-level completion (for SSE: received [DONE]).
    End,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub provider_data: Option<ProviderData>,
    pub finish: FinishReason,
    pub usage: Option<Usage>,
}
impl ModelReply {
    pub fn into_message(self) -> Message {
        Message::Assistant {
            content: self.content,
            tool_calls: self.tool_calls,
            reasoning_content: self.reasoning_content,
            provider_data: self.provider_data,
        }
    }
}

pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent>> + Send>>;

/// Raw adapter. Application plugins should use RunContext.model, not this interface.
#[async_trait]
pub trait Model: Send + Sync {
    /// Pure, bounded replay validation before context transforms or network calls.
    /// Does not test credentials/window size or modify history. Adapters own protocol rules.
    fn validate_history(&self, _messages: &[Message], _options: &ModelOptions) -> Result<()> {
        Ok(())
    }
    /// Optional capacity for the effective model selected by these options.
    fn context_window_tokens(&self, _options: &ModelOptions) -> Option<u64> {
        None
    }
    async fn stream(&self, request: ModelRequest, cancel: CancellationToken)
        -> Result<ModelStream>;
}

/// Nonblocking observer of a model call. Final correctness comes from ModelReply.
pub trait ModelSink: Send + Sync {
    fn text(&self, delta: &str);
    /// Metadata is cumulative; arguments is only the newly arrived fragment.
    /// UI preview only: runtime execution still requires a complete validated ModelReply.
    fn tool_delta(&self, _index: usize, _id: Option<&str>, _name: Option<&str>, _arguments: &str) {}
}

/// An occupancy estimate, never a billing record. Appended messages are still estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputTokenEstimate {
    pub tokens: u64,
    pub provider_anchored: bool,
}

/// Budgeted, timed, bounded gateway bound to a Run. Shared by tools and transforms.
#[async_trait]
pub trait ModelCaller: Send + Sync {
    /// Reuse a primary-call usage anchor only for the same effective model/options/tools
    /// and an unchanged request prefix. Auxiliary calls must never update that anchor.
    fn estimate_input_tokens(&self, _request: &ModelRequest) -> Option<InputTokenEstimate> {
        None
    }

    async fn complete(
        &self,
        request: ModelRequest,
        sink: Option<Arc<dyn ModelSink>>,
    ) -> Result<ModelReply>;
}
