use crate::{protocol::*, service::{lock, Entry}};
use std::sync::Arc;
use tokio::sync::{watch, OwnedSemaphorePermit};

/// Pull-based replay/live subscription. No per-client producer or unbounded queue.
/// Dropping it detaches the client only; it never cancels the task.
pub struct Subscription {
    entry: Arc<Entry>, changed: watch::Receiver<u64>, cursor: u64,
    initial: bool, ended: bool, _permit: OwnedSemaphorePermit,
}
impl Subscription {
    pub(crate) fn new(entry: Arc<Entry>, after: Option<u64>, permit: OwnedSemaphorePermit) -> ApplicationResult<Self> {
        // Subscribe to revisions BEFORE inspecting state (no lost-wakeup window).
        let changed = entry.changed.subscribe();
        if after.is_some_and(|after| after > lock(&entry.data).snapshot.seq) {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "cursor is ahead of this run"));
        }
        Ok(Self { entry, changed, cursor: after.unwrap_or(0), initial: after.is_none(), ended: false, _permit: permit })
    }
    pub async fn next(&mut self) -> Option<StreamFrame> {
        loop {
            if self.ended { return None; }
            let frame = {
                let data = lock(&self.entry.data);
                let oldest = data.journal.front().map(|(e, _)| e.seq).unwrap_or(data.snapshot.seq + 1);
                let reason = if self.initial { Some(SnapshotReason::Initial) }
                    else if self.cursor < data.reset_floor { Some(SnapshotReason::SourceResync) }
                    else if self.cursor + 1 < oldest { Some(SnapshotReason::CursorExpired) }
                    else { None };
                if let Some(reason) = reason {
                    Some(StreamFrame::Snapshot { reason, snapshot: data.snapshot.clone() })
                } else if let Some((envelope, _)) = data.journal.iter().find(|(e, _)| e.seq > self.cursor) {
                    Some(StreamFrame::Event { envelope: envelope.clone() })
                } else if let Some(error) = &data.fault {
                    self.ended = true; Some(StreamFrame::Fault { error: error.clone() })
                } else if data.snapshot.outcome.is_some() {
                    self.ended = true; None
                } else { None }
            };
            if let Some(frame) = frame {
                self.initial = false;
                if let Some(seq) = frame.sequence() { self.cursor = seq; }
                return Some(frame);
            }
            if self.ended || self.changed.changed().await.is_err() { self.ended = true; return None; }
        }
    }
}
