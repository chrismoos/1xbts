use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, transport::Server};

use crate::enricher::HlrEnricher;
use crate::proto::{
    EventSource, ListenEventsRequest, MobileIdentity, NetworkEvent, PublishAck, PublishRequest,
    Subscriber,
    event_service_server::{EventService, EventServiceServer},
};

#[derive(Debug, Clone)]
pub struct EventBusConfig {
    pub subscriber_queue_capacity: usize,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            subscriber_queue_capacity: 1024,
        }
    }
}

fn is_identity_empty(id: &MobileIdentity) -> bool {
    id.imsi.is_empty() && id.esn == 0 && id.meid.is_empty()
}

fn is_subscriber_empty(sub: &Subscriber) -> bool {
    sub.subscriber_id.is_empty()
        && sub.phone_number.is_empty()
        && sub.display_name.is_empty()
        && sub.status == 0
}

/// One registered ListenEvents subscriber. Not to be confused with the
/// proto `Subscriber` message (the HLR-resolved record carried inside
/// each event).
struct SubscriberSlot {
    filter: Vec<i32>,
    tx: mpsc::Sender<Result<NetworkEvent, Status>>,
}

#[derive(Clone)]
pub struct EventBusServer {
    inner: Arc<Inner>,
    enricher: Option<Arc<dyn HlrEnricher>>,
}

struct Inner {
    config: EventBusConfig,
    sequence: AtomicU64,
    next_subscriber_id: AtomicU64,
    // Keyed by stable per-subscriber ID so concurrent fan_out removals
    // can't accidentally target the wrong subscriber via shifting indices.
    subscribers: RwLock<HashMap<u64, SubscriberSlot>>,
}

impl EventBusServer {
    pub fn new(config: EventBusConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                sequence: AtomicU64::new(0),
                next_subscriber_id: AtomicU64::new(1),
                subscribers: RwLock::new(HashMap::new()),
            }),
            enricher: None,
        }
    }

    /// Attach an HLR enricher. When set, the bus awaits
    /// `enricher.enrich(...)` on every Publish before fan-out.
    pub fn with_enricher(mut self, enricher: Arc<dyn HlrEnricher>) -> Self {
        self.enricher = Some(enricher);
        self
    }

    pub fn into_service(self) -> EventServiceServer<Self> {
        EventServiceServer::new(self)
    }

    fn fan_out(&self, mut event: NetworkEvent) -> u64 {
        let seq = self.inner.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        event.sequence = seq;
        if event.timestamp.is_none() {
            event.timestamp = Some(prost_types::Timestamp::from(std::time::SystemTime::now()));
        }

        // Common path: read lock only. Collect the stable IDs of any
        // subscribers whose mpsc rejected the send (Full = slow,
        // Closed = dead). Escalate to a write lock only to remove them
        // by ID — index-based removal would race under concurrent
        // fan_out because swap_remove shifts unrelated entries.
        let mut dead: Vec<u64> = Vec::new();
        {
            let subs = self.inner.subscribers.read();
            for (id, sub) in subs.iter() {
                if !sub.filter.is_empty() && !sub.filter.contains(&event.source) {
                    continue;
                }
                match sub.tx.try_send(Ok(event.clone())) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        log::warn!(
                            "cdma-events: dropping slow subscriber {} (queue full at capacity {})",
                            id,
                            sub.tx.max_capacity()
                        );
                        dead.push(*id);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => dead.push(*id),
                }
            }
        }
        if !dead.is_empty() {
            let mut subs = self.inner.subscribers.write();
            for id in &dead {
                subs.remove(id);
            }
        }
        seq
    }
}

#[tonic::async_trait]
impl EventService for EventBusServer {
    type ListenEventsStream =
        Pin<Box<dyn Stream<Item = Result<NetworkEvent, Status>> + Send + 'static>>;

    async fn listen_events(
        &self,
        request: Request<ListenEventsRequest>,
    ) -> Result<Response<Self::ListenEventsStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = mpsc::channel(self.inner.config.subscriber_queue_capacity);
        let id = self
            .inner
            .next_subscriber_id
            .fetch_add(1, Ordering::Relaxed);
        {
            let mut subs = self.inner.subscribers.write();
            subs.insert(
                id,
                SubscriberSlot {
                    filter: req.source_filter,
                    tx,
                },
            );
        }
        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn publish(
        &self,
        request: Request<PublishRequest>,
    ) -> Result<Response<PublishAck>, Status> {
        let mut event = request
            .into_inner()
            .event
            .ok_or_else(|| Status::invalid_argument("PublishRequest.event is required"))?;
        if event.source == EventSource::Unspecified as i32 {
            return Err(Status::invalid_argument("NetworkEvent.source must be set"));
        }
        if let Some(enricher) = &self.enricher {
            let mut identity = event.identity.take().unwrap_or_default();
            let mut subscriber = event.subscriber.take().unwrap_or_default();
            enricher.enrich(&mut identity, &mut subscriber).await;
            // Only re-attach if any field is non-default — keeps the wire
            // event clean for system events that carry no mobile.
            if !is_identity_empty(&identity) {
                event.identity = Some(identity);
            }
            if !is_subscriber_empty(&subscriber) {
                event.subscriber = Some(subscriber);
            }
        }
        let seq = self.fan_out(event);
        Ok(Response::new(PublishAck {
            assigned_sequence: seq,
        }))
    }
}

#[cfg(test)]
impl EventBusServer {
    pub(crate) fn test_register_subscriber(
        &self,
        filter: Vec<i32>,
        capacity: usize,
    ) -> mpsc::Receiver<Result<NetworkEvent, Status>> {
        let (tx, rx) = mpsc::channel(capacity);
        let id = self
            .inner
            .next_subscriber_id
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .subscribers
            .write()
            .insert(id, SubscriberSlot { filter, tx });
        rx
    }

    pub(crate) fn test_subscriber_count(&self) -> usize {
        self.inner.subscribers.read().len()
    }

    pub(crate) fn test_publish(&self, event: NetworkEvent) -> u64 {
        self.fan_out(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{EventSource, PdsnNetworkEvent, network_event, pdsn_network_event};

    fn dummy_pdsn_event() -> NetworkEvent {
        NetworkEvent {
            timestamp: None,
            source: EventSource::Pdsn as i32,
            sequence: 0,
            producer_instance: "pdsn-test".into(),
            identity: None,
            subscriber: None,
            body: Some(network_event::Body::Pdsn(PdsnNetworkEvent {
                event: Some(pdsn_network_event::Event::Bound(
                    crate::proto::PacketSessionBound::default(),
                )),
            })),
        }
    }

    #[tokio::test]
    async fn slow_subscriber_is_dropped_after_queue_fills() {
        let bus = EventBusServer::new(EventBusConfig {
            subscriber_queue_capacity: 1,
        });
        let _rx = bus.test_register_subscriber(vec![], 1);
        assert_eq!(bus.test_subscriber_count(), 1);
        bus.test_publish(dummy_pdsn_event());
        assert_eq!(bus.test_subscriber_count(), 1);
        bus.test_publish(dummy_pdsn_event());
        assert_eq!(
            bus.test_subscriber_count(),
            0,
            "subscriber should be removed when its mpsc fills"
        );
    }

    /// Regression: removing dead subscribers must not collaterally damage
    /// the alive ones. Earlier implementation used Vec + swap_remove with
    /// indices captured under a read lock; swap_remove shifted unrelated
    /// entries and a second concurrent fan_out could remove the wrong
    /// subscriber. Keyed-by-ID removal makes this impossible.
    #[tokio::test]
    async fn removing_dead_subscriber_preserves_alive_ones() {
        let bus = EventBusServer::new(EventBusConfig {
            subscriber_queue_capacity: 16,
        });
        // Alive: capacity 16 so try_send always succeeds.
        let mut alive_rx = bus.test_register_subscriber(vec![], 16);
        // Slow: capacity 1, will fill on the second publish.
        let _slow_rx = bus.test_register_subscriber(vec![], 1);
        // Another alive.
        let mut alive2_rx = bus.test_register_subscriber(vec![], 16);
        assert_eq!(bus.test_subscriber_count(), 3);

        // First publish — everyone receives.
        bus.test_publish(dummy_pdsn_event());
        assert_eq!(bus.test_subscriber_count(), 3);

        // Second publish — the slow subscriber's mpsc is full, so it gets
        // dropped. The two alive subscribers must remain.
        bus.test_publish(dummy_pdsn_event());
        assert_eq!(bus.test_subscriber_count(), 2);

        // Alive subscribers must have received both events.
        for _ in 0..2 {
            alive_rx.recv().await.expect("alive sub closed").unwrap();
            alive2_rx.recv().await.expect("alive2 sub closed").unwrap();
        }
    }
}

/// Convenience: bind a gRPC server hosting the event bus on `addr` and
/// drive it until the future is dropped.
pub async fn serve(
    addr: std::net::SocketAddr,
    config: EventBusConfig,
) -> Result<(), tonic::transport::Error> {
    let bus = EventBusServer::new(config);
    Server::builder()
        .add_service(bus.into_service())
        .serve(addr)
        .await
}
