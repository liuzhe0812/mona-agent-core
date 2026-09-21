#![allow(dead_code)]
use api::*;
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}, time::Duration};
use tokio::sync::Notify;

pub struct ScriptModel {
    pub replies: Mutex<VecDeque<Vec<ModelEvent>>>,
    pub requests: Mutex<Vec<ModelRequest>>,
}
impl ScriptModel {
    pub fn new(replies: Vec<Vec<ModelEvent>>) -> Arc<Self> {
        Arc::new(Self { replies: Mutex::new(replies.into()), requests: Mutex::new(vec![]) })
    }
}
#[async_trait]
impl Model for ScriptModel {
    async fn stream(&self, request: ModelRequest, _cancel: CancellationToken) -> Result<ModelStream> {
        self.requests.lock().unwrap().push(request);
        let events = self.replies.lock().unwrap().pop_front()
            .ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "test script exhausted"))?;
        Ok(Box::pin(futures_util::stream::iter(events.into_iter().map(Ok))))
    }
}
pub fn answer(text: &str) -> Vec<ModelEvent> {
    vec![ModelEvent::Text(text.into()), ModelEvent::Finish(FinishReason::Stop),
        ModelEvent::Usage(Usage { input_tokens: 10, output_tokens: 5 }), ModelEvent::End]
}
pub fn calls(items: &[(&str, &str, Value)]) -> Vec<ModelEvent> {
    let mut events = items.iter().enumerate().map(|(index, (id, name, args))| ModelEvent::ToolDelta {
        index, id: Some((*id).into()), name: Some((*name).into()), arguments: args.to_string(),
    }).collect::<Vec<_>>();
    events.extend([ModelEvent::Finish(FinishReason::ToolCalls), ModelEvent::Usage(Usage { input_tokens: 10, output_tokens: 5 }), ModelEvent::End]);
    events
}
pub fn call(id: &str) -> Vec<ModelEvent> { calls(&[(id, "count", json!({"value":1}))]) }

#[derive(Default)]
pub struct Probe {
    pub count: AtomicUsize,
    pub active: AtomicUsize,
    pub peak: AtomicUsize,
    pub exclusive_overlap: AtomicUsize,
    pub log: Mutex<Vec<String>>,
}
struct Active(Arc<Probe>);
impl Drop for Active { fn drop(&mut self) { self.0.active.fetch_sub(1, Ordering::SeqCst); } }

pub struct CountTool {
    pub name: &'static str,
    pub effects: bool,
    pub concurrency: ToolConcurrency,
    pub probe: Arc<Probe>,
    pub started: Arc<Notify>,
    pub release: Option<Arc<Notify>>,
    pub wait_for_cancel: bool,
    pub delay: Duration,
    pub payload: Option<String>,
    pub panic: bool,
}
impl Default for CountTool {
    fn default() -> Self {
        Self { name: "count", effects: false, concurrency: ToolConcurrency::Exclusive,
            probe: Arc::new(Probe::default()), started: Arc::new(Notify::new()), release: None,
            wait_for_cancel: false, delay: Duration::ZERO, payload: None, panic: false }
    }
}
#[async_trait]
impl Tool for CountTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.name.into(), description: "A local test tool".into(),
            parameters: json!({"type":"object","properties":{"value":{"type":"integer"}},"required":["value"],"additionalProperties":false}),
            concurrency: self.concurrency, side_effects: self.effects }
    }
    async fn execute(&self, ctx: ToolContext, args: Value) -> Result<ToolOutput> {
        self.probe.count.fetch_add(1, Ordering::SeqCst);
        let active = self.probe.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.probe.peak.fetch_max(active, Ordering::SeqCst);
        let _active = Active(self.probe.clone());
        if self.concurrency == ToolConcurrency::Exclusive && active > 1 {
            self.probe.exclusive_overlap.fetch_add(1, Ordering::SeqCst);
        }
        self.probe.log.lock().unwrap().push(format!("{}:{}", self.name, args["value"]));
        self.started.notify_one();
        ctx.progress.report("started");
        assert!(!self.panic, "deliberate tool panic");
        if self.wait_for_cancel {
            ctx.run.cancel.cancelled().await;
            return Err(AgentError::new(ErrorCode::Cancelled, "test tool cooperatively stopped"));
        }
        if let Some(release) = &self.release {
            tokio::select! {
                _ = release.notified() => {},
                _ = ctx.run.cancel.cancelled() => return Err(AgentError::new(ErrorCode::Cancelled, "test tool cancelled")),
            }
        }
        tokio::time::sleep(self.delay).await;
        Ok(self.payload.clone().unwrap_or_else(|| args["value"].to_string()).into())
    }
}
pub fn results(report: &RunReport) -> Vec<&ToolResult> {
    report.transcript.iter().filter_map(|message| match message { Message::Tool { result } => Some(result), _ => None }).collect()
}
