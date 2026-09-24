#![cfg(feature = "search")]
use api::*;
use sessions::{search::*, Prepared, SessionSink, Store};
use std::{collections::BTreeMap, sync::Arc};
struct Echo;
#[async_trait]
impl Model for Echo {
    async fn stream(&self, _: ModelRequest, _: CancellationToken) -> api::Result<ModelStream> {
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Text("历史原文 answer-27182".into())),
            Ok(ModelEvent::Finish(FinishReason::Stop)),
            Ok(ModelEvent::End),
        ])))
    }
}
async fn turn(store: &Arc<Store>, id: &str, key: &str, prompt: &str) {
    let mut host = runtime::HostBuilder::new()
        .model(Arc::new(Echo))
        .checkpoint_sink(Arc::new(SessionSink(store.clone())))
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut req = RunRequest::new(prompt);
    let Prepared::New { mut history } = store
        .prepare(
            id,
            store.header(id).unwrap().revision,
            key,
            prompt,
            req.limits.max_initial_history_bytes,
        )
        .unwrap()
    else {
        panic!()
    };
    history.push(Message::user(prompt));
    req.messages = history;
    req.metadata = BTreeMap::from([
        (sessions::SESSION_KEY.into(), id.into()),
        (sessions::TURN_KEY.into(), key.into()),
    ]);
    let report = sessions::runtime(Arc::new(host.engine()), store.clone())
        .execute(req)
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Completed);
    host.shutdown().await.unwrap();
}
fn query(text: &str) -> SearchRequest {
    SearchRequest {
        query: text.into(),
        session_id: None,
        offset: None,
        limit: None,
    }
}
fn locator(hit: &Hit) -> ReadRequest {
    ReadRequest {
        session_id: hit.session_id.clone(),
        revision: hit.revision,
        message_hash: hit.message_hash.clone(),
        message_index: hit.message_index,
        offset: None,
        max_bytes: None,
    }
}
#[tokio::test]
async fn literal_chinese_ids_scope_original_read_append_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("sessions"), tmp.path()).unwrap();
    let a = store.create("normal").unwrap();
    let b = store
        .create_in(
            "project",
            tmp.path(),
            BTreeMap::from([("project.id".into(), "p1".into())]),
        )
        .unwrap();
    turn(&store, &a.id, "one", "只在内网部署，工单 TICKET_31415").await;
    turn(&store, &b.id, "one", "项目秘密内网部署").await;
    let search = HistorySearch::open(store.clone()).unwrap();
    let c = CancellationToken::new();
    let normal = Scope::Metadata {
        key: "project.id".into(),
        value: None,
    };
    let hits = search.search(&normal, &query("内网部署"), &c).unwrap();
    assert_eq!(hits.hits.len(), 1);
    assert_eq!(hits.hits[0].session_id, a.id);
    assert_eq!(
        search
            .search(&normal, &query("TICKET_31415"), &c)
            .unwrap()
            .hits
            .len(),
        1
    );
    assert!(search
        .search(&normal, &query("doesNotExist"), &c)
        .unwrap()
        .hits
        .is_empty());
    assert!(search
        .search(&normal, &query("\" OR private --"), &c)
        .unwrap()
        .hits
        .is_empty());
    let read = locator(&hits.hits[0]);
    assert_eq!(
        search.read(&normal, &read, &c).unwrap().text,
        "只在内网部署，工单 TICKET_31415"
    );
    turn(&store, &a.id, "two", "继续").await;
    // Tool checkpoints and appended turns must not make a current-session original unreadable.
    assert!(search.read(&normal, &read, &c).unwrap().revision > read.revision);
    let project = Scope::Metadata {
        key: "project.id".into(),
        value: Some("p1".into()),
    };
    assert!(search.read(&project, &read, &c).is_err());
    let foreign = SearchRequest {
        session_id: Some(b.id.clone()),
        ..query("内网部署")
    };
    assert!(search
        .search(&normal, &foreign, &c)
        .unwrap()
        .hits
        .is_empty());
    let mut invalid = read.clone();
    invalid.message_hash = "0".repeat(64);
    assert_eq!(
        search.read(&normal, &invalid, &c).unwrap_err().code,
        sessions::SessionErrorCode::Conflict
    );
    let h = store.header(&a.id).unwrap();
    store.set_archived(&a.id, h.revision, true).unwrap();
    assert_eq!(
        search
            .search(&normal, &query("内网部署"), &c)
            .unwrap()
            .hits
            .len(),
        1
    );
    store
        .delete(&a.id, store.header(&a.id).unwrap().revision)
        .unwrap();
    assert!(search
        .search(&normal, &query("内网部署"), &c)
        .unwrap()
        .hits
        .is_empty());
    assert!(search.read(&normal, &read, &c).is_err());
    c.cancel();
    assert!(search.search(&Scope::All, &query("内网"), &c).is_err());
}
#[tokio::test]
async fn restart_pages_and_index_rebuild_preserve_exact_text() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("sessions");
    let id;
    {
        let store = Store::open(&base, tmp.path()).unwrap();
        id = store.create("long").unwrap().id;
        turn(
            &store,
            &id,
            "first",
            &format!("精准检索 {} END_MARKER", "中文换行\n".repeat(1000)),
        )
        .await;
        let index = HistorySearch::open(store).unwrap();
        assert_eq!(
            index
                .search(&Scope::All, &query("精准检索"), &CancellationToken::new())
                .unwrap()
                .hits
                .len(),
            1
        );
    }
    let store = Store::open(&base, tmp.path()).unwrap();
    let index = HistorySearch::open(store.clone()).unwrap();
    let c = CancellationToken::new();
    let hit = index
        .search(&Scope::All, &query("精准检索"), &c)
        .unwrap()
        .hits
        .remove(0);
    let mut req = locator(&hit);
    req.max_bytes = Some(511);
    let mut all = String::new();
    loop {
        let page = index.read(&Scope::All, &req, &c).unwrap();
        assert!(page.text.len() <= 511);
        all.push_str(&page.text);
        if let Some(n) = page.next_offset {
            req.offset = Some(n)
        } else {
            break;
        }
    }
    assert_eq!(all, store.get(&id).unwrap().body.turns[0].prompt);
    drop(index);
    std::fs::remove_file(base.join("history-index.sqlite3")).unwrap();
    let rebuilt = HistorySearch::open(store).unwrap();
    assert_eq!(
        rebuilt
            .search(&Scope::All, &query("END_MARKER"), &c)
            .unwrap()
            .hits
            .len(),
        1
    );
}
#[tokio::test]
async fn corrupt_source_or_index_is_not_an_empty_result() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("sessions");
    let store = Store::open(&base, tmp.path()).unwrap();
    let h = store.create("c").unwrap();
    turn(&store, &h.id, "one", "needle").await;
    let index = HistorySearch::open(store.clone()).unwrap();
    let c = CancellationToken::new();
    assert_eq!(
        index
            .search(&Scope::All, &query("needle"), &c)
            .unwrap()
            .hits
            .len(),
        1
    );
    std::fs::write(base.join(format!("{}.jsonl", h.id)), "broken").unwrap();
    assert!(index.search(&Scope::All, &query("needle"), &c).is_err());
    drop(index);
    std::fs::write(base.join("history-index.sqlite3"), "corrupt sqlite").unwrap();
    assert!(HistorySearch::open(store).is_err());
}
