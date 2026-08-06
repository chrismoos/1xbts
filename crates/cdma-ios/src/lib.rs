//! A1/IOS signaling codec primitives between the BSC and MSC.

use std::fmt::{Display, Formatter};

/// A1 message encoding and decoding (TIA/EIA/IS-2001).
pub mod a1_message;
/// A1 call-control procedures (origination, paging, assignment, release).
pub mod procedures;
/// Well-known A1 port and address constants.
pub mod transport;
/// Strongly-typed A1 information element structs.
pub mod typed;
/// RTP voice bearer management for BSC–voice-gateway media paths.
pub mod voice_bearer;

/// Errors returned by the A1 codec helpers.
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
    InvalidValue {
        context: &'static str,
        reason: &'static str,
    },
    ReservedValue {
        context: &'static str,
        value: u8,
    },
    DuplicateElement {
        message_type: u8,
        id: u8,
    },
    MissingRequiredElement {
        message_type: u8,
        id: u8,
    },
}

/// Result type used by the `cdma-ios` crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

pub use a1_message::{A1TransportError, EncodedA1Message};
pub use procedures::{
    BsServiceProcedure, BsServiceState, BsServiceTimer, CallControlProcedure, CallControlState,
    CallControlTimer, EngineEvent, EngineTimer, MobilityManagementProcedure,
    MobilityManagementState, MobilityManagementTimer, ProcedureDirection, ProcedureEngine,
    ProcedureError, ProcedureMessage, SourceHandoffProcedure, SourceHandoffState,
    SourceHandoffTimer, TargetHandoffProcedure, TargetHandoffState, TargetHandoffTimer,
    TimerAction, Transition,
};
pub use typed::{
    A2pBearerFormatParams,
    A2pBearerSessionParams,
    // ADDS messages (A.S0001 §6.1.7)
    AddsDeliverAckMessage,
    AddsDeliverMessage,
    AddsPageAckMessage,
    AddsPageMessage,
    AddsTransferAckMessage,
    AddsTransferMessage,
    AddsUserPart,
    AlertWithInformationMessage,
    AssignmentCompleteMessage,
    AssignmentFailureMessage,
    AssignmentRequestMessage,
    AuthenticationChallengeParameter,
    AuthenticationConfirmationParameter,
    AuthenticationData,
    AuthenticationEvent,
    AuthenticationParameterCount,
    AuthenticationRequestBsmapMessage,
    AuthenticationRequestDtapMessage,
    AuthenticationRequestMessage,
    AuthenticationResponseBsmapMessage,
    AuthenticationResponseDtapMessage,
    AuthenticationResponseMessage,
    AuthenticationResponseParameter,
    BaseStationChallengeMessage,
    BaseStationChallengeResponseMessage,
    BearerFormatEntry,
    BsServiceRequestMessage,
    BsServiceResponseMessage,
    CalledPartyBcdNumber,
    CallingPartyAsciiNumber,
    Cause,
    CauseLayer3,
    CellId,
    CellIdentifierList,
    ChannelNumber,
    ChannelType,
    CircuitIdentityCode,
    CircuitIdentityCodeExtension,
    ClassmarkInformationType2,
    ClearCommandMessage,
    ClearCompleteMessage,
    ClearRequestMessage,
    CmServiceRequestMessage,
    CmServiceType,
    CompleteLayer3InformationMessage,
    ConnectMessage,
    EncryptionInformation,
    EncryptionParameter,
    ExtendedHandoffDirectionParameters,
    HandoffCdmaServingOneWayDelay,
    HandoffCellIdentifier,
    HandoffCellIdentifierList,
    HandoffCommandMessage,
    HandoffCommencedMessage,
    HandoffCompleteMessage,
    HandoffDownlinkRadioEnvironment,
    HandoffDownlinkRadioEnvironmentRecord,
    HandoffFailureMessage,
    HandoffPerformedMessage,
    HandoffPowerLevel,
    HandoffRequestAcknowledgeMessage,
    HandoffRequestMessage,
    HandoffRequiredMessage,
    HandoffRequiredRejectMessage,
    HardHandoffParameters,
    Is95ChannelEntry,
    Is95ChannelIdentity,
    Is95MsMeasuredChannelIdentity,
    Is2000ChannelEntry,
    Is2000ChannelIdentity,
    Is2000MobileCapabilities,
    Is2000NonNegotiableServiceConfigurationRecord,
    Is2000PhysicalChannelType,
    Is2000ServiceConfigurationRecord,
    Layer3Information,
    LocationAreaIdentification,
    LocationUpdatingAcceptMessage,
    LocationUpdatingRejectMessage,
    LocationUpdatingRequestMessage,
    MobileIdentity,
    MsInformationRecord,
    MsInformationRecords,
    PacaTimestamp,
    PagingRequestMessage,
    PagingResponseMessage,
    ParameterUpdateConfirmMessage,
    ParameterUpdateRequestMessage,
    PdsnIpAddress,
    Priority,
    PrivacyModeCommandMessage,
    PrivacyModeCompleteMessage,
    ProgressMessage,
    ProtocolType,
    QualityOfServiceParameters,
    RadioEnvironmentAndResources,
    RegistrationType,
    RejectCause,
    RfChannelIdentity,
    ServiceOption,
    Sid,
    Signal,
    SlotCycleIndex,
    SsdUpdateChallengeParameter,
    SsdUpdateRequestMessage,
    SsdUpdateResponseMessage,
    Tag,
    UserZoneId,
    UserZoneUpdateMessage,
};
pub use voice_bearer::{
    BearerEvent, BearerPayloadTypes, DtmfBearerEvent, RtpSendState, VoiceBearerFormat,
    VoiceBearerFrame, VoiceBearerManager, VoiceBearerPayloadType,
};

/// Supported A1 message types used for dispatch.
///
/// Variants map to spec BSMAP and DTAP message codes (A.S0001). The u8
/// discriminants are project-local routing tags written into the A1 envelope
/// prefix byte by `encode()` — they are not BSMAP/DTAP message type codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    // BSMAP messages
    CompleteLayer3Information = 0x01,
    CmServiceRequest = 0x02,
    PagingRequest = 0x03,
    PagingResponse = 0x04,
    Connect = 0x0a,
    Progress = 0x0c,
    AssignmentRequest = 0x0f,
    AssignmentComplete = 0x10,
    AssignmentFailure = 0x11,
    ClearRequest = 0x14,
    ClearCommand = 0x15,
    ClearComplete = 0x16,
    AlertWithInformation = 0x18,
    BsServiceRequest = 0x1c,
    BsServiceResponse = 0x1d,
    UserZoneUpdate = 0x1e,
    ParameterUpdateRequest = 0x1f,
    ParameterUpdateConfirm = 0x20,
    PrivacyModeCommand = 0x21,
    PrivacyModeComplete = 0x22,
    LocationUpdatingAccept = 0x23,
    LocationUpdatingReject = 0x24,
    HandoffRequired = 0x40,
    HandoffRequest = 0x41,
    HandoffRequestAcknowledge = 0x42,
    HandoffFailure = 0x43,
    HandoffCommand = 0x44,
    HandoffRequiredReject = 0x45,
    HandoffCommenced = 0x46,
    HandoffComplete = 0x47,
    HandoffPerformed = 0x48,
    // ADDS messages (A.S0001 §6.1.7)
    AddsPage = 0x49,
    AddsTransfer = 0x4a,
    AddsPageAck = 0x4b,
    AddsDeliver = 0x4c,
    AddsDeliverAck = 0x4d,
    AddsTransferAck = 0x4e,
}

impl MessageType {
    /// Parses an A1 message type from the envelope prefix byte.
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Self::CompleteLayer3Information),
            0x02 => Ok(Self::CmServiceRequest),
            0x03 => Ok(Self::PagingRequest),
            0x04 => Ok(Self::PagingResponse),
            0x0a => Ok(Self::Connect),
            0x0c => Ok(Self::Progress),
            0x0f => Ok(Self::AssignmentRequest),
            0x10 => Ok(Self::AssignmentComplete),
            0x11 => Ok(Self::AssignmentFailure),
            0x14 => Ok(Self::ClearRequest),
            0x15 => Ok(Self::ClearCommand),
            0x16 => Ok(Self::ClearComplete),
            0x18 => Ok(Self::AlertWithInformation),
            0x1c => Ok(Self::BsServiceRequest),
            0x1d => Ok(Self::BsServiceResponse),
            0x1e => Ok(Self::UserZoneUpdate),
            0x1f => Ok(Self::ParameterUpdateRequest),
            0x20 => Ok(Self::ParameterUpdateConfirm),
            0x21 => Ok(Self::PrivacyModeCommand),
            0x22 => Ok(Self::PrivacyModeComplete),
            0x23 => Ok(Self::LocationUpdatingAccept),
            0x24 => Ok(Self::LocationUpdatingReject),
            0x40 => Ok(Self::HandoffRequired),
            0x41 => Ok(Self::HandoffRequest),
            0x42 => Ok(Self::HandoffRequestAcknowledge),
            0x43 => Ok(Self::HandoffFailure),
            0x44 => Ok(Self::HandoffCommand),
            0x45 => Ok(Self::HandoffRequiredReject),
            0x46 => Ok(Self::HandoffCommenced),
            0x47 => Ok(Self::HandoffComplete),
            0x48 => Ok(Self::HandoffPerformed),
            0x49 => Ok(Self::AddsPage),
            0x4a => Ok(Self::AddsTransfer),
            0x4b => Ok(Self::AddsPageAck),
            0x4c => Ok(Self::AddsDeliver),
            0x4d => Ok(Self::AddsDeliverAck),
            0x4e => Ok(Self::AddsTransferAck),
            other => Err(Error::UnknownMessageType(other)),
        }
    }
}

/// Minimal A1 envelope used by the migration work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub message_type: MessageType,
    pub payload: Vec<u8>,
}

impl Message {
    /// Creates an A1 message from its message type and encoded payload body.
    pub fn new(message_type: MessageType, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            message_type,
            payload: payload.into(),
        }
    }
}

/// Encodes an A1 message as `[envelope_type_byte | payload]`.
#[must_use = "encoding produces a new buffer; the result should not be discarded"]
pub fn encode(message: &Message) -> Vec<u8> {
    let mut out = vec![message.message_type as u8];
    out.extend_from_slice(&message.payload);
    out
}

/// Decodes an A1 message envelope from bytes.
pub fn decode(input: &[u8]) -> Result<Message> {
    let Some((&message_type, rest)) = input.split_first() else {
        return Err(Error::EmptyMessage);
    };
    Ok(Message::new(
        MessageType::from_u8(message_type)?,
        rest.to_vec(),
    ))
}
