//! BSC-side MSC/A1 seam.
//!
//! The wire-format `EncodedA1Message` now lives in `cdma-ios` so both BSC and
//! MSC can use it. This module re-exports it and defines the BSC-facing
//! `MscClient` trait plus the in-process loopback used for testing.

pub mod network;

pub use cdma_ios::{A1TransportError, EncodedA1Message};

use tokio::sync::{Mutex, mpsc};

/// Legacy alias — code that referenced `MscClientError` keeps compiling.
pub type MscClientError = A1TransportError;

/// BSC-facing client used to exchange encoded A1 messages with the MSC.
#[tonic::async_trait]
pub trait MscClient: Send + Sync {
    /// Sends one encoded A1 message toward the MSC.
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), A1TransportError>;

    /// Polls for one encoded A1 message coming back from the MSC.
    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, A1TransportError>;
}

/// In-process loopback client implementing the BSC side of the MSC seam.
#[derive(Debug)]
pub struct InProcessMscClient {
    outbound_tx: mpsc::Sender<EncodedA1Message>,
    inbound_rx: Mutex<mpsc::Receiver<EncodedA1Message>>,
}

/// Server-side endpoint paired with [`InProcessMscClient`].
#[derive(Debug)]
pub struct InProcessMscEndpoint {
    inbound_rx: Mutex<mpsc::Receiver<EncodedA1Message>>,
    outbound_tx: mpsc::Sender<EncodedA1Message>,
}

impl InProcessMscClient {
    /// Creates a loopback pair for in-process BSC/MSC integration.
    pub fn pair(buffer: usize) -> (Self, InProcessMscEndpoint) {
        let (bsc_to_msc_tx, bsc_to_msc_rx) = mpsc::channel(buffer);
        let (msc_to_bsc_tx, msc_to_bsc_rx) = mpsc::channel(buffer);
        (
            Self {
                outbound_tx: bsc_to_msc_tx,
                inbound_rx: Mutex::new(msc_to_bsc_rx),
            },
            InProcessMscEndpoint {
                inbound_rx: Mutex::new(bsc_to_msc_rx),
                outbound_tx: msc_to_bsc_tx,
            },
        )
    }
}

#[tonic::async_trait]
impl MscClient for InProcessMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        self.outbound_tx
            .send(message)
            .await
            .map_err(|_| A1TransportError::Closed)
    }

    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, A1TransportError> {
        Ok(self.inbound_rx.lock().await.recv().await)
    }
}

impl InProcessMscEndpoint {
    /// Receives one encoded A1 message from the BSC side.
    pub async fn recv_from_bsc(&self) -> Option<EncodedA1Message> {
        self.inbound_rx.lock().await.recv().await
    }

    /// Sends one encoded A1 message back toward the BSC.
    pub async fn send_to_bsc(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        self.outbound_tx
            .send(message)
            .await
            .map_err(|_| A1TransportError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_ios::{ConnectMessage, Message, MessageType};

    #[tokio::test]
    async fn loopback_pair_roundtrips_a1_messages() {
        let (client, endpoint) = InProcessMscClient::pair(4);
        let outbound = EncodedA1Message::from_message(&Message::new(
            MessageType::Connect,
            ConnectMessage.encode().unwrap(),
        ));

        client.send_a1(outbound.clone()).await.unwrap();
        let received = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(received.message_type(), MessageType::Connect);
        assert_eq!(
            received.decode().unwrap().message_type,
            MessageType::Connect
        );

        endpoint.send_to_bsc(received.clone()).await.unwrap();
        let echoed = client.poll_a1().await.unwrap().unwrap();
        assert_eq!(echoed, received);
    }
}
