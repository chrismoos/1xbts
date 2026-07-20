//! HRPD Rev 0 Reverse Access Channel long code mask.
//!
//! Spec reference: C.S0024-0 v4.0 §8.3.6.1.4.1.2 (Access Channel Long Code
//! Mask) and §9.2.1.3.8.2 (Long Codes / characteristic polynomial).
//!
//! The reverse access long code is a 42-stage maximal-length sequence with
//! the same characteristic polynomial as IS-95/1x. What is HRPD-specific is
//! the public mask layout for the access channel (M_IACMAC), which encodes a
//! fixed prefix, the AccessCycleNumber, and a fixed permutation of
//! `ColorCode | SectorID[23:0]`.

/// Characteristic polynomial for the HRPD reverse long-code generator
/// (C.S0024-0 v4.0 §9.2.1.3.8.2):
///
///   p(x) = x^42 + x^35 + x^33 + x^31 + x^27 + x^26 + x^25 + x^22 + x^21
///        + x^19 + x^18 + x^17 + x^16 + x^10 + x^7 + x^6 + x^5 + x^3
///        + x^2 + x + 1.
///
/// Encoded as a bitfield with bit `i` set iff the polynomial contains the
/// `x^i` term. The HRPD generator is identical to the cdma2000/IS-95 long-
/// code generator; what differs from 1x is the mask layout (see
/// `HrpdAccessLongCodeMask`).
pub const HRPD_REVERSE_LONG_CODE_POLY: u64 = (1u64 << 42)
    | (1u64 << 35)
    | (1u64 << 33)
    | (1u64 << 31)
    | (1u64 << 27)
    | (1u64 << 26)
    | (1u64 << 25)
    | (1u64 << 22)
    | (1u64 << 21)
    | (1u64 << 19)
    | (1u64 << 18)
    | (1u64 << 17)
    | (1u64 << 16)
    | (1u64 << 10)
    | (1u64 << 7)
    | (1u64 << 6)
    | (1u64 << 5)
    | (1u64 << 3)
    | (1u64 << 2)
    | (1u64 << 1)
    | 1u64;

/// Initial loading value of the long-code generator at the start of each
/// short-code period (C.S0024-0 v4.0 §9.2.1.3.8.2).
pub const HRPD_LONG_CODE_INITIAL_STATE: u64 = 0x0_24B9_1BFD_3A8;

/// Inputs that derive the 42-bit reverse access long-code I-mask
/// (M_IACMAC), per C.S0024-0 v4.0 §8.3.6.1.4.1.2 Table 8.3.6.1.4.1.2-1.
///
/// The mask is *not* a function of the AccessSignature — that field gates
/// the persistence test / probe selection but does not enter the long-code
/// mask in HRPD Rev 0. The mask is fully determined by:
///   * AccessCycleNumber = SystemTime (in slots) mod 256
///   * SectorID LSBs (only the lower 24 bits feed the permutation)
///   * ColorCode (8 bits, broadcast in the sector parameters)
#[derive(Debug, Clone, Copy)]
pub struct HrpdAccessLongCodeMask {
    /// AccessCycleNumber = SystemTime mod 256 (8 bits).
    pub access_cycle_number: u8,
    /// Lower 24 bits of SectorID for the target sector.
    pub sector_id_lsb: u32,
    /// ColorCode broadcast by the target sector (8 bits).
    pub color_code: u8,
}

impl HrpdAccessLongCodeMask {
    /// Pack the inputs into the 42-bit access long-code I-mask `M_IACMAC`.
    ///
    /// Layout per Table 8.3.6.1.4.1.2-1, with bit numbers as MSB...LSB
    /// (`MIACMAC[41]` is the most significant bit):
    ///
    /// ```text
    ///   [41:40]  = 0b11                                  (fixed prefix)
    ///   [39:32]  = AccessCycleNumber                     (8 bits)
    ///   [31:00]  = Permuted(ColorCode | SectorID[23:0])  (32 bits)
    /// ```
    ///
    /// The permutation is defined on the 32-bit word
    /// `S31 S30 ... S0 = ColorCode | SectorID[23:0]` where `S31..S24`
    /// are the ColorCode bits (MSB first) and `S23..S0` are
    /// `SectorID[23:0]` (MSB first), and the permuted output (MSB first,
    /// i.e. mapped to mask bits 31 down to 0) is:
    ///
    ///   `S0,  S31, S22, S13, S4,  S26, S17, S8,
    ///    S30, S21, S12, S3,  S25, S16, S7,  S29,
    ///    S20, S11, S2,  S24, S15, S6,  S28, S19,
    ///    S10, S1,  S23, S14, S5,  S27, S18, S9`.
    pub fn to_mask(&self) -> u64 {
        // Build the 32-bit S word: S31..S24 = ColorCode, S23..S0 = SectorID[23:0].
        let s: u32 = ((self.color_code as u32) << 24) | (self.sector_id_lsb & 0x00FF_FFFF);

        // Permutation source-bit indices in MSB-first output order, i.e.
        // PERM[0] feeds mask bit 31, PERM[1] feeds mask bit 30, ...,
        // PERM[31] feeds mask bit 0.
        const PERM: [u8; 32] = [
            0, 31, 22, 13, 4, 26, 17, 8, 30, 21, 12, 3, 25, 16, 7, 29, 20, 11, 2, 24, 15, 6, 28,
            19, 10, 1, 23, 14, 5, 27, 18, 9,
        ];

        let mut permuted: u32 = 0;
        for (out_pos_from_msb, src_bit) in PERM.iter().enumerate() {
            let bit = (s >> *src_bit) & 1;
            let out_bit_index = 31 - out_pos_from_msb as u32;
            permuted |= bit << out_bit_index;
        }

        let prefix: u64 = 0b11 << 40;
        let acn: u64 = (self.access_cycle_number as u64) << 32;
        prefix | acn | (permuted as u64)
    }
}

/// Derive the Q-mask `M_QACMAC` from the I-mask `M_IACMAC` per
/// C.S0024-0 v4.0 §8.3.6.1.4.1.2:
///
///   M_Q[k] = M_I[k-1] for k = 1..=41
///   M_Q[0] = XOR of M_I[i] for i in
///            {0,1,2,4,5,6,9,15,16,17,18,20,21,24,25,26,30,32,34,41}.
pub fn derive_q_mask(i_mask: u64) -> u64 {
    const XOR_TAPS: [u32; 20] = [
        0, 1, 2, 4, 5, 6, 9, 15, 16, 17, 18, 20, 21, 24, 25, 26, 30, 32, 34, 41,
    ];
    let mut q0: u64 = 0;
    for t in XOR_TAPS {
        q0 ^= (i_mask >> t) & 1;
    }
    let shifted = (i_mask & ((1u64 << 41) - 1)) << 1; // bits 0..40 of I -> bits 1..41 of Q
    (shifted & !1u64) | (q0 & 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polynomial_has_degree_42() {
        // Highest set bit corresponds to the leading x^42 term.
        let highest = 63 - HRPD_REVERSE_LONG_CODE_POLY.leading_zeros();
        assert_eq!(highest, 42, "polynomial leading term must be x^42");
        // Constant term `+1` must be present.
        assert_eq!(HRPD_REVERSE_LONG_CODE_POLY & 1, 1);
        // Exact bit pattern: every tap from the spec, no more, no less.
        let expected_taps: [u32; 21] = [
            42, 35, 33, 31, 27, 26, 25, 22, 21, 19, 18, 17, 16, 10, 7, 6, 5, 3, 2, 1, 0,
        ];
        let mut expected: u64 = 0;
        for t in expected_taps {
            expected |= 1u64 << t;
        }
        assert_eq!(HRPD_REVERSE_LONG_CODE_POLY, expected);
    }

    #[test]
    fn mask_is_42_bits_wide() {
        let m = HrpdAccessLongCodeMask {
            access_cycle_number: 0xFF,
            sector_id_lsb: 0x00FF_FFFF,
            color_code: 0xFF,
        }
        .to_mask();
        assert_eq!(m >> 42, 0, "mask must not exceed 42 bits");
    }

    #[test]
    fn mask_layout_matches_spec_table() {
        // Choose ColorCode = 0xA5 and SectorID[23:0] = 0x5A_C3_3C so that
        //   S31..S24 = 0xA5  = 1010 0101
        //   S23..S0  = 0x5A_C3_3C = 0101_1010 1100_0011 0011_1100
        // and AccessCycleNumber = 0x42.
        let mask = HrpdAccessLongCodeMask {
            access_cycle_number: 0x42,
            sector_id_lsb: 0x005A_C33C,
            color_code: 0xA5,
        }
        .to_mask();

        // Fixed prefix bits 41 and 40 must both be 1.
        assert_eq!((mask >> 41) & 1, 1, "bit 41 must be 1");
        assert_eq!((mask >> 40) & 1, 1, "bit 40 must be 1");

        // AccessCycleNumber occupies bits [39:32].
        let acn_field = (mask >> 32) & 0xFF;
        assert_eq!(acn_field, 0x42);

        // Permuted word in low 32 bits.
        let permuted = (mask & 0xFFFF_FFFF) as u32;

        // Reconstruct S = ColorCode | SectorID[23:0] and verify the
        // permutation bit-by-bit against the table:
        //   output MSB (mask bit 31) = S0, then S31, S22, S13, S4, S26, ...
        let s: u32 = (0xA5u32 << 24) | 0x005A_C33C;
        let perm: [u8; 32] = [
            0, 31, 22, 13, 4, 26, 17, 8, 30, 21, 12, 3, 25, 16, 7, 29, 20, 11, 2, 24, 15, 6, 28,
            19, 10, 1, 23, 14, 5, 27, 18, 9,
        ];
        for (i, src) in perm.iter().enumerate() {
            let expected_bit = (s >> *src) & 1;
            let actual_bit = (permuted >> (31 - i as u32)) & 1;
            assert_eq!(
                actual_bit, expected_bit,
                "permutation mismatch at output position {i} (src S{src})",
            );
        }
    }

    #[test]
    fn mask_with_zero_inputs_is_just_prefix() {
        let m = HrpdAccessLongCodeMask {
            access_cycle_number: 0,
            sector_id_lsb: 0,
            color_code: 0,
        }
        .to_mask();
        assert_eq!(m, 0b11u64 << 40);
    }

    #[test]
    fn q_mask_shifts_i_mask_and_xors_taps() {
        // Pick an I-mask with known bits set and verify Q-mask construction.
        let i_mask: u64 = (1u64 << 41) | (1u64 << 5) | (1u64 << 0);
        let q = derive_q_mask(i_mask);
        // Bits [1..=41] of Q are bits [0..=40] of I.
        // I bit 0 -> Q bit 1; I bit 5 -> Q bit 6; I bit 41 is dropped from the shift.
        assert_eq!((q >> 1) & 1, 1);
        assert_eq!((q >> 6) & 1, 1);
        assert_eq!((q >> 42) & 1, 0, "Q must not exceed 42 bits");
        // Q[0] = XOR of I[taps]. Taps with I set: 0, 5, 41.
        // 1 ^ 1 ^ 1 = 1.
        assert_eq!(q & 1, 1);
    }
}
