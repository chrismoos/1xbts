//!
//! # `uhd`: Bindings to the USRP Hardware Driver library
//!
//! ## Status
//!
//! Basic functionality for configuring some USRP settings and receiving samples is working.
//!
//! Some things are not yet implemented:
//!
//! * Various configuration options related to transmitting
//! * Some configuration options related to receiving and time synchronization
//! * Sending samples to transmit
//!

extern crate libc;
extern crate num_complex;
extern crate uhd_sys;

mod daughter_board_eeprom;
mod error;
mod motherboard_eeprom;
pub mod range;
mod receiver;
mod stream;
mod string_vector;
mod transmitter;
mod tune_request;
mod tune_result;
mod usrp;
mod utils;

// Re-export many public items at the root
pub use daughter_board_eeprom::DaughterBoardEeprom;
pub use error::*;
pub use motherboard_eeprom::MotherboardEeprom;
pub use receiver::{error::*, info::ReceiveInfo, metadata::*, streamer::ReceiveStreamer};
pub use stream::*;
pub use transmitter::{
    async_metadata::TransmitAsyncEvent, info::TransmitInfo, metadata::*, streamer::TransmitStreamer,
};
pub use tune_request::*;
pub use tune_result::TuneResult;
pub use usrp::Usrp;
pub use utils::alloc_boxed_slice;
// Common definitions

/// A time value, represented as an integer number of seconds and a floating-point fraction of
/// a second
#[derive(Debug, Clone, Default, PartialOrd, PartialEq)]
pub struct TimeSpec {
    // In some versions of UHD, the corresponding field of uhd::time_spec_t is a time_t.
    // In other versions, it's a int64_t. The Rust code does conversion to keep this
    // an i64.
    pub seconds: i64,
    pub fraction: f64,
}
