//! Round-trip encode/decode tests for every Abis control message type.

use cdma_abis::control::typed::*;
use cdma_abis::control::{ElementId, MessageType, decode, encode};
use cdma_abis::signaling_framing::{SignalingFrame, SignalingFrameStreamDecoder};

fn test_ccr() -> CallConnectionReference {
    CallConnectionReference {
        market_id: 0x1234,
        generating_entity_id: 0x5678,
        call_connection_reference: 0xABCD_EF01,
    }
}

fn test_cell() -> CellId {
    CellId {
        cell: 0x100,
        sector: 0x03,
    }
}

fn test_serving_delay() -> CdmaServingOneWayDelay {
    CdmaServingOneWayDelay {
        cell: test_cell(),
        delay_100ns: 500,
    }
}

fn test_cell_info_record() -> CellInfoRecord {
    CellInfoRecord {
        cell: test_cell(),
        qof_mask: 0x01,
        new_cell: true,
        power_combine_indication: false,
        pilot_pn: 0x0042,
        code_channel: 10,
    }
}

fn test_traffic_circuit_id() -> TrafficCircuitId {
    TrafficCircuitId {
        traffic_circuit_identifier: 0x0001,
        traffic_connection_identifier: 0x02,
    }
}

fn test_a3_connect_info() -> A3ConnectInformation {
    A3ConnectInformation {
        physical_channel_type: PhysicalChannelType::Fch,
        new_a3: true,
        cell_info_records: vec![test_cell_info_record()],
        traffic_circuit_id: test_traffic_circuit_id(),
        extended_handoff_direction_parameters: Some(ExtendedHandoffDirectionParameters {
            search_window_a_size: 8,
            search_window_n_size: 7,
            search_window_r_size: 6,
            t_add: 28,
            t_drop: 30,
            compare_threshold: 5,
            drop_timer_value: 3,
            neighbor_max_age: 2,
            soft_slope: 10,
            add_intercept: 15,
            drop_intercept: 20,
            target_bs_p_rev: 6,
        }),
        channel_element_id: vec![0x01, 0x02],
        a3_originating_id: 0x0A0B,
        a7_destination_id: 0x0C0D,
    }
}

fn test_physical_channel_info() -> PhysicalChannelInfo {
    PhysicalChannelInfo {
        frame_offset: 2,
        pilot_gating_rate: PilotGatingRate::Full,
        arfcn: 283,
        otd: false,
        physical_channels: vec![PhysicalChannelType::Fch],
    }
}

fn test_a3_connect_ack_info() -> A3ConnectAckInformation {
    A3ConnectAckInformation {
        soft_handoff_leg: 1,
        pmc_cause: None,
        transmit_tch_status: true,
        traffic_circuit_id: test_traffic_circuit_id(),
        channel_element_id: vec![0x01],
        a3_originating_id: 0x0A0B,
        a3_destination_id: 0x0C0D,
    }
}

fn test_a3_remove_info() -> A3RemoveInformation {
    A3RemoveInformation {
        traffic_circuit_id: test_traffic_circuit_id(),
        cells_to_be_removed: vec![CellIdWithMscId {
            mscid: 0x001234,
            cell: 0x100,
            sector: 0x03,
        }],
        a3_destination_id: 0x0C0D,
        a7_destination_id: 0x0E0F,
    }
}

// ---- Typed message round-trip tests ----

#[test]
fn round_trip_bts_release() {
    let msg = BtsReleaseMessage {
        call_connection_reference: test_ccr(),
        cell_identifier_list: Some(vec![test_cell()]),
        correlation_id: Some(CorrelationId(0x11223344)),
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsReleaseMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_bts_release_ack() {
    let msg = BtsReleaseAckMessage {
        call_connection_reference: test_ccr(),
        correlation_id: Some(CorrelationId(0xDEAD_BEEF)),
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsReleaseAckMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_bts_release_request() {
    let msg = BtsReleaseRequestMessage {
        call_connection_reference: test_ccr(),
        cause: Some(0x07),
        manufacturer_specific_records: Some(ManufacturerSpecificRecords {
            manufacturer_id: 0x42,
            information: vec![0x01, 0x02, 0x03],
        }),
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsReleaseRequestMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_bts_release_request_minimal() {
    let msg = BtsReleaseRequestMessage {
        call_connection_reference: test_ccr(),
        cause: None,
        manufacturer_specific_records: None,
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsReleaseRequestMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_connect() {
    let msg = ConnectMessage {
        call_connection_reference: test_ccr(),
        correlation_id: Some(CorrelationId(0xAABBCCDD)),
        sdu_id: Some(SduId::new(vec![0x01, 0x02, 0x03]).unwrap()),
        connect_information: vec![test_a3_connect_info()],
        physical_channel_info: test_physical_channel_info(),
    };
    let encoded = msg.encode().unwrap();
    let decoded = ConnectMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_connect_ack() {
    let msg = ConnectAckMessage {
        call_connection_reference: test_ccr(),
        correlation_id: Some(CorrelationId(0x11111111)),
        connect_ack_information: vec![test_a3_connect_ack_info()],
    };
    let encoded = msg.encode().unwrap();
    let decoded = ConnectAckMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_remove() {
    let msg = RemoveMessage {
        call_connection_reference: test_ccr(),
        correlation_id: Some(CorrelationId(0x22222222)),
        sdu_id: Some(SduId::new(vec![0x0A]).unwrap()),
        remove_information: vec![test_a3_remove_info()],
    };
    let encoded = msg.encode().unwrap();
    let decoded = RemoveMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_remove_ack() {
    let msg = RemoveAckMessage {
        call_connection_reference: test_ccr(),
        correlation_id: Some(CorrelationId(0x33333333)),
        a3_destination_id: Some(0x0A0B),
    };
    let encoded = msg.encode().unwrap();
    let decoded = RemoveAckMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_traffic_channel_status() {
    let msg = TrafficChannelStatusMessage {
        call_connection_reference: test_ccr(),
        cell_identifier_list: vec![CellIdWithMscId {
            mscid: 0x001234,
            cell: 0x100,
            sector: 0x03,
        }],
        channel_element_status: ChannelElementStatus { transmit_on: true },
        sdu_id: Some(SduId::new(vec![0x01, 0x02]).unwrap()),
        a3_destination_id: Some(0x0A0B),
        a7_destination_id: Some(0x0C0D),
    };
    let encoded = msg.encode().unwrap();
    let decoded = TrafficChannelStatusMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_bts_setup_minimal() {
    let msg = BtsSetupMessage {
        call_connection_reference: test_ccr(),
        band_class: None,
        privacy_info: None,
        sdu_id: None,
        mobile_identities: vec![],
        physical_channel_info: None,
        service_option: None,
        paca_timestamp: None,
        quality_of_service_parameters: None,
        connect_information: vec![],
        abis_originating_id: None,
        cdma_serving_one_way_delay: test_serving_delay(),
        cdma_target_one_way_delay: None,
        walsh_code_assignment_request: false,
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsSetupMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_bts_setup_full() {
    let msg = BtsSetupMessage {
        call_connection_reference: test_ccr(),
        band_class: Some(BandClass::Pcs),
        privacy_info: Some(PrivacyInfo {
            privacy_masks: vec![PrivacyMaskInformation {
                privacy_mask_type: 1,
                status: true,
                available: true,
                privacy_mask: vec![0xAA, 0xBB, 0xCC],
            }],
        }),
        sdu_id: Some(SduId::new(vec![0x01]).unwrap()),
        mobile_identities: vec![MobileIdentity::Esn(0x12345678)],
        physical_channel_info: Some(test_physical_channel_info()),
        service_option: Some(ServiceOption::HIGH_RATE_PACKET_DATA),
        paca_timestamp: Some(PacaTimestamp(1000)),
        quality_of_service_parameters: Some(QualityOfServiceParameters { packet_priority: 5 }),
        connect_information: vec![test_a3_connect_info()],
        abis_originating_id: Some(AbisOriginatingId::new(vec![0x01, 0x02]).unwrap()),
        cdma_serving_one_way_delay: test_serving_delay(),
        cdma_target_one_way_delay: Some(CdmaTargetOneWayDelay(200)),
        walsh_code_assignment_request: true,
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsSetupMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_bts_setup_ack() {
    let msg = BtsSetupAckMessage {
        call_connection_reference: test_ccr(),
        connect_information: vec![test_a3_connect_info()],
        abis_originating_id: Some(AbisOriginatingId::new(vec![0x01]).unwrap()),
        abis_destination_id: Some(AbisDestinationId::new(vec![0x02]).unwrap()),
        cause: None,
    };
    let encoded = msg.encode().unwrap();
    let decoded = BtsSetupAckMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_pch_message_transfer_minimal() {
    let msg = PchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![],
        cell_identifier_list: None,
        air_interface_message: None,
        layer2_ack_request_results: None,
        abis_ack_notify: None,
    };
    let encoded = msg.encode().unwrap();
    let decoded = PchMessageTransferMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_pch_message_transfer_with_ack() {
    let msg = PchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x44444444)),
        mobile_identities: vec![MobileIdentity::Imsi("3101234567890".to_string())],
        cell_identifier_list: Some(vec![test_cell()]),
        air_interface_message: Some(
            AirInterfaceMessagePayload::new(0x20, vec![0xDE, 0xAD]).unwrap(),
        ),
        layer2_ack_request_results: Some(Layer2AckRequestResults { layer2_ack: true }),
        abis_ack_notify: Some(AbisAckNotify),
    };
    let encoded = msg.encode().unwrap();
    let decoded = PchMessageTransferMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_pch_message_transfer_ack() {
    let msg = PchMessageTransferAckMessage {
        correlation_id: Some(CorrelationId(0x55555555)),
        cause: Some(0x07),
        bts_l2_termination: Some(true),
    };
    let encoded = msg.encode().unwrap();
    let decoded = PchMessageTransferAckMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_ach_message_transfer() {
    let msg = AchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x66666666)),
        mobile_identities: vec![MobileIdentity::Esn(0xDEADBEEF)],
        cell_identifier: Some(test_cell()),
        bts_l2_termination: Some(true),
        air_interface_message: Some(
            AirInterfaceMessagePayload::new(0x10, vec![0x01, 0x02, 0x03]).unwrap(),
        ),
        cdma_serving_one_way_delay: test_serving_delay(),
        authentication_challenge_parameter: Some(AuthenticationChallengeParameter::new([
            0xAA, 0xBB, 0xCC, 0xDD,
        ])),
    };
    let encoded = msg.encode().unwrap();
    let decoded = AchMessageTransferMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_ach_message_transfer_minimal() {
    let msg = AchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![],
        cell_identifier: None,
        bts_l2_termination: None,
        air_interface_message: None,
        cdma_serving_one_way_delay: test_serving_delay(),
        authentication_challenge_parameter: None,
    };
    let encoded = msg.encode().unwrap();
    let decoded = AchMessageTransferMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_paca_update() {
    let msg = PacaUpdateMessage {
        call_connection_reference: test_ccr(),
        mobile_identity_imsi: Some(MobileIdentity::Imsi("3101234567890".to_string())),
        action_required: Some(PacaActionRequired::UpdateQueuePosition),
    };
    let encoded = msg.encode().unwrap();
    let decoded = PacaUpdateMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn round_trip_paca_update_remove() {
    let msg = PacaUpdateMessage {
        call_connection_reference: test_ccr(),
        mobile_identity_imsi: None,
        action_required: Some(PacaActionRequired::RemoveMsFromQueue),
    };
    let encoded = msg.encode().unwrap();
    let decoded = PacaUpdateMessage::decode(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

// ---- Raw AbisMessage codec round-trip ----

#[test]
fn raw_codec_round_trip_bts_release() {
    let msg = BtsReleaseMessage {
        call_connection_reference: test_ccr(),
        cell_identifier_list: None,
        correlation_id: None,
    };
    let wire = msg.encode().unwrap();
    let abis_msg = decode(&wire).unwrap();
    assert_eq!(abis_msg.message_type, MessageType::BtsRelease);
    let re_encoded = encode(&abis_msg).unwrap();
    assert_eq!(wire[1..], re_encoded[1..]);
    let decoded2 = BtsReleaseMessage::decode(&re_encoded).unwrap();
    assert_eq!(
        msg.call_connection_reference,
        decoded2.call_connection_reference
    );
}

#[test]
fn raw_codec_round_trip_connect() {
    let msg = ConnectMessage {
        call_connection_reference: test_ccr(),
        correlation_id: None,
        sdu_id: None,
        connect_information: vec![test_a3_connect_info()],
        physical_channel_info: test_physical_channel_info(),
    };
    let wire = msg.encode().unwrap();
    let abis_msg = decode(&wire).unwrap();
    assert_eq!(abis_msg.message_type, MessageType::Connect);
    let re_encoded = encode(&abis_msg).unwrap();
    assert_eq!(wire, re_encoded);
}

#[test]
fn raw_codec_round_trip_pch_transfer() {
    let msg = PchMessageTransferMessage {
        correlation_id: Some(CorrelationId(0x99887766)),
        mobile_identities: vec![],
        cell_identifier_list: None,
        air_interface_message: Some(AirInterfaceMessagePayload::new(0x20, vec![0x01]).unwrap()),
        layer2_ack_request_results: None,
        abis_ack_notify: None,
    };
    let wire = msg.encode().unwrap();
    let abis_msg = decode(&wire).unwrap();
    assert_eq!(abis_msg.message_type, MessageType::PchMessageTransfer);
    let re_encoded = encode(&abis_msg).unwrap();
    assert_eq!(wire, re_encoded);
}

// ---- Identifier IE tests ----

#[test]
fn call_connection_reference_round_trip() {
    let ccr = test_ccr();
    let encoded = ccr.encode();
    let decoded = CallConnectionReference::decode(&encoded).unwrap();
    assert_eq!(ccr, decoded);
}

#[test]
fn correlation_id_round_trip() {
    let cid = CorrelationId(0x12345678);
    let encoded = cid.encode();
    let decoded = CorrelationId::decode(&encoded).unwrap();
    assert_eq!(cid, decoded);
}

#[test]
fn sdu_id_round_trip() {
    let sdu = SduId::new(vec![0x01, 0x02, 0x03, 0x04]).unwrap();
    let encoded = sdu.encode();
    let decoded = SduId::decode(encoded).unwrap();
    assert_eq!(sdu, decoded);
}

#[test]
fn mobile_identity_imsi_round_trip() {
    let imsi = MobileIdentity::Imsi("310123456789012".to_string());
    let encoded = imsi.encode().unwrap();
    let decoded = MobileIdentity::decode(&encoded).unwrap();
    assert_eq!(imsi, decoded);
}

#[test]
fn mobile_identity_imsi_even_digits_round_trip() {
    let imsi = MobileIdentity::Imsi("31012345678901".to_string());
    let encoded = imsi.encode().unwrap();
    let decoded = MobileIdentity::decode(&encoded).unwrap();
    assert_eq!(imsi, decoded);
}

#[test]
fn mobile_identity_imsi_10_digits() {
    let imsi = MobileIdentity::Imsi("3101234567".to_string());
    let encoded = imsi.encode().unwrap();
    let decoded = MobileIdentity::decode(&encoded).unwrap();
    assert_eq!(imsi, decoded);
}

#[test]
fn mobile_identity_esn_round_trip() {
    let esn = MobileIdentity::Esn(0xDEADBEEF);
    let encoded = esn.encode().unwrap();
    let decoded = MobileIdentity::decode(&encoded).unwrap();
    assert_eq!(esn, decoded);
}

#[test]
fn cell_id_round_trip() {
    let cell = test_cell();
    let encoded = cell.encode().unwrap();
    let decoded = CellId::decode(&encoded).unwrap();
    assert_eq!(cell, decoded);
}

#[test]
fn cell_id_with_mscid_round_trip() {
    let cell = CellIdWithMscId {
        mscid: 0x001234,
        cell: 0x100,
        sector: 0x03,
    };
    let encoded = cell.encode().unwrap();
    let decoded = CellIdWithMscId::decode(&encoded).unwrap();
    assert_eq!(cell, decoded);
}

// ---- Sub-structure round-trip tests ----

#[test]
fn physical_channel_info_round_trip() {
    let pci = test_physical_channel_info();
    let encoded = pci.encode().unwrap();
    let decoded = PhysicalChannelInfo::decode(&encoded).unwrap();
    assert_eq!(pci, decoded);
}

#[test]
fn physical_channel_info_two_channels() {
    let pci = PhysicalChannelInfo {
        frame_offset: 5,
        pilot_gating_rate: PilotGatingRate::Half,
        arfcn: 100,
        otd: true,
        physical_channels: vec![PhysicalChannelType::Fch, PhysicalChannelType::Dcch],
    };
    let encoded = pci.encode().unwrap();
    let decoded = PhysicalChannelInfo::decode(&encoded).unwrap();
    assert_eq!(pci, decoded);
}

#[test]
fn extended_handoff_direction_parameters_round_trip() {
    let params = ExtendedHandoffDirectionParameters {
        search_window_a_size: 15,
        search_window_n_size: 14,
        search_window_r_size: 13,
        t_add: 63,
        t_drop: 62,
        compare_threshold: 15,
        drop_timer_value: 14,
        neighbor_max_age: 13,
        soft_slope: 63,
        add_intercept: 63,
        drop_intercept: 63,
        target_bs_p_rev: 7,
    };
    let encoded = params.encode().unwrap();
    let decoded = ExtendedHandoffDirectionParameters::decode(&encoded).unwrap();
    assert_eq!(params, decoded);
}

#[test]
fn traffic_circuit_id_round_trip() {
    let tcid = test_traffic_circuit_id();
    let encoded = tcid.encode();
    let decoded = TrafficCircuitId::decode(&encoded).unwrap();
    assert_eq!(tcid, decoded);
}

#[test]
fn forward_burst_radio_info_round_trip() {
    let info = ForwardBurstRadioInfo {
        coding_indicator: 1,
        qof_mask: 2,
        forward_code_channel_index: 0x0123,
        pilot_pn_code: 0x01AB,
        forward_supplemental_channel_rate: 5,
        forward_supplemental_channel_start_time: 10,
        start_time_unit: 3,
        forward_supplemental_channel_duration: 7,
    };
    let encoded = info.encode().unwrap();
    let decoded = ForwardBurstRadioInfo::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn reverse_burst_radio_info_round_trip() {
    let info = ReverseBurstRadioInfo {
        coding_indicator: 1,
        reverse_supplemental_channel_rate: 8,
        reverse_supplemental_channel_start_time: 200,
        start_time_unit: 5,
        reverse_supplemental_channel_duration: 12,
    };
    let encoded = info.encode().unwrap();
    let decoded = ReverseBurstRadioInfo::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn is2000_fpc_gain_ratio_info_round_trip() {
    let info = Is2000FpcGainRatioInfo {
        initial_gain_ratio: 0x80,
        gain_adjust_step_size: 5,
        gain_ratio_pairs: [
            GainRatioPair {
                min_gain_ratio: 10,
                max_gain_ratio: 20,
            },
            GainRatioPair {
                min_gain_ratio: 30,
                max_gain_ratio: 40,
            },
            GainRatioPair {
                min_gain_ratio: 50,
                max_gain_ratio: 60,
            },
        ],
    };
    let encoded = info.encode().unwrap();
    let decoded = Is2000FpcGainRatioInfo::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn a3_connect_info_round_trip() {
    let info = test_a3_connect_info();
    let encoded = info.encode().unwrap();
    let decoded = A3ConnectInformation::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn a3_connect_ack_info_round_trip() {
    let info = test_a3_connect_ack_info();
    let encoded = info.encode().unwrap();
    let decoded = A3ConnectAckInformation::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn a3_connect_ack_info_with_pmc_cause() {
    let info = A3ConnectAckInformation {
        soft_handoff_leg: 2,
        pmc_cause: Some(0x05),
        transmit_tch_status: false,
        traffic_circuit_id: test_traffic_circuit_id(),
        channel_element_id: vec![0x01, 0x02],
        a3_originating_id: 0x0A0B,
        a3_destination_id: 0x0C0D,
    };
    let encoded = info.encode().unwrap();
    let decoded = A3ConnectAckInformation::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn a3_remove_info_round_trip() {
    let info = test_a3_remove_info();
    let encoded = info.encode().unwrap();
    let decoded = A3RemoveInformation::decode(&encoded).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn downlink_radio_environment_round_trip() {
    let env = DownlinkRadioEnvironment {
        records: vec![DownlinkRadioEnvironmentRecord {
            cell: test_cell(),
            downlink_signal_strength_raw: 30,
            cdma_target_one_way_delay: 1000,
        }],
    };
    let encoded = env.encode().unwrap();
    let decoded = DownlinkRadioEnvironment::decode(&encoded).unwrap();
    assert_eq!(env, decoded);
}

// ---- Signaling framing tests ----

#[test]
fn signaling_frame_round_trip() {
    let payload = vec![0x01, 0x02, 0x03, 0x04];
    let frame = SignalingFrame::new(payload.clone());
    let encoded = frame.encode().unwrap();
    let decoded = SignalingFrame::decode(&encoded).unwrap();
    assert_eq!(frame, decoded);
    assert_eq!(decoded.payload, payload);
}

#[test]
fn signaling_frame_prefix_decode() {
    let payload = vec![0xAA, 0xBB];
    let frame = SignalingFrame::new(payload);
    let mut bytes = frame.encode().unwrap();
    bytes.extend_from_slice(&[0xFF, 0xFF]);
    let (decoded, consumed) = SignalingFrame::decode_prefix(&bytes).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(consumed, 6);
}

#[test]
fn signaling_stream_decoder_single_frame() {
    let frame = SignalingFrame::new(vec![0x01, 0x02, 0x03]);
    let encoded = frame.encode().unwrap();
    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&encoded);
    let result = decoder.next_frame().unwrap();
    assert_eq!(result, Some(frame));
    assert_eq!(decoder.buffered_len(), 0);
}

#[test]
fn signaling_stream_decoder_partial_then_complete() {
    let frame = SignalingFrame::new(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let encoded = frame.encode().unwrap();
    let mut decoder = SignalingFrameStreamDecoder::new();

    decoder.push_bytes(&encoded[..3]);
    assert_eq!(decoder.next_frame().unwrap(), None);

    decoder.push_bytes(&encoded[3..]);
    let result = decoder.next_frame().unwrap();
    assert_eq!(result, Some(frame));
}

#[test]
fn signaling_stream_decoder_two_frames() {
    let frame1 = SignalingFrame::new(vec![0x01]);
    let frame2 = SignalingFrame::new(vec![0x02, 0x03]);
    let mut bytes = frame1.encode().unwrap();
    bytes.extend(frame2.encode().unwrap());
    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&bytes);
    assert_eq!(decoder.next_frame().unwrap(), Some(frame1));
    assert_eq!(decoder.next_frame().unwrap(), Some(frame2));
    assert_eq!(decoder.next_frame().unwrap(), None);
}

#[test]
fn signaling_stream_decoder_resync_on_garbage() {
    let frame = SignalingFrame::new(vec![0xAA]);
    let encoded = frame.encode().unwrap();
    let mut bytes = vec![0xFF, 0xFF, 0xFF];
    bytes.extend(encoded);
    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&bytes);
    let result = decoder.next_frame().unwrap();
    assert_eq!(result, Some(frame));
}

// ---- Full pipeline: typed message -> wire encode -> framing -> wire decode -> typed message ----

#[test]
fn full_pipeline_bts_release_through_signaling_frame() {
    let msg = BtsReleaseMessage {
        call_connection_reference: test_ccr(),
        cell_identifier_list: Some(vec![test_cell()]),
        correlation_id: Some(CorrelationId(0xCAFEBABE)),
    };
    let wire = msg.encode().unwrap();
    let frame = SignalingFrame::new(wire.clone());
    let framed = frame.encode().unwrap();

    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&framed);
    let recovered_frame = decoder.next_frame().unwrap().unwrap();
    assert_eq!(recovered_frame.payload, wire);

    let decoded_msg = BtsReleaseMessage::decode(&recovered_frame.payload).unwrap();
    assert_eq!(msg, decoded_msg);
}

#[test]
fn full_pipeline_connect_through_signaling_frame() {
    let msg = ConnectMessage {
        call_connection_reference: test_ccr(),
        correlation_id: None,
        sdu_id: None,
        connect_information: vec![test_a3_connect_info()],
        physical_channel_info: test_physical_channel_info(),
    };
    let wire = msg.encode().unwrap();
    let frame = SignalingFrame::new(wire.clone());
    let framed = frame.encode().unwrap();

    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&framed);
    let recovered = decoder.next_frame().unwrap().unwrap();
    let decoded_msg = ConnectMessage::decode(&recovered.payload).unwrap();
    assert_eq!(msg, decoded_msg);
}

// ---- Validation / error tests ----

#[test]
fn imsi_too_short_fails() {
    let imsi = MobileIdentity::Imsi("123456789".to_string());
    assert!(imsi.encode().is_err());
}

#[test]
fn imsi_too_long_fails() {
    let imsi = MobileIdentity::Imsi("1234567890123456".to_string());
    assert!(imsi.encode().is_err());
}

#[test]
fn sdu_id_too_long_fails() {
    assert!(SduId::new(vec![1, 2, 3, 4, 5, 6, 7]).is_err());
}

#[test]
fn sdu_id_empty_fails() {
    assert!(SduId::new(Vec::<u8>::new()).is_err());
}

#[test]
fn cell_id_zero_fails() {
    let cell = CellId { cell: 0, sector: 0 };
    assert!(cell.encode().is_err());
}

#[test]
fn pch_transfer_ack_no_correlation_ok() {
    let msg = PchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![],
        cell_identifier_list: None,
        air_interface_message: None,
        layer2_ack_request_results: None,
        abis_ack_notify: None,
    };
    assert!(msg.encode().is_ok());
}

#[test]
fn pch_transfer_ack_requires_correlation_with_l2_ack() {
    let msg = PchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![],
        cell_identifier_list: None,
        air_interface_message: None,
        layer2_ack_request_results: Some(Layer2AckRequestResults { layer2_ack: true }),
        abis_ack_notify: None,
    };
    assert!(msg.encode().is_err());
}

#[test]
fn ach_transfer_auth_requires_mobile_identity() {
    let msg = AchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![],
        cell_identifier: None,
        bts_l2_termination: None,
        air_interface_message: None,
        cdma_serving_one_way_delay: test_serving_delay(),
        authentication_challenge_parameter: Some(AuthenticationChallengeParameter::new([0x00; 4])),
    };
    assert!(msg.encode().is_err());
}

// ---- Message type byte verification ----

#[test]
fn message_type_values_match_spec() {
    assert_eq!(MessageType::Connect.value(), 0x01);
    assert_eq!(MessageType::ConnectAck.value(), 0x02);
    assert_eq!(MessageType::Remove.value(), 0x03);
    assert_eq!(MessageType::RemoveAck.value(), 0x04);
    assert_eq!(MessageType::FchForward.value(), 0x0b);
    assert_eq!(MessageType::FchReverse.value(), 0x0c);
    assert_eq!(MessageType::TrafficChannelStatus.value(), 0x0d);
    assert_eq!(MessageType::BtsSetup.value(), 0x80);
    assert_eq!(MessageType::BtsSetupAck.value(), 0x81);
    assert_eq!(MessageType::BtsRelease.value(), 0x82);
    assert_eq!(MessageType::BtsReleaseAck.value(), 0x83);
    assert_eq!(MessageType::BtsReleaseRequest.value(), 0x84);
    assert_eq!(MessageType::PchMessageTransfer.value(), 0x8c);
    assert_eq!(MessageType::PchMessageTransferAck.value(), 0x8d);
    assert_eq!(MessageType::AchMessageTransfer.value(), 0x8e);
    assert_eq!(MessageType::BurstRequest.value(), 0x90);
    assert_eq!(MessageType::BurstResponse.value(), 0x91);
    assert_eq!(MessageType::BurstCommit.value(), 0x92);
    assert_eq!(MessageType::PacaUpdate.value(), 0x6e);
}

// ---- IE identifier byte verification ----

#[test]
fn element_id_values_match_spec() {
    assert_eq!(ElementId::ServiceOption.value(), 0x03);
    assert_eq!(ElementId::Cause.value(), 0x04);
    assert_eq!(ElementId::CellIdentifier.value(), 0x05);
    assert_eq!(ElementId::CallConnectionReference.value(), 0x3f);
    assert_eq!(ElementId::CorrelationId.value(), 0x13);
    assert_eq!(ElementId::SduId.value(), 0x4c);
    assert_eq!(ElementId::MobileIdentity.value(), 0x0d);
    assert_eq!(ElementId::AirInterfaceMessage.value(), 0x21);
    assert_eq!(ElementId::AbisOriginatingId.value(), 0x71);
    assert_eq!(ElementId::AbisDestinationId.value(), 0x72);
    assert_eq!(ElementId::BtsL2Termination.value(), 0x73);
    assert_eq!(ElementId::AbisAckNotify.value(), 0x75);
    assert_eq!(ElementId::ManufacturerSpecificRecords.value(), 0x70);
}

// ---- Timer tests ----

#[test]
fn timer_defaults_match_spec_table() {
    use cdma_abis::control::AbisTimerKind;
    assert_eq!(AbisTimerKind::Tconnb.definition().default_ms, 100);
    assert_eq!(AbisTimerKind::Tsetupb.definition().default_ms, 100);
    assert_eq!(AbisTimerKind::Tchanstatb.definition().default_ms, 500);
    assert_eq!(AbisTimerKind::Tdisconb.definition().default_ms, 100);
    assert_eq!(AbisTimerKind::Tdrptgtb.definition().default_ms, 500);
    assert_eq!(AbisTimerKind::Tbstreqb.definition().default_ms, 500);
    assert_eq!(AbisTimerKind::Tbstcomb.definition().default_ms, 500);
    assert_eq!(AbisTimerKind::Trelreqb.definition().default_ms, 100);
}
