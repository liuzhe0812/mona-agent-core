use api::{
    CancellationToken, ModelCaller, ModelOptions, RunContext, RunLimits, Services, TaskControl,
    Tool, ToolContext, ToolProgress,
};
use std::{collections::BTreeMap, sync::Arc};
use tools::{EditTool, ToolConfig};

struct NoProgress;
impl ToolProgress for NoProgress {
    fn report(&self, _: &str) {}
}

struct NoModel;
#[api::async_trait]
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
    let cancel = CancellationToken::new();
    ToolContext {
        run: RunContext {
            run_id: "r".into(),
            task: TaskControl::default(),
            cancel,
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

#[tokio::test]
async fn applies_multiple_original_file_edits_atomically_and_preserves_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one\r\ntwo\r\nthree\r\n").unwrap();
    let tool = EditTool::new(ToolConfig::new(dir.path(), "bash"));
    tool.execute(context(),serde_json::json!({"path":"a.txt","edits":[{"oldText":"one\ntwo","newText":"ONE\nTWO"},{"oldText":"three","newText":"THREE"}]})).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "ONE\r\nTWO\r\nTHREE\r\n"
    );
}

#[tokio::test]
async fn ambiguous_or_overlapping_edits_leave_file_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "same same").unwrap();
    let tool = EditTool::new(ToolConfig::new(dir.path(), "bash"));
    assert!(tool
        .execute(
            context(),
            serde_json::json!({"path":"a.txt","edits":[{"oldText":"same","newText":"x"}]})
        )
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "same same");
    std::fs::write(&file, "aaa").unwrap();
    assert!(tool
        .execute(
            context(),
            serde_json::json!({"path":"a.txt","edits":[{"oldText":"aa","newText":"x"}]})
        )
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "aaa");
}

#[tokio::test]
async fn concurrent_edits_on_different_regions_preserve_both_changes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    let tool = EditTool::new(ToolConfig::new(dir.path(), "bash"));
    let first = tool.execute(
        context(),
        serde_json::json!({"path":"a.txt","edits":[{"oldText":"one","newText":"ONE"}]}),
    );
    let second = tool.execute(
        context(),
        serde_json::json!({"path":"a.txt","edits":[{"oldText":"two","newText":"TWO"}]}),
    );
    let (first, second) = tokio::join!(first, second);
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "ONE\nTWO\n");
}

#[tokio::test]
async fn a_failed_batch_does_not_write_earlier_successful_edits() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    let tool = EditTool::new(ToolConfig::new(dir.path(), "bash"));
    assert!(tool
        .execute(
            context(),
            serde_json::json!({"path":"a.txt","edits":[
                {"oldText":"one","newText":"ONE"},
                {"oldText":"missing","newText":"x"}
            ]}),
        )
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "one\ntwo\n");
}

#[tokio::test]
async fn fuzzy_matching_preserves_unmatched_unicode_and_trailing_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "print(“hello”)   \nkeep  \n").unwrap();
    let tool = EditTool::new(ToolConfig::new(dir.path(), "bash"));
    tool.execute(
        context(),
        serde_json::json!({"path":"a.txt","edits":[{"oldText":"print(\"hello\")","newText":"print(\"bye\")"}]}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "print(\"bye\")   \nkeep  \n"
    );
}

#[tokio::test]
async fn result_contains_real_bounded_diff_and_first_changed_line() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let tool = EditTool::new(ToolConfig::new(dir.path(), "bash"));
    let output = tool
        .execute(
            context(),
            serde_json::json!({"path":"a.txt","edits":[{"oldText":"two","newText":"TWO"}]}),
        )
        .await
        .unwrap();
    assert!(output.content.preview().contains("Successfully replaced"));
    let detail = output.structured.unwrap();
    let diff = detail["diff"].as_str().unwrap();
    assert!(diff.contains("-two"));
    assert!(diff.contains("+TWO"));
    assert_eq!(detail["firstChangedLine"], 2);
}
