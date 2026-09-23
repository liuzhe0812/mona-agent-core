//! Trusted-host assembly and authenticated browser reads for local spill archives.
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use spill::{LocalSpillStore, SpillConfig, SpillPlugin, SpillStore};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct SpillHost {
    store: Arc<LocalSpillStore>,
    config: SpillConfig,
}

impl SpillHost {
    pub fn from_environment() -> Result<Self, Box<dyn std::error::Error>> {
        let root = match std::env::var_os("AGENT_SPILL_DIR") {
            Some(value) => PathBuf::from(value),
            None => state_root()?.join("mona-agent-core").join("spill"),
        };
        let config = SpillConfig::default();
        let store = Arc::new(LocalSpillStore::new(root, config.clone())?);
        Ok(Self { store, config })
    }

    pub fn plugin(&self) -> SpillPlugin {
        SpillPlugin::new(self.store.clone(), self.config.clone()).through_core_read()
    }

    pub fn read_extension(&self) -> SpillReadExtension {
        SpillReadExtension {
            store: self.store.clone(),
            max_page_bytes: self.config.max_page_bytes,
            sessions: None,
        }
    }

    pub async fn cleanup_startup(&self) -> api::Result<()> {
        self.store.cleanup(&BTreeSet::new()).await.map(|_| ())
    }
}

/// Tools depend on their small retention seam, not this Web host or a concrete store.
pub struct SpillOutputArchive { archive: Arc<spill::SpillArchive> }
impl SpillOutputArchive {
    pub fn new(archive: Arc<spill::SpillArchive>) -> Self { Self { archive } }
}
#[api::async_trait]
impl tools::OutputArchive for SpillOutputArchive {
    fn trigger_bytes(&self) -> usize { self.archive.config().trigger_bytes }
    fn preview_bytes(&self) -> usize { self.archive.config().preview_bytes }
    async fn store(&self, ctx: &api::ToolContext,
        source: &mut (dyn tokio::io::AsyncRead + Unpin + Send), bytes: usize) -> api::Result<api::ArtifactRef> {
        let record = self.archive.put_stream(&ctx.run, &ctx.call_id, source, bytes).await?;
        Ok(api::ArtifactRef { uri: format!("{}{}", spill::SPILL_URI_SCHEME, record.id), bytes: record.bytes })
    }
}

pub struct SpillReadExtension {
    store: Arc<dyn SpillStore>,
    max_page_bytes: usize,
    sessions: Option<Arc<sessions::Store>>,
}

impl SpillReadExtension {
    pub fn with_sessions(mut self, store: Arc<sessions::Store>) -> Self {
        self.sessions = Some(store); self
    }
}

#[api::async_trait]
impl tools::ReadExtension for SpillReadExtension {
    fn supports(&self, path: &str) -> bool {
        path.starts_with(spill::SPILL_URI_SCHEME)
    }

    async fn read(
        &self,
        ctx: api::ToolContext,
        path: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> api::Result<api::ToolOutput> {
        let id = path.strip_prefix(spill::SPILL_URI_SCHEME).ok_or_else(|| {
            api::AgentError::new(api::ErrorCode::Schema, "invalid spill artifact path")
        })?;
        let owner = match (&self.sessions, ctx.run.metadata.get(sessions::SESSION_KEY)) {
            (Some(store), Some(session)) => {
                let store = store.clone(); let session = session.clone(); let run = ctx.run.run_id.clone(); let uri = path.to_owned();
                tokio::task::spawn_blocking(move || store.artifact_owner(&session, &run, &uri)).await
                    .map_err(|_| api::AgentError::new(api::ErrorCode::Tool, "会话归档授权检查失败。"))?
                    .map_err(|_| api::AgentError::new(api::ErrorCode::Tool, "归档不属于当前会话，或确认记录不可读。"))?
                    .unwrap_or_else(|| ctx.run.run_id.clone())
            },
            _ => ctx.run.run_id.clone(),
        };
        let byte_offset = offset.unwrap_or(1).saturating_sub(1);
        let mut byte_limit = limit
            .unwrap_or(self.max_page_bytes)
            .min(self.max_page_bytes)
            .min(ctx.run.limits.max_tool_result_bytes);
        if byte_limit == 0 {
            return Err(api::AgentError::new(
                api::ErrorCode::Schema,
                "spill read limit must be positive",
            ));
        }
        loop {
            ctx.run.task.check()?;
            if ctx.run.cancel.is_cancelled() {
                return Err(api::AgentError::new(
                    api::ErrorCode::Cancelled,
                    "archive read cancelled",
                ));
            }
            let page = self
                .store
                .read_page(&owner, id, byte_offset, byte_limit)
                .await.map_err(|mut error| {
                    if error.message == "spill entry is unknown or expired" {
                        error.message = "归档已过期、已删除或不在当前授权范围内；不能恢复全文，请使用已保存的预览或重新获取来源。".into();
                    }
                    error
                })?;
            ctx.run.task.check()?;
            if ctx.run.cancel.is_cancelled() {
                return Err(api::AgentError::new(
                    api::ErrorCode::Cancelled,
                    "archive read cancelled",
                ));
            }
            let text_bytes = page.text.len();
            let mut output = api::ToolOutput::new(page.text);
            output.structured = Some(serde_json::json!({
                "path": path,
                "unit": "utf8_bytes",
                "offset": page.offset + 1,
                "next_offset": if page.eof { serde_json::Value::Null } else { serde_json::json!(page.next_offset + 1) },
                "total_bytes": page.total_bytes,
                "eof": page.eof,
            }));
            output.artifact = Some(api::ArtifactRef {
                uri: path.into(),
                bytes: page.total_bytes as u64,
            });
            let total = api::ToolResult::from_output(&ctx.call_id, output.clone()).payload_bytes();
            if total <= ctx.run.limits.max_tool_result_bytes {
                return Ok(output);
            }
            let next_limit = ctx
                .run
                .limits
                .max_tool_result_bytes
                .saturating_sub(total.saturating_sub(text_bytes));
            if next_limit == 0 || next_limit >= byte_limit {
                return Ok(api::ToolOutput::error(
                    "archive metadata cannot fit within the run result limit",
                ));
            }
            byte_limit = next_limit;
        }
    }
}

fn state_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| "set AGENT_SPILL_DIR for this host".into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadQuery {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    spill::DEFAULT_MAX_PAGE_BYTES
}

#[derive(Clone)]
struct ReadState {
    store: Arc<LocalSpillStore>,
}
struct Auth {
    token: String,
}

pub fn router(
    host: SpillHost,
    token: String,
    origin: Option<String>,
) -> Result<Router, Box<dyn std::error::Error>> {
    if token.len() < 32 || token.len() > 512 || !token.bytes().all(|c| c.is_ascii_graphic()) {
        return Err("spill reads require a 32..512 character bearer token".into());
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET])
        .allow_headers([header::AUTHORIZATION]);
    if let Some(origin) = origin {
        if origin == "*" || !(origin.starts_with("https://") || origin.starts_with("http://")) {
            return Err("spill reads require an explicit HTTP(S) origin".into());
        }
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }
    Ok(Router::new()
        .route("/api/spill/{run_id}/{id}", get(read_spill))
        .with_state(ReadState { store: host.store })
        .layer(middleware::from_fn_with_state(
            Arc::new(Auth { token }),
            authenticate,
        ))
        .layer(cors))
}

async fn authenticate(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !bearer.is_some_and(|v| bool::from(v.as_bytes().ct_eq(auth.token.as_bytes()))) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message":"valid bearer authorization is required"})),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn read_spill(
    State(state): State<ReadState>,
    Path((run_id, id)): Path<(String, String)>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<spill::SpillPage>, SpillApiError> {
    state
        .store
        .read_page(&run_id, &id, query.offset, query.limit)
        .await
        .map(Json)
        .map_err(SpillApiError)
}

struct SpillApiError(api::AgentError);
impl IntoResponse for SpillApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            api::ErrorCode::Schema | api::ErrorCode::Limit => StatusCode::BAD_REQUEST,
            api::ErrorCode::Tool => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({"code":format!("{:?}",self.0.code).to_lowercase(),"message":self.0.message}))).into_response()
    }
}

#[cfg(all(test, feature = "compaction"))]
#[path = "spill_flow_tests.rs"]
mod flow_tests;

#[cfg(test)]
#[path = "spill_session_tests.rs"]
mod session_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use api::{AgentExecutor, RunRequest};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tempfile::tempdir;
    use tower::ServiceExt;

    const TOKEN: &str = "test-spill-read-token-with-32-bytes";

    #[tokio::test]
    async fn formal_host_keeps_exactly_four_default_model_tools_with_spill_enabled() {
        let directory = tempdir().unwrap();
        let config = SpillConfig::default();
        let store =
            Arc::new(LocalSpillStore::new(directory.path().join("spill"), config.clone()).unwrap());
        let spill_host = SpillHost { store, config };
        let mut tool_config = tools::ToolConfig::new(directory.path(), "unused-shell");
        tool_config
            .read_extensions
            .push(Arc::new(spill_host.read_extension()));
        let mut builder = runtime::HostBuilder::new()
            .model(Arc::new(demo::ScriptedModel::new(vec![demo::text("ok")])));
        for tool in tools::core_tools(&tool_config) {
            builder = builder.tool(tool);
        }
        for name in ["shell", "edit", "write"] {
            builder = builder.allow_side_effect_tool(name);
        }
        let mut host = builder
            .plugin(Arc::new(spill_host.plugin()))
            .build()
            .await
            .unwrap();
        let report = host
            .engine()
            .execute(RunRequest::new("inspect tools"))
            .await
            .unwrap();
        assert_eq!(
            report.model_requests[0]
                .request
                .as_ref()
                .unwrap()
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["edit", "read", "shell", "write"]
        );
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn browser_read_requires_auth_and_stays_run_scoped() {
        let directory = tempdir().unwrap();
        let config = SpillConfig::default();
        let store = Arc::new(LocalSpillStore::new(directory.path(), config.clone()).unwrap());
        let record = store.put("run-a", "call", "complete text").await.unwrap();
        let app = router(SpillHost { store, config }, TOKEN.into(), None).unwrap();
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/spill/run-a/{}", record.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/spill/run-a/{}?offset=0&limit=16", record.id))
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["text"],
            "complete text"
        );

        let denied = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/spill/run-b/{}", record.id))
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }
}
