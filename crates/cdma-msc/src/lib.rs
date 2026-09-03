//! `cdma-msc` — MSC node crate.
//!
//! This crate owns MSC-side circuit/session policy above the BSC radio-access
//! domain. Track-B work starts by defining the MSC-side call-control state and
//! the media-gateway abstraction that replaces direct BSC ownership of voice
//! orchestration.

pub mod base_station;
pub mod call_control;
pub mod circuit;
pub mod config;
pub mod grpc;
pub mod management;
pub mod media;
pub mod media_gateway;
pub mod media_gateway_service;
pub mod mgmt_proxy;
pub mod mo_call;
pub mod mt_call;
pub mod mt_page_retry;
pub mod node;
pub mod otasp;
pub mod runtime;
pub(crate) mod sms;
pub mod voice_gateway_client;

pub use base_station::{A1Event, BaseStationId, BaseStations, MscA1Endpoint, ServedCell};
pub use call_control::{
    CallControlError, CallDirection, CallId, CallSessionSnapshot, MscCallController,
};
pub use config::{
    BaseStationConfig, HomeNetworkConfig, MediaRingbackType, MmsConfig, MoOriginationContext,
    MoRoutingDecision, MscNodeConfig, NamDefaultsConfig, OtaspConfig, OtaspWritesConfig,
    StaticVoicePolicy, SystemTagConfig, VoiceConfig, VoiceGatewayConfig, VoicePolicy,
    VoicePolicySnapshot, WelcomeSmsConfig,
};
pub use management::{
    InitiateCallAccepted, InitiateCallRequest, ManagementError, MtCallPlan, PendingControlRequest,
};
pub use media_gateway::{
    CallHandle, CreateCallRequest, MediaGatewayClient, MediaGatewayEvent, MgwError, ReleaseCause,
    VocoderFrame,
};
pub use node::{MscCliOverrides, resolve_config_dir, run_node};
pub use runtime::{MscRuntime, MscRuntimeConfig};
pub use voice_gateway_client::{VoiceGatewayClient, spawn_voice_gateway_client};
