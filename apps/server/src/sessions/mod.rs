//! Local conversations belong to the formal host, not to the embedded runtime or UI cache.
mod store;
mod view;
pub mod references;
#[cfg(test)] mod tests;

pub use store::Store;
use store::{error, Prepared};
pub use store::{SESSION_KEY, TURN_KEY};
use api::*;
use application::{AgentApplication, ApplicationError, ApplicationErrorCode as Code, ApplicationResult, StartRequest};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, VecDeque}, sync::Arc};
use subtle::ConstantTimeEq;
use tokio::sync::{watch, Mutex};
use tower_http::cors::CorsLayer;

async fn disk<T: Send + 'static>(action: impl FnOnce() -> ApplicationResult<T> + Send + 'static) -> ApplicationResult<T> {
    tokio::task::spawn_blocking(action).await.map_err(|_| error(Code::Internal, "会话存储任务失败。"))?
}
pub struct SessionSink(pub Arc<Store>);
#[async_trait]
impl CheckpointSink for SessionSink {
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, cancel: CancellationToken) -> api::Result<()> {
        let store = self.0.clone();
        disk(move || store.commit(&checkpoint, &cancel)).await
            .map_err(|_| AgentError::new(ErrorCode::Checkpoint, "local session checkpoint was not acknowledged"))
    }
}
struct SessionRuntime { inner: Arc<dyn AgentRuntime>, store: Arc<Store> }
pub fn runtime(inner: Arc<dyn AgentRuntime>, store: Arc<Store>) -> Arc<dyn AgentRuntime> {
    Arc::new(SessionRuntime { inner, store })
}
struct SavedRun { inner: Arc<dyn RunSession>, saved: watch::Receiver<Option<api::Result<Arc<RunReport>>>> }
#[async_trait]
impl RunSession for SavedRun {
    fn run_id(&self) -> &str { self.inner.run_id() }
    fn cancel(&self) { self.inner.cancel(); }
    fn snapshot(&self) -> RunSnapshot { self.inner.snapshot() }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventEnvelope> { self.inner.subscribe() }
    async fn steer(&self, text: String) -> api::Result<()> { self.inner.steer(text).await }
    async fn wait(&self) -> api::Result<Arc<RunReport>> {
        let mut saved = self.saved.clone();
        loop {
            if let Some(result) = saved.borrow().clone() { return result; }
            saved.changed().await.map_err(|_| AgentError::new(ErrorCode::Checkpoint, "session finalization unavailable"))?;
        }
    }
}
#[async_trait]
impl AgentExecutor for SessionRuntime {
    async fn execute(&self, request: RunRequest) -> api::Result<Arc<RunReport>> {
        self.start(request)?.wait_owned().await
    }
}
impl AgentRuntime for SessionRuntime {
    fn start(&self, request: RunRequest) -> api::Result<RunHandle> {
        let binding = request.metadata.get(SESSION_KEY).zip(request.metadata.get(TURN_KEY)).map(|(a,b)| (a.clone(),b.clone()));
        let handle = self.inner.start(request)?;
        let Some((id, key)) = binding else { return Ok(handle) };
        let (inner, events) = handle.into_parts();
        let waiting = inner.clone(); let store = self.store.clone();
        let (tx, rx) = watch::channel(None);
        // This is lifecycle settlement, not a second model/tool loop. HTTP disconnect does not own it.
        tokio::spawn(async move {
            let result = waiting.wait().await;
            if let Ok(report) = &result {
                let report = report.clone();
                if disk(move || store.finish(&id, &key, &report)).await.is_err() {
                    eprintln!("local session finalization failed; retained checkpoint remains authoritative");
                }
            }
            tx.send_replace(Some(result));
        });
        Ok(RunHandle::new(Arc::new(SavedRun { inner, saved: rx }), events))
    }
}

#[derive(Clone)]
struct Service {
    store: Arc<Store>, app: AgentApplication,
    // Short admission transaction only; never held across a model or tool execution.
    starts: Arc<Mutex<VecDeque<String>>>,
}
#[derive(Serialize)]
struct TurnResponse { session: store::Header, turn_id: String, run_id: Option<String>, reused: bool, live: bool }
impl Service {
    async fn start(&self, id: String, request: NewTurn) -> ApplicationResult<TurnResponse> {
        let mut retained = self.starts.lock().await;
        // Durable history is not tied to the application's short in-memory replay window.
        retained.retain(|run| match self.app.get_result(run) {
            Ok(result) if result.outcome.as_ref().is_some_and(|o| o.error.as_ref().is_none_or(|e| e.code != ErrorCode::Checkpoint)) => {
                if result.outcome.is_some() { let _ = self.app.forget(run); false } else { true }
            }
            Err(e) if e.code == Code::NotFound => false,
            _ => true,
        });
        let store = self.store.clone(); let sid = id.clone(); let req = request.clone();
        let prepared = disk(move || store.prepare(&sid, req.revision, &req.request_id, &req.prompt)).await?;
        if let Prepared::Existing(turn, session) = prepared {
            let live = turn.run_id.as_ref().is_some_and(|run| self.app.get_snapshot(run).is_ok());
            return Ok(TurnResponse { session, turn_id: turn.id, run_id: turn.run_id, reused: true, live });
        }
        let Prepared::New { history } = prepared else { unreachable!() };
        let metadata = BTreeMap::from([(SESSION_KEY.into(), id.clone()), (TURN_KEY.into(), request.request_id.clone())]);
        use sha2::{Digest, Sha256};
        let request_key = format!("session-{:x}", Sha256::digest(format!("{}:{}", id, request.request_id).as_bytes()));
        let started = self.app.start_task_with_history(StartRequest {
            request_id: request_key, prompt: request.prompt,
        }, history, metadata);
        let response = match started {
            Ok(response) => response,
            Err(failure) => {
                let store = self.store.clone(); let sid = id.clone(); let key = request.request_id.clone();
                let message = failure.message.clone();
                disk(move || store.fail_start(&sid, &key, &message)).await?;
                return Err(failure);
            }
        };
        let run_id = response.run_id.clone(); let store = self.store.clone(); let key = request.request_id.clone();
        let session = match disk(move || store.bind(&id, &key, &run_id)).await {
            Ok(header) => header,
            Err(failure) => { let _ = self.app.cancel_task(&response.run_id); return Err(failure); }
        };
        retained.push_back(response.run_id.clone());
        Ok(TurnResponse { session, turn_id: request.request_id, run_id: Some(response.run_id), reused: false, live: true })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create { request_id: String }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewTurn { request_id: String, revision: u64, prompt: String }
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
        .with_state(Service { store, app, starts: Arc::new(Mutex::new(VecDeque::new())) })
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn_with_state(Arc::new(Auth(token)), authenticate)).layer(cors))
}
async fn list(State(s): State<Service>, q: std::result::Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<store::Listing>, HttpError> {
    let q = query(q)?;
    let archived = q.archived.unwrap_or(false);
    Ok(Json(disk(move || s.store.list(q.offset.unwrap_or(0), q.limit.unwrap_or(50), q.q.as_deref().unwrap_or(""), archived)).await?))
}
async fn create(State(s): State<Service>, value: std::result::Result<Json<Create>, JsonRejection>) -> std::result::Result<Json<store::Header>, HttpError> {
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
    // Disconnect cannot cancel the interval between the durable input receipt and runtime start.
    let result = tokio::spawn(async move { s.start(id, value).await }).await
        .map_err(|_| error(Code::Internal, "会话启动结果未知，请刷新历史确认，不要重复提交。"))??;
    Ok(Json(result))
}
async fn rename(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Rename>, JsonRejection>) -> std::result::Result<Json<store::Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.rename(&id, value.revision, &value.title)).await?))
}
async fn delete(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Revision>, JsonRejection>) -> std::result::Result<StatusCode, HttpError> {
    let value = body(value)?; disk(move || s.store.delete(&id, value.revision)).await?; Ok(StatusCode::NO_CONTENT)
}
async fn pin(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<store::Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_pinned(&id, value.revision, value.value)).await?))
}
async fn archive(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<store::Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_archived(&id, value.revision, value.value)).await?))
}
async fn unread(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<store::Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_unread(&id, value.revision, value.value)).await?))
}
