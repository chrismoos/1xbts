//! `IpTransport` implementation that bridges HRPD Stream 1 (Default Packet
//! Application) PDUs to the existing packet-session machinery.
//!
//! Direction summary:
//! - **Network → Mobile (downlink):** the caller (e.g. cdma-an, when a PDSN
//!   bearer frame arrives) pushes IP packets into `downlink_tx`. The
//!   transport forwards each packet via `to_mobile_tx` so `run_session`
//!   delivers it as RLP frames into the AN's Stream 1 SDU pipeline.
//! - **Mobile → Network (uplink):** `run_session` calls `send_to_network`
//!   with each decoded IP packet; the transport pushes onto `uplink_tx` so
//!   the AN runtime can forward upstream (to the PDSN).
//!
//! Compared with `TunTransport` / `FouTransport` this transport never
//! touches a kernel device or socket — both directions are pure in-process
//! mpsc channels owned by the application.

use std::io;
use std::net::Ipv4Addr;
use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::ip_transport::IpTransport;

/// Bridge between cdma-packet's `IpTransport` and an HRPD AN's Stream-1
/// pipeline.
pub struct HrpdStreamTransport {
    /// Channel for IP packets headed *out* of the mobile (uplink → PDSN).
    /// Cloned by the AN runtime to drain into the upstream side.
    uplink_tx: mpsc::Sender<Vec<u8>>,
    /// Receiver for IP packets headed *to* the mobile (downlink ← PDSN). The
    /// transport spawns a forwarder task on `setup` that drains this into
    /// `to_mobile_tx` so `run_session` sees one downlink packet per recv.
    downlink_rx: Mutex<Option<mpsc::Receiver<Vec<u8>>>>,
    /// Worker handle for the downlink forwarder; aborted on teardown.
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl HrpdStreamTransport {
    /// Build a transport plus the two halves needed by the AN side:
    /// - `uplink_rx`: receives one IP packet per `send_to_network` call.
    /// - `downlink_tx`: each packet pushed here becomes a downlink IP
    ///   delivery to the mobile via `run_session`.
    pub fn new() -> (Self, mpsc::Receiver<Vec<u8>>, mpsc::Sender<Vec<u8>>) {
        let (uplink_tx, uplink_rx) = mpsc::channel(64);
        let (downlink_tx, downlink_rx) = mpsc::channel(64);
        (
            Self {
                uplink_tx,
                downlink_rx: Mutex::new(Some(downlink_rx)),
                worker: Mutex::new(None),
            },
            uplink_rx,
            downlink_tx,
        )
    }
}

impl IpTransport for HrpdStreamTransport {
    fn setup(
        &mut self,
        _local_ip: Ipv4Addr,
        _peer_ip: Ipv4Addr,
        to_mobile_tx: mpsc::Sender<Vec<u8>>,
    ) -> io::Result<String> {
        let rx = self.downlink_rx.lock().unwrap().take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::AlreadyExists, "transport already set up")
        })?;
        let handle = tokio::spawn(async move {
            let mut rx = rx;
            while let Some(pkt) = rx.recv().await {
                if to_mobile_tx.send(pkt).await.is_err() {
                    break;
                }
            }
        });
        *self.worker.lock().unwrap() = Some(handle);
        Ok("hrpd-stream-1".to_string())
    }

    fn send_to_network(&self, ip_packet: &[u8]) -> io::Result<()> {
        // try_send so we don't block the synth/run_session loop. Drops are
        // logged at the caller; we treat a full uplink_tx as backpressure.
        self.uplink_tx
            .try_send(ip_packet.to_vec())
            .map_err(|e| io::Error::other(format!("hrpd uplink send failed: {e}")))
    }

    fn teardown(&mut self) {
        if let Some(h) = self.worker.lock().unwrap().take() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uplink_send_to_network_pushes_into_uplink_rx() {
        let (t, mut uplink_rx, _downlink_tx) = HrpdStreamTransport::new();
        let pkt = vec![0x45, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00];
        t.send_to_network(&pkt).unwrap();
        let received = uplink_rx.recv().await.unwrap();
        assert_eq!(received, pkt);
    }

    #[tokio::test]
    async fn downlink_tx_forwards_to_setup_to_mobile_tx() {
        let (mut t, _uplink_rx, downlink_tx) = HrpdStreamTransport::new();
        let (to_mobile_tx, mut to_mobile_rx) = mpsc::channel(8);
        t.setup(
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            to_mobile_tx,
        )
        .unwrap();
        let pkt = vec![1, 2, 3, 4];
        downlink_tx.send(pkt.clone()).await.unwrap();
        let received = to_mobile_rx.recv().await.unwrap();
        assert_eq!(received, pkt);
        t.teardown();
    }

    #[tokio::test]
    async fn second_setup_fails() {
        let (mut t, _uplink_rx, _downlink_tx) = HrpdStreamTransport::new();
        let (to_mobile_tx, _to_mobile_rx) = mpsc::channel(1);
        t.setup(
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::UNSPECIFIED,
            to_mobile_tx.clone(),
        )
        .unwrap();
        let err = t
            .setup(Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED, to_mobile_tx)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }
}
