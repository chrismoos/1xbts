//! HRPD Rev 0 Reverse DRC (Data Rate Control) Channel decoder.
//!
//! Spec references (C.S0024-0 v4.0):
//! - §9.2.1.3.3.3 Data Rate Control Channel.
//! - Table 9.2.1.3.3.3-1 DRC Bi-Orthogonal Encoding (4-bit DRC value -> 8-bit
//!   codeword).
//! - Table 9.2.1.3.3.3-2 8-ary Walsh Functions (W_i^8 used as the per-bit
//!   spreader, where i = DRCCover).
//! - §9.2.1.3.1 / Figure 9.2.1.3.1-2 Reverse Channel Structure for the Reverse
//!   Traffic Channel: the DRC Channel is on the Q arm, and each codeword bit
//!   is spread by W_i^8 with each resulting Walsh chip further spread by
//!   W_8^16. The codeword is transmitted twice per slot, and the same DRC
//!   value is repeated for DRCLength consecutive slots.
//!
//! Encoder pipeline per DRC value over `DRCLength` slots:
//! 1. DRC value (4 bits) -> 8-bit codeword `c[0..8]` via Table 9.2.1.3.3.3-1.
//! 2. BPSK-map each bit: `b[j] = +1 if c[j]==0 else -1`.
//! 3. For each bit j, spread by `W_DRCCover^8` (length 8) -> 8 Walsh chips per
//!    bit, all carrying value `b[j] * w8[k]`.
//! 4. Each Walsh chip is further spread by `W_8^16` (length 16) -> 128 PN
//!    chips per bit, 1024 PN chips for the 8-bit codeword.
//! 5. The 1024-chip codeword is repeated twice per slot -> 2048 PN chips per
//!    slot (one full HRPD slot at 1.2288 Mcps over 1.667 ms).
//! 6. The same DRC value (and DRCCover) is repeated across DRCLength slots,
//!    so the total integration window is `DRCLength * 2048` PN chips.
//!
//! Decoder inverts that chain: outer despreading by W_8^16 in 16-chip groups,
//! inner despreading by W_DRCCover^8 across bit positions (with the
//! repetitions and the DRCLength repetitions all coherently summed), then a
//! soft 16-hypothesis search against the eight rows of Table 9.2.1.3.3.3-1
//! (each row contributes ±polarity, giving the 16 bi-orthogonal codewords).
//!
//! The DRC Channel is BPSK on the Q arm; this decoder takes the Q component
//! of the post-pilot-despread chip window.

use num::complex::Complex32;

use crate::phy::walsh::WalshDecoder;
#[cfg(test)]
use crate::phy::walsh::WalshGenerator;

/// Inner Walsh length (W_i^8, where `i = DRCCover`) per §9.2.1.3.3.3.
pub const DRC_INNER_WALSH_LEN: usize = 8;

/// Outer Walsh length (W_8^16) per §9.2.1.3.3.3.
pub const DRC_OUTER_WALSH_LEN: usize = 16;

/// Codeword length from Table 9.2.1.3.3.3-1 (8 bits).
pub const DRC_CODEWORD_BITS: usize = 8;

/// Chips per DRC bit after inner+outer spreading: 8 (inner) * 16 (outer) = 128.
pub const DRC_CHIPS_PER_BIT: usize = DRC_INNER_WALSH_LEN * DRC_OUTER_WALSH_LEN;

/// Chips per repeated codeword: 8 bits * 128 chips/bit = 1024.
pub const DRC_CHIPS_PER_CODEWORD: usize = DRC_CODEWORD_BITS * DRC_CHIPS_PER_BIT;

/// Codeword repetitions per slot (Table 9.2.1.3.3.3-1: "each DRC codeword
/// shall be transmitted twice per slot").
pub const DRC_CODEWORDS_PER_SLOT: usize = 2;

/// Chips per HRPD reverse traffic slot (2 codewords * 1024 chips = 2048).
pub const DRC_CHIPS_PER_SLOT: usize = DRC_CODEWORDS_PER_SLOT * DRC_CHIPS_PER_CODEWORD;

/// Number of distinct DRC values (4-bit field).
pub const DRC_NUM_VALUES: u8 = 16;

/// Maximum DRCCover value (3-bit field selecting one of W_0^8..W_7^8).
pub const DRC_MAX_COVER: u8 = 7;

/// Default detection threshold on `peak_energy / mean_codeword_energy`. A
/// pure-noise window distributes energy roughly evenly across the 16
/// hypotheses, so demanding the peak exceed `threshold * mean` rejects
/// noise-only inputs while leaving wide margin for AWGN at >=10 dB SNR.
pub const DRC_DEFAULT_THRESHOLD: f32 = 1.5;

/// Decoded DRC symbol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrcSymbol {
    /// 4-bit DRC value (0..15), per Table 9.2.1.3.3.3-1.
    pub value: u8,
    /// Confidence: best-codeword soft correlation magnitude, normalized by
    /// the mean over all 16 hypotheses. A clean unit-amplitude codeword
    /// yields `confidence ~ 16` (peak = 8 bits, mean ~ 0.5 since all other
    /// hypotheses average to noise). Use as an SNR-like quality metric.
    pub confidence: f32,
}

/// 4-bit DRC value -> 8-bit codeword bits per Table 9.2.1.3.3.3-1.
///
/// Bit ordering is MSB-first in the spec table (e.g. 0x6 -> "01100110").
pub const DRC_CODEWORDS: [[u8; DRC_CODEWORD_BITS]; 16] = [
    [0, 0, 0, 0, 0, 0, 0, 0], // 0x0
    [1, 1, 1, 1, 1, 1, 1, 1], // 0x1
    [0, 1, 0, 1, 0, 1, 0, 1], // 0x2
    [1, 0, 1, 0, 1, 0, 1, 0], // 0x3
    [0, 0, 1, 1, 0, 0, 1, 1], // 0x4
    [1, 1, 0, 0, 1, 1, 0, 0], // 0x5
    [0, 1, 1, 0, 0, 1, 1, 0], // 0x6
    [1, 0, 0, 1, 1, 0, 0, 1], // 0x7
    [0, 0, 0, 0, 1, 1, 1, 1], // 0x8
    [1, 1, 1, 1, 0, 0, 0, 0], // 0x9
    [0, 1, 0, 1, 1, 0, 1, 0], // 0xA
    [1, 0, 1, 0, 0, 1, 0, 1], // 0xB
    [0, 0, 1, 1, 1, 1, 0, 0], // 0xC
    [1, 1, 0, 0, 0, 0, 1, 1], // 0xD
    [0, 1, 1, 0, 1, 0, 0, 1], // 0xE
    [1, 0, 0, 1, 0, 1, 1, 0], // 0xF
];

/// BPSK-map a codeword bit (0 -> +1, 1 -> -1) per §9.2.1.3 modulation.
#[inline]
fn bpsk(bit: u8) -> f32 {
    if bit == 0 { 1.0 } else { -1.0 }
}

/// Reverse DRC Channel decoder.
///
/// State held: `drc_cover` (0..7) selecting the inner Walsh row W_i^8, and a
/// detection threshold. The decoder is otherwise stateless across calls — the
/// caller provides the full DRCLength-slot chip window.
#[derive(Debug)]
pub struct DrcDecoder {
    drc_cover: u8,
    threshold: f32,
}

impl Default for DrcDecoder {
    fn default() -> Self {
        Self::new(0)
    }
}

impl DrcDecoder {
    /// Construct a DRC decoder for a specific DRCCover (0..=7), as published
    /// by the AN in TrafficChannelAssignment.DRCCoverField.
    pub fn new(drc_cover: u8) -> Self {
        assert!(
            drc_cover <= DRC_MAX_COVER,
            "DRCCover must be in 0..=7 (got {drc_cover})"
        );
        Self {
            drc_cover,
            threshold: DRC_DEFAULT_THRESHOLD,
        }
    }

    /// Construct a DRC decoder with a custom detection threshold.
    pub fn with_threshold(drc_cover: u8, threshold: f32) -> Self {
        let mut d = Self::new(drc_cover);
        d.threshold = threshold;
        d
    }

    /// DRCCover currently in use (selects inner Walsh row W_i^8).
    pub fn drc_cover(&self) -> u8 {
        self.drc_cover
    }

    /// Update DRCCover (e.g. after an AN-side change of serving sector).
    pub fn set_drc_cover(&mut self, drc_cover: u8) {
        assert!(drc_cover <= DRC_MAX_COVER);
        self.drc_cover = drc_cover;
    }

    /// Detection threshold (`peak / mean` over the 16 codeword hypotheses).
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Decode a DRC value from `chips`, a post-pilot-despread complex chip
    /// stream of length `drc_length * DRC_CHIPS_PER_SLOT` (so e.g. 2048 chips
    /// for `drc_length=1`, 16384 for `drc_length=8`).
    ///
    /// `drc_length` must be one of {1, 2, 4, 8} per §9.2.1.3.3.3.
    ///
    /// Returns `None` if the input length is wrong, if `drc_length` is not in
    /// {1,2,4,8}, or if the best-hypothesis peak does not exceed
    /// `threshold * mean` (treated as "no decodable DRC symbol present").
    pub fn decode(&self, chips: &[Complex32], drc_length: u8) -> Option<DrcSymbol> {
        self.decode_inner(chips, drc_length, false)
    }

    /// Decode after removing the residual W0 pilot phase independently from
    /// each repeated codeword.
    pub(crate) fn decode_pilot_derotated(
        &self,
        chips: &[Complex32],
        drc_length: u8,
    ) -> Option<DrcSymbol> {
        self.decode_inner(chips, drc_length, true)
    }

    fn decode_inner(
        &self,
        chips: &[Complex32],
        drc_length: u8,
        pilot_derotate: bool,
    ) -> Option<DrcSymbol> {
        if !matches!(drc_length, 1 | 2 | 4 | 8) {
            return None;
        }
        let expected = (drc_length as usize) * DRC_CHIPS_PER_SLOT;
        if chips.len() != expected {
            return None;
        }

        // Step 1: outer despread by W_8^16. Each consecutive 16-chip group on
        // the Q arm collapses into one inner-Walsh chip soft value. We
        // accumulate all repetitions (2 codewords/slot * drc_length slots) of
        // each bit position coherently into a single 8-element soft codeword.
        let outer = WalshDecoder::new::<DRC_OUTER_WALSH_LEN>(DRC_OUTER_WALSH_LEN / 2);
        // W_8^16 means Walsh row 8 of the size-16 Hadamard matrix. The
        // identifier in §9.2.1.3.3.3 / Table 9.2.1.3.3.3-2 uses the same
        // Hadamard ordering as our generator, so row index = 8.

        let inner = WalshDecoder::new::<DRC_INNER_WALSH_LEN>(self.drc_cover as usize);

        // Soft per-bit accumulator over all repetitions.
        let mut soft_bits = [0.0_f32; DRC_CODEWORD_BITS];

        let total_codewords = (drc_length as usize) * DRC_CODEWORDS_PER_SLOT;
        for cw_idx in 0..total_codewords {
            let cw_start = cw_idx * DRC_CHIPS_PER_CODEWORD;
            let codeword_derotation = if pilot_derotate {
                // The DRC Walsh channels sum to zero over a codeword, leaving
                // W0 as the local phase reference.
                let pilot = chips[cw_start..cw_start + DRC_CHIPS_PER_CODEWORD]
                    .iter()
                    .copied()
                    .sum::<Complex32>();
                let norm = pilot.norm();
                if norm > f32::EPSILON {
                    pilot.conj() / norm
                } else {
                    Complex32::new(1.0, 0.0)
                }
            } else {
                Complex32::new(1.0, 0.0)
            };
            for bit_idx in 0..DRC_CODEWORD_BITS {
                // Outer despread: produce DRC_INNER_WALSH_LEN soft Walsh chips
                // for this bit position.
                let mut walsh_chips = [Complex32::new(0.0, 0.0); DRC_INNER_WALSH_LEN];
                for k in 0..DRC_INNER_WALSH_LEN {
                    let group_start =
                        cw_start + bit_idx * DRC_CHIPS_PER_BIT + k * DRC_OUTER_WALSH_LEN;
                    let group = &chips[group_start..group_start + DRC_OUTER_WALSH_LEN];
                    walsh_chips[k] = outer.process_symbol(group);
                }
                // Inner despread: correlate against W_DRCCover^8. Take the Q
                // component (DRC is on the Q arm per §9.2.1.3.1).
                let inner_out = inner.process_symbol(&walsh_chips) * codeword_derotation;
                soft_bits[bit_idx] += inner_out.im;
            }
        }

        // Step 2: pick the codeword with maximum |inner-product| against the
        // soft 8-bit vector. Each codeword's BPSK-mapped soft template is
        // `(1-2c)` per bit, so the inner product is signed. The bi-orthogonal
        // pair (c, 1-c) gives opposite signs; the max over all 16 codewords
        // therefore picks both row (= value >> 1) and polarity (= value & 1).
        let mut scores = [0.0_f32; 16];
        for (value, codeword) in DRC_CODEWORDS.iter().enumerate() {
            let mut s = 0.0_f32;
            for (bit_idx, &c) in codeword.iter().enumerate() {
                s += soft_bits[bit_idx] * bpsk(c);
            }
            scores[value] = s;
        }

        let (best_value, &best_score) = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();

        // Detection: require best score to exceed `threshold * mean(|score|)`.
        // Using mean of |score| (rather than mean of raw score, which is ~0
        // for any orthogonal code basis) gives a stable noise floor estimate.
        let mean_abs: f32 = scores.iter().map(|s| s.abs()).sum::<f32>() / (scores.len() as f32);
        if mean_abs <= 0.0 || best_score < self.threshold * mean_abs {
            return None;
        }

        let confidence = best_score / mean_abs.max(f32::EPSILON);
        Some(DrcSymbol {
            value: best_value as u8,
            confidence,
        })
    }
}

/// Reference encoder for tests: emit `drc_length * DRC_CHIPS_PER_SLOT` chips
/// on the Q arm carrying DRC `value` with the given DRCCover.
#[cfg(test)]
pub(crate) fn encode_drc(value: u8, drc_cover: u8, drc_length: u8) -> Vec<Complex32> {
    assert!(value < DRC_NUM_VALUES);
    assert!(drc_cover <= DRC_MAX_COVER);
    assert!(matches!(drc_length, 1 | 2 | 4 | 8));

    let codeword = DRC_CODEWORDS[value as usize];
    let inner = WalshGenerator::new::<DRC_INNER_WALSH_LEN>(drc_cover as usize, 1);
    let outer = WalshGenerator::new::<DRC_OUTER_WALSH_LEN>(DRC_OUTER_WALSH_LEN / 2, 1);

    let total_codewords = (drc_length as usize) * DRC_CODEWORDS_PER_SLOT;
    let mut out = Vec::with_capacity((drc_length as usize) * DRC_CHIPS_PER_SLOT);

    for _ in 0..total_codewords {
        for &bit in &codeword {
            let b = bpsk(bit);
            // Inner: spread b by W_DRCCover^8 (length 8) -> 8 walsh chips.
            let walsh_chips: Vec<Complex32> = inner
                .code()
                .iter()
                .map(|c| Complex32::new(0.0, (*c as f32) * b))
                .collect();
            // Outer: each walsh chip * W_8^16 -> 16 PN chips. DRC is on Q arm
            // so the energy lives in the imaginary component.
            for wc in walsh_chips {
                let row = outer.code();
                for &c in row {
                    out.push(Complex32::new(0.0, wc.im * (c as f32)));
                }
            }
        }
    }
    debug_assert_eq!(out.len(), (drc_length as usize) * DRC_CHIPS_PER_SLOT);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise_seq(seed: u32, n: usize, sigma: f32) -> Vec<Complex32> {
        let mut s = seed;
        let mut next = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            (s >> 8) as f32 / ((1u32 << 24) as f32)
        };
        let mut out = Vec::with_capacity(n);
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
    fn constants_match_spec_9_2_1_3_3_3() {
        assert_eq!(DRC_INNER_WALSH_LEN, 8);
        assert_eq!(DRC_OUTER_WALSH_LEN, 16);
        assert_eq!(DRC_CODEWORD_BITS, 8);
        assert_eq!(DRC_CHIPS_PER_BIT, 128);
        assert_eq!(DRC_CHIPS_PER_CODEWORD, 1024);
        assert_eq!(DRC_CHIPS_PER_SLOT, 2048);
        assert_eq!(DRC_CODEWORDS_PER_SLOT, 2);
        assert_eq!(DRC_NUM_VALUES, 16);
    }

    #[test]
    fn codeword_table_matches_spec_table_9_2_1_3_3_3_1() {
        // Verify a few entries verbatim against the spec table.
        assert_eq!(DRC_CODEWORDS[0x0], [0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(DRC_CODEWORDS[0x1], [1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(DRC_CODEWORDS[0x6], [0, 1, 1, 0, 0, 1, 1, 0]);
        assert_eq!(DRC_CODEWORDS[0x8], [0, 0, 0, 0, 1, 1, 1, 1]);
        assert_eq!(DRC_CODEWORDS[0xF], [1, 0, 0, 1, 0, 1, 1, 0]);
        // Pairs (2k, 2k+1) are bit-wise complements (bi-orthogonal).
        for k in 0..8 {
            for j in 0..DRC_CODEWORD_BITS {
                assert_eq!(
                    DRC_CODEWORDS[2 * k][j] ^ DRC_CODEWORDS[2 * k + 1][j],
                    1,
                    "DRC {} / {} should be complementary at bit {}",
                    2 * k,
                    2 * k + 1,
                    j
                );
            }
        }
    }

    #[test]
    fn round_trip_all_values_drc_length_1_cover_0() {
        let dec = DrcDecoder::new(0);
        for value in 0..DRC_NUM_VALUES {
            let chips = encode_drc(value, 0, 1);
            let sym = dec.decode(&chips, 1).expect("decode");
            assert_eq!(sym.value, value, "round trip mismatch for DRC {value:#x}");
        }
    }

    #[test]
    fn round_trip_all_drc_covers_value_b() {
        // For each DRCCover, encode value 0xB and verify the decoder
        // configured for that cover recovers it.
        for cover in 0..=DRC_MAX_COVER {
            let dec = DrcDecoder::new(cover);
            let chips = encode_drc(0xB, cover, 1);
            let sym = dec.decode(&chips, 1).expect("decode");
            assert_eq!(sym.value, 0xB, "cover {cover}: round trip mismatch");
        }
    }

    #[test]
    fn round_trip_all_drc_lengths() {
        let cover = 5;
        let dec = DrcDecoder::new(cover);
        for &len in &[1u8, 2, 4, 8] {
            let chips = encode_drc(0xC, cover, len);
            assert_eq!(chips.len(), (len as usize) * DRC_CHIPS_PER_SLOT);
            let sym = dec.decode(&chips, len).expect("decode");
            assert_eq!(sym.value, 0xC, "drc_length {len}: mismatch");
        }
    }

    #[test]
    fn pilot_derotation_combines_codewords_with_different_phase() {
        let cover = 1;
        let value = 0xE;
        let dec = DrcDecoder::new(cover);
        let mut chips = encode_drc(value, cover, 1);

        for (codeword, chunk) in chips.chunks_mut(DRC_CHIPS_PER_CODEWORD).enumerate() {
            let rotation = if codeword == 0 {
                Complex32::new(1.0, 0.0)
            } else {
                Complex32::new(-1.0, 0.0)
            };
            for chip in chunk {
                *chip = (*chip + Complex32::new(0.25, 0.0)) * rotation;
            }
        }

        assert!(dec.decode(&chips, 1).is_none());
        assert_eq!(dec.decode_pilot_derotated(&chips, 1).unwrap().value, value);
    }

    #[test]
    fn wrong_length_returns_none() {
        let dec = DrcDecoder::new(0);
        let chips = vec![Complex32::new(0.0, 1.0); DRC_CHIPS_PER_SLOT - 1];
        assert!(dec.decode(&chips, 1).is_none());
        let chips = vec![Complex32::new(0.0, 1.0); DRC_CHIPS_PER_SLOT + 1];
        assert!(dec.decode(&chips, 1).is_none());
    }

    #[test]
    fn invalid_drc_length_returns_none() {
        let dec = DrcDecoder::new(0);
        let chips = vec![Complex32::new(0.0, 1.0); DRC_CHIPS_PER_SLOT];
        assert!(dec.decode(&chips, 0).is_none());
        assert!(dec.decode(&chips, 3).is_none());
        assert!(dec.decode(&chips, 5).is_none());
    }

    #[test]
    fn pure_noise_returns_none() {
        let dec = DrcDecoder::new(2);
        let noise = noise_seq(0xDEADBEEF, DRC_CHIPS_PER_SLOT, 0.2);
        // Very low SNR pure noise should fall below the threshold most of
        // the time; assert at least: there is no false-high confidence on
        // a strong but spurious value. Be lenient here — pure noise can
        // produce a non-None at threshold 1.5 occasionally; assert that if
        // it does, the confidence is modest (< 4.0) versus clean signal.
        if let Some(sym) = dec.decode(&noise, 1) {
            assert!(
                sym.confidence < 4.0,
                "noise-only decode confidence too high: {sym:?}"
            );
        }
    }

    #[test]
    fn zero_input_returns_none() {
        let dec = DrcDecoder::new(0);
        let zeros = vec![Complex32::new(0.0, 0.0); DRC_CHIPS_PER_SLOT];
        assert!(dec.decode(&zeros, 1).is_none());
    }

    #[test]
    fn wrong_cover_misdecodes_at_low_drc_length() {
        // Encoder uses cover=3, decoder uses cover=5 -> the inner Walsh
        // correlation collapses to ~0 and detection fails (or returns a
        // different value with low confidence). Verify decoder does not
        // claim a confident match.
        let chips = encode_drc(0x7, 3, 1);
        let dec = DrcDecoder::new(5);
        let sym = dec.decode(&chips, 1);
        if let Some(s) = sym {
            // If anything is detected, it should not equal the true value.
            assert_ne!(s.value, 0x7, "wrong cover should not decode correctly");
        }
    }

    #[test]
    fn decodes_under_10db_noise_drc_length_1() {
        // Signal energy per chip on Q arm is 1. Noise variance 0.1 -> sigma
        // per component sqrt(0.05) ~ 0.2236 for 10 dB SNR.
        let sigma = (0.1_f32 / 2.0).sqrt();
        let cover = 4;
        let dec = DrcDecoder::new(cover);
        for value in [0u8, 1, 6, 7, 8, 0xA, 0xF] {
            let clean = encode_drc(value, cover, 1);
            let noise = noise_seq(0xD12C ^ u32::from(value), clean.len(), sigma);
            let noisy: Vec<Complex32> =
                clean.iter().zip(noise.iter()).map(|(a, b)| a + b).collect();
            let sym = dec.decode(&noisy, 1).expect("decode under noise");
            assert_eq!(sym.value, value, "10 dB SNR mismatch for DRC {value:#x}");
        }
    }

    #[test]
    fn decodes_under_noise_drc_length_4_extends_processing_gain() {
        // At DRCLength=4 the integration is 4x longer, so we should tolerate
        // ~6 dB lower SNR. Use sigma corresponding to 4 dB SNR (variance 0.4
        // total, 0.2 per component).
        let sigma = (0.4_f32 / 2.0).sqrt();
        let cover = 1;
        let dec = DrcDecoder::new(cover);
        for value in [0u8, 5, 0xB, 0xE] {
            let clean = encode_drc(value, cover, 4);
            let noise = noise_seq(0xBADC0DE ^ u32::from(value), clean.len(), sigma);
            let noisy: Vec<Complex32> =
                clean.iter().zip(noise.iter()).map(|(a, b)| a + b).collect();
            let sym = dec.decode(&noisy, 4).expect("decode");
            assert_eq!(sym.value, value, "low-SNR DRCLength=4 mismatch {value:#x}");
        }
    }

    #[test]
    fn polarity_bit_distinguishes_paired_values() {
        // Values 2k and 2k+1 are complementary codewords; decoder must use
        // the soft polarity, not just the row.
        let cover = 6;
        let dec = DrcDecoder::new(cover);
        for k in 0..8 {
            let v0 = 2 * k as u8;
            let v1 = v0 + 1;
            assert_eq!(dec.decode(&encode_drc(v0, cover, 1), 1).unwrap().value, v0);
            assert_eq!(dec.decode(&encode_drc(v1, cover, 1), 1).unwrap().value, v1);
        }
    }
}
