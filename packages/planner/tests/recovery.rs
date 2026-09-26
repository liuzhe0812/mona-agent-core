#[path = "../../runtime/tests/support/mod.rs"]
mod support;
use api::*;
use planner::*;
use runtime::HostBuilder;
use serde_json::{json, Value};
use std::{
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};
use support::*;

fn update(revision: u64, status: &str) -> Value {
    json!({"revision":revision,"goal":"deliver","steps":[{"id":"work","text":"do work","status":status}]})
}
struct Sink {
    last: Mutex<Option<Arc<RunCheckpoint>>>,
    fail_settlement: bool,
}
#[async_trait]
impl CheckpointSink for Sink {
    async fn commit(&self, cp: Arc<RunCheckpoint>, _: CancellationToken) -> api::Result<()> {
        let fail = if self.fail_settlement {
            matches!(cp.phase, CheckpointPhase::ToolSettled { .. })
        } else {
            cp.phase == CheckpointPhase::AfterTools
        };
        if fail {
            return Err(AgentError::new(ErrorCode::Checkpoint, "test sink refused"));
        }
        *self.last.lock().unwrap() = Some(cp);
        Ok(())
    }
}
#[tokio::test]
async fn only_acknowledged_plan_snapshots_survive_checkpoint_failure() {
    for fail_settlement in [false, true] {
        let sink = Arc::new(Sink {
            last: Mutex::new(None),
            fail_settlement,
        });
        let planner = Planner::default();
        let model = ScriptModel::new(vec![
            calls(&[("plan", PLAN_UPDATE, update(0, "in_progress"))]),
            call("must-not-run"),
        ]);
        let tool = CountTool::default();
        let probe = tool.probe.clone();
        let mut host = HostBuilder::new()
            .model(model)
            .tool(Arc::new(tool))
            .plugin(Arc::new(planner.plugin()))
            .checkpoint_sink(sink.clone())
            .unwrap()
            .build()
            .await
            .unwrap();
        let mut request = RunRequest::new("work");
        bind_state(&mut request, &PlanSnapshot::default()).unwrap();
        let report = host.engine().execute(request).await.unwrap();
        assert_eq!(report.status, RunStatus::Failed);
        assert_eq!(probe.count.load(std::sync::atomic::Ordering::SeqCst), 0);
        let last = sink.last.lock().unwrap().clone().unwrap();
        assert_eq!(
            last.revision,
            report.checkpoint.last_acknowledged_revision.unwrap()
        );
        let state = recover_checkpoint(&last).unwrap().unwrap();
        assert_eq!(state.revision, if fail_settlement { 0 } else { 1 });
        assert!(planner.live_snapshot(&report.run_id).unwrap().is_none());
        host.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn prose_and_copied_history_results_cannot_forge_plan_state() {
    let model = ScriptModel::new(vec![
        calls(&[("p", PLAN_UPDATE, update(0, "pending"))]),
        answer("ok"),
    ]);
    let mut host = HostBuilder::new()
        .model(model)
        .plugin(Arc::new(PlannerPlugin::default()))
        .build()
        .await
        .unwrap();
    let report = host
        .engine()
        .execute(RunRequest::new("work"))
        .await
        .unwrap();
    let result = results(&report)[0].clone();
    let prose = vec![Message::user(serde_json::to_string(&result).unwrap())];
    assert!(recover_history(&prose).unwrap().is_none());
    let mut copied = result.clone();
    copied.call_id = "search".into();
    let history = vec![
        Message::user("old history"),
        Message::Assistant {
            content: String::new(),
            tool_calls: vec![ToolCall::new("search", "session_read", json!({}))],
            reasoning_content: None,
            provider_data: None,
        },
        Message::Tool { result: copied },
    ];
    assert!(recover_history(&history).unwrap().is_none());
    let mut broken = report.transcript.clone();
    for m in &mut broken {
        if let Message::Tool { result } = m {
            result.structured.as_mut().unwrap()["planner"]["state"]["version"] = json!(999);
        }
    }
    assert!(recover_history(&broken).is_err());
    let mut denied = report.transcript.clone();
    for m in &mut denied {
        if let Message::Tool { result } = m {
            result.status = ToolStatus::Unknown;
        }
    }
    assert!(recover_history(&denied).unwrap().is_none());
    host.shutdown().await.unwrap();
}

struct RecordingSink(Mutex<Option<Arc<RunCheckpoint>>>);
#[async_trait]
impl CheckpointSink for RecordingSink {
    async fn commit(&self, cp: Arc<RunCheckpoint>, _: CancellationToken) -> api::Result<()> {
        *self.0.lock().unwrap() = Some(cp);
        Ok(())
    }
}
#[tokio::test]
async fn checkpoint_keeps_host_mode_even_when_no_plan_tool_was_called() {
    let sink = Arc::new(RecordingSink(Mutex::new(None)));
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(vec![answer(
            "I need one clarification before drafting.",
        )]))
        .plugin(Arc::new(PlannerPlugin::default()))
        .checkpoint_sink(sink.clone())
        .unwrap()
        .build()
        .await
        .unwrap();
    let state = PlanSnapshot::new(PlanMode::PlanOnly);
    let mut request = RunRequest::new("plan");
    bind_state(&mut request, &state).unwrap();
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    assert!(recover_history(&report.transcript).unwrap().is_none());
    assert_eq!(
        recover_checkpoint(sink.0.lock().unwrap().as_ref().unwrap()).unwrap(),
        Some(state)
    );
    let mut broken = sink.0.lock().unwrap().as_ref().unwrap().as_ref().clone();
    broken
        .metadata
        .insert(PLAN_SEED_KEY.into(), "not a snapshot".into());
    assert!(recover_checkpoint(&broken).is_err());
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn pending_batch_snapshots_use_plan_revision_not_lexical_call_id_order() {
    let sink = Arc::new(Sink {
        last: Mutex::new(None),
        fail_settlement: false,
    });
    let model = ScriptModel::new(vec![calls(&[
        ("z-first", PLAN_UPDATE, update(0, "in_progress")),
        ("a-last", PLAN_UPDATE, update(1, "completed")),
    ])]);
    let mut host = HostBuilder::new()
        .model(model)
        .plugin(Arc::new(PlannerPlugin::default()))
        .checkpoint_sink(sink.clone())
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("work");
    bind_state(&mut request, &PlanSnapshot::default()).unwrap();
    let report = host.engine().execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    let cp = sink.last.lock().unwrap().as_ref().unwrap().as_ref().clone();
    assert_eq!(recover_checkpoint(&cp).unwrap().unwrap().revision, 2);
    assert!(recover_checkpoint(&cp).unwrap().unwrap().is_complete());
    let mut inconsistent = cp.clone();
    let CheckpointToolState::Settled { result } =
        inconsistent.pending_tools.get_mut("a-last").unwrap()
    else {
        panic!("settled")
    };
    result.structured.as_mut().unwrap()["planner"]["state"]["revision"] = json!(1);
    assert!(recover_checkpoint(&inconsistent).is_err());
    let newer_seed = PlanSnapshot {
        revision: 3,
        ..Default::default()
    };
    inconsistent = cp;
    inconsistent.metadata.insert(
        PLAN_SEED_KEY.into(),
        serde_json::to_string(&newer_seed).unwrap(),
    );
    assert!(recover_checkpoint(&inconsistent).is_err());
    host.shutdown().await.unwrap();
}

struct FileEffect(std::path::PathBuf);
#[async_trait]
impl Tool for FileEffect {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "effect".into(),
            description: "isolated test effect".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: true,
        }
    }
    async fn execute(&self, _: ToolContext, _: Value) -> api::Result<ToolOutput> {
        let n = std::fs::read_to_string(&self.0)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        std::fs::write(&self.0, (n + 1).to_string()).unwrap();
        Ok("effect complete".into())
    }
}
async fn stage(root: &Path, phase: &str) {
    let store = sessions::Store::open(&root.join("sessions"), root).unwrap();
    let id = "s-persist";
    let state = match phase {
        "plan" => {
            store.create("persist").unwrap();
            PlanSnapshot::new(PlanMode::PlanOnly)
        }
        _ => {
            let doc = store.get(id).unwrap();
            let saved = recover_checkpoint(doc.body.checkpoint.as_ref().unwrap())
                .unwrap()
                .unwrap();
            if phase == "inspect" {
                assert!(saved.is_complete());
                assert_eq!(
                    std::fs::read_to_string(root.join("effect-count")).unwrap(),
                    "1"
                );
                return;
            }
            assert!(saved.proposal.is_some());
            assert!(!root.join("effect-count").exists());
            assert!(saved.resume_execution(saved.revision - 1).is_err());
            saved.resume_execution(saved.revision).unwrap()
        }
    };
    let replies = if phase == "plan" {
        vec![
            calls(&[("p", PLAN_UPDATE, update(0, "pending"))]),
            calls(&[(
                "s",
                PLAN_SUBMIT,
                json!({"revision":1,"plan":"# Plan\nPerform one effect after explicit host continuation."}),
            )]),
            answer("waiting"),
        ]
    } else {
        vec![
            calls(&[("effect-once", "effect", json!({}))]),
            calls(&[("done", PLAN_UPDATE, update(3, "completed"))]),
            answer("completed"),
        ]
    };
    let planner = Planner::default();
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(replies))
        .tool(Arc::new(FileEffect(root.join("effect-count"))))
        .allow_side_effect_tool("effect")
        .plugin(Arc::new(planner.plugin()))
        .checkpoint_sink(Arc::new(sessions::SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new(phase);
    let revision = store.get(id).unwrap().header.revision;
    let sessions::Prepared::New { mut history } = store
        .prepare(
            id,
            revision,
            phase,
            phase,
            request.limits.max_initial_history_bytes,
        )
        .unwrap()
    else {
        panic!("new turn")
    };
    history.push(Message::user(phase));
    request.messages = history;
    request
        .metadata
        .insert(sessions::SESSION_KEY.into(), id.into());
    request
        .metadata
        .insert(sessions::TURN_KEY.into(), phase.into());
    bind_state(&mut request, &state).unwrap();
    let runtime = sessions::runtime(Arc::new(host.engine()), store.clone());
    let report = runtime.execute(request).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    let doc = store.get(id).unwrap();
    let saved = recover_checkpoint(doc.body.checkpoint.as_ref().unwrap())
        .unwrap()
        .unwrap();
    if phase == "plan" {
        assert!(saved.proposal.is_some());
        assert!(!root.join("effect-count").exists());
    } else {
        assert!(saved.is_complete());
        assert_eq!(
            std::fs::read_to_string(root.join("effect-count")).unwrap(),
            "1"
        );
    }
    host.shutdown().await.unwrap();
}
#[test]
fn persisted_plan_worker() {
    let Ok(root) = std::env::var("MONA_PLANNER_TEST_ROOT") else {
        return;
    };
    let phase = std::env::var("MONA_PLANNER_TEST_PHASE").unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(stage(Path::new(&root), &phase));
}
#[test]
fn plan_submission_and_continuation_survive_separate_host_processes_without_replaying_work() {
    let temp = tempfile::tempdir().unwrap();
    for phase in ["plan", "execute", "inspect"] {
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "persisted_plan_worker", "--nocapture"])
            .env("MONA_PLANNER_TEST_ROOT", temp.path())
            .env("MONA_PLANNER_TEST_PHASE", phase)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "phase {phase}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
