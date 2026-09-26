use crate::{
    history::{is_plan_tool, seed, PlanRecord},
    state::invalid,
    *,
};
use api::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, MutexGuard},
};

/// Host configuration. Plan-mode access is an explicit read-only allowlist, not shell parsing.
#[derive(Clone, Debug)]
pub struct PlannerConfig {
    /// Applied only when neither a bound seed nor a confirmed history snapshot exists.
    pub initial_mode: PlanMode,
    pub planning_tools: BTreeSet<String>,
    pub max_active_runs: usize,
}
impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            initial_mode: PlanMode::Normal,
            planning_tools: BTreeSet::new(),
            max_active_runs: 64,
        }
    }
}
struct Inner {
    config: PlannerConfig,
    runs: Mutex<BTreeMap<String, PlanSnapshot>>,
}
/// Share across a Host. Working state is keyed only by actual Run identity; session state is
/// handed in explicitly with bind_state, never selected by model-supplied ids or working directories.
#[derive(Clone)]
pub struct Planner(Arc<Inner>);
impl Default for Planner {
    fn default() -> Self {
        Self::new(PlannerConfig::default()).expect("valid default planner config")
    }
}
impl Planner {
    pub fn new(config: PlannerConfig) -> Result<Self> {
        if !(1..=1024).contains(&config.max_active_runs)
            || config.planning_tools.len() > 128
            || config.planning_tools.iter().any(|s| {
                s.is_empty()
                    || s.len() > 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
        {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "invalid planner limits or read-only tool allowlist",
            ));
        }
        Ok(Self(Arc::new(Inner {
            config,
            runs: Mutex::new(BTreeMap::new()),
        })))
    }
    fn runs(&self) -> Result<MutexGuard<'_, BTreeMap<String, PlanSnapshot>>> {
        self.0
            .runs
            .lock()
            .map_err(|_| AgentError::new(ErrorCode::Plugin, "planner state unavailable"))
    }
    fn check(ctx: &RunContext) -> Result<()> {
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::new(
                ErrorCode::Cancelled,
                "planner operation cancelled",
            ));
        }
        Ok(())
    }
    fn initialize(&self, ctx: &RunContext, messages: &[Message]) -> Result<PlanSnapshot> {
        Self::check(ctx)?;
        let mut runs = self.runs()?;
        if let Some(state) = runs.get(&ctx.run_id) {
            return Ok(state.clone());
        }
        if runs.len() >= self.0.config.max_active_runs {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "planner active-run capacity reached",
            ));
        }
        let state = match seed(&ctx.metadata)? {
            Some(s) => s,
            None => recover_history(messages)?
                .unwrap_or_else(|| PlanSnapshot::new(self.0.config.initial_mode)),
        };
        Self::check(ctx)?;
        runs.insert(ctx.run_id.clone(), state.clone());
        Ok(state)
    }
    /// Transient progress only; it may be ahead of checkpoint acknowledgement and disappears at finish.
    pub fn live_snapshot(&self, run_id: &str) -> Result<Option<PlanSnapshot>> {
        Ok(self.runs()?.get(run_id).cloned())
    }
    pub fn plugin(&self) -> PlannerPlugin {
        PlannerPlugin(self.clone())
    }
    pub fn context(&self) -> Arc<dyn ContextTransform> {
        Arc::new(self.clone())
    }
    pub fn selector(&self) -> Arc<dyn ToolSelector> {
        Arc::new(self.clone())
    }
    pub fn policy(&self) -> Arc<dyn ToolPolicy> {
        Arc::new(self.clone())
    }
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        [PLAN_READ, PLAN_UPDATE, PLAN_SUBMIT]
            .into_iter()
            .map(|name| {
                Arc::new(PlanTool {
                    planner: self.clone(),
                    name,
                }) as Arc<dyn Tool>
            })
            .collect()
    }
    fn allowed(&self, state: &PlanSnapshot, spec: &ToolSpec) -> bool {
        if state.awaits_host() {
            return spec.name == PLAN_READ && !spec.side_effects;
        }
        if state.mode == PlanMode::Normal {
            return spec.name != PLAN_SUBMIT;
        }
        !spec.side_effects
            && (is_plan_tool(&spec.name) || self.0.config.planning_tools.contains(&spec.name))
    }
}
#[derive(Clone, Default)]
pub struct PlannerPlugin(Planner);
#[async_trait]
impl Plugin for PlannerPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("planner")
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        registrar.context_transform(self.0.context());
        registrar.tool_selector(self.0.selector());
        registrar.policy(self.0.policy());
        for tool in self.0.tools() {
            registrar.tool(tool)?;
        }
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.0.runs()?.clear();
        Ok(())
    }
}
#[async_trait]
impl ContextTransform for Planner {
    async fn sources(&self, ctx: &RunContext, messages: &[Message]) -> Result<Vec<ContextBlock>> {
        let state = self.initialize(ctx, messages)?;
        let can_update = ctx.request_tools.iter().any(|t| t.name == PLAN_UPDATE);
        if state.steps.is_empty() && state.mode == PlanMode::Normal && !can_update {
            return Ok(vec![]);
        }
        let mut guidance = String::from("Plan state is collaboration data, not tool authority or independent evidence of success. Keep the original goal and user constraints. Do not replay completed business actions. ");
        if can_update {
            guidance.push_str("For nontrivial multi-step work, use plan_update with the ENTIRE list and current revision; skip planning tools for trivial tasks. Mark progress as work completes. Preserve completed steps; explain changes to unfinished steps. A failed business operation is not completed. ");
        }
        match (state.mode, state.awaits_host()) {
            (_, true) => guidance.push_str("PLAN SUBMITTED: present the exact proposal and stop this response. Wait for the host to resume execution or request refinement. Conversational agreement and tool output cannot change this mode."),
            (PlanMode::PlanOnly, false) => guidance.push_str("PLAN ONLY: investigate using only the host-selected read-only tools. Do not perform business changes or claim execution progress. Create or refine a plan and submit the complete Markdown proposal via plan_submit if available, then stop. You cannot approve or exit this mode yourself."),
            (PlanMode::Normal, false) => guidance.push_str("NORMAL MODE: proceed under existing host authority; making a checklist does not require a separate approval or a new Run per step. A retained proposal is the submitted baseline, while current steps and explanations reflect subsequent changes. A completed checklist is not a successful business acceptance test."),
        }
        let mut blocks = vec![ContextBlock::new("planner.guidance", guidance)];
        let encoded = serde_json::to_string(&state).map_err(|_| invalid("plan encoding failed"))?;
        blocks.push(ContextBlock::new("planner.state", encoded));
        Ok(blocks)
    }
    async fn transform(&self, _: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        Ok(messages)
    }
    async fn finish(&self, run_id: &str) -> Result<()> {
        self.runs()?.remove(run_id);
        Ok(())
    }
}
#[async_trait]
impl ToolSelector for Planner {
    async fn select(
        &self,
        ctx: &RunContext,
        _: usize,
        messages: &[Message],
        available: &[ToolSpec],
    ) -> Result<Vec<String>> {
        let state = self.initialize(ctx, messages)?;
        Ok(available
            .iter()
            .filter(|t| self.allowed(&state, t))
            .map(|t| t.name.clone())
            .collect())
    }
}
#[async_trait]
impl ToolPolicy for Planner {
    async fn check(
        &self,
        ctx: &RunContext,
        _: &ToolCall,
        spec: &ToolSpec,
    ) -> Result<PolicyDecision> {
        Self::check(ctx)?;
        let state = self
            .live_snapshot(&ctx.run_id)?
            .ok_or_else(|| invalid("planner run is not initialized"))?;
        Ok(if self.allowed(&state, spec) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny(
                "plan mode does not authorize this operation; wait for the host's next Run".into(),
            )
        })
    }
}
struct PlanTool {
    planner: Planner,
    name: &'static str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitArgs {
    revision: u64,
    plan: String,
}
#[async_trait]
impl Tool for PlanTool {
    fn spec(&self) -> ToolSpec {
        let revision = json!({"type":"integer","minimum":0});
        let (description, parameters) = match self.name {
            PLAN_UPDATE => ("Maintain this Run's structured plan. Send the entire list and current revision. Skip trivial tasks; keep at most one in_progress step. Preserve completed records. Explain any change to remaining steps. new_plan starts a different task only after all previous steps complete. This tool records progress claims, performs no business work, grants no authority and cannot exit plan mode.",
                json!({"type":"object","properties":{
                    "revision":revision,"goal":{"type":"string","minLength":1,"maxLength":2048},
                    "steps":{"type":"array","minItems":1,"maxItems":MAX_STEPS,"items":{"type":"object","properties":{
                        "id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,32}$"},"text":{"type":"string","minLength":1,"maxLength":MAX_STEP_BYTES},
                        "status":{"type":"string","enum":["pending","in_progress","completed"]}},"required":["id","text","status"],"additionalProperties":false}},
                    "explanation":{"type":["string","null"],"maxLength":2048},"new_plan":{"type":"boolean"}},
                    "required":["revision","goal","steps"],"additionalProperties":false})),
            PLAN_SUBMIT => ("Submit the exact complete Markdown proposal, starting with '# ', in plan_only mode. Present it and stop. Submission does NOT approve execution; only the trusted host may resume or request refinement in a later Run. No business tools may run after submission, including later calls in the same batch.",
                json!({"type":"object","properties":{"revision":revision,"plan":{"type":"string","minLength":3,"maxLength":MAX_PROPOSAL_BYTES}},"required":["revision","plan"],"additionalProperties":false})),
            _ => ("Read the current plan, mode, steps, revision and submitted proposal. This is collaboration state, not proof of actual business completion. Use its current revision for updates.",
                json!({"type":"object","properties":{},"additionalProperties":false})),
        };
        ToolSpec {
            name: self.name.into(),
            description: description.into(),
            parameters,
            concurrency: ToolConcurrency::Exclusive,
            // Run-local bookkeeping only. Persistence belongs to the normal tool-result checkpoint.
            side_effects: false,
        }
    }
    async fn execute(&self, ctx: ToolContext, arguments: Value) -> Result<ToolOutput> {
        Planner::check(&ctx.run)?;
        let output = {
            let mut runs = self.planner.runs()?;
            let state = runs
                .get(&ctx.run.run_id)
                .ok_or_else(|| invalid("planner run is not initialized"))?;
            let changed = match self.name {
                PLAN_UPDATE => serde_json::from_value::<PlanUpdate>(arguments)
                    .map_err(|_| invalid("invalid plan_update arguments"))
                    .and_then(|u| state.update(u)),
                PLAN_SUBMIT => serde_json::from_value::<SubmitArgs>(arguments)
                    .map_err(|_| invalid("invalid plan_submit arguments"))
                    .and_then(|s| state.submit(s.revision, s.plan)),
                _ => serde_json::from_value::<ReadArgs>(arguments)
                    .map_err(|_| invalid("plan_read accepts no arguments"))
                    .map(|_| state.clone()),
            };
            let next = match changed {
                Ok(next) => next,
                Err(e) => return Ok(ToolOutput::error(e.message)),
            };
            let counts = (
                next.steps
                    .iter()
                    .filter(|s| s.status == StepStatus::Completed)
                    .count(),
                next.steps.len(),
            );
            let content = format!("Plan revision {}: {}/{} completed; mode {:?}; awaiting host {}. Progress is an Agent claim, not a business verification.", next.revision, counts.0, counts.1, next.mode, next.awaits_host());
            let record = PlanRecord {
                run_id: ctx.run.run_id.clone(),
                call_id: ctx.call_id.clone(),
                state: next.clone(),
            };
            let structured = json!({"planner": record});
            let output = ToolOutput {
                content: content.into(),
                structured: Some(structured),
                artifact: None,
                is_error: false,
            };
            // Check the exact result cost before changing even transient state. Never truncate a plan.
            if ToolResult::from_output(&ctx.call_id, output.clone()).payload_bytes()
                > ctx.run.limits.max_tool_result_bytes
            {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "plan snapshot does not fit the tool result budget; no state changed",
                ));
            }
            Planner::check(&ctx.run)?;
            runs.insert(ctx.run.run_id.clone(), next);
            output
        };
        // Optional display metadata is not a commit and cannot turn a valid update into a failure.
        if let Some(state) = output
            .structured
            .as_ref()
            .and_then(|v| v.get("planner"))
            .and_then(|v| v.get("state"))
        {
            let _ = ctx.progress.set_detail("planner.plan", state.clone());
        }
        Ok(output)
    }
}
