use api::CancellationToken;
use memory::*;
use std::sync::Arc;
fn change(s: &dyn Backend, ops: Vec<Operation>) -> Change {
    Change {
        revision: s.read(&CancellationToken::new()).unwrap().revision,
        operations: ops,
    }
}
fn add(text: &str) -> Operation {
    Operation::Add { text: text.into() }
}
#[test]
fn markdown_roundtrip_atomic_batch_conflicts_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let s = FileStore::open(dir.path(), Limits::default()).unwrap();
    let first = s
        .apply(
            &change(
                &s,
                vec![add("使用中文\n保留用户纠正。"), add("只在内网部署")],
            ),
            Origin::host(),
            &cancel,
        )
        .unwrap();
    assert!(std::fs::read_to_string(dir.path().join("MEMORY.md"))
        .unwrap()
        .contains("只在内网部署"));
    drop(s);
    let s = FileStore::open(dir.path(), Limits::default()).unwrap();
    assert_eq!(s.read(&cancel).unwrap().entries, first.entries);
    let failed = Change {
        revision: first.revision.clone(),
        operations: vec![
            add("must not persist"),
            Operation::Remove {
                id: "missing".into(),
            },
        ],
    };
    assert!(s.apply(&failed, Origin::host(), &cancel).is_err());
    assert_eq!(s.read(&cancel).unwrap().revision, first.revision);
    let next = s
        .apply(
            &change(
                &s,
                vec![
                    Operation::Replace {
                        id: first.entries[1].id.clone(),
                        text: "内网 Linux 虚拟机".into(),
                    },
                    Operation::Remove {
                        id: first.entries[0].id.clone(),
                    },
                ],
            ),
            Origin::agent("run-1", "call-1"),
            &cancel,
        )
        .unwrap();
    assert_eq!(next.entries.len(), 1);
    assert_eq!(next.entries[0].origin.actor, "agent");
    assert_eq!(
        s.apply(&failed, Origin::host(), &cancel).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert!(!next.markdown.contains("使用中文"));
}
#[test]
fn limits_are_final_batch_limits_not_silent_eviction_and_duplicates_are_noop() {
    let s = InMemoryStore::new(Limits {
        max_text_bytes: 10,
        max_entry_bytes: 10,
        max_entries: 1,
    })
    .unwrap();
    let c = CancellationToken::new();
    let first = s
        .apply(&change(&s, vec![add("0123456789")]), Origin::host(), &c)
        .unwrap();
    let again = s
        .apply(&change(&s, vec![add("0123456789")]), Origin::host(), &c)
        .unwrap();
    assert_eq!(first.revision, again.revision);
    assert_eq!(
        s.apply(&change(&s, vec![add("new")]), Origin::host(), &c)
            .unwrap_err()
            .code,
        ErrorCode::Capacity
    );
    let v = s
        .apply(
            &change(
                &s,
                vec![
                    add("replacemen"),
                    Operation::Remove {
                        id: first.entries[0].id.clone(),
                    },
                ],
            ),
            Origin::host(),
            &c,
        )
        .unwrap();
    assert_eq!(v.entries.len(), 1);
    assert_eq!(v.entries[0].text, "replacemen");
}
#[test]
fn corrupt_file_cancel_and_failed_io_do_not_reset_or_claim_success() {
    let dir = tempfile::tempdir().unwrap();
    let s = FileStore::open(dir.path(), Limits::default()).unwrap();
    let c = CancellationToken::new();
    let change = change(&s, vec![add("safe")]);
    c.cancel();
    assert_eq!(
        s.apply(&change, Origin::host(), &c).unwrap_err().code,
        ErrorCode::Closed
    );
    assert!(!dir.path().join("MEMORY.md").exists());
    std::fs::write(dir.path().join("MEMORY.md"), "external incomplete data").unwrap();
    assert!(s.read(&CancellationToken::new()).is_err());
    assert!(s
        .apply(&change, Origin::host(), &CancellationToken::new())
        .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("MEMORY.md")).unwrap(),
        "external incomplete data"
    );
    std::fs::remove_file(dir.path().join("MEMORY.md")).unwrap();
    std::fs::create_dir(dir.path().join("MEMORY.md")).unwrap();
    assert!(s
        .apply(&change, Origin::host(), &CancellationToken::new())
        .is_err());
}
#[test]
fn two_handles_cannot_overwrite_a_newer_revision() {
    let dir = tempfile::tempdir().unwrap();
    let a = FileStore::open(dir.path(), Limits::default()).unwrap();
    let b = FileStore::open(dir.path(), Limits::default()).unwrap();
    let old = change(&a, vec![add("old")]);
    let c = CancellationToken::new();
    b.apply(&change(&b, vec![add("new")]), Origin::host(), &c)
        .unwrap();
    assert_eq!(
        a.apply(&old, Origin::host(), &c).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(a.read(&c).unwrap().entries[0].text, "new");
}
#[tokio::test]
async fn independent_bindings_have_stable_projection_and_refresh_without_rewriting_history() {
    struct Model;
    #[api::async_trait]
    impl api::Model for Model {
        async fn stream(
            &self,
            _: api::ModelRequest,
            _: CancellationToken,
        ) -> api::Result<api::ModelStream> {
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(api::ModelEvent::Text("ok".into())),
                Ok(api::ModelEvent::Finish(api::FinishReason::Stop)),
                Ok(api::ModelEvent::End),
            ])))
        }
    }
    use api::AgentExecutor;
    let s = Arc::new(InMemoryStore::default());
    let c = CancellationToken::new();
    let first = s
        .apply(
            &change(s.as_ref(), vec![add("stable fact")]),
            Origin::host(),
            &c,
        )
        .unwrap();
    let plugin = MemoryPlugin::new(vec![Binding::new("personal", s.clone(), false)]).unwrap();
    assert_eq!(plugin.tools().len(), 1);
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(Model))
        .plugin(Arc::new(plugin))
        .build()
        .await
        .unwrap();
    let a = host
        .engine()
        .execute(api::RunRequest::new("one"))
        .await
        .unwrap();
    let b = host
        .engine()
        .execute(api::RunRequest::new("two"))
        .await
        .unwrap();
    assert_eq!(
        a.model_requests[0].request.as_ref().unwrap().messages[0],
        b.model_requests[0].request.as_ref().unwrap().messages[0]
    );
    assert!(!a
        .transcript
        .iter()
        .any(|m| m.text().contains("stable fact")));
    s.apply(
        &change(
            s.as_ref(),
            vec![Operation::Remove {
                id: first.entries[0].id.clone(),
            }],
        ),
        Origin::host(),
        &c,
    )
    .unwrap();
    let d = host
        .engine()
        .execute(api::RunRequest::new("three"))
        .await
        .unwrap();
    assert!(!d.model_requests[0]
        .request
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .any(|m| m.text().contains("stable fact")));
    host.shutdown().await.unwrap();
}
