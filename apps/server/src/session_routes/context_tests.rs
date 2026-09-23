use super::*;
use compaction::{CompactedRange, CompactionPlugin, CompactionState};
use sha2::{Digest, Sha256};

#[derive(Default)]
struct ArchiveModel {
    summaries: AtomicUsize,
    primary: StdMutex<Vec<ModelRequest>>,
}
#[async_trait]
impl Model for ArchiveModel {
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> api::Result<ModelStream> {
        let summary = request
            .messages
            .first()
            .is_some_and(|m| m.text().starts_with("Summarize the earlier"));
        if summary {
            self.summaries.fetch_add(1, Ordering::SeqCst);
        } else {
            self.primary.lock().unwrap().push(request);
        }
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(
                if summary {
                    serde_json::to_string(&compaction::TaskSummary { goal: "preserved decision 31415".into(), ..Default::default() }).unwrap()
                } else {
                    "ACK".into()
                }
                .into(),
            )),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}
async fn compacting_service(
    store: Arc<Store>,
    model: Arc<ArchiveModel>,
    request_limit: usize,
) -> (runtime::Host, TestService) {
    let plugin = CompactionPlugin::default();
    store.attach_compactor(plugin.compactor()).unwrap();
    let host = runtime::HostBuilder::new()
        .model(model)
        .plugin(Arc::new(plugin))
        .checkpoint_sink(Arc::new(SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut config = application::ApplicationConfig::default();
    config.run_limits.max_context_bytes = request_limit;
    config.run_limits.max_output_tokens = 64;
    let app = AgentApplication::new(
        sessions::runtime(Arc::new(host.engine()), store.clone()),
        config,
    )
    .unwrap();
    (
        host,
        TestService {
            tasks: SessionApplication::new(store.clone(), app.clone()),
            store,
            app,
        },
    )
}
#[tokio::test]
async fn summary_survives_new_runs_and_restart_without_rewriting_archive_or_resummarizing() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("sessions");
    let model = Arc::new(ArchiveModel::default());
    let first = format!("decision 31415 {}", "A".repeat(2500));
    let id = {
        let store = Store::open(&base, temp.path()).unwrap();
        let id = store.create("summaries").unwrap().id;
        let (mut host, service) = compacting_service(store, model.clone(), 4096).await;
        for (key, prompt) in [
            ("first", first.as_str()),
            ("second", &"B".repeat(1000)),
            ("third", "continue now"),
        ] {
            let turn = turn(&service, &id, key, prompt).await;
            assert_eq!(
                completed(&service, turn.run_id.as_deref().unwrap())
                    .await
                    .status,
                RunStatus::Completed
            );
        }
        assert_eq!(model.summaries.load(Ordering::SeqCst), 1);
        let archive = service.store.get(&id).unwrap().history().unwrap();
        assert_eq!(archive[0].text(), first);
        assert_eq!(archive.len(), 6);
        assert!(!archive
            .iter()
            .any(|m| m.text().contains("[Earlier conversation summary")));
        service.app.shutdown(Duration::from_secs(3)).await.unwrap();
        host.shutdown().await.unwrap();
        id
    };
    let store = Store::open(&base, temp.path()).unwrap();
    let (mut host, service) = compacting_service(store, model.clone(), 4096).await;
    let resumed = turn(&service, &id, "fourth", "after restart").await;
    assert_eq!(
        completed(&service, resumed.run_id.as_deref().unwrap())
            .await
            .status,
        RunStatus::Completed
    );
    assert_eq!(model.summaries.load(Ordering::SeqCst), 1);
    let requests = model.primary.lock().unwrap();
    assert!(serde_json::to_string(requests.last().unwrap())
        .unwrap()
        .contains("preserved decision 31415"));
    drop(requests);
    let doc = service.store.get(&id).unwrap();
    assert_eq!(doc.history().unwrap()[0].text(), first);
    assert_eq!(doc.history().unwrap().len(), 8);
    assert_eq!(doc.header.version, 3);
    let view = crate::session_routes::view::turn_page(&doc, "first", None, 50).unwrap();
    assert_eq!(view.turn.prompt, first);
    service.app.shutdown(Duration::from_secs(3)).await.unwrap();
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn archive_above_core_admission_cap_still_continues_from_verified_bounded_workset() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("sessions");
    let store = Store::open(&base, temp.path()).unwrap();
    let header = store.create("large-archive").unwrap();
    store
        .prepare(&header.id, header.revision, "old", "archive start", RunLimits::default().max_initial_history_bytes)
        .unwrap();
    let mut history = vec![Message::user("archive start")];
    let mut ranges = Vec::new();
    for index in 0..90 {
        let start = history.len();
        let id = format!("call-{index}");
        history.push(Message::Assistant {
            content: String::new(),
            tool_calls: vec![ToolCall::new(&id, "read", json!({"path":"fixture"}))],
            reasoning_content: None,
            provider_data: None,
        });
        history.push(Message::Tool {
            result: ToolResult::new(&id, ToolStatus::Success, "x".repeat(50 * 1024)),
        });
        if index < 89 {
            ranges.push(CompactedRange {
                start,
                end: history.len(),
                sha256: format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&history[start..]).unwrap())
                ),
            });
        }
    }
    history.push(Message::Assistant {
        content: "old final".into(),
        tool_calls: vec![],
        reasoning_content: None,
        provider_data: None,
    });
    assert!(
        serde_json::to_vec(&history).unwrap().len()
            > RunLimits::default().max_initial_history_bytes
    );
    let state = CompactionState {
        version: 2,
        source_messages: history.len(),
        ranges,
        summary: serde_json::to_string(&compaction::TaskSummary { goal: "prior tool rounds retained in archive".into(), ..Default::default() }).unwrap(),
    };
    let mut cp = checkpoint(&header.id, "old", history.clone());
    cp.phase = CheckpointPhase::RunFinished;
    cp.status = Some(RunStatus::Completed);
    store.commit(&cp, &CancellationToken::new()).unwrap();
    // A versioned persistence fixture represents earlier successful incremental compactions;
    // the following continuation is exercised through the real Runtime, not a fake admission path.
    let path = file_path(&base, &header.id);
    let text = std::fs::read_to_string(&path).unwrap();
    let (head, body) = text.split_once('\n').unwrap();
    let mut body: serde_json::Value = serde_json::from_str(body).unwrap();
    body["compaction"] = serde_json::to_value(state).unwrap();
    std::fs::write(&path, format!("{head}\n{body}\n")).unwrap();
    drop(store);
    let model = Arc::new(ArchiveModel::default());
    let store = Store::open(&base, temp.path()).unwrap();
    let preserved = std::fs::read(&path).unwrap();
    let revision = store.get(&header.id).unwrap().header.revision;
    assert_eq!(
        store
            .prepare(
                &header.id,
                revision,
                "disabled",
                "cannot admit full archive",
                RunLimits::default().max_initial_history_bytes
            )
            .err()
            .unwrap()
            .code,
        sessions::SessionErrorCode::Capacity
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        preserved,
        "disabling compaction cannot erase an existing valid workset"
    );
    let (mut host, service) = compacting_service(store, model.clone(), 256 * 1024).await;
    let next = turn(&service, &header.id, "next", "continue from old history").await;
    assert_eq!(
        completed(&service, next.run_id.as_deref().unwrap())
            .await
            .status,
        RunStatus::Completed
    );
    assert!(
        serde_json::to_vec(&model.primary.lock().unwrap()[0])
            .unwrap()
            .len()
            < 256 * 1024
    );
    let doc = service.store.get(&header.id).unwrap();
    let archive = doc.history().unwrap();
    assert_eq!(&archive[..history.len()], history.as_slice());
    assert!(
        serde_json::to_vec(doc.body.checkpoint.as_ref().unwrap())
            .unwrap()
            .len()
            < 256 * 1024
    );
    assert_eq!(model.summaries.load(Ordering::SeqCst), 0);
    service.app.shutdown(Duration::from_secs(3)).await.unwrap();
    host.shutdown().await.unwrap();
}
#[test]
fn corrupt_compaction_never_overwrites_current_history() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("data");
    let store = Store::open(&base, temp.path()).unwrap();
    let h = store.create("corrupt-summary").unwrap();
    store.prepare(&h.id, h.revision, "first", "hello", RunLimits::default().max_initial_history_bytes).unwrap();
    let mut cp = checkpoint(&h.id, "first", vec![Message::user("hello")]);
    cp.phase = CheckpointPhase::RunFinished;
    cp.status = Some(RunStatus::Completed);
    store.commit(&cp, &CancellationToken::new()).unwrap();
    store.attach_compactor(CompactionPlugin::default().compactor()).unwrap();
    let path = file_path(&base, &h.id);
    let text = std::fs::read_to_string(&path).unwrap();
    let (head, body) = text.split_once('\n').unwrap();
    let mut body: serde_json::Value = serde_json::from_str(body).unwrap();
    body["compaction"] = json!({"version":1,"source_messages":1,"summary":"invalid","ranges":[{"start":0,"end":1,"sha256":"wrong"}]});
    let broken = format!("{head}\n{body}\n");
    std::fs::write(&path, &broken).unwrap();
    let revision = store.get(&h.id).unwrap().header.revision;
    assert!(store.prepare(&h.id, revision, "next", "must not execute", RunLimits::default().max_initial_history_bytes).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}
