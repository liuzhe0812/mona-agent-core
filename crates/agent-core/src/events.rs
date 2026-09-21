use crate::gate::{lifecycle, lock};
use agent_api::*;
use std::{sync::{Arc, Mutex}, time::Duration};
use tokio::{sync::{broadcast, watch}, task::JoinHandle};

#[derive(Clone)]
pub(crate) struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
    state: Arc<Mutex<RunSnapshot>>,
}
impl EventBus {
    pub fn new(run_id: String, capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender, state: Arc::new(Mutex::new(RunSnapshot::new(run_id))) }
    }
    pub fn snapshot(&self) -> RunSnapshot { lock(&self.state).clone() }
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> { self.sender.subscribe() }
    fn publish(&self, state: &mut RunSnapshot, event: RunEvent) {
        if state.outcome.is_some() { return; }
        let envelope = EventEnvelope { protocol_version: STREAM_VERSION,
            run_id: state.run_id.clone(), seq: state.seq + 1, event };
        let applied = state.apply(&envelope);
        debug_assert!(applied);
        let _ = self.sender.send(envelope);
    }
    pub fn emit(&self, event: RunEvent) { self.publish(&mut lock(&self.state), event); }
    fn change(&self, id: &str, complete: bool, f: impl FnOnce(&mut WorkItem)) {
        let mut state = lock(&self.state);
        let Some(mut item) = state.items.iter().find(|i| i.id == id && !i.state.terminal()).cloned() else { return; };
        f(&mut item);
        let event = if complete { RunEvent::ItemCompleted { item } } else { RunEvent::ItemUpdated { item } };
        self.publish(&mut state, event);
    }
    pub fn complete_message(&self, step: usize, text: &str) {
        self.change(&message_item_id(step), true, |item| {
            item.state = ItemState::Completed;
            item.content = ItemContent::AgentMessage {
                text: clip_utf8(text, UI_TEXT_BYTES).to_owned(), truncated: text.len() > UI_TEXT_BYTES,
            };
        });
    }
    pub fn tool_call(&self, step: usize, index: usize, call: &ToolCall) {
        // Normally created by the model delta; also supports adapters with no tool preview.
        let mut state = lock(&self.state);
        let id = tool_item_id(step, index);
        let mut item = state.items.iter().find(|i| i.id == id).cloned().unwrap_or_else(|| WorkItem::tool(step, index));
        let existed = state.items.iter().any(|i| i.id == id);
        item.set_call(call);
        self.publish(&mut state, if existed { RunEvent::ItemUpdated { item } } else { RunEvent::ItemStarted { item } });
    }
    pub fn start_tool(&self, step: usize, index: usize) {
        self.change(&tool_item_id(step, index), false, |item| item.state = ItemState::Running);
    }
    pub fn complete_tool(&self, step: usize, index: usize, result: &ToolResult) {
        self.change(&tool_item_id(step, index), true, |item| {
            item.state = ItemState::from_tool(result.status);
            if let ItemContent::ToolCall { result: final_result, .. } = &mut item.content {
                *final_result = Some(UiToolResult::from_result(result));
            }
        });
    }
    pub fn finish(&self, outcome: RunOutcome) {
        let mut state = lock(&self.state);
        let unfinished = state.items.iter().filter(|i| !i.state.terminal()).cloned().collect::<Vec<_>>();
        for mut item in unfinished {
            item.state = match &item.content {
                ItemContent::ToolCall { .. } if item.state == ItemState::Running => ItemState::Unknown,
                ItemContent::ToolCall { .. } => ItemState::Skipped,
                _ if outcome.status == RunStatus::Cancelled => ItemState::Cancelled,
                _ => ItemState::Failed,
            };
            self.publish(&mut state, RunEvent::ItemCompleted { item });
        }
        self.publish(&mut state, RunEvent::RunFinished { outcome });
    }
    fn tool_delta(&self, step: usize, index: usize, id: Option<&str>, name: Option<&str>, args: &str) {
        let mut state = lock(&self.state);
        if state.outcome.is_some() { return; }
        let item_id = tool_item_id(step, index);
        if !state.items.iter().any(|i| i.id == item_id) {
            self.publish(&mut state, RunEvent::ItemStarted { item: WorkItem::tool(step, index) });
        }
        let mut rest = args;
        // Even an empty delta may carry metadata.
        loop {
            let part = clip_utf8(rest, 4096);
            self.publish(&mut state, RunEvent::ToolArgumentsDelta { item_id: item_id.clone(),
                call_id: id.map(|s| clip_utf8(s, 256).to_owned()), name: name.map(|s| clip_utf8(s, 128).to_owned()),
                delta: part.to_owned() });
            rest = &rest[part.len()..];
            if rest.is_empty() { break; }
        }
    }
    fn progress(&self, item_id: &str, text: &str) {
        let mut state = lock(&self.state);
        if !state.items.iter().any(|i| i.id == item_id && i.state == ItemState::Running) { return; }
        // Bounded preview; the final tool result is a separate authoritative value.
        let mut rest = text;
        while !rest.is_empty() {
            let part = clip_utf8(rest, 4096);
            self.publish(&mut state, RunEvent::ToolOutputDelta { item_id: item_id.to_owned(), text: part.to_owned() });
            rest = &rest[part.len()..];
        }
    }
    fn detail(&self, item_id: &str, key: &str, value: serde_json::Value) -> Result<()> {
        if key.len() > 128 || !key.contains('.') || !key.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) {
            return Err(AgentError::new(ErrorCode::Schema, "UI detail requires a namespaced ASCII key"));
        }
        if serde_json::to_vec(&value).map_or(true, |v| v.len() > UI_DETAIL_BYTES) {
            return Err(AgentError::new(ErrorCode::Limit, "UI detail exceeds byte limit"));
        }
        let mut state = lock(&self.state);
        let Some(mut item) = state.items.iter().find(|i| i.id == item_id && i.state == ItemState::Running).cloned() else {
            return Err(AgentError::new(ErrorCode::Closed, "tool item is not running"));
        };
        if let ItemContent::ToolCall { details, .. } = &mut item.content {
            if details.len() >= UI_DETAIL_KEYS && !details.contains_key(key) {
                return Err(AgentError::new(ErrorCode::Limit, "too many structured UI details"));
            }
            details.insert(key.to_owned(), value);
        }
        self.publish(&mut state, RunEvent::ItemUpdated { item });
        Ok(())
    }
    pub fn observe(&self, observer: Arc<dyn EventObserver>, duration: Duration, mut done: watch::Receiver<bool>) -> JoinHandle<()> {
        let mut receiver = self.subscribe();
        let run_id = lock(&self.state).run_id.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! { item = receiver.recv() => item, _ = done.changed() => break };
                match event {
                    Ok(event) => { let _ = lifecycle(duration, observer.on_event(event)).await; }
                    Err(broadcast::error::RecvError::Lagged(lost)) => { let _ = lifecycle(duration, observer.on_lagged(&run_id, lost)).await; }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
                if *done.borrow() { break; }
            }
        })
    }
}

pub(crate) struct TextSink { bus: EventBus, step: usize }
impl TextSink {
    pub fn new(bus: EventBus, step: usize) -> Self {
        bus.emit(RunEvent::ItemStarted { item: WorkItem::message(step) });
        Self { bus, step }
    }
}
impl ModelSink for TextSink {
    fn text(&self, delta: &str) {
        let mut rest = delta;
        while !rest.is_empty() {
            let part = clip_utf8(rest, 4096);
            self.bus.emit(RunEvent::TextDelta { item_id: message_item_id(self.step), text: part.to_owned() });
            rest = &rest[part.len()..];
        }
    }
    fn tool_delta(&self, index: usize, id: Option<&str>, name: Option<&str>, arguments: &str) {
        self.bus.tool_delta(self.step, index, id, name, arguments);
    }
}
pub(crate) struct ProgressSink { pub bus: EventBus, pub item_id: String }
impl ToolProgress for ProgressSink {
    fn report(&self, text: &str) { self.bus.progress(&self.item_id, text); }
    fn set_detail(&self, key: &str, value: serde_json::Value) -> Result<()> { self.bus.detail(&self.item_id, key, value) }
}
