use api::*;
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::Duration;
struct Fixture;
#[async_trait]
impl Model for Fixture {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let events = if request.messages.iter().any(|m| matches!(m,Message::Tool { .. })) {
            vec![ModelEvent::Text("done".into()), ModelEvent::Finish(FinishReason::Stop), ModelEvent::End]
        } else { vec![ModelEvent::ToolDelta {index:0,id:Some("one".into()),name:Some("effect".into()),arguments:"{}".into()},
            ModelEvent::Finish(FinishReason::ToolCalls),ModelEvent::End] };
        Ok(Box::pin(futures_util::stream::iter(events.into_iter().map(Ok))))
    }
}
struct Effect(Arc<AtomicUsize>);
#[async_trait]
impl Tool for Effect {
    fn spec(&self) -> ToolSpec { ToolSpec {name:"effect".into(),description:"count".into(),
        parameters:serde_json::json!({"type":"object"}),side_effects:true,concurrency:ToolConcurrency::Exclusive} }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        self.0.fetch_add(1,Ordering::SeqCst); Ok("executed".into())
    }
}
struct Policy { allowed:Arc<AtomicBool>, checks:Arc<AtomicUsize> }
#[async_trait]
impl ToolPolicy for Policy {
    async fn check(&self, _: &RunContext, _: &ToolCall, _: &ToolSpec) -> Result<PolicyDecision> {
        self.checks.fetch_add(1,Ordering::SeqCst);
        Ok(if self.allowed.load(Ordering::SeqCst) {PolicyDecision::Allow} else {PolicyDecision::Deny("revoked during intent persistence".into())})
    }
}
struct SlowIntent(Arc<AtomicBool>);
#[async_trait]
impl CheckpointSink for SlowIntent {
    async fn commit(&self, cp:Arc<RunCheckpoint>, _:CancellationToken) -> Result<()> {
        if matches!(cp.phase,CheckpointPhase::ToolIntent { .. }) {
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.0.store(false,Ordering::SeqCst);
        }
        Ok(())
    }
}
#[tokio::test]
async fn policy_is_checked_once_after_intent_ack_not_before_slow_persistence() {
    let allowed=Arc::new(AtomicBool::new(true)); let checks=Arc::new(AtomicUsize::new(0)); let effects=Arc::new(AtomicUsize::new(0));
    let mut host=runtime::HostBuilder::new().model(Arc::new(Fixture)).tool(Arc::new(Effect(effects.clone())))
        .allow_side_effect_tool("effect").policy(Arc::new(Policy {allowed:allowed.clone(), checks:checks.clone()}))
        .checkpoint_sink(Arc::new(SlowIntent(allowed))).unwrap().build().await.unwrap();
    let report=host.engine().execute(RunRequest::new("go")).await.unwrap();
    assert_eq!(report.status,RunStatus::Completed); assert_eq!(effects.load(Ordering::SeqCst),0); assert_eq!(checks.load(Ordering::SeqCst),1);
    assert!(report.transcript.iter().any(|m| matches!(m,Message::Tool {result} if result.status==ToolStatus::Denied)));
    host.shutdown().await.unwrap();
}
