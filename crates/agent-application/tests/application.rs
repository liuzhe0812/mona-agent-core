use agent_api::*;
use agent_application::*;
use agent_core::{Host, HostBuilder};
use agent_demo::{ScriptedModel, text};
use std::{sync::Arc, time::Duration};

async fn fixture(config: ApplicationConfig, events: Vec<ModelEvent>) -> (Host, AgentApplication) {
    let host = HostBuilder::new().model(Arc::new(ScriptedModel::new(vec![events]))).event_capacity(4096).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), config).unwrap(); (host, app)
}
fn request(key: &str) -> StartRequest { StartRequest { request_id: key.into(), prompt: "hello".into() } }
async fn drain(app: &AgentApplication, id: &str, after: Option<u64>) -> Vec<StreamFrame> {
    let mut stream = app.subscribe_events(id, after).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut frames = vec![]; while let Some(frame) = stream.next().await { frames.push(frame); } frames
    }).await.unwrap()
}
#[tokio::test]
async fn start_is_idempotent_and_rejects_conflicting_body() {
    let (mut host, app) = fixture(Default::default(), text("ok")).await;
    let first = app.start_task(request("same")).unwrap(); let second = app.start_task(request("same")).unwrap();
    assert_eq!(first.run_id, second.run_id); assert!(second.reused);
    let bad = app.start_task(StartRequest { request_id: "same".into(), prompt:"different".into() }).err().unwrap();
    assert_eq!(bad.code, ApplicationErrorCode::Conflict);
    drain(&app, &first.run_id, None).await; host.shutdown().await.unwrap();
}
#[tokio::test]
async fn both_replay_and_snapshot_end_in_same_outcome() {
    let (mut host, app) = fixture(Default::default(), text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id;
    drain(&app, &id, Some(0)).await;
    let snapshot = app.get_snapshot(&id).unwrap();
    assert_eq!(snapshot.outcome.as_ref().unwrap().output.as_deref(), Some("ok"));
    let frames = drain(&app, &id, Some(snapshot.seq)).await; assert!(frames.is_empty());
    let initial = drain(&app, &id, None).await;
    assert!(matches!(initial.first(), Some(StreamFrame::Snapshot { .. })));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn stale_cursor_gets_snapshot_not_incomplete_deltas() {
    let config = ApplicationConfig { journal_events: 2, ..Default::default() };
    let (mut host, app) = fixture(config, text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id; drain(&app, &id, None).await;
    let frames = drain(&app, &id, Some(0)).await;
    assert!(matches!(frames.first(), Some(StreamFrame::Snapshot { reason: SnapshotReason::CursorExpired, .. })));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn future_cursor_and_cross_run_lookup_are_rejected() {
    let (mut host, app) = fixture(Default::default(), text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id;
    assert!(app.subscribe_events(&id, Some(u64::MAX)).is_err());
    assert_eq!(app.get_snapshot("other").err().unwrap().code, ApplicationErrorCode::NotFound);
    drain(&app, &id, None).await; host.shutdown().await.unwrap();
}
#[tokio::test]
async fn dropping_subscription_does_not_cancel_run() {
    let (mut host, app) = fixture(Default::default(), text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id;
    drop(app.subscribe_events(&id, None).unwrap());
    drain(&app, &id, None).await;
    assert_eq!(app.get_result(&id).unwrap().outcome.unwrap().status, RunStatus::Completed);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn subscriptions_and_retained_run_registry_are_bounded() {
    let config = ApplicationConfig { max_retained_runs: 1, max_subscriptions: 1, ..Default::default() };
    let (mut host, app) = fixture(config, text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id;
    assert_eq!(app.start_task(request("two")).err().unwrap().code, ApplicationErrorCode::Capacity);
    let subscription = app.subscribe_events(&id, None).unwrap();
    assert!(app.subscribe_events(&id, None).is_err()); drop(subscription);
    drain(&app, &id, None).await; app.forget(&id).unwrap();
    assert!(app.get_snapshot(&id).is_err()); host.shutdown().await.unwrap();
}
#[tokio::test]
async fn byte_cap_forces_resync_instead_of_silent_event_loss() {
    let config = ApplicationConfig { journal_bytes: 32, ..Default::default() };
    let (mut host, app) = fixture(config, text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id;
    drain(&app, &id, None).await;
    let frames = drain(&app, &id, Some(0)).await;
    assert!(matches!(frames.first(), Some(StreamFrame::Snapshot { reason: SnapshotReason::SourceResync, .. })));
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn shutdown_closes_admission_without_exposing_host_controls() {
    let (mut host, app) = fixture(Default::default(), text("ok")).await;
    app.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(app.start_task(request("one")).err().unwrap().code, ApplicationErrorCode::Closed);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn public_snapshot_omits_system_prompt_and_request_audits() {
    let config = ApplicationConfig { system_prompt: Some("secret-host-instructions".into()), ..Default::default() };
    let (mut host, app) = fixture(config, text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id; drain(&app, &id, None).await;
    let encoded = serde_json::to_string(&app.get_snapshot(&id).unwrap()).unwrap();
    assert!(!encoded.contains("secret-host-instructions")); assert!(!encoded.contains("model_requests"));
    host.shutdown().await.unwrap();
}

struct WaitingModel { started: tokio::sync::Notify }
#[async_trait]
impl Model for WaitingModel {
    async fn stream(&self, _request: ModelRequest, _cancel: CancellationToken) -> Result<ModelStream> {
        self.started.notify_one(); Ok(Box::pin(futures_util::stream::pending()))
    }
}
#[tokio::test]
async fn cancel_is_a_signal_and_eventually_produces_a_terminal_outcome() {
    let model = Arc::new(WaitingModel { started: tokio::sync::Notify::new() });
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let id = app.start_task(request("cancel")).unwrap().run_id;
    tokio::time::timeout(Duration::from_secs(1), model.started.notified()).await.unwrap();
    assert!(app.cancel_task(&id).unwrap().signalled);
    drain(&app, &id, None).await;
    assert_eq!(app.get_result(&id).unwrap().outcome.unwrap().status, RunStatus::Cancelled);
    assert!(!app.cancel_task(&id).unwrap().signalled);
    host.shutdown().await.unwrap();
}
struct InputModel {
    requests: std::sync::Mutex<Vec<ModelRequest>>,
    started: tokio::sync::Notify, release: tokio::sync::Notify,
}
#[async_trait]
impl Model for InputModel {
    async fn stream(&self, request: ModelRequest, _cancel: CancellationToken) -> Result<ModelStream> {
        let first = { let mut requests = self.requests.lock().unwrap(); requests.push(request); requests.len() == 1 };
        if first { self.started.notify_one(); self.release.notified().await; }
        Ok(Box::pin(futures_util::stream::iter(text(if first { "intermediate" } else { "done" }).into_iter().map(Ok))))
    }
}
#[tokio::test]
async fn duplicate_input_request_is_applied_once_at_a_safe_boundary() {
    let model = Arc::new(InputModel { requests: Default::default(), started: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
    let mut host = HostBuilder::new().model(model.clone()).build().await.unwrap();
    let app = AgentApplication::new(Arc::new(host.engine()), Default::default()).unwrap();
    let id = app.start_task(request("input")).unwrap().run_id;
    tokio::time::timeout(Duration::from_secs(1), model.started.notified()).await.unwrap();
    let input = InputRequest { request_id: "input-key".into(), text: "additional instruction".into() };
    let (one, two, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(app.send_input(&id, input.clone()), app.send_input(&id, input), async {
            tokio::time::sleep(Duration::from_millis(20)).await; model.release.notify_one();
        })
    }).await.unwrap();
    assert!(one.unwrap().applied && two.unwrap().applied);
    drain(&app, &id, None).await;
    {
        let requests = model.requests.lock().unwrap(); assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].messages.iter().filter(|m| matches!(m, Message::User { content } if content.text() == "additional instruction")).count(), 1);
    }
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn trusted_model_defaults_are_passed_without_widening_bridge_requests() {
    let config = ApplicationConfig { model_options: ModelOptions { temperature: Some(0.2), ..Default::default() },
        allowed_tools: Some(std::collections::BTreeSet::new()), ..Default::default() };
    let (mut host, app) = fixture(config, text("ok")).await;
    let id = app.start_task(request("one")).unwrap().run_id;
    drain(&app, &id, None).await;
    // The UI protocol remains input-only; secrets/policies/options are host configuration.
    assert!(serde_json::from_value::<StartRequest>(serde_json::json!({"request_id":"bad","prompt":"go","model_options":{"temperature":1}})).is_err());
    assert_eq!(app.get_result(&id).unwrap().outcome.unwrap().status, RunStatus::Completed);
    app.shutdown(Duration::from_secs(1)).await.unwrap(); host.shutdown().await.unwrap();
}
