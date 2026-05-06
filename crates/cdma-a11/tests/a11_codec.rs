use cdma_a11::{
    AuthenticationExtension, AuthenticationExtensionType, CapabilitiesInfo,
    CapabilitiesInfoAcknowledge, Error, Extension, Message, Nvse, PcfEnabledFeatureNvse,
    PdsnEnabledFeatureNvse, RawExtension, RegistrationAcknowledge, RegistrationReply,
    RegistrationRequest, RegistrationUpdate, SessionParameterNvse, SessionSpecificExtension,
    SessionUpdate, SessionUpdateAcknowledge, UnknownNvse, UnverifiedDecodeReason,
    decode_unverified, decode_verified, encode,
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

fn mobile_home_authentication_extension() -> AuthenticationExtension {
    AuthenticationExtension {
        extension_type: AuthenticationExtensionType::MobileHome,
        security_parameter_index: 0x1122_3344,
        authenticator: vec![0xaa; 16],
    }
}

fn update_authentication_extension() -> AuthenticationExtension {
    AuthenticationExtension {
        extension_type: AuthenticationExtensionType::RegistrationUpdate,
        security_parameter_index: 0x0102_0304,
        authenticator: vec![0xbb; 16],
    }
}

fn sample_registration_request() -> Message {
    Message::RegistrationRequest(RegistrationRequest {
        flags: 0x0a,
        lifetime: 30,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x0102_0304_0506_0708,
        session: session(),
        extensions: vec![Extension::Authentication(
            mobile_home_authentication_extension(),
        )],
    })
}

#[test]
fn decode_verified_accepts_verifier_decision() {
    let message = sample_registration_request();
    let encoded = encode(&message).unwrap();
    let verifier = |wire: &[u8], decoded: &Message, auth: &AuthenticationExtension| {
        assert_eq!(wire, encoded.as_slice());
        assert_eq!(decoded, &message);
        assert_eq!(auth.security_parameter_index, 0x1122_3344);
        assert_eq!(auth.authenticator, vec![0xaa; 16]);
        Ok(())
    };

    let verified = decode_verified(&encoded, &verifier).unwrap();
    assert_eq!(verified.message(), &message);
    assert_eq!(verified.into_message(), message);
}

#[test]
fn decode_verified_rejects_verifier_failure() {
    let message = sample_registration_request();
    let encoded = encode(&message).unwrap();
    let verifier = |_wire: &[u8], _decoded: &Message, _auth: &AuthenticationExtension| {
        Err(Error::AuthenticationRejected {
            context: "test",
            reason: "bad authenticator",
        })
    };

    assert_eq!(
        decode_verified(&encoded, &verifier).unwrap_err(),
        Error::AuthenticationRejected {
            context: "test",
            reason: "bad authenticator",
        }
    );
}

#[test]
fn registration_request_roundtrip() {
    let message = Message::RegistrationRequest(RegistrationRequest {
        flags: 0x0a,
        lifetime: 30,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x0102_0304_0506_0708,
        session: session(),
        extensions: vec![
            Extension::Raw(RawExtension {
                extension_type: 0x90,
                value: vec![1, 2, 3],
            }),
            Extension::Authentication(mobile_home_authentication_extension()),
        ],
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x01);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn registration_reply_roundtrip_with_nvse() {
    let message = Message::RegistrationReply(RegistrationReply {
        code: 0,
        lifetime: 30,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        identification: 0x1112_1314_1516_1718,
        session: session(),
        extensions: vec![
            Extension::Nvse(Nvse::PdsnEnabledFeature(
                PdsnEnabledFeatureNvse::PacketBoundaryEnabled,
            )),
            Extension::Authentication(mobile_home_authentication_extension()),
        ],
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x03);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn registration_update_roundtrip_with_pdsn_code() {
    let message = Message::RegistrationUpdate(RegistrationUpdate {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        identification: 0x2122_2324_2526_2728,
        session: session(),
        nvses: vec![Nvse::PdsnCode(0xcb)],
        authentication_extension: update_authentication_extension(),
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x14);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn registration_acknowledge_roundtrip() {
    let message = Message::RegistrationAcknowledge(RegistrationAcknowledge {
        reserved: [0, 0],
        status: 0x00,
        home_address: [0, 0, 0, 0],
        care_of_address: [172, 16, 0, 2],
        identification: 0x3132_3334_3536_3738,
        session: session(),
        authentication_extension: update_authentication_extension(),
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x15);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn session_update_roundtrip() {
    let message = Message::SessionUpdate(SessionUpdate {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        identification: 0x4142_4344_4546_4748,
        session: session(),
        nvses: vec![
            Nvse::AnchorPPAddress([198, 51, 100, 1]),
            Nvse::SessionParameter(SessionParameterNvse::RnPdit(30)),
            Nvse::SessionParameter(SessionParameterNvse::AlwaysOn),
        ],
        authentication_extension: update_authentication_extension(),
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x16);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn session_update_acknowledge_roundtrip() {
    let message = Message::SessionUpdateAcknowledge(SessionUpdateAcknowledge {
        reserved: [0, 0],
        status: 0xc9,
        home_address: [0, 0, 0, 0],
        care_of_address: [192, 0, 2, 2],
        identification: 0x5152_5354_5556_5758,
        session: session(),
        authentication_extension: update_authentication_extension(),
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x17);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn capabilities_info_roundtrip() {
    let message = Message::CapabilitiesInfo(CapabilitiesInfo {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x6162_6364_6566_6768,
        nvses: vec![
            Nvse::PcfEnabledFeature(PcfEnabledFeatureNvse::ShortDataIndicationSupported),
            Nvse::PcfEnabledFeature(PcfEnabledFeatureNvse::GreSegmentationEnabled),
        ],
        authentication_extension: update_authentication_extension(),
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x18);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn capabilities_info_ack_roundtrip() {
    let message = Message::CapabilitiesInfoAcknowledge(CapabilitiesInfoAcknowledge {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        care_of_address: [192, 0, 2, 2],
        identification: 0x7172_7374_7576_7778,
        nvses: vec![
            Nvse::PdsnEnabledFeature(PdsnEnabledFeatureNvse::FlowControlEnabled),
            Nvse::PdsnEnabledFeature(PdsnEnabledFeatureNvse::PacketBoundaryEnabled),
        ],
        authentication_extension: update_authentication_extension(),
    });

    let encoded = encode(&message).unwrap();
    assert_eq!(encoded[0], 0x19);
    assert_eq!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture).unwrap(),
        message
    );
}

#[test]
fn unknown_nvse_roundtrip_is_preserved() {
    let message = Message::CapabilitiesInfo(CapabilitiesInfo {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x8182_8384_8586_8788,
        nvses: vec![
            Nvse::PcfEnabledFeature(PcfEnabledFeatureNvse::GreSegmentationEnabled),
            Nvse::Unknown(UnknownNvse {
                vendor_id: 0x0000_159f,
                application_type: 0xee,
                application_subtype: 0x01,
                application_data: vec![0xaa, 0xbb],
            }),
        ],
        authentication_extension: update_authentication_extension(),
    });

    assert_eq!(
        decode_unverified(
            &encode(&message).unwrap(),
            UnverifiedDecodeReason::TestFixture
        )
        .unwrap(),
        message
    );
}

#[test]
fn rejects_registration_request_with_invalid_protocol_type() {
    let invalid = Message::RegistrationRequest(RegistrationRequest {
        flags: 0x0a,
        lifetime: 30,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x0102_0304_0506_0708,
        session: SessionSpecificExtension {
            protocol_type: 0x880b,
            ..session()
        },
        extensions: vec![Extension::Authentication(
            mobile_home_authentication_extension(),
        )],
    });

    assert!(matches!(
        encode(&invalid).unwrap_err(),
        Error::InvalidValue {
            context: "session.protocol_type",
            ..
        }
    ));
}

#[test]
fn rejects_capabilities_info_without_feature_nvse() {
    let invalid = Message::CapabilitiesInfo(CapabilitiesInfo {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        care_of_address: [192, 0, 2, 2],
        identification: 0x9192_9394_9596_9798,
        nvses: vec![Nvse::PdsnCode(0xcb)],
        authentication_extension: update_authentication_extension(),
    });

    assert!(matches!(
        encode(&invalid).unwrap_err(),
        Error::InvalidValue {
            context: "capabilities info.nvses",
            ..
        }
    ));
}

#[test]
fn rejects_duplicate_nvse_application_keys() {
    let invalid = Message::CapabilitiesInfoAcknowledge(CapabilitiesInfoAcknowledge {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        care_of_address: [192, 0, 2, 2],
        identification: 0xa1a2_a3a4_a5a6_a7a8,
        nvses: vec![
            Nvse::PdsnEnabledFeature(PdsnEnabledFeatureNvse::FlowControlEnabled),
            Nvse::PdsnEnabledFeature(PdsnEnabledFeatureNvse::FlowControlEnabled),
        ],
        authentication_extension: update_authentication_extension(),
    });

    assert_eq!(
        encode(&invalid).unwrap_err(),
        Error::DuplicateExtension {
            extension_type: 0x86
        }
    );
}

#[test]
fn rejects_session_update_with_wrong_authentication_extension_type() {
    let invalid = Message::SessionUpdate(SessionUpdate {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        identification: 0xb1b2_b3b4_b5b6_b7b8,
        session: session(),
        nvses: vec![Nvse::SessionParameter(SessionParameterNvse::AlwaysOn)],
        authentication_extension: mobile_home_authentication_extension(),
    });

    assert!(matches!(
        encode(&invalid).unwrap_err(),
        Error::InvalidValue {
            context: "session update.authentication_extension",
            ..
        }
    ));
}

#[test]
fn rejects_truncated_extension_sequence() {
    let message = Message::SessionUpdate(SessionUpdate {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [10, 0, 0, 1],
        identification: 0xc1c2_c3c4_c5c6_c7c8,
        session: session(),
        nvses: vec![Nvse::SessionParameter(SessionParameterNvse::RnPdit(10))],
        authentication_extension: update_authentication_extension(),
    });
    let mut encoded = encode(&message).unwrap();
    encoded.pop();
    assert!(matches!(
        decode_unverified(&encoded, UnverifiedDecodeReason::TestFixture),
        Err(Error::Truncated { .. })
    ));
}
