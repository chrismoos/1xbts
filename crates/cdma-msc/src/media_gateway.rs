//! MSC-facing media-gateway abstraction.
//!
//! The MSC owns call/session policy, but it must not embed SIP/RTP transport
//! objects or voice-gateway internals. This trait is the seam between MSC call
//! control and the standalone media-gateway process.

use async_trait::async_trait;
use std::fmt::{Display, Formatter};

/// Opaque media-gateway call handle owned by the MSC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallHandle(pub u64);

/// Request to create a new media-gateway call leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCallRequest {
    /// MSC-local call identifier that this media leg belongs to.
    pub call_id: u64,
    /// Digit string or address identifying the caller, if known.
    pub calling_party: Option<String>,
    /// Digit string or address identifying the callee, if known.
    pub called_party: Option<String>,
    /// Service option used by the mobile leg.
    pub service_option: u16,
}

/// Cause used when the MSC releases a media-gateway call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseCause {
    /// The mobile or BSC cleared the call.
    RadioReleased,
    /// The remote side or gateway cleared the call.
    RemoteReleased,
    /// The remote SIP leg returned a failure response.
    SipFailure,
    /// The gateway timed out waiting for setup or media.
    GatewayTimeout,
    /// The gateway failed to process media.
    MediaError,
    /// The MSC cleared the call for policy or timer reasons.
    Administrative,
    /// Assignment or setup failed before the call was fully connected.
    SetupFailed,
}

/// Vocoder payload forwarded between the MSC and the media gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocoderFrame {
    /// Raw vocoder or traffic-frame payload bytes.
    pub payload: Vec<u8>,
    /// Nominal frame rate in bits per second when the payload depends on it.
    pub rate_bps: u32,
}

/// Events emitted by the media gateway toward the MSC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaGatewayEvent {
    /// SIP 180/183. `codec` is set when the provisional carried SDP.
    Ringing {
        handle: CallHandle,
        sip_status: u32,
        codec: String,
    },
    /// Remote side answered.
    Answered {
        handle: CallHandle,
        sip_status: u32,
        codec: String,
    },
    /// Gateway failed setup.
    Failed {
        handle: CallHandle,
        sip_status: Option<u32>,
        cause: ReleaseCause,
        reason: String,
    },
    /// Gateway released the call.
    Released {
        handle: CallHandle,
        cause: ReleaseCause,
    },
    InboundCall {
        session_id: String,
        called_number: String,
        caller_number: String,
        caller_display: String,
        offered_codecs: Vec<String>,
    },
    InboundCancel {
        session_id: String,
    },
    /// Gateway media frame toward the mobile.
    MediaFrame {
        handle: CallHandle,
        payload: VocoderFrame,
        sequence: u64,
        service_option: u16,
    },
}

/// Errors returned by [`MediaGatewayClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MgwError {
    /// The remote gateway is unavailable.
    Unavailable,
    /// The referenced call handle is unknown.
    UnknownCall(CallHandle),
    /// The request was rejected by the gateway.
    Rejected(&'static str),
    /// Transport-level failure.
    Transport(String),
}

impl Display for MgwError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for MgwError {}

/// MSC-facing client for the standalone media gateway.
#[async_trait]
pub trait MediaGatewayClient: Send + Sync {
    /// Creates a new media-gateway call leg.
    async fn create_call(&self, req: CreateCallRequest) -> Result<CallHandle, MgwError>;

    /// Answers an existing media-gateway call leg.
    async fn answer_call(&self, handle: CallHandle) -> Result<(), MgwError>;

    /// Releases an existing media-gateway call leg.
    async fn release_call(&self, handle: CallHandle, cause: ReleaseCause) -> Result<(), MgwError>;

    /// Forwards a vocoder payload to the media gateway.
    async fn forward_payload(
        &self,
        handle: CallHandle,
        payload: VocoderFrame,
    ) -> Result<(), MgwError>;

    /// Receives the next media-gateway event.
    async fn recv_event(&self) -> Option<MediaGatewayEvent>;

    /// Bind the gateway-allocated inbound `session_id` to a local `CallHandle`
    /// so MSC can drive audio toward the trunk via `forward_payload`.
    async fn register_inbound_session(
        &self,
        _session_id: String,
        _service_option: u16,
    ) -> Result<CallHandle, MgwError> {
        Err(MgwError::Unavailable)
    }

    async fn inbound_progress(&self, _session_id: &str) -> Result<(), MgwError> {
        Ok(())
    }

    async fn inbound_answer(&self, _session_id: &str, _codec: &str) -> Result<(), MgwError> {
        Ok(())
    }

    async fn inbound_reject(&self, _session_id: &str, _sip_status: u16) -> Result<(), MgwError> {
        Ok(())
    }
}
