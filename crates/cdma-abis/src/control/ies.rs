//! Abis control-plane information elements.

use super::messages::MessageType;
use crate::{Error, Result};

/// Known Abis information element identifiers used by the supported message set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ElementId {
    ServiceOption,
    Cause,
    CellIdentifier,
    PhysicalChannelInfo,
    QualityOfServiceParameters,
    AbisCellInfo,
    CdmaTargetOneWayDelay,
    CdmaServingOneWayDelay,
    MobileIdentity,
    ForwardBurstRadioInfo,
    ReverseBurstRadioInfo,
    CorrelationId,
    Is2000PowerControlMode,
    Is2000FpcGainRatioInfo,
    ChannelElementStatus,
    CellIdentifierList,
    A3ConnectInformation,
    A3ConnectAckInformation,
    PrivacyInfo,
    A3RemoveInformation,
    AirInterfaceMessage,
    Layer2AckRequestResults,
    DownlinkRadioEnvironment,
    A7DestinationId,
    CallConnectionReference,
    AuthenticationChallengeParameter,
    SduId,
    PacaTimestamp,
    BandClass,
    PacaOrder,
    A3DestinationId,
    ManufacturerSpecificRecords,
    AbisOriginatingId,
    AbisDestinationId,
    BtsL2Termination,
    WalshCodeAssignmentRequest,
    AbisAckNotify,
}

impl ElementId {
    /// Parses an information-element identifier from its on-wire value.
    pub fn from_u8(value: u8) -> Result<Self> {
        let id = match value {
            0x03 => ElementId::ServiceOption,
            0x04 => ElementId::Cause,
            0x05 => ElementId::CellIdentifier,
            0x07 => ElementId::QualityOfServiceParameters,
            0x08 => ElementId::AbisCellInfo,
            0x0b => ElementId::CdmaTargetOneWayDelay,
            0x0c => ElementId::CdmaServingOneWayDelay,
            0x0d => ElementId::MobileIdentity,
            0x11 => ElementId::ForwardBurstRadioInfo,
            0x12 => ElementId::ReverseBurstRadioInfo,
            0x13 => ElementId::CorrelationId,
            0x14 => ElementId::Is2000PowerControlMode,
            0x15 => ElementId::Is2000FpcGainRatioInfo,
            0x18 => ElementId::ChannelElementStatus,
            0x1a => ElementId::CellIdentifierList,
            0x1b => ElementId::A3ConnectInformation,
            0x1c => ElementId::A3ConnectAckInformation,
            0x1d => ElementId::PrivacyInfo,
            0x1e => ElementId::A3RemoveInformation,
            0x21 => ElementId::AirInterfaceMessage,
            0x23 => ElementId::Layer2AckRequestResults,
            0x29 => ElementId::DownlinkRadioEnvironment,
            0x2d => ElementId::A7DestinationId,
            0x3f => ElementId::CallConnectionReference,
            0x41 => ElementId::AuthenticationChallengeParameter,
            0x4c => ElementId::SduId,
            0x4e => ElementId::PacaTimestamp,
            0x5d => ElementId::BandClass,
            0x55 => ElementId::A3DestinationId,
            0x5f => ElementId::PacaOrder,
            0x70 => ElementId::ManufacturerSpecificRecords,
            0x71 => ElementId::AbisOriginatingId,
            0x72 => ElementId::AbisDestinationId,
            0x73 => ElementId::BtsL2Termination,
            0x74 => ElementId::WalshCodeAssignmentRequest,
            0x75 => ElementId::AbisAckNotify,
            other => return Err(Error::UnknownInformationElement(other)),
        };
        Ok(id)
    }

    pub(crate) fn classify_for_message(
        message_type: MessageType,
        seen: &[InformationElement],
        value: u8,
    ) -> Result<Self> {
        match (message_type, value) {
            (MessageType::Connect, 0x07) => Ok(ElementId::PhysicalChannelInfo),
            (MessageType::BtsSetup, 0x07) => {
                let crossed_physical_channel_slot = seen.iter().any(|element| {
                    matches!(
                        element.id,
                        ElementId::PhysicalChannelInfo
                            | ElementId::ServiceOption
                            | ElementId::PacaTimestamp
                            | ElementId::QualityOfServiceParameters
                    )
                });
                if crossed_physical_channel_slot {
                    Ok(ElementId::QualityOfServiceParameters)
                } else {
                    Ok(ElementId::PhysicalChannelInfo)
                }
            }
            _ => Self::from_u8(value),
        }
    }

    /// Returns the encoded identifier value.
    pub const fn value(self) -> u8 {
        match self {
            ElementId::ServiceOption => 0x03,
            ElementId::Cause => 0x04,
            ElementId::CellIdentifier => 0x05,
            ElementId::PhysicalChannelInfo | ElementId::QualityOfServiceParameters => 0x07,
            ElementId::AbisCellInfo => 0x08,
            ElementId::CdmaTargetOneWayDelay => 0x0b,
            ElementId::CdmaServingOneWayDelay => 0x0c,
            ElementId::MobileIdentity => 0x0d,
            ElementId::ForwardBurstRadioInfo => 0x11,
            ElementId::ReverseBurstRadioInfo => 0x12,
            ElementId::CorrelationId => 0x13,
            ElementId::Is2000PowerControlMode => 0x14,
            ElementId::Is2000FpcGainRatioInfo => 0x15,
            ElementId::ChannelElementStatus => 0x18,
            ElementId::CellIdentifierList => 0x1a,
            ElementId::A3ConnectInformation => 0x1b,
            ElementId::A3ConnectAckInformation => 0x1c,
            ElementId::PrivacyInfo => 0x1d,
            ElementId::A3RemoveInformation => 0x1e,
            ElementId::AirInterfaceMessage => 0x21,
            ElementId::Layer2AckRequestResults => 0x23,
            ElementId::DownlinkRadioEnvironment => 0x29,
            ElementId::A7DestinationId => 0x2d,
            ElementId::CallConnectionReference => 0x3f,
            ElementId::AuthenticationChallengeParameter => 0x41,
            ElementId::SduId => 0x4c,
            ElementId::PacaTimestamp => 0x4e,
            ElementId::A3DestinationId => 0x55,
            ElementId::BandClass => 0x5d,
            ElementId::PacaOrder => 0x5f,
            ElementId::ManufacturerSpecificRecords => 0x70,
            ElementId::AbisOriginatingId => 0x71,
            ElementId::AbisDestinationId => 0x72,
            ElementId::BtsL2Termination => 0x73,
            ElementId::WalshCodeAssignmentRequest => 0x74,
            ElementId::AbisAckNotify => 0x75,
        }
    }

    pub(crate) const fn framing(self) -> ElementFraming {
        match self {
            ElementId::ServiceOption => ElementFraming::Fixed { payload_len: 2 },
            _ => ElementFraming::Tlv,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElementFraming {
    Fixed { payload_len: usize },
    Tlv,
}

/// A raw Abis information element payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InformationElement {
    pub id: ElementId,
    pub value: Vec<u8>,
}

impl InformationElement {
    /// Creates an information element from an identifier and raw value bytes.
    pub fn new(id: ElementId, value: impl Into<Vec<u8>>) -> Self {
        Self {
            id,
            value: value.into(),
        }
    }

    /// Encodes the element using the on-wire framing defined for its identifier.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        match self.id.framing() {
            ElementFraming::Fixed { payload_len } => {
                if self.value.len() != payload_len {
                    return Err(Error::InvalidLength {
                        context: "Abis information element",
                        expected: payload_len,
                        actual: self.value.len(),
                    });
                }
                out.push(self.id.value());
                out.extend_from_slice(&self.value);
            }
            ElementFraming::Tlv => {
                if self.value.len() > u8::MAX as usize {
                    return Err(Error::InvalidLength {
                        context: "Abis information element",
                        expected: u8::MAX as usize,
                        actual: self.value.len(),
                    });
                }
                out.push(self.id.value());
                out.push(self.value.len() as u8);
                out.extend_from_slice(&self.value);
            }
        }
        Ok(())
    }

    /// Decodes a single information element and returns the element plus bytes consumed.
    pub fn decode(input: &[u8]) -> Result<(Self, usize)> {
        if input.is_empty() {
            return Err(Error::Truncated {
                context: "Abis information element header",
                needed: 1,
                actual: input.len(),
            });
        }
        let id = ElementId::from_u8(input[0])?;
        match id.framing() {
            ElementFraming::Fixed { payload_len } => {
                let end = 1 + payload_len;
                if input.len() < end {
                    return Err(Error::Truncated {
                        context: "Abis information element value",
                        needed: end,
                        actual: input.len(),
                    });
                }
                Ok((
                    Self {
                        id,
                        value: input[1..end].to_vec(),
                    },
                    end,
                ))
            }
            ElementFraming::Tlv => {
                if input.len() < 2 {
                    return Err(Error::Truncated {
                        context: "Abis information element header",
                        needed: 2,
                        actual: input.len(),
                    });
                }
                let len = input[1] as usize;
                let end = 2 + len;
                if input.len() < end {
                    return Err(Error::Truncated {
                        context: "Abis information element value",
                        needed: end,
                        actual: input.len(),
                    });
                }
                Ok((
                    Self {
                        id,
                        value: input[2..end].to_vec(),
                    },
                    end,
                ))
            }
        }
    }
}
