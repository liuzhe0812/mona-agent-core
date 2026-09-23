mod support;
use api::*;
use runtime::HostBuilder;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use support::*;

struct Unused;
#[async_trait]
impl Tool for Unused {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "unused".into(),
            description: "x".repeat(16000),
            parameters: json!({"type":"object"}),
            concurrency: ToolConcurrency::ParallelSafe,
            side_effects: false,
        }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        panic!("not selected")
    }
}
struct Select(AtomicUsize);
#[async_trait]
impl ToolSelector for Select {
    async fn select(
        &self,
        _: &RunContext,
        _: usize,
        messages: &[Message],
        available: &[ToolSpec],
    ) -> Result<Vec<String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert!(messages.iter().any(|m| m.text() == "canonical-marker"));
        assert!(!messages.iter().any(|m| m.text().contains("pinned-source")));
        assert!(available.iter().any(|t| t.name == "count"));
        Ok(vec!["count".into()])
    }
}
#[derive(Default)]
struct Projection {
    sources: AtomicUsize,
    recovered: AtomicUsize,
    sizes: Mutex<Vec<usize>>,
}
fn assert_view(ctx: &RunContext) {
    assert_eq!(
        ctx.request_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["count"]
    );
    assert_eq!(
        ctx.allowed_tools
            .as_ref()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["count"]
    );
    assert!(ctx.tools_enabled);
}
#[async_trait]
impl ContextTransform for Projection {
    async fn sources(&self, ctx: &RunContext, _: &[Message]) -> Result<Vec<ContextBlock>> {
        assert_view(ctx);
        self.sources.fetch_add(1, Ordering::SeqCst);
        Ok(vec![ContextBlock::new(
            "pinned-source",
            "unchanged-guidance",
        )])
    }
    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        assert_view(ctx);
        let exact = serde_json::to_vec(&ctx.model_request(messages.clone()))
            .unwrap()
            .len();
        let measured = ctx.request_overhead_bytes + serde_json::to_vec(&messages).unwrap().len();
        assert_eq!(
            measured, exact,
            "selected schemas and sources must be charged exactly once"
        );
        self.sizes.lock().unwrap().push(exact);
        Ok(messages)
    }
    async fn recover_context(
        &self,
        ctx: &RunContext,
        messages: Vec<Message>,
    ) -> Result<Option<Vec<Message>>> {
        assert_view(ctx);
        assert_eq!(ctx.context_sources.len(), 1);
        self.recovered.fetch_add(1, Ordering::SeqCst);
        Ok(Some(
            messages
                .into_iter()
                .filter(|m| m.text() != "old-history".repeat(100))
                .collect(),
        ))
    }
}
#[tokio::test]
async fn selected_tool_schemas_are_used_before_sources_and_projection() {
    let selector = Arc::new(Select(AtomicUsize::new(0)));
    let projection = Arc::new(Projection::default());
    let model = ScriptModel::new(vec![call("a"), answer("done")]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(CountTool::default()))
        .tool(Arc::new(Unused))
        .tool_selector(selector.clone())
        .context_transform(projection.clone())
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("canonical-marker");
    run.limits.max_context_bytes = 2048;
    let report = host.engine().execute(run).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(selector.0.load(Ordering::SeqCst), 2);
    assert_eq!(projection.sources.load(Ordering::SeqCst), 2);
    let requests = model.requests.lock().unwrap();
    let sizes = projection.sizes.lock().unwrap();
    for (request, size) in requests.iter().zip(sizes.iter()) {
        assert_eq!(request.tools.len(), 1);
        assert_eq!(*size, serde_json::to_vec(request).unwrap().len());
        assert!(*size < 2048);
    }
    drop(requests);
    drop(sizes);
    host.shutdown().await.unwrap();
}
struct RecoveryModel {
    attempts: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl Model for RecoveryModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        self.requests.lock().unwrap().push(request);
        match self.attempts.fetch_add(1, Ordering::SeqCst) {
            0 => Err(AgentError::new(ErrorCode::ModelRateLimit, "temporary")),
            1 => Err(AgentError::new(
                ErrorCode::ModelContextWindow,
                "shrink once",
            )),
            n => Ok(Box::pin(futures_util::stream::iter(
                if n == 2 {
                    call("recovered-call")
                } else {
                    answer("done")
                }
                .into_iter()
                .map(Ok),
            ))),
        }
    }
}
#[tokio::test]
async fn retry_and_context_recovery_reuse_selected_tools_and_collected_sources() {
    let selector = Arc::new(Select(AtomicUsize::new(0)));
    let projection = Arc::new(Projection::default());
    let model = Arc::new(RecoveryModel {
        attempts: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(tool.clone())
        .tool(Arc::new(Unused))
        .tool_selector(selector.clone())
        .context_transform(projection.clone())
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("canonical-marker");
    run.messages
        .insert(0, Message::user("old-history".repeat(100)));
    run.limits.max_context_bytes = 3000;
    run.limits.model_retry = ModelRetryPolicy {
        max_retries: 1,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
    };
    let report = host.engine().execute(run).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(report.task_usage.model_calls, 4);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    assert_eq!(selector.0.load(Ordering::SeqCst), 2);
    assert_eq!(projection.sources.load(Ordering::SeqCst), 2);
    assert_eq!(projection.recovered.load(Ordering::SeqCst), 1);
    let requests = model.requests.lock().unwrap();
    let signature = serde_json::to_vec(&requests[0].tools).unwrap();
    assert!(requests
        .iter()
        .all(|r| serde_json::to_vec(&r.tools).unwrap() == signature));
    assert_eq!(requests[0].messages, requests[1].messages);
    assert!(
        serde_json::to_vec(&requests[2]).unwrap().len()
            < serde_json::to_vec(&requests[1]).unwrap().len()
    );
    assert_eq!(requests[0].messages[0], requests[2].messages[0]);
    drop(requests);
    host.shutdown().await.unwrap();
}

struct NoneSelected;
#[async_trait]
impl ToolSelector for NoneSelected {
    async fn select(
        &self,
        _: &RunContext,
        _: usize,
        _: &[Message],
        _: &[ToolSpec],
    ) -> Result<Vec<String>> {
        Ok(vec![])
    }
}
struct NeedsReader;
#[async_trait]
impl ContextTransform for NeedsReader {
    async fn sources(&self, ctx: &RunContext, _: &[Message]) -> Result<Vec<ContextBlock>> {
        assert!(!ctx.tools_enabled);
        assert!(ctx.request_tools.is_empty());
        assert!(ctx.allowed_tools.as_ref().unwrap().is_empty());
        Ok(vec![])
    }
    async fn transform(&self, _: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        Ok(messages)
    }
}
#[tokio::test]
async fn no_selected_tools_means_no_tool_cost_or_unreadable_source_references() {
    let model = ScriptModel::new(vec![answer("done")]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(Unused))
        .tool_selector(Arc::new(NoneSelected))
        .context_transform(Arc::new(NeedsReader))
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("go");
    run.limits.max_context_bytes = 512;
    assert_eq!(
        host.engine().execute(run).await.unwrap().status,
        RunStatus::Completed
    );
    assert!(model.requests.lock().unwrap()[0].tools.is_empty());
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn absent_selector_still_counts_all_offered_tools_and_rejects_before_model() {
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .tool(Arc::new(Unused))
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("go");
    run.limits.max_context_bytes = 512;
    assert_eq!(
        host.engine().execute(run).await.unwrap().status,
        RunStatus::Limited
    );
    assert!(model.requests.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}
