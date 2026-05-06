use cdma_a11::{
    AuthenticationExtension, AuthenticationExtensionType, CapabilitiesInfo, Message, Nvse,
    PcfEnabledFeatureNvse, RegistrationRequest, SessionSpecificExtension, UdpFrame,
    UnverifiedDecodeReason, decode_unverified, encode,
};

fn session() -> SessionSpecificExtension {
    SessionSpecificExtension {
        protocol_type: 0x8881,
        pcf_session_id: 0x0102_0304,
        session_id_version: 1,
        mn_session_reference_id: 1,
        mn_id_type: 0x0006,
        mn_id: vec![0x20, 0x43, 0x65, 0x87, 0x09, 0xf1],
    }
}

fn request_message() -> Message {
    Message::RegistrationRequest(RegistrationRequest {
        flags: 0x0a,
        lifetime: 30,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x0102_0304_0506_0708,
        session: session(),
        extensions: vec![cdma_a11::Extension::Authentication(
            AuthenticationExtension {
                extension_type: AuthenticationExtensionType::MobileHome,
                security_parameter_index: 0x1122_3344,
                authenticator: vec![0xaa; 16],
            },
        )],
    })
}

fn capabilities_message() -> Message {
    Message::CapabilitiesInfo(CapabilitiesInfo {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x1112_1314_1516_1718,
        nvses: vec![Nvse::PcfEnabledFeature(
            PcfEnabledFeatureNvse::GreSegmentationEnabled,
        )],
        authentication_extension: AuthenticationExtension {
            extension_type: AuthenticationExtensionType::RegistrationUpdate,
            security_parameter_index: 0x0102_0304,
            authenticator: vec![0xbb; 16],
        },
    })
}

#[test]
fn udp_frame_roundtrip_for_registration_request() {
    let frame = UdpFrame::new(request_message());
    let encoded = frame.encode().unwrap();

    let payload = encode(&frame.message).unwrap();
    assert_eq!(
        &encoded[..2],
        &(u16::try_from(payload.len()).unwrap()).to_be_bytes()
    );
    assert_eq!(
        UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        frame
    );
}

#[test]
fn udp_frame_roundtrip_for_capabilities_info() {
    let frame = UdpFrame::new(capabilities_message());
    let encoded = frame.encode().unwrap();
    assert_eq!(
        UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        frame
    );
}

#[test]
fn udp_frame_decode_verified_returns_verified_message() {
    let frame = UdpFrame::new(request_message());
    let encoded = frame.encode().unwrap();
    let verifier = |_wire: &[u8], _message: &Message, auth: &AuthenticationExtension| {
        assert_eq!(auth.security_parameter_index, 0x1122_3344);
        Ok(())
    };

    let decoded = UdpFrame::decode_verified(&encoded, &verifier).unwrap();
    assert_eq!(decoded.message.message(), &frame.message);
}

#[test]
fn payload_len_matches_embedded_message() {
    let frame = UdpFrame::new(request_message());
    let payload = encode(&frame.message).unwrap();
    assert_eq!(frame.payload_len().unwrap(), payload.len() as u16);
}

#[test]
fn rejects_truncated_header() {
    assert_eq!(
        UdpFrame::decode_unverified(&[0x00], UnverifiedDecodeReason::TestFixture).unwrap_err(),
        cdma_a11::Error::Truncated {
            needed: 2,
            actual: 1
        }
    );
}

#[test]
fn rejects_truncated_payload() {
    let frame = UdpFrame::new(request_message());
    let mut encoded = frame.encode().unwrap();
    encoded.pop();

    let expected_needed = u16::from_be_bytes([encoded[0], encoded[1]]) as usize + 2;
    assert_eq!(
        UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap_err(),
        cdma_a11::Error::Truncated {
            needed: expected_needed,
            actual: encoded.len()
        }
    );
}

#[test]
fn rejects_trailing_bytes_beyond_declared_payload() {
    let frame = UdpFrame::new(request_message());
    let mut encoded = frame.encode().unwrap();
    encoded.extend_from_slice(&[0xde, 0xad]);

    assert!(matches!(
        UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap_err(),
        cdma_a11::Error::InvalidValue {
            context: "udp_frame.length",
            ..
        }
    ));
}

#[test]
fn rejects_invalid_embedded_message_bytes() {
    let payload = vec![0xff, 0x00];
    let mut encoded = Vec::with_capacity(2 + payload.len());
    encoded.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    encoded.extend_from_slice(&payload);

    assert_eq!(
        UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap_err(),
        cdma_a11::Error::UnknownMessageType(0xff)
    );
}

#[test]
fn decoded_frame_message_remains_valid_a11_message() {
    let frame = UdpFrame::new(capabilities_message());
    let encoded = frame.encode().unwrap();
    let decoded =
        UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap();
    let message_bytes = encode(&decoded.message).unwrap();

    assert_eq!(
        decode_unverified(&message_bytes, UnverifiedDecodeReason::TestFixture).unwrap(),
        decoded.message
    );
}
