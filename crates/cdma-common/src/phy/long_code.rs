/// Long code generator for CDMA2000 as specified in C.S0002-E v1.0
///
/// The long code is generated using a 42-bit linear feedback shift register (LFSR)
/// with taps defined by the characteristic polynomial:
/// p(x) = x^42 + x^35 + x^33 + x^31 + x^27 + x^26 + x^25 + x^22 + x^21 + x^19 + x^18 + x^17 + x^16 + x^10 + x^7 + x^6 + x^5 + x^3 + x^2 + x^1 + 1

#[derive(Clone)]
pub struct LongCodeGenerator {
    state: u64,
    mask: u64,
}

impl LongCodeGenerator {
    pub fn new(mask: u64) -> Self {
        Self {
            // Initial state per spec: 42nd bit = 1, all others = 0
            state: 1u64 << 41,
            mask,
        }
    }

    pub fn new_access_channel(acn: u8, pcn: u8, base_id: u16, pilot_pn: u16) -> Self {
        // Access channel long code mask (C.S0002-E 2.1.3.1.16-2):
        // M41-M33: 110001111
        // M32-M28: ACN (Access Channel Number)
        // M27-M25: PCN (Paging Channel Number)
        // M24-M9:  BASE_ID
        // M8-M0:   PILOT_PN
        let fixed_pattern = 0b110001111u64; // 9 bits
        let mask = (fixed_pattern << 33)
            | ((acn as u64 & 0x1f) << 28)
            | ((pcn as u64 & 0x7) << 25)
            | ((base_id as u64 & 0xffff) << 9)
            | (pilot_pn as u64 & 0x1ff);
        Self::new(mask)
    }

    pub fn new_access_channel_with_state(
        acn: u8,
        pcn: u8,
        base_id: u16,
        pilot_pn: u16,
        state: u64,
    ) -> Self {
        let mut generator = Self::new_access_channel(acn, pcn, base_id, pilot_pn);
        generator.state = state;
        generator
    }

    pub fn new_paging_channel(pcn: u8, pilot_pn: u16) -> Self {
        // Paging channel long code mask:
        // M41-M29: 1100011001101
        // M28-M24: 00000
        // M23-M21: PCN (Paging Channel Number)
        // M20-M9: 000000000000
        // M8-M0: PILOT_PN
        let fixed_pattern = 0b1100011001101u64; // 13 bits
        let mask = (fixed_pattern << 29) | ((pcn as u64 & 0x7) << 21) | (pilot_pn as u64 & 0x1FF);
        Self::new(mask)
    }

    pub fn new_paging_channel_with_state(pcn: u8, pilot_pn: u16, state: u64) -> Self {
        let mut generator = Self::new_paging_channel(pcn, pilot_pn);
        generator.state = state;
        generator
    }

    /// Traffic channel long code mask (PLCM_TYPE = 0000, ESN-based).
    ///
    /// Per C.S0005-E Section 2.3.6.1.1 and C.S0002-E Table 2.1.3.1.15-1:
    /// - M41..M37 = 11000 (fixed prefix for PLCM_42)
    /// - M36..M32 = 11000 (fixed prefix for PLCM_37)
    /// - M31..M0  = permuted ESN (32 bits)
    ///
    /// The top 10 bits are always 0b11000_11000 = 0x318 << 32.
    /// Default for all P_REV < 11 mobile stations.
    pub fn new_traffic_channel(esn: u32) -> Self {
        let permuted = Self::permute_esn(esn);
        let mask = 0x318_0000_0000u64 | (permuted as u64);
        Self::new(mask)
    }

    pub fn new_traffic_channel_with_state(esn: u32, state: u64) -> Self {
        let mut generator = Self::new_traffic_channel(esn);
        generator.state = state;
        generator
    }

    /// Traffic channel long code mask with a raw 42-bit PLCM value.
    ///
    /// Use this when you already have the full PLCM_42 mask computed externally.
    pub fn new_traffic_channel_raw_mask(mask: u64) -> Self {
        Self::new(mask & 0x3FF_FFFF_FFFF)
    }

    /// Traffic channel long code mask (PLCM_TYPE = 0001, IMSI_O_S-based).
    ///
    /// Per C.S0005-E Table 2.3.6.1.1-1:
    /// - PLCM_42 = 11000 || PLCM_37
    /// - For type 0001: PLCM_37 = 00001 || IMSI_O_S[31:0]
    ///
    /// `imsi_s` is the 34-bit IMSI_S value; only the lower 32 bits are used.
    pub fn new_traffic_channel_imsi_s(imsi_s: u64) -> Self {
        // M41..M37 = 11000, M36..M32 = 00001, M31..M0 = IMSI_O_S[31:0]
        let mask = 0x301_0000_0000u64 | (imsi_s & 0xFFFF_FFFF);
        Self::new(mask)
    }

    /// Alternate IMSI PLCM where all 34 bits of IMSI_S are used.
    ///
    /// PLCM_42 = 11000 || 001 || IMSI_S[33:0]
    pub fn new_traffic_channel_imsi_s_34bit(imsi_s: u64) -> Self {
        // M41..M37 = 11000, M36..M34 = 001, M33..M0 = IMSI_S[33:0]
        let mask = 0x304_0000_0000u64 | (imsi_s & 0x3_FFFF_FFFF);
        Self::new(mask)
    }

    /// Permute ESN bits per C.S0005-E Section 2.3.6.1.1.
    ///
    /// The spec lists the permuted ESN as (MSB first):
    ///   (E0, E31, E22, E13, E4, E26, E17, E8, E30, E21, E12, E3,
    ///    E25, E16, E7, E29, E20, E11, E2, E24, E15, E6, E28, E19,
    ///    E10, E1, E23, E14, E5, E27, E18, E9)
    ///
    /// PERM[i] = source ESN bit index for output bit i (LSB = bit 0).
    pub fn permute_esn(esn: u32) -> u32 {
        // Index = output bit position (0=LSB M0, 31=MSB M31)
        // Value = source ESN bit position
        const PERM: [u8; 32] = [
            9, 18, 27, 5, 14, 23, 1, 10, // M0..M7
            19, 28, 6, 15, 24, 2, 11, 20, // M8..M15
            29, 7, 16, 25, 3, 12, 21, 30, // M16..M23
            8, 17, 26, 4, 13, 22, 31, 0, // M24..M31
        ];
        let mut result = 0u32;
        for (out_bit, &src_bit) in PERM.iter().enumerate() {
            if (esn >> src_bit) & 1 == 1 {
                result |= 1 << out_bit;
            }
        }
        result
    }

    pub fn next_chip(&mut self) -> u8 {
        let chip = self.peek_output();
        self.advance_state();
        chip
    }

    pub fn state(&self) -> u64 {
        self.state
    }

    pub fn set_state(&mut self, state: u64) {
        self.state = state;
    }

    pub fn mask(&self) -> u64 {
        self.mask
    }

    pub fn set_mask(&mut self, mask: u64) {
        self.mask = mask;
    }

    fn peek_output(&self) -> u8 {
        let masked = self.state & self.mask;
        (masked.count_ones() & 1) as u8
    }

    fn advance_state(&mut self) {
        // Figure 2.1.3.1.16-1 is a 42-stage Galois LFSR:
        // - feedback bit is the stage-42 output
        // - register shifts toward higher stage number (left shift in this bit layout)
        // - feedback is XOR-injected at stages corresponding to polynomial terms x^1..x^35
        //   (excluding x^42 and constant 1)
        let feedback = ((self.state >> 41) & 1) as u64;
        self.state = ((self.state << 1) | feedback) & 0x3FF_FFFF_FFFF;

        if feedback == 1 {
            // Toggle bit positions for x^1, x^2, x^3, x^5, x^6, x^7, x^10, x^16, x^17, x^18,
            // x^19, x^21, x^22, x^25, x^26, x^27, x^31, x^33, x^35.
            self.state ^= (1u64 << 1)
                | (1u64 << 2)
                | (1u64 << 3)
                | (1u64 << 5)
                | (1u64 << 6)
                | (1u64 << 7)
                | (1u64 << 10)
                | (1u64 << 16)
                | (1u64 << 17)
                | (1u64 << 18)
                | (1u64 << 19)
                | (1u64 << 21)
                | (1u64 << 22)
                | (1u64 << 25)
                | (1u64 << 26)
                | (1u64 << 27)
                | (1u64 << 31)
                | (1u64 << 33)
                | (1u64 << 35);
        }
    }

    pub fn advance_chips(&mut self, mut num_chips: usize) {
        if num_chips >= 64 {
            // Use matrix-based jump for large deltas
            let mut delta = num_chips as u64;
            let mut current_matrix = Self::get_t_matrix();
            while delta > 0 {
                if delta & 1 == 1 {
                    self.apply_matrix(&current_matrix);
                }
                current_matrix = Self::multiply_matrices(&current_matrix, &current_matrix);
                delta >>= 1;
            }
        } else {
            while num_chips > 0 {
                self.advance_state();
                num_chips -= 1;
            }
        }
    }

    fn get_t_matrix() -> [u64; 42] {
        let mut matrix = [0u64; 42];

        for i in 0..42 {
            let mut next_row = 0u64;
            if i < 41 {
                next_row |= 1 << (i + 1);
            }

            // In the Galois form, only stage-42 output (bit 41) is the feedback source.
            if i == 41 {
                // Shifted-in feedback to stage 1 (bit 0).
                next_row |= 1;

                // XOR-injection points from polynomial terms (excluding x^42 and +1).
                for bit in [
                    1usize, 2, 3, 5, 6, 7, 10, 16, 17, 18, 19, 21, 22, 25, 26, 27, 31, 33, 35,
                ] {
                    next_row |= 1 << bit;
                }
            }
            matrix[i] = next_row;
        }

        matrix
    }

    fn apply_matrix(&mut self, matrix: &[u64; 42]) {
        let mut new_state = 0u64;
        for i in 0..42 {
            if (self.state >> i) & 1 == 1 {
                new_state ^= matrix[i];
            }
        }
        self.state = new_state;
    }

    fn multiply_matrices(a: &[u64; 42], b: &[u64; 42]) -> [u64; 42] {
        let mut result = [0u64; 42];
        for i in 0..42 {
            let mut row = 0u64;
            for j in 0..42 {
                if (a[i] >> j) & 1 == 1 {
                    row ^= b[j];
                }
            }
            result[i] = row;
        }
        result
    }

    pub fn generate_sequence(&mut self, num_chips: usize) -> Vec<u8> {
        (0..num_chips).map(|_| self.next_chip()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_long_code() {
        /*
                2026-03-03T05:25:22.475571Z DEBUG cdma_bts::receiver::tests: sync_pdu_lc_state: 0x38e9ecec314 sys_time: 17861676480
        2026-03-03T05:25:22.515662Z DEBUG cdma_bts::receiver::tests: sync_lc_state_mismatch: expected=0x18b290d593f observed=0x38e9ecec314 delta_chips=294912 delta_sys_time=3
        2026-03-03T05:25:22.515668Z DEBUG cdma_bts::receiver::tests: discard frame
        2026-03-03T05:25:22.515670Z DEBUG cdma_bts::receiver::tests: message length: 224
        2026-03-03T05:25:22.515672Z DEBUG cdma_bts::receiver::tests: crc:   110100011010100111001101010111, exepcted:   110100011010100111001101010111
        2026-03-03T05:25:22.515673Z DEBUG cdma_bts::receiver::tests: CRC GOOD
        2026-03-03T05:25:22.515674Z DEBUG cdma_bts::receiver::tests: got frame: [0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0, 1, 1, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        2026-03-03T05:25:22.515688Z DEBUG cdma_bts::receiver::tests: sync_pdu_lc_state: 0x3b10f0437f3 sys_time: 17861676483
        2026-03-03T05:25:22.555673Z DEBUG cdma_bts::receiver::tests: sync_lc_state_mismatch: expected=0x31a1616b89c observed=0x3b10f0437f3 delta_chips=294912 delta_sys_time=3
         */
        // Two consecutive sync messages from a live capture.
        // SYS_TIME (sync msg field) incremented by 3; that field is in 80ms
        // units, so 3 × 80ms = 240ms = 294,912 chips.
        // (CDMA system time itself is 20ms ticks, but the sync message
        // SYS_TIME field uses 80ms granularity.)
        // LC verification uses chip deltas from paging_start_chip positions,
        // not SYS_TIME arithmetic.
        let start = 0x38e9ecec314;
        let end = 0x3b10f0437f3;
        let chip_diff = 294912;

        // Reference path: chip-by-chip advance.
        let mut step_generator = LongCodeGenerator::new(0);
        step_generator.set_state(start);
        for _ in 0..chip_diff {
            step_generator.advance_chips(1);
        }
        assert_eq!(end, step_generator.state);

        // Optimized path: matrix jump in advance_chips().
        let mut jump_generator = LongCodeGenerator::new(0);
        jump_generator.set_state(start);
        jump_generator.advance_chips(chip_diff);
        assert_eq!(end, jump_generator.state);
    }

    #[test]
    fn test_sync_initial_state() {
        // With the synchronization mask (MSB=1, others 0), the initial state
        // defined by the spec must produce a '1' at that instant.
        let mut generator = LongCodeGenerator::new(1u64 << 41); // Sync mask
        assert_eq!(
            generator.next_chip(),
            1,
            "Initial synchronized output should be 1"
        );
    }

    #[test]
    fn test_paging_channel_mask() {
        // PCH 1, Pilot 0
        let gen1 = LongCodeGenerator::new_paging_channel(1, 0);
        // PCH 2, Pilot 0
        let gen2 = LongCodeGenerator::new_paging_channel(2, 0);
        // PCH 1, Pilot 1
        let gen3 = LongCodeGenerator::new_paging_channel(1, 1);

        assert_ne!(gen1.mask(), gen2.mask());
        assert_ne!(gen1.mask(), gen3.mask());
        assert_ne!(gen2.mask(), gen3.mask());
    }

    #[test]
    fn test_access_channel_mask_fields() {
        let generator = LongCodeGenerator::new_access_channel(0, 1, 1, 0);
        let expected = (0b110001111u64 << 33) | (1u64 << 25) | (1u64 << 9);
        assert_eq!(expected, generator.mask());
    }

    #[test]
    fn test_access_channel_mask_changes_with_inputs() {
        let gen1 = LongCodeGenerator::new_access_channel(0, 1, 1, 0);
        let gen2 = LongCodeGenerator::new_access_channel(1, 1, 1, 0);
        let gen3 = LongCodeGenerator::new_access_channel(0, 2, 1, 0);
        let gen4 = LongCodeGenerator::new_access_channel(0, 1, 2, 0);
        let gen5 = LongCodeGenerator::new_access_channel(0, 1, 1, 1);

        assert_ne!(gen1.mask(), gen2.mask());
        assert_ne!(gen1.mask(), gen3.mask());
        assert_ne!(gen1.mask(), gen4.mask());
        assert_ne!(gen1.mask(), gen5.mask());
    }

    #[test]
    fn test_capture_transitions() {
        // Raw states from PDU log:
        // s1: 0x12d794e6178
        // s2: 0x38e289a145c
        // s3: 0x13dd8dee532

        println!("Searching for transitions with parsing offsets...");

        // s1 was parsed from bits 64..106 of the PDU payload.
        // What if it started at 63 or 65?

        // I can't easily do that without the raw PDU bits.
        // But I can try to see if s2 is related to s1 by a shift.

        let s1: u64 = 0x12d794e6178;
        let s2: u64 = 0x38e289a145c;
        let delta = 98304;
        let taps = [
            42, 35, 33, 31, 27, 26, 25, 22, 21, 19, 18, 17, 16, 10, 7, 6, 5, 3, 2, 1,
        ];

        // Try Fibonacci Left
        let mut state = s1;
        for _ in 0..delta {
            let mut feedback = 0u64;
            for &t in &taps {
                feedback ^= (state >> (t - 1)) & 1;
            }
            state = ((state << 1) | feedback) & 0x3FF_FFFF_FFFF;
        }
        println!(
            "Fibonacci Left s1->s2: expected=0x{:x}, actual=0x{:x}",
            state, s2
        );

        // Try Fibonacci Right
        let mut state = s1;
        for _ in 0..delta {
            let mut feedback = 0u64;
            for &t in &taps {
                feedback ^= (state >> (t - 1)) & 1;
            }
            state = (state >> 1) | (feedback << 41);
        }
        println!(
            "Fibonacci Right s1->s2: expected=0x{:x}, actual=0x{:x}",
            state, s2
        );
    }

    #[test]
    fn test_advance_chips_full_period_returns_to_initial_state() {
        // The 42-bit LFSR has period 2^42 - 1. Advancing by the full period
        // must return the generator to its original state.
        let initial_state = 1u64 << 41;
        let period: u64 = (1u64 << 42) - 1; // 4,398,046,511,103

        let mut lcg = LongCodeGenerator::new(0);
        assert_eq!(lcg.state(), initial_state);

        lcg.advance_chips(period as usize);
        assert_eq!(
            lcg.state(),
            initial_state,
            "After advancing by full period (2^42 - 1 = {}), state should return to initial. Got 0x{:x}",
            period,
            lcg.state()
        );
    }

    #[test]
    fn test_advance_chips_double_period_returns_to_initial_state() {
        // Advancing by 2 * (2^42 - 1) should also return to initial state.
        let initial_state = 1u64 << 41;
        let period: u64 = (1u64 << 42) - 1;

        let mut lcg = LongCodeGenerator::new(0);
        lcg.advance_chips(period as usize);
        lcg.advance_chips(period as usize);
        assert_eq!(
            lcg.state(),
            initial_state,
            "After advancing by 2x full period, state should return to initial. Got 0x{:x}",
            lcg.state()
        );
    }

    #[test]
    fn test_advance_chips_large_value_consistency() {
        // Verify advance_chips at a large runtime-like value (~1.79 trillion chips)
        // produces a consistent state, and that chip-by-chip from that state matches
        // a second matrix jump.
        let large_jump: usize = 1_792_000_000_000;
        let small_step: usize = 1000;

        // Jump forward by a large amount from initial state.
        let mut gen_a = LongCodeGenerator::new(0);
        gen_a.advance_chips(large_jump);
        let state_after_large_jump = gen_a.state();

        // Do the same jump again from a fresh generator -- must match.
        let mut gen_b = LongCodeGenerator::new(0);
        gen_b.advance_chips(large_jump);
        assert_eq!(
            gen_b.state(),
            state_after_large_jump,
            "Two identical large jumps from initial state should produce the same state"
        );

        // From the jumped state, advance by a small amount using matrix jump.
        let mut gen_matrix = LongCodeGenerator::new(0);
        gen_matrix.set_state(state_after_large_jump);
        gen_matrix.advance_chips(small_step);

        // From the same jumped state, advance chip-by-chip.
        let mut gen_step = LongCodeGenerator::new(0);
        gen_step.set_state(state_after_large_jump);
        for _ in 0..small_step {
            gen_step.advance_state();
        }

        assert_eq!(
            gen_matrix.state(),
            gen_step.state(),
            "Matrix jump and chip-by-chip should agree after {} steps from state 0x{:x}",
            small_step,
            state_after_large_jump
        );
    }

    #[test]
    fn test_advance_chips_large_value_output_sequence() {
        // After a large jump, verify the output chip sequence matches chip-by-chip.
        let large_jump: usize = 1_792_000_000_000;
        let mask: u64 = 0x3141592653; // arbitrary nonzero mask

        let mut lcg = LongCodeGenerator::new(mask);
        lcg.advance_chips(large_jump);

        // Capture 64 output chips via next_chip().
        let state_snapshot = lcg.state();
        let chips_from_jumped: Vec<u8> = (0..64).map(|_| lcg.next_chip()).collect();

        // Reproduce chip-by-chip from the same snapshot state.
        let mut gen2 = LongCodeGenerator::new(mask);
        gen2.set_state(state_snapshot);
        let chips_from_step: Vec<u8> = (0..64).map(|_| gen2.next_chip()).collect();

        assert_eq!(
            chips_from_jumped, chips_from_step,
            "Output chips after large jump should match chip-by-chip generation"
        );
    }

    #[test]
    fn test_advance_chips_half_period_twice_returns_to_initial() {
        // Advancing by (2^42 - 1) / 2 twice, plus one more step, should NOT
        // return to initial (since the period is odd, half-period is not exact).
        // But advancing by half_period and then half_period again equals the
        // full period minus 1 step (since 2 * floor((2^42-1)/2) = 2^42 - 2).
        // So we need one more step to complete the period.
        let initial_state = 1u64 << 41;
        let period: u64 = (1u64 << 42) - 1;
        let half: u64 = period / 2; // floor division, = 2_199_023_255_551

        let mut lcg = LongCodeGenerator::new(0);
        lcg.advance_chips(half as usize);
        lcg.advance_chips(half as usize);
        // We've advanced by 2*half = period - 1 chips, so one more to complete.
        assert_ne!(
            lcg.state(),
            initial_state,
            "After period-1 chips, state should NOT be initial"
        );
        lcg.advance_chips(1);
        assert_eq!(
            lcg.state(),
            initial_state,
            "After period-1 + 1 = period chips, state should be initial. Got 0x{:x}",
            lcg.state()
        );
    }

    #[test]
    fn test_traffic_channel_mask_prefix() {
        // PLCM_42 top 10 bits must be 11000_11000 = 0x318 << 32
        let lcg = LongCodeGenerator::new_traffic_channel(0x12345678);
        let mask = lcg.mask();
        let top10 = mask >> 32;
        assert_eq!(
            top10, 0x318,
            "Top 10 bits of PLCM must be 0x318, got 0x{:x}",
            top10
        );
    }

    #[test]
    fn test_traffic_channel_mask_zero_esn() {
        // Zero ESN should permute to zero, so mask = prefix only
        let lcg = LongCodeGenerator::new_traffic_channel(0);
        assert_eq!(lcg.mask(), 0x318_0000_0000u64);
    }

    #[test]
    fn test_traffic_channel_different_esns() {
        let gen1 = LongCodeGenerator::new_traffic_channel(0xAABBCCDD);
        let gen2 = LongCodeGenerator::new_traffic_channel(0x11223344);
        assert_ne!(
            gen1.mask(),
            gen2.mask(),
            "Different ESNs must produce different masks"
        );
    }

    #[test]
    fn test_permute_esn_invertible() {
        // Permutation is a bijection — different inputs must give different outputs
        let p1 = LongCodeGenerator::permute_esn(0x00000001);
        let p2 = LongCodeGenerator::permute_esn(0x00000002);
        let p3 = LongCodeGenerator::permute_esn(0x80000000);
        assert_ne!(p1, p2);
        assert_ne!(p1, p3);
        assert_ne!(p2, p3);
    }

    #[test]
    fn test_permute_esn_single_bits() {
        // Verify a few single-bit inputs against the PERM table:
        // PERM[0] = 9 → input bit 9 maps to output bit 0
        // So ESN = 1<<9 should produce result with bit 0 set
        let result = LongCodeGenerator::permute_esn(1 << 9);
        assert_eq!(result & 1, 1, "ESN bit 9 should map to output bit 0");

        // PERM[31] = 0 → input bit 0 maps to output bit 31
        let result = LongCodeGenerator::permute_esn(1 << 0);
        assert_eq!(
            (result >> 31) & 1,
            1,
            "ESN bit 0 should map to output bit 31"
        );

        // PERM[30] = 31 → input bit 31 maps to output bit 30
        let result = LongCodeGenerator::permute_esn(1 << 31);
        assert_eq!(
            (result >> 30) & 1,
            1,
            "ESN bit 31 should map to output bit 30"
        );
    }

    #[test]
    fn test_permute_esn_all_ones() {
        // All bits set → permuted should also have all bits set
        let result = LongCodeGenerator::permute_esn(0xFFFFFFFF);
        assert_eq!(result, 0xFFFFFFFF, "All-ones ESN must permute to all-ones");
    }

    #[test]
    fn test_traffic_channel_mask_42bit_range() {
        // Mask must fit in 42 bits (the LFSR width)
        let lcg = LongCodeGenerator::new_traffic_channel(0xFFFFFFFF);
        assert!(lcg.mask() < (1u64 << 42), "Mask must fit in 42 bits");
    }

    #[test]
    fn test_traffic_channel_with_state() {
        let custom_state = 0x1234_5678_ABu64;
        let lcg = LongCodeGenerator::new_traffic_channel_with_state(0xDEADBEEF, custom_state);
        assert_eq!(lcg.state(), custom_state);
        // Mask should still be correct
        let top10 = lcg.mask() >> 32;
        assert_eq!(top10, 0x318);
    }
}
