//! Optional awaited persistence boundary; no database and no automatic replay engine.
use crate::{
    AgentError, CancellationToken, Message, ModelOptions, Result, RunStatus, TaskUsage, ToolResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const CHECKPOINT_VERSION: u32 = 2;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum CheckpointPhase {
    BeforeModel,
    InputApplied,
    AfterModel,
    ToolIntent { call_id: String },
    ToolSettled { call_id: String },
    AfterTools,
    RunFinished,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CheckpointToolState {
    Pending,
    /// Durable intent is acknowledged; a crash does NOT tell us if dispatch happened.
    IntentRecorded,
    Settled {
        result: ToolResult,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub schema_version: u32,
    pub run_id: String,
    /// Per-run monotonically increasing commit key. Sink implementations must be idempotent.
    pub revision: u64,
    pub phase: CheckpointPhase,
    pub step: usize,
    /// Canonical history. During a tool batch this can have pending assistant calls.
    /// Partial results are in pending_tools; do not send this directly to a model.
    pub transcript: Vec<Message>,
    pub pending_tools: BTreeMap<String, CheckpointToolState>,
    pub selected_tools: Vec<String>,
    pub model_options: ModelOptions,
    pub metadata: BTreeMap<String, String>,
    pub task_usage: TaskUsage,
    pub statistics: crate::RunStatistics,
    pub status: Option<RunStatus>,
    pub error: Option<AgentError>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointStatus {
    pub configured: bool,
    pub last_acknowledged_revision: Option<u64>,
    /// A failed acknowledgement does not prove the sink did not commit.
    pub error: Option<AgentError>,
}
#[async_trait]
pub trait CheckpointSink: Send + Sync {
    /// Success acknowledges this exact immutable snapshot under the sink's documented
    /// durability contract. Called serially per Run; different Runs may call concurrently.
    /// Must honor cancellation and implement idempotency by (run_id, revision).
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, cancel: CancellationToken)
        -> Result<()>;
}
