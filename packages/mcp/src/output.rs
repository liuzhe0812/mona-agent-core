use crate::{bounded, error};
use api::{Content, ContentBlock, ImageSource, Result, ToolOutput};
use serde_json::Value;
/// Preserve ordered text/images. Locators are text, never implicitly fetched or executed.
pub(crate) fn project(value: Value) -> Result<ToolOutput> {
    bounded(&value, 4 * 1024 * 1024, "result")?;
    let mut blocks = Vec::new();
    let is_error = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let source = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| error("MCP response has no content array"))?;
    if source.len() > 128 {
        return Err(error("MCP result has too many blocks"));
    }
    for block in source {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => blocks.push(ContentBlock::Text {
                text: block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error("invalid MCP text block"))?
                    .into(),
            }),
            Some("image") => {
                let image = ContentBlock::Image {
                    media_type: block
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    source: ImageSource::Base64 {
                        data: block
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    },
                };
                if Content::Blocks(vec![image.clone()]).validate().is_ok() {
                    blocks.push(image)
                } else {
                    blocks.push(ContentBlock::Text {
                        text: "[MCP image omitted: unsupported media type or invalid base64]"
                            .into(),
                    });
                }
            }
            Some("resource_link") => blocks.push(ContentBlock::Text {
                text: format!(
                    "[MCP resource link: {} · {}]",
                    block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("resource"),
                    block.get("uri").and_then(Value::as_str).unwrap_or("")
                ),
            }),
            Some("resource") => {
                let resource = block
                    .get("resource")
                    .ok_or_else(|| error("invalid embedded MCP resource"))?;
                blocks.push(ContentBlock::Text {
                    text: resource
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| {
                            format!(
                                "[MCP embedded resource: {}]\n{text}",
                                resource.get("uri").and_then(Value::as_str).unwrap_or("")
                            )
                        })
                        .unwrap_or_else(|| {
                            "[MCP embedded binary resource omitted; no binary-to-text decoding]"
                                .into()
                        }),
                });
            }
            Some("audio") => blocks.push(ContentBlock::Text {
                text: "[MCP audio is not supported by this Agent content contract]".into(),
            }),
            _ => return Err(error("unsupported MCP content block")),
        }
    }
    if blocks.is_empty() {
        blocks.push(ContentBlock::Text {
            text: value
                .get("structuredContent")
                .map(Value::to_string)
                .unwrap_or_else(|| "MCP tool returned no content.".into()),
        });
    }
    let content = if blocks
        .iter()
        .all(|b| matches!(b, ContentBlock::Text { .. }))
    {
        Content::Text(
            blocks
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    } else {
        Content::Blocks(blocks)
    };
    Ok(ToolOutput {
        content,
        structured: value
            .get("structuredContent")
            .cloned()
            .map(|v| serde_json::json!({"mcp":v})),
        is_error,
        artifact: None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn errors_and_structured_results_are_not_success() {
        let o=project(json!({"content":[{"type":"text","text":"denied"}],"isError":true,"structuredContent":{"ok":false}})).unwrap();
        assert!(o.is_error);
        assert_eq!(o.content.text(), "denied");
        assert_eq!(o.structured.unwrap()["mcp"]["ok"], false);
    }
    #[test]
    fn unsupported_media_stays_visible_without_base64_leak() {
        let o=project(json!({"content":[{"type":"audio","data":"SECRET_AUDIO"},{"type":"image","mimeType":"image/png","data":"bad"}]})).unwrap();
        assert!(o.content.text().contains("omitted"));
        assert!(!o.content.text().contains("SECRET_AUDIO"));
    }
}
