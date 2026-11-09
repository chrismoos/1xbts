//! UDP transport for A11 signaling datagrams (A.S0012-D §3.5).

use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::{
    AuthenticationVerifier, Error, Message, Result, UnverifiedDecodeReason, VerifiedMessage,
    decode_unverified, decode_verified, encode,
};

/// A.S0012-D §3.5. Replies return to the request's source port.
pub const A11_STANDARD_UDP_PORT: u16 = 699;

/// No outer header (A.S0012-D §3.5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpFrame {
    /// Encoded A11 message carried by this frame.
    pub message: Message,
}

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

    pub fn encode(&self) -> Result<Vec<u8>> {
        encode(&self.message)
    }

    pub fn decode_unverified(input: &[u8], reason: UnverifiedDecodeReason) -> Result<Self> {
        let message = decode_unverified(input, reason)?;
        Ok(Self { message })
    }

    pub fn decode_verified<V>(input: &[u8], verifier: &V) -> Result<VerifiedUdpFrame>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let message = decode_verified(input, verifier)?;
        Ok(VerifiedUdpFrame { message })
    }
}

/// Native UDP/IP endpoint for A11 signaling.
pub struct UdpEndpoint {
    socket: UdpSocket,
}

impl UdpEndpoint {
    pub async fn bind(local_addr: SocketAddr) -> std::io::Result<Self> {
        UdpSocket::bind(local_addr)
            .await
            .map(|socket| Self { socket })
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

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

fn to_io_invalid_data(error: Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}
