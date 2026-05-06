use num_complex::Complex32;

use crate::phy::coding::convolutional::{Encoder, ViterbiDecoder};

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

/// Hard-decision Viterbi decoder for K=9 convolutional codes.
///
/// `N` is the code rate denominator (3 for rate 1/3, 4 for rate 1/4).
/// Input samples use the raw-value convention:
/// - positive -> hard bit 0
/// - negative -> hard bit 1
///
/// Much faster than the soft decoder since branch metrics are simple
/// Hamming distances (integer adds) rather than floating-point operations.
pub struct HardViterbiDecoderProcessor<const N: usize> {
    decoder: ViterbiDecoder<9, N>,
    encoder: Encoder<9, N>,
    buffer: Vec<f32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    reset_per_block: bool,
    assume_zero_end_state: bool,
}

/// Rate 1/3 hard Viterbi decoder (RC1/RC2).
pub type HardViterbiDecoderR13Processor = HardViterbiDecoderProcessor<3>;

/// Rate 1/4 hard Viterbi decoder (RC3).
pub type HardViterbiDecoderR14Processor = HardViterbiDecoderProcessor<4>;

fn hard_decision(v: f32) -> u8 {
    if v >= 0.0 { 0 } else { 1 }
}

fn samples_to_hard_symbols<const N: usize>(samples: &[f32]) -> Vec<[u8; N]> {
    samples
        .chunks_exact(N)
        .map(|chunk| std::array::from_fn(|i| hard_decision(chunk[i])))
        .collect()
}

impl<const N: usize> HardViterbiDecoderProcessor<N> {
    /// Creates a new hard Viterbi decoder processor.
    pub fn new(encoder: Encoder<9, N>) -> Self {
        Self {
            decoder: ViterbiDecoder::new(encoder),
            encoder,
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            reset_per_block: false,
            assume_zero_end_state: false,
        }
    }

    /// When true, each input block is decoded independently.
    pub fn with_reset_per_block(mut self, reset_per_block: bool) -> Self {
        self.reset_per_block = reset_per_block;
        self
    }

    /// When true, the decoder assumes the encoder ends in the zero state.
    pub fn with_assume_zero_end_state(mut self, assume_zero_end_state: bool) -> Self {
        self.assume_zero_end_state = assume_zero_end_state;
        self
    }

    fn decode_unconstrained(
        encoder: Encoder<9, N>,
        raw: &[f32],
        sample_rate_hz: f64,
        chip_start: usize,
        tags: std::collections::HashMap<&'static str, i64>,
    ) -> Vec<SampleBlock> {
        if raw.is_empty() {
            return Vec::new();
        }
        let mut decoder = ViterbiDecoder::new(encoder);
        let inputs = samples_to_hard_symbols::<N>(raw);
        let mut out_bits = Vec::with_capacity(inputs.len() + 8);
        for input in &inputs {
            if let Some(bit) = decoder.process(input) {
                out_bits.push(bit);
            }
        }
        out_bits.extend(decoder.finish());
        let out_samples: Vec<Complex32> = out_bits
            .into_iter()
            .map(|bit| Complex32::new(bit as f32, 0.0))
            .collect();
        if out_samples.is_empty() {
            return Vec::new();
        }
        let out_rate = if sample_rate_hz > 0.0 {
            sample_rate_hz / N as f64
        } else {
            0.0
        };
        let mut out_block = SampleBlock::new(out_samples, chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = tags;
        vec![out_block]
    }

    fn decode_terminated_zero_state(
        encoder: Encoder<9, N>,
        raw: &[f32],
        sample_rate_hz: f64,
        chip_start: usize,
        tags: std::collections::HashMap<&'static str, i64>,
    ) -> Vec<SampleBlock> {
        if raw.is_empty() {
            return Vec::new();
        }
        let mut decoder = ViterbiDecoder::new(encoder);
        let inputs = samples_to_hard_symbols::<N>(raw);
        let out_samples = decoder
            .decode_block_from_state(&inputs, 0)
            .into_iter()
            .map(|bit| Complex32::new(bit as f32, 0.0))
            .collect::<Vec<_>>();
        if out_samples.is_empty() {
            return Vec::new();
        }
        let out_rate = if sample_rate_hz > 0.0 {
            sample_rate_hz / N as f64
        } else {
            0.0
        };
        let mut out_block = SampleBlock::new(out_samples, chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = tags;
        vec![out_block]
    }
}

impl<const N: usize> PipelineProcessor for HardViterbiDecoderProcessor<N> {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if block.samples.is_empty() {
            return vec![block];
        }
        if self.reset_per_block {
            let raw: Vec<f32> = block.samples.iter().map(|s| s.re).collect();
            return if self.assume_zero_end_state {
                Self::decode_terminated_zero_state(
                    self.encoder,
                    &raw,
                    block.sample_rate_hz,
                    block.chip_start,
                    block.tags,
                )
            } else {
                Self::decode_unconstrained(
                    self.encoder,
                    &raw,
                    block.sample_rate_hz,
                    block.chip_start,
                    block.tags,
                )
            };
        }

        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend(block.samples.iter().map(|s| s.re));

        let full_groups = self.buffer.len() / N;
        if full_groups == 0 {
            return Vec::new();
        }

        let symbols = samples_to_hard_symbols::<N>(&self.buffer[..full_groups * N]);
        self.buffer.drain(..full_groups * N);

        let decoded_bits = self.decoder.process_batch(&symbols);
        let out_samples: Vec<Complex32> = decoded_bits
            .into_iter()
            .map(|bit| Complex32::new(bit as f32, 0.0))
            .collect();

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / N as f64
        } else {
            0.0
        };
        let out_chip_start = self.buffer_chip_start;
        let len = out_samples.len();
        self.buffer_chip_start += len * chips_per_sample(out_rate);

        let mut out_block =
            SampleBlock::new(out_samples, out_chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = self.buffer_tags.clone();
        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }
        vec![out_block]
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.buffer.clear();
        let decoded: Vec<Complex32> = self
            .decoder
            .finish()
            .into_iter()
            .map(|bit| Complex32::new(bit as f32, 0.0))
            .collect();
        if decoded.is_empty() {
            return Vec::new();
        }
        vec![
            SampleBlock::new(decoded, self.buffer_chip_start)
                .with_sample_rate_hz(self.buffer_sample_rate_hz / N as f64),
        ]
    }

    fn name(&self) -> &'static str {
        match N {
            3 => "HardViterbiDecoderR13Processor",
            4 => "HardViterbiDecoderR14Processor",
            _ => "HardViterbiDecoderProcessor",
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::HardViterbiDecoderProcessor;
    use crate::{
        phy::coding::convolutional::{get_1_3_k9_encoder, get_1_4_k9_encoder},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_hard_viterbi_r13_processor_roundtrip_noiseless() {
        let msg = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];
        let mut enc = get_1_3_k9_encoder();
        let encoded = msg.iter().flat_map(|b| enc.encode(*b)).collect::<Vec<_>>();
        let block = SampleBlock::new(
            encoded
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );

        let mut p = HardViterbiDecoderProcessor::<3>::new(get_1_3_k9_encoder());
        let mut out = Vec::new();
        for blk in p.process_block(block) {
            out.extend_from_slice(&blk.samples);
        }
        for blk in p.flush() {
            out.extend_from_slice(&blk.samples);
        }
        let out_bits: Vec<u8> = out.iter().map(|s| s.re as u8).collect();
        assert!(out_bits.len() >= msg.len());
        assert_eq!(&msg[..], &out_bits[..msg.len()]);
    }

    #[test]
    fn test_hard_viterbi_r14_processor_roundtrip_noiseless() {
        let msg = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];
        let mut enc = get_1_4_k9_encoder();
        let encoded = msg.iter().flat_map(|b| enc.encode(*b)).collect::<Vec<_>>();
        let block = SampleBlock::new(
            encoded
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );

        let mut p = HardViterbiDecoderProcessor::<4>::new(get_1_4_k9_encoder());
        let mut out = Vec::new();
        for blk in p.process_block(block) {
            out.extend_from_slice(&blk.samples);
        }
        for blk in p.flush() {
            out.extend_from_slice(&blk.samples);
        }
        let out_bits: Vec<u8> = out.iter().map(|s| s.re as u8).collect();
        assert!(out_bits.len() >= msg.len());
        assert_eq!(&msg[..], &out_bits[..msg.len()]);
    }
}
