//! Bounded, run-scoped archiving for large plain-text tool results.
//!
//! Spill deliberately owns only the archive and retrieval capability. The runtime
//! still owns result limits, permissions, cancellation and execution identity.
#![forbid(unsafe_code)]

mod archive;
mod local;
mod tool;

use api::{
    AgentError, ArtifactRef, Content, ErrorCode, Result, ResultTransform, RunContext, ToolCall,
    ToolResult, ToolStatus,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::io::AsyncRead;

pub use archive::SpillArchive;
pub use local::LocalSpillStore;
pub use tool::SpillReadTool;

/// Typed service key published by [`SpillPlugin`].
pub const SPILL_SERVICE: &str = "spill.store";
/// URI scheme used in model-visible artifact references.
pub const SPILL_URI_SCHEME: &str = "spill:";

pub const DEFAULT_TRIGGER_BYTES: usize = 32 * 1024;
pub const DEFAULT_PREVIEW_BYTES: usize = 4 * 1024;
pub const DEFAULT_MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_MAX_RUN_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_PAGE_BYTES: usize = 16 * 1024;
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const SPILL_READ_MARKER: &str = "\n[spill: content omitted; use spill_read on artifact]\n";
const CORE_READ_MARKER: &str =
    "\n[spill: content omitted; use read with the spill: artifact path]\n";

/// Limits are intentionally finite. A trusted host may choose stricter values.
#[derive(Clone, Debug)]
pub struct SpillConfig {
    pub trigger_bytes: usize,
    pub preview_bytes: usize,
    pub max_entry_bytes: usize,
    pub max_run_bytes: usize,
    pub max_total_bytes: usize,
    pub max_page_bytes: usize,
    pub retention: Duration,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            trigger_bytes: DEFAULT_TRIGGER_BYTES,
            preview_bytes: DEFAULT_PREVIEW_BYTES,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_run_bytes: DEFAULT_MAX_RUN_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_page_bytes: DEFAULT_MAX_PAGE_BYTES,
            retention: DEFAULT_RETENTION,
        }
    }
}

impl SpillConfig {
    pub fn validate(&self) -> Result<()> {
        if self.trigger_bytes == 0
            || self.preview_bytes == 0
            || self.max_entry_bytes == 0
            || self.max_run_bytes == 0
            || self.max_total_bytes == 0
            || self.max_page_bytes == 0
            || self.retention.is_zero()
        {
            return Err(error(
                ErrorCode::Configuration,
                "spill limits must be positive",
            ));
        }
        if self.trigger_bytes > self.max_entry_bytes
            || self.preview_bytes > self.max_entry_bytes
            || self.max_entry_bytes > self.max_run_bytes
            || self.max_run_bytes > self.max_total_bytes
        {
            return Err(error(
                ErrorCode::Configuration,
                "spill limits must be ordered entry <= run <= total",
            ));
        }
        if self.max_page_bytes > self.max_entry_bytes {
            return Err(error(
                ErrorCode::Configuration,
                "spill page limit must not exceed entry limit",
            ));
        }
        Ok(())
    }
}

/// A committed archive entry. The ID is opaque and must only be used with the
/// read tool; it never contains a filesystem path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpillRecord {
    pub id: String,
    pub bytes: u64,
}

/// One bounded page from an archive entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpillPage {
    pub id: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub eof: bool,
    pub text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    pub removed_entries: usize,
    pub removed_bytes: usize,
}

/// Storage seam for alternate backends. Implementations must make `put`
/// visible only after the complete UTF-8 payload is committed.
#[async_trait]
pub trait SpillStore: Send + Sync {
    async fn put(&self, run_id: &str, call_id: &str, text: &str) -> Result<SpillRecord> {
        self.put_stream(run_id, call_id, &mut text.as_bytes(), text.len()).await
    }
    /// One commit path for strings and bounded UTF-8 streams. Reject wrong sizes/encoding.
    async fn put_stream(&self, run_id: &str, call_id: &str,
        source: &mut (dyn AsyncRead + Unpin + Send), bytes: usize) -> Result<SpillRecord>;
    async fn read_page(
        &self,
        run_id: &str,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<SpillPage>;
    async fn cleanup(&self, active_runs: &BTreeSet<String>) -> Result<CleanupReport>;
}

/// Thin plugin adapter. The transform and tool use the same store instance.
pub struct SpillPlugin {
    archive: Arc<SpillArchive>,
    reader_tool: &'static str,
    register_reader_tool: bool,
}

impl SpillPlugin {
    pub fn new(store: Arc<dyn SpillStore>, config: SpillConfig) -> Self {
        Self {
            archive: Arc::new(SpillArchive::new(store, config)),
            reader_tool: "spill_read",
            register_reader_tool: true,
        }
    }

    /// Formal hosts can route `spill:` locators through their fixed `read` tool
    /// while retaining the same store, ownership checks and result transform.
    pub fn through_core_read(mut self) -> Self {
        self.reader_tool = "read";
        self.register_reader_tool = false;
        self
    }

    pub fn store(&self) -> &Arc<dyn SpillStore> { &self.archive.store }
    /// Share with streaming tool adapters so all output uses the same lifecycle and backend.
    pub fn archive(&self) -> Arc<SpillArchive> { self.archive.clone() }
}

#[async_trait]
impl api::Plugin for SpillPlugin {
    fn manifest(&self) -> api::PluginManifest {
        let mut manifest = api::PluginManifest::new("spill");
        manifest.provides.push(SPILL_SERVICE.into());
        manifest
    }

    async fn install(&self, registrar: &mut dyn api::Registrar) -> Result<()> {
        self.archive.config.validate()?;
        registrar.publish(api::ServiceRegistration::new(
            SPILL_SERVICE,
            Arc::new(SpillService(self.archive.store.clone())),
        ))?;
        registrar.result_transform(Arc::new(SpillResultTransform::with_archive(
            self.archive.clone(), self.reader_tool,
        )));
        if self.register_reader_tool {
            registrar.tool(Arc::new(SpillReadTool::new(
                self.archive.store.clone(),
                self.archive.config.max_page_bytes,
            )))?;
        }
        Ok(())
    }
}

/// Typed wrapper used by the generic service registry.
pub struct SpillService(pub Arc<dyn SpillStore>);

/// Result transform for large, plain-text tool results.
pub struct SpillResultTransform {
    archive: Arc<SpillArchive>,
    reader_tool: &'static str,
}

impl SpillResultTransform {
    pub fn new(store: Arc<dyn SpillStore>, config: SpillConfig) -> Self {
        Self::with_reader(store, config, "spill_read")
    }
    pub fn with_reader(
        store: Arc<dyn SpillStore>,
        config: SpillConfig,
        reader_tool: &'static str,
    ) -> Self {
        Self::with_archive(Arc::new(SpillArchive::new(store, config)), reader_tool)
    }
    pub fn with_archive(archive: Arc<SpillArchive>, reader_tool: &'static str) -> Self {
        Self { archive, reader_tool }
    }
}

#[async_trait]
impl ResultTransform for SpillResultTransform {
    async fn finish(&self, run_id: &str) -> Result<()> {
        self.archive.finish(run_id).await
    }
    async fn transform(
        &self,
        ctx: &RunContext,
        call: &ToolCall,
        mut result: ToolResult,
    ) -> Result<ToolResult> {
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() {
            return Err(error(ErrorCode::Cancelled, "spill cancelled"));
        }
        // The read tool is always bounded, but this guard also prevents a host
        // from accidentally creating a spill -> read -> spill chain.
        if !ctx.tools_enabled
            || call.name == self.reader_tool
            || result.artifact.is_some()
            || !matches!(result.status, ToolStatus::Success | ToolStatus::Error)
            || ctx
                .allowed_tools
                .as_ref()
                .is_some_and(|tools| !tools.contains(self.reader_tool))
        {
            return Ok(result);
        }
        let Content::Text(text) = &result.content else {
            return Ok(result);
        };
        // The transform now uses the trusted per-run result ceiling when the
        // host supplies it. This keeps the preview below the final core cap.
        let trigger_bytes = self
            .archive.config
            .trigger_bytes
            .min(ctx.limits.max_tool_result_bytes / 2);
        if trigger_bytes == 0 || text.len() < trigger_bytes {
            return Ok(result);
        }
        // Structured observations (e.g. shell exit_code) must survive byte-for-byte.
        // They do not prevent archiving an independent large text body, but are never
        // truncated or moved into a text-only archive to disguise an oversized value.
        let structured_bytes = result.structured.as_ref()
            .map_or(0, |value| serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len()));
        if structured_bytes >= ctx.limits.max_tool_result_bytes { return Ok(result); }

        let original_bytes = result.original_bytes.max(text.len());
        let record = self.archive.put(ctx, &call.id, text).await?;
        ctx.task.check()?;
        if ctx.cancel.is_cancelled() {
            return Err(error(
                ErrorCode::Cancelled,
                "spill cancelled before publishing",
            ));
        }
        validate_spill_id(&record.id)?;
        let artifact = ArtifactRef {
            uri: format!("{SPILL_URI_SCHEME}{}", record.id),
            bytes: record.bytes,
        };
        // Runtime applies the final result cap after transforms. Reserve the
        // artifact descriptor first so a small per-run cap still gets a valid,
        // readable result instead of being rejected as an oversized rich value.
        let artifact_bytes = serde_json::to_vec(&artifact).map_or(usize::MAX, |bytes| bytes.len());
        let preview_limit = ctx
            .limits
            .max_tool_result_bytes
            .saturating_sub(artifact_bytes)
            .saturating_sub(structured_bytes)
            .min(self.archive.config.preview_bytes);
        // Keep a useful marker alongside the reference. If even that cannot
        // fit under the Run's result cap, leave the original result for the
        // core's normal limit handling instead of publishing an unusable
        // artifact result.
        let marker = if self.reader_tool == "read" {
            CORE_READ_MARKER
        } else {
            SPILL_READ_MARKER
        };
        if preview_limit < marker.len() {
            return Ok(result);
        }
        let preview = preview_text(text, preview_limit, marker);
        result.content = preview.into();
        result.original_bytes = original_bytes.max(record.bytes.min(usize::MAX as u64) as usize);
        result.truncated = true;
        result.artifact = Some(artifact);
        Ok(result)
    }
}

fn preview_text(text: &str, max_bytes: usize, marker: &str) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    if max_bytes <= marker.len() + 2 {
        return marker.to_owned();
    }
    let available = max_bytes - marker.len();
    let head = available / 2;
    let tail = available - head;
    let prefix = api::clip_utf8(text, head);
    let suffix_start = text.len().saturating_sub(tail);
    let suffix_start = (suffix_start..text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len());
    format!("{prefix}{marker}{}", &text[suffix_start..])
}

pub(crate) fn error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

pub(crate) fn validate_run_id(run_id: &str) -> Result<()> {
    if run_id.is_empty() || run_id.len() > 256 || run_id.chars().any(char::is_control) {
        return Err(error(ErrorCode::Schema, "invalid run id"));
    }
    Ok(())
}

pub(crate) fn validate_spill_id(id: &str) -> Result<()> {
    if id.len() < 4
        || id.len() > 96
        || !id.starts_with("sp_")
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(error(ErrorCode::Schema, "invalid spill id"));
    }
    Ok(())
}
