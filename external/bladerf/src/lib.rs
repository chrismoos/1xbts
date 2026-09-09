pub mod device;
pub mod error;
pub mod stream;

pub use device::Device;
pub use error::Error;
pub use stream::{RxSync, TxSync};

/// Sample formats accepted by [`Device::sync_config`].
pub mod format {
    /// Interleaved 16-bit I/Q with no metadata header.
    pub const SC16_Q11: u32 = bladerf_sys::bladerf_format::BLADERF_FORMAT_SC16_Q11 as u32;
    /// Interleaved 16-bit I/Q carrying the hardware timestamp header.
    pub const SC16_Q11_META: u32 = bladerf_sys::bladerf_format::BLADERF_FORMAT_SC16_Q11_META as u32;
}

/// Metadata flags and status bits for the synchronous stream calls.
pub mod meta {
    /// Marks the beginning of a TX burst.
    pub const FLAG_TX_BURST_START: u32 = bladerf_sys::BLADERF_META_FLAG_TX_BURST_START;
    /// Marks the end of a TX burst.
    pub const FLAG_TX_BURST_END: u32 = bladerf_sys::BLADERF_META_FLAG_TX_BURST_END;
    /// Transmits immediately, ignoring the metadata timestamp.
    pub const FLAG_TX_NOW: u32 = bladerf_sys::BLADERF_META_FLAG_TX_NOW;
    /// Schedules the burst at the metadata timestamp.
    pub const FLAG_TX_UPDATE_TIMESTAMP: u32 = bladerf_sys::BLADERF_META_FLAG_TX_UPDATE_TIMESTAMP;
    /// Returns samples immediately, ignoring the metadata timestamp.
    pub const FLAG_RX_NOW: u32 = bladerf_sys::BLADERF_META_FLAG_RX_NOW;
    /// An RX overrun occurred, or the hardware reported a fault.
    pub const STATUS_OVERRUN: u32 = bladerf_sys::BLADERF_META_STATUS_OVERRUN;
    /// A TX underrun occurred.
    pub const STATUS_UNDERRUN: u32 = bladerf_sys::BLADERF_META_STATUS_UNDERRUN;
}

/// Stream channel layouts for [`Device::sync_config`].
pub mod layout {
    /// Single-channel RX.
    pub const RX_X1: u32 = bladerf_sys::bladerf_channel_layout::BLADERF_RX_X1 as u32;
    /// Single-channel TX.
    pub const TX_X1: u32 = bladerf_sys::bladerf_channel_layout::BLADERF_TX_X1 as u32;
}

/// Stream directions for [`Device::get_timestamp`].
pub mod direction {
    /// Receive.
    pub const RX: u32 = bladerf_sys::bladerf_direction::BLADERF_RX as u32;
    /// Transmit.
    pub const TX: u32 = bladerf_sys::bladerf_direction::BLADERF_TX as u32;
}

/// Gain control modes for [`Device::set_gain_mode`].
pub mod gain_mode {
    /// Manual gain control, the only mode every bladeRF supports.
    pub const MGC: u32 = bladerf_sys::bladerf_gain_mode::BLADERF_GAIN_MGC as u32;
}
