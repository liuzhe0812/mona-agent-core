use api::*;
use runtime::Engine;
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::{Arc, Mutex}};

pub struct ScriptedModel { replies: Mutex<VecDeque<Vec<ModelEvent>>> }
impl ScriptedModel { pub fn new(replies: Vec<Vec<ModelEvent>>) -> Self { Self { replies: Mutex::new(replies.into()) } } }
#[async_trait]
impl Model for ScriptedModel {
    async fn stream(&self, _request: ModelRequest, _cancel: CancellationToken) -> Result<ModelStream> {
        let reply = self.replies.lock().unwrap().pop_front().ok_or_else(|| AgentError::new(ErrorCode::ModelProtocol, "demo model script exhausted"))?;
        Ok(Box::pin(futures_util::stream::iter(reply.into_iter().map(Ok))))
    }
}
pub fn text(text: &str) -> Vec<ModelEvent> {
    vec![ModelEvent::Text(text.into()), ModelEvent::Finish(FinishReason::Stop),
        ModelEvent::Usage(Usage { input_tokens: 20, output_tokens: 10, cache_read_tokens: None, cache_write_tokens: None }), ModelEvent::End]
}
pub fn call(id: &str, name: &str, arguments: Value) -> Vec<ModelEvent> {
    vec![ModelEvent::ToolDelta { index: 0, id: Some(id.into()), name: Some(name.into()), arguments: arguments.to_string() },
        ModelEvent::Finish(FinishReason::ToolCalls), ModelEvent::Usage(Usage { input_tokens: 20, output_tokens: 10, cache_read_tokens: None, cache_write_tokens: None }), ModelEvent::End]
}
pub struct Add;
#[async_trait]
impl Tool for Add {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "add".into(), description: "Add two signed 64-bit integers.".into(),
            parameters: json!({"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"],"additionalProperties":false}),
            concurrency: ToolConcurrency::ParallelSafe, side_effects: false }
    }
    async fn execute(&self, ctx: ToolContext, args: Value) -> Result<ToolOutput> {
        ctx.progress.report("adding");
        let a = args["a"].as_i64().ok_or_else(|| AgentError::new(ErrorCode::Tool, "a out of i64 range"))?;
        let b = args["b"].as_i64().ok_or_else(|| AgentError::new(ErrorCode::Tool, "b out of i64 range"))?;
        let result = a.checked_add(b).ok_or_else(|| AgentError::new(ErrorCode::Tool, "integer overflow"))?;
        Ok(result.to_string().into())
    }
}
pub async fn display_run(engine: &Engine, request: RunRequest) -> Result<Arc<RunReport>> {
    let mut handle = engine.start(request)?;
    let replacement = handle.subscribe();
    let mut events = std::mem::replace(&mut handle.events, replacement);
    let completion = handle.wait();
    tokio::pin!(completion);
    let report = loop {
        tokio::select! {
            report = &mut completion => break report?,
            event = events.recv() => match event {
                Ok(event) => println!("{} {:?}", event.seq, event.event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => eprintln!("missed {n} telemetry events; final report remains authoritative"),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break completion.await?,
            }
        }
    };
    println!("status={:?}; final={:?}; reported_tokens={}", report.status, report.output, report.task_usage.reported_tokens);
    Ok(report)
}
