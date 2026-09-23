#![cfg(feature = "compaction")]
//! Controlled semantic fixtures verify information plumbing, not live-model factual accuracy.
use api::*;
use compaction::{CompactionConfig, CompactionPlugin, TaskSummary};
use sessions::{Prepared, SessionSink, Store, SESSION_KEY, TURN_KEY};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Default)]
struct Fixture {
    primary: AtomicUsize,
    summaries: AtomicUsize,
    handoffs: Mutex<Vec<TaskSummary>>,
}
#[async_trait]
impl Model for Fixture {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let summary = request.messages[0]
            .text()
            .starts_with("Summarize the earlier");
        let text = if summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            let input: serde_json::Value =
                serde_json::from_str(&request.messages[1].text()).unwrap();
            let source = format!("{} {}", input["previous_handoff"], input["settled_history"]);
            assert!(
                source.contains("31415"),
                "critical identifier disappeared before summarizing"
            );
            assert!(
                source.contains("no automatic replay"),
                "prior prohibition disappeared"
            );
            assert!(
                source.contains("effect-1"),
                "completed action evidence disappeared"
            );
            let corrected = source.contains("NO_LEGACY");
            let handoff = TaskSummary {
                goal: "general reusable agent 31415".into(),
                constraints: vec![
                    "no automatic replay".into(),
                    if corrected {
                        "NO_LEGACY".into()
                    } else {
                        "ALLOW_LEGACY".into()
                    },
                ],
                corrections: if corrected {
                    vec!["ALLOW_LEGACY -> NO_LEGACY".into()]
                } else {
                    vec![]
                },
                decisions: vec!["core/extensions/Web".into()],
                completed: vec!["effect-1 succeeded once".into()],
                pending: vec!["verify deployment before proceeding".into()],
                references: vec!["31415".into()],
            };
            self.handoffs.lock().unwrap().push(handoff.clone());
            serde_json::to_string(&handoff).unwrap()
        } else {
            let count = self.primary.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                return Ok(Box::pin(futures_util::stream::iter(vec![
                    Ok(ModelEvent::ToolDelta {
                        index: 0,
                        id: Some("effect-1".into()),
                        name: Some("effect".into()),
                        arguments: "{}".into(),
                    }),
                    Ok(ModelEvent::Finish(FinishReason::ToolCalls)),
                    Ok(ModelEvent::End),
                ])));
            }
            format!(
                "Progress recorded; verify deployment before proceeding. {}",
                "n".repeat(6000)
            )
        };
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(text)),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}
struct Effect(Arc<AtomicUsize>);
#[async_trait]
impl Tool for Effect {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "effect".into(),
            description: "test effect".into(),
            parameters: serde_json::json!({"type":"object"}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("effect-1 succeeded once".into())
    }
}
#[tokio::test]
async fn repeated_compaction_and_reopen_keep_corrections_actions_and_pending_work_without_reexecution(
) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("state");
    let fixture = Arc::new(Fixture::default());
    let effects = Arc::new(AtomicUsize::new(0));
    let mut id = String::new();
    for index in 0..5 {
        let store = Store::open(&base, temp.path()).unwrap();
        if index == 0 {
            id = store.create("long").unwrap().id;
        }
        let plugin = CompactionPlugin::new(CompactionConfig {
            recent_groups: 1,
            ..Default::default()
        })
        .unwrap();
        let compactor = plugin.compactor();
        store.attach_compactor(compactor.clone()).unwrap();
        let mut host = runtime::HostBuilder::new()
            .model(fixture.clone())
            .plugin(Arc::new(plugin))
            .tool(Arc::new(Effect(effects.clone())))
            .allow_side_effect_tool("effect")
            .checkpoint_sink(Arc::new(SessionSink(store.clone())))
            .unwrap()
            .build()
            .await
            .unwrap();
        let executor = sessions::runtime(Arc::new(host.engine()), store.clone());
        let prompt = match index {
            0 => "Build general reusable agent 31415; no automatic replay; ALLOW_LEGACY for now"
                .into(),
            1 => format!(
                "User correction NO_LEGACY replaces ALLOW_LEGACY; keep core/extensions/Web. {}",
                "x".repeat(3500)
            ),
            _ => format!(
                "Continue round {index}; retain all prior corrections. {}",
                "y".repeat(3500)
            ),
        };
        let revision = store.get(&id).unwrap().header.revision;
        let Prepared::New { mut history } = store
            .prepare(
                &id,
                revision,
                &format!("turn-{index}"),
                &prompt,
                4 * 1024 * 1024,
            )
            .unwrap()
        else {
            panic!("new turn");
        };
        history.push(Message::user(&prompt));
        let mut request = RunRequest::new("unused");
        request.messages = history;
        request.limits.max_context_bytes = 12000;
        request.limits.max_output_tokens = 4096;
        request.metadata = BTreeMap::from([
            (SESSION_KEY.into(), id.clone()),
            (TURN_KEY.into(), format!("turn-{index}")),
        ]);
        let report = executor.execute(request).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
        let doc = store.get(&id).unwrap();
        let full = doc.history().unwrap();
        assert_eq!(
            full.iter()
                .filter(|m| matches!(m,Message::Tool {result} if result.call_id=="effect-1"))
                .count(),
            1
        );
        assert!(full.iter().any(|m| m.text() == prompt));
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        if index >= 2 {
            let state: compaction::CompactionState =
                serde_json::from_value(doc.body.compaction.clone().unwrap()).unwrap();
            let summary = TaskSummary::parse(&state.summary).unwrap();
            assert!(summary.constraints.contains(&"NO_LEGACY".into()));
            assert!(!summary.constraints.contains(&"ALLOW_LEGACY".into()));
            assert!(!summary.corrections.is_empty());
            assert!(summary.completed.iter().any(|s| s.contains("effect-1")));
            assert!(summary
                .pending
                .iter()
                .any(|s| s.contains("verify deployment")));
        }
        host.shutdown().await.unwrap();
    }
    assert!(fixture.summaries.load(Ordering::SeqCst) >= 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}
