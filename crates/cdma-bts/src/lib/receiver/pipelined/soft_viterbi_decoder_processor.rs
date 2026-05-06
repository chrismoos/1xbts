use std::collections::VecDeque;

use num_complex::Complex32;

use crate::phy::coding::convolutional::{SoftViterbiDecoder, get_1_2_k9_encoder};

use super::{PipelineProcessor, SampleBlock, chips_per_sample, raw_to_soft};

/// Soft-decision Viterbi decoder (rate 1/2, K=9). Accumulates pairs of
/// raw soft values (Complex32.re, where positive → bit 0, negative → bit 1)
/// and normalizes them to [0.0, 1.0] before feeding to the squared-Euclidean-
/// distance soft decoder.
///
/// Output bits are encoded as Complex32.re 0.0/1.0.
pub struct SoftViterbiDecoderProcessor {
    decoder: SoftViterbiDecoder<9, 2>,
    buffer: VecDeque<f32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    swap_pair: bool,
    invert_pair: bool,
    sticky_locked_offset: Option<i64>,
    sticky_locked_shift: Option<i64>,
    sticky_locked_invert: Option<i64>,
    lock_signature: Option<(i64, i64, i64)>,
    skip_output_bits: usize,
}

impl SoftViterbiDecoderProcessor {
    pub fn new(decoder: SoftViterbiDecoder<9, 2>, swap_pair: bool, invert_pair: bool) -> Self {
        Self {
            decoder,
            buffer: VecDeque::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            swap_pair,
            invert_pair,
            sticky_locked_offset: None,
            sticky_locked_shift: None,
            sticky_locked_invert: None,
            lock_signature: None,
            skip_output_bits: 0,
        }
    }
}

impl PipelineProcessor for SoftViterbiDecoderProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if let Some(v) = block.tags.get("deinterleaver_locked_offset") {
            self.sticky_locked_offset = Some(*v);
        }
        if let Some(v) = block.tags.get("deinterleaver_locked_shift") {
            self.sticky_locked_shift = Some(*v);
        }
        if let Some(v) = block.tags.get("deinterleaver_locked_invert") {
            self.sticky_locked_invert = Some(*v);
        }
        if let (Some(off), Some(shift), Some(inv)) = (
            self.sticky_locked_offset,
            self.sticky_locked_shift,
            self.sticky_locked_invert,
        ) {
            let sig = (off, shift, inv);
            if self.lock_signature != Some(sig) {
                // Lock transition: clear traceback history and drop startup bits.
                self.decoder = SoftViterbiDecoder::new(get_1_2_k9_encoder());
                self.buffer.clear();
                self.buffer_chip_start = block.chip_start;
                self.lock_signature = Some(sig);
                self.skip_output_bits = 0;
            }
        }
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend(block.samples.iter().map(|s| s.re));

        // Compute peak for normalization over current buffer contents.
        let peak = self.buffer.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };

        let mut out_samples = Vec::new();
        while self.buffer.len() >= 2 {
            let r0 = self.buffer.pop_front().unwrap();
            let r1 = self.buffer.pop_front().unwrap();

            let mut pair = [raw_to_soft(r0, inv_peak), raw_to_soft(r1, inv_peak)];
            let effective_invert_pair =
                self.invert_pair ^ (self.sticky_locked_invert.unwrap_or(0) == 1);
            if effective_invert_pair {
                pair[0] = 1.0 - pair[0];
                pair[1] = 1.0 - pair[1];
            }
            if self.swap_pair {
                pair.swap(0, 1);
            }

            if let Some(bit) = self.decoder.process(&pair) {
                if self.skip_output_bits > 0 {
                    self.skip_output_bits -= 1;
                } else {
                    out_samples.push(Complex32::new(bit as f32, 0.0));
                }
            }
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / 2.0
        } else {
            0.0
        };

        let out_chip_start = self.buffer_chip_start;
        let len = out_samples.len();

        let mut out_block =
            SampleBlock::new(out_samples, out_chip_start).with_sample_rate_hz(out_rate);
        // Advance chip_start in chip-rate units: each output bit spans
        // chip_rate / output_rate chips.
        self.buffer_chip_start += len * chips_per_sample(out_rate);

        out_block.tags = self.buffer_tags.clone();
        if let Some(v) = self.sticky_locked_offset {
            out_block.tags.insert("deinterleaver_locked_offset", v);
        }
        if let Some(v) = self.sticky_locked_shift {
            out_block.tags.insert("deinterleaver_locked_shift", v);
        }
        if let Some(v) = self.sticky_locked_invert {
            out_block.tags.insert("deinterleaver_locked_invert", v);
        }
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
        vec![SampleBlock::new(decoded, 0).with_sample_rate_hz(self.buffer_sample_rate_hz / 2.0)]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::SoftViterbiDecoderProcessor;
    use crate::{
        phy::coding::convolutional::{SoftViterbiDecoder, get_1_2_k9_encoder},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_soft_viterbi_processor_roundtrip_noiseless() {
        let msg = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];

        let mut enc = get_1_2_k9_encoder();
        let encoded = msg.iter().flat_map(|b| enc.encode(*b)).collect::<Vec<_>>();
        // Raw-value convention: bit 0 → +1.0, bit 1 → -1.0
        let block = SampleBlock::new(
            encoded
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );

        let mut p = SoftViterbiDecoderProcessor::new(
            SoftViterbiDecoder::new(get_1_2_k9_encoder()),
            false,
            false,
        );
        let mut out_samples: Vec<Complex32> = Vec::new();
        for blk in p.process_block(block) {
            out_samples.extend_from_slice(&blk.samples);
        }
        for blk in p.flush() {
            out_samples.extend_from_slice(&blk.samples);
        }
        let out_bits: Vec<u8> = out_samples.iter().map(|s| s.re as u8).collect();

        assert!(out_bits.len() >= msg.len());
        assert_eq!(&msg[..], &out_bits[..msg.len()]);
    }

    #[test]
    fn test_soft_viterbi_processor_with_noisy_raw_input() {
        let msg = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];

        let mut enc = get_1_2_k9_encoder();
        let encoded = msg.iter().flat_map(|b| enc.encode(*b)).collect::<Vec<_>>();

        // Raw values with noise (positive=0, negative=1)
        let samples: Vec<Complex32> = encoded
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let raw = if *b == 0 { 1.0f32 } else { -1.0 };
                let noise = ((i as f32 * 0.7).sin()) * 0.3;
                Complex32::new(raw + noise, 0.0)
            })
            .collect();
        let block = SampleBlock::new(samples, 0);

        let mut p = SoftViterbiDecoderProcessor::new(
            SoftViterbiDecoder::new(get_1_2_k9_encoder()),
            false,
            false,
        );
        let mut out_samples: Vec<Complex32> = Vec::new();
        for blk in p.process_block(block) {
            out_samples.extend_from_slice(&blk.samples);
        }
        for blk in p.flush() {
            out_samples.extend_from_slice(&blk.samples);
        }
        let out_bits: Vec<u8> = out_samples.iter().map(|s| s.re as u8).collect();

        assert!(out_bits.len() >= msg.len());
        assert_eq!(&msg[..], &out_bits[..msg.len()]);
    }

    #[test]
    fn test_soft_viterbi_processor_lock_transition_resets_state_and_propagates_tags() {
        let msg = (0..160).map(|i| (i & 1) as u8).collect::<Vec<_>>();
        let mut enc = get_1_2_k9_encoder();
        let encoded = msg.iter().flat_map(|b| enc.encode(*b)).collect::<Vec<_>>();
        let raw_samples = encoded
            .iter()
            .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
            .collect::<Vec<_>>();

        let mut p = SoftViterbiDecoderProcessor::new(
            SoftViterbiDecoder::new(get_1_2_k9_encoder()),
            false,
            false,
        );
        let first_out = p.process_block(SampleBlock::new(raw_samples.clone(), 0));
        assert!(
            !first_out.is_empty(),
            "expected decoded output before lock transition"
        );

        let mut locked_block = SampleBlock::new(raw_samples, 0);
        locked_block.chip_start = 1_000;
        locked_block.tags.insert("deinterleaver_locked_offset", 0);
        locked_block.tags.insert("deinterleaver_locked_shift", 31);
        locked_block.tags.insert("deinterleaver_locked_invert", 0);
        let lock_out = p.process_block(locked_block);
        assert!(
            !lock_out.is_empty(),
            "expected decoded output after lock transition"
        );
        let out = &lock_out[0];
        assert_eq!(
            1_000, out.chip_start,
            "lock transition should reset Viterbi output chip_start origin"
        );
        assert_eq!(Some(&0), out.tags.get("deinterleaver_locked_offset"));
        assert_eq!(Some(&31), out.tags.get("deinterleaver_locked_shift"));
        assert_eq!(Some(&0), out.tags.get("deinterleaver_locked_invert"));
    }
}
