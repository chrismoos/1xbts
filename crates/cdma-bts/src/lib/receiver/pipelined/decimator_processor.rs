use num_complex::Complex32;

use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

/// Fixed-rate sample decimator that sums each input group.
pub struct DecimatorProcessor {
    rate: usize,
}

impl DecimatorProcessor {
    /// Create a new fixed-rate decimator.
    pub fn new(rate: usize) -> DecimatorProcessor {
        DecimatorProcessor { rate }
    }
}

impl PipelineProcessor for DecimatorProcessor {
    fn process_block(&mut self, block: super::SampleBlock) -> Vec<super::SampleBlock> {
        assert_eq!(block.len() % self.rate, 0);
        let chunks = block.samples.chunks_exact(self.rate).collect::<Vec<_>>();

        let samples = chunks
            .into_iter()
            .map(|s| {
                let mut i = 0.0;
                let mut q = 0.0;
                for samp in s {
                    i += samp.re;
                    q += samp.im;
                }
                //Complex32::new(i / s.len() as f32, q / s.len() as f32)
                Complex32::new(i, q)
            })
            .collect::<Vec<_>>();
        vec![
            SampleBlock::new(samples, block.chip_start / self.rate)
                .with_sample_rate_hz(block.sample_rate_hz / self.rate as f64)
                .with_tags(block.tags),
        ]
    }
}
