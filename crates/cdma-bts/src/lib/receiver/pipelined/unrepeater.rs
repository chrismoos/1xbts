use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};

/// Averages groups of `factor` symbols into a single soft value.
///
/// Output: Complex32 with re = averaged value (positive → bit 0, negative → bit 1).
/// The continuous value preserves soft decision information for downstream decoders.
pub struct Unrepeater {
    factor: usize,
    buffer: Vec<Complex32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
}

impl Unrepeater {
    pub fn new(factor: usize) -> Self {
        Self {
            factor: factor.max(1),
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
        }
    }
}

impl PipelineProcessor for Unrepeater {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);

        let mut out_samples = Vec::new();
        while self.buffer.len() >= self.factor {
            let avg_re: f32 =
                self.buffer.drain(..self.factor).map(|v| v.re).sum::<f32>() / self.factor as f32;
            out_samples.push(Complex32::new(avg_re, 0.0));
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / self.factor as f64
        } else {
            0.0
        };
        let mut out_block =
            SampleBlock::new(out_samples, self.buffer_chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = self.buffer_tags.clone();
        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }
        vec![out_block]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::Unrepeater;
    use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

    #[test]
    fn test_unrepeater_averages_soft_output() {
        let mut p = Unrepeater::new(2);
        let block = SampleBlock::new(
            vec![
                Complex32::new(2.0, 0.0),
                Complex32::new(1.0, 0.0),
                Complex32::new(-2.0, 0.0),
                Complex32::new(-1.0, 0.0),
            ],
            10,
        );

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(2, out[0].len());
        // Positive average → bit 0 direction
        assert!(out[0].samples[0].re > 0.0);
        assert_eq!(1.5, out[0].samples[0].re);
        // Negative average → bit 1 direction
        assert!(out[0].samples[1].re < 0.0);
        assert_eq!(-1.5, out[0].samples[1].re);
        assert_eq!(10, out[0].chip_start);
    }

    #[test]
    fn test_unrepeater_buffers_partial_group() {
        let mut p = Unrepeater::new(3);
        let out1 = p.process_block(SampleBlock::new(vec![Complex32::new(1.0, 0.0); 2], 0));
        assert!(out1.is_empty());

        let out2 = p.process_block(SampleBlock::new(vec![Complex32::new(1.0, 0.0); 1], 2));
        assert_eq!(1, out2.len());
        assert_eq!(1, out2[0].len());
        // All positive inputs → positive average
        assert!(out2[0].samples[0].re > 0.0);
    }
}
