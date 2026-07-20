//! HRPD Subtype 2 Physical Layer reverse RRI Channel encoder/decoder.
//!
//! Spec references (C.S0024-A v3.0):
//! - §13.2.1.3.3.2 Reverse Rate Indicator Channel: the 6-bit RRI symbol is a
//!   4-bit payload index plus a 2-bit sub-packet index, encoded on a
//!   32-dimensional bi-orthogonal signal constellation. The codeword for
//!   payload index `i` and sub-packet index `j` is Walsh function
//!   `W_(2i+floor(j/2))^32`, complemented when `j` is odd. The codeword is
//!   repeated four times, signal-point mapped (0 -> +1, 1 -> -1), covered by
//!   `W_4^16`, and transmitted on the I channel. Payload index 0x0 with
//!   sub-packet index 0 is the null-rate RRI.
//! - Table 13.2.1.3.3.2-1 Payload Size to Payload Index Mapping.
//! - Table 13.2.1.3.3.2-2 Sub-packet Identifier to Sub-packet Index Mapping
//!   (identity).
//! - §13.2.1.3.1 / Table 13.2.1.3.1-1: the RRI Channel is transmitted
//!   continuously on Walsh channel `W_4^16` while the Reverse Traffic Channel
//!   is active, and a sub-frame is 4 slots of 2048 chips.
//!
//! Each codeword symbol spans one `W_4^16` cover period (16 chips), so the
//! 32-symbol codeword spans 512 chips and its four repeats fill exactly one
//! slot; the slot pattern repeats across the sub-frame (16 codeword passes
//! per 8192-chip sub-frame). This was verified against a live AT capture:
//! decoding with 64-chip symbols instead sums 4 consecutive codeword symbols,
//! which algebraically zeroes every Walsh row not divisible by 4 — the null
//! (row 0) still decodes, but real rates (e.g. rows 2 and 6) vanish, so a
//! wrong symbol duration is invisible to null-only and loopback tests.
//!
//! This differs from the Rev 0 RRI in [`super::rri_processor`], which is a
//! 3-bit simplex codeword TDM'd onto the pilot's `W_0^16` slot head.

use num::complex::Complex32;

use crate::phy::walsh::WalshGenerator;

use super::despread::HRPD_SLOT_CHIPS;

/// Bi-orthogonal codeword length (§13.2.1.3.3.2).
pub const RRI_SUBTYPE2_CODEWORD_SYMBOLS: usize = 32;
/// Codeword repeats per slot (§13.2.1.3.3.2: the codeword is repeated four
/// times; 4 × 512 chips fills one 2048-chip slot).
pub const RRI_SUBTYPE2_CODEWORD_REPEATS_PER_SLOT: usize = 4;
/// Codeword repeats per 4-slot sub-frame.
pub const RRI_SUBTYPE2_CODEWORD_REPEATS: usize =
    RRI_SUBTYPE2_CODEWORD_REPEATS_PER_SLOT * HRPD_SUBFRAME_SLOTS;
/// RRI symbols per 4-slot sub-frame after repetition.
pub const RRI_SUBTYPE2_SUBFRAME_SYMBOLS: usize =
    RRI_SUBTYPE2_CODEWORD_SYMBOLS * RRI_SUBTYPE2_CODEWORD_REPEATS;
/// Walsh index `i` in `W_i^16` for the RRI cover (Table 13.2.1.3.1-1).
pub const RRI_SUBTYPE2_WALSH_COVER_INDEX: usize = 4;
/// RRI Walsh cover length (`W_4^16`).
pub const RRI_SUBTYPE2_WALSH_COVER_LEN: usize = 16;
/// Slots per Subtype 2 reverse sub-frame (§13.2.1.3.1).
pub const HRPD_SUBFRAME_SLOTS: usize = 4;
/// Chips per Subtype 2 reverse sub-frame (4 slots × 2048 chips).
pub const HRPD_SUBFRAME_CHIPS: usize = HRPD_SUBFRAME_SLOTS * HRPD_SLOT_CHIPS;
/// Chips per codeword symbol: one `W_4^16` cover period.
pub const RRI_SUBTYPE2_CHIPS_PER_SYMBOL: usize =
    HRPD_SUBFRAME_CHIPS / RRI_SUBTYPE2_SUBFRAME_SYMBOLS;

/// Null-rate RRI payload index (§13.2.1.3.3.2, with sub-packet index 0).
pub const RRI_SUBTYPE2_NULL_PAYLOAD_INDEX: u8 = 0x0;
/// Null-rate RRI sub-packet index (§13.2.1.3.3.2).
pub const RRI_SUBTYPE2_NULL_SUBPACKET_ID: u8 = 0;
/// Highest assigned payload index; 0xd..0xf are reserved
/// (Table 13.2.1.3.3.2-1).
pub const RRI_SUBTYPE2_MAX_PAYLOAD_INDEX: u8 = 0xc;
/// Number of sub-packet identifiers (Table 13.2.1.3.3.2-2).
pub const RRI_SUBTYPE2_SUBPACKET_IDS: u8 = 4;

/// Payload index → payload size in bits (Table 13.2.1.3.3.2-1). Index 0x0 is
/// the null rate. Note this is offset by one from the Subtype 2 scrambler's
/// payload-size code in `data_decoder`, which has no null entry.
pub const RRI_SUBTYPE2_PAYLOAD_BITS: [u32; (RRI_SUBTYPE2_MAX_PAYLOAD_INDEX + 1) as usize] = [
    0, 128, 256, 512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288,
];

/// Decoded Subtype 2 RRI symbol for one sub-frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RriSubtype2Detection {
    /// 4-bit payload index (0x0 = null rate) per Table 13.2.1.3.3.2-1.
    pub payload_index: u8,
    /// 2-bit sub-packet identifier per Table 13.2.1.3.3.2-2.
    pub subpacket_id: u8,
    /// Payload size in bits for `payload_index` (0 for null rate).
    pub payload_bits: u32,
    /// Soft correlation of the winning codeword hypothesis.
    pub best_score: f32,
    /// Soft correlation of the runner-up hypothesis.
    pub second_score: f32,
    /// `best_score - second_score` across all 49 valid hypotheses
    /// (13 payload indices × 4 sub-packet IDs minus the 3 illegal
    /// null-rate sub-packets).
    pub margin: f32,
}

/// True for the one legal null-rate RRI symbol. Payload index 0 with any other
/// sub-packet ID is not a legal idle indication and must not clear HARQ state.
pub fn is_rri_subtype2_null(detection: &RriSubtype2Detection) -> bool {
    detection.payload_index == RRI_SUBTYPE2_NULL_PAYLOAD_INDEX
        && detection.subpacket_id == RRI_SUBTYPE2_NULL_SUBPACKET_ID
}

pub fn is_rri_subtype2_valid(detection: &RriSubtype2Detection) -> bool {
    detection.payload_index <= RRI_SUBTYPE2_MAX_PAYLOAD_INDEX
        && detection.subpacket_id < RRI_SUBTYPE2_SUBPACKET_IDS
        && (detection.payload_index != RRI_SUBTYPE2_NULL_PAYLOAD_INDEX
            || detection.subpacket_id == RRI_SUBTYPE2_NULL_SUBPACKET_ID)
}

pub fn rri_subtype2_payload_index_for_bits(payload_bits: u32) -> Option<u8> {
    RRI_SUBTYPE2_PAYLOAD_BITS
        .iter()
        .position(|&bits| bits == payload_bits)
        .map(|idx| idx as u8)
}

/// ±1 signal points of the 32-symbol bi-orthogonal codeword for
/// (`payload_index`, `subpacket_id`) per the §13.2.1.3.3.2 Walsh equations.
pub fn rri_subtype2_codeword(
    payload_index: u8,
    subpacket_id: u8,
) -> [f32; RRI_SUBTYPE2_CODEWORD_SYMBOLS] {
    assert!(
        payload_index <= RRI_SUBTYPE2_MAX_PAYLOAD_INDEX,
        "payload index must be in 0x0..=0xc (got {payload_index:#x})"
    );
    assert!(
        subpacket_id < RRI_SUBTYPE2_SUBPACKET_IDS,
        "sub-packet ID must be in 0..=3 (got {subpacket_id})"
    );
    let row = 2 * payload_index as usize + (subpacket_id / 2) as usize;
    let sign = if subpacket_id % 2 == 0 { 1.0 } else { -1.0 };
    let matrix = WalshGenerator::generate_matrix::<RRI_SUBTYPE2_CODEWORD_SYMBOLS>();
    std::array::from_fn(|symbol| sign * f32::from(matrix[row][symbol]))
}

/// Post-Walsh-cover I-arm chips for one sub-frame of RRI
/// (`HRPD_SUBFRAME_CHIPS` chips at 1.2288 Mcps). Same post-despread chip
/// convention consumed by the other reverse-traffic sub-chain processors.
pub fn encode_rri_subtype2_subframe(payload_index: u8, subpacket_id: u8) -> Vec<Complex32> {
    let codeword = rri_subtype2_codeword(payload_index, subpacket_id);
    let cover =
        WalshGenerator::new::<RRI_SUBTYPE2_WALSH_COVER_LEN>(RRI_SUBTYPE2_WALSH_COVER_INDEX, 1);
    let cover_row = cover.code().to_vec();
    (0..HRPD_SUBFRAME_CHIPS)
        .map(|chip_idx| {
            let symbol = codeword
                [(chip_idx / RRI_SUBTYPE2_CHIPS_PER_SYMBOL) % RRI_SUBTYPE2_CODEWORD_SYMBOLS];
            let chip = f32::from(cover_row[chip_idx % RRI_SUBTYPE2_WALSH_COVER_LEN]);
            Complex32::new(symbol * chip, 0.0)
        })
        .collect()
}

/// `W_4^16`-decover one sub-frame of despread chips into the 512 per-symbol
/// soft values (I arm), normalized so a clean unit-amplitude symbol is ±1.
/// Returns `None` if fewer than `HRPD_SUBFRAME_CHIPS` chips are supplied.
pub fn rri_subtype2_soft_symbols(
    chips: &[Complex32],
) -> Option<[f32; RRI_SUBTYPE2_SUBFRAME_SYMBOLS]> {
    if chips.len() < HRPD_SUBFRAME_CHIPS {
        return None;
    }
    let cover =
        WalshGenerator::new::<RRI_SUBTYPE2_WALSH_COVER_LEN>(RRI_SUBTYPE2_WALSH_COVER_INDEX, 1);
    let cover_row = cover.code().to_vec();
    let mut soft = [0.0f32; RRI_SUBTYPE2_SUBFRAME_SYMBOLS];
    for (symbol_idx, soft_symbol) in soft.iter_mut().enumerate() {
        let base = symbol_idx * RRI_SUBTYPE2_CHIPS_PER_SYMBOL;
        let window = &chips[base..base + RRI_SUBTYPE2_CHIPS_PER_SYMBOL];
        let acc: f32 = window
            .iter()
            .enumerate()
            .map(|(offset, chip)| {
                chip.re * f32::from(cover_row[offset % RRI_SUBTYPE2_WALSH_COVER_LEN])
            })
            .sum();
        *soft_symbol = acc / RRI_SUBTYPE2_CHIPS_PER_SYMBOL as f32;
    }
    Some(soft)
}

/// Soft-decode one sub-frame's decovered RRI symbols by correlating against
/// all legal codewords: the one null-rate symbol plus 12 payload indices × 4
/// sub-packet IDs. Reserved payload indices 0xd..0xf and invalid null
/// sub-packets are never hypothesized.
pub fn decode_rri_subtype2(
    soft_symbols: &[f32; RRI_SUBTYPE2_SUBFRAME_SYMBOLS],
) -> RriSubtype2Detection {
    let mut folded = [0.0f32; RRI_SUBTYPE2_CODEWORD_SYMBOLS];
    for (idx, &soft) in soft_symbols.iter().enumerate() {
        folded[idx % RRI_SUBTYPE2_CODEWORD_SYMBOLS] += soft;
    }
    let matrix = WalshGenerator::generate_matrix::<RRI_SUBTYPE2_CODEWORD_SYMBOLS>();
    let hypotheses =
        usize::from(RRI_SUBTYPE2_MAX_PAYLOAD_INDEX + 1) * usize::from(RRI_SUBTYPE2_SUBPACKET_IDS);
    let mut scored = Vec::with_capacity(hypotheses);
    for payload_index in 0..=RRI_SUBTYPE2_MAX_PAYLOAD_INDEX {
        for half in 0..2u8 {
            let row = 2 * payload_index as usize + half as usize;
            let corr: f32 = matrix[row]
                .iter()
                .zip(folded.iter())
                .map(|(&chip, &soft)| f32::from(chip) * soft)
                .sum();
            // Even sub-packet indices carry the plain Walsh row, odd ones the
            // complement (§13.2.1.3.3.2 equations).
            let even_subpacket = 2 * half;
            if payload_index != RRI_SUBTYPE2_NULL_PAYLOAD_INDEX
                || even_subpacket == RRI_SUBTYPE2_NULL_SUBPACKET_ID
            {
                scored.push((payload_index, even_subpacket, corr));
            }
            let odd_subpacket = 2 * half + 1;
            if payload_index != RRI_SUBTYPE2_NULL_PAYLOAD_INDEX
                || odd_subpacket == RRI_SUBTYPE2_NULL_SUBPACKET_ID
            {
                scored.push((payload_index, odd_subpacket, -corr));
            }
        }
    }
    scored.sort_by(|a, b| b.2.total_cmp(&a.2));
    let (payload_index, subpacket_id, best_score) = scored[0];
    let second_score = scored[1].2;
    RriSubtype2Detection {
        payload_index,
        subpacket_id,
        payload_bits: RRI_SUBTYPE2_PAYLOAD_BITS[payload_index as usize],
        best_score,
        second_score,
        margin: best_score - second_score,
    }
}

/// Decode one sub-frame of despread chips (decover + codeword search).
pub fn decode_rri_subtype2_subframe(chips: &[Complex32]) -> Option<RriSubtype2Detection> {
    rri_subtype2_soft_symbols(chips).map(|soft| decode_rri_subtype2(&soft))
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
    fn constants_match_spec_13_2_1_3_3_2() {
        assert_eq!(RRI_SUBTYPE2_CODEWORD_SYMBOLS, 32);
        assert_eq!(RRI_SUBTYPE2_CODEWORD_REPEATS_PER_SLOT, 4);
        assert_eq!(RRI_SUBTYPE2_CODEWORD_REPEATS, 16);
        assert_eq!(RRI_SUBTYPE2_SUBFRAME_SYMBOLS, 512);
        assert_eq!(HRPD_SUBFRAME_CHIPS, 8192);
        // One codeword symbol spans one W_4^16 cover period.
        assert_eq!(RRI_SUBTYPE2_CHIPS_PER_SYMBOL, RRI_SUBTYPE2_WALSH_COVER_LEN);
        // Four codeword repeats span exactly one slot.
        assert_eq!(
            RRI_SUBTYPE2_CODEWORD_REPEATS_PER_SLOT
                * RRI_SUBTYPE2_CODEWORD_SYMBOLS
                * RRI_SUBTYPE2_CHIPS_PER_SYMBOL,
            HRPD_SLOT_CHIPS
        );
    }

    #[test]
    fn payload_mapping_matches_table_13_2_1_3_3_2_1() {
        let expected: [(u8, u32); 13] = [
            (0x0, 0),
            (0x1, 128),
            (0x2, 256),
            (0x3, 512),
            (0x4, 768),
            (0x5, 1024),
            (0x6, 1536),
            (0x7, 2048),
            (0x8, 3072),
            (0x9, 4096),
            (0xa, 6144),
            (0xb, 8192),
            (0xc, 12288),
        ];
        for (index, bits) in expected {
            assert_eq!(
                RRI_SUBTYPE2_PAYLOAD_BITS[index as usize], bits,
                "Table 13.2.1.3.3.2-1 mismatch at payload index {index:#x}"
            );
            assert_eq!(rri_subtype2_payload_index_for_bits(bits), Some(index));
        }
        assert_eq!(rri_subtype2_payload_index_for_bits(64), None);
    }

    #[test]
    fn walsh_cover_matches_figure_13_2_1_3_1_2() {
        // W_4^16 = (+ + + + − − − − + + + + − − − −) per Figure 13.2.1.3.1-2.
        let cover =
            WalshGenerator::new::<RRI_SUBTYPE2_WALSH_COVER_LEN>(RRI_SUBTYPE2_WALSH_COVER_INDEX, 1);
        let expected: [i8; RRI_SUBTYPE2_WALSH_COVER_LEN] =
            [1, 1, 1, 1, -1, -1, -1, -1, 1, 1, 1, 1, -1, -1, -1, -1];
        assert_eq!(cover.code(), expected.as_slice());
    }

    #[test]
    fn codeword_vectors_match_spec_equations_13_2_1_3_3_2() {
        // Null rate (payload 0x0, sub-packet 0) is W_0^32: all binary 0,
        // signal points all +1.
        assert_eq!(
            rri_subtype2_codeword(RRI_SUBTYPE2_NULL_PAYLOAD_INDEX, 0),
            [1.0; RRI_SUBTYPE2_CODEWORD_SYMBOLS]
        );
        // (i=1, j=0) → W_2^32 = (+ + − −) tiled.
        let w2: [f32; RRI_SUBTYPE2_CODEWORD_SYMBOLS] =
            std::array::from_fn(|s| [1.0, 1.0, -1.0, -1.0][s % 4]);
        assert_eq!(rri_subtype2_codeword(1, 0), w2);
        // (i=1, j=1) → complement of W_2^32 (odd sub-packet index).
        let w2c: [f32; RRI_SUBTYPE2_CODEWORD_SYMBOLS] = std::array::from_fn(|s| -w2[s]);
        assert_eq!(rri_subtype2_codeword(1, 1), w2c);
        // (i=1, j=2) → W_3^32 = (+ − − +) tiled.
        let w3: [f32; RRI_SUBTYPE2_CODEWORD_SYMBOLS] =
            std::array::from_fn(|s| [1.0, -1.0, -1.0, 1.0][s % 4]);
        assert_eq!(rri_subtype2_codeword(1, 2), w3);
        // (i=1, j=3) → complement of W_3^32.
        let w3c: [f32; RRI_SUBTYPE2_CODEWORD_SYMBOLS] = std::array::from_fn(|s| -w3[s]);
        assert_eq!(rri_subtype2_codeword(1, 3), w3c);
    }

    #[test]
    fn round_trip_all_pairs_clean() {
        for payload_index in 0..=RRI_SUBTYPE2_MAX_PAYLOAD_INDEX {
            for subpacket_id in 0..RRI_SUBTYPE2_SUBPACKET_IDS {
                if payload_index == RRI_SUBTYPE2_NULL_PAYLOAD_INDEX
                    && subpacket_id != RRI_SUBTYPE2_NULL_SUBPACKET_ID
                {
                    continue;
                }
                let chips = encode_rri_subtype2_subframe(payload_index, subpacket_id);
                assert_eq!(chips.len(), HRPD_SUBFRAME_CHIPS);
                let det = decode_rri_subtype2_subframe(&chips).expect("decode");
                assert_eq!(
                    (det.payload_index, det.subpacket_id),
                    (payload_index, subpacket_id),
                    "round trip mismatch for ({payload_index:#x}, {subpacket_id})"
                );
                assert_eq!(
                    det.payload_bits,
                    RRI_SUBTYPE2_PAYLOAD_BITS[payload_index as usize]
                );
                // A clean codeword is orthogonal to every other-row hypothesis
                // and antipodal to its complement, so the runner-up score is 0
                // and the margin equals the peak.
                assert!(
                    det.margin > 0.9 * det.best_score,
                    "weak clean margin for ({payload_index:#x}, {subpacket_id}): {det:?}"
                );
            }
        }
    }

    #[test]
    fn decodes_under_noise() {
        // sigma = 4.0 per component is about -15 dB SNR per chip; the
        // 8192-chip coherent integration leaves ~20 sigma of decision margin.
        let sigma = 4.0f32;
        for payload_index in 0..=RRI_SUBTYPE2_MAX_PAYLOAD_INDEX {
            for subpacket_id in 0..RRI_SUBTYPE2_SUBPACKET_IDS {
                if payload_index == RRI_SUBTYPE2_NULL_PAYLOAD_INDEX
                    && subpacket_id != RRI_SUBTYPE2_NULL_SUBPACKET_ID
                {
                    continue;
                }
                let clean = encode_rri_subtype2_subframe(payload_index, subpacket_id);
                let seed = 0x5EED_1234 ^ (u32::from(payload_index) << 8) ^ u32::from(subpacket_id);
                let noise = noise_seq(seed, clean.len(), sigma);
                let noisy: Vec<Complex32> =
                    clean.iter().zip(noise.iter()).map(|(a, b)| a + b).collect();
                let det = decode_rri_subtype2_subframe(&noisy).expect("decode");
                assert_eq!(
                    (det.payload_index, det.subpacket_id),
                    (payload_index, subpacket_id),
                    "noisy mismatch for ({payload_index:#x}, {subpacket_id}): {det:?}"
                );
                assert!(det.margin > 0.0, "non-positive noisy margin: {det:?}");
            }
        }
    }

    #[test]
    fn decoder_never_returns_invalid_null_subpacket() {
        for subpacket_id in 1..RRI_SUBTYPE2_SUBPACKET_IDS {
            let chips = encode_rri_subtype2_subframe(RRI_SUBTYPE2_NULL_PAYLOAD_INDEX, subpacket_id);
            let det = decode_rri_subtype2_subframe(&chips).expect("decode");
            assert!(
                is_rri_subtype2_valid(&det),
                "decoder selected invalid null subpacket from reserved codeword: {det:?}"
            );
            assert_ne!(
                (det.payload_index, det.subpacket_id),
                (RRI_SUBTYPE2_NULL_PAYLOAD_INDEX, subpacket_id),
                "reserved null subpacket must not be a decode hypothesis"
            );
        }
    }

    #[test]
    fn short_input_returns_none() {
        let chips = vec![Complex32::new(1.0, 0.0); HRPD_SUBFRAME_CHIPS - 1];
        assert!(decode_rri_subtype2_subframe(&chips).is_none());
        assert!(rri_subtype2_soft_symbols(&chips).is_none());
    }
}
