use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    InvalidRequest,
    NotFound,
    Conflict,
    Closed,
    Capacity,
    Internal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
}
impl SessionError {
    pub fn new(code: SessionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for SessionError {}
pub type SessionResult<T> = std::result::Result<T, SessionError>;
