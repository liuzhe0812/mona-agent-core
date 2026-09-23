//! Durable linear sessions and acknowledged working context, independent of Web and Runtime.
#![forbid(unsafe_code)]
mod error;
mod lifecycle;
pub mod references;
mod store;

pub use error::{SessionError, SessionErrorCode, SessionResult};
pub use lifecycle::{runtime, SessionSink};
pub use store::{Document, Header, Listing, Prepared, Status, Store, Turn, SESSION_KEY, TURN_KEY};

pub(crate) async fn disk<T: Send + 'static>(
    action: impl FnOnce() -> SessionResult<T> + Send + 'static,
) -> SessionResult<T> {
    tokio::task::spawn_blocking(action).await.map_err(|_| {
        SessionError::new(SessionErrorCode::Internal, "session storage worker failed")
    })?
}
