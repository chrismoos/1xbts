use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::SymmetricComplexFir32};

use super::{PipelineProcessor, SampleBlock};

/// Applies the CDMA2000 baseband matched filter to incoming complex samples.
pub struct PulseMatchedFilterProcessor {
    matched: SymmetricComplexFir32,
}

impl PulseMatchedFilterProcessor {
    pub fn new() -> Self {
        let taps = cdma2000_baseband_filter_taps_f64();
        Self {
            matched: SymmetricComplexFir32::new(&taps),
        }
    }
}

impl PipelineProcessor for PulseMatchedFilterProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let filtered = self.matched.process_block(&block.samples);

        let mut out =
            SampleBlock::new(filtered, block.chip_start).with_sample_rate_hz(block.sample_rate_hz);
        out.tags = block.tags;
        vec![out]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::PulseMatchedFilterProcessor;
    use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

    #[test]
    fn test_pulse_matched_filter_processor_preserves_len_and_tags() {
        let mut p = PulseMatchedFilterProcessor::new();
        let mut block = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 32], 77)
            .with_sample_rate_hz(1_228_800.0);
        block.tags.insert("x", 1);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(32, out[0].len());
        assert_eq!(77, out[0].chip_start);
        assert_eq!(Some(&1), out[0].tags.get("x"));
    }
}
