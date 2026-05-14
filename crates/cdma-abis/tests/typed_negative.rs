use cdma_abis::control::{
    AbisDestinationId, AbisMessage, AchMessageTransferMessage, AirInterfaceMessagePayload,
    AuthenticationChallengeParameter, BtsReleaseRequestMessage, BtsSetupAckMessage,
    BurstResponseMessage, CallConnectionReference, CdmaServingOneWayDelay, CellId,
    ChannelElementStatus, CorrelationId, ElementId, InformationElement, Layer2AckRequestResults,
    MessageType, MobileIdentity, PacaUpdateMessage, PchMessageTransferAckMessage,
    PchMessageTransferMessage, ServiceOption,
};

fn encode_raw(message: &AbisMessage) -> Vec<u8> {
    let mut encoded = vec![message.message_type.value()];
    for element in &message.elements {
        element.encode(&mut encoded).unwrap();
    }
    encoded
}

#[test]
fn ach_decode_allows_missing_air_interface_message() {
    let message = AbisMessage {
        message_type: MessageType::AchMessageTransfer,
        elements: vec![
            InformationElement::new(
                ElementId::MobileIdentity,
                MobileIdentity::Imsi("12345678901".to_string())
                    .encode()
                    .unwrap(),
            ),
            InformationElement::new(
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
            ),
        ],
    };
    let encoded = encode_raw(&message);
    let decoded = AchMessageTransferMessage::decode(&encoded).unwrap();
    assert_eq!(decoded.air_interface_message, None);
}

#[test]
fn pch_decode_rejects_ack_ie_without_correlation() {
    let message = AbisMessage {
        message_type: MessageType::PchMessageTransfer,
        elements: vec![
            InformationElement::new(
                ElementId::MobileIdentity,
                MobileIdentity::Imsi("12345678901".to_string())
                    .encode()
                    .unwrap(),
            ),
            InformationElement::new(ElementId::AirInterfaceMessage, [0xca, 0x02, 0xba, 0xbe]),
            InformationElement::new(ElementId::AbisAckNotify, []),
        ],
    };
    let encoded = encode_raw(&message);

    assert_eq!(
        PchMessageTransferMessage::decode(&encoded).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Abis-PCH Msg Transfer",
            reason: "ack-related IEs require a correlation identifier",
        }
    );
}

#[test]
fn encode_allows_raw_ach_message_without_air_interface_message() {
    let message = AbisMessage {
        message_type: MessageType::AchMessageTransfer,
        elements: vec![InformationElement::new(
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
        )],
    };
    assert_eq!(
        cdma_abis::control::encode(&message).unwrap(),
        encode_raw(&message)
    );
}

#[test]
fn encode_rejects_invalid_raw_pch_message() {
    let message = AbisMessage {
        message_type: MessageType::PchMessageTransfer,
        elements: vec![
            InformationElement::new(ElementId::AirInterfaceMessage, [0xca, 0x02, 0xba, 0xbe]),
            InformationElement::new(ElementId::AbisAckNotify, []),
        ],
    };

    assert_eq!(
        cdma_abis::control::encode(&message).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Abis-PCH Msg Transfer",
            reason: "ack-related IEs require a correlation identifier",
        }
    );
}

#[test]
fn encode_allows_raw_pch_message_without_air_interface_message() {
    let message = AbisMessage {
        message_type: MessageType::PchMessageTransfer,
        elements: vec![
            InformationElement::new(ElementId::CorrelationId, CorrelationId(0x01020304).encode()),
            InformationElement::new(
                ElementId::Layer2AckRequestResults,
                Layer2AckRequestResults::request().encode(),
            ),
            InformationElement::new(ElementId::AbisAckNotify, []),
        ],
    };

    assert_eq!(
        cdma_abis::control::encode(&message).unwrap(),
        encode_raw(&message)
    );
}

#[test]
fn air_interface_message_rejects_empty_payload() {
    assert_eq!(
        AirInterfaceMessagePayload::new(0xca, Vec::<u8>::new()).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Air Interface Message",
            reason: "payload must not be empty",
        }
    );
}

#[test]
fn air_interface_message_rejects_length_mismatch() {
    assert_eq!(
        AirInterfaceMessagePayload::decode(&[0xca, 0x03, 0xba, 0xbe]).unwrap_err(),
        cdma_abis::Error::InvalidLength {
            context: "Air Interface Message",
            expected: 5,
            actual: 4,
        }
    );
}

#[test]
fn authentication_challenge_parameter_rejects_empty_payload() {
    assert_eq!(
        AuthenticationChallengeParameter::decode(&[]).unwrap_err(),
        cdma_abis::Error::InvalidLength {
            context: "Authentication Challenge Parameter",
            expected: 5,
            actual: 0,
        }
    );
}

#[test]
fn burst_response_rejects_multiple_committed_cells() {
    let message = BurstResponseMessage {
        call_connection_reference: None,
        correlation_id: Some(CorrelationId(0x01020304)),
        committed_cell_identifier_list: Some(vec![
            CellId {
                cell: 0x123,
                sector: 0x4,
            },
            CellId {
                cell: 0x124,
                sector: 0x5,
            },
        ]),
        uncommitted_cell_identifier_list: None,
        forward_burst_radio_info: None,
        reverse_burst_radio_info: None,
        abis_destination_id: Some(AbisDestinationId::new([0x22, 0x22]).unwrap()),
    };
    assert_eq!(
        message.encode().unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Abis-Burst Response",
            reason: "each response may include at most one committed and one uncommitted cell",
        }
    );
}

#[test]
fn fixed_service_option_ie_rejects_truncated_payload() {
    assert_eq!(
        InformationElement::decode(&[ElementId::ServiceOption.value(), 0x00]).unwrap_err(),
        cdma_abis::Error::Truncated {
            context: "Abis information element value",
            needed: 3,
            actual: 2,
        }
    );
}

#[test]
fn fixed_service_option_ie_encodes_without_length_octet() {
    let mut encoded = Vec::new();
    InformationElement::new(
        ElementId::ServiceOption,
        ServiceOption::HIGH_RATE_PACKET_DATA.encode(),
    )
    .encode(&mut encoded)
    .unwrap();
    assert_eq!(encoded, vec![ElementId::ServiceOption.value(), 0x00, 0x21]);
}

fn call_ref() -> CallConnectionReference {
    CallConnectionReference {
        market_id: 0x0001,
        generating_entity_id: 0x0002,
        call_connection_reference: 0x0000_0003,
    }
}

#[test]
fn pch_message_transfer_ack_rejects_non_spec_cause() {
    assert_eq!(
        PchMessageTransferAckMessage {
            correlation_id: Some(CorrelationId(0x01020304)),
            cause: Some(0x00),
            bts_l2_termination: Some(true),
        }
        .encode()
        .unwrap_err(),
        cdma_abis::Error::ReservedValue {
            context: "Abis-PCH Msg Transfer Ack cause",
            value: 0x00,
        }
    );
}

#[test]
fn bts_setup_ack_rejects_non_spec_cause() {
    assert_eq!(
        BtsSetupAckMessage {
            call_connection_reference: call_ref(),
            connect_information: vec![],
            abis_originating_id: None,
            abis_destination_id: None,
            cause: Some(0x20),
        }
        .encode()
        .unwrap_err(),
        cdma_abis::Error::ReservedValue {
            context: "Abis-BTS Setup Ack cause",
            value: 0x20,
        }
    );
}

#[test]
fn bts_release_request_rejects_non_spec_cause() {
    assert_eq!(
        BtsReleaseRequestMessage {
            call_connection_reference: call_ref(),
            cause: Some(0x21),
            manufacturer_specific_records: None,
        }
        .encode()
        .unwrap_err(),
        cdma_abis::Error::ReservedValue {
            context: "Abis-BTS Release Request cause",
            value: 0x21,
        }
    );
}

#[test]
fn cell_identifier_rejects_zero_cell_value() {
    assert_eq!(
        CellId::decode(&[0x02, 0x00, 0x04]).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Cell Identifier",
            reason: "cell identifier must be in the range 0x001..=0x0fff",
        }
    );
}

#[test]
fn layer2_ack_rejects_reserved_bits() {
    assert_eq!(
        Layer2AckRequestResults::decode(&[0x81]).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Layer 2 Ack Request Results",
            reason: "reserved bits must be zero",
        }
    );
}

#[test]
fn channel_element_status_rejects_reserved_bits() {
    assert_eq!(
        ChannelElementStatus::decode(&[0x02]).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "Channel Element Status",
            reason: "reserved bits must be zero",
        }
    );
}

#[test]
fn paca_update_rejects_reserved_paca_order_bits() {
    let encoded = vec![
        MessageType::PacaUpdate.value(),
        ElementId::CallConnectionReference.value(),
        0x08,
        0x00,
        0x01,
        0x00,
        0x02,
        0x00,
        0x00,
        0x00,
        0x03,
        ElementId::PacaOrder.value(),
        0x02,
        0x00,
        0x09,
    ];
    assert_eq!(
        PacaUpdateMessage::decode(&encoded).unwrap_err(),
        cdma_abis::Error::InvalidValue {
            context: "PACA Order",
            reason: "reserved bits must be zero",
        }
    );
}
