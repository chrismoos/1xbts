use cdma_ios::{Cause, ClearRequestMessage, Message, MessageType, decode, encode};

#[test]
fn a1_message_roundtrip() {
    let msg = ClearRequestMessage {
        cause: Cause(0x09),
        cause_layer3: None,
    };
    let payload = msg.encode().unwrap();
    let message = Message::new(MessageType::ClearRequest, payload);
    let encoded = encode(&message);
    // Envelope format: [type_byte=0x14][BSMAP disc=0x00][LI][spec_msg_type=0x22][IEs...]
    assert_eq!(encoded[0], MessageType::ClearRequest as u8);
    // Inner BSMAP frame: discrimination byte at offset 1
    assert_eq!(encoded[1], 0x00);
    // BSMAP Clear Request message type at offset 3
    assert_eq!(encoded[3], 0x22);
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.message_type, MessageType::ClearRequest);
    assert_eq!(decoded.payload, message.payload);
}
