use api::*;
use serde_json::json;
use sessions::{Prepared, SessionErrorCode as Code, Status, Store, SESSION_KEY, TURN_KEY};
use std::collections::BTreeMap;

fn file_path(base: &std::path::Path, id: &str) -> std::path::PathBuf {
    base.join(format!("{id}.jsonl"))
}
fn checkpoint(id: &str, key: &str, messages: Vec<Message>) -> RunCheckpoint {
    RunCheckpoint {
        schema_version: CHECKPOINT_VERSION,
        run_id: "fixture-run".into(),
        revision: 1,
        phase: CheckpointPhase::BeforeModel,
        step: 1,
        transcript: messages,
        pending_tools: BTreeMap::new(),
        selected_tools: vec![],
        model_options: ModelOptions::default(),
        metadata: BTreeMap::from([
            (SESSION_KEY.into(), id.into()),
            (TURN_KEY.into(), key.into()),
        ]),
        task_usage: TaskUsage {
            model_calls: 1,
            reported_tokens: 3,
            usage_complete: true,
        },
        statistics: RunStatistics::default(),
        status: None,
        error: None,
    }
}

#[test]
fn crash_recovery_closes_pending_pairs_without_claiming_rollback_or_replaying() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("crash").unwrap();
    store
        .prepare(
            &h.id,
            h.revision,
            "turn",
            "go",
            RunLimits::default().max_initial_history_bytes,
        )
        .unwrap();
    let mut cp = checkpoint(
        &h.id,
        "turn",
        vec![
            Message::user("go"),
            Message::Assistant {
                content: "working".into(),
                tool_calls: ["a", "b", "c"]
                    .into_iter()
                    .map(|id| ToolCall::new(id, "tool", json!({})))
                    .collect(),
                reasoning_content: None,
                provider_data: None,
            },
        ],
    );
    cp.phase = CheckpointPhase::ToolSettled {
        call_id: "c".into(),
    };
    cp.statistics.input_tokens = 100;
    cp.statistics.cache_read_tokens = Some(80);
    cp.statistics.context = Some(ContextUsage { tokens: Some(100), capacity: Some(1000),
        provider_anchored: true, observed_at: 1, system_tokens: Some(20),
        tool_tokens: Some(30), message_tokens: Some(50) });
    cp.pending_tools = BTreeMap::from([
        ("a".into(), CheckpointToolState::Pending),
        ("b".into(), CheckpointToolState::IntentRecorded),
        (
            "c".into(),
            CheckpointToolState::Settled {
                result: ToolResult::new("c", ToolStatus::Success, "known result"),
            },
        ),
    ]);
    store.commit(&cp, &CancellationToken::new()).unwrap();
    drop(store);
    let store = Store::open(&base, tmp.path()).unwrap();
    let doc = store.get(&h.id).unwrap();
    assert_eq!(doc.header.status, Status::Interrupted);
    assert_eq!(doc.body.turns[0].steps, 1);
    assert_eq!(doc.body.turns[0].statistics.cache_read_tokens, Some(80));
    assert_eq!(doc.body.turns[0].statistics.context.as_ref().unwrap().tokens, Some(100));
    assert_eq!(
        serde_json::to_value(doc.body.checkpoint.as_ref().unwrap()).unwrap(),
        serde_json::to_value(&cp).unwrap()
    );
    let history = doc.history().unwrap();
    api::validate_messages(&history).unwrap();
    let results: Vec<_> = history
        .iter()
        .filter_map(|m| {
            if let Message::Tool { result } = m {
                Some(result.status)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        results,
        [
            ToolStatus::Skipped,
            ToolStatus::Unknown,
            ToolStatus::Success
        ]
    );
    let Prepared::New { history: resumed } = store
        .prepare(
            &h.id,
            doc.header.revision,
            "next",
            "inspect first",
            RunLimits::default().max_initial_history_bytes,
        )
        .unwrap()
    else {
        panic!("new turn expected")
    };
    assert_eq!(resumed, history);
}

#[test]
fn accepted_input_without_a_run_is_retained_and_not_restarted_by_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("pending").unwrap();
    store
        .prepare(
            &h.id,
            h.revision,
            "once",
            "saved before dispatch",
            RunLimits::default().max_initial_history_bytes,
        )
        .unwrap();
    drop(store);
    let store = Store::open(&base, tmp.path()).unwrap();
    let doc = store.get(&h.id).unwrap();
    assert_eq!(doc.header.status, Status::Interrupted);
    assert_eq!(doc.history().unwrap()[0].text(), "saved before dispatch");
    assert!(matches!(
        store
            .prepare(
                &h.id,
                0,
                "once",
                "saved before dispatch",
                RunLimits::default().max_initial_history_bytes
            )
            .unwrap(),
        Prepared::Existing(..)
    ));
}

#[test]
fn revisions_pagination_workspace_scope_and_deletion_are_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("one").unwrap();
    store.create("two").unwrap();
    let renamed = store.rename(&h.id, h.revision, "我的会话").unwrap();
    assert_eq!(
        store.rename(&h.id, h.revision, "stale").err().unwrap().code,
        Code::Conflict
    );
    assert_eq!(store.list(0, 1, "", false).unwrap().next_offset, Some(1));
    assert_eq!(store.list(0, 50, "我的", false).unwrap().sessions.len(), 1);
    let other_dir = tmp.path().join("other");
    std::fs::create_dir(&other_dir).unwrap();
    // One authority store keeps every bound cwd; another writer may not take it over.
    assert!(Store::open(&base, &other_dir).is_err());
    let other = Store::open(&tmp.path().join("other-state"), &other_dir).unwrap();
    assert!(other.list(0, 50, "", false).unwrap().sessions.is_empty());
    let bound = store.create_in("other-dir", &other_dir, BTreeMap::new()).unwrap();
    assert_ne!(bound.workspace, h.workspace);
    assert!(store.create_in("other-dir", tmp.path(), BTreeMap::new()).is_err());
    assert!(store.get("../one").is_err());
    assert!(store.get("s-one/..").is_err());
    store.delete(&h.id, renamed.revision).unwrap();
    assert_eq!(store.get(&h.id).err().unwrap().code, Code::NotFound);
}

#[test]
fn malformed_storage_never_becomes_an_empty_session_or_gets_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("bad").unwrap();
    store.create("good").unwrap();
    let path = file_path(&base, &h.id);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    let broken = std::fs::read(&path).unwrap();
    assert!(store.get(&h.id).is_err());
    assert!(store.create("bad").is_err());
    assert!(store
        .prepare(
            &h.id,
            h.revision,
            "x",
            "do not execute",
            RunLimits::default().max_initial_history_bytes
        )
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), broken);
    let listed = store.list(0, 50, "", false).unwrap();
    assert_eq!(listed.unreadable, 1);
    assert_eq!(listed.sessions.len(), 1);
}

#[test]
fn cancelled_commit_and_conflicting_revision_preserve_last_acknowledged_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("atomic").unwrap();
    store
        .prepare(
            &h.id,
            h.revision,
            "t",
            "go",
            RunLimits::default().max_initial_history_bytes,
        )
        .unwrap();
    let mut cp = checkpoint(&h.id, "t", vec![Message::user("go")]);
    store.commit(&cp, &CancellationToken::new()).unwrap();
    let path = file_path(&base, &h.id);
    let saved = std::fs::read(&path).unwrap();
    store.commit(&cp, &CancellationToken::new()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    cp.transcript.push(Message::user("different same revision"));
    assert!(store.commit(&cp, &CancellationToken::new()).is_err());
    cp.revision += 1;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(store.commit(&cp, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
}

#[test]
fn inconsistent_turn_ranges_are_rejected_before_read_or_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("range").unwrap();
    store
        .prepare(
            &h.id,
            h.revision,
            "t",
            "go",
            RunLimits::default().max_initial_history_bytes,
        )
        .unwrap();
    store.fail_start(&h.id, "t", "test-only failure").unwrap();
    let path = file_path(&base, &h.id);
    let bytes = std::fs::read_to_string(&path).unwrap();
    let (header, body) = bytes.split_once('\n').unwrap();
    let mut body: serde_json::Value = serde_json::from_str(body).unwrap();
    body["turns"][0]["end"] = json!(999);
    let broken = format!("{header}\n{body}\n");
    std::fs::write(&path, &broken).unwrap();
    assert!(store.get(&h.id).is_err());
    assert!(store.rename(&h.id, 3, "must not rewrite").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

#[test]
fn empty_session_is_durable_and_single_writer_owned() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("sessions"), temp.path()).unwrap();
    let created = store.create("create-once").unwrap();
    assert_eq!(store.create("create-once").unwrap().id, created.id);
    assert!(Store::open(&temp.path().join("sessions"), temp.path()).is_err());
    drop(store);
    let store = Store::open(&temp.path().join("sessions"), temp.path()).unwrap();
    assert_eq!(store.list(0, 50, "", false).unwrap().sessions.len(), 1);
    assert_eq!(
        store.get(&created.id).unwrap().header.revision,
        created.revision
    );
}

#[test]
fn unrelated_directories_are_not_part_of_the_current_session_format() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("sessions");
    std::fs::create_dir_all(base.join("a".repeat(64))).unwrap();
    std::fs::write(base.join("note.txt"), "ignored").unwrap();

    let store = Store::open(&base, temp.path()).unwrap();
    assert!(store.list(0, 50, "", false).unwrap().sessions.is_empty());
    let created = store.create("current").unwrap();
    assert_eq!(store.header(&created.id).unwrap().id, created.id);
}

#[test]
fn pinned_archived_and_unread_flags_are_durable_and_select_the_list_view() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("data");
    let store = Store::open(&base, tmp.path()).unwrap();
    let one = store.create("one").unwrap();
    let two = store.create("two").unwrap();
    let pinned = store.set_pinned(&one.id, one.revision, true).unwrap();
    assert!(pinned.pinned && !pinned.archived);
    assert!(
        store
            .set_archived(&two.id, two.revision, true)
            .unwrap()
            .archived
    );
    let active = store.list(0, 50, "", false).unwrap();
    assert_eq!(
        active
            .sessions
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        [one.id.as_str()]
    );
    assert!(active.sessions[0].pinned);
    let archived = store.list(0, 50, "", true).unwrap();
    assert_eq!(
        archived
            .sessions
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        [two.id.as_str()]
    );
    // A stale revision is refused instead of overwriting newer state.
    assert_eq!(
        store
            .set_pinned(&one.id, one.revision, false)
            .err()
            .unwrap()
            .code,
        Code::Conflict
    );
    assert!(
        store
            .set_archived(&one.id, pinned.revision, true)
            .unwrap()
            .archived
    );
    drop(store);
    let store = Store::open(&base, tmp.path()).unwrap();
    let reopened = store.get(&one.id).unwrap().header;
    assert!(reopened.pinned && reopened.archived);
    // Unarchiving puts the task back into the active view.
    store
        .set_archived(&one.id, reopened.revision, false)
        .unwrap();
    assert_eq!(
        store
            .list(0, 50, "", false)
            .unwrap()
            .sessions
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        [one.id.as_str()]
    );
    // The unread mark is durable as well, and clearing it leaves the other flags alone.
    let revision = store.get(&one.id).unwrap().header.revision;
    let marked = store.set_unread(&one.id, revision, true).unwrap();
    assert!(marked.unread && marked.pinned && !marked.archived);
    drop(store);
    let store = Store::open(&base, tmp.path()).unwrap();
    assert!(store.get(&one.id).unwrap().header.unread);
    let revision = store.get(&one.id).unwrap().header.revision;
    let cleared = store.set_unread(&one.id, revision, false).unwrap();
    assert!(!cleared.unread && cleared.pinned && !cleared.archived);
}
