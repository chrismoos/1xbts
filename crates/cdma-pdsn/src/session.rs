//! PDSN-owned packet-data session state.
//!
//! This module keeps A11 registration state, allocated mobile IP address, and
//! A10 bearer binding out of the BSC and PCF crates.

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

/// Result type used by the PDSN session manager.
pub type Result<T> = std::result::Result<T, PdsnError>;

/// Simple sequential IPv4 allocation pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpPool {
    pub base: Ipv4Addr,
    pub next_host_offset: u32,
}

impl Default for IpPool {
    fn default() -> Self {
        Self {
            base: Ipv4Addr::new(10, 64, 0, 0),
            next_host_offset: 2,
        }
    }
}

impl IpPool {
    /// Allocates the next IPv4 address from the pool.
    ///
    /// Returns `None` when the pool is exhausted (offset would overflow).
    pub fn allocate(&mut self) -> Option<Ipv4Addr> {
        let base = u32::from(self.base);
        let offset = self.next_host_offset;
        let addr = base.checked_add(offset)?;
        self.next_host_offset = offset.checked_add(1)?;
        Some(Ipv4Addr::from(addr))
    }
}

/// PDSN session timer policy.
#[derive(Debug, Clone, Copy)]
pub struct PdsnTimerPolicy {
    /// Maximum idle time before the PDSN releases a session.
    pub inactivity_timeout: Duration,
    /// Maximum time a session may remain in Releasing before force-removal.
    pub release_timeout: Duration,
    /// A11 registration lifetime — the PDSN expects the PCF to refresh
    /// within this window or the session is expired.
    pub registration_lifetime: Duration,
}

impl Default for PdsnTimerPolicy {
    fn default() -> Self {
        Self {
            inactivity_timeout: Duration::from_secs(120),
            release_timeout: Duration::from_secs(10),
            registration_lifetime: Duration::from_secs(600),
        }
    }
}

/// PDSN session lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdsnSessionPhase {
    Registered,
    A10Bound,
    Releasing,
}

/// PDSN-owned packet-data session.
#[derive(Clone)]
pub struct PdsnSession {
    pub key: cdma_a11::SessionKey,
    pub phase: PdsnSessionPhase,
    pub mobile_ip: Ipv4Addr,
    pub a10_bearer: Option<cdma_a10::BearerSession>,
    pub legacy_status: Option<Arc<Mutex<cdma_packet::session_task::SessionStatus>>>,
    /// Timestamp when the session was registered or last refreshed.
    pub registered_at: Instant,
    /// Timestamp of the last data activity on this session.
    pub last_activity_at: Instant,
    /// Timestamp when release was initiated.
    pub release_started_at: Option<Instant>,
}

impl std::fmt::Debug for PdsnSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdsnSession")
            .field("key", &self.key)
            .field("phase", &self.phase)
            .field("mobile_ip", &self.mobile_ip)
            .field("a10_bearer", &self.a10_bearer)
            .field("legacy_status_present", &self.legacy_status.is_some())
            .field("registered_at", &self.registered_at)
            .field("last_activity_at", &self.last_activity_at)
            .field("release_started_at", &self.release_started_at)
            .finish()
    }
}

/// Observable PDSN events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdsnEvent {
    A11Applied(cdma_a11::ProcedureEvent),
    SessionRegistered {
        key: cdma_a11::SessionKey,
        mobile_ip: Ipv4Addr,
    },
    A10BearerBound {
        key: cdma_a11::SessionKey,
    },
    SessionRemoved {
        key: cdma_a11::SessionKey,
    },
    TimerExpired {
        key: cdma_a11::SessionKey,
        phase: PdsnSessionPhase,
        reason: &'static str,
    },
}

/// Errors returned by PDSN session operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdsnError {
    A11(cdma_a11::Error),
    UnknownSession(cdma_a11::SessionKey),
    DuplicateSession(cdma_a11::SessionKey),
    IpPoolExhausted,
}

impl From<cdma_a11::Error> for PdsnError {
    fn from(value: cdma_a11::Error) -> Self {
        Self::A11(value)
    }
}

impl Display for PdsnError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PdsnError {}

/// PDSN A11/A10 session manager.
#[derive(Debug, Default)]
pub struct PdsnSessionManager {
    procedures: cdma_a11::SessionProcedureTable,
    sessions: HashMap<cdma_a11::SessionKey, PdsnSession>,
    ip_pool: IpPool,
}

impl PdsnSessionManager {
    /// Creates a manager with the default mobile-IP pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a manager with an explicit mobile-IP pool.
    pub fn with_ip_pool(ip_pool: IpPool) -> Self {
        Self {
            procedures: cdma_a11::SessionProcedureTable::new(),
            sessions: HashMap::new(),
            ip_pool,
        }
    }

    /// Applies one A11 message to the PDSN procedure table.
    pub fn apply_a11(
        &mut self,
        now_seconds: u64,
        direction: cdma_a11::Direction,
        message: &cdma_a11::Message,
    ) -> Result<PdsnEvent> {
        let event = self.procedures.apply(now_seconds, direction, message)?;

        match event {
            cdma_a11::ProcedureEvent::Registered { key, .. }
            | cdma_a11::ProcedureEvent::Refreshed { key, .. } => {
                if !self.sessions.contains_key(&key) {
                    let mobile_ip = self.ip_pool.allocate().ok_or(PdsnError::IpPoolExhausted)?;
                    let now = Instant::now();
                    self.sessions.insert(
                        key,
                        PdsnSession {
                            key,
                            phase: PdsnSessionPhase::Registered,
                            mobile_ip,
                            a10_bearer: None,
                            legacy_status: None,
                            registered_at: now,
                            last_activity_at: now,
                            release_started_at: None,
                        },
                    );
                    return Ok(PdsnEvent::SessionRegistered { key, mobile_ip });
                }
            }
            cdma_a11::ProcedureEvent::Cleared { key, .. }
            | cdma_a11::ProcedureEvent::Expired { key }
            | cdma_a11::ProcedureEvent::Rejected { key, .. } => {
                self.sessions.remove(&key);
                return Ok(PdsnEvent::SessionRemoved { key });
            }
            _ => {}
        }

        Ok(PdsnEvent::A11Applied(event))
    }

    /// Binds the A10 GRE bearer for an accepted A11 session.
    pub fn bind_a10_bearer(
        &mut self,
        key: cdma_a11::SessionKey,
        bearer: cdma_a10::BearerSession,
    ) -> Result<PdsnEvent> {
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(PdsnError::UnknownSession(key))?;
        session.a10_bearer = Some(bearer);
        session.phase = PdsnSessionPhase::A10Bound;
        Ok(PdsnEvent::A10BearerBound { key })
    }

    /// Installs a PDSN session after an accepted A11 registration.
    pub fn install_registered_session(&mut self, key: cdma_a11::SessionKey) -> Result<PdsnEvent> {
        if self.sessions.contains_key(&key) {
            return Err(PdsnError::DuplicateSession(key));
        }
        let mobile_ip = self.ip_pool.allocate().ok_or(PdsnError::IpPoolExhausted)?;
        let now = Instant::now();
        self.sessions.insert(
            key,
            PdsnSession {
                key,
                phase: PdsnSessionPhase::Registered,
                mobile_ip,
                a10_bearer: None,
                legacy_status: None,
                registered_at: now,
                last_activity_at: now,
                release_started_at: None,
            },
        );
        Ok(PdsnEvent::SessionRegistered { key, mobile_ip })
    }

    /// Starts the legacy PPP/IP/TUN packet-core task under PDSN ownership.
    ///
    /// This is a migration adapter: packet-core anchoring is now modeled as
    /// PDSN state even while the old `cdma-packet` engine still performs PPP,
    /// IPCP, and local host I/O.
    pub fn start_legacy_packet_core(
        &mut self,
        key: cdma_a11::SessionKey,
        session_id: String,
        service_option: u32,
        metadata: cdma_packet::session_task::SessionMetadata,
        transport: Box<dyn cdma_packet::ip_transport::IpTransport>,
        allocator: Arc<dyn cdma_packet::ip_allocator::IpAllocator>,
    ) -> Result<(
        mpsc::Sender<cdma_packet::proto::SessionFrame>,
        mpsc::Receiver<cdma_packet::proto::SessionFrame>,
    )> {
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(PdsnError::UnknownSession(key))?;
        let (uplink_tx, uplink_rx) = mpsc::channel(256);
        let (downlink_tx, downlink_rx) = mpsc::channel(256);
        let metadata_for_task = metadata.clone();
        let status = Arc::new(Mutex::new(cdma_packet::session_task::SessionStatus::new(
            service_option,
            metadata,
        )));
        session.legacy_status = Some(status.clone());
        // PDSN-managed sessions don't currently expose F-SCH control; the
        // sender is dropped immediately so `control_rx.recv()` always yields
        // `None` and the session stays FCH-only.
        let (_control_tx, control_rx) =
            mpsc::channel::<cdma_packet::session_task::SessionControl>(1);
        let sink: Arc<dyn cdma_packet::session_lifecycle::SessionLifecycleSink> =
            Arc::new(cdma_packet::session_lifecycle::NullSink);
        tokio::spawn(async move {
            cdma_packet::session_task::run_session(
                session_id,
                service_option,
                transport,
                uplink_rx,
                downlink_tx,
                status,
                allocator,
                control_rx,
                metadata_for_task,
                sink,
            )
            .await;
        });
        Ok((uplink_tx, downlink_rx))
    }

    /// Returns one PDSN session.
    pub fn session(&self, key: cdma_a11::SessionKey) -> Option<&PdsnSession> {
        self.sessions.get(&key)
    }

    /// Returns installed PDSN sessions.
    pub fn sessions(&self) -> impl Iterator<Item = &PdsnSession> {
        self.sessions.values()
    }

    /// Records data activity on a session (resets the inactivity timer).
    pub fn record_activity(&mut self, key: cdma_a11::SessionKey) -> Result<()> {
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(PdsnError::UnknownSession(key))?;
        session.last_activity_at = Instant::now();
        Ok(())
    }

    /// Polls timer-driven state transitions and returns expired-session events.
    pub fn poll_timers(&mut self, policy: &PdsnTimerPolicy) -> Vec<PdsnEvent> {
        let mut events = Vec::new();
        let snapshots: Vec<(
            cdma_a11::SessionKey,
            PdsnSessionPhase,
            Instant,
            Instant,
            Option<Instant>,
        )> = self
            .sessions
            .values()
            .map(|s| {
                (
                    s.key,
                    s.phase,
                    s.registered_at,
                    s.last_activity_at,
                    s.release_started_at,
                )
            })
            .collect();

        for (key, phase, registered_at, last_activity_at, release_started_at) in snapshots {
            match phase {
                PdsnSessionPhase::Registered | PdsnSessionPhase::A10Bound => {
                    if registered_at.elapsed() >= policy.registration_lifetime {
                        self.sessions.remove(&key);
                        events.push(PdsnEvent::TimerExpired {
                            key,
                            phase,
                            reason: "registration lifetime expired",
                        });
                    } else if last_activity_at.elapsed() >= policy.inactivity_timeout {
                        if let Some(s) = self.sessions.get_mut(&key) {
                            s.phase = PdsnSessionPhase::Releasing;
                            s.release_started_at = Some(Instant::now());
                        }
                        events.push(PdsnEvent::TimerExpired {
                            key,
                            phase,
                            reason: "inactivity timeout",
                        });
                    }
                }
                PdsnSessionPhase::Releasing => {
                    if let Some(rel_at) = release_started_at {
                        if rel_at.elapsed() >= policy.release_timeout {
                            self.sessions.remove(&key);
                            events.push(PdsnEvent::TimerExpired {
                                key,
                                phase: PdsnSessionPhase::Releasing,
                                reason: "release timeout",
                            });
                        }
                    }
                }
            }
        }
        events
    }

    /// Computes the next deadline for timer polling.
    pub fn next_poll_deadline(&self, policy: &PdsnTimerPolicy) -> Option<Instant> {
        let mut earliest: Option<Instant> = None;
        for session in self.sessions.values() {
            let deadline = match session.phase {
                PdsnSessionPhase::Registered | PdsnSessionPhase::A10Bound => {
                    let reg_deadline = session.registered_at + policy.registration_lifetime;
                    let idle_deadline = session.last_activity_at + policy.inactivity_timeout;
                    reg_deadline.min(idle_deadline)
                }
                PdsnSessionPhase::Releasing => match session.release_started_at {
                    Some(rel_at) => rel_at + policy.release_timeout,
                    None => continue,
                },
            };
            earliest = Some(earliest.map_or(deadline, |e: Instant| e.min(deadline)));
        }
        earliest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> cdma_a11::SessionKey {
        cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 100,
        }
    }

    #[test]
    fn inactivity_timeout_transitions_to_releasing() {
        let mut mgr = PdsnSessionManager::new();
        let key = test_key();
        mgr.install_registered_session(key).unwrap();
        if let Some(s) = mgr.sessions.get_mut(&key) {
            s.last_activity_at = Instant::now() - Duration::from_secs(200);
        }
        let policy = PdsnTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            PdsnEvent::TimerExpired {
                reason: "inactivity timeout",
                ..
            }
        ));
        assert_eq!(mgr.session(key).unwrap().phase, PdsnSessionPhase::Releasing);
    }

    #[test]
    fn registration_lifetime_expires_removes_session() {
        let mut mgr = PdsnSessionManager::new();
        let key = test_key();
        mgr.install_registered_session(key).unwrap();
        if let Some(s) = mgr.sessions.get_mut(&key) {
            s.registered_at = Instant::now() - Duration::from_secs(700);
        }
        let policy = PdsnTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            PdsnEvent::TimerExpired {
                reason: "registration lifetime expired",
                ..
            }
        ));
        assert!(mgr.session(key).is_none());
    }

    #[test]
    fn release_timeout_removes_session() {
        let mut mgr = PdsnSessionManager::new();
        let key = test_key();
        mgr.install_registered_session(key).unwrap();
        if let Some(s) = mgr.sessions.get_mut(&key) {
            s.phase = PdsnSessionPhase::Releasing;
            s.release_started_at = Some(Instant::now() - Duration::from_secs(20));
        }
        let policy = PdsnTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            PdsnEvent::TimerExpired {
                phase: PdsnSessionPhase::Releasing,
                reason: "release timeout",
                ..
            }
        ));
        assert!(mgr.session(key).is_none());
    }

    #[test]
    fn record_activity_resets_inactivity() {
        let mut mgr = PdsnSessionManager::new();
        let key = test_key();
        mgr.install_registered_session(key).unwrap();
        if let Some(s) = mgr.sessions.get_mut(&key) {
            s.last_activity_at = Instant::now() - Duration::from_secs(100);
        }
        mgr.record_activity(key).unwrap();
        let policy = PdsnTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert!(events.is_empty());
    }

    #[test]
    fn next_poll_deadline_returns_none_when_empty() {
        let mgr = PdsnSessionManager::new();
        let policy = PdsnTimerPolicy::default();
        assert!(mgr.next_poll_deadline(&policy).is_none());
    }
}
