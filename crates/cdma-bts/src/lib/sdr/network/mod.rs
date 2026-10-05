pub mod clock;
pub mod wire;

#[cfg(feature = "network-backend")]
mod client;
#[cfg(feature = "network-backend")]
pub mod proto {
    tonic::include_proto!("radio.v1");
}
#[cfg(feature = "network-backend")]
pub use client::*;

/// Hardware times cross the wire in nanoseconds.
pub const NETWORK_TICK_RATE: u64 = 1_000_000_000;
