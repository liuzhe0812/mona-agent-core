#![cfg(all(feature = "model-management", feature = "compaction"))]
//! Actual HTTP provider + model manager + durable session admission; no paid endpoint.
use api::*;
use application::sessions::{SessionApplication, TurnRequest};
use application::{AgentApplication, ApplicationConfig, ApplicationErrorCode};
use axum::{extract::State, routing::post, Json, Router};
use models::{ModelManager, Selection, SettingsStore};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Default)]
struct Settings(Mutex<Option<Vec<u8>>>);
impl SettingsStore for Settings {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, bytes: &[u8]) -> Result<()> {
        *self.0.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}
#[derive(Default)]
struct Fixture {
    private: AtomicBool,
    requests: Mutex<Vec<Value>>,
}
async fn reply(
    State(f): State<Arc<Fixture>>,
    Json(body): Json<Value>,
) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    f.requests.lock().unwrap().push(body);
    let delta = if f.private.load(Ordering::SeqCst) {
        json!({"content":"confirmed", "reasoning_content":"PRIVATE_REASONING_DO_NOT_LEAK"})
    } else {
        json!({"content":"confirmed"})
    };
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"index":0,"delta":delta,"finish_reason":"stop"}]})
        ),
    )
}
fn select(manager: &ModelManager, model: &str) {
    manager
        .set_default(
            manager.view().revision,
            Selection {
                provider_id: "local".into(),
                model_id: model.into(),
            },
        )
        .unwrap();
}
async fn assemble(
    store: Arc<sessions::Store>,
    manager: &ModelManager,
) -> (runtime::Host, AgentApplication, SessionApplication) {
    let compaction = compaction::CompactionPlugin::default();
    store.attach_compactor(compaction.compactor()).unwrap();
    let host = runtime::HostBuilder::new()
        .plugin(manager.plugin())
        .plugin(Arc::new(compaction))
        .checkpoint_sink(Arc::new(sessions::SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let app = AgentApplication::new(
        sessions::runtime(manager.runtime(Arc::new(host.engine())), store.clone()),
        ApplicationConfig::default(),
    )
    .unwrap();
    let service = SessionApplication::new(store, app.clone());
    (host, app, service)
}
async fn turn(
    store: &sessions::Store,
    app: &AgentApplication,
    service: &SessionApplication,
    id: &str,
    key: &str,
) -> application::ApplicationResult<application::sessions::TurnResponse> {
    let result = service
        .start_turn(
            id.into(),
            TurnRequest {
                request_id: key.into(),
                revision: store.get(id).unwrap().header.revision,
                prompt: format!("continue {key}"),
            },
        )
        .await?;
    if let Some(run) = &result.run_id {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if app.get_result(run).unwrap().outcome.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    Ok(result)
}
fn file(base: &std::path::Path, id: &str) -> Vec<u8> {
    // Format 3 keys sessions by the host state namespace, not the current cwd.
    std::fs::read(base.join(format!("{id}.jsonl"))).unwrap()
}
#[tokio::test]
async fn incompatible_switch_is_rejected_before_saving_or_network_and_same_route_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("state");
    let fixture = Arc::new(Fixture::default());
    fixture.private.store(true, Ordering::SeqCst);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/chat/completions", post(reply))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let settings = Arc::new(Settings::default());
    let manager = ModelManager::open(settings.clone(), true).unwrap();
    manager
        .upsert(
            serde_json::from_value(
                json!({"revision":0,"id":"local","name":"local","protocol":"chat_completions","api_base":endpoint,
        "models":[{"id":"alpha","enabled":true},{"id":"beta","enabled":true}]}),
            )
            .unwrap(),
        )
        .unwrap();
    let id;
    {
        let store = sessions::Store::open(&base, temp.path()).unwrap();
        id = store.create("private").unwrap().id;
        let (mut host, app, service) = assemble(store.clone(), &manager).await;
        turn(&store, &app, &service, &id, "one").await.unwrap();
        let original = file(&base, &id);
        let count = fixture.requests.lock().unwrap().len();
        let preflight_runtime = manager.runtime(Arc::new(host.engine()));
        preflight_runtime
            .validate_history(
                &store.get(&id).unwrap().history().unwrap(),
                &ModelOptions::default(),
            )
            .unwrap();
        // A successful preflight does not reserve the model: start below must check again.
        select(&manager, "beta");
        let error = turn(&store, &app, &service, &id, "two")
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, ApplicationErrorCode::InvalidRequest);
        assert!(error.message.contains("请新建会话"));
        assert!(!error.message.contains("PRIVATE_REASONING"));
        assert_eq!(file(&base, &id), original);
        assert_eq!(fixture.requests.lock().unwrap().len(), count);
        assert_eq!(store.list(0, 10, "", false).unwrap().sessions.len(), 1);
        // Direct runtime reuse has the same guard, before summarizer/audit/tool execution.
        let mut request = RunRequest::new("direct");
        request.messages = store.get(&id).unwrap().history().unwrap();
        request.messages.push(Message::user("direct"));
        let task = request.task.clone();
        let error = manager
            .runtime(Arc::new(host.engine()))
            .start(request)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::ModelHistoryIncompatible);
        assert_eq!(task.usage().model_calls, 0);
        select(&manager, "alpha");
        turn(&store, &app, &service, &id, "two").await.unwrap(); // previous rejection did not consume this key
        assert_eq!(store.get(&id).unwrap().body.turns.len(), 2);
        fixture.private.store(false, Ordering::SeqCst);
        let plain = store.create("plain").unwrap().id;
        turn(&store, &app, &service, &plain, "p1").await.unwrap();
        select(&manager, "beta");
        turn(&store, &app, &service, &plain, "p2").await.unwrap();
        assert_eq!(
            fixture.requests.lock().unwrap().last().unwrap()["model"],
            "beta"
        );
        select(&manager, "alpha");
        app.shutdown(Duration::from_secs(2)).await.unwrap();
        host.shutdown().await.unwrap();
    }
    let reopened_manager = ModelManager::open(settings, true).unwrap();
    let store = sessions::Store::open(&base, temp.path()).unwrap();
    let (mut host, app, service) = assemble(store.clone(), &reopened_manager).await;
    turn(&store, &app, &service, &id, "three").await.unwrap();
    let requests = fixture.requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert_eq!(last["model"], "alpha");
    assert!(last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["reasoning_content"] == "PRIVATE_REASONING_DO_NOT_LEAK"));
    assert!(!serde_json::to_string(last).unwrap().contains("\"route\""));
    drop(requests);
    assert_eq!(store.get(&id).unwrap().body.turns.len(), 3);
    app.shutdown(Duration::from_secs(2)).await.unwrap();
    host.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}
