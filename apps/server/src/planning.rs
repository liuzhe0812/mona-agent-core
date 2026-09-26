//! Product assembly for the optional planner. No browser-supplied seed or Runtime changes.
#[cfg(test)]
#[path = "planning_tests.rs"]
mod tests;
use api::{AgentError, ErrorCode, RunCheckpoint, RunRequest};
#[cfg(not(feature = "sandbox"))]
use api::{async_trait, CancellationToken, CheckpointSink};
use planner::{PlanMode, PlanSnapshot};
use sessions::{Document, HostState, SessionError, SessionErrorCode, SessionResult, Store};
use std::{collections::BTreeMap, sync::Arc};

pub const STATE_KEY: &str = "planner";
fn invalid(message: &str) -> SessionError {
    SessionError::new(SessionErrorCode::InvalidRequest, message)
}
fn failure(error: AgentError) -> SessionError { invalid(&error.message) }

pub fn state(doc: &Document) -> SessionResult<PlanSnapshot> {
    if let Some(value) = doc.body.state.get(STATE_KEY) {
        let state: PlanSnapshot = serde_json::from_value(value.clone())
            .map_err(|_| invalid("已保存的计划状态无效，未重置为空计划。"))?;
        state.validate().map_err(failure)?;
        return Ok(state);
    }
    let recovered = if let Some(cp) = &doc.body.checkpoint {
        planner::recover_checkpoint(cp).map_err(failure)?
    } else {
        let history = doc.history()?;
        if history.is_empty() { None } else { planner::recover_history(&history).map_err(failure)? }
    };
    Ok(recovered.unwrap_or_default())
}

pub fn context(active: bool) -> application::sessions::SessionContext {
    Arc::new(move |doc| {
        let plan = state(doc)?;
        if !active {
            if plan.mode == PlanMode::PlanOnly {
                return Err(invalid("此会话处于计划模式，但 Planner 当前未启用；请启用组件并重启，或新建会话。"));
            }
            return Ok(BTreeMap::new());
        }
        let mut request = RunRequest::new("");
        planner::bind_state(&mut request, &plan).map_err(failure)?;
        Ok(request.metadata)
    })
}

pub(crate) fn checkpoint_state(cp: &RunCheckpoint) -> api::Result<HostState> {
    let mut patch = HostState::new();
    // An unbound/disabled Run cannot restore an obsolete mode from old tool history.
    if cp.metadata.contains_key(planner::PLAN_SEED_KEY) {
        if let Some(state) = planner::recover_checkpoint(cp)? {
            patch.insert(STATE_KEY.into(), serde_json::to_value(state)
                .map_err(|_| AgentError::new(ErrorCode::Checkpoint, "plan state encoding failed"))?);
        }
    }
    Ok(patch)
}

/// A plan result and its current session projection are acknowledged in ONE file transaction.
/// Ordinary SessionSink users remain independent of Planner.
#[cfg(not(feature = "sandbox"))]
pub struct PlanningSink(pub Arc<Store>);
#[cfg(not(feature = "sandbox"))]
#[async_trait]
impl CheckpointSink for PlanningSink {
    async fn commit(&self, cp: Arc<RunCheckpoint>, cancel: CancellationToken) -> api::Result<()> {
        let store = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let patch = checkpoint_state(&cp)?;
            store.commit_with_state(&cp, &cancel, &patch)
                .map_err(|e| AgentError::new(ErrorCode::Checkpoint, e.message))
        }).await.map_err(|_| AgentError::new(ErrorCode::Checkpoint, "plan persistence worker failed"))?
    }
}

#[derive(serde::Serialize)]
pub struct PlanView {
    pub session_id: String,
    pub revision: u64,
    pub status: sessions::Status,
    pub enabled: bool,
    pub plan: PlanSnapshot,
}
pub fn view(doc: Document, enabled: bool) -> SessionResult<PlanView> {
    let plan = state(&doc)?;
    Ok(PlanView { session_id: doc.header.id, revision: doc.header.revision,
        status: doc.header.status, enabled, plan })
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanAction {
    pub revision: u64,
    pub plan_revision: u64,
    pub action: Action,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action { Plan, Normal, Refine, Resume, Reset }

pub fn change(store: &Store, id: &str, request: PlanAction, enabled: bool) -> SessionResult<()> {
    if !enabled { return Err(invalid("当前宿主未启用 Planner，设置保存后需要重启才生效。")); }
    store.update_state(id, request.revision, STATE_KEY, |doc| {
        let old = state(doc)?;
        if old.revision != request.plan_revision {
            return Err(SessionError::new(SessionErrorCode::Conflict, "计划已更新，请重新读取后操作。"));
        }
        let next = match request.action {
            Action::Plan | Action::Refine => old.enter_plan_mode(old.revision).map_err(failure)?,
            Action::Resume => old.resume_execution(old.revision).map_err(failure)?,
            Action::Reset => old.reset(old.revision).map_err(failure)?,
            Action::Normal => {
                if old.awaits_host() { return Err(invalid("方案已提交，请明确选择按此计划执行或继续修改。")); }
                let mut next = old.clone();
                if next.mode != PlanMode::Normal {
                    next.mode = PlanMode::Normal;
                    next.revision = next.revision.checked_add(1).ok_or_else(|| invalid("计划修订号已耗尽。"))?;
                }
                next.validate().map_err(failure)?;
                next
            }
        };
        serde_json::to_value(next).map_err(|_| invalid("计划编码失败。"))
    })?;
    Ok(())
}
