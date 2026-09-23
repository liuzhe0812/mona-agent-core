use crate::{
    support::{error, resolve_path},
    ToolConfig,
};
use api::{
    async_trait, Content, ContentBlock, ErrorCode, ImageSource, Tool, ToolConcurrency, ToolContext,
    ToolOutput, ToolSpec,
};
use base64::Engine;
use image::{DynamicImage, ImageFormat};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io::Cursor, path::Path};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
};

/// A host-owned reader for paths that are not ordinary local files.
///
/// Extensions are checked in registration order. They receive the original path so
/// URI-like values can be recognized before local path resolution is attempted.
#[async_trait]
pub trait ReadExtension: Send + Sync {
    fn supports(&self, path: &str) -> bool;

    async fn read(
        &self,
        ctx: ToolContext,
        path: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> api::Result<ToolOutput>;
}

pub struct ReadTool {
    config: ToolConfig,
}

impl ReadTool {
    pub fn new(config: ToolConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: format!(
                "Read a UTF-8 text file by 1-indexed lines, attach a PNG, JPEG, WebP, GIF, or decoded BMP image, or read a host-provided archive path by 1-indexed bytes. Text output is limited to {} lines or {} bytes; use offset and limit to continue.",
                self.config.max_read_lines, self.config.max_read_bytes
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "minLength": 1, "description": "Path to the file to read (relative or absolute)"},
                    "offset": {"type": "integer", "minimum": 1, "description": "1-indexed line number (or 1-indexed byte offset for an archive URI) to start reading from"},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum number of lines (or bytes for an archive URI) to read"}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }

    async fn execute(&self, ctx: ToolContext, arguments: Value) -> api::Result<ToolOutput> {
        ctx.run.task.check()?;
        let args: Arguments = serde_json::from_value(arguments)
            .map_err(|_| error(ErrorCode::Schema, "invalid read arguments"))?;
        if args.path.is_empty() || args.path.contains('\0') {
            return Err(error(ErrorCode::Schema, "path must be nonempty text"));
        }
        if args.offset == Some(0) || args.limit == Some(0) {
            return Err(error(
                ErrorCode::Schema,
                "offset and limit must be positive",
            ));
        }
        if self.config.max_read_bytes == 0 || self.config.max_read_lines == 0 {
            return Err(error(
                ErrorCode::Configuration,
                "read limits must be positive",
            ));
        }

        // Extensions get the first chance to handle the original path. This keeps
        // URI-like paths available to host integrations.
        for extension in &self.config.read_extensions {
            if extension.supports(&args.path) {
                let result = extension
                    .read(ctx.clone(), &args.path, args.offset, args.limit)
                    .await?;
                ctx.run.task.check()?;
                return Ok(result);
            }
        }

        let path = resolve_path(&self.config.cwd, &args.path)?;
        let probe = read_probe(&ctx, &path).await?;
        if let Some(format) = supported_image_format(&probe) {
            let bytes = read_all(&ctx, &path).await?;
            return read_image(&ctx, &path, bytes, format).await;
        }

        read_text_file(
            &ctx,
            &path,
            args.offset,
            args.limit,
            self.config.max_read_bytes,
            self.config.max_read_lines,
        )
        .await
    }
}

async fn read_probe(ctx: &ToolContext, path: &Path) -> api::Result<Vec<u8>> {
    let metadata = tokio::fs::metadata(path).await.map_err(|e| {
        error(
            ErrorCode::Tool,
            format!("cannot inspect {}: {e}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(error(
            ErrorCode::Unsupported,
            "read requires a regular file",
        ));
    }
    let mut file = File::open(path).await.map_err(|e| {
        error(
            ErrorCode::Tool,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let mut probe = vec![0u8; 64];
    let count = tokio::select! {
        biased;
        _ = ctx.run.cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "read cancelled")),
        result = file.read(&mut probe) => result.map_err(|e| error(ErrorCode::Tool, format!("cannot read {}: {e}", path.display())))?,
    };
    probe.truncate(count);
    ctx.run.task.check()?;
    Ok(probe)
}

async fn read_all(ctx: &ToolContext, path: &Path) -> api::Result<Vec<u8>> {
    const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
    let file = File::open(path)
        .await
        .map_err(|e| error(ErrorCode::Tool, format!("cannot read image: {e}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|e| error(ErrorCode::Tool, format!("cannot inspect image: {e}")))?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES {
        return Err(error(
            ErrorCode::Limit,
            "image source must be a regular file of at most 32 MiB",
        ));
    }
    let mut bytes = Vec::new();
    let mut bounded = file.take(MAX_IMAGE_BYTES + 1);
    tokio::select! {
        biased;
        _ = ctx.run.cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "read cancelled")),
        result = bounded.read_to_end(&mut bytes) => result.map_err(|e| error(ErrorCode::Tool, format!("cannot read {}: {e}", path.display())))?,
    };
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(error(ErrorCode::Limit, "image source exceeds 32 MiB"));
    }
    ctx.run.task.check()?;
    Ok(bytes)
}

async fn read_text_file(
    ctx: &ToolContext,
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    max_bytes: usize,
    max_lines: usize,
) -> api::Result<ToolOutput> {
    let file = File::open(path).await.map_err(|e| {
        error(
            ErrorCode::Tool,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let mut reader = BufReader::new(file);
    let start = offset.unwrap_or(1);
    let requested_limit = limit.unwrap_or(max_lines).min(max_lines);
    let output_budget = max_bytes.min(ctx.run.limits.max_tool_result_bytes);
    if output_budget == 0 {
        return Ok(ToolOutput::new(String::new()));
    }

    let mut line_number = 0usize;
    let mut ended_with_newline = false;
    let mut current_line = Vec::new();
    let mut current_line_bytes = 0usize;
    let mut current_line_too_long = false;
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;
    let mut byte_limited = false;
    let mut oversized_line = false;
    let mut utf8_tail = Vec::new();

    loop {
        ctx.run.task.check()?;
        let chunk = tokio::select! {
            biased;
            _ = ctx.run.cancel.cancelled() => return Err(error(ErrorCode::Cancelled, "read cancelled")),
            result = reader.fill_buf() => result.map_err(|e| error(ErrorCode::Tool, format!("cannot read {}: {e}", path.display())))?,
        };
        if chunk.is_empty() {
            break;
        }
        if chunk.contains(&0) {
            return Err(error(
                ErrorCode::Unsupported,
                "read supports UTF-8 text files without NUL bytes",
            ));
        }
        validate_utf8_chunk(chunk, &mut utf8_tail)?;
        let consumed = chunk.len();
        for &byte in chunk {
            current_line_bytes = current_line_bytes.saturating_add(1);
            let candidate = line_number.saturating_add(1) >= start
                && selected.len() < requested_limit
                && !byte_limited
                && !oversized_line;
            if candidate && current_line_bytes <= output_budget {
                current_line.push(byte);
            } else if candidate {
                current_line_too_long = true;
            }
            if byte == b'\n' {
                commit_line(
                    &mut line_number,
                    start,
                    requested_limit,
                    output_budget,
                    &mut current_line,
                    &mut current_line_bytes,
                    &mut current_line_too_long,
                    &mut selected,
                    &mut selected_bytes,
                    &mut byte_limited,
                    &mut oversized_line,
                )?;
                ended_with_newline = true;
            } else {
                ended_with_newline = false;
            }
        }
        reader.consume(consumed);
    }

    if !utf8_tail.is_empty() {
        return Err(error(
            ErrorCode::Unsupported,
            "read supports valid UTF-8 text files",
        ));
    }
    if current_line_bytes > 0 {
        commit_line(
            &mut line_number,
            start,
            requested_limit,
            output_budget,
            &mut current_line,
            &mut current_line_bytes,
            &mut current_line_too_long,
            &mut selected,
            &mut selected_bytes,
            &mut byte_limited,
            &mut oversized_line,
        )?;
    }

    let total_lines = if line_number == 0 {
        1
    } else if ended_with_newline {
        line_number.saturating_add(1)
    } else {
        line_number
    };
    if start > total_lines {
        return Err(error(
            ErrorCode::Tool,
            format!(
                "offset {} is beyond the end of {} ({} lines)",
                start,
                path.display(),
                total_lines
            ),
        ));
    }

    // The empty line after a final newline is a real page in Pi's line model.
    if selected.len() < requested_limit
        && start.saturating_add(selected.len()) == total_lines
        && (line_number == 0 || ended_with_newline)
        && !byte_limited
        && !oversized_line
    {
        selected.push(String::new());
    }

    if oversized_line {
        let message = format!(
            "[Line {} is larger than the {} byte read limit. Use shell to read that line in smaller chunks.]",
            start, output_budget
        );
        return Ok(ToolOutput::new(
            api::clip_utf8(&message, output_budget).to_owned(),
        ));
    }

    let available = total_lines.saturating_sub(start.saturating_sub(1));
    let (output_text, shown, truncated_by) = render_page(
        &selected,
        start,
        total_lines,
        available,
        byte_limited,
        limit.is_some(),
        output_budget,
    );
    if shown == 0 && !selected.is_empty() {
        return Ok(ToolOutput::error(api::clip_utf8(
            &format!("Line {start} cannot fit with its continuation notice. Use shell to read smaller chunks."),
            output_budget,
        ).to_owned()));
    }
    let remaining = available.saturating_sub(shown);
    let mut output = ToolOutput::new(output_text);
    let metadata = json!({
        "path": path.to_string_lossy(),
        "offset": start,
        "lines": shown,
        "total_lines": total_lines,
        "next_offset": if remaining > 0 { Some(start + shown) } else { None::<usize> },
        "truncated": remaining > 0,
        "truncated_by": truncated_by,
    });
    if output
        .content
        .byte_len()
        .saturating_add(json_size(&metadata))
        <= ctx.run.limits.max_tool_result_bytes
    {
        output.structured = Some(metadata);
    }
    Ok(output)
}

fn commit_line(
    line_number: &mut usize,
    start: usize,
    requested_limit: usize,
    output_budget: usize,
    current_line: &mut Vec<u8>,
    current_line_bytes: &mut usize,
    current_line_too_long: &mut bool,
    selected: &mut Vec<String>,
    selected_bytes: &mut usize,
    byte_limited: &mut bool,
    oversized_line: &mut bool,
) -> api::Result<()> {
    let current_number = line_number.saturating_add(1);
    if current_number >= start
        && selected.len() < requested_limit
        && !*byte_limited
        && !*oversized_line
    {
        if *current_line_too_long {
            if selected.is_empty() {
                *oversized_line = true;
            } else {
                *byte_limited = true;
            }
        } else {
            let text = String::from_utf8(std::mem::take(current_line)).map_err(|_| {
                error(
                    ErrorCode::Unsupported,
                    "read supports valid UTF-8 text files",
                )
            })?;
            if selected_bytes.saturating_add(text.len()) > output_budget {
                *byte_limited = true;
            } else {
                *selected_bytes = selected_bytes.saturating_add(text.len());
                selected.push(text);
            }
        }
    }
    *line_number = (*line_number).saturating_add(1);
    current_line.clear();
    *current_line_bytes = 0;
    *current_line_too_long = false;
    Ok(())
}

fn validate_utf8_chunk(chunk: &[u8], tail: &mut Vec<u8>) -> api::Result<()> {
    if tail.is_empty() {
        match std::str::from_utf8(chunk) {
            Ok(_) => Ok(()),
            Err(error_value) if error_value.error_len().is_none() => {
                let start = error_value.valid_up_to();
                tail.extend_from_slice(&chunk[start..]);
                Ok(())
            }
            Err(_) => Err(error(
                ErrorCode::Unsupported,
                "read supports valid UTF-8 text files",
            )),
        }
    } else {
        let mut combined = Vec::with_capacity(tail.len().saturating_add(chunk.len()));
        combined.extend_from_slice(tail);
        combined.extend_from_slice(chunk);
        tail.clear();
        match std::str::from_utf8(&combined) {
            Ok(_) => Ok(()),
            Err(error_value) if error_value.error_len().is_none() => {
                let start = error_value.valid_up_to();
                if combined.len().saturating_sub(start) > 3 {
                    return Err(error(
                        ErrorCode::Unsupported,
                        "read supports valid UTF-8 text files",
                    ));
                }
                tail.extend_from_slice(&combined[start..]);
                Ok(())
            }
            Err(_) => Err(error(
                ErrorCode::Unsupported,
                "read supports valid UTF-8 text files",
            )),
        }
    }
}

fn render_page(
    selected: &[String],
    start: usize,
    total_lines: usize,
    available: usize,
    byte_limited: bool,
    user_limited: bool,
    max_bytes: usize,
) -> (String, usize, Option<&'static str>) {
    let mut shown = selected.len();
    let truncated_by = if byte_limited {
        Some("bytes")
    } else if available > shown {
        Some(if user_limited { "limit" } else { "lines" })
    } else {
        None
    };
    loop {
        let remaining = available.saturating_sub(shown);
        let body = selected[..shown].concat();
        if remaining == 0 {
            return (body, shown, truncated_by);
        }
        let next_offset = start.saturating_add(shown);
        let full_marker = if byte_limited {
            format!(
                "[Showing lines {}-{} of {} (byte limit). Use offset={} to continue.]",
                start,
                start.saturating_add(shown).saturating_sub(1),
                total_lines,
                next_offset
            )
        } else if user_limited {
            format!(
                "[{} more lines in file. Use offset={} to continue.]",
                remaining, next_offset
            )
        } else {
            format!(
                "[Showing lines {}-{} of {} (line limit). Use offset={} to continue.]",
                start,
                start.saturating_add(shown).saturating_sub(1),
                total_lines,
                next_offset
            )
        };
        let marker = if full_marker.len() <= max_bytes {
            full_marker
        } else {
            format!("[offset={next_offset}]")
        };
        let body_without_final_newline = body.strip_suffix('\n').unwrap_or(&body);
        let candidate = if body_without_final_newline.is_empty() {
            marker.clone()
        } else {
            format!("{body_without_final_newline}\n\n{marker}")
        };
        if candidate.len() <= max_bytes {
            return (candidate, shown, truncated_by);
        }
        if shown == 0 {
            return (
                api::clip_utf8(&marker, max_bytes).to_owned(),
                0,
                truncated_by,
            );
        }
        shown -= 1;
    }
}

async fn read_image(
    ctx: &ToolContext,
    path: &Path,
    bytes: Vec<u8>,
    format: ImageFormat,
) -> api::Result<ToolOutput> {
    let cancel = ctx.run.cancel.clone();
    let path = path.to_string_lossy().into_owned();
    let budget = ctx.run.limits.max_tool_result_bytes;
    let output = tokio::task::spawn_blocking(move || {
        read_image_blocking(&cancel, &path, &bytes, format, budget)
    })
    .await
    .map_err(|e| error(ErrorCode::Tool, format!("image operation failed: {e}")))??;
    ctx.run.task.check()?;
    Ok(output)
}

fn read_image_blocking(
    cancel: &api::CancellationToken,
    path: &str,
    bytes: &[u8],
    format: ImageFormat,
    budget: usize,
) -> api::Result<ToolOutput> {
    if cancel.is_cancelled() {
        return Err(error(ErrorCode::Cancelled, "read cancelled"));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| {
        error(
            ErrorCode::Unsupported,
            format!("cannot decode image {path}: {e}"),
        )
    })?;
    let original_width = decoded.width();
    let original_height = decoded.height();
    if (original_width as u64).saturating_mul(original_height as u64) > 64_000_000 {
        return Err(error(
            ErrorCode::Limit,
            "image exceeds the 64 million pixel decode limit",
        ));
    }
    let source_media_type = image_media_type(format);

    if format != ImageFormat::Bmp && original_width <= 2000 && original_height <= 2000 {
        if let Some(output) = image_output(
            bytes,
            source_media_type,
            original_width,
            original_height,
            original_width,
            original_height,
            false,
            budget,
        ) {
            return Ok(output);
        }
    }

    let mut dimensions = fit_dimensions(original_width, original_height, 2000);
    loop {
        if cancel.is_cancelled() {
            return Err(error(ErrorCode::Cancelled, "read cancelled"));
        }
        let resized = if dimensions == (original_width, original_height) {
            decoded.clone()
        } else {
            decoded.resize_exact(
                dimensions.0,
                dimensions.1,
                image::imageops::FilterType::Lanczos3,
            )
        };

        let png = encode_image(&resized, ImageFormat::Png, None)?;
        if cancel.is_cancelled() {
            return Err(error(ErrorCode::Cancelled, "read cancelled"));
        }
        if let Some(output) = image_output(
            &png,
            "image/png",
            original_width,
            original_height,
            dimensions.0,
            dimensions.1,
            dimensions != (original_width, original_height) || format == ImageFormat::Bmp,
            budget,
        ) {
            return Ok(output);
        }
        for quality in [80u8, 60, 40] {
            if cancel.is_cancelled() {
                return Err(error(ErrorCode::Cancelled, "read cancelled"));
            }
            let jpeg = encode_image(&resized, ImageFormat::Jpeg, Some(quality))?;
            if let Some(output) = image_output(
                &jpeg,
                "image/jpeg",
                original_width,
                original_height,
                dimensions.0,
                dimensions.1,
                true,
                budget,
            ) {
                return Ok(output);
            }
        }
        if dimensions == (1, 1) {
            break;
        }
        dimensions = (
            dimensions.0.saturating_mul(3).saturating_div(4).max(1),
            dimensions.1.saturating_mul(3).saturating_div(4).max(1),
        );
    }

    let message = format!(
        "[Image omitted: it could not be encoded within the {} byte run output budget.]",
        budget
    );
    Ok(ToolOutput::new(api::clip_utf8(&message, budget).to_owned()))
}

fn image_output(
    bytes: &[u8],
    media_type: &'static str,
    original_width: u32,
    original_height: u32,
    width: u32,
    height: u32,
    resized: bool,
    budget: usize,
) -> Option<ToolOutput> {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let content = Content::Blocks(vec![ContentBlock::Image {
        media_type: media_type.into(),
        source: ImageSource::Base64 { data },
    }]);
    let mut output = ToolOutput::new(content);
    if output.content.byte_len() > budget {
        return None;
    }
    let metadata = json!({
        "media_type": media_type,
        "width": width,
        "height": height,
        "original_width": original_width,
        "original_height": original_height,
        "resized": resized,
    });
    if output
        .content
        .byte_len()
        .saturating_add(json_size(&metadata))
        <= budget
    {
        output.structured = Some(metadata);
    }
    Some(output)
}

fn encode_image(
    image: &DynamicImage,
    format: ImageFormat,
    jpeg_quality: Option<u8>,
) -> api::Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    if format == ImageFormat::Jpeg {
        let rgb = DynamicImage::ImageRgb8(image.to_rgb8());
        rgb.write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut cursor,
            jpeg_quality.unwrap_or(80),
        ))
        .map_err(|e| {
            error(
                ErrorCode::Unsupported,
                format!("cannot encode JPEG image: {e}"),
            )
        })?;
    } else {
        image
            .write_to(&mut cursor, format)
            .map_err(|e| error(ErrorCode::Unsupported, format!("cannot encode image: {e}")))?;
    }
    Ok(cursor.into_inner())
}

fn supported_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    let format = image::guess_format(bytes).ok()?;
    match format {
        ImageFormat::Png
        | ImageFormat::Jpeg
        | ImageFormat::Gif
        | ImageFormat::WebP
        | ImageFormat::Bmp => Some(format),
        _ => None,
    }
}

fn image_media_type(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Bmp => "image/png",
        _ => "image/png",
    }
}

fn fit_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if width <= max_edge && height <= max_edge {
        return (width, height);
    }
    if width >= height {
        (
            max_edge,
            ((height as u64 * max_edge as u64) / width as u64)
                .max(1)
                .min(u32::MAX as u64) as u32,
        )
    } else {
        (
            ((width as u64 * max_edge as u64) / height as u64)
                .max(1)
                .min(u32::MAX as u64) as u32,
            max_edge,
        )
    }
}

fn json_size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}
