use cdma_ios::{
    Cause, CellId, ClearRequestMessage, CompleteLayer3InformationMessage, ConnectMessage,
    Layer3Information, Message, MessageType, a1_frame_message_type, decode, encode,
};

#[test]
fn a1_message_is_the_bare_bsmap_frame() {
    let msg = ClearRequestMessage {
        cause: Cause(0x09),
        cause_layer3: None,
    };
    let payload = msg.encode().unwrap();
    let message = Message::new(MessageType::ClearRequest, payload.clone());
    let encoded = encode(&message);
    assert_eq!(encoded, payload);
    // BSMAP discrimination, length indicator, Clear Request message type.
    assert_eq!(encoded[0], 0x00);
    assert_eq!(encoded[2], 0x22);
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.message_type, MessageType::ClearRequest);
    assert_eq!(decoded.payload, message.payload);
}

#[test]
fn dtap_frame_type_comes_from_protocol_and_message_type() {
    let connect = ConnectMessage.encode().unwrap();
    assert_eq!(connect[0], 0x01);
    assert_eq!(
        a1_frame_message_type(&connect).unwrap(),
        MessageType::Connect
    );
}

#[test]
fn complete_layer3_with_paging_response_is_a_paging_response() {
    let mut l3 = vec![0x03, 0x00, 0x27];
    l3.extend_from_slice(&[0x12, 0x04, 0x01, 0x02, 0x03, 0x04]);
    let cli3 = CompleteLayer3InformationMessage {
        cell_identifier: CellId { cell: 1, sector: 0 },
        layer3_information: Layer3Information(l3),
    }
    .encode()
    .unwrap();
    assert_eq!(cli3[2], 0x57);
    assert_eq!(
        a1_frame_message_type(&cli3).unwrap(),
        MessageType::PagingResponse
    );

    let location = CompleteLayer3InformationMessage {
        cell_identifier: CellId { cell: 1, sector: 0 },
        layer3_information: Layer3Information(vec![0x05, 0x00, 0x08]),
    }
    .encode()
    .unwrap();
    assert_eq!(
        a1_frame_message_type(&location).unwrap(),
        MessageType::CompleteLayer3Information
    );
}

#[test]
fn unknown_frames_are_rejected() {
    assert!(a1_frame_message_type(&[0x00, 0x01, 0xee]).is_err());
    assert!(a1_frame_message_type(&[0x05]).is_err());
    assert!(decode(&[]).is_err());
}
