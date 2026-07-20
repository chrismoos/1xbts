//! PCF-owned packet-data session state.
//!
//! Owns packet anchoring — the A8 bearer binding and A11 forwarding queue —
//! while leaving the BSC-facing side as A9 signaling only.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

/// Result type used by the PCF session manager.
pub type Result<T> = std::result::Result<T, PcfError>;

/// Stable PCF-local session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PcfSessionId(pub u64);

/// PCF session lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcfSessionPhase {
    /// A9 setup has been accepted and the PCF is waiting for bearer binding.
    A9Accepted,
    /// A8 bearer is available and the PCF is registering the session over A11.
    A11RegistrationPending,
    /// A11 registration completed and bearer traffic may flow.
    Active,
    /// Release has been requested and cleanup is in progress.
    Releasing,
    /// Session state has been torn down.
    Released,
}

/// PCF session timer policy.
#[derive(Debug, Clone, Copy)]
pub struct PcfTimerPolicy {
    /// Maximum time in A9Accepted before the PCF releases the session.
    pub a9_setup_timeout: Duration,
    /// Maximum time in A11RegistrationPending before giving up.
    pub a11_registration_timeout: Duration,
    /// Maximum idle time in Active before the PCF initiates release.
    pub inactivity_timeout: Duration,
    /// Maximum time in Releasing before the PCF force-removes state.
    pub release_timeout: Duration,
}

impl Default for PcfTimerPolicy {
    fn default() -> Self {
        Self {
            a9_setup_timeout: Duration::from_secs(10),
            a11_registration_timeout: Duration::from_secs(15),
            inactivity_timeout: Duration::from_secs(120),
            release_timeout: Duration::from_secs(10),
        }
    }
}

/// PCF-owned view of a packet-data session.
#[derive(Debug, Clone)]
pub struct PcfSession {
    pub id: PcfSessionId,
    pub mobile_identity: Option<Vec<u8>>,
    pub phase: PcfSessionPhase,
    pub a8_bearer: Option<cdma_a8::BearerSession>,
    pub a10_bearer: Option<cdma_a10::BearerSession>,
    pub a11_session_key: Option<cdma_a11::SessionKey>,
    /// Timestamp when the session entered its current phase.
    pub phase_entered_at: Instant,
    /// Timestamp of the last data activity on this session.
    pub last_activity_at: Instant,
}

/// Observable PCF session-manager events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcfEvent {
    SessionCreated {
        id: PcfSessionId,
    },
    A9Applied(cdma_a9::ProcedureEvent),
    A8BearerBound {
        id: PcfSessionId,
    },
    A10BearerBound {
        id: PcfSessionId,
    },
    A11RegistrationQueued {
        id: PcfSessionId,
        queue_depth: usize,
    },
    A11RegistrationCompleted {
        id: PcfSessionId,
        key: cdma_a11::SessionKey,
    },
    ReleaseStarted {
        id: PcfSessionId,
    },
    SessionRemoved {
        id: PcfSessionId,
    },
    TimerExpired {
        id: PcfSessionId,
        phase: PcfSessionPhase,
        reason: &'static str,
    },
}

/// Errors returned by PCF session ownership operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcfError {
    DuplicateSession(PcfSessionId),
    UnknownSession(PcfSessionId),
    InvalidSessionPhase {
        id: PcfSessionId,
        phase: PcfSessionPhase,
        operation: &'static str,
    },
}

impl Display for PcfError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PcfError {}

/// PCF packet-data session manager.
#[derive(Debug)]
pub struct PcfSessionManager {
    a9_engine: cdma_a9::ProcedureEngine,
    next_session_id: u64,
    sessions: BTreeMap<PcfSessionId, PcfSession>,
    pending_a11: BTreeMap<PcfSessionId, VecDeque<cdma_a11::Message>>,
}

impl Default for PcfSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PcfSessionManager {
    /// Creates an empty manager.
    pub fn new() -> Self {
        Self {
            a9_engine: cdma_a9::ProcedureEngine::new(cdma_a9::ProcedureRole::Pcf),
            next_session_id: 0,
            sessions: BTreeMap::new(),
            pending_a11: BTreeMap::new(),
        }
    }

    /// Applies one inbound A9 procedure message from a BSC.
    pub fn apply_inbound_a9(
        &mut self,
        message: cdma_a9::ProcedureMessage,
    ) -> std::result::Result<PcfEvent, cdma_a9::Error> {
        self.a9_engine
            .apply_inbound(message)
            .map(PcfEvent::A9Applied)
    }

    /// Applies one outbound A9 procedure message emitted toward a BSC.
    pub fn apply_outbound_a9(
        &mut self,
        message: cdma_a9::ProcedureMessage,
    ) -> std::result::Result<PcfEvent, cdma_a9::Error> {
        self.a9_engine
            .apply_outbound(message)
            .map(PcfEvent::A9Applied)
    }

    /// Retargets the PCF's A9 procedure state after accepting a fresh
    /// `SetupA8` for an already-connected A8 traffic identifier.
    pub fn retarget_connected_a9(
        &mut self,
        setup: &cdma_a9::SetupA8Message,
        connect: &cdma_a9::ConnectA8Message,
    ) -> std::result::Result<PcfEvent, cdma_a9::Error> {
        self.a9_engine
            .retarget_connected_a8(setup, connect)
            .map(PcfEvent::A9Applied)
    }

    /// Retargets the PCF's A9 procedure state from an existing A8 traffic
    /// identifier to a fresh `SetupA8`/`ConnectA8` exchange.
    pub fn retarget_connected_a9_from(
        &mut self,
        existing_traffic_id: &cdma_a9::A8TrafficId,
        setup: &cdma_a9::SetupA8Message,
        connect: &cdma_a9::ConnectA8Message,
    ) -> std::result::Result<PcfEvent, cdma_a9::Error> {
        self.a9_engine
            .retarget_connected_a8_from(existing_traffic_id, setup, connect)
            .map(PcfEvent::A9Applied)
    }

    /// Finds an existing PCF session for a normalized A9 mobile identity.
    pub fn session_id_by_mobile_identity(&self, mobile_identity: &[u8]) -> Option<PcfSessionId> {
        self.sessions
            .values()
            .find(|session| session.mobile_identity.as_deref() == Some(mobile_identity))
            .map(|session| session.id)
    }

    /// Retargets an already-connected same-mobile A8 procedure to a fresh A8
    /// traffic identifier. This keeps duplicate SetupA8 recovery inside PCF
    /// state instead of letting the UDP service guess identity policy.
    pub fn retarget_same_mobile_connected_a9_from(
        &mut self,
        id: PcfSessionId,
        existing_traffic_id: &cdma_a9::A8TrafficId,
        mobile_identity: &[u8],
        setup: &cdma_a9::SetupA8Message,
        connect: &cdma_a9::ConnectA8Message,
    ) -> std::result::Result<PcfEvent, cdma_a9::Error> {
        let session = self
            .sessions
            .get(&id)
            .ok_or(cdma_a9::Error::UnknownSession)?;
        if session.mobile_identity.as_deref() != Some(mobile_identity) {
            return Err(cdma_a9::Error::InvalidProcedureState {
                message_type: cdma_a9::MessageType::SetupA8,
                state: "duplicate SetupA8 mobile identity does not match PCF session",
            });
        }
        self.retarget_connected_a9_from(existing_traffic_id, setup, connect)
    }

    /// Starts a PCF-owned session after accepting A9 setup from the BSC.
    pub fn create_from_a9(&mut self, mobile_identity: Option<Vec<u8>>) -> Result<PcfEvent> {
        self.next_session_id = self.next_session_id.wrapping_add(1);
        let id = PcfSessionId(self.next_session_id);
        if self.sessions.contains_key(&id) {
            return Err(PcfError::DuplicateSession(id));
        }

        let now = Instant::now();
        self.sessions.insert(
            id,
            PcfSession {
                id,
                mobile_identity,
                phase: PcfSessionPhase::A9Accepted,
                a8_bearer: None,
                a10_bearer: None,
                a11_session_key: None,
                phase_entered_at: now,
                last_activity_at: now,
            },
        );

        Ok(PcfEvent::SessionCreated { id })
    }

    /// Binds the A8 GRE bearer negotiated for a PCF session.
    pub fn bind_a8_bearer(
        &mut self,
        id: PcfSessionId,
        bearer: cdma_a8::BearerSession,
    ) -> Result<PcfEvent> {
        let session = self.session_mut(id)?;
        if !matches!(session.phase, PcfSessionPhase::A9Accepted) {
            return Err(PcfError::InvalidSessionPhase {
                id,
                phase: session.phase,
                operation: "bind_a8_bearer",
            });
        }

        session.a8_bearer = Some(bearer);
        session.phase = PcfSessionPhase::A11RegistrationPending;
        session.phase_entered_at = Instant::now();
        session.last_activity_at = Instant::now();
        Ok(PcfEvent::A8BearerBound { id })
    }

    /// Binds the A10 GRE bearer negotiated toward the PDSN.
    pub fn bind_a10_bearer(
        &mut self,
        id: PcfSessionId,
        bearer: cdma_a10::BearerSession,
    ) -> Result<PcfEvent> {
        let session = self.session_mut(id)?;
        if !matches!(
            session.phase,
            PcfSessionPhase::A11RegistrationPending | PcfSessionPhase::Active
        ) {
            return Err(PcfError::InvalidSessionPhase {
                id,
                phase: session.phase,
                operation: "bind_a10_bearer",
            });
        }

        session.a10_bearer = Some(bearer);
        Ok(PcfEvent::A10BearerBound { id })
    }

    /// Queues an A11 message for transmission to the PDSN.
    pub fn enqueue_a11(
        &mut self,
        id: PcfSessionId,
        message: cdma_a11::Message,
    ) -> Result<PcfEvent> {
        let phase = self.session(id)?.phase;
        if !matches!(
            phase,
            PcfSessionPhase::A11RegistrationPending
                | PcfSessionPhase::Active
                | PcfSessionPhase::Releasing
        ) {
            return Err(PcfError::InvalidSessionPhase {
                id,
                phase,
                operation: "enqueue_a11",
            });
        }

        let queue = self.pending_a11.entry(id).or_default();
        queue.push_back(message);
        Ok(PcfEvent::A11RegistrationQueued {
            id,
            queue_depth: queue.len(),
        })
    }

    /// Pops the next A11 message for any session that should be sent to the PDSN.
    pub fn pop_pending_a11(&mut self) -> Option<cdma_a11::Message> {
        let first_key = *self.pending_a11.keys().next()?;
        let queue = self.pending_a11.get_mut(&first_key)?;
        let msg = queue.pop_front();
        if queue.is_empty() {
            self.pending_a11.remove(&first_key);
        }
        msg
    }

    /// Marks a session active after the PDSN accepts A11 registration.
    ///
    /// A positive-lifetime A11 Registration Request is also used as a refresh
    /// for an already-active HRPD packet session. That refresh is important
    /// after an AT-requested PPP teardown: the PCF may still own the A8/A9
    /// session while the PDSN has removed its A10 packet task, so the next
    /// traffic-channel retarget must be able to complete A11 registration again.
    pub fn complete_a11_registration(
        &mut self,
        id: PcfSessionId,
        key: cdma_a11::SessionKey,
    ) -> Result<PcfEvent> {
        let session = self.session_mut(id)?;
        if !matches!(
            session.phase,
            PcfSessionPhase::A11RegistrationPending | PcfSessionPhase::Active
        ) {
            return Err(PcfError::InvalidSessionPhase {
                id,
                phase: session.phase,
                operation: "complete_a11_registration",
            });
        }

        session.a11_session_key = Some(key);
        session.phase = PcfSessionPhase::Active;
        session.phase_entered_at = Instant::now();
        session.last_activity_at = Instant::now();
        Ok(PcfEvent::A11RegistrationCompleted { id, key })
    }

    /// Starts controlled release of a session.
    pub fn start_release(&mut self, id: PcfSessionId) -> Result<PcfEvent> {
        let session = self.session_mut(id)?;
        session.phase = PcfSessionPhase::Releasing;
        session.phase_entered_at = Instant::now();
        Ok(PcfEvent::ReleaseStarted { id })
    }

    /// Removes all PCF-owned state for a session.
    pub fn remove_session(&mut self, id: PcfSessionId) -> Result<PcfEvent> {
        self.sessions
            .remove(&id)
            .ok_or(PcfError::UnknownSession(id))?;
        Ok(PcfEvent::SessionRemoved { id })
    }

    /// Returns an immutable session snapshot.
    pub fn session(&self, id: PcfSessionId) -> Result<&PcfSession> {
        self.sessions.get(&id).ok_or(PcfError::UnknownSession(id))
    }

    /// Returns all installed sessions in stable identifier order.
    pub fn sessions(&self) -> impl Iterator<Item = &PcfSession> {
        self.sessions.values()
    }

    /// Records data activity on a session (resets the inactivity timer).
    pub fn record_activity(&mut self, id: PcfSessionId) -> Result<()> {
        let session = self.session_mut(id)?;
        session.last_activity_at = Instant::now();
        Ok(())
    }

    /// Polls timer-driven state transitions and returns expired-session events.
    pub fn poll_timers(&mut self, policy: &PcfTimerPolicy) -> Vec<PcfEvent> {
        let mut events = Vec::new();
        let ids: Vec<PcfSessionId> = self.sessions.keys().copied().collect();
        for id in ids {
            let Some(session) = self.sessions.get(&id) else {
                continue;
            };
            let elapsed = session.phase_entered_at.elapsed();
            match session.phase {
                PcfSessionPhase::A9Accepted => {
                    if elapsed >= policy.a9_setup_timeout {
                        self.sessions.remove(&id);
                        events.push(PcfEvent::TimerExpired {
                            id,
                            phase: PcfSessionPhase::A9Accepted,
                            reason: "A9 setup timeout",
                        });
                    }
                }
                PcfSessionPhase::A11RegistrationPending => {
                    if elapsed >= policy.a11_registration_timeout {
                        self.sessions.remove(&id);
                        events.push(PcfEvent::TimerExpired {
                            id,
                            phase: PcfSessionPhase::A11RegistrationPending,
                            reason: "A11 registration timeout",
                        });
                    }
                }
                PcfSessionPhase::Active => {
                    if session.last_activity_at.elapsed() >= policy.inactivity_timeout {
                        if let Some(s) = self.sessions.get_mut(&id) {
                            s.phase = PcfSessionPhase::Releasing;
                            s.phase_entered_at = Instant::now();
                        }
                        events.push(PcfEvent::TimerExpired {
                            id,
                            phase: PcfSessionPhase::Active,
                            reason: "inactivity timeout",
                        });
                    }
                }
                PcfSessionPhase::Releasing => {
                    if elapsed >= policy.release_timeout {
                        self.sessions.remove(&id);
                        events.push(PcfEvent::TimerExpired {
                            id,
                            phase: PcfSessionPhase::Releasing,
                            reason: "release timeout",
                        });
                    }
                }
                PcfSessionPhase::Released => {}
            }
        }
        events
    }

    /// Computes the next deadline for timer polling.
    pub fn next_poll_deadline(&self, policy: &PcfTimerPolicy) -> Option<Instant> {
        let mut earliest: Option<Instant> = None;
        for session in self.sessions.values() {
            let deadline = match session.phase {
                PcfSessionPhase::A9Accepted => session.phase_entered_at + policy.a9_setup_timeout,
                PcfSessionPhase::A11RegistrationPending => {
                    session.phase_entered_at + policy.a11_registration_timeout
                }
                PcfSessionPhase::Active => session.last_activity_at + policy.inactivity_timeout,
                PcfSessionPhase::Releasing => session.phase_entered_at + policy.release_timeout,
                PcfSessionPhase::Released => continue,
            };
            earliest = Some(earliest.map_or(deadline, |e: Instant| e.min(deadline)));
        }
        earliest
    }

    fn session_mut(&mut self, id: PcfSessionId) -> Result<&mut PcfSession> {
        self.sessions
            .get_mut(&id)
            .ok_or(PcfError::UnknownSession(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_message(con_ref: u8, key: u32) -> cdma_a9::SetupA8Message {
        cdma_a9::SetupA8Message {
            call_connection_reference: Some(cdma_a9::CallConnectionReference::new(
                0x0102, 0x0304, 0x05060708,
            )),
            correlation_id: Some(cdma_a9::CorrelationId([9, 10, 11, 12])),
            imsi: Some("123456789012345".into()),
            esn: Some(0x01020304),
            meid: None,
            con_ref: cdma_a9::ConRef(con_ref),
            quality_of_service_parameters: None,
            bsc_id: cdma_a9::BscId(vec![0xaa, 0xbb]),
            a8_traffic_id: cdma_a9::A8TrafficId::gre_ppp(key, [192, 0, 2, 10]),
            service_option: cdma_a9::ServiceOptionValue::HIGH_RATE_PACKET_DATA,
            a9_indicators: cdma_a9::A9Indicators {
                packet_boundary_supported: false,
                gre_segmentation_supported: false,
                sdb_supported: false,
                ccpd_mode: false,
                data_ready: true,
                handoff: false,
            },
            user_zone_id: None,
        }
    }

    fn connect_response(setup: &cdma_a9::SetupA8Message) -> cdma_a9::ConnectA8Message {
        cdma_a9::ConnectA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: setup.imsi.clone(),
            esn: setup.esn,
            meid: None,
            con_ref: setup.con_ref,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            cause: cdma_a9::CauseValue(0x13),
            pdsn_ip_address: None,
        }
    }

    #[test]
    fn a9_setup_timeout_removes_session() {
        let mut mgr = PcfSessionManager::new();
        let ev = mgr.create_from_a9(None).unwrap();
        let id = match ev {
            PcfEvent::SessionCreated { id } => id,
            _ => panic!("unexpected event"),
        };
        if let Some(s) = mgr.sessions.get_mut(&id) {
            s.phase_entered_at = Instant::now() - Duration::from_secs(20);
        }
        let policy = PcfTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            PcfEvent::TimerExpired {
                phase: PcfSessionPhase::A9Accepted,
                reason: "A9 setup timeout",
                ..
            }
        ));
        assert!(mgr.session(id).is_err());
    }

    #[test]
    fn inactivity_timeout_transitions_active_to_releasing() {
        let mut mgr = PcfSessionManager::new();
        let ev = mgr.create_from_a9(None).unwrap();
        let id = match ev {
            PcfEvent::SessionCreated { id } => id,
            _ => panic!("unexpected event"),
        };
        mgr.bind_a8_bearer(
            id,
            cdma_a8::BearerSession::new(
                1,
                cdma_a8::BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 2]),
            ),
        )
        .unwrap();
        let key = cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 100,
        };
        mgr.complete_a11_registration(id, key).unwrap();
        assert_eq!(mgr.session(id).unwrap().phase, PcfSessionPhase::Active);

        if let Some(s) = mgr.sessions.get_mut(&id) {
            s.last_activity_at = Instant::now() - Duration::from_secs(200);
        }
        let policy = PcfTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            PcfEvent::TimerExpired {
                phase: PcfSessionPhase::Active,
                reason: "inactivity timeout",
                ..
            }
        ));
        assert_eq!(mgr.session(id).unwrap().phase, PcfSessionPhase::Releasing);
    }

    #[test]
    fn a11_registration_refresh_keeps_active_session() {
        let mut mgr = PcfSessionManager::new();
        let ev = mgr.create_from_a9(None).unwrap();
        let id = match ev {
            PcfEvent::SessionCreated { id } => id,
            _ => panic!("unexpected event"),
        };
        mgr.bind_a8_bearer(
            id,
            cdma_a8::BearerSession::new(
                1,
                cdma_a8::BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 2]),
            ),
        )
        .unwrap();
        let key = cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 100,
        };
        mgr.complete_a11_registration(id, key).unwrap();

        let refreshed_key = cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 101,
        };
        let event = mgr.complete_a11_registration(id, refreshed_key).unwrap();

        assert_eq!(
            event,
            PcfEvent::A11RegistrationCompleted {
                id,
                key: refreshed_key
            }
        );
        let session = mgr.session(id).unwrap();
        assert_eq!(session.phase, PcfSessionPhase::Active);
        assert_eq!(session.a11_session_key, Some(refreshed_key));
    }

    #[test]
    fn duplicate_mobile_retarget_requires_matching_pcf_identity() {
        let mut mgr = PcfSessionManager::new();
        let identity = vec![0x11, 0x22, 0x33];
        let setup = setup_message(0x10, 0x0102_0304);
        let connect = connect_response(&setup);
        mgr.apply_inbound_a9(cdma_a9::ProcedureMessage::SetupA8(setup.clone()))
            .unwrap();
        let id = match mgr.create_from_a9(Some(identity.clone())).unwrap() {
            PcfEvent::SessionCreated { id } => id,
            event => panic!("unexpected event {event:?}"),
        };
        mgr.apply_outbound_a9(cdma_a9::ProcedureMessage::ConnectA8(connect))
            .unwrap();

        assert_eq!(mgr.session_id_by_mobile_identity(&identity), Some(id));

        let retarget_setup = setup_message(0x11, 0x0102_0305);
        let retarget_connect = connect_response(&retarget_setup);
        mgr.retarget_same_mobile_connected_a9_from(
            id,
            &setup.a8_traffic_id,
            &identity,
            &retarget_setup,
            &retarget_connect,
        )
        .unwrap();

        let err = mgr
            .retarget_same_mobile_connected_a9_from(
                id,
                &retarget_setup.a8_traffic_id,
                &[0xff],
                &setup,
                &connect_response(&setup),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            cdma_a9::Error::InvalidProcedureState {
                message_type: cdma_a9::MessageType::SetupA8,
                ..
            }
        ));
    }

    #[test]
    fn release_timeout_removes_session() {
        let mut mgr = PcfSessionManager::new();
        let ev = mgr.create_from_a9(None).unwrap();
        let id = match ev {
            PcfEvent::SessionCreated { id } => id,
            _ => panic!("unexpected event"),
        };
        mgr.start_release(id).unwrap();
        if let Some(s) = mgr.sessions.get_mut(&id) {
            s.phase_entered_at = Instant::now() - Duration::from_secs(20);
        }
        let policy = PcfTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            PcfEvent::TimerExpired {
                phase: PcfSessionPhase::Releasing,
                ..
            }
        ));
        assert!(mgr.session(id).is_err());
    }

    #[test]
    fn record_activity_resets_inactivity_timer() {
        let mut mgr = PcfSessionManager::new();
        let ev = mgr.create_from_a9(None).unwrap();
        let id = match ev {
            PcfEvent::SessionCreated { id } => id,
            _ => panic!("unexpected event"),
        };
        mgr.bind_a8_bearer(
            id,
            cdma_a8::BearerSession::new(
                1,
                cdma_a8::BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 2]),
            ),
        )
        .unwrap();
        let key = cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 100,
        };
        mgr.complete_a11_registration(id, key).unwrap();

        if let Some(s) = mgr.sessions.get_mut(&id) {
            s.last_activity_at = Instant::now() - Duration::from_secs(100);
        }
        mgr.record_activity(id).unwrap();
        let policy = PcfTimerPolicy::default();
        let events = mgr.poll_timers(&policy);
        assert!(events.is_empty());
        assert_eq!(mgr.session(id).unwrap().phase, PcfSessionPhase::Active);
    }

    #[test]
    fn next_poll_deadline_returns_none_when_empty() {
        let mgr = PcfSessionManager::new();
        let policy = PcfTimerPolicy::default();
        assert!(mgr.next_poll_deadline(&policy).is_none());
    }
}
