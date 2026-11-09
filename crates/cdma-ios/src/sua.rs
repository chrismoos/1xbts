//! SUA (RFC 3868) for A1p (A.S0012-D §3.2.2.6). A parameter's length excludes
//! its padding to four octets.

use std::fmt;

/// SUA version 1 (RFC 3868 §3.1).
pub const SUA_VERSION: u8 = 1;
/// SCTP payload protocol identifier for SUA (RFC 3868 §7.1).
pub const SUA_PAYLOAD_PROTOCOL_ID: u32 = 4;
/// SCTP port for SUA on A1p (A.S0012-D §3.2.2.8).
pub const A1P_SCTP_PORT: u16 = 14001;
/// BSAP subsystem number (A.S0012-D §3.1.3.3.4).
pub const BSAP_SSN: u8 = 252;

const HEADER_LEN: usize = 8;
const PARAMETER_HEADER_LEN: usize = 4;
const PARAMETER_ALIGNMENT: usize = 4;

/// Message class and type (RFC 3868 §3.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuaMessageKind {
    Error,
    Notify,
    AspUp,
    AspDown,
    Heartbeat,
    AspUpAck,
    AspDownAck,
    HeartbeatAck,
    AspActive,
    AspInactive,
    AspActiveAck,
    AspInactiveAck,
    /// CLDT: connectionless data transfer.
    ConnectionlessData,
    /// CLDR: connectionless data response.
    ConnectionlessDataResponse,
    /// CORE: connection request.
    ConnectionRequest,
    /// COAK: connection acknowledge.
    ConnectionAck,
    /// COREF: connection refused.
    ConnectionRefused,
    /// RELRE: release request.
    ReleaseRequest,
    /// RELCO: release complete.
    ReleaseComplete,
    /// CODT: connection-oriented data transfer.
    ConnectionData,
    /// COERR: connection-oriented error.
    ConnectionError,
    /// COIT: inactivity test.
    InactivityTest,
}

const CLASS_MGMT: u8 = 0;
const CLASS_ASPSM: u8 = 3;
const CLASS_ASPTM: u8 = 4;
const CLASS_CL: u8 = 7;
const CLASS_CO: u8 = 8;

impl SuaMessageKind {
    fn class_and_type(self) -> (u8, u8) {
        match self {
            Self::Error => (CLASS_MGMT, 0),
            Self::Notify => (CLASS_MGMT, 1),
            Self::AspUp => (CLASS_ASPSM, 1),
            Self::AspDown => (CLASS_ASPSM, 2),
            Self::Heartbeat => (CLASS_ASPSM, 3),
            Self::AspUpAck => (CLASS_ASPSM, 4),
            Self::AspDownAck => (CLASS_ASPSM, 5),
            Self::HeartbeatAck => (CLASS_ASPSM, 6),
            Self::AspActive => (CLASS_ASPTM, 1),
            Self::AspInactive => (CLASS_ASPTM, 2),
            Self::AspActiveAck => (CLASS_ASPTM, 3),
            Self::AspInactiveAck => (CLASS_ASPTM, 4),
            Self::ConnectionlessData => (CLASS_CL, 1),
            Self::ConnectionlessDataResponse => (CLASS_CL, 2),
            Self::ConnectionRequest => (CLASS_CO, 1),
            Self::ConnectionAck => (CLASS_CO, 2),
            Self::ConnectionRefused => (CLASS_CO, 3),
            Self::ReleaseRequest => (CLASS_CO, 4),
            Self::ReleaseComplete => (CLASS_CO, 5),
            Self::ConnectionData => (CLASS_CO, 8),
            Self::ConnectionError => (CLASS_CO, 10),
            Self::InactivityTest => (CLASS_CO, 11),
        }
    }

    fn from_class_and_type(class: u8, message_type: u8) -> Option<Self> {
        Some(match (class, message_type) {
            (CLASS_MGMT, 0) => Self::Error,
            (CLASS_MGMT, 1) => Self::Notify,
            (CLASS_ASPSM, 1) => Self::AspUp,
            (CLASS_ASPSM, 2) => Self::AspDown,
            (CLASS_ASPSM, 3) => Self::Heartbeat,
            (CLASS_ASPSM, 4) => Self::AspUpAck,
            (CLASS_ASPSM, 5) => Self::AspDownAck,
            (CLASS_ASPSM, 6) => Self::HeartbeatAck,
            (CLASS_ASPTM, 1) => Self::AspActive,
            (CLASS_ASPTM, 2) => Self::AspInactive,
            (CLASS_ASPTM, 3) => Self::AspActiveAck,
            (CLASS_ASPTM, 4) => Self::AspInactiveAck,
            (CLASS_CL, 1) => Self::ConnectionlessData,
            (CLASS_CL, 2) => Self::ConnectionlessDataResponse,
            (CLASS_CO, 1) => Self::ConnectionRequest,
            (CLASS_CO, 2) => Self::ConnectionAck,
            (CLASS_CO, 3) => Self::ConnectionRefused,
            (CLASS_CO, 4) => Self::ReleaseRequest,
            (CLASS_CO, 5) => Self::ReleaseComplete,
            (CLASS_CO, 8) => Self::ConnectionData,
            (CLASS_CO, 10) => Self::ConnectionError,
            (CLASS_CO, 11) => Self::InactivityTest,
            _ => return None,
        })
    }

    /// Management messages use stream 0 (RFC 3868 §4.1.1).
    pub fn is_data(self) -> bool {
        matches!(self.class_and_type().0, CLASS_CL | CLASS_CO)
    }
}

/// Parameter tags (RFC 3868 §3.9, §3.10).
pub mod tag {
    pub const INFO_STRING: u16 = 0x0004;
    pub const ROUTING_CONTEXT: u16 = 0x0006;
    pub const HEARTBEAT_DATA: u16 = 0x0009;
    pub const TRAFFIC_MODE_TYPE: u16 = 0x000b;
    pub const ERROR_CODE: u16 = 0x000c;
    pub const ASP_IDENTIFIER: u16 = 0x0011;
    pub const SOURCE_ADDRESS: u16 = 0x0102;
    pub const DESTINATION_ADDRESS: u16 = 0x0103;
    pub const SOURCE_REFERENCE_NUMBER: u16 = 0x0104;
    pub const DESTINATION_REFERENCE_NUMBER: u16 = 0x0105;
    pub const SCCP_CAUSE: u16 = 0x0106;
    pub const DATA: u16 = 0x010b;
    pub const PROTOCOL_CLASS: u16 = 0x0115;
    pub const SEQUENCE_CONTROL: u16 = 0x0116;
    pub const POINT_CODE: u16 = 0x8002;
    pub const SUBSYSTEM_NUMBER: u16 = 0x8003;
}

/// RFC 3868 §3.10.2.1 routing indicator: route on SSN + PC.
const ROUTE_ON_SSN_PC: u16 = 2;
/// RFC 3868 §3.10.2.2 address indicator bits.
const ADDRESS_INDICATOR_SSN: u16 = 0b001;
const ADDRESS_INDICATOR_PC: u16 = 0b010;

/// RFC 3868 §3.10.21 protocol classes A1p uses.
pub const PROTOCOL_CLASS_0: u32 = 0;
pub const PROTOCOL_CLASS_2: u32 = 2;

/// RFC 3868 §3.8.2 traffic mode: override.
pub const TRAFFIC_MODE_OVERRIDE: u32 = 1;

/// RFC 3868 §3.10.6 cause types.
pub const CAUSE_TYPE_REFUSAL: u8 = 0x2;
pub const CAUSE_TYPE_RELEASE: u8 = 0x3;
/// Q.713 §3.11 release cause "end user originated".
pub const RELEASE_CAUSE_END_USER_ORIGINATED: u8 = 0x00;
/// Q.713 §3.15 refusal cause "end user originated".
pub const REFUSAL_CAUSE_END_USER_ORIGINATED: u8 = 0x00;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuaAddress {
    pub point_code: u32,
    pub ssn: u8,
}

impl SuaAddress {
    fn encode(self) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(&ROUTE_ON_SSN_PC.to_be_bytes());
        value.extend_from_slice(&(ADDRESS_INDICATOR_SSN | ADDRESS_INDICATOR_PC).to_be_bytes());
        push_parameter(&mut value, tag::POINT_CODE, &self.point_code.to_be_bytes());
        push_parameter(
            &mut value,
            tag::SUBSYSTEM_NUMBER,
            &u32::from(self.ssn).to_be_bytes(),
        );
        value
    }

    fn decode(value: &[u8]) -> Result<Self, SuaError> {
        if value.len() < 4 {
            return Err(SuaError::Truncated("address"));
        }
        let mut point_code = None;
        let mut ssn = None;
        for (sub_tag, sub_value) in Parameters::new(&value[4..]) {
            match sub_tag {
                tag::POINT_CODE => point_code = Some(read_u32(sub_value)?),
                tag::SUBSYSTEM_NUMBER => ssn = Some((read_u32(sub_value)? & 0xff) as u8),
                _ => {}
            }
        }
        Ok(Self {
            point_code: point_code.ok_or(SuaError::Missing("point code"))?,
            ssn: ssn.ok_or(SuaError::Missing("subsystem number"))?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuaError {
    Truncated(&'static str),
    BadVersion(u8),
    UnknownMessage { class: u8, message_type: u8 },
    Missing(&'static str),
}

impl fmt::Display for SuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "SUA {what} truncated"),
            Self::BadVersion(version) => write!(f, "SUA version {version} is not 1"),
            Self::UnknownMessage {
                class,
                message_type,
            } => write!(f, "unknown SUA message class {class} type {message_type}"),
            Self::Missing(what) => write!(f, "SUA message lacks {what}"),
        }
    }
}

impl std::error::Error for SuaError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuaMessage {
    pub kind: SuaMessageKind,
    pub parameters: Vec<(u16, Vec<u8>)>,
}

impl SuaMessage {
    pub fn new(kind: SuaMessageKind) -> Self {
        Self {
            kind,
            parameters: Vec::new(),
        }
    }

    pub fn with(mut self, tag: u16, value: impl Into<Vec<u8>>) -> Self {
        self.parameters.push((tag, value.into()));
        self
    }

    pub fn with_u32(self, tag: u16, value: u32) -> Self {
        self.with(tag, value.to_be_bytes())
    }

    pub fn with_address(self, tag: u16, address: SuaAddress) -> Self {
        self.with(tag, address.encode())
    }

    pub fn with_cause(self, cause_type: u8, cause_value: u8) -> Self {
        self.with(tag::SCCP_CAUSE, [0, 0, cause_type, cause_value])
    }

    pub fn parameter(&self, tag: u16) -> Option<&[u8]> {
        self.parameters
            .iter()
            .find(|(candidate, _)| *candidate == tag)
            .map(|(_, value)| value.as_slice())
    }

    pub fn u32_parameter(&self, tag: u16) -> Option<u32> {
        self.parameter(tag).and_then(|value| read_u32(value).ok())
    }

    pub fn address(&self, tag: u16) -> Option<SuaAddress> {
        self.parameter(tag)
            .and_then(|value| SuaAddress::decode(value).ok())
    }

    pub fn data(&self) -> Option<&[u8]> {
        self.parameter(tag::DATA)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for (tag, value) in &self.parameters {
            push_parameter(&mut body, *tag, value);
        }
        let (class, message_type) = self.kind.class_and_type();
        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.push(SUA_VERSION);
        out.push(0);
        out.push(class);
        out.push(message_type);
        out.extend_from_slice(&((HEADER_LEN + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    pub fn decode(input: &[u8]) -> Result<Self, SuaError> {
        if input.len() < HEADER_LEN {
            return Err(SuaError::Truncated("header"));
        }
        if input[0] != SUA_VERSION {
            return Err(SuaError::BadVersion(input[0]));
        }
        let kind = SuaMessageKind::from_class_and_type(input[2], input[3]).ok_or(
            SuaError::UnknownMessage {
                class: input[2],
                message_type: input[3],
            },
        )?;
        let length = read_u32(&input[4..8])? as usize;
        if length < HEADER_LEN || input.len() < length {
            return Err(SuaError::Truncated("message"));
        }
        let parameters = Parameters::new(&input[HEADER_LEN..length])
            .map(|(tag, value)| (tag, value.to_vec()))
            .collect();
        Ok(Self { kind, parameters })
    }
}

fn push_parameter(out: &mut Vec<u8>, tag: u16, value: &[u8]) {
    let length = PARAMETER_HEADER_LEN + value.len();
    out.extend_from_slice(&tag.to_be_bytes());
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.extend_from_slice(value);
    out.resize(out.len() + padding(length), 0);
}

fn padding(length: usize) -> usize {
    (PARAMETER_ALIGNMENT - length % PARAMETER_ALIGNMENT) % PARAMETER_ALIGNMENT
}

fn read_u32(value: &[u8]) -> Result<u32, SuaError> {
    let bytes: [u8; 4] = value
        .get(value.len().saturating_sub(4)..)
        .and_then(|tail| tail.try_into().ok())
        .ok_or(SuaError::Truncated("32-bit parameter"))?;
    Ok(u32::from_be_bytes(bytes))
}

/// Iterates over TLV parameters, stopping at the first truncated one.
struct Parameters<'a> {
    rest: &'a [u8],
}

impl<'a> Parameters<'a> {
    fn new(rest: &'a [u8]) -> Self {
        Self { rest }
    }
}

impl<'a> Iterator for Parameters<'a> {
    type Item = (u16, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.len() < PARAMETER_HEADER_LEN {
            return None;
        }
        let tag = u16::from_be_bytes([self.rest[0], self.rest[1]]);
        let length = usize::from(u16::from_be_bytes([self.rest[2], self.rest[3]]));
        if length < PARAMETER_HEADER_LEN || self.rest.len() < length {
            self.rest = &[];
            return None;
        }
        let value = &self.rest[PARAMETER_HEADER_LEN..length];
        let consumed = (length + padding(length)).min(self.rest.len());
        self.rest = &self.rest[consumed..];
        Some((tag, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cldt_matches_rfc3868_layout() {
        let message = SuaMessage::new(SuaMessageKind::ConnectionlessData)
            .with_u32(tag::ROUTING_CONTEXT, 1)
            .with_u32(tag::PROTOCOL_CLASS, PROTOCOL_CLASS_0)
            .with_address(
                tag::SOURCE_ADDRESS,
                SuaAddress {
                    point_code: 0x0102,
                    ssn: BSAP_SSN,
                },
            )
            .with_u32(tag::SEQUENCE_CONTROL, 0)
            .with(tag::DATA, [0x00, 0x01, 0x52]);
        let wire = message.encode();
        assert_eq!(&wire[..8], &[1, 0, 7, 1, 0, 0, 0, wire.len() as u8]);
        assert_eq!(&wire[8..16], &[0x00, 0x06, 0x00, 0x08, 0, 0, 0, 1]);
        // Source address: RI=2, AI=SSN|PC, then PC and SSN sub-parameters.
        let source = &wire[24..48];
        assert_eq!(
            source,
            &[
                0x01, 0x02, 0x00, 0x18, 0x00, 0x02, 0x00, 0x03, 0x80, 0x02, 0x00, 0x08, 0, 0, 0x01,
                0x02, 0x80, 0x03, 0x00, 0x08, 0, 0, 0, 252,
            ]
        );
        // Data parameter length 7, padded to 8.
        assert_eq!(
            &wire[wire.len() - 8..],
            &[0x01, 0x0b, 0x00, 0x07, 0x00, 0x01, 0x52, 0]
        );
        assert_eq!(wire.len() % 4, 0);
        let decoded = SuaMessage::decode(&wire).unwrap();
        assert_eq!(decoded, message);
        assert_eq!(
            decoded.address(tag::SOURCE_ADDRESS),
            Some(SuaAddress {
                point_code: 0x0102,
                ssn: BSAP_SSN
            })
        );
        assert_eq!(decoded.data(), Some(&[0x00, 0x01, 0x52][..]));
    }

    #[test]
    fn aspsm_and_asptm_use_their_classes() {
        let up = SuaMessage::new(SuaMessageKind::AspUp).encode();
        assert_eq!(&up[..8], &[1, 0, 3, 1, 0, 0, 0, 8]);
        let active = SuaMessage::new(SuaMessageKind::AspActiveAck)
            .with_u32(tag::ROUTING_CONTEXT, 7)
            .encode();
        assert_eq!(&active[2..4], &[4, 3]);
        assert_eq!(
            SuaMessage::decode(&active)
                .unwrap()
                .u32_parameter(tag::ROUTING_CONTEXT),
            Some(7)
        );
        assert!(!SuaMessageKind::AspUp.is_data());
        assert!(SuaMessageKind::ConnectionData.is_data());
    }

    #[test]
    fn decode_rejects_bad_version_and_unknown_message() {
        assert_eq!(
            SuaMessage::decode(&[2, 0, 7, 1, 0, 0, 0, 8]),
            Err(SuaError::BadVersion(2))
        );
        assert!(matches!(
            SuaMessage::decode(&[1, 0, 8, 99, 0, 0, 0, 8]),
            Err(SuaError::UnknownMessage { .. })
        ));
        assert_eq!(
            SuaMessage::decode(&[1, 0, 8, 1, 0, 0, 0, 12]),
            Err(SuaError::Truncated("message"))
        );
    }
}
