//! Bounded collaboration state. A checked step is a progress claim, not proof of a tool effect.
use api::{AgentError, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PLAN_VERSION: u32 = 1;
pub const MAX_PLAN_BYTES: usize = 12 * 1024;
pub const MAX_STEPS: usize = 32;
pub const MAX_STEP_BYTES: usize = 1024;
pub const MAX_PROPOSAL_BYTES: usize = 6 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    #[default]
    Normal,
    PlanOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    pub id: String,
    pub text: String,
    pub status: StepStatus,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSnapshot {
    pub version: u32,
    pub revision: u64,
    pub mode: PlanMode,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    /// The last explicit explanation, retained through ordinary progress updates.
    pub explanation: Option<String>,
    /// Exact submitted Markdown, retained as the execution baseline after host continuation.
    pub proposal: Option<String>,
}
impl Default for PlanSnapshot {
    fn default() -> Self {
        Self::new(PlanMode::Normal)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanUpdate {
    pub revision: u64,
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub explanation: Option<String>,
    /// An explicit new task, allowed only after the preceding plan is complete.
    #[serde(default)]
    pub new_plan: bool,
}

pub(crate) fn invalid(message: &str) -> AgentError {
    AgentError::new(ErrorCode::Tool, message)
}
pub(crate) fn text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}
impl PlanSnapshot {
    pub fn new(mode: PlanMode) -> Self {
        Self {
            version: PLAN_VERSION,
            revision: 0,
            mode,
            goal: String::new(),
            steps: vec![],
            explanation: None,
            proposal: None,
        }
    }
    pub fn is_complete(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.status == StepStatus::Completed)
    }
    /// Collaboration state only; this does not suspend a Runtime task.
    pub fn awaits_host(&self) -> bool {
        self.mode == PlanMode::PlanOnly && self.proposal.is_some()
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != PLAN_VERSION {
            return Err(invalid("unsupported plan snapshot version"));
        }
        if self.steps.len() > MAX_STEPS
            || self.goal.is_empty() != self.steps.is_empty()
            || (!self.goal.is_empty() && !text(&self.goal, 2048))
        {
            return Err(invalid(
                "plan needs a nonempty goal and 1..32 steps, or an empty state",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut titles = BTreeSet::new();
        let mut active = 0;
        for s in &self.steps {
            if s.id.is_empty()
                || s.id.len() > 32
                || !s
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                || !ids.insert(&s.id)
                || !text(&s.text, MAX_STEP_BYTES)
                || !titles.insert(&s.text)
            {
                return Err(invalid(
                    "plan step ids and texts must be nonempty, bounded and unique",
                ));
            }
            active += usize::from(s.status == StepStatus::InProgress);
        }
        if active > 1 {
            return Err(invalid("at most one plan step may be in_progress"));
        }
        if self.explanation.as_ref().is_some_and(|s| !text(s, 2048)) {
            return Err(invalid(
                "plan explanation must be nonempty and at most 2048 UTF-8 bytes",
            ));
        }
        if let Some(proposal) = &self.proposal {
            if self.steps.is_empty()
                || (self.mode == PlanMode::PlanOnly && self.is_complete())
                || !text(proposal, MAX_PROPOSAL_BYTES)
                || !proposal.starts_with("# ")
            {
                return Err(invalid("a proposal needs plan steps and headed Markdown; plan_only must have remaining work"));
            }
        }
        if serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_PLAN_BYTES) {
            return Err(invalid("plan snapshot exceeds the 12 KiB serialized limit"));
        }
        Ok(())
    }
    fn checked(&self, revision: u64) -> Result<()> {
        self.validate()?;
        if self.revision != revision {
            return Err(invalid(
                "plan revision conflict; read the current plan before updating",
            ));
        }
        Ok(())
    }
    fn advance(mut self) -> Result<Self> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("plan revision exhausted"))?;
        self.validate()?;
        Ok(self)
    }
    /// Whole-list replacement; completed records cannot be changed or silently removed.
    pub fn update(&self, update: PlanUpdate) -> Result<Self> {
        self.checked(update.revision)?;
        if self.awaits_host() {
            return Err(invalid(
                "plan is submitted; only the host can request refinement or execution",
            ));
        }
        if update.steps.is_empty() {
            return Err(invalid("plan_update requires at least one step"));
        }
        let new_task = self.steps.is_empty() || update.new_plan;
        if update.new_plan && !self.steps.is_empty() && !self.is_complete() {
            return Err(invalid(
                "finish the current plan or ask the host to reset it before starting a new plan",
            ));
        }
        let mut needs_reason = update.new_plan && !self.steps.is_empty();
        if !new_task {
            if update.goal != self.goal {
                return Err(invalid(
                    "the goal of an existing plan cannot be silently replaced",
                ));
            }
            let old_completed: Vec<_> = self
                .steps
                .iter()
                .filter(|s| s.status == StepStatus::Completed)
                .collect();
            let kept_completed: Vec<_> = update
                .steps
                .iter()
                .filter(|s| old_completed.iter().any(|old| old.id == s.id))
                .collect();
            if old_completed != kept_completed {
                return Err(invalid(
                    "completed steps must retain their identity, text, order and completed status",
                ));
            }
            needs_reason |= self
                .steps
                .iter()
                .map(|s| (&s.id, &s.text))
                .collect::<Vec<_>>()
                != update
                    .steps
                    .iter()
                    .map(|s| (&s.id, &s.text))
                    .collect::<Vec<_>>();
            needs_reason |= self.steps.iter().any(|old| {
                old.status == StepStatus::InProgress
                    && update
                        .steps
                        .iter()
                        .any(|s| s.id == old.id && s.status == StepStatus::Pending)
            });
        }
        if needs_reason && update.explanation.as_ref().is_none_or(|s| !text(s, 2048)) {
            return Err(invalid(
                "explain why unfinished steps were added, removed, reordered or revised",
            ));
        }
        if self.mode == PlanMode::PlanOnly {
            for s in &update.steps {
                let expected = if new_task {
                    StepStatus::Pending
                } else {
                    self.steps
                        .iter()
                        .find(|old| old.id == s.id)
                        .map_or(StepStatus::Pending, |old| old.status)
                };
                if s.status != expected {
                    return Err(invalid(
                        "plan_only cannot claim execution progress; new steps must be pending",
                    ));
                }
            }
        }
        let next = Self {
            version: PLAN_VERSION,
            revision: self.revision,
            mode: self.mode,
            goal: update.goal,
            steps: update.steps,
            proposal: if new_task {
                None
            } else {
                self.proposal.clone()
            },
            explanation: update.explanation.or_else(|| {
                if new_task {
                    None
                } else {
                    self.explanation.clone()
                }
            }),
        };
        next.validate()?;
        if &next == self {
            return Ok(next);
        }
        next.advance()
    }
    pub fn submit(&self, revision: u64, proposal: String) -> Result<Self> {
        self.checked(revision)?;
        if self.mode != PlanMode::PlanOnly || self.proposal.is_some() {
            return Err(invalid(
                "plan_submit requires an unsubmitted plan_only state",
            ));
        }
        let mut next = self.clone();
        next.proposal = Some(proposal);
        next.advance()
    }
    /// Trusted host operation, not exposed as a model tool. Also reopens a submitted plan for feedback.
    pub fn enter_plan_mode(&self, revision: u64) -> Result<Self> {
        self.checked(revision)?;
        let mut next = self.clone();
        next.mode = PlanMode::PlanOnly;
        next.proposal = None;
        next.advance()
    }
    /// Continue the exact submitted revision in a later Run; does not authorize business tools.
    pub fn resume_execution(&self, revision: u64) -> Result<Self> {
        self.checked(revision)?;
        if self.mode != PlanMode::PlanOnly || self.proposal.is_none() {
            return Err(invalid("only a submitted plan can be resumed by the host"));
        }
        let mut next = self.clone();
        next.mode = PlanMode::Normal;
        // Retain detailed constraints that may not fit in step titles or a later summary.
        next.advance()
    }
    /// Explicit host abandonment/new-task boundary. Old snapshots remain in the host's history.
    pub fn reset(&self, revision: u64) -> Result<Self> {
        self.checked(revision)?;
        let mut next = Self::new(self.mode);
        next.revision = self.revision;
        next.advance()
    }
}
