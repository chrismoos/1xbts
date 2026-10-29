use cdma_ios::femto_interface_msg::{
    CONTENT_TYPE_FEMTO, CONTENT_TYPE_SDP, CONTENT_TYPE_SMS, Discriminator, FemtoError, FemtoIe,
    FemtoInterfaceMsg, MimePart, Multipart,
};
use cdma_ios::typed::{
    AssignmentCompleteMessage, ChannelNumber, CmServiceType, MobileIdentity, ServiceOption,
};

/// The reference X.S0059-200 Table 2 `Location Updating Request` body, with the
/// CRLF line endings the grammar requires.
fn reference_body() -> Vec<u8> {
    let lines = [
        "08 (=\"0 Location Updating Request\")",
        "0D=DQaWMRWJCPA=",
        "12=EgzJAOYAAAAAAQMBAAY=",
        "1F=HwE=",
        "0D=DQUFKa4zmQ==",
        "35=NQE=",
        "42=QgQBATjd",
        "28=KP0=",
        "40=QAA=",
        "41=QQUBAAAA/Q==",
        "0D=DQjxAAAAAAAA8A==",
    ];
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push_str("\r\n");
    }
    body.into_bytes()
}

#[test]
fn reference_location_updating_request_round_trips_byte_identical() {
    let body = reference_body();
    let msg = FemtoInterfaceMsg::parse(&body).expect("reference body parses");

    assert_eq!(msg.message_type, 0x08);
    assert_eq!(msg.discriminator, Some(Discriminator::Dtap));
    assert_eq!(
        msg.message_name.as_deref(),
        Some("Location Updating Request")
    );
    assert_eq!(msg.ies.len(), 10);

    let reencoded = msg.encode();
    assert_eq!(
        reencoded,
        body,
        "re-encoded body must be byte-identical:\n{}\nvs\n{}",
        String::from_utf8_lossy(&reencoded),
        String::from_utf8_lossy(&body),
    );
}

#[test]
fn reference_decodes_ie_values_from_table_3() {
    let msg = FemtoInterfaceMsg::parse(&reference_body()).expect("reference body parses");

    match msg.mobile_identity().expect("mobile identity decodes") {
        MobileIdentity::Imsi(imsi) => assert_eq!(imsi, "9135198800"),
        other => panic!("expected IMSI, got {other:?}"),
    }

    // Auth Confirmation Parameter (IEI 0x28): RANDC = 0xFD.
    let randc = msg.first_ie(0x28).expect("auth confirmation present");
    assert_eq!(randc.content, vec![0x28, 0xfd]);

    let identities: Vec<_> = msg.ies.iter().filter(|ie| ie.iei == 0x0d).collect();
    assert_eq!(identities.len(), 3);
    match MobileIdentity::decode(&identities[1].content[2..]).expect("ESN decodes") {
        MobileIdentity::Esn(esn) => assert_eq!(esn, 0x29AE3399),
        other => panic!("expected ESN, got {other:?}"),
    }
    assert!(matches!(
        MobileIdentity::decode(&identities[2].content[2..]).expect("MEID decodes"),
        MobileIdentity::Meid(_)
    ));
}

#[test]
fn reference_without_meid_maps_through_typed_a1_message() {
    // The typed LocationUpdatingRequestMessage models one IMSI and one ESN, not
    // the trailing MEID identity.
    let full = FemtoInterfaceMsg::parse(&reference_body()).expect("reference body parses");
    let mut trimmed = full.clone();
    trimmed.ies.pop();
    let trimmed_body = trimmed.encode();

    let typed = trimmed
        .to_location_updating_request()
        .expect("trimmed body maps to typed A1 message");
    match &typed.mobile_identity_imsi {
        MobileIdentity::Imsi(imsi) => assert_eq!(imsi, "9135198800"),
        other => panic!("expected IMSI, got {other:?}"),
    }
    assert_eq!(
        typed
            .authentication_confirmation_parameter
            .expect("RANDC present")
            .0,
        0xfd
    );
    assert!(matches!(
        typed.mobile_identity_esn,
        Some(MobileIdentity::Esn(0x29AE3399))
    ));

    let rebuilt = FemtoInterfaceMsg::from_location_updating_request(&typed)
        .expect("typed message re-encodes to femto");
    assert_eq!(rebuilt.encode(), trimmed_body);
}

#[test]
fn cm_service_request_maps_through_typed_a1_message() {
    let classmark = vec![
        0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
    ];
    let imsi = MobileIdentity::Imsi("12345678901".to_string());
    let message = FemtoInterfaceMsg {
        message_type: 0x24,
        discriminator: Some(Discriminator::Dtap),
        message_name: Some("CM Service Request".to_string()),
        ies: vec![
            FemtoIe {
                iei: 0x90,
                content: vec![0x91],
            },
            FemtoIe::tlv(0x12, &classmark).expect("classmark IE"),
            FemtoIe::tlv(0x0d, &imsi.encode().expect("IMSI")).expect("mobile identity IE"),
        ],
    };

    let typed = message
        .to_cm_service_request()
        .expect("CM Service Request maps to typed A1");
    assert_eq!(
        typed.cm_service_type,
        CmServiceType::MobileOriginatingCallEstablishment
    );
    assert_eq!(typed.classmark_information_type_2.0, classmark);
    assert_eq!(typed.mobile_identity_imsi, imsi);
}

#[test]
fn assignment_complete_maps_through_full_bsmap_framing() {
    let typed = AssignmentCompleteMessage {
        channel_number: ChannelNumber(50),
        encryption_information: None,
        service_option: Some(ServiceOption::EVRC_A),
        a2p_bearer_session_params: None,
        a2p_bearer_format_params: None,
    };

    let femto = FemtoInterfaceMsg::from_assignment_complete(&typed)
        .expect("Assignment Complete maps to femto");
    let wire = femto.to_bsmap_wire().expect("full BSMAP wire");

    assert_eq!(wire[0], 0x00);
    assert_eq!(wire[1] as usize, wire.len() - 2);
    assert_eq!(wire[2], 0x02);
    assert_eq!(
        femto
            .to_assignment_complete()
            .expect("femto maps to Assignment Complete"),
        typed
    );
}

#[test]
fn assignment_complete_accepts_legacy_body_without_discriminator() {
    let femto = FemtoInterfaceMsg::parse(b"02\r\n23=IwAA\r\n03=AwAD\r\n")
        .expect("legacy Assignment Complete parses");

    let complete = femto
        .to_assignment_complete()
        .expect("known Assignment Complete implies BSMAP");

    assert_eq!(complete.channel_number, ChannelNumber(0));
    assert_eq!(complete.service_option, Some(ServiceOption::EVRC_A));
}

#[test]
fn paging_response_maps_through_dtap_framing() {
    let classmark = vec![
        0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
    ];
    let imsi = MobileIdentity::Imsi("5551239876".to_string());
    let esn = MobileIdentity::Esn(0x8078d840);
    let meid = MobileIdentity::Meid([0x09, 0xfd, 0x3e, 0x72, 0x00, 0x00, 0x0a]);
    let femto = FemtoInterfaceMsg {
        message_type: cdma_ios::femto_interface_msg::MSG_TYPE_PAGING_RESPONSE,
        discriminator: Some(Discriminator::Dtap),
        message_name: Some("Paging Response".to_string()),
        ies: vec![
            FemtoIe::tlv(0x12, &classmark).expect("classmark IE"),
            FemtoIe::tlv(0x0d, &imsi.encode().expect("IMSI")).expect("IMSI IE"),
            FemtoIe::tlv(0x0d, &esn.encode().expect("ESN")).expect("ESN IE"),
            FemtoIe::tlv(0x0d, &meid.encode().expect("MEID")).expect("MEID IE"),
            FemtoIe::tv(0x03, &ServiceOption::EVRC_A.encode()),
        ],
    };

    let paging = femto
        .to_paging_response()
        .expect("femto maps to Paging Response");

    assert_eq!(paging.classmark_information_type_2.0, classmark);
    assert_eq!(paging.mobile_identity_imsi, imsi);
    assert_eq!(paging.mobile_identity_esn, Some(esn));
    assert_eq!(paging.mobile_identity_meid, Some(meid));
    assert_eq!(paging.service_option, Some(ServiceOption::EVRC_A));
}

#[test]
fn rejects_bad_base64() {
    let body = b"08 (=\"0 Location Updating Request\")\r\n0D=DQaWMRWJCP!=\r\n";
    assert!(matches!(
        FemtoInterfaceMsg::parse(body),
        Err(FemtoError::BadBase64 { .. })
    ));
}

#[test]
fn rejects_missing_mandatory_mobile_identity() {
    let body = b"08\r\n35=NQE=\r\n";
    assert!(matches!(
        FemtoInterfaceMsg::parse(body),
        Err(FemtoError::MissingMandatoryIe {
            message_type: 0x08,
            iei: 0x0d,
        })
    ));
}

#[test]
fn rejects_conflicting_code_point_without_discriminator() {
    // 0x53 is DTAP ADDS Deliver vs BSMAP Privacy Mode Command: ambiguous with
    // no discriminator.
    let body = b"53\r\n";
    assert!(matches!(
        FemtoInterfaceMsg::parse(body),
        Err(FemtoError::MissingDiscriminatorOnConflict { message_type: 0x53 })
    ));

    let ok = b"53 (=\"1 Privacy Mode Command\")\r\n";
    let msg = FemtoInterfaceMsg::parse(ok).expect("disambiguated body parses");
    assert_eq!(msg.discriminator, Some(Discriminator::Bsmap));
}

#[test]
fn rejects_bad_discriminator_char() {
    let body = b"53 (=\"2 bogus\")\r\n";
    assert!(matches!(
        FemtoInterfaceMsg::parse(body),
        Err(FemtoError::BadDiscriminator { value: '2' })
    ));
}

#[test]
fn rejects_bare_lf_line_endings() {
    let body = b"08\n0D=DQaWMRWJCPA=\n";
    assert!(matches!(
        FemtoInterfaceMsg::parse(body),
        Err(FemtoError::NotCrlf)
    ));
}

#[test]
fn multipart_build_and_parse_round_trip() {
    let femto_body = reference_body();
    let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\n".to_vec();
    let sms = vec![0x00u8, 0x03, 0xff, 0x10, 0x80]; // arbitrary binary payload

    let multipart = Multipart::new(
        "femtoboundary42",
        vec![
            MimePart::text(CONTENT_TYPE_FEMTO, femto_body.clone()),
            MimePart::text(CONTENT_TYPE_SDP, sdp.clone()),
            MimePart::binary(CONTENT_TYPE_SMS, sms.clone()),
        ],
    );

    let encoded = multipart.encode();
    let parsed = Multipart::parse("femtoboundary42", &encoded).expect("multipart parses");
    assert_eq!(parsed.parts.len(), 3);

    let femto_part = parsed
        .part_by_content_type(CONTENT_TYPE_FEMTO)
        .expect("femto part present");
    assert_eq!(femto_part.body, femto_body);
    FemtoInterfaceMsg::parse(&femto_part.body).expect("embedded femto body parses");

    let sdp_part = parsed
        .part_by_content_type(CONTENT_TYPE_SDP)
        .expect("sdp part present");
    assert_eq!(sdp_part.body, sdp);

    let sms_part = parsed
        .part_by_content_type(CONTENT_TYPE_SMS)
        .expect("sms part present");
    assert_eq!(sms_part.body, sms);
    assert_eq!(sms_part.transfer_encoding.as_deref(), Some("binary"));

    assert_eq!(parsed, multipart);
}

#[test]
fn femto_ie_constructors_frame_type3_and_type4() {
    let type4 = FemtoIe::tlv(0x0d, &[0x05, 0x29, 0xae, 0x33, 0x99]).expect("tlv");
    assert_eq!(
        type4.content,
        vec![0x0d, 0x05, 0x05, 0x29, 0xae, 0x33, 0x99]
    );
    let type3 = FemtoIe::tv(0x28, &[0xfd]);
    assert_eq!(type3.content, vec![0x28, 0xfd]);
}

#[test]
fn adds_transfer_fx2_mapping_reproduces_the_a1_wire_bytes() {
    let transfer = cdma_ios::AddsTransferMessage {
        mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
        adds_user_part: cdma_ios::AddsUserPart {
            burst_type: 0x03,
            data: vec![0x00, 0x01, 0x02, 0x03],
        },
        mobile_identity_esn: Some(MobileIdentity::Esn(0x8052_1F3C)),
        mobile_identity_meid: Some(MobileIdentity::Meid([
            0xa1, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44,
        ])),
        cell_identifier: Some(cdma_ios::CellId {
            cell: 0x0193,
            sector: 0,
        }),
        tag: Some(cdma_ios::Tag(0x0000_0007)),
        service_option: Some(ServiceOption(3)),
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        authentication_event: None,
        authentication_data: None,
    };

    let femto = FemtoInterfaceMsg::from_adds_transfer(&transfer).expect("Fx2 mapping");
    assert_eq!(femto.discriminator, Some(Discriminator::Bsmap));
    assert_eq!(femto.to_layer3_sdu(), transfer.encode().expect("A1 wire"));
}

#[test]
fn adds_page_ack_fx2_mapping_reproduces_the_a1_wire_bytes() {
    let ack = cdma_ios::AddsPageAckMessage {
        mobile_identity: MobileIdentity::Imsi("310005551239999".to_string()),
        tag: Some(cdma_ios::Tag(0x0000_0009)),
        mobile_identity_esn: Some(MobileIdentity::Esn(0x8052_1F3C)),
        cause: None,
    };

    let femto = FemtoInterfaceMsg::from_adds_page_ack(&ack).expect("Fx2 mapping");
    assert_eq!(femto.discriminator, Some(Discriminator::Bsmap));
    assert_eq!(femto.to_layer3_sdu(), ack.encode().expect("A1 wire"));
}

#[test]
fn bsmap_frames_split_into_their_message_type_and_sdu() {
    let clear = cdma_ios::ClearRequestMessage {
        cause: cdma_ios::Cause(0x09),
        cause_layer3: None,
    };
    let payload = clear.encode().expect("A1 wire");

    let frame = cdma_ios::split_a1_frame(&payload).expect("BSMAP frame");
    assert!(frame.bsmap);
    // A.S0001 Table 3.1.1-1: Clear Request is BSMAP 0x22.
    assert_eq!(frame.message_type, 0x22);
    // A BSMAP SDU keeps its discrimination and length octets, so it is the
    // encoded payload unchanged.
    assert_eq!(frame.sdu, payload);
}

#[test]
fn dtap_frames_drop_the_a1_link_header_from_the_sdu() {
    let payload = cdma_ios::ConnectMessage.encode().expect("A1 wire");

    let frame = cdma_ios::split_a1_frame(&payload).expect("DTAP frame");
    assert!(!frame.bsmap);
    // A.S0001 Table 3.1.2-1: Connect is DTAP 0x07.
    assert_eq!(frame.message_type, 0x07);
    // [discrimination][DLCI][LI] is A1 link framing. The SDU starts at the
    // protocol discriminator, matching `to_layer3_sdu()` for an Fx2 body.
    assert_eq!(frame.sdu, payload[3..]);
    assert_eq!(frame.sdu[2], 0x07);
}

#[test]
fn colliding_code_points_are_named_only_with_a_discriminator() {
    use cdma_ios::femto_interface_msg::message_type_name;

    // 0x03 is BSMAP Assignment Failure and DTAP Progress.
    assert_eq!(
        message_type_name(0x03, Some(Discriminator::Bsmap)),
        Some("Assignment Failure")
    );
    assert_eq!(
        message_type_name(0x03, Some(Discriminator::Dtap)),
        Some("Progress")
    );
    assert_eq!(message_type_name(0x03, None), None);

    // 0x04 is BSMAP User Zone Update and DTAP Location Updating Reject.
    assert_eq!(
        message_type_name(0x04, Some(Discriminator::Bsmap)),
        Some("User Zone Update")
    );
    assert_eq!(
        message_type_name(0x04, Some(Discriminator::Dtap)),
        Some("Location Updating Reject")
    );
    assert_eq!(message_type_name(0x04, None), None);

    assert_eq!(
        message_type_name(0x20, Some(Discriminator::Bsmap)),
        Some("Clear Command")
    );
    assert_eq!(
        message_type_name(0x21, Some(Discriminator::Bsmap)),
        Some("Clear Complete")
    );
    assert_eq!(
        message_type_name(0x22, Some(Discriminator::Bsmap)),
        Some("Clear Request")
    );
    assert_eq!(
        message_type_name(0x57, Some(Discriminator::Bsmap)),
        Some("Complete Layer 3 Information")
    );
    assert_eq!(
        message_type_name(0x26, Some(Discriminator::Dtap)),
        Some("Alert With Information")
    );
}
