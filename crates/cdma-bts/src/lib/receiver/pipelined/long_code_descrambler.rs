use log::trace;
use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;

use super::{PipelineProcessor, SampleBlock};

/// XORs incoming bits (encoded as Complex32.re 0.0/1.0) with the long code
/// sequence, advancing the generator by `decimation` chips per bit.
///
/// Optionally verifies its internal LC generator state against checkpoints
/// received via `expected_lc_state` / `expected_lc_chip` tags on incoming
/// blocks (set by MobileStation from sync channel events).
pub struct LongCodeDescrambler {
    generator: LongCodeGenerator,
    decimation: usize,
    bypass: bool,
    /// Optional absolute chip cursor that the generator state currently represents.
    /// When set, the descrambler advances the generator to each incoming block's
    /// `chip_start` before processing so LC stays aligned across delayed starts.
    chip_cursor: Option<usize>,
    /// Total chips consumed by this descrambler so far (in chip-rate units).
    chips_consumed: usize,
    /// One-shot: chip position at which the generator was seeded.
    seed_chip: Option<usize>,
    /// Next paging half-frame boundary (chip-rate) at which to log live LC state.
    next_half_frame_chip: Option<usize>,
}

impl LongCodeDescrambler {
    pub fn new(generator: LongCodeGenerator, decimation: usize) -> Self {
        Self {
            generator,
            decimation: decimation.max(1),
            bypass: false,
            chip_cursor: None,
            chips_consumed: 0,
            seed_chip: None,
            next_half_frame_chip: None,
        }
    }

    pub fn with_bypass(mut self, bypass: bool) -> Self {
        self.bypass = bypass;
        self
    }

    pub fn with_chip_cursor(mut self, chip_cursor: usize) -> Self {
        self.chip_cursor = Some(chip_cursor);
        self
    }

    /// Verify generator state against an expected checkpoint.
    ///
    /// `expected_state`: the LC LFSR state that should be valid at `expected_chip`.
    /// `current_state`: our generator's current state.
    /// `current_chip`: the chip position our generator is currently at.
    fn verify_checkpoint(
        &self,
        expected_state: u64,
        expected_chip: usize,
        current_state: u64,
        current_chip: usize,
    ) {
        // Build a temporary generator from expected_state and advance to current_chip.
        let chip_delta = if current_chip >= expected_chip {
            current_chip - expected_chip
        } else {
            // current_chip is before expected_chip — can't verify yet.
            return;
        };

        let mut verify_gen = LongCodeGenerator::new(0);
        verify_gen.set_state(expected_state);
        verify_gen.advance_chips(chip_delta);
        let predicted = verify_gen.state();

        if predicted == current_state {
            // Only log successes silently; failures are always printed.
        } else {
            trace!(
                "lc_descrambler: LC CHECK FAIL chip={} delta={} expected=0x{:x} actual=0x{:x}",
                current_chip, chip_delta, predicted, current_state
            );
        }
    }
}

impl PipelineProcessor for LongCodeDescrambler {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if let Some(cursor) = self.chip_cursor {
            if block.chip_start > cursor {
                self.generator.advance_chips(block.chip_start - cursor);
            }
            self.chip_cursor = Some(block.chip_start);
        }

        // Snapshot LC state before processing this block.
        let lc_state_before = self.generator.state();

        // Record seed chip on the very first block.
        if self.seed_chip.is_none() {
            self.seed_chip = Some(block.chip_start);
            self.next_half_frame_chip = Some(block.chip_start);
            trace!(
                "lc_descrambler: seeded at chip_start={} lc_state=0x{:x}",
                block.chip_start, lc_state_before
            );
        }

        // Check for verification checkpoint from MobileStation.
        let expected_lc_state = block.tags.get("expected_lc_state").copied();
        let expected_lc_chip = block.tags.get("expected_lc_chip").copied();
        if let (Some(exp_state), Some(exp_chip)) = (expected_lc_state, expected_lc_chip) {
            self.verify_checkpoint(
                exp_state as u64,
                exp_chip as usize,
                lc_state_before,
                block.chip_start,
            );
        }

        let mut out_samples: Vec<Complex32> = Vec::with_capacity(block.samples.len());
        const HALF_FRAME_CHIPS: usize = 12_288; // 10 ms at 1.2288 Mcps
        for (idx, s) in block.samples.iter().enumerate() {
            let symbol_chip = block
                .chip_start
                .saturating_add(idx.saturating_mul(self.decimation));
            if let Some(mut next_hf_chip) = self.next_half_frame_chip {
                while symbol_chip >= next_hf_chip {
                    if symbol_chip == next_hf_chip {
                        trace!(
                            "rx_fpch_lc_half_boundary chip={} lc_state=0x{:x}",
                            symbol_chip,
                            self.generator.state()
                        );
                    }
                    next_hf_chip = next_hf_chip.saturating_add(HALF_FRAME_CHIPS);
                }
                self.next_half_frame_chip = Some(next_hf_chip);
            }

            if self.bypass {
                out_samples.push(*s);
                continue;
            }

            let lc_chip = self.generator.next_chip();
            for _ in 1..self.decimation {
                self.generator.next_chip();
            }
            if let Some(cursor) = self.chip_cursor {
                self.chip_cursor = Some(cursor.saturating_add(self.decimation));
            }
            // Soft descrambling: flip sign when LC chip = 1, preserving
            // soft decision magnitude for downstream Viterbi.
            let sign = if lc_chip == 1 { -1.0 } else { 1.0 };
            out_samples.push(Complex32::new(s.re * sign, s.im));
            //trace!("raw: {}", s.re);
            //out_samples.push(Complex32::new(s.re, s.im));
        }

        self.chips_consumed += out_samples.len() * self.decimation;

        let mut out_block = SampleBlock::new(out_samples, block.chip_start)
            .with_sample_rate_hz(block.sample_rate_hz);
        out_block.tags = block.tags;
        // Expose LC state so downstream processors can verify.
        out_block
            .tags
            .insert("lc_state_at_chip", lc_state_before as i64);
        out_block
            .tags
            .insert("lc_state_chip_start", block.chip_start as i64);
        vec![out_block]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::LongCodeDescrambler;
    use crate::{
        phy::coding::long_code::LongCodeGenerator,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_long_code_descrambler_soft_sign_flip() {
        let mut p = LongCodeDescrambler::new(LongCodeGenerator::new(1u64 << 41), 2);
        let mut ref_gen = LongCodeGenerator::new(1u64 << 41);

        // Soft values: positive → bit 0, negative → bit 1.
        let input_soft = [0.8f32, -0.6, 0.3, -0.9];
        let block = SampleBlock::new(
            input_soft.iter().map(|v| Complex32::new(*v, 0.0)).collect(),
            0,
        );

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(input_soft.len(), out[0].len());

        for i in 0..out[0].len() {
            let lc_chip = ref_gen.next_chip();
            let _ = ref_gen.next_chip(); // decimation = 2
            let sign = if lc_chip == 1 { -1.0 } else { 1.0 };
            let expected = input_soft[i] * sign;
            assert_eq!(expected, out[0].samples[i].re);
        }
        assert_eq!(0, out[0].chip_start);
    }
}
