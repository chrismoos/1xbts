//! Async TCP server for the A21 reference point.
//!
//! The A21 boundary is a real network boundary — callers cannot bypass this
//! socket to reach the peer in-process. Each accepted TCP connection runs a
//! framed read loop that dispatches inbound [`A21Message`]s into a user
//! [`A21Handler`].

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::error::{A21Error, Result};
use crate::message::A21Message;
use crate::transport::{read_frame, write_frame};

/// Application-side handler for inbound A21 messages.
///
/// Implementations get a [`A21Connection`] handle for sending replies on the
/// same TCP connection that delivered the request. Returning an `Err` from
/// any handler method tears the connection down.
pub trait A21Handler: Send + Sync + 'static {
    fn on_identity_binding(
        &self,
        peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn on_identity_release(
        &self,
        peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn on_cross_page_request(
        &self,
        peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
        source: crate::message::PagingSource,
        payload: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn on_cross_page_ack(
        &self,
        peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
        accepted: bool,
        reason: Option<String>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn on_suppression_start(
        &self,
        peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
        source: crate::message::PagingSource,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn on_suppression_end(
        &self,
        peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

/// Server-side handle that lets a handler reply on the inbound connection.
#[derive(Clone)]
pub struct A21Connection {
    pub(crate) inner: Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>,
}

impl A21Connection {
    /// Sends one A21 message back to the connected peer.
    pub async fn send(&self, msg: &A21Message) -> Result<()> {
        let mut guard = self.inner.lock().await;
        write_frame(&mut *guard, msg).await
    }
}

/// Tokio TCP server for the A21 reference point.
pub struct A21Server {
    listener: TcpListener,
}

impl A21Server {
    /// Binds the server to `addr` without yet accepting connections.
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }

    /// Returns the actual bound local address (useful when port 0 was requested).
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accepts and serves connections until the listener errors.
    ///
    /// Each connection runs in its own Tokio task; per-connection handler
    /// errors close that connection but do not stop the server.
    pub async fn serve<H: A21Handler>(self, handler: H) -> Result<()> {
        let handler = Arc::new(handler);
        loop {
            let (stream, peer) = self.listener.accept().await?;
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let _ = serve_connection(stream, peer, handler).await;
            });
        }
    }

    /// Same as `serve` but registers each accepted connection with an
    /// `A21Hub` so the application can broadcast outbound messages to all
    /// currently-connected peers.
    pub async fn serve_with_hub<H: A21Handler>(
        self,
        handler: H,
        hub: crate::hub::A21Hub,
    ) -> Result<()> {
        let handler = Arc::new(handler);
        loop {
            let (stream, peer) = self.listener.accept().await?;
            let handler = Arc::clone(&handler);
            let hub = hub.clone();
            tokio::spawn(async move {
                let _ = serve_connection_with_hub(stream, peer, handler, hub).await;
            });
        }
    }
}

async fn serve_connection_with_hub<H: A21Handler>(
    stream: TcpStream,
    peer: SocketAddr,
    handler: Arc<H>,
    hub: crate::hub::A21Hub,
) -> Result<()> {
    let (mut rd, wr) = stream.into_split();
    let conn = A21Connection {
        inner: Arc::new(Mutex::new(wr)),
    };
    hub.register(conn.clone()).await;
    loop {
        let msg = match read_frame(&mut rd).await {
            Ok(m) => m,
            Err(A21Error::Closed) => return Ok(()),
            Err(e) => return Err(e),
        };
        let c = conn.clone();
        dispatch_message(handler.as_ref(), peer, c, msg).await?;
    }
}

async fn dispatch_message<H: A21Handler>(
    handler: &H,
    peer: SocketAddr,
    c: A21Connection,
    msg: A21Message,
) -> Result<()> {
    match msg {
        A21Message::IdentityBinding { imsi } => handler.on_identity_binding(peer, c, imsi).await,
        A21Message::IdentityRelease { imsi } => handler.on_identity_release(peer, c, imsi).await,
        A21Message::CrossPageRequest {
            imsi,
            source,
            payload,
        } => {
            handler
                .on_cross_page_request(peer, c, imsi, source, payload)
                .await
        }
        A21Message::CrossPageAck {
            imsi,
            accepted,
            reason,
        } => {
            handler
                .on_cross_page_ack(peer, c, imsi, accepted, reason)
                .await
        }
        A21Message::SuppressionStart { imsi, source } => {
            handler.on_suppression_start(peer, c, imsi, source).await
        }
        A21Message::SuppressionEnd { imsi } => handler.on_suppression_end(peer, c, imsi).await,
    }
}

async fn serve_connection<H: A21Handler>(
    stream: TcpStream,
    peer: SocketAddr,
    handler: Arc<H>,
) -> Result<()> {
    let (mut rd, wr) = stream.into_split();
    let conn = A21Connection {
        inner: Arc::new(Mutex::new(wr)),
    };
    loop {
        let msg = match read_frame(&mut rd).await {
            Ok(m) => m,
            Err(A21Error::Closed) => return Ok(()),
            Err(e) => return Err(e),
        };
        let c = conn.clone();
        match msg {
            A21Message::IdentityBinding { imsi } => {
                handler.on_identity_binding(peer, c, imsi).await?
            }
            A21Message::IdentityRelease { imsi } => {
                handler.on_identity_release(peer, c, imsi).await?
            }
            A21Message::CrossPageRequest {
                imsi,
                source,
                payload,
            } => {
                handler
                    .on_cross_page_request(peer, c, imsi, source, payload)
                    .await?
            }
            A21Message::CrossPageAck {
                imsi,
                accepted,
                reason,
            } => {
                handler
                    .on_cross_page_ack(peer, c, imsi, accepted, reason)
                    .await?
            }
            A21Message::SuppressionStart { imsi, source } => {
                handler.on_suppression_start(peer, c, imsi, source).await?
            }
            A21Message::SuppressionEnd { imsi } => {
                handler.on_suppression_end(peer, c, imsi).await?
            }
        }
    }
}
