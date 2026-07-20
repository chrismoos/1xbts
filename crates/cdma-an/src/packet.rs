//! HRPD Default Packet Application adapter: AN-side packet-session bookkeeping.
//!
//! C.S0024-500 §5 + X.S0011-006. Rev 0 defaults bind the Default Packet
//! Application to Stream 1; negotiated subtype paths may bind it elsewhere.
//! Every selected packet-stream SDU
//! emitted by the Stream Layer is delivered to the PCF as uplink data, and
//! incoming PDSN downlink packets are wrapped as Stream-1 PDUs and queued
//! for the next Forward Traffic slot.
//!
//! This module owns the AT-side bookkeeping (UATI → packet session id,
//! uplink/downlink `tokio::sync::mpsc` endpoints, byte counters) and a small trait that
//! the runtime implements to bridge into `cdma-packet` / `cdma-pcf`. The
//! actual PCF client wiring lives in `main.rs` (or the launcher) so this
//! module stays test-friendly without dragging the entire packet stack in.

use std::collections::HashMap;

use cdma_packet::hrpd_stream_transport::HrpdStreamTransport;
use tokio::sync::mpsc;

use crate::uati::Uati;

/// Outcome of an attempt to open a packet session for an AT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketOpenError {
    /// Another session for the same UATI is already open.
    AlreadyOpen,
    /// The configured packet backend rejected the open call.
    BackendRefused(String),
}

/// Per-AT packet session bookkeeping inside the AN. Maps UATI to a stable
/// session identifier that the PCF understands plus running counters.
#[derive(Debug, Default, Clone)]
pub struct PacketAdapter {
    sessions: HashMap<u32, PacketSession>,
    next_session_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketSession {
    pub uati: u32,
    pub pcf_session_id: u64,
    pub uplink_pdus: u64,
    pub downlink_pdus: u64,
    pub uplink_bytes: u64,
    pub downlink_bytes: u64,
}

/// Endpoints handed back to the AN runtime when a Stream-1 session opens.
/// The runtime spawns `cdma_packet::session_task::run_session` with the
/// returned `transport` and drains `uplink_rx` toward the PDSN; downlink IP
/// packets pushed into `downlink_tx` flow back to the mobile.
pub struct OpenedPacketSession {
    pub pcf_session_id: u64,
    pub transport: HrpdStreamTransport,
    pub uplink_rx: mpsc::Receiver<Vec<u8>>,
    pub downlink_tx: mpsc::Sender<Vec<u8>>,
}

impl PacketAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a session for an HRPD AT. Returns the new pcf_session_id only,
    /// for callers that don't yet want a real Stream-1 transport (counters
    /// only). New integrations should prefer `open_with_transport`.
    pub fn open(&mut self, uati: Uati) -> Result<u64, PacketOpenError> {
        let key = uati.as_u32();
        if self.sessions.contains_key(&key) {
            return Err(PacketOpenError::AlreadyOpen);
        }
        self.next_session_id += 1;
        let id = self.next_session_id;
        self.sessions.insert(
            key,
            PacketSession {
                uati: key,
                pcf_session_id: id,
                uplink_pdus: 0,
                downlink_pdus: 0,
                uplink_bytes: 0,
                downlink_bytes: 0,
            },
        );
        Ok(id)
    }

    /// Open a session and hand back an `HrpdStreamTransport` plus the
    /// uplink/downlink endpoints. The caller is responsible for spawning
    /// `cdma_packet::session_task::run_session` with the transport.
    pub fn open_with_transport(
        &mut self,
        uati: Uati,
    ) -> Result<OpenedPacketSession, PacketOpenError> {
        let id = self.open(uati)?;
        let (transport, uplink_rx, downlink_tx) = HrpdStreamTransport::new();
        Ok(OpenedPacketSession {
            pcf_session_id: id,
            transport,
            uplink_rx,
            downlink_tx,
        })
    }

    pub fn close(&mut self, uati: Uati) -> Option<PacketSession> {
        self.sessions.remove(&uati.as_u32())
    }

    pub fn session(&self, uati: Uati) -> Option<&PacketSession> {
        self.sessions.get(&uati.as_u32())
    }

    pub fn observe_uplink(&mut self, uati: Uati, pdu: &[u8]) {
        if let Some(s) = self.sessions.get_mut(&uati.as_u32()) {
            s.uplink_pdus += 1;
            s.uplink_bytes += pdu.len() as u64;
        }
    }

    pub fn observe_downlink(&mut self, uati: Uati, pdu: &[u8]) {
        if let Some(s) = self.sessions.get_mut(&uati.as_u32()) {
            s.downlink_pdus += 1;
            s.downlink_bytes += pdu.len() as u64;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &PacketSession> {
        self.sessions.values()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(v: u32) -> Uati {
        Uati::from_compact(v, [0; 13], 0, 0)
    }

    #[test]
    fn open_assigns_monotonic_session_ids() {
        let mut a = PacketAdapter::new();
        let id1 = a.open(u(0x1)).unwrap();
        let id2 = a.open(u(0x2)).unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn open_duplicate_uati_errors() {
        let mut a = PacketAdapter::new();
        a.open(u(0x1)).unwrap();
        assert_eq!(a.open(u(0x1)), Err(PacketOpenError::AlreadyOpen));
    }

    #[test]
    fn observe_uplink_downlink_updates_counters() {
        let mut a = PacketAdapter::new();
        a.open(u(0x5)).unwrap();
        a.observe_uplink(u(0x5), &[1, 2, 3]);
        a.observe_downlink(u(0x5), &[1, 2, 3, 4, 5]);
        let s = a.session(u(0x5)).unwrap();
        assert_eq!(s.uplink_pdus, 1);
        assert_eq!(s.uplink_bytes, 3);
        assert_eq!(s.downlink_pdus, 1);
        assert_eq!(s.downlink_bytes, 5);
    }

    #[tokio::test]
    async fn open_with_transport_round_trips_uplink_and_downlink() {
        use cdma_packet::ip_transport::IpTransport;
        use std::net::Ipv4Addr;

        let mut a = PacketAdapter::new();
        let mut opened = a.open_with_transport(u(0x1234)).unwrap();
        assert!(opened.pcf_session_id > 0);

        // Drive the transport like run_session would.
        let (to_mobile_tx, mut to_mobile_rx) = mpsc::channel(8);
        opened
            .transport
            .setup(
                Ipv4Addr::new(10, 0, 0, 1),
                Ipv4Addr::new(10, 0, 0, 2),
                to_mobile_tx,
            )
            .unwrap();

        // Uplink: send_to_network -> uplink_rx
        opened.transport.send_to_network(&[1, 2, 3]).unwrap();
        let up = opened.uplink_rx.recv().await.unwrap();
        assert_eq!(up, vec![1, 2, 3]);

        // Downlink: downlink_tx -> to_mobile_tx (via forwarder spawned in setup)
        opened.downlink_tx.send(vec![9, 8, 7]).await.unwrap();
        let down = to_mobile_rx.recv().await.unwrap();
        assert_eq!(down, vec![9, 8, 7]);

        opened.transport.teardown();
    }

    #[test]
    fn close_returns_session_and_removes_it() {
        let mut a = PacketAdapter::new();
        a.open(u(0x9)).unwrap();
        let s = a.close(u(0x9)).unwrap();
        assert_eq!(s.uati, 0x9);
        assert!(a.session(u(0x9)).is_none());
    }
}
