//! UDP transport framing helpers for A11 signaling datagrams.

use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::{
    AuthenticationVerifier, Error, Message, Result, UnverifiedDecodeReason, VerifiedMessage,
    decode_unverified, decode_verified, encode,
};

/// Fixed header size for the crate-local A11 UDP frame wrapper.
pub const UDP_FRAME_HEADER_LEN: usize = 2;

/// Length-delimited UDP frame carrying one exact A11 message payload.
///
/// This helper is intentionally transport-local to the crate. It does not model
/// sockets, retries, or peer state. It only provides a reusable datagram format
/// for bundling one encoded A11 message with an explicit payload length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpFrame {
    /// Encoded A11 message carried by this frame.
    pub message: Message,
}

/// Length-delimited UDP frame carrying one verified A11 message payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUdpFrame {
    /// Verified A11 message carried by this frame.
    pub message: VerifiedMessage,
}

impl UdpFrame {
    /// Builds a UDP frame from a typed A11 message.
    pub fn new(message: Message) -> Self {
        Self { message }
    }

    /// Returns the encoded message payload length in bytes.
    pub fn payload_len(&self) -> Result<u16> {
        let payload = encode(&self.message)?;
        u16::try_from(payload.len()).map_err(|_| Error::InvalidValue {
            context: "udp_frame.payload_len",
            reason: "encoded message exceeds u16 transport length",
        })
    }

    /// Encodes the UDP frame as `payload_length | message_bytes`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let payload = encode(&self.message)?;
        let payload_len = u16::try_from(payload.len()).map_err(|_| Error::InvalidValue {
            context: "udp_frame.payload_len",
            reason: "encoded message exceeds u16 transport length",
        })?;

        let mut out = Vec::with_capacity(UDP_FRAME_HEADER_LEN + payload.len());
        out.extend_from_slice(&payload_len.to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Decodes one UDP frame from an exact datagram slice without verifying authentication.
    pub fn decode_unverified(input: &[u8], reason: UnverifiedDecodeReason) -> Result<Self> {
        let payload = udp_frame_payload(input)?;
        let message = decode_unverified(payload, reason)?;
        Ok(Self { message })
    }

    /// Decodes one UDP frame from an exact datagram slice and verifies authentication.
    pub fn decode_verified<V>(input: &[u8], verifier: &V) -> Result<VerifiedUdpFrame>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let payload = udp_frame_payload(input)?;
        let message = decode_verified(payload, verifier)?;
        Ok(VerifiedUdpFrame { message })
    }
}

/// Native UDP/IP endpoint for A11 signaling.
pub struct UdpEndpoint {
    socket: UdpSocket,
}

impl UdpEndpoint {
    /// Binds an A11 UDP endpoint to a local address.
    pub async fn bind(local_addr: SocketAddr) -> std::io::Result<Self> {
        UdpSocket::bind(local_addr)
            .await
            .map(|socket| Self { socket })
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Sends one length-delimited A11 UDP frame to a peer.
    pub async fn send_frame(&self, peer: SocketAddr, frame: &UdpFrame) -> std::io::Result<usize> {
        let bytes = frame.encode().map_err(to_io_invalid_data)?;
        self.socket.send_to(&bytes, peer).await
    }

    /// Sends one typed A11 message to a peer.
    pub async fn send_message(&self, peer: SocketAddr, message: Message) -> std::io::Result<usize> {
        self.send_frame(peer, &UdpFrame::new(message)).await
    }

    /// Receives and decodes one A11 UDP frame without verifying authentication.
    pub async fn recv_frame_unverified(
        &self,
        buf: &mut [u8],
        reason: UnverifiedDecodeReason,
    ) -> std::io::Result<(UdpFrame, SocketAddr)> {
        let (len, peer) = self.socket.recv_from(buf).await?;
        let frame = UdpFrame::decode_unverified(&buf[..len], reason).map_err(to_io_invalid_data)?;
        Ok((frame, peer))
    }

    /// Receives and decodes one typed A11 message without verifying authentication.
    pub async fn recv_message_unverified(
        &self,
        buf: &mut [u8],
        reason: UnverifiedDecodeReason,
    ) -> std::io::Result<(Message, SocketAddr)> {
        let (frame, peer) = self.recv_frame_unverified(buf, reason).await?;
        Ok((frame.message, peer))
    }

    /// Receives, decodes, and verifies one A11 UDP frame.
    pub async fn recv_frame_verified<V>(
        &self,
        buf: &mut [u8],
        verifier: &V,
    ) -> std::io::Result<(VerifiedUdpFrame, SocketAddr)>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let (len, peer) = self.socket.recv_from(buf).await?;
        let frame = UdpFrame::decode_verified(&buf[..len], verifier).map_err(to_io_invalid_data)?;
        Ok((frame, peer))
    }

    /// Receives, decodes, and verifies one typed A11 message.
    pub async fn recv_message_verified<V>(
        &self,
        buf: &mut [u8],
        verifier: &V,
    ) -> std::io::Result<(VerifiedMessage, SocketAddr)>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let (frame, peer) = self.recv_frame_verified(buf, verifier).await?;
        Ok((frame.message, peer))
    }
}

fn udp_frame_payload(input: &[u8]) -> Result<&[u8]> {
    if input.len() < UDP_FRAME_HEADER_LEN {
        return Err(Error::Truncated {
            needed: UDP_FRAME_HEADER_LEN,
            actual: input.len(),
        });
    }

    let payload_len = u16::from_be_bytes([input[0], input[1]]) as usize;
    let total_len = UDP_FRAME_HEADER_LEN + payload_len;
    if input.len() < total_len {
        return Err(Error::Truncated {
            needed: total_len,
            actual: input.len(),
        });
    }
    if input.len() != total_len {
        return Err(Error::InvalidValue {
            context: "udp_frame.length",
            reason: "frame contains trailing bytes beyond the declared payload",
        });
    }

    Ok(&input[UDP_FRAME_HEADER_LEN..total_len])
}

fn to_io_invalid_data(error: Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}
