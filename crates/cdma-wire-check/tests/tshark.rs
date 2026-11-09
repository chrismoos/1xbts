//! Each test encodes IOS messages with the production codecs, writes them to
//! a pcap file and requires the Wireshark dissector to decode them without
//! errors or warnings.

use std::net::Ipv4Addr;

use cdma_a8::{
    DEFAULT_MAX_GRE_PAYLOAD, GRE_PROTOCOL_TYPE_3GPP2_PACKET, GreAttribute, GrePacket, GreSegment,
    PppFrameAligner,
};
use cdma_a11::{
    A11_STANDARD_UDP_PORT, AuthenticationExtension, AuthenticationExtensionType, Extension,
    Message as A11Message, Nvse, PcfEnabledFeatureNvse, PdsnEnabledFeatureNvse, RegistrationReply,
    RegistrationRequest, SessionSpecificExtension,
};
use cdma_a21::{
    A21_UDP_PORT, A21Cause, A21EventKind, A21Message, MessageTransmissionControl,
    MobileIdentity as A21Identity,
};
use cdma_ios::sua::{
    self, A1P_SCTP_PORT, BSAP_SSN, SUA_PAYLOAD_PROTOCOL_ID, SuaAddress, SuaMessage, SuaMessageKind,
    tag,
};
use cdma_ios::voice_bearer::VoiceBearerFormat;
use cdma_ios::{
    A2pBearerFormatParams, A2pBearerSessionParams, AssignmentCompleteMessage,
    AssignmentRequestMessage, CellId, ChannelNumber, ChannelType, CircuitIdentityCode,
    ClassmarkInformationType2, ClearCommandMessage, CmServiceRequestMessage, CmServiceType,
    CompleteLayer3InformationMessage, EvrcFrameType, Layer3Information,
    LocationUpdatingAcceptMessage, LocationUpdatingRequestMessage, MobileIdentity,
    PagingRequestMessage, PagingResponseMessage, Tag,
};
use cdma_wire_check::{Capture, Packet, tshark_available};

const BS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
const PCF: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);
const PDSN: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 3);
const AN_A21: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 4);
const MSC_SCTP_PORT: u16 = 40_000;
const BS_POINT_CODE: u32 = 1;
const MSC_POINT_CODE: u32 = 2;
const ROUTING_CONTEXT: u32 = 1;
const IMSI: &str = "310009176936269";
/// Unstructured Byte Stream, the A10 GRE and A11 protocol type
/// (A.S0017-D §2.2.2.4).
const A11_PROTOCOL_TYPE_BYTE_STREAM: u16 = 0x8881;
const HDLC_FLAG: u8 = 0x7e;
/// RFC 1662 §C.2 FCS-16 initial value, reflected polynomial and final XOR.
const PPP_FCS_INIT: u16 = 0xffff;
const PPP_FCS_POLYNOMIAL: u16 = 0x8408;
const PPP_FCS_XOR: u16 = 0xffff;
const RTP_EVRC_PAYLOAD_TYPE: u8 = 97;
const RTP_PORT: u16 = 40_002;

/// BSAP preferences for the ANSI A dissectors, which default to GSM BSSAP.
const BSAP_PREFERENCES: [&str; 2] = [
    "bssap.default_protocol:bsap",
    "ansi_a_bsmap.global_variant:IOS 5.0.1",
];

fn capture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("wire-check")
}

fn cell() -> CellId {
    CellId {
        cell: 0x123,
        sector: 1,
    }
}

fn classmark() -> ClassmarkInformationType2 {
    ClassmarkInformationType2(vec![
        0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
    ])
}

fn imsi() -> MobileIdentity {
    MobileIdentity::Imsi(IMSI.into())
}

fn cm_service_request() -> Vec<u8> {
    let dtap = CmServiceRequestMessage {
        cm_service_type: CmServiceType::MobileOriginatingCallEstablishment,
        classmark_information_type_2: classmark(),
        mobile_identity_imsi: imsi(),
        called_party_bcd_number: None,
        tag: None,
        mobile_identity_esn: None,
        mobile_identity_meid: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: None,
        voice_privacy_request: false,
        radio_environment_and_resources: None,
        called_party_ascii_number: None,
        circuit_identity_code: None,
        cdma_serving_one_way_delay: None,
        authentication_event: None,
        authentication_data: None,
        paca_reorigination_indicator: false,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    CompleteLayer3InformationMessage {
        cell_identifier: cell(),
        layer3_information: Layer3Information::from_cm_service_request(&dtap).unwrap(),
    }
    .encode()
    .unwrap()
}

fn paging_response(tag: u32) -> Vec<u8> {
    PagingResponseMessage {
        classmark_information_type_2: classmark(),
        mobile_identity_imsi: imsi(),
        tag: Some(Tag(tag)),
        mobile_identity_esn: None,
        mobile_identity_meid: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: None,
        voice_privacy_request: false,
        circuit_identity_code: None,
        cdma_serving_one_way_delay: None,
        authentication_event: None,
        radio_environment_and_resources: None,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    }
    .encode_a1(cell())
    .unwrap()
}

fn location_updating_request() -> Vec<u8> {
    let request = LocationUpdatingRequestMessage {
        mobile_identity_imsi: imsi(),
        location_area_identification: None,
        classmark_information_type_2: None,
        registration_type: None,
        mobile_identity_esn: None,
        mobile_identity_meid: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        authentication_event: None,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    CompleteLayer3InformationMessage {
        cell_identifier: cell(),
        layer3_information: Layer3Information::from_location_updating_request(&request).unwrap(),
    }
    .encode()
    .unwrap()
}

fn bs_address() -> SuaAddress {
    SuaAddress {
        point_code: BS_POINT_CODE,
        ssn: BSAP_SSN,
    }
}

fn msc_address() -> SuaAddress {
    SuaAddress {
        point_code: MSC_POINT_CODE,
        ssn: BSAP_SSN,
    }
}

fn connection_request(local_ref: u32, data: Vec<u8>) -> SuaMessage {
    SuaMessage::new(SuaMessageKind::ConnectionRequest)
        .with_u32(tag::ROUTING_CONTEXT, ROUTING_CONTEXT)
        .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_2)
        .with_u32(tag::SOURCE_REFERENCE_NUMBER, local_ref)
        .with_address(tag::DESTINATION_ADDRESS, msc_address())
        .with_u32(tag::SEQUENCE_CONTROL, 0)
        .with_address(tag::SOURCE_ADDRESS, bs_address())
        .with(tag::DATA, data)
}

fn connection_ack(local_ref: u32, remote_ref: u32, data: Vec<u8>) -> SuaMessage {
    SuaMessage::new(SuaMessageKind::ConnectionAck)
        .with_u32(tag::ROUTING_CONTEXT, ROUTING_CONTEXT)
        .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_2)
        .with_u32(tag::DESTINATION_REFERENCE_NUMBER, remote_ref)
        .with_u32(tag::SOURCE_REFERENCE_NUMBER, local_ref)
        .with_u32(tag::SEQUENCE_CONTROL, 0)
        .with(tag::DATA, data)
}

fn connection_refused(remote_ref: u32, data: Vec<u8>) -> SuaMessage {
    SuaMessage::new(SuaMessageKind::ConnectionRefused)
        .with_u32(tag::ROUTING_CONTEXT, ROUTING_CONTEXT)
        .with_u32(tag::DESTINATION_REFERENCE_NUMBER, remote_ref)
        .with_cause(
            sua::CAUSE_TYPE_REFUSAL,
            sua::REFUSAL_CAUSE_END_USER_ORIGINATED,
        )
        .with(tag::DATA, data)
}

fn connectionless(data: Vec<u8>) -> SuaMessage {
    SuaMessage::new(SuaMessageKind::ConnectionlessData)
        .with_u32(tag::ROUTING_CONTEXT, ROUTING_CONTEXT)
        .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_0)
        .with_address(tag::SOURCE_ADDRESS, msc_address())
        .with_address(tag::DESTINATION_ADDRESS, bs_address())
        .with_u32(tag::SEQUENCE_CONTROL, 0)
        .with(tag::DATA, data)
}

#[test]
fn tshark_decodes_a1p_bsap_over_sua() {
    if !tshark_available() {
        return;
    }
    let bs = (BS, A1P_SCTP_PORT);
    let msc = (BS, MSC_SCTP_PORT);
    let page_tag = 77;
    let clear_command = ClearCommandMessage {
        cause: cdma_ios::Cause(0x09),
        cause_layer3: None,
    }
    .encode()
    .unwrap();
    let page = PagingRequestMessage {
        mobile_identity_imsi: imsi(),
        mobile_identity_meid: None,
        tag: Some(Tag(page_tag)),
        cell_identifier_list: None,
        slot_cycle_index: None,
        service_option: None,
        is2000_mobile_capabilities: None,
    }
    .encode()
    .unwrap();
    let accept = LocationUpdatingAcceptMessage {
        location_area_identification: None,
    }
    .encode()
    .unwrap();
    let messages = [
        (bs, msc, connection_request(1, location_updating_request())),
        (msc, bs, connection_refused(1, accept)),
        (bs, msc, connection_request(2, cm_service_request())),
        (msc, bs, connection_ack(100, 2, clear_command)),
        (msc, bs, connectionless(page)),
        (bs, msc, connection_request(3, paging_response(page_tag))),
    ];
    let packets: Vec<Packet> = messages
        .iter()
        .enumerate()
        .map(|(index, (src, dst, message))| {
            Packet::sctp_data(
                *src,
                *dst,
                index as u32 + 1,
                0,
                SUA_PAYLOAD_PROTOCOL_ID,
                &message.encode(),
            )
        })
        .collect();
    let mut capture = Capture::write(&capture_dir(), "a1p", &packets).unwrap();
    for preference in BSAP_PREFERENCES {
        capture = capture.preference(preference);
    }
    capture.assert_clean();

    let rows = capture.fields(&[
        "sua.message_class",
        "sua.message_type",
        "ansi_a_bsmap.msgtype",
        "ansi_a_bsmap.dtap_msgtype",
        "ansi_a_bsmap.imsi",
    ]);
    assert_eq!(
        rows,
        [
            // CORE, Complete Layer 3 Information, Location Updating Request.
            "8\t1\t0x57\t0x08\t310009176936269",
            // COREF, Location Updating Accept.
            "8\t3\t\t0x02\t",
            // CORE, Complete Layer 3 Information, CM Service Request.
            "8\t1\t0x57\t0x24\t310009176936269",
            // COAK, Clear Command.
            "8\t2\t0x20\t\t",
            // CLDT, Paging Request.
            "7\t1\t0x52\t\t310009176936269",
            // CORE, Complete Layer 3 Information, Paging Response.
            "8\t1\t0x57\t0x27\t310009176936269",
        ]
    );
}

const A2P_PORT: u16 = 40_004;
const EVRC_PAYLOAD_TYPE: u8 = 97;
const TELEPHONE_EVENT_PAYLOAD_TYPE: u8 = 101;

fn a2p_assignment() -> (Vec<u8>, Vec<u8>) {
    let session = Some(A2pBearerSessionParams {
        ip_address: BS,
        udp_port: A2P_PORT,
    });
    let format = Some(A2pBearerFormatParams::evrc_with_telephone_event(
        EVRC_PAYLOAD_TYPE,
        TELEPHONE_EVENT_PAYLOAD_TYPE,
    ));
    let request = AssignmentRequestMessage {
        channel_type: ChannelType {
            speech_or_data_indicator: ChannelType::SPEECH_OR_DATA_SPEECH,
            channel_rate_and_type: ChannelType::RATE_FULL_RATE_TCH_BM,
            coding: ChannelType::CODING_QCELP_13K,
        },
        circuit_identity_code: Some(CircuitIdentityCode {
            pcm_multiplexer: 0,
            timeslot: 29,
        }),
        encryption_information: None,
        service_option: Some(cdma_ios::ServiceOption::EVRC_A),
        signals: Vec::new(),
        ms_information_records: None,
        priority: None,
        paca_timestamp: None,
        quality_of_service_parameters: None,
        a2p_bearer_session_params: session,
        a2p_bearer_format_params: format.clone(),
    }
    .encode()
    .unwrap();
    let complete = AssignmentCompleteMessage {
        channel_number: ChannelNumber(283),
        encryption_information: None,
        service_option: Some(cdma_ios::ServiceOption::EVRC_A),
        a2p_bearer_session_params: session,
        a2p_bearer_format_params: format,
    }
    .encode()
    .unwrap();
    (request, complete)
}

#[test]
fn tshark_decodes_a2p_bearer_parameters_in_assignment() {
    if !tshark_available() {
        return;
    }
    let bs = (BS, A1P_SCTP_PORT);
    let msc = (BS, MSC_SCTP_PORT);
    let (request, complete) = a2p_assignment();
    let packets = [
        Packet::sctp_data(
            msc,
            bs,
            1,
            0,
            SUA_PAYLOAD_PROTOCOL_ID,
            &connectionless(request).encode(),
        ),
        Packet::sctp_data(
            bs,
            msc,
            2,
            0,
            SUA_PAYLOAD_PROTOCOL_ID,
            &connectionless(complete).encode(),
        ),
    ];
    let mut capture = Capture::write(&capture_dir(), "a2p-assignment", &packets).unwrap();
    for preference in BSAP_PREFERENCES {
        capture = capture.preference(preference);
    }
    capture.assert_clean();
    assert_eq!(capture.fields(&["ansi_a_bsmap.msgtype"]), ["0x01", "0x02"]);
}

fn session() -> SessionSpecificExtension {
    SessionSpecificExtension {
        protocol_type: A11_PROTOCOL_TYPE_BYTE_STREAM,
        pcf_session_id: 0x0102_0304,
        session_id_version: 1,
        mn_session_reference_id: 1,
        mn_id_type: 0x0006,
        // IMSI 310009176936269. The first octet holds digit 1 and the odd/even
        // indicator. The identity type is in `mn_id_type` (A.S0017-D §2.2.2.4).
        mn_id: vec![0x31, 0x01, 0x00, 0x19, 0x67, 0x39, 0x26, 0x96],
    }
}

fn mobile_home_authentication() -> Extension {
    Extension::Authentication(AuthenticationExtension {
        extension_type: AuthenticationExtensionType::MobileHome,
        security_parameter_index: 0x0000_0100,
        authenticator: vec![0xaa; 16],
    })
}

#[test]
fn tshark_decodes_a11_registration() {
    if !tshark_available() {
        return;
    }
    let request = A11Message::RegistrationRequest(RegistrationRequest {
        flags: 0x0a,
        lifetime: 1800,
        home_address: [0, 0, 0, 0],
        home_agent: PDSN.octets(),
        care_of_address: PCF.octets(),
        identification: 0x0102_0304_0506_0708,
        session: session(),
        extensions: vec![
            Extension::Nvse(Nvse::PcfEnabledFeature(
                PcfEnabledFeatureNvse::GreSegmentationEnabled,
            )),
            mobile_home_authentication(),
        ],
    });
    let reply = A11Message::RegistrationReply(RegistrationReply {
        code: 0,
        lifetime: 1800,
        home_address: [0, 0, 0, 0],
        home_agent: PDSN.octets(),
        identification: 0x0102_0304_0506_0708,
        session: session(),
        extensions: vec![
            Extension::Nvse(Nvse::PdsnEnabledFeature(
                PdsnEnabledFeatureNvse::PacketBoundaryEnabled,
            )),
            Extension::Nvse(Nvse::PdsnEnabledFeature(
                PdsnEnabledFeatureNvse::GreSegmentationEnabled,
            )),
            mobile_home_authentication(),
        ],
    });
    let packets = [
        Packet::udp(
            (PCF, A11_STANDARD_UDP_PORT),
            (PDSN, A11_STANDARD_UDP_PORT),
            &cdma_a11::encode(&request).unwrap(),
        ),
        Packet::udp(
            (PDSN, A11_STANDARD_UDP_PORT),
            (PCF, A11_STANDARD_UDP_PORT),
            &cdma_a11::encode(&reply).unwrap(),
        ),
    ];
    let capture = Capture::write(&capture_dir(), "a11", &packets).unwrap();
    capture.assert_clean();
    assert_eq!(
        capture.fields(&["a11.type", "a11.ext.key", "a11.ext.msid", "a11.ext.apptype"]),
        [
            "1\t0x01020304\t310009176936269\t0x0b02",
            "3\t0x01020304\t310009176936269\t0x0a02,0x0a03",
        ]
    );
}

#[test]
fn tshark_decodes_a21_page_exchange() {
    if !tshark_available() {
        return;
    }
    let identity = A21Identity::Imsi(IMSI.into());
    let page = A21Message::air_interface_signaling(
        42,
        identity.clone(),
        MessageTransmissionControl {
            paging_message: true,
            ..MessageTransmissionControl::default()
        },
        vec![vec![0x01, 0x02, 0x03]],
    );
    let ack = A21Message::ack(
        Some(42),
        Some(identity.clone()),
        Some(A21Cause::TunnelingNotAvailable),
    );
    let present = A21Message::event_notification(43, identity, A21EventKind::PresentInHrpd);
    let packets = [
        Packet::udp(
            (BS, A21_UDP_PORT),
            (AN_A21, A21_UDP_PORT),
            &page.encode().unwrap(),
        ),
        Packet::udp(
            (AN_A21, A21_UDP_PORT),
            (BS, A21_UDP_PORT),
            &ack.encode().unwrap(),
        ),
        Packet::udp(
            (AN_A21, A21_UDP_PORT),
            (BS, A21_UDP_PORT),
            &present.encode().unwrap(),
        ),
    ];
    let capture = Capture::write(&capture_dir(), "a21", &packets)
        .unwrap()
        .decode_as(&format!("udp.port=={A21_UDP_PORT},a21"));
    capture.assert_clean();
    assert_eq!(
        capture.fields(&["a21.message_type", "a21.corr_id_corr_value", "e212.imsi"]),
        [
            "1\t42\t310009176936269",
            "2\t42\t310009176936269",
            "4\t43\t310009176936269",
        ]
    );
}

/// One RFC 1662 frame. The test contents need no octet stuffing.
fn hdlc_frame(contents: &[u8]) -> Vec<u8> {
    let mut fcs = PPP_FCS_INIT;
    for octet in contents {
        fcs ^= u16::from(*octet);
        for _ in 0..8 {
            fcs = if fcs & 1 == 1 {
                (fcs >> 1) ^ PPP_FCS_POLYNOMIAL
            } else {
                fcs >> 1
            };
        }
    }
    let fcs = fcs ^ PPP_FCS_XOR;
    let mut frame = vec![HDLC_FLAG];
    frame.extend_from_slice(contents);
    frame.extend_from_slice(&fcs.to_le_bytes());
    frame.push(HDLC_FLAG);
    frame
}

#[test]
fn tshark_decodes_a8_a10_gre_attributes() {
    if !tshark_available() {
        return;
    }
    // LCP Configure-Request, identifier 1, no options.
    let ppp = hdlc_frame(&[0xff, 0x03, 0xc0, 0x21, 0x01, 0x01, 0x00, 0x04]);
    let forward = GrePacket {
        key_present: true,
        sequence_present: true,
        protocol_type: GRE_PROTOCOL_TYPE_3GPP2_PACKET,
        key: Some(0x0000_0101),
        sequence_number: Some(7),
        attributes: vec![GreAttribute::Segmentation(GreSegment::Started)],
        payload: ppp.clone(),
    };
    let a10 = GrePacket {
        key_present: true,
        sequence_present: false,
        protocol_type: A11_PROTOCOL_TYPE_BYTE_STREAM,
        key: Some(0x0000_0202),
        sequence_number: None,
        attributes: Vec::new(),
        payload: ppp,
    };
    let packets = [
        Packet::gre(PCF, BS, &forward.encode().unwrap()),
        Packet::gre(PDSN, PCF, &a10.encode().unwrap()),
    ];
    let capture = Capture::write(&capture_dir(), "gre", &packets).unwrap();
    capture.assert_clean();
    assert_eq!(
        capture.fields(&["gre.proto", "gre.3gpp2_attrib_id", "ppp.protocol"]),
        ["0x88d2\t0x04\t0xc021", "0x8881\t\t0xc021"]
    );
}

/// Octets one 9600 bps reverse fundamental channel RLP frame delivers.
const RLP_FRAME_OCTETS: usize = 20;

#[test]
fn tshark_decodes_each_aligned_reverse_gre_frame_as_ppp() {
    if !tshark_available() {
        return;
    }
    // LCP Configure-Request, then LCP Echo-Request with 32 octets of data.
    let mut echo = vec![
        0xff, 0x03, 0xc0, 0x21, 0x09, 0x02, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00,
    ];
    echo.extend_from_slice(&[0x20; 32]);
    let stream = [
        hdlc_frame(&[0xff, 0x03, 0xc0, 0x21, 0x01, 0x01, 0x00, 0x04]),
        hdlc_frame(&echo),
    ]
    .concat();
    let mut aligner = PppFrameAligner::new(DEFAULT_MAX_GRE_PAYLOAD);
    let packets: Vec<Packet> = stream
        .chunks(RLP_FRAME_OCTETS)
        .flat_map(|octets| aligner.push(octets))
        .enumerate()
        .map(|(sequence, chunk)| {
            let gre = GrePacket::octet_stream(0x0000_0101, Some(sequence as u32), chunk.payload);
            Packet::gre(BS, PCF, &gre.encode().unwrap())
        })
        .collect();
    let capture = Capture::write(&capture_dir(), "gre-aligned", &packets).unwrap();
    capture.assert_clean();
    assert_eq!(
        capture.fields(&["ppp.protocol", "ppp.code"]),
        ["0xc021\t1", "0xc021\t9"]
    );
}

#[test]
fn tshark_decodes_a2p_evrc_rtp_payload() {
    if !tshark_available() {
        return;
    }
    let format = VoiceBearerFormat::Evrc;
    let frames = [
        (EvrcFrameType::Full, vec![0x11; 22]),
        (EvrcFrameType::Eighth, vec![0x22; 2]),
    ];
    let packets: Vec<Packet> = frames
        .iter()
        .enumerate()
        .map(|(index, (frame_type, frame))| {
            let mut rtp = vec![0x80, RTP_EVRC_PAYLOAD_TYPE];
            rtp.extend_from_slice(&(index as u16).to_be_bytes());
            rtp.extend_from_slice(&((index as u32) * 160).to_be_bytes());
            rtp.extend_from_slice(&0x0102_0304u32.to_be_bytes());
            rtp.extend_from_slice(&format.encode_payload(*frame_type, frame).unwrap());
            Packet::udp((BS, RTP_PORT), (BS, RTP_PORT + 2), &rtp)
        })
        .collect();
    let capture = Capture::write(&capture_dir(), "a2p_evrc", &packets)
        .unwrap()
        .decode_as(&format!("udp.port=={RTP_PORT},rtp"))
        .decode_as(&format!("rtp.pt=={RTP_EVRC_PAYLOAD_TYPE},evrc"));
    capture.assert_clean();
    assert_eq!(
        capture.fields(&["evrc.frame_count", "evrc.toc.frame_type_hi"]),
        ["0\t4", "0\t1"]
    );
}
