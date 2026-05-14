//! Integration tests for the aggregated event bus.
//!
//! Each test spins up the gRPC server on a localhost port, connects one or
//! more publisher clients and one or more `ListenEvents` subscribers, and
//! asserts the contract.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cdma_events::proto::event_service_client::EventServiceClient;
use cdma_events::proto::{
    BscNetworkEvent, EventSource, ListenEventsRequest, MobileIdentity, NetworkEvent,
    PdsnNetworkEvent, PublishRequest, Subscriber, network_event, pdsn_network_event,
};
use cdma_events::{EventBusConfig, EventBusServer, EventServiceServer, HlrEnricher};
use tokio_stream::StreamExt;
use tonic::transport::Server;

async fn spawn_bus(queue_capacity: usize) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bus = EventBusServer::new(EventBusConfig {
        subscriber_queue_capacity: queue_capacity,
    });
    let stream = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        Server::builder()
            .add_service(EventServiceServer::new(bus))
            .serve_with_incoming(stream)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

fn pdsn_dummy_event(source_instance: &str, mobile_ip: &str) -> NetworkEvent {
    NetworkEvent {
        timestamp: None,
        source: EventSource::Pdsn as i32,
        sequence: 0,
        producer_instance: source_instance.into(),
        identity: None,
        subscriber: None,
        body: Some(network_event::Body::Pdsn(PdsnNetworkEvent {
            event: Some(pdsn_network_event::Event::Bound(
                cdma_events::proto::PacketSessionBound {
                    session_id: "session-1".into(),
                    mobile_ip: mobile_ip.into(),
                    pdsn_ip: "10.55.0.1".into(),
                    service_option: 33,
                },
            )),
        })),
    }
}

#[tokio::test]
async fn fans_out_to_two_subscribers_with_monotonic_sequence() {
    let addr = spawn_bus(1024).await;
    let endpoint = format!("http://{addr}");

    let mut sub_a = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut sub_b = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut a_stream = sub_a
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    let mut b_stream = sub_b
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut pub_client = EventServiceClient::connect(endpoint).await.unwrap();
    for i in 0..5 {
        let ack = pub_client
            .publish(PublishRequest {
                event: Some(pdsn_dummy_event("pdsn-0", &format!("10.0.0.{i}"))),
            })
            .await
            .unwrap();
        assert_eq!(ack.into_inner().assigned_sequence, (i + 1) as u64);
    }

    for expected_seq in 1..=5u64 {
        let ev_a = a_stream.next().await.unwrap().unwrap();
        let ev_b = b_stream.next().await.unwrap().unwrap();
        assert_eq!(ev_a.sequence, expected_seq);
        assert_eq!(ev_b.sequence, expected_seq);
        assert_eq!(ev_a.source, EventSource::Pdsn as i32);
    }
}

#[tokio::test]
async fn source_filter_is_respected() {
    let addr = spawn_bus(1024).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest {
            source_filter: vec![EventSource::Pdsn as i32],
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut pub_client = EventServiceClient::connect(endpoint).await.unwrap();
    pub_client
        .publish(PublishRequest {
            event: Some(pdsn_dummy_event("pdsn-0", "10.0.0.1")),
        })
        .await
        .unwrap();
    pub_client
        .publish(PublishRequest {
            event: Some(NetworkEvent {
                timestamp: None,
                source: EventSource::Bsc as i32,
                sequence: 0,
                producer_instance: "bsc-0".into(),
                identity: None,
                subscriber: None,
                body: Some(network_event::Body::Bsc(BscNetworkEvent::default())),
            }),
        })
        .await
        .unwrap();

    let ev = stream.next().await.unwrap().unwrap();
    assert_eq!(ev.source, EventSource::Pdsn as i32);
    let nothing = tokio::time::timeout(Duration::from_millis(300), stream.next()).await;
    assert!(nothing.is_err(), "BSC event must be filtered out");
}

// ─── HlrEnricher direction tests ─────────────────────────────────────

/// Stub enricher records the direction it was asked to resolve and
/// returns a canned result.
struct StubEnricher {
    forward_calls: AtomicUsize,
    reverse_calls: AtomicUsize,
    forward_imsi: String,
    canned_subscriber: Subscriber,
    canned_identity: MobileIdentity,
}

impl StubEnricher {
    fn new(forward_imsi: &str, sub: Subscriber, ident: MobileIdentity) -> Self {
        Self {
            forward_calls: AtomicUsize::new(0),
            reverse_calls: AtomicUsize::new(0),
            forward_imsi: forward_imsi.to_string(),
            canned_subscriber: sub,
            canned_identity: ident,
        }
    }
}

#[tonic::async_trait]
impl HlrEnricher for StubEnricher {
    async fn enrich(&self, identity: &mut MobileIdentity, subscriber: &mut Subscriber) {
        let has_id = !identity.imsi.is_empty() || identity.esn != 0;
        let has_sub = !subscriber.subscriber_id.is_empty();
        if has_id && !has_sub {
            self.forward_calls.fetch_add(1, Ordering::SeqCst);
            if identity.imsi == self.forward_imsi {
                *subscriber = self.canned_subscriber.clone();
            }
        } else if has_sub && !has_id {
            self.reverse_calls.fetch_add(1, Ordering::SeqCst);
            *identity = self.canned_identity.clone();
        }
    }
}

async fn spawn_bus_with_enricher(enricher: Arc<dyn HlrEnricher>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bus = EventBusServer::new(EventBusConfig::default()).with_enricher(enricher);
    let stream = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        Server::builder()
            .add_service(bus.into_service())
            .serve_with_incoming(stream)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

fn empty_pdsn_event(producer: &str) -> NetworkEvent {
    NetworkEvent {
        timestamp: None,
        source: EventSource::Pdsn as i32,
        sequence: 0,
        producer_instance: producer.into(),
        identity: None,
        subscriber: None,
        body: Some(network_event::Body::Pdsn(PdsnNetworkEvent::default())),
    }
}

#[tokio::test]
async fn enricher_forward_identity_to_subscriber() {
    let canned_sub = Subscriber {
        subscriber_id: "sub-uuid-1".into(),
        phone_number: "+15555550101".into(),
        display_name: "Alice".into(),
        status: cdma_events::proto::SubscriberStatus::Active as i32,
    };
    let stub = Arc::new(StubEnricher::new(
        "310170111111111",
        canned_sub.clone(),
        MobileIdentity::default(),
    ));
    let addr = spawn_bus_with_enricher(stub.clone()).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut publisher = EventServiceClient::connect(endpoint).await.unwrap();
    let mut event = empty_pdsn_event("pdsn-0");
    event.identity = Some(MobileIdentity {
        imsi: "310170111111111".into(),
        esn: 0,
        meid: String::new(),
    });
    publisher
        .publish(PublishRequest { event: Some(event) })
        .await
        .unwrap();

    let ev = stream.next().await.unwrap().unwrap();
    let subscriber = ev
        .subscriber
        .expect("enrichment should populate subscriber");
    assert_eq!(subscriber.subscriber_id, "sub-uuid-1");
    assert_eq!(subscriber.display_name, "Alice");
    assert_eq!(stub.forward_calls.load(Ordering::SeqCst), 1);
    assert_eq!(stub.reverse_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn enricher_reverse_subscriber_to_identity() {
    let canned_identity = MobileIdentity {
        imsi: "310170222222222".into(),
        esn: 0xABCD_1234,
        meid: String::new(),
    };
    let stub = Arc::new(StubEnricher::new(
        "",
        Subscriber::default(),
        canned_identity.clone(),
    ));
    let addr = spawn_bus_with_enricher(stub.clone()).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut publisher = EventServiceClient::connect(endpoint).await.unwrap();
    let mut event = empty_pdsn_event("pdsn-0");
    event.subscriber = Some(Subscriber {
        subscriber_id: "sub-uuid-2".into(),
        ..Subscriber::default()
    });
    publisher
        .publish(PublishRequest { event: Some(event) })
        .await
        .unwrap();

    let ev = stream.next().await.unwrap().unwrap();
    let identity = ev.identity.expect("enrichment should populate identity");
    assert_eq!(identity.imsi, "310170222222222");
    assert_eq!(identity.esn, 0xABCD_1234);
    assert_eq!(stub.forward_calls.load(Ordering::SeqCst), 0);
    assert_eq!(stub.reverse_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn enricher_skipped_when_both_pre_populated() {
    let stub = Arc::new(StubEnricher::new(
        "anything",
        Subscriber::default(),
        MobileIdentity::default(),
    ));
    let addr = spawn_bus_with_enricher(stub.clone()).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut publisher = EventServiceClient::connect(endpoint).await.unwrap();
    let mut event = empty_pdsn_event("pdsn-0");
    event.identity = Some(MobileIdentity {
        imsi: "310170999999999".into(),
        ..MobileIdentity::default()
    });
    event.subscriber = Some(Subscriber {
        subscriber_id: "sub-uuid-3".into(),
        display_name: "Bob".into(),
        ..Subscriber::default()
    });
    publisher
        .publish(PublishRequest { event: Some(event) })
        .await
        .unwrap();

    let ev = stream.next().await.unwrap().unwrap();
    assert_eq!(ev.identity.unwrap().imsi, "310170999999999");
    assert_eq!(ev.subscriber.unwrap().display_name, "Bob");
    // Neither direction should have fired.
    assert_eq!(stub.forward_calls.load(Ordering::SeqCst), 0);
    assert_eq!(stub.reverse_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn enricher_roamer_no_subscriber_record() {
    // Stub only recognizes a specific IMSI; ours doesn't match → no
    // subscriber returned. The event must still flow with `identity`
    // and an empty `subscriber`.
    let stub = Arc::new(StubEnricher::new(
        "310170111111111",
        Subscriber {
            subscriber_id: "would-not-match".into(),
            ..Subscriber::default()
        },
        MobileIdentity::default(),
    ));
    let addr = spawn_bus_with_enricher(stub.clone()).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut publisher = EventServiceClient::connect(endpoint).await.unwrap();
    let mut event = empty_pdsn_event("pdsn-0");
    event.identity = Some(MobileIdentity {
        imsi: "999999999999999".into(), // unknown to the stub
        ..MobileIdentity::default()
    });
    publisher
        .publish(PublishRequest { event: Some(event) })
        .await
        .unwrap();

    let ev = stream.next().await.unwrap().unwrap();
    assert_eq!(ev.identity.unwrap().imsi, "999999999999999");
    assert!(
        ev.subscriber.is_none(),
        "roamer must come through with no subscriber"
    );
    assert_eq!(stub.forward_calls.load(Ordering::SeqCst), 1);
}
