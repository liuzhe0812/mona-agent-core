use api::*;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
struct Session { cancels: AtomicUsize, complete: bool }
#[async_trait]
impl RunSession for Session {
    fn run_id(&self) -> &str { "owned-wait" }
    fn cancel(&self) { self.cancels.fetch_add(1, Ordering::SeqCst); }
    fn snapshot(&self) -> RunSnapshot { RunSnapshot::new(self.run_id()) }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventEnvelope> { tokio::sync::broadcast::channel(1).1 }
    async fn steer(&self, _: String) -> Result<()> { Ok(()) }
    async fn wait(&self) -> Result<Arc<RunReport>> {
        if !self.complete { return std::future::pending().await; }
        Ok(Arc::new(RunReport { run_id:self.run_id().into(), status:RunStatus::Completed,
            output:Some("done".into()), error:None, transcript:vec![], model_requests:vec![],
            task_usage:TaskControl::default().usage(), steps:1, statistics:RunStatistics::default(), checkpoint:Default::default() }))
    }
}
#[tokio::test]
async fn owned_wait_cancels_even_if_dropped_before_first_poll_but_not_after_success() {
    let session = Arc::new(Session { cancels:AtomicUsize::new(0), complete:false });
    drop(RunHandle::new(session.clone(), session.subscribe()).wait_owned());
    assert_eq!(session.cancels.load(Ordering::SeqCst), 1);
    let done = Arc::new(Session { cancels:AtomicUsize::new(0), complete:true });
    RunHandle::new(done.clone(), done.subscribe()).wait_owned().await.unwrap();
    assert_eq!(done.cancels.load(Ordering::SeqCst), 0);
    drop(RunHandle::new(session.clone(), session.subscribe()));
    assert_eq!(session.cancels.load(Ordering::SeqCst), 1);
}
