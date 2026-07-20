//! Shared library for the network-in-a-box launcher.
//!
//! Holds the reusable, unit-testable glue (proto conversions) that the
//! `cdma-nib` binary orchestrates.

pub mod convert;
pub mod hrpd_bridge;
