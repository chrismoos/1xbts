use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::engine::{
    PacketSessionTelemetry, PacketTraceEvent, PppSessionState, SessionPhase, now_ms,
};

/// Status snapshot for a session, shared with the gRPC query handlers.
///
/// `subscriber_id` is `None` for unprovisioned/roaming mobiles. The
/// `Option` flavor must be propagated through to the event bus so it
/// can do forward enrichment from `imsi`/`esn` instead of resolving a
/// bogus subscriber. Callers at the gRPC boundary should map empty
/// strings to `None`.
/// The A10 connection a session was registered on, from the A11 Session
/// Specific Extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A10Reference {
    pub pcf_session_id: u32,
    pub mn_session_reference_id: u16,
}

#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    pub access_technology: String,
    pub mobile_address: String,
    pub subscriber_id: Option<String>,
    pub phone_number: String,
    /// IMSI of the handset, if known at session-open time.
    pub imsi: Option<String>,
    /// ESN of the handset, if known at session-open time.
    pub esn: Option<u32>,
    /// MEID of the handset, if known at session-open time.
    pub meid: Option<String>,
    /// A11 MN ID used to register the A10 bearer. This can be an
    /// IMSI-format fallback and is not necessarily a subscriber IMSI.
    pub mn_id: Option<String>,
    pub mn_id_source: Option<String>,
    /// Authoritative subscriber IMSI, if separately resolved.
    pub subscriber_imsi: Option<String>,
    /// Present only for a session registered over A11.
    pub a10: Option<A10Reference>,
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
    pub imsi: String,
    pub esn: u32,
    pub meid: String,
    pub mn_id: String,
    pub mn_id_source: String,
    pub subscriber_imsi: String,
    /// Empty when the mobile sent none.
    pub nai: String,
    pub a10: Option<A10Reference>,
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
    pub access_technology: String,
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
    Hit(Box<PppSessionCacheHit>),
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
        PppSessionCacheLookup::Hit(Box::new(PppSessionCacheHit {
            state: cached.state.clone(),
            allocation_key: cached.allocation_key.clone(),
            peer_ip: cached.state.ipcp.config.peer_ip,
            idle_for,
        }))
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

    pub fn remove(&self, identity_key: &str) -> Option<PppSessionCacheExpired> {
        let cached = self.sessions.lock().unwrap().remove(identity_key)?;
        Some(PppSessionCacheExpired {
            identity_key: identity_key.to_string(),
            allocation_key: cached.allocation_key,
            peer_ip: cached.state.ipcp.config.peer_ip,
            idle_for: cached.last_activity_at.elapsed(),
        })
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
            access_technology: if metadata.access_technology.is_empty() {
                "1x".to_string()
            } else {
                metadata.access_technology.clone()
            },
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
            imsi: metadata.imsi.unwrap_or_default(),
            esn: metadata.esn.unwrap_or_default(),
            meid: metadata.meid.unwrap_or_default(),
            mn_id: metadata.mn_id.unwrap_or_default(),
            mn_id_source: metadata.mn_id_source.unwrap_or_default(),
            subscriber_imsi: metadata.subscriber_imsi.unwrap_or_default(),
            nai: String::new(),
            a10: metadata.a10,
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
        if let Some(nai) = telemetry.mobile_ip_nai {
            self.nai = nai;
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

pub fn ppp_identity_key(metadata: &SessionMetadata) -> Option<String> {
    if metadata.access_technology == "HRPD"
        && let Some(mn_id) = metadata.mn_id.as_deref().filter(|value| !value.is_empty())
    {
        return Some(format!("hrpd-mn-id:{mn_id}"));
    }
    match (metadata.imsi.as_deref(), metadata.esn) {
        (Some(imsi), Some(esn)) => Some(format!("imsi:{}:esn:{:08x}", imsi, esn)),
        (Some(imsi), None) => Some(format!("imsi:{}", imsi)),
        (None, Some(esn)) => Some(format!("esn:{:08x}", esn)),
        (None, None) => None,
    }
}

fn push_ring<T>(ring: &mut VecDeque<T>, item: T, max_len: usize) {
    if ring.len() >= max_len {
        ring.pop_front();
    }
    ring.push_back(item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile_ip::{MobileIpConfig, MobileIpSession};
    use crate::ppp::ipcp::{IpcpConfig, IpcpOpenState};
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
                    enable_uplink_vj_compression: false,
                    enable_downlink_vj_compression: false,
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
    fn hrpd_ppp_identity_uses_mn_id_without_claiming_imsi() {
        let metadata = SessionMetadata {
            access_technology: "HRPD".to_string(),
            mn_id: Some("310556898017332".to_string()),
            imsi: None,
            ..SessionMetadata::default()
        };
        assert_eq!(
            ppp_identity_key(&metadata).as_deref(),
            Some("hrpd-mn-id:310556898017332")
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

    #[test]
    fn ppp_session_store_removes_explicitly_invalidated_session() {
        let store = PppSessionStore::new();
        store.store(
            "imsi:test:esn:00000001".to_string(),
            "device:imsi:test:esn:00000001".to_string(),
            ppp_state(Ipv4Addr::new(10, 55, 0, 7)),
            Instant::now(),
        );

        let removed = store
            .remove("imsi:test:esn:00000001")
            .expect("cache entry should be removed");
        assert_eq!(removed.identity_key, "imsi:test:esn:00000001");
        assert_eq!(removed.allocation_key, "device:imsi:test:esn:00000001");
        assert_eq!(removed.peer_ip, Ipv4Addr::new(10, 55, 0, 7));
        assert!(store.remove("imsi:test:esn:00000001").is_none());
        assert!(matches!(
            store.lookup("imsi:test:esn:00000001", Duration::from_secs(30)),
            PppSessionCacheLookup::Miss
        ));
    }
}
