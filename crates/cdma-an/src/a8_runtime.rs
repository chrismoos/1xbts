use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use cdma_common::error::Error;
use cdma_common::hrpd::air as hrpd_air;
use log::{debug, info, trace, warn};

use crate::rlp;

const HRPD_BEARER_MAX_DATAGRAMS_PER_PASS: usize = 64;
// Per RLP segment the forward framing adds 32 bits (22-bit RLP sequence,
// 2-bit stream header, 8-bit Format B length), so a 120-octet segment frames
// to exactly 992 bits — the enhanced 1024-bit packet's security capacity, and
// an even divisor of every larger enhanced and legacy packet size. It must
// divide every enhanced and legacy packet size; 121 octets does not (it
// overflows the enhanced 1024-bit packet).
const HRPD_A8_FORWARD_MAX_STREAM_OCTETS: usize = 120;
const HRPD_CONTROL_CHANNEL_CYCLE_MILLIS: u64 = 427;
const HRPD_OPEN_CONNECTION_RETRY_INTERVAL: Duration = Duration::from_millis(2500);
const HRPD_DATA_READY_RETRY_INTERVAL: Duration = Duration::from_millis(1200);
// Conservative forward DRC used only until the A8 has observed its first
// valid reverse-DRC report. Once a valid DRC has been seen, retain it as the
// packet-packing hint across DRC outages. The BTS scheduler owns the governing
// per-window DRC and can split an oversized Format-B packet, whereas packing
// at this fallback during a tune-away leaves hundreds of 120-octet packets
// that each consume a full high-rate slot after the AT returns.
const HRPD_A8_FALLBACK_FORWARD_DRC_INDEX: u8 = 0x2;
const HRPD_A8_DOWNLINK_STATS_INTERVAL: Duration = Duration::from_secs(1);
const HRPD_A8_PARTIAL_PACKET_COALESCE_DELAY: Duration = Duration::from_millis(12);
const HRPD_A8_FORWARD_MAX_PACKETS_PER_FLUSH: usize = 16;
const HRPD_A8_RLP_RETRANSMIT_MATERIALIZE_BUDGET: usize = HRPD_A8_FORWARD_MAX_PACKETS_PER_FLUSH;
const HRPD_A8_RLP_HISTORY_MAX_OCTETS: usize = 1 << 20;
const HRPD_DISABLE_HRPD_A8_OPEN_CONNECTION_PAGE: bool = false;
const HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES: u16 = 12;
const HRPD_DEFAULT_PACKET_DATA_READY: u8 = 0x0b;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HrpdAnForwardTrafficPacket {
    pub mac_index: u8,
    pub physical_layer_subtype: u16,
    pub forward_traffic_mac_subtype: u16,
    pub high_priority: bool,
    /// One bit per byte (0 or 1), matching the HRPD forward scheduler format.
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct HrpdSessionConfigurationState {
    physical_layer_subtype: u16,
    forward_traffic_mac_subtype: u16,
    idle_preferred_control_channel_cycle: Option<u16>,
    idle_page_period_cycles: u16,
}

#[derive(Debug)]
enum HrpdAnA8Command {
    Register {
        session_uati: u32,
        uati: u32,
        mac_index: u8,
        bearer: cdma_a8::BearerSession,
    },
    Release {
        uati: u32,
        retain_session_configuration: bool,
    },
    SendUplink {
        uati: u32,
        payload: Vec<u8>,
        queued_at: Instant,
    },
    SetTrafficChannelOpen {
        uati: u32,
        open: bool,
    },
    SetTrafficMacIndex {
        uati: u32,
        mac_index: u8,
    },
    SetTrafficSetupPending {
        uati: u32,
        pending: bool,
    },
    SetAddressManagementPending {
        uati: u32,
        pending: bool,
    },
    SetTrafficConfigurationPending {
        uati: u32,
        pending: bool,
    },
    SetSessionConfigurationComplete {
        uati: u32,
        complete: bool,
        physical_layer_subtype: u16,
        forward_traffic_mac_subtype: u16,
        idle_preferred_control_channel_cycle: Option<u16>,
        idle_page_period_cycles: u16,
    },
    SetDefaultPacketFlowOpen {
        uati: u32,
        open: bool,
    },
    SetDefaultPacketStreamConfiguration {
        uati: u32,
        stream_id: u8,
        protocol_type: u8,
    },
    DefaultPacketDataReadyAck {
        uati: u32,
        transaction_id: u8,
    },
    ResetDefaultPacketRlp {
        uati: u32,
    },
    RetransmitDefaultPacketRlp {
        uati: u32,
        requests: Vec<hrpd_air::HrpdDefaultPacketRlpNakRequest>,
    },
    UpdateDrc {
        uati: u32,
        drc_index: u8,
    },
    RetargetPendingDownlink {
        uati: u32,
        include_open_sessions: bool,
    },
}

#[derive(Clone)]
pub struct HrpdAnA8Runtime {
    tx: tokio::sync::mpsc::UnboundedSender<HrpdAnA8Command>,
}

impl HrpdAnA8Runtime {
    pub fn register(
        &self,
        session_uati: u32,
        uati: u32,
        mac_index: u8,
        bearer: cdma_a8::BearerSession,
    ) {
        if self
            .tx
            .send(HrpdAnA8Command::Register {
                session_uati,
                uati,
                mac_index,
                bearer,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before session registration");
        }
    }

    pub fn release_a8(&self, uati: u32) {
        if self
            .tx
            .send(HrpdAnA8Command::Release {
                uati,
                retain_session_configuration: true,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before session release");
        }
    }

    pub fn release_session(&self, uati: u32) {
        if self
            .tx
            .send(HrpdAnA8Command::Release {
                uati,
                retain_session_configuration: false,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before session release");
        }
    }

    pub fn send_uplink(&self, uati: u32, payload: Vec<u8>) {
        if self
            .tx
            .send(HrpdAnA8Command::SendUplink {
                uati,
                payload,
                queued_at: Instant::now(),
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before uplink delivery");
        }
    }

    pub fn set_traffic_channel_open(&self, uati: u32, open: bool) {
        if self
            .tx
            .send(HrpdAnA8Command::SetTrafficChannelOpen { uati, open })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before traffic-state update");
        }
    }

    pub fn set_traffic_mac_index(&self, uati: u32, mac_index: u8) {
        if self
            .tx
            .send(HrpdAnA8Command::SetTrafficMacIndex { uati, mac_index })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before traffic-MAC update");
        }
    }

    pub fn set_traffic_setup_pending(&self, uati: u32, pending: bool) {
        if self
            .tx
            .send(HrpdAnA8Command::SetTrafficSetupPending { uati, pending })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before traffic-setup update");
        }
    }

    pub fn set_address_management_pending(&self, uati: u32, pending: bool) {
        if self
            .tx
            .send(HrpdAnA8Command::SetAddressManagementPending { uati, pending })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before address-management update");
        }
    }

    pub fn set_traffic_configuration_pending(&self, uati: u32, pending: bool) {
        if self
            .tx
            .send(HrpdAnA8Command::SetTrafficConfigurationPending { uati, pending })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before traffic-configuration update");
        }
    }

    pub fn set_session_configuration_complete(
        &self,
        uati: u32,
        complete: bool,
        physical_layer_subtype: u16,
        forward_traffic_mac_subtype: u16,
        idle_preferred_control_channel_cycle: Option<u16>,
        idle_page_period_cycles: u16,
    ) {
        if self
            .tx
            .send(HrpdAnA8Command::SetSessionConfigurationComplete {
                uati,
                complete,
                physical_layer_subtype,
                forward_traffic_mac_subtype,
                idle_preferred_control_channel_cycle,
                idle_page_period_cycles,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before session-configuration update");
        }
    }

    pub fn set_default_packet_flow_open(&self, uati: u32, open: bool) {
        if self
            .tx
            .send(HrpdAnA8Command::SetDefaultPacketFlowOpen { uati, open })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before flow-control update");
        }
    }

    pub fn set_default_packet_stream_configuration(
        &self,
        uati: u32,
        stream_id: u8,
        protocol_type: u8,
    ) {
        if self
            .tx
            .send(HrpdAnA8Command::SetDefaultPacketStreamConfiguration {
                uati,
                stream_id,
                protocol_type,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before stream-configuration update");
        }
    }

    pub fn default_packet_data_ready_ack(&self, uati: u32, transaction_id: u8) {
        if self
            .tx
            .send(HrpdAnA8Command::DefaultPacketDataReadyAck {
                uati,
                transaction_id,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before DataReadyAck update");
        }
    }

    pub fn reset_default_packet_rlp(&self, uati: u32) {
        if self
            .tx
            .send(HrpdAnA8Command::ResetDefaultPacketRlp { uati })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before RLP reset update");
        }
    }

    pub fn retransmit_default_packet_rlp(
        &self,
        uati: u32,
        requests: Vec<hrpd_air::HrpdDefaultPacketRlpNakRequest>,
    ) {
        if self
            .tx
            .send(HrpdAnA8Command::RetransmitDefaultPacketRlp { uati, requests })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before RLP NAK update");
        }
    }

    pub fn update_drc(&self, uati: u32, drc_index: u8) {
        if self
            .tx
            .send(HrpdAnA8Command::UpdateDrc { uati, drc_index })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before DRC update");
        }
    }

    pub fn retarget_stale_downlink_to_active_uati(&self, uati: u32) {
        if self
            .tx
            .send(HrpdAnA8Command::RetargetPendingDownlink {
                uati,
                include_open_sessions: true,
            })
            .is_err()
        {
            warn!("HRPD AN A8: bearer runtime stopped before active-UATI downlink retarget");
        }
    }
}

#[derive(Clone, Debug)]
struct HrpdRlpHistoryOctet {
    sequence: u32,
    octet: u8,
}

#[derive(Clone, Debug)]
struct HrpdRlpRetransmitSegment {
    sequence: u32,
    octets: Vec<u8>,
}

#[derive(Clone, Debug)]
struct HrpdRlpRetransmitRequest {
    next_sequence: u32,
    remaining: u16,
}

#[derive(Clone, Debug)]
struct HrpdA8QueuedRlpSegment {
    sequence: u32,
    octets: Vec<u8>,
    retransmission: bool,
}

#[derive(Clone, Debug, Default)]
struct HrpdA8TrafficWindowStats {
    started_at: Option<Instant>,
    downlink_packets: usize,
    downlink_octets: usize,
    downlink_chunks: usize,
    max_pending_chunks: usize,
    forward_packets: usize,
    rlp_segments: usize,
    rlp_full_packets: usize,
    rlp_partial_packets: usize,
    rlp_partial_new_packets: usize,
    rlp_partial_retx_packets: usize,
    rlp_partial_mixed_packets: usize,
    rlp_partial_deferred: usize,
    rlp_new_octets: usize,
    rlp_retx_octets: usize,
    rlp_nak_requests: usize,
    rlp_nak_reaches_v_s: usize,
    flush_source_drained: usize,
    flush_capacity_limited: usize,
    flush_drc_starved: usize,
}

#[derive(Clone, Debug)]
struct HrpdAnA8Session {
    session_uati: u32,
    mac_index: u8,
    rlp_seq: u32,
    traffic_open: bool,
    traffic_setup_pending: bool,
    session_configuration_complete: bool,
    physical_layer_subtype: u16,
    forward_traffic_mac_subtype: u16,
    idle_preferred_control_channel_cycle: Option<u16>,
    idle_page_period_cycles: u16,
    traffic_configuration_complete: bool,
    initial_connection_close_observed: bool,
    address_management_pending: bool,
    open_connection_last_sent: Option<Instant>,
    default_packet_flow_open: bool,
    default_packet_stream_id: u8,
    default_packet_protocol_type: u8,
    data_ready_transaction: u8,
    data_ready_last_sent: Option<Instant>,
    data_ready_outstanding: Option<u8>,
    data_ready_last_transaction_sent: Option<u8>,
    data_ready_acknowledged: bool,
    last_drc_index: Option<u8>,
    last_drc_at: Option<Instant>,
    pending_downlink_partial_hold_at: Option<Instant>,
    pending_downlink: VecDeque<Vec<u8>>,
    rlp_history: VecDeque<HrpdRlpHistoryOctet>,
    pending_retransmit: VecDeque<HrpdRlpRetransmitSegment>,
    pending_retransmit_requests: VecDeque<HrpdRlpRetransmitRequest>,
    downlink_stats_last_log: Instant,
    downlink_stats_packets: usize,
    downlink_stats_octets: usize,
    downlink_stats_chunks: usize,
    traffic_window_stats: HrpdA8TrafficWindowStats,
}

impl HrpdAnA8Session {
    fn new(session_uati: u32, mac_index: u8) -> Self {
        Self {
            session_uati,
            mac_index,
            rlp_seq: 0,
            traffic_open: false,
            traffic_setup_pending: false,
            session_configuration_complete: false,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            idle_preferred_control_channel_cycle: None,
            idle_page_period_cycles: HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES,
            traffic_configuration_complete: false,
            initial_connection_close_observed: false,
            address_management_pending: false,
            open_connection_last_sent: None,
            default_packet_flow_open: false,
            default_packet_stream_id: cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM_ID,
            default_packet_protocol_type:
                cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE,
            data_ready_transaction: 0,
            data_ready_last_sent: None,
            data_ready_outstanding: None,
            data_ready_last_transaction_sent: None,
            data_ready_acknowledged: false,
            last_drc_index: None,
            last_drc_at: None,
            pending_downlink_partial_hold_at: None,
            pending_downlink: VecDeque::new(),
            rlp_history: VecDeque::new(),
            pending_retransmit: VecDeque::new(),
            pending_retransmit_requests: VecDeque::new(),
            downlink_stats_last_log: Instant::now(),
            downlink_stats_packets: 0,
            downlink_stats_octets: 0,
            downlink_stats_chunks: 0,
            traffic_window_stats: HrpdA8TrafficWindowStats::default(),
        }
    }
}

fn hrpd_a8_note_downlink_buffered(uati: u32, session: &mut HrpdAnA8Session, octets: usize) {
    let chunks = octets.div_ceil(HRPD_A8_FORWARD_MAX_STREAM_OCTETS);
    session.downlink_stats_packets += 1;
    session.downlink_stats_octets += octets;
    session.downlink_stats_chunks += chunks;
    session.traffic_window_stats.downlink_packets += 1;
    session.traffic_window_stats.downlink_octets += octets;
    session.traffic_window_stats.downlink_chunks += chunks;
    session.traffic_window_stats.max_pending_chunks = session
        .traffic_window_stats
        .max_pending_chunks
        .max(session.pending_downlink.len());
    trace!(
        "HRPD AN A8: buffered downlink UATI=0x{uati:08x} octets={} chunks={} traffic_open={} packet_flow_open={}",
        octets,
        session.pending_downlink.len(),
        session.traffic_open,
        session.default_packet_flow_open
    );
    if session.downlink_stats_last_log.elapsed() < HRPD_A8_DOWNLINK_STATS_INTERVAL {
        return;
    }
    debug!(
        "HRPD AN A8: downlink intake UATI=0x{uati:08x} packets={} octets={} chunks={} pending_chunks={} pending_retx_segments={} pending_retx_requests={} rlp_forward_packets={} rlp_segments={} rlp_full_packets={} rlp_partial_packets={} rlp_partial_new_packets={} rlp_partial_retx_packets={} rlp_partial_mixed_packets={} rlp_partial_deferred={} traffic_open={} packet_flow_open={}",
        session.downlink_stats_packets,
        session.downlink_stats_octets,
        session.downlink_stats_chunks,
        session.pending_downlink.len(),
        session.pending_retransmit.len(),
        session.pending_retransmit_requests.len(),
        session.traffic_window_stats.forward_packets,
        session.traffic_window_stats.rlp_segments,
        session.traffic_window_stats.rlp_full_packets,
        session.traffic_window_stats.rlp_partial_packets,
        session.traffic_window_stats.rlp_partial_new_packets,
        session.traffic_window_stats.rlp_partial_retx_packets,
        session.traffic_window_stats.rlp_partial_mixed_packets,
        session.traffic_window_stats.rlp_partial_deferred,
        session.traffic_open,
        session.default_packet_flow_open
    );
    session.downlink_stats_last_log = Instant::now();
    session.downlink_stats_packets = 0;
    session.downlink_stats_octets = 0;
    session.downlink_stats_chunks = 0;
}

fn hrpd_a8_enqueue_downlink_payload(uati: u32, session: &mut HrpdAnA8Session, payload: &[u8]) {
    if !session.default_packet_flow_open
        && payload.len() <= HRPD_A8_FORWARD_MAX_STREAM_OCTETS
        && session
            .pending_downlink
            .back()
            .is_some_and(|previous| previous.as_slice() == payload)
    {
        trace!(
            "HRPD AN A8: coalesced duplicate closed-flow downlink UATI=0x{uati:08x} octets={} pending_chunks={}",
            payload.len(),
            session.pending_downlink.len()
        );
        return;
    }

    // A partial packet is held until the byte stream has been quiet for the
    // coalescing interval. Full packets still leave immediately on the flush
    // below, while sustained A10 arrivals keep their one incomplete tail for
    // the next delivery instead of emitting it on a periodic timer.
    session.pending_downlink_partial_hold_at = None;

    // Default Packet RLP is an octet stream. A10 delivery boundaries are not
    // RLP segment boundaries, so first fill the queued tail before creating
    // another segment. Otherwise a stream of ordinary PPP packets produces
    // five nominally full RLP slots containing substantially less than the
    // 600-octet capacity of a 5120-bit Rev A packet.
    let mut remaining = payload;
    if let Some(tail) = session.pending_downlink.back_mut() {
        let available = HRPD_A8_FORWARD_MAX_STREAM_OCTETS.saturating_sub(tail.len());
        let take = available.min(remaining.len());
        tail.extend_from_slice(&remaining[..take]);
        remaining = &remaining[take..];
    }
    for chunk in remaining.chunks(HRPD_A8_FORWARD_MAX_STREAM_OCTETS) {
        session.pending_downlink.push_back(chunk.to_vec());
    }
}

fn reset_hrpd_a8_traffic_window_stats(session: &mut HrpdAnA8Session) {
    session.traffic_window_stats = HrpdA8TrafficWindowStats {
        started_at: Some(Instant::now()),
        ..HrpdA8TrafficWindowStats::default()
    };
}

fn log_hrpd_a8_traffic_close_summary(uati: u32, session: &HrpdAnA8Session) {
    let stats = &session.traffic_window_stats;
    let elapsed_ms = stats
        .started_at
        .map(|started_at| started_at.elapsed().as_millis() as u64)
        .unwrap_or(0);
    let downlink_kbps = if elapsed_ms == 0 {
        0.0
    } else {
        stats.downlink_octets as f64 * 8.0 / elapsed_ms as f64
    };
    let rlp_new_kbps = if elapsed_ms == 0 {
        0.0
    } else {
        stats.rlp_new_octets as f64 * 8.0 / elapsed_ms as f64
    };
    let last_drc = session
        .last_drc_index
        .map(|drc| format!("0x{drc:x}"))
        .unwrap_or_else(|| "none".to_string());
    info!(
        "HRPD AN A8: traffic close summary UATI=0x{uati:08x} mac={} elapsed_ms={} downlink_kbps={:.1} rlp_new_kbps={:.1} downlink_packets={} downlink_octets={} downlink_chunks={} max_pending_chunks={} forward_packets={} rlp_segments={} rlp_full_packets={} rlp_partial_packets={} rlp_partial_new_packets={} rlp_partial_retx_packets={} rlp_partial_mixed_packets={} rlp_partial_deferred={} rlp_new_octets={} rlp_retx_octets={} rlp_nak_requests={} rlp_nak_reaches_v_s={} flush_source_drained={} flush_capacity_limited={} flush_drc_starved={} pending_chunks={} pending_retx_segments={} pending_retx_requests={} rlp_seq={} last_drc={}",
        session.mac_index,
        elapsed_ms,
        downlink_kbps,
        rlp_new_kbps,
        stats.downlink_packets,
        stats.downlink_octets,
        stats.downlink_chunks,
        stats.max_pending_chunks,
        stats.forward_packets,
        stats.rlp_segments,
        stats.rlp_full_packets,
        stats.rlp_partial_packets,
        stats.rlp_partial_new_packets,
        stats.rlp_partial_retx_packets,
        stats.rlp_partial_mixed_packets,
        stats.rlp_partial_deferred,
        stats.rlp_new_octets,
        stats.rlp_retx_octets,
        stats.rlp_nak_requests,
        stats.rlp_nak_reaches_v_s,
        stats.flush_source_drained,
        stats.flush_capacity_limited,
        stats.flush_drc_starved,
        session.pending_downlink.len(),
        session.pending_retransmit.len(),
        session.pending_retransmit_requests.len(),
        session.rlp_seq,
        last_drc
    );
}

fn valid_hrpd_forward_drc(drc_index: u8, forward_traffic_mac_subtype: u16) -> Option<usize> {
    cdma_common::hrpd::traffic::implemented_forward_traffic_payload_bits_for_drc_for_mac_subtype(
        drc_index,
        forward_traffic_mac_subtype,
    )
}

fn hrpd_a8_live_forward_rate(session: &HrpdAnA8Session) -> Option<(u8, usize)> {
    let last_valid = session.last_drc_index.and_then(|drc_index| {
        valid_hrpd_forward_drc(drc_index, session.forward_traffic_mac_subtype)
            .map(|bits| (drc_index, bits))
    });
    if let Some(rate) = last_valid {
        return Some(rate);
    }
    // No valid A8-side DRC has arrived yet. Keep LCP/IPCP setup moving; the
    // BTS scheduler still re-rates the packet at the real governing DRC.
    let drc_index = HRPD_A8_FALLBACK_FORWARD_DRC_INDEX;
    let physical_bits = valid_hrpd_forward_drc(drc_index, session.forward_traffic_mac_subtype)?;
    Some((drc_index, physical_bits))
}

fn hrpd_rlp_request_reaches_v_s(
    v_s: u32,
    request: &hrpd_air::HrpdDefaultPacketRlpNakRequest,
) -> bool {
    if request.window_len == 0 {
        return false;
    }
    let first = request.first_erased & rlp::SEQUENCE_MASK;
    let first_from_v_s = first.wrapping_sub(v_s) & rlp::SEQUENCE_MASK;
    if first_from_v_s < rlp::SEQUENCE_MODULUS / 2 {
        return true;
    }
    let v_s_from_first = v_s.wrapping_sub(first) & rlp::SEQUENCE_MASK;
    v_s_from_first < u32::from(request.window_len)
}

fn record_hrpd_rlp_history(session: &mut HrpdAnA8Session, sequence: u32, octets: &[u8]) {
    let mut seq = sequence & rlp::SEQUENCE_MASK;
    for &octet in octets {
        session.rlp_history.push_back(HrpdRlpHistoryOctet {
            sequence: seq,
            octet,
        });
        seq = rlp::next(seq);
    }
    while session.rlp_history.len() > HRPD_A8_RLP_HISTORY_MAX_OCTETS {
        let _ = session.rlp_history.pop_front();
    }
}

fn hrpd_rlp_history_octet(session: &HrpdAnA8Session, sequence: u32) -> Option<u8> {
    let sequence = sequence & rlp::SEQUENCE_MASK;
    let first = session.rlp_history.front()?;
    let distance = sequence.wrapping_sub(first.sequence) & rlp::SEQUENCE_MASK;
    let index = usize::try_from(distance).ok()?;
    let entry = session.rlp_history.get(index)?;
    (entry.sequence == sequence).then_some(entry.octet)
}

fn reset_hrpd_a8_default_packet_rlp(session: &mut HrpdAnA8Session) {
    session.rlp_seq = 0;
    session.rlp_history.clear();
    session.pending_retransmit.clear();
    session.pending_retransmit_requests.clear();
}

fn queue_hrpd_rlp_nak_retransmissions(
    uati: u32,
    session: &mut HrpdAnA8Session,
    requests: &[hrpd_air::HrpdDefaultPacketRlpNakRequest],
) {
    session.traffic_window_stats.rlp_nak_requests += requests.len();
    let mut queued_segments = 0usize;
    let mut queued_octets = 0usize;
    let mut deferred_requests = 0usize;
    for request in requests {
        let mut sequence = request.first_erased & rlp::SEQUENCE_MASK;
        let mut remaining = usize::from(request.window_len);
        while remaining > 0 {
            if queued_segments >= HRPD_A8_RLP_RETRANSMIT_MATERIALIZE_BUDGET {
                session
                    .pending_retransmit_requests
                    .push_back(HrpdRlpRetransmitRequest {
                        next_sequence: sequence,
                        remaining: u16::try_from(remaining).unwrap_or(u16::MAX),
                    });
                deferred_requests += 1;
                break;
            }
            let segment_sequence = sequence;
            let mut octets = Vec::with_capacity(remaining.min(HRPD_A8_FORWARD_MAX_STREAM_OCTETS));
            while remaining > 0 && octets.len() < HRPD_A8_FORWARD_MAX_STREAM_OCTETS {
                let Some(octet) = hrpd_rlp_history_octet(session, sequence) else {
                    warn!(
                        "HRPD AN A8: DefaultPacket RLP Nak UATI=0x{uati:08x} missing history first_erased={} window_len={} missing_seq={} queued_octets={}",
                        request.first_erased, request.window_len, sequence, queued_octets
                    );
                    remaining = 0;
                    break;
                };
                octets.push(octet);
                sequence = rlp::next(sequence);
                remaining -= 1;
            }
            if octets.is_empty() {
                break;
            }
            queued_octets += octets.len();
            queued_segments += 1;
            session
                .pending_retransmit
                .push_back(HrpdRlpRetransmitSegment {
                    sequence: segment_sequence,
                    octets,
                });
        }
    }
    let request_ranges = requests
        .iter()
        .map(|request| format!("{}+{}", request.first_erased, request.window_len))
        .collect::<Vec<_>>()
        .join(",");
    let reaches_v_s = requests
        .iter()
        .filter(|request| hrpd_rlp_request_reaches_v_s(session.rlp_seq, request))
        .count();
    session.traffic_window_stats.rlp_nak_reaches_v_s += reaches_v_s;
    let history_first = session
        .rlp_history
        .front()
        .map(|entry| entry.sequence)
        .unwrap_or(session.rlp_seq);
    let history_last = session
        .rlp_history
        .back()
        .map(|entry| entry.sequence)
        .unwrap_or(session.rlp_seq);
    debug!(
        "HRPD AN A8: queued DefaultPacket RLP retransmit UATI=0x{uati:08x} requests={} ranges=[{}] rlp_seq={} history_first={} history_last={} history_octets={} reaches_v_s={} segments={} octets={} deferred_requests={} pending_retx_segments={} pending_retx_requests={}",
        requests.len(),
        request_ranges,
        session.rlp_seq,
        history_first,
        history_last,
        session.rlp_history.len(),
        reaches_v_s,
        queued_segments,
        queued_octets,
        deferred_requests,
        session.pending_retransmit.len(),
        session.pending_retransmit_requests.len()
    );
}

fn materialize_next_hrpd_rlp_retransmission(
    uati: u32,
    session: &mut HrpdAnA8Session,
) -> Option<HrpdRlpRetransmitSegment> {
    while let Some(mut request) = session.pending_retransmit_requests.pop_front() {
        let segment_sequence = request.next_sequence & rlp::SEQUENCE_MASK;
        let mut sequence = segment_sequence;
        let mut remaining = usize::from(request.remaining);
        let mut octets = Vec::with_capacity(remaining.min(HRPD_A8_FORWARD_MAX_STREAM_OCTETS));
        while remaining > 0 && octets.len() < HRPD_A8_FORWARD_MAX_STREAM_OCTETS {
            let Some(octet) = hrpd_rlp_history_octet(session, sequence) else {
                warn!(
                    "HRPD AN A8: deferred DefaultPacket RLP Nak UATI=0x{uati:08x} missing history next_sequence={} remaining={} missing_seq={}",
                    request.next_sequence, request.remaining, sequence
                );
                remaining = 0;
                break;
            };
            octets.push(octet);
            sequence = rlp::next(sequence);
            remaining -= 1;
        }
        if remaining > 0 {
            request.next_sequence = sequence;
            request.remaining = u16::try_from(remaining).unwrap_or(u16::MAX);
            session.pending_retransmit_requests.push_front(request);
        }
        if !octets.is_empty() {
            return Some(HrpdRlpRetransmitSegment {
                sequence: segment_sequence,
                octets,
            });
        }
    }
    None
}

fn hrpd_a8_traffic_configuration_complete_on_open(
    session: &HrpdAnA8Session,
    traffic_configuration_pending: bool,
) -> bool {
    session.session_configuration_complete && !traffic_configuration_pending
}

fn hrpd_a8_update_pending_traffic_configuration_for_open(
    uati: u32,
    session_configuration_complete: bool,
    pending_traffic_configuration: &mut HashSet<u32>,
) -> bool {
    if session_configuration_complete {
        pending_traffic_configuration.remove(&uati);
        false
    } else {
        pending_traffic_configuration.insert(uati);
        true
    }
}

fn hrpd_a8_take_pending_uati_alias(
    pending: &mut HashSet<u32>,
    uati: u32,
    session_uati: u32,
) -> bool {
    let direct = pending.remove(&uati);
    let session = pending.remove(&session_uati);
    direct || session
}

fn hrpd_a8_pending_uati_alias(pending: &HashSet<u32>, uati: u32, session_uati: u32) -> bool {
    pending.contains(&uati) || pending.contains(&session_uati)
}

fn hrpd_a8_pending_default_packet_stream(
    pending: &HashMap<u32, (u8, u8)>,
    uati: u32,
    session_uati: u32,
) -> Option<(u8, u8)> {
    pending
        .get(&uati)
        .copied()
        .or_else(|| pending.get(&session_uati).copied())
}

fn hrpd_idle_open_connection_schedule(
    session: &HrpdAnA8Session,
) -> Option<hrpd_air::HrpdSynchronousControlCycle> {
    let preferred_cycle = session.idle_preferred_control_channel_cycle?;
    let modulus = session.idle_page_period_cycles;
    if modulus == 0 {
        return None;
    }
    // The Control Channel MAC capsule header currently advertises Offset=0,
    // so C.S0024-400-C §1.5.6.1.6 reduces to (cycle + R) mod Period = 0.
    Some(hrpd_air::HrpdSynchronousControlCycle {
        modulus,
        residue: (modulus - (preferred_cycle % modulus)) % modulus,
    })
}

fn hrpd_open_connection_page_cycle(
    session: &HrpdAnA8Session,
) -> Option<hrpd_air::HrpdSynchronousControlCycle> {
    hrpd_idle_open_connection_schedule(session)
}

fn hrpd_a8_note_commit_close_window(session: &mut HrpdAnA8Session) {
    if !session.traffic_open {
        session.initial_connection_close_observed = true;
    }
}

fn hrpd_a8_note_traffic_setup_pending(session: &mut HrpdAnA8Session) {
    session.address_management_pending = false;
    session.traffic_configuration_complete = false;
    session.data_ready_last_sent = None;
    session.data_ready_outstanding = None;
    session.last_drc_index = None;
    session.last_drc_at = None;
}

fn hrpd_a8_note_traffic_closed(session: &mut HrpdAnA8Session) {
    session.open_connection_last_sent = None;
    session.data_ready_last_sent = None;
    session.data_ready_outstanding = None;
    session.last_drc_index = None;
    session.last_drc_at = None;
    session.traffic_configuration_complete = false;
    if session.session_configuration_complete {
        hrpd_a8_note_commit_close_window(session);
    }
}

fn hrpd_open_connection_retry_interval(session: &HrpdAnA8Session) -> Duration {
    let scheduled_period = session
        .idle_preferred_control_channel_cycle
        .and_then(|_| {
            (session.idle_page_period_cycles > 0).then_some(Duration::from_millis(
                u64::from(session.idle_page_period_cycles) * HRPD_CONTROL_CHANNEL_CYCLE_MILLIS,
            ))
        })
        .unwrap_or_default();
    HRPD_OPEN_CONNECTION_RETRY_INTERVAL.max(scheduled_period)
}

fn send_hrpd_a8_open_connection_page(
    uati: u32,
    session: &mut HrpdAnA8Session,
    forward_signaling_tx: &tokio::sync::mpsc::UnboundedSender<
        hrpd_air::HrpdForwardSignalingRequest,
    >,
) -> bool {
    if HRPD_DISABLE_HRPD_A8_OPEN_CONNECTION_PAGE {
        info!(
            "HRPD AN A8: not queueing IdleState OpenConnection Page UATI=0x{uati:08x}; paging disabled for packet-data debug buffered_chunks={}",
            session.pending_downlink.len()
        );
        return true;
    }
    if session.traffic_open || session.pending_downlink.is_empty() {
        return true;
    }
    if session.traffic_setup_pending {
        return true;
    }
    if session.address_management_pending {
        return true;
    }
    if let Some(last_sent) = session.open_connection_last_sent
        && last_sent.elapsed() < hrpd_open_connection_retry_interval(session)
    {
        return true;
    }
    let target_ati = hrpd_air::AccessTerminalIdentifier {
        ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
        value: uati,
    };
    let monitor_reopen =
        session.initial_connection_close_observed && session.open_connection_last_sent.is_none();
    let synchronous_control_cycle = hrpd_open_connection_page_cycle(session);
    let page = hrpd_air::HrpdForwardSignalingRequest::idle_state_page_for_control_cycle(
        uati,
        target_ati,
        synchronous_control_cycle,
    );
    let retry = session.open_connection_last_sent.is_some();
    session.open_connection_last_sent = Some(Instant::now());
    info!(
        "HRPD AN A8: queueing IdleState OpenConnection Page UATI=0x{uati:08x} buffered_chunks={} retry={retry} monitor_reopen={monitor_reopen} synchronous_cycle={synchronous_control_cycle:?} data_ready_outstanding={:?}",
        session.pending_downlink.len(),
        session.data_ready_outstanding,
    );
    forward_signaling_tx.send(page).is_ok()
}

fn send_hrpd_a8_data_ready(
    uati: u32,
    session: &mut HrpdAnA8Session,
    forward_signaling_tx: &tokio::sync::mpsc::UnboundedSender<
        hrpd_air::HrpdForwardSignalingRequest,
    >,
    forward_traffic_tx: &tokio::sync::mpsc::UnboundedSender<HrpdAnForwardTrafficPacket>,
) -> bool {
    if session.default_packet_flow_open
        || session.data_ready_acknowledged
        || session.pending_downlink.is_empty()
    {
        return true;
    }
    if session.address_management_pending || session.traffic_setup_pending {
        return true;
    }
    let traffic_ready = session.traffic_open
        && session.session_configuration_complete
        && session.traffic_configuration_complete;
    let idle_control_ready = !session.traffic_open && session.session_configuration_complete;
    if !traffic_ready && !idle_control_ready {
        info!(
            "HRPD AN A8: deferring DefaultPacket DataReady UATI=0x{uati:08x}; waiting for configured traffic or idle control buffered_chunks={}",
            session.pending_downlink.len()
        );
        return true;
    }
    if let Some(last_sent) = session.data_ready_last_sent
        && last_sent.elapsed() < HRPD_DATA_READY_RETRY_INTERVAL
    {
        return true;
    }
    let new_data_ready = session.data_ready_outstanding.is_none();
    let transaction_id = session
        .data_ready_outstanding
        .unwrap_or(session.data_ready_transaction);
    let data_ready_payload = [HRPD_DEFAULT_PACKET_DATA_READY, transaction_id];
    let target_ati = hrpd_air::AccessTerminalIdentifier {
        ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
        value: uati,
    };
    if forward_signaling_tx
        .send(hrpd_air::HrpdForwardSignalingRequest {
            uati: Some(uati),
            target_ati,
            protocol_type: session.default_packet_protocol_type,
            payload: data_ready_payload.to_vec(),
            channel: hrpd_air::HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        })
        .is_err()
    {
        return false;
    }
    let mut queued_ftc = None;
    if traffic_ready && let Some((drc_index, physical_bits)) = hrpd_a8_live_forward_rate(session) {
        let payload = match cdma_common::hrpd::traffic::default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
            physical_bits,
            session.default_packet_protocol_type,
            &data_ready_payload,
            None,
            false,
            None,
            session.forward_traffic_mac_subtype,
        ) {
            Ok(payload) => payload,
            Err(err) => {
                warn!(
                    "HRPD AN A8: failed to build FTC DefaultPacket DataReady UATI=0x{uati:08x}: {err:?}"
                );
                Vec::new()
            }
        };
        if !payload.is_empty() {
            if forward_traffic_tx
                .send(HrpdAnForwardTrafficPacket {
                    mac_index: session.mac_index,
                    physical_layer_subtype: session.physical_layer_subtype,
                    forward_traffic_mac_subtype: session.forward_traffic_mac_subtype,
                    high_priority: false,
                    payload,
                })
                .is_err()
            {
                return false;
            }
            queued_ftc = Some((drc_index, physical_bits));
        }
    }
    if let Some((drc_index, physical_bits)) = queued_ftc {
        info!(
            "HRPD AN A8: queueing DefaultPacket DataReady msg_id=0x{HRPD_DEFAULT_PACKET_DATA_READY:02x} UATI=0x{uati:08x} session_uati=0x{:08x} stream={} protocol=0x{:02x} transaction=0x{transaction_id:02x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} channel=cc+ftc drc=0x{drc_index:x} physical_bits={physical_bits} buffered_chunks={}",
            session.session_uati,
            session.default_packet_stream_id,
            session.default_packet_protocol_type,
            session.physical_layer_subtype,
            session.forward_traffic_mac_subtype,
            session.pending_downlink.len(),
        );
    } else {
        info!(
            "HRPD AN A8: queueing DefaultPacket DataReady msg_id=0x{HRPD_DEFAULT_PACKET_DATA_READY:02x} UATI=0x{uati:08x} session_uati=0x{:08x} stream={} protocol=0x{:02x} transaction=0x{transaction_id:02x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} channel=cc buffered_chunks={}",
            session.session_uati,
            session.default_packet_stream_id,
            session.default_packet_protocol_type,
            session.physical_layer_subtype,
            session.forward_traffic_mac_subtype,
            session.pending_downlink.len(),
        );
    }
    session.data_ready_last_sent = Some(Instant::now());
    session.data_ready_outstanding = Some(transaction_id);
    session.data_ready_last_transaction_sent = Some(transaction_id);
    session.data_ready_acknowledged = false;
    if new_data_ready {
        session.data_ready_transaction = session.data_ready_transaction.wrapping_add(1);
    }
    true
}

// A DataReadyAck can arrive after a traffic close/reopen cleared the
// outstanding transaction; matching the last transaction actually sent lets
// the late ACK still stop retransmission (its only effect).
fn hrpd_a8_data_ready_ack_matches(session: &HrpdAnA8Session, transaction_id: u8) -> bool {
    session.data_ready_outstanding == Some(transaction_id)
        || session.data_ready_last_transaction_sent == Some(transaction_id)
}

struct HrpdA8RlpQueueParams {
    drc_index: u8,
    physical_bits: usize,
    max_segments: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HrpdA8RlpQueueStatus {
    Queued,
    Skipped,
    Closed,
}

// Segments per forward packet: each 120-octet segment frames to 992 bits
// (see HRPD_A8_FORWARD_MAX_STREAM_OCTETS), so a packet carries
// floor(security_capacity / 992) segments. For the default MAC that is one
// segment per 1024-bit MAC packet; for the enhanced MAC the whole physical
// packet is one MAC packet (capacities 992/2016/3040/4064/5088 bits).
fn hrpd_a8_default_ftc_rlp_segments_per_packet(
    physical_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> usize {
    match forward_traffic_mac_subtype {
        cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT => match physical_bits {
            1024 => 1,
            2048 => 2,
            3072 => 3,
            4096 => 4,
            _ => 1,
        },
        cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED => match physical_bits {
            1024 => 1,
            2048 => 2,
            3072 => 3,
            4096 => 4,
            5120 => 5,
            _ => 1,
        },
        _ => 1,
    }
}

fn hrpd_a8_hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    use std::fmt::Write as _;

    let show = bytes.len().min(max_bytes);
    let mut out = String::with_capacity(show * 2 + usize::from(bytes.len() > show) * 3);
    for byte in &bytes[..show] {
        let _ = write!(&mut out, "{byte:02x}");
    }
    if bytes.len() > show {
        out.push_str("...");
    }
    out
}

fn queue_hrpd_a8_default_packet_rlp_segments(
    uati: u32,
    session: &mut HrpdAnA8Session,
    forward_traffic_tx: &tokio::sync::mpsc::UnboundedSender<HrpdAnForwardTrafficPacket>,
    segments: &[HrpdA8QueuedRlpSegment],
    params: HrpdA8RlpQueueParams,
) -> HrpdA8RlpQueueStatus {
    if segments.is_empty() {
        return HrpdA8RlpQueueStatus::Skipped;
    }
    let rlp_packets = segments
        .iter()
        .map(|segment| (segment.sequence, segment.octets.as_slice()))
        .collect::<Vec<_>>();
    let payload =
        match cdma_common::hrpd::traffic::default_packet_ftc_payload_bits_many_for_mac_subtype(
            session.default_packet_stream_id,
            &rlp_packets,
            params.physical_bits,
            session.forward_traffic_mac_subtype,
        ) {
            Ok(payload) => payload,
            Err(err) => {
                warn!("HRPD AN A8: failed to build DefaultPacket RLP UATI=0x{uati:08x}: {err:?}");
                return HrpdA8RlpQueueStatus::Skipped;
            }
        };
    if forward_traffic_tx
        .send(HrpdAnForwardTrafficPacket {
            mac_index: session.mac_index,
            physical_layer_subtype: session.physical_layer_subtype,
            forward_traffic_mac_subtype: session.forward_traffic_mac_subtype,
            high_priority: segments.iter().any(|segment| segment.retransmission),
            payload,
        })
        .is_err()
    {
        return HrpdA8RlpQueueStatus::Closed;
    }
    for segment in segments {
        if !segment.retransmission {
            record_hrpd_rlp_history(session, segment.sequence, &segment.octets);
        }
    }
    let octets = segments
        .iter()
        .map(|segment| segment.octets.len())
        .sum::<usize>();
    let mut retransmissions = 0usize;
    let mut retransmit_octets = 0usize;
    let mut new_octets = 0usize;
    for segment in segments {
        if segment.retransmission {
            retransmissions += 1;
            retransmit_octets += segment.octets.len();
        } else {
            new_octets += segment.octets.len();
        }
    }
    session.traffic_window_stats.forward_packets += 1;
    session.traffic_window_stats.rlp_segments += segments.len();
    if segments.len() == params.max_segments {
        session.traffic_window_stats.rlp_full_packets += 1;
    } else {
        session.traffic_window_stats.rlp_partial_packets += 1;
        if retransmissions == 0 {
            session.traffic_window_stats.rlp_partial_new_packets += 1;
        } else if retransmissions == segments.len() {
            session.traffic_window_stats.rlp_partial_retx_packets += 1;
        } else {
            session.traffic_window_stats.rlp_partial_mixed_packets += 1;
        }
    }
    session.traffic_window_stats.rlp_new_octets += new_octets;
    session.traffic_window_stats.rlp_retx_octets += retransmit_octets;
    log::log!(
        if retransmissions == 0 {
            log::Level::Trace
        } else {
            log::Level::Debug
        },
        "HRPD AN A8: queueing DefaultPacket RLP UATI=0x{uati:08x} mac_index={} stream={} protocol=0x{:02x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} drc=0x{:x} physical_bits={} segments={} retransmissions={} first_seq={} first_octets={} octets={} buffered_after={} pending_retx_segments={} pending_retx_requests={}",
        session.mac_index,
        session.default_packet_stream_id,
        session.default_packet_protocol_type,
        session.physical_layer_subtype,
        session.forward_traffic_mac_subtype,
        params.drc_index,
        params.physical_bits,
        segments.len(),
        retransmissions,
        segments[0].sequence,
        hrpd_a8_hex_preview(&segments[0].octets, 32),
        octets,
        session.pending_downlink.len(),
        session.pending_retransmit.len(),
        session.pending_retransmit_requests.len()
    );
    HrpdA8RlpQueueStatus::Queued
}

fn restore_hrpd_a8_rlp_segments(
    session: &mut HrpdAnA8Session,
    segments: Vec<HrpdA8QueuedRlpSegment>,
) {
    for segment in segments.into_iter().rev() {
        if segment.retransmission {
            session
                .pending_retransmit
                .push_front(HrpdRlpRetransmitSegment {
                    sequence: segment.sequence,
                    octets: segment.octets,
                });
        } else {
            session.pending_downlink.push_front(segment.octets);
        }
    }
}

fn hrpd_a8_should_defer_partial_new_packet(
    session: &mut HrpdAnA8Session,
    segments: &[HrpdA8QueuedRlpSegment],
    max_segments: usize,
) -> bool {
    let has_fillable_tail = segments.split_last().is_some_and(|(last, preceding)| {
        last.octets.len() < HRPD_A8_FORWARD_MAX_STREAM_OCTETS
            && preceding
                .iter()
                .all(|segment| segment.octets.len() == HRPD_A8_FORWARD_MAX_STREAM_OCTETS)
    });
    let partial = segments.len() < max_segments || has_fillable_tail;
    if !(max_segments > 1
        && !segments.is_empty()
        && partial
        && session.pending_downlink.is_empty()
        && segments.iter().all(|segment| !segment.retransmission))
    {
        return false;
    }

    session
        .pending_downlink_partial_hold_at
        .get_or_insert_with(Instant::now)
        .elapsed()
        < HRPD_A8_PARTIAL_PACKET_COALESCE_DELAY
}

fn flush_hrpd_a8_downlink(
    uati: u32,
    session: &mut HrpdAnA8Session,
    forward_signaling_tx: &tokio::sync::mpsc::UnboundedSender<
        hrpd_air::HrpdForwardSignalingRequest,
    >,
    forward_traffic_tx: &tokio::sync::mpsc::UnboundedSender<HrpdAnForwardTrafficPacket>,
) -> bool {
    if session.address_management_pending {
        return true;
    }
    if session.traffic_setup_pending {
        return true;
    }
    if !session.traffic_open {
        return send_hrpd_a8_data_ready(uati, session, forward_signaling_tx, forward_traffic_tx)
            && send_hrpd_a8_open_connection_page(uati, session, forward_signaling_tx);
    }
    if !session.session_configuration_complete || !session.traffic_configuration_complete {
        info!(
            "HRPD AN A8: deferring open-traffic DefaultPacket work UATI=0x{uati:08x}; waiting for current traffic SessionConfigurationComplete buffered_chunks={}",
            session.pending_downlink.len()
        );
        return true;
    }
    if !session.default_packet_flow_open {
        return send_hrpd_a8_data_ready(uati, session, forward_signaling_tx, forward_traffic_tx);
    }
    let mut sent_packets = 0usize;
    while (!session.pending_retransmit.is_empty()
        || !session.pending_retransmit_requests.is_empty()
        || !session.pending_downlink.is_empty())
        && sent_packets < HRPD_A8_FORWARD_MAX_PACKETS_PER_FLUSH
    {
        let Some((drc_index, physical_bits)) = hrpd_a8_live_forward_rate(session) else {
            session.traffic_window_stats.flush_drc_starved += 1;
            info!(
                "HRPD AN A8: deferring DefaultPacket RLP UATI=0x{uati:08x}; no valid live DRC buffered_chunks={} pending_retx_segments={} pending_retx_requests={}",
                session.pending_downlink.len(),
                session.pending_retransmit.len(),
                session.pending_retransmit_requests.len()
            );
            return true;
        };
        let max_segments = hrpd_a8_default_ftc_rlp_segments_per_packet(
            physical_bits,
            session.forward_traffic_mac_subtype,
        );
        let mut segments = Vec::with_capacity(max_segments);
        while segments.len() < max_segments {
            if let Some(segment) = session
                .pending_retransmit
                .pop_front()
                .or_else(|| materialize_next_hrpd_rlp_retransmission(uati, session))
            {
                segments.push(HrpdA8QueuedRlpSegment {
                    sequence: segment.sequence,
                    octets: segment.octets,
                    retransmission: true,
                });
                continue;
            }
            let Some(chunk) = session.pending_downlink.pop_front() else {
                break;
            };
            let sequence = session.rlp_seq.wrapping_add(
                segments
                    .iter()
                    .filter(|segment| !segment.retransmission)
                    .map(|segment| segment.octets.len() as u32)
                    .sum::<u32>(),
            ) & rlp::SEQUENCE_MASK;
            segments.push(HrpdA8QueuedRlpSegment {
                sequence,
                octets: chunk,
                retransmission: false,
            });
        }
        if hrpd_a8_should_defer_partial_new_packet(session, &segments, max_segments) {
            session.traffic_window_stats.rlp_partial_deferred += 1;
            restore_hrpd_a8_rlp_segments(session, segments);
            return true;
        }
        match queue_hrpd_a8_default_packet_rlp_segments(
            uati,
            session,
            forward_traffic_tx,
            &segments,
            HrpdA8RlpQueueParams {
                drc_index,
                physical_bits,
                max_segments,
            },
        ) {
            HrpdA8RlpQueueStatus::Queued => {
                let sent_new_octets = segments
                    .iter()
                    .filter(|segment| !segment.retransmission)
                    .map(|segment| segment.octets.len() as u32)
                    .sum::<u32>();
                session.rlp_seq =
                    session.rlp_seq.wrapping_add(sent_new_octets) & rlp::SEQUENCE_MASK;
                sent_packets += 1;
            }
            HrpdA8RlpQueueStatus::Skipped => {
                restore_hrpd_a8_rlp_segments(session, segments);
                return true;
            }
            HrpdA8RlpQueueStatus::Closed => return false,
        }
    }
    if sent_packets > 0 {
        if session.pending_downlink.is_empty()
            && session.pending_retransmit.is_empty()
            && session.pending_retransmit_requests.is_empty()
        {
            session.traffic_window_stats.flush_source_drained += 1;
        } else if sent_packets == HRPD_A8_FORWARD_MAX_PACKETS_PER_FLUSH {
            session.traffic_window_stats.flush_capacity_limited += 1;
        }
    }
    if session.pending_downlink.is_empty() {
        session.pending_downlink_partial_hold_at = None;
    }
    true
}

fn acknowledge_hrpd_a8_data_ready(
    uati: u32,
    request_uati: u32,
    transaction_id: u8,
    session: &mut HrpdAnA8Session,
    forward_signaling_tx: &tokio::sync::mpsc::UnboundedSender<
        hrpd_air::HrpdForwardSignalingRequest,
    >,
    forward_traffic_tx: &tokio::sync::mpsc::UnboundedSender<HrpdAnForwardTrafficPacket>,
) -> bool {
    // DataReadyAck only acknowledges receipt of DataReady (C.S0024-A
    // §3.6.4.1.1): stop retransmitting it, but keep the flow closed. The AN
    // instance leaves Close State only on Rx XonRequest or reverse RLP
    // (§3.6.4.1.2.2), and the live handset does send XonRequest.
    session.data_ready_acknowledged = true;
    session.data_ready_outstanding = None;
    session.data_ready_last_transaction_sent = None;
    session.data_ready_last_sent = None;
    info!(
        "HRPD AN A8: DefaultPacket DataReady ACKed UATI=0x{uati:08x} request_uati=0x{request_uati:08x} transaction=0x{transaction_id:02x} buffered_chunks={} packet_flow_open={}",
        session.pending_downlink.len(),
        session.default_packet_flow_open,
    );
    flush_hrpd_a8_downlink(uati, session, forward_signaling_tx, forward_traffic_tx)
}

fn hrpd_a8_periodic_flush_ready(session: &HrpdAnA8Session) -> bool {
    if (session.pending_downlink.is_empty()
        && session.pending_retransmit.is_empty()
        && session.pending_retransmit_requests.is_empty())
        || session.address_management_pending
        || session.traffic_setup_pending
    {
        return false;
    }
    if !session.traffic_open {
        return session.session_configuration_complete;
    }
    if !session.session_configuration_complete || !session.traffic_configuration_complete {
        return false;
    }
    if !session.default_packet_flow_open {
        return hrpd_a8_live_forward_rate(session).is_some();
    }
    hrpd_a8_live_forward_rate(session).is_some()
}

fn migrate_closed_hrpd_a8_downlink(
    target_uati: u32,
    table: &mut cdma_a8::BearerTable,
    sessions: &mut HashMap<u32, HrpdAnA8Session>,
    pending_traffic_open: &mut HashSet<u32>,
    pending_traffic_setup: &mut HashSet<u32>,
) -> usize {
    migrate_hrpd_a8_downlink(
        target_uati,
        table,
        sessions,
        pending_traffic_open,
        pending_traffic_setup,
        false,
    )
}

fn migrate_hrpd_a8_downlink(
    target_uati: u32,
    table: &mut cdma_a8::BearerTable,
    sessions: &mut HashMap<u32, HrpdAnA8Session>,
    pending_traffic_open: &mut HashSet<u32>,
    pending_traffic_setup: &mut HashSet<u32>,
    include_open_sessions: bool,
) -> usize {
    let stale_uatis: Vec<u32> = sessions
        .iter()
        .filter_map(|(&uati, session)| {
            (uati != target_uati
                && (include_open_sessions || !session.traffic_open)
                && !session.pending_downlink.is_empty())
            .then_some(uati)
        })
        .collect();
    if stale_uatis.is_empty() {
        return 0;
    }

    let mut pending_downlink = VecDeque::new();
    let mut migrated_chunks = 0usize;
    let mut removed_uatis = Vec::new();
    let mut target_mac_index = None;
    let mut rekey_bearer = None;
    let mut stale_inbound_keys = Vec::new();
    for stale_uati in stale_uatis {
        if let Some(mut stale_session) = sessions.remove(&stale_uati) {
            migrated_chunks += stale_session.pending_downlink.len();
            pending_downlink.append(&mut stale_session.pending_downlink);
            target_mac_index.get_or_insert(stale_session.mac_index);
            pending_traffic_open.remove(&stale_uati);
            pending_traffic_setup.remove(&stale_uati);
            if let Some(bearer) = table.remove_session_if_present(stale_uati) {
                stale_inbound_keys.push(bearer.inbound_session_key);
                if rekey_bearer.is_none() && !table.has_session(target_uati) {
                    rekey_bearer = Some(bearer);
                }
            }
            removed_uatis.push(format!("0x{stale_uati:08x}"));
        }
    }

    if migrated_chunks > 0 {
        if let Some(bearer) = rekey_bearer {
            let rebound = cdma_a8::BearerSession::with_directional_keys(
                target_uati,
                bearer.inbound_session_key,
                bearer.outbound_session_key,
                bearer.endpoint,
                bearer.profile,
            );
            match table.apply_session(rebound) {
                Ok(outcome) => info!(
                    "HRPD AN A8: rebound bearer session from stale downlink to UATI=0x{target_uati:08x} outcome={outcome:?}"
                ),
                Err(err) => warn!(
                    "HRPD AN A8: failed to rebound bearer session to UATI=0x{target_uati:08x}: {err}"
                ),
            }
        }
        for key in stale_inbound_keys {
            match table.add_inbound_key_alias(target_uati, key) {
                Ok(()) => info!(
                    "HRPD AN A8: retained stale inbound GRE key 0x{key:08x} for live UATI=0x{target_uati:08x}"
                ),
                Err(err) => warn!(
                    "HRPD AN A8: failed to retain stale inbound GRE key 0x{key:08x} for live UATI=0x{target_uati:08x}: {err}"
                ),
            }
        }
        let target = sessions
            .entry(target_uati)
            .or_insert_with(|| HrpdAnA8Session::new(target_uati, target_mac_index.unwrap_or(0)));
        target.traffic_setup_pending = pending_traffic_setup.contains(&target_uati);
        target.pending_downlink.append(&mut pending_downlink);
        info!(
            "HRPD AN A8: migrated {migrated_chunks} buffered downlink chunk(s) from stale UATI(s) [{}] to UATI=0x{target_uati:08x} include_open_sessions={include_open_sessions}",
            removed_uatis.join(",")
        );
    }
    migrated_chunks
}

fn hrpd_a8_session_key_for_uati_alias(
    sessions: &HashMap<u32, HrpdAnA8Session>,
    uati: u32,
) -> Option<u32> {
    if sessions.contains_key(&uati) {
        return Some(uati);
    }
    sessions
        .iter()
        .find_map(|(key, session)| (session.session_uati == uati).then_some(*key))
}

fn hrpd_a8_retarget_target_uati(sessions: &HashMap<u32, HrpdAnA8Session>, uati: u32) -> u32 {
    hrpd_a8_session_key_for_uati_alias(sessions, uati).unwrap_or(uati)
}

fn hrpd_a8_retry_delay(last_sent: Option<Instant>, interval: Duration) -> Duration {
    last_sent
        .map(|sent| interval.saturating_sub(sent.elapsed()))
        .unwrap_or(Duration::ZERO)
}

fn hrpd_a8_next_flush_delay(session: &HrpdAnA8Session) -> Option<Duration> {
    if !hrpd_a8_periodic_flush_ready(session) {
        return None;
    }

    if !session.traffic_open {
        if session.pending_downlink.is_empty() {
            return None;
        }
        let open_delay = hrpd_a8_retry_delay(
            session.open_connection_last_sent,
            hrpd_open_connection_retry_interval(session),
        );
        if session.default_packet_flow_open || session.data_ready_acknowledged {
            return Some(open_delay);
        }
        let data_ready_delay =
            hrpd_a8_retry_delay(session.data_ready_last_sent, HRPD_DATA_READY_RETRY_INTERVAL);
        return Some(open_delay.min(data_ready_delay));
    }

    if !session.default_packet_flow_open {
        if session.pending_downlink.is_empty() || session.data_ready_acknowledged {
            return None;
        }
        return Some(hrpd_a8_retry_delay(
            session.data_ready_last_sent,
            HRPD_DATA_READY_RETRY_INTERVAL,
        ));
    }

    Some(
        session
            .pending_downlink_partial_hold_at
            .map(|held_at| HRPD_A8_PARTIAL_PACKET_COALESCE_DELAY.saturating_sub(held_at.elapsed()))
            .unwrap_or(Duration::ZERO),
    )
}

fn hrpd_a8_next_runtime_wakeup(sessions: &HashMap<u32, HrpdAnA8Session>) -> Option<Duration> {
    sessions.values().filter_map(hrpd_a8_next_flush_delay).min()
}

struct HrpdAnA8Actor {
    table: cdma_a8::BearerTable,
    sessions: HashMap<u32, HrpdAnA8Session>,
    pending_traffic_open: HashSet<u32>,
    pending_traffic_setup: HashSet<u32>,
    pending_traffic_configuration: HashSet<u32>,
    pending_session_configuration_complete: HashMap<u32, HrpdSessionConfigurationState>,
    pending_drc_by_uati: HashMap<u32, (u8, Instant)>,
    pending_default_packet_stream_by_uati: HashMap<u32, (u8, u8)>,
    pending_default_packet_flow_open: HashSet<u32>,
    uplink_timing_started: Instant,
    uplink_timing_samples: u64,
    uplink_timing_octets: u64,
    uplink_queue_us_sum: u128,
    uplink_queue_us_max: u128,
    uplink_send_us_sum: u128,
    uplink_send_us_max: u128,
    pending_command: Option<HrpdAnA8Command>,
    buf: Vec<u8>,
}

impl HrpdAnA8Actor {
    fn new() -> Self {
        Self {
            table: cdma_a8::BearerTable::new(),
            sessions: HashMap::new(),
            pending_traffic_open: HashSet::new(),
            pending_traffic_setup: HashSet::new(),
            pending_traffic_configuration: HashSet::new(),
            pending_session_configuration_complete: HashMap::new(),
            pending_drc_by_uati: HashMap::new(),
            pending_default_packet_stream_by_uati: HashMap::new(),
            pending_default_packet_flow_open: HashSet::new(),
            uplink_timing_started: Instant::now(),
            uplink_timing_samples: 0,
            uplink_timing_octets: 0,
            uplink_queue_us_sum: 0,
            uplink_queue_us_max: 0,
            uplink_send_us_sum: 0,
            uplink_send_us_max: 0,
            pending_command: None,
            buf: vec![0u8; 8192],
        }
    }

    async fn run(
        mut self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<HrpdAnA8Command>,
        bearer: cdma_a8::TokioUdpGreEndpoint,
        endpoint: cdma_a8::BearerEndpoint,
        forward_signaling_tx: tokio::sync::mpsc::UnboundedSender<
            hrpd_air::HrpdForwardSignalingRequest,
        >,
        forward_traffic_tx: tokio::sync::mpsc::UnboundedSender<HrpdAnForwardTrafficPacket>,
    ) {
        info!("HRPD AN A8 bearer listener started");
        loop {
            let mut pass_active = false;
            while let Some(command) = self.pending_command.take().or_else(|| rx.try_recv().ok()) {
                pass_active = true;
                match command {
                    HrpdAnA8Command::Register {
                        session_uati,
                        uati,
                        mac_index,
                        bearer,
                    } => {
                        let new_inbound_session_key = bearer.inbound_session_key;
                        match self.table.apply_session(bearer) {
                            Ok(outcome) => {
                                if let cdma_a8::ApplySessionOutcome::Rebound {
                                    previous_inbound_session_key,
                                    ..
                                } = outcome
                                    && previous_inbound_session_key != new_inbound_session_key
                                {
                                    match self
                                        .table
                                        .add_inbound_key_alias(uati, previous_inbound_session_key)
                                    {
                                        Ok(()) => info!(
                                            "HRPD AN A8: retained rebound inbound GRE key 0x{previous_inbound_session_key:08x} as alias for UATI=0x{uati:08x}"
                                        ),
                                        Err(err) => warn!(
                                            "HRPD AN A8: failed to retain rebound inbound GRE key 0x{previous_inbound_session_key:08x} as alias for UATI=0x{uati:08x}: {err}"
                                        ),
                                    }
                                }
                                let traffic_open = self.pending_traffic_open.contains(&uati);
                                let traffic_setup_pending =
                                    self.pending_traffic_setup.contains(&uati) && !traffic_open;
                                let session_configuration_subtypes = self
                                    .pending_session_configuration_complete
                                    .get(&uati)
                                    .copied();
                                let session_configuration_complete =
                                    session_configuration_subtypes.is_some();
                                if session_configuration_complete {
                                    self.pending_traffic_configuration.remove(&uati);
                                }
                                let traffic_configuration_pending =
                                    self.pending_traffic_configuration.contains(&uati);
                                let default_packet_stream = hrpd_a8_pending_default_packet_stream(
                                    &self.pending_default_packet_stream_by_uati,
                                    uati,
                                    session_uati,
                                );
                                let default_packet_flow_open = if hrpd_a8_pending_uati_alias(
                                    &self.pending_default_packet_flow_open,
                                    uati,
                                    session_uati,
                                ) {
                                    hrpd_a8_take_pending_uati_alias(
                                        &mut self.pending_default_packet_flow_open,
                                        uati,
                                        session_uati,
                                    )
                                } else {
                                    false
                                };
                                let session = self
                                    .sessions
                                    .entry(uati)
                                    .and_modify(|session| {
                                        session.session_uati = session_uati;
                                        session.mac_index = mac_index;
                                        session.traffic_open = traffic_open;
                                        session.traffic_setup_pending = traffic_setup_pending;
                                        if let Some((drc_index, drc_at)) =
                                            self.pending_drc_by_uati.get(&uati).copied()
                                        {
                                            session.last_drc_index = Some(drc_index);
                                            session.last_drc_at = Some(drc_at);
                                        } else {
                                            session.last_drc_index = None;
                                            session.last_drc_at = None;
                                        }
                                        session.open_connection_last_sent = None;
                                        // DefaultPacket flow-control is an
                                        // application state. Traffic channel
                                        // churn must not undo an Xon; explicit
                                        // Xoff/session reset handles close.
                                        if default_packet_flow_open {
                                            session.default_packet_flow_open = true;
                                            session.data_ready_last_sent = None;
                                            session.data_ready_outstanding = None;
                                            session.data_ready_acknowledged = false;
                                        }
                                        if session_configuration_complete {
                                            session.session_configuration_complete = true;
                                            if let Some(config) = session_configuration_subtypes {
                                                session.physical_layer_subtype =
                                                    config.physical_layer_subtype;
                                                session.forward_traffic_mac_subtype =
                                                    config.forward_traffic_mac_subtype;
                                                session.idle_preferred_control_channel_cycle =
                                                    config.idle_preferred_control_channel_cycle;
                                                session.idle_page_period_cycles =
                                                    config.idle_page_period_cycles;
                                            }
                                        }
                                        if let Some((stream_id, protocol_type)) =
                                            default_packet_stream
                                        {
                                            session.default_packet_stream_id = stream_id;
                                            session.default_packet_protocol_type = protocol_type;
                                        }
                                        session.traffic_configuration_complete = traffic_open
                                            && !traffic_setup_pending
                                            && hrpd_a8_traffic_configuration_complete_on_open(
                                                session,
                                                traffic_configuration_pending,
                                            );
                                    })
                                    .or_insert_with(|| {
                                        HrpdAnA8Session::new(session_uati, mac_index)
                                    });
                                session.session_uati = session_uati;
                                session.traffic_open = traffic_open;
                                session.traffic_setup_pending = traffic_setup_pending;
                                if let Some((drc_index, drc_at)) =
                                    self.pending_drc_by_uati.get(&uati).copied()
                                {
                                    session.last_drc_index = Some(drc_index);
                                    session.last_drc_at = Some(drc_at);
                                } else {
                                    session.last_drc_index = None;
                                    session.last_drc_at = None;
                                }
                                if session_configuration_complete {
                                    session.session_configuration_complete = true;
                                    if let Some(config) = session_configuration_subtypes {
                                        session.physical_layer_subtype =
                                            config.physical_layer_subtype;
                                        session.forward_traffic_mac_subtype =
                                            config.forward_traffic_mac_subtype;
                                        session.idle_preferred_control_channel_cycle =
                                            config.idle_preferred_control_channel_cycle;
                                        session.idle_page_period_cycles =
                                            config.idle_page_period_cycles;
                                    }
                                }
                                if let Some((stream_id, protocol_type)) = default_packet_stream {
                                    session.default_packet_stream_id = stream_id;
                                    session.default_packet_protocol_type = protocol_type;
                                }
                                if default_packet_flow_open {
                                    session.default_packet_flow_open = true;
                                    session.data_ready_last_sent = None;
                                    session.data_ready_outstanding = None;
                                    session.data_ready_acknowledged = false;
                                }
                                session.traffic_configuration_complete = traffic_open
                                    && !traffic_setup_pending
                                    && hrpd_a8_traffic_configuration_complete_on_open(
                                        session,
                                        traffic_configuration_pending,
                                    );
                                info!(
                                    "HRPD AN A8: registered UATI=0x{uati:08x} mac_index={mac_index} traffic_open={traffic_open} traffic_setup_pending={traffic_setup_pending} session_config_complete={} traffic_config_complete={} last_drc={} outcome={outcome:?}",
                                    session.session_configuration_complete,
                                    session.traffic_configuration_complete,
                                    session
                                        .last_drc_index
                                        .map(|drc| format!("0x{drc:x}"))
                                        .unwrap_or_else(|| "none".to_string())
                                );
                                let migrated_chunks = migrate_closed_hrpd_a8_downlink(
                                    uati,
                                    &mut self.table,
                                    &mut self.sessions,
                                    &mut self.pending_traffic_open,
                                    &mut self.pending_traffic_setup,
                                );
                                if let Some(session) = self.sessions.get_mut(&uati)
                                    && (traffic_open || migrated_chunks > 0)
                                    && !flush_hrpd_a8_downlink(
                                        uati,
                                        session,
                                        &forward_signaling_tx,
                                        &forward_traffic_tx,
                                    )
                                {
                                    warn!("HRPD AN A8: BTS forward queue closed");
                                    return;
                                }
                            }
                            Err(err) => {
                                warn!("HRPD AN A8: failed to register UATI=0x{uati:08x}: {err}")
                            }
                        }
                    }
                    HrpdAnA8Command::Release {
                        uati,
                        retain_session_configuration,
                    } => {
                        let removed_session = self.sessions.remove(&uati);
                        let retain_default_packet_flow_open = retain_session_configuration
                            && removed_session
                                .as_ref()
                                .is_some_and(|session| session.default_packet_flow_open);
                        if retain_session_configuration {
                            if let Some(session) = removed_session.as_ref() {
                                if session.session_configuration_complete {
                                    self.pending_session_configuration_complete.insert(
                                        uati,
                                        HrpdSessionConfigurationState {
                                            physical_layer_subtype: session.physical_layer_subtype,
                                            forward_traffic_mac_subtype: session
                                                .forward_traffic_mac_subtype,
                                            idle_preferred_control_channel_cycle: session
                                                .idle_preferred_control_channel_cycle,
                                            idle_page_period_cycles: session
                                                .idle_page_period_cycles,
                                        },
                                    );
                                }
                                self.pending_default_packet_stream_by_uati.insert(
                                    uati,
                                    (
                                        session.default_packet_stream_id,
                                        session.default_packet_protocol_type,
                                    ),
                                );
                                if retain_default_packet_flow_open {
                                    self.pending_default_packet_flow_open
                                        .insert(session.session_uati);
                                }
                            }
                        } else {
                            self.pending_session_configuration_complete.remove(&uati);
                            self.pending_default_packet_stream_by_uati.remove(&uati);
                        }
                        if let Some(session) = removed_session.as_ref() {
                            if !retain_default_packet_flow_open {
                                self.pending_default_packet_flow_open
                                    .remove(&session.session_uati);
                            }
                        }
                        self.pending_traffic_open.remove(&uati);
                        self.pending_traffic_setup.remove(&uati);
                        self.pending_traffic_configuration.remove(&uati);
                        self.pending_drc_by_uati.remove(&uati);
                        if retain_default_packet_flow_open {
                            self.pending_default_packet_flow_open.insert(uati);
                        } else {
                            self.pending_default_packet_flow_open.remove(&uati);
                        }
                        let removed_bearer = self.table.remove_session_if_present(uati);
                        info!(
                            "HRPD AN A8: released UATI=0x{uati:08x} buffered_chunks={} bearer_removed={} retained_session_config={retain_session_configuration}",
                            removed_session
                                .as_ref()
                                .map(|session| session.pending_downlink.len())
                                .unwrap_or(0),
                            removed_bearer.is_some()
                        );
                    }
                    HrpdAnA8Command::SendUplink {
                        uati,
                        payload,
                        queued_at,
                    } => {
                        if payload.is_empty() {
                            continue;
                        }
                        let queue_elapsed = queued_at.elapsed();
                        let Some(session) = self.sessions.get(&uati) else {
                            warn!("HRPD AN A8: dropping uplink for unregistered UATI=0x{uati:08x}");
                            continue;
                        };
                        let payload_len = payload.len();
                        let outbound = match self.table.build_outbound_packet(uati, payload) {
                            Ok(outbound) => outbound,
                            Err(err) => {
                                warn!(
                                    "HRPD AN A8: failed to encode uplink UATI=0x{uati:08x}: {err}"
                                );
                                continue;
                            }
                        };
                        let send_started = Instant::now();
                        if let Err(err) = bearer.send_wire_packet(&outbound.wire_bytes).await {
                            warn!(
                                "HRPD AN A8: failed to send uplink UATI=0x{uati:08x} mac_index={}: {err}",
                                session.mac_index
                            );
                        }
                        let send_elapsed = send_started.elapsed();
                        self.uplink_timing_samples += 1;
                        self.uplink_timing_octets += payload_len as u64;
                        self.uplink_queue_us_sum += queue_elapsed.as_micros();
                        self.uplink_queue_us_max =
                            self.uplink_queue_us_max.max(queue_elapsed.as_micros());
                        self.uplink_send_us_sum += send_elapsed.as_micros();
                        self.uplink_send_us_max =
                            self.uplink_send_us_max.max(send_elapsed.as_micros());
                        if self.uplink_timing_started.elapsed() >= Duration::from_secs(5) {
                            debug!(
                                "HRPD AN A8 uplink timing: samples={} octets={} command_queue_us_avg={:.1} command_queue_us_max={} socket_send_us_avg={:.1} socket_send_us_max={}",
                                self.uplink_timing_samples,
                                self.uplink_timing_octets,
                                self.uplink_queue_us_sum as f64 / self.uplink_timing_samples as f64,
                                self.uplink_queue_us_max,
                                self.uplink_send_us_sum as f64 / self.uplink_timing_samples as f64,
                                self.uplink_send_us_max,
                            );
                            self.uplink_timing_started = Instant::now();
                            self.uplink_timing_samples = 0;
                            self.uplink_timing_octets = 0;
                            self.uplink_queue_us_sum = 0;
                            self.uplink_queue_us_max = 0;
                            self.uplink_send_us_sum = 0;
                            self.uplink_send_us_max = 0;
                        }
                    }
                    HrpdAnA8Command::SetTrafficChannelOpen { uati, open } => {
                        let Some(session) = self.sessions.get_mut(&uati) else {
                            if open {
                                self.pending_traffic_open.insert(uati);
                                self.pending_traffic_setup.remove(&uati);
                                self.pending_traffic_configuration.insert(uati);
                            } else {
                                self.pending_traffic_open.remove(&uati);
                                self.pending_traffic_setup.remove(&uati);
                                self.pending_traffic_configuration.remove(&uati);
                            }
                            warn!(
                                "HRPD AN A8: traffic-state update for unregistered UATI=0x{uati:08x} open={open}"
                            );
                            continue;
                        };
                        if open {
                            self.pending_traffic_open.insert(uati);
                            self.pending_traffic_setup.remove(&uati);
                            hrpd_a8_update_pending_traffic_configuration_for_open(
                                uati,
                                session.session_configuration_complete,
                                &mut self.pending_traffic_configuration,
                            );
                        } else {
                            self.pending_traffic_open.remove(&uati);
                            self.pending_traffic_setup.remove(&uati);
                            self.pending_traffic_configuration.remove(&uati);
                        }
                        let was_open = session.traffic_open;
                        session.traffic_open = open;
                        session.traffic_setup_pending = false;
                        if open {
                            session.traffic_configuration_complete =
                                hrpd_a8_traffic_configuration_complete_on_open(
                                    session,
                                    self.pending_traffic_configuration.contains(&uati),
                                );
                            if !was_open {
                                reset_hrpd_a8_traffic_window_stats(session);
                                // C.S0024-500-C §2.4.4.1.1.1 initializes
                                // Default Packet RLP on ConnectionOpened.
                                reset_hrpd_a8_default_packet_rlp(session);
                            }
                            session.open_connection_last_sent = None;
                            session.initial_connection_close_observed = false;
                            info!(
                                "HRPD AN A8: traffic channel open UATI=0x{uati:08x} duplicate={} buffered_chunks={} packet_flow_open={} data_ready_outstanding={:?} data_ready_acknowledged={} session_config_complete={} traffic_config_complete={}",
                                was_open,
                                session.pending_downlink.len(),
                                session.default_packet_flow_open,
                                session.data_ready_outstanding,
                                session.data_ready_acknowledged,
                                session.session_configuration_complete,
                                session.traffic_configuration_complete
                            );
                            if !flush_hrpd_a8_downlink(
                                uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            ) {
                                warn!("HRPD AN A8: BTS forward-traffic queue closed");
                                return;
                            }
                        } else {
                            if was_open {
                                log_hrpd_a8_traffic_close_summary(uati, session);
                            }
                            hrpd_a8_note_traffic_closed(session);
                            info!(
                                "HRPD AN A8: traffic channel closed UATI=0x{uati:08x} buffered_chunks={}",
                                session.pending_downlink.len()
                            );
                            if !flush_hrpd_a8_downlink(
                                uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            ) {
                                warn!("HRPD AN A8: BTS forward queue closed");
                                return;
                            }
                        }
                    }
                    HrpdAnA8Command::SetTrafficMacIndex { uati, mac_index } => {
                        let session = self
                            .sessions
                            .entry(uati)
                            .or_insert_with(|| HrpdAnA8Session::new(uati, mac_index));
                        if session.mac_index != mac_index {
                            info!(
                                "HRPD AN A8: traffic MAC retarget UATI=0x{uati:08x} old_mac={} new_mac={mac_index} buffered_chunks={}",
                                session.mac_index,
                                session.pending_downlink.len()
                            );
                            session.mac_index = mac_index;
                        }
                    }
                    HrpdAnA8Command::SetTrafficSetupPending { uati, pending } => {
                        if pending {
                            self.pending_traffic_setup.insert(uati);
                        } else {
                            self.pending_traffic_setup.remove(&uati);
                        }
                        let Some(session) = self.sessions.get_mut(&uati) else {
                            info!(
                                "HRPD AN A8: traffic setup pending for unregistered UATI=0x{uati:08x} pending={pending}"
                            );
                            continue;
                        };
                        let was_pending = session.traffic_setup_pending;
                        session.traffic_setup_pending = pending && !session.traffic_open;
                        if session.traffic_setup_pending {
                            hrpd_a8_note_traffic_setup_pending(session);
                        }
                        if session.traffic_setup_pending != was_pending {
                            info!(
                                "HRPD AN A8: traffic setup pending UATI=0x{uati:08x} pending={} buffered_chunks={}",
                                session.traffic_setup_pending,
                                session.pending_downlink.len()
                            );
                        }
                        if !session.traffic_setup_pending
                            && !flush_hrpd_a8_downlink(
                                uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            )
                        {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::SetAddressManagementPending { uati, pending } => {
                        let Some(session) = self.sessions.get_mut(&uati) else {
                            info!(
                                "HRPD AN A8: address management pending for unregistered UATI=0x{uati:08x} pending={pending}"
                            );
                            continue;
                        };
                        let was_pending = session.address_management_pending;
                        session.address_management_pending = pending && !session.traffic_open;
                        if session.address_management_pending != was_pending {
                            info!(
                                "HRPD AN A8: address management pending UATI=0x{uati:08x} pending={} buffered_chunks={}",
                                session.address_management_pending,
                                session.pending_downlink.len()
                            );
                        }
                        if !session.address_management_pending
                            && !flush_hrpd_a8_downlink(
                                uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            )
                        {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::SetTrafficConfigurationPending { uati, pending } => {
                        if pending {
                            self.pending_traffic_configuration.insert(uati);
                        } else {
                            self.pending_traffic_configuration.remove(&uati);
                        }
                        let Some(session) = self.sessions.get_mut(&uati) else {
                            info!(
                                "HRPD AN A8: traffic configuration pending for unregistered UATI=0x{uati:08x} pending={pending}"
                            );
                            continue;
                        };
                        let was_complete = session.traffic_configuration_complete;
                        if pending {
                            session.traffic_configuration_complete = false;
                            session.data_ready_last_sent = None;
                            session.data_ready_outstanding = None;
                        } else {
                            session.traffic_configuration_complete =
                                session.session_configuration_complete && session.traffic_open;
                        }
                        if session.traffic_configuration_complete != was_complete || pending {
                            info!(
                                "HRPD AN A8: traffic configuration pending UATI=0x{uati:08x} pending={pending} traffic_complete={} buffered_chunks={}",
                                session.traffic_configuration_complete,
                                session.pending_downlink.len()
                            );
                        }
                        if !pending
                            && !flush_hrpd_a8_downlink(
                                uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            )
                        {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::SetSessionConfigurationComplete {
                        uati,
                        complete,
                        physical_layer_subtype,
                        forward_traffic_mac_subtype,
                        idle_preferred_control_channel_cycle,
                        idle_page_period_cycles,
                    } => {
                        if complete {
                            self.pending_session_configuration_complete.insert(
                                uati,
                                HrpdSessionConfigurationState {
                                    physical_layer_subtype,
                                    forward_traffic_mac_subtype,
                                    idle_preferred_control_channel_cycle,
                                    idle_page_period_cycles,
                                },
                            );
                            self.pending_traffic_configuration.remove(&uati);
                        } else {
                            self.pending_session_configuration_complete.remove(&uati);
                            self.pending_traffic_configuration.remove(&uati);
                        }
                        let Some(session) = self.sessions.get_mut(&uati) else {
                            info!(
                                "HRPD AN A8: session configuration update for unregistered UATI=0x{uati:08x} complete={complete}"
                            );
                            continue;
                        };
                        let was_session_complete = session.session_configuration_complete;
                        let was_traffic_complete = session.traffic_configuration_complete;
                        let session_config_changed = !was_session_complete
                            || session.physical_layer_subtype != physical_layer_subtype
                            || session.forward_traffic_mac_subtype != forward_traffic_mac_subtype;
                        if complete {
                            session.session_configuration_complete = true;
                            session.physical_layer_subtype = physical_layer_subtype;
                            session.forward_traffic_mac_subtype = forward_traffic_mac_subtype;
                            session.idle_preferred_control_channel_cycle =
                                idle_preferred_control_channel_cycle;
                            session.idle_page_period_cycles = idle_page_period_cycles;
                            session.traffic_configuration_complete = session.traffic_open;
                            session.traffic_setup_pending = false;
                            hrpd_a8_note_commit_close_window(session);
                            if session_config_changed {
                                session.data_ready_last_sent = None;
                                session.data_ready_outstanding = None;
                                session.data_ready_acknowledged = false;
                                session.data_ready_last_transaction_sent = None;
                            }
                        } else {
                            session.session_configuration_complete = false;
                            session.physical_layer_subtype = 0;
                            session.forward_traffic_mac_subtype = 0;
                            session.idle_preferred_control_channel_cycle = None;
                            session.idle_page_period_cycles =
                                HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES;
                            session.traffic_configuration_complete = false;
                            session.default_packet_flow_open = false;
                            session.data_ready_last_sent = None;
                            session.data_ready_outstanding = None;
                            session.data_ready_acknowledged = false;
                        }
                        if session.session_configuration_complete != was_session_complete
                            || session.traffic_configuration_complete != was_traffic_complete
                        {
                            info!(
                                "HRPD AN A8: SessionConfigurationComplete UATI=0x{uati:08x} complete={complete} session_complete={} traffic_complete={} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} idle_preferred_cycle={:?} idle_period_cycles={} buffered_chunks={}",
                                session.session_configuration_complete,
                                session.traffic_configuration_complete,
                                session.physical_layer_subtype,
                                session.forward_traffic_mac_subtype,
                                session.idle_preferred_control_channel_cycle,
                                session.idle_page_period_cycles,
                                session.pending_downlink.len()
                            );
                        }
                        if complete
                            && !flush_hrpd_a8_downlink(
                                uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            )
                        {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::SetDefaultPacketFlowOpen { uati, open } => {
                        let Some(session_key) =
                            hrpd_a8_session_key_for_uati_alias(&self.sessions, uati)
                        else {
                            if open {
                                self.pending_default_packet_flow_open.insert(uati);
                            } else {
                                self.pending_default_packet_flow_open.remove(&uati);
                            }
                            warn!(
                                "HRPD AN A8: flow-control update for unregistered UATI=0x{uati:08x} open={open}"
                            );
                            continue;
                        };
                        let session = self
                            .sessions
                            .get_mut(&session_key)
                            .expect("resolved A8 session key must be present");
                        let was_open = session.default_packet_flow_open;
                        session.default_packet_flow_open = open;
                        if open {
                            session.data_ready_last_sent = None;
                            session.data_ready_outstanding = None;
                            session.data_ready_last_transaction_sent = None;
                            session.data_ready_acknowledged = false;
                            if !was_open {
                                info!(
                                    "HRPD AN A8: DefaultPacket flow open UATI=0x{session_key:08x} request_uati=0x{uati:08x} buffered_chunks={}",
                                    session.pending_downlink.len()
                                );
                            }
                            if session.traffic_open {
                                if !flush_hrpd_a8_downlink(
                                    session_key,
                                    session,
                                    &forward_signaling_tx,
                                    &forward_traffic_tx,
                                ) {
                                    warn!("HRPD AN A8: BTS forward-traffic queue closed");
                                    return;
                                }
                            }
                        } else {
                            session.data_ready_acknowledged = false;
                            info!(
                                "HRPD AN A8: DefaultPacket flow closed UATI=0x{session_key:08x} request_uati=0x{uati:08x}"
                            );
                        }
                    }
                    HrpdAnA8Command::SetDefaultPacketStreamConfiguration {
                        uati,
                        stream_id,
                        protocol_type,
                    } => {
                        self.pending_default_packet_stream_by_uati
                            .insert(uati, (stream_id, protocol_type));
                        let Some(session) = self.sessions.get_mut(&uati) else {
                            info!(
                                "HRPD AN A8: cached DefaultPacket stream configuration for unregistered UATI=0x{uati:08x} stream={stream_id} protocol=0x{protocol_type:02x}"
                            );
                            continue;
                        };
                        session.default_packet_stream_id = stream_id;
                        session.default_packet_protocol_type = protocol_type;
                        info!(
                            "HRPD AN A8: DefaultPacket stream configured UATI=0x{uati:08x} stream={stream_id} protocol=0x{protocol_type:02x} buffered_chunks={}",
                            session.pending_downlink.len()
                        );
                        if !flush_hrpd_a8_downlink(
                            uati,
                            session,
                            &forward_signaling_tx,
                            &forward_traffic_tx,
                        ) {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::DefaultPacketDataReadyAck {
                        uati,
                        transaction_id,
                    } => {
                        let Some(session_key) =
                            hrpd_a8_session_key_for_uati_alias(&self.sessions, uati)
                        else {
                            warn!(
                                "HRPD AN A8: DataReadyAck for unregistered UATI=0x{uati:08x} transaction=0x{transaction_id:02x}"
                            );
                            continue;
                        };
                        let session = self
                            .sessions
                            .get_mut(&session_key)
                            .expect("resolved A8 session key must be present");
                        if !hrpd_a8_data_ready_ack_matches(session, transaction_id) {
                            if session.default_packet_flow_open {
                                info!(
                                    "HRPD AN A8: ignoring late/duplicate DefaultPacket DataReadyAck UATI=0x{session_key:08x} request_uati=0x{uati:08x} transaction=0x{transaction_id:02x} packet_flow_open=true"
                                );
                            } else {
                                warn!(
                                    "HRPD AN A8: unexpected DefaultPacket DataReadyAck UATI=0x{session_key:08x} request_uati=0x{uati:08x} transaction=0x{transaction_id:02x} outstanding={:?} packet_flow_open=false",
                                    session.data_ready_outstanding
                                );
                            }
                            continue;
                        }
                        if !acknowledge_hrpd_a8_data_ready(
                            session_key,
                            uati,
                            transaction_id,
                            session,
                            &forward_signaling_tx,
                            &forward_traffic_tx,
                        ) {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::ResetDefaultPacketRlp { uati } => {
                        let Some(session_key) =
                            hrpd_a8_session_key_for_uati_alias(&self.sessions, uati)
                        else {
                            warn!("HRPD AN A8: RLP reset for unregistered UATI=0x{uati:08x}");
                            continue;
                        };
                        let session = self
                            .sessions
                            .get_mut(&session_key)
                            .expect("resolved A8 session key must be present");
                        reset_hrpd_a8_default_packet_rlp(session);
                        info!(
                            "HRPD AN A8: DefaultPacket RLP reset UATI=0x{session_key:08x} request_uati=0x{uati:08x}; transmit sequence and retransmit history reset"
                        );
                    }
                    HrpdAnA8Command::RetransmitDefaultPacketRlp { uati, requests } => {
                        let Some(session_key) =
                            hrpd_a8_session_key_for_uati_alias(&self.sessions, uati)
                        else {
                            warn!(
                                "HRPD AN A8: RLP Nak for unregistered UATI=0x{uati:08x} requests={}",
                                requests.len()
                            );
                            continue;
                        };
                        let session = self
                            .sessions
                            .get_mut(&session_key)
                            .expect("resolved A8 session key must be present");
                        queue_hrpd_rlp_nak_retransmissions(session_key, session, &requests);
                        if !flush_hrpd_a8_downlink(
                            session_key,
                            session,
                            &forward_signaling_tx,
                            &forward_traffic_tx,
                        ) {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                    HrpdAnA8Command::UpdateDrc { uati, drc_index } => {
                        if let Some(session) = self.sessions.get(&uati)
                            && valid_hrpd_forward_drc(
                                drc_index,
                                session.forward_traffic_mac_subtype,
                            )
                            .is_none()
                        {
                            warn!(
                                "HRPD AN A8: ignoring invalid/null live DRC UATI=0x{uati:08x} drc=0x{drc_index:x} ftc_mac_subtype=0x{:04x}",
                                session.forward_traffic_mac_subtype
                            );
                            continue;
                        }
                        let drc_at = Instant::now();
                        self.pending_drc_by_uati.insert(uati, (drc_index, drc_at));
                        if let Some(session) = self.sessions.get_mut(&uati) {
                            session.last_drc_index = Some(drc_index);
                            session.last_drc_at = Some(drc_at);
                        }
                    }
                    HrpdAnA8Command::RetargetPendingDownlink {
                        uati,
                        include_open_sessions,
                    } => {
                        let target_uati = hrpd_a8_retarget_target_uati(&self.sessions, uati);
                        if target_uati != uati {
                            info!(
                                "HRPD AN A8: resolved downlink retarget request_uati=0x{uati:08x} to active UATI=0x{target_uati:08x}"
                            );
                        }
                        let migrated_chunks = migrate_hrpd_a8_downlink(
                            target_uati,
                            &mut self.table,
                            &mut self.sessions,
                            &mut self.pending_traffic_open,
                            &mut self.pending_traffic_setup,
                            include_open_sessions,
                        );
                        if (migrated_chunks > 0 || target_uati != uati)
                            && let Some(session) = self.sessions.get_mut(&target_uati)
                            && !flush_hrpd_a8_downlink(
                                target_uati,
                                session,
                                &forward_signaling_tx,
                                &forward_traffic_tx,
                            )
                        {
                            warn!("HRPD AN A8: BTS forward queue closed");
                            return;
                        }
                    }
                }
            }

            for (&uati, session) in self.sessions.iter_mut() {
                if !hrpd_a8_periodic_flush_ready(session) {
                    continue;
                }
                if !flush_hrpd_a8_downlink(
                    uati,
                    session,
                    &forward_signaling_tx,
                    &forward_traffic_tx,
                ) {
                    warn!("HRPD AN A8: BTS forward queue closed");
                    return;
                }
            }

            for _ in 0..HRPD_BEARER_MAX_DATAGRAMS_PER_PASS {
                let (packet, _) = match bearer.try_recv_gre_packet(&mut self.buf) {
                    Ok(value) => value,
                    Err(cdma_a8::Error::UdpTransport(err)) if is_udp_timeout(&err) => break,
                    Err(err) => {
                        warn!("HRPD AN A8: receive/decode failed: {err}");
                        continue;
                    }
                };
                pass_active = true;
                let wire = match packet.encode() {
                    Ok(wire) => wire,
                    Err(err) => {
                        warn!("HRPD AN A8: failed to reserialize inbound GRE packet: {err}");
                        continue;
                    }
                };
                let inbound = match self.table.decode_for_session(endpoint, &wire) {
                    Ok(inbound) => inbound,
                    Err(err) => {
                        warn!("HRPD AN A8: bearer packet rejected: {err}");
                        continue;
                    }
                };
                let Some(session) = self.sessions.get_mut(&inbound.session_id) else {
                    warn!(
                        "HRPD AN A8: decoded packet for unknown UATI=0x{:08x}",
                        inbound.session_id
                    );
                    continue;
                };
                hrpd_a8_enqueue_downlink_payload(inbound.session_id, session, &inbound.payload);
                hrpd_a8_note_downlink_buffered(inbound.session_id, session, inbound.payload.len());
                if !flush_hrpd_a8_downlink(
                    inbound.session_id,
                    session,
                    &forward_signaling_tx,
                    &forward_traffic_tx,
                ) {
                    warn!("HRPD AN A8: BTS forward queue closed");
                    return;
                }
            }
            if !pass_active {
                let next_wakeup = hrpd_a8_next_runtime_wakeup(&self.sessions);
                if next_wakeup == Some(Duration::ZERO) {
                    continue;
                }
                match next_wakeup {
                    Some(delay) => {
                        tokio::select! {
                            command = rx.recv() => {
                                let Some(command) = command else {
                                    info!("HRPD AN A8 bearer listener stopped");
                                    return;
                                };
                                self.pending_command = Some(command);
                            }
                            result = bearer.readable() => {
                                if let Err(err) = result {
                                    warn!("HRPD AN A8 readiness failed: {err}");
                                    return;
                                }
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                    None => {
                        tokio::select! {
                            command = rx.recv() => {
                                let Some(command) = command else {
                                    info!("HRPD AN A8 bearer listener stopped");
                                    return;
                                };
                                self.pending_command = Some(command);
                            }
                            result = bearer.readable() => {
                                if let Err(err) = result {
                                    warn!("HRPD AN A8 readiness failed: {err}");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn spawn_hrpd_an_a8_runtime(
    config: cdma_a8::BearerTransportConfig,
    endpoint: cdma_a8::BearerEndpoint,
    forward_signaling_tx: tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdForwardSignalingRequest>,
    forward_traffic_tx: tokio::sync::mpsc::UnboundedSender<HrpdAnForwardTrafficPacket>,
) -> Result<HrpdAnA8Runtime, Error> {
    let bearer = cdma_a8::UdpGreEndpoint::bind(config, "an.a8_bearer")
        .map_err(|err| Error::from(format!("HRPD AN A8 bind failed: {err}")))?
        .into_tokio()
        .map_err(|err| Error::from(format!("HRPD AN A8 Tokio setup failed: {err}")))?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(HrpdAnA8Actor::new().run(
        rx,
        bearer,
        endpoint,
        forward_signaling_tx,
        forward_traffic_tx,
    ));
    Ok(HrpdAnA8Runtime { tx })
}

fn is_udp_timeout(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    err.contains("wouldblock")
        || err.contains("would block")
        || err.contains("resource temporarily unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrpd_a8_runtime_wakeup_is_event_driven_without_pending_work() {
        let sessions = HashMap::from([(1, HrpdAnA8Session::new(1, 6))]);

        assert_eq!(hrpd_a8_next_runtime_wakeup(&sessions), None);
    }

    #[test]
    fn hrpd_a8_runtime_wakeup_tracks_partial_packet_deadline() {
        let mut session = HrpdAnA8Session::new(1, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.pending_downlink.push_back(vec![0xaa]);
        session.pending_downlink_partial_hold_at = Some(Instant::now());

        let delay = hrpd_a8_next_flush_delay(&session).unwrap();
        assert!(delay > Duration::ZERO);
        assert!(delay <= HRPD_A8_PARTIAL_PACKET_COALESCE_DELAY);
    }

    #[test]
    fn hrpd_a8_runtime_wakeup_uses_data_ready_retry_deadline() {
        let mut session = HrpdAnA8Session::new(1, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.pending_downlink.push_back(vec![0xaa]);
        session.data_ready_last_sent = Some(Instant::now());

        let delay = hrpd_a8_next_flush_delay(&session).unwrap();
        assert!(delay > Duration::from_millis(1100));
        assert!(delay <= HRPD_DATA_READY_RETRY_INTERVAL);
    }

    #[test]
    fn hrpd_a8_reopen_reuses_completed_session_configuration() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);

        assert!(!hrpd_a8_traffic_configuration_complete_on_open(
            &session, false
        ));

        session.session_configuration_complete = true;
        assert!(hrpd_a8_traffic_configuration_complete_on_open(
            &session, false
        ));
        assert!(!hrpd_a8_traffic_configuration_complete_on_open(
            &session, true
        ));
        session.initial_connection_close_observed = true;
        assert!(!hrpd_a8_traffic_configuration_complete_on_open(
            &session, true
        ));
    }

    #[test]
    fn hrpd_a8_cached_config_clears_open_pending_configuration_gate() {
        let uati = 0x1a05_8001;
        let mut pending = HashSet::from([uati]);

        assert!(!hrpd_a8_update_pending_traffic_configuration_for_open(
            uati,
            true,
            &mut pending
        ));
        assert!(!pending.contains(&uati));

        assert!(hrpd_a8_update_pending_traffic_configuration_for_open(
            uati,
            false,
            &mut pending
        ));
        assert!(pending.contains(&uati));
    }

    #[test]
    fn hrpd_a8_first_page_after_close_targets_preferred_wakeup() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.session_configuration_complete = true;
        session.initial_connection_close_observed = true;
        session.idle_preferred_control_channel_cycle = Some(7);
        session.idle_page_period_cycles = 12;

        let cycle = hrpd_open_connection_page_cycle(&session).expect("first page is slotted");
        assert_eq!(cycle.modulus, 12);
        assert_eq!(cycle.residue, 5);
    }

    #[test]
    fn hrpd_a8_traffic_close_preserves_preferred_wakeup_schedule() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.idle_preferred_control_channel_cycle = Some(7);
        session.idle_page_period_cycles = 12;
        session.session_configuration_complete = true;

        hrpd_a8_note_commit_close_window(&mut session);

        assert!(session.initial_connection_close_observed);
        let cycle = hrpd_open_connection_page_cycle(&session).expect("page is slotted");
        assert_eq!(cycle.modulus, 12);
        assert_eq!(cycle.residue, 5);
    }

    #[test]
    fn hrpd_a8_closed_flow_coalesces_duplicate_single_chunk_downlink() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        let lcp_frame = vec![0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7e];

        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &lcp_frame);
        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &lcp_frame);

        assert_eq!(session.pending_downlink.len(), 1);
        assert_eq!(session.pending_downlink[0], lcp_frame);

        let next_frame = vec![0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x01, 0x7e];
        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &next_frame);

        assert_eq!(session.pending_downlink.len(), 1);
        assert_eq!(
            session.pending_downlink[0],
            [lcp_frame.as_slice(), next_frame.as_slice()].concat()
        );
    }

    #[test]
    fn hrpd_a8_open_flow_preserves_duplicate_downlink_in_octet_stream() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.default_packet_flow_open = true;
        let lcp_frame = vec![0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7e];

        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &lcp_frame);
        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &lcp_frame);

        assert_eq!(session.pending_downlink.len(), 1);
        assert_eq!(
            session.pending_downlink[0],
            [lcp_frame.as_slice(), lcp_frame.as_slice()].concat()
        );
    }

    #[test]
    fn hrpd_a8_closed_flow_preserves_multi_chunk_downlink() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        let payload = vec![0x42; HRPD_A8_FORWARD_MAX_STREAM_OCTETS + 1];

        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &payload);
        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &payload);

        assert_eq!(session.pending_downlink.len(), 3);
        assert_eq!(
            session.pending_downlink[0].len(),
            HRPD_A8_FORWARD_MAX_STREAM_OCTETS
        );
        assert_eq!(
            session.pending_downlink[1].len(),
            HRPD_A8_FORWARD_MAX_STREAM_OCTETS
        );
        assert_eq!(session.pending_downlink[2].len(), 2);
    }

    #[test]
    fn hrpd_a8_enqueue_fills_rlp_segment_across_a10_boundaries() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.default_packet_flow_open = true;

        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &[0x11; 113]);
        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &[0x22; 62]);

        assert_eq!(session.pending_downlink.len(), 2);
        assert_eq!(session.pending_downlink[0].len(), 120);
        assert_eq!(&session.pending_downlink[0][..113], &[0x11; 113]);
        assert_eq!(&session.pending_downlink[0][113..], &[0x22; 7]);
        assert_eq!(session.pending_downlink[1], vec![0x22; 55]);
    }

    #[test]
    fn hrpd_a8_closed_traffic_with_pending_downlink_pages_and_retains_buffer() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.session_configuration_complete = true;
        session.idle_preferred_control_channel_cycle = Some(7);
        session.idle_page_period_cycles = 12;
        session.default_packet_stream_id = 2;
        session.default_packet_protocol_type = 0x16;
        session.pending_downlink.push_back(vec![0xde, 0xad]);
        hrpd_a8_note_commit_close_window(&mut session);

        assert!(hrpd_a8_periodic_flush_ready(&session));
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        let data_ready = signaling_rx.try_recv().unwrap();
        assert_eq!(data_ready.uati, Some(0x1a05_8001));
        assert_eq!(
            data_ready.target_ati,
            hrpd_air::AccessTerminalIdentifier {
                ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            }
        );
        assert_eq!(data_ready.protocol_type, 0x16);
        assert_eq!(data_ready.payload, vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]);
        assert_eq!(
            data_ready.channel,
            hrpd_air::HrpdForwardChannel::AsynchronousControl
        );
        assert_eq!(data_ready.synchronous_control_cycle, None);

        let page = signaling_rx.try_recv().unwrap();
        assert_eq!(page.uati, Some(0x1a05_8001));
        assert_eq!(
            page.target_ati,
            hrpd_air::AccessTerminalIdentifier {
                ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            }
        );
        assert_eq!(page.payload, vec![0x00]);
        assert_eq!(
            page.channel,
            hrpd_air::HrpdForwardChannel::SynchronousControl
        );
        assert_eq!(
            page.synchronous_control_cycle,
            Some(hrpd_air::HrpdSynchronousControlCycle {
                modulus: 12,
                residue: 5,
            })
        );
        assert!(traffic_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.len(), 1);
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(session.data_ready_transaction, 1);
        assert!(session.open_connection_last_sent.is_some());

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(signaling_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.len(), 1);
    }

    #[test]
    fn hrpd_a8_idle_data_ready_transaction_survives_traffic_reopen() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.session_configuration_complete = true;
        session.default_packet_stream_id = 2;
        session.default_packet_protocol_type = 0x16;
        session.pending_downlink.push_back(vec![0xde, 0xad]);
        hrpd_a8_note_commit_close_window(&mut session);

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert_eq!(
            signaling_rx.try_recv().unwrap().payload,
            vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]
        );
        assert_eq!(signaling_rx.try_recv().unwrap().payload, vec![0x00]);
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(session.data_ready_transaction, 1);

        reset_hrpd_a8_default_packet_rlp(&mut session);
        session.traffic_open = true;
        session.traffic_configuration_complete = true;
        session.open_connection_last_sent = None;
        session.initial_connection_close_observed = false;

        assert!(hrpd_a8_data_ready_ack_matches(&session, 0));
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(signaling_rx.try_recv().is_err());
        assert!(traffic_rx.try_recv().is_err());
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(session.data_ready_transaction, 1);

        session.data_ready_last_sent =
            Some(Instant::now() - HRPD_DATA_READY_RETRY_INTERVAL - Duration::from_millis(1));
        session.last_drc_index = Some(0x0b);
        session.last_drc_at = Some(Instant::now());
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        let original = traffic_rx.try_recv().unwrap();
        assert!(!original.high_priority);
        assert_eq!(
            signaling_rx.try_recv().unwrap().payload,
            vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]
        );
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(session.data_ready_transaction, 1);
    }

    #[test]
    fn hrpd_a8_control_alias_accepts_session_or_traffic_uati() {
        let mut sessions = HashMap::new();
        sessions.insert(0x1a05_8001, HrpdAnA8Session::new(0x0005_8001, 6));

        assert_eq!(
            hrpd_a8_session_key_for_uati_alias(&sessions, 0x1a05_8001),
            Some(0x1a05_8001)
        );
        assert_eq!(
            hrpd_a8_session_key_for_uati_alias(&sessions, 0x0005_8001),
            Some(0x1a05_8001)
        );
        assert_eq!(
            hrpd_a8_session_key_for_uati_alias(&sessions, 0x0005_8002),
            None
        );
    }

    #[test]
    fn hrpd_a8_pending_flow_open_accepts_session_or_traffic_uati() {
        let mut pending = HashSet::from([0x0005_8001]);

        assert!(hrpd_a8_pending_uati_alias(
            &pending,
            0x1a05_8001,
            0x0005_8001
        ));
        assert!(hrpd_a8_take_pending_uati_alias(
            &mut pending,
            0x1a05_8001,
            0x0005_8001
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn hrpd_a8_retarget_session_uati_keeps_active_traffic_session() {
        let traffic_uati = 0x1a05_8001;
        let session_uati = 0x0005_8001;
        let mut sessions = HashMap::new();
        let mut session = HrpdAnA8Session::new(session_uati, 6);
        session.traffic_open = true;
        session.pending_downlink.push_back(vec![0xc0, 0x21]);
        sessions.insert(traffic_uati, session);
        let mut table = cdma_a8::BearerTable::new();
        table
            .apply_session(cdma_a8::BearerSession::new(
                traffic_uati,
                cdma_a8::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]),
            ))
            .unwrap();
        let mut pending_traffic_open = HashSet::new();
        let mut pending_traffic_setup = HashSet::new();

        let target = hrpd_a8_retarget_target_uati(&sessions, session_uati);
        assert_eq!(target, traffic_uati);
        let migrated = migrate_hrpd_a8_downlink(
            target,
            &mut table,
            &mut sessions,
            &mut pending_traffic_open,
            &mut pending_traffic_setup,
            true,
        );

        assert_eq!(migrated, 0);
        assert!(sessions.contains_key(&traffic_uati));
        assert!(!sessions.contains_key(&session_uati));
        assert_eq!(sessions[&traffic_uati].pending_downlink.len(), 1);
        assert!(table.has_session(traffic_uati));
    }

    #[test]
    fn hrpd_a8_data_ready_uses_control_and_open_traffic_ftc() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.physical_layer_subtype = 0;
        session.forward_traffic_mac_subtype = 0;
        session.default_packet_stream_id = 2;
        session.default_packet_protocol_type = 0x16;
        session.last_drc_index = Some(0x0b);
        session.last_drc_at = Some(Instant::now());
        session.pending_downlink.push_back(vec![0x7e]);

        assert!(send_hrpd_a8_data_ready(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        let packet = traffic_rx.try_recv().unwrap();
        assert!(!packet.high_priority);
        let signaling = signaling_rx.try_recv().unwrap();
        assert_eq!(signaling.uati, Some(0x1a05_8001));
        assert_eq!(
            signaling.target_ati,
            hrpd_air::AccessTerminalIdentifier {
                ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001
            }
        );
        assert_eq!(signaling.protocol_type, 0x16);
        assert_eq!(signaling.payload, vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]);
        assert_eq!(
            signaling.channel,
            hrpd_air::HrpdForwardChannel::AsynchronousControl
        );
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
    }

    #[test]
    fn hrpd_a8_rev_a_data_ready_waits_for_ack_before_flushing_rlp() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.physical_layer_subtype = 0x0002;
        session.forward_traffic_mac_subtype =
            cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED;
        session.default_packet_stream_id = 2;
        session.default_packet_protocol_type = 0x16;
        session.last_drc_index = Some(0x0e);
        session.last_drc_at = Some(Instant::now());
        session
            .pending_downlink
            .push_back(vec![0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e]);

        assert!(send_hrpd_a8_data_ready(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        assert!(!session.default_packet_flow_open);
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(session.data_ready_transaction, 1);
        assert_eq!(session.pending_downlink.len(), 1);
        assert_eq!(session.rlp_seq, 0);

        let data_ready_ftc = traffic_rx.try_recv().unwrap();
        assert_eq!(data_ready_ftc.payload.len(), 5120);
        assert!(traffic_rx.try_recv().is_err());

        let signaling = signaling_rx.try_recv().unwrap();
        assert_eq!(signaling.protocol_type, 0x16);
        assert_eq!(signaling.payload, vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]);
        assert!(signaling_rx.try_recv().is_err());
    }

    #[test]
    fn hrpd_a8_data_ready_retransmit_reuses_outstanding_transaction() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.last_drc_index = Some(0x0b);
        session.last_drc_at = Some(Instant::now());
        session.pending_downlink.push_back(vec![0x7e]);

        assert!(send_hrpd_a8_data_ready(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(traffic_rx.try_recv().is_ok());
        assert_eq!(
            signaling_rx.try_recv().unwrap().payload,
            vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]
        );
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(session.data_ready_transaction, 1);

        session.data_ready_last_sent =
            Some(Instant::now() - HRPD_DATA_READY_RETRY_INTERVAL - Duration::from_millis(1));
        assert!(send_hrpd_a8_data_ready(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(traffic_rx.try_recv().is_ok());
        assert_eq!(
            signaling_rx.try_recv().unwrap().payload,
            vec![HRPD_DEFAULT_PACKET_DATA_READY, 0]
        );
        assert_eq!(session.data_ready_outstanding, Some(0));
        assert_eq!(session.data_ready_last_transaction_sent, Some(0));
        assert_eq!(
            session.data_ready_transaction, 1,
            "retransmitting an unacked DataReady must not allocate tx=0x01"
        );
    }

    /// DataReadyAck only acknowledges the DataReady message: retransmission
    /// stops but the flow stays in Close State until XonRequest (or reverse
    /// RLP) opens it, per the Flow Control Protocol state machine.
    #[test]
    fn hrpd_a8_data_ready_ack_stops_retransmission_without_opening_flow() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x0005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.physical_layer_subtype = 0;
        session.forward_traffic_mac_subtype = 0;
        session.default_packet_stream_id = 2;
        session.default_packet_protocol_type = 0x16;
        session.data_ready_outstanding = Some(0x34);
        session.data_ready_last_transaction_sent = Some(0x34);
        session.data_ready_last_sent = Some(Instant::now());
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());
        for octet in [0x01, 0x02, 0x03, 0x04] {
            session.pending_downlink.push_back(vec![octet]);
        }
        assert!(hrpd_a8_data_ready_ack_matches(&session, 0x34));
        assert!(!hrpd_a8_data_ready_ack_matches(&session, 0x35));

        assert!(acknowledge_hrpd_a8_data_ready(
            0x1a05_8001,
            0x0005_8001,
            0x34,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        assert!(!session.default_packet_flow_open);
        assert!(session.data_ready_acknowledged);
        assert_eq!(session.data_ready_outstanding, None);
        assert_eq!(session.data_ready_last_transaction_sent, None);
        assert_eq!(session.data_ready_last_sent, None);
        assert_eq!(session.pending_downlink.len(), 4);
        assert!(traffic_rx.try_recv().is_err());
        assert!(signaling_rx.try_recv().is_err());

        // XonRequest opens the flow; the next flush drains the buffer.
        session.default_packet_flow_open = true;
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert_eq!(session.pending_downlink.len(), 0);
        assert_eq!(session.rlp_seq, 4);
        let packet = traffic_rx.try_recv().unwrap();
        assert_eq!(packet.payload.len(), 4096);
        assert!(traffic_rx.try_recv().is_err());
        assert!(signaling_rx.try_recv().is_err());
    }

    #[test]
    fn hrpd_a8_late_data_ready_ack_survives_traffic_reopen() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x0005_8001, 6);
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.physical_layer_subtype = 0;
        session.forward_traffic_mac_subtype = 0;
        session.default_packet_stream_id = 2;
        session.default_packet_protocol_type = 0x16;
        session.data_ready_last_transaction_sent = Some(0);
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());
        for octet in [0x01, 0x02, 0x03, 0x04] {
            session.pending_downlink.push_back(vec![octet]);
        }
        assert!(hrpd_a8_data_ready_ack_matches(&session, 0));
        assert!(!hrpd_a8_data_ready_ack_matches(&session, 1));

        assert!(acknowledge_hrpd_a8_data_ready(
            0x1a05_8001,
            0x0005_8001,
            0,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        assert!(!session.default_packet_flow_open);
        assert!(session.data_ready_acknowledged);
        assert_eq!(session.data_ready_last_transaction_sent, None);
        assert_eq!(session.pending_downlink.len(), 4);
        assert!(traffic_rx.try_recv().is_err());
        assert!(
            signaling_rx.try_recv().is_ok(),
            "ACK-triggered closed-traffic flush should still page the AT"
        );
        assert!(signaling_rx.try_recv().is_err());

        // Reopening traffic does not open the flow by itself; the buffered
        // downlink waits for XonRequest.
        session.traffic_open = true;
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(!session.default_packet_flow_open);
        assert_eq!(session.pending_downlink.len(), 4);
        assert!(traffic_rx.try_recv().is_err());

        session.default_packet_flow_open = true;
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert_eq!(session.pending_downlink.len(), 0);
        assert!(traffic_rx.try_recv().is_ok());
        while signaling_rx.try_recv().is_ok() {}
    }

    #[test]
    fn hrpd_a8_default_packet_flow_survives_traffic_close() {
        let mut session = HrpdAnA8Session::new(0x0005_8001, 6);
        session.traffic_open = false;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.data_ready_last_sent = Some(Instant::now());
        session.data_ready_outstanding = Some(0x34);
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());

        hrpd_a8_note_traffic_closed(&mut session);

        assert!(session.default_packet_flow_open);
        assert!(!session.traffic_configuration_complete);
        assert!(session.initial_connection_close_observed);
        assert_eq!(session.data_ready_last_sent, None);
        assert_eq!(session.data_ready_outstanding, None);
        assert_eq!(session.last_drc_index, None);
        assert_eq!(session.last_drc_at, None);
    }

    #[test]
    fn hrpd_a8_default_packet_flow_survives_traffic_setup_pending() {
        let mut session = HrpdAnA8Session::new(0x0005_8001, 6);
        session.address_management_pending = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.data_ready_last_sent = Some(Instant::now());
        session.data_ready_outstanding = Some(0x35);
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());

        hrpd_a8_note_traffic_setup_pending(&mut session);

        assert!(session.default_packet_flow_open);
        assert!(!session.address_management_pending);
        assert!(!session.traffic_configuration_complete);
        assert_eq!(session.data_ready_last_sent, None);
        assert_eq!(session.data_ready_outstanding, None);
        assert_eq!(session.last_drc_index, None);
        assert_eq!(session.last_drc_at, None);
    }

    #[test]
    fn hrpd_a8_connection_open_resets_default_packet_rlp_state() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.rlp_seq = 16_460;
        record_hrpd_rlp_history(&mut session, 0, &[0xaa, 0xbb, 0xcc]);
        session
            .pending_retransmit
            .push_back(HrpdRlpRetransmitSegment {
                sequence: 0,
                octets: vec![0xaa],
            });
        session
            .pending_retransmit_requests
            .push_back(HrpdRlpRetransmitRequest {
                next_sequence: 1,
                remaining: 2,
            });
        session.pending_downlink.push_back(vec![0x7e]);

        reset_hrpd_a8_default_packet_rlp(&mut session);

        assert_eq!(session.rlp_seq, 0);
        assert!(session.rlp_history.is_empty());
        assert!(session.pending_retransmit.is_empty());
        assert!(session.pending_retransmit_requests.is_empty());
        assert_eq!(session.pending_downlink.len(), 1);
    }

    #[test]
    fn hrpd_a8_flush_uses_fallback_rate_before_first_drc() {
        let (signaling_tx, _signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        // No valid A8-side DRC has been observed yet.
        session.pending_downlink.push_back(vec![0xc0, 0x21]);

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        // Instead of stalling PPP, the downlink RLP flushes at the conservative
        // fallback rate; the BTS scheduler re-rates the packet on air.
        let packet = traffic_rx.try_recv().expect("fallback-rate packet emitted");
        assert!(!packet.payload.is_empty());
        assert!(session.pending_downlink.is_empty());
    }

    #[test]
    fn hrpd_a8_flush_retains_last_valid_drc_as_packing_hint() {
        let (signaling_tx, _signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.physical_layer_subtype = 2;
        session.forward_traffic_mac_subtype = 1;
        session.last_drc_index = Some(0x0e);
        session.last_drc_at = Some(Instant::now() - Duration::from_secs(10));
        for value in 0..5 {
            session.pending_downlink.push_back(vec![value]);
        }

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        let packet = traffic_rx.try_recv().expect("last-DRC packet emitted");
        assert_eq!(packet.payload.len(), 5120);
        assert!(session.pending_downlink.is_empty());
    }

    #[test]
    fn hrpd_a8_flush_packs_rev0_rlp_segments_into_one_physical_packet() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());
        session.pending_downlink.push_back(vec![0x01]);
        session.pending_downlink.push_back(vec![0x02]);
        session.pending_downlink.push_back(vec![0x03]);
        session.pending_downlink.push_back(vec![0x04]);

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        let packet = traffic_rx.try_recv().unwrap();
        assert_eq!(packet.payload.len(), 4096);
        for idx in 0..4 {
            let mac_start = idx * 1024;
            assert_eq!(
                packet.payload[mac_start + 1000],
                1,
                "MAC {idx} ConnectionLayerFormat=Format B"
            );
            assert_eq!(
                packet.payload[mac_start + 1001],
                1,
                "MAC {idx} MACLayerFormat=valid"
            );
        }
        assert!(traffic_rx.try_recv().is_err());
        assert!(signaling_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.len(), 0);
        assert_eq!(session.rlp_seq, 4);
        assert_eq!(session.rlp_history.len(), 4);
        assert_eq!(session.traffic_window_stats.forward_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_segments, 4);
        assert_eq!(session.traffic_window_stats.rlp_full_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_packets, 0);
    }

    #[test]
    fn hrpd_a8_flush_defers_new_rlp_tail_for_coalescing() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());
        for idx in 0..5 {
            session.pending_downlink.push_back(vec![idx]);
        }

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        assert!(traffic_rx.try_recv().is_ok());
        assert!(traffic_rx.try_recv().is_err());
        assert!(signaling_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.len(), 1);
        assert_eq!(session.pending_downlink[0], vec![4]);
        assert_eq!(session.rlp_seq, 4);
        assert_eq!(session.traffic_window_stats.forward_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_full_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_packets, 0);
        assert_eq!(session.traffic_window_stats.rlp_partial_deferred, 1);
        assert!(session.pending_downlink_partial_hold_at.is_some());

        session.pending_downlink_partial_hold_at =
            Some(Instant::now() - HRPD_A8_PARTIAL_PACKET_COALESCE_DELAY - Duration::from_millis(1));
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        assert!(traffic_rx.try_recv().is_ok());
        assert!(traffic_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.len(), 0);
        assert_eq!(session.rlp_seq, 5);
        assert_eq!(session.traffic_window_stats.forward_packets, 2);
        assert_eq!(session.traffic_window_stats.rlp_full_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_new_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_retx_packets, 0);
        assert_eq!(session.traffic_window_stats.rlp_partial_mixed_packets, 0);
    }

    #[test]
    fn hrpd_a8_flush_fills_rev_a_packet_tail_across_a10_boundaries() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.forward_traffic_mac_subtype =
            cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED;
        session.last_drc_index = Some(0x0e);
        session.last_drc_at = Some(Instant::now());

        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &[0x11; 598]);
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(traffic_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.len(), 5);
        assert_eq!(session.pending_downlink.back().unwrap().len(), 118);

        session.pending_downlink_partial_hold_at =
            Some(Instant::now() - HRPD_A8_PARTIAL_PACKET_COALESCE_DELAY);
        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &[0x22]);
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        assert!(traffic_rx.try_recv().is_err());
        assert_eq!(session.pending_downlink.back().unwrap().len(), 119);

        hrpd_a8_enqueue_downlink_payload(0x1a05_8001, &mut session, &[0x33]);
        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));

        let packet = traffic_rx.try_recv().expect("full Rev A packet emitted");
        assert_eq!(packet.payload.len(), 5120);
        assert!(traffic_rx.try_recv().is_err());
        assert!(signaling_rx.try_recv().is_err());
        assert!(session.pending_downlink.is_empty());
        assert_eq!(session.rlp_seq, 600);
        assert_eq!(session.rlp_history.len(), 600);
        assert_eq!(session.traffic_window_stats.rlp_full_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_packets, 0);
        assert_eq!(session.traffic_window_stats.rlp_partial_deferred, 2);
    }

    #[test]
    fn hrpd_a8_rlp_nak_queues_history_retransmission() {
        let (signaling_tx, mut signaling_rx) =
            tokio::sync::mpsc::unbounded_channel::<hrpd_air::HrpdForwardSignalingRequest>();
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.last_drc_index = Some(0x0c);
        session.last_drc_at = Some(Instant::now());
        for octet in [0x10, 0x11, 0x12, 0x13] {
            session.pending_downlink.push_back(vec![octet]);
        }

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        let original = traffic_rx.try_recv().unwrap();
        assert!(!original.high_priority);
        assert_eq!(session.rlp_seq, 4);
        assert_eq!(session.rlp_history.len(), 4);

        queue_hrpd_rlp_nak_retransmissions(
            0x1a05_8001,
            &mut session,
            &[hrpd_air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 1,
                window_len: 2,
            }],
        );
        assert_eq!(session.pending_retransmit.len(), 1);
        assert_eq!(session.pending_retransmit[0].sequence, 1);
        assert_eq!(session.pending_retransmit[0].octets, vec![0x11, 0x12]);

        assert!(flush_hrpd_a8_downlink(
            0x1a05_8001,
            &mut session,
            &signaling_tx,
            &traffic_tx
        ));
        let repair = traffic_rx.try_recv().unwrap();
        assert!(repair.high_priority);
        assert!(traffic_rx.try_recv().is_err());
        assert!(signaling_rx.try_recv().is_err());
        assert!(session.pending_retransmit.is_empty());
        assert_eq!(session.rlp_seq, 4);
    }

    #[test]
    fn hrpd_a8_rlp_nak_reaches_v_s_detection() {
        let request = |first_erased, window_len| hrpd_air::HrpdDefaultPacketRlpNakRequest {
            first_erased,
            window_len,
        };

        assert!(!hrpd_rlp_request_reaches_v_s(100, &request(1, 99)));
        assert!(hrpd_rlp_request_reaches_v_s(100, &request(1, 100)));
        assert!(hrpd_rlp_request_reaches_v_s(100, &request(100, 1)));
        assert!(hrpd_rlp_request_reaches_v_s(100, &request(120, 4)));
        assert!(!hrpd_rlp_request_reaches_v_s(100, &request(99, 0)));
        assert!(hrpd_rlp_request_reaches_v_s(
            rlp::SEQUENCE_MODULUS - 4,
            &request(2, 3)
        ));
    }

    #[test]
    fn hrpd_a8_rlp_nak_can_recover_initial_sequence_after_large_burst() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        let octets = (0..12_000)
            .map(|idx| (idx & 0xff) as u8)
            .collect::<Vec<_>>();

        record_hrpd_rlp_history(&mut session, 0, &octets);
        queue_hrpd_rlp_nak_retransmissions(
            0x1a05_8001,
            &mut session,
            &[hrpd_air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 0,
                window_len: 4,
            }],
        );

        assert_eq!(session.pending_retransmit.len(), 1);
        assert_eq!(session.pending_retransmit[0].sequence, 0);
        assert_eq!(session.pending_retransmit[0].octets, vec![0, 1, 2, 3]);
    }

    #[test]
    fn hrpd_a8_rlp_retransmit_does_not_duplicate_history() {
        let (traffic_tx, mut traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        session.traffic_open = true;
        session.session_configuration_complete = true;
        session.traffic_configuration_complete = true;
        session.default_packet_flow_open = true;
        session.physical_layer_subtype = 0;
        session.forward_traffic_mac_subtype = 0;
        let octets = (0..14_776)
            .map(|idx| (idx & 0xff) as u8)
            .collect::<Vec<_>>();
        record_hrpd_rlp_history(&mut session, 0, &octets);

        let segment = HrpdA8QueuedRlpSegment {
            sequence: 8068,
            octets: octets[8068..8189].to_vec(),
            retransmission: true,
        };
        assert_eq!(
            queue_hrpd_a8_default_packet_rlp_segments(
                0x1a05_8001,
                &mut session,
                &traffic_tx,
                &[segment],
                HrpdA8RlpQueueParams {
                    drc_index: 0x0c,
                    physical_bits: 4096,
                    max_segments: 4,
                },
            ),
            HrpdA8RlpQueueStatus::Queued
        );
        assert!(traffic_rx.try_recv().is_ok());
        assert_eq!(session.rlp_history.len(), octets.len());
        assert_eq!(session.traffic_window_stats.forward_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_segments, 1);
        assert_eq!(session.traffic_window_stats.rlp_full_packets, 0);
        assert_eq!(session.traffic_window_stats.rlp_partial_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_new_packets, 0);
        assert_eq!(session.traffic_window_stats.rlp_partial_retx_packets, 1);
        assert_eq!(session.traffic_window_stats.rlp_partial_mixed_packets, 0);

        queue_hrpd_rlp_nak_retransmissions(
            0x1a05_8001,
            &mut session,
            &[hrpd_air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 13_188,
                window_len: 4,
            }],
        );

        assert_eq!(session.pending_retransmit.len(), 1);
        assert_eq!(session.pending_retransmit[0].sequence, 13_188);
        assert_eq!(session.pending_retransmit[0].octets, octets[13_188..13_192]);
    }

    #[test]
    fn hrpd_a8_large_rlp_nak_is_deferred_not_fully_materialized() {
        let mut session = HrpdAnA8Session::new(0x8005_8001, 6);
        let octets = (0..20_000)
            .map(|idx| (idx & 0xff) as u8)
            .collect::<Vec<_>>();
        record_hrpd_rlp_history(&mut session, 0, &octets);

        queue_hrpd_rlp_nak_retransmissions(
            0x1a05_8001,
            &mut session,
            &[hrpd_air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 0,
                window_len: 20_000,
            }],
        );

        assert_eq!(
            session.pending_retransmit.len(),
            HRPD_A8_RLP_RETRANSMIT_MATERIALIZE_BUDGET
        );
        assert_eq!(session.pending_retransmit_requests.len(), 1);
        assert_eq!(
            session.pending_retransmit_requests[0].next_sequence,
            (HRPD_A8_RLP_RETRANSMIT_MATERIALIZE_BUDGET * HRPD_A8_FORWARD_MAX_STREAM_OCTETS) as u32
        );
    }
}
