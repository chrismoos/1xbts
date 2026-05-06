//! Async TCP transport for Abis signaling on port 5604.
//!
//! One TCP connection per (BTS, BSC) peer pair. Uses `SignalingFrame`
//! (0xF634 flag + 16-bit length) for framing per A.S0003-A §4.5.6.4.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use log::{debug, info, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::control::{AbisMessage, decode, encode};
use crate::signaling_framing::{SignalingFrame, SignalingFrameStreamDecoder};

/// Default Abis signaling port per A.S0003-A §4.5.6.4.
pub const ABIS_SIGNALING_PORT: u16 = 5604;

/// Default BSC-side bearer UDP port (receives reverse frames from BTS).
///
/// Distinct from A1_SIGNALING_PORT (17013/TCP in cdma-ios) — different protocol
/// so the OS allows both, but dedicated ports avoid confusion in split deployments.
pub const ABIS_BSC_BEARER_PORT: u16 = 17022;

/// Default BTS-side bearer UDP port (receives forward frames from BSC).
pub const ABIS_BTS_BEARER_PORT: u16 = 17014;

/// Events emitted by the transport to the consumer.
#[derive(Debug)]
pub enum TransportEvent {
    /// A decoded Abis control message was received.
    Message(AbisMessage),
    /// The peer disconnected or the connection failed.
    Disconnected(io::Error),
}

/// Handle for sending messages to a connected peer.
#[derive(Clone)]
pub struct TransportSender {
    tx: mpsc::Sender<Vec<u8>>,
}

impl TransportSender {
    /// Encodes and queues an Abis message for transmission.
    pub async fn send(&self, message: &AbisMessage) -> Result<(), TransportSendError> {
        let payload = encode(message).map_err(TransportSendError::Codec)?;
        let frame = SignalingFrame::new(payload);
        let bytes = frame.encode().map_err(TransportSendError::Codec)?;
        self.tx
            .send(bytes)
            .await
            .map_err(|_| TransportSendError::Closed)
    }

    /// Encodes and queues an Abis message for transmission (non-blocking).
    pub fn try_send(&self, message: &AbisMessage) -> Result<(), TransportSendError> {
        let payload = encode(message).map_err(TransportSendError::Codec)?;
        let frame = SignalingFrame::new(payload);
        let bytes = frame.encode().map_err(TransportSendError::Codec)?;
        self.tx
            .try_send(bytes)
            .map_err(|_| TransportSendError::Closed)
    }

    /// Queues raw pre-encoded bytes for transmission (for bearer passthrough).
    pub async fn send_raw(&self, bytes: Vec<u8>) -> Result<(), TransportSendError> {
        self.tx
            .send(bytes)
            .await
            .map_err(|_| TransportSendError::Closed)
    }
}

/// Errors from sending on the transport.
#[derive(Debug)]
pub enum TransportSendError {
    Codec(crate::Error),
    Closed,
}

impl std::fmt::Display for TransportSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportSendError::Codec(e) => write!(f, "Abis codec error: {e}"),
            TransportSendError::Closed => f.write_str("Abis transport closed"),
        }
    }
}

impl std::error::Error for TransportSendError {}

/// Accepts a single Abis signaling connection on the given listener.
///
/// Returns a sender for outbound messages and a receiver for inbound events.
/// The transport runs two background tasks (read/write) that terminate when
/// the connection drops or the sender is dropped.
pub async fn accept(
    listener: &TcpListener,
) -> io::Result<(TransportSender, mpsc::Receiver<TransportEvent>)> {
    let (stream, peer) = listener.accept().await?;
    info!("Abis signaling: accepted connection from {peer}");
    Ok(spawn_transport(stream, peer))
}

/// Connects to a remote Abis signaling peer.
pub async fn connect(
    addr: SocketAddr,
) -> io::Result<(TransportSender, mpsc::Receiver<TransportEvent>)> {
    let stream = TcpStream::connect(addr).await?;
    info!("Abis signaling: connected to {addr}");
    Ok(spawn_transport(stream, addr))
}

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Runs `op` in a loop with exponential backoff until it succeeds.
async fn with_backoff<T, F, Fut>(label: &str, mut op: F) -> io::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = io::Result<T>>,
{
    let mut attempt: u32 = 0;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match op().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                attempt += 1;
                warn!(
                    "Abis signaling: {label} failed (attempt {attempt}): {e}; \
                     retrying in {backoff:?}"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// Connects to a remote Abis signaling peer with exponential backoff on failure.
///
/// Retries with delays of 1s, 2s, 4s, 8s, 16s, capped at 30s until a connection
/// succeeds. Returns the sender/receiver pair from the first successful connection.
pub async fn connect_with_reconnect(
    addr: SocketAddr,
) -> io::Result<(TransportSender, mpsc::Receiver<TransportEvent>)> {
    with_backoff(&format!("connect to {addr}"), || connect(addr)).await
}

/// Accepts an Abis signaling connection, retrying on transient accept errors
/// with exponential backoff (1s, 2s, 4s, … capped at 30s).
///
/// Returns the sender/receiver pair from the first successful accept.
pub async fn accept_with_retry(
    listener: &TcpListener,
) -> io::Result<(TransportSender, mpsc::Receiver<TransportEvent>)> {
    with_backoff("accept", || accept(listener)).await
}

fn spawn_transport(
    stream: TcpStream,
    peer: SocketAddr,
) -> (TransportSender, mpsc::Receiver<TransportEvent>) {
    let (read_half, write_half) = stream.into_split();
    let (event_tx, event_rx) = mpsc::channel(256);
    let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>(256);

    tokio::spawn(read_loop(read_half, event_tx, peer));
    tokio::spawn(write_loop(write_half, write_rx, peer));

    (TransportSender { tx: write_tx }, event_rx)
}

async fn read_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    event_tx: mpsc::Sender<TransportEvent>,
    peer: SocketAddr,
) {
    let mut decoder = SignalingFrameStreamDecoder::new();
    let mut buf = [0u8; 4096];

    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                info!("Abis signaling: peer {peer} disconnected");
                let _ = event_tx
                    .send(TransportEvent::Disconnected(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "peer disconnected",
                    )))
                    .await;
                return;
            }
            Ok(n) => {
                decoder.push_bytes(&buf[..n]);
                loop {
                    match decoder.next_frame() {
                        Ok(Some(frame)) => match decode(&frame.payload) {
                            Ok(message) => {
                                info!(
                                    "Abis: rx {:?} ({} bytes) from {peer}",
                                    message.message_type,
                                    frame.payload.len(),
                                );
                                if event_tx
                                    .send(TransportEvent::Message(message))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Abis signaling: decode error from {peer}: {e} ({} payload bytes)",
                                    frame.payload.len()
                                );
                            }
                        },
                        Ok(None) => break,
                        Err(e) => {
                            warn!("Abis signaling: framing error from {peer}: {e}");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Abis signaling: read error from {peer}: {e}");
                let _ = event_tx.send(TransportEvent::Disconnected(e)).await;
                return;
            }
        }
    }
}

async fn write_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut write_rx: mpsc::Receiver<Vec<u8>>,
    peer: SocketAddr,
) {
    while let Some(bytes) = write_rx.recv().await {
        if let Err(e) = writer.write_all(&bytes).await {
            warn!("Abis signaling: write error to {peer}: {e}");
            return;
        }
    }
    debug!("Abis signaling: write loop closed for {peer}");
}

/// Creates an in-memory channel-backed transport pair.
///
/// Returns `(client_sender, client_events, server_sender, server_events)`.
/// Messages sent by the client appear as `TransportEvent::Message` on the
/// server event receiver, and vice versa. No TCP, no framing — messages are
/// passed directly as decoded `AbisMessage` values.
///
/// Both sides share the same `TransportSender` / `Receiver<TransportEvent>`
/// API as the TCP transport, so `NetworkBtsControlClient::from_transport()`
/// works unchanged.
pub fn spawn_channel_transport() -> (
    TransportSender,
    mpsc::Receiver<TransportEvent>,
    TransportSender,
    mpsc::Receiver<TransportEvent>,
) {
    let (client_write_tx, mut client_write_rx) = mpsc::channel::<Vec<u8>>(256);
    let (server_write_tx, mut server_write_rx) = mpsc::channel::<Vec<u8>>(256);
    let (client_event_tx, client_event_rx) = mpsc::channel(256);
    let (server_event_tx, server_event_rx) = mpsc::channel(256);

    // client → server bridge
    tokio::spawn(async move {
        while let Some(bytes) = client_write_rx.recv().await {
            let frame = match SignalingFrame::decode(&bytes) {
                Ok(f) => f,
                Err(_) => continue,
            };
            match decode(&frame.payload) {
                Ok(msg) => {
                    if server_event_tx
                        .send(TransportEvent::Message(msg))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    warn!("channel transport: client→server decode error: {e}");
                }
            }
        }
    });

    // server → client bridge
    tokio::spawn(async move {
        while let Some(bytes) = server_write_rx.recv().await {
            let frame = match SignalingFrame::decode(&bytes) {
                Ok(f) => f,
                Err(_) => continue,
            };
            match decode(&frame.payload) {
                Ok(msg) => {
                    if client_event_tx
                        .send(TransportEvent::Message(msg))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    warn!("channel transport: server→client decode error: {e}");
                }
            }
        }
    });

    (
        TransportSender {
            tx: client_write_tx,
        },
        client_event_rx,
        TransportSender {
            tx: server_write_tx,
        },
        server_event_rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::MessageType;
    use crate::control::typed::*;

    #[tokio::test]
    async fn transport_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle = tokio::spawn(async move { accept(&listener).await.unwrap() });

        let (client_tx, mut client_rx) = connect(addr).await.unwrap();
        let (server_tx, mut server_rx) = accept_handle.await.unwrap();

        let msg = BtsReleaseMessage {
            call_connection_reference: CallConnectionReference {
                market_id: 100,
                generating_entity_id: 200,
                call_connection_reference: 300,
            },
            cell_identifier_list: None,
            correlation_id: None,
        };
        let wire = msg.encode().unwrap();
        let abis_msg = decode(&wire).unwrap();

        client_tx.send(&abis_msg).await.unwrap();

        match server_rx.recv().await.unwrap() {
            TransportEvent::Message(received) => {
                assert_eq!(received.message_type, abis_msg.message_type);
                let re_encoded = encode(&received).unwrap();
                let decoded = BtsReleaseMessage::decode(&re_encoded).unwrap();
                assert_eq!(
                    decoded.call_connection_reference,
                    msg.call_connection_reference
                );
            }
            other => panic!("expected Message, got {other:?}"),
        }

        let reply = BtsReleaseAckMessage {
            call_connection_reference: msg.call_connection_reference,
            correlation_id: None,
        };
        let reply_wire = reply.encode().unwrap();
        let reply_abis = decode(&reply_wire).unwrap();

        server_tx.send(&reply_abis).await.unwrap();

        match client_rx.recv().await.unwrap() {
            TransportEvent::Message(received) => {
                assert_eq!(received.message_type, MessageType::BtsReleaseAck);
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_with_reconnect_retries_then_succeeds() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        // addr is now refusing connections — start reconnect in background
        let reconnect = tokio::spawn(async move { connect_with_reconnect(addr).await });

        // Let it fail a few times
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // Re-bind the same port so the next attempt succeeds
        let listener = TcpListener::bind(addr).await.unwrap();
        let accept_handle = tokio::spawn(async move { accept(&listener).await.unwrap() });

        let (client_tx, _client_rx) = reconnect.await.unwrap().unwrap();
        let (_server_tx, mut server_rx) = accept_handle.await.unwrap();

        drop(client_tx);
        match server_rx.recv().await.unwrap() {
            TransportEvent::Disconnected(_) => {}
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn accept_with_retry_succeeds_immediately() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle = tokio::spawn(async move { accept_with_retry(&listener).await });

        let (_client_tx, _client_rx) = connect(addr).await.unwrap();
        let (_server_tx, _server_rx) = accept_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn transport_disconnect_event() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle = tokio::spawn(async move { accept(&listener).await.unwrap() });

        let (client_tx, _client_rx) = connect(addr).await.unwrap();
        let (_server_tx, mut server_rx) = accept_handle.await.unwrap();

        drop(client_tx);

        match server_rx.recv().await.unwrap() {
            TransportEvent::Disconnected(_) => {}
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }
}
