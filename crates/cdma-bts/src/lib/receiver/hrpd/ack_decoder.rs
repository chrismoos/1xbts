//! HRPD Reverse ACK Channel decoder.
//!
//! Spec references (C.S0024-0 v4.0):
//! - §9.2.1.3.3.4 ACK Channel.
//! - §9.2.1.3.1   Reverse Channel Structure (ACK on Walsh channel `W_4^8`,
//!   transmitted on the I channel, first half of slot, 1024 PN chips).
//! - Figures 9.2.1.3.1-5 / 9.2.1.3.1-6 (ACK Channel timing relative to the
//!   Forward Traffic Channel slot n: the AT transmits the response in
//!   reverse slot `n + 3`).
//! - §9.2.1.3 modulation: codeword bit `0` maps to chip `+1`, bit `1` maps
//!   to chip `-1`.
//!
//! Per §9.2.1.3.3.4:
//!   - "The ACK Channel shall be BPSK modulated. A '0' bit (ACK) shall be
//!     transmitted on the ACK Channel if a Forward Traffic Channel physical
//!     layer packet has been successfully received; otherwise, a '1' bit
//!     (NAK) shall be transmitted."
//!   - "For a Forward Traffic Channel physical layer packet transmitted in
//!     slot n on the Forward Channel, the corresponding ACK Channel bit
//!     shall be transmitted in slot n + 3 on the Reverse Channel."
//!   - "The ACK Channel transmission shall be transmitted in the first half
//!     of the slot and shall last for 1024 PN chips."
//!   - "The ACK Channel shall use the Walsh channel identified by the Walsh
//!     function W_4^8 and shall be transmitted on the I channel."
//!
//! Gating: per §9.2.1.3.3.4, the AT transmits an ACK Channel bit only in
//! response to a Forward Traffic Channel slot that is associated with a
//! detected preamble directed to the AT, plus at most one redundant positive
//! ACK on a slot detected as a continuation of a successfully received
//! packet. In all other reverse slots the ACK Channel is gated off. The
//! decoder distinguishes "gated off" from "transmitted NAK" via a magnitude
//! threshold on the integrated I-arm despread output.

use num::complex::Complex32;

use crate::phy::walsh::WalshDecoder;

/// Length of the Rev 0 ACK Channel inner Walsh cover (`W_4^8`) per
/// C.S0024-0 §9.2.1.3.3.4.
pub const ACK_WALSH_LEN: usize = 8;

/// Walsh row `i` in `W_i^8` for the ACK Channel cover.
pub const ACK_WALSH_INDEX: u8 = 4;

/// Length of the subtype-2 ACK Channel inner Walsh cover (`W_12^32`) per
/// C.S0024-200-C §2.3.1.3.3.4.
pub const ACK_SUBTYPE2_WALSH_LEN: usize = 32;

/// Walsh row `i` in `W_i^32` for the subtype-2 ACK Channel cover.
pub const ACK_SUBTYPE2_WALSH_INDEX: u8 = 12;

/// Number of PN chips the ACK transmission occupies (first half slot,
/// "shall last for 1024 PN chips").
pub const ACK_CHIPS_PER_BIT: usize = 1024;

/// Number of repeated length-8 Walsh symbols per ACK bit (`1024 / 8 = 128`).
pub const ACK_WALSH_SYMBOLS_PER_BIT: usize = ACK_CHIPS_PER_BIT / ACK_WALSH_LEN;

/// Number of repeated length-32 subtype-2 Walsh symbols per ACK bit.
pub const ACK_SUBTYPE2_WALSH_SYMBOLS_PER_BIT: usize = ACK_CHIPS_PER_BIT / ACK_SUBTYPE2_WALSH_LEN;

/// Reverse-slot offset of the ACK response relative to the forward slot the
/// AT is acknowledging: forward slot `n` -> reverse slot `n + 3`
/// (§9.2.1.3.3.4 and Figures 9.2.1.3.1-5 / 9.2.1.3.1-6).
pub const ACK_FORWARD_TO_REVERSE_SLOT_OFFSET: u32 = 3;

/// Default gating threshold as a fraction of the reference amplitude (the
/// frame's despread pilot amplitude). The finger's despread chips keep the
/// raw capture amplitude — the PN/LC reference and pilot phase correction
/// are both unit magnitude — so an absolute gate is meaningless across RF
/// gain settings. Live AT NAKs measure ~0.16-0.25 of the despread pilot
/// (well under the -3 dB the TCA grants, between phase error and the AT's
/// own ACK gain choice), so the gate sits at 0.10: still ~2 sigma above
/// integrated noise at the pilot SNRs we acquire at while keeping every
/// observed response decodable.
pub const ACK_DEFAULT_THRESHOLD: f32 = 0.10;

/// Decoded ACK Channel symbol for one reverse slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AckSymbol {
    /// Binary '0' (ACK) -> chip `+1` -> positive real after despread.
    Ack,
    /// Binary '1' (NAK) -> chip `-1` -> negative real after despread.
    Nak,
    /// `|despread.re|` below threshold -> AT did not transmit (gated off
    /// per §9.2.1.3.3.4).
    Gated,
}

impl AckSymbol {}

/// Reverse ACK Channel decoder.
///
/// One decoder instance can be reused across many slots. Configuration is
/// limited to the gating threshold; the spec parameters (Walsh cover, chip
/// length, I-arm placement, BPSK polarity) are fixed.
#[derive(Debug)]
pub struct AckDecoder {
    threshold: f32,
    walsh_index: usize,
    walsh_len: usize,
}

impl Default for AckDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AckDecoder {
    /// Construct an ACK decoder with the default gating threshold.
    pub fn new() -> Self {
        Self {
            threshold: ACK_DEFAULT_THRESHOLD,
            walsh_index: usize::from(ACK_WALSH_INDEX),
            walsh_len: ACK_WALSH_LEN,
        }
    }

    /// Construct an ACK decoder for the negotiated Physical Layer subtype.
    ///
    /// Rev 0/default uses `W_4^8` (C.S0024-0 §9.2.1.3.3.4). Physical Layer
    /// subtype 2 uses `W_12^32` (C.S0024-200-C §2.3.1.3.3.4). Other
    /// subtypes remain on the default cover until their reverse physical
    /// channel is implemented explicitly.
    pub fn for_physical_layer_subtype(physical_layer_subtype: u16) -> Self {
        if physical_layer_subtype == 2 {
            Self {
                threshold: ACK_DEFAULT_THRESHOLD,
                walsh_index: usize::from(ACK_SUBTYPE2_WALSH_INDEX),
                walsh_len: ACK_SUBTYPE2_WALSH_LEN,
            }
        } else {
            Self::new()
        }
    }

    /// Construct an ACK decoder with a custom gating threshold on
    /// `|despread.re|`.
    pub fn with_threshold(threshold: f32) -> Self {
        Self {
            threshold,
            walsh_index: usize::from(ACK_WALSH_INDEX),
            walsh_len: ACK_WALSH_LEN,
        }
    }

    /// Current gating threshold.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    pub fn walsh_index(&self) -> usize {
        self.walsh_index
    }

    pub fn walsh_len(&self) -> usize {
        self.walsh_len
    }

    pub fn walsh_symbols_per_bit(&self) -> usize {
        ACK_CHIPS_PER_BIT / self.walsh_len
    }

    /// Despread the ACK Channel arm of one reverse slot and return the mean
    /// post-despread value over the first 1024 chips. Returns `None` when
    /// fewer than `ACK_CHIPS_PER_BIT` chips are available. The result keeps
    /// the caller's amplitude scale: a clean ACK/NAK bit lands at ±(ACK
    /// channel amplitude) on the real axis.
    pub fn despread_slot(&self, samples: &[Complex32]) -> Option<Complex32> {
        if samples.len() < ACK_CHIPS_PER_BIT {
            return None;
        }
        let ack_samples = &samples[..ACK_CHIPS_PER_BIT];

        let decoder = build_walsh_decoder(self.walsh_len, self.walsh_index);

        // `process_symbol` normalizes by Walsh length, so summing and
        // dividing by the repetition count yields the mean post-despread
        // value per repetition.
        let mut acc = Complex32::new(0.0, 0.0);
        for chunk in ack_samples.chunks_exact(self.walsh_len) {
            acc += decoder.process_symbol(chunk);
        }
        Some(acc / (self.walsh_symbols_per_bit() as f32))
    }

    /// Decode the ACK symbol for one reverse slot, gating against
    /// `reference_amplitude` (the frame's despread pilot amplitude in the
    /// same units as `samples`). The despread chips keep the raw capture
    /// amplitude, so the gate must be relative to a same-frame reference —
    /// an absolute gate silently reads everything as `Gated` at low capture
    /// levels and as noise-driven ACK/NAK at high ones.
    ///
    /// `samples` must contain at least `ACK_CHIPS_PER_BIT` (1024) complex
    /// chips starting at the slot boundary; the decoder only reads the first
    /// 1024. The caller is responsible for selecting the correct reverse
    /// slot (forward slot `n` -> reverse slot `n + 3`, see
    /// [`ACK_FORWARD_TO_REVERSE_SLOT_OFFSET`]).
    ///
    /// Returns:
    /// - `AckSymbol::Ack` for a transmitted binary '0' (positive I-arm).
    /// - `AckSymbol::Nak` for a transmitted binary '1' (negative I-arm).
    /// - `AckSymbol::Gated` if `|despread.re|` is below
    ///   `threshold * reference_amplitude` (the AT did not transmit in this
    ///   slot).
    pub fn decode_slot(&self, samples: &[Complex32], reference_amplitude: f32) -> AckSymbol {
        let Some(avg) = self.despread_slot(samples) else {
            return AckSymbol::Gated;
        };
        self.classify(avg, reference_amplitude)
    }

    /// Classify a mean despread value from [`Self::despread_slot`] against
    /// `threshold * reference_amplitude`. ACK Channel is BPSK on the I arm;
    /// `|re|` gates, the sign decides the bit (chip `+1` -> ACK, chip `-1`
    /// -> NAK per §9.2.1.3 mapping with §9.2.1.3.3.4 binary 0=ACK / 1=NAK).
    pub fn classify(&self, avg: Complex32, reference_amplitude: f32) -> AckSymbol {
        // `<=` so a zero despread stays Gated even when the reference (and
        // therefore the gate) degenerates to zero.
        if avg.re.abs() <= self.threshold * reference_amplitude || avg.re == 0.0 {
            AckSymbol::Gated
        } else if avg.re > 0.0 {
            AckSymbol::Ack
        } else {
            AckSymbol::Nak
        }
    }
}

/// Bitmask helper: given the bitmask of "expected ACK slots" within a 16-slot
/// reverse traffic frame (bit i = reverse slot i is expected to carry an ACK
/// response), return whether slot `i` should be decoded.
///
/// The producer of this mask is the scheduler that drives the forward link;
/// it knows which forward subpackets were directed at this AT and computes
/// the corresponding reverse slot indices via
/// [`ACK_FORWARD_TO_REVERSE_SLOT_OFFSET`]. When no scheduler back-channel is
/// available, callers may pass `0xffff` (decode every slot) and rely on the
/// gating threshold for false-positive rejection.
#[inline]
pub fn slot_expected_in_mask(expected_ack_slot_mask: u16, slot_idx: u32) -> bool {
    if slot_idx >= 16 {
        return false;
    }
    (expected_ack_slot_mask >> slot_idx) & 1 != 0
}

fn build_walsh_decoder(walsh_len: usize, walsh_row: usize) -> WalshDecoder {
    match walsh_len {
        2 => WalshDecoder::new::<2>(walsh_row),
        4 => WalshDecoder::new::<4>(walsh_row),
        8 => WalshDecoder::new::<8>(walsh_row),
        16 => WalshDecoder::new::<16>(walsh_row),
        32 => WalshDecoder::new::<32>(walsh_row),
        64 => WalshDecoder::new::<64>(walsh_row),
        128 => WalshDecoder::new::<128>(walsh_row),
        // walsh_len is one of the fixed ACK Walsh-cover constants, never arbitrary.
        _ => unreachable!("unsupported ACK walsh_len {walsh_len}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::walsh::WalshGenerator;

    fn spread_ack_bit(bit_chip: f32) -> Vec<Complex32> {
        spread_ack_bit_with_cover(bit_chip, ACK_WALSH_LEN, usize::from(ACK_WALSH_INDEX))
    }

    fn spread_ack_bit_with_cover(
        bit_chip: f32,
        walsh_len: usize,
        walsh_index: usize,
    ) -> Vec<Complex32> {
        match walsh_len {
            8 => WalshGenerator::new::<8>(walsh_index, ACK_CHIPS_PER_BIT / walsh_len)
                .feed(Complex32::new(bit_chip, 0.0)),
            32 => WalshGenerator::new::<32>(walsh_index, ACK_CHIPS_PER_BIT / walsh_len)
                .feed(Complex32::new(bit_chip, 0.0)),
            _ => panic!("unsupported test walsh_len {walsh_len}"),
        }
    }

    #[test]
    fn ack_walsh_cover_matches_spec_9_2_1_3_3_4() {
        assert_eq!(ACK_WALSH_LEN, 8);
        assert_eq!(ACK_WALSH_INDEX, 4);
    }

    #[test]
    fn subtype2_ack_walsh_cover_matches_spec_2_3_1_3_3_4() {
        assert_eq!(ACK_SUBTYPE2_WALSH_LEN, 32);
        assert_eq!(ACK_SUBTYPE2_WALSH_INDEX, 12);
        assert_eq!(ACK_SUBTYPE2_WALSH_SYMBOLS_PER_BIT, 32);
    }

    #[test]
    fn ack_chip_duration_matches_spec_9_2_1_3_3_4() {
        assert_eq!(ACK_CHIPS_PER_BIT, 1024);
        assert_eq!(ACK_WALSH_SYMBOLS_PER_BIT, 128);
    }

    #[test]
    fn ack_slot_offset_matches_spec_9_2_1_3_3_4() {
        assert_eq!(ACK_FORWARD_TO_REVERSE_SLOT_OFFSET, 3);
    }

    #[test]
    fn decodes_ack_plus_one() {
        // Binary 0 -> chip +1 -> ACK.
        let samples = spread_ack_bit(1.0);
        let dec = AckDecoder::new();
        assert_eq!(dec.decode_slot(&samples, 1.0), AckSymbol::Ack);
    }

    #[test]
    fn decodes_subtype2_ack_plus_one() {
        let samples = spread_ack_bit_with_cover(
            1.0,
            ACK_SUBTYPE2_WALSH_LEN,
            usize::from(ACK_SUBTYPE2_WALSH_INDEX),
        );
        let dec = AckDecoder::for_physical_layer_subtype(2);
        assert_eq!(dec.decode_slot(&samples, 1.0), AckSymbol::Ack);
    }

    #[test]
    fn decodes_nak_minus_one() {
        // Binary 1 -> chip -1 -> NAK.
        let samples = spread_ack_bit(-1.0);
        let dec = AckDecoder::new();
        assert_eq!(dec.decode_slot(&samples, 1.0), AckSymbol::Nak);
    }

    #[test]
    fn zero_energy_returns_gated() {
        let samples = vec![Complex32::new(0.0, 0.0); ACK_CHIPS_PER_BIT];
        let dec = AckDecoder::new();
        assert_eq!(dec.decode_slot(&samples, 1.0), AckSymbol::Gated);
    }

    #[test]
    fn tiny_energy_below_relative_threshold_returns_gated() {
        let samples: Vec<Complex32> = spread_ack_bit(1.0).into_iter().map(|s| s * 0.01).collect();
        let dec = AckDecoder::new();
        assert_eq!(dec.decode_slot(&samples, 1.0), AckSymbol::Gated);
    }

    #[test]
    fn low_capture_amplitude_still_decodes_with_matching_reference() {
        // An ACK at 1% of full scale must decode when the pilot reference
        // is at the same capture level — the gate is relative, not absolute.
        let samples: Vec<Complex32> = spread_ack_bit(1.0).into_iter().map(|s| s * 0.01).collect();
        let dec = AckDecoder::new();
        assert_eq!(dec.decode_slot(&samples, 0.01), AckSymbol::Ack);
        let naks: Vec<Complex32> = spread_ack_bit(-1.0).into_iter().map(|s| s * 0.01).collect();
        assert_eq!(dec.decode_slot(&naks, 0.01), AckSymbol::Nak);
    }

    #[test]
    fn too_short_input_returns_gated() {
        let samples = vec![Complex32::new(1.0, 0.0); ACK_CHIPS_PER_BIT - 1];
        let dec = AckDecoder::new();
        assert_eq!(dec.decode_slot(&samples, 1.0), AckSymbol::Gated);
    }

    #[test]
    fn slot_expected_in_mask_basic() {
        assert!(slot_expected_in_mask(0xffff, 0));
        assert!(slot_expected_in_mask(0xffff, 15));
        assert!(!slot_expected_in_mask(0xffff, 16));
        assert!(slot_expected_in_mask(0b0000_0000_0000_1000, 3));
        assert!(!slot_expected_in_mask(0b0000_0000_0000_1000, 2));
        assert!(!slot_expected_in_mask(0x0000, 0));
    }

    #[test]
    fn decodes_under_10db_noise() {
        fn noise_seq(seed: u32, n: usize, sigma: f32) -> Vec<Complex32> {
            let mut s = seed;
            let mut out = Vec::with_capacity(n);
            let mut next = || {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / ((1u32 << 24) as f32)
            };
            for _ in 0..n {
                let mut u1 = next();
                if u1 < 1e-7 {
                    u1 = 1e-7;
                }
                let u2 = next();
                let r = (-2.0_f32 * u1.ln()).sqrt();
                let th = 2.0 * std::f32::consts::PI * u2;
                out.push(Complex32::new(sigma * r * th.cos(), sigma * r * th.sin()));
            }
            out
        }

        let sigma = (0.1_f32 / 2.0).sqrt(); // 10 dB SNR.
        for (bit, expected) in [(1.0_f32, AckSymbol::Ack), (-1.0_f32, AckSymbol::Nak)] {
            let clean = spread_ack_bit(bit);
            let noise = noise_seq(0xC0FFEE ^ bit.to_bits(), clean.len(), sigma);
            let noisy: Vec<Complex32> =
                clean.iter().zip(noise.iter()).map(|(a, b)| a + b).collect();
            let dec = AckDecoder::new();
            assert_eq!(
                dec.decode_slot(&noisy, 1.0),
                expected,
                "10 dB SNR ACK decode mismatch for bit {bit}"
            );
        }
    }
}
