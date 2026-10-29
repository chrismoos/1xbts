//! TCP-backed [`MscClient`] that serves A1 signaling to the MSC.
//!
//! The MSC initiates A1: it pulls this node's enrollment over gRPC, then
//! dials the address the enrollment names. The BSC listens and accepts one
//! MSC at a time, re-accepting after the MSC goes away so an MSC restart does
//! not strand the cell.

use std::net::SocketAddr;

use log::{info, warn};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use cdma_ios::transport::{A1TransportEvent, A1TransportSender};
use cdma_ios::{A1TransportError, EncodedA1Message};

use super::{MscClient, events::A1EventSink};

pub struct NetworkMscClient {
    listener: TcpListener,
    event_sink: Option<A1EventSink>,
    sender: Mutex<Option<A1TransportSender>>,
    events_rx: Mutex<Option<tokio::sync::mpsc::Receiver<A1TransportEvent>>>,
}

impl NetworkMscClient {
    /// Binds the A1 signaling socket. The MSC connection is accepted lazily,
    /// on the first poll.
    pub async fn bind(addr: SocketAddr) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            event_sink: None,
            sender: Mutex::new(None),
            events_rx: Mutex::new(None),
        })
    }

    pub fn with_events(mut self, publisher: cdma_events::EventPublisher, node_id: String) -> Self {
        self.event_sink = Some(A1EventSink::new(publisher, node_id));
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }

    async fn drop_link(&self) {
        *self.sender.lock().await = None;
        if let Some(sink) = &self.event_sink {
            sink.clear();
        }
    }
}

#[tonic::async_trait]
impl MscClient for NetworkMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        let sender = self.sender.lock().await.clone();
        match sender {
            Some(sender) => {
                sender.send(&message).await?;
                if let Some(sink) = &self.event_sink {
                    sink.publish(&message, cdma_events::proto::A1Direction::Uplink);
                }
                Ok(())
            }
            None => Err(A1TransportError::Closed),
        }
    }

    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, A1TransportError> {
        let mut events_rx = self.events_rx.lock().await;
        loop {
            if events_rx.is_none() {
                let (sender, rx) = cdma_ios::transport::accept_with_retry(&self.listener)
                    .await
                    .map_err(A1TransportError::Io)?;
                info!("BSC accepted A1 signaling connection from MSC");
                *self.sender.lock().await = Some(sender);
                *events_rx = Some(rx);
            }
            let rx = events_rx.as_mut().expect("A1 link established above");
            match rx.recv().await {
                Some(A1TransportEvent::Message(message)) => {
                    if let Some(sink) = &self.event_sink {
                        sink.publish(&message, cdma_events::proto::A1Direction::Downlink);
                    }
                    return Ok(Some(message));
                }
                Some(A1TransportEvent::Disconnected(error)) => {
                    warn!("BSC A1 endpoint: MSC disconnected: {error}, awaiting reconnect");
                    *events_rx = None;
                    self.drop_link().await;
                }
                None => {
                    warn!("BSC A1 endpoint: transport closed, awaiting reconnect");
                    *events_rx = None;
                    self.drop_link().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepts_the_msc_and_relays_both_ways() {
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = client.local_addr().unwrap();
        let (msc_sender, mut msc_rx) = cdma_ios::transport::connect(addr).await.unwrap();

        let inbound = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::PagingRequest,
            vec![1, 2, 3],
        ));
        msc_sender.send(&inbound).await.unwrap();
        let received = client.poll_a1().await.unwrap().expect("message");
        assert_eq!(
            received.message_type(),
            cdma_ios::MessageType::PagingRequest
        );

        let outbound = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::CompleteLayer3Information,
            vec![9],
        ));
        client.send_a1(outbound).await.unwrap();
        match msc_rx.recv().await {
            Some(A1TransportEvent::Message(message)) => assert_eq!(
                message.message_type(),
                cdma_ios::MessageType::CompleteLayer3Information
            ),
            other => panic!("expected a message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_bsc_accepts_a_replacement_msc_after_a_disconnect() {
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = client.local_addr().unwrap();

        let (first_sender, first_rx) = cdma_ios::transport::connect(addr).await.unwrap();
        let first = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::PagingRequest,
            vec![1],
        ));
        first_sender.send(&first).await.unwrap();
        assert_eq!(
            client
                .poll_a1()
                .await
                .unwrap()
                .expect("first message")
                .message_type(),
            cdma_ios::MessageType::PagingRequest
        );
        drop(first_sender);
        drop(first_rx);

        let (second_sender, _second_rx) = cdma_ios::transport::connect(addr).await.unwrap();
        let second = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::CompleteLayer3Information,
            vec![2],
        ));
        second_sender.send(&second).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), client.poll_a1())
            .await
            .expect("the BSC re-accepts once the first MSC goes away")
            .unwrap()
            .expect("second message");
        assert_eq!(
            received.message_type(),
            cdma_ios::MessageType::CompleteLayer3Information
        );
    }

    #[tokio::test]
    async fn a_send_before_the_msc_connects_reports_closed() {
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let message = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::PagingRequest,
            vec![],
        ));
        assert!(matches!(
            client.send_a1(message).await,
            Err(A1TransportError::Closed)
        ));
    }
}

#[cfg(test)]
mod event_tests {
    use std::time::Duration;

    use cdma_events::proto::{A1Direction, EventSource, ListenEventsRequest, network_event};
    use cdma_events::{
        EventBusConfig, EventBusServer, EventPublisher, EventPublisherConfig, EventServiceClient,
        EventServiceServer,
    };
    use cdma_ios::{
        ClearCompleteMessage, EncodedA1Message, Message, MessageType, MobileIdentity,
        PagingRequestMessage,
    };
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;

    use super::*;

    fn paging(call_id: u64) -> EncodedA1Message {
        let payload = PagingRequestMessage {
            mobile_identity_meid: None,
            mobile_identity_imsi: MobileIdentity::Imsi("310009135198800".into()),
            tag: None,
            cell_identifier_list: None,
            slot_cycle_index: None,
            service_option: None,
            is2000_mobile_capabilities: None,
        }
        .encode()
        .unwrap();
        EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::PagingRequest, payload),
            Some(call_id),
        )
    }

    fn clear_complete(call_id: u64) -> EncodedA1Message {
        let payload = ClearCompleteMessage {
            power_down_indicator: false,
        }
        .encode()
        .unwrap();
        EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::ClearComplete, payload),
            Some(call_id),
        )
    }

    #[tokio::test]
    async fn native_and_femto_a1_reach_the_same_bus_once_per_observation() {
        let bus_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", bus_listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            Server::builder()
                .add_service(EventServiceServer::new(EventBusServer::new(
                    EventBusConfig::default(),
                )))
                .serve_with_incoming(TcpListenerStream::new(bus_listener))
                .await
                .unwrap();
        });
        let mut subscriber = EventServiceClient::connect(endpoint.clone()).await.unwrap();
        let mut stream = subscriber
            .listen_events(ListenEventsRequest {
                source_filter: vec![EventSource::Bsc as i32, EventSource::Femto as i32],
            })
            .await
            .unwrap()
            .into_inner();
        let publisher =
            EventPublisher::spawn(EventPublisherConfig::new(endpoint, "test-bsc")).unwrap();
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap()
            .with_events(publisher.clone(), "bsc-1".into());

        assert!(client.send_a1(clear_complete(77)).await.is_err());
        let (msc, mut msc_rx) = cdma_ios::transport::connect(client.local_addr().unwrap())
            .await
            .unwrap();
        let inbound = paging(77);
        msc.send(&inbound).await.unwrap();
        assert_eq!(client.poll_a1().await.unwrap().unwrap(), inbound);
        let outbound = clear_complete(77);
        client.send_a1(outbound.clone()).await.unwrap();
        assert!(
            matches!(msc_rx.recv().await, Some(A1TransportEvent::Message(message)) if message == outbound)
        );
        client.send_a1(outbound.clone()).await.unwrap();
        assert!(
            matches!(msc_rx.recv().await, Some(A1TransportEvent::Message(message)) if message == outbound)
        );

        let femto = cdma_ios::femto_interface_msg::FemtoInterfaceMsg::from_paging_request(
            &PagingRequestMessage::decode(&inbound.decode().unwrap().payload).unwrap(),
        )
        .unwrap();
        publisher.publish(cdma_events::a1::a1_event(
            EventSource::Femto,
            "fap-1",
            &femto,
            &cdma_events::a1::A1Context {
                direction: A1Direction::Downlink,
                ..Default::default()
            },
        ));

        for (index, expected_direction) in [
            A1Direction::Downlink,
            A1Direction::Uplink,
            A1Direction::Uplink,
            A1Direction::Downlink,
        ]
        .into_iter()
        .enumerate()
        {
            let event = tokio::time::timeout(Duration::from_secs(5), stream.message())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                event.source,
                if index == 3 {
                    EventSource::Femto
                } else {
                    EventSource::Bsc
                } as i32
            );
            let Some(network_event::Body::A1(a1)) = event.body else {
                panic!("expected A1");
            };
            assert_eq!(a1.direction, expected_direction as i32);
            assert_eq!(a1.peer_id, if index == 3 { "fap-1" } else { "bsc-1" });
            if index == 2 {
                assert!(
                    event.identity.is_none(),
                    "cleared calls must not retain identity"
                );
            } else {
                assert_eq!(event.identity.unwrap().imsi, "310009135198800");
            }
            if index < 3 {
                assert_eq!(a1.a1_call_id, 77);
                let wire = if index == 0 { &inbound } else { &outbound };
                assert_eq!(
                    a1.sdu,
                    cdma_ios::split_a1_frame(&wire.decode().unwrap().payload)
                        .unwrap()
                        .sdu
                );
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(200), stream.message())
                .await
                .is_err(),
            "no duplicate or failed-send events"
        );
        server.abort();
    }
}
