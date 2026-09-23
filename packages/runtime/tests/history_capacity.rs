mod support;
use api::*;
use runtime::HostBuilder;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use support::*;
use tokio::sync::Notify;

fn size(messages: &[Message]) -> usize {
    serde_json::to_vec(messages).unwrap().len()
}
fn bounded(report: &RunReport, cap: usize) {
    assert!(
        size(&report.transcript) <= cap,
        "history: {} > {cap}",
        size(&report.transcript)
    );
    validate_messages(&report.transcript).unwrap();
}
#[derive(Default)]
struct Saved(Mutex<Vec<RunCheckpoint>>);
#[async_trait]
impl CheckpointSink for Saved {
    async fn commit(&self, cp: Arc<RunCheckpoint>, _: CancellationToken) -> Result<()> {
        self.0.lock().unwrap().push((*cp).clone());
        Ok(())
    }
}

#[tokio::test]
async fn initial_and_accumulated_caps_are_independent_and_empty_reply_room_is_checked_before_model()
{
    let model = ScriptModel::new(vec![]);
    let mut host = HostBuilder::new()
        .model(model.clone())
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("x".repeat(200));
    run.limits.max_history_bytes = size(&run.messages);
    let report = host.engine().execute(run.clone()).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    bounded(&report, run.limits.max_history_bytes);
    run.messages.push(Message::user("cannot fit"));
    assert!(host.engine().start(run).is_err());
    assert!(model.requests.lock().unwrap().is_empty());
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn model_text_reasoning_and_private_data_cannot_bypass_history_admission() {
    for extra in [
        ModelEvent::Text("\u{0}".repeat(300)),
        ModelEvent::Reasoning("\u{0}".repeat(300)),
        ModelEvent::ProviderData {
            target: ProtocolTarget::Assistant,
            data: ProviderData {
                namespace: "fixture".into(),
                value: json!({"opaque":"\u{0}".repeat(300)}),
            },
        },
    ] {
        let mut events = vec![extra];
        events.extend(answer("done"));
        let model = ScriptModel::new(vec![events]);
        let mut host = HostBuilder::new().model(model).build().await.unwrap();
        let mut run = RunRequest::new("confirmed");
        run.limits.max_history_bytes = 512;
        let report = host.engine().execute(run).await.unwrap();
        assert_eq!(report.status, RunStatus::Limited, "{:?}", report.error);
        assert_eq!(report.transcript, vec![Message::user("confirmed")]);
        assert_eq!(report.task_usage.model_calls, 1);
        bounded(&report, 512);
        host.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn tool_proposals_are_not_admitted_without_room_to_close_their_pairs() {
    let tool = Arc::new(CountTool::default());
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(vec![call("one")]))
        .tool(tool.clone())
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("confirmed");
    run.limits.max_history_bytes = 300;
    let report = host.engine().execute(run).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 0);
    assert_eq!(report.transcript, vec![Message::user("confirmed")]);
    bounded(&report, 300);
    host.shutdown().await.unwrap();
}

struct Recent;
#[async_trait]
impl ContextTransform for Recent {
    async fn transform(&self, _: &RunContext, history: Vec<Message>) -> Result<Vec<Message>> {
        let mut view = vec![history[0].clone()];
        if history.len() > 2 {
            view.extend_from_slice(&history[history.len() - 2..]);
        }
        Ok(view)
    }
}
struct Growing(AtomicUsize);
#[async_trait]
impl Model for Growing {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        assert!(serde_json::to_vec(&request).unwrap().len() <= 2048);
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        assert!(n < 50, "history must stop this loop, not the model");
        Ok(Box::pin(futures_util::stream::iter(
            call(&format!("call-{n}")).into_iter().map(Ok),
        )))
    }
}
#[tokio::test]
async fn small_projection_cannot_hide_accumulating_history_without_a_checkpoint_sink() {
    let tool = Arc::new(CountTool {
        payload: Some("\u{0}".repeat(128)),
        ..Default::default()
    });
    let mut host = HostBuilder::new()
        .model(Arc::new(Growing(AtomicUsize::new(0))))
        .tool(tool.clone())
        .context_transform(Arc::new(Recent))
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("confirmed");
    run.limits.max_history_bytes = 8192;
    run.limits.max_context_bytes = 2048;
    run.limits.max_tool_result_bytes = 256;
    run.limits.max_steps = 100;
    run.limits.audit_mode = AuditMode::Metadata;
    run.task = TaskControl::new(TaskLimits {
        max_model_calls: 100,
        ..Default::default()
    });
    let report = host.engine().execute(run).await.unwrap();
    assert_eq!(report.status, RunStatus::Limited, "{:?}", report.error);
    assert!(report
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("history capacity"));
    assert!(!report.checkpoint.configured);
    let executed = tool.probe.count.load(Ordering::SeqCst);
    assert!((4..50).contains(&executed));
    assert_eq!(
        results(&report)
            .iter()
            .filter(|r| r.status == ToolStatus::Success)
            .count(),
        executed
    );
    assert!(size(&report.transcript) > 2048);
    for result in results(&report) {
        if result.status == ToolStatus::Success {
            assert_eq!(result.content.text(), "\u{0}".repeat(128));
        }
    }
    bounded(&report, 8192);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn exhausted_batch_preserves_completed_results_and_emits_every_terminal_once() {
    for durable in [false, true] {
        let saved = Arc::new(Saved::default());
        let ids = (0..12).map(|i| format!("call-{i}")).collect::<Vec<_>>();
        let input = ids
            .iter()
            .map(|id| (id.as_str(), "count", json!({"value":1})))
            .collect::<Vec<_>>();
        let tool = Arc::new(CountTool {
            concurrency: ToolConcurrency::ParallelSafe,
            payload: Some("\u{0}".repeat(256)),
            delay: Duration::from_millis(2),
            ..Default::default()
        });
        let mut builder = HostBuilder::new()
            .model(ScriptModel::new(vec![calls(&input)]))
            .tool(tool.clone())
            .event_capacity(256);
        if durable {
            builder = builder.checkpoint_sink(saved.clone()).unwrap();
        }
        let mut host = builder.build().await.unwrap();
        let mut run = RunRequest::new("go");
        run.limits.max_history_bytes = 6500;
        run.limits.max_tool_result_bytes = 256;
        let mut handle = host.engine().start(run).unwrap();
        let report = handle.wait().await.unwrap();
        assert_eq!(report.status, RunStatus::Limited, "{:?}", report.error);
        let executed = tool.probe.count.load(Ordering::SeqCst);
        assert!((1..12).contains(&executed), "executed={executed}");
        let result = results(&report);
        assert_eq!(result.len(), 12);
        for (i, r) in result.iter().enumerate() {
            assert_eq!(r.call_id, ids[i]);
            if r.status == ToolStatus::Success {
                assert_eq!(r.content.text(), "\u{0}".repeat(256));
            } else {
                assert_eq!(r.status, ToolStatus::Skipped);
            }
        }
        assert_eq!(
            result
                .iter()
                .filter(|r| r.status == ToolStatus::Success)
                .count(),
            executed
        );
        let mut terminal = std::collections::BTreeSet::new();
        while let Ok(event) = handle.events.try_recv() {
            if let RunEvent::ItemCompleted { item } = event.event {
                if let ItemContent::ToolCall {
                    call_id: Some(id), ..
                } = item.content
                {
                    assert!(terminal.insert(id), "duplicate terminal");
                }
            }
        }
        assert_eq!(terminal.len(), ids.len());
        bounded(&report, 6500);
        if durable {
            let checkpoints = saved.0.lock().unwrap();
            assert!(checkpoints.iter().all(|cp| size(&cp.transcript) <= 6500));
            let cp = checkpoints.last().unwrap();
            assert_eq!(cp.phase, CheckpointPhase::RunFinished);
            assert_eq!(cp.transcript, report.transcript);
        }
        host.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn in_flight_reservations_drain_instead_of_rejecting_a_batch_that_can_fit() {
    let tool = Arc::new(CountTool {
        concurrency: ToolConcurrency::ParallelSafe,
        delay: Duration::from_millis(2),
        ..Default::default()
    });
    let model = ScriptModel::new(vec![
        calls(&[
            ("a", "count", json!({"value":1})),
            ("b", "count", json!({"value":2})),
            ("c", "count", json!({"value":3})),
        ]),
        answer("done"),
    ]);
    let mut host = HostBuilder::new()
        .model(model)
        .tool(tool.clone())
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("go");
    run.limits.max_history_bytes = 3000;
    run.limits.max_tool_result_bytes = 256;
    let report = host.engine().execute(run).await.unwrap();
    assert_eq!(report.status, RunStatus::Completed, "{:?}", report.error);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 3);
    assert_eq!(tool.probe.peak.load(Ordering::SeqCst), 1);
    bounded(&report, 3000);
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_a_history_throttled_batch_closes_all_pairs_without_starting_the_queue() {
    let tool = Arc::new(CountTool {
        concurrency: ToolConcurrency::ParallelSafe,
        wait_for_cancel: true,
        ..Default::default()
    });
    let mut host = HostBuilder::new()
        .model(ScriptModel::new(vec![calls(&[
            ("a", "count", json!({"value":1})),
            ("b", "count", json!({"value":2})),
            ("c", "count", json!({"value":3})),
        ])]))
        .tool(tool.clone())
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("go");
    run.limits.max_history_bytes = 3000;
    run.limits.max_tool_result_bytes = 256;
    let handle = host.engine().start(run).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tool.started.notified())
        .await
        .unwrap();
    handle.cancel();
    let report = tokio::time::timeout(Duration::from_secs(2), handle.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.status, RunStatus::Cancelled);
    assert_eq!(tool.probe.count.load(Ordering::SeqCst), 1);
    assert_eq!(results(&report).len(), 3);
    assert_eq!(results(&report)[0].status, ToolStatus::Unknown);
    assert!(results(&report)[1..]
        .iter()
        .all(|r| r.status == ToolStatus::Skipped));
    bounded(&report, 3000);
    host.shutdown().await.unwrap();
}

struct Paused {
    started: Notify,
    release: Notify,
}
#[async_trait]
impl Model for Paused {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(Box::pin(futures_util::stream::iter(
            answer("done").into_iter().map(Ok),
        )))
    }
}
#[tokio::test]
async fn injected_input_is_rejected_before_checkpoint_or_ack_when_history_is_full() {
    let model = Arc::new(Paused {
        started: Notify::new(),
        release: Notify::new(),
    });
    let saved = Arc::new(Saved::default());
    let mut host = HostBuilder::new()
        .model(model.clone())
        .checkpoint_sink(saved.clone())
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut run = RunRequest::new("go");
    run.limits.max_history_bytes = 1024;
    let handle = host.engine().start(run).unwrap();
    model.started.notified().await;
    let input = handle.steer("x".repeat(3000));
    tokio::pin!(input);
    assert!(futures_util::poll!(input.as_mut()).is_pending());
    model.release.notify_one();
    let report = handle.wait().await.unwrap();
    assert_eq!(input.await.unwrap_err().code, ErrorCode::Limit);
    assert_eq!(report.status, RunStatus::Limited);
    assert_eq!(report.transcript.len(), 2);
    assert!(!saved
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|cp| cp.phase == CheckpointPhase::InputApplied));
    bounded(&report, 1024);
    host.shutdown().await.unwrap();
}
