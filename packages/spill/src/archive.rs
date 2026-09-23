//! Shared writer lifecycle for result transforms and streaming tool captures.
use crate::{error, SpillConfig, SpillRecord, SpillStore};
use api::{ErrorCode, Result, RunContext};
use std::{collections::BTreeSet, sync::Arc};
use tokio::{io::AsyncRead, sync::Mutex};

pub struct SpillArchive {
    pub(crate) store: Arc<dyn SpillStore>,
    pub(crate) config: SpillConfig,
    active: Mutex<BTreeSet<String>>,
}
impl SpillArchive {
    pub fn new(store: Arc<dyn SpillStore>, config: SpillConfig) -> Self {
        Self { store, config, active: Mutex::new(BTreeSet::new()) }
    }
    pub fn config(&self) -> &SpillConfig { &self.config }
    /// Source must contain exactly `bytes` UTF-8 bytes. No complete body is buffered.
    /// The caller owns its deadline/cancellation; no locator is returned before commit.
    pub async fn put_stream(&self, ctx: &RunContext, call_id: &str,
        source: &mut (dyn AsyncRead + Unpin + Send), bytes: usize) -> Result<SpillRecord> {
        self.config.validate()?;
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() { return Err(error(ErrorCode::Cancelled, "spill cancelled")); }
        let mut active = self.active.lock().await;
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() { return Err(error(ErrorCode::Cancelled, "spill cancelled")); }
        active.insert(ctx.run_id.clone());
        // Admission and cleanup share the same protection through publication.
        self.store.cleanup(&active).await?;
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() { return Err(error(ErrorCode::Cancelled, "spill cancelled before writing")); }
        let task_cancel = ctx.task.cancellation();
        let record = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "spill cancelled")),
            _ = task_cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "spill task cancelled")),
            _ = tokio::time::sleep_until(ctx.task.deadline()) => return Err(error(ErrorCode::Deadline, "spill deadline reached")),
            record = self.store.put_stream(&ctx.run_id, call_id, source, bytes) => record?,
        };
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() { return Err(error(ErrorCode::Cancelled, "spill cancelled before publication")); }
        Ok(record)
    }
    pub async fn put(&self, ctx: &RunContext, call_id: &str, text: &str) -> Result<SpillRecord> {
        self.put_stream(ctx, call_id, &mut text.as_bytes(), text.len()).await
    }
    /// Call after this Run's tools have stopped. The plugin invokes this through its transform.
    pub async fn finish(&self, run_id: &str) -> Result<()> {
        let mut active = self.active.lock().await;
        active.remove(run_id);
        self.store.cleanup(&active).await.map(|_| ())
    }
}
