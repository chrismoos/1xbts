use num_complex::Complex32;

use crate::{
    phy::coding::{
        block_interleaver::BitReversalInterleaver, convolutional::ViterbiDecoder,
        long_code::LongCodeGenerator,
    },
    phy::walsh::WalshDecoder,
};

/// Configuration options for [`PipelinedReceiver`].
pub struct PipelinedReceiverOptions {
    /// Pilot Walsh decoder configuration (optional, defaults to Walsh 0 with rep 4)
    pub pilot_walsh: Option<WalshDecoder>,
    /// Long code generator for scrambling/descrambling (optional)
    pub long_code_generator: Option<LongCodeGenerator>,
    /// Whether to wait for all zeros before starting decoding
    pub wait_all_zeros: bool,
    /// Decimation factor for long code (default 64 for paging channel)
    pub long_code_decimation: usize,
    /// Swap each convolutional pair before Viterbi (for parity with legacy hypotheses)
    pub conv_swap_pair: bool,
    /// Invert each convolutional pair before Viterbi (for parity with legacy hypotheses)
    pub conv_invert_pair: bool,
}

impl Default for PipelinedReceiverOptions {
    fn default() -> Self {
        Self {
            pilot_walsh: None,
            long_code_generator: None,
            wait_all_zeros: true,
            long_code_decimation: 64,
            conv_swap_pair: false,
            conv_invert_pair: false,
        }
    }
}

/// Legacy single-channel decode pipeline.
pub struct PipelinedReceiver<const K: usize, const N: usize, S> {
    stream: S,
    channel_walsh: WalshDecoder,
    pilot_walsh: WalshDecoder,
    unrepeat: usize,
    interleaver: BitReversalInterleaver,
    deinterleave_repeats: usize,
    wait_all_zeros: bool,
    primed: bool,
    viterbi_decoder: ViterbiDecoder<K, N>,
    buffer: Vec<u8>,
    bit_chip_starts: Vec<usize>,
    input_chips_consumed: usize,
    last_output_chip_span: Option<(usize, usize)>,
    finished: bool,
    long_code_generator: Option<LongCodeGenerator>,
    long_code_decimation: usize,
    conv_swap_pair: bool,
    conv_invert_pair: bool,
}

impl<const K: usize, const N: usize, S> PipelinedReceiver<K, N, S>
where
    S: Iterator<Item = Complex32>,
{
    /// Returns the chip span `[start, end)` corresponding to the most recent
    /// non-empty decoded output block returned by `next()`.
    pub fn take_last_output_chip_span(&mut self) -> Option<(usize, usize)> {
        self.last_output_chip_span.take()
    }

    /// Create a new legacy decode pipeline with explicit channel settings.
    pub fn new(
        stream: S,
        channel_walsh: WalshDecoder,
        unrepeat: usize,
        interleaver: BitReversalInterleaver,
        deinterleave_repeats: usize,
        wait_all_zeros: bool,
        viterbi_decoder: ViterbiDecoder<K, N>,
    ) -> PipelinedReceiver<K, N, S> {
        PipelinedReceiver {
            stream,
            channel_walsh,
            unrepeat,
            interleaver,
            deinterleave_repeats,
            wait_all_zeros,
            pilot_walsh: WalshDecoder::new::<64>(0),
            viterbi_decoder,
            primed: false,
            buffer: Vec::new(),
            bit_chip_starts: Vec::new(),
            input_chips_consumed: 0,
            last_output_chip_span: None,
            finished: false,
            long_code_generator: None,
            long_code_decimation: 64,
            conv_swap_pair: false,
            conv_invert_pair: false,
        }
    }

    /// Create a new legacy decode pipeline using [`PipelinedReceiverOptions`].
    pub fn new_with_options(
        stream: S,
        channel_walsh: WalshDecoder,
        unrepeat: usize,
        interleaver: BitReversalInterleaver,
        deinterleave_repeats: usize,
        viterbi_decoder: ViterbiDecoder<K, N>,
        options: PipelinedReceiverOptions,
    ) -> PipelinedReceiver<K, N, S> {
        PipelinedReceiver {
            stream,
            channel_walsh,
            unrepeat,
            interleaver,
            deinterleave_repeats,
            wait_all_zeros: options.wait_all_zeros,
            pilot_walsh: options
                .pilot_walsh
                .unwrap_or_else(|| WalshDecoder::new::<64>(0)),
            viterbi_decoder,
            primed: false,
            buffer: Vec::new(),
            bit_chip_starts: Vec::new(),
            input_chips_consumed: 0,
            last_output_chip_span: None,
            finished: false,
            long_code_generator: options.long_code_generator,
            long_code_decimation: options.long_code_decimation,
            conv_swap_pair: options.conv_swap_pair,
            conv_invert_pair: options.conv_invert_pair,
        }
    }
}

impl<const K: usize, const N: usize, S> Iterator for PipelinedReceiver<K, N, S>
where
    S: Iterator<Item = Complex32>,
{
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.buffer.len() >= self.interleaver.block_len() {
                let block = self
                    .buffer
                    .drain(0..self.interleaver.block_len())
                    .collect::<Vec<_>>();
                let block_chip_starts = self
                    .bit_chip_starts
                    .drain(0..self.interleaver.block_len())
                    .collect::<Vec<_>>();
                debug_assert_eq!(block.len(), block_chip_starts.len());
                let block_chip_span = block_chip_starts
                    .first()
                    .copied()
                    .zip(block_chip_starts.last().copied())
                    .map(|(start, last)| (start, last + (64 * self.unrepeat)));

                // Apply long code descrambling if configured
                let buf = if let Some(ref mut long_code_gen) = self.long_code_generator {
                    block
                        .into_iter()
                        .map(|bit| {
                            // Generate long code chips (decimated)
                            let long_code_chip = long_code_gen.next_chip();
                            // Skip decimation_factor-1 chips
                            for _ in 1..self.long_code_decimation {
                                long_code_gen.next_chip();
                            }
                            // XOR with long code for descrambling
                            bit ^ long_code_chip
                        })
                        .collect()
                } else {
                    block
                };

                let deinterleaved = self
                    .interleaver
                    .decode(&buf)
                    .chunks_exact(self.deinterleave_repeats)
                    .flat_map(|n| vec![n[0]])
                    .collect::<Vec<_>>();
                let mut should_decode = true;
                if !self.primed && self.wait_all_zeros {
                    let all_zeros = deinterleaved.iter().all(|n| *n == 0);
                    if all_zeros {
                        self.primed = true;
                    } else {
                        should_decode = false;
                    }
                }

                if should_decode {
                    let result = deinterleaved
                        .chunks_exact(N)
                        .map(|c| {
                            let mut pair = [0u8; N];
                            pair.copy_from_slice(c);
                            if N == 2 {
                                if self.conv_invert_pair {
                                    pair[0] ^= 1;
                                    pair[1] ^= 1;
                                }
                                if self.conv_swap_pair {
                                    pair.swap(0, 1);
                                }
                            }
                            self.viterbi_decoder.process(&pair)
                        })
                        .flatten()
                        .collect::<Vec<_>>();
                    if !result.is_empty() {
                        self.last_output_chip_span = block_chip_span;
                        return Some(result);
                    }
                }
                self.last_output_chip_span = None;
            }

            let walsh_chunk = self
                .stream
                .by_ref()
                .take(64 * self.unrepeat)
                .collect::<Vec<_>>();
            if walsh_chunk.len() != 64 * self.unrepeat {
                if self.finished {
                    self.last_output_chip_span = None;
                    return None;
                } else {
                    self.finished = true;
                    self.last_output_chip_span = None;
                    return Some(self.viterbi_decoder.finish());
                }
            }

            let channel = walsh_chunk
                .chunks_exact(64)
                .flat_map(|c| self.channel_walsh.process(c))
                .collect::<Vec<_>>();
            let pilot = walsh_chunk
                .chunks_exact(64)
                .flat_map(|c| self.pilot_walsh.process(c))
                .collect::<Vec<_>>();

            let combined = (0..pilot.len())
                .map(|n| {
                    Complex32::new(
                        (channel[n].re * pilot[n].re) * 5.0 + (channel[n].im * pilot[n].im) * 5.0,
                        0.0,
                    )
                })
                .collect::<Vec<_>>()
                .chunks_exact(self.unrepeat)
                .flat_map(|n| {
                    vec![Complex32::new(
                        n.iter().map(|v| v.re).sum::<f32>() as f32 / (n.len() as f32),
                        n.iter().map(|v| v.im).sum::<f32>() as f32 / (n.len() as f32),
                    )]
                })
                .map(|s| {
                    (
                        if s.re > 0.0 { 0u8 } else { 1u8 },
                        if s.im > 0.0 { 0u8 } else { 1u8 },
                    )
                })
                .map(|s| s.0)
                .collect::<Vec<_>>();

            let chunk_chip_start = self.input_chips_consumed;
            let chip_stride = 64 * self.unrepeat;
            let combined_len = combined.len();
            self.buffer.extend(combined);
            self.bit_chip_starts
                .extend((0..combined_len).map(|n| chunk_chip_start + (n * chip_stride)));
            self.input_chips_consumed = self.input_chips_consumed.saturating_add(walsh_chunk.len());
        }
    }
}
