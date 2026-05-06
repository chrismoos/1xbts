//! `cdma-pcf` — PCF node crate.
//!
//! Initial scope (WS-0 PR1): node configuration only. A9 signaling, A8
//! GRE bearers, and PCF session state land in WS-3 / WS-4.

pub mod a9_agent;
pub mod config;
pub mod session;

pub use config::PcfNodeConfig;
pub use session::{
    PcfError, PcfEvent, PcfSession, PcfSessionId, PcfSessionManager, PcfSessionPhase,
    PcfTimerPolicy, Result,
};
