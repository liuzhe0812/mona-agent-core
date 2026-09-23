//! Host-injected output retention. Tools do not own an indexed archive or a second URI scheme.
use api::{async_trait, ArtifactRef, Result, ToolContext};
use tokio::io::AsyncRead;

#[async_trait]
pub trait OutputArchive: Send + Sync {
    fn trigger_bytes(&self) -> usize;
    fn preview_bytes(&self) -> usize;
    /// Import exactly `bytes` UTF-8 bytes from a completed bounded capture.
    /// Return a locator only after durable publication; honor the caller's cancellation.
    async fn store(&self, ctx: &ToolContext, source: &mut (dyn AsyncRead + Unpin + Send), bytes: usize)
        -> Result<ArtifactRef>;
}
