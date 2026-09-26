//! Deterministic CPU/payload probe, not an RSS or storage/fsync benchmark.
//! Run explicitly: cargo test -p runtime --test long_task_cost -- --ignored --nocapture
use api::*;
use runtime::HostBuilder;
use serde_json::{json, Value};
use std::{hint::black_box, sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}}, time::Instant};

const ROUNDS: usize = 64;
const RESULT_BYTES: usize = 4096;
const AUDIT_REPEATS: usize = 5;

#[derive(Default)]
struct Costs {
    history_clone_ns: AtomicU64,
    history_messages: AtomicU64,
    checkpoint_clone_ns: AtomicU64,
    checkpoint_serialize_ns: AtomicU64,
    checkpoint_bytes: AtomicU64,
    checkpoints: AtomicU64,
}
struct CostModel { calls: AtomicUsize, costs: Arc<Costs> }
#[async_trait]
impl Model for CostModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        // A probe of cloning the exact history seen by this call. This is extra
        // measurement work, not instrumentation of every internal Runtime copy.
        let start = Instant::now();
        black_box(request.messages.clone());
        self.costs.history_clone_ns.fetch_add(start.elapsed().as_nanos() as u64, Ordering::SeqCst);
        self.costs.history_messages.fetch_add(request.messages.len() as u64, Ordering::SeqCst);
        let round = self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(round <= ROUNDS);
        let events = if round < ROUNDS {
            vec![
                ModelEvent::ToolDelta { index: 0, id: Some(format!("call-{round}")), name: Some("work".into()),
                    arguments: json!({"round":round}).to_string() },
                ModelEvent::Finish(FinishReason::ToolCalls),
                ModelEvent::Usage(Usage { input_tokens: 100, output_tokens: 1, cache_read_tokens: None, cache_write_tokens: None }), ModelEvent::End,
            ]
        } else {
            vec![ModelEvent::Text("finished".into()), ModelEvent::Finish(FinishReason::Stop),
                ModelEvent::Usage(Usage { input_tokens: 100, output_tokens: 1, cache_read_tokens: None, cache_write_tokens: None }), ModelEvent::End]
        };
        Ok(Box::pin(futures_util::stream::iter(events.into_iter().map(Ok))))
    }
}
struct Work(AtomicUsize);
#[async_trait]
impl Tool for Work {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "work".into(), description: "Fixed payload cost probe".into(),
            parameters: json!({"type":"object","required":["round"],"properties":{"round":{"type":"integer"}},"additionalProperties":false}),
            concurrency: ToolConcurrency::Exclusive, side_effects: false }
    }
    async fn execute(&self, _: ToolContext, _: Value) -> Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("x".repeat(RESULT_BYTES).into())
    }
}
struct MeasuringSink(Arc<Costs>);
#[async_trait]
impl CheckpointSink for MeasuringSink {
    async fn commit(&self, checkpoint: Arc<RunCheckpoint>, cancel: CancellationToken) -> Result<()> {
        assert!(!cancel.is_cancelled());
        let count = self.0.checkpoints.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(checkpoint.revision, count);
        let start = Instant::now();
        black_box(checkpoint.as_ref().clone());
        self.0.checkpoint_clone_ns.fetch_add(start.elapsed().as_nanos() as u64, Ordering::SeqCst);
        let start = Instant::now();
        let bytes = serde_json::to_vec(checkpoint.as_ref()).unwrap();
        self.0.checkpoint_serialize_ns.fetch_add(start.elapsed().as_nanos() as u64, Ordering::SeqCst);
        self.0.checkpoint_bytes.fetch_add(bytes.len() as u64, Ordering::SeqCst);
        // Deliberately non-retaining, awaited sink. No claim of disk durability.
        Ok(())
    }
}
fn repeated_us(mut operation: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..AUDIT_REPEATS { operation(); }
    start.elapsed().as_secs_f64() * 1_000_000.0 / AUDIT_REPEATS as f64
}
fn micros(value: &AtomicU64) -> f64 { value.load(Ordering::SeqCst) as f64 / 1000.0 }

#[tokio::test]
#[ignore = "explicit deterministic long-task cost probe"]
async fn fixed_long_task_audit_history_and_checkpoint_costs() {
    let mut baseline = None;
    for checkpoints in [false, true] {
        for mode in [AuditMode::Full, AuditMode::Metadata] {
            let costs = Arc::new(Costs::default());
            let model = Arc::new(CostModel { calls: AtomicUsize::new(0), costs: costs.clone() });
            let work = Arc::new(Work(AtomicUsize::new(0)));
            let mut builder = HostBuilder::new().model(model.clone()).tool(work.clone());
            if checkpoints { builder = builder.checkpoint_sink(Arc::new(MeasuringSink(costs.clone()))).unwrap(); }
            let mut host = builder.build().await.unwrap();
            let mut request = RunRequest::new("p".repeat(32 * 1024));
            request.limits.max_steps = ROUNDS + 1;
            request.limits.max_context_bytes = 512 * 1024;
            request.limits.max_audit_bytes = 64 * 1024 * 1024;
            request.limits.audit_mode = mode;
            request.task = TaskControl::new(TaskLimits { max_model_calls: (ROUNDS + 1) as u64, ..Default::default() });
            let start = Instant::now();
            let report = host.engine().execute(request).await.unwrap();
            let run_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
            assert_eq!(report.output.as_deref(), Some("finished"));
            assert_eq!(report.task_usage.model_calls, (ROUNDS + 1) as u64);
            assert_eq!(work.0.load(Ordering::SeqCst), ROUNDS);
            assert_eq!(model.calls.load(Ordering::SeqCst), ROUNDS + 1);
            assert_eq!(report.model_requests.len(), ROUNDS + 1);
            if let Some(previous) = &baseline { assert_eq!(&report.transcript, previous); }
            else { baseline = Some(report.transcript.clone()); }
            assert!(report.model_requests.iter().all(|record| record.request.is_some() == (mode == AuditMode::Full)));
            let audit_json_bytes = serde_json::to_vec(&report.model_requests).unwrap().len();
            let audit_clone_us = repeated_us(|| { black_box(report.model_requests.clone()); });
            let audit_serialize_us = repeated_us(|| { black_box(serde_json::to_vec(&report.model_requests).unwrap()); });
            let checkpoint_count = costs.checkpoints.load(Ordering::SeqCst);
            assert_eq!(checkpoint_count, if checkpoints { (ROUNDS * 5 + 3) as u64 } else { 0 });
            if checkpoints { assert_eq!(report.checkpoint.last_acknowledged_revision, Some(checkpoint_count)); }
            println!("COST_PROBE {}", json!({
                "mode": if mode == AuditMode::Full { "full" } else { "metadata" },
                "checkpoints": checkpoints, "rounds": ROUNDS, "result_bytes": RESULT_BYTES,
                "model_calls": report.task_usage.model_calls,
                "audit_json_bytes": audit_json_bytes,
                "audit_clone_us_per_copy": audit_clone_us,
                "audit_serialize_us_per_copy": audit_serialize_us,
                "history_clone_us_total": micros(&costs.history_clone_ns),
                "history_cloned_messages": costs.history_messages.load(Ordering::SeqCst),
                "checkpoint_count": checkpoint_count,
                "checkpoint_serialized_bytes_total": costs.checkpoint_bytes.load(Ordering::SeqCst),
                "checkpoint_clone_us_total": micros(&costs.checkpoint_clone_ns),
                "checkpoint_serialize_us_total": micros(&costs.checkpoint_serialize_ns),
                "instrumented_run_wall_ms": run_wall_ms,
            }));
            host.shutdown().await.unwrap();
        }
    }
}
