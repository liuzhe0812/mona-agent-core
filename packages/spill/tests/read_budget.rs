use api::{
    Content, FinishReason, ModelCaller, ModelReply, ModelRequest, ModelSink, RunContext, RunLimits,
    Services, TaskControl, Tool, ToolOutput, ToolResult,
};
use async_trait::async_trait;
use serde_json::json;
use spill::{CleanupReport, LocalSpillStore, SpillPage, SpillReadTool, SpillRecord, SpillStore};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tempfile::tempdir;

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
            tool_calls: Vec::new(),
            reasoning_content: None,
            provider_data: None,
            finish: FinishReason::Stop,
            usage: None,
        })
    }
}

struct NoProgress;

impl api::ToolProgress for NoProgress {
    fn report(&self, _text: &str) {}
}

fn context(max_tool_result_bytes: usize) -> RunContext {
    RunContext {
        run_id: "run-a".into(),
        task: TaskControl::default(),
        cancel: api::CancellationToken::new(),
        model: Arc::new(NoopModel),
        services: Services::default(),
        metadata: Arc::default(),
        model_options: Default::default(),
        limits: RunLimits {
            max_tool_result_bytes,
            ..RunLimits::default()
        },
        request_overhead_bytes: 0,
        request_tools: Arc::new(Vec::new()),
        context_sources: Arc::new(Vec::new()),
        model_context_window_tokens: None,
        tools_enabled: true,
        allowed_tools: None,
    }
}

struct BoundedStore {
    text: String,
    requests: Mutex<Vec<usize>>,
}

impl BoundedStore {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<usize> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl SpillStore for BoundedStore {
    async fn put_stream(&self, _run_id: &str, _call_id: &str,
        _source: &mut (dyn tokio::io::AsyncRead + Unpin + Send), _bytes: usize) -> api::Result<SpillRecord> {
        Err(api::AgentError::new(
            api::ErrorCode::Unsupported,
            "test store does not write entries",
        ))
    }

    async fn read_page(
        &self,
        _run_id: &str,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> api::Result<SpillPage> {
        self.requests.lock().unwrap().push(limit);
        let mut start = offset;
        while start < self.text.len() && !self.text.is_char_boundary(start) {
            start += 1;
        }
        let mut end = (start + limit).min(self.text.len());
        while end > start && !self.text.is_char_boundary(end) {
            end -= 1;
        }
        Ok(SpillPage {
            id: id.to_owned(),
            offset,
            next_offset: end,
            total_bytes: self.text.len(),
            eof: end >= self.text.len(),
            text: self.text[start..end].to_owned(),
        })
    }

    async fn cleanup(&self, _active_runs: &BTreeSet<String>) -> api::Result<CleanupReport> {
        Ok(CleanupReport::default())
    }
}

async fn read(
    tool: &SpillReadTool,
    ctx: RunContext,
    offset: usize,
    limit: usize,
) -> api::Result<ToolOutput> {
    tool.execute(
        api::ToolContext {
            run: ctx,
            call_id: "call-42".into(),
            progress: Arc::new(NoProgress),
        },
        json!({"id": "sp_mock", "offset": offset, "limit": limit}),
    )
    .await
}

#[tokio::test]
async fn run_budget_caps_a_large_requested_page_and_keeps_continuation() {
    let store = Arc::new(BoundedStore::new("0123456789abcdef".repeat(256)));
    let tool = SpillReadTool::new(store.clone(), 16 * 1024);
    let output = read(&tool, context(512), 0, 2048).await.unwrap();
    let result = ToolResult::from_output("call-42", output);

    assert!(result.payload_bytes() <= 512);
    assert_eq!(result.call_id, "call-42");
    assert!(result.structured.is_some());
    assert!(result.content.text().len() < 2048);
    assert!(store.requests().iter().all(|limit| *limit <= 512));

    let next_offset = result.structured.as_ref().unwrap()["next_offset"]
        .as_u64()
        .unwrap() as usize;
    assert!(next_offset > 0);
    let next = read(&tool, context(512), next_offset, 2048).await.unwrap();
    let next_result = ToolResult::from_output("call-42", next);
    assert!(next_result.payload_bytes() <= 512);
    assert_eq!(
        next_result.structured.as_ref().unwrap()["offset"],
        next_offset
    );
}

#[tokio::test]
async fn utf8_page_and_complete_metadata_fit_the_run_budget() {
    let root = tempdir().unwrap();
    let mut config = spill::SpillConfig::default();
    config.max_page_bytes = 16 * 1024;
    let store = Arc::new(LocalSpillStore::new(root.path(), config).unwrap());
    let record = store
        .put("run-a", "call-42", &"界".repeat(700))
        .await
        .unwrap();
    let tool = SpillReadTool::new(store, 16 * 1024);
    let ctx = context(128);
    let output = tool
        .execute(
            api::ToolContext {
                run: ctx.clone(),
                call_id: "call-42".into(),
                progress: Arc::new(NoProgress),
            },
            json!({"id": record.id, "limit": 2048}),
        )
        .await
        .unwrap();
    let text = match &output.content {
        Content::Text(text) => text,
        Content::Blocks(_) => panic!("spill_read must return text"),
    };
    assert!(text.chars().all(|character| character == '界'));
    let result = ToolResult::from_output("call-42", output);
    assert!(result.payload_bytes() <= ctx.limits.max_tool_result_bytes);
    assert!(result.structured.is_some());
}

#[tokio::test]
async fn too_small_budget_returns_a_plain_tool_error() {
    let store = Arc::new(BoundedStore::new("0123456789"));
    let tool = SpillReadTool::new(store, 16 * 1024);
    let output = read(&tool, context(64), 0, 2048).await.unwrap();

    assert!(output.is_error);
    assert!(output.structured.is_none());
    assert!(output.content.text().contains("run result limit"));
}

#[tokio::test]
async fn empty_eof_page_still_budgets_metadata() {
    let store = Arc::new(BoundedStore::new(""));
    let tool = SpillReadTool::new(store, 16 * 1024);
    let output = read(&tool, context(64), 0, 2048).await.unwrap();
    assert!(output.is_error);
    assert!(output.structured.is_none());
}
