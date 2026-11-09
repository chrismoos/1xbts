//! The MSC dials A1p, so the BSC listens and re-accepts after an MSC restart.

use std::net::SocketAddr;
use std::time::Duration;

use log::{info, warn};
use tokio::sync::{Mutex, mpsc};

use cdma_ios::transport::{A1Listener, A1Role, A1TransportEvent, A1TransportSender, A1pConfig};
use cdma_ios::{A1TransportError, EncodedA1Message};

use super::{MscClient, events::A1EventSink};

type A1Link = (A1TransportSender, mpsc::Receiver<A1TransportEvent>);

const ACCEPT_ERROR_DELAY: Duration = Duration::from_secs(1);

pub struct NetworkMscClient {
    local_addr: SocketAddr,
    event_sink: Option<A1EventSink>,
    sender: std::sync::Mutex<Option<A1TransportSender>>,
    links: Mutex<mpsc::Receiver<A1Link>>,
    events_rx: Mutex<Option<mpsc::Receiver<A1TransportEvent>>>,
    acceptor: tokio::task::JoinHandle<()>,
}

impl NetworkMscClient {
    pub async fn bind(addr: SocketAddr, config: A1pConfig) -> Result<Self, std::io::Error> {
        let listener = A1Listener::bind(&[addr], config, A1Role::BaseStation)?;
        let local_addr = listener.local_addr()?;
        let (link_tx, links) = mpsc::channel(1);
        // poll_a1 must stay cancel-safe, so accepting runs in its own task.
        let acceptor = tokio::spawn(async move {
            loop {
                match cdma_ios::transport::accept_with_retry(&listener).await {
                    Ok(link) => {
                        if link_tx.send(link).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        warn!("BSC A1 endpoint: accept failed: {e}");
                        tokio::time::sleep(ACCEPT_ERROR_DELAY).await;
                    }
                }
            }
        });
        Ok(Self {
            local_addr,
            event_sink: None,
            sender: std::sync::Mutex::new(None),
            links: Mutex::new(links),
            events_rx: Mutex::new(None),
            acceptor,
        })
    }

    pub fn with_events(mut self, publisher: cdma_events::EventPublisher, node_id: String) -> Self {
        self.event_sink = Some(A1EventSink::new(publisher, node_id));
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        Ok(self.local_addr)
    }

    fn current_sender(&self) -> Option<A1TransportSender> {
        self.sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_sender(&self, sender: Option<A1TransportSender>) {
        *self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = sender;
    }

    fn drop_link(&self) {
        self.set_sender(None);
        if let Some(sink) = &self.event_sink {
            sink.clear();
        }
    }
}

impl Drop for NetworkMscClient {
    fn drop(&mut self) {
        self.acceptor.abort();
    }
}

#[tonic::async_trait]
impl MscClient for NetworkMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        match self.current_sender() {
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
                let Some((sender, rx)) = self.links.lock().await.recv().await else {
                    return Err(A1TransportError::Closed);
                };
                info!("BSC accepted A1 signaling connection from MSC");
                self.set_sender(Some(sender));
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
                    self.drop_link();
                }
                None => {
                    warn!("BSC A1 endpoint: transport closed, awaiting reconnect");
                    *events_rx = None;
                    self.drop_link();
                }
            }
        }
    }
}

#[cfg(test)]
async fn test_msc(
    addr: SocketAddr,
) -> (
    A1TransportSender,
    tokio::sync::mpsc::Receiver<A1TransportEvent>,
) {
    cdma_ios::transport::connect(addr, &in_process_msc_config(), A1Role::Msc)
        .await
        .unwrap()
}

#[cfg(test)]
fn in_process_msc_config() -> A1pConfig {
    in_process(A1pConfig::for_role(A1Role::Msc))
}

#[cfg(test)]
fn in_process_bsc_config() -> A1pConfig {
    in_process(A1pConfig::default())
}

#[cfg(test)]
fn in_process(mut config: A1pConfig) -> A1pConfig {
    config.sctp.fallback_udp_encapsulation.local_port = in_process_udp_port();
    config.sctp.fallback_udp_encapsulation.remote_port = in_process_udp_port();
    config.sctp.rto_initial_ms = Some(TEST_RTO_INITIAL_MS);
    config.sctp.rto_min_ms = Some(TEST_RTO_MIN_MS);
    config.sctp.rto_max_ms = Some(TEST_RTO_MAX_MS);
    config
}

#[cfg(test)]
fn in_process_udp_port() -> u16 {
    static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *PORT.get_or_init(|| {
        std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free UDP port")
            .port()
    })
}

#[cfg(test)]
const TEST_RTO_INITIAL_MS: u32 = 200;
#[cfg(test)]
const TEST_RTO_MIN_MS: u32 = 200;
#[cfg(test)]
const TEST_RTO_MAX_MS: u32 = 1_000;

#[cfg(test)]
fn msc_sending(
    addr: SocketAddr,
    message: EncodedA1Message,
) -> tokio::task::JoinHandle<(
    A1TransportSender,
    tokio::sync::mpsc::Receiver<A1TransportEvent>,
)> {
    tokio::spawn(async move {
        let (sender, rx) = test_msc(addr).await;
        sender.send(&message).await.unwrap();
        (sender, rx)
    })
}

#[cfg(test)]
fn test_frame(message_type: cdma_ios::MessageType) -> EncodedA1Message {
    let payload = match message_type {
        cdma_ios::MessageType::PagingRequest => cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi("310009135198800".into()),
            mobile_identity_meid: None,
            tag: Some(cdma_ios::Tag(1)),
            cell_identifier_list: None,
            slot_cycle_index: None,
            service_option: None,
            is2000_mobile_capabilities: None,
        }
        .encode()
        .unwrap(),
        _ => cdma_ios::CompleteLayer3InformationMessage {
            cell_identifier: cdma_ios::CellId { cell: 1, sector: 0 },
            layer3_information: cdma_ios::Layer3Information(vec![0x05, 0x00, 0x08]),
        }
        .encode()
        .unwrap(),
    };
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(message_type, payload),
        (message_type != cdma_ios::MessageType::PagingRequest).then_some(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_cancelled_poll_does_not_abort_an_msc_mid_handshake() {
        use cdma_ios::sua::{self, SuaMessage, SuaMessageKind};

        let config = in_process_bsc_config();
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap(), config.clone())
            .await
            .unwrap();
        let msc =
            cdma_sctp::SctpAssociation::connect(&[client.local_addr().unwrap()], &[], &config.sctp)
                .await
                .unwrap();
        for _ in 0..20 {
            let _ =
                tokio::time::timeout(std::time::Duration::from_millis(5), client.poll_a1()).await;
        }

        msc.send(
            &SuaMessage::new(SuaMessageKind::AspUp).encode(),
            cdma_sctp::SendInfo {
                stream_id: 0,
                ppid: sua::SUA_PAYLOAD_PROTOCOL_ID,
                ..cdma_sctp::SendInfo::default()
            },
        )
        .await
        .unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(2), msc.recv())
            .await
            .expect("an answer to ASP Up")
            .expect("the association is still up");

        assert_eq!(
            SuaMessage::decode(&reply.data).unwrap().kind,
            SuaMessageKind::AspUpAck
        );
    }

    #[tokio::test]
    async fn accepts_the_msc_and_relays_both_ways() {
        let client =
            NetworkMscClient::bind("127.0.0.1:0".parse().unwrap(), in_process_bsc_config())
                .await
                .unwrap();
        let addr = client.local_addr().unwrap();
        let msc = msc_sending(addr, test_frame(cdma_ios::MessageType::PagingRequest));
        let received = client.poll_a1().await.unwrap().expect("message");
        let (_msc_sender, mut msc_rx) = msc.await.unwrap();
        assert_eq!(
            received.message_type(),
            cdma_ios::MessageType::PagingRequest
        );

        let outbound = test_frame(cdma_ios::MessageType::CompleteLayer3Information);
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
        let client =
            NetworkMscClient::bind("127.0.0.1:0".parse().unwrap(), in_process_bsc_config())
                .await
                .unwrap();
        let addr = client.local_addr().unwrap();

        let first = msc_sending(addr, test_frame(cdma_ios::MessageType::PagingRequest));
        assert_eq!(
            client
                .poll_a1()
                .await
                .unwrap()
                .expect("first message")
                .message_type(),
            cdma_ios::MessageType::PagingRequest
        );
        drop(first.await.unwrap());

        let _second = msc_sending(addr, test_frame(cdma_ios::MessageType::PagingRequest));
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), client.poll_a1())
            .await
            .expect("the BSC re-accepts once the first MSC goes away")
            .unwrap()
            .expect("second message");
        assert_eq!(
            received.message_type(),
            cdma_ios::MessageType::PagingRequest
        );
    }

    #[tokio::test]
    async fn a_send_before_the_msc_connects_reports_closed() {
        let client =
            NetworkMscClient::bind("127.0.0.1:0".parse().unwrap(), in_process_bsc_config())
                .await
                .unwrap();
        let message = test_frame(cdma_ios::MessageType::PagingRequest);
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
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;

    use super::*;

    fn paging(call_id: u64) -> EncodedA1Message {
        let payload = PagingRequestMessage {
            mobile_identity_meid: None,
            mobile_identity_imsi: MobileIdentity::Imsi("310009135198800".into()),
            tag: Some(cdma_ios::Tag(call_id as u32)),
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
        let client =
            NetworkMscClient::bind("127.0.0.1:0".parse().unwrap(), in_process_bsc_config())
                .await
                .unwrap()
                .with_events(publisher.clone(), "bsc-1".into());

        assert!(client.send_a1(clear_complete(77)).await.is_err());
        let inbound = paging(77);
        let msc = msc_sending(client.local_addr().unwrap(), inbound.clone());
        assert_eq!(
            client.poll_a1().await.unwrap().unwrap().as_bytes(),
            inbound.as_bytes()
        );
        let (_msc, mut msc_rx) = msc.await.unwrap();
        let outbound = clear_complete(77);
        client.send_a1(outbound.clone()).await.unwrap();
        assert!(
            matches!(msc_rx.recv().await, Some(A1TransportEvent::Message(message)) if message.as_bytes() == outbound.as_bytes())
        );
        client.send_a1(outbound.clone()).await.unwrap();
        assert!(
            matches!(msc_rx.recv().await, Some(A1TransportEvent::Message(message)) if message.as_bytes() == outbound.as_bytes())
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
                assert_eq!(a1.a1_call_id, if index == 0 { 0 } else { 77 });
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
