//! PDSN-side adapter to the aggregated event bus.
//!
//! Implements `cdma_packet::session_lifecycle::SessionLifecycleSink` so the
//! packet service can emit bind/unbind events at the exact moment the IP
//! is assigned or released — no separate plumbing needed. The adapter
//! publishes via gRPC (`cdma_events::EventPublisher`), never in-process.

use cdma_events::EventPublisher;
use cdma_events::proto::{
    EventSource, MobileIdentity, NetworkEvent, PacketSessionBound, PacketSessionUnbindReason,
    PacketSessionUnbound, PdsnNetworkEvent, Subscriber, network_event, pdsn_network_event,
};
use cdma_packet::session_lifecycle::{
    SessionBoundInfo, SessionLifecycleSink, SessionUnboundInfo, UnbindReason,
};

/// PDSN lifecycle sink that publishes packet-session events to the bus.
///
/// Producer fills in what it has (subscriber UUID from the BSC, plus
/// raw IMSI/ESN if available). The bus's `HlrEnricher` fills in the
/// missing fields before fan-out.
pub struct PdsnLifecycleSink {
    publisher: EventPublisher,
}

impl PdsnLifecycleSink {
    pub fn new(publisher: EventPublisher) -> Self {
        Self { publisher }
    }
}

impl SessionLifecycleSink for PdsnLifecycleSink {
    fn on_bound(&self, info: SessionBoundInfo) {
        let identity = build_identity(info.imsi.as_deref(), info.esn);
        let subscriber = build_subscriber(info.subscriber_id.as_deref());
        let body = pdsn_network_event::Event::Bound(PacketSessionBound {
            session_id: info.session_id,
            mobile_ip: info.peer_ip.to_string(),
            pdsn_ip: info.our_ip.to_string(),
            // Variant numbers equal the standard CDMA SO wire value, so
            // a raw `as i32` cast is correct even when no enum variant
            // matches.
            service_option: info.service_option as i32,
        });
        self.publisher
            .publish(envelope(&self.publisher, identity, subscriber, body));
    }

    fn on_unbound(&self, info: SessionUnboundInfo) {
        let identity = build_identity(info.imsi.as_deref(), info.esn);
        let subscriber = build_subscriber(info.subscriber_id.as_deref());
        let reason = match info.reason {
            UnbindReason::UplinkClosed => PacketSessionUnbindReason::UplinkClosed,
        };
        let body = pdsn_network_event::Event::Unbound(PacketSessionUnbound {
            session_id: info.session_id,
            mobile_ip: info.peer_ip.to_string(),
            reason: reason as i32,
        });
        self.publisher
            .publish(envelope(&self.publisher, identity, subscriber, body));
    }
}

fn build_identity(imsi: Option<&str>, esn: Option<u32>) -> Option<MobileIdentity> {
    let imsi = imsi.unwrap_or("");
    let esn = esn.unwrap_or(0);
    if imsi.is_empty() && esn == 0 {
        return None;
    }
    Some(MobileIdentity {
        imsi: imsi.to_string(),
        esn,
        meid: String::new(),
    })
}

fn build_subscriber(subscriber_id: Option<&str>) -> Option<Subscriber> {
    subscriber_id.map(|id| Subscriber {
        subscriber_id: id.to_string(),
        ..Subscriber::default()
    })
}

fn envelope(
    publisher: &EventPublisher,
    identity: Option<MobileIdentity>,
    subscriber: Option<Subscriber>,
    body: pdsn_network_event::Event,
) -> NetworkEvent {
    NetworkEvent {
        timestamp: None,
        source: EventSource::Pdsn as i32,
        sequence: 0,
        producer_instance: publisher.producer_instance().to_string(),
        identity,
        subscriber,
        body: Some(network_event::Body::Pdsn(PdsnNetworkEvent {
            event: Some(body),
        })),
    }
}
