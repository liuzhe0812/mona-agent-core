//! Ephemeral ZCode-style side conversations. Product state only; not a second Runtime or Session store.
use crate::{environment::Environments, workspace_setup::WorkspaceSettings};
use api::{Message, RunSnapshot};
use application::{
    AgentApplication, ApplicationError, ApplicationErrorCode as Code, ApplicationResult,
    StartRequest,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key {
    parent: String,
    window: String,
}
#[derive(Clone)]
struct SideTurn {
    request_id: String,
    prompt: String,
    run_id: String,
    settled: Option<RunSnapshot>,
}
struct Thread {
    /// Trusted model history. The browser only receives safe snapshots below.
    history: Vec<Message>,
    turns: Vec<SideTurn>,
    requests: HashMap<String, (String, String)>,
    active: Option<String>,
}
pub struct Service {
    app: AgentApplication,
    environments: Arc<Environments>,
    settings: Arc<WorkspaceSettings>,
    store: Arc<sessions::Store>,
    threads: Mutex<HashMap<Key, Thread>>,
    admission: tokio::sync::Mutex<()>,
}
impl Service {
    pub fn new(
        app: AgentApplication,
        environments: Arc<Environments>,
        settings: Arc<WorkspaceSettings>,
        store: Arc<sessions::Store>,
    ) -> Arc<Self> {
        Arc::new(Self {
            app,
            environments,
            settings,
            store,
            threads: Mutex::new(HashMap::new()),
            admission: tokio::sync::Mutex::new(()),
        })
    }
    pub fn drop_parent(&self, parent: &str) {
        let removed = {
            let Ok(mut threads) = self.threads.lock() else {
                return;
            };
            let keys = threads
                .keys()
                .filter(|key| key.parent == parent)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| threads.remove(&key))
                .collect::<Vec<_>>()
        };
        for thread in removed {
            if let Some(run) = thread.active {
                let _ = self.app.cancel_task(&run);
            }
        }
    }
    fn view(&self, key: &Key) -> ApplicationResult<SideView> {
        let threads = self
            .threads
            .lock()
            .map_err(|_| internal("侧边对话状态不可用。"))?;
        let thread = threads
            .get(key)
            .ok_or_else(|| ApplicationError::new(Code::NotFound, "侧边对话不存在。"))?;
        let turns = thread
            .turns
            .iter()
            .map(|turn| SideTurnView {
                request_id: turn.request_id.clone(),
                prompt: turn.prompt.clone(),
                run_id: turn.run_id.clone(),
                snapshot: self
                    .app
                    .get_snapshot(&turn.run_id)
                    .ok()
                    .or_else(|| turn.settled.clone()),
            })
            .collect();
        Ok(SideView {
            parent_session: key.parent.clone(),
            window_id: key.window.clone(),
            turns,
        })
    }
}
fn internal(message: &str) -> ApplicationError {
    ApplicationError::new(Code::Internal, message)
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[derive(Serialize)]
struct SideView {
    parent_session: String,
    window_id: String,
    turns: Vec<SideTurnView>,
}
#[derive(Serialize)]
struct SideTurnView {
    request_id: String,
    prompt: String,
    run_id: String,
    snapshot: Option<RunSnapshot>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRequest {
    window_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnRequest {
    window_id: String,
    request_id: String,
    prompt: String,
}
#[derive(Serialize)]
struct StartView {
    run_id: String,
    reused: bool,
}

#[derive(Clone)]
struct StateData {
    service: Arc<Service>,
}
struct Auth(String);
struct HttpError(ApplicationError);
impl From<ApplicationError> for HttpError {
    fn from(value: ApplicationError) -> Self {
        Self(value)
    }
}
impl From<sessions::SessionError> for HttpError {
    fn from(value: sessions::SessionError) -> Self {
        let code = match value.code {
            sessions::SessionErrorCode::InvalidRequest => Code::InvalidRequest,
            sessions::SessionErrorCode::NotFound => Code::NotFound,
            sessions::SessionErrorCode::Conflict => Code::Conflict,
            sessions::SessionErrorCode::Capacity => Code::Capacity,
            _ => Code::Internal,
        };
        Self(ApplicationError::new(code, value.message))
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            Code::InvalidRequest => StatusCode::BAD_REQUEST,
            Code::NotFound => StatusCode::NOT_FOUND,
            Code::Conflict | Code::Closed => StatusCode::CONFLICT,
            Code::Capacity => StatusCode::TOO_MANY_REQUESTS,
            Code::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}
async fn authenticate(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .is_some_and(|v| bool::from(v.as_bytes().ct_eq(auth.0.as_bytes())));
    let mut response = if valid {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApplicationError::new(
                Code::InvalidRequest,
                "valid bearer authorization is required",
            )),
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

pub fn router(
    service: Arc<Service>,
    token: String,
    origin: Option<String>,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    if !(32..=512).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("invalid side-conversation bearer token".into());
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::POST, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !(origin.starts_with("http://") || origin.starts_with("https://")) {
            return Err("side conversation requires explicit CORS origin".into());
        }
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }
    Ok(Router::new()
        .route("/api/sessions/{id}/side", post(open))
        .route("/api/sessions/{id}/side/turns", post(start))
        .route(
            "/api/sessions/{id}/side/{window}",
            axum::routing::delete(close),
        )
        .with_state(StateData { service })
        .layer(DefaultBodyLimit::max(96 * 1024))
        .layer(middleware::from_fn_with_state(
            Arc::new(Auth(token)),
            authenticate,
        ))
        .layer(cors))
}

async fn open(
    State(s): State<StateData>,
    Path(parent): Path<String>,
    Json(value): Json<OpenRequest>,
) -> Result<Json<SideView>, HttpError> {
    if !valid_id(&parent) || !valid_id(&value.window_id) {
        return Err(ApplicationError::new(Code::InvalidRequest, "无效的侧边对话身份。").into());
    }
    let _admission = s.service.admission.lock().await;
    let key = Key {
        parent: parent.clone(),
        window: value.window_id,
    };
    if s.service
        .threads
        .lock()
        .map_err(|_| internal("侧边对话状态不可用。"))?
        .contains_key(&key)
    {
        return Ok(Json(s.service.view(&key)?));
    }
    let store = s.service.store.clone();
    let parent_for_read = parent.clone();
    let history = tokio::task::spawn_blocking(move || store.get(&parent_for_read)?.history())
        .await
        .map_err(|_| internal("主会话读取失败。"))??;
    if serde_json::to_vec(&history)
        .map_err(|_| internal("上下文读取失败。"))?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(ApplicationError::new(
            Code::Capacity,
            "主会话原文过长，未创建侧边副本；请在主任务中继续。",
        )
        .into());
    }
    let mut threads = s
        .service
        .threads
        .lock()
        .map_err(|_| internal("侧边对话状态不可用。"))?;
    if threads.len() >= 8 {
        return Err(ApplicationError::new(
            Code::Capacity,
            "最多保留 8 个侧边对话，请先关闭其他旁支。",
        )
        .into());
    }
    threads.entry(key.clone()).or_insert_with(|| Thread {
        history,
        turns: Vec::new(),
        requests: HashMap::new(),
        active: None,
    });
    drop(threads);
    Ok(Json(s.service.view(&key)?))
}

async fn start(
    State(s): State<StateData>,
    Path(parent): Path<String>,
    Json(value): Json<TurnRequest>,
) -> Result<Json<StartView>, HttpError> {
    // Complete admission even if the requesting HTTP client disconnects.
    tokio::spawn(start_owned(s, parent, value))
        .await
        .map_err(|_| HttpError(internal("启动结果未确认，请刷新旁支。")))?
}
async fn start_owned(
    s: StateData,
    parent: String,
    value: TurnRequest,
) -> Result<Json<StartView>, HttpError> {
    let _admission = s.service.admission.lock().await;
    if !valid_id(&parent)
        || !valid_id(&value.window_id)
        || !valid_id(&value.request_id)
        || value.prompt.trim().is_empty()
        || value.prompt.len() > 64 * 1024
    {
        return Err(ApplicationError::new(Code::InvalidRequest, "无效的侧边对话请求。").into());
    }
    let key = Key {
        parent: parent.clone(),
        window: value.window_id.clone(),
    };
    {
        let threads = s
            .service
            .threads
            .lock()
            .map_err(|_| internal("侧边对话状态不可用。"))?;
        let thread = threads
            .get(&key)
            .ok_or_else(|| ApplicationError::new(Code::NotFound, "请先打开侧边对话。"))?;
        if let Some((run, prompt)) = thread.requests.get(&value.request_id) {
            if prompt != &value.prompt {
                return Err(ApplicationError::new(
                    Code::Conflict,
                    "同一请求标识不能更改旁支内容。",
                )
                .into());
            }
            return Ok(Json(StartView {
                run_id: run.clone(),
                reused: true,
            }));
        }
        if thread.turns.len() >= 16 {
            return Err(ApplicationError::new(
                Code::Capacity,
                "此临时旁支达到 16 轮上限，请结束旁支后在主会话继续。",
            )
            .into());
        }
        if thread.active.is_some() {
            return Err(ApplicationError::new(
                Code::Conflict,
                "侧边对话仍在执行，请等待结束或先停止。",
            )
            .into());
        }
    }
    let history = {
        let threads = s
            .service
            .threads
            .lock()
            .map_err(|_| internal("侧边对话状态不可用。"))?;
        threads
            .get(&key)
            .ok_or_else(|| ApplicationError::new(Code::NotFound, "侧边对话不存在。"))?
            .history
            .clone()
    };
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{}:{}:{}", parent, value.window_id, value.request_id).as_bytes())
    );
    let response = s
        .service
        .environments
        .start_ephemeral_with_history(
            s.service.settings.clone(),
            s.service.app.clone(),
            parent.clone(),
            format!("side-{}", &digest[..32]),
            StartRequest {
                request_id: format!("side-{}", digest),
                prompt: value.prompt.clone(),
            },
            history,
        )
        .await?;
    {
        let mut threads = s
            .service
            .threads
            .lock()
            .map_err(|_| internal("侧边对话状态不可用。"))?;
        let Some(thread) = threads.get_mut(&key) else {
            let _ = s.service.app.cancel_task(&response.run_id);
            return Err(
                ApplicationError::new(Code::Closed, "主会话已关闭，旁支已请求取消。").into(),
            );
        };
        thread.active = Some(response.run_id.clone());
        thread.requests.insert(
            value.request_id.clone(),
            (response.run_id.clone(), value.prompt.clone()),
        );
        thread.turns.push(SideTurn {
            request_id: value.request_id,
            prompt: value.prompt,
            run_id: response.run_id.clone(),
            settled: None,
        });
    }
    let service = s.service.clone();
    let run_id = response.run_id.clone();
    tokio::spawn(async move {
        let report = service.app.wait_report(&run_id).await;
        let snapshot = service.app.get_snapshot(&run_id).ok();
        let Ok(mut threads) = service.threads.lock() else {
            return;
        };
        let Some(thread) = threads.get_mut(&key) else {
            return;
        };
        if let Some(turn) = thread.turns.iter_mut().find(|turn| turn.run_id == run_id) {
            turn.settled = snapshot;
        }
        if thread.active.as_deref() == Some(&run_id) {
            thread.active = None;
        }
        if let Ok(report) = report {
            thread.history = report.transcript.clone();
        }
    });
    Ok(Json(StartView {
        run_id: response.run_id,
        reused: response.reused,
    }))
}

async fn close(
    State(s): State<StateData>,
    Path((parent, window)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    if !valid_id(&parent) || !valid_id(&window) {
        return Err(ApplicationError::new(Code::InvalidRequest, "无效的侧边对话身份。").into());
    }
    let _admission = s.service.admission.lock().await;
    let thread = s
        .service
        .threads
        .lock()
        .map_err(|_| internal("侧边对话状态不可用。"))?
        .remove(&Key { parent, window });
    if let Some(run) = thread.and_then(|thread| thread.active) {
        let _ = s.service.app.cancel_task(&run);
    }
    Ok(StatusCode::NO_CONTENT)
}
