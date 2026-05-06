use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};

/// Decimates an oversampled signal by selecting the peak sample per chip period.
///
/// After a matched filter, the raised cosine response has zero ISI at the peak
/// sample instant. Unlike sum-and-dump decimation (which integrates adjacent-chip
/// sidelobe energy), this selects the single sample at the sampling phase that
/// maximizes total energy across the block.
pub struct PeakSampleDecimator {
    rate: usize,
}

impl PeakSampleDecimator {
    pub fn new(rate: usize) -> Self {
        Self { rate: rate.max(1) }
    }
}

impl PipelineProcessor for PeakSampleDecimator {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.rate <= 1 {
            return vec![block];
        }
        assert_eq!(block.len() % self.rate, 0);

        let chunks: Vec<&[Complex32]> = block.samples.chunks_exact(self.rate).collect();
        if chunks.is_empty() {
            return vec![
                SampleBlock::new(vec![], block.chip_start / self.rate)
                    .with_sample_rate_hz(block.sample_rate_hz / self.rate as f64)
                    .with_tags(block.tags),
            ];
        }

        // Find the sampling phase (0..rate) that maximizes total energy.
        let mut best_offset = 0usize;
        let mut best_energy = 0.0f64;
        for offset in 0..self.rate {
            let energy: f64 = chunks
                .iter()
                .map(|chunk| {
                    let s = chunk[offset];
                    (s.re * s.re + s.im * s.im) as f64
                })
                .sum();
            if energy > best_energy {
                best_energy = energy;
                best_offset = offset;
            }
        }

        let samples: Vec<Complex32> = chunks.iter().map(|chunk| chunk[best_offset]).collect();

        vec![
            SampleBlock::new(samples, block.chip_start / self.rate)
                .with_sample_rate_hz(block.sample_rate_hz / self.rate as f64)
                .with_tags(block.tags),
        ]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::PeakSampleDecimator;
    use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

    #[test]
    fn test_peak_sample_decimator_selects_strongest_phase() {
        let mut p = PeakSampleDecimator::new(4);
        // 4x oversampled: peak at offset 1
        let samples = vec![
            // chip 0
            Complex32::new(0.1, 0.0),
            Complex32::new(1.0, 0.0), // peak
            Complex32::new(0.2, 0.0),
            Complex32::new(0.05, 0.0),
            // chip 1
            Complex32::new(0.1, 0.0),
            Complex32::new(-1.0, 0.0), // peak (negative)
            Complex32::new(0.2, 0.0),
            Complex32::new(0.05, 0.0),
        ];
        let block = SampleBlock::new(samples, 0).with_sample_rate_hz(1_228_800.0 * 4.0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(2, out[0].len());
        assert_eq!(1.0, out[0].samples[0].re);
        assert_eq!(-1.0, out[0].samples[1].re);
    }

    #[test]
    fn test_peak_sample_decimator_passthrough_rate_1() {
        let mut p = PeakSampleDecimator::new(1);
        let samples = vec![Complex32::new(1.0, 0.0); 8];
        let block = SampleBlock::new(samples.clone(), 0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(8, out[0].len());
    }
}
