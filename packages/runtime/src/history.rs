//! Canonical history accounting. No storage, summarization, or UI retention policy.
use api::{AgentError, ErrorCode, Message, Result, ToolCall, ToolResult, ToolStatus};
use serde::Serialize;
use std::{
    io::{self, Write},
    ops::Deref,
};

/// Count canonical JSON without allocating another complete copy of the history.
pub(crate) fn serialized_bytes<T: Serialize + ?Sized>(value: &T) -> Result<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| io::Error::other("serialized size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| AgentError::new(ErrorCode::Limit, "cannot measure canonical history"))?;
    Ok(counter.0)
}

pub(crate) fn history_limit() -> AgentError {
    AgentError::new(
        ErrorCode::Limit,
        "run history capacity reached; confirmed history retained, no further tool dispatched",
    )
}

pub(crate) const UNKNOWN_RESULT: &str =
    "driver stopped before settlement; execution state unknown, do not blindly replay";
const SLOT_NOTICE_BYTES: usize = 128;

pub(crate) struct History {
    messages: Vec<Message>,
    bytes: usize,
    limit: usize,
}
impl Deref for History {
    type Target = [Message];
    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}
impl History {
    pub fn new(messages: Vec<Message>, admission: usize, limit: usize) -> Result<Self> {
        let bytes = serialized_bytes(&messages)?;
        if bytes > admission {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "initial history exceeds admission byte limit",
            ));
        }
        if bytes > limit {
            return Err(history_limit());
        }
        Ok(Self {
            messages,
            bytes,
            limit,
        })
    }
    pub fn check_append(&self, messages: &[Message]) -> Result<usize> {
        let mut next = self.bytes;
        for (index, message) in messages.iter().enumerate() {
            let separator = usize::from(!self.messages.is_empty() || index != 0);
            next = next
                .checked_add(serialized_bytes(message)?)
                .and_then(|n| n.checked_add(separator))
                .ok_or_else(history_limit)?;
        }
        if next > self.limit {
            return Err(history_limit());
        }
        Ok(next)
    }
    pub fn extend(&mut self, messages: impl IntoIterator<Item = Message>) -> Result<()> {
        let messages: Vec<_> = messages.into_iter().collect();
        let next = self.check_append(&messages)?;
        self.messages.extend(messages);
        self.bytes = next;
        Ok(())
    }
    pub fn check_model_room(&self) -> Result<()> {
        self.check_append(&[Message::Assistant {
            content: String::new(),
            tool_calls: Vec::new(),
            reasoning_content: None,
            provider_data: None,
        }])
        .map(|_| ())
    }
    /// A response is admitted atomically with enough space to close every tool pair,
    /// even if no tool can start. Unadmitted responses never authorize execution.
    pub fn admit_reply(&mut self, message: Message) -> Result<ToolBudget> {
        let next = self.check_append(std::slice::from_ref(&message))?;
        let calls = match &message {
            Message::Assistant { tool_calls, .. } => tool_calls.as_slice(),
            _ => &[],
        };
        let budget = ToolBudget::new(self.limit - next, calls)?;
        self.messages.push(message);
        self.bytes = next;
        Ok(budget)
    }
    pub fn take(&mut self) -> Vec<Message> {
        self.bytes = 2;
        std::mem::take(&mut self.messages)
    }
}

#[derive(Serialize)]
struct ToolMessage<'a> {
    role: &'static str,
    result: &'a ToolResult,
}
fn result_bytes(result: &ToolResult) -> Result<usize> {
    serialized_bytes(&ToolMessage {
        role: "tool",
        result,
    })?
    .checked_add(1)
    .ok_or_else(history_limit)
}

/// Owned by one batch scheduler. Slots reserve space for waiting/running/settled calls;
/// actual results release spare capacity before another tool is dispatched.
pub(crate) struct ToolBudget {
    limit: usize,
    used: usize,
    slots: Vec<usize>,
}
impl ToolBudget {
    fn new(limit: usize, calls: &[ToolCall]) -> Result<Self> {
        let mut slots = Vec::with_capacity(calls.len());
        let mut used = 0usize;
        for call in calls {
            let mut fallback =
                ToolResult::new(&call.id, ToolStatus::Skipped, "x".repeat(SLOT_NOTICE_BYTES));
            fallback.original_bytes = usize::MAX;
            let bytes = result_bytes(&fallback)?;
            used = used.checked_add(bytes).ok_or_else(history_limit)?;
            slots.push(bytes);
        }
        if used > limit {
            return Err(history_limit());
        }
        Ok(Self { limit, used, slots })
    }
    fn replace(&mut self, index: usize, bytes: usize) -> Result<()> {
        let next = (self.used - self.slots[index])
            .checked_add(bytes)
            .ok_or_else(history_limit)?;
        if next > self.limit {
            return Err(history_limit());
        }
        self.used = next;
        self.slots[index] = bytes;
        Ok(())
    }
    pub fn settle(&mut self, index: usize, result: &ToolResult) -> Result<()> {
        self.replace(index, result_bytes(result)?)
    }
    /// A text byte can occupy six JSON bytes (e.g. a control character). Rich payloads
    /// already count their own serialization. Reserve the envelope, optional keys and
    /// largest numeric fields too. This is a bound, not a per-tool memory allocation.
    pub fn reserve(&mut self, index: usize, call_id: &str, max_payload: usize) -> Result<()> {
        let empty = ToolResult::new(call_id, ToolStatus::Skipped, "");
        let bytes = max_payload
            .checked_mul(6)
            .and_then(|n| n.checked_add(64))
            .and_then(|n| {
                result_bytes(&empty)
                    .ok()
                    .and_then(|base| base.checked_add(n))
            })
            .ok_or_else(history_limit)?;
        self.replace(index, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{ArtifactRef, Content, ContentBlock};
    #[test]
    fn incremental_count_includes_json_escaping_and_never_mutates_on_rejection() {
        let initial = vec![Message::user("\u{0}中文\\\"\n")];
        let second = Message::user("next");
        let exact = serialized_bytes(&[initial[0].clone(), second.clone()]).unwrap();
        let mut history = History::new(initial, exact, exact).unwrap();
        history.extend([second]).unwrap();
        assert_eq!(history.bytes, serialized_bytes(&*history).unwrap());
        assert!(history.extend([Message::user("no room")]).is_err());
        assert_eq!(history.len(), 2);
    }
    #[test]
    fn tool_reservation_bounds_text_rich_results_and_long_escaped_identifiers() {
        let id = "\u{0}".repeat(256);
        let call = ToolCall::new(&id, "tool", serde_json::json!({}));
        for content in [
            Content::from("\u{0}".repeat(1000)),
            Content::Blocks(vec![ContentBlock::Text {
                text: "\u{0}".repeat(100),
            }]),
        ] {
            let mut result = ToolResult::new(&id, ToolStatus::Unknown, content);
            result.structured = Some(serde_json::json!({"escaped":"\u{0}".repeat(100)}));
            result.artifact = Some(ArtifactRef {
                uri: "spill:abc".into(),
                bytes: u64::MAX,
            });
            result.original_bytes = usize::MAX;
            let mut budget = ToolBudget::new(usize::MAX, std::slice::from_ref(&call)).unwrap();
            budget.reserve(0, &id, result.payload_bytes()).unwrap();
            let reserved = budget.used;
            budget.settle(0, &result).unwrap();
            assert!(budget.used <= reserved);
            assert_eq!(
                budget.used,
                serialized_bytes(&Message::Tool { result }).unwrap() + 1
            );
        }
    }
}
