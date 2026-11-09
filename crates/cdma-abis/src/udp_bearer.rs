//! One bare Abis traffic frame per UDP datagram (A.S0003-A §4.5.6.2).

use crate::Result;
use crate::bearer::{ChannelFamily, Direction, frame_kind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpBearerDatagram {
    pub channel_family: ChannelFamily,
    pub direction: Direction,
    pub bearer_id: u32,
    /// Starts at the message type octet.
    pub payload: Vec<u8>,
}

impl UdpBearerDatagram {
    pub fn from_payload(bearer_id: u32, payload: Vec<u8>) -> Result<Self> {
        let (channel_family, direction) = frame_kind(&payload)?;
        Ok(Self {
            channel_family,
            direction,
            bearer_id,
            payload,
        })
    }
}
