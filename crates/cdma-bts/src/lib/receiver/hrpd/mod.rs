//! HRPD (1xEV-DO Rev 0) reverse-link receiver pipeline.
//!
//! Hosts access-channel and (later) reverse-traffic demodulation. Kept
//! separate from the 1x receiver modules so HRPD-specific finger acquisition,
//! long-code generation, and capsule framing can evolve independently.

pub mod access;
pub mod ack_decoder;
pub mod data_decoder;
pub mod drc_decoder;
pub mod forward;
pub mod long_code;
pub mod pilot_tracker;
pub mod reverse_correlator_base;
pub mod reverse_fft_pilot_search;
pub mod reverse_spread;
pub mod reverse_traffic;
pub mod reverse_traffic_rake;
