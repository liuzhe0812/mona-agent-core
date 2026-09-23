//! Web-only session routes, safe projections and deployment path selection.
mod view;
#[cfg(test)] mod tests;

use sessions::{Store, Header, Listing, SessionError, SessionErrorCode, SessionResult};
use application::{AgentApplication, ApplicationError, ApplicationErrorCode as Code, ApplicationResult};
use application::sessions::{SessionApplication, TurnRequest as NewTurn, TurnResponse};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router,
};
use serde::Deserialize;
use std::{path::{Path as FsPath, PathBuf}, sync::Arc};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

fn error(code: Code, message: &str) -> ApplicationError { ApplicationError::new(code, message) }
async fn disk<T: Send + 'static>(action: impl FnOnce() -> SessionResult<T> + Send + 'static) -> ApplicationResult<T> {
    tokio::task::spawn_blocking(action).await
        .map_err(|_| error(Code::Internal, "会话存储任务失败。"))?.map_err(Into::into)
}

pub fn from_environment(workspace: &FsPath, demo: bool) -> SessionResult<Arc<Store>> {
    let base = match std::env::var_os("AGENT_SESSIONS_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        Some(_) => return Err(SessionError::new(SessionErrorCode::InvalidRequest, "AGENT_SESSIONS_DIR 不能为空。")),
        None => std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("XDG_STATE_HOME"))
            .map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
            .ok_or_else(|| SessionError::new(SessionErrorCode::InvalidRequest, "请为此宿主设置 AGENT_SESSIONS_DIR。"))?
            .join("mona-agent-core").join(if demo { "demo-sessions" } else { "sessions" }),
    };
    Store::open(&base, workspace)
}

#[derive(Clone)]
struct Service { store: Arc<Store>, tasks: SessionApplication }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create { request_id: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rename { revision: u64, title: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision { revision: u64 }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Flag { revision: u64, value: bool }
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery { offset: Option<usize>, limit: Option<usize>, q: Option<String>, archived: Option<bool> }
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery { before: Option<usize>, limit: Option<usize> }
struct Auth(String);
struct HttpError(ApplicationError);
impl From<ApplicationError> for HttpError { fn from(e: ApplicationError) -> Self { Self(e) } }
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            Code::InvalidRequest => StatusCode::BAD_REQUEST, Code::NotFound => StatusCode::NOT_FOUND,
            Code::Conflict | Code::Closed => StatusCode::CONFLICT, Code::Capacity => StatusCode::TOO_MANY_REQUESTS,
            Code::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}
fn body<T>(value: std::result::Result<Json<T>, JsonRejection>) -> std::result::Result<T, HttpError> {
    value.map(|Json(v)| v).map_err(|_| error(Code::InvalidRequest, "无效或过大的会话请求。").into())
}
fn query<T>(value: std::result::Result<Query<T>, axum::extract::rejection::QueryRejection>) -> std::result::Result<T, HttpError> {
    value.map(|Query(v)| v).map_err(|_| error(Code::InvalidRequest, "无效的分页参数。").into())
}
async fn authenticate(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let valid = request.headers().get(header::AUTHORIZATION).and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ")).is_some_and(|t| bool::from(t.as_bytes().ct_eq(auth.0.as_bytes())));
    let mut response = if valid { next.run(request).await } else {
        (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"code":"unauthorized", "message":"valid bearer authorization is required"}))).into_response()
    };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")); response
}
pub fn router(store: Arc<Store>, app: AgentApplication, token: String, origin: Option<String>) -> std::result::Result<Router, Box<dyn std::error::Error>> {
    if !(32..=512).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) { return Err("invalid session bearer token".into()); }
    let mut cors = CorsLayer::new().allow_methods([Method::GET, Method::POST]).allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !(origin.starts_with("http://") || origin.starts_with("https://")) { return Err("session CORS needs an explicit origin".into()); }
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }
    Ok(Router::new()
        .route("/api/sessions", get(list).post(create))
        .route("/api/sessions/{id}", get(detail))
        .route("/api/sessions/{id}/turns", post(start))
        .route("/api/sessions/{id}/turns/{turn}", get(turn_detail))
        .route("/api/sessions/{id}/rename", post(rename))
        .route("/api/sessions/{id}/pin", post(pin))
        .route("/api/sessions/{id}/archive", post(archive))
        .route("/api/sessions/{id}/unread", post(unread))
        .route("/api/sessions/{id}/delete", post(delete))
        .with_state(Service { tasks: SessionApplication::new(store.clone(), app), store })
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn_with_state(Arc::new(Auth(token)), authenticate)).layer(cors))
}
async fn list(State(s): State<Service>, q: std::result::Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<Listing>, HttpError> {
    let q = query(q)?;
    let archived = q.archived.unwrap_or(false);
    Ok(Json(disk(move || s.store.list(q.offset.unwrap_or(0), q.limit.unwrap_or(50), q.q.as_deref().unwrap_or(""), archived)).await?))
}
async fn create(State(s): State<Service>, value: std::result::Result<Json<Create>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.create(&value.request_id)).await?))
}
async fn detail(State(s): State<Service>, Path(id): Path<String>, q: std::result::Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<view::SessionPage>, HttpError> {
    let q = query(q)?; Ok(Json(disk(move || view::session_page(s.store.get(&id)?, q.before, q.limit.unwrap_or(20))).await?))
}
async fn turn_detail(State(s): State<Service>, Path((id, turn)): Path<(String,String)>, q: std::result::Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<view::TurnPage>, HttpError> {
    let q = query(q)?; Ok(Json(disk(move || view::turn_page(&s.store.get(&id)?, &turn, q.before, q.limit.unwrap_or(50))).await?))
}
async fn start(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<NewTurn>, JsonRejection>) -> std::result::Result<Json<TurnResponse>, HttpError> {
    let value = body(value)?;
    Ok(Json(s.tasks.start_turn(id, value).await?))
}
async fn rename(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Rename>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.rename(&id, value.revision, &value.title)).await?))
}
async fn delete(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Revision>, JsonRejection>) -> std::result::Result<StatusCode, HttpError> {
    let value = body(value)?; disk(move || s.store.delete(&id, value.revision)).await?; Ok(StatusCode::NO_CONTENT)
}
async fn pin(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_pinned(&id, value.revision, value.value)).await?))
}
async fn archive(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_archived(&id, value.revision, value.value)).await?))
}
async fn unread(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_unread(&id, value.revision, value.value)).await?))
}
