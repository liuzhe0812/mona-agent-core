//! Browser-safe historical projections. Never serialize a checkpoint, audit or provider replay data.
use super::store::{error, Document, Header, Status, Turn};
use api::*;
use application::{ApplicationErrorCode as Code, ApplicationResult};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct TurnSummary {
    pub id: String, pub run_id: Option<String>, pub prompt: String,
    pub started_at: u64, pub finished_at: Option<u64>, pub status: Status,
}
impl From<&Turn> for TurnSummary {
    fn from(t: &Turn) -> Self {
        Self { id: t.id.clone(), run_id: t.run_id.clone(), prompt: t.prompt.clone(),
            started_at: t.started_at, finished_at: t.finished_at, status: t.status }
    }
}
#[derive(Serialize)]
pub struct SessionPage {
    pub session: Header, pub turns: Vec<TurnSummary>, pub next_before: Option<usize>,
}
pub fn session_page(doc: Document, before: Option<usize>, limit: usize) -> ApplicationResult<SessionPage> {
    if !(1..=20).contains(&limit) { return Err(error(Code::InvalidRequest, "每页会话记录数量应为 1–20。")); }
    let end = before.unwrap_or(doc.body.turns.len()).min(doc.body.turns.len());
    let start = end.saturating_sub(limit);
    Ok(SessionPage { turns: doc.body.turns[start..end].iter().map(Into::into).collect(),
        session: doc.header, next_before: (start > 0).then_some(start) })
}
#[derive(Serialize)]
pub struct TurnPage {
    pub turn: TurnSummary, pub snapshot: RunSnapshot, pub supplemental_inputs: Vec<String>,
    pub next_before: Option<usize>, pub total_items: usize,
}
pub fn turn_page(doc: &Document, id: &str, before: Option<usize>, limit: usize) -> ApplicationResult<TurnPage> {
    if !(1..=50).contains(&limit) { return Err(error(Code::InvalidRequest, "每页执行记录数量应为 1–50。")); }
    let turn = doc.body.turns.iter().find(|t| t.id == id).ok_or_else(|| error(Code::NotFound, "这轮对话不存在。"))?;
    let history = doc.history()?;
    let messages = history.get(turn.start..turn.end).ok_or_else(|| error(Code::Internal, "会话消息范围损坏，未修改原文件。"))?;
    let results: BTreeMap<_, _> = messages.iter().filter_map(|m| match m {
        Message::Tool { result } => Some((result.call_id.as_str(), result)), _ => None,
    }).collect();
    let mut items = Vec::new();
    let mut inputs = Vec::new();
    let mut step = 0;
    let mut output = None;
    for message in messages {
        match message {
            Message::Assistant { content, tool_calls, .. } => {
                step += 1;
                if !content.is_empty() {
                    let mut item = WorkItem::message(step);
                    item.state = ItemState::Completed;
                    item.content = ItemContent::AgentMessage { text: clip_utf8(content, UI_TEXT_BYTES).into(), truncated: content.len() > UI_TEXT_BYTES };
                    items.push(item);
                }
                output = tool_calls.is_empty().then(|| clip_utf8(content, UI_TEXT_BYTES).to_owned());
                for (index, call) in tool_calls.iter().enumerate() {
                    let mut item = WorkItem::tool(step, index); item.set_call(call);
                    if let Some(result) = results.get(call.id.as_str()) {
                        item.state = ItemState::from_tool(result.status);
                        if let ItemContent::ToolCall { result: value, .. } = &mut item.content { *value = Some(UiToolResult::from_result(result)); }
                    }
                    items.push(item);
                }
            }
            Message::User { content } => inputs.push(content.text().into_owned()),
            _ => {},
        }
    }
    let total_items = items.len();
    let end = before.unwrap_or(total_items).min(total_items);
    let start = end.saturating_sub(limit);
    let mut snapshot = RunSnapshot::new(turn.run_id.as_deref().unwrap_or(&turn.id));
    snapshot.started = true; snapshot.step = step;
    snapshot.items = items.drain(start..end).collect();
    snapshot.pruned_items = (total_items - snapshot.items.len()) as u64;
    if turn.status != Status::Running {
        let mut failure = turn.error.clone();
        if let Some(e) = &mut failure {
            e.message = if turn.status == Status::Interrupted {
                "上次进程已中断；已恢复确认过的记录，不会自动重跑工具。".into()
            } else { "run stopped; inspect trusted host diagnostics for details".into() };
        }
        snapshot.outcome = Some(RunOutcome { status: turn.status.outcome(),
            output: if turn.status == Status::Completed { output } else { None },
            error: failure, task_usage: turn.usage.clone(), steps: step });
    }
    Ok(TurnPage { turn: turn.into(), snapshot, supplemental_inputs: inputs.into_iter().skip(1).collect(),
        next_before: (start > 0).then_some(start), total_items })
}
