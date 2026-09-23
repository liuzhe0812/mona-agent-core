//! Host-owned management HTTP surface. The generic task bridges remain unchanged.
use api::{AgentError, ErrorCode};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use models::{
    DiscoverRequest, EncryptedFileStore, ModelEntry, ModelManager, Selection, SettingsView,
    UpsertProvider,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

/// Default Web composition: encrypted local settings, seeded once from existing model env.
/// Deployments should provide a separate stable AGENT_MODEL_STORE_KEY; the initial provider
/// key is a convenient fallback for local development and must remain stable across restarts.
pub fn from_environment() -> Result<ModelManager, Box<dyn std::error::Error>> {
    let api_key = std::env::var("AGENT_MODEL_KEY")
        .ok()
        .filter(|v| !v.is_empty());
    let key = std::env::var("AGENT_MODEL_STORE_KEY").ok().or_else(|| api_key.clone())
        .ok_or("model management requires AGENT_MODEL_STORE_KEY (at least 16 random characters); use AGENT_MODEL_MANAGEMENT=0 for fixed-model mode")?;
    let path = match std::env::var_os("AGENT_MODEL_SETTINGS_PATH") {
        Some(value) => PathBuf::from(value),
        None => {
            let root = std::env::var_os("LOCALAPPDATA")
                .or_else(|| std::env::var_os("XDG_STATE_HOME"))
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
                .ok_or("set AGENT_MODEL_SETTINGS_PATH for this host")?;
            root.join("mona-agent-core").join("model-settings.enc")
        }
    };
    let manager = ModelManager::open(
        Arc::new(EncryptedFileStore::new(path, &key)?),
        std::env::var("AGENT_ALLOW_HTTP_LOOPBACK").as_deref() == Ok("1"),
    )?;
    if manager.view().revision == 0 {
        let endpoint = std::env::var("AGENT_MODEL_ENDPOINT")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let model = std::env::var("AGENT_MODEL_NAME")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let (endpoint, model) = match (endpoint, model) {
            (None, None) => return Ok(manager),
            (Some(endpoint), Some(model)) => (endpoint, model),
            _ => return Err(
                "set both AGENT_MODEL_ENDPOINT and AGENT_MODEL_NAME when importing a fixed model"
                    .into(),
            ),
        };
        let extra = std::env::var("AGENT_MODEL_EXTRA_JSON")
            .ok()
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        manager.seed(
            UpsertProvider {
                revision: 0,
                id: "environment".into(),
                name: "默认供应商".into(),
                api_base: endpoint,
                api_key,
                clear_key: false,
                models: vec![ModelEntry {
                    id: model,
                    enabled: true,
                    context_window_tokens: std::env::var("AGENT_MODEL_CONTEXT_TOKENS").ok()
                        .map(|value| value.parse::<u64>()).transpose()?,
                }],
            },
            extra,
        )?;
    }
    Ok(manager)
}

#[derive(Clone)]
struct Management {
    manager: ModelManager,
    discoveries: Arc<tokio::sync::Semaphore>,
}
struct Auth {
    token: String,
}

pub fn router(
    manager: ModelManager,
    token: String,
    origin: Option<String>,
) -> Result<Router, Box<dyn std::error::Error>> {
    if token.len() < 32 || token.len() > 512 || !token.bytes().all(|c| c.is_ascii_graphic()) {
        return Err("management requires a 32..512 character bearer token".into());
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !(origin.starts_with("https://") || origin.starts_with("http://")) {
            return Err("management requires an explicit HTTP(S) origin".into());
        }
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }
    Ok(Router::new()
        .route("/api/model-settings", get(settings))
        .route("/api/model-settings/providers", post(upsert))
        .route("/api/model-settings/delete", post(delete_provider))
        .route("/api/model-settings/default", post(set_default))
        .route("/api/model-settings/visibility", post(visibility))
        .route("/api/model-settings/discover", post(discover))
        .with_state(Management {
            manager,
            discoveries: Arc::new(tokio::sync::Semaphore::new(4)),
        })
        .layer(DefaultBodyLimit::max(256 * 1024))
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

struct ApiError(StatusCode, String);
impl From<AgentError> for ApiError {
    fn from(error: AgentError) -> Self {
        if error.message.starts_with("settings_conflict:") {
            Self(StatusCode::CONFLICT, error.message)
        } else if error.code == ErrorCode::Configuration {
            Self(StatusCode::BAD_REQUEST, error.message)
        } else {
            Self(
                StatusCode::INTERNAL_SERVER_ERROR,
                "model settings operation failed".into(),
            )
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"message":self.1}))).into_response()
    }
}
fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    input.map(|Json(v)| v).map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "invalid or oversized model settings request".into(),
        )
    })
}
async fn change(
    manager: ModelManager,
    action: impl FnOnce(ModelManager) -> api::Result<SettingsView> + Send + 'static,
) -> Result<Json<SettingsView>, ApiError> {
    tokio::task::spawn_blocking(move || action(manager))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "model settings storage unavailable".into(),
            )
        })?
        .map(Json)
        .map_err(Into::into)
}
async fn settings(State(state): State<Management>) -> Result<Json<SettingsView>, ApiError> {
    change(state.manager, |m| Ok(m.view())).await
}
async fn upsert(
    State(state): State<Management>,
    input: Result<Json<UpsertProvider>, JsonRejection>,
) -> Result<Json<SettingsView>, ApiError> {
    let input = body(input)?;
    change(state.manager, |m| m.upsert(input)).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    revision: u64,
    provider_id: String,
}
async fn delete_provider(
    State(state): State<Management>,
    input: Result<Json<Delete>, JsonRejection>,
) -> Result<Json<SettingsView>, ApiError> {
    let input = body(input)?;
    change(state.manager, move |m| {
        m.delete(input.revision, &input.provider_id)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultModel {
    revision: u64,
    provider_id: String,
    model_id: String,
}
async fn set_default(
    State(state): State<Management>,
    input: Result<Json<DefaultModel>, JsonRejection>,
) -> Result<Json<SettingsView>, ApiError> {
    let input = body(input)?;
    change(state.manager, move |m| {
        m.set_default(
            input.revision,
            Selection {
                provider_id: input.provider_id,
                model_id: input.model_id,
            },
        )
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Visibility {
    revision: u64,
    provider_id: String,
    model_id: Option<String>,
    enabled: bool,
}
async fn visibility(
    State(state): State<Management>,
    input: Result<Json<Visibility>, JsonRejection>,
) -> Result<Json<SettingsView>, ApiError> {
    let input = body(input)?;
    change(state.manager, move |m| {
        m.set_visibility(
            input.revision,
            &input.provider_id,
            input.model_id.as_deref(),
            input.enabled,
        )
    })
    .await
}
async fn discover(
    State(state): State<Management>,
    input: Result<Json<DiscoverRequest>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let input = body(input)?;
    let _permit = state.discoveries.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "too many discovery requests".into(),
        )
    })?;
    let models = state.manager.discover(input).await?;
    Ok(Json(serde_json::json!({"models":models})))
}
