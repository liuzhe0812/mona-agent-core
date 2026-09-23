#![cfg(feature = "compaction")]
//! Exercises extension composition without Application, HTTP, Server or Web.
use api::*;
use compaction::{CompactionConfig, CompactionPlugin, CompactionState};
use sessions::{Prepared, SessionSink, Store};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Default)]
struct ModelFixture {
    primary: AtomicUsize,
    summaries: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl Model for ModelFixture {
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> api::Result<ModelStream> {
        let summary = request
            .messages
            .first()
            .is_some_and(|m| m.text().starts_with("Summarize the earlier"));
        let events = if summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            vec![
                ModelEvent::Text(
                    serde_json::to_string(&compaction::TaskSummary { goal: "Earlier settled work is complete. Preserve the user's fact 27182.".into(), ..Default::default() }).unwrap(),
                ),
                ModelEvent::Finish(FinishReason::Stop),
                ModelEvent::End,
            ]
        } else {
            let n = self.primary.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(request);
            if n == 0 {
                vec![
                    ModelEvent::ToolDelta {
                        index: 0,
                        id: Some("nested-read".into()),
                        name: Some("read".into()),
                        arguments: serde_json::json!({"path":"nested/data.txt"}).to_string(),
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
            }
        };
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
async fn assemble(
    store: Arc<Store>,
    workspace: &Path,
    model: Arc<ModelFixture>,
) -> (runtime::Host, Arc<dyn AgentRuntime>) {
    let compaction = CompactionPlugin::new(CompactionConfig {
        recent_groups: 1,
        ..Default::default()
    })
    .unwrap();
    store.attach_compactor(compaction.compactor()).unwrap();
    let history_store = store.clone();
    let rules = instructions::ProjectInstructions::new(workspace)
        .unwrap()
        .with_history(Arc::new(move |ctx: &RunContext| {
            history_store
                .source_history(
                    ctx.metadata
                        .get(sessions::SESSION_KEY)
                        .expect("bound session"),
                )
                .map_err(|_| AgentError::new(ErrorCode::Checkpoint, "cannot load rule scopes"))
        }));
    let roots = skills::discovery_roots(workspace, None, None).unwrap();
    let tools = tools::ToolConfig::new(workspace, "unused-shell");
    let host = runtime::HostBuilder::new()
        .model(model)
        .plugin(Arc::new(rules.plugin()))
        .plugin(Arc::new(
            skills::SkillsPlugin::local(roots, Default::default()).unwrap(),
        ))
        .plugin(Arc::new(compaction))
        .tool(Arc::new(tools::ReadTool::new(tools)))
        .checkpoint_sink(Arc::new(SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let executor = sessions::runtime(Arc::new(host.engine()), store);
    (host, executor)
}
async fn run(
    store: &Store,
    executor: &dyn AgentExecutor,
    id: &str,
    key: &str,
    prompt: &str,
) -> Arc<RunReport> {
    let mut request = RunRequest::new(prompt);
    request.limits.max_context_bytes = 12000;
    request.limits.max_output_tokens = 64;
    let revision = store.get(id).unwrap().header.revision;
    let Prepared::New { mut history } = store
        .prepare(
            id,
            revision,
            key,
            prompt,
            request.limits.max_initial_history_bytes,
        )
        .unwrap()
    else {
        panic!("new turn expected")
    };
    history.push(Message::user(prompt));
    request.messages = history;
    request.metadata = BTreeMap::from([
        (sessions::SESSION_KEY.into(), id.into()),
        (sessions::TURN_KEY.into(), key.into()),
    ]);
    let report = executor.execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    report
}
#[tokio::test]
async fn saved_workset_and_rule_scopes_reopen_with_skills_without_a_web_host() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path();
    std::fs::create_dir(workspace.join("nested")).unwrap();
    std::fs::create_dir_all(workspace.join(".agents/skills/check-context")).unwrap();
    std::fs::write(workspace.join("AGENTS.md"), "ROOT_RULE").unwrap();
    std::fs::write(workspace.join("nested/AGENTS.md"), "NESTED_ALPHA").unwrap();
    std::fs::write(workspace.join("nested/data.txt"), "27182").unwrap();
    std::fs::write(
        workspace.join(".agents/skills/check-context/SKILL.md"),
        "---\nname: check-context\ndescription: Use context carefully.\n---\nPRIVATE_SKILL_BODY",
    )
    .unwrap();
    let base = workspace.join("state");
    let model = Arc::new(ModelFixture::default());
    let first_prompt = format!("remember 27182 {}", "x".repeat(5000));
    let id = {
        let store = Store::open(&base, workspace).unwrap();
        let id = store.create("composed").unwrap().id;
        let (mut host, executor) = assemble(store.clone(), workspace, model.clone()).await;
        run(&store, executor.as_ref(), &id, "one", &first_prompt).await;
        run(
            &store,
            executor.as_ref(),
            &id,
            "two",
            &format!("continue {}", "y".repeat(6000)),
        )
        .await;
        let doc = store.get(&id).unwrap();
        let state: CompactionState =
            serde_json::from_value(doc.body.compaction.clone().expect("summary was saved"))
                .unwrap();
        let working = state
            .project(&doc.body.checkpoint.as_ref().unwrap().transcript)
            .unwrap();
        assert!(!working.iter().any(|m| matches!(m, Message::Assistant { tool_calls, .. } if tool_calls.iter().any(|c| c.id == "nested-read"))));
        assert!(doc
            .history()
            .unwrap()
            .iter()
            .any(|m| m.text() == first_prompt));
        assert_eq!(model.summaries.load(Ordering::SeqCst), 1);
        host.shutdown().await.unwrap();
        id
    };
    std::fs::write(
        workspace.join("nested/AGENTS.md"),
        "NESTED_BETA_AFTER_REOPEN",
    )
    .unwrap();
    let store = Store::open(&base, workspace).unwrap();
    let (mut host, executor) = assemble(store.clone(), workspace, model.clone()).await;
    run(
        &store,
        executor.as_ref(),
        &id,
        "three",
        "continue after reopen",
    )
    .await;
    assert_eq!(model.summaries.load(Ordering::SeqCst), 1);
    let requests = model.requests.lock().unwrap();
    let restored = serde_json::to_string(requests.last().unwrap()).unwrap();
    assert!(restored.contains("NESTED_BETA_AFTER_REOPEN"));
    assert!(!restored.contains("NESTED_ALPHA"));
    assert!(restored.contains("check-context"));
    assert!(!restored.contains("PRIVATE_SKILL_BODY"));
    assert_eq!(
        requests
            .last()
            .unwrap()
            .messages
            .iter()
            .filter(|m| m
                .text()
                .contains("Host context source: project.instructions"))
            .count(),
        1
    );
    drop(requests);
    host.shutdown().await.unwrap();
}
