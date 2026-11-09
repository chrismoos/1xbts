//! A9 over UDP or TCP to PCF port 5603 (A.S0012-D §3.4). Over TCP each message
//! has the §2.5.1 delimitation header and only the BS opens connections.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};
use tokio::sync::{Mutex, mpsc};

use crate::{Error, Message, MessageType, Result, decode, encode};

/// Registered A9 signaling port at the PCF (A.S0012-D §3.4.1, §3.4.2).
pub const A9_SIGNALING_PORT: u16 = 5603;

/// Message delimitation header version 1 (A.S0012-D §2.5.1).
pub const TCP_DELIMITER_VERSION: u8 = 0x01;

/// Version octet plus the four-octet length.
pub const TCP_DELIMITER_LEN: usize = 5;

pub const MAX_TCP_MESSAGE_LEN: usize = 64 * 1024;

const RECEIVED_QUEUE_DEPTH: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A9Transport {
    #[default]
    Udp,
    Tcp,
}

/// The type octet is already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalingMessage {
    pub message_type: MessageType,
    /// Encoded A9 message bytes, starting with the message type octet.
    pub payload: Vec<u8>,
}

impl SignalingMessage {
    pub fn new(payload: impl Into<Vec<u8>>) -> Result<Self> {
        let payload = payload.into();
        let Some(&message_type) = payload.first() else {
            return Err(Error::EmptyMessage);
        };
        Ok(Self {
            message_type: MessageType::from_u8(message_type)?,
            payload,
        })
    }

    pub fn decode_message(&self) -> Result<Message> {
        decode(&self.payload)
    }
}

/// Prepends the A.S0012-D §2.5.1 version 1 message delimitation header.
pub fn delimit(payload: &[u8]) -> Result<Vec<u8>> {
    let len = u32::try_from(payload.len()).map_err(|_| Error::InvalidLength {
        expected: u32::MAX as usize,
        actual: payload.len(),
    })?;
    let mut out = Vec::with_capacity(TCP_DELIMITER_LEN + payload.len());
    out.push(TCP_DELIMITER_VERSION);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

#[derive(Debug, Default)]
pub struct DelimitedReader {
    buf: Vec<u8>,
}

impl DelimitedReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_message(&mut self) -> Result<Option<Vec<u8>>> {
        if self.buf.len() < TCP_DELIMITER_LEN {
            return Ok(None);
        }
        if self.buf[0] != TCP_DELIMITER_VERSION {
            return Err(Error::InvalidValue {
                context: "A9 TCP message delimiter version",
                value: u32::from(self.buf[0]),
            });
        }
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_TCP_MESSAGE_LEN {
            return Err(Error::InvalidLength {
                expected: MAX_TCP_MESSAGE_LEN,
                actual: len,
            });
        }
        if self.buf.len() < TCP_DELIMITER_LEN + len {
            return Ok(None);
        }
        let message = self.buf[TCP_DELIMITER_LEN..TCP_DELIMITER_LEN + len].to_vec();
        self.buf.drain(..TCP_DELIMITER_LEN + len);
        Ok(Some(message))
    }
}

pub struct SignalingEndpoint {
    inner: EndpointInner,
}

enum EndpointInner {
    Udp(UdpSocket),
    Tcp(TcpEndpoint),
}

impl SignalingEndpoint {
    pub async fn listen(local_addr: SocketAddr, transport: A9Transport) -> std::io::Result<Self> {
        let inner = match transport {
            A9Transport::Udp => EndpointInner::Udp(UdpSocket::bind(local_addr).await?),
            A9Transport::Tcp => EndpointInner::Tcp(TcpEndpoint::listen(local_addr).await?),
        };
        Ok(Self { inner })
    }

    /// Over TCP the connection opens on the first send.
    pub async fn client(local_addr: SocketAddr, transport: A9Transport) -> std::io::Result<Self> {
        let inner = match transport {
            A9Transport::Udp => EndpointInner::Udp(UdpSocket::bind(local_addr).await?),
            A9Transport::Tcp => EndpointInner::Tcp(TcpEndpoint::client(local_addr)),
        };
        Ok(Self { inner })
    }

    /// For a TCP client, the configured address before any connection.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        match &self.inner {
            EndpointInner::Udp(socket) => socket.local_addr(),
            EndpointInner::Tcp(tcp) => Ok(tcp.local_addr),
        }
    }

    pub async fn send_payload(&self, peer: SocketAddr, payload: &[u8]) -> std::io::Result<()> {
        SignalingMessage::new(payload.to_vec()).map_err(to_io_invalid_data)?;
        match &self.inner {
            EndpointInner::Udp(socket) => {
                socket.send_to(payload, peer).await?;
                Ok(())
            }
            EndpointInner::Tcp(tcp) => tcp.send(peer, payload).await,
        }
    }

    pub async fn send_message(&self, peer: SocketAddr, message: &Message) -> std::io::Result<()> {
        let payload = encode(message).map_err(to_io_invalid_data)?;
        self.send_payload(peer, &payload).await
    }

    pub async fn recv(&self, buf: &mut [u8]) -> std::io::Result<(SignalingMessage, SocketAddr)> {
        let (payload, peer) = match &self.inner {
            EndpointInner::Udp(socket) => {
                let (len, peer) = socket.recv_from(buf).await?;
                (buf[..len].to_vec(), peer)
            }
            EndpointInner::Tcp(tcp) => tcp.recv().await?,
        };
        let message = SignalingMessage::new(payload).map_err(to_io_invalid_data)?;
        Ok((message, peer))
    }

    pub async fn recv_message(&self, buf: &mut [u8]) -> std::io::Result<(Message, SocketAddr)> {
        let (message, peer) = self.recv(buf).await?;
        let message = message.decode_message().map_err(to_io_invalid_data)?;
        Ok((message, peer))
    }
}

type Received = std::io::Result<(Vec<u8>, SocketAddr)>;

/// Over TCP only the BS opens connections (A.S0012-D §3.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpRole {
    Pcf,
    Bs,
}

struct TcpEndpoint {
    local_addr: SocketAddr,
    role: TcpRole,
    writers: Arc<Mutex<HashMap<SocketAddr, OwnedWriteHalf>>>,
    received_tx: mpsc::Sender<Received>,
    received_rx: Mutex<mpsc::Receiver<Received>>,
}

impl TcpEndpoint {
    fn client(local_addr: SocketAddr) -> Self {
        Self::new(local_addr, TcpRole::Bs)
    }

    fn new(local_addr: SocketAddr, role: TcpRole) -> Self {
        let (received_tx, received_rx) = mpsc::channel(RECEIVED_QUEUE_DEPTH);
        Self {
            local_addr,
            role,
            writers: Arc::new(Mutex::new(HashMap::new())),
            received_tx,
            received_rx: Mutex::new(received_rx),
        }
    }

    async fn listen(local_addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(local_addr).await?;
        let endpoint = Self::new(listener.local_addr()?, TcpRole::Pcf);
        let writers = Arc::clone(&endpoint.writers);
        let received_tx = endpoint.received_tx.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        log::info!("A9 TCP connection accepted peer={peer}");
                        attach(stream, peer, &writers, &received_tx).await;
                    }
                    Err(err) => {
                        log::warn!("A9 TCP accept failed: {err}");
                        if received_tx.send(Err(err)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Ok(endpoint)
    }

    async fn send(&self, peer: SocketAddr, payload: &[u8]) -> std::io::Result<()> {
        let frame = delimit(payload).map_err(to_io_invalid_data)?;
        let mut writers = self.writers.lock().await;
        if !writers.contains_key(&peer) && self.role == TcpRole::Bs {
            drop(writers);
            let stream = self.connect(peer).await?;
            attach(stream, peer, &self.writers, &self.received_tx).await;
            writers = self.writers.lock().await;
        }
        let Some(writer) = writers.get_mut(&peer) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                format!("no A9 TCP connection to {peer}"),
            ));
        };
        if let Err(err) = writer.write_all(&frame).await {
            writers.remove(&peer);
            return Err(err);
        }
        Ok(())
    }

    async fn connect(&self, peer: SocketAddr) -> std::io::Result<TcpStream> {
        let socket = if peer.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        if self.local_addr.is_ipv4() == peer.is_ipv4() {
            socket.bind(SocketAddr::new(self.local_addr.ip(), 0))?;
        }
        let stream = socket.connect(peer).await?;
        log::info!("A9 TCP connection opened peer={peer}");
        Ok(stream)
    }

    async fn recv(&self) -> std::io::Result<(Vec<u8>, SocketAddr)> {
        let mut received = self.received_rx.lock().await;
        match received.recv().await {
            Some(result) => result,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "A9 TCP endpoint closed",
            )),
        }
    }
}

async fn attach(
    stream: TcpStream,
    peer: SocketAddr,
    writers: &Arc<Mutex<HashMap<SocketAddr, OwnedWriteHalf>>>,
    received_tx: &mpsc::Sender<Received>,
) {
    if let Err(err) = stream.set_nodelay(true) {
        log::debug!("A9 TCP set_nodelay failed peer={peer}: {err}");
    }
    let (mut reader, writer) = stream.into_split();
    writers.lock().await.insert(peer, writer);
    let writers = Arc::clone(writers);
    let received_tx = received_tx.clone();
    tokio::spawn(async move {
        let mut delimited = DelimitedReader::default();
        let mut chunk = vec![0u8; 4096];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(len) => delimited.push(&chunk[..len]),
                Err(err) => {
                    log::warn!("A9 TCP read failed peer={peer}: {err}");
                    break;
                }
            }
            loop {
                match delimited.next_message() {
                    Ok(Some(message)) => {
                        if received_tx.send(Ok((message, peer))).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        log::warn!("A9 TCP stream from {peer} is not delimited: {err}");
                        writers.lock().await.remove(&peer);
                        return;
                    }
                }
            }
        }
        log::info!("A9 TCP connection closed peer={peer}");
        writers.lock().await.remove(&peer);
    });
}

fn to_io_invalid_data(error: Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}
