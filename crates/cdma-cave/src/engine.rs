//! The CAVE round function (S.S0053-0 v2.0 §2.1, Exhibit 2-4).

use crate::table::CAVE_TABLE;

const LOMASK: u8 = 0x0F;
const HIMASK: u8 = 0xF0;
pub(crate) const REGISTER_COUNT: usize = 16;
const OCTET_BITS: u32 = u8::BITS;
const LFSR_FEEDBACK_SHIFT: u32 = u32::BITS - 1;
/// Exhibit 2-4: after 32 consecutive nibble matches the LFSR low octet is bumped and the search ends.
const MAX_NIBBLE_RETRIES: u32 = 32;
const SIGNATURE_HIGH_BITS_MASK: u32 = 0b11;

pub(crate) struct Cave {
    /// LFSR_A..LFSR_D, LFSR_A most significant.
    pub(crate) lfsr: [u8; 4],
    /// R00..R15, R00 most significant for the rotation.
    pub(crate) registers: [u8; REGISTER_COUNT],
    pub(crate) offset_1: u8,
    pub(crate) offset_2: u8,
}

impl Cave {
    pub(crate) fn new(
        lfsr: [u8; 4],
        registers: [u8; REGISTER_COUNT],
        offset_1: u8,
        offset_2: u8,
    ) -> Self {
        Self {
            lfsr,
            registers,
            offset_1,
            offset_2,
        }
    }

    /// Feedback taps LFSR_B bit 6 and LFSR_D bits 2, 1, 0 into the LFSR_A MSB on a right shift.
    fn lfsr_cycle(&mut self) {
        let [a, b, c, d] = self.lfsr;
        let feedback = ((b >> 6) ^ (d >> 2) ^ (d >> 1) ^ d) & 1;
        let shifted = u32::from_be_bytes([a, b, c, d]) >> 1;
        self.lfsr = (shifted | (u32::from(feedback) << LFSR_FEEDBACK_SHIFT)).to_be_bytes();
    }

    /// LFSR_D is incremented with no carry into LFSR_C.
    fn bump_lfsr_d(&mut self) {
        self.lfsr[3] = self.lfsr[3].wrapping_add(1);
    }

    fn select_nibble(&mut self, r: u8, use_high: bool) -> u8 {
        let (mask, lfsr_byte) = if use_high { (HIMASK, 1) } else { (LOMASK, 0) };
        let mut fail_count = 0;
        loop {
            let offset = if use_high {
                &mut self.offset_2
            } else {
                &mut self.offset_1
            };
            *offset = offset.wrapping_add(self.lfsr[lfsr_byte] ^ r);
            let nibble = CAVE_TABLE[usize::from(*offset)] & mask;
            if nibble != (r & mask) {
                return nibble;
            }
            self.lfsr_cycle();
            fail_count += 1;
            if fail_count == MAX_NIBBLE_RETRIES {
                self.bump_lfsr_d();
                return nibble;
            }
        }
    }

    pub(crate) fn run(&mut self, number_of_rounds: usize) {
        for round_index in (0..number_of_rounds).rev() {
            let r0 = self.registers[0];
            for r_index in 0..REGISTER_COUNT {
                let r = self.registers[r_index];
                let low = self.select_nibble(r, false);
                let high = self.select_nibble(r, true);
                let temp = low | high;
                let next = if r_index == REGISTER_COUNT - 1 {
                    r0
                } else {
                    self.registers[r_index + 1]
                };
                self.registers[r_index] = next ^ temp;
                self.lfsr_cycle();
            }
            self.registers = u128::from_be_bytes(self.registers)
                .rotate_right(1)
                .to_be_bytes();
            let mut shuffled = [0u8; REGISTER_COUNT];
            for r_index in 0..REGISTER_COUNT {
                let dest = CAVE_TABLE[REGISTER_COUNT * round_index + r_index] & LOMASK;
                shuffled[usize::from(dest)] = self.registers[r_index];
            }
            self.registers = shuffled;
        }
    }

    /// The "roll-over RAND" LFSR reload between key iterations (S.S0053-0 §2.5.1.1).
    pub(crate) fn roll_lfsr(&mut self, saved_rand: [u8; 4]) {
        let r = &self.registers;
        self.lfsr = [r[0], r[1], r[14], r[15]];
        if self.lfsr == [0; 4] {
            self.lfsr = saved_rand;
        }
    }

    /// 18 bits from (R00^R13) low 2 bits, R01^R14 and R02^R15 (S.S0053-0 §2.4).
    pub(crate) fn signature(&self) -> u32 {
        let r = &self.registers;
        (u32::from(r[0] ^ r[13]) & SIGNATURE_HIGH_BITS_MASK) << (2 * OCTET_BITS)
            | u32::from(r[1] ^ r[14]) << OCTET_BITS
            | u32::from(r[2] ^ r[15])
    }
}
