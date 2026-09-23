use api::*;
use runtime::HostBuilder;
use spill::*;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

#[derive(Default)]
struct Store {
    cleanups: Mutex<Vec<BTreeSet<String>>>,
    wrote: Notify,
}
#[async_trait]
impl SpillStore for Store {
    async fn put_stream(&self, _: &str, _: &str,
        source: &mut (dyn tokio::io::AsyncRead + Unpin + Send), bytes: usize) -> Result<SpillRecord> {
        use tokio::io::AsyncReadExt;
        let mut text = String::new(); source.read_to_string(&mut text).await.unwrap();
        assert_eq!(text.len(), bytes);
        self.wrote.notify_one();
        Ok(SpillRecord {
            id: "sp_fixture".into(),
            bytes: text.len() as u64,
        })
    }
    async fn read_page(&self, _: &str, _: &str, _: usize, _: usize) -> Result<SpillPage> {
        unreachable!()
    }
    async fn cleanup(&self, active: &BTreeSet<String>) -> Result<CleanupReport> {
        self.cleanups.lock().unwrap().push(active.clone());
        Ok(CleanupReport::default())
    }
}
struct FixtureModel;
#[async_trait]
impl Model for FixtureModel {
    async fn stream(&self, request: ModelRequest, _: CancellationToken) -> Result<ModelStream> {
        let hold = request.messages[0].text() == "hold";
        let events = if hold
            && request
                .messages
                .iter()
                .any(|m| matches!(m, Message::Tool { .. }))
        {
            return std::future::pending().await;
        } else if hold {
            vec![
                ModelEvent::ToolDelta {
                    index: 0,
                    id: Some("one".into()),
                    name: Some("work".into()),
                    arguments: "{}".into(),
                },
                ModelEvent::Finish(FinishReason::ToolCalls),
                ModelEvent::End,
            ]
        } else {
            vec![
                ModelEvent::Text("done".into()),
                ModelEvent::Finish(FinishReason::Stop),
                ModelEvent::End,
            ]
        };
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
struct Work;
#[async_trait]
impl Tool for Work {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "work".into(),
            description: "Test tool".into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            concurrency: ToolConcurrency::Exclusive,
            side_effects: false,
        }
    }
    async fn execute(&self, _: ToolContext, _: serde_json::Value) -> Result<ToolOutput> {
        Ok("x".repeat(1000).into())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ten_completed_runs_reliably_invoke_spill_cleanup_ten_times() {
    let store = Arc::new(Store::default());
    let mut host = HostBuilder::new()
        .model(Arc::new(FixtureModel))
        .plugin(Arc::new(SpillPlugin::new(
            store.clone(),
            SpillConfig::default(),
        )))
        .build()
        .await
        .unwrap();
    for _ in 0..10 {
        assert_eq!(
            host.engine()
                .execute(RunRequest::new("short"))
                .await
                .unwrap()
                .status,
            RunStatus::Completed
        );
    }
    assert_eq!(store.cleanups.lock().unwrap().len(), 10);
    assert!(store
        .cleanups
        .lock()
        .unwrap()
        .iter()
        .all(BTreeSet::is_empty));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn finishing_another_run_keeps_active_archives_pinned_and_cancel_releases_them() {
    let store = Arc::new(Store::default());
    let mut host = HostBuilder::new()
        .model(Arc::new(FixtureModel))
        .tool(Arc::new(Work))
        .plugin(Arc::new(SpillPlugin::new(
            store.clone(),
            SpillConfig::default(),
        )))
        .build()
        .await
        .unwrap();
    let mut request = RunRequest::new("hold");
    request.limits.max_tool_result_bytes = 512;
    let held = host.engine().start(request).unwrap();
    store.wrote.notified().await;
    assert_eq!(
        host.engine()
            .execute(RunRequest::new("other"))
            .await
            .unwrap()
            .status,
        RunStatus::Completed
    );
    assert!(store
        .cleanups
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains(&held.run_id));
    held.cancel();
    assert_eq!(held.wait().await.unwrap().status, RunStatus::Cancelled);
    assert!(store.cleanups.lock().unwrap().last().unwrap().is_empty());
    host.shutdown().await.unwrap();
}
