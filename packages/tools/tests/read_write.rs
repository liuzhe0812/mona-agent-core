use api::{
    async_trait, CancellationToken, Content, ContentBlock, ImageSource, ModelCaller, ModelOptions,
    RunContext, RunLimits, Services, TaskControl, Tool, ToolContext, ToolOutput, ToolProgress,
    ToolResult,
};
use base64::Engine;
use image::GenericImageView;
use std::{collections::BTreeMap, io::Cursor, sync::Arc};
use tools::{ReadExtension, ReadTool, ToolConfig, WriteTool};

struct NoProgress;
impl ToolProgress for NoProgress {
    fn report(&self, _: &str) {}
}

struct NoModel;
#[async_trait]
impl ModelCaller for NoModel {
    async fn complete(
        &self,
        _: api::ModelRequest,
        _: Option<Arc<dyn api::ModelSink>>,
    ) -> api::Result<api::ModelReply> {
        Err(api::AgentError::new(api::ErrorCode::Unsupported, "unused"))
    }
}

fn context() -> ToolContext {
    ToolContext {
        run: RunContext {
            run_id: "r".into(),
            task: TaskControl::default(),
            cancel: CancellationToken::new(),
            model: Arc::new(NoModel),
            services: Services::default(),
            metadata: Arc::new(BTreeMap::new()),
            model_options: ModelOptions::default(),
            limits: RunLimits::default(),
            request_overhead_bytes: 0,
            request_tools: Arc::new(Vec::new()),
            context_sources: Arc::new(Vec::new()),
            model_context_window_tokens: None,
            tools_enabled: true,
            allowed_tools: None,
        },
        call_id: "c".into(),
        progress: Arc::new(NoProgress),
    }
}

fn context_with_budget(max_tool_result_bytes: usize) -> ToolContext {
    let mut context = context();
    context.run.limits.max_tool_result_bytes = max_tool_result_bytes;
    context
}

#[tokio::test]
async fn reads_one_based_text_pages_with_bounded_output() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let mut config = ToolConfig::new(dir.path(), "bash");
    config.max_read_lines = 2;
    let tool = ReadTool::new(config);

    let first = tool
        .execute(
            context(),
            serde_json::json!({"path":"a.txt", "offset":1, "limit":2}),
        )
        .await
        .unwrap();
    assert_eq!(
        first.content.text(),
        "one\ntwo\n\n[2 more lines in file. Use offset=3 to continue.]"
    );

    let second = tool
        .execute(context(), serde_json::json!({"path":"a.txt", "offset":3}))
        .await
        .unwrap();
    assert_eq!(second.content.text(), "three\n");
}

#[tokio::test]
async fn rejects_non_utf8_and_nul_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.bin"), [0xff, 0xfe]).unwrap();
    std::fs::write(dir.path().join("nul.txt"), b"ok\0bad").unwrap();
    let tool = ReadTool::new(ToolConfig::new(dir.path(), "bash"));

    let invalid = tool
        .execute(context(), serde_json::json!({"path":"bad.bin"}))
        .await
        .unwrap_err();
    assert_eq!(invalid.code, api::ErrorCode::Unsupported);

    let nul = tool
        .execute(context(), serde_json::json!({"path":"nul.txt"}))
        .await
        .unwrap_err();
    assert_eq!(nul.code, api::ErrorCode::Unsupported);
}

#[tokio::test]
async fn reads_supported_images_as_base64_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let image = image::DynamicImage::ImageRgba8(image::ImageBuffer::from_fn(4, 3, |x, y| {
        image::Rgba([(x * 40) as u8, (y * 60) as u8, 120, 255])
    }));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.path().join("image.png"), encoded.into_inner()).unwrap();
    let tool = ReadTool::new(ToolConfig::new(dir.path(), "bash"));

    let output = tool
        .execute(context(), serde_json::json!({"path":"image.png"}))
        .await
        .unwrap();
    let Content::Blocks(blocks) = output.content else {
        panic!("expected image content blocks");
    };
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0],
        ContentBlock::Image {
            media_type,
            source: ImageSource::Base64 { .. }
        } if media_type == "image/png"
    ));
}

#[tokio::test]
async fn decodes_bmp_and_converts_it_to_a_supported_image() {
    let dir = tempfile::tempdir().unwrap();
    let image = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(3, 2, |x, y| {
        image::Rgb([(x * 80) as u8, (y * 100) as u8, 40])
    }));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Bmp)
        .unwrap();
    std::fs::write(dir.path().join("image.bmp"), encoded.into_inner()).unwrap();
    let tool = ReadTool::new(ToolConfig::new(dir.path(), "bash"));

    let output = tool
        .execute(context(), serde_json::json!({"path":"image.bmp"}))
        .await
        .unwrap();
    let Content::Blocks(blocks) = output.content else {
        panic!("expected image content blocks");
    };
    let ContentBlock::Image {
        media_type,
        source: ImageSource::Base64 { data },
    } = &blocks[0]
    else {
        panic!("expected an image block");
    };
    assert!(media_type == "image/png" || media_type == "image/jpeg");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!(decoded.dimensions(), (3, 2));
}

#[tokio::test]
async fn resizes_a_large_image_without_a_fixed_small_file_cap() {
    let dir = tempfile::tempdir().unwrap();
    let image = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(2200, 1400, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
    }));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Jpeg)
        .unwrap();
    let bytes = encoded.into_inner();
    assert!(bytes.len() > 50 * 1024);
    std::fs::write(dir.path().join("large.jpg"), bytes).unwrap();
    let tool = ReadTool::new(ToolConfig::new(dir.path(), "bash"));

    let output = tool
        .execute(
            context_with_budget(256 * 1024),
            serde_json::json!({"path":"large.jpg"}),
        )
        .await
        .unwrap();
    let Content::Blocks(blocks) = output.content else {
        panic!("expected image content blocks");
    };
    let ContentBlock::Image {
        source: ImageSource::Base64 { data },
        ..
    } = &blocks[0]
    else {
        panic!("expected an image block");
    };
    let decoded = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
    )
    .unwrap();
    assert!(decoded.width().max(decoded.height()) <= 2000);
}

#[tokio::test]
async fn keeps_text_page_marker_when_run_budget_is_small() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.txt"),
        "0123456789\nabcdefghij\nklmnopqrst\n",
    )
    .unwrap();
    let mut config = ToolConfig::new(dir.path(), "bash");
    config.max_read_bytes = 256;
    let tool = ReadTool::new(config);

    let output = tool
        .execute(
            context_with_budget(128),
            serde_json::json!({"path":"a.txt", "offset":1, "limit":2}),
        )
        .await
        .unwrap();
    assert!(output.content.text().contains("offset="));
    let result = ToolResult::from_output("read", output);
    assert!(result.payload_bytes() <= 128);
    std::fs::write(
        dir.path().join("long.txt"),
        format!("{}\nnext\n", "x".repeat(120)),
    )
    .unwrap();
    let no_progress = tool
        .execute(
            context_with_budget(128),
            serde_json::json!({"path":"long.txt", "limit":1}),
        )
        .await
        .unwrap();
    assert!(
        no_progress.is_error,
        "a page must not silently repeat the same offset forever"
    );
}

struct Extension;
#[async_trait]
impl ReadExtension for Extension {
    fn supports(&self, path: &str) -> bool {
        path.starts_with("artifact:")
    }

    async fn read(
        &self,
        _ctx: ToolContext,
        path: &str,
        _offset: Option<usize>,
        _limit: Option<usize>,
    ) -> api::Result<ToolOutput> {
        Ok(ToolOutput::new(format!("extension:{path}")))
    }
}

#[tokio::test]
async fn routes_supported_paths_to_extensions_before_local_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ToolConfig::new(dir.path(), "bash");
    config.read_extensions.push(Arc::new(Extension));
    let tool = ReadTool::new(config);

    let output = tool
        .execute(context(), serde_json::json!({"path":"artifact:one"}))
        .await
        .unwrap();
    assert_eq!(output.content.text(), "extension:artifact:one");
}

#[tokio::test]
async fn writes_and_overwrites_files_with_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool::new(ToolConfig::new(dir.path(), "bash"));

    tool.execute(
        context(),
        serde_json::json!({"path":"nested/a.txt", "content":"first"}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/a.txt")).unwrap(),
        "first"
    );

    tool.execute(
        context(),
        serde_json::json!({"path":"nested/a.txt", "content":"second"}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/a.txt")).unwrap(),
        "second"
    );
}
