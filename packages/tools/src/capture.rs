//! Bounded command capture. A temporary spool is not an archive and has no public locator.
use crate::{support::error, OutputArchive, ToolConfig};
use api::{ErrorCode, ToolContext, ToolOutput, ToolResult};
use std::{collections::VecDeque, io::SeekFrom, sync::Arc};
use tokio::{fs::File, io::{AsyncSeekExt, AsyncWriteExt}};

const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) struct Capture {
    preview: VecDeque<u8>,
    spool: Option<File>,
    bytes: usize,
    raw_bytes: usize,
    trigger: usize,
    preview_limit: usize,
    archive: Option<Arc<dyn OutputArchive>>,
    decoders: [Decoder; 2],
}
impl Capture {
    pub fn new(config: &ToolConfig, ctx: &ToolContext) -> api::Result<Self> {
        if config.max_command_bytes == 0 { return Err(error(ErrorCode::Configuration, "shell output limit must be positive")); }
        let can_read = ctx.run.tools_enabled && ctx.run.allowed_tools.as_ref().is_none_or(|names| names.contains("read"));
        let archive = config.output_archive.clone().filter(|_| can_read);
        if archive.as_ref().is_some_and(|a| a.trigger_bytes() == 0 || a.preview_bytes() == 0) {
            return Err(error(ErrorCode::Configuration, "output archive limits must be positive"));
        }
        let inline = config.max_command_bytes.min(MAX_OUTPUT_BYTES).min(ctx.run.limits.max_tool_result_bytes / 2).max(1);
        let trigger = archive.as_ref().map_or(inline, |a| a.trigger_bytes().min(inline));
        let preview_limit = archive.as_ref().map_or(inline, |a| a.preview_bytes().min(inline));
        Ok(Self { preview: VecDeque::new(), spool: None, bytes: 0, raw_bytes: 0,
            trigger, preview_limit, archive, decoders: Default::default() })
    }
    pub async fn append(&mut self, stream: usize, input: &[u8], eof: bool) -> api::Result<()> {
        self.raw_bytes = self.raw_bytes.saturating_add(input.len());
        let text = self.decoders[stream].decode(input, eof);
        let bytes = text.as_bytes();
        if self.raw_bytes > MAX_OUTPUT_BYTES || self.bytes.saturating_add(bytes.len()) > MAX_OUTPUT_BYTES {
            return Err(error(ErrorCode::Limit, "command stopped: output exceeds the 8 MiB capture limit"));
        }
        if self.spool.is_none() && self.archive.is_some() && self.bytes.saturating_add(bytes.len()) >= self.trigger {
            // Anonymous handle: OS cleanup on drop, no index and no second archive scheme.
            let mut spool = File::from_std(tempfile::tempfile().map_err(capture_error)?);
            let (head, tail) = self.preview.as_slices();
            spool.write_all(head).await.map_err(capture_error)?;
            spool.write_all(tail).await.map_err(capture_error)?;
            self.spool = Some(spool);
        }
        if let Some(spool) = &mut self.spool { spool.write_all(bytes).await.map_err(capture_error)?; }
        self.bytes += bytes.len();
        self.preview.extend(bytes);
        // Until spooling begins keep the complete prefix, even if the final preview is smaller.
        let limit = if self.archive.is_some() && self.spool.is_none() { self.trigger } else { self.preview_limit };
        if self.preview.len() > limit { self.preview.drain(..self.preview.len() - limit); }
        Ok(())
    }
    pub async fn finish(mut self, ctx: &ToolContext, exit_code: Option<i32>, success: bool) -> api::Result<ToolOutput> {
        let artifact = if let Some(mut spool) = self.spool.take() {
            spool.flush().await.map_err(capture_error)?;
            spool.seek(SeekFrom::Start(0)).await.map_err(capture_error)?;
            let archive = self.archive.as_ref().expect("spooling requires an archive");
            let artifact = archive.store(ctx, &mut spool, self.bytes).await.map_err(|mut error| {
                error.message = format!("command completed (exit {exit_code:?}); output archive failed: {}. Inspect the result before considering another execution.", error.message);
                error
            })?;
            artifact.validate()?;
            if artifact.bytes != self.bytes as u64 { return Err(error(ErrorCode::Tool, "archive returned an inconsistent capture size")); }
            Some(artifact)
        } else { None };
        let mut truncated = self.preview.len() < self.bytes;
        let mut output = ToolOutput::new("");
        output.is_error = !success;
        output.structured = Some(serde_json::json!({"exit_code":exit_code,"truncated":truncated,
            "output_bytes":self.bytes,"raw_output_bytes":self.raw_bytes,
            "utf8_replacements":self.decoders.iter().any(|d| d.replaced),
            "full_output_path":artifact.as_ref().map(|a| a.uri.as_str())}));
        output.artifact = artifact;
        let mut overhead = ToolResult::from_output(&ctx.call_id, output.clone()).payload_bytes();
        if !truncated && self.preview.len() > ctx.run.limits.max_tool_result_bytes.saturating_sub(overhead) {
            truncated = true;
            output.structured.as_mut().expect("shell metadata")["truncated"] = true.into();
            overhead = ToolResult::from_output(&ctx.call_id, output.clone()).payload_bytes();
        }
        let marker = if !truncated { "" } else if output.artifact.is_some() {
            "[Output shortened; use read with the artifact URI. Offset and limit are bytes, starting at 1.]\n"
        } else { "[Output shortened; omitted bytes were not retained because no usable output archive is installed.]\n" };
        let available = ctx.run.limits.max_tool_result_bytes.saturating_sub(overhead);
        if available < marker.len() { return Err(error(ErrorCode::Limit, "command completed; result budget cannot fit its exit status and archive metadata")); }
        let bytes: Vec<u8> = self.preview.into_iter().collect();
        let mut start = bytes.len().saturating_sub(available - marker.len());
        while start < bytes.len() && bytes[start] & 0xc0 == 0x80 { start += 1; }
        let tail = String::from_utf8_lossy(&bytes[start..]);
        output.content = if self.bytes == 0 { "(no output)".into() } else { format!("{marker}{tail}").into() };
        if ToolResult::from_output(&ctx.call_id, output.clone()).payload_bytes() > ctx.run.limits.max_tool_result_bytes {
            return Err(error(ErrorCode::Limit, "command result exceeds its output budget"));
        }
        Ok(output)
    }
}
fn capture_error(e: std::io::Error) -> api::AgentError { error(ErrorCode::Tool, format!("command capture I/O failed: {e}")) }

/// Each pipe has its own UTF-8 tail. A split character must not be mixed with stderr.
#[derive(Default)]
struct Decoder { tail: Vec<u8>, replaced: bool }
impl Decoder {
    fn decode(&mut self, input: &[u8], eof: bool) -> String {
        let mut bytes = std::mem::take(&mut self.tail); bytes.extend_from_slice(input);
        let mut remaining = bytes.as_slice(); let mut text = String::new();
        while !remaining.is_empty() {
            match std::str::from_utf8(remaining) {
                Ok(valid) => { text.push_str(valid); break; },
                Err(e) => {
                    text.push_str(std::str::from_utf8(&remaining[..e.valid_up_to()]).expect("valid UTF-8 prefix"));
                    remaining = &remaining[e.valid_up_to()..];
                    match e.error_len() {
                        Some(count) => { text.push('\u{fffd}'); self.replaced = true; remaining = &remaining[count..]; },
                        None if eof => { text.push('\u{fffd}'); self.replaced = true; break; },
                        None => { self.tail.extend_from_slice(remaining); break; },
                    }
                }
            }
        }
        text
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_utf8_is_preserved_and_only_invalid_bytes_are_replaced() {
        let mut d = Decoder::default();
        assert_eq!(d.decode(&[0xe4], false), "");
        assert_eq!(d.decode(&[0xb8, 0xad], false), "中");
        assert!(!d.replaced);
        assert_eq!(d.decode(&[0xff, b'x', 0xe4], false), "\u{fffd}x");
        assert_eq!(d.decode(&[], true), "\u{fffd}");
        assert!(d.replaced);
    }
}
