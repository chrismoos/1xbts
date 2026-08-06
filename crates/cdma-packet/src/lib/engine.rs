use std::collections::VecDeque;
/// Packet data session engine — wires RLP + PPP into a single pipeline.
///
/// Pipeline:
///   Uplink:   RLP frames from mobile → RLP decode → byte stream → PPP deframe → IP packets
///   Downlink: IP packets → PPP frame → byte stream → RLP encode → RLP frames to mobile
///
/// Manages the full lifecycle:
///   1. RLP SYNC handshake
///   2. LCP negotiation
///   3. IPCP negotiation (IP assignment)
///   4. IP forwarding
use std::net::Ipv4Addr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::capture::{self, Direction as CaptureDirection};
use crate::mobile_ip::{MobileIpPacketResult, MobileIpSession};
use crate::ppp::framing::{self, HdlcDeframer, PppPacket};
use crate::ppp::ipcp::{IPCP_PROTOCOL, IpcpConfig, IpcpOpenState, IpcpSession};
use crate::ppp::lcp::{LCP_PROTOCOL, LcpOpenState, LcpSession, LcpState, TERMINATE_REQUEST};
use crate::ppp::vj::{PPP_IP_PROTOCOL, PPP_VJ_COMPRESSED_TCP, PPP_VJ_UNCOMPRESSED_TCP, VjState};
use crate::rlp::{self as rlp_codec, RlpFrame};
use crate::rlp_session::{RlpOutput, RlpSession, RlpState};
use crate::rlp3_frames::MuxOption;
use crate::rlp3_session::{FrameRate, Rlp3Config, Rlp3Session, Rlp3State, RlpEvent};
use cdma_common::consts::SERVICE_OPTION_HIGH_RATE_PACKET_DATA;
use cdma_common::crc::crc16_sch;
use cdma_common::hrpd::traffic::{
    DEFAULT_PACKET_RLP_SEQUENCE_BITS, default_packet_rlp_packet_bits,
};
use cdma_common::sch::{DEFAULT_RC3_F_SCH_RATE_BPS, Rc3FschProfile};

/// Session phase — tracks the overall negotiation progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// RLP SYNC handshake in progress.
    RlpSync,
    /// RLP is up, LCP negotiation in progress.
    Lcp,
    /// LCP is open, IPCP negotiation in progress.
    Ipcp,
    /// IPCP is open, waiting for Mobile IPv4 registration.
    Mip4Pending,
    /// IPCP is open — forwarding IP packets.
    Active,
    /// Session terminated.
    Closed,
}

/// An action the caller must perform after a tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAction {
    /// Send this frame (raw bits + rate) on the downlink FCH traffic channel.
    SendFrame { bits: Vec<u8>, rate_bps: u32 },
    /// Send this frame on the downlink SCH supplemental channel.
    SendSchFrame { bits: Vec<u8>, rate_bps: u32 },
    /// An IP packet was received from the mobile and is ready for the TUN/network.
    DeliverIpPacket(Vec<u8>),
    /// The packet session should be closed and cleaned up by the owner.
    CloseSession { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PacketTraceEvent {
    pub timestamp_ms: u64,
    pub layer: String,
    pub direction: String,
    pub summary: String,
    pub detail: String,
    pub payload_hex: String,
}

#[derive(Debug, Clone, Default)]
pub struct PacketSessionTelemetry {
    pub rlp_state: String,
    pub lcp_state: String,
    pub ipcp_state: String,
    pub lcp_configure_restarts: u32,
    pub ipcp_configure_restarts: u32,
    pub ipcp_omitted_peer_ip_naks: u32,
    pub last_rx_control: String,
    pub last_tx_control: String,
    pub last_rx_control_repeats: u64,
    pub last_tx_control_repeats: u64,
    pub last_uplink_rate_bps: u32,
    pub last_downlink_rate_bps: u32,
    pub recent_ppp_events: Vec<PacketTraceEvent>,
}

#[derive(Debug, Clone, Default)]
struct RlpTelemetry {
    state: String,
    last_rx_control: String,
    last_tx_control: String,
    last_rx_control_repeats: u64,
    last_tx_control_repeats: u64,
    last_uplink_rate_bps: u32,
    last_downlink_rate_bps: u32,
}

// ---------------------------------------------------------------------------
// RLP backend trait — abstracts over RLP Type 1 (SO 7) and Type 3 (SO 33).
// ---------------------------------------------------------------------------

trait RlpBackend: Send {
    /// Process an uplink frame (raw primary traffic bits + rate).
    /// Returns bytes delivered to the upper layer (PPP), if any.
    fn receive_frame_bits(&mut self, bits: &[u8], rate_bps: u32) -> Option<Vec<u8>>;

    /// Feed data from the upper layer (PPP) for downlink transmission.
    fn enqueue_data(&mut self, data: &[u8]);

    /// Generate the next downlink frame as (bits, rate_bps) for the FCH.
    ///
    /// When `allow_payload` is false, only RLP control/fill/idle should be
    /// emitted; byte-stream payload remains queued for SCH.
    fn next_frame_bits(&mut self, allow_payload: bool) -> (Vec<u8>, u32);

    /// Generate a downlink frame only when a complete bearer chunk is ready.
    /// Frame-timed RLP backends do not distinguish partial bearer chunks.
    fn next_full_frame_bits(&mut self, allow_payload: bool) -> (Vec<u8>, u32) {
        self.next_frame_bits(allow_payload)
    }

    /// Maximum number of downlink frames that may be emitted from one
    /// `PacketSession::tick`. Legacy 1x RLP paths are frame-timed; HRPD A10
    /// Unstructured Byte Stream is a bearer octet stream and can burst queued
    /// chunks without applying a 1x air-interface timing rule.
    fn max_downlink_frames_per_tick(&self) -> usize {
        1
    }

    /// Generate a supplemental channel frame with `info_bits` usable bits.
    /// Returns (bits, rate_bps) or None if SCH is unavailable.
    /// Default: no SCH support.
    fn next_sch_frame_bits(&mut self, _info_bits: usize, _rate_bps: u32) -> Option<(Vec<u8>, u32)> {
        None
    }

    /// Returns true if the RLP link is established (data transfer state).
    fn is_data_transfer(&self) -> bool;

    /// Attach an optional log context string such as a session id.
    fn set_log_context(&mut self, _context: String) {}

    /// Returns a snapshot of backend telemetry for UI/diagnostics.
    fn telemetry(&self) -> RlpTelemetry {
        RlpTelemetry::default()
    }

    /// Returns the number of queued upper-layer bytes awaiting transmission.
    fn tx_queue_len(&self) -> usize {
        0
    }
}

// ---------------------------------------------------------------------------
// HRPD A8/A10 Unstructured Byte Stream backend.
// ---------------------------------------------------------------------------

const A10_UNSTRUCTURED_BYTE_STREAM_MAX_OCTETS: usize = 1024;
const A10_UNSTRUCTURED_BYTE_STREAM_MAX_CHUNKS_PER_TICK: usize = 16;

struct UnstructuredByteStreamBackend {
    tx_queue: VecDeque<u8>,
    last_rx_control: String,
    last_tx_control: String,
}

impl UnstructuredByteStreamBackend {
    fn new() -> Self {
        Self {
            tx_queue: VecDeque::new(),
            last_rx_control: String::new(),
            last_tx_control: String::new(),
        }
    }
}

impl RlpBackend for UnstructuredByteStreamBackend {
    fn receive_frame_bits(&mut self, octets: &[u8], _rate_bps: u32) -> Option<Vec<u8>> {
        if octets.is_empty() {
            return None;
        }
        self.last_rx_control = format!("A10 UnstructuredByteStream octets={}", octets.len());
        Some(octets.to_vec())
    }

    fn enqueue_data(&mut self, data: &[u8]) {
        self.tx_queue.extend(data.iter().copied());
    }

    fn next_frame_bits(&mut self, allow_payload: bool) -> (Vec<u8>, u32) {
        if !allow_payload || self.tx_queue.is_empty() {
            return (Vec::new(), 0);
        }
        let take = self
            .tx_queue
            .len()
            .min(A10_UNSTRUCTURED_BYTE_STREAM_MAX_OCTETS);
        let mut octets = Vec::with_capacity(take);
        for _ in 0..take {
            if let Some(byte) = self.tx_queue.pop_front() {
                octets.push(byte);
            }
        }
        self.last_tx_control = format!("A10 UnstructuredByteStream octets={take}");
        (octets, 0)
    }

    fn next_full_frame_bits(&mut self, allow_payload: bool) -> (Vec<u8>, u32) {
        if !allow_payload || self.tx_queue.len() < A10_UNSTRUCTURED_BYTE_STREAM_MAX_OCTETS {
            return (Vec::new(), 0);
        }
        self.next_frame_bits(allow_payload)
    }

    fn max_downlink_frames_per_tick(&self) -> usize {
        A10_UNSTRUCTURED_BYTE_STREAM_MAX_CHUNKS_PER_TICK
    }

    fn is_data_transfer(&self) -> bool {
        true
    }

    fn telemetry(&self) -> RlpTelemetry {
        RlpTelemetry {
            state: "A10 Unstructured Byte Stream".to_string(),
            last_rx_control: self.last_rx_control.clone(),
            last_tx_control: self.last_tx_control.clone(),
            last_rx_control_repeats: u64::from(!self.last_rx_control.is_empty()),
            last_tx_control_repeats: u64::from(!self.last_tx_control.is_empty()),
            last_uplink_rate_bps: 0,
            last_downlink_rate_bps: 0,
        }
    }

    fn tx_queue_len(&self) -> usize {
        self.tx_queue.len()
    }
}

// ---------------------------------------------------------------------------
// HRPD Default Packet Application RLP backend.
//
// This is the AN/BTS air-side wrapper used between Stream 1 and the HRPD
// Physical Layer. It is not the A8/A10 bearer format; A8/A10 use the
// `UnstructuredByteStreamBackend` above.
// ---------------------------------------------------------------------------

const HRPD_DEFAULT_PACKET_MAX_OCTETS_1024_FTC: usize = 121;

struct HrpdDefaultPacketBackend {
    tx_queue: VecDeque<u8>,
    tx_seq: u32,
    rx_last_seq: Option<u32>,
    last_rx_control: String,
    last_tx_control: String,
}

impl HrpdDefaultPacketBackend {
    fn new() -> Self {
        Self {
            tx_queue: VecDeque::new(),
            tx_seq: 0,
            rx_last_seq: None,
            last_rx_control: String::new(),
            last_tx_control: String::new(),
        }
    }
}

impl RlpBackend for HrpdDefaultPacketBackend {
    fn receive_frame_bits(&mut self, bits: &[u8], _rate_bps: u32) -> Option<Vec<u8>> {
        if bits.len() < DEFAULT_PACKET_RLP_SEQUENCE_BITS
            || (bits.len() - DEFAULT_PACKET_RLP_SEQUENCE_BITS) % 8 != 0
        {
            return None;
        }
        let seq = bits[..DEFAULT_PACKET_RLP_SEQUENCE_BITS]
            .iter()
            .fold(0u32, |acc, &bit| (acc << 1) | u32::from(bit & 1));
        self.rx_last_seq = Some(seq);
        self.last_rx_control = format!("HRPD DefaultPacket seq={seq} bits={}", bits.len());
        let mut octets = Vec::with_capacity((bits.len() - DEFAULT_PACKET_RLP_SEQUENCE_BITS) / 8);
        for chunk in bits[DEFAULT_PACKET_RLP_SEQUENCE_BITS..].chunks_exact(8) {
            octets.push(chunk.iter().fold(0u8, |acc, &bit| (acc << 1) | (bit & 1)));
        }
        if octets.is_empty() {
            None
        } else {
            Some(octets)
        }
    }

    fn enqueue_data(&mut self, data: &[u8]) {
        self.tx_queue.extend(data.iter().copied());
    }

    fn next_frame_bits(&mut self, allow_payload: bool) -> (Vec<u8>, u32) {
        if !allow_payload || self.tx_queue.is_empty() {
            return (Vec::new(), 0);
        }
        let take = self
            .tx_queue
            .len()
            .min(HRPD_DEFAULT_PACKET_MAX_OCTETS_1024_FTC);
        let mut octets = Vec::with_capacity(take);
        for _ in 0..take {
            if let Some(byte) = self.tx_queue.pop_front() {
                octets.push(byte);
            }
        }
        let bits = default_packet_rlp_packet_bits(self.tx_seq, &octets);
        self.last_tx_control = format!("HRPD DefaultPacket seq={} octets={}", self.tx_seq, take);
        self.tx_seq = (self.tx_seq + 1) & ((1 << DEFAULT_PACKET_RLP_SEQUENCE_BITS) - 1);
        (bits, 0)
    }

    fn is_data_transfer(&self) -> bool {
        true
    }

    fn telemetry(&self) -> RlpTelemetry {
        RlpTelemetry {
            state: "HRPD Default Packet".to_string(),
            last_rx_control: self.last_rx_control.clone(),
            last_tx_control: self.last_tx_control.clone(),
            last_rx_control_repeats: u64::from(!self.last_rx_control.is_empty()),
            last_tx_control_repeats: u64::from(!self.last_tx_control.is_empty()),
            last_uplink_rate_bps: 0,
            last_downlink_rate_bps: 0,
        }
    }

    fn tx_queue_len(&self) -> usize {
        self.tx_queue.len()
    }
}

// ---------------------------------------------------------------------------
// RLP Type 1 backend (SO 7)
// ---------------------------------------------------------------------------

struct Rlp1Backend {
    session: RlpSession,
    last_rx_control: String,
    last_tx_control: String,
    last_uplink_rate_bps: u32,
    last_downlink_rate_bps: u32,
}

impl Rlp1Backend {
    fn new() -> Self {
        Self {
            session: RlpSession::new(),
            last_rx_control: String::new(),
            last_tx_control: String::new(),
            last_uplink_rate_bps: 0,
            last_downlink_rate_bps: 0,
        }
    }
}

impl RlpBackend for Rlp1Backend {
    fn receive_frame_bits(&mut self, bits: &[u8], rate_bps: u32) -> Option<Vec<u8>> {
        if rate_bps > 0 {
            self.last_uplink_rate_bps = rate_bps;
        }
        let rlp_rate = match rate_bps {
            9600 => Some(crate::rlp::RlpRate::Full),
            4800 => Some(crate::rlp::RlpRate::Half),
            1200 => Some(crate::rlp::RlpRate::Eighth),
            _ => None,
        };
        let decoded = rlp_rate.and_then(|rate| rlp_codec::decode_frame(bits, rate));
        if let Some(ref frame) = decoded {
            if matches!(frame, RlpFrame::Control { .. }) {
                self.last_rx_control = format_rlp_frame(frame);
            }
            if !frame.is_idle() && !is_rlp_handshake(frame) {
                log::debug!("RLP1 RX: {}", format_rlp_frame(frame));
            }
        }
        let delivery = self.session.receive_frame(decoded.as_ref());
        delivery.map(|d| d.data)
    }

    fn enqueue_data(&mut self, data: &[u8]) {
        self.session.enqueue_data(data);
    }

    fn next_frame_bits(&mut self, _allow_payload: bool) -> (Vec<u8>, u32) {
        match self.session.next_frame() {
            RlpOutput::SendFrame(frame) => {
                if !frame.is_idle() && !is_rlp_handshake(&frame) {
                    log::debug!("RLP1 TX: {}", format_rlp_frame(&frame));
                }
                let rate_bps = if frame.is_idle() { 1200u32 } else { 9600 };
                self.last_downlink_rate_bps = rate_bps;
                if matches!(frame, RlpFrame::Control { .. }) {
                    self.last_tx_control = format_rlp_frame(&frame);
                }
                let rlp_rate = if frame.is_idle() {
                    crate::rlp::RlpRate::Eighth
                } else {
                    crate::rlp::RlpRate::Full
                };
                let bits = rlp_codec::encode_frame(&frame, rlp_rate)
                    .expect("RLP1 session produced a frame invalid for selected rate");
                (bits, rate_bps)
            }
            RlpOutput::Nothing => {
                // Shouldn't happen — RLP always produces a frame. Fallback to idle.
                let idle = crate::rlp::idle_frame(0);
                let bits = rlp_codec::encode_frame(&idle, crate::rlp::RlpRate::Eighth)
                    .expect("RLP1 idle frame must encode at eighth rate");
                (bits, 1200)
            }
        }
    }

    fn is_data_transfer(&self) -> bool {
        self.session.state() == RlpState::DataTransfer
    }

    fn telemetry(&self) -> RlpTelemetry {
        RlpTelemetry {
            state: format_rlp1_state(self.session.state()),
            last_rx_control: self.last_rx_control.clone(),
            last_tx_control: self.last_tx_control.clone(),
            last_rx_control_repeats: u64::from(!self.last_rx_control.is_empty()),
            last_tx_control_repeats: u64::from(!self.last_tx_control.is_empty()),
            last_uplink_rate_bps: self.last_uplink_rate_bps,
            last_downlink_rate_bps: self.last_downlink_rate_bps,
        }
    }

    fn tx_queue_len(&self) -> usize {
        self.session.tx_queue_len()
    }
}

// ---------------------------------------------------------------------------
// RLP Type 3 backend (SO 33)
// ---------------------------------------------------------------------------

struct Rlp3Backend {
    session: Rlp3Session,
    log_context: Option<String>,
    tx_control_log: RepeatedControlLog,
    rx_control_log: RepeatedControlLog,
    rx_frame_count: u64,
    rx_decoded_count: u64,
    rx_decode_error_count: u64,
    tx_frame_count: u64,
    tx_sch_sdu_count: u64,
    tx_sch_blocked_count: u64,
    last_uplink_rate_bps: u32,
    last_downlink_rate_bps: u32,
}

impl Rlp3Backend {
    fn new(config: Rlp3Config) -> Self {
        Self {
            session: Rlp3Session::new(config),
            log_context: None,
            tx_control_log: RepeatedControlLog::default(),
            rx_control_log: RepeatedControlLog::default(),
            rx_frame_count: 0,
            rx_decoded_count: 0,
            rx_decode_error_count: 0,
            tx_frame_count: 0,
            tx_sch_sdu_count: 0,
            tx_sch_blocked_count: 0,
            last_uplink_rate_bps: 0,
            last_downlink_rate_bps: 0,
        }
    }

    fn log_control_frame(
        log_context: Option<&str>,
        direction: &str,
        state: &mut RepeatedControlLog,
        frame: &crate::rlp3_frames::Rlp3Frame,
    ) {
        let signature = format_rlp3_frame(frame);
        let prefix = if let Some(context) = log_context {
            format!("RLP3 {}[{}]", direction, context)
        } else {
            format!("RLP3 {}", direction)
        };

        if state.signature.as_deref() == Some(signature.as_str()) {
            state.repeat_count += 1;
            if state.repeat_count <= 3 {
                log::debug!("{}: {} (repeat #{})", prefix, signature, state.repeat_count);
            } else if state.repeat_count % 50 == 0 {
                log::debug!("{}: {} x{}", prefix, signature, state.repeat_count);
            }
            return;
        }

        state.signature = Some(signature.clone());
        state.repeat_count = 1;
        log::debug!("{}: {}", prefix, signature);
    }
}

impl RlpBackend for Rlp3Backend {
    fn set_log_context(&mut self, context: String) {
        self.log_context = Some(context);
    }

    fn receive_frame_bits(&mut self, bits: &[u8], rate_bps: u32) -> Option<Vec<u8>> {
        if rate_bps != 0 && !bits.is_empty() {
            capture::write_rlp_frame(CaptureDirection::Uplink, rate_bps, bits);
        }
        let rate = match rate_bps {
            9600 => FrameRate::Full,
            4800 => FrameRate::Half,
            2700 | 2400 => FrameRate::Quarter,
            1500 | 1200 => FrameRate::Eighth,
            _ => FrameRate::Blank,
        };
        // Log uplink frames (not blank ticks) for diagnostics.
        if rate != FrameRate::Blank && !bits.is_empty() {
            self.last_uplink_rate_bps = rate_bps;
            self.rx_frame_count += 1;
            if self.rx_frame_count <= 5 || self.rx_frame_count % 50 == 0 {
                let hex_preview: String = bits
                    .iter()
                    .take(32)
                    .map(|&b| if b != 0 { '1' } else { '0' })
                    .collect();
                log::debug!(
                    "RLP3 UL[{}]: frame #{} rate={} len={} bits={}{}",
                    self.log_context.as_deref().unwrap_or("?"),
                    self.rx_frame_count,
                    rate_bps,
                    bits.len(),
                    hex_preview,
                    if bits.len() > 32 { "..." } else { "" }
                );
            }
        }
        // Log received control frames (full-rate or sub-rate).
        let decoded_for_log = if rate == FrameRate::Full {
            Some(crate::rlp3_frames::decode_rlp3_frame(
                bits,
                crate::rlp3_frames::MuxOption::Odd,
            ))
        } else if let Some(n) = crate::rlp3_frames::sub_rate_info_bits(rate) {
            Some(crate::rlp3_frames::decode_sub_rate_frame(bits, n))
        } else {
            None
        };
        if let Some(ref result) = decoded_for_log {
            match result {
                Ok(frame) => {
                    self.rx_decoded_count = self.rx_decoded_count.saturating_add(1);
                    if matches!(frame, crate::rlp3_frames::Rlp3Frame::Control { .. }) {
                        Self::log_control_frame(
                            self.log_context.as_deref(),
                            "RX",
                            &mut self.rx_control_log,
                            frame,
                        );
                    }
                    if let crate::rlp3_frames::Rlp3Frame::Nak {
                        seq,
                        seq_hi,
                        payload,
                    } = frame
                    {
                        log::debug!(
                            "RLP3 PEER NAK[{}]: frame={} rate={} seq={} seq_hi={} payload={} tx_q={} rexmit_q={}",
                            self.log_context.as_deref().unwrap_or("?"),
                            self.rx_frame_count,
                            rate_bps,
                            seq,
                            seq_hi,
                            summarize_rlp3_nak_payload(payload),
                            self.session.tx_queue_len(),
                            self.session.rexmit_queue_len()
                        );
                    }
                    if !is_rlp3_idle_like(frame)
                        || self.rx_decoded_count <= 10
                        || self.rx_decoded_count % 500 == 0
                    {
                        log::debug!(
                            "RLP3 RXF[{}]: frame={} rate={} decoded={} summary={}",
                            self.log_context.as_deref().unwrap_or("?"),
                            self.rx_frame_count,
                            rate_bps,
                            self.rx_decoded_count,
                            summarize_rlp3_tx_frame(frame)
                        );
                    }
                }
                Err(e) => {
                    self.rx_decode_error_count = self.rx_decode_error_count.saturating_add(1);
                    let all_bits: String = bits
                        .iter()
                        .take(96)
                        .map(|&b| if b != 0 { '1' } else { '0' })
                        .collect();
                    let detail = if let Some(n) = crate::rlp3_frames::sub_rate_info_bits(rate) {
                        crate::rlp3_frames::diagnose_sub_rate_frame(bits, n)
                    } else {
                        "rate1_or_unknown".to_string()
                    };
                    log::warn!(
                        "RLP3 UL[{}]: decode failed: {:?} (error_count={} frame={} rate={} len={} detail={} bits={}{})",
                        self.log_context.as_deref().unwrap_or("?"),
                        e,
                        self.rx_decode_error_count,
                        self.rx_frame_count,
                        rate_bps,
                        bits.len(),
                        detail,
                        all_bits,
                        if bits.len() > 96 { "..." } else { "" }
                    );
                }
            }
        }
        let events = self.session.receive_frame(bits, rate);
        let mut delivered = Vec::new();
        for event in events {
            match event {
                RlpEvent::StateChanged(new_state) => {
                    log::debug!("RLP3: state → {:?}", new_state);
                }
                RlpEvent::DataDelivered(data) => {
                    delivered.extend_from_slice(&data);
                }
                RlpEvent::SendNak { first, last } => {
                    log::debug!("RLP3: NAK first={} last={}", first, last);
                }
                RlpEvent::NakAbandoned { first, last } => {
                    log::debug!("RLP3: NAK abandoned first={} last={}", first, last);
                }
            }
        }
        // Also drain the receive buffer (in-order data).
        if let Some(data) = self.session.receive_data() {
            delivered.extend_from_slice(&data);
        }
        if delivered.is_empty() {
            None
        } else {
            Some(delivered)
        }
    }

    fn enqueue_data(&mut self, data: &[u8]) {
        self.session.send_data(data);
    }

    fn next_frame_bits(&mut self, allow_payload: bool) -> (Vec<u8>, u32) {
        // Use full rate if we have data pending, pending controls (NAKs),
        // or are in handshake. In data-transfer idle periods, use quarter
        // rate: RLP3 less-than-Rate-1 fill/idle needs at least 40 info bits.
        let has_data = if self.session.state() != Rlp3State::DataTransfer {
            true
        } else if allow_payload {
            !self.session.tx_queue_is_empty() || self.session.has_pending_controls()
        } else {
            self.session.has_pending_control_frames()
        };
        let rate = if has_data {
            FrameRate::Full
        } else {
            FrameRate::Quarter
        };
        let state_before = self.session.state();
        let queue_before = self.session.tx_queue_len();
        let bits = if allow_payload {
            self.session.next_frame(rate)
        } else {
            self.session.next_frame_control_only(rate)
        };
        let queue_after = self.session.tx_queue_len();
        self.last_downlink_rate_bps = match rate {
            FrameRate::Full => 9600,
            FrameRate::Half => 4800,
            FrameRate::Quarter => 2700,
            FrameRate::Eighth => 1200,
            FrameRate::Blank => 0,
        };
        if rate == FrameRate::Full
            && let Ok(frame) =
                crate::rlp3_frames::decode_rlp3_frame(&bits, crate::rlp3_frames::MuxOption::Odd)
            && (state_before != Rlp3State::DataTransfer
                || matches!(frame, crate::rlp3_frames::Rlp3Frame::Control { .. }))
        {
            Self::log_control_frame(
                self.log_context.as_deref(),
                "TX",
                &mut self.tx_control_log,
                &frame,
            );
        }
        let rate_bps = match rate {
            FrameRate::Full => 9600,
            FrameRate::Half => 4800,
            FrameRate::Quarter => 2700,
            FrameRate::Eighth => 1200,
            FrameRate::Blank => 0,
        };
        if rate != FrameRate::Blank && !bits.is_empty() {
            self.tx_frame_count = self.tx_frame_count.saturating_add(1);
            capture::write_rlp_frame(CaptureDirection::Downlink, rate_bps, &bits);
            let summary = if rate == FrameRate::Full {
                crate::rlp3_frames::decode_rlp3_frame(&bits, crate::rlp3_frames::MuxOption::Odd)
                    .map(|frame| summarize_rlp3_tx_frame(&frame))
                    .unwrap_or_else(|e| format!("decode_error={:?}", e))
            } else if let Some(n) = crate::rlp3_frames::sub_rate_info_bits(rate) {
                crate::rlp3_frames::decode_sub_rate_frame(&bits, n)
                    .map(|frame| summarize_rlp3_tx_frame(&frame))
                    .unwrap_or_else(|e| format!("decode_error={:?}", e))
            } else {
                "blank_or_unsupported".to_string()
            };
            let should_log = !summary.starts_with("fill") && !summary.starts_with("idle")
                || self.tx_frame_count <= 10
                || self.tx_frame_count % 500 == 0;
            if should_log {
                log::debug!(
                    "RLP3 TXF[{}]: frame={} rate={} summary={} q_before={} q_after={}",
                    self.log_context.as_deref().unwrap_or("?"),
                    self.tx_frame_count,
                    rate_bps,
                    summary,
                    queue_before,
                    queue_after
                );
            } else {
                log::trace!(
                    "RLP3 TXF[{}]: rate={} frame={} q_before={} q_after={}",
                    self.log_context.as_deref().unwrap_or("?"),
                    rate_bps,
                    summary,
                    queue_before,
                    queue_after
                );
            }
        }
        (bits, rate_bps)
    }

    fn next_sch_frame_bits(&mut self, info_bits: usize, rate_bps: u32) -> Option<(Vec<u8>, u32)> {
        let profile = Rc3FschProfile::from_rate_bps(rate_bps)?;
        if self.session.state() != Rlp3State::DataTransfer || info_bits != profile.info_bits {
            return None;
        }

        let data_block_bits = sch_type3_data_block_bits(profile)?;
        let queue_before = self.session.tx_queue_len();
        let rexmit_before = self.session.rexmit_queue_len();
        let mut blocks = Vec::new();
        if let Some(first) = self.session.next_supplemental_frame(data_block_bits) {
            blocks.push(first);
        } else {
            if queue_before != 0 || rexmit_before != 0 {
                self.tx_sch_blocked_count = self.tx_sch_blocked_count.saturating_add(1);
                if self.tx_sch_blocked_count <= 10 || self.tx_sch_blocked_count % 50 == 0 {
                    log::debug!(
                        "RLP3 TX SCH blocked[{}]: count={} rate={} info_bits={} block_bits={} q={} rexmit_q={} l_v_s={} l_v_n_peer={} needs_seq_hi={}",
                        self.log_context.as_deref().unwrap_or("?"),
                        self.tx_sch_blocked_count,
                        rate_bps,
                        info_bits,
                        data_block_bits,
                        queue_before,
                        rexmit_before,
                        self.session.l_v_s(),
                        self.session.l_v_n_peer(),
                        self.session.next_new_data_requires_seq_hi()
                    );
                }
            }
        }
        let max_blocks = max_sch_type3_blocks_for_profile(profile);
        while blocks.len() < max_blocks {
            let Some(block) = self.session.next_supplemental_frame(data_block_bits) else {
                break;
            };
            blocks.push(block);
        }
        let queue_after = self.session.tx_queue_len();
        let rexmit_after = self.session.rexmit_queue_len();
        let data_block_octets =
            crate::rlp3_frames::supplemental_format_c_data_len(data_block_bits).unwrap_or(0);
        let rexmit_blocks = blocks
            .iter()
            .filter(|block| block.len() >= 2 && block[0] == 1 && block[1] == 1)
            .count();
        let new_blocks = blocks.len().saturating_sub(rexmit_blocks);
        let fill_only = blocks.is_empty();
        let sch_bits = build_sch_type3_sdu(&blocks, profile);

        self.tx_sch_sdu_count = self.tx_sch_sdu_count.saturating_add(1);
        if fill_only {
            if self.tx_sch_sdu_count <= 10 || self.tx_sch_sdu_count % 500 == 0 {
                log::debug!(
                    "RLP3 TX SCH fill[{}]: sdu={} rate={} info_bits={} block_bits={} q={} rexmit_q={}",
                    self.log_context.as_deref().unwrap_or("?"),
                    self.tx_sch_sdu_count,
                    profile.rate_bps,
                    info_bits,
                    data_block_bits,
                    queue_before,
                    rexmit_before
                );
            } else {
                log::trace!(
                    "RLP3 TX SCH fill: rate={} info_bits={} block_bits={}",
                    profile.rate_bps,
                    info_bits,
                    data_block_bits
                );
            }
        } else if self.tx_sch_sdu_count <= 10 || self.tx_sch_sdu_count % 50 == 0 {
            log::debug!(
                "RLP3 TX SCH[{}]: sdu={} blocks={} new={} rexmit={} rate={} info_bits={} block_bits={} block_octets={} q_before={} q_after={} rexmit_q_before={} rexmit_q_after={}",
                self.log_context.as_deref().unwrap_or("?"),
                self.tx_sch_sdu_count,
                blocks.len(),
                new_blocks,
                rexmit_blocks,
                profile.rate_bps,
                info_bits,
                data_block_bits,
                data_block_octets,
                queue_before,
                queue_after,
                rexmit_before,
                rexmit_after
            );
        } else {
            log::trace!(
                "RLP3 TX SCH: {} supplemental Format C frame(s) rate={} info_bits={} block_octets={}",
                blocks.len(),
                profile.rate_bps,
                info_bits,
                data_block_octets
            );
        }
        Some((sch_bits, profile.rate_bps))
    }

    fn is_data_transfer(&self) -> bool {
        self.session.state() == Rlp3State::DataTransfer
    }

    fn telemetry(&self) -> RlpTelemetry {
        RlpTelemetry {
            state: format_rlp3_state(self.session.state()),
            last_rx_control: self.rx_control_log.signature.clone().unwrap_or_default(),
            last_tx_control: self.tx_control_log.signature.clone().unwrap_or_default(),
            last_rx_control_repeats: self.rx_control_log.repeat_count,
            last_tx_control_repeats: self.tx_control_log.repeat_count,
            last_uplink_rate_bps: self.last_uplink_rate_bps,
            last_downlink_rate_bps: self.last_downlink_rate_bps,
        }
    }

    fn tx_queue_len(&self) -> usize {
        self.session.tx_queue_len()
    }
}

// ---------------------------------------------------------------------------
// Packet data session engine.
// ---------------------------------------------------------------------------

const SCH_0X0809_DATA_BLOCK_BITS: usize = 170;
const SCH_0X0921_DATA_BLOCK_BITS: usize = 346;
const SCH_TYPE3_HEADER_BITS: usize = 6;
const SCH_TYPE3_LTUS_REQUIRED_INFO_BITS: usize = 744;
const SCH_TYPE3_LTUS_SIZE_BITS: usize = 368;
const SCH_TYPE3_LTUS_PAYLOAD_BITS: usize = 352;
const SCH_TYPE3_LTUS_CRC_BITS: usize = 16;
const SCH_PRIMARY_SR_ID: u8 = 1;
const SCH_FILL_SR_ID: u8 = 0b111;

fn max_sch_type3_blocks_for_profile(profile: Rc3FschProfile) -> usize {
    let Some(data_block_bits) = sch_type3_data_block_bits(profile) else {
        return 0;
    };
    if sch_type3_uses_ltus(profile.info_bits) {
        let blocks_per_ltu = match data_block_bits {
            SCH_0X0809_DATA_BLOCK_BITS => 2,
            SCH_0X0921_DATA_BLOCK_BITS => 1,
            _ => 0,
        };
        return (profile.info_bits / SCH_TYPE3_LTUS_SIZE_BITS) * blocks_per_ltu;
    }
    profile.info_bits / (SCH_TYPE3_HEADER_BITS + data_block_bits)
}

fn sch_type3_data_block_bits(profile: Rc3FschProfile) -> Option<usize> {
    match profile.mux_option {
        0x0809 | 0x0811 | 0x0821 => Some(SCH_0X0809_DATA_BLOCK_BITS),
        0x0921 => Some(SCH_0X0921_DATA_BLOCK_BITS),
        _ => None,
    }
}

fn sch_type3_uses_ltus(info_bits: usize) -> bool {
    info_bits >= SCH_TYPE3_LTUS_REQUIRED_INFO_BITS
}

fn build_sch_type3_sdu(blocks: &[Vec<u8>], profile: Rc3FschProfile) -> Vec<u8> {
    let info_bits = profile.info_bits;
    let data_block_bits = sch_type3_data_block_bits(profile).unwrap_or(SCH_0X0809_DATA_BLOCK_BITS);

    if sch_type3_uses_ltus(info_bits) {
        return build_sch_type3_ltu_sdu(blocks, info_bits, data_block_bits);
    }

    let mut bits = Vec::with_capacity(info_bits);
    for block in blocks {
        debug_assert_eq!(block.len(), data_block_bits);
        if bits.len() + SCH_TYPE3_HEADER_BITS + data_block_bits > info_bits {
            break;
        }
        append_sch_type3_muxpdu(&mut bits, SCH_PRIMARY_SR_ID, block);
    }
    if bits.len() + SCH_TYPE3_HEADER_BITS <= info_bits {
        append_sch_type3_fill_muxpdu(&mut bits);
    }
    bits.resize(info_bits, 0);
    bits
}

fn build_sch_type3_ltu_sdu(
    blocks: &[Vec<u8>],
    info_bits: usize,
    data_block_bits: usize,
) -> Vec<u8> {
    let ltu_count = info_bits / SCH_TYPE3_LTUS_SIZE_BITS;
    let mut bits = Vec::with_capacity(info_bits);
    let mut next_block = blocks.iter();
    let blocks_per_ltu = match data_block_bits {
        SCH_0X0809_DATA_BLOCK_BITS => 2,
        SCH_0X0921_DATA_BLOCK_BITS => 1,
        _ => 0,
    };

    for _ in 0..ltu_count {
        let mut ltu_payload = Vec::with_capacity(SCH_TYPE3_LTUS_PAYLOAD_BITS);
        for _ in 0..blocks_per_ltu {
            if let Some(block) = next_block.next() {
                debug_assert_eq!(block.len(), data_block_bits);
                append_sch_type3_ltu_muxpdu(&mut ltu_payload, SCH_PRIMARY_SR_ID, block);
            } else {
                append_sch_type3_ltu_fill_muxpdu(&mut ltu_payload, data_block_bits);
            }
        }
        debug_assert_eq!(ltu_payload.len(), SCH_TYPE3_LTUS_PAYLOAD_BITS);
        let crc = crc16_sch(&ltu_payload);
        bits.extend_from_slice(&ltu_payload);
        push_bits(&mut bits, crc as u32, SCH_TYPE3_LTUS_CRC_BITS);
    }

    bits.resize(info_bits, 0);
    bits
}

fn append_sch_type3_muxpdu(bits: &mut Vec<u8>, sr_id: u8, data_block: &[u8]) {
    debug_assert!(matches!(sr_id, 1..=6));
    debug_assert!(matches!(
        data_block.len(),
        SCH_0X0809_DATA_BLOCK_BITS | SCH_0X0921_DATA_BLOCK_BITS
    ));
    let header_start = bits.len();
    push_bits(bits, sr_id as u32, 3);
    push_bits(bits, 0, 3);
    debug_assert_eq!(bits.len() - header_start, SCH_TYPE3_HEADER_BITS);
    bits.extend_from_slice(data_block);
}

fn append_sch_type3_fill_muxpdu(bits: &mut Vec<u8>) {
    let header_start = bits.len();
    push_bits(bits, SCH_FILL_SR_ID as u32, 3);
    push_bits(bits, 0, 3);
    debug_assert_eq!(bits.len() - header_start, SCH_TYPE3_HEADER_BITS);
    bits.resize(bits.len() + SCH_0X0809_DATA_BLOCK_BITS, 0);
}

fn append_sch_type3_ltu_muxpdu(bits: &mut Vec<u8>, sr_id: u8, data_block: &[u8]) {
    debug_assert!(matches!(sr_id, 1..=6));
    debug_assert!(matches!(
        data_block.len(),
        SCH_0X0809_DATA_BLOCK_BITS | SCH_0X0921_DATA_BLOCK_BITS
    ));
    append_sch_type3_muxpdu(bits, sr_id, data_block);
}

fn append_sch_type3_ltu_fill_muxpdu(bits: &mut Vec<u8>, data_block_bits: usize) {
    let muxpdu_bits = SCH_TYPE3_HEADER_BITS + data_block_bits;
    debug_assert!(bits.len() + muxpdu_bits <= SCH_TYPE3_LTUS_PAYLOAD_BITS);
    let header_start = bits.len();
    push_bits(bits, SCH_FILL_SR_ID as u32, 3);
    push_bits(bits, 0, 3);
    debug_assert_eq!(bits.len() - header_start, SCH_TYPE3_HEADER_BITS);
    bits.resize(bits.len() + data_block_bits, 0);
}

fn push_bits(bits: &mut Vec<u8>, value: u32, width: usize) {
    for bit in (0..width).rev() {
        bits.push(((value >> bit) & 1) as u8);
    }
}

pub struct PacketSession {
    rlp: Box<dyn RlpBackend>,
    deframer: HdlcDeframer,
    lcp: LcpSession,
    ipcp: IpcpSession,
    mobile_ip: MobileIpSession,
    vj: VjState,
    log_context: Option<String>,
    phase: SessionPhase,
    /// Pending PPP packets to send on downlink (queued during a single tick).
    ppp_tx_queue: Vec<PppPacket>,
    /// Host-originated IP packets waiting for PPP control negotiation to finish.
    pending_downlink_ip: VecDeque<Vec<u8>>,
    pending_downlink_ip_octets: usize,
    /// Whether we've sent our LCP Configure-Request.
    lcp_started: bool,
    /// Whether we've sent our IPCP Configure-Request.
    ipcp_started: bool,
    /// Always-on ring of recent PPP control-plane activity for diagnostics.
    recent_ppp_events: VecDeque<PacketTraceEvent>,
    /// When true, also generate supplemental channel frames each tick.
    sch_active: bool,
    /// SCH info bits per frame for the configured RC3 F-SCH rate.
    sch_info_bits: usize,
    /// Configured RC3 F-SCH rate.
    sch_rate_bps: u32,
    /// True after first IP traffic; PPP control stays on FCH before this.
    sch_data_ready: bool,
    /// Ticks spent in RlpSync without SYNC completing; bounded by
    /// `RLP_SYNC_MAX_TICKS` so a stuck MS gets torn down.
    rlp_sync_ticks: u32,
    /// Cached open PPP state to restore once this traffic channel's RLP is up.
    pending_ppp_resume: Option<PppSessionState>,
    /// Set whenever PPP control or IP payload crosses this engine.
    ppp_activity_since_last_check: bool,
    /// Set when the peer explicitly sends LCP Terminate-Request.
    peer_requested_lcp_terminate: bool,
    /// Some HRPD A10 byte-stream peers complete only one side of LCP, then wait
    /// for IPCP. This is scoped away from legacy 1x PPP.
    lcp_open_after_one_sided_ack: bool,
}

/// Max RlpSync ticks (20 ms each) before giving up. 500 = 10 s.
const RLP_SYNC_MAX_TICKS: u32 = 500;
/// HRPD A10 byte-stream PPP can begin while the AT is in the HRPD closed
/// connection state. The first PPP frames then drive DefaultPacket DataReady
/// paging, UATI refresh, and traffic re-open before the AT can answer LCP/IPCP.
/// Keep the RFC-style default Max-Configure for other sessions, but give this
/// dormant-reactivation path a one-minute negotiation budget.
const HRPD_A10_PPP_MAX_CONFIGURE_RESTARTS: u32 = 60;
/// Retransmits of our LCP Configure-Request (with the peer's ACK already in
/// hand) before an A10 peer is treated as one-sided and LCP is forced open.
const LCP_ONE_SIDED_ACK_MIN_RETRANSMITS: u32 = 2;
const PENDING_DOWNLINK_IP_MAX_PACKETS: usize = 256;
const PENDING_DOWNLINK_IP_MAX_OCTETS: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct PppSessionState {
    pub lcp: LcpOpenState,
    pub ipcp: IpcpOpenState,
    pub mobile_ip: Box<MobileIpSession>,
    pub vj: VjState,
}

impl PacketSession {
    pub fn new(service_option: u32, ipcp_config: IpcpConfig) -> Self {
        Self::new_with_ppp_resume(service_option, ipcp_config, None)
    }

    pub fn new_with_ppp_resume(
        service_option: u32,
        ipcp_config: IpcpConfig,
        ppp_resume: Option<PppSessionState>,
    ) -> Self {
        let rlp: Box<dyn RlpBackend> =
            if service_option == u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA) {
                log::debug!("PacketSession: using RLP Type 3 for SO {}", service_option);
                Box::new(Rlp3Backend::new(Rlp3Config {
                    mux_option: MuxOption::Odd, // 171-bit frames (MUX option 0x1, Rate Set 1)
                    ..Rlp3Config::default()
                }))
            } else {
                log::debug!("PacketSession: using RLP Type 1 for SO {}", service_option);
                Box::new(Rlp1Backend::new())
            };
        let mobile_ip = ppp_resume
            .as_ref()
            .map(|state| state.mobile_ip.as_ref().clone())
            .unwrap_or_else(|| MobileIpSession::new(ipcp_config.mobile_ip.clone()));
        Self {
            rlp,
            deframer: HdlcDeframer::new(),
            lcp: LcpSession::new(),
            ipcp: IpcpSession::new(ipcp_config),
            mobile_ip,
            vj: VjState::default(),
            log_context: None,
            phase: SessionPhase::RlpSync,
            ppp_tx_queue: Vec::new(),
            pending_downlink_ip: VecDeque::new(),
            pending_downlink_ip_octets: 0,
            lcp_started: false,
            ipcp_started: false,
            recent_ppp_events: VecDeque::new(),
            sch_active: false,
            sch_info_bits: Rc3FschProfile::default_19k2().info_bits,
            sch_rate_bps: DEFAULT_RC3_F_SCH_RATE_BPS,
            sch_data_ready: false,
            rlp_sync_ticks: 0,
            pending_ppp_resume: ppp_resume,
            ppp_activity_since_last_check: false,
            peer_requested_lcp_terminate: false,
            lcp_open_after_one_sided_ack: false,
        }
    }

    pub fn new_hrpd_default_packet(
        service_option: u32,
        ipcp_config: IpcpConfig,
        ppp_resume: Option<PppSessionState>,
    ) -> Self {
        log::debug!(
            "PacketSession: using HRPD Default Packet RLP for SO {}",
            service_option
        );
        let mobile_ip = ppp_resume
            .as_ref()
            .map(|state| state.mobile_ip.as_ref().clone())
            .unwrap_or_else(|| MobileIpSession::new(ipcp_config.mobile_ip.clone()));
        Self {
            rlp: Box::new(HrpdDefaultPacketBackend::new()),
            deframer: HdlcDeframer::new(),
            lcp: LcpSession::new(),
            ipcp: IpcpSession::new(ipcp_config),
            mobile_ip,
            vj: VjState::default(),
            log_context: None,
            phase: SessionPhase::RlpSync,
            ppp_tx_queue: Vec::new(),
            pending_downlink_ip: VecDeque::new(),
            pending_downlink_ip_octets: 0,
            lcp_started: false,
            ipcp_started: false,
            recent_ppp_events: VecDeque::new(),
            sch_active: false,
            sch_info_bits: Rc3FschProfile::default_19k2().info_bits,
            sch_rate_bps: DEFAULT_RC3_F_SCH_RATE_BPS,
            sch_data_ready: false,
            rlp_sync_ticks: 0,
            pending_ppp_resume: ppp_resume,
            ppp_activity_since_last_check: false,
            peer_requested_lcp_terminate: false,
            lcp_open_after_one_sided_ack: false,
        }
    }

    pub fn new_a10_unstructured_byte_stream(
        service_option: u32,
        ipcp_config: IpcpConfig,
        ppp_resume: Option<PppSessionState>,
    ) -> Self {
        log::debug!(
            "PacketSession: using A10 Unstructured Byte Stream for SO {}",
            service_option
        );
        let mobile_ip = ppp_resume
            .as_ref()
            .map(|state| state.mobile_ip.as_ref().clone())
            .unwrap_or_else(|| MobileIpSession::new(ipcp_config.mobile_ip.clone()));
        let mut session = Self {
            rlp: Box::new(UnstructuredByteStreamBackend::new()),
            deframer: HdlcDeframer::new(),
            lcp: LcpSession::new(),
            ipcp: IpcpSession::new(ipcp_config),
            mobile_ip,
            vj: VjState::default(),
            log_context: None,
            phase: SessionPhase::RlpSync,
            ppp_tx_queue: Vec::new(),
            pending_downlink_ip: VecDeque::new(),
            pending_downlink_ip_octets: 0,
            lcp_started: false,
            ipcp_started: false,
            recent_ppp_events: VecDeque::new(),
            sch_active: false,
            sch_info_bits: Rc3FschProfile::default_19k2().info_bits,
            sch_rate_bps: DEFAULT_RC3_F_SCH_RATE_BPS,
            sch_data_ready: false,
            rlp_sync_ticks: 0,
            pending_ppp_resume: ppp_resume,
            ppp_activity_since_last_check: false,
            peer_requested_lcp_terminate: false,
            lcp_open_after_one_sided_ack: true,
        };
        session.set_ppp_max_configure_restarts(HRPD_A10_PPP_MAX_CONFIGURE_RESTARTS);
        session
    }

    /// Enable or disable supplemental channel frame generation.
    pub fn set_sch_active(&mut self, active: bool) {
        self.sch_active = active;
        if !active {
            self.sch_data_ready = false;
        }
        log::info!(
            "{}: SCH {}",
            self.log_prefix("PacketSession"),
            if active { "activated" } else { "deactivated" }
        );
    }

    pub fn set_sch_active_with_rate(&mut self, active: bool, rate_bps: u32) {
        if let Some(profile) = Rc3FschProfile::from_rate_bps(rate_bps) {
            self.sch_info_bits = profile.info_bits;
            self.sch_rate_bps = profile.rate_bps;
        } else {
            log::warn!(
                "{}: unsupported SCH rate {}, keeping {}",
                self.log_prefix("PacketSession"),
                rate_bps,
                self.sch_rate_bps
            );
        }
        self.set_sch_active(active);
    }

    /// Returns whether SCH is active.
    pub fn is_sch_active(&self) -> bool {
        self.sch_active
    }

    pub fn enable_sch_data_path(&mut self) {
        if !self.sch_data_ready {
            self.sch_data_ready = true;
            log::info!(
                "{}: SCH data path enabled after first downlink IP",
                self.log_prefix("PacketSession")
            );
        }
    }

    pub fn downlink_queue_len(&self) -> usize {
        self.rlp.tx_queue_len()
    }

    pub fn set_log_context(&mut self, context: String) {
        self.rlp.set_log_context(context.clone());
        self.lcp.set_log_context(context.clone());
        self.ipcp.set_log_context(context.clone());
        self.log_context = Some(context);
    }

    fn set_ppp_max_configure_restarts(&mut self, restarts: u32) {
        self.lcp.set_max_configure_restarts(restarts);
        self.ipcp.set_max_configure_restarts(restarts);
    }

    fn log_prefix(&self, label: &str) -> String {
        match self.log_context.as_deref() {
            Some(context) => format!("{}[{}]", label, context),
            None => label.to_string(),
        }
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn peer_ip(&self) -> Ipv4Addr {
        self.ipcp.peer_ip()
    }

    pub fn our_ip(&self) -> Ipv4Addr {
        self.ipcp.our_ip()
    }

    /// Reassign the configured peer IP after a successful allocator claim;
    /// IPCP state flags are untouched.
    pub fn reassign_peer_ip(&mut self, peer_ip: Ipv4Addr) {
        self.ipcp.reassign_peer_ip(peer_ip);
        log::info!(
            "{}: peer IP reassigned to {}",
            self.log_prefix("PacketSession"),
            peer_ip
        );
    }

    /// Inject an IP packet from the network/TUN side for delivery to the mobile.
    /// Queues while PPP control is negotiating; emits only after IPCP is open.
    pub fn send_ip_packet(&mut self, ip_packet: &[u8]) {
        match self.phase {
            SessionPhase::Active => self.enqueue_downlink_ip_packet(ip_packet),
            SessionPhase::Closed => {
                log::debug!(
                    "{}: dropping downlink IP packet len={} in closed session",
                    self.log_prefix("IP egress"),
                    ip_packet.len()
                );
            }
            _ => self.queue_pending_downlink_ip(ip_packet),
        }
    }

    fn enqueue_downlink_ip_packet(&mut self, ip_packet: &[u8]) {
        self.ppp_activity_since_last_check = true;
        let ppp = self.vj.compress_ip_packet(ip_packet);
        self.ppp_tx_queue.push(ppp);
    }

    fn queue_pending_downlink_ip(&mut self, ip_packet: &[u8]) {
        if self.pending_downlink_ip.is_empty() {
            log::debug!(
                "{}: queueing downlink IP while phase={:?}",
                self.log_prefix("IP egress"),
                self.phase
            );
        }
        self.pending_downlink_ip_octets += ip_packet.len();
        self.pending_downlink_ip.push_back(ip_packet.to_vec());

        let mut evicted_packets = 0usize;
        let mut evicted_octets = 0usize;
        while self.pending_downlink_ip.len() > PENDING_DOWNLINK_IP_MAX_PACKETS
            || self.pending_downlink_ip_octets > PENDING_DOWNLINK_IP_MAX_OCTETS
        {
            let Some(evicted) = self.pending_downlink_ip.pop_front() else {
                break;
            };
            evicted_packets += 1;
            evicted_octets += evicted.len();
            self.pending_downlink_ip_octets = self
                .pending_downlink_ip_octets
                .saturating_sub(evicted.len());
        }

        if evicted_packets > 0 {
            log::warn!(
                "{}: evicted queued downlink IP packets={} octets={} remaining_packets={} remaining_octets={}",
                self.log_prefix("IP egress"),
                evicted_packets,
                evicted_octets,
                self.pending_downlink_ip.len(),
                self.pending_downlink_ip_octets
            );
        }
    }

    fn flush_pending_downlink_ip(&mut self) {
        if self.pending_downlink_ip.is_empty() {
            return;
        }
        let packets = self.pending_downlink_ip.len();
        let octets = self.pending_downlink_ip_octets;
        while let Some(ip_packet) = self.pending_downlink_ip.pop_front() {
            self.pending_downlink_ip_octets = self
                .pending_downlink_ip_octets
                .saturating_sub(ip_packet.len());
            self.enqueue_downlink_ip_packet(&ip_packet);
        }
        debug_assert_eq!(self.pending_downlink_ip_octets, 0);
        log::info!(
            "{}: flushed queued downlink IP packets={} octets={}",
            self.log_prefix("IP egress"),
            packets,
            octets
        );
    }

    pub fn snapshot_ppp_state(&self) -> Option<PppSessionState> {
        Some(PppSessionState {
            lcp: self.lcp.open_state()?,
            ipcp: self.ipcp.open_state()?,
            mobile_ip: Box::new(self.mobile_ip.clone()),
            vj: self.vj.clone(),
        })
    }

    pub fn take_ppp_activity(&mut self) -> bool {
        let active = self.ppp_activity_since_last_check;
        self.ppp_activity_since_last_check = false;
        active
    }

    pub fn take_peer_requested_lcp_terminate(&mut self) -> bool {
        let requested = self.peer_requested_lcp_terminate;
        self.peer_requested_lcp_terminate = false;
        requested
    }

    /// Ingest an A10 Unstructured Byte Stream chunk without advancing the
    /// 20 ms packet-session clock. A10 chunks are bearer transport fragments,
    /// not air-interface frames, so their arrival rate must not drive PPP
    /// restart timers or impose a one-chunk-per-tick receive limit.
    pub fn ingest_a10_byte_stream(&mut self, octets: &[u8]) -> Vec<SessionAction> {
        let mut actions = Vec::new();
        self.process_uplink(Some((octets, 0)), &mut actions);
        actions
    }

    /// Frame queued downlink PPP and emit complete A10 chunks without advancing
    /// protocol timers. A partial byte-stream tail remains queued for the next
    /// arrival or the regular tick, which bounds latency while preserving A10
    /// datagram packing efficiency.
    ///
    /// HRPD A10 callers use this when an IP packet arrives so the 20 ms
    /// protocol clock does not impose a burst cadence on the bearer.
    pub fn emit_full_pending_downlink(&mut self) -> Vec<SessionAction> {
        let mut actions = Vec::new();
        self.append_pending_downlink_actions(&mut actions, false);
        actions
    }

    fn append_pending_downlink_actions(
        &mut self,
        actions: &mut Vec<SessionAction>,
        flush_partial: bool,
    ) {
        let frame_opts = if self.lcp.is_open() {
            framing::FrameOptions {
                tx_accm: self.lcp.negotiated.tx_accm,
                acfc: self.lcp.negotiated.we_send_acfc,
                pfc: self.lcp.negotiated.we_send_pfc,
            }
        } else {
            framing::FrameOptions::default()
        };
        let ppp_queue: Vec<PppPacket> = self.ppp_tx_queue.drain(..).collect();
        for ppp in &ppp_queue {
            self.ppp_activity_since_last_check = true;
            self.record_ppp_event("downlink", ppp);
            log::trace!("{}: {}", self.log_prefix("PPP TX"), format_ppp_packet(ppp));
            let txq_before = self.rlp.tx_queue_len();
            capture::write_ppp_packet(CaptureDirection::Downlink, ppp, &frame_opts);
            let hdlc_bytes = framing::frame_with_options(ppp, &frame_opts);
            let hdlc_len = hdlc_bytes.len();
            self.rlp.enqueue_data(&hdlc_bytes);
            let txq_after = self.rlp.tx_queue_len();
            log::trace!(
                "{}: {} hdlc_len={} rlp_txq_before={} rlp_txq_after={}",
                self.log_prefix("PPP TX enqueue"),
                format_ppp_packet(ppp),
                hdlc_len,
                txq_before,
                txq_after
            );
        }

        let sch_data_path =
            self.sch_active && self.phase == SessionPhase::Active && self.sch_data_ready;
        let allow_fch_payload = !sch_data_path;
        let max_downlink_frames = self.rlp.max_downlink_frames_per_tick().max(1);
        for _ in 0..max_downlink_frames {
            let (bits, rate_bps) = if flush_partial {
                self.rlp.next_frame_bits(allow_fch_payload)
            } else {
                self.rlp.next_full_frame_bits(allow_fch_payload)
            };
            if bits.is_empty() {
                break;
            }
            actions.push(SessionAction::SendFrame { bits, rate_bps });
        }

        if self.sch_active && self.phase == SessionPhase::Active && self.sch_data_ready {
            if let Some((sch_bits, sch_rate)) = self
                .rlp
                .next_sch_frame_bits(self.sch_info_bits, self.sch_rate_bps)
            {
                actions.push(SessionAction::SendSchFrame {
                    bits: sch_bits,
                    rate_bps: sch_rate,
                });
            }
        }
    }

    /// Process one frame period (20ms tick).
    ///
    /// `uplink`: raw primary traffic bits + rate_bps from the mobile this period,
    /// or `None` for an erasure/no frame.
    ///
    /// Returns a list of actions for the caller to perform.
    pub fn tick(&mut self, uplink: Option<(&[u8], u32)>) -> Vec<SessionAction> {
        let mut actions = Vec::new();
        self.process_uplink(uplink, &mut actions);

        if self.phase == SessionPhase::RlpSync {
            self.rlp_sync_ticks = self.rlp_sync_ticks.saturating_add(1);
            if self.rlp_sync_ticks == RLP_SYNC_MAX_TICKS {
                log::warn!(
                    "{}: SYNC handshake did not complete in {} ticks ({} ms), closing session",
                    self.log_prefix("RLP"),
                    RLP_SYNC_MAX_TICKS,
                    RLP_SYNC_MAX_TICKS * 20
                );
                actions.push(SessionAction::CloseSession {
                    reason: format!("RLP3 SYNC timeout after {} ms", RLP_SYNC_MAX_TICKS * 20),
                });
                return actions;
            }
        } else {
            self.rlp_sync_ticks = 0;
        }

        // --- Send our initial requests when entering new phases ---
        if self.phase == SessionPhase::Lcp && !self.lcp_started {
            let req = self.lcp.start();
            self.ppp_tx_queue.push(req);
            self.lcp_started = true;
        }

        if self.phase == SessionPhase::Ipcp && !self.ipcp_started {
            let req = self.ipcp.start();
            self.ppp_tx_queue.push(req);
            self.ipcp_started = true;
        }

        // PPP requires pending Configure-Requests to be retransmitted until
        // they are ACKed. Bearer/air delivery can lose the first request, so
        // drive the restart timers from the packet-session tick cadence.
        if self.phase == SessionPhase::Lcp
            && let Some(req) = self.lcp.maybe_retransmit_configure_request()
        {
            self.ppp_tx_queue.push(req);
        }
        self.maybe_open_lcp_after_one_sided_ack();
        if self.phase == SessionPhase::Lcp && self.lcp.configure_failed() {
            actions.push(SessionAction::CloseSession {
                reason: format!(
                    "LCP Configure-Request failed after {} retransmits",
                    self.lcp.configure_restarts()
                ),
            });
        }

        if self.phase == SessionPhase::Ipcp
            && let Some(req) = self.ipcp.maybe_retransmit_configure_request()
        {
            self.ppp_tx_queue.push(req);
        }
        if self.phase == SessionPhase::Ipcp && self.ipcp.configure_failed() {
            actions.push(SessionAction::CloseSession {
                reason: format!(
                    "IPCP Configure-Request failed after {} retransmits",
                    self.ipcp.configure_restarts()
                ),
            });
        }

        self.append_pending_downlink_actions(&mut actions, true);

        actions
    }

    /// Close the session.
    /// Generate an LCP Echo-Request keepalive if the link is open.
    /// Call periodically (e.g., every 30s). Returns a PPP packet to
    /// send via the RLP downlink, or None.
    pub fn maybe_send_echo(&mut self) -> Option<Vec<u8>> {
        let ppp = self.lcp.maybe_send_echo()?;
        self.ppp_activity_since_last_check = true;
        self.record_ppp_event("TX", &ppp);
        capture::write_ppp_packet(
            CaptureDirection::Downlink,
            &ppp,
            &framing::FrameOptions::default(),
        );
        let frame = framing::frame(&ppp);
        self.rlp.enqueue_data(&frame);
        Some(frame)
    }

    /// Returns true if the LCP echo keepalive has detected a dead peer.
    pub fn echo_dead(&self) -> bool {
        self.lcp.echo_dead()
    }

    pub fn close(&mut self) {
        self.phase = SessionPhase::Closed;
        self.pending_downlink_ip.clear();
        self.pending_downlink_ip_octets = 0;
    }

    pub fn telemetry(&self) -> PacketSessionTelemetry {
        let rlp = self.rlp.telemetry();
        PacketSessionTelemetry {
            rlp_state: rlp.state,
            lcp_state: format_lcp_state(self.lcp.state),
            ipcp_state: format_ipcp_state(self.ipcp.state),
            lcp_configure_restarts: self.lcp.configure_restarts(),
            ipcp_configure_restarts: self.ipcp.configure_restarts(),
            ipcp_omitted_peer_ip_naks: self.ipcp.omitted_peer_ip_naks(),
            last_rx_control: rlp.last_rx_control,
            last_tx_control: rlp.last_tx_control,
            last_rx_control_repeats: rlp.last_rx_control_repeats,
            last_tx_control_repeats: rlp.last_tx_control_repeats,
            last_uplink_rate_bps: rlp.last_uplink_rate_bps,
            last_downlink_rate_bps: rlp.last_downlink_rate_bps,
            recent_ppp_events: self.recent_ppp_events.iter().cloned().collect(),
        }
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn process_uplink(&mut self, uplink: Option<(&[u8], u32)>, actions: &mut Vec<SessionAction>) {
        let delivery = if let Some((bits, rate_bps)) = uplink {
            self.rlp.receive_frame_bits(bits, rate_bps)
        } else {
            self.rlp.receive_frame_bits(&[], 0)
        };

        // Check if RLP just entered DataTransfer (phase transition RlpSync → Lcp).
        if self.phase == SessionPhase::RlpSync && self.rlp.is_data_transfer() {
            if let Some(ppp_state) = self.pending_ppp_resume.take() {
                self.lcp.restore_open_state(ppp_state.lcp);
                self.ipcp.restore_open_state(ppp_state.ipcp);
                self.mobile_ip = *ppp_state.mobile_ip;
                self.vj = ppp_state.vj;
                self.lcp_started = true;
                self.ipcp_started = true;
                self.phase = SessionPhase::Active;
                self.ppp_activity_since_last_check = true;
                self.flush_pending_downlink_ip();
                log::info!(
                    "{}: link established, resumed open PPP session peer={} gateway={}",
                    self.log_prefix("RLP"),
                    self.ipcp.peer_ip(),
                    self.ipcp.our_ip()
                );
            } else {
                log::debug!(
                    "{}: link established, entering LCP phase",
                    self.log_prefix("RLP")
                );
                self.phase = SessionPhase::Lcp;
            }
            self.rlp_sync_ticks = 0;
        }

        let Some(data) = delivery else {
            return;
        };
        log::trace!(
            "{}: delivered {} bytes to PPP deframer",
            self.log_prefix("RLP"),
            data.len()
        );
        let ppp_packets = self.deframer.feed(&data);
        for ppp in ppp_packets {
            self.ppp_activity_since_last_check = true;
            capture::write_ppp_packet(
                CaptureDirection::Uplink,
                &ppp,
                &self.uplink_capture_frame_options(&ppp),
            );
            self.record_ppp_event("uplink", &ppp);
            log::trace!("{}: {}", self.log_prefix("PPP RX"), format_ppp_packet(&ppp));
            self.process_uplink_ppp(&ppp, actions);
        }
    }

    fn process_uplink_ppp(&mut self, ppp: &PppPacket, actions: &mut Vec<SessionAction>) {
        match ppp.protocol {
            LCP_PROTOCOL => {
                let was_open = self.lcp.is_open();
                let peer_terminated = ppp.payload.first().copied() == Some(TERMINATE_REQUEST);
                let responses = self.lcp.receive(ppp);
                for resp in responses {
                    self.ppp_tx_queue.push(resp);
                }
                if peer_terminated {
                    self.peer_requested_lcp_terminate = true;
                }

                // Peer restarted LCP while we were in IPCP or Active. RFC
                // 1661 sends PPP back to Link Establishment; the traffic
                // channel stays up while upper-layer NCPs renegotiate.
                if was_open && !self.lcp.is_open() {
                    log::info!(
                        "{}: peer restarted, resetting IPCP and returning to LCP phase",
                        self.log_prefix("LCP")
                    );
                    self.ipcp = IpcpSession::new(self.ipcp.config.clone());
                    self.mobile_ip = MobileIpSession::new(self.ipcp.config.mobile_ip.clone());
                    if let Some(context) = &self.log_context {
                        self.ipcp.set_log_context(context.clone());
                    }
                    self.vj = VjState::default();
                    self.ipcp_started = false;
                    self.sch_data_ready = false;
                    self.phase = SessionPhase::Lcp;
                }

                // Check for LCP open → transition to IPCP.
                if self.lcp.is_open() && self.phase == SessionPhase::Lcp {
                    log::info!(
                        "{}: link opened, entering IPCP phase",
                        self.log_prefix("LCP")
                    );
                    self.phase = SessionPhase::Ipcp;
                }
            }
            IPCP_PROTOCOL => {
                if !self.lcp.is_open() && self.lcp.state == LcpState::AckSent {
                    log::info!(
                        "{}: received IPCP while in AckSent, forcing LCP open",
                        self.log_prefix("LCP")
                    );
                    self.lcp.force_open();
                    self.phase = SessionPhase::Ipcp;
                }

                let responses = self.ipcp.receive(ppp);
                for resp in responses {
                    self.ppp_tx_queue.push(resp);
                }
                // Check for IPCP open → transition to IP forwarding or MIP4 registration.
                if self.ipcp.is_open() && self.phase == SessionPhase::Ipcp {
                    self.enter_ipcp_open_phase();
                }
            }
            PPP_IP_PROTOCOL => {
                self.deliver_uplink_ip_packet(&ppp.payload, actions);
            }
            PPP_VJ_UNCOMPRESSED_TCP | PPP_VJ_COMPRESSED_TCP => {
                match self.vj.decompress_packet(ppp.protocol, &ppp.payload) {
                    Ok(ip_packet) => self.deliver_uplink_ip_packet(&ip_packet, actions),
                    Err(err) => {
                        log::warn!(
                            "{}: dropping VJ packet protocol=0x{:04X} err={:?} len={}",
                            self.log_prefix("PPP RX"),
                            ppp.protocol,
                            err,
                            ppp.payload.len()
                        );
                    }
                }
            }
            other => {
                log::debug!(
                    "{}: ignoring PPP protocol 0x{:04X}",
                    self.log_prefix("PPP RX"),
                    other
                );
            }
        }
    }

    fn maybe_open_lcp_after_one_sided_ack(&mut self) {
        if !self.lcp_open_after_one_sided_ack
            || self.phase != SessionPhase::Lcp
            || self.lcp.is_open()
        {
            return;
        }
        // Only treat the peer as one-sided once it has ACKed our
        // Configure-Request yet ignored repeated retransmits of it without
        // ever sending its own. A peer that skips its Configure-Request but
        // moves on to IPCP is handled by the IPCP-triggered force open.
        if !self.lcp.peer_acked_our_request()
            || self.lcp.configure_restarts() < LCP_ONE_SIDED_ACK_MIN_RETRANSMITS
        {
            return;
        }

        log::info!(
            "{}: peer ACKed our Configure-Request but sent none after {} retransmits; \
             forcing open for A10 and entering IPCP",
            self.log_prefix("LCP"),
            self.lcp.configure_restarts()
        );
        self.lcp.force_open();
        self.phase = SessionPhase::Ipcp;
    }

    fn deliver_uplink_ip_packet(&mut self, ip_packet: &[u8], actions: &mut Vec<SessionAction>) {
        if self.phase == SessionPhase::Mip4Pending {
            if !self.handle_mobile_ip_packet(ip_packet) {
                log::debug!(
                    "{}: dropping non-MIP4 packet while registration is pending",
                    self.log_prefix("IP ingress")
                );
            }
            return;
        }
        if self.mobile_ip.config().enabled
            && self.mobile_ip.binding().is_some()
            && self.handle_mobile_ip_packet(ip_packet)
        {
            return;
        }

        // Forward only in Active when src matches the IPCP-negotiated peer.
        let Some(src_ip) = self.parse_uplink_ipv4(ip_packet) else {
            return;
        };

        if self.phase != SessionPhase::Active {
            log::debug!(
                "{}: dropping uplink IP packet src={} in phase={:?} (IPCP not yet open)",
                self.log_prefix("IP ingress"),
                src_ip,
                self.phase
            );
            return;
        }
        let expected = self.ipcp.peer_ip();
        if src_ip != expected {
            log::debug!(
                "{}: dropping uplink IP packet src={} (expected {} per IPCP)",
                self.log_prefix("IP ingress"),
                src_ip,
                expected
            );
            return;
        }
        actions.push(SessionAction::DeliverIpPacket(ip_packet.to_vec()));
        self.ppp_activity_since_last_check = true;
    }

    fn handle_mobile_ip_packet(&mut self, ip_packet: &[u8]) -> bool {
        match self
            .mobile_ip
            .handle_ipv4_packet(ip_packet, self.ipcp.peer_ip())
        {
            MobileIpPacketResult::NotMobileIp => false,
            MobileIpPacketResult::Ignored => true,
            MobileIpPacketResult::AuthenticationRequired => {
                let req = self.ipcp.restart_for_simple_ip();
                self.ppp_tx_queue.push(req);
                self.phase = SessionPhase::Ipcp;
                self.ppp_activity_since_last_check = true;
                true
            }
            MobileIpPacketResult::Reply(reply) | MobileIpPacketResult::Deregistered { reply } => {
                self.ppp_tx_queue.push(PppPacket {
                    protocol: PPP_IP_PROTOCOL,
                    payload: reply,
                });
                self.ppp_activity_since_last_check = true;
                if self.mobile_ip.binding().is_none() && self.phase == SessionPhase::Active {
                    self.phase = SessionPhase::Mip4Pending;
                }
                true
            }
            MobileIpPacketResult::Registered { binding, reply } => {
                self.ppp_tx_queue.push(PppPacket {
                    protocol: PPP_IP_PROTOCOL,
                    payload: reply,
                });
                self.ipcp.reassign_peer_ip(binding.home_address);
                self.phase = SessionPhase::Active;
                self.ppp_activity_since_last_check = true;
                self.flush_pending_downlink_ip();
                log::info!(
                    "{}: MIP4 registration accepted, active peer={} home_agent={} lifetime={}",
                    self.log_prefix("PacketSession"),
                    binding.home_address,
                    binding.home_agent,
                    binding.lifetime_secs
                );
                true
            }
        }
    }

    fn enter_ipcp_open_phase(&mut self) {
        self.vj
            .configure(self.ipcp.peer_vj_options(), self.ipcp.local_vj_options());
        if self.ipcp.config.mobile_ip.enabled && !self.ipcp.peer_ip_address_negotiated() {
            self.mobile_ip.reset(self.ipcp.config.mobile_ip.clone());
            self.phase = SessionPhase::Mip4Pending;
            log::info!(
                "{}: IPCP opened without peer IP, waiting for MIP4 registration peer={} fa={} ha={}",
                self.log_prefix("PacketSession"),
                self.ipcp.peer_ip(),
                self.mobile_ip.config().fa_address,
                self.mobile_ip.config().home_agent_address
            );
            if let Some(advertisement) = self.mobile_ip.next_unsolicited_advertisement() {
                self.ppp_tx_queue.push(PppPacket {
                    protocol: PPP_IP_PROTOCOL,
                    payload: advertisement,
                });
            }
            return;
        }

        self.phase = SessionPhase::Active;
        self.flush_pending_downlink_ip();
        log::info!(
            "{}: active peer={} gateway={}",
            self.log_prefix("PacketSession"),
            self.ipcp.peer_ip(),
            self.ipcp.our_ip()
        );
    }

    fn parse_uplink_ipv4(&self, payload: &[u8]) -> Option<Ipv4Addr> {
        if payload.len() < 20 || (payload[0] >> 4) != 4 {
            log::warn!(
                "{}: dropping malformed packet (len={} ver={})",
                self.log_prefix("IP ingress"),
                payload.len(),
                payload.get(0).map(|b| b >> 4).unwrap_or(0),
            );
            return None;
        }
        let src_ip = Ipv4Addr::new(payload[12], payload[13], payload[14], payload[15]);
        if src_ip.is_unspecified() {
            // DHCP discover (0.0.0.0 -> 255.255.255.255) is normal phone
            // behavior, but CDMA2000 packet data assigns IP via IPCP.
            log::debug!(
                "{}: ignoring DHCP discover from 0.0.0.0",
                self.log_prefix("IP ingress")
            );
            return None;
        }
        Some(src_ip)
    }

    fn uplink_capture_frame_options(&self, ppp: &PppPacket) -> framing::FrameOptions {
        if ppp.protocol == LCP_PROTOCOL || !self.lcp.is_open() {
            framing::FrameOptions::default()
        } else {
            framing::FrameOptions {
                tx_accm: self.lcp.negotiated.rx_accm,
                acfc: self.lcp.negotiated.peer_sends_acfc,
                pfc: self.lcp.negotiated.peer_sends_pfc,
            }
        }
    }

    fn record_ppp_event(&mut self, direction: &str, ppp: &PppPacket) {
        if ppp.protocol == PPP_IP_PROTOCOL {
            return;
        }
        const MAX_EVENTS: usize = 64;
        if self.recent_ppp_events.len() >= MAX_EVENTS {
            self.recent_ppp_events.pop_front();
        }
        self.recent_ppp_events.push_back(PacketTraceEvent {
            timestamp_ms: now_ms(),
            layer: "ppp".to_string(),
            direction: direction.to_string(),
            summary: format_ppp_packet(ppp),
            detail: format!(
                "protocol={} payload_len={}",
                format_ppp_protocol(ppp.protocol),
                ppp.payload.len()
            ),
            payload_hex: bytes_to_hex(&ppp.payload),
        });
    }
}

#[derive(Debug, Default)]
struct RepeatedControlLog {
    signature: Option<String>,
    repeat_count: u64,
}

/// Returns true for RLP handshake control frames (SYNC, SYNC/ACK, ACK).
fn is_rlp_handshake(frame: &RlpFrame) -> bool {
    matches!(
        frame,
        RlpFrame::Control {
            control_type: crate::rlp::ControlType::Sync
                | crate::rlp::ControlType::SyncAck
                | crate::rlp::ControlType::Ack,
            ..
        }
    )
}

/// Format an RLP Type 1 frame for logging.
fn format_rlp_frame(frame: &RlpFrame) -> String {
    match frame {
        RlpFrame::Control {
            seq,
            control_type,
            first,
            last,
            ..
        } => {
            let ct = match control_type {
                crate::rlp::ControlType::Sync => "SYNC",
                crate::rlp::ControlType::SyncAck => "SYNC/ACK",
                crate::rlp::ControlType::Ack => "ACK",
                crate::rlp::ControlType::Nak => "NAK",
            };
            if matches!(control_type, crate::rlp::ControlType::Nak) {
                format!("Control({} seq={} first={} last={})", ct, seq, first, last)
            } else {
                format!("Control({} seq={})", ct, seq)
            }
        }
        RlpFrame::Data { seq, data } => {
            format!(
                "Data(seq={} len={} payload={})",
                seq,
                data.len(),
                hex_preview(data, 32)
            )
        }
        RlpFrame::DataFormatB { seq, data } => {
            format!(
                "DataFmtB(seq={} len={} payload={})",
                seq,
                data.len(),
                hex_preview(data, 32)
            )
        }
        RlpFrame::Segmented {
            seq,
            segment_type,
            data,
        } => {
            format!(
                "Segmented(seq={} type={:?} len={} payload={})",
                seq,
                segment_type,
                data.len(),
                hex_preview(data, 32)
            )
        }
        RlpFrame::Idle { seq } => {
            format!("Idle(seq={})", seq)
        }
    }
}

fn format_rlp3_frame(frame: &crate::rlp3_frames::Rlp3Frame) -> String {
    match frame {
        crate::rlp3_frames::Rlp3Frame::Control {
            seq,
            control_type,
            init_var,
            nak_param_incl,
        } => {
            let ct = match control_type {
                crate::rlp3_frames::Rlp3ControlType::Sync => "SYNC",
                crate::rlp3_frames::Rlp3ControlType::SyncAck => "SYNC/ACK",
                crate::rlp3_frames::Rlp3ControlType::Ack => "ACK",
                crate::rlp3_frames::Rlp3ControlType::Nak => "NAK",
            };
            format!(
                "Control({} seq={} init_var={} nak_param_incl={})",
                ct, seq, init_var, nak_param_incl
            )
        }
        crate::rlp3_frames::Rlp3Frame::Data { seq, rexmit, data } => {
            format!(
                "Data(seq={} rexmit={} len={} payload={})",
                seq,
                rexmit,
                data.len(),
                hex_preview(data, 32)
            )
        }
        crate::rlp3_frames::Rlp3Frame::DataFormatB { seq, rexmit, data } => {
            format!(
                "DataFmtB(seq={} rexmit={} len={} payload={})",
                seq,
                rexmit,
                data.len(),
                hex_preview(data, 32)
            )
        }
        crate::rlp3_frames::Rlp3Frame::Nak {
            seq,
            seq_hi,
            payload,
        } => format!("Nak(seq={} seq_hi={} payload={:?})", seq, seq_hi, payload),
        crate::rlp3_frames::Rlp3Frame::Segmented {
            seq,
            sqi,
            last_seg,
            rexmit,
            seq_hi,
            s_seq,
            data,
        } => format!(
            "Segmented(seq={} sqi={} last_seg={} rexmit={} seq_hi={:?} s_seq={} len={} payload={})",
            seq,
            sqi,
            last_seg,
            rexmit,
            seq_hi,
            s_seq,
            data.len(),
            hex_preview(data, 32)
        ),
        crate::rlp3_frames::Rlp3Frame::Fill { seq, seq_hi } => {
            format!("Fill(seq={} seq_hi={})", seq, seq_hi)
        }
        crate::rlp3_frames::Rlp3Frame::Idle1 { seq, seq_hi } => {
            format!("Idle1(seq={} seq_hi={})", seq, seq_hi)
        }
        crate::rlp3_frames::Rlp3Frame::Idle2 { seq } => format!("Idle2(seq={})", seq),
    }
}

fn is_rlp3_idle_like(frame: &crate::rlp3_frames::Rlp3Frame) -> bool {
    match frame {
        crate::rlp3_frames::Rlp3Frame::Fill { .. }
        | crate::rlp3_frames::Rlp3Frame::Idle1 { .. }
        | crate::rlp3_frames::Rlp3Frame::Idle2 { .. } => true,
        crate::rlp3_frames::Rlp3Frame::Data { data, .. } => data.is_empty(),
        _ => false,
    }
}

fn summarize_rlp3_tx_frame(frame: &crate::rlp3_frames::Rlp3Frame) -> String {
    match frame {
        crate::rlp3_frames::Rlp3Frame::Control { control_type, .. } => {
            let kind = match control_type {
                crate::rlp3_frames::Rlp3ControlType::Sync => "control:sync",
                crate::rlp3_frames::Rlp3ControlType::SyncAck => "control:sync_ack",
                crate::rlp3_frames::Rlp3ControlType::Ack => "control:ack",
                crate::rlp3_frames::Rlp3ControlType::Nak => "control:nak",
            };
            kind.to_string()
        }
        crate::rlp3_frames::Rlp3Frame::Data { seq, rexmit, data } => {
            format!("data seq={} rexmit={} bytes={}", seq, rexmit, data.len())
        }
        crate::rlp3_frames::Rlp3Frame::DataFormatB { seq, rexmit, data } => {
            format!("data_b seq={} rexmit={} bytes={}", seq, rexmit, data.len())
        }
        crate::rlp3_frames::Rlp3Frame::Segmented {
            seq,
            last_seg,
            rexmit,
            s_seq,
            data,
            ..
        } => format!(
            "seg seq={} s_seq={} last_seg={} rexmit={} bytes={}",
            seq,
            s_seq,
            last_seg,
            rexmit,
            data.len()
        ),
        crate::rlp3_frames::Rlp3Frame::Fill { seq, .. } => format!("fill seq={}", seq),
        crate::rlp3_frames::Rlp3Frame::Idle1 { seq, .. } => format!("idle1 seq={}", seq),
        crate::rlp3_frames::Rlp3Frame::Idle2 { seq } => format!("idle2 seq={}", seq),
        crate::rlp3_frames::Rlp3Frame::Nak {
            seq,
            seq_hi,
            payload,
            ..
        } => {
            format!("nak seq={} seq_hi={} payload={:?}", seq, seq_hi, payload)
        }
    }
}

fn summarize_rlp3_nak_payload(payload: &crate::rlp3_frames::NakPayload) -> String {
    match payload {
        crate::rlp3_frames::NakPayload::Gap(entries) => {
            let ranges = entries
                .iter()
                .map(|entry| format!("{}-{}", entry.first, entry.last))
                .collect::<Vec<_>>()
                .join(",");
            format!("gap[{}]", ranges)
        }
        crate::rlp3_frames::NakPayload::Map(entries) => {
            let ranges = entries
                .iter()
                .map(|entry| format!("first={} bitmap=0x{:02x}", entry.nak_map_seq, entry.nak_map))
                .collect::<Vec<_>>()
                .join(",");
            format!("map[{}]", ranges)
        }
        crate::rlp3_frames::NakPayload::SegmentRange(entries) => {
            let ranges = entries
                .iter()
                .map(|entry| {
                    format!(
                        "seq={} s_seq={}-{}",
                        entry.frame_seq, entry.first_s_seq, entry.last_s_seq
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("segment_range[{}]", ranges)
        }
        crate::rlp3_frames::NakPayload::SegmentLength(entries) => {
            let ranges = entries
                .iter()
                .map(|entry| {
                    format!(
                        "seq={} s_seq={} len={}",
                        entry.frame_seq, entry.first_s_seq, entry.length_s_seq
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("segment_length[{}]", ranges)
        }
    }
}

/// Format a PPP packet for logging.
fn format_ppp_packet(ppp: &PppPacket) -> String {
    let proto_name = match ppp.protocol {
        0xC021 => "LCP",
        0x8021 => "IPCP",
        PPP_IP_PROTOCOL => "IP",
        PPP_VJ_COMPRESSED_TCP => "VJ-Compressed-TCP",
        PPP_VJ_UNCOMPRESSED_TCP => "VJ-Uncompressed-TCP",
        0xC023 => "PAP",
        0xC223 => "CHAP",
        other => return format!("proto=0x{:04X} len={}", other, ppp.payload.len()),
    };

    if ppp.protocol == PPP_IP_PROTOCOL {
        return format!("{} {}", proto_name, summarize_ipv4_packet(&ppp.payload));
    }
    if ppp.protocol == PPP_VJ_UNCOMPRESSED_TCP {
        let slot = ppp.payload.get(9).copied();
        let mut restored = ppp.payload.clone();
        if restored.len() > 9 {
            restored[9] = 6;
        }
        return format!(
            "{} slot={:?} {}",
            proto_name,
            slot,
            summarize_ipv4_packet(&restored)
        );
    }
    if ppp.protocol == PPP_VJ_COMPRESSED_TCP {
        return format!("{} {}", proto_name, summarize_vj_compressed(&ppp.payload));
    }

    // LCP/IPCP: show code, id, and payload
    if ppp.payload.len() >= 4 {
        let code = ppp.payload[0];
        let id = ppp.payload[1];
        let code_name = if ppp.protocol == 0xC021 {
            match code {
                1 => "Configure-Request",
                2 => "Configure-Ack",
                3 => "Configure-Nak",
                4 => "Configure-Reject",
                5 => "Terminate-Request",
                6 => "Terminate-Ack",
                9 => "Echo-Request",
                10 => "Echo-Reply",
                11 => "Discard-Request",
                _ => "Unknown",
            }
        } else {
            match code {
                1 => "Configure-Request",
                2 => "Configure-Ack",
                3 => "Configure-Nak",
                4 => "Configure-Reject",
                _ => "Unknown",
            }
        };
        format!(
            "{} {}(id={} len={} data={})",
            proto_name,
            code_name,
            id,
            ppp.payload.len(),
            hex_preview(&ppp.payload[4..], 32)
        )
    } else {
        format!(
            "{} len={} payload={}",
            proto_name,
            ppp.payload.len(),
            hex_preview(&ppp.payload, 32)
        )
    }
}

fn summarize_vj_compressed(payload: &[u8]) -> String {
    let Some(changes) = payload.first().copied() else {
        return "len=0".to_string();
    };
    let has_slot = changes & 0x40 != 0;
    let header_len = 1 + usize::from(has_slot) + 2;
    let slot = if has_slot {
        payload.get(1).copied()
    } else {
        None
    };
    format!(
        "changes=0x{:02x} slot={:?} len={} payload={}",
        changes,
        slot,
        payload.len(),
        hex_preview(payload.get(header_len..).unwrap_or(&[]), 24)
    )
}

fn summarize_ipv4_packet(packet: &[u8]) -> String {
    if packet.len() < 20 {
        return format!("len={} payload={}", packet.len(), hex_preview(packet, 20));
    }
    let version = packet[0] >> 4;
    if version != 4 {
        return format!(
            "v{} len={} payload={}",
            version,
            packet.len(),
            hex_preview(packet, 20)
        );
    }
    let ihl_bytes = usize::from(packet[0] & 0x0f) * 4;
    if packet.len() < ihl_bytes || ihl_bytes < 20 {
        return format!(
            "bad-ihl len={} ihl={} payload={}",
            packet.len(),
            ihl_bytes,
            hex_preview(packet, 20)
        );
    }
    let total_len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    let protocol = packet[9];
    let src = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let dst = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    match protocol {
        6 => summarize_tcp_segment(packet, ihl_bytes, src, dst, total_len),
        17 => summarize_udp_datagram(packet, ihl_bytes, src, dst, total_len),
        _ => format!(
            "{} -> {} proto={} len={}",
            src,
            dst,
            protocol,
            total_len.max(packet.len())
        ),
    }
}

fn summarize_udp_datagram(
    packet: &[u8],
    ihl_bytes: usize,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    total_len: usize,
) -> String {
    if packet.len() < ihl_bytes + 8 {
        return format!(
            "{} -> {} UDP truncated len={}",
            src,
            dst,
            total_len.max(packet.len())
        );
    }
    let src_port = u16::from_be_bytes([packet[ihl_bytes], packet[ihl_bytes + 1]]);
    let dst_port = u16::from_be_bytes([packet[ihl_bytes + 2], packet[ihl_bytes + 3]]);
    let udp_len = u16::from_be_bytes([packet[ihl_bytes + 4], packet[ihl_bytes + 5]]) as usize;
    format!(
        "{}:{} -> {}:{} UDP len={}",
        src,
        src_port,
        dst,
        dst_port,
        udp_len.max(total_len.saturating_sub(ihl_bytes))
    )
}

fn summarize_tcp_segment(
    packet: &[u8],
    ihl_bytes: usize,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    total_len: usize,
) -> String {
    if packet.len() < ihl_bytes + 20 {
        return format!(
            "{} -> {} TCP truncated len={}",
            src,
            dst,
            total_len.max(packet.len())
        );
    }
    let src_port = u16::from_be_bytes([packet[ihl_bytes], packet[ihl_bytes + 1]]);
    let dst_port = u16::from_be_bytes([packet[ihl_bytes + 2], packet[ihl_bytes + 3]]);
    let seq = u32::from_be_bytes([
        packet[ihl_bytes + 4],
        packet[ihl_bytes + 5],
        packet[ihl_bytes + 6],
        packet[ihl_bytes + 7],
    ]);
    let ack = u32::from_be_bytes([
        packet[ihl_bytes + 8],
        packet[ihl_bytes + 9],
        packet[ihl_bytes + 10],
        packet[ihl_bytes + 11],
    ]);
    let data_offset = usize::from(packet[ihl_bytes + 12] >> 4) * 4;
    let flags = packet[ihl_bytes + 13];
    let header_end = ihl_bytes.saturating_add(data_offset);
    let payload_len = total_len.saturating_sub(header_end);
    let ip_checksum = summarize_ipv4_header_checksum(packet, ihl_bytes);
    let tcp_checksum = summarize_tcp_checksum(packet, ihl_bytes, src, dst, total_len);
    format!(
        "{}:{} -> {}:{} TCP flags={} seq={} ack={} payload={} ip_csum={} tcp_csum={}",
        src,
        src_port,
        dst,
        dst_port,
        format_tcp_flags(flags),
        seq,
        ack,
        payload_len,
        ip_checksum,
        tcp_checksum
    )
}

fn summarize_ipv4_header_checksum(packet: &[u8], ihl_bytes: usize) -> String {
    if ihl_bytes < 20 || packet.len() < ihl_bytes {
        return "bad".to_string();
    }
    let received = u16::from_be_bytes([packet[10], packet[11]]);
    let mut header = packet[..ihl_bytes].to_vec();
    header[10] = 0;
    header[11] = 0;
    let computed = checksum16(&[&header]);
    if received == computed {
        "ok".to_string()
    } else {
        format!("bad(rx=0x{:04x} calc=0x{:04x})", received, computed)
    }
}

fn summarize_tcp_checksum(
    packet: &[u8],
    ihl_bytes: usize,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    total_len: usize,
) -> String {
    let segment_len = total_len
        .saturating_sub(ihl_bytes)
        .min(packet.len().saturating_sub(ihl_bytes));
    if segment_len < 20 || packet.len() < ihl_bytes + segment_len {
        return "bad".to_string();
    }
    let mut segment = packet[ihl_bytes..ihl_bytes + segment_len].to_vec();
    let received = u16::from_be_bytes([segment[16], segment[17]]);
    segment[16] = 0;
    segment[17] = 0;
    let pseudo = tcp_udp_pseudo_header(src, dst, 6, segment_len as u16);
    let computed = checksum16(&[&pseudo, &segment]);
    if received == computed {
        "ok".to_string()
    } else {
        format!("bad(rx=0x{:04x} calc=0x{:04x})", received, computed)
    }
}

fn tcp_udp_pseudo_header(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, length: u16) -> [u8; 12] {
    let mut pseudo = [0u8; 12];
    pseudo[..4].copy_from_slice(&src.octets());
    pseudo[4..8].copy_from_slice(&dst.octets());
    pseudo[8] = 0;
    pseudo[9] = protocol;
    pseudo[10..12].copy_from_slice(&length.to_be_bytes());
    pseudo
}

fn checksum16(parts: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    for part in parts {
        let mut i = 0usize;
        while i + 1 < part.len() {
            sum = sum.wrapping_add(u16::from_be_bytes([part[i], part[i + 1]]) as u32);
            i += 2;
        }
        if i < part.len() {
            sum = sum.wrapping_add(u16::from_be_bytes([part[i], 0]) as u32);
        }
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub(crate) fn format_tcp_flags(flags: u8) -> String {
    let mut out = String::new();
    if flags & 0x02 != 0 {
        out.push('S');
    }
    if flags & 0x10 != 0 {
        out.push('A');
    }
    if flags & 0x01 != 0 {
        out.push('F');
    }
    if flags & 0x04 != 0 {
        out.push('R');
    }
    if flags & 0x08 != 0 {
        out.push('P');
    }
    if out.is_empty() {
        out.push('-');
    }
    out
}

fn format_ppp_protocol(protocol: u16) -> &'static str {
    match protocol {
        0xC021 => "LCP",
        0x8021 => "IPCP",
        PPP_IP_PROTOCOL => "IP",
        PPP_VJ_COMPRESSED_TCP => "VJ-Compressed-TCP",
        PPP_VJ_UNCOMPRESSED_TCP => "VJ-Uncompressed-TCP",
        0xC023 => "PAP",
        0xC223 => "CHAP",
        _ => "UNKNOWN",
    }
}

fn format_rlp1_state(state: RlpState) -> String {
    match state {
        RlpState::Uninitialized => "uninitialized".to_string(),
        RlpState::Sync => "sync".to_string(),
        RlpState::SyncAck => "sync_ack".to_string(),
        RlpState::Ack => "ack".to_string(),
        RlpState::DataTransfer => "data_transfer".to_string(),
    }
}

fn format_rlp3_state(state: Rlp3State) -> String {
    match state {
        Rlp3State::Uninitialized => "uninitialized".to_string(),
        Rlp3State::Sync => "sync".to_string(),
        Rlp3State::SyncAck => "sync_ack".to_string(),
        Rlp3State::Ack => "ack".to_string(),
        Rlp3State::DataTransfer => "data_transfer".to_string(),
    }
}

fn format_lcp_state(state: crate::ppp::lcp::LcpState) -> String {
    match state {
        crate::ppp::lcp::LcpState::Closed => "closed".to_string(),
        crate::ppp::lcp::LcpState::RequestSent => "request_sent".to_string(),
        crate::ppp::lcp::LcpState::AckReceived => "ack_received".to_string(),
        crate::ppp::lcp::LcpState::AckSent => "ack_sent".to_string(),
        crate::ppp::lcp::LcpState::Opened => "opened".to_string(),
    }
}

fn format_ipcp_state(state: crate::ppp::ipcp::IpcpState) -> String {
    match state {
        crate::ppp::ipcp::IpcpState::Closed => "closed".to_string(),
        crate::ppp::ipcp::IpcpState::RequestSent => "request_sent".to_string(),
        crate::ppp::ipcp::IpcpState::AckReceived => "ack_received".to_string(),
        crate::ppp::ipcp::IpcpState::AckSent => "ack_sent".to_string(),
        crate::ppp::ipcp::IpcpState::Opened => "opened".to_string(),
    }
}

pub(crate) fn bytes_to_hex(data: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Format bytes as hex, truncating with "..." if longer than max_bytes.
fn hex_preview(data: &[u8], max_bytes: usize) -> String {
    if data.is_empty() {
        return "[]".to_string();
    }
    let show = data.len().min(max_bytes);
    let hex: String = data[..show]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("");
    if data.len() > max_bytes {
        format!("[{}...]", hex)
    } else {
        format!("[{}]", hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile_ip::{
        MobileIpConfig, UDP_PORT_MOBILE_IP, build_ipv4_packet, build_udp_packet,
    };
    use crate::ppp::framing::{self, PppPacket};
    use crate::ppp::ipcp;
    use crate::ppp::lcp;
    use crate::rlp;
    use cdma_common::consts::SERVICE_OPTION_PACKET_DATA;

    const TEST_MIP_RRQ_TYPE: u8 = 1;
    const TEST_MIP_RRQ_FLAGS_NONE: u8 = 0;
    const TEST_MIP_RRQ_LIFETIME_SECS: u16 = 600;
    const TEST_MIP_RRQ_IDENTIFICATION: u64 = 0x0102_0304_0506_0708;
    const TEST_MIP_MOBILE_HOME_AUTH_EXT: u8 = 32;
    const TEST_MIP_GENERALIZED_AUTH_EXT: u8 = 36;
    const TEST_MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA: u8 = 1;
    const TEST_MIP_NAI_EXT: u8 = 131;
    const TEST_MIP_NAI: &[u8] = b"ms@test";
    const TEST_IP_PROTO_UDP: u8 = 17;

    #[test]
    fn sch_0x0809_sdu_wraps_one_data_block_and_fill_muxpdu() {
        let data = vec![1u8; SCH_0X0809_DATA_BLOCK_BITS];
        let profile = Rc3FschProfile::default_19k2();
        let bits = build_sch_type3_sdu(&[data], profile);

        assert_eq!(bits.len(), profile.info_bits);
        assert_eq!(&bits[0..SCH_TYPE3_HEADER_BITS], &[0, 0, 1, 0, 0, 0]);
        let fill_start = SCH_TYPE3_HEADER_BITS + SCH_0X0809_DATA_BLOCK_BITS;
        assert_eq!(
            &bits[fill_start..fill_start + SCH_TYPE3_HEADER_BITS],
            &[1, 1, 1, 0, 0, 0]
        );
    }

    #[test]
    fn sch_0x0809_empty_sdu_uses_fill_muxpdu() {
        let profile = Rc3FschProfile::default_19k2();
        let bits = build_sch_type3_sdu(&[], profile);

        assert_eq!(bits.len(), profile.info_bits);
        assert_eq!(&bits[0..SCH_TYPE3_HEADER_BITS], &[1, 1, 1, 0, 0, 0]);
        assert!(bits[SCH_TYPE3_HEADER_BITS..].iter().all(|bit| *bit == 0));
    }

    #[test]
    fn sch_0x0809_sdu_wraps_two_data_blocks() {
        let first = vec![1u8; SCH_0X0809_DATA_BLOCK_BITS];
        let second = vec![0u8; SCH_0X0809_DATA_BLOCK_BITS];
        let profile = Rc3FschProfile::default_19k2();
        let bits = build_sch_type3_sdu(&[first, second], profile);

        assert_eq!(bits.len(), profile.info_bits);
        assert_eq!(&bits[0..SCH_TYPE3_HEADER_BITS], &[0, 0, 1, 0, 0, 0]);
        let second_start = SCH_TYPE3_HEADER_BITS + SCH_0X0809_DATA_BLOCK_BITS;
        assert_eq!(
            &bits[second_start..second_start + SCH_TYPE3_HEADER_BITS],
            &[0, 0, 1, 0, 0, 0]
        );
    }

    #[test]
    fn sch_0x0811_sdu_uses_two_ltu_crc_blocks() {
        let first = vec![1u8; SCH_0X0809_DATA_BLOCK_BITS];
        let second = vec![0u8; SCH_0X0809_DATA_BLOCK_BITS];
        let profile = Rc3FschProfile::from_rate_bps(38_400).unwrap();
        let bits = build_sch_type3_sdu(&[first, second], profile);

        assert_eq!(bits.len(), profile.info_bits);
        assert_eq!(bits.len(), 744);
        assert_eq!(&bits[0..SCH_TYPE3_HEADER_BITS], &[0, 0, 1, 0, 0, 0]);
        let second_start = SCH_TYPE3_HEADER_BITS + SCH_0X0809_DATA_BLOCK_BITS;
        assert_eq!(
            &bits[second_start..second_start + SCH_TYPE3_HEADER_BITS],
            &[0, 0, 1, 0, 0, 0]
        );
        assert_eq!(
            &bits[SCH_TYPE3_LTUS_SIZE_BITS..SCH_TYPE3_LTUS_SIZE_BITS + SCH_TYPE3_HEADER_BITS],
            &[1, 1, 1, 0, 0, 0]
        );
        assert_ltu_crc(&bits[0..SCH_TYPE3_LTUS_SIZE_BITS]);
        assert_ltu_crc(&bits[SCH_TYPE3_LTUS_SIZE_BITS..SCH_TYPE3_LTUS_SIZE_BITS * 2]);
    }

    #[test]
    fn sch_0x0821_sdu_uses_four_ltu_crc_blocks() {
        let data = vec![1u8; SCH_0X0809_DATA_BLOCK_BITS];
        let blocks = vec![data; 8];
        let profile = Rc3FschProfile::from_rate_bps(76_800).unwrap();
        let bits = build_sch_type3_sdu(&blocks, profile);

        assert_eq!(bits.len(), profile.info_bits);
        assert_eq!(bits.len(), 1512);
        for ltu_start in (0..SCH_TYPE3_LTUS_SIZE_BITS * 4).step_by(SCH_TYPE3_LTUS_SIZE_BITS) {
            let second_muxpdu = ltu_start + SCH_TYPE3_HEADER_BITS + SCH_0X0809_DATA_BLOCK_BITS;
            assert_eq!(
                &bits[second_muxpdu..second_muxpdu + SCH_TYPE3_HEADER_BITS],
                &[0, 0, 1, 0, 0, 0]
            );
        }
        for ltu in bits[..SCH_TYPE3_LTUS_SIZE_BITS * 4].chunks_exact(SCH_TYPE3_LTUS_SIZE_BITS) {
            assert_ltu_crc(ltu);
        }
    }

    #[test]
    fn sch_153k6_sdu_uses_eight_ltu_crc_blocks() {
        let data = vec![1u8; SCH_0X0921_DATA_BLOCK_BITS];
        let profile = Rc3FschProfile::from_rate_bps(153_600).unwrap();
        let bits = build_sch_type3_sdu(&[data], profile);

        assert_eq!(bits.len(), profile.info_bits);
        assert_eq!(bits.len(), 3048);
        assert_eq!(&bits[0..SCH_TYPE3_HEADER_BITS], &[0, 0, 1, 0, 0, 0]);
        assert_eq!(
            &bits[SCH_TYPE3_HEADER_BITS..SCH_TYPE3_HEADER_BITS + SCH_0X0921_DATA_BLOCK_BITS],
            &[1u8; SCH_0X0921_DATA_BLOCK_BITS]
        );
        for ltu in bits[..SCH_TYPE3_LTUS_SIZE_BITS * 8].chunks_exact(SCH_TYPE3_LTUS_SIZE_BITS) {
            assert_ltu_crc(ltu);
        }
    }

    fn assert_ltu_crc(ltu: &[u8]) {
        assert_eq!(ltu.len(), SCH_TYPE3_LTUS_SIZE_BITS);
        let expected = crc16_sch(&ltu[..SCH_TYPE3_LTUS_PAYLOAD_BITS]);
        let actual = bits_to_u16(&ltu[SCH_TYPE3_LTUS_PAYLOAD_BITS..]);
        assert_eq!(actual, expected);
    }

    fn bits_to_u16(bits: &[u8]) -> u16 {
        bits.iter()
            .fold(0u16, |acc, bit| (acc << 1) | ((*bit as u16) & 1))
    }

    /// Helper: encode an RLP Type 1 frame to raw bits for feeding into tick().
    fn encode_rlp1(frame: &rlp::RlpFrame) -> (Vec<u8>, u32) {
        let rate_bps = if frame.is_idle() { 1200u32 } else { 9600 };
        let rlp_rate = if frame.is_idle() {
            rlp::RlpRate::Eighth
        } else {
            rlp::RlpRate::Full
        };
        (
            rlp::encode_frame(frame, rlp_rate).expect("test RLP1 frame must encode"),
            rate_bps,
        )
    }

    fn encode_rlp3(frame: &crate::rlp3_frames::Rlp3Frame) -> (Vec<u8>, u32) {
        (
            frame
                .encode(crate::rlp3_frames::MuxOption::Odd)
                .expect("test RLP3 frame must encode"),
            9600,
        )
    }

    /// Helper: drive the BS through the RLP SYNC handshake with a simulated mobile.
    fn complete_rlp_handshake(bs: &mut PacketSession) {
        // Tick 1: BS sends SYNC (auto-initializes).
        bs.tick(None);

        // Mobile sends SYNC/ACK.
        let sync_ack = rlp::sync_ack_frame(0);
        let (bits, rate) = encode_rlp1(&sync_ack);
        bs.tick(Some((&bits, rate)));

        // Tick through the ACK frames (need >= round_trip_counter).
        for _ in 0..6 {
            let idle = rlp::idle_frame(0);
            let (bits, rate) = encode_rlp1(&idle);
            bs.tick(Some((&bits, rate)));
        }

        assert_eq!(bs.phase(), SessionPhase::Lcp);
    }

    fn complete_rlp3_handshake(bs: &mut PacketSession) {
        bs.tick(None);

        let sync_ack = crate::rlp3_frames::Rlp3Frame::Control {
            seq: 0,
            control_type: crate::rlp3_frames::Rlp3ControlType::SyncAck,
            init_var: false,
            nak_param_incl: false,
        };
        let (bits, rate) = encode_rlp3(&sync_ack);
        bs.tick(Some((&bits, rate)));

        for _ in 0..6 {
            bs.tick(None);
        }

        assert_eq!(bs.phase(), SessionPhase::Lcp);
    }

    /// Build LCP Configure-Request from mobile.
    fn mobile_lcp_configure_request(id: u8) -> PppPacket {
        let lcp = lcp::LcpPacket {
            code: 1, // Configure-Request
            identifier: id,
            data: vec![], // no options
        };
        lcp.to_ppp()
    }

    /// Build LCP Configure-Ack from mobile.
    fn mobile_lcp_configure_ack(id: u8, data: Vec<u8>) -> PppPacket {
        let lcp = lcp::LcpPacket {
            code: 2, // Configure-Ack
            identifier: id,
            data,
        };
        lcp.to_ppp()
    }

    /// Option bytes of the session's pending LCP Configure-Request. The
    /// Magic-Number is per-session, so a simulated peer Configure-Ack must
    /// echo the live request rather than a fixed byte string.
    fn our_lcp_request_data(session: &PacketSession) -> Vec<u8> {
        session.lcp.pending_request_data()
    }

    /// Build IPCP Configure-Request from mobile (requesting 0.0.0.0).
    fn mobile_ipcp_request_zero(id: u8) -> PppPacket {
        let ipcp = ipcp::IpcpPacket {
            code: 1,
            identifier: id,
            data: vec![3, 6, 0, 0, 0, 0], // IP-Address option, 0.0.0.0
        };
        ipcp.to_ppp()
    }

    /// Build IPCP Configure-Request from mobile with specific IP.
    fn mobile_ipcp_request_ip(id: u8, ip: Ipv4Addr) -> PppPacket {
        let octets = ip.octets();
        let ipcp = ipcp::IpcpPacket {
            code: 1,
            identifier: id,
            data: vec![3, 6, octets[0], octets[1], octets[2], octets[3]],
        };
        ipcp.to_ppp()
    }

    fn mobile_ipcp_request_dns_only(id: u8, dns: Ipv4Addr) -> PppPacket {
        let dns = dns.octets();
        let ipcp = ipcp::IpcpPacket {
            code: 1,
            identifier: id,
            data: vec![
                129, 6, dns[0], dns[1], dns[2], dns[3], 131, 6, dns[0], dns[1], dns[2], dns[3],
            ],
        };
        ipcp.to_ppp()
    }

    fn feed_a10_ppp(session: &mut PacketSession, packet: &PppPacket) -> Vec<SessionAction> {
        let hdlc = framing::frame(packet);
        session.tick(Some((&hdlc, 0)))
    }

    fn mobile_ipcp_request_ip_with_vj(id: u8, ip: Ipv4Addr) -> PppPacket {
        let octets = ip.octets();
        let ipcp = ipcp::IpcpPacket {
            code: 1,
            identifier: id,
            data: vec![
                3, 6, octets[0], octets[1], octets[2], octets[3], 2, 6, 0x00, 0x2d, 0x0f, 0x01,
            ],
        };
        ipcp.to_ppp()
    }

    /// Build IPCP Configure-Ack from mobile.
    fn mobile_ipcp_ack(id: u8, data: Vec<u8>) -> PppPacket {
        let ipcp = ipcp::IpcpPacket {
            code: 2,
            identifier: id,
            data,
        };
        ipcp.to_ppp()
    }

    fn our_default_ipcp_request_data() -> Vec<u8> {
        vec![3, 6, 10, 0, 0, 1] // IP-Address 10.0.0.1
    }

    fn our_vj_ipcp_request_data() -> Vec<u8> {
        vec![
            3, 6, 10, 0, 0, 1, // IP-Address 10.0.0.1
            2, 6, 0x00, 0x2d, 0x0f,
            0x01, // VJ compressed TCP/IP, 16 slots, slot id compression
        ]
    }

    fn ipcp_config_with_local_vj() -> IpcpConfig {
        IpcpConfig {
            request_vj: true,
            ..IpcpConfig::default()
        }
    }

    fn mobile_ipv4_packet(src: Ipv4Addr, dst: Ipv4Addr) -> Vec<u8> {
        let src = src.octets();
        let dst = dst.octets();
        vec![
            0x45, 0x00, 0x00, 0x1C, 0x00, 0x01, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00, src[0], src[1],
            src[2], src[3], dst[0], dst[1], dst[2], dst[3], 0xC0, 0x00, 0x00, 0x35, 0x00, 0x08,
            0x00, 0x00,
        ]
    }

    fn mobile_tcp_packet(src: Ipv4Addr, dst: Ipv4Addr, seq_no: u32, ip_id: u16) -> Vec<u8> {
        let src = src.octets();
        let dst = dst.octets();
        let mut packet = vec![0u8; 41];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&41u16.to_be_bytes());
        packet[4..6].copy_from_slice(&ip_id.to_be_bytes());
        packet[8] = 64;
        packet[9] = 6;
        packet[12..16].copy_from_slice(&src);
        packet[16..20].copy_from_slice(&dst);
        packet[20..22].copy_from_slice(&1234u16.to_be_bytes());
        packet[22..24].copy_from_slice(&80u16.to_be_bytes());
        packet[24..28].copy_from_slice(&seq_no.to_be_bytes());
        packet[28..32].copy_from_slice(&1u32.to_be_bytes());
        packet[32] = 5 << 4;
        packet[33] = 0x10;
        packet[34..36].copy_from_slice(&4096u16.to_be_bytes());
        packet[36..38].copy_from_slice(&0x1234u16.to_be_bytes());
        packet[40] = b'x';
        let sum = checksum16(&[&packet[..20]]);
        packet[10..12].copy_from_slice(&sum.to_be_bytes());
        packet
    }

    fn mobile_mip_rrq_packet(src: Ipv4Addr, fa: Ipv4Addr) -> Vec<u8> {
        mobile_mip_rrq_packet_with_extensions(src, fa, &[])
    }

    fn mobile_mip_rrq_packet_with_extensions(
        src: Ipv4Addr,
        fa: Ipv4Addr,
        extensions: &[u8],
    ) -> Vec<u8> {
        let mut rrq = Vec::new();
        rrq.push(TEST_MIP_RRQ_TYPE);
        rrq.push(TEST_MIP_RRQ_FLAGS_NONE);
        rrq.extend_from_slice(&TEST_MIP_RRQ_LIFETIME_SECS.to_be_bytes());
        rrq.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        rrq.extend_from_slice(&Ipv4Addr::BROADCAST.octets());
        rrq.extend_from_slice(&fa.octets());
        rrq.extend_from_slice(&TEST_MIP_RRQ_IDENTIFICATION.to_be_bytes());
        rrq.extend_from_slice(&[TEST_MIP_NAI_EXT, TEST_MIP_NAI.len() as u8]);
        rrq.extend_from_slice(TEST_MIP_NAI);
        rrq.extend_from_slice(extensions);
        let udp = build_udp_packet(UDP_PORT_MOBILE_IP, UDP_PORT_MOBILE_IP, &rrq);
        build_ipv4_packet(src, fa, TEST_IP_PROTO_UDP, &udp)
    }

    fn mobile_mip_rrq_auth_extensions() -> Vec<u8> {
        let mut out = Vec::new();
        out.push(TEST_MIP_MOBILE_HOME_AUTH_EXT);
        out.push(20);
        out.extend_from_slice(&300u32.to_be_bytes());
        out.extend_from_slice(&[0x11; 16]);
        out.push(TEST_MIP_GENERALIZED_AUTH_EXT);
        out.push(TEST_MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA);
        out.extend_from_slice(&20u16.to_be_bytes());
        out.extend_from_slice(&2u32.to_be_bytes());
        out.extend_from_slice(&[0x22; 16]);
        out
    }

    /// Feed a PPP packet to the session via RLP data frames.
    /// Splits the HDLC bytes across multiple RLP frames if needed.
    /// Tracks the uplink SEQ number to maintain proper RLP sequencing.
    fn feed_ppp_via_rlp(
        session: &mut PacketSession,
        ppp: &PppPacket,
        seq: &mut u8,
    ) -> Vec<SessionAction> {
        let hdlc = framing::frame(ppp);
        let mut all_actions = Vec::new();

        // Split into 19-byte chunks (Format A max payload).
        for chunk in hdlc.chunks(19) {
            let frame = rlp::data_frame(*seq, chunk);
            *seq = seq.wrapping_add(1);
            let (bits, rate) = encode_rlp1(&frame);
            let actions = session.tick(Some((&bits, rate)));
            all_actions.extend(actions);
        }

        all_actions
    }

    fn hrpd_rlp_seq(bits: &[u8]) -> u32 {
        bits[..DEFAULT_PACKET_RLP_SEQUENCE_BITS]
            .iter()
            .fold(0u32, |acc, bit| (acc << 1) | u32::from(bit & 1))
    }

    #[test]
    fn a10_unstructured_byte_stream_session_starts_lcp_before_uplink() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        assert_eq!(session.phase(), SessionPhase::RlpSync);

        let actions = session.tick(None);
        assert_eq!(session.phase(), SessionPhase::Lcp);
        let octets = actions
            .iter()
            .find_map(|action| match action {
                SessionAction::SendFrame { bits, rate_bps } => Some((bits, *rate_bps)),
                _ => None,
            })
            .expect("A10 byte-stream session should originate LCP before uplink");
        assert_eq!(octets.1, 0);
        assert!(octets.0.starts_with(&[0x7e]));
        assert!(octets.0.ends_with(&[0x7e]));
    }

    #[test]
    fn a10_unstructured_byte_stream_backend_preserves_octets() {
        let payload = [0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e];
        let mut backend = UnstructuredByteStreamBackend::new();

        let delivered = backend
            .receive_frame_bits(&payload, 0)
            .expect("non-empty byte-stream payload should deliver octets");

        assert_eq!(delivered, payload);
        assert_eq!(
            backend.last_rx_control,
            "A10 UnstructuredByteStream octets=6"
        );
    }

    #[test]
    fn a10_uplink_burst_is_ingested_without_packet_session_ticks() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.phase = SessionPhase::Active;
        let ip_packet = mobile_ipv4_packet(Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(8, 8, 8, 8));
        let hdlc = framing::frame(&PppPacket {
            protocol: PPP_IP_PROTOCOL,
            payload: ip_packet.clone(),
        });

        let delivered = (0..256)
            .flat_map(|_| session.ingest_a10_byte_stream(&hdlc))
            .filter(|action| {
                matches!(action, SessionAction::DeliverIpPacket(packet) if packet == &ip_packet)
            })
            .count();

        assert_eq!(delivered, 256);
        assert_eq!(session.telemetry().lcp_configure_restarts, 0);
    }

    #[test]
    fn a10_lcp_peer_ack_without_peer_request_advances_to_ipcp_after_retransmits() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.tick(None);

        let peer_ack = mobile_lcp_configure_ack(1, our_lcp_request_data(&session));
        feed_a10_ppp(&mut session, &peer_ack);

        assert_eq!(
            session.phase(),
            SessionPhase::Lcp,
            "a one-sided peer ACK alone must not force LCP open"
        );

        // Drive the restart timer through one retransmit of our
        // Configure-Request: still not enough evidence of a stuck peer.
        let mut actions = Vec::new();
        for _ in 0..=lcp::CONFIGURE_RESTART_TICKS {
            actions.extend(session.tick(None));
        }
        assert_eq!(
            session.phase(),
            SessionPhase::Lcp,
            "one retransmit is not yet evidence of a stuck peer"
        );

        // Second retransmit with the peer ACK in hand forces LCP open.
        for _ in 0..=lcp::CONFIGURE_RESTART_TICKS {
            actions.extend(session.tick(None));
            if session.phase() == SessionPhase::Ipcp {
                break;
            }
        }
        actions.extend(session.tick(None));
        assert_eq!(session.phase(), SessionPhase::Ipcp);

        let mut deframer = framing::HdlcDeframer::new();
        let downlink_ppp = actions
            .iter()
            .filter_map(|action| match action {
                SessionAction::SendFrame { bits, .. } => Some(bits.as_slice()),
                _ => None,
            })
            .flat_map(|bits| deframer.feed(bits))
            .collect::<Vec<_>>();

        assert!(
            downlink_ppp
                .iter()
                .any(|ppp| ppp.protocol == IPCP_PROTOCOL && ppp.payload.first() == Some(&1)),
            "A10 should send IPCP once the one-sided peer ACK is confirmed by retransmits"
        );
    }

    #[test]
    fn a10_lcp_peer_request_without_peer_ack_waits_for_peer_ipcp() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.tick(None);

        let peer_request = mobile_lcp_configure_request(139);
        let actions = feed_a10_ppp(&mut session, &peer_request);

        assert_eq!(
            session.phase(),
            SessionPhase::Lcp,
            "ACKing the peer request without a peer ACK of ours must not open LCP"
        );

        let mut deframer = framing::HdlcDeframer::new();
        let downlink_ppp = actions
            .iter()
            .filter_map(|action| match action {
                SessionAction::SendFrame { bits, .. } => Some(bits.as_slice()),
                _ => None,
            })
            .flat_map(|bits| deframer.feed(bits))
            .collect::<Vec<_>>();
        assert!(
            downlink_ppp
                .iter()
                .any(|ppp| ppp.protocol == LCP_PROTOCOL && ppp.payload.first() == Some(&2)),
            "A10 should ACK the peer LCP request"
        );
        assert!(
            !downlink_ppp.iter().any(|ppp| ppp.protocol == IPCP_PROTOCOL),
            "A10 must not start IPCP on the one-sided LCP exchange alone"
        );

        // The peer proceeding to IPCP on its own is the force-open evidence.
        feed_a10_ppp(&mut session, &mobile_ipcp_request_zero(1));
        assert_eq!(session.phase(), SessionPhase::Ipcp);
    }

    #[test]
    fn a10_simple_ip_dns_only_peer_request_opens_active_after_max_failure() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.tick(None);

        let our_lcp_data = our_lcp_request_data(&session);
        feed_a10_ppp(&mut session, &mobile_lcp_configure_request(1));
        feed_a10_ppp(&mut session, &mobile_lcp_configure_ack(1, our_lcp_data));
        assert_eq!(session.phase(), SessionPhase::Ipcp);

        feed_a10_ppp(
            &mut session,
            &mobile_ipcp_ack(1, our_default_ipcp_request_data()),
        );
        assert_eq!(session.phase(), SessionPhase::Ipcp);

        for id in 1..=5 {
            feed_a10_ppp(
                &mut session,
                &mobile_ipcp_request_dns_only(id, Ipv4Addr::new(10, 55, 0, 1)),
            );
            assert_eq!(session.phase(), SessionPhase::Ipcp);
        }

        feed_a10_ppp(
            &mut session,
            &mobile_ipcp_request_dns_only(6, Ipv4Addr::new(10, 55, 0, 1)),
        );

        assert_eq!(session.phase(), SessionPhase::Active);
        assert_eq!(session.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(session.our_ip(), Ipv4Addr::new(10, 0, 0, 1));
    }

    #[test]
    fn a10_peer_lcp_terminate_sets_one_shot_signal() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.tick(None);

        let terminate = lcp::LcpPacket {
            code: lcp::TERMINATE_REQUEST,
            identifier: 7,
            data: vec![],
        }
        .to_ppp();
        let hdlc = framing::frame(&terminate);
        let actions = session.tick(Some((&hdlc, 0)));

        assert!(session.take_peer_requested_lcp_terminate());
        assert!(!session.take_peer_requested_lcp_terminate());

        let mut deframer = framing::HdlcDeframer::new();
        let downlink_ppp = actions
            .iter()
            .filter_map(|action| match action {
                SessionAction::SendFrame { bits, .. } => Some(bits.as_slice()),
                _ => None,
            })
            .flat_map(|bits| deframer.feed(bits))
            .collect::<Vec<_>>();
        assert!(
            downlink_ppp
                .iter()
                .any(|ppp| ppp.protocol == LCP_PROTOCOL && ppp.payload.first() == Some(&6)),
            "LCP Terminate-Request must still be acknowledged before HRPD task teardown"
        );
    }

    #[test]
    fn a10_unstructured_byte_stream_tick_drains_backlog_in_multiple_chunks() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.phase = SessionPhase::Active;
        session.rlp.enqueue_data(&vec![0xaa; 2500]);

        let actions = session.tick(None);
        let frames = actions
            .iter()
            .filter_map(|action| match action {
                SessionAction::SendFrame { bits, rate_bps } => Some((bits.len(), *rate_bps)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(frames, vec![(1024, 0), (1024, 0), (452, 0)]);
        assert_eq!(session.rlp.tx_queue_len(), 0);
    }

    #[test]
    fn a10_unstructured_byte_stream_coalesces_partial_arrival_chunks() {
        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        session.phase = SessionPhase::Active;
        session.rlp_sync_ticks = 17;
        let ip_packet = vec![0x45; 700];

        session.send_ip_packet(&ip_packet);
        let first_actions = session.emit_full_pending_downlink();
        assert!(first_actions.is_empty());
        assert_eq!(session.rlp_sync_ticks, 17);

        session.send_ip_packet(&ip_packet);
        let actions = session.emit_full_pending_downlink();

        assert_eq!(session.rlp_sync_ticks, 17);
        assert_eq!(actions.len(), 1);
        assert!(session.rlp.tx_queue_len() < A10_UNSTRUCTURED_BYTE_STREAM_MAX_OCTETS);
        let mut deframer = framing::HdlcDeframer::new();
        let mut packets = actions
            .iter()
            .filter_map(|action| match action {
                SessionAction::SendFrame { bits, rate_bps } => {
                    assert_eq!(*rate_bps, 0);
                    assert!(bits.len() <= A10_UNSTRUCTURED_BYTE_STREAM_MAX_OCTETS);
                    Some(bits.as_slice())
                }
                _ => None,
            })
            .flat_map(|octets| deframer.feed(octets))
            .collect::<Vec<_>>();
        assert_eq!(packets.len(), 1);

        let flush_actions = session.tick(None);
        packets.extend(
            flush_actions
                .iter()
                .filter_map(|action| match action {
                    SessionAction::SendFrame { bits, .. } => Some(bits.as_slice()),
                    _ => None,
                })
                .flat_map(|octets| deframer.feed(octets)),
        );
        assert_eq!(packets.len(), 2);
        assert!(
            packets
                .iter()
                .all(|packet| packet.protocol == PPP_IP_PROTOCOL && packet.payload == ip_packet)
        );
    }

    #[test]
    fn hrpd_default_packet_session_emits_lcp_without_rlp_sync() {
        let mut session = PacketSession::new_hrpd_default_packet(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
            None,
        );
        assert_eq!(session.phase(), SessionPhase::RlpSync);

        let actions = session.tick(None);

        assert_eq!(session.phase(), SessionPhase::Lcp);
        let frame = actions
            .iter()
            .find_map(|action| match action {
                SessionAction::SendFrame { bits, rate_bps } => Some((bits, *rate_bps)),
                _ => None,
            })
            .expect("HRPD Default Packet session should send LCP as first downlink RLP packet");
        assert_eq!(frame.1, 0);
        assert_eq!(frame.0.len() % 8, 6);
        assert_eq!(hrpd_rlp_seq(frame.0), 0);
        assert!(frame.0.len() > DEFAULT_PACKET_RLP_SEQUENCE_BITS);
    }

    #[test]
    fn hrpd_default_packet_backend_extracts_uplink_octets() {
        let payload = [0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e];
        let bits = default_packet_rlp_packet_bits(3, &payload);
        let mut backend = HrpdDefaultPacketBackend::new();

        let delivered = backend
            .receive_frame_bits(&bits, 0)
            .expect("valid HRPD RLP packet should deliver octets");

        assert_eq!(delivered, payload);
        assert_eq!(backend.rx_last_seq, Some(3));
        assert_eq!(backend.last_rx_control, "HRPD DefaultPacket seq=3 bits=70");
    }

    #[test]
    fn hrpd_default_packet_backend_fragments_for_1024_bit_forward_traffic() {
        let mut backend = HrpdDefaultPacketBackend::new();
        backend.enqueue_data(&[0xaa; 200]);

        let (first, first_rate) = backend.next_frame_bits(true);
        let (second, second_rate) = backend.next_frame_bits(true);

        assert_eq!(first_rate, 0);
        assert_eq!(second_rate, 0);
        assert_eq!(
            first.len(),
            DEFAULT_PACKET_RLP_SEQUENCE_BITS + HRPD_DEFAULT_PACKET_MAX_OCTETS_1024_FTC * 8
        );
        assert_eq!(second.len(), DEFAULT_PACKET_RLP_SEQUENCE_BITS + 79 * 8);
        assert_eq!(hrpd_rlp_seq(&first), 0);
        assert_eq!(hrpd_rlp_seq(&second), 1);
        assert_eq!(backend.tx_queue_len(), 0);
    }

    #[test]
    fn session_starts_in_rlp_sync() {
        let session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        assert_eq!(session.phase(), SessionPhase::RlpSync);
    }

    #[test]
    fn rlp_sync_timeout_closes_session_when_ms_never_engages() {
        // Simulate the w14 pathology: BTS RX delivers signaling but the
        // MS never sends RLP3 SYNC/ACK. After RLP_SYNC_MAX_TICKS ticks
        // the engine must emit CloseSession with an RLP3 SYNC timeout
        // reason.
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        assert_eq!(session.phase(), SessionPhase::RlpSync);

        // Tick up to (but not including) the threshold — no close yet.
        for _ in 0..(RLP_SYNC_MAX_TICKS - 1) {
            let actions = session.tick(None);
            assert!(
                actions
                    .iter()
                    .all(|a| !matches!(a, SessionAction::CloseSession { .. }))
            );
        }
        // One more tick should trip the timeout.
        let actions = session.tick(None);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, SessionAction::CloseSession { reason }
                    if reason.contains("RLP3 SYNC timeout")))
        );
    }

    #[test]
    fn rlp_sync_timer_resets_after_handshake_completes() {
        // Ticks accumulated in RlpSync do not arm a close once RLP
        // has transitioned to DataTransfer / Lcp.
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        assert_eq!(session.phase(), SessionPhase::Lcp);
        // Tick well past the timeout threshold — no close.
        for _ in 0..(RLP_SYNC_MAX_TICKS + 10) {
            let actions = session.tick(None);
            assert!(
                actions
                    .iter()
                    .all(|a| !matches!(a, SessionAction::CloseSession { .. }))
            );
        }
    }

    #[test]
    fn rlp_handshake_transitions_to_lcp() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        assert_eq!(session.phase(), SessionPhase::Lcp);
    }

    #[test]
    fn full_negotiation_to_active() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        assert_eq!(session.phase(), SessionPhase::Lcp);

        let mut seq: u8 = 0;

        // Mobile sends LCP Configure-Request.
        let mobile_lcp_req = mobile_lcp_configure_request(1);
        let actions = feed_ppp_via_rlp(&mut session, &mobile_lcp_req, &mut seq);
        // BS should respond with Configure-Ack (carried in the downlink RLP frame).
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, SessionAction::SendFrame { .. }))
        );

        // Mobile acks our Configure-Request.
        let mobile_lcp_ack = mobile_lcp_configure_ack(1, our_lcp_request_data(&session));
        feed_ppp_via_rlp(&mut session, &mobile_lcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Ipcp);

        // Mobile sends IPCP Configure-Request with IP=0.0.0.0.
        let mobile_ipcp_req = mobile_ipcp_request_zero(1);
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_req, &mut seq);

        // Mobile retries with correct IP.
        let mobile_ipcp_req2 = mobile_ipcp_request_ip(2, Ipv4Addr::new(10, 0, 0, 2));
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_req2, &mut seq);

        // Mobile acks our IPCP request (ID=1).
        let our_ipcp_data = our_default_ipcp_request_data();
        let mobile_ipcp_ack = mobile_ipcp_ack(1, our_ipcp_data);
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Active);
        assert_eq!(session.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(session.our_ip(), Ipv4Addr::new(10, 0, 0, 1));
    }

    #[test]
    fn mobile_ip_mode_waits_for_registration_after_ipcp_without_peer_ip() {
        let config = IpcpConfig {
            mobile_ip: MobileIpConfig {
                enabled: true,
                fa_address: Ipv4Addr::new(10, 0, 0, 1),
                home_agent_address: Ipv4Addr::new(10, 0, 0, 1),
                ..MobileIpConfig::default()
            },
            ..IpcpConfig::default()
        };
        let mut session = PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), config);
        complete_rlp_handshake(&mut session);

        let mut seq: u8 = 0;
        feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);
        let our_lcp_data = our_lcp_request_data(&session);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_lcp_configure_ack(1, our_lcp_data),
            &mut seq,
        );
        assert_eq!(session.phase(), SessionPhase::Ipcp);

        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_dns_only(1, Ipv4Addr::UNSPECIFIED),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_dns_only(2, Ipv4Addr::new(10, 55, 0, 1)),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_ack(1, our_default_ipcp_request_data()),
            &mut seq,
        );
        assert_eq!(session.phase(), SessionPhase::Mip4Pending);

        let rrq = mobile_mip_rrq_packet(Ipv4Addr::UNSPECIFIED, Ipv4Addr::new(10, 0, 0, 1));
        let ppp = PppPacket {
            protocol: PPP_IP_PROTOCOL,
            payload: rrq,
        };
        let actions = feed_ppp_via_rlp(&mut session, &ppp, &mut seq);
        assert_eq!(session.phase(), SessionPhase::Active);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::DeliverIpPacket(_))),
            "MIP4 Registration Request must not be forwarded as user IP"
        );

        let ip_packet = mobile_ipv4_packet(Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(8, 8, 8, 8));
        let ppp = PppPacket {
            protocol: PPP_IP_PROTOCOL,
            payload: ip_packet.clone(),
        };
        let actions = feed_ppp_via_rlp(&mut session, &ppp, &mut seq);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, SessionAction::DeliverIpPacket(data) if data == &ip_packet))
        );
    }

    #[test]
    fn authenticated_mobile_ip_rrq_falls_back_to_simple_ip() {
        let config = IpcpConfig {
            mobile_ip: MobileIpConfig {
                enabled: true,
                ..MobileIpConfig::default()
            },
            ..IpcpConfig::default()
        };
        let mut session = PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), config);
        complete_rlp_handshake(&mut session);

        let mut seq: u8 = 0;
        feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);
        let our_lcp_data = our_lcp_request_data(&session);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_lcp_configure_ack(1, our_lcp_data),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_dns_only(1, Ipv4Addr::UNSPECIFIED),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_dns_only(2, Ipv4Addr::new(10, 55, 0, 1)),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_ack(1, our_default_ipcp_request_data()),
            &mut seq,
        );
        assert_eq!(session.phase(), SessionPhase::Mip4Pending);

        let rrq = mobile_mip_rrq_packet_with_extensions(
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::new(10, 0, 0, 1),
            &mobile_mip_rrq_auth_extensions(),
        );
        let ppp = PppPacket {
            protocol: PPP_IP_PROTOCOL,
            payload: rrq,
        };
        let actions = feed_ppp_via_rlp(&mut session, &ppp, &mut seq);
        assert_eq!(session.phase(), SessionPhase::Ipcp);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::DeliverIpPacket(_))),
            "Authenticated MIP4 RRQ must not be forwarded as user IP"
        );

        feed_ppp_via_rlp(&mut session, &mobile_ipcp_request_zero(3), &mut seq);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_ip(4, Ipv4Addr::new(10, 0, 0, 2)),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_ack(2, our_default_ipcp_request_data()),
            &mut seq,
        );
        assert_eq!(session.phase(), SessionPhase::Active);
    }

    #[test]
    fn cached_ppp_state_resumes_to_active_after_fresh_rlp_sync() {
        let mut first =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        drive_to_active(&mut first);
        let snapshot = first
            .snapshot_ppp_state()
            .expect("active PPP session should snapshot");

        let mut resumed = PacketSession::new_with_ppp_resume(
            u32::from(SERVICE_OPTION_PACKET_DATA),
            snapshot.ipcp.config.clone(),
            Some(snapshot),
        );

        resumed.tick(None);
        let sync_ack = rlp::sync_ack_frame(0);
        let (bits, rate) = encode_rlp1(&sync_ack);
        resumed.tick(Some((&bits, rate)));
        for _ in 0..6 {
            let idle = rlp::idle_frame(0);
            let (bits, rate) = encode_rlp1(&idle);
            resumed.tick(Some((&bits, rate)));
        }

        assert_eq!(resumed.phase(), SessionPhase::Active);
        assert_eq!(resumed.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));

        let mut seq = 0;
        let ip_packet = mobile_ipv4_packet(Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(8, 8, 8, 8));
        let ppp = PppPacket {
            protocol: 0x0021,
            payload: ip_packet.clone(),
        };
        let actions = feed_ppp_via_rlp(&mut resumed, &ppp, &mut seq);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, SessionAction::DeliverIpPacket(data) if data == &ip_packet))
        );
    }

    #[test]
    fn lcp_restart_after_active_renegotiates_without_closing_session() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        let mut seq = drive_to_active(&mut session);
        assert_eq!(session.phase(), SessionPhase::Active);

        let changed_lcp_req = lcp::LcpPacket {
            code: 1,
            identifier: 9,
            data: vec![1, 4, 0x05, 0xDC],
        }
        .to_ppp();
        let actions = feed_ppp_via_rlp(&mut session, &changed_lcp_req, &mut seq);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::CloseSession { .. })),
            "LCP restart should renegotiate PPP in-place"
        );
        assert_eq!(session.phase(), SessionPhase::Lcp);

        let mobile_lcp_ack = mobile_lcp_configure_ack(2, our_lcp_request_data(&session));
        feed_ppp_via_rlp(&mut session, &mobile_lcp_ack, &mut seq);
        assert_eq!(session.phase(), SessionPhase::Ipcp);

        let ip_packet = vec![
            0x45, 0x00, 0x00, 0x1C, 0x00, 0x01, 0x00, 0x00, 0x40, 0x01, 0x00, 0x00, 0x0A, 0x00,
            0x00, 0x01, 0x0A, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01,
        ];
        session.send_ip_packet(&ip_packet);
        assert_eq!(session.pending_downlink_ip.len(), 1);

        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_ip(3, Ipv4Addr::new(10, 0, 0, 2)),
            &mut seq,
        );
        let actions = feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_ack(1, our_default_ipcp_request_data()),
            &mut seq,
        );

        assert_eq!(session.phase(), SessionPhase::Active);
        assert!(session.pending_downlink_ip.is_empty());
        assert!(
            actions.iter().any(
                |a| matches!(a, SessionAction::SendFrame { rate_bps, .. } if *rate_bps > 1200)
            ),
            "downlink queued during LCP restart should flush after IPCP reopens"
        );
    }

    #[test]
    fn sch_dtxes_control_frames_until_ppp_active() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        session.set_sch_active(true);

        let mut seq = 0;
        let actions = feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);

        assert_eq!(session.phase(), SessionPhase::Lcp);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, SessionAction::SendFrame { .. })),
            "LCP control should still go out on FCH"
        );
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::SendSchFrame { .. })),
            "SCH should remain DTX/blank before PPP is active"
        );
    }

    #[test]
    fn active_sch_emits_fill_muxpdu_when_rlp_queue_empty() {
        let mut session = PacketSession::new(
            u32::from(SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
            IpcpConfig::default(),
        );
        complete_rlp3_handshake(&mut session);

        // Drive PPP through LCP + IPCP to Active via the real handshake.
        let mut seq: u8 = 0;
        feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);
        let our_lcp_data = our_lcp_request_data(&session);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_lcp_configure_ack(1, our_lcp_data),
            &mut seq,
        );
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_request_zero(1), &mut seq);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_ip(2, Ipv4Addr::new(10, 0, 0, 2)),
            &mut seq,
        );
        let our_ipcp_data = our_default_ipcp_request_data();
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_ack(1, our_ipcp_data), &mut seq);
        assert_eq!(session.phase(), SessionPhase::Active);

        session.set_sch_active(true);
        session.enable_sch_data_path();

        let actions = session.tick(None);
        let sch_frame = actions
            .iter()
            .find_map(|action| match action {
                SessionAction::SendSchFrame { bits, rate_bps } => Some((bits, *rate_bps)),
                _ => None,
            })
            .expect("active SCH should emit a fill SDU instead of DTX");

        assert_eq!(sch_frame.1, DEFAULT_RC3_F_SCH_RATE_BPS);
        assert_eq!(&sch_frame.0[0..SCH_TYPE3_HEADER_BITS], &[1, 1, 1, 0, 0, 0]);
    }

    #[test]
    fn ip_packet_delivery_uplink() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        let mut seq = drive_to_active(&mut session);

        // Mobile sends an IP packet via PPP.
        let ip_packet = vec![
            0x45, 0x00, 0x00, 0x1C, // IPv4 header start
            0x00, 0x01, 0x00, 0x00, 0x40, 0x01, 0x00, 0x00, // TTL=64, ICMP
            0x0A, 0x00, 0x00, 0x02, // src: 10.0.0.2
            0x0A, 0x00, 0x00, 0x01, // dst: 10.0.0.1
            0x08, 0x00, 0x00, 0x00, // ICMP echo request
            0x00, 0x01, 0x00, 0x01,
        ];
        let ppp = PppPacket {
            protocol: 0x0021,
            payload: ip_packet.clone(),
        };
        let actions = feed_ppp_via_rlp(&mut session, &ppp, &mut seq);

        let ip_actions: Vec<&SessionAction> = actions
            .iter()
            .filter(|a| matches!(a, SessionAction::DeliverIpPacket(_)))
            .collect();
        assert_eq!(ip_actions.len(), 1);
        if let SessionAction::DeliverIpPacket(data) = ip_actions[0] {
            assert_eq!(data, &ip_packet);
        }
    }

    #[test]
    fn vj_uplink_tcp_packets_are_restored_and_delivered() {
        let mut session = PacketSession::new(
            u32::from(SERVICE_OPTION_PACKET_DATA),
            ipcp_config_with_local_vj(),
        );
        let mut seq = drive_to_active_with_our_ipcp_data(&mut session, our_vj_ipcp_request_data());

        let mut mobile_vj = VjState::default();
        mobile_vj.configure(Some(crate::ppp::vj::VjCompressionOptions::default()), None);
        let first = mobile_tcp_packet(
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(10, 0, 0, 1),
            100,
            1,
        );
        let second = mobile_tcp_packet(
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(10, 0, 0, 1),
            101,
            2,
        );

        let seed = mobile_vj.compress_ip_packet(&first);
        assert_eq!(seed.protocol, PPP_VJ_UNCOMPRESSED_TCP);
        let seed_actions = feed_ppp_via_rlp(&mut session, &seed, &mut seq);
        assert!(
            seed_actions
                .iter()
                .any(|a| matches!(a, SessionAction::DeliverIpPacket(data) if data == &first))
        );

        let compressed = mobile_vj.compress_ip_packet(&second);
        assert_eq!(compressed.protocol, PPP_VJ_COMPRESSED_TCP);
        let compressed_actions = feed_ppp_via_rlp(&mut session, &compressed, &mut seq);
        assert!(
            compressed_actions
                .iter()
                .any(|a| matches!(a, SessionAction::DeliverIpPacket(data) if data == &second))
        );
    }

    #[test]
    fn vj_downlink_tcp_packets_are_compressed_when_peer_negotiates_vj() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        let mut seq = 0;

        feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);
        let our_lcp_data = our_lcp_request_data(&session);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_lcp_configure_ack(1, our_lcp_data),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_ip_with_vj(1, Ipv4Addr::new(10, 0, 0, 2)),
            &mut seq,
        );
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_ack(1, our_default_ipcp_request_data()),
            &mut seq,
        );
        assert_eq!(session.phase(), SessionPhase::Active);

        let first = mobile_tcp_packet(
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            100,
            1,
        );
        let second = mobile_tcp_packet(
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            101,
            2,
        );
        session.send_ip_packet(&first);
        session.send_ip_packet(&second);

        assert_eq!(session.ppp_tx_queue.len(), 2);
        assert_eq!(session.ppp_tx_queue[0].protocol, PPP_VJ_UNCOMPRESSED_TCP);
        assert_eq!(session.ppp_tx_queue[1].protocol, PPP_VJ_COMPRESSED_TCP);
    }

    #[test]
    fn uplink_ip_packet_dropped_before_active() {
        // IP packets before IPCP completes must be dropped, no claim or delivery.
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);

        let mut seq = 0;
        let mobile_lcp_ack = mobile_lcp_configure_ack(1, our_lcp_request_data(&session));
        feed_ppp_via_rlp(&mut session, &mobile_lcp_ack, &mut seq);
        assert_eq!(session.phase(), SessionPhase::Lcp);

        let ip_packet = mobile_ipv4_packet(Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(8, 8, 8, 8));
        let ppp = PppPacket {
            protocol: 0x0021,
            payload: ip_packet,
        };
        let actions = feed_ppp_via_rlp(&mut session, &ppp, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Lcp);
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::DeliverIpPacket(_)))
        );
    }

    #[test]
    fn stale_ipcp_requested_address_is_handled_by_ipcp_nak_only() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);

        let mut seq = 0;
        feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);
        let our_lcp_data = our_lcp_request_data(&session);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_lcp_configure_ack(1, our_lcp_data),
            &mut seq,
        );
        assert_eq!(session.phase(), SessionPhase::Ipcp);

        let actions = feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_ip(4, Ipv4Addr::new(10, 0, 0, 3)),
            &mut seq,
        );

        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::CloseSession { .. }))
        );
        assert!(
            actions
                .iter()
                .all(|a| !matches!(a, SessionAction::DeliverIpPacket(_)))
        );
        assert_eq!(session.phase(), SessionPhase::Ipcp);
        assert_eq!(session.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));
    }

    #[test]
    fn ip_packet_delivery_downlink() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        drive_to_active(&mut session);

        // Inject an IP packet for downlink delivery.
        let ip_packet = vec![
            0x45, 0x00, 0x00, 0x1C, 0x00, 0x01, 0x00, 0x00, 0x40, 0x01, 0x00, 0x00, 0x0A, 0x00,
            0x00, 0x01, // src: 10.0.0.1
            0x0A, 0x00, 0x00, 0x02, // dst: 10.0.0.2
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01,
        ];
        session.send_ip_packet(&ip_packet);

        // Tick to generate downlink RLP frames.
        let actions = session.tick(None);

        // Should have at least one SendFrame.
        let send_frames: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, SessionAction::SendFrame { .. }))
            .collect();
        assert!(!send_frames.is_empty());

        // Verify the frame carries data (rate > eighth = not just idle).
        let has_data = send_frames
            .iter()
            .any(|a| matches!(a, SessionAction::SendFrame { rate_bps, .. } if *rate_bps > 1200));
        assert!(has_data, "downlink should contain data frames");
    }

    #[test]
    fn send_ip_packet_queued_before_active_and_flushed_after_ipcp_open() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        assert_eq!(session.phase(), SessionPhase::Lcp);

        let ip_packet = vec![
            0x45, 0x00, 0x00, 0x1C, 0x00, 0x01, 0x00, 0x00, 0x40, 0x01, 0x00, 0x00, 0x0A, 0x00,
            0x00, 0x01, 0x0A, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01,
        ];
        session.send_ip_packet(&ip_packet);
        assert_eq!(session.pending_downlink_ip.len(), 1);
        assert_eq!(session.pending_downlink_ip_octets, ip_packet.len());

        let mut seq: u8 = 0;
        feed_ppp_via_rlp(&mut session, &mobile_lcp_configure_request(1), &mut seq);
        let our_lcp_data = our_lcp_request_data(&session);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_lcp_configure_ack(1, our_lcp_data),
            &mut seq,
        );
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_request_zero(1), &mut seq);
        feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_request_ip(2, Ipv4Addr::new(10, 0, 0, 2)),
            &mut seq,
        );
        let actions = feed_ppp_via_rlp(
            &mut session,
            &mobile_ipcp_ack(1, our_default_ipcp_request_data()),
            &mut seq,
        );

        assert_eq!(session.phase(), SessionPhase::Active);
        assert!(session.pending_downlink_ip.is_empty());
        assert_eq!(session.pending_downlink_ip_octets, 0);
        assert!(
            actions.iter().any(
                |a| matches!(a, SessionAction::SendFrame { rate_bps, .. } if *rate_bps > 1200)
            ),
            "queued downlink IP should be emitted once IPCP opens"
        );
    }

    #[test]
    fn pending_downlink_ip_queue_is_bounded() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);

        for i in 0..300usize {
            let packet = vec![i as u8; 1000];
            session.send_ip_packet(&packet);
        }

        assert_eq!(
            session.pending_downlink_ip.len(),
            PENDING_DOWNLINK_IP_MAX_PACKETS
        );
        assert_eq!(
            session.pending_downlink_ip_octets,
            PENDING_DOWNLINK_IP_MAX_PACKETS * 1000
        );
        assert_eq!(session.pending_downlink_ip.front().unwrap()[0], 44);
    }

    #[test]
    fn close_session() {
        let mut session =
            PacketSession::new(u32::from(SERVICE_OPTION_PACKET_DATA), IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        session.send_ip_packet(&[0x45, 0x00]);
        assert_eq!(session.pending_downlink_ip.len(), 1);
        session.close();
        assert_eq!(session.phase(), SessionPhase::Closed);
        assert!(session.pending_downlink_ip.is_empty());
        assert_eq!(session.pending_downlink_ip_octets, 0);
    }

    // -----------------------------------------------------------------------
    // Helper: drive session all the way to Active phase.
    // Returns the next uplink RLP SEQ number for continued use.
    // -----------------------------------------------------------------------

    fn drive_to_active(session: &mut PacketSession) -> u8 {
        drive_to_active_with_our_ipcp_data(session, our_default_ipcp_request_data())
    }

    fn drive_to_active_with_our_ipcp_data(
        session: &mut PacketSession,
        our_ipcp_data: Vec<u8>,
    ) -> u8 {
        complete_rlp_handshake(session);

        let mut seq: u8 = 0;

        // LCP: mobile Configure-Request + Ack our request.
        let mobile_lcp_req = mobile_lcp_configure_request(1);
        feed_ppp_via_rlp(session, &mobile_lcp_req, &mut seq);

        let mobile_lcp_ack = mobile_lcp_configure_ack(1, our_lcp_request_data(&session));
        feed_ppp_via_rlp(session, &mobile_lcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Ipcp);

        // IPCP: mobile requests 0.0.0.0 → NAK → retry with 10.0.0.2 → ACK.
        let mobile_ipcp_req = mobile_ipcp_request_zero(1);
        feed_ppp_via_rlp(session, &mobile_ipcp_req, &mut seq);

        let mobile_ipcp_req2 = mobile_ipcp_request_ip(2, Ipv4Addr::new(10, 0, 0, 2));
        feed_ppp_via_rlp(session, &mobile_ipcp_req2, &mut seq);

        let mobile_ipcp_ack = mobile_ipcp_ack(1, our_ipcp_data);
        feed_ppp_via_rlp(session, &mobile_ipcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Active);
        seq
    }
}
