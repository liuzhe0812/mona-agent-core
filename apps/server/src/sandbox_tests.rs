use super::*;
use api::*;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Arc<sessions::Store>, SandboxHost) {
    let root = tempfile::tempdir().unwrap();
    let store = sessions::Store::open(&root.path().join("state"), root.path()).unwrap();
    let host = SandboxHost {
        provider: Arc::new(LocalSandbox::new(Runner::embedded(
            std::env::current_exe().unwrap(),
        ))),
        default_mode: Mode::WorkspaceWrite,
        locked: false,
    };
    (root, store, host)
}
fn action(revision: u64, mode: Mode) -> SandboxAction {
    SandboxAction { revision, mode }
}
fn checkpoint(
    header: &sessions::Header,
    turn: &sessions::Turn,
    metadata: BTreeMap<String, String>,
) -> RunCheckpoint {
    let mut metadata = metadata;
    metadata.insert(sessions::SESSION_KEY.into(), header.id.clone());
    metadata.insert(sessions::TURN_KEY.into(), turn.id.clone());
    RunCheckpoint {
        schema_version: CHECKPOINT_VERSION,
        run_id: "sandbox-test-run".into(),
        revision: 1,
        phase: CheckpointPhase::BeforeModel,
        step: 1,
        transcript: vec![Message::user("test")],
        pending_tools: BTreeMap::new(),
        selected_tools: vec![],
        model_options: ModelOptions::default(),
        metadata,
        task_usage: TaskUsage {
            model_calls: 0,
            reported_tokens: 0,
            usage_complete: true,
        },
        statistics: RunStatistics::default(),
        status: None,
        error: None,
    }
}

#[test]
fn modes_are_durable_idle_revision_guarded_and_never_accept_browser_roots() {
    let (root, store, host) = fixture();
    let header = store.create("sandbox-session").unwrap();
    assert_eq!(
        host.metadata(&store.get(&header.id).unwrap(), true)
            .unwrap()[MODE_KEY],
        "workspace-write"
    );
    host.change(
        &store,
        &header.id,
        action(header.revision, Mode::ReadOnly),
        true,
    )
    .unwrap();
    let doc = store.get(&header.id).unwrap();
    assert_eq!(stored_mode(&doc).unwrap(), Some("read-only"));
    assert!(doc.body.turns.is_empty());
    assert!(host.metadata(&doc, false).is_err());
    assert!(host
        .change(
            &store,
            &header.id,
            action(header.revision, Mode::DangerFullAccess),
            true
        )
        .is_err());
    assert!(serde_json::from_value::<SandboxAction>(
        json!({"revision":doc.header.revision,"mode":"workspace-write","workspace":"C:/"})
    )
    .is_err());
    let revision = doc.header.revision;
    drop(store);
    let store = sessions::Store::open(&root.path().join("state"), root.path()).unwrap();
    assert_eq!(
        stored_mode(&store.get(&header.id).unwrap()).unwrap(),
        Some("read-only")
    );
    host.change(
        &store,
        &header.id,
        action(revision, Mode::WorkspaceWrite),
        true,
    )
    .unwrap();
    let doc = store.get(&header.id).unwrap();
    store
        .prepare(&header.id, doc.header.revision, "turn", "test", 10000)
        .unwrap();
    let current = store.header(&header.id).unwrap();
    assert!(host
        .change(
            &store,
            &header.id,
            action(current.revision, Mode::DangerFullAccess),
            true
        )
        .is_err());
}
#[test]
fn deployment_policy_wins_but_disabling_never_opens_a_confined_session() {
    let (_root, store, mut host) = fixture();
    let header = store.create("locked").unwrap();
    host.change(
        &store,
        &header.id,
        action(header.revision, Mode::WorkspaceWrite),
        true,
    )
    .unwrap();
    let doc = store.get(&header.id).unwrap();
    host.default_mode = Mode::ReadOnly;
    host.locked = true;
    assert_eq!(host.metadata(&doc, true).unwrap()[MODE_KEY], "read-only");
    assert!(host
        .change(
            &store,
            &header.id,
            action(doc.header.revision, Mode::DangerFullAccess),
            true
        )
        .is_err());
    assert!(host.metadata(&doc, false).is_err());
}
#[tokio::test]
async fn default_run_policy_is_saved_with_checkpoint_and_not_resurrected_when_unbound() {
    let (_root, store, host) = fixture();
    let header = store.create("default").unwrap();
    let meta = host
        .metadata(&store.get(&header.id).unwrap(), true)
        .unwrap();
    store
        .prepare(&header.id, header.revision, "first", "test", 10000)
        .unwrap();
    let doc = store.get(&header.id).unwrap();
    let cp = checkpoint(&doc.header, &doc.body.turns[0], meta);
    ProductSink(store.clone())
        .commit(Arc::new(cp), CancellationToken::new())
        .await
        .unwrap();
    let doc = store.get(&header.id).unwrap();
    assert_eq!(stored_mode(&doc).unwrap(), Some("workspace-write"));
    assert!(require_unconfined(&doc).is_err());
    assert_eq!(
        doc.body.checkpoint.as_ref().unwrap().metadata[MODE_KEY],
        "workspace-write"
    );
}
#[tokio::test]
async fn cancelled_checkpoint_does_not_publish_a_policy_or_replace_previous_bytes() {
    let (_root, store, host) = fixture();
    let header = store.create("cancel").unwrap();
    store
        .prepare(&header.id, header.revision, "first", "test", 10000)
        .unwrap();
    let doc = store.get(&header.id).unwrap();
    let cp = checkpoint(
        &doc.header,
        &doc.body.turns[0],
        host.metadata(&doc, true).unwrap(),
    );
    let revision = doc.header.revision;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(ProductSink(store.clone())
        .commit(Arc::new(cp), cancel)
        .await
        .is_err());
    let doc = store.get(&header.id).unwrap();
    assert_eq!(doc.header.revision, revision);
    assert!(!doc.body.state.contains_key(STATE_KEY));
    assert!(doc.body.checkpoint.is_none());
}
#[test]
fn malformed_saved_policy_never_becomes_an_unrestricted_default() {
    let (_root, store, host) = fixture();
    let header = store.create("corrupt").unwrap();
    for value in [
        json!({"version":2,"mode":"read-only"}),
        json!({"version":1,"mode":"unknown"}),
        json!({"version":1,"mode":"danger-full-access","extra":true}),
    ] {
        let h = store.header(&header.id).unwrap();
        store
            .update_state(&h.id, h.revision, STATE_KEY, |_| Ok(value))
            .unwrap();
        let doc = store.get(&h.id).unwrap();
        assert!(host.metadata(&doc, true).is_err());
        assert!(require_unconfined(&doc).is_err());
    }
}
