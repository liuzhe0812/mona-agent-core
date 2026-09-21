use agent_api::{AgentError, CancellationToken, ErrorCode, Result};
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe, sync::{Mutex, MutexGuard}, time::Duration};
use tokio::time::{sleep_until, timeout, Instant};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(crate) struct CancelOnDrop(pub CancellationToken);
impl Drop for CancelOnDrop { fn drop(&mut self) { self.0.cancel(); } }

/// The future must be cooperative. This is not process isolation or rollback.
pub(crate) async fn bounded<T, F>(
    parent: &CancellationToken, operation: &CancellationToken,
    deadline: Instant, grace: Duration, future: F,
) -> Result<T>
where F: Future<Output = Result<T>> + Send, T: Send {
    let future = AssertUnwindSafe(future).catch_unwind();
    tokio::pin!(future);
    let reason = tokio::select! {
        biased;
        _ = parent.cancelled() => AgentError::new(ErrorCode::Cancelled, "operation cancelled"),
        _ = sleep_until(deadline) => AgentError::new(ErrorCode::Deadline, "operation deadline reached"),
        result = &mut future => return result.unwrap_or_else(|_| Err(AgentError::new(ErrorCode::Panic, "extension panicked"))),
    };
    operation.cancel();
    // Give a cooperative tool a bounded chance to stop/reap its own children.
    let _ = timeout(grace, &mut future).await;
    Err(reason)
}

pub(crate) async fn lifecycle<T, F>(duration: Duration, future: F) -> Result<T>
where F: Future<Output = Result<T>> + Send, T: Send {
    let cancel = CancellationToken::new();
    bounded(&cancel, &cancel, Instant::now() + duration, Duration::ZERO, future).await
}
