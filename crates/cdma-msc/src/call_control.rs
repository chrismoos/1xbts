//! MSC-side ownership of A1 call-control state.

use std::collections::HashMap;

use cdma_ios::{
    CallControlState, EngineEvent, MobileIdentity, ProcedureDirection, ProcedureEngine,
    ProcedureError, ProcedureMessage,
};

use crate::media_gateway::CallHandle;

/// Stable MSC-local call identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId(pub u64);

/// High-level direction of the call from the MSC point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallDirection {
    /// The mobile initiated the call toward the MSC.
    MobileOriginated,
    /// The MSC initiated paging toward the mobile.
    MobileTerminated,
}

/// Read-only snapshot of an MSC-owned call session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSessionSnapshot {
    /// MSC-local call identifier.
    pub id: CallId,
    /// Call direction from the MSC perspective.
    pub direction: CallDirection,
    /// Current A1 call-control state.
    pub state: CallControlState,
    /// Mobile identity associated with the call when known.
    pub mobile_identity: Option<MobileIdentity>,
    /// Hardware identity associated with the call when known.
    pub mobile_identity_esn: Option<MobileIdentity>,
    /// Attached media-gateway handle when the call has been bound to one.
    pub media_gateway_handle: Option<CallHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallSession {
    id: CallId,
    direction: CallDirection,
    mobile_identity: Option<MobileIdentity>,
    mobile_identity_esn: Option<MobileIdentity>,
    media_gateway_handle: Option<CallHandle>,
    engine: ProcedureEngine,
}

impl CallSession {
    fn snapshot(&self) -> CallSessionSnapshot {
        CallSessionSnapshot {
            id: self.id,
            direction: self.direction,
            state: self.engine.call_control().state(),
            mobile_identity: self.mobile_identity.clone(),
            mobile_identity_esn: self.mobile_identity_esn.clone(),
            media_gateway_handle: self.media_gateway_handle,
        }
    }
}

/// Errors returned by [`MscCallController`].
#[derive(Debug)]
pub enum CallControlError {
    /// The referenced call does not exist.
    UnknownCall(CallId),
    /// The underlying A1 procedure engine rejected the transition.
    Procedure(ProcedureError),
}

impl std::fmt::Display for CallControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CallControlError {}

impl From<ProcedureError> for CallControlError {
    fn from(value: ProcedureError) -> Self {
        Self::Procedure(value)
    }
}

/// MSC-owned controller for A1 call-control sessions.
#[derive(Debug)]
pub struct MscCallController {
    next_call_id: u64,
    calls: HashMap<CallId, CallSession>,
}

impl Default for MscCallController {
    fn default() -> Self {
        Self::new()
    }
}

impl MscCallController {
    /// Creates an empty MSC call controller.
    pub fn new() -> Self {
        Self {
            next_call_id: 1,
            calls: HashMap::new(),
        }
    }

    /// Creates a new call session owned by the MSC.
    pub fn create_call(
        &mut self,
        direction: CallDirection,
        mobile_identity: Option<MobileIdentity>,
    ) -> CallId {
        let id = loop {
            let candidate = CallId(self.next_call_id);
            self.next_call_id = self.next_call_id.wrapping_add(1);
            if !self.calls.contains_key(&candidate) {
                break candidate;
            }
        };
        self.insert_call(id, direction, mobile_identity);
        id
    }

    /// Creates a call session with an externally supplied call identifier.
    pub fn create_call_with_id(
        &mut self,
        id: CallId,
        direction: CallDirection,
        mobile_identity: Option<MobileIdentity>,
    ) -> CallId {
        self.insert_call(id, direction, mobile_identity);
        id
    }

    fn insert_call(
        &mut self,
        id: CallId,
        direction: CallDirection,
        mobile_identity: Option<MobileIdentity>,
    ) {
        self.calls.insert(
            id,
            CallSession {
                id,
                direction,
                mobile_identity,
                mobile_identity_esn: None,
                media_gateway_handle: None,
                engine: ProcedureEngine::new(),
            },
        );
    }

    /// Stores the ESN/MEID identity learned from Layer 3 for this call.
    pub fn set_mobile_identity_esn(
        &mut self,
        call_id: CallId,
        mobile_identity_esn: Option<MobileIdentity>,
    ) -> Result<(), CallControlError> {
        let session = self
            .calls
            .get_mut(&call_id)
            .ok_or(CallControlError::UnknownCall(call_id))?;
        session.mobile_identity_esn = mobile_identity_esn;
        Ok(())
    }

    /// Returns an immutable snapshot of the named call session.
    pub fn snapshot(&self, call_id: CallId) -> Option<CallSessionSnapshot> {
        self.calls.get(&call_id).map(CallSession::snapshot)
    }

    /// Returns the current A1 call-control state for the named call.
    pub fn state(&self, call_id: CallId) -> Option<CallControlState> {
        self.calls
            .get(&call_id)
            .map(|session| session.engine.call_control().state())
    }

    /// Attaches a media-gateway handle to an existing call session.
    pub fn attach_media_gateway_handle(
        &mut self,
        call_id: CallId,
        handle: CallHandle,
    ) -> Result<(), CallControlError> {
        let session = self
            .calls
            .get_mut(&call_id)
            .ok_or(CallControlError::UnknownCall(call_id))?;
        session.media_gateway_handle = Some(handle);
        Ok(())
    }

    /// Applies a BSC-originated A1 message to an existing MSC call session.
    pub fn apply_from_bsc(
        &mut self,
        call_id: CallId,
        message: &ProcedureMessage,
    ) -> Result<EngineEvent, CallControlError> {
        let session = self
            .calls
            .get_mut(&call_id)
            .ok_or(CallControlError::UnknownCall(call_id))?;
        Ok(session
            .engine
            .apply(ProcedureDirection::BscToMsc, message)?)
    }

    /// Applies an MSC-originated A1 message to an existing MSC call session.
    pub fn apply_from_msc(
        &mut self,
        call_id: CallId,
        message: &ProcedureMessage,
    ) -> Result<EngineEvent, CallControlError> {
        let session = self
            .calls
            .get_mut(&call_id)
            .ok_or(CallControlError::UnknownCall(call_id))?;
        Ok(session
            .engine
            .apply(ProcedureDirection::MscToBsc, message)?)
    }

    /// Reset the call-control engine to post-PagingRequest so a fresh
    /// PagingResponse is accepted.
    pub fn rearm_for_repage(
        &mut self,
        call_id: CallId,
        paging_request: &ProcedureMessage,
    ) -> Result<(), CallControlError> {
        let session = self
            .calls
            .get_mut(&call_id)
            .ok_or(CallControlError::UnknownCall(call_id))?;
        session.engine = ProcedureEngine::new();
        session
            .engine
            .apply(ProcedureDirection::MscToBsc, paging_request)?;
        Ok(())
    }

    /// Removes a call session once higher layers are done with it.
    pub fn remove_call(&mut self, call_id: CallId) -> Option<CallSessionSnapshot> {
        self.calls
            .remove(&call_id)
            .map(|session| session.snapshot())
    }

    /// Returns the number of active call sessions owned by the controller.
    pub fn active_call_count(&self) -> usize {
        self.calls.len()
    }

    /// Returns snapshots of all active call sessions.
    pub fn all_snapshots(&self) -> Vec<CallSessionSnapshot> {
        self.calls.values().map(CallSession::snapshot).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_ios::{
        AlertWithInformationMessage, AssignmentCompleteMessage, AssignmentRequestMessage,
        AuthenticationChallengeParameter, AuthenticationConfirmationParameter, AuthenticationEvent,
        AuthenticationParameterCount, AuthenticationResponseParameter, Cause, CellId,
        CellIdentifierList, ChannelNumber, ChannelType, CircuitIdentityCode,
        ClassmarkInformationType2, ClearCommandMessage, ClearCompleteMessage, ClearRequestMessage,
        CompleteLayer3InformationMessage, ConnectMessage, EngineEvent,
        HandoffCdmaServingOneWayDelay, HandoffCellIdentifier, Layer3Information, MobileIdentity,
        PagingRequestMessage, ProcedureMessage, RadioEnvironmentAndResources, ServiceOption,
        SlotCycleIndex, Tag, UserZoneId,
    };

    fn complete_layer3_information() -> CompleteLayer3InformationMessage {
        CompleteLayer3InformationMessage {
            cell_identifier: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            layer3_information: Layer3Information(vec![0x03, 0x00, 0x24, 0x01]),
        }
    }

    fn paging_request() -> PagingRequestMessage {
        PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            tag: Some(Tag(0x01020304)),
            cell_identifier_list: Some(CellIdentifierList::Cells(vec![CellId {
                cell: 0x123,
                sector: 0x4,
            }])),
            slot_cycle_index: Some(SlotCycleIndex(0x05)),
            service_option: Some(ServiceOption(0x0003)),
            is2000_mobile_capabilities: None,
        }
    }

    fn paging_response() -> cdma_ios::PagingResponseMessage {
        cdma_ios::PagingResponseMessage {
            classmark_information_type_2: ClassmarkInformationType2(vec![
                0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
            ]),
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            tag: Some(Tag(0x01020304)),
            mobile_identity_esn: Some(MobileIdentity::Esn(0x11223344)),
            slot_cycle_index: Some(SlotCycleIndex(0x05)),
            authentication_response_parameter: Some(AuthenticationResponseParameter([
                0x10, 0x20, 0x30, 0x40,
            ])),
            authentication_confirmation_parameter: Some(AuthenticationConfirmationParameter(0x99)),
            authentication_parameter_count: Some(AuthenticationParameterCount(0x1f)),
            authentication_challenge_parameter: Some(AuthenticationChallengeParameter([
                0x10, 0x01, 0x02, 0x03, 0x04,
            ])),
            service_option: Some(ServiceOption::HIGH_RATE_PACKET_DATA),
            voice_privacy_request: true,
            circuit_identity_code: Some(CircuitIdentityCode {
                pcm_multiplexer: 0x0123,
                timeslot: 0x1a,
            }),
            cdma_serving_one_way_delay: Some(HandoffCdmaServingOneWayDelay {
                cell: HandoffCellIdentifier::Cell(CellId {
                    cell: 0x123,
                    sector: 0x4,
                }),
                delay_100ns: 0x0102,
            }),
            authentication_event: Some(AuthenticationEvent(0x02)),
            radio_environment_and_resources: Some(RadioEnvironmentAndResources {
                include_priority: false,
                forward: 0x00,
                reverse: 0x00,
                allocated: true,
                available: true,
            }),
            user_zone_id: Some(UserZoneId(0x3344)),
            is2000_mobile_capabilities: None,
        }
    }

    fn assignment_request() -> AssignmentRequestMessage {
        AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 0x01,
                channel_rate_and_type: 0x08,
                coding: 0x05,
            },
            circuit_identity_code: CircuitIdentityCode {
                pcm_multiplexer: 0x0123,
                timeslot: 0x1a,
            },
            encryption_information: None,
            service_option: Some(ServiceOption(0x0003)),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        }
    }

    #[test]
    fn externally_supplied_call_id_does_not_advance_msc_allocator() {
        let mut controller = MscCallController::new();
        let bsc_call_id = CallId(0x1_0000_0000);
        controller.create_call_with_id(bsc_call_id, CallDirection::MobileOriginated, None);

        let mt_call_id = controller.create_call(CallDirection::MobileTerminated, None);

        assert_eq!(mt_call_id, CallId(1));
        assert!(controller.snapshot(bsc_call_id).is_some());
        assert!(controller.snapshot(mt_call_id).is_some());
    }

    #[test]
    fn msc_allocator_skips_an_occupied_external_call_id() {
        let mut controller = MscCallController::new();
        controller.create_call_with_id(CallId(1), CallDirection::MobileOriginated, None);

        let mt_call_id = controller.create_call(CallDirection::MobileTerminated, None);

        assert_eq!(mt_call_id, CallId(2));
        assert_eq!(controller.active_call_count(), 2);
    }

    #[test]
    fn mobile_originated_call_flow_reaches_connected() {
        let mut controller = MscCallController::new();
        let call_id = controller.create_call(CallDirection::MobileOriginated, None);

        let event = controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::CompleteLayer3Information(complete_layer3_information()),
            )
            .unwrap();
        assert!(matches!(
            event,
            EngineEvent::CallControl(transition)
                if transition.new_state == CallControlState::AccessPending
        ));

        controller
            .apply_from_msc(
                call_id,
                &ProcedureMessage::AssignmentRequest(assignment_request()),
            )
            .unwrap();
        assert_eq!(
            controller.state(call_id),
            Some(CallControlState::AssignmentPending)
        );

        controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::AssignmentComplete(AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }),
            )
            .unwrap();
        assert_eq!(controller.state(call_id), Some(CallControlState::Assigned));

        controller
            .apply_from_bsc(call_id, &ProcedureMessage::Connect(ConnectMessage))
            .unwrap();
        assert_eq!(controller.state(call_id), Some(CallControlState::Connected));
    }

    #[test]
    fn mobile_terminated_call_flows_through_paging_and_assignment() {
        let mut controller = MscCallController::new();
        let call_id = controller.create_call(
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );

        controller
            .apply_from_msc(call_id, &ProcedureMessage::PagingRequest(paging_request()))
            .unwrap();
        assert_eq!(controller.state(call_id), Some(CallControlState::Paging));

        controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::PagingResponse(paging_response()),
            )
            .unwrap();
        assert_eq!(
            controller.state(call_id),
            Some(CallControlState::AccessPending)
        );

        controller
            .apply_from_msc(
                call_id,
                &ProcedureMessage::AssignmentRequest(assignment_request()),
            )
            .unwrap();
        controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::AssignmentComplete(AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }),
            )
            .unwrap();
        controller
            .apply_from_msc(
                call_id,
                &ProcedureMessage::AlertWithInformation(AlertWithInformationMessage {
                    ms_information_records: None,
                }),
            )
            .unwrap();
        assert_eq!(controller.state(call_id), Some(CallControlState::Alerting));
    }

    #[test]
    fn controller_tracks_media_gateway_handle_and_release() {
        let mut controller = MscCallController::new();
        let call_id = controller.create_call(CallDirection::MobileOriginated, None);
        controller
            .attach_media_gateway_handle(call_id, CallHandle(77))
            .unwrap();
        let snapshot = controller.snapshot(call_id).unwrap();
        assert_eq!(snapshot.media_gateway_handle, Some(CallHandle(77)));

        let removed = controller.remove_call(call_id).unwrap();
        assert_eq!(removed.id, call_id);
        assert_eq!(controller.active_call_count(), 0);
    }

    #[test]
    fn mobile_terminated_clear_flow_reaches_released() {
        let mut controller = MscCallController::new();
        let call_id = controller.create_call(
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );

        controller
            .apply_from_msc(call_id, &ProcedureMessage::PagingRequest(paging_request()))
            .unwrap();
        controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::PagingResponse(paging_response()),
            )
            .unwrap();
        controller
            .apply_from_msc(
                call_id,
                &ProcedureMessage::AssignmentRequest(assignment_request()),
            )
            .unwrap();
        controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::AssignmentComplete(AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }),
            )
            .unwrap();
        controller
            .apply_from_bsc(call_id, &ProcedureMessage::Connect(ConnectMessage))
            .unwrap();

        let clear = controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::ClearRequest(ClearRequestMessage {
                    cause: Cause(0x09),
                    cause_layer3: None,
                }),
            )
            .unwrap();
        assert!(matches!(
            clear,
            EngineEvent::CallControl(transition)
                if transition.new_state == CallControlState::Clearing
        ));
        let clear_command = controller
            .apply_from_msc(
                call_id,
                &ProcedureMessage::ClearCommand(ClearCommandMessage {
                    cause: Cause(0x09),
                    cause_layer3: None,
                }),
            )
            .unwrap();
        assert!(matches!(
            clear_command,
            EngineEvent::CallControl(transition)
                if transition.previous_state == CallControlState::Clearing
                    && transition.new_state == CallControlState::Clearing
                    && transition.timer_actions.is_empty()
        ));

        let cleared = controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::ClearComplete(ClearCompleteMessage {
                    power_down_indicator: false,
                }),
            )
            .unwrap();
        assert!(matches!(
            cleared,
            EngineEvent::CallControl(transition)
                if transition.new_state == CallControlState::Released
        ));
        assert_eq!(controller.state(call_id), Some(CallControlState::Released));
    }

    #[test]
    fn unknown_call_is_rejected() {
        let mut controller = MscCallController::new();
        let err = controller
            .apply_from_bsc(
                CallId(99),
                &ProcedureMessage::ClearRequest(cdma_ios::ClearRequestMessage {
                    cause: Cause(0x09),
                    cause_layer3: None,
                }),
            )
            .unwrap_err();
        assert!(matches!(err, CallControlError::UnknownCall(CallId(99))));
    }
}
