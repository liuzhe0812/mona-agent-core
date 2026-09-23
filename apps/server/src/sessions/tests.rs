use super::*;
use super::store::Status;
use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex as StdMutex};
use std::time::Duration;
use serde_json::json;
use axum::{body::Body, http::Request as HttpRequest};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[cfg(feature = "compaction")]
#[path = "context_tests.rs"]
mod context_tests;

#[derive(Default)]
struct RememberModel { requests: StdMutex<Vec<ModelRequest>>, calls: AtomicUsize }
#[async_trait]
impl Model for RememberModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = request.messages.iter().filter_map(|m| match m { Message::User { content } => Some(content.text().into_owned()), _ => None }).collect::<Vec<_>>().join(" / ");
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(text)), Ok(ModelEvent::Reasoning("PRIVATE_REASONING".into())),
            Ok(ModelEvent::ProviderData { target: ProtocolTarget::Assistant, data: ProviderData { namespace: "test".into(), value: json!({"signature":"PRIVATE_REPLAY"}) } }),
            Ok(ModelEvent::Finish(FinishReason::Stop)), Ok(ModelEvent::Usage(Usage { input_tokens: 10, output_tokens: 2 })), Ok(ModelEvent::End),
        ])))
    }
}
async fn service(store: Arc<Store>, model: Arc<dyn Model>) -> (runtime::Host, Service) {
    let host = runtime::HostBuilder::new().model(model).checkpoint_sink(Arc::new(SessionSink(store.clone()))).unwrap().build().await.unwrap();
    let app = AgentApplication::new(super::runtime(Arc::new(host.engine()), store.clone()), Default::default()).unwrap();
    (host, Service { store, app, starts: Arc::new(Mutex::new(VecDeque::new())) })
}
async fn completed(service: &Service, run: &str) -> RunOutcome {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(outcome) = service.app.get_result(run).unwrap().outcome { return outcome; }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }).await.unwrap()
}
async fn turn(service: &Service, id: &str, key: &str, prompt: &str) -> TurnResponse {
    let revision = service.store.get(id).unwrap().header.revision;
    service.start(id.into(), NewTurn { request_id: key.into(), revision, prompt: prompt.into() }).await.unwrap()
}
fn file_path(base: &std::path::Path, id: &str) -> std::path::PathBuf {
    std::fs::read_dir(base).unwrap().next().unwrap().unwrap().path().join(format!("{id}.jsonl"))
}
fn checkpoint(id: &str, key: &str, messages: Vec<Message>) -> RunCheckpoint {
    RunCheckpoint { schema_version: CHECKPOINT_VERSION, run_id: "fixture-run".into(), revision: 1,
        phase: CheckpointPhase::BeforeModel, step: 1, transcript: messages, pending_tools: BTreeMap::new(), selected_tools: vec![],
        model_options: ModelOptions::default(), metadata: BTreeMap::from([(SESSION_KEY.into(), id.into()), (TURN_KEY.into(), key.into())]),
        task_usage: TaskUsage { model_calls: 1, reported_tokens: 3, usage_complete: true }, status: None, error: None }
}

#[tokio::test]
async fn completed_turns_survive_reopen_and_continuation_uses_authoritative_history() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let model = Arc::new(RememberModel::default());
    let id = {
        let store = Store::open(&base, tmp.path()).unwrap();
        let header = store.create("durable").unwrap();
        let (mut host, service) = service(store, model.clone()).await;
        let first = turn(&service, &header.id, "first", "remember 31415").await;
        assert_eq!(completed(&service, first.run_id.as_deref().unwrap()).await.status, RunStatus::Completed);
        let second = turn(&service, &header.id, "second", "what was it?").await;
        assert!(completed(&service, second.run_id.as_deref().unwrap()).await.output.unwrap().contains("remember 31415"));
        service.app.shutdown(Duration::from_secs(3)).await.unwrap(); host.shutdown().await.unwrap();
        header.id
    };
    let store = Store::open(&base, tmp.path()).unwrap();
    let doc = store.get(&id).unwrap(); assert_eq!(doc.body.turns.len(), 2); assert_eq!(doc.header.status, Status::Completed);
    let (mut host, service) = service(store, model.clone()).await;
    let count = model.calls.load(Ordering::SeqCst);
    let duplicate = service.start(id.clone(), NewTurn { revision: 0, request_id: "second".into(), prompt: "what was it?".into() }).await.unwrap();
    assert!(duplicate.reused); assert!(!duplicate.live); assert_eq!(model.calls.load(Ordering::SeqCst), count);
    let third = turn(&service, &id, "third", "continue after restart").await;
    let result = completed(&service, third.run_id.as_deref().unwrap()).await;
    assert_eq!(result.output.as_deref(), Some("remember 31415 / what was it? / continue after restart"));
    let public = view::turn_page(&service.store.get(&id).unwrap(), "first", None, 50).unwrap();
    let wire = serde_json::to_string(&public).unwrap();
    assert!(!wire.contains("PRIVATE_REASONING")); assert!(!wire.contains("PRIVATE_REPLAY"));
    assert!(wire.contains("remember 31415"));
    service.app.shutdown(Duration::from_secs(3)).await.unwrap(); host.shutdown().await.unwrap();
}

#[test]
fn crash_recovery_closes_pending_pairs_without_claiming_rollback_or_replaying() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap(); let h = store.create("crash").unwrap();
    store.prepare(&h.id, h.revision, "turn", "go").unwrap();
    let mut cp = checkpoint(&h.id, "turn", vec![Message::user("go"), Message::Assistant {
        content: "working".into(), tool_calls: ["a","b","c"].into_iter().map(|id| ToolCall::new(id,"tool",json!({}))).collect(), reasoning_content: None, provider_data: None,
    }]);
    cp.phase = CheckpointPhase::ToolSettled { call_id: "c".into() };
    cp.pending_tools = BTreeMap::from([
        ("a".into(), CheckpointToolState::Pending), ("b".into(), CheckpointToolState::IntentRecorded),
        ("c".into(), CheckpointToolState::Settled { result: ToolResult::new("c", ToolStatus::Success, "known result") }),
    ]);
    store.commit(&cp, &CancellationToken::new()).unwrap(); drop(store);
    let store = Store::open(&base, tmp.path()).unwrap(); let doc = store.get(&h.id).unwrap();
    assert_eq!(doc.header.status, Status::Interrupted);
    assert_eq!(serde_json::to_value(doc.body.checkpoint.as_ref().unwrap()).unwrap(), serde_json::to_value(&cp).unwrap());
    let history = doc.history().unwrap(); runtime::validate_messages(&history).unwrap();
    let results: Vec<_> = history.iter().filter_map(|m| if let Message::Tool { result } = m { Some(result.status) } else { None }).collect();
    assert_eq!(results, [ToolStatus::Skipped, ToolStatus::Unknown, ToolStatus::Success]);
    let Prepared::New { history: resumed } = store.prepare(&h.id, doc.header.revision, "next", "inspect first").unwrap() else { panic!("new turn expected") };
    assert_eq!(resumed, history);
}

#[test]
fn accepted_input_without_a_run_is_retained_and_not_restarted_by_duplicate() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap(); let h = store.create("pending").unwrap();
    store.prepare(&h.id, h.revision, "once", "saved before dispatch").unwrap(); drop(store);
    let store = Store::open(&base, tmp.path()).unwrap(); let doc = store.get(&h.id).unwrap();
    assert_eq!(doc.header.status, Status::Interrupted); assert_eq!(doc.history().unwrap()[0].text(), "saved before dispatch");
    assert!(matches!(store.prepare(&h.id, 0, "once", "saved before dispatch").unwrap(), Prepared::Existing(..)));
}

#[test]
fn revisions_pagination_workspace_scope_and_deletion_are_explicit() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap(); let h = store.create("one").unwrap(); store.create("two").unwrap();
    let renamed = store.rename(&h.id, h.revision, "我的会话").unwrap();
    assert_eq!(store.rename(&h.id, h.revision, "stale").err().unwrap().code, Code::Conflict);
    assert_eq!(store.list(0, 1, "", false).unwrap().next_offset, Some(1));
    assert_eq!(store.list(0, 50, "我的", false).unwrap().sessions.len(), 1);
    let other_dir = tmp.path().join("other"); std::fs::create_dir(&other_dir).unwrap();
    let other = Store::open(&base, &other_dir).unwrap(); assert!(other.list(0, 50, "", false).unwrap().sessions.is_empty());
    assert!(store.get("../one").is_err()); assert!(store.get("s-one/..").is_err());
    store.delete(&h.id, renamed.revision).unwrap(); assert_eq!(store.get(&h.id).err().unwrap().code, Code::NotFound);
}

#[test]
fn malformed_storage_never_becomes_an_empty_session_or_gets_overwritten() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap(); let h = store.create("bad").unwrap(); store.create("good").unwrap();
    let path = file_path(&base, &h.id); let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len()/2]).unwrap(); let broken = std::fs::read(&path).unwrap();
    assert!(store.get(&h.id).is_err()); assert!(store.create("bad").is_err());
    assert!(store.prepare(&h.id, h.revision, "x", "do not execute").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), broken);
    let listed = store.list(0, 50, "", false).unwrap(); assert_eq!(listed.unreadable, 1); assert_eq!(listed.sessions.len(), 1);
}

#[test]
fn cancelled_commit_and_conflicting_revision_preserve_last_acknowledged_bytes() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap(); let h = store.create("atomic").unwrap();
    store.prepare(&h.id, h.revision, "t", "go").unwrap();
    let mut cp = checkpoint(&h.id,"t",vec![Message::user("go")]);
    store.commit(&cp,&CancellationToken::new()).unwrap();
    let path = file_path(&base,&h.id); let saved = std::fs::read(&path).unwrap();
    store.commit(&cp,&CancellationToken::new()).unwrap(); assert_eq!(std::fs::read(&path).unwrap(),saved);
    cp.transcript.push(Message::user("different same revision")); assert!(store.commit(&cp,&CancellationToken::new()).is_err());
    cp.revision += 1; let cancel = CancellationToken::new(); cancel.cancel(); assert!(store.commit(&cp,&cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(),saved);
}

#[tokio::test]
async fn host_routes_require_auth_and_never_accept_client_history_or_limits() {
    let tmp = tempfile::tempdir().unwrap(); let store = Store::open(&tmp.path().join("data"),tmp.path()).unwrap();
    let (mut host, service) = service(store.clone(),Arc::new(RememberModel::default())).await;
    let token = "test-only-sessions-token-32-characters";
    let router = super::router(store,service.app.clone(),token.into(),None).unwrap();
    let unauth = router.clone().oneshot(HttpRequest::builder().uri("/api/sessions").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(unauth.status(),StatusCode::UNAUTHORIZED); assert_eq!(unauth.headers()["cache-control"],"no-store");
    for payload in [json!({"request_id":"ok","history":[]}), json!({"request_id":"ok","limits":{}})] {
        let response = router.clone().oneshot(HttpRequest::builder().method("POST").uri("/api/sessions")
            .header("authorization",format!("Bearer {token}")).header("content-type","application/json").body(Body::from(payload.to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(),StatusCode::BAD_REQUEST);
    }
    let response = router.oneshot(HttpRequest::builder().method("POST").uri("/api/sessions")
        .header("authorization",format!("Bearer {token}")).header("content-type","application/json").body(Body::from(r#"{"request_id":"ok"}"#)).unwrap()).await.unwrap();
    assert_eq!(response.status(),StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes(); assert!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"].is_string());
    service.app.shutdown(Duration::from_secs(2)).await.unwrap(); host.shutdown().await.unwrap();
}


#[test]
fn inconsistent_turn_ranges_are_rejected_before_read_or_rewrite() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap(); let h = store.create("range").unwrap();
    store.prepare(&h.id, h.revision, "t", "go").unwrap();
    store.fail_start(&h.id, "t", "test-only failure").unwrap();
    let path = file_path(&base, &h.id);
    let bytes = std::fs::read_to_string(&path).unwrap();
    let (header, body) = bytes.split_once('\n').unwrap();
    let mut body: serde_json::Value = serde_json::from_str(body).unwrap();
    body["turns"][0]["end"] = json!(999);
    let broken = format!("{header}\n{body}\n"); std::fs::write(&path, &broken).unwrap();
    assert!(store.get(&h.id).is_err());
    assert!(store.rename(&h.id, 3, "must not rewrite").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

#[test]
fn empty_session_is_durable_and_single_writer_owned() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("sessions"), temp.path()).unwrap();
    let created = store.create("create-once").unwrap();
    assert_eq!(store.create("create-once").unwrap().id, created.id);
    assert!(Store::open(&temp.path().join("sessions"), temp.path()).is_err());
    drop(store);
    let store = Store::open(&temp.path().join("sessions"), temp.path()).unwrap();
    assert_eq!(store.list(0, 50, "", false).unwrap().sessions.len(), 1);
    assert_eq!(store.get(&created.id).unwrap().header.revision, created.revision);
}

#[test]
fn pinned_archived_and_unread_flags_are_durable_and_select_the_list_view() {
    let tmp = tempfile::tempdir().unwrap(); let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let one = store.create("one").unwrap(); let two = store.create("two").unwrap();
    let pinned = store.set_pinned(&one.id, one.revision, true).unwrap();
    assert!(pinned.pinned && !pinned.archived);
    assert!(store.set_archived(&two.id, two.revision, true).unwrap().archived);
    let active = store.list(0, 50, "", false).unwrap();
    assert_eq!(active.sessions.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), [one.id.as_str()]);
    assert!(active.sessions[0].pinned);
    let archived = store.list(0, 50, "", true).unwrap();
    assert_eq!(archived.sessions.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), [two.id.as_str()]);
    // A stale revision is refused instead of overwriting newer state.
    assert_eq!(store.set_pinned(&one.id, one.revision, false).err().unwrap().code, Code::Conflict);
    assert!(store.set_archived(&one.id, pinned.revision, true).unwrap().archived);
    drop(store);
    let store = Store::open(&base, tmp.path()).unwrap();
    let reopened = store.get(&one.id).unwrap().header;
    assert!(reopened.pinned && reopened.archived);
    // Unarchiving puts the task back into the active view.
    store.set_archived(&one.id, reopened.revision, false).unwrap();
    assert_eq!(store.list(0, 50, "", false).unwrap().sessions.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), [one.id.as_str()]);
    // The unread mark is durable as well, and clearing it leaves the other flags alone.
    let revision = store.get(&one.id).unwrap().header.revision;
    let marked = store.set_unread(&one.id, revision, true).unwrap();
    assert!(marked.unread && marked.pinned && !marked.archived);
    drop(store);
    let store = Store::open(&base, tmp.path()).unwrap();
    assert!(store.get(&one.id).unwrap().header.unread);
    let revision = store.get(&one.id).unwrap().header.revision;
    let cleared = store.set_unread(&one.id, revision, false).unwrap();
    assert!(!cleared.unread && cleared.pinned && !cleared.archived);
}
