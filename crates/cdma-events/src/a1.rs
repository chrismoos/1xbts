//! A1 observations use A.S0001 §3.1 and X.S0059-200 §8.1.1.2 framing. Encryption keys are redacted.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::NetworkEvent;
use crate::proto::{self, EventSource, network_event};
use cdma_ios::femto_interface_msg::{
    self as fx2, Discriminator, FemtoInterfaceMsg, MSG_TYPE_ADDS_PAGE, MSG_TYPE_ADDS_PAGE_ACK,
    MSG_TYPE_ADDS_TRANSFER, MSG_TYPE_ADDS_TRANSFER_ACK, MSG_TYPE_ASSIGNMENT_COMPLETE,
    MSG_TYPE_ASSIGNMENT_REQUEST, MSG_TYPE_CM_SERVICE_REQUEST, MSG_TYPE_LOCATION_UPDATING_ACCEPT,
    MSG_TYPE_LOCATION_UPDATING_REQUEST, MSG_TYPE_PAGING_REQUEST, MSG_TYPE_PAGING_RESPONSE,
};
use cdma_ios::femto_interface_msg::{
    IE_ADDS_USER_PART, IE_AUTHENTICATION_CHALLENGE_PARAMETER,
    IE_AUTHENTICATION_CONFIRMATION_PARAMETER, IE_AUTHENTICATION_PARAMETER_COUNT,
    IE_AUTHENTICATION_RESPONSE_PARAMETER, IE_CALLED_PARTY_BCD_NUMBER, IE_CHANNEL_NUMBER,
    IE_CHANNEL_TYPE, IE_CM_SERVICE_TYPE, IE_MOBILE_IDENTITY, IE_REGISTRATION_TYPE,
    IE_SERVICE_OPTION, IE_SLOT_CYCLE_INDEX, IE_TAG,
};
use cdma_ios::{CellId, CmServiceType, MobileIdentity};

mod native;

/// ADDS Data Burst Type code points (A.S0001 §6.2.2.67).
const ADDS_BURST_SMS: u8 = 0x03;
const ADDS_BURST_OTA: u8 = 0x04;
const ADDS_BURST_PLD: u8 = 0x05;
const ADDS_BURST_TYPE_MASK: u8 = 0x3f;

// Event IDs must differ within a microsecond because the UI deduplicates by ID.
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct A1Context {
    pub direction: proto::A1Direction,
    pub cell: Option<CellId>,
    pub sip_call_id: Option<String>,
    pub content_type: Option<String>,
    pub peer: Option<String>,
    pub a1_call_id: Option<u64>,
    pub correlated_imsi: Option<String>,
    pub peer_id: Option<String>,
}

impl Default for A1Context {
    fn default() -> Self {
        Self {
            direction: proto::A1Direction::Unspecified,
            cell: None,
            sip_call_id: None,
            content_type: None,
            peer: None,
            a1_call_id: None,
            correlated_imsi: None,
            peer_id: None,
        }
    }
}

struct A1Body {
    message_type: u8,
    discriminator: Option<Discriminator>,
    sdu: Vec<u8>,
    information_elements: Vec<proto::A1InformationElement>,
    message_name: Option<String>,
    voice_privacy: Option<String>,
}

impl A1Body {
    fn from_fx2(message: &FemtoInterfaceMsg) -> Self {
        let mut message = message.clone();
        let mut voice_privacy = (message.message_type == fx2::MSG_TYPE_PRIVACY_MODE_COMPLETE
            && message
                .ies
                .iter()
                .any(|ie| ie.iei == fx2::IE_VOICE_PRIVACY_REQUEST))
        .then(|| "pending".to_string());
        for ie in &mut message.ies {
            if ie.iei != fx2::IE_ENCRYPTION_INFORMATION {
                continue;
            }
            const TLV_HEADER_OCTETS: usize = 2;
            if let Some(payload) = ie.content.get(TLV_HEADER_OCTETS..) {
                if let Ok(mut info) = cdma_ios::EncryptionInformation::decode(payload) {
                    for parameter in &mut info.parameters {
                        if parameter.identifier == cdma_ios::typed::PRIVATE_LONG_CODE_IDENTIFIER {
                            voice_privacy = Some(
                                if message.message_type == fx2::MSG_TYPE_PRIVACY_MODE_COMMAND
                                    || (message.message_type == fx2::MSG_TYPE_ASSIGNMENT_REQUEST
                                        && parameter.status)
                                {
                                    "pending"
                                } else if parameter.status {
                                    "private"
                                } else {
                                    "public"
                                }
                                .to_string(),
                            );
                        }
                        parameter.value.fill(0);
                    }
                    if let Ok(payload) = info.encode() {
                        ie.content.truncate(TLV_HEADER_OCTETS);
                        ie.content.extend_from_slice(&payload);
                    }
                } else {
                    ie.content[TLV_HEADER_OCTETS..].fill(0);
                }
            }
        }
        Self {
            voice_privacy,
            message_type: message.message_type,
            discriminator: message.discriminator,
            sdu: message.to_layer3_sdu(),
            information_elements: message
                .ies
                .iter()
                .map(|ie| proto::A1InformationElement {
                    iei: u32::from(ie.iei),
                    name: fx2::information_element_name(ie.iei)
                        .map(str::to_string)
                        .unwrap_or_default(),
                    decoded: decode_ie(ie.iei, &ie.content),
                    content: ie.content.clone(),
                })
                .collect(),
            message_name: message.message_name.clone(),
        }
    }

    /// From an A1 message observed at the MSC boundary. Type and discriminator
    /// come from the framing the encoder wrote (A.S0001 §3.1), not from the
    /// project-local [`cdma_ios::MessageType`] routing tag.
    ///
    /// Information elements are left unsplit: BSMAP IE framing is
    /// per-message-type, not per-IEI — `Cause` is Type 4 in Clear Request and
    /// Type 3 in ADDS Transfer Ack — so a generic scan would desynchronise.
    fn from_a1_wire(encoded: &cdma_ios::EncodedA1Message) -> Option<Self> {
        let message = encoded.decode().ok()?;
        match message.message_type {
            cdma_ios::MessageType::AssignmentRequest => {
                let request = cdma_ios::AssignmentRequestMessage::decode(&message.payload).ok()?;
                if request.encryption_information.is_some() {
                    return Some(Self::from_fx2(
                        &FemtoInterfaceMsg::from_assignment_request(&request).ok()?,
                    ));
                }
            }
            cdma_ios::MessageType::AssignmentComplete => {
                let complete =
                    cdma_ios::AssignmentCompleteMessage::decode(&message.payload).ok()?;
                if complete.encryption_information.is_some() {
                    return Some(Self::from_fx2(
                        &FemtoInterfaceMsg::from_assignment_complete(&complete).ok()?,
                    ));
                }
            }
            cdma_ios::MessageType::PrivacyModeCommand => {
                let command = cdma_ios::PrivacyModeCommandMessage::decode(&message.payload).ok()?;
                return Some(Self::from_fx2(
                    &FemtoInterfaceMsg::from_privacy_mode_command(&command).ok()?,
                ));
            }
            cdma_ios::MessageType::PrivacyModeComplete => {
                let complete =
                    cdma_ios::PrivacyModeCompleteMessage::decode(&message.payload).ok()?;
                return Some(Self::from_fx2(
                    &FemtoInterfaceMsg::from_privacy_mode_complete(&complete).ok()?,
                ));
            }
            _ => {}
        }
        let frame = cdma_ios::split_a1_frame(&message.payload)?;
        Some(Self {
            message_type: frame.message_type,
            discriminator: Some(if frame.bsmap {
                Discriminator::Bsmap
            } else {
                Discriminator::Dtap
            }),
            sdu: frame.sdu,
            voice_privacy: None,
            information_elements: Vec::new(),
            message_name: Some(format!("{:?}", message.message_type)),
        })
    }
}

pub fn a1_signaling_event(
    message: &FemtoInterfaceMsg,
    context: &A1Context,
) -> proto::A1SignalingEvent {
    signaling_event(&A1Body::from_fx2(message), context)
}

fn signaling_event(body: &A1Body, context: &A1Context) -> proto::A1SignalingEvent {
    let sdu = body.sdu.clone();
    let discriminator = match body.discriminator {
        Some(Discriminator::Bsmap) => proto::A1MessageDiscriminator::Bsmap,
        Some(Discriminator::Dtap) => proto::A1MessageDiscriminator::Dtap,
        None => proto::A1MessageDiscriminator::Unspecified,
    };
    let message_type = classify_message_type(body.message_type, body.discriminator);
    let message_type_name = fx2::message_type_name(body.message_type, body.discriminator)
        .map(str::to_string)
        .or_else(|| body.message_name.clone())
        .unwrap_or_else(|| format!("A1 0x{:02X}", body.message_type));

    let information_elements = body.information_elements.clone();

    let mobile_identity =
        mobile_identity(&information_elements, context.correlated_imsi.as_deref());
    let adds = adds_payload(&information_elements);

    proto::A1SignalingEvent {
        voice_privacy: body.voice_privacy.clone(),
        direction: context.direction as i32,
        discriminator: discriminator as i32,
        message_type: message_type as i32,
        message_type_value: u32::from(body.message_type),
        summary: summarize(
            &message_type_name,
            information_elements.len(),
            mobile_identity.as_ref(),
            adds.as_ref(),
        ),
        message_type_name,
        sdu_hex: hex_upper(&sdu),
        sdu,
        information_elements,
        cell: context.cell.map(|cell| proto::CellId {
            cell: u32::from(cell.cell),
            sector: u32::from(cell.sector),
        }),
        mobile_identity,
        adds,
        timestamp_us: now_us(),
        event_id: next_event_id(),
        sip_call_id: context.sip_call_id.clone().unwrap_or_default(),
        content_type: context.content_type.clone().unwrap_or_default(),
        peer: context.peer.clone().unwrap_or_default(),
        peer_id: context.peer_id.clone().unwrap_or_default(),
        a1_call_id: context.a1_call_id.unwrap_or_default(),
    }
}

pub fn a1_event(
    source: EventSource,
    peer_id: &str,
    message: &FemtoInterfaceMsg,
    context: &A1Context,
) -> NetworkEvent {
    wrap_a1_event(source, peer_id, A1Body::from_fx2(message), context)
}

pub fn a1_event_from_wire(
    source: EventSource,
    peer_id: &str,
    encoded: &cdma_ios::EncodedA1Message,
    context: &A1Context,
) -> Option<NetworkEvent> {
    let context = A1Context {
        a1_call_id: context.a1_call_id.or(encoded.call_id()),
        ..context.clone()
    };
    let mut event = wrap_a1_event(source, peer_id, A1Body::from_a1_wire(encoded)?, &context);
    native::enrich(&mut event, encoded);
    Some(event)
}

fn wrap_a1_event(
    source: EventSource,
    peer_id: &str,
    body: A1Body,
    context: &A1Context,
) -> NetworkEvent {
    let mut context = context.clone();
    if context.peer_id.is_none() && !peer_id.is_empty() {
        context.peer_id = Some(peer_id.to_string());
    }
    let a1 = signaling_event(&body, &context);
    let identity = a1.mobile_identity.as_ref().map(|id| proto::MobileIdentity {
        imsi: id.imsi.clone(),
        esn: id.esn,
        meid: id.meid.clone(),
    });
    NetworkEvent {
        source: source as i32,
        identity,
        body: Some(network_event::Body::A1(Box::new(a1))),
        ..Default::default()
    }
}

/// The discriminator disambiguates the code points DTAP and BSMAP share.
fn classify_message_type(
    message_type: u8,
    discriminator: Option<Discriminator>,
) -> proto::A1MessageType {
    use proto::A1MessageType as T;
    let bsmap = matches!(discriminator, Some(Discriminator::Bsmap));
    match (message_type, bsmap) {
        (MSG_TYPE_ASSIGNMENT_REQUEST, true) => T::AssignmentRequest,
        (MSG_TYPE_ASSIGNMENT_COMPLETE, true) => T::AssignmentComplete,
        (MSG_TYPE_LOCATION_UPDATING_ACCEPT, false) => T::LocationUpdatingAccept,
        (MSG_TYPE_LOCATION_UPDATING_REQUEST, _) => T::LocationUpdatingRequest,
        (MSG_TYPE_CM_SERVICE_REQUEST, _) => T::CmServiceRequest,
        (MSG_TYPE_PAGING_RESPONSE, _) => T::PagingResponse,
        (MSG_TYPE_PAGING_REQUEST, _) => T::PagingRequest,
        (MSG_TYPE_ADDS_PAGE, _) => T::AddsPage,
        (MSG_TYPE_ADDS_PAGE_ACK, _) => T::AddsPageAck,
        (MSG_TYPE_ADDS_TRANSFER, _) => T::AddsTransfer,
        (MSG_TYPE_ADDS_TRANSFER_ACK, _) => T::AddsTransferAck,
        (0x53, false) => T::AddsDeliver,
        (0x54, false) => T::AddsDeliverAck,
        // Observed at the A1 reference point rather than over Fx2. The clear
        // procedures and Complete Layer 3 Information are BSMAP. Call progress
        // is DTAP. 0x03 and 0x04 collide across the two, so both are keyed on
        // the discriminator the framing supplied.
        (fx2::MSG_TYPE_COMPLETE_LAYER3_INFORMATION, true) => T::CompleteLayer3Information,
        (fx2::MSG_TYPE_CLEAR_COMMAND, true) => T::ClearCommand,
        (fx2::MSG_TYPE_CLEAR_COMPLETE, true) => T::ClearComplete,
        (fx2::MSG_TYPE_CLEAR_REQUEST, true) => T::ClearRequest,
        (fx2::MSG_TYPE_ASSIGNMENT_FAILURE, true) => T::AssignmentFailure,
        (fx2::MSG_TYPE_PROGRESS, false) if matches!(discriminator, Some(Discriminator::Dtap)) => {
            T::Progress
        }
        (fx2::MSG_TYPE_CONNECT, false) => T::Connect,
        (fx2::MSG_TYPE_ALERT_WITH_INFORMATION, false) => T::AlertWithInformation,
        _ => T::Other,
    }
}

/// A1 messages routinely carry several Mobile Identity IEs — IMSI plus ESN,
/// sometimes MEID — as repeated IEI 0x0D.
fn mobile_identity(
    information_elements: &[proto::A1InformationElement],
    correlated_imsi: Option<&str>,
) -> Option<proto::A1MobileIdentity> {
    let mut out = proto::A1MobileIdentity::default();
    for ie in information_elements
        .iter()
        .filter(|ie| ie.iei == u32::from(IE_MOBILE_IDENTITY))
    {
        // Content is [IEI, length, value...].
        let Some(value) = ie.content.get(2..) else {
            continue;
        };
        match MobileIdentity::decode(value) {
            Ok(MobileIdentity::Imsi(imsi)) => out.imsi = imsi,
            Ok(MobileIdentity::Esn(esn)) => out.esn = esn,
            Ok(MobileIdentity::Meid(meid)) => {
                out.meid = canonical_fx2_meid(&meid);
            }
            Err(_) => {}
        }
    }
    if let Some(imsi) = correlated_imsi
        && (out.imsi.is_empty() || imsi.ends_with(&out.imsi))
    {
        out.imsi = imsi.to_string();
    }
    (!out.imsi.is_empty() || out.esn != 0 || !out.meid.is_empty()).then_some(out)
}

/// IE framing. A Type 3/4 element carries a length octet after the IEI. A
/// fixed-width element goes straight to its value. A Type 1 element packs the
/// IEI and the value into a single octet.
///
/// Framing is per-element, not derivable from the bytes: `1F 03` is a two-byte
/// Registration Type whose value is `03`, while `0D 06 ...` is a Mobile
/// Identity whose `06` is a length.
const LOW_NIBBLE: u8 = 0x0f;
/// Type-of-number occupies three bits above the numbering plan (ANSI T1.607).
const NUMBER_TYPE_SHIFT: u8 = 4;
const NUMBER_TYPE_MASK: u8 = 0x07;
/// Terminates a BCD digit string.
const BCD_FILLER: u8 = 0x0f;

enum IeFraming {
    /// `[iei, len, value..]`
    Tlv,
    /// `[iei, value..]`
    Fixed,
}

fn ie_value(content: &[u8], framing: IeFraming) -> Option<&[u8]> {
    let skip = match framing {
        IeFraming::Tlv => 2,
        IeFraming::Fixed => 1,
    };
    let value = content.get(skip..)?;
    match framing {
        // Trust the length octet over the transport's framing when both are
        // present, so a trailing byte cannot leak into the value.
        IeFraming::Tlv => {
            let len = usize::from(*content.get(1)?);
            value.get(..len.min(value.len()))
        }
        IeFraming::Fixed => Some(value),
    }
}

fn be_u16(value: &[u8]) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(
        value.get(..2)?.try_into().ok()?,
    )))
}

/// Decode the BCD digits of a Called Party BCD Number, low nibble first,
/// stopping at the `0xF` filler. Digits 0-9 plus the `*`, `#`, a, b, c codes
/// C.S0005 allows in a dialed string.
fn bcd_digits(value: &[u8]) -> String {
    let mut out = String::new();
    for byte in value {
        for nibble in [byte & 0x0f, byte >> 4] {
            match nibble {
                0x0..=0x9 => out.push((b'0' + nibble) as char),
                0xa => out.push('*'),
                0xb => out.push('#'),
                0xc => out.push('a'),
                0xd => out.push('b'),
                0xe => out.push('c'),
                nibble if nibble >= BCD_FILLER => return out,
                _ => return out,
            }
        }
    }
    out
}

fn decode_ie(iei: u8, content: &[u8]) -> Option<proto::a1_information_element::Decoded> {
    use proto::a1_information_element::Decoded;
    match iei {
        IE_MOBILE_IDENTITY => {
            let value = ie_value(content, IeFraming::Tlv)?;
            let mut ie = proto::A1MobileIdentityIe::default();
            match MobileIdentity::decode(value).ok()? {
                MobileIdentity::Imsi(imsi) => {
                    ie.form = proto::A1MobileIdentityForm::Imsi as i32;
                    ie.imsi = imsi;
                }
                MobileIdentity::Esn(esn) => {
                    ie.form = proto::A1MobileIdentityForm::Esn as i32;
                    ie.esn = esn;
                }
                MobileIdentity::Meid(meid) => {
                    ie.form = proto::A1MobileIdentityForm::Meid as i32;
                    ie.meid = canonical_fx2_meid(&meid);
                }
            }
            Some(Decoded::MobileIdentity(ie))
        }
        IE_SERVICE_OPTION => Some(Decoded::ServiceOption(proto::A1ServiceOptionIe {
            service_option: be_u16(ie_value(content, IeFraming::Fixed)?)? as i32,
        })),
        IE_REGISTRATION_TYPE => {
            let value = *ie_value(content, IeFraming::Fixed)?.first()?;
            Some(Decoded::RegistrationType(proto::A1RegistrationTypeIe {
                r#type: i32::from(value),
            }))
        }
        IE_CM_SERVICE_TYPE => {
            // Type 1: the IEI and the value share the single octet, so the
            // whole octet goes to the decoder rather than a value slice.
            let octet = *content.first()?;
            CmServiceType::decode(octet).ok()?;
            Some(Decoded::CmServiceType(proto::A1CmServiceTypeIe {
                r#type: i32::from(octet & LOW_NIBBLE),
            }))
        }
        IE_SLOT_CYCLE_INDEX => Some(Decoded::SlotCycleIndex(proto::A1SlotCycleIndexIe {
            value: u32::from(*ie_value(content, IeFraming::Fixed)?.first()?),
        })),
        IE_TAG => {
            let value = ie_value(content, IeFraming::Fixed)?;
            Some(Decoded::Tag(proto::A1TagIe {
                value: u32::from_be_bytes(value.get(..4)?.try_into().ok()?),
            }))
        }
        IE_CHANNEL_TYPE => {
            let value = ie_value(content, IeFraming::Tlv)?;
            Some(Decoded::ChannelType(proto::A1ChannelTypeIe {
                speech_or_data_indicator: u32::from(*value.first()?),
                channel_rate_and_type: u32::from(*value.get(1)?),
                coding: u32::from(*value.get(2)?),
            }))
        }
        IE_CHANNEL_NUMBER => Some(Decoded::ChannelNumber(proto::A1ChannelNumberIe {
            value: be_u16(ie_value(content, IeFraming::Fixed)?)?,
        })),
        IE_CALLED_PARTY_BCD_NUMBER => {
            let value = ie_value(content, IeFraming::Tlv)?;
            // Octet 1 is the type-of-number and numbering-plan pair. The
            // dialed digits are the BCD that follows.
            let header = *value.first()?;
            Some(Decoded::CalledParty(proto::A1CalledPartyIe {
                number_type: i32::from((header >> NUMBER_TYPE_SHIFT) & NUMBER_TYPE_MASK),
                numbering_plan: i32::from(header & LOW_NIBBLE),
                digits: bcd_digits(value.get(1..)?),
            }))
        }
        IE_AUTHENTICATION_RESPONSE_PARAMETER => {
            let value = ie_value(content, IeFraming::Tlv)?;
            Some(Decoded::AuthResponse(proto::A1AuthResponseIe {
                value_hex: hex_upper(value),
                value: value.to_vec(),
            }))
        }
        IE_AUTHENTICATION_CONFIRMATION_PARAMETER => {
            Some(Decoded::AuthConfirmation(proto::A1AuthConfirmationIe {
                randc: u32::from(*ie_value(content, IeFraming::Fixed)?.first()?),
            }))
        }
        IE_AUTHENTICATION_PARAMETER_COUNT => Some(Decoded::AuthCount(proto::A1AuthCountIe {
            count: u32::from(*ie_value(content, IeFraming::Fixed)?.first()?),
        })),
        IE_AUTHENTICATION_CHALLENGE_PARAMETER => {
            let value = ie_value(content, IeFraming::Tlv)?;
            Some(Decoded::AuthChallenge(proto::A1AuthChallengeIe {
                value_hex: hex_upper(value),
                value: value.to_vec(),
            }))
        }
        _ => None,
    }
}

fn adds_payload(
    information_elements: &[proto::A1InformationElement],
) -> Option<proto::A1AddsPayload> {
    let ie = information_elements
        .iter()
        .find(|ie| ie.iei == u32::from(IE_ADDS_USER_PART))?;
    // Content is [IEI, length, burst_type, data...].
    let body = ie.content.get(2..)?;
    let (&burst, data) = body.split_first()?;
    Some(adds_summary(burst, data))
}

fn adds_summary(burst: u8, data: &[u8]) -> proto::A1AddsPayload {
    let burst_type_value = burst & ADDS_BURST_TYPE_MASK;
    let burst_type = match burst_type_value {
        ADDS_BURST_SMS => proto::AddsBurstType::Sms,
        ADDS_BURST_OTA => proto::AddsBurstType::Ota,
        ADDS_BURST_PLD => proto::AddsBurstType::Pld,
        _ => proto::AddsBurstType::Other,
    };
    proto::A1AddsPayload {
        burst_type: burst_type as i32,
        burst_type_value: u32::from(burst_type_value),
        payload_length_bytes: data.len() as u32,
    }
}

fn summarize(
    name: &str,
    ie_count: usize,
    identity: Option<&proto::A1MobileIdentity>,
    adds: Option<&proto::A1AddsPayload>,
) -> String {
    let mut parts = Vec::new();
    if let Some(identity) = identity {
        if !identity.imsi.is_empty() {
            parts.push(format!("IMSI {}", identity.imsi));
        } else if identity.esn != 0 {
            parts.push(format!("ESN 0x{:08X}", identity.esn));
        } else if !identity.meid.is_empty() {
            parts.push(format!("MEID {}", identity.meid));
        }
    }
    if let Some(adds) = adds {
        let kind = match proto::AddsBurstType::try_from(adds.burst_type) {
            Ok(proto::AddsBurstType::Sms) => "SMS",
            Ok(proto::AddsBurstType::Ota) => "OTA",
            Ok(proto::AddsBurstType::Pld) => "PLD",
            _ => "ADDS",
        };
        parts.push(format!("{kind} {} bytes", adds.payload_length_bytes));
    }
    if ie_count > 0 {
        parts.push(format!("{ie_count} IE(s)"));
    }
    if parts.is_empty() {
        return name.to_string();
    }
    format!("{name} — {}", parts.join(", "))
}

fn canonical_fx2_meid(digits: &[u8]) -> String {
    let transmitted: String = digits.iter().map(|byte| format!("{byte:02x}")).collect();
    transmitted.chars().rev().collect()
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn next_event_id() -> String {
    format!(
        "a1-{:x}-{:x}",
        now_us(),
        EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod privacy_tests {
    use super::*;

    #[test]
    fn privacy_observations_redact_key_bytes_and_keep_pending_and_active_distinct() {
        let command = cdma_ios::PrivacyModeCommandMessage {
            encryption_information: cdma_ios::EncryptionInformation::private_long_code(
                0x147_80cc_b8df,
                true,
            )
            .unwrap(),
        };
        let femto = FemtoInterfaceMsg::from_privacy_mode_command(&command).unwrap();
        let event = a1_signaling_event(&femto, &A1Context::default());
        assert_eq!(event.voice_privacy.as_deref(), Some("pending"));
        assert!(!event.sdu_hex.contains("014780CCB8DF"));
        assert!(
            event
                .information_elements
                .iter()
                .all(|ie| !hex_upper(&ie.content).contains("014780CCB8DF"))
        );
        assert_eq!(
            cdma_ios::PrivacyModeCommandMessage::decode(&femto.to_bsmap_wire().unwrap()).unwrap(),
            command
        );
        let complete = cdma_ios::PrivacyModeCompleteMessage {
            encryption_information: Some(
                cdma_ios::EncryptionInformation::private_long_code_status(true),
            ),
            voice_privacy_request: false,
        };
        let femto = FemtoInterfaceMsg::from_privacy_mode_complete(&complete).unwrap();
        let event = a1_signaling_event(&femto, &A1Context::default());
        assert_eq!(event.voice_privacy.as_deref(), Some("private"));
        assert_eq!(event.sdu_hex, "0005550A029300");
    }
}
