//! BSC-side A9 edge.
//!
//! The BSC owns radio assignment and packet air-link coordination, but it no
//! longer owns packet-core anchoring. This module exposes the BSC-facing A9/PCF
//! client boundary used by packet-data traffic-channel code.

pub use crate::packet::{
    InProcessPcfClient, LegacyPcfClient, PacketBearerFrame, PacketSessionMetadata, PcfClient,
};
