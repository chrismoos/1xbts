use std::collections::VecDeque;

use num_complex::Complex32;

use crate::phy::coding::convolutional::{ViterbiDecoder, get_1_2_k9_encoder};

use super::{PipelineProcessor, SampleBlock};

/// Hard-decision Viterbi decoder (rate 1/2, K=9) with dual-decoder
/// polarity handling.
///
/// Runs two Viterbi decoders in parallel — one with normal polarity and
/// one with inverted input bits. Both produce decoded output simultaneously.
/// Output is two interleaved streams packed into a single sample vector:
/// `[normal_0, inverted_0, normal_1, inverted_1, ...]`.
///
/// The downstream PagingChannelProcessor (which already runs dual
/// PagingFrameReaders) is updated to consume this interleaved format
/// and pick whichever decoder's output produces CRC-valid messages.
///
/// This eliminates the ~2 second Viterbi settling period during
/// polarity transitions because one decoder is always on the correct
/// trellis path.
pub struct ViterbiDecoderProcessor {
    decoder_normal: ViterbiDecoder<9, 2>,
    decoder_inverted: ViterbiDecoder<9, 2>,
    buffer: VecDeque<f32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    swap_pair: bool,
    invert_pair: bool,
}

impl ViterbiDecoderProcessor {
    pub fn new(decoder: ViterbiDecoder<9, 2>, swap_pair: bool, invert_pair: bool) -> Self {
        let decoder_inverted = ViterbiDecoder::new(get_1_2_k9_encoder());
        Self {
            decoder_normal: decoder,
            decoder_inverted,
            buffer: VecDeque::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            swap_pair,
            invert_pair,
        }
    }
}

impl PipelineProcessor for ViterbiDecoderProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend(block.samples.iter().map(|s| s.re));

        let mut out_normal = Vec::new();
        let mut out_inverted = Vec::new();

        while self.buffer.len() >= 2 {
            let r0 = self.buffer.pop_front().unwrap();
            let r1 = self.buffer.pop_front().unwrap();

            // Hard decision: positive → 0, negative → 1
            let mut pair = [
                if r0 >= 0.0 { 0u8 } else { 1u8 },
                if r1 >= 0.0 { 0u8 } else { 1u8 },
            ];
            if self.invert_pair {
                pair[0] ^= 1;
                pair[1] ^= 1;
            }
            if self.swap_pair {
                pair.swap(0, 1);
            }

            let inv_pair = [pair[0] ^ 1, pair[1] ^ 1];

            if let Some(bit) = self.decoder_normal.process(&pair) {
                out_normal.push(bit);
            }
            if let Some(bit) = self.decoder_inverted.process(&inv_pair) {
                out_inverted.push(bit);
            }
        }

        // Both decoders should produce the same number of bits.
        let len = out_normal.len().min(out_inverted.len());
        if len == 0 {
            return Vec::new();
        }

        // Interleave: [n0, i0, n1, i1, ...] — downstream will deinterleave.
        let mut interleaved = Vec::with_capacity(len * 2);
        for i in 0..len {
            interleaved.push(Complex32::new(out_normal[i] as f32, 0.0));
            interleaved.push(Complex32::new(out_inverted[i] as f32, 0.0));
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            // Rate is halved by the decoder (rate 1/2), but we output 2×
            // samples (dual), so effective output rate equals input rate.
            self.buffer_sample_rate_hz / 2.0
        } else {
            0.0
        };

        let out_chip_start = self.buffer_chip_start;
        self.buffer_chip_start += len;

        let mut out_block =
            SampleBlock::new(interleaved, out_chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = self.buffer_tags.clone();
        out_block.tags.insert("viterbi_dual", 1);
        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }
        vec![out_block]
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.buffer.clear();
        let normal_bits = self.decoder_normal.finish();
        let inverted_bits = self.decoder_inverted.finish();
        let len = normal_bits.len().min(inverted_bits.len());
        if len == 0 {
            return Vec::new();
        }
        let mut interleaved = Vec::with_capacity(len * 2);
        for i in 0..len {
            interleaved.push(Complex32::new(normal_bits[i] as f32, 0.0));
            interleaved.push(Complex32::new(inverted_bits[i] as f32, 0.0));
        }
        let mut blk =
            SampleBlock::new(interleaved, 0).with_sample_rate_hz(self.buffer_sample_rate_hz / 2.0);
        blk.tags.insert("viterbi_dual", 1);
        vec![blk]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::ViterbiDecoderProcessor;
    use crate::{
        phy::coding::convolutional::{ViterbiDecoder, get_1_2_k9_encoder},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_viterbi_decoder_processor_roundtrip_noiseless() {
        let msg = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];

        let mut enc = get_1_2_k9_encoder();
        let encoded = msg.iter().flat_map(|b| enc.encode(*b)).collect::<Vec<_>>();
        // Map encoded hard bits to raw-value convention: 0 → +1.0, 1 → -1.0
        let block = SampleBlock::new(
            encoded
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );

        let mut p =
            ViterbiDecoderProcessor::new(ViterbiDecoder::new(get_1_2_k9_encoder()), false, false);
        let mut out_samples: Vec<Complex32> = Vec::new();
        for blk in p.process_block(block) {
            out_samples.extend_from_slice(&blk.samples);
        }
        for blk in p.flush() {
            out_samples.extend_from_slice(&blk.samples);
        }
        // Dual output: deinterleave normal bits (even indices).
        let out_bits: Vec<u8> = out_samples.iter().step_by(2).map(|s| s.re as u8).collect();

        assert!(out_bits.len() >= msg.len());
        assert_eq!(&msg[..], &out_bits[..msg.len()]);
    }

    #[test]
    fn test_viterbi_decoder_processor_flush_handles_dangling_half_pair() {
        let mut p =
            ViterbiDecoderProcessor::new(ViterbiDecoder::new(get_1_2_k9_encoder()), false, false);
        let _ = p.process_block(SampleBlock::new(vec![Complex32::new(1.0, 0.0)], 0));
        let _ = p.flush();
    }
}
