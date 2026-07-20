//! `cdma-pcf` — PCF node crate.
//!
//! PCF node configuration, A9 signaling, and the A8 GRE bearer relay for
//! HRPD packet-data sessions.

pub mod a9_agent;
pub mod bearer_relay;
pub mod config;
pub mod hrpd_a9_service;
pub mod session;

pub use bearer_relay::{HrpdPcfBearerRuntime, spawn_hrpd_pcf_bearer_relay};
pub use config::PcfNodeConfig;
pub use hrpd_a9_service::{
    build_hrpd_a11_registration_request, configured_a8_ipv4_pair, inverted_udp_gre_bearer,
    spawn_hrpd_pcf_a9_service,
};
pub use session::{
    PcfError, PcfEvent, PcfSession, PcfSessionId, PcfSessionManager, PcfSessionPhase,
    PcfTimerPolicy, Result,
};
