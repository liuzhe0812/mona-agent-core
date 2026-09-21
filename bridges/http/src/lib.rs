//! Optional Axum router adapter. No listener is opened by this library.
//! Uses the same AgentApplication as the Tauri bridge; contains no ReAct loop.
#![forbid(unsafe_code)]
use agent_application::*;
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State, rejection::JsonRejection},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next}, response::{IntoResponse, Response, sse::{Event, KeepAlive, Sse}},
    routing::{delete, get, post}, Json, Router,
};
use futures_util::{Stream, stream};
use serde::Deserialize;
use std::{convert::Infallible, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

/// A token identifies ONE authority domain/AgentApplication, not a multi-tenant identity system.
/// For remote access terminate TLS and enforce user identity in the host deployment.
pub struct HttpConfig {
    pub bearer_token: String,
    pub allowed_origins: Vec<String>,
    pub max_body_bytes: usize,
}
impl HttpConfig {
    pub fn new(token: impl Into<String>) -> Self {
        Self { bearer_token: token.into(), allowed_origins: vec![], max_body_bytes: 1024 * 1024 }
    }
}
struct Auth { token: String }
#[derive(Clone)]
struct BridgeState { app: AgentApplication }

pub fn router(app: AgentApplication, config: HttpConfig) -> ApplicationResult<Router> {
    if config.bearer_token.len() < 32 || config.bearer_token.len() > 512
        || !config.bearer_token.bytes().all(|c| c.is_ascii_graphic()) || config.max_body_bytes == 0 {
        return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "HTTP requires a 32..512 character bearer token and a positive body limit"));
    }
    let mut origins = vec![];
    for origin in config.allowed_origins {
        if origin == "*" || !(origin.starts_with("https://") || origin.starts_with("http://")) {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "CORS requires explicit HTTP(S) origins"));
        }
        origins.push(HeaderValue::from_str(&origin).map_err(|_| ApplicationError::new(ApplicationErrorCode::InvalidRequest, "invalid CORS origin"))?);
    }
    let mut cors = CorsLayer::new().allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, HeaderName::from_static("last-event-id")]);
    if !origins.is_empty() { cors = cors.allow_origin(origins); }
    let auth = Arc::new(Auth { token: config.bearer_token });
    Ok(Router::new()
        .route("/v1/info", get(info))
        .route("/v1/runs", post(start))
        .route("/v1/runs/{id}", delete(forget))
        .route("/v1/runs/{id}/cancel", post(cancel))
        .route("/v1/runs/{id}/input", post(input))
        .route("/v1/runs/{id}/snapshot", get(snapshot))
        .route("/v1/runs/{id}/result", get(result))
        .route("/v1/runs/{id}/events", get(events))
        .with_state(BridgeState { app })
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(middleware::from_fn_with_state(auth, authenticate))
        // CORS is outermost so authenticated browsers can preflight without a token on OPTIONS.
        .layer(cors))
}
async fn authenticate(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let token = request.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let authorized = token.is_some_and(|t| t.as_bytes().ct_eq(auth.token.as_bytes()).into());
    if !authorized {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"code":"unauthorized","message":"valid bearer authorization is required"}))).into_response();
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    response
}
struct HttpError(ApplicationError);
impl From<ApplicationError> for HttpError { fn from(e: ApplicationError) -> Self { Self(e) } }
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            ApplicationErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
            ApplicationErrorCode::NotFound => StatusCode::NOT_FOUND,
            ApplicationErrorCode::Conflict | ApplicationErrorCode::Closed => StatusCode::CONFLICT,
            ApplicationErrorCode::Capacity => StatusCode::TOO_MANY_REQUESTS,
            ApplicationErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}
fn json_body<T>(body: std::result::Result<Json<T>, JsonRejection>) -> std::result::Result<T, HttpError> {
    body.map(|Json(value)| value).map_err(|_| ApplicationError::new(ApplicationErrorCode::InvalidRequest, "invalid or oversized JSON request body").into())
}
async fn info() -> Json<serde_json::Value> {
    Json(serde_json::json!({"protocol_version": agent_api::STREAM_VERSION,
        "stream":"sse", "replay":"bounded_in_memory", "durable": false}))
}
async fn start(State(state): State<BridgeState>, body: std::result::Result<Json<StartRequest>, JsonRejection>) -> std::result::Result<Json<StartResponse>, HttpError> {
    Ok(Json(state.app.start_task(json_body(body)?)?))
}
async fn cancel(State(state): State<BridgeState>, Path(id): Path<String>) -> std::result::Result<Json<CancelReceipt>, HttpError> {
    Ok(Json(state.app.cancel_task(&id)?))
}
async fn input(State(state): State<BridgeState>, Path(id): Path<String>, body: std::result::Result<Json<InputRequest>, JsonRejection>) -> std::result::Result<Json<InputReceipt>, HttpError> {
    Ok(Json(state.app.send_input(&id, json_body(body)?).await?))
}
async fn snapshot(State(state): State<BridgeState>, Path(id): Path<String>) -> std::result::Result<Json<agent_api::RunSnapshot>, HttpError> {
    Ok(Json(state.app.get_snapshot(&id)?))
}
async fn result(State(state): State<BridgeState>, Path(id): Path<String>) -> std::result::Result<Json<ResultResponse>, HttpError> {
    Ok(Json(state.app.get_result(&id)?))
}
async fn forget(State(state): State<BridgeState>, Path(id): Path<String>) -> std::result::Result<StatusCode, HttpError> {
    state.app.forget(&id)?; Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventQuery { after: Option<u64> }
async fn events(State(state): State<BridgeState>, Path(id): Path<String>, query: std::result::Result<Query<EventQuery>, axum::extract::rejection::QueryRejection>, headers: HeaderMap)
    -> std::result::Result<Response, HttpError> {
    let Query(query) = query.map_err(|_| ApplicationError::new(ApplicationErrorCode::InvalidRequest, "invalid cursor query"))?;
    let header_cursor = headers.get("last-event-id").map(|value| {
        value.to_str().ok().and_then(|v| v.parse::<u64>().ok()).ok_or_else(|| ApplicationError::new(ApplicationErrorCode::InvalidRequest, "Last-Event-ID must be an unsigned sequence"))
    }).transpose()?;
    if query.after.is_some() && header_cursor.is_some() && query.after != header_cursor {
        return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "query cursor and Last-Event-ID disagree").into());
    }
    let subscription = state.app.subscribe_events(&id, header_cursor.or(query.after))?;
    let mut response = Sse::new(sse_stream(subscription))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive")).into_response();
    response.headers_mut().insert(HeaderName::from_static("x-accel-buffering"), HeaderValue::from_static("no"));
    Ok(response)
}
fn sse_stream(subscription: Subscription) -> impl Stream<Item = std::result::Result<Event, Infallible>> + Send {
    stream::unfold(subscription, |mut subscription| async move {
        let frame = subscription.next().await?;
        let mut event = Event::default().event("agent");
        if let Some(seq) = frame.sequence() { event = event.id(seq.to_string()); }
        let event = event.json_data(&frame).unwrap_or_else(|_| Event::default().event("agent").data(
            r#"{"kind":"fault","error":{"code":"internal","message":"event serialization failed"}}"#));
        Some((Ok(event), subscription))
    })
}
