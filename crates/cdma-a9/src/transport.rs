//! Versioned UDP framing for A9 signaling messages.

use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::{Error, Message, MessageType, Result, decode, encode};

/// Current version of the crate-local A9 UDP signaling wrapper.
pub const VERSION: u8 = 1;

/// Fixed header size, in octets, for the crate-local A9 UDP signaling wrapper.
pub const HEADER_LEN: usize = 16;

/// Transport metadata carried ahead of the A9 message bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportMetadata {
    /// Bitfield reserved for transport-scoped options.
    pub flags: u8,
    /// Local transport session identifier used to correlate datagrams with session state.
    pub session_id: u32,
    /// Per-session transport sequence number.
    pub sequence_no: u32,
}

/// Versioned UDP datagram carrying one encoded A9 signaling message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpSignalingDatagram {
    /// Transport metadata carried in the fixed wrapper header.
    pub metadata: TransportMetadata,
    /// Message type repeated in the wrapper header for transport-friendly routing.
    pub message_type: MessageType,
    /// Encoded A9 message bytes, starting with the A9 message type octet.
    pub payload: Vec<u8>,
}

impl UdpSignalingDatagram {
    /// Builds a datagram from already encoded A9 message bytes.
    pub fn new(metadata: TransportMetadata, payload: impl Into<Vec<u8>>) -> Result<Self> {
        let payload = payload.into();
        let Some((&message_type, _)) = payload.split_first() else {
            return Err(Error::EmptyMessage);
        };
        let message_type = MessageType::from_u8(message_type)?;
        if payload.len() > u16::MAX as usize {
            return Err(Error::InvalidLength {
                expected: u16::MAX as usize,
                actual: payload.len(),
            });
        }
        Ok(Self {
            metadata,
            message_type,
            payload,
        })
    }

    /// Builds a datagram from a decoded A9 message by encoding it first.
    pub fn from_message(metadata: TransportMetadata, message: &Message) -> Result<Self> {
        Self::new(metadata, encode(message)?)
    }

    /// Encodes the datagram into `header | payload`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.payload.len() > u16::MAX as usize {
            return Err(Error::InvalidLength {
                expected: u16::MAX as usize,
                actual: self.payload.len(),
            });
        }
        let Some((&message_type, _)) = self.payload.split_first() else {
            return Err(Error::EmptyMessage);
        };
        let payload_message_type = MessageType::from_u8(message_type)?;
        if payload_message_type != self.message_type {
            return Err(Error::PayloadMessageTypeMismatch {
                header: self.message_type,
                payload: payload_message_type,
            });
        }
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(VERSION);
        out.push(self.metadata.flags);
        out.push(self.message_type as u8);
        out.push(0);
        out.extend_from_slice(&self.metadata.session_id.to_be_bytes());
        out.extend_from_slice(&self.metadata.sequence_no.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Decodes the fixed wrapper header and validates the wrapped A9 message bytes.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < HEADER_LEN {
            return Err(Error::Truncated {
                needed: HEADER_LEN,
                actual: input.len(),
            });
        }
        if input[0] != VERSION {
            return Err(Error::InvalidValue {
                context: "A9 UDP signaling wrapper version",
                value: input[0] as u32,
            });
        }
        let message_type = MessageType::from_u8(input[2])?;
        let payload_len = u16::from_be_bytes([input[12], input[13]]) as usize;
        let total_len = HEADER_LEN + payload_len;
        if input.len() < total_len {
            return Err(Error::Truncated {
                needed: total_len,
                actual: input.len(),
            });
        }
        if input.len() != total_len {
            return Err(Error::InvalidLength {
                expected: total_len,
                actual: input.len(),
            });
        }
        let payload = input[HEADER_LEN..].to_vec();
        let Some((&payload_type, _)) = payload.split_first() else {
            return Err(Error::EmptyMessage);
        };
        let payload_message_type = MessageType::from_u8(payload_type)?;
        if payload_message_type != message_type {
            return Err(Error::PayloadMessageTypeMismatch {
                header: message_type,
                payload: payload_message_type,
            });
        }
        Ok(Self {
            metadata: TransportMetadata {
                flags: input[1],
                session_id: u32::from_be_bytes([input[4], input[5], input[6], input[7]]),
                sequence_no: u32::from_be_bytes([input[8], input[9], input[10], input[11]]),
            },
            message_type,
            payload,
        })
    }

    /// Decodes the wrapped A9 message using the crate's exact A9 message codec.
    pub fn decode_message(&self) -> Result<Message> {
        decode(&self.payload)
    }
}

/// Native UDP/IP endpoint for A9 signaling.
pub struct UdpSignalingEndpoint {
    socket: UdpSocket,
}

impl UdpSignalingEndpoint {
    /// Binds an A9 UDP endpoint to a local address.
    pub async fn bind(local_addr: SocketAddr) -> std::io::Result<Self> {
        UdpSocket::bind(local_addr)
            .await
            .map(|socket| Self { socket })
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Sends one encoded A9 datagram to a peer.
    pub async fn send_datagram(
        &self,
        peer: SocketAddr,
        datagram: &UdpSignalingDatagram,
    ) -> std::io::Result<usize> {
        let bytes = datagram.encode().map_err(to_io_invalid_data)?;
        self.socket.send_to(&bytes, peer).await
    }

    /// Sends one typed A9 message to a peer using the crate-local UDP wrapper.
    pub async fn send_message(
        &self,
        peer: SocketAddr,
        metadata: TransportMetadata,
        message: &Message,
    ) -> std::io::Result<usize> {
        let datagram =
            UdpSignalingDatagram::from_message(metadata, message).map_err(to_io_invalid_data)?;
        self.send_datagram(peer, &datagram).await
    }

    /// Receives and decodes one A9 UDP datagram.
    pub async fn recv_datagram(
        &self,
        buf: &mut [u8],
    ) -> std::io::Result<(UdpSignalingDatagram, SocketAddr)> {
        let (len, peer) = self.socket.recv_from(buf).await?;
        let datagram = UdpSignalingDatagram::decode(&buf[..len]).map_err(to_io_invalid_data)?;
        Ok((datagram, peer))
    }

    /// Receives and decodes one typed A9 message.
    pub async fn recv_message(&self, buf: &mut [u8]) -> std::io::Result<(Message, SocketAddr)> {
        let (datagram, peer) = self.recv_datagram(buf).await?;
        let message = datagram.decode_message().map_err(to_io_invalid_data)?;
        Ok((message, peer))
    }
}

fn to_io_invalid_data(error: Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}
