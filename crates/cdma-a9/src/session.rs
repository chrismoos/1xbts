//! A9 control-plane procedure and session state.

use std::collections::BTreeMap;

use crate::{
    A8TrafficId, AlConnectedAckMessage, AlConnectedMessage, AlDisconnectedAckMessage,
    AlDisconnectedMessage, BsServiceRequestMessage, BsServiceResponseMessage,
    CallConnectionReference, ConRef, ConnectA8Message, CorrelationId, DataCount,
    DisconnectA8Message, Error, Meid, MessageType, ReleaseA8CompleteMessage, ReleaseA8Message,
    Result, ServiceOptionValue, SetupA8Message, ShortDataAckMessage, ShortDataDeliveryMessage,
    UpdateA8AckMessage, UpdateA8Message, VersionInfoAckMessage, VersionInfoMessage,
};

/// Local endpoint role for directional procedure validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcedureRole {
    Bsc,
    Pcf,
}

/// Stable session phase for an A9 bearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    SetupPending {
        initiated_by_local: bool,
        pending: PendingRequestIdentity,
    },
    Connected,
    DisconnectPending {
        initiated_by_local: bool,
        pending: PendingRequestIdentity,
    },
    ReleasePending {
        initiated_by_local: bool,
        pending: PendingRequestIdentity,
    },
}

/// Stable access-link sub-phase for a connected bearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLinkPhase {
    Disconnected,
    ConnectPending {
        initiated_by_local: bool,
        pending: PendingRequestIdentity,
    },
    Connected,
    DisconnectPending {
        initiated_by_local: bool,
        pending: PendingRequestIdentity,
    },
}

/// Stable identifier set for matching A9 request and response messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingRequestIdentity {
    /// Optional call-connection reference carried with the request.
    pub call_connection_reference: Option<CallConnectionReference>,
    /// Optional correlation identifier carried with the request.
    pub correlation_id: Option<CorrelationId>,
}

/// Stable BS service request state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BsServicePhase {
    Idle,
    RequestPending {
        request: BsServiceRequestState,
        initiated_by_local: bool,
    },
}

/// Stable version-information procedure state guarded by `Tvers9`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionInfoPhase {
    Idle,
    RequestPending {
        request: VersionInfoRequestState,
        initiated_by_local: bool,
    },
}

/// Stable contents of a pending `A9-Version Info` procedure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfoRequestState {
    /// Optional correlation identifier carried with the request.
    pub correlation_id: Option<CorrelationId>,
}

/// Stable per-session update procedure state guarded by `Tupd9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionUpdatePhase {
    Idle,
    RequestPending {
        pending: PendingRequestIdentity,
        initiated_by_local: bool,
    },
}

/// Stable short-data-delivery procedure state guarded by `Tsdd9`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortDataPhase {
    Idle,
    DeliveryPending {
        request: ShortDataRequestState,
        initiated_by_local: bool,
    },
}

/// Stable contents of a pending short-data-delivery request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortDataRequestState {
    /// Optional correlation identifier carried with the request.
    pub correlation_id: Option<CorrelationId>,
    /// Optional IMSI carried with the request.
    pub imsi: Option<String>,
    /// Optional ESN carried with the request.
    pub esn: Option<u32>,
    /// Optional MEID carried with the request.
    pub meid: Option<Meid>,
}

/// Stable contents of a pending A9 BS service request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsServiceRequestState {
    /// Optional correlation identifier carried with the request.
    pub correlation_id: Option<CorrelationId>,
    /// Optional IMSI carried with the request.
    pub imsi: Option<String>,
    /// Optional ESN carried with the request.
    pub esn: Option<u32>,
    /// Optional MEID carried with the request.
    pub meid: Option<Meid>,
    /// Service option requested for the BS service procedure.
    pub service_option: ServiceOptionValue,
    /// Data count carried with the BS service request.
    pub data_count: DataCount,
}

/// Typed procedure message surface used by the session engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcedureMessage {
    SetupA8(SetupA8Message),
    ConnectA8(ConnectA8Message),
    DisconnectA8(DisconnectA8Message),
    ReleaseA8(ReleaseA8Message),
    ReleaseA8Complete(ReleaseA8CompleteMessage),
    BsServiceRequest(BsServiceRequestMessage),
    BsServiceResponse(BsServiceResponseMessage),
    AlConnected(AlConnectedMessage),
    AlConnectedAck(AlConnectedAckMessage),
    AlDisconnected(AlDisconnectedMessage),
    AlDisconnectedAck(AlDisconnectedAckMessage),
    VersionInfo(VersionInfoMessage),
    VersionInfoAck(VersionInfoAckMessage),
    UpdateA8(UpdateA8Message),
    UpdateA8Ack(UpdateA8AckMessage),
    ShortDataDelivery(ShortDataDeliveryMessage),
    ShortDataAck(ShortDataAckMessage),
}

impl ProcedureMessage {
    fn message_type(&self) -> MessageType {
        match self {
            Self::SetupA8(_) => MessageType::SetupA8,
            Self::ConnectA8(_) => MessageType::ConnectA8,
            Self::DisconnectA8(_) => MessageType::DisconnectA8,
            Self::ReleaseA8(_) => MessageType::ReleaseA8,
            Self::ReleaseA8Complete(_) => MessageType::ReleaseA8Complete,
            Self::BsServiceRequest(_) => MessageType::BsServiceRequest,
            Self::BsServiceResponse(_) => MessageType::BsServiceResponse,
            Self::AlConnected(_) => MessageType::AlConnected,
            Self::AlConnectedAck(_) => MessageType::AlConnectedAck,
            Self::AlDisconnected(_) => MessageType::AlDisconnected,
            Self::AlDisconnectedAck(_) => MessageType::AlDisconnectedAck,
            Self::VersionInfo(_) => MessageType::VersionInfo,
            Self::VersionInfoAck(_) => MessageType::VersionInfoAck,
            Self::UpdateA8(_) => MessageType::UpdateA8,
            Self::UpdateA8Ack(_) => MessageType::UpdateA8Ack,
            Self::ShortDataDelivery(_) => MessageType::ShortDataDelivery,
            Self::ShortDataAck(_) => MessageType::ShortDataAck,
        }
    }
}

/// Observable result of applying a procedure message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcedureEvent {
    SessionCreated {
        con_ref: Vec<u8>,
        phase: SessionPhase,
    },
    SessionUpdated {
        con_ref: Vec<u8>,
        phase: SessionPhase,
        access_link_phase: AccessLinkPhase,
    },
    SessionReleased {
        con_ref: Vec<u8>,
    },
    BsServiceUpdated(BsServicePhase),
    VersionInfoUpdated(VersionInfoPhase),
    SessionUpdateUpdated {
        con_ref: Vec<u8>,
        phase: SessionUpdatePhase,
    },
    ShortDataUpdated(ShortDataPhase),
    Ignored {
        message_type: MessageType,
    },
}

/// Tracked A9 session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    con_ref: ConRef,
    call_connection_reference: Option<CallConnectionReference>,
    correlation_id: Option<CorrelationId>,
    a8_traffic_id: A8TrafficId,
    phase: SessionPhase,
    access_link_phase: AccessLinkPhase,
    session_update_phase: SessionUpdatePhase,
}

impl Session {
    /// Returns the current bearer phase.
    pub const fn phase(&self) -> SessionPhase {
        self.phase
    }

    /// Returns the current access-link phase.
    pub const fn access_link_phase(&self) -> AccessLinkPhase {
        self.access_link_phase
    }

    /// Returns the current A9 session-update sub-phase.
    pub const fn session_update_phase(&self) -> SessionUpdatePhase {
        self.session_update_phase
    }

    fn con_ref(&self) -> &ConRef {
        &self.con_ref
    }

    fn traffic_id(&self) -> &A8TrafficId {
        &self.a8_traffic_id
    }
}

/// Stateful A9 procedure engine.
#[derive(Debug)]
pub struct ProcedureEngine {
    role: ProcedureRole,
    sessions: BTreeMap<Vec<u8>, Session>,
    traffic_index: BTreeMap<Vec<u8>, Vec<u8>>,
    call_ref_index: BTreeMap<[u8; 8], Vec<u8>>,
    correlation_index: BTreeMap<[u8; 4], Vec<u8>>,
    bs_service_phase: BsServicePhase,
    version_info_phase: VersionInfoPhase,
    short_data_phase: ShortDataPhase,
}

impl ProcedureEngine {
    /// Creates an empty procedure engine for the given role.
    pub fn new(role: ProcedureRole) -> Self {
        Self {
            role,
            sessions: BTreeMap::new(),
            traffic_index: BTreeMap::new(),
            call_ref_index: BTreeMap::new(),
            correlation_index: BTreeMap::new(),
            bs_service_phase: BsServicePhase::Idle,
            version_info_phase: VersionInfoPhase::Idle,
            short_data_phase: ShortDataPhase::Idle,
        }
    }

    /// Applies an outbound A9 procedure message.
    pub fn apply_outbound(&mut self, message: ProcedureMessage) -> Result<ProcedureEvent> {
        self.apply(message, true)
    }

    /// Applies an inbound A9 procedure message.
    pub fn apply_inbound(&mut self, message: ProcedureMessage) -> Result<ProcedureEvent> {
        self.apply(message, false)
    }

    /// Returns a tracked session by connection reference bytes.
    pub fn session(&self, con_ref: &[u8]) -> Option<&Session> {
        self.sessions.get(con_ref)
    }

    /// Returns a tracked session by `A8_Traffic_ID`.
    pub fn session_by_traffic_id(&self, traffic_id: &A8TrafficId) -> Option<&Session> {
        let key = traffic_key(traffic_id);
        let con_ref = self.traffic_index.get(&key)?;
        self.sessions.get(con_ref)
    }

    /// Returns the BS service phase.
    pub fn bs_service_phase(&self) -> &BsServicePhase {
        &self.bs_service_phase
    }

    /// Returns the version-information procedure phase.
    pub fn version_info_phase(&self) -> &VersionInfoPhase {
        &self.version_info_phase
    }

    /// Returns the short-data-delivery procedure phase.
    pub fn short_data_phase(&self) -> &ShortDataPhase {
        &self.short_data_phase
    }

    fn apply(
        &mut self,
        message: ProcedureMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        validate_direction(self.role, message.message_type(), initiated_by_local)?;
        match message {
            ProcedureMessage::SetupA8(message) => self.apply_setup(message, initiated_by_local),
            ProcedureMessage::ConnectA8(message) => self.apply_connect(message, initiated_by_local),
            ProcedureMessage::DisconnectA8(message) => {
                self.apply_disconnect(message, initiated_by_local)
            }
            ProcedureMessage::ReleaseA8(message) => self.apply_release(message, initiated_by_local),
            ProcedureMessage::ReleaseA8Complete(message) => self.apply_release_complete(message),
            ProcedureMessage::BsServiceRequest(message) => {
                self.apply_bs_service_request(message, initiated_by_local)
            }
            ProcedureMessage::BsServiceResponse(message) => {
                self.apply_bs_service_response(message, initiated_by_local)
            }
            ProcedureMessage::AlConnected(message) => {
                self.apply_al_connected(message, initiated_by_local)
            }
            ProcedureMessage::AlConnectedAck(message) => {
                self.apply_al_connected_ack(message, initiated_by_local)
            }
            ProcedureMessage::AlDisconnected(message) => {
                self.apply_al_disconnected(message, initiated_by_local)
            }
            ProcedureMessage::AlDisconnectedAck(message) => {
                self.apply_al_disconnected_ack(message, initiated_by_local)
            }
            ProcedureMessage::VersionInfo(message) => {
                self.apply_version_info(message, initiated_by_local)
            }
            ProcedureMessage::VersionInfoAck(message) => {
                self.apply_version_info_ack(message, initiated_by_local)
            }
            ProcedureMessage::UpdateA8(message) => {
                self.apply_update_a8(message, initiated_by_local)
            }
            ProcedureMessage::UpdateA8Ack(message) => {
                self.apply_update_a8_ack(message, initiated_by_local)
            }
            ProcedureMessage::ShortDataDelivery(message) => {
                self.apply_short_data_delivery(message, initiated_by_local)
            }
            ProcedureMessage::ShortDataAck(message) => {
                self.apply_short_data_ack(message, initiated_by_local)
            }
        }
    }

    fn apply_setup(
        &mut self,
        message: SetupA8Message,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let con_ref = con_ref_key(&message.con_ref);
        if self.sessions.contains_key(&con_ref) {
            return Err(Error::DuplicateSession);
        }
        let traffic_key = traffic_key(&message.a8_traffic_id);
        if self.traffic_index.contains_key(&traffic_key) {
            return Err(Error::DuplicateTrafficId(message.a8_traffic_id.key));
        }

        let session = Session {
            con_ref: message.con_ref,
            call_connection_reference: message.call_connection_reference,
            correlation_id: message.correlation_id,
            a8_traffic_id: message.a8_traffic_id,
            phase: SessionPhase::SetupPending {
                initiated_by_local,
                pending: PendingRequestIdentity {
                    call_connection_reference: message.call_connection_reference,
                    correlation_id: message.correlation_id,
                },
            },
            access_link_phase: AccessLinkPhase::Disconnected,
            session_update_phase: SessionUpdatePhase::Idle,
        };
        self.insert_indexes(&con_ref, &session);
        self.sessions.insert(con_ref.clone(), session.clone());
        Ok(ProcedureEvent::SessionCreated {
            con_ref,
            phase: session.phase,
        })
    }

    fn apply_connect(
        &mut self,
        message: ConnectA8Message,
        _initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let (con_ref, phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self.session_mut_by_con_ref(&message.con_ref)?;
            ensure_matching_traffic_id(session, &message.a8_traffic_id)?;
            let pending = match session.phase {
                SessionPhase::SetupPending { pending, .. } => pending,
                _ => {
                    return Err(Error::InvalidProcedureState {
                        message_type: MessageType::ConnectA8,
                        state: "session is not awaiting connect",
                    });
                }
            };
            ensure_response_identity(
                pending,
                message.call_connection_reference,
                message.correlation_id,
                MessageType::ConnectA8,
                "connect",
            )?;
            if let Some(call_connection_reference) = message.call_connection_reference {
                ensure_call_connection_reference(
                    session,
                    call_connection_reference,
                    MessageType::ConnectA8,
                )?;
            }
            if let Some(correlation_id) = message.correlation_id {
                ensure_correlation_id(session, correlation_id, MessageType::ConnectA8)?;
            }
            session.phase = SessionPhase::Connected;
            if message.call_connection_reference.is_some() {
                session.call_connection_reference = message.call_connection_reference;
            }
            if message.correlation_id.is_some() {
                session.correlation_id = message.correlation_id;
            }
            (
                con_ref_key(session.con_ref()),
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_disconnect(
        &mut self,
        message: DisconnectA8Message,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let (con_ref, phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self.session_mut_by_con_ref(&message.con_ref)?;
            ensure_matching_traffic_id(session, &message.a8_traffic_id)?;
            if !matches!(session.phase, SessionPhase::Connected) {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::DisconnectA8,
                    state: "session is not connected",
                });
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                ensure_call_connection_reference(
                    session,
                    call_connection_reference,
                    MessageType::DisconnectA8,
                )?;
            }
            if let Some(correlation_id) = message.correlation_id {
                ensure_correlation_id(session, correlation_id, MessageType::DisconnectA8)?;
            }
            session.phase = SessionPhase::DisconnectPending {
                initiated_by_local,
                pending: PendingRequestIdentity {
                    call_connection_reference: message.call_connection_reference,
                    correlation_id: message.correlation_id,
                },
            };
            if message.call_connection_reference.is_some() {
                session.call_connection_reference = message.call_connection_reference;
            }
            if message.correlation_id.is_some() {
                session.correlation_id = message.correlation_id;
            }
            (
                con_ref_key(session.con_ref()),
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_release(
        &mut self,
        message: ReleaseA8Message,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let (con_ref, phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self.session_mut_by_con_ref(&message.con_ref)?;
            ensure_matching_traffic_id(session, &message.a8_traffic_id)?;
            if matches!(session.phase, SessionPhase::ReleasePending { .. }) {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::ReleaseA8,
                    state: "session is already releasing",
                });
            }
            if let SessionPhase::DisconnectPending { pending, .. } = session.phase {
                ensure_release_identity_against_disconnect(
                    pending,
                    message.correlation_id,
                    MessageType::ReleaseA8,
                )?;
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                ensure_call_connection_reference(
                    session,
                    call_connection_reference,
                    MessageType::ReleaseA8,
                )?;
            }
            if let Some(correlation_id) = message.correlation_id {
                ensure_correlation_id(session, correlation_id, MessageType::ReleaseA8)?;
            }
            session.phase = SessionPhase::ReleasePending {
                initiated_by_local,
                pending: PendingRequestIdentity {
                    call_connection_reference: message.call_connection_reference,
                    correlation_id: message.correlation_id,
                },
            };
            if message.call_connection_reference.is_some() {
                session.call_connection_reference = message.call_connection_reference;
            }
            if message.correlation_id.is_some() {
                session.correlation_id = message.correlation_id;
            }
            (
                con_ref_key(session.con_ref()),
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_release_complete(
        &mut self,
        message: ReleaseA8CompleteMessage,
    ) -> Result<ProcedureEvent> {
        let con_ref = self.lookup_con_ref_by_response(
            message.call_connection_reference,
            message.correlation_id,
            MessageType::ReleaseA8Complete,
        )?;
        let session = self.sessions.get(&con_ref).ok_or(Error::UnknownSession)?;
        let pending = match session.phase {
            SessionPhase::ReleasePending { pending, .. } => pending,
            _ => {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::ReleaseA8Complete,
                    state: "session is not awaiting release complete",
                });
            }
        };
        ensure_response_identity(
            pending,
            message.call_connection_reference,
            message.correlation_id,
            MessageType::ReleaseA8Complete,
            "release complete",
        )?;
        let session = self
            .sessions
            .remove(&con_ref)
            .ok_or(Error::UnknownSession)?;
        self.remove_indexes(&con_ref, &session);
        Ok(ProcedureEvent::SessionReleased { con_ref })
    }

    fn apply_bs_service_request(
        &mut self,
        message: BsServiceRequestMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        match self.bs_service_phase {
            BsServicePhase::Idle => {
                self.bs_service_phase = BsServicePhase::RequestPending {
                    request: BsServiceRequestState {
                        correlation_id: message.correlation_id,
                        imsi: message.imsi,
                        esn: message.esn,
                        meid: message.meid,
                        service_option: message.service_option,
                        data_count: message.data_count,
                    },
                    initiated_by_local,
                };
                Ok(ProcedureEvent::BsServiceUpdated(
                    self.bs_service_phase.clone(),
                ))
            }
            BsServicePhase::RequestPending { .. } => Err(Error::InvalidProcedureState {
                message_type: MessageType::BsServiceRequest,
                state: "BS service request already pending",
            }),
        }
    }

    fn apply_bs_service_response(
        &mut self,
        message: BsServiceResponseMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        match self.bs_service_phase {
            BsServicePhase::Idle => Err(Error::InvalidProcedureState {
                message_type: MessageType::BsServiceResponse,
                state: "no BS service request is pending",
            }),
            BsServicePhase::RequestPending {
                ref request,
                initiated_by_local: request_initiated_by_local,
            } => {
                ensure_response_direction(
                    request_initiated_by_local,
                    initiated_by_local,
                    MessageType::BsServiceResponse,
                    "BS service response direction does not match the pending request",
                )?;
                ensure_optional_response_correlation(
                    request.correlation_id,
                    message.correlation_id,
                    MessageType::BsServiceResponse,
                    "BS service response",
                )?;
                self.bs_service_phase = BsServicePhase::Idle;
                Ok(ProcedureEvent::BsServiceUpdated(
                    self.bs_service_phase.clone(),
                ))
            }
        }
    }

    fn apply_al_connected(
        &mut self,
        message: AlConnectedMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let (con_ref, phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self.session_mut_by_traffic_id(&message.a8_traffic_id)?;
            if !matches!(session.phase, SessionPhase::Connected) {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::AlConnected,
                    state: "session is not connected",
                });
            }
            if !matches!(session.access_link_phase, AccessLinkPhase::Disconnected) {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::AlConnected,
                    state: "access link is not disconnected",
                });
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                ensure_call_connection_reference(
                    session,
                    call_connection_reference,
                    MessageType::AlConnected,
                )?;
            }
            if let Some(correlation_id) = message.correlation_id {
                ensure_correlation_id(session, correlation_id, MessageType::AlConnected)?;
            }
            session.access_link_phase = AccessLinkPhase::ConnectPending {
                initiated_by_local,
                pending: PendingRequestIdentity {
                    call_connection_reference: message.call_connection_reference,
                    correlation_id: message.correlation_id,
                },
            };
            if message.call_connection_reference.is_some() {
                session.call_connection_reference = message.call_connection_reference;
            }
            if message.correlation_id.is_some() {
                session.correlation_id = message.correlation_id;
            }
            (
                con_ref_key(session.con_ref()),
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_al_connected_ack(
        &mut self,
        message: AlConnectedAckMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let con_ref = self.lookup_con_ref_by_response(
            message.call_connection_reference,
            message.correlation_id,
            MessageType::AlConnectedAck,
        )?;
        let (phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self
                .sessions
                .get_mut(&con_ref)
                .ok_or(Error::UnknownSession)?;
            match session.access_link_phase {
                AccessLinkPhase::ConnectPending {
                    pending,
                    initiated_by_local: request_initiated_by_local,
                } => {
                    ensure_response_direction(
                        request_initiated_by_local,
                        initiated_by_local,
                        MessageType::AlConnectedAck,
                        "access link connect acknowledgement direction does not match the pending request",
                    )?;
                    ensure_response_identity(
                        pending,
                        message.call_connection_reference,
                        message.correlation_id,
                        MessageType::AlConnectedAck,
                        "access link connect acknowledgement",
                    )?;
                    session.access_link_phase = AccessLinkPhase::Connected;
                }
                _ => {
                    return Err(Error::InvalidProcedureState {
                        message_type: MessageType::AlConnectedAck,
                        state: "access link is not awaiting connect acknowledgement",
                    });
                }
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                session.call_connection_reference = Some(call_connection_reference);
            }
            if let Some(correlation_id) = message.correlation_id {
                session.correlation_id = Some(correlation_id);
            }
            (
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_al_disconnected(
        &mut self,
        message: AlDisconnectedMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let (con_ref, phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self.session_mut_by_traffic_id(&message.a8_traffic_id)?;
            if !matches!(session.access_link_phase, AccessLinkPhase::Connected) {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::AlDisconnected,
                    state: "access link is not connected",
                });
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                ensure_call_connection_reference(
                    session,
                    call_connection_reference,
                    MessageType::AlDisconnected,
                )?;
            }
            if let Some(correlation_id) = message.correlation_id {
                ensure_correlation_id(session, correlation_id, MessageType::AlDisconnected)?;
            }
            session.access_link_phase = AccessLinkPhase::DisconnectPending {
                initiated_by_local,
                pending: PendingRequestIdentity {
                    call_connection_reference: message.call_connection_reference,
                    correlation_id: message.correlation_id,
                },
            };
            if message.call_connection_reference.is_some() {
                session.call_connection_reference = message.call_connection_reference;
            }
            if message.correlation_id.is_some() {
                session.correlation_id = message.correlation_id;
            }
            (
                con_ref_key(session.con_ref()),
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_al_disconnected_ack(
        &mut self,
        message: AlDisconnectedAckMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let con_ref = self.lookup_con_ref_by_response(
            message.call_connection_reference,
            message.correlation_id,
            MessageType::AlDisconnectedAck,
        )?;
        let (phase, access_link_phase, call_connection_reference, correlation_id) = {
            let session = self
                .sessions
                .get_mut(&con_ref)
                .ok_or(Error::UnknownSession)?;
            match session.access_link_phase {
                AccessLinkPhase::DisconnectPending {
                    pending,
                    initiated_by_local: request_initiated_by_local,
                } => {
                    ensure_response_direction(
                        request_initiated_by_local,
                        initiated_by_local,
                        MessageType::AlDisconnectedAck,
                        "access link disconnect acknowledgement direction does not match the pending request",
                    )?;
                    ensure_response_identity(
                        pending,
                        message.call_connection_reference,
                        message.correlation_id,
                        MessageType::AlDisconnectedAck,
                        "access link disconnect acknowledgement",
                    )?;
                    session.access_link_phase = AccessLinkPhase::Disconnected;
                }
                _ => {
                    return Err(Error::InvalidProcedureState {
                        message_type: MessageType::AlDisconnectedAck,
                        state: "access link is not awaiting disconnect acknowledgement",
                    });
                }
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                session.call_connection_reference = Some(call_connection_reference);
            }
            if let Some(correlation_id) = message.correlation_id {
                session.correlation_id = Some(correlation_id);
            }
            (
                session.phase,
                session.access_link_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdated {
            con_ref,
            phase,
            access_link_phase,
        })
    }

    fn apply_version_info(
        &mut self,
        message: VersionInfoMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        match self.version_info_phase {
            VersionInfoPhase::Idle => {
                self.version_info_phase = VersionInfoPhase::RequestPending {
                    request: VersionInfoRequestState {
                        correlation_id: message.correlation_id,
                    },
                    initiated_by_local,
                };
                Ok(ProcedureEvent::VersionInfoUpdated(
                    self.version_info_phase.clone(),
                ))
            }
            VersionInfoPhase::RequestPending { .. } => Err(Error::InvalidProcedureState {
                message_type: MessageType::VersionInfo,
                state: "version information request already pending",
            }),
        }
    }

    fn apply_version_info_ack(
        &mut self,
        message: VersionInfoAckMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        match self.version_info_phase.clone() {
            VersionInfoPhase::Idle => Ok(ProcedureEvent::Ignored {
                message_type: MessageType::VersionInfoAck,
            }),
            VersionInfoPhase::RequestPending {
                request,
                initiated_by_local: request_initiated_by_local,
            } => {
                ensure_response_direction(
                    request_initiated_by_local,
                    initiated_by_local,
                    MessageType::VersionInfoAck,
                    "version information acknowledgement direction does not match the pending request",
                )?;
                ensure_optional_response_correlation(
                    request.correlation_id,
                    message.correlation_id,
                    MessageType::VersionInfoAck,
                    "version information acknowledgement",
                )?;
                self.version_info_phase = VersionInfoPhase::Idle;
                Ok(ProcedureEvent::VersionInfoUpdated(
                    self.version_info_phase.clone(),
                ))
            }
        }
    }

    fn apply_update_a8(
        &mut self,
        message: UpdateA8Message,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let con_ref = self.lookup_con_ref_by_identity(
            message.call_connection_reference,
            message.correlation_id,
            MessageType::UpdateA8,
        )?;
        let (phase, call_connection_reference, correlation_id) = {
            let session = self
                .sessions
                .get_mut(&con_ref)
                .ok_or(Error::UnknownSession)?;
            if !matches!(session.session_update_phase, SessionUpdatePhase::Idle) {
                return Err(Error::InvalidProcedureState {
                    message_type: MessageType::UpdateA8,
                    state: "session update request already pending",
                });
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                ensure_call_connection_reference(
                    session,
                    call_connection_reference,
                    MessageType::UpdateA8,
                )?;
            }
            if let Some(correlation_id) = message.correlation_id {
                ensure_correlation_id(session, correlation_id, MessageType::UpdateA8)?;
            }
            session.session_update_phase = SessionUpdatePhase::RequestPending {
                pending: PendingRequestIdentity {
                    call_connection_reference: message.call_connection_reference,
                    correlation_id: message.correlation_id,
                },
                initiated_by_local,
            };
            if message.call_connection_reference.is_some() {
                session.call_connection_reference = message.call_connection_reference;
            }
            if message.correlation_id.is_some() {
                session.correlation_id = message.correlation_id;
            }
            (
                session.session_update_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdateUpdated { con_ref, phase })
    }

    fn apply_update_a8_ack(
        &mut self,
        message: UpdateA8AckMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        let con_ref = self.lookup_con_ref_by_response(
            message.call_connection_reference,
            message.correlation_id,
            MessageType::UpdateA8Ack,
        )?;
        let (phase, call_connection_reference, correlation_id) = {
            let session = self
                .sessions
                .get_mut(&con_ref)
                .ok_or(Error::UnknownSession)?;
            match session.session_update_phase {
                SessionUpdatePhase::RequestPending {
                    pending,
                    initiated_by_local: request_initiated_by_local,
                } => {
                    ensure_response_direction(
                        request_initiated_by_local,
                        initiated_by_local,
                        MessageType::UpdateA8Ack,
                        "session update acknowledgement direction does not match the pending request",
                    )?;
                    ensure_response_identity(
                        pending,
                        message.call_connection_reference,
                        message.correlation_id,
                        MessageType::UpdateA8Ack,
                        "session update acknowledgement",
                    )?;
                    session.session_update_phase = SessionUpdatePhase::Idle;
                }
                SessionUpdatePhase::Idle => {
                    return Err(Error::InvalidProcedureState {
                        message_type: MessageType::UpdateA8Ack,
                        state: "session is not awaiting update acknowledgement",
                    });
                }
            }
            if let Some(call_connection_reference) = message.call_connection_reference {
                session.call_connection_reference = Some(call_connection_reference);
            }
            if let Some(correlation_id) = message.correlation_id {
                session.correlation_id = Some(correlation_id);
            }
            (
                session.session_update_phase,
                session.call_connection_reference,
                session.correlation_id,
            )
        };
        self.refresh_indexes_for_refs(&con_ref, call_connection_reference, correlation_id);
        Ok(ProcedureEvent::SessionUpdateUpdated { con_ref, phase })
    }

    fn apply_short_data_delivery(
        &mut self,
        message: ShortDataDeliveryMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        if !short_data_requires_ack(self.role, initiated_by_local) {
            return Ok(ProcedureEvent::ShortDataUpdated(
                self.short_data_phase.clone(),
            ));
        }
        match self.short_data_phase {
            ShortDataPhase::Idle => {
                self.short_data_phase = ShortDataPhase::DeliveryPending {
                    request: ShortDataRequestState {
                        correlation_id: message.correlation_id,
                        imsi: message.imsi,
                        esn: message.esn,
                        meid: message.meid,
                    },
                    initiated_by_local,
                };
                Ok(ProcedureEvent::ShortDataUpdated(
                    self.short_data_phase.clone(),
                ))
            }
            ShortDataPhase::DeliveryPending { .. } => Err(Error::InvalidProcedureState {
                message_type: MessageType::ShortDataDelivery,
                state: "short data delivery request already pending",
            }),
        }
    }

    fn apply_short_data_ack(
        &mut self,
        message: ShortDataAckMessage,
        initiated_by_local: bool,
    ) -> Result<ProcedureEvent> {
        match self.short_data_phase.clone() {
            ShortDataPhase::Idle => Err(Error::InvalidProcedureState {
                message_type: MessageType::ShortDataAck,
                state: "no short data delivery request is pending",
            }),
            ShortDataPhase::DeliveryPending {
                request,
                initiated_by_local: request_initiated_by_local,
            } => {
                ensure_response_direction(
                    request_initiated_by_local,
                    initiated_by_local,
                    MessageType::ShortDataAck,
                    "short data acknowledgement direction does not match the pending request",
                )?;
                ensure_optional_response_correlation(
                    request.correlation_id,
                    message.correlation_id,
                    MessageType::ShortDataAck,
                    "short data acknowledgement",
                )?;
                ensure_optional_identity_match(
                    request.imsi.as_deref(),
                    message.imsi.as_deref(),
                    MessageType::ShortDataAck,
                    "short data acknowledgement IMSI does not match the pending request",
                    "short data acknowledgement IMSI is present without a pending request IMSI",
                )?;
                ensure_optional_scalar_match(
                    request.esn,
                    message.esn,
                    MessageType::ShortDataAck,
                    "short data acknowledgement ESN does not match the pending request",
                    "short data acknowledgement ESN is present without a pending request ESN",
                )?;
                ensure_optional_scalar_match(
                    request.meid,
                    message.meid,
                    MessageType::ShortDataAck,
                    "short data acknowledgement MEID does not match the pending request",
                    "short data acknowledgement MEID is present without a pending request MEID",
                )?;
                self.short_data_phase = ShortDataPhase::Idle;
                Ok(ProcedureEvent::ShortDataUpdated(
                    self.short_data_phase.clone(),
                ))
            }
        }
    }

    fn session_mut_by_con_ref(&mut self, con_ref: &ConRef) -> Result<&mut Session> {
        self.sessions
            .get_mut(&con_ref_key(con_ref))
            .ok_or(Error::UnknownSession)
    }

    fn session_mut_by_traffic_id(&mut self, traffic_id: &A8TrafficId) -> Result<&mut Session> {
        let key = traffic_key(traffic_id);
        let con_ref = self
            .traffic_index
            .get(&key)
            .cloned()
            .ok_or(Error::UnknownTrafficId(traffic_id.key))?;
        self.sessions.get_mut(&con_ref).ok_or(Error::UnknownSession)
    }

    fn lookup_con_ref_by_response(
        &self,
        call_connection_reference: Option<CallConnectionReference>,
        correlation_id: Option<CorrelationId>,
        message_type: MessageType,
    ) -> Result<Vec<u8>> {
        let from_call_reference = match call_connection_reference {
            Some(call_connection_reference) => self
                .call_ref_index
                .get(&call_connection_reference.encode())
                .cloned(),
            None => None,
        };
        let from_correlation = match correlation_id {
            Some(correlation_id) => self.correlation_index.get(&correlation_id.0).cloned(),
            None => None,
        };
        match (from_call_reference, from_correlation) {
            (Some(from_call_reference), Some(from_correlation)) => {
                if from_call_reference != from_correlation {
                    return Err(Error::InvalidProcedureState {
                        message_type,
                        state: "response identifiers resolve to different sessions",
                    });
                }
                Ok(from_call_reference)
            }
            (Some(from_call_reference), None) => Ok(from_call_reference),
            (None, Some(from_correlation)) => Ok(from_correlation),
            (None, None) => Err(Error::UnknownSession),
        }
    }

    fn lookup_con_ref_by_identity(
        &self,
        call_connection_reference: Option<CallConnectionReference>,
        correlation_id: Option<CorrelationId>,
        message_type: MessageType,
    ) -> Result<Vec<u8>> {
        self.lookup_con_ref_by_response(call_connection_reference, correlation_id, message_type)
    }

    fn insert_indexes(&mut self, con_ref: &[u8], session: &Session) {
        self.traffic_index
            .insert(traffic_key(&session.a8_traffic_id), con_ref.to_vec());
        if let Some(call_connection_reference) = session.call_connection_reference {
            self.call_ref_index
                .insert(call_connection_reference.encode(), con_ref.to_vec());
        }
        if let Some(correlation_id) = session.correlation_id {
            self.correlation_index
                .insert(correlation_id.0, con_ref.to_vec());
        }
    }

    fn remove_indexes(&mut self, _con_ref: &[u8], session: &Session) {
        self.traffic_index
            .remove(&traffic_key(&session.a8_traffic_id));
        if let Some(call_connection_reference) = session.call_connection_reference {
            self.call_ref_index
                .remove(&call_connection_reference.encode());
        }
        if let Some(correlation_id) = session.correlation_id {
            self.correlation_index.remove(&correlation_id.0);
        }
    }

    fn refresh_indexes_for_refs(
        &mut self,
        con_ref: &[u8],
        call_connection_reference: Option<CallConnectionReference>,
        correlation_id: Option<CorrelationId>,
    ) {
        if let Some(call_connection_reference) = call_connection_reference {
            self.call_ref_index
                .insert(call_connection_reference.encode(), con_ref.to_vec());
        }
        if let Some(correlation_id) = correlation_id {
            self.correlation_index
                .insert(correlation_id.0, con_ref.to_vec());
        }
    }
}

fn con_ref_key(con_ref: &ConRef) -> Vec<u8> {
    vec![con_ref.0]
}

fn traffic_key(traffic_id: &A8TrafficId) -> Vec<u8> {
    traffic_id.encode()
}

fn ensure_matching_traffic_id(session: &Session, actual: &A8TrafficId) -> Result<()> {
    let expected = session.traffic_id();
    if expected != actual {
        return Err(Error::TrafficIdMismatch {
            expected: expected.key,
            actual: actual.key,
        });
    }
    Ok(())
}

fn ensure_response_direction(
    request_initiated_by_local: bool,
    response_initiated_by_local: bool,
    message_type: MessageType,
    state: &'static str,
) -> Result<()> {
    if request_initiated_by_local == response_initiated_by_local {
        return Err(Error::InvalidProcedureDirection {
            message_type,
            state,
        });
    }
    Ok(())
}

fn ensure_optional_response_correlation(
    expected: Option<CorrelationId>,
    actual: Option<CorrelationId>,
    message_type: MessageType,
    label: &'static str,
) -> Result<()> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if actual == expected => Ok(()),
        (Some(_), None) => Ok(()),
        (Some(_), Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: mismatch_correlation_state(label),
        }),
        (None, Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: unexpected_correlation_state(label),
        }),
        (None, None) => Ok(()),
    }
}

fn ensure_response_identity(
    pending: PendingRequestIdentity,
    call_connection_reference: Option<CallConnectionReference>,
    correlation_id: Option<CorrelationId>,
    message_type: MessageType,
    label: &'static str,
) -> Result<()> {
    if let (Some(expected), Some(actual)) =
        (pending.call_connection_reference, call_connection_reference)
        && expected != actual
    {
        return Err(Error::InvalidProcedureState {
            message_type,
            state: mismatch_call_connection_reference_state(label),
        });
    }
    ensure_optional_response_correlation(
        pending.correlation_id,
        correlation_id,
        message_type,
        label,
    )
}

fn ensure_release_identity_against_disconnect(
    pending: PendingRequestIdentity,
    correlation_id: Option<CorrelationId>,
    message_type: MessageType,
) -> Result<()> {
    if pending.correlation_id.is_some() {
        ensure_required_correlation(
            pending.correlation_id,
            correlation_id,
            message_type,
            "release",
        )?;
    }
    Ok(())
}

fn ensure_required_correlation(
    expected: Option<CorrelationId>,
    actual: Option<CorrelationId>,
    message_type: MessageType,
    label: &'static str,
) -> Result<()> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if actual == expected => Ok(()),
        (Some(_), None) => Err(Error::InvalidProcedureState {
            message_type,
            state: missing_correlation_state(label),
        }),
        (Some(_), Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: mismatch_correlation_state(label),
        }),
        (None, Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: unexpected_correlation_state(label),
        }),
        (None, None) => Ok(()),
    }
}

fn ensure_call_connection_reference(
    session: &Session,
    call_connection_reference: CallConnectionReference,
    message_type: MessageType,
) -> Result<()> {
    if let Some(expected) = session.call_connection_reference
        && expected != call_connection_reference
    {
        return Err(Error::InvalidProcedureState {
            message_type,
            state: "call connection reference does not match the session",
        });
    }
    Ok(())
}

fn ensure_correlation_id(
    session: &Session,
    correlation_id: CorrelationId,
    message_type: MessageType,
) -> Result<()> {
    if let Some(expected) = session.correlation_id
        && expected != correlation_id
    {
        return Err(Error::InvalidProcedureState {
            message_type,
            state: "correlation does not match the session",
        });
    }
    Ok(())
}

fn missing_correlation_state(label: &'static str) -> &'static str {
    match label {
        "connect" => "connect correlation is missing for a setup that carried correlation",
        "release" => "release correlation is missing for a disconnect that carried correlation",
        "release complete" => {
            "release complete correlation is missing for a release that carried correlation"
        }
        "BS service response" => {
            "BS service response correlation is missing for the pending request"
        }
        "version information acknowledgement" => {
            "version information acknowledgement correlation is missing for the pending request"
        }
        "short data acknowledgement" => {
            "short data acknowledgement correlation is missing for the pending request"
        }
        "access link connect acknowledgement" => {
            "access link connect acknowledgement correlation is missing for the pending request"
        }
        "access link disconnect acknowledgement" => {
            "access link disconnect acknowledgement correlation is missing for the pending request"
        }
        _ => "response correlation is missing for the pending request",
    }
}

fn mismatch_correlation_state(label: &'static str) -> &'static str {
    match label {
        "connect" => "connect correlation does not match the setup request",
        "release" => "release correlation does not match the disconnect request",
        "release complete" => "release complete correlation does not match the release request",
        "BS service response" => {
            "BS service response correlation does not match the pending request"
        }
        "version information acknowledgement" => {
            "version information acknowledgement correlation does not match the pending request"
        }
        "short data acknowledgement" => {
            "short data acknowledgement correlation does not match the pending request"
        }
        "access link connect acknowledgement" => {
            "access link connect acknowledgement correlation does not match the pending request"
        }
        "access link disconnect acknowledgement" => {
            "access link disconnect acknowledgement correlation does not match the pending request"
        }
        _ => "response correlation does not match the pending request",
    }
}

fn unexpected_correlation_state(label: &'static str) -> &'static str {
    match label {
        "connect" => "connect correlation is present without setup correlation",
        "release complete" => "release complete correlation is present without release correlation",
        "BS service response" => {
            "BS service response correlation is present without a pending request correlation"
        }
        "version information acknowledgement" => {
            "version information acknowledgement correlation is present without a pending request correlation"
        }
        "short data acknowledgement" => {
            "short data acknowledgement correlation is present without a pending request correlation"
        }
        "access link connect acknowledgement" => {
            "access link connect acknowledgement correlation is present without a pending request correlation"
        }
        "access link disconnect acknowledgement" => {
            "access link disconnect acknowledgement correlation is present without a pending request correlation"
        }
        _ => "response correlation is present without a pending request correlation",
    }
}

fn mismatch_call_connection_reference_state(label: &'static str) -> &'static str {
    match label {
        "connect" => "connect call connection reference does not match the setup request",
        "release complete" => {
            "release complete call connection reference does not match the release request"
        }
        "access link connect acknowledgement" => {
            "access link connect acknowledgement call connection reference does not match the pending request"
        }
        "access link disconnect acknowledgement" => {
            "access link disconnect acknowledgement call connection reference does not match the pending request"
        }
        _ => "response call connection reference does not match the pending request",
    }
}

fn validate_direction(
    role: ProcedureRole,
    message_type: MessageType,
    outbound: bool,
) -> Result<()> {
    let valid = matches!(
        (role, outbound, message_type),
        (ProcedureRole::Bsc, true, MessageType::SetupA8)
            | (ProcedureRole::Bsc, true, MessageType::ReleaseA8)
            | (ProcedureRole::Bsc, true, MessageType::AlConnected)
            | (ProcedureRole::Bsc, true, MessageType::AlDisconnected)
            | (ProcedureRole::Bsc, true, MessageType::BsServiceResponse)
            | (ProcedureRole::Bsc, true, MessageType::VersionInfo)
            | (ProcedureRole::Bsc, true, MessageType::VersionInfoAck)
            | (ProcedureRole::Bsc, true, MessageType::UpdateA8)
            | (ProcedureRole::Bsc, true, MessageType::UpdateA8Ack)
            | (ProcedureRole::Bsc, true, MessageType::ShortDataDelivery)
            | (ProcedureRole::Bsc, true, MessageType::ShortDataAck)
            | (ProcedureRole::Bsc, false, MessageType::ConnectA8)
            | (ProcedureRole::Bsc, false, MessageType::DisconnectA8)
            | (ProcedureRole::Bsc, false, MessageType::ReleaseA8Complete)
            | (ProcedureRole::Bsc, false, MessageType::AlConnectedAck)
            | (ProcedureRole::Bsc, false, MessageType::AlDisconnectedAck)
            | (ProcedureRole::Bsc, false, MessageType::BsServiceRequest)
            | (ProcedureRole::Bsc, false, MessageType::VersionInfo)
            | (ProcedureRole::Bsc, false, MessageType::VersionInfoAck)
            | (ProcedureRole::Bsc, false, MessageType::UpdateA8)
            | (ProcedureRole::Bsc, false, MessageType::UpdateA8Ack)
            | (ProcedureRole::Bsc, false, MessageType::ShortDataDelivery)
            | (ProcedureRole::Pcf, false, MessageType::SetupA8)
            | (ProcedureRole::Pcf, false, MessageType::ReleaseA8)
            | (ProcedureRole::Pcf, false, MessageType::AlConnected)
            | (ProcedureRole::Pcf, false, MessageType::AlDisconnected)
            | (ProcedureRole::Pcf, false, MessageType::BsServiceResponse)
            | (ProcedureRole::Pcf, false, MessageType::VersionInfo)
            | (ProcedureRole::Pcf, false, MessageType::VersionInfoAck)
            | (ProcedureRole::Pcf, false, MessageType::UpdateA8)
            | (ProcedureRole::Pcf, false, MessageType::UpdateA8Ack)
            | (ProcedureRole::Pcf, false, MessageType::ShortDataDelivery)
            | (ProcedureRole::Pcf, false, MessageType::ShortDataAck)
            | (ProcedureRole::Pcf, true, MessageType::ConnectA8)
            | (ProcedureRole::Pcf, true, MessageType::DisconnectA8)
            | (ProcedureRole::Pcf, true, MessageType::ReleaseA8Complete)
            | (ProcedureRole::Pcf, true, MessageType::AlConnectedAck)
            | (ProcedureRole::Pcf, true, MessageType::AlDisconnectedAck)
            | (ProcedureRole::Pcf, true, MessageType::BsServiceRequest)
            | (ProcedureRole::Pcf, true, MessageType::VersionInfo)
            | (ProcedureRole::Pcf, true, MessageType::VersionInfoAck)
            | (ProcedureRole::Pcf, true, MessageType::UpdateA8)
            | (ProcedureRole::Pcf, true, MessageType::UpdateA8Ack)
            | (ProcedureRole::Pcf, true, MessageType::ShortDataDelivery)
    );
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidProcedureDirection {
            message_type,
            state: if outbound {
                "message direction is not valid for outbound routing on this role"
            } else {
                "message direction is not valid for inbound routing on this role"
            },
        })
    }
}

fn short_data_requires_ack(role: ProcedureRole, outbound: bool) -> bool {
    matches!(
        (role, outbound),
        (ProcedureRole::Pcf, true) | (ProcedureRole::Bsc, false)
    )
}

fn ensure_optional_identity_match(
    expected: Option<&str>,
    actual: Option<&str>,
    message_type: MessageType,
    mismatch_state: &'static str,
    unexpected_state: &'static str,
) -> Result<()> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if actual == expected => Ok(()),
        (Some(_), None) | (Some(_), Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: mismatch_state,
        }),
        (None, Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: unexpected_state,
        }),
        (None, None) => Ok(()),
    }
}

fn ensure_optional_scalar_match<T: Copy + PartialEq>(
    expected: Option<T>,
    actual: Option<T>,
    message_type: MessageType,
    mismatch_state: &'static str,
    unexpected_state: &'static str,
) -> Result<()> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if actual == expected => Ok(()),
        (Some(_), None) | (Some(_), Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: mismatch_state,
        }),
        (None, Some(_)) => Err(Error::InvalidProcedureState {
            message_type,
            state: unexpected_state,
        }),
        (None, None) => Ok(()),
    }
}
