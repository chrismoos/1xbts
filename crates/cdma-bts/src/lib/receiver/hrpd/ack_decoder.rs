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

pub const ACK_DEFAULT_THRESHOLD: f32 = 0.10;

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

const ACK_MIN_STANDARD_ERRORS: f32 = 4.0;
const PILOT_MIN_STANDARD_ERRORS: f32 = 3.0;

pub(crate) struct AckObservation {
    pub mean: Complex32,
    standard_error: f32,
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

    /// Returns the mean over the first 1024 chips in the caller's amplitude scale.
    pub fn despread_slot(&self, samples: &[Complex32]) -> Option<Complex32> {
        self.observe_slot(samples)
            .map(|observation| observation.mean)
    }

    pub(crate) fn observe_slot(&self, samples: &[Complex32]) -> Option<AckObservation> {
        self.observe_rotated_slot(samples, Complex32::new(1.0, 0.0))
    }

    pub(crate) fn observe_pilot_referenced_slot(
        &self,
        samples: &[Complex32],
    ) -> Option<(AckObservation, f32)> {
        let ack_samples = samples.get(..ACK_CHIPS_PER_BIT)?;
        let count = self.walsh_symbols_per_bit() as f64;
        let mut sum = Complex32::new(0.0, 0.0);
        let mut power_sum = 0.0_f64;
        // C.S0024-200-C §2.3.1.3.1: subtype-2 W0 pilot is continuous and orthogonal to ACK.
        for chunk in ack_samples.chunks_exact(self.walsh_len) {
            let pilot = chunk.iter().copied().sum::<Complex32>() / self.walsh_len as f32;
            sum += pilot;
            power_sum += f64::from(pilot.norm_sqr());
        }
        let pilot = sum / count as f32;
        let amplitude = pilot.norm();
        let variance =
            (power_sum - f64::from(sum.norm_sqr()) / count).max(0.0) / (count * (count - 1.0));
        if !amplitude.is_finite()
            || !variance.is_finite()
            || amplitude <= PILOT_MIN_STANDARD_ERRORS * variance.sqrt() as f32
        {
            return Some((
                AckObservation {
                    mean: Complex32::new(0.0, 0.0),
                    standard_error: 0.0,
                },
                0.0,
            ));
        }
        self.observe_rotated_slot(samples, pilot.conj() / amplitude)
            .map(|observation| (observation, amplitude))
    }

    fn observe_rotated_slot(
        &self,
        samples: &[Complex32],
        correction: Complex32,
    ) -> Option<AckObservation> {
        let ack_samples = samples.get(..ACK_CHIPS_PER_BIT)?;
        let decoder = build_walsh_decoder(self.walsh_len, self.walsh_index);
        let mut sum = Complex32::new(0.0, 0.0);
        let mut real_sum = 0.0_f64;
        let mut real_square_sum = 0.0_f64;
        for chunk in ack_samples.chunks_exact(self.walsh_len) {
            let symbol = decoder.process_symbol(chunk) * correction;
            sum += symbol;
            let real = f64::from(symbol.re);
            real_sum += real;
            real_square_sum += real * real;
        }
        let count = self.walsh_symbols_per_bit() as f64;
        let variance_of_mean =
            (real_square_sum - real_sum * real_sum / count).max(0.0) / (count * (count - 1.0));
        Some(AckObservation {
            mean: sum / count as f32,
            standard_error: variance_of_mean.sqrt() as f32,
        })
    }

    pub(crate) fn classify_observation(
        &self,
        observation: &AckObservation,
        reference_amplitude: f32,
    ) -> AckSymbol {
        let noise_gate = ACK_MIN_STANDARD_ERRORS * observation.standard_error;
        if !observation.mean.re.is_finite()
            || !noise_gate.is_finite()
            || observation.mean.re.abs() <= noise_gate
        {
            AckSymbol::Gated
        } else {
            self.classify(observation.mean, reference_amplitude)
        }
    }

    /// Samples begin at the reverse-slot boundary and must include its first 1024 chips.
    pub fn decode_slot(&self, samples: &[Complex32], reference_amplitude: f32) -> AckSymbol {
        let Some(observation) = self.observe_slot(samples) else {
            return AckSymbol::Gated;
        };
        self.classify_observation(&observation, reference_amplitude)
    }

    /// C.S0024 §9.2.1.3.3.4 maps positive I to ACK and negative I to NAK.
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

    #[test]
    fn pilot_reference_preserves_ack_and_nak_across_phase_and_gain() {
        let decoder = AckDecoder::for_physical_layer_subtype(2);
        for phase in [-2.8, -1.6, 0.0, 1.7, 2.1, 3.0] {
            for gain in [0.0001, 0.01, 1.0] {
                for (bit, expected) in [(1.4, AckSymbol::Ack), (-1.4, AckSymbol::Nak)] {
                    let cover = spread_ack_bit_with_cover(
                        bit,
                        ACK_SUBTYPE2_WALSH_LEN,
                        usize::from(ACK_SUBTYPE2_WALSH_INDEX),
                    );
                    let noise = noise_seq(123, ACK_CHIPS_PER_BIT, 0.5);
                    let rotor = Complex32::from_polar(gain, phase);
                    let samples: Vec<_> = cover
                        .iter()
                        .zip(noise)
                        .map(|(ack, noise)| (*ack + Complex32::new(1.0, 0.0) + noise) * rotor)
                        .collect();
                    let (observation, amplitude) =
                        decoder.observe_pilot_referenced_slot(&samples).unwrap();
                    assert_eq!(
                        decoder.classify_observation(&observation, amplitude),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn pilot_reference_rejects_ack_without_pilot_and_pilot_without_ack() {
        let decoder = AckDecoder::for_physical_layer_subtype(2);
        let cover = spread_ack_bit_with_cover(
            1.4,
            ACK_SUBTYPE2_WALSH_LEN,
            usize::from(ACK_SUBTYPE2_WALSH_INDEX),
        );
        for seed in 1..=256 {
            let noise = noise_seq(seed, ACK_CHIPS_PER_BIT, 1.0);
            for (pilot_gain, ack_gain) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0)] {
                let samples: Vec<_> = cover
                    .iter()
                    .zip(&noise)
                    .map(|(ack, noise)| *ack * ack_gain + Complex32::new(pilot_gain, 0.0) + *noise)
                    .collect();
                let (observation, amplitude) =
                    decoder.observe_pilot_referenced_slot(&samples).unwrap();
                assert_eq!(
                    decoder.classify_observation(&observation, amplitude),
                    AckSymbol::Gated,
                    "seed={seed} pilot={pilot_gain} ack={ack_gain}"
                );
            }
        }
    }

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

    #[test]
    fn decodes_under_10db_noise() {
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
    #[test]
    fn noisy_silent_slots_do_not_acknowledge_packets() {
        const TRIALS: u32 = 1024;
        const MAX_ERRORS: u32 = 4;
        const CHIP_NOISE_SIGMA: f32 = 2.0;
        for subtype in [0, 2] {
            let decoder = AckDecoder::for_physical_layer_subtype(subtype);
            let cover = spread_ack_bit_with_cover(0.5, decoder.walsh_len, decoder.walsh_index);
            let mut false_feedback = 0;
            let mut missed_ack = 0;
            let mut missed_nak = 0;
            for trial in 0..TRIALS {
                let noise = noise_seq(trial + 1, ACK_CHIPS_PER_BIT, CHIP_NOISE_SIGMA);
                false_feedback += u32::from(decoder.decode_slot(&noise, 1.0) != AckSymbol::Gated);
                for (sign, expected, missed) in [
                    (1.0, AckSymbol::Ack, &mut missed_ack),
                    (-1.0, AckSymbol::Nak, &mut missed_nak),
                ] {
                    let noisy: Vec<_> = noise
                        .iter()
                        .zip(&cover)
                        .map(|(n, s)| n + s * sign)
                        .collect();
                    *missed += u32::from(decoder.decode_slot(&noisy, 1.0) != expected);
                }
            }
            assert!(
                false_feedback <= MAX_ERRORS,
                "subtype={subtype} false_feedback={false_feedback}"
            );
            assert!(
                missed_ack <= MAX_ERRORS,
                "subtype={subtype} missed_ack={missed_ack}"
            );
            assert!(
                missed_nak <= MAX_ERRORS,
                "subtype={subtype} missed_nak={missed_nak}"
            );
        }
    }
}
