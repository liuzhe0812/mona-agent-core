use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct NoModel;
#[async_trait]
impl ModelCaller for NoModel {
    async fn complete(&self, _: ModelRequest, _: Option<Arc<dyn ModelSink>>) -> Result<ModelReply> {
        panic!("no model call expected")
    }
}
fn context() -> RunContext {
    RunContext {
        run_id: "rules-test".into(),
        task: TaskControl::default(),
        cancel: CancellationToken::new(),
        model: Arc::new(NoModel),
        services: Services::default(),
        metadata: Arc::default(),
        model_options: ModelOptions::default(),
        limits: RunLimits::default(),
        request_overhead_bytes: 0,
        request_tools: Arc::default(),
        context_sources: Arc::default(),
        model_context_window_tokens: None,
        tools_enabled: true,
        allowed_tools: None,
    }
}
fn spec(side_effects: bool) -> ToolSpec {
    ToolSpec {
        name: "write".into(),
        description: "test write".into(),
        parameters: json!({"type":"object"}),
        concurrency: ToolConcurrency::Exclusive,
        side_effects,
    }
}
#[tokio::test]
async fn outside_history_path_does_not_block_project_instructions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let outside_file = temp.path().join("inaccessible-parent");
    fs::write(&outside_file, "not a directory").unwrap();
    fs::write(root.join("AGENTS.md"), "ROOT_RULE").unwrap();
    let rules = ProjectInstructions::new(&root).unwrap();
    let history = Message::Assistant {
        content: String::new(),
        tool_calls: vec![ToolCall::new(
            "outside",
            "write",
            json!({"path": outside_file.join("file.txt")}),
        )],
        reasoning_content: None,
        provider_data: None,
    };
    let blocks = rules.sources(&context(), &[history]).await.unwrap();
    assert!(blocks[0].content.contains("ROOT_RULE"));
    assert_eq!(
        rules
            .directory(root.join("new.txt").to_str().unwrap())
            .unwrap(),
        Some(rules.root.clone())
    );
}
#[tokio::test]
async fn scopes_refresh_without_elevating_rules_or_mistaking_them_for_user_turns() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("nested")).unwrap();
    fs::create_dir(root.join("unrelated")).unwrap();
    fs::write(root.join("AGENTS.md"), "ROOT_ALPHA").unwrap();
    fs::write(root.join("CLAUDE.md"), "ROOT_FALLBACK_NOT_SELECTED").unwrap();
    fs::write(root.join("nested/CLAUDE.md"), "NESTED_BETA").unwrap();
    fs::write(root.join("unrelated/AGENTS.md"), "UNRELATED_SECRET").unwrap();
    let rules = ProjectInstructions::new(root).unwrap();
    let ctx = context();
    let messages = vec![Message::user("actual request")];
    let first = rules.sources(&ctx, &messages).await.unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].content.contains("ROOT_ALPHA"));
    assert!(!first[0].content.contains("FALLBACK_NOT_SELECTED"));
    assert!(!first[0].content.contains("NESTED_BETA"));
    assert_eq!(
        rules.transform(&ctx, messages.clone()).await.unwrap(),
        messages
    );
    let call = ToolCall::new("w1", "write", json!({"path":"nested/new.txt"}));
    assert!(matches!(
        rules.check(&ctx, &call, &spec(true)).await.unwrap(),
        PolicyDecision::Deny(_)
    ));
    let second = rules.sources(&ctx, &messages).await.unwrap();
    assert!(second[0].content.contains("NESTED_BETA"));
    assert!(!second[0].content.contains("UNRELATED_SECRET"));
    assert!(matches!(
        rules.check(&ctx, &call, &spec(true)).await.unwrap(),
        PolicyDecision::Allow
    ));
    fs::write(root.join("nested/AGENTS.md"), "NESTED_CHANGED").unwrap();
    assert!(matches!(
        rules.check(&ctx, &call, &spec(true)).await.unwrap(),
        PolicyDecision::Deny(_)
    ));
    let refreshed = rules.sources(&ctx, &messages).await.unwrap();
    assert!(refreshed[0].content.contains("NESTED_CHANGED"));
    assert!(!refreshed[0].content.contains("NESTED_BETA"));
    fs::remove_file(root.join("nested/AGENTS.md")).unwrap();
    fs::remove_file(root.join("nested/CLAUDE.md")).unwrap();
    let refreshed = rules.sources(&ctx, &messages).await.unwrap();
    assert!(!refreshed[0].content.contains("NESTED_"));
    rules.finish(&ctx.run_id).await.unwrap();
    assert!(rules.runs.lock().unwrap().is_empty());
}
#[tokio::test]
async fn corrupt_oversized_and_cancelled_rules_fail_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    let rules = ProjectInstructions::new(temp.path()).unwrap();
    let ctx = context();
    fs::write(temp.path().join("AGENTS.md"), [0xff, 0xfe]).unwrap();
    assert!(rules.sources(&ctx, &[Message::user("go")]).await.is_err());
    fs::write(temp.path().join("AGENTS.md"), "x".repeat(MAX_BYTES + 1)).unwrap();
    assert!(rules.sources(&ctx, &[Message::user("go")]).await.is_err());
    fs::write(temp.path().join("AGENTS.md"), "short").unwrap();
    ctx.cancel.cancel();
    assert!(rules.sources(&ctx, &[Message::user("go")]).await.is_err());
}
struct ChangingRulesModel {
    calls: AtomicUsize,
    root: PathBuf,
}
#[async_trait]
impl Model for ChangingRulesModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let round = self.calls.fetch_add(1, Ordering::SeqCst);
        let call = |index, id: &str, path: &str, content: &str| ModelEvent::ToolDelta {
            index,
            id: Some(id.into()),
            name: Some("write".into()),
            arguments: json!({"path":path,"content":content}).to_string(),
        };
        let events = match round {
            0 => vec![
                call(
                    0,
                    "new-rules",
                    "AGENTS.md",
                    "RULE_UPDATED: reconsider target.txt.",
                ),
                call(1, "stale-target", "target.txt", "must not be written"),
                ModelEvent::Finish(FinishReason::ToolCalls),
                ModelEvent::End,
            ],
            1 => {
                assert!(
                    !self.root.join("target.txt").exists(),
                    "stale same-batch action was dispatched"
                );
                assert!(serde_json::to_string(&request.messages)
                    .unwrap()
                    .contains("RULE_UPDATED"));
                assert!(request
                    .messages
                    .iter()
                    .any(|m| matches!(m, Message::Tool { result }
                    if result.call_id == "stale-target" && result.status == ToolStatus::Denied)));
                vec![
                    call(0, "fresh-target", "target.txt", "reconsidered"),
                    ModelEvent::Finish(FinishReason::ToolCalls),
                    ModelEvent::End,
                ]
            }
            2 => vec![
                ModelEvent::Text("done".into()),
                ModelEvent::Finish(FinishReason::Stop),
                ModelEvent::End,
            ],
            _ => panic!("unexpected repeated model call"),
        };
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
struct SavePolicyResults(Arc<StdMutex<Vec<RunCheckpoint>>>);
use std::sync::Mutex as StdMutex;
#[async_trait]
impl CheckpointSink for SavePolicyResults {
    async fn commit(&self, cp: Arc<RunCheckpoint>, _: CancellationToken) -> Result<()> {
        self.0.lock().unwrap().push((*cp).clone());
        Ok(())
    }
}
#[tokio::test]
async fn same_batch_rule_change_denies_stale_write_then_allows_fresh_call_with_durable_results() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("AGENTS.md"), "INITIAL_RULE").unwrap();
    let rules = Arc::new(ProjectInstructions::new(temp.path()).unwrap());
    let checkpoints = Arc::new(StdMutex::new(Vec::new()));
    let config = tools::ToolConfig::new(temp.path(), "unused-shell");
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(ChangingRulesModel {
            calls: AtomicUsize::new(0),
            root: temp.path().into(),
        }))
        .context_transform(rules.clone())
        .policy(rules)
        .tool(Arc::new(tools::WriteTool::new(config)))
        .allow_side_effect_tool("write")
        .checkpoint_sink(Arc::new(SavePolicyResults(checkpoints.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let handle = host
        .engine()
        .start(RunRequest::new("change rules and reconsider target"))
        .unwrap();
    let mut events = handle.subscribe();
    let report = handle.wait().await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(
        fs::read_to_string(temp.path().join("target.txt")).unwrap(),
        "reconsidered"
    );
    assert_eq!(
        report
            .transcript
            .iter()
            .filter(|m| matches!(m, Message::Tool { result }
        if result.call_id == "new-rules" && result.status == ToolStatus::Success))
            .count(),
        1
    );
    assert!(checkpoints
        .lock()
        .unwrap()
        .iter()
        .any(|cp| matches!(cp.pending_tools.get("stale-target"),
        Some(CheckpointToolState::Settled { result }) if result.status == ToolStatus::Denied)));
    let mut ended = 0;
    while let Ok(event) = events.try_recv() {
        if let RunEvent::ItemCompleted { item } = event.event {
            if matches!(item.content, ItemContent::ToolCall { call_id:Some(ref id), .. } if id == "stale-target")
            {
                assert_eq!(item.state, ItemState::Denied);
                ended += 1;
            }
        }
    }
    assert_eq!(ended, 1);
    host.shutdown().await.unwrap();
}

struct WriteOnce {
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Tool for WriteOnce {
    fn spec(&self) -> ToolSpec {
        spec(true)
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("written once".into())
    }
}
struct RulesModel {
    calls: AtomicUsize,
}
#[async_trait]
impl Model for RulesModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let turn = self.calls.fetch_add(1, Ordering::SeqCst);
        let context = serde_json::to_string(&request.messages).unwrap();
        if turn == 0 {
            assert!(!context.contains("NESTED_REQUIRED"));
        } else {
            assert!(context.contains("NESTED_REQUIRED"));
        }
        let events = if turn < 2 {
            vec![
                ModelEvent::ToolDelta {
                    index: 0,
                    id: Some(format!("attempt-{turn}")),
                    name: Some("write".into()),
                    arguments: r#"{"path":"nested/new.txt"}"#.into(),
                },
                ModelEvent::Finish(FinishReason::ToolCalls),
                ModelEvent::End,
            ]
        } else {
            vec![
                ModelEvent::Text("done".into()),
                ModelEvent::Finish(FinishReason::Stop),
                ModelEvent::End,
            ]
        };
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
#[tokio::test]
async fn runtime_replans_unseen_nested_rules_before_any_write_body_runs() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("nested")).unwrap();
    fs::write(temp.path().join("nested/AGENTS.md"), "NESTED_REQUIRED").unwrap();
    let rules = Arc::new(ProjectInstructions::new(temp.path()).unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(RulesModel {
            calls: AtomicUsize::new(0),
        }))
        .plugin(Arc::new(rules.as_ref().clone().plugin()))
        .tool(Arc::new(WriteOnce {
            calls: calls.clone(),
        }))
        .allow_side_effect_tool("write")
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("write the file"))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(report
        .transcript
        .iter()
        .any(|m| matches!(m, Message::Tool { result } if result.status == ToolStatus::Denied)));
    assert!(!report
        .transcript
        .iter()
        .any(|m| m.text().starts_with("[Host context source:")));
    assert!(rules.runs.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}
