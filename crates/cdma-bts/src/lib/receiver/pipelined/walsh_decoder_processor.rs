use num_complex::Complex32;

use crate::phy::walsh::WalshDecoder;

use super::{PipelineProcessor, SampleBlock};

/// Single Walsh decoder processor (no pilot combining).
pub struct WalshDecoderProcessor {
    walsh: WalshDecoder,
    buffer: Vec<Complex32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    absolute_chip_modulus: Option<usize>,
}

impl WalshDecoderProcessor {
    pub fn new(walsh: WalshDecoder) -> Self {
        Self {
            walsh,
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            absolute_chip_modulus: None,
        }
    }

    pub fn with_absolute_chip_modulus(mut self, modulus: usize) -> Self {
        self.absolute_chip_modulus = Some(modulus.max(1));
        self
    }
}

impl PipelineProcessor for WalshDecoderProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);

        if let Some(modulus) = self.absolute_chip_modulus {
            while !self.buffer.is_empty() && (self.buffer_chip_start % modulus != 0) {
                self.buffer.remove(0);
                self.buffer_chip_start = self.buffer_chip_start.saturating_add(1);
            }
        }

        let mut out_samples = Vec::new();
        while self.buffer.len() >= 64 {
            let chunk = self.buffer.drain(..64).collect::<Vec<_>>();
            out_samples.push(self.walsh.process_symbol(&chunk));
            self.buffer_chip_start = self.buffer_chip_start.saturating_add(64);
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let symbol_chip_span = 64usize;
        let out_chip_start = self
            .buffer_chip_start
            .saturating_sub(out_samples.len() * symbol_chip_span);
        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / 64.0
        } else {
            0.0
        };

        let mut out_block =
            SampleBlock::new(out_samples, out_chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = self.buffer_tags.clone();
        out_block
            .tags
            .insert("global_chip_start", out_chip_start as i64);
        out_block
            .tags
            .insert("walsh_phase", (out_chip_start % 64) as i64);

        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }

        vec![out_block]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::WalshDecoderProcessor;
    use crate::{
        phy::walsh::{WalshDecoder, WalshGenerator},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_walsh_decoder_processor_outputs_one_symbol_per_64_chips() {
        let mut p = WalshDecoderProcessor::new(WalshDecoder::new::<64>(0));
        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut samples = Vec::new();
        for _ in 0..3usize {
            for chip in walsh0 {
                samples.push(Complex32::new(chip as f32, 0.0));
            }
        }

        let block = SampleBlock::new(samples, 0).with_sample_rate_hz(1_228_800.0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(3, out[0].len());
        assert!(out[0].samples.iter().all(|s| s.re > 0.9));
    }

    #[test]
    fn test_walsh_decoder_processor_alignment_modulus() {
        let mut p =
            WalshDecoderProcessor::new(WalshDecoder::new::<64>(0)).with_absolute_chip_modulus(64);
        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut samples = Vec::new();
        for _ in 0..63 {
            samples.push(Complex32::new(0.0, 0.0));
        }
        for chip in walsh0 {
            samples.push(Complex32::new(chip as f32, 0.0));
        }

        let block = SampleBlock::new(samples, 1).with_sample_rate_hz(1_228_800.0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(1, out[0].len());
        assert_eq!(0, out[0].chip_start % 64);
    }
}
