//! The only persistence-aware component. It owns ordering, not storage or replay.
use crate::gate::{bounded, CancelOnDrop};
use agent_api::*;
use std::{collections::BTreeMap, sync::Arc};
use tokio::{sync::Mutex, time::Instant};

struct State { current: RunCheckpoint, status: CheckpointStatus }
#[derive(Clone)]
pub(crate) struct Checkpoints {
    sink: Option<Arc<dyn CheckpointSink>>,
    state: Arc<Mutex<State>>,
    task: TaskControl,
    limits: RunLimits,
}
impl Checkpoints {
    pub fn new(sink: Option<Arc<dyn CheckpointSink>>, ctx: &RunContext, limits: RunLimits) -> Self {
        let configured = sink.is_some();
        Self {
            sink, task: ctx.task.clone(), limits,
            state: Arc::new(Mutex::new(State {
                current: RunCheckpoint {
                    schema_version: CHECKPOINT_VERSION, run_id: ctx.run_id.clone(), revision: 0,
                    phase: CheckpointPhase::BeforeModel, step: 0, transcript: vec![],
                    pending_tools: BTreeMap::new(), selected_tools: vec![],
                    model_options: ctx.model_options.clone(), metadata: (*ctx.metadata).clone(),
                    task_usage: ctx.task.usage(), status: None, error: None,
                },
                status: CheckpointStatus { configured, ..CheckpointStatus::default() },
            })),
        }
    }
    pub async fn status(&self) -> CheckpointStatus { self.state.lock().await.status.clone() }
    pub async fn check(&self) -> Result<()> {
        if let Some(error) = &self.state.lock().await.status.error { Err(error.clone()) } else { Ok(()) }
    }
    async fn write(&self, phase: CheckpointPhase, update: impl FnOnce(&mut RunCheckpoint) + Send) -> Result<()> {
        let Some(sink) = &self.sink else { return Ok(()); };
        // Serialize commits for one Run, including parallel-tool intent/result updates.
        // No global lock and no lock across the tool body itself.
        let mut state = self.state.lock().await;
        if let Some(error) = &state.status.error { return Err(error.clone()); }
        update(&mut state.current);
        state.current.revision += 1;
        state.current.phase = phase;
        state.current.task_usage = self.task.usage();
        if serde_json::to_vec(&state.current).map_or(true, |v| v.len() > self.limits.max_checkpoint_bytes) {
            let error = AgentError::new(ErrorCode::Checkpoint, "checkpoint exceeds configured byte limit");
            state.status.error = Some(error.clone());
            return Err(error);
        }
        let snapshot = Arc::new(state.current.clone());
        // Persistence is a bounded settlement operation, even after task cancellation.
        // Its token must not already be cancelled, otherwise terminal facts are never saved.
        let operation = CancellationToken::new();
        let _cancel_on_drop = CancelOnDrop(operation.clone());
        let saved = bounded(&operation, &operation, Instant::now() + self.limits.checkpoint_timeout,
            self.limits.cancellation_grace, sink.commit(snapshot, operation.clone())).await;
        match saved {
            Ok(()) => { let revision = state.current.revision; state.status.last_acknowledged_revision = Some(revision); Ok(()) }
            Err(_) => {
                // Do not propagate sink messages that may contain credentials/payloads.
                let error = AgentError::new(ErrorCode::Checkpoint, "checkpoint acknowledgement failed; no automatic retry or replay");
                state.status.error = Some(error.clone());
                Err(error)
            }
        }
    }
    pub async fn before_model(&self, step: usize, transcript: &[Message], tools: &[ToolSpec]) -> Result<()> {
        self.write(CheckpointPhase::BeforeModel, |state| {
            state.step = step; state.transcript = transcript.to_vec();
            state.selected_tools = tools.iter().map(|t| t.name.clone()).collect();
            state.pending_tools.clear();
        }).await
    }
    pub async fn input(&self, transcript: &[Message]) -> Result<()> {
        self.write(CheckpointPhase::InputApplied, |state| state.transcript = transcript.to_vec()).await
    }
    pub async fn after_model(&self, transcript: &[Message], calls: &[ToolCall]) -> Result<()> {
        self.write(CheckpointPhase::AfterModel, |state| {
            state.transcript = transcript.to_vec();
            state.pending_tools = calls.iter().map(|c| (c.id.clone(), CheckpointToolState::Pending)).collect();
        }).await
    }
    pub async fn intent(&self, call_id: &str) -> Result<()> {
        self.write(CheckpointPhase::ToolIntent { call_id: call_id.to_owned() }, |state| {
            state.pending_tools.insert(call_id.to_owned(), CheckpointToolState::IntentRecorded);
        }).await
    }
    pub async fn settled(&self, result: &ToolResult) -> Result<()> {
        self.write(CheckpointPhase::ToolSettled { call_id: result.call_id.clone() }, |state| {
            state.pending_tools.insert(result.call_id.clone(), CheckpointToolState::Settled { result: result.clone() });
        }).await
    }
    pub async fn after_tools(&self, transcript: &[Message]) -> Result<()> {
        self.write(CheckpointPhase::AfterTools, |state| {
            state.transcript = transcript.to_vec(); state.pending_tools.clear();
        }).await
    }
    pub async fn finish(&self, transcript: &[Message], status: RunStatus, error: Option<AgentError>) -> Result<()> {
        self.write(CheckpointPhase::RunFinished, |state| {
            state.transcript = transcript.to_vec(); state.pending_tools.clear();
            state.status = Some(status); state.error = error;
        }).await
    }
}
