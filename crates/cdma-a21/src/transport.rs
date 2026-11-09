//! A21 over UDP (A.S0008-C §2.8.1). A response returns to the request's
//! source address and port.

use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::error::Result;
use crate::message::A21Message;

/// Registered A21 port (A.S0008-C §2.8.1).
pub const A21_UDP_PORT: u16 = 4597;

/// The BSC keeps [`A21_UDP_PORT`] on a shared address.
pub const SINGLE_HOST_AN_PORT: u16 = 17049;

pub const MAX_DATAGRAM_LEN: usize = 2048;

pub struct A21Endpoint {
    socket: UdpSocket,
}

impl A21Endpoint {
    pub async fn bind(local_addr: SocketAddr) -> Result<Self> {
        Ok(Self {
            socket: UdpSocket::bind(local_addr).await?,
        })
    }

    pub fn from_std(socket: std::net::UdpSocket) -> Result<Self> {
        Ok(Self {
            socket: UdpSocket::from_std(socket)?,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }

    pub async fn send(&self, peer: SocketAddr, message: &A21Message) -> Result<()> {
        self.socket.send_to(&message.encode()?, peer).await?;
        Ok(())
    }

    /// A datagram that does not decode returns an error with its sender.
    pub async fn recv(&self) -> (SocketAddr, Result<A21Message>) {
        let mut buf = [0u8; MAX_DATAGRAM_LEN];
        match self.socket.recv_from(&mut buf).await {
            Ok((len, peer)) => (peer, A21Message::decode(&buf[..len])),
            Err(err) => (SocketAddr::from(([0, 0, 0, 0], 0)), Err(err.into())),
        }
    }
}
