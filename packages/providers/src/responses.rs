//! Stateless Responses API. Replay is local; store/background/remote tools cannot be enabled.
use crate::{
    config::{Protocol, ProviderConfig},
    native::*,
};
use api::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub struct ResponsesModel {
    http: Http,
}
impl ResponsesModel {
    pub fn new(config: ProviderConfig) -> Result<Self> {
        Ok(Self {
            http: Http::new(config, Protocol::Responses)?,
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
            let items = self.http.replay(data, options)?;
            let (text, tools, _, refusal) = inspect(items, true).map_err(|_| incompatible())?;
            if refusal || text != content || tools != calls {
                return Err(incompatible());
            }
            return Ok(items.clone());
        }
        let mut items = vec![];
        if !content.is_empty() {
            items.push(json!({"role":"assistant","content":content}));
        }
        for call in calls {
            items.push(json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":serde_json::to_string(&call.arguments).map_err(|_|protocol_error("cannot serialize tool arguments"))?}));
        }
        Ok(items)
    }
    pub(crate) fn body(&self, request: &ModelRequest) -> Result<Value> {
        self.http.config.capabilities.request(request)?;
        self.validate_history(&request.messages, &request.options)?;
        api::validate_messages(&request.messages)?;
        if request.options.stop.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(unsupported("Responses does not support stop sequences"));
        }
        let mut input = vec![];
        for message in &request.messages {
            match message {
                Message::System { content } => {
                    input.push(json!({"role":"system","content":content}))
                }
                Message::User { content } => {
                    input.push(json!({"role":"user","content":self.http.content(content)?}))
                }
                Message::Assistant {
                    content,
                    tool_calls,
                    reasoning_content,
                    provider_data,
                } => input.extend(self.assistant(
                    content,
                    tool_calls,
                    reasoning_content,
                    provider_data,
                    &request.options,
                )?),
                Message::Tool { result } => {
                    let mut output = vec![json!({"type":"input_text","text":tool_text(result)})];
                    if let Content::Blocks(blocks) = &result.content {
                        for block in blocks {
                            match block {
                                ContentBlock::Image { .. } => output.push(self.http.image(block)?),
                                ContentBlock::Resource { .. } => {
                                    return Err(unsupported(
                                        "Responses requires resolved resource content",
                                    ))
                                }
                                _ => {}
                            }
                        }
                    }
                    let output = if output.len() == 1 {
                        json!(tool_text(result))
                    } else {
                        json!(output)
                    };
                    input.push(json!({"type":"function_call_output","call_id":result.call_id,"output":output}));
                }
            }
        }
        let mut body = json!({"model":self.http.model(&request.options)?,"input":input,"stream":true,"store":false,
            "include":["reasoning.encrypted_content"],"max_output_tokens":request.max_output_tokens});
        if !request.tools.is_empty() {
            body["tools"]=json!(request.tools.iter().map(|t|json!({"type":"function","name":t.name,"description":t.description,"parameters":t.parameters,"strict":false})).collect::<Vec<_>>());
        }
        if let Some(t) = request.options.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.options.top_p {
            body["top_p"] = json!(p);
        }
        for (key, value) in self.http.options(&request.options)? {
            body[&key] = value;
        }
        Ok(body)
    }
}
#[async_trait]
impl Model for ResponsesModel {
    fn context_window_tokens(&self, options: &ModelOptions) -> Option<u64> {
        self.http.window(options)
    }
    fn validate_history(&self, messages: &[Message], options: &ModelOptions) -> Result<()> {
        self.http.model(options)?;
        self.http.options(options)?;
        self.http.config.capabilities.history(messages, options)?;
        if options.stop.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(unsupported("Responses does not support stop sequences"));
        }
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
        self.http
            .stream(
                body,
                ResponsesDecoder::new(
                    self.http.config.protocol_namespace.clone(),
                    self.http.route(&request.options)?,
                ),
                cancel,
            )
            .await
    }
}
fn inspect(
    items: &[Value],
    require_encrypted: bool,
) -> Result<(String, Vec<ToolCall>, bool, bool)> {
    let mut text = String::new();
    let mut calls = vec![];
    let mut private = false;
    let mut refusal = false;
    let mut ids = BTreeSet::new();
    for item in items {
        let kind = string(item, "type")?;
        let keys: &[&str] = match kind {
            "message" => &["id", "type", "role", "content", "status", "phase"],
            "function_call" => &["id", "type", "call_id", "name", "arguments", "status"],
            "reasoning" => &[
                "id",
                "type",
                "summary",
                "encrypted_content",
                "status",
                "content",
            ],
            _ => {
                return Err(unsupported(
                    "unsupported Responses output item; hosted tools are disabled",
                ))
            }
        };
        if item
            .as_object()
            .is_none_or(|o| o.keys().any(|k| !keys.contains(&k.as_str())))
        {
            return Err(unsupported("unsupported Responses item fields"));
        }
        identifier(item, "id")?;
        if item
            .get("status")
            .is_some_and(|v| !v.is_null() && v != "completed")
        {
            return Err(protocol_error("incomplete output item"));
        }
        match kind {
            "message" => {
                if item["role"] != "assistant" {
                    return Err(protocol_error("non-assistant output message"));
                }
                let content = item["content"]
                    .as_array()
                    .filter(|a| a.len() <= 128)
                    .ok_or_else(|| protocol_error("invalid output content"))?;
                for block in content {
                    match string(block, "type")? {
                        "output_text" => {
                            if block
                                .get("annotations")
                                .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
                            {
                                return Err(unsupported(
                                    "Responses citations require a separate content adapter",
                                ));
                            }
                            text.push_str(string(block, "text")?);
                        }
                        "refusal" => {
                            refusal = true;
                            string(block, "refusal")?;
                        }
                        _ => return Err(unsupported("unsupported Responses output content")),
                    }
                }
                private |= item.get("phase").is_some_and(|v| !v.is_null());
            }
            "function_call" => {
                let id = identifier(item, "call_id")?;
                let name = identifier(item, "name")?;
                if !ids.insert(id.clone()) || calls.len() >= MAX_TOOL_CALLS_PER_STEP {
                    return Err(protocol_error("duplicate or excessive tool calls"));
                }
                let args = serde_json::from_str(string(item, "arguments")?)
                    .map_err(|_| protocol_error("incomplete function arguments"))?;
                calls.push(ToolCall::new(id, name, args));
            }
            "reasoning" => {
                private = true;
                if require_encrypted
                    && item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s.is_empty())
                {
                    return Err(unsupported(
                        "stateless reasoning requires encrypted_content from the selected endpoint",
                    ));
                }
                for key in ["summary", "content"] {
                    if let Some(value) = item.get(key).filter(|v| !v.is_null()) {
                        let parts = value
                            .as_array()
                            .filter(|a| a.len() <= 128)
                            .ok_or_else(|| protocol_error("invalid reasoning content"))?;
                        for part in parts {
                            let kind = string(part, "type")?;
                            if !matches!(kind, "summary_text" | "reasoning_text") {
                                return Err(unsupported("unsupported reasoning block"));
                            }
                            string(part, "text")?;
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    Ok((text, calls, private, refusal))
}
struct Item {
    identity: Value,
    text: String,
    arguments: String,
    call_index: Option<usize>,
    done: Option<Value>,
}
pub(crate) struct ResponsesDecoder {
    namespace: String,
    route: String,
    response_id: Option<String>,
    items: Vec<Item>,
    calls: usize,
    ended: bool,
    output: bool,
    text: String,
}
impl ResponsesDecoder {
    pub(crate) fn new(namespace: String, route: String) -> Self {
        Self {
            namespace,
            route,
            response_id: None,
            items: vec![],
            calls: 0,
            ended: false,
            output: false,
            text: String::new(),
        }
    }
    fn item(&mut self, event: &Value) -> Result<&mut Item> {
        let i = index(event, "output_index")?;
        let item = self
            .items
            .get_mut(i)
            .filter(|i| i.done.is_none())
            .ok_or_else(|| protocol_error("delta without an active output item"))?;
        if event
            .get("item_id")
            .is_some_and(|id| id != &item.identity["id"])
        {
            return Err(protocol_error("output item identity changed"));
        }
        Ok(item)
    }
    fn finish_item(&mut self, i: usize, value: Value) -> Result<Vec<ModelEvent>> {
        let item = self
            .items
            .get_mut(i)
            .filter(|i| i.done.is_none())
            .ok_or_else(|| protocol_error("duplicate or missing output item"))?;
        if value["id"] != item.identity["id"] || value["type"] != item.identity["type"] {
            return Err(protocol_error("output item identity changed"));
        }
        let (text, calls, private, refusal) = inspect(std::slice::from_ref(&value), false)?;
        self.output |= !text.is_empty() || !calls.is_empty() || private || refusal;
        let mut events = vec![];
        if let Some(call_index) = item.call_index {
            let call = calls
                .first()
                .ok_or_else(|| protocol_error("function output is missing"))?;
            if value["call_id"] != item.identity["call_id"]
                || value["name"] != item.identity["name"]
            {
                return Err(protocol_error("function identity changed"));
            }
            let args = string(&value, "arguments")?;
            if !args.starts_with(&item.arguments) {
                return Err(protocol_error(
                    "final function arguments contradict streamed arguments",
                ));
            }
            let suffix = &args[item.arguments.len()..];
            if !suffix.is_empty() {
                events.push(ModelEvent::ToolDelta {
                    index: call_index,
                    id: None,
                    name: None,
                    arguments: suffix.into(),
                });
            }
            let _ = call;
            item.arguments = args.into();
        } else if value["type"] == "message" {
            if !text.starts_with(&item.text) {
                return Err(protocol_error("final message contradicts streamed text"));
            }
            let suffix = &text[item.text.len()..];
            if !suffix.is_empty() {
                self.text.push_str(suffix);
                events.push(ModelEvent::Text(suffix.into()));
            }
            item.text = text;
            if refusal {
                self.output = true;
            }
        }
        item.done = Some(value);
        Ok(events)
    }
}
impl Decoder for ResponsesDecoder {
    fn done(&self) -> bool {
        self.ended
    }
    fn output_started(&self) -> bool {
        self.output
    }
    fn decode(&mut self, event: Value) -> Result<Vec<ModelEvent>> {
        if self.ended {
            return Err(protocol_error("Responses event after completion"));
        }
        let kind = string(&event, "type")?;
        if kind == "error" {
            return Err(crate::chat::classify_provider_error(&event));
        }
        if kind == "response.failed" {
            return Err(crate::chat::classify_provider_error(
                &event["response"]["error"],
            ));
        }
        if kind == "response.created" {
            if self.response_id.is_some() {
                return Err(protocol_error("duplicate response.created"));
            }
            self.response_id = Some(identifier(&event["response"], "id")?);
            return Ok(vec![]);
        }
        let id = self
            .response_id
            .as_ref()
            .ok_or_else(|| protocol_error("Responses stream lacks response.created"))?;
        if event
            .get("response_id")
            .and_then(Value::as_str)
            .is_some_and(|other| other != id)
        {
            return Err(protocol_error("response identity changed"));
        }
        let mut events = vec![];
        match kind {
            "response.in_progress" => {
                if event["response"]
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|other| other != id)
                {
                    return Err(protocol_error("response identity changed"));
                }
            }
            "response.output_item.added" => {
                if index(&event, "output_index")? != self.items.len() {
                    return Err(protocol_error("output items must have contiguous indices"));
                }
                let value = event["item"].clone();
                let kind = string(&value, "type")?;
                identifier(&value, "id")?;
                let mut call_index = None;
                let mut arguments = String::new();
                let mut initial_text = String::new();
                match kind {
                    "function_call" => {
                        self.output = true;
                        if self.calls >= MAX_TOOL_CALLS_PER_STEP {
                            return Err(protocol_error("too many tool calls"));
                        }
                        call_index = Some(self.calls);
                        self.calls += 1;
                        arguments = string(&value, "arguments")?.into();
                        events.push(ModelEvent::ToolDelta {
                            index: call_index.unwrap(),
                            id: Some(identifier(&value, "call_id")?),
                            name: Some(identifier(&value, "name")?),
                            arguments: arguments.clone(),
                        });
                    }
                    "message" => {
                        if value["role"] != "assistant" {
                            return Err(protocol_error("unexpected output role"));
                        }
                        // Some custom endpoints include text in the initial item.
                        // Preserve that content and its retry fence, not only later deltas.
                        let mut initial = value.clone();
                        initial["status"] = json!("completed");
                        let (text, _, private, refusal) = inspect(&[initial], false)?;
                        self.output |= !text.is_empty() || private || refusal;
                        if !text.is_empty() {
                            self.text.push_str(&text);
                            events.push(ModelEvent::Text(text.clone()));
                        }
                        initial_text = text;
                    }
                    "reasoning" => {
                        self.output = true;
                    }
                    _ => {
                        return Err(unsupported(
                            "unsupported output type; hosted tools are disabled",
                        ))
                    }
                }
                self.items.push(Item {
                    identity: value,
                    text: initial_text,
                    arguments,
                    call_index,
                    done: None,
                });
            }
            "response.output_text.delta" => {
                index(&event, "content_index")?;
                let delta = string(&event, "delta")?;
                self.output |= !delta.is_empty();
                let item = self.item(&event)?;
                if item.identity["type"] != "message" {
                    return Err(protocol_error("text delta on non-message"));
                }
                item.text.push_str(delta);
                self.text.push_str(delta);
                events.push(ModelEvent::Text(delta.into()));
            }
            "response.function_call_arguments.delta" => {
                let delta = string(&event, "delta")?;
                self.output = true;
                let item = self.item(&event)?;
                let call_index = item
                    .call_index
                    .ok_or_else(|| protocol_error("arguments on a non-function item"))?;
                item.arguments.push_str(delta);
                events.push(ModelEvent::ToolDelta {
                    index: call_index,
                    id: None,
                    name: None,
                    arguments: delta.into(),
                });
            }
            "response.function_call_arguments.done" => {
                let full = string(&event, "arguments")?;
                let item = self.item(&event)?;
                let call_index = item
                    .call_index
                    .ok_or_else(|| protocol_error("arguments on a non-function item"))?;
                if !full.starts_with(&item.arguments) {
                    return Err(protocol_error("argument completion contradicts deltas"));
                }
                let suffix = &full[item.arguments.len()..];
                if !suffix.is_empty() {
                    events.push(ModelEvent::ToolDelta {
                        index: call_index,
                        id: None,
                        name: None,
                        arguments: suffix.into(),
                    });
                }
                item.arguments = full.into();
            }
            "response.output_item.done" => {
                events.extend(
                    self.finish_item(index(&event, "output_index")?, event["item"].clone())?,
                );
            }
            "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.refusal.delta"
            | "response.refusal.done" => {
                let item = self.item(&event)?;
                if item.identity["type"] != "message" {
                    return Err(protocol_error("content event on non-message"));
                }
                if let Some(part) = event.get("part") {
                    if !matches!(part["type"].as_str(), Some("output_text" | "refusal")) {
                        return Err(unsupported("unsupported Responses content part"));
                    }
                }
                if kind.starts_with("response.refusal") {
                    self.output = true;
                }
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done" => {
                let item = self.item(&event)?;
                if item.identity["type"] != "reasoning" {
                    return Err(protocol_error("reasoning event on wrong item"));
                }
                self.output = true;
            }
            "response.completed" | "response.incomplete" => {
                let response = &event["response"];
                if response["id"].as_str() != Some(id.as_str()) {
                    return Err(protocol_error("terminal response identity changed"));
                }
                if let Some(usage) = response.get("usage").filter(|v| !v.is_null()) {
                    let input_tokens = usage["input_tokens"]
                        .as_u64()
                        .ok_or_else(|| protocol_error("invalid input usage"))?;
                    let output_tokens = usage["output_tokens"]
                        .as_u64()
                        .ok_or_else(|| protocol_error("invalid output usage"))?;
                    events.push(ModelEvent::Usage(Usage {
                        input_tokens,
                        output_tokens,
                    }));
                }
                let finish = if kind == "response.incomplete" {
                    if response["status"] != "incomplete" {
                        return Err(protocol_error("inconsistent incomplete status"));
                    }
                    match response["incomplete_details"]["reason"].as_str() {
                        Some("max_output_tokens") => FinishReason::Length,
                        Some("content_filter") => FinishReason::Filtered,
                        _ => return Err(unsupported("unsupported Responses incomplete reason")),
                    }
                } else {
                    if response["status"] != "completed" {
                        return Err(protocol_error("inconsistent completion status"));
                    }
                    let final_items = response["output"]
                        .as_array()
                        .ok_or_else(|| protocol_error("missing terminal output"))?;
                    if final_items.len() != self.items.len()
                        || self.items.iter().any(|i| i.done.is_none())
                    {
                        return Err(protocol_error("response completed with unfinished items"));
                    }
                    for (i, final_item) in self.items.iter().zip(final_items) {
                        let prior = i.done.as_ref().unwrap();
                        if prior["id"] != final_item["id"] || prior["type"] != final_item["type"] {
                            return Err(protocol_error("terminal output identity changed"));
                        }
                        // Encrypted state may arrive only in the terminal snapshot. Do not
                        // replace an already supplied signature or any other completed field.
                        let mut old = prior.clone();
                        let mut new = final_item.clone();
                        if prior["type"] == "reasoning"
                            && prior.get("encrypted_content").is_none_or(|v| v.is_null())
                        {
                            old.as_object_mut().unwrap().remove("encrypted_content");
                            new.as_object_mut().unwrap().remove("encrypted_content");
                        }
                        old.as_object_mut().unwrap().remove("status");
                        new.as_object_mut()
                            .ok_or_else(|| protocol_error("invalid terminal item"))?
                            .remove("status");
                        if old != new {
                            return Err(protocol_error("terminal item contradicts completed item"));
                        }
                    }
                    let (text, calls, private, refusal) = inspect(final_items, true)?;
                    if text != self.text || calls.len() != self.calls {
                        return Err(protocol_error(
                            "terminal output differs from streamed content",
                        ));
                    }
                    if private && !refusal {
                        events.push(replay_event(&self.namespace, &self.route, final_items)?);
                    }
                    if refusal {
                        FinishReason::Filtered
                    } else if calls.is_empty() {
                        FinishReason::Stop
                    } else {
                        FinishReason::ToolCalls
                    }
                };
                self.ended = true;
                events.extend([ModelEvent::Finish(finish), ModelEvent::End]);
            }
            _ => {
                return Err(unsupported(
                    "unknown Responses event; refusing to lose response semantics",
                ))
            }
        }
        Ok(events)
    }
}
