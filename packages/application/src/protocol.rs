use api::{AgentError, ErrorCode, EventEnvelope, RunOutcome, RunSnapshot};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationErrorCode { InvalidRequest, NotFound, Conflict, Capacity, Closed, Internal }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationError { pub code: ApplicationErrorCode, pub message: String }
impl ApplicationError {
    pub fn new(code: ApplicationErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
}
impl fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{:?}: {}", self.code, self.message) }
}
impl std::error::Error for ApplicationError {}
impl From<AgentError> for ApplicationError {
    fn from(error: AgentError) -> Self {
        // Do not expose arbitrary provider/plugin error strings through either bridge.
        let (code, message) = match error.code {
            ErrorCode::Limit => (ApplicationErrorCode::Capacity, "runtime limit reached"),
            ErrorCode::Closed | ErrorCode::Cancelled => (ApplicationErrorCode::Closed, "run is no longer accepting this operation"),
            ErrorCode::Deadline => (ApplicationErrorCode::Closed, "operation deadline reached"),
            ErrorCode::Schema | ErrorCode::ModelProtocol => (ApplicationErrorCode::InvalidRequest, "invalid request or model protocol"),
            _ => (ApplicationErrorCode::Internal, "runtime operation failed; inspect trusted host diagnostics"),
        };
        Self::new(code, message)
    }
}
pub type ApplicationResult<T> = std::result::Result<T, ApplicationError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    /// Idempotency key, scoped to this application instance and its retention window.
    pub request_id: String,
    pub prompt: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StartResponse { pub run_id: String, pub reused: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRequest { pub request_id: String, pub text: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputReceipt { pub request_id: String, pub applied: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CancelReceipt {
    pub run_id: String,
    /// True means cancellation was signalled, NOT that side effects were rolled back.
    pub signalled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultResponse { pub run_id: String, pub outcome: Option<RunOutcome> }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason { Initial, CursorExpired, SourceResync }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamFrame {
    Event { envelope: EventEnvelope },
    Snapshot { reason: SnapshotReason, snapshot: RunSnapshot },
    /// Only used if an executor violates its terminal-result contract.
    Fault { error: ApplicationError },
}
impl StreamFrame {
    pub fn sequence(&self) -> Option<u64> {
        match self { Self::Event { envelope } => Some(envelope.seq), Self::Snapshot { snapshot, .. } => Some(snapshot.seq), Self::Fault { .. } => None }
    }
}
pub(crate) fn validate_key(key: &str) -> ApplicationResult<()> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
        return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "request_id must be 1..128 ASCII identifier characters"));
    }
    Ok(())
}
pub(crate) fn public_outcome(outcome: &mut RunOutcome) {
    if let Some(error) = &mut outcome.error {
        error.message = "run stopped; inspect trusted host diagnostics for details".to_owned();
    }
}
pub(crate) fn public_snapshot(mut snapshot: RunSnapshot) -> RunSnapshot {
    if let Some(outcome) = &mut snapshot.outcome { public_outcome(outcome); }
    snapshot
}
pub(crate) fn public_event(mut event: EventEnvelope) -> EventEnvelope {
    if let api::RunEvent::RunFinished { outcome } = &mut event.event { public_outcome(outcome); }
    event
}
