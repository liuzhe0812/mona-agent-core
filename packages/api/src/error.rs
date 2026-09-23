use serde::{Deserialize, Serialize};
use std::fmt;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Configuration,
    Unsupported,
    Checkpoint,
    Dependency,
    Plugin,
    Schema,
    Policy,
    ModelAuthentication,
    ModelQuota,
    ModelRateLimit,
    ModelServer,
    ModelContextWindow,
    /// The selected adapter cannot safely replay this history. Never retry or strip fields.
    ModelHistoryIncompatible,
    ModelRequest,
    ModelTransport,
    ModelProtocol,
    ModelTruncated,
    Tool,
    Cancelled,
    Deadline,
    Limit,
    Closed,
    Panic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// True when a failed model attempt already emitted model-visible output.
    #[serde(default, skip_serializing_if = "is_false")]
    pub model_output_started: bool,
}
impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            http_status: None,
            retry_after_ms: None,
            model_output_started: false,
        }
    }
    pub fn with_http(mut self, status: u16, retry_after_ms: Option<u64>) -> Self {
        self.http_status = Some(status);
        self.retry_after_ms = retry_after_ms;
        self
    }
}
impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for AgentError {}
pub type Result<T> = std::result::Result<T, AgentError>;
