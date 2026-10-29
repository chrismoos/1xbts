//! `FemtoInterfaceMsg` codec (3GPP2 X.S0059-200 §8.1.1.2).
//!
//! Internal BSC/MSC routing tags and transport call IDs must not enter SIP bodies.

use crate::typed::{
    AddsDeliverAckMessage, AddsDeliverMessage, AddsPageAckMessage, AddsPageMessage,
    AddsTransferAckMessage, AddsTransferMessage, AssignmentCompleteMessage,
    AssignmentRequestMessage, CmServiceRequestMessage, LocationUpdatingAcceptMessage,
    LocationUpdatingRequestMessage, MobileIdentity, PagingRequestMessage, PagingResponseMessage,
};

pub const CONTENT_TYPE_FEMTO: &str = "application/vnd.3gpp2.FemtoInterfaceMsg";
pub const CONTENT_TYPE_SDP: &str = "application/sdp";
pub const CONTENT_TYPE_SMS: &str = "application/vnd.3gpp2.sms";

pub const DISCRIMINATOR_DTAP: char = '0';
pub const DISCRIMINATOR_BSMAP: char = '1';

const CRLF: &str = "\r\n";
const HEX_RADIX: u32 = 16;

const DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT: u8 = 0x05;
const DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING: u8 = 0x03;
const DTAP_RESERVED_OCTET: u8 = 0x00;
const BSMAP_MESSAGE_DISCRIMINATION: u8 = 0x00;
const DTAP_MESSAGE_DISCRIMINATION: u8 = 0x01;
const DTAP_LINK_IDENTIFIER: u8 = 0x00;

/// DTAP Message Type Value for `Location Updating Request` (A.S0014 §3.1.2.6).
pub const MSG_TYPE_LOCATION_UPDATING_REQUEST: u8 = 0x08;
/// DTAP Message Type Value for `CM Service Request` (A.S0014).
pub const MSG_TYPE_CM_SERVICE_REQUEST: u8 = 0x24;
pub const MSG_TYPE_LOCATION_UPDATING_ACCEPT: u8 = 0x02;
pub const MSG_TYPE_ASSIGNMENT_REQUEST: u8 = 0x01;
pub const MSG_TYPE_ASSIGNMENT_COMPLETE: u8 = 0x02;
pub const MSG_TYPE_PAGING_REQUEST: u8 = 0x52;
pub const MSG_TYPE_ADDS_DELIVER: u8 = 0x53;
pub const MSG_TYPE_ADDS_DELIVER_ACK: u8 = 0x54;
pub const MSG_TYPE_ADDS_PAGE: u8 = 0x65;
pub const MSG_TYPE_ADDS_PAGE_ACK: u8 = 0x66;
pub const MSG_TYPE_ADDS_TRANSFER: u8 = 0x67;
pub const MSG_TYPE_ADDS_TRANSFER_ACK: u8 = 0x68;
/// A.S0014-D §3.3.12 and §3.3.13.
pub const MSG_TYPE_PRIVACY_MODE_COMMAND: u8 = 0x53;
pub const MSG_TYPE_PRIVACY_MODE_COMPLETE: u8 = 0x55;
pub const IE_VOICE_PRIVACY_REQUEST: u8 = 0xa1;
pub const MSG_TYPE_PAGING_RESPONSE: u8 = 0x27;
/// BSMAP Message Type Values this stack observes but does not carry over Fx2.
/// Named here so an A1 message seen at the MSC boundary still logs by name
/// (A.S0001 Table 3.1.1-1).
pub const MSG_TYPE_ASSIGNMENT_FAILURE: u8 = 0x03;
pub const MSG_TYPE_USER_ZONE_UPDATE: u8 = 0x04;
pub const MSG_TYPE_CLEAR_COMMAND: u8 = 0x20;
pub const MSG_TYPE_CLEAR_COMPLETE: u8 = 0x21;
pub const MSG_TYPE_CLEAR_REQUEST: u8 = 0x22;
pub const MSG_TYPE_COMPLETE_LAYER3_INFORMATION: u8 = 0x57;
/// DTAP Message Type Values likewise (A.S0014 / A.S0001 Table 3.1.2-1).
pub const MSG_TYPE_PROGRESS: u8 = 0x03;
pub const MSG_TYPE_LOCATION_UPDATING_REJECT: u8 = 0x04;
pub const MSG_TYPE_CONNECT: u8 = 0x07;
pub const MSG_TYPE_ALERT_WITH_INFORMATION: u8 = 0x26;

// The femto body carries the IEI inside the base64 content of every Type 3/4 IE.
pub const IE_USER_ZONE_ID: u8 = 0x02;
pub const IE_CAUSE: u8 = 0x04;
pub const IE_CIRCUIT_IDENTITY_CODE: u8 = 0x01;
pub const IE_SERVICE_OPTION: u8 = 0x03;
pub const IE_PRIORITY: u8 = 0x06;
pub const IE_QUALITY_OF_SERVICE_PARAMETERS: u8 = 0x07;
pub const IE_ENCRYPTION_INFORMATION: u8 = 0x0a;
pub const IE_CHANNEL_TYPE: u8 = 0x0b;
pub const IE_CHANNEL_NUMBER: u8 = 0x23;
pub const IE_MOBILE_IDENTITY: u8 = 0x0d;
pub const IE_IS2000_MOBILE_CAPABILITIES: u8 = 0x11;
pub const IE_LOCATION_AREA_IDENTIFICATION: u8 = 0x13;
pub const IE_CLASSMARK_INFORMATION_TYPE_2: u8 = 0x12;
pub const IE_REGISTRATION_TYPE: u8 = 0x1f;
pub const IE_AUTHENTICATION_CONFIRMATION_PARAMETER: u8 = 0x28;
pub const IE_SLOT_CYCLE_INDEX: u8 = 0x35;
pub const IE_AUTHENTICATION_PARAMETER_COUNT: u8 = 0x40;
pub const IE_AUTHENTICATION_CHALLENGE_PARAMETER: u8 = 0x41;
pub const IE_AUTHENTICATION_RESPONSE_PARAMETER: u8 = 0x42;
pub const IE_AUTHENTICATION_EVENT: u8 = 0x4a;
pub const IE_CM_SERVICE_TYPE: u8 = 0x90;
pub const IE_MS_INFORMATION_RECORDS: u8 = 0x15;
pub const IE_CELL_IDENTIFIER: u8 = 0x05;
pub const IE_CELL_IDENTIFIER_LIST: u8 = 0x1a;
pub const IE_TAG: u8 = 0x33;
pub const IE_SIGNAL: u8 = 0x34;
pub const IE_A2P_BEARER_SESSION_PARAMS: u8 = 0x45;
pub const IE_A2P_BEARER_FORMAT_PARAMS: u8 = 0x46;
pub const IE_PACA_TIMESTAMP: u8 = 0x4e;
pub const IE_ADDS_USER_PART: u8 = 0x3d;
pub const IE_CALLED_PARTY_BCD_NUMBER: u8 = 0x5e;

const AUTHENTICATION_PARAMETER_COUNT_MASK: u8 = 0x3f;

/// The discriminator is required because DTAP and BSMAP share code points:
/// `0x02` is DTAP `Location Updating Accept` but BSMAP `Assignment Complete`,
/// and `0x53`/`0x54` collide likewise.
pub fn message_type_name(
    message_type: u8,
    discriminator: Option<Discriminator>,
) -> Option<&'static str> {
    if message_type == MSG_TYPE_LOCATION_UPDATING_ACCEPT {
        return match discriminator {
            Some(Discriminator::Bsmap) => Some("Assignment Complete"),
            Some(Discriminator::Dtap) => Some("Location Updating Accept"),
            None => None,
        };
    }
    if message_type == MSG_TYPE_ASSIGNMENT_FAILURE {
        return match discriminator {
            Some(Discriminator::Bsmap) => Some("Assignment Failure"),
            Some(Discriminator::Dtap) => Some("Progress"),
            None => None,
        };
    }
    if message_type == MSG_TYPE_USER_ZONE_UPDATE {
        return match discriminator {
            Some(Discriminator::Bsmap) => Some("User Zone Update"),
            Some(Discriminator::Dtap) => Some("Location Updating Reject"),
            None => None,
        };
    }
    let bsmap = matches!(discriminator, Some(Discriminator::Bsmap));
    Some(match (message_type, bsmap) {
        (MSG_TYPE_PRIVACY_MODE_COMMAND, true) => "Privacy Mode Command",
        (MSG_TYPE_PRIVACY_MODE_COMPLETE, true) => "Privacy Mode Complete",
        (MSG_TYPE_ASSIGNMENT_REQUEST, true) => "Assignment Request",
        (MSG_TYPE_CLEAR_COMMAND, true) => "Clear Command",
        (MSG_TYPE_CLEAR_COMPLETE, true) => "Clear Complete",
        (MSG_TYPE_CLEAR_REQUEST, true) => "Clear Request",
        (MSG_TYPE_COMPLETE_LAYER3_INFORMATION, true) => "Complete Layer 3 Information",
        (MSG_TYPE_CONNECT, false) => "Connect",
        (MSG_TYPE_ALERT_WITH_INFORMATION, false) => "Alert With Information",
        (MSG_TYPE_LOCATION_UPDATING_REQUEST, _) => "Location Updating Request",
        (MSG_TYPE_CM_SERVICE_REQUEST, _) => "CM Service Request",
        (MSG_TYPE_PAGING_RESPONSE, _) => "Paging Response",
        (MSG_TYPE_PAGING_REQUEST, _) => "Paging Request",
        (MSG_TYPE_ADDS_PAGE, _) => "ADDS Page",
        (MSG_TYPE_ADDS_PAGE_ACK, _) => "ADDS Page Ack",
        (MSG_TYPE_ADDS_TRANSFER, _) => "ADDS Transfer",
        (MSG_TYPE_ADDS_TRANSFER_ACK, _) => "ADDS Transfer Ack",
        (0x53, false) => "ADDS Deliver",
        (0x54, false) => "ADDS Deliver Ack",
        _ => return None,
    })
}

pub fn information_element_name(iei: u8) -> Option<&'static str> {
    Some(match iei {
        IE_CIRCUIT_IDENTITY_CODE => "Circuit Identity Code",
        IE_USER_ZONE_ID => "User Zone ID",
        IE_SERVICE_OPTION => "Service Option",
        IE_CAUSE => "Cause",
        IE_PRIORITY => "Priority",
        IE_QUALITY_OF_SERVICE_PARAMETERS => "Quality of Service Parameters",
        IE_ENCRYPTION_INFORMATION => "Encryption Information",
        IE_CHANNEL_TYPE => "Channel Type",
        IE_MOBILE_IDENTITY => "Mobile Identity",
        IE_IS2000_MOBILE_CAPABILITIES => "IS-2000 Mobile Capabilities",
        IE_CLASSMARK_INFORMATION_TYPE_2 => "Classmark Information Type 2",
        IE_LOCATION_AREA_IDENTIFICATION => "Location Area Identification",
        IE_MS_INFORMATION_RECORDS => "MS Information Records",
        IE_CALLED_PARTY_BCD_NUMBER => "Called Party BCD Number",
        IE_CELL_IDENTIFIER_LIST => "Cell Identifier List",
        IE_REGISTRATION_TYPE => "Registration Type",
        IE_CHANNEL_NUMBER => "Channel Number",
        IE_TAG => "Tag",
        IE_SIGNAL => "Signal",
        IE_SLOT_CYCLE_INDEX => "Slot Cycle Index",
        IE_ADDS_USER_PART => "ADDS User Part",
        IE_AUTHENTICATION_PARAMETER_COUNT => "Authentication Parameter COUNT",
        IE_AUTHENTICATION_CHALLENGE_PARAMETER => "Authentication Challenge Parameter",
        IE_AUTHENTICATION_RESPONSE_PARAMETER => "Authentication Response Parameter",
        IE_AUTHENTICATION_CONFIRMATION_PARAMETER => "Authentication Confirmation Parameter",
        IE_AUTHENTICATION_EVENT => "Authentication Event",
        IE_A2P_BEARER_SESSION_PARAMS => "A2p Bearer Session-Level Parameters",
        IE_A2P_BEARER_FORMAT_PARAMS => "A2p Bearer Format-Specific Parameters",
        IE_PACA_TIMESTAMP => "PACA Timestamp",
        IE_CM_SERVICE_TYPE => "CM Service Type",
        _ => return None,
    })
}

/// On these code points the message-discriminator is mandatory to tell the two
/// apart. `0x53` is DTAP `ADDS Deliver` versus BSMAP `Privacy Mode Command`.
/// `0x54` is DTAP `ADDS Deliver Ack` versus a BSMAP code point at the same value.
const CONFLICTING_CODE_POINTS: [u8; 2] = [0x53, 0x54];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FemtoError {
    NotCrlf,
    EmptyBody,
    BadHex { field: &'static str, value: String },
    BadDiscriminator { value: char },
    MissingDiscriminatorOnConflict { message_type: u8 },
    MalformedIeLine { line: String },
    BadBase64 { detail: &'static str },
    MissingMandatoryIe { message_type: u8, iei: u8 },
    A1Codec(crate::Error),
    MalformedMultipart { detail: &'static str },
}

impl std::fmt::Display for FemtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for FemtoError {}

impl From<crate::Error> for FemtoError {
    fn from(value: crate::Error) -> Self {
        Self::A1Codec(value)
    }
}

pub type Result<T> = std::result::Result<T, FemtoError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discriminator {
    Dtap,
    Bsmap,
}

impl Discriminator {
    fn from_char(value: char) -> Result<Self> {
        match value {
            DISCRIMINATOR_DTAP => Ok(Self::Dtap),
            DISCRIMINATOR_BSMAP => Ok(Self::Bsmap),
            other => Err(FemtoError::BadDiscriminator { value: other }),
        }
    }

    fn as_char(self) -> char {
        match self {
            Self::Dtap => DISCRIMINATOR_DTAP,
            Self::Bsmap => DISCRIMINATOR_BSMAP,
        }
    }
}

/// One information element as carried in the femto body: the two-hex IEI prefix
/// plus the exact bytes the base64 content decodes to. For a Type 3/4 IE those
/// bytes begin with the IEI itself, so `content[0] == iei`. The struct keeps
/// them verbatim so any IE — including ones this crate has no typed model for —
/// round-trips byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FemtoIe {
    pub iei: u8,
    pub content: Vec<u8>,
}

impl FemtoIe {
    pub fn tlv(iei: u8, value: &[u8]) -> Result<Self> {
        if value.len() > u8::MAX as usize {
            return Err(FemtoError::A1Codec(crate::Error::InvalidLength {
                expected: u8::MAX as usize,
                actual: value.len(),
            }));
        }
        let mut content = Vec::with_capacity(2 + value.len());
        content.push(iei);
        content.push(value.len() as u8);
        content.extend_from_slice(value);
        Ok(Self { iei, content })
    }

    pub fn tv(iei: u8, value: &[u8]) -> Self {
        let mut content = Vec::with_capacity(1 + value.len());
        content.push(iei);
        content.extend_from_slice(value);
        Self { iei, content }
    }

    fn value(&self, type4: bool) -> Result<&[u8]> {
        let skip = if type4 { 2 } else { 1 };
        self.content
            .get(skip..)
            .ok_or(FemtoError::A1Codec(crate::Error::Truncated {
                needed: skip,
                actual: self.content.len(),
            }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FemtoInterfaceMsg {
    pub message_type: u8,
    pub discriminator: Option<Discriminator>,
    pub message_name: Option<String>,
    pub ies: Vec<FemtoIe>,
}

impl FemtoInterfaceMsg {
    pub fn new(message_type: u8, ies: Vec<FemtoIe>) -> Self {
        Self {
            message_type,
            discriminator: None,
            message_name: None,
            ies,
        }
    }

    pub fn first_ie(&self, iei: u8) -> Option<&FemtoIe> {
        self.ies.iter().find(|ie| ie.iei == iei)
    }

    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str(&format!("{:02X}", self.message_type));
        if self.discriminator.is_some() || self.message_name.is_some() {
            out.push_str(" (=\"");
            if let Some(discriminator) = self.discriminator {
                out.push(discriminator.as_char());
            }
            if let Some(name) = &self.message_name {
                if self.discriminator.is_some() {
                    out.push(' ');
                }
                out.push_str(name);
            }
            out.push_str("\")");
        }
        out.push_str(CRLF);
        for ie in &self.ies {
            out.push_str(&format!(
                "{:02X}={}{}",
                ie.iei,
                base64_encode(&ie.content),
                CRLF
            ));
        }
        out.into_bytes()
    }

    pub fn parse(body: &[u8]) -> Result<Self> {
        Self::decode(body)
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(body).map_err(|_| FemtoError::BadBase64 {
            detail: "body is not valid UTF-8 text",
        })?;
        if text.contains('\n') && !text.contains(CRLF) {
            return Err(FemtoError::NotCrlf);
        }
        let bytes = text.as_bytes();
        for (index, &byte) in bytes.iter().enumerate() {
            if byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r') {
                return Err(FemtoError::NotCrlf);
            }
        }
        let mut lines = text.split(CRLF).filter(|line| !line.is_empty());
        let identifier = lines.next().ok_or(FemtoError::EmptyBody)?;
        let (message_type, discriminator, message_name) = parse_identifier(identifier)?;
        let mut ies = Vec::new();
        for line in lines {
            let (prefix, encoded) =
                line.split_once('=')
                    .ok_or_else(|| FemtoError::MalformedIeLine {
                        line: line.to_string(),
                    })?;
            let iei = parse_hex_octet("iei", prefix)?;
            let content = base64_decode(encoded)?;
            ies.push(FemtoIe { iei, content });
        }
        let msg = Self {
            message_type,
            discriminator,
            message_name,
            ies,
        };
        msg.validate()?;
        Ok(msg)
    }

    pub fn validate(&self) -> Result<()> {
        if CONFLICTING_CODE_POINTS.contains(&self.message_type) && self.discriminator.is_none() {
            return Err(FemtoError::MissingDiscriminatorOnConflict {
                message_type: self.message_type,
            });
        }
        if self.message_type == MSG_TYPE_LOCATION_UPDATING_REQUEST
            && self.first_ie(IE_MOBILE_IDENTITY).is_none()
        {
            return Err(FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_MOBILE_IDENTITY,
            });
        }
        if self.message_type == MSG_TYPE_CM_SERVICE_REQUEST {
            for iei in [
                IE_CM_SERVICE_TYPE,
                IE_CLASSMARK_INFORMATION_TYPE_2,
                IE_MOBILE_IDENTITY,
            ] {
                if self.first_ie(iei).is_none() {
                    return Err(FemtoError::MissingMandatoryIe {
                        message_type: self.message_type,
                        iei,
                    });
                }
            }
        }
        Ok(())
    }

    pub fn from_location_updating_request(msg: &LocationUpdatingRequestMessage) -> Result<Self> {
        let mut ies = Vec::new();
        ies.push(FemtoIe::tlv(
            IE_MOBILE_IDENTITY,
            &msg.mobile_identity_imsi.encode()?,
        )?);
        if let Some(lai) = msg.location_area_identification {
            ies.push(FemtoIe::tv(IE_LOCATION_AREA_IDENTIFICATION, &lai.encode()?));
        }
        if let Some(classmark) = &msg.classmark_information_type_2 {
            ies.push(FemtoIe::tlv(
                IE_CLASSMARK_INFORMATION_TYPE_2,
                classmark.encode()?,
            )?);
        }
        if let Some(registration_type) = msg.registration_type {
            ies.push(FemtoIe::tv(
                IE_REGISTRATION_TYPE,
                &[registration_type.encode()?],
            ));
        }
        if let Some(esn) = &msg.mobile_identity_esn {
            ies.push(FemtoIe::tlv(IE_MOBILE_IDENTITY, &esn.encode()?)?);
        }
        if let Some(meid) = &msg.mobile_identity_meid {
            ies.push(FemtoIe::tlv(IE_MOBILE_IDENTITY, &meid.encode()?)?);
        }
        if let Some(slot_cycle_index) = msg.slot_cycle_index {
            ies.push(FemtoIe::tv(
                IE_SLOT_CYCLE_INDEX,
                &[slot_cycle_index.encode()?],
            ));
        }
        if let Some(response) = msg.authentication_response_parameter {
            ies.push(FemtoIe::tlv(
                IE_AUTHENTICATION_RESPONSE_PARAMETER,
                &response.encode(),
            )?);
        }
        if let Some(confirmation) = msg.authentication_confirmation_parameter {
            ies.push(FemtoIe::tv(
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                &[confirmation.0],
            ));
        }
        if let Some(count) = msg.authentication_parameter_count {
            ies.push(FemtoIe::tv(
                IE_AUTHENTICATION_PARAMETER_COUNT,
                &[count.0 & AUTHENTICATION_PARAMETER_COUNT_MASK],
            ));
        }
        if let Some(challenge) = msg.authentication_challenge_parameter {
            ies.push(FemtoIe::tlv(
                IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                &challenge.encode(),
            )?);
        }
        if let Some(event) = msg.authentication_event {
            ies.push(FemtoIe::tlv(IE_AUTHENTICATION_EVENT, &[event.0])?);
        }
        if let Some(user_zone_id) = msg.user_zone_id {
            ies.push(FemtoIe::tv(IE_USER_ZONE_ID, &user_zone_id.encode()));
        }
        if let Some(capabilities) = &msg.is2000_mobile_capabilities {
            ies.push(FemtoIe::tlv(
                IE_IS2000_MOBILE_CAPABILITIES,
                capabilities.encode()?,
            )?);
        }
        Ok(Self {
            message_type: MSG_TYPE_LOCATION_UPDATING_REQUEST,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("Location Updating Request".to_string()),
            ies,
        })
    }

    pub fn from_location_updating_accept(msg: &LocationUpdatingAcceptMessage) -> Result<Self> {
        let mut ies = Vec::new();
        if let Some(location_area_identification) = msg.location_area_identification {
            ies.push(FemtoIe::tv(
                IE_LOCATION_AREA_IDENTIFICATION,
                &location_area_identification.encode()?,
            ));
        }
        Ok(Self {
            message_type: MSG_TYPE_LOCATION_UPDATING_ACCEPT,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("Location Updating Accept".to_string()),
            ies,
        })
    }

    pub fn from_privacy_mode_command(msg: &crate::PrivacyModeCommandMessage) -> Result<Self> {
        Ok(Self {
            message_type: MSG_TYPE_PRIVACY_MODE_COMMAND,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("Privacy Mode Command".to_string()),
            ies: vec![FemtoIe::tlv(
                IE_ENCRYPTION_INFORMATION,
                &msg.encryption_information.encode()?,
            )?],
        })
    }

    pub fn from_privacy_mode_complete(msg: &crate::PrivacyModeCompleteMessage) -> Result<Self> {
        msg.encode()?;
        let mut ies = Vec::new();
        if let Some(info) = &msg.encryption_information {
            ies.push(FemtoIe::tlv(IE_ENCRYPTION_INFORMATION, &info.encode()?)?);
        }
        if msg.voice_privacy_request {
            ies.push(FemtoIe {
                iei: IE_VOICE_PRIVACY_REQUEST,
                content: vec![IE_VOICE_PRIVACY_REQUEST],
            });
        }
        Ok(Self {
            message_type: MSG_TYPE_PRIVACY_MODE_COMPLETE,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("Privacy Mode Complete".to_string()),
            ies,
        })
    }

    pub fn from_assignment_request(msg: &AssignmentRequestMessage) -> Result<Self> {
        let mut ies = vec![FemtoIe::tlv(IE_CHANNEL_TYPE, &msg.channel_type.encode())?];
        if let Some(circuit_identity_code) = msg.circuit_identity_code {
            ies.push(FemtoIe::tv(
                IE_CIRCUIT_IDENTITY_CODE,
                &circuit_identity_code.encode()?,
            ));
        }
        if let Some(encryption_information) = &msg.encryption_information {
            ies.push(FemtoIe::tlv(
                IE_ENCRYPTION_INFORMATION,
                &encryption_information.encode()?,
            )?);
        }
        if let Some(service_option) = msg.service_option {
            ies.push(FemtoIe::tv(IE_SERVICE_OPTION, &service_option.encode()));
        }
        for signal in &msg.signals {
            ies.push(FemtoIe::tv(IE_SIGNAL, &[signal.encode()?]));
        }
        if let Some(ms_information_records) = &msg.ms_information_records {
            ies.push(FemtoIe::tlv(
                IE_MS_INFORMATION_RECORDS,
                &ms_information_records.encode()?,
            )?);
        }
        if let Some(priority) = msg.priority {
            ies.push(FemtoIe::tlv(IE_PRIORITY, &priority.encode()?)?);
        }
        if let Some(paca_timestamp) = msg.paca_timestamp {
            ies.push(FemtoIe::tlv(IE_PACA_TIMESTAMP, &paca_timestamp.encode())?);
        }
        if let Some(parameters) = msg.quality_of_service_parameters {
            ies.push(FemtoIe::tlv(
                IE_QUALITY_OF_SERVICE_PARAMETERS,
                &parameters.encode()?,
            )?);
        }
        if let Some(parameters) = msg.a2p_bearer_session_params {
            ies.push(FemtoIe::tlv(
                IE_A2P_BEARER_SESSION_PARAMS,
                &parameters.encode(),
            )?);
        }
        if let Some(parameters) = &msg.a2p_bearer_format_params {
            ies.push(FemtoIe::tlv(
                IE_A2P_BEARER_FORMAT_PARAMS,
                &parameters.encode(),
            )?);
        }
        Ok(Self {
            message_type: MSG_TYPE_ASSIGNMENT_REQUEST,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("Assignment Request".to_string()),
            ies,
        })
    }

    pub fn from_assignment_complete(msg: &AssignmentCompleteMessage) -> Result<Self> {
        let mut ies = vec![FemtoIe::tv(IE_CHANNEL_NUMBER, &msg.channel_number.encode())];
        if let Some(encryption_information) = &msg.encryption_information {
            ies.push(FemtoIe::tlv(
                IE_ENCRYPTION_INFORMATION,
                &encryption_information.encode()?,
            )?);
        }
        if let Some(service_option) = msg.service_option {
            ies.push(FemtoIe::tv(IE_SERVICE_OPTION, &service_option.encode()));
        }
        if let Some(parameters) = msg.a2p_bearer_session_params {
            ies.push(FemtoIe::tlv(
                IE_A2P_BEARER_SESSION_PARAMS,
                &parameters.encode(),
            )?);
        }
        if let Some(parameters) = &msg.a2p_bearer_format_params {
            ies.push(FemtoIe::tlv(
                IE_A2P_BEARER_FORMAT_PARAMS,
                &parameters.encode(),
            )?);
        }
        Ok(Self {
            message_type: MSG_TYPE_ASSIGNMENT_COMPLETE,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("Assignment Complete".to_string()),
            ies,
        })
    }

    pub fn to_assignment_complete(&self) -> Result<AssignmentCompleteMessage> {
        if self.message_type != MSG_TYPE_ASSIGNMENT_COMPLETE {
            return Err(FemtoError::A1Codec(crate::Error::UnknownMessageType(
                self.message_type,
            )));
        }
        if self.discriminator == Some(Discriminator::Dtap) {
            return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                context: "FemtoInterfaceMsg",
                reason: "Assignment Complete discriminator must not identify DTAP",
            }));
        }
        Ok(AssignmentCompleteMessage::decode(
            &self.to_bsmap_wire_inner()?,
        )?)
    }

    pub fn from_paging_request(msg: &PagingRequestMessage) -> Result<Self> {
        let mut ies = vec![FemtoIe::tlv(
            IE_MOBILE_IDENTITY,
            &msg.mobile_identity_imsi.encode()?,
        )?];
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(cell_identifier_list) = &msg.cell_identifier_list {
            ies.push(FemtoIe::tlv(
                IE_CELL_IDENTIFIER_LIST,
                &cell_identifier_list.encode()?,
            )?);
        }
        if let Some(slot_cycle_index) = msg.slot_cycle_index {
            ies.push(FemtoIe::tv(
                IE_SLOT_CYCLE_INDEX,
                &[slot_cycle_index.encode()?],
            ));
        }
        if let Some(service_option) = msg.service_option {
            ies.push(FemtoIe::tv(IE_SERVICE_OPTION, &service_option.encode()));
        }
        if let Some(capabilities) = &msg.is2000_mobile_capabilities {
            ies.push(FemtoIe::tlv(
                IE_IS2000_MOBILE_CAPABILITIES,
                capabilities.encode()?,
            )?);
        }
        Ok(Self {
            message_type: MSG_TYPE_PAGING_REQUEST,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("Paging Request".to_string()),
            ies,
        })
    }

    /// Fx2 includes the IEI even for the mandatory positional ADDS User Part.
    pub fn from_adds_deliver(msg: &AddsDeliverMessage) -> Result<Self> {
        let mut ies = vec![FemtoIe::tlv(
            IE_ADDS_USER_PART,
            &msg.adds_user_part.encode_body_public(),
        )?];
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_DELIVER,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("ADDS Deliver".to_string()),
            ies,
        })
    }

    pub fn to_adds_deliver(&self) -> Result<AddsDeliverMessage> {
        self.require_adds_dtap(MSG_TYPE_ADDS_DELIVER)?;
        let user_parts: Vec<_> = self
            .ies
            .iter()
            .filter(|ie| ie.iei == IE_ADDS_USER_PART)
            .collect();
        if user_parts.len() != 1 {
            return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                context: "ADDS Deliver",
                reason: "exactly one ADDS User Part is required",
            }));
        }
        let user_part = &user_parts[0].content;
        if user_part.first() != Some(&IE_ADDS_USER_PART)
            || user_part.get(1).map(|n| *n as usize + 2) != Some(user_part.len())
        {
            return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                context: "ADDS Deliver",
                reason: "invalid ADDS User Part framing",
            }));
        }
        let mut wire = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            DTAP_RESERVED_OCTET,
            MSG_TYPE_ADDS_DELIVER,
        ];
        wire.extend_from_slice(&user_part[1..]);
        for ie in self.ies.iter().filter(|ie| ie.iei != IE_ADDS_USER_PART) {
            if ie.content.first() != Some(&ie.iei) {
                return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                    context: "ADDS Deliver",
                    reason: "IEI missing from information element",
                }));
            }
            wire.extend_from_slice(&ie.content);
        }
        Ok(AddsDeliverMessage::decode(&Self::adds_dtap_frame(wire)?)?)
    }

    pub fn from_adds_deliver_ack(msg: &AddsDeliverAckMessage) -> Result<Self> {
        let mut ies = Vec::new();
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(cause) = msg.cause {
            ies.push(FemtoIe::tv(IE_CAUSE, &cause.encode()));
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_DELIVER_ACK,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("ADDS Deliver Ack".to_string()),
            ies,
        })
    }

    pub fn to_adds_deliver_ack(&self) -> Result<AddsDeliverAckMessage> {
        self.require_adds_dtap(MSG_TYPE_ADDS_DELIVER_ACK)?;
        for ie in &self.ies {
            if ie.content.first() != Some(&ie.iei) {
                return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                    context: "ADDS Deliver Ack",
                    reason: "IEI missing from information element",
                }));
            }
        }
        Ok(AddsDeliverAckMessage::decode(&Self::adds_dtap_frame(
            self.to_layer3_sdu(),
        )?)?)
    }

    fn adds_dtap_frame(sdu: Vec<u8>) -> Result<Vec<u8>> {
        let length = u8::try_from(sdu.len()).map_err(|_| {
            FemtoError::A1Codec(crate::Error::InvalidLength {
                expected: u8::MAX as usize,
                actual: sdu.len(),
            })
        })?;
        let mut wire = vec![DTAP_MESSAGE_DISCRIMINATION, DTAP_LINK_IDENTIFIER, length];
        wire.extend(sdu);
        Ok(wire)
    }

    fn require_adds_dtap(&self, expected: u8) -> Result<()> {
        if self.message_type != expected || self.discriminator != Some(Discriminator::Dtap) {
            return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                context: "FemtoInterfaceMsg",
                reason: "expected ADDS DTAP message",
            }));
        }
        Ok(())
    }

    pub fn from_adds_page(msg: &AddsPageMessage) -> Result<Self> {
        let mut ies = vec![
            FemtoIe::tlv(IE_MOBILE_IDENTITY, &msg.mobile_identity.encode()?)?,
            FemtoIe::tlv(IE_ADDS_USER_PART, &msg.adds_user_part.encode_body_public())?,
        ];
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(slot_cycle_index) = msg.slot_cycle_index {
            ies.push(FemtoIe::tv(
                IE_SLOT_CYCLE_INDEX,
                &[slot_cycle_index.encode()?],
            ));
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_PAGE,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("ADDS Page".to_string()),
            ies,
        })
    }

    pub fn from_adds_page_metadata(msg: &AddsPageMessage) -> Result<Self> {
        let mut ies = vec![
            FemtoIe::tlv(IE_MOBILE_IDENTITY, &msg.mobile_identity.encode()?)?,
            // The Airave reads the Data Burst Type from the ADDS User Part IE
            // and the SMS payload from the separate vnd.3gpp2.sms MIME body.
            // Carry just the burst type here (no data) so the FAP sees a valid
            // burst type (0x03 SMS) without duplicating the payload.
            FemtoIe::tlv(IE_ADDS_USER_PART, &[msg.adds_user_part.burst_type & 0x3f])?,
        ];
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(slot_cycle_index) = msg.slot_cycle_index {
            ies.push(FemtoIe::tv(
                IE_SLOT_CYCLE_INDEX,
                &[slot_cycle_index.encode()?],
            ));
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_PAGE,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("ADDS Page".to_string()),
            ies,
        })
    }

    pub fn from_adds_transfer(msg: &AddsTransferMessage) -> Result<Self> {
        let mut ies = vec![
            FemtoIe::tlv(IE_MOBILE_IDENTITY, &msg.mobile_identity_imsi.encode()?)?,
            FemtoIe::tlv(IE_ADDS_USER_PART, &msg.adds_user_part.encode_body_public())?,
        ];
        if let Some(esn) = &msg.mobile_identity_esn {
            ies.push(FemtoIe::tlv(IE_MOBILE_IDENTITY, &esn.encode()?)?);
        }
        if let Some(cell) = msg.cell_identifier {
            ies.push(FemtoIe::tlv(IE_CELL_IDENTIFIER, &cell.encode()?)?);
        }
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(service_option) = msg.service_option {
            ies.push(FemtoIe::tv(IE_SERVICE_OPTION, &service_option.encode()));
        }
        if let Some(meid) = &msg.mobile_identity_meid {
            ies.push(FemtoIe::tlv(IE_MOBILE_IDENTITY, &meid.encode()?)?);
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_TRANSFER,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("ADDS Transfer".to_string()),
            ies,
        })
    }

    pub fn from_adds_page_ack(msg: &AddsPageAckMessage) -> Result<Self> {
        let mut ies = vec![FemtoIe::tlv(
            IE_MOBILE_IDENTITY,
            &msg.mobile_identity.encode()?,
        )?];
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(esn) = &msg.mobile_identity_esn {
            ies.push(FemtoIe::tlv(IE_MOBILE_IDENTITY, &esn.encode()?)?);
        }
        if let Some(cause) = msg.cause {
            ies.push(FemtoIe::tv(IE_CAUSE, &cause.encode()));
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_PAGE_ACK,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("ADDS Page Ack".to_string()),
            ies,
        })
    }

    pub fn from_adds_transfer_ack(msg: &AddsTransferAckMessage) -> Result<Self> {
        let mut ies = vec![FemtoIe::tlv(
            IE_MOBILE_IDENTITY,
            &msg.mobile_identity_imsi.encode()?,
        )?];
        if let Some(tag) = msg.tag {
            ies.push(FemtoIe::tv(IE_TAG, &tag.encode()));
        }
        if let Some(cause) = msg.cause {
            ies.push(FemtoIe::tv(IE_CAUSE, &cause.encode()));
        }
        Ok(Self {
            message_type: MSG_TYPE_ADDS_TRANSFER_ACK,
            discriminator: Some(Discriminator::Bsmap),
            message_name: Some("ADDS Transfer Ack".to_string()),
            ies,
        })
    }

    pub fn to_bsmap_wire(&self) -> Result<Vec<u8>> {
        if self.discriminator != Some(Discriminator::Bsmap) {
            return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                context: "FemtoInterfaceMsg",
                reason: "message discriminator must identify BSMAP",
            }));
        }
        self.to_bsmap_wire_inner()
    }

    fn to_bsmap_wire_inner(&self) -> Result<Vec<u8>> {
        let message_length = 1usize + self.ies.iter().map(|ie| ie.content.len()).sum::<usize>();
        if message_length > u8::MAX as usize {
            return Err(FemtoError::A1Codec(crate::Error::InvalidLength {
                expected: u8::MAX as usize,
                actual: message_length,
            }));
        }
        let mut wire = Vec::with_capacity(message_length + 2);
        wire.push(BSMAP_MESSAGE_DISCRIMINATION);
        wire.push(message_length as u8);
        wire.push(self.message_type);
        for ie in &self.ies {
            if ie.content.first() != Some(&ie.iei) {
                return Err(FemtoError::A1Codec(crate::Error::InvalidValue {
                    context: "FemtoInterfaceMsg",
                    reason: "BSMAP information element content must include its IEI",
                }));
            }
            wire.extend_from_slice(&ie.content);
        }
        Ok(wire)
    }

    /// Deliberately lenient where [`FemtoInterfaceMsg::to_bsmap_wire`] is strict:
    /// it does not reject an information element whose content does not lead with
    /// its own IEI. A malformed or nonconforming element should still be visible
    /// to an operator rather than collapsing the whole view.
    pub fn to_layer3_sdu(&self) -> Vec<u8> {
        let ie_bytes = || {
            self.ies.iter().flat_map(|ie| {
                // The mandatory ADDS User Part is positional in the DTAP SDU.
                // Fx2 includes its IEI, which must be removed on reconstruction.
                let content = if self.message_type == MSG_TYPE_ADDS_DELIVER
                    && self.discriminator == Some(Discriminator::Dtap)
                    && ie.iei == IE_ADDS_USER_PART
                {
                    ie.content.get(1..).unwrap_or_default()
                } else {
                    &ie.content
                };
                content.iter().copied()
            })
        };
        if matches!(self.discriminator, Some(Discriminator::Bsmap)) {
            // BSMAP framing: discrimination octet, length, message type, IEs.
            // The length field is one octet, so a well-formed BSMAP message can
            // never exceed 255. Saturate rather than refuse to render.
            let length = 1 + self.ies.iter().map(|ie| ie.content.len()).sum::<usize>();
            let mut wire = vec![
                BSMAP_MESSAGE_DISCRIMINATION,
                u8::try_from(length).unwrap_or(u8::MAX),
                self.message_type,
            ];
            wire.extend(ie_bytes());
            return wire;
        }
        let protocol_discriminator = match self.message_type {
            MSG_TYPE_LOCATION_UPDATING_REQUEST | MSG_TYPE_LOCATION_UPDATING_ACCEPT => {
                DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT
            }
            _ => DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
        };
        let mut wire = vec![
            protocol_discriminator,
            DTAP_RESERVED_OCTET,
            self.message_type,
        ];
        wire.extend(ie_bytes());
        wire
    }

    pub fn to_location_updating_request(&self) -> Result<LocationUpdatingRequestMessage> {
        if self.message_type != MSG_TYPE_LOCATION_UPDATING_REQUEST {
            return Err(FemtoError::A1Codec(crate::Error::UnknownMessageType(
                self.message_type,
            )));
        }
        let mut wire = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT,
            DTAP_RESERVED_OCTET,
            MSG_TYPE_LOCATION_UPDATING_REQUEST,
        ];
        let (first, rest) = self
            .ies
            .split_first()
            .ok_or(FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_MOBILE_IDENTITY,
            })?;
        // Positional Mobile Identity: drop the femto IEI byte to recover the
        // wire length-value, keeping the length octet the typed decoder reads.
        wire.extend_from_slice(first.content.get(1..).ok_or(FemtoError::A1Codec(
            crate::Error::Truncated {
                needed: 1,
                actual: first.content.len(),
            },
        ))?);
        for ie in rest {
            wire.extend_from_slice(&ie.content);
        }
        Ok(LocationUpdatingRequestMessage::decode(&wire)?)
    }

    pub fn to_cm_service_request(&self) -> Result<CmServiceRequestMessage> {
        if self.message_type != MSG_TYPE_CM_SERVICE_REQUEST {
            return Err(FemtoError::A1Codec(crate::Error::UnknownMessageType(
                self.message_type,
            )));
        }
        let service_type =
            self.first_ie(IE_CM_SERVICE_TYPE)
                .ok_or(FemtoError::MissingMandatoryIe {
                    message_type: self.message_type,
                    iei: IE_CM_SERVICE_TYPE,
                })?;
        let classmark = self.first_ie(IE_CLASSMARK_INFORMATION_TYPE_2).ok_or(
            FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_CLASSMARK_INFORMATION_TYPE_2,
            },
        )?;
        let imsi = self
            .first_ie(IE_MOBILE_IDENTITY)
            .ok_or(FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_MOBILE_IDENTITY,
            })?;

        let mut wire = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            DTAP_RESERVED_OCTET,
            MSG_TYPE_CM_SERVICE_REQUEST,
        ];
        wire.extend_from_slice(&service_type.content);
        wire.extend_from_slice(&classmark.content);
        wire.extend_from_slice(imsi.content.get(1..).ok_or(FemtoError::A1Codec(
            crate::Error::Truncated {
                needed: 1,
                actual: imsi.content.len(),
            },
        ))?);

        let mut skipped_service_type = false;
        let mut skipped_classmark = false;
        let mut skipped_imsi = false;
        for ie in &self.ies {
            if ie.iei == IE_CM_SERVICE_TYPE && !skipped_service_type {
                skipped_service_type = true;
            } else if ie.iei == IE_CLASSMARK_INFORMATION_TYPE_2 && !skipped_classmark {
                skipped_classmark = true;
            } else if ie.iei == IE_MOBILE_IDENTITY && !skipped_imsi {
                skipped_imsi = true;
            } else {
                wire.extend_from_slice(&ie.content);
            }
        }
        Ok(CmServiceRequestMessage::decode(&wire)?)
    }

    pub fn to_paging_response(&self) -> Result<PagingResponseMessage> {
        if self.message_type != MSG_TYPE_PAGING_RESPONSE {
            return Err(FemtoError::A1Codec(crate::Error::UnknownMessageType(
                self.message_type,
            )));
        }
        let classmark = self.first_ie(IE_CLASSMARK_INFORMATION_TYPE_2).ok_or(
            FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_CLASSMARK_INFORMATION_TYPE_2,
            },
        )?;
        let imsi = self
            .first_ie(IE_MOBILE_IDENTITY)
            .ok_or(FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_MOBILE_IDENTITY,
            })?;
        let mut wire = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            DTAP_RESERVED_OCTET,
            MSG_TYPE_PAGING_RESPONSE,
        ];
        wire.extend_from_slice(&classmark.content);
        wire.extend_from_slice(imsi.content.get(1..).ok_or(FemtoError::A1Codec(
            crate::Error::Truncated {
                needed: 1,
                actual: imsi.content.len(),
            },
        ))?);
        let mut skipped_classmark = false;
        let mut skipped_imsi = false;
        for ie in &self.ies {
            if ie.iei == IE_CLASSMARK_INFORMATION_TYPE_2 && !skipped_classmark {
                skipped_classmark = true;
            } else if ie.iei == IE_MOBILE_IDENTITY && !skipped_imsi {
                skipped_imsi = true;
            } else {
                wire.extend_from_slice(&ie.content);
            }
        }
        Ok(PagingResponseMessage::decode(&wire)?)
    }

    pub fn mobile_identity(&self) -> Result<MobileIdentity> {
        let ie = self
            .first_ie(IE_MOBILE_IDENTITY)
            .ok_or(FemtoError::MissingMandatoryIe {
                message_type: self.message_type,
                iei: IE_MOBILE_IDENTITY,
            })?;
        Ok(MobileIdentity::decode(ie.value(true)?)?)
    }
}

fn parse_identifier(line: &str) -> Result<(u8, Option<Discriminator>, Option<String>)> {
    let (type_field, remainder) = match line.split_once(' ') {
        Some((head, tail)) => (head, Some(tail)),
        None => (line, None),
    };
    let message_type = parse_hex_octet("message-type", type_field)?;
    let Some(remainder) = remainder else {
        return Ok((message_type, None, None));
    };
    let inner = remainder
        .trim()
        .strip_prefix("(=\"")
        .and_then(|s| s.strip_suffix("\")"));
    let Some(inner) = inner else {
        return Ok((message_type, None, None));
    };
    // A leading ASCII digit standing alone or before a space occupies the
    // discriminator slot. It must be `0` or `1`. Anything else is a name.
    let first = inner.chars().next();
    let (discriminator, name_start) = match first {
        Some(c) if c.is_ascii_digit() && (inner.len() == 1 || inner[1..].starts_with(' ')) => {
            (Some(Discriminator::from_char(c)?), 1)
        }
        _ => (None, 0),
    };
    let name = inner[name_start..].trim();
    let message_name = if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    };
    Ok((message_type, discriminator, message_name))
}

fn parse_hex_octet(field: &'static str, value: &str) -> Result<u8> {
    if value.len() != 2 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(FemtoError::BadHex {
            field,
            value: value.to_string(),
        });
    }
    u8::from_str_radix(value, HEX_RADIX).map_err(|_| FemtoError::BadHex {
        field,
        value: value.to_string(),
    })
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_PAD: u8 = b'=';

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(BASE64_ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            BASE64_PAD as char
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[triple as usize & 0x3f] as char
        } else {
            BASE64_PAD as char
        });
    }
    out
}

fn base64_value(byte: u8) -> Result<u32> {
    BASE64_ALPHABET
        .iter()
        .position(|&c| c == byte)
        .map(|p| p as u32)
        .ok_or(FemtoError::BadBase64 {
            detail: "invalid base64 character",
        })
}

fn base64_decode(input: &str) -> Result<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(FemtoError::BadBase64 {
            detail: "length is not a multiple of four",
        });
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|&&b| b == BASE64_PAD).count();
        if pad > 2 {
            return Err(FemtoError::BadBase64 {
                detail: "too much padding",
            });
        }
        // Padding is only valid in the trailing positions of the final chunk.
        if (chunk[0] == BASE64_PAD || chunk[1] == BASE64_PAD)
            || (chunk[2] == BASE64_PAD && chunk[3] != BASE64_PAD)
        {
            return Err(FemtoError::BadBase64 {
                detail: "misplaced padding",
            });
        }
        let v0 = base64_value(chunk[0])?;
        let v1 = base64_value(chunk[1])?;
        let v2 = if chunk[2] == BASE64_PAD {
            0
        } else {
            base64_value(chunk[2])?
        };
        let v3 = if chunk[3] == BASE64_PAD {
            0
        } else {
            base64_value(chunk[3])?
        };
        let triple = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
        out.push((triple >> 16) as u8);
        if pad < 2 {
            out.push((triple >> 8) as u8);
        }
        if pad < 1 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimePart {
    pub content_type: String,
    pub transfer_encoding: Option<String>,
    pub body: Vec<u8>,
}

impl MimePart {
    pub fn text(content_type: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            content_type: content_type.into(),
            transfer_encoding: None,
            body: body.into(),
        }
    }

    pub fn binary(content_type: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            content_type: content_type.into(),
            transfer_encoding: Some("binary".to_string()),
            body: body.into(),
        }
    }
}

const MULTIPART_DASHES: &str = "--";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Multipart {
    pub boundary: String,
    pub parts: Vec<MimePart>,
}

impl Multipart {
    pub fn new(boundary: impl Into<String>, parts: Vec<MimePart>) -> Self {
        Self {
            boundary: boundary.into(),
            parts,
        }
    }

    pub fn part_by_content_type(&self, content_type: &str) -> Option<&MimePart> {
        self.parts
            .iter()
            .find(|part| part.content_type == content_type)
    }

    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for part in &self.parts {
            out.extend_from_slice(MULTIPART_DASHES.as_bytes());
            out.extend_from_slice(self.boundary.as_bytes());
            out.extend_from_slice(CRLF.as_bytes());
            out.extend_from_slice(format!("Content-Type: {}{CRLF}", part.content_type).as_bytes());
            if let Some(encoding) = &part.transfer_encoding {
                out.extend_from_slice(
                    format!("Content-Transfer-Encoding: {encoding}{CRLF}").as_bytes(),
                );
            }
            out.extend_from_slice(CRLF.as_bytes());
            out.extend_from_slice(&part.body);
            out.extend_from_slice(CRLF.as_bytes());
        }
        out.extend_from_slice(MULTIPART_DASHES.as_bytes());
        out.extend_from_slice(self.boundary.as_bytes());
        out.extend_from_slice(MULTIPART_DASHES.as_bytes());
        out.extend_from_slice(CRLF.as_bytes());
        out
    }

    pub fn parse(boundary: &str, body: &[u8]) -> Result<Self> {
        let delimiter = format!("{MULTIPART_DASHES}{boundary}");
        let sections = split_on(body, delimiter.as_bytes());
        let mut parts = Vec::new();
        for section in sections {
            if section.starts_with(MULTIPART_DASHES.as_bytes()) || is_blank(section) {
                continue;
            }
            parts.push(parse_part(section)?);
        }
        Ok(Self {
            boundary: boundary.to_string(),
            parts,
        })
    }
}

fn parse_part(section: &[u8]) -> Result<MimePart> {
    // A part is `CRLF header-lines CRLF CRLF body CRLF`. Drop exactly the one
    // CRLF that follows the boundary and the one CRLF before the next boundary,
    // leaving any CRLF that is genuinely part of the body.
    let trimmed = strip_one_crlf_suffix(strip_one_crlf_prefix(section));
    let text_end = find_subslice(trimmed, b"\r\n\r\n").ok_or(FemtoError::MalformedMultipart {
        detail: "part is missing a header/body separator",
    })?;
    let header_text =
        std::str::from_utf8(&trimmed[..text_end]).map_err(|_| FemtoError::MalformedMultipart {
            detail: "part headers are not valid UTF-8",
        })?;
    let mut content_type = None;
    let mut transfer_encoding = None;
    for header in header_text.split(CRLF).filter(|line| !line.is_empty()) {
        let (name, value) = header
            .split_once(':')
            .ok_or(FemtoError::MalformedMultipart {
                detail: "part header is missing a colon",
            })?;
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value);
        } else if name.eq_ignore_ascii_case("content-transfer-encoding") {
            transfer_encoding = Some(value);
        }
    }
    let content_type = content_type.ok_or(FemtoError::MalformedMultipart {
        detail: "part is missing a Content-Type header",
    })?;
    let body = trimmed[text_end + 4..].to_vec();
    Ok(MimePart {
        content_type,
        transfer_encoding,
        body,
    })
}

fn split_on<'a>(input: &'a [u8], delimiter: &[u8]) -> Vec<&'a [u8]> {
    let mut sections = Vec::new();
    let mut start = 0;
    let mut cursor = 0;
    while cursor + delimiter.len() <= input.len() {
        if &input[cursor..cursor + delimiter.len()] == delimiter {
            sections.push(&input[start..cursor]);
            cursor += delimiter.len();
            start = cursor;
        } else {
            cursor += 1;
        }
    }
    sections.push(&input[start..]);
    sections
}

fn find_subslice(input: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || input.len() < needle.len() {
        return None;
    }
    (0..=input.len() - needle.len()).find(|&i| &input[i..i + needle.len()] == needle)
}

fn strip_one_crlf_prefix(input: &[u8]) -> &[u8] {
    input.strip_prefix(b"\r\n").unwrap_or(input)
}

fn strip_one_crlf_suffix(input: &[u8]) -> &[u8] {
    input.strip_suffix(b"\r\n").unwrap_or(input)
}

fn is_blank(input: &[u8]) -> bool {
    input.iter().all(|&b| b == b'\r' || b == b'\n')
}

#[cfg(test)]
mod otasp_transport_tests {
    use super::*;
    use crate::{AddsUserPart, Cause, Tag};

    #[test]
    fn otasp_deliver_round_trips_payload_and_full_width_tag() {
        for length in [0, 1, 32, 244, 245] {
            let deliver = AddsDeliverMessage {
                adds_user_part: AddsUserPart {
                    burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                    data: (0..length).map(|n| n as u8).collect(),
                },
                tag: Some(Tag(0x80000001)),
            };
            let femto = FemtoInterfaceMsg::from_adds_deliver(&deliver).unwrap();
            assert!(
                femto
                    .encode()
                    .starts_with(b"53 (=\"0 ADDS Deliver\")\r\n3D=")
            );
            let parsed = FemtoInterfaceMsg::parse(&femto.encode()).unwrap();
            assert_eq!(parsed.to_layer3_sdu(), deliver.encode().unwrap()[3..]);
            assert_eq!(parsed.to_adds_deliver().unwrap(), deliver);
        }
    }

    #[test]
    fn otasp_deliver_rejects_wrong_discriminator_and_invalid_mandatory_ie() {
        let deliver = AddsDeliverMessage {
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6],
            },
            tag: None,
        };
        let mut femto = FemtoInterfaceMsg::from_adds_deliver(&deliver).unwrap();
        femto.discriminator = Some(Discriminator::Bsmap);
        assert!(femto.to_adds_deliver().is_err());
        femto.discriminator = Some(Discriminator::Dtap);
        femto.ies[0].content[1] = 99;
        assert!(femto.to_adds_deliver().is_err());
        femto = FemtoInterfaceMsg::from_adds_deliver(&deliver).unwrap();
        femto.ies.push(femto.ies[0].clone());
        assert!(femto.to_adds_deliver().is_err());
        femto.ies.clear();
        assert!(femto.to_adds_deliver().is_err());
    }

    #[test]
    fn otasp_delivery_ack_preserves_success_and_failure() {
        for cause in [None, Some(Cause(0x20))] {
            let ack = AddsDeliverAckMessage {
                tag: Some(Tag(0x80000001)),
                cause,
            };
            let femto = FemtoInterfaceMsg::from_adds_deliver_ack(&ack).unwrap();
            let parsed = FemtoInterfaceMsg::parse(&femto.encode()).unwrap();
            assert_eq!(parsed.to_adds_deliver_ack().unwrap(), ack);
        }
    }
}
