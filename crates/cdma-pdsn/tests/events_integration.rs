//! End-to-end: PacketServiceImpl invokes the SessionLifecycleSink
//! (wrapping cdma-events EventPublisher) at session bind/unbind on the
//! actual live path. The bus is a real gRPC server.

use std::sync::Arc;
use std::time::Duration;

use cdma_events::proto::event_service_client::EventServiceClient;
use cdma_events::proto::{
    ListenEventsRequest, PacketSessionUnbindReason, network_event, pdsn_network_event,
};
use cdma_events::{
    EventBusConfig, EventBusServer, EventPublisher, EventPublisherConfig, EventServiceServer,
};
use cdma_packet::ip_transport::IpTransportConfig;
use cdma_packet::session_task::SessionMetadata;
use cdma_pdsn::events::PdsnLifecycleSink;
use tokio_stream::StreamExt;
use tonic::transport::Server;

async fn spawn_bus() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bus = EventBusServer::new(EventBusConfig::default());
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

#[tokio::test]
async fn live_session_bind_and_unbind_land_on_the_bus() {
    let addr = spawn_bus().await;
    let endpoint = format!("http://{addr}");

    // Subscribe before the producer fires.
    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Build the publisher → sink → packet service chain.
    let publisher = EventPublisher::spawn(EventPublisherConfig::new(endpoint, "pdsn-0"))
        .expect("valid endpoint");
    let sink: Arc<dyn cdma_packet::session_lifecycle::SessionLifecycleSink> =
        Arc::new(PdsnLifecycleSink::new(publisher));

    // FOU-TCP transport with a loopback that won't actually connect — fine,
    // the session task happily allocates the IP and emits the bound event
    // before any transport I/O matters for this assertion.
    let service = cdma_packet::grpc::PacketServiceImpl::new(
        IpTransportConfig::FouTcp {
            remote_addr: "127.0.0.1:1".parse().unwrap(),
        },
        None,
        Some(cdma_packet::fou_tcp_transport::FouTcpTunnel::new(
            "127.0.0.1:1".parse().unwrap(),
        )),
    )
    .with_lifecycle_sink(sink);

    let (uplink_tx, _downlink_rx) = service
        .open_session_direct(
            "session-test".to_string(),
            33,
            SessionMetadata {
                access_technology: "1x".to_string(),
                mobile_address: "10.0.0.99".to_string(),
                subscriber_id: Some("sub-7".to_string()),
                phone_number: "+15558675309".to_string(),
                imsi: Some("310170123456789".to_string()),
                esn: Some(0xDEAD_BEEF),
                meid: None,
                hrpd_mn_id: None,
                hrpd_mn_id_source: None,
                subscriber_imsi: None,
                traffic_walsh_code: 12,
            },
        )
        .expect("open session");

    // First event must be a PacketSessionBound with the real allocated IP.
    let ev1 = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("bound event arrives")
        .expect("stream item")
        .expect("ok");
    // Subscriber identity now lives in the envelope.
    let sub = ev1.subscriber.as_ref().expect("envelope subscriber");
    assert_eq!(sub.subscriber_id, "sub-7");
    let identity = ev1.identity.as_ref().expect("envelope identity");
    assert_eq!(identity.imsi, "310170123456789");
    assert_eq!(identity.esn, 0xDEAD_BEEF);
    match &ev1.body {
        Some(network_event::Body::Pdsn(p)) => match &p.event {
            Some(pdsn_network_event::Event::Bound(b)) => {
                assert_eq!(b.service_option, 33);
                assert!(
                    b.mobile_ip.starts_with("10.55.0."),
                    "expected default-subnet IP, got {:?}",
                    b.mobile_ip
                );
                assert_eq!(b.pdsn_ip, "10.55.0.1");
            }
            other => panic!("expected bound, got {other:?}"),
        },
        other => panic!("expected pdsn body, got {other:?}"),
    }

    // Close via the service API. `close_session_direct` is graceful:
    // it drops the cloned uplink sender, awaits the task, and lets
    // cleanup (and the lifecycle sink's `on_unbound`) run to completion.
    drop(uplink_tx);
    service.close_session_direct("session-test").await;

    let ev2 = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("unbound event arrives")
        .expect("stream item")
        .expect("ok");
    let sub2 = ev2.subscriber.as_ref().expect("envelope subscriber");
    assert_eq!(sub2.subscriber_id, "sub-7");
    match &ev2.body {
        Some(network_event::Body::Pdsn(p)) => match &p.event {
            Some(pdsn_network_event::Event::Unbound(u)) => {
                assert_eq!(u.reason, PacketSessionUnbindReason::UplinkClosed as i32);
            }
            other => panic!("expected unbound, got {other:?}"),
        },
        other => panic!("expected pdsn body, got {other:?}"),
    }
}

/// Roaming/unprovisioned mobile: no HLR record, so the BSC never
/// resolves a `subscriber_id`. The PDSN producer must emit the event
/// with `subscriber = None` and `identity` populated from IMSI/ESN so
/// the bus's forward-enrichment path is what fires.
#[tokio::test]
async fn unprovisioned_mobile_emits_identity_only() {
    let addr = spawn_bus().await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest::default())
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let publisher =
        EventPublisher::spawn(EventPublisherConfig::new(endpoint, "pdsn-0")).expect("endpoint");
    let sink: Arc<dyn cdma_packet::session_lifecycle::SessionLifecycleSink> =
        Arc::new(PdsnLifecycleSink::new(publisher));

    let service = cdma_packet::grpc::PacketServiceImpl::new(
        IpTransportConfig::FouTcp {
            remote_addr: "127.0.0.1:1".parse().unwrap(),
        },
        None,
        Some(cdma_packet::fou_tcp_transport::FouTcpTunnel::new(
            "127.0.0.1:1".parse().unwrap(),
        )),
    )
    .with_lifecycle_sink(sink);

    let (uplink_tx, _downlink_rx) = service
        .open_session_direct(
            "session-roam".to_string(),
            33,
            SessionMetadata {
                access_technology: "1x".to_string(),
                mobile_address: "10.0.0.50".to_string(),
                subscriber_id: None, // unprovisioned — no HLR record
                phone_number: String::new(),
                imsi: Some("310170555555555".to_string()),
                esn: Some(0xCAFE_BABE),
                meid: None,
                hrpd_mn_id: None,
                hrpd_mn_id_source: None,
                subscriber_imsi: None,
                traffic_walsh_code: 14,
            },
        )
        .expect("open session");

    let ev = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("bound event arrives")
        .expect("stream item")
        .expect("ok");
    assert!(
        ev.subscriber.is_none(),
        "roamer must have no envelope subscriber (got {:?})",
        ev.subscriber
    );
    let identity = ev.identity.expect("identity should still be present");
    assert_eq!(identity.imsi, "310170555555555");
    assert_eq!(identity.esn, 0xCAFE_BABE);

    drop(uplink_tx);
    service.close_session_direct("session-roam").await;
}
