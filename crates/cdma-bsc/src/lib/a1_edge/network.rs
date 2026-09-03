//! TCP-backed [`MscClient`] that serves A1 signaling to the MSC.
//!
//! The MSC initiates A1: it pulls this node's enrollment over gRPC, then
//! dials the address the enrollment names. The BSC listens and accepts one
//! MSC at a time, re-accepting after the MSC goes away so an MSC restart does
//! not strand the cell.

use std::net::SocketAddr;

use log::{info, warn};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use cdma_ios::transport::{A1TransportEvent, A1TransportSender};
use cdma_ios::{A1TransportError, EncodedA1Message};

use super::MscClient;

/// A1 endpoint owning the BSC's listening socket.
pub struct NetworkMscClient {
    listener: TcpListener,
    sender: Mutex<Option<A1TransportSender>>,
    events_rx: Mutex<Option<tokio::sync::mpsc::Receiver<A1TransportEvent>>>,
}

impl NetworkMscClient {
    /// Binds the A1 signaling socket. The MSC connection is accepted lazily,
    /// on the first poll.
    pub async fn bind(addr: SocketAddr) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            sender: Mutex::new(None),
            events_rx: Mutex::new(None),
        })
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }

    async fn drop_link(&self) {
        *self.sender.lock().await = None;
    }
}

#[tonic::async_trait]
impl MscClient for NetworkMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        let sender = self.sender.lock().await.clone();
        match sender {
            Some(sender) => sender.send(&message).await,
            None => Err(A1TransportError::Closed),
        }
    }

    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, A1TransportError> {
        let mut events_rx = self.events_rx.lock().await;
        loop {
            if events_rx.is_none() {
                // Backs off internally and never gives up, so the BSC waits
                // here for the MSC rather than failing its run loop.
                let (sender, rx) = cdma_ios::transport::accept_with_retry(&self.listener)
                    .await
                    .map_err(A1TransportError::Io)?;
                info!("BSC accepted A1 signaling connection from MSC");
                *self.sender.lock().await = Some(sender);
                *events_rx = Some(rx);
            }
            let rx = events_rx.as_mut().expect("A1 link established above");
            match rx.recv().await {
                Some(A1TransportEvent::Message(message)) => return Ok(Some(message)),
                Some(A1TransportEvent::Disconnected(error)) => {
                    warn!("BSC A1 endpoint: MSC disconnected: {error}, awaiting reconnect");
                    *events_rx = None;
                    self.drop_link().await;
                }
                None => {
                    warn!("BSC A1 endpoint: transport closed, awaiting reconnect");
                    *events_rx = None;
                    self.drop_link().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepts_the_msc_and_relays_both_ways() {
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = client.local_addr().unwrap();
        let (msc_sender, mut msc_rx) = cdma_ios::transport::connect(addr).await.unwrap();

        let inbound = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::PagingRequest,
            vec![1, 2, 3],
        ));
        msc_sender.send(&inbound).await.unwrap();
        let received = client.poll_a1().await.unwrap().expect("message");
        assert_eq!(
            received.message_type(),
            cdma_ios::MessageType::PagingRequest
        );

        let outbound = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::CompleteLayer3Information,
            vec![9],
        ));
        client.send_a1(outbound).await.unwrap();
        match msc_rx.recv().await {
            Some(A1TransportEvent::Message(message)) => assert_eq!(
                message.message_type(),
                cdma_ios::MessageType::CompleteLayer3Information
            ),
            other => panic!("expected a message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_send_before_the_msc_connects_reports_closed() {
        let client = NetworkMscClient::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let message = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::PagingRequest,
            vec![],
        ));
        assert!(matches!(
            client.send_a1(message).await,
            Err(A1TransportError::Closed)
        ));
    }
}
