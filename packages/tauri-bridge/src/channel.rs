use application::*;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{Arc, Weak, Mutex, MutexGuard, atomic::{AtomicU64, Ordering}}, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelPacket { pub subscription_id: String, pub delivery_id: u64, pub frame: StreamFrame }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelSubscription { pub subscription_id: String }
pub trait ChannelSink: Send + Sync {
    fn send(&self, packet: ChannelPacket) -> ApplicationResult<()>;
}
struct Ticket { owner: String, cancel: CancellationToken, acknowledged: watch::Sender<u64>, sent: u64 }
struct Inner {
    app: AgentApplication, tickets: Mutex<HashMap<String, Ticket>>, ack_timeout: Duration,
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> { mutex.lock().unwrap_or_else(|p| p.into_inner()) }
impl Drop for Inner {
    fn drop(&mut self) { for ticket in lock(&self.tickets).values() { ticket.cancel.cancel(); } }
}
struct TicketLease { inner: Weak<Inner>, id: String }
impl Drop for TicketLease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() { lock(&inner.tickets).remove(&self.id); }
    }
}
#[derive(Clone)]
pub struct ChannelBridge { inner: Arc<Inner> }
impl ChannelBridge {
    pub fn new(app: AgentApplication, ack_timeout: Duration) -> ApplicationResult<Self> {
        if ack_timeout.is_zero() { return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "ack timeout must be positive")); }
        Ok(Self { inner: Arc::new(Inner { app, tickets: Mutex::new(HashMap::new()), ack_timeout }) })
    }
    pub fn application(&self) -> &AgentApplication { &self.inner.app }
    pub fn subscribe(&self, owner: &str, run_id: &str, after: Option<u64>, sink: Arc<dyn ChannelSink>) -> ApplicationResult<ChannelSubscription> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| ApplicationError::new(ApplicationErrorCode::Internal, "a Tokio runtime is required"))?;
        let mut subscription = self.inner.app.subscribe_events(run_id, after)?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = format!("subscription-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let cancel = CancellationToken::new();
        let (acknowledged, mut ack) = watch::channel(0);
        lock(&self.inner.tickets).insert(id.clone(), Ticket { owner: owner.to_owned(), cancel: cancel.clone(), acknowledged, sent: 0 });
        let weak = Arc::downgrade(&self.inner);
        let subscription_id = id.clone(); let duration = self.inner.ack_timeout;
        handle.spawn(async move {
            let _lease = TicketLease { inner: weak.clone(), id: subscription_id.clone() };
            let mut delivery = 0u64;
            loop {
                let frame = tokio::select! { biased; _ = cancel.cancelled() => break, frame = subscription.next() => frame };
                let Some(frame) = frame else { break; };
                delivery += 1;
                // Reserve the sent sequence BEFORE Channel::send: JS may acknowledge immediately.
                {
                    let Some(inner) = weak.upgrade() else { break; };
                    let mut tickets = lock(&inner.tickets);
                    let Some(ticket) = tickets.get_mut(&subscription_id) else { break; };
                    ticket.sent = delivery;
                }
                if sink.send(ChannelPacket { subscription_id: subscription_id.clone(), delivery_id: delivery, frame }).is_err() { break; }
                // At most ONE IPC packet in flight. Slow JS cannot create an unbounded WebView queue.
                let acknowledged = tokio::time::timeout(duration, async {
                    loop {
                        if *ack.borrow() >= delivery { return true; }
                        tokio::select! {
                            _ = cancel.cancelled() => return false,
                            changed = ack.changed() => if changed.is_err() { return false; },
                        }
                    }
                }).await.unwrap_or(false);
                if !acknowledged { break; }
            }
        });
        Ok(ChannelSubscription { subscription_id: id })
    }
    pub fn acknowledge(&self, owner: &str, id: &str, delivery_id: u64) -> ApplicationResult<()> {
        let tickets = lock(&self.inner.tickets);
        let ticket = tickets.get(id).filter(|t| t.owner == owner).ok_or_else(|| ApplicationError::new(ApplicationErrorCode::NotFound, "subscription not found for this webview"))?;
        if delivery_id == 0 || delivery_id > ticket.sent {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest, "acknowledgement exceeds sent delivery"));
        }
        ticket.acknowledged.send_modify(|last| *last = (*last).max(delivery_id));
        Ok(())
    }
    pub fn unsubscribe(&self, owner: &str, id: &str) -> ApplicationResult<()> {
        let mut tickets = lock(&self.inner.tickets);
        if let Some(ticket) = tickets.get(id) {
            if ticket.owner != owner { return Err(ApplicationError::new(ApplicationErrorCode::NotFound, "subscription not found for this webview")); }
        } else { return Ok(()); }
        if let Some(ticket) = tickets.remove(id) { ticket.cancel.cancel(); }
        Ok(())
    }
    pub fn detach_owner(&self, owner: &str) {
        let mut tickets = lock(&self.inner.tickets);
        tickets.retain(|_, ticket| {
            if ticket.owner == owner { ticket.cancel.cancel(); false } else { true }
        });
    }
}
