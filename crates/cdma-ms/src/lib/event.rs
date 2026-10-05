use std::sync::Arc;
use std::sync::mpsc;

use crate::ms::MsEvent;

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &MsEvent);
}

pub struct ChannelSink {
    tx: mpsc::Sender<MsEvent>,
}

impl ChannelSink {
    pub fn new() -> (Arc<ChannelSink>, mpsc::Receiver<MsEvent>) {
        let (tx, rx) = mpsc::channel();
        (Arc::new(ChannelSink { tx }), rx)
    }
}

impl EventSink for ChannelSink {
    fn emit(&self, event: &MsEvent) {
        let _ = self.tx.send(event.clone());
    }
}

pub struct BroadcastSink {
    tx: tokio::sync::broadcast::Sender<MsEvent>,
}

impl BroadcastSink {
    pub fn new(tx: tokio::sync::broadcast::Sender<MsEvent>) -> Arc<BroadcastSink> {
        Arc::new(BroadcastSink { tx })
    }
}

impl EventSink for BroadcastSink {
    fn emit(&self, event: &MsEvent) {
        let _ = self.tx.send(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ms::MsCore;
    use crate::tx::MsAccessIdentity;

    #[test]
    fn channel_sink_receives_core_events_without_async() {
        let (sink, rx) = ChannelSink::new();
        let mut core = MsCore::new(MsAccessIdentity {
            esn: 1,
            imsi_s: 2,
            ..Default::default()
        });
        core.add_event_sink(sink);
        core.power_on();
        let events: Vec<MsEvent> = rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MsEvent::StateChange { .. })),
            "expected state-change events, got {events:?}"
        );
    }
}
