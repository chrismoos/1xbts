//! Abis control-plane message inventory and validation.

use super::ies::{ElementId, InformationElement};
use crate::{Error, Result};

/// Nominal message direction on the Abis control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    BscToBts,
    BtsToBsc,
}

/// Supported Abis control-plane and inherited traffic message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Connect = 0x01,
    ConnectAck = 0x02,
    Remove = 0x03,
    RemoveAck = 0x04,
    FchForward = 0x0b,
    FchReverse = 0x0c,
    TrafficChannelStatus = 0x0d,
    DcchForward = 0x0e,
    DcchReverse = 0x0f,
    SchForward = 0x10,
    SchReverse = 0x11,
    PacaUpdate = 0x6e,
    BtsSetup = 0x80,
    BtsSetupAck = 0x81,
    BtsRelease = 0x82,
    BtsReleaseAck = 0x83,
    BtsReleaseRequest = 0x84,
    PchMessageTransfer = 0x8c,
    PchMessageTransferAck = 0x8d,
    AchMessageTransfer = 0x8e,
    BurstRequest = 0x90,
    BurstResponse = 0x91,
    BurstCommit = 0x92,
}

impl MessageType {
    /// Parses an Abis message type from its on-wire value.
    pub fn from_u8(value: u8) -> Result<Self> {
        let message_type = match value {
            0x01 => MessageType::Connect,
            0x02 => MessageType::ConnectAck,
            0x03 => MessageType::Remove,
            0x04 => MessageType::RemoveAck,
            0x0b => MessageType::FchForward,
            0x0c => MessageType::FchReverse,
            0x0d => MessageType::TrafficChannelStatus,
            0x0e => MessageType::DcchForward,
            0x0f => MessageType::DcchReverse,
            0x10 => MessageType::SchForward,
            0x11 => MessageType::SchReverse,
            0x6e => MessageType::PacaUpdate,
            0x80 => MessageType::BtsSetup,
            0x81 => MessageType::BtsSetupAck,
            0x82 => MessageType::BtsRelease,
            0x83 => MessageType::BtsReleaseAck,
            0x84 => MessageType::BtsReleaseRequest,
            0x8c => MessageType::PchMessageTransfer,
            0x8d => MessageType::PchMessageTransferAck,
            0x8e => MessageType::AchMessageTransfer,
            0x90 => MessageType::BurstRequest,
            0x91 => MessageType::BurstResponse,
            0x92 => MessageType::BurstCommit,
            other => return Err(Error::UnknownMessageType(other)),
        };
        Ok(message_type)
    }

    /// Returns the encoded message-type octet.
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Returns the nominal direction defined by the spec for this message.
    pub const fn direction(self) -> Direction {
        match self {
            MessageType::Connect
            | MessageType::Remove
            | MessageType::TrafficChannelStatus
            | MessageType::FchReverse
            | MessageType::DcchReverse
            | MessageType::SchReverse
            | MessageType::BtsSetupAck
            | MessageType::BtsReleaseAck
            | MessageType::BtsReleaseRequest
            | MessageType::PchMessageTransferAck
            | MessageType::AchMessageTransfer
            | MessageType::BurstResponse => Direction::BtsToBsc,
            MessageType::ConnectAck
            | MessageType::RemoveAck
            | MessageType::FchForward
            | MessageType::DcchForward
            | MessageType::SchForward
            | MessageType::PacaUpdate
            | MessageType::BtsSetup
            | MessageType::BtsRelease
            | MessageType::PchMessageTransfer
            | MessageType::BurstRequest
            | MessageType::BurstCommit => Direction::BscToBts,
        }
    }
}

/// A decoded Abis control message consisting of ordered information elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbisMessage {
    pub message_type: MessageType,
    pub elements: Vec<InformationElement>,
}

impl AbisMessage {
    /// Creates a validated Abis control message.
    pub fn new(message_type: MessageType, elements: Vec<InformationElement>) -> Result<Self> {
        validate_elements(message_type, &elements)?;
        super::typed::validate_message_semantics(message_type, &elements)?;
        Ok(Self {
            message_type,
            elements,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ElementRule {
    pub id: ElementId,
    pub required: bool,
    pub repeatable: bool,
}

const fn rule(id: ElementId, required: bool) -> ElementRule {
    ElementRule {
        id,
        required,
        repeatable: false,
    }
}

const fn repeat_rule(id: ElementId, required: bool) -> ElementRule {
    ElementRule {
        id,
        required,
        repeatable: true,
    }
}

pub(crate) fn rules(message_type: MessageType) -> Vec<ElementRule> {
    use ElementId::*;
    match message_type {
        MessageType::AchMessageTransfer => vec![
            rule(CorrelationId, false),
            repeat_rule(MobileIdentity, false),
            rule(CellIdentifier, false),
            rule(BtsL2Termination, false),
            rule(AirInterfaceMessage, false),
            rule(CdmaServingOneWayDelay, true),
            rule(AuthenticationChallengeParameter, false),
        ],
        MessageType::PchMessageTransfer => vec![
            rule(CorrelationId, false),
            repeat_rule(MobileIdentity, false),
            rule(CellIdentifierList, false),
            rule(AirInterfaceMessage, false),
            rule(Layer2AckRequestResults, false),
            rule(AbisAckNotify, false),
        ],
        MessageType::PchMessageTransferAck => vec![
            rule(CorrelationId, false),
            rule(Cause, false),
            rule(BtsL2Termination, false),
        ],
        MessageType::BtsSetup => vec![
            rule(CallConnectionReference, true),
            rule(BandClass, false),
            rule(PrivacyInfo, false),
            rule(SduId, false),
            repeat_rule(MobileIdentity, false),
            rule(PhysicalChannelInfo, false),
            rule(ServiceOption, false),
            rule(PacaTimestamp, false),
            rule(QualityOfServiceParameters, false),
            repeat_rule(A3ConnectInformation, false),
            rule(AbisOriginatingId, false),
            rule(CdmaServingOneWayDelay, true),
            rule(CdmaTargetOneWayDelay, false),
            rule(WalshCodeAssignmentRequest, false),
        ],
        MessageType::BtsSetupAck => vec![
            rule(CallConnectionReference, true),
            repeat_rule(A3ConnectInformation, false),
            rule(AbisOriginatingId, false),
            rule(AbisDestinationId, false),
            rule(Cause, false),
        ],
        MessageType::BtsRelease => vec![
            rule(CallConnectionReference, true),
            rule(CellIdentifierList, false),
            rule(CorrelationId, false),
        ],
        MessageType::BtsReleaseAck => vec![
            rule(CallConnectionReference, true),
            rule(CorrelationId, false),
        ],
        MessageType::BurstRequest => vec![
            rule(CallConnectionReference, false),
            rule(BandClass, false),
            rule(DownlinkRadioEnvironment, false),
            rule(CdmaServingOneWayDelay, false),
            rule(PrivacyInfo, false),
            rule(CorrelationId, false),
            rule(SduId, false),
            repeat_rule(MobileIdentity, false),
            rule(CellIdentifierList, false),
            rule(ForwardBurstRadioInfo, false),
            rule(ReverseBurstRadioInfo, false),
            rule(AbisDestinationId, false),
        ],
        MessageType::BurstResponse => vec![
            rule(CallConnectionReference, false),
            rule(CorrelationId, false),
            repeat_rule(CellIdentifierList, false),
            rule(ForwardBurstRadioInfo, false),
            rule(ReverseBurstRadioInfo, false),
            rule(AbisDestinationId, false),
        ],
        MessageType::BurstCommit => vec![
            rule(CallConnectionReference, false),
            rule(CorrelationId, false),
            repeat_rule(CellIdentifierList, false),
            rule(ForwardBurstRadioInfo, false),
            rule(ReverseBurstRadioInfo, false),
            rule(Is2000PowerControlMode, false),
            rule(Is2000FpcGainRatioInfo, false),
            rule(AbisDestinationId, false),
        ],
        MessageType::Connect => vec![
            rule(CallConnectionReference, true),
            rule(CorrelationId, false),
            rule(SduId, false),
            repeat_rule(A3ConnectInformation, true),
            rule(PhysicalChannelInfo, true),
        ],
        MessageType::ConnectAck => vec![
            rule(CallConnectionReference, true),
            rule(CorrelationId, false),
            repeat_rule(A3ConnectAckInformation, true),
        ],
        MessageType::Remove => vec![
            rule(CallConnectionReference, true),
            rule(CorrelationId, false),
            rule(SduId, false),
            repeat_rule(A3RemoveInformation, true),
        ],
        MessageType::RemoveAck => vec![
            rule(CallConnectionReference, true),
            rule(CorrelationId, false),
            rule(A3DestinationId, false),
        ],
        MessageType::TrafficChannelStatus => vec![
            rule(CallConnectionReference, true),
            rule(CellIdentifierList, true),
            rule(ChannelElementStatus, true),
            rule(SduId, false),
            rule(A3DestinationId, false),
            rule(A7DestinationId, false),
        ],
        MessageType::BtsReleaseRequest => vec![
            rule(CallConnectionReference, true),
            rule(Cause, false),
            rule(ManufacturerSpecificRecords, false),
        ],
        MessageType::PacaUpdate => vec![
            rule(CallConnectionReference, true),
            rule(MobileIdentity, false),
            rule(PacaOrder, false),
        ],
        MessageType::FchForward
        | MessageType::FchReverse
        | MessageType::DcchForward
        | MessageType::DcchReverse
        | MessageType::SchForward
        | MessageType::SchReverse => Vec::new(),
    }
}

pub(crate) fn validate_elements(
    message_type: MessageType,
    elements: &[InformationElement],
) -> Result<()> {
    let rules = rules(message_type);
    if rules.is_empty() {
        if elements.is_empty() {
            return Ok(());
        }
        return Err(Error::InvalidValue {
            context: "Abis bearer message",
            reason: "traffic messages are encoded by cdma_abis::bearer",
        });
    }

    let mut last_rule_index = 0usize;
    for element in elements {
        let Some((index, element_rule)) =
            rules.iter().enumerate().find(|(_, r)| r.id == element.id)
        else {
            return Err(Error::UnknownInformationElement(element.id.value()));
        };
        if index < last_rule_index {
            return Err(Error::OutOfOrderElement {
                message_type: message_type.value(),
                id: element.id.value(),
            });
        }
        if !element_rule.repeatable
            && elements
                .iter()
                .filter(|candidate| candidate.id == element.id)
                .count()
                > 1
        {
            return Err(Error::DuplicateElement {
                message_type: message_type.value(),
                id: element.id.value(),
            });
        }
        last_rule_index = index;
    }

    for element_rule in rules.iter().filter(|rule| rule.required) {
        if !elements.iter().any(|element| element.id == element_rule.id) {
            return Err(Error::MissingRequiredElement {
                message_type: message_type.value(),
                id: element_rule.id.value(),
            });
        }
    }

    Ok(())
}
