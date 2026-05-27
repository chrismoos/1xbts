use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::engine::{
    PacketSession, PacketSessionTelemetry, PacketTraceEvent, PppSessionState, SessionAction,
    SessionPhase, bytes_to_hex, format_tcp_flags, now_ms,
};
use crate::ip_allocator::IpAllocator;
use crate::ip_transport::IpTransport;
use crate::ppp::ipcp::IpcpConfig;

/// Status snapshot for a session, shared with the gRPC query handlers.
///
/// `subscriber_id` is `None` for unprovisioned/roaming mobiles. The
/// `Option` flavor must be propagated through to the event bus so it
/// can do forward enrichment from `imsi`/`esn` instead of resolving a
/// bogus subscriber. Callers at the gRPC boundary should map empty
/// strings to `None`.
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    pub mobile_address: String,
    pub subscriber_id: Option<String>,
    pub phone_number: String,
    /// IMSI of the handset, if known at session-open time.
    pub imsi: Option<String>,
    /// ESN of the handset, if known at session-open time.
    pub esn: Option<u32>,
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
    pub lcp_configure_restarts: u32,
    pub ipcp_configure_restarts: u32,
    pub ipcp_omitted_peer_ip_naks: u32,
    pub capture_enabled: bool,
    pub last_rx_control: String,
    pub last_tx_control: String,
    pub last_rx_control_repeats: u64,
    pub last_tx_control_repeats: u64,
    pub recent_ppp_events: VecDeque<PacketTraceEvent>,
    pub capture_events: VecDeque<PacketTraceEvent>,
}

#[derive(Debug, Clone)]
pub struct PppSessionCacheHit {
    pub state: PppSessionState,
    pub allocation_key: String,
    pub peer_ip: std::net::Ipv4Addr,
    pub idle_for: Duration,
}

#[derive(Debug, Clone)]
pub struct PppSessionCacheExpired {
    pub identity_key: String,
    pub allocation_key: String,
    pub peer_ip: std::net::Ipv4Addr,
    pub idle_for: Duration,
}

#[derive(Debug, Clone)]
pub enum PppSessionCacheLookup {
    Hit(PppSessionCacheHit),
    Expired(PppSessionCacheExpired),
    Miss,
}

#[derive(Debug, Clone)]
struct CachedPppSession {
    state: PppSessionState,
    allocation_key: String,
    last_activity_at: Instant,
}

#[derive(Debug, Default)]
pub struct PppSessionStore {
    sessions: Mutex<HashMap<String, CachedPppSession>>,
}

impl PppSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, identity_key: &str, timeout: Duration) -> PppSessionCacheLookup {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(cached) = sessions.get(identity_key) else {
            return PppSessionCacheLookup::Miss;
        };
        let idle_for = cached.last_activity_at.elapsed();
        if idle_for >= timeout {
            let expired = sessions
                .remove(identity_key)
                .expect("cached PPP session disappeared while locked");
            return PppSessionCacheLookup::Expired(PppSessionCacheExpired {
                identity_key: identity_key.to_string(),
                allocation_key: expired.allocation_key,
                peer_ip: expired.state.ipcp.config.peer_ip,
                idle_for,
            });
        }
        PppSessionCacheLookup::Hit(PppSessionCacheHit {
            state: cached.state.clone(),
            allocation_key: cached.allocation_key.clone(),
            peer_ip: cached.state.ipcp.config.peer_ip,
            idle_for,
        })
    }

    pub fn store(
        &self,
        identity_key: String,
        allocation_key: String,
        state: PppSessionState,
        last_activity_at: Instant,
    ) {
        self.sessions.lock().unwrap().insert(
            identity_key,
            CachedPppSession {
                state,
                allocation_key,
                last_activity_at,
            },
        );
    }

    pub fn reap_expired(&self, timeout: Duration) -> Vec<PppSessionCacheExpired> {
        let mut sessions = self.sessions.lock().unwrap();
        let expired_keys = sessions
            .iter()
            .filter(|(_, cached)| cached.last_activity_at.elapsed() >= timeout)
            .map(|(identity_key, _)| identity_key.clone())
            .collect::<Vec<_>>();

        expired_keys
            .into_iter()
            .filter_map(|identity_key| {
                let expired = sessions.remove(&identity_key)?;
                Some(PppSessionCacheExpired {
                    identity_key,
                    allocation_key: expired.allocation_key,
                    peer_ip: expired.state.ipcp.config.peer_ip,
                    idle_for: expired.last_activity_at.elapsed(),
                })
            })
            .collect()
    }
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
            // SessionStatus is a display-side snapshot; an empty string
            // means "unknown subscriber" here. The Option flavor on
            // SessionMetadata is what flows to the event bus where None
            // vs Some matters.
            subscriber_id: metadata.subscriber_id.unwrap_or_default(),
            phone_number: metadata.phone_number,
            traffic_walsh_code: metadata.traffic_walsh_code,
            rlp_state: "sync".into(),
            lcp_state: "closed".into(),
            ipcp_state: "closed".into(),
            lcp_configure_restarts: 0,
            ipcp_configure_restarts: 0,
            ipcp_omitted_peer_ip_naks: 0,
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
            SessionPhase::Mip4Pending => "mip4_pending",
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
        self.lcp_configure_restarts = telemetry.lcp_configure_restarts;
        self.ipcp_configure_restarts = telemetry.ipcp_configure_restarts;
        self.ipcp_omitted_peer_ip_naks = telemetry.ipcp_omitted_peer_ip_naks;
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

#[derive(Debug, Default)]
struct TcpLogState {
    logged_packets: u64,
    uplink_syn: u64,
    downlink_syn_ack: u64,
    uplink_ack_after_syn_ack: u64,
    uplink_payload: u64,
    downlink_payload: u64,
    pending_syns: HashMap<TcpFlowKey, u32>,
    pending_syn_acks: HashMap<TcpFlowKey, TcpSynAckState>,
    handshake_logged: bool,
    data_exchange_logged: bool,
    window_uplink_syn: u64,
    window_uplink_syn_retx: u64,
    window_downlink_syn_ack: u64,
    window_downlink_syn_ack_retx: u64,
    window_uplink_payload_packets: u64,
    window_downlink_payload_packets: u64,
    window_uplink_payload_bytes: u64,
    window_downlink_payload_bytes: u64,
    window_downlink_payload_retx_packets: u64,
    window_downlink_payload_retx_bytes: u64,
    window_uplink_payload_retx_packets: u64,
    window_uplink_payload_retx_bytes: u64,
    window_downlink_acked_bytes: u64,
    window_uplink_acked_bytes: u64,
    flow_progress: HashMap<TcpFlowKey, TcpFlowProgress>,
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

impl TcpLogState {
    fn reset_window(&mut self) {
        self.window_uplink_syn = 0;
        self.window_uplink_syn_retx = 0;
        self.window_downlink_syn_ack = 0;
        self.window_downlink_syn_ack_retx = 0;
        self.window_uplink_payload_packets = 0;
        self.window_downlink_payload_packets = 0;
        self.window_uplink_payload_bytes = 0;
        self.window_downlink_payload_bytes = 0;
        self.window_downlink_payload_retx_packets = 0;
        self.window_downlink_payload_retx_bytes = 0;
        self.window_uplink_payload_retx_packets = 0;
        self.window_uplink_payload_retx_bytes = 0;
        self.window_downlink_acked_bytes = 0;
        self.window_uplink_acked_bytes = 0;
    }
}

/// Run a single packet data session as an async task.
///
/// This owns the PacketSession engine and IP transport lifecycle.
/// The BSC communicates via the mpsc channels only.
/// Out-of-band session-control commands sent from BSC to the session task.
#[derive(Debug, Clone)]
pub enum SessionControl {
    SetSchActive { active: bool, rate_bps: u32 },
}

pub async fn run_session(
    session_id: String,
    service_option: u32,
    mut transport: Box<dyn IpTransport>,
    mut uplink_rx: mpsc::Receiver<SessionFrame>,
    downlink_tx: mpsc::Sender<SessionFrame>,
    status: Arc<Mutex<SessionStatus>>,
    allocator: Arc<dyn IpAllocator>,
    mut control_rx: mpsc::Receiver<SessionControl>,
    metadata: SessionMetadata,
    lifecycle_sink: Arc<dyn crate::session_lifecycle::SessionLifecycleSink>,
    ppp_session_store: Option<Arc<PppSessionStore>>,
    ppp_session_timeout: Duration,
) {
    let ppp_identity_key = ppp_identity_key(&metadata);
    let allocation_key = session_allocation_key(&session_id, &status, &metadata);
    let mut ppp_resume_state = None;
    if let (Some(store), Some(identity_key)) = (&ppp_session_store, &ppp_identity_key) {
        match store.lookup(identity_key, ppp_session_timeout) {
            PppSessionCacheLookup::Hit(hit) => {
                log::info!(
                    "packet-service: session {} walsh={} PPP cache hit identity={} peer_ip={} idle_secs={} allocation_key={}",
                    session_id,
                    metadata.traffic_walsh_code,
                    identity_key,
                    hit.peer_ip,
                    hit.idle_for.as_secs(),
                    hit.allocation_key
                );
                ppp_resume_state = Some(hit.state);
            }
            PppSessionCacheLookup::Expired(expired) => {
                log::info!(
                    "packet-service: session {} walsh={} PPP cache expired identity={} peer_ip={} idle_secs={} allocation_key={}",
                    session_id,
                    metadata.traffic_walsh_code,
                    identity_key,
                    expired.peer_ip,
                    expired.idle_for.as_secs(),
                    expired.allocation_key
                );
                allocator.release(&expired.allocation_key);
            }
            PppSessionCacheLookup::Miss => {
                log::info!(
                    "packet-service: session {} walsh={} PPP cache miss identity={}",
                    session_id,
                    metadata.traffic_walsh_code,
                    identity_key
                );
            }
        }
    } else {
        log::info!(
            "packet-service: session {} walsh={} PPP cache unavailable identity={}",
            session_id,
            metadata.traffic_walsh_code,
            ppp_identity_key.as_deref().unwrap_or("unknown")
        );
    }
    let allocated = allocator.allocate(&allocation_key);
    let allocator_failed = allocated.is_none();
    let mut ipcp_config = allocated.unwrap_or_else(|| {
        log::warn!(
            "packet-service: IP pool exhausted for session {} key {}, falling back to default",
            session_id,
            allocation_key
        );
        IpcpConfig::default()
    });
    if let Some(resume_state) = &ppp_resume_state {
        let cached_config = resume_state.ipcp.config.clone();
        if !allocator_failed && cached_config.peer_ip == ipcp_config.peer_ip {
            ipcp_config = cached_config;
        } else {
            log::warn!(
                "packet-service: session {} walsh={} PPP cache discarded identity={} cached_peer_ip={} allocated_peer_ip={} allocator_failed={}",
                session_id,
                metadata.traffic_walsh_code,
                ppp_identity_key.as_deref().unwrap_or("unknown"),
                cached_config.peer_ip,
                ipcp_config.peer_ip,
                allocator_failed
            );
            ppp_resume_state = None;
        }
    }
    log::info!(
        "packet-service: session {} walsh={} allocated peer_ip={} key={} ppp_resume={}",
        session_id,
        metadata.traffic_walsh_code,
        ipcp_config.peer_ip,
        allocation_key,
        ppp_resume_state.is_some()
    );

    let bind_peer_ip = ipcp_config.peer_ip;
    let bind_our_ip = ipcp_config.our_ip;
    if !allocator_failed {
        lifecycle_sink.on_bound(crate::session_lifecycle::SessionBoundInfo {
            session_id: session_id.clone(),
            service_option,
            subscriber_id: metadata.subscriber_id.clone(),
            imsi: metadata.imsi.clone(),
            esn: metadata.esn,
            peer_ip: bind_peer_ip,
            our_ip: bind_our_ip,
        });
    }
    let mut session =
        PacketSession::new_with_ppp_resume(service_option, ipcp_config, ppp_resume_state);
    session.set_log_context(format!(
        "session={} walsh={}",
        session_id, metadata.traffic_walsh_code
    ));
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
    let mut first_sch_payload_logged = false;
    let mut first_uplink_ip_logged = false;
    let mut first_downlink_ip_logged = false;
    let mut tcp_log_state = TcpLogState::default();
    let mut last_ppp_activity_at = Instant::now();

    log::info!(
        "packet-service: session {} walsh={} started (SO {})",
        session_id,
        metadata.traffic_walsh_code,
        service_option
    );

    loop {
        tokio::select! {
            // Out-of-band control commands from BSC.
            ctl = control_rx.recv() => {
                match ctl {
                    Some(SessionControl::SetSchActive { active, rate_bps }) => {
                        log::info!(
                            "packet-service: session {} SetSchActive({}, rate={})",
                            session_id, active, rate_bps
                        );
                        session.set_sch_active_with_rate(active, rate_bps);
                    }
                    None => {
                        // Sender dropped. The session keeps running on the
                        // bearer channels; only the control surface is gone.
                        log::debug!(
                            "packet-service: session {} control channel closed",
                            session_id
                        );
                    }
                }
            }

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
                let close_session = process_actions(
                    &session_id, &mut session, &mut transport, &mut transport_ready,
                    &to_mobile_tx, &downlink_tx, &status, &allocator, &allocation_key, &mut path_stats,
                    &mut first_sch_payload_logged, &mut first_uplink_ip_logged,
                    &mut tcp_log_state, actions,
                ).await;
                if session.take_ppp_activity() {
                    last_ppp_activity_at = Instant::now();
                }
                if close_session {
                    break;
                }
            }

            _ = path_health_interval.tick() => {
                path_stats.pending_uplinks_at_last_report = pending_uplinks.len();
                const PATH_HEALTH_WINDOW_SECS: f64 = 5.0;
                let ul_ip_kbps =
                    path_stats.uplink_ip_bytes as f64 * 8.0 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                let dl_ip_kbps =
                    path_stats.downlink_ip_bytes as f64 * 8.0 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                let fch_air_kbps =
                    path_stats.downlink_rlp_bits as f64 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                let sch_air_kbps =
                    path_stats.downlink_sch_bits as f64 / PATH_HEALTH_WINDOW_SECS / 1000.0;
                let telemetry = session.telemetry();
                log::debug!(
                    "packet-service: session {} path_health age_ms={} phase={:?} lcp={} ipcp={} ppp_restarts={{lcp:{} ipcp:{} ipcp_omitted_ip_naks:{}}} transport_ready={} uplink_ip={} uplink_ip_bytes={} ul_ip_kbps={:.1} downlink_ip={} downlink_ip_bytes={} dl_ip_kbps={:.1} downlink_rlp_frames={} downlink_rlp_bits={} fch_air_kbps={:.1} rates={{9600:{},4800:{},2700:{},1200:{}}} downlink_sch_frames={} downlink_sch_bits={} sch_air_kbps={:.1} pending_uplinks={} max_pending_uplinks={} tcp={{ul_syn:{} ul_syn_retx:{} dl_syn_ack:{} dl_syn_ack_retx:{} ul_payload_pkts:{} ul_payload_bytes:{} ul_retx_pkts:{} ul_retx_bytes:{} ul_acked_bytes:{} dl_payload_pkts:{} dl_payload_bytes:{} dl_retx_pkts:{} dl_retx_bytes:{} dl_acked_bytes:{}}}",
                    session_id,
                    session_started.elapsed().as_millis(),
                    session.phase(),
                    telemetry.lcp_state,
                    telemetry.ipcp_state,
                    telemetry.lcp_configure_restarts,
                    telemetry.ipcp_configure_restarts,
                    telemetry.ipcp_omitted_peer_ip_naks,
                    transport_ready,
                    path_stats.uplink_ip_packets,
                    path_stats.uplink_ip_bytes,
                    ul_ip_kbps,
                    path_stats.downlink_ip_packets,
                    path_stats.downlink_ip_bytes,
                    dl_ip_kbps,
                    path_stats.downlink_rlp_frames,
                    path_stats.downlink_rlp_bits,
                    fch_air_kbps,
                    path_stats.downlink_full_frames,
                    path_stats.downlink_half_frames,
                    path_stats.downlink_quarter_frames,
                    path_stats.downlink_eighth_frames,
                    path_stats.downlink_sch_frames,
                    path_stats.downlink_sch_bits,
                    sch_air_kbps,
                    pending_uplinks.len(),
                    path_stats.max_pending_uplinks,
                    tcp_log_state.window_uplink_syn,
                    tcp_log_state.window_uplink_syn_retx,
                    tcp_log_state.window_downlink_syn_ack,
                    tcp_log_state.window_downlink_syn_ack_retx,
                    tcp_log_state.window_uplink_payload_packets,
                    tcp_log_state.window_uplink_payload_bytes,
                    tcp_log_state.window_uplink_payload_retx_packets,
                    tcp_log_state.window_uplink_payload_retx_bytes,
                    tcp_log_state.window_uplink_acked_bytes,
                    tcp_log_state.window_downlink_payload_packets,
                    tcp_log_state.window_downlink_payload_bytes,
                    tcp_log_state.window_downlink_payload_retx_packets,
                    tcp_log_state.window_downlink_payload_retx_bytes,
                    tcp_log_state.window_downlink_acked_bytes
                );
                path_stats.reset_window();
                tcp_log_state.reset_window();
            }

            // LCP Echo keepalive (every 30s)
            _ = echo_interval.tick() => {
                session.maybe_send_echo();
                if session.take_ppp_activity() {
                    last_ppp_activity_at = Instant::now();
                }
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
                if !first_downlink_ip_logged {
                    log::info!(
                        "packet-service: session {} first downlink IP {}",
                        session_id,
                        summarize_ip_packet(&ip_data)
                    );
                    first_downlink_ip_logged = true;
                    session.enable_sch_data_path();
                }
                log_tcp_packet(&session_id, "downlink", &ip_data, &mut tcp_log_state);
                log::debug!(
                    "packet-service: session {} downlink IP {}",
                    session_id,
                    summarize_ip_packet(&ip_data)
                );
                session.send_ip_packet(&ip_data);
                if session.take_ppp_activity() {
                    last_ppp_activity_at = Instant::now();
                }
                // Do not advance RLP here. Packet sessions are driven strictly
                // by the 20 ms traffic-channel cadence above; generating frames
                // from network arrival skews RLP timing and injects synthetic
                // blank uplink periods.
            }
        }
    }

    let ppp_snapshot = session.snapshot_ppp_state();
    let ppp_cache_kept = if let (Some(store), Some(identity_key), Some(snapshot)) =
        (&ppp_session_store, ppp_identity_key.as_ref(), ppp_snapshot)
    {
        let peer_ip = snapshot.ipcp.config.peer_ip;
        store.store(
            identity_key.clone(),
            allocation_key.clone(),
            snapshot,
            last_ppp_activity_at,
        );
        log::info!(
            "packet-service: session {} walsh={} stored open PPP session identity={} peer_ip={} allocation_key={} idle_secs={}",
            session_id,
            metadata.traffic_walsh_code,
            identity_key,
            peer_ip,
            allocation_key,
            last_ppp_activity_at.elapsed().as_secs()
        );
        true
    } else {
        false
    };

    // Cleanup
    transport.teardown();
    if ppp_cache_kept {
        log::info!(
            "packet-service: session {} walsh={} keeping IP allocation for cached PPP session key={}",
            session_id,
            metadata.traffic_walsh_code,
            allocation_key
        );
    } else {
        allocator.release(&allocation_key);
    }
    {
        let mut s = status.lock().unwrap();
        s.sync_telemetry(SessionPhase::Closed, session.telemetry());
    }
    // Only emit unbound if we actually emitted bound. AllocatorFailure
    // means the session never became observable to the bus.
    if !allocator_failed {
        lifecycle_sink.on_unbound(crate::session_lifecycle::SessionUnboundInfo {
            session_id: session_id.clone(),
            subscriber_id: metadata.subscriber_id.clone(),
            imsi: metadata.imsi.clone(),
            esn: metadata.esn,
            peer_ip: bind_peer_ip,
            reason: crate::session_lifecycle::UnbindReason::UplinkClosed,
        });
    }
    log::info!("packet-service: session {} ended", session_id);
}

fn session_allocation_key(
    session_id: &str,
    status: &Arc<Mutex<SessionStatus>>,
    metadata: &SessionMetadata,
) -> String {
    if let Some(key) = ppp_identity_key(metadata) {
        return format!("device:{}", key);
    }
    let s = status.lock().unwrap();
    if !s.mobile_address.is_empty() {
        return format!("mobile:{}", s.mobile_address);
    }
    if !s.subscriber_id.is_empty() {
        return format!("subscriber:{}", s.subscriber_id);
    }
    format!("session:{}", session_id)
}

pub fn ppp_identity_key(metadata: &SessionMetadata) -> Option<String> {
    match (metadata.imsi.as_deref(), metadata.esn) {
        (Some(imsi), Some(esn)) => Some(format!("imsi:{}:esn:{:08x}", imsi, esn)),
        (Some(imsi), None) => Some(format!("imsi:{}", imsi)),
        (None, Some(esn)) => Some(format!("esn:{:08x}", esn)),
        (None, None) => None,
    }
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
    _allocator: &Arc<dyn IpAllocator>,
    _allocation_key: &str,
    path_stats: &mut PacketPathStats,
    first_sch_payload_logged: &mut bool,
    first_uplink_ip_logged: &mut bool,
    tcp_log_state: &mut TcpLogState,
    actions: Vec<SessionAction>,
) -> bool {
    for action in actions {
        match action {
            SessionAction::CloseSession { reason } => {
                log::warn!(
                    "packet-service: session {} closing from engine: {}",
                    session_id,
                    reason
                );
                session.close();
                return true;
            }
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
                    return true;
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
                if !*first_sch_payload_logged {
                    log::info!(
                        "packet-service: session {} first SCH payload frame rate={} bits={}",
                        session_id,
                        rate_bps,
                        num_bits
                    );
                    *first_sch_payload_logged = true;
                }
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
                    return true;
                }
                path_stats.record_downlink_sch_frame(num_bits);
            }
            SessionAction::DeliverIpPacket(ip_data) => {
                path_stats.uplink_ip_packets = path_stats.uplink_ip_packets.saturating_add(1);
                path_stats.uplink_ip_bytes = path_stats
                    .uplink_ip_bytes
                    .saturating_add(ip_data.len() as u64);
                record_ip_capture(status, "uplink", &ip_data, "mobile -> network");
                if !*first_uplink_ip_logged {
                    log::info!(
                        "packet-service: session {} first uplink IP {}",
                        session_id,
                        summarize_ip_packet(&ip_data)
                    );
                    *first_uplink_ip_logged = true;
                }
                log_tcp_packet(session_id, "uplink", &ip_data, tcp_log_state);
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
    false
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

#[derive(Debug)]
struct TcpPacketInfo {
    src: String,
    dst: String,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    ip_total_len: usize,
    tcp_header_len: usize,
    payload_len: usize,
    options: TcpOptionsInfo,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TcpOptionsInfo {
    mss: Option<u16>,
    window_scale: Option<u8>,
    sack_permitted: bool,
    timestamp: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TcpFlowKey {
    client: String,
    server: String,
    client_port: u16,
    server_port: u16,
}

#[derive(Debug, Clone, Copy)]
struct TcpSynAckState {
    client_next_seq: u32,
    server_next_seq: u32,
}

#[derive(Debug, Default)]
struct TcpFlowProgress {
    highest_downlink_end: Option<u32>,
    highest_uplink_end: Option<u32>,
    highest_downlink_ack: Option<u32>,
    highest_uplink_ack: Option<u32>,
}

fn log_tcp_packet(session_id: &str, direction: &str, packet: &[u8], state: &mut TcpLogState) {
    let Some(tcp) = parse_tcp_packet(packet) else {
        return;
    };
    let syn = tcp.flags & 0x02 != 0;
    let ack_flag = tcp.flags & 0x10 != 0;
    match direction {
        "uplink" => {
            let key = tcp.uplink_flow_key();
            let progress = state.flow_progress.entry(key.clone()).or_default();
            if ack_flag {
                let acked = tcp_ack_delta(&mut progress.highest_downlink_ack, tcp.ack);
                state.window_downlink_acked_bytes =
                    state.window_downlink_acked_bytes.saturating_add(acked);
            }
            if syn && !ack_flag {
                state.uplink_syn = state.uplink_syn.saturating_add(1);
                state.window_uplink_syn = state.window_uplink_syn.saturating_add(1);
                let client_next_seq = tcp.seq.wrapping_add(1);
                if state.pending_syns.get(&key) == Some(&client_next_seq) {
                    state.window_uplink_syn_retx = state.window_uplink_syn_retx.saturating_add(1);
                }
                state.pending_syns.insert(key, client_next_seq);
            } else if ack_flag && !syn && state.downlink_syn_ack > 0 {
                let key = tcp.uplink_flow_key();
                if let Some(syn_ack) = state.pending_syn_acks.remove(&key)
                    && tcp.seq == syn_ack.client_next_seq
                    && tcp.ack == syn_ack.server_next_seq
                {
                    state.uplink_ack_after_syn_ack =
                        state.uplink_ack_after_syn_ack.saturating_add(1);
                    if !state.handshake_logged {
                        log::info!(
                            "packet-service: session {} TCP handshake observed {}:{} -> {}:{} client_next_seq={} server_next_seq={}",
                            session_id,
                            key.client,
                            key.client_port,
                            key.server,
                            key.server_port,
                            syn_ack.client_next_seq,
                            syn_ack.server_next_seq
                        );
                        state.handshake_logged = true;
                    }
                }
            }
            if tcp.payload_len > 0 {
                let end_seq = tcp.seq.wrapping_add(tcp.payload_len as u32);
                if tcp_range_retransmitted(&mut progress.highest_uplink_end, end_seq) {
                    state.window_uplink_payload_retx_packets =
                        state.window_uplink_payload_retx_packets.saturating_add(1);
                    state.window_uplink_payload_retx_bytes = state
                        .window_uplink_payload_retx_bytes
                        .saturating_add(tcp.payload_len as u64);
                }
                state.uplink_payload = state.uplink_payload.saturating_add(1);
                state.window_uplink_payload_packets =
                    state.window_uplink_payload_packets.saturating_add(1);
                state.window_uplink_payload_bytes = state
                    .window_uplink_payload_bytes
                    .saturating_add(tcp.payload_len as u64);
            }
        }
        "downlink" => {
            let key = tcp.downlink_flow_key();
            let progress = state.flow_progress.entry(key.clone()).or_default();
            if ack_flag {
                let acked = tcp_ack_delta(&mut progress.highest_uplink_ack, tcp.ack);
                state.window_uplink_acked_bytes =
                    state.window_uplink_acked_bytes.saturating_add(acked);
            }
            if syn && ack_flag {
                if let Some(client_next_seq) = state.pending_syns.remove(&key)
                    && tcp.ack == client_next_seq
                {
                    state.downlink_syn_ack = state.downlink_syn_ack.saturating_add(1);
                    state.window_downlink_syn_ack = state.window_downlink_syn_ack.saturating_add(1);
                    state.pending_syn_acks.insert(
                        key,
                        TcpSynAckState {
                            client_next_seq,
                            server_next_seq: tcp.seq.wrapping_add(1),
                        },
                    );
                } else if let Some(syn_ack) = state.pending_syn_acks.get(&key)
                    && syn_ack.client_next_seq == tcp.ack
                    && syn_ack.server_next_seq == tcp.seq.wrapping_add(1)
                {
                    state.window_downlink_syn_ack_retx =
                        state.window_downlink_syn_ack_retx.saturating_add(1);
                }
            }
            if tcp.payload_len > 0 {
                let end_seq = tcp.seq.wrapping_add(tcp.payload_len as u32);
                if tcp_range_retransmitted(&mut progress.highest_downlink_end, end_seq) {
                    state.window_downlink_payload_retx_packets =
                        state.window_downlink_payload_retx_packets.saturating_add(1);
                    state.window_downlink_payload_retx_bytes = state
                        .window_downlink_payload_retx_bytes
                        .saturating_add(tcp.payload_len as u64);
                }
                state.downlink_payload = state.downlink_payload.saturating_add(1);
                state.window_downlink_payload_packets =
                    state.window_downlink_payload_packets.saturating_add(1);
                state.window_downlink_payload_bytes = state
                    .window_downlink_payload_bytes
                    .saturating_add(tcp.payload_len as u64);
            }
        }
        _ => {}
    }

    if state.logged_packets < 40 || tcp.payload_len > 0 || syn {
        let tcp_options = if syn {
            let summary = tcp.options.summary();
            if summary.is_empty() {
                String::new()
            } else {
                format!(" opts=[{}]", summary)
            }
        } else {
            String::new()
        };
        log::debug!(
            "packet-service: session {} TCP {} {}:{} -> {}:{} flags={} seq={} ack={} win={} ip_len={} tcp_hlen={} payload={}{}",
            session_id,
            direction,
            tcp.src,
            tcp.src_port,
            tcp.dst,
            tcp.dst_port,
            format_tcp_flags(tcp.flags),
            tcp.seq,
            tcp.ack,
            tcp.window,
            tcp.ip_total_len,
            tcp.tcp_header_len,
            tcp.payload_len,
            tcp_options
        );
        state.logged_packets = state.logged_packets.saturating_add(1);
    }

    if !state.data_exchange_logged && state.uplink_payload > 0 && state.downlink_payload > 0 {
        log::info!(
            "packet-service: session {} TCP data exchanged uplink_payload_packets={} downlink_payload_packets={}",
            session_id,
            state.uplink_payload,
            state.downlink_payload
        );
        state.data_exchange_logged = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile_ip::{MobileIpConfig, MobileIpSession};
    use crate::ppp::ipcp::IpcpOpenState;
    use crate::ppp::lcp::{LcpOpenState, NegotiatedOptions};
    use crate::ppp::vj::{VjCompressionOptions, VjState};
    use std::net::Ipv4Addr;

    fn ppp_state(peer_ip: Ipv4Addr) -> PppSessionState {
        PppSessionState {
            lcp: LcpOpenState {
                negotiated: NegotiatedOptions::default(),
                last_acked_peer_request_data: Vec::new(),
            },
            ipcp: IpcpOpenState {
                config: IpcpConfig {
                    our_ip: Ipv4Addr::new(10, 55, 0, 1),
                    peer_ip,
                    primary_dns: Ipv4Addr::new(10, 55, 0, 1),
                    secondary_dns: Ipv4Addr::new(10, 55, 0, 1),
                    request_vj: false,
                    mobile_ip: MobileIpConfig::default(),
                },
                request_local_ip: true,
                request_vj: false,
                requested_vj: VjCompressionOptions::default(),
                peer_vj: None,
                local_vj: None,
                last_acked_peer_request_data: Vec::new(),
            },
            mobile_ip: Box::new(MobileIpSession::new(MobileIpConfig::default())),
            vj: VjState::default(),
        }
    }

    #[test]
    fn ppp_identity_uses_imsi_and_esn() {
        let metadata = SessionMetadata {
            imsi: Some("001010123456789".to_string()),
            esn: Some(0x1234abcd),
            ..SessionMetadata::default()
        };
        assert_eq!(
            ppp_identity_key(&metadata).as_deref(),
            Some("imsi:001010123456789:esn:1234abcd")
        );
    }

    #[test]
    fn ppp_session_store_hits_and_expires_by_last_activity() {
        let store = PppSessionStore::new();
        store.store(
            "imsi:test:esn:00000001".to_string(),
            "device:imsi:test:esn:00000001".to_string(),
            ppp_state(Ipv4Addr::new(10, 55, 0, 7)),
            Instant::now(),
        );

        match store.lookup("imsi:test:esn:00000001", Duration::from_secs(30)) {
            PppSessionCacheLookup::Hit(hit) => {
                assert_eq!(hit.peer_ip, Ipv4Addr::new(10, 55, 0, 7));
            }
            other => panic!("expected cache hit, got {other:?}"),
        }

        store.store(
            "imsi:test:esn:00000002".to_string(),
            "device:imsi:test:esn:00000002".to_string(),
            ppp_state(Ipv4Addr::new(10, 55, 0, 8)),
            Instant::now() - Duration::from_secs(31),
        );
        match store.lookup("imsi:test:esn:00000002", Duration::from_secs(30)) {
            PppSessionCacheLookup::Expired(expired) => {
                assert_eq!(expired.peer_ip, Ipv4Addr::new(10, 55, 0, 8));
            }
            other => panic!("expected cache expiry, got {other:?}"),
        }
        assert!(matches!(
            store.lookup("imsi:test:esn:00000002", Duration::from_secs(30)),
            PppSessionCacheLookup::Miss
        ));
    }

    #[test]
    fn ppp_session_store_reaps_expired_sessions() {
        let store = PppSessionStore::new();
        store.store(
            "imsi:test:esn:00000001".to_string(),
            "device:imsi:test:esn:00000001".to_string(),
            ppp_state(Ipv4Addr::new(10, 55, 0, 7)),
            Instant::now() - Duration::from_secs(29),
        );
        store.store(
            "imsi:test:esn:00000002".to_string(),
            "device:imsi:test:esn:00000002".to_string(),
            ppp_state(Ipv4Addr::new(10, 55, 0, 8)),
            Instant::now() - Duration::from_secs(31),
        );

        let expired = store.reap_expired(Duration::from_secs(30));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].identity_key, "imsi:test:esn:00000002");
        assert_eq!(expired[0].allocation_key, "device:imsi:test:esn:00000002");
        assert_eq!(expired[0].peer_ip, Ipv4Addr::new(10, 55, 0, 8));

        assert!(matches!(
            store.lookup("imsi:test:esn:00000001", Duration::from_secs(30)),
            PppSessionCacheLookup::Hit(_)
        ));
        assert!(matches!(
            store.lookup("imsi:test:esn:00000002", Duration::from_secs(30)),
            PppSessionCacheLookup::Miss
        ));
    }
}

fn tcp_range_retransmitted(highest_end: &mut Option<u32>, end_seq: u32) -> bool {
    match *highest_end {
        Some(prev) if !tcp_seq_after(end_seq, prev) => true,
        Some(_) | None => {
            *highest_end = Some(end_seq);
            false
        }
    }
}

fn tcp_ack_delta(highest_ack: &mut Option<u32>, ack: u32) -> u64 {
    match *highest_ack {
        Some(prev) if tcp_seq_after(ack, prev) => {
            *highest_ack = Some(ack);
            ack.wrapping_sub(prev) as u64
        }
        Some(_) => 0,
        None => {
            *highest_ack = Some(ack);
            0
        }
    }
}

fn tcp_seq_after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

impl TcpPacketInfo {
    fn uplink_flow_key(&self) -> TcpFlowKey {
        TcpFlowKey {
            client: self.src.clone(),
            server: self.dst.clone(),
            client_port: self.src_port,
            server_port: self.dst_port,
        }
    }

    fn downlink_flow_key(&self) -> TcpFlowKey {
        TcpFlowKey {
            client: self.dst.clone(),
            server: self.src.clone(),
            client_port: self.dst_port,
            server_port: self.src_port,
        }
    }
}

fn parse_tcp_packet(packet: &[u8]) -> Option<TcpPacketInfo> {
    if packet.len() < 40 || packet[0] >> 4 != 4 || packet[9] != 6 {
        return None;
    }
    let ihl_bytes = usize::from(packet[0] & 0x0f) * 4;
    if ihl_bytes < 20 || packet.len() < ihl_bytes + 20 {
        return None;
    }
    let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    let data_offset = usize::from(packet[ihl_bytes + 12] >> 4) * 4;
    let header_len = ihl_bytes + data_offset;
    if data_offset < 20 || packet.len() < header_len || total_len < header_len {
        return None;
    }
    let total_len = total_len.min(packet.len());
    Some(TcpPacketInfo {
        src: format!(
            "{}.{}.{}.{}",
            packet[12], packet[13], packet[14], packet[15]
        ),
        dst: format!(
            "{}.{}.{}.{}",
            packet[16], packet[17], packet[18], packet[19]
        ),
        src_port: u16::from_be_bytes([packet[ihl_bytes], packet[ihl_bytes + 1]]),
        dst_port: u16::from_be_bytes([packet[ihl_bytes + 2], packet[ihl_bytes + 3]]),
        seq: u32::from_be_bytes([
            packet[ihl_bytes + 4],
            packet[ihl_bytes + 5],
            packet[ihl_bytes + 6],
            packet[ihl_bytes + 7],
        ]),
        ack: u32::from_be_bytes([
            packet[ihl_bytes + 8],
            packet[ihl_bytes + 9],
            packet[ihl_bytes + 10],
            packet[ihl_bytes + 11],
        ]),
        flags: packet[ihl_bytes + 13],
        window: u16::from_be_bytes([packet[ihl_bytes + 14], packet[ihl_bytes + 15]]),
        ip_total_len: total_len,
        tcp_header_len: data_offset,
        payload_len: total_len.saturating_sub(ihl_bytes.saturating_add(data_offset)),
        options: parse_tcp_options(&packet[ihl_bytes + 20..ihl_bytes + data_offset]),
    })
}

impl TcpOptionsInfo {
    fn summary(&self) -> String {
        let mut fields = Vec::new();
        if let Some(mss) = self.mss {
            fields.push(format!("mss={}", mss));
        }
        if let Some(window_scale) = self.window_scale {
            fields.push(format!("wscale={}", window_scale));
        }
        if self.sack_permitted {
            fields.push("sack=ok".to_string());
        }
        if let Some((ts_val, ts_ecr)) = self.timestamp {
            fields.push(format!("ts={}:{}", ts_val, ts_ecr));
        }
        fields.join(" ")
    }
}

fn parse_tcp_options(options: &[u8]) -> TcpOptionsInfo {
    let mut parsed = TcpOptionsInfo::default();
    let mut pos = 0usize;
    while pos < options.len() {
        match options[pos] {
            0 => break,    // End of option list.
            1 => pos += 1, // NOP.
            kind => {
                if pos + 1 >= options.len() {
                    break;
                }
                let len = usize::from(options[pos + 1]);
                if len < 2 || pos + len > options.len() {
                    break;
                }
                let data = &options[pos + 2..pos + len];
                match kind {
                    2 if data.len() == 2 => {
                        parsed.mss = Some(u16::from_be_bytes([data[0], data[1]]));
                    }
                    3 if data.len() == 1 => {
                        parsed.window_scale = Some(data[0]);
                    }
                    4 if data.is_empty() => {
                        parsed.sack_permitted = true;
                    }
                    8 if data.len() == 8 => {
                        parsed.timestamp = Some((
                            u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
                            u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
                        ));
                    }
                    _ => {}
                }
                pos += len;
            }
        }
    }
    parsed
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
    let window = u16::from_be_bytes([packet[ihl_bytes + 14], packet[ihl_bytes + 15]]);
    let options = if flags & 0x02 != 0 {
        let parsed = parse_tcp_options(&packet[ihl_bytes + 20..ihl_bytes + data_offset]);
        let summary = parsed.summary();
        if summary.is_empty() {
            String::new()
        } else {
            format!(" opts=[{}]", summary)
        }
    } else {
        String::new()
    };
    format!(
        "IPv4 {}:{} -> {}:{} TCP flags={} seq={} ack={} win={} tcp_hlen={} payload={}{}",
        src,
        src_port,
        dst,
        dst_port,
        format_tcp_flags(flags),
        seq,
        ack,
        window,
        data_offset,
        payload_len,
        options
    )
}
