//! Async TCP transport for A1 signaling between BSC and MSC.
//!
//! Uses a simple frame format: `[0xA1][0x01][u16 BE length][u64 BE call_id][payload]`.
//! The call_id field carries transport-level call correlation (analogous to
//! SCCP connection references in real IOS networks).

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use log::{debug, info, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::a1_message::{A1TransportError, EncodedA1Message};

/// Default A1 signaling port.
pub const A1_SIGNALING_PORT: u16 = 17013;

/// A1 transport frame flag bytes.
const FRAME_FLAG: [u8; 2] = [0xA1, 0x01];

/// Frame header: 2 flag + 2 length + 8 call_id = 12 bytes.
const HEADER_LEN: usize = 12;

/// Events emitted by the transport to the consumer.
#[derive(Debug)]
pub enum A1TransportEvent {
    /// A decoded A1 message was received.
    Message(EncodedA1Message),
    /// The peer disconnected or the connection failed.
    Disconnected(io::Error),
}

/// Handle for sending A1 messages to a connected peer.
#[derive(Clone)]
pub struct A1TransportSender {
    tx: mpsc::Sender<Vec<u8>>,
    peer: SocketAddr,
}

impl A1TransportSender {
    /// Encodes and queues an A1 message for transmission.
    pub async fn send(&self, message: &EncodedA1Message) -> Result<(), A1TransportError> {
        let payload = message.as_bytes();
        let call_id = message.call_id().unwrap_or(0);
        let payload_len = payload.len();
        if payload_len > u16::MAX as usize {
            return Err(A1TransportError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "A1 payload exceeds maximum frame size",
            )));
        }

        info!(
            "A1: tx {:?} call_id={:?} ({} bytes) to {}",
            message.message_type(),
            message.call_id(),
            payload_len,
            self.peer,
        );

        let mut frame = Vec::with_capacity(HEADER_LEN + payload_len);
        frame.extend_from_slice(&FRAME_FLAG);
        frame.extend_from_slice(&(payload_len as u16).to_be_bytes());
        frame.extend_from_slice(&call_id.to_be_bytes());
        frame.extend_from_slice(payload);

        self.tx
            .send(frame)
            .await
            .map_err(|_| A1TransportError::Closed)
    }
}

/// Accepts a single A1 signaling connection on the given listener.
pub async fn accept(
    listener: &TcpListener,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let (stream, peer) = listener.accept().await?;
    info!("A1 signaling: accepted connection from {peer}");
    Ok(spawn_transport(stream, peer))
}

/// Connects to a remote A1 signaling peer.
pub async fn connect(
    addr: SocketAddr,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let stream = TcpStream::connect(addr).await?;
    info!("A1 signaling: connected to {addr}");
    Ok(spawn_transport(stream, addr))
}

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Connects to a remote A1 signaling peer with exponential backoff.
pub async fn connect_with_reconnect(
    addr: SocketAddr,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let mut attempt: u32 = 0;
    let mut backoff = INITIAL_BACKOFF;

    loop {
        match connect(addr).await {
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

/// Accepts an A1 connection with retry on transient errors.
pub async fn accept_with_retry(
    listener: &TcpListener,
) -> io::Result<(A1TransportSender, mpsc::Receiver<A1TransportEvent>)> {
    let mut attempt: u32 = 0;
    let mut backoff = INITIAL_BACKOFF;

    loop {
        match accept(listener).await {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                attempt += 1;
                warn!(
                    "A1 signaling: accept failed (attempt {attempt}): {e}; \
                     retrying in {backoff:?}"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

fn spawn_transport(
    stream: TcpStream,
    peer: SocketAddr,
) -> (A1TransportSender, mpsc::Receiver<A1TransportEvent>) {
    let (read_half, write_half) = stream.into_split();
    let (event_tx, event_rx) = mpsc::channel(256);
    let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>(256);

    tokio::spawn(read_loop(read_half, event_tx, peer));
    tokio::spawn(write_loop(write_half, write_rx, peer));

    (A1TransportSender { tx: write_tx, peer }, event_rx)
}

async fn read_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    event_tx: mpsc::Sender<A1TransportEvent>,
    peer: SocketAddr,
) {
    let mut buf = [0u8; 4096];
    let mut accum = Vec::new();

    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                info!("A1 signaling: peer {peer} disconnected");
                let _ = event_tx
                    .send(A1TransportEvent::Disconnected(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "peer disconnected",
                    )))
                    .await;
                return;
            }
            Ok(n) => {
                accum.extend_from_slice(&buf[..n]);
                loop {
                    match decode_frame(&accum) {
                        FrameResult::Complete { message, consumed } => {
                            info!(
                                "A1: rx {:?} call_id={:?} ({} bytes) from {peer}",
                                message.message_type(),
                                message.call_id(),
                                consumed - HEADER_LEN,
                            );
                            accum.drain(..consumed);
                            if event_tx
                                .send(A1TransportEvent::Message(message))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        FrameResult::Incomplete => break,
                        FrameResult::Invalid(reason) => {
                            warn!("A1 signaling: frame error from {peer}: {reason}");
                            resynchronize(&mut accum);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("A1 signaling: read error from {peer}: {e}");
                let _ = event_tx.send(A1TransportEvent::Disconnected(e)).await;
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
            warn!("A1 signaling: write error to {peer}: {e}");
            return;
        }
    }
    debug!("A1 signaling: write loop closed for {peer}");
}

enum FrameResult {
    Complete {
        message: EncodedA1Message,
        consumed: usize,
    },
    Incomplete,
    Invalid(&'static str),
}

fn decode_frame(buf: &[u8]) -> FrameResult {
    if buf.len() < HEADER_LEN {
        return FrameResult::Incomplete;
    }

    if buf[0] != FRAME_FLAG[0] || buf[1] != FRAME_FLAG[1] {
        return FrameResult::Invalid("bad frame flag");
    }

    let payload_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let call_id = u64::from_be_bytes([
        buf[4], buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11],
    ]);
    let frame_len = HEADER_LEN + payload_len;

    if buf.len() < frame_len {
        return FrameResult::Incomplete;
    }

    let payload = buf[HEADER_LEN..frame_len].to_vec();
    let call_id_opt = if call_id == 0 { None } else { Some(call_id) };

    match EncodedA1Message::from_bytes_with_call_id(payload, call_id_opt) {
        Ok(message) => FrameResult::Complete {
            message,
            consumed: frame_len,
        },
        Err(_) => FrameResult::Invalid("A1 payload decode failed"),
    }
}

fn resynchronize(buf: &mut Vec<u8>) {
    let mut offset = 1;
    while offset + 1 < buf.len() {
        if buf[offset] == FRAME_FLAG[0] && buf[offset + 1] == FRAME_FLAG[1] {
            break;
        }
        offset += 1;
    }
    buf.drain(..offset);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectMessage, Message, MessageType};

    #[tokio::test]
    async fn transport_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle = tokio::spawn(async move { accept(&listener).await.unwrap() });
        let (client_tx, mut client_rx) = connect(addr).await.unwrap();
        let (server_tx, mut server_rx) = accept_handle.await.unwrap();

        let msg = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::Connect, ConnectMessage.encode().unwrap()),
            Some(42),
        );

        client_tx.send(&msg).await.unwrap();

        match server_rx.recv().await.unwrap() {
            A1TransportEvent::Message(received) => {
                assert_eq!(received.message_type(), MessageType::Connect);
                assert_eq!(received.call_id(), Some(42));
            }
            other => panic!("expected Message, got {other:?}"),
        }

        let reply = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::ClearComplete,
                crate::ClearCompleteMessage {
                    power_down_indicator: false,
                }
                .encode()
                .unwrap(),
            ),
            Some(42),
        );
        server_tx.send(&reply).await.unwrap();

        match client_rx.recv().await.unwrap() {
            A1TransportEvent::Message(received) => {
                assert_eq!(received.message_type(), MessageType::ClearComplete);
                assert_eq!(received.call_id(), Some(42));
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_no_call_id_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle = tokio::spawn(async move { accept(&listener).await.unwrap() });
        let (client_tx, _client_rx) = connect(addr).await.unwrap();
        let (_server_tx, mut server_rx) = accept_handle.await.unwrap();

        let msg = EncodedA1Message::from_message(&Message::new(
            MessageType::Connect,
            ConnectMessage.encode().unwrap(),
        ));

        client_tx.send(&msg).await.unwrap();

        match server_rx.recv().await.unwrap() {
            A1TransportEvent::Message(received) => {
                assert_eq!(received.message_type(), MessageType::Connect);
                assert_eq!(received.call_id(), None);
            }
            other => panic!("expected Message, got {other:?}"),
        }
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
            A1TransportEvent::Disconnected(_) => {}
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_messages_in_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle = tokio::spawn(async move { accept(&listener).await.unwrap() });
        let (client_tx, _client_rx) = connect(addr).await.unwrap();
        let (_server_tx, mut server_rx) = accept_handle.await.unwrap();

        for i in 1u64..=5 {
            let msg = EncodedA1Message::from_message_for_call(
                &Message::new(MessageType::Connect, ConnectMessage.encode().unwrap()),
                Some(i),
            );
            client_tx.send(&msg).await.unwrap();
        }

        for i in 1u64..=5 {
            match server_rx.recv().await.unwrap() {
                A1TransportEvent::Message(received) => {
                    assert_eq!(received.call_id(), Some(i));
                }
                other => panic!("expected Message for call_id={i}, got {other:?}"),
            }
        }
    }
}
