use api::Message;
use serde_json::json;
use sessions::{HostState, Prepared, Store};

#[test]
fn trusted_initial_history_is_not_a_fake_turn_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("sessions");
    let store = Store::open(&base, dir.path()).unwrap();
    let h = store.create("fork").unwrap();
    let history = vec![Message::user("exact inherited constraint")];
    let h = store
        .initialize_history(&h.id, h.revision, history.clone(), HostState::new())
        .unwrap();
    let doc = store.get(&h.id).unwrap();
    assert_eq!(doc.header.turn_count, 0);
    assert_eq!(doc.history().unwrap(), history);
    assert!(store
        .update_state(&h.id, h.revision, "sessions.origin", |_| Ok(json!({})))
        .is_err());
    assert!(store
        .initialize_history(&h.id, h.revision, vec![], HostState::new())
        .is_err());
    assert!(
        matches!(store.prepare(&h.id,h.revision,"turn","new work",10000).unwrap(),Prepared::New{history:h} if h==history)
    );
    let running = store.header(&h.id).unwrap();
    assert!(store
        .update_state(&h.id, running.revision, "sandbox", |_| Ok(json!({})))
        .is_err());
    store
        .update_auxiliary_state(&h.id, "aux.inbox", |_| Ok(json!(["supplement"])))
        .unwrap();
    assert!(store
        .update_auxiliary_state(&h.id, "sandbox", |_| Ok(json!({})))
        .is_err());
    drop(store);
    let store = Store::open(&base, dir.path()).unwrap();
    let doc = store.get(&h.id).unwrap();
    assert_eq!(doc.header.status, sessions::Status::Interrupted);
    assert_eq!(doc.body.state["aux.inbox"], json!(["supplement"]));
    assert_eq!(
        doc.history().unwrap(),
        vec![
            Message::user("exact inherited constraint"),
            Message::user("new work")
        ]
    );
}
#[test]
fn seed_rejects_system_and_unbalanced_tool_history_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("s"), dir.path()).unwrap();
    let h = store.create("bad").unwrap();
    assert!(store
        .initialize_history(
            &h.id,
            h.revision,
            vec![Message::system("no")],
            HostState::new()
        )
        .is_err());
    assert_eq!(store.header(&h.id).unwrap().revision, h.revision);
}
