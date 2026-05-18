// TODO: remove blanket #[allow(dead_code)] before release — audit each module for
// genuinely dead items and fix them individually instead of suppressing wholesale.
#[allow(dead_code)]
pub(crate) mod a1;
#[allow(dead_code)]
pub(crate) mod access;
#[allow(dead_code)]
pub(crate) mod core;
#[allow(dead_code)]
pub(crate) mod events;
#[allow(dead_code)]
pub mod launcher;
#[allow(dead_code)]
pub(crate) mod mobiles;
#[allow(dead_code)]
pub(crate) mod packet;
#[allow(dead_code)]
pub(crate) mod paging;
#[allow(dead_code)]
pub(crate) mod runtime;
#[allow(dead_code)]
pub(crate) mod sms;
#[cfg(any(test, feature = "test-utils"))]
pub(crate) mod test_utils;
#[allow(dead_code)]
pub(crate) mod traffic_assignment;
#[allow(dead_code)]
pub(crate) mod traffic_bearer;
#[allow(dead_code)]
pub(crate) mod traffic_channel;
#[allow(dead_code)]
pub(crate) mod traffic_events;
#[allow(dead_code)]
pub(crate) mod traffic_forward;
#[allow(dead_code)]
pub(crate) mod traffic_lifecycle;
#[allow(dead_code)]
pub(crate) mod traffic_signaling;
#[allow(dead_code)]
pub(crate) mod voice;

#[cfg(test)]
pub(crate) use a1::PendingA1Assignment;
pub(crate) use a1::{A1ClearState, A1Service};
pub(crate) use access::{AccessService, AccessTx, HlrResolution};
pub use core::{Bsc, Config};
pub use launcher::{
    BscLaunchInputs, BscLaunchParts, build_bsc_launch_parts, connect_configured_bts_client,
};
pub use mobiles::MobileInfo;
pub use packet::DataCallRequest;
pub use paging::{PagingEvent, pch_transmit_event_to_paging_event};
pub use sms::SmsRequest;
pub use traffic_channel::{TrafficPowerOverrideAction, TrafficPowerOverrideRequest};
pub use traffic_events::TrafficEvent;

pub(crate) use core::{
    DEFAULT_PAGE_TIMEOUT_MS, PAGE_RETRY_GUARD_MS, next_bsc_event_id, next_pch_correlation_id,
    recv_or_pending, recv_unbounded_or_pending,
};
pub(crate) use events::EventService;
pub(crate) use mobiles::{AccessRegistrationUpdate, MobileRegistryService, MobileStation, MsState};
pub(crate) use packet::PacketService;
pub(crate) use paging::{PagingService, PendingPage, PendingVoicePage};
#[cfg(test)]
pub(crate) use sms::PendingSmsAck;
pub(crate) use sms::{SmsAckKey, SmsService};
pub(crate) use traffic_assignment::TrafficAssignmentService;
pub(crate) use traffic_bearer::TrafficBearerService;
pub(crate) use traffic_channel::ChannelState;
pub(crate) use traffic_channel::{
    ServiceNegotiationMode, TrafficChannelAction, TrafficChannelInfo, VOICE_TRAFFIC_CON_REF,
    VOICE_TRAFFIC_SR_ID, VoicePollAction, traffic_channel_power_snapshot,
};
pub(crate) use traffic_forward::ForwardSignalingRoute;
pub(crate) use traffic_lifecycle::TrafficLifecycleService;
pub(crate) use traffic_signaling::{
    TrafficSignalingService, mark_reverse_regular_msg_seq_received,
};
pub(crate) use voice::{
    PendingAssignmentFailure, VoiceAlertMode, VoiceLegRole, VoiceService, VoiceSessionKind,
};

/// Re-export the generated `bsc.v1` protobuf API at the path tonic-build uses
/// for cross-package references from the management protos.
pub mod v1 {
    pub use crate::grpc::proto::*;
}

pub use cdma_bts::bts::build_scheduled_message;
pub use cdma_common::overhead::OverheadParameters;

// build_scheduled_message is now in cdma-bts/src/lib/bts/settings.rs
// and re-exported via `cdma_bts::bts::build_scheduled_message`.

#[cfg(test)]
mod tests;
