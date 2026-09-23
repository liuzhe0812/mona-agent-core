use api::*;
use sessions::{Prepared, SessionErrorCode, SessionSink, Store};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Default)]
struct Echo {
    requests: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl Model for Echo {
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> api::Result<ModelStream> {
        let text = request
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::User { content } => Some(content.text().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" / ");
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text(text)),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}
async fn execute(
    store: &Store,
    executor: &dyn AgentExecutor,
    id: &str,
    key: &str,
    prompt: &str,
) -> Arc<RunReport> {
    let mut request = RunRequest::new(prompt);
    let revision = store.get(id).unwrap().header.revision;
    let Prepared::New { mut history } = store
        .prepare(
            id,
            revision,
            key,
            prompt,
            request.limits.max_initial_history_bytes,
        )
        .unwrap()
    else {
        panic!("expected fresh turn")
    };
    history.push(Message::user(prompt));
    request.messages = history;
    request.metadata = BTreeMap::from([
        (sessions::SESSION_KEY.into(), id.into()),
        (sessions::TURN_KEY.into(), key.into()),
    ]);
    executor.execute(request).await.unwrap()
}
#[tokio::test]
async fn embedded_session_reopens_and_continues_without_application_or_web() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("state");
    let model = Arc::new(Echo::default());
    let id = {
        let store = Store::open(&base, temp.path()).unwrap();
        let id = store.create("embedded").unwrap().id;
        let mut host = runtime::HostBuilder::new()
            .model(model.clone())
            .checkpoint_sink(Arc::new(SessionSink(store.clone())))
            .unwrap()
            .build()
            .await
            .unwrap();
        let executor = sessions::runtime(Arc::new(host.engine()), store.clone());
        let first = execute(&store, executor.as_ref(), &id, "first", "remember 27182").await;
        assert_eq!(first.status, RunStatus::Completed);
        assert_eq!(
            store.get(&id).unwrap().body.turns[0].run_id.as_deref(),
            Some(first.run_id.as_str())
        );
        host.shutdown().await.unwrap();
        id
    };
    let store = Store::open(&base, temp.path()).unwrap();
    assert!(matches!(
        store.prepare(&id, 0, "first", "remember 27182", 1).unwrap(),
        Prepared::Existing(..)
    ));
    let mut host = runtime::HostBuilder::new()
        .model(model.clone())
        .checkpoint_sink(Arc::new(SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let executor = sessions::runtime(Arc::new(host.engine()), store.clone());
    let second = execute(&store, executor.as_ref(), &id, "second", "continue").await;
    assert_eq!(second.output.as_deref(), Some("remember 27182 / continue"));
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    let saved = store.get(&id).unwrap();
    assert_eq!(saved.body.turns.len(), 2);
    assert_eq!(saved.history().unwrap().len(), 4);
    host.shutdown().await.unwrap();
}

#[test]
fn unsupported_format_and_missing_current_fields_are_rejected_not_migrated() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("state");
    let store = Store::open(&base, temp.path()).unwrap();
    let header = store.create("format").unwrap();
    let path = base.join(format!("{}.jsonl", header.id));
    let original = std::fs::read_to_string(&path).unwrap();
    let (head, body) = original.split_once('\n').unwrap();
    for mode in 0..3 {
        let mut head: serde_json::Value = serde_json::from_str(head).unwrap();
        let mut body: serde_json::Value = serde_json::from_str(body).unwrap();
        match mode {
            0 => head["version"] = serde_json::json!(1),
            1 => {
                head.as_object_mut().unwrap().remove("pinned");
            }
            _ => {
                body.as_object_mut().unwrap().remove("archive");
            }
        }
        let broken = format!("{head}\n{body}\n");
        std::fs::write(&path, &broken).unwrap();
        assert!(store.get(&header.id).is_err());
        assert!(store
            .rename(&header.id, header.revision, "no migration")
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }
}

#[test]
fn custom_admission_limit_rejects_before_saving_a_new_turn() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("state"), temp.path()).unwrap();
    let h = store.create("limits").unwrap();
    assert_eq!(
        store
            .prepare(&h.id, h.revision, "first", "x", 1)
            .err()
            .unwrap()
            .code,
        SessionErrorCode::Capacity
    );
    assert_eq!(store.get(&h.id).unwrap().header.revision, h.revision);
    assert_eq!(store.get(&h.id).unwrap().body.turns.len(), 0);
    let exact = serde_json::to_vec(&vec![Message::user("x")]).unwrap().len();
    assert!(matches!(
        store
            .prepare(&h.id, h.revision, "first", "x", exact)
            .unwrap(),
        Prepared::New { .. }
    ));
}

#[tokio::test]
async fn missing_binding_or_checkpoint_sink_cannot_report_a_saved_success() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("state"), temp.path()).unwrap();
    let header = store.create("missing-sink").unwrap();
    let model = Arc::new(Echo::default());
    let mut host = runtime::HostBuilder::new()
        .model(model.clone())
        .build()
        .await
        .unwrap();
    let executor = sessions::runtime(Arc::new(host.engine()), store.clone());
    let mut request = RunRequest::new("unsaved");
    request
        .metadata
        .insert(sessions::SESSION_KEY.into(), header.id.clone());
    assert!(executor.start(request.clone()).is_err());
    assert!(model.requests.lock().unwrap().is_empty());
    store
        .prepare(
            &header.id,
            header.revision,
            "one",
            "unsaved",
            request.limits.max_initial_history_bytes,
        )
        .unwrap();
    request
        .metadata
        .insert(sessions::TURN_KEY.into(), "one".into());
    let error = executor.execute(request).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Checkpoint);
    assert_eq!(
        store.get(&header.id).unwrap().history().unwrap(),
        vec![Message::user("unsaved")]
    );
    // Unbound Runs deliberately remain ephemeral, even through the same decorator.
    assert_eq!(
        executor
            .execute(RunRequest::new("temporary"))
            .await
            .unwrap()
            .status,
        RunStatus::Completed
    );
    host.shutdown().await.unwrap();
}

struct WaitingModel {
    calls: AtomicUsize,
    started: tokio::sync::Notify,
}
#[async_trait]
impl Model for WaitingModel {
    async fn stream(&self, _: ModelRequest, cancel: CancellationToken) -> api::Result<ModelStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        cancel.cancelled().await;
        Err(AgentError::new(ErrorCode::Cancelled, "cancelled fixture"))
    }
}
#[tokio::test]
async fn embedded_owned_execution_cancels_and_settles_the_saved_turn() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("state"), temp.path()).unwrap();
    let id = store.create("cancel").unwrap().id;
    let model = Arc::new(WaitingModel {
        calls: AtomicUsize::new(0),
        started: tokio::sync::Notify::new(),
    });
    let mut host = runtime::HostBuilder::new()
        .model(model.clone())
        .checkpoint_sink(Arc::new(SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let executor = sessions::runtime(Arc::new(host.engine()), store.clone());
    let task_store = store.clone();
    let task_id = id.clone();
    let task = tokio::spawn(async move {
        execute(&task_store, executor.as_ref(), &task_id, "one", "wait").await
    });
    model.started.notified().await;
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if store.get(&id).unwrap().header.status == sessions::Status::Cancelled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    host.shutdown().await.unwrap();
}
