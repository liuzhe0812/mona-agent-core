use api::{
    async_trait, CancellationToken, FinishReason, ModelCaller, ModelReply, ModelRequest, ModelSink,
    RunContext, RunLimits, Services, TaskControl, Tool, ToolContext, ToolProgress,
};
use serde_json::json;
use std::{fs, path::Path, sync::Arc};
use tempfile::tempdir;
use tools::{FindTool, GrepTool, LsTool, ToolConfig};

struct NoopModel;

#[async_trait]
impl ModelCaller for NoopModel {
    async fn complete(
        &self,
        _request: ModelRequest,
        _sink: Option<Arc<dyn ModelSink>>,
    ) -> api::Result<ModelReply> {
        Ok(ModelReply {
            content: String::new(),
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
            finish: FinishReason::Stop,
            usage: None,
        })
    }
}

struct NoProgress;

impl ToolProgress for NoProgress {
    fn report(&self, _text: &str) {}
}

fn context(cancel: CancellationToken, _cwd: &Path) -> ToolContext {
    let run = RunContext {
        run_id: "search-test".into(),
        task: TaskControl::default(),
        cancel,
        model: Arc::new(NoopModel),
        services: Services::default(),
        metadata: Arc::default(),
        model_options: Default::default(),
        limits: RunLimits::default(),
        request_overhead_bytes: 0,
        request_tools: Arc::new(Vec::new()),
        context_sources: Arc::new(Vec::new()),
        model_context_window_tokens: None,
        tools_enabled: true,
        allowed_tools: None,
    };
    ToolContext {
        run,
        call_id: "search-call".into(),
        progress: Arc::new(NoProgress),
    }
}

fn config(root: &Path) -> ToolConfig {
    ToolConfig::new(root, "bash")
}

#[tokio::test]
async fn grep_is_recursive_literal_glob_filtered_and_skips_binary() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "first\nneedle here\n").unwrap();
    fs::write(root.path().join("src/skip.txt"), "needle but filtered\n").unwrap();
    fs::write(root.path().join("binary.bin"), b"needle\0hidden").unwrap();

    let mut tool_config = config(root.path());
    tool_config.max_read_bytes = 4096;
    tool_config.max_search_results = 10;
    let tool = GrepTool::new(tool_config);
    let output = tool
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"query":"needle", "glob":"*.rs"}),
        )
        .await
        .unwrap();

    assert_eq!(output.content.text(), "src/main.rs:2: needle here");
}

#[tokio::test]
async fn grep_scans_past_the_output_read_limit() {
    let root = tempdir().unwrap();
    let content = format!("{}needle\n", "x\n".repeat(40));
    fs::write(root.path().join("late.txt"), content).unwrap();

    let mut tool_config = config(root.path());
    tool_config.max_read_bytes = 64;
    tool_config.max_search_results = 10;
    let tool = GrepTool::new(tool_config);
    let output = tool
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"query":"needle"}),
        )
        .await
        .unwrap();

    assert!(output.content.text().contains("late.txt:41: needle"));
}

#[tokio::test]
async fn grep_reports_result_limit_truncation() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "needle\n").unwrap();
    fs::write(root.path().join("b.txt"), "needle\n").unwrap();

    let mut tool_config = config(root.path());
    tool_config.max_read_bytes = 4096;
    tool_config.max_search_results = 1;
    let tool = GrepTool::new(tool_config);
    let output = tool
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"query":"needle"}),
        )
        .await
        .unwrap();

    assert!(output.content.text().contains("result limit reached"));
}

#[tokio::test]
async fn grep_and_find_respect_gitignore() {
    let root = tempdir().unwrap();
    fs::write(
        root.path().join(".gitignore"),
        "ignored.txt\nignored-dir/\n",
    )
    .unwrap();
    fs::write(root.path().join("visible.txt"), "needle\n").unwrap();
    fs::write(root.path().join("ignored.txt"), "needle\n").unwrap();
    fs::create_dir(root.path().join("ignored-dir")).unwrap();
    fs::write(root.path().join("ignored-dir/hidden.txt"), "needle\n").unwrap();

    let mut tool_config = config(root.path());
    tool_config.max_read_bytes = 4096;
    tool_config.max_search_results = 10;
    let grep = GrepTool::new(tool_config.clone());
    let grep_output = grep
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"query":"needle", "glob":"*.txt"}),
        )
        .await
        .unwrap();
    assert_eq!(grep_output.content.text(), "visible.txt:1: needle");

    let find = FindTool::new(tool_config);
    let find_output = find
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"pattern":"*.txt"}),
        )
        .await
        .unwrap();
    assert_eq!(find_output.content.text(), "visible.txt");
}

#[tokio::test]
async fn find_and_ls_are_bounded_and_sorted() {
    let root = tempdir().unwrap();
    fs::create_dir(root.path().join("z-dir")).unwrap();
    fs::create_dir(root.path().join("a-dir")).unwrap();
    fs::write(root.path().join("b.txt"), "b").unwrap();
    fs::write(root.path().join("a.rs"), "a").unwrap();
    fs::write(root.path().join("z-dir/nested.rs"), "z").unwrap();

    let mut tool_config = config(root.path());
    tool_config.max_read_bytes = 4096;
    tool_config.max_search_results = 2;
    let find = FindTool::new(tool_config.clone());
    let find_output = find
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"pattern":"**/*.rs"}),
        )
        .await
        .unwrap();
    assert_eq!(find_output.content.text(), "a.rs\nz-dir/nested.rs");

    let ls = LsTool::new(tool_config);
    let ls_output = ls
        .execute(context(CancellationToken::new(), root.path()), json!({}))
        .await
        .unwrap();
    assert_eq!(
        ls_output.content.text(),
        "a-dir/\na.rs\n\n[2 result limit reached; increase max_search_results to see more.]"
    );
}

#[tokio::test]
async fn find_and_ls_report_result_limit_truncation() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "a").unwrap();
    fs::write(root.path().join("b.txt"), "b").unwrap();

    let mut tool_config = config(root.path());
    tool_config.max_read_bytes = 4096;
    tool_config.max_search_results = 1;

    let find = FindTool::new(tool_config.clone());
    let find_output = find
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"pattern":"*.txt"}),
        )
        .await
        .unwrap();
    assert!(find_output.content.text().contains("result limit reached"));

    let ls = LsTool::new(tool_config);
    let ls_output = ls
        .execute(context(CancellationToken::new(), root.path()), json!({}))
        .await
        .unwrap();
    assert!(ls_output.content.text().contains("result limit reached"));
}

#[tokio::test]
async fn search_honors_unknown_field_schema_and_cancellation() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("one.txt"), "needle").unwrap();
    let tool = GrepTool::new(config(root.path()));

    let schema_error = tool
        .execute(
            context(CancellationToken::new(), root.path()),
            json!({"query":"needle", "unexpected":true}),
        )
        .await
        .unwrap_err();
    assert_eq!(schema_error.code, api::ErrorCode::Schema);

    let cancel = CancellationToken::new();
    cancel.cancel();
    let cancelled = tool
        .execute(context(cancel, root.path()), json!({"query":"needle"}))
        .await
        .unwrap_err();
    assert_eq!(cancelled.code, api::ErrorCode::Cancelled);
}
