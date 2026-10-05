use num_complex::Complex32;

use crate::phy::coding::block_interleaver::BitReversalInterleaver;

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

/// Input must start on an interleaver block boundary. No offset search is performed.
pub struct DeinterleaverProcessor {
    interleaver: BitReversalInterleaver,
    deinterleave_repeats: usize,
    buffer: Vec<Complex32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    reset_on_tag: Option<&'static str>,
}

impl DeinterleaverProcessor {
    pub fn new(interleaver: BitReversalInterleaver, deinterleave_repeats: usize) -> Self {
        Self {
            interleaver,
            deinterleave_repeats: deinterleave_repeats.max(1),
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            reset_on_tag: None,
        }
    }

    pub fn with_reset_on_tag(mut self, tag: &'static str) -> Self {
        self.reset_on_tag = Some(tag);
        self
    }

    fn maybe_reset_on_tag(&mut self, tags: &std::collections::HashMap<&'static str, i64>) {
        let should_reset = self
            .reset_on_tag
            .and_then(|tag| tags.get(tag))
            .copied()
            .unwrap_or(0)
            == 1;
        if should_reset {
            self.buffer.clear();
        }
    }
}

impl PipelineProcessor for DeinterleaverProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        // Pass through empty event blocks (e.g. preamble detection) unchanged.
        if block.samples.is_empty() {
            return vec![block];
        }
        self.maybe_reset_on_tag(&block.tags);
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);
        let block_len = self.interleaver.block_len();
        let mut out_samples = Vec::new();

        let cps = chips_per_sample(self.buffer_sample_rate_hz);
        let out_chip_start = self.buffer_chip_start;
        while self.buffer.len() >= block_len {
            let chunk: Vec<f32> = self.buffer.drain(..block_len).map(|s| s.re).collect();
            let deinterleaved = self.interleaver.decode_soft(&chunk);

            out_samples.extend(
                deinterleaved
                    .chunks_exact(self.deinterleave_repeats)
                    .map(|c| {
                        let avg: f32 = c.iter().sum::<f32>() / c.len() as f32;
                        Complex32::new(avg, 0.0)
                    }),
            );
            // Advance chip_start past the consumed interleaver block.
            self.buffer_chip_start += block_len * cps;
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / self.deinterleave_repeats as f64
        } else {
            0.0
        };
        let mut out_block =
            SampleBlock::new(out_samples, out_chip_start).with_sample_rate_hz(out_rate);
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

    use super::DeinterleaverProcessor;
    use crate::{
        phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_48},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_deinterleaver_processor_reverses_interleaving() {
        let mut i = BitReversalInterleaver::new(SR1_PARAMS_48);
        let original = (0..48u8).map(|v| v % 2).collect::<Vec<_>>();
        let interleaved = i.encode(&original);

        let mut p = DeinterleaverProcessor::new(BitReversalInterleaver::new(SR1_PARAMS_48), 1);
        let block = SampleBlock::new(
            interleaved
                .iter()
                .map(|b| Complex32::new(*b as f32, 0.0))
                .collect(),
            123,
        );

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        let out_bits: Vec<u8> = out[0].samples.iter().map(|s| s.re as u8).collect();
        assert_eq!(original, out_bits);
        assert_eq!(123, out[0].chip_start);
    }

    #[test]
    fn test_deinterleaver_processor_with_repeat_stride() {
        let mut p = DeinterleaverProcessor::new(BitReversalInterleaver::new(SR1_PARAMS_48), 2);
        let block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 48], 0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(24, out[0].len());
    }
}
