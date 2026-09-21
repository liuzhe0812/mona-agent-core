#![cfg(feature = "model-management")]

// The integration test exercises the management router; environment composition belongs to the server binary.
#[allow(dead_code)]
#[path = "../src/model_settings.rs"]
mod management;

use api::{AgentExecutor, Result, RunRequest, RunStatus};
use runtime::HostBuilder;
use models::{ModelManager, SettingsStore};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tower::ServiceExt;

const TOKEN: &str = "test-model-settings-bearer-token-32-bytes";
const API_KEY: &str = "test-secret-is-never-returned";

#[derive(Default)]
struct MemoryStore(Mutex<Option<Vec<u8>>>);

impl SettingsStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().expect("memory store lock").clone())
    }

    fn save(&self, bytes: &[u8]) -> Result<()> {
        *self.0.lock().expect("memory store lock") = Some(bytes.to_vec());
        Ok(())
    }
}

fn manager(allow_http_loopback: bool) -> ModelManager {
    ModelManager::open(Arc::new(MemoryStore::default()), allow_http_loopback)
        .expect("open in-memory model settings")
}

fn provider(revision: u64, id: &str, api_base: &str) -> Value {
    json!({
        "revision": revision,
        "id": id,
        "name": format!("{id} provider"),
        "api_base": api_base,
        "api_key": API_KEY,
        "clear_key": false,
        "models": [
            {"id": "alpha", "enabled": true},
            {"id": "beta", "enabled": true}
        ]
    })
}

async fn call(
    router: Router,
    method: Method,
    uri: &str,
    bearer: Option<&str>,
    payload: Option<Value>,
) -> (StatusCode, Vec<u8>, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = match payload {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize request body"))
        }
        None => Body::empty(),
    };
    let response = router
        .oneshot(request.body(body).expect("build request"))
        .await
        .expect("model settings router response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("read model settings response")
        .to_bytes()
        .to_vec();
    let value = serde_json::from_slice(&bytes).expect("model settings response is JSON");
    (status, bytes, value)
}

fn settings_router(manager: ModelManager) -> Router {
    management::router(manager, TOKEN.to_owned(), None).expect("build model settings router")
}

#[tokio::test]
async fn management_routes_require_bearer_authentication() {
    let router = settings_router(manager(false));

    let (status, _, _) = call(
        router.clone(),
        Method::GET,
        "/api/model-settings",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = call(
        router,
        Method::POST,
        "/api/model-settings/providers",
        Some("wrong-token"),
        Some(provider(0, "local", "https://example.com/v1")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn management_rejects_unknown_fields_and_does_not_return_api_keys() {
    let router = settings_router(manager(false));

    let mut unknown = provider(0, "local", "https://example.com/v1");
    unknown
        .as_object_mut()
        .expect("provider payload object")
        .insert("unexpected".into(), json!(true));
    let (status, _, _) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/providers",
        Some(TOKEN),
        Some(unknown),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, bytes, view) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/providers",
        Some(TOKEN),
        Some(provider(0, "local", "https://example.com/v1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 1);
    assert_eq!(view["providers"][0]["has_key"], true);
    assert!(view["providers"][0].get("api_key").is_none());
    assert!(!String::from_utf8_lossy(&bytes).contains(API_KEY));

    let (status, bytes, view) = call(
        router,
        Method::GET,
        "/api/model-settings",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 1);
    assert_eq!(view["providers"][0]["has_key"], true);
    assert!(view["providers"][0].get("api_key").is_none());
    assert!(!String::from_utf8_lossy(&bytes).contains(API_KEY));
}

#[tokio::test]
async fn management_uses_revision_conflicts_and_controls_default_visibility_and_delete() {
    let router = settings_router(manager(false));
    let (status, _, view) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/providers",
        Some(TOKEN),
        Some(provider(0, "local", "https://example.com/v1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 1);
    assert_eq!(
        view["default"],
        json!({"provider_id": "local", "model_id": "alpha"})
    );

    let (status, _, conflict) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/providers",
        Some(TOKEN),
        Some(provider(0, "stale", "https://example.com/v1")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(conflict["message"]
        .as_str()
        .unwrap_or_default()
        .starts_with("settings_conflict:"));

    let (status, _, view) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/default",
        Some(TOKEN),
        Some(json!({"revision": 1, "provider_id": "local", "model_id": "beta"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 2);
    assert_eq!(
        view["default"],
        json!({"provider_id": "local", "model_id": "beta"})
    );

    let (status, _, view) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/visibility",
        Some(TOKEN),
        Some(json!({"revision": 2, "provider_id": "local", "model_id": "alpha", "enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 3);
    let models = view["providers"][0]["models"]
        .as_array()
        .expect("model list");
    assert_eq!(models[0], json!({"id": "alpha", "enabled": false}));
    assert_eq!(models[1], json!({"id": "beta", "enabled": true}));

    let (status, _, conflict) = call(
        router,
        Method::POST,
        "/api/model-settings/delete",
        Some(TOKEN),
        Some(json!({"revision": 3, "provider_id": "local"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(conflict["message"]
        .as_str()
        .unwrap_or_default()
        .starts_with("default_conflict:"));
}

async fn read_http_body(socket: &mut TcpStream) -> Value {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = socket.read(&mut chunk).await.expect("read model request");
        assert!(count > 0, "model request ended before headers");
        received.extend_from_slice(&chunk[..count]);
        if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&received[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .expect("model request content length");
    while received.len() < header_end + length {
        let count = socket
            .read(&mut chunk)
            .await
            .expect("read model request body");
        assert!(count > 0, "model request ended before body");
        received.extend_from_slice(&chunk[..count]);
    }
    serde_json::from_slice(&received[header_end..header_end + length]).expect("model request JSON")
}

async fn mock_chat_server(requests: usize) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local mock model");
    let endpoint = format!(
        "http://{}/v1",
        listener.local_addr().expect("mock model address")
    );
    let task = tokio::spawn(async move {
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let mut bodies = Vec::with_capacity(requests);
        for _ in 0..requests {
            let (mut socket, _) = listener.accept().await.expect("accept model request");
            bodies.push(read_http_body(&mut socket).await);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(), sse
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write model response");
        }
        bodies
    });
    (endpoint, task)
}

#[tokio::test]
async fn new_runs_use_the_current_default_model() {
    let (endpoint, server) = mock_chat_server(2).await;
    let manager = manager(true);
    let router = settings_router(manager.clone());

    let (status, _, view) = call(
        router.clone(),
        Method::POST,
        "/api/model-settings/providers",
        Some(TOKEN),
        Some(provider(0, "local", &endpoint)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 1);

    let mut host = HostBuilder::new()
        .plugin(manager.plugin())
        .build()
        .await
        .expect("build managed model host");
    let runtime = manager.runtime(Arc::new(host.engine()));
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.execute(RunRequest::new("first run")),
    )
    .await
    .expect("first run does not hang")
    .expect("first run succeeds");
    assert_eq!(first.status, RunStatus::Completed);

    let (status, _, view) = call(
        router,
        Method::POST,
        "/api/model-settings/default",
        Some(TOKEN),
        Some(json!({"revision": 1, "provider_id": "local", "model_id": "beta"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["revision"], 2);

    let second = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.execute(RunRequest::new("second run")),
    )
    .await
    .expect("second run does not hang")
    .expect("second run succeeds");
    assert_eq!(second.status, RunStatus::Completed);
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("mock model server completes")
        .expect("mock model server task succeeds");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "alpha");
    assert_eq!(requests[1]["model"], "beta");
    assert!(requests
        .iter()
        .all(|request| request.get("api_key").is_none()));

    host.shutdown().await.expect("shutdown managed model host");
}

#[tokio::test]
async fn changing_default_during_a_tool_run_does_not_change_its_next_round() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let manager = manager(true);
        manager.upsert(serde_json::from_value(provider(0, "local", &endpoint)).unwrap()).unwrap();
        let changing_manager = manager.clone();
        let server = tokio::spawn(async move {
            let mut requests = vec![];
            for index in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(read_http_body(&mut socket).await);
                let chunk = if index == 0 {
                    // The first round is already dispatched. Change the default before
                    // the tool reply starts round two, without timing-dependent sleeps.
                    changing_manager.set_default(1, models::Selection {
                        provider_id: "local".into(), model_id: "beta".into(),
                    }).unwrap();
                    json!({"choices":[{"index":0,"delta":{"tool_calls":[{
                        "index":0,"id":"add-1","type":"function",
                        "function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}
                    }]},"finish_reason":"tool_calls"}]})
                } else {
                    json!({"choices":[{"index":0,"delta":{"content":"done"},"finish_reason":"stop"}]})
                };
                let sse = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}", sse.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let mut host = HostBuilder::new().plugin(manager.plugin()).tool(Arc::new(demo::Add)).build().await.unwrap();
        let runtime = manager.runtime(Arc::new(host.engine()));
        let first = runtime.execute(RunRequest::new("add the numbers")).await.unwrap();
        assert_eq!(first.status, RunStatus::Completed, "{:?}", first.error);
        assert_eq!(first.steps, 2);
        let second = runtime.execute(RunRequest::new("new task")).await.unwrap();
        assert_eq!(second.status, RunStatus::Completed);
        let requests = server.await.unwrap();
        assert_eq!(requests.iter().map(|r| r["model"].as_str().unwrap()).collect::<Vec<_>>(), vec!["alpha", "alpha", "beta"]);
        host.shutdown().await.unwrap();
    }).await.expect("multi-round model routing must finish");
}
