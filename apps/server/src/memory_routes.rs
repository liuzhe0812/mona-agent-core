//! Product assembly and authenticated management. Memory and history implementations stay reusable.
use api::CancellationToken;
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(feature = "memory")]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

#[cfg(feature = "memory")]
pub struct MemoryHost {
    root: PathBuf,
    personal: Arc<dyn memory::Backend>,
}
#[cfg(feature = "memory")]
impl MemoryHost {
    pub fn from_environment() -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
        let root = match std::env::var_os("AGENT_MEMORY_DIR") {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            Some(_) => return Err("AGENT_MEMORY_DIR cannot be empty".into()),
            None => crate::workspace_setup::state_root()?.join("memory"),
        };
        let personal = Arc::new(memory::FileStore::open(
            &root.join("personal"),
            memory::Limits::default(),
        )?);
        Ok(Arc::new(Self { root, personal }))
    }
    fn workspace(&self, cwd: &str) -> memory::Result<Arc<dyn memory::Backend>> {
        use sha2::{Digest, Sha256};
        if cwd.is_empty() || !Path::new(cwd).is_absolute() {
            return Err(memory::Error {
                code: memory::ErrorCode::InvalidRequest,
                message: "工作区记忆需要已绑定的会话目录。".into(),
            });
        }
        let key = format!("{:x}", Sha256::digest(cwd.as_bytes()));
        Ok(Arc::new(memory::FileStore::open(
            &self.root.join("workspaces").join(key),
            memory::Limits::default(),
        )?))
    }
    pub fn plugin(&self, cwd: &Path, writable: bool) -> api::Result<memory::MemoryPlugin> {
        let path = cwd.to_str().ok_or_else(|| {
            api::AgentError::new(api::ErrorCode::Configuration, "workspace path is not UTF-8")
        })?;
        let workspace = self
            .workspace(path)
            .map_err(|e| api::AgentError::new(api::ErrorCode::Configuration, e.message))?;
        memory::MemoryPlugin::new(vec![
            memory::Binding::new("personal", self.personal.clone(), writable),
            memory::Binding::new("workspace", workspace, writable),
        ])
    }
    fn resolve(
        &self,
        store: &sessions::Store,
        scope: &str,
        session: Option<&str>,
    ) -> Result<Arc<dyn memory::Backend>, HttpError> {
        match scope {
            "personal" => Ok(self.personal.clone()),
            "workspace" => {
                let id = session.ok_or_else(|| {
                    http(
                        StatusCode::BAD_REQUEST,
                        "请先打开一个已保存会话，再管理其工作区记忆。",
                    )
                })?;
                let h = store.header(id).map_err(session_error)?;
                self.workspace(&h.workspace).map_err(memory_error)
            }
            _ => Err(http(
                StatusCode::BAD_REQUEST,
                "无效记忆范围；不接受任意目录或用户标识。",
            )),
        }
    }
}
#[derive(Clone)]
struct Service {
    store: Arc<sessions::Store>,
    #[cfg(feature = "memory")]
    memory: Option<Arc<MemoryHost>>,
    #[cfg(feature = "history-search")]
    search: Option<Arc<sessions::search::HistorySearch>>,
    agent_writes: bool,
}
struct Auth(String);
struct HttpError(StatusCode, String);
fn http(status: StatusCode, message: &str) -> HttpError {
    HttpError(status, message.into())
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"message":self.1}))).into_response()
    }
}
fn session_error(e: sessions::SessionError) -> HttpError {
    use sessions::SessionErrorCode as C;
    let status = match e.code {
        C::NotFound => StatusCode::NOT_FOUND,
        C::Conflict | C::Closed => StatusCode::CONFLICT,
        C::Capacity => StatusCode::TOO_MANY_REQUESTS,
        C::InvalidRequest => StatusCode::BAD_REQUEST,
        C::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    HttpError(status, e.message)
}
#[cfg(feature = "memory")]
fn memory_error(e: memory::Error) -> HttpError {
    use memory::ErrorCode as C;
    let status = match e.code {
        C::Conflict | C::Closed => StatusCode::CONFLICT,
        C::Capacity => StatusCode::PAYLOAD_TOO_LARGE,
        C::InvalidRequest => StatusCode::BAD_REQUEST,
        C::Forbidden => StatusCode::FORBIDDEN,
        C::Io => StatusCode::INTERNAL_SERVER_ERROR,
    };
    HttpError(status, e.message)
}
async fn blocking<T: Send + 'static>(
    action: impl FnOnce() -> Result<T, HttpError> + Send + 'static,
) -> Result<T, HttpError> {
    tokio::task::spawn_blocking(action).await.map_err(|_| {
        http(
            StatusCode::INTERNAL_SERVER_ERROR,
            "记忆操作状态未知，请刷新确认。",
        )
    })?
}
async fn authenticate(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .is_some_and(|s| bool::from(s.as_bytes().ct_eq(auth.0.as_bytes())));
    let mut response = if valid {
        next.run(request).await
    } else {
        http(StatusCode::UNAUTHORIZED, "需要有效的宿主访问凭据。").into_response()
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
    factory: &crate::environment::Factory,
    token: String,
    origin: Option<String>,
) -> api::Result<Router> {
    if !(32..=512).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(api::AgentError::new(
            api::ErrorCode::Configuration,
            "invalid memory management token",
        ));
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !(origin.starts_with("http://") || origin.starts_with("https://")) {
            return Err(api::AgentError::new(
                api::ErrorCode::Configuration,
                "memory CORS requires explicit origin",
            ));
        }
        cors = cors.allow_origin(origin.parse::<HeaderValue>().map_err(|_| {
            api::AgentError::new(api::ErrorCode::Configuration, "invalid memory CORS")
        })?);
    }
    let service = Service {
        store: factory.store.clone(),
        #[cfg(feature = "memory")]
        memory: factory.memory.clone(),
        #[cfg(feature = "history-search")]
        search: factory.history.clone(),
        agent_writes: factory.capabilities.active(crate::capabilities::MEMORY)
            && factory
                .capabilities
                .active(crate::capabilities::MEMORY_UPDATE),
    };
    let router = Router::new().route("/api/memory", get(view));
    #[cfg(feature = "memory")]
    let router = router.route("/api/memory/update", post(update));
    #[cfg(feature = "history-search")]
    let router = router
        .route("/api/history/search", post(search))
        .route("/api/history/read", post(read));
    Ok(router
        .with_state(service)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(
            Arc::new(Auth(token)),
            authenticate,
        ))
        .layer(cors))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewQuery {
    session_id: Option<String>,
}
async fn view(
    State(s): State<Service>,
    q: Result<Query<ViewQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, HttpError> {
    let q = q
        .map_err(|_| http(StatusCode::BAD_REQUEST, "无效的记忆查询参数。"))?
        .0;
    blocking(move||{
        if let Some(id) = q.session_id.as_deref() { s.store.header(id).map_err(session_error)?; }
        #[cfg(feature="memory")]
        let (enabled, scopes) = {
            let mut scopes = Vec::<Value>::new();
            if let Some(memory) = &s.memory {
                for scope in if q.session_id.is_some(){vec!["personal","workspace"]}else{vec!["personal"]}{
                    let backend=memory.resolve(&s.store,scope,q.session_id.as_deref())?;
                    let snapshot=backend.read(&CancellationToken::new()).map_err(memory_error)?;
                    scopes.push(json!({"name":scope,"revision":snapshot.revision,"entries":snapshot.entries,"text_bytes":snapshot.text_bytes,"limit_bytes":snapshot.limit_bytes}));
                }
            }
            (s.memory.is_some(), scopes)
        };
        #[cfg(not(feature="memory"))]
        let (enabled, scopes) = (false, Vec::<Value>::new());
        let history_enabled={#[cfg(feature="history-search")]{s.search.is_some()}#[cfg(not(feature="history-search"))]{false}};
        Ok(Json(json!({"enabled":enabled,"agent_writes":s.agent_writes,"history_enabled":history_enabled,"scopes":scopes})))
    }).await
}
#[cfg(feature = "memory")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    scope: String,
    session_id: Option<String>,
    revision: String,
    operations: Vec<memory::Operation>,
}
#[cfg(feature = "memory")]
async fn update(
    State(s): State<Service>,
    body: Result<Json<Update>, JsonRejection>,
) -> Result<Json<Value>, HttpError> {
    let value = body
        .map_err(|_| http(StatusCode::BAD_REQUEST, "记忆修改请求无效或过大。"))?
        .0;
    blocking(move||{
        let memory=s.memory.ok_or_else(||http(StatusCode::NOT_FOUND,"当前宿主未装配长期记忆。"))?;
        let backend=memory.resolve(&s.store,&value.scope,value.session_id.as_deref())?;
        let snapshot=backend.apply(&memory::Change{revision:value.revision,operations:value.operations},memory::Origin::host(),&CancellationToken::new()).map_err(memory_error)?;
        Ok(Json(json!({"scope":value.scope,"revision":snapshot.revision,"entries":snapshot.entries,"text_bytes":snapshot.text_bytes,"limit_bytes":snapshot.limit_bytes})))
    }).await
}
#[cfg(feature = "history-search")]
async fn search(
    State(s): State<Service>,
    body: Result<Json<sessions::search::SearchRequest>, JsonRejection>,
) -> Result<Json<sessions::search::SearchPage>, HttpError> {
    let value = body
        .map_err(|_| http(StatusCode::BAD_REQUEST, "历史搜索参数无效。"))?
        .0;
    let service = s
        .search
        .ok_or_else(|| http(StatusCode::NOT_FOUND, "当前宿主未装配历史检索。"))?;
    // This management API belongs to the single authenticated owner. Model tools get narrower scopes.
    blocking(move || {
        service
            .search(
                &sessions::search::Scope::All,
                &value,
                &CancellationToken::new(),
            )
            .map(Json)
            .map_err(session_error)
    })
    .await
}
#[cfg(feature = "history-search")]
async fn read(
    State(s): State<Service>,
    body: Result<Json<sessions::search::ReadRequest>, JsonRejection>,
) -> Result<Json<sessions::search::MessagePage>, HttpError> {
    let value = body
        .map_err(|_| http(StatusCode::BAD_REQUEST, "历史原文参数无效。"))?
        .0;
    let service = s
        .search
        .ok_or_else(|| http(StatusCode::NOT_FOUND, "当前宿主未装配历史检索。"))?;
    blocking(move || {
        service
            .read(
                &sessions::search::Scope::All,
                &value,
                &CancellationToken::new(),
            )
            .map(Json)
            .map_err(session_error)
    })
    .await
}
