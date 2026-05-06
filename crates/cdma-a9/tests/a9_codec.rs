use cdma_a9::{
    A8IpAddress, A8TrafficId, A9Indicators, AddsUserPart, AlConnectedAckMessage,
    AlConnectedMessage, AlDisconnectedAckMessage, AlDisconnectedMessage, AnchorPdsnAddress,
    AnchorPpAddress, BsServiceRequestMessage, BsServiceResponseMessage, BscId,
    CallConnectionReference, CauseValue, ConRef, ConnectA8Message, CorrelationId, DataCount,
    DisconnectA8Message, ElementId, InformationElement, Is2000ServiceConfigurationRecord, Meid,
    Message, MessageType, MobileIdentity, PdsnIpAddress, QualityOfServiceParametersTyped,
    ReleaseA8CompleteMessage, ReleaseA8Message, RnPdit, ServiceOptionValue, SetupA8Message,
    ShortDataAckMessage, ShortDataDeliveryMessage, SoftwareVersion, SrId, UpdateA8AckMessage,
    UpdateA8Message, UserZoneId, VersionInfoAckMessage, VersionInfoMessage, decode, encode,
};

#[test]
fn setup_a8_golden_roundtrip() {
    let traffic_id = A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]);
    let message = Message::new(
        MessageType::SetupA8,
        vec![
            InformationElement::new(ElementId::ConRef, [7]),
            InformationElement::new(ElementId::BscId, [0xaa, 0xbb]),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
            InformationElement::new(ElementId::ServiceOption, [0x00, 0x21]),
            InformationElement::new(ElementId::A9Indicators, [0x03]),
        ],
    )
    .unwrap();
    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x01);
    assert_eq!(decode(&encoded).unwrap(), message);
}

#[test]
fn rejects_missing_required_a8_traffic_id() {
    assert!(
        Message::new(
            MessageType::SetupA8,
            vec![
                InformationElement::new(ElementId::ConRef, [1]),
                InformationElement::new(ElementId::BscId, [1]),
                InformationElement::new(ElementId::ServiceOption, [0, 0x21]),
                InformationElement::new(ElementId::A9Indicators, [0x01]),
            ],
        )
        .is_err()
    );
}

#[test]
fn rejects_release_a8_without_required_cause() {
    let traffic_id = A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]);
    let error = Message::new(
        MessageType::ReleaseA8,
        vec![
            InformationElement::new(ElementId::ConRef, [7]),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
        ],
    )
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::MissingRequiredInformationElement(ElementId::Cause as u8)
    );
}

#[test]
fn rejects_duplicate_information_elements() {
    let traffic_id = A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]);
    let error = Message::new(
        MessageType::SetupA8,
        vec![
            InformationElement::new(ElementId::ConRef, [7]),
            InformationElement::new(ElementId::BscId, [0xaa, 0xbb]),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
            InformationElement::new(ElementId::ServiceOption, [0x00, 0x21]),
            InformationElement::new(ElementId::A9Indicators, [0x01]),
        ],
    )
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::DuplicateInformationElement(ElementId::A8TrafficId as u8)
    );
}

#[test]
fn typed_setup_a8_roundtrip() {
    let message = SetupA8Message {
        call_connection_reference: Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708)),
        correlation_id: Some(CorrelationId([9, 10, 11, 12])),
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        con_ref: ConRef(0x12),
        quality_of_service_parameters: Some(QualityOfServiceParametersTyped { packet_priority: 4 }),
        bsc_id: BscId(vec![0xaa, 0xbb]),
        a8_traffic_id: A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]),
        service_option: ServiceOptionValue(0x0021),
        a9_indicators: A9Indicators {
            packet_boundary_supported: false,
            gre_segmentation_supported: false,
            sdb_supported: false,
            ccpd_mode: false,
            data_ready: true,
            handoff: true,
        },
        user_zone_id: Some(UserZoneId(0x0102)),
    };
    let encoded = message.encode().unwrap();
    assert_eq!(SetupA8Message::decode(&encoded).unwrap(), message);
}

#[test]
fn typed_message_roundtrips_cover_full_inventory() {
    let traffic_id = A8TrafficId::gre_ppp(0x11121314, [192, 0, 2, 20]);
    let ccr = Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708));
    let corr = Some(CorrelationId([9, 10, 11, 12]));

    let connect = ConnectA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        con_ref: ConRef(0x77),
        a8_traffic_id: traffic_id.clone(),
        cause: CauseValue(0x13),
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
    };
    assert_eq!(
        ConnectA8Message::decode(&connect.encode().unwrap()).unwrap(),
        connect
    );

    let disconnect = DisconnectA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        con_ref: ConRef(0x77),
        a8_traffic_id: traffic_id.clone(),
        cause: CauseValue(0x10),
    };
    assert_eq!(
        DisconnectA8Message::decode(&disconnect.encode().unwrap()).unwrap(),
        disconnect
    );

    let release = ReleaseA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: None,
        esn: None,
        meid: None,
        con_ref: ConRef(0x77),
        a8_traffic_id: traffic_id.clone(),
        cause: CauseValue(0x14),
    };
    assert_eq!(
        ReleaseA8Message::decode(&release.encode().unwrap()).unwrap(),
        release
    );

    let release_complete = ReleaseA8CompleteMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
    };
    assert_eq!(
        ReleaseA8CompleteMessage::decode(&release_complete.encode().unwrap()).unwrap(),
        release_complete
    );

    let bs_request = BsServiceRequestMessage {
        correlation_id: corr,
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        service_option: ServiceOptionValue(0x0021),
        data_count: DataCount(0x0102),
    };
    assert_eq!(
        BsServiceRequestMessage::decode(&bs_request.encode().unwrap()).unwrap(),
        bs_request
    );

    let bs_response = BsServiceResponseMessage {
        correlation_id: corr,
        cause: Some(CauseValue(0x08)),
    };
    assert_eq!(
        BsServiceResponseMessage::decode(&bs_response.encode().unwrap()).unwrap(),
        bs_response
    );

    let bs_response_success = BsServiceResponseMessage {
        correlation_id: None,
        cause: None,
    };
    assert_eq!(
        BsServiceResponseMessage::decode(&bs_response_success.encode().unwrap()).unwrap(),
        bs_response_success
    );

    let al_connected = AlConnectedMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
        a8_traffic_id: traffic_id.clone(),
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
    };
    assert_eq!(
        AlConnectedMessage::decode(&al_connected.encode().unwrap()).unwrap(),
        al_connected
    );

    let al_connected_ack = AlConnectedAckMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
    };
    assert_eq!(
        AlConnectedAckMessage::decode(&al_connected_ack.encode().unwrap()).unwrap(),
        al_connected_ack
    );

    let al_disconnected = AlDisconnectedMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
        a8_traffic_id: traffic_id.clone(),
    };
    assert_eq!(
        AlDisconnectedMessage::decode(&al_disconnected.encode().unwrap()).unwrap(),
        al_disconnected
    );

    let al_disconnected_ack = AlDisconnectedAckMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
    };
    assert_eq!(
        AlDisconnectedAckMessage::decode(&al_disconnected_ack.encode().unwrap()).unwrap(),
        al_disconnected_ack
    );
}

#[test]
fn typed_message_golden_encodings_cover_full_inventory() {
    let traffic_id = A8TrafficId::gre_ppp(0x11121314, [192, 0, 2, 20]);
    let ccr = Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708));
    let corr = Some(CorrelationId([9, 10, 11, 12]));

    let setup = SetupA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: None,
        esn: None,
        meid: None,
        con_ref: ConRef(0x77),
        quality_of_service_parameters: None,
        bsc_id: BscId(vec![0xaa, 0xbb]),
        a8_traffic_id: traffic_id.clone(),
        service_option: ServiceOptionValue(0x0021),
        a9_indicators: A9Indicators {
            packet_boundary_supported: false,
            gre_segmentation_supported: false,
            sdb_supported: false,
            ccpd_mode: false,
            data_ready: true,
            handoff: true,
        },
        user_zone_id: None,
    };
    assert_eq!(
        setup.encode().unwrap(),
        vec![
            0x01, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12, 0x01, 0x01, 0x77,
            0x06, 0x02, 0xaa, 0xbb, 0x08, 0x0c, 0x01, 0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0x01,
            192, 0, 2, 20, 0x03, 0x02, 0x00, 0x21, 0x05, 0x01, 0x03,
        ]
    );

    let connect = ConnectA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: None,
        esn: None,
        meid: None,
        con_ref: ConRef(0x77),
        a8_traffic_id: traffic_id.clone(),
        cause: CauseValue(0x13),
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
    };
    assert_eq!(
        connect.encode().unwrap(),
        vec![
            0x02, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12, 0x01, 0x01, 0x77,
            0x08, 0x0c, 0x01, 0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0x01, 192, 0, 2, 20, 0x04, 0x01,
            0x13, 0x14, 0x04, 192, 0, 2, 10,
        ]
    );

    let disconnect = DisconnectA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: None,
        esn: None,
        meid: None,
        con_ref: ConRef(0x77),
        a8_traffic_id: traffic_id.clone(),
        cause: CauseValue(0x10),
    };
    assert_eq!(
        disconnect.encode().unwrap(),
        vec![
            0x03, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12, 0x01, 0x01, 0x77,
            0x08, 0x0c, 0x01, 0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0x01, 192, 0, 2, 20, 0x04, 0x01,
            0x10,
        ]
    );

    let release = ReleaseA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: None,
        esn: None,
        meid: None,
        con_ref: ConRef(0x77),
        a8_traffic_id: traffic_id.clone(),
        cause: CauseValue(0x14),
    };
    assert_eq!(
        release.encode().unwrap(),
        vec![
            0x04, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12, 0x01, 0x01, 0x77,
            0x08, 0x0c, 0x01, 0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0x01, 192, 0, 2, 20, 0x04, 0x01,
            0x14,
        ]
    );

    let release_complete = ReleaseA8CompleteMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
    };
    assert_eq!(
        release_complete.encode().unwrap(),
        vec![
            0x05, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12,
        ]
    );

    let bs_request = BsServiceRequestMessage {
        correlation_id: corr,
        imsi: None,
        esn: None,
        meid: None,
        service_option: ServiceOptionValue(0x0021),
        data_count: DataCount(0x0102),
    };
    assert_eq!(
        bs_request.encode().unwrap(),
        vec![
            0x06, 0x13, 0x04, 9, 10, 11, 12, 0x03, 0x02, 0x00, 0x21, 0x09, 0x02, 0x01, 0x02
        ]
    );

    let bs_response = BsServiceResponseMessage {
        correlation_id: corr,
        cause: Some(CauseValue(0x08)),
    };
    assert_eq!(
        bs_response.encode().unwrap(),
        vec![0x07, 0x13, 0x04, 9, 10, 11, 12, 0x04, 0x01, 0x08]
    );

    let al_connected = AlConnectedMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
        a8_traffic_id: traffic_id.clone(),
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
    };
    assert_eq!(
        al_connected.encode().unwrap(),
        vec![
            0x08, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12, 0x08, 0x0c, 0x01,
            0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0x01, 192, 0, 2, 20, 0x14, 0x04, 192, 0, 2, 10,
        ]
    );

    let al_connected_ack = AlConnectedAckMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
    };
    assert_eq!(
        al_connected_ack.encode().unwrap(),
        vec![
            0x09, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12,
        ]
    );

    let al_disconnected = AlDisconnectedMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
        a8_traffic_id: traffic_id,
    };
    assert_eq!(
        al_disconnected.encode().unwrap(),
        vec![
            0x0a, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12, 0x08, 0x0c, 0x01,
            0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0x01, 192, 0, 2, 20,
        ]
    );

    let al_disconnected_ack = AlDisconnectedAckMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
    };
    assert_eq!(
        al_disconnected_ack.encode().unwrap(),
        vec![
            0x0b, 0x3f, 0x08, 1, 2, 3, 4, 5, 6, 7, 8, 0x13, 0x04, 9, 10, 11, 12,
        ]
    );
}

#[test]
fn typed_setup_rejects_unexpected_information_element() {
    let traffic_id = A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]);
    let error = Message::new(
        MessageType::SetupA8,
        vec![
            InformationElement::new(ElementId::ConRef, [7]),
            InformationElement::new(ElementId::BscId, [0xaa, 0xbb]),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
            InformationElement::new(ElementId::ServiceOption, [0x00, 0x21]),
            InformationElement::new(ElementId::A9Indicators, [0x01]),
            InformationElement::new(ElementId::Cause, [0x01]),
        ],
    )
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::UnexpectedInformationElement {
            message_type: MessageType::SetupA8,
            element_id: ElementId::Cause as u8,
        }
    );
}

#[test]
fn typed_connect_rejects_unexpected_information_element() {
    let traffic_id = A8TrafficId::gre_ppp(0x11121314, [192, 0, 2, 20]);
    let error = Message::new(
        MessageType::ConnectA8,
        vec![
            InformationElement::new(ElementId::ConRef, [0x77]),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
            InformationElement::new(ElementId::Cause, [0x13]),
            InformationElement::new(ElementId::BscId, [0xaa]),
        ],
    )
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::UnexpectedInformationElement {
            message_type: MessageType::ConnectA8,
            element_id: ElementId::BscId as u8,
        }
    );
}

#[test]
fn rejects_out_of_order_information_elements() {
    let traffic_id = A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]);
    let error = Message::new(
        MessageType::SetupA8,
        vec![
            InformationElement::new(ElementId::ConRef, [7]),
            InformationElement::new(ElementId::A8TrafficId, traffic_id.encode()),
            InformationElement::new(ElementId::BscId, [0xaa, 0xbb]),
            InformationElement::new(ElementId::ServiceOption, [0x00, 0x21]),
            InformationElement::new(ElementId::A9Indicators, [0x01]),
        ],
    )
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::OutOfOrderInformationElement {
            message_type: MessageType::SetupA8,
            element_id: ElementId::BscId as u8,
        }
    );
}

#[test]
fn rejects_mobile_identity_esn_without_imsi() {
    let error = BsServiceRequestMessage {
        correlation_id: None,
        imsi: None,
        esn: Some(0x01020304),
        meid: None,
        service_option: ServiceOptionValue(0x0021),
        data_count: DataCount(0x0010),
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidValue {
            context: "MobileIdentity.sequence",
            value: 0x05,
        }
    );
}

#[test]
fn rejects_mobile_identity_esn_before_imsi_on_decode() {
    let message = vec![
        0x06, 0x0d, 0x05, 0x05, 0x01, 0x02, 0x03, 0x04, 0x03, 0x02, 0x00, 0x21, 0x09, 0x02, 0x00,
        0x10,
    ];
    let error = BsServiceRequestMessage::decode(&message).unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidValue {
            context: "MobileIdentity.sequence",
            value: 0x05,
        }
    );
}

#[test]
fn rejects_invalid_a8_traffic_id_values() {
    let invalid_protocol_stack =
        A8TrafficId::decode(&[0x02, 0x88, 0x0b, 0x01, 0x02, 0x03, 0x04, 0x01, 10, 0, 0, 1])
            .unwrap_err();
    assert_eq!(
        invalid_protocol_stack,
        cdma_a9::Error::InvalidValue {
            context: "A8TrafficId.protocol_stack",
            value: 0x02,
        }
    );

    let invalid_protocol_type =
        A8TrafficId::decode(&[0x01, 0x12, 0x34, 0x01, 0x02, 0x03, 0x04, 0x01, 10, 0, 0, 1])
            .unwrap_err();
    assert_eq!(
        invalid_protocol_type,
        cdma_a9::Error::InvalidValue {
            context: "A8TrafficId.protocol_type",
            value: 0x1234,
        }
    );

    let invalid_address_type =
        A8TrafficId::decode(&[0x01, 0x88, 0x81, 0x01, 0x02, 0x03, 0x04, 0x03, 10, 0, 0, 1])
            .unwrap_err();
    assert_eq!(
        invalid_address_type,
        cdma_a9::Error::InvalidValue {
            context: "A8TrafficId.address_type",
            value: 0x03,
        }
    );
}

#[test]
fn supports_ipv6_a8_traffic_id_roundtrip() {
    let message =
        A8TrafficId::gre_ppp_ipv6(0x01020304, [0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8]);
    let decoded = A8TrafficId::decode(&message.encode()).unwrap();
    assert_eq!(decoded, message);
    assert_eq!(
        decoded.ip_address,
        A8IpAddress::V6([0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8])
    );
}

#[test]
fn rejects_invalid_typed_payloads() {
    let qos_error = QualityOfServiceParametersTyped::decode(&[0xf1]).unwrap_err();
    assert_eq!(
        qos_error,
        cdma_a9::Error::InvalidValue {
            context: "QualityOfServiceParameters.reserved_bits",
            value: 0xf1,
        }
    );

    let indicators_error = A9Indicators::decode(&[0x04]).unwrap_err();
    assert_eq!(
        indicators_error,
        cdma_a9::Error::InvalidValue {
            context: "A9Indicators.reserved_bits",
            value: 0x04,
        }
    );

    let mobile_identity_error = MobileIdentity::decode(&[0x16, 0xf2, 0x34]).unwrap_err();
    assert_eq!(
        mobile_identity_error,
        cdma_a9::Error::InvalidValue {
            context: "MobileIdentity.imsi.filler",
            value: 0x0f,
        }
    );
}

#[test]
fn rejects_invalid_message_specific_causes() {
    let connect_error = ConnectA8Message {
        call_connection_reference: None,
        correlation_id: None,
        imsi: None,
        esn: None,
        meid: None,
        con_ref: ConRef(1),
        a8_traffic_id: A8TrafficId::gre_ppp(0x01020304, [10, 0, 0, 1]),
        cause: CauseValue(0x14),
        pdsn_ip_address: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        connect_error,
        cdma_a9::Error::InvalidValue {
            context: "ConnectA8.cause",
            value: 0x14,
        }
    );

    let bs_response_error = BsServiceResponseMessage {
        correlation_id: None,
        cause: Some(CauseValue(0x20)),
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        bs_response_error,
        cdma_a9::Error::InvalidValue {
            context: "BsServiceResponse.cause",
            value: 0x20,
        }
    );

    let version_info_error = VersionInfoMessage {
        correlation_id: None,
        cause: Some(CauseValue(0x13)),
        software_version: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        version_info_error,
        cdma_a9::Error::InvalidValue {
            context: "VersionInfo.cause",
            value: 0x13,
        }
    );

    let update_ack_error = UpdateA8AckMessage {
        call_connection_reference: None,
        correlation_id: None,
        cause: Some(CauseValue(0x1b)),
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        update_ack_error,
        cdma_a9::Error::InvalidValue {
            context: "UpdateA8Ack.cause",
            value: 0x1b,
        }
    );

    let short_data_ack_error = ShortDataAckMessage {
        correlation_id: None,
        imsi: Some("123456789012345".into()),
        esn: None,
        meid: None,
        cause: CauseValue(0x11),
    }
    .encode()
    .unwrap_err();
    assert_eq!(
        short_data_ack_error,
        cdma_a9::Error::InvalidValue {
            context: "ShortDataAck.cause",
            value: 0x11,
        }
    );
}

#[test]
fn typed_roundtrips_cover_version_update_and_short_data_families() {
    let corr = Some(CorrelationId([1, 2, 3, 4]));
    let ccr = Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708));
    let meid = Some(Meid([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]));

    let version = VersionInfoMessage {
        correlation_id: corr,
        cause: Some(CauseValue(0x07)),
        software_version: Some(SoftwareVersion {
            ios_major_revision_level: 2,
            ios_minor_revision_level: 5,
            ios_point_release_level: 9,
            manufacturer_carrier_software_information: "pcf-build-1".into(),
        }),
    };
    assert_eq!(
        VersionInfoMessage::decode(&version.encode().unwrap()).unwrap(),
        version
    );

    let version_ack = VersionInfoAckMessage {
        correlation_id: corr,
        software_version: Some(SoftwareVersion {
            ios_major_revision_level: 3,
            ios_minor_revision_level: 0,
            ios_point_release_level: 1,
            manufacturer_carrier_software_information: "bsc-build-2".into(),
        }),
    };
    assert_eq!(
        VersionInfoAckMessage::decode(&version_ack.encode().unwrap()).unwrap(),
        version_ack
    );

    let update = UpdateA8Message {
        call_connection_reference: ccr,
        correlation_id: corr,
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid,
        service_configuration_record: Some(Is2000ServiceConfigurationRecord {
            fill_bits: 3,
            content: vec![0xaa, 0xbb, 0xcc],
        }),
        service_option: Some(ServiceOptionValue(0x0021)),
        user_zone_id: Some(UserZoneId(0x0102)),
        quality_of_service_parameters: Some(QualityOfServiceParametersTyped { packet_priority: 7 }),
        cause: Some(CauseValue(0x1b)),
        rn_pdit: Some(RnPdit(4)),
        sr_id: Some(SrId(7)),
        a9_indicators: Some(A9Indicators {
            packet_boundary_supported: true,
            gre_segmentation_supported: true,
            sdb_supported: true,
            ccpd_mode: false,
            data_ready: false,
            handoff: true,
        }),
        pdsn_ip_address: Some(PdsnIpAddress([192, 0, 2, 10])),
        anchor_pdsn_address: Some(AnchorPdsnAddress([192, 0, 2, 11])),
        anchor_pp_address: Some(AnchorPpAddress([192, 0, 2, 12])),
    };
    assert_eq!(
        UpdateA8Message::decode(&update.encode().unwrap()).unwrap(),
        update
    );

    let update_ack = UpdateA8AckMessage {
        call_connection_reference: ccr,
        correlation_id: corr,
        cause: Some(CauseValue(0x36)),
    };
    assert_eq!(
        UpdateA8AckMessage::decode(&update_ack.encode().unwrap()).unwrap(),
        update_ack
    );

    let short_data_delivery = ShortDataDeliveryMessage {
        correlation_id: corr,
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid,
        sr_id: Some(SrId(5)),
        data_count: Some(DataCount(0x0020)),
        adds_user_part: AddsUserPart::short_data_burst([0xde, 0xad, 0xbe, 0xef]),
        a9_indicators: Some(A9Indicators {
            packet_boundary_supported: true,
            gre_segmentation_supported: false,
            sdb_supported: true,
            ccpd_mode: true,
            data_ready: false,
            handoff: false,
        }),
    };
    assert_eq!(
        ShortDataDeliveryMessage::decode(&short_data_delivery.encode().unwrap()).unwrap(),
        short_data_delivery
    );

    let short_data_ack = ShortDataAckMessage {
        correlation_id: corr,
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid,
        cause: CauseValue(0x17),
    };
    assert_eq!(
        ShortDataAckMessage::decode(&short_data_ack.encode().unwrap()).unwrap(),
        short_data_ack
    );
}

#[test]
fn rejects_short_data_delivery_with_non_sdb_burst_type() {
    let error = ShortDataDeliveryMessage::decode(&[
        MessageType::ShortDataDelivery as u8,
        ElementId::AddsUserPart as u8,
        0x02,
        0x01,
        0xaa,
    ])
    .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidValue {
            context: "AddsUserPart.data_burst_type",
            value: 0x01,
        }
    );
}
