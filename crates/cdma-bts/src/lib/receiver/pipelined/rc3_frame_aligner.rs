use std::collections::{HashMap, VecDeque};
use std::f32::consts::{FRAC_PI_2, PI};

use cdma_common::crc::{crc6, crc8, crc12};
use cdma_common::diagnostics::{rc3_lower_rate_diag_enabled_for_walsh, rc3_lower_rate_diag_limit};
use log::{debug, trace};
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock, raw_to_soft};
use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_1536};
use crate::phy::coding::convolutional::get_1_4_k9_soft_viterbi_decoder;
use crate::receiver::pipelined::traffic_channel_processor::parse_reverse_mux1_full_rate_format;
use cdma_common::consts::SR1_PCGS_PER_FRAME;

/// RC3 R-FCH 20ms frame: 1536 interleaver symbols per frame.
const FRAME_SYMBOLS_20MS: usize = 1536;

/// Per C.S0002-E §2.1.3.12.7: when REV_FCH_GATING_MODE=1 and rate is
/// 1500 bps (eighth rate), R-FCH transmits on PCGs {2,3,6,7,10,11,14,15} only.
const EIGHTH_RATE_GATED_PCGS: [bool; 16] = [
    false, false, true, true, // 0-3
    false, false, true, true, // 4-7
    false, false, true, true, // 8-11
    false, false, true, true, // 12-15
];

/// Soft symbols per PCG (1536 / 16 = 96).
const SYMBOLS_PER_PCG: usize = FRAME_SYMBOLS_20MS / SR1_PCGS_PER_FRAME;

/// Each soft symbol is a single f32 (BPSK, not 64-ary).
const SOFT_BITS_PER_SYMBOL: usize = 1;

/// Chips per BPSK symbol (Walsh length W(4,16) = 16 chips).
const CHIPS_PER_SYMBOL: usize = 16;
#[cfg(test)]
const PCG_CHIPS: usize = SYMBOLS_PER_PCG * CHIPS_PER_SYMBOL;

/// RC3 R-FCH 20 ms frame = 24576 chips (C.S0002-E §2.1.3.12.1).
const FRAME_CHIPS_20MS: usize = FRAME_SYMBOLS_20MS * CHIPS_PER_SYMBOL;

/// Configurable frame duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDuration {
    /// 20 ms frame = 24,576 chips = 1536 symbols.
    TwentyMs,
}

impl FrameDuration {
    const fn symbols(self) -> usize {
        match self {
            Self::TwentyMs => FRAME_SYMBOLS_20MS,
        }
    }

    const fn chips(self) -> usize {
        match self {
            Self::TwentyMs => FRAME_CHIPS_20MS,
        }
    }
}

// ---------------------------------------------------------------------------
// RC3 rate definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rc3TrafficRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl Rc3TrafficRate {
    const SEARCH_ORDER: [Self; 4] = [Self::Full, Self::Half, Self::Quarter, Self::Eighth];

    const fn frame_bits(self) -> usize {
        match self {
            Self::Full => 192,
            Self::Half => 96,
            Self::Quarter => 54,
            Self::Eighth => 30,
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
            Self::Quarter | Self::Eighth => 6,
        }
    }

    const fn tail_bits(self) -> usize {
        8
    }

    const fn repetition_factor(self) -> usize {
        match self {
            Self::Full => 2,
            Self::Half => 4,
            Self::Quarter => 8,
            Self::Eighth => 16,
        }
    }

    const fn rate_bps(self) -> usize {
        match self {
            Self::Full => 9600,
            Self::Half => 4800,
            Self::Quarter => 2700,
            Self::Eighth => 1500,
        }
    }
}

// ---------------------------------------------------------------------------
// Frame validation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameValidation {
    fqi_valid: bool,
    tail_valid: bool,
    phy_valid: bool,
}

impl FrameValidation {
    fn for_rate(rate: Rc3TrafficRate, bits: &[u8]) -> Self {
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
            Rc3TrafficRate::Full => {
                let computed = crc12(&bits[..rate.info_bits()]);
                let mut received: u16 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit as u16 & 1);
                }
                computed == received
            }
            Rc3TrafficRate::Half => {
                let computed = crc8(&bits[..rate.info_bits()]);
                let mut received: u8 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit & 1);
                }
                computed == received
            }
            Rc3TrafficRate::Quarter | Rc3TrafficRate::Eighth => {
                let computed = crc6(&bits[..rate.info_bits()]);
                let mut received: u8 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit & 1);
                }
                computed == received
            }
        };

        Self {
            fqi_valid,
            tail_valid,
            phy_valid: tail_valid && fqi_valid,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DecodedFrame {
    rate: Rc3TrafficRate,
    bits: Vec<u8>,
    validation: FrameValidation,
    /// Unconstrained ML best terminal state from Viterbi forward pass.
    /// 0 = the decoder naturally converged to state 0 (strong signal that
    /// this is the correct rate). Non-zero = likely wrong rate or noise.
    ml_best_state: u8,
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameAlignerState {
    /// Waiting for absolute_chip_start tag so we can align to the next
    /// spec-aligned frame boundary.
    WaitingForChipTag,
    /// Draining soft symbols until `drain_target_symbols` more have been
    /// consumed to reach the next frame boundary.
    DrainingToBoundary { drain_target_symbols: usize },
    /// Aligned — emit one frame per `FRAME_SYMBOLS_*` soft bits.
    Aligned,
}

/// RC3 reverse traffic channel frame aligner.
///
/// Uses `absolute_chip_start` from upstream tags plus `FRAME_OFFSET=0` (our
/// BTS default) to compute the next spec-aligned 20 ms (or optionally 5 ms)
/// frame boundary per C.S0002-E §2.1.3.12.1 ("A zero-offset 20 ms Reverse
/// Fundamental Channel frame shall begin only when System Time is an
/// integral multiple of 20 ms"). Drains soft symbols up to that boundary
/// and then emits one decoded frame every `FRAME_SYMBOLS` soft bits,
/// trying all rates and scoring by CRC validity.
pub struct Rc3FrameAligner {
    state: FrameAlignerState,
    symbol_buf: VecDeque<Complex32>,
    tags: HashMap<&'static str, i64>,
    chip_start: usize,
    sample_rate_hz: f64,
    absolute_chip_start: Option<i64>,
    frame_duration: FrameDuration,
    walsh_code: Option<u8>,
    locked_rate: Option<Rc3TrafficRate>,
    symbol_axis_phase: Option<f32>,
    rev_fch_gating_mode: bool,
    lower_rate_diag_logged: usize,
    /// When true, upstream despreader performed pilot-aided coherent demod:
    /// signal is on .re, noise on .im, axis is 0.  Skip M2 estimation.
    pilot_coherent: bool,
    /// Per-PCG pilot metrics from the despreader, buffered in parallel with
    /// `symbol_buf`.  Each entry is `(pilot_norm_sq, pilot_sym_power_sum, traffic_power_sum, chip_power_sum)`.
    pilot_metrics_buf: VecDeque<(f32, f32, f32, f32)>,
}

impl Rc3FrameAligner {
    pub fn new() -> Self {
        Self::with_frame_duration(FrameDuration::TwentyMs)
    }

    /// Create an aligner configured for a specific frame duration.
    /// Defaults to 20 ms frames.
    pub fn with_frame_duration(frame_duration: FrameDuration) -> Self {
        Self {
            state: FrameAlignerState::WaitingForChipTag,
            symbol_buf: VecDeque::new(),
            tags: HashMap::new(),
            chip_start: 0,
            sample_rate_hz: 0.0,
            absolute_chip_start: None,
            frame_duration,
            walsh_code: None,
            locked_rate: None,
            symbol_axis_phase: None,
            rev_fch_gating_mode: false,
            lower_rate_diag_logged: 0,
            pilot_coherent: false,
            pilot_metrics_buf: VecDeque::new(),
        }
    }

    pub fn with_rev_fch_gating_mode(mut self, mode: bool) -> Self {
        self.rev_fch_gating_mode = mode;
        self
    }

    pub fn with_walsh_code(mut self, walsh_code: u8) -> Self {
        self.walsh_code = Some(walsh_code);
        self
    }

    fn frame_symbols(&self) -> usize {
        self.frame_duration.symbols()
    }

    fn frame_chips(&self) -> usize {
        self.frame_duration.chips()
    }

    /// Per C.S0002-E §2.1.3.12.7: when REV_FCH_GATING_MODE=1 and rate
    /// is eighth (1500 bps), only PCGs {2,3,6,7,10,11,14,15} carry R-FCH.
    /// All other rates transmit on all 16 PCGs.
    fn active_pcg_mask_for_rate(&self, rate: Rc3TrafficRate) -> [bool; 16] {
        if self.rev_fch_gating_mode && rate == Rc3TrafficRate::Eighth {
            EIGHTH_RATE_GATED_PCGS
        } else {
            [true; 16]
        }
    }

    /// Compute 16 per-PCG signal quality estimates from the raw soft
    /// symbols of a 20ms frame. Each PCG spans 96 BPSK soft symbols
    /// (1536 / 16). The metric is the mean squared magnitude of the soft
    /// symbols within each PCG, converted to dB.
    ///
    /// This is a simpler estimate than RC1's FHT-based peak/mean-of-others
    /// Walsh-bin SNR, but it serves the same purpose: giving the BSC's
    /// inner loop a per-PCG signal quality measurement to drive power
    /// control bit decisions.
    ///
    /// For gated-off PCGs (where the mobile doesn't transmit), the mean
    /// energy will be near zero → very low dB → the BSC's inner loop
    /// will treat them as inactive.
    ///
    /// Compute per-PCG traffic Eb/Nt (dB) for a full 20 ms frame of complex
    /// symbols.
    ///
    /// Uses per-PCG axis estimation so the phase tracks across the frame,
    /// then computes the linear-domain Eb/Nt for each active PCG (see
    /// [`pcg_eb_nt_db`] for the formula). This is a decoder-quality estimate,
    /// not the RC3 inner-loop power-control metric.
    fn pcg_eb_nt_db_for_frame(
        symbols: &[Complex32],
        initial_axis_phase: f32,
        rate: Rc3TrafficRate,
        active_mask: &[bool; 16],
    ) -> Option<Vec<f32>> {
        if symbols.len() < FRAME_SYMBOLS_20MS {
            return None;
        }
        let mut per_pcg_db = Vec::with_capacity(SR1_PCGS_PER_FRAME);
        let mut axis_phase = initial_axis_phase;
        for pcg in 0..SR1_PCGS_PER_FRAME {
            if !active_mask[pcg] {
                per_pcg_db.push(f32::NAN);
                continue;
            }
            let start = pcg * SYMBOLS_PER_PCG;
            let end = start + SYMBOLS_PER_PCG;
            let pcg_symbols = &symbols[start..end];
            let (db, next_axis) = Self::pcg_eb_nt_db(pcg_symbols, axis_phase, rate);
            axis_phase = next_axis;
            per_pcg_db.push(db);
        }
        Some(per_pcg_db)
    }

    fn soft_bits_per_frame(&self) -> usize {
        self.frame_symbols() * SOFT_BITS_PER_SYMBOL
    }

    fn wrap_phase(phase: f32) -> f32 {
        let two_pi = 2.0 * PI;
        (phase + PI).rem_euclid(two_pi) - PI
    }

    fn phase_distance(a: f32, b: f32) -> f32 {
        Self::wrap_phase(a - b).abs()
    }

    fn estimate_symbol_axis_phase(symbols: &[Complex32], prev: f32) -> f32 {
        let m2 = symbols
            .iter()
            .fold(Complex32::new(0.0, 0.0), |acc, sym| acc + (*sym * *sym));
        let phase = if m2.norm_sqr() <= 1e-9 {
            prev
        } else {
            let base = 0.5 * m2.im.atan2(m2.re);
            let alt = Self::wrap_phase(base + PI);
            if Self::phase_distance(base, prev) <= Self::phase_distance(alt, prev) {
                base
            } else {
                alt
            }
        };
        phase
    }

    fn project_symbols(symbols: &[Complex32], axis_phase: f32) -> Vec<f32> {
        let axis = Complex32::new(axis_phase.cos(), axis_phase.sin());
        symbols
            .iter()
            .map(|symbol| symbol.re * axis.re + symbol.im * axis.im)
            .collect()
    }

    fn project_symbols_per_pcg(symbols: &[Complex32], initial_axis_phase: f32) -> (Vec<f32>, f32) {
        if symbols.len() != FRAME_SYMBOLS_20MS {
            return (
                Self::project_symbols(symbols, initial_axis_phase),
                initial_axis_phase,
            );
        }

        let mut axis_phase = initial_axis_phase;
        let mut soft = Vec::with_capacity(symbols.len());
        for pcg_symbols in symbols.chunks_exact(SYMBOLS_PER_PCG) {
            axis_phase = Self::estimate_symbol_axis_phase(pcg_symbols, axis_phase);
            soft.extend(Self::project_symbols(pcg_symbols, axis_phase));
        }
        (soft, axis_phase)
    }

    /// Compute pilot Ec/Io in dB from one PCG's coherent pilot power and
    /// total wideband chip power.
    #[cfg(test)]
    fn pcg_pilot_ec_io_db(pilot_norm_sq: f32, chip_power_sum: f32) -> f32 {
        if chip_power_sum > 1e-12 {
            let n = SYMBOLS_PER_PCG as f32;
            let n_chips = PCG_CHIPS as f32;
            let ec_io = pilot_norm_sq * n_chips / (n * n * chip_power_sum);
            10.0 * ec_io.max(1e-12).log10()
        } else {
            40.0
        }
    }

    /// Compute true air-interface Eb/Nt (dB) for one PCG of complex despread
    /// symbols.
    ///
    /// **Non-pilot-coherent mode (M2 estimator):**
    /// After Walsh decover the R-FCH signal is BPSK on one axis. The on-axis
    /// projection contains signal + noise; the off-axis (perpendicular)
    /// projection contains noise only.  Working in linear domain:
    ///
    ///   Es  = mean(soft²)           — signal + noise energy per symbol
    ///   Nn  = mean(perp²)           — noise energy per symbol (one real
    ///                                  dimension = half the complex noise)
    ///   Eb/Nt = (768 / info_bits) × (Es − Nn) / Nn
    ///
    /// RC3 inner-loop power control uses pilot [`pcg_pilot_ec_io_db`]
    /// measurements. This function remains the traffic-symbol Eb/Nt estimator
    /// used for decoder-quality diagnostics and tests.
    fn pcg_eb_nt_db(
        symbols: &[Complex32],
        prev_axis_phase: f32,
        rate: Rc3TrafficRate,
    ) -> (f32, f32) {
        // M2 axis is always computed for diagnostic tracking.
        let m2_axis = Self::estimate_symbol_axis_phase(symbols, prev_axis_phase);

        let sym_to_bit = (FRAME_SYMBOLS_20MS / 2) as f32 / rate.info_bits() as f32;

        // Always use M2 axis for Eb/Nt measurement — it tracks the actual
        // signal axis regardless of demod mode. With pilot-coherent symbols,
        // M2 finds axis ≈ 0 (signal on .re) and the on/off-axis decomposition
        // correctly separates signal from noise without cross-term contamination.
        let demod_axis = m2_axis;
        let cos_a = demod_axis.cos();
        let sin_a = demod_axis.sin();
        let n = symbols.len().max(1) as f32;

        let mut es_sum = 0.0f32;
        let mut nn_sum = 0.0f32;
        for &sym in symbols {
            let soft = sym.re * cos_a + sym.im * sin_a;
            let perp = sym.im * cos_a - sym.re * sin_a;
            es_sum += soft * soft;
            nn_sum += perp * perp;
        }
        let es_lin = es_sum / n;
        let nn_lin = nn_sum / n;
        let signal_lin = (es_lin - nn_lin).max(0.0);

        let eb_nt_lin = if nn_lin > 1e-12 {
            sym_to_bit * signal_lin / nn_lin
        } else {
            1e12
        };
        (10.0 * eb_nt_lin.max(1e-12).log10(), m2_axis)
    }

    fn refresh_absolute_chip_tags(&mut self) {
        if let Some(absolute_chip_start) = self.tags.get("absolute_chip_start").copied() {
            self.absolute_chip_start = Some(absolute_chip_start);
        }
    }

    // -----------------------------------------------------------------------
    // Decode helpers
    // -----------------------------------------------------------------------

    fn decode_soft_bits(collapsed: &[f32]) -> (Vec<u8>, u8) {
        let peak = collapsed.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let mut viterbi = get_1_4_k9_soft_viterbi_decoder();
        let inputs: Vec<[f32; 4]> = collapsed
            .chunks_exact(4)
            .map(|chunk| {
                [
                    raw_to_soft(chunk[0], inv_peak),
                    raw_to_soft(chunk[1], inv_peak),
                    raw_to_soft(chunk[2], inv_peak),
                    raw_to_soft(chunk[3], inv_peak),
                ]
            })
            .collect();
        let bits = viterbi.decode_block_from_state(&inputs, 0);
        let ml_best_state = viterbi.ml_best_terminal_state() as u8;
        (bits, ml_best_state)
    }

    fn deinterleave_frame_soft(soft: &[f32]) -> Vec<f32> {
        // For 20ms frames this uses the 1536-symbol interleaver. 5ms frames
        // would need a 384-symbol interleaver (not yet implemented).
        let interleaver = BitReversalInterleaver::new(SR1_PARAMS_1536);
        interleaver.decode_soft(soft)
    }

    fn decode_frame_from_deinterleaved(
        deinterleaved: &[f32],
        rate: Rc3TrafficRate,
    ) -> DecodedFrame {
        let repeated_symbols = match rate {
            Rc3TrafficRate::Full | Rc3TrafficRate::Half => deinterleaved.to_vec(),
            Rc3TrafficRate::Quarter => {
                let input_len = 216 * rate.repetition_factor();
                let output_len = deinterleaved.len();
                let mut out = vec![0.0f32; input_len];
                for (k, &sym) in deinterleaved.iter().enumerate() {
                    let input_idx = (k * input_len) / output_len;
                    out[input_idx] = sym;
                }
                out
            }
            Rc3TrafficRate::Eighth => {
                let input_len = 120 * rate.repetition_factor();
                let output_len = deinterleaved.len();
                let mut out = vec![0.0f32; input_len];
                for (k, &sym) in deinterleaved.iter().enumerate() {
                    let input_idx = (k * input_len) / output_len;
                    out[input_idx] = sym;
                }
                out
            }
        };

        let rep = rate.repetition_factor();
        let code_symbols: Vec<f32> = repeated_symbols
            .chunks_exact(rep)
            .map(|c| c.iter().sum::<f32>() / rep as f32)
            .collect();

        let (bits, ml_best_state) = Self::decode_soft_bits(&code_symbols);
        let validation = FrameValidation::for_rate(rate, &bits);

        DecodedFrame {
            rate,
            bits,
            validation,
            ml_best_state,
        }
    }

    #[cfg(test)]
    fn decode_frame_soft(soft: &[f32], rate: Rc3TrafficRate) -> DecodedFrame {
        let deinterleaved = Self::deinterleave_frame_soft(soft);
        Self::decode_frame_from_deinterleaved(&deinterleaved, rate)
    }

    // -----------------------------------------------------------------------
    // Direct alignment: compute the next frame boundary from
    // absolute_chip_start per C.S0002-E §2.1.3.12.1. FRAME_OFFSET is
    // assumed to be 0 (the BTS default); add support when we send a
    // non-zero FRAME_OFFSET in the ECAM.
    // -----------------------------------------------------------------------

    /// Compute how many soft bits must be dropped from the front of
    /// `symbol_buf` so that the next symbol aligns with a spec-defined
    /// frame boundary.
    fn soft_bits_to_next_frame_boundary(&self) -> Option<usize> {
        let abs = self.absolute_chip_start? as u64;
        let period = self.frame_chips() as u64;
        let offset_in_frame = (abs % period) as usize;
        let chips_to_boundary = (self.frame_chips() - offset_in_frame) % self.frame_chips();
        // Sanity: the underlying chip stream is walsh-aligned (16-chip
        // boundaries). If it isn't, we can't usefully align within a
        // fraction of a symbol.
        if chips_to_boundary % CHIPS_PER_SYMBOL != 0 {
            debug!(
                "rc3_frame_aligner: chip stream not walsh-aligned \
                 (chips_to_boundary={} CHIPS_PER_SYMBOL={}); alignment \
                 may be off — will still drain to nearest symbol",
                chips_to_boundary, CHIPS_PER_SYMBOL,
            );
        }
        let symbols_to_boundary = chips_to_boundary / CHIPS_PER_SYMBOL;
        Some(symbols_to_boundary * SOFT_BITS_PER_SYMBOL)
    }

    // -----------------------------------------------------------------------
    // Locked: multi-rate decode and emit
    // -----------------------------------------------------------------------

    fn decode_with_rate_priority(
        &self,
        deinterleaved: &[f32],
        locked_rate: Option<Rc3TrafficRate>,
    ) -> DecodedFrame {
        let priority = Self::build_rate_priority(locked_rate);

        let mut first_decoded: Option<DecodedFrame> = None;
        let mut best_tail_valid: Option<DecodedFrame> = None;
        for rate in priority {
            let decoded = Self::decode_frame_from_deinterleaved(deinterleaved, rate);
            if first_decoded.is_none() {
                first_decoded = Some(decoded.clone());
            }
            if decoded.validation.phy_valid {
                return decoded;
            }
            if best_tail_valid.is_none() && decoded.validation.tail_valid {
                best_tail_valid = Some(decoded);
            }
        }

        best_tail_valid
            .or(first_decoded)
            .expect("RC3 rate-priority decode must attempt at least one rate")
    }

    fn build_rate_priority(locked_rate: Option<Rc3TrafficRate>) -> Vec<Rc3TrafficRate> {
        let mut priority = Vec::with_capacity(Rc3TrafficRate::SEARCH_ORDER.len());
        priority.push(Rc3TrafficRate::Full);
        if let Some(rate) = locked_rate
            && rate != Rc3TrafficRate::Full
        {
            priority.push(rate);
        }
        for rate in Rc3TrafficRate::SEARCH_ORDER {
            if !priority.contains(&rate) {
                priority.push(rate);
            }
        }
        priority
    }

    fn maybe_log_lower_rate_diagnostic(
        &mut self,
        frame_chip_start: usize,
        selected_rate: Rc3TrafficRate,
        selected_source: &'static str,
        recovered_axis_phase: f32,
        deinterleaved: &[f32],
    ) {
        if !matches!(
            selected_rate,
            Rc3TrafficRate::Quarter | Rc3TrafficRate::Eighth
        ) {
            return;
        }
        let Some(walsh_code) = self.walsh_code else {
            return;
        };
        if !rc3_lower_rate_diag_enabled_for_walsh(walsh_code) {
            return;
        }
        if self.lower_rate_diag_logged >= rc3_lower_rate_diag_limit() {
            return;
        }

        let quarter = Self::decode_frame_from_deinterleaved(deinterleaved, Rc3TrafficRate::Quarter);
        let eighth = Self::decode_frame_from_deinterleaved(deinterleaved, Rc3TrafficRate::Eighth);
        let quarter_info_ones = quarter
            .bits
            .iter()
            .take(quarter.rate.info_bits())
            .filter(|&&bit| bit == 1)
            .count();
        let eighth_info_ones = eighth
            .bits
            .iter()
            .take(eighth.rate.info_bits())
            .filter(|&&bit| bit == 1)
            .count();

        debug!(
            "rc3_frame_aligner[w{}]: lower_rate_diag chip={} source={} selected={} axis_deg={:.1} ambiguous={} q={{phy={},fqi={},tail={},ml={},info_ones={}}} e={{phy={},fqi={},tail={},ml={},info_ones={}}}",
            walsh_code,
            frame_chip_start,
            selected_source,
            selected_rate.rate_bps(),
            recovered_axis_phase.to_degrees(),
            quarter.validation.phy_valid && eighth.validation.phy_valid,
            quarter.validation.phy_valid,
            quarter.validation.fqi_valid,
            quarter.validation.tail_valid,
            quarter.ml_best_state,
            quarter_info_ones,
            eighth.validation.phy_valid,
            eighth.validation.fqi_valid,
            eighth.validation.tail_valid,
            eighth.ml_best_state,
            eighth_info_ones,
        );
        self.lower_rate_diag_logged += 1;
    }

    fn emit_frames(&mut self) -> Vec<SampleBlock> {
        let soft_bits_per_frame = self.soft_bits_per_frame();
        let mut out = Vec::new();
        while self.symbol_buf.len() >= soft_bits_per_frame {
            let frame_chip_start = self.chip_start;

            let raw_symbols: Vec<Complex32> = self
                .symbol_buf
                .iter()
                .take(soft_bits_per_frame)
                .copied()
                .collect();
            let default_axis = if self.pilot_coherent { 0.0 } else { -FRAC_PI_2 };
            let prev_axis_phase = self.symbol_axis_phase.unwrap_or(default_axis);
            let (estimated_axis_phase, estimated_soft) = if self.pilot_coherent {
                // Pilot-aided: signal already on .re — extract directly.
                (0.0, raw_symbols.iter().map(|s| s.re).collect::<Vec<f32>>())
            } else {
                let axis = Self::estimate_symbol_axis_phase(&raw_symbols, prev_axis_phase);
                (axis, Self::project_symbols(&raw_symbols, axis))
            };
            let estimated_deinterleaved = Self::deinterleave_frame_soft(&estimated_soft);
            let (pcg_soft, pcg_axis_phase) = if self.pilot_coherent {
                (raw_symbols.iter().map(|s| s.re).collect::<Vec<f32>>(), 0.0)
            } else {
                Self::project_symbols_per_pcg(&raw_symbols, prev_axis_phase)
            };
            let pcg_deinterleaved = Self::deinterleave_frame_soft(&pcg_soft);

            // RC3 frame decode is intentionally simple: estimate one whole-frame
            // axis first, always trying full rate before any locked/subrate
            // hypothesis so reverse signaling cannot be starved by an earlier
            // low-rate PHY-valid decode. If that misses CRC, retry using the
            // per-PCG adaptive axis with the same rate order.
            let estimated_decoded =
                self.decode_with_rate_priority(&estimated_deinterleaved, self.locked_rate);
            let pcg_decoded = (!estimated_decoded.validation.phy_valid)
                .then(|| self.decode_with_rate_priority(&pcg_deinterleaved, self.locked_rate));
            let pcg_phy_valid = pcg_decoded
                .as_ref()
                .is_some_and(|decoded| decoded.validation.phy_valid);
            let (decoded, selected_deinterleaved, recovered_axis_phase, selected_source) =
                if estimated_decoded.validation.phy_valid {
                    (
                        estimated_decoded,
                        &estimated_deinterleaved,
                        estimated_axis_phase,
                        "whole",
                    )
                } else if pcg_phy_valid {
                    (
                        pcg_decoded
                            .clone()
                            .expect("pcg decode should exist when phy_valid is true"),
                        &pcg_deinterleaved,
                        pcg_axis_phase,
                        "pcg",
                    )
                } else if estimated_decoded.validation.tail_valid {
                    (
                        estimated_decoded,
                        &estimated_deinterleaved,
                        estimated_axis_phase,
                        "whole",
                    )
                } else if let Some(decoded) = pcg_decoded {
                    (decoded, &pcg_deinterleaved, pcg_axis_phase, "pcg")
                } else {
                    (
                        estimated_decoded,
                        &estimated_deinterleaved,
                        estimated_axis_phase,
                        "whole",
                    )
                };
            // Decode the frame. Unlike the old searcher we never "lose
            // lock" — we always trust the 20ms boundary derived from
            // system time. If a frame fails CRC at every rate, we emit
            // a best-effort decode and move on to the next boundary.
            if decoded.validation.phy_valid {
                self.symbol_axis_phase = Some(recovered_axis_phase);
                self.locked_rate = Some(decoded.rate);
            }

            let is_preamble =
                decoded.rate == Rc3TrafficRate::Full && decoded.bits.iter().all(|b| *b == 0);

            self.maybe_log_lower_rate_diagnostic(
                frame_chip_start,
                decoded.rate,
                selected_source,
                recovered_axis_phase,
                selected_deinterleaved,
            );

            trace!(
                "rc3_frame_aligner: emit rate={} chip_start={} fqi_valid={} tail_valid={} preamble={}",
                decoded.rate.rate_bps(),
                self.chip_start,
                decoded.validation.fqi_valid,
                decoded.validation.tail_valid,
                is_preamble,
            );

            let mut tags = self.tags.clone();
            tags.insert("traffic_decoded_frame", 1);
            tags.insert("traffic_frame_aligned", 1);
            tags.insert("traffic_rate_bps", decoded.rate.rate_bps() as i64);
            tags.insert("traffic_info_bits", decoded.rate.info_bits() as i64);
            tags.insert("traffic_fqi_bits", decoded.rate.fqi_bits() as i64);
            tags.insert("traffic_tail_bits", decoded.rate.tail_bits() as i64);
            tags.insert("traffic_fqi_valid", decoded.validation.fqi_valid as i64);
            tags.insert("traffic_tail_valid", decoded.validation.tail_valid as i64);
            tags.insert("traffic_phy_valid", decoded.validation.phy_valid as i64);
            tags.insert("traffic_is_preamble", is_preamble as i64);
            if let Some(abs) = self.absolute_chip_start {
                tags.insert("absolute_chip_start", abs);
            }
            if decoded.rate == Rc3TrafficRate::Full
                && decoded.bits.len() >= decoded.rate.info_bits()
                && let Some(format) =
                    parse_reverse_mux1_full_rate_format(&decoded.bits[..decoded.rate.info_bits()])
            {
                tags.insert("traffic_mux_header", format.mux_header as i64);
                tags.insert("traffic_mux_header_bits", format.header_bits as i64);
                tags.insert("traffic_mux_primary_bits", format.primary_bits as i64);
                tags.insert("traffic_mux_signaling_bits", format.signaling_bits as i64);
            }

            // Per-PCG traffic Eb/Nt diagnostics over the aligned frame.
            let mask = self.active_pcg_mask_for_rate(decoded.rate);
            let pcg_snr_db = if self.frame_duration == FrameDuration::TwentyMs {
                let default_axis = if self.pilot_coherent { 0.0 } else { -FRAC_PI_2 };
                let prev_phase = self.symbol_axis_phase.unwrap_or(default_axis);
                Self::pcg_eb_nt_db_for_frame(&raw_symbols, prev_phase, decoded.rate, &mask)
            } else {
                None // 5ms frames don't map cleanly to 16 PCGs
            };
            if let Some(ref pcg_db) = pcg_snr_db {
                log::trace!(
                    "rc3_frame_aligner[w{}]: pcg_eb_nt chip={} rate={} [{}]",
                    self.walsh_code.unwrap_or(0),
                    frame_chip_start,
                    decoded.rate.rate_bps(),
                    pcg_db
                        .iter()
                        .enumerate()
                        .map(|(i, v)| if mask[i] {
                            format!("{:.1}", v)
                        } else {
                            "---".to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            let active_pcg_mask: Option<[bool; 16]> =
                if self.frame_duration == FrameDuration::TwentyMs {
                    Some(mask)
                } else {
                    None
                };

            let samples = decoded
                .bits
                .iter()
                .take(decoded.rate.frame_bits())
                .map(|&bit| Complex32::new(bit as f32, 0.0))
                .collect::<Vec<_>>();
            let mut block = SampleBlock::new(samples, frame_chip_start)
                .with_sample_rate_hz(self.sample_rate_hz);
            block.tags = tags;
            block.pcg_signal_snr_db = pcg_snr_db;
            block.active_pcg_mask = active_pcg_mask;
            out.push(block);

            let frame_chips = self.frame_chips();
            self.chip_start += frame_chips;
            if let Some(abs) = &mut self.absolute_chip_start {
                *abs = abs.saturating_add(frame_chips as i64);
            }
            self.symbol_buf.drain(..soft_bits_per_frame);
            // Drain corresponding pilot metrics for this frame.
            let pcgs_per_frame = soft_bits_per_frame / SOFT_BITS_PER_SYMBOL / SYMBOLS_PER_PCG;
            let drain_pcgs = pcgs_per_frame.min(self.pilot_metrics_buf.len());
            self.pilot_metrics_buf.drain(..drain_pcgs);
        }
        out
    }

    fn drain_front(&mut self, n_bits: usize) {
        let n = n_bits.min(self.symbol_buf.len());
        self.symbol_buf.drain(..n);
        let symbols_drained = n / SOFT_BITS_PER_SYMBOL;
        // Drain corresponding pilot metrics (one entry per PCG).
        let pcgs_drained = symbols_drained / SYMBOLS_PER_PCG;
        let drain_pcgs = pcgs_drained.min(self.pilot_metrics_buf.len());
        self.pilot_metrics_buf.drain(..drain_pcgs);
        let chip_advance = symbols_drained * CHIPS_PER_SYMBOL;
        self.chip_start = self.chip_start.saturating_add(chip_advance);
        if let Some(abs) = &mut self.absolute_chip_start {
            *abs = abs.saturating_add(chip_advance as i64);
        }
    }

    /// Advance through the state machine: compute the boundary if we
    /// haven't, drain to it, then emit frames from any complete frames
    /// sitting in the buffer.
    fn step(&mut self) -> Vec<SampleBlock> {
        if self.state == FrameAlignerState::WaitingForChipTag {
            if let Some(drain_bits) = self.soft_bits_to_next_frame_boundary() {
                debug!(
                    "rc3_frame_aligner: aligning to {:?} boundary \
                     drain_symbols={} absolute_chip_start={:?} chip_start={}",
                    self.frame_duration,
                    drain_bits / SOFT_BITS_PER_SYMBOL,
                    self.absolute_chip_start,
                    self.chip_start,
                );
                self.state = FrameAlignerState::DrainingToBoundary {
                    drain_target_symbols: drain_bits / SOFT_BITS_PER_SYMBOL,
                };
            }
        }

        if let FrameAlignerState::DrainingToBoundary {
            drain_target_symbols,
        } = self.state
        {
            let target_bits = drain_target_symbols * SOFT_BITS_PER_SYMBOL;
            if self.symbol_buf.len() >= target_bits {
                self.drain_front(target_bits);
                self.state = FrameAlignerState::Aligned;
                self.tags.insert("traffic_frame_aligned", 1);
                debug!(
                    "rc3_frame_aligner: aligned at chip_start={} \
                     absolute_chip_start={:?} frame_duration={:?}",
                    self.chip_start, self.absolute_chip_start, self.frame_duration,
                );
            } else {
                return Vec::new();
            }
        }

        if self.state == FrameAlignerState::Aligned {
            // PCG measurements are now emitted early by Rc3BpskDespread
            // via the PipelineEmitter for minimal latency.
            return self.emit_frames();
        }
        Vec::new()
    }
}

impl PipelineProcessor for Rc3FrameAligner {
    fn name(&self) -> &'static str {
        "Rc3FrameAligner"
    }

    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        // Pass through empty event blocks (e.g. preamble detection) unchanged.
        if block.samples.is_empty() {
            return vec![block];
        }

        if self.symbol_buf.is_empty() {
            self.chip_start = block.chip_start;
            self.sample_rate_hz = block.sample_rate_hz;
        }
        for (&k, &v) in &block.tags {
            self.tags.insert(k, v);
        }
        if !self.pilot_coherent && block.tags.get("pilot_coherent").is_some() {
            self.pilot_coherent = true;
        }
        self.refresh_absolute_chip_tags();
        for &symbol in &block.samples {
            self.symbol_buf.push_back(symbol);
        }
        // Ingest per-PCG pilot metrics from the despreader.
        if let Some(metrics) = &block.pcg_pilot_metrics {
            for &m in metrics {
                self.pilot_metrics_buf.push_back(m);
            }
        }

        self.step()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.step()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DecodedFrame, FRAME_SYMBOLS_20MS, FrameValidation, Rc3FrameAligner, Rc3TrafficRate,
    };
    use cdma_common::crc::{crc6, crc8, crc12};

    fn deterministic_soft_input() -> Vec<f32> {
        (0..FRAME_SYMBOLS_20MS)
            .map(|i| {
                let x = ((i * 37 + 11) % 257) as f32;
                (x - 128.0) / 64.0
            })
            .collect()
    }

    fn assert_same_decode(lhs: &DecodedFrame, rhs: &DecodedFrame) {
        assert_eq!(lhs.rate, rhs.rate);
        assert_eq!(lhs.bits, rhs.bits);
        assert_eq!(lhs.validation, rhs.validation);
    }

    fn valid_rate_frame(rate: Rc3TrafficRate) -> Vec<u8> {
        let mut bits = vec![0u8; rate.frame_bits()];
        for (idx, bit) in bits[..rate.info_bits()].iter_mut().enumerate() {
            *bit = ((idx * 17 + 5) & 1) as u8;
        }

        let fqi_start = rate.info_bits();
        match rate {
            Rc3TrafficRate::Full => {
                let crc = crc12(&bits[..rate.info_bits()]);
                for bit_idx in 0..rate.fqi_bits() {
                    bits[fqi_start + bit_idx] =
                        ((crc >> (rate.fqi_bits() - 1 - bit_idx)) & 1) as u8;
                }
            }
            Rc3TrafficRate::Half => {
                let crc = crc8(&bits[..rate.info_bits()]);
                for bit_idx in 0..rate.fqi_bits() {
                    bits[fqi_start + bit_idx] =
                        ((crc >> (rate.fqi_bits() - 1 - bit_idx)) & 1) as u8;
                }
            }
            Rc3TrafficRate::Quarter | Rc3TrafficRate::Eighth => {
                let crc = crc6(&bits[..rate.info_bits()]);
                for bit_idx in 0..rate.fqi_bits() {
                    bits[fqi_start + bit_idx] =
                        ((crc >> (rate.fqi_bits() - 1 - bit_idx)) & 1) as u8;
                }
            }
        }

        bits
    }

    #[test]
    fn rc3_subrate_validation_requires_real_fqi_crc() {
        for rate in [
            Rc3TrafficRate::Half,
            Rc3TrafficRate::Quarter,
            Rc3TrafficRate::Eighth,
        ] {
            let mut bits = valid_rate_frame(rate);
            let valid = FrameValidation::for_rate(rate, &bits);
            assert!(valid.tail_valid, "tail should be valid for {rate:?}");
            assert!(valid.fqi_valid, "FQI should validate for {rate:?}");
            assert!(valid.phy_valid, "PHY frame should validate for {rate:?}");

            bits[rate.info_bits()] ^= 1;
            let invalid = FrameValidation::for_rate(rate, &bits);
            assert!(invalid.tail_valid, "tail should stay valid for {rate:?}");
            assert!(
                !invalid.fqi_valid,
                "flipping one FQI bit must fail for {rate:?}"
            );
            assert!(
                !invalid.phy_valid,
                "sub-rate RC3 must not be accepted on tail alone for {rate:?}"
            );
        }
    }

    #[test]
    fn rc3_decode_from_deinterleaved_matches_legacy_wrapper() {
        let soft = deterministic_soft_input();
        let deinterleaved = Rc3FrameAligner::deinterleave_frame_soft(&soft);

        for rate in Rc3TrafficRate::SEARCH_ORDER {
            let legacy = Rc3FrameAligner::decode_frame_soft(&soft, rate);
            let optimized = Rc3FrameAligner::decode_frame_from_deinterleaved(&deinterleaved, rate);
            assert_same_decode(&legacy, &optimized);
        }
    }

    #[test]
    fn rc3_rate_priority_always_tries_full_first() {
        assert_eq!(
            Rc3FrameAligner::build_rate_priority(None),
            vec![
                Rc3TrafficRate::Full,
                Rc3TrafficRate::Half,
                Rc3TrafficRate::Quarter,
                Rc3TrafficRate::Eighth,
            ]
        );
        assert_eq!(
            Rc3FrameAligner::build_rate_priority(Some(Rc3TrafficRate::Quarter)),
            vec![
                Rc3TrafficRate::Full,
                Rc3TrafficRate::Quarter,
                Rc3TrafficRate::Half,
                Rc3TrafficRate::Eighth,
            ]
        );
        assert_eq!(
            Rc3FrameAligner::build_rate_priority(Some(Rc3TrafficRate::Full)),
            vec![
                Rc3TrafficRate::Full,
                Rc3TrafficRate::Half,
                Rc3TrafficRate::Quarter,
                Rc3TrafficRate::Eighth,
            ]
        );
    }

    // -----------------------------------------------------------------------
    // Eb/Nt measurement verification against known chip-level SNR
    // -----------------------------------------------------------------------

    use num_complex::Complex32;

    /// Generate one PCG (96 symbols) of BPSK signal + complex AWGN at a
    /// known per-chip SNR.  Each symbol represents 16 Walsh-despread chips,
    /// so:
    ///   signal per symbol  = 16 × A           (coherent Walsh sum)
    ///   noise per symbol   = sum of 16 iid CN(0, σ²) scaled by ±1
    ///                      → CN(0, 16σ²)
    ///
    /// We simulate the post-Walsh output directly: place the signal on the
    /// real axis (BPSK after pilot correction + −j rotation) and add
    /// circular Gaussian noise with the correct per-symbol variance.
    fn generate_pcg_at_chip_snr(chip_snr_linear: f32, seed: u64) -> Vec<Complex32> {
        // Signal amplitude per chip = 1.0, so Ec = 1.0
        // Noise variance per chip (complex) = σ² = Ec / snr = 1/snr
        // After Walsh(16): signal = 16, noise variance = 16 × σ² = 16/snr
        // Per-dimension noise std = sqrt(16/(2×snr)) = sqrt(8/snr)
        let signal_per_symbol = 16.0_f32;
        let noise_var_per_dim = 8.0 / chip_snr_linear;
        let noise_std = noise_var_per_dim.sqrt();

        // Simple deterministic pseudo-random for reproducibility.
        // Use a basic xorshift64 seeded from the input.
        let mut rng_state = seed ^ 0xDEAD_BEEF_CAFE_1234;
        let mut next_u64 = || -> u64 {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            rng_state
        };
        // Box-Muller for Gaussian samples
        let mut next_gaussian = || -> (f32, f32) {
            let u1 = (next_u64() as f64 / u64::MAX as f64).max(1e-15);
            let u2 = next_u64() as f64 / u64::MAX as f64;
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            ((r * theta.cos()) as f32, (r * theta.sin()) as f32)
        };

        let mut symbols = Vec::with_capacity(super::SYMBOLS_PER_PCG);
        for i in 0..super::SYMBOLS_PER_PCG {
            // BPSK: alternating +1/-1 data modulation
            let data = if i % 2 == 0 { 1.0_f32 } else { -1.0_f32 };
            let sig = signal_per_symbol * data;
            let (n_re, n_im) = next_gaussian();
            symbols.push(Complex32::new(
                sig + n_re as f32 * noise_std,
                n_im as f32 * noise_std,
            ));
        }
        symbols
    }

    /// Expected Eb/Nt (dB) given a chip-level Ec/Nt (linear) for full
    /// rate RC3.
    ///
    ///   Eb/Nt = (24576 / N_info) × (Ec / Nt)
    ///
    /// For full rate: 24576 / 172 = 142.93
    fn expected_eb_nt_db(chip_snr_linear: f32, rate: Rc3TrafficRate) -> f32 {
        let total_chips = super::FRAME_SYMBOLS_20MS * super::CHIPS_PER_SYMBOL; // 24576
        let processing_gain = total_chips as f32 / rate.info_bits() as f32;
        10.0 * (processing_gain * chip_snr_linear).log10()
    }

    #[test]
    fn eb_nt_matches_known_chip_snr_full_rate() {
        // Test at several chip-level SNR points
        let test_points: &[(f32, &str)] = &[
            (10.0, "10 dB chip SNR"),
            (1.0, "0 dB chip SNR"),
            (0.1, "-10 dB chip SNR"),
            (0.01, "-20 dB chip SNR"),
        ];

        for &(chip_snr, label) in test_points {
            let expected_db = expected_eb_nt_db(chip_snr, Rc3TrafficRate::Full);

            // Average over multiple trials to reduce noise in the estimate
            let trials = 200;
            let mut sum_db = 0.0_f64;
            for trial in 0..trials {
                let symbols = generate_pcg_at_chip_snr(chip_snr, trial as u64);
                let (eb_nt_db, _phase) = Rc3FrameAligner::pcg_eb_nt_db(
                    &symbols,
                    0.0, // prev axis phase (signal is on real axis)
                    Rc3TrafficRate::Full,
                );
                sum_db += eb_nt_db as f64;
            }
            let measured_db = (sum_db / trials as f64) as f32;
            let error = (measured_db - expected_db).abs();

            eprintln!(
                "{}: expected={:.2} dB  measured={:.2} dB  error={:.2} dB",
                label, expected_db, measured_db, error,
            );
            assert!(
                error < 1.5,
                "{}: Eb/Nt error {:.2} dB exceeds 1.5 dB tolerance \
                 (expected={:.2}, measured={:.2})",
                label,
                error,
                expected_db,
                measured_db,
            );
        }
    }

    #[test]
    fn eb_nt_matches_known_chip_snr_eighth_rate() {
        // Eighth rate has much higher processing gain (24576/16 = 1536)
        let chip_snr = 1.0_f32; // 0 dB chip SNR
        let expected_db = expected_eb_nt_db(chip_snr, Rc3TrafficRate::Eighth);

        let trials = 200;
        let mut sum_db = 0.0_f64;
        for trial in 0..trials {
            let symbols = generate_pcg_at_chip_snr(chip_snr, trial as u64 + 10000);
            let (eb_nt_db, _) =
                Rc3FrameAligner::pcg_eb_nt_db(&symbols, 0.0, Rc3TrafficRate::Eighth);
            sum_db += eb_nt_db as f64;
        }
        let measured_db = (sum_db / trials as f64) as f32;
        let error = (measured_db - expected_db).abs();

        eprintln!(
            "eighth rate 0dB chip: expected={:.2} dB  measured={:.2} dB  error={:.2} dB",
            expected_db, measured_db, error,
        );
        assert!(
            error < 1.5,
            "Eighth rate Eb/Nt error {:.2} dB exceeds 1.5 dB (expected={:.2}, measured={:.2})",
            error,
            expected_db,
            measured_db,
        );
    }

    /// Encode a valid full-rate RC3 frame through the full TX chain
    /// (R=1/4 K=9 conv → 2× repeat → interleave) and return 1536
    /// interleaved BPSK symbols as ±1.0 floats.
    fn encode_full_rate_frame(info_bits: &[u8]) -> Vec<f32> {
        use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_1536};
        use crate::phy::coding::convolutional::get_1_4_k9_encoder;

        // Build frame: info (172) + CRC12 + 8 tail = 192 bits
        let mut frame = Vec::with_capacity(192);
        for i in 0..172 {
            frame.push(*info_bits.get(i).unwrap_or(&0));
        }
        let crc = crc12(&frame[..172]);
        for bit in (0..12).rev() {
            frame.push(((crc >> bit) & 1) as u8);
        }
        frame.extend(std::iter::repeat_n(0u8, 8)); // tail
        assert_eq!(frame.len(), 192);

        // R=1/4 K=9 convolutional encode → 768 coded symbols
        let mut encoder = get_1_4_k9_encoder();
        let mut code_symbols = Vec::with_capacity(768);
        for &bit in &frame {
            code_symbols.extend_from_slice(&encoder.encode(bit));
        }
        assert_eq!(code_symbols.len(), 768);

        // 2× repeat → 1536 symbols
        let repeated: Vec<u8> = code_symbols
            .iter()
            .flat_map(|&s| std::iter::repeat_n(s, 2))
            .collect();
        assert_eq!(repeated.len(), 1536);

        // Bit-reversal interleave
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_1536);
        let interleaved = interleaver.encode(&repeated);

        // Map 0→+1.0, 1→-1.0
        interleaved
            .into_iter()
            .map(|bit| if bit == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    /// Simulate post-Walsh(16) symbols at a given chip-level SNR.
    ///
    /// Takes 1536 encoded BPSK symbols (±1.0) and produces 1536 complex
    /// symbols as if they had been through Walsh despreading:
    ///   signal_per_symbol = 16 × data (coherent Walsh sum)
    ///   noise ~ CN(0, 16/chip_snr) per symbol
    ///
    /// Signal is placed on the real axis (post pilot-correction + −j).
    fn add_noise_post_walsh(
        encoded_symbols: &[f32],
        chip_snr_linear: f32,
        seed: u64,
    ) -> Vec<Complex32> {
        let signal_scale = 16.0_f32;
        let noise_std_per_dim = (8.0 / chip_snr_linear).sqrt();

        let mut rng_state = seed ^ 0xABCD_EF01_2345_6789;
        let mut next_u64 = || -> u64 {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            rng_state
        };
        let mut next_gaussian = || -> (f32, f32) {
            let u1 = (next_u64() as f64 / u64::MAX as f64).max(1e-15);
            let u2 = next_u64() as f64 / u64::MAX as f64;
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            ((r * theta.cos()) as f32, (r * theta.sin()) as f32)
        };

        encoded_symbols
            .iter()
            .map(|&data| {
                let (n_re, n_im) = next_gaussian();
                Complex32::new(
                    signal_scale * data + n_re * noise_std_per_dim,
                    n_im * noise_std_per_dim,
                )
            })
            .collect()
    }

    #[test]
    fn decoder_succeeds_at_low_eb_nt() {
        // Generate a valid full-rate frame
        let info_bits: Vec<u8> = (0..172).map(|i| ((i * 13 + 7) & 1) as u8).collect();
        let encoded = encode_full_rate_frame(&info_bits);

        // Test at several chip-SNR points
        let test_points: &[(f32, &str, bool)] = &[
            (10.0, "10 dB chip (31.5 dB Eb/Nt)", true),
            (1.0, "0 dB chip (21.5 dB Eb/Nt)", true),
            (0.1, "-10 dB chip (11.5 dB Eb/Nt)", true),
            (0.01, "-20 dB chip (1.5 dB Eb/Nt)", false), // may or may not decode
        ];

        for &(chip_snr, label, must_pass) in test_points {
            let mut pass_count = 0;
            let trials = 50;
            for trial in 0..trials {
                let noisy = add_noise_post_walsh(&encoded, chip_snr, trial as u64);

                // Project onto real axis (signal is already there)
                let soft: Vec<f32> = noisy.iter().map(|s| s.re).collect();

                // Decode through the standard RC3 path
                let decoded = Rc3FrameAligner::decode_frame_soft(&soft, Rc3TrafficRate::Full);
                if decoded.validation.fqi_valid {
                    pass_count += 1;
                }
            }
            let fer_pct = 100.0 * (1.0 - pass_count as f64 / trials as f64);
            let expected_eb_nt = expected_eb_nt_db(chip_snr, Rc3TrafficRate::Full);
            eprintln!(
                "{}: FER={:.1}% ({}/{} passed) Eb/Nt={:.1} dB",
                label, fer_pct, pass_count, trials, expected_eb_nt,
            );
            if must_pass {
                assert!(
                    pass_count >= trials * 9 / 10,
                    "{}: expected ≥90% decode rate, got {}/{}",
                    label,
                    pass_count,
                    trials,
                );
            }
        }
    }

    #[test]
    fn eb_nt_noise_only_reports_very_low() {
        // With no signal (pure noise), Eb/Nt should be near 0 or negative dB
        let noise_std = 1.0_f32;
        let mut rng_state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next_u64 = || -> u64 {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            rng_state
        };
        let mut next_gaussian = || -> (f32, f32) {
            let u1 = (next_u64() as f64 / u64::MAX as f64).max(1e-15);
            let u2 = next_u64() as f64 / u64::MAX as f64;
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            ((r * theta.cos()) as f32, (r * theta.sin()) as f32)
        };

        let symbols: Vec<Complex32> = (0..super::SYMBOLS_PER_PCG)
            .map(|_| {
                let (re, im) = next_gaussian();
                Complex32::new(re as f32 * noise_std, im as f32 * noise_std)
            })
            .collect();

        let (eb_nt_db, _) = Rc3FrameAligner::pcg_eb_nt_db(&symbols, 0.0, Rc3TrafficRate::Full);
        eprintln!("noise-only: eb_nt_db={:.2}", eb_nt_db);
        // With 96 samples of pure noise, the M2 estimator will find a
        // spurious axis, biasing Es slightly above Nn.  The reported
        // Eb/Nt should still be well below any operational threshold.
        assert!(
            eb_nt_db < 10.0,
            "noise-only Eb/Nt should be low, got {:.2} dB",
            eb_nt_db
        );
    }

    /// Verify the pilot Ec/Io helper against exact power-ratio cases.
    #[test]
    fn pilot_ec_io_matches_known_power_ratio() {
        let n = super::SYMBOLS_PER_PCG as f32;
        let n_chips = super::PCG_CHIPS as f32;
        let cases = [
            ("pilot only", n * n, n_chips, 0.0_f32),
            (
                "3 dB total overhead",
                n * n,
                2.0 * n_chips,
                -3.010_300_2_f32,
            ),
            (
                "6 dB total overhead",
                n * n,
                4.0 * n_chips,
                -6.020_600_3_f32,
            ),
        ];

        for &(label, pilot_norm_sq, chip_power_sum, expected_db) in &cases {
            let measured_db = Rc3FrameAligner::pcg_pilot_ec_io_db(pilot_norm_sq, chip_power_sum);
            let error = (measured_db - expected_db).abs();
            eprintln!(
                "{}: expected={:.2} dB measured={:.2} dB error={:.4} dB",
                label, expected_db, measured_db, error,
            );
            assert!(
                error < 1e-4,
                "{}: Ec/Io mismatch expected {:.4} dB measured {:.4} dB",
                label,
                expected_db,
                measured_db,
            );
        }
    }

    /// Sweep pilot phase estimation error and measure the FER impact.
    ///
    /// At 0 dB chip SNR (21.5 dB Eb/Nt) the decoder handles noise easily.
    /// This test applies a per-PCG random phase error (simulating a noisy
    /// pilot estimate) and measures how many dB of phase noise it takes
    /// to break decoding.
    #[test]
    fn phase_error_impact_on_fer() {
        let info_bits: Vec<u8> = (0..172).map(|i| ((i * 13 + 7) & 1) as u8).collect();
        let encoded = encode_full_rate_frame(&info_bits);
        let chip_snr = 1.0_f32; // 0 dB chip SNR → 21.5 dB Eb/Nt (perfect decode)
        let trials = 100;

        // Phase error standard deviations to sweep (radians)
        let phase_errors_deg: &[f32] = &[0.0, 2.0, 5.0, 10.0, 15.0, 20.0, 30.0, 45.0, 60.0];

        eprintln!("\n=== Phase Error Impact on FER (0 dB chip SNR, 21.5 dB Eb/Nt) ===");
        eprintln!(
            "{:<12} {:<10} {:<12} {:<10}",
            "phase_std", "FER%", "pass/total", "Eb/Nt_meas"
        );

        for &phase_std_deg in phase_errors_deg {
            let phase_std_rad = phase_std_deg * std::f32::consts::PI / 180.0;
            let mut pass_count = 0;
            let mut eb_nt_sum = 0.0_f64;

            for trial in 0..trials {
                // Generate clean post-Walsh symbols with noise
                let noisy = add_noise_post_walsh(&encoded, chip_snr, trial as u64 + 50000);

                // Apply per-PCG phase rotation error (simulating noisy pilot
                // phase estimate — one random rotation applied to all 96
                // symbols in each PCG)
                let mut rng_state: u64 = trial as u64 ^ 0xFACE_CAFE_0000_0000;
                let mut next_u64 = || -> u64 {
                    rng_state ^= rng_state << 13;
                    rng_state ^= rng_state >> 7;
                    rng_state ^= rng_state << 17;
                    rng_state
                };

                let mut phase_corrupted = Vec::with_capacity(noisy.len());
                for pcg in 0..super::SR1_PCGS_PER_FRAME {
                    // Generate a random phase error for this PCG
                    // Box-Muller for one Gaussian sample
                    let u1 = (next_u64() as f64 / u64::MAX as f64).max(1e-15);
                    let u2 = next_u64() as f64 / u64::MAX as f64;
                    let gaussian =
                        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
                    let phase_err = gaussian as f32 * phase_std_rad;
                    let rot = Complex32::new(phase_err.cos(), phase_err.sin());

                    let start = pcg * super::SYMBOLS_PER_PCG;
                    let end = start + super::SYMBOLS_PER_PCG;
                    for &sym in &noisy[start..end] {
                        phase_corrupted.push(sym * rot);
                    }
                }

                // Measure Eb/Nt (the measurement also estimates axis, so it
                // partially adapts to the phase error)
                let mut pcg_eb_nt_sum = 0.0_f64;
                let mut axis = 0.0_f32;
                for pcg in 0..super::SR1_PCGS_PER_FRAME {
                    let start = pcg * super::SYMBOLS_PER_PCG;
                    let end = start + super::SYMBOLS_PER_PCG;
                    let (db, next_axis) = Rc3FrameAligner::pcg_eb_nt_db(
                        &phase_corrupted[start..end],
                        axis,
                        Rc3TrafficRate::Full,
                    );
                    axis = next_axis;
                    pcg_eb_nt_sum += db as f64;
                }
                eb_nt_sum += pcg_eb_nt_sum / super::SR1_PCGS_PER_FRAME as f64;

                // Decode — project onto real axis (imperfect due to phase error)
                let soft: Vec<f32> = phase_corrupted.iter().map(|s| s.re).collect();
                let decoded = Rc3FrameAligner::decode_frame_soft(&soft, Rc3TrafficRate::Full);
                if decoded.validation.fqi_valid {
                    pass_count += 1;
                }
            }

            let fer_pct = 100.0 * (1.0 - pass_count as f64 / trials as f64);
            let avg_eb_nt = eb_nt_sum / trials as f64;
            eprintln!(
                "{:>8.1}°    {:>6.1}%    {:>4}/{:<4}     {:.1} dB",
                phase_std_deg, fer_pct, pass_count, trials, avg_eb_nt,
            );
        }
    }

    /// Sweep residual CFO (frequency offset) and measure FER impact.
    ///
    /// Unlike the per-PCG phase error test, CFO causes a *linear phase ramp*
    /// within each PCG — the phase changes from symbol to symbol.  The M2
    /// axis estimator finds the average axis but individual symbols drift
    /// away from it, causing irreducible degradation that coding can't
    /// fully recover from.
    #[test]
    fn cfo_phase_ramp_impact_on_fer() {
        let info_bits: Vec<u8> = (0..172).map(|i| ((i * 13 + 7) & 1) as u8).collect();
        let encoded = encode_full_rate_frame(&info_bits);
        let chip_snr = 1.0_f32; // 0 dB chip SNR → 21.5 dB Eb/Nt
        let trials = 100;

        // CFO in Hz — each symbol spans 16 chips at 1.2288 Mcps = 13.02 μs
        // Phase per symbol = 2π × cfo_hz × 16 / 1_228_800
        let cfo_hz_values: &[f32] = &[
            0.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
        ];

        eprintln!("\n=== CFO (Within-PCG Phase Ramp) Impact on FER (0 dB chip SNR) ===");
        eprintln!(
            "{:<10} {:<14} {:<14} {:<10} {:<12} {:<10}",
            "CFO_Hz", "°/symbol", "°/PCG", "FER%", "pass/total", "Eb/Nt"
        );

        for &cfo_hz in cfo_hz_values {
            let phase_per_symbol = 2.0 * std::f32::consts::PI * cfo_hz * 16.0 / 1_228_800.0;
            let deg_per_symbol = phase_per_symbol * 180.0 / std::f32::consts::PI;
            let deg_per_pcg = deg_per_symbol * super::SYMBOLS_PER_PCG as f32;

            let mut pass_count = 0;
            let mut eb_nt_sum = 0.0_f64;

            for trial in 0..trials {
                let noisy = add_noise_post_walsh(&encoded, chip_snr, trial as u64 + 70000);

                // Apply linear phase ramp across the entire frame
                let phase_ramped: Vec<Complex32> = noisy
                    .iter()
                    .enumerate()
                    .map(|(i, &sym)| {
                        let phase = phase_per_symbol * i as f32;
                        let rot = Complex32::new(phase.cos(), phase.sin());
                        sym * rot
                    })
                    .collect();

                // Measure Eb/Nt per-PCG (M2 estimator adapts per-PCG)
                let mut pcg_eb_nt_sum = 0.0_f64;
                let mut axis = 0.0_f32;
                for pcg in 0..super::SR1_PCGS_PER_FRAME {
                    let start = pcg * super::SYMBOLS_PER_PCG;
                    let end = start + super::SYMBOLS_PER_PCG;
                    let (db, next_axis) = Rc3FrameAligner::pcg_eb_nt_db(
                        &phase_ramped[start..end],
                        axis,
                        Rc3TrafficRate::Full,
                    );
                    axis = next_axis;
                    pcg_eb_nt_sum += db as f64;
                }
                eb_nt_sum += pcg_eb_nt_sum / super::SR1_PCGS_PER_FRAME as f64;

                // Decode using per-PCG axis estimation (matches live path)
                let (soft, _) = Rc3FrameAligner::project_symbols_per_pcg(&phase_ramped, 0.0);
                let decoded = Rc3FrameAligner::decode_frame_soft(&soft, Rc3TrafficRate::Full);
                if decoded.validation.fqi_valid {
                    pass_count += 1;
                }
            }

            let fer_pct = 100.0 * (1.0 - pass_count as f64 / trials as f64);
            let avg_eb_nt = eb_nt_sum / trials as f64;
            eprintln!(
                "{:>8.0}   {:>10.3}°    {:>10.1}°   {:>6.1}%    {:>4}/{:<4}     {:.1} dB",
                cfo_hz, deg_per_symbol, deg_per_pcg, fer_pct, pass_count, trials, avg_eb_nt,
            );
        }
    }
}
