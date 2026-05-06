use cdma_abis::Error;
use cdma_abis::bearer::{
    ChannelFamily, Direction, ForwardFchDcchFrame, FrameContent, ReverseFchDcchFrame,
    ReverseSchFrame, TrafficFrame,
};
use cdma_abis::control::{
    A3ConnectInformation, AbisMessage, AbisOriginatingId, BtsSetupMessage, CallConnectionReference,
    CdmaServingOneWayDelay, CellId, CellInfoRecord, ConnectMessage, Direction as ControlDirection,
    ElementId, ExtendedHandoffDirectionParameters, InformationElement, MessageType, PacaTimestamp,
    PhysicalChannelInfo, PhysicalChannelType, PilotGatingRate, PrivacyInfo, PrivacyMaskInformation,
    QualityOfServiceParameters, SduId, ServiceOption, TrafficCircuitId, decode, encode,
};
use cdma_abis::udp_bearer::{HEADER_LEN, UdpBearerDatagram};

fn cc_ref() -> InformationElement {
    InformationElement::new(ElementId::CallConnectionReference, [0, 1, 0, 2, 0, 0, 0, 3])
}

fn call_ref_typed() -> CallConnectionReference {
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

#[test]
fn bts_setup_minimal_golden_roundtrip() {
    let message = AbisMessage::new(
        MessageType::BtsSetup,
        vec![
            cc_ref(),
            InformationElement::new(
                ElementId::CdmaServingOneWayDelay,
                [0x02, 0x01, 0x23, 0x12, 0x34],
            ),
        ],
    )
    .unwrap();

    let encoded = encode(&message).unwrap();
    assert_eq!(
        encoded,
        vec![
            0x80, 0x3f, 0x08, 0, 1, 0, 2, 0, 0, 0, 3, 0x0c, 0x05, 0x02, 0x01, 0x23, 0x12, 0x34
        ]
    );
    assert_eq!(decode(&encoded).unwrap(), message);
}

#[test]
fn rejects_missing_required_ie() {
    let error = AbisMessage::new(MessageType::BtsSetup, vec![cc_ref()]).unwrap_err();
    assert_eq!(
        error,
        Error::MissingRequiredElement {
            message_type: 0x80,
            id: 0x0c
        }
    );
}

#[test]
fn rejects_out_of_order_ie() {
    let error = AbisMessage::new(
        MessageType::BtsRelease,
        vec![
            InformationElement::new(ElementId::CorrelationId, [1, 2, 3, 4]),
            cc_ref(),
        ],
    )
    .unwrap_err();
    assert_eq!(
        error,
        Error::OutOfOrderElement {
            message_type: 0x82,
            id: 0x3f
        }
    );
}

#[test]
fn decodes_all_control_message_type_values() {
    let values = [
        0x01, 0x02, 0x03, 0x04, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x6e, 0x80, 0x81, 0x82,
        0x83, 0x84, 0x8c, 0x8d, 0x8e, 0x90, 0x91, 0x92,
    ];
    for value in values {
        assert_eq!(MessageType::from_u8(value).unwrap().value(), value);
    }
}

#[test]
fn bts_release_ack_direction_matches_spec() {
    assert_eq!(
        MessageType::BtsReleaseAck.direction(),
        ControlDirection::BtsToBsc
    );
}

#[test]
fn frame_content_maps_all_is2001_table_values() {
    let values = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x20, 0x21, 0x22, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37,
        0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x50, 0x51, 0x52,
        0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b, 0x5c, 0x5d, 0x5e, 0x5f, 0x60, 0x61,
        0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70,
        0x71, 0x7d, 0x7e, 0x7f,
    ];
    for value in values {
        assert_eq!(FrameContent::from_u8(value).unwrap().value(), value);
    }
    assert!(FrameContent::from_u8(0x13).is_none());
    assert_eq!(FrameContent::FchRc5_7200.value(), 0x10);
    assert_eq!(FrameContent::FchRc5_7200.rate_bps(), Some(7200));
}

#[test]
fn bearer_forward_fch_golden_roundtrip() {
    let frame = TrafficFrame::ForwardFchDcch(ForwardFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        fpc_slc: 1,
        fsn: 9,
        fpc_gr: 0x22,
        rpc_olt: 0x33,
        frame_content: FrameContent::FchRc1_1200,
        forward_link_information: vec![0xaa, 0xbb, 0xcc],
        message_crc: 0x1234,
    });
    let encoded = frame.encode().unwrap();
    assert_eq!(
        encoded,
        vec![0x0b, 0x19, 0x22, 0x33, 0x04, 0xaa, 0xbb, 0xcc, 0x12, 0x34]
    );
    assert_eq!(
        TrafficFrame::decode(ChannelFamily::Fch, Direction::Forward, &encoded).unwrap(),
        frame
    );
}

#[test]
fn bearer_reverse_fch_golden_roundtrip() {
    let frame = TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 2,
        fsn: 3,
        fqi: true,
        reverse_link_quality: 0x44,
        scaling: 2,
        packet_arrival_time_error: 0x15,
        frame_content: FrameContent::FchRc3Forward5ms_9600,
        fpc_s: 0x12,
        eib: true,
        reverse_link_information: vec![0xde, 0xad],
        message_crc: 0xbeef,
    });
    let encoded = frame.encode().unwrap();
    assert_eq!(
        encoded,
        vec![0x0c, 0x23, 0xc4, 0x95, 0x09, 0x25, 0xde, 0xad, 0xbe, 0xef]
    );
    assert_eq!(
        TrafficFrame::decode(ChannelFamily::Fch, Direction::Reverse, &encoded).unwrap(),
        frame
    );
}

#[test]
fn bearer_fch_rejects_mismatched_message_type() {
    let encoded = vec![0x0e, 0x19, 0x22, 0x33, 0x04, 0x12, 0x34];
    let err = TrafficFrame::decode(ChannelFamily::Fch, Direction::Forward, &encoded).unwrap_err();
    assert!(matches!(err, Error::InvalidValue { .. }));
}

#[test]
fn bearer_reverse_sch_golden_roundtrip() {
    let frame = TrafficFrame::ReverseSch(ReverseSchFrame {
        soft_handoff_leg: 2,
        fsn: 3,
        fqi: true,
        reverse_link_quality: 0x44,
        scaling: 2,
        packet_arrival_time_error: 0x15,
        frame_content: FrameContent::Sch20msRc4_115200,
        reverse_link_information: vec![0xde, 0xad],
        message_crc: 0xbeef,
    });
    let encoded = frame.encode().unwrap();
    assert_eq!(
        encoded,
        vec![0x23, 0xc4, 0x95, 0x3d, 0xde, 0xad, 0xbe, 0xef]
    );
    assert_eq!(
        TrafficFrame::decode(ChannelFamily::Sch, Direction::Reverse, &encoded).unwrap(),
        frame
    );
}

#[test]
fn udp_bearer_header_golden_roundtrip() {
    let packet = UdpBearerDatagram {
        flags: 0x80,
        channel_family: ChannelFamily::Dcch,
        direction: Direction::Reverse,
        bts_id: 1,
        cell_id: 2,
        bearer_id: 3,
        sequence_no: 4,
        tx_frame_number: 5,
        payload: vec![0xaa, 0xbb],
    };
    let encoded = packet.encode().unwrap();
    assert_eq!(encoded.len(), HEADER_LEN + 2);
    assert_eq!(
        &encoded[..28],
        &[
            1, 0x80, 3, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 0
        ]
    );
    assert_eq!(UdpBearerDatagram::decode(&encoded).unwrap(), packet);
}

#[test]
fn generic_decode_classifies_connect_physical_channel_info() {
    let encoded = ConnectMessage {
        call_connection_reference: call_ref_typed(),
        correlation_id: None,
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        connect_information: vec![connect_information()],
        physical_channel_info: physical_channel_info(),
    }
    .encode()
    .unwrap();

    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.message_type, MessageType::Connect);
    assert!(
        decoded
            .elements
            .iter()
            .any(|element| element.id == ElementId::PhysicalChannelInfo)
    );
}

#[test]
fn generic_decode_preserves_bts_setup_ie07_roles_and_status_inventory() {
    let encoded = BtsSetupMessage {
        call_connection_reference: call_ref_typed(),
        band_class: None,
        privacy_info: Some(PrivacyInfo {
            privacy_masks: vec![PrivacyMaskInformation {
                privacy_mask_type: 0x01,
                status: true,
                available: true,
                privacy_mask: vec![0xde, 0xad],
            }],
        }),
        sdu_id: Some(SduId::new([0x55]).unwrap()),
        mobile_identities: vec![],
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
            delay_100ns: 0x2222,
        },
        cdma_target_one_way_delay: None,
        walsh_code_assignment_request: false,
    }
    .encode()
    .unwrap();

    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.message_type, MessageType::BtsSetup);
    assert_eq!(
        decoded
            .elements
            .iter()
            .filter(|element| element.id == ElementId::PhysicalChannelInfo)
            .count(),
        1
    );
    assert_eq!(
        decoded
            .elements
            .iter()
            .filter(|element| element.id == ElementId::QualityOfServiceParameters)
            .count(),
        1
    );
    assert!(
        decoded
            .elements
            .iter()
            .any(|element| element.id == ElementId::ServiceOption)
    );
    assert!(
        decoded
            .elements
            .iter()
            .any(|element| element.id == ElementId::PacaTimestamp)
    );
}

#[test]
fn bts_setup_service_option_uses_fixed_framing() {
    let encoded = BtsSetupMessage {
        call_connection_reference: call_ref_typed(),
        band_class: None,
        privacy_info: None,
        sdu_id: None,
        mobile_identities: vec![],
        physical_channel_info: Some(physical_channel_info()),
        service_option: Some(ServiceOption(0x0021)),
        paca_timestamp: None,
        quality_of_service_parameters: None,
        connect_information: vec![connect_information()],
        abis_originating_id: None,
        cdma_serving_one_way_delay: CdmaServingOneWayDelay {
            cell: CellId {
                cell: 0x123,
                sector: 0x4,
            },
            delay_100ns: 0x2222,
        },
        cdma_target_one_way_delay: None,
        walsh_code_assignment_request: false,
    }
    .encode()
    .unwrap();

    assert!(
        encoded
            .windows(3)
            .any(|window| window == [0x03, 0x00, 0x21])
    );
    assert!(
        !encoded
            .windows(4)
            .any(|window| window == [0x03, 0x02, 0x00, 0x21])
    );
    assert_eq!(
        decode(&encoded).unwrap().message_type,
        MessageType::BtsSetup
    );
}
