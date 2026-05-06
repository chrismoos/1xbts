//! Session and procedure state for A11 registration, session-update, and capabilities signaling.

use std::collections::HashMap;

use crate::{
    CapabilitiesInfo, CapabilitiesInfoAcknowledge, Error, Message, RegistrationAcknowledge,
    RegistrationReply, RegistrationRequest, RegistrationUpdate, Result, SessionSpecificExtension,
    SessionUpdate, SessionUpdateAcknowledge, VerifiedMessage, validate_acknowledge,
    validate_capabilities_info, validate_capabilities_info_ack, validate_reply, validate_request,
    validate_session_update, validate_session_update_acknowledge, validate_update,
};

/// Applies a message as seen by the local node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The local node is transmitting the message.
    Outbound,
    /// The local node is receiving the message.
    Inbound,
}

/// Stable key for a single A11 registration session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub pcf_session_id: u32,
    pub mn_session_reference_id: u16,
}

impl SessionKey {
    /// Builds a key from a typed Session Specific Extension.
    pub fn from_session(session: &SessionSpecificExtension) -> Self {
        Self {
            pcf_session_id: session.pcf_session_id,
            mn_session_reference_id: session.mn_session_reference_id,
        }
    }
}

/// Externally visible session state tracked by the local procedure table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// A registration request or periodic re-registration is awaiting a reply.
    PendingRegistration,
    /// The session is established and has a committed lifetime.
    Active,
    /// A remote registration update is awaiting a local acknowledge.
    PendingUpdateAcknowledge,
    /// A local teardown request is awaiting a reply.
    PendingTeardown,
    /// A remote session update is awaiting a local acknowledge.
    PendingSessionUpdateAcknowledge,
    /// A locally initiated session update is awaiting an acknowledge.
    PendingSessionUpdate,
}

/// Reason a session left the procedure table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearReason {
    /// The local node initiated teardown with a lifetime-zero registration request.
    LocalTeardown,
    /// The remote node initiated teardown with a registration update.
    RemoteTeardown,
    /// The session was cleared locally without an on-wire teardown.
    Administrative,
    /// The granted lifetime expired before a refresh completed.
    LifetimeExpired,
}

/// Snapshot of a tracked session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub key: SessionKey,
    pub state: SessionState,
    pub protocol_type: u16,
    pub mn_id: Vec<u8>,
    pub home_agent: [u8; 4],
    pub care_of_address: [u8; 4],
    pub identification: u64,
    pub granted_lifetime: u16,
    pub expires_at: Option<u64>,
}

/// Result of applying a message or timer action to the procedure table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcedureEvent {
    PendingRegistration {
        key: SessionKey,
    },
    PendingRefresh {
        key: SessionKey,
    },
    Registered {
        key: SessionKey,
        expires_at: u64,
    },
    Refreshed {
        key: SessionKey,
        expires_at: u64,
    },
    PendingUpdateAcknowledge {
        key: SessionKey,
    },
    PendingTeardown {
        key: SessionKey,
    },
    PendingSessionUpdate {
        key: SessionKey,
    },
    PendingSessionUpdateAcknowledge {
        key: SessionKey,
    },
    SessionParametersUpdated {
        key: SessionKey,
    },
    PendingCapabilitiesInfo {
        identification: u64,
    },
    PendingCapabilitiesInfoAcknowledge {
        identification: u64,
    },
    CapabilitiesInfoCompleted {
        identification: u64,
    },
    IgnoredCapabilitiesInfoAcknowledge {
        identification: u64,
    },
    SessionUpdateExpired {
        key: SessionKey,
    },
    CapabilitiesInfoExpired {
        identification: u64,
    },
    Cleared {
        key: SessionKey,
        reason: ClearReason,
    },
    Rejected {
        key: SessionKey,
        code: u8,
    },
    Expired {
        key: SessionKey,
    },
}

/// Stateful tracker for A11 session-level procedures.
#[derive(Debug, Default)]
pub struct SessionProcedureTable {
    sessions: HashMap<SessionKey, SessionRecord>,
    outbound_capabilities: Option<PendingCapabilitiesState>,
    inbound_capabilities: Option<PendingCapabilitiesState>,
}

impl SessionProcedureTable {
    /// Creates an empty A11 procedure table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a typed A11 message to the procedure table.
    pub fn apply(
        &mut self,
        now_seconds: u64,
        direction: Direction,
        message: &Message,
    ) -> Result<ProcedureEvent> {
        match (direction, message) {
            (Direction::Outbound, Message::RegistrationRequest(message)) => {
                self.apply_outbound_request(now_seconds, message)
            }
            (Direction::Inbound, Message::RegistrationReply(message)) => {
                self.apply_inbound_reply(now_seconds, message)
            }
            (Direction::Inbound, Message::RegistrationUpdate(message)) => {
                self.apply_inbound_update(now_seconds, message)
            }
            (Direction::Outbound, Message::RegistrationAcknowledge(message)) => {
                self.apply_outbound_acknowledge(message)
            }
            (Direction::Outbound, Message::SessionUpdate(message)) => {
                self.apply_outbound_session_update(now_seconds, message)
            }
            (Direction::Inbound, Message::SessionUpdate(message)) => {
                self.apply_inbound_session_update(now_seconds, message)
            }
            (Direction::Inbound, Message::SessionUpdateAcknowledge(message)) => {
                self.apply_inbound_session_update_acknowledge(message)
            }
            (Direction::Outbound, Message::SessionUpdateAcknowledge(message)) => {
                self.apply_outbound_session_update_acknowledge(message)
            }
            (Direction::Outbound, Message::CapabilitiesInfo(message)) => {
                self.apply_outbound_capabilities_info(now_seconds, message)
            }
            (Direction::Inbound, Message::CapabilitiesInfo(message)) => {
                self.apply_inbound_capabilities_info(now_seconds, message)
            }
            (Direction::Inbound, Message::CapabilitiesInfoAcknowledge(message)) => {
                self.apply_inbound_capabilities_info_acknowledge(message)
            }
            (Direction::Outbound, Message::CapabilitiesInfoAcknowledge(message)) => {
                self.apply_outbound_capabilities_info_acknowledge(message)
            }
            (Direction::Inbound, Message::RegistrationRequest(_)) => {
                Err(Error::ProcedureViolation {
                    context: "direction",
                    reason: "registration requests must be applied as outbound messages",
                })
            }
            (Direction::Outbound, Message::RegistrationReply(_)) => {
                Err(Error::ProcedureViolation {
                    context: "direction",
                    reason: "registration replies must be applied as inbound messages",
                })
            }
            (Direction::Outbound, Message::RegistrationUpdate(_)) => {
                Err(Error::ProcedureViolation {
                    context: "direction",
                    reason: "registration updates must be applied as inbound messages",
                })
            }
            (Direction::Inbound, Message::RegistrationAcknowledge(_)) => {
                Err(Error::ProcedureViolation {
                    context: "direction",
                    reason: "registration acknowledges must be applied as outbound messages",
                })
            }
        }
    }

    /// Applies a verified typed A11 message to the procedure table.
    pub fn apply_verified(
        &mut self,
        now_seconds: u64,
        direction: Direction,
        message: &VerifiedMessage,
    ) -> Result<ProcedureEvent> {
        self.apply(now_seconds, direction, message.message())
    }

    /// Returns a snapshot for one tracked session.
    pub fn session(&self, key: SessionKey) -> Option<SessionSnapshot> {
        self.sessions.get(&key).map(SessionRecord::snapshot)
    }

    /// Returns stable snapshots of all tracked sessions.
    pub fn sessions(&self) -> Vec<SessionSnapshot> {
        let mut snapshots = self
            .sessions
            .values()
            .map(SessionRecord::snapshot)
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| {
            (
                snapshot.key.pcf_session_id,
                snapshot.key.mn_session_reference_id,
            )
        });
        snapshots
    }

    /// Expires all sessions whose granted lifetime has elapsed.
    pub fn expire_sessions(&mut self, now_seconds: u64) -> Vec<ProcedureEvent> {
        let expired = self
            .sessions
            .iter()
            .filter_map(|(key, record)| {
                let committed = record.committed.as_ref()?;
                (committed.expires_at <= now_seconds).then_some(*key)
            })
            .collect::<Vec<_>>();

        expired
            .into_iter()
            .map(|key| {
                self.sessions.remove(&key);
                ProcedureEvent::Expired { key }
            })
            .collect()
    }

    /// Expires pending Tsesupd and Tvers11 procedures.
    pub fn expire_protocol_timers(
        &mut self,
        now_seconds: u64,
        tsesupd_seconds: u64,
        tvers11_seconds: u64,
    ) -> Vec<ProcedureEvent> {
        let mut events = Vec::new();

        let expired_session_updates = self
            .sessions
            .iter()
            .filter_map(|(key, record)| match record.pending {
                Some(PendingState {
                    kind: PendingKind::LocalSessionUpdate,
                    started_at,
                    ..
                }) if started_at.saturating_add(tsesupd_seconds) <= now_seconds => Some(*key),
                _ => None,
            })
            .collect::<Vec<_>>();

        for key in expired_session_updates {
            if let Some(record) = self.sessions.get_mut(&key) {
                record.pending = None;
                events.push(ProcedureEvent::SessionUpdateExpired { key });
            }
        }

        if let Some(pending) = self.outbound_capabilities
            && pending.started_at.saturating_add(tvers11_seconds) <= now_seconds
        {
            self.outbound_capabilities = None;
            events.push(ProcedureEvent::CapabilitiesInfoExpired {
                identification: pending.identification,
            });
        }

        events
    }

    /// Clears a tracked session immediately without an on-wire teardown exchange.
    pub fn clear_session(&mut self, key: SessionKey) -> Result<ProcedureEvent> {
        if self.sessions.remove(&key).is_some() {
            Ok(ProcedureEvent::Cleared {
                key,
                reason: ClearReason::Administrative,
            })
        } else {
            Err(Error::ProcedureViolation {
                context: "session",
                reason: "cannot clear an unknown session",
            })
        }
    }

    fn apply_outbound_request(
        &mut self,
        now_seconds: u64,
        message: &RegistrationRequest,
    ) -> Result<ProcedureEvent> {
        validate_request(message)?;
        let key = SessionKey::from_session(&message.session);
        match self.sessions.get_mut(&key) {
            None => {
                if message.lifetime == 0 {
                    return Err(Error::ProcedureViolation {
                        context: "registration request",
                        reason: "cannot send a lifetime-zero request for an unknown session",
                    });
                }
                self.sessions.insert(
                    key,
                    SessionRecord {
                        session: message.session.clone(),
                        committed: None,
                        pending: Some(PendingState {
                            kind: PendingKind::Registration,
                            identification: message.identification,
                            lifetime: message.lifetime,
                            home_agent: message.home_agent,
                            care_of_address: message.care_of_address,
                            started_at: now_seconds,
                        }),
                    },
                );
                Ok(ProcedureEvent::PendingRegistration { key })
            }
            Some(record) => {
                ensure_same_session(&record.session, &message.session)?;
                if record.pending.is_some() {
                    return Err(Error::ProcedureViolation {
                        context: "registration request",
                        reason: "session already has a pending transaction",
                    });
                }
                let kind = if message.lifetime == 0 {
                    PendingKind::LocalTeardown
                } else {
                    PendingKind::Refresh
                };
                record.pending = Some(PendingState {
                    kind,
                    identification: message.identification,
                    lifetime: message.lifetime,
                    home_agent: message.home_agent,
                    care_of_address: message.care_of_address,
                    started_at: now_seconds,
                });
                Ok(if message.lifetime == 0 {
                    ProcedureEvent::PendingTeardown { key }
                } else {
                    ProcedureEvent::PendingRefresh { key }
                })
            }
        }
    }

    fn apply_inbound_reply(
        &mut self,
        now_seconds: u64,
        message: &RegistrationReply,
    ) -> Result<ProcedureEvent> {
        validate_reply(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "registration reply",
                reason: "reply received without a matching pending request",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        let Some(pending) = record.pending.take() else {
            return Err(Error::ProcedureViolation {
                context: "registration reply",
                reason: "reply received without a pending request",
            });
        };
        if pending.identification != message.identification {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "registration reply",
                reason: "reply identification does not match the pending request",
            });
        }

        match message.code {
            0 => {
                if pending.kind == PendingKind::LocalTeardown {
                    if message.lifetime != 0 {
                        record.pending = Some(pending);
                        return Err(Error::ProcedureViolation {
                            context: "registration reply",
                            reason: "accepted teardown replies must carry lifetime zero",
                        });
                    }
                    self.sessions.remove(&key);
                    return Ok(ProcedureEvent::Cleared {
                        key,
                        reason: ClearReason::LocalTeardown,
                    });
                }
                if message.lifetime == 0 {
                    record.pending = Some(pending);
                    return Err(Error::ProcedureViolation {
                        context: "registration reply",
                        reason: "accepted registration replies must grant a non-zero lifetime",
                    });
                }
                let expires_at = now_seconds + u64::from(message.lifetime);
                record.committed = Some(CommittedState {
                    identification: message.identification,
                    lifetime: message.lifetime,
                    home_agent: message.home_agent,
                    care_of_address: pending.care_of_address,
                    expires_at,
                });
                Ok(if pending.kind == PendingKind::Refresh {
                    ProcedureEvent::Refreshed { key, expires_at }
                } else {
                    ProcedureEvent::Registered { key, expires_at }
                })
            }
            rejected => {
                let had_committed = record.committed.is_some();
                if !had_committed {
                    self.sessions.remove(&key);
                }
                Ok(ProcedureEvent::Rejected {
                    key,
                    code: rejected,
                })
            }
        }
    }

    fn apply_inbound_update(
        &mut self,
        now_seconds: u64,
        message: &RegistrationUpdate,
    ) -> Result<ProcedureEvent> {
        validate_update(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "registration update",
                reason: "update received for an unknown session",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        if record.pending.is_some() {
            return Err(Error::ProcedureViolation {
                context: "registration update",
                reason: "session already has a pending transaction",
            });
        }
        let Some(committed) = record.committed.as_ref() else {
            return Err(Error::ProcedureViolation {
                context: "registration update",
                reason: "update received before the session was established",
            });
        };
        if committed.identification != message.identification {
            return Err(Error::ProcedureViolation {
                context: "registration update",
                reason: "update identification does not match the active session",
            });
        }
        if committed.home_agent != message.home_agent {
            return Err(Error::ProcedureViolation {
                context: "registration update",
                reason: "update home agent does not match the active session",
            });
        }
        record.pending = Some(PendingState {
            kind: PendingKind::RemoteUpdate,
            identification: message.identification,
            lifetime: committed.lifetime,
            home_agent: message.home_agent,
            care_of_address: committed.care_of_address,
            started_at: now_seconds,
        });
        Ok(ProcedureEvent::PendingTeardown { key })
    }

    fn apply_outbound_acknowledge(
        &mut self,
        message: &RegistrationAcknowledge,
    ) -> Result<ProcedureEvent> {
        validate_acknowledge(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "registration acknowledge",
                reason: "acknowledge sent for an unknown session",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        let Some(pending) = record.pending.take() else {
            return Err(Error::ProcedureViolation {
                context: "registration acknowledge",
                reason: "acknowledge sent without a pending update",
            });
        };
        if pending.kind != PendingKind::RemoteUpdate {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "registration acknowledge",
                reason: "acknowledge does not match the current pending transaction",
            });
        }
        if pending.identification != message.identification {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "registration acknowledge",
                reason: "acknowledge identification does not match the pending update",
            });
        }
        if pending.care_of_address != message.care_of_address {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "registration acknowledge",
                reason: "acknowledge care-of-address does not match the active session",
            });
        }
        match message.status {
            0 => {
                self.sessions.remove(&key);
                Ok(ProcedureEvent::Cleared {
                    key,
                    reason: ClearReason::RemoteTeardown,
                })
            }
            rejected => Ok(ProcedureEvent::Rejected {
                key,
                code: rejected,
            }),
        }
    }

    fn apply_outbound_session_update(
        &mut self,
        now_seconds: u64,
        message: &SessionUpdate,
    ) -> Result<ProcedureEvent> {
        validate_session_update(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "cannot send a session update for an unknown session",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        if record.pending.is_some() {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "session already has a pending transaction",
            });
        }
        let Some(committed) = record.committed else {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "cannot send a session update before registration completes",
            });
        };
        if committed.home_agent != message.home_agent {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "session update home agent does not match the active session",
            });
        }
        record.pending = Some(PendingState {
            kind: PendingKind::LocalSessionUpdate,
            identification: message.identification,
            lifetime: committed.lifetime,
            home_agent: message.home_agent,
            care_of_address: committed.care_of_address,
            started_at: now_seconds,
        });
        Ok(ProcedureEvent::PendingSessionUpdate { key })
    }

    fn apply_inbound_session_update(
        &mut self,
        now_seconds: u64,
        message: &SessionUpdate,
    ) -> Result<ProcedureEvent> {
        validate_session_update(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "session update received for an unknown session",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        if record.pending.is_some() {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "session already has a pending transaction",
            });
        }
        let Some(committed) = record.committed else {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "session update received before registration completes",
            });
        };
        if committed.home_agent != message.home_agent {
            return Err(Error::ProcedureViolation {
                context: "session update",
                reason: "session update home agent does not match the active session",
            });
        }
        record.pending = Some(PendingState {
            kind: PendingKind::RemoteSessionUpdate,
            identification: message.identification,
            lifetime: committed.lifetime,
            home_agent: message.home_agent,
            care_of_address: committed.care_of_address,
            started_at: now_seconds,
        });
        Ok(ProcedureEvent::PendingSessionUpdateAcknowledge { key })
    }

    fn apply_inbound_session_update_acknowledge(
        &mut self,
        message: &SessionUpdateAcknowledge,
    ) -> Result<ProcedureEvent> {
        validate_session_update_acknowledge(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "session update acknowledge received for an unknown session",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        let Some(pending) = record.pending.take() else {
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "session update acknowledge received without a pending session update",
            });
        };
        if pending.kind != PendingKind::LocalSessionUpdate {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "acknowledge does not match the current pending transaction",
            });
        }
        if pending.identification != message.identification {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "acknowledge identification does not match the pending session update",
            });
        }
        if pending.care_of_address != message.care_of_address {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "acknowledge care-of-address does not match the active session",
            });
        }
        if message.status == 0 {
            Ok(ProcedureEvent::SessionParametersUpdated { key })
        } else {
            Ok(ProcedureEvent::Rejected {
                key,
                code: message.status,
            })
        }
    }

    fn apply_outbound_session_update_acknowledge(
        &mut self,
        message: &SessionUpdateAcknowledge,
    ) -> Result<ProcedureEvent> {
        validate_session_update_acknowledge(message)?;
        let key = SessionKey::from_session(&message.session);
        let Some(record) = self.sessions.get_mut(&key) else {
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "session update acknowledge sent for an unknown session",
            });
        };
        ensure_same_session(&record.session, &message.session)?;
        let Some(pending) = record.pending.take() else {
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "session update acknowledge sent without a pending session update",
            });
        };
        if pending.kind != PendingKind::RemoteSessionUpdate {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "acknowledge does not match the current pending transaction",
            });
        }
        if pending.identification != message.identification {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "acknowledge identification does not match the pending session update",
            });
        }
        if pending.care_of_address != message.care_of_address {
            record.pending = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "session update acknowledge",
                reason: "acknowledge care-of-address does not match the active session",
            });
        }
        if message.status == 0 {
            Ok(ProcedureEvent::SessionParametersUpdated { key })
        } else {
            Ok(ProcedureEvent::Rejected {
                key,
                code: message.status,
            })
        }
    }

    fn apply_outbound_capabilities_info(
        &mut self,
        now_seconds: u64,
        message: &CapabilitiesInfo,
    ) -> Result<ProcedureEvent> {
        validate_capabilities_info(message)?;
        if self.outbound_capabilities.is_some() {
            return Err(Error::ProcedureViolation {
                context: "capabilities info",
                reason: "a capabilities-info procedure is already pending",
            });
        }
        self.outbound_capabilities = Some(PendingCapabilitiesState {
            identification: message.identification,
            feature_class: feature_class_from_nvses(&message.nvses),
            started_at: now_seconds,
        });
        Ok(ProcedureEvent::PendingCapabilitiesInfo {
            identification: message.identification,
        })
    }

    fn apply_inbound_capabilities_info(
        &mut self,
        now_seconds: u64,
        message: &CapabilitiesInfo,
    ) -> Result<ProcedureEvent> {
        validate_capabilities_info(message)?;
        if self.inbound_capabilities.is_some() {
            return Err(Error::ProcedureViolation {
                context: "capabilities info",
                reason: "a capabilities-info acknowledgement is already pending",
            });
        }
        self.inbound_capabilities = Some(PendingCapabilitiesState {
            identification: message.identification,
            feature_class: feature_class_from_nvses(&message.nvses),
            started_at: now_seconds,
        });
        Ok(ProcedureEvent::PendingCapabilitiesInfoAcknowledge {
            identification: message.identification,
        })
    }

    fn apply_inbound_capabilities_info_acknowledge(
        &mut self,
        message: &CapabilitiesInfoAcknowledge,
    ) -> Result<ProcedureEvent> {
        validate_capabilities_info_ack(message)?;
        let Some(pending) = self.outbound_capabilities.take() else {
            return Ok(ProcedureEvent::IgnoredCapabilitiesInfoAcknowledge {
                identification: message.identification,
            });
        };
        if pending.identification != message.identification {
            self.outbound_capabilities = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "capabilities info acknowledge",
                reason: "acknowledge identification does not match the pending capabilities-info request",
            });
        }
        validate_capabilities_response_class(pending.feature_class, &message.nvses)?;
        Ok(ProcedureEvent::CapabilitiesInfoCompleted {
            identification: message.identification,
        })
    }

    fn apply_outbound_capabilities_info_acknowledge(
        &mut self,
        message: &CapabilitiesInfoAcknowledge,
    ) -> Result<ProcedureEvent> {
        validate_capabilities_info_ack(message)?;
        let Some(pending) = self.inbound_capabilities.take() else {
            return Err(Error::ProcedureViolation {
                context: "capabilities info acknowledge",
                reason: "capabilities info acknowledge sent without a pending capabilities-info request",
            });
        };
        if pending.identification != message.identification {
            self.inbound_capabilities = Some(pending);
            return Err(Error::ProcedureViolation {
                context: "capabilities info acknowledge",
                reason: "acknowledge identification does not match the pending capabilities-info request",
            });
        }
        validate_capabilities_response_class(pending.feature_class, &message.nvses)?;
        Ok(ProcedureEvent::CapabilitiesInfoCompleted {
            identification: message.identification,
        })
    }
}

#[derive(Debug)]
struct SessionRecord {
    session: SessionSpecificExtension,
    committed: Option<CommittedState>,
    pending: Option<PendingState>,
}

impl SessionRecord {
    fn snapshot(&self) -> SessionSnapshot {
        let key = SessionKey::from_session(&self.session);
        let state = match self.pending.as_ref().map(|pending| pending.kind) {
            Some(PendingKind::Registration | PendingKind::Refresh) => {
                SessionState::PendingRegistration
            }
            Some(PendingKind::LocalTeardown) => SessionState::PendingTeardown,
            Some(PendingKind::RemoteUpdate) => SessionState::PendingTeardown,
            Some(PendingKind::RemoteSessionUpdate) => SessionState::PendingSessionUpdateAcknowledge,
            Some(PendingKind::LocalSessionUpdate) => SessionState::PendingSessionUpdate,
            None => SessionState::Active,
        };
        let (home_agent, care_of_address, identification, granted_lifetime, expires_at) =
            match (&self.pending, &self.committed) {
                (Some(pending), Some(committed))
                    if matches!(
                        pending.kind,
                        PendingKind::Registration | PendingKind::Refresh
                    ) =>
                {
                    (
                        pending.home_agent,
                        pending.care_of_address,
                        pending.identification,
                        committed.lifetime,
                        Some(committed.expires_at),
                    )
                }
                (Some(pending), None) => (
                    pending.home_agent,
                    pending.care_of_address,
                    pending.identification,
                    pending.lifetime,
                    None,
                ),
                (_, Some(committed)) => (
                    committed.home_agent,
                    committed.care_of_address,
                    committed.identification,
                    committed.lifetime,
                    Some(committed.expires_at),
                ),
                (None, None) => ([0; 4], [0; 4], 0, 0, None),
            };

        SessionSnapshot {
            key,
            state,
            protocol_type: self.session.protocol_type,
            mn_id: self.session.mn_id.clone(),
            home_agent,
            care_of_address,
            identification,
            granted_lifetime,
            expires_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Registration,
    Refresh,
    LocalTeardown,
    RemoteUpdate,
    LocalSessionUpdate,
    RemoteSessionUpdate,
}

#[derive(Debug, Clone, Copy)]
struct PendingState {
    kind: PendingKind,
    identification: u64,
    lifetime: u16,
    home_agent: [u8; 4],
    care_of_address: [u8; 4],
    started_at: u64,
}

#[derive(Debug, Clone, Copy)]
struct CommittedState {
    identification: u64,
    lifetime: u16,
    home_agent: [u8; 4],
    care_of_address: [u8; 4],
    expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingCapabilitiesState {
    identification: u64,
    feature_class: CapabilitiesFeatureClass,
    started_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilitiesFeatureClass {
    UnknownOrMixed,
    PdsnFeatures,
    PcfFeatures,
}

fn feature_class_from_nvses(nvses: &[crate::Nvse]) -> CapabilitiesFeatureClass {
    let has_pdsn = nvses
        .iter()
        .any(|nvse| matches!(nvse, crate::Nvse::PdsnEnabledFeature(_)));
    let has_pcf = nvses
        .iter()
        .any(|nvse| matches!(nvse, crate::Nvse::PcfEnabledFeature(_)));
    match (has_pdsn, has_pcf) {
        (true, false) => CapabilitiesFeatureClass::PdsnFeatures,
        (false, true) => CapabilitiesFeatureClass::PcfFeatures,
        _ => CapabilitiesFeatureClass::UnknownOrMixed,
    }
}

fn validate_capabilities_response_class(
    requested: CapabilitiesFeatureClass,
    response_nvses: &[crate::Nvse],
) -> Result<()> {
    let response = feature_class_from_nvses(response_nvses);
    match requested {
        CapabilitiesFeatureClass::PdsnFeatures => {
            if !matches!(response, CapabilitiesFeatureClass::PcfFeatures) {
                return Err(Error::ProcedureViolation {
                    context: "capabilities info acknowledge",
                    reason: "acknowledge must return PCF feature capabilities for a PDSN-feature request",
                });
            }
        }
        CapabilitiesFeatureClass::PcfFeatures => {
            if !matches!(response, CapabilitiesFeatureClass::PdsnFeatures) {
                return Err(Error::ProcedureViolation {
                    context: "capabilities info acknowledge",
                    reason: "acknowledge must return PDSN feature capabilities for a PCF-feature request",
                });
            }
        }
        CapabilitiesFeatureClass::UnknownOrMixed => {}
    }
    Ok(())
}

fn ensure_same_session(
    expected: &SessionSpecificExtension,
    actual: &SessionSpecificExtension,
) -> Result<()> {
    if expected != actual {
        return Err(Error::ProcedureViolation {
            context: "session",
            reason: "message does not match the tracked session identity",
        });
    }
    Ok(())
}
