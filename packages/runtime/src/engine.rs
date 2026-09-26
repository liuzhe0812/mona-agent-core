use crate::{
    checkpoint::Checkpoints,
    events::{EventBus, TextSink},
    gate::{bounded, lifecycle, lock, CancelOnDrop},
    history::{serialized_bytes, History, UNKNOWN_RESULT},
    host::Registry,
    model::Gateway,
    tools::execute_batch,
};
use api::*;
use futures_util::FutureExt;
use std::{
    collections::{BTreeSet, HashMap},
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch, Notify},
    time::{timeout, Instant},
};

#[derive(Clone)]
pub(crate) struct EngineConfig {
    pub allowed_side_effect_tools: BTreeSet<String>,
    pub max_concurrent_runs: usize,
    pub event_capacity: usize,
}
impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            allowed_side_effect_tools: BTreeSet::new(),
            max_concurrent_runs: 32,
            event_capacity: 128,
        }
    }
}
#[derive(Default)]
struct Admission {
    closed: bool,
    runs: HashMap<String, CancellationToken>,
}
struct Inner {
    registry: RwLock<Option<Arc<Registry>>>,
    state: Mutex<Admission>,
    drained: Notify,
    config: EngineConfig,
}
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}
struct Lease {
    inner: Arc<Inner>,
    id: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        lock(&self.inner.state).runs.remove(&self.id);
        self.inner.drained.notify_one();
    }
}
struct Command {
    text: String,
    applied: oneshot::Sender<Result<()>>,
}

struct CoreSession {
    run_id: String,
    bus: EventBus,
    result: watch::Receiver<Option<Arc<RunReport>>>,
    cancel: CancellationToken,
    commands: mpsc::Sender<Command>,
}
#[async_trait]
impl RunSession for CoreSession {
    fn run_id(&self) -> &str {
        &self.run_id
    }
    fn cancel(&self) {
        self.cancel.cancel();
    }
    fn snapshot(&self) -> RunSnapshot {
        self.bus.snapshot()
    }
    fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.bus.subscribe()
    }
    async fn wait(&self) -> Result<Arc<RunReport>> {
        let mut result = self.result.clone();
        loop {
            let current = result.borrow().clone();
            if let Some(report) = current {
                return Ok(report);
            }
            result.changed().await.map_err(|_| {
                AgentError::new(ErrorCode::Closed, "run task lost without a report")
            })?;
        }
    }
    async fn steer(&self, text: String) -> Result<()> {
        if text.is_empty() || text.len() > 16 * 1024 {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "injected input must be 1..16384 bytes",
            ));
        }
        let (sender, receiver) = oneshot::channel();
        self.commands
            .try_send(Command {
                text,
                applied: sender,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    AgentError::new(ErrorCode::Limit, "run input queue is full")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    AgentError::new(ErrorCode::Closed, "run has stopped accepting input")
                }
            })?;
        receiver.await.map_err(|_| {
            AgentError::new(ErrorCode::Closed, "run ended before input could be applied")
        })?
    }
}

impl Engine {
    pub(crate) fn new(registry: Registry, config: EngineConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(Some(Arc::new(registry))),
                state: Mutex::new(Admission::default()),
                drained: Notify::new(),
                config,
            }),
        }
    }
    fn registry(&self) -> Result<Arc<Registry>> {
        self.inner
            .registry
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| AgentError::new(ErrorCode::Closed, "host registry has been revoked"))
    }
    pub fn services(&self) -> Result<Services> {
        if lock(&self.inner.state).closed {
            return Err(AgentError::new(ErrorCode::Closed, "host is closing"));
        }
        Ok(self.registry()?.services.without(MODEL_SERVICE))
    }
    pub fn start(&self, request: RunRequest) -> Result<RunHandle> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            AgentError::new(
                ErrorCode::Configuration,
                "Engine::start requires a Tokio runtime",
            )
        })?;
        request.limits.validate()?;
        request.model_options.validate()?;
        request.task.check()?;
        let transcript = History::new(request.messages, request.limits.max_initial_history_bytes,
            request.limits.max_history_bytes)?;
        validate_messages(&transcript)?;
        if request
            .metadata
            .iter()
            .map(|(k, v)| k.len().saturating_add(v.len()))
            .sum::<usize>()
            > 16 * 1024
        {
            return Err(AgentError::new(ErrorCode::Limit, "run metadata too large"));
        }
        let id = run_id();
        let cancel = request.task.cancellation().child_token();
        let registry = {
            let mut state = lock(&self.inner.state);
            if state.closed {
                return Err(AgentError::new(ErrorCode::Closed, "host is closing"));
            }
            if state.runs.len() >= self.inner.config.max_concurrent_runs {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "host concurrent-run limit reached",
                ));
            }
            let registry = self.registry()?;
            if request
                .allowed_tools
                .as_ref()
                .is_some_and(|names| names.iter().any(|n| !registry.tools.contains_key(n)))
            {
                return Err(AgentError::new(
                    ErrorCode::Configuration,
                    "run tool ceiling contains an unregistered tool",
                ));
            }
            state.runs.insert(id.clone(), cancel.clone());
            registry
        };
        let lease = Lease {
            inner: self.inner.clone(),
            id: id.clone(),
        };
        let provider = registry.services.get::<ModelProvider>(MODEL_SERVICE)?;
        // Reject incompatible replay before any transform can call a summarizer or erase it.
        provider.0.validate_history(&transcript, &request.model_options)?;
        let model_context_window_tokens =
            provider.0.context_window_tokens(&request.model_options);
        let gateway = Arc::new(Gateway::new(
            provider.0.clone(),
            request.task.clone(),
            cancel.clone(),
            request.limits.clone(),
            request.model_options.clone(),
        ));
        // No tool schema is charged until the per-round selector has fixed its view.
        let request_overhead_bytes = request_overhead(
            &[], request.limits.max_output_tokens, &request.model_options,
        )?;
        let context_tool_ceiling = request.allowed_tools.clone().map(Arc::new);
        let context = RunContext {
            run_id: id.clone(),
            task: request.task.clone(),
            cancel: cancel.clone(),
            model: gateway.clone(),
            services: registry.services.without(MODEL_SERVICE),
            metadata: Arc::new(request.metadata),
            model_options: request.model_options,
            limits: request.limits.clone(),
            request_overhead_bytes,
            model_context_window_tokens,
            request_tools: Arc::new(Vec::new()),
            context_sources: Arc::new(Vec::new()),
            tools_enabled: request.enable_tools,
            allowed_tools: context_tool_ceiling,
        };
        let checkpoints = Checkpoints::new(
            registry.checkpoint.clone(),
            &context,
            request.limits.clone(),
        );
        let bus = EventBus::new(id.clone(), self.inner.config.event_capacity);
        let events = bus.subscribe();
        let (result_tx, result_rx) = watch::channel(None);
        let (command_tx, command_rx) = mpsc::channel(32);
        let mut execution = Execution {
            context,
            registry: registry.clone(),
            gateway,
            transcript,
            limits: request.limits,
            enable_tools: request.enable_tools,
            allowed_tools: request.allowed_tools,
            checkpoints,
            steps: 0,
            commands: command_rx,
            bus: bus.clone(),
            config: self.inner.config.clone(),
        };
        runtime.spawn(async move {
            let _lease = lease;
            let (observers_done, done_rx) = watch::channel(false);
            let mut observers = registry
                .observers
                .iter()
                .map(|observer| {
                    execution.bus.observe(
                        observer.clone(),
                        execution.limits.hook_timeout,
                        done_rx.clone(),
                    )
                })
                .collect::<Vec<_>>();
            execution.bus.emit(RunEvent::RunStarted);
            let outcome = AssertUnwindSafe(execution.drive())
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(AgentError::new(ErrorCode::Panic, "run driver panicked")));
            execution.context.cancel.cancel();
            execution.commands.close();
            let repaired = repair_unsettled(&mut execution.transcript);
            let outcome = outcome.and_then(|output| repaired.map(|_| output));
            let (mut status, mut output, mut error) = match outcome {
                Ok(output) => (RunStatus::Completed, Some(output), None),
                Err(mut error) => {
                    error.message = clip_utf8(&error.message, 4096).to_owned();
                    (RunStatus::from_error(&error), None, Some(error))
                }
            };
            if let Err(cleanup_error) = execution.finish_transforms().await {
                if let Some(previous) = &mut error {
                    previous.message = clip_utf8(
                        &format!(
                            "{}; resource cleanup failed: {}",
                            previous.message, cleanup_error
                        ),
                        4096,
                    )
                    .to_owned();
                } else {
                    status = RunStatus::Failed;
                    output = None;
                    error = Some(cleanup_error);
                }
            }
            if let Err(commit_error) = execution
                .checkpoints
                .finish(&execution.transcript, status, error.clone())
                .await
            {
                if status == RunStatus::Completed {
                    status = RunStatus::Failed;
                    output = None;
                    error = Some(commit_error);
                }
            }
            let checkpoint = execution.checkpoints.status().await;
            let (task_usage, statistics) = execution.context.task.accounting();
            let report = Arc::new(RunReport {
                run_id: execution.context.run_id.clone(),
                status,
                output,
                error,
                transcript: execution.transcript.take(),
                model_requests: execution.gateway.audits(),
                task_usage,
                statistics,
                steps: execution.steps,
                checkpoint,
            });
            execution.bus.finish(RunOutcome {
                status,
                output: report.output.clone(),
                error: report.error.clone(),
                task_usage: report.task_usage.clone(),
                steps: report.steps,
            });
            let _ = result_tx.send(Some(report));
            let _ = observers_done.send(true);
            // Observers are noncritical, but may not outlive plugin resource teardown.
            for mut observer in observers.drain(..) {
                if timeout(execution.limits.cancellation_grace, &mut observer)
                    .await
                    .is_err()
                {
                    observer.abort();
                    let _ = observer.await;
                }
            }
        });
        Ok(RunHandle::new(
            Arc::new(CoreSession {
                run_id: id,
                bus,
                result: result_rx,
                cancel,
                commands: command_tx,
            }),
            events,
        ))
    }
    pub(crate) fn cancel_all(&self) {
        let mut state = lock(&self.inner.state);
        state.closed = true;
        for cancel in state.runs.values() {
            cancel.cancel();
        }
    }
    pub(crate) async fn close(&self, duration: Duration) -> Result<()> {
        self.cancel_all();
        timeout(duration, async {
            loop {
                let notified = self.inner.drained.notified();
                if lock(&self.inner.state).runs.is_empty() {
                    break;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| {
            AgentError::new(
                ErrorCode::Deadline,
                "host drain timed out; plugins kept installed",
            )
        })?;
        *self
            .inner
            .registry
            .write()
            .unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }
}
#[async_trait]
impl AgentExecutor for Engine {
    async fn execute(&self, request: RunRequest) -> Result<Arc<RunReport>> {
        self.start(request)?.wait_owned().await
    }
}

impl AgentRuntime for Engine {
    fn validate_history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        options.validate()?;
        self.registry()?.services.get::<ModelProvider>(MODEL_SERVICE)?.0.validate_history(messages, options)
    }
    fn start(&self, request: RunRequest) -> Result<RunHandle> {
        Engine::start(self, request)
    }
}

struct Execution {
    context: RunContext,
    registry: Arc<Registry>,
    gateway: Arc<Gateway>,
    transcript: History,
    limits: RunLimits,
    enable_tools: bool,
    allowed_tools: Option<BTreeSet<String>>,
    checkpoints: Checkpoints,
    steps: usize,
    commands: mpsc::Receiver<Command>,
    bus: EventBus,
    config: EngineConfig,
}
impl Execution {
    async fn finish_transforms(&self) -> Result<()> {
        let mut first_error = None;
        for transform in self.registry.results.iter().rev() {
            if let Err(error) = lifecycle(
                self.limits.hook_timeout,
                transform.finish(&self.context.run_id),
            )
            .await
            {
                first_error.get_or_insert(error);
            }
        }
        for transform in self.registry.contexts.iter().rev() {
            if let Err(error) = lifecycle(
                self.limits.hook_timeout,
                transform.finish(&self.context.run_id),
            )
            .await
            {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
    async fn apply_inputs(&mut self) -> Result<bool> {
        let mut commands = vec![];
        for _ in 0..32 {
            let Ok(command) = self.commands.try_recv() else {
                break;
            };
            commands.push(command);
        }
        if commands.is_empty() {
            return Ok(false);
        }
        let additions: Vec<_> = commands.iter().map(|c| Message::user(c.text.clone())).collect();
        let accepted = async {
            // Admission precedes copying and checkpoint publication; rejected input is never ACKed.
            self.transcript.check_append(&additions)?;
            let mut next = self.transcript.to_vec();
            next.extend(additions.iter().cloned());
            validate_messages(&next)?;
            self.checkpoints.input(&next).await?;
            self.transcript.extend(additions)
        }.await;
        for command in commands {
            let _ = command.applied.send(accepted.clone());
        }
        accepted?;
        self.bus.emit(RunEvent::InputApplied);
        Ok(true)
    }
    async fn selected_tools(&self, step: usize, messages: &[Message]) -> Result<Vec<ToolSpec>> {
        if !self.enable_tools {
            return Ok(vec![]);
        }
        let mut available: Vec<_> = self
            .registry
            .tools
            .values()
            .filter(|t| {
                self.allowed_tools
                    .as_ref()
                    .map_or(true, |names| names.contains(&t.spec.name))
            })
            .map(|t| t.spec.clone())
            .collect();
        for selector in &self.registry.selectors {
            let operation = self.context.cancel.child_token();
            let _drop_cancel = CancelOnDrop(operation.clone());
            let mut ctx = self.context.clone();
            ctx.cancel = operation.clone();
            let selected = bounded(
                &self.context.cancel,
                &operation,
                self.context
                    .task
                    .deadline()
                    .min(Instant::now() + self.limits.hook_timeout),
                self.limits.cancellation_grace,
                selector.select(&ctx, step, messages, &available),
            )
            .await?;
            if selected.len() > available.len() {
                return Err(AgentError::new(
                    ErrorCode::Plugin,
                    "tool selector returned too many names",
                ));
            }
            let selected_len = selected.len();
            let names: BTreeSet<_> = selected.into_iter().collect();
            if names.len() != selected_len {
                return Err(AgentError::new(
                    ErrorCode::Plugin,
                    "tool selector returned duplicate names",
                ));
            }
            let previous: BTreeSet<_> = available.iter().map(|t| t.name.clone()).collect();
            if !names.is_subset(&previous) {
                return Err(AgentError::new(
                    ErrorCode::Plugin,
                    "tool selector tried to expand its permitted view",
                ));
            }
            available.retain(|t| names.contains(&t.name));
        }
        Ok(available)
    }
    async fn projection_context(&self, tools: Vec<ToolSpec>) -> Result<RunContext> {
        let mut round = self.context.clone();
        round.request_overhead_bytes = request_overhead(&tools, self.limits.max_output_tokens, &round.model_options)?;
        round.tools_enabled = !tools.is_empty();
        round.allowed_tools = Some(Arc::new(tools.iter().map(|t| t.name.clone()).collect()));
        round.request_tools = Arc::new(tools);
        let mut blocks = Vec::new();
        let mut names = BTreeSet::new();
        let mut bytes = 0usize;
        for transform in &self.registry.contexts {
            let operation = self.context.cancel.child_token();
            let _drop_cancel = CancelOnDrop(operation.clone());
            let mut ctx = round.clone(); ctx.cancel = operation.clone();
            let supplied = bounded(&self.context.cancel, &operation,
                self.context.task.deadline().min(Instant::now() + self.limits.context_timeout),
                self.limits.cancellation_grace, transform.sources(&ctx, &self.transcript)).await?;
            for block in supplied {
                block.validate()?;
                if blocks.len() >= 128 || !names.insert(block.source.clone()) {
                    return Err(AgentError::new(ErrorCode::Plugin, "too many or duplicate context sources"));
                }
                bytes = bytes.saturating_add(serialized_bytes(&block.message())?).saturating_add(1);
                if bytes.saturating_add(round.request_overhead_bytes) >= self.limits.max_context_bytes {
                    return Err(AgentError::new(ErrorCode::Limit, "protected context sources leave no model request budget"));
                }
                blocks.push(block);
            }
        }
        round.request_overhead_bytes = round.request_overhead_bytes.saturating_add(bytes);
        round.context_sources = Arc::new(blocks);
        Ok(round)
    }
    async fn projection(&self, tools: Vec<ToolSpec>) -> Result<(Vec<Message>, RunContext)> {
        let projection_ctx = self.projection_context(tools).await?;
        let mut messages = self.transcript.to_vec();
        for transform in &self.registry.contexts {
            let operation = self.context.cancel.child_token();
            let _drop_cancel = CancelOnDrop(operation.clone());
            let mut ctx = projection_ctx.clone();
            ctx.cancel = operation.clone();
            messages = bounded(
                &self.context.cancel,
                &operation,
                self.context
                    .task
                    .deadline()
                    .min(Instant::now() + self.limits.context_timeout),
                self.limits.cancellation_grace,
                transform.transform(&ctx, messages),
            )
            .await?;
            validate_messages(&messages)?;
            if serialized_bytes(&messages)? > self.limits.max_history_bytes {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "context transform exceeded working history byte limit",
                ));
            }
        }
        if serialized_bytes(&messages)?.saturating_add(projection_ctx.request_overhead_bytes) > self.limits.max_context_bytes {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "final context projection exceeded request byte limit",
            ));
        }
        Ok((projection_ctx.with_sources(messages), projection_ctx))
    }
    async fn recover_projection(&self, failed: Vec<Message>, projection_ctx: &RunContext) -> Result<Option<Vec<Message>>> {
        let before = serialized_bytes(&failed)?;
        let mut messages = self.transcript.to_vec();
        let mut changed = false;
        for transform in &self.registry.contexts {
            let operation = self.context.cancel.child_token();
            let _drop_cancel = CancelOnDrop(operation.clone());
            let mut ctx = projection_ctx.clone();
            ctx.cancel = operation.clone();
            let recovered = bounded(
                &self.context.cancel,
                &operation,
                self.context
                    .task
                    .deadline()
                    .min(Instant::now() + self.limits.context_timeout),
                self.limits.cancellation_grace,
                transform.recover_context(&ctx, messages.clone()),
            )
            .await?;
            messages = if let Some(next) = recovered {
                changed = true;
                next
            } else {
                bounded(
                    &self.context.cancel,
                    &operation,
                    self.context
                        .task
                        .deadline()
                        .min(Instant::now() + self.limits.context_timeout),
                    self.limits.cancellation_grace,
                    transform.transform(&ctx, messages),
                )
                .await?
            };
            validate_messages(&messages)?;
            if serialized_bytes(&messages)? > self.limits.max_history_bytes {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "context recovery exceeded working history byte limit",
                ));
            }
        }
        if !changed {
            return Ok(None);
        }
        let request_bytes = serialized_bytes(&messages)?.saturating_add(projection_ctx.request_overhead_bytes);
        let messages = projection_ctx.with_sources(messages);
        let after = serialized_bytes(&messages)?;
        if after >= before {
            return Err(AgentError::new(
                ErrorCode::ModelProtocol,
                "context recovery did not shrink the failed projection",
            ));
        }
        if request_bytes > self.limits.max_context_bytes {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "context recovery still exceeds the request byte limit",
            ));
        }
        Ok(Some(messages))
    }
    async fn drive(&mut self) -> Result<String> {
        for step in 1..=self.limits.max_steps {
            self.context.task.check()?;
            if self.context.cancel.is_cancelled() {
                return Err(AgentError::new(ErrorCode::Cancelled, "run cancelled"));
            }
            self.apply_inputs().await?;
            self.transcript.check_model_room()?;
            self.steps = step;
            self.bus.emit(RunEvent::StepStarted { step });
            // Select once from settled canonical history, then collect sources and compact.
            // This same view survives retries/recovery and reaches dispatch unchanged.
            let tools = self.selected_tools(step, &self.transcript).await?;
            let (messages, projection_ctx) = self.projection(tools.clone()).await?;
            self.checkpoints
                .before_model(step, &self.transcript, &tools)
                .await?;
            let offered: BTreeSet<_> = tools.iter().map(|t| t.name.clone()).collect();
            let sink: Arc<dyn ModelSink> = Arc::new(TextSink::new(self.bus.clone(), step));
            let request = ModelRequest {
                messages: messages.clone(),
                tools: tools.clone(),
                max_output_tokens: self.limits.max_output_tokens,
                options: self.context.model_options.clone(),
            };
            let reply = match self.gateway.complete_primary(request, Some(sink.clone())).await {
                Err(error) if error.code == ErrorCode::ModelContextWindow && !error.model_output_started => {
                    let Some(recovered) = self.recover_projection(messages, &projection_ctx).await? else {
                        return Err(error);
                    };
                    self.checkpoints
                        .before_model(step, &self.transcript, &tools)
                        .await?;
                    self.gateway
                        .complete_primary(
                            ModelRequest {
                                messages: recovered,
                                tools,
                                max_output_tokens: self.limits.max_output_tokens,
                                options: self.context.model_options.clone(),
                            },
                            Some(sink),
                        )
                        .await?
                }
                other => other?,
            };
            if !self.enable_tools && !reply.tool_calls.is_empty() {
                return Err(AgentError::new(
                    ErrorCode::ModelProtocol,
                    "model called a tool in a tools-disabled run",
                ));
            }
            let seen: BTreeSet<_> = self
                .transcript
                .iter()
                .flat_map(|message| match message {
                    Message::Assistant { tool_calls, .. } => {
                        tool_calls.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
                    }
                    _ => vec![],
                })
                .collect();
            if reply.tool_calls.iter().any(|call| seen.contains(&call.id)) {
                return Err(AgentError::new(
                    ErrorCode::ModelProtocol,
                    "model reused a tool-call id; not replayed",
                ));
            }
            let calls = reply.tool_calls.clone();
            let answer = reply.content.clone();
            let result_budget = self.transcript.admit_reply(reply.into_message())?;
            if let Err(error) = self.checkpoints.after_model(&self.transcript, &calls).await {
                self.transcript
                    .extend(calls.iter().map(|call| Message::Tool {
                        result: ToolResult::new(
                            &call.id,
                            ToolStatus::Skipped,
                            "checkpoint failed before dispatch",
                        ),
                    }))?;
                return Err(error);
            }
            let still_running = self.context.task.check().and_then(|_| {
                if self.context.cancel.is_cancelled() {
                    Err(AgentError::new(
                        ErrorCode::Cancelled,
                        "run cancelled before dispatch",
                    ))
                } else {
                    Ok(())
                }
            });
            if let Err(error) = still_running {
                self.transcript
                    .extend(calls.iter().map(|call| Message::Tool {
                        result: ToolResult::new(
                            &call.id,
                            ToolStatus::Skipped,
                            "stopped before dispatch",
                        ),
                    }))?;
                return Err(error);
            }
            self.bus.complete_message(step, &answer);
            for (index, call) in calls.iter().enumerate() {
                self.bus.tool_call(step, index, call);
            }
            if calls.is_empty() {
                self.bus.emit(RunEvent::StepFinished { step });
                if self.apply_inputs().await? {
                    continue;
                }
                return Ok(answer);
            }
            let (results, error) = execute_batch(
                projection_ctx,
                self.registry.clone(),
                self.limits.clone(),
                &self.config.allowed_side_effect_tools,
                &offered,
                &calls,
                self.bus.clone(),
                step,
                self.checkpoints.clone(),
                result_budget,
            )
            .await;
            self.transcript
                .extend(results.into_iter().map(|result| Message::Tool { result }))?;
            validate_messages(&self.transcript)?;
            self.bus.emit(RunEvent::StepFinished { step });
            let committed = self.checkpoints.after_tools(&self.transcript).await;
            if let Some(error) = error {
                return Err(error);
            }
            committed?;
        }
        Err(AgentError::new(ErrorCode::Limit, "run step limit reached"))
    }
}

fn request_overhead(tools: &[ToolSpec], max_output_tokens: u32, options: &ModelOptions) -> Result<usize> {
    let empty = ModelRequest { messages: vec![], tools: tools.to_vec(),
        max_output_tokens, options: options.clone() };
    // The real nonempty message array supplies its own brackets and separators.
    Ok(serialized_bytes(&empty)?.saturating_sub(2))
}

fn repair_unsettled(messages: &mut History) -> Result<()> {
    let mut calls = Vec::new();
    let mut settled = BTreeSet::new();
    for message in messages.iter() {
        match message {
            Message::Assistant { tool_calls, .. } => calls.extend(tool_calls.iter().cloned()),
            Message::Tool { result } => {
                settled.insert(result.call_id.clone());
            }
            _ => {}
        }
    }
    for call in calls {
        if !settled.contains(&call.id) {
            messages.extend([Message::Tool { result: ToolResult::new(call.id, ToolStatus::Unknown,
                UNKNOWN_RESULT) }])?;
        }
    }
    Ok(())
}

fn run_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!(
        "run-{}-{ms}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
