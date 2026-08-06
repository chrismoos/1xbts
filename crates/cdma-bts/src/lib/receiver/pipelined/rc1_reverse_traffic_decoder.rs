use std::collections::{HashMap, VecDeque};

use cdma_common::crc::{crc8, crc12};
use log::info;
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock, raw_to_soft};
use crate::phy::coding::block_interleaver::{
    Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
};
use crate::phy::coding::convolutional::get_1_3_k9_soft_viterbi_decoder;
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::walsh::WalshGenerator;
use crate::receiver::pipelined::traffic_channel_processor::{
    ReverseMux1SignalingLayout, parse_reverse_mux1_full_rate_format,
};

use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME,
    RC1_SYMBOLS_PER_PCG, RC1_WALSH_CHIPS_PER_SYMBOL, SR1_PCGS_PER_FRAME,
};

const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;
const SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const SOFT_BITS_PER_PCG: usize = RC1_SYMBOLS_PER_PCG * RC1_SOFT_BITS_PER_SYMBOL;
const PCG_CHIPS: usize = RC1_SYMBOLS_PER_PCG * PN_CHIPS_PER_SYMBOL;
const PREAMBLE_NULL_FRAME_THRESHOLD: usize = 16;
const RATE_HISTORY_LEN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecoderState {
    Preamble,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rc1TrafficRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl Rc1TrafficRate {
    const SEARCH_ORDER: [Self; 4] = [Self::Full, Self::Half, Self::Quarter, Self::Eighth];

    const fn to_interleaver_rate(self) -> Rc12ReverseTrafficRate {
        match self {
            Self::Full => Rc12ReverseTrafficRate::Full,
            Self::Half => Rc12ReverseTrafficRate::Half,
            Self::Quarter => Rc12ReverseTrafficRate::Quarter,
            Self::Eighth => Rc12ReverseTrafficRate::Eighth,
        }
    }

    const fn repetition_factor(self) -> usize {
        self.to_interleaver_rate().repetition_factor()
    }

    const fn frame_bits(self) -> usize {
        match self {
            Self::Full => 192,
            Self::Half => 96,
            Self::Quarter => 48,
            Self::Eighth => 24,
        }
    }

    const fn info_bits(self) -> usize {
        match self {
            Self::Full => 172,
            Self::Half => 80,
            Self::Quarter => 40,
            Self::Eighth => 16,
        }
    }

    const fn fqi_bits(self) -> usize {
        match self {
            Self::Full => 12,
            Self::Half => 8,
            Self::Quarter | Self::Eighth => 0,
        }
    }

    const fn tail_bits(self) -> usize {
        8
    }

    const fn rate_bps(self) -> usize {
        match self {
            Self::Full => 9600,
            Self::Half => 4800,
            Self::Quarter => 2400,
            Self::Eighth => 1200,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameValidation {
    fqi_valid: bool,
    tail_valid: bool,
    phy_valid: bool,
}

impl FrameValidation {
    fn for_rate(rate: Rc1TrafficRate, bits: &[u8]) -> Self {
        if bits.len() < rate.frame_bits() {
            return Self {
                fqi_valid: false,
                tail_valid: false,
                phy_valid: false,
            };
        }
        let tail_start = rate.frame_bits() - rate.tail_bits();
        let tail_valid = bits[tail_start..rate.frame_bits()]
            .iter()
            .all(|bit| *bit == 0);
        if !tail_valid {
            return Self {
                fqi_valid: false,
                tail_valid: false,
                phy_valid: false,
            };
        }
        let fqi_valid = match rate {
            Rc1TrafficRate::Full => {
                let computed = crc12(&bits[..rate.info_bits()]);
                let mut received: u16 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit as u16 & 1);
                }
                computed == received
            }
            Rc1TrafficRate::Half => {
                let computed = crc8(&bits[..rate.info_bits()]);
                let mut received: u8 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit & 1);
                }
                computed == received
            }
            Rc1TrafficRate::Quarter | Rc1TrafficRate::Eighth => true,
        };
        Self {
            fqi_valid,
            tail_valid,
            phy_valid: tail_valid && fqi_valid,
        }
    }
}

#[derive(Clone, Debug)]
struct DecodedTrafficFrame {
    rate: Rc1TrafficRate,
    bits: Vec<u8>,
    validation: FrameValidation,
    ml_terminal_matches_zero: bool,
}

/// RC1 reverse traffic decoder using absolute 20ms frame boundary alignment.
///
/// Simplified replacement for `Rc1TrafficFrameAligner`. Instead of searching
/// over all chip_phase/frame_phase hypotheses, this decoder anchors directly
/// on the 256-chip Walsh symbol grid and 24576-chip frame grid from the
/// absolute chip counter provided by the PnLcCorrelator.
///
/// State machine:
///   Preamble → Locked
///
/// - **Preamble**: decode at sub-rates on known frame boundaries, count
///   consecutive null frames, emit preamble event at threshold.
/// - **Locked**: multi-rate adaptive decode, emit decoded frames + per-PCG
///   Eb/Nt measurements for closed-loop power control.
pub struct Rc1ReverseTrafficDecoder {
    state: DecoderState,
    esn: u32,

    // Sample accumulation (chip-rate)
    pending_samples: VecDeque<Complex32>,
    pending_chip_start: Option<usize>,

    // Current frame accumulation
    frame_soft: Vec<f32>,
    frame_chip_start: usize,
    frame_symbol_count: usize,
    /// Per-symbol 64-bin Walsh energies for Eb/Nt computation.
    symbol_energies: Vec<[f32; RC1_WALSH_CHIPS_PER_SYMBOL]>,

    // Preamble state
    consecutive_null_frames: usize,
    preamble_event_sent: bool,

    // Locked state
    locked_rate: Option<Rc1TrafficRate>,
    locked_mux_layout: Option<ReverseMux1SignalingLayout>,
    rate_history: [Rc1TrafficRate; RATE_HISTORY_LEN],
    rate_history_count: usize,

    // PCG measurement tracking
    next_measurement_abs_pcg: Option<u64>,
    pcg_measurement_rate: Option<Rc1TrafficRate>,
    last_processing_absolute_chip_end: Option<u64>,

    // Propagated tags from upstream blocks
    tags: HashMap<&'static str, i64>,
    sample_rate_hz: f64,

    // Metrics
    symbols_decoded: u64,
    frames_decoded: u64,
}

impl Rc1ReverseTrafficDecoder {
    pub fn new(esn: u32) -> Self {
        Self {
            state: DecoderState::Preamble,
            esn,
            pending_samples: VecDeque::new(),
            pending_chip_start: None,
            frame_soft: Vec::with_capacity(SOFT_BITS_PER_FRAME),
            frame_chip_start: 0,
            frame_symbol_count: 0,
            symbol_energies: Vec::with_capacity(RC1_SYMBOLS_PER_FRAME),
            consecutive_null_frames: 0,
            preamble_event_sent: false,
            locked_rate: None,
            locked_mux_layout: None,
            rate_history: [Rc1TrafficRate::Full; RATE_HISTORY_LEN],
            rate_history_count: 0,
            next_measurement_abs_pcg: None,
            pcg_measurement_rate: None,
            last_processing_absolute_chip_end: None,
            tags: HashMap::new(),
            sample_rate_hz: 0.0,
            symbols_decoded: 0,
            frames_decoded: 0,
        }
    }

    // ---------------------------------------------------------------
    // Symbol demodulation (from ReverseAccessDecoder pattern)
    // ---------------------------------------------------------------

    fn symbol_energies_from_chips(chips: &[Complex32]) -> [f32; RC1_WALSH_CHIPS_PER_SYMBOL] {
        debug_assert_eq!(chips.len(), PN_CHIPS_PER_SYMBOL);
        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for walsh_chip_idx in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
            let base = walsh_chip_idx * RC1_PN_CHIPS_PER_WALSH_CHIP;
            walsh_chips[walsh_chip_idx] = chips[base..base + RC1_PN_CHIPS_PER_WALSH_CHIP]
                .iter()
                .copied()
                .sum::<Complex32>();
        }
        WalshGenerator::fwht_fixed(&mut walsh_chips);
        std::array::from_fn(|i| walsh_chips[i].norm_sqr())
    }

    fn soft_bits_from_energies(
        energies: &[f32; RC1_WALSH_CHIPS_PER_SYMBOL],
    ) -> [f32; RC1_SOFT_BITS_PER_SYMBOL] {
        let mut out = [0.0f32; RC1_SOFT_BITS_PER_SYMBOL];
        for bit in 0..RC1_SOFT_BITS_PER_SYMBOL {
            let mut max_zero = f32::NEG_INFINITY;
            let mut max_one = f32::NEG_INFINITY;
            for (row, &energy) in energies.iter().enumerate() {
                if ((row >> bit) & 1) == 0 {
                    max_zero = max_zero.max(energy);
                } else {
                    max_one = max_one.max(energy);
                }
            }
            out[bit] = max_zero - max_one;
        }
        out
    }

    // ---------------------------------------------------------------
    // Frame decoding
    // ---------------------------------------------------------------

    fn apply_pcg_mask_at(
        &self,
        raw_soft: &[f32],
        rate: Rc1TrafficRate,
        frame_chip_start: usize,
    ) -> Vec<f32> {
        let mut masked = raw_soft.to_vec();
        let active_pcgs = self.exact_active_pcgs_for_rate(rate, frame_chip_start);
        for (pcg_idx, active) in active_pcgs.iter().copied().enumerate() {
            if active {
                continue;
            }
            let start = pcg_idx * SOFT_BITS_PER_PCG;
            let end = start + SOFT_BITS_PER_PCG;
            masked[start..end].fill(0.0);
        }
        masked
    }

    fn collapse_repetition(deinterleaved: &[f32], repetition_factor: usize) -> Vec<f32> {
        deinterleaved
            .chunks_exact(repetition_factor)
            .map(|chunk| chunk.iter().sum::<f32>() / repetition_factor as f32)
            .collect()
    }

    fn decode_bits(collapsed: &[f32]) -> (Vec<u8>, u8) {
        let peak = collapsed.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let inputs = collapsed
            .chunks_exact(3)
            .map(|chunk| {
                [
                    raw_to_soft(chunk[0], inv_peak),
                    raw_to_soft(chunk[1], inv_peak),
                    raw_to_soft(chunk[2], inv_peak),
                ]
            })
            .collect::<Vec<_>>();
        let mut decoder = get_1_3_k9_soft_viterbi_decoder();
        let bits = decoder.decode_block_from_state(&inputs, 0);
        let ml_best_state = decoder.ml_best_terminal_state() as u8;
        (bits, ml_best_state)
    }

    fn decode_frame_soft(&self, frame_soft: &[f32], rate: Rc1TrafficRate) -> DecodedTrafficFrame {
        self.decode_frame_soft_at(frame_soft, rate, self.frame_chip_start)
    }

    fn decode_frame_soft_at(
        &self,
        frame_soft: &[f32],
        rate: Rc1TrafficRate,
        frame_chip_start: usize,
    ) -> DecodedTrafficFrame {
        let masked_soft = self.apply_pcg_mask_at(frame_soft, rate, frame_chip_start);
        let interleaver = Rc12ReverseTrafficInterleaver::new(rate.to_interleaver_rate());
        let deinterleaved = interleaver.decode_soft(&masked_soft);
        let collapsed = Self::collapse_repetition(&deinterleaved, rate.repetition_factor());
        let (bits, ml_best_state) = Self::decode_bits(&collapsed);
        let validation = FrameValidation::for_rate(rate, &bits);
        DecodedTrafficFrame {
            rate,
            bits,
            validation,
            ml_terminal_matches_zero: ml_best_state == 0,
        }
    }

    // ---------------------------------------------------------------
    // Multi-rate adaptive decode
    // ---------------------------------------------------------------

    fn record_rate(&mut self, rate: Rc1TrafficRate) {
        let idx = self.rate_history_count % RATE_HISTORY_LEN;
        self.rate_history[idx] = rate;
        self.rate_history_count = self.rate_history_count.saturating_add(1);
        self.locked_rate = Some(rate);
    }

    fn adaptive_search_order(&self) -> [Rc1TrafficRate; 4] {
        if self.rate_history_count == 0 {
            return Rc1TrafficRate::SEARCH_ORDER;
        }
        let window = &self.rate_history[..self.rate_history_count.min(RATE_HISTORY_LEN)];
        let mut counts = [0u32; 4];
        for &r in window {
            let idx = match r {
                Rc1TrafficRate::Full => 0,
                Rc1TrafficRate::Half => 1,
                Rc1TrafficRate::Quarter => 2,
                Rc1TrafficRate::Eighth => 3,
            };
            counts[idx] += 1;
        }
        let mut order = Rc1TrafficRate::SEARCH_ORDER;
        order.sort_by(|a, b| {
            let ai = match a {
                Rc1TrafficRate::Full => 0,
                Rc1TrafficRate::Half => 1,
                Rc1TrafficRate::Quarter => 2,
                Rc1TrafficRate::Eighth => 3,
            };
            let bi = match b {
                Rc1TrafficRate::Full => 0,
                Rc1TrafficRate::Half => 1,
                Rc1TrafficRate::Quarter => 2,
                Rc1TrafficRate::Eighth => 3,
            };
            counts[bi].cmp(&counts[ai])
        });
        order
    }

    fn score_no_fqi_candidate(&self, decoded: &DecodedTrafficFrame) -> usize {
        let has_nonzero = decoded.bits.iter().any(|bit| *bit != 0);
        let mut score = 0usize;
        if Some(decoded.rate) == self.locked_rate {
            score += 1000;
        }
        if decoded.ml_terminal_matches_zero {
            score += 500;
        }
        if has_nonzero {
            score += 50;
        }
        score += match decoded.rate {
            Rc1TrafficRate::Quarter => 20,
            Rc1TrafficRate::Eighth => 10,
            Rc1TrafficRate::Full | Rc1TrafficRate::Half => 0,
        };
        score
    }

    fn choose_best_rate(&self, frame_soft: &[f32]) -> Option<DecodedTrafficFrame> {
        let adaptive_order = self.adaptive_search_order();

        // CRC-bearing rates are authoritative when they pass FQI.
        for rate in adaptive_order {
            if rate.fqi_bits() == 0 {
                continue;
            }
            let decoded = self.decode_frame_soft(frame_soft, rate);
            if decoded.validation.phy_valid {
                return Some(decoded);
            }
        }

        // No-FQI rates must both be tried before choosing. Tail bits alone are
        // too weak at low power; require the Viterbi terminal state expected by
        // the all-zero encoder tail before the frame can count as decoded.
        let mut best: Option<(usize, DecodedTrafficFrame)> = None;
        for rate in [Rc1TrafficRate::Quarter, Rc1TrafficRate::Eighth] {
            let decoded = self.decode_frame_soft(frame_soft, rate);
            if !Self::no_fqi_candidate_acceptable(&decoded) {
                continue;
            }
            let score = self.score_no_fqi_candidate(&decoded);
            let replace = best
                .as_ref()
                .map(|(best_score, _)| score > *best_score)
                .unwrap_or(true);
            if replace {
                best = Some((score, decoded));
            }
        }
        best.map(|(_, decoded)| decoded)
    }

    fn no_fqi_candidate_acceptable(decoded: &DecodedTrafficFrame) -> bool {
        decoded.validation.phy_valid && decoded.ml_terminal_matches_zero
    }

    // ---------------------------------------------------------------
    // PCG gating (long-code randomizer)
    // ---------------------------------------------------------------

    fn lc_randomizer_bits(&self, frame_chip_start: usize) -> [u8; 14] {
        let mut generator = LongCodeGenerator::new_traffic_channel(self.esn);
        let offset = frame_chip_start.saturating_sub(1536 + 14);
        generator.advance_chips(offset);
        let mut bits = [0u8; 14];
        for bit in &mut bits {
            *bit = generator.next_chip();
        }
        bits
    }

    fn exact_active_pcgs_for_rate(
        &self,
        rate: Rc1TrafficRate,
        frame_chip_start: usize,
    ) -> [bool; SR1_PCGS_PER_FRAME] {
        let mut active = [false; SR1_PCGS_PER_FRAME];
        if rate == Rc1TrafficRate::Full {
            active.fill(true);
            return active;
        }
        let b = self.lc_randomizer_bits(frame_chip_start);
        match rate {
            Rc1TrafficRate::Half => {
                for i in 0..8usize {
                    active[2 * i + b[i] as usize] = true;
                }
            }
            Rc1TrafficRate::Quarter => {
                active[if b[8] == 0 { b[0] } else { 2 + b[1] } as usize] = true;
                active[(if b[9] == 0 { 4 + b[2] } else { 6 + b[3] }) as usize] = true;
                active[(if b[10] == 0 { 8 + b[4] } else { 10 + b[5] }) as usize] = true;
                active[(if b[11] == 0 { 12 + b[6] } else { 14 + b[7] }) as usize] = true;
            }
            Rc1TrafficRate::Eighth => {
                let lower = if b[12] == 0 {
                    if b[8] == 0 {
                        b[0] as usize
                    } else {
                        2 + b[1] as usize
                    }
                } else if b[9] == 0 {
                    4 + b[2] as usize
                } else {
                    6 + b[3] as usize
                };
                let upper = if b[13] == 0 {
                    if b[10] == 0 {
                        8 + b[4] as usize
                    } else {
                        10 + b[5] as usize
                    }
                } else if b[11] == 0 {
                    12 + b[6] as usize
                } else {
                    14 + b[7] as usize
                };
                active[lower] = true;
                active[upper] = true;
            }
            Rc1TrafficRate::Full => {}
        }
        active
    }

    fn active_pcgs_for_rate(&self, rate: Rc1TrafficRate) -> [bool; SR1_PCGS_PER_FRAME] {
        self.exact_active_pcgs_for_rate(rate, self.frame_chip_start)
    }

    // ---------------------------------------------------------------
    // Per-PCG Eb/Nt computation
    // ---------------------------------------------------------------

    fn pcg_eb_nt_db(&self, pcg_idx: usize) -> f32 {
        let sym_start = pcg_idx * RC1_SYMBOLS_PER_PCG;
        if sym_start >= self.symbol_energies.len() {
            return -30.0;
        }
        let sym_end = (sym_start + RC1_SYMBOLS_PER_PCG).min(self.symbol_energies.len());
        let n = sym_end - sym_start;
        if n == 0 {
            return -30.0;
        }
        let mut linear_eb_nt = 0.0f32;
        for sym_idx in sym_start..sym_end {
            let energies = &self.symbol_energies[sym_idx];
            let peak = energies.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let total: f32 = energies.iter().sum();
            let noise_sum = (total - peak).max(0.0);
            let denom = (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32;
            let noise_mean = (noise_sum / denom).max(1e-12);
            let es_nt = (peak / noise_mean - 1.0).max(0.0);
            linear_eb_nt += (es_nt * 0.5).max(1e-9);
        }
        let linear_mean = linear_eb_nt / n as f32;
        10.0 * linear_mean.max(1e-9).log10()
    }

    fn pcg_snr_db_for_frame(&self) -> Vec<f32> {
        (0..SR1_PCGS_PER_FRAME)
            .map(|pcg| self.pcg_eb_nt_db(pcg))
            .collect()
    }

    fn pcg_mobile_power_dbfs(&self, pcg: usize) -> f32 {
        let start = pcg * RC1_SYMBOLS_PER_PCG;
        let end = start + RC1_SYMBOLS_PER_PCG;
        super::walsh64_mobile_power_dbfs(self.symbol_energies[start..end].iter())
    }

    /// Emit per-PCG Eb/Nt measurements incrementally as each 1.25ms PCG
    /// completes (6 symbols buffered). Called after every symbol so the
    /// measurement reaches the BSC within ~1 PCG of real-time, matching
    /// the RC3 path and keeping `power_control_delay_pcgs=2` viable.
    fn emit_pcg_measurements(&mut self) -> Vec<SampleBlock> {
        if self.state != DecoderState::Locked {
            return Vec::new();
        }
        let Some(processing_end) = self.last_processing_absolute_chip_end else {
            return Vec::new();
        };

        // PCG boundary: chip % 1536 == 0.  We track measurement progress
        // via next_measurement_abs_pcg.  Each completed PCG = 6 symbols
        // in symbol_energies at the right offset within the frame.
        let frame_abs_pcg = (self.frame_chip_start as u64) / PCG_CHIPS as u64;
        let symbols_available = self.symbol_energies.len();
        let pcgs_available = symbols_available / RC1_SYMBOLS_PER_PCG;
        if pcgs_available == 0 {
            return Vec::new();
        }
        let available_end_pcg = frame_abs_pcg + pcgs_available as u64;

        let mut next_pcg = self.next_measurement_abs_pcg.unwrap_or(frame_abs_pcg);
        if next_pcg < frame_abs_pcg {
            next_pcg = frame_abs_pcg;
        }
        if next_pcg >= available_end_pcg {
            return Vec::new();
        }

        // Active PCG mask depends on rate; cache per frame.
        let rate = self.pcg_measurement_rate.unwrap_or(Rc1TrafficRate::Full);
        let active_mask = self.exact_active_pcgs_for_rate(rate, self.frame_chip_start);

        let mut out = Vec::new();
        while next_pcg < available_end_pcg {
            let pcg_in_frame = (next_pcg - frame_abs_pcg) as usize;
            if pcg_in_frame >= SR1_PCGS_PER_FRAME {
                break;
            }
            next_pcg += 1;
            if !active_mask[pcg_in_frame] {
                continue;
            }
            let measurement_abs_chip =
                frame_abs_pcg * PCG_CHIPS as u64 + pcg_in_frame as u64 * PCG_CHIPS as u64;
            let age_chips = processing_end.saturating_sub(measurement_abs_chip);
            let eb_nt_db = self.pcg_eb_nt_db(pcg_in_frame);
            let mobile_power_dbfs = self.pcg_mobile_power_dbfs(pcg_in_frame);

            let mut block = SampleBlock::new(Vec::new(), measurement_abs_chip as usize)
                .with_sample_rate_hz(self.sample_rate_hz);
            block.tags = self.tags.clone();
            block
                .tags
                .insert("absolute_chip_start", measurement_abs_chip as i64);
            block.tags.insert("traffic_pcg_measurement", 1);
            block
                .tags
                .insert("traffic_measurement_age_chips", age_chips as i64);
            block.tags.insert(
                "traffic_pcg_mobile_power_mdbfs",
                (mobile_power_dbfs * 1000.0) as i64,
            );
            block.pcg_signal_snr_db = Some(vec![eb_nt_db]);
            out.push(block);
        }
        self.next_measurement_abs_pcg = Some(next_pcg);
        out
    }

    // ---------------------------------------------------------------
    // Frame handling
    // ---------------------------------------------------------------

    fn handle_complete_frame(&mut self) -> Vec<SampleBlock> {
        if self.frame_soft.len() < SOFT_BITS_PER_FRAME {
            return Vec::new();
        }

        let mut out = Vec::new();

        // Preamble tracking: count consecutive null frames for preamble event.
        if self.state == DecoderState::Preamble {
            let is_null = [
                Rc1TrafficRate::Eighth,
                Rc1TrafficRate::Quarter,
                Rc1TrafficRate::Half,
            ]
            .iter()
            .any(|&rate| {
                let decoded = self.decode_frame_soft(&self.frame_soft, rate);
                decoded.validation.tail_valid
            }) || {
                let decoded = self.decode_frame_soft(&self.frame_soft, Rc1TrafficRate::Full);
                decoded.validation.tail_valid && decoded.bits.iter().all(|bit| *bit == 0)
            };

            if is_null {
                self.consecutive_null_frames += 1;
            } else {
                self.consecutive_null_frames = 0;
            }

            if self.consecutive_null_frames >= PREAMBLE_NULL_FRAME_THRESHOLD {
                info!(
                    "rc1_reverse_traffic_decoder: preamble detected after {} null frames esn=0x{:08X} frame_chip_start={}",
                    self.consecutive_null_frames, self.esn, self.frame_chip_start,
                );

                if !self.preamble_event_sent {
                    let mut block = SampleBlock::new(Vec::new(), self.frame_chip_start)
                        .with_sample_rate_hz(self.sample_rate_hz);
                    block.tags = self.tags.clone();
                    block.tags.insert("traffic_preamble_detected", 1);
                    block.tags.insert(
                        "traffic_preamble_frames",
                        self.consecutive_null_frames as i64,
                    );
                    block
                        .tags
                        .insert("absolute_chip_start", self.frame_chip_start as i64);
                    out.push(block);
                    self.preamble_event_sent = true;
                }

                self.state = DecoderState::Locked;
                self.next_measurement_abs_pcg =
                    Some(self.frame_chip_start as u64 / PCG_CHIPS as u64);
            }
        }

        // Always decode and emit the frame (even during preamble), so we
        // don't miss signaling frames that overlap with the preamble window.
        out.extend(self.handle_locked_frame());
        out
    }

    fn handle_locked_frame(&mut self) -> Vec<SampleBlock> {
        let frame_soft = self.frame_soft.clone();
        let Some(decoded) = self.choose_best_rate(&frame_soft) else {
            return vec![self.build_failed_locked_frame()];
        };

        self.frames_decoded += 1;
        self.record_rate(decoded.rate);
        if decoded.validation.phy_valid {
            self.pcg_measurement_rate = Some(decoded.rate);
        }

        let is_preamble =
            decoded.rate == Rc1TrafficRate::Full && decoded.bits.iter().all(|bit| *bit == 0);

        // Build output tags
        let mut tags = self.tags.clone();
        tags.insert("traffic_decoded_frame", 1);
        tags.insert("traffic_frame_aligned", 1);
        tags.insert("traffic_walsh_locked", 1);
        tags.insert("traffic_rate_bps", decoded.rate.rate_bps() as i64);
        tags.insert("traffic_info_bits", decoded.rate.info_bits() as i64);
        tags.insert("traffic_fqi_bits", decoded.rate.fqi_bits() as i64);
        tags.insert("traffic_tail_bits", decoded.rate.tail_bits() as i64);
        tags.insert("traffic_fqi_valid", decoded.validation.fqi_valid as i64);
        tags.insert("traffic_tail_valid", decoded.validation.tail_valid as i64);
        tags.insert("traffic_phy_valid", decoded.validation.phy_valid as i64);
        tags.insert(
            "traffic_ml_tail_match",
            decoded.ml_terminal_matches_zero as i64,
        );
        tags.insert("traffic_is_preamble", is_preamble as i64);
        if let Some(layout) = self.locked_mux_layout {
            tags.insert("traffic_mux_signaling_layout", layout.tag_value());
        }
        tags.insert("absolute_chip_start", self.frame_chip_start as i64);
        if decoded.rate == Rc1TrafficRate::Full
            && decoded.bits.len() >= decoded.rate.info_bits()
            && let Some(format) =
                parse_reverse_mux1_full_rate_format(&decoded.bits[..decoded.rate.info_bits()])
        {
            tags.insert("traffic_mux_header", format.mux_header as i64);
            tags.insert("traffic_mux_header_bits", format.header_bits as i64);
            tags.insert("traffic_mux_primary_bits", format.primary_bits as i64);
            tags.insert("traffic_mux_signaling_bits", format.signaling_bits as i64);
        }

        let samples = decoded
            .bits
            .iter()
            .take(decoded.rate.frame_bits())
            .map(|&bit| Complex32::new(bit as f32, 0.0))
            .collect::<Vec<_>>();

        // Per-PCG Eb/Nt and active PCG mask (exact from long-code randomizer)
        let pcg_snr_db = Some(self.pcg_snr_db_for_frame());
        let active_pcg_mask = Some(self.active_pcgs_for_rate(decoded.rate));

        let mut block = SampleBlock::new(samples, self.frame_chip_start)
            .with_sample_rate_hz(self.sample_rate_hz);
        block.tags = tags;
        block.pcg_signal_snr_db = pcg_snr_db;
        block.active_pcg_mask = active_pcg_mask;

        // PCG measurements are emitted incrementally in process_symbols()
        // as each 1.25ms PCG completes, so no batch emit needed here.
        vec![block]
    }

    fn build_failed_locked_frame(&self) -> SampleBlock {
        let mut tags = self.tags.clone();
        tags.insert("traffic_decoded_frame", 1);
        tags.insert("traffic_frame_aligned", 1);
        tags.insert("traffic_walsh_locked", 1);
        tags.insert("traffic_rate_bps", 0);
        tags.insert("traffic_info_bits", 0);
        tags.insert("traffic_fqi_bits", 0);
        tags.insert(
            "traffic_tail_bits",
            Rc1TrafficRate::Eighth.tail_bits() as i64,
        );
        tags.insert("traffic_fqi_valid", 0);
        tags.insert("traffic_tail_valid", 0);
        tags.insert("traffic_phy_valid", 0);
        tags.insert("traffic_ml_tail_match", 0);
        tags.insert("traffic_is_preamble", 0);
        tags.insert("absolute_chip_start", self.frame_chip_start as i64);

        let mut block = SampleBlock::new(Vec::new(), self.frame_chip_start)
            .with_sample_rate_hz(self.sample_rate_hz);
        block.tags = tags;
        block.pcg_signal_snr_db = Some(self.pcg_snr_db_for_frame());
        block.active_pcg_mask = Some([true; SR1_PCGS_PER_FRAME]);
        block
    }

    // ---------------------------------------------------------------
    // Symbol processing loop
    // ---------------------------------------------------------------

    fn process_symbols(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();

        while self.pending_samples.len() >= PN_CHIPS_PER_SYMBOL {
            let chip_start = self.pending_chip_start.unwrap_or(0);
            let symbol_in_frame = (chip_start / PN_CHIPS_PER_SYMBOL) % RC1_SYMBOLS_PER_FRAME;

            // Wait for a real frame boundary before starting accumulation.
            // The finger may start mid-frame; skip those leading symbols.
            if self.frame_symbol_count == 0 && symbol_in_frame != 0 {
                self.pending_samples.drain(..PN_CHIPS_PER_SYMBOL);
                self.pending_chip_start = Some(chip_start + PN_CHIPS_PER_SYMBOL);
                self.symbols_decoded += 1;
                continue;
            }

            // Frame boundary — process the previous complete frame
            if symbol_in_frame == 0 && self.frame_symbol_count > 0 {
                if self.frame_symbol_count == RC1_SYMBOLS_PER_FRAME {
                    out.extend(self.handle_complete_frame());
                }
                self.frame_soft.clear();
                self.symbol_energies.clear();
                self.frame_symbol_count = 0;
            }

            if self.frame_symbol_count == 0 {
                self.frame_chip_start = chip_start;
            }

            // Demodulate the symbol
            let samples: Vec<Complex32> =
                self.pending_samples.drain(..PN_CHIPS_PER_SYMBOL).collect();
            let energies = Self::symbol_energies_from_chips(&samples);
            let soft_bits = Self::soft_bits_from_energies(&energies);

            self.frame_soft.extend_from_slice(&soft_bits);
            self.symbol_energies.push(energies);
            self.frame_symbol_count += 1;
            self.symbols_decoded += 1;
            self.pending_chip_start = Some(chip_start + PN_CHIPS_PER_SYMBOL);

            // Advance the processing frontier so PCG measurement age
            // reflects the symbol we just consumed, not the block end.
            // This matches the RC3 pattern where age ≈ 0 because the
            // measurement is emitted from the same processing position.
            self.last_processing_absolute_chip_end =
                Some((chip_start + PN_CHIPS_PER_SYMBOL) as u64);

            // Emit PCG measurement as soon as each 1.25ms PCG completes
            // (every 6 symbols on a PCG boundary).
            if self.frame_symbol_count % RC1_SYMBOLS_PER_PCG == 0 {
                out.extend(self.emit_pcg_measurements());
            }
        }

        out
    }
}

impl PipelineProcessor for Rc1ReverseTrafficDecoder {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.pending_chip_start.is_none() {
            self.pending_chip_start = Some(block.chip_start);
        }
        self.sample_rate_hz = block.sample_rate_hz;
        self.tags = block.tags.clone();

        let block_abs_end = block
            .tags
            .get("absolute_chip_start")
            .copied()
            .and_then(|v| u64::try_from(v).ok())
            .map(|start| start + block.samples.len() as u64);
        if let Some(end) = block_abs_end {
            self.last_processing_absolute_chip_end = Some(end);
        }

        self.pending_samples.extend(block.samples);

        // Align to 256-chip symbol grid
        if let Some(chip_start) = self.pending_chip_start {
            let rem = chip_start % PN_CHIPS_PER_SYMBOL;
            if rem != 0 {
                let skip_chips = PN_CHIPS_PER_SYMBOL - rem;
                if self.pending_samples.len() < skip_chips {
                    return Vec::new();
                }
                self.pending_samples.drain(..skip_chips);
                self.pending_chip_start = Some(chip_start + skip_chips);
            }
        }

        self.process_symbols()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        // Process any remaining complete frame
        if self.frame_symbol_count == RC1_SYMBOLS_PER_FRAME {
            let out = self.handle_complete_frame();
            self.frame_soft.clear();
            self.symbol_energies.clear();
            self.frame_symbol_count = 0;
            return out;
        }
        Vec::new()
    }

    fn name(&self) -> &'static str {
        "Rc1ReverseTrafficDecoder"
    }

    fn metrics(&self) -> Vec<(&'static str, String)> {
        let state = match self.state {
            DecoderState::Preamble => "preamble",
            DecoderState::Locked => "locked",
        };
        vec![
            ("state", state.to_string()),
            ("symbols", self.symbols_decoded.to_string()),
            ("frames", self.frames_decoded.to_string()),
            ("preamble_nulls", self.consecutive_null_frames.to_string()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_fqi_frame(phy_valid: bool, ml_terminal_matches_zero: bool) -> DecodedTrafficFrame {
        DecodedTrafficFrame {
            rate: Rc1TrafficRate::Eighth,
            bits: vec![0; Rc1TrafficRate::Eighth.frame_bits()],
            validation: FrameValidation {
                fqi_valid: true,
                tail_valid: phy_valid,
                phy_valid,
            },
            ml_terminal_matches_zero,
        }
    }

    #[test]
    fn no_fqi_candidate_requires_phy_valid_and_ml_terminal_zero() {
        assert!(Rc1ReverseTrafficDecoder::no_fqi_candidate_acceptable(
            &no_fqi_frame(true, true)
        ));
        assert!(!Rc1ReverseTrafficDecoder::no_fqi_candidate_acceptable(
            &no_fqi_frame(true, false)
        ));
        assert!(!Rc1ReverseTrafficDecoder::no_fqi_candidate_acceptable(
            &no_fqi_frame(false, true)
        ));
    }

    #[test]
    fn failed_locked_frame_emits_invalid_decoded_frame_for_fer_counting() {
        let mut decoder = Rc1ReverseTrafficDecoder::new(0x1234_5678);
        decoder.frame_chip_start = 24_576;
        decoder.sample_rate_hz = 1_228_800.0;
        decoder.tags.insert("traffic_walsh_code", 10);

        let block = decoder.build_failed_locked_frame();

        assert_eq!(block.chip_start, 24_576);
        assert_eq!(block.sample_rate_hz, 1_228_800.0);
        assert_eq!(block.tags.get("traffic_decoded_frame"), Some(&1));
        assert_eq!(block.tags.get("traffic_frame_aligned"), Some(&1));
        assert_eq!(block.tags.get("traffic_phy_valid"), Some(&0));
        assert_eq!(block.tags.get("traffic_tail_valid"), Some(&0));
        assert_eq!(block.tags.get("traffic_fqi_valid"), Some(&0));
        assert_eq!(block.tags.get("traffic_fqi_bits"), Some(&0));
        assert_eq!(block.tags.get("traffic_rate_bps"), Some(&0));
        assert_eq!(block.tags.get("absolute_chip_start"), Some(&24_576));
        assert!(block.samples.is_empty());
        assert_eq!(block.pcg_signal_snr_db.as_ref().map(Vec::len), Some(16));
        assert_eq!(block.active_pcg_mask, Some([true; SR1_PCGS_PER_FRAME]));
    }

    #[test]
    fn production_rc1_pcg_measurement_includes_mobile_power() {
        let mut decoder = Rc1ReverseTrafficDecoder::new(0x1234_5678);
        decoder.state = DecoderState::Locked;
        decoder.frame_chip_start = 0;
        decoder.sample_rate_hz = 1_228_800.0;
        decoder.last_processing_absolute_chip_end = Some(PCG_CHIPS as u64);
        decoder.next_measurement_abs_pcg = Some(0);
        decoder.pcg_measurement_rate = Some(Rc1TrafficRate::Full);
        decoder.symbol_energies = (0..RC1_SYMBOLS_PER_PCG)
            .map(|_| {
                let mut energies = [1.0; RC1_WALSH_CHIPS_PER_SYMBOL];
                energies[17] = 10_000.0;
                energies
            })
            .collect();

        let measurements = decoder.emit_pcg_measurements();

        assert_eq!(measurements.len(), 1);
        assert!(
            measurements[0]
                .tags
                .contains_key("traffic_pcg_mobile_power_mdbfs")
        );
    }
}
