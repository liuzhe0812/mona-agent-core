use api::*;
use sessions::*;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn idle_state_is_versioned_bounded_and_persistent_without_a_second_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let store = Store::open(&root, temp.path()).unwrap();
    let first = store.create("host-state").unwrap();
    let header = store.update_state(&first.id, first.revision, "test.plan", |_| Ok(json!({"mode":"plan_only"}))).unwrap();
    assert_eq!(header.revision, first.revision + 1);
    assert_eq!(store.get(&first.id).unwrap().body.state["test.plan"]["mode"], "plan_only");
    assert!(store.update_state(&first.id, first.revision, "test.plan", |_| Ok(json!({}))).is_err());
    let before = std::fs::read(root.join(format!("{}.jsonl", first.id))).unwrap();
    assert!(store.update_state(&first.id, header.revision, "../invalid", |_| Ok(json!("no"))).is_err());
    assert!(store.update_state(&first.id, header.revision, "test.large", |_| Ok(json!("x".repeat(65536)))).is_err());
    assert_eq!(std::fs::read(root.join(format!("{}.jsonl", first.id))).unwrap(), before);
    drop(store);
    let store = Store::open(&root, temp.path()).unwrap();
    assert_eq!(store.get(&first.id).unwrap().body.state["test.plan"]["mode"], "plan_only");
}

#[test]
fn admission_uses_exact_state_and_duplicates_never_rerun_host_projection() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("s"), temp.path()).unwrap();
    let header = store.create("context").unwrap();
    let header = store.update_state(&header.id, header.revision, "mode", |_| Ok(json!("plan_only"))).unwrap();
    let before = store.get(&header.id).unwrap().header.revision;
    assert!(store.prepare_with_context(&header.id, before, "rejected", "hello", 100000, |_, _| {
        Ok(BTreeMap::from([(SESSION_KEY.into(), "forged".into())]))
    }).is_err());
    assert_eq!(store.header(&header.id).unwrap().revision, before);
    let (prepared, context) = store.prepare_with_context(&header.id, before, "turn", "hello", 100000, |doc, history| {
        assert!(history.is_empty()); assert_eq!(doc.header.revision, before);
        Ok(BTreeMap::from([("example.mode".into(), doc.body.state["mode"].as_str().unwrap().into())]))
    }).unwrap();
    assert!(matches!(prepared, Prepared::New { .. })); assert_eq!(context["example.mode"], "plan_only");
    assert!(store.update_state(&header.id, store.header(&header.id).unwrap().revision, "mode", |_| Ok(json!("normal"))).is_err());
    let (prepared, context) = store.prepare_with_context(&header.id, 0, "turn", "hello", 1, |_, _| panic!("duplicate callback must not run")).unwrap();
    assert!(matches!(prepared, Prepared::Existing(..))); assert!(context.is_empty());
}

struct ModelOnce;
#[async_trait]
impl Model for ModelOnce {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        Ok(Box::pin(futures_util::stream::iter(vec![Ok(ModelEvent::Text("done".into())), Ok(ModelEvent::Finish(FinishReason::Stop)), Ok(ModelEvent::End)])))
    }
}
struct StateSink(Arc<Store>);
#[async_trait]
impl CheckpointSink for StateSink {
    async fn commit(&self, cp: Arc<RunCheckpoint>, cancel: CancellationToken) -> api::Result<()> {
        let patch = HostState::from([("test.revision".into(), json!(cp.revision))]);
        self.0.commit_with_state(&cp, &cancel, &patch).map_err(|e| AgentError::new(ErrorCode::Checkpoint, e.message))?;
        self.0.commit_with_state(&cp, &cancel, &patch).unwrap();
        let inconsistent = HostState::from([("test.revision".into(), json!(cp.revision + 1))]);
        assert!(self.0.commit_with_state(&cp, &cancel, &inconsistent).is_err());
        Ok(())
    }
}
#[tokio::test]
async fn checkpoint_and_projection_acknowledge_the_same_immutable_revision() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("s"), temp.path()).unwrap();
    let header = store.create("atomic").unwrap();
    let mut host = runtime::HostBuilder::new().model(Arc::new(ModelOnce))
        .checkpoint_sink(Arc::new(StateSink(store.clone()))).unwrap().build().await.unwrap();
    store.prepare(&header.id, header.revision, "turn", "hello", 10000).unwrap();
    let mut req = RunRequest::new("hello");
    req.metadata = BTreeMap::from([(SESSION_KEY.into(), header.id.clone()), (TURN_KEY.into(), "turn".into())]);
    let result = host.engine().execute(req).await.unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    let saved = store.get(&header.id).unwrap();
    assert_eq!(saved.body.state["test.revision"].as_u64(), result.checkpoint.last_acknowledged_revision);
    assert_eq!(saved.body.checkpoint.as_ref().unwrap().revision, saved.body.state["test.revision"].as_u64().unwrap());
    host.shutdown().await.unwrap();
}
