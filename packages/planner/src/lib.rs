//! Optional single-agent plan management. No model calls, workflow loop, Web UI or storage backend.
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
mod history;
mod plugin;
mod state;

pub use history::{
    bind_state, recover_checkpoint, recover_history, PLAN_READ, PLAN_SEED_KEY, PLAN_SUBMIT,
    PLAN_UPDATE,
};
pub use plugin::{Planner, PlannerConfig, PlannerPlugin};
pub use state::{
    PlanMode, PlanSnapshot, PlanStep, PlanUpdate, StepStatus, MAX_PLAN_BYTES, MAX_PROPOSAL_BYTES,
    MAX_STEPS, MAX_STEP_BYTES, PLAN_VERSION,
};
