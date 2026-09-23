use crate::{
    AgentError, CheckpointStatus, ErrorCode, Message, ModelOptions, ModelRequest, Result, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub const MAX_TOOL_CALLS_PER_STEP: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditMode {
    Full,
    Metadata,
}

#[derive(Clone, Debug)]
pub struct ModelRetryPolicy {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}
impl Default for ModelRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskLimits {
    pub wall_time: Duration,
    pub max_model_calls: u64,
    /// A circuit breaker on *reported* usage; not a prepaid billing guarantee.
    pub max_reported_tokens: Option<u64>,
}
impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            wall_time: Duration::from_secs(300),
            max_model_calls: 32,
            max_reported_tokens: None,
        }
    }
}

struct TaskInner {
    cancel: CancellationToken,
    deadline: Instant,
    limits: TaskLimits,
    calls: AtomicU64,
    tokens: AtomicU64,
    usage_incomplete: AtomicBool,
}
#[derive(Clone)]
pub struct TaskControl(Arc<TaskInner>);
impl Default for TaskControl {
    fn default() -> Self {
        Self::new(TaskLimits::default())
    }
}
impl TaskControl {
    pub fn new(limits: TaskLimits) -> Self {
        Self(Arc::new(TaskInner {
            cancel: CancellationToken::new(),
            deadline: Instant::now() + limits.wall_time,
            limits,
            calls: AtomicU64::new(0),
            tokens: AtomicU64::new(0),
            usage_incomplete: AtomicBool::new(false),
        }))
    }
    pub fn cancel(&self) {
        self.0.cancel.cancel();
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.0.cancel.clone()
    }
    pub fn deadline(&self) -> Instant {
        self.0.deadline
    }
    pub fn check(&self) -> Result<()> {
        if self.0.cancel.is_cancelled() {
            return Err(AgentError::new(ErrorCode::Cancelled, "task cancelled"));
        }
        if Instant::now() >= self.0.deadline {
            return Err(AgentError::new(
                ErrorCode::Deadline,
                "task deadline reached",
            ));
        }
        if self
            .0
            .limits
            .max_reported_tokens
            .is_some_and(|n| self.0.tokens.load(Ordering::SeqCst) >= n)
        {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "reported-token circuit breaker reached",
            ));
        }
        Ok(())
    }
    /// Non-reserving preflight for work such as retry waits. A successful check
    /// is not a reservation; concurrent callers must still reserve atomically.
    /// Kept separate from check() so the last permitted successful call can finish.
    pub fn check_model_call_available(&self) -> Result<()> {
        self.check()?;
        if self.0.calls.load(Ordering::SeqCst) >= self.0.limits.max_model_calls {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "task model-call limit reached",
            ));
        }
        Ok(())
    }
    pub fn reserve_model_call(&self) -> Result<()> {
        self.check()?;
        let max = self.0.limits.max_model_calls;
        self.0
            .calls
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                if n < max {
                    Some(n + 1)
                } else {
                    None
                }
            })
            .map_err(|_| AgentError::new(ErrorCode::Limit, "task model-call limit reached"))?;
        Ok(())
    }
    pub fn record_usage(&self, usage: Option<Usage>) {
        if let Some(usage) = usage {
            let _ = self
                .0
                .tokens
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    Some(n.saturating_add(usage.total()))
                });
        } else {
            self.0.usage_incomplete.store(true, Ordering::SeqCst);
        }
    }
    pub fn mark_usage_incomplete(&self) {
        self.0.usage_incomplete.store(true, Ordering::SeqCst);
    }
    pub fn usage(&self) -> TaskUsage {
        TaskUsage {
            model_calls: self.0.calls.load(Ordering::SeqCst),
            reported_tokens: self.0.tokens.load(Ordering::SeqCst),
            usage_complete: !self.0.usage_incomplete.load(Ordering::SeqCst),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskUsage {
    pub model_calls: u64,
    pub reported_tokens: u64,
    pub usage_complete: bool,
}

#[derive(Clone, Debug)]
pub struct RunLimits {
    pub max_steps: usize,
    pub max_parallel_tools: usize,
    pub model_timeout: Duration,
    pub tool_timeout: Duration,
    pub hook_timeout: Duration,
    /// Context projection may perform a budgeted auxiliary model call.
    pub context_timeout: Duration,
    pub checkpoint_timeout: Duration,
    pub max_checkpoint_bytes: usize,
    pub cancellation_grace: Duration,
    /// Maximum canonical history accepted at Run start before any projection.
    pub max_initial_history_bytes: usize,
    /// Maximum serialized canonical transcript retained during a Run, including tool results.
    /// Independent of model projection and checkpoint storage. Never silently evicts history.
    pub max_history_bytes: usize,
    /// Maximum serialized request sent to a model after context transforms.
    pub max_context_bytes: usize,
    pub max_response_bytes: usize,
    pub max_tool_result_bytes: usize,
    pub max_audit_bytes: usize,
    pub max_output_tokens: u32,
    pub max_tools_per_step: usize,
    pub model_retry: ModelRetryPolicy,
    pub audit_mode: AuditMode,
}
impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_steps: 16,
            max_parallel_tools: 4,
            model_timeout: Duration::from_secs(90),
            tool_timeout: Duration::from_secs(30),
            hook_timeout: Duration::from_secs(5),
            context_timeout: Duration::from_secs(60),
            checkpoint_timeout: Duration::from_secs(5),
            max_checkpoint_bytes: 8 * 1024 * 1024,
            cancellation_grace: Duration::from_millis(200),
            max_initial_history_bytes: 4 * 1024 * 1024,
            max_history_bytes: 8 * 1024 * 1024,
            max_context_bytes: 256 * 1024,
            max_response_bytes: 1024 * 1024,
            max_tool_result_bytes: 64 * 1024,
            max_audit_bytes: 4 * 1024 * 1024,
            max_output_tokens: 4096,
            max_tools_per_step: 32,
            model_retry: ModelRetryPolicy::default(),
            audit_mode: AuditMode::Full,
        }
    }
}
impl RunLimits {
    pub fn validate(&self) -> Result<()> {
        if self.max_tools_per_step > MAX_TOOL_CALLS_PER_STEP {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                format!("max_tools_per_step must be at most {MAX_TOOL_CALLS_PER_STEP}"),
            ));
        }
        if self.max_steps == 0
            || self.max_parallel_tools == 0
            || self.max_initial_history_bytes < self.max_context_bytes
            || self.max_context_bytes == 0
            || self.max_history_bytes < 128
            || self.max_response_bytes == 0
            || self.max_tool_result_bytes < 128
            || self.max_checkpoint_bytes == 0
            || self.checkpoint_timeout.is_zero()
            || self.max_audit_bytes == 0
            || self.max_output_tokens == 0
            || self.max_tools_per_step == 0
            || self.model_timeout.is_zero()
            || self.tool_timeout.is_zero()
            || self.hook_timeout.is_zero()
            || self.context_timeout.is_zero()
            || self.model_retry.initial_delay.is_zero()
            || self.model_retry.max_delay.is_zero()
            || self.model_retry.initial_delay > self.model_retry.max_delay
        {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "run limits must be positive; tool result cap must be >= 128",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct RunRequest {
    pub messages: Vec<Message>,
    pub limits: RunLimits,
    pub task: TaskControl,
    pub metadata: BTreeMap<String, String>,
    /// Tools can be disabled for a planning-only child run.
    pub enable_tools: bool,
    /// Trusted per-run ceiling. None means all registered tools; Some(empty) means none.
    pub allowed_tools: Option<BTreeSet<String>>,
    pub model_options: ModelOptions,
}
impl RunRequest {
    pub fn new(prompt: impl Into<crate::Content>) -> Self {
        Self {
            messages: vec![Message::user(prompt)],
            limits: RunLimits::default(),
            task: TaskControl::default(),
            metadata: BTreeMap::new(),
            enable_tools: true,
            allowed_tools: None,
            model_options: ModelOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Limited,
}
impl RunStatus {
    pub fn from_error(error: &AgentError) -> Self {
        match error.code {
            ErrorCode::Cancelled => Self::Cancelled,
            ErrorCode::Deadline => Self::TimedOut,
            ErrorCode::Limit => Self::Limited,
            _ => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestAudit {
    pub call_number: u64,
    pub request_bytes: usize,
    /// Full mode keeps the body. Metadata mode emits an explicit JSON null;
    /// a missing field in older metadata records remains deserializable.
    #[serde(default)]
    pub request: Option<ModelRequest>,
    pub error: Option<AgentError>,
    pub usage: Option<Usage>,
    pub settled: bool,
    /// Full-mode-only interrupted output evidence; never fed back automatically.
    pub partial_text: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunReport {
    pub run_id: String,
    pub status: RunStatus,
    pub output: Option<String>,
    pub error: Option<AgentError>,
    pub transcript: Vec<Message>,
    pub model_requests: Vec<RequestAudit>,
    pub task_usage: TaskUsage,
    pub steps: usize,
    #[serde(default)]
    pub checkpoint: CheckpointStatus,
}

#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn execute(&self, request: RunRequest) -> Result<Arc<RunReport>>;
}
