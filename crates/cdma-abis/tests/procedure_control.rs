use cdma_abis::control::{
    A3ConnectAckInformation, A3ConnectInformation, A3RemoveInformation, AbisConnectInformation,
    AbisDestinationId, AbisMessage, AbisOriginatingId, AbisTimerKind, AccessTransferDispatch,
    AccessTransferKind, AccessTransferMessage, AccessTransferProcedure, AccessTransferState,
    AchMessageTransferMessage, AirInterfaceMessagePayload, AuthenticationChallengeParameter,
    BtsReleaseAckMessage, BtsReleaseMessage, BtsReleaseRequestDisposition,
    BtsReleaseRequestMessage, BtsSetupAckMessage, BtsSetupMessage, BurstAllocationProcedure,
    BurstAllocationState, BurstAllocationTimeoutAction, BurstCommitMessage, BurstRequestMessage,
    BurstResponseDisposition, BurstResponseMessage, CallConnectionReference,
    CdmaServingOneWayDelay, CellId, CellIdWithMscId, CellInfoRecord, ChannelElementStatus,
    ConnectAckMessage, ConnectMessage, CorrelationId, ElementId,
    ExtendedHandoffDirectionParameters, ForwardBurstRadioInfo, InformationElement, MobileIdentity,
    PacaActionRequired, PacaDisposition, PacaProcedure, PacaState, PacaUpdateMessage,
    PagingDispatch, PagingProcedure, PagingRequest, PagingState, PchMessageTransferAckMessage,
    PchMessageTransferMessage, PhysicalChannelInfo, PhysicalChannelType, PilotGatingRate,
    QualityOfServiceParameters, RemoveAckMessage, RemoveMessage, ReverseBurstRadioInfo,
    ServiceOption, SetupAckOutcome, TimerDefinition, TrafficChannelStatusMessage, TrafficCircuitId,
    TrafficReleaseProcedure, TrafficReleaseState, TrafficReleaseTimeoutAction,
    TrafficSetupProcedure, TrafficSetupState, TrafficSetupTimeoutAction,
};

fn call_ref() -> CallConnectionReference {
    CallConnectionReference {
        market_id: 0x0001,
        generating_entity_id: 0x0002,
        call_connection_reference: 0x0000_0003,
    }
}

fn connect_information() -> A3ConnectInformation {
    A3ConnectInformation {
        physical_channel_type: PhysicalChannelType::Fch,
        new_a3: true,
        cell_info_records: vec![CellInfoRecord {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            qof_mask: 0x02,
            new_cell: true,
            power_combine_indication: false,
            pilot_pn: 0x101,
            code_channel: 0x22,
        }],
        traffic_circuit_id: TrafficCircuitId {
            traffic_circuit_identifier: 0x1020,
            traffic_connection_identifier: 0x33,
        },
        extended_handoff_direction_parameters: Some(ExtendedHandoffDirectionParameters {
            search_window_a_size: 1,
            search_window_n_size: 2,
            search_window_r_size: 3,
            t_add: 4,
            t_drop: 5,
            compare_threshold: 6,
            drop_timer_value: 7,
            neighbor_max_age: 8,
            soft_slope: 9,
            add_intercept: 10,
            drop_intercept: 11,
            target_bs_p_rev: 12,
        }),
        channel_element_id: vec![0xaa, 0xbb],
        a3_originating_id: 0x2001,
        a7_destination_id: 0x2002,
    }
}

fn physical_channel_info() -> PhysicalChannelInfo {
    PhysicalChannelInfo {
        frame_offset: 0x22,
        pilot_gating_rate: PilotGatingRate::Half,
        arfcn: 0x345,
        otd: true,
        physical_channels: vec![PhysicalChannelType::Fch, PhysicalChannelType::Dcch],
    }
}

fn setup_message() -> BtsSetupMessage {
    BtsSetupMessage {
        call_connection_reference: call_ref(),
        band_class: None,
        privacy_info: None,
        sdu_id: None,
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        physical_channel_info: Some(physical_channel_info()),
        service_option: Some(ServiceOption(0x0021)),
        paca_timestamp: None,
        quality_of_service_parameters: Some(QualityOfServiceParameters {
            packet_priority: 0x0a,
        }),
        connect_information: vec![AbisConnectInformation::from(connect_information())],
        abis_originating_id: Some(AbisOriginatingId::new([0x44, 0x44]).unwrap()),
        cdma_serving_one_way_delay: cdma_abis::control::CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x3344,
        },
        cdma_target_one_way_delay: None,
        walsh_code_assignment_request: true,
    }
}

fn setup_ack_message(cause: Option<u8>) -> BtsSetupAckMessage {
    BtsSetupAckMessage {
        call_connection_reference: call_ref(),
        connect_information: vec![AbisConnectInformation::from(connect_information())],
        abis_originating_id: Some(AbisOriginatingId::new([0x11, 0x11]).unwrap()),
        abis_destination_id: None,
        cause,
    }
}

fn connect_message(correlation_id: Option<CorrelationId>) -> ConnectMessage {
    ConnectMessage {
        call_connection_reference: call_ref(),
        correlation_id,
        sdu_id: None,
        connect_information: vec![connect_information()],
        physical_channel_info: physical_channel_info(),
    }
}

fn connect_ack_message(correlation_id: Option<CorrelationId>) -> ConnectAckMessage {
    ConnectAckMessage {
        call_connection_reference: call_ref(),
        correlation_id,
        connect_ack_information: vec![A3ConnectAckInformation {
            soft_handoff_leg: 2,
            pmc_cause: None,
            transmit_tch_status: true,
            traffic_circuit_id: TrafficCircuitId {
                traffic_circuit_identifier: 0x1020,
                traffic_connection_identifier: 0x33,
            },
            channel_element_id: vec![0xaa, 0xbb],
            a3_originating_id: 0x2001,
            a3_destination_id: 0x2002,
        }],
    }
}

fn traffic_status_message() -> TrafficChannelStatusMessage {
    TrafficChannelStatusMessage {
        call_connection_reference: call_ref(),
        cell_identifier_list: vec![CellIdWithMscId {
            mscid: 0x001234,
            cell: 0x123,
            sector: 0x4,
        }],
        channel_element_status: ChannelElementStatus { transmit_on: true },
        sdu_id: None,
        a3_destination_id: Some(0x2001),
        a7_destination_id: Some(0x2002),
    }
}

fn remove_message(correlation_id: Option<CorrelationId>) -> RemoveMessage {
    RemoveMessage {
        call_connection_reference: call_ref(),
        correlation_id,
        sdu_id: None,
        remove_information: vec![A3RemoveInformation {
            traffic_circuit_id: TrafficCircuitId {
                traffic_circuit_identifier: 0x1020,
                traffic_connection_identifier: 0x33,
            },
            cells_to_be_removed: vec![CellIdWithMscId {
                mscid: 0x001234,
                cell: 0x123,
                sector: 0x4,
            }],
            a3_destination_id: 0x2001,
            a7_destination_id: 0x2002,
        }],
    }
}

fn paging_message(with_correlation: bool) -> AbisMessage {
    let mut elements = vec![InformationElement::new(
        ElementId::MobileIdentity,
        MobileIdentity::Imsi("12345678901".to_string())
            .encode()
            .unwrap(),
    )];
    if with_correlation {
        elements.insert(
            0,
            InformationElement::new(ElementId::CorrelationId, CorrelationId(0x01020304).encode()),
        );
    }
    elements.push(InformationElement::new(
        ElementId::AirInterfaceMessage,
        [0xca, 0x02, 0xba, 0xbe],
    ));
    elements.push(InformationElement::new(
        ElementId::Layer2AckRequestResults,
        [0x01],
    ));
    elements.push(InformationElement::new(ElementId::AbisAckNotify, []));
    AbisMessage {
        message_type: cdma_abis::control::MessageType::PchMessageTransfer,
        elements,
    }
}

fn burst_request_message() -> BurstRequestMessage {
    BurstRequestMessage {
        call_connection_reference: Some(call_ref()),
        band_class: None,
        downlink_radio_environment: None,
        cdma_serving_one_way_delay: None,
        privacy_info: None,
        correlation_id: Some(CorrelationId(0x01020304)),
        sdu_id: None,
        mobile_identities: vec![],
        cell_identifier_list: Some(vec![
            CellId {
                cell: 0x123,
                sector: 0x4,
            },
            CellId {
                cell: 0x124,
                sector: 0x5,
            },
        ]),
        forward_burst_radio_info: None,
        reverse_burst_radio_info: None,
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    }
}

fn burst_response_message() -> BurstResponseMessage {
    BurstResponseMessage {
        call_connection_reference: Some(call_ref()),
        correlation_id: Some(CorrelationId(0x01020304)),
        committed_cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        uncommitted_cell_identifier_list: Some(vec![CellId {
            cell: 0x124,
            sector: 0x5,
        }]),
        forward_burst_radio_info: None,
        reverse_burst_radio_info: None,
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    }
}

fn burst_commit_message() -> BurstCommitMessage {
    BurstCommitMessage {
        call_connection_reference: Some(call_ref()),
        correlation_id: Some(CorrelationId(0x01020304)),
        forward_cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        reverse_cell_identifier_list: Some(vec![CellId {
            cell: 0x124,
            sector: 0x5,
        }]),
        forward_burst_radio_info: None,
        reverse_burst_radio_info: None,
        is2000_forward_power_control_mode: None,
        is2000_fpc_gain_ratio_info: None,
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    }
}

fn ach_message(with_air_interface_message: bool, with_mobile_identity: bool) -> AbisMessage {
    let mut elements = Vec::new();
    elements.push(InformationElement::new(
        ElementId::CorrelationId,
        CorrelationId(0x01020304).encode(),
    ));
    if with_mobile_identity {
        elements.push(InformationElement::new(
            ElementId::MobileIdentity,
            MobileIdentity::Imsi("12345678901".to_string())
                .encode()
                .unwrap(),
        ));
    }
    elements.push(InformationElement::new(
        ElementId::CellIdentifier,
        CellId {
            cell: 0x123,
            sector: 0x4,
        }
        .encode()
        .unwrap(),
    ));
    elements.push(InformationElement::new(ElementId::BtsL2Termination, [0x01]));
    if with_air_interface_message {
        elements.push(InformationElement::new(
            ElementId::AirInterfaceMessage,
            [0xde, 0x02, 0xbe, 0xef],
        ));
    }
    elements.push(InformationElement::new(
        ElementId::CdmaServingOneWayDelay,
        CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x3344,
        }
        .encode()
        .unwrap(),
    ));
    elements.push(InformationElement::new(
        ElementId::AuthenticationChallengeParameter,
        [0x01, 0x02, 0x03, 0x04, 0x05],
    ));
    AbisMessage {
        message_type: cdma_abis::control::MessageType::AchMessageTransfer,
        elements,
    }
}

fn typed_pch_message(with_ack: bool) -> PchMessageTransferMessage {
    PchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        air_interface_message: Some(AirInterfaceMessagePayload::new(0xca, [0xba, 0xbe]).unwrap()),
        layer2_ack_request_results: with_ack
            .then_some(cdma_abis::control::Layer2AckRequestResults::request()),
        abis_ack_notify: with_ack.then_some(cdma_abis::control::AbisAckNotify),
    }
}

fn typed_ach_message(correlation_id: Option<CorrelationId>) -> AchMessageTransferMessage {
    AchMessageTransferMessage {
        correlation_id,
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        cell_identifier: Some(CellId {
            cell: 0x123,
            sector: 0x4,
        }),
        bts_l2_termination: Some(true),
        air_interface_message: Some(AirInterfaceMessagePayload::new(0xde, [0xbe, 0xef]).unwrap()),
        cdma_serving_one_way_delay: CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x3344,
        },
        authentication_challenge_parameter: Some(AuthenticationChallengeParameter::new([
            0x01, 0x02, 0x03, 0x04,
        ])),
    }
}

#[test]
fn traffic_setup_procedure_happy_path() {
    let mut procedure = TrafficSetupProcedure::new(call_ref());

    procedure.start_setup(&setup_message()).unwrap();
    assert_eq!(procedure.state(), TrafficSetupState::AwaitingConnect);

    procedure
        .on_connect(&connect_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(procedure.state(), TrafficSetupState::AwaitingConnectAck);

    procedure
        .on_connect_ack(&connect_ack_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(
        procedure.state(),
        TrafficSetupState::AwaitingSetupCompletion
    );

    let outcome = procedure.on_setup_ack(&setup_ack_message(None)).unwrap();
    assert_eq!(outcome, SetupAckOutcome::Accepted);
    assert_eq!(
        procedure.state(),
        TrafficSetupState::AwaitingSetupCompletion
    );

    let status = traffic_status_message();
    procedure.on_traffic_channel_status(&status).unwrap();
    assert_eq!(procedure.last_status(), Some(&status));
    assert_eq!(procedure.state(), TrafficSetupState::Connected);
}

#[test]
fn traffic_setup_procedure_rejects_out_of_order_setup_ack() {
    let mut procedure = TrafficSetupProcedure::new(call_ref());
    procedure.start_setup(&setup_message()).unwrap();

    let error = procedure
        .on_setup_ack(&setup_ack_message(None))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BtsSetupAck.value(),
            reason: "unexpected BTS Setup Ack for current procedure state",
        }
    );
}

#[test]
fn traffic_setup_procedure_rejects_correlation_mismatch() {
    let mut procedure = TrafficSetupProcedure::new(call_ref());
    procedure.start_setup(&setup_message()).unwrap();
    procedure
        .on_connect(&connect_message(Some(CorrelationId(0x01020304))))
        .unwrap();

    let error = procedure
        .on_connect_ack(&connect_ack_message(Some(CorrelationId(0xaabbccdd))))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::ConnectAck.value(),
            reason: "correlation identifier mismatch",
        }
    );
}

#[test]
fn traffic_setup_procedure_tracks_setup_reject() {
    let mut procedure = TrafficSetupProcedure::new(call_ref());
    procedure.start_setup(&setup_message()).unwrap();
    procedure
        .on_connect(&connect_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    procedure
        .on_connect_ack(&connect_ack_message(Some(CorrelationId(0x01020304))))
        .unwrap();

    let outcome = procedure
        .on_setup_ack(&setup_ack_message(Some(0x21)))
        .unwrap();
    assert_eq!(outcome, SetupAckOutcome::Rejected { cause: 0x21 });
    assert_eq!(procedure.state(), TrafficSetupState::Failed);
}

#[test]
fn traffic_release_procedure_bsc_originated_flow() {
    let mut procedure = TrafficReleaseProcedure::new(call_ref());
    let release = BtsReleaseMessage {
        call_connection_reference: call_ref(),
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        correlation_id: Some(CorrelationId(0x01020304)),
    };

    procedure.start_release(&release).unwrap();
    assert_eq!(
        procedure.state(),
        TrafficReleaseState::AwaitingBtsReleaseAck
    );

    procedure
        .on_release_ack(&BtsReleaseAckMessage {
            call_connection_reference: call_ref(),
            correlation_id: Some(CorrelationId(0x01020304)),
        })
        .unwrap();
    assert_eq!(procedure.state(), TrafficReleaseState::Released);
}

#[test]
fn traffic_release_procedure_rejects_release_ack_before_release() {
    let mut procedure = TrafficReleaseProcedure::new(call_ref());

    let error = procedure
        .on_release_ack(&BtsReleaseAckMessage {
            call_connection_reference: call_ref(),
            correlation_id: Some(CorrelationId(0x01020304)),
        })
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BtsReleaseAck.value(),
            reason: "unexpected BTS Release Ack for current procedure state",
        }
    );
}

#[test]
fn traffic_release_procedure_bts_originated_remove_flow() {
    let mut procedure = TrafficReleaseProcedure::new(call_ref());
    let disposition = procedure
        .on_release_request(&BtsReleaseRequestMessage {
            call_connection_reference: call_ref(),
            cause: Some(0x10),
            manufacturer_specific_records: None,
        })
        .unwrap();
    assert_eq!(
        disposition,
        BtsReleaseRequestDisposition {
            cause: Some(0x10),
            has_manufacturer_specific_records: false,
        }
    );
    assert_eq!(procedure.state(), TrafficReleaseState::AwaitingRemove);

    procedure
        .on_remove(&remove_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(procedure.state(), TrafficReleaseState::AwaitingRemoveAck);

    procedure
        .on_remove_ack(&RemoveAckMessage {
            call_connection_reference: call_ref(),
            correlation_id: Some(CorrelationId(0x01020304)),
            a3_destination_id: Some(0x2001),
        })
        .unwrap();
    assert_eq!(procedure.state(), TrafficReleaseState::Released);
}

#[test]
fn traffic_release_procedure_rejects_remove_ack_before_remove() {
    let mut procedure = TrafficReleaseProcedure::new(call_ref());

    let error = procedure
        .on_remove_ack(&RemoveAckMessage {
            call_connection_reference: call_ref(),
            correlation_id: Some(CorrelationId(0x01020304)),
            a3_destination_id: Some(0x2001),
        })
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::RemoveAck.value(),
            reason: "unexpected Remove Ack for current procedure state",
        }
    );
}

#[test]
fn paging_procedure_tracks_fixture_style_ack_flow() {
    let fixture_bytes = cdma_abis::control::encode(&paging_message(true)).unwrap();
    let decoded = cdma_abis::control::decode(&fixture_bytes).unwrap();
    let request = PagingRequest::try_from(&decoded).unwrap();
    assert!(request.ack_expected());
    assert_eq!(request.mobile_identity_count, 1);

    let mut procedure = PagingProcedure::new();
    let dispatch = procedure.start_message(&decoded).unwrap();
    assert_eq!(dispatch, PagingDispatch::AwaitingAck);
    assert_eq!(procedure.state(), PagingState::AwaitingAck);

    let outcome = procedure
        .on_ack(&cdma_abis::control::PchMessageTransferAckMessage {
            correlation_id: Some(CorrelationId(0x01020304)),
            cause: None,
            bts_l2_termination: Some(true),
        })
        .unwrap();
    assert_eq!(outcome.cause, None);
    assert_eq!(outcome.bts_l2_termination, Some(true));
    assert_eq!(procedure.state(), PagingState::Completed);
    assert_eq!(procedure.last_outcome(), Some(outcome));
}

#[test]
fn paging_procedure_rejects_ack_tracking_without_correlation() {
    let mut procedure = PagingProcedure::new();
    let error = procedure.start_message(&paging_message(false)).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::PchMessageTransfer.value(),
            reason: "paging ack tracking requires a correlation identifier",
        }
    );
}

#[test]
fn burst_allocation_procedure_happy_path() {
    let mut procedure = BurstAllocationProcedure::new();
    procedure.start_request(&burst_request_message()).unwrap();
    assert_eq!(procedure.state(), BurstAllocationState::AwaitingResponse);

    let disposition = procedure.on_response(&burst_response_message()).unwrap();
    assert_eq!(
        disposition,
        BurstResponseDisposition {
            committed_cells: 1,
            uncommitted_cells: 1,
            awaiting_more_cells: false,
        }
    );
    assert_eq!(procedure.state(), BurstAllocationState::AwaitingCommit);

    procedure.on_commit(&burst_commit_message()).unwrap();
    assert_eq!(procedure.state(), BurstAllocationState::Committed);
}

#[test]
fn burst_allocation_procedure_rejects_commit_before_response() {
    let mut procedure = BurstAllocationProcedure::new();
    procedure.start_request(&burst_request_message()).unwrap();

    let error = procedure.on_commit(&burst_commit_message()).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BurstCommit.value(),
            reason: "unexpected Burst Commit for current procedure state",
        }
    );
}

#[test]
fn burst_allocation_procedure_rejects_identifier_mismatch() {
    let mut procedure = BurstAllocationProcedure::new();
    procedure.start_request(&burst_request_message()).unwrap();

    let mut response = burst_response_message();
    response.correlation_id = Some(CorrelationId(0xaabbccdd));
    let error = procedure.on_response(&response).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BurstResponse.value(),
            reason: "correlation identifier mismatch",
        }
    );
}

#[test]
fn burst_allocation_procedure_rejects_commit_cell_not_offered() {
    let mut procedure = BurstAllocationProcedure::new();
    procedure.start_request(&burst_request_message()).unwrap();
    procedure.on_response(&burst_response_message()).unwrap();

    let mut commit = burst_commit_message();
    commit.reverse_cell_identifier_list = Some(vec![CellId {
        cell: 0x999,
        sector: 0x1,
    }]);
    let error = procedure.on_commit(&commit).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BurstCommit.value(),
            reason: "reverse burst commit cell was not offered in Burst Response",
        }
    );
}

#[test]
fn burst_allocation_procedure_requires_tracking_key() {
    let mut procedure = BurstAllocationProcedure::new();
    let mut request = burst_request_message();
    request.call_connection_reference = None;
    request.correlation_id = None;

    let error = procedure.start_request(&request).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BurstRequest.value(),
            reason: "burst reservation tracking requires call reference or correlation identifier",
        }
    );
}

#[test]
fn access_transfer_parser_handles_ach_and_pch() {
    let ach = AccessTransferMessage::try_from(&ach_message(true, true)).unwrap();
    assert_eq!(ach.kind, AccessTransferKind::AccessChannel);
    assert_eq!(ach.correlation_id, Some(CorrelationId(0x01020304)));
    assert_eq!(ach.mobile_identity_count, 1);
    assert!(ach.has_cell_identifier);
    assert!(ach.has_air_interface_message);
    assert!(ach.has_authentication_challenge);
    assert_eq!(ach.bts_l2_termination, Some(true));

    let pch = AccessTransferMessage::try_from(&paging_message(true)).unwrap();
    assert_eq!(pch.kind, AccessTransferKind::PagingChannel);
    assert!(pch.ack_expected());
    assert_eq!(pch.mobile_identity_count, 1);
}

#[test]
fn access_transfer_parser_allows_missing_air_interface_message() {
    let ach = AccessTransferMessage::try_from(&ach_message(false, true)).unwrap();
    assert_eq!(ach.kind, AccessTransferKind::AccessChannel);
    assert!(!ach.has_air_interface_message);
}

#[test]
fn access_transfer_parser_rejects_auth_challenge_without_identity() {
    let error = AccessTransferMessage::try_from(&ach_message(true, false)).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::AchMessageTransfer.value(),
            reason: "ACH Msg Transfer with authentication challenge requires a mobile identity",
        }
    );
}

#[test]
fn access_transfer_parser_rejects_multiple_pch_mobile_identities() {
    let message = AbisMessage {
        message_type: cdma_abis::control::MessageType::PchMessageTransfer,
        elements: vec![
            InformationElement::new(
                ElementId::MobileIdentity,
                MobileIdentity::Imsi("12345678901".to_string())
                    .encode()
                    .unwrap(),
            ),
            InformationElement::new(
                ElementId::MobileIdentity,
                MobileIdentity::Esn(0x01020304).encode().unwrap(),
            ),
            InformationElement::new(ElementId::AirInterfaceMessage, [0xca, 0x02, 0xba, 0xbe]),
        ],
    };
    assert_eq!(
        AccessTransferMessage::try_from(&message).unwrap_err(),
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::PchMessageTransfer.value(),
            reason: "PCH Msg Transfer may carry at most one mobile identity",
        }
    );
}

#[test]
fn access_transfer_parser_rejects_ack_notify_without_layer2_ack_request() {
    let message = AbisMessage {
        message_type: cdma_abis::control::MessageType::PchMessageTransfer,
        elements: vec![
            InformationElement::new(ElementId::CorrelationId, CorrelationId(0x01020304).encode()),
            InformationElement::new(ElementId::AirInterfaceMessage, [0xca, 0x02, 0xba, 0xbe]),
            InformationElement::new(ElementId::AbisAckNotify, []),
        ],
    };
    assert_eq!(
        AccessTransferMessage::try_from(&message).unwrap_err(),
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::PchMessageTransfer.value(),
            reason: "Abis Ack Notify requires Layer 2 Ack Request/Results",
        }
    );
}

#[test]
fn access_transfer_procedure_enforces_paging_then_access_order() {
    let mut procedure = AccessTransferProcedure::new();
    let dispatch = procedure
        .on_paging_transfer(&typed_pch_message(true))
        .unwrap();
    assert_eq!(dispatch, AccessTransferDispatch::AwaitingPagingAck);
    assert_eq!(procedure.state(), AccessTransferState::AwaitingPagingAck);

    procedure
        .on_paging_ack(&PchMessageTransferAckMessage {
            correlation_id: Some(CorrelationId(0x01020304)),
            cause: None,
            bts_l2_termination: Some(true),
        })
        .unwrap();
    assert_eq!(
        procedure.state(),
        AccessTransferState::AwaitingAccessChannel
    );

    procedure
        .on_access_transfer(&typed_ach_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(procedure.state(), AccessTransferState::AccessReceived);
}

#[test]
fn access_transfer_procedure_allows_ack_only_paging_then_access_channel() {
    let mut procedure = AccessTransferProcedure::new();
    let mut paging = typed_pch_message(true);
    paging.air_interface_message = None;

    let dispatch = procedure.on_paging_transfer(&paging).unwrap();
    assert_eq!(dispatch, AccessTransferDispatch::AwaitingPagingAck);
    assert_eq!(procedure.state(), AccessTransferState::AwaitingPagingAck);

    procedure
        .on_paging_ack(&PchMessageTransferAckMessage {
            correlation_id: Some(CorrelationId(0x01020304)),
            cause: None,
            bts_l2_termination: Some(true),
        })
        .unwrap();

    let mut access = typed_ach_message(Some(CorrelationId(0x01020304)));
    access.air_interface_message = None;
    procedure.on_access_transfer(&access).unwrap();
    assert_eq!(procedure.state(), AccessTransferState::AccessReceived);
}

#[test]
fn access_transfer_procedure_rejects_ach_before_paging_stage() {
    let mut procedure = AccessTransferProcedure::new();
    let error = procedure
        .on_access_transfer(&typed_ach_message(Some(CorrelationId(0x01020304))))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::AchMessageTransfer.value(),
            reason: "ACH transfer is out of order for current access-transfer state",
        }
    );
}

#[test]
fn abis_timer_definitions_match_spec_table() {
    assert_eq!(
        AbisTimerKind::Tconnb.definition(),
        TimerDefinition {
            default_ms: 100,
            min_ms: 0,
            max_ms: 1000,
            granularity_ms: 100,
        }
    );
    assert_eq!(
        AbisTimerKind::Tsetupb.definition(),
        TimerDefinition {
            default_ms: 100,
            min_ms: 0,
            max_ms: 500,
            granularity_ms: 100,
        }
    );
    assert_eq!(AbisTimerKind::Trelreqb.definition().default_ms, 100);
    assert_eq!(AbisTimerKind::Tbstreqb.definition().default_ms, 500);
    assert_eq!(AbisTimerKind::Tbstcomb.definition().default_ms, 500);
}

#[test]
fn traffic_setup_procedure_tracks_spec_timers() {
    let mut procedure = TrafficSetupProcedure::new(call_ref());
    procedure.start_setup(&setup_message()).unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tsetupb]);

    procedure
        .on_connect(&connect_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tconnb]);

    procedure
        .on_connect_ack(&connect_ack_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tchanstatb]);
    assert_eq!(procedure.pending_status_count(), 1);

    procedure.on_setup_ack(&setup_ack_message(None)).unwrap();
    assert_eq!(
        procedure.state(),
        TrafficSetupState::AwaitingSetupCompletion
    );

    procedure
        .on_traffic_channel_status(&traffic_status_message())
        .unwrap();
    assert!(procedure.active_timers().is_empty());
    assert_eq!(procedure.pending_status_count(), 0);
}

#[test]
fn traffic_setup_timeout_actions_follow_spec() {
    let mut procedure = TrafficSetupProcedure::new(call_ref());
    procedure.start_setup(&setup_message()).unwrap();
    assert_eq!(
        procedure.on_timer_expiry(AbisTimerKind::Tsetupb).unwrap(),
        TrafficSetupTimeoutAction::ResendBtsSetup
    );

    procedure
        .on_connect(&connect_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(
        procedure.on_timer_expiry(AbisTimerKind::Tconnb).unwrap(),
        TrafficSetupTimeoutAction::ResendConnect
    );

    procedure
        .on_connect_ack(&connect_ack_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(
        procedure
            .on_timer_expiry(AbisTimerKind::Tchanstatb)
            .unwrap(),
        TrafficSetupTimeoutAction::ReleaseUnreportedCells
    );
}

#[test]
fn traffic_release_procedure_tracks_spec_timers() {
    let mut procedure = TrafficReleaseProcedure::new(call_ref());
    procedure
        .on_release_request(&BtsReleaseRequestMessage {
            call_connection_reference: call_ref(),
            cause: Some(0x10),
            manufacturer_specific_records: None,
        })
        .unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Trelreqb]);
    assert_eq!(
        procedure.on_timer_expiry(AbisTimerKind::Trelreqb).unwrap(),
        TrafficReleaseTimeoutAction::ResendBtsReleaseRequest
    );

    let mut procedure = TrafficReleaseProcedure::new(call_ref());
    let release = BtsReleaseMessage {
        call_connection_reference: call_ref(),
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        correlation_id: Some(CorrelationId(0x01020304)),
    };
    procedure.start_release(&release).unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tdrptgtb]);
    assert_eq!(
        procedure.on_timer_expiry(AbisTimerKind::Tdrptgtb).unwrap(),
        TrafficReleaseTimeoutAction::ResendBtsRelease
    );

    let mut procedure = TrafficReleaseProcedure::new(call_ref());
    procedure
        .on_remove(&remove_message(Some(CorrelationId(0x01020304))))
        .unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tdisconb]);
    assert_eq!(
        procedure.on_timer_expiry(AbisTimerKind::Tdisconb).unwrap(),
        TrafficReleaseTimeoutAction::ResendRemove
    );
}

#[test]
fn burst_allocation_procedure_tracks_multi_response_and_timers() {
    let mut procedure = BurstAllocationProcedure::new();
    procedure.start_request(&burst_request_message()).unwrap();
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tbstreqb]);

    let first = BurstResponseMessage {
        committed_cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        uncommitted_cell_identifier_list: None,
        ..burst_response_message()
    };
    let disposition = procedure.on_response(&first).unwrap();
    assert_eq!(
        disposition,
        BurstResponseDisposition {
            committed_cells: 1,
            uncommitted_cells: 0,
            awaiting_more_cells: true,
        }
    );
    assert_eq!(procedure.state(), BurstAllocationState::AwaitingResponse);
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tbstreqb]);

    let second = BurstResponseMessage {
        committed_cell_identifier_list: None,
        uncommitted_cell_identifier_list: Some(vec![CellId {
            cell: 0x124,
            sector: 0x5,
        }]),
        ..burst_response_message()
    };
    let disposition = procedure.on_response(&second).unwrap();
    assert_eq!(
        disposition,
        BurstResponseDisposition {
            committed_cells: 0,
            uncommitted_cells: 1,
            awaiting_more_cells: false,
        }
    );
    assert_eq!(procedure.state(), BurstAllocationState::AwaitingCommit);
    assert_eq!(procedure.active_timers(), vec![AbisTimerKind::Tbstcomb]);

    assert_eq!(
        procedure.on_timer_expiry(AbisTimerKind::Tbstcomb).unwrap(),
        BurstAllocationTimeoutAction::DecommitReservedResources
    );
    assert_eq!(procedure.state(), BurstAllocationState::Idle);
}

#[test]
fn burst_allocation_procedure_rejects_commit_rate_upgrade() {
    let mut procedure = BurstAllocationProcedure::new();
    let mut request = burst_request_message();
    request.forward_burst_radio_info = Some(ForwardBurstRadioInfo {
        coding_indicator: 1,
        qof_mask: 2,
        forward_code_channel_index: 0x123,
        pilot_pn_code: 0x101,
        forward_supplemental_channel_rate: 0x03,
        forward_supplemental_channel_start_time: 0x1b,
        start_time_unit: 0x05,
        forward_supplemental_channel_duration: 0x0c,
    });
    request.reverse_burst_radio_info = Some(ReverseBurstRadioInfo {
        coding_indicator: 1,
        reverse_supplemental_channel_rate: 0x03,
        reverse_supplemental_channel_start_time: 0x44,
        start_time_unit: 0x05,
        reverse_supplemental_channel_duration: 0x0c,
    });
    procedure.start_request(&request).unwrap();

    let mut response = burst_response_message();
    response.forward_burst_radio_info = Some(ForwardBurstRadioInfo {
        coding_indicator: 1,
        qof_mask: 2,
        forward_code_channel_index: 0x123,
        pilot_pn_code: 0x101,
        forward_supplemental_channel_rate: 0x03,
        forward_supplemental_channel_start_time: 0x1b,
        start_time_unit: 0x05,
        forward_supplemental_channel_duration: 0x0c,
    });
    response.reverse_burst_radio_info = Some(ReverseBurstRadioInfo {
        coding_indicator: 1,
        reverse_supplemental_channel_rate: 0x03,
        reverse_supplemental_channel_start_time: 0x44,
        start_time_unit: 0x05,
        reverse_supplemental_channel_duration: 0x0c,
    });
    procedure.on_response(&response).unwrap();

    let mut commit = burst_commit_message();
    commit.forward_burst_radio_info = Some(ForwardBurstRadioInfo {
        coding_indicator: 1,
        qof_mask: 2,
        forward_code_channel_index: 0x123,
        pilot_pn_code: 0x101,
        forward_supplemental_channel_rate: 0x05,
        forward_supplemental_channel_start_time: 0x1b,
        start_time_unit: 0x05,
        forward_supplemental_channel_duration: 0x0c,
    });
    let error = procedure.on_commit(&commit).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::BurstCommit.value(),
            reason: "forward burst commit rate exceeds Burst Response",
        }
    );
}

#[test]
fn paca_procedure_applies_and_blocks_conflicting_updates() {
    let mut procedure = PacaProcedure::new(call_ref());

    let disposition = procedure
        .apply_update(&PacaUpdateMessage {
            call_connection_reference: call_ref(),
            mobile_identity_imsi: Some(MobileIdentity::Imsi("12345678901".to_string())),
            action_required: Some(PacaActionRequired::UpdateQueuePosition),
        })
        .unwrap();
    assert_eq!(disposition, PacaDisposition::UpdateQueuePosition);
    assert_eq!(procedure.state(), PacaState::QueuePositionUpdated);
    assert_eq!(procedure.mobile_identity_imsi(), Some("12345678901"));

    let disposition = procedure
        .apply_update(&PacaUpdateMessage {
            call_connection_reference: call_ref(),
            mobile_identity_imsi: Some(MobileIdentity::Imsi("12345678901".to_string())),
            action_required: Some(PacaActionRequired::RemoveMsFromQueue),
        })
        .unwrap();
    assert_eq!(disposition, PacaDisposition::RemoveMsFromQueue);
    assert_eq!(procedure.state(), PacaState::Removed);

    let error = procedure
        .apply_update(&PacaUpdateMessage {
            call_connection_reference: call_ref(),
            mobile_identity_imsi: Some(MobileIdentity::Imsi("12345678901".to_string())),
            action_required: Some(PacaActionRequired::UpdateQueuePosition),
        })
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::PacaUpdate.value(),
            reason: "queue-position update is invalid after PACA removal",
        }
    );
}

#[test]
fn paca_procedure_rejects_esn_identity() {
    let mut procedure = PacaProcedure::new(call_ref());
    let error = procedure
        .apply_update(&PacaUpdateMessage {
            call_connection_reference: call_ref(),
            mobile_identity_imsi: Some(MobileIdentity::Esn(0x01020304)),
            action_required: None,
        })
        .unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidMessage {
            message_type: cdma_abis::control::MessageType::PacaUpdate.value(),
            reason: "PACA Update mobile identity must be IMSI when present",
        }
    );
}
