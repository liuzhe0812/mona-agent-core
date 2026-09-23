//! Common bounded HTTP/SSE transport and replay envelope for the native protocols.
use crate::{
    config::{Protocol, ProviderConfig},
    sse::SseDecoder,
};
use api::*;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::{Client, Url};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, pin::Pin, time::Duration};

pub(crate) fn protocol_error(message: &str) -> AgentError {
    AgentError::new(ErrorCode::ModelProtocol, message)
}
pub(crate) fn unsupported(message: &str) -> AgentError {
    AgentError::new(ErrorCode::Unsupported, message)
}
pub(crate) fn incompatible() -> AgentError {
    AgentError::new(
        ErrorCode::ModelHistoryIncompatible,
        "model-specific history cannot be replayed by this route; start a new session",
    )
}
pub(crate) fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error("missing or invalid protocol string"))
}
pub(crate) fn index(value: &Value, key: &str) -> Result<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n < MAX_TOOL_CALLS_PER_STEP + 128)
        .ok_or_else(|| protocol_error("invalid or excessive protocol index"))
}
pub(crate) fn identifier(value: &Value, key: &str) -> Result<String> {
    let s = string(value, key)?;
    if s.is_empty() || s.len() > 512 || s.chars().any(char::is_control) {
        return Err(protocol_error("invalid protocol identity"));
    }
    Ok(s.into())
}
pub(crate) fn tool_text(result: &ToolResult) -> String {
    json!({"status":result.status,"content":result.content.preview(),"structured":result.structured,
        "truncated":result.truncated,"original_bytes":result.original_bytes,"artifact":result.artifact}).to_string()
}

pub(crate) struct Http {
    pub config: ProviderConfig,
    client: Client,
    endpoint: Url,
}
impl Http {
    pub fn new(config: ProviderConfig, expected: Protocol) -> Result<Self> {
        if config.protocol != expected {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "adapter and configured protocol differ",
            ));
        }
        let endpoint = Url::parse(&config.endpoint)
            .map_err(|_| AgentError::new(ErrorCode::Configuration, "invalid model endpoint"))?;
        let local = endpoint
            .host_str()
            .is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "::1" | "[::1]"));
        if endpoint.host_str().is_none()
            || !(endpoint.scheme() == "https"
                || (config.allow_http_loopback && local && endpoint.scheme() == "http"))
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(AgentError::new(ErrorCode::Configuration, "model endpoint requires HTTPS without credentials/query/fragment; loopback HTTP needs opt-in"));
        }
        if config.model.trim().is_empty()
            || config.model.len() > 512
            || config.model.chars().any(char::is_control)
            || config.request_timeout.is_zero()
            || config.max_sse_event_bytes == 0
            || config.max_wire_bytes == 0
            || config.context_window_tokens == Some(0)
            || config.api_key.as_ref().is_some_and(|key| {
                key.is_empty() || key.len() > 8192 || !key.bytes().all(|c| c.is_ascii_graphic())
            })
        {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "invalid provider configuration",
            ));
        }
        config.capabilities.validate()?;
        ProviderData {
            namespace: config.protocol_namespace.clone(),
            value: Value::Null,
        }
        .validate()?;
        validate_extra(expected, &config.extra_body)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(config.request_timeout)
            .user_agent(concat!("mona-agent-core/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| {
                AgentError::new(
                    ErrorCode::Configuration,
                    "cannot initialize model transport",
                )
            })?;
        Ok(Self {
            config,
            client,
            endpoint,
        })
    }
    pub fn model<'a>(&'a self, options: &'a ModelOptions) -> Result<&'a str> {
        options.validate()?;
        let model = options.model.as_deref().unwrap_or(&self.config.model);
        if model != self.config.model && !self.config.allowed_models.contains(model) {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "model is outside the configured allowlist",
            ));
        }
        Ok(model)
    }
    pub fn window(&self, options: &ModelOptions) -> Option<u64> {
        self.model(options)
            .ok()
            .filter(|m| *m == self.config.model)
            .and(self.config.context_window_tokens)
    }
    pub fn options(&self, options: &ModelOptions) -> Result<Map<String, Value>> {
        let mut fields = self.config.extra_body.clone();
        if let Some(data) = &options.provider_options {
            data.validate()?;
            if data.namespace != self.config.protocol_namespace {
                return Err(unsupported(
                    "provider options belong to a different adapter",
                ));
            }
            let supplied = data
                .value
                .as_object()
                .ok_or_else(|| unsupported("provider options must be an object"))?;
            validate_extra(self.config.protocol, supplied)?;
            fields.extend(supplied.clone());
        }
        Ok(fields)
    }
    pub fn route(&self, options: &ModelOptions) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    self.config.protocol,
                    &self.config.protocol_namespace,
                    self.endpoint.as_str(),
                    self.model(options)?,
                    self.options(options)?
                ))
                .map_err(|_| incompatible())?
            )
        ))
    }
    pub fn replay<'a>(
        &self,
        data: &'a ProviderData,
        options: &ModelOptions,
    ) -> Result<&'a Vec<Value>> {
        data.validate()?;
        let object = data.value.as_object().ok_or_else(incompatible)?;
        if data.namespace != self.config.protocol_namespace
            || object.len() != 2
            || object.get("route").and_then(Value::as_str) != Some(self.route(options)?.as_str())
        {
            return Err(incompatible());
        }
        object
            .get("items")
            .and_then(Value::as_array)
            .filter(|v| v.len() <= MAX_TOOL_CALLS_PER_STEP + 128)
            .ok_or_else(incompatible)
    }
    pub fn image(&self, block: &ContentBlock) -> Result<Value> {
        let ContentBlock::Image { media_type, source } = block else {
            return Err(protocol_error("expected image"));
        };
        Content::Blocks(vec![block.clone()]).validate()?;
        if self.config.capabilities.images == Some(false) {
            return Err(unsupported("selected model does not support images"));
        }
        match source {
            ImageSource::Base64 { data } => Ok(if self.config.protocol == Protocol::Messages {
                json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}})
            } else {
                json!({"type":"input_image","image_url":format!("data:{media_type};base64,{data}"),"detail":"auto"})
            }),
            ImageSource::Url { url } => {
                let parsed = Url::parse(url).map_err(|_| protocol_error("invalid image URL"))?;
                if !self.config.allow_image_urls
                    || parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(unsupported(
                        "remote image URLs require explicit host opt-in and credential-free HTTPS",
                    ));
                }
                Ok(if self.config.protocol == Protocol::Messages {
                    json!({"type":"image","source":{"type":"url","url":url}})
                } else {
                    json!({"type":"input_image","image_url":url,"detail":"auto"})
                })
            }
        }
    }
    pub fn content(&self, content: &Content) -> Result<Vec<Value>> {
        content.validate()?;
        let text_type = if self.config.protocol == Protocol::Messages {
            "text"
        } else {
            "input_text"
        };
        match content {
            Content::Text(text) => Ok(vec![json!({"type":text_type,"text":text})]),
            Content::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => Ok(json!({"type":text_type,"text":text})),
                    ContentBlock::Image { .. } => self.image(b),
                    ContentBlock::Resource { .. } => Err(unsupported(
                        "resource locators are not file contents; install an explicit resolver",
                    )),
                })
                .collect(),
        }
    }
    pub async fn stream<D: Decoder + 'static>(
        &self,
        body: Value,
        decoder: D,
        cancel: CancellationToken,
    ) -> Result<ModelStream> {
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header("Accept", "text/event-stream")
            .json(&body);
        if self.config.protocol == Protocol::Messages {
            request = request.header("anthropic-version", "2023-06-01");
            if let Some(key) = &self.config.api_key {
                request = request.header("x-api-key", key);
            }
        } else if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key);
        }
        let response = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(AgentError::new(ErrorCode::Cancelled,"model request cancelled")),
            response = request.send() => response.map_err(|_| AgentError::new(ErrorCode::ModelTransport,"model HTTP request failed; request details omitted"))?,
        };
        if !response.status().is_success() {
            return Err(crate::chat::classify_http_error(response, cancel).await);
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase()
            .starts_with("text/event-stream")
        {
            return Err(protocol_error("expected text/event-stream response"));
        }
        Ok(Wire {
            body: Box::pin(response.bytes_stream()),
            decoder,
            sse: SseDecoder::new(self.config.max_sse_event_bytes),
            pending: VecDeque::new(),
            error: None,
            cancel,
            received: 0,
            maximum: self.config.max_wire_bytes,
        }
        .into_stream())
    }
}
fn validate_extra(protocol: Protocol, fields: &Map<String, Value>) -> Result<()> {
    let allowed: &[&str] = match protocol {
        Protocol::Responses => &["reasoning"],
        Protocol::Messages => &["thinking", "output_config"],
        _ => &[],
    };
    if fields.keys().any(|k| !allowed.contains(&k.as_str()))
        || serde_json::to_vec(fields).map_or(true, |v| v.len() > 16 * 1024)
    {
        return Err(AgentError::new(ErrorCode::Configuration,"unsupported private option; hosted tools, remote state and core-controlled fields are not allowed"));
    }
    for (key, value) in fields {
        let object = value
            .as_object()
            .ok_or_else(|| unsupported("private generation option must be an object"))?;
        let keys: &[&str] = match key.as_str() {
            "reasoning" => &["effort", "summary"],
            "thinking" => &["type", "budget_tokens", "display"],
            "output_config" => &["effort"],
            _ => &[],
        };
        if object.keys().any(|k| !keys.contains(&k.as_str())) {
            return Err(unsupported(
                "private generation option contains unsupported fields",
            ));
        }
        if key == "thinking" {
            let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
            if !matches!(kind, "enabled" | "disabled" | "adaptive") {
                return Err(unsupported(
                    "thinking type must be enabled, disabled or adaptive",
                ));
            }
            if kind == "enabled"
                && object
                    .get("budget_tokens")
                    .and_then(Value::as_u64)
                    .is_none_or(|n| n < 1024 || n > u32::MAX as u64)
            {
                return Err(unsupported(
                    "enabled thinking needs a positive budget of at least 1024 tokens",
                ));
            }
            if kind != "enabled" && object.contains_key("budget_tokens") {
                return Err(unsupported("only enabled thinking accepts budget_tokens"));
            }
        }
        for (k, v) in object {
            if k != "budget_tokens"
                && v.as_str()
                    .is_none_or(|s| s.is_empty() || s.len() > 64 || s.chars().any(char::is_control))
            {
                return Err(unsupported(
                    "private generation option has an invalid value",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn replay_event(namespace: &str, route: &str, items: &[Value]) -> Result<ModelEvent> {
    let data = ProviderData {
        namespace: namespace.into(),
        value: json!({"route":route,"items":items}),
    };
    data.validate()?;
    Ok(ModelEvent::ProviderData {
        target: ProtocolTarget::Assistant,
        data,
    })
}
pub(crate) trait Decoder: Send {
    fn decode(&mut self, value: Value) -> Result<Vec<ModelEvent>>;
    fn done(&self) -> bool;
    fn output_started(&self) -> bool;
}
type BytesStream = Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>;
struct Wire<D> {
    body: BytesStream,
    decoder: D,
    sse: SseDecoder,
    pending: VecDeque<ModelEvent>,
    error: Option<AgentError>,
    cancel: CancellationToken,
    received: usize,
    maximum: usize,
}
impl<D: Decoder + 'static> Wire<D> {
    fn fail(&mut self, mut error: AgentError) {
        error.model_output_started |= self.decoder.output_started();
        // A bad trailing frame in the SAME chunk cannot hide behind a queued End.
        self.pending.retain(|e| !matches!(e, ModelEvent::End));
        self.error = Some(error);
    }
    fn ingest(&mut self, bytes: &[u8]) -> Result<()> {
        // Feed through newline boundaries so a later malformed SSE frame cannot erase
        // already decoded content/usage from the same network chunk.
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            for raw in self.sse.push(part)? {
                if raw.trim() == "[DONE]" && self.decoder.done() {
                    continue;
                }
                if self.decoder.done() {
                    return Err(protocol_error("data after protocol completion"));
                }
                let value =
                    serde_json::from_str(&raw).map_err(|_| protocol_error("invalid SSE JSON"))?;
                self.pending.extend(self.decoder.decode(value)?);
            }
        }
        Ok(())
    }
    fn into_stream(self) -> ModelStream {
        Box::pin(futures_util::stream::try_unfold(
            self,
            |mut state| async move {
                loop {
                    if let Some(e) = state.pending.pop_front() {
                        return Ok(Some((e, state)));
                    }
                    if let Some(e) = state.error.take() {
                        return Err(e);
                    }
                    if state.decoder.done() {
                        return Ok(None);
                    }
                    let result = tokio::select! { biased;
                        _=state.cancel.cancelled()=>Err(AgentError::new(ErrorCode::Cancelled,"model stream cancelled")),
                        chunk=state.body.next()=>match chunk {
                            Some(Ok(bytes))=>Ok(bytes), Some(Err(_))=>Err(AgentError::new(ErrorCode::ModelTransport,"model stream transport failed")),
                            None=>Err(AgentError::new(ErrorCode::ModelTransport,"model stream ended before protocol completion")),
                        }
                    };
                    match result {
                        Err(e) => state.fail(e),
                        Ok(bytes) => {
                            state.received = state.received.saturating_add(bytes.len());
                            if state.received > state.maximum {
                                state.fail(AgentError::new(
                                    ErrorCode::Limit,
                                    "model stream exceeds wire byte limit",
                                ));
                            } else if let Err(e) = state.ingest(&bytes) {
                                state.fail(e);
                            }
                        }
                    }
                }
            },
        ))
    }
}
