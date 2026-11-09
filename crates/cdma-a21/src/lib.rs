//! A21 between the HRPD AN and the IWS, which is the 1x BSC (A.S0008-C §2.8).

pub mod error;
pub mod identity_cache;
pub mod message;
pub mod transport;

pub use error::{A21Error, Result};
pub use identity_cache::{CachedIdentity, HybridIdentityCache};
pub use message::{
    A21Cause, A21Element, A21EventKind, A21Message, A21MessageType, MessageTransmissionControl,
    MobileIdentity,
};
pub use transport::{A21_UDP_PORT, A21Endpoint, MAX_DATAGRAM_LEN, SINGLE_HOST_AN_PORT};
