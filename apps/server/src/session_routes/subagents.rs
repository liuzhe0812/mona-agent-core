//! Subagent product views reuse the session router's authentication and safe history projection.
use super::*;
use serde_json::{json, Value};
use subagent::Config;

type ResponseResult = std::result::Result<Json<Value>, HttpError>;
fn host(
    s: &Service,
) -> std::result::Result<(Arc<crate::subagent_setup::SubagentHost>, bool), HttpError> {
    let f = &s
        .workspaces
        .as_ref()
        .ok_or_else(|| error(Code::NotFound, "当前宿主未装配子 Agent。"))?
        .environments
        .factory;
    Ok((
        f.subagents.clone(),
        !f.demo && f.capabilities.active(crate::capabilities::SUBAGENT),
    ))
}
fn agent_error(e: api::AgentError) -> HttpError {
    let code = match e.code {
        api::ErrorCode::Limit => Code::Capacity,
        api::ErrorCode::Checkpoint | api::ErrorCode::Plugin => Code::Internal,
        _ => Code::Conflict,
    };
    HttpError(error(code, &e.message))
}
fn encode(value: impl serde::Serialize) -> ResponseResult {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|_| error(Code::Internal, "子任务展示编码失败。").into())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Update {
    revision: u64,
    config: Config,
}
pub(super) async fn settings(State(s): State<Service>) -> ResponseResult {
    let (host, enabled) = host(&s)?;
    encode(host.view(enabled).map_err(agent_error)?)
}
pub(super) async fn save_settings(
    State(s): State<Service>,
    request: std::result::Result<Json<Update>, JsonRejection>,
) -> ResponseResult {
    let request = body(request)?;
    let (host, enabled) = host(&s)?;
    let view =
        tokio::task::spawn_blocking(move || host.save(request.revision, request.config, enabled))
            .await
            .map_err(|_| error(Code::Internal, "配置保存结果未知，请重新读取。"))?
            .map_err(agent_error)?;
    encode(view)
}
pub(super) async fn list(State(s): State<Service>, Path(root): Path<String>) -> ResponseResult {
    let (host, enabled) = host(&s)?;
    let mut agents = host.service.list(&root).await.map_err(agent_error)?;
    for agent in &mut agents {
        agent.output = None;
    }
    Ok(Json(
        json!({"version":1,"root":root,"enabled":enabled,"parent_run":host.service.active_parent(&root),"agents":agents}),
    ))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChildPage {
    turn: Option<String>,
    before: Option<usize>,
}
pub(super) async fn detail(
    State(s): State<Service>,
    Path((root, id)): Path<(String, String)>,
    q: std::result::Result<Query<ChildPage>, axum::extract::rejection::QueryRejection>,
) -> ResponseResult {
    let q = query(q)?;
    let (host, enabled) = host(&s)?;
    let agent = host.service.view(&root, &id).await.map_err(agent_error)?;
    let store = s.store.clone();
    let page = disk(move || {
        let doc = store.get(&id)?;
        let chosen = q
            .turn
            .as_deref()
            .or_else(|| doc.body.turns.last().map(|t| t.id.as_str()));
        let page = chosen
            .map(|turn| view::turn_page(&doc, turn, q.before, 30))
            .transpose()?;
        let turns: Vec<view::TurnSummary> = doc
            .body
            .turns
            .iter()
            .rev()
            .take(50)
            .map(Into::into)
            .collect();
        Ok(json!({"history":page,"turns":turns,"older_turns":doc.body.turns.len()>50}))
    })
    .await?;
    Ok(Json(
        json!({"version":1,"root":root,"enabled":enabled,"agent":agent,"page":page}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stop {
    run_id: String,
}
pub(super) async fn interrupt(
    State(s): State<Service>,
    Path((root, id)): Path<(String, String)>,
    request: std::result::Result<Json<Stop>, JsonRejection>,
) -> ResponseResult {
    let request = body(request)?;
    let (host, _) = host(&s)?;
    if request.run_id.is_empty() || request.run_id.len() > 128 {
        return Err(error(Code::InvalidRequest, "停止操作缺少准确的运行身份。").into());
    }
    encode(
        host.service
            .interrupt(&root, &id, Some(&request.run_id))
            .await
            .map_err(agent_error)?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Message {
    request_id: String,
    parent_run: String,
    text: String,
}
fn control(
    s: &Service,
    root: &str,
    request: &Message,
) -> std::result::Result<Arc<crate::subagent_setup::SubagentHost>, HttpError> {
    let (host, enabled) = host(s)?;
    if !enabled {
        return Err(error(Code::Closed, "子 Agent 当前未启用；已有记录仅供查看。").into());
    }
    if !(1..=64).contains(&request.request_id.len())
        || !request
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(error(Code::InvalidRequest, "操作编号无效。").into());
    }
    if host.service.active_parent(root).as_deref() != Some(request.parent_run.as_str()) {
        return Err(error(
            Code::Conflict,
            "主任务已结束或变更；请从主会话明确继续，不能另建不受预算约束的子任务。",
        )
        .into());
    }
    Ok(host)
}
pub(super) async fn message(
    State(s): State<Service>,
    Path((root, id)): Path<(String, String)>,
    request: std::result::Result<Json<Message>, JsonRejection>,
) -> ResponseResult {
    let request = body(request)?;
    let host = control(&s, &root, &request)?;
    host.service
        .send_message(
            &request.parent_run,
            &format!("ui:{}", request.request_id),
            &id,
            &request.text,
        )
        .await
        .map_err(agent_error)?;
    Ok(Json(
        json!({"accepted":true,"starts_turn":false,"delivery":"next_model_request"}),
    ))
}
pub(super) async fn followup(
    State(s): State<Service>,
    Path((root, id)): Path<(String, String)>,
    request: std::result::Result<Json<Message>, JsonRejection>,
) -> ResponseResult {
    let request = body(request)?;
    let host = control(&s, &root, &request)?;
    encode(
        host.service
            .followup(
                &request.parent_run,
                &format!("ui:{}", request.request_id),
                &id,
                &request.text,
            )
            .await
            .map_err(agent_error)?,
    )
}
