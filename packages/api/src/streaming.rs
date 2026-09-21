use crate::{AgentExecutor, EventEnvelope, Result, RunReport, RunRequest, RunSnapshot};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Engine-independent control plane. Bridges must never import an engine implementation.
#[async_trait]
pub trait RunSession: Send + Sync {
    fn run_id(&self) -> &str;
    fn cancel(&self);
    fn snapshot(&self) -> RunSnapshot;
    /// Future events only. On Lagged fetch snapshot(), then discard seq <= snapshot.seq.
    fn subscribe(&self) -> broadcast::Receiver<EventEnvelope>;
    async fn wait(&self) -> Result<Arc<RunReport>>;
    async fn steer(&self, text: String) -> Result<()>;
}

/// A receiver created BEFORE execution starts; avoids the first-event subscription race.
/// Dropping a handle does not cancel its run. Use cancel()/application shutdown explicitly.
pub struct RunHandle {
    pub run_id: String,
    pub events: broadcast::Receiver<EventEnvelope>,
    session: Arc<dyn RunSession>,
}
impl RunHandle {
    pub fn new(session: Arc<dyn RunSession>, events: broadcast::Receiver<EventEnvelope>) -> Self {
        Self { run_id: session.run_id().to_owned(), session, events }
    }
    pub fn cancel(&self) { self.session.cancel(); }
    pub fn snapshot(&self) -> RunSnapshot { self.session.snapshot() }
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> { self.session.subscribe() }
    pub fn session(&self) -> Arc<dyn RunSession> { self.session.clone() }
    pub fn into_parts(self) -> (Arc<dyn RunSession>, broadcast::Receiver<EventEnvelope>) { (self.session, self.events) }
    pub async fn wait(&self) -> Result<Arc<RunReport>> { self.session.wait().await }
    pub async fn steer(&self, text: impl Into<String>) -> Result<()> { self.session.steer(text.into()).await }
}

/// The default Engine and alternative runtimes share this contract.
/// A final-result-only Planner remains an AgentExecutor until it defines composite stream semantics.
pub trait AgentRuntime: AgentExecutor {
    fn start(&self, request: RunRequest) -> Result<RunHandle>;
}
