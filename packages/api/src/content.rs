//! Model-visible content. Media bytes are never implicitly fetched or exposed to UI.
use crate::{AgentError, ArtifactRef, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// A string remains a string on the wire; rich input is an ordered block array.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}
impl Default for Content {
    fn default() -> Self {
        Self::Text(String::new())
    }
}
impl From<String> for Content {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl From<&str> for Content {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}
impl From<&String> for Content {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}
impl From<Vec<ContentBlock>> for Content {
    fn from(value: Vec<ContentBlock>) -> Self {
        Self::Blocks(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        source: ImageSource,
    },
    /// Application-owned reference. An adapter must explicitly support/resolve it,
    /// or reject it. A URI is NOT the contents of the referenced file.
    Resource {
        reference: ArtifactRef,
        media_type: String,
        name: Option<String>,
    },
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { data: String },
    Url { url: String },
}
impl std::fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base64 { data } => f
                .debug_struct("Base64")
                .field("encoded_bytes", &data.len())
                .finish(),
            Self::Url { .. } => f.write_str("Url(<redacted>)"),
        }
    }
}
impl Content {
    /// Text-only projection; never substitutes a locator/base64 string for vision.
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Self::Text(text) => Cow::Borrowed(text),
            Self::Blocks(blocks) => Cow::Owned(
                blocks
                    .iter()
                    .filter_map(|block| {
                        if let ContentBlock::Text { text } = block {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        }
    }
    pub fn has_media(&self) -> bool {
        matches!(self, Self::Blocks(blocks) if blocks.iter().any(|b| !matches!(b, ContentBlock::Text { .. })))
    }
    /// Text byte length for legacy text, serialized size for blocks.
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text(s) => s.len(),
            Self::Blocks(_) => serde_json::to_vec(self).map_or(usize::MAX, |v| v.len()),
        }
    }
    /// Safe default UI preview: no inline image bytes, URLs, or file paths.
    pub fn preview(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::Image { media_type, .. } => format!("[image: {media_type}]"),
                    ContentBlock::Resource { media_type, .. } => {
                        format!("[resource: {media_type}]")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
    pub fn validate(&self) -> Result<()> {
        let Self::Blocks(blocks) = self else {
            return Ok(());
        };
        if blocks.is_empty() || blocks.len() > 128 {
            return Err(AgentError::new(
                ErrorCode::Schema,
                "content must contain 1..128 blocks",
            ));
        }
        for block in blocks {
            match block {
                ContentBlock::Text { .. } => {}
                ContentBlock::Image { media_type, source } => {
                    if !matches!(
                        media_type.as_str(),
                        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                    ) {
                        return Err(AgentError::new(
                            ErrorCode::Unsupported,
                            "unsupported image media type",
                        ));
                    }
                    match source {
                        ImageSource::Base64 { data } if !valid_base64(data) => {
                            return Err(AgentError::new(
                                ErrorCode::Schema,
                                "image data must be nonempty canonical padded base64",
                            ));
                        }
                        ImageSource::Url { url }
                            if url.len() > 4096
                                || !url.starts_with("https://")
                                || url.chars().any(char::is_whitespace)
                                || url.contains('#') =>
                        {
                            return Err(AgentError::new(
                                ErrorCode::Schema,
                                "image URL must be bounded HTTPS without fragments",
                            ));
                        }
                        _ => {}
                    }
                }
                ContentBlock::Resource {
                    reference,
                    media_type,
                    name,
                } => {
                    reference.validate()?;
                    if reference.uri.is_empty()
                        || reference.uri.len() > 4096
                        || media_type.is_empty()
                        || media_type.len() > 128
                        || name.as_ref().is_some_and(|n| n.len() > 256)
                    {
                        return Err(AgentError::new(
                            ErrorCode::Schema,
                            "invalid resource descriptor",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

// Validate the encoding without decoding/allocating a second image buffer.
// This checks base64 syntax, not whether bytes really decode to an image.
fn valid_base64(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return false;
    }
    let padding = bytes.iter().rev().take_while(|&&b| b == b'=').count();
    if padding > 2 {
        return false;
    }
    let data = &bytes[..bytes.len() - padding];
    let value = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    if data.iter().any(|&b| value(b).is_none()) {
        return false;
    }
    let last = data.last().and_then(|&b| value(b)).unwrap_or(0);
    match padding {
        1 => last & 3 == 0,
        2 => last & 15 == 0,
        _ => true,
    }
}
