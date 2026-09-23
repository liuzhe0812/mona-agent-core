use crate::{AgentError, Content, ErrorCode, ProviderData, Result, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Opaque adapter replay data. Not arguments and never included in UI work items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_data: Option<ProviderData>,
}
impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            provider_data: None,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
    Denied,
    Skipped,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArtifactRef {
    /// Application-owned locator; the core does not dereference it.
    pub uri: String,
    pub bytes: u64,
}
impl ArtifactRef {
    pub fn validate(&self) -> Result<()> {
        if self.uri.is_empty() || self.uri.len() > 4096 || self.uri.chars().any(char::is_control) {
            return Err(AgentError::new(
                ErrorCode::Schema,
                "invalid artifact locator",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub status: ToolStatus,
    pub content: Content,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    pub truncated: bool,
    pub original_bytes: usize,
    pub artifact: Option<ArtifactRef>,
}
impl ToolResult {
    pub fn new(
        call_id: impl Into<String>,
        status: ToolStatus,
        content: impl Into<Content>,
    ) -> Self {
        let content = content.into();
        let original_bytes = content.byte_len();
        Self {
            call_id: call_id.into(),
            status,
            content,
            structured: None,
            truncated: false,
            original_bytes,
            artifact: None,
        }
    }
    pub fn from_output(call_id: impl Into<String>, output: ToolOutput) -> Self {
        let mut result = Self::new(
            call_id,
            if output.is_error {
                ToolStatus::Error
            } else {
                ToolStatus::Success
            },
            output.content,
        );
        result.structured = output.structured;
        result.artifact = output.artifact;
        result.original_bytes = result.payload_bytes();
        result
    }
    pub fn validate(&self) -> Result<()> {
        self.content.validate()?;
        if let Some(artifact) = &self.artifact {
            artifact.validate()?;
        }
        Ok(())
    }
    pub fn payload_bytes(&self) -> usize {
        self.content
            .byte_len()
            .saturating_add(
                self.structured
                    .as_ref()
                    .map_or(0, |v| serde_json::to_vec(v).map_or(usize::MAX, |s| s.len())),
            )
            .saturating_add(
                self.artifact
                    .as_ref()
                    .map_or(0, |v| serde_json::to_vec(v).map_or(usize::MAX, |s| s.len())),
            )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: Content,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
        /// Legacy provider-returned protocol data, never default UI reasoning.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_data: Option<ProviderData>,
    },
    Tool {
        result: ToolResult,
    },
}
impl Message {
    pub fn user(content: impl Into<Content>) -> Self {
        Self::User {
            content: content.into(),
        }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Self::System { content } | Self::Assistant { content, .. } => Cow::Borrowed(content),
            Self::User { content } => content.text(),
            Self::Tool { result } => result.content.text(),
        }
    }
}
/// Byte limit with a UTF-8 boundary. A caller must separately mark truncation.
pub fn clip_utf8(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
