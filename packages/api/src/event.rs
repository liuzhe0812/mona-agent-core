use crate::{AgentError, Result, RunStatus, TaskUsage, ToolCall, ToolResult, ToolStatus, clip_utf8};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// This is our versioned UI protocol, NOT the Codex JSON-RPC wire protocol.
pub const STREAM_VERSION: u32 = 2;
pub const UI_TEXT_BYTES: usize = 64 * 1024;
pub const UI_RETAINED_ITEMS: usize = 256;
pub const UI_DETAIL_BYTES: usize = 16 * 1024;
pub const UI_DETAIL_KEYS: usize = 8;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemState { Pending, Running, Completed, Failed, Cancelled, Denied, Skipped, Unknown }
impl ItemState {
    pub fn terminal(self) -> bool { !matches!(self, Self::Pending | Self::Running) }
    pub fn from_tool(status: ToolStatus) -> Self {
        match status {
            ToolStatus::Success => Self::Completed, ToolStatus::Error => Self::Failed,
            ToolStatus::Denied => Self::Denied, ToolStatus::Skipped => Self::Skipped,
            ToolStatus::Unknown => Self::Unknown,
        }
    }
}

/// Intentionally distinct from the model's rich ToolResult. Keeps UI protocol v2.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiToolResult {
    pub call_id: String,
    pub status: ToolStatus,
    pub content: String,
    pub truncated: bool,
    pub original_bytes: usize,
    pub artifact: Option<crate::ArtifactRef>,
}
impl UiToolResult {
    pub fn from_result(result: &ToolResult) -> Self {
        let preview = result.content.preview();
        Self { call_id: result.call_id.clone(), status: result.status,
            content: clip_utf8(&preview, UI_TEXT_BYTES).to_owned(),
            truncated: result.truncated || preview.len() > UI_TEXT_BYTES || result.content.has_media()
                || result.structured.is_some(),
            original_bytes: result.original_bytes,
            artifact: result.artifact.clone().filter(|a| a.uri.len() <= 4096),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemContent {
    AgentMessage { text: String, truncated: bool },
    ToolCall {
        call_id: Option<String>, name: String,
        /// Preview only: never parse or execute a delta on the client.
        arguments_text: String, arguments: Option<Value>, arguments_truncated: bool,
        output: String, output_truncated: bool, result: Option<UiToolResult>,
        /// Namespaced structured UI details (e.g. coding.diff). Not executable instructions.
        details: BTreeMap<String, Value>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: String,
    pub step: usize,
    pub state: ItemState,
    pub content: ItemContent,
}
impl WorkItem {
    pub fn message(step: usize) -> Self {
        Self { id: message_item_id(step), step, state: ItemState::Running,
            content: ItemContent::AgentMessage { text: String::new(), truncated: false } }
    }
    pub fn tool(step: usize, index: usize) -> Self {
        Self { id: tool_item_id(step, index), step, state: ItemState::Pending,
            content: ItemContent::ToolCall { call_id: None, name: String::new(),
                arguments_text: String::new(), arguments: None, arguments_truncated: false,
                output: String::new(), output_truncated: false, result: None, details: BTreeMap::new() } }
    }
    pub fn set_call(&mut self, call: &ToolCall) {
        if let ItemContent::ToolCall { call_id, name, arguments_text, arguments, arguments_truncated, .. } = &mut self.content {
            *call_id = Some(call.id.clone()); *name = call.name.clone();
            let raw = call.arguments.to_string();
            *arguments_truncated = raw.len() > UI_TEXT_BYTES;
            *arguments_text = clip_utf8(&raw, UI_TEXT_BYTES).to_owned();
            *arguments = if *arguments_truncated { None } else { Some(call.arguments.clone()) };
        }
    }
}
pub fn message_item_id(step: usize) -> String { format!("step-{step}-message") }
pub fn tool_item_id(step: usize, index: usize) -> String { format!("step-{step}-tool-{index}") }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub output: Option<String>,
    pub error: Option<AgentError>,
    pub task_usage: TaskUsage,
    pub steps: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub protocol_version: u32,
    pub run_id: String,
    /// Monotonic per run, assigned atomically with snapshot mutation.
    pub seq: u64,
    pub event: RunEvent,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RunEvent {
    #[serde(rename = "run/started")]
    RunStarted,
    #[serde(rename = "step/started")]
    StepStarted { step: usize },
    #[serde(rename = "item/started")]
    ItemStarted { item: WorkItem },
    #[serde(rename = "item/updated")]
    ItemUpdated { item: WorkItem },
    #[serde(rename = "item/agentMessage/delta")]
    TextDelta { item_id: String, text: String },
    #[serde(rename = "item/toolCall/argumentsDelta")]
    ToolArgumentsDelta { item_id: String, call_id: Option<String>, name: Option<String>, delta: String },
    #[serde(rename = "item/toolCall/outputDelta")]
    ToolOutputDelta { item_id: String, text: String },
    #[serde(rename = "item/completed")]
    ItemCompleted { item: WorkItem },
    #[serde(rename = "input/applied")]
    InputApplied,
    #[serde(rename = "step/completed")]
    StepFinished { step: usize },
    #[serde(rename = "run/completed")]
    RunFinished { outcome: RunOutcome },
}

/// UI projection, not a transcript, durable event log or model context.
/// Text and item retention are bounded; original execution facts remain in RunReport.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub protocol_version: u32,
    pub run_id: String,
    pub seq: u64,
    pub started: bool,
    pub step: usize,
    pub items: Vec<WorkItem>,
    pub pruned_items: u64,
    pub outcome: Option<RunOutcome>,
}
impl RunSnapshot {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self { protocol_version: STREAM_VERSION, run_id: run_id.into(), seq: 0,
            started: false, step: 0, items: vec![], pruned_items: 0, outcome: None }
    }
    /// False means an out-of-order/gapped/wrong-run event: fetch an atomic snapshot.
    pub fn apply(&mut self, envelope: &EventEnvelope) -> bool {
        if envelope.protocol_version != STREAM_VERSION || envelope.run_id != self.run_id
            || envelope.seq != self.seq + 1 { return false; }
        self.seq = envelope.seq;
        match &envelope.event {
            RunEvent::RunStarted => self.started = true,
            RunEvent::StepStarted { step } => self.step = *step,
            RunEvent::ItemStarted { item } | RunEvent::ItemUpdated { item } | RunEvent::ItemCompleted { item } => {
                if let Some(old) = self.items.iter_mut().find(|old| old.id == item.id) { *old = item.clone(); }
                else {
                    if self.items.len() >= UI_RETAINED_ITEMS {
                        let index = self.items.iter().position(|i| i.state.terminal()).unwrap_or(0);
                        self.items.remove(index); self.pruned_items += 1;
                    }
                    self.items.push(item.clone());
                }
            }
            RunEvent::TextDelta { item_id, text } => {
                if let Some(WorkItem { content: ItemContent::AgentMessage { text: content, truncated }, .. }) = self.items.iter_mut().find(|i| &i.id == item_id) {
                    append_preview(content, truncated, text);
                }
            }
            RunEvent::ToolArgumentsDelta { item_id, call_id: id, name: new_name, delta } => {
                if let Some(WorkItem { content: ItemContent::ToolCall { call_id, name, arguments_text, arguments_truncated, .. }, .. }) = self.items.iter_mut().find(|i| &i.id == item_id) {
                    if id.is_some() { *call_id = id.clone(); }
                    if let Some(new_name) = new_name { *name = clip_utf8(new_name, 128).to_owned(); }
                    append_preview(arguments_text, arguments_truncated, delta);
                }
            }
            RunEvent::ToolOutputDelta { item_id, text } => {
                if let Some(WorkItem { content: ItemContent::ToolCall { output, output_truncated, .. }, .. }) = self.items.iter_mut().find(|i| &i.id == item_id) {
                    append_preview(output, output_truncated, text);
                }
            }
            RunEvent::RunFinished { outcome } => self.outcome = Some(outcome.clone()),
            RunEvent::InputApplied | RunEvent::StepFinished { .. } => {},
        }
        true
    }
}
fn append_preview(target: &mut String, truncated: &mut bool, delta: &str) {
    if *truncated { return; }
    let remaining = UI_TEXT_BYTES.saturating_sub(target.len());
    let part = clip_utf8(delta, remaining);
    target.push_str(part); *truncated |= part.len() < delta.len();
}

/// Best-effort observer; NOT a transaction participant or durable audit sink.
#[async_trait]
pub trait EventObserver: Send + Sync {
    async fn on_event(&self, event: EventEnvelope) -> Result<()>;
    async fn on_lagged(&self, _run_id: &str, _lost: u64) -> Result<()> { Ok(()) }
}
