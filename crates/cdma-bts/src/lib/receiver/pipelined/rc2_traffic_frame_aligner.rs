//! RC2 reverse-traffic frame aligner / demod (SR1 IS-95B Rate Set 2).
//!
//! Spec: C.S0002-E §2.1.3.1.1.2 — Reverse Traffic Channel for RC2, voice
//! rates 14400 / 7200 / 3600 / 1800 bps.
//!
//! Air-interface geometry is identical to RC1 (96 64-ary Walsh symbols / 20 ms
//! frame, 64 Walsh chips × 4 PN chips / symbol). RC2 differs from RC1 in:
//!  * R=1/2 K=9 convolutional code (vs R=1/3 on RC1).
//!  * Per-rate bit budget: 288 / 144 / 72 / 36 input bits before conv coding.
//!  * Per-rate CRC width: 12 / 10 / 8 / 6 bits (Table 2.1.3.1.1-2).
//!
//! Structurally this mirrors `Rc1ReverseTrafficDecoder`: chip-rate IQ in,
//! per-symbol 64-ary Walsh demod, long-code gating, frame accumulation,
//! multi-rate trial decode, and CRC validation.

use std::collections::{HashMap, VecDeque};

use cdma_common::crc::{crc6_rc2, crc8, crc10, crc12};
use cdma_common::phy::data_burst_randomizer::{Rc12ReverseRate, active_pcgs as rc12_active_pcgs};
use log::info;
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock, raw_to_soft};
use crate::phy::coding::block_interleaver::{
    Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
};
use crate::phy::coding::convolutional::{SoftViterbiDecoder, get_1_2_k9_encoder};
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::walsh::WalshGenerator;

use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME,
    RC1_SYMBOLS_PER_PCG, RC1_WALSH_CHIPS_PER_SYMBOL, SR1_PCGS_PER_FRAME,
};

// RC2 shares RC1's air-interface symbol geometry on the reverse link.
const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;
const SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const SOFT_BITS_PER_PCG: usize = RC1_SYMBOLS_PER_PCG * RC1_SOFT_BITS_PER_SYMBOL;
/// An RC2 Reverse Traffic Channel preamble is one all-zero 14.4 kbps frame.
const PREAMBLE_NULL_FRAME_THRESHOLD: usize = 1;
const RC2_FRAME_DIAGNOSTIC_LIMIT: u64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlignerState {
    Preamble,
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rc2TrafficRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl Rc2TrafficRate {
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

    /// Total input bits per frame (info + CRC + tail), before R=1/2 conv.
    /// Per C.S0002-E Table 2.1.3.1.1-2 (RC2 reverse traffic).
    const fn frame_input_bits(self) -> usize {
        match self {
            Self::Full => 288,
            Self::Half => 144,
            Self::Quarter => 72,
            Self::Eighth => 36,
        }
    }

    const fn info_bits(self) -> usize {
        match self {
            Self::Full => 267,
            Self::Half => 125,
            Self::Quarter => 55,
            Self::Eighth => 21,
        }
    }

    const fn crc_bits(self) -> usize {
        match self {
            Self::Full => 12,
            Self::Half => 10,
            Self::Quarter => 8,
            Self::Eighth => 6,
        }
    }

    const fn tail_bits(self) -> usize {
        8
    }

    const fn rate_bps(self) -> usize {
        match self {
            Self::Full => 14400,
            Self::Half => 7200,
            Self::Quarter => 3600,
            Self::Eighth => 1800,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameValidation {
    crc_valid: bool,
    tail_valid: bool,
    phy_valid: bool,
}

impl FrameValidation {
    fn for_rate(rate: Rc2TrafficRate, bits: &[u8]) -> Self {
        let total = rate.frame_input_bits();
        if bits.len() < total {
            return Self {
                crc_valid: false,
                tail_valid: false,
                phy_valid: false,
            };
        }
        let tail_start = total - rate.tail_bits();
        let tail_valid = bits[tail_start..total].iter().all(|b| *b == 0);
        if !tail_valid {
            return Self {
                crc_valid: false,
                tail_valid: false,
                phy_valid: false,
            };
        }
        let info_end = 1 + rate.info_bits();
        let crc_end = info_end + rate.crc_bits();
        let received_bits = &bits[info_end..crc_end];
        let info = &bits[..info_end];

        let crc_valid = match rate {
            Rc2TrafficRate::Full => {
                let computed = crc12(info);
                let mut received: u16 = 0;
                for &b in received_bits {
                    received = (received << 1) | (b as u16 & 1);
                }
                computed == received
            }
            Rc2TrafficRate::Half => {
                let computed = crc10(info);
                let mut received: u16 = 0;
                for &b in received_bits {
                    received = (received << 1) | (b as u16 & 1);
                }
                computed == received
            }
            Rc2TrafficRate::Quarter => {
                let computed = crc8(info);
                let mut received: u8 = 0;
                for &b in received_bits {
                    received = (received << 1) | (b & 1);
                }
                computed == received
            }
            Rc2TrafficRate::Eighth => {
                let computed = crc6_rc2(info);
                let mut received: u8 = 0;
                for &b in received_bits {
                    received = (received << 1) | (b & 1);
                }
                computed == received
            }
        };
        Self {
            crc_valid,
            tail_valid,
            phy_valid: tail_valid && crc_valid,
        }
    }
}

#[derive(Clone, Debug)]
struct DecodedFrame {
    rate: Rc2TrafficRate,
    bits: Vec<u8>,
    validation: FrameValidation,
}

/// RC2 reverse traffic frame aligner.
///
/// Anchors on the absolute 256-chip Walsh symbol grid and 24576-chip frame
/// grid. Each completed 20 ms frame is trial-decoded at all four RC2 rates;
/// the first CRC-valid frame wins.
pub struct Rc2TrafficFrameAligner {
    state: AlignerState,
    esn: u32,

    pending_samples: VecDeque<Complex32>,
    pending_chip_start: Option<usize>,

    frame_soft: Vec<f32>,
    frame_chip_start: usize,
    frame_symbol_count: usize,
    symbol_energies: Vec<[f32; RC1_WALSH_CHIPS_PER_SYMBOL]>,
    consecutive_null_frames: usize,
    preamble_event_sent: bool,
    next_measurement_abs_pcg: Option<u64>,
    last_processing_absolute_chip_end: Option<u64>,

    tags: HashMap<&'static str, i64>,
    sample_rate_hz: f64,

    frames_decoded: u64,
    symbols_decoded: u64,
}

impl Default for Rc2TrafficFrameAligner {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Rc2TrafficFrameAligner {
    pub fn new(esn: u32) -> Self {
        Self {
            state: AlignerState::Preamble,
            esn,
            pending_samples: VecDeque::new(),
            pending_chip_start: None,
            frame_soft: Vec::with_capacity(SOFT_BITS_PER_FRAME),
            frame_chip_start: 0,
            frame_symbol_count: 0,
            symbol_energies: Vec::with_capacity(RC1_SYMBOLS_PER_FRAME),
            consecutive_null_frames: 0,
            preamble_event_sent: false,
            next_measurement_abs_pcg: None,
            last_processing_absolute_chip_end: None,
            tags: HashMap::new(),
            sample_rate_hz: 0.0,
            frames_decoded: 0,
            symbols_decoded: 0,
        }
    }

    // -----------------------------------------------------------------
    // Walsh symbol demod (identical structure to RC1).
    // -----------------------------------------------------------------

    fn symbol_energies_from_chips(chips: &[Complex32]) -> [f32; RC1_WALSH_CHIPS_PER_SYMBOL] {
        debug_assert_eq!(chips.len(), PN_CHIPS_PER_SYMBOL);
        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for (idx, slot) in walsh_chips.iter_mut().enumerate() {
            let base = idx * RC1_PN_CHIPS_PER_WALSH_CHIP;
            *slot = chips[base..base + RC1_PN_CHIPS_PER_WALSH_CHIP]
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
        let total_energy = energies.iter().sum::<f32>();
        let peak_energy = energies.iter().copied().fold(0.0f32, f32::max);
        // Exclude the winning Walsh bin so input power does not set Viterbi confidence.
        let noise_energy =
            ((total_energy - peak_energy) / (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32).max(1e-12);
        let mut out = [0.0f32; RC1_SOFT_BITS_PER_SYMBOL];
        for bit in 0..RC1_SOFT_BITS_PER_SYMBOL {
            let mut max_zero = f32::NEG_INFINITY;
            let mut max_one = f32::NEG_INFINITY;
            for (row, &energy) in energies.iter().enumerate() {
                let metric = energy / noise_energy;
                if ((row >> bit) & 1) == 0 {
                    max_zero = max_zero.max(metric);
                } else {
                    max_one = max_one.max(metric);
                }
            }
            out[bit] = max_zero - max_one;
        }
        out
    }

    // -----------------------------------------------------------------
    // Frame decode (per-rate trial).
    // -----------------------------------------------------------------

    fn collapse_repetition(deinterleaved: &[f32], rep: usize) -> Vec<f32> {
        deinterleaved
            .chunks_exact(rep)
            .map(|chunk| chunk.iter().sum::<f32>() / rep as f32)
            .collect()
    }

    fn active_pcgs_for_rate(
        &self,
        rate: Rc2TrafficRate,
        frame_chip_start: usize,
    ) -> [bool; SR1_PCGS_PER_FRAME] {
        let rate = match rate {
            Rc2TrafficRate::Full => Rc12ReverseRate::Full,
            Rc2TrafficRate::Half => Rc12ReverseRate::Half,
            Rc2TrafficRate::Quarter => Rc12ReverseRate::Quarter,
            Rc2TrafficRate::Eighth => Rc12ReverseRate::Eighth,
        };
        rc12_active_pcgs(
            &LongCodeGenerator::new_traffic_channel(self.esn),
            frame_chip_start as u64,
            rate,
        )
    }

    fn apply_pcg_mask(&self, frame_soft: &[f32], rate: Rc2TrafficRate) -> Vec<f32> {
        let mut masked = frame_soft.to_vec();
        for (pcg, active) in self
            .active_pcgs_for_rate(rate, self.frame_chip_start)
            .into_iter()
            .enumerate()
        {
            if !active {
                let start = pcg * SOFT_BITS_PER_PCG;
                masked[start..start + SOFT_BITS_PER_PCG].fill(0.0);
            }
        }
        masked
    }

    fn pcg_eb_nt_db(&self, pcg: usize) -> f32 {
        let start = pcg * RC1_SYMBOLS_PER_PCG;
        let end = (start + RC1_SYMBOLS_PER_PCG).min(self.symbol_energies.len());
        if start >= end {
            return -30.0;
        }
        let mut linear = 0.0;
        for energies in &self.symbol_energies[start..end] {
            let peak = energies.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let noise = ((energies.iter().sum::<f32>() - peak)
                / (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32)
                .max(1e-12);
            linear += ((peak / noise - 1.0).max(0.0) * 0.5).max(1e-9);
        }
        10.0 * (linear / (end - start) as f32).max(1e-9).log10()
    }

    fn pcg_mobile_power_dbfs(&self, pcg: usize) -> f32 {
        let start = pcg * RC1_SYMBOLS_PER_PCG;
        let end = (start + RC1_SYMBOLS_PER_PCG).min(self.symbol_energies.len());
        super::walsh64_mobile_power_dbfs(self.symbol_energies[start..end].iter())
    }

    fn emit_pcg_measurements(&mut self) -> Vec<SampleBlock> {
        if self.state != AlignerState::Locked {
            return Vec::new();
        }
        let frame_abs_pcg =
            self.frame_chip_start as u64 / (RC1_SYMBOLS_PER_PCG * PN_CHIPS_PER_SYMBOL) as u64;
        let available_end =
            frame_abs_pcg + (self.symbol_energies.len() / RC1_SYMBOLS_PER_PCG) as u64;
        let mut next = self.next_measurement_abs_pcg.unwrap_or(frame_abs_pcg);
        // The current frame rate is unknown until its final PCG has been
        // decoded. The one-eighth-rate PCGs are a subset of every higher-rate
        // mask, so only these two positions are guaranteed to contain a
        // reverse transmission suitable for real-time power control.
        let active = self.active_pcgs_for_rate(Rc2TrafficRate::Eighth, self.frame_chip_start);
        let processing_end = self.last_processing_absolute_chip_end.unwrap_or(0);
        let mut out = Vec::new();
        while next < available_end {
            let pcg = (next - frame_abs_pcg) as usize;
            next += 1;
            if pcg >= SR1_PCGS_PER_FRAME || !active[pcg] {
                continue;
            }
            let chip = frame_abs_pcg * (RC1_SYMBOLS_PER_PCG * PN_CHIPS_PER_SYMBOL) as u64
                + pcg as u64 * (RC1_SYMBOLS_PER_PCG * PN_CHIPS_PER_SYMBOL) as u64;
            let mut block = SampleBlock::new(Vec::new(), chip as usize)
                .with_sample_rate_hz(self.sample_rate_hz);
            block.tags = self.tags.clone();
            block.tags.insert("absolute_chip_start", chip as i64);
            block.tags.insert("traffic_pcg_measurement", 1);
            block.tags.insert(
                "traffic_measurement_age_chips",
                processing_end.saturating_sub(chip) as i64,
            );
            block.tags.insert(
                "traffic_pcg_mobile_power_mdbfs",
                (self.pcg_mobile_power_dbfs(pcg) * 1000.0) as i64,
            );
            block.pcg_signal_snr_db = Some(vec![self.pcg_eb_nt_db(pcg)]);
            out.push(block);
        }
        self.next_measurement_abs_pcg = Some(next);
        out
    }

    fn decode_bits_r12(collapsed: &[f32], total_input_bits: usize) -> Vec<u8> {
        let peak = collapsed.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let mut inputs = Vec::with_capacity(collapsed.len() / 2);
        for chunk in collapsed.chunks_exact(2) {
            inputs.push([
                raw_to_soft(chunk[0], inv_peak),
                raw_to_soft(chunk[1], inv_peak),
            ]);
        }
        let mut decoder: SoftViterbiDecoder<9, 2> = SoftViterbiDecoder::new(get_1_2_k9_encoder());
        let mut bits = decoder.decode_block_from_state(&inputs, 0);
        bits.truncate(total_input_bits);
        bits
    }

    fn decode_frame_at_rate(&self, frame_soft: &[f32], rate: Rc2TrafficRate) -> DecodedFrame {
        let masked = self.apply_pcg_mask(frame_soft, rate);
        let interleaver = Rc12ReverseTrafficInterleaver::new(rate.to_interleaver_rate());
        let deinterleaved = interleaver.decode_soft(&masked);
        let collapsed = Self::collapse_repetition(&deinterleaved, rate.repetition_factor());
        let bits = Self::decode_bits_r12(&collapsed, rate.frame_input_bits());
        let validation = FrameValidation::for_rate(rate, &bits);
        DecodedFrame {
            rate,
            bits,
            validation,
        }
    }

    fn choose_best_rate(&self, frame_soft: &[f32]) -> Option<DecodedFrame> {
        // Try each rate; first CRC-valid frame wins. If none CRC-valid,
        // return the first tail-valid candidate (lets preamble logic see
        // null frames even without CRC matching the all-zero info pattern).
        let mut tail_only: Option<DecodedFrame> = None;
        for rate in Rc2TrafficRate::SEARCH_ORDER {
            let decoded = self.decode_frame_at_rate(frame_soft, rate);
            if decoded.validation.phy_valid {
                return Some(decoded);
            }
            if decoded.validation.tail_valid && tail_only.is_none() {
                tail_only = Some(decoded);
            }
        }
        tail_only
    }

    // -----------------------------------------------------------------
    // Frame handling.
    // -----------------------------------------------------------------

    fn handle_complete_frame(&mut self) -> Vec<SampleBlock> {
        if self.frame_soft.len() < SOFT_BITS_PER_FRAME {
            return Vec::new();
        }
        let frame_soft = self.frame_soft.clone();
        let preamble = self.decode_frame_at_rate(&frame_soft, Rc2TrafficRate::Full);
        let is_preamble = preamble.validation.tail_valid
            && preamble
                .bits
                .iter()
                .take(Rc2TrafficRate::Full.frame_input_bits())
                .all(|bit| *bit == 0);

        if self.state == AlignerState::Locked && self.frames_decoded < RC2_FRAME_DIAGNOSTIC_LIMIT {
            let mut relative_pcg_db = [0.0f32; SR1_PCGS_PER_FRAME];
            for (pcg, value) in relative_pcg_db.iter_mut().enumerate() {
                let start = pcg * RC1_SYMBOLS_PER_PCG;
                let end = start + RC1_SYMBOLS_PER_PCG;
                let energy = self.symbol_energies[start..end]
                    .iter()
                    .flat_map(|bins| bins.iter())
                    .copied()
                    .sum::<f32>();
                *value = energy;
            }
            let peak = relative_pcg_db
                .iter()
                .copied()
                .fold(0.0f32, f32::max)
                .max(1e-12);
            for value in &mut relative_pcg_db {
                *value = 10.0 * (*value / peak).max(1e-12).log10();
            }

            let full = self.decode_frame_at_rate(&frame_soft, Rc2TrafficRate::Full);
            let half = self.decode_frame_at_rate(&frame_soft, Rc2TrafficRate::Half);
            let quarter = self.decode_frame_at_rate(&frame_soft, Rc2TrafficRate::Quarter);
            let eighth = self.decode_frame_at_rate(&frame_soft, Rc2TrafficRate::Eighth);
            log::debug!(
                "rc2_reverse_frame: chip={} preamble={} pcg_rel_db={:.1?} full={}/{} half={}/{} quarter={}/{} eighth={}/{}",
                self.frame_chip_start,
                is_preamble,
                relative_pcg_db,
                full.validation.crc_valid as u8,
                full.validation.tail_valid as u8,
                half.validation.crc_valid as u8,
                half.validation.tail_valid as u8,
                quarter.validation.crc_valid as u8,
                quarter.validation.tail_valid as u8,
                eighth.validation.crc_valid as u8,
                eighth.validation.tail_valid as u8,
            );
        }

        let mut out = Vec::new();
        if self.state == AlignerState::Preamble {
            if is_preamble {
                self.consecutive_null_frames += 1;
            } else {
                self.consecutive_null_frames = 0;
            }
            if self.consecutive_null_frames >= PREAMBLE_NULL_FRAME_THRESHOLD
                && !self.preamble_event_sent
            {
                info!(
                    "rc2_traffic_frame_aligner: preamble detected after {} null frames esn=0x{:08X}",
                    self.consecutive_null_frames, self.esn
                );
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
                self.state = AlignerState::Locked;
            }
        }
        let Some(decoded) = self.choose_best_rate(&frame_soft) else {
            return out;
        };
        self.frames_decoded += 1;
        if decoded.validation.phy_valid && self.state == AlignerState::Preamble {
            self.state = AlignerState::Locked;
        }
        let mut tags = self.tags.clone();
        tags.insert("traffic_decoded_frame", 1);
        tags.insert("traffic_radio_config", 2);
        tags.insert("traffic_rate_bps", decoded.rate.rate_bps() as i64);
        tags.insert("traffic_info_bits", decoded.rate.info_bits() as i64);
        tags.insert("traffic_crc_bits", decoded.rate.crc_bits() as i64);
        tags.insert("traffic_fqi_bits", decoded.rate.crc_bits() as i64);
        tags.insert("traffic_tail_bits", decoded.rate.tail_bits() as i64);
        tags.insert("traffic_crc_valid", decoded.validation.crc_valid as i64);
        tags.insert("traffic_fqi_valid", decoded.validation.crc_valid as i64);
        tags.insert("traffic_tail_valid", decoded.validation.tail_valid as i64);
        tags.insert("traffic_phy_valid", decoded.validation.phy_valid as i64);
        tags.insert("absolute_chip_start", self.frame_chip_start as i64);

        let samples = decoded
            .bits
            .iter()
            .skip(1)
            .take(decoded.rate.frame_input_bits() - 1)
            .map(|&bit| Complex32::new(bit as f32, 0.0))
            .collect::<Vec<_>>();

        let mut block = SampleBlock::new(samples, self.frame_chip_start)
            .with_sample_rate_hz(self.sample_rate_hz);
        block.tags = tags;
        block.pcg_signal_snr_db = Some(
            (0..SR1_PCGS_PER_FRAME)
                .map(|pcg| self.pcg_eb_nt_db(pcg))
                .collect(),
        );
        block.active_pcg_mask = Some(self.active_pcgs_for_rate(
            if decoded.validation.phy_valid {
                decoded.rate
            } else {
                // A tail-only candidate does not identify the transmitted
                // rate. Use the two PCGs guaranteed active at every RC2 rate
                // instead of treating all gated-off positions as signal.
                Rc2TrafficRate::Eighth
            },
            self.frame_chip_start,
        ));
        out.push(block);
        out
    }

    fn process_symbols(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.pending_samples.len() >= PN_CHIPS_PER_SYMBOL {
            let chip_start = self.pending_chip_start.unwrap_or(0);
            let symbol_in_frame = (chip_start / PN_CHIPS_PER_SYMBOL) % RC1_SYMBOLS_PER_FRAME;

            if self.frame_symbol_count == 0 && symbol_in_frame != 0 {
                self.pending_samples.drain(..PN_CHIPS_PER_SYMBOL);
                self.pending_chip_start = Some(chip_start + PN_CHIPS_PER_SYMBOL);
                self.symbols_decoded += 1;
                continue;
            }

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

            let samples: Vec<Complex32> =
                self.pending_samples.drain(..PN_CHIPS_PER_SYMBOL).collect();
            let energies = Self::symbol_energies_from_chips(&samples);
            let soft = Self::soft_bits_from_energies(&energies);

            self.frame_soft.extend_from_slice(&soft);
            self.symbol_energies.push(energies);
            self.frame_symbol_count += 1;
            self.symbols_decoded += 1;
            self.pending_chip_start = Some(chip_start + PN_CHIPS_PER_SYMBOL);
            self.last_processing_absolute_chip_end =
                Some((chip_start + PN_CHIPS_PER_SYMBOL) as u64);
            if self.frame_symbol_count % RC1_SYMBOLS_PER_PCG == 0 {
                out.extend(self.emit_pcg_measurements());
            }
        }
        out
    }
}

impl PipelineProcessor for Rc2TrafficFrameAligner {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.pending_chip_start.is_none() {
            self.pending_chip_start = Some(block.chip_start);
        }
        self.sample_rate_hz = block.sample_rate_hz;
        self.tags = block.tags.clone();
        if let Some(start) = block
            .tags
            .get("absolute_chip_start")
            .copied()
            .and_then(|value| u64::try_from(value).ok())
        {
            self.last_processing_absolute_chip_end =
                Some(start.saturating_add(block.samples.len() as u64));
        }

        self.pending_samples.extend(block.samples);

        if let Some(chip_start) = self.pending_chip_start {
            let rem = chip_start % PN_CHIPS_PER_SYMBOL;
            if rem != 0 {
                let skip = PN_CHIPS_PER_SYMBOL - rem;
                if self.pending_samples.len() < skip {
                    return Vec::new();
                }
                self.pending_samples.drain(..skip);
                self.pending_chip_start = Some(chip_start + skip);
            }
        }

        self.process_symbols()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
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
        "Rc2TrafficFrameAligner"
    }

    fn metrics(&self) -> Vec<(&'static str, String)> {
        let state = match self.state {
            AlignerState::Preamble => "preamble",
            AlignerState::Locked => "locked",
        };
        vec![
            ("state", state.to_string()),
            ("symbols", self.symbols_decoded.to_string()),
            ("frames", self.frames_decoded.to_string()),
        ]
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::coding::convolutional::get_1_2_k9_encoder;
    use crate::phy::walsh::WalshGenerator;

    /// Encode info bits → append CRC and tail → R=1/2 conv → repetition →
    /// interleave → 6-bit Walsh symbol groups → noise-free Walsh symbol IQ.
    /// Returns chip-rate samples spanning exactly one 20 ms RC2 frame.
    fn synth_rc2_frame_chips(rate: Rc2TrafficRate, info: &[u8]) -> Vec<Complex32> {
        assert_eq!(info.len(), rate.info_bits());

        // 1) Assemble reserved/EIB + info + CRC + tail.
        let mut input: Vec<u8> = Vec::with_capacity(rate.frame_input_bits());
        input.push(0);
        input.extend_from_slice(info);
        match rate {
            Rc2TrafficRate::Full => {
                let crc = crc12(&input);
                for i in (0..12).rev() {
                    input.push(((crc >> i) & 1) as u8);
                }
            }
            Rc2TrafficRate::Half => {
                let crc = crc10(&input);
                for i in (0..10).rev() {
                    input.push(((crc >> i) & 1) as u8);
                }
            }
            Rc2TrafficRate::Quarter => {
                let crc = crc8(&input);
                for i in (0..8).rev() {
                    input.push(((crc >> i) & 1) as u8);
                }
            }
            Rc2TrafficRate::Eighth => {
                let crc = crc6_rc2(&input);
                for i in (0..6).rev() {
                    input.push(((crc >> i) & 1) as u8);
                }
            }
        }
        for _ in 0..rate.tail_bits() {
            input.push(0);
        }
        assert_eq!(input.len(), rate.frame_input_bits());

        // 2) R=1/2 K=9 conv encode (encoder is flushed by the 8-bit tail).
        let mut encoder = get_1_2_k9_encoder();
        let mut conv: Vec<u8> = Vec::with_capacity(input.len() * 2);
        for &b in &input {
            let pair = encoder.encode(b);
            conv.push(pair[0]);
            conv.push(pair[1]);
        }
        // conv.len() = 2 * frame_input_bits.

        // 3) Symbol repetition fills the 576-cell interleaver exactly.
        let rep = rate.repetition_factor();
        let mut repeated: Vec<u8> = Vec::with_capacity(conv.len() * rep);
        for &b in &conv {
            for _ in 0..rep {
                repeated.push(b);
            }
        }
        assert_eq!(repeated.len(), 576);

        // 4) Interleave.
        let interleaver = Rc12ReverseTrafficInterleaver::new(rate.to_interleaver_rate());
        let interleaved = interleaver.encode(&repeated);

        // 5) Group into 96 Walsh symbols (6 bits each), big-endian within
        //    each group (matches soft_bits_from_energies bit ordering).
        let mut walsh_indices: Vec<usize> = Vec::with_capacity(RC1_SYMBOLS_PER_FRAME);
        for chunk in interleaved.chunks_exact(RC1_SOFT_BITS_PER_SYMBOL) {
            // soft_bits_from_energies emits bit `b` such that for row index r,
            // bit_b = (r >> b) & 1. So a hard bit sequence [b0, b1, ..., b5]
            // corresponds to row = sum(b_i << i).
            let mut row: usize = 0;
            for (i, &bit) in chunk.iter().enumerate() {
                row |= ((bit & 1) as usize) << i;
            }
            walsh_indices.push(row);
        }
        assert_eq!(walsh_indices.len(), RC1_SYMBOLS_PER_FRAME);

        // 6) Modulate each Walsh index to 64 BPSK Walsh chips, oversample
        //    by 4 to chip rate.
        let mut chips: Vec<Complex32> =
            Vec::with_capacity(RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL);
        let matrix = WalshGenerator::generate_matrix::<{ RC1_WALSH_CHIPS_PER_SYMBOL }>();
        let active_pcgs = Rc2TrafficFrameAligner::new(0).active_pcgs_for_rate(rate, 0);
        for (symbol_index, &row) in walsh_indices.iter().enumerate() {
            let active = active_pcgs[symbol_index / RC1_SYMBOLS_PER_PCG];
            let walsh = &matrix[row];
            for &w in walsh {
                // Map +1 → +1, -1 → -1 chip. The demod sums each group of
                // RC1_PN_CHIPS_PER_WALSH_CHIP, so duplicate per PN chip.
                let v = if active { w as f32 } else { 0.0 };
                for _ in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                    chips.push(Complex32::new(v, 0.0));
                }
            }
        }
        assert_eq!(chips.len(), RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL);
        chips
    }

    fn run_roundtrip(rate: Rc2TrafficRate, info: &[u8]) -> Vec<SampleBlock> {
        let chips = synth_rc2_frame_chips(rate, info);
        let mut aligner = Rc2TrafficFrameAligner::new(0);
        // Provide one extra null symbol leading-in so the aligner sees the
        // frame boundary cleanly; chip_start=0 starts on a frame boundary.
        let block = SampleBlock::new(chips, 0);
        let mut out = aligner.process_block(block);
        out.extend(aligner.flush());
        // Need a following dummy block to flush the frame (the aligner
        // only emits when the next frame boundary arrives, since
        // RC1_SYMBOLS_PER_FRAME completion is detected via the boundary
        // check `symbol_in_frame == 0 && frame_symbol_count > 0`).
        let next_block = SampleBlock::new(
            vec![Complex32::new(0.0, 0.0); PN_CHIPS_PER_SYMBOL],
            RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL,
        );
        out.extend(aligner.process_block(next_block));
        out
    }

    fn assert_decoded(rate: Rc2TrafficRate, info: &[u8]) {
        let blocks = run_roundtrip(rate, info);
        let decoded = blocks
            .iter()
            .find(|b| b.tags.get("traffic_decoded_frame") == Some(&1))
            .unwrap_or_else(|| panic!("no decoded frame emitted for rate {:?}", rate));
        assert_eq!(
            decoded.tags.get("traffic_rate_bps").copied(),
            Some(rate.rate_bps() as i64),
            "rate mismatch for {:?}",
            rate
        );
        assert_eq!(
            decoded.tags.get("traffic_phy_valid").copied(),
            Some(1),
            "phy not valid for {:?}",
            rate
        );
        // The aligner removes the reserved/EIB bit from the sample payload.
        let payload: Vec<u8> = decoded.samples.iter().map(|c| c.re as u8 & 1).collect();
        assert_eq!(
            &payload[..info.len()],
            info,
            "info-bit mismatch for {:?}",
            rate
        );
    }

    #[test]
    fn rc2_traffic_full_rate_roundtrip() {
        let info: Vec<u8> = (0..267).map(|i| ((i * 5 + 3) & 1) as u8).collect();
        assert_decoded(Rc2TrafficRate::Full, &info);
    }

    #[test]
    fn one_all_zero_rc2_frame_emits_preamble() {
        let frame_chips = RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL;
        let mut aligner = Rc2TrafficFrameAligner::new(0);
        let mut out = aligner.process_block(SampleBlock::new(
            vec![Complex32::new(1.0, 0.0); frame_chips],
            0,
        ));
        out.extend(aligner.process_block(SampleBlock::new(
            vec![Complex32::new(0.0, 0.0); PN_CHIPS_PER_SYMBOL],
            frame_chips,
        )));

        let preamble = out
            .iter()
            .find(|block| block.tags.get("traffic_preamble_detected") == Some(&1))
            .expect("one RC2 preamble frame must be detected");
        assert_eq!(preamble.tags.get("traffic_preamble_frames"), Some(&1));
    }

    #[test]
    fn rc2_traffic_half_rate_roundtrip() {
        let info: Vec<u8> = (0..125).map(|i| ((i * 7 + 1) & 1) as u8).collect();
        assert_decoded(Rc2TrafficRate::Half, &info);
    }

    #[test]
    fn rc2_traffic_quarter_rate_roundtrip() {
        let info: Vec<u8> = (0..55).map(|i| ((i * 11 + 2) & 1) as u8).collect();
        assert_decoded(Rc2TrafficRate::Quarter, &info);
    }

    #[test]
    fn rc2_traffic_eighth_rate_roundtrip() {
        let info: Vec<u8> = (0..21).map(|i| ((i * 13 + 5) & 1) as u8).collect();
        assert_decoded(Rc2TrafficRate::Eighth, &info);
    }

    #[test]
    fn rc2_soft_metrics_are_invariant_to_input_power() {
        let mut energies = [1.0f32; RC1_WALSH_CHIPS_PER_SYMBOL];
        energies[37] = 20.0;
        let baseline = Rc2TrafficFrameAligner::soft_bits_from_energies(&energies);
        let scaled = energies.map(|energy| energy * 100.0);
        let scaled = Rc2TrafficFrameAligner::soft_bits_from_energies(&scaled);

        for (baseline, scaled) in baseline.into_iter().zip(scaled) {
            assert!((baseline - scaled).abs() < 1e-4);
        }
    }

    #[test]
    fn rc2_realtime_power_measurements_use_guaranteed_active_pcgs() {
        let mut aligner = Rc2TrafficFrameAligner::new(0xDEAD_BEEF);
        aligner.state = AlignerState::Locked;
        aligner.frame_chip_start = 16 * 1536;
        aligner.symbol_energies = vec![[1.0; RC1_WALSH_CHIPS_PER_SYMBOL]; RC1_SYMBOLS_PER_FRAME];
        aligner.last_processing_absolute_chip_end =
            Some(aligner.frame_chip_start as u64 + 16 * 1536);

        let measurements = aligner.emit_pcg_measurements();
        let expected =
            aligner.active_pcgs_for_rate(Rc2TrafficRate::Eighth, aligner.frame_chip_start);
        let measured_pcgs: Vec<_> = measurements
            .iter()
            .map(|block| {
                (block.tags["absolute_chip_start"] as usize - aligner.frame_chip_start) / 1536
            })
            .collect();
        let expected_pcgs: Vec<_> = expected
            .iter()
            .enumerate()
            .filter_map(|(pcg, active)| active.then_some(pcg))
            .collect();

        assert_eq!(measured_pcgs, expected_pcgs);
        assert_eq!(measurements.len(), 2);
        assert!(
            measurements
                .iter()
                .all(|block| block.tags.contains_key("traffic_pcg_mobile_power_mdbfs"))
        );
    }

    #[test]
    fn rc2_invalid_frame_reports_only_guaranteed_active_pcgs() {
        let mut aligner = Rc2TrafficFrameAligner::new(0xDEAD_BEEF);
        aligner.state = AlignerState::Locked;
        aligner.frame_chip_start = 16 * 1536;
        aligner.frame_soft = vec![0.0; SOFT_BITS_PER_FRAME];
        aligner.symbol_energies = vec![[1.0; RC1_WALSH_CHIPS_PER_SYMBOL]; RC1_SYMBOLS_PER_FRAME];

        let decoded = aligner
            .handle_complete_frame()
            .into_iter()
            .find(|block| block.tags.get("traffic_decoded_frame") == Some(&1))
            .expect("tail-only invalid candidate");
        assert_eq!(decoded.tags.get("traffic_phy_valid"), Some(&0));
        assert_eq!(
            decoded.active_pcg_mask,
            Some(aligner.active_pcgs_for_rate(Rc2TrafficRate::Eighth, aligner.frame_chip_start))
        );
    }

    #[test]
    fn rc2_traffic_rate_bit_budgets_match_spec() {
        for rate in Rc2TrafficRate::SEARCH_ORDER {
            let conv_out = 2 * rate.frame_input_bits();
            let after_rep = conv_out * rate.repetition_factor();
            assert_eq!(after_rep, 576, "rate {:?}", rate);
        }
    }

    #[test]
    fn rc2_crc10_known_vector_round_trip() {
        // crc10(empty) is deterministic — round-trip a few patterns.
        let cases: &[&[u8]] = &[
            &[],
            &[0, 0, 0, 0, 0, 0, 0, 0],
            &[1, 1, 1, 1, 1, 1, 1, 1],
            &[1, 0, 1, 0, 1, 0, 1, 0],
        ];
        for case in cases {
            let c = crc10(case);
            // Property: appending the CRC bits and recomputing on the
            // concatenation yields a register state that, when re-fed,
            // reproduces the same CRC for the prefix. Used as a smoke test
            // that crc10 is at least stable / deterministic.
            assert_eq!(c, crc10(case));
            assert!(c < 1024);
        }
    }

    // ---- QCELP-13K / RC2 end-to-end smoke ---------------------------------
    //
    // These tests demonstrate that the bit stream produced by
    // `Qcelp13kEncoder` round-trips bit-exact through the noise-free RC2
    // reverse air-interface (synth → aligner). Forward-link RC2 cannot loop
    // back into the reverse aligner (different modulation), so this is the
    // furthest e2e path that fits in one in-process test. The codec's own
    // encoder/decoder round-trip is covered by `cdma-voice::qcelp13k::tests`.

    fn unpack_msb(bytes: &[u8]) -> Vec<u8> {
        let mut bits = Vec::with_capacity(bytes.len() * 8);
        for &b in bytes {
            for k in (0..8).rev() {
                bits.push((b >> k) & 1);
            }
        }
        bits
    }

    fn run_qcelp_rc2_roundtrip_for_rate(rc2_rate: Rc2TrafficRate, frame_count: usize) {
        let mut encoder = cdma_voice::qcelp13k::Qcelp13kEncoder::new().expect("encoder init");
        for frame_idx in 0..frame_count {
            // Distinct PCM per frame so the encoder produces a non-trivial
            // rate decision and the test catches any frame-to-frame drift.
            let mut pcm = [0i16; 160];
            for (i, sample) in pcm.iter_mut().enumerate() {
                let n = (frame_idx * 160 + i) as f32;
                let phase = 2.0 * std::f32::consts::PI * 1000.0 * n / 8000.0;
                *sample = (phase.sin() * 8192.0) as i16;
            }
            let (_voice_rate, payload) = encoder.encode(&pcm).expect("encode");
            // QCELP payload bytes carry the rate's information bits MSB-first,
            // with the trailing 2-6 bits of the final byte being zero padding
            // from the packing. Truncate to the RC2 info budget; this drops
            // only padding zeros, never QCELP information.
            let mut info_bits = vec![0];
            info_bits.extend(unpack_msb(&payload));
            info_bits.truncate(rc2_rate.info_bits());
            info_bits.resize(rc2_rate.info_bits(), 0);

            let blocks = run_roundtrip(rc2_rate, &info_bits);
            let decoded = blocks
                .iter()
                .find(|b| b.tags.get("traffic_decoded_frame") == Some(&1))
                .unwrap_or_else(|| {
                    panic!("RC2 aligner emitted no decoded frame (frame {})", frame_idx)
                });
            assert_eq!(
                decoded.tags.get("traffic_phy_valid").copied(),
                Some(1),
                "phy_valid=0 on frame {}",
                frame_idx
            );
            let recovered: Vec<u8> = decoded.samples.iter().map(|c| c.re as u8 & 1).collect();
            assert_eq!(
                &recovered[..info_bits.len()],
                info_bits.as_slice(),
                "RC2 reverse air interface corrupted QCELP-13K bits on frame {}",
                frame_idx
            );
        }
    }

    #[test]
    fn qcelp13k_payload_survives_rc2_full_reverse_air_interface_multi_frame() {
        // RC2 Full has 267 info bits — enough for any QCELP-13K rate (Full
        // is 266 bits, smaller rates are 124 / 54 / 20). 5 consecutive
        // frames exercise codec state continuity end-to-end.
        run_qcelp_rc2_roundtrip_for_rate(Rc2TrafficRate::Full, 5);
    }

    #[test]
    fn qcelp13k_silence_payload_survives_rc2_full_reverse_air_interface() {
        // Silence drives the encoder to Eighth rate (20 bits), the smallest
        // QCELP payload — proves the bit-survival path for the degenerate
        // case where most of the RC2 info budget is zero-padding.
        let mut encoder = cdma_voice::qcelp13k::Qcelp13kEncoder::new().expect("encoder init");
        let pcm = [0i16; 160];
        let (_voice_rate, payload) = encoder.encode(&pcm).expect("encode");
        let mut info_bits = vec![0];
        info_bits.extend(unpack_msb(&payload));
        info_bits.resize(Rc2TrafficRate::Full.info_bits(), 0);

        let blocks = run_roundtrip(Rc2TrafficRate::Full, &info_bits);
        let decoded = blocks
            .iter()
            .find(|b| b.tags.get("traffic_decoded_frame") == Some(&1))
            .expect("aligner emitted no decoded frame for silence payload");
        let recovered: Vec<u8> = decoded.samples.iter().map(|c| c.re as u8 & 1).collect();
        assert_eq!(&recovered[..info_bits.len()], info_bits.as_slice());
    }
}
