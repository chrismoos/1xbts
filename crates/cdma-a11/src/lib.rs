//! A11 registration and session signaling codec primitives between the PCF and PDSN.

use std::collections::HashSet;
use std::fmt::{Display, Formatter};

pub mod procedure;
pub mod transport;

pub use procedure::{
    ClearReason, Direction, ProcedureEvent, SessionKey, SessionProcedureTable, SessionSnapshot,
    SessionState,
};
pub use transport::{UdpEndpoint, UdpFrame, VerifiedUdpFrame};

const THREEGPP2_VENDOR_ID: u32 = 0x0000_159f;
const PROTOCOL_TYPE_UNSTRUCTURED_BYTE_STREAM: u16 = 0x8881;
const SESSION_SPECIFIC_MSID_TYPE_IMSI: u16 = 0x0006;

/// Errors returned by the A11 codec helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyMessage,
    UnknownMessageType(u8),
    Truncated {
        needed: usize,
        actual: usize,
    },
    InvalidExtensionLength {
        expected_min: usize,
        actual: usize,
    },
    /// RFC 3344 §3.5.3: SPI must be non-zero.
    InvalidSpi,
    InvalidValue {
        context: &'static str,
        reason: &'static str,
    },
    DuplicateExtension {
        extension_type: u8,
    },
    AuthenticationRejected {
        context: &'static str,
        reason: &'static str,
    },
    ProcedureViolation {
        context: &'static str,
        reason: &'static str,
    },
}

/// Result type used by the `cdma-a11` crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

/// Supported A11 message type values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    RegistrationRequest = 0x01,
    RegistrationReply = 0x03,
    RegistrationUpdate = 0x14,
    RegistrationAcknowledge = 0x15,
    SessionUpdate = 0x16,
    SessionUpdateAcknowledge = 0x17,
    CapabilitiesInfo = 0x18,
    CapabilitiesInfoAcknowledge = 0x19,
}

impl MessageType {
    /// Parses an A11 message type value.
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Self::RegistrationRequest),
            0x03 => Ok(Self::RegistrationReply),
            0x14 => Ok(Self::RegistrationUpdate),
            0x15 => Ok(Self::RegistrationAcknowledge),
            0x16 => Ok(Self::SessionUpdate),
            0x17 => Ok(Self::SessionUpdateAcknowledge),
            0x18 => Ok(Self::CapabilitiesInfo),
            0x19 => Ok(Self::CapabilitiesInfoAcknowledge),
            other => Err(Error::UnknownMessageType(other)),
        }
    }
}

/// Exact on-wire authentication extension type values used by A11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthenticationExtensionType {
    MobileHome = 0x20,
    RegistrationUpdate = 0x28,
}

impl AuthenticationExtensionType {
    /// Parses an on-wire authentication extension type value.
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x20 => Ok(Self::MobileHome),
            0x28 => Ok(Self::RegistrationUpdate),
            _ => Err(Error::InvalidValue {
                context: "authentication.extension_type",
                reason: "unsupported authentication extension type",
            }),
        }
    }
}

/// Mobile-IP authentication extension used by A11 procedures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationExtension {
    pub extension_type: AuthenticationExtensionType,
    pub security_parameter_index: u32,
    pub authenticator: Vec<u8>,
}

impl AuthenticationExtension {
    /// Encodes the authentication extension.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.security_parameter_index == 0 {
            return Err(Error::InvalidSpi);
        }
        if self.authenticator.len() != 16 {
            return Err(Error::InvalidExtensionLength {
                expected_min: 16,
                actual: self.authenticator.len(),
            });
        }
        let length = 4 + self.authenticator.len();
        let mut out = Vec::with_capacity(2 + length);
        out.push(self.extension_type as u8);
        out.push(length as u8);
        out.extend_from_slice(&self.security_parameter_index.to_be_bytes());
        out.extend_from_slice(&self.authenticator);
        Ok(out)
    }

    /// Decodes an authentication extension payload.
    pub fn decode(extension_type: u8, value: &[u8]) -> Result<Self> {
        if value.len() < 4 {
            return Err(Error::InvalidExtensionLength {
                expected_min: 4,
                actual: value.len(),
            });
        }
        let spi = u32::from_be_bytes([value[0], value[1], value[2], value[3]]);
        if spi == 0 {
            return Err(Error::InvalidSpi);
        }
        let authenticator = value[4..].to_vec();
        if authenticator.len() != 16 {
            return Err(Error::InvalidExtensionLength {
                expected_min: 16,
                actual: authenticator.len(),
            });
        }
        Ok(Self {
            extension_type: AuthenticationExtensionType::from_u8(extension_type)?,
            security_parameter_index: spi,
            authenticator,
        })
    }
}

/// Unknown extension preserved for forward-compatible decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawExtension {
    pub extension_type: u8,
    pub value: Vec<u8>,
}

impl RawExtension {
    /// Encodes the raw extension.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.value.len() > u8::MAX as usize {
            return Err(Error::InvalidExtensionLength {
                expected_min: u8::MAX as usize,
                actual: self.value.len(),
            });
        }
        let mut out = Vec::with_capacity(2 + self.value.len());
        out.push(self.extension_type);
        out.push(self.value.len() as u8);
        out.extend_from_slice(&self.value);
        Ok(out)
    }
}

/// Known Session Parameter NVSE content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionParameterNvse {
    RnPdit(u8),
    AlwaysOn,
}

/// PDSN enabled-feature NVSE content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdsnEnabledFeatureNvse {
    FlowControlEnabled,
    PacketBoundaryEnabled,
}

/// PCF enabled-feature NVSE content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcfEnabledFeatureNvse {
    ShortDataIndicationSupported,
    GreSegmentationEnabled,
}

/// Unknown A.S0017 Normal Vendor/Organization Specific Extension content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownNvse {
    pub vendor_id: u32,
    pub application_type: u8,
    pub application_subtype: u8,
    pub application_data: Vec<u8>,
}

/// Exact typed A.S0017 Normal Vendor/Organization Specific Extension content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nvse {
    AccessNetworkIdentifiers(Vec<u8>),
    AnchorPPAddress([u8; 4]),
    AllDormantIndicator,
    PdsnCode(u8),
    SessionParameter(SessionParameterNvse),
    ServiceOption(u16),
    PdsnEnabledFeature(PdsnEnabledFeatureNvse),
    PcfEnabledFeature(PcfEnabledFeatureNvse),
    Unknown(UnknownNvse),
}

impl Nvse {
    /// Exact on-wire element type for an A.S0017 NVSE.
    pub const TYPE: u8 = 0x86;

    fn header_bytes(
        application_type: u8,
        application_subtype: u8,
        application_data_len: usize,
    ) -> Result<Vec<u8>> {
        let length = 8 + application_data_len;
        if length > u8::MAX as usize {
            return Err(Error::InvalidExtensionLength {
                expected_min: u8::MAX as usize,
                actual: length,
            });
        }
        let mut out = Vec::with_capacity(2 + length);
        out.push(Self::TYPE);
        out.push(length as u8);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&THREEGPP2_VENDOR_ID.to_be_bytes());
        out.push(application_type);
        out.push(application_subtype);
        Ok(out)
    }

    /// Encodes the NVSE into `type | length | value`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::AccessNetworkIdentifiers(data) => {
                let mut out = Self::header_bytes(0x04, 0x01, data.len())?;
                out.extend_from_slice(data);
                Ok(out)
            }
            Self::AnchorPPAddress(address) => {
                let mut out = Self::header_bytes(0x05, 0x01, 4)?;
                out.extend_from_slice(address);
                Ok(out)
            }
            Self::AllDormantIndicator => {
                let mut out = Self::header_bytes(0x06, 0x01, 2)?;
                out.extend_from_slice(&[0, 0]);
                Ok(out)
            }
            Self::PdsnCode(code) => {
                if !matches!(*code, 0xc1..=0xc8 | 0xca | 0xcb) {
                    return Err(Error::InvalidValue {
                        context: "nvse.pdsn_code",
                        reason: "unsupported PDSN Code value",
                    });
                }
                let mut out = Self::header_bytes(0x07, 0x01, 1)?;
                out.push(*code);
                Ok(out)
            }
            Self::SessionParameter(SessionParameterNvse::RnPdit(value)) => {
                if *value == 0 {
                    return Err(Error::InvalidValue {
                        context: "nvse.rn_pdit",
                        reason: "RN-PDIT must be in the range 1..=255 seconds",
                    });
                }
                let mut out = Self::header_bytes(0x08, 0x01, 1)?;
                out.push(*value);
                Ok(out)
            }
            Self::SessionParameter(SessionParameterNvse::AlwaysOn) => {
                Self::header_bytes(0x08, 0x02, 0)
            }
            Self::ServiceOption(value) => {
                let mut out = Self::header_bytes(0x09, 0x01, 2)?;
                out.extend_from_slice(&value.to_be_bytes());
                Ok(out)
            }
            Self::PdsnEnabledFeature(PdsnEnabledFeatureNvse::FlowControlEnabled) => {
                Self::header_bytes(0x0a, 0x01, 0)
            }
            Self::PdsnEnabledFeature(PdsnEnabledFeatureNvse::PacketBoundaryEnabled) => {
                Self::header_bytes(0x0a, 0x02, 0)
            }
            Self::PcfEnabledFeature(PcfEnabledFeatureNvse::ShortDataIndicationSupported) => {
                Self::header_bytes(0x0b, 0x01, 0)
            }
            Self::PcfEnabledFeature(PcfEnabledFeatureNvse::GreSegmentationEnabled) => {
                Self::header_bytes(0x0b, 0x02, 0)
            }
            Self::Unknown(unknown) => {
                let length = 8 + unknown.application_data.len();
                if length > u8::MAX as usize {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: u8::MAX as usize,
                        actual: length,
                    });
                }
                let mut out = Vec::with_capacity(2 + length);
                out.push(Self::TYPE);
                out.push(length as u8);
                out.extend_from_slice(&[0, 0]);
                out.extend_from_slice(&unknown.vendor_id.to_be_bytes());
                out.push(unknown.application_type);
                out.push(unknown.application_subtype);
                out.extend_from_slice(&unknown.application_data);
                Ok(out)
            }
        }
    }

    /// Decodes one exact NVSE from the supplied bytes.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 10 {
            return Err(Error::InvalidExtensionLength {
                expected_min: 10,
                actual: input.len().saturating_sub(2),
            });
        }
        if input[0] != Self::TYPE {
            return Err(Error::InvalidValue {
                context: "nvse.type",
                reason: "A.S0017 NVSE must use element type 0x86",
            });
        }
        let length = input[1] as usize;
        if input.len() != 2 + length {
            return Err(Error::Truncated {
                needed: 2 + length,
                actual: input.len(),
            });
        }
        if length < 8 {
            return Err(Error::InvalidExtensionLength {
                expected_min: 8,
                actual: length,
            });
        }
        let value = &input[2..];
        if value[0] != 0 || value[1] != 0 {
            return Err(Error::InvalidValue {
                context: "nvse.reserved",
                reason: "reserved NVSE bytes must be zero",
            });
        }
        let vendor_id = u32::from_be_bytes([value[2], value[3], value[4], value[5]]);
        let application_type = value[6];
        let application_subtype = value[7];
        let application_data = &value[8..];

        if vendor_id != THREEGPP2_VENDOR_ID {
            return Ok(Self::Unknown(UnknownNvse {
                vendor_id,
                application_type,
                application_subtype,
                application_data: application_data.to_vec(),
            }));
        }

        match (application_type, application_subtype) {
            (0x04, 0x01) => Ok(Self::AccessNetworkIdentifiers(application_data.to_vec())),
            (0x05, 0x01) => {
                if application_data.len() != 4 {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 4,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::AnchorPPAddress([
                    application_data[0],
                    application_data[1],
                    application_data[2],
                    application_data[3],
                ]))
            }
            (0x06, 0x01) => {
                if application_data != [0, 0] {
                    return Err(Error::InvalidValue {
                        context: "nvse.all_dormant_indicator",
                        reason: "All Dormant Indicator must encode the value 0x0000",
                    });
                }
                Ok(Self::AllDormantIndicator)
            }
            (0x07, 0x01) => {
                if application_data.len() != 1 {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 1,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::PdsnCode(application_data[0]))
            }
            (0x08, 0x01) => {
                if application_data.len() != 1 {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 1,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::SessionParameter(SessionParameterNvse::RnPdit(
                    application_data[0],
                )))
            }
            (0x08, 0x02) => {
                if !application_data.is_empty() {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 0,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::SessionParameter(SessionParameterNvse::AlwaysOn))
            }
            (0x09, 0x01) => {
                if application_data.len() != 2 {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 2,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::ServiceOption(u16::from_be_bytes([
                    application_data[0],
                    application_data[1],
                ])))
            }
            (0x0a, 0x01) => {
                if !application_data.is_empty() {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 0,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::PdsnEnabledFeature(
                    PdsnEnabledFeatureNvse::FlowControlEnabled,
                ))
            }
            (0x0a, 0x02) => {
                if !application_data.is_empty() {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 0,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::PdsnEnabledFeature(
                    PdsnEnabledFeatureNvse::PacketBoundaryEnabled,
                ))
            }
            (0x0b, 0x01) => {
                if !application_data.is_empty() {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 0,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::PcfEnabledFeature(
                    PcfEnabledFeatureNvse::ShortDataIndicationSupported,
                ))
            }
            (0x0b, 0x02) => {
                if !application_data.is_empty() {
                    return Err(Error::InvalidExtensionLength {
                        expected_min: 0,
                        actual: application_data.len(),
                    });
                }
                Ok(Self::PcfEnabledFeature(
                    PcfEnabledFeatureNvse::GreSegmentationEnabled,
                ))
            }
            _ => Ok(Self::Unknown(UnknownNvse {
                vendor_id,
                application_type,
                application_subtype,
                application_data: application_data.to_vec(),
            })),
        }
    }

    fn application_key(&self) -> (u32, u8, u8) {
        match self {
            Self::AccessNetworkIdentifiers(_) => (THREEGPP2_VENDOR_ID, 0x04, 0x01),
            Self::AnchorPPAddress(_) => (THREEGPP2_VENDOR_ID, 0x05, 0x01),
            Self::AllDormantIndicator => (THREEGPP2_VENDOR_ID, 0x06, 0x01),
            Self::PdsnCode(_) => (THREEGPP2_VENDOR_ID, 0x07, 0x01),
            Self::SessionParameter(SessionParameterNvse::RnPdit(_)) => {
                (THREEGPP2_VENDOR_ID, 0x08, 0x01)
            }
            Self::SessionParameter(SessionParameterNvse::AlwaysOn) => {
                (THREEGPP2_VENDOR_ID, 0x08, 0x02)
            }
            Self::ServiceOption(_) => (THREEGPP2_VENDOR_ID, 0x09, 0x01),
            Self::PdsnEnabledFeature(PdsnEnabledFeatureNvse::FlowControlEnabled) => {
                (THREEGPP2_VENDOR_ID, 0x0a, 0x01)
            }
            Self::PdsnEnabledFeature(PdsnEnabledFeatureNvse::PacketBoundaryEnabled) => {
                (THREEGPP2_VENDOR_ID, 0x0a, 0x02)
            }
            Self::PcfEnabledFeature(PcfEnabledFeatureNvse::ShortDataIndicationSupported) => {
                (THREEGPP2_VENDOR_ID, 0x0b, 0x01)
            }
            Self::PcfEnabledFeature(PcfEnabledFeatureNvse::GreSegmentationEnabled) => {
                (THREEGPP2_VENDOR_ID, 0x0b, 0x02)
            }
            Self::Unknown(unknown) => (
                unknown.vendor_id,
                unknown.application_type,
                unknown.application_subtype,
            ),
        }
    }
}

/// Supported typed A11 extension inventory carried after the base header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extension {
    SessionSpecific(SessionSpecificExtension),
    Authentication(AuthenticationExtension),
    Nvse(Nvse),
    Raw(RawExtension),
}

impl Extension {
    /// Returns the on-wire extension type value.
    pub fn extension_type(&self) -> u8 {
        match self {
            Self::SessionSpecific(_) => SessionSpecificExtension::TYPE,
            Self::Authentication(extension) => extension.extension_type as u8,
            Self::Nvse(_) => Nvse::TYPE,
            Self::Raw(extension) => extension.extension_type,
        }
    }

    /// Encodes the extension into `type | length | value`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::SessionSpecific(extension) => extension.encode(),
            Self::Authentication(extension) => extension.encode(),
            Self::Nvse(extension) => extension.encode(),
            Self::Raw(extension) => extension.encode(),
        }
    }

    /// Decodes a single extension from an exact byte slice.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let extension_type = input[0];
        let length = input[1] as usize;
        if input.len() != 2 + length {
            return Err(Error::Truncated {
                needed: 2 + length,
                actual: input.len(),
            });
        }
        let value = &input[2..];
        match extension_type {
            SessionSpecificExtension::TYPE => {
                let (extension, used) = SessionSpecificExtension::decode(input)?;
                if used != input.len() {
                    return Err(Error::Truncated {
                        needed: input.len(),
                        actual: used,
                    });
                }
                Ok(Self::SessionSpecific(extension))
            }
            0x20 | 0x28 => Ok(Self::Authentication(AuthenticationExtension::decode(
                extension_type,
                value,
            )?)),
            0x86 => Ok(Self::Nvse(Nvse::decode(input)?)),
            _ => Ok(Self::Raw(RawExtension {
                extension_type,
                value: value.to_vec(),
            })),
        }
    }
}

/// Structured Session Specific Extension carrying the PCF session identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpecificExtension {
    pub protocol_type: u16,
    pub pcf_session_id: u32,
    pub session_id_version: u8,
    pub mn_session_reference_id: u16,
    pub mn_id_type: u16,
    pub mn_id: Vec<u8>,
}

impl SessionSpecificExtension {
    pub const TYPE: u8 = 0x27;

    /// Encodes the Session Specific Extension.
    pub fn encode(&self) -> Result<Vec<u8>> {
        validate_session(self)?;
        let len = 13 + self.mn_id.len();
        let mut out = Vec::with_capacity(2 + len);
        out.push(Self::TYPE);
        out.push(len as u8);
        out.extend_from_slice(&self.protocol_type.to_be_bytes());
        out.extend_from_slice(&self.pcf_session_id.to_be_bytes());
        out.push(0);
        out.push(self.session_id_version << 6);
        out.extend_from_slice(&self.mn_session_reference_id.to_be_bytes());
        out.extend_from_slice(&self.mn_id_type.to_be_bytes());
        out.push(self.mn_id.len() as u8);
        out.extend_from_slice(&self.mn_id);
        Ok(out)
    }

    /// Decodes a Session Specific Extension and returns the bytes consumed.
    pub fn decode(input: &[u8]) -> Result<(Self, usize)> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                needed: 2,
                actual: input.len(),
            });
        }
        let len = input[1] as usize;
        if input.len() < 2 + len {
            return Err(Error::Truncated {
                needed: 2 + len,
                actual: input.len(),
            });
        }
        if input[0] != Self::TYPE || len < 13 {
            return Err(Error::InvalidExtensionLength {
                expected_min: 13,
                actual: len,
            });
        }
        let value = &input[2..2 + len];
        if value[6] != 0 {
            return Err(Error::InvalidValue {
                context: "session.reserved",
                reason: "reserved session byte must be zero",
            });
        }
        if value[7] & 0x3f != 0 {
            return Err(Error::InvalidValue {
                context: "session.reserved",
                reason: "reserved session bits must be zero",
            });
        }
        let mn_len = value[12] as usize;
        if value.len() != 13 + mn_len {
            return Err(Error::InvalidExtensionLength {
                expected_min: 13 + mn_len,
                actual: value.len(),
            });
        }
        let session = Self {
            protocol_type: u16::from_be_bytes([value[0], value[1]]),
            pcf_session_id: u32::from_be_bytes([value[2], value[3], value[4], value[5]]),
            session_id_version: value[7] >> 6,
            mn_session_reference_id: u16::from_be_bytes([value[8], value[9]]),
            mn_id_type: u16::from_be_bytes([value[10], value[11]]),
            mn_id: value[13..13 + mn_len].to_vec(),
        };
        validate_session(&session)?;
        Ok((session, 2 + len))
    }
}

/// Decoded A11 Registration Request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRequest {
    pub flags: u8,
    pub lifetime: u16,
    pub home_address: [u8; 4],
    pub home_agent: [u8; 4],
    pub care_of_address: [u8; 4],
    pub identification: u64,
    pub session: SessionSpecificExtension,
    pub extensions: Vec<Extension>,
}

/// Decoded A11 Registration Reply body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationReply {
    pub code: u8,
    pub lifetime: u16,
    pub home_address: [u8; 4],
    pub home_agent: [u8; 4],
    pub identification: u64,
    pub session: SessionSpecificExtension,
    pub extensions: Vec<Extension>,
}

/// Decoded A11 Registration Update body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationUpdate {
    pub reserved: [u8; 3],
    pub home_address: [u8; 4],
    pub home_agent: [u8; 4],
    pub identification: u64,
    pub session: SessionSpecificExtension,
    pub nvses: Vec<Nvse>,
    pub authentication_extension: AuthenticationExtension,
}

/// Decoded A11 Registration Acknowledge body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationAcknowledge {
    pub reserved: [u8; 2],
    pub status: u8,
    pub home_address: [u8; 4],
    pub care_of_address: [u8; 4],
    pub identification: u64,
    pub session: SessionSpecificExtension,
    pub authentication_extension: AuthenticationExtension,
}

/// Decoded A11 Session Update body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUpdate {
    pub reserved: [u8; 3],
    pub home_address: [u8; 4],
    pub home_agent: [u8; 4],
    pub identification: u64,
    pub session: SessionSpecificExtension,
    pub nvses: Vec<Nvse>,
    pub authentication_extension: AuthenticationExtension,
}

/// Decoded A11 Session Update Acknowledge body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUpdateAcknowledge {
    pub reserved: [u8; 2],
    pub status: u8,
    pub home_address: [u8; 4],
    pub care_of_address: [u8; 4],
    pub identification: u64,
    pub session: SessionSpecificExtension,
    pub authentication_extension: AuthenticationExtension,
}

/// Decoded A11 Capabilities Info body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitiesInfo {
    pub reserved: [u8; 3],
    pub home_address: [u8; 4],
    pub home_agent: [u8; 4],
    pub care_of_address: [u8; 4],
    pub identification: u64,
    pub nvses: Vec<Nvse>,
    pub authentication_extension: AuthenticationExtension,
}

/// Decoded A11 Capabilities Info Acknowledge body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitiesInfoAcknowledge {
    pub reserved: [u8; 3],
    pub home_address: [u8; 4],
    pub care_of_address: [u8; 4],
    pub identification: u64,
    pub nvses: Vec<Nvse>,
    pub authentication_extension: AuthenticationExtension,
}

/// Supported A11 message bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    RegistrationRequest(RegistrationRequest),
    RegistrationReply(RegistrationReply),
    RegistrationUpdate(RegistrationUpdate),
    RegistrationAcknowledge(RegistrationAcknowledge),
    SessionUpdate(SessionUpdate),
    SessionUpdateAcknowledge(SessionUpdateAcknowledge),
    CapabilitiesInfo(CapabilitiesInfo),
    CapabilitiesInfoAcknowledge(CapabilitiesInfoAcknowledge),
}

impl Message {
    /// Returns the message type discriminator for this typed A11 message.
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::RegistrationRequest(_) => MessageType::RegistrationRequest,
            Self::RegistrationReply(_) => MessageType::RegistrationReply,
            Self::RegistrationUpdate(_) => MessageType::RegistrationUpdate,
            Self::RegistrationAcknowledge(_) => MessageType::RegistrationAcknowledge,
            Self::SessionUpdate(_) => MessageType::SessionUpdate,
            Self::SessionUpdateAcknowledge(_) => MessageType::SessionUpdateAcknowledge,
            Self::CapabilitiesInfo(_) => MessageType::CapabilitiesInfo,
            Self::CapabilitiesInfoAcknowledge(_) => MessageType::CapabilitiesInfoAcknowledge,
        }
    }

    fn required_authentication_extension(&self) -> Result<&AuthenticationExtension> {
        match self {
            Self::RegistrationRequest(message) => required_extension_authentication(
                &message.extensions,
                AuthenticationExtensionType::MobileHome,
                "registration request.extensions",
            ),
            Self::RegistrationReply(message) => required_extension_authentication(
                &message.extensions,
                AuthenticationExtensionType::MobileHome,
                "registration reply.extensions",
            ),
            Self::RegistrationUpdate(message) => Ok(&message.authentication_extension),
            Self::RegistrationAcknowledge(message) => Ok(&message.authentication_extension),
            Self::SessionUpdate(message) => Ok(&message.authentication_extension),
            Self::SessionUpdateAcknowledge(message) => Ok(&message.authentication_extension),
            Self::CapabilitiesInfo(message) => Ok(&message.authentication_extension),
            Self::CapabilitiesInfoAcknowledge(message) => Ok(&message.authentication_extension),
        }
    }
}

/// Explicit reason for decoding an A11 message without verifying its authenticator.
///
/// This exists to keep unauthenticated parsing visible at every call site. Production
/// receive paths should normally use [`decode_verified`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnverifiedDecodeReason {
    /// The message is decoded in a unit/integration test that is not exercising authentication.
    TestFixture,
    /// Authentication was already enforced by a containing transport or protocol layer.
    AuthenticatedByOuterLayer,
    /// The caller will verify the authenticator before applying the message to trusted state.
    DeferredAuthentication,
}

/// Verifies the authenticator carried by a decoded A11 message.
///
/// The crate owns the A11 wire parsing and identifies the exact authentication
/// extension that must be checked. Callers own SPI lookup, peer policy, replay
/// policy, and the concrete MAC implementation.
pub trait AuthenticationVerifier {
    /// Verify `authentication` for `message` as received in `wire_bytes`.
    fn verify_authentication(
        &self,
        wire_bytes: &[u8],
        message: &Message,
        authentication: &AuthenticationExtension,
    ) -> Result<()>;
}

impl<F> AuthenticationVerifier for F
where
    F: Fn(&[u8], &Message, &AuthenticationExtension) -> Result<()>,
{
    fn verify_authentication(
        &self,
        wire_bytes: &[u8],
        message: &Message,
        authentication: &AuthenticationExtension,
    ) -> Result<()> {
        self(wire_bytes, message, authentication)
    }
}

/// A decoded A11 message whose required authenticator has been accepted by a verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMessage {
    message: Message,
}

impl VerifiedMessage {
    /// Returns the verified typed message.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Consumes the wrapper and returns the typed message.
    pub fn into_message(self) -> Message {
        self.message
    }
}

impl AsRef<Message> for VerifiedMessage {
    fn as_ref(&self) -> &Message {
        self.message()
    }
}

/// Encodes an A11 message body.
pub fn encode(message: &Message) -> Result<Vec<u8>> {
    match message {
        Message::RegistrationRequest(message) => {
            encode_request(MessageType::RegistrationRequest, message)
        }
        Message::RegistrationReply(message) => {
            encode_reply(MessageType::RegistrationReply, message)
        }
        Message::RegistrationUpdate(message) => {
            encode_update(MessageType::RegistrationUpdate, message)
        }
        Message::RegistrationAcknowledge(message) => {
            encode_acknowledge(MessageType::RegistrationAcknowledge, message)
        }
        Message::SessionUpdate(message) => {
            encode_session_update(MessageType::SessionUpdate, message)
        }
        Message::SessionUpdateAcknowledge(message) => {
            encode_session_update_acknowledge(MessageType::SessionUpdateAcknowledge, message)
        }
        Message::CapabilitiesInfo(message) => {
            encode_capabilities_info(MessageType::CapabilitiesInfo, message)
        }
        Message::CapabilitiesInfoAcknowledge(message) => {
            encode_capabilities_info_ack(MessageType::CapabilitiesInfoAcknowledge, message)
        }
    }
}

/// Decodes and verifies an A11 message body.
pub fn decode_verified<V>(input: &[u8], verifier: &V) -> Result<VerifiedMessage>
where
    V: AuthenticationVerifier + ?Sized,
{
    let message = decode_message(input)?;
    let authentication = message.required_authentication_extension()?;
    verifier.verify_authentication(input, &message, authentication)?;
    Ok(VerifiedMessage { message })
}

/// Decodes an A11 message body without verifying its authenticator.
pub fn decode_unverified(input: &[u8], _reason: UnverifiedDecodeReason) -> Result<Message> {
    decode_message(input)
}

fn decode_message(input: &[u8]) -> Result<Message> {
    let Some((&message_type, _)) = input.split_first() else {
        return Err(Error::EmptyMessage);
    };
    match MessageType::from_u8(message_type)? {
        MessageType::RegistrationRequest => {
            Ok(Message::RegistrationRequest(decode_request(input)?))
        }
        MessageType::RegistrationReply => Ok(Message::RegistrationReply(decode_reply(input)?)),
        MessageType::RegistrationUpdate => Ok(Message::RegistrationUpdate(decode_update(input)?)),
        MessageType::RegistrationAcknowledge => {
            Ok(Message::RegistrationAcknowledge(decode_acknowledge(input)?))
        }
        MessageType::SessionUpdate => Ok(Message::SessionUpdate(decode_session_update(input)?)),
        MessageType::SessionUpdateAcknowledge => Ok(Message::SessionUpdateAcknowledge(
            decode_session_update_acknowledge(input)?,
        )),
        MessageType::CapabilitiesInfo => {
            Ok(Message::CapabilitiesInfo(decode_capabilities_info(input)?))
        }
        MessageType::CapabilitiesInfoAcknowledge => Ok(Message::CapabilitiesInfoAcknowledge(
            decode_capabilities_info_ack(input)?,
        )),
    }
}

fn encode_request(message_type: MessageType, message: &RegistrationRequest) -> Result<Vec<u8>> {
    validate_request(message)?;
    let mut out = vec![message_type as u8, message.flags];
    out.extend_from_slice(&message.lifetime.to_be_bytes());
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.home_agent);
    out.extend_from_slice(&message.care_of_address);
    out.extend_from_slice(&message.identification.to_be_bytes());
    out.extend_from_slice(&message.session.encode()?);
    for extension in &message.extensions {
        out.extend_from_slice(&extension.encode()?);
    }
    Ok(out)
}

fn encode_reply(message_type: MessageType, message: &RegistrationReply) -> Result<Vec<u8>> {
    validate_reply(message)?;
    let mut out = vec![message_type as u8, message.code];
    out.extend_from_slice(&message.lifetime.to_be_bytes());
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.home_agent);
    out.extend_from_slice(&message.identification.to_be_bytes());
    out.extend_from_slice(&message.session.encode()?);
    for extension in &message.extensions {
        out.extend_from_slice(&extension.encode()?);
    }
    Ok(out)
}

fn encode_update(message_type: MessageType, message: &RegistrationUpdate) -> Result<Vec<u8>> {
    validate_update(message)?;
    let mut out = vec![message_type as u8];
    out.extend_from_slice(&message.reserved);
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.home_agent);
    out.extend_from_slice(&message.identification.to_be_bytes());
    out.extend_from_slice(&message.session.encode()?);
    for nvse in &message.nvses {
        out.extend_from_slice(&nvse.encode()?);
    }
    out.extend_from_slice(&message.authentication_extension.encode()?);
    Ok(out)
}

fn encode_acknowledge(
    message_type: MessageType,
    message: &RegistrationAcknowledge,
) -> Result<Vec<u8>> {
    validate_acknowledge(message)?;
    let mut out = vec![message_type as u8];
    out.extend_from_slice(&message.reserved);
    out.push(message.status);
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.care_of_address);
    out.extend_from_slice(&message.identification.to_be_bytes());
    out.extend_from_slice(&message.session.encode()?);
    out.extend_from_slice(&message.authentication_extension.encode()?);
    Ok(out)
}

fn encode_session_update(message_type: MessageType, message: &SessionUpdate) -> Result<Vec<u8>> {
    validate_session_update(message)?;
    let mut out = vec![message_type as u8];
    out.extend_from_slice(&message.reserved);
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.home_agent);
    out.extend_from_slice(&message.identification.to_be_bytes());
    out.extend_from_slice(&message.session.encode()?);
    for nvse in &message.nvses {
        out.extend_from_slice(&nvse.encode()?);
    }
    out.extend_from_slice(&message.authentication_extension.encode()?);
    Ok(out)
}

fn encode_session_update_acknowledge(
    message_type: MessageType,
    message: &SessionUpdateAcknowledge,
) -> Result<Vec<u8>> {
    validate_session_update_acknowledge(message)?;
    let mut out = vec![message_type as u8];
    out.extend_from_slice(&message.reserved);
    out.push(message.status);
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.care_of_address);
    out.extend_from_slice(&message.identification.to_be_bytes());
    out.extend_from_slice(&message.session.encode()?);
    out.extend_from_slice(&message.authentication_extension.encode()?);
    Ok(out)
}

fn encode_capabilities_info(
    message_type: MessageType,
    message: &CapabilitiesInfo,
) -> Result<Vec<u8>> {
    validate_capabilities_info(message)?;
    let mut out = vec![message_type as u8];
    out.extend_from_slice(&message.reserved);
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.home_agent);
    out.extend_from_slice(&message.care_of_address);
    out.extend_from_slice(&message.identification.to_be_bytes());
    for nvse in &message.nvses {
        out.extend_from_slice(&nvse.encode()?);
    }
    out.extend_from_slice(&message.authentication_extension.encode()?);
    Ok(out)
}

fn encode_capabilities_info_ack(
    message_type: MessageType,
    message: &CapabilitiesInfoAcknowledge,
) -> Result<Vec<u8>> {
    validate_capabilities_info_ack(message)?;
    let mut out = vec![message_type as u8];
    out.extend_from_slice(&message.reserved);
    out.extend_from_slice(&message.home_address);
    out.extend_from_slice(&message.care_of_address);
    out.extend_from_slice(&message.identification.to_be_bytes());
    for nvse in &message.nvses {
        out.extend_from_slice(&nvse.encode()?);
    }
    out.extend_from_slice(&message.authentication_extension.encode()?);
    Ok(out)
}

fn decode_request(input: &[u8]) -> Result<RegistrationRequest> {
    if input.len() < 24 {
        return Err(Error::Truncated {
            needed: 24,
            actual: input.len(),
        });
    }
    let (session, used) = SessionSpecificExtension::decode(&input[24..])?;
    let message = RegistrationRequest {
        flags: input[1],
        lifetime: u16::from_be_bytes([input[2], input[3]]),
        home_address: [input[4], input[5], input[6], input[7]],
        home_agent: [input[8], input[9], input[10], input[11]],
        care_of_address: [input[12], input[13], input[14], input[15]],
        identification: u64::from_be_bytes([
            input[16], input[17], input[18], input[19], input[20], input[21], input[22], input[23],
        ]),
        session,
        extensions: collect_extensions(&input[24 + used..])?,
    };
    validate_request(&message)?;
    Ok(message)
}

fn decode_reply(input: &[u8]) -> Result<RegistrationReply> {
    if input.len() < 20 {
        return Err(Error::Truncated {
            needed: 20,
            actual: input.len(),
        });
    }
    let (session, used) = SessionSpecificExtension::decode(&input[20..])?;
    let message = RegistrationReply {
        code: input[1],
        lifetime: u16::from_be_bytes([input[2], input[3]]),
        home_address: [input[4], input[5], input[6], input[7]],
        home_agent: [input[8], input[9], input[10], input[11]],
        identification: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        session,
        extensions: collect_extensions(&input[20 + used..])?,
    };
    validate_reply(&message)?;
    Ok(message)
}

fn decode_update(input: &[u8]) -> Result<RegistrationUpdate> {
    if input.len() < 20 {
        return Err(Error::Truncated {
            needed: 20,
            actual: input.len(),
        });
    }
    let (session, used) = SessionSpecificExtension::decode(&input[20..])?;
    let (nvses, authentication_extension) = decode_nvse_sequence_with_auth(
        &input[20 + used..],
        AuthenticationExtensionType::RegistrationUpdate,
        "registration update",
    )?;
    let message = RegistrationUpdate {
        reserved: [input[1], input[2], input[3]],
        home_address: [input[4], input[5], input[6], input[7]],
        home_agent: [input[8], input[9], input[10], input[11]],
        identification: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        session,
        nvses,
        authentication_extension,
    };
    validate_update(&message)?;
    Ok(message)
}

fn decode_acknowledge(input: &[u8]) -> Result<RegistrationAcknowledge> {
    if input.len() < 20 {
        return Err(Error::Truncated {
            needed: 20,
            actual: input.len(),
        });
    }
    let (session, used) = SessionSpecificExtension::decode(&input[20..])?;
    let message = RegistrationAcknowledge {
        reserved: [input[1], input[2]],
        status: input[3],
        home_address: [input[4], input[5], input[6], input[7]],
        care_of_address: [input[8], input[9], input[10], input[11]],
        identification: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        session,
        authentication_extension: decode_required_authentication_extension(
            &input[20 + used..],
            AuthenticationExtensionType::RegistrationUpdate,
            "registration acknowledge",
        )?,
    };
    validate_acknowledge(&message)?;
    Ok(message)
}

fn decode_session_update(input: &[u8]) -> Result<SessionUpdate> {
    if input.len() < 20 {
        return Err(Error::Truncated {
            needed: 20,
            actual: input.len(),
        });
    }
    let (session, used) = SessionSpecificExtension::decode(&input[20..])?;
    let (nvses, authentication_extension) = decode_nvse_sequence_with_auth(
        &input[20 + used..],
        AuthenticationExtensionType::RegistrationUpdate,
        "session update",
    )?;
    let message = SessionUpdate {
        reserved: [input[1], input[2], input[3]],
        home_address: [input[4], input[5], input[6], input[7]],
        home_agent: [input[8], input[9], input[10], input[11]],
        identification: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        session,
        nvses,
        authentication_extension,
    };
    validate_session_update(&message)?;
    Ok(message)
}

fn decode_session_update_acknowledge(input: &[u8]) -> Result<SessionUpdateAcknowledge> {
    if input.len() < 20 {
        return Err(Error::Truncated {
            needed: 20,
            actual: input.len(),
        });
    }
    let (session, used) = SessionSpecificExtension::decode(&input[20..])?;
    let message = SessionUpdateAcknowledge {
        reserved: [input[1], input[2]],
        status: input[3],
        home_address: [input[4], input[5], input[6], input[7]],
        care_of_address: [input[8], input[9], input[10], input[11]],
        identification: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        session,
        authentication_extension: decode_required_authentication_extension(
            &input[20 + used..],
            AuthenticationExtensionType::RegistrationUpdate,
            "session update acknowledge",
        )?,
    };
    validate_session_update_acknowledge(&message)?;
    Ok(message)
}

fn decode_capabilities_info(input: &[u8]) -> Result<CapabilitiesInfo> {
    if input.len() < 24 {
        return Err(Error::Truncated {
            needed: 24,
            actual: input.len(),
        });
    }
    let (nvses, authentication_extension) = decode_nvse_sequence_with_auth(
        &input[24..],
        AuthenticationExtensionType::RegistrationUpdate,
        "capabilities info",
    )?;
    let message = CapabilitiesInfo {
        reserved: [input[1], input[2], input[3]],
        home_address: [input[4], input[5], input[6], input[7]],
        home_agent: [input[8], input[9], input[10], input[11]],
        care_of_address: [input[12], input[13], input[14], input[15]],
        identification: u64::from_be_bytes([
            input[16], input[17], input[18], input[19], input[20], input[21], input[22], input[23],
        ]),
        nvses,
        authentication_extension,
    };
    validate_capabilities_info(&message)?;
    Ok(message)
}

fn decode_capabilities_info_ack(input: &[u8]) -> Result<CapabilitiesInfoAcknowledge> {
    if input.len() < 20 {
        return Err(Error::Truncated {
            needed: 20,
            actual: input.len(),
        });
    }
    let (nvses, authentication_extension) = decode_nvse_sequence_with_auth(
        &input[20..],
        AuthenticationExtensionType::RegistrationUpdate,
        "capabilities info acknowledge",
    )?;
    let message = CapabilitiesInfoAcknowledge {
        reserved: [input[1], input[2], input[3]],
        home_address: [input[4], input[5], input[6], input[7]],
        care_of_address: [input[8], input[9], input[10], input[11]],
        identification: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        nvses,
        authentication_extension,
    };
    validate_capabilities_info_ack(&message)?;
    Ok(message)
}

fn collect_extensions(input: &[u8]) -> Result<Vec<Extension>> {
    let mut extensions = Vec::new();
    let mut raw_types = HashSet::new();
    let mut auth_types = HashSet::new();
    let mut nvse_keys = HashSet::new();
    let mut offset = 0usize;
    while offset < input.len() {
        if input.len() - offset < 2 {
            return Err(Error::Truncated {
                needed: offset + 2,
                actual: input.len(),
            });
        }
        let length = input[offset + 1] as usize;
        let end = offset + 2 + length;
        if input.len() < end {
            return Err(Error::Truncated {
                needed: end,
                actual: input.len(),
            });
        }
        let extension = Extension::decode(&input[offset..end])?;
        match &extension {
            Extension::SessionSpecific(_) => {
                return Err(Error::InvalidValue {
                    context: "extensions",
                    reason: "duplicate Session Specific Extension must not appear after the base session field",
                });
            }
            Extension::Authentication(authentication) => {
                let key = authentication.extension_type as u8;
                if !auth_types.insert(key) {
                    return Err(Error::DuplicateExtension {
                        extension_type: key,
                    });
                }
            }
            Extension::Nvse(nvse) => {
                let key = nvse.application_key();
                if !nvse_keys.insert(key) {
                    return Err(Error::DuplicateExtension {
                        extension_type: Nvse::TYPE,
                    });
                }
            }
            Extension::Raw(raw) => {
                if !raw_types.insert(raw.extension_type) {
                    return Err(Error::DuplicateExtension {
                        extension_type: raw.extension_type,
                    });
                }
            }
        }
        extensions.push(extension);
        offset = end;
    }
    Ok(extensions)
}

fn decode_nvse_sequence_with_auth(
    input: &[u8],
    expected_type: AuthenticationExtensionType,
    context: &'static str,
) -> Result<(Vec<Nvse>, AuthenticationExtension)> {
    if input.is_empty() {
        return Err(Error::Truncated {
            needed: 2,
            actual: 0,
        });
    }
    let mut nvses = Vec::new();
    let mut keys = HashSet::new();
    let mut offset = 0usize;
    while offset < input.len() {
        if input.len() - offset < 2 {
            return Err(Error::Truncated {
                needed: offset + 2,
                actual: input.len(),
            });
        }
        let extension_type = input[offset];
        let length = input[offset + 1] as usize;
        let end = offset + 2 + length;
        if input.len() < end {
            return Err(Error::Truncated {
                needed: end,
                actual: input.len(),
            });
        }
        if extension_type == expected_type as u8 {
            if end != input.len() {
                return Err(Error::InvalidValue {
                    context,
                    reason: "authentication extension must terminate the message",
                });
            }
            return Ok((
                nvses,
                decode_required_authentication_extension(
                    &input[offset..end],
                    expected_type,
                    context,
                )?,
            ));
        }
        if extension_type != Nvse::TYPE {
            return Err(Error::InvalidValue {
                context,
                reason: "only NVSEs may appear before the required authentication extension",
            });
        }
        let nvse = Nvse::decode(&input[offset..end])?;
        let key = nvse.application_key();
        if !keys.insert(key) {
            return Err(Error::DuplicateExtension {
                extension_type: Nvse::TYPE,
            });
        }
        nvses.push(nvse);
        offset = end;
    }
    Err(Error::InvalidValue {
        context,
        reason: "required authentication extension is missing",
    })
}

pub(crate) fn validate_request(message: &RegistrationRequest) -> Result<()> {
    validate_session(&message.session)?;
    if !matches!(message.flags, 0x0a | 0x8a) {
        return Err(Error::InvalidValue {
            context: "registration request.flags",
            reason: "flags must encode one of the A.S0017 request flag patterns",
        });
    }
    if message.lifetime == u16::MAX {
        return Err(Error::InvalidValue {
            context: "registration request.lifetime",
            reason: "lifetime must not be 0xffff",
        });
    }
    validate_home_address_zero(message.home_address, "registration request.home_address")?;
    validate_extensions(&message.extensions, MessageType::RegistrationRequest)?;
    require_authentication_extension(
        &message.extensions,
        AuthenticationExtensionType::MobileHome,
        "registration request.extensions",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_reply(message: &RegistrationReply) -> Result<()> {
    validate_session(&message.session)?;
    validate_registration_reply_code(message.code)?;
    if message.lifetime == u16::MAX {
        return Err(Error::InvalidValue {
            context: "registration reply.lifetime",
            reason: "lifetime must not be 0xffff",
        });
    }
    validate_home_address_zero(message.home_address, "registration reply.home_address")?;
    validate_extensions(&message.extensions, MessageType::RegistrationReply)?;
    require_authentication_extension(
        &message.extensions,
        AuthenticationExtensionType::MobileHome,
        "registration reply.extensions",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_update(message: &RegistrationUpdate) -> Result<()> {
    validate_session(&message.session)?;
    validate_reserved_3(
        message.reserved,
        "registration update.reserved",
        "reserved octets must be zero",
    )?;
    validate_home_address_zero(message.home_address, "registration update.home_address")?;
    validate_nvse_list(&message.nvses, MessageType::RegistrationUpdate)?;
    validate_authentication_extension(
        &message.authentication_extension,
        AuthenticationExtensionType::RegistrationUpdate,
        "registration update.authentication_extension",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_acknowledge(message: &RegistrationAcknowledge) -> Result<()> {
    validate_session(&message.session)?;
    validate_reserved_2(
        message.reserved,
        "registration acknowledge.reserved",
        "reserved octets must be zero",
    )?;
    validate_update_status(message.status, false, "registration acknowledge.status")?;
    validate_home_address_zero(
        message.home_address,
        "registration acknowledge.home_address",
    )?;
    validate_authentication_extension(
        &message.authentication_extension,
        AuthenticationExtensionType::RegistrationUpdate,
        "registration acknowledge.authentication_extension",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_session_update(message: &SessionUpdate) -> Result<()> {
    validate_session(&message.session)?;
    validate_reserved_3(
        message.reserved,
        "session update.reserved",
        "reserved octets must be zero",
    )?;
    validate_home_address_zero(message.home_address, "session update.home_address")?;
    validate_nvse_list(&message.nvses, MessageType::SessionUpdate)?;
    if !message.nvses.iter().any(|nvse| {
        matches!(
            nvse,
            Nvse::AnchorPPAddress(_) | Nvse::SessionParameter(_) | Nvse::Unknown(_)
        )
    }) {
        return Err(Error::InvalidValue {
            context: "session update.nvses",
            reason: "session update must carry session-parameter or anchor-P-P NVSE content",
        });
    }
    validate_authentication_extension(
        &message.authentication_extension,
        AuthenticationExtensionType::RegistrationUpdate,
        "session update.authentication_extension",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_session_update_acknowledge(
    message: &SessionUpdateAcknowledge,
) -> Result<()> {
    validate_session(&message.session)?;
    validate_reserved_2(
        message.reserved,
        "session update acknowledge.reserved",
        "reserved octets must be zero",
    )?;
    validate_update_status(message.status, true, "session update acknowledge.status")?;
    validate_home_address_zero(
        message.home_address,
        "session update acknowledge.home_address",
    )?;
    validate_authentication_extension(
        &message.authentication_extension,
        AuthenticationExtensionType::RegistrationUpdate,
        "session update acknowledge.authentication_extension",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_capabilities_info(message: &CapabilitiesInfo) -> Result<()> {
    validate_reserved_3(
        message.reserved,
        "capabilities info.reserved",
        "reserved octets must be zero",
    )?;
    validate_home_address_zero(message.home_address, "capabilities info.home_address")?;
    validate_nvse_list(&message.nvses, MessageType::CapabilitiesInfo)?;
    if !message.nvses.iter().any(|nvse| {
        matches!(
            nvse,
            Nvse::PdsnEnabledFeature(_) | Nvse::PcfEnabledFeature(_) | Nvse::Unknown(_)
        )
    }) {
        return Err(Error::InvalidValue {
            context: "capabilities info.nvses",
            reason: "capabilities info must include feature NVSE content",
        });
    }
    validate_authentication_extension(
        &message.authentication_extension,
        AuthenticationExtensionType::RegistrationUpdate,
        "capabilities info.authentication_extension",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

pub(crate) fn validate_capabilities_info_ack(message: &CapabilitiesInfoAcknowledge) -> Result<()> {
    validate_reserved_3(
        message.reserved,
        "capabilities info acknowledge.reserved",
        "reserved octets must be zero",
    )?;
    validate_home_address_zero(
        message.home_address,
        "capabilities info acknowledge.home_address",
    )?;
    validate_nvse_list(&message.nvses, MessageType::CapabilitiesInfoAcknowledge)?;
    if !message.nvses.iter().any(|nvse| {
        matches!(
            nvse,
            Nvse::PdsnEnabledFeature(_) | Nvse::PcfEnabledFeature(_) | Nvse::Unknown(_)
        )
    }) {
        return Err(Error::InvalidValue {
            context: "capabilities info acknowledge.nvses",
            reason: "capabilities info acknowledge must include feature NVSE content",
        });
    }
    validate_authentication_extension(
        &message.authentication_extension,
        AuthenticationExtensionType::RegistrationUpdate,
        "capabilities info acknowledge.authentication_extension",
    )?;
    validate_identification_non_zero(message.identification)?;
    Ok(())
}

fn validate_extensions(extensions: &[Extension], message_type: MessageType) -> Result<()> {
    let mut raw_types = HashSet::new();
    let mut auth_types = HashSet::new();
    let mut nvse_keys = HashSet::new();
    for extension in extensions {
        match extension {
            Extension::SessionSpecific(_) => {
                return Err(Error::InvalidValue {
                    context: "extensions",
                    reason: "session specific extension is carried in the mandatory session field",
                });
            }
            Extension::Authentication(authentication) => {
                let extension_type = authentication.extension_type as u8;
                if !auth_types.insert(extension_type) {
                    return Err(Error::DuplicateExtension { extension_type });
                }
            }
            Extension::Nvse(nvse) => {
                let key = nvse.application_key();
                if !nvse_keys.insert(key) {
                    return Err(Error::DuplicateExtension {
                        extension_type: Nvse::TYPE,
                    });
                }
                validate_nvse_for_message(nvse, message_type)?;
            }
            Extension::Raw(raw) => {
                if !raw_types.insert(raw.extension_type) {
                    return Err(Error::DuplicateExtension {
                        extension_type: raw.extension_type,
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_nvse_list(nvses: &[Nvse], message_type: MessageType) -> Result<()> {
    let mut keys = HashSet::new();
    for nvse in nvses {
        let key = nvse.application_key();
        if !keys.insert(key) {
            return Err(Error::DuplicateExtension {
                extension_type: Nvse::TYPE,
            });
        }
        validate_nvse_for_message(nvse, message_type)?;
    }
    Ok(())
}

fn validate_session(session: &SessionSpecificExtension) -> Result<()> {
    if session.protocol_type != PROTOCOL_TYPE_UNSTRUCTURED_BYTE_STREAM {
        return Err(Error::InvalidValue {
            context: "session.protocol_type",
            reason: "A.S0017 A11 requires protocol type 0x8881",
        });
    }
    if session.pcf_session_id == 0 {
        return Err(Error::InvalidValue {
            context: "session.pcf_session_id",
            reason: "pcf_session_id must be non-zero",
        });
    }
    if session.session_id_version > 1 {
        return Err(Error::InvalidValue {
            context: "session.session_id_version",
            reason: "session ID version must be 0 or 1",
        });
    }
    if !(1..=6).contains(&session.mn_session_reference_id) {
        return Err(Error::InvalidValue {
            context: "session.mn_session_reference_id",
            reason: "MN Session Reference ID must be in the range 1..=6",
        });
    }
    if session.mn_id_type != SESSION_SPECIFIC_MSID_TYPE_IMSI {
        return Err(Error::InvalidValue {
            context: "session.mn_id_type",
            reason: "MSID type must be IMSI (0x0006)",
        });
    }
    if !(6..=8).contains(&session.mn_id.len()) {
        return Err(Error::InvalidExtensionLength {
            expected_min: 6,
            actual: session.mn_id.len(),
        });
    }
    validate_session_msid_bcd(&session.mn_id)?;
    Ok(())
}

fn validate_session_msid_bcd(msid: &[u8]) -> Result<()> {
    let first = msid[0];
    let first_digit = first >> 4;
    let odd_even = first & 0x0f;
    if first_digit > 9 {
        return Err(Error::InvalidValue {
            context: "session.mn_id.first_digit",
            reason: "first IMSI digit must be BCD encoded",
        });
    }
    if !matches!(odd_even, 0x00 | 0x01) {
        return Err(Error::InvalidValue {
            context: "session.mn_id.odd_even",
            reason: "odd/even indicator must be 0 or 1",
        });
    }
    let even_digits = odd_even == 0;
    for (index, byte) in msid.iter().copied().enumerate().skip(1) {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low > 9 {
            return Err(Error::InvalidValue {
                context: "session.mn_id.low_digit",
                reason: "IMSI low digit must be BCD encoded",
            });
        }
        let is_last = index == msid.len() - 1;
        if high == 0x0f {
            if even_digits && is_last {
                continue;
            }
            return Err(Error::InvalidValue {
                context: "session.mn_id.filler",
                reason: "IMSI filler nibble is only valid for the final even-digit octet",
            });
        }
        if high > 9 {
            return Err(Error::InvalidValue {
                context: "session.mn_id.high_digit",
                reason: "IMSI high digit must be BCD encoded",
            });
        }
    }
    Ok(())
}

fn decode_required_authentication_extension(
    input: &[u8],
    expected_type: AuthenticationExtensionType,
    context: &'static str,
) -> Result<AuthenticationExtension> {
    if input.is_empty() {
        return Err(Error::Truncated {
            needed: 2,
            actual: 0,
        });
    }
    let extension = Extension::decode(input)?;
    match extension {
        Extension::Authentication(extension) => {
            validate_authentication_extension(&extension, expected_type, context)?;
            Ok(extension)
        }
        _ => Err(Error::InvalidValue {
            context,
            reason: "message must end with the required authentication extension",
        }),
    }
}

fn validate_authentication_extension(
    extension: &AuthenticationExtension,
    expected_type: AuthenticationExtensionType,
    context: &'static str,
) -> Result<()> {
    if extension.extension_type != expected_type {
        return Err(Error::InvalidValue {
            context,
            reason: "unexpected authentication extension type",
        });
    }
    Ok(())
}

fn require_authentication_extension(
    extensions: &[Extension],
    expected_type: AuthenticationExtensionType,
    context: &'static str,
) -> Result<()> {
    required_extension_authentication(extensions, expected_type, context).map(|_| ())
}

fn required_extension_authentication<'a>(
    extensions: &'a [Extension],
    expected_type: AuthenticationExtensionType,
    context: &'static str,
) -> Result<&'a AuthenticationExtension> {
    let Some(last) = extensions.last() else {
        return Err(Error::InvalidValue {
            context,
            reason: "required authentication extension is missing",
        });
    };
    match last {
        Extension::Authentication(authentication)
            if authentication.extension_type == expected_type =>
        {
            Ok(authentication)
        }
        Extension::Authentication(_) => Err(Error::InvalidValue {
            context,
            reason: "unexpected authentication extension type",
        }),
        _ => Err(Error::InvalidValue {
            context,
            reason: "required authentication extension is missing",
        }),
    }
}

fn validate_registration_reply_code(code: u8) -> Result<()> {
    if matches!(
        code,
        0x00 | 0x80 | 0x81 | 0x82 | 0x83 | 0x85 | 0x86 | 0x88 | 0x89 | 0x8a | 0x8d
    ) {
        Ok(())
    } else {
        Err(Error::InvalidValue {
            context: "registration reply.code",
            reason: "unsupported A.S0017 registration reply code",
        })
    }
}

fn validate_update_status(
    status: u8,
    allow_session_parameters_not_updated: bool,
    context: &'static str,
) -> Result<()> {
    let valid = if allow_session_parameters_not_updated {
        matches!(status, 0x00 | 0x80 | 0x83 | 0x85 | 0x86 | 0xc9)
    } else {
        matches!(status, 0x00 | 0x80 | 0x83 | 0x85 | 0x86)
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidValue {
            context,
            reason: "unsupported A.S0017 update status value",
        })
    }
}

fn validate_nvse_for_message(nvse: &Nvse, message_type: MessageType) -> Result<()> {
    match message_type {
        MessageType::RegistrationRequest => match nvse {
            Nvse::AccessNetworkIdentifiers(_)
            | Nvse::AnchorPPAddress(_)
            | Nvse::AllDormantIndicator
            | Nvse::ServiceOption(_)
            | Nvse::PcfEnabledFeature(_)
            | Nvse::Unknown(_) => Ok(()),
            _ => invalid_nvse(message_type),
        },
        MessageType::RegistrationReply => match nvse {
            Nvse::AnchorPPAddress(_)
            | Nvse::SessionParameter(_)
            | Nvse::PdsnEnabledFeature(_)
            | Nvse::Unknown(_) => Ok(()),
            _ => invalid_nvse(message_type),
        },
        MessageType::RegistrationUpdate => match nvse {
            Nvse::PdsnCode(_) | Nvse::Unknown(_) => Ok(()),
            _ => invalid_nvse(message_type),
        },
        MessageType::SessionUpdate => match nvse {
            Nvse::AnchorPPAddress(_) | Nvse::SessionParameter(_) | Nvse::Unknown(_) => Ok(()),
            _ => invalid_nvse(message_type),
        },
        MessageType::CapabilitiesInfo => match nvse {
            Nvse::PdsnEnabledFeature(_)
            | Nvse::PcfEnabledFeature(_)
            | Nvse::PdsnCode(_)
            | Nvse::Unknown(_) => Ok(()),
            _ => invalid_nvse(message_type),
        },
        MessageType::CapabilitiesInfoAcknowledge => match nvse {
            Nvse::PdsnEnabledFeature(_) | Nvse::PcfEnabledFeature(_) | Nvse::Unknown(_) => Ok(()),
            _ => invalid_nvse(message_type),
        },
        MessageType::RegistrationAcknowledge | MessageType::SessionUpdateAcknowledge => {
            invalid_nvse(message_type)
        }
    }
}

fn invalid_nvse<T>(message_type: MessageType) -> Result<T> {
    Err(Error::InvalidValue {
        context: "nvse",
        reason: match message_type {
            MessageType::RegistrationRequest => {
                "NVSE content is not valid for A11-Registration Request"
            }
            MessageType::RegistrationReply => {
                "NVSE content is not valid for A11-Registration Reply"
            }
            MessageType::RegistrationUpdate => {
                "NVSE content is not valid for A11-Registration Update"
            }
            MessageType::RegistrationAcknowledge => {
                "NVSE content is not valid for A11-Registration Acknowledge"
            }
            MessageType::SessionUpdate => "NVSE content is not valid for A11-Session Update",
            MessageType::SessionUpdateAcknowledge => {
                "NVSE content is not valid for A11-Session Update Acknowledge"
            }
            MessageType::CapabilitiesInfo => "NVSE content is not valid for A11-Capabilities Info",
            MessageType::CapabilitiesInfoAcknowledge => {
                "NVSE content is not valid for A11-Capabilities Info Ack"
            }
        },
    })
}

fn validate_identification_non_zero(identification: u64) -> Result<()> {
    if identification == 0 {
        Err(Error::InvalidValue {
            context: "identification",
            reason: "identification must be non-zero",
        })
    } else {
        Ok(())
    }
}

fn validate_home_address_zero(address: [u8; 4], context: &'static str) -> Result<()> {
    if address != [0, 0, 0, 0] {
        return Err(Error::InvalidValue {
            context,
            reason: "home address must be 0.0.0.0 on the A11 interface",
        });
    }
    Ok(())
}

fn validate_reserved_2(
    reserved: [u8; 2],
    context: &'static str,
    reason: &'static str,
) -> Result<()> {
    if reserved != [0; 2] {
        Err(Error::InvalidValue { context, reason })
    } else {
        Ok(())
    }
}

fn validate_reserved_3(
    reserved: [u8; 3],
    context: &'static str,
    reason: &'static str,
) -> Result<()> {
    if reserved != [0; 3] {
        Err(Error::InvalidValue { context, reason })
    } else {
        Ok(())
    }
}
