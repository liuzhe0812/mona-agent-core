use crate::{protocol::*, subscription::Subscription};
use agent_api::{AgentRuntime, Message, ModelOptions, RunEvent, RunLimits, RunRequest, RunSession, RunSnapshot, TaskControl, TaskLimits};
use futures_util::FutureExt;
use std::{collections::{BTreeSet, BTreeMap, HashMap, VecDeque}, panic::AssertUnwindSafe,
    sync::{Arc, Mutex, MutexGuard}, time::Duration};
use tokio::{sync::{broadcast, watch, Semaphore}, time::Instant};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> { mutex.lock().unwrap_or_else(|p| p.into_inner()) }

#[derive(Clone)]
pub struct ApplicationConfig {
    pub max_retained_runs: usize,
    pub completed_ttl: Duration,
    pub max_prompt_bytes: usize,
    pub journal_events: usize,
    pub journal_bytes: usize,
    pub max_subscriptions: usize,
    pub max_input_operations: usize,
    /// Trusted configuration only; bridges never accept these values from user JSON.
    pub system_prompt: Option<String>,
    pub allowed_tools: Option<BTreeSet<String>>,
    pub model_options: ModelOptions,
    pub run_limits: RunLimits,
    pub task_limits: TaskLimits,
}
impl Default for ApplicationConfig {
    fn default() -> Self {
        Self { max_retained_runs: 32, completed_ttl: Duration::from_secs(600),
            max_prompt_bytes: 64 * 1024, journal_events: 512, journal_bytes: 2 * 1024 * 1024,
            max_subscriptions: 64, max_input_operations: 128, system_prompt: None, allowed_tools: None, model_options: ModelOptions::default(),
            run_limits: RunLimits::default(), task_limits: TaskLimits::default() }
    }
}
impl ApplicationConfig {
    fn validate(&self) -> ApplicationResult<()> {
        self.run_limits.validate().map_err(ApplicationError::from)?;
        self.model_options.validate().map_err(ApplicationError::from)?;
        if self.max_retained_runs == 0 || self.completed_ttl.is_zero() || self.max_prompt_bytes == 0
            || self.journal_events == 0 || self.journal_bytes == 0 || self.max_subscriptions == 0
            || self.max_input_operations == 0 || self.task_limits.wall_time.is_zero()
            || self.task_limits.max_model_calls == 0 {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "application limits must be positive"));
        }
        Ok(())
    }
}
struct InputOperation { text: String, result: watch::Receiver<Option<ApplicationResult<InputReceipt>>> }
pub(crate) struct RunData {
    pub snapshot: RunSnapshot,
    pub journal: VecDeque<(agent_api::EventEnvelope, usize)>,
    pub journal_bytes: usize,
    pub reset_floor: u64,
    pub fault: Option<ApplicationError>,
    completed_at: Option<Instant>,
    inputs: HashMap<String, InputOperation>,
}
pub(crate) struct Entry {
    pub session: Arc<dyn RunSession>,
    pub data: Mutex<RunData>,
    pub changed: watch::Sender<u64>,
    request: StartRequest,
}
impl Entry {
    fn notify(&self) { self.changed.send_modify(|revision| *revision = revision.wrapping_add(1)); }
    fn terminal(data: &RunData) -> bool { data.snapshot.outcome.is_some() || data.fault.is_some() }
    fn accept(&self, event: agent_api::EventEnvelope, config: &ApplicationConfig) {
        let mut data = lock(&self.data);
        if event.seq <= data.snapshot.seq { return; }
        let event = public_event(event);
        if !data.snapshot.apply(&event) {
            drop(data); self.resync(); return;
        }
        let bytes = serde_json::to_vec(&event).map_or(usize::MAX, |v| v.len());
        if bytes <= config.journal_bytes {
            data.journal_bytes = data.journal_bytes.saturating_add(bytes);
            data.journal.push_back((event, bytes));
        } else {
            // Never silently skip a too-large event: force a snapshot for older cursors.
            data.journal.clear(); data.journal_bytes = 0; data.reset_floor = data.snapshot.seq;
        }
        while data.journal.len() > config.journal_events || data.journal_bytes > config.journal_bytes {
            if let Some((_, bytes)) = data.journal.pop_front() { data.journal_bytes = data.journal_bytes.saturating_sub(bytes); }
            else { break; }
        }
        if Self::terminal(&data) && data.completed_at.is_none() { data.completed_at = Some(Instant::now()); }
        drop(data); self.notify();
    }
    fn resync(&self) {
        // Core snapshot and seq are atomic. Buffered events <= seq are discarded later.
        let snapshot = public_snapshot(self.session.snapshot());
        let mut data = lock(&self.data);
        if snapshot.seq < data.snapshot.seq { return; }
        data.reset_floor = snapshot.seq;
        data.snapshot = snapshot;
        data.journal.clear(); data.journal_bytes = 0;
        if Self::terminal(&data) && data.completed_at.is_none() { data.completed_at = Some(Instant::now()); }
        drop(data); self.notify();
    }
    fn fault(&self) {
        let mut data = lock(&self.data);
        if data.snapshot.outcome.is_none() {
            data.fault = Some(ApplicationError::new(ApplicationErrorCode::Internal, "executor ended without a terminal snapshot"));
            data.completed_at = Some(Instant::now());
        }
        drop(data); self.notify();
    }
}
struct State { closed: bool, runs: HashMap<String, Arc<Entry>>, requests: HashMap<String, String> }
struct Inner {
    runtime: Arc<dyn AgentRuntime>, config: ApplicationConfig, state: Mutex<State>, subscriptions: Arc<Semaphore>,
}
impl Drop for Inner {
    fn drop(&mut self) { for entry in lock(&self.state).runs.values() { entry.session.cancel(); } }
}
#[derive(Clone)]
pub struct AgentApplication { inner: Arc<Inner> }
impl AgentApplication {
    pub fn new(runtime: Arc<dyn AgentRuntime>, config: ApplicationConfig) -> ApplicationResult<Self> {
        config.validate()?;
        Ok(Self { inner: Arc::new(Inner { runtime, subscriptions: Arc::new(Semaphore::new(config.max_subscriptions)),
            config, state: Mutex::new(State { closed: false, runs: HashMap::new(), requests: HashMap::new() }) }) })
    }
    fn prune(&self, state: &mut State) {
        let now = Instant::now();
        let expired = state.runs.iter().filter_map(|(id, entry)| {
            let data = lock(&entry.data);
            data.completed_at.filter(|at| now.saturating_duration_since(*at) >= self.inner.config.completed_ttl).map(|_| id.clone())
        }).collect::<Vec<_>>();
        for id in expired {
            if let Some(entry) = state.runs.remove(&id) { state.requests.remove(&entry.request.request_id); }
        }
    }
    fn entry(&self, id: &str) -> ApplicationResult<Arc<Entry>> {
        let mut state = lock(&self.inner.state); self.prune(&mut state);
        state.runs.get(id).cloned().ok_or_else(|| ApplicationError::new(ApplicationErrorCode::NotFound, "run is absent or expired"))
    }
    pub fn start_task(&self, request: StartRequest) -> ApplicationResult<StartResponse> {
        validate_key(&request.request_id)?;
        if request.prompt.is_empty() || request.prompt.len() > self.inner.config.max_prompt_bytes {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "prompt is empty or too large"));
        }
        let runtime_handle = tokio::runtime::Handle::try_current().map_err(|_| ApplicationError::new(ApplicationErrorCode::Internal, "a Tokio runtime is required"))?;
        let mut state = lock(&self.inner.state); self.prune(&mut state);
        if state.closed { return Err(ApplicationError::new(ApplicationErrorCode::Closed, "application is shutting down")); }
        if let Some(id) = state.requests.get(&request.request_id) {
            let entry = &state.runs[id];
            if entry.request.prompt != request.prompt { return Err(ApplicationError::new(ApplicationErrorCode::Conflict, "request_id was used with a different prompt")); }
            return Ok(StartResponse { run_id: id.clone(), reused: true });
        }
        if state.runs.len() >= self.inner.config.max_retained_runs {
            return Err(ApplicationError::new(ApplicationErrorCode::Capacity, "run registry is full; forget a completed run or wait for TTL"));
        }
        let mut messages = vec![];
        if let Some(system) = &self.inner.config.system_prompt { messages.push(Message::system(system)); }
        messages.push(Message::user(&request.prompt));
        let run = RunRequest { messages, limits: self.inner.config.run_limits.clone(),
            task: TaskControl::new(self.inner.config.task_limits.clone()), metadata: BTreeMap::new(), enable_tools: true,
            allowed_tools: self.inner.config.allowed_tools.clone(), model_options: self.inner.config.model_options.clone() };
        let handle = self.inner.runtime.start(run).map_err(ApplicationError::from)?;
        let (session, events) = handle.into_parts();
        let id = session.run_id().to_owned();
        if state.runs.contains_key(&id) {
            session.cancel(); return Err(ApplicationError::new(ApplicationErrorCode::Internal, "runtime reused a run id"));
        }
        let (changed, _) = watch::channel(0);
        // Start with seq=0 and the pre-created receiver, NOT a racy subscribe-after-start.
        let entry = Arc::new(Entry { data: Mutex::new(RunData { snapshot: RunSnapshot::new(&id),
            journal: VecDeque::new(), journal_bytes: 0, reset_floor: 0, fault: None, completed_at: None, inputs: HashMap::new() }),
            session, changed, request: request.clone() });
        state.requests.insert(request.request_id, id.clone()); state.runs.insert(id.clone(), entry.clone());
        let config = self.inner.config.clone();
        runtime_handle.spawn(collect(entry, events, config));
        Ok(StartResponse { run_id: id, reused: false })
    }
    pub fn cancel_task(&self, id: &str) -> ApplicationResult<CancelReceipt> {
        let entry = self.entry(id)?;
        let signalled = !Entry::terminal(&lock(&entry.data));
        if signalled { entry.session.cancel(); }
        Ok(CancelReceipt { run_id: id.to_owned(), signalled })
    }
    pub fn get_snapshot(&self, id: &str) -> ApplicationResult<RunSnapshot> {
        let entry = self.entry(id)?;
        let snapshot = lock(&entry.data).snapshot.clone();
        Ok(snapshot)
    }
    pub fn get_result(&self, id: &str) -> ApplicationResult<ResultResponse> {
        let entry = self.entry(id)?; let data = lock(&entry.data);
        if let Some(error) = &data.fault { return Err(error.clone()); }
        Ok(ResultResponse { run_id: id.to_owned(), outcome: data.snapshot.outcome.clone() })
    }
    pub fn subscribe_events(&self, id: &str, after: Option<u64>) -> ApplicationResult<Subscription> {
        let entry = self.entry(id)?;
        let permit = self.inner.subscriptions.clone().try_acquire_owned()
            .map_err(|_| ApplicationError::new(ApplicationErrorCode::Capacity, "too many stream subscribers"))?;
        Subscription::new(entry, after, permit)
    }
    pub async fn send_input(&self, id: &str, request: InputRequest) -> ApplicationResult<InputReceipt> {
        validate_key(&request.request_id)?;
        if request.text.is_empty() || request.text.len() > 16 * 1024 {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "input must be 1..16384 bytes"));
        }
        let entry = self.entry(id)?;
        let mut receiver = {
            let mut data = lock(&entry.data);
            if let Some(operation) = data.inputs.get(&request.request_id) {
                if operation.text != request.text { return Err(ApplicationError::new(ApplicationErrorCode::Conflict, "input request_id was reused with different text")); }
                operation.result.clone()
            } else {
                if Entry::terminal(&data) { return Err(ApplicationError::new(ApplicationErrorCode::Closed, "run is finished")); }
                if data.inputs.len() >= self.inner.config.max_input_operations {
                    return Err(ApplicationError::new(ApplicationErrorCode::Capacity, "input operation retention limit reached"));
                }
                let (sender, receiver) = watch::channel(None);
                data.inputs.insert(request.request_id.clone(), InputOperation { text: request.text.clone(), result: receiver.clone() });
                let session = entry.session.clone();
                tokio::spawn(async move {
                    let result = AssertUnwindSafe(session.steer(request.text)).catch_unwind().await
                        .map_err(|_| ApplicationError::new(ApplicationErrorCode::Internal, "input handler panicked"))
                        .and_then(|r| r.map_err(ApplicationError::from))
                        .map(|_| InputReceipt { request_id: request.request_id, applied: true });
                    let _ = sender.send(Some(result));
                });
                receiver
            }
        };
        // Disconnecting a bridge request does NOT undo an already accepted input operation.
        loop {
            let result = receiver.borrow().clone();
            if let Some(result) = result { return result; }
            receiver.changed().await.map_err(|_| ApplicationError::new(ApplicationErrorCode::Internal, "input operation lost its result"))?;
        }
    }
    pub fn forget(&self, id: &str) -> ApplicationResult<()> {
        let mut state = lock(&self.inner.state);
        let entry = state.runs.get(id).ok_or_else(|| ApplicationError::new(ApplicationErrorCode::NotFound, "run not found"))?;
        if !Entry::terminal(&lock(&entry.data)) { return Err(ApplicationError::new(ApplicationErrorCode::Conflict, "cannot forget an active run")); }
        let key = entry.request.request_id.clone(); state.runs.remove(id); state.requests.remove(&key);
        Ok(())
    }
    /// Host lifecycle, not a remotely exposed bridge endpoint. Shut the plugin Host down afterwards.
    pub async fn shutdown(&self, grace: Duration) -> ApplicationResult<()> {
        let sessions = {
            let mut state = lock(&self.inner.state); state.closed = true;
            state.runs.values().map(|e| e.session.clone()).collect::<Vec<_>>()
        };
        for session in &sessions { session.cancel(); }
        tokio::time::timeout(grace, async {
            for session in sessions { session.wait().await.map_err(ApplicationError::from)?; }
            Ok::<(), ApplicationError>(())
        }).await.map_err(|_| ApplicationError::new(ApplicationErrorCode::Closed, "application shutdown timed out"))?
    }
}
async fn collect(entry: Arc<Entry>, mut events: broadcast::Receiver<agent_api::EventEnvelope>, config: ApplicationConfig) {
    let session = entry.session.clone();
    let completion = session.wait(); tokio::pin!(completion);
    loop {
        tokio::select! {
            biased;
            event = events.recv() => match event {
                Ok(event) => {
                    let done = matches!(&event.event, RunEvent::RunFinished { .. });
                    entry.accept(event, &config);
                    if done { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    entry.resync();
                    if Entry::terminal(&lock(&entry.data)) { break; }
                }
                Err(broadcast::error::RecvError::Closed) => { entry.resync(); entry.fault(); break; }
            },
            _ = &mut completion => { entry.resync(); entry.fault(); break; }
        }
    }
}
