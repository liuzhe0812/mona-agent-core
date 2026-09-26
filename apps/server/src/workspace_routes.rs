//! Authenticated host adapters for default directories, optional projects and read-only files.
use crate::{environment::Environments, workspace_setup::WorkspaceSettings};
#[cfg(feature = "projects")]
use axum::routing::post;
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;
use workspace::{error, Result};

pub struct Service {
    pub settings: Arc<WorkspaceSettings>,
    pub store: Arc<sessions::Store>,
    pub environments: Arc<Environments>,
    #[cfg(feature = "projects")]
    projects: Option<Arc<projects::Registry>>,
    readers: Arc<tokio::sync::Semaphore>,
}
impl Service {
    pub fn new(
        settings: Arc<WorkspaceSettings>,
        store: Arc<sessions::Store>,
        environments: Arc<Environments>,
        demo: bool,
    ) -> Result<Arc<Self>> {
        #[cfg(feature = "projects")]
        let projects = if !demo && std::env::var("AGENT_PROJECTS").as_deref() != Ok("0") {
            let path = std::env::var_os("AGENT_PROJECTS_PATH")
                .map(PathBuf::from)
                .unwrap_or(crate::workspace_setup::state_root()?.join("projects.json"));
            Some(Arc::new(projects::Registry::open(&path)?))
        } else {
            None
        };
        #[cfg(not(feature = "projects"))]
        let _ = demo;
        Ok(Arc::new(Self {
            settings,
            store,
            environments,
            #[cfg(feature = "projects")]
            projects,
            readers: Arc::new(tokio::sync::Semaphore::new(4)),
        }))
    }
    pub fn projects_enabled(&self) -> bool {
        #[cfg(feature = "projects")]
        {
            self.projects.is_some()
        }
        #[cfg(not(feature = "projects"))]
        {
            false
        }
    }
    pub fn project_ids(&self) -> Result<Vec<String>> {
        #[cfg(feature = "projects")]
        if let Some(p) = &self.projects {
            return Ok(p.list()?.projects.into_iter().map(|p| p.id).collect());
        }
        Ok(Vec::new())
    }
    #[cfg(feature = "projects")]
    fn project_files(&self, id: &str) -> Result<workspace::Directory> {
        let registry = self.projects.as_ref().ok_or_else(|| error("unsupported", "项目管理未装配。"))?;
        let project = registry.get(id)?;
        let root = self.settings.allowed_root(std::path::Path::new(&project.path))?;
        Ok(workspace::Directory::open(&root)?.excluding(self.settings.private_paths()))
    }
    /// Trusted application access shared by preview, review and user-owned terminals.
    pub fn directory(&self, kind: &str, id: &str) -> Result<workspace::Directory> {
        match kind {
            "session" => {
                let header = self.store.header(id).map_err(|e| error("not_found", &e.message))?;
                self.settings.files(&header)
            }
            #[cfg(feature = "projects")]
            "project" => self.project_files(id),
            _ => Err(error("not_found", "工作区入口不存在。")),
        }
    }
    pub fn create(&self, key: &str, project_id: Option<&str>) -> Result<sessions::Header> {
        if let Ok(header) = self.store.header(&format!("s-{key}")) {
            if header.metadata.get("project.id").map(String::as_str) != project_id {
                return Err(error("conflict", "同一创建请求不能改变项目归属。"));
            }
            return Ok(header);
        }
        let (explicit, metadata): (Option<PathBuf>, BTreeMap<String, String>) =
            if let Some(id) = project_id {
                #[cfg(feature = "projects")]
                {
                    let registry = self
                        .projects
                        .as_ref()
                        .ok_or_else(|| error("unsupported", "项目管理未装配。"))?;
                    let project = registry.get(id)?;
                    let root = self
                        .settings
                        .allowed_root(std::path::Path::new(&project.path))?;
                    (
                        Some(root),
                        BTreeMap::from([
                            ("project.id".into(), project.id),
                            ("project.name".into(), project.name),
                        ]),
                    )
                }
                #[cfg(not(feature = "projects"))]
                {
                    let _ = id;
                    return Err(error("unsupported", "项目管理未装配。"));
                }
            } else {
                (None, BTreeMap::new())
            };
        self.settings
            .create_session(&self.store, key, explicit.as_deref(), metadata)
    }
}
#[derive(Clone)]
struct StateData {
    service: Arc<Service>,
}
struct Auth(String);
pub struct HttpError(pub workspace::Error);
impl From<workspace::Error> for HttpError {
    fn from(e: workspace::Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "invalid_request" => StatusCode::BAD_REQUEST,
            "forbidden" => StatusCode::FORBIDDEN,
            "not_found" => StatusCode::NOT_FOUND,
            "conflict" => StatusCode::CONFLICT,
            "capacity" => StatusCode::PAYLOAD_TOO_LARGE,
            "busy" => StatusCode::TOO_MANY_REQUESTS,
            "unsupported" => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
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
            Json(error(
                "unauthorized",
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
pub fn router(service: Arc<Service>, token: String, origin: Option<String>) -> Result<Router> {
    if !(32..=512).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(error("invalid_request", "invalid workspace bearer token"));
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !crate::ui_origin_allowed(&origin) {
            return Err(error(
                "invalid_request",
                "workspace requires explicit CORS origin",
            ));
        }
        cors = cors.allow_origin(
            origin
                .parse::<HeaderValue>()
                .map_err(|_| error("invalid_request", "invalid CORS origin"))?,
        );
    }
    let router = Router::new()
        .route(
            "/api/workspace-settings",
            get(settings).put(update_settings),
        )
        .route("/api/sessions/{id}/workspace", get(info))
        .route("/api/sessions/{id}/files", get(list_files))
        .route("/api/sessions/{id}/files/search", get(search_session_files))
        .route("/api/sessions/{id}/file", get(read_file));
    #[cfg(feature = "projects")]
    let router = router
        .route("/api/projects", get(list_projects).post(add_project))
        .route("/api/projects/create", post(create_project))
        .route("/api/projects/{id}/remove", post(remove_project))
        .route("/api/projects/{id}/files", get(list_project_files))
        .route("/api/projects/{id}/files/search", get(search_project_files))
        .route("/api/projects/{id}/file", get(read_project_file));
    Ok(router
        .with_state(StateData { service })
        .layer(DefaultBodyLimit::max(32 * 1024))
        .layer(middleware::from_fn_with_state(
            Arc::new(Auth(token)),
            authenticate,
        ))
        .layer(cors))
}
async fn disk<T: Send + 'static>(
    action: impl FnOnce() -> Result<T> + Send + 'static,
) -> std::result::Result<T, HttpError> {
    tokio::task::spawn_blocking(action)
        .await
        .map_err(|_| HttpError(error("io", "工作区操作失败，结果未确认。")))?
        .map_err(Into::into)
}
async fn settings(
    State(s): State<StateData>,
) -> std::result::Result<Json<crate::workspace_setup::View>, HttpError> {
    let mut view = s.service.settings.view(s.service.projects_enabled())?;
    view.project_create_mode = if s.service.projects_enabled() { "name" } else { "none" };
    Ok(Json(view))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    revision: u64,
    default_root: String,
}
async fn update_settings(
    State(s): State<StateData>,
    Json(value): Json<Update>,
) -> std::result::Result<Json<crate::workspace_setup::View>, HttpError> {
    let settings = s.service.settings.clone();
    let enabled = s.service.projects_enabled();
    let mut result =
        disk(move || settings.update(value.revision, std::path::Path::new(&value.default_root)))
            .await?;
    result.projects_enabled = enabled;
    result.project_create_mode = if enabled { "name" } else { "none" };
    Ok(Json(result))
}
#[derive(Serialize)]
struct WorkspaceInfo {
    root: String,
    name: String,
    available: bool,
    message: Option<String>,
    metadata: BTreeMap<String, String>,
}
async fn info(
    State(s): State<StateData>,
    Path(id): Path<String>,
) -> std::result::Result<Json<WorkspaceInfo>, HttpError> {
    let service = s.service;
    Ok(Json(
        disk(move || {
            let header = service
                .store
                .header(&id)
                .map_err(|e| error("not_found", &e.message))?;
            let result = service.settings.files(&header);
            Ok(WorkspaceInfo {
                name: std::path::Path::new(&header.workspace)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("工作区")
                    .into(),
                root: header.workspace,
                available: result.is_ok(),
                message: result.err().map(|e| e.message),
                metadata: header.metadata,
            })
        })
        .await?,
    ))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    path: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    revision: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    limit: Option<usize>,
}
#[derive(Serialize)]
struct SearchResult {
    entries: Vec<workspace::Entry>,
}
fn search_directory(directory: workspace::Directory, q: &str, limit: usize) -> Result<SearchResult> {
    let q = q.trim().to_lowercase();
    if q.is_empty() || q.len() > 320 || !(1..=200).contains(&limit) {
        return Err(error("invalid_request", "文件搜索参数无效。"));
    }
    let mut stack = vec![String::new()];
    let mut matches = Vec::new();
    let mut scanned = 0usize;
    while let Some(path) = stack.pop() {
        let mut offset = 0usize;
        loop {
            let page = directory.list(&path, offset, 200, None)?;
            for entry in page.entries {
                scanned = scanned.saturating_add(1);
                if scanned > 10_000 {
                    return Err(error("capacity", "工作区文件超过搜索上限，请缩小搜索范围。"));
                }
                if entry.kind == "directory" {
                    stack.push(entry.path.clone());
                } else if entry.kind == "file" && entry.path.to_lowercase().contains(&q) {
                    matches.push(entry);
                    if matches.len() >= limit {
                        return Ok(SearchResult { entries: matches });
                    }
                }
            }
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
    }
    Ok(SearchResult { entries: matches })
}
async fn list_files(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> std::result::Result<Json<workspace::DirectoryPage>, HttpError> {
    let service = s.service;
    let permit = service
        .readers
        .clone()
        .try_acquire_owned()
        .map_err(|_| HttpError(error("busy", "正在读取其他文件，请稍后重试。")))?;
    Ok(Json(
        disk(move || {
            let _permit = permit;
            let h = service
                .store
                .header(&id)
                .map_err(|e| error("not_found", &e.message))?;
            service.settings.files(&h)?.list(
                q.path.as_deref().unwrap_or(""),
                q.offset.unwrap_or(0),
                q.limit.unwrap_or(100),
                q.revision.as_deref(),
            )
        })
        .await?,
    ))
}
async fn read_file(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> std::result::Result<Json<workspace::FilePage>, HttpError> {
    let service = s.service;
    let permit = service
        .readers
        .clone()
        .try_acquire_owned()
        .map_err(|_| HttpError(error("busy", "正在读取其他文件，请稍后重试。")))?;
    Ok(Json(
        disk(move || {
            let _permit = permit;
            let h = service
                .store
                .header(&id)
                .map_err(|e| error("not_found", &e.message))?;
            service.settings.files(&h)?.read(
                q.path.as_deref().unwrap_or(""),
                q.offset.unwrap_or(0),
                q.limit.unwrap_or(64 * 1024),
                q.revision.as_deref(),
            )
        })
        .await?,
    ))
}
async fn search_session_files(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Query(q): Query<SearchQuery>,
) -> std::result::Result<Json<SearchResult>, HttpError> {
    let service = s.service;
    let permit = service.readers.clone().try_acquire_owned()
        .map_err(|_| HttpError(error("busy", "正在读取其他文件，请稍后重试。")))?;
    Ok(Json(disk(move || {
        let _permit = permit;
        let header = service.store.header(&id).map_err(|e| error("not_found", &e.message))?;
        search_directory(service.settings.files(&header)?, &q.q, q.limit.unwrap_or(100))
    }).await?))
}

#[cfg(feature = "projects")]
async fn list_project_files(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> std::result::Result<Json<workspace::DirectoryPage>, HttpError> {
    let service = s.service;
    let permit = service.readers.clone().try_acquire_owned()
        .map_err(|_| HttpError(error("busy", "正在读取其他文件，请稍后重试。")))?;
    Ok(Json(disk(move || {
        let _permit = permit;
        service.project_files(&id)?.list(
            q.path.as_deref().unwrap_or(""), q.offset.unwrap_or(0), q.limit.unwrap_or(100), q.revision.as_deref())
    }).await?))
}

#[cfg(feature = "projects")]
async fn read_project_file(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> std::result::Result<Json<workspace::FilePage>, HttpError> {
    let service = s.service;
    let permit = service.readers.clone().try_acquire_owned()
        .map_err(|_| HttpError(error("busy", "正在读取其他文件，请稍后重试。")))?;
    Ok(Json(disk(move || {
        let _permit = permit;
        service.project_files(&id)?.read(
            q.path.as_deref().unwrap_or(""), q.offset.unwrap_or(0), q.limit.unwrap_or(64 * 1024), q.revision.as_deref())
    }).await?))
}

#[cfg(feature = "projects")]
async fn search_project_files(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Query(q): Query<SearchQuery>,
) -> std::result::Result<Json<SearchResult>, HttpError> {
    let service = s.service;
    let permit = service.readers.clone().try_acquire_owned()
        .map_err(|_| HttpError(error("busy", "正在读取其他文件，请稍后重试。")))?;
    Ok(Json(disk(move || {
        let _permit = permit;
        search_directory(service.project_files(&id)?, &q.q, q.limit.unwrap_or(100))
    }).await?))
}
#[cfg(feature = "projects")]
async fn list_projects(
    State(s): State<StateData>,
) -> std::result::Result<Json<projects::Listing>, HttpError> {
    let registry = s
        .service
        .projects
        .as_ref()
        .ok_or_else(|| error("unsupported", "项目管理未装配。"))?;
    Ok(Json(registry.list()?))
}
#[cfg(feature = "projects")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddProject {
    request_id: String,
    revision: u64,
    name: String,
    path: String,
}
#[cfg(feature = "projects")]
async fn add_project(
    State(s): State<StateData>,
    Json(v): Json<AddProject>,
) -> std::result::Result<Json<projects::Listing>, HttpError> {
    let registry = s
        .service
        .projects
        .clone()
        .ok_or_else(|| error("unsupported", "项目管理未装配。"))?;
    let settings = s.service.settings.clone();
    Ok(Json(
        disk(move || {
            let root = settings.allowed_root(std::path::Path::new(&v.path))?;
            registry.add(&v.request_id, v.revision, &v.name, &root)
        })
        .await?,
    ))
}
#[cfg(feature = "projects")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProject {
    request_id: String,
    revision: u64,
    name: String,
}
#[cfg(feature = "projects")]
fn valid_project_directory(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    !name.is_empty() && name.chars().count() <= 80 && name.len() <= 240
        && name != "." && name != ".." && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9")
}
#[cfg(feature = "projects")]
async fn create_project(
    State(s): State<StateData>,
    Json(v): Json<CreateProject>,
) -> std::result::Result<Json<projects::Listing>, HttpError> {
    let registry = s.service.projects.clone().ok_or_else(|| error("unsupported", "项目管理未装配。"))?;
    let settings = s.service.settings.clone();
    Ok(Json(disk(move || create_named_project(&registry, &settings, v)).await?))
}
#[cfg(feature = "projects")]
fn create_named_project(registry: &projects::Registry, settings: &WorkspaceSettings, v: CreateProject) -> Result<projects::Listing> {
    let name = v.name.trim();
    if !valid_project_directory(name) {
        return Err(error("invalid_request", "项目名称需为有效的单层目录名，最多 80 个字符。"));
    }
    if v.request_id.is_empty() || v.request_id.len() > 64 || !v.request_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
        return Err(error("invalid_request", "项目请求 ID 无效。"));
    }
    if registry.list()?.revision != v.revision {
        return Err(error("conflict", "项目列表已更新，请刷新。"));
    }
    let base = settings.allowed_root(std::path::Path::new(&settings.view(false)?.default_root))?;
    let path = base.join(name);
    std::fs::create_dir(&path).map_err(|cause| if cause.kind() == std::io::ErrorKind::AlreadyExists {
        error("conflict", "默认工作区已有同名目录，请换一个项目名称。")
    } else { error("io", "无法在默认工作区创建项目目录。") })?;
    let result = (|| {
        let root = settings.allowed_root(&path)?;
        if !workspace::contains_path(&base, &root) || base == root {
            return Err(error("forbidden", "项目目录必须位于默认工作区内。"));
        }
        registry.add(&v.request_id, v.revision, name, &root)
    })();
    match result {
        Ok(listing) => Ok(listing),
        Err(failure) => {
            if ["conflict", "capacity", "invalid_request", "forbidden"].contains(&failure.code.as_str()) {
                let _ = std::fs::remove_dir(&path);
            }
            if failure.code == "io" {
                Err(error("io", "项目目录已创建，但登记未确认；请刷新项目列表并检查目录。"))
            } else {
                Err(failure)
            }
        }
    }
}
#[cfg(feature = "projects")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: u64,
}
#[cfg(feature = "projects")]
async fn remove_project(
    State(s): State<StateData>,
    Path(id): Path<String>,
    Json(v): Json<Revision>,
) -> std::result::Result<Json<projects::Listing>, HttpError> {
    let registry = s
        .service
        .projects
        .clone()
        .ok_or_else(|| error("unsupported", "项目管理未装配。"))?;
    Ok(Json(disk(move || registry.remove(&id, v.revision)).await?))
}

#[cfg(all(test, feature = "projects"))]
mod project_create_tests {
    use super::*;

    #[test]
    fn name_creation_stays_under_default_root_and_rejects_unsafe_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        let settings = WorkspaceSettings::open(temp.path().join("settings.json"), root.clone(), None, vec![]).unwrap();
        let registry = projects::Registry::open(&temp.path().join("projects.json")).unwrap();
        let listing = create_named_project(&registry, &settings, CreateProject {
            request_id: "first".into(), revision: 0, name: "项目一".into(),
        }).unwrap();
        assert!(root.join("项目一").is_dir());
        assert_eq!(listing.projects[0].name, "项目一");
        for name in ["..", "../outside", "nested/child", "C:\\outside", "CON", "bad."] {
            let failure = create_named_project(&registry, &settings, CreateProject {
                request_id: format!("bad-{}", name.len()), revision: listing.revision, name: name.into(),
            }).err().unwrap();
            assert_eq!(failure.code, "invalid_request");
        }
        assert!(!temp.path().join("outside").exists());
        let duplicate = create_named_project(&registry, &settings, CreateProject {
            request_id: "second".into(), revision: listing.revision, name: "项目一".into(),
        }).err().unwrap();
        assert_eq!(duplicate.code, "conflict");
        assert_eq!(registry.list().unwrap().projects.len(), 1);
    }
}
