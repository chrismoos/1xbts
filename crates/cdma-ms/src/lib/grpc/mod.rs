mod service;

pub use service::{MsServiceImpl, run_grpc_server, serve_on_listener};

pub mod proto {
    tonic::include_proto!("ms.v1");
}

impl proto::MsState {
    pub fn label(self) -> &'static str {
        match self {
            proto::MsState::Unspecified => "unknown",
            proto::MsState::Off => "off",
            proto::MsState::SystemDetermination => "system_determination",
            proto::MsState::PilotAcquisition => "pilot_acquisition",
            proto::MsState::SyncAcquisition => "sync_acquisition",
            proto::MsState::TimingChange => "timing_change",
            proto::MsState::Idle => "idle",
            proto::MsState::SystemAccess => "system_access",
            proto::MsState::TrafficChannelInit => "traffic_channel_init",
            proto::MsState::TrafficChannel => "traffic_channel",
        }
    }
}

impl proto::PrlVerdictReason {
    pub fn label(self) -> &'static str {
        match self {
            proto::PrlVerdictReason::Unspecified => "unknown",
            proto::PrlVerdictReason::Preferred => "preferred",
            proto::PrlVerdictReason::UnlistedAllowed => "unlisted_allowed",
            proto::PrlVerdictReason::Negative => "negative",
            proto::PrlVerdictReason::Unlisted => "unlisted",
        }
    }
}

impl proto::ScanMode {
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "camp" => Some(proto::ScanMode::Camp),
            "survey" => Some(proto::ScanMode::Survey),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            proto::ScanMode::Unspecified => "unknown",
            proto::ScanMode::Camp => "camp",
            proto::ScanMode::Survey => "survey",
        }
    }
}

impl proto::MobileState {
    pub fn label(self) -> &'static str {
        match self {
            proto::MobileState::Unspecified => "unknown",
            proto::MobileState::Registered => "registered",
            proto::MobileState::Paged => "paged",
            proto::MobileState::PageResponseReceived => "page_response_received",
            proto::MobileState::TrafficAssigning => "traffic_assigning",
            proto::MobileState::TrafficActive => "traffic_active",
        }
    }
}
