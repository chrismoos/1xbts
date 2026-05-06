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
use crate::ppp::framing::{self, HdlcDeframer, PppPacket};
use crate::ppp::ipcp::{IPCP_PROTOCOL, IpcpConfig, IpcpSession};
use crate::ppp::lcp::{LCP_PROTOCOL, LcpSession};
use crate::rlp::{self as rlp_codec, RlpFrame};
use crate::rlp_session::{RlpOutput, RlpSession, RlpState};
use crate::rlp3_frames::MuxOption;
use crate::rlp3_session::{FrameRate, Rlp3Config, Rlp3Session, Rlp3State, RlpEvent};

/// Session phase — tracks the overall negotiation progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// RLP SYNC handshake in progress.
    RlpSync,
    /// RLP is up, LCP negotiation in progress.
    Lcp,
    /// LCP is open, IPCP negotiation in progress.
    Ipcp,
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
    fn next_frame_bits(&mut self) -> (Vec<u8>, u32);

    /// Generate a supplemental channel frame with `info_bits` usable bits.
    /// Returns (bits, rate_bps) or None if no SCH data to send.
    /// Default: no SCH support.
    fn next_sch_frame_bits(&mut self, _info_bits: usize) -> Option<(Vec<u8>, u32)> {
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

    fn next_frame_bits(&mut self) -> (Vec<u8>, u32) {
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
                    if matches!(frame, crate::rlp3_frames::Rlp3Frame::Control { .. }) {
                        Self::log_control_frame(
                            self.log_context.as_deref(),
                            "RX",
                            &mut self.rx_control_log,
                            frame,
                        );
                    }
                }
                Err(e) => {
                    if self.rx_frame_count <= 5 || self.rx_frame_count % 100 == 0 {
                        let all_bits: String = bits
                            .iter()
                            .take(48)
                            .map(|&b| if b != 0 { '1' } else { '0' })
                            .collect();
                        log::warn!(
                            "RLP3 UL[{}]: decode failed: {:?} (frame={} rate={} len={} bits={}{})",
                            self.log_context.as_deref().unwrap_or("?"),
                            e,
                            self.rx_frame_count,
                            rate_bps,
                            bits.len(),
                            all_bits,
                            if bits.len() > 48 { "..." } else { "" }
                        );
                    }
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

    fn next_frame_bits(&mut self) -> (Vec<u8>, u32) {
        // Use full rate if we have data pending, pending controls (NAKs),
        // or are in handshake. In data-transfer idle periods, use quarter
        // rate: RLP3 less-than-Rate-1 fill/idle needs at least 40 info bits.
        let has_data = self.session.state() != Rlp3State::DataTransfer
            || !self.session.tx_queue_is_empty()
            || self.session.has_pending_controls();
        let rate = if has_data {
            FrameRate::Full
        } else {
            FrameRate::Quarter
        };
        let state_before = self.session.state();
        let queue_before = self.session.tx_queue_len();
        let bits = self.session.next_frame(rate);
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
            log::trace!(
                "RLP3 TXF[{}]: rate={} frame={} q_before={} q_after={}",
                self.log_context.as_deref().unwrap_or("?"),
                rate_bps,
                summary,
                queue_before,
                queue_after
            );
        }
        (bits, rate_bps)
    }

    fn next_sch_frame_bits(&mut self, info_bits: usize) -> Option<(Vec<u8>, u32)> {
        // Only produce SCH frames when in data transfer state with data pending
        if self.session.state() != Rlp3State::DataTransfer || self.session.tx_queue_is_empty() {
            return None;
        }
        // Generate a full-rate RLP3 data frame for the SCH.
        // The SCH carries bulk data using the shared sequence space.
        let bits = self.session.next_frame(FrameRate::Full);
        // Pad or truncate to the SCH info_bits size.
        // At 19.2 kbps, info_bits=360 vs FCH full-rate=172.
        // The RLP3 frame is 172 bits (FCH full rate); we need to fill
        // the larger SCH frame. For Phase 1, we generate one RLP3 frame
        // and zero-pad the remainder of the SCH frame.
        let mut sch_bits = Vec::with_capacity(info_bits);
        // MUX header: bit 0 = 1 (data present)
        sch_bits.push(1);
        // Copy RLP3 frame data (up to info_bits - 1 for MUX header)
        let data_bits = info_bits - 1;
        let copy_len = bits.len().min(data_bits);
        sch_bits.extend_from_slice(&bits[..copy_len]);
        // Zero-pad remainder
        for _ in sch_bits.len()..info_bits {
            sch_bits.push(0);
        }
        Some((sch_bits, 19200))
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

/// Packet data session engine.
/// F-SCH info bits per frame at 19.2 kbps.
const SCH_19K2_INFO_BITS: usize = 360;

pub struct PacketSession {
    rlp: Box<dyn RlpBackend>,
    deframer: HdlcDeframer,
    lcp: LcpSession,
    ipcp: IpcpSession,
    phase: SessionPhase,
    /// Pending PPP packets to send on downlink (queued during a single tick).
    ppp_tx_queue: Vec<PppPacket>,
    /// Whether we've sent our LCP Configure-Request.
    lcp_started: bool,
    /// Whether we've sent our IPCP Configure-Request.
    ipcp_started: bool,
    /// Always-on ring of recent PPP control-plane activity for diagnostics.
    recent_ppp_events: VecDeque<PacketTraceEvent>,
    /// When true, also generate supplemental channel frames each tick.
    sch_active: bool,
    /// SCH info bits per frame (360 for 19.2 kbps).
    sch_info_bits: usize,
}

impl PacketSession {
    pub fn new(service_option: u32, ipcp_config: IpcpConfig) -> Self {
        let rlp: Box<dyn RlpBackend> = if service_option == 33 {
            log::debug!("PacketSession: using RLP Type 3 for SO {}", service_option);
            Box::new(Rlp3Backend::new(Rlp3Config {
                mux_option: MuxOption::Odd, // 171-bit frames (MUX option 0x1, Rate Set 1)
                ..Rlp3Config::default()
            }))
        } else {
            log::debug!("PacketSession: using RLP Type 1 for SO {}", service_option);
            Box::new(Rlp1Backend::new())
        };
        Self {
            rlp,
            deframer: HdlcDeframer::new(),
            lcp: LcpSession::new(),
            ipcp: IpcpSession::new(ipcp_config),
            phase: SessionPhase::RlpSync,
            ppp_tx_queue: Vec::new(),
            lcp_started: false,
            ipcp_started: false,
            recent_ppp_events: VecDeque::new(),
            sch_active: false,
            sch_info_bits: SCH_19K2_INFO_BITS,
        }
    }

    /// Enable or disable supplemental channel frame generation.
    pub fn set_sch_active(&mut self, active: bool) {
        self.sch_active = active;
        log::info!(
            "PacketSession: SCH {}",
            if active { "activated" } else { "deactivated" }
        );
    }

    /// Returns whether SCH is active.
    pub fn is_sch_active(&self) -> bool {
        self.sch_active
    }

    pub fn set_log_context(&mut self, context: String) {
        self.rlp.set_log_context(context);
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

    /// Inject an IP packet from the network/TUN side for delivery to the mobile.
    /// Only valid when phase is Active.
    pub fn send_ip_packet(&mut self, ip_packet: &[u8]) {
        if self.phase != SessionPhase::Active {
            return;
        }
        let ppp = PppPacket {
            protocol: 0x0021, // IP
            payload: ip_packet.to_vec(),
        };
        self.ppp_tx_queue.push(ppp);
    }

    /// Process one frame period (20ms tick).
    ///
    /// `uplink`: raw primary traffic bits + rate_bps from the mobile this period,
    /// or `None` for an erasure/no frame.
    ///
    /// Returns a list of actions for the caller to perform.
    pub fn tick(&mut self, uplink: Option<(&[u8], u32)>) -> Vec<SessionAction> {
        let mut actions = Vec::new();

        // --- Uplink: process received frame through the RLP backend ---
        let delivery = if let Some((bits, rate_bps)) = uplink {
            self.rlp.receive_frame_bits(bits, rate_bps)
        } else {
            self.rlp.receive_frame_bits(&[], 0)
        };

        // Check if RLP just entered DataTransfer (phase transition RlpSync → Lcp).
        if self.phase == SessionPhase::RlpSync && self.rlp.is_data_transfer() {
            log::debug!("RLP: link established, entering LCP phase");
            self.phase = SessionPhase::Lcp;
        }

        // Feed any delivered bytes into the PPP deframer.
        if let Some(data) = delivery {
            log::debug!("RLP: delivered {} bytes to PPP deframer", data.len());
            let ppp_packets = self.deframer.feed(&data);
            for ppp in ppp_packets {
                capture::write_ppp_packet(
                    CaptureDirection::Uplink,
                    &ppp,
                    &self.uplink_capture_frame_options(&ppp),
                );
                self.record_ppp_event("uplink", &ppp);
                log::debug!("PPP RX: {}", format_ppp_packet(&ppp));
                self.process_uplink_ppp(&ppp, &mut actions);
            }
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

        if self.phase == SessionPhase::Ipcp
            && let Some(req) = self.ipcp.maybe_retransmit_configure_request()
        {
            self.ppp_tx_queue.push(req);
        }

        // --- Downlink: convert queued PPP packets to RLP byte stream ---
        // Build TX framing options from LCP negotiation.
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
            self.record_ppp_event("downlink", ppp);
            log::debug!("PPP TX: {}", format_ppp_packet(ppp));
            let txq_before = self.rlp.tx_queue_len();
            capture::write_ppp_packet(CaptureDirection::Downlink, ppp, &frame_opts);
            let hdlc_bytes = framing::frame_with_options(ppp, &frame_opts);
            let hdlc_len = hdlc_bytes.len();
            self.rlp.enqueue_data(&hdlc_bytes);
            let txq_after = self.rlp.tx_queue_len();
            log::debug!(
                "PPP TX enqueue: {} hdlc_len={} rlp_txq_before={} rlp_txq_after={}",
                format_ppp_packet(ppp),
                hdlc_len,
                txq_before,
                txq_after
            );
        }

        // --- Get next downlink FCH frame ---
        let (bits, rate_bps) = self.rlp.next_frame_bits();
        if !bits.is_empty() {
            actions.push(SessionAction::SendFrame { bits, rate_bps });
        }

        // --- Get next downlink SCH frame (if active) ---
        if self.sch_active {
            if let Some((sch_bits, sch_rate)) = self.rlp.next_sch_frame_bits(self.sch_info_bits) {
                actions.push(SessionAction::SendSchFrame {
                    bits: sch_bits,
                    rate_bps: sch_rate,
                });
            }
        }

        actions
    }

    /// Close the session.
    /// Generate an LCP Echo-Request keepalive if the link is open.
    /// Call periodically (e.g., every 30s). Returns a PPP packet to
    /// send via the RLP downlink, or None.
    pub fn maybe_send_echo(&mut self) -> Option<Vec<u8>> {
        let ppp = self.lcp.maybe_send_echo()?;
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
    }

    pub fn telemetry(&self) -> PacketSessionTelemetry {
        let rlp = self.rlp.telemetry();
        PacketSessionTelemetry {
            rlp_state: rlp.state,
            lcp_state: format_lcp_state(self.lcp.state),
            ipcp_state: format_ipcp_state(self.ipcp.state),
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

    fn process_uplink_ppp(&mut self, ppp: &PppPacket, actions: &mut Vec<SessionAction>) {
        match ppp.protocol {
            LCP_PROTOCOL => {
                let was_open = self.lcp.is_open();
                let responses = self.lcp.receive(ppp);
                for resp in responses {
                    self.ppp_tx_queue.push(resp);
                }

                // Peer restarted LCP while we were in IPCP or Active — reset
                // upper layers and go back to LCP phase.
                if was_open && !self.lcp.is_open() {
                    log::info!("LCP: peer restarted, resetting IPCP and returning to LCP phase");
                    self.ipcp = IpcpSession::new(self.ipcp.config.clone());
                    self.ipcp_started = false;
                    self.phase = SessionPhase::Lcp;
                }

                // Check for LCP open → transition to IPCP.
                if self.lcp.is_open() && self.phase == SessionPhase::Lcp {
                    log::info!("LCP: link opened, entering IPCP phase");
                    self.phase = SessionPhase::Ipcp;
                }
            }
            IPCP_PROTOCOL => {
                // NOTE: force-open disabled — the two root causes that
                // required it have been fixed:
                //
                // 1. Uplink frame queuing: session_task used Option::replace
                //    which silently dropped frames delivered in bursts between
                //    20ms ticks.  Fixed by using VecDeque with one-per-tick
                //    dequeue (commit 3b0c1b3).
                //
                // 2. NAK rate selection: RLP NAK control frames could only be
                //    sent on full-rate downlink frames, but the rate selector
                //    chose eighth-rate when the tx_queue was empty.  NAKs sat
                //    in pending_controls unable to go out, so retransmission
                //    rounds expired without ever reaching the MS.  Fixed by
                //    including has_pending_controls() in the rate decision.
                //
                // If IPCP frames arrive while LCP is still in AckSent, it
                // means the peer's Configure-Ack was genuinely lost despite
                // working retransmission.  Re-enable this block if that
                // occurs on real air-interface links.
                //
                // if !self.lcp.is_open() && self.lcp.state == LcpState::AckSent {
                //     log::info!(
                //         "LCP: received IPCP while in AckSent — peer's Configure-Ack \
                //          was lost, forcing LCP open"
                //     );
                //     self.lcp.force_open();
                //     self.phase = SessionPhase::Ipcp;
                // }

                let responses = self.ipcp.receive(ppp);
                for resp in responses {
                    self.ppp_tx_queue.push(resp);
                }
                // Check for IPCP open → transition to Active.
                if self.ipcp.is_open() && self.phase == SessionPhase::Ipcp {
                    self.phase = SessionPhase::Active;
                    log::info!(
                        "Packet session active: peer={} gateway={}",
                        self.ipcp.peer_ip(),
                        self.ipcp.our_ip()
                    );
                }
            }
            0x0021 => {
                // IP packet from mobile.
                if self.phase == SessionPhase::Active {
                    let payload = &ppp.payload;
                    // Basic IPv4 validation + source IP ingress filter.
                    if payload.len() < 20 || (payload[0] >> 4) != 4 {
                        log::warn!(
                            "IP ingress: dropping malformed packet (len={} ver={})",
                            payload.len(),
                            payload.get(0).map(|b| b >> 4).unwrap_or(0),
                        );
                        return;
                    }
                    let src_ip = Ipv4Addr::new(payload[12], payload[13], payload[14], payload[15]);
                    let expected = self.ipcp.peer_ip();
                    if src_ip != expected && !src_ip.is_unspecified() {
                        log::warn!(
                            "IP ingress: dropping spoofed source {} (expected {})",
                            src_ip,
                            expected,
                        );
                        return;
                    }
                    if src_ip.is_unspecified() {
                        // DHCP discover (0.0.0.0 → 255.255.255.255) — normal phone behavior,
                        // not needed for CDMA2000 (IP assigned via IPCP). Drop silently.
                        log::debug!("IP ingress: ignoring DHCP discover from 0.0.0.0");
                        return;
                    }
                    actions.push(SessionAction::DeliverIpPacket(ppp.payload.clone()));
                }
            }
            other => {
                log::debug!("Ignoring PPP protocol 0x{:04X}", other);
            }
        }
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
        if ppp.protocol == 0x0021 {
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
        crate::rlp3_frames::Rlp3Frame::Nak { seq, payload, .. } => {
            format!("nak seq={} payload={:?}", seq, payload)
        }
    }
}

/// Format a PPP packet for logging.
fn format_ppp_packet(ppp: &PppPacket) -> String {
    let proto_name = match ppp.protocol {
        0xC021 => "LCP",
        0x8021 => "IPCP",
        0x0021 => "IP",
        0xC023 => "PAP",
        0xC223 => "CHAP",
        other => return format!("proto=0x{:04X} len={}", other, ppp.payload.len()),
    };

    if ppp.protocol == 0x0021 {
        return format!("{} {}", proto_name, summarize_ipv4_packet(&ppp.payload));
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
        0x0021 => "IP",
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
    data.iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("")
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
    use crate::ppp::framing::{self, PppPacket};
    use crate::ppp::ipcp;
    use crate::ppp::lcp;
    use crate::rlp;

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

    /// Build IPCP Configure-Ack from mobile.
    fn mobile_ipcp_ack(id: u8, data: Vec<u8>) -> PppPacket {
        let ipcp = ipcp::IpcpPacket {
            code: 2,
            identifier: id,
            data,
        };
        ipcp.to_ppp()
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

    #[test]
    fn session_starts_in_rlp_sync() {
        let session = PacketSession::new(7, IpcpConfig::default());
        assert_eq!(session.phase(), SessionPhase::RlpSync);
    }

    #[test]
    fn rlp_handshake_transitions_to_lcp() {
        let mut session = PacketSession::new(7, IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        assert_eq!(session.phase(), SessionPhase::Lcp);
    }

    #[test]
    fn full_negotiation_to_active() {
        let mut session = PacketSession::new(7, IpcpConfig::default());
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

        // Mobile acks our Configure-Request (ID=1, MRU option).
        let mobile_lcp_ack = mobile_lcp_configure_ack(1, vec![1, 4, 0x05, 0xDC]);
        feed_ppp_via_rlp(&mut session, &mobile_lcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Ipcp);

        // Mobile sends IPCP Configure-Request with IP=0.0.0.0.
        let mobile_ipcp_req = mobile_ipcp_request_zero(1);
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_req, &mut seq);

        // Mobile retries with correct IP.
        let mobile_ipcp_req2 = mobile_ipcp_request_ip(2, Ipv4Addr::new(10, 0, 0, 2));
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_req2, &mut seq);

        // Mobile acks our IPCP request (ID=1).
        let our_ipcp_data = vec![3, 6, 10, 0, 0, 1]; // IP-Address 10.0.0.1
        let mobile_ipcp_ack = mobile_ipcp_ack(1, our_ipcp_data);
        feed_ppp_via_rlp(&mut session, &mobile_ipcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Active);
        assert_eq!(session.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(session.our_ip(), Ipv4Addr::new(10, 0, 0, 1));
    }

    #[test]
    fn ip_packet_delivery_uplink() {
        let mut session = PacketSession::new(7, IpcpConfig::default());
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
    fn ip_packet_delivery_downlink() {
        let mut session = PacketSession::new(7, IpcpConfig::default());
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
    fn send_ip_packet_ignored_before_active() {
        let mut session = PacketSession::new(7, IpcpConfig::default());
        complete_rlp_handshake(&mut session);
        assert_eq!(session.phase(), SessionPhase::Lcp);

        // Try to send IP — should be silently ignored.
        session.send_ip_packet(&[0x45, 0x00]);
        let actions = session.tick(None);

        // Only RLP frame actions, no IP delivery.
        for action in &actions {
            assert!(matches!(action, SessionAction::SendFrame { .. }));
        }
    }

    #[test]
    fn close_session() {
        let mut session = PacketSession::new(7, IpcpConfig::default());
        session.close();
        assert_eq!(session.phase(), SessionPhase::Closed);
    }

    // -----------------------------------------------------------------------
    // Helper: drive session all the way to Active phase.
    // Returns the next uplink RLP SEQ number for continued use.
    // -----------------------------------------------------------------------

    fn drive_to_active(session: &mut PacketSession) -> u8 {
        complete_rlp_handshake(session);

        let mut seq: u8 = 0;

        // LCP: mobile Configure-Request + Ack our request.
        let mobile_lcp_req = mobile_lcp_configure_request(1);
        feed_ppp_via_rlp(session, &mobile_lcp_req, &mut seq);

        let mobile_lcp_ack = mobile_lcp_configure_ack(1, vec![1, 4, 0x05, 0xDC]);
        feed_ppp_via_rlp(session, &mobile_lcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Ipcp);

        // IPCP: mobile requests 0.0.0.0 → NAK → retry with 10.0.0.2 → ACK.
        let mobile_ipcp_req = mobile_ipcp_request_zero(1);
        feed_ppp_via_rlp(session, &mobile_ipcp_req, &mut seq);

        let mobile_ipcp_req2 = mobile_ipcp_request_ip(2, Ipv4Addr::new(10, 0, 0, 2));
        feed_ppp_via_rlp(session, &mobile_ipcp_req2, &mut seq);

        let our_ipcp_data = vec![3, 6, 10, 0, 0, 1];
        let mobile_ipcp_ack = mobile_ipcp_ack(1, our_ipcp_data);
        feed_ppp_via_rlp(session, &mobile_ipcp_ack, &mut seq);

        assert_eq!(session.phase(), SessionPhase::Active);
        seq
    }
}
