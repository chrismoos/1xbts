//! HRPD Rev 0 reverse Traffic Channel RAKE finger and sub-chain processors.
//!
//! This module hosts the four-stage finger sub-chain used by a future HRPD
//! reverse-traffic correlator:
//!
//! ```text
//!  raw IQ block
//!       │
//! HrpdReverseTrafficFinger (one per active UATI/MAC lock)
//!       │ despread → 32768 chips per 16-slot frame, tagged
//!       ▼
//! HrpdReverseTrafficRriProcessor (decode 3-bit RRI codeword)
//!       │
//!       ▼
//! HrpdReverseTrafficAckProcessor (per-slot ACK, HARQ feedback)
//!       │
//!       ▼
//! HrpdReverseTrafficDrcProcessor (per-window DRC values)
//!       │
//!       ▼
//! HrpdReverseTrafficDataProcessor (Turbo Data Channel events)
//! ```
//!
//! The finger emits one `SampleBlock` per 26.6667 ms reverse Traffic
//! physical-layer packet (16 slots × 2048 chips = 32768 despread chips). The
//! tag schema used to communicate per-frame state between the finger and the
//! downstream processors is declared in [`finger`] as `TAG_*` constants.

mod ack_processor;
mod correlator;
mod data_processor;
pub mod despread;
mod drc_processor;
pub mod finger;
mod rri_processor;
pub mod rri_subtype2;
pub mod subframe_harq;
pub mod subtype2_data;

pub use ack_processor::{HrpdReverseTrafficAckProcessor, TAG_ACK_PATTERN_PACKED};
pub use correlator::HrpdReverseTrafficCorrelator;
pub use data_processor::HrpdReverseTrafficDataProcessor;
pub use drc_processor::{DRC_SLOT_GATED_VALUE, HrpdReverseTrafficDrcProcessor, TAG_DRC_PACKED};
pub use finger::{
    HrpdReverseTrafficFinger, HrpdReverseTrafficFingerConfig, HrpdReverseTrafficFingerLock,
    TAG_DRC_COVER, TAG_DRC_LENGTH, TAG_FRAME_START_CHIP, TAG_MAC_INDEX, TAG_PILOT_COHERENCE_X1000,
    TAG_PILOT_SNR_DB_TENTHS, TAG_UATI,
};
pub use rri_processor::{
    HrpdReverseTrafficRriProcessor, TAG_RRI_MARGIN_DB_TENTHS, TAG_RRI_RATE_BPS,
    detect_hrpd_reverse_rri_rate,
};

#[cfg(test)]
mod tests;
