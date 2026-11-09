//! A1p: BSAP over SUA over SCTP (A.S0012-D §3.2.2, RFC 3868 IPSP single
//! exchange). BSAP messages map onto SUA per A.S0012-D Table 3.2.2.6.4-2:
//!
//! - Complete Layer 3 Information (including a Paging Response) and Handoff
//!   Request open a connection with CORE.
//! - The first answer on a connection from its peer is a COAK, except a
//!   Location Updating Accept or Reject or a Handoff Failure, which refuse it
//!   with COREF.
//! - Every later message of the connection is a CODT. After a Clear Complete
//!   the MSC releases the connection with RELRE and the base station answers
//!   RELCO.
//! - Paging Request, BS Service Request/Response and the ADDS Page and
//!   Transfer messages are connectionless CLDTs.
//!
//! `call_id` never appears on the wire. A Paging Response takes the id of the
//! Paging Request whose Tag it echoes.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cdma_sctp::{
    CloseReason, Delivery, SctpAssociation, SctpConfig, SctpListener, SctpMessage, SendInfo,
};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::a1_message::{A1TransportError, EncodedA1Message};
use crate::sua::{self, SuaAddress, SuaMessage, SuaMessageKind, tag};
use crate::{MessageType, a1_frame_message_type};

pub const A1_SIGNALING_PORT: u16 = sua::A1P_SCTP_PORT;

const QUEUE_DEPTH: usize = 256;
const MANAGEMENT_STREAM: u16 = 0;
const FIRST_DATA_STREAM: u16 = 1;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// The stack keeps retransmitting an unanswered INIT for minutes, which would
/// stall a reconnect loop on a peer that has moved.
const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Inbound connections take ids from here so they never meet ids a base
/// station or the MSC allocates for its own calls.
const INBOUND_CALL_ID_BASE: u64 = 1 << 50;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A1Role {
    BaseStation,
    Msc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct A1pConfig {
    pub sctp: SctpConfig,
    pub local_point_code: u32,
    pub peer_point_code: u32,
    pub routing_context: u32,
}

/// A base station on the same host binds the RFC 6951 port.
pub const MSC_FALLBACK_SCTP_UDP_PORT: u16 = 9900;

pub const DEFAULT_BASE_STATION_POINT_CODE: u32 = 1;
pub const DEFAULT_MSC_POINT_CODE: u32 = 2;
const DEFAULT_ROUTING_CONTEXT: u32 = 1;

impl Default for A1pConfig {
    fn default() -> Self {
        Self::for_role(A1Role::BaseStation)
    }
}

impl A1pConfig {
    pub fn for_role(role: A1Role) -> Self {
        let (local_point_code, peer_point_code) = match role {
            A1Role::BaseStation => (DEFAULT_BASE_STATION_POINT_CODE, DEFAULT_MSC_POINT_CODE),
            A1Role::Msc => (DEFAULT_MSC_POINT_CODE, DEFAULT_BASE_STATION_POINT_CODE),
        };
        let mut sctp = SctpConfig::default();
        if role == A1Role::Msc {
            sctp.fallback_udp_encapsulation.local_port = MSC_FALLBACK_SCTP_UDP_PORT;
        }
        Self {
            sctp,
            local_point_code,
            peer_point_code,
            routing_context: DEFAULT_ROUTING_CONTEXT,
        }
    }

    fn local_address(&self) -> SuaAddress {
        SuaAddress {
            point_code: self.local_point_code,
            ssn: sua::BSAP_SSN,
        }
    }

    fn peer_address(&self) -> SuaAddress {
        SuaAddress {
            point_code: self.peer_point_code,
            ssn: sua::BSAP_SSN,
        }
    }
}

/// Events emitted by the transport to the consumer.
#[derive(Debug)]
pub enum A1TransportEvent {
    /// A decoded A1 message was received.
    Message(EncodedA1Message),
    Disconnected(io::Error),
}

/// Handle for sending A1 messages to a connected peer.
#[derive(Clone)]
pub struct A1TransportSender {
    tx: mpsc::Sender<EncodedA1Message>,
    peer: SocketAddr,
}

impl A1TransportSender {
    pub async fn send(&self, message: &EncodedA1Message) -> Result<(), A1TransportError> {
        info!(
            "A1: tx {:?} call_id={:?} ({} bytes) to {}",
            message.message_type(),
            message.call_id(),
            message.as_bytes().len(),
            self.peer,
        );
        self.tx
            .send(message.clone())
            .await
            .map_err(|_| A1TransportError::Closed)
    }
}

pub struct A1Listener {
    inner: SctpListener,
    config: A1pConfig,
    role: A1Role,
}

impl A1Listener {
    pub fn bind(addrs: &[SocketAddr], config: A1pConfig, role: A1Role) -> io::Result<Self> {
        let inner = SctpListener::bind(addrs, &config.sctp).map_err(sctp_io_error)?;
        Ok(Self {
            inner,
            config,
            role,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .local_addrs()
            .map_err(sctp_io_error)?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "A1p listener has no address"))
    }
}

pub async fn accept(
    listener: &A1Listener,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let (association, peer) = listener.inner.accept().await.map_err(sctp_io_error)?;
    info!("A1 signaling: accepted SCTP association from {peer}");
    let association = Arc::new(association);
    answer_handshake(&association, &listener.config).await?;
    info!("A1 signaling: SUA peer {peer} is ASP-ACTIVE");
    Ok(spawn_link(
        association,
        peer,
        listener.config.clone(),
        listener.role,
    ))
}

pub async fn accept_with_retry(
    listener: &A1Listener,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match accept(listener).await {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                warn!("A1 signaling: accept failed: {e}; retrying in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// An unbound SCTP socket lists every host address in INIT.
fn route_source(peer: SocketAddr) -> io::Result<SocketAddr> {
    let probe = std::net::UdpSocket::bind(if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    probe.connect(peer)?;
    Ok(SocketAddr::new(probe.local_addr()?.ip(), 0))
}

pub async fn connect(
    addr: SocketAddr,
    config: &A1pConfig,
    role: A1Role,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let local = route_source(addr)?;
    let association = tokio::time::timeout(
        ASSOCIATION_TIMEOUT,
        SctpAssociation::connect(&[addr], &[local], &config.sctp),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("no SCTP INIT ACK from {addr} within {ASSOCIATION_TIMEOUT:?}"),
        )
    })?
    .map_err(sctp_io_error)?;
    let association = Arc::new(association);
    initiate_handshake(&association, config).await?;
    info!("A1 signaling: SUA peer {addr} is ASP-ACTIVE");
    Ok(spawn_link(association, addr, config.clone(), role))
}

pub async fn connect_with_reconnect(
    addr: SocketAddr,
    config: &A1pConfig,
    role: A1Role,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let mut attempt: u32 = 0;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match connect(addr, config, role).await {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                attempt += 1;
                warn!(
                    "A1 signaling: connect to {addr} failed (attempt {attempt}): {e}; \
                     retrying in {backoff:?}"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

fn sctp_io_error(error: cdma_sctp::Error) -> io::Error {
    match error {
        cdma_sctp::Error::Io(error) => error,
        cdma_sctp::Error::Closed(reason) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!("SCTP association closed: {reason}"),
        ),
        other => io::Error::other(other.to_string()),
    }
}

async fn send_sua(
    association: &SctpAssociation,
    message: &SuaMessage,
    stream_id: u16,
) -> io::Result<()> {
    association
        .send(
            &message.encode(),
            SendInfo {
                stream_id,
                ppid: sua::SUA_PAYLOAD_PROTOCOL_ID,
                delivery: Delivery::Ordered,
            },
        )
        .await
        .map_err(sctp_io_error)
}

async fn recv_sua(association: &SctpAssociation) -> io::Result<SuaMessage> {
    loop {
        let message = association.recv().await.map_err(sctp_io_error)?;
        match SuaMessage::decode(&message.data) {
            Ok(sua) => return Ok(sua),
            Err(error) => warn!("A1 signaling: dropping undecodable SUA message: {error}"),
        }
    }
}

async fn expect_sua(
    association: &SctpAssociation,
    expected: SuaMessageKind,
) -> io::Result<SuaMessage> {
    let message = tokio::time::timeout(HANDSHAKE_TIMEOUT, recv_sua(association))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("no SUA {expected:?}")))??;
    if message.kind != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected SUA {expected:?}, got {:?}", message.kind),
        ));
    }
    Ok(message)
}

async fn initiate_handshake(association: &SctpAssociation, config: &A1pConfig) -> io::Result<()> {
    send_sua(
        association,
        &SuaMessage::new(SuaMessageKind::AspUp),
        MANAGEMENT_STREAM,
    )
    .await?;
    expect_sua(association, SuaMessageKind::AspUpAck).await?;
    send_sua(
        association,
        &SuaMessage::new(SuaMessageKind::AspActive)
            .with_u32(tag::TRAFFIC_MODE_TYPE, sua::TRAFFIC_MODE_OVERRIDE)
            .with_u32(tag::ROUTING_CONTEXT, config.routing_context),
        MANAGEMENT_STREAM,
    )
    .await?;
    expect_sua(association, SuaMessageKind::AspActiveAck).await?;
    Ok(())
}

async fn answer_handshake(association: &SctpAssociation, config: &A1pConfig) -> io::Result<()> {
    expect_sua(association, SuaMessageKind::AspUp).await?;
    send_sua(
        association,
        &SuaMessage::new(SuaMessageKind::AspUpAck),
        MANAGEMENT_STREAM,
    )
    .await?;
    let active = expect_sua(association, SuaMessageKind::AspActive).await?;
    let routing_context = active
        .u32_parameter(tag::ROUTING_CONTEXT)
        .unwrap_or(config.routing_context);
    if routing_context != config.routing_context {
        warn!(
            "A1 signaling: peer activated routing context {routing_context}, configured {}",
            config.routing_context
        );
    }
    send_sua(
        association,
        &SuaMessage::new(SuaMessageKind::AspActiveAck)
            .with_u32(tag::TRAFFIC_MODE_TYPE, sua::TRAFFIC_MODE_OVERRIDE)
            .with_u32(tag::ROUTING_CONTEXT, routing_context),
        MANAGEMENT_STREAM,
    )
    .await
}

fn spawn_link(
    association: Arc<SctpAssociation>,
    peer: SocketAddr,
    config: A1pConfig,
    role: A1Role,
) -> (A1TransportSender, mpsc::Receiver<A1TransportEvent>) {
    let (event_tx, event_rx) = mpsc::channel(QUEUE_DEPTH);
    let (write_tx, write_rx) = mpsc::channel(QUEUE_DEPTH);
    let (inbound_tx, inbound_rx) = mpsc::channel(QUEUE_DEPTH);
    let reader = Arc::clone(&association);
    tokio::spawn(async move {
        loop {
            let result = reader.recv().await;
            let closed = result.is_err();
            if inbound_tx.send(result).await.is_err() || closed {
                return;
            }
        }
    });
    let link = Link {
        association,
        peer,
        config,
        role,
        connections: Connections::default(),
        event_tx,
    };
    tokio::spawn(link.run(write_rx, inbound_rx));
    (A1TransportSender { tx: write_tx, peer }, event_rx)
}

#[derive(Debug)]
struct Connection {
    call_id: u64,
    local_ref: u32,
    remote_ref: Option<u32>,
    state: ConnectionState,
}

#[derive(Debug)]
enum ConnectionState {
    AwaitingAck(Vec<EncodedA1Message>),
    AwaitingAnswer,
    Established,
    Releasing,
}

#[derive(Default)]
struct Connections {
    by_call: HashMap<u64, Connection>,
    call_by_local_ref: HashMap<u32, u64>,
    paged_calls: HashMap<u32, u64>,
    next_local_ref: u32,
    next_inbound_call_id: u64,
}

impl Connections {
    fn allocate_local_ref(&mut self) -> u32 {
        loop {
            self.next_local_ref = self.next_local_ref.wrapping_add(1);
            let candidate = self.next_local_ref;
            if candidate != 0 && !self.call_by_local_ref.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn allocate_inbound_call_id(&mut self) -> u64 {
        loop {
            let candidate = INBOUND_CALL_ID_BASE + self.next_inbound_call_id;
            self.next_inbound_call_id = self.next_inbound_call_id.wrapping_add(1);
            if !self.by_call.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn insert(&mut self, connection: Connection) {
        self.call_by_local_ref
            .insert(connection.local_ref, connection.call_id);
        self.by_call.insert(connection.call_id, connection);
    }

    fn by_local_ref(&mut self, local_ref: u32) -> Option<&mut Connection> {
        let call_id = *self.call_by_local_ref.get(&local_ref)?;
        self.by_call.get_mut(&call_id)
    }

    fn remove_by_local_ref(&mut self, local_ref: u32) -> Option<Connection> {
        let call_id = self.call_by_local_ref.remove(&local_ref)?;
        self.by_call.remove(&call_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    Connectionless,
    OpensConnection,
    RefusesConnection,
    OnConnection,
}

fn carrier(message_type: MessageType) -> Carrier {
    use MessageType as M;
    match message_type {
        M::PagingRequest
        | M::BsServiceRequest
        | M::BsServiceResponse
        | M::AddsPage
        | M::AddsPageAck
        | M::AddsTransfer
        | M::AddsTransferAck => Carrier::Connectionless,
        M::CompleteLayer3Information
        | M::PagingResponse
        | M::CmServiceRequest
        | M::HandoffRequest => Carrier::OpensConnection,
        M::LocationUpdatingAccept | M::LocationUpdatingReject | M::HandoffFailure => {
            Carrier::RefusesConnection
        }
        _ => Carrier::OnConnection,
    }
}

struct Link {
    association: Arc<SctpAssociation>,
    peer: SocketAddr,
    config: A1pConfig,
    role: A1Role,
    connections: Connections,
    event_tx: mpsc::Sender<A1TransportEvent>,
}

impl Link {
    async fn run(
        mut self,
        mut write_rx: mpsc::Receiver<EncodedA1Message>,
        mut inbound_rx: mpsc::Receiver<cdma_sctp::Result<SctpMessage>>,
    ) {
        loop {
            tokio::select! {
                outbound = write_rx.recv() => {
                    let Some(message) = outbound else {
                        let _ = self.association.shutdown();
                        return;
                    };
                    if let Err(error) = self.send_a1(message).await {
                        self.disconnect(error).await;
                        return;
                    }
                }
                inbound = inbound_rx.recv() => {
                    let message = match inbound {
                        Some(Ok(message)) => message,
                        Some(Err(cdma_sctp::Error::Closed(reason))) => {
                            let kind = if reason == CloseReason::Shutdown {
                                io::ErrorKind::ConnectionAborted
                            } else {
                                io::ErrorKind::ConnectionReset
                            };
                            self.disconnect(io::Error::new(kind, format!("A1p association {reason}"))).await;
                            return;
                        }
                        Some(Err(error)) => {
                            self.disconnect(sctp_io_error(error)).await;
                            return;
                        }
                        None => return,
                    };
                    if let Err(error) = self.handle_inbound(message).await {
                        self.disconnect(error).await;
                        return;
                    }
                }
            }
        }
    }

    async fn disconnect(&self, error: io::Error) {
        info!("A1 signaling: link to {} down: {error}", self.peer);
        let _ = self
            .event_tx
            .send(A1TransportEvent::Disconnected(error))
            .await;
    }

    fn data_stream(&self, local_ref: u32) -> u16 {
        let data_streams = self.association.outbound_streams().saturating_sub(1).max(1);
        FIRST_DATA_STREAM + (local_ref % u32::from(data_streams)) as u16
    }

    async fn send_data(&self, message: SuaMessage, local_ref: u32) -> io::Result<()> {
        send_sua(&self.association, &message, self.data_stream(local_ref)).await
    }

    fn connectionless(&self, data: &[u8]) -> SuaMessage {
        SuaMessage::new(SuaMessageKind::ConnectionlessData)
            .with_u32(tag::ROUTING_CONTEXT, self.config.routing_context)
            .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_0)
            .with_address(tag::SOURCE_ADDRESS, self.config.local_address())
            .with_address(tag::DESTINATION_ADDRESS, self.config.peer_address())
            .with_u32(tag::SEQUENCE_CONTROL, 0)
            .with(tag::DATA, data.to_vec())
    }

    fn connection_request(&self, local_ref: u32, data: &[u8]) -> SuaMessage {
        SuaMessage::new(SuaMessageKind::ConnectionRequest)
            .with_u32(tag::ROUTING_CONTEXT, self.config.routing_context)
            .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_2)
            .with_u32(tag::SOURCE_REFERENCE_NUMBER, local_ref)
            .with_address(tag::DESTINATION_ADDRESS, self.config.peer_address())
            .with_u32(tag::SEQUENCE_CONTROL, 0)
            .with_address(tag::SOURCE_ADDRESS, self.config.local_address())
            .with(tag::DATA, data.to_vec())
    }

    fn connection_ack(&self, local_ref: u32, remote_ref: u32, data: &[u8]) -> SuaMessage {
        SuaMessage::new(SuaMessageKind::ConnectionAck)
            .with_u32(tag::ROUTING_CONTEXT, self.config.routing_context)
            .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_2)
            .with_u32(tag::DESTINATION_REFERENCE_NUMBER, remote_ref)
            .with_u32(tag::SOURCE_REFERENCE_NUMBER, local_ref)
            .with_u32(tag::SEQUENCE_CONTROL, 0)
            .with(tag::DATA, data.to_vec())
    }

    fn connection_refused(&self, remote_ref: u32, data: &[u8]) -> SuaMessage {
        SuaMessage::new(SuaMessageKind::ConnectionRefused)
            .with_u32(tag::ROUTING_CONTEXT, self.config.routing_context)
            .with_u32(tag::DESTINATION_REFERENCE_NUMBER, remote_ref)
            .with_cause(
                sua::CAUSE_TYPE_REFUSAL,
                sua::REFUSAL_CAUSE_END_USER_ORIGINATED,
            )
            .with(tag::DATA, data.to_vec())
    }

    fn connection_data(&self, remote_ref: u32, data: &[u8]) -> SuaMessage {
        SuaMessage::new(SuaMessageKind::ConnectionData)
            .with_u32(tag::ROUTING_CONTEXT, self.config.routing_context)
            .with_u32(tag::DESTINATION_REFERENCE_NUMBER, remote_ref)
            .with(tag::DATA, data.to_vec())
    }

    fn release(&self, kind: SuaMessageKind, local_ref: u32, remote_ref: u32) -> SuaMessage {
        let message = SuaMessage::new(kind)
            .with_u32(tag::ROUTING_CONTEXT, self.config.routing_context)
            .with_u32(tag::DESTINATION_REFERENCE_NUMBER, remote_ref)
            .with_u32(tag::SOURCE_REFERENCE_NUMBER, local_ref);
        if kind == SuaMessageKind::ReleaseRequest {
            message.with_cause(
                sua::CAUSE_TYPE_RELEASE,
                sua::RELEASE_CAUSE_END_USER_ORIGINATED,
            )
        } else {
            message
        }
    }

    async fn send_a1(&mut self, message: EncodedA1Message) -> io::Result<()> {
        let data = message.as_bytes();
        let call_id = message.call_id();
        let route = match (carrier(message.message_type()), call_id) {
            (Carrier::Connectionless, _) | (_, None) => Carrier::Connectionless,
            (route, Some(_)) => route,
        };
        if route == Carrier::Connectionless {
            if self.role == A1Role::Msc
                && message.message_type() == MessageType::PagingRequest
                && let (Some(call_id), Some(tag)) = (call_id, paging_request_tag(data))
            {
                self.connections.paged_calls.insert(tag, call_id);
            }
            return send_sua(
                &self.association,
                &self.connectionless(data),
                FIRST_DATA_STREAM,
            )
            .await;
        }
        let call_id = call_id.expect("connectionless messages returned above");

        let Some(connection) = self.connections.by_call.get_mut(&call_id) else {
            if route != Carrier::OpensConnection {
                warn!(
                    "A1 signaling: {:?} call_id={call_id} names no SUA connection, sending it connectionless",
                    message.message_type()
                );
                return send_sua(
                    &self.association,
                    &self.connectionless(data),
                    FIRST_DATA_STREAM,
                )
                .await;
            }
            let local_ref = self.connections.allocate_local_ref();
            self.connections.insert(Connection {
                call_id,
                local_ref,
                remote_ref: None,
                state: ConnectionState::AwaitingAck(Vec::new()),
            });
            return self
                .send_data(self.connection_request(local_ref, data), local_ref)
                .await;
        };

        let local_ref = connection.local_ref;
        match (&mut connection.state, connection.remote_ref) {
            (ConnectionState::AwaitingAck(queue), _) => {
                queue.push(message);
                Ok(())
            }
            (ConnectionState::AwaitingAnswer, Some(remote_ref))
                if route == Carrier::RefusesConnection =>
            {
                self.connections.remove_by_local_ref(local_ref);
                self.send_data(self.connection_refused(remote_ref, data), local_ref)
                    .await
            }
            (ConnectionState::AwaitingAnswer, Some(remote_ref)) => {
                connection.state = ConnectionState::Established;
                self.send_data(self.connection_ack(local_ref, remote_ref, data), local_ref)
                    .await
            }
            (_, Some(remote_ref)) => {
                self.send_data(self.connection_data(remote_ref, data), local_ref)
                    .await
            }
            (_, None) => {
                warn!("A1 signaling: call_id={call_id} has no peer reference, dropping");
                Ok(())
            }
        }
    }

    async fn deliver(&self, data: &[u8], call_id: Option<u64>) {
        match EncodedA1Message::from_bytes_with_call_id(data.to_vec(), call_id) {
            Ok(message) => {
                info!(
                    "A1: rx {:?} call_id={:?} ({} bytes) from {}",
                    message.message_type(),
                    message.call_id(),
                    data.len(),
                    self.peer,
                );
                let _ = self.event_tx.send(A1TransportEvent::Message(message)).await;
            }
            Err(error) => warn!("A1 signaling: undecodable BSAP from {}: {error}", self.peer),
        }
    }

    async fn handle_inbound(&mut self, message: SctpMessage) -> io::Result<()> {
        if message.ppid != sua::SUA_PAYLOAD_PROTOCOL_ID {
            debug!(
                "A1 signaling: message with payload protocol {} from {}",
                message.ppid, self.peer
            );
        }
        let sua = match SuaMessage::decode(&message.data) {
            Ok(sua) => sua,
            Err(error) => {
                warn!("A1 signaling: undecodable SUA from {}: {error}", self.peer);
                return Ok(());
            }
        };
        let destination_ref = sua.u32_parameter(tag::DESTINATION_REFERENCE_NUMBER);
        let source_ref = sua.u32_parameter(tag::SOURCE_REFERENCE_NUMBER);
        match sua.kind {
            SuaMessageKind::ConnectionlessData => {
                if let Some(data) = sua.data() {
                    self.deliver(data, None).await;
                }
            }
            SuaMessageKind::ConnectionRequest => {
                let (Some(remote_ref), Some(data)) = (source_ref, sua.data()) else {
                    warn!(
                        "A1 signaling: CORE without reference or data from {}",
                        self.peer
                    );
                    return Ok(());
                };
                let call_id = self.inbound_call_id(data);
                let local_ref = self.connections.allocate_local_ref();
                self.connections.insert(Connection {
                    call_id,
                    local_ref,
                    remote_ref: Some(remote_ref),
                    state: ConnectionState::AwaitingAnswer,
                });
                self.deliver(data, Some(call_id)).await;
            }
            SuaMessageKind::ConnectionAck => {
                let Some(local_ref) = destination_ref else {
                    return Ok(());
                };
                let Some(connection) = self.connections.by_local_ref(local_ref) else {
                    warn!("A1 signaling: COAK for unknown reference {local_ref}");
                    return Ok(());
                };
                connection.remote_ref = source_ref;
                let call_id = connection.call_id;
                let queued =
                    match std::mem::replace(&mut connection.state, ConnectionState::Established) {
                        ConnectionState::AwaitingAck(queued) => queued,
                        _ => Vec::new(),
                    };
                if let Some(data) = sua.data() {
                    self.deliver(data, Some(call_id)).await;
                }
                for message in queued {
                    self.send_a1(message).await?;
                }
            }
            SuaMessageKind::ConnectionRefused => {
                let Some(local_ref) = destination_ref else {
                    return Ok(());
                };
                let Some(connection) = self.connections.remove_by_local_ref(local_ref) else {
                    return Ok(());
                };
                if let Some(data) = sua.data() {
                    self.deliver(data, Some(connection.call_id)).await;
                }
            }
            SuaMessageKind::ConnectionData => {
                let Some(local_ref) = destination_ref else {
                    return Ok(());
                };
                let Some(call_id) = self.connections.call_by_local_ref.get(&local_ref).copied()
                else {
                    warn!("A1 signaling: CODT for unknown reference {local_ref}");
                    return Ok(());
                };
                let Some(data) = sua.data() else {
                    return Ok(());
                };
                self.deliver(data, Some(call_id)).await;
                if self.role == A1Role::Msc
                    && a1_frame_message_type(data).ok() == Some(MessageType::ClearComplete)
                {
                    self.release_connection(local_ref).await?;
                }
            }
            SuaMessageKind::ReleaseRequest => {
                let Some(local_ref) = destination_ref else {
                    return Ok(());
                };
                let remote_ref = source_ref.unwrap_or_default();
                self.connections.remove_by_local_ref(local_ref);
                self.send_data(
                    self.release(SuaMessageKind::ReleaseComplete, local_ref, remote_ref),
                    local_ref,
                )
                .await?;
            }
            SuaMessageKind::ReleaseComplete => {
                if let Some(local_ref) = destination_ref {
                    self.connections.remove_by_local_ref(local_ref);
                }
            }
            SuaMessageKind::Heartbeat => {
                let mut ack = SuaMessage::new(SuaMessageKind::HeartbeatAck);
                if let Some(data) = sua.parameter(tag::HEARTBEAT_DATA) {
                    ack = ack.with(tag::HEARTBEAT_DATA, data.to_vec());
                }
                send_sua(&self.association, &ack, MANAGEMENT_STREAM).await?;
            }
            SuaMessageKind::AspDown | SuaMessageKind::AspInactive => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    format!("SUA peer sent {:?}", sua.kind),
                ));
            }
            other => debug!("A1 signaling: ignoring SUA {other:?} from {}", self.peer),
        }
        Ok(())
    }

    async fn release_connection(&mut self, local_ref: u32) -> io::Result<()> {
        let Some(connection) = self.connections.by_local_ref(local_ref) else {
            return Ok(());
        };
        let Some(remote_ref) = connection.remote_ref else {
            return Ok(());
        };
        connection.state = ConnectionState::Releasing;
        self.send_data(
            self.release(SuaMessageKind::ReleaseRequest, local_ref, remote_ref),
            local_ref,
        )
        .await
    }

    fn inbound_call_id(&mut self, data: &[u8]) -> u64 {
        if self.role == A1Role::Msc
            && let Some(tag) = paging_response_tag(data)
            && let Some(call_id) = self.connections.paged_calls.remove(&tag)
            && !self.connections.by_call.contains_key(&call_id)
        {
            return call_id;
        }
        self.connections.allocate_inbound_call_id()
    }
}

fn paging_request_tag(frame: &[u8]) -> Option<u32> {
    crate::PagingRequestMessage::decode(frame)
        .ok()
        .and_then(|request| request.tag)
        .map(|tag| tag.0)
}

fn paging_response_tag(frame: &[u8]) -> Option<u32> {
    if a1_frame_message_type(frame).ok()? != MessageType::PagingResponse {
        return None;
    }
    crate::CompleteLayer3InformationMessage::decode(frame)
        .ok()?
        .layer3_information
        .decode_paging_response()
        .ok()?
        .tag
        .map(|tag| tag.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1p_connects_from_the_one_address_routed_to_the_peer() {
        let peer: SocketAddr = "127.0.0.2:14001".parse().unwrap();

        let local = route_source(peer).unwrap();

        assert!(local.ip().is_loopback(), "{local}");
        assert_eq!(local.port(), 0);
    }
}
