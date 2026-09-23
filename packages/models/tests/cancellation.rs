use api::*;
use std::{sync::{Arc, Mutex}, time::Duration};
use tokio::sync::mpsc;

#[derive(Default)]
struct Settings(Mutex<Option<Vec<u8>>>);
impl models::SettingsStore for Settings {
    fn load(&self) -> Result<Option<Vec<u8>>> { Ok(self.0.lock().unwrap().clone()) }
    fn save(&self, bytes: &[u8]) -> Result<()> { *self.0.lock().unwrap() = Some(bytes.to_vec()); Ok(()) }
}
struct WaitingModel(mpsc::UnboundedSender<CancellationToken>);
#[async_trait]
impl Model for WaitingModel {
    async fn stream(&self, _: ModelRequest, cancel: CancellationToken) -> Result<ModelStream> {
        self.0.send(cancel).unwrap();
        std::future::pending().await
    }
}
fn manager() -> models::ModelManager {
    let manager = models::ModelManager::open(Arc::new(Settings::default()), false).unwrap();
    manager.upsert(serde_json::from_value(serde_json::json!({
        "revision":0,"id":"fixture","name":"fixture","api_base":"https://example.invalid/v1",
        "models":[{"id":"never-called","enabled":true,"context_window_tokens":null}]
    })).unwrap()).unwrap();
    manager
}
async fn entered(rx: &mut mpsc::UnboundedReceiver<CancellationToken>) -> CancellationToken {
    tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap()
}
#[tokio::test]
async fn dropping_managed_execute_cancels_only_its_run_through_the_real_engine() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut host = runtime::HostBuilder::new().model(Arc::new(WaitingModel(tx))).build().await.unwrap();
    let managed = manager().runtime(Arc::new(host.engine()));
    let other = managed.start(RunRequest::new("independent")).unwrap();
    let other_cancel = entered(&mut rx).await;
    let request = RunRequest::new("owned"); let parent = request.task.clone();
    let waiting = managed.clone();
    let task = tokio::spawn(async move { waiting.execute(request).await });
    let owned_cancel = entered(&mut rx).await;
    task.abort(); assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), owned_cancel.cancelled()).await.unwrap();
    assert!(!other_cancel.is_cancelled()); assert!(!parent.cancellation().is_cancelled());
    other.cancel(); assert_eq!(other.wait().await.unwrap().status, RunStatus::Cancelled);
    host.shutdown().await.unwrap();
}
#[tokio::test]
async fn dropping_a_managed_start_handle_or_passive_wait_does_not_cancel_execution() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut host = runtime::HostBuilder::new().model(Arc::new(WaitingModel(tx))).build().await.unwrap();
    let managed = manager().runtime(Arc::new(host.engine()));
    let handle = managed.start(RunRequest::new("detached observer")).unwrap();
    let session = handle.session(); let cancel = entered(&mut rx).await;
    let observe = tokio::spawn(async move { handle.wait().await });
    tokio::task::yield_now().await; observe.abort(); assert!(observe.await.unwrap_err().is_cancelled());
    assert!(!cancel.is_cancelled());
    session.cancel(); assert_eq!(session.wait().await.unwrap().status, RunStatus::Cancelled);
    host.shutdown().await.unwrap();
}
