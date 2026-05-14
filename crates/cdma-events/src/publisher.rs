//! Lossy fire-and-forget event publisher.
//!
//! Callers `publish()` on the hot path and never block on the network.
//! Events land in a bounded ring buffer; a background task drains the ring
//! and issues unary `Publish` RPCs over a kept-warm gRPC channel. When the
//! ring is full, the oldest event is shed to make room for the newest —
//! events are timely-or-nothing, not durable.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex as PlMutex;
use tokio::sync::Notify;
use tonic::transport::{Channel, Endpoint};

use crate::proto::{NetworkEvent, PublishRequest, event_service_client::EventServiceClient};

#[derive(Debug, Clone)]
pub struct EventPublisherConfig {
    /// Bus endpoint, e.g. "http://127.0.0.1:17023".
    pub endpoint: String,
    /// Ring buffer capacity. Once full, each new event sheds the oldest.
    pub queue_capacity: usize,
    /// Backoff between reconnect attempts after a publish failure.
    pub reconnect_backoff: Duration,
    /// Per-call deadline on the unary Publish RPC. A hung server can't
    /// freeze the drainer beyond this.
    pub publish_timeout: Duration,
    /// Producer-identifying label written into NetworkEvent.producer_instance.
    pub producer_instance: String,
}

impl EventPublisherConfig {
    pub fn new(endpoint: impl Into<String>, producer_instance: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            queue_capacity: 2048,
            reconnect_backoff: Duration::from_secs(2),
            publish_timeout: Duration::from_secs(5),
            producer_instance: producer_instance.into(),
        }
    }
}

struct Ring {
    queue: PlMutex<VecDeque<NetworkEvent>>,
    notify: Notify,
    capacity: usize,
    shed_count: AtomicU64,
    drop_count: AtomicU64,
    high_water_since: PlMutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct EventPublisher {
    ring: Arc<Ring>,
    producer_instance: Arc<String>,
}

impl EventPublisher {
    /// Spawns a background publisher task after validating the endpoint
    /// URL. Returns immediately even if the bus isn't reachable yet — the
    /// drainer will retry-connect on each event. A malformed URL fails
    /// here at startup rather than silently in the drainer.
    pub fn spawn(config: EventPublisherConfig) -> Result<Self, EventPublisherError> {
        let endpoint = Endpoint::from_shared(config.endpoint.clone())
            .map_err(|e| EventPublisherError::InvalidEndpoint {
                endpoint: config.endpoint.clone(),
                source: e.to_string(),
            })?
            .connect_timeout(Duration::from_secs(5))
            .tcp_keepalive(Some(Duration::from_secs(30)));
        let ring = Arc::new(Ring {
            queue: PlMutex::new(VecDeque::with_capacity(config.queue_capacity)),
            notify: Notify::new(),
            capacity: config.queue_capacity,
            shed_count: AtomicU64::new(0),
            drop_count: AtomicU64::new(0),
            high_water_since: PlMutex::new(None),
        });
        let producer_instance = Arc::new(config.producer_instance.clone());
        tokio::spawn(run_publisher(config, endpoint, ring.clone()));
        Ok(Self {
            ring,
            producer_instance,
        })
    }

    pub fn producer_instance(&self) -> &str {
        &self.producer_instance
    }

    /// Pushes one event onto the ring. Never blocks. If the ring is full,
    /// the oldest queued event is shed to make room.
    pub fn publish(&self, event: NetworkEvent) {
        let mut q = self.ring.queue.lock();
        if q.len() >= self.ring.capacity {
            q.pop_front();
            let n = self.ring.shed_count.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                log::warn!(
                    "cdma-events publisher: shed {} events so far (ring full at {})",
                    n,
                    self.ring.capacity
                );
            }
        }
        q.push_back(event);
        let len = q.len();
        drop(q);
        self.observe_fill(len);
        self.ring.notify.notify_one();
    }

    /// Number of events shed because the ring was full at publish time.
    pub fn shed_count(&self) -> u64 {
        self.ring.shed_count.load(Ordering::Relaxed)
    }

    /// Number of events dropped because the unary Publish call failed.
    pub fn drop_count(&self) -> u64 {
        self.ring.drop_count.load(Ordering::Relaxed)
    }

    fn observe_fill(&self, len: usize) {
        let threshold = self.ring.capacity * 3 / 4;
        let mut high = self.ring.high_water_since.lock();
        if len >= threshold {
            let started = *high.get_or_insert_with(Instant::now);
            if started.elapsed() >= Duration::from_secs(5) {
                log::warn!(
                    "cdma-events publisher: ring >=75% full for {:?} (len={}, cap={}) — can't publish fast enough",
                    started.elapsed(),
                    len,
                    self.ring.capacity
                );
                // Reset so the warning fires at most every 5 seconds of sustained pressure.
                *high = Some(Instant::now());
            }
        } else {
            *high = None;
        }
    }
}

#[cfg(test)]
impl EventPublisher {
    /// Test-only: synchronous publish that doesn't spawn a drainer. Used to
    /// validate the shed-oldest behavior without standing up a gRPC bus.
    pub fn for_test(capacity: usize) -> Self {
        let ring = Arc::new(Ring {
            queue: PlMutex::new(VecDeque::with_capacity(capacity)),
            notify: Notify::new(),
            capacity,
            shed_count: AtomicU64::new(0),
            drop_count: AtomicU64::new(0),
            high_water_since: PlMutex::new(None),
        });
        Self {
            ring,
            producer_instance: Arc::new("test".to_string()),
        }
    }

    pub fn queue_len(&self) -> usize {
        self.ring.queue.lock().len()
    }

    pub fn queue_front_seq(&self) -> Option<u64> {
        self.ring.queue.lock().front().map(|e| e.sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{EventSource, NetworkEvent};

    fn event_with_seq(seq: u64) -> NetworkEvent {
        NetworkEvent {
            timestamp: None,
            source: EventSource::Pdsn as i32,
            sequence: seq,
            producer_instance: "t".into(),
            identity: None,
            subscriber: None,
            body: None,
        }
    }

    #[test]
    fn full_queue_sheds_oldest() {
        let pub_ = EventPublisher::for_test(3);
        for i in 0..3 {
            pub_.publish(event_with_seq(i));
        }
        assert_eq!(pub_.queue_len(), 3);
        assert_eq!(pub_.queue_front_seq(), Some(0));
        assert_eq!(pub_.shed_count(), 0);

        // Push a fourth — oldest (seq=0) must be shed.
        pub_.publish(event_with_seq(99));
        assert_eq!(pub_.queue_len(), 3);
        assert_eq!(pub_.queue_front_seq(), Some(1));
        assert_eq!(pub_.shed_count(), 1);

        // And again.
        pub_.publish(event_with_seq(100));
        assert_eq!(pub_.queue_front_seq(), Some(2));
        assert_eq!(pub_.shed_count(), 2);
    }
}

/// Reasons an `EventPublisher::spawn` may fail at construction time.
#[derive(Debug)]
pub enum EventPublisherError {
    InvalidEndpoint { endpoint: String, source: String },
}

impl std::fmt::Display for EventPublisherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEndpoint { endpoint, source } => {
                write!(f, "invalid event bus endpoint {endpoint:?}: {source}")
            }
        }
    }
}

impl std::error::Error for EventPublisherError {}

async fn run_publisher(config: EventPublisherConfig, endpoint: Endpoint, ring: Arc<Ring>) {
    let mut client: Option<EventServiceClient<Channel>> = None;

    loop {
        // Pop the next event under the lock (no await held).
        let next = ring.queue.lock().pop_front();
        let event = match next {
            Some(e) => e,
            None => {
                ring.notify.notified().await;
                continue;
            }
        };

        // Ensure we have a connected client; retry on connect failure.
        loop {
            if client.is_none() {
                match endpoint.connect().await {
                    Ok(channel) => client = Some(EventServiceClient::new(channel)),
                    Err(err) => {
                        log::warn!(
                            "cdma-events publisher: connect to {} failed: {}",
                            config.endpoint,
                            err
                        );
                        tokio::time::sleep(config.reconnect_backoff).await;
                        continue;
                    }
                }
            }
            break;
        }

        let c = client.as_mut().expect("connected client");
        let mut request = tonic::Request::new(PublishRequest { event: Some(event) });
        request.set_timeout(config.publish_timeout);
        if let Err(err) = c.publish(request).await {
            log::warn!(
                "cdma-events publisher: publish failed ({}); reconnecting",
                err
            );
            client = None;
            ring.drop_count.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(config.reconnect_backoff).await;
        }
    }
}
