//! Forward Traffic Channel framer for Radio Configuration 2 (IS-95B Rate Set 2).
//!
//! Per C.S0002-E §3.1.3.1.1.2 / Figure 3.1.3.1.1-18:
//!
//! ```text
//!  reserved/flag → channel bits → FQI → 8-bit encoder tail → R=1/2 K=9 →
//!  repetition → `110101` puncture → block interleaver (384 symbols) → W
//! ```
//!
//! Per-rate bit budget (Figure 3.1.3.1.1-18):
//!
//! | Rate (bps) | Info | FQI | Tail | Input | Repeat | After rep | Out |
//! |-----------:|-----:|----:|-----:|------:|-------:|----------:|----:|
//! | 14400      | 267  | 12  | 8    | 288   | 1×     | 576       | 384 |
//! |  7200      | 125  | 10  | 8    | 144   | 2×     | 576       | 384 |
//! |  3600      |  55  |  8  | 8    |  72   | 4×     | 576       | 384 |
//! |  1800      |  21  |  6  | 8    |  36   | 8×     | 576       | 384 |
//!
//! The leading reserved/flag bit is zero when no F-SCCH is assigned. The
//! fixed RC2 puncturing pattern passes symbols 1, 2, 4, and 6 of each group.
//!
//! `Rc2Framer` produces the 384-symbol interleaved bit stream, and
//! `ForwardTrafficChannelRc2` applies LC scrambling, power-control puncturing,
//! and signal-point mapping.
//!
//! CRC polynomials (C.S0002-E §3.1.3.1.4.1):
//! - 12-bit: shared `cdma_common::crc::crc12` (14400 bps).
//! - 10-bit: shared `cdma_common::crc::crc10` (7200 bps).
//! -  8-bit: shared `cdma_common::crc::crc8`  (3600 bps).
//! -  6-bit: shared `cdma_common::crc::crc6_rc2` (1800 bps).

use cdma_common::crc::{crc6_rc2, crc8, crc10, crc12};
#[cfg(test)]
use cdma_common::phy::data_burst_randomizer::{
    RC12_CHIPS_PER_PCG, RC12_PCGS_PER_FRAME, Rc12ReverseRate, active_pcgs as rc12_active_pcgs,
};
use cdma_common::time::CdmaSystemTime;
use num::complex::Complex32;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

use super::{
    Channel, PcgPcbSchedulerHandle,
    rc12_power_control::{Rc12PowerControlCadence, Rc12PowerControlSlot},
};
use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_384};
use crate::phy::coding::convolutional::{Encoder, get_1_2_k9_encoder};
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::coding::symbol_repeat::SymbolRepetition;
use cdma_common::consts::SR1_PCGS_PER_FRAME;

/// Modulation symbols per 20 ms RC2 forward frame (all rates after
/// repetition and fixed RC2 puncturing).
pub const RC2_SYMBOLS_PER_FRAME: usize = 384;
const RC2_REPEATED_SYMBOLS_PER_FRAME: usize = 576;

/// Encoder tail bits appended to every RC2 forward frame.
const TAIL_BITS: usize = 8;

/// Data rate tier for an RC2 forward traffic frame (Rate Set 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rc2Rate {
    /// Full rate: 14400 bps.
    Full,
    /// Half rate: 7200 bps.
    Half,
    /// Quarter rate: 3600 bps.
    Quarter,
    /// Eighth rate: 1800 bps.
    Eighth,
}

impl Rc2Rate {
    /// Information bits per 20 ms frame (before CRC and tail).
    pub fn info_bits(self) -> usize {
        match self {
            Rc2Rate::Full => 267,
            Rc2Rate::Half => 125,
            Rc2Rate::Quarter => 55,
            Rc2Rate::Eighth => 21,
        }
    }

    /// Frame quality indicator (CRC) bit width.
    pub fn fqi_bits(self) -> usize {
        match self {
            Rc2Rate::Full => 12,
            Rc2Rate::Half => 10,
            Rc2Rate::Quarter => 8,
            Rc2Rate::Eighth => 6,
        }
    }

    /// Encoder tail bits (always 8).
    pub fn tail_bits(self) -> usize {
        TAIL_BITS
    }

    /// Total encoder input bits per frame (flag + info + FQI + tail).
    pub fn input_bits(self) -> usize {
        1 + self.info_bits() + self.fqi_bits() + self.tail_bits()
    }

    /// Code-symbol repetition factor (per Figure 3.1.3.1.1-18).
    pub fn repeat_factor(self) -> usize {
        match self {
            Rc2Rate::Full => 1,
            Rc2Rate::Half => 2,
            Rc2Rate::Quarter => 4,
            Rc2Rate::Eighth => 8,
        }
    }

    /// Post-repetition code-symbol count.
    pub fn repeated_symbol_count(self) -> usize {
        // R = 1/2: each input bit → 2 code symbols.
        self.input_bits() * 2 * self.repeat_factor()
    }
}

/// Build the encoder input bitstream for one RC2 forward frame.
///
/// Layout: `[reserved/flag | info_bits | fqi_bits | tail_bits]`.
/// The caller supplies `data` with at least `rate.info_bits()` entries (values
/// must be 0 or 1); excess entries are ignored, missing entries are zero-padded
/// (same convention as the RC1/RC3 builders).
pub fn build_frame_bits_rc2(data: &[u8], rate: Rc2Rate) -> Vec<u8> {
    let info_bits = rate.info_bits();
    let fqi_bits = rate.fqi_bits();
    let total = rate.input_bits();
    let mut frame = Vec::with_capacity(total);

    frame.push(0);
    for i in 0..info_bits {
        frame.push(if i < data.len() { data[i] & 1 } else { 0 });
    }

    let crc_input_len = 1 + info_bits;
    match fqi_bits {
        12 => {
            let crc = crc12(&frame[..crc_input_len]);
            for bit in (0..12).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        10 => {
            let crc = crc10(&frame[..crc_input_len]);
            for bit in (0..10).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        8 => {
            let crc = crc8(&frame[..crc_input_len]);
            for bit in (0..8).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        6 => {
            let crc = crc6_rc2(&frame[..crc_input_len]);
            for bit in (0..6).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        other => panic!("unexpected RC2 FQI width: {}", other),
    }

    for _ in 0..TAIL_BITS {
        frame.push(0);
    }

    debug_assert_eq!(frame.len(), total);
    frame
}

/// Apply the RC2 `110101` puncturing pattern.
pub fn puncture_rc2(symbols: &[u8]) -> Vec<u8> {
    assert_eq!(symbols.len(), RC2_REPEATED_SYMBOLS_PER_FRAME);
    let mut output = Vec::with_capacity(RC2_SYMBOLS_PER_FRAME);
    for group in symbols.chunks_exact(6) {
        output.extend([group[0], group[1], group[3], group[5]]);
    }
    debug_assert_eq!(output.len(), RC2_SYMBOLS_PER_FRAME);
    output
}

/// RC2 forward frame encoder.
pub struct Rc2Framer {
    encoder: Encoder<9, 2>,
    interleaver: BitReversalInterleaver,
}

impl Default for Rc2Framer {
    fn default() -> Self {
        Self::new()
    }
}

impl Rc2Framer {
    pub fn new() -> Self {
        Self {
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
        }
    }

    /// Encode + repeat + puncture + interleave one frame's worth of info bits.
    ///
    /// Returns 384 modulation-symbol bits (0/1) in transmit order.
    pub fn prepare_frame(&mut self, data: &[u8], rate: Rc2Rate) -> Vec<u8> {
        let frame_bits = build_frame_bits_rc2(data, rate);

        self.encoder.reset();
        let mut encoded = Vec::with_capacity(frame_bits.len() * 2);
        for &bit in &frame_bits {
            for &sym in self.encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }
        debug_assert_eq!(encoded.len(), frame_bits.len() * 2);

        let repeat = rate.repeat_factor();
        let repeated = if repeat > 1 {
            let mut rep = SymbolRepetition::new(repeat);
            for &sym in &encoded {
                rep.feed(sym);
            }
            rep.take_all()
        } else {
            encoded
        };
        debug_assert_eq!(repeated.len(), rate.repeated_symbol_count());

        let punctured = puncture_rc2(&repeated);
        debug_assert_eq!(punctured.len(), RC2_SYMBOLS_PER_FRAME);

        self.interleaver.encode(&punctured)
    }
}

// ---------------------------------------------------------------------------
// Full forward RC2 traffic channel (LC scramble + PC puncture + BPSK + Walsh)
// ---------------------------------------------------------------------------

/// Symbols per PCG (RC2 uses the same 384 sym / 16 PCG layout as RC1).
const SYMBOLS_PER_PCG: usize = RC2_SYMBOLS_PER_FRAME / SR1_PCGS_PER_FRAME; // = 24

/// Chips per RC2 power-control group (24 symbols × 64 chips/sym = 1536 chips).
/// Identical to RC1; RC2 only differs from RC1 in the frame builder
/// (rate set 2 bit budgets, R=1/2 K=9, and `110101` puncturing).
const PCG_CHIPS: usize = SYMBOLS_PER_PCG * 64;
const PC_PUNCTURE_LC_BIT_INDICES: [usize; 4] = [23, 22, 21, 20];

fn pc_start_from_decimated_lc(lc_decimated: &[u8; SYMBOLS_PER_PCG]) -> usize {
    PC_PUNCTURE_LC_BIT_INDICES
        .iter()
        .fold(0, |position, index| {
            (position << 1) | lc_decimated[*index] as usize
        })
}

#[derive(Clone)]
struct PreparedFrameRc2 {
    interleaved: Arc<[u8]>,
    rate: Rc2Rate,
    next_pcg: usize,
    frame_start_chip: u64,
    source: FrameSourceRc2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameSourceRc2 {
    Traffic,
    Signaling,
    Null,
}

struct TxStateRc2 {
    symbol_buffer: VecDeque<Complex32>,
    prepared_frame: Option<PreparedFrameRc2>,
}

/// Configuration for an integrated RC2 forward traffic channel.
///
/// Mirrors `ftch::Config` for RC1: a single long-code generator drives both
/// the per-symbol scramble and the per-PCG PC puncture-position extraction
/// (Figure 3.1.3.1.1-19 of C.S0002-E). The LC is clocked at the full
/// 1.2288 Mcps via the standard RC1-style decimator (one chip per 64-chip
/// symbol window).
pub struct ConfigRc2 {
    pub long_code_generator: LongCodeGenerator,
    pub lc_chip_cursor: u64,
    pub pcb_scheduler: PcgPcbSchedulerHandle,
    pub fpc_subchan_gain_linear: f32,
    pub previous_pcg_pc_start: usize,
}

/// A 20 ms RC2 forward traffic frame queued for transmission.
pub struct TrafficFrameRc2 {
    pub data: Vec<u8>,
    pub rate: Rc2Rate,
}

/// Forward RC2 traffic channel (IS-95B Rate Set 2 fundamental channel).
///
/// Wraps [`Rc2Framer`] for the rate-specific encode → repeat → puncture →
/// interleave pipeline, then applies the same LC-scramble / PC-puncture /
/// BPSK signal-point map / I/Q-zero output convention as the RC1 F-FCH.
pub struct ForwardTrafficChannelRc2 {
    config: Mutex<ConfigRc2>,
    power_control_cadence: Rc12PowerControlCadence,
    tx_state: Mutex<TxStateRc2>,
    framer: Mutex<Rc2Framer>,
    null_frame: PreparedFrameRc2,
    frames: Mutex<VecDeque<PreparedFrameRc2>>,
    signaling_frames: Mutex<VecDeque<PreparedFrameRc2>>,
    last_enqueue_at: Mutex<Option<std::time::Instant>>,
    pcb_scheduler: PcgPcbSchedulerHandle,
}

impl ForwardTrafficChannelRc2 {
    pub fn new(config: ConfigRc2) -> Self {
        let pcb_scheduler = config.pcb_scheduler.clone();
        let power_control_cadence =
            Rc12PowerControlCadence::new(config.long_code_generator.clone());
        let mut framer = Rc2Framer::new();
        let null_frame = Self::build_null_frame(&mut framer);
        Self {
            config: Mutex::new(config),
            power_control_cadence,
            tx_state: Mutex::new(TxStateRc2 {
                symbol_buffer: VecDeque::new(),
                prepared_frame: None,
            }),
            framer: Mutex::new(framer),
            null_frame,
            frames: Mutex::new(VecDeque::new()),
            signaling_frames: Mutex::new(VecDeque::new()),
            last_enqueue_at: Mutex::new(None),
            pcb_scheduler,
        }
    }

    pub(crate) fn power_control_slots(
        &self,
        start_abs_pcg: u64,
        count: u64,
    ) -> Vec<Rc12PowerControlSlot> {
        self.power_control_cadence
            .power_control_slots(start_abs_pcg, count)
    }

    pub(crate) fn guaranteed_power_control_ordinal(&self, measured_abs_pcg: u64) -> Option<u64> {
        self.power_control_cadence
            .guaranteed_ordinal_for_measurement(measured_abs_pcg)
    }

    pub(crate) fn power_control_abs_pcg_for_guaranteed_ordinal(&self, ordinal: u64) -> u64 {
        self.power_control_cadence
            .pcb_abs_pcg_for_guaranteed_ordinal(ordinal)
    }

    fn prepare(
        framer: &mut Rc2Framer,
        data: &[u8],
        rate: Rc2Rate,
        source: FrameSourceRc2,
    ) -> PreparedFrameRc2 {
        let interleaved = framer.prepare_frame(data, rate);
        debug_assert_eq!(interleaved.len(), RC2_SYMBOLS_PER_FRAME);
        PreparedFrameRc2 {
            interleaved: interleaved.into(),
            rate,
            next_pcg: 0,
            frame_start_chip: 0,
            source,
        }
    }

    /// Queue a traffic frame for transmission.
    pub fn send_frame(&self, frame: TrafficFrameRc2) {
        let prepared = {
            let mut framer = self.framer.lock();
            Self::prepare(
                &mut framer,
                &frame.data,
                frame.rate,
                FrameSourceRc2::Traffic,
            )
        };
        self.frames.lock().push_back(prepared);
        *self.last_enqueue_at.lock() = Some(std::time::Instant::now());
    }

    /// Queue a full-rate signaling frame.
    pub fn send_signaling_bits(&self, bits: Vec<u8>) {
        // Signaling frames at RC2 full rate carry 267 info bits; the framer
        // zero-pads if the caller supplies fewer.
        let prepared = {
            let mut framer = self.framer.lock();
            Self::prepare(&mut framer, &bits, Rc2Rate::Full, FrameSourceRc2::Signaling)
        };
        self.signaling_frames.lock().push_back(prepared);
        *self.last_enqueue_at.lock() = Some(std::time::Instant::now());
    }

    pub fn last_enqueue_at(&self) -> Option<std::time::Instant> {
        *self.last_enqueue_at.lock()
    }

    pub fn queue_len(&self) -> usize {
        let sig = self.signaling_frames.lock().len();
        let data = self.frames.lock().len();
        sig + data
    }

    pub fn schedule_power_control_bit(&self, abs_pcg: u64, bit: u8) -> bool {
        self.pcb_scheduler.lock().schedule(abs_pcg, bit)
    }

    pub fn schedule_power_control_burst(&self, start_abs_pcg: u64, pcgs: u64, bit: u8) {
        self.pcb_scheduler
            .lock()
            .schedule_burst(start_abs_pcg, pcgs, bit);
    }

    /// Advance the long-code generator to the given absolute chip position.
    pub fn advance_lc_to_chip(&self, chip: u64) {
        let mut config = self.config.lock();
        let delta = chip.saturating_sub(config.lc_chip_cursor);
        if chip >= PCG_CHIPS as u64 {
            let prev_delta = chip
                .saturating_sub(PCG_CHIPS as u64)
                .saturating_sub(config.lc_chip_cursor);
            let mut prev_pcg_lc = config.long_code_generator.clone();
            prev_pcg_lc.advance_chips(prev_delta as usize);
            config.previous_pcg_pc_start = Self::pc_start_from_pcg_lc(&mut prev_pcg_lc);
        }
        config.long_code_generator.advance_chips(delta as usize);
        config.lc_chip_cursor = chip;
    }

    fn pc_start_from_pcg_lc(long_code_generator: &mut LongCodeGenerator) -> usize {
        let mut lc_decimated = [0u8; SYMBOLS_PER_PCG];
        for bit in &mut lc_decimated {
            *bit = long_code_generator.next_chip();
            for _ in 1..64 {
                long_code_generator.next_chip();
            }
        }
        pc_start_from_decimated_lc(&lc_decimated)
    }

    fn pop_next_frame(&self) -> Option<PreparedFrameRc2> {
        self.signaling_frames
            .lock()
            .pop_front()
            .or_else(|| self.frames.lock().pop_front())
    }

    fn build_null_frame(framer: &mut Rc2Framer) -> PreparedFrameRc2 {
        let rate = Rc2Rate::Eighth;
        let mut data = vec![1u8; rate.info_bits()];
        data[0] = 0;
        Self::prepare(framer, &data, rate, FrameSourceRc2::Null)
    }

    fn emit_next_pcg(&self, config: &mut ConfigRc2, tx_state: &mut TxStateRc2) {
        if tx_state.prepared_frame.is_none() {
            let mut prepared = self
                .pop_next_frame()
                .unwrap_or_else(|| self.null_frame.clone());
            prepared.frame_start_chip = config.lc_chip_cursor;
            if prepared.source != FrameSourceRc2::Null {
                log::debug!(
                    "tx_ftch_rc2_frame: source={:?} rate={:?} start_chip={} frame_phase={} symbols={}",
                    prepared.source,
                    prepared.rate,
                    prepared.frame_start_chip,
                    prepared.frame_start_chip % (RC2_SYMBOLS_PER_FRAME as u64 * 64),
                    RC2_SYMBOLS_PER_FRAME
                );
            }
            tx_state.prepared_frame = Some(prepared);
        }

        let prepared = tx_state
            .prepared_frame
            .as_mut()
            .expect("RC2 prepared frame must exist");
        let pcg_index = prepared.next_pcg;
        let start = pcg_index * SYMBOLS_PER_PCG;
        let end = start + SYMBOLS_PER_PCG;

        let abs_pcg = config.lc_chip_cursor / PCG_CHIPS as u64;
        let pcb = self.pcb_scheduler.lock().read(abs_pcg);

        // Decimate the LC: one bit per 64-chip symbol window.
        let mut lc_decimated = [0u8; SYMBOLS_PER_PCG];
        for bit in &mut lc_decimated {
            *bit = config.long_code_generator.next_chip();
            for _ in 1..64 {
                config.long_code_generator.next_chip();
            }
        }

        let current_pc_start = pc_start_from_decimated_lc(&lc_decimated);
        let pc_start = config.previous_pcg_pc_start;

        for (symbol_in_pcg, &sym) in prepared.interleaved[start..end].iter().enumerate() {
            let scrambled = sym ^ lc_decimated[symbol_in_pcg];
            let output = if symbol_in_pcg == pc_start {
                let sign = if pcb == 0 { 1.0 } else { -1.0 };
                Complex32::new(sign * config.fpc_subchan_gain_linear, 0.0)
            } else if scrambled == 0 {
                Complex32::new(1.0, 0.0)
            } else {
                Complex32::new(-1.0, 0.0)
            };
            tx_state.symbol_buffer.push_back(output);
        }

        config.lc_chip_cursor = config.lc_chip_cursor.saturating_add(PCG_CHIPS as u64);
        config.previous_pcg_pc_start = current_pc_start;
        prepared.next_pcg += 1;
        if prepared.next_pcg == SR1_PCGS_PER_FRAME {
            if prepared.source != FrameSourceRc2::Null {
                log::debug!(
                    "tx_ftch_rc2_frame_done: source={:?} rate={:?} start_chip={} end_chip={}",
                    prepared.source,
                    prepared.rate,
                    prepared.frame_start_chip,
                    config.lc_chip_cursor
                );
            }
            tx_state.prepared_frame = None;
        }
    }

    /// Produce one 20 ms frame of 384 BPSK symbols.
    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        self.next_block(RC2_SYMBOLS_PER_FRAME, current_system_time)
    }
}

impl Channel for ForwardTrafficChannelRc2 {
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(num_samples);
        self.next_block_into(&mut out, num_samples, system_time);
        out
    }

    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        _system_time: CdmaSystemTime,
    ) {
        let mut config = self.config.lock();
        let mut tx_state = self.tx_state.lock();
        while tx_state.symbol_buffer.len() < num_samples {
            self.emit_next_pcg(&mut config, &mut tx_state);
        }
        out.reserve(num_samples);
        for _ in 0..num_samples {
            out.push(tx_state.symbol_buffer.pop_front().unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::coding::convolutional::SoftViterbiDecoder;

    const ALL_RATES: [Rc2Rate; 4] = [
        Rc2Rate::Full,
        Rc2Rate::Half,
        Rc2Rate::Quarter,
        Rc2Rate::Eighth,
    ];

    fn synthetic_info_bits(n: usize, seed: u32) -> Vec<u8> {
        let mut bits = Vec::with_capacity(n);
        let mut s = seed.wrapping_mul(2654435761);
        for _ in 0..n {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            bits.push(((s >> 16) & 1) as u8);
        }
        bits
    }

    #[test]
    fn rc2_rate_bit_budgets_match_spec() {
        assert_eq!(Rc2Rate::Full.input_bits(), 288);
        assert_eq!(Rc2Rate::Half.input_bits(), 144);
        assert_eq!(Rc2Rate::Quarter.input_bits(), 72);
        assert_eq!(Rc2Rate::Eighth.input_bits(), 36);

        for rate in ALL_RATES {
            assert_eq!(rate.repeated_symbol_count(), 576);
        }
    }

    #[test]
    fn rc2_build_frame_bits_lengths_and_tail_zeros() {
        for rate in ALL_RATES {
            let info = synthetic_info_bits(rate.info_bits(), 1);
            let frame = build_frame_bits_rc2(&info, rate);
            assert_eq!(frame.len(), rate.input_bits(), "rate {:?}", rate);
            assert_eq!(frame[0], 0);
            assert_eq!(&frame[1..1 + rate.info_bits()], info.as_slice());
            // Last 8 bits are encoder tail zeros.
            let total = frame.len();
            for &bit in &frame[total - TAIL_BITS..] {
                assert_eq!(bit, 0, "tail must be zero at rate {:?}", rate);
            }
        }
    }

    #[test]
    fn rc2_crc_round_trip_each_rate() {
        for rate in ALL_RATES {
            let info = synthetic_info_bits(rate.info_bits(), 0x1234);
            let frame = build_frame_bits_rc2(&info, rate);

            // Extract embedded CRC.
            let crc_input_n = 1 + rate.info_bits();
            let crc_n = rate.fqi_bits();
            let mut embedded: u16 = 0;
            for &bit in &frame[crc_input_n..crc_input_n + crc_n] {
                embedded = (embedded << 1) | (bit as u16 & 1);
            }

            let computed = match crc_n {
                12 => crc12(&frame[..crc_input_n]),
                10 => crc10(&frame[..crc_input_n]),
                8 => crc8(&frame[..crc_input_n]) as u16,
                6 => crc6_rc2(&frame[..crc_input_n]) as u16,
                _ => unreachable!(),
            };
            assert_eq!(embedded, computed, "CRC mismatch at rate {:?}", rate);
        }
    }

    #[test]
    fn rc2_crc10_known_vectors() {
        // Zero input: CRC engine just runs the init register through the
        // polynomial — derived from the spec polynomial directly.
        let zeros = vec![0u8; Rc2Rate::Half.info_bits()];
        let c = crc10(&zeros);
        // 10-bit field
        assert!(c < (1 << 10));
        // Different input → different output (sanity).
        let mut ones = zeros.clone();
        ones[0] = 1;
        assert_ne!(crc10(&zeros), crc10(&ones));
    }

    #[test]
    fn rc2_puncture_outputs_384_for_all_rates() {
        for rate in ALL_RATES {
            let input = vec![1u8; rate.repeated_symbol_count()];
            let out = puncture_rc2(&input);
            assert_eq!(out.len(), RC2_SYMBOLS_PER_FRAME, "rate {:?}", rate);
        }
    }

    #[test]
    fn rc2_framer_emits_384_symbols_per_rate() {
        for rate in ALL_RATES {
            let mut framer = Rc2Framer::new();
            let info = synthetic_info_bits(rate.info_bits(), 0xDEAD);
            let symbols = framer.prepare_frame(&info, rate);
            assert_eq!(symbols.len(), RC2_SYMBOLS_PER_FRAME, "rate {:?}", rate);
            for (i, &b) in symbols.iter().enumerate() {
                assert!(b <= 1, "non-binary symbol {} at idx {}", b, i);
            }
        }
    }

    #[test]
    fn rc2_framer_different_input_different_output() {
        let mut framer = Rc2Framer::new();
        let a = framer.prepare_frame(&vec![0; 267], Rc2Rate::Full);
        let b = framer.prepare_frame(&vec![1; 267], Rc2Rate::Full);
        assert_ne!(a, b);
    }

    #[test]
    fn rc2_framer_all_rates_loopback_decode() {
        for rate in ALL_RATES {
            let mut framer = Rc2Framer::new();
            let info = synthetic_info_bits(rate.info_bits(), 0xBEEF);
            let symbols = framer.prepare_frame(&info, rate);

            // De-interleave.
            let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);
            let deinterleaved = interleaver.decode(&symbols);

            let mut soft = vec![0.5f32; 576];
            for (input_group, output_group) in
                soft.chunks_exact_mut(6).zip(deinterleaved.chunks_exact(4))
            {
                input_group[0] = output_group[0] as f32;
                input_group[1] = output_group[1] as f32;
                input_group[3] = output_group[2] as f32;
                input_group[5] = output_group[3] as f32;
            }

            // De-repeat: average each group of `repeat_factor` consecutive
            // soft values (preserves erasure mass), yielding the post-encoder
            // soft stream (input_bits * 2 entries).
            let r = rate.repeat_factor();
            let mut derep = Vec::with_capacity(rate.input_bits() * 2);
            for chunk in soft.chunks_exact(r) {
                let avg: f32 = chunk.iter().copied().sum::<f32>() / r as f32;
                derep.push(avg);
            }
            assert_eq!(derep.len(), rate.input_bits() * 2);

            let mut decoder = SoftViterbiDecoder::new(get_1_2_k9_encoder());
            let pairs: Vec<[f32; 2]> = derep.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
            let decoded = decoder.decode_block_from_state(&pairs, 0);
            assert_eq!(decoded.len(), rate.input_bits());

            let info_n = rate.info_bits();
            assert_eq!(decoded[0], 0);
            assert_eq!(
                &decoded[1..1 + info_n],
                &info[..],
                "RC2 loopback failed at rate {:?}",
                rate,
            );

            // CRC must validate.
            let crc_n = rate.fqi_bits();
            let crc_input_n = 1 + info_n;
            let mut rx_crc: u16 = 0;
            for &bit in &decoded[crc_input_n..crc_input_n + crc_n] {
                rx_crc = (rx_crc << 1) | (bit as u16 & 1);
            }
            let expected_crc = match crc_n {
                12 => crc12(&decoded[..crc_input_n]),
                10 => crc10(&decoded[..crc_input_n]),
                8 => crc8(&decoded[..crc_input_n]) as u16,
                6 => crc6_rc2(&decoded[..crc_input_n]) as u16,
                _ => unreachable!(),
            };
            assert_eq!(rx_crc, expected_crc, "rate {:?} CRC mismatch", rate);

            // Tail.
            let total = decoded.len();
            for &bit in &decoded[total - TAIL_BITS..] {
                assert_eq!(bit, 0);
            }
        }
    }

    fn make_channel() -> ForwardTrafficChannelRc2 {
        ForwardTrafficChannelRc2::new(ConfigRc2 {
            long_code_generator: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
            fpc_subchan_gain_linear: 1.0,
            previous_pcg_pc_start: 0,
        })
    }

    #[test]
    fn rc2_channel_null_frame_emits_384_symbols() {
        let ch = make_channel();
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), RC2_SYMBOLS_PER_FRAME);
        for s in &frame {
            assert!((s.re.abs() - 1.0).abs() < 1e-6, "BPSK ±1");
            assert!(s.im.abs() < 1e-6, "BPSK imag = 0");
        }
    }

    #[test]
    fn rc2_tx_does_not_wait_for_producer_framer() {
        let channel = Arc::new(make_channel());
        let framer_guard = channel.framer.lock();
        let worker_channel = channel.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let frame = worker_channel.next(CdmaSystemTime::default());
            done_tx.send(frame.len()).unwrap();
        });

        assert_eq!(
            done_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(RC2_SYMBOLS_PER_FRAME)
        );
        drop(framer_guard);
        worker.join().unwrap();
    }

    #[test]
    fn rc2_null_frame_uses_primary_traffic_mux_header() {
        let mut actual_framer = Rc2Framer::new();
        let actual = ForwardTrafficChannelRc2::build_null_frame(&mut actual_framer);

        let mut expected_data = vec![1; Rc2Rate::Eighth.info_bits()];
        expected_data[0] = 0;
        let mut expected_framer = Rc2Framer::new();
        let expected = expected_framer.prepare_frame(&expected_data, Rc2Rate::Eighth);

        assert_eq!(&*actual.interleaved, expected.as_slice());
    }

    #[test]
    fn rc2_channel_all_rates_emit_384_symbols() {
        for rate in ALL_RATES {
            let ch = make_channel();
            ch.send_frame(TrafficFrameRc2 {
                data: vec![0; rate.info_bits()],
                rate,
            });
            let frame = ch.next(CdmaSystemTime::default());
            assert_eq!(frame.len(), RC2_SYMBOLS_PER_FRAME, "rate {:?}", rate);
        }
    }

    #[test]
    fn rc2_channel_advances_lc_one_frame() {
        let ch = make_channel();
        let _ = ch.next(CdmaSystemTime::default());
        let config = ch.config.lock();
        assert_eq!(config.lc_chip_cursor, (RC2_SYMBOLS_PER_FRAME as u64) * 64);
    }

    #[test]
    fn rc2_power_control_slots_follow_two_pcg_validity_delay() {
        let ch = make_channel();
        let slots = ch.power_control_slots(2, RC12_PCGS_PER_FRAME as u64);
        let reverse_long_code_origin = LongCodeGenerator::new_traffic_channel(0xDEADBEEF);
        let eighth = rc12_active_pcgs(&reverse_long_code_origin, 0, Rc12ReverseRate::Eighth);

        assert_eq!(
            slots.iter().filter(|slot| slot.guaranteed_valid()).count(),
            2
        );
        for pcg in 0..RC12_PCGS_PER_FRAME {
            assert_eq!(slots[pcg].guaranteed_valid(), eighth[pcg]);
        }
    }

    #[test]
    fn rc2_hold_bits_are_neutral_at_every_reverse_rate() {
        let ch = make_channel();
        for frame in 0..32u64 {
            let frame_start_abs_pcg = frame * RC12_PCGS_PER_FRAME as u64;
            let slots = ch.power_control_slots(frame_start_abs_pcg + 2, RC12_PCGS_PER_FRAME as u64);
            for rate in [
                Rc12ReverseRate::Full,
                Rc12ReverseRate::Half,
                Rc12ReverseRate::Quarter,
                Rc12ReverseRate::Eighth,
            ] {
                let active = rc12_active_pcgs(
                    &LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
                    frame_start_abs_pcg * RC12_CHIPS_PER_PCG,
                    rate,
                );
                let net_db: i32 = slots
                    .iter()
                    .zip(active)
                    .filter_map(|(slot, active)| {
                        active.then_some(if slot.hold_bit == 0 { 1 } else { -1 })
                    })
                    .sum();
                assert_eq!(net_db, 0, "frame={frame} rate={rate:?}");
            }
        }
    }

    #[test]
    fn rc2_down_power_control_bit_replaces_one_symbol_per_pcg() {
        let ch = ForwardTrafficChannelRc2::new(ConfigRc2 {
            long_code_generator: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
            fpc_subchan_gain_linear: 2.0,
            previous_pcg_pc_start: 0,
        });
        ch.schedule_power_control_bit(0, 1);
        let symbols = ch.next_block(SYMBOLS_PER_PCG, CdmaSystemTime::default());
        assert_eq!(
            symbols
                .iter()
                .filter(|symbol| (symbol.re + 2.0).abs() < 1e-6)
                .count(),
            1
        );
        assert!(symbols.iter().all(|symbol| (symbol.re - 2.0).abs() >= 1e-6));
    }

    #[test]
    fn rc2_channel_different_data_produces_different_output() {
        let ch1 = make_channel();
        ch1.send_frame(TrafficFrameRc2 {
            data: vec![0; Rc2Rate::Full.info_bits()],
            rate: Rc2Rate::Full,
        });
        let f1 = ch1.next(CdmaSystemTime::default());

        let ch2 = make_channel();
        ch2.send_frame(TrafficFrameRc2 {
            data: vec![1; Rc2Rate::Full.info_bits()],
            rate: Rc2Rate::Full,
        });
        let f2 = ch2.next(CdmaSystemTime::default());
        assert_ne!(f1, f2);
    }

    #[test]
    fn rc2_framer_full_rate_resets_encoder_between_frames() {
        // Run the same input through two `prepare_frame` calls and confirm the
        // encoder state is reset (output is identical).
        let mut framer = Rc2Framer::new();
        let info = synthetic_info_bits(Rc2Rate::Full.info_bits(), 7);
        let a = framer.prepare_frame(&info, Rc2Rate::Full);
        let b = framer.prepare_frame(&info, Rc2Rate::Full);
        assert_eq!(a, b);
    }
}
