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
        Self {
            run_id: session.run_id().to_owned(),
            session,
            events,
        }
    }
    pub fn cancel(&self) {
        self.session.cancel();
    }
    pub fn snapshot(&self) -> RunSnapshot {
        self.session.snapshot()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.session.subscribe()
    }
    pub fn session(&self) -> Arc<dyn RunSession> {
        self.session.clone()
    }
    pub fn into_parts(self) -> (Arc<dyn RunSession>, broadcast::Receiver<EventEnvelope>) {
        (self.session, self.events)
    }
    /// Own this execution until it settles. Dropping this future cancels only its Run.
    /// Use this from AgentExecutor wrappers; passive observers should use wait().
    pub fn wait_owned(self) -> impl std::future::Future<Output = Result<Arc<RunReport>>> + Send {
        struct CancelOnDrop(Option<Arc<dyn RunSession>>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                if let Some(session) = &self.0 {
                    session.cancel();
                }
            }
        }
        let guard = CancelOnDrop(Some(self.session.clone()));
        async move {
            let mut guard = guard;
            let result = self.session.wait().await;
            if result.is_ok() {
                guard.0.take();
            }
            drop(guard);
            result
        }
    }
    /// Observe completion without owning cancellation; disconnecting observers is harmless.
    pub async fn wait(&self) -> Result<Arc<RunReport>> {
        self.session.wait().await
    }
    pub async fn steer(&self, text: impl Into<String>) -> Result<()> {
        self.session.steer(text.into()).await
    }
}

/// The default Engine and alternative runtimes share this contract.
/// A final-result-only Planner remains an AgentExecutor until it defines composite stream semantics.
pub trait AgentRuntime: AgentExecutor {
    fn start(&self, request: RunRequest) -> Result<RunHandle>;
}
