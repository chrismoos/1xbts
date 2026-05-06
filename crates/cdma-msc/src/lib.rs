//! `cdma-msc` — MSC node crate.
//!
//! This crate owns MSC-side circuit/session policy above the BSC radio-access
//! domain. Track-B work starts by defining the MSC-side call-control state and
//! the media-gateway abstraction that replaces direct BSC ownership of voice
//! orchestration.

pub mod call_control;
pub mod circuit;
pub mod config;
pub mod grpc;
pub mod management;
pub mod media;
pub mod media_gateway;
pub mod media_gateway_service;
pub mod mo_call;
pub mod mt_call;
pub mod runtime;
pub(crate) mod sms;
pub mod voice_gateway_client;

pub use call_control::{
    CallControlError, CallDirection, CallId, CallSessionSnapshot, MscCallController,
};
pub use config::{
    A1PeerConfig, MediaRingbackType, MoOriginationContext, MoRoutingDecision, MscNodeConfig,
    StaticVoicePolicy, VoiceConfig, VoiceGatewayConfig, VoicePolicy, VoicePolicySnapshot,
    WelcomeSmsConfig,
};
pub use management::{
    InitiateCallAccepted, InitiateCallRequest, ManagementError, MtCallPlan, PendingControlRequest,
};
pub use media_gateway::{
    CallHandle, CreateCallRequest, MediaGatewayClient, MediaGatewayEvent, MgwError, ReleaseCause,
    VocoderFrame,
};
pub use runtime::{MscA1Endpoint, MscRuntime, MscRuntimeConfig};
pub use voice_gateway_client::{VoiceGatewayClient, spawn_voice_gateway_client};
