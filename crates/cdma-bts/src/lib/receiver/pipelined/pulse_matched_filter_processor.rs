use num_complex::Complex32;
use sdr::FIR;

use crate::sdr::cdma2000_baseband_filter_taps_f64;

use super::{PipelineProcessor, SampleBlock};

/// Applies the CDMA2000 baseband matched filter to incoming complex samples.
pub struct PulseMatchedFilterProcessor {
    matched_i: FIR<f32>,
    matched_q: FIR<f32>,
}

impl PulseMatchedFilterProcessor {
    pub fn new() -> Self {
        let taps = cdma2000_baseband_filter_taps_f64();
        Self {
            matched_i: FIR::new(&taps, 1, 1),
            matched_q: FIR::new(&taps, 1, 1),
        }
    }
}

impl PipelineProcessor for PulseMatchedFilterProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let i_vals: Vec<f32> = block.samples.iter().map(|i| i.re).collect();
        let q_vals: Vec<f32> = block.samples.iter().map(|i| i.im).collect();
        let filtered_i = self.matched_i.process(&i_vals);
        let filtered_q = self.matched_q.process(&q_vals);
        let filtered: Vec<Complex32> = filtered_i
            .into_iter()
            .zip(filtered_q)
            .map(|(i, q)| Complex32::new(i, q))
            .collect();

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
