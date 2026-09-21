use agent_api::{AgentError, ErrorCode, Message, Result};
use std::collections::BTreeSet;

/// Validate pairing after every context transform, never send orphaned tool results.
pub fn validate_messages(messages: &[Message]) -> Result<()> {
    if messages.is_empty() {
        return Err(AgentError::new(ErrorCode::ModelProtocol, "empty model history"));
    }
    let mut pending = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for message in messages {
        match message {
            Message::User { content } => content.validate()?,
            Message::Tool { result } => result.validate()?,
            Message::Assistant { provider_data, tool_calls, .. } => {
                if let Some(data) = provider_data { data.validate()?; }
                for call in tool_calls { if let Some(data) = &call.provider_data { data.validate()?; } }
            }
            _ => {}
        }
        match message {
            Message::Tool { result } => {
                if !pending.remove(&result.call_id) {
                    return Err(AgentError::new(ErrorCode::ModelProtocol, "orphaned or duplicated tool result"));
                }
            }
            Message::Assistant { tool_calls, .. } => {
                if !pending.is_empty() {
                    return Err(AgentError::new(ErrorCode::ModelProtocol, "assistant message before tool batch settled"));
                }
                for call in tool_calls {
                    if call.id.is_empty() || !seen.insert(call.id.clone()) {
                        return Err(AgentError::new(ErrorCode::ModelProtocol, "empty or repeated tool-call id"));
                    }
                    pending.insert(call.id.clone());
                }
            }
            Message::System { .. } | Message::User { .. } => {
                if !pending.is_empty() {
                    return Err(AgentError::new(ErrorCode::ModelProtocol, "input interleaves an unsettled tool batch"));
                }
            }
        }
    }
    if !pending.is_empty() {
        return Err(AgentError::new(ErrorCode::ModelProtocol, "missing tool result(s)"));
    }
    Ok(())
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
