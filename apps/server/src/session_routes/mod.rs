//! Web-only session routes, safe projections and deployment path selection.
mod view;
#[cfg(feature = "mcp")] mod mcp;
#[cfg(feature = "subagent")] mod subagents;
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
use std::{path::Path as FsPath, sync::Arc};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

fn error(code: Code, message: &str) -> ApplicationError { ApplicationError::new(code, message) }
async fn disk<T: Send + 'static>(action: impl FnOnce() -> SessionResult<T> + Send + 'static) -> ApplicationResult<T> {
    tokio::task::spawn_blocking(action).await
        .map_err(|_| error(Code::Internal, "会话存储任务失败。"))?.map_err(Into::into)
}

pub fn from_environment(workspace: &FsPath, demo: bool) -> SessionResult<Arc<Store>> {
    let base = crate::workspace_setup::session_base(demo)
        .map_err(|e| SessionError::new(SessionErrorCode::InvalidRequest,e.message))?;
    Store::open(&base, workspace)
}

#[derive(Clone)]
struct Service {
    store: Arc<Store>,
    tasks: SessionApplication,
    workspaces: Option<Arc<crate::workspace_routes::Service>>,
    side: Option<Arc<crate::side_routes::Service>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create { request_id: String, project_id: Option<String> }
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
struct ListQuery { offset: Option<usize>, limit: Option<usize>, q: Option<String>, archived: Option<bool>, project_id: Option<String> }
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery { before: Option<usize>, limit: Option<usize> }
struct Auth(String);
struct HttpError(ApplicationError);
impl From<ApplicationError> for HttpError { fn from(e: ApplicationError) -> Self { Self(e) } }
impl From<workspace::Error> for HttpError {
    fn from(e: workspace::Error) -> Self {
        let code = match e.code.as_str() { "conflict"=>Code::Conflict,"not_found"=>Code::NotFound,"capacity"=>Code::Capacity,"io"=>Code::Internal,_=>Code::InvalidRequest };
        Self(error(code,&e.message))
    }
}
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
pub fn router(
    store: Arc<Store>,
    app: AgentApplication,
    token: String,
    origin: Option<String>,
    workspaces: Option<Arc<crate::workspace_routes::Service>>,
    side: Option<Arc<crate::side_routes::Service>>,
) -> std::result::Result<Router, Box<dyn std::error::Error>> {
    if !(32..=512).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) { return Err("invalid session bearer token".into()); }
    let mut cors = CorsLayer::new().allow_methods([Method::GET, Method::POST]).allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !crate::ui_origin_allowed(&origin) { return Err("session CORS needs an explicit origin".into()); }
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }
    let tasks = SessionApplication::new(store.clone(), app);
    #[cfg(feature = "planner")]
    let context = crate::planning::context(workspaces.as_ref().is_some_and(|w|
        !w.environments.factory.demo && w.environments.factory.capabilities.active(crate::capabilities::PLANNER)));
    #[cfg(not(feature = "planner"))]
    let context: application::sessions::SessionContext = Arc::new(|doc| {
        if doc.body.state.get("planner").and_then(|v| v.get("mode")).and_then(|v| v.as_str()) == Some("plan_only") {
            return Err(SessionError::new(SessionErrorCode::InvalidRequest, "此会话处于计划模式，但当前构建不含 Planner。"));
        }
        Ok(std::collections::BTreeMap::new())
    });
    #[cfg(feature = "sandbox")]
    let sandbox = workspaces.as_ref().map(|w| (w.environments.factory.sandbox.clone(),
        !w.environments.factory.demo && w.environments.factory.capabilities.active(crate::capabilities::SANDBOX)));
    let tasks = tasks.with_context(Arc::new(move |doc| {
        if doc.header.metadata.contains_key("subagent.root") {
            return Err(SessionError::new(SessionErrorCode::InvalidRequest, "子会话只能通过所属主任务委派继续，不能绕过预算独立启动。"));
        }
        #[allow(unused_mut)]
        let mut metadata = context(doc)?;
        #[cfg(feature = "sandbox")]
        if let Some((sandbox, active)) = &sandbox { metadata.extend(sandbox.metadata(doc, *active)?); }
        else { crate::sandbox_setup::require_unconfined(doc)?; }
        #[cfg(not(feature = "sandbox"))]
        crate::sandbox_setup::require_unconfined(doc)?;
        Ok(metadata)
    }));
    let router = Router::new();
    #[cfg(feature = "mcp")]
    let router = router.route("/api/mcp", get(mcp::settings))
        .route("/api/mcp/servers", post(mcp::upsert))
        .route("/api/mcp/delete", post(mcp::remove))
        .route("/api/mcp/reconnect", post(mcp::reconnect));
    #[cfg(feature = "subagent")]
    let router = router.route("/api/subagents", get(subagents::settings).post(subagents::save_settings))
        .route("/api/sessions/{id}/agents", get(subagents::list))
        .route("/api/sessions/{id}/agents/{child}", get(subagents::detail))
        .route("/api/sessions/{id}/agents/{child}/interrupt", post(subagents::interrupt))
        .route("/api/sessions/{id}/agents/{child}/message", post(subagents::message))
        .route("/api/sessions/{id}/agents/{child}/followup", post(subagents::followup));
    #[cfg(feature = "planner")]
    let router = router.route("/api/sessions/{id}/plan", get(get_plan).post(change_plan));
    #[cfg(feature = "sandbox")]
    let router = router.route("/api/sandbox", get(default_sandbox))
        .route("/api/sessions/{id}/sandbox", get(get_sandbox).post(change_sandbox));
    Ok(router
        .route("/api/sessions", get(list).post(create))
        .route("/api/sessions/{id}", get(detail))
        .route("/api/sessions/{id}/turns", post(start))
        .route("/api/sessions/{id}/turns/{turn}", get(turn_detail))
        .route("/api/sessions/{id}/rename", post(rename))
        .route("/api/sessions/{id}/pin", post(pin))
        .route("/api/sessions/{id}/archive", post(archive))
        .route("/api/sessions/{id}/unread", post(unread))
        .route("/api/sessions/{id}/delete", post(delete))
        .with_state(Service { tasks, store, workspaces, side })
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn_with_state(Arc::new(Auth(token)), authenticate)).layer(cors))
}
#[cfg(feature = "planner")]
fn planner_enabled(s: &Service) -> bool {
    s.workspaces.as_ref().is_some_and(|w| !w.environments.factory.demo
        && w.environments.factory.capabilities.active(crate::capabilities::PLANNER))
}
#[cfg(feature = "planner")]
async fn get_plan(State(s): State<Service>, Path(id): Path<String>) -> std::result::Result<Json<crate::planning::PlanView>, HttpError> {
    let enabled = planner_enabled(&s);
    Ok(Json(disk(move || crate::planning::view(s.store.get(&id)?, enabled)).await?))
}
#[cfg(feature = "planner")]
async fn change_plan(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<crate::planning::PlanAction>, JsonRejection>) -> std::result::Result<Json<crate::planning::PlanView>, HttpError> {
    let request = body(value)?;
    let enabled = planner_enabled(&s);
    Ok(Json(disk(move || {
        crate::planning::change(&s.store, &id, request, enabled)?;
        crate::planning::view(s.store.get(&id)?, enabled)
    }).await?))
}
#[cfg(feature = "sandbox")]
fn sandbox_host(s: &Service) -> std::result::Result<(Arc<crate::sandbox_setup::SandboxHost>, bool), HttpError> {
    let factory = &s.workspaces.as_ref().ok_or_else(|| error(Code::NotFound, "当前宿主未装配沙箱管理。"))?.environments.factory;
    Ok((factory.sandbox.clone(), !factory.demo && factory.capabilities.active(crate::capabilities::SANDBOX)))
}
#[cfg(feature = "sandbox")]
async fn default_sandbox(State(s): State<Service>) -> std::result::Result<Json<crate::sandbox_setup::SandboxView>, HttpError> {
    let (host, active) = sandbox_host(&s)?;
    Ok(Json(host.view(None, active).await.map_err(ApplicationError::from)?))
}
#[cfg(feature = "sandbox")]
async fn get_sandbox(State(s): State<Service>, Path(id): Path<String>) -> std::result::Result<Json<crate::sandbox_setup::SandboxView>, HttpError> {
    let (host, active) = sandbox_host(&s)?;
    let doc = disk(move || s.store.get(&id)).await?;
    Ok(Json(host.view(Some(doc), active).await.map_err(ApplicationError::from)?))
}
#[cfg(feature = "sandbox")]
async fn change_sandbox(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<crate::sandbox_setup::SandboxAction>, JsonRejection>) -> std::result::Result<Json<crate::sandbox_setup::SandboxView>, HttpError> {
    let request = body(value)?;
    let (host, active) = sandbox_host(&s)?; let update = host.clone();
    let doc = disk(move || { update.change(&s.store, &id, request, active)?; s.store.get(&id) }).await?;
    Ok(Json(host.view(Some(doc), active).await.map_err(ApplicationError::from)?))
}

async fn list(State(s): State<Service>, q: std::result::Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<Listing>, HttpError> {
    let q = query(q)?;
    let archived = q.archived.unwrap_or(false);
    let project_ids = s.workspaces.as_ref().map(|w| w.project_ids()).transpose()?.unwrap_or_default();
    Ok(Json(disk(move || s.store.list_matching(q.offset.unwrap_or(0), q.limit.unwrap_or(50), q.q.as_deref().unwrap_or(""), archived, |h| {
        if h.metadata.contains_key("subagent.root") { return false; }
        match q.project_id.as_deref() {
            None => true,
            Some("") => h.metadata.get("project.id").is_none_or(|id| !project_ids.contains(id)),
            Some(id) => h.metadata.get("project.id").is_some_and(|v| v == id),
        }
    })).await?))
}
async fn create(State(s): State<Service>, value: std::result::Result<Json<Create>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?;
    if let Some(workspaces) = s.workspaces {
        let header = tokio::task::spawn_blocking(move || workspaces.create(&value.request_id,value.project_id.as_deref()))
            .await.map_err(|_| error(Code::Internal,"会话创建结果未知，请刷新确认。"))??;
        Ok(Json(header))
    } else {
        if value.project_id.is_some() { return Err(error(Code::InvalidRequest,"当前宿主未装配项目管理。").into()); }
        Ok(Json(disk(move || s.store.create(&value.request_id)).await?))
    }
}
async fn detail(State(s): State<Service>, Path(id): Path<String>, q: std::result::Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<view::SessionPage>, HttpError> {
    let q = query(q)?; Ok(Json(disk(move || view::session_page(s.store.get(&id)?, q.before, q.limit.unwrap_or(20))).await?))
}
async fn turn_detail(State(s): State<Service>, Path((id, turn)): Path<(String,String)>, q: std::result::Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>) -> std::result::Result<Json<view::TurnPage>, HttpError> {
    let q = query(q)?; Ok(Json(disk(move || view::turn_page(&s.store.get(&id)?, &turn, q.before, q.limit.unwrap_or(50))).await?))
}
async fn start(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<NewTurn>, JsonRejection>) -> std::result::Result<Json<TurnResponse>, HttpError> {
    let value = body(value)?;
    if let Some(w) = s.workspaces {
        Ok(Json(w.environments.start(w.settings.clone(),s.tasks,id,value).await?))
    } else { Ok(Json(s.tasks.start_turn(id, value).await?)) }
}
async fn rename(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Rename>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.rename(&id, value.revision, &value.title)).await?))
}
async fn delete(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Revision>, JsonRejection>) -> std::result::Result<StatusCode, HttpError> {
    let value = body(value)?;
    let sid = id.clone(); disk(move || s.store.delete(&sid, value.revision)).await?;
    #[cfg(feature = "subagent")]
    if let Some(workspaces) = &s.workspaces {
        let removed=workspaces.environments.factory.subagents.service.purge_deleted_root(&id).await
            .map_err(|e|error(Code::Internal,&format!("主记录已删除，但子记录清理失败：{}",e.message)))?;
        #[cfg(feature = "sandbox")]
        for child in removed {workspaces.environments.factory.sandbox.provider.release_session(&child).await
            .map_err(|e|error(Code::Internal,&format!("子记录已删除，但临时资源清理失败：{e}")))?;}
        #[cfg(not(feature = "sandbox"))]
        let _=removed;
    }
    #[cfg(feature = "sandbox")]
    if let Some(workspaces) = &s.workspaces {
        workspaces.environments.factory.sandbox.provider.release_session(&id).await
            .map_err(|e| error(Code::Internal, &format!("会话已删除，但沙箱临时资源清理失败：{e}")))?;
    }
    if let Some(side) = s.side { side.drop_parent(&id); }
    Ok(StatusCode::NO_CONTENT)
}
async fn pin(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_pinned(&id, value.revision, value.value)).await?))
}
async fn archive(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; let archived = value.value; let sid = id.clone();
    let header = disk(move || s.store.set_archived(&sid, value.revision, archived)).await?;
    if archived { if let Some(side) = s.side { side.drop_parent(&id); } }
    Ok(Json(header))
}
async fn unread(State(s): State<Service>, Path(id): Path<String>, value: std::result::Result<Json<Flag>, JsonRejection>) -> std::result::Result<Json<Header>, HttpError> {
    let value = body(value)?; Ok(Json(disk(move || s.store.set_unread(&id, value.revision, value.value)).await?))
}
