use cdma_a9::{
    A8TrafficId, CallConnectionReference, CauseValue, ConRef, ConnectA8Message, CorrelationId,
    Error, HEADER_LEN, Message, MessageType, ShortDataDeliveryMessage, TransportMetadata,
    UdpSignalingDatagram, decode, encode,
};

fn connect_message() -> ConnectA8Message {
    ConnectA8Message {
        call_connection_reference: Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708)),
        correlation_id: Some(CorrelationId([9, 10, 11, 12])),
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        con_ref: ConRef(0x01),
        a8_traffic_id: A8TrafficId::gre_ppp(0x01020304, [192, 0, 2, 10]),
        cause: CauseValue(0x13),
        pdsn_ip_address: None,
    }
}

#[test]
fn udp_transport_golden_roundtrip() {
    let message = connect_message();
    let metadata = TransportMetadata {
        flags: 0x80,
        session_id: 0x01020304,
        sequence_no: 0x05060708,
    };
    let datagram = UdpSignalingDatagram::new(metadata, message.encode().unwrap()).unwrap();

    let encoded = datagram.encode().unwrap();
    assert_eq!(encoded.len(), HEADER_LEN + datagram.payload.len());
    assert_eq!(
        &encoded[..HEADER_LEN],
        &[
            1,
            0x80,
            MessageType::ConnectA8 as u8,
            0,
            0x01,
            0x02,
            0x03,
            0x04,
            0x05,
            0x06,
            0x07,
            0x08,
            0,
            datagram.payload.len() as u8,
            0,
            0,
        ]
    );

    let decoded = UdpSignalingDatagram::decode(&encoded).unwrap();
    assert_eq!(decoded, datagram);
    assert_eq!(ConnectA8Message::decode(&decoded.payload).unwrap(), message);
}

#[test]
fn udp_transport_from_message_roundtrip() {
    let typed = connect_message();
    let message = decode(&typed.encode().unwrap()).unwrap();
    let metadata = TransportMetadata {
        flags: 0,
        session_id: 42,
        sequence_no: 7,
    };

    let datagram = UdpSignalingDatagram::from_message(metadata, &message).unwrap();
    assert_eq!(datagram.message_type, MessageType::ConnectA8);
    assert_eq!(datagram.decode_message().unwrap(), message);
}

#[test]
fn udp_transport_rejects_truncated_header() {
    let error = UdpSignalingDatagram::decode(&[1, 0, MessageType::ConnectA8 as u8]).unwrap_err();
    assert_eq!(
        error,
        Error::Truncated {
            needed: HEADER_LEN,
            actual: 3,
        }
    );
}

#[test]
fn udp_transport_rejects_truncated_payload() {
    let message = connect_message().encode().unwrap();
    let mut encoded = vec![
        1,
        0,
        MessageType::ConnectA8 as u8,
        0,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
        2,
        0,
        (message.len() as u8) + 1,
        0,
        0,
    ];
    encoded.extend_from_slice(&message);

    let error = UdpSignalingDatagram::decode(&encoded).unwrap_err();
    assert_eq!(
        error,
        Error::Truncated {
            needed: HEADER_LEN + message.len() + 1,
            actual: HEADER_LEN + message.len(),
        }
    );
}

#[test]
fn udp_transport_rejects_extra_bytes_beyond_declared_payload_length() {
    let message = connect_message().encode().unwrap();
    let mut encoded = vec![
        1,
        0,
        MessageType::ConnectA8 as u8,
        0,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
        2,
        0,
        (message.len() as u8).saturating_sub(1),
        0,
        0,
    ];
    encoded.extend_from_slice(&message);

    let error = UdpSignalingDatagram::decode(&encoded).unwrap_err();
    assert_eq!(
        error,
        Error::InvalidLength {
            expected: HEADER_LEN + message.len() - 1,
            actual: HEADER_LEN + message.len(),
        }
    );
}

#[test]
fn udp_transport_rejects_unknown_header_message_type() {
    let error =
        UdpSignalingDatagram::decode(&[1, 0, 0xff, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 1, 0, 0, 0xff])
            .unwrap_err();
    assert_eq!(error, Error::UnknownMessageType(0xff));
}

#[test]
fn udp_transport_rejects_header_payload_message_type_mismatch() {
    let payload = connect_message().encode().unwrap();
    let datagram = UdpSignalingDatagram {
        metadata: TransportMetadata {
            flags: 0,
            session_id: 1,
            sequence_no: 2,
        },
        message_type: MessageType::DisconnectA8,
        payload,
    };

    let error = datagram.encode().unwrap_err();
    assert_eq!(
        error,
        Error::PayloadMessageTypeMismatch {
            header: MessageType::DisconnectA8,
            payload: MessageType::ConnectA8,
        }
    );
}

#[test]
fn udp_transport_rejects_payload_with_unknown_message_type() {
    let metadata = TransportMetadata {
        flags: 0,
        session_id: 1,
        sequence_no: 2,
    };
    let error = UdpSignalingDatagram::new(metadata, vec![0xff, 0x00]).unwrap_err();
    assert_eq!(error, Error::UnknownMessageType(0xff));
}

#[test]
fn udp_transport_rejects_invalid_wrapper_version() {
    let message = connect_message().encode().unwrap();
    let mut encoded = vec![
        2,
        0,
        MessageType::ConnectA8 as u8,
        0,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
        2,
        0,
        message.len() as u8,
        0,
        0,
    ];
    encoded.extend_from_slice(&message);

    let error = UdpSignalingDatagram::decode(&encoded).unwrap_err();
    assert_eq!(
        error,
        Error::InvalidValue {
            context: "A9 UDP signaling wrapper version",
            value: 2,
        }
    );
}

#[test]
fn udp_transport_decodes_wrapped_message() {
    let message = Message::new(
        MessageType::ConnectA8,
        decode(&connect_message().encode().unwrap())
            .unwrap()
            .elements,
    )
    .unwrap();
    let datagram = UdpSignalingDatagram::from_message(
        TransportMetadata {
            flags: 0x01,
            session_id: 9,
            sequence_no: 10,
        },
        &message,
    )
    .unwrap();

    assert_eq!(datagram.decode_message().unwrap(), message);
    assert_eq!(
        encode(&datagram.decode_message().unwrap()).unwrap(),
        datagram.payload
    );
}

#[test]
fn udp_transport_roundtrips_short_data_delivery_payloads() {
    let typed = ShortDataDeliveryMessage {
        correlation_id: Some(CorrelationId([1, 2, 3, 4])),
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        sr_id: None,
        data_count: Some(cdma_a9::DataCount(0x0008)),
        adds_user_part: cdma_a9::AddsUserPart::short_data_burst([0xde, 0xad]),
        a9_indicators: None,
    };
    let datagram = UdpSignalingDatagram::new(
        TransportMetadata {
            flags: 0x40,
            session_id: 0x11223344,
            sequence_no: 0x55667788,
        },
        typed.encode().unwrap(),
    )
    .unwrap();

    let encoded = datagram.encode().unwrap();
    let decoded = UdpSignalingDatagram::decode(&encoded).unwrap();
    assert_eq!(decoded.message_type, MessageType::ShortDataDelivery);
    assert_eq!(
        ShortDataDeliveryMessage::decode(&decoded.payload).unwrap(),
        typed
    );
}
