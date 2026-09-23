use api::{
    Content, FinishReason, ModelCaller, ModelReply, ModelRequest, ModelSink, ResultTransform,
    RunContext, RunLimits, Services, TaskControl, Tool, ToolCall, ToolResult, ToolStatus,
};
use async_trait::async_trait;
use serde_json::json;
use spill::{LocalSpillStore, SpillConfig, SpillReadTool, SpillResultTransform, SpillStore};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
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
            tool_calls: vec![],
            reasoning_content: None,
            provider_data: None,
            finish: FinishReason::Stop,
            usage: None,
        })
    }
}

fn context(run_id: &str) -> RunContext {
    RunContext {
        run_id: run_id.into(),
        task: TaskControl::default(),
        cancel: api::CancellationToken::new(),
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
    }
}

#[tokio::test]
async fn archives_and_reads_only_within_the_run() {
    let root = tempdir().unwrap();
    let mut config = SpillConfig::default();
    config.trigger_bytes = 8;
    config.preview_bytes = 80;
    config.max_page_bytes = 8;
    let store = Arc::new(LocalSpillStore::new(root.path(), config.clone()).unwrap());
    let transform = SpillResultTransform::new(store.clone(), config.clone());
    let text = "0123456789abcdefghijklmnopqrstuvwxyz";
    let call = ToolCall::new("call-1", "big_tool", json!({}));
    let result = ToolResult::new(&call.id, ToolStatus::Success, text);
    let transformed = transform
        .transform(&context("run-a"), &call, result)
        .await
        .unwrap();
    assert!(transformed.truncated);
    let artifact = transformed.artifact.clone().unwrap();
    let id = artifact.uri.strip_prefix("spill:").unwrap();
    let mut offset = 0;
    let mut joined = String::new();
    loop {
        let page = store.read_page("run-a", id, offset, 8).await.unwrap();
        joined.push_str(&page.text);
        offset = page.next_offset;
        if page.eof {
            break;
        }
    }
    assert_eq!(joined, text);
    assert!(store.read_page("run-b", id, 0, 8).await.is_err());
}

#[tokio::test]
async fn quota_failure_does_not_publish_an_entry() {
    let root = tempdir().unwrap();
    let mut config = SpillConfig::default();
    config.trigger_bytes = 4;
    config.max_entry_bytes = 8;
    config.max_run_bytes = 8;
    config.max_total_bytes = 8;
    config.preview_bytes = 4;
    config.max_page_bytes = 8;
    let store = Arc::new(LocalSpillStore::new(root.path(), config.clone()).unwrap());
    let transform = SpillResultTransform::new(store.clone(), config);
    let call = ToolCall::new("call-1", "big_tool", json!({}));
    let result = ToolResult::new(&call.id, ToolStatus::Success, "0123456789");
    assert!(transform
        .transform(&context("run-a"), &call, result)
        .await
        .is_err());
    assert!(!root.path().join("72756e2d61").exists());
}

#[tokio::test]
async fn spill_read_tool_is_bounded_and_does_not_recurse() {
    let root = tempdir().unwrap();
    let mut config = SpillConfig::default();
    config.trigger_bytes = 4;
    config.max_page_bytes = 8;
    let store = Arc::new(LocalSpillStore::new(root.path(), config.clone()).unwrap());
    let record = store.put("run-a", "call", "abcdefghijk").await.unwrap();
    let tool = SpillReadTool::new(store, 8);
    let ctx = api::ToolContext {
        run: context("run-a"),
        call_id: "read".into(),
        progress: Arc::new(NoProgress),
    };
    let output = tool
        .execute(ctx, json!({"id":record.id,"limit":8}))
        .await
        .unwrap();
    assert_eq!(output.content, Content::Text("abcdefgh".into()));
    assert!(output.structured.is_some());
    assert!(tool
        .execute(
            api::ToolContext {
                run: context("run-a"),
                call_id: "read".into(),
                progress: Arc::new(NoProgress)
            },
            json!({"id":record.id,"offset":0,"limit":9})
        )
        .await
        .is_err());
}

#[tokio::test]
async fn disabled_read_capability_keeps_the_original_result() {
    let root = tempdir().unwrap();
    let mut config = SpillConfig::default();
    config.trigger_bytes = 4;
    let store = Arc::new(LocalSpillStore::new(root.path(), config.clone()).unwrap());
    let transform = SpillResultTransform::new(store, config);
    let call = ToolCall::new("call-1", "big_tool", json!({}));
    let mut ctx = context("run-a");
    ctx.allowed_tools = Some(Arc::new(BTreeSet::new()));
    let result = ToolResult::new(&call.id, ToolStatus::Success, "0123456789");
    let transformed = transform
        .transform(&ctx, &call, result.clone())
        .await
        .unwrap();
    assert_eq!(transformed, result);
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn cleanup_removes_expired_inactive_entries() {
    let root = tempdir().unwrap();
    let mut config = SpillConfig::default();
    config.retention = Duration::from_millis(1);
    let store = LocalSpillStore::new(root.path(), config).unwrap();
    let record = store.put("run-a", "call", "old").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let report = store.cleanup(&BTreeSet::new()).await.unwrap();
    assert_eq!(report.removed_entries, 1);
    assert!(store.read_page("run-a", &record.id, 0, 8).await.is_err());
}

#[tokio::test]
async fn preview_respects_a_small_run_result_cap() {
    let root = tempdir().unwrap();
    let mut config = SpillConfig::default();
    config.trigger_bytes = 8;
    let store = Arc::new(LocalSpillStore::new(root.path(), config.clone()).unwrap());
    let transform = SpillResultTransform::new(store, config);
    let call = ToolCall::new("call-1", "big_tool", json!({}));
    let mut ctx = context("run-a");
    ctx.limits.max_tool_result_bytes = 128;
    let result = ToolResult::new(&call.id, ToolStatus::Success, "x".repeat(256));
    let transformed = transform.transform(&ctx, &call, result).await.unwrap();
    assert!(transformed.payload_bytes() <= ctx.limits.max_tool_result_bytes);
    assert!(transformed.artifact.is_some());
    assert!(transformed.content.text().contains("spill_read"));
}

#[tokio::test]
async fn structured_metadata_is_preserved_while_only_the_large_text_body_is_archived() {
    let root = tempdir().unwrap();
    let config = SpillConfig::default();
    let store = Arc::new(LocalSpillStore::new(root.path(), config.clone()).unwrap());
    let transform = SpillResultTransform::new(store.clone(), config);
    let call = ToolCall::new("shell-exit", "shell", json!({"command":"test only"}));
    let mut ctx = context("run-structured"); ctx.limits.max_tool_result_bytes = 256;
    let mut result = ToolResult::new(&call.id, ToolStatus::Error, "large body ".repeat(4000));
    result.structured = Some(json!({"exit_code":7,"truncated":false,"output_bytes":44000}));
    let original = result.clone();
    let transformed = transform.transform(&ctx, &call, result).await.unwrap();
    assert_eq!(transformed.status, ToolStatus::Error);
    assert_eq!(transformed.call_id, original.call_id);
    assert_eq!(transformed.structured, original.structured);
    assert!(transformed.payload_bytes() <= ctx.limits.max_tool_result_bytes);
    let id = transformed.artifact.as_ref().unwrap().uri.strip_prefix("spill:").unwrap();
    let page = store.read_page("run-structured", id, 0, 16).await.unwrap();
    assert!(original.content.text().starts_with(&page.text));
    let mut oversized = original; oversized.structured = Some(json!({"too_large":"m".repeat(512)}));
    assert_eq!(transform.transform(&ctx, &call, oversized.clone()).await.unwrap(), oversized);
    transform.finish(&ctx.run_id).await.unwrap();
}

struct NoProgress;
impl api::ToolProgress for NoProgress {
    fn report(&self, _text: &str) {}
}
