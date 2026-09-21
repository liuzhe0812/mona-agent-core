use crate::{gate::{bounded, lock, CancelOnDrop}, validation::{valid_name, validate_messages}};
use agent_api::*;
use futures_util::StreamExt;
use std::{collections::BTreeMap, sync::{Arc, Mutex}};
use tokio::time::Instant;

#[derive(Default)]
struct Audits { records: Vec<RequestAudit>, reserved_bytes: usize }

pub(crate) struct Gateway {
    raw: Arc<dyn Model>, task: TaskControl, cancel: CancellationToken,
    defaults: ModelOptions,
    limits: RunLimits, audits: Arc<Mutex<Audits>>,
}
impl Gateway {
    pub fn new(raw: Arc<dyn Model>, task: TaskControl, cancel: CancellationToken, limits: RunLimits, defaults: ModelOptions) -> Self {
        Self { raw, task, cancel, limits, defaults, audits: Arc::new(Mutex::new(Audits::default())) }
    }
    pub fn audits(&self) -> Vec<RequestAudit> { lock(&self.audits).records.clone() }
}

struct AuditGuard {
    audits: Arc<Mutex<Audits>>, task: TaskControl, index: usize, settled: bool,
}
impl AuditGuard {
    fn settle(&mut self, error: Option<AgentError>, usage: Option<Usage>, partial_text: Option<String>) {
        let mut audits = lock(&self.audits);
        let record = &mut audits.records[self.index];
        record.error = error.map(|mut e| { e.message = clip_utf8(&e.message, 4096).to_owned(); e });
        record.usage = usage;
        record.partial_text = partial_text;
        record.settled = true;
        self.settled = true;
    }
}
impl Drop for AuditGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.task.record_usage(None);
            self.settle(Some(AgentError::new(ErrorCode::Cancelled, "model future dropped before settlement")), None, None);
        }
    }
}

#[async_trait]
impl ModelCaller for Gateway {
    async fn complete(&self, mut request: ModelRequest, sink: Option<Arc<dyn ModelSink>>) -> Result<ModelReply> {
        request.options = request.options.inherit(&self.defaults);
        request.options.validate()?;
        validate_messages(&request.messages)?;
        let offered: std::collections::BTreeSet<_> = request.tools.iter().map(|t| t.name.clone()).collect();
        if offered.len() != request.tools.len() || offered.iter().any(|name| !valid_name(name)) {
            return Err(AgentError::new(ErrorCode::ModelProtocol, "invalid or duplicate model tool declaration"));
        }
        let bytes = serde_json::to_vec(&request)
            .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "request serialization failed"))?.len();
        if bytes > self.limits.max_context_bytes {
            return Err(AgentError::new(ErrorCode::Limit, "model request exceeds byte limit; use a context transform"));
        }
        if request.max_output_tokens == 0 || request.max_output_tokens > self.limits.max_output_tokens {
            return Err(AgentError::new(ErrorCode::Limit, "model output-token request exceeds run limit"));
        }
        if self.cancel.is_cancelled() { return Err(AgentError::new(ErrorCode::Cancelled, "run cancelled")); }
        let index = {
            let mut audits = lock(&self.audits);
            // Reserve room for bounded error/partial-text evidence as well as the request.
            let reservation = bytes.saturating_add(8192);
            if audits.reserved_bytes.saturating_add(reservation) > self.limits.max_audit_bytes {
                return Err(AgentError::new(ErrorCode::Limit, "run audit capacity reached"));
            }
            self.task.reserve_model_call()?;
            let index = audits.records.len();
            audits.records.push(RequestAudit { call_number: index as u64 + 1, request: request.clone(),
                error: None, usage: None, settled: false, partial_text: None });
            audits.reserved_bytes += reservation;
            index
        };
        let mut guard = AuditGuard { audits: self.audits.clone(), task: self.task.clone(), index, settled: false };
        let operation = self.cancel.child_token();
        let _drop_cancel = CancelOnDrop(operation.clone());
        let mut collected = Collected::default();
        let deadline = self.task.deadline().min(Instant::now() + self.limits.model_timeout);
        let result = bounded(&self.cancel, &operation, deadline, self.limits.cancellation_grace, async {
            let mut stream = self.raw.stream(request, operation.clone()).await?;
            while let Some(event) = stream.next().await {
                collected.push(event?, &self.limits, sink.as_deref())?;
            }
            let reply = collected.reply()?;
            if reply.tool_calls.iter().any(|call| !offered.contains(&call.name)) {
                return Err(AgentError::new(ErrorCode::ModelProtocol, "model requested a tool outside this round's offered set"));
            }
            Ok(reply)
        }).await;
        self.task.record_usage(collected.usage);
        if result.is_err() { self.task.mark_usage_incomplete(); }
        let result = result.and_then(|reply| { self.task.check()?; Ok(reply) });
        let error = result.as_ref().err().cloned();
        let partial = if error.is_some() { Some(clip_utf8(&collected.text, 4096).to_owned()) } else { None };
        guard.settle(error, collected.usage, partial);
        result
    }
}

#[derive(Default)]
struct PartialCall { id: String, name: String, arguments: String, provider_data: Option<ProviderData> }
#[derive(Default)]
struct Collected {
    text: String, reasoning: String, provider_data: Option<ProviderData>, calls: BTreeMap<usize, PartialCall>,
    finish: Option<FinishReason>, ended: bool, usage: Option<Usage>, bytes: usize,
}
impl Collected {
    fn push(&mut self, event: ModelEvent, limits: &RunLimits, sink: Option<&dyn ModelSink>) -> Result<()> {
        if self.ended { return Err(AgentError::new(ErrorCode::ModelProtocol, "event after model stream end")); }
        let data_bytes = match &event {
            ModelEvent::Text(s) | ModelEvent::Reasoning(s) => s.len(),
            ModelEvent::ToolDelta { id, name, arguments, .. } => id.as_ref().map_or(0, String::len)
                .saturating_add(name.as_ref().map_or(0, String::len)).saturating_add(arguments.len()),
            ModelEvent::ProviderData { data, .. } => serde_json::to_vec(data).map_or(usize::MAX, |v| v.len()),
            _ => 0,
        };
        self.bytes = self.bytes.saturating_add(data_bytes);
        if self.bytes > limits.max_response_bytes {
            return Err(AgentError::new(ErrorCode::Limit, "model output exceeds byte limit"));
        }
        if self.finish.is_some() && matches!(&event, ModelEvent::Text(_) | ModelEvent::Reasoning(_) | ModelEvent::ToolDelta { .. } | ModelEvent::ProviderData { .. }) {
            return Err(AgentError::new(ErrorCode::ModelProtocol, "model emitted content after finish reason"));
        }
        match event {
            ModelEvent::Text(text) => {
                self.text.push_str(&text);
                if let Some(sink) = sink { sink.text(&text); }
            }
            ModelEvent::Reasoning(text) => self.reasoning.push_str(&text),
            ModelEvent::ToolDelta { index, id, name, arguments } => {
                if index >= limits.max_tools_per_step { return Err(AgentError::new(ErrorCode::Limit, "too many tool calls")); }
                let call = self.calls.entry(index).or_default();
                if let Some(id) = id {
                    if !call.id.is_empty() && call.id != id {
                        return Err(AgentError::new(ErrorCode::ModelProtocol, "tool-call id changed mid-stream"));
                    }
                    call.id = id;
                }
                if let Some(name) = name {
                    // Names may be fragmented; identical repeated metadata is tolerated.
                    if call.name != name { call.name.push_str(&name); }
                }
                call.arguments.push_str(&arguments);
                if let Some(sink) = sink {
                    sink.tool_delta(index, if call.id.is_empty() { None } else { Some(&call.id) },
                        if call.name.is_empty() { None } else { Some(&call.name) }, &arguments);
                }
            }
            ModelEvent::ProviderData { target, data } => {
                data.validate()?;
                let slot = match target {
                    ProtocolTarget::Assistant => &mut self.provider_data,
                    ProtocolTarget::ToolCall { index } => {
                        if index >= limits.max_tools_per_step { return Err(AgentError::new(ErrorCode::Limit, "too many tool calls")); }
                        &mut self.calls.entry(index).or_default().provider_data
                    }
                };
                if slot.as_ref().is_some_and(|old| old.namespace != data.namespace) {
                    return Err(AgentError::new(ErrorCode::ModelProtocol, "provider-data namespace changed mid-stream"));
                }
                *slot = Some(data);
            }
            ModelEvent::Usage(usage) => self.usage = Some(usage),
            ModelEvent::Finish(reason) => {
                if self.finish.is_some_and(|old| old != reason) {
                    return Err(AgentError::new(ErrorCode::ModelProtocol, "conflicting finish reasons"));
                }
                self.finish = Some(reason);
            }
            ModelEvent::End => self.ended = true,
        }
        Ok(())
    }
    fn reply(&self) -> Result<ModelReply> {
        if self.calls.keys().copied().enumerate().any(|(expected, actual)| expected != actual) {
            return Err(AgentError::new(ErrorCode::ModelProtocol, "tool-call indexes must be contiguous"));
        }
        if !self.ended { return Err(AgentError::new(ErrorCode::ModelTransport, "stream ended without transport completion")); }
        let finish = self.finish.ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "missing finish reason"))?;
        if finish == FinishReason::Length { return Err(AgentError::new(ErrorCode::ModelTruncated, "model output was truncated")); }
        if finish == FinishReason::Filtered { return Err(AgentError::new(ErrorCode::ModelProtocol, "provider filtered the response")); }
        if (finish == FinishReason::ToolCalls) != !self.calls.is_empty() {
            return Err(AgentError::new(ErrorCode::ModelProtocol, "finish reason and tool calls disagree"));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut calls = vec![];
        for (expected, (index, call)) in self.calls.iter().enumerate() {
            if expected != *index || call.id.is_empty() || call.id.len() > 256 || !ids.insert(call.id.clone()) || !valid_name(&call.name) {
                return Err(AgentError::new(ErrorCode::ModelProtocol, "invalid tool-call index, id, or name"));
            }
            let arguments: serde_json::Value = serde_json::from_str(&call.arguments)
                .map_err(|_| AgentError::new(ErrorCode::ModelProtocol, "tool arguments are not complete JSON"))?;
            if !arguments.is_object() { return Err(AgentError::new(ErrorCode::ModelProtocol, "tool arguments must be an object")); }
            calls.push(ToolCall { id: call.id.clone(), name: call.name.clone(), arguments, provider_data: call.provider_data.clone() });
        }
        Ok(ModelReply { content: self.text.clone(), tool_calls: calls, provider_data: self.provider_data.clone(),
            reasoning_content: if self.reasoning.is_empty() { None } else { Some(self.reasoning.clone()) },
            finish, usage: self.usage })
    }
}
