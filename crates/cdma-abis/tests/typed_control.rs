use cdma_abis::control::{
    A3ConnectAckInformation, A3ConnectInformation, A3RemoveInformation, AbisAckNotify,
    AbisDestinationId, AbisOriginatingId, AchMessageTransferMessage, AirInterfaceMessagePayload,
    AuthenticationChallengeParameter, BandClass, BtsReleaseAckMessage, BtsReleaseMessage,
    BtsReleaseRequestMessage, BtsSetupAckMessage, BtsSetupMessage, BurstCommitMessage,
    BurstRequestMessage, BurstResponseMessage, CallConnectionReference, CdmaServingOneWayDelay,
    CdmaTargetOneWayDelay, CellId, CellIdWithMscId, CellInfoRecord, ChannelElementStatus,
    ConnectAckMessage, ConnectMessage, CorrelationId, DownlinkRadioEnvironment,
    DownlinkRadioEnvironmentRecord, ExtendedHandoffDirectionParameters, ForwardBurstRadioInfo,
    GainRatioPair, Is2000ForwardPowerControlMode, Is2000FpcGainRatioInfo, Layer2AckRequestResults,
    ManufacturerSpecificRecords, MobileIdentity, PacaActionRequired, PacaTimestamp,
    PacaUpdateMessage, PchMessageTransferAckMessage, PchMessageTransferMessage,
    PhysicalChannelInfo, PhysicalChannelType, PilotGatingRate, PrivacyInfo, PrivacyMaskInformation,
    QualityOfServiceParameters, RemoveAckMessage, RemoveMessage, ReverseBurstRadioInfo, SduId,
    ServiceOption, TrafficChannelStatusMessage, TrafficCircuitId,
};

fn call_ref() -> CallConnectionReference {
    CallConnectionReference {
        market_id: 0x0001,
        generating_entity_id: 0x0002,
        call_connection_reference: 0x0000_0003,
    }
}

#[test]
fn connect_ack_roundtrip() {
    let message = ConnectAckMessage {
        call_connection_reference: call_ref(),
        correlation_id: Some(CorrelationId(0x01020304)),
        connect_ack_information: vec![A3ConnectAckInformation {
            soft_handoff_leg: 2,
            pmc_cause: Some(0x0a),
            transmit_tch_status: true,
            traffic_circuit_id: TrafficCircuitId {
                traffic_circuit_identifier: 0x1020,
                traffic_connection_identifier: 0x33,
            },
            channel_element_id: vec![0xaa, 0xbb],
            a3_originating_id: 0x2001,
            a3_destination_id: 0x2002,
        }],
    };
    let encoded = message.encode().unwrap();
    assert_eq!(ConnectAckMessage::decode(&encoded).unwrap(), message);
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

#[test]
fn connect_roundtrip() {
    let message = ConnectMessage {
        call_connection_reference: call_ref(),
        correlation_id: Some(CorrelationId(0x01020304)),
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        connect_information: vec![connect_information()],
        physical_channel_info: physical_channel_info(),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(ConnectMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn bts_setup_roundtrip() {
    let message = BtsSetupMessage {
        call_connection_reference: call_ref(),
        band_class: Some(BandClass::Pcs),
        privacy_info: Some(PrivacyInfo {
            privacy_masks: vec![PrivacyMaskInformation {
                privacy_mask_type: 0x01,
                status: true,
                available: true,
                privacy_mask: vec![0xde, 0xad, 0xbe, 0xef],
            }],
        }),
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        mobile_identities: vec![
            MobileIdentity::Imsi("12345678901".to_string()),
            MobileIdentity::Esn(0x01020304),
        ],
        physical_channel_info: Some(physical_channel_info()),
        service_option: Some(ServiceOption(0x0021)),
        paca_timestamp: Some(PacaTimestamp(0x01020304)),
        quality_of_service_parameters: Some(QualityOfServiceParameters {
            packet_priority: 0x0a,
        }),
        connect_information: vec![connect_information()],
        abis_originating_id: Some(AbisOriginatingId::new([0x44, 0x44]).unwrap()),
        cdma_serving_one_way_delay: CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x3344,
        },
        cdma_target_one_way_delay: Some(CdmaTargetOneWayDelay(0x5566)),
        walsh_code_assignment_request: true,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BtsSetupMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn bts_setup_ack_roundtrip() {
    let message = BtsSetupAckMessage {
        call_connection_reference: call_ref(),
        connect_information: vec![connect_information()],
        abis_originating_id: Some(AbisOriginatingId::new([0x11, 0x11]).unwrap()),
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
        cause: Some(0x21),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BtsSetupAckMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn burst_request_roundtrip() {
    let message = BurstRequestMessage {
        call_connection_reference: Some(call_ref()),
        band_class: Some(BandClass::Pcs),
        downlink_radio_environment: Some(DownlinkRadioEnvironment {
            records: vec![DownlinkRadioEnvironmentRecord {
                cell: CellId {
                    cell: 0x123,
                    sector: 0x4,
                },
                downlink_signal_strength_raw: 0x12,
                cdma_target_one_way_delay: 0x3344,
            }],
        }),
        cdma_serving_one_way_delay: Some(CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x5566,
        }),
        privacy_info: Some(PrivacyInfo {
            privacy_masks: vec![PrivacyMaskInformation {
                privacy_mask_type: 0x01,
                status: false,
                available: true,
                privacy_mask: vec![0xaa, 0xbb],
            }],
        }),
        correlation_id: Some(CorrelationId(0x01020304)),
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        forward_burst_radio_info: Some(ForwardBurstRadioInfo {
            coding_indicator: 1,
            qof_mask: 2,
            forward_code_channel_index: 0x123,
            pilot_pn_code: 0x101,
            forward_supplemental_channel_rate: 0x0a,
            forward_supplemental_channel_start_time: 0x1b,
            start_time_unit: 0x05,
            forward_supplemental_channel_duration: 0x0c,
        }),
        reverse_burst_radio_info: Some(ReverseBurstRadioInfo {
            coding_indicator: 1,
            reverse_supplemental_channel_rate: 0x0a,
            reverse_supplemental_channel_start_time: 0x44,
            start_time_unit: 0x05,
            reverse_supplemental_channel_duration: 0x0c,
        }),
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BurstRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn burst_response_roundtrip() {
    let message = BurstResponseMessage {
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
        forward_burst_radio_info: Some(ForwardBurstRadioInfo {
            coding_indicator: 1,
            qof_mask: 2,
            forward_code_channel_index: 0x123,
            pilot_pn_code: 0x101,
            forward_supplemental_channel_rate: 0x0a,
            forward_supplemental_channel_start_time: 0x1b,
            start_time_unit: 0x05,
            forward_supplemental_channel_duration: 0x0c,
        }),
        reverse_burst_radio_info: Some(ReverseBurstRadioInfo {
            coding_indicator: 1,
            reverse_supplemental_channel_rate: 0x0a,
            reverse_supplemental_channel_start_time: 0x44,
            start_time_unit: 0x05,
            reverse_supplemental_channel_duration: 0x0c,
        }),
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BurstResponseMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn burst_commit_roundtrip() {
    let message = BurstCommitMessage {
        call_connection_reference: Some(call_ref()),
        correlation_id: Some(CorrelationId(0x01020304)),
        forward_cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        reverse_cell_identifier_list: Some(Vec::new()),
        forward_burst_radio_info: Some(ForwardBurstRadioInfo {
            coding_indicator: 1,
            qof_mask: 2,
            forward_code_channel_index: 0x123,
            pilot_pn_code: 0x101,
            forward_supplemental_channel_rate: 0x0a,
            forward_supplemental_channel_start_time: 0x1b,
            start_time_unit: 0x05,
            forward_supplemental_channel_duration: 0x0c,
        }),
        reverse_burst_radio_info: Some(ReverseBurstRadioInfo {
            coding_indicator: 1,
            reverse_supplemental_channel_rate: 0x0a,
            reverse_supplemental_channel_start_time: 0x44,
            start_time_unit: 0x05,
            reverse_supplemental_channel_duration: 0x0c,
        }),
        is2000_forward_power_control_mode: Some(Is2000ForwardPowerControlMode { fpc_mode: 0x03 }),
        is2000_fpc_gain_ratio_info: Some(Is2000FpcGainRatioInfo {
            initial_gain_ratio: 0x10,
            gain_adjust_step_size: 0x04,
            gain_ratio_pairs: [
                GainRatioPair {
                    min_gain_ratio: 0x11,
                    max_gain_ratio: 0x22,
                },
                GainRatioPair {
                    min_gain_ratio: 0x33,
                    max_gain_ratio: 0x44,
                },
                GainRatioPair {
                    min_gain_ratio: 0x55,
                    max_gain_ratio: 0x66,
                },
            ],
        }),
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BurstCommitMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn remove_roundtrip() {
    let message = RemoveMessage {
        call_connection_reference: call_ref(),
        correlation_id: Some(CorrelationId(0x01020304)),
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        remove_information: vec![A3RemoveInformation {
            traffic_circuit_id: TrafficCircuitId {
                traffic_circuit_identifier: 0x1001,
                traffic_connection_identifier: 0x44,
            },
            cells_to_be_removed: vec![CellIdWithMscId {
                mscid: 0x010203,
                cell: 0x0123,
                sector: 0x4,
            }],
            a3_destination_id: 0x2010,
            a7_destination_id: 0x2011,
        }],
    };
    let encoded = message.encode().unwrap();
    assert_eq!(RemoveMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn remove_ack_roundtrip() {
    let message = RemoveAckMessage {
        call_connection_reference: call_ref(),
        correlation_id: Some(CorrelationId(0x01020304)),
        a3_destination_id: Some(0x2001),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(RemoveAckMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn traffic_channel_status_roundtrip() {
    let message = TrafficChannelStatusMessage {
        call_connection_reference: call_ref(),
        cell_identifier_list: vec![CellIdWithMscId {
            mscid: 0x010203,
            cell: 0x0123,
            sector: 0x4,
        }],
        channel_element_status: ChannelElementStatus { transmit_on: true },
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        a3_destination_id: Some(0x1001),
        a7_destination_id: Some(0x1002),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        TrafficChannelStatusMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn bts_release_request_roundtrip() {
    let message = BtsReleaseRequestMessage {
        call_connection_reference: call_ref(),
        cause: Some(0x10),
        manufacturer_specific_records: Some(ManufacturerSpecificRecords {
            manufacturer_id: 0x42,
            information: vec![0xaa, 0xbb],
        }),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BtsReleaseRequestMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn bts_release_roundtrip() {
    let message = BtsReleaseMessage {
        call_connection_reference: call_ref(),
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        correlation_id: Some(CorrelationId(0x01020304)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BtsReleaseMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn bts_release_ack_roundtrip() {
    let message = BtsReleaseAckMessage {
        call_connection_reference: call_ref(),
        correlation_id: Some(CorrelationId(0x01020304)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(BtsReleaseAckMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn ach_message_transfer_roundtrip() {
    let message = AchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
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
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        AchMessageTransferMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn ach_message_transfer_rejects_auth_without_identity() {
    let message = AchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
        mobile_identities: vec![],
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
    };
    assert_eq!(
        message.encode().unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Abis-ACH Msg Transfer",
            reason: "authentication challenge requires at least one mobile identity",
        }
    );
}

#[test]
fn pch_message_transfer_roundtrip() {
    let message = PchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
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
        air_interface_message: Some(AirInterfaceMessagePayload::new(0xca, [0xba, 0xbe]).unwrap()),
        layer2_ack_request_results: Some(Layer2AckRequestResults::request()),
        abis_ack_notify: Some(AbisAckNotify),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        PchMessageTransferMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn pch_message_transfer_rejects_ack_ies_without_correlation() {
    let message = PchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        air_interface_message: Some(AirInterfaceMessagePayload::new(0xca, [0xba, 0xbe]).unwrap()),
        layer2_ack_request_results: Some(Layer2AckRequestResults::request()),
        abis_ack_notify: None,
    };
    assert_eq!(
        message.encode().unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Abis-PCH Msg Transfer",
            reason: "ack-related IEs require a correlation identifier",
        }
    );
}

#[test]
fn ach_message_transfer_roundtrip_without_air_interface_message() {
    let message = AchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        cell_identifier: Some(CellId {
            cell: 0x123,
            sector: 0x4,
        }),
        bts_l2_termination: Some(true),
        air_interface_message: None,
        cdma_serving_one_way_delay: CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x3344,
        },
        authentication_challenge_parameter: None,
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        AchMessageTransferMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn pch_message_transfer_roundtrip_without_air_interface_message() {
    let message = PchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
        mobile_identities: vec![MobileIdentity::Imsi("12345678901".to_string())],
        cell_identifier_list: Some(vec![CellId {
            cell: 0x123,
            sector: 0x4,
        }]),
        air_interface_message: None,
        layer2_ack_request_results: Some(Layer2AckRequestResults::request()),
        abis_ack_notify: Some(AbisAckNotify),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        PchMessageTransferMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn pch_message_transfer_ack_roundtrip() {
    let message = PchMessageTransferAckMessage {
        correlation_id: Some(CorrelationId(0x01020304)),
        cause: Some(0x20),
        bts_l2_termination: Some(true),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(
        PchMessageTransferAckMessage::decode(&encoded).unwrap(),
        message
    );
}

#[test]
fn paca_update_roundtrip() {
    let message = PacaUpdateMessage {
        call_connection_reference: call_ref(),
        mobile_identity_imsi: Some(MobileIdentity::Imsi("12345678901".to_string())),
        action_required: Some(PacaActionRequired::UpdateQueuePosition),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(PacaUpdateMessage::decode(&encoded).unwrap(), message);
}
