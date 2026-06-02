//! Exact typed models for A1/IOS BSMAP messages.

use crate::{Error, Result};

const BSMAP_MESSAGE_DISCRIMINATION: u8 = 0x00;
const A1_DTAP_MESSAGE_DISCRIMINATION: u8 = 0x01;
const COMPLETE_LAYER3_INFORMATION: u8 = 0x57;
const USER_ZONE_UPDATE: u8 = 0x04;
const PAGING_REQUEST: u8 = 0x52;
const ASSIGNMENT_REQUEST: u8 = 0x01;
const ASSIGNMENT_COMPLETE: u8 = 0x02;
const ASSIGNMENT_FAILURE: u8 = 0x03;
const CLEAR_COMMAND: u8 = 0x20;
const CLEAR_COMPLETE: u8 = 0x21;
const CLEAR_REQUEST: u8 = 0x22;
const BS_SERVICE_REQUEST: u8 = 0x09;
const BS_SERVICE_RESPONSE: u8 = 0x0a;
const HANDOFF_REQUEST: u8 = 0x10;
const HANDOFF_REQUIRED: u8 = 0x11;
const HANDOFF_REQUEST_ACKNOWLEDGE: u8 = 0x12;
const HANDOFF_COMMAND: u8 = 0x13;
const HANDOFF_COMPLETE: u8 = 0x14;
const HANDOFF_COMMENCED: u8 = 0x15;
const HANDOFF_FAILURE: u8 = 0x16;
const HANDOFF_PERFORMED: u8 = 0x17;
const HANDOFF_REQUIRED_REJECT: u8 = 0x1a;
const CONNECT: u8 = 0x07;
const PROGRESS: u8 = 0x03;
const ALERT_WITH_INFORMATION: u8 = 0x26;
const PARAMETER_UPDATE_CONFIRM: u8 = 0x2b;
const PARAMETER_UPDATE_REQUEST: u8 = 0x2c;
const PRIVACY_MODE_COMMAND: u8 = 0x53;
const PRIVACY_MODE_COMPLETE: u8 = 0x55;
const CM_SERVICE_REQUEST: u8 = 0x24;
const PAGING_RESPONSE: u8 = 0x27;
const LOCATION_UPDATING_REQUEST: u8 = 0x08;
const LOCATION_UPDATING_ACCEPT: u8 = 0x02;
const LOCATION_UPDATING_REJECT: u8 = 0x04;
const AUTHENTICATION_REQUEST: u8 = 0x45;
const AUTHENTICATION_RESPONSE: u8 = 0x46;
const SSD_UPDATE_REQUEST: u8 = 0x47;
const BASE_STATION_CHALLENGE: u8 = 0x48;
const BASE_STATION_CHALLENGE_RESPONSE: u8 = 0x49;
const SSD_UPDATE_RESPONSE: u8 = 0x4a;
const DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING: u8 = 0x03;
const DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT: u8 = 0x05;

const IE_SERVICE_OPTION: u8 = 0x03;
const IE_CAUSE: u8 = 0x04;
const IE_CELL_IDENTIFIER: u8 = 0x05;
const IE_PRIORITY: u8 = 0x06;
const IE_QUALITY_OF_SERVICE_PARAMETERS: u8 = 0x07;
const IE_CAUSE_LAYER_3: u8 = 0x08;
const IE_IS2000_CHANNEL_IDENTITY: u8 = 0x09;
const IE_ENCRYPTION_INFORMATION: u8 = 0x0a;
const IE_CHANNEL_TYPE: u8 = 0x0b;
const IE_CDMA_SERVING_ONE_WAY_DELAY: u8 = 0x0c;
const IE_MOBILE_IDENTITY: u8 = 0x0d;
const IE_IS2000_SERVICE_CONFIGURATION_RECORD: u8 = 0x0e;
const IE_IS2000_NON_NEGOTIABLE_SERVICE_CONFIGURATION_RECORD: u8 = 0x0f;
const IE_EXTENDED_HANDOFF_DIRECTION_PARAMETERS: u8 = 0x10;
const IE_CIRCUIT_IDENTITY_CODE: u8 = 0x01;
const IE_IS2000_MOBILE_CAPABILITIES: u8 = 0x11;
const IE_RESPONSE_REQUEST: u8 = 0x1b;
const IE_CIRCUIT_IDENTITY_CODE_EXTENSION: u8 = 0x24;
const IE_DOWNLINK_RADIO_ENVIRONMENT: u8 = 0x29;
const IE_PDSN_IP_ADDRESS: u8 = 0x14;
const IE_PROTOCOL_TYPE: u8 = 0x18;
const IE_HARD_HANDOFF_PARAMETERS: u8 = 0x16;
const IE_RF_CHANNEL_IDENTITY: u8 = 0x21;
const IE_IS95_CHANNEL_IDENTITY: u8 = 0x22;
const IE_HANDOFF_POWER_LEVEL: u8 = 0x26;
const IE_SID: u8 = 0x32;
const IE_IS95_MS_MEASURED_CHANNEL_IDENTITY: u8 = 0x64;
const IE_MS_INFORMATION_RECORDS: u8 = 0x15;
const IE_LAYER_3_INFORMATION: u8 = 0x17;
const IE_CELL_IDENTIFIER_LIST: u8 = 0x1a;
const IE_TAG: u8 = 0x33;
const IE_SIGNAL: u8 = 0x34;
const IE_SLOT_CYCLE_INDEX: u8 = 0x35;
const IE_PACA_TIMESTAMP: u8 = 0x4e;
const IE_CALLED_PARTY_BCD_NUMBER: u8 = 0x5e;
const IE_CALLED_PARTY_ASCII_NUMBER_ALT: u8 = 0x5b;
const IE_AUTHENTICATION_CONFIRMATION_PARAMETER: u8 = 0x28;
const IE_AUTHENTICATION_PARAMETER_COUNT: u8 = 0x40;
const IE_AUTHENTICATION_CHALLENGE_PARAMETER: u8 = 0x41;
const IE_AUTHENTICATION_RESPONSE_PARAMETER: u8 = 0x42;
const IE_VOICE_PRIVACY_REQUEST: u8 = 0xa1;
const IE_RADIO_ENVIRONMENT_AND_RESOURCES: u8 = 0x1d;
const IE_AUTHENTICATION_EVENT: u8 = 0x4a;
const IE_AUTHENTICATION_DATA: u8 = 0x59;
const IE_PACA_REORIGINATION_INDICATOR: u8 = 0x60;
const IE_USER_ZONE_ID: u8 = 0x02;
const IE_CHANNEL_NUMBER: u8 = 0x23;
const IE_POWER_DOWN_INDICATOR: u8 = 0xa2;
const IE_LOCATION_AREA_IDENTIFICATION: u8 = 0x13;
// ADDS message type codes (A.S0001 §6.1.7) — used in BSMAP/DTAP inner payload
const ADDS_PAGE: u8 = 0x65;
const ADDS_PAGE_ACK: u8 = 0x66;
const ADDS_TRANSFER: u8 = 0x67;
const ADDS_TRANSFER_ACK: u8 = 0x68;
const ADDS_DELIVER_DTAP: u8 = 0x53; // DTAP msg type; same value as BSMAP Privacy Mode Command
const ADDS_DELIVER_ACK_DTAP: u8 = 0x54;
const IE_ADDS_USER_PART: u8 = 0x3d;
const IE_A2P_BEARER_SESSION_PARAMS: u8 = 0x45;
const IE_A2P_BEARER_FORMAT_PARAMS: u8 = 0x46;

/// Exact A1 cell identifier using discriminator `0x02`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellId {
    pub cell: u16,
    pub sector: u8,
}

impl CellId {
    /// Encodes the cell identifier payload.
    pub fn encode(self) -> Result<[u8; 3]> {
        if !(1..=0x0fff).contains(&self.cell) || self.sector > 0x0f {
            return Err(Error::InvalidValue {
                context: "Cell Identifier",
                reason: "cell must be 1..=0x0fff and sector must be 0..=0x0f",
            });
        }
        Ok([
            0x02,
            (self.cell >> 4) as u8,
            (((self.cell & 0x000f) as u8) << 4) | (self.sector & 0x0f),
        ])
    }

    /// Decodes the cell identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 3 {
            return Err(Error::InvalidLength {
                expected: 3,
                actual: input.len(),
            });
        }
        if input[0] != 0x02 {
            return Err(Error::ReservedValue {
                context: "Cell Identifier discriminator",
                value: input[0],
            });
        }
        Ok(Self {
            cell: ((input[1] as u16) << 4) | ((input[2] as u16) >> 4),
            sector: input[2] & 0x0f,
        })
    }
}

/// Exact A1 mobile identity payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIdentity {
    Imsi(String),
    Esn(u32),
    /// MEID as 14 hex digits packed into 7 bytes (A.S0014-D §4.2.13).
    Meid([u8; 7]),
}

impl MobileIdentity {
    /// Encodes the mobile identity payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Imsi(imsi) => encode_imsi(imsi),
            Self::Esn(esn) => {
                let mut out = vec![0x05];
                out.extend_from_slice(&esn.to_be_bytes());
                Ok(out)
            }
            Self::Meid(digits) => {
                // A.S0014-D §4.2.13: 14 hex digits packed odd-in-high, even-in-low,
                // with Fill=F in the high nibble of the final octet.
                let d = digits;
                Ok(vec![
                    (d[0] & 0xf0) | 0x01,
                    (d[1] & 0xf0) | (d[0] & 0x0f),
                    (d[2] & 0xf0) | (d[1] & 0x0f),
                    (d[3] & 0xf0) | (d[2] & 0x0f),
                    (d[4] & 0xf0) | (d[3] & 0x0f),
                    (d[5] & 0xf0) | (d[4] & 0x0f),
                    (d[6] & 0xf0) | (d[5] & 0x0f),
                    0xf0 | (d[6] & 0x0f),
                ])
            }
        }
    }

    /// Decodes the mobile identity payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&first, rest)) = input.split_first() else {
            return Err(Error::Truncated {
                needed: 1,
                actual: 0,
            });
        };
        match first & 0x07 {
            0x06 => decode_imsi(first, rest),
            0x05 => {
                if input.len() != 5 {
                    return Err(Error::InvalidLength {
                        expected: 5,
                        actual: input.len(),
                    });
                }
                Ok(Self::Esn(u32::from_be_bytes([
                    input[1], input[2], input[3], input[4],
                ])))
            }
            0x01 => {
                if input.len() != 8 {
                    return Err(Error::InvalidLength {
                        expected: 8,
                        actual: input.len(),
                    });
                }
                let _ = rest;
                Ok(Self::Meid([
                    (input[0] & 0xf0) | (input[1] & 0x0f),
                    (input[1] & 0xf0) | (input[2] & 0x0f),
                    (input[2] & 0xf0) | (input[3] & 0x0f),
                    (input[3] & 0xf0) | (input[4] & 0x0f),
                    (input[4] & 0xf0) | (input[5] & 0x0f),
                    (input[5] & 0xf0) | (input[6] & 0x0f),
                    (input[6] & 0xf0) | (input[7] & 0x0f),
                ]))
            }
            other => Err(Error::ReservedValue {
                context: "Mobile Identity type",
                value: other,
            }),
        }
    }
}

/// Exact A1 layer 3 information payload carried inside `Complete Layer 3 Information`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer3Information(pub Vec<u8>);

impl Layer3Information {
    /// Encodes the layer 3 information payload bytes.
    pub fn encode(&self) -> &[u8] {
        &self.0
    }

    /// Decodes the layer 3 information payload bytes.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidValue {
                context: "Layer 3 Information",
                reason: "payload must not be empty",
            });
        }
        Ok(Self(input.to_vec()))
    }

    /// Builds a `Layer 3 Information` payload from a typed `CM Service Request` DTAP message.
    pub fn from_cm_service_request(message: &CmServiceRequestMessage) -> Result<Self> {
        Ok(Self(message.encode()?))
    }

    /// Builds a `Layer 3 Information` payload from a typed `Paging Response` DTAP message.
    pub fn from_paging_response(message: &PagingResponseMessage) -> Result<Self> {
        Ok(Self(message.encode()?))
    }

    /// Builds a `Layer 3 Information` payload from a typed `Location Updating Request` DTAP message.
    pub fn from_location_updating_request(
        message: &LocationUpdatingRequestMessage,
    ) -> Result<Self> {
        Ok(Self(message.encode()?))
    }

    /// Attempts to decode the payload as a `CM Service Request` DTAP message.
    pub fn decode_cm_service_request(&self) -> Result<CmServiceRequestMessage> {
        CmServiceRequestMessage::decode(&self.0)
    }

    /// Attempts to decode the payload as a `Paging Response` DTAP message.
    pub fn decode_paging_response(&self) -> Result<PagingResponseMessage> {
        PagingResponseMessage::decode(&self.0)
    }

    /// Attempts to decode the payload as a `Location Updating Request` DTAP message.
    pub fn decode_location_updating_request(&self) -> Result<LocationUpdatingRequestMessage> {
        LocationUpdatingRequestMessage::decode(&self.0)
    }
}

/// Exact raw payload for `Classmark Information Type 2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassmarkInformationType2(pub Vec<u8>);

impl ClassmarkInformationType2 {
    /// Encodes the classmark payload.
    pub fn encode(&self) -> Result<&[u8]> {
        if self.0.len() < 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: self.0.len(),
            });
        }
        Ok(&self.0)
    }

    /// Decodes the classmark payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self(input.to_vec()))
    }
}

/// Exact raw payload for `Called Party BCD Number`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalledPartyBcdNumber(pub Vec<u8>);

impl CalledPartyBcdNumber {
    /// Encodes the called-party BCD payload.
    pub fn encode(&self) -> Result<&[u8]> {
        if self.0.len() < 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: self.0.len(),
            });
        }
        Ok(&self.0)
    }

    /// Decodes the called-party BCD payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(input.to_vec()))
    }
}

/// Exact raw payload for `Authentication Response Parameter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationResponseParameter(pub [u8; 4]);

impl AuthenticationResponseParameter {
    /// Encodes the authentication-response payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0
    }

    /// Decodes the authentication-response payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3]]))
    }
}

/// Exact one-octet `RANDC` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationConfirmationParameter(pub u8);

/// Exact one-octet `COUNT` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationParameterCount(pub u8);

/// Exact raw payload for `Authentication Challenge Parameter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationChallengeParameter(pub [u8; 5]);

impl AuthenticationChallengeParameter {
    /// Encodes the authentication-challenge payload.
    pub const fn encode(self) -> [u8; 5] {
        self.0
    }

    /// Decodes the authentication-challenge payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                expected: 5,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3], input[4]]))
    }
}

/// Exact raw payload for `Authentication Challenge Parameter (RANDSSD)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SsdUpdateChallengeParameter(pub [u8; 8]);

impl SsdUpdateChallengeParameter {
    /// Encodes the SSD-update challenge payload.
    pub const fn encode(self) -> [u8; 8] {
        self.0
    }

    /// Decodes the SSD-update challenge payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 8 {
            return Err(Error::InvalidLength {
                expected: 8,
                actual: input.len(),
            });
        }
        Ok(Self([
            input[0], input[1], input[2], input[3], input[4], input[5], input[6], input[7],
        ]))
    }
}

/// Exact one-octet `Radio Environment and Resources` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioEnvironmentAndResources {
    pub include_priority: bool,
    pub forward: u8,
    pub reverse: u8,
    pub allocated: bool,
    pub available: bool,
}

impl RadioEnvironmentAndResources {
    /// Encodes the radio-environment-and-resources payload.
    pub fn encode(self) -> Result<u8> {
        if self.forward > 0x03 || self.reverse > 0x03 {
            return Err(Error::InvalidValue {
                context: "Radio Environment and Resources",
                reason: "forward/reverse values must fit in 2 bits",
            });
        }
        Ok(((self.include_priority as u8) << 6)
            | ((self.forward & 0x03) << 4)
            | ((self.reverse & 0x03) << 2)
            | ((self.allocated as u8) << 1)
            | (self.available as u8))
    }

    /// Decodes the radio-environment-and-resources payload.
    pub fn decode(input: u8) -> Self {
        Self {
            include_priority: input & 0x40 != 0,
            forward: (input >> 4) & 0x03,
            reverse: (input >> 2) & 0x03,
            allocated: input & 0x02 != 0,
            available: input & 0x01 != 0,
        }
    }
}

/// Exact fixed-width user-zone identifier payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserZoneId(pub u16);

impl UserZoneId {
    /// Encodes the user-zone identifier payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the user-zone identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact fixed-width location-area-identification payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocationAreaIdentification {
    pub mcc_digit_1: u8,
    pub mcc_digit_2: u8,
    pub mcc_digit_3: u8,
    pub mnc_digit_1: u8,
    pub mnc_digit_2: u8,
    pub mnc_digit_3: u8,
    pub lac: u16,
}

impl LocationAreaIdentification {
    /// Encodes the location-area-identification payload.
    pub fn encode(self) -> Result<[u8; 5]> {
        let digits = [
            self.mcc_digit_1,
            self.mcc_digit_2,
            self.mcc_digit_3,
            self.mnc_digit_1,
            self.mnc_digit_2,
            self.mnc_digit_3,
        ];
        if digits.iter().any(|digit| *digit > 0x0f) {
            return Err(Error::InvalidValue {
                context: "Location Area Identification",
                reason: "MCC and MNC digits must fit in a BCD nibble",
            });
        }
        if self.lac == 0 {
            return Err(Error::InvalidValue {
                context: "Location Area Identification",
                reason: "LAC must be in the range 0x0001..=0xffff",
            });
        }
        Ok([
            (self.mcc_digit_2 << 4) | self.mcc_digit_1,
            (self.mnc_digit_3 << 4) | self.mcc_digit_3,
            (self.mnc_digit_2 << 4) | self.mnc_digit_1,
            (self.lac >> 8) as u8,
            self.lac as u8,
        ])
    }

    /// Decodes the location-area-identification payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                expected: 5,
                actual: input.len(),
            });
        }
        let lac = u16::from_be_bytes([input[3], input[4]]);
        if lac == 0 {
            return Err(Error::InvalidValue {
                context: "Location Area Identification",
                reason: "LAC must be in the range 0x0001..=0xffff",
            });
        }
        Ok(Self {
            mcc_digit_1: input[0] & 0x0f,
            mcc_digit_2: input[0] >> 4,
            mcc_digit_3: input[1] & 0x0f,
            mnc_digit_1: input[2] & 0x0f,
            mnc_digit_2: input[2] >> 4,
            mnc_digit_3: input[1] >> 4,
            lac,
        })
    }
}

/// Exact one-octet `Reject Cause` payload for `Location Updating Reject`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectCause(pub u8);

impl RejectCause {
    /// Encodes the reject-cause payload.
    pub fn encode(self) -> Result<u8> {
        match self.0 {
            0x03 | 0x0b | 0x51 | 0x56 => Ok(self.0),
            _ => Err(Error::InvalidValue {
                context: "Reject Cause",
                reason: "value is not allowed for Location Updating Reject",
            }),
        }
    }

    /// Decodes the reject-cause payload.
    pub fn decode(input: u8) -> Result<Self> {
        Self(input).encode()?;
        Ok(Self(input))
    }
}

/// Exact one-octet `Registration Type` payload for `Location Updating Request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationType(pub u8);

impl RegistrationType {
    /// Encodes the registration-type payload.
    pub fn encode(self) -> Result<u8> {
        match self.0 {
            0x00 | 0x01 | 0x02 | 0x03 | 0x04 | 0x06 => Ok(self.0),
            _ => Err(Error::InvalidValue {
                context: "Registration Type",
                reason: "value is not allowed for Location Updating Request",
            }),
        }
    }

    /// Decodes the registration-type payload.
    pub fn decode(input: u8) -> Result<Self> {
        Self(input).encode()?;
        Ok(Self(input))
    }
}

/// Exact fixed-width authentication-event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationEvent(pub u8);

/// Exact raw payload for `Authentication Data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationData(pub [u8; 3]);

impl AuthenticationData {
    /// Encodes the authentication-data payload.
    pub const fn encode(self) -> [u8; 3] {
        self.0
    }

    /// Decodes the authentication-data payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 3 {
            return Err(Error::InvalidLength {
                expected: 3,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2]]))
    }
}

/// Service types carried by the type-1 `CM Service Type` information element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CmServiceType {
    MobileOriginatingCallEstablishment = 0x01,
    EmergencyCallEstablishment = 0x02,
    ShortMessageTransfer = 0x04,
    SupplementaryServiceActivation = 0x08,
}

impl CmServiceType {
    /// Encodes the type-1 CM Service Type octet.
    pub const fn encode(self) -> u8 {
        0x90 | (self as u8)
    }

    /// Decodes the type-1 CM Service Type octet.
    pub fn decode(input: u8) -> Result<Self> {
        if input >> 4 != 0x09 {
            return Err(Error::ReservedValue {
                context: "CM Service Type IEI",
                value: input >> 4,
            });
        }
        match input & 0x0f {
            0x01 => Ok(Self::MobileOriginatingCallEstablishment),
            0x02 => Ok(Self::EmergencyCallEstablishment),
            0x04 => Ok(Self::ShortMessageTransfer),
            0x08 => Ok(Self::SupplementaryServiceActivation),
            other => Err(Error::ReservedValue {
                context: "CM Service Type",
                value: other,
            }),
        }
    }
}

/// Exact DTAP `CM Service Request` payload carried inside `Layer 3 Information`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmServiceRequestMessage {
    pub cm_service_type: CmServiceType,
    pub classmark_information_type_2: ClassmarkInformationType2,
    pub mobile_identity_imsi: MobileIdentity,
    pub called_party_bcd_number: Option<CalledPartyBcdNumber>,
    pub tag: Option<Tag>,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub slot_cycle_index: Option<SlotCycleIndex>,
    pub authentication_response_parameter: Option<AuthenticationResponseParameter>,
    pub authentication_confirmation_parameter: Option<AuthenticationConfirmationParameter>,
    pub authentication_parameter_count: Option<AuthenticationParameterCount>,
    pub authentication_challenge_parameter: Option<AuthenticationChallengeParameter>,
    pub service_option: Option<ServiceOption>,
    pub voice_privacy_request: bool,
    pub radio_environment_and_resources: Option<RadioEnvironmentAndResources>,
    pub called_party_ascii_number: Option<CallingPartyAsciiNumber>,
    pub circuit_identity_code: Option<CircuitIdentityCode>,
    pub authentication_event: Option<AuthenticationEvent>,
    pub authentication_data: Option<AuthenticationData>,
    pub paca_reorigination_indicator: bool,
    pub user_zone_id: Option<UserZoneId>,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
    pub cdma_serving_one_way_delay: Option<HandoffCdmaServingOneWayDelay>,
}

impl CmServiceRequestMessage {
    /// Encodes the DTAP message body including protocol discriminator and message type.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            0x00,
            CM_SERVICE_REQUEST,
            self.cm_service_type.encode(),
        ];
        push_l3_tlv(&mut out, 0x12, self.classmark_information_type_2.encode()?)?;
        let mobile_identity = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "CM Service Request",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        push_l3_lv(&mut out, &mobile_identity)?;
        if let Some(called_party_bcd_number) = &self.called_party_bcd_number {
            push_l3_tlv(
                &mut out,
                IE_CALLED_PARTY_BCD_NUMBER,
                called_party_bcd_number.encode()?,
            )?;
        }
        if let Some(tag) = self.tag {
            push_fixed(&mut out, IE_TAG, &tag.encode());
        }
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "CM Service Request",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_l3_tlv(&mut out, IE_MOBILE_IDENTITY, &esn)?;
        }
        if let Some(slot_cycle_index) = self.slot_cycle_index {
            push_fixed(&mut out, IE_SLOT_CYCLE_INDEX, &[slot_cycle_index.encode()?]);
        }
        if let Some(authentication_response_parameter) = self.authentication_response_parameter {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_RESPONSE_PARAMETER,
                &authentication_response_parameter.encode(),
            )?;
        }
        if let Some(authentication_confirmation_parameter) =
            self.authentication_confirmation_parameter
        {
            push_fixed(
                &mut out,
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                &[authentication_confirmation_parameter.0],
            );
        }
        if let Some(authentication_parameter_count) = self.authentication_parameter_count {
            push_fixed(
                &mut out,
                IE_AUTHENTICATION_PARAMETER_COUNT,
                &[authentication_parameter_count.0 & 0x3f],
            );
        }
        if let Some(authentication_challenge_parameter) = self.authentication_challenge_parameter {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                &authentication_challenge_parameter.encode(),
            )?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut out, IE_SERVICE_OPTION, &service_option.encode());
        }
        if self.voice_privacy_request {
            out.push(IE_VOICE_PRIVACY_REQUEST);
        }
        if let Some(radio_environment_and_resources) = self.radio_environment_and_resources {
            push_fixed(
                &mut out,
                IE_RADIO_ENVIRONMENT_AND_RESOURCES,
                &[radio_environment_and_resources.encode()?],
            );
        }
        if let Some(called_party_ascii_number) = &self.called_party_ascii_number {
            push_l3_tlv(
                &mut out,
                IE_CALLED_PARTY_ASCII_NUMBER_ALT,
                called_party_ascii_number.encode()?,
            )?;
        }
        if let Some(circuit_identity_code) = self.circuit_identity_code {
            push_fixed(
                &mut out,
                IE_CIRCUIT_IDENTITY_CODE,
                &circuit_identity_code.encode()?,
            );
        }
        if let Some(authentication_event) = self.authentication_event {
            push_l3_tlv(&mut out, IE_AUTHENTICATION_EVENT, &[authentication_event.0])?;
        }
        if let Some(authentication_data) = self.authentication_data {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_DATA,
                &authentication_data.encode(),
            )?;
        }
        if self.paca_reorigination_indicator {
            push_l3_tlv(&mut out, IE_PACA_REORIGINATION_INDICATOR, &[0x01])?;
        }
        if let Some(user_zone_id) = self.user_zone_id {
            push_fixed(&mut out, IE_USER_ZONE_ID, &user_zone_id.encode());
        }
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_l3_tlv(
                &mut out,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        if let Some(cdma_serving_one_way_delay) = self.cdma_serving_one_way_delay {
            push_l3_tlv(
                &mut out,
                IE_CDMA_SERVING_ONE_WAY_DELAY,
                &cdma_serving_one_way_delay.encode()?,
            )?;
        }
        Ok(out)
    }

    /// Decodes the DTAP message body including protocol discriminator and message type.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let rest = parse_dtap(
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            CM_SERVICE_REQUEST,
            input,
        )?;
        let mut offset = 0;
        ensure_remaining(rest, offset, 1)?;
        let cm_service_type = CmServiceType::decode(rest[offset])?;
        offset += 1;
        let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
        if rest[offset] != 0x12 {
            return Err(Error::UnknownInformationElement(rest[offset]));
        }
        let classmark_information_type_2 = ClassmarkInformationType2::decode(payload)?;
        offset += consumed;
        let (_, imsi_payload, consumed) = decode_lv(&rest[offset..])?;
        let mobile_identity_imsi = MobileIdentity::decode(imsi_payload)?;
        if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "CM Service Request",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        offset += consumed;
        let mut called_party_bcd_number = None;
        let mut tag = None;
        let mut mobile_identity_esn = None;
        let mut slot_cycle_index = None;
        let mut authentication_response_parameter = None;
        let mut authentication_confirmation_parameter = None;
        let mut authentication_parameter_count = None;
        let mut authentication_challenge_parameter = None;
        let mut service_option = None;
        let mut voice_privacy_request = false;
        let mut radio_environment_and_resources = None;
        let mut called_party_ascii_number = None;
        let mut circuit_identity_code = None;
        let mut authentication_event = None;
        let mut authentication_data = None;
        let mut paca_reorigination_indicator = false;
        let mut user_zone_id = None;
        let mut is2000_mobile_capabilities = None;
        let mut cdma_serving_one_way_delay = None;
        while offset < rest.len() {
            match rest[offset] {
                IE_CALLED_PARTY_BCD_NUMBER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut called_party_bcd_number,
                        CalledPartyBcdNumber::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_CALLED_PARTY_BCD_NUMBER,
                    )?;
                    offset += consumed;
                }
                IE_TAG => {
                    set_once(
                        &mut tag,
                        Tag::decode(take_fixed(&rest[offset..], 4)?)?,
                        CM_SERVICE_REQUEST,
                        IE_TAG,
                    )?;
                    offset += 5;
                }
                IE_MOBILE_IDENTITY => {
                    let (id, payload, consumed) = decode_tlv(&rest[offset..])?;
                    debug_assert_eq!(id, IE_MOBILE_IDENTITY);
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "CM Service Request",
                            reason: "ESN field must contain ESN mobile identity",
                        });
                    }
                    set_once(
                        &mut mobile_identity_esn,
                        identity,
                        CM_SERVICE_REQUEST,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_SLOT_CYCLE_INDEX => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut slot_cycle_index,
                        SlotCycleIndex::decode(rest[offset + 1])?,
                        CM_SERVICE_REQUEST,
                        IE_SLOT_CYCLE_INDEX,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_RESPONSE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_response_parameter,
                        AuthenticationResponseParameter::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_AUTHENTICATION_RESPONSE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut authentication_confirmation_parameter,
                        AuthenticationConfirmationParameter(rest[offset + 1]),
                        CM_SERVICE_REQUEST,
                        IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_PARAMETER_COUNT => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut authentication_parameter_count,
                        AuthenticationParameterCount(rest[offset + 1] & 0x3f),
                        CM_SERVICE_REQUEST,
                        IE_AUTHENTICATION_PARAMETER_COUNT,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_CHALLENGE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_challenge_parameter,
                        AuthenticationChallengeParameter::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_SERVICE_OPTION => {
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(take_fixed(&rest[offset..], 2)?)?,
                        CM_SERVICE_REQUEST,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_VOICE_PRIVACY_REQUEST => {
                    set_marker_once(
                        &mut voice_privacy_request,
                        CM_SERVICE_REQUEST,
                        IE_VOICE_PRIVACY_REQUEST,
                    )?;
                    offset += 1;
                }
                IE_RADIO_ENVIRONMENT_AND_RESOURCES => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut radio_environment_and_resources,
                        RadioEnvironmentAndResources::decode(rest[offset + 1]),
                        CM_SERVICE_REQUEST,
                        IE_RADIO_ENVIRONMENT_AND_RESOURCES,
                    )?;
                    offset += 2;
                }
                IE_CALLED_PARTY_ASCII_NUMBER_ALT => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut called_party_ascii_number,
                        CallingPartyAsciiNumber::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_CALLED_PARTY_ASCII_NUMBER_ALT,
                    )?;
                    offset += consumed;
                }
                IE_CIRCUIT_IDENTITY_CODE => {
                    set_once(
                        &mut circuit_identity_code,
                        CircuitIdentityCode::decode(take_fixed(&rest[offset..], 2)?)?,
                        CM_SERVICE_REQUEST,
                        IE_CIRCUIT_IDENTITY_CODE,
                    )?;
                    offset += 3;
                }
                IE_AUTHENTICATION_EVENT => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    if payload.len() != 1 {
                        return Err(Error::InvalidLength {
                            expected: 1,
                            actual: payload.len(),
                        });
                    }
                    set_once(
                        &mut authentication_event,
                        AuthenticationEvent(payload[0]),
                        CM_SERVICE_REQUEST,
                        IE_AUTHENTICATION_EVENT,
                    )?;
                    offset += consumed;
                }
                IE_AUTHENTICATION_DATA => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_data,
                        AuthenticationData::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_AUTHENTICATION_DATA,
                    )?;
                    offset += consumed;
                }
                IE_PACA_REORIGINATION_INDICATOR => {
                    let (_, _payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_marker_once(
                        &mut paca_reorigination_indicator,
                        CM_SERVICE_REQUEST,
                        IE_PACA_REORIGINATION_INDICATOR,
                    )?;
                    offset += consumed;
                }
                IE_USER_ZONE_ID => {
                    set_once(
                        &mut user_zone_id,
                        UserZoneId::decode(take_fixed(&rest[offset..], 2)?)?,
                        CM_SERVICE_REQUEST,
                        IE_USER_ZONE_ID,
                    )?;
                    offset += 3;
                }
                IE_IS2000_MOBILE_CAPABILITIES => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut is2000_mobile_capabilities,
                        Is2000MobileCapabilities::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_IS2000_MOBILE_CAPABILITIES,
                    )?;
                    offset += consumed;
                }
                IE_CDMA_SERVING_ONE_WAY_DELAY => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut cdma_serving_one_way_delay,
                        HandoffCdmaServingOneWayDelay::decode(payload)?,
                        CM_SERVICE_REQUEST,
                        IE_CDMA_SERVING_ONE_WAY_DELAY,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            cm_service_type,
            classmark_information_type_2,
            mobile_identity_imsi,
            called_party_bcd_number,
            tag,
            mobile_identity_esn,
            slot_cycle_index,
            authentication_response_parameter,
            authentication_confirmation_parameter,
            authentication_parameter_count,
            authentication_challenge_parameter,
            service_option,
            voice_privacy_request,
            radio_environment_and_resources,
            called_party_ascii_number,
            circuit_identity_code,
            authentication_event,
            authentication_data,
            paca_reorigination_indicator,
            user_zone_id,
            is2000_mobile_capabilities,
            cdma_serving_one_way_delay,
        })
    }
}

/// Exact DTAP `Paging Response` payload carried inside `Layer 3 Information`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagingResponseMessage {
    pub classmark_information_type_2: ClassmarkInformationType2,
    pub mobile_identity_imsi: MobileIdentity,
    pub tag: Option<Tag>,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub slot_cycle_index: Option<SlotCycleIndex>,
    pub authentication_response_parameter: Option<AuthenticationResponseParameter>,
    pub authentication_confirmation_parameter: Option<AuthenticationConfirmationParameter>,
    pub authentication_parameter_count: Option<AuthenticationParameterCount>,
    pub authentication_challenge_parameter: Option<AuthenticationChallengeParameter>,
    pub service_option: Option<ServiceOption>,
    pub voice_privacy_request: bool,
    pub circuit_identity_code: Option<CircuitIdentityCode>,
    pub authentication_event: Option<AuthenticationEvent>,
    pub radio_environment_and_resources: Option<RadioEnvironmentAndResources>,
    pub user_zone_id: Option<UserZoneId>,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
    pub cdma_serving_one_way_delay: Option<HandoffCdmaServingOneWayDelay>,
}

impl PagingResponseMessage {
    /// Encodes the DTAP message body including protocol discriminator and message type.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            0x00,
            PAGING_RESPONSE,
        ];
        push_l3_tlv(&mut out, 0x12, self.classmark_information_type_2.encode()?)?;
        let mobile_identity = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "Paging Response",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        push_l3_lv(&mut out, &mobile_identity)?;
        if let Some(tag) = self.tag {
            push_fixed(&mut out, IE_TAG, &tag.encode());
        }
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "Paging Response",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_l3_tlv(&mut out, IE_MOBILE_IDENTITY, &esn)?;
        }
        if let Some(slot_cycle_index) = self.slot_cycle_index {
            push_fixed(&mut out, IE_SLOT_CYCLE_INDEX, &[slot_cycle_index.encode()?]);
        }
        if let Some(authentication_response_parameter) = self.authentication_response_parameter {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_RESPONSE_PARAMETER,
                &authentication_response_parameter.encode(),
            )?;
        }
        if let Some(authentication_confirmation_parameter) =
            self.authentication_confirmation_parameter
        {
            push_fixed(
                &mut out,
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                &[authentication_confirmation_parameter.0],
            );
        }
        if let Some(authentication_parameter_count) = self.authentication_parameter_count {
            push_fixed(
                &mut out,
                IE_AUTHENTICATION_PARAMETER_COUNT,
                &[authentication_parameter_count.0 & 0x3f],
            );
        }
        if let Some(authentication_challenge_parameter) = self.authentication_challenge_parameter {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                &authentication_challenge_parameter.encode(),
            )?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut out, IE_SERVICE_OPTION, &service_option.encode());
        }
        if self.voice_privacy_request {
            out.push(IE_VOICE_PRIVACY_REQUEST);
        }
        if let Some(circuit_identity_code) = self.circuit_identity_code {
            push_fixed(
                &mut out,
                IE_CIRCUIT_IDENTITY_CODE,
                &circuit_identity_code.encode()?,
            );
        }
        if let Some(authentication_event) = self.authentication_event {
            push_l3_tlv(&mut out, IE_AUTHENTICATION_EVENT, &[authentication_event.0])?;
        }
        if let Some(radio_environment_and_resources) = self.radio_environment_and_resources {
            push_fixed(
                &mut out,
                IE_RADIO_ENVIRONMENT_AND_RESOURCES,
                &[radio_environment_and_resources.encode()?],
            );
        }
        if let Some(user_zone_id) = self.user_zone_id {
            push_fixed(&mut out, IE_USER_ZONE_ID, &user_zone_id.encode());
        }
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_l3_tlv(
                &mut out,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        if let Some(cdma_serving_one_way_delay) = self.cdma_serving_one_way_delay {
            push_l3_tlv(
                &mut out,
                IE_CDMA_SERVING_ONE_WAY_DELAY,
                &cdma_serving_one_way_delay.encode()?,
            )?;
        }
        Ok(out)
    }

    /// Decodes the DTAP message body including protocol discriminator and message type.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let rest = parse_dtap(
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            PAGING_RESPONSE,
            input,
        )?;
        let mut offset = 0;
        let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
        if rest[offset] != 0x12 {
            return Err(Error::UnknownInformationElement(rest[offset]));
        }
        let classmark_information_type_2 = ClassmarkInformationType2::decode(payload)?;
        offset += consumed;
        let (_, imsi_payload, consumed) = decode_lv(&rest[offset..])?;
        let mobile_identity_imsi = MobileIdentity::decode(imsi_payload)?;
        if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "Paging Response",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        offset += consumed;
        let mut tag = None;
        let mut mobile_identity_esn = None;
        let mut slot_cycle_index = None;
        let mut authentication_response_parameter = None;
        let mut authentication_confirmation_parameter = None;
        let mut authentication_parameter_count = None;
        let mut authentication_challenge_parameter = None;
        let mut service_option = None;
        let mut voice_privacy_request = false;
        let mut circuit_identity_code = None;
        let mut authentication_event = None;
        let mut radio_environment_and_resources = None;
        let mut user_zone_id = None;
        let mut is2000_mobile_capabilities = None;
        let mut cdma_serving_one_way_delay = None;
        while offset < rest.len() {
            match rest[offset] {
                IE_TAG => {
                    set_once(
                        &mut tag,
                        Tag::decode(take_fixed(&rest[offset..], 4)?)?,
                        PAGING_RESPONSE,
                        IE_TAG,
                    )?;
                    offset += 5;
                }
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "Paging Response",
                            reason: "ESN field must contain ESN mobile identity",
                        });
                    }
                    set_once(
                        &mut mobile_identity_esn,
                        identity,
                        PAGING_RESPONSE,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_SLOT_CYCLE_INDEX => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut slot_cycle_index,
                        SlotCycleIndex::decode(rest[offset + 1])?,
                        PAGING_RESPONSE,
                        IE_SLOT_CYCLE_INDEX,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_RESPONSE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_response_parameter,
                        AuthenticationResponseParameter::decode(payload)?,
                        PAGING_RESPONSE,
                        IE_AUTHENTICATION_RESPONSE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut authentication_confirmation_parameter,
                        AuthenticationConfirmationParameter(rest[offset + 1]),
                        PAGING_RESPONSE,
                        IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_PARAMETER_COUNT => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut authentication_parameter_count,
                        AuthenticationParameterCount(rest[offset + 1] & 0x3f),
                        PAGING_RESPONSE,
                        IE_AUTHENTICATION_PARAMETER_COUNT,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_CHALLENGE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_challenge_parameter,
                        AuthenticationChallengeParameter::decode(payload)?,
                        PAGING_RESPONSE,
                        IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_SERVICE_OPTION => {
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(take_fixed(&rest[offset..], 2)?)?,
                        PAGING_RESPONSE,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_VOICE_PRIVACY_REQUEST => {
                    set_marker_once(
                        &mut voice_privacy_request,
                        PAGING_RESPONSE,
                        IE_VOICE_PRIVACY_REQUEST,
                    )?;
                    offset += 1;
                }
                IE_CIRCUIT_IDENTITY_CODE => {
                    set_once(
                        &mut circuit_identity_code,
                        CircuitIdentityCode::decode(take_fixed(&rest[offset..], 2)?)?,
                        PAGING_RESPONSE,
                        IE_CIRCUIT_IDENTITY_CODE,
                    )?;
                    offset += 3;
                }
                IE_AUTHENTICATION_EVENT => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    if payload.len() != 1 {
                        return Err(Error::InvalidLength {
                            expected: 1,
                            actual: payload.len(),
                        });
                    }
                    set_once(
                        &mut authentication_event,
                        AuthenticationEvent(payload[0]),
                        PAGING_RESPONSE,
                        IE_AUTHENTICATION_EVENT,
                    )?;
                    offset += consumed;
                }
                IE_RADIO_ENVIRONMENT_AND_RESOURCES => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut radio_environment_and_resources,
                        RadioEnvironmentAndResources::decode(rest[offset + 1]),
                        PAGING_RESPONSE,
                        IE_RADIO_ENVIRONMENT_AND_RESOURCES,
                    )?;
                    offset += 2;
                }
                IE_USER_ZONE_ID => {
                    set_once(
                        &mut user_zone_id,
                        UserZoneId::decode(take_fixed(&rest[offset..], 2)?)?,
                        PAGING_RESPONSE,
                        IE_USER_ZONE_ID,
                    )?;
                    offset += 3;
                }
                IE_IS2000_MOBILE_CAPABILITIES => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut is2000_mobile_capabilities,
                        Is2000MobileCapabilities::decode(payload)?,
                        PAGING_RESPONSE,
                        IE_IS2000_MOBILE_CAPABILITIES,
                    )?;
                    offset += consumed;
                }
                IE_CDMA_SERVING_ONE_WAY_DELAY => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut cdma_serving_one_way_delay,
                        HandoffCdmaServingOneWayDelay::decode(payload)?,
                        PAGING_RESPONSE,
                        IE_CDMA_SERVING_ONE_WAY_DELAY,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            classmark_information_type_2,
            mobile_identity_imsi,
            tag,
            mobile_identity_esn,
            slot_cycle_index,
            authentication_response_parameter,
            authentication_confirmation_parameter,
            authentication_parameter_count,
            authentication_challenge_parameter,
            service_option,
            voice_privacy_request,
            circuit_identity_code,
            authentication_event,
            radio_environment_and_resources,
            user_zone_id,
            is2000_mobile_capabilities,
            cdma_serving_one_way_delay,
        })
    }
}

/// Exact A1 tag value used to correlate requests and responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag(pub u32);

impl Tag {
    /// Encodes the fixed-width tag payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// Decodes the fixed-width tag payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self(u32::from_be_bytes([
            input[0], input[1], input[2], input[3],
        ])))
    }
}

/// Exact A1 slot cycle index used for slotted paging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotCycleIndex(pub u8);

impl SlotCycleIndex {
    /// Encodes the slot cycle index octet.
    pub fn encode(self) -> Result<u8> {
        if self.0 > 0x07 {
            return Err(Error::ReservedValue {
                context: "Slot Cycle Index",
                value: self.0,
            });
        }
        Ok(self.0)
    }

    /// Decodes the slot cycle index octet.
    pub fn decode(input: u8) -> Result<Self> {
        let value = input & 0x07;
        if value > 0x07 {
            return Err(Error::ReservedValue {
                context: "Slot Cycle Index",
                value,
            });
        }
        Ok(Self(value))
    }
}

/// Exact fixed-width service-option payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOption(pub u16);

impl ServiceOption {
    /// SO3: EVRC-A / IS-127 narrowband.
    pub const EVRC_A: Self = Self(3);

    /// SO6: Short Message Services.
    pub const SMS: Self = Self(6);

    /// SO7: Packet data, async/fax data service.
    pub const PACKET_DATA: Self = Self(7);

    /// SO33: High-rate packet data service.
    pub const HIGH_RATE_PACKET_DATA: Self = Self(33);

    /// SO68: EVRC-B narrowband.
    pub const EVRC_B: Self = Self(68);

    /// SO70: EVRC-WB.
    pub const EVRC_WB: Self = Self(70);

    /// Encodes the service-option payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the service-option payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact packet-data QoS payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityOfServiceParameters {
    pub packet_priority: u8,
}

impl QualityOfServiceParameters {
    /// Encodes the QoS payload.
    pub fn encode(self) -> Result<[u8; 1]> {
        if self.packet_priority > 0x0d {
            return Err(Error::ReservedValue {
                context: "Packet Priority",
                value: self.packet_priority,
            });
        }
        Ok([self.packet_priority & 0x0f])
    }

    /// Decodes the QoS payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        let packet_priority = input[0] & 0x0f;
        if packet_priority > 0x0d {
            return Err(Error::ReservedValue {
                context: "Packet Priority",
                value: packet_priority,
            });
        }
        Ok(Self { packet_priority })
    }
}

/// A2p Bearer Session-Level Parameters (A.S0014-D v2.0 §4.2.89, IE 0x45).
///
/// Carries the IP address and UDP port for a per-circuit RTP voice bearer
/// session between MSC and BSC. Exchanged in AssignmentRequest (MSC→BSC) and
/// AssignmentComplete (BSC→MSC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A2pBearerSessionParams {
    pub ip_address: std::net::Ipv4Addr,
    pub udp_port: u16,
}

impl A2pBearerSessionParams {
    /// Encodes per §4.2.89: octet 3 = flags, octets 4-7 = IPv4, octets 8-9 = port.
    pub fn encode(&self) -> [u8; 7] {
        let ip = self.ip_address.octets();
        let port = self.udp_port.to_be_bytes();
        [0x01, ip[0], ip[1], ip[2], ip[3], port[0], port[1]]
    }

    /// Decodes per §4.2.89.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 7 {
            return Err(Error::InvalidLength {
                expected: 7,
                actual: input.len(),
            });
        }
        let ip_address = std::net::Ipv4Addr::new(input[1], input[2], input[3], input[4]);
        let udp_port = u16::from_be_bytes([input[5], input[6]]);
        Ok(Self {
            ip_address,
            udp_port,
        })
    }
}

/// A single bearer format entry within A2p Bearer Format-Specific Parameters.
///
/// Per A.S0014-D v2.0 §4.2.90, each entry describes one codec/RTP-payload-type
/// the bearer supports. Optional per-format IP address and UDP port override
/// the session-level values from IE 0x45.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerFormatEntry {
    pub bearer_format_tag_type: u8,
    pub bearer_format_id: u8,
    pub rtp_payload_type: u8,
    pub bearer_addr: Option<(std::net::Ipv4Addr, u16)>,
}

/// A2p Bearer Format-Specific Parameters (A.S0014-D v2.0 §4.2.90, IE 0x46).
///
/// Lists the bearer formats (codecs) supported for an A2p voice call.
/// Per spec note o (§3.1.7), the MSCe SHALL include both IE 0x45 and IE 0x46
/// or neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A2pBearerFormatParams {
    pub formats: Vec<BearerFormatEntry>,
}

impl A2pBearerFormatParams {
    /// Canonical entry list for a voice call carrying EVRC plus RFC 4733
    /// telephone-event for DTMF (A.S0014-D Table 4.2.90-3 IDs 3 and 7).
    pub fn evrc_with_telephone_event(evrc_pt: u8, telephone_event_pt: u8) -> Self {
        Self {
            formats: vec![
                BearerFormatEntry {
                    bearer_format_tag_type: 1,
                    bearer_format_id: crate::voice_bearer::bearer_format_id::EVRC,
                    rtp_payload_type: evrc_pt,
                    bearer_addr: None,
                },
                BearerFormatEntry {
                    bearer_format_tag_type: 1,
                    bearer_format_id: crate::voice_bearer::bearer_format_id::TELEPHONE_EVENT,
                    rtp_payload_type: telephone_event_pt,
                    bearer_addr: None,
                },
            ],
        }
    }

    /// PT for the first telephone-event entry, if present.
    pub fn telephone_event_pt(&self) -> Option<u8> {
        self.formats
            .iter()
            .find(|f| f.bearer_format_id == crate::voice_bearer::bearer_format_id::TELEPHONE_EVENT)
            .map(|f| f.rtp_payload_type)
    }

    /// PT for the first EVRC entry, if present.
    pub fn evrc_pt(&self) -> Option<u8> {
        self.formats
            .iter()
            .find(|f| f.bearer_format_id == crate::voice_bearer::bearer_format_id::EVRC)
            .map(|f| f.rtp_payload_type)
    }

    /// Encodes per §4.2.90.
    ///
    /// Octet 3: bits 7-2 = number of bearer formats, bits 1-0 = IP addr type (00=IPv4).
    /// Per format: length, ext|tag_type|format_id, rtp_pt|addr_flag, [addr, port].
    pub fn encode(&self) -> Vec<u8> {
        let count = self.formats.len() as u8;
        let mut out = vec![count << 2]; // addr type = 00 (IPv4) in bits 1-0
        for f in &self.formats {
            let has_addr = f.bearer_addr.is_some();
            let entry_len: u8 = if has_addr { 2 + 4 + 2 } else { 2 };
            out.push(entry_len);
            // ext=0 (bit 7), tag_type (bits 6-4), format_id (bits 3-0)
            out.push((f.bearer_format_tag_type & 0x07) << 4 | (f.bearer_format_id & 0x0F));
            // rtp_payload_type (bits 7-1), bearer_addr_flag (bit 0)
            let addr_flag: u8 = if has_addr { 1 } else { 0 };
            out.push((f.rtp_payload_type << 1) | addr_flag);
            if let Some((ip, port)) = f.bearer_addr {
                out.extend_from_slice(&ip.octets());
                out.extend_from_slice(&port.to_be_bytes());
            }
        }
        out
    }

    /// Decodes per §4.2.90.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: 0,
            });
        }
        let count = (input[0] >> 2) as usize;
        let mut pos = 1;
        let mut formats = Vec::with_capacity(count);
        for _ in 0..count {
            if pos >= input.len() {
                return Err(Error::InvalidLength {
                    expected: pos + 1,
                    actual: input.len(),
                });
            }
            let entry_len = input[pos] as usize;
            pos += 1;
            if pos + entry_len > input.len() {
                return Err(Error::InvalidLength {
                    expected: pos + entry_len,
                    actual: input.len(),
                });
            }
            let tag_id_byte = input[pos];
            let bearer_format_tag_type = (tag_id_byte >> 4) & 0x07;
            let bearer_format_id = tag_id_byte & 0x0F;
            let pt_flag_byte = input[pos + 1];
            let rtp_payload_type = pt_flag_byte >> 1;
            let addr_flag = pt_flag_byte & 0x01;
            let bearer_addr = if addr_flag == 1 && entry_len >= 8 {
                let ip = std::net::Ipv4Addr::new(
                    input[pos + 2],
                    input[pos + 3],
                    input[pos + 4],
                    input[pos + 5],
                );
                let port = u16::from_be_bytes([input[pos + 6], input[pos + 7]]);
                Some((ip, port))
            } else {
                None
            };
            formats.push(BearerFormatEntry {
                bearer_format_tag_type,
                bearer_format_id,
                rtp_payload_type,
                bearer_addr,
            });
            pos += entry_len;
        }
        Ok(Self { formats })
    }
}

/// Exact PACA queueing timestamp payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacaTimestamp(pub u32);

impl PacaTimestamp {
    /// Encodes the PACA timestamp payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// Decodes the PACA timestamp payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self(u32::from_be_bytes([
            input[0], input[1], input[2], input[3],
        ])))
    }
}

/// Exact A1 priority payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Priority {
    pub call_priority: u8,
    pub queuing_allowed: bool,
    pub preemption_allowed: bool,
}

impl Priority {
    /// Encodes the priority payload.
    pub fn encode(self) -> Result<[u8; 1]> {
        if self.call_priority > 0x0f {
            return Err(Error::ReservedValue {
                context: "Call Priority",
                value: self.call_priority,
            });
        }
        Ok([((self.call_priority & 0x0f) << 2)
            | ((self.queuing_allowed as u8) << 1)
            | (self.preemption_allowed as u8)])
    }

    /// Decodes the priority payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        Ok(Self {
            call_priority: input[0] >> 2,
            queuing_allowed: input[0] & 0x02 != 0,
            preemption_allowed: input[0] & 0x01 != 0,
        })
    }
}

/// Exact cause payload used by `Clear Command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cause(pub u8);

impl Cause {
    /// Encodes the cause payload.
    pub const fn encode(self) -> [u8; 1] {
        [self.0]
    }

    /// Decodes the cause payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        Ok(Self(input[0]))
    }
}

/// Exact Q.931-style cause payload used by `Clear Command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CauseLayer3 {
    pub coding_standard: u8,
    pub location: u8,
    pub cause_value: u8,
}

impl CauseLayer3 {
    /// Encodes the cause-layer-3 payload.
    pub fn encode(self) -> Result<[u8; 2]> {
        if self.coding_standard > 0x03 || self.location > 0x0f || self.cause_value > 0x7f {
            return Err(Error::InvalidValue {
                context: "Cause Layer 3",
                reason: "field exceeds bit width",
            });
        }
        Ok([
            0x80 | ((self.coding_standard & 0x03) << 5) | (self.location & 0x0f),
            0x80 | (self.cause_value & 0x7f),
        ])
    }

    /// Decodes the cause-layer-3 payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self {
            coding_standard: (input[0] >> 5) & 0x03,
            location: input[0] & 0x0f,
            cause_value: input[1] & 0x7f,
        })
    }
}

/// Exact fixed-width channel-type payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelType {
    pub speech_or_data_indicator: u8,
    pub channel_rate_and_type: u8,
    pub coding: u8,
}

impl ChannelType {
    /// Encodes the channel-type payload.
    pub const fn encode(self) -> [u8; 3] {
        [
            self.speech_or_data_indicator,
            self.channel_rate_and_type,
            self.coding,
        ]
    }

    /// Decodes the channel-type payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 3 {
            return Err(Error::InvalidLength {
                expected: 3,
                actual: input.len(),
            });
        }
        Ok(Self {
            speech_or_data_indicator: input[0],
            channel_rate_and_type: input[1],
            coding: input[2],
        })
    }
}

/// Exact fixed-width circuit identity code payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitIdentityCode {
    pub pcm_multiplexer: u16,
    pub timeslot: u8,
}

impl CircuitIdentityCode {
    /// Returns the packed 16-bit circuit identity (pcm_multiplexer << 5 | timeslot).
    pub fn to_packed(&self) -> u16 {
        (self.pcm_multiplexer << 5) | self.timeslot as u16
    }

    /// Encodes the circuit identity code payload.
    pub fn encode(self) -> Result<[u8; 2]> {
        if self.pcm_multiplexer > 0x07ff || self.timeslot > 0x1f {
            return Err(Error::InvalidValue {
                context: "Circuit Identity Code",
                reason: "PCM multiplexer or timeslot out of range",
            });
        }
        let packed = (self.pcm_multiplexer << 5) | self.timeslot as u16;
        Ok(packed.to_be_bytes())
    }

    /// Decodes the circuit identity code payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        let packed = u16::from_be_bytes([input[0], input[1]]);
        Ok(Self {
            pcm_multiplexer: packed >> 5,
            timeslot: (packed & 0x1f) as u8,
        })
    }
}

/// Exact fixed-width channel-number payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelNumber(pub u16);

impl ChannelNumber {
    /// Encodes the channel-number payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the channel-number payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact encryption-parameter entry nested inside `Encryption Information`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionParameter {
    pub identifier: u8,
    pub status: bool,
    pub available: bool,
    pub value: Vec<u8>,
}

impl EncryptionParameter {
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        if self.identifier > 0x1f || self.value.len() > u8::MAX as usize {
            return Err(Error::InvalidValue {
                context: "Encryption Information",
                reason: "identifier or length out of range",
            });
        }
        out.push(
            0x80 | ((self.identifier & 0x1f) << 2)
                | ((self.status as u8) << 1)
                | (self.available as u8),
        );
        out.push(self.value.len() as u8);
        out.extend_from_slice(&self.value);
        Ok(())
    }

    fn decode(input: &[u8]) -> Result<(Self, usize)> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let length = input[1] as usize;
        if input.len() < 2 + length {
            return Err(Error::Truncated {
                needed: 2 + length,
                actual: input.len(),
            });
        }
        Ok((
            Self {
                identifier: (input[0] >> 2) & 0x1f,
                status: input[0] & 0x02 != 0,
                available: input[0] & 0x01 != 0,
                value: input[2..2 + length].to_vec(),
            },
            2 + length,
        ))
    }
}

/// Exact variable-length encryption information payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionInformation {
    pub parameters: Vec<EncryptionParameter>,
}

impl EncryptionInformation {
    /// Encodes the encryption-information payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for parameter in &self.parameters {
            parameter.encode(&mut out)?;
        }
        Ok(out)
    }

    /// Decodes the encryption-information payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut parameters = Vec::new();
        let mut offset = 0;
        while offset < input.len() {
            let (parameter, consumed) = EncryptionParameter::decode(&input[offset..])?;
            parameters.push(parameter);
            offset += consumed;
        }
        Ok(Self { parameters })
    }
}

/// Exact fixed-width signal payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signal {
    pub signal_value: u8,
    pub alert_pitch: u8,
}

impl Signal {
    /// Encodes the signal payload.
    pub fn encode(self) -> Result<u8> {
        if self.signal_value > 0x3f || self.alert_pitch > 0x03 {
            return Err(Error::InvalidValue {
                context: "Signal",
                reason: "signal value or alert pitch out of range",
            });
        }
        Ok((self.signal_value << 2) | self.alert_pitch)
    }

    /// Decodes the signal payload.
    pub fn decode(input: u8) -> Self {
        Self {
            signal_value: input >> 2,
            alert_pitch: input & 0x03,
        }
    }
}

/// Exact calling-party ASCII number payload as raw on-wire octets following the IE length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallingPartyAsciiNumber(pub Vec<u8>);

impl CallingPartyAsciiNumber {
    /// Encodes the raw calling-party ASCII number payload.
    pub fn encode(&self) -> Result<&[u8]> {
        if self.0.is_empty() {
            return Err(Error::InvalidValue {
                context: "Calling Party ASCII Number",
                reason: "payload must not be empty",
            });
        }
        Ok(&self.0)
    }

    /// Decodes the raw calling-party ASCII number payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidValue {
                context: "Calling Party ASCII Number",
                reason: "payload must not be empty",
            });
        }
        Ok(Self(input.to_vec()))
    }
}

/// One nested MS information record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsInformationRecord {
    pub record_type: u8,
    pub content: Vec<u8>,
}

/// Exact `MS Information Records` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsInformationRecords {
    pub records: Vec<MsInformationRecord>,
}

impl MsInformationRecords {
    /// Encodes the MS-information-records payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.records.is_empty() {
            return Err(Error::InvalidValue {
                context: "MS Information Records",
                reason: "must contain at least one record",
            });
        }
        let mut out = Vec::new();
        for record in &self.records {
            if record.content.len() > u8::MAX as usize {
                return Err(Error::InvalidLength {
                    expected: u8::MAX as usize,
                    actual: record.content.len(),
                });
            }
            out.push(record.record_type);
            out.push(record.content.len() as u8);
            out.extend_from_slice(&record.content);
        }
        Ok(out)
    }

    /// Decodes the MS-information-records payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidValue {
                context: "MS Information Records",
                reason: "must contain at least one record",
            });
        }
        let mut offset = 0;
        let mut records = Vec::new();
        while offset < input.len() {
            if input.len() - offset < 2 {
                return Err(Error::Truncated {
                    needed: offset + 2,
                    actual: input.len(),
                });
            }
            let length = input[offset + 1] as usize;
            if input.len() < offset + 2 + length {
                return Err(Error::Truncated {
                    needed: offset + 2 + length,
                    actual: input.len(),
                });
            }
            records.push(MsInformationRecord {
                record_type: input[offset],
                content: input[offset + 2..offset + 2 + length].to_vec(),
            });
            offset += 2 + length;
        }
        Ok(Self { records })
    }
}

/// Exact cell-identifier-list payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellIdentifierList {
    Cells(Vec<CellId>),
    LocationAreas(Vec<u16>),
}

impl CellIdentifierList {
    /// Encodes the cell-identifier-list payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Cells(cells) => {
                if cells.is_empty() {
                    return Err(Error::InvalidValue {
                        context: "Cell Identifier List",
                        reason: "must contain at least one identifier",
                    });
                }
                let mut out = Vec::with_capacity(1 + cells.len() * 2);
                out.push(0x02);
                for cell in cells {
                    let encoded = cell.encode()?;
                    out.extend_from_slice(&encoded[1..]);
                }
                Ok(out)
            }
            Self::LocationAreas(lacs) => {
                if lacs.is_empty() {
                    return Err(Error::InvalidValue {
                        context: "Cell Identifier List",
                        reason: "must contain at least one identifier",
                    });
                }
                let mut out = Vec::with_capacity(1 + lacs.len() * 2);
                out.push(0x05);
                for lac in lacs {
                    out.extend_from_slice(&lac.to_be_bytes());
                }
                Ok(out)
            }
        }
    }

    /// Decodes the cell-identifier-list payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&discriminator, rest)) = input.split_first() else {
            return Err(Error::InvalidValue {
                context: "Cell Identifier List",
                reason: "payload must not be empty",
            });
        };
        match discriminator {
            0x02 => {
                if rest.is_empty() || rest.len() % 2 != 0 {
                    return Err(Error::InvalidValue {
                        context: "Cell Identifier List",
                        reason: "cell list length must be a non-zero multiple of 2",
                    });
                }
                let mut cells = Vec::new();
                let mut offset = 0;
                while offset < rest.len() {
                    cells.push(CellId::decode(&[0x02, rest[offset], rest[offset + 1]])?);
                    offset += 2;
                }
                Ok(Self::Cells(cells))
            }
            0x05 => {
                if rest.is_empty() || rest.len() % 2 != 0 {
                    return Err(Error::InvalidValue {
                        context: "Cell Identifier List",
                        reason: "LAC list length must be a non-zero multiple of 2",
                    });
                }
                let mut lacs = Vec::new();
                let mut offset = 0;
                while offset < rest.len() {
                    lacs.push(u16::from_be_bytes([rest[offset], rest[offset + 1]]));
                    offset += 2;
                }
                Ok(Self::LocationAreas(lacs))
            }
            other => Err(Error::ReservedValue {
                context: "Cell Identifier List discriminator",
                value: other,
            }),
        }
    }
}

/// Exact raw payload for the `IS-2000 Mobile Capabilities` IE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is2000MobileCapabilities(pub Vec<u8>);

impl Is2000MobileCapabilities {
    /// Encodes the raw mobile-capabilities payload.
    pub fn encode(&self) -> Result<&[u8]> {
        if self.0.is_empty() {
            return Err(Error::InvalidValue {
                context: "IS-2000 Mobile Capabilities",
                reason: "payload must not be empty",
            });
        }
        Ok(&self.0)
    }

    /// Decodes the raw mobile-capabilities payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidValue {
                context: "IS-2000 Mobile Capabilities",
                reason: "payload must not be empty",
            });
        }
        Ok(Self(input.to_vec()))
    }
}

/// Exact BSMAP `Authentication Request` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationRequestBsmapMessage {
    pub authentication_challenge_parameter_randu: AuthenticationChallengeParameter,
    pub mobile_identity_imsi: Option<MobileIdentity>,
    pub tag: Option<Tag>,
    pub cell_identifier_list: Option<CellIdentifierList>,
    pub slot_cycle_index: Option<SlotCycleIndex>,
}

impl AuthenticationRequestBsmapMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(
            &mut body,
            IE_AUTHENTICATION_CHALLENGE_PARAMETER,
            &self.authentication_challenge_parameter_randu.encode(),
        )?;
        if let Some(mobile_identity_imsi) = &self.mobile_identity_imsi {
            let imsi = mobile_identity_imsi.encode()?;
            if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
                return Err(Error::InvalidValue {
                    context: "Authentication Request BSMAP",
                    reason: "mobile identity IMSI field must contain IMSI",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &imsi)?;
        }
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            let payload = cell_identifier_list.encode()?;
            push_tlv(&mut body, IE_CELL_IDENTIFIER_LIST, &payload)?;
        }
        if let Some(slot_cycle_index) = self.slot_cycle_index {
            push_fixed(
                &mut body,
                IE_SLOT_CYCLE_INDEX,
                &[slot_cycle_index.encode()?],
            );
        }
        encode_bsmap(AUTHENTICATION_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(AUTHENTICATION_REQUEST, input)?;
        let mut offset = 0;
        let mut authentication_challenge_parameter_randu = None;
        let mut mobile_identity_imsi = None;
        let mut tag = None;
        let mut cell_identifier_list = None;
        let mut slot_cycle_index = None;
        while offset < body.len() {
            match body[offset] {
                IE_AUTHENTICATION_CHALLENGE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut authentication_challenge_parameter_randu,
                        AuthenticationChallengeParameter::decode(payload)?,
                        AUTHENTICATION_REQUEST,
                        IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Imsi(_)) {
                        return Err(Error::InvalidValue {
                            context: "Authentication Request BSMAP",
                            reason: "mobile identity IMSI field must contain IMSI",
                        });
                    }
                    set_once(
                        &mut mobile_identity_imsi,
                        identity,
                        AUTHENTICATION_REQUEST,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(
                        &mut tag,
                        Tag::decode(payload)?,
                        AUTHENTICATION_REQUEST,
                        IE_TAG,
                    )?;
                    offset += 5;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cell_identifier_list,
                        CellIdentifierList::decode(payload)?,
                        AUTHENTICATION_REQUEST,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += consumed;
                }
                IE_SLOT_CYCLE_INDEX => {
                    ensure_remaining(body, offset + 1, 1)?;
                    set_once(
                        &mut slot_cycle_index,
                        SlotCycleIndex::decode(body[offset + 1])?,
                        AUTHENTICATION_REQUEST,
                        IE_SLOT_CYCLE_INDEX,
                    )?;
                    offset += 2;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            authentication_challenge_parameter_randu: authentication_challenge_parameter_randu
                .ok_or(Error::MissingRequiredElement {
                    message_type: AUTHENTICATION_REQUEST,
                    id: IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                })?,
            mobile_identity_imsi,
            tag,
            cell_identifier_list,
            slot_cycle_index,
        })
    }
}

/// Exact DTAP `Authentication Request` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationRequestDtapMessage {
    pub authentication_challenge_parameter_randu: AuthenticationChallengeParameter,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
}

impl AuthenticationRequestDtapMessage {
    /// Encodes the message using the exact A1 mobility-management DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(
            &mut body,
            IE_AUTHENTICATION_CHALLENGE_PARAMETER,
            &self.authentication_challenge_parameter_randu.encode(),
        )?;
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_tlv(
                &mut body,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        encode_a1_mm_dtap(AUTHENTICATION_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 mobility-management DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(AUTHENTICATION_REQUEST, input)?;
        let mut offset = 0;
        let mut authentication_challenge_parameter_randu = None;
        let mut is2000_mobile_capabilities = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_AUTHENTICATION_CHALLENGE_PARAMETER => set_once(
                    &mut authentication_challenge_parameter_randu,
                    AuthenticationChallengeParameter::decode(payload)?,
                    AUTHENTICATION_REQUEST,
                    IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                )?,
                IE_IS2000_MOBILE_CAPABILITIES => set_once(
                    &mut is2000_mobile_capabilities,
                    Is2000MobileCapabilities::decode(payload)?,
                    AUTHENTICATION_REQUEST,
                    IE_IS2000_MOBILE_CAPABILITIES,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            authentication_challenge_parameter_randu: authentication_challenge_parameter_randu
                .ok_or(Error::MissingRequiredElement {
                    message_type: AUTHENTICATION_REQUEST,
                    id: IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                })?,
            is2000_mobile_capabilities,
        })
    }
}

/// Exact typed `Authentication Request` message, which may be BSMAP or DTAP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthenticationRequestMessage {
    Bsmap(AuthenticationRequestBsmapMessage),
    Dtap(AuthenticationRequestDtapMessage),
}

impl AuthenticationRequestMessage {
    /// Encodes the message using the exact A1 wire format for its variant.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Bsmap(message) => message.encode(),
            Self::Dtap(message) => message.encode(),
        }
    }

    /// Decodes the message using the exact A1 wire format for either supported variant.
    pub fn decode(input: &[u8]) -> Result<Self> {
        match input.first().copied() {
            Some(BSMAP_MESSAGE_DISCRIMINATION) => Ok(Self::Bsmap(
                AuthenticationRequestBsmapMessage::decode(input)?,
            )),
            Some(A1_DTAP_MESSAGE_DISCRIMINATION) => {
                Ok(Self::Dtap(AuthenticationRequestDtapMessage::decode(input)?))
            }
            Some(other) => Err(Error::ReservedValue {
                context: "Authentication Request message discrimination",
                value: other,
            }),
            None => Err(Error::EmptyMessage),
        }
    }
}

/// Exact BSMAP `Authentication Response` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationResponseBsmapMessage {
    pub authentication_response_parameter_authu: AuthenticationResponseParameter,
    pub mobile_identity_imsi: Option<MobileIdentity>,
    pub tag: Option<Tag>,
    pub mobile_identity_esn: Option<MobileIdentity>,
}

impl AuthenticationResponseBsmapMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(
            &mut body,
            IE_AUTHENTICATION_RESPONSE_PARAMETER,
            &self.authentication_response_parameter_authu.encode(),
        )?;
        if let Some(mobile_identity_imsi) = &self.mobile_identity_imsi {
            let imsi = mobile_identity_imsi.encode()?;
            if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
                return Err(Error::InvalidValue {
                    context: "Authentication Response BSMAP",
                    reason: "mobile identity IMSI field must contain IMSI",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &imsi)?;
        }
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "Authentication Response BSMAP",
                    reason: "mobile identity ESN field must contain ESN",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &esn)?;
        }
        encode_bsmap(AUTHENTICATION_RESPONSE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(AUTHENTICATION_RESPONSE, input)?;
        let mut offset = 0;
        let mut authentication_response_parameter_authu = None;
        let mut mobile_identity_imsi = None;
        let mut tag = None;
        let mut mobile_identity_esn = None;
        while offset < body.len() {
            match body[offset] {
                IE_AUTHENTICATION_RESPONSE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut authentication_response_parameter_authu,
                        AuthenticationResponseParameter::decode(payload)?,
                        AUTHENTICATION_RESPONSE,
                        IE_AUTHENTICATION_RESPONSE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    match identity {
                        MobileIdentity::Imsi(_) => set_once(
                            &mut mobile_identity_imsi,
                            identity,
                            AUTHENTICATION_RESPONSE,
                            IE_MOBILE_IDENTITY,
                        )?,
                        MobileIdentity::Esn(_) => set_once(
                            &mut mobile_identity_esn,
                            identity,
                            AUTHENTICATION_RESPONSE,
                            IE_MOBILE_IDENTITY,
                        )?,
                        MobileIdentity::Meid(_) => {
                            return Err(Error::InvalidValue {
                                context: "Authentication Response",
                                reason: "MEID identity not expected here",
                            });
                        }
                    }
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(
                        &mut tag,
                        Tag::decode(payload)?,
                        AUTHENTICATION_RESPONSE,
                        IE_TAG,
                    )?;
                    offset += 5;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            authentication_response_parameter_authu: authentication_response_parameter_authu
                .ok_or(Error::MissingRequiredElement {
                    message_type: AUTHENTICATION_RESPONSE,
                    id: IE_AUTHENTICATION_RESPONSE_PARAMETER,
                })?,
            mobile_identity_imsi,
            tag,
            mobile_identity_esn,
        })
    }
}

/// Exact DTAP `Authentication Response` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationResponseDtapMessage {
    pub authentication_response_parameter_authu: AuthenticationResponseParameter,
}

impl AuthenticationResponseDtapMessage {
    /// Encodes the message using the exact A1 mobility-management DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(
            AUTHENTICATION_RESPONSE,
            &encode_single_tlv(
                IE_AUTHENTICATION_RESPONSE_PARAMETER,
                &self.authentication_response_parameter_authu.encode(),
            )?,
        )
    }

    /// Decodes the message from the exact A1 mobility-management DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(AUTHENTICATION_RESPONSE, input)?;
        let (id, payload, consumed) = decode_tlv(body)?;
        if id != IE_AUTHENTICATION_RESPONSE_PARAMETER {
            return Err(Error::UnknownInformationElement(id));
        }
        if consumed != body.len() {
            return Err(Error::InvalidLength {
                expected: consumed,
                actual: body.len(),
            });
        }
        Ok(Self {
            authentication_response_parameter_authu: AuthenticationResponseParameter::decode(
                payload,
            )?,
        })
    }
}

/// Exact typed `Authentication Response` message, which may be BSMAP or DTAP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthenticationResponseMessage {
    Bsmap(AuthenticationResponseBsmapMessage),
    Dtap(AuthenticationResponseDtapMessage),
}

impl AuthenticationResponseMessage {
    /// Encodes the message using the exact A1 wire format for its variant.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Bsmap(message) => message.encode(),
            Self::Dtap(message) => message.encode(),
        }
    }

    /// Decodes the message using the exact A1 wire format for either supported variant.
    pub fn decode(input: &[u8]) -> Result<Self> {
        match input.first().copied() {
            Some(BSMAP_MESSAGE_DISCRIMINATION) => Ok(Self::Bsmap(
                AuthenticationResponseBsmapMessage::decode(input)?,
            )),
            Some(A1_DTAP_MESSAGE_DISCRIMINATION) => Ok(Self::Dtap(
                AuthenticationResponseDtapMessage::decode(input)?,
            )),
            Some(other) => Err(Error::ReservedValue {
                context: "Authentication Response message discrimination",
                value: other,
            }),
            None => Err(Error::EmptyMessage),
        }
    }
}

/// Exact `SSD Update Request` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SsdUpdateRequestMessage {
    pub authentication_challenge_parameter_randssd: SsdUpdateChallengeParameter,
}

impl SsdUpdateRequestMessage {
    /// Encodes the message using the exact A1 mobility-management DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(
            SSD_UPDATE_REQUEST,
            &encode_single_tlv(
                IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                &self.authentication_challenge_parameter_randssd.encode(),
            )?,
        )
    }

    /// Decodes the message from the exact A1 mobility-management DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(SSD_UPDATE_REQUEST, input)?;
        let (id, payload, consumed) = decode_tlv(body)?;
        if id != IE_AUTHENTICATION_CHALLENGE_PARAMETER {
            return Err(Error::UnknownInformationElement(id));
        }
        if consumed != body.len() {
            return Err(Error::InvalidLength {
                expected: consumed,
                actual: body.len(),
            });
        }
        Ok(Self {
            authentication_challenge_parameter_randssd: SsdUpdateChallengeParameter::decode(
                payload,
            )?,
        })
    }
}

/// Exact `Base Station Challenge` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseStationChallengeMessage {
    pub authentication_challenge_parameter_randbs: AuthenticationChallengeParameter,
}

impl BaseStationChallengeMessage {
    /// Encodes the message using the exact A1 mobility-management DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(
            BASE_STATION_CHALLENGE,
            &encode_single_tlv(
                IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                &self.authentication_challenge_parameter_randbs.encode(),
            )?,
        )
    }

    /// Decodes the message from the exact A1 mobility-management DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(BASE_STATION_CHALLENGE, input)?;
        let (id, payload, consumed) = decode_tlv(body)?;
        if id != IE_AUTHENTICATION_CHALLENGE_PARAMETER {
            return Err(Error::UnknownInformationElement(id));
        }
        if consumed != body.len() {
            return Err(Error::InvalidLength {
                expected: consumed,
                actual: body.len(),
            });
        }
        Ok(Self {
            authentication_challenge_parameter_randbs: AuthenticationChallengeParameter::decode(
                payload,
            )?,
        })
    }
}

/// Exact `Base Station Challenge Response` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaseStationChallengeResponseMessage {
    pub authentication_response_parameter_authbs: AuthenticationResponseParameter,
}

impl BaseStationChallengeResponseMessage {
    /// Encodes the message using the exact A1 mobility-management DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(
            BASE_STATION_CHALLENGE_RESPONSE,
            &encode_single_tlv(
                IE_AUTHENTICATION_RESPONSE_PARAMETER,
                &self.authentication_response_parameter_authbs.encode(),
            )?,
        )
    }

    /// Decodes the message from the exact A1 mobility-management DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(BASE_STATION_CHALLENGE_RESPONSE, input)?;
        let (id, payload, consumed) = decode_tlv(body)?;
        if id != IE_AUTHENTICATION_RESPONSE_PARAMETER {
            return Err(Error::UnknownInformationElement(id));
        }
        if consumed != body.len() {
            return Err(Error::InvalidLength {
                expected: consumed,
                actual: body.len(),
            });
        }
        Ok(Self {
            authentication_response_parameter_authbs: AuthenticationResponseParameter::decode(
                payload,
            )?,
        })
    }
}

/// Exact `SSD Update Response` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SsdUpdateResponseMessage {
    pub cause_layer_3: Option<CauseLayer3>,
}

impl SsdUpdateResponseMessage {
    /// Encodes the message using the exact A1 mobility-management DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if let Some(cause_layer_3) = self.cause_layer_3 {
            push_tlv(&mut body, IE_CAUSE_LAYER_3, &cause_layer_3.encode()?)?;
        }
        encode_a1_mm_dtap(SSD_UPDATE_RESPONSE, &body)
    }

    /// Decodes the message from the exact A1 mobility-management DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(SSD_UPDATE_RESPONSE, input)?;
        let mut offset = 0;
        let mut cause_layer_3 = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CAUSE_LAYER_3 => set_once(
                    &mut cause_layer_3,
                    CauseLayer3::decode(payload)?,
                    SSD_UPDATE_RESPONSE,
                    IE_CAUSE_LAYER_3,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self { cause_layer_3 })
    }
}

/// Exact DTAP `Location Updating Request` payload carried inside `Layer 3 Information`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationUpdatingRequestMessage {
    pub mobile_identity_imsi: MobileIdentity,
    pub location_area_identification: Option<LocationAreaIdentification>,
    pub classmark_information_type_2: Option<ClassmarkInformationType2>,
    pub registration_type: Option<RegistrationType>,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub slot_cycle_index: Option<SlotCycleIndex>,
    pub authentication_response_parameter: Option<AuthenticationResponseParameter>,
    pub authentication_confirmation_parameter: Option<AuthenticationConfirmationParameter>,
    pub authentication_parameter_count: Option<AuthenticationParameterCount>,
    pub authentication_challenge_parameter: Option<AuthenticationChallengeParameter>,
    pub authentication_event: Option<AuthenticationEvent>,
    pub user_zone_id: Option<UserZoneId>,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
}

impl LocationUpdatingRequestMessage {
    /// Encodes the DTAP message body including protocol discriminator and message type.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![
            DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT,
            0x00,
            LOCATION_UPDATING_REQUEST,
        ];
        let mobile_identity = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "Location Updating Request",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        push_l3_lv(&mut out, &mobile_identity)?;
        if let Some(location_area_identification) = self.location_area_identification {
            push_fixed(
                &mut out,
                IE_LOCATION_AREA_IDENTIFICATION,
                &location_area_identification.encode()?,
            );
        }
        if let Some(classmark_information_type_2) = &self.classmark_information_type_2 {
            push_l3_tlv(&mut out, 0x12, classmark_information_type_2.encode()?)?;
        }
        if let Some(registration_type) = self.registration_type {
            push_fixed(&mut out, 0x1f, &[registration_type.encode()?]);
        }
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "Location Updating Request",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_l3_tlv(&mut out, IE_MOBILE_IDENTITY, &esn)?;
        }
        if let Some(slot_cycle_index) = self.slot_cycle_index {
            push_fixed(&mut out, IE_SLOT_CYCLE_INDEX, &[slot_cycle_index.encode()?]);
        }
        if let Some(authentication_response_parameter) = self.authentication_response_parameter {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_RESPONSE_PARAMETER,
                &authentication_response_parameter.encode(),
            )?;
        }
        if let Some(authentication_confirmation_parameter) =
            self.authentication_confirmation_parameter
        {
            push_fixed(
                &mut out,
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                &[authentication_confirmation_parameter.0],
            );
        }
        if let Some(authentication_parameter_count) = self.authentication_parameter_count {
            push_fixed(
                &mut out,
                IE_AUTHENTICATION_PARAMETER_COUNT,
                &[authentication_parameter_count.0 & 0x3f],
            );
        }
        if let Some(authentication_challenge_parameter) = self.authentication_challenge_parameter {
            push_l3_tlv(
                &mut out,
                IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                &authentication_challenge_parameter.encode(),
            )?;
        }
        if let Some(authentication_event) = self.authentication_event {
            push_l3_tlv(&mut out, IE_AUTHENTICATION_EVENT, &[authentication_event.0])?;
        }
        if let Some(user_zone_id) = self.user_zone_id {
            push_fixed(&mut out, IE_USER_ZONE_ID, &user_zone_id.encode());
        }
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_l3_tlv(
                &mut out,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        Ok(out)
    }

    /// Decodes the DTAP message body including protocol discriminator and message type.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let rest = parse_dtap(
            DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT,
            LOCATION_UPDATING_REQUEST,
            input,
        )?;
        let mut offset = 0;
        let (_, imsi_payload, consumed) = decode_lv(&rest[offset..])?;
        let mobile_identity_imsi = MobileIdentity::decode(imsi_payload)?;
        if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "Location Updating Request",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        offset += consumed;
        let mut location_area_identification = None;
        let mut classmark_information_type_2 = None;
        let mut registration_type = None;
        let mut mobile_identity_esn = None;
        let mut slot_cycle_index = None;
        let mut authentication_response_parameter = None;
        let mut authentication_confirmation_parameter = None;
        let mut authentication_parameter_count = None;
        let mut authentication_challenge_parameter = None;
        let mut authentication_event = None;
        let mut user_zone_id = None;
        let mut is2000_mobile_capabilities = None;
        while offset < rest.len() {
            match rest[offset] {
                IE_LOCATION_AREA_IDENTIFICATION => {
                    set_once(
                        &mut location_area_identification,
                        LocationAreaIdentification::decode(take_fixed(&rest[offset..], 5)?)?,
                        LOCATION_UPDATING_REQUEST,
                        IE_LOCATION_AREA_IDENTIFICATION,
                    )?;
                    offset += 6;
                }
                0x12 => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut classmark_information_type_2,
                        ClassmarkInformationType2::decode(payload)?,
                        LOCATION_UPDATING_REQUEST,
                        0x12,
                    )?;
                    offset += consumed;
                }
                0x1f => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut registration_type,
                        RegistrationType::decode(rest[offset + 1])?,
                        LOCATION_UPDATING_REQUEST,
                        0x1f,
                    )?;
                    offset += 2;
                }
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "Location Updating Request",
                            reason: "ESN field must contain ESN mobile identity",
                        });
                    }
                    set_once(
                        &mut mobile_identity_esn,
                        identity,
                        LOCATION_UPDATING_REQUEST,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_SLOT_CYCLE_INDEX => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut slot_cycle_index,
                        SlotCycleIndex::decode(rest[offset + 1])?,
                        LOCATION_UPDATING_REQUEST,
                        IE_SLOT_CYCLE_INDEX,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_RESPONSE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_response_parameter,
                        AuthenticationResponseParameter::decode(payload)?,
                        LOCATION_UPDATING_REQUEST,
                        IE_AUTHENTICATION_RESPONSE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_AUTHENTICATION_CONFIRMATION_PARAMETER => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut authentication_confirmation_parameter,
                        AuthenticationConfirmationParameter(rest[offset + 1]),
                        LOCATION_UPDATING_REQUEST,
                        IE_AUTHENTICATION_CONFIRMATION_PARAMETER,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_PARAMETER_COUNT => {
                    ensure_remaining(rest, offset + 1, 1)?;
                    set_once(
                        &mut authentication_parameter_count,
                        AuthenticationParameterCount(rest[offset + 1] & 0x3f),
                        LOCATION_UPDATING_REQUEST,
                        IE_AUTHENTICATION_PARAMETER_COUNT,
                    )?;
                    offset += 2;
                }
                IE_AUTHENTICATION_CHALLENGE_PARAMETER => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut authentication_challenge_parameter,
                        AuthenticationChallengeParameter::decode(payload)?,
                        LOCATION_UPDATING_REQUEST,
                        IE_AUTHENTICATION_CHALLENGE_PARAMETER,
                    )?;
                    offset += consumed;
                }
                IE_AUTHENTICATION_EVENT => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    if payload.len() != 1 {
                        return Err(Error::InvalidLength {
                            expected: 1,
                            actual: payload.len(),
                        });
                    }
                    set_once(
                        &mut authentication_event,
                        AuthenticationEvent(payload[0]),
                        LOCATION_UPDATING_REQUEST,
                        IE_AUTHENTICATION_EVENT,
                    )?;
                    offset += consumed;
                }
                IE_USER_ZONE_ID => {
                    set_once(
                        &mut user_zone_id,
                        UserZoneId::decode(take_fixed(&rest[offset..], 2)?)?,
                        LOCATION_UPDATING_REQUEST,
                        IE_USER_ZONE_ID,
                    )?;
                    offset += 3;
                }
                IE_IS2000_MOBILE_CAPABILITIES => {
                    let (_, payload, consumed) = decode_tlv(&rest[offset..])?;
                    set_once(
                        &mut is2000_mobile_capabilities,
                        Is2000MobileCapabilities::decode(payload)?,
                        LOCATION_UPDATING_REQUEST,
                        IE_IS2000_MOBILE_CAPABILITIES,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            mobile_identity_imsi,
            location_area_identification,
            classmark_information_type_2,
            registration_type,
            mobile_identity_esn,
            slot_cycle_index,
            authentication_response_parameter,
            authentication_confirmation_parameter,
            authentication_parameter_count,
            authentication_challenge_parameter,
            authentication_event,
            user_zone_id,
            is2000_mobile_capabilities,
        })
    }
}

/// Exact `Complete Layer 3 Information` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteLayer3InformationMessage {
    pub cell_identifier: CellId,
    pub layer3_information: Layer3Information,
}

impl CompleteLayer3InformationMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(
            &mut body,
            IE_CELL_IDENTIFIER,
            &self.cell_identifier.encode()?,
        )?;
        push_tlv(
            &mut body,
            IE_LAYER_3_INFORMATION,
            self.layer3_information.encode(),
        )?;
        encode_bsmap(COMPLETE_LAYER3_INFORMATION, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(COMPLETE_LAYER3_INFORMATION, input)?;
        let mut offset = 0;
        let mut cell_identifier = None;
        let mut layer3_information = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CELL_IDENTIFIER => set_once(
                    &mut cell_identifier,
                    CellId::decode(payload)?,
                    COMPLETE_LAYER3_INFORMATION,
                    IE_CELL_IDENTIFIER,
                )?,
                IE_LAYER_3_INFORMATION => set_once(
                    &mut layer3_information,
                    Layer3Information::decode(payload)?,
                    COMPLETE_LAYER3_INFORMATION,
                    IE_LAYER_3_INFORMATION,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            cell_identifier: cell_identifier.ok_or(Error::MissingRequiredElement {
                message_type: COMPLETE_LAYER3_INFORMATION,
                id: IE_CELL_IDENTIFIER,
            })?,
            layer3_information: layer3_information.ok_or(Error::MissingRequiredElement {
                message_type: COMPLETE_LAYER3_INFORMATION,
                id: IE_LAYER_3_INFORMATION,
            })?,
        })
    }
}

/// Exact `Paging Request` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagingRequestMessage {
    pub mobile_identity_imsi: MobileIdentity,
    pub tag: Option<Tag>,
    pub cell_identifier_list: Option<CellIdentifierList>,
    pub slot_cycle_index: Option<SlotCycleIndex>,
    pub service_option: Option<ServiceOption>,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
}

impl PagingRequestMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let mobile_identity = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "Paging Request",
                reason: "mobile identity must be IMSI",
            });
        }
        push_tlv(&mut body, IE_MOBILE_IDENTITY, &mobile_identity)?;
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            let payload = cell_identifier_list.encode()?;
            push_tlv(&mut body, IE_CELL_IDENTIFIER_LIST, &payload)?;
        }
        if let Some(slot_cycle_index) = self.slot_cycle_index {
            push_fixed(
                &mut body,
                IE_SLOT_CYCLE_INDEX,
                &[slot_cycle_index.encode()?],
            );
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_tlv(
                &mut body,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        encode_bsmap(PAGING_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(PAGING_REQUEST, input)?;
        let mut offset = 0;
        let mut mobile_identity_imsi = None;
        let mut tag = None;
        let mut cell_identifier_list = None;
        let mut slot_cycle_index = None;
        let mut service_option = None;
        let mut is2000_mobile_capabilities = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (id, payload, consumed) = decode_tlv(&body[offset..])?;
                    debug_assert_eq!(id, IE_MOBILE_IDENTITY);
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Imsi(_)) {
                        return Err(Error::InvalidValue {
                            context: "Paging Request",
                            reason: "mobile identity must be IMSI",
                        });
                    }
                    set_once(
                        &mut mobile_identity_imsi,
                        identity,
                        PAGING_REQUEST,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, PAGING_REQUEST, IE_TAG)?;
                    offset += 5;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cell_identifier_list,
                        CellIdentifierList::decode(payload)?,
                        PAGING_REQUEST,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += consumed;
                }
                IE_SLOT_CYCLE_INDEX => {
                    ensure_remaining(body, offset + 1, 1)?;
                    set_once(
                        &mut slot_cycle_index,
                        SlotCycleIndex::decode(body[offset + 1])?,
                        PAGING_REQUEST,
                        IE_SLOT_CYCLE_INDEX,
                    )?;
                    offset += 2;
                }
                IE_SERVICE_OPTION => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(payload)?,
                        PAGING_REQUEST,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_IS2000_MOBILE_CAPABILITIES => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_mobile_capabilities,
                        Is2000MobileCapabilities::decode(payload)?,
                        PAGING_REQUEST,
                        IE_IS2000_MOBILE_CAPABILITIES,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            mobile_identity_imsi: mobile_identity_imsi.ok_or(Error::MissingRequiredElement {
                message_type: PAGING_REQUEST,
                id: IE_MOBILE_IDENTITY,
            })?,
            tag,
            cell_identifier_list,
            slot_cycle_index,
            service_option,
            is2000_mobile_capabilities,
        })
    }
}

/// Exact `Assignment Request` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentRequestMessage {
    pub channel_type: ChannelType,
    pub circuit_identity_code: CircuitIdentityCode,
    pub encryption_information: Option<EncryptionInformation>,
    pub service_option: Option<ServiceOption>,
    pub signals: Vec<Signal>,
    pub ms_information_records: Option<MsInformationRecords>,
    pub priority: Option<Priority>,
    pub paca_timestamp: Option<PacaTimestamp>,
    pub quality_of_service_parameters: Option<QualityOfServiceParameters>,
    pub a2p_bearer_session_params: Option<A2pBearerSessionParams>,
    pub a2p_bearer_format_params: Option<A2pBearerFormatParams>,
}

impl AssignmentRequestMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CHANNEL_TYPE, &self.channel_type.encode())?;
        push_fixed(
            &mut body,
            IE_CIRCUIT_IDENTITY_CODE,
            &self.circuit_identity_code.encode()?,
        );
        if let Some(encryption_information) = &self.encryption_information {
            let payload = encryption_information.encode()?;
            push_tlv(&mut body, IE_ENCRYPTION_INFORMATION, &payload)?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        for signal in &self.signals {
            push_fixed(&mut body, IE_SIGNAL, &[signal.encode()?]);
        }
        if let Some(ms_information_records) = &self.ms_information_records {
            let payload = ms_information_records.encode()?;
            push_tlv(&mut body, IE_MS_INFORMATION_RECORDS, &payload)?;
        }
        if let Some(priority) = self.priority {
            push_tlv(&mut body, IE_PRIORITY, &priority.encode()?)?;
        }
        if let Some(paca_timestamp) = self.paca_timestamp {
            push_tlv(&mut body, IE_PACA_TIMESTAMP, &paca_timestamp.encode())?;
        }
        if let Some(quality_of_service_parameters) = self.quality_of_service_parameters {
            push_tlv(
                &mut body,
                IE_QUALITY_OF_SERVICE_PARAMETERS,
                &quality_of_service_parameters.encode()?,
            )?;
        }
        if let Some(params) = self.a2p_bearer_session_params {
            push_tlv(&mut body, IE_A2P_BEARER_SESSION_PARAMS, &params.encode())?;
        }
        if let Some(params) = &self.a2p_bearer_format_params {
            push_tlv(&mut body, IE_A2P_BEARER_FORMAT_PARAMS, &params.encode())?;
        }
        encode_bsmap(ASSIGNMENT_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ASSIGNMENT_REQUEST, input)?;
        let mut offset = 0;
        let mut channel_type = None;
        let mut circuit_identity_code = None;
        let mut encryption_information = None;
        let mut service_option = None;
        let mut signals = Vec::new();
        let mut ms_information_records = None;
        let mut priority = None;
        let mut paca_timestamp = None;
        let mut quality_of_service_parameters = None;
        let mut a2p_bearer_session_params = None;
        let mut a2p_bearer_format_params = None;
        while offset < body.len() {
            match body[offset] {
                IE_CHANNEL_TYPE => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut channel_type,
                        ChannelType::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_CHANNEL_TYPE,
                    )?;
                    offset += consumed;
                }
                IE_CIRCUIT_IDENTITY_CODE => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut circuit_identity_code,
                        CircuitIdentityCode::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_CIRCUIT_IDENTITY_CODE,
                    )?;
                    offset += 3;
                }
                IE_ENCRYPTION_INFORMATION => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut encryption_information,
                        EncryptionInformation::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_ENCRYPTION_INFORMATION,
                    )?;
                    offset += consumed;
                }
                IE_SERVICE_OPTION => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_SIGNAL => {
                    ensure_remaining(body, offset + 1, 1)?;
                    signals.push(Signal::decode(body[offset + 1]));
                    offset += 2;
                }
                IE_MS_INFORMATION_RECORDS => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut ms_information_records,
                        MsInformationRecords::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_MS_INFORMATION_RECORDS,
                    )?;
                    offset += consumed;
                }
                IE_PRIORITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut priority,
                        Priority::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_PRIORITY,
                    )?;
                    offset += consumed;
                }
                IE_PACA_TIMESTAMP => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut paca_timestamp,
                        PacaTimestamp::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_PACA_TIMESTAMP,
                    )?;
                    offset += consumed;
                }
                IE_QUALITY_OF_SERVICE_PARAMETERS => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut quality_of_service_parameters,
                        QualityOfServiceParameters::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_QUALITY_OF_SERVICE_PARAMETERS,
                    )?;
                    offset += consumed;
                }
                IE_A2P_BEARER_SESSION_PARAMS => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut a2p_bearer_session_params,
                        A2pBearerSessionParams::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_A2P_BEARER_SESSION_PARAMS,
                    )?;
                    offset += consumed;
                }
                IE_A2P_BEARER_FORMAT_PARAMS => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut a2p_bearer_format_params,
                        A2pBearerFormatParams::decode(payload)?,
                        ASSIGNMENT_REQUEST,
                        IE_A2P_BEARER_FORMAT_PARAMS,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            channel_type: channel_type.ok_or(Error::MissingRequiredElement {
                message_type: ASSIGNMENT_REQUEST,
                id: IE_CHANNEL_TYPE,
            })?,
            circuit_identity_code: circuit_identity_code.ok_or(Error::MissingRequiredElement {
                message_type: ASSIGNMENT_REQUEST,
                id: IE_CIRCUIT_IDENTITY_CODE,
            })?,
            encryption_information,
            service_option,
            signals,
            ms_information_records,
            priority,
            paca_timestamp,
            quality_of_service_parameters,
            a2p_bearer_session_params,
            a2p_bearer_format_params,
        })
    }
}

/// Exact `Assignment Complete` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentCompleteMessage {
    pub channel_number: ChannelNumber,
    pub encryption_information: Option<EncryptionInformation>,
    pub service_option: Option<ServiceOption>,
    pub a2p_bearer_session_params: Option<A2pBearerSessionParams>,
    pub a2p_bearer_format_params: Option<A2pBearerFormatParams>,
}

impl AssignmentCompleteMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_fixed(&mut body, IE_CHANNEL_NUMBER, &self.channel_number.encode());
        if let Some(encryption_information) = &self.encryption_information {
            let payload = encryption_information.encode()?;
            push_tlv(&mut body, IE_ENCRYPTION_INFORMATION, &payload)?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        if let Some(params) = self.a2p_bearer_session_params {
            push_tlv(&mut body, IE_A2P_BEARER_SESSION_PARAMS, &params.encode())?;
        }
        if let Some(params) = &self.a2p_bearer_format_params {
            push_tlv(&mut body, IE_A2P_BEARER_FORMAT_PARAMS, &params.encode())?;
        }
        encode_bsmap(ASSIGNMENT_COMPLETE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ASSIGNMENT_COMPLETE, input)?;
        let mut offset = 0;
        let mut channel_number = None;
        let mut encryption_information = None;
        let mut service_option = None;
        let mut a2p_bearer_session_params = None;
        let mut a2p_bearer_format_params = None;
        while offset < body.len() {
            match body[offset] {
                IE_CHANNEL_NUMBER => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut channel_number,
                        ChannelNumber::decode(payload)?,
                        ASSIGNMENT_COMPLETE,
                        IE_CHANNEL_NUMBER,
                    )?;
                    offset += 3;
                }
                IE_ENCRYPTION_INFORMATION => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut encryption_information,
                        EncryptionInformation::decode(payload)?,
                        ASSIGNMENT_COMPLETE,
                        IE_ENCRYPTION_INFORMATION,
                    )?;
                    offset += consumed;
                }
                IE_SERVICE_OPTION => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(payload)?,
                        ASSIGNMENT_COMPLETE,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_A2P_BEARER_SESSION_PARAMS => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut a2p_bearer_session_params,
                        A2pBearerSessionParams::decode(payload)?,
                        ASSIGNMENT_COMPLETE,
                        IE_A2P_BEARER_SESSION_PARAMS,
                    )?;
                    offset += consumed;
                }
                IE_A2P_BEARER_FORMAT_PARAMS => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut a2p_bearer_format_params,
                        A2pBearerFormatParams::decode(payload)?,
                        ASSIGNMENT_COMPLETE,
                        IE_A2P_BEARER_FORMAT_PARAMS,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            channel_number: channel_number.ok_or(Error::MissingRequiredElement {
                message_type: ASSIGNMENT_COMPLETE,
                id: IE_CHANNEL_NUMBER,
            })?,
            encryption_information,
            service_option,
            a2p_bearer_session_params,
            a2p_bearer_format_params,
        })
    }
}

/// Exact `Assignment Failure` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssignmentFailureMessage {
    pub cause: Cause,
}

impl AssignmentFailureMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        encode_bsmap(ASSIGNMENT_FAILURE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ASSIGNMENT_FAILURE, input)?;
        let mut offset = 0;
        let mut cause = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CAUSE => set_once(
                    &mut cause,
                    Cause::decode(payload)?,
                    ASSIGNMENT_FAILURE,
                    IE_CAUSE,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: ASSIGNMENT_FAILURE,
                id: IE_CAUSE,
            })?,
        })
    }
}

/// Exact `Clear Request` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearRequestMessage {
    pub cause: Cause,
    pub cause_layer3: Option<CauseLayer3>,
}

impl ClearRequestMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        if let Some(cause_layer3) = self.cause_layer3 {
            push_tlv(&mut body, IE_CAUSE_LAYER_3, &cause_layer3.encode()?)?;
        }
        encode_bsmap(CLEAR_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(CLEAR_REQUEST, input)?;
        let mut offset = 0;
        let mut cause = None;
        let mut cause_layer3 = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CAUSE => set_once(&mut cause, Cause::decode(payload)?, CLEAR_REQUEST, IE_CAUSE)?,
                IE_CAUSE_LAYER_3 => set_once(
                    &mut cause_layer3,
                    CauseLayer3::decode(payload)?,
                    CLEAR_REQUEST,
                    IE_CAUSE_LAYER_3,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: CLEAR_REQUEST,
                id: IE_CAUSE,
            })?,
            cause_layer3,
        })
    }
}

/// Exact `Clear Command` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearCommandMessage {
    pub cause: Cause,
    pub cause_layer3: Option<CauseLayer3>,
}

impl ClearCommandMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        if let Some(cause_layer3) = self.cause_layer3 {
            push_tlv(&mut body, IE_CAUSE_LAYER_3, &cause_layer3.encode()?)?;
        }
        encode_bsmap(CLEAR_COMMAND, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(CLEAR_COMMAND, input)?;
        let mut offset = 0;
        let mut cause = None;
        let mut cause_layer3 = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CAUSE => set_once(&mut cause, Cause::decode(payload)?, CLEAR_COMMAND, IE_CAUSE)?,
                IE_CAUSE_LAYER_3 => set_once(
                    &mut cause_layer3,
                    CauseLayer3::decode(payload)?,
                    CLEAR_COMMAND,
                    IE_CAUSE_LAYER_3,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: CLEAR_COMMAND,
                id: IE_CAUSE,
            })?,
            cause_layer3,
        })
    }
}

/// Exact `Clear Complete` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearCompleteMessage {
    pub power_down_indicator: bool,
}

impl ClearCompleteMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if self.power_down_indicator {
            body.push(IE_POWER_DOWN_INDICATOR);
        }
        encode_bsmap(CLEAR_COMPLETE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(CLEAR_COMPLETE, input)?;
        let mut power_down_indicator = false;
        let mut offset = 0;
        while offset < body.len() {
            match body[offset] {
                IE_POWER_DOWN_INDICATOR => {
                    set_marker_once(
                        &mut power_down_indicator,
                        CLEAR_COMPLETE,
                        IE_POWER_DOWN_INDICATOR,
                    )?;
                    offset += 1;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            power_down_indicator,
        })
    }
}

/// Exact `Connect` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConnectMessage;

impl ConnectMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_dtap(CONNECT, &[])
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_dtap(CONNECT, input)?;
        if !body.is_empty() {
            return Err(Error::InvalidLength {
                expected: 5,
                actual: input.len(),
            });
        }
        Ok(Self)
    }
}

/// Exact `Progress` DTAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressMessage {
    pub signal: Option<Signal>,
    pub ms_information_records: Option<MsInformationRecords>,
}

impl ProgressMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.signal.is_some() && self.ms_information_records.is_some() {
            return Err(Error::InvalidValue {
                context: "Progress",
                reason: "signal and MS information records are mutually exclusive",
            });
        }
        let mut body = Vec::new();
        if let Some(signal) = self.signal {
            push_fixed(&mut body, IE_SIGNAL, &[signal.encode()?]);
        }
        if let Some(ms_information_records) = &self.ms_information_records {
            let payload = ms_information_records.encode()?;
            push_tlv(&mut body, IE_MS_INFORMATION_RECORDS, &payload)?;
        }
        encode_a1_dtap(PROGRESS, &body)
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_dtap(PROGRESS, input)?;
        let mut signal = None;
        let mut ms_information_records = None;
        let mut offset = 0;
        while offset < body.len() {
            match body[offset] {
                IE_SIGNAL => {
                    if ms_information_records.is_some() {
                        return Err(Error::InvalidValue {
                            context: "Progress",
                            reason: "signal and MS information records are mutually exclusive",
                        });
                    }
                    ensure_remaining(body, offset + 1, 1)?;
                    set_once(
                        &mut signal,
                        Signal::decode(body[offset + 1]),
                        PROGRESS,
                        IE_SIGNAL,
                    )?;
                    offset += 2;
                }
                IE_MS_INFORMATION_RECORDS => {
                    if signal.is_some() {
                        return Err(Error::InvalidValue {
                            context: "Progress",
                            reason: "signal and MS information records are mutually exclusive",
                        });
                    }
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut ms_information_records,
                        MsInformationRecords::decode(payload)?,
                        PROGRESS,
                        IE_MS_INFORMATION_RECORDS,
                    )?;
                    offset += consumed;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            signal,
            ms_information_records,
        })
    }
}

/// Exact `Alert With Information` DTAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertWithInformationMessage {
    pub ms_information_records: Option<MsInformationRecords>,
}

impl AlertWithInformationMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if let Some(ms_information_records) = &self.ms_information_records {
            let payload = ms_information_records.encode()?;
            push_tlv(&mut body, IE_MS_INFORMATION_RECORDS, &payload)?;
        }
        encode_a1_dtap(ALERT_WITH_INFORMATION, &body)
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_dtap(ALERT_WITH_INFORMATION, input)?;
        let mut ms_information_records = None;
        let mut offset = 0;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_MS_INFORMATION_RECORDS => set_once(
                    &mut ms_information_records,
                    MsInformationRecords::decode(payload)?,
                    ALERT_WITH_INFORMATION,
                    IE_MS_INFORMATION_RECORDS,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            ms_information_records,
        })
    }
}

/// Exact `Parameter Update Request` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterUpdateRequestMessage;

impl ParameterUpdateRequestMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(PARAMETER_UPDATE_REQUEST, &[])
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(PARAMETER_UPDATE_REQUEST, input)?;
        if !body.is_empty() {
            return Err(Error::InvalidLength {
                expected: 0,
                actual: body.len(),
            });
        }
        Ok(Self)
    }
}

/// Exact `Parameter Update Confirm` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterUpdateConfirmMessage;

impl ParameterUpdateConfirmMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(PARAMETER_UPDATE_CONFIRM, &[])
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_mm_dtap(PARAMETER_UPDATE_CONFIRM, input)?;
        if !body.is_empty() {
            return Err(Error::InvalidLength {
                expected: 0,
                actual: body.len(),
            });
        }
        Ok(Self)
    }
}

/// Exact `Location Updating Accept` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocationUpdatingAcceptMessage {
    pub location_area_identification: Option<LocationAreaIdentification>,
}

impl LocationUpdatingAcceptMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if let Some(location_area_identification) = self.location_area_identification {
            push_fixed(
                &mut body,
                IE_LOCATION_AREA_IDENTIFICATION,
                &location_area_identification.encode()?,
            );
        }
        encode_a1_mm_dtap(LOCATION_UPDATING_ACCEPT, &body)
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let rest = parse_a1_mm_dtap(LOCATION_UPDATING_ACCEPT, input)?;
        let mut offset = 0;
        let mut location_area_identification = None;
        while offset < rest.len() {
            match rest[offset] {
                IE_LOCATION_AREA_IDENTIFICATION => {
                    set_once(
                        &mut location_area_identification,
                        LocationAreaIdentification::decode(take_fixed(&rest[offset..], 5)?)?,
                        LOCATION_UPDATING_ACCEPT,
                        IE_LOCATION_AREA_IDENTIFICATION,
                    )?;
                    offset += 6;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            location_area_identification,
        })
    }
}

/// Exact `Location Updating Reject` DTAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocationUpdatingRejectMessage {
    pub reject_cause: RejectCause,
}

impl LocationUpdatingRejectMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_a1_mm_dtap(LOCATION_UPDATING_REJECT, &[self.reject_cause.encode()?])
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let rest = parse_a1_mm_dtap(LOCATION_UPDATING_REJECT, input)?;
        if rest.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: rest.len(),
            });
        }
        Ok(Self {
            reject_cause: RejectCause::decode(rest[0])?,
        })
    }
}

/// Exact `User Zone Update` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserZoneUpdateMessage {
    pub user_zone_id: Option<UserZoneId>,
}

impl UserZoneUpdateMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if let Some(user_zone_id) = self.user_zone_id {
            push_tlv(&mut body, IE_USER_ZONE_ID, &user_zone_id.encode())?;
        }
        encode_bsmap(USER_ZONE_UPDATE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(USER_ZONE_UPDATE, input)?;
        let mut offset = 0;
        let mut user_zone_id = None;
        while offset < body.len() {
            match body[offset] {
                IE_USER_ZONE_ID => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut user_zone_id,
                        UserZoneId::decode(payload)?,
                        USER_ZONE_UPDATE,
                        IE_USER_ZONE_ID,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self { user_zone_id })
    }
}

/// Exact `Privacy Mode Command` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyModeCommandMessage {
    pub encryption_information: EncryptionInformation,
}

impl PrivacyModeCommandMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(
            &mut body,
            IE_ENCRYPTION_INFORMATION,
            &self.encryption_information.encode()?,
        )?;
        encode_bsmap(PRIVACY_MODE_COMMAND, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(PRIVACY_MODE_COMMAND, input)?;
        let mut offset = 0;
        let mut encryption_information = None;
        while offset < body.len() {
            match body[offset] {
                IE_ENCRYPTION_INFORMATION => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut encryption_information,
                        EncryptionInformation::decode(payload)?,
                        PRIVACY_MODE_COMMAND,
                        IE_ENCRYPTION_INFORMATION,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            encryption_information: encryption_information.ok_or(
                Error::MissingRequiredElement {
                    message_type: PRIVACY_MODE_COMMAND,
                    id: IE_ENCRYPTION_INFORMATION,
                },
            )?,
        })
    }
}

/// Exact `Privacy Mode Complete` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyModeCompleteMessage {
    pub encryption_information: Option<EncryptionInformation>,
    pub voice_privacy_request: bool,
}

impl PrivacyModeCompleteMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.encryption_information.is_some() && self.voice_privacy_request {
            return Err(Error::InvalidValue {
                context: "Privacy Mode Complete",
                reason: "encryption information and voice privacy request are mutually exclusive",
            });
        }
        let mut body = Vec::new();
        if let Some(encryption_information) = &self.encryption_information {
            push_tlv(
                &mut body,
                IE_ENCRYPTION_INFORMATION,
                &encryption_information.encode()?,
            )?;
        }
        if self.voice_privacy_request {
            body.push(IE_VOICE_PRIVACY_REQUEST);
        }
        encode_bsmap(PRIVACY_MODE_COMPLETE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(PRIVACY_MODE_COMPLETE, input)?;
        let mut offset = 0;
        let mut encryption_information = None;
        let mut voice_privacy_request = false;
        while offset < body.len() {
            match body[offset] {
                IE_ENCRYPTION_INFORMATION => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut encryption_information,
                        EncryptionInformation::decode(payload)?,
                        PRIVACY_MODE_COMPLETE,
                        IE_ENCRYPTION_INFORMATION,
                    )?;
                    offset += used;
                }
                IE_VOICE_PRIVACY_REQUEST => {
                    set_marker_once(
                        &mut voice_privacy_request,
                        PRIVACY_MODE_COMPLETE,
                        IE_VOICE_PRIVACY_REQUEST,
                    )?;
                    offset += 1;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        if encryption_information.is_some() && voice_privacy_request {
            return Err(Error::InvalidValue {
                context: "Privacy Mode Complete",
                reason: "encryption information and voice privacy request are mutually exclusive",
            });
        }
        Ok(Self {
            encryption_information,
            voice_privacy_request,
        })
    }
}

/// Exact `BS Service Request` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsServiceRequestMessage {
    pub mobile_identity_imsi: MobileIdentity,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub service_option: Option<ServiceOption>,
    pub tag: Option<Tag>,
}

impl BsServiceRequestMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let imsi = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "BS Service Request",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        push_l3_lv(&mut body, &imsi)?;
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "BS Service Request",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &esn)?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        encode_bsmap(BS_SERVICE_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(BS_SERVICE_REQUEST, input)?;
        let (_, imsi_payload, consumed) = decode_lv(body)?;
        let mobile_identity_imsi = MobileIdentity::decode(imsi_payload)?;
        if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "BS Service Request",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        let mut offset = consumed;
        let mut mobile_identity_esn = None;
        let mut service_option = None;
        let mut tag = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "BS Service Request",
                            reason: "ESN field must contain ESN mobile identity",
                        });
                    }
                    set_once(
                        &mut mobile_identity_esn,
                        identity,
                        BS_SERVICE_REQUEST,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_SERVICE_OPTION => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(payload)?,
                        BS_SERVICE_REQUEST,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, BS_SERVICE_REQUEST, IE_TAG)?;
                    offset += 5;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            mobile_identity_imsi,
            mobile_identity_esn,
            service_option,
            tag,
        })
    }
}

/// Exact `BS Service Response` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsServiceResponseMessage {
    pub mobile_identity_imsi: MobileIdentity,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub tag: Option<Tag>,
    pub cause: Option<Cause>,
}

impl BsServiceResponseMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let imsi = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "BS Service Response",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        push_l3_lv(&mut body, &imsi)?;
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "BS Service Response",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &esn)?;
        }
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(cause) = self.cause {
            push_tlv(&mut body, IE_CAUSE, &cause.encode())?;
        }
        encode_bsmap(BS_SERVICE_RESPONSE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(BS_SERVICE_RESPONSE, input)?;
        let (_, imsi_payload, consumed) = decode_lv(body)?;
        let mobile_identity_imsi = MobileIdentity::decode(imsi_payload)?;
        if !matches!(mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "BS Service Response",
                reason: "mobile identity IMSI field must contain IMSI",
            });
        }
        let mut offset = consumed;
        let mut mobile_identity_esn = None;
        let mut tag = None;
        let mut cause = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "BS Service Response",
                            reason: "ESN field must contain ESN mobile identity",
                        });
                    }
                    set_once(
                        &mut mobile_identity_esn,
                        identity,
                        BS_SERVICE_RESPONSE,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, BS_SERVICE_RESPONSE, IE_TAG)?;
                    offset += 5;
                }
                IE_CAUSE => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cause,
                        Cause::decode(payload)?,
                        BS_SERVICE_RESPONSE,
                        IE_CAUSE,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            mobile_identity_imsi,
            mobile_identity_esn,
            tag,
            cause,
        })
    }
}

/// Exact `Handoff Failure` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffFailureMessage {
    pub cause: Cause,
}

impl HandoffFailureMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        encode_bsmap(HANDOFF_FAILURE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_FAILURE, input)?;
        let mut offset = 0;
        let mut cause = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CAUSE => set_once(
                    &mut cause,
                    Cause::decode(payload)?,
                    HANDOFF_FAILURE,
                    IE_CAUSE,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: HANDOFF_FAILURE,
                id: IE_CAUSE,
            })?,
        })
    }
}

/// Exact `Handoff Required Reject` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffRequiredRejectMessage {
    pub cause: Cause,
}

impl HandoffRequiredRejectMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        encode_bsmap(HANDOFF_REQUIRED_REJECT, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_REQUIRED_REJECT, input)?;
        let mut offset = 0;
        let mut cause = None;
        while offset < body.len() {
            let (id, payload, consumed) = decode_tlv(&body[offset..])?;
            match id {
                IE_CAUSE => set_once(
                    &mut cause,
                    Cause::decode(payload)?,
                    HANDOFF_REQUIRED_REJECT,
                    IE_CAUSE,
                )?,
                other => return Err(Error::UnknownInformationElement(other)),
            }
            offset += consumed;
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: HANDOFF_REQUIRED_REJECT,
                id: IE_CAUSE,
            })?,
        })
    }
}

/// Exact `Handoff Commenced` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffCommencedMessage;

impl HandoffCommencedMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_bsmap(HANDOFF_COMMENCED, &[])
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_COMMENCED, input)?;
        if !body.is_empty() {
            return Err(Error::InvalidLength {
                expected: 0,
                actual: body.len(),
            });
        }
        Ok(Self)
    }
}

/// Exact `Handoff Complete` BSMAP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffCompleteMessage;

impl HandoffCompleteMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_bsmap(HANDOFF_COMPLETE, &[])
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_COMPLETE, input)?;
        if !body.is_empty() {
            return Err(Error::InvalidLength {
                expected: 0,
                actual: body.len(),
            });
        }
        Ok(Self)
    }
}

/// Exact `Handoff Performed` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffPerformedMessage {
    pub cause: Cause,
    pub cell_identifier_list: Option<HandoffCellIdentifierList>,
}

impl HandoffPerformedMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        validate_handoff_performed_cause(self.cause)?;

        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            push_tlv(
                &mut body,
                IE_CELL_IDENTIFIER_LIST,
                &cell_identifier_list.encode()?,
            )?;
        }
        encode_bsmap(HANDOFF_PERFORMED, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_PERFORMED, input)?;
        let mut offset = 0;
        let mut cause = None;
        let mut cell_identifier_list = None;
        while offset < body.len() {
            match body[offset] {
                IE_CAUSE => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    let value = Cause::decode(payload)?;
                    validate_handoff_performed_cause(value)?;
                    set_once(&mut cause, value, HANDOFF_PERFORMED, IE_CAUSE)?;
                    offset += used;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cell_identifier_list,
                        HandoffCellIdentifierList::decode(payload)?,
                        HANDOFF_PERFORMED,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: HANDOFF_PERFORMED,
                id: IE_CAUSE,
            })?,
            cell_identifier_list,
        })
    }
}

/// Exact handoff cell identifier using discriminator `0x02` or `0x07`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffCellIdentifier {
    Cell(CellId),
    CellWithMscId { mscid: u32, cell: u16, sector: u8 },
}

impl HandoffCellIdentifier {
    fn encode(self) -> Result<Vec<u8>> {
        match self {
            Self::Cell(cell) => Ok(cell.encode()?.to_vec()),
            Self::CellWithMscId {
                mscid,
                cell,
                sector,
            } => {
                if mscid > 0x00ff_ffff || !(1..=0x0fff).contains(&cell) || sector > 0x0f {
                    return Err(Error::InvalidValue {
                        context: "Handoff Cell Identifier",
                        reason: "mscid/cell/sector out of range",
                    });
                }
                Ok(vec![
                    0x07,
                    ((mscid >> 16) & 0xff) as u8,
                    ((mscid >> 8) & 0xff) as u8,
                    (mscid & 0xff) as u8,
                    (cell >> 4) as u8,
                    (((cell & 0x000f) as u8) << 4) | (sector & 0x0f),
                ])
            }
        }
    }

    fn decode(discriminator: u8, input: &[u8]) -> Result<(Self, usize)> {
        match discriminator {
            0x02 => {
                if input.len() < 2 {
                    return Err(Error::Truncated {
                        needed: 2,
                        actual: input.len(),
                    });
                }
                Ok((Self::Cell(CellId::decode(&[0x02, input[0], input[1]])?), 2))
            }
            0x07 => {
                if input.len() < 5 {
                    return Err(Error::Truncated {
                        needed: 5,
                        actual: input.len(),
                    });
                }
                Ok((
                    Self::CellWithMscId {
                        mscid: ((input[0] as u32) << 16)
                            | ((input[1] as u32) << 8)
                            | input[2] as u32,
                        cell: ((input[3] as u16) << 4) | ((input[4] as u16) >> 4),
                        sector: input[4] & 0x0f,
                    },
                    5,
                ))
            }
            other => Err(Error::ReservedValue {
                context: "Handoff Cell Identifier discriminator",
                value: other,
            }),
        }
    }
}

/// Exact handoff cell-identifier-list payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffCellIdentifierList {
    pub cells: Vec<HandoffCellIdentifier>,
}

impl HandoffCellIdentifierList {
    /// Encodes the handoff cell-identifier-list payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let Some(first) = self.cells.first().copied() else {
            return Err(Error::InvalidValue {
                context: "Handoff Cell Identifier List",
                reason: "must contain at least one cell",
            });
        };
        let discriminator = match first {
            HandoffCellIdentifier::Cell(_) => 0x02,
            HandoffCellIdentifier::CellWithMscId { .. } => 0x07,
        };
        let mut out = Vec::new();
        out.push(discriminator);
        for cell in &self.cells {
            let encoded = cell.encode()?;
            if encoded[0] != discriminator {
                return Err(Error::InvalidValue {
                    context: "Handoff Cell Identifier List",
                    reason: "all cells must use the same discriminator",
                });
            }
            out.extend_from_slice(&encoded[1..]);
        }
        Ok(out)
    }

    /// Decodes the handoff cell-identifier-list payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&discriminator, rest)) = input.split_first() else {
            return Err(Error::InvalidValue {
                context: "Handoff Cell Identifier List",
                reason: "payload must not be empty",
            });
        };
        let mut cells = Vec::new();
        let mut offset = 0usize;
        while offset < rest.len() {
            let (cell, used) = HandoffCellIdentifier::decode(discriminator, &rest[offset..])?;
            cells.push(cell);
            offset += used;
        }
        if cells.is_empty() {
            return Err(Error::InvalidValue {
                context: "Handoff Cell Identifier List",
                reason: "must contain at least one cell",
            });
        }
        Ok(Self { cells })
    }
}

/// Exact handoff downlink radio-environment record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffDownlinkRadioEnvironmentRecord {
    pub cell: HandoffCellIdentifier,
    pub downlink_signal_strength_raw: u8,
    pub cdma_target_one_way_delay: u16,
}

/// Exact handoff `Downlink Radio Environment` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffDownlinkRadioEnvironment {
    pub records: Vec<HandoffDownlinkRadioEnvironmentRecord>,
}

impl HandoffDownlinkRadioEnvironment {
    /// Encodes the downlink-radio-environment payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let Some(first) = self.records.first() else {
            return Err(Error::InvalidValue {
                context: "Downlink Radio Environment",
                reason: "must contain at least one record",
            });
        };
        let discriminator = match first.cell {
            HandoffCellIdentifier::Cell(_) => 0x02,
            HandoffCellIdentifier::CellWithMscId { .. } => 0x07,
        };
        let mut out = Vec::new();
        out.push(self.records.len() as u8);
        out.push(discriminator);
        for record in &self.records {
            let cell = record.cell.encode()?;
            if cell[0] != discriminator {
                return Err(Error::InvalidValue {
                    context: "Downlink Radio Environment",
                    reason: "all records must use the same discriminator",
                });
            }
            out.extend_from_slice(&cell[1..]);
            out.push(record.downlink_signal_strength_raw & 0x3f);
            out.extend_from_slice(&record.cdma_target_one_way_delay.to_be_bytes());
        }
        Ok(out)
    }

    /// Decodes the downlink-radio-environment payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let count = input[0] as usize;
        let discriminator = input[1];
        let record_cell_len = match discriminator {
            0x02 => 2,
            0x07 => 5,
            other => {
                return Err(Error::ReservedValue {
                    context: "Downlink Radio Environment discriminator",
                    value: other,
                });
            }
        };
        let record_len = record_cell_len + 3;
        if input.len() != 2 + count * record_len {
            return Err(Error::InvalidLength {
                expected: 2 + count * record_len,
                actual: input.len(),
            });
        }
        let mut records = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            let (cell, used) = HandoffCellIdentifier::decode(discriminator, &input[offset..])?;
            offset += used;
            records.push(HandoffDownlinkRadioEnvironmentRecord {
                cell,
                downlink_signal_strength_raw: input[offset] & 0x3f,
                cdma_target_one_way_delay: u16::from_be_bytes([
                    input[offset + 1],
                    input[offset + 2],
                ]),
            });
            offset += 3;
        }
        Ok(Self { records })
    }
}

/// Exact handoff `CDMA Serving One Way Delay` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffCdmaServingOneWayDelay {
    pub cell: HandoffCellIdentifier,
    pub delay_100ns: u16,
}

impl HandoffCdmaServingOneWayDelay {
    /// Encodes the delay payload.
    pub fn encode(self) -> Result<Vec<u8>> {
        let mut out = self.cell.encode()?;
        out.extend_from_slice(&self.delay_100ns.to_be_bytes());
        Ok(out)
    }

    /// Decodes the delay payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some(&discriminator) = input.first() else {
            return Err(Error::Truncated {
                needed: 1,
                actual: 0,
            });
        };
        let cell_total_len = match discriminator {
            0x02 => 3,
            0x07 => 6,
            other => {
                return Err(Error::ReservedValue {
                    context: "CDMA Serving One Way Delay discriminator",
                    value: other,
                });
            }
        };
        if input.len() != cell_total_len + 2 {
            return Err(Error::InvalidLength {
                expected: cell_total_len + 2,
                actual: input.len(),
            });
        }
        let (cell, _) = HandoffCellIdentifier::decode(discriminator, &input[1..cell_total_len])?;
        Ok(Self {
            cell,
            delay_100ns: u16::from_be_bytes([input[cell_total_len], input[cell_total_len + 1]]),
        })
    }
}

/// Exact `Response Request` presence indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResponseRequest;

/// Exact `IS-95 MS Measured Channel Identity` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Is95MsMeasuredChannelIdentity {
    pub band_class: u8,
    pub arfcn: u16,
}

impl Is95MsMeasuredChannelIdentity {
    /// Encodes the payload.
    pub fn encode(self) -> Result<[u8; 2]> {
        if self.band_class > 0x1f || self.arfcn > 0x07ff {
            return Err(Error::InvalidValue {
                context: "IS-95 MS Measured Channel Identity",
                reason: "band class or ARFCN out of range",
            });
        }
        Ok([
            (self.band_class << 3) | ((self.arfcn >> 8) as u8 & 0x07),
            self.arfcn as u8,
        ])
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self {
            band_class: input[0] >> 3,
            arfcn: (((input[0] & 0x07) as u16) << 8) | input[1] as u16,
        })
    }
}

/// Exact `PDSN IP Address` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdsnIpAddress(pub [u8; 4]);

impl PdsnIpAddress {
    /// Encodes the payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3]]))
    }
}

/// Exact `Protocol Type` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolType(pub u16);

impl ProtocolType {
    /// Encodes the payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact `Circuit Identity Code Extension` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitIdentityCodeExtension {
    pub circuit_identity_code: CircuitIdentityCode,
    pub circuit_mode: u8,
}

impl CircuitIdentityCodeExtension {
    /// Encodes the payload.
    pub fn encode(self) -> Result<[u8; 3]> {
        if self.circuit_mode > 0x0f {
            return Err(Error::InvalidValue {
                context: "Circuit Identity Code Extension",
                reason: "circuit mode out of range",
            });
        }
        let cic = self.circuit_identity_code.encode()?;
        Ok([cic[0], cic[1], self.circuit_mode & 0x0f])
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 3 {
            return Err(Error::InvalidLength {
                expected: 3,
                actual: input.len(),
            });
        }
        Ok(Self {
            circuit_identity_code: CircuitIdentityCode::decode(&input[..2])?,
            circuit_mode: input[2] & 0x0f,
        })
    }
}

/// One TIA/EIA-95 channel assignment entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Is95ChannelEntry {
    pub walsh_code_channel_index: u8,
    pub pilot_pn: u16,
    pub power_combined: bool,
    pub arfcn: Option<u16>,
}

/// Exact `IS-95 Channel Identity` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is95ChannelIdentity {
    pub hard_handoff: bool,
    pub frame_offset: u8,
    pub channels: Vec<Is95ChannelEntry>,
}

impl Is95ChannelIdentity {
    /// Encodes the payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.channels.is_empty() || self.channels.len() > 0x07 {
            return Err(Error::InvalidValue {
                context: "IS-95 Channel Identity",
                reason: "must contain 1..=7 channels",
            });
        }
        let mut out = Vec::with_capacity(2 + self.channels.len() * 4);
        out.push(((self.hard_handoff as u8) << 7) | (self.channels.len() as u8 & 0x07));
        out.push(self.frame_offset & 0x0f);
        for (index, channel) in self.channels.iter().enumerate() {
            if channel.pilot_pn > 0x01ff {
                return Err(Error::InvalidValue {
                    context: "IS-95 Channel Identity",
                    reason: "pilot PN out of range",
                });
            }
            let (freq_included, arfcn) = match channel.arfcn {
                Some(arfcn) => (true, arfcn),
                None => (false, 0),
            };
            out.push(channel.walsh_code_channel_index);
            out.push((channel.pilot_pn & 0xff) as u8);
            out.push(
                (((channel.pilot_pn >> 8) as u8) & 0x01)
                    | ((channel.power_combined as u8) << 1)
                    | ((freq_included as u8) << 3)
                    | (((arfcn >> 8) as u8 & 0x07) << 4),
            );
            out.push(arfcn as u8);
            if index == 0 && channel.power_combined {
                return Err(Error::InvalidValue {
                    context: "IS-95 Channel Identity",
                    reason: "first channel cannot set power combined",
                });
            }
        }
        Ok(out)
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let channel_count = (input[0] & 0x07) as usize;
        if channel_count == 0 || input.len() != 2 + channel_count * 4 {
            return Err(Error::InvalidLength {
                expected: 2 + channel_count * 4,
                actual: input.len(),
            });
        }
        let mut channels = Vec::with_capacity(channel_count);
        let mut offset = 2;
        for _ in 0..channel_count {
            let arfcn = (((input[offset + 2] >> 4) & 0x07) as u16) << 8 | input[offset + 3] as u16;
            channels.push(Is95ChannelEntry {
                walsh_code_channel_index: input[offset],
                pilot_pn: (((input[offset + 2] & 0x01) as u16) << 8) | input[offset + 1] as u16,
                power_combined: input[offset + 2] & 0x02 != 0,
                arfcn: if input[offset + 2] & 0x08 != 0 {
                    Some(arfcn)
                } else {
                    None
                },
            });
            offset += 4;
        }
        Ok(Self {
            hard_handoff: input[0] & 0x80 != 0,
            frame_offset: input[1] & 0x0f,
            channels,
        })
    }
}

/// Physical channel type carried in `IS-2000 Channel Identity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Is2000PhysicalChannelType {
    Fch = 0x01,
    Dcch = 0x02,
    Sch = 0x03,
}

/// One TIA/EIA/IS-2000 channel assignment entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Is2000ChannelEntry {
    pub physical_channel_type: Is2000PhysicalChannelType,
    pub pilot_gating_rate: u8,
    pub qof_mask: u8,
    pub walsh_code_channel_index: u8,
    pub pilot_pn: u16,
    pub arfcn: Option<u16>,
}

/// Exact `IS-2000 Channel Identity` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is2000ChannelIdentity {
    pub otd: bool,
    pub frame_offset: u8,
    pub channels: Vec<Is2000ChannelEntry>,
}

impl Is2000ChannelIdentity {
    /// Encodes the payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.channels.is_empty() || self.channels.len() > 0x07 {
            return Err(Error::InvalidValue {
                context: "IS-2000 Channel Identity",
                reason: "must contain 1..=7 channels",
            });
        }
        let mut out = Vec::with_capacity(2 + self.channels.len() * 6);
        out.push(((self.otd as u8) << 7) | (self.channels.len() as u8 & 0x07));
        out.push(self.frame_offset & 0x0f);
        for channel in &self.channels {
            if channel.pilot_gating_rate > 0x02
                || channel.qof_mask > 0x03
                || channel.pilot_pn > 0x01ff
            {
                return Err(Error::InvalidValue {
                    context: "IS-2000 Channel Identity",
                    reason: "one or more fields out of range",
                });
            }
            let (freq_included, arfcn) = match channel.arfcn {
                Some(arfcn) => (true, arfcn),
                None => (false, 0),
            };
            out.push(channel.physical_channel_type as u8);
            out.push((channel.pilot_gating_rate << 4) | (channel.qof_mask & 0x03));
            out.push(channel.walsh_code_channel_index);
            out.push((channel.pilot_pn & 0xff) as u8);
            out.push(
                (((channel.pilot_pn >> 8) as u8) & 0x01)
                    | ((freq_included as u8) << 3)
                    | (((arfcn >> 8) as u8 & 0x07) << 4),
            );
            out.push(arfcn as u8);
        }
        Ok(out)
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let channel_count = (input[0] & 0x07) as usize;
        if channel_count == 0 || input.len() != 2 + channel_count * 6 {
            return Err(Error::InvalidLength {
                expected: 2 + channel_count * 6,
                actual: input.len(),
            });
        }
        let mut channels = Vec::with_capacity(channel_count);
        let mut offset = 2;
        for _ in 0..channel_count {
            let physical_channel_type = match input[offset] {
                0x01 => Is2000PhysicalChannelType::Fch,
                0x02 => Is2000PhysicalChannelType::Dcch,
                0x03 => Is2000PhysicalChannelType::Sch,
                other => {
                    return Err(Error::ReservedValue {
                        context: "IS-2000 Physical Channel Type",
                        value: other,
                    });
                }
            };
            let arfcn = (((input[offset + 4] >> 4) & 0x07) as u16) << 8 | input[offset + 5] as u16;
            channels.push(Is2000ChannelEntry {
                physical_channel_type,
                pilot_gating_rate: (input[offset + 1] >> 4) & 0x03,
                qof_mask: input[offset + 1] & 0x03,
                walsh_code_channel_index: input[offset + 2],
                pilot_pn: (((input[offset + 4] & 0x01) as u16) << 8) | input[offset + 3] as u16,
                arfcn: if input[offset + 4] & 0x08 != 0 {
                    Some(arfcn)
                } else {
                    None
                },
            });
            offset += 6;
        }
        Ok(Self {
            otd: input[0] & 0x80 != 0,
            frame_offset: input[1] & 0x0f,
            channels,
        })
    }
}

/// Exact bit-exact service configuration record wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is2000ServiceConfigurationRecord {
    pub fill_bits: u8,
    pub content: Vec<u8>,
}

impl Is2000ServiceConfigurationRecord {
    /// Encodes the payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.fill_bits > 7 || self.content.is_empty() {
            return Err(Error::InvalidValue {
                context: "IS-2000 Service Configuration Record",
                reason: "fill bits out of range or content empty",
            });
        }
        let mut out = Vec::with_capacity(2 + self.content.len());
        out.push(self.content.len() as u8);
        out.push(self.fill_bits & 0x07);
        out.extend_from_slice(&self.content);
        Ok(out)
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let len = input[0] as usize;
        if input.len() != 2 + len {
            return Err(Error::InvalidLength {
                expected: 2 + len,
                actual: input.len(),
            });
        }
        Ok(Self {
            fill_bits: input[1] & 0x07,
            content: input[2..].to_vec(),
        })
    }
}

/// Exact bit-exact non-negotiable service configuration record wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is2000NonNegotiableServiceConfigurationRecord {
    pub fill_bits: u8,
    pub content: Vec<u8>,
}

impl Is2000NonNegotiableServiceConfigurationRecord {
    /// Encodes the payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.fill_bits > 7 || self.content.is_empty() {
            return Err(Error::InvalidValue {
                context: "IS-2000 Non-Negotiable Service Configuration Record",
                reason: "fill bits out of range or content empty",
            });
        }
        let mut out = Vec::with_capacity(2 + self.content.len());
        out.push(self.content.len() as u8);
        out.push(self.fill_bits & 0x07);
        out.extend_from_slice(&self.content);
        Ok(out)
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let len = input[0] as usize;
        if input.len() != 2 + len {
            return Err(Error::InvalidLength {
                expected: 2 + len,
                actual: input.len(),
            });
        }
        Ok(Self {
            fill_bits: input[1] & 0x07,
            content: input[2..].to_vec(),
        })
    }
}

/// Exact `RF Channel Identity` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfChannelIdentity {
    pub color_code: u8,
    pub n_amps: bool,
    pub ansi_eia_tia_553: bool,
    pub timeslot_number: u8,
    pub arfcn: u16,
}

impl RfChannelIdentity {
    /// Encodes the payload.
    pub fn encode(self) -> Result<[u8; 5]> {
        if self.timeslot_number > 0x03 || self.arfcn > 0x07ff {
            return Err(Error::InvalidValue {
                context: "RF Channel Identity",
                reason: "timeslot or ARFCN out of range",
            });
        }
        Ok([
            self.color_code,
            ((self.n_amps as u8) << 1) | (self.ansi_eia_tia_553 as u8),
            self.timeslot_number & 0x03,
            ((self.arfcn >> 8) as u8) & 0x07,
            self.arfcn as u8,
        ])
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                expected: 5,
                actual: input.len(),
            });
        }
        Ok(Self {
            color_code: input[0],
            n_amps: input[1] & 0x02 != 0,
            ansi_eia_tia_553: input[1] & 0x01 != 0,
            timeslot_number: input[2] & 0x03,
            arfcn: (((input[3] & 0x07) as u16) << 8) | input[4] as u16,
        })
    }
}

/// Exact `SID` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sid(pub u16);

impl Sid {
    /// Encodes the payload.
    pub fn encode(self) -> Result<[u8; 2]> {
        if self.0 > 0x7fff {
            return Err(Error::InvalidValue {
                context: "SID",
                reason: "SID out of range",
            });
        }
        Ok(self.0.to_be_bytes())
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]]) & 0x7fff))
    }
}

/// Exact `Hard Handoff Parameters` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardHandoffParameters {
    pub band_class: u8,
    pub number_of_preamble_frames: u8,
    pub reset_l2: bool,
    pub reset_fpc: bool,
    pub encryption_mode: u8,
    pub private_lcm: bool,
    pub nom_pwr: u8,
    pub nom_pwr_ext: bool,
    pub fpc_subchannel_information: u8,
    pub fpc_subchannel_info_included: bool,
    pub power_control_step: u8,
    pub power_control_step_included: bool,
}

impl HardHandoffParameters {
    /// Encodes the payload.
    pub fn encode(self) -> Result<[u8; 5]> {
        if self.band_class > 0x1f
            || self.number_of_preamble_frames > 0x07
            || self.encryption_mode > 0x03
            || self.nom_pwr > 0x0f
            || self.fpc_subchannel_information > 0x0f
            || self.power_control_step > 0x07
        {
            return Err(Error::InvalidValue {
                context: "Hard Handoff Parameters",
                reason: "one or more fields exceed bit width",
            });
        }
        Ok([
            (self.band_class << 3) | (self.number_of_preamble_frames & 0x07),
            ((self.reset_l2 as u8) << 7)
                | ((self.reset_fpc as u8) << 6)
                | ((self.encryption_mode & 0x03) << 4)
                | ((self.private_lcm as u8) << 3),
            ((self.nom_pwr & 0x0f) << 3) | ((self.nom_pwr_ext as u8) << 2),
            (self.fpc_subchannel_information & 0x0f) << 4 | ((self.power_control_step & 0x07) << 1),
            ((self.fpc_subchannel_info_included as u8) << 1)
                | (self.power_control_step_included as u8),
        ])
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                expected: 5,
                actual: input.len(),
            });
        }
        Ok(Self {
            band_class: input[0] >> 3,
            number_of_preamble_frames: input[0] & 0x07,
            reset_l2: input[1] & 0x80 != 0,
            reset_fpc: input[1] & 0x40 != 0,
            encryption_mode: (input[1] >> 4) & 0x03,
            private_lcm: input[1] & 0x08 != 0,
            nom_pwr: input[2] >> 3,
            nom_pwr_ext: input[2] & 0x04 != 0,
            fpc_subchannel_information: input[3] >> 4,
            power_control_step: (input[3] >> 1) & 0x07,
            fpc_subchannel_info_included: input[4] & 0x02 != 0,
            power_control_step_included: input[4] & 0x01 != 0,
        })
    }
}

/// Exact typed extended handoff direction parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedHandoffDirectionParameters {
    pub search_window_a_size: u8,
    pub search_window_n_size: u8,
    pub search_window_r_size: u8,
    pub t_add: u8,
    pub t_drop: u8,
    pub compare_threshold: u8,
    pub drop_timer_value: u8,
    pub neighbor_max_age: u8,
    pub soft_slope: u8,
    pub add_intercept: u8,
    pub drop_intercept: u8,
    pub target_bs_p_rev: u8,
}

impl ExtendedHandoffDirectionParameters {
    /// Encodes the fixed-width payload.
    pub fn encode(self) -> Result<[u8; 9]> {
        if self.search_window_a_size > 0x0f
            || self.search_window_n_size > 0x0f
            || self.search_window_r_size > 0x0f
            || self.t_add > 0x3f
            || self.t_drop > 0x3f
            || self.compare_threshold > 0x0f
            || self.drop_timer_value > 0x0f
            || self.neighbor_max_age > 0x0f
            || self.soft_slope > 0x3f
            || self.add_intercept > 0x3f
            || self.drop_intercept > 0x3f
        {
            return Err(Error::InvalidValue {
                context: "Extended Handoff Direction Parameters",
                reason: "one or more fields exceed their bit width",
            });
        }
        Ok([
            (self.search_window_a_size << 4) | (self.search_window_n_size & 0x0f),
            (self.search_window_r_size << 4) | ((self.t_add >> 2) & 0x0f),
            ((self.t_add & 0x03) << 6) | (self.t_drop & 0x3f),
            (self.compare_threshold << 4) | (self.drop_timer_value & 0x0f),
            self.neighbor_max_age << 4,
            self.soft_slope,
            self.add_intercept,
            self.drop_intercept,
            self.target_bs_p_rev,
        ])
    }

    /// Decodes the fixed-width payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 9 {
            return Err(Error::InvalidLength {
                expected: 9,
                actual: input.len(),
            });
        }
        Ok(Self {
            search_window_a_size: input[0] >> 4,
            search_window_n_size: input[0] & 0x0f,
            search_window_r_size: input[1] >> 4,
            t_add: ((input[1] & 0x0f) << 2) | (input[2] >> 6),
            t_drop: input[2] & 0x3f,
            compare_threshold: input[3] >> 4,
            drop_timer_value: input[3] & 0x0f,
            neighbor_max_age: input[4] >> 4,
            soft_slope: input[5] & 0x3f,
            add_intercept: input[6] & 0x3f,
            drop_intercept: input[7] & 0x3f,
            target_bs_p_rev: input[8],
        })
    }
}

/// Exact `Handoff Power Level` payload preserved as an exact wire blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffPowerLevel(pub Vec<u8>);

impl HandoffPowerLevel {
    /// Encodes the payload.
    pub fn encode(&self) -> Result<&[u8]> {
        if self.0.is_empty() {
            return Err(Error::InvalidValue {
                context: "Handoff Power Level",
                reason: "payload must not be empty",
            });
        }
        Ok(&self.0)
    }

    /// Decodes the payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidValue {
                context: "Handoff Power Level",
                reason: "payload must not be empty",
            });
        }
        Ok(Self(input.to_vec()))
    }
}

/// Exact `Handoff Required` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRequiredMessage {
    pub cause: Cause,
    pub target_cell_identifier_list: HandoffCellIdentifierList,
    pub classmark_information_type_2: Option<ClassmarkInformationType2>,
    pub response_request: bool,
    pub encryption_information: Option<EncryptionInformation>,
    pub is95_channel_identity: Option<Is95ChannelIdentity>,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub downlink_radio_environment: Option<HandoffDownlinkRadioEnvironment>,
    pub service_option: Option<ServiceOption>,
    pub cdma_serving_one_way_delay: Option<HandoffCdmaServingOneWayDelay>,
    pub is95_ms_measured_channel_identity: Option<Is95MsMeasuredChannelIdentity>,
    pub is2000_channel_identity: Option<Is2000ChannelIdentity>,
    pub quality_of_service_parameters: Option<QualityOfServiceParameters>,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
    pub is2000_service_configuration_record: Option<Is2000ServiceConfigurationRecord>,
    pub pdsn_ip_address: Option<PdsnIpAddress>,
    pub protocol_type: Option<ProtocolType>,
}

impl HandoffRequiredMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.is95_channel_identity.is_some() && self.is2000_channel_identity.is_some() {
            return Err(Error::InvalidValue {
                context: "Handoff Required",
                reason: "IS-95 and IS-2000 channel identity must not both be present",
            });
        }
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CAUSE, &self.cause.encode())?;
        push_tlv(
            &mut body,
            IE_CELL_IDENTIFIER_LIST,
            &self.target_cell_identifier_list.encode()?,
        )?;
        if let Some(classmark_information_type_2) = &self.classmark_information_type_2 {
            push_tlv(&mut body, 0x12, classmark_information_type_2.encode()?)?;
        }
        if self.response_request {
            body.push(IE_RESPONSE_REQUEST);
        }
        if let Some(encryption_information) = &self.encryption_information {
            push_tlv(
                &mut body,
                IE_ENCRYPTION_INFORMATION,
                &encryption_information.encode()?,
            )?;
        }
        if let Some(is95_channel_identity) = &self.is95_channel_identity {
            push_tlv(
                &mut body,
                IE_IS95_CHANNEL_IDENTITY,
                &is95_channel_identity.encode()?,
            )?;
        }
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let payload = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "Handoff Required",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &payload)?;
        }
        if let Some(downlink_radio_environment) = &self.downlink_radio_environment {
            push_tlv(
                &mut body,
                IE_DOWNLINK_RADIO_ENVIRONMENT,
                &downlink_radio_environment.encode()?,
            )?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        if let Some(cdma_serving_one_way_delay) = self.cdma_serving_one_way_delay {
            push_tlv(
                &mut body,
                IE_CDMA_SERVING_ONE_WAY_DELAY,
                &cdma_serving_one_way_delay.encode()?,
            )?;
        }
        if let Some(is95_ms_measured_channel_identity) = self.is95_ms_measured_channel_identity {
            push_tlv(
                &mut body,
                IE_IS95_MS_MEASURED_CHANNEL_IDENTITY,
                &is95_ms_measured_channel_identity.encode()?,
            )?;
        }
        if let Some(is2000_channel_identity) = &self.is2000_channel_identity {
            push_tlv(
                &mut body,
                IE_IS2000_CHANNEL_IDENTITY,
                &is2000_channel_identity.encode()?,
            )?;
        }
        if let Some(quality_of_service_parameters) = self.quality_of_service_parameters {
            push_tlv(
                &mut body,
                IE_QUALITY_OF_SERVICE_PARAMETERS,
                &quality_of_service_parameters.encode()?,
            )?;
        }
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_tlv(
                &mut body,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        if let Some(is2000_service_configuration_record) = &self.is2000_service_configuration_record
        {
            push_tlv(
                &mut body,
                IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                &is2000_service_configuration_record.encode()?,
            )?;
        }
        if let Some(pdsn_ip_address) = self.pdsn_ip_address {
            push_tlv(&mut body, IE_PDSN_IP_ADDRESS, &pdsn_ip_address.encode())?;
        }
        if let Some(protocol_type) = self.protocol_type {
            push_tlv(&mut body, IE_PROTOCOL_TYPE, &protocol_type.encode())?;
        }
        encode_bsmap(HANDOFF_REQUIRED, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_REQUIRED, input)?;
        let mut offset = 0;
        let mut cause = None;
        let mut target_cell_identifier_list = None;
        let mut classmark_information_type_2 = None;
        let mut response_request = false;
        let mut encryption_information = None;
        let mut is95_channel_identity = None;
        let mut mobile_identity_esn = None;
        let mut downlink_radio_environment = None;
        let mut service_option = None;
        let mut cdma_serving_one_way_delay = None;
        let mut is95_ms_measured_channel_identity = None;
        let mut is2000_channel_identity = None;
        let mut quality_of_service_parameters = None;
        let mut is2000_mobile_capabilities = None;
        let mut is2000_service_configuration_record = None;
        let mut pdsn_ip_address = None;
        let mut protocol_type = None;
        while offset < body.len() {
            match body[offset] {
                IE_CAUSE => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cause,
                        Cause::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_CAUSE,
                    )?;
                    offset += used;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut target_cell_identifier_list,
                        HandoffCellIdentifierList::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += used;
                }
                0x12 => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut classmark_information_type_2,
                        ClassmarkInformationType2::decode(payload)?,
                        HANDOFF_REQUIRED,
                        0x12,
                    )?;
                    offset += used;
                }
                IE_RESPONSE_REQUEST => {
                    set_marker_once(&mut response_request, HANDOFF_REQUIRED, IE_RESPONSE_REQUEST)?;
                    offset += 1;
                }
                IE_ENCRYPTION_INFORMATION => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut encryption_information,
                        EncryptionInformation::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_ENCRYPTION_INFORMATION,
                    )?;
                    offset += used;
                }
                IE_IS95_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is95_channel_identity,
                        Is95ChannelIdentity::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_IS95_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_MOBILE_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if !matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "Handoff Required",
                            reason: "ESN field must contain ESN mobile identity",
                        });
                    }
                    set_once(
                        &mut mobile_identity_esn,
                        identity,
                        HANDOFF_REQUIRED,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_DOWNLINK_RADIO_ENVIRONMENT => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut downlink_radio_environment,
                        HandoffDownlinkRadioEnvironment::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_DOWNLINK_RADIO_ENVIRONMENT,
                    )?;
                    offset += used;
                }
                IE_SERVICE_OPTION => {
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(take_fixed(&body[offset..], 2)?)?,
                        HANDOFF_REQUIRED,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_CDMA_SERVING_ONE_WAY_DELAY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cdma_serving_one_way_delay,
                        HandoffCdmaServingOneWayDelay::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_CDMA_SERVING_ONE_WAY_DELAY,
                    )?;
                    offset += used;
                }
                IE_IS95_MS_MEASURED_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is95_ms_measured_channel_identity,
                        Is95MsMeasuredChannelIdentity::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_IS95_MS_MEASURED_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_IS2000_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_channel_identity,
                        Is2000ChannelIdentity::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_IS2000_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_QUALITY_OF_SERVICE_PARAMETERS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut quality_of_service_parameters,
                        QualityOfServiceParameters::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_QUALITY_OF_SERVICE_PARAMETERS,
                    )?;
                    offset += used;
                }
                IE_IS2000_MOBILE_CAPABILITIES => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_mobile_capabilities,
                        Is2000MobileCapabilities::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_IS2000_MOBILE_CAPABILITIES,
                    )?;
                    offset += used;
                }
                IE_IS2000_SERVICE_CONFIGURATION_RECORD => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_service_configuration_record,
                        Is2000ServiceConfigurationRecord::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                    )?;
                    offset += used;
                }
                IE_PDSN_IP_ADDRESS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut pdsn_ip_address,
                        PdsnIpAddress::decode(payload)?,
                        HANDOFF_REQUIRED,
                        IE_PDSN_IP_ADDRESS,
                    )?;
                    offset += used;
                }
                IE_PROTOCOL_TYPE => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut protocol_type,
                        ProtocolType::decode(payload)?,
                        0x11,
                        IE_PROTOCOL_TYPE,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            cause: cause.ok_or(Error::MissingRequiredElement {
                message_type: HANDOFF_REQUIRED,
                id: IE_CAUSE,
            })?,
            target_cell_identifier_list: target_cell_identifier_list.ok_or(
                Error::MissingRequiredElement {
                    message_type: HANDOFF_REQUIRED,
                    id: IE_CELL_IDENTIFIER_LIST,
                },
            )?,
            classmark_information_type_2,
            response_request,
            encryption_information,
            is95_channel_identity,
            mobile_identity_esn,
            downlink_radio_environment,
            service_option,
            cdma_serving_one_way_delay,
            is95_ms_measured_channel_identity,
            is2000_channel_identity,
            quality_of_service_parameters,
            is2000_mobile_capabilities,
            is2000_service_configuration_record,
            pdsn_ip_address,
            protocol_type,
        })
    }
}

/// Exact `Handoff Request` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRequestMessage {
    pub channel_type: ChannelType,
    pub encryption_information: Option<EncryptionInformation>,
    pub classmark_information_type_2: Option<ClassmarkInformationType2>,
    pub target_cell_identifier_list: Option<HandoffCellIdentifierList>,
    pub circuit_identity_code_extension: Option<CircuitIdentityCodeExtension>,
    pub is95_channel_identity: Option<Is95ChannelIdentity>,
    pub mobile_identity_imsi: MobileIdentity,
    pub mobile_identity_esn: Option<MobileIdentity>,
    pub downlink_radio_environment: Option<HandoffDownlinkRadioEnvironment>,
    pub service_option: Option<ServiceOption>,
    pub cdma_serving_one_way_delay: Option<HandoffCdmaServingOneWayDelay>,
    pub is95_ms_measured_channel_identity: Option<Is95MsMeasuredChannelIdentity>,
    pub is2000_channel_identity: Option<Is2000ChannelIdentity>,
    pub quality_of_service_parameters: Option<QualityOfServiceParameters>,
    pub is2000_mobile_capabilities: Option<Is2000MobileCapabilities>,
    pub is2000_service_configuration_record: Option<Is2000ServiceConfigurationRecord>,
    pub pdsn_ip_address: Option<PdsnIpAddress>,
    pub protocol_type: Option<ProtocolType>,
}

impl HandoffRequestMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.is95_channel_identity.is_some() && self.is2000_channel_identity.is_some() {
            return Err(Error::InvalidValue {
                context: "Handoff Request",
                reason: "IS-95 and IS-2000 channel identity must not both be present",
            });
        }
        let mut body = Vec::new();
        push_tlv(&mut body, IE_CHANNEL_TYPE, &self.channel_type.encode())?;
        if let Some(encryption_information) = &self.encryption_information {
            push_tlv(
                &mut body,
                IE_ENCRYPTION_INFORMATION,
                &encryption_information.encode()?,
            )?;
        }
        if let Some(classmark_information_type_2) = &self.classmark_information_type_2 {
            push_tlv(&mut body, 0x12, classmark_information_type_2.encode()?)?;
        }
        if let Some(target_cell_identifier_list) = &self.target_cell_identifier_list {
            push_tlv(
                &mut body,
                IE_CELL_IDENTIFIER_LIST,
                &target_cell_identifier_list.encode()?,
            )?;
        }
        if let Some(circuit_identity_code_extension) = self.circuit_identity_code_extension {
            push_tlv(
                &mut body,
                IE_CIRCUIT_IDENTITY_CODE_EXTENSION,
                &circuit_identity_code_extension.encode()?,
            )?;
        }
        if let Some(is95_channel_identity) = &self.is95_channel_identity {
            push_tlv(
                &mut body,
                IE_IS95_CHANNEL_IDENTITY,
                &is95_channel_identity.encode()?,
            )?;
        }
        let imsi = self.mobile_identity_imsi.encode()?;
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "Handoff Request",
                reason: "IMSI field must contain IMSI mobile identity",
            });
        }
        push_tlv(&mut body, IE_MOBILE_IDENTITY, &imsi)?;
        if let Some(mobile_identity_esn) = &self.mobile_identity_esn {
            let esn = mobile_identity_esn.encode()?;
            if !matches!(mobile_identity_esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "Handoff Request",
                    reason: "ESN field must contain ESN mobile identity",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &esn)?;
        }
        if let Some(downlink_radio_environment) = &self.downlink_radio_environment {
            push_tlv(
                &mut body,
                IE_DOWNLINK_RADIO_ENVIRONMENT,
                &downlink_radio_environment.encode()?,
            )?;
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        if let Some(cdma_serving_one_way_delay) = self.cdma_serving_one_way_delay {
            push_tlv(
                &mut body,
                IE_CDMA_SERVING_ONE_WAY_DELAY,
                &cdma_serving_one_way_delay.encode()?,
            )?;
        }
        if let Some(is95_ms_measured_channel_identity) = self.is95_ms_measured_channel_identity {
            push_tlv(
                &mut body,
                IE_IS95_MS_MEASURED_CHANNEL_IDENTITY,
                &is95_ms_measured_channel_identity.encode()?,
            )?;
        }
        if let Some(is2000_channel_identity) = &self.is2000_channel_identity {
            push_tlv(
                &mut body,
                IE_IS2000_CHANNEL_IDENTITY,
                &is2000_channel_identity.encode()?,
            )?;
        }
        if let Some(quality_of_service_parameters) = self.quality_of_service_parameters {
            push_tlv(
                &mut body,
                IE_QUALITY_OF_SERVICE_PARAMETERS,
                &quality_of_service_parameters.encode()?,
            )?;
        }
        if let Some(is2000_mobile_capabilities) = &self.is2000_mobile_capabilities {
            push_tlv(
                &mut body,
                IE_IS2000_MOBILE_CAPABILITIES,
                is2000_mobile_capabilities.encode()?,
            )?;
        }
        if let Some(is2000_service_configuration_record) = &self.is2000_service_configuration_record
        {
            push_tlv(
                &mut body,
                IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                &is2000_service_configuration_record.encode()?,
            )?;
        }
        if let Some(pdsn_ip_address) = self.pdsn_ip_address {
            push_tlv(&mut body, IE_PDSN_IP_ADDRESS, &pdsn_ip_address.encode())?;
        }
        if let Some(protocol_type) = self.protocol_type {
            push_tlv(&mut body, IE_PROTOCOL_TYPE, &protocol_type.encode())?;
        }
        encode_bsmap(HANDOFF_REQUEST, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_REQUEST, input)?;
        let mut offset = 0;
        let mut channel_type = None;
        let mut encryption_information = None;
        let mut classmark_information_type_2 = None;
        let mut target_cell_identifier_list = None;
        let mut circuit_identity_code_extension = None;
        let mut is95_channel_identity = None;
        let mut mobile_identity_imsi = None;
        let mut mobile_identity_esn = None;
        let mut downlink_radio_environment = None;
        let mut service_option = None;
        let mut cdma_serving_one_way_delay = None;
        let mut is95_ms_measured_channel_identity = None;
        let mut is2000_channel_identity = None;
        let mut quality_of_service_parameters = None;
        let mut is2000_mobile_capabilities = None;
        let mut is2000_service_configuration_record = None;
        let mut pdsn_ip_address = None;
        let mut protocol_type = None;
        while offset < body.len() {
            match body[offset] {
                IE_CHANNEL_TYPE => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut channel_type,
                        ChannelType::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_CHANNEL_TYPE,
                    )?;
                    offset += used;
                }
                IE_ENCRYPTION_INFORMATION => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut encryption_information,
                        EncryptionInformation::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_ENCRYPTION_INFORMATION,
                    )?;
                    offset += used;
                }
                0x12 => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut classmark_information_type_2,
                        ClassmarkInformationType2::decode(payload)?,
                        HANDOFF_REQUEST,
                        0x12,
                    )?;
                    offset += used;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut target_cell_identifier_list,
                        HandoffCellIdentifierList::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += used;
                }
                IE_CIRCUIT_IDENTITY_CODE_EXTENSION => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut circuit_identity_code_extension,
                        CircuitIdentityCodeExtension::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_CIRCUIT_IDENTITY_CODE_EXTENSION,
                    )?;
                    offset += used;
                }
                IE_IS95_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is95_channel_identity,
                        Is95ChannelIdentity::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_IS95_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_MOBILE_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    match identity {
                        MobileIdentity::Imsi(_) if mobile_identity_imsi.is_none() => {
                            mobile_identity_imsi = Some(identity)
                        }
                        MobileIdentity::Esn(_) => set_once(
                            &mut mobile_identity_esn,
                            identity,
                            HANDOFF_REQUEST,
                            IE_MOBILE_IDENTITY,
                        )?,
                        _ => {
                            return Err(Error::InvalidValue {
                                context: "Handoff Request",
                                reason: "unexpected mobile identity ordering",
                            });
                        }
                    }
                    offset += used;
                }
                IE_DOWNLINK_RADIO_ENVIRONMENT => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut downlink_radio_environment,
                        HandoffDownlinkRadioEnvironment::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_DOWNLINK_RADIO_ENVIRONMENT,
                    )?;
                    offset += used;
                }
                IE_SERVICE_OPTION => {
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(take_fixed(&body[offset..], 2)?)?,
                        HANDOFF_REQUEST,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                IE_CDMA_SERVING_ONE_WAY_DELAY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cdma_serving_one_way_delay,
                        HandoffCdmaServingOneWayDelay::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_CDMA_SERVING_ONE_WAY_DELAY,
                    )?;
                    offset += used;
                }
                IE_IS95_MS_MEASURED_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is95_ms_measured_channel_identity,
                        Is95MsMeasuredChannelIdentity::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_IS95_MS_MEASURED_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_IS2000_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_channel_identity,
                        Is2000ChannelIdentity::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_IS2000_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_QUALITY_OF_SERVICE_PARAMETERS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut quality_of_service_parameters,
                        QualityOfServiceParameters::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_QUALITY_OF_SERVICE_PARAMETERS,
                    )?;
                    offset += used;
                }
                IE_IS2000_MOBILE_CAPABILITIES => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_mobile_capabilities,
                        Is2000MobileCapabilities::decode(payload)?,
                        HANDOFF_REQUEST,
                        IE_IS2000_MOBILE_CAPABILITIES,
                    )?;
                    offset += used;
                }
                IE_IS2000_SERVICE_CONFIGURATION_RECORD => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_service_configuration_record,
                        Is2000ServiceConfigurationRecord::decode(payload)?,
                        0x10,
                        IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                    )?;
                    offset += used;
                }
                IE_PDSN_IP_ADDRESS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut pdsn_ip_address,
                        PdsnIpAddress::decode(payload)?,
                        0x10,
                        IE_PDSN_IP_ADDRESS,
                    )?;
                    offset += used;
                }
                IE_PROTOCOL_TYPE => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut protocol_type,
                        ProtocolType::decode(payload)?,
                        0x10,
                        IE_PROTOCOL_TYPE,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            channel_type: channel_type.ok_or(Error::MissingRequiredElement {
                message_type: HANDOFF_REQUEST,
                id: IE_CHANNEL_TYPE,
            })?,
            encryption_information,
            classmark_information_type_2,
            target_cell_identifier_list,
            circuit_identity_code_extension,
            is95_channel_identity,
            mobile_identity_imsi: mobile_identity_imsi.ok_or(Error::MissingRequiredElement {
                message_type: HANDOFF_REQUEST,
                id: IE_MOBILE_IDENTITY,
            })?,
            mobile_identity_esn,
            downlink_radio_environment,
            service_option,
            cdma_serving_one_way_delay,
            is95_ms_measured_channel_identity,
            is2000_channel_identity,
            quality_of_service_parameters,
            is2000_mobile_capabilities,
            is2000_service_configuration_record,
            pdsn_ip_address,
            protocol_type,
        })
    }
}

/// Exact `Handoff Request Acknowledge` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRequestAcknowledgeMessage {
    pub is95_channel_identity: Option<Is95ChannelIdentity>,
    pub cell_identifier_list: Option<HandoffCellIdentifierList>,
    pub extended_handoff_direction_parameters: Option<ExtendedHandoffDirectionParameters>,
    pub hard_handoff_parameters: Option<HardHandoffParameters>,
    pub is2000_channel_identity: Option<Is2000ChannelIdentity>,
    pub is2000_service_configuration_record: Option<Is2000ServiceConfigurationRecord>,
    pub is2000_non_negotiable_service_configuration_record:
        Option<Is2000NonNegotiableServiceConfigurationRecord>,
}

impl HandoffRequestAcknowledgeMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.is95_channel_identity.is_some() && self.is2000_channel_identity.is_some() {
            return Err(Error::InvalidValue {
                context: "Handoff Request Acknowledge",
                reason: "IS-95 and IS-2000 channel identity must not both be present",
            });
        }
        if self.is95_channel_identity.is_none() && self.is2000_channel_identity.is_none() {
            return Err(Error::InvalidValue {
                context: "Handoff Request Acknowledge",
                reason: "one channel identity must be present",
            });
        }
        if self.is2000_channel_identity.is_none()
            && (self.is2000_service_configuration_record.is_some()
                || self
                    .is2000_non_negotiable_service_configuration_record
                    .is_some())
        {
            return Err(Error::InvalidValue {
                context: "Handoff Request Acknowledge",
                reason: "IS-2000 service configuration requires IS-2000 channel identity",
            });
        }
        let mut body = Vec::new();
        if let Some(is95_channel_identity) = &self.is95_channel_identity {
            push_tlv(
                &mut body,
                IE_IS95_CHANNEL_IDENTITY,
                &is95_channel_identity.encode()?,
            )?;
        }
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            push_tlv(
                &mut body,
                IE_CELL_IDENTIFIER_LIST,
                &cell_identifier_list.encode()?,
            )?;
        }
        if let Some(extended_handoff_direction_parameters) =
            self.extended_handoff_direction_parameters
        {
            push_tlv(
                &mut body,
                IE_EXTENDED_HANDOFF_DIRECTION_PARAMETERS,
                &extended_handoff_direction_parameters.encode()?,
            )?;
        }
        if let Some(hard_handoff_parameters) = self.hard_handoff_parameters {
            push_tlv(
                &mut body,
                IE_HARD_HANDOFF_PARAMETERS,
                &hard_handoff_parameters.encode()?,
            )?;
        }
        if let Some(is2000_channel_identity) = &self.is2000_channel_identity {
            push_tlv(
                &mut body,
                IE_IS2000_CHANNEL_IDENTITY,
                &is2000_channel_identity.encode()?,
            )?;
        }
        if let Some(is2000_service_configuration_record) = &self.is2000_service_configuration_record
        {
            push_tlv(
                &mut body,
                IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                &is2000_service_configuration_record.encode()?,
            )?;
        }
        if let Some(is2000_non_negotiable_service_configuration_record) =
            &self.is2000_non_negotiable_service_configuration_record
        {
            push_tlv(
                &mut body,
                IE_IS2000_NON_NEGOTIABLE_SERVICE_CONFIGURATION_RECORD,
                &is2000_non_negotiable_service_configuration_record.encode()?,
            )?;
        }
        encode_bsmap(HANDOFF_REQUEST_ACKNOWLEDGE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_REQUEST_ACKNOWLEDGE, input)?;
        let mut offset = 0;
        let mut is95_channel_identity = None;
        let mut cell_identifier_list = None;
        let mut extended_handoff_direction_parameters = None;
        let mut hard_handoff_parameters = None;
        let mut is2000_channel_identity = None;
        let mut is2000_service_configuration_record = None;
        let mut is2000_non_negotiable_service_configuration_record = None;
        while offset < body.len() {
            match body[offset] {
                IE_IS95_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is95_channel_identity,
                        Is95ChannelIdentity::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_IS95_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cell_identifier_list,
                        HandoffCellIdentifierList::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += used;
                }
                IE_EXTENDED_HANDOFF_DIRECTION_PARAMETERS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut extended_handoff_direction_parameters,
                        ExtendedHandoffDirectionParameters::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_EXTENDED_HANDOFF_DIRECTION_PARAMETERS,
                    )?;
                    offset += used;
                }
                IE_HARD_HANDOFF_PARAMETERS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut hard_handoff_parameters,
                        HardHandoffParameters::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_HARD_HANDOFF_PARAMETERS,
                    )?;
                    offset += used;
                }
                IE_IS2000_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_channel_identity,
                        Is2000ChannelIdentity::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_IS2000_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_IS2000_SERVICE_CONFIGURATION_RECORD => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_service_configuration_record,
                        Is2000ServiceConfigurationRecord::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                    )?;
                    offset += used;
                }
                IE_IS2000_NON_NEGOTIABLE_SERVICE_CONFIGURATION_RECORD => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is2000_non_negotiable_service_configuration_record,
                        Is2000NonNegotiableServiceConfigurationRecord::decode(payload)?,
                        HANDOFF_REQUEST_ACKNOWLEDGE,
                        IE_IS2000_NON_NEGOTIABLE_SERVICE_CONFIGURATION_RECORD,
                    )?;
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            is95_channel_identity,
            cell_identifier_list,
            extended_handoff_direction_parameters,
            hard_handoff_parameters,
            is2000_channel_identity,
            is2000_service_configuration_record,
            is2000_non_negotiable_service_configuration_record,
        })
    }
}

/// Exact `Handoff Command` BSMAP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffCommandMessage {
    pub rf_channel_identity: Option<RfChannelIdentity>,
    pub is95_channel_identity: Option<Is95ChannelIdentity>,
    pub cell_identifier_list: Option<HandoffCellIdentifierList>,
    pub handoff_power_level: Option<HandoffPowerLevel>,
    pub sid: Option<Sid>,
    pub extended_handoff_direction_parameters: Option<ExtendedHandoffDirectionParameters>,
    pub hard_handoff_parameters: Option<HardHandoffParameters>,
    pub is2000_channel_identity: Option<Is2000ChannelIdentity>,
    pub is2000_service_configuration_record: Option<Is2000ServiceConfigurationRecord>,
    pub is2000_non_negotiable_service_configuration_record:
        Option<Is2000NonNegotiableServiceConfigurationRecord>,
}

impl HandoffCommandMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let cdma_target_present =
            self.is95_channel_identity.is_some() || self.is2000_channel_identity.is_some();
        if self.rf_channel_identity.is_some() && cdma_target_present {
            return Err(Error::InvalidValue {
                context: "Handoff Command",
                reason: "RF channel identity must not be present with CDMA channel identity",
            });
        }
        if self.is95_channel_identity.is_some() && self.is2000_channel_identity.is_some() {
            return Err(Error::InvalidValue {
                context: "Handoff Command",
                reason: "IS-95 and IS-2000 channel identity must not both be present",
            });
        }
        if self.rf_channel_identity.is_none() && !cdma_target_present {
            return Err(Error::InvalidValue {
                context: "Handoff Command",
                reason: "one target channel identity must be present",
            });
        }
        if self.is2000_channel_identity.is_none()
            && (self.is2000_service_configuration_record.is_some()
                || self
                    .is2000_non_negotiable_service_configuration_record
                    .is_some())
        {
            return Err(Error::InvalidValue {
                context: "Handoff Command",
                reason: "IS-2000 service configuration requires IS-2000 channel identity",
            });
        }
        let mut body = Vec::new();
        if let Some(rf_channel_identity) = self.rf_channel_identity {
            push_tlv(
                &mut body,
                IE_RF_CHANNEL_IDENTITY,
                &rf_channel_identity.encode()?,
            )?;
        }
        if let Some(is95_channel_identity) = &self.is95_channel_identity {
            push_tlv(
                &mut body,
                IE_IS95_CHANNEL_IDENTITY,
                &is95_channel_identity.encode()?,
            )?;
        }
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            push_tlv(
                &mut body,
                IE_CELL_IDENTIFIER_LIST,
                &cell_identifier_list.encode()?,
            )?;
        }
        if let Some(handoff_power_level) = &self.handoff_power_level {
            push_tlv(
                &mut body,
                IE_HANDOFF_POWER_LEVEL,
                handoff_power_level.encode()?,
            )?;
        }
        if let Some(sid) = self.sid {
            push_tlv(&mut body, IE_SID, &sid.encode()?)?;
        }
        if let Some(extended_handoff_direction_parameters) =
            self.extended_handoff_direction_parameters
        {
            push_tlv(
                &mut body,
                IE_EXTENDED_HANDOFF_DIRECTION_PARAMETERS,
                &extended_handoff_direction_parameters.encode()?,
            )?;
        }
        if let Some(hard_handoff_parameters) = self.hard_handoff_parameters {
            push_tlv(
                &mut body,
                IE_HARD_HANDOFF_PARAMETERS,
                &hard_handoff_parameters.encode()?,
            )?;
        }
        if let Some(is2000_channel_identity) = &self.is2000_channel_identity {
            push_tlv(
                &mut body,
                IE_IS2000_CHANNEL_IDENTITY,
                &is2000_channel_identity.encode()?,
            )?;
        }
        if let Some(is2000_service_configuration_record) = &self.is2000_service_configuration_record
        {
            push_tlv(
                &mut body,
                IE_IS2000_SERVICE_CONFIGURATION_RECORD,
                &is2000_service_configuration_record.encode()?,
            )?;
        }
        if let Some(is2000_non_negotiable_service_configuration_record) =
            &self.is2000_non_negotiable_service_configuration_record
        {
            push_tlv(
                &mut body,
                IE_IS2000_NON_NEGOTIABLE_SERVICE_CONFIGURATION_RECORD,
                &is2000_non_negotiable_service_configuration_record.encode()?,
            )?;
        }
        encode_bsmap(HANDOFF_COMMAND, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(HANDOFF_COMMAND, input)?;
        let mut offset = 0;
        let mut rf_channel_identity = None;
        let mut is95_channel_identity = None;
        let mut cell_identifier_list = None;
        let mut handoff_power_level = None;
        let mut sid = None;
        let mut extended_handoff_direction_parameters = None;
        let mut hard_handoff_parameters = None;
        let mut is2000_channel_identity = None;
        let mut is2000_service_configuration_record = None;
        let mut is2000_non_negotiable_service_configuration_record = None;
        while offset < body.len() {
            match body[offset] {
                IE_RF_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut rf_channel_identity,
                        RfChannelIdentity::decode(payload)?,
                        HANDOFF_COMMAND,
                        IE_RF_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_IS95_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut is95_channel_identity,
                        Is95ChannelIdentity::decode(payload)?,
                        HANDOFF_COMMAND,
                        IE_IS95_CHANNEL_IDENTITY,
                    )?;
                    offset += used;
                }
                IE_CELL_IDENTIFIER_LIST => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cell_identifier_list,
                        HandoffCellIdentifierList::decode(payload)?,
                        HANDOFF_COMMAND,
                        IE_CELL_IDENTIFIER_LIST,
                    )?;
                    offset += used;
                }
                IE_HANDOFF_POWER_LEVEL => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut handoff_power_level,
                        HandoffPowerLevel::decode(payload)?,
                        HANDOFF_COMMAND,
                        IE_HANDOFF_POWER_LEVEL,
                    )?;
                    offset += used;
                }
                IE_SID => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    set_once(&mut sid, Sid::decode(payload)?, HANDOFF_COMMAND, IE_SID)?;
                    offset += used;
                }
                IE_EXTENDED_HANDOFF_DIRECTION_PARAMETERS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    extended_handoff_direction_parameters =
                        Some(ExtendedHandoffDirectionParameters::decode(payload)?);
                    offset += used;
                }
                IE_HARD_HANDOFF_PARAMETERS => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    hard_handoff_parameters = Some(HardHandoffParameters::decode(payload)?);
                    offset += used;
                }
                IE_IS2000_CHANNEL_IDENTITY => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    is2000_channel_identity = Some(Is2000ChannelIdentity::decode(payload)?);
                    offset += used;
                }
                IE_IS2000_SERVICE_CONFIGURATION_RECORD => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    is2000_service_configuration_record =
                        Some(Is2000ServiceConfigurationRecord::decode(payload)?);
                    offset += used;
                }
                IE_IS2000_NON_NEGOTIABLE_SERVICE_CONFIGURATION_RECORD => {
                    let (_, payload, used) = decode_tlv(&body[offset..])?;
                    is2000_non_negotiable_service_configuration_record = Some(
                        Is2000NonNegotiableServiceConfigurationRecord::decode(payload)?,
                    );
                    offset += used;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            rf_channel_identity,
            is95_channel_identity,
            cell_identifier_list,
            handoff_power_level,
            sid,
            extended_handoff_direction_parameters,
            hard_handoff_parameters,
            is2000_channel_identity,
            is2000_service_configuration_record,
            is2000_non_negotiable_service_configuration_record,
        })
    }
}

fn encode_bsmap(message_type: u8, body: &[u8]) -> Result<Vec<u8>> {
    let li = 1usize + body.len();
    if li > u8::MAX as usize {
        return Err(Error::InvalidLength {
            expected: u8::MAX as usize,
            actual: li,
        });
    }
    let mut out = Vec::with_capacity(li + 2);
    out.push(BSMAP_MESSAGE_DISCRIMINATION);
    out.push(li as u8);
    out.push(message_type);
    out.extend_from_slice(body);
    Ok(out)
}

fn validate_handoff_performed_cause(cause: Cause) -> Result<()> {
    match cause.0 {
        0x02 | 0x03 | 0x04 | 0x05 | 0x06 | 0x07 | 0x0e | 0x0f | 0x1b | 0x1d => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "Handoff Performed Cause",
            reason: "cause value is not allowed for Handoff Performed",
        }),
    }
}

fn parse_bsmap(expected_message_type: u8, input: &[u8]) -> Result<&[u8]> {
    if input.len() < 3 {
        return Err(Error::Truncated {
            needed: 3,
            actual: input.len(),
        });
    }
    if input[0] != BSMAP_MESSAGE_DISCRIMINATION {
        return Err(Error::InvalidValue {
            context: "A1 message discrimination",
            reason: "expected BSMAP message discrimination 0x00",
        });
    }
    let li = input[1] as usize;
    if input.len() != li + 2 {
        return Err(Error::InvalidLength {
            expected: li + 2,
            actual: input.len(),
        });
    }
    if input[2] != expected_message_type {
        return Err(Error::UnknownMessageType(input[2]));
    }
    Ok(&input[3..])
}

fn encode_a1_dtap(message_type: u8, body: &[u8]) -> Result<Vec<u8>> {
    encode_a1_dtap_with_protocol(
        message_type,
        DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
        body,
    )
}

fn encode_a1_mm_dtap(message_type: u8, body: &[u8]) -> Result<Vec<u8>> {
    encode_a1_dtap_with_protocol(
        message_type,
        DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT,
        body,
    )
}

fn encode_a1_dtap_with_protocol(
    message_type: u8,
    protocol_discriminator: u8,
    body: &[u8],
) -> Result<Vec<u8>> {
    // A1 DTAP frame: [disc=0x01][DLCI=0x00][LI][proto_disc][reserved=0x00][msg_type][body]
    // LI counts bytes from after itself: proto_disc(1) + reserved(1) + msg_type(1) + body
    let li = 3usize + body.len();
    if li > u8::MAX as usize {
        return Err(Error::InvalidLength {
            expected: u8::MAX as usize,
            actual: li,
        });
    }
    let mut out = Vec::with_capacity(li + 3);
    out.push(A1_DTAP_MESSAGE_DISCRIMINATION);
    out.push(0x00); // DLCI
    out.push(li as u8);
    out.push(protocol_discriminator);
    out.push(0x00); // reserved
    out.push(message_type);
    out.extend_from_slice(body);
    Ok(out)
}

fn parse_a1_dtap(expected_message_type: u8, input: &[u8]) -> Result<&[u8]> {
    parse_a1_dtap_with_protocol(
        expected_message_type,
        DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
        input,
    )
}

fn parse_a1_mm_dtap(expected_message_type: u8, input: &[u8]) -> Result<&[u8]> {
    parse_a1_dtap_with_protocol(
        expected_message_type,
        DTAP_PROTOCOL_DISCRIMINATOR_MOBILITY_MANAGEMENT,
        input,
    )
}

fn parse_a1_dtap_with_protocol(
    expected_message_type: u8,
    expected_protocol_discriminator: u8,
    input: &[u8],
) -> Result<&[u8]> {
    // A1 DTAP frame: [disc=0x01][DLCI=0x00][LI][proto_disc][reserved=0x00][msg_type][body]
    if input.len() < 6 {
        return Err(Error::Truncated {
            needed: 6,
            actual: input.len(),
        });
    }
    if input[0] != A1_DTAP_MESSAGE_DISCRIMINATION {
        return Err(Error::InvalidValue {
            context: "A1 DTAP message discrimination",
            reason: "expected DTAP message discrimination 0x01",
        });
    }
    // input[1] = DLCI (not enforced — per spec always 0x00 but we accept any value)
    let li = input[2] as usize;
    if input.len() != li + 3 {
        return Err(Error::InvalidLength {
            expected: li + 3,
            actual: input.len(),
        });
    }
    if input[3] != expected_protocol_discriminator {
        return Err(Error::ReservedValue {
            context: "A1 DTAP protocol discriminator",
            value: input[3],
        });
    }
    if input[4] != 0x00 {
        return Err(Error::InvalidValue {
            context: "A1 DTAP reserved octet",
            reason: "reserved octet must be zero",
        });
    }
    if input[5] != expected_message_type {
        return Err(Error::UnknownMessageType(input[5]));
    }
    Ok(&input[6..])
}

fn parse_dtap(
    expected_protocol_discriminator: u8,
    expected_message_type: u8,
    input: &[u8],
) -> Result<&[u8]> {
    if input.len() < 3 {
        return Err(Error::Truncated {
            needed: 3,
            actual: input.len(),
        });
    }
    if input[0] != expected_protocol_discriminator {
        return Err(Error::ReservedValue {
            context: "DTAP Protocol Discriminator",
            value: input[0],
        });
    }
    if input[1] != 0x00 {
        return Err(Error::InvalidValue {
            context: "DTAP Reserved Octet",
            reason: "reserved octet must be zero",
        });
    }
    if input[2] != expected_message_type {
        return Err(Error::UnknownMessageType(input[2]));
    }
    Ok(&input[3..])
}

fn push_tlv(out: &mut Vec<u8>, id: u8, payload: &[u8]) -> Result<()> {
    if payload.len() > u8::MAX as usize {
        return Err(Error::InvalidLength {
            expected: u8::MAX as usize,
            actual: payload.len(),
        });
    }
    out.push(id);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    Ok(())
}

fn encode_single_tlv(id: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    push_tlv(&mut out, id, payload)?;
    Ok(out)
}

fn push_fixed(out: &mut Vec<u8>, id: u8, payload: &[u8]) {
    out.push(id);
    out.extend_from_slice(payload);
}

fn decode_tlv(input: &[u8]) -> Result<(u8, &[u8], usize)> {
    if input.len() < 2 {
        return Err(Error::Truncated {
            needed: 2,
            actual: input.len(),
        });
    }
    let id = input[0];
    let length = input[1] as usize;
    if input.len() < 2 + length {
        return Err(Error::Truncated {
            needed: 2 + length,
            actual: input.len(),
        });
    }
    Ok((id, &input[2..2 + length], 2 + length))
}

fn set_once<T>(slot: &mut Option<T>, value: T, message_type: u8, id: u8) -> Result<()> {
    if slot.is_some() {
        return Err(Error::DuplicateElement { message_type, id });
    }
    *slot = Some(value);
    Ok(())
}

fn set_marker_once(slot: &mut bool, message_type: u8, id: u8) -> Result<()> {
    if *slot {
        return Err(Error::DuplicateElement { message_type, id });
    }
    *slot = true;
    Ok(())
}

fn decode_lv(input: &[u8]) -> Result<(u8, &[u8], usize)> {
    if input.is_empty() {
        return Err(Error::Truncated {
            needed: 1,
            actual: 0,
        });
    }
    let len = input[0] as usize;
    if input.len() < 1 + len {
        return Err(Error::Truncated {
            needed: 1 + len,
            actual: input.len(),
        });
    }
    Ok((len as u8, &input[1..1 + len], 1 + len))
}

fn take_fixed(input: &[u8], payload_len: usize) -> Result<&[u8]> {
    if input.is_empty() {
        return Err(Error::Truncated {
            needed: 1 + payload_len,
            actual: 0,
        });
    }
    ensure_remaining(input, 1, payload_len)?;
    Ok(&input[1..1 + payload_len])
}

fn ensure_remaining(input: &[u8], offset: usize, needed: usize) -> Result<()> {
    if input.len() < offset + needed {
        return Err(Error::Truncated {
            needed: offset + needed,
            actual: input.len(),
        });
    }
    Ok(())
}

fn push_l3_lv(out: &mut Vec<u8>, payload: &[u8]) -> Result<()> {
    if payload.len() > u8::MAX as usize {
        return Err(Error::InvalidLength {
            expected: u8::MAX as usize,
            actual: payload.len(),
        });
    }
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    Ok(())
}

fn push_l3_tlv(out: &mut Vec<u8>, id: u8, payload: &[u8]) -> Result<()> {
    push_tlv(out, id, payload)
}

fn encode_imsi(imsi: &str) -> Result<Vec<u8>> {
    let digits: Vec<u8> = imsi
        .chars()
        .map(|c| c.to_digit(10).map(|d| d as u8))
        .collect::<Option<Vec<_>>>()
        .ok_or(Error::InvalidValue {
            context: "IMSI Mobile Identity",
            reason: "IMSI must contain digits only",
        })?;
    if !(10..=15).contains(&digits.len()) {
        return Err(Error::InvalidValue {
            context: "IMSI Mobile Identity",
            reason: "IMSI must contain 10 to 15 digits",
        });
    }
    let odd = digits.len() % 2 == 1;
    let mut out = Vec::with_capacity(1 + digits.len().div_ceil(2));
    out.push((digits[0] << 4) | ((odd as u8) << 3) | 0b110);
    let mut index = 1;
    while index < digits.len() {
        let low = digits[index];
        let high = if index + 1 < digits.len() {
            digits[index + 1]
        } else {
            0x0f
        };
        out.push((high << 4) | low);
        index += 2;
    }
    Ok(out)
}

fn decode_imsi(first: u8, rest: &[u8]) -> Result<MobileIdentity> {
    let odd = first & 0x08 != 0;
    let mut digits = String::new();
    digits.push(char::from(b'0' + ((first >> 4) & 0x0f)));
    for (index, byte) in rest.iter().copied().enumerate() {
        let low = byte & 0x0f;
        let high = byte >> 4;
        digits.push(char::from(b'0' + low));
        let is_last_high_nibble = index == rest.len() - 1;
        if !(is_last_high_nibble && !odd && high == 0x0f) {
            digits.push(char::from(b'0' + high));
        }
    }
    Ok(MobileIdentity::Imsi(digits))
}

// ──────────────────────────────────────────────────────────────────────────────
// ADDS (Application Data Delivery Service) messages — A.S0001 §6.1.7
// ──────────────────────────────────────────────────────────────────────────────

/// ADDS User Part IE payload (A.S0001 §6.2.2.67).
///
/// Carries the application data to be delivered to or from the mobile station.
/// For SMS, `burst_type = 0x03` and `data` is a C.S0015-B encoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsUserPart {
    /// Data Burst Type: 0x03 = SMS, 0x04 = OTA, 0x05 = PLD.
    pub burst_type: u8,
    /// Application data message (C.S0015-B payload for SMS).
    pub data: Vec<u8>,
}

impl AddsUserPart {
    /// Public encoder for tests and external decoders.
    pub fn encode_body_public(&self) -> Vec<u8> {
        self.encode_body()
    }

    /// Public decoder for tests and external decoders.
    pub fn decode_body_public(input: &[u8]) -> Result<Self> {
        Self::decode_body(input)
    }

    /// Encodes as a BSMAP TLV value body (IE tag and length not included).
    pub(crate) fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.data.len());
        out.push(self.burst_type & 0x3f); // upper 2 bits reserved
        out.extend_from_slice(&self.data);
        out
    }

    /// Decodes from BSMAP TLV value bytes (after IE tag and length).
    pub(crate) fn decode_body(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::Truncated {
                needed: 1,
                actual: 0,
            });
        }
        Ok(Self {
            burst_type: input[0] & 0x3f,
            data: input[1..].to_vec(),
        })
    }
}

/// BSMAP `ADDS Page` message — MSC→BS (A.S0001 §6.1.7.1).
///
/// Requests the BS to deliver an application data message to an idle mobile
/// station on the paging channel. For SMS, carries the SMS Deliver payload in
/// `adds_user_part`. The optional `tag` enables Layer 2 acknowledgement
/// notification from BS to MSC via `AddsPageAckMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsPageMessage {
    /// Mobile Identity of the target MS (IMSI required).
    pub mobile_identity: MobileIdentity,
    /// Application data to deliver (SMS payload + burst type).
    pub adds_user_part: AddsUserPart,
    /// Optional correlation tag echoed in the ADDS Page Ack.
    pub tag: Option<Tag>,
    /// Optional slotted-paging cycle index for the target MS.
    pub slot_cycle_index: Option<SlotCycleIndex>,
}

impl AddsPageMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let imsi = self.mobile_identity.encode()?;
        if !matches!(self.mobile_identity, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "ADDS Page",
                reason: "mobile identity must be IMSI",
            });
        }
        push_tlv(&mut body, IE_MOBILE_IDENTITY, &imsi)?;
        let user_part_body = self.adds_user_part.encode_body();
        push_tlv(&mut body, IE_ADDS_USER_PART, &user_part_body)?;
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(sci) = self.slot_cycle_index {
            push_fixed(&mut body, IE_SLOT_CYCLE_INDEX, &[sci.encode()?]);
        }
        encode_bsmap(ADDS_PAGE, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ADDS_PAGE, input)?;
        let mut offset = 0;
        let mut mobile_identity = None;
        let mut adds_user_part = None;
        let mut tag = None;
        let mut slot_cycle_index = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut mobile_identity,
                        MobileIdentity::decode(payload)?,
                        ADDS_PAGE,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_ADDS_USER_PART => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut adds_user_part,
                        AddsUserPart::decode_body(payload)?,
                        ADDS_PAGE,
                        IE_ADDS_USER_PART,
                    )?;
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, ADDS_PAGE, IE_TAG)?;
                    offset += 5;
                }
                IE_SLOT_CYCLE_INDEX => {
                    let payload = take_fixed(&body[offset..], 1)?;
                    set_once(
                        &mut slot_cycle_index,
                        SlotCycleIndex::decode(payload[0])?,
                        ADDS_PAGE,
                        IE_SLOT_CYCLE_INDEX,
                    )?;
                    offset += 2;
                }
                _ => {
                    // Skip unknown optional IEs using TLV length
                    let (_, _, consumed) = decode_tlv(&body[offset..])?;
                    offset += consumed;
                }
            }
        }
        Ok(Self {
            mobile_identity: mobile_identity.ok_or(Error::MissingRequiredElement {
                message_type: ADDS_PAGE,
                id: IE_MOBILE_IDENTITY,
            })?,
            adds_user_part: adds_user_part.ok_or(Error::MissingRequiredElement {
                message_type: ADDS_PAGE,
                id: IE_ADDS_USER_PART,
            })?,
            tag,
            slot_cycle_index,
        })
    }
}

/// BSMAP `ADDS Transfer` message — BS→MSC (A.S0014-D §3.6.3).
///
/// Sent by the BS to the MSC when a mobile-originated application data message
/// is received on the access channel. Carries SMS, OTASP, PDS, or SDB payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsTransferMessage {
    /// IMSI of the originating mobile.
    pub mobile_identity_imsi: MobileIdentity,
    /// Application data received from the MS (payload + Data Burst Type).
    pub adds_user_part: AddsUserPart,
    /// ESN of the originating mobile.
    pub mobile_identity_esn: Option<MobileIdentity>,
    /// MEID of the originating mobile.
    pub mobile_identity_meid: Option<MobileIdentity>,
    /// Cell identifier where the application data was received.
    pub cell_identifier: Option<CellId>,
    /// Correlation tag echoed in the ADDS Transfer Ack.
    pub tag: Option<Tag>,
    /// Service Option (included when origination-triggered).
    pub service_option: Option<ServiceOption>,
}

impl AddsTransferMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "ADDS Transfer",
                reason: "first mobile identity must be IMSI",
            });
        }
        let imsi = self.mobile_identity_imsi.encode()?;
        push_tlv(&mut body, IE_MOBILE_IDENTITY, &imsi)?;
        let user_part_body = self.adds_user_part.encode_body();
        push_tlv(&mut body, IE_ADDS_USER_PART, &user_part_body)?;
        if let Some(esn) = &self.mobile_identity_esn {
            if !matches!(esn, MobileIdentity::Esn(_)) {
                return Err(Error::InvalidValue {
                    context: "ADDS Transfer",
                    reason: "ESN identity must be ESN variant",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &esn.encode()?)?;
        }
        if let Some(cell) = self.cell_identifier {
            push_tlv(&mut body, IE_CELL_IDENTIFIER, &cell.encode()?)?;
        }
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(service_option) = self.service_option {
            push_fixed(&mut body, IE_SERVICE_OPTION, &service_option.encode());
        }
        if let Some(meid) = &self.mobile_identity_meid {
            if !matches!(meid, MobileIdentity::Meid(_)) {
                return Err(Error::InvalidValue {
                    context: "ADDS Transfer",
                    reason: "MEID identity must be MEID variant",
                });
            }
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &meid.encode()?)?;
        }
        encode_bsmap(ADDS_TRANSFER, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ADDS_TRANSFER, input)?;
        let mut offset = 0;
        let mut mobile_identity_imsi = None;
        let mut adds_user_part = None;
        let mut mobile_identity_esn = None;
        let mut mobile_identity_meid = None;
        let mut cell_identifier = None;
        let mut tag = None;
        let mut service_option = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    match &identity {
                        MobileIdentity::Imsi(_) => {
                            if mobile_identity_imsi.is_some() {
                                return Err(Error::DuplicateElement {
                                    message_type: ADDS_TRANSFER,
                                    id: IE_MOBILE_IDENTITY,
                                });
                            }
                            mobile_identity_imsi = Some(identity);
                        }
                        MobileIdentity::Esn(_) => mobile_identity_esn = Some(identity),
                        MobileIdentity::Meid(_) => mobile_identity_meid = Some(identity),
                    }
                    offset += consumed;
                }
                IE_ADDS_USER_PART => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut adds_user_part,
                        AddsUserPart::decode_body(payload)?,
                        ADDS_TRANSFER,
                        IE_ADDS_USER_PART,
                    )?;
                    offset += consumed;
                }
                IE_CELL_IDENTIFIER => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut cell_identifier,
                        CellId::decode(payload)?,
                        ADDS_TRANSFER,
                        IE_CELL_IDENTIFIER,
                    )?;
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, ADDS_TRANSFER, IE_TAG)?;
                    offset += 5;
                }
                IE_SERVICE_OPTION => {
                    let payload = take_fixed(&body[offset..], 2)?;
                    set_once(
                        &mut service_option,
                        ServiceOption::decode(payload)?,
                        ADDS_TRANSFER,
                        IE_SERVICE_OPTION,
                    )?;
                    offset += 3;
                }
                _ => {
                    let (_, _, consumed) = decode_tlv(&body[offset..])?;
                    offset += consumed;
                }
            }
        }
        Ok(Self {
            mobile_identity_imsi: mobile_identity_imsi.ok_or(Error::MissingRequiredElement {
                message_type: ADDS_TRANSFER,
                id: IE_MOBILE_IDENTITY,
            })?,
            adds_user_part: adds_user_part.ok_or(Error::MissingRequiredElement {
                message_type: ADDS_TRANSFER,
                id: IE_ADDS_USER_PART,
            })?,
            mobile_identity_esn,
            mobile_identity_meid,
            cell_identifier,
            tag,
            service_option,
        })
    }
}

/// BSMAP `ADDS Transfer Ack` message — MSC→BS (A.S0014-D §3.6.4).
///
/// Sent by the MSC to acknowledge an ADDS Transfer. Required for SDB and
/// dormant-handoff flows; the Tag is echoed from the original Transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsTransferAckMessage {
    /// IMSI of the mobile (mandatory per §3.6.4).
    pub mobile_identity_imsi: MobileIdentity,
    /// Correlation tag echoed from the ADDS Transfer.
    pub tag: Option<Tag>,
    /// Failure cause — absent on success.
    pub cause: Option<Cause>,
}

impl AddsTransferAckMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if !matches!(self.mobile_identity_imsi, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "ADDS Transfer Ack",
                reason: "mobile identity must be IMSI",
            });
        }
        let mut body = Vec::new();
        push_tlv(
            &mut body,
            IE_MOBILE_IDENTITY,
            &self.mobile_identity_imsi.encode()?,
        )?;
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(cause) = self.cause {
            push_fixed(&mut body, IE_CAUSE, &cause.encode());
        }
        encode_bsmap(ADDS_TRANSFER_ACK, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ADDS_TRANSFER_ACK, input)?;
        let mut offset = 0;
        let mut mobile_identity_imsi = None;
        let mut tag = None;
        let mut cause = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    set_once(
                        &mut mobile_identity_imsi,
                        MobileIdentity::decode(payload)?,
                        ADDS_TRANSFER_ACK,
                        IE_MOBILE_IDENTITY,
                    )?;
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, ADDS_TRANSFER_ACK, IE_TAG)?;
                    offset += 5;
                }
                IE_CAUSE => {
                    let payload = take_fixed(&body[offset..], 1)?;
                    set_once(
                        &mut cause,
                        Cause::decode(payload)?,
                        ADDS_TRANSFER_ACK,
                        IE_CAUSE,
                    )?;
                    offset += 2;
                }
                _ => {
                    let (_, _, consumed) = decode_tlv(&body[offset..])?;
                    offset += consumed;
                }
            }
        }
        Ok(Self {
            mobile_identity_imsi: mobile_identity_imsi.ok_or(Error::MissingRequiredElement {
                message_type: ADDS_TRANSFER_ACK,
                id: IE_MOBILE_IDENTITY,
            })?,
            tag,
            cause,
        })
    }
}

/// BSMAP `ADDS Page Ack` message — BS→MSC (A.S0001 §6.1.7.4).
///
/// Sent by the BS to the MSC after receiving a Layer 2 acknowledgement from the
/// MS for an `ADDS Page` message. The `cause` field is absent on success and
/// present on failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsPageAckMessage {
    /// IMSI of the mobile that acknowledged (or failed to receive) the page.
    pub mobile_identity: MobileIdentity,
    /// Correlation tag echoed from the ADDS Page message.
    pub tag: Option<Tag>,
    /// ESN of the acknowledging mobile.
    pub mobile_identity_esn: Option<MobileIdentity>,
    /// Failure cause — absent means successful delivery.
    pub cause: Option<Cause>,
}

impl AddsPageAckMessage {
    /// Encodes the message using the exact A1 BSMAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let imsi = self.mobile_identity.encode()?;
        if !matches!(self.mobile_identity, MobileIdentity::Imsi(_)) {
            return Err(Error::InvalidValue {
                context: "ADDS Page Ack",
                reason: "mobile identity must be IMSI",
            });
        }
        push_tlv(&mut body, IE_MOBILE_IDENTITY, &imsi)?;
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(esn) = &self.mobile_identity_esn {
            push_tlv(&mut body, IE_MOBILE_IDENTITY, &esn.encode()?)?;
        }
        if let Some(cause) = self.cause {
            push_fixed(&mut body, IE_CAUSE, &cause.encode());
        }
        encode_bsmap(ADDS_PAGE_ACK, &body)
    }

    /// Decodes the message from the exact A1 BSMAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_bsmap(ADDS_PAGE_ACK, input)?;
        let mut offset = 0;
        let mut mobile_identity = None;
        let mut tag = None;
        let mut mobile_identity_esn = None;
        let mut cause = None;
        while offset < body.len() {
            match body[offset] {
                IE_MOBILE_IDENTITY => {
                    let (_, payload, consumed) = decode_tlv(&body[offset..])?;
                    let identity = MobileIdentity::decode(payload)?;
                    if mobile_identity.is_none() {
                        mobile_identity = Some(identity);
                    } else {
                        mobile_identity_esn = Some(identity);
                    }
                    offset += consumed;
                }
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, ADDS_PAGE_ACK, IE_TAG)?;
                    offset += 5;
                }
                IE_CAUSE => {
                    let payload = take_fixed(&body[offset..], 1)?;
                    set_once(&mut cause, Cause::decode(payload)?, ADDS_PAGE_ACK, IE_CAUSE)?;
                    offset += 2;
                }
                _ => {
                    let (_, _, consumed) = decode_tlv(&body[offset..])?;
                    offset += consumed;
                }
            }
        }
        Ok(Self {
            mobile_identity: mobile_identity.ok_or(Error::MissingRequiredElement {
                message_type: ADDS_PAGE_ACK,
                id: IE_MOBILE_IDENTITY,
            })?,
            tag,
            mobile_identity_esn,
            cause,
        })
    }
}

/// DTAP `ADDS Deliver` message — bidirectional MSC↔BS (A.S0001 §6.1.7.3).
///
/// Sent by the MSC to the BS to deliver an SMS to a mobile on a traffic
/// channel, or sent by the BS to the MSC to deliver a mobile-originated SMS
/// received on a traffic channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsDeliverMessage {
    /// Application data to deliver (SMS payload + burst type).
    pub adds_user_part: AddsUserPart,
    /// Optional correlation tag (MSC→BS: present to request Layer 2 ack notification).
    pub tag: Option<Tag>,
}

impl AddsDeliverMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        // DTAP body for ADDS Deliver: [user_part_len][burst_type][data...] [optional Tag TLV]
        let user_part_body = self.adds_user_part.encode_body();
        let mut body = Vec::new();
        if user_part_body.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                expected: u8::MAX as usize,
                actual: user_part_body.len(),
            });
        }
        body.push(user_part_body.len() as u8);
        body.extend_from_slice(&user_part_body);
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        encode_a1_dtap_with_protocol(
            ADDS_DELIVER_DTAP,
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            &body,
        )
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_dtap_with_protocol(
            ADDS_DELIVER_DTAP,
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            input,
        )?;
        if body.is_empty() {
            return Err(Error::Truncated {
                needed: 1,
                actual: 0,
            });
        }
        let user_part_len = body[0] as usize;
        if body.len() < 1 + user_part_len {
            return Err(Error::Truncated {
                needed: 1 + user_part_len,
                actual: body.len(),
            });
        }
        let adds_user_part = AddsUserPart::decode_body(&body[1..1 + user_part_len])?;
        let mut offset = 1 + user_part_len;
        let mut tag = None;
        while offset < body.len() {
            match body[offset] {
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(&mut tag, Tag::decode(payload)?, ADDS_DELIVER_DTAP, IE_TAG)?;
                    offset += 5;
                }
                _ => {
                    let (_, _, consumed) = decode_tlv(&body[offset..])?;
                    offset += consumed;
                }
            }
        }
        Ok(Self {
            adds_user_part,
            tag,
        })
    }
}

/// DTAP `ADDS Deliver Ack` message — BS→MSC (A.S0001 §6.1.7.5).
///
/// Sent by the BS to the MSC when it receives a Layer 2 acknowledgement from
/// the MS for an `ADDS Deliver` message that contained a Tag element. The
/// `cause` field is absent on success and present on failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsDeliverAckMessage {
    /// Correlation tag echoed from the ADDS Deliver message.
    pub tag: Option<Tag>,
    /// Failure cause — absent means successful delivery.
    pub cause: Option<Cause>,
}

impl AddsDeliverAckMessage {
    /// Encodes the message using the exact A1 DTAP wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        if let Some(tag) = self.tag {
            push_fixed(&mut body, IE_TAG, &tag.encode());
        }
        if let Some(cause) = self.cause {
            push_fixed(&mut body, IE_CAUSE, &cause.encode());
        }
        encode_a1_dtap_with_protocol(
            ADDS_DELIVER_ACK_DTAP,
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            &body,
        )
    }

    /// Decodes the message from the exact A1 DTAP wire format.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let body = parse_a1_dtap_with_protocol(
            ADDS_DELIVER_ACK_DTAP,
            DTAP_PROTOCOL_DISCRIMINATOR_CALL_PROCESSING,
            input,
        )?;
        let mut offset = 0;
        let mut tag = None;
        let mut cause = None;
        while offset < body.len() {
            match body[offset] {
                IE_TAG => {
                    let payload = take_fixed(&body[offset..], 4)?;
                    set_once(
                        &mut tag,
                        Tag::decode(payload)?,
                        ADDS_DELIVER_ACK_DTAP,
                        IE_TAG,
                    )?;
                    offset += 5;
                }
                IE_CAUSE => {
                    let payload = take_fixed(&body[offset..], 1)?;
                    set_once(
                        &mut cause,
                        Cause::decode(payload)?,
                        ADDS_DELIVER_ACK_DTAP,
                        IE_CAUSE,
                    )?;
                    offset += 2;
                }
                _ => {
                    let (_, _, consumed) = decode_tlv(&body[offset..])?;
                    offset += consumed;
                }
            }
        }
        Ok(Self { tag, cause })
    }
}
