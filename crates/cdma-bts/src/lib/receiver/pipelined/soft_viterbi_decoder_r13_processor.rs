use num_complex::Complex32;

use crate::phy::coding::convolutional::SoftViterbiDecoder;

use super::{PipelineProcessor, SampleBlock, chips_per_sample, raw_to_soft};

/// Soft-decision Viterbi decoder for K=9, rate 1/3 symbols.
///
/// Input samples use the existing raw-value convention:
/// - positive -> hard bit 0 tendency
/// - negative -> hard bit 1 tendency
///
/// The processor groups symbols into triplets and maps each raw value into
/// soft range [0.0, 1.0] before feeding the decoder.
pub struct SoftViterbiDecoderR13Processor {
    decoder: SoftViterbiDecoder<9, 3>,
    buffer: Vec<f32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    reset_per_block: bool,
    assume_zero_end_state: bool,
}

impl SoftViterbiDecoderR13Processor {
    pub fn new(decoder: SoftViterbiDecoder<9, 3>) -> Self {
        Self {
            decoder,
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            reset_per_block: false,
            assume_zero_end_state: false,
        }
    }

    pub fn with_reset_per_block(mut self, reset_per_block: bool) -> Self {
        self.reset_per_block = reset_per_block;
        self
    }

    pub fn with_assume_zero_end_state(mut self, assume_zero_end_state: bool) -> Self {
        self.assume_zero_end_state = assume_zero_end_state;
        self
    }

    fn decode_triplets_unconstrained(
        triplets: &[f32],
        sample_rate_hz: f64,
        chip_start: usize,
        tags: std::collections::HashMap<&'static str, i64>,
    ) -> Vec<SampleBlock> {
        if triplets.is_empty() {
            return Vec::new();
        }

        let peak = triplets.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let mut decoder = crate::phy::coding::convolutional::get_1_3_k9_soft_viterbi_decoder();
        let mut out_samples = Vec::new();

        for chunk in triplets.chunks_exact(3) {
            let input = [
                raw_to_soft(chunk[0], inv_peak),
                raw_to_soft(chunk[1], inv_peak),
                raw_to_soft(chunk[2], inv_peak),
            ];
            if let Some(bit) = decoder.process(&input) {
                out_samples.push(Complex32::new(bit as f32, 0.0));
            }
        }
        out_samples.extend(
            decoder
                .finish()
                .into_iter()
                .map(|bit| Complex32::new(bit as f32, 0.0)),
        );

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if sample_rate_hz > 0.0 {
            sample_rate_hz / 3.0
        } else {
            0.0
        };
        let mut out_block = SampleBlock::new(out_samples, chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = tags;
        vec![out_block]
    }

    fn decode_triplets_terminated_zero_state(
        triplets: &[f32],
        sample_rate_hz: f64,
        chip_start: usize,
        tags: std::collections::HashMap<&'static str, i64>,
    ) -> Vec<SampleBlock> {
        if triplets.is_empty() {
            return Vec::new();
        }

        let peak = triplets.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let mut decoder = crate::phy::coding::convolutional::get_1_3_k9_soft_viterbi_decoder();
        let inputs = triplets
            .chunks_exact(3)
            .map(|chunk| {
                [
                    raw_to_soft(chunk[0], inv_peak),
                    raw_to_soft(chunk[1], inv_peak),
                    raw_to_soft(chunk[2], inv_peak),
                ]
            })
            .collect::<Vec<_>>();

        let out_samples = decoder
            .decode_block_from_state(&inputs, 0)
            .into_iter()
            .map(|bit| Complex32::new(bit as f32, 0.0))
            .collect::<Vec<_>>();

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if sample_rate_hz > 0.0 {
            sample_rate_hz / 3.0
        } else {
            0.0
        };
        let mut out_block = SampleBlock::new(out_samples, chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = tags;
        vec![out_block]
    }
}

impl PipelineProcessor for SoftViterbiDecoderR13Processor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.reset_per_block {
            let raw: Vec<f32> = block.samples.iter().map(|s| s.re).collect();
            return if self.assume_zero_end_state {
                Self::decode_triplets_terminated_zero_state(
                    &raw,
                    block.sample_rate_hz,
                    block.chip_start,
                    block.tags,
                )
            } else {
                Self::decode_triplets_unconstrained(
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

        let peak = self.buffer.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };

        let mut out_samples = Vec::new();
        let full_triplets = self.buffer.len() / 3;
        for t in 0..full_triplets {
            let base = t * 3;
            let r0 = self.buffer[base];
            let r1 = self.buffer[base + 1];
            let r2 = self.buffer[base + 2];
            let input = [
                raw_to_soft(r0, inv_peak),
                raw_to_soft(r1, inv_peak),
                raw_to_soft(r2, inv_peak),
            ];
            if let Some(bit) = self.decoder.process(&input) {
                out_samples.push(Complex32::new(bit as f32, 0.0));
            }
        }
        if full_triplets > 0 {
            self.buffer.drain(..full_triplets * 3);
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / 3.0
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
        if self.reset_per_block {
            return Vec::new();
        }
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
                .with_sample_rate_hz(self.buffer_sample_rate_hz / 3.0),
        ]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::SoftViterbiDecoderR13Processor;
    use crate::{
        phy::coding::convolutional::{get_1_3_k9_encoder, get_1_3_k9_soft_viterbi_decoder},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_soft_viterbi_r13_processor_roundtrip_noiseless() {
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

        let mut p = SoftViterbiDecoderR13Processor::new(get_1_3_k9_soft_viterbi_decoder());
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
    fn test_soft_viterbi_r13_processor_can_reset_per_block() {
        let msg_a = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];
        let msg_b = [0u8, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1];

        let mut enc_a = get_1_3_k9_encoder();
        let encoded_a = msg_a
            .iter()
            .flat_map(|b| enc_a.encode(*b))
            .collect::<Vec<_>>();
        let mut enc_b = get_1_3_k9_encoder();
        let encoded_b = msg_b
            .iter()
            .flat_map(|b| enc_b.encode(*b))
            .collect::<Vec<_>>();

        let block_a = SampleBlock::new(
            encoded_a
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );
        let block_b = SampleBlock::new(
            encoded_b
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            100,
        );

        let mut p = SoftViterbiDecoderR13Processor::new(get_1_3_k9_soft_viterbi_decoder())
            .with_reset_per_block(true);
        let out_a = p.process_block(block_a);
        let out_b = p.process_block(block_b);

        let bits_a: Vec<u8> = out_a[0].samples.iter().map(|s| s.re as u8).collect();
        let bits_b: Vec<u8> = out_b[0].samples.iter().map(|s| s.re as u8).collect();

        assert!(bits_a.len() >= msg_a.len());
        assert!(bits_b.len() >= msg_b.len());
        assert_eq!(&msg_a[..], &bits_a[..msg_a.len()]);
        assert_eq!(&msg_b[..], &bits_b[..msg_b.len()]);
    }

    #[test]
    fn test_soft_viterbi_r13_processor_terminated_block_zero_end_state() {
        let msg = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];
        let mut terminated = msg.to_vec();
        terminated.extend(std::iter::repeat_n(0u8, 8));

        let mut enc = get_1_3_k9_encoder();
        let encoded = terminated
            .iter()
            .flat_map(|b| enc.encode(*b))
            .collect::<Vec<_>>();

        let block = SampleBlock::new(
            encoded
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );

        let mut p = SoftViterbiDecoderR13Processor::new(get_1_3_k9_soft_viterbi_decoder())
            .with_reset_per_block(true)
            .with_assume_zero_end_state(true);
        let out = p.process_block(block);
        let bits: Vec<u8> = out[0].samples.iter().map(|s| s.re as u8).collect();

        assert_eq!(terminated.len(), bits.len());
        assert_eq!(terminated, bits);
    }

    #[test]
    fn test_soft_viterbi_r13_processor_terminated_access_frame_length() {
        let mut info = vec![0u8; 88];
        for (i, b) in info.iter_mut().enumerate() {
            *b = ((i * 5 + 1) % 9 >= 4) as u8;
        }
        let mut frame = info.clone();
        frame.extend(std::iter::repeat_n(0u8, 8));

        let mut enc = get_1_3_k9_encoder();
        let encoded = frame
            .iter()
            .flat_map(|b| enc.encode(*b))
            .collect::<Vec<_>>();

        let block = SampleBlock::new(
            encoded
                .iter()
                .map(|b| Complex32::new(if *b == 0 { 1.0 } else { -1.0 }, 0.0))
                .collect(),
            0,
        );

        let mut p = SoftViterbiDecoderR13Processor::new(get_1_3_k9_soft_viterbi_decoder())
            .with_reset_per_block(true)
            .with_assume_zero_end_state(true);
        let out = p.process_block(block);
        let bits: Vec<u8> = out[0].samples.iter().map(|s| s.re as u8).collect();

        assert_eq!(frame, bits);
    }
}
