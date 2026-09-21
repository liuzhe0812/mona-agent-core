mod support;
use agent_api::*;
use agent_core::HostBuilder;
use std::sync::{Arc, atomic::Ordering};
use support::*;

#[tokio::test]
async fn only_api_trait_is_needed_for_stream_cancel_and_result() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![answer("hello")])).build().await.unwrap();
    let runtime: Arc<dyn AgentRuntime> = Arc::new(host.engine());
    let mut run = runtime.start(RunRequest::new("test")).unwrap();
    let report = run.wait().await.unwrap();
    let snapshot = run.snapshot();
    assert_eq!(snapshot.outcome.unwrap().status, report.status);
    let first = run.events.try_recv().unwrap(); assert_eq!(first.seq, 1);
    assert!(matches!(first.event, RunEvent::RunStarted));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn tool_deltas_and_full_terminal_tool_result_share_one_item() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("c1"), answer("done")]))
        .tool(Arc::new(CountTool::default())).event_capacity(1024).build().await.unwrap();
    let mut run = host.engine().start(RunRequest::new("test")).unwrap();
    run.wait().await.unwrap();
    let mut projection = RunSnapshot::new(&run.run_id);
    let mut arguments = false; let mut output = false; let mut completed = false;
    while let Ok(event) = run.events.try_recv() {
        match &event.event {
            RunEvent::ToolArgumentsDelta { item_id, .. } => { arguments = true; assert_eq!(item_id, "step-1-tool-0"); }
            RunEvent::ToolOutputDelta { item_id, .. } => { output = true; assert_eq!(item_id, "step-1-tool-0"); }
            RunEvent::ItemCompleted { item } if item.id == "step-1-tool-0" => {
                completed = true;
                assert!(matches!(&item.content, ItemContent::ToolCall { result: Some(result), arguments: Some(_), .. } if result.status == ToolStatus::Success));
            }
            _ => {},
        }
        assert!(projection.apply(&event));
    }
    assert!(arguments && output && completed);
    assert_eq!(projection.seq, run.snapshot().seq);
    assert!(projection.outcome.is_some()); host.shutdown().await.unwrap();
}
#[tokio::test]
async fn denied_tool_has_terminal_item_without_running() {
    let tool = Arc::new(CountTool { effects: true, ..Default::default() });
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("c1"), answer("denied")])).tool(tool.clone()).build().await.unwrap();
    let run = host.engine().start(RunRequest::new("test")).unwrap(); run.wait().await.unwrap();
    let snapshot = run.snapshot();
    assert_eq!(snapshot.items.iter().find(|i| i.id == "step-1-tool-0").unwrap().state, ItemState::Denied);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0); host.shutdown().await.unwrap();
}
#[tokio::test]
async fn partial_tool_arguments_remain_preview_and_end_skipped() {
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new().tool(tool.clone()).model(ScriptModel::new(vec![vec![
        ModelEvent::ToolDelta { index: 0, id: Some("c1".into()), name: Some("count".into()), arguments: "{".into() },
        ModelEvent::Finish(FinishReason::Length), ModelEvent::End,
    ]])).build().await.unwrap();
    let run = host.engine().start(RunRequest::new("test")).unwrap(); run.wait().await.unwrap();
    let snapshot = run.snapshot();
    assert!(snapshot.items.iter().all(|i| i.state.terminal()));
    assert_eq!(snapshot.items.iter().find(|i| i.id == "step-1-tool-0").unwrap().state, ItemState::Skipped);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0); host.shutdown().await.unwrap();
}
#[tokio::test]
async fn slow_core_subscriber_recovers_from_atomic_snapshot() {
    let mut events = (0..300).map(|_| ModelEvent::Text("字".into())).collect::<Vec<_>>();
    events.extend([ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]);
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![events])).event_capacity(2).build().await.unwrap();
    let mut run = host.engine().start(RunRequest::new("test")).unwrap(); run.wait().await.unwrap();
    assert!(matches!(run.events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))));
    let snapshot = run.snapshot(); assert!(snapshot.outcome.is_some());
    assert!(matches!(&snapshot.items[0].content, ItemContent::AgentMessage { text, .. } if text == &"字".repeat(300)));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn hidden_reasoning_is_never_broadcast_as_visible_text() {
    let mut events = answer("public"); events.insert(0, ModelEvent::Reasoning("private-provider-protocol".into()));
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![events])).build().await.unwrap();
    let mut run = host.engine().start(RunRequest::new("test")).unwrap(); run.wait().await.unwrap();
    while let Ok(event) = run.events.try_recv() { assert!(!serde_json::to_string(&event).unwrap().contains("private-provider-protocol")); }
    assert!(!serde_json::to_string(&run.snapshot()).unwrap().contains("private-provider-protocol"));
    host.shutdown().await.unwrap();
}

struct DetailTool;
#[async_trait]
impl Tool for DetailTool {
    fn spec(&self) -> ToolSpec { CountTool::default().spec() }
    async fn execute(&self, ctx: ToolContext, _args: serde_json::Value) -> Result<ToolOutput> {
        assert!(ctx.progress.set_detail("unsafe-key", serde_json::json!({})).is_err());
        assert!(ctx.progress.set_detail("coding.large", serde_json::json!("x".repeat(UI_DETAIL_BYTES + 1))).is_err());
        ctx.progress.set_detail("coding.diff", serde_json::json!({"path":"example.rs","added":2}))?;
        ctx.progress.report("a line of output"); Ok("result".into())
    }
}
#[tokio::test]
async fn structured_tool_details_survive_authoritative_completion() {
    let mut host = HostBuilder::new().model(ScriptModel::new(vec![call("c1"), answer("done")]))
        .tool(Arc::new(DetailTool)).build().await.unwrap();
    let run = host.engine().start(RunRequest::new("details")).unwrap(); run.wait().await.unwrap();
    let snapshot = run.snapshot();
    let item = snapshot.items.iter().find(|i| i.id == "step-1-tool-0").unwrap();
    assert!(matches!(&item.content, ItemContent::ToolCall { details, result: Some(result), .. }
        if details["coding.diff"]["added"] == 2 && result.status == ToolStatus::Success));
    host.shutdown().await.unwrap();
}
