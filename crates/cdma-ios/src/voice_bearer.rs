//! A2p voice bearer transport (A.S0014-D v2.0 §4.2.89/§4.2.90).
//!
//! Carries circuit-switched voice frames between the MSC media plane and the
//! BSC radio-access bearer relay using RTP over UDP/IP per 3GPP2 A.S0014-D.
//! Payload format: EVRC header-full mode per RFC 3558 (Bearer Format ID 3,
//! Table 4.2.90-3).
//!
//! Each voice circuit gets its own UDP socket pair, with bearer parameters
//! (IP address and UDP port) exchanged via A2p Bearer Session-Level Parameters
//! IE (0x45) in AssignmentRequest (MSC→BSC) and AssignmentComplete (BSC→MSC).

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Bearer Format IDs from A.S0014-D Table 4.2.90-3.
pub mod bearer_format_id {
    pub const PCMU: u8 = 0;
    pub const PCMA: u8 = 1;
    pub const VOCODER_13K: u8 = 2;
    pub const EVRC: u8 = 3;
    pub const EVRC0: u8 = 4;
    pub const SMV: u8 = 5;
    pub const SMV0: u8 = 6;
    pub const TELEPHONE_EVENT: u8 = 7;
    pub const EVRCB: u8 = 8;
    pub const EVRCB0: u8 = 9;
    pub const EVRCWB: u8 = 0xA;
    pub const EVRCWB0: u8 = 0xB;
    pub const EVRCNW: u8 = 0xC;
    pub const EVRCNW0: u8 = 0xD;
}

/// Default dynamic PT for EVRC. The actual PT is negotiated per call via the
/// BearerFormatEntry IE (A.S0014-D §4.2.90); this is the historical fallback.
pub const EVRC_RTP_PAYLOAD_TYPE: u8 = 96;

/// Default dynamic PT for telephone-event. Negotiated per call; see EVRC.
pub const TELEPHONE_EVENT_RTP_PAYLOAD_TYPE: u8 = 101;

const TIMESTAMP_INCREMENT: u32 = 160;

/// RFC 4733 §2.5.1.4: end-of-event packet is repeated three times.
pub const RFC4733_END_REPEAT_COUNT: usize = 3;

const RTP_HEADER_LEN: usize = 12;

/// RFC 3558 §4, Table 1: EVRC frame type codes used in the ToC entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EvrcFrameType {
    Blank = 0,
    Eighth = 1,
    Quarter = 2,
    Half = 3,
    Full = 4,
    Erasure = 14,
}

impl EvrcFrameType {
    pub fn from_rate_bps(rate_bps: u32) -> Self {
        match rate_bps {
            9600 => Self::Full,
            4800 => Self::Half,
            2400 => Self::Quarter,
            1200 => Self::Eighth,
            _ => Self::Blank,
        }
    }

    pub fn to_rate_bps(self) -> u32 {
        match self {
            Self::Full => 9600,
            Self::Half => 4800,
            Self::Quarter => 2400,
            Self::Eighth => 1200,
            Self::Erasure | Self::Blank => 0,
        }
    }

    fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Blank),
            1 => Some(Self::Eighth),
            2 => Some(Self::Quarter),
            3 => Some(Self::Half),
            4 => Some(Self::Full),
            14 => Some(Self::Erasure),
            _ => None,
        }
    }
}

/// RFC 4733 telephone-event report on the A2p bearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtmfBearerEvent {
    pub circuit_id: u16,
    pub event: u8,
    pub volume: u8,
    pub duration_samples: u16,
    pub end: bool,
    /// RTP marker bit, set on the first packet of an event (RFC 4733 §2.5).
    pub start_of_event: bool,
}

/// RFC 4733 §3.2 telephone-event numbers for DTMF.
pub mod rfc4733_event {
    pub const ZERO: u8 = 0;
    pub const STAR: u8 = 10;
    pub const POUND: u8 = 11;
}

impl DtmfBearerEvent {
    /// Map a 4-bit BDTMFM / Continuous DTMF DIGIT (C.S0005-E
    /// Table 2.7.1.3.2.4-4) to an RFC 4733 event number.
    /// Reserved codes (0x0D-0x0F) return `None`, matching
    /// `validate_dtmf_digit` in cdma-common.
    pub fn event_from_cdma_digit(digit: u8) -> Option<u8> {
        use cdma_common::access::bdtmfm_digit;
        match digit & 0x0F {
            d @ 0x01..=0x09 => Some(d),
            bdtmfm_digit::ZERO => Some(rfc4733_event::ZERO),
            bdtmfm_digit::STAR => Some(rfc4733_event::STAR),
            bdtmfm_digit::POUND => Some(rfc4733_event::POUND),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BearerEvent {
    Voice(VoiceBearerFrame),
    Dtmf(DtmfBearerEvent),
}

/// Per-circuit RTP sequence number and timestamp tracker.
#[derive(Debug, Clone)]
pub struct RtpSendState {
    seq: u16,
    timestamp: u32,
}

impl RtpSendState {
    pub fn new() -> Self {
        Self {
            seq: 0,
            timestamp: 0,
        }
    }

    pub fn advance(&mut self) -> (u16, u32) {
        let result = (self.seq, self.timestamp);
        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(TIMESTAMP_INCREMENT);
        result
    }
}

impl Default for RtpSendState {
    fn default() -> Self {
        Self::new()
    }
}

/// A decoded A2p voice bearer frame.
///
/// On the wire this is an RTP packet with EVRC header-full payload (RFC 3558).
/// The `circuit_id` identifies which per-circuit RTP session this frame belongs
/// to. `rate_bps` maps to the RFC 3558 Table of Contents frame type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceBearerFrame {
    /// Circuit identity assigned in the A1 AssignmentRequest.
    pub circuit_id: u16,
    /// Vocoder frame rate in bits per second (9600, 4800, 2400, 1200, or 0).
    pub rate_bps: u32,
    /// Raw EVRC codec payload bytes (no MuxPDU header).
    pub payload: Vec<u8>,
}

impl VoiceBearerFrame {
    /// Encodes as an RTP packet with EVRC header-full payload (RFC 3558 §5.2).
    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode_rtp(&self, seq: u16, timestamp: u32, ssrc: u32) -> Vec<u8> {
        let ft = EvrcFrameType::from_rate_bps(self.rate_bps);
        let toc = ft as u8;

        let mut buf = Vec::with_capacity(RTP_HEADER_LEN + 1 + self.payload.len());

        // V=2, P=0, X=0, CC=0
        buf.push(0x80);
        // M=0, PT
        buf.push(EVRC_RTP_PAYLOAD_TYPE);
        buf.extend_from_slice(&seq.to_be_bytes());
        buf.extend_from_slice(&timestamp.to_be_bytes());
        buf.extend_from_slice(&ssrc.to_be_bytes());
        // RFC 3558 ToC entry (bits 7-4 reserved, bits 3-0 = frame type)
        buf.push(toc);
        buf.extend_from_slice(&self.payload);

        buf
    }

    /// Decodes an RTP/EVRC header-full datagram into a voice bearer frame.
    ///
    /// The `circuit_id` is set by the caller from the per-circuit session
    /// context (not from the RTP header).
    pub fn decode_rtp(buf: &[u8], circuit_id: u16) -> Option<Self> {
        if buf.len() < RTP_HEADER_LEN + 1 {
            return None;
        }
        if (buf[0] >> 6) != 2 {
            return None;
        }

        let toc = buf[RTP_HEADER_LEN];
        let ft = EvrcFrameType::from_u8(toc & 0x0F)?;
        let payload = buf[RTP_HEADER_LEN + 1..].to_vec();

        Some(Self {
            circuit_id,
            rate_bps: ft.to_rate_bps(),
            payload,
        })
    }
}

/// Per-circuit RTP payload types negotiated via the BearerFormatEntry IE
/// (A.S0014-D §4.2.90). `telephone_event = None` disables DTMF on this
/// circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerPayloadTypes {
    pub evrc: u8,
    pub telephone_event: Option<u8>,
}

impl Default for BearerPayloadTypes {
    fn default() -> Self {
        Self {
            evrc: EVRC_RTP_PAYLOAD_TYPE,
            telephone_event: None,
        }
    }
}

struct CircuitBearerSession {
    socket: std::sync::Arc<UdpSocket>,
    remote_addr: Option<SocketAddr>,
    send_state: RtpSendState,
    /// RFC 4733 §2.5.1.2 holds the RTP timestamp constant across every
    /// packet of one event; captured on the marker packet, reused thereafter.
    dtmf_event_timestamp: Option<u32>,
    /// Shared with the recv task so PT renegotiation is observable there.
    payload_types: std::sync::Arc<Mutex<BearerPayloadTypes>>,
    ssrc: u32,
}

fn encode_dtmf_payload(event_code: u8, volume: u8, duration_samples: u16, end: bool) -> [u8; 4] {
    let e_bit: u8 = if end { 0x80 } else { 0x00 };
    [
        event_code,
        e_bit | (volume & 0x3F),
        (duration_samples >> 8) as u8,
        duration_samples as u8,
    ]
}

fn decode_dtmf_payload(buf: &[u8]) -> Option<(u8, u8, u16, bool)> {
    if buf.len() < 4 {
        return None;
    }
    let end = (buf[1] & 0x80) != 0;
    let volume = buf[1] & 0x3F;
    let duration = u16::from_be_bytes([buf[2], buf[3]]);
    Some((buf[0], volume, duration, end))
}

fn encode_rtp_packet(
    payload_type: u8,
    marker: bool,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
    body: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(RTP_HEADER_LEN + body.len());
    buf.push(0x80); // V=2, P=0, X=0, CC=0
    let mb: u8 = if marker { 0x80 } else { 0x00 };
    buf.push(mb | (payload_type & 0x7F));
    buf.extend_from_slice(&seq.to_be_bytes());
    buf.extend_from_slice(&timestamp.to_be_bytes());
    buf.extend_from_slice(&ssrc.to_be_bytes());
    buf.extend_from_slice(body);
    buf
}

/// A2p voice bearer manager with per-circuit RTP sessions.
///
/// Each circuit gets its own UDP socket. Bearer parameters (IP address and UDP
/// port) are exchanged via the A2p Bearer Session-Level Parameters IE in
/// AssignmentRequest/AssignmentComplete.
pub struct VoiceBearerManager {
    /// Bind IP address for new circuit sockets (typically 0.0.0.0 or the
    /// node's interface address).
    bind_ip: Ipv4Addr,
    circuits: Mutex<HashMap<u16, CircuitBearerSession>>,
    frame_tx: mpsc::Sender<BearerEvent>,
    frame_rx: tokio::sync::Mutex<mpsc::Receiver<BearerEvent>>,
}

impl VoiceBearerManager {
    /// Creates a new bearer manager that binds circuit sockets on `bind_ip`.
    pub fn new(bind_ip: Ipv4Addr) -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            bind_ip,
            circuits: Mutex::new(HashMap::new()),
            frame_tx: tx,
            frame_rx: tokio::sync::Mutex::new(rx),
        }
    }

    /// Opens a new bearer circuit, binding a UDP socket on an ephemeral port.
    ///
    /// Returns the local `SocketAddr` for inclusion in the A2p Bearer
    /// Session-Level Parameters IE. If a remote address is known (e.g. from
    /// an incoming AssignmentRequest), pass it here; otherwise call
    /// `set_circuit_remote()` later when the peer's AssignmentComplete arrives.
    pub async fn open_circuit(
        &self,
        circuit_id: u16,
        remote_addr: Option<SocketAddr>,
    ) -> io::Result<SocketAddr> {
        let socket = UdpSocket::bind(SocketAddr::new(self.bind_ip.into(), 0)).await?;
        let local_addr = socket.local_addr()?;
        let socket = std::sync::Arc::new(socket);
        let payload_types = std::sync::Arc::new(Mutex::new(BearerPayloadTypes::default()));

        let session = CircuitBearerSession {
            socket: socket.clone(),
            remote_addr,
            send_state: RtpSendState::new(),
            dtmf_event_timestamp: None,
            payload_types: payload_types.clone(),
            ssrc: rand_ssrc(),
        };

        {
            let mut circuits = self.circuits.lock().unwrap();
            circuits.insert(circuit_id, session);
        }

        let tx = self.frame_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((len, _src)) = socket.recv_from(&mut buf).await {
                let datagram = &buf[..len];
                if datagram.len() < RTP_HEADER_LEN + 1 || (datagram[0] >> 6) != 2 {
                    continue;
                }
                let marker = (datagram[1] & 0x80) != 0;
                let pt = datagram[1] & 0x7F;
                let pts = *payload_types.lock().unwrap();
                let event = if Some(pt) == pts.telephone_event {
                    decode_dtmf_payload(&datagram[RTP_HEADER_LEN..]).map(
                        |(event_code, volume, duration_samples, end)| {
                            BearerEvent::Dtmf(DtmfBearerEvent {
                                circuit_id,
                                event: event_code,
                                volume,
                                duration_samples,
                                end,
                                start_of_event: marker,
                            })
                        },
                    )
                } else if pt == pts.evrc {
                    VoiceBearerFrame::decode_rtp(datagram, circuit_id).map(BearerEvent::Voice)
                } else {
                    None
                };
                if let Some(event) = event
                    && tx.send(event).await.is_err()
                {
                    break;
                }
            }
        });

        Ok(local_addr)
    }

    /// Records the per-circuit PTs negotiated via the BearerFormatEntry IE.
    /// Until this is called, DTMF events are dropped at recv time and the
    /// `send_dtmf_event` path returns `NotFound`.
    pub fn set_circuit_payload_types(&self, circuit_id: u16, pts: BearerPayloadTypes) {
        let circuits = self.circuits.lock().unwrap();
        if let Some(session) = circuits.get(&circuit_id) {
            *session.payload_types.lock().unwrap() = pts;
        }
    }

    /// Sets or updates the remote address for an existing circuit.
    ///
    /// Called when the peer's bearer parameters arrive (e.g. in
    /// AssignmentComplete from BSC, or AssignmentRequest from MSC).
    pub fn set_circuit_remote(&self, circuit_id: u16, remote_addr: SocketAddr) {
        let mut circuits = self.circuits.lock().unwrap();
        if let Some(session) = circuits.get_mut(&circuit_id) {
            session.remote_addr = Some(remote_addr);
        }
    }

    /// Closes a bearer circuit, dropping its socket.
    pub fn close_circuit(&self, circuit_id: u16) {
        let mut circuits = self.circuits.lock().unwrap();
        circuits.remove(&circuit_id);
    }

    /// Returns a voice frame (EVRC) or a DTMF event (telephone-event), each
    /// matched against the per-circuit negotiated PT.
    pub async fn recv(&self) -> Option<BearerEvent> {
        let mut rx = self.frame_rx.lock().await;
        rx.recv().await
    }

    pub async fn send_frame(&self, frame: &VoiceBearerFrame) -> io::Result<()> {
        let (socket, remote, encoded) = {
            let mut circuits = self.circuits.lock().unwrap();
            let session = circuits.get_mut(&frame.circuit_id).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no bearer circuit for circuit_id={}", frame.circuit_id),
                )
            })?;
            let remote = session.remote_addr.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotConnected,
                    format!("no remote addr for bearer circuit_id={}", frame.circuit_id),
                )
            })?;
            let (seq, ts) = session.send_state.advance();
            let encoded = frame.encode_rtp(seq, ts, session.ssrc);
            (session.socket.clone(), remote, encoded)
        };
        socket.send_to(&encoded, remote).await?;
        Ok(())
    }

    /// Build an RFC 4733 packet on `circuit_id` using the negotiated
    /// telephone-event PT. `start_of_event` selects the RTP marker bit and
    /// captures the event timestamp per RFC 4733 §2.5.1.2.
    fn prepare_dtmf_packet(
        &self,
        circuit_id: u16,
        event_code: u8,
        volume: u8,
        duration_samples: u16,
        end: bool,
        start_of_event: bool,
    ) -> io::Result<(std::sync::Arc<UdpSocket>, SocketAddr, Vec<u8>)> {
        let mut circuits = self.circuits.lock().unwrap();
        let session = circuits.get_mut(&circuit_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no bearer circuit for circuit_id={}", circuit_id),
            )
        })?;
        let remote = session.remote_addr.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                format!("no remote addr for bearer circuit_id={}", circuit_id),
            )
        })?;
        let pt = session
            .payload_types
            .lock()
            .unwrap()
            .telephone_event
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "telephone-event PT not negotiated on circuit_id={}",
                        circuit_id
                    ),
                )
            })?;
        if start_of_event || session.dtmf_event_timestamp.is_none() {
            session.dtmf_event_timestamp = Some(session.send_state.timestamp);
        }
        let timestamp = session.dtmf_event_timestamp.unwrap();
        let seq = session.send_state.seq;
        session.send_state.seq = session.send_state.seq.wrapping_add(1);
        let body = encode_dtmf_payload(event_code, volume, duration_samples, end);
        let packet = encode_rtp_packet(pt, start_of_event, seq, timestamp, session.ssrc, &body);
        Ok((session.socket.clone(), remote, packet))
    }

    pub async fn send_dtmf_event(
        &self,
        circuit_id: u16,
        event_code: u8,
        volume: u8,
        duration_samples: u16,
        end: bool,
        start_of_event: bool,
    ) -> io::Result<()> {
        let (socket, remote, packet) = self.prepare_dtmf_packet(
            circuit_id,
            event_code,
            volume,
            duration_samples,
            end,
            start_of_event,
        )?;
        socket.send_to(&packet, remote).await?;
        Ok(())
    }

    pub fn try_send_dtmf_event(
        &self,
        circuit_id: u16,
        event_code: u8,
        volume: u8,
        duration_samples: u16,
        end: bool,
        start_of_event: bool,
    ) -> io::Result<()> {
        let (socket, remote, packet) = self.prepare_dtmf_packet(
            circuit_id,
            event_code,
            volume,
            duration_samples,
            end,
            start_of_event,
        )?;
        socket.try_send_to(&packet, remote)?;
        Ok(())
    }

    pub fn try_send_frame(&self, frame: &VoiceBearerFrame) -> io::Result<()> {
        let mut circuits = self.circuits.lock().unwrap();
        let session = circuits.get_mut(&frame.circuit_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no bearer circuit for circuit_id={}", frame.circuit_id),
            )
        })?;
        let remote = session.remote_addr.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                format!("no remote addr for bearer circuit_id={}", frame.circuit_id),
            )
        })?;
        let (seq, ts) = session.send_state.advance();
        let encoded = frame.encode_rtp(seq, ts, session.ssrc);
        session.socket.try_send_to(&encoded, remote)?;
        Ok(())
    }
}

fn rand_ssrc() -> u32 {
    use rand::RngCore;
    rand::thread_rng().next_u32()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_frame_roundtrip() {
        let frame = VoiceBearerFrame {
            circuit_id: 42,
            rate_bps: 9600,
            payload: vec![0xAA; 22],
        };
        let encoded = frame.encode_rtp(100, 16000, 0x12345678);
        let decoded = VoiceBearerFrame::decode_rtp(&encoded, 42).unwrap();
        assert_eq!(frame.circuit_id, decoded.circuit_id);
        assert_eq!(frame.rate_bps, decoded.rate_bps);
        assert_eq!(frame.payload, decoded.payload);
    }

    #[test]
    fn rtp_header_structure() {
        let frame = VoiceBearerFrame {
            circuit_id: 7,
            rate_bps: 4800,
            payload: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        };
        let encoded = frame.encode_rtp(0x1234, 0x56789ABC, 0xDEADBEEF);

        // V=2, P=0, X=0, CC=0
        assert_eq!(encoded[0], 0x80);
        // M=0, PT=96
        assert_eq!(encoded[1], EVRC_RTP_PAYLOAD_TYPE);
        // Sequence number
        assert_eq!(&encoded[2..4], &[0x12, 0x34]);
        // Timestamp
        assert_eq!(&encoded[4..8], &[0x56, 0x78, 0x9A, 0xBC]);
        // SSRC
        assert_eq!(&encoded[8..12], &[0xDE, 0xAD, 0xBE, 0xEF]);
        // ToC: Half rate = frame type 3
        assert_eq!(encoded[12], 3);
        // Payload
        assert_eq!(&encoded[13..], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn all_evrc_rates() {
        for (rate_bps, expected_ft) in [(9600u32, 4u8), (4800, 3), (2400, 2), (1200, 1), (0, 0)] {
            let frame = VoiceBearerFrame {
                circuit_id: 1,
                rate_bps,
                payload: vec![],
            };
            let encoded = frame.encode_rtp(0, 0, 0);
            assert_eq!(encoded[12] & 0x0F, expected_ft, "rate_bps={}", rate_bps);

            let decoded = VoiceBearerFrame::decode_rtp(&encoded, 1).unwrap();
            assert_eq!(decoded.rate_bps, rate_bps);
        }
    }

    #[test]
    fn decode_rejects_non_rtp() {
        assert!(VoiceBearerFrame::decode_rtp(&[0, 1, 2], 1).is_none());
        // Wrong version (V=1)
        assert!(VoiceBearerFrame::decode_rtp(&[0x40; 14], 1).is_none());
    }

    #[test]
    fn decode_rejects_invalid_frame_type() {
        let mut pkt = vec![0x80, EVRC_RTP_PAYLOAD_TYPE];
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes());
        pkt.push(15); // invalid frame type
        assert!(VoiceBearerFrame::decode_rtp(&pkt, 1).is_none());
    }

    #[test]
    fn rtp_send_state_advances() {
        let mut state = RtpSendState::new();
        let (seq0, ts0) = state.advance();
        assert_eq!((seq0, ts0), (0, 0));
        let (seq1, ts1) = state.advance();
        assert_eq!((seq1, ts1), (1, 160));
        let (seq2, ts2) = state.advance();
        assert_eq!((seq2, ts2), (2, 320));
    }

    #[test]
    fn rtp_send_state_wraps() {
        let mut state = RtpSendState {
            seq: u16::MAX,
            timestamp: u32::MAX - 100,
        };
        let (seq, ts) = state.advance();
        assert_eq!(seq, u16::MAX);
        assert_eq!(ts, u32::MAX - 100);
        let (seq2, ts2) = state.advance();
        assert_eq!(seq2, 0);
        assert_eq!(ts2, (u32::MAX - 100).wrapping_add(160));
    }

    #[tokio::test]
    async fn manager_per_circuit_send_recv() {
        let mgr_a = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);
        let mgr_b = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);

        let circuit_id: u16 = 7;

        let a_local = mgr_a.open_circuit(circuit_id, None).await.unwrap();
        let b_local = mgr_b.open_circuit(circuit_id, Some(a_local)).await.unwrap();
        mgr_a.set_circuit_remote(circuit_id, b_local);

        let frame = VoiceBearerFrame {
            circuit_id,
            rate_bps: 4800,
            payload: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        };

        mgr_a.send_frame(&frame).await.unwrap();
        let received = match mgr_b.recv().await.unwrap() {
            BearerEvent::Voice(f) => f,
            BearerEvent::Dtmf(_) => panic!("expected voice frame"),
        };
        assert_eq!(received.circuit_id, frame.circuit_id);
        assert_eq!(received.rate_bps, frame.rate_bps);
        assert_eq!(received.payload, frame.payload);
    }

    #[tokio::test]
    async fn manager_multiple_circuits() {
        let mgr_a = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);
        let mgr_b = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);

        let cid1: u16 = 1;
        let cid2: u16 = 2;

        let a1_local = mgr_a.open_circuit(cid1, None).await.unwrap();
        let b1_local = mgr_b.open_circuit(cid1, Some(a1_local)).await.unwrap();
        mgr_a.set_circuit_remote(cid1, b1_local);

        let a2_local = mgr_a.open_circuit(cid2, None).await.unwrap();
        let b2_local = mgr_b.open_circuit(cid2, Some(a2_local)).await.unwrap();
        mgr_a.set_circuit_remote(cid2, b2_local);

        let frame1 = VoiceBearerFrame {
            circuit_id: cid1,
            rate_bps: 9600,
            payload: vec![0xAA; 22],
        };
        let frame2 = VoiceBearerFrame {
            circuit_id: cid2,
            rate_bps: 1200,
            payload: vec![0xBB; 2],
        };

        mgr_a.send_frame(&frame1).await.unwrap();
        mgr_a.send_frame(&frame2).await.unwrap();

        let mut received: Vec<VoiceBearerFrame> = Vec::new();
        for _ in 0..2 {
            match mgr_b.recv().await.unwrap() {
                BearerEvent::Voice(f) => received.push(f),
                BearerEvent::Dtmf(_) => panic!("expected voice frame"),
            }
        }
        received.sort_by_key(|f| f.circuit_id);

        assert_eq!(received[0].circuit_id, cid1);
        assert_eq!(received[0].rate_bps, 9600);
        assert_eq!(received[1].circuit_id, cid2);
        assert_eq!(received[1].rate_bps, 1200);
    }

    #[tokio::test]
    async fn manager_send_frame_async() {
        let mgr_a = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);
        let mgr_b = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);

        let circuit_id: u16 = 42;

        let a_local = mgr_a.open_circuit(circuit_id, None).await.unwrap();
        let b_local = mgr_b.open_circuit(circuit_id, Some(a_local)).await.unwrap();
        mgr_a.set_circuit_remote(circuit_id, b_local);

        let frame = VoiceBearerFrame {
            circuit_id,
            rate_bps: 9600,
            payload: vec![0xCC; 22],
        };

        mgr_a.send_frame(&frame).await.unwrap();
        let received = match mgr_b.recv().await.unwrap() {
            BearerEvent::Voice(f) => f,
            BearerEvent::Dtmf(_) => panic!("expected voice frame"),
        };
        assert_eq!(received.circuit_id, frame.circuit_id);
        assert_eq!(received.rate_bps, frame.rate_bps);
        assert_eq!(received.payload, frame.payload);
    }

    #[test]
    fn dtmf_payload_roundtrip() {
        let body = encode_dtmf_payload(5, 10, 800, false);
        let (event_code, volume, duration, end) = decode_dtmf_payload(&body).unwrap();
        assert_eq!(event_code, 5);
        assert_eq!(volume, 10);
        assert_eq!(duration, 800);
        assert!(!end);

        // End-of-event sets the E bit.
        let body = encode_dtmf_payload(0, 5, 1600, true);
        let (_e, _v, _d, end) = decode_dtmf_payload(&body).unwrap();
        assert!(end);
    }

    #[test]
    fn dtmf_event_from_cdma_digit_maps_per_table() {
        // C.S0005-E Table 2.7.1.3.2.4-4
        assert_eq!(DtmfBearerEvent::event_from_cdma_digit(0x01), Some(1));
        assert_eq!(DtmfBearerEvent::event_from_cdma_digit(0x09), Some(9));
        assert_eq!(DtmfBearerEvent::event_from_cdma_digit(0x0A), Some(0));
        assert_eq!(DtmfBearerEvent::event_from_cdma_digit(0x0B), Some(10)); // '*'
        assert_eq!(DtmfBearerEvent::event_from_cdma_digit(0x0C), Some(11)); // '#'
        assert_eq!(DtmfBearerEvent::event_from_cdma_digit(0x00), None);
    }

    #[tokio::test]
    async fn manager_sends_and_receives_dtmf_event() {
        let mgr_a = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);
        let mgr_b = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);

        let circuit_id: u16 = 9;

        let a_local = mgr_a.open_circuit(circuit_id, None).await.unwrap();
        let b_local = mgr_b.open_circuit(circuit_id, Some(a_local)).await.unwrap();
        mgr_a.set_circuit_remote(circuit_id, b_local);
        let pts = BearerPayloadTypes {
            evrc: EVRC_RTP_PAYLOAD_TYPE,
            telephone_event: Some(TELEPHONE_EVENT_RTP_PAYLOAD_TYPE),
        };
        mgr_a.set_circuit_payload_types(circuit_id, pts);
        mgr_b.set_circuit_payload_types(circuit_id, pts);

        mgr_a
            .send_dtmf_event(circuit_id, 5, 10, 160, false, true)
            .await
            .unwrap();
        match mgr_b.recv().await.unwrap() {
            BearerEvent::Dtmf(ev) => {
                assert_eq!(ev.circuit_id, circuit_id);
                assert_eq!(ev.event, 5);
                assert_eq!(ev.duration_samples, 160);
                assert!(!ev.end);
            }
            BearerEvent::Voice(_) => panic!("expected dtmf event"),
        }

        // End-of-event packet.
        mgr_a
            .send_dtmf_event(circuit_id, 5, 10, 1600, true, false)
            .await
            .unwrap();
        match mgr_b.recv().await.unwrap() {
            BearerEvent::Dtmf(ev) => {
                assert!(ev.end);
                assert_eq!(ev.duration_samples, 1600);
            }
            BearerEvent::Voice(_) => panic!("expected dtmf event"),
        }
    }

    #[tokio::test]
    async fn manager_close_circuit() {
        let mgr = VoiceBearerManager::new(Ipv4Addr::LOCALHOST);
        let circuit_id: u16 = 10;
        mgr.open_circuit(circuit_id, None).await.unwrap();
        mgr.close_circuit(circuit_id);

        let frame = VoiceBearerFrame {
            circuit_id,
            rate_bps: 9600,
            payload: vec![],
        };
        assert!(mgr.try_send_frame(&frame).is_err());
    }
}
