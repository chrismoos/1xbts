//! HRPD Rev 0 Reverse Traffic Channel definitions.
//!
//! Spec references (C.S0024-0 v4.0):
//! - §9.2.1.3.1   Reverse Channel Structure (Walsh assignments, I/Q arms).
//! - §9.2.1.3.3   Reverse Traffic Channel composition.
//! - §9.2.1.3.3.1 Reverse Pilot Channel (TDM with RRI).
//! - §9.2.1.3.3.2 Reverse Rate Indicator (RRI) Channel.
//! - §9.2.1.3.3.3 Data Rate Control (DRC) Channel.
//! - §9.2.1.3.3.4 ACK Channel.
//! - §9.2.1.3.3.5 Data Channel.
//! - Figure 9.2.1.3.1-2 / 9.2.1.3.1-3: Reverse Channel Structure for the
//!   Reverse Traffic Channel (Parts 1 and 2 of 2) — source of the per-sub-
//!   channel Walsh covers and I/Q arm assignments encoded below.
//! - Table 9.2.1.3.1.1-1 / Table 9.2.1.3.4.1-1: physical-layer packet sizes
//!   per data rate (256/512/1024/2048/4096 bits at 9.6/19.2/38.4/76.8/153.6
//!   kbps).
//!
//! NOTE: the C.S0024-0 v4.0 baseline document places the reverse PHY under
//! §9.2. §11-style numbering for the reverse link appears in later revisions
//! (C.S0024-B). All citations here track the v4.0 numbering and the matching
//! figures.

use num::complex::Complex32;

use cdma_common::hrpd::{air::HrpdTrafficEvent, traffic::TrafficFrameError};

use super::data_decoder::{DataDecoder, ReverseDataRate};

/// Number of slots per Reverse Traffic Channel physical-layer packet
/// (C.S0024-0 v4.0 §9.2.1.3.1: "Each frame shall consist of 16 slots").
pub const REVERSE_TRAFFIC_FRAME_SLOTS: usize = 16;

/// Frame duration numerator in milliseconds. The Reverse Traffic Channel
/// frame is 26.66… ms (= 80/3 ms), aligned to PN rollover
/// (C.S0024-0 v4.0 §9.2.1.3.1).
pub const REVERSE_TRAFFIC_FRAME_MS_NUM: u32 = 80;

/// Frame duration denominator in milliseconds. See
/// [`REVERSE_TRAFFIC_FRAME_MS_NUM`].
pub const REVERSE_TRAFFIC_FRAME_MS_DEN: u32 = 3;

/// Chips per slot on the reverse link (C.S0024-0 v4.0 §9.2.1.3.1:
/// "Each slot contains 2048 PN chips").
pub const REVERSE_SLOT_CHIPS: usize = 2048;

/// Reverse-link sub-channels multiplexed on a single Reverse Traffic
/// Channel transmission (C.S0024-0 v4.0 §9.2.1.3.3). The Reverse Rate
/// Indicator (RRI) is TDM-multiplexed onto the Pilot Walsh channel
/// (§9.2.1.3.3.1) and is not enumerated as a distinct sub-channel here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReverseSubChannel {
    /// Unmodulated pilot, TDM-shared with the RRI symbols.
    Pilot,
    /// 4-bit DRC value per active slot, bi-orthogonally encoded.
    Drc,
    /// One BPSK ACK/NAK bit per detected forward subpacket.
    Ack,
    /// Encoded user data at 9.6 / 19.2 / 38.4 / 76.8 / 153.6 kbps.
    Data,
}

/// IQ arm assignment for a reverse sub-channel
/// (C.S0024-0 v4.0 §9.2.1.3.1, Figure 9.2.1.3.1-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IqArm {
    /// In-phase (cosine) carrier component.
    I,
    /// Quadrature (sine) carrier component.
    Q,
}

/// Walsh cover applied to a reverse sub-channel. Some sub-channels (notably
/// DRC) are covered by an outer length-16 Walsh together with an inner
/// length-8 Walsh selected by `DRCCover` (C.S0024-0 v4.0 §9.2.1.3.3.3); we
/// only encode the fixed outer cover here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalshCover {
    /// Walsh function index `i` in `W_i^N`.
    pub index: u8,
    /// Walsh function length `N` in chips.
    pub length: u16,
}

/// Per-sub-channel layout for a single Reverse Traffic Channel transmission.
///
/// Constants below come from C.S0024-0 v4.0 §9.2.1.3.1 and the
/// Figure 9.2.1.3.1-2 / 9.2.1.3.1-3 reverse-traffic block diagram:
///
/// | Sub-channel | Walsh cover | I/Q arm |
/// |-------------|-------------|---------|
/// | Pilot+RRI   | `W_0^16`    | I       |
/// | DRC (outer) | `W_8^16`    | Q       |
/// | ACK         | `W_4^8`     | I       |
/// | Data        | `W_2^4`     | Q       |
#[derive(Debug, Clone, Copy)]
pub struct ReverseTrafficLayout {
    /// Pilot Channel (TDM-multiplexed with the RRI symbols).
    pub pilot: (WalshCover, IqArm),
    /// DRC Channel outer Walsh cover. The inner `W_i^8` (DRCCover) is a
    /// per-slot input and is not captured by this layout.
    pub drc: (WalshCover, IqArm),
    /// ACK Channel.
    pub ack: (WalshCover, IqArm),
    /// Data Channel.
    pub data: (WalshCover, IqArm),
}

impl ReverseTrafficLayout {
    /// Baseline Reverse Traffic Channel layout for HRPD Rev 0.
    ///
    /// Walsh assignments and I/Q arms are taken verbatim from
    /// C.S0024-0 v4.0 §9.2.1.3.1 and Figures 9.2.1.3.1-2 / 9.2.1.3.1-3.
    pub const DEFAULT: Self = Self {
        pilot: (
            WalshCover {
                index: 0,
                length: 16,
            },
            IqArm::I,
        ),
        drc: (
            WalshCover {
                index: 8,
                length: 16,
            },
            IqArm::Q,
        ),
        ack: (
            WalshCover {
                index: 4,
                length: 8,
            },
            IqArm::I,
        ),
        data: (
            WalshCover {
                index: 2,
                length: 4,
            },
            IqArm::Q,
        ),
    };
}

/// One decoded Reverse Traffic Channel physical-layer frame
/// (C.S0024-0 v4.0 §9.2.1.3.3).
#[derive(Debug, Clone)]
pub struct ReverseTrafficFrame {
    /// System-time slot index aligned to the start of the 16-slot frame.
    pub slot_index: u64,
    /// Decoded Data Channel payload, post-CRC, MSB-first packed into bytes.
    pub data_bits: Vec<u8>,
    /// 4-bit DRC value reported per slot (`0x0..=0xF`), in slot order.
    pub drc_per_slot: [u8; REVERSE_TRAFFIC_FRAME_SLOTS],
    /// ACK Channel bit per slot. `None` for gated-off slots,
    /// `Some(true)` for ACK and `Some(false)` for NAK
    /// (C.S0024-0 v4.0 §9.2.1.3.3.4: "A '0' bit (ACK) shall be transmitted
    /// … otherwise, a '1' bit (NAK) shall be transmitted").
    pub ack_per_slot: [Option<bool>; REVERSE_TRAFFIC_FRAME_SLOTS],
    /// Pilot SNR estimate over the frame, in dB.
    pub pilot_snr_db: f32,
}

/// Reverse Traffic Channel decoder trait. Implementations are responsible
/// for pilot tracking, RRI / DRC / ACK demodulation, and Data Channel
/// turbo-decoded payload recovery.
pub trait ReverseTrafficDecoder {
    /// Attempt to decode one 16-slot Reverse Traffic Channel frame from
    /// chip-rate samples covering at least one full frame. Returns `None`
    /// when the pilot has not yet been acquired or when frame alignment
    /// fails.
    fn decode_frame(&mut self, samples: &[Complex32]) -> Option<ReverseTrafficFrame>;
}

/// Reverse Traffic Channel assignment state needed after the AN sends a
/// TrafficChannelAssignment. This is not acquisition state; it is the
/// spec-facing contract for an already assigned MAC index and data-channel
/// rate context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReverseTrafficAssignment {
    pub uati: u32,
    pub mac_index: u8,
    pub rate: ReverseDataRate,
}

impl ReverseTrafficAssignment {
    pub fn new(uati: u32, mac_index: u8, rate: ReverseDataRate) -> Self {
        Self {
            uati,
            mac_index,
            rate,
        }
    }
}

/// Processor for one already-aligned 16-slot Reverse Data Channel frame.
///
/// Pilot acquisition, RRI detection, carrier tracking, and chip/frame
/// alignment stay outside this type. Once those stages supply a frame, this
/// processor performs the spec-bound lower-layer validation and unwrap:
/// Data Channel PHY FCS/tail -> Reverse Traffic MAC -> Connection Layer
/// Format B -> Stream 1 Default Packet Application.
#[derive(Debug, Clone)]
pub struct AlignedReverseTrafficProcessor {
    assignment: ReverseTrafficAssignment,
    data: DataDecoder,
}

impl AlignedReverseTrafficProcessor {
    pub fn new(assignment: ReverseTrafficAssignment) -> Self {
        Self {
            data: DataDecoder::new(assignment.rate),
            assignment,
        }
    }

    pub fn assignment(&self) -> ReverseTrafficAssignment {
        self.assignment
    }

    pub fn process_aligned_data_frame(
        &self,
        samples: &[Complex32],
    ) -> Result<Vec<HrpdTrafficEvent>, TrafficFrameError> {
        self.data
            .decode_stream1_events(self.assignment.uati, self.assignment.mac_index, samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_traffic_frame_slot_count_matches_spec_9_2_1_3_1() {
        // C.S0024-0 v4.0 §9.2.1.3.1: "Each frame shall consist of 16 slots".
        assert_eq!(REVERSE_TRAFFIC_FRAME_SLOTS, 16);
    }

    #[test]
    fn reverse_traffic_frame_duration_matches_spec_9_2_1_3_1() {
        // 80 / 3 ms = 26.66… ms.
        assert_eq!(REVERSE_TRAFFIC_FRAME_MS_NUM, 80);
        assert_eq!(REVERSE_TRAFFIC_FRAME_MS_DEN, 3);
        let ms = f64::from(REVERSE_TRAFFIC_FRAME_MS_NUM) / f64::from(REVERSE_TRAFFIC_FRAME_MS_DEN);
        assert!((ms - 26.6666_f64).abs() < 1e-3);
    }

    #[test]
    fn reverse_traffic_slot_chip_count_matches_spec_9_2_1_3_1() {
        // "Each slot contains 2048 PN chips".
        assert_eq!(REVERSE_SLOT_CHIPS, 2048);
    }

    #[test]
    fn reverse_traffic_layout_matches_figure_9_2_1_3_1_3() {
        let layout = ReverseTrafficLayout::DEFAULT;

        // Pilot+RRI: W_0^16 on I.
        assert_eq!(layout.pilot.0.index, 0);
        assert_eq!(layout.pilot.0.length, 16);
        assert_eq!(layout.pilot.1, IqArm::I);

        // DRC outer: W_8^16 on Q.
        assert_eq!(layout.drc.0.index, 8);
        assert_eq!(layout.drc.0.length, 16);
        assert_eq!(layout.drc.1, IqArm::Q);

        // ACK: W_4^8 on I.
        assert_eq!(layout.ack.0.index, 4);
        assert_eq!(layout.ack.0.length, 8);
        assert_eq!(layout.ack.1, IqArm::I);

        // Data: W_2^4 on Q.
        assert_eq!(layout.data.0.index, 2);
        assert_eq!(layout.data.0.length, 4);
        assert_eq!(layout.data.1, IqArm::Q);
    }

    #[test]
    fn aligned_processor_preserves_assignment_context() {
        let assignment =
            ReverseTrafficAssignment::new(0x8005_8001, 5, super::ReverseDataRate::Kbps9_6);
        let processor = AlignedReverseTrafficProcessor::new(assignment);

        assert_eq!(processor.assignment().uati, 0x8005_8001);
        assert_eq!(processor.assignment().mac_index, 5);
        assert_eq!(processor.assignment().rate, super::ReverseDataRate::Kbps9_6);
    }
}
