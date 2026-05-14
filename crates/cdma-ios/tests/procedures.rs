use cdma_ios::{
    AlertWithInformationMessage, AssignmentCompleteMessage, AssignmentFailureMessage,
    AssignmentRequestMessage, BsServiceProcedure, BsServiceRequestMessage,
    BsServiceResponseMessage, CallControlProcedure, CallControlState, CallControlTimer, CellId,
    ChannelType, ClassmarkInformationType2, ClearCompleteMessage, ClearRequestMessage,
    CmServiceRequestMessage, CmServiceType, CompleteLayer3InformationMessage, ConnectMessage,
    EngineEvent, EngineTimer, HandoffCellIdentifier, HandoffCellIdentifierList,
    HandoffCommandMessage, HandoffCommencedMessage, HandoffCompleteMessage, HandoffFailureMessage,
    HandoffPerformedMessage, HandoffRequestAcknowledgeMessage, HandoffRequestMessage,
    HandoffRequiredMessage, HandoffRequiredRejectMessage, Layer3Information, MobileIdentity,
    PagingRequestMessage, PagingResponseMessage, ProcedureDirection, ProcedureEngine,
    ProcedureError, ProcedureMessage, ProgressMessage, SourceHandoffProcedure, SourceHandoffState,
    SourceHandoffTimer, Tag, TargetHandoffProcedure, TargetHandoffState, TargetHandoffTimer,
    TimerAction,
};

fn cell_id() -> CellId {
    CellId {
        cell: 0x123,
        sector: 0x4,
    }
}

fn assignment_request() -> AssignmentRequestMessage {
    AssignmentRequestMessage {
        channel_type: ChannelType {
            speech_or_data_indicator: 0x01,
            channel_rate_and_type: 0x08,
            coding: 0x05,
        },
        circuit_identity_code: cdma_ios::CircuitIdentityCode {
            pcm_multiplexer: 0x0123,
            timeslot: 0x1a,
        },
        encryption_information: None,
        service_option: None,
        signals: vec![],
        ms_information_records: None,
        priority: None,
        paca_timestamp: None,
        quality_of_service_parameters: None,
        a2p_bearer_session_params: None,
        a2p_bearer_format_params: None,
    }
}

fn paging_request() -> PagingRequestMessage {
    PagingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        tag: Some(Tag(0x01020304)),
        cell_identifier_list: None,
        slot_cycle_index: None,
        service_option: None,
        is2000_mobile_capabilities: None,
    }
}

fn paging_response() -> PagingResponseMessage {
    PagingResponseMessage {
        classmark_information_type_2: ClassmarkInformationType2(vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ]),
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        tag: None,
        mobile_identity_esn: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: None,
        voice_privacy_request: false,
        circuit_identity_code: None,
        cdma_serving_one_way_delay: None,
        authentication_event: None,
        radio_environment_and_resources: None,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    }
}

fn complete_layer3_information() -> CompleteLayer3InformationMessage {
    let dtap = CmServiceRequestMessage {
        cm_service_type: CmServiceType::MobileOriginatingCallEstablishment,
        classmark_information_type_2: ClassmarkInformationType2(vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ]),
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        called_party_bcd_number: None,
        tag: None,
        mobile_identity_esn: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: None,
        voice_privacy_request: false,
        radio_environment_and_resources: None,
        called_party_ascii_number: None,
        circuit_identity_code: None,
        cdma_serving_one_way_delay: None,
        authentication_event: None,
        authentication_data: None,
        paca_reorigination_indicator: false,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    CompleteLayer3InformationMessage {
        cell_identifier: cell_id(),
        layer3_information: Layer3Information::from_cm_service_request(&dtap).unwrap(),
    }
}

fn handoff_required() -> HandoffRequiredMessage {
    HandoffRequiredMessage {
        cause: cdma_ios::Cause(0x0e),
        target_cell_identifier_list: HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        },
        classmark_information_type_2: None,
        response_request: false,
        encryption_information: None,
        is95_channel_identity: None,
        mobile_identity_esn: None,
        downlink_radio_environment: None,
        service_option: None,
        cdma_serving_one_way_delay: None,
        is95_ms_measured_channel_identity: None,
        is2000_channel_identity: None,
        quality_of_service_parameters: None,
        is2000_mobile_capabilities: None,
        is2000_service_configuration_record: None,
        pdsn_ip_address: None,
        protocol_type: None,
    }
}

fn handoff_request() -> HandoffRequestMessage {
    HandoffRequestMessage {
        channel_type: ChannelType {
            speech_or_data_indicator: 0x01,
            channel_rate_and_type: 0x08,
            coding: 0x05,
        },
        encryption_information: None,
        classmark_information_type_2: None,
        target_cell_identifier_list: None,
        circuit_identity_code_extension: None,
        is95_channel_identity: None,
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        mobile_identity_esn: None,
        downlink_radio_environment: None,
        service_option: None,
        cdma_serving_one_way_delay: None,
        is95_ms_measured_channel_identity: None,
        is2000_channel_identity: None,
        quality_of_service_parameters: None,
        is2000_mobile_capabilities: None,
        is2000_service_configuration_record: None,
        pdsn_ip_address: None,
        protocol_type: None,
    }
}

#[test]
fn call_control_mobile_originated_flow() {
    let mut procedure = CallControlProcedure::new();

    assert_eq!(procedure.state(), CallControlState::Idle);
    procedure
        .on_complete_layer3_information(&complete_layer3_information())
        .unwrap();
    let assignment = procedure
        .on_assignment_request(&assignment_request())
        .unwrap();
    assert_eq!(
        assignment.timer_actions,
        vec![TimerAction::Arm(CallControlTimer::Assignment)]
    );
    procedure
        .on_assignment_complete(&AssignmentCompleteMessage {
            channel_number: cdma_ios::ChannelNumber(0x1122),
            encryption_information: None,
            service_option: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        })
        .unwrap();
    procedure
        .on_progress(&ProgressMessage {
            signal: None,
            ms_information_records: None,
        })
        .unwrap();
    procedure.on_connect(&ConnectMessage).unwrap();
    procedure
        .on_clear_request(&ClearRequestMessage {
            cause: cdma_ios::Cause(0x09),
            cause_layer3: None,
        })
        .unwrap();
    let cleared = procedure
        .on_clear_complete(&ClearCompleteMessage {
            power_down_indicator: false,
        })
        .unwrap();

    assert_eq!(cleared.new_state, CallControlState::Released);
    assert_eq!(
        cleared.timer_actions,
        vec![TimerAction::Cancel(CallControlTimer::Clear)]
    );
}

#[test]
fn call_control_mobile_terminated_flow() {
    let mut procedure = CallControlProcedure::new();

    procedure.on_paging_request(&paging_request()).unwrap();
    procedure.on_paging_response(&paging_response()).unwrap();
    procedure
        .on_assignment_request(&assignment_request())
        .unwrap();
    procedure
        .on_assignment_complete(&AssignmentCompleteMessage {
            channel_number: cdma_ios::ChannelNumber(0x1122),
            encryption_information: None,
            service_option: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        })
        .unwrap();
    procedure.on_connect(&ConnectMessage).unwrap();

    assert_eq!(procedure.state(), CallControlState::Connected);
}

#[test]
fn call_control_rejects_out_of_order_connect() {
    let mut procedure = CallControlProcedure::new();
    let error = procedure.on_connect(&ConnectMessage).unwrap_err();
    assert_eq!(
        error,
        ProcedureError::InvalidTransition {
            procedure: "CallControl",
            state: "Idle",
            reason: "Connect is only valid from Assigned or Alerting",
        }
    );
}

#[test]
fn call_control_assignment_failure_clears() {
    let mut procedure = CallControlProcedure::new();
    procedure
        .on_complete_layer3_information(&complete_layer3_information())
        .unwrap();
    procedure
        .on_assignment_request(&assignment_request())
        .unwrap();
    procedure
        .on_assignment_failure(&AssignmentFailureMessage {
            cause: cdma_ios::Cause(0x21),
        })
        .unwrap();
    let clearing = procedure
        .on_clear_request(&ClearRequestMessage {
            cause: cdma_ios::Cause(0x09),
            cause_layer3: None,
        })
        .unwrap();
    assert_eq!(clearing.new_state, CallControlState::Clearing);
}

#[test]
fn bs_service_request_response_flow() {
    let mut procedure = BsServiceProcedure::new();
    let request = procedure
        .on_request(&BsServiceRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            mobile_identity_esn: None,
            service_option: None,
            tag: None,
        })
        .unwrap();
    assert_eq!(
        request.timer_actions,
        vec![TimerAction::Arm(cdma_ios::BsServiceTimer::Response)]
    );
    let response = procedure
        .on_response(&BsServiceResponseMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            mobile_identity_esn: None,
            tag: None,
            cause: None,
        })
        .unwrap();
    assert_eq!(
        response.timer_actions,
        vec![TimerAction::Cancel(cdma_ios::BsServiceTimer::Response)]
    );
}

#[test]
fn source_handoff_required_to_command_flow() {
    let mut procedure = SourceHandoffProcedure::new();
    let required = procedure.on_handoff_required(&handoff_required()).unwrap();
    assert_eq!(
        required.timer_actions,
        vec![TimerAction::Arm(SourceHandoffTimer::Command)]
    );
    let commanded = procedure
        .on_handoff_command(&HandoffCommandMessage {
            rf_channel_identity: None,
            is95_channel_identity: None,
            cell_identifier_list: None,
            handoff_power_level: None,
            sid: None,
            extended_handoff_direction_parameters: None,
            hard_handoff_parameters: None,
            is2000_channel_identity: None,
            is2000_service_configuration_record: None,
            is2000_non_negotiable_service_configuration_record: None,
        })
        .unwrap();
    assert_eq!(procedure.state(), SourceHandoffState::Commanded);
    assert_eq!(
        commanded.timer_actions,
        vec![TimerAction::Cancel(SourceHandoffTimer::Command)]
    );
}

#[test]
fn source_handoff_reject_flow() {
    let mut procedure = SourceHandoffProcedure::new();
    procedure.on_handoff_required(&handoff_required()).unwrap();
    let rejected = procedure
        .on_handoff_required_reject(&HandoffRequiredRejectMessage {
            cause: cdma_ios::Cause(0x2a),
        })
        .unwrap();
    assert_eq!(procedure.state(), SourceHandoffState::Rejected);
    assert_eq!(
        rejected.timer_actions,
        vec![TimerAction::Cancel(SourceHandoffTimer::Command)]
    );
}

#[test]
fn target_handoff_request_ack_flow() {
    let mut procedure = TargetHandoffProcedure::new();
    let requested = procedure.on_handoff_request(&handoff_request()).unwrap();
    assert_eq!(
        requested.timer_actions,
        vec![TimerAction::Arm(TargetHandoffTimer::Response)]
    );
    let acknowledged = procedure
        .on_handoff_request_acknowledge(&HandoffRequestAcknowledgeMessage {
            is95_channel_identity: None,
            cell_identifier_list: None,
            extended_handoff_direction_parameters: None,
            hard_handoff_parameters: None,
            is2000_channel_identity: None,
            is2000_service_configuration_record: None,
            is2000_non_negotiable_service_configuration_record: None,
        })
        .unwrap();
    assert_eq!(procedure.state(), TargetHandoffState::AwaitingArrival);
    assert_eq!(
        acknowledged.timer_actions,
        vec![
            TimerAction::Cancel(TargetHandoffTimer::Response),
            TimerAction::Arm(TargetHandoffTimer::Arrival),
        ]
    );
}

#[test]
fn target_handoff_failure_flow() {
    let mut procedure = TargetHandoffProcedure::new();
    procedure.on_handoff_request(&handoff_request()).unwrap();
    let failed = procedure
        .on_handoff_failure(&HandoffFailureMessage {
            cause: cdma_ios::Cause(0x21),
        })
        .unwrap();
    assert_eq!(procedure.state(), TargetHandoffState::Failed);
    assert_eq!(
        failed.timer_actions,
        vec![TimerAction::Cancel(TargetHandoffTimer::Response)]
    );
}

#[test]
fn source_handoff_commenced_to_clear_flow() {
    let mut procedure = SourceHandoffProcedure::new();
    procedure.on_handoff_required(&handoff_required()).unwrap();
    procedure
        .on_handoff_command(&HandoffCommandMessage {
            rf_channel_identity: None,
            is95_channel_identity: None,
            cell_identifier_list: None,
            handoff_power_level: None,
            sid: None,
            extended_handoff_direction_parameters: None,
            hard_handoff_parameters: None,
            is2000_channel_identity: None,
            is2000_service_configuration_record: None,
            is2000_non_negotiable_service_configuration_record: None,
        })
        .unwrap();
    let commenced = procedure
        .on_handoff_commenced(&HandoffCommencedMessage)
        .unwrap();
    assert_eq!(procedure.state(), SourceHandoffState::Commenced);
    assert_eq!(
        commenced.timer_actions,
        vec![TimerAction::Arm(SourceHandoffTimer::Clear)]
    );
    let cleared = procedure
        .on_clear_command(&cdma_ios::ClearCommandMessage {
            cause: cdma_ios::Cause(0x09),
            cause_layer3: None,
        })
        .unwrap();
    assert_eq!(procedure.state(), SourceHandoffState::Cleared);
    assert_eq!(
        cleared.timer_actions,
        vec![TimerAction::Cancel(SourceHandoffTimer::Clear)]
    );
}

#[test]
fn target_handoff_complete_flow() {
    let mut procedure = TargetHandoffProcedure::new();
    procedure.on_handoff_request(&handoff_request()).unwrap();
    procedure
        .on_handoff_request_acknowledge(&HandoffRequestAcknowledgeMessage {
            is95_channel_identity: None,
            cell_identifier_list: None,
            extended_handoff_direction_parameters: None,
            hard_handoff_parameters: None,
            is2000_channel_identity: None,
            is2000_service_configuration_record: None,
            is2000_non_negotiable_service_configuration_record: None,
        })
        .unwrap();
    let complete = procedure
        .on_handoff_complete(&HandoffCompleteMessage)
        .unwrap();
    assert_eq!(procedure.state(), TargetHandoffState::Completed);
    assert_eq!(
        complete.timer_actions,
        vec![TimerAction::Cancel(TargetHandoffTimer::Arrival)]
    );
}

#[test]
fn source_handoff_performed_keeps_state() {
    let mut procedure = SourceHandoffProcedure::new();
    let transition = procedure
        .on_handoff_performed(&HandoffPerformedMessage {
            cause: cdma_ios::Cause(0x0e),
            cell_identifier_list: Some(HandoffCellIdentifierList {
                cells: vec![HandoffCellIdentifier::Cell(cell_id())],
            }),
        })
        .unwrap();
    assert_eq!(procedure.state(), SourceHandoffState::Idle);
    assert_eq!(transition.previous_state, SourceHandoffState::Idle);
    assert_eq!(transition.new_state, SourceHandoffState::Idle);
    assert!(transition.timer_actions.is_empty());
}

#[test]
fn call_control_timer_expiry_paths() {
    let mut procedure = CallControlProcedure::new();
    procedure
        .on_complete_layer3_information(&complete_layer3_information())
        .unwrap();
    procedure
        .on_assignment_request(&assignment_request())
        .unwrap();
    let timed_out = procedure
        .on_timer_expired(CallControlTimer::Assignment)
        .unwrap();
    assert_eq!(timed_out.new_state, CallControlState::TimedOut);

    let mut clearing = CallControlProcedure::new();
    clearing
        .on_complete_layer3_information(&complete_layer3_information())
        .unwrap();
    clearing
        .on_assignment_request(&assignment_request())
        .unwrap();
    clearing
        .on_assignment_complete(&AssignmentCompleteMessage {
            channel_number: cdma_ios::ChannelNumber(0x1122),
            encryption_information: None,
            service_option: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        })
        .unwrap();
    clearing
        .on_clear_request(&ClearRequestMessage {
            cause: cdma_ios::Cause(0x09),
            cause_layer3: None,
        })
        .unwrap();
    let released = clearing.on_timer_expired(CallControlTimer::Clear).unwrap();
    assert_eq!(released.new_state, CallControlState::Released);
}

#[test]
fn bs_service_timer_expiry_path() {
    let mut procedure = BsServiceProcedure::new();
    procedure
        .on_request(&BsServiceRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            mobile_identity_esn: None,
            service_option: None,
            tag: None,
        })
        .unwrap();
    let timed_out = procedure
        .on_timer_expired(cdma_ios::BsServiceTimer::Response)
        .unwrap();
    assert_eq!(timed_out.new_state, cdma_ios::BsServiceState::TimedOut);
}

#[test]
fn handoff_timer_expiry_paths() {
    let mut source = SourceHandoffProcedure::new();
    source.on_handoff_required(&handoff_required()).unwrap();
    let source_timeout = source
        .on_timer_expired(SourceHandoffTimer::Command)
        .unwrap();
    assert_eq!(source_timeout.new_state, SourceHandoffState::TimedOut);

    let mut target = TargetHandoffProcedure::new();
    target.on_handoff_request(&handoff_request()).unwrap();
    let target_timeout = target
        .on_timer_expired(TargetHandoffTimer::Response)
        .unwrap();
    assert_eq!(target_timeout.new_state, TargetHandoffState::TimedOut);

    let mut commenced = SourceHandoffProcedure::new();
    commenced.on_handoff_required(&handoff_required()).unwrap();
    commenced
        .on_handoff_command(&HandoffCommandMessage {
            rf_channel_identity: None,
            is95_channel_identity: None,
            cell_identifier_list: None,
            handoff_power_level: None,
            sid: None,
            extended_handoff_direction_parameters: None,
            hard_handoff_parameters: None,
            is2000_channel_identity: None,
            is2000_service_configuration_record: None,
            is2000_non_negotiable_service_configuration_record: None,
        })
        .unwrap();
    commenced
        .on_handoff_commenced(&HandoffCommencedMessage)
        .unwrap();
    let commenced_timeout = commenced
        .on_timer_expired(SourceHandoffTimer::Clear)
        .unwrap();
    assert_eq!(commenced_timeout.new_state, SourceHandoffState::TimedOut);

    let mut arrival = TargetHandoffProcedure::new();
    arrival.on_handoff_request(&handoff_request()).unwrap();
    arrival
        .on_handoff_request_acknowledge(&HandoffRequestAcknowledgeMessage {
            is95_channel_identity: None,
            cell_identifier_list: None,
            extended_handoff_direction_parameters: None,
            hard_handoff_parameters: None,
            is2000_channel_identity: None,
            is2000_service_configuration_record: None,
            is2000_non_negotiable_service_configuration_record: None,
        })
        .unwrap();
    let arrival_timeout = arrival
        .on_timer_expired(TargetHandoffTimer::Arrival)
        .unwrap();
    assert_eq!(arrival_timeout.new_state, TargetHandoffState::TimedOut);
}

#[test]
fn procedure_engine_routes_mobile_terminated_flow() {
    let mut engine = ProcedureEngine::new();

    let event = engine
        .apply(
            ProcedureDirection::MscToBsc,
            &ProcedureMessage::PagingRequest(paging_request()),
        )
        .unwrap();
    assert!(matches!(event, EngineEvent::CallControl(_)));

    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::PagingResponse(paging_response()),
        )
        .unwrap();
    engine
        .apply(
            ProcedureDirection::MscToBsc,
            &ProcedureMessage::AssignmentRequest(assignment_request()),
        )
        .unwrap();
    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::AssignmentComplete(AssignmentCompleteMessage {
                channel_number: cdma_ios::ChannelNumber(0x1122),
                encryption_information: None,
                service_option: None,
                a2p_bearer_session_params: None,
                a2p_bearer_format_params: None,
            }),
        )
        .unwrap();
    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::Connect(ConnectMessage),
        )
        .unwrap();

    assert_eq!(engine.call_control().state(), CallControlState::Connected);
}

#[test]
fn procedure_engine_accepts_cm_service_request_direction() {
    let mut engine = ProcedureEngine::new();
    let event = engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::CmServiceRequest(CmServiceRequestMessage {
                cm_service_type: CmServiceType::MobileOriginatingCallEstablishment,
                classmark_information_type_2: ClassmarkInformationType2(vec![
                    0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
                ]),
                mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
                called_party_bcd_number: None,
                tag: None,
                mobile_identity_esn: None,
                slot_cycle_index: None,
                authentication_response_parameter: None,
                authentication_confirmation_parameter: None,
                authentication_parameter_count: None,
                authentication_challenge_parameter: None,
                service_option: None,
                voice_privacy_request: false,
                radio_environment_and_resources: None,
                called_party_ascii_number: None,
                circuit_identity_code: None,
                cdma_serving_one_way_delay: None,
                authentication_event: None,
                authentication_data: None,
                paca_reorigination_indicator: false,
                user_zone_id: None,
                is2000_mobile_capabilities: None,
            }),
        )
        .unwrap();
    assert!(matches!(event, EngineEvent::CallControl(_)));
    assert_eq!(
        engine.call_control().state(),
        CallControlState::AccessPending
    );
}

#[test]
fn procedure_engine_rejects_wrong_direction() {
    let mut engine = ProcedureEngine::new();
    let error = engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::PagingRequest(paging_request()),
        )
        .unwrap_err();
    assert_eq!(
        error,
        ProcedureError::InvalidDirection {
            procedure: "A1ProcedureEngine",
            message: "Paging Request",
            expected: ProcedureDirection::MscToBsc,
            actual: ProcedureDirection::BscToMsc,
        }
    );
}

#[test]
fn procedure_engine_routes_handoff_and_bs_service() {
    let mut engine = ProcedureEngine::new();

    let bs_event = engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::BsServiceRequest(BsServiceRequestMessage {
                mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
                mobile_identity_esn: None,
                service_option: None,
                tag: None,
            }),
        )
        .unwrap();
    assert!(matches!(bs_event, EngineEvent::BsService(_)));

    let handoff_event = engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::HandoffRequired(handoff_required()),
        )
        .unwrap();
    assert!(matches!(handoff_event, EngineEvent::SourceHandoff(_)));

    let performed_event = engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::HandoffPerformed(HandoffPerformedMessage {
                cause: cdma_ios::Cause(0x0e),
                cell_identifier_list: None,
            }),
        )
        .unwrap();
    assert!(matches!(performed_event, EngineEvent::SourceHandoff(_)));
}

#[test]
fn procedure_engine_accepts_spec_direction_call_control_messages() {
    let mut engine = ProcedureEngine::new();
    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::CompleteLayer3Information(complete_layer3_information()),
        )
        .unwrap();
    engine
        .apply(
            ProcedureDirection::MscToBsc,
            &ProcedureMessage::AssignmentRequest(assignment_request()),
        )
        .unwrap();
    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::AssignmentComplete(AssignmentCompleteMessage {
                channel_number: cdma_ios::ChannelNumber(0x1122),
                encryption_information: None,
                service_option: None,
                a2p_bearer_session_params: None,
                a2p_bearer_format_params: None,
            }),
        )
        .unwrap();

    assert!(matches!(
        engine
            .apply(
                ProcedureDirection::MscToBsc,
                &ProcedureMessage::Progress(ProgressMessage {
                    signal: None,
                    ms_information_records: None,
                }),
            )
            .unwrap(),
        EngineEvent::CallControl(_)
    ));
    assert!(matches!(
        engine
            .apply(
                ProcedureDirection::MscToBsc,
                &ProcedureMessage::AlertWithInformation(AlertWithInformationMessage {
                    ms_information_records: None,
                }),
            )
            .unwrap(),
        EngineEvent::CallControl(_)
    ));
    assert!(matches!(
        engine
            .apply(
                ProcedureDirection::BscToMsc,
                &ProcedureMessage::Connect(ConnectMessage)
            )
            .unwrap(),
        EngineEvent::CallControl(_)
    ));
}

#[test]
fn procedure_engine_accepts_bs_service_response_from_msc() {
    let mut engine = ProcedureEngine::new();
    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::BsServiceRequest(BsServiceRequestMessage {
                mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
                mobile_identity_esn: None,
                service_option: None,
                tag: Some(Tag(0x01020304)),
            }),
        )
        .unwrap();

    let event = engine
        .apply(
            ProcedureDirection::MscToBsc,
            &ProcedureMessage::BsServiceResponse(BsServiceResponseMessage {
                mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
                mobile_identity_esn: None,
                tag: Some(Tag(0x01020304)),
                cause: None,
            }),
        )
        .unwrap();
    assert!(matches!(event, EngineEvent::BsService(_)));
}

#[test]
fn procedure_engine_routes_timer_expiry() {
    let mut engine = ProcedureEngine::new();
    engine
        .apply(
            ProcedureDirection::BscToMsc,
            &ProcedureMessage::CompleteLayer3Information(complete_layer3_information()),
        )
        .unwrap();
    engine
        .apply(
            ProcedureDirection::MscToBsc,
            &ProcedureMessage::AssignmentRequest(assignment_request()),
        )
        .unwrap();

    let event = engine
        .on_timer_expired(EngineTimer::CallControl(CallControlTimer::Assignment))
        .unwrap();
    match event {
        EngineEvent::CallControl(transition) => {
            assert_eq!(transition.new_state, CallControlState::TimedOut);
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn expected_direction_matches_a1_message_roles() {
    assert_eq!(
        ProcedureMessage::Connect(ConnectMessage).expected_direction(),
        ProcedureDirection::BscToMsc
    );
    assert_eq!(
        ProcedureMessage::Progress(ProgressMessage {
            signal: Some(cdma_ios::Signal {
                signal_value: 0x00,
                alert_pitch: 0x00,
            }),
            ms_information_records: None,
        })
        .expected_direction(),
        ProcedureDirection::MscToBsc
    );
    assert_eq!(
        ProcedureMessage::AlertWithInformation(cdma_ios::AlertWithInformationMessage {
            ms_information_records: None,
        })
        .expected_direction(),
        ProcedureDirection::MscToBsc
    );
    assert_eq!(
        ProcedureMessage::BsServiceRequest(BsServiceRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            mobile_identity_esn: None,
            service_option: None,
            tag: None,
        })
        .expected_direction(),
        ProcedureDirection::BscToMsc
    );
    assert_eq!(
        ProcedureMessage::BsServiceResponse(BsServiceResponseMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            mobile_identity_esn: None,
            tag: None,
            cause: None,
        })
        .expected_direction(),
        ProcedureDirection::MscToBsc
    );
}
