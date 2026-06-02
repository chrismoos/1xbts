//! OTASP / OTAPA protocol codecs per 3GPP2 C.S0016-D.
//!
//! Pure wire codecs for OTASP Data Messages and parameter blocks. Per §2.3
//! the BTS hands these bytes directly to LAC Data Burst Messages with
//! BURST_TYPE = 0b000100; this crate does not provide its own transport,
//! segmentation, or CRC.

pub mod bits;
pub mod digit;
pub mod imsi;
pub mod message;
pub mod param;

pub use cdma_common::consts::BURST_TYPE_OTASP;
pub use message::result_code::ResultCode;

pub type Error = cdma_common::error::Error;
