//! A9 signaling codec primitives between the BSC and PCF.

use std::fmt::{Display, Formatter};

pub mod session;
pub mod transport;
pub mod typed;

/// Errors returned by the A9 codec helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyMessage,
    UnknownMessageType(u8),
    UnknownInformationElement(u8),
    Truncated {
        needed: usize,
        actual: usize,
    },
    InvalidLength {
        expected: usize,
        actual: usize,
    },
    DuplicateInformationElement(u8),
    MissingRequiredInformationElement(u8),
    UnexpectedInformationElement {
        message_type: MessageType,
        element_id: u8,
    },
    OutOfOrderInformationElement {
        message_type: MessageType,
        element_id: u8,
    },
    InvalidValue {
        context: &'static str,
        value: u32,
    },
    PayloadMessageTypeMismatch {
        header: MessageType,
        payload: MessageType,
    },
    DuplicateSession,
    DuplicateTrafficId(u32),
    UnknownSession,
    UnknownTrafficId(u32),
    TrafficIdMismatch {
        expected: u32,
        actual: u32,
    },
    CauseMismatch {
        expected: u8,
        actual: u8,
    },
    InvalidProcedureDirection {
        message_type: MessageType,
        state: &'static str,
    },
    InvalidProcedureState {
        message_type: MessageType,
        state: &'static str,
    },
}

/// Result type used by the `cdma-a9` crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

pub use session::{
    AccessLinkPhase, BsServicePhase, BsServiceRequestState, PendingRequestIdentity,
    ProcedureEngine, ProcedureEvent, ProcedureMessage, ProcedureRole, Session, SessionPhase,
    SessionUpdatePhase, ShortDataPhase, ShortDataRequestState, VersionInfoPhase,
    VersionInfoRequestState,
};
pub use transport::{
    HEADER_LEN, TransportMetadata, UdpSignalingDatagram, UdpSignalingEndpoint, VERSION,
};
pub use typed::{
    A9Indicators, AddsUserPart, AlConnectedAckMessage, AlConnectedMessage,
    AlDisconnectedAckMessage, AlDisconnectedMessage, AnchorPdsnAddress, AnchorPpAddress,
    BsServiceRequestMessage, BsServiceResponseMessage, BscId, CallConnectionReference, CauseValue,
    ConRef, ConnectA8Message, CorrelationId, DataCount, DisconnectA8Message,
    Is2000ServiceConfigurationRecord, Meid, MobileIdentity, PdsnIpAddress,
    QualityOfServiceParametersTyped, ReleaseA8CompleteMessage, ReleaseA8Message, RnPdit,
    ServiceOptionValue, SetupA8Message, ShortDataAckMessage, ShortDataDeliveryMessage,
    SoftwareVersion, SrId, UpdateA8AckMessage, UpdateA8Message, UserZoneId, VersionInfoAckMessage,
    VersionInfoMessage,
};

/// Supported A9 message type values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    SetupA8 = 0x01,
    ConnectA8 = 0x02,
    DisconnectA8 = 0x03,
    ReleaseA8 = 0x04,
    ReleaseA8Complete = 0x05,
    BsServiceRequest = 0x06,
    BsServiceResponse = 0x07,
    AlConnected = 0x08,
    AlConnectedAck = 0x09,
    AlDisconnected = 0x0a,
    AlDisconnectedAck = 0x0b,
    ShortDataDelivery = 0x0c,
    ShortDataAck = 0x0d,
    UpdateA8 = 0x0e,
    UpdateA8Ack = 0x0f,
    VersionInfo = 0x10,
    VersionInfoAck = 0x11,
}

impl MessageType {
    /// Parses an A9 message type from its on-wire value.
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Self::SetupA8),
            0x02 => Ok(Self::ConnectA8),
            0x03 => Ok(Self::DisconnectA8),
            0x04 => Ok(Self::ReleaseA8),
            0x05 => Ok(Self::ReleaseA8Complete),
            0x06 => Ok(Self::BsServiceRequest),
            0x07 => Ok(Self::BsServiceResponse),
            0x08 => Ok(Self::AlConnected),
            0x09 => Ok(Self::AlConnectedAck),
            0x0a => Ok(Self::AlDisconnected),
            0x0b => Ok(Self::AlDisconnectedAck),
            0x0c => Ok(Self::ShortDataDelivery),
            0x0d => Ok(Self::ShortDataAck),
            0x0e => Ok(Self::UpdateA8),
            0x0f => Ok(Self::UpdateA8Ack),
            0x10 => Ok(Self::VersionInfo),
            0x11 => Ok(Self::VersionInfoAck),
            other => Err(Error::UnknownMessageType(other)),
        }
    }
}

/// Supported A9 information elements used by the initial crate surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ElementId {
    ConRef = 0x01,
    UserZoneId = 0x02,
    ServiceOption = 0x03,
    Cause = 0x04,
    A9Indicators = 0x05,
    BscId = 0x06,
    QualityOfServiceParameters = 0x07,
    A8TrafficId = 0x08,
    DataCount = 0x09,
    SrId = 0x0b,
    CorrelationId = 0x13,
    PdsnIpAddress = 0x14,
    MobileIdentity = 0x0d,
    Is2000ServiceConfigurationRecord = 0x0e,
    RnPdit = 0x0f,
    AnchorPdsnAddress = 0x30,
    SoftwareVersion = 0x31,
    AddsUserPart = 0x3d,
    CallConnectionReference = 0x3f,
    AnchorPpAddress = 0x40,
}

impl ElementId {
    /// Parses an A9 information-element identifier.
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Self::ConRef),
            0x02 => Ok(Self::UserZoneId),
            0x03 => Ok(Self::ServiceOption),
            0x04 => Ok(Self::Cause),
            0x05 => Ok(Self::A9Indicators),
            0x06 => Ok(Self::BscId),
            0x07 => Ok(Self::QualityOfServiceParameters),
            0x08 => Ok(Self::A8TrafficId),
            0x09 => Ok(Self::DataCount),
            0x0b => Ok(Self::SrId),
            0x0d => Ok(Self::MobileIdentity),
            0x0e => Ok(Self::Is2000ServiceConfigurationRecord),
            0x0f => Ok(Self::RnPdit),
            0x13 => Ok(Self::CorrelationId),
            0x14 => Ok(Self::PdsnIpAddress),
            0x30 => Ok(Self::AnchorPdsnAddress),
            0x31 => Ok(Self::SoftwareVersion),
            0x3d => Ok(Self::AddsUserPart),
            0x3f => Ok(Self::CallConnectionReference),
            0x40 => Ok(Self::AnchorPpAddress),
            other => Err(Error::UnknownInformationElement(other)),
        }
    }
}

/// Raw A9 information element payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InformationElement {
    pub id: ElementId,
    pub value: Vec<u8>,
}

impl InformationElement {
    /// Creates an A9 information element from its identifier and raw value.
    pub fn new(id: ElementId, value: impl Into<Vec<u8>>) -> Self {
        Self {
            id,
            value: value.into(),
        }
    }
}

/// Structured `A8_Traffic_ID` information element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A8TrafficId {
    pub protocol_stack: u8,
    pub protocol_type: u16,
    pub key: u32,
    pub ip_address: A8IpAddress,
}

/// IP-address variants supported by the `A8_Traffic_ID` information element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A8IpAddress {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl A8TrafficId {
    /// Builds a GRE/IP `A8_Traffic_ID` value for the unstructured byte-stream bearer profile.
    pub fn gre_ppp(key: u32, ip_address: [u8; 4]) -> Self {
        Self {
            protocol_stack: 1,
            protocol_type: 0x8881,
            key,
            ip_address: A8IpAddress::V4(ip_address),
        }
    }

    /// Builds a GRE/IP `A8_Traffic_ID` value with an IPv6 bearer endpoint.
    pub fn gre_ppp_ipv6(key: u32, ip_address: [u8; 16]) -> Self {
        Self {
            protocol_stack: 1,
            protocol_type: 0x8881,
            key,
            ip_address: A8IpAddress::V6(ip_address),
        }
    }

    /// Encodes the traffic identifier into its on-wire payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; 8 + self.address_len()];
        out[0] = self.protocol_stack;
        out[1..3].copy_from_slice(&self.protocol_type.to_be_bytes());
        out[3..7].copy_from_slice(&self.key.to_be_bytes());
        out[7] = self.address_type();
        match self.ip_address {
            A8IpAddress::V4(address) => out[8..12].copy_from_slice(&address),
            A8IpAddress::V6(address) => out[8..24].copy_from_slice(&address),
        }
        out
    }

    /// Decodes an `A8_Traffic_ID` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 8 {
            return Err(Error::InvalidLength {
                expected: 8,
                actual: input.len(),
            });
        }
        let protocol_stack = input[0];
        let protocol_type = u16::from_be_bytes([input[1], input[2]]);
        let key = u32::from_be_bytes([input[3], input[4], input[5], input[6]]);
        let address_type = input[7];
        if protocol_stack != 1 {
            return Err(Error::InvalidValue {
                context: "A8TrafficId.protocol_stack",
                value: protocol_stack as u32,
            });
        }
        if protocol_type != 0x8881 {
            return Err(Error::InvalidValue {
                context: "A8TrafficId.protocol_type",
                value: protocol_type as u32,
            });
        }
        let ip_address = match address_type {
            1 => {
                if input.len() != 12 {
                    return Err(Error::InvalidLength {
                        expected: 12,
                        actual: input.len(),
                    });
                }
                A8IpAddress::V4([input[8], input[9], input[10], input[11]])
            }
            2 => {
                if input.len() != 24 {
                    return Err(Error::InvalidLength {
                        expected: 24,
                        actual: input.len(),
                    });
                }
                A8IpAddress::V6([
                    input[8], input[9], input[10], input[11], input[12], input[13], input[14],
                    input[15], input[16], input[17], input[18], input[19], input[20], input[21],
                    input[22], input[23],
                ])
            }
            _ => {
                return Err(Error::InvalidValue {
                    context: "A8TrafficId.address_type",
                    value: address_type as u32,
                });
            }
        };
        Ok(Self {
            protocol_stack,
            protocol_type,
            key,
            ip_address,
        })
    }

    fn address_type(&self) -> u8 {
        match self.ip_address {
            A8IpAddress::V4(_) => 1,
            A8IpAddress::V6(_) => 2,
        }
    }

    fn address_len(&self) -> usize {
        match self.ip_address {
            A8IpAddress::V4(_) => 4,
            A8IpAddress::V6(_) => 16,
        }
    }
}

/// Decoded A9 message with ordered information elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub message_type: MessageType,
    pub elements: Vec<InformationElement>,
}

impl Message {
    /// Creates a validated A9 message.
    pub fn new(message_type: MessageType, elements: Vec<InformationElement>) -> Result<Self> {
        validate_required(message_type, &elements)?;
        Ok(Self {
            message_type,
            elements,
        })
    }
}

/// Encodes an A9 message as `message_type | IEI | len | value ...`.
pub fn encode(message: &Message) -> Result<Vec<u8>> {
    validate_required(message.message_type, &message.elements)?;
    let mut out = vec![message.message_type as u8];
    for element in &message.elements {
        if element.value.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                expected: u8::MAX as usize,
                actual: element.value.len(),
            });
        }
        out.push(element.id as u8);
        out.push(element.value.len() as u8);
        out.extend_from_slice(&element.value);
    }
    Ok(out)
}

/// Decodes an A9 message from bytes.
pub fn decode(input: &[u8]) -> Result<Message> {
    let Some((&message_type, rest)) = input.split_first() else {
        return Err(Error::EmptyMessage);
    };
    let message_type = MessageType::from_u8(message_type)?;
    let mut elements = Vec::new();
    let mut offset = 0usize;
    while offset < rest.len() {
        if rest.len() - offset < 2 {
            return Err(Error::Truncated {
                needed: offset + 2,
                actual: rest.len(),
            });
        }
        let id = ElementId::from_u8(rest[offset])?;
        let len = rest[offset + 1] as usize;
        let value_start = offset + 2;
        let value_end = value_start + len;
        if rest.len() < value_end {
            return Err(Error::Truncated {
                needed: value_end,
                actual: rest.len(),
            });
        }
        elements.push(InformationElement::new(
            id,
            rest[value_start..value_end].to_vec(),
        ));
        offset = value_end;
    }
    Message::new(message_type, elements)
}

fn validate_required(message_type: MessageType, elements: &[InformationElement]) -> Result<()> {
    use ElementId::*;
    let mut seen = std::collections::BTreeMap::<u8, usize>::new();
    for element in elements {
        validate_allowed(message_type, element.id)?;
        let count = seen.entry(element.id as u8).or_default();
        *count += 1;
        if *count > allowed_repetitions(message_type, element.id) {
            return Err(Error::DuplicateInformationElement(element.id as u8));
        }
    }
    validate_element_order(message_type, elements)?;
    let required: &[ElementId] = match message_type {
        MessageType::SetupA8 => &[ConRef, BscId, A8TrafficId, ServiceOption, A9Indicators],
        MessageType::ConnectA8 => &[ConRef, A8TrafficId, Cause],
        MessageType::DisconnectA8 => &[ConRef, A8TrafficId, Cause],
        MessageType::ReleaseA8 => &[ConRef, A8TrafficId, Cause],
        MessageType::ReleaseA8Complete => &[],
        MessageType::BsServiceRequest => &[ServiceOption, DataCount],
        MessageType::BsServiceResponse => &[],
        MessageType::AlConnected => &[A8TrafficId],
        MessageType::AlConnectedAck => &[],
        MessageType::AlDisconnected => &[A8TrafficId],
        MessageType::AlDisconnectedAck => &[],
        MessageType::ShortDataDelivery => &[AddsUserPart],
        MessageType::ShortDataAck => &[Cause],
        MessageType::UpdateA8 => &[],
        MessageType::UpdateA8Ack => &[],
        MessageType::VersionInfo => &[],
        MessageType::VersionInfoAck => &[],
    };
    for required_id in required {
        if !elements.iter().any(|element| element.id == *required_id) {
            return Err(Error::MissingRequiredInformationElement(*required_id as u8));
        }
    }
    Ok(())
}

fn validate_allowed(message_type: MessageType, element_id: ElementId) -> Result<()> {
    use ElementId::*;
    let allowed: &[ElementId] = match message_type {
        MessageType::SetupA8 => &[
            CallConnectionReference,
            CorrelationId,
            MobileIdentity,
            ConRef,
            QualityOfServiceParameters,
            BscId,
            A8TrafficId,
            ServiceOption,
            A9Indicators,
            UserZoneId,
        ],
        MessageType::ConnectA8 => &[
            CallConnectionReference,
            CorrelationId,
            MobileIdentity,
            ConRef,
            A8TrafficId,
            Cause,
            PdsnIpAddress,
        ],
        MessageType::DisconnectA8 | MessageType::ReleaseA8 => &[
            CallConnectionReference,
            CorrelationId,
            MobileIdentity,
            ConRef,
            A8TrafficId,
            Cause,
        ],
        MessageType::ReleaseA8Complete
        | MessageType::AlConnectedAck
        | MessageType::AlDisconnectedAck => &[CallConnectionReference, CorrelationId],
        MessageType::BsServiceRequest => &[CorrelationId, MobileIdentity, ServiceOption, DataCount],
        MessageType::BsServiceResponse => &[CorrelationId, Cause],
        MessageType::AlConnected => &[
            CallConnectionReference,
            CorrelationId,
            A8TrafficId,
            PdsnIpAddress,
        ],
        MessageType::AlDisconnected => &[CallConnectionReference, CorrelationId, A8TrafficId],
        MessageType::ShortDataDelivery => &[
            CorrelationId,
            MobileIdentity,
            SrId,
            DataCount,
            AddsUserPart,
            A9Indicators,
        ],
        MessageType::ShortDataAck => &[CorrelationId, MobileIdentity, Cause],
        MessageType::UpdateA8 => &[
            CallConnectionReference,
            CorrelationId,
            MobileIdentity,
            Is2000ServiceConfigurationRecord,
            ServiceOption,
            UserZoneId,
            QualityOfServiceParameters,
            Cause,
            RnPdit,
            SrId,
            A9Indicators,
            PdsnIpAddress,
            AnchorPdsnAddress,
            AnchorPpAddress,
        ],
        MessageType::UpdateA8Ack => &[CallConnectionReference, CorrelationId, Cause],
        MessageType::VersionInfo => &[CorrelationId, Cause, SoftwareVersion],
        MessageType::VersionInfoAck => &[CorrelationId, SoftwareVersion],
    };
    if allowed.contains(&element_id) {
        Ok(())
    } else {
        Err(Error::UnexpectedInformationElement {
            message_type,
            element_id: element_id as u8,
        })
    }
}

fn validate_element_order(
    message_type: MessageType,
    elements: &[InformationElement],
) -> Result<()> {
    let mut last_order = None::<u8>;
    let mut saw_imsi = false;
    for element in elements {
        let order = match (message_type, element.id) {
            (_, ElementId::CallConnectionReference) => 0,
            (_, ElementId::CorrelationId) => 1,
            (
                MessageType::SetupA8
                | MessageType::ConnectA8
                | MessageType::DisconnectA8
                | MessageType::ReleaseA8
                | MessageType::BsServiceRequest
                | MessageType::ShortDataDelivery
                | MessageType::ShortDataAck
                | MessageType::UpdateA8,
                ElementId::MobileIdentity,
            ) => match mobile_identity_order(&element.value)? {
                MobileIdentityKind::Imsi => {
                    saw_imsi = true;
                    2
                }
                MobileIdentityKind::Esn => {
                    if !saw_imsi {
                        return Err(Error::InvalidValue {
                            context: "MobileIdentity.sequence",
                            value: 0x05,
                        });
                    }
                    3
                }
                MobileIdentityKind::Meid => {
                    if !saw_imsi {
                        return Err(Error::InvalidValue {
                            context: "MobileIdentity.sequence",
                            value: 0x01,
                        });
                    }
                    4
                }
            },
            (_, ElementId::ConRef) => 4,
            (MessageType::SetupA8, ElementId::QualityOfServiceParameters) => 5,
            (MessageType::SetupA8, ElementId::BscId) => 6,
            (MessageType::UpdateA8, ElementId::Is2000ServiceConfigurationRecord) => 5,
            (MessageType::UpdateA8, ElementId::ServiceOption) => 6,
            (MessageType::UpdateA8, ElementId::UserZoneId) => 7,
            (MessageType::UpdateA8, ElementId::QualityOfServiceParameters) => 8,
            (MessageType::UpdateA8, ElementId::Cause) => 9,
            (MessageType::UpdateA8, ElementId::RnPdit) => 10,
            (MessageType::UpdateA8, ElementId::SrId) => 11,
            (MessageType::UpdateA8, ElementId::A9Indicators) => 12,
            (MessageType::UpdateA8, ElementId::PdsnIpAddress) => 13,
            (MessageType::UpdateA8, ElementId::AnchorPdsnAddress) => 14,
            (MessageType::UpdateA8, ElementId::AnchorPpAddress) => 15,
            (
                MessageType::SetupA8
                | MessageType::ConnectA8
                | MessageType::DisconnectA8
                | MessageType::ReleaseA8
                | MessageType::AlConnected
                | MessageType::AlDisconnected,
                ElementId::A8TrafficId,
            ) => 7,
            (MessageType::SetupA8 | MessageType::BsServiceRequest, ElementId::ServiceOption) => 8,
            (MessageType::ShortDataDelivery, ElementId::SrId) => 5,
            (MessageType::ShortDataDelivery, ElementId::DataCount) => 6,
            (MessageType::ShortDataDelivery, ElementId::AddsUserPart) => 7,
            (MessageType::ShortDataDelivery, ElementId::A9Indicators) => 8,
            (
                MessageType::ConnectA8
                | MessageType::DisconnectA8
                | MessageType::ReleaseA8
                | MessageType::BsServiceResponse
                | MessageType::ShortDataAck
                | MessageType::VersionInfo
                | MessageType::UpdateA8Ack,
                ElementId::Cause,
            ) => 8,
            (MessageType::SetupA8, ElementId::A9Indicators) => 9,
            (MessageType::BsServiceRequest, ElementId::DataCount) => 9,
            (MessageType::ConnectA8 | MessageType::AlConnected, ElementId::PdsnIpAddress) => 9,
            (MessageType::SetupA8, ElementId::UserZoneId) => 10,
            (
                MessageType::VersionInfo | MessageType::VersionInfoAck,
                ElementId::SoftwareVersion,
            ) => 9,
            _ => continue,
        };
        if let Some(previous) = last_order
            && order < previous
        {
            return Err(Error::OutOfOrderInformationElement {
                message_type,
                element_id: element.id as u8,
            });
        }
        last_order = Some(order);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobileIdentityKind {
    Imsi,
    Esn,
    Meid,
}

fn mobile_identity_order(value: &[u8]) -> Result<MobileIdentityKind> {
    let Some((&first, _)) = value.split_first() else {
        return Err(Error::InvalidLength {
            expected: 1,
            actual: 0,
        });
    };
    match first & 0x07 {
        0x06 => Ok(MobileIdentityKind::Imsi),
        0x05 => Ok(MobileIdentityKind::Esn),
        0x01 => Ok(MobileIdentityKind::Meid),
        other => Err(Error::UnknownInformationElement(other)),
    }
}

fn allowed_repetitions(message_type: MessageType, element_id: ElementId) -> usize {
    match (message_type, element_id) {
        (
            MessageType::SetupA8
            | MessageType::ConnectA8
            | MessageType::DisconnectA8
            | MessageType::ReleaseA8
            | MessageType::BsServiceRequest
            | MessageType::ShortDataDelivery
            | MessageType::ShortDataAck
            | MessageType::UpdateA8,
            ElementId::MobileIdentity,
        ) => 3,
        _ => 1,
    }
}
