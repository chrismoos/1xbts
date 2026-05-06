//! TCP-backed [`MscClient`] that speaks A1 signaling over TCP transport.

use std::net::SocketAddr;

use log::warn;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use cdma_ios::transport::{A1TransportEvent, A1TransportSender};
use cdma_ios::{A1TransportError, EncodedA1Message};

use super::MscClient;

/// Network-backed A1 client connecting to an MSC over TCP.
pub struct NetworkMscClient {
    sender: A1TransportSender,
    events_rx: Mutex<tokio::sync::mpsc::Receiver<A1TransportEvent>>,
}

impl NetworkMscClient {
    /// Connects to an MSC A1 signaling endpoint.
    pub async fn connect(addr: SocketAddr) -> Result<Self, std::io::Error> {
        let (sender, events_rx) = cdma_ios::transport::connect(addr).await?;
        Ok(Self {
            sender,
            events_rx: Mutex::new(events_rx),
        })
    }

    /// Connects with exponential backoff retry.
    pub async fn connect_with_reconnect(addr: SocketAddr) -> Result<Self, std::io::Error> {
        let (sender, events_rx) = cdma_ios::transport::connect_with_reconnect(addr).await?;
        Ok(Self {
            sender,
            events_rx: Mutex::new(events_rx),
        })
    }

    /// Wraps an already-established transport pair.
    pub fn from_transport(
        sender: A1TransportSender,
        events_rx: tokio::sync::mpsc::Receiver<A1TransportEvent>,
    ) -> Self {
        Self {
            sender,
            events_rx: Mutex::new(events_rx),
        }
    }
}

#[tonic::async_trait]
impl MscClient for NetworkMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        self.sender.send(&message).await
    }

    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, A1TransportError> {
        let mut rx = self.events_rx.lock().await;
        match rx.recv().await {
            Some(A1TransportEvent::Message(msg)) => Ok(Some(msg)),
            Some(A1TransportEvent::Disconnected(e)) => {
                warn!("A1 network client: peer disconnected: {e}");
                Err(A1TransportError::Io(e))
            }
            None => Ok(None),
        }
    }
}

/// MSC-side A1 endpoint that accepts a BSC connection.
pub struct MscA1Endpoint {
    sender: A1TransportSender,
    events_rx: Mutex<tokio::sync::mpsc::Receiver<A1TransportEvent>>,
}

impl MscA1Endpoint {
    /// Accepts one BSC connection on the given listener.
    pub async fn accept(listener: &TcpListener) -> Result<Self, std::io::Error> {
        let (sender, events_rx) = cdma_ios::transport::accept(listener).await?;
        Ok(Self {
            sender,
            events_rx: Mutex::new(events_rx),
        })
    }

    /// Accepts with retry on transient errors.
    pub async fn accept_with_retry(listener: &TcpListener) -> Result<Self, std::io::Error> {
        let (sender, events_rx) = cdma_ios::transport::accept_with_retry(listener).await?;
        Ok(Self {
            sender,
            events_rx: Mutex::new(events_rx),
        })
    }

    /// Wraps an already-established transport pair.
    pub fn from_transport(
        sender: A1TransportSender,
        events_rx: tokio::sync::mpsc::Receiver<A1TransportEvent>,
    ) -> Self {
        Self {
            sender,
            events_rx: Mutex::new(events_rx),
        }
    }

    /// Receives one A1 message from the BSC.
    pub async fn recv_from_bsc(&self) -> Option<EncodedA1Message> {
        let mut rx = self.events_rx.lock().await;
        loop {
            match rx.recv().await {
                Some(A1TransportEvent::Message(msg)) => return Some(msg),
                Some(A1TransportEvent::Disconnected(e)) => {
                    warn!("MSC A1 endpoint: BSC disconnected: {e}");
                    return None;
                }
                None => return None,
            }
        }
    }

    /// Sends one A1 message toward the BSC.
    pub async fn send_to_bsc(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        self.sender.send(&message).await
    }
}

#[async_trait::async_trait]
impl cdma_msc::MscA1Endpoint for MscA1Endpoint {
    async fn recv_from_bsc(&self) -> Option<EncodedA1Message> {
        self.recv_from_bsc().await
    }

    async fn send_to_bsc(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        self.send_to_bsc(message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_ios::{ConnectMessage, Message, MessageType};

    #[tokio::test]
    async fn network_msc_client_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let accept_handle =
            tokio::spawn(async move { MscA1Endpoint::accept(&listener).await.unwrap() });

        let client = NetworkMscClient::connect(addr).await.unwrap();
        let endpoint = accept_handle.await.unwrap();

        let outbound = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::Connect, ConnectMessage.encode().unwrap()),
            Some(99),
        );

        client.send_a1(outbound.clone()).await.unwrap();
        let received = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(received.message_type(), MessageType::Connect);
        assert_eq!(received.call_id(), Some(99));

        endpoint.send_to_bsc(received).await.unwrap();
        let echoed = client.poll_a1().await.unwrap().unwrap();
        assert_eq!(echoed.message_type(), MessageType::Connect);
        assert_eq!(echoed.call_id(), Some(99));
    }
}
