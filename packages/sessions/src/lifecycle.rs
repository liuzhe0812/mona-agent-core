//! Awaited checkpoint adapter and run finalization. No model/tool execution loop.
use crate::{disk, Store, SESSION_KEY, TURN_KEY};
use api::*;
use std::sync::Arc;
use tokio::sync::watch;

pub struct SessionSink(pub Arc<Store>);
#[async_trait]
impl CheckpointSink for SessionSink {
    async fn commit(
        &self,
        checkpoint: Arc<RunCheckpoint>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let store = self.0.clone();
        disk(move || store.commit(&checkpoint, &cancel))
            .await
            .map_err(|_| {
                AgentError::new(
                    ErrorCode::Checkpoint,
                    "session checkpoint was not acknowledged",
                )
            })
    }
}
struct SessionRuntime {
    inner: Arc<dyn AgentRuntime>,
    store: Arc<Store>,
}
/// Wrap the same Runtime that was assembled with SessionSink for this Store.
/// prepare() must durably admit a turn before starting its bound Run.
pub fn runtime(inner: Arc<dyn AgentRuntime>, store: Arc<Store>) -> Arc<dyn AgentRuntime> {
    Arc::new(SessionRuntime { inner, store })
}
struct SavedRun {
    inner: Arc<dyn RunSession>,
    saved: watch::Receiver<Option<Result<Arc<RunReport>>>>,
}
#[async_trait]
impl RunSession for SavedRun {
    fn run_id(&self) -> &str {
        self.inner.run_id()
    }
    fn cancel(&self) {
        self.inner.cancel();
    }
    fn snapshot(&self) -> RunSnapshot {
        self.inner.snapshot()
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventEnvelope> {
        self.inner.subscribe()
    }
    async fn steer(&self, text: String) -> Result<()> {
        self.inner.steer(text).await
    }
    async fn wait(&self) -> Result<Arc<RunReport>> {
        let mut saved = self.saved.clone();
        loop {
            if let Some(result) = saved.borrow().clone() {
                return result;
            }
            saved.changed().await.map_err(|_| {
                AgentError::new(ErrorCode::Checkpoint, "session finalization unavailable")
            })?;
        }
    }
}
#[async_trait]
impl AgentExecutor for SessionRuntime {
    async fn execute(&self, request: RunRequest) -> Result<Arc<RunReport>> {
        self.start(request)?.wait_owned().await
    }
}
impl AgentRuntime for SessionRuntime {
    fn start(&self, request: RunRequest) -> Result<RunHandle> {
        let binding = match (
            request.metadata.get(SESSION_KEY),
            request.metadata.get(TURN_KEY),
        ) {
            (Some(id), Some(key)) => Some((id.clone(), key.clone())),
            (None, None) => None,
            _ => {
                return Err(AgentError::new(
                    ErrorCode::Configuration,
                    "session and turn identity must be supplied together",
                ))
            }
        };
        let handle = self.inner.start(request)?;
        let Some((id, key)) = binding else {
            return Ok(handle);
        };
        let (inner, events) = handle.into_parts();
        let waiting = inner.clone();
        let store = self.store.clone();
        let (tx, rx) = watch::channel(None);
        // Detached observers do not own this settlement. wait_owned still propagates cancellation.
        tokio::spawn(async move {
            let mut result = waiting.wait().await;
            if let Ok(report) = &result {
                let report = report.clone();
                if disk(move || store.finish(&id, &key, &report))
                    .await
                    .is_err()
                {
                    result = Err(AgentError::new(ErrorCode::Checkpoint,
                        "session finalization failed; last acknowledged checkpoint remains authoritative"));
                }
            }
            tx.send_replace(Some(result));
        });
        Ok(RunHandle::new(
            Arc::new(SavedRun { inner, saved: rx }),
            events,
        ))
    }
}
