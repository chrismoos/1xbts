//! Session lifecycle observation hook.
//!
//! `SessionLifecycleSink` lets a packet-service host plug in an external
//! observer that fires when a session crosses bind/unbind. The trait lives
//! in `cdma-packet` so it can be invoked from `session_task::run_session`
//! at the exact moments the assigned IP becomes real, without dragging in
//! any event-bus dependency. `cdma-pdsn` implements this trait to publish
//! to the aggregated event bus.

use std::net::Ipv4Addr;

/// Reason a packet session was unbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnbindReason {
    /// Mobile dropped the session — the uplink channel closed.
    UplinkClosed,
}

/// Snapshot of the binding moment: subscriber/identity context plus the
/// assigned IP. The downstream lifecycle sink decides how to translate
/// these into bus-level events; `subscriber_id` is what the BSC already
/// resolved (`None` for unprovisioned/roaming mobiles), and `imsi`/`esn`
/// are the raw radio identifiers if the producer has them.
#[derive(Debug, Clone)]
pub struct SessionBoundInfo {
    pub session_id: String,
    pub service_option: u32,
    pub subscriber_id: Option<String>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub peer_ip: Ipv4Addr,
    pub our_ip: Ipv4Addr,
}

/// Snapshot of the unbinding moment.
#[derive(Debug, Clone)]
pub struct SessionUnboundInfo {
    pub session_id: String,
    pub subscriber_id: Option<String>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub peer_ip: Ipv4Addr,
    pub reason: UnbindReason,
}

/// Observer for packet-session bind/unbind. Implementations must be cheap
/// to call from the session task — no blocking I/O on the call site.
pub trait SessionLifecycleSink: Send + Sync {
    fn on_bound(&self, info: SessionBoundInfo);
    fn on_unbound(&self, info: SessionUnboundInfo);
}

/// No-op sink used when no observer is configured. Lets call sites avoid
/// an `Option<Arc<dyn _>>` everywhere.
#[derive(Debug, Default)]
pub struct NullSink;

impl SessionLifecycleSink for NullSink {
    fn on_bound(&self, _: SessionBoundInfo) {}
    fn on_unbound(&self, _: SessionUnboundInfo) {}
}
