//! End-to-end exercise of the MSC serving two base stations at once: both
//! enroll over their management planes, the MSC dials each one's A1 address,
//! colliding wire call ids stay distinct inside the MSC, sends route to the
//! owning node, and one node detaching leaves the other untouched.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cdma_ios::transport::{A1TransportEvent, A1TransportSender};
use cdma_ios::{EncodedA1Message, Message, MessageType};
use cdma_msc::call_control::CallId;
use cdma_msc::config::BaseStationConfig;
use cdma_msc::grpc::base_station::v1::base_station_service_server::{
    BaseStationService, BaseStationServiceServer,
};
use cdma_msc::grpc::base_station::v1::{EnrollRequest, EnrollResponse, ServedCell};
use cdma_msc::grpc::bsc::v1::CellId as ProtoCellId;
use cdma_msc::{A1Event, BaseStationId, BaseStations, MscA1Endpoint};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};

const WAIT: Duration = Duration::from_secs(30);
const COLLIDING_WIRE_ID: u64 = 7;
const MSC_ALLOCATED_ID: u64 = 4242;

struct FakeEnrollService {
    node_id: &'static str,
    a1_addr: SocketAddr,
    cell: u32,
    sid: u32,
}

#[tonic::async_trait]
impl BaseStationService for FakeEnrollService {
    async fn enroll(
        &self,
        _request: Request<EnrollRequest>,
    ) -> Result<Response<EnrollResponse>, Status> {
        Ok(Response::new(EnrollResponse {
            node_id: self.node_id.to_string(),
            a1_addr: self.a1_addr.to_string(),
            cells: vec![ServedCell {
                cell: Some(ProtoCellId {
                    cell: self.cell,
                    sector: 0,
                }),
                sid: self.sid,
                nid: 1,
                mcc_digits: "310".to_string(),
                imsi_11_12_digits: "00".to_string(),
                in_service: true,
            }],
        }))
    }
}

type A1Link = (A1TransportSender, mpsc::Receiver<A1TransportEvent>);

/// One fake base station: an enrollment gRPC server plus an A1 listener that
/// hands each accepted connection to the test.
struct FakeNode {
    endpoint: String,
    links: mpsc::UnboundedReceiver<A1Link>,
}

async fn start_fake_node(node_id: &'static str, cell: u32, sid: u32) -> FakeNode {
    let a1_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a1_addr = a1_listener.local_addr().unwrap();
    let (links_tx, links) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok(link) = cdma_ios::transport::accept_with_retry(&a1_listener).await else {
                return;
            };
            if links_tx.send(link).is_err() {
                return;
            }
        }
    });

    let grpc_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", grpc_listener.local_addr().unwrap());
    let service = FakeEnrollService {
        node_id,
        a1_addr,
        cell,
        sid,
    };
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(BaseStationServiceServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(grpc_listener)),
    );

    FakeNode { endpoint, links }
}

fn message_with_call_id(call_id: u64) -> EncodedA1Message {
    EncodedA1Message::from_message(&Message::new(MessageType::PagingRequest, vec![1, 2, 3]))
        .with_call_id(Some(call_id))
}

async fn next_event(nodes: &Arc<BaseStations>) -> A1Event {
    tokio::time::timeout(WAIT, nodes.recv())
        .await
        .expect("timed out waiting for an A1 event")
        .expect("event stream ended")
}

async fn wait_attached(nodes: &Arc<BaseStations>) -> BaseStationId {
    loop {
        if let A1Event::Attached(node) = next_event(nodes).await {
            return node;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_base_stations_enroll_route_and_detach_independently() {
    let mut node_a = start_fake_node("bsc-a", 1, 100).await;
    let mut node_b = start_fake_node("bsc-b", 2, 200).await;

    let nodes = BaseStations::new(&[
        BaseStationConfig {
            management_endpoint: node_a.endpoint.clone(),
            id: None,
        },
        BaseStationConfig {
            management_endpoint: node_b.endpoint.clone(),
            id: None,
        },
    ]);
    nodes.spawn_attach_tasks();

    let first = wait_attached(&nodes).await;
    let second = wait_attached(&nodes).await;
    let mut attached = vec![first.0.clone(), second.0.clone()];
    attached.sort();
    assert_eq!(attached, ["bsc-a", "bsc-b"]);

    let (link_a, mut rx_a) = tokio::time::timeout(WAIT, node_a.links.recv())
        .await
        .unwrap()
        .expect("node a accepted the MSC");
    let (link_b, mut rx_b) = tokio::time::timeout(WAIT, node_b.links.recv())
        .await
        .unwrap()
        .expect("node b accepted the MSC");

    let id_a = BaseStationId("bsc-a".to_string());
    let id_b = BaseStationId("bsc-b".to_string());
    assert!(nodes.is_attached(&id_a));
    assert!(nodes.is_attached(&id_b));

    let summaries = nodes.summaries();
    assert_eq!(summaries.len(), 2);
    for summary in &summaries {
        assert!(summary.attached, "{summary:?}");
        let cells: Vec<u16> = summary.cells.iter().map(|c| c.cell.cell).collect();
        match summary.node_id.as_ref().map(|n| n.0.as_str()) {
            Some("bsc-a") => assert_eq!(cells, [1]),
            Some("bsc-b") => assert_eq!(cells, [2]),
            other => panic!("unexpected node {other:?}"),
        }
    }
    let served = nodes
        .served_cell(&id_b, cdma_ios::CellId { cell: 2, sector: 0 })
        .expect("node b's cell is enrolled");
    assert_eq!(served.sid, 200);

    // Both nodes use the same wire call id. The MSC must keep them apart.
    link_a
        .send(&message_with_call_id(COLLIDING_WIRE_ID))
        .await
        .unwrap();
    let (from_node, internal_a) = match next_event(&nodes).await {
        A1Event::Message { node, message } => (node, message.call_id().unwrap()),
        other => panic!("expected a message, got {other:?}"),
    };
    assert_eq!(from_node, id_a);
    link_b
        .send(&message_with_call_id(COLLIDING_WIRE_ID))
        .await
        .unwrap();
    let (from_node, internal_b) = match next_event(&nodes).await {
        A1Event::Message { node, message } => (node, message.call_id().unwrap()),
        other => panic!("expected a message, got {other:?}"),
    };
    assert_eq!(from_node, id_b);
    assert_ne!(internal_a, internal_b);
    assert_eq!(internal_a.min(internal_b), COLLIDING_WIRE_ID);

    // A call-scoped send comes out on the owning node with its wire id back.
    nodes.send(message_with_call_id(internal_b)).await.unwrap();
    match tokio::time::timeout(WAIT, rx_b.recv()).await.unwrap() {
        Some(A1TransportEvent::Message(message)) => {
            assert_eq!(message.call_id(), Some(COLLIDING_WIRE_ID));
        }
        other => panic!("expected node b to receive the send, got {other:?}"),
    }

    // An MSC-allocated id bound to a node routes there unchanged.
    nodes.bind_call(CallId(MSC_ALLOCATED_ID), &id_a);
    nodes
        .send(message_with_call_id(MSC_ALLOCATED_ID))
        .await
        .unwrap();
    match tokio::time::timeout(WAIT, rx_a.recv()).await.unwrap() {
        Some(A1TransportEvent::Message(message)) => {
            assert_eq!(message.call_id(), Some(MSC_ALLOCATED_ID));
        }
        other => panic!("expected node a to receive the send, got {other:?}"),
    }

    // Node a goes away. The MSC hands the runtime that node's calls, keeps
    // node b attached, and re-attaches node a when it comes back.
    drop(link_a);
    drop(rx_a);
    let (detached_node, mut dropped) = loop {
        match next_event(&nodes).await {
            A1Event::Detached { node, calls } => break (node, calls),
            A1Event::Message { .. } => continue,
            other => panic!("expected a detach, got {other:?}"),
        }
    };
    assert_eq!(detached_node, id_a);
    let mut dropped: Vec<u64> = dropped.drain(..).map(|call| call.0).collect();
    dropped.sort();
    let mut expected = vec![internal_a, MSC_ALLOCATED_ID];
    expected.sort();
    assert_eq!(dropped, expected);

    assert!(nodes.is_attached(&id_b));
    assert!(nodes.node_for_call(CallId(internal_b)).is_some());
    nodes.send(message_with_call_id(internal_b)).await.unwrap();
    match tokio::time::timeout(WAIT, rx_b.recv()).await.unwrap() {
        Some(A1TransportEvent::Message(message)) => {
            assert_eq!(message.call_id(), Some(COLLIDING_WIRE_ID));
        }
        other => panic!("expected node b to keep receiving, got {other:?}"),
    }

    assert_eq!(wait_attached(&nodes).await, id_a);
    let _relink = tokio::time::timeout(WAIT, node_a.links.recv())
        .await
        .unwrap()
        .expect("node a accepted the MSC again");
    assert!(nodes.is_attached(&id_a));
}
