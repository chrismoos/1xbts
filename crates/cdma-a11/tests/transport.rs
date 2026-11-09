use cdma_a11::{
    A11SecurityAssociation, A11SecurityConfig, AuthenticationExtension,
    AuthenticationExtensionType, CapabilitiesInfo, Message, Nvse, PcfEnabledFeatureNvse,
    RegistrationRequest, SessionSpecificExtension, UdpFrame, UnverifiedDecodeReason,
    decode_unverified, encode,
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

    assert_eq!(encoded, encode(&frame.message).unwrap());
    assert_eq!(encoded[0], 0x01, "datagram starts with the RRQ type octet");
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
fn rejects_truncated_message() {
    let frame = UdpFrame::new(request_message());
    let mut encoded = frame.encode().unwrap();
    encoded.pop();
    assert!(UdpFrame::decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).is_err());
}

#[test]
fn rejects_unknown_message_type() {
    assert_eq!(
        UdpFrame::decode_unverified(&[0xff, 0x00], UnverifiedDecodeReason::TestFixture)
            .unwrap_err(),
        cdma_a11::Error::UnknownMessageType(0xff)
    );
}

fn test_security() -> A11SecurityAssociation {
    A11SecurityAssociation::from_config(&A11SecurityConfig {
        spi: 0x1234,
        shared_secret_hex: "737065632d6131312d736563726574".to_string(),
    })
    .unwrap()
}

#[test]
fn signed_datagram_verifies_authenticator() {
    let security = test_security();
    let mut message = request_message();
    security.sign_message(&mut message).unwrap();

    let encoded = UdpFrame::new(message.clone()).encode().unwrap();
    let verified = UdpFrame::decode_verified(&encoded, &security).unwrap();
    assert_eq!(verified.message.message(), &message);
}

#[tokio::test]
async fn endpoint_sends_bare_registration_datagrams() {
    let endpoint = cdma_a11::UdpEndpoint::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    endpoint
        .send_message(peer.local_addr().unwrap(), request_message())
        .await
        .unwrap();
    let mut buf = [0u8; 2048];
    let (len, _) = peer.recv_from(&mut buf).await.unwrap();
    assert_eq!(&buf[..len], encode(&request_message()).unwrap().as_slice());
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
