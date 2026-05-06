pub use cdma_common::mac::{AccessMode, ChannelType, Reason, SchedulingHint};

use cdma_common::{bits::Bitstream, time::CdmaSystemTime};

use crate::lac::MessageControlStatusBlock;

#[derive(Debug)]
pub enum MacMessage {
    SDUReadyRequest(SDUReadyRequest),
    SDUReadyResponse(SDUReadyResponse),
    DataRequest(DataRequest),
    DataIndication(DataIndication),
    AvailabilityIndication(AvailabilityIndication),
}

/// MAC-Data.Request: LAC sublayer requests PDU transmission.
#[derive(Debug)]
pub struct DataRequest {
    pub channel_type: ChannelType,
    pub size: usize,
    pub data: Bitstream,
    pub mcsb: MessageControlStatusBlock,
}

/// MAC-SDUReady.Response: MAC notifies LAC of selected access mode.
#[derive(Debug)]
pub struct SDUReadyResponse {
    pub access_mode: AccessMode,
}

/// MAC-Data.Indication: MAC delivers received PDU to LAC.
#[derive(Debug)]
pub struct DataIndication {
    pub channel_id: usize,
    pub channel_type: ChannelType,
    pub data: Bitstream,
    pub system_time: CdmaSystemTime,
    pub physical_channel_id: usize,
}

/// MAC-SDUReady.Request: LAC requests availability notifications for a PDU.
#[derive(Debug)]
pub struct SDUReadyRequest {
    pub channel_type: ChannelType,
    pub size: usize,
    pub p: usize,
    pub seqno: usize,
    pub scheduling_hint: SchedulingHint,
}

/// MAC-Availability.Indication: MAC notifies LAC of next transmit window.
#[derive(Debug)]
pub struct AvailabilityIndication {
    pub channel_type: ChannelType,
    pub max_size: usize,
    pub system_time: CdmaSystemTime,
    pub sync_superframe_start: bool,
    /// Exact chip position (since CDMA epoch) for this availability event.
    /// Avoids precision loss from CdmaSystemTime nanosecond round-trip.
    pub chip_cursor: u64,
}
