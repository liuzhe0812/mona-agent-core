//! Anthropic Messages API. Local tool execution; no server tools or remote conversation state.
use crate::{
    config::{Protocol, ProviderConfig},
    native::*,
};
use api::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub struct MessagesModel {
    http: Http,
}
impl MessagesModel {
    pub fn new(config: ProviderConfig) -> Result<Self> {
        Ok(Self {
            http: Http::new(config, Protocol::Messages)?,
        })
    }
    fn assistant(
        &self,
        content: &str,
        calls: &[ToolCall],
        reasoning: &Option<String>,
        data: &Option<ProviderData>,
        options: &ModelOptions,
    ) -> Result<Vec<Value>> {
        if reasoning.is_some() || calls.iter().any(|c| c.provider_data.is_some()) {
            return Err(incompatible());
        }
        if let Some(data) = data {
            let blocks = self.http.replay(data, options)?;
            let (text, tools, _) = inspect(blocks).map_err(|_| incompatible())?;
            if text != content || tools != calls {
                return Err(incompatible());
            }
            return Ok(blocks.clone());
        }
        let mut blocks = Vec::new();
        if !content.is_empty() {
            blocks.push(json!({"type":"text","text":content}));
        }
        for call in calls {
            tool_identity(&call.id)?;
            tool_identity(&call.name)?;
            if !call.arguments.is_object() {
                return Err(unsupported("Messages tool input must be a JSON object"));
            }
            blocks.push(
                json!({"type":"tool_use","id":call.id,"name":call.name,"input":call.arguments}),
            );
        }
        Ok(blocks)
    }
    pub(crate) fn body(&self, request: &ModelRequest) -> Result<Value> {
        self.http.config.capabilities.request(request)?;
        self.validate_history(&request.messages, &request.options)?;
        api::validate_messages(&request.messages)?;
        let mut messages = Vec::new();
        let mut system = Vec::new();
        let mut seen = false;
        for message in &request.messages {
            match message {
                Message::System { content } => {
                    if seen {
                        return Err(unsupported(
                            "Messages requires system instructions before conversation history",
                        ));
                    }
                    if !content.is_empty() {
                        system.push(json!({"type":"text","text":content}));
                    }
                }
                Message::User { content } => {
                    seen = true;
                    push(&mut messages, "user", self.http.content(content)?);
                }
                Message::Assistant {
                    content,
                    tool_calls,
                    reasoning_content,
                    provider_data,
                } => {
                    seen = true;
                    push(
                        &mut messages,
                        "assistant",
                        self.assistant(
                            content,
                            tool_calls,
                            reasoning_content,
                            provider_data,
                            &request.options,
                        )?,
                    );
                }
                Message::Tool { result } => {
                    seen = true;
                    tool_identity(&result.call_id)?;
                    let mut content = vec![json!({"type":"text","text":tool_text(result)})];
                    if let Content::Blocks(blocks) = &result.content {
                        for b in blocks {
                            match b {
                                ContentBlock::Image { .. } => content.push(self.http.image(b)?),
                                ContentBlock::Resource { .. } => {
                                    return Err(unsupported(
                                        "Messages requires resolved file content",
                                    ))
                                }
                                _ => {}
                            }
                        }
                    }
                    push(
                        &mut messages,
                        "user",
                        vec![
                            json!({"type":"tool_result","tool_use_id":result.call_id,"is_error":result.status!=ToolStatus::Success,"content":content}),
                        ],
                    );
                }
            }
        }
        let mut body = json!({"model":self.http.model(&request.options)?,"messages":messages,"max_tokens":request.max_output_tokens,"stream":true});
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !request.tools.is_empty() {
            let mut tools = Vec::new();
            for tool in &request.tools {
                tool_identity(&tool.name)?;
                tools.push(json!({"name":tool.name,"description":tool.description,"input_schema":tool.parameters}));
            }
            body["tools"] = json!(tools);
        }
        if let Some(t) = request.options.temperature {
            if t > 1.0 {
                return Err(unsupported("Messages temperature must be within 0..1"));
            }
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.options.top_p {
            body["top_p"] = json!(p);
        }
        if let Some(stop) = &request.options.stop {
            if !stop.is_empty() {
                body["stop_sequences"] = json!(stop);
            }
        }
        let extra = self.http.options(&request.options)?;
        if let Some(thinking) = extra.get("thinking") {
            if thinking["type"] == "enabled"
                && thinking["budget_tokens"]
                    .as_u64()
                    .is_some_and(|n| n >= request.max_output_tokens as u64)
            {
                return Err(AgentError::new(
                    ErrorCode::Limit,
                    "thinking budget must fit within this request's output budget",
                ));
            }
            if thinking["type"] != "disabled"
                && (request.options.temperature.is_some() || request.options.top_p.is_some())
            {
                return Err(unsupported(
                    "explicit sampling options with thinking are not supported by this adapter",
                ));
            }
        }
        for (key, value) in extra {
            body[&key] = value;
        }
        Ok(body)
    }
}
#[async_trait]
impl Model for MessagesModel {
    fn context_window_tokens(&self, options: &ModelOptions) -> Option<u64> {
        self.http.window(options)
    }
    fn validate_history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        self.http.model(options)?;
        self.http.options(options)?;
        self.http.config.capabilities.history(messages, options)?;
        for message in messages {
            match message {
                Message::Assistant {
                    content,
                    tool_calls,
                    reasoning_content,
                    provider_data,
                } => {
                    self.assistant(
                        content,
                        tool_calls,
                        reasoning_content,
                        provider_data,
                        options,
                    )?;
                }
                Message::User { content } => {
                    self.http.content(content)?;
                }
                Message::Tool { result } => {
                    tool_identity(&result.call_id)?;
                    self.http.content(&result.content)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream> {
        let body = self.body(&request)?;
        let decoder = MessagesDecoder::new(
            self.http.config.protocol_namespace.clone(),
            self.http.route(&request.options)?,
        );
        self.http.stream(body, decoder, cancel).await
    }
}
fn push(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut().filter(|m| m["role"] == role) {
        last["content"]
            .as_array_mut()
            .expect("constructed block list")
            .extend(blocks);
    } else {
        messages.push(json!({"role":role,"content":blocks}));
    }
}
fn tool_identity(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(incompatible());
    }
    Ok(())
}
fn inspect(blocks: &[Value]) -> Result<(String, Vec<ToolCall>, bool)> {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut private = false;
    let mut ids = BTreeSet::new();
    for block in blocks {
        let kind = string(block, "type")?;
        let keys: &[&str] = match kind {
            "text" => &["type", "text", "citations"],
            "tool_use" => &["type", "id", "name", "input"],
            "thinking" => &["type", "thinking", "signature"],
            "redacted_thinking" => &["type", "data"],
            _ => {
                return Err(unsupported(
                    "unsupported Messages content block; server tools are not enabled",
                ))
            }
        };
        if block
            .as_object()
            .is_none_or(|o| o.keys().any(|k| !keys.contains(&k.as_str())))
        {
            return Err(protocol_error("unsupported Messages block fields"));
        }
        match kind {
            "text" => {
                if block
                    .get("citations")
                    .is_some_and(|c| !c.is_null() && c.as_array().is_none_or(|a| !a.is_empty()))
                {
                    return Err(unsupported(
                        "citation blocks require a separate content adapter",
                    ));
                }
                text.push_str(string(block, "text")?);
            }
            "tool_use" => {
                let id = identifier(block, "id")?;
                let name = identifier(block, "name")?;
                tool_identity(&id)?;
                tool_identity(&name)?;
                if !ids.insert(id.clone())
                    || calls.len() >= MAX_TOOL_CALLS_PER_STEP
                    || !block["input"].is_object()
                {
                    return Err(protocol_error("invalid or duplicate tool input"));
                }
                calls.push(ToolCall::new(id, name, block["input"].clone()));
            }
            "thinking" => {
                string(block, "thinking")?;
                if string(block, "signature")?.is_empty() {
                    return Err(protocol_error("thinking block is missing its signature"));
                }
                private = true;
            }
            "redacted_thinking" => {
                if string(block, "data")?.is_empty() {
                    return Err(protocol_error("redacted thinking is empty"));
                }
                private = true;
            }
            _ => unreachable!(),
        }
    }
    Ok((text, calls, private))
}
struct Block {
    value: Value,
    arguments: String,
    call_index: Option<usize>,
    closed: bool,
}
pub(crate) struct MessagesDecoder {
    namespace: String,
    route: String,
    started: bool,
    ended: bool,
    output: bool,
    blocks: Vec<Block>,
    calls: usize,
    finish: Option<FinishReason>,
    input: Option<u64>,
    output_tokens: Option<u64>,
    private_bytes: usize,
}
impl MessagesDecoder {
    pub(crate) fn new(namespace: String, route: String) -> Self {
        Self {
            namespace,
            route,
            started: false,
            ended: false,
            output: false,
            blocks: vec![],
            calls: 0,
            finish: None,
            input: None,
            output_tokens: None,
            private_bytes: 0,
        }
    }
    fn usage(&mut self, value: &Value, initial: bool) -> Result<Vec<ModelEvent>> {
        if value.is_null() {
            return Ok(vec![]);
        }
        let object = value
            .as_object()
            .ok_or_else(|| protocol_error("invalid usage"))?;
        let number = |key: &str| -> Result<Option<u64>> {
            object
                .get(key)
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| protocol_error("invalid token count"))
                })
                .transpose()
        };
        if initial {
            if let Some(input) = number("input_tokens")? {
                let read = number("cache_read_input_tokens")?.unwrap_or(0);
                let created = number("cache_creation_input_tokens")?.unwrap_or(0);
                self.input = Some(
                    input
                        .checked_add(read)
                        .and_then(|v| v.checked_add(created))
                        .ok_or_else(|| protocol_error("token count overflow"))?,
                );
            }
        }
        if let Some(tokens) = number("output_tokens")? {
            if self.output_tokens.is_some_and(|old| old > tokens) {
                return Err(protocol_error("usage regressed"));
            }
            self.output_tokens = Some(tokens);
        }
        Ok(match (self.input, self.output_tokens) {
            (Some(input_tokens), Some(output_tokens)) => vec![ModelEvent::Usage(Usage {
                input_tokens,
                output_tokens,
            })],
            _ => vec![],
        })
    }
}
impl Decoder for MessagesDecoder {
    fn done(&self) -> bool {
        self.ended
    }
    fn output_started(&self) -> bool {
        self.output
    }
    fn decode(&mut self, event: Value) -> Result<Vec<ModelEvent>> {
        if self.ended {
            return Err(protocol_error("Messages event after completion"));
        }
        let kind = string(&event, "type")?;
        if kind == "error" {
            return Err(crate::chat::classify_provider_error(&event["error"]));
        }
        if kind == "ping" {
            return Ok(vec![]);
        }
        if kind == "message_start" {
            if self.started
                || event["message"]["role"] != "assistant"
                || event["message"]["content"]
                    .as_array()
                    .is_none_or(|a| !a.is_empty())
            {
                return Err(protocol_error("invalid message_start"));
            }
            identifier(&event["message"], "id")?;
            self.started = true;
            return self.usage(&event["message"]["usage"], true);
        }
        if !self.started {
            return Err(protocol_error("Messages stream lacks message_start"));
        }
        let mut events = vec![];
        match kind {
            "content_block_start" => {
                if self.finish.is_some()
                    || index(&event, "index")? != self.blocks.len()
                    || self.blocks.last().is_some_and(|b| !b.closed)
                {
                    return Err(protocol_error("invalid content block order"));
                }
                let mut value = event["content_block"].clone();
                if value["type"] == "thinking" && value.get("signature").is_none() {
                    value["signature"] = json!("");
                }
                let block_type = string(&value, "type")?;
                let mut call_index = None;
                match block_type {
                    "text"=>{let text=string(&value,"text")?; if !text.is_empty(){self.output=true;events.push(ModelEvent::Text(text.into()));}}
                    "tool_use"=>{
                        self.output=true;
                        if self.calls>=MAX_TOOL_CALLS_PER_STEP||!value["input"].is_object(){return Err(protocol_error("invalid tool block"));}
                        let id=identifier(&value,"id")?;let name=identifier(&value,"name")?; tool_identity(&id)?;tool_identity(&name)?;
                        call_index=Some(self.calls);self.calls+=1;
                        events.push(ModelEvent::ToolDelta{index:call_index.unwrap(),id:Some(id),name:Some(name),arguments:String::new()});
                    }
                    "thinking"|"redacted_thinking"=>{self.output=true;}
                    _=>return Err(unsupported("unsupported Messages block; hosted tools and fallback execution are disabled")),
                }
                self.blocks.push(Block {
                    value,
                    arguments: String::new(),
                    call_index,
                    closed: false,
                });
            }
            "content_block_delta" => {
                let i = index(&event, "index")?;
                let block = self
                    .blocks
                    .get_mut(i)
                    .filter(|b| !b.closed)
                    .ok_or_else(|| protocol_error("delta without an active content block"))?;
                let delta = &event["delta"];
                let kind = string(delta, "type")?;
                let (field, source) = match (kind, block.value["type"].as_str()) {
                    ("text_delta", Some("text")) => ("text", "text"),
                    ("thinking_delta", Some("thinking")) => ("thinking", "thinking"),
                    ("signature_delta", Some("thinking")) => ("signature", "signature"),
                    ("input_json_delta", Some("tool_use")) => {
                        if block.value["input"]
                            .as_object()
                            .is_some_and(|v| !v.is_empty())
                        {
                            return Err(protocol_error("tool input mixes full value and deltas"));
                        }
                        let text = string(delta, "partial_json")?;
                        block.arguments.push_str(text);
                        events.push(ModelEvent::ToolDelta {
                            index: block.call_index.unwrap(),
                            id: None,
                            name: None,
                            arguments: text.into(),
                        });
                        return Ok(events);
                    }
                    _ => return Err(unsupported("unsupported or mismatched Messages delta")),
                };
                let text = string(delta, source)?;
                self.output |= !text.is_empty();
                let old = block.value[field]
                    .as_str()
                    .ok_or_else(|| protocol_error("invalid delta target"))?;
                block.value[field] = json!(format!("{old}{text}"));
                if field == "text" {
                    events.push(ModelEvent::Text(text.into()));
                } else {
                    self.private_bytes = self.private_bytes.saturating_add(text.len());
                    if self.private_bytes > MAX_PROVIDER_DATA_BYTES {
                        return Err(AgentError::new(
                            ErrorCode::Limit,
                            "private reasoning exceeds replay storage limit",
                        ));
                    }
                }
            }
            "content_block_stop" => {
                let i = index(&event, "index")?;
                let block = self
                    .blocks
                    .get_mut(i)
                    .filter(|b| !b.closed)
                    .ok_or_else(|| protocol_error("duplicate or missing content block"))?;
                if let Some(call_index) = block.call_index {
                    if block.arguments.is_empty() {
                        block.arguments = block.value["input"].to_string();
                        events.push(ModelEvent::ToolDelta {
                            index: call_index,
                            id: None,
                            name: None,
                            arguments: block.arguments.clone(),
                        });
                    }
                    block.value["input"] = serde_json::from_str(&block.arguments)
                        .map_err(|_| protocol_error("incomplete tool input JSON"))?;
                    if !block.value["input"].is_object() {
                        return Err(protocol_error("tool input is not an object"));
                    }
                }
                block.closed = true;
            }
            "message_delta" => {
                if self.blocks.iter().any(|b| !b.closed) {
                    return Err(protocol_error("message_delta before content closed"));
                }
                if let Some(reason) = event["delta"].get("stop_reason").filter(|r| !r.is_null()) {
                    if self.finish.is_some() {
                        return Err(protocol_error("duplicate stop reason"));
                    }
                    self.finish = Some(match reason.as_str() {
                        Some("end_turn" | "stop_sequence") => FinishReason::Stop,
                        Some("tool_use") => FinishReason::ToolCalls,
                        Some("max_tokens" | "model_context_window_exceeded") => {
                            FinishReason::Length
                        }
                        Some("refusal") => FinishReason::Filtered,
                        _ => {
                            return Err(unsupported(
                                "Messages stop reason requires unsupported execution behavior",
                            ))
                        }
                    });
                }
                events.extend(self.usage(&event["usage"], false)?);
            }
            "message_stop" => {
                let finish = self
                    .finish
                    .ok_or_else(|| protocol_error("message_stop lacks a stop reason"))?;
                if self.blocks.iter().any(|b| !b.closed) {
                    return Err(protocol_error("message ended with unfinished blocks"));
                }
                if matches!(finish, FinishReason::Stop | FinishReason::ToolCalls) {
                    let values = self
                        .blocks
                        .iter()
                        .map(|b| b.value.clone())
                        .collect::<Vec<_>>();
                    let (_, calls, private) = inspect(&values)?;
                    if (finish == FinishReason::ToolCalls) != (!calls.is_empty()) {
                        return Err(protocol_error("stop reason contradicts tool calls"));
                    }
                    if private {
                        events.push(replay_event(&self.namespace, &self.route, &values)?);
                    }
                }
                self.ended = true;
                events.extend([ModelEvent::Finish(finish), ModelEvent::End]);
            }
            _ => {
                return Err(unsupported(
                    "unknown Messages event; refusing to lose response semantics",
                ))
            }
        }
        Ok(events)
    }
}
