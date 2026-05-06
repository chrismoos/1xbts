use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::engine::{
    PacketSession, PacketSessionTelemetry, PacketTraceEvent, SessionAction, SessionPhase,
    bytes_to_hex, format_tcp_flags, now_ms,
};
use crate::ip_allocator::IpAllocator;
use crate::ip_transport::IpTransport;
use crate::ppp::ipcp::IpcpConfig;

/// Status snapshot for a session, shared with the gRPC query handlers.
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    pub mobile_address: String,
    pub subscriber_id: String,
    pub phone_number: String,
    pub traffic_walsh_code: u32,
}

pub struct SessionStatus {
    pub phase: String,
    pub service_option: u32,
    pub peer_ip: String,
    pub our_ip: String,
    pub tun_device: String,
    pub uplink_frames: u64,
    pub downlink_frames: u64,
    pub uplink_bytes: u64,
    pub downlink_bytes: u64,
    pub created_at_ms: u64,
    pub last_phase_change_at_ms: u64,
    pub last_uplink_at_ms: u64,
    pub last_downlink_at_ms: u64,
    pub last_activity_at_ms: u64,
    pub last_uplink_rate_bps: u32,
    pub last_downlink_rate_bps: u32,
    pub mobile_address: String,
    pub subscriber_id: String,
    pub phone_number: String,
    pub traffic_walsh_code: u32,
    pub rlp_state: String,
    pub lcp_state: String,
    pub ipcp_state: String,
    pub capture_enabled: bool,
    pub last_rx_control: String,
    pub last_tx_control: String,
    pub last_rx_control_repeats: u64,
    pub last_tx_control_repeats: u64,
    pub recent_ppp_events: VecDeque<PacketTraceEvent>,
    pub capture_events: VecDeque<PacketTraceEvent>,
}

impl SessionStatus {
    pub fn new(service_option: u32, metadata: SessionMetadata) -> Self {
        let now = now_ms();
        Self {
            phase: "rlp_sync".into(),
            service_option,
            peer_ip: String::new(),
            our_ip: String::new(),
            tun_device: String::new(),
            uplink_frames: 0,
            downlink_frames: 0,
            uplink_bytes: 0,
            downlink_bytes: 0,
            created_at_ms: now,
            last_phase_change_at_ms: now,
            last_uplink_at_ms: 0,
            last_downlink_at_ms: 0,
            last_activity_at_ms: 0,
            last_uplink_rate_bps: 0,
            last_downlink_rate_bps: 0,
            mobile_address: metadata.mobile_address,
            subscriber_id: metadata.subscriber_id,
            phone_number: metadata.phone_number,
            traffic_walsh_code: metadata.traffic_walsh_code,
            rlp_state: "sync".into(),
            lcp_state: "closed".into(),
            ipcp_state: "closed".into(),
            capture_enabled: false,
            last_rx_control: String::new(),
            last_tx_control: String::new(),
            last_rx_control_repeats: 0,
            last_tx_control_repeats: 0,
            recent_ppp_events: VecDeque::new(),
            capture_events: VecDeque::new(),
        }
    }

    pub fn set_capture_enabled(&mut self, enabled: bool) {
        let now = now_ms();
        if enabled {
            self.capture_events.clear();
            self.capture_enabled = true;
            self.push_capture_event_unchecked(PacketTraceEvent {
                timestamp_ms: now,
                layer: "session".to_string(),
                direction: "internal".to_string(),
                summary: "Capture started".to_string(),
                detail: "IP frame capture enabled".to_string(),
                payload_hex: String::new(),
            });
        } else if self.capture_enabled {
            self.push_capture_event_unchecked(PacketTraceEvent {
                timestamp_ms: now,
                layer: "session".to_string(),
                direction: "internal".to_string(),
                summary: "Capture stopped".to_string(),
                detail: "IP frame capture disabled".to_string(),
                payload_hex: String::new(),
            });
            self.capture_enabled = false;
        }
    }

    pub fn push_capture_event(&mut self, event: PacketTraceEvent) {
        if self.capture_enabled {
            self.push_capture_event_unchecked(event);
        }
    }

    fn push_capture_event_unchecked(&mut self, event: PacketTraceEvent) {
        const MAX_CAPTURE_EVENTS: usize = 256;
        push_ring(&mut self.capture_events, event, MAX_CAPTURE_EVENTS);
    }

    pub fn sync_telemetry(&mut self, phase: SessionPhase, telemetry: PacketSessionTelemetry) {
        let now = now_ms();
        let phase_str = match phase {
            SessionPhase::RlpSync => "rlp_sync",
            SessionPhase::Lcp => "lcp",
            SessionPhase::Ipcp => "ipcp",
            SessionPhase::Active => "active",
            SessionPhase::Closed => "closed",
        };
        if self.phase != phase_str {
            self.phase = phase_str.to_string();
            self.last_phase_change_at_ms = now;
        }
        self.rlp_state = telemetry.rlp_state;
        self.lcp_state = telemetry.lcp_state;
        self.ipcp_state = telemetry.ipcp_state;
        self.last_rx_control = telemetry.last_rx_control;
        self.last_tx_control = telemetry.last_tx_control;
        self.last_rx_control_repeats = telemetry.last_rx_control_repeats;
        self.last_tx_control_repeats = telemetry.last_tx_control_repeats;
        self.last_uplink_rate_bps = telemetry.last_uplink_rate_bps;
        self.last_downlink_rate_bps = telemetry.last_downlink_rate_bps;
        self.recent_ppp_events = telemetry.recent_ppp_events.into_iter().collect();
    }
}

/// Proto-generated SessionFrame type alias for convenience.
pub use crate::proto::SessionFrame;

#[derive(Debug, Default)]
struct PacketPathStats {
    uplink_ip_packets: u64,
    uplink_ip_bytes: u64,
    downlink_ip_packets: u64,
    downlink_ip_bytes: u64,
    downlink_rlp_frames: u64,
    downlink_rlp_bits: u64,
    downlink_full_frames: u64,
    downlink_half_frames: u64,
    downlink_quarter_frames: u64,
    downlink_eighth_frames: u64,
    downlink_sch_frames: u64,
    downlink_sch_bits: u64,
    max_pending_uplinks: usize,
    pending_uplinks_at_last_report: usize,
}

impl PacketPathStats {
    fn record_downlink_frame(&mut self, rate_bps: u32, bits: u32) {
        self.downlink_rlp_frames = self.downlink_rlp_frames.saturating_add(1);
        self.downlink_rlp_bits = self.downlink_rlp_bits.saturating_add(bits as u64);
        match rate_bps {
            9600 => self.downlink_full_frames = self.downlink_full_frames.saturating_add(1),
            4800 => self.downlink_half_frames = self.downlink_half_frames.saturating_add(1),
            2700 | 2400 => {
                self.downlink_quarter_frames = self.downlink_quarter_frames.saturating_add(1)
            }
            1500 | 1200 => {
                self.downlink_eighth_frames = self.downlink_eighth_frames.saturating_add(1)
            }
            _ => {}
        }
    }

    fn record_downlink_sch_frame(&mut self, bits: u32) {
        self.downlink_sch_frames = self.downlink_sch_frames.saturating_add(1);
        self.downlink_sch_bits = self.downlink_sch_bits.saturating_add(bits as u64);
    }

    fn reset_window(&mut self) {
        let pending = self.pending_uplinks_at_last_report;
        *self = Self::default();
        self.pending_uplinks_at_last_report = pending;
    }
}

/// Run a single packet data session as an async task.
///
/// This owns the PacketSession engine and IP transport lifecycle.
/// The BSC communicates via the mpsc channels only.
pub async fn run_session(
    session_id: String,
    service_option: u32,
    mut transport: Box<dyn IpTransport>,
    mut uplink_rx: mpsc::Receiver<SessionFrame>,
    downlink_tx: mpsc::Sender<SessionFrame>,
    status: Arc<Mutex<SessionStatus>>,
    allocator: Arc<dyn IpAllocator>,
) {
    let ipcp_config = allocator.allocate(&session_id).unwrap_or_else(|| {
        log::warn!(
            "packet-service: IP pool exhausted for session {}, falling back to default",
            session_id
        );
        IpcpConfig::default()
    });
    log::info!(
        "packet-service: session {} allocated peer_ip={}",
        session_id,
        ipcp_config.peer_ip
    );
    let mut session = PacketSession::new(service_option, ipcp_config);
    session.set_log_context(session_id.clone());
    let mut transport_ready = false;
    let (to_mobile_tx, mut to_mobile_rx) = mpsc::channel::<Vec<u8>>(256);

    let mut tick_interval = tokio::time::interval(Duration::from_millis(20));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut echo_interval = tokio::time::interval(Duration::from_secs(30));
    echo_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut path_health_interval = tokio::time::interval(Duration::from_secs(5));
    path_health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut path_stats = PacketPathStats::default();
    let session_started = Instant::now();
    // Packet sessions are frame-synchronous: advance the RLP state machine only
    // on the 20 ms traffic-channel cadence.  The receiver pipeline may deliver
    // frames in bursts, so we queue them and dequeue one per tick to keep the
    // RLP timing correct (one frame period = one tick).
    let mut pending_uplinks: VecDeque<SessionFrame> = VecDeque::new();

    log::info!(
        "packet-service: session {} started (SO {})",
        session_id,
        service_option
    );

    loop {
        tokio::select! {
            // Uplink frame from BSC
            frame = uplink_rx.recv() => {
                let frame = match frame {
                    Some(f) => f,
                    None => {
                        log::info!("packet-service: session {} uplink channel closed", session_id);
                        break;
                    }
                };

                let bits_len = frame.bits.len() as u64;
                let frame_rate_bps = frame.rate_bps;
                pending_uplinks.push_back(frame);
                path_stats.max_pending_uplinks =
                    path_stats.max_pending_uplinks.max(pending_uplinks.len());

                // Update stats
                {
                    let mut s = status.lock().unwrap();
                    s.uplink_frames += 1;
                    s.uplink_bytes += bits_len;
                    s.last_uplink_at_ms = now_ms();
                    s.last_activity_at_ms = s.last_uplink_at_ms;
                    s.last_uplink_rate_bps = frame_rate_bps;
                }
            }

            // Periodic tick (drives RLP SYNC, idle frames, etc.)
            // Dequeue at most one uplink frame per tick to keep RLP timing
            // correct — each tick() = one 20ms frame period.
            _ = tick_interval.tick() => {
                let actions = if let Some(frame) = pending_uplinks.pop_front() {
                    session.tick(Some((&frame.bits, frame.rate_bps)))
                } else {
                    session.tick(None)
                };
                process_actions(
                    &session_id, &mut session, &mut transport, &mut transport_ready,
                    &to_mobile_tx, &downlink_tx, &status, &mut path_stats, actions,
                ).await;
            }

            _ = path_health_interval.tick() => {
                path_stats.pending_uplinks_at_last_report = pending_uplinks.len();
                log::debug!(
                    "packet-service: session {} path_health age_ms={} phase={:?} transport_ready={} uplink_ip={} uplink_ip_bytes={} downlink_ip={} downlink_ip_bytes={} downlink_rlp_frames={} downlink_rlp_bits={} rates={{9600:{},4800:{},2700:{},1200:{}}} downlink_sch_frames={} downlink_sch_bits={} pending_uplinks={} max_pending_uplinks={}",
                    session_id,
                    session_started.elapsed().as_millis(),
                    session.phase(),
                    transport_ready,
                    path_stats.uplink_ip_packets,
                    path_stats.uplink_ip_bytes,
                    path_stats.downlink_ip_packets,
                    path_stats.downlink_ip_bytes,
                    path_stats.downlink_rlp_frames,
                    path_stats.downlink_rlp_bits,
                    path_stats.downlink_full_frames,
                    path_stats.downlink_half_frames,
                    path_stats.downlink_quarter_frames,
                    path_stats.downlink_eighth_frames,
                    path_stats.downlink_sch_frames,
                    path_stats.downlink_sch_bits,
                    pending_uplinks.len(),
                    path_stats.max_pending_uplinks
                );
                path_stats.reset_window();
            }

            // LCP Echo keepalive (every 30s)
            _ = echo_interval.tick() => {
                session.maybe_send_echo();
                if session.echo_dead() {
                    log::warn!(
                        "packet-service: session {} LCP echo dead, closing",
                        session_id
                    );
                    break;
                }
            }

            // Network -> mobile: receive IP packets from transport
            Some(ip_data) = to_mobile_rx.recv(), if transport_ready => {
                path_stats.downlink_ip_packets = path_stats.downlink_ip_packets.saturating_add(1);
                path_stats.downlink_ip_bytes =
                    path_stats.downlink_ip_bytes.saturating_add(ip_data.len() as u64);
                record_ip_capture(&status, "downlink", &ip_data, "network -> mobile");
                log::debug!(
                    "packet-service: session {} downlink IP {}",
                    session_id,
                    summarize_ip_packet(&ip_data)
                );
                session.send_ip_packet(&ip_data);
                // Do not advance RLP here. Packet sessions are driven strictly
                // by the 20 ms traffic-channel cadence above; generating frames
                // from network arrival skews RLP timing and injects synthetic
                // blank uplink periods.
            }
        }
    }

    // Cleanup
    transport.teardown();
    allocator.release(&session_id);
    {
        let mut s = status.lock().unwrap();
        s.sync_telemetry(SessionPhase::Closed, session.telemetry());
    }
    log::info!("packet-service: session {} ended", session_id);
}

/// Process session actions: send downlink RLP frames, set up IP transport on first IP packet.
async fn process_actions(
    session_id: &str,
    session: &mut PacketSession,
    transport: &mut Box<dyn IpTransport>,
    transport_ready: &mut bool,
    to_mobile_tx: &mpsc::Sender<Vec<u8>>,
    downlink_tx: &mpsc::Sender<SessionFrame>,
    status: &Arc<Mutex<SessionStatus>>,
    path_stats: &mut PacketPathStats,
    actions: Vec<SessionAction>,
) {
    for action in actions {
        match action {
            SessionAction::SendFrame { bits, rate_bps } => {
                let num_bits = bits.len() as u32;

                let frame = SessionFrame {
                    session_id: session_id.to_string(),
                    bits,
                    num_bits,
                    rate_bps,
                };

                if downlink_tx.send(frame).await.is_err() {
                    log::warn!(
                        "packet-service: session {} downlink channel closed",
                        session_id
                    );
                    return;
                }
                path_stats.record_downlink_frame(rate_bps, num_bits);

                {
                    let mut s = status.lock().unwrap();
                    s.downlink_frames += 1;
                    s.downlink_bytes = s.downlink_bytes.wrapping_add(num_bits as u64);
                    s.last_downlink_at_ms = now_ms();
                    s.last_activity_at_ms = s.last_downlink_at_ms;
                    s.last_downlink_rate_bps = rate_bps;
                }
            }
            SessionAction::SendSchFrame { bits, rate_bps } => {
                // SCH frames are sent to the same downlink channel.
                // The BSC will route them to the SCH physical channel.
                let num_bits = bits.len() as u32;
                let frame = SessionFrame {
                    session_id: session_id.to_string(),
                    bits,
                    num_bits,
                    rate_bps,
                };
                if downlink_tx.send(frame).await.is_err() {
                    log::warn!(
                        "packet-service: session {} SCH downlink channel closed",
                        session_id
                    );
                    return;
                }
                path_stats.record_downlink_sch_frame(num_bits);
            }
            SessionAction::DeliverIpPacket(ip_data) => {
                path_stats.uplink_ip_packets = path_stats.uplink_ip_packets.saturating_add(1);
                path_stats.uplink_ip_bytes = path_stats
                    .uplink_ip_bytes
                    .saturating_add(ip_data.len() as u64);
                record_ip_capture(status, "uplink", &ip_data, "mobile -> network");
                // Lazily set up transport
                if !*transport_ready {
                    let local_ip = session.our_ip();
                    let peer_ip = session.peer_ip();
                    match transport.setup(local_ip, peer_ip, to_mobile_tx.clone()) {
                        Ok(name) => {
                            log::info!(
                                "packet-service: session {} transport {} ready ({}->{})",
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
                        }
                        Err(e) => {
                            log::warn!(
                                "packet-service: session {} transport setup failed: {}",
                                session_id,
                                e
                            );
                        }
                    }
                }

                // Forward IP packet to network
                if *transport_ready {
                    log::debug!(
                        "packet-service: session {} uplink IP {}",
                        session_id,
                        summarize_ip_packet(&ip_data)
                    );
                    if let Err(e) = transport.send_to_network(&ip_data) {
                        log::warn!("packet-service: send_to_network error: {}", e);
                    } else {
                        let mut s = status.lock().unwrap();
                        s.uplink_bytes += ip_data.len() as u64;
                    }
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
}

fn push_ring<T>(ring: &mut VecDeque<T>, item: T, max_len: usize) {
    if ring.len() >= max_len {
        ring.pop_front();
    }
    ring.push_back(item);
}

fn record_ip_capture(
    status: &Arc<Mutex<SessionStatus>>,
    direction: &str,
    packet: &[u8],
    detail_prefix: &str,
) {
    let event = PacketTraceEvent {
        timestamp_ms: now_ms(),
        layer: "ip".to_string(),
        direction: direction.to_string(),
        summary: summarize_ip_packet(packet),
        detail: format!("{} len={}", detail_prefix, packet.len()),
        payload_hex: bytes_to_hex(packet),
    };
    let mut s = status.lock().unwrap();
    s.push_capture_event(event);
}

fn summarize_ip_packet(packet: &[u8]) -> String {
    if packet.len() < 20 {
        return format!("IP len={} (too short)", packet.len());
    }
    let version = packet[0] >> 4;
    if version != 4 {
        return format!("IP v{} len={}", version, packet.len());
    }
    let total_len = u16::from(packet[2]) << 8 | u16::from(packet[3]);
    let protocol = packet[9];
    let src = format!(
        "{}.{}.{}.{}",
        packet[12], packet[13], packet[14], packet[15]
    );
    let dst = format!(
        "{}.{}.{}.{}",
        packet[16], packet[17], packet[18], packet[19]
    );
    let ihl_bytes = usize::from(packet[0] & 0x0f) * 4;
    if ihl_bytes < 20 || packet.len() < ihl_bytes {
        return format!(
            "IPv4 {} -> {} proto={} len={} bad_ihl={}",
            src,
            dst,
            protocol,
            total_len.max(packet.len() as u16),
            ihl_bytes
        );
    }
    if protocol == 6 {
        return summarize_tcp_packet(
            packet,
            ihl_bytes,
            &src,
            &dst,
            total_len.max(packet.len() as u16),
        );
    }
    if protocol == 17 {
        return summarize_udp_packet(
            packet,
            ihl_bytes,
            &src,
            &dst,
            total_len.max(packet.len() as u16),
        );
    }
    format!(
        "IPv4 {} -> {} proto={} len={}",
        src,
        dst,
        protocol,
        total_len.max(packet.len() as u16)
    )
}

fn summarize_udp_packet(
    packet: &[u8],
    ihl_bytes: usize,
    src: &str,
    dst: &str,
    total_len: u16,
) -> String {
    if packet.len() < ihl_bytes + 8 {
        return format!("IPv4 {} -> {} UDP truncated len={}", src, dst, total_len);
    }
    let src_port = u16::from_be_bytes([packet[ihl_bytes], packet[ihl_bytes + 1]]);
    let dst_port = u16::from_be_bytes([packet[ihl_bytes + 2], packet[ihl_bytes + 3]]);
    let udp_len = u16::from_be_bytes([packet[ihl_bytes + 4], packet[ihl_bytes + 5]]);
    format!(
        "IPv4 {}:{} -> {}:{} UDP len={}",
        src, src_port, dst, dst_port, udp_len
    )
}

fn summarize_tcp_packet(
    packet: &[u8],
    ihl_bytes: usize,
    src: &str,
    dst: &str,
    total_len: u16,
) -> String {
    if packet.len() < ihl_bytes + 20 {
        return format!("IPv4 {} -> {} TCP truncated len={}", src, dst, total_len);
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
    let payload_len = usize::from(total_len).saturating_sub(ihl_bytes.saturating_add(data_offset));
    let flags = packet[ihl_bytes + 13];
    format!(
        "IPv4 {}:{} -> {}:{} TCP flags={} seq={} ack={} payload={}",
        src,
        src_port,
        dst,
        dst_port,
        format_tcp_flags(flags),
        seq,
        ack,
        payload_len
    )
}
