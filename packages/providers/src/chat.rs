use crate::sse::SseDecoder;
use api::*;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::{Client, Url};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::{VecDeque, BTreeSet}, pin::Pin, time::Duration};

#[derive(Clone, Copy)]
pub enum OutputTokenField { MaxTokens, MaxCompletionTokens }

/// Secrets intentionally have no Debug implementation.
pub struct ChatConfig {
    /// Full endpoint, including /chat/completions. Never inferred from a base URL.
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    pub allow_http_loopback: bool,
    pub request_timeout: Duration,
    pub max_sse_event_bytes: usize,
    pub max_wire_bytes: usize,
    /// Optional effective context capacity for this adapter/model route.
    pub context_window_tokens: Option<u64>,
    pub include_usage: bool,
    pub output_token_field: OutputTokenField,
    /// Provider options, e.g. thinking mode. Core-controlled fields cannot be overridden.
    pub extra_body: Map<String, Value>,
    /// Optional per-run model selection cannot escape this configured allowlist.
    pub allowed_models: BTreeSet<String>,
    pub allow_image_urls: bool,
    /// Name this differently for adapters whose replay data is incompatible.
    pub protocol_namespace: String,
    /// Host-approved, complete-snapshot replay fields. Never identity/control fields.
    pub message_replay_fields: BTreeSet<String>,
    pub tool_replay_fields: BTreeSet<String>,
    /// Whitelist for trusted ModelOptions.provider_options. Reserved keys stay blocked.
    pub request_option_fields: BTreeSet<String>,
}
impl ChatConfig {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into(), model: model.into(), api_key: None,
            allow_http_loopback: false, request_timeout: Duration::from_secs(120),
            max_sse_event_bytes: 256 * 1024, max_wire_bytes: 8 * 1024 * 1024,
            context_window_tokens: None,
            include_usage: true, output_token_field: OutputTokenField::MaxTokens, extra_body: Map::new(),
            allowed_models: BTreeSet::new(), allow_image_urls: false,
            protocol_namespace: "chat_completions".into(),
            message_replay_fields: ["reasoning_content".to_owned()].into_iter().collect(),
            tool_replay_fields: BTreeSet::new(), request_option_fields: BTreeSet::new() }
    }
}

pub struct ChatModel { client: Client, endpoint: Url, config: ChatConfig }
impl ChatModel {
    pub fn new(config: ChatConfig) -> Result<Self> {
        let endpoint = Url::parse(&config.endpoint).map_err(|_| AgentError::new(ErrorCode::Configuration, "invalid model endpoint URL"))?;
        let local = endpoint.host_str().is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "::1" | "[::1]"));
        if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && local && config.allow_http_loopback) {
            return Err(AgentError::new(ErrorCode::Configuration, "model endpoint must use HTTPS; HTTP requires explicit loopback opt-in"));
        }
        if !endpoint.username().is_empty() || endpoint.password().is_some() || endpoint.query().is_some() || endpoint.fragment().is_some() {
            return Err(AgentError::new(ErrorCode::Configuration, "endpoint URL must not contain userinfo, query, or fragment"));
        }
        if config.model.is_empty() || config.model.len() > 512 || config.request_timeout.is_zero()
            || config.max_sse_event_bytes == 0 || config.max_wire_bytes == 0
            || config.context_window_tokens == Some(0) {
            return Err(AgentError::new(ErrorCode::Configuration, "invalid model adapter limits or model id"));
        }
        let reserved = RESERVED_REQUEST_FIELDS;
        if config.extra_body.keys().any(|key| reserved.contains(&key.as_str())) {
            return Err(AgentError::new(ErrorCode::Configuration, "extra_body may not override runtime-controlled request fields"));
        }
        ProviderData { namespace: config.protocol_namespace.clone(), value: Value::Null }.validate()?;
        if config.request_option_fields.iter().any(|k| RESERVED_REQUEST_FIELDS.contains(&k.as_str()))
            || config.message_replay_fields.iter().any(|k| RESERVED_REPLAY_FIELDS.contains(&k.as_str()))
            || config.tool_replay_fields.iter().any(|k| RESERVED_REPLAY_FIELDS.contains(&k.as_str())) {
            return Err(AgentError::new(ErrorCode::Configuration, "replay/option whitelist contains a reserved protocol field"));
        }
        let client = Client::builder().redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("mona-agent-core/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10)).timeout(config.request_timeout).build()
            .map_err(|_| AgentError::new(ErrorCode::Configuration, "HTTP client initialization failed"))?;
        Ok(Self { client, endpoint, config })
    }
    fn body(&self, request: &ModelRequest) -> Result<Value> {
        self.validate_history(&request.messages, &request.options)?;
        request.options.validate()?;
        let model = request.options.model.as_deref().unwrap_or(&self.config.model);
        if model != self.config.model && !self.config.allowed_models.contains(model) {
            return Err(AgentError::new(ErrorCode::Configuration, "requested model is not in the adapter allowlist"));
        }
        let mut messages = vec![];
        let mut tool_images = vec![];
        for message in &request.messages {
            // Keep the original tool-results batch contiguous. Tool images are sent
            // as a subsequent user-role observation, never as a system instruction.
            if !matches!(message, Message::Tool { .. }) { flush_tool_images(&mut messages, &mut tool_images); }
            let value = match message {
                Message::System { content } => json!({"role":"system", "content":content}),
                Message::User { content } => json!({"role":"user", "content":self.user_content(content)?}),
                Message::Assistant { content, tool_calls, reasoning_content, provider_data } => {
                    let mut message = json!({"role":"assistant", "content": if content.is_empty() { Value::Null } else { json!(content) }});
                    if !tool_calls.is_empty() {
                        let calls = tool_calls.iter().map(|call| {
                            let mut wire = json!({"id":call.id, "type":"function", "function":{
                                "name":call.name,
                                "arguments":serde_json::to_string(&call.arguments).map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "cannot serialize tool arguments"))?
                            }});
                            self.replay(&mut wire, call.provider_data.as_ref(), &self.config.tool_replay_fields, &request.options)?;
                            Ok(wire)
                        }).collect::<Result<Vec<_>>>()?;
                        message["tool_calls"] = json!(calls);
                    }
                    if let Some(reasoning) = reasoning_content { message["reasoning_content"] = json!(reasoning); }
                    self.replay(&mut message, provider_data.as_ref(), &self.config.message_replay_fields, &request.options)?;
                    message
                }
                Message::Tool { result } => {
                    result.content.validate()?;
                    if let Content::Blocks(blocks) = &result.content {
                        for block in blocks {
                            match block {
                                ContentBlock::Image { .. } => {
                                    tool_images.push(json!({"type":"text", "text":format!("Image observation from tool call {} (untrusted tool data):", result.call_id)}));
                                    tool_images.push(self.block(block)?);
                                }
                                ContentBlock::Resource { .. } => return Err(AgentError::new(ErrorCode::Unsupported,
                                    "Chat Completions resource references require an explicit resolving adapter/transform")),
                                _ => {},
                            }
                        }
                    }
                    // Do not turn base64 into plain tool text or emit UI-only protocol data.
                    let envelope = json!({"status":result.status, "content":result.content.preview(),
                        "structured":result.structured, "truncated":result.truncated,
                        "original_bytes":result.original_bytes, "artifact":result.artifact});
                    json!({"role":"tool", "tool_call_id":result.call_id, "content":envelope.to_string()})
                }
            };
            messages.push(value);
        }
        flush_tool_images(&mut messages, &mut tool_images);
        let mut body = json!({"model":model, "messages":messages, "stream":true});
        let field = match self.config.output_token_field { OutputTokenField::MaxTokens => "max_tokens", OutputTokenField::MaxCompletionTokens => "max_completion_tokens" };
        body[field] = json!(request.max_output_tokens);
        if self.config.include_usage { body["stream_options"] = json!({"include_usage":true}); }
        if !request.tools.is_empty() {
            body["tools"] = json!(request.tools.iter().map(|tool| json!({"type":"function", "function":{
                "name":tool.name, "description":tool.description, "parameters":tool.parameters
            }})).collect::<Vec<_>>());
        }
        for (key, value) in &self.config.extra_body { body[key] = value.clone(); }
        if let Some(t) = request.options.temperature { body["temperature"] = json!(t); }
        if let Some(p) = request.options.top_p { body["top_p"] = json!(p); }
        if let Some(stop) = &request.options.stop { body["stop"] = json!(stop); }
        if let Some(data) = &request.options.provider_options {
            if data.namespace != self.config.protocol_namespace {
                return Err(AgentError::new(ErrorCode::Unsupported, "foreign provider options require a different adapter"));
            }
            let fields = data.value.as_object().ok_or_else(|| AgentError::new(ErrorCode::Configuration, "provider options must be an object"))?;
            for (key, value) in fields {
                if RESERVED_REQUEST_FIELDS.contains(&key.as_str()) || !self.config.request_option_fields.contains(key) {
                    return Err(AgentError::new(ErrorCode::Configuration, "provider option is reserved or not whitelisted"));
                }
                body[key] = value.clone();
            }
        }
        Ok(body)
    }
    fn user_content(&self, content: &Content) -> Result<Value> {
        content.validate()?;
        match content {
            Content::Text(text) => Ok(json!(text)),
            Content::Blocks(blocks) => Ok(Value::Array(blocks.iter().map(|b| self.block(b)).collect::<Result<Vec<_>>>()?)),
        }
    }
    fn block(&self, block: &ContentBlock) -> Result<Value> {
        match block {
            ContentBlock::Text { text } => Ok(json!({"type":"text", "text":text})),
            ContentBlock::Image { media_type, source } => {
                let url = match source {
                    ImageSource::Base64 { data } => format!("data:{media_type};base64,{data}"),
                    ImageSource::Url { url } => {
                        if !self.config.allow_image_urls { return Err(AgentError::new(ErrorCode::Configuration, "remote image URLs require host opt-in")); }
                        let parsed = Url::parse(url).map_err(|_| AgentError::new(ErrorCode::Schema, "invalid image URL"))?;
                        if parsed.scheme() != "https" || parsed.host_str().is_none() || !parsed.username().is_empty()
                            || parsed.password().is_some() || parsed.fragment().is_some() {
                            return Err(AgentError::new(ErrorCode::Schema, "image URL must be HTTPS without credentials or fragment"));
                        }
                        url.clone()
                    }
                };
                Ok(json!({"type":"image_url", "image_url":{"url":url}}))
            }
            ContentBlock::Resource { .. } => Err(AgentError::new(ErrorCode::Unsupported,
                "resource references are not file bytes; provide a resolving adapter/transform")),
        }
    }
    fn replay_route(&self, options: &ModelOptions) -> Result<String> {
        let model = options.model.as_deref().unwrap_or(&self.config.model);
        if model != self.config.model && !self.config.allowed_models.contains(model) {
            return Err(AgentError::new(ErrorCode::Configuration, "requested model is not in the adapter allowlist"));
        }
        // Stable across restart and credential rotation, but not a different model/endpoint/profile.
        // Neither credentials nor the transient ManagedRuntime selector are serialized here.
        let route = serde_json::to_vec(&(&self.config.protocol_namespace, self.endpoint.as_str(), model, &self.config.extra_body, &options.provider_options))
            .map_err(|_| incompatible_history())?;
        Ok(format!("{:x}", Sha256::digest(route)))
    }
    fn replay(&self, target: &mut Value, data: Option<&ProviderData>, fields: &BTreeSet<String>, options: &ModelOptions) -> Result<()> {
        let Some(data) = data else { return Ok(()); };
        data.validate()?;
        let envelope = data.value.as_object().ok_or_else(incompatible_history)?;
        if data.namespace != self.config.protocol_namespace || envelope.len() != 2
            || envelope.get("route").and_then(Value::as_str) != Some(self.replay_route(options)?.as_str()) {
            return Err(incompatible_history());
        }
        let object = envelope.get("fields").and_then(Value::as_object).ok_or_else(incompatible_history)?;
        for (key, value) in object {
            if key == "reasoning_content" || RESERVED_REPLAY_FIELDS.contains(&key.as_str()) || !fields.contains(key) {
                return Err(incompatible_history());
            }
            if target.get(key).is_some_and(|old| old != value) {
                return Err(AgentError::new(ErrorCode::ModelProtocol, "conflicting legacy and opaque replay fields"));
            }
            target[key] = value.clone();
        }
        Ok(())
    }

}

fn incompatible_history() -> AgentError {
    AgentError::new(ErrorCode::ModelHistoryIncompatible,
        "history contains model-specific replay data incompatible with the selected model or endpoint; start a new session")
}

fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(value) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        return Some(value);
    }
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|seconds| seconds.checked_mul(1000))
}

pub(crate) async fn classify_http_error(
    mut response: reqwest::Response,
    cancel: CancellationToken,
) -> AgentError {
    const MAX_ERROR_BODY: usize = 64 * 1024;
    let status = response.status().as_u16();
    let retry_after = retry_after_ms(response.headers());
    let mut bytes = Vec::new();
    while bytes.len() < MAX_ERROR_BODY {
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => return AgentError::new(ErrorCode::Cancelled, "model request cancelled"),
            chunk = response.chunk() => chunk,
        };
        let Ok(Some(chunk)) = chunk else { break };
        let remaining = MAX_ERROR_BODY - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    let provider_class = serde_json::from_slice::<Value>(&bytes).ok()
        .and_then(|body| body.get("error").and_then(|error| {
            // A conflict is retryable only when the provider explicitly says so.
            let allow_message = status != 409;
            classify_provider_error_class(error, allow_message)
        }))
        .filter(|(code, _)| status != 409 || matches!(code, ErrorCode::ModelRateLimit | ErrorCode::ModelServer));
    let (code, message) = if status == 401 || status == 403 {
        (ErrorCode::ModelAuthentication, "model authentication was rejected")
    } else if status == 402 {
        (ErrorCode::ModelQuota, "model quota or billing limit reached")
    } else if let Some(class) = provider_class {
        class
    } else if status == 409 {
        (ErrorCode::ModelRequest, "model request was rejected")
    } else if status == 408 || status == 429 {
        (ErrorCode::ModelRateLimit, "model request was rate limited")
    } else if status >= 500 {
        (ErrorCode::ModelServer, "model provider is temporarily unavailable")
    } else {
        (ErrorCode::ModelRequest, "model request was rejected")
    };
    AgentError::new(code, message).with_http(status, retry_after)
}

type ProviderErrorClass = (ErrorCode, &'static str);

fn classify_error_identifier(identifier: &str) -> Option<ProviderErrorClass> {
    let identifier = identifier.trim().to_ascii_lowercase();
    Some(match identifier.as_str() {
        "context_length_exceeded" | "context_window_exceeded" | "maximum_context_length_exceeded"
        | "max_context_length_exceeded" => (ErrorCode::ModelContextWindow, "model context window exceeded"),
        "authentication_error" | "authentication_failed" | "unauthorized" | "unauthorized_error"
        | "invalid_api_key" | "invalid_api_key_error" | "permission_denied" | "permission_denied_error"
            => (ErrorCode::ModelAuthentication, "model authentication was rejected"),
        "insufficient_quota" | "quota_exceeded" | "quota_exceeded_error" | "billing_error"
        | "billing_hard_limit_reached" | "billing_limit_reached" | "out_of_budget"
            => (ErrorCode::ModelQuota, "model quota or billing limit reached"),
        "rate_limit" | "rate_limit_exceeded" | "rate_limit_error" | "too_many_requests"
            => (ErrorCode::ModelRateLimit, "model request was rate limited"),
        "server_error" | "service_unavailable" | "service_unavailable_error" | "overloaded"
        | "overloaded_error" | "temporary_failure" | "temporarily_unavailable"
        | "internal_server_error" | "internal_error"
            => (ErrorCode::ModelServer, "model provider is temporarily unavailable"),
        _ => return None,
    })
}

fn classify_error_message(message: &str) -> Option<ProviderErrorClass> {
    let message = message.trim().trim_end_matches(|character| matches!(character, '.' | '!')).to_ascii_lowercase();
    if let Some(class) = classify_error_identifier(&message) {
        return Some(class);
    }
    match message.as_str() {
        "context length exceeded" | "context window exceeded" | "maximum context length exceeded"
            => Some((ErrorCode::ModelContextWindow, "model context window exceeded")),
        "authentication failed" | "unauthorized" | "invalid api key" | "permission denied"
            => Some((ErrorCode::ModelAuthentication, "model authentication was rejected")),
        "insufficient quota" | "quota exceeded" | "out of budget" | "billing limit reached"
            => Some((ErrorCode::ModelQuota, "model quota or billing limit reached")),
        "rate limit exceeded" | "too many requests" | "temporarily rate limited"
            => Some((ErrorCode::ModelRateLimit, "model request was rate limited")),
        "server error" | "service unavailable" | "overloaded" | "temporarily unavailable"
            => Some((ErrorCode::ModelServer, "model provider is temporarily unavailable")),
        _ => None,
    }
}

// Anthropic reports input overflow as invalid_request_error, not a dedicated code.
// Accept only its bounded, complete grammar; arbitrary quoted prompt text is not a signal.
fn prompt_too_long(message: &str) -> bool {
    let message = message.trim().trim_end_matches('.');
    if message == "prompt is too long" { return true; }
    if message.len() > 160 { return false; }
    let Some(counts) = message.strip_prefix("prompt is too long: ") else { return false; };
    let Some((input, maximum)) = counts.split_once(" tokens > ") else { return false; };
    let Some(maximum) = maximum.strip_suffix(" maximum") else { return false; };
    if input.is_empty() || maximum.is_empty() || !input.bytes().all(|c| c.is_ascii_digit()) || !maximum.bytes().all(|c| c.is_ascii_digit()) { return false; }
    matches!((input.parse::<u64>(), maximum.parse::<u64>()), (Ok(input), Ok(maximum)) if maximum > 0 && input > maximum)
}

fn classify_provider_error_class(error: &Value, allow_message: bool) -> Option<ProviderErrorClass> {
    let object = error.as_object();
    for field in ["code", "type"] {
        if let Some(identifier) = object.and_then(|object| object.get(field)).and_then(Value::as_str) {
            if let Some(class) = classify_error_identifier(identifier) {
                return Some(class);
            }
        }
    }
    if allow_message {
        if error.get("type").and_then(Value::as_str) == Some("invalid_request_error")
            && error.get("message").and_then(Value::as_str).is_some_and(prompt_too_long) {
            return Some((ErrorCode::ModelContextWindow, "model context window exceeded"));
        }
        object.and_then(|object| object.get("message")).and_then(Value::as_str)
            .and_then(classify_error_message)
            .or_else(|| error.as_str().and_then(classify_error_message))
    } else {
        None
    }
}

pub(crate) fn classify_provider_error(error: &Value) -> AgentError {
    let (code, message) = classify_provider_error_class(error, true)
        .unwrap_or((ErrorCode::ModelRequest, "provider emitted an error object"));
    AgentError::new(code, message)
}

#[async_trait]
impl Model for ChatModel {
    fn validate_history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        options.validate()?;
        self.replay_route(options)?;
        for message in messages {
            if let Message::Assistant { reasoning_content, provider_data, tool_calls, .. } = message {
                let mut target = json!({});
                self.replay(&mut target, provider_data.as_ref(), &self.config.message_replay_fields, options)?;
                // Reasoning has a single content carrier; the envelope binds its origin,
                // without duplicating every reasoning token in opaque fields.
                if reasoning_content.is_some() && (provider_data.is_none()
                    || !self.config.message_replay_fields.contains("reasoning_content")) {
                    return Err(incompatible_history());
                }
                for call in tool_calls {
                    self.replay(&mut json!({}), call.provider_data.as_ref(), &self.config.tool_replay_fields, options)?;
                }
            }
        }
        Ok(())
    }
    fn context_window_tokens(&self, options: &ModelOptions) -> Option<u64> {
        match options.model.as_deref() {
            None => self.config.context_window_tokens,
            Some(model) if model == self.config.model => self.config.context_window_tokens,
            Some(_) => None,
        }
    }

    async fn stream(&self, request: ModelRequest, cancel: CancellationToken) -> Result<ModelStream> {
        let route = self.replay_route(&request.options)?;
        let body = self.body(&request)?;
        let mut request = self.client.post(self.endpoint.clone()).json(&body).header("Accept", "text/event-stream");
        if let Some(key) = &self.config.api_key { request = request.bearer_auth(key); }
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AgentError::new(ErrorCode::Cancelled, "model request cancelled")),
            response = request.send() => response.map_err(|_| AgentError::new(ErrorCode::ModelTransport, "model HTTP request failed (credentials/body omitted)"))?,
        };
        if !response.status().is_success() {
            return Err(classify_http_error(response, cancel.clone()).await);
        }
        let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or("");
        if !content_type.to_ascii_lowercase().starts_with("text/event-stream") {
            return Err(AgentError::new(ErrorCode::ModelProtocol, "expected text/event-stream response"));
        }
        let state = WireState { body: Box::pin(response.bytes_stream()), decoder: SseDecoder::new(self.config.max_sse_event_bytes),
            pending: VecDeque::new(), failure: None, output_started: false,
            done: false, cancel, received: 0, max_wire: self.config.max_wire_bytes,
            replay: ReplayCapture { namespace: self.config.protocol_namespace.clone(), route, reasoning_seen: false,
                message_fields: self.config.message_replay_fields.clone(), tool_fields: self.config.tool_replay_fields.clone(),
                message: Map::new(), tools: std::collections::BTreeMap::new() } };
        Ok(state.into_stream())
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>;
struct WireState {
    body: ByteStream, decoder: SseDecoder, pending: VecDeque<ModelEvent>, done: bool,
    failure: Option<AgentError>, output_started: bool,
    cancel: CancellationToken, received: usize, max_wire: usize, replay: ReplayCapture,
}
impl WireState {
    fn mark_error(&self, mut error: AgentError) -> AgentError {
        error.model_output_started |= self.output_started;
        error
    }
    fn ingest(&mut self, chunk: &[u8]) -> Result<()> {
        for data in self.decoder.push(chunk)? {
            if self.done { return Err(AgentError::new(ErrorCode::ModelProtocol, "SSE data after [DONE]")); }
            if data.trim() == "[DONE]" {
                self.done = true;
                self.pending.push_back(ModelEvent::End);
            } else {
                let mut events = decode_chunk(&data)?;
                let replay = self.replay.capture(&data)?;
                let position = events.iter().position(|e| matches!(e, ModelEvent::Finish(_))).unwrap_or(events.len());
                events.splice(position..position, replay);
                self.output_started |= events.iter().any(|event| match event {
                    ModelEvent::Text(text) | ModelEvent::Reasoning(text) => !text.is_empty(),
                    ModelEvent::ToolDelta { .. } | ModelEvent::ProviderData { .. } => true,
                    _ => false,
                });
                self.pending.extend(events);
            }
        }
        Ok(())
    }
    fn into_stream(self) -> ModelStream {
        Box::pin(futures_util::stream::try_unfold(self, |mut state| async move {
            loop {
                // A network chunk can contain both content/usage and a later error.
                // Deliver earlier facts first, then fail once without reading more bytes.
                if let Some(event) = state.pending.pop_front() { return Ok(Some((event, state))); }
                if let Some(error) = state.failure.take() { return Err(error); }
                if state.done { return Ok(None); }
                let item = tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => return Err(state.mark_error(AgentError::new(ErrorCode::Cancelled, "model stream cancelled"))),
                    item = state.body.next() => item,
                };
                let chunk = match item {
                    Some(Ok(bytes)) => bytes,
                    Some(Err(_)) => return Err(state.mark_error(AgentError::new(ErrorCode::ModelTransport, "model stream transport error"))),
                    None => return Err(state.mark_error(AgentError::new(ErrorCode::ModelTransport, "model connection closed without [DONE]"))),
                };
                state.received = state.received.saturating_add(chunk.len());
                if state.received > state.max_wire {
                    return Err(state.mark_error(AgentError::new(ErrorCode::Limit, "SSE response exceeds wire byte limit")));
                }
                if let Err(error) = state.ingest(&chunk) {
                    state.failure = Some(state.mark_error(error));
                }
            }
        }))
    }
}
const RESERVED_REQUEST_FIELDS: &[&str] = &["model", "messages", "tools", "tool_choice", "stream", "stream_options",
    "max_tokens", "max_completion_tokens", "n", "temperature", "top_p", "stop",
    "api_key", "authorization", "endpoint", "base_url", "headers"];
const RESERVED_REPLAY_FIELDS: &[&str] = &["role", "content", "tool_calls", "tool_call_id", "id", "type", "function", "name", "arguments"];
fn flush_tool_images(messages: &mut Vec<Value>, images: &mut Vec<Value>) {
    if !images.is_empty() { messages.push(json!({"role":"user", "content":std::mem::take(images)})); }
}
struct ReplayCapture {
    route: String,
    reasoning_seen: bool,
    namespace: String, message_fields: BTreeSet<String>, tool_fields: BTreeSet<String>,
    message: Map<String, Value>, tools: std::collections::BTreeMap<usize, Map<String, Value>>,
}
impl ReplayCapture {
    /// Configured extra fields must be complete values, NOT string/JSON deltas.
    fn capture(&mut self, raw: &str) -> Result<Vec<ModelEvent>> {
        let frame: Value = serde_json::from_str(raw).map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "invalid replay frame"))?;
        let mut events = vec![];
        if let Some(choices) = frame.get("choices").and_then(Value::as_array) {
            for choice in choices {
                let delta = &choice["delta"];
                let mut changed = false;
                for key in &self.message_fields {
                    // reasoning_content is a stream of text fragments, already handled separately.
                    if key == "reasoning_content" { continue; }
                    if let Some(value) = delta.get(key) { self.message.insert(key.clone(), value.clone()); changed = true; }
                }
                if let Some(value) = delta.get("reasoning_content").filter(|v| !v.is_null()) {
                    let text = value.as_str().ok_or_else(incompatible_history)?;
                    if !self.message_fields.contains("reasoning_content") { return Err(incompatible_history()); }
                    self.reasoning_seen |= !text.is_empty();
                }
                if choice.get("finish_reason").is_some_and(|v| !v.is_null()) && self.reasoning_seen {
                    changed = true;
                }
                if changed { events.push(ModelEvent::ProviderData { target: ProtocolTarget::Assistant,
                    data: ProviderData { namespace: self.namespace.clone(), value: json!({"route":self.route,"fields":self.message}) } }); }
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let index = call["index"].as_u64().and_then(|i| usize::try_from(i).ok())
                            .ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "missing replay tool index"))?;
                        // Bound the adapter as well as the Core collector.
                        if index >= MAX_TOOL_CALLS_PER_STEP { return Err(AgentError::new(ErrorCode::Limit, "replay tool index too large")); }
                        let mut changed = false;
                        let data = self.tools.entry(index).or_default();
                        for key in &self.tool_fields {
                            if let Some(value) = call.get(key) { data.insert(key.clone(), value.clone()); changed = true; }
                        }
                        if changed { events.push(ModelEvent::ProviderData { target: ProtocolTarget::ToolCall { index },
                            data: ProviderData { namespace: self.namespace.clone(), value: json!({"route":self.route,"fields":data}) } }); }
                    }
                }
            }
        }
        for event in &events { if let ModelEvent::ProviderData { data, .. } = event { data.validate()?; } }
        Ok(events)
    }
}
fn decode_chunk(data: &str) -> Result<Vec<ModelEvent>> {
    let value: Value = serde_json::from_str(data).map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "invalid SSE JSON"))?;
    if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
        return Err(classify_provider_error(error));
    }
    let mut events = vec![];
    if let Some(usage) = value.get("usage") {
        if let (Some(input), Some(output)) = (usage["prompt_tokens"].as_u64(), usage["completion_tokens"].as_u64()) {
            events.push(ModelEvent::Usage(Usage { input_tokens: input, output_tokens: output }));
        }
    }
    let choices = value.get("choices").and_then(Value::as_array)
        .ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "SSE chunk lacks choices array"))?;
    if choices.len() > 1 { return Err(AgentError::new(ErrorCode::ModelProtocol, "multiple completion choices are unsupported")); }
    for choice in choices {
        if choice.get("index").and_then(Value::as_u64).unwrap_or(0) != 0 {
            return Err(AgentError::new(ErrorCode::ModelProtocol, "unexpected completion index"));
        }
        let delta = &choice["delta"];
        if let Some(content) = delta.get("content").filter(|v| !v.is_null()) {
            events.push(ModelEvent::Text(content.as_str().ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "non-text content is unsupported"))?.to_owned()));
        }
        if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
            events.push(ModelEvent::Reasoning(reasoning.to_owned()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                if call.get("type").and_then(Value::as_str).is_some_and(|kind| kind != "function") {
                    return Err(AgentError::new(ErrorCode::ModelProtocol, "unsupported tool-call type"));
                }
                let index = call["index"].as_u64().and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "missing tool-call index"))?;
                let function = &call["function"];
                events.push(ModelEvent::ToolDelta { index,
                    // Some compatible providers repeat empty identity placeholders in
                    // argument-only chunks. They do not replace the established identity.
                    id: call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned),
                    name: function.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_owned),
                    arguments: function.get("arguments").and_then(Value::as_str).unwrap_or("").to_owned(),
                });
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            let reason = match reason {
                "stop" => FinishReason::Stop, "tool_calls" => FinishReason::ToolCalls,
                "length" => FinishReason::Length, "content_filter" => FinishReason::Filtered,
                _ => return Err(AgentError::new(ErrorCode::ModelProtocol, "unsupported finish reason")),
            };
            events.push(ModelEvent::Finish(reason));
        }
    }
    Ok(events)
}

#[cfg(test)]
#[path = "chat_history_tests.rs"]
mod history_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    pub(super) fn scoped(model: &ChatModel, fields: Value) -> ProviderData {
        ProviderData { namespace: model.config.protocol_namespace.clone(),
            value: json!({"route":model.replay_route(&ModelOptions::default()).unwrap(),"fields":fields}) }
    }
    fn coalesced_stream(raw: String) -> ModelStream {
        WireState {
            body: Box::pin(futures_util::stream::iter(vec![Ok(Bytes::from(raw))])),
            decoder: SseDecoder::new(64 * 1024), pending: VecDeque::new(),
            failure: None, output_started: false, done: false,
            cancel: CancellationToken::new(), received: 0, max_wire: 64 * 1024,
            replay: ReplayCapture { namespace: "test".into(), route: "test-route".into(), reasoning_seen: false,
                message_fields: ["signed_blocks".to_owned(), "reasoning_content".to_owned()].into_iter().collect(),
                tool_fields: BTreeSet::new(), message: Map::new(), tools: Default::default() },
        }.into_stream()
    }
    #[tokio::test]
    async fn coalesced_content_and_failure_preserve_order_usage_and_retry_fence() {
        for delta in [
            json!({"content":"partial"}),
            json!({"reasoning_content":"hidden"}),
            json!({"tool_calls":[{"index":0,"id":"c","function":{"name":"work","arguments":"{"}}]}),
            json!({"signed_blocks":["opaque"]}),
        ] {
            let first = json!({"choices":[{"delta":delta}]}).to_string();
            let raw = format!("data: {first}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":8,\"completion_tokens\":3}}}}\n\ndata: {{\"error\":{{\"code\":\"server_error\"}}}}\n\n");
            let events = coalesced_stream(raw).collect::<Vec<_>>().await;
            assert!(events[0].is_ok(), "content must not disappear behind the later error");
            assert!(events.iter().any(|event| matches!(event, Ok(ModelEvent::Usage(Usage { input_tokens: 8, output_tokens: 3 })))));
            let error = events.last().unwrap().as_ref().unwrap_err();
            assert_eq!(error.code, ErrorCode::ModelServer);
            assert!(error.model_output_started);
            assert_eq!(events.iter().filter(|event| event.is_err()).count(), 1);
        }
    }
    #[tokio::test]
    async fn coalesced_error_before_content_does_not_claim_partial_output() {
        let events = coalesced_stream("data: {\"error\":{\"code\":\"server_error\"}}\n\n".into())
            .collect::<Vec<_>>().await;
        assert_eq!(events.len(), 1);
        assert!(!events[0].as_ref().unwrap_err().model_output_started);
    }

    #[test]
    fn usage_only_frame_is_supported() {
        let events = decode_chunk(r#"{"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":3}}"#).unwrap();
        assert!(matches!(events.as_slice(), [ModelEvent::Usage(Usage { input_tokens: 8, output_tokens: 3 })]));
    }
    #[test]
    fn provider_error_classification_uses_identifiers_and_strict_messages() {
        assert_eq!(classify_provider_error(&json!({"message":"prompt mentions a context window"})).code, ErrorCode::ModelRequest);
        assert_eq!(classify_provider_error(&json!({"type":"rate_limit_error", "message":"context window"})).code, ErrorCode::ModelRateLimit);
        assert_eq!(classify_provider_error(&json!({"message":"context window exceeded"})).code, ErrorCode::ModelContextWindow);
    }
    #[test]
    fn argument_chunks_treat_empty_identity_as_absent() {
        let events = decode_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"","type":"function","function":{"name":"","arguments":"17"}}]}}]}"#).unwrap();
        assert!(matches!(events.as_slice(), [ModelEvent::ToolDelta { index: 0, id: None, name: None, arguments }] if arguments == "17"));
        let events = decode_chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"different-id","type":"function","function":{"name":"add","arguments":""}}]}}]}"#).unwrap();
        assert!(matches!(events.as_slice(), [ModelEvent::ToolDelta { id: Some(id), name: Some(name), .. }] if id == "different-id" && name == "add"));
    }
    #[test]
    fn length_is_not_success() {
        let events = decode_chunk(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#).unwrap();
        assert!(matches!(events.last(), Some(ModelEvent::Finish(FinishReason::Length))));
    }
    #[test]
    fn config_cannot_override_messages() {
        let mut config = ChatConfig::new("https://example.invalid/chat/completions", "test");
        config.extra_body.insert("messages".into(), json!([]));
        assert!(ChatModel::new(config).is_err());
    }
    #[test]
    fn nonlocal_cleartext_is_rejected() {
        let mut config = ChatConfig::new("http://example.invalid/chat/completions", "test");
        config.allow_http_loopback = true;
        assert!(ChatModel::new(config).is_err());
    }
    #[test]
    fn reasoning_protocol_data_is_preserved_in_history() {
        let model = ChatModel::new(ChatConfig::new("https://example.invalid/chat/completions", "test")).unwrap();
        let request = ModelRequest { messages: vec![Message::Assistant { content: "".into(),
            tool_calls: vec![ToolCall::new("one", "test", json!({}))],
            reasoning_content: Some("provider-returned-data".into()), provider_data: Some(scoped(&model, json!({}))) }], tools: vec![], max_output_tokens: 32, options: ModelOptions::default() };
        let body = model.body(&request).unwrap();
        assert_eq!(body["messages"][0]["reasoning_content"], "provider-returned-data");
        assert_eq!(body["messages"][0]["tool_calls"][0]["function"]["arguments"], "{}");
    }

    async fn local_server(body: &'static str, status: &str) -> (String, tokio::task::JoinHandle<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let status = status.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = vec![];
            let mut buffer = [0u8; 4096];
            let (header_end, length) = loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0); received.extend_from_slice(&buffer[..count]);
                if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&received[..end]);
                    assert!(headers.lines().any(|line| line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("user-agent")
                            && value.trim() == concat!("mona-agent-core/", env!("CARGO_PKG_VERSION"))
                    })), "model requests must identify the actual client");
                    let length = headers.lines().find_map(|line| line.split_once(':').and_then(|(k, v)| {
                        if k.eq_ignore_ascii_case("content-length") { v.trim().parse::<usize>().ok() } else { None }
                    })).unwrap();
                    break (end + 4, length);
                }
            };
            while received.len() < header_end + length {
                let count = socket.read(&mut buffer).await.unwrap(); assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
            }
            let request: Value = serde_json::from_slice(&received[header_end..header_end+length]).unwrap();
            let headers = format!("HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            socket.write_all(headers.as_bytes()).await.unwrap();
            // Intentionally split across many byte boundaries, not JSON boundaries.
            for part in body.as_bytes().chunks(7) { socket.write_all(part).await.unwrap(); }
            request
        });
        (endpoint, task)
    }
    fn request() -> ModelRequest {
        ModelRequest { messages: vec![Message::user("hello")], tools: vec![], max_output_tokens: 64, options: ModelOptions::default() }
    }
    #[tokio::test]
    async fn local_http_stream_and_request_envelope() {
        let (endpoint, server) = local_server("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你好\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n", "200 OK").await;
        let mut config = ChatConfig::new(endpoint, "test-model"); config.allow_http_loopback = true;
        let model = ChatModel::new(config).unwrap();
        let events = model.stream(request(), CancellationToken::new()).await.unwrap().collect::<Vec<_>>().await;
        assert!(events.iter().all(|event| event.is_ok()));
        assert!(matches!(events.last().unwrap().as_ref().unwrap(), ModelEvent::End));
        let sent = server.await.unwrap();
        assert_eq!(sent["messages"][0]["content"], "hello");
        assert_eq!(sent["max_tokens"], 64);
        assert_eq!(sent["stream"], true);
    }
    #[tokio::test]
    async fn local_http_missing_done_is_an_error() {
        let (endpoint, server) = local_server("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"stop\"}]}\n\n", "200 OK").await;
        let mut config = ChatConfig::new(endpoint, "test-model"); config.allow_http_loopback = true;
        let model = ChatModel::new(config).unwrap();
        let events = model.stream(request(), CancellationToken::new()).await.unwrap().collect::<Vec<_>>().await;
        assert!(events.last().unwrap().is_err());
        server.await.unwrap();
    }
    #[tokio::test]
    async fn local_http_error_is_not_retried() {
        let (endpoint, server) = local_server("", "429 Too Many Requests").await;
        let mut config = ChatConfig::new(endpoint, "test-model"); config.allow_http_loopback = true;
        let model = ChatModel::new(config).unwrap();
        let error = match model.stream(request(), CancellationToken::new()).await {
            Err(error) => error,
            Ok(_) => panic!("429 unexpectedly opened a model stream"),
        };
        assert_eq!(error.code, ErrorCode::ModelRateLimit);
        assert_eq!(error.http_status, Some(429));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn http_status_precedence_and_conflict_default_are_stable() {
        let cases = [
            (r#"{"error":{"message":"context window"}}"#, "401 Unauthorized", ErrorCode::ModelAuthentication),
            (r#"{"error":{"message":"context window"}}"#, "402 Payment Required", ErrorCode::ModelQuota),
            (r#"{"error":{"message":"rate limit exceeded"}}"#, "409 Conflict", ErrorCode::ModelRequest),
            (r#"{"error":{"code":"overloaded_error","message":"echoed prompt"}}"#, "409 Conflict", ErrorCode::ModelServer),
        ];
        for (body, status, expected) in cases {
            let (endpoint, server) = local_server(body, status).await;
            let mut config = ChatConfig::new(endpoint, "test-model"); config.allow_http_loopback = true;
            let model = ChatModel::new(config).unwrap();
            let error = match model.stream(request(), CancellationToken::new()).await {
                Err(error) => error,
                Ok(_) => panic!("HTTP error unexpectedly opened a model stream"),
            };
            assert_eq!(error.code, expected);
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn context_overflow_is_machine_readable_without_exposing_the_body() {
        let body = r#"{"error":{"code":"context_length_exceeded","message":"SECRET prompt is too long"}}"#;
        let (endpoint, server) = local_server(body, "400 Bad Request").await;
        let mut config = ChatConfig::new(endpoint, "test-model"); config.allow_http_loopback = true;
        let model = ChatModel::new(config).unwrap();
        let error = match model.stream(request(), CancellationToken::new()).await {
            Err(error) => error,
            Ok(_) => panic!("context overflow unexpectedly opened a model stream"),
        };
        assert_eq!(error.code, ErrorCode::ModelContextWindow);
        assert_eq!(error.http_status, Some(400));
        assert!(!error.message.contains("SECRET"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn deterministic_http_request_error_is_not_retryable_transport() {
        let body = r#"{"error":{"code":"model_not_found","message":"unknown model"}}"#;
        let (endpoint, server) = local_server(body, "404 Not Found").await;
        let mut config = ChatConfig::new(endpoint, "test-model"); config.allow_http_loopback = true;
        let model = ChatModel::new(config).unwrap();
        let error = match model.stream(request(), CancellationToken::new()).await {
            Err(error) => error,
            Ok(_) => panic!("404 unexpectedly opened a model stream"),
        };
        assert_eq!(error.code, ErrorCode::ModelRequest);
        assert_eq!(error.http_status, Some(404));
        server.await.unwrap();
    }
}

#[cfg(test)]
mod generic_tests {
    use super::*;
    use super::tests::scoped;
    fn config() -> ChatConfig { ChatConfig::new("https://example.invalid/chat/completions", "base") }
    fn request(messages: Vec<Message>) -> ModelRequest {
        ModelRequest { messages, tools: vec![], max_output_tokens: 128, options: ModelOptions::default() }
    }
    fn image() -> ContentBlock {
        ContentBlock::Image { media_type: "image/png".into(), source: ImageSource::Base64 { data: "AQ==".into() } }
    }
    fn assistant(calls: Vec<ToolCall>, provider_data: Option<ProviderData>) -> Message {
        Message::Assistant { content: String::new(), tool_calls: calls, reasoning_content: None, provider_data }
    }

    #[test]
    fn image_blocks_are_not_encoded_as_plain_text() {
        let model = ChatModel::new(config()).unwrap();
        let body = model.body(&request(vec![Message::user(Content::Blocks(vec![
            ContentBlock::Text { text: "describe".into() }, image()]))])).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
        assert_eq!(body["messages"][0]["content"][1]["image_url"]["url"], "data:image/png;base64,AQ==");
    }
    #[test]
    fn image_url_needs_opt_in_and_rejects_embedded_credentials() {
        let content = |url: &str| Content::Blocks(vec![ContentBlock::Image { media_type: "image/png".into(),
            source: ImageSource::Url { url: url.into() } }]);
        let model = ChatModel::new(config()).unwrap();
        assert!(model.body(&request(vec![Message::user(content("https://example.invalid/image"))])).is_err());
        let mut c = config(); c.allow_image_urls = true;
        let model = ChatModel::new(c).unwrap();
        assert!(model.body(&request(vec![Message::user(content("https://example.invalid/image"))])).is_ok());
        assert!(model.body(&request(vec![Message::user(content("https://user:password@example.invalid/image"))])).is_err());
    }
    #[test]
    fn unresolved_resource_references_fail_explicitly() {
        let content = Content::Blocks(vec![ContentBlock::Resource {
            reference: ArtifactRef { uri: "asset://document".into(), bytes: 10 }, media_type: "application/pdf".into(), name: None }]);
        let model = ChatModel::new(config()).unwrap();
        assert_eq!(model.body(&request(vec![Message::user(content)])).unwrap_err().code, ErrorCode::Unsupported);
    }
    #[test]
    fn tool_image_projection_keeps_tool_results_contiguous() {
        let messages = vec![assistant(vec![ToolCall::new("a", "camera", json!({})), ToolCall::new("b", "read", json!({}))], None),
            Message::Tool { result: ToolResult::from_output("a", ToolOutput::new(Content::Blocks(vec![image()]))) },
            Message::Tool { result: ToolResult::new("b", ToolStatus::Success, "text") }];
        let canonical = messages.clone();
        let model = ChatModel::new(config()).unwrap();
        let body = model.body(&request(messages)).unwrap();
        let sent = body["messages"].as_array().unwrap();
        assert_eq!(sent.iter().map(|v| v["role"].as_str().unwrap()).collect::<Vec<_>>(), vec!["assistant", "tool", "tool", "user"]);
        assert!(!sent[1]["content"].as_str().unwrap().contains("AQ=="));
        assert_eq!(sent[3]["content"][1]["image_url"]["url"], "data:image/png;base64,AQ==");
        assert!(matches!(&canonical[1], Message::Tool { result } if result.content.has_media()));
    }
    #[test]
    fn structured_error_result_is_preserved_in_wire_envelope() {
        let mut output = ToolOutput::error("conflict"); output.structured = Some(json!({"actual":4}));
        let model = ChatModel::new(config()).unwrap();
        let body = model.body(&request(vec![Message::Tool { result: ToolResult::from_output("a", output) }])).unwrap();
        let payload: Value = serde_json::from_str(body["messages"][0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(payload["status"], "error"); assert_eq!(payload["structured"]["actual"], 4);
    }
    #[test]
    fn generation_options_are_explicit_and_model_override_is_allowlisted() {
        let mut req = request(vec![Message::user("go")]);
        req.options = ModelOptions { model: Some("alternate".into()), temperature: Some(0.2), top_p: Some(0.9), stop: Some(vec![]), ..Default::default() };
        assert!(ChatModel::new(config()).unwrap().body(&req).is_err());
        let mut c = config(); c.allowed_models.insert("alternate".into());
        let body = ChatModel::new(c).unwrap().body(&req).unwrap();
        assert_eq!(body["model"], "alternate"); assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 0.9); assert_eq!(body["stop"], json!([]));
        assert_eq!(body["max_tokens"], 128);
    }
    #[test]
    fn context_capacity_only_applies_to_the_configured_model() {
        let mut c = config(); c.context_window_tokens = Some(8192);
        let model = ChatModel::new(c).unwrap();
        assert_eq!(model.context_window_tokens(&ModelOptions::default()), Some(8192));
        assert_eq!(model.context_window_tokens(&ModelOptions { model: Some("base".into()), ..Default::default() }), Some(8192));
        assert_eq!(model.context_window_tokens(&ModelOptions { model: Some("alternate".into()), ..Default::default() }), None);
    }
    #[test]
    fn provider_options_cannot_override_tools_tokens_or_credentials() {
        for key in ["tools", "max_tokens", "api_key", "endpoint"] {
            let mut c = config(); c.request_option_fields.insert(key.into());
            assert!(ChatModel::new(c).is_err());
        }
        let mut req = request(vec![Message::user("go")]);
        req.options.provider_options = Some(ProviderData { namespace: "chat_completions".into(), value: json!({"tools":[]}) });
        assert!(ChatModel::new(config()).unwrap().body(&req).is_err());
    }
    #[test]
    fn whitelisted_provider_option_reaches_only_its_adapter() {
        let mut c = config(); c.request_option_fields.insert("reasoning_effort".into());
        let model = ChatModel::new(c).unwrap();
        let mut req = request(vec![Message::user("go")]);
        req.options.provider_options = Some(ProviderData { namespace: "chat_completions".into(), value: json!({"reasoning_effort":"low"}) });
        assert_eq!(model.body(&req).unwrap()["reasoning_effort"], "low");
        req.options.provider_options.as_mut().unwrap().namespace = "other".into();
        assert_eq!(model.body(&req).unwrap_err().code, ErrorCode::Unsupported);
    }
    #[test]
    fn replay_is_preserved_only_for_the_matching_namespace_and_fields() {
        let mut c = config(); c.message_replay_fields.insert("signed_blocks".into()); c.tool_replay_fields.insert("signature".into());
        let model = ChatModel::new(c).unwrap();
        let data = scoped(&model, json!({"signed_blocks":[{"x":1},{"x":2}]}));
        let mut call = ToolCall::new("a", "count", json!({}));
        call.provider_data = Some(scoped(&model, json!({"signature":"signed"})));
        let body = model.body(&request(vec![assistant(vec![call], Some(data.clone()))])).unwrap();
        assert_eq!(body["messages"][0]["signed_blocks"], data.value["fields"]["signed_blocks"]);
        assert_eq!(body["messages"][0]["tool_calls"][0]["signature"], "signed");
        let foreign = ProviderData { namespace: "foreign".into(), value: data.value };
        assert_eq!(model.body(&request(vec![assistant(vec![], Some(foreign))])).unwrap_err().code, ErrorCode::ModelHistoryIncompatible);
    }
    #[test]
    fn opaque_replay_cannot_rewrite_identity_or_actions() {
        let mut c = config(); c.message_replay_fields.insert("tool_calls".into());
        assert!(ChatModel::new(c).is_err());
        let model = ChatModel::new(config()).unwrap();
        // Use a valid route envelope so this exercises reserved-field rejection,
        // rather than stopping earlier on an obsolete/unscoped replay format.
        let data = scoped(&model, json!({"role":"system"}));
        assert_eq!(model.body(&request(vec![assistant(vec![], Some(data))])).unwrap_err().code, ErrorCode::ModelHistoryIncompatible);
        let mut call = ToolCall::new("actual-call", "count", json!({}));
        call.provider_data = Some(scoped(&model, json!({"id":"forged-call"})));
        assert_eq!(model.body(&request(vec![assistant(vec![call], None)])).unwrap_err().code, ErrorCode::ModelHistoryIncompatible);
    }
    #[test]
    fn complete_snapshot_capture_preserves_order_and_call_association() {
        let mut capture = ReplayCapture { namespace: "test".into(), route: "test-route".into(), reasoning_seen: false, message_fields: ["blocks".to_owned()].into_iter().collect(),
            tool_fields: ["signature".to_owned()].into_iter().collect(), message: Map::new(), tools: std::collections::BTreeMap::new() };
        let raw = r#"{"choices":[{"delta":{"blocks":["first","second"],"tool_calls":[{"index":0,"signature":"s1"}]}}]}"#;
        let events = capture.capture(raw).unwrap();
        assert!(matches!(&events[0], ModelEvent::ProviderData { target: ProtocolTarget::Assistant, data } if data.value["fields"]["blocks"] == json!(["first","second"])));
        assert!(matches!(&events[1], ModelEvent::ProviderData { target: ProtocolTarget::ToolCall { index: 0 }, data } if data.value["fields"]["signature"] == "s1"));
        let next = capture.capture(r#"{"choices":[{"delta":{"blocks":["replacement"]}}]}"#).unwrap();
        assert!(matches!(&next[0], ModelEvent::ProviderData { data, .. } if data.value["fields"]["blocks"] == json!(["replacement"])));
    }
    #[test]
    fn replay_index_uses_the_api_tool_call_limit() {
        let mut capture = ReplayCapture { namespace: "test".into(), route: "test-route".into(), reasoning_seen: false, message_fields: BTreeSet::new(),
            tool_fields: ["signature".to_owned()].into_iter().collect(), message: Map::new(), tools: std::collections::BTreeMap::new() };
        let too_large = json!({"choices":[{"delta":{"tool_calls":[{"index":MAX_TOOL_CALLS_PER_STEP,"signature":"s"}]}}]}).to_string();
        assert_eq!(capture.capture(&too_large).unwrap_err().code, ErrorCode::Limit);
        let last_valid = json!({"choices":[{"delta":{"tool_calls":[{"index":MAX_TOOL_CALLS_PER_STEP - 1,"signature":"s"}]}}]}).to_string();
        assert!(capture.capture(&last_valid).is_ok());
    }
    #[test]
    fn conflicting_reasoning_and_scoped_replay_are_rejected() {
        let model = ChatModel::new(config()).unwrap();
        let message = Message::Assistant { content: "".into(), tool_calls: vec![], reasoning_content: Some("a".into()),
            provider_data: Some(scoped(&model, json!({"reasoning_content":"b"}))) };
        assert_eq!(model.body(&request(vec![message])).unwrap_err().code, ErrorCode::ModelHistoryIncompatible);
    }
}
