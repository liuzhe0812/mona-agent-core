use api::*;
use spill::*;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn streamed_utf8_is_complete_and_readable_after_reopening_the_store() {
    let temp = tempfile::tempdir().unwrap();
    let config = SpillConfig::default();
    let store = LocalSpillStore::new(temp.path(), config.clone()).unwrap();
    let text = format!("{}中间{}TAIL", "x".repeat(16383), "🙂".repeat(40000));
    let (mut producer, mut reader) = tokio::io::duplex(7);
    let expected = text.clone();
    let writer = tokio::spawn(async move {
        for part in expected.as_bytes().chunks(5) { producer.write_all(part).await.unwrap(); }
    });
    let record = store.put_stream("first", "call", &mut reader, text.len()).await.unwrap();
    writer.await.unwrap(); drop(store);
    let reopened = LocalSpillStore::new(temp.path(), config).unwrap();
    let mut full = String::new(); let mut offset = 0;
    loop {
        let page = reopened.read_page("first", &record.id, offset, 997).await.unwrap();
        full.push_str(&page.text); offset = page.next_offset;
        if page.eof { break; }
    }
    assert_eq!(full, text); assert_eq!(record.bytes, text.len() as u64);
    assert!(reopened.read_page("other", &record.id, 0, 20).await.is_err());
}
#[tokio::test]
async fn invalid_short_oversized_and_failed_sources_never_publish_an_archive() {
    let temp = tempfile::tempdir().unwrap();
    let store = LocalSpillStore::new(temp.path(), SpillConfig::default()).unwrap();
    for (bytes, declared) in [(vec![0xff],1), (b"short".to_vec(),9), (b"too-long".to_vec(),3), (vec![0xe4],1)] {
        assert!(store.put_stream("test", "bad", &mut bytes.as_slice(), declared).await.is_err());
    }
    let huge = SpillConfig::default().max_entry_bytes + 1;
    assert_eq!(store.put_stream("test", "quota", &mut &b""[..], huge).await.unwrap_err().code, ErrorCode::Limit);
    for dir in std::fs::read_dir(temp.path()).unwrap() {
        if dir.as_ref().unwrap().file_type().unwrap().is_dir() {
            assert_eq!(std::fs::read_dir(dir.unwrap().path()).unwrap().count(), 0);
        }
    }
    store.put("test", "valid-after-failure", "ok").await.unwrap();
}
#[tokio::test]
async fn cancelling_an_inflight_stream_releases_quota_and_removes_partial_files() {
    let temp = tempfile::tempdir().unwrap();
    let config = SpillConfig::default();
    let store = Arc::new(LocalSpillStore::new(temp.path(), config).unwrap());
    let (mut producer, mut reader) = tokio::io::duplex(1);
    let writing = store.clone();
    let task = tokio::spawn(async move { writing.put_stream("cancelled", "partial", &mut reader, 100000).await });
    tokio::time::timeout(Duration::from_secs(8), producer.write_all(b"incomplete")).await.unwrap().unwrap();
    task.abort(); assert!(task.await.unwrap_err().is_cancelled()); drop(producer);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let files: Vec<_> = std::fs::read_dir(temp.path()).unwrap().filter_map(|d| {
                d.ok().filter(|d| d.file_type().is_ok_and(|t| t.is_dir()))
            }).flat_map(|d| std::fs::read_dir(d.path()).unwrap().collect::<Vec<_>>()).collect();
            if files.is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    store.put("cancelled", "next", "success").await.unwrap();
    store.cleanup(&BTreeSet::new()).await.unwrap();
}
