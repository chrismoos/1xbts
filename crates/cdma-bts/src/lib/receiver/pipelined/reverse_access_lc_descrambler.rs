use log::info;
use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;

use super::{PipelineProcessor, SampleBlock};

/// Simple long-code descrambler for reverse access channel.
///
/// Unlike `ReverseAccessLongCodeProcessor`, this assumes the LC phase is
/// already known (provided via the `absolute_chip_start` tag set by the
/// searcher). No acquisition, FFT search, or preamble detection is performed —
/// it simply generates the LC sequence at the given chip offset and multiplies
/// to remove the long code.
pub struct ReverseAccessLcDescrambler {
    template: LongCodeGenerator,
    current_chip: Option<usize>,
    generator: LongCodeGenerator,
}

impl ReverseAccessLcDescrambler {
    pub fn new(generator: LongCodeGenerator) -> Self {
        Self {
            template: generator.clone(),
            current_chip: None,
            generator,
        }
    }

    fn chip_start_from_block(&self, block: &SampleBlock) -> usize {
        block
            .tags
            .get("absolute_chip_start")
            .copied()
            .map(|v| v.max(0) as usize)
            .unwrap_or(block.chip_start)
    }
}

impl PipelineProcessor for ReverseAccessLcDescrambler {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let chip_start = self.chip_start_from_block(&block);

        // Seek the LC generator to the right position.
        match self.current_chip {
            Some(cur) if chip_start >= cur => {
                self.generator.advance_chips(chip_start - cur);
            }
            _ => {
                self.generator = self.template.clone();
                self.generator.advance_chips(chip_start);
            }
        }
        self.current_chip = Some(chip_start);

        // Despread: one LC chip per sample (input is chip-rate).
        let mut out = Vec::with_capacity(block.samples.len());
        let mut pos_count = 0usize;
        for &sample in &block.samples {
            let sign: f32 = if self.generator.next_chip() == 1 {
                -1.0
            } else {
                1.0
            };
            let d = Complex32::new(sample.re * sign, sample.im * sign);
            if d.re > 0.0 {
                pos_count += 1;
            }
            out.push(d);
        }
        let is_first = self.current_chip.is_none() || self.current_chip == Some(chip_start);
        if is_first || pos_count * 2 < block.samples.len() {
            info!(
                "lc_descramble: chip_start={} samples={} pos_re={}/{} pilot_phase={:?}",
                chip_start,
                block.samples.len(),
                pos_count,
                block.samples.len(),
                block.tags.get("pilot_phase"),
            );
        }
        self.current_chip = Some(chip_start + block.samples.len());

        let mut out_block =
            SampleBlock::new(out, block.chip_start).with_sample_rate_hz(block.sample_rate_hz);
        out_block.tags = block.tags;
        vec![out_block]
    }

    fn name(&self) -> &'static str {
        "ReverseAccessLcDescrambler"
    }
}
