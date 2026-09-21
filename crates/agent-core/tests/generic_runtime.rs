mod support;
use agent_api::*;
use agent_core::HostBuilder;
use serde_json::json;
use std::{collections::BTreeSet, sync::{Arc, atomic::Ordering}, time::Duration};
use support::*;

struct Only(Vec<&'static str>);
#[async_trait]
impl ToolSelector for Only {
    async fn select(&self, _: &RunContext, _: usize, _: &[Message], _: &[ToolSpec]) -> Result<Vec<String>> {
        Ok(self.0.iter().map(|s| (*s).to_owned()).collect())
    }
}
struct RoundTools;
#[async_trait]
impl ToolSelector for RoundTools {
    async fn select(&self, _: &RunContext, step: usize, _: &[Message], available: &[ToolSpec]) -> Result<Vec<String>> {
        let name = if step == 1 { "lookup" } else { "count" };
        Ok(available.iter().filter(|s| s.name == name).map(|s| s.name.clone()).collect())
    }
}

#[tokio::test]
async fn tool_view_is_recomputed_each_round_without_reregistering() {
    let model = ScriptModel::new(vec![calls(&[("a", "lookup", json!({"value":1}))]),
        call("b"), answer("done")]);
    let mut host = HostBuilder::new().model(model.clone())
        .tool(Arc::new(CountTool { name: "lookup", ..Default::default() }))
        .tool(Arc::new(CountTool::default())).tool_selector(Arc::new(RoundTools)).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests[0].tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["lookup"]);
        assert_eq!(requests[1].tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["count"]);
    }
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn per_run_ceiling_cannot_be_expanded_by_a_selector() {
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).tool(Arc::new(CountTool::default()))
        .tool(Arc::new(CountTool { name: "hidden", ..Default::default() }))
        .tool_selector(Arc::new(Only(vec!["hidden"]))).build().await.unwrap();
    let mut request = RunRequest::new("go");
    request.allowed_tools = Some(["count".to_owned()].into_iter().collect());
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Plugin);
    assert!(model.requests.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn later_selector_cannot_readd_an_earlier_removed_tool() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![]))
        .tool(Arc::new(CountTool::default()))
        .tool_selector(Arc::new(Only(vec![]))).tool_selector(Arc::new(Only(vec!["count"])))
        .build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Plugin);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn hidden_tool_hallucination_never_dispatches() {
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a")])).tool(tool.clone())
        .tool_selector(Arc::new(Only(vec![]))).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::ModelProtocol);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    assert!(results(&report).is_empty());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn empty_run_ceiling_is_not_the_all_tools_default() {
    let model = ScriptModel::new(vec![answer("no tools")]);
    let mut host = HostBuilder::new().model(model.clone()).tool(Arc::new(CountTool::default())).build().await.unwrap();
    let mut request = RunRequest::new("go"); request.allowed_tools = Some(BTreeSet::new());
    assert_eq!(host.engine().execute(request).await.unwrap().status, RunStatus::Completed);
    assert!(model.requests.lock().unwrap()[0].tools.is_empty());
    let mut invalid = RunRequest::new("go"); invalid.allowed_tools = Some(["unknown".to_owned()].into_iter().collect());
    assert!(host.engine().start(invalid).is_err());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn selected_tool_still_requires_final_host_authority() {
    let tool = Arc::new(CountTool { effects: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a"), answer("denied")])).tool(tool.clone())
        .tool_selector(Arc::new(Only(vec!["count"]))).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(results(&report)[0].status, ToolStatus::Denied);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    host.shutdown().await.unwrap();
}

struct SlowSelector;
#[async_trait]
impl ToolSelector for SlowSelector {
    async fn select(&self, ctx: &RunContext, _: usize, _: &[Message], _: &[ToolSpec]) -> Result<Vec<String>> {
        ctx.cancel.cancelled().await;
        Err(AgentError::new(ErrorCode::Cancelled, "stopped"))
    }
}
#[tokio::test]
async fn tool_selection_obeys_hook_timeout() {
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new().model(model.clone()).tool_selector(Arc::new(SlowSelector)).build().await.unwrap();
    let mut request = RunRequest::new("go"); request.limits.hook_timeout = Duration::from_millis(10);
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::TimedOut);
    assert!(model.requests.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}

fn image() -> ContentBlock {
    ContentBlock::Image { media_type: "image/png".into(), source: ImageSource::Base64 { data: "AQ==".into() } }
}
struct RichTool { output: ToolOutput }
#[async_trait]
impl Tool for RichTool {
    fn spec(&self) -> ToolSpec { CountTool::default().spec() }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> { Ok(self.output.clone()) }
}

#[tokio::test]
async fn image_input_and_rich_tool_result_survive_the_model_boundary() {
    let content = Content::Blocks(vec![ContentBlock::Text { text: "look".into() }, image()]);
    let mut output = ToolOutput::new(content.clone()); output.structured = Some(json!({"private_object_count":2}));
    let model = ScriptModel::new(vec![call("a"), answer("two objects")]);
    let mut host = HostBuilder::new().model(model.clone()).tool(Arc::new(RichTool { output })).build().await.unwrap();
    let handle = host.engine().start(RunRequest::new(content.clone())).unwrap();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(results(&report)[0].content, content);
    assert_eq!(results(&report)[0].structured, Some(json!({"private_object_count":2})));
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests[0].messages[0], Message::user(content.clone()));
        assert!(matches!(requests[1].messages.last(), Some(Message::Tool { result }) if result.content.has_media()));
    }
    let ui = serde_json::to_string(&handle.snapshot()).unwrap();
    assert!(!ui.contains("AQ=="));
    assert!(!ui.contains("private_object_count")); // Final text contains words, not the structured key.
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_tool_error_is_not_relabelled_success() {
    let mut output = ToolOutput::error("not found"); output.structured = Some(json!({"code":404}));
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("a"), answer("no result")]))
        .tool(Arc::new(RichTool { output })).build().await.unwrap();
    let report = host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(results(&report)[0].status, ToolStatus::Error);
    assert_eq!(results(&report)[0].structured, Some(json!({"code":404})));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_rich_output_fails_explicitly_without_sending_broken_media() {
    let output = ToolOutput::new(Content::Blocks(vec![ContentBlock::Image { media_type: "image/png".into(),
        source: ImageSource::Base64 { data: "AAAA".repeat(256) } }]));
    let model = ScriptModel::new(vec![call("a")]);
    let mut host = HostBuilder::new().model(model.clone()).tool(Arc::new(RichTool { output })).build().await.unwrap();
    let mut request = RunRequest::new("go"); request.limits.max_tool_result_bytes = 256;
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert!(results(&report)[0].truncated);
    assert!(!results(&report)[0].content.has_media());
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn opaque_reply_and_tool_data_are_replayed_but_never_ui_events() {
    let assistant_data = ProviderData { namespace: "test".into(), value: json!({"signed_blocks":["PRIVATE_SIGNATURE"]}) };
    let tool_data = ProviderData { namespace: "test".into(), value: json!({"call_signature":"PRIVATE_CALL"}) };
    let mut events = call("a");
    events.insert(1, ModelEvent::ProviderData { target: ProtocolTarget::Assistant, data: assistant_data.clone() });
    events.insert(2, ModelEvent::ProviderData { target: ProtocolTarget::ToolCall { index: 0 }, data: tool_data.clone() });
    let model = ScriptModel::new(vec![events, answer("done")]);
    let mut host = HostBuilder::new().model(model.clone()).tool(Arc::new(CountTool::default())).build().await.unwrap();
    let mut handle = host.engine().start(RunRequest::new("go")).unwrap();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    {
        let requests = model.requests.lock().unwrap();
        assert!(matches!(&requests[1].messages[1], Message::Assistant { provider_data: Some(data), tool_calls, .. }
            if data == &assistant_data && tool_calls[0].provider_data.as_ref() == Some(&tool_data)));
    }
    let mut ui = serde_json::to_string(&handle.snapshot()).unwrap();
    while let Ok(event) = handle.events.try_recv() { ui.push_str(&serde_json::to_string(&event).unwrap()); }
    assert!(!ui.contains("PRIVATE_SIGNATURE")); assert!(!ui.contains("PRIVATE_CALL"));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn per_run_model_options_reach_adapter_and_request_audit() {
    let model = ScriptModel::new(vec![answer("ok")]);
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let mut request = RunRequest::new("go");
    request.model_options = ModelOptions { model: Some("configured-model".into()), temperature: Some(0.2), ..Default::default() };
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.model_requests[0].request.options.model.as_deref(), Some("configured-model"));
    assert_eq!(model.requests.lock().unwrap()[0].options.temperature, Some(0.2));
    host.shutdown().await.unwrap();
}

struct Auxiliary;
#[async_trait]
impl ContextTransform for Auxiliary {
    async fn transform(&self, ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        ctx.model.complete(ModelRequest { messages: vec![Message::user("evaluate")], tools: vec![],
            max_output_tokens: 16, options: ModelOptions::default() }, None).await?;
        Ok(messages)
    }
}
#[tokio::test]
async fn auxiliary_model_calls_inherit_options_and_share_budget() {
    let model = ScriptModel::new(vec![answer("evaluated"), answer("done")]);
    let mut host = HostBuilder::new().model(model.clone()).context_transform(Arc::new(Auxiliary)).build().await.unwrap();
    let mut request = RunRequest::new("go"); request.model_options.temperature = Some(0.3);
    request.task = TaskControl::new(TaskLimits { max_model_calls: 2, ..Default::default() });
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.task_usage.model_calls, 2);
    assert!(model.requests.lock().unwrap().iter().all(|r| r.options.temperature == Some(0.3)));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn planner_forwards_host_selected_model_options_and_tool_ceiling() {
    let model = ScriptModel::new(vec![answer(r#"{"steps":["do work"]}"#), answer("done")]);
    let mut host = HostBuilder::new().model(model.clone()).tool(Arc::new(CountTool::default())).build().await.unwrap();
    let mut request = agent_planner::PlanRequest::new("work");
    request.allowed_tools = Some(BTreeSet::new());
    request.model_options.temperature = Some(0.1);
    let report = agent_planner::Planner::default().plan_and_execute(&host.engine(), request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert!(model.requests.lock().unwrap().iter().all(|r| r.tools.is_empty() && r.options.temperature == Some(0.1)));
    host.shutdown().await.unwrap();
}
