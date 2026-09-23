#![cfg(feature = "sessions")]
use api::*;
use application::sessions::{SessionApplication, TurnRequest};
use application::{AgentApplication, ApplicationConfig, ApplicationErrorCode};
use sessions::{SessionSink, Store};
use std::{sync::Arc, time::Duration};

struct NoCalls;
#[async_trait]
impl Model for NoCalls {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        panic!("rejected admission must not call a model")
    }
}
#[tokio::test]
async fn application_reserves_current_system_prompt_before_durable_turn_admission() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("state"), temp.path()).unwrap();
    let header = store.create("bounded").unwrap();
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(NoCalls))
        .checkpoint_sink(Arc::new(SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut config = ApplicationConfig::default();
    config.run_limits.max_context_bytes = 256;
    config.run_limits.max_initial_history_bytes = 512;
    config.system_prompt = Some("s".repeat(400));
    let app = AgentApplication::new(
        sessions::runtime(Arc::new(host.engine()), store.clone()),
        config,
    )
    .unwrap();
    let service = SessionApplication::new(store.clone(), app.clone());
    let result = service
        .start_turn(
            header.id.clone(),
            TurnRequest {
                request_id: "one".into(),
                revision: header.revision,
                prompt: "p".repeat(150),
            },
        )
        .await;
    assert_eq!(result.err().unwrap().code, ApplicationErrorCode::Capacity);
    let doc = store.get(&header.id).unwrap();
    assert_eq!(doc.header.revision, header.revision);
    assert!(doc.body.turns.is_empty());
    app.shutdown(Duration::from_secs(1)).await.unwrap();
    host.shutdown().await.unwrap();
}

struct Complete(std::sync::atomic::AtomicUsize);
#[async_trait]
impl Model for Complete {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text("confirmed result".into())),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}
async fn saved_host(store: Arc<Store>, model: Arc<Complete>) -> runtime::Host {
    runtime::HostBuilder::new()
        .model(model)
        .checkpoint_sink(Arc::new(SessionSink(store)))
        .unwrap()
        .build()
        .await
        .unwrap()
}
async fn wait_saved(store: &Store, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.get(id).unwrap().header.status == sessions::Status::Running {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the accepted turn must settle durably");
    assert_eq!(
        store.get(id).unwrap().header.status,
        sessions::Status::Completed
    );
}
#[tokio::test]
async fn concurrent_duplicate_turns_execute_once_and_conflicting_input_preserves_history() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("state"), temp.path()).unwrap();
    let header = store.create("dedup").unwrap();
    let model = Arc::new(Complete(AtomicUsize::new(0)));
    let mut host = saved_host(store.clone(), model.clone()).await;
    let app = AgentApplication::new(
        sessions::runtime(Arc::new(host.engine()), store.clone()),
        ApplicationConfig::default(),
    )
    .unwrap();
    let service = SessionApplication::new(store.clone(), app.clone());
    let request = TurnRequest {
        request_id: "one".into(),
        revision: header.revision,
        prompt: "once".into(),
    };
    let (a, b) = tokio::join!(
        service.start_turn(header.id.clone(), request.clone()),
        service.start_turn(header.id.clone(), request.clone()),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.run_id, b.run_id);
    assert_ne!(a.reused, b.reused);
    wait_saved(&store, &header.id).await;
    let before = store.get(&header.id).unwrap();
    let history = before.history().unwrap();
    let mut conflict = request.clone();
    conflict.prompt = "different input under the same ID".into();
    assert_eq!(
        service
            .start_turn(header.id.clone(), conflict)
            .await
            .err()
            .unwrap()
            .code,
        ApplicationErrorCode::Conflict
    );
    let after = store.get(&header.id).unwrap();
    assert_eq!(before.header.revision, after.header.revision);
    assert_eq!(history, after.history().unwrap());
    assert_eq!(after.body.turns.len(), 1);
    assert_eq!(model.0.load(Ordering::SeqCst), 1);
    app.shutdown(Duration::from_secs(1)).await.unwrap();
    host.shutdown().await.unwrap();
}

// Block only the test wrapper's synchronous start. The caller can disconnect while
// the session adapter owns the already-accepted admission, before it receives a Run ID.
struct StartGate {
    inner: Arc<dyn AgentRuntime>,
    entered: tokio::sync::Notify,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}
#[async_trait]
impl AgentExecutor for StartGate {
    async fn execute(&self, request: RunRequest) -> api::Result<Arc<RunReport>> {
        self.start(request)?.wait_owned().await
    }
}
impl AgentRuntime for StartGate {
    fn start(&self, request: RunRequest) -> api::Result<RunHandle> {
        self.entered.notify_one();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("test must release the gated admission");
        self.inner.start(request)
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnected_start_caller_does_not_abandon_durable_admission_or_replay_on_retry() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("state"), temp.path()).unwrap();
    let header = store.create("disconnect").unwrap();
    let model = Arc::new(Complete(AtomicUsize::new(0)));
    let mut host = saved_host(store.clone(), model.clone()).await;
    let (release, receiver) = std::sync::mpsc::channel();
    let gate = Arc::new(StartGate {
        inner: sessions::runtime(Arc::new(host.engine()), store.clone()),
        entered: tokio::sync::Notify::new(),
        release: std::sync::Mutex::new(receiver),
    });
    let app = AgentApplication::new(gate.clone(), ApplicationConfig::default()).unwrap();
    let service = SessionApplication::new(store.clone(), app.clone());
    let request = TurnRequest {
        request_id: "one".into(),
        revision: header.revision,
        prompt: "accepted input".into(),
    };
    let task_service = service.clone();
    let id = header.id.clone();
    let task_request = request.clone();
    let caller = tokio::spawn(async move { task_service.start_turn(id, task_request).await });
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    // prepare has saved the input, but start has not yet returned to its owning adapter.
    let admitted = store.get(&header.id).unwrap();
    assert_eq!(admitted.body.turns.len(), 1);
    assert!(admitted.body.turns[0].run_id.is_none());
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    release.send(()).unwrap();
    wait_saved(&store, &header.id).await;
    let retry = service
        .start_turn(header.id.clone(), request)
        .await
        .unwrap();
    let saved = store.get(&header.id).unwrap();
    assert!(retry.reused);
    assert_eq!(retry.run_id, saved.body.turns[0].run_id);
    assert_eq!(
        saved.history().unwrap().last().unwrap().text(),
        "confirmed result"
    );
    assert_eq!(saved.body.turns.len(), 1);
    assert_eq!(model.0.load(Ordering::SeqCst), 1);
    app.shutdown(Duration::from_secs(1)).await.unwrap();
    host.shutdown().await.unwrap();
}
