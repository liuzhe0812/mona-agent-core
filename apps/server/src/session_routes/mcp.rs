//! MCP management shares the product session router's bearer/CORS/body boundaries.
use super::*;
use serde_json::Value;
fn host(s: &Service) -> std::result::Result<(Arc<crate::mcp_setup::McpHost>, bool), HttpError> {
    let f = &s
        .workspaces
        .as_ref()
        .ok_or_else(|| error(Code::NotFound, "当前宿主未装配 MCP。"))?
        .environments
        .factory;
    Ok((
        f.mcp.clone(),
        !f.demo && f.capabilities.active(crate::capabilities::MCP),
    ))
}
fn failure(e: api::AgentError) -> HttpError {
    error(Code::Conflict, &e.message).into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Update {
    revision: u64,
    id: String,
    server: ::mcp::Server,
    #[serde(default)]
    clear_secrets: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Remove {
    revision: u64,
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Connect {
    id: String,
}
pub(super) async fn settings(
    State(s): State<Service>,
) -> std::result::Result<Json<Value>, HttpError> {
    let (h, enabled) = host(&s)?;
    Ok(Json(h.view(enabled).map_err(failure)?))
}
pub(super) async fn upsert(
    State(s): State<Service>,
    value: std::result::Result<Json<Update>, JsonRejection>,
) -> std::result::Result<Json<Value>, HttpError> {
    let r = body(value)?;
    let (h, enabled) = host(&s)?;
    let view = tokio::task::spawn_blocking(move || {
        h.upsert(r.revision, r.id, r.server, r.clear_secrets, enabled)
    })
    .await
    .map_err(|_| error(Code::Internal, "MCP 保存结果未知，请重新读取；未自动重试。"))?
    .map_err(failure)?;
    Ok(Json(view))
}
pub(super) async fn remove(
    State(s): State<Service>,
    value: std::result::Result<Json<Remove>, JsonRejection>,
) -> std::result::Result<Json<Value>, HttpError> {
    let r = body(value)?;
    let (h, enabled) = host(&s)?;
    let view = tokio::task::spawn_blocking(move || h.delete(r.revision, &r.id, enabled))
        .await
        .map_err(|_| error(Code::Internal, "MCP 删除结果未知，请重新读取。"))?
        .map_err(failure)?;
    Ok(Json(view))
}
pub(super) async fn reconnect(
    State(s): State<Service>,
    value: std::result::Result<Json<Connect>, JsonRejection>,
) -> std::result::Result<Json<Value>, HttpError> {
    let r = body(value)?;
    let (h, enabled) = host(&s)?;
    if !enabled {
        return Err(error(Code::Closed, "MCP 未启用，重启后再连接。").into());
    }
    h.service.reconnect(&r.id).await.map_err(failure)?;
    Ok(Json(h.view(enabled).map_err(failure)?))
}
