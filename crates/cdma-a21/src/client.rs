//! Async TCP client for the A21 reference point.

use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::error::Result;
use crate::message::A21Message;
use crate::transport::{read_frame, write_frame};

/// Tokio TCP client for one A21 peer.
///
/// The client is not internally synchronized: `&mut self` enforces serialized
/// `send` / `recv` calls. Wrap it in a mutex or split into per-direction
/// owned halves at a higher layer if concurrent send/recv is needed.
pub struct A21Client {
    rd: OwnedReadHalf,
    wr: OwnedWriteHalf,
}

impl A21Client {
    /// Opens a TCP connection to an A21 peer.
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        let (rd, wr) = stream.into_split();
        Ok(Self { rd, wr })
    }

    /// Sends one A21 message to the peer.
    pub async fn send(&mut self, msg: &A21Message) -> Result<()> {
        write_frame(&mut self.wr, msg).await
    }

    /// Awaits the next A21 message from the peer.
    ///
    /// Returns [`crate::error::A21Error::Closed`] when the peer cleanly
    /// closes the connection between frames. This is the `incoming()`
    /// equivalent for crates that do not depend on `futures::Stream`.
    pub async fn recv(&mut self) -> Result<A21Message> {
        read_frame(&mut self.rd).await
    }
}
