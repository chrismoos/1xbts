use parking_lot::Mutex;
use std::collections::VecDeque;

use cdma_common::time::CdmaSystemTime;
use log::trace;
use num::complex::Complex32;

use crate::{
    mac::types::DataRequest,
    phy::coding::{
        block_interleaver::BitReversalInterleaver, convolutional::Encoder,
        long_code::LongCodeGenerator,
    },
};

use super::Channel;

pub struct Config<const EK: usize, const ER: usize> {
    pub data_rate: usize,
    pub encoder: Encoder<EK, ER>,
    pub interleaver: BitReversalInterleaver,
    pub long_code_generator: LongCodeGenerator,
    /// Debug mode: bypass long-code scrambling and emit the encoded/interleaved
    /// paging symbols directly.
    pub bypass_long_code: bool,
    pub pn_pilot_offset: usize,
    /// Debug mode: force all paging encoder input bits to 0 while still
    /// draining incoming fragments. Useful for end-to-end chipping checks.
    pub force_zero_payload_bits: bool,
    /// Absolute chip cursor (since CDMA epoch) corresponding to the current
    /// long-code generator state.
    pub lc_chip_cursor: u64,
    /// Bounded debug instrumentation budget to avoid flooding logs.
    pub debug_windows_left: usize,
}

pub struct ForwardPagingChannel<const EK: usize, const ER: usize> {
    config: Mutex<Config<EK, ER>>,
    fragments: Mutex<VecDeque<DataRequest>>,
}

impl<const EK: usize, const ER: usize> ForwardPagingChannel<EK, ER> {
    pub fn new(config: Config<EK, ER>) -> ForwardPagingChannel<EK, ER> {
        ForwardPagingChannel {
            fragments: Mutex::new(VecDeque::new()),
            config: Mutex::new(config),
        }
    }

    pub fn send_fragment(&self, fragment: DataRequest) {
        self.fragments.lock().push_back(fragment);
    }

    /// Advance the internal long code generator to the given absolute chip
    /// position. On the first call (from lc_chip_cursor=0) this advances by
    /// `chip` chips. Subsequent calls advance by the delta from the current
    /// position, making the function safe to call multiple times (e.g. after
    /// a TX re-anchor).
    pub fn advance_lc_to_chip(&self, chip: u64) {
        let mut config = self.config.lock();
        let delta = chip.saturating_sub(config.lc_chip_cursor);
        config.long_code_generator.advance_chips(delta as usize);
        config.lc_chip_cursor = chip;
    }

    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut config = self.config.lock();

        let mut block = Vec::new();
        let mut source_bits = Vec::new();
        let mut consumed_data_bits = 0usize;
        let mut consumed_input_one_bits = 0usize;
        let mut encoded_input_one_bits = 0usize;

        // Loop through bits until we have enough for an interleaver block
        while block.len() < config.interleaver.block_len() {
            let mut bit = 0;
            let mut source_bit = 0;

            let mut fragments = self.fragments.lock();
            if let Some(fragment) = fragments.front_mut() {
                let mut can_send_now = true;

                if let Some(ts) = fragment.mcsb.requested_tx_time {
                    if current_system_time < ts {
                        can_send_now = false;
                    }
                }

                if can_send_now {
                    if fragment.size == fragment.data.len() {
                        trace!(
                            "F-pch fragment sending {:?}",
                            fragment
                                .data
                                .bits()
                                .iter()
                                .map(|s| format!("{}", s))
                                .collect::<Vec<_>>()
                                .join("")
                        );
                    }
                    if let Some(next) = fragment.data.take_next() {
                        source_bit = next;
                        consumed_input_one_bits += next as usize;
                        bit = if config.force_zero_payload_bits {
                            0
                        } else {
                            next
                        };
                        encoded_input_one_bits += bit as usize;
                        consumed_data_bits += 1;
                    }
                    if fragment.data.len() == 0 {
                        trace!("F-pch fragment of size {} sent fully", fragment.size);
                        let _ = fragments.pop_front();
                    }
                }
            }
            source_bits.push(source_bit);

            // Convolutional encode (rate 1/2) — no symbol repetition for 9600 bps
            config.encoder.encode(bit).iter().for_each(|b| {
                block.push(*b);
            });
        }

        // Interleave
        let interleaved = config.interleaver.encode(&block);

        // Long code scrambling: XOR each symbol with first chip of every 64 long code chips
        let lc_state_start = config.long_code_generator.state();
        let lc_chip_start = config.lc_chip_cursor;
        let half_symbols = block.len() / 2;
        let half_chips = (half_symbols as u64) * 64;
        let lc_chip_mid = lc_chip_start.saturating_add(half_chips);
        let mut lc_state_mid = lc_state_start;
        let mut out = Vec::with_capacity(interleaved.len());
        for (idx, _sym) in interleaved.into_iter().enumerate() {
            if idx == half_symbols {
                // Actual live generator state at the exact second half-frame boundary.
                lc_state_mid = config.long_code_generator.state();
            }
            let lc_chip = config.long_code_generator.next_chip();
            // Skip 63 chips (decimation factor 64)
            for _ in 1..64 {
                config.long_code_generator.next_chip();
            }
            let scrambled = if config.bypass_long_code {
                0
            } else {
                _sym ^ lc_chip
            };
            out.push(Complex32::new(if scrambled == 0 { 1.0 } else { -1.0 }, 0.0));
            //out.push(Complex32::new(if sym == 1 { 1.0 } else { -1.0 }, 0.0));
            // out.push(Complex32::new(-1.0, 0.0));
        }
        let chips_advanced = (out.len() as u64) * 64;
        config.lc_chip_cursor = config.lc_chip_cursor.saturating_add(chips_advanced);
        let lc_state_end = config.long_code_generator.state();
        let lc_chip_end = config.lc_chip_cursor;
        let hf0_sci = source_bits.first().copied().unwrap_or(0);
        let hf1_sci = source_bits.get(source_bits.len() / 2).copied().unwrap_or(0);

        // Actual live long-code generator state at each 10 ms paging
        // half-frame boundary on TX. This is the reference stream that should
        // align 1:1 with RX half-frame boundary descrambler logs once locked.
        trace!(
            "tx_fpch_lc_half_boundary chip={} lc_state=0x{:x}",
            lc_chip_start, lc_state_start
        );
        trace!(
            "tx_fpch_lc_half_boundary chip={} lc_state=0x{:x}",
            lc_chip_mid, lc_state_mid
        );

        if hf0_sci == 1 || hf1_sci == 1 {
            trace!(
                "tx_fpch_boundary hf0_chip={} hf0_lc=0x{:x} hf0_sci={} hf1_chip={} hf1_lc=0x{:x} hf1_sci={} frame_end_chip={} frame_end_lc=0x{:x} force_zero_payload_bits={}",
                lc_chip_start,
                lc_state_start,
                hf0_sci,
                lc_chip_mid,
                lc_state_mid,
                hf1_sci,
                lc_chip_end,
                lc_state_end,
                config.force_zero_payload_bits
            );
        }

        if config.debug_windows_left > 0 {
            trace!(
                "tx_fpch_lc_window start_chip={} end_chip={} lc_state_start=0x{:x} lc_state_end=0x{:x} symbols={} chips={} consumed_data_bits={} consumed_input_ones={} encoded_input_ones={} force_zero_payload_bits={}",
                lc_chip_start,
                lc_chip_end,
                lc_state_start,
                lc_state_end,
                out.len(),
                chips_advanced,
                consumed_data_bits,
                consumed_input_one_bits,
                encoded_input_one_bits,
                config.force_zero_payload_bits
            );
            config.debug_windows_left -= 1;
        }

        out
    }
}

impl<const EK: usize, const ER: usize> Channel for ForwardPagingChannel<EK, ER> {
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut output = Vec::with_capacity(num_samples);
        self.next_block_into(&mut output, num_samples, system_time);
        output
    }

    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) {
        let start = out.len();
        while out.len() - start < num_samples {
            out.extend(self.next(system_time));
        }
    }
}
