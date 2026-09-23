//! Explicit protocol selection and model facts. Unknown capability is not a guessed promise.
use api::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    ChatCompletions,
    Responses,
    Messages,
}
impl Protocol {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "chat_completions" => Ok(Self::ChatCompletions),
            "responses" => Ok(Self::Responses),
            "messages" => Ok(Self::Messages),
            _ => Err(AgentError::new(
                ErrorCode::Configuration,
                "protocol must be chat_completions, responses or messages",
            )),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Self::ChatCompletions => "/chat/completions",
            Self::Responses => "/responses",
            Self::Messages => "/messages",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapabilities {
    pub tools: Option<bool>,
    pub images: Option<bool>,
    pub temperature: Option<bool>,
    pub top_p: Option<bool>,
    pub stop: Option<bool>,
    pub max_output_tokens: Option<u32>,
}
impl ModelCapabilities {
    pub fn validate(&self) -> Result<()> {
        if self
            .max_output_tokens
            .is_some_and(|n| n == 0 || n > 1_000_000_000)
        {
            return Err(AgentError::new(
                ErrorCode::Configuration,
                "model maximum output must be positive or unknown",
            ));
        }
        Ok(())
    }
    pub(crate) fn history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        options.validate()?;
        if (self.temperature == Some(false) && options.temperature.is_some())
            || (self.top_p == Some(false) && options.top_p.is_some())
            || (self.stop == Some(false) && options.stop.as_ref().is_some_and(|s| !s.is_empty()))
        {
            return Err(unsupported(
                "selected model does not support a supplied generation parameter",
            ));
        }
        if self.images == Some(false)
            && messages.iter().any(|m| match m {
                Message::User { content } => content.has_media(),
                Message::Tool { result } => result.content.has_media(),
                _ => false,
            })
        {
            return Err(unsupported("selected model does not support media input"));
        }
        Ok(())
    }
    pub(crate) fn request(&self, request: &ModelRequest) -> Result<()> {
        self.history(&request.messages, &request.options)?;
        if self.tools == Some(false) && !request.tools.is_empty() {
            return Err(unsupported("selected model does not support tool calling"));
        }
        if self
            .max_output_tokens
            .is_some_and(|n| request.max_output_tokens > n)
        {
            return Err(AgentError::new(
                ErrorCode::Limit,
                "requested output exceeds the selected model's declared maximum",
            ));
        }
        Ok(())
    }
}
fn unsupported(message: &str) -> AgentError {
    AgentError::new(ErrorCode::Unsupported, message)
}

/// Full endpoint, not a guessed base URL. No Debug: this configuration owns a secret.
pub struct ProviderConfig {
    pub protocol: Protocol,
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    pub allow_http_loopback: bool,
    pub allow_image_urls: bool,
    pub request_timeout: Duration,
    pub max_sse_event_bytes: usize,
    pub max_wire_bytes: usize,
    pub context_window_tokens: Option<u64>,
    pub capabilities: ModelCapabilities,
    pub extra_body: Map<String, Value>,
    pub protocol_namespace: String,
    pub allowed_models: BTreeSet<String>,
}
impl ProviderConfig {
    pub fn new(protocol: Protocol, endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            protocol,
            endpoint: endpoint.into(),
            model: model.into(),
            api_key: None,
            allow_http_loopback: false,
            allow_image_urls: false,
            request_timeout: Duration::from_secs(120),
            max_sse_event_bytes: 256 * 1024,
            max_wire_bytes: 8 * 1024 * 1024,
            context_window_tokens: None,
            capabilities: ModelCapabilities::default(),
            extra_body: Map::new(),
            protocol_namespace: protocol.name().into(),
            allowed_models: BTreeSet::new(),
        }
    }
}

/// Shared factory used by both model management and fixed-model hosts. Direct adapter APIs remain usable.
pub fn create_model(config: ProviderConfig) -> Result<Arc<dyn Model>> {
    config.capabilities.validate()?;
    match config.protocol {
        Protocol::ChatCompletions => {
            let caps = config.capabilities.clone();
            let mut chat = crate::ChatConfig::new(config.endpoint, config.model);
            chat.api_key = config.api_key;
            chat.allow_http_loopback = config.allow_http_loopback;
            chat.allow_image_urls = config.allow_image_urls;
            chat.request_timeout = config.request_timeout;
            chat.max_sse_event_bytes = config.max_sse_event_bytes;
            chat.max_wire_bytes = config.max_wire_bytes;
            chat.context_window_tokens = config.context_window_tokens;
            chat.extra_body = config.extra_body;
            chat.protocol_namespace = config.protocol_namespace;
            chat.allowed_models = config.allowed_models;
            Ok(Arc::new(CapabilityModel {
                inner: Arc::new(crate::ChatModel::new(chat)?),
                caps,
            }))
        }
        Protocol::Responses => Ok(Arc::new(crate::ResponsesModel::new(config)?)),
        Protocol::Messages => Ok(Arc::new(crate::MessagesModel::new(config)?)),
    }
}
struct CapabilityModel {
    inner: Arc<dyn Model>,
    caps: ModelCapabilities,
}
#[async_trait]
impl Model for CapabilityModel {
    fn context_window_tokens(&self, options: &ModelOptions) -> Option<u64> {
        self.inner.context_window_tokens(options)
    }
    fn validate_history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        self.caps.history(messages, options)?;
        self.inner.validate_history(messages, options)
    }
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream> {
        self.caps.request(&request)?;
        self.inner.stream(request, cancel).await
    }
}
