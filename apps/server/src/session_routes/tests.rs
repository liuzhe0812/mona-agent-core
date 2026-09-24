use super::*;
use api::*;
use sessions::{SessionSink, Status};
#[cfg(feature = "compaction")]
use sessions::{SESSION_KEY, TURN_KEY};
#[cfg(feature = "compaction")]
use std::collections::BTreeMap;
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
struct TestService { store: Arc<Store>, app: AgentApplication, tasks: SessionApplication }
async fn service(store: Arc<Store>, model: Arc<dyn Model>) -> (runtime::Host, TestService) {
    let host = runtime::HostBuilder::new().model(model).checkpoint_sink(Arc::new(SessionSink(store.clone()))).unwrap().build().await.unwrap();
    let app = AgentApplication::new(sessions::runtime(Arc::new(host.engine()), store.clone()), Default::default()).unwrap();
    (host, TestService { tasks: SessionApplication::new(store.clone(), app.clone()), store, app })
}
async fn completed(service: &TestService, run: &str) -> RunOutcome {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(outcome) = service.app.get_result(run).unwrap().outcome { return outcome; }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }).await.unwrap()
}
async fn turn(service: &TestService, id: &str, key: &str, prompt: &str) -> TurnResponse {
    let revision = service.store.get(id).unwrap().header.revision;
    service.tasks.start_turn(id.into(), NewTurn { request_id: key.into(), revision, prompt: prompt.into() }).await.unwrap()
}
#[cfg(feature = "compaction")]
fn file_path(base: &std::path::Path, id: &str) -> std::path::PathBuf {
    base.join(format!("{id}.jsonl"))
}
#[cfg(feature = "compaction")]
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
    let duplicate = service.tasks.start_turn(id.clone(), NewTurn { revision: 0, request_id: "second".into(), prompt: "what was it?".into() }).await.unwrap();
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

#[tokio::test]
async fn host_routes_require_auth_and_never_accept_client_history_or_limits() {
    let tmp = tempfile::tempdir().unwrap(); let store = Store::open(&tmp.path().join("data"),tmp.path()).unwrap();
    let (mut host, service) = service(store.clone(),Arc::new(RememberModel::default())).await;
    let token = "test-only-sessions-token-32-characters";
    let router = super::router(store,service.app.clone(),token.into(),None,None,None).unwrap();
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


