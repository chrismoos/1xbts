//! HRPD A10 Unstructured Byte Stream packet-session task.
//!
//! This is deliberately separate from `session_task::run_session`, whose wire
//! contract is 1x traffic-channel `SessionFrame`s with 1x RLP rate metadata.
//! HRPD air-side Stream 1 carries Default Packet Application RLP packets; the
//! A8/A10 bearer is spec-shaped as GRE protocol type `0x8881`
//! Unstructured Byte Stream and carries the upper-layer PPP octet stream.
//! The AN/BTS edge wraps and unwraps those octets into HRPD
//! Stream/Connection/MAC/PHY packets.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::engine::{
    PacketSession, PacketTraceEvent, SessionAction, SessionPhase, bytes_to_hex, now_ms,
};
use crate::ip_allocator::IpAllocator;
use crate::ip_transport::IpTransport;
use crate::session_lifecycle::{
    SessionBoundInfo, SessionLifecycleSink, SessionUnboundInfo, UnbindReason,
};
use crate::session_task::{
    PppSessionCacheLookup, PppSessionStore, SessionMetadata, SessionStatus, ppp_identity_key,
};

const HRPD_IP_SAMPLE_LIMIT: u32 = 24;
const HRPD_TCP_DOWNLINK_STALL_THRESHOLD: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
struct HrpdPathStats {
    uplink_a10_frames: u64,
    uplink_a10_bytes: u64,
    downlink_a10_frames: u64,
    downlink_a10_bytes: u64,
    uplink_ip_packets: u64,
    uplink_ip_bytes: u64,
    downlink_ip_packets: u64,
    downlink_ip_bytes: u64,
    uplink_tcp_syn: u64,
    downlink_tcp_syn_ack: u64,
    uplink_tcp_fin_or_rst: u64,
    downlink_tcp_fin_or_rst: u64,
    uplink_tcp_payload_packets: u64,
    uplink_tcp_payload_bytes: u64,
    downlink_tcp_payload_packets: u64,
    downlink_tcp_payload_bytes: u64,
    uplink_udp_packets: u64,
    uplink_udp_bytes: u64,
    downlink_udp_packets: u64,
    downlink_udp_bytes: u64,
}

impl HrpdPathStats {
    fn reset_window(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HrpdTcpFlowKey {
    remote_addr: [u8; 4],
    remote_port: u16,
    mobile_port: u16,
}

#[derive(Debug, Default)]
struct HrpdTcpFlowState {
    mobile_window_scale: Option<u8>,
    mobile_mss: Option<u16>,
    highest_downlink_end: Option<u32>,
    highest_uplink_ack: Option<u32>,
    last_uplink_ack_at: Option<Instant>,
    last_uplink_ack_advance_at: Option<Instant>,
    last_mobile_window_bytes: Option<u64>,
    last_downlink_payload_at: Option<Instant>,
    last_downlink_flight_bytes: Option<u64>,
    last_downlink_window_bytes: Option<u64>,
    last_downlink_window_headroom_bytes: Option<u64>,
    ack_advances_since_last_downlink: u64,
    acked_bytes_since_last_downlink: u64,
    window_updates_since_last_downlink: u64,
    downlink_markers: VecDeque<(u32, Instant)>,
    window: HrpdTcpFlowWindowStats,
}

#[derive(Debug, Default)]
struct HrpdTcpFlowWindowStats {
    downlink_payload_packets: u64,
    downlink_payload_bytes: u64,
    downlink_retransmit_packets: u64,
    downlink_retransmit_bytes: u64,
    uplink_ack_packets: u64,
    uplink_ack_only_packets: u64,
    uplink_ack_advances: u64,
    uplink_duplicate_acks: u64,
    uplink_acked_bytes: u64,
    ack_gap_samples: u64,
    ack_gap_total_us: u128,
    max_ack_gap_us: u64,
    ack_delay_samples: u64,
    ack_delay_total_us: u128,
    max_ack_delay_us: u64,
    min_mobile_window_bytes: Option<u64>,
    max_inflight_bytes: u64,
    min_window_headroom_bytes: Option<u64>,
    max_window_utilization_permille: u64,
    receive_window_full_samples: u64,
    zero_window_advertisements: u64,
    sack_ack_packets: u64,
    sack_blocks: u64,
    ece_ack_packets: u64,
    downlink_stall_resumes: u64,
    max_downlink_stall_us: u64,
    resume_rwnd_limited: u64,
    resume_ack_clocked: u64,
    resume_window_update: u64,
    resume_without_mobile_feedback: u64,
}

impl HrpdTcpFlowWindowStats {
    fn is_active(&self) -> bool {
        self.downlink_payload_packets > 0 || self.uplink_ack_packets > 0
    }
}

#[derive(Debug)]
struct HrpdTcpDownlinkResume {
    key: HrpdTcpFlowKey,
    classification: &'static str,
    gap_us: u64,
    mobile_mss: Option<u16>,
    flight_after_previous_send: Option<u64>,
    window_after_previous_send: Option<u64>,
    headroom_after_previous_send: Option<u64>,
    flight_before_resume: Option<u64>,
    window_before_resume: Option<u64>,
    headroom_before_resume: Option<u64>,
    ack_advances_during_gap: u64,
    acked_bytes_during_gap: u64,
    window_updates_during_gap: u64,
    last_ack_advance_age_us: Option<u64>,
}

#[derive(Debug, Default)]
struct HrpdTcpWindowStats {
    uplink_ack_packets: u64,
    uplink_ack_only_packets: u64,
    uplink_ack_advances: u64,
    uplink_duplicate_acks: u64,
    uplink_acked_bytes: u64,
    max_uplink_ack_gap_us: u64,
    ack_delay_samples: u64,
    ack_delay_total_us: u128,
    max_ack_delay_us: u64,
    min_mobile_window_bytes: Option<u64>,
    max_inflight_bytes: u64,
    downlink_retransmit_packets: u64,
    downlink_retransmit_bytes: u64,
}

#[derive(Debug, Default)]
struct HrpdTcpDiagnostics {
    flows: HashMap<HrpdTcpFlowKey, HrpdTcpFlowState>,
    window: HrpdTcpWindowStats,
}

impl HrpdTcpDiagnostics {
    fn reset_window(&mut self) {
        self.window = HrpdTcpWindowStats::default();
    }

    fn record(
        &mut self,
        direction: &str,
        info: &HrpdIpPacketInfo,
    ) -> Option<HrpdTcpDownlinkResume> {
        self.record_at(direction, info, Instant::now())
    }

    fn record_at(
        &mut self,
        direction: &str,
        info: &HrpdIpPacketInfo,
        now: Instant,
    ) -> Option<HrpdTcpDownlinkResume> {
        let HrpdIpPacketInfo::Tcp {
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            flags,
            sequence,
            acknowledgement,
            window,
            window_scale,
            maximum_segment_size,
            sack_blocks,
            payload_len,
            ..
        } = info
        else {
            return None;
        };
        let syn = flags & 0x02 != 0;
        let fin = flags & 0x01 != 0;
        let ack = flags & 0x10 != 0;
        let key = match direction {
            "downlink" => HrpdTcpFlowKey {
                remote_addr: *src_addr,
                remote_port: *src_port,
                mobile_port: *dst_port,
            },
            "uplink" => HrpdTcpFlowKey {
                remote_addr: *dst_addr,
                remote_port: *dst_port,
                mobile_port: *src_port,
            },
            _ => return None,
        };
        let state = self.flows.entry(key).or_default();
        let mut downlink_resume = None;

        if direction == "downlink" {
            let sequence_len = (*payload_len as u32)
                .saturating_add(u32::from(syn))
                .saturating_add(u32::from(fin));
            let segment_end = sequence.wrapping_add(sequence_len);
            if *payload_len > 0 {
                state.window.downlink_payload_packets =
                    state.window.downlink_payload_packets.saturating_add(1);
                state.window.downlink_payload_bytes = state
                    .window
                    .downlink_payload_bytes
                    .saturating_add(*payload_len as u64);
                if let Some(previous_at) = state.last_downlink_payload_at {
                    let gap_us = now.saturating_duration_since(previous_at).as_micros() as u64;
                    if gap_us >= HRPD_TCP_DOWNLINK_STALL_THRESHOLD.as_micros() as u64 {
                        let (flight_before_resume, window_before_resume, headroom_before_resume) =
                            tcp_flow_window_snapshot(state);
                        let rwnd_limit = u64::from(state.mobile_mss.unwrap_or(1));
                        let classification = if state
                            .last_downlink_window_headroom_bytes
                            .is_some_and(|headroom| headroom <= rwnd_limit)
                        {
                            state.window.resume_rwnd_limited =
                                state.window.resume_rwnd_limited.saturating_add(1);
                            "rwnd_limited"
                        } else if state.ack_advances_since_last_downlink > 0 {
                            state.window.resume_ack_clocked =
                                state.window.resume_ack_clocked.saturating_add(1);
                            "ack_preceded_resume_non_rwnd"
                        } else if state.window_updates_since_last_downlink > 0 {
                            state.window.resume_window_update =
                                state.window.resume_window_update.saturating_add(1);
                            "window_update_non_rwnd"
                        } else {
                            state.window.resume_without_mobile_feedback = state
                                .window
                                .resume_without_mobile_feedback
                                .saturating_add(1);
                            "no_mobile_feedback_before_resume"
                        };
                        state.window.downlink_stall_resumes =
                            state.window.downlink_stall_resumes.saturating_add(1);
                        state.window.max_downlink_stall_us =
                            state.window.max_downlink_stall_us.max(gap_us);
                        downlink_resume = Some(HrpdTcpDownlinkResume {
                            key,
                            classification,
                            gap_us,
                            mobile_mss: state.mobile_mss,
                            flight_after_previous_send: state.last_downlink_flight_bytes,
                            window_after_previous_send: state.last_downlink_window_bytes,
                            headroom_after_previous_send: state.last_downlink_window_headroom_bytes,
                            flight_before_resume,
                            window_before_resume,
                            headroom_before_resume,
                            ack_advances_during_gap: state.ack_advances_since_last_downlink,
                            acked_bytes_during_gap: state.acked_bytes_since_last_downlink,
                            window_updates_during_gap: state.window_updates_since_last_downlink,
                            last_ack_advance_age_us: state
                                .last_uplink_ack_advance_at
                                .map(|at| now.saturating_duration_since(at).as_micros() as u64),
                        });
                    }
                }
            }
            if *payload_len > 0
                && state
                    .highest_downlink_end
                    .is_some_and(|highest| tcp_seq_after(highest, *sequence))
            {
                self.window.downlink_retransmit_packets =
                    self.window.downlink_retransmit_packets.saturating_add(1);
                self.window.downlink_retransmit_bytes = self
                    .window
                    .downlink_retransmit_bytes
                    .saturating_add(*payload_len as u64);
                state.window.downlink_retransmit_packets =
                    state.window.downlink_retransmit_packets.saturating_add(1);
                state.window.downlink_retransmit_bytes = state
                    .window
                    .downlink_retransmit_bytes
                    .saturating_add(*payload_len as u64);
            }
            if sequence_len > 0
                && state
                    .highest_downlink_end
                    .is_none_or(|highest| tcp_seq_after(segment_end, highest))
            {
                state.highest_downlink_end = Some(segment_end);
                state.downlink_markers.push_back((segment_end, now));
                while state.downlink_markers.len() > 4096 {
                    state.downlink_markers.pop_front();
                }
            }
            if *payload_len > 0 {
                state.last_downlink_payload_at = Some(now);
                let (flight, rwnd, headroom) = tcp_flow_window_snapshot(state);
                state.last_downlink_flight_bytes = flight;
                state.last_downlink_window_bytes = rwnd;
                state.last_downlink_window_headroom_bytes = headroom;
                state.ack_advances_since_last_downlink = 0;
                state.acked_bytes_since_last_downlink = 0;
                state.window_updates_since_last_downlink = 0;
            }
        } else if syn {
            state.mobile_window_scale = Some(window_scale.unwrap_or(0).min(14));
            state.mobile_mss = *maximum_segment_size;
        }

        if direction == "uplink" && ack {
            self.window.uplink_ack_packets = self.window.uplink_ack_packets.saturating_add(1);
            state.window.uplink_ack_packets = state.window.uplink_ack_packets.saturating_add(1);
            if *payload_len == 0 && flags & 0x07 == 0 {
                self.window.uplink_ack_only_packets =
                    self.window.uplink_ack_only_packets.saturating_add(1);
                state.window.uplink_ack_only_packets =
                    state.window.uplink_ack_only_packets.saturating_add(1);
            }
            if let Some(previous_at) = state.last_uplink_ack_at {
                let gap_us = now.saturating_duration_since(previous_at).as_micros() as u64;
                self.window.max_uplink_ack_gap_us = self.window.max_uplink_ack_gap_us.max(gap_us);
                state.window.ack_gap_samples = state.window.ack_gap_samples.saturating_add(1);
                state.window.ack_gap_total_us =
                    state.window.ack_gap_total_us.saturating_add(gap_us as u128);
                state.window.max_ack_gap_us = state.window.max_ack_gap_us.max(gap_us);
            }
            state.last_uplink_ack_at = Some(now);

            let previous_ack = state.highest_uplink_ack;
            match state.highest_uplink_ack {
                Some(previous) if tcp_seq_after(*acknowledgement, previous) => {
                    let acknowledged = acknowledgement.wrapping_sub(previous) as u64;
                    self.window.uplink_ack_advances =
                        self.window.uplink_ack_advances.saturating_add(1);
                    self.window.uplink_acked_bytes =
                        self.window.uplink_acked_bytes.saturating_add(acknowledged);
                    state.window.uplink_ack_advances =
                        state.window.uplink_ack_advances.saturating_add(1);
                    state.window.uplink_acked_bytes =
                        state.window.uplink_acked_bytes.saturating_add(acknowledged);
                    state.highest_uplink_ack = Some(*acknowledgement);
                    state.last_uplink_ack_advance_at = Some(now);
                    if state.last_downlink_payload_at.is_some() {
                        state.ack_advances_since_last_downlink =
                            state.ack_advances_since_last_downlink.saturating_add(1);
                        state.acked_bytes_since_last_downlink = state
                            .acked_bytes_since_last_downlink
                            .saturating_add(acknowledged);
                    }
                }
                Some(previous) if *acknowledgement == previous => {
                    self.window.uplink_duplicate_acks =
                        self.window.uplink_duplicate_acks.saturating_add(1);
                    state.window.uplink_duplicate_acks =
                        state.window.uplink_duplicate_acks.saturating_add(1);
                }
                None => state.highest_uplink_ack = Some(*acknowledgement),
                _ => {}
            }

            while state
                .downlink_markers
                .front()
                .is_some_and(|(end, _)| tcp_seq_at_or_after(*acknowledgement, *end))
            {
                let (_, sent_at) = state
                    .downlink_markers
                    .pop_front()
                    .expect("front marker was present");
                let delay_us = now.saturating_duration_since(sent_at).as_micros() as u64;
                self.window.ack_delay_samples = self.window.ack_delay_samples.saturating_add(1);
                self.window.ack_delay_total_us = self
                    .window
                    .ack_delay_total_us
                    .saturating_add(delay_us as u128);
                self.window.max_ack_delay_us = self.window.max_ack_delay_us.max(delay_us);
                state.window.ack_delay_samples = state.window.ack_delay_samples.saturating_add(1);
                state.window.ack_delay_total_us = state
                    .window
                    .ack_delay_total_us
                    .saturating_add(delay_us as u128);
                state.window.max_ack_delay_us = state.window.max_ack_delay_us.max(delay_us);
            }

            let scaled_window = (*window as u64)
                .checked_shl(u32::from(state.mobile_window_scale.unwrap_or(0)))
                .unwrap_or(u64::MAX);
            self.window.min_mobile_window_bytes = Some(
                self.window
                    .min_mobile_window_bytes
                    .map_or(scaled_window, |current| current.min(scaled_window)),
            );
            state.window.min_mobile_window_bytes = Some(
                state
                    .window
                    .min_mobile_window_bytes
                    .map_or(scaled_window, |current| current.min(scaled_window)),
            );
            if scaled_window == 0 {
                state.window.zero_window_advertisements =
                    state.window.zero_window_advertisements.saturating_add(1);
            }
            if state.last_mobile_window_bytes != Some(scaled_window)
                && state.last_downlink_payload_at.is_some()
            {
                state.window_updates_since_last_downlink =
                    state.window_updates_since_last_downlink.saturating_add(1);
            }
            state.last_mobile_window_bytes = Some(scaled_window);
            if *sack_blocks > 0 {
                state.window.sack_ack_packets = state.window.sack_ack_packets.saturating_add(1);
                state.window.sack_blocks =
                    state.window.sack_blocks.saturating_add(*sack_blocks as u64);
            }
            if flags & 0x40 != 0 {
                state.window.ece_ack_packets = state.window.ece_ack_packets.saturating_add(1);
            }

            if previous_ack.is_none() {
                state.last_uplink_ack_advance_at = Some(now);
            }
        }

        if let Some(flight) = tcp_flow_flight_bytes(state) {
            self.window.max_inflight_bytes = self.window.max_inflight_bytes.max(flight);
        }
        record_tcp_flow_window_snapshot(state);
        downlink_resume
    }

    fn log_active_flows(&mut self, session_id: &str, window_secs: f64) {
        for (key, state) in &mut self.flows {
            let stats = &state.window;
            if !stats.is_active() {
                continue;
            }
            let ack_gap_avg_ms = if stats.ack_gap_samples == 0 {
                0.0
            } else {
                stats.ack_gap_total_us as f64 / stats.ack_gap_samples as f64 / 1000.0
            };
            let ack_delay_avg_ms = if stats.ack_delay_samples == 0 {
                0.0
            } else {
                stats.ack_delay_total_us as f64 / stats.ack_delay_samples as f64 / 1000.0
            };
            let acked_per_advance = if stats.uplink_ack_advances == 0 {
                0.0
            } else {
                stats.uplink_acked_bytes as f64 / stats.uplink_ack_advances as f64
            };
            let (inflight, rwnd, headroom) = tcp_flow_window_snapshot(state);
            log::debug!(
                "hrpd-packet-service: session {} tcp_flow_health remote={}.{}.{}.{}:{} mobile_port={} window_scale={} mss={} dl={{packets:{} bytes:{} kbps:{:.1} retransmit_packets:{} retransmit_bytes:{} stall_resumes:{} stall_max_ms:{:.1} resume_rwnd_limited:{} resume_after_ack:{} resume_window_update:{} resume_without_mobile_feedback:{}}} ack={{packets:{} ack_only:{} advances:{} duplicate:{} acked_bytes:{} acked_kbps:{:.1} acked_per_advance:{:.1} gap_avg_ms:{:.1} gap_max_ms:{:.1} delay_samples:{} delay_avg_ms:{:.1} delay_max_ms:{:.1}}} rwnd={{latest:{} min:{} inflight_latest:{} inflight_max:{} headroom_latest:{} headroom_min:{} utilization_max_pct:{:.1} full_samples:{} zero_advertisements:{}}} signals={{sack_ack_packets:{} sack_blocks:{} ece_ack_packets:{}}}",
                session_id,
                key.remote_addr[0],
                key.remote_addr[1],
                key.remote_addr[2],
                key.remote_addr[3],
                key.remote_port,
                key.mobile_port,
                state.mobile_window_scale.unwrap_or(0),
                state.mobile_mss.unwrap_or(0),
                stats.downlink_payload_packets,
                stats.downlink_payload_bytes,
                stats.downlink_payload_bytes as f64 * 8.0 / window_secs / 1000.0,
                stats.downlink_retransmit_packets,
                stats.downlink_retransmit_bytes,
                stats.downlink_stall_resumes,
                stats.max_downlink_stall_us as f64 / 1000.0,
                stats.resume_rwnd_limited,
                stats.resume_ack_clocked,
                stats.resume_window_update,
                stats.resume_without_mobile_feedback,
                stats.uplink_ack_packets,
                stats.uplink_ack_only_packets,
                stats.uplink_ack_advances,
                stats.uplink_duplicate_acks,
                stats.uplink_acked_bytes,
                stats.uplink_acked_bytes as f64 * 8.0 / window_secs / 1000.0,
                acked_per_advance,
                ack_gap_avg_ms,
                stats.max_ack_gap_us as f64 / 1000.0,
                stats.ack_delay_samples,
                ack_delay_avg_ms,
                stats.max_ack_delay_us as f64 / 1000.0,
                rwnd.unwrap_or(0),
                stats.min_mobile_window_bytes.unwrap_or(0),
                inflight.unwrap_or(0),
                stats.max_inflight_bytes,
                headroom.unwrap_or(0),
                stats.min_window_headroom_bytes.unwrap_or(0),
                stats.max_window_utilization_permille as f64 / 10.0,
                stats.receive_window_full_samples,
                stats.zero_window_advertisements,
                stats.sack_ack_packets,
                stats.sack_blocks,
                stats.ece_ack_packets,
            );
            state.window = HrpdTcpFlowWindowStats::default();
        }
    }
}

fn tcp_flow_flight_bytes(state: &HrpdTcpFlowState) -> Option<u64> {
    let (sent, acked) = (state.highest_downlink_end?, state.highest_uplink_ack?);
    Some(if tcp_seq_after(sent, acked) {
        sent.wrapping_sub(acked) as u64
    } else {
        0
    })
}

fn tcp_flow_window_snapshot(state: &HrpdTcpFlowState) -> (Option<u64>, Option<u64>, Option<u64>) {
    let flight = tcp_flow_flight_bytes(state);
    let rwnd = state.last_mobile_window_bytes;
    let headroom = flight
        .zip(rwnd)
        .map(|(flight, rwnd)| rwnd.saturating_sub(flight));
    (flight, rwnd, headroom)
}

fn record_tcp_flow_window_snapshot(state: &mut HrpdTcpFlowState) {
    let (Some(flight), Some(rwnd), Some(headroom)) = tcp_flow_window_snapshot(state) else {
        return;
    };
    state.window.max_inflight_bytes = state.window.max_inflight_bytes.max(flight);
    state.window.min_window_headroom_bytes = Some(
        state
            .window
            .min_window_headroom_bytes
            .map_or(headroom, |current| current.min(headroom)),
    );
    let utilization_permille = if rwnd == 0 {
        u64::from(flight > 0) * 1000
    } else {
        flight.saturating_mul(1000).div_ceil(rwnd).min(1000)
    };
    state.window.max_window_utilization_permille = state
        .window
        .max_window_utilization_permille
        .max(utilization_permille);
    if flight > 0 && flight >= rwnd {
        state.window.receive_window_full_samples =
            state.window.receive_window_full_samples.saturating_add(1);
    }
}

fn tcp_seq_after(value: u32, reference: u32) -> bool {
    (value.wrapping_sub(reference) as i32) > 0
}

fn tcp_seq_at_or_after(value: u32, reference: u32) -> bool {
    value == reference || tcp_seq_after(value, reference)
}

#[derive(Default)]
struct HrpdIpLogState {
    uplink_samples: u32,
    downlink_samples: u32,
}

/// Runs one HRPD A10 Unstructured Byte Stream PPP/IP session.
///
/// `uplink_rx` receives A10 bearer payload octets from the PCF.
/// `downlink_tx` emits A10 bearer payload octets toward the PCF. The caller is
/// responsible for A10 GRE encapsulation and the PCF/AN is responsible for
/// air-side HRPD Default Packet RLP wrapping.
struct HrpdA10Session {
    session_id: String,
    service_option: u32,
    transport: Box<dyn IpTransport>,
    uplink_rx: mpsc::Receiver<Vec<u8>>,
    downlink_tx: mpsc::Sender<Vec<u8>>,
    shutdown_rx: oneshot::Receiver<()>,
    status: Arc<Mutex<SessionStatus>>,
    allocator: Arc<dyn IpAllocator>,
    metadata: SessionMetadata,
    lifecycle_sink: Arc<dyn SessionLifecycleSink>,
    ppp_session_store: Option<Arc<PppSessionStore>>,
    ppp_session_timeout: Duration,
}

pub async fn run_hrpd_a10_byte_stream_session(
    session_id: String,
    service_option: u32,
    transport: Box<dyn IpTransport>,
    uplink_rx: mpsc::Receiver<Vec<u8>>,
    downlink_tx: mpsc::Sender<Vec<u8>>,
    shutdown_rx: oneshot::Receiver<()>,
    status: Arc<Mutex<SessionStatus>>,
    allocator: Arc<dyn IpAllocator>,
    metadata: SessionMetadata,
    lifecycle_sink: Arc<dyn SessionLifecycleSink>,
    ppp_session_store: Option<Arc<PppSessionStore>>,
    ppp_session_timeout: Duration,
) {
    HrpdA10Session {
        session_id,
        service_option,
        transport,
        uplink_rx,
        downlink_tx,
        shutdown_rx,
        status,
        allocator,
        metadata,
        lifecycle_sink,
        ppp_session_store,
        ppp_session_timeout,
    }
    .run()
    .await
}

impl HrpdA10Session {
    async fn run(mut self) {
        let ppp_identity_key = ppp_identity_key(&self.metadata);
        let allocation_key = hrpd_allocation_key(&self.session_id, &self.metadata);
        let mut ppp_resume_state = None;
        if let (Some(store), Some(identity_key)) = (&self.ppp_session_store, &ppp_identity_key) {
            match store.lookup(identity_key, self.ppp_session_timeout) {
                PppSessionCacheLookup::Hit(hit) => {
                    log::info!(
                        "hrpd-packet-service: session {} PPP cache hit identity={} peer_ip={} idle_secs={} allocation_key={}",
                        self.session_id,
                        identity_key,
                        hit.peer_ip,
                        hit.idle_for.as_secs(),
                        hit.allocation_key
                    );
                    ppp_resume_state = Some(hit.state);
                }
                PppSessionCacheLookup::Expired(expired) => {
                    log::info!(
                        "hrpd-packet-service: session {} PPP cache expired identity={} peer_ip={} idle_secs={} allocation_key={}",
                        self.session_id,
                        identity_key,
                        expired.peer_ip,
                        expired.idle_for.as_secs(),
                        expired.allocation_key
                    );
                    self.allocator.release(&expired.allocation_key);
                }
                PppSessionCacheLookup::Miss => {
                    log::info!(
                        "hrpd-packet-service: session {} PPP cache miss identity={}",
                        self.session_id,
                        identity_key
                    );
                }
            }
        } else {
            log::info!(
                "hrpd-packet-service: session {} PPP cache unavailable identity={}",
                self.session_id,
                ppp_identity_key.as_deref().unwrap_or("unknown")
            );
        }

        let Some(mut ipcp_config) = self.allocator.allocate(&allocation_key) else {
            log::warn!(
                "hrpd-packet-service: session {} IP pool exhausted for key {}",
                self.session_id,
                allocation_key
            );
            return;
        };
        if let Some(resume_state) = &ppp_resume_state {
            let cached_config = resume_state.ipcp.config.clone();
            if cached_config.peer_ip == ipcp_config.peer_ip {
                ipcp_config = cached_config;
            } else {
                log::warn!(
                    "hrpd-packet-service: session {} PPP cache discarded identity={} cached_peer_ip={} allocated_peer_ip={}",
                    self.session_id,
                    ppp_identity_key.as_deref().unwrap_or("unknown"),
                    cached_config.peer_ip,
                    ipcp_config.peer_ip
                );
                if let (Some(store), Some(identity_key)) =
                    (&self.ppp_session_store, &ppp_identity_key)
                {
                    if let Some(removed) = store.remove(identity_key) {
                        self.allocator.release(&removed.allocation_key);
                    }
                }
                ppp_resume_state = None;
            }
        }
        let bind_peer_ip = ipcp_config.peer_ip;
        let bind_our_ip = ipcp_config.our_ip;
        {
            let mut s = self.status.lock().unwrap();
            s.peer_ip = bind_peer_ip.to_string();
            s.our_ip = bind_our_ip.to_string();
        }
        self.lifecycle_sink.on_bound(SessionBoundInfo {
            session_id: self.session_id.clone(),
            service_option: self.service_option,
            subscriber_id: self.metadata.subscriber_id.clone(),
            imsi: self.metadata.imsi.clone(),
            esn: self.metadata.esn,
            meid: self.metadata.meid.clone(),
            hrpd_mn_id: self.metadata.hrpd_mn_id.clone(),
            hrpd_mn_id_source: self.metadata.hrpd_mn_id_source.clone(),
            subscriber_imsi: self.metadata.subscriber_imsi.clone(),
            peer_ip: bind_peer_ip,
            our_ip: bind_our_ip,
        });

        let mut session = PacketSession::new_a10_unstructured_byte_stream(
            self.service_option,
            ipcp_config,
            ppp_resume_state,
        );
        session.set_log_context(format!(
            "hrpd-session={} uati={}",
            self.session_id, self.metadata.traffic_walsh_code
        ));
        let (to_mobile_tx, mut to_mobile_rx) = mpsc::channel::<Vec<u8>>(256);
        let mut transport_ready = false;
        let mut first_uplink_ip_logged = false;
        let mut first_downlink_ip_logged = false;
        let mut ip_log_state = HrpdIpLogState::default();
        let mut last_ppp_activity_at = Instant::now();
        let mut peer_requested_lcp_terminate = false;
        let mut path_health_interval = tokio::time::interval(Duration::from_secs(5));
        path_health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut path_stats = HrpdPathStats::default();
        let mut tcp_diagnostics = HrpdTcpDiagnostics::default();
        let session_started = Instant::now();

        let mut tick_interval = tokio::time::interval(Duration::from_millis(20));
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        log::info!(
            "hrpd-packet-service: session {} started (SO {}, peer={} gateway={})",
            self.session_id,
            self.service_option,
            bind_peer_ip,
            bind_our_ip
        );

        loop {
            tokio::select! {
                _ = &mut self.shutdown_rx => {
                    log::info!("hrpd-packet-service: session {} shutdown requested", self.session_id);
                    break;
                }
                uplink = self.uplink_rx.recv() => {
                    let Some(bits) = uplink else {
                        log::info!("hrpd-packet-service: session {} uplink A10 byte-stream channel closed", self.session_id);
                        break;
                    };
                    {
                        let mut s = self.status.lock().unwrap();
                        s.uplink_frames = s.uplink_frames.saturating_add(1);
                        s.uplink_bytes = s.uplink_bytes.saturating_add(bits.len() as u64);
                        s.last_uplink_at_ms = now_ms();
                        s.last_activity_at_ms = s.last_uplink_at_ms;
                    }
                    path_stats.uplink_a10_frames = path_stats.uplink_a10_frames.saturating_add(1);
                    path_stats.uplink_a10_bytes = path_stats.uplink_a10_bytes.saturating_add(bits.len() as u64);
                    let actions = session.ingest_a10_byte_stream(&bits);
                    if process_hrpd_actions(
                        &self.session_id,
                        &mut session,
                        &mut self.transport,
                        &mut transport_ready,
                        &to_mobile_tx,
                        &self.downlink_tx,
                        &self.status,
                        &mut first_uplink_ip_logged,
                        &mut ip_log_state,
                        &mut path_stats,
                        &mut tcp_diagnostics,
                        actions,
                    ).await {
                        break;
                    }
                    ensure_hrpd_transport_ready(
                        &self.session_id,
                        &session,
                        &mut self.transport,
                        &mut transport_ready,
                        &to_mobile_tx,
                        &self.status,
                    );
                    if session.take_ppp_activity() {
                        last_ppp_activity_at = Instant::now();
                    }
                    if session.take_peer_requested_lcp_terminate() {
                        peer_requested_lcp_terminate = true;
                        log::info!(
                            "hrpd-packet-service: session {} peer requested LCP terminate; closing HRPD A10 packet task",
                            self.session_id
                        );
                        break;
                    }
                }
                _ = tick_interval.tick() => {
                    let actions = session.tick(None);
                    if process_hrpd_actions(
                        &self.session_id,
                        &mut session,
                        &mut self.transport,
                        &mut transport_ready,
                        &to_mobile_tx,
                        &self.downlink_tx,
                        &self.status,
                        &mut first_uplink_ip_logged,
                        &mut ip_log_state,
                        &mut path_stats,
                        &mut tcp_diagnostics,
                        actions,
                    ).await {
                        break;
                    }
                    ensure_hrpd_transport_ready(
                        &self.session_id,
                        &session,
                        &mut self.transport,
                        &mut transport_ready,
                        &to_mobile_tx,
                        &self.status,
                    );
                    if session.take_ppp_activity() {
                        last_ppp_activity_at = Instant::now();
                    }
                    if session.take_peer_requested_lcp_terminate() {
                        peer_requested_lcp_terminate = true;
                        log::info!(
                            "hrpd-packet-service: session {} peer requested LCP terminate; closing HRPD A10 packet task",
                            self.session_id
                        );
                        break;
                    }
                }
                _ = path_health_interval.tick() => {
                    const PATH_HEALTH_WINDOW_SECS: f64 = 5.0;
                    let ul_a10_kbps =
                        path_stats.uplink_a10_bytes as f64 * 8.0 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                    let dl_a10_kbps =
                        path_stats.downlink_a10_bytes as f64 * 8.0 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                    let ul_ip_kbps =
                        path_stats.uplink_ip_bytes as f64 * 8.0 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                    let dl_ip_kbps =
                        path_stats.downlink_ip_bytes as f64 * 8.0 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                    let tcp_window = &tcp_diagnostics.window;
                    let tcp_acked_kbps = tcp_window.uplink_acked_bytes as f64 * 8.0
                        / PATH_HEALTH_WINDOW_SECS
                        / 1000.0;
                    let ack_delay_avg_ms = if tcp_window.ack_delay_samples == 0 {
                        0.0
                    } else {
                        tcp_window.ack_delay_total_us as f64
                            / tcp_window.ack_delay_samples as f64
                            / 1000.0
                    };
                    let telemetry = session.telemetry();
                    log::debug!(
                        "hrpd-packet-service: session {} path_health age_ms={} phase={:?} lcp={} ipcp={} ppp_restarts={{lcp:{} ipcp:{} ipcp_omitted_ip_naks:{}}} transport_ready={} peer_ip={} uplink_a10_frames={} uplink_a10_bytes={} ul_a10_kbps={:.1} downlink_a10_frames={} downlink_a10_bytes={} dl_a10_kbps={:.1} uplink_ip={} uplink_ip_bytes={} ul_ip_kbps={:.1} downlink_ip={} downlink_ip_bytes={} dl_ip_kbps={:.1} uplink_ingest=immediate tcp={{ul_syn:{} dl_syn_ack:{} ul_payload_pkts:{} ul_payload_bytes:{} dl_payload_pkts:{} dl_payload_bytes:{} ul_fin_rst:{} dl_fin_rst:{}}} tcp_path={{flows:{} ul_ack:{} ack_only:{} ack_advance:{} dup_ack:{} acked_bytes:{} acked_kbps:{:.1} ack_gap_max_ms:{:.1} ack_delay_samples:{} ack_delay_avg_ms:{:.1} ack_delay_max_ms:{:.1} mobile_window_min:{} inflight_max:{} dl_retx_pkts:{} dl_retx_bytes:{}}} udp={{ul_pkts:{} ul_bytes:{} dl_pkts:{} dl_bytes:{}}}",
                        self.session_id,
                        session_started.elapsed().as_millis(),
                        session.phase(),
                        telemetry.lcp_state,
                        telemetry.ipcp_state,
                        telemetry.lcp_configure_restarts,
                        telemetry.ipcp_configure_restarts,
                        telemetry.ipcp_omitted_peer_ip_naks,
                        transport_ready,
                        session.peer_ip(),
                        path_stats.uplink_a10_frames,
                        path_stats.uplink_a10_bytes,
                        ul_a10_kbps,
                        path_stats.downlink_a10_frames,
                        path_stats.downlink_a10_bytes,
                        dl_a10_kbps,
                        path_stats.uplink_ip_packets,
                        path_stats.uplink_ip_bytes,
                        ul_ip_kbps,
                        path_stats.downlink_ip_packets,
                        path_stats.downlink_ip_bytes,
                        dl_ip_kbps,
                        path_stats.uplink_tcp_syn,
                        path_stats.downlink_tcp_syn_ack,
                        path_stats.uplink_tcp_payload_packets,
                        path_stats.uplink_tcp_payload_bytes,
                        path_stats.downlink_tcp_payload_packets,
                        path_stats.downlink_tcp_payload_bytes,
                        path_stats.uplink_tcp_fin_or_rst,
                        path_stats.downlink_tcp_fin_or_rst,
                        tcp_diagnostics.flows.len(),
                        tcp_window.uplink_ack_packets,
                        tcp_window.uplink_ack_only_packets,
                        tcp_window.uplink_ack_advances,
                        tcp_window.uplink_duplicate_acks,
                        tcp_window.uplink_acked_bytes,
                        tcp_acked_kbps,
                        tcp_window.max_uplink_ack_gap_us as f64 / 1000.0,
                        tcp_window.ack_delay_samples,
                        ack_delay_avg_ms,
                        tcp_window.max_ack_delay_us as f64 / 1000.0,
                        tcp_window.min_mobile_window_bytes.unwrap_or(0),
                        tcp_window.max_inflight_bytes,
                        tcp_window.downlink_retransmit_packets,
                        tcp_window.downlink_retransmit_bytes,
                        path_stats.uplink_udp_packets,
                        path_stats.uplink_udp_bytes,
                        path_stats.downlink_udp_packets,
                        path_stats.downlink_udp_bytes
                    );
                    tcp_diagnostics.log_active_flows(&self.session_id, PATH_HEALTH_WINDOW_SECS);
                    path_stats.reset_window();
                    tcp_diagnostics.reset_window();
                }
                Some(ip_data) = to_mobile_rx.recv(), if transport_ready => {
                    if !first_downlink_ip_logged {
                        log::info!(
                            "hrpd-packet-service: session {} first downlink IP {}",
                            self.session_id,
                            summarize_hrpd_ip_packet(&ip_data)
                        );
                        first_downlink_ip_logged = true;
                    }
                    log_hrpd_ip_sample(&self.session_id, "downlink", &ip_data, &mut ip_log_state);
                    path_stats.downlink_ip_packets = path_stats.downlink_ip_packets.saturating_add(1);
                    path_stats.downlink_ip_bytes =
                        path_stats.downlink_ip_bytes.saturating_add(ip_data.len() as u64);
                    update_hrpd_ip_stats(
                        &self.session_id,
                        &mut path_stats,
                        &mut tcp_diagnostics,
                        "downlink",
                        &ip_data,
                    );
                    session.send_ip_packet(&ip_data);
                    record_hrpd_ip_capture(&self.status, "downlink", &ip_data, "network -> mobile");
                    let actions = session.emit_full_pending_downlink();
                    if process_hrpd_actions(
                        &self.session_id,
                        &mut session,
                        &mut self.transport,
                        &mut transport_ready,
                        &to_mobile_tx,
                        &self.downlink_tx,
                        &self.status,
                        &mut first_uplink_ip_logged,
                        &mut ip_log_state,
                        &mut path_stats,
                        &mut tcp_diagnostics,
                        actions,
                    ).await {
                        break;
                    }
                    if session.take_ppp_activity() {
                        last_ppp_activity_at = Instant::now();
                    }
                }
            }
        }

        session.close();
        let ppp_cache_kept = if peer_requested_lcp_terminate {
            if let (Some(store), Some(identity_key)) = (&self.ppp_session_store, &ppp_identity_key)
            {
                if let Some(removed) = store.remove(identity_key) {
                    self.allocator.release(&removed.allocation_key);
                    log::info!(
                        "hrpd-packet-service: session {} removed PPP cache after peer terminate identity={} peer_ip={} allocation_key={}",
                        self.session_id,
                        identity_key,
                        removed.peer_ip,
                        removed.allocation_key
                    );
                }
            }
            false
        } else if let (Some(store), Some(identity_key), Some(snapshot)) = (
            &self.ppp_session_store,
            ppp_identity_key.as_ref(),
            session.snapshot_ppp_state(),
        ) {
            let peer_ip = snapshot.ipcp.config.peer_ip;
            store.store(
                identity_key.clone(),
                allocation_key.clone(),
                snapshot,
                last_ppp_activity_at,
            );
            log::info!(
                "hrpd-packet-service: session {} stored open PPP session identity={} peer_ip={} allocation_key={} idle_secs={}",
                self.session_id,
                identity_key,
                peer_ip,
                allocation_key,
                last_ppp_activity_at.elapsed().as_secs()
            );
            true
        } else {
            false
        };
        self.transport.teardown();
        if ppp_cache_kept {
            log::info!(
                "hrpd-packet-service: session {} keeping IP allocation for cached PPP session key={}",
                self.session_id,
                allocation_key
            );
        } else {
            self.allocator.release(&allocation_key);
        }
        self.lifecycle_sink.on_unbound(SessionUnboundInfo {
            session_id: self.session_id.clone(),
            subscriber_id: self.metadata.subscriber_id,
            imsi: self.metadata.imsi,
            esn: self.metadata.esn,
            meid: self.metadata.meid,
            hrpd_mn_id: self.metadata.hrpd_mn_id,
            hrpd_mn_id_source: self.metadata.hrpd_mn_id_source,
            subscriber_imsi: self.metadata.subscriber_imsi,
            peer_ip: bind_peer_ip,
            reason: UnbindReason::UplinkClosed,
        });
        {
            let mut s = self.status.lock().unwrap();
            s.sync_telemetry(SessionPhase::Closed, session.telemetry());
        }
        log::info!("hrpd-packet-service: session {} ended", self.session_id);
    }
}

fn hrpd_allocation_key(session_id: &str, metadata: &SessionMetadata) -> String {
    if let Some(identity) = ppp_identity_key(metadata) {
        format!("device:{identity}")
    } else if !metadata.mobile_address.is_empty() {
        format!("hrpd-mobile:{}", metadata.mobile_address)
    } else {
        format!("hrpd-session:{session_id}")
    }
}

async fn process_hrpd_actions(
    session_id: &str,
    session: &mut PacketSession,
    transport: &mut Box<dyn IpTransport>,
    transport_ready: &mut bool,
    to_mobile_tx: &mpsc::Sender<Vec<u8>>,
    downlink_tx: &mpsc::Sender<Vec<u8>>,
    status: &Arc<Mutex<SessionStatus>>,
    first_uplink_ip_logged: &mut bool,
    ip_log_state: &mut HrpdIpLogState,
    path_stats: &mut HrpdPathStats,
    tcp_diagnostics: &mut HrpdTcpDiagnostics,
    actions: Vec<SessionAction>,
) -> bool {
    for action in actions {
        match action {
            SessionAction::CloseSession { reason } => {
                log::warn!(
                    "hrpd-packet-service: session {} closing from engine: {}",
                    session_id,
                    reason
                );
                session.close();
                return true;
            }
            SessionAction::SendFrame { bits, .. } => {
                let len = bits.len();
                if downlink_tx.send(bits).await.is_err() {
                    log::warn!(
                        "hrpd-packet-service: session {} downlink A10 byte-stream channel closed",
                        session_id
                    );
                    return true;
                }
                path_stats.downlink_a10_frames = path_stats.downlink_a10_frames.saturating_add(1);
                path_stats.downlink_a10_bytes =
                    path_stats.downlink_a10_bytes.saturating_add(len as u64);
                let mut s = status.lock().unwrap();
                s.downlink_frames = s.downlink_frames.saturating_add(1);
                s.downlink_bytes = s.downlink_bytes.saturating_add(len as u64);
                s.last_downlink_at_ms = now_ms();
                s.last_activity_at_ms = s.last_downlink_at_ms;
            }
            SessionAction::SendSchFrame { .. } => {
                log::warn!(
                    "hrpd-packet-service: session {} ignored unexpected 1x SCH action in HRPD task",
                    session_id
                );
            }
            SessionAction::DeliverIpPacket(ip_data) => {
                record_hrpd_ip_capture(status, "uplink", &ip_data, "mobile -> network");
                path_stats.uplink_ip_packets = path_stats.uplink_ip_packets.saturating_add(1);
                path_stats.uplink_ip_bytes = path_stats
                    .uplink_ip_bytes
                    .saturating_add(ip_data.len() as u64);
                update_hrpd_ip_stats(session_id, path_stats, tcp_diagnostics, "uplink", &ip_data);
                if !*first_uplink_ip_logged {
                    log::info!(
                        "hrpd-packet-service: session {} first uplink IP {}",
                        session_id,
                        summarize_hrpd_ip_packet(&ip_data)
                    );
                    *first_uplink_ip_logged = true;
                }
                log_hrpd_ip_sample(session_id, "uplink", &ip_data, ip_log_state);
                ensure_hrpd_transport_ready(
                    session_id,
                    session,
                    transport,
                    transport_ready,
                    to_mobile_tx,
                    status,
                );
                if *transport_ready && let Err(err) = transport.send_to_network(&ip_data) {
                    log::warn!(
                        "hrpd-packet-service: session {} send_to_network failed: {}",
                        session_id,
                        err
                    );
                }
            }
        }
    }
    {
        let mut s = status.lock().unwrap();
        s.sync_telemetry(session.phase(), session.telemetry());
        if session.phase() == SessionPhase::Active {
            s.peer_ip = session.peer_ip().to_string();
            s.our_ip = session.our_ip().to_string();
        }
    }
    session.phase() == SessionPhase::Closed
}

fn ensure_hrpd_transport_ready(
    session_id: &str,
    session: &PacketSession,
    transport: &mut Box<dyn IpTransport>,
    transport_ready: &mut bool,
    to_mobile_tx: &mpsc::Sender<Vec<u8>>,
    status: &Arc<Mutex<SessionStatus>>,
) -> bool {
    if *transport_ready || session.phase() != SessionPhase::Active {
        return false;
    }

    let local_ip = session.our_ip();
    let peer_ip = session.peer_ip();
    match transport.setup(local_ip, peer_ip, to_mobile_tx.clone()) {
        Ok(name) => {
            log::info!(
                "hrpd-packet-service: session {} transport {} ready after IPCP ({}->{})",
                session_id,
                name,
                local_ip,
                peer_ip
            );
            *transport_ready = true;
            let mut s = status.lock().unwrap();
            s.tun_device = name;
            s.peer_ip = peer_ip.to_string();
            s.our_ip = local_ip.to_string();
            true
        }
        Err(err) => {
            log::warn!(
                "hrpd-packet-service: session {} transport setup failed after IPCP: {}",
                session_id,
                err
            );
            false
        }
    }
}

fn record_hrpd_ip_capture(
    status: &Arc<Mutex<SessionStatus>>,
    direction: &str,
    packet: &[u8],
    detail_prefix: &str,
) {
    let event = PacketTraceEvent {
        timestamp_ms: now_ms(),
        layer: "ip".to_string(),
        direction: direction.to_string(),
        summary: summarize_hrpd_ip_packet(packet),
        detail: format!("{detail_prefix} len={}", packet.len()),
        payload_hex: bytes_to_hex(packet),
    };
    let mut s = status.lock().unwrap();
    s.push_capture_event(event);
}

fn log_hrpd_ip_sample(
    session_id: &str,
    direction: &str,
    packet: &[u8],
    state: &mut HrpdIpLogState,
) {
    let samples = if direction == "uplink" {
        &mut state.uplink_samples
    } else {
        &mut state.downlink_samples
    };
    if *samples >= HRPD_IP_SAMPLE_LIMIT {
        return;
    }
    *samples += 1;
    log::debug!(
        "hrpd-packet-service: session {} {} IP sample#{} {}",
        session_id,
        direction,
        *samples,
        summarize_hrpd_ip_packet(packet)
    );
}

fn update_hrpd_ip_stats(
    session_id: &str,
    stats: &mut HrpdPathStats,
    tcp_diagnostics: &mut HrpdTcpDiagnostics,
    direction: &str,
    packet: &[u8],
) {
    let Some(info) = parse_hrpd_ip_packet(packet) else {
        return;
    };
    if let Some(resume) = tcp_diagnostics.record(direction, &info) {
        log::debug!(
            "hrpd-packet-service: session {} tcp_downlink_resume remote={}.{}.{}.{}:{} mobile_port={} classification={} gap_ms={:.1} mss={} previous={{flight:{} rwnd:{} headroom:{}}} before_resume={{flight:{} rwnd:{} headroom:{}}} mobile_feedback={{ack_advances:{} acked_bytes:{} window_updates:{} last_ack_advance_age_ms:{:.1}}}",
            session_id,
            resume.key.remote_addr[0],
            resume.key.remote_addr[1],
            resume.key.remote_addr[2],
            resume.key.remote_addr[3],
            resume.key.remote_port,
            resume.key.mobile_port,
            resume.classification,
            resume.gap_us as f64 / 1000.0,
            resume.mobile_mss.unwrap_or(0),
            resume.flight_after_previous_send.unwrap_or(0),
            resume.window_after_previous_send.unwrap_or(0),
            resume.headroom_after_previous_send.unwrap_or(0),
            resume.flight_before_resume.unwrap_or(0),
            resume.window_before_resume.unwrap_or(0),
            resume.headroom_before_resume.unwrap_or(0),
            resume.ack_advances_during_gap,
            resume.acked_bytes_during_gap,
            resume.window_updates_during_gap,
            resume.last_ack_advance_age_us.unwrap_or(0) as f64 / 1000.0,
        );
    }
    match info {
        HrpdIpPacketInfo::Tcp {
            flags, payload_len, ..
        } => {
            let syn = flags & 0x02 != 0;
            let ack = flags & 0x10 != 0;
            let fin_or_rst = flags & 0x05 != 0;
            match direction {
                "uplink" => {
                    if syn && !ack {
                        stats.uplink_tcp_syn = stats.uplink_tcp_syn.saturating_add(1);
                    }
                    if fin_or_rst {
                        stats.uplink_tcp_fin_or_rst = stats.uplink_tcp_fin_or_rst.saturating_add(1);
                    }
                    if payload_len > 0 {
                        stats.uplink_tcp_payload_packets =
                            stats.uplink_tcp_payload_packets.saturating_add(1);
                        stats.uplink_tcp_payload_bytes = stats
                            .uplink_tcp_payload_bytes
                            .saturating_add(payload_len as u64);
                    }
                }
                "downlink" => {
                    if syn && ack {
                        stats.downlink_tcp_syn_ack = stats.downlink_tcp_syn_ack.saturating_add(1);
                    }
                    if fin_or_rst {
                        stats.downlink_tcp_fin_or_rst =
                            stats.downlink_tcp_fin_or_rst.saturating_add(1);
                    }
                    if payload_len > 0 {
                        stats.downlink_tcp_payload_packets =
                            stats.downlink_tcp_payload_packets.saturating_add(1);
                        stats.downlink_tcp_payload_bytes = stats
                            .downlink_tcp_payload_bytes
                            .saturating_add(payload_len as u64);
                    }
                }
                _ => {}
            }
        }
        HrpdIpPacketInfo::Udp { payload_len, .. } => match direction {
            "uplink" => {
                stats.uplink_udp_packets = stats.uplink_udp_packets.saturating_add(1);
                stats.uplink_udp_bytes = stats.uplink_udp_bytes.saturating_add(payload_len as u64);
            }
            "downlink" => {
                stats.downlink_udp_packets = stats.downlink_udp_packets.saturating_add(1);
                stats.downlink_udp_bytes =
                    stats.downlink_udp_bytes.saturating_add(payload_len as u64);
            }
            _ => {}
        },
        HrpdIpPacketInfo::Other => {}
    }
}

fn summarize_hrpd_ip_packet(packet: &[u8]) -> String {
    match parse_hrpd_ip_packet(packet) {
        Some(HrpdIpPacketInfo::Tcp {
            src,
            dst,
            src_port,
            dst_port,
            flags,
            ip_len,
            payload_len,
            ..
        }) => format!(
            "IPv4 {src}:{src_port} -> {dst}:{dst_port} tcp flags={} len={} payload={}",
            format_tcp_flags(flags),
            ip_len,
            payload_len
        ),
        Some(HrpdIpPacketInfo::Udp {
            src,
            dst,
            src_port,
            dst_port,
            ip_len,
            payload_len,
        }) => format!(
            "IPv4 {src}:{src_port} -> {dst}:{dst_port} udp len={} payload={}",
            ip_len, payload_len
        ),
        Some(HrpdIpPacketInfo::Other) => summarize_hrpd_ipv4_header(packet),
        None => summarize_hrpd_ipv4_header(packet),
    }
}

enum HrpdIpPacketInfo {
    Tcp {
        src: String,
        dst: String,
        src_addr: [u8; 4],
        dst_addr: [u8; 4],
        src_port: u16,
        dst_port: u16,
        flags: u8,
        sequence: u32,
        acknowledgement: u32,
        window: u16,
        window_scale: Option<u8>,
        maximum_segment_size: Option<u16>,
        sack_blocks: usize,
        ip_len: usize,
        payload_len: usize,
    },
    Udp {
        src: String,
        dst: String,
        src_port: u16,
        dst_port: u16,
        ip_len: usize,
        payload_len: usize,
    },
    Other,
}

fn parse_hrpd_ip_packet(packet: &[u8]) -> Option<HrpdIpPacketInfo> {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return None;
    }
    let ihl_bytes = usize::from(packet[0] & 0x0f) * 4;
    if ihl_bytes < 20 || packet.len() < ihl_bytes {
        return None;
    }
    let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    let ip_len = if total_len >= ihl_bytes && total_len <= packet.len() {
        total_len
    } else {
        packet.len()
    };
    let src_addr = [packet[12], packet[13], packet[14], packet[15]];
    let dst_addr = [packet[16], packet[17], packet[18], packet[19]];
    let src = format!(
        "{}.{}.{}.{}",
        src_addr[0], src_addr[1], src_addr[2], src_addr[3]
    );
    let dst = format!(
        "{}.{}.{}.{}",
        dst_addr[0], dst_addr[1], dst_addr[2], dst_addr[3]
    );
    match packet[9] {
        6 if ip_len >= ihl_bytes + 20 => {
            let tcp_header_len = usize::from(packet[ihl_bytes + 12] >> 4) * 4;
            if tcp_header_len < 20 || ip_len < ihl_bytes + tcp_header_len {
                return Some(HrpdIpPacketInfo::Other);
            }
            let tcp_options =
                parse_tcp_options(&packet[ihl_bytes + 20..ihl_bytes + tcp_header_len]);
            Some(HrpdIpPacketInfo::Tcp {
                src,
                dst,
                src_addr,
                dst_addr,
                src_port: u16::from_be_bytes([packet[ihl_bytes], packet[ihl_bytes + 1]]),
                dst_port: u16::from_be_bytes([packet[ihl_bytes + 2], packet[ihl_bytes + 3]]),
                flags: packet[ihl_bytes + 13],
                sequence: u32::from_be_bytes([
                    packet[ihl_bytes + 4],
                    packet[ihl_bytes + 5],
                    packet[ihl_bytes + 6],
                    packet[ihl_bytes + 7],
                ]),
                acknowledgement: u32::from_be_bytes([
                    packet[ihl_bytes + 8],
                    packet[ihl_bytes + 9],
                    packet[ihl_bytes + 10],
                    packet[ihl_bytes + 11],
                ]),
                window: u16::from_be_bytes([packet[ihl_bytes + 14], packet[ihl_bytes + 15]]),
                window_scale: tcp_options.window_scale,
                maximum_segment_size: tcp_options.maximum_segment_size,
                sack_blocks: tcp_options.sack_blocks,
                ip_len,
                payload_len: ip_len - ihl_bytes - tcp_header_len,
            })
        }
        17 if ip_len >= ihl_bytes + 8 => Some(HrpdIpPacketInfo::Udp {
            src,
            dst,
            src_port: u16::from_be_bytes([packet[ihl_bytes], packet[ihl_bytes + 1]]),
            dst_port: u16::from_be_bytes([packet[ihl_bytes + 2], packet[ihl_bytes + 3]]),
            ip_len,
            payload_len: ip_len - ihl_bytes - 8,
        }),
        _ => Some(HrpdIpPacketInfo::Other),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct HrpdTcpOptions {
    window_scale: Option<u8>,
    maximum_segment_size: Option<u16>,
    sack_blocks: usize,
}

fn parse_tcp_options(mut options: &[u8]) -> HrpdTcpOptions {
    let mut parsed = HrpdTcpOptions::default();
    while let Some((&kind, rest)) = options.split_first() {
        match kind {
            0 => break,
            1 => options = rest,
            _ => {
                let Some((&len, _)) = rest.split_first() else {
                    break;
                };
                let len = usize::from(len);
                if len < 2 || options.len() < len {
                    break;
                }
                match (kind, len) {
                    (2, 4) => {
                        parsed.maximum_segment_size =
                            Some(u16::from_be_bytes([options[2], options[3]]));
                    }
                    (3, 3) => parsed.window_scale = Some(options[2]),
                    (5, len) if len >= 10 && (len - 2) % 8 == 0 => {
                        parsed.sack_blocks += (len - 2) / 8;
                    }
                    _ => {}
                }
                options = &options[len..];
            }
        }
    }
    parsed
}

fn summarize_hrpd_ipv4_header(packet: &[u8]) -> String {
    if packet.len() < 20 {
        return format!("IP len={} (too short)", packet.len());
    }
    let version = packet[0] >> 4;
    if version != 4 {
        return format!("IP v{} len={}", version, packet.len());
    }
    let total_len = u16::from_be_bytes([packet[2], packet[3]]).max(packet.len() as u16);
    let protocol = packet[9];
    let src = format!(
        "{}.{}.{}.{}",
        packet[12], packet[13], packet[14], packet[15]
    );
    let dst = format!(
        "{}.{}.{}.{}",
        packet[16], packet[17], packet[18], packet[19]
    );
    format!("IPv4 {src} -> {dst} proto={protocol} len={total_len}")
}

fn format_tcp_flags(flags: u8) -> String {
    let mut names = Vec::new();
    if flags & 0x01 != 0 {
        names.push("FIN");
    }
    if flags & 0x02 != 0 {
        names.push("SYN");
    }
    if flags & 0x04 != 0 {
        names.push("RST");
    }
    if flags & 0x08 != 0 {
        names.push("PSH");
    }
    if flags & 0x10 != 0 {
        names.push("ACK");
    }
    if flags & 0x20 != 0 {
        names.push("URG");
    }
    if names.is_empty() {
        format!("0x{flags:02x}")
    } else {
        names.join("|")
    }
}

/// Compatibility wrapper for older call sites. This function now runs the
/// spec-shaped A10 byte-stream session; HRPD Default Packet RLP wrapping is an
/// AN/BTS air-side responsibility.
pub async fn run_hrpd_default_packet_session(
    session_id: String,
    service_option: u32,
    transport: Box<dyn IpTransport>,
    uplink_rx: mpsc::Receiver<Vec<u8>>,
    downlink_tx: mpsc::Sender<Vec<u8>>,
    allocator: Arc<dyn IpAllocator>,
    metadata: SessionMetadata,
    lifecycle_sink: Arc<dyn SessionLifecycleSink>,
) {
    let (_shutdown_tx, shutdown_rx) = oneshot::channel();
    run_hrpd_a10_byte_stream_session(
        session_id,
        service_option,
        transport,
        uplink_rx,
        downlink_tx,
        shutdown_rx,
        Arc::new(Mutex::new(SessionStatus::new(
            service_option,
            metadata.clone(),
        ))),
        allocator,
        metadata,
        lifecycle_sink,
        None,
        Duration::ZERO,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::net::Ipv4Addr;
    use std::sync::Mutex;

    use crate::ppp::ipcp::IpcpConfig;
    use crate::session_lifecycle::NullSink;

    #[derive(Default)]
    struct TestAllocator {
        allocated: Mutex<Vec<String>>,
        released: Mutex<Vec<String>>,
    }

    impl IpAllocator for TestAllocator {
        fn allocate(&self, session_id: &str) -> Option<IpcpConfig> {
            self.allocated.lock().unwrap().push(session_id.to_string());
            Some(IpcpConfig::default())
        }

        fn release(&self, session_id: &str) {
            self.released.lock().unwrap().push(session_id.to_string());
        }

        fn claim_peer_ip(
            &self,
            _session_id: &str,
            _peer_ip: Ipv4Addr,
        ) -> crate::ip_allocator::IpClaimResult {
            crate::ip_allocator::IpClaimResult::OutOfPool
        }
    }

    #[derive(Default)]
    struct TestTransport;

    impl IpTransport for TestTransport {
        fn setup(
            &mut self,
            _local_ip: Ipv4Addr,
            _peer_ip: Ipv4Addr,
            _to_mobile_tx: mpsc::Sender<Vec<u8>>,
        ) -> io::Result<String> {
            Ok("test-hrpd".to_string())
        }

        fn send_to_network(&self, _ip_packet: &[u8]) -> io::Result<()> {
            Ok(())
        }

        fn teardown(&mut self) {}
    }

    fn tcp_packet(
        direction: &str,
        flags: u8,
        sequence: u32,
        acknowledgement: u32,
        window: u16,
        window_scale: Option<u8>,
        payload_len: usize,
    ) -> HrpdIpPacketInfo {
        let (src_addr, dst_addr, src_port, dst_port) = if direction == "uplink" {
            ([10, 55, 0, 2], [192, 0, 2, 1], 50_000, 443)
        } else {
            ([192, 0, 2, 1], [10, 55, 0, 2], 443, 50_000)
        };
        HrpdIpPacketInfo::Tcp {
            src: src_addr.map(|octet| octet.to_string()).join("."),
            dst: dst_addr.map(|octet| octet.to_string()).join("."),
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            flags,
            sequence,
            acknowledgement,
            window,
            window_scale,
            maximum_segment_size: None,
            sack_blocks: 0,
            ip_len: 40 + payload_len,
            payload_len,
        }
    }

    fn with_tcp_options(
        mut packet: HrpdIpPacketInfo,
        maximum_segment_size: Option<u16>,
        sack_blocks: usize,
    ) -> HrpdIpPacketInfo {
        let HrpdIpPacketInfo::Tcp {
            maximum_segment_size: packet_mss,
            sack_blocks: packet_sack_blocks,
            ..
        } = &mut packet
        else {
            panic!("test packet must be TCP");
        };
        *packet_mss = maximum_segment_size;
        *packet_sack_blocks = sack_blocks;
        packet
    }

    #[test]
    fn tcp_window_scale_parser_walks_nops_and_other_options() {
        assert_eq!(
            parse_tcp_options(&[1, 2, 4, 0x05, 0xb4, 3, 3, 7, 0]).window_scale,
            Some(7)
        );
        assert_eq!(parse_tcp_options(&[2, 4, 0x05, 0xb4, 0]).window_scale, None);
    }

    #[test]
    fn tcp_option_parser_reports_mss_window_scale_and_sack_blocks() {
        let options = [
            2, 4, 0x05, 0x48, 3, 3, 7, 1, 5, 18, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4,
        ];
        let parsed = parse_tcp_options(&options);
        assert_eq!(parsed.maximum_segment_size, Some(1352));
        assert_eq!(parsed.window_scale, Some(7));
        assert_eq!(parsed.sack_blocks, 2);
    }

    #[test]
    fn tcp_diagnostics_tracks_ack_progress_window_and_retransmission() {
        let mut diagnostics = HrpdTcpDiagnostics::default();
        diagnostics.record(
            "uplink",
            &tcp_packet("uplink", 0x02, 1, 0, 1000, Some(7), 0),
        );
        diagnostics.record(
            "downlink",
            &tcp_packet("downlink", 0x10, 100, 2, 65_535, None, 1000),
        );
        diagnostics.record(
            "uplink",
            &tcp_packet("uplink", 0x10, 2, 1100, 1000, None, 0),
        );
        diagnostics.record(
            "downlink",
            &tcp_packet("downlink", 0x10, 1100, 2, 65_535, None, 1000),
        );
        diagnostics.record(
            "uplink",
            &tcp_packet("uplink", 0x10, 2, 2100, 1000, None, 0),
        );
        diagnostics.record(
            "downlink",
            &tcp_packet("downlink", 0x10, 1100, 2, 65_535, None, 1000),
        );

        assert_eq!(diagnostics.flows.len(), 1);
        assert_eq!(diagnostics.window.uplink_ack_packets, 2);
        assert_eq!(diagnostics.window.uplink_ack_only_packets, 2);
        assert_eq!(diagnostics.window.uplink_ack_advances, 1);
        assert_eq!(diagnostics.window.uplink_duplicate_acks, 0);
        assert_eq!(diagnostics.window.uplink_acked_bytes, 1000);
        assert_eq!(diagnostics.window.min_mobile_window_bytes, Some(128_000));
        assert_eq!(diagnostics.window.max_inflight_bytes, 1000);
        assert_eq!(diagnostics.window.downlink_retransmit_packets, 1);
        assert_eq!(diagnostics.window.downlink_retransmit_bytes, 1000);
    }

    #[test]
    fn tcp_diagnostics_classifies_ack_preceded_and_receive_window_stalls() {
        let start = Instant::now();
        let mut diagnostics = HrpdTcpDiagnostics::default();
        diagnostics.record_at(
            "uplink",
            &with_tcp_options(
                tcp_packet("uplink", 0x02, 1, 0, 1000, Some(2), 0),
                Some(1352),
                0,
            ),
            start,
        );
        diagnostics.record_at(
            "downlink",
            &tcp_packet("downlink", 0x12, 100, 2, 65_535, None, 0),
            start + Duration::from_millis(1),
        );
        diagnostics.record_at(
            "uplink",
            &tcp_packet("uplink", 0x10, 2, 101, 1000, None, 0),
            start + Duration::from_millis(2),
        );
        diagnostics.record_at(
            "downlink",
            &tcp_packet("downlink", 0x10, 101, 2, 65_535, None, 1000),
            start + Duration::from_millis(3),
        );
        diagnostics.record_at(
            "uplink",
            &tcp_packet("uplink", 0x10, 2, 1101, 1000, None, 0),
            start + Duration::from_millis(50),
        );
        let resume = diagnostics
            .record_at(
                "downlink",
                &tcp_packet("downlink", 0x10, 1101, 2, 65_535, None, 1000),
                start + Duration::from_millis(203),
            )
            .expect("downlink gap should be classified");
        assert_eq!(resume.classification, "ack_preceded_resume_non_rwnd");
        assert_eq!(resume.flight_after_previous_send, Some(1000));
        assert_eq!(resume.window_after_previous_send, Some(4000));
        assert_eq!(resume.headroom_after_previous_send, Some(3000));
        assert_eq!(resume.flight_before_resume, Some(0));
        assert_eq!(resume.headroom_before_resume, Some(4000));
        assert_eq!(resume.ack_advances_during_gap, 1);
        assert_eq!(resume.acked_bytes_during_gap, 1000);

        let mut rwnd_diagnostics = HrpdTcpDiagnostics::default();
        rwnd_diagnostics.record_at(
            "uplink",
            &with_tcp_options(
                tcp_packet("uplink", 0x02, 1, 0, 1000, Some(0), 0),
                Some(1000),
                0,
            ),
            start,
        );
        rwnd_diagnostics.record_at(
            "downlink",
            &tcp_packet("downlink", 0x12, 100, 2, 65_535, None, 0),
            start + Duration::from_millis(1),
        );
        rwnd_diagnostics.record_at(
            "uplink",
            &tcp_packet("uplink", 0x10, 2, 101, 1000, None, 0),
            start + Duration::from_millis(2),
        );
        rwnd_diagnostics.record_at(
            "downlink",
            &tcp_packet("downlink", 0x10, 101, 2, 65_535, None, 1000),
            start + Duration::from_millis(3),
        );
        rwnd_diagnostics.record_at(
            "uplink",
            &tcp_packet("uplink", 0x10, 2, 1101, 1000, None, 0),
            start + Duration::from_millis(50),
        );
        let resume = rwnd_diagnostics
            .record_at(
                "downlink",
                &tcp_packet("downlink", 0x10, 1101, 2, 65_535, None, 1000),
                start + Duration::from_millis(203),
            )
            .expect("receive-window stall should be classified");
        assert_eq!(resume.classification, "rwnd_limited");
        assert_eq!(resume.headroom_after_previous_send, Some(0));
    }

    #[test]
    fn tcp_sequence_comparison_handles_wraparound() {
        assert!(tcp_seq_after(0x0000_0010, 0xffff_fff0));
        assert!(!tcp_seq_after(0xffff_fff0, 0x0000_0010));
        assert!(tcp_seq_at_or_after(0x1234_5678, 0x1234_5678));
    }

    #[tokio::test]
    async fn hrpd_task_originates_first_a10_byte_stream_chunk() {
        let allocator = Arc::new(TestAllocator::default());
        let (uplink_tx, uplink_rx) = mpsc::channel(8);
        let (downlink_tx, mut downlink_rx) = mpsc::channel(8);
        let (_shutdown_tx, shutdown_rx) = oneshot::channel();

        let task_allocator = allocator.clone();
        let handle = tokio::spawn(async move {
            run_hrpd_a10_byte_stream_session(
                "uati-80058001".to_string(),
                u32::from(cdma_common::consts::SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
                Box::new(TestTransport),
                uplink_rx,
                downlink_tx,
                shutdown_rx,
                Arc::new(Mutex::new(SessionStatus::new(
                    u32::from(cdma_common::consts::SERVICE_OPTION_HIGH_RATE_PACKET_DATA),
                    SessionMetadata {
                        access_technology: "HRPD".to_string(),
                        mobile_address: "uati:80058001".to_string(),
                        subscriber_id: None,
                        phone_number: String::new(),
                        imsi: None,
                        esn: None,
                        meid: None,
                        hrpd_mn_id: None,
                        hrpd_mn_id_source: None,
                        subscriber_imsi: None,
                        traffic_walsh_code: 0x8005_8001,
                    },
                ))),
                task_allocator,
                SessionMetadata {
                    access_technology: "HRPD".to_string(),
                    mobile_address: "uati:80058001".to_string(),
                    subscriber_id: None,
                    phone_number: String::new(),
                    imsi: None,
                    esn: None,
                    meid: None,
                    hrpd_mn_id: None,
                    hrpd_mn_id_source: None,
                    subscriber_imsi: None,
                    traffic_walsh_code: 0x8005_8001,
                },
                Arc::new(NullSink),
                None,
                Duration::ZERO,
            )
            .await;
        });

        let downlink = tokio::time::timeout(Duration::from_secs(1), downlink_rx.recv())
            .await
            .expect("HRPD task should originate downlink A10 byte-stream PPP")
            .expect("downlink channel should stay open");

        assert!(downlink.starts_with(&[0x7e]));
        assert!(downlink.ends_with(&[0x7e]));
        drop(uplink_tx);
        handle.await.unwrap();
        assert_eq!(
            allocator.released.lock().unwrap().as_slice(),
            &["hrpd-mobile:uati:80058001"]
        );
    }
}
