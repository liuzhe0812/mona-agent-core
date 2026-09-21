use agent_application::*;
use agent_core::HostBuilder;
use agent_demo::{ScriptedModel, text};
use axum::{Router, body::Body, http::{Request, StatusCode}};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;
const TOKEN: &str = "test-only-token-with-at-least-32-bytes";
async fn fixture() -> (agent_core::Host, Router) {
    let host = HostBuilder::new().model(Arc::new(ScriptedModel::new(vec![text("ok")]))).event_capacity(4096).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let router = agent_bridge_http::router(app, agent_bridge_http::HttpConfig::new(TOKEN)).unwrap(); (host, router)
}
async fn send(router: Router, method: &str, path: &str, body: &str, auth: bool) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(path).header("content-type", "application/json");
    if auth { request = request.header("authorization", format!("Bearer {TOKEN}")); }
    router.oneshot(request.body(Body::from(body.to_owned())).unwrap()).await.unwrap()
}
#[tokio::test]
async fn authentication_is_required_even_on_info_and_events() {
    let (mut host, router) = fixture().await;
    assert_eq!(send(router.clone(), "GET", "/v1/info", "", false).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(router, "GET", "/v1/runs/none/events", "", false).await.status(), StatusCode::UNAUTHORIZED);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn http_start_sse_result_and_cursor_reconnect_share_protocol() {
    let (mut host, router) = fixture().await;
    let body = r#"{"request_id":"one","prompt":"hello"}"#;
    let response = send(router.clone(), "POST", "/v1/runs", body, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let start: StartResponse = serde_json::from_slice(&bytes).unwrap();
    let replay = send(router.clone(), "GET", &format!("/v1/runs/{}/events?after=0", start.run_id), "", true).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert!(replay.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let stream = replay.into_body().collect().await.unwrap().to_bytes();
    let stream = String::from_utf8(stream.to_vec()).unwrap();
    assert!(stream.contains("run/completed")); assert!(stream.contains("item/completed"));
    let final_response = send(router.clone(), "GET", &format!("/v1/runs/{}/result", start.run_id), "", true).await;
    let bytes = final_response.into_body().collect().await.unwrap().to_bytes();
    let final_result: ResultResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(final_result.outcome.unwrap().output.as_deref(), Some("ok"));
    let bad = send(router, "GET", &format!("/v1/runs/{}/events?after=18446744073709551615", start.run_id), "", true).await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST); host.shutdown().await.unwrap();
}
#[tokio::test]
async fn invalid_json_and_unknown_privileged_fields_are_rejected() {
    let (mut host, router) = fixture().await;
    assert_eq!(send(router.clone(), "POST", "/v1/runs", "{", true).await.status(), StatusCode::BAD_REQUEST);
    let body = r#"{"request_id":"one","prompt":"hello","system_prompt":"override"}"#;
    assert_eq!(send(router, "POST", "/v1/runs", body, true).await.status(), StatusCode::BAD_REQUEST);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn repeated_http_start_is_not_another_model_run() {
    let (mut host, router) = fixture().await;
    let body = r#"{"request_id":"one","prompt":"hello"}"#;
    let first = send(router.clone(), "POST", "/v1/runs", body, true).await.into_body().collect().await.unwrap().to_bytes();
    let second = send(router, "POST", "/v1/runs", body, true).await.into_body().collect().await.unwrap().to_bytes();
    let first: StartResponse = serde_json::from_slice(&first).unwrap(); let second: StartResponse = serde_json::from_slice(&second).unwrap();
    assert_eq!(first.run_id, second.run_id); assert!(second.reused); host.shutdown().await.unwrap();
}
