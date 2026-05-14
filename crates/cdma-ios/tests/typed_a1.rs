use cdma_ios::{
    // ADDS messages
    AddsDeliverAckMessage,
    AddsDeliverMessage,
    AddsPageAckMessage,
    AddsPageMessage,
    AddsTransferMessage,
    AddsUserPart,
    AlertWithInformationMessage,
    AssignmentCompleteMessage,
    AssignmentFailureMessage,
    AssignmentRequestMessage,
    AuthenticationChallengeParameter,
    AuthenticationConfirmationParameter,
    AuthenticationData,
    AuthenticationEvent,
    AuthenticationParameterCount,
    AuthenticationRequestBsmapMessage,
    AuthenticationRequestDtapMessage,
    AuthenticationRequestMessage,
    AuthenticationResponseBsmapMessage,
    AuthenticationResponseDtapMessage,
    AuthenticationResponseMessage,
    AuthenticationResponseParameter,
    BaseStationChallengeMessage,
    BaseStationChallengeResponseMessage,
    BsServiceRequestMessage,
    BsServiceResponseMessage,
    CallingPartyAsciiNumber,
    Cause,
    CauseLayer3,
    CellId,
    CellIdentifierList,
    ChannelNumber,
    ChannelType,
    CircuitIdentityCode,
    CircuitIdentityCodeExtension,
    ClassmarkInformationType2,
    ClearCommandMessage,
    ClearCompleteMessage,
    ClearRequestMessage,
    CmServiceRequestMessage,
    CmServiceType,
    CompleteLayer3InformationMessage,
    ConnectMessage,
    EncryptionInformation,
    EncryptionParameter,
    ExtendedHandoffDirectionParameters,
    HandoffCdmaServingOneWayDelay,
    HandoffCellIdentifier,
    HandoffCellIdentifierList,
    HandoffCommandMessage,
    HandoffCommencedMessage,
    HandoffCompleteMessage,
    HandoffDownlinkRadioEnvironment,
    HandoffDownlinkRadioEnvironmentRecord,
    HandoffFailureMessage,
    HandoffPerformedMessage,
    HandoffRequestAcknowledgeMessage,
    HandoffRequestMessage,
    HandoffRequiredMessage,
    HandoffRequiredRejectMessage,
    HardHandoffParameters,
    Is95ChannelEntry,
    Is95ChannelIdentity,
    Is95MsMeasuredChannelIdentity,
    Is2000ChannelEntry,
    Is2000ChannelIdentity,
    Is2000MobileCapabilities,
    Is2000NonNegotiableServiceConfigurationRecord,
    Is2000PhysicalChannelType,
    Is2000ServiceConfigurationRecord,
    Layer3Information,
    LocationAreaIdentification,
    LocationUpdatingAcceptMessage,
    LocationUpdatingRejectMessage,
    LocationUpdatingRequestMessage,
    MobileIdentity,
    MsInformationRecord,
    MsInformationRecords,
    PacaTimestamp,
    PagingRequestMessage,
    PagingResponseMessage,
    ParameterUpdateConfirmMessage,
    ParameterUpdateRequestMessage,
    PdsnIpAddress,
    Priority,
    PrivacyModeCommandMessage,
    PrivacyModeCompleteMessage,
    ProgressMessage,
    ProtocolType,
    QualityOfServiceParameters,
    RadioEnvironmentAndResources,
    RegistrationType,
    RejectCause,
    RfChannelIdentity,
    ServiceOption,
    Signal,
    SlotCycleIndex,
    SsdUpdateChallengeParameter,
    SsdUpdateRequestMessage,
    SsdUpdateResponseMessage,
    Tag,
    UserZoneId,
    UserZoneUpdateMessage,
};

fn cell_id() -> CellId {
    CellId {
        cell: 0x123,
        sector: 0x4,
    }
}

#[test]
fn complete_layer3_information_roundtrip() {
    let message = CompleteLayer3InformationMessage {
        cell_identifier: cell_id(),
        layer3_information: Layer3Information(vec![0x03, 0x00, 0x24, 0x01]),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        CompleteLayer3InformationMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn paging_request_roundtrip() {
    let message = PagingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        tag: Some(Tag(0x01020304)),
        cell_identifier_list: Some(CellIdentifierList::Cells(vec![cell_id()])),
        slot_cycle_index: Some(SlotCycleIndex(0x05)),
        service_option: Some(ServiceOption(0x0003)),
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x31, 0x02, 0x03])),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(PagingRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn assignment_request_roundtrip() {
    let message = AssignmentRequestMessage {
        channel_type: ChannelType {
            speech_or_data_indicator: 0x01,
            channel_rate_and_type: 0x08,
            coding: 0x05,
        },
        circuit_identity_code: CircuitIdentityCode {
            pcm_multiplexer: 0x0123,
            timeslot: 0x1a,
        },
        encryption_information: Some(EncryptionInformation {
            parameters: vec![
                EncryptionParameter {
                    identifier: 0x01,
                    status: true,
                    available: false,
                    value: vec![0, 1, 2, 3, 4, 5, 6, 7],
                },
                EncryptionParameter {
                    identifier: 0x04,
                    status: false,
                    available: true,
                    value: vec![0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
                },
            ],
        }),
        service_option: Some(ServiceOption(0x0021)),
        signals: vec![
            Signal {
                signal_value: 0x10,
                alert_pitch: 0x01,
            },
            Signal {
                signal_value: 0x20,
                alert_pitch: 0x02,
            },
        ],
        ms_information_records: Some(MsInformationRecords {
            records: vec![MsInformationRecord {
                record_type: 0x42,
                content: vec![0xaa, 0xbb],
            }],
        }),
        priority: Some(Priority {
            call_priority: 0x03,
            queuing_allowed: true,
            preemption_allowed: false,
        }),
        paca_timestamp: Some(PacaTimestamp(0x01020304)),
        quality_of_service_parameters: Some(QualityOfServiceParameters {
            packet_priority: 0x0a,
        }),
        a2p_bearer_session_params: None,
        a2p_bearer_format_params: None,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(AssignmentRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn clear_command_roundtrip() {
    let message = ClearCommandMessage {
        cause: Cause(0x09),
        cause_layer3: Some(CauseLayer3 {
            coding_standard: 0x00,
            location: 0x04,
            cause_value: 0x10,
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(ClearCommandMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn assignment_complete_roundtrip() {
    let message = AssignmentCompleteMessage {
        channel_number: ChannelNumber(0x1122),
        encryption_information: Some(EncryptionInformation {
            parameters: vec![EncryptionParameter {
                identifier: 0x01,
                status: true,
                available: false,
                value: vec![],
            }],
        }),
        service_option: Some(ServiceOption(0x0003)),
        a2p_bearer_session_params: None,
        a2p_bearer_format_params: None,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        AssignmentCompleteMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn assignment_request_with_a2p_bearer_format_roundtrip() {
    use cdma_ios::{A2pBearerFormatParams, A2pBearerSessionParams, BearerFormatEntry};
    let message = AssignmentRequestMessage {
        channel_type: ChannelType {
            speech_or_data_indicator: 0x01,
            channel_rate_and_type: 0x08,
            coding: 0x05,
        },
        circuit_identity_code: CircuitIdentityCode {
            pcm_multiplexer: 0x0001,
            timeslot: 0x01,
        },
        encryption_information: None,
        service_option: Some(ServiceOption(0x0003)),
        signals: vec![],
        ms_information_records: None,
        priority: None,
        paca_timestamp: None,
        quality_of_service_parameters: None,
        a2p_bearer_session_params: Some(A2pBearerSessionParams {
            ip_address: std::net::Ipv4Addr::new(192, 168, 1, 100),
            udp_port: 5004,
        }),
        a2p_bearer_format_params: Some(A2pBearerFormatParams {
            formats: vec![
                BearerFormatEntry {
                    bearer_format_tag_type: 1,
                    bearer_format_id: 0,
                    rtp_payload_type: 96,
                    bearer_addr: None,
                },
                BearerFormatEntry {
                    bearer_format_tag_type: 2,
                    bearer_format_id: 1,
                    rtp_payload_type: 97,
                    bearer_addr: Some((std::net::Ipv4Addr::new(10, 0, 0, 1), 6000)),
                },
            ],
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(AssignmentRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn assignment_complete_with_a2p_bearer_format_roundtrip() {
    use cdma_ios::{A2pBearerFormatParams, A2pBearerSessionParams, BearerFormatEntry};
    let message = AssignmentCompleteMessage {
        channel_number: ChannelNumber(0x1122),
        encryption_information: None,
        service_option: Some(ServiceOption(0x0003)),
        a2p_bearer_session_params: Some(A2pBearerSessionParams {
            ip_address: std::net::Ipv4Addr::new(10, 0, 0, 5),
            udp_port: 7000,
        }),
        a2p_bearer_format_params: Some(A2pBearerFormatParams {
            formats: vec![BearerFormatEntry {
                bearer_format_tag_type: 1,
                bearer_format_id: 0,
                rtp_payload_type: 96,
                bearer_addr: None,
            }],
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        AssignmentCompleteMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn assignment_failure_roundtrip() {
    let message = AssignmentFailureMessage { cause: Cause(0x21) };
    let encoded = message.encode().unwrap();
    assert_eq!(AssignmentFailureMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn clear_request_roundtrip() {
    let message = ClearRequestMessage {
        cause: Cause(0x09),
        cause_layer3: Some(CauseLayer3 {
            coding_standard: 0x00,
            location: 0x04,
            cause_value: 0x10,
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(ClearRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn clear_complete_roundtrip() {
    let message = ClearCompleteMessage {
        power_down_indicator: true,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(ClearCompleteMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn connect_roundtrip() {
    let message = ConnectMessage;
    let encoded = message.encode().unwrap();
    assert_eq!(ConnectMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn progress_roundtrip() {
    let message = ProgressMessage {
        signal: Some(Signal {
            signal_value: 0x10,
            alert_pitch: 0x01,
        }),
        ms_information_records: None,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(ProgressMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn alert_with_information_roundtrip() {
    let message = AlertWithInformationMessage {
        ms_information_records: Some(MsInformationRecords {
            records: vec![MsInformationRecord {
                record_type: 0x01,
                content: vec![0x40, 0x41],
            }],
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        AlertWithInformationMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn parameter_update_request_roundtrip() {
    let message = ParameterUpdateRequestMessage;
    let encoded = message.encode().unwrap();
    assert_eq!(
        ParameterUpdateRequestMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn parameter_update_confirm_roundtrip() {
    let message = ParameterUpdateConfirmMessage;
    let encoded = message.encode().unwrap();
    assert_eq!(
        ParameterUpdateConfirmMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn location_updating_accept_roundtrip() {
    let message = LocationUpdatingAcceptMessage {
        location_area_identification: Some(LocationAreaIdentification {
            mcc_digit_1: 3,
            mcc_digit_2: 1,
            mcc_digit_3: 0,
            mnc_digit_1: 2,
            mnc_digit_2: 6,
            mnc_digit_3: 15,
            lac: 0x1234,
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        LocationUpdatingAcceptMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn location_updating_reject_roundtrip() {
    let message = LocationUpdatingRejectMessage {
        reject_cause: RejectCause(0x56),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        LocationUpdatingRejectMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn user_zone_update_roundtrip() {
    let message = UserZoneUpdateMessage {
        user_zone_id: Some(UserZoneId(0x1234)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(UserZoneUpdateMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn privacy_mode_command_roundtrip() {
    let message = PrivacyModeCommandMessage {
        encryption_information: EncryptionInformation {
            parameters: vec![EncryptionParameter {
                identifier: 0x01,
                status: true,
                available: false,
                value: vec![0, 1, 2, 3, 4, 5, 6, 7],
            }],
        },
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        PrivacyModeCommandMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn privacy_mode_complete_roundtrip() {
    let message = PrivacyModeCompleteMessage {
        encryption_information: Some(EncryptionInformation {
            parameters: vec![EncryptionParameter {
                identifier: 0x01,
                status: false,
                available: true,
                value: vec![],
            }],
        }),
        voice_privacy_request: false,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        PrivacyModeCompleteMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn bs_service_request_roundtrip() {
    let message = BsServiceRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x11223344)),
        service_option: Some(ServiceOption(0x0021)),
        tag: Some(Tag(0x01020304)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BsServiceRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn bs_service_response_roundtrip() {
    let message = BsServiceResponseMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x11223344)),
        tag: Some(Tag(0x01020304)),
        cause: Some(Cause(0x11)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BsServiceResponseMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_failure_roundtrip() {
    let message = HandoffFailureMessage { cause: Cause(0x21) };
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffFailureMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_required_reject_roundtrip() {
    let message = HandoffRequiredRejectMessage { cause: Cause(0x2a) };
    let encoded = message.encode().unwrap();
    assert_eq!(
        HandoffRequiredRejectMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn handoff_commenced_roundtrip() {
    let message = HandoffCommencedMessage;
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffCommencedMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_complete_roundtrip() {
    let message = HandoffCompleteMessage;
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffCompleteMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_performed_roundtrip() {
    let message = HandoffPerformedMessage {
        cause: Cause(0x1b),
        cell_identifier_list: Some(HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffPerformedMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_required_roundtrip() {
    let message = HandoffRequiredMessage {
        cause: Cause(0x0e),
        target_cell_identifier_list: HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        },
        classmark_information_type_2: Some(ClassmarkInformationType2(vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ])),
        response_request: true,
        encryption_information: None,
        is95_channel_identity: None,
        mobile_identity_esn: Some(MobileIdentity::Esn(0x11223344)),
        downlink_radio_environment: Some(HandoffDownlinkRadioEnvironment {
            records: vec![HandoffDownlinkRadioEnvironmentRecord {
                cell: HandoffCellIdentifier::Cell(cell_id()),
                downlink_signal_strength_raw: 20,
                cdma_target_one_way_delay: 0x1234,
            }],
        }),
        service_option: Some(ServiceOption(0x0021)),
        cdma_serving_one_way_delay: Some(HandoffCdmaServingOneWayDelay {
            cell: HandoffCellIdentifier::Cell(cell_id()),
            delay_100ns: 0x0123,
        }),
        is95_ms_measured_channel_identity: Some(Is95MsMeasuredChannelIdentity {
            band_class: 1,
            arfcn: 384,
        }),
        is2000_channel_identity: None,
        quality_of_service_parameters: Some(QualityOfServiceParameters { packet_priority: 4 }),
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x18, 0x01, 0x00, 0x00])),
        is2000_service_configuration_record: None,
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
        protocol_type: Some(ProtocolType(0x880b)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffRequiredMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_request_roundtrip() {
    let message = HandoffRequestMessage {
        channel_type: ChannelType {
            speech_or_data_indicator: 0x01,
            channel_rate_and_type: 0x08,
            coding: 0x05,
        },
        encryption_information: None,
        classmark_information_type_2: Some(ClassmarkInformationType2(vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ])),
        target_cell_identifier_list: Some(HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        }),
        circuit_identity_code_extension: Some(CircuitIdentityCodeExtension {
            circuit_identity_code: CircuitIdentityCode {
                pcm_multiplexer: 0x0123,
                timeslot: 0x1a,
            },
            circuit_mode: 0,
        }),
        is95_channel_identity: None,
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x11223344)),
        downlink_radio_environment: None,
        service_option: Some(ServiceOption(0x0021)),
        cdma_serving_one_way_delay: None,
        is95_ms_measured_channel_identity: Some(Is95MsMeasuredChannelIdentity {
            band_class: 1,
            arfcn: 384,
        }),
        is2000_channel_identity: None,
        quality_of_service_parameters: Some(QualityOfServiceParameters { packet_priority: 4 }),
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x18, 0x01, 0x00, 0x00])),
        is2000_service_configuration_record: Some(Is2000ServiceConfigurationRecord {
            fill_bits: 0,
            content: vec![0xaa, 0xbb],
        }),
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
        protocol_type: Some(ProtocolType(0x880b)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn handoff_request_ack_roundtrip() {
    let message = HandoffRequestAcknowledgeMessage {
        is95_channel_identity: None,
        cell_identifier_list: Some(HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        }),
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
            target_bs_p_rev: 6,
        }),
        hard_handoff_parameters: Some(HardHandoffParameters {
            band_class: 1,
            number_of_preamble_frames: 2,
            reset_l2: true,
            reset_fpc: true,
            encryption_mode: 1,
            private_lcm: false,
            nom_pwr: 7,
            nom_pwr_ext: false,
            fpc_subchannel_information: 3,
            fpc_subchannel_info_included: true,
            power_control_step: 2,
            power_control_step_included: true,
        }),
        is2000_channel_identity: Some(Is2000ChannelIdentity {
            otd: false,
            frame_offset: 4,
            channels: vec![Is2000ChannelEntry {
                physical_channel_type: Is2000PhysicalChannelType::Fch,
                pilot_gating_rate: 0,
                qof_mask: 1,
                walsh_code_channel_index: 22,
                pilot_pn: 0x111,
                arfcn: Some(384),
            }],
        }),
        is2000_service_configuration_record: Some(Is2000ServiceConfigurationRecord {
            fill_bits: 0,
            content: vec![0xaa, 0xbb],
        }),
        is2000_non_negotiable_service_configuration_record: Some(
            Is2000NonNegotiableServiceConfigurationRecord {
                fill_bits: 0,
                content: vec![0xcc, 0xdd],
            },
        ),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        HandoffRequestAcknowledgeMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn handoff_command_roundtrip() {
    let message = HandoffCommandMessage {
        rf_channel_identity: None,
        is95_channel_identity: None,
        cell_identifier_list: Some(HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        }),
        handoff_power_level: None,
        sid: None,
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
            target_bs_p_rev: 6,
        }),
        hard_handoff_parameters: Some(HardHandoffParameters {
            band_class: 1,
            number_of_preamble_frames: 2,
            reset_l2: true,
            reset_fpc: true,
            encryption_mode: 1,
            private_lcm: false,
            nom_pwr: 7,
            nom_pwr_ext: false,
            fpc_subchannel_information: 3,
            fpc_subchannel_info_included: true,
            power_control_step: 2,
            power_control_step_included: true,
        }),
        is2000_channel_identity: Some(Is2000ChannelIdentity {
            otd: false,
            frame_offset: 4,
            channels: vec![Is2000ChannelEntry {
                physical_channel_type: Is2000PhysicalChannelType::Fch,
                pilot_gating_rate: 0,
                qof_mask: 1,
                walsh_code_channel_index: 22,
                pilot_pn: 0x111,
                arfcn: Some(384),
            }],
        }),
        is2000_service_configuration_record: Some(Is2000ServiceConfigurationRecord {
            fill_bits: 0,
            content: vec![0xaa, 0xbb],
        }),
        is2000_non_negotiable_service_configuration_record: Some(
            Is2000NonNegotiableServiceConfigurationRecord {
                fill_bits: 0,
                content: vec![0xcc, 0xdd],
            },
        ),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(HandoffCommandMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn parameter_update_request_rejects_unexpected_body() {
    // New DTAP format: [disc=0x01][DLCI=0x00][LI][proto_disc][reserved][msg_type][body...]
    // LI=4: proto_disc(1)+reserved(1)+msg_type(1)+extra(1)=4; proto_disc=0x03 is wrong (expects 0x05)
    let error = ParameterUpdateRequestMessage::decode(&[0x01, 0x00, 0x04, 0x03, 0x00, 0x2c, 0xaa])
        .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::ReservedValue {
            context: "A1 DTAP protocol discriminator",
            value: 0x03,
        }
    );
}

#[test]
fn privacy_mode_complete_rejects_mutually_exclusive_indicators() {
    let error = PrivacyModeCompleteMessage {
        encryption_information: Some(EncryptionInformation {
            parameters: vec![EncryptionParameter {
                identifier: 0x01,
                status: false,
                available: true,
                value: vec![],
            }],
        }),
        voice_privacy_request: true,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Privacy Mode Complete",
            reason: "encryption information and voice privacy request are mutually exclusive",
        }
    );
}

#[test]
fn location_updating_reject_rejects_invalid_cause() {
    let error = LocationUpdatingRejectMessage {
        reject_cause: RejectCause(0x09),
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Reject Cause",
            reason: "value is not allowed for Location Updating Reject",
        }
    );
}

#[test]
fn handoff_commenced_rejects_unexpected_body() {
    let error = HandoffCommencedMessage::decode(&[0x00, 0x02, 0x15, 0xaa]).unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidLength {
            expected: 0,
            actual: 1,
        }
    );
}

#[test]
fn handoff_complete_rejects_unexpected_body() {
    let error = HandoffCompleteMessage::decode(&[0x00, 0x02, 0x14, 0xaa]).unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidLength {
            expected: 0,
            actual: 1,
        }
    );
}

#[test]
fn handoff_performed_rejects_invalid_cause() {
    let error = HandoffPerformedMessage {
        cause: Cause(0x09),
        cell_identifier_list: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Performed Cause",
            reason: "cause value is not allowed for Handoff Performed",
        }
    );
}

#[test]
fn handoff_required_rejects_dual_channel_identity_families() {
    let error = HandoffRequiredMessage {
        cause: Cause(0x0e),
        target_cell_identifier_list: HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        },
        classmark_information_type_2: None,
        response_request: false,
        encryption_information: None,
        is95_channel_identity: Some(Is95ChannelIdentity {
            hard_handoff: true,
            frame_offset: 1,
            channels: vec![Is95ChannelEntry {
                walsh_code_channel_index: 1,
                pilot_pn: 1,
                power_combined: false,
                arfcn: Some(384),
            }],
        }),
        mobile_identity_esn: None,
        downlink_radio_environment: None,
        service_option: None,
        cdma_serving_one_way_delay: None,
        is95_ms_measured_channel_identity: None,
        is2000_channel_identity: Some(Is2000ChannelIdentity {
            otd: false,
            frame_offset: 1,
            channels: vec![Is2000ChannelEntry {
                physical_channel_type: Is2000PhysicalChannelType::Fch,
                pilot_gating_rate: 0,
                qof_mask: 0,
                walsh_code_channel_index: 1,
                pilot_pn: 1,
                arfcn: Some(384),
            }],
        }),
        quality_of_service_parameters: None,
        is2000_mobile_capabilities: None,
        is2000_service_configuration_record: None,
        pdsn_ip_address: None,
        protocol_type: None,
    }
    .encode()
    .unwrap_err();
    assert!(matches!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Required",
            ..
        }
    ));
}

#[test]
fn handoff_command_rejects_analog_and_cdma_targets_together() {
    let error = HandoffCommandMessage {
        rf_channel_identity: Some(RfChannelIdentity {
            color_code: 1,
            n_amps: false,
            ansi_eia_tia_553: true,
            timeslot_number: 0,
            arfcn: 512,
        }),
        is95_channel_identity: Some(Is95ChannelIdentity {
            hard_handoff: true,
            frame_offset: 1,
            channels: vec![Is95ChannelEntry {
                walsh_code_channel_index: 1,
                pilot_pn: 1,
                power_combined: false,
                arfcn: Some(384),
            }],
        }),
        cell_identifier_list: None,
        handoff_power_level: None,
        sid: None,
        extended_handoff_direction_parameters: None,
        hard_handoff_parameters: None,
        is2000_channel_identity: None,
        is2000_service_configuration_record: None,
        is2000_non_negotiable_service_configuration_record: None,
    }
    .encode()
    .unwrap_err();
    assert!(matches!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Command",
            ..
        }
    ));
}

#[test]
fn handoff_request_ack_rejects_missing_channel_identity() {
    let error = HandoffRequestAcknowledgeMessage {
        is95_channel_identity: None,
        cell_identifier_list: None,
        extended_handoff_direction_parameters: None,
        hard_handoff_parameters: None,
        is2000_channel_identity: None,
        is2000_service_configuration_record: None,
        is2000_non_negotiable_service_configuration_record: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Request Acknowledge",
            reason: "one channel identity must be present",
        }
    );
}

#[test]
fn handoff_request_ack_rejects_is2000_service_config_without_is2000_channel_identity() {
    let error = HandoffRequestAcknowledgeMessage {
        is95_channel_identity: Some(Is95ChannelIdentity {
            hard_handoff: true,
            frame_offset: 1,
            channels: vec![Is95ChannelEntry {
                walsh_code_channel_index: 1,
                pilot_pn: 1,
                power_combined: false,
                arfcn: Some(384),
            }],
        }),
        cell_identifier_list: None,
        extended_handoff_direction_parameters: None,
        hard_handoff_parameters: None,
        is2000_channel_identity: None,
        is2000_service_configuration_record: Some(Is2000ServiceConfigurationRecord {
            fill_bits: 0,
            content: vec![0xaa, 0xbb],
        }),
        is2000_non_negotiable_service_configuration_record: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Request Acknowledge",
            reason: "IS-2000 service configuration requires IS-2000 channel identity",
        }
    );
}

#[test]
fn handoff_command_rejects_missing_target_identity() {
    let error = HandoffCommandMessage {
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
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Command",
            reason: "one target channel identity must be present",
        }
    );
}

#[test]
fn handoff_command_rejects_is2000_service_config_without_is2000_channel_identity() {
    let error = HandoffCommandMessage {
        rf_channel_identity: Some(RfChannelIdentity {
            color_code: 1,
            n_amps: false,
            ansi_eia_tia_553: true,
            timeslot_number: 0,
            arfcn: 512,
        }),
        is95_channel_identity: None,
        cell_identifier_list: None,
        handoff_power_level: None,
        sid: None,
        extended_handoff_direction_parameters: None,
        hard_handoff_parameters: None,
        is2000_channel_identity: None,
        is2000_service_configuration_record: Some(Is2000ServiceConfigurationRecord {
            fill_bits: 0,
            content: vec![0xaa, 0xbb],
        }),
        is2000_non_negotiable_service_configuration_record: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_ios::Error::InvalidValue {
            context: "Handoff Command",
            reason: "IS-2000 service configuration requires IS-2000 channel identity",
        }
    );
}

#[test]
fn cm_service_request_roundtrip() {
    let message = CmServiceRequestMessage {
        cm_service_type: CmServiceType::MobileOriginatingCallEstablishment,
        classmark_information_type_2: ClassmarkInformationType2(vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ]),
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        called_party_bcd_number: Some(cdma_ios::CalledPartyBcdNumber(vec![0x91, 0x21, 0x43])),
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
        service_option: Some(ServiceOption(0x0021)),
        voice_privacy_request: true,
        radio_environment_and_resources: Some(RadioEnvironmentAndResources {
            include_priority: false,
            forward: 0x01,
            reverse: 0x01,
            allocated: true,
            available: true,
        }),
        called_party_ascii_number: Some(CallingPartyAsciiNumber(vec![
            0x80, 0x80, b'5', b'5', b'5',
        ])),
        circuit_identity_code: Some(CircuitIdentityCode {
            pcm_multiplexer: 0x0123,
            timeslot: 0x1a,
        }),
        cdma_serving_one_way_delay: Some(HandoffCdmaServingOneWayDelay {
            cell: HandoffCellIdentifier::Cell(cell_id()),
            delay_100ns: 0x1234,
        }),
        authentication_event: Some(AuthenticationEvent(0x01)),
        authentication_data: Some(AuthenticationData([0xaa, 0xbb, 0xcc])),
        paca_reorigination_indicator: true,
        user_zone_id: Some(UserZoneId(0x3344)),
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x31, 0x02, 0x03])),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(CmServiceRequestMessage::decode(&encoded).unwrap(), message);

    let l3 = Layer3Information::from_cm_service_request(&message).unwrap();
    assert_eq!(l3.decode_cm_service_request().unwrap(), message);
}

#[test]
fn paging_response_roundtrip() {
    let message = PagingResponseMessage {
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
        service_option: Some(ServiceOption(0x0021)),
        voice_privacy_request: true,
        circuit_identity_code: Some(CircuitIdentityCode {
            pcm_multiplexer: 0x0123,
            timeslot: 0x1a,
        }),
        cdma_serving_one_way_delay: Some(HandoffCdmaServingOneWayDelay {
            cell: HandoffCellIdentifier::Cell(cell_id()),
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
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x31, 0x02, 0x03])),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(PagingResponseMessage::decode(&encoded).unwrap(), message);

    let l3 = Layer3Information::from_paging_response(&message).unwrap();
    assert_eq!(l3.decode_paging_response().unwrap(), message);
}

#[test]
fn location_updating_request_roundtrip() {
    let message = LocationUpdatingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        location_area_identification: Some(LocationAreaIdentification {
            mcc_digit_1: 3,
            mcc_digit_2: 1,
            mcc_digit_3: 0,
            mnc_digit_1: 2,
            mnc_digit_2: 6,
            mnc_digit_3: 0xf,
            lac: 0x1234,
        }),
        classmark_information_type_2: Some(ClassmarkInformationType2(vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ])),
        registration_type: Some(RegistrationType(0x01)),
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
        authentication_event: Some(AuthenticationEvent(0x01)),
        user_zone_id: Some(UserZoneId(0x3344)),
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x31, 0x02, 0x03])),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        LocationUpdatingRequestMessage::decode(&encoded).unwrap(),
        message
    );

    let l3 = Layer3Information::from_location_updating_request(&message).unwrap();
    assert_eq!(l3.decode_location_updating_request().unwrap(), message);
}

#[test]
fn authentication_request_bsmap_roundtrip() {
    let message = AuthenticationRequestMessage::Bsmap(AuthenticationRequestBsmapMessage {
        authentication_challenge_parameter_randu: AuthenticationChallengeParameter([
            0x10, 0x01, 0x02, 0x03, 0x04,
        ]),
        mobile_identity_imsi: Some(MobileIdentity::Imsi("12345678901".to_string())),
        tag: Some(Tag(0x01020304)),
        cell_identifier_list: Some(CellIdentifierList::Cells(vec![cell_id()])),
        slot_cycle_index: Some(SlotCycleIndex(0x05)),
    });
    let encoded = message.encode().unwrap();
    assert_eq!(
        AuthenticationRequestMessage::decode(&encoded).unwrap(),
        message
    );
    assert_eq!(encoded[0], 0x00);
}

#[test]
fn authentication_request_dtap_roundtrip() {
    let message = AuthenticationRequestMessage::Dtap(AuthenticationRequestDtapMessage {
        authentication_challenge_parameter_randu: AuthenticationChallengeParameter([
            0x10, 0x11, 0x22, 0x33, 0x44,
        ]),
        is2000_mobile_capabilities: Some(Is2000MobileCapabilities(vec![0x31, 0x02, 0x03])),
    });
    let encoded = message.encode().unwrap();
    assert_eq!(
        AuthenticationRequestMessage::decode(&encoded).unwrap(),
        message
    );
    // DTAP frame: [disc=0x01][DLCI][LI][proto_disc=0x05][reserved][msg_type]...
    assert_eq!(encoded[3], 0x05);
}

#[test]
fn authentication_response_bsmap_roundtrip() {
    let message = AuthenticationResponseMessage::Bsmap(AuthenticationResponseBsmapMessage {
        authentication_response_parameter_authu: AuthenticationResponseParameter([
            0x10, 0x20, 0x30, 0x40,
        ]),
        mobile_identity_imsi: Some(MobileIdentity::Imsi("12345678901".to_string())),
        tag: Some(Tag(0x01020304)),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x11223344)),
    });
    let encoded = message.encode().unwrap();
    assert_eq!(
        AuthenticationResponseMessage::decode(&encoded).unwrap(),
        message
    );
    assert_eq!(encoded[0], 0x00);
}

#[test]
fn authentication_response_dtap_roundtrip() {
    let message = AuthenticationResponseMessage::Dtap(AuthenticationResponseDtapMessage {
        authentication_response_parameter_authu: AuthenticationResponseParameter([
            0x10, 0x20, 0x30, 0x40,
        ]),
    });
    let encoded = message.encode().unwrap();
    assert_eq!(
        AuthenticationResponseMessage::decode(&encoded).unwrap(),
        message
    );
    // DTAP frame: [disc=0x01][DLCI][LI][proto_disc=0x05][reserved][msg_type]...
    assert_eq!(encoded[3], 0x05);
}

#[test]
fn ssd_update_sequence_message_roundtrips() {
    // DTAP frame: [disc=0x01][DLCI][LI][proto_disc=0x05][reserved][msg_type]...
    let ssd_update_request = SsdUpdateRequestMessage {
        authentication_challenge_parameter_randssd: SsdUpdateChallengeParameter([
            0x40, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        ]),
    };
    let encoded = ssd_update_request.encode().unwrap();
    assert_eq!(
        SsdUpdateRequestMessage::decode(&encoded).unwrap(),
        ssd_update_request
    );
    assert_eq!(encoded[3], 0x05);

    let base_station_challenge = BaseStationChallengeMessage {
        authentication_challenge_parameter_randbs: AuthenticationChallengeParameter([
            0x80, 0xaa, 0xbb, 0xcc, 0xdd,
        ]),
    };
    let encoded = base_station_challenge.encode().unwrap();
    assert_eq!(
        BaseStationChallengeMessage::decode(&encoded).unwrap(),
        base_station_challenge
    );
    assert_eq!(encoded[3], 0x05);

    let challenge_response = BaseStationChallengeResponseMessage {
        authentication_response_parameter_authbs: AuthenticationResponseParameter([
            0x30, 0x01, 0x02, 0x03,
        ]),
    };
    let encoded = challenge_response.encode().unwrap();
    assert_eq!(
        BaseStationChallengeResponseMessage::decode(&encoded).unwrap(),
        challenge_response
    );
    assert_eq!(encoded[3], 0x05);

    let response = SsdUpdateResponseMessage {
        cause_layer_3: Some(CauseLayer3 {
            coding_standard: 0,
            location: 0x04,
            cause_value: 0x3b,
        }),
    };
    let encoded = response.encode().unwrap();
    assert_eq!(
        SsdUpdateResponseMessage::decode(&encoded).unwrap(),
        response
    );
    assert_eq!(encoded[3], 0x05);
}

#[test]
fn parameter_update_request_uses_mobility_management_protocol_discriminator() {
    let encoded = ParameterUpdateRequestMessage.encode().unwrap();
    // DTAP frame: [disc=0x01][DLCI][LI][proto_disc=0x05][reserved][msg_type]...
    assert_eq!(encoded[3], 0x05);
}

#[test]
fn complete_layer3_information_rejects_duplicate_cell_identifier() {
    let message = CompleteLayer3InformationMessage {
        cell_identifier: cell_id(),
        layer3_information: Layer3Information(vec![0x03, 0x00, 0x24, 0x01]),
    };
    let mut encoded = message.encode().unwrap();
    let duplicate = [0x05, 0x03, 0x02, 0x12, 0x34];
    encoded.extend_from_slice(&duplicate);
    encoded[1] = encoded[1].wrapping_add(duplicate.len() as u8);

    assert_eq!(
        CompleteLayer3InformationMessage::decode(&encoded).unwrap_err(),
        cdma_ios::Error::DuplicateElement {
            message_type: 0x57,
            id: 0x05,
        }
    );
}

#[test]
fn paging_request_rejects_duplicate_tag() {
    let message = PagingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        tag: Some(Tag(0x01020304)),
        cell_identifier_list: None,
        slot_cycle_index: None,
        service_option: None,
        is2000_mobile_capabilities: None,
    };
    let mut encoded = message.encode().unwrap();
    let duplicate = [0x33, 0x01, 0x02, 0x03, 0x04];
    encoded.extend_from_slice(&duplicate);
    encoded[1] = encoded[1].wrapping_add(duplicate.len() as u8);

    assert_eq!(
        PagingRequestMessage::decode(&encoded).unwrap_err(),
        cdma_ios::Error::DuplicateElement {
            message_type: 0x52,
            id: 0x33,
        }
    );
}

#[test]
fn bs_service_response_rejects_duplicate_cause() {
    let message = BsServiceResponseMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
        mobile_identity_esn: None,
        tag: None,
        cause: Some(Cause(0x11)),
    };
    let mut encoded = message.encode().unwrap();
    let duplicate = [0x04, 0x01, 0x22];
    encoded.extend_from_slice(&duplicate);
    encoded[1] = encoded[1].wrapping_add(duplicate.len() as u8);

    assert_eq!(
        BsServiceResponseMessage::decode(&encoded).unwrap_err(),
        cdma_ios::Error::DuplicateElement {
            message_type: 0x0a,
            id: 0x04,
        }
    );
}

#[test]
fn handoff_required_rejects_duplicate_response_request() {
    let message = HandoffRequiredMessage {
        cause: Cause(0x0e),
        target_cell_identifier_list: HandoffCellIdentifierList {
            cells: vec![HandoffCellIdentifier::Cell(cell_id())],
        },
        classmark_information_type_2: None,
        response_request: true,
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
    };
    let mut encoded = message.encode().unwrap();
    let duplicate = [0x1b];
    encoded.extend_from_slice(&duplicate);
    encoded[1] = encoded[1].wrapping_add(duplicate.len() as u8);

    assert_eq!(
        HandoffRequiredMessage::decode(&encoded).unwrap_err(),
        cdma_ios::Error::DuplicateElement {
            message_type: 0x11,
            id: 0x1b,
        }
    );
}

#[test]
fn classmark_information_type_2_requires_minimum_legacy_payload() {
    assert_eq!(
        cdma_ios::ClassmarkInformationType2::decode(&[0x31, 0x02]).unwrap_err(),
        cdma_ios::Error::InvalidLength {
            expected: 4,
            actual: 2,
        }
    );
}

#[test]
fn called_party_bcd_number_requires_at_least_two_octets() {
    assert_eq!(
        cdma_ios::CalledPartyBcdNumber::decode(&[0x91]).unwrap_err(),
        cdma_ios::Error::InvalidLength {
            expected: 2,
            actual: 1,
        }
    );
}

// ── ADDS message round-trip tests ────────────────────────────────────────────

fn sms_user_part() -> AddsUserPart {
    AddsUserPart {
        burst_type: 0x03, // SMS
        data: vec![0x00, 0x01, 0x02, 0x03, 0xAB, 0xCD],
    }
}

#[test]
fn adds_page_roundtrip_with_tag() {
    let msg = AddsPageMessage {
        mobile_identity: MobileIdentity::Imsi("31026200000001".to_string()),
        adds_user_part: sms_user_part(),
        tag: Some(Tag(0xDEADBEEF)),
        slot_cycle_index: Some(SlotCycleIndex(2)),
    };
    let encoded = msg.encode().unwrap();
    // BSMAP discrimination byte
    assert_eq!(encoded[0], 0x00);
    // BSMAP ADDS Page message type at byte 2
    assert_eq!(encoded[2], 0x65);
    let decoded = AddsPageMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn adds_page_roundtrip_minimal() {
    let msg = AddsPageMessage {
        mobile_identity: MobileIdentity::Imsi("31026200000001".to_string()),
        adds_user_part: sms_user_part(),
        tag: None,
        slot_cycle_index: None,
    };
    let encoded = msg.encode().unwrap();
    assert_eq!(AddsPageMessage::decode(&encoded).unwrap(), msg);
}

#[test]
fn adds_transfer_roundtrip_with_esn() {
    let msg = AddsTransferMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("31026200000001".to_string()),
        adds_user_part: sms_user_part(),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x12345678)),
    };
    let encoded = msg.encode().unwrap();
    assert_eq!(encoded[0], 0x00);
    assert_eq!(encoded[2], 0x67);
    let decoded = AddsTransferMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn adds_page_ack_roundtrip_success() {
    let msg = AddsPageAckMessage {
        mobile_identity: MobileIdentity::Imsi("31026200000001".to_string()),
        tag: Some(Tag(0xDEADBEEF)),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x12345678)),
        cause: None,
    };
    let encoded = msg.encode().unwrap();
    assert_eq!(encoded[0], 0x00);
    assert_eq!(encoded[2], 0x66);
    let decoded = AddsPageAckMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn adds_page_ack_roundtrip_failure() {
    let msg = AddsPageAckMessage {
        mobile_identity: MobileIdentity::Imsi("31026200000001".to_string()),
        tag: Some(Tag(0x00000001)),
        mobile_identity_esn: None,
        cause: Some(Cause(0x20)), // equipment failure
    };
    let encoded = msg.encode().unwrap();
    let decoded = AddsPageAckMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
    assert_eq!(decoded.cause, Some(Cause(0x20)));
}

#[test]
fn adds_deliver_roundtrip_with_tag() {
    let msg = AddsDeliverMessage {
        adds_user_part: sms_user_part(),
        tag: Some(Tag(0x0000002A)),
    };
    let encoded = msg.encode().unwrap();
    // DTAP discrimination byte
    assert_eq!(encoded[0], 0x01);
    // Protocol discriminator at byte 3 (after disc, DLCI, LI)
    assert_eq!(encoded[3], 0x03);
    // ADDS Deliver message type at byte 5
    assert_eq!(encoded[5], 0x53);
    let decoded = AddsDeliverMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn adds_deliver_roundtrip_no_tag() {
    let msg = AddsDeliverMessage {
        adds_user_part: sms_user_part(),
        tag: None,
    };
    let encoded = msg.encode().unwrap();
    assert_eq!(AddsDeliverMessage::decode(&encoded).unwrap(), msg);
}

#[test]
fn adds_deliver_ack_roundtrip_success() {
    let msg = AddsDeliverAckMessage {
        tag: Some(Tag(0x0000002A)),
        cause: None,
    };
    let encoded = msg.encode().unwrap();
    assert_eq!(encoded[0], 0x01);
    assert_eq!(encoded[5], 0x54);
    let decoded = AddsDeliverAckMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn adds_deliver_ack_roundtrip_failure() {
    let msg = AddsDeliverAckMessage {
        tag: Some(Tag(0x0000002A)),
        cause: Some(Cause(0x70)), // rejection indication from mobile
    };
    let encoded = msg.encode().unwrap();
    let decoded = AddsDeliverAckMessage::decode(&encoded).unwrap();
    assert_eq!(decoded, msg);
}
