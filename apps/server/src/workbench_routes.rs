//! App-only read-only review, media downloads and interactive user terminals.
use crate::terminal::{Target, Terminals};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{io::Read, sync::Arc};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;
use workspace::{error, Result};
#[derive(Clone)]
struct Service {
    workspaces: Arc<crate::workspace_routes::Service>,
    terminals: Arc<Terminals>,
    readers: Arc<tokio::sync::Semaphore>,
}
impl Service {
    fn permit(&self) -> std::result::Result<tokio::sync::OwnedSemaphorePermit, Error> {
        self.readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("busy", "正在读取其他内容，请稍后重试。").into())
    }
}
struct Auth(String);
type Error = crate::workspace_routes::HttpError;
async fn auth(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .is_some_and(|s| bool::from(s.as_bytes().ct_eq(auth.0.as_bytes())));
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
pub fn router(
    workspaces: Arc<crate::workspace_routes::Service>,
    terminals: Arc<Terminals>,
    token: String,
    origin: Option<String>,
) -> Result<Router> {
    if !(32..=512).contains(&token.len()) {
        return Err(error("invalid_request", "invalid bearer"));
    }
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if let Some(origin) = origin {
        if origin == "*" || !(origin.starts_with("http://") || origin.starts_with("https://")) {
            return Err(error("invalid_request", "explicit CORS origin required"));
        }
        cors = cors.allow_origin(
            origin
                .parse::<HeaderValue>()
                .map_err(|_| error("invalid_request", "invalid origin"))?,
        );
    }
    Ok(Router::new()
        .route("/api/workbench/capabilities", get(capabilities))
        .route("/api/workbench/{kind}/{id}/review", get(review))
        .route("/api/workbench/{kind}/{id}/diff", get(diff))
        .route("/api/workbench/{kind}/{id}/bytes", get(bytes))
        .route("/api/workbench/{kind}/{id}/terminals", post(open_terminal))
        .route(
            "/api/workbench/{kind}/{id}/terminals/{terminal}",
            axum::routing::delete(close_terminal),
        )
        .route(
            "/api/workbench/{kind}/{id}/terminals/{terminal}/output",
            get(terminal_output),
        )
        .route(
            "/api/workbench/{kind}/{id}/terminals/{terminal}/input",
            post(terminal_input),
        )
        .route(
            "/api/workbench/{kind}/{id}/terminals/{terminal}/size",
            post(terminal_size),
        )
        .with_state(Service {
            workspaces,
            terminals,
            readers: Arc::new(tokio::sync::Semaphore::new(4)),
        })
        .layer(DefaultBodyLimit::max(96 * 1024))
        .layer(middleware::from_fn_with_state(Arc::new(Auth(token)), auth))
        .layer(cors))
}
async fn disk<T: Send + 'static>(
    action: impl FnOnce() -> Result<T> + Send + 'static,
) -> std::result::Result<T, Error> {
    tokio::task::spawn_blocking(action)
        .await
        .map_err(|_| error("io", "面板操作失败。"))?
        .map_err(Into::into)
}
async fn capabilities() -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"terminal":Terminals::enabled(),"review":true,"bytes":true,"side":true,"embedded_browser":false}),
    )
}
async fn review(
    State(s): State<Service>,
    Path((kind, id)): Path<(String, String)>,
) -> std::result::Result<Json<crate::review::Review>, Error> {
    let _permit = s.permit()?;
    let directory = s.workspaces.directory(&kind, &id)?;
    Ok(Json(
        crate::review::list(directory, s.workspaces.settings.private_paths()).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffQuery {
    path: String,
    #[serde(default)]
    staged: bool,
}
async fn diff(
    State(s): State<Service>,
    Path((kind, id)): Path<(String, String)>,
    Query(q): Query<DiffQuery>,
) -> std::result::Result<Json<crate::review::Diff>, Error> {
    let _permit = s.permit()?;
    let directory = s.workspaces.directory(&kind, &id)?;
    Ok(Json(
        crate::review::diff(
            directory,
            s.workspaces.settings.private_paths(),
            &q.path,
            q.staged,
        )
        .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BytesQuery {
    path: String,
    revision: Option<String>,
}
async fn bytes(
    State(s): State<Service>,
    Path((kind, id)): Path<(String, String)>,
    Query(q): Query<BytesQuery>,
) -> std::result::Result<Response, Error> {
    let permit = s.permit()?;
    let value = disk(move || {
        let _permit = permit;
        let directory = s.workspaces.directory(&kind, &id)?;
        // Reuse the existing authorization and version checks; no independent permissive path resolver.
        let page = directory.read(&q.path, 0, 4, q.revision.as_deref())?;
        if page.bytes > 16 * 1024 * 1024 {
            return Err(error("capacity", "文件超过 16 MiB 限制。"));
        }
        let mut value = Vec::new();
        std::fs::File::open(directory.root().join(&q.path))
            .map_err(|_| error("io", "文件读取失败。"))?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut value)
            .map_err(|_| error("io", "文件读取失败。"))?;
        if value.len() > 16 * 1024 * 1024
            || format!("{:x}", Sha256::digest(&value)) != page.revision
        {
            return Err(error("conflict", "文件已变化，请刷新。"));
        }
        directory.read(&q.path, 0, 4, Some(&page.revision))?;
        Ok(value)
    })
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment"),
        ],
        value,
    )
        .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    request_id: String,
    rows: u16,
    cols: u16,
}
async fn open_terminal(
    State(s): State<Service>,
    Path((kind, id)): Path<(String, String)>,
    Json(q): Json<Open>,
) -> std::result::Result<Json<serde_json::Value>, Error> {
    let id = disk(move || {
        let directory = s.workspaces.directory(&kind, &id)?;
        s.terminals.open(
            Target { kind, id },
            &q.request_id,
            directory.root(),
            q.rows,
            q.cols,
        )
    })
    .await?;
    Ok(Json(serde_json::json!({"id":id})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    #[serde(default)]
    after: u64,
}
async fn terminal_output(
    State(s): State<Service>,
    Path((kind, id, terminal)): Path<(String, String, String)>,
    Query(q): Query<Cursor>,
) -> std::result::Result<Json<crate::terminal::Page>, Error> {
    Ok(Json(s.terminals.output(
        &Target { kind, id },
        &terminal,
        q.after,
    )?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    sequence: u64,
    data: String,
}
async fn terminal_input(
    State(s): State<Service>,
    Path((kind, id, terminal)): Path<(String, String, String)>,
    Json(q): Json<Input>,
) -> std::result::Result<StatusCode, Error> {
    disk(move || {
        s.terminals
            .input(&Target { kind, id }, &terminal, q.sequence, &q.data)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Size {
    rows: u16,
    cols: u16,
}
async fn terminal_size(
    State(s): State<Service>,
    Path((kind, id, terminal)): Path<(String, String, String)>,
    Json(q): Json<Size>,
) -> std::result::Result<StatusCode, Error> {
    disk(move || {
        s.terminals
            .resize(&Target { kind, id }, &terminal, q.rows, q.cols)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn close_terminal(
    State(s): State<Service>,
    Path((kind, id, terminal)): Path<(String, String, String)>,
) -> std::result::Result<StatusCode, Error> {
    disk(move || s.terminals.close(&Target { kind, id }, &terminal)).await?;
    Ok(StatusCode::NO_CONTENT)
}
