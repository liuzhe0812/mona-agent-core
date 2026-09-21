use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Configuration, Unsupported, Checkpoint, Dependency, Plugin, Schema, Policy, ModelTransport,
    ModelProtocol, ModelTruncated, Tool, Cancelled, Deadline, Limit, Closed, Panic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentError {
    pub code: ErrorCode,
    pub message: String,
}
impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}
impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for AgentError {}
pub type Result<T> = std::result::Result<T, AgentError>;
