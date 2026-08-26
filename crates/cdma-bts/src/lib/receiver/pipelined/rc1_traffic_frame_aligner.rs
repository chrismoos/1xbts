use std::collections::HashMap;

use cdma_common::bits::Bitstream;
use cdma_common::crc::{crc8, crc12};
use log::{debug, info};
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock, raw_to_soft};
use crate::phy::coding::block_interleaver::{
    Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
};
use crate::phy::coding::convolutional::get_1_3_k9_soft_viterbi_decoder;
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::walsh::WalshGenerator;
use crate::receiver::access::DedicatedFrameReader;
use crate::receiver::pipelined::traffic_channel_processor::{
    ReverseMux1SignalingLayout, extract_reverse_mux1_full_rate_signaling_block,
    parse_reverse_mux1_full_rate_format,
};

use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME,
    RC1_SYMBOLS_PER_PCG, RC1_WALSH_CHIPS_PER_SYMBOL, SR1_PCGS_PER_FRAME,
};

const SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const SOFT_BITS_PER_PCG: usize = RC1_SYMBOLS_PER_PCG * RC1_SOFT_BITS_PER_SYMBOL;
const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;
const PN_SHORT_CODE_CHIPS: usize = 32768;
const FRAME_CHIPS: usize = RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL;
const PCG_CHIPS: usize = RC1_SYMBOLS_PER_PCG * PN_CHIPS_PER_SYMBOL;
const MIN_LOCK_FRAMES: usize = 2;
const MAX_LOCK_FRAMES: usize = 128;
const MAX_SEARCH_BUFFER_CHIPS: usize = FRAME_CHIPS * (MAX_LOCK_FRAMES + 3);
/// Maximum number of search attempts before giving up. Each attempt covers
/// up to MAX_LOCK_FRAMES frames × multiple chip/frame phases. 32 attempts
/// at 8-frame (160ms) intervals ≈ 5 seconds — enough time for the mobile
/// to receive BS Ack, transition, and start sending full-rate frames.
const MAX_SEARCH_ATTEMPTS: usize = 32;
/// Minimum number of consecutive null frames (all-zero full-rate) after lock
/// before declaring preamble detected.  The mobile sends null frames as
/// reverse-traffic preamble; we need to see enough of them to be confident
/// the mobile is actually on-channel before triggering BS Ack.
const PREAMBLE_NULL_FRAME_THRESHOLD: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameAlignerState {
    /// Phase 1: looking for preamble (eighth-rate null frames).
    /// Walsh-demod + Viterbi decode at every chip_phase/frame_phase hypothesis,
    /// but only counting tail-valid eighth-rate frames.  Once we see
    /// PREAMBLE_NULL_FRAME_THRESHOLD consecutive null frames at a single
    /// hypothesis, emit a preamble event and transition to Locking.
    SearchingPreamble,
    /// Phase 2: preamble detected, now looking for the first full-rate CRC-valid
    /// frame (the mobile's MS Ack or first traffic frame after BS Ack).
    Locking,
    /// Locked on full-rate frames — steady-state decoding.
    Locked,
    /// The aligner exhausted its search budget without locking.
    /// No further processing will occur — the pipeline should be torn down.
    GaveUp,
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

    const fn active_pcg_count(self) -> usize {
        match self {
            Self::Full => 16,
            Self::Half => 8,
            Self::Quarter => 4,
            Self::Eighth => 2,
        }
    }

    fn active_pcgs_from_ranked_metric(self, metric: &[f32]) -> [bool; SR1_PCGS_PER_FRAME] {
        let mut active = [false; SR1_PCGS_PER_FRAME];
        if self == Self::Full {
            active.fill(true);
            return active;
        }

        let mut energies = [(0usize, 0.0f32); SR1_PCGS_PER_FRAME];
        for (pcg_idx, entry) in energies.iter_mut().enumerate() {
            *entry = (pcg_idx, metric[pcg_idx].abs());
        }
        energies.sort_by(|a, b| b.1.total_cmp(&a.1));
        for &(pcg_idx, _) in energies.iter().take(self.active_pcg_count()) {
            active[pcg_idx] = true;
        }
        active
    }

    fn active_pcgs_from_soft(self, raw_soft: &[f32]) -> [bool; SR1_PCGS_PER_FRAME] {
        if self == Self::Full {
            let mut active = [false; SR1_PCGS_PER_FRAME];
            active.fill(true);
            return active;
        }

        let mut pcg_energy = [0.0f32; SR1_PCGS_PER_FRAME];
        for (pcg_idx, energy) in pcg_energy.iter_mut().enumerate() {
            let start = pcg_idx * SOFT_BITS_PER_PCG;
            let end = start + SOFT_BITS_PER_PCG;
            *energy = raw_soft[start..end].iter().map(|v| v.abs()).sum::<f32>();
        }
        self.active_pcgs_from_ranked_metric(&pcg_energy)
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
    /// Raw Viterbi ML best terminal state after the forward pass
    /// (unconstrained). For a clean terminated block this should be
    /// 0, since the encoder drives to state 0 via 8 zero tail bits.
    /// Anything else is a soft signal that the decode is suspect:
    /// the bit value itself has a Hamming interpretation (=last 8
    /// info bits the ML path selected), and popcount() gives a
    /// "distance from zero-terminated" number in [0, 8].
    ml_terminal_best_state: u8,
    /// Convenience: `ml_terminal_best_state == 0`.
    ml_terminal_matches_zero: bool,
}

#[derive(Clone, Copy, Debug)]
struct LockCandidate {
    chip_phase: usize,
    frame_phase: usize,
    rate: Rc1TrafficRate,
    layout: ReverseMux1SignalingLayout,
    lock_frame_index: usize,
}

#[derive(Clone, Copy, Debug)]
struct SearchSummary {
    best_chip_phase: usize,
    best_frame_phase: usize,
    best_rate: Rc1TrafficRate,
    best_valid_full_frames: usize,
    best_valid_any_frames: usize,
}

/// Align RC1 reverse traffic frames by brute-forcing chip offsets and frame
/// phases directly on despread chip-rate samples.
///
/// The RC1 reverse traffic path does not use the access-style W0 post-preamble
/// transition heuristic. Instead, it:
/// 1. buffers despread chips from the Pn/Lc finger,
/// 2. tries every chip phase (0..255) and every frame phase (0..95 symbols),
/// 3. demodulates 64-ary Walsh symbols directly from chips,
/// 4. tries all four RC1 rates, and
/// 5. locks only when reverse dedicated SAR reassembly yields an actual
///    CRC-valid message.
/// Number of recent frames to track for rate adaptation.
const RATE_HISTORY_LEN: usize = 8;

pub struct Rc1TrafficFrameAligner {
    state: FrameAlignerState,
    chip_buf: Vec<Complex32>,
    tags: HashMap<&'static str, i64>,
    chip_start: usize,
    sample_rate_hz: f64,
    absolute_chip_start: Option<i64>,
    esn: u32,
    locked_rate: Option<Rc1TrafficRate>,
    locked_mux_layout: Option<ReverseMux1SignalingLayout>,
    last_search_frame_budget: usize,
    search_attempts: usize,
    preamble_event_sent: bool,
    /// Best preamble hypothesis found so far during SearchingPreamble.
    /// Tracks (chip_phase, frame_phase, consecutive_null_count).
    preamble_best: Option<(usize, usize, usize)>,
    /// Circular buffer of recently decoded rates for adaptive fast-path ordering.
    rate_history: [Rc1TrafficRate; RATE_HISTORY_LEN],
    rate_history_idx: usize,
    rate_history_count: usize,
    next_measurement_abs_pcg: Option<u64>,
    pcg_measurement_rate: Option<Rc1TrafficRate>,
    last_processing_absolute_chip_end: Option<u64>,
}

impl Rc1TrafficFrameAligner {
    pub fn new(esn: u32) -> Self {
        Self {
            state: FrameAlignerState::SearchingPreamble,
            chip_buf: Vec::new(),
            tags: HashMap::new(),
            chip_start: 0,
            sample_rate_hz: 0.0,
            absolute_chip_start: None,
            esn,
            locked_rate: None,
            locked_mux_layout: None,
            last_search_frame_budget: 0,
            search_attempts: 0,
            preamble_event_sent: false,
            preamble_best: None,
            rate_history: [Rc1TrafficRate::Eighth; RATE_HISTORY_LEN],
            rate_history_idx: 0,
            rate_history_count: 0,
            next_measurement_abs_pcg: None,
            pcg_measurement_rate: None,
            last_processing_absolute_chip_end: None,
        }
    }

    /// Record a decoded rate into the history ring buffer.
    fn record_rate(&mut self, rate: Rc1TrafficRate) {
        self.rate_history[self.rate_history_idx] = rate;
        self.rate_history_idx = (self.rate_history_idx + 1) % RATE_HISTORY_LEN;
        if self.rate_history_count < RATE_HISTORY_LEN {
            self.rate_history_count += 1;
        }
    }

    /// Return the rate search order adapted to recent history.
    /// The most frequently seen rate goes first, with ties broken by the
    /// default SEARCH_ORDER.
    fn adaptive_search_order(&self) -> [Rc1TrafficRate; 4] {
        if self.rate_history_count == 0 {
            return Rc1TrafficRate::SEARCH_ORDER;
        }
        let window = &self.rate_history[..self.rate_history_count.min(RATE_HISTORY_LEN)];
        let mut counts = [0u32; 4]; // Full, Half, Quarter, Eighth
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
        // Stable sort so default order is preserved for equal counts.
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

    fn emit_preamble_event(&self, preamble_frames: usize) -> SampleBlock {
        let mut tags = self.tags.clone();
        tags.insert("traffic_preamble_detected", 1);
        tags.insert("traffic_preamble_frames", preamble_frames.max(1) as i64);
        let mut block =
            SampleBlock::new(Vec::new(), self.chip_start).with_sample_rate_hz(self.sample_rate_hz);
        block.tags = tags;
        block
    }

    /// Format a bit slice as a MSB-first hex string, e.g. bits
    /// `[1,0,1,0,1,1,0,0]` → `"ac"`. Bits must be 0 or 1.
    fn format_bits_hex(bits: &[u8]) -> String {
        let mut out = String::with_capacity(bits.len().div_ceil(4));
        let mut nibble = 0u8;
        let mut bits_in_nibble = 0usize;
        for &b in bits {
            nibble = (nibble << 1) | (b & 1);
            bits_in_nibble += 1;
            if bits_in_nibble == 4 {
                out.push(char::from_digit(nibble as u32, 16).unwrap());
                nibble = 0;
                bits_in_nibble = 0;
            }
        }
        if bits_in_nibble > 0 {
            nibble <<= 4 - bits_in_nibble;
            out.push(char::from_digit(nibble as u32, 16).unwrap());
        }
        out
    }

    fn search_frame_count(&self) -> usize {
        let total_frames = self.chip_buf.len() / FRAME_CHIPS;
        total_frames.min(MAX_LOCK_FRAMES)
    }

    fn absolute_frame_chip_start(&self, chip_offset: usize) -> Option<usize> {
        let base = usize::try_from(self.absolute_chip_start?).ok()?;
        base.checked_add(chip_offset)
    }

    fn lock_timing_metrics(
        &self,
        chip_phase: usize,
        frame_phase: usize,
        lock_frame_index: usize,
    ) -> Option<(usize, usize, usize, usize, usize)> {
        let chip_offset = chip_phase
            + (frame_phase + lock_frame_index * RC1_SYMBOLS_PER_FRAME) * PN_CHIPS_PER_SYMBOL;
        let abs_chip = self.absolute_frame_chip_start(chip_offset)?;
        Some((
            abs_chip,
            abs_chip % FRAME_CHIPS,
            abs_chip % PN_CHIPS_PER_SYMBOL,
            abs_chip % PN_SHORT_CODE_CHIPS,
            (abs_chip / PN_CHIPS_PER_SYMBOL) % RC1_SYMBOLS_PER_FRAME,
        ))
    }

    /// Per C.S0002-E 2.1.3.1.14.2, derive the 14 long-code randomizer bits
    /// that determine the RC1 lower-rate active-PCG mask for this frame.
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

    fn active_pcgs_for_rate(
        &self,
        rate: Rc1TrafficRate,
        raw_soft: &[f32],
        chip_offset: usize,
    ) -> [bool; SR1_PCGS_PER_FRAME] {
        if let Some(frame_chip_start) = self.absolute_frame_chip_start(chip_offset) {
            self.exact_active_pcgs_for_rate(rate, frame_chip_start)
        } else {
            rate.active_pcgs_from_soft(raw_soft)
        }
    }

    fn apply_pcg_mask(
        &self,
        raw_soft: &[f32],
        rate: Rc1TrafficRate,
        chip_offset: usize,
    ) -> Vec<f32> {
        let mut masked = raw_soft.to_vec();
        let active_pcgs = self.active_pcgs_for_rate(rate, raw_soft, chip_offset);
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

    /// Decode a frame's soft bits at the given rate.
    ///
    /// Returns `(bits, ml_best_state)`:
    /// - `bits` come from the constrained traceback (terminal state
    ///   forced to 0), which is bit-identical to the existing decode
    ///   path and the mathematically correct decode for a terminated
    ///   block since the encoder really ends each 20 ms frame at state
    ///   0 via its 8 zero tail bits.
    /// - `ml_best_state` is the unconstrained ML best terminal state
    ///   queried from the decoder's path metrics after the forward
    ///   pass (8-bit state for K=9). For a clean frame this should be
    ///   0; for a noisy or wrong-rate decode it drifts off. The
    ///   caller can compare against 0 directly, use `.count_ones()`
    ///   as a "distance from zero termination" metric, etc.
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

    fn decode_frame_soft(
        &self,
        frame_soft: &[f32],
        rate: Rc1TrafficRate,
        chip_offset: usize,
    ) -> DecodedTrafficFrame {
        let masked_soft = self.apply_pcg_mask(frame_soft, rate, chip_offset);
        let interleaver = Rc12ReverseTrafficInterleaver::new(rate.to_interleaver_rate());
        let deinterleaved = interleaver.decode_soft(&masked_soft);
        let collapsed = Self::collapse_repetition(&deinterleaved, rate.repetition_factor());
        let (bits, ml_best_state) = Self::decode_bits(&collapsed);
        let validation = FrameValidation::for_rate(rate, &bits);

        DecodedTrafficFrame {
            rate,
            bits,
            validation,
            ml_terminal_best_state: ml_best_state,
            ml_terminal_matches_zero: ml_best_state == 0,
        }
    }

    /// Demodulate one 256-chip Walsh symbol. Returns the 6 soft bits for
    /// the downstream decoder.
    fn demodulate_symbol(chips: &[Complex32]) -> [f32; RC1_SOFT_BITS_PER_SYMBOL] {
        Self::demodulate_symbol_with_energies(chips).0
    }

    /// Demodulate one 256-chip Walsh symbol and return both the soft bits
    /// and the raw 64 Walsh-hypothesis energies. The energies are needed
    /// by the per-PCG Eb/Nt computation used for closed-loop power
    /// control (see `emit_frames` / `pcg_snr_db_from_energies`).
    fn demodulate_symbol_with_energies(
        chips: &[Complex32],
    ) -> (
        [f32; RC1_SOFT_BITS_PER_SYMBOL],
        [f32; RC1_WALSH_CHIPS_PER_SYMBOL],
    ) {
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

        let mut energies = [0.0f32; RC1_WALSH_CHIPS_PER_SYMBOL];
        for (idx, corr) in walsh_chips.iter().enumerate() {
            energies[idx] = corr.re * corr.re + corr.im * corr.im;
        }

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
        (out, energies)
    }

    /// Compute 16 per-PCG signal-to-noise ratios in dB for the 96-symbol
    /// frame starting at `chip_phase` in `self.chip_buf`. Per symbol we
    /// take `peak_row_energy / mean(63 other row energies)` as a linear
    /// SNR estimate, then average the 6 symbols in each PCG in the linear
    /// domain before converting to dB.
    ///
    /// This is the measurement the BSC uses to make per-PCG power control
    /// decisions. It is gain-independent (a ratio of row energies) and
    /// survives rate-variable transmission: on an eighth-rate frame
    /// where the mobile is gated off for 14 of 16 PCGs, the 2 active PCGs
    /// show their true Eb/Nt and the 14 gated PCGs show near-zero SNR,
    /// which the BSC can filter out (e.g. by taking the max per frame)
    /// instead of being fooled by a whole-frame mean.
    /// Compute per-PCG Eb/Nt in dB for the frame starting at `chip_phase`.
    ///
    /// For each Walsh symbol we take the FHT peak-row energy and the mean of
    /// the 63 non-peak rows. Under the standard CDMA model (orthogonal Walsh,
    /// PN/LC despread, AWGN + multi-access interference modeled as white):
    ///
    /// ```text
    /// E[|peak|^2]        = 256*Es + 256*Nt
    /// E[mean 63 others]  = 256*Nt
    /// peak/mean          = Es/Nt + 1
    /// ```
    ///
    /// so true Es/Nt = (peak/mean) - 1. For RC1 the 1/3 convolutional code and
    /// 64-ary Walsh modulation give Es = 2*Eb, so Eb/Nt = Es/Nt / 2.
    fn pcg_eb_nt_db_at_offset(&self, chip_offset: usize) -> Option<f32> {
        if self.chip_buf.len() < chip_offset + PCG_CHIPS {
            return None;
        }

        let mut linear_eb_nt = 0.0f32;
        for sym_idx in 0..RC1_SYMBOLS_PER_PCG {
            let start = chip_offset + sym_idx * PN_CHIPS_PER_SYMBOL;
            let end = start + PN_CHIPS_PER_SYMBOL;
            let (_, energies) = Self::demodulate_symbol_with_energies(&self.chip_buf[start..end]);
            let peak = energies.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let total: f32 = energies.iter().sum();
            let noise_sum = (total - peak).max(0.0);
            let denom = (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32;
            let noise_mean = (noise_sum / denom).max(1e-12);
            let es_nt = (peak / noise_mean - 1.0).max(0.0);
            linear_eb_nt += (es_nt * 0.5).max(1e-9);
        }

        let linear_mean = linear_eb_nt / RC1_SYMBOLS_PER_PCG as f32;
        Some(10.0 * linear_mean.max(1e-9).log10())
    }

    fn pcg_mobile_power_dbfs_at_offset(&self, chip_offset: usize) -> Option<f32> {
        if self.chip_buf.len() < chip_offset + PCG_CHIPS {
            return None;
        }
        let energies = (0..RC1_SYMBOLS_PER_PCG)
            .map(|symbol| {
                let start = chip_offset + symbol * PN_CHIPS_PER_SYMBOL;
                let end = start + PN_CHIPS_PER_SYMBOL;
                Self::demodulate_symbol_with_energies(&self.chip_buf[start..end]).1
            })
            .collect::<Vec<_>>();
        Some(super::rc1_walsh64_mobile_power_dbfs(energies.iter()))
    }

    fn pcg_snr_db_for_frame(&self, chip_phase: usize) -> Option<Vec<f32>> {
        if self.chip_buf.len() < chip_phase + RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL {
            return None;
        }
        let mut per_symbol_eb_nt = [0.0f32; RC1_SYMBOLS_PER_FRAME];
        for sym_idx in 0..RC1_SYMBOLS_PER_FRAME {
            let start = chip_phase + sym_idx * PN_CHIPS_PER_SYMBOL;
            let end = start + PN_CHIPS_PER_SYMBOL;
            let (_, energies) = Self::demodulate_symbol_with_energies(&self.chip_buf[start..end]);
            let peak = energies.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let total: f32 = energies.iter().sum();
            let noise_sum = (total - peak).max(0.0);
            let denom = (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32;
            let noise_mean = (noise_sum / denom).max(1e-12);
            // peak/mean = Es/Nt + 1; subtract 1 for true Es/Nt, then divide
            // by 2 for Eb/Nt (Es = 2*Eb for rate-1/3 conv + 64-ary Walsh).
            let es_nt = (peak / noise_mean - 1.0).max(0.0);
            per_symbol_eb_nt[sym_idx] = (es_nt * 0.5).max(1e-9);
        }
        let mut per_pcg_db = Vec::with_capacity(SR1_PCGS_PER_FRAME);
        for pcg in 0..SR1_PCGS_PER_FRAME {
            let start = pcg * RC1_SYMBOLS_PER_PCG;
            let end = start + RC1_SYMBOLS_PER_PCG;
            let linear_mean =
                per_symbol_eb_nt[start..end].iter().sum::<f32>() / RC1_SYMBOLS_PER_PCG as f32;
            per_pcg_db.push(10.0 * linear_mean.max(1e-9).log10());
        }
        Some(per_pcg_db)
    }

    fn emitted_active_pcg_mask(
        &self,
        rate: Rc1TrafficRate,
        pcg_snr_db: Option<&[f32]>,
    ) -> Option<[bool; SR1_PCGS_PER_FRAME]> {
        if let Some(frame_chip_start) = self.absolute_frame_chip_start(0) {
            return Some(self.exact_active_pcgs_for_rate(rate, frame_chip_start));
        }

        let metrics = pcg_snr_db?;
        if metrics.len() != SR1_PCGS_PER_FRAME {
            return None;
        }
        Some(rate.active_pcgs_from_ranked_metric(metrics))
    }

    fn emit_ready_pcg_measurements(&mut self) -> Vec<SampleBlock> {
        if self.state != FrameAlignerState::Locked {
            return Vec::new();
        }

        let Some(base_abs_chip) = self
            .absolute_chip_start
            .and_then(|chip| u64::try_from(chip).ok())
        else {
            return Vec::new();
        };
        let base_abs_pcg = base_abs_chip / PCG_CHIPS as u64;
        let mut next_abs_pcg = self.next_measurement_abs_pcg.unwrap_or(base_abs_pcg);
        if next_abs_pcg < base_abs_pcg {
            next_abs_pcg = base_abs_pcg;
        }

        let mut out = Vec::new();
        let rate = self
            .pcg_measurement_rate
            .or(self.locked_rate)
            .unwrap_or(Rc1TrafficRate::Full);
        let mut current_mask_frame: Option<(u64, [bool; SR1_PCGS_PER_FRAME])> = None;
        while next_abs_pcg >= base_abs_pcg {
            let rel_pcg = (next_abs_pcg - base_abs_pcg) as usize;
            let chip_offset = rel_pcg * PCG_CHIPS;
            let Some(pcg_end) = chip_offset.checked_add(PCG_CHIPS) else {
                break;
            };
            if pcg_end > self.chip_buf.len() {
                break;
            }

            let pcg_in_frame = (next_abs_pcg % SR1_PCGS_PER_FRAME as u64) as usize;
            let frame_start_abs_chip = next_abs_pcg
                .saturating_sub(pcg_in_frame as u64)
                .saturating_mul(PCG_CHIPS as u64);
            let active_mask = match current_mask_frame {
                Some((cached_start, mask)) if cached_start == frame_start_abs_chip => mask,
                _ => {
                    let mask = usize::try_from(frame_start_abs_chip)
                        .ok()
                        .map(|chip| self.exact_active_pcgs_for_rate(rate, chip))
                        .unwrap_or([true; SR1_PCGS_PER_FRAME]);
                    current_mask_frame = Some((frame_start_abs_chip, mask));
                    mask
                }
            };
            if !active_mask[pcg_in_frame] {
                next_abs_pcg += 1;
                continue;
            }

            let Some(eb_nt_db) = self.pcg_eb_nt_db_at_offset(chip_offset) else {
                break;
            };
            let Some(mobile_power_dbfs) = self.pcg_mobile_power_dbfs_at_offset(chip_offset) else {
                break;
            };
            let measurement_abs_chip = base_abs_chip.saturating_add(chip_offset as u64);
            let age_chips = self
                .last_processing_absolute_chip_end
                .map(|chip| chip.saturating_sub(measurement_abs_chip))
                .unwrap_or(0);

            let mut tags = self.tags.clone();
            tags.insert("traffic_pcg_measurement", 1);
            tags.insert("absolute_chip_start", measurement_abs_chip as i64);
            tags.insert(
                "traffic_measurement_age_chips",
                i64::try_from(age_chips).unwrap_or(i64::MAX),
            );
            tags.insert(
                "traffic_pcg_mobile_power_mdbfs",
                (mobile_power_dbfs * 1000.0) as i64,
            );

            let mut block = SampleBlock::new(Vec::new(), self.chip_start + chip_offset)
                .with_sample_rate_hz(self.sample_rate_hz)
                .with_tags(tags);
            block.pcg_signal_snr_db = Some(vec![eb_nt_db]);
            out.push(block);
            next_abs_pcg += 1;
        }

        self.next_measurement_abs_pcg = Some(next_abs_pcg);
        out
    }

    fn demodulate_symbol_stream(&self, chip_phase: usize) -> Vec<[f32; RC1_SOFT_BITS_PER_SYMBOL]> {
        let total_symbols = self.chip_buf.len().saturating_sub(chip_phase) / PN_CHIPS_PER_SYMBOL;
        let mut out = Vec::with_capacity(total_symbols);
        for symbol_idx in 0..total_symbols {
            let start = chip_phase + symbol_idx * PN_CHIPS_PER_SYMBOL;
            let end = start + PN_CHIPS_PER_SYMBOL;
            out.push(Self::demodulate_symbol(&self.chip_buf[start..end]));
        }
        out
    }

    fn frame_soft_from_symbols(
        symbols: &[[f32; RC1_SOFT_BITS_PER_SYMBOL]],
        symbol_start: usize,
    ) -> Option<Vec<f32>> {
        if symbol_start + RC1_SYMBOLS_PER_FRAME > symbols.len() {
            return None;
        }

        let mut out = Vec::with_capacity(SOFT_BITS_PER_FRAME);
        for symbol in &symbols[symbol_start..symbol_start + RC1_SYMBOLS_PER_FRAME] {
            out.extend_from_slice(symbol);
        }
        Some(out)
    }

    /// Predict chip_phase from absolute_chip_start.  Walsh symbol boundaries
    /// fall every PN_CHIPS_PER_SYMBOL (256) chips at system_time % 256 == 0.
    /// Returns None if no absolute_chip_start.
    fn predicted_chip_phase(&self) -> Option<usize> {
        let abs_start = self.absolute_chip_start? as u64;
        let remainder = (abs_start % PN_CHIPS_PER_SYMBOL as u64) as usize;
        Some((PN_CHIPS_PER_SYMBOL - remainder) % PN_CHIPS_PER_SYMBOL)
    }

    /// Generate chip_phase candidates. If we have absolute chip timing, RC1
    /// traffic must land on that exact symbol boundary, so only test the
    /// predicted phase. Otherwise fall back to the preamble / energy search.
    fn chip_phase_candidates(&self) -> Vec<usize> {
        const SEARCH_RADIUS: usize = 4;
        if let Some(predicted) = self.predicted_chip_phase() {
            vec![predicted]
        } else if matches!(self.state, FrameAlignerState::Locking)
            && let Some((preamble_chip_phase, _, _)) = self.preamble_best
        {
            let mut candidates = Vec::with_capacity(SEARCH_RADIUS * 2 + 1);
            for delta in 0..=SEARCH_RADIUS {
                candidates.push((preamble_chip_phase + delta) % PN_CHIPS_PER_SYMBOL);
                if delta > 0 {
                    candidates.push(
                        (preamble_chip_phase + PN_CHIPS_PER_SYMBOL - delta) % PN_CHIPS_PER_SYMBOL,
                    );
                }
            }
            candidates
        } else {
            self.rank_chip_phases_by_energy(8)
        }
    }

    /// Compute predicted frame_phase from system time for a given chip_phase.
    /// Returns None if absolute_chip_start is not available.
    fn predicted_frame_phase(&self, chip_phase: usize) -> Option<usize> {
        let abs_start = self.absolute_chip_start? as u64;
        // Frame boundaries occur at system_time % FRAME_CHIPS == 0.
        // The buffer starts at abs_start. With chip_phase offset, the first
        // valid symbol starts at chip_phase chips into the buffer.
        // We need: (abs_start + chip_phase + frame_phase * PN_CHIPS_PER_SYMBOL) % FRAME_CHIPS == 0
        let offset_in_frame = ((abs_start + chip_phase as u64) % FRAME_CHIPS as u64) as usize;
        let chips_to_boundary = (FRAME_CHIPS - offset_in_frame) % FRAME_CHIPS;
        // Round to nearest symbol boundary
        let frame_phase = (chips_to_boundary + PN_CHIPS_PER_SYMBOL / 2) / PN_CHIPS_PER_SYMBOL;
        Some(frame_phase % RC1_SYMBOLS_PER_FRAME)
    }

    /// Generate frame phases to search. If absolute chip timing is available,
    /// RC1 traffic must land on the exact 20 ms boundary implied by that
    /// system time, so only test the predicted phase.
    fn frame_phase_candidates(&self, chip_phase: usize) -> Vec<usize> {
        if let Some(predicted) = self.predicted_frame_phase(chip_phase) {
            vec![predicted]
        } else if matches!(self.state, FrameAlignerState::Locking)
            && let Some((_, preamble_frame_phase, _)) = self.preamble_best
        {
            // Without absolute timing, keep the preamble-based search classes
            // to avoid sweeping all 96 symbol positions.
            vec![
                (preamble_frame_phase + 64) % RC1_SYMBOLS_PER_FRAME,
                preamble_frame_phase % RC1_SYMBOLS_PER_FRAME,
                (preamble_frame_phase + 32) % RC1_SYMBOLS_PER_FRAME,
            ]
        } else {
            (0..RC1_SYMBOLS_PER_FRAME).collect()
        }
    }

    fn try_lock_with_symbols(
        &self,
        symbols: &[[f32; RC1_SOFT_BITS_PER_SYMBOL]],
        chip_phase: usize,
        search_frames: usize,
        best_summary: &mut Option<SearchSummary>,
    ) -> Option<LockCandidate> {
        // Lock requires Full-rate CRC-12 valid frames for SAR reassembly.
        // Only try Full rate during search — skip Half/Quarter/Eighth entirely.

        let frame_phases = self.frame_phase_candidates(chip_phase);
        for &frame_phase in &frame_phases {
            let mut valid_full_frames = 0usize;
            let mut prefix_reader = DedicatedFrameReader::new();
            let mut suffix_reader = DedicatedFrameReader::new();

            for frame_idx in 0..search_frames {
                let symbol_start = frame_phase + frame_idx * RC1_SYMBOLS_PER_FRAME;
                let Some(frame_soft) = Self::frame_soft_from_symbols(symbols, symbol_start) else {
                    break;
                };

                let chip_offset = chip_phase
                    + (frame_phase + frame_idx * RC1_SYMBOLS_PER_FRAME) * PN_CHIPS_PER_SYMBOL;
                let decoded =
                    self.decode_frame_soft(&frame_soft, Rc1TrafficRate::Full, chip_offset);
                if !decoded.validation.phy_valid {
                    continue;
                }
                valid_full_frames += 1;

                if decoded.bits.len() < Rc1TrafficRate::Full.info_bits() {
                    continue;
                }

                for layout in ReverseMux1SignalingLayout::SEARCH_ORDER {
                    let Some(signaling_block) = extract_reverse_mux1_full_rate_signaling_block(
                        &decoded.bits[..Rc1TrafficRate::Full.info_bits()],
                        layout,
                    ) else {
                        continue;
                    };

                    let reader = match layout {
                        ReverseMux1SignalingLayout::Prefix => &mut prefix_reader,
                        ReverseMux1SignalingLayout::Suffix => &mut suffix_reader,
                    };
                    let mut fragment = Bitstream::new_init(&signaling_block.bits);
                    if let Ok(Some(frame)) = reader.process(&mut fragment)
                        && frame.crc_valid
                    {
                        let chip_offset = chip_phase + frame_phase * PN_CHIPS_PER_SYMBOL;
                        let lock_timing =
                            self.lock_timing_metrics(chip_phase, frame_phase, frame_idx);
                        info!(
                            "rc1_traffic_frame_aligner: CRC lock esn=0x{:08X} chip_phase={} frame_phase={} chip_offset={} rate={} layout={:?} lock_frame_idx={} lock_abs_chip={:?} lock_chip_mod_frame={:?} lock_chip_mod_symbol={:?} lock_chip_mod_pn={:?} lock_symbol_mod_frame={:?} msg_len={} payload_bits={}",
                            self.esn,
                            chip_phase,
                            frame_phase,
                            chip_offset,
                            Rc1TrafficRate::Full.rate_bps(),
                            layout,
                            frame_idx,
                            lock_timing.map(|m| m.0),
                            lock_timing.map(|m| m.1),
                            lock_timing.map(|m| m.2),
                            lock_timing.map(|m| m.3),
                            lock_timing.map(|m| m.4),
                            frame.msg_length_octets,
                            frame.data.len(),
                        );
                        return Some(LockCandidate {
                            chip_phase,
                            frame_phase,
                            rate: Rc1TrafficRate::Full,
                            layout,
                            lock_frame_index: frame_idx,
                        });
                    }
                }
            }

            let summary = SearchSummary {
                best_chip_phase: chip_phase,
                best_frame_phase: frame_phase,
                best_rate: Rc1TrafficRate::Full,
                best_valid_full_frames: valid_full_frames,
                best_valid_any_frames: valid_full_frames,
            };
            let replace = best_summary
                .map(|best| summary.best_valid_full_frames > best.best_valid_full_frames)
                .unwrap_or(true);
            if replace {
                *best_summary = Some(summary);
            }
        }

        None
    }

    /// For a given chip_phase, demodulate a small number of symbols and return
    /// the average peak Walsh bin energy. High energy means the chip_phase is
    /// likely aligned to a real signal.
    fn chip_phase_energy(&self, chip_phase: usize, num_probe_symbols: usize) -> f32 {
        let available_symbols =
            self.chip_buf.len().saturating_sub(chip_phase) / PN_CHIPS_PER_SYMBOL;
        let n = available_symbols.min(num_probe_symbols);
        if n == 0 {
            return 0.0;
        }

        let mut total_peak = 0.0f32;
        for sym_idx in 0..n {
            let start = chip_phase + sym_idx * PN_CHIPS_PER_SYMBOL;

            // Accumulate PN chips into Walsh chips
            let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
            for wc in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
                let base = start + wc * RC1_PN_CHIPS_PER_WALSH_CHIP;
                walsh_chips[wc] = self.chip_buf[base..base + RC1_PN_CHIPS_PER_WALSH_CHIP]
                    .iter()
                    .copied()
                    .sum::<Complex32>();
            }
            WalshGenerator::fwht_fixed(&mut walsh_chips);

            let peak = walsh_chips
                .iter()
                .map(|c| c.re * c.re + c.im * c.im)
                .fold(0.0f32, f32::max);
            total_peak += peak;
        }
        total_peak / n as f32
    }

    /// Rank all 256 chip_phases by Walsh energy and return the top N candidates.
    fn rank_chip_phases_by_energy(&self, top_n: usize) -> Vec<usize> {
        // Probe ~16 symbols spread across the buffer for a representative sample
        let probe_symbols = 16;

        let mut scored: Vec<(usize, f32)> = (0..PN_CHIPS_PER_SYMBOL)
            .map(|cp| (cp, self.chip_phase_energy(cp, probe_symbols)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));

        scored.into_iter().take(top_n).map(|(cp, _)| cp).collect()
    }

    fn acquire_lock(&self) -> (Option<LockCandidate>, Option<SearchSummary>) {
        let search_frames = self.search_frame_count();
        if search_frames < MIN_LOCK_FRAMES {
            return (None, None);
        }

        // With absolute chip timing, RC1 should be tested only on the exact
        // symbol/frame boundary predicted from system time.
        let candidates = self.chip_phase_candidates();

        let mut best_summary = None;
        for &chip_phase in &candidates {
            let symbols = self.demodulate_symbol_stream(chip_phase);
            if let Some(candidate) =
                self.try_lock_with_symbols(&symbols, chip_phase, search_frames, &mut best_summary)
            {
                return (Some(candidate), best_summary);
            }
        }

        // If absolute timing is unavailable, fall back to energy ranking
        // with a small candidate set. Only do this when we have enough data
        // (avoids wasting time on early small buffers).
        if self.predicted_chip_phase().is_none() && search_frames >= 8 {
            const FALLBACK_TOP: usize = 4;
            let energy_candidates = self.rank_chip_phases_by_energy(FALLBACK_TOP);
            for &chip_phase in &energy_candidates {
                // Skip phases already tried via prediction
                if candidates.contains(&chip_phase) {
                    continue;
                }
                let symbols = self.demodulate_symbol_stream(chip_phase);
                if let Some(candidate) = self.try_lock_with_symbols(
                    &symbols,
                    chip_phase,
                    search_frames,
                    &mut best_summary,
                ) {
                    return (Some(candidate), best_summary);
                }
            }
        }

        (None, best_summary)
    }

    /// Demodulate symbols at chip_offset once, then return soft bits for the frame.
    fn demodulate_frame_at_offset(&self, chip_offset: usize) -> Option<Vec<f32>> {
        if self.chip_buf.len() < chip_offset + FRAME_CHIPS {
            return None;
        }

        let mut frame_soft = Vec::with_capacity(SOFT_BITS_PER_FRAME);
        for symbol_idx in 0..RC1_SYMBOLS_PER_FRAME {
            let start = chip_offset + symbol_idx * PN_CHIPS_PER_SYMBOL;
            let end = start + PN_CHIPS_PER_SYMBOL;
            frame_soft.extend_from_slice(&Self::demodulate_symbol(&self.chip_buf[start..end]));
        }
        Some(frame_soft)
    }

    fn choose_best_frame_at_offset(&self, chip_offset: usize) -> Option<DecodedTrafficFrame> {
        let frame_soft = self.demodulate_frame_at_offset(chip_offset)?;

        // (A) Fast path: try the adaptive most-likely rate first (from recent
        // history). If it has a real CRC and validates, return immediately.
        let adaptive_order = self.adaptive_search_order();
        let fast_rate = adaptive_order[0];
        let fast_tried = if fast_rate.fqi_bits() > 0 {
            let decoded = self.decode_frame_soft(&frame_soft, fast_rate, chip_offset);
            if decoded.validation.phy_valid {
                return Some(decoded);
            }
            Some(fast_rate)
        } else {
            None
        };

        // Also try locked_rate if it differs from the adaptive pick.
        let locked_tried = if let Some(locked_rate) = self.locked_rate {
            if Some(locked_rate) != fast_tried && locked_rate.fqi_bits() > 0 {
                let decoded = self.decode_frame_soft(&frame_soft, locked_rate, chip_offset);
                if decoded.validation.phy_valid {
                    return Some(decoded);
                }
                Some(locked_rate)
            } else {
                fast_tried.filter(|_| Some(fast_rate) == self.locked_rate)
            }
        } else {
            None
        };

        // (C) Fallback: try remaining rates in adaptive order, skipping any
        // already tried in the fast path above.
        let mut best: Option<(usize, DecodedTrafficFrame)> = None;
        for rate in adaptive_order {
            // (A) Skip rates already attempted above.
            if Some(rate) == fast_tried || Some(rate) == locked_tried {
                continue;
            }

            let decoded = self.decode_frame_soft(&frame_soft, rate, chip_offset);
            if !decoded.validation.phy_valid {
                continue;
            }

            // (B) CRC-valid is definitive — no ambiguity, early-exit.
            // Only for rates that *actually carry a CRC*. For Quarter
            // and Eighth rate, `FrameValidation::fqi_valid` is a
            // hardcoded `true` placeholder because the spec defines
            // those rates as having no Frame Quality Indicator at all
            // (C.S0002-E §2.1.3.12.1, Table 2.1.3.12.1-1). Treating
            // that placeholder as "a CRC passed" would cause the
            // first Q/E rate in `adaptive_order` to trivially
            // early-return on every frame, which is what the aligner
            // was doing historically — it's why the scoring block
            // below was dead code. Gating on `fqi_bits() > 0` lets
            // Q/E rates fall through to the scoring block where the
            // Viterbi ML best-terminal-state check can actually pick
            // between them on quality instead of on adaptive-order
            // position.
            if rate.fqi_bits() > 0 && decoded.validation.fqi_valid {
                return Some(decoded);
            }

            // Score the remaining candidates. Ordering is by relative
            // weight:
            //   +1000  locked_rate inertia (stick with the currently
            //          locked rate unless something much better shows
            //          up; prevents rate-pick oscillation)
            //   +500   ml_clean — the unconstrained Viterbi ML best
            //          terminal state equals 0, meaning the decoder
            //          agrees on its own that the encoder really did
            //          drive back to state 0. This is the REAL in-band
            //          validity check for rates without a CRC and is
            //          what distinguishes a legitimate Eighth-rate
            //          null-traffic frame from a noise decode that
            //          picked Quarter by accident.
            //   +60    is_full_preamble (preserve existing behavior)
            //   +50    has_nonzero (prefer content over trivially zero)
            //   +10..40 rate tiebreak (Full=40, Half=30, Quarter=20,
            //          Eighth=10) — only matters when nothing else
            //          discriminates.
            //
            // Historically this block also scored `tail_valid` as
            // +100. That check was tautologically `true` for every
            // frame that got past `!phy_valid { continue; }` above
            // (because `decode_block_from_state(..., 0)` constrains
            // the traceback to terminate at state 0, so the decoded
            // last-8-bits are always zero), so it added a constant
            // to every candidate and affected no ordering. It's now
            // fully subsumed by the `ml_terminal_matches_zero` term.
            let is_full_preamble =
                rate == Rc1TrafficRate::Full && decoded.bits.iter().all(|bit| *bit == 0);
            let has_nonzero = decoded.bits.iter().any(|bit| *bit != 0);
            let mut score = 0usize;
            if Some(rate) == self.locked_rate {
                score += 1000;
            }
            if decoded.ml_terminal_matches_zero {
                score += 500;
            }
            if has_nonzero {
                score += 50;
            }
            if is_full_preamble {
                score += 60;
            }
            score += match rate {
                Rc1TrafficRate::Full => 40,
                Rc1TrafficRate::Half => 30,
                Rc1TrafficRate::Quarter => 20,
                Rc1TrafficRate::Eighth => 10,
            };

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

    /// Diagnostic: on a sub-rate (non-Full) emission, log the decoded
    /// frame plus a side-by-side comparison decode at *all four* rates
    /// for the same raw soft symbols.
    ///
    /// The spec (C.S0002-E §2.1.3.12.1.2) guarantees every RC1 reverse
    /// traffic frame — including 2400 and 1200 bps frames that carry
    /// no CRC — ends with 8 zero Encoder Tail Bits by construction.
    /// Note that *the literal last 8 decoded bits are always zero*
    /// because `decode_block_from_state(..., 0)` forces the Viterbi
    /// traceback to terminate at state 0. So `tail == 0` is a
    /// tautology and is NOT a quality signal. The real signal is
    /// `ml_best_state`: the *unconstrained* ML best terminal state,
    /// which for a clean real frame converges to 0 on its own.
    ///
    /// Logging every rate's decoded info bits + ml_best_state at the
    /// same input tells us which rate (if any) is actually "real" for
    /// this frame and which are noise hallucinations the scoring
    /// picked by accident. A cleanly-converging `ml_best_state=0x00
    /// pop=0` at one rate and a scattered state at others is the
    /// signature of that rate being the genuinely-transmitted one.
    fn log_subrate_trial_decode(&self, emitted: &DecodedTrafficFrame) {
        // Dump the picked frame's own info first.
        let picked_info_bits = emitted.rate.info_bits();
        if emitted.bits.len() >= picked_info_bits {
            let picked_info = &emitted.bits[..picked_info_bits];
            let info_nonzero = picked_info.iter().filter(|b| **b != 0).count();
            debug!(
                "rc1_traffic_frame_aligner: sub_rate_frame picked rate={} chip_start={} \
                 ml_best_state=0x{:02x}(pop={}) info_nonzero={}/{} info_hex={}",
                emitted.rate.rate_bps(),
                self.chip_start,
                emitted.ml_terminal_best_state,
                emitted.ml_terminal_best_state.count_ones(),
                info_nonzero,
                picked_info.len(),
                Self::format_bits_hex(picked_info),
            );
        }

        // Re-demodulate once at offset 0 (same offset `emit_frames`
        // used for the picked decode) and trial-decode at every rate.
        let Some(frame_soft) = self.demodulate_frame_at_offset(0) else {
            return;
        };
        for rate in [
            Rc1TrafficRate::Full,
            Rc1TrafficRate::Half,
            Rc1TrafficRate::Quarter,
            Rc1TrafficRate::Eighth,
        ] {
            let masked = self.apply_pcg_mask(&frame_soft, rate, 0);
            let interleaver = Rc12ReverseTrafficInterleaver::new(rate.to_interleaver_rate());
            let deinterleaved = interleaver.decode_soft(&masked);
            let collapsed = Self::collapse_repetition(&deinterleaved, rate.repetition_factor());
            let (bits, ml_state) = Self::decode_bits(&collapsed);

            let info_bits = rate.info_bits();
            if bits.len() < info_bits {
                continue;
            }
            let info = &bits[..info_bits];
            let info_nonzero = info.iter().filter(|b| **b != 0).count();
            let ml_clean = ml_state == 0;
            debug!(
                "rc1_traffic_frame_aligner:   trial rate={} ml_best_state=0x{:02x}(pop={}) \
                 ml_clean={} info_nonzero={}/{} info_hex={}",
                rate.rate_bps(),
                ml_state,
                ml_state.count_ones(),
                ml_clean,
                info_nonzero,
                info.len(),
                Self::format_bits_hex(info),
            );
        }
    }

    fn refresh_absolute_chip_tags(&mut self) {
        if let Some(absolute_chip_start) = self.tags.get("absolute_chip_start").copied() {
            self.absolute_chip_start = Some(absolute_chip_start);
        }
    }

    fn drain_front_chips(&mut self, n_chips: usize) {
        let n = n_chips.min(self.chip_buf.len());
        self.chip_buf.drain(..n);
        self.chip_start = self.chip_start.saturating_add(n);
        if let Some(absolute_chip_start) = &mut self.absolute_chip_start {
            *absolute_chip_start = absolute_chip_start.saturating_add(n as i64);
        }
        self.last_search_frame_budget = 0;
    }

    /// Phase-1 search: try every chip_phase/frame_phase hypothesis and count
    /// consecutive null frames (sub-rate with valid tail bits).  Once we see
    /// PREAMBLE_NULL_FRAME_THRESHOLD consecutive null frames at any single
    /// hypothesis, emit a preamble event and transition to Locking.
    fn preamble_search_step(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        let search_frames = self.search_frame_count();
        if search_frames < MIN_LOCK_FRAMES {
            return out;
        }

        let search_bucket = (search_frames / 8) * 8;
        if search_bucket == self.last_search_frame_budget
            && self.chip_buf.len() <= MAX_SEARCH_BUFFER_CHIPS
        {
            return out;
        }
        self.last_search_frame_budget = search_bucket;
        self.search_attempts = self.search_attempts.saturating_add(1);

        let candidates = self.chip_phase_candidates();

        let mut global_best: Option<(usize, usize, usize)> = self.preamble_best;

        for &chip_phase in &candidates {
            let frame_phases = self.frame_phase_candidates(chip_phase);
            for &frame_phase in &frame_phases {
                let mut consecutive_null = 0usize;
                let mut best_consecutive = 0usize;

                for frame_idx in 0..search_frames {
                    let chip_offset = chip_phase
                        + (frame_phase + frame_idx * RC1_SYMBOLS_PER_FRAME) * PN_CHIPS_PER_SYMBOL;
                    // Not enough data for this frame
                    if chip_offset + FRAME_CHIPS > self.chip_buf.len() {
                        break;
                    }

                    let Some(frame_soft) = self.demodulate_frame_at_offset(chip_offset) else {
                        break;
                    };

                    // Try sub-rates (eighth and quarter are most likely for preamble)
                    let mut is_null = false;
                    for &rate in &[
                        Rc1TrafficRate::Eighth,
                        Rc1TrafficRate::Quarter,
                        Rc1TrafficRate::Half,
                    ] {
                        let decoded = self.decode_frame_soft(&frame_soft, rate, chip_offset);
                        if decoded.validation.tail_valid {
                            is_null = true;
                            break;
                        }
                    }
                    // Also accept full-rate all-zeros
                    if !is_null {
                        let decoded =
                            self.decode_frame_soft(&frame_soft, Rc1TrafficRate::Full, chip_offset);
                        if decoded.validation.tail_valid && decoded.bits.iter().all(|bit| *bit == 0)
                        {
                            is_null = true;
                        }
                    }

                    if is_null {
                        consecutive_null += 1;
                        best_consecutive = best_consecutive.max(consecutive_null);
                    } else {
                        consecutive_null = 0;
                    }
                }

                let replace = global_best
                    .map(|(_, _, prev)| best_consecutive > prev)
                    .unwrap_or(true);
                if replace && best_consecutive > 0 {
                    global_best = Some((chip_phase, frame_phase, best_consecutive));
                }

                // Early exit if we've already hit the threshold
                if best_consecutive >= PREAMBLE_NULL_FRAME_THRESHOLD {
                    break;
                }
            }
            if global_best
                .map(|(_, _, n)| n >= PREAMBLE_NULL_FRAME_THRESHOLD)
                .unwrap_or(false)
            {
                break;
            }
        }

        self.preamble_best = global_best;

        if let Some((chip_phase, frame_phase, null_count)) = global_best
            && null_count >= PREAMBLE_NULL_FRAME_THRESHOLD
        {
            info!(
                "rc1_traffic_frame_aligner: preamble detected in search phase after {} null frames \
                 esn=0x{:08X} chip_phase={} frame_phase={} chip_start={} absolute_chip_start={:?}",
                null_count,
                self.esn,
                chip_phase,
                frame_phase,
                self.chip_start,
                self.absolute_chip_start,
            );
            out.push(self.emit_preamble_event(null_count));
            self.preamble_event_sent = true;

            // Transition to Locking — keep the buffer for CRC-based lock search.
            // Reset search counters so Locking gets a fresh budget.
            self.state = FrameAlignerState::Locking;
            self.next_measurement_abs_pcg = None;
            self.search_attempts = 0;
            self.last_search_frame_budget = 0;
            return out;
        }

        if let Some((cp, fp, n)) = global_best
            && (self.search_attempts <= 3 || self.search_attempts % 4 == 0)
        {
            info!(
                "rc1_traffic_frame_aligner: preamble search attempt={} best_null_count={} \
                 chip_phase={} frame_phase={} search_frames={} chip_start={} absolute_chip_start={:?}",
                self.search_attempts,
                n,
                cp,
                fp,
                search_frames,
                self.chip_start,
                self.absolute_chip_start,
            );
        }

        if self.search_attempts >= MAX_SEARCH_ATTEMPTS {
            info!(
                "rc1_traffic_frame_aligner: preamble search giving up after {} attempts esn=0x{:08X}",
                self.search_attempts, self.esn,
            );
            self.state = FrameAlignerState::GaveUp;
            self.next_measurement_abs_pcg = None;
            self.chip_buf.clear();
            let mut block = SampleBlock::new(Vec::new(), self.chip_start)
                .with_sample_rate_hz(self.sample_rate_hz);
            block.tags.insert("traffic_search_gave_up", 1);
            out.push(block);
            return out;
        }

        if self.chip_buf.len() > MAX_SEARCH_BUFFER_CHIPS {
            self.drain_front_chips(FRAME_CHIPS);
        }
        out
    }

    fn search_step(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();

        let search_frames = self.search_frame_count();
        if search_frames < MIN_LOCK_FRAMES {
            return out;
        }
        // Bucket searches at 8-frame intervals to avoid re-searching at
        // 2,3,4,5,6,7 frames when lock typically needs more data.
        let search_bucket = (search_frames / 8) * 8;
        if search_bucket == self.last_search_frame_budget
            && self.chip_buf.len() <= MAX_SEARCH_BUFFER_CHIPS
        {
            return out;
        }
        self.last_search_frame_budget = search_bucket;
        self.search_attempts = self.search_attempts.saturating_add(1);

        let (lock, best_summary) = self.acquire_lock();
        if let Some(lock) = lock {
            let chip_offset = lock.chip_phase + lock.frame_phase * PN_CHIPS_PER_SYMBOL;
            let lock_timing =
                self.lock_timing_metrics(lock.chip_phase, lock.frame_phase, lock.lock_frame_index);
            self.drain_front_chips(chip_offset);
            self.state = FrameAlignerState::Locked;
            self.next_measurement_abs_pcg = self
                .absolute_chip_start
                .and_then(|chip| u64::try_from(chip).ok())
                .map(|chip| chip / PCG_CHIPS as u64);
            self.locked_rate = Some(lock.rate);
            self.pcg_measurement_rate = Some(lock.rate);
            self.locked_mux_layout = Some(lock.layout);
            self.last_search_frame_budget = 0;
            self.search_attempts = 0;
            self.tags.insert("traffic_frame_aligned", 1);
            self.tags.insert("traffic_walsh_locked", 1);
            self.tags
                .insert("traffic_lock_chip_phase", lock.chip_phase as i64);
            self.tags
                .insert("traffic_lock_frame_phase", lock.frame_phase as i64);
            self.tags
                .insert("traffic_lock_frame_idx", lock.lock_frame_index as i64);
            if let Some((abs_chip, mod_frame, mod_symbol, mod_pn, sym_in_frame)) = lock_timing {
                self.tags.insert("traffic_lock_abs_chip", abs_chip as i64);
                self.tags
                    .insert("traffic_lock_chip_mod_frame", mod_frame as i64);
                self.tags
                    .insert("traffic_lock_chip_mod_symbol", mod_symbol as i64);
                self.tags.insert("traffic_lock_chip_mod_pn", mod_pn as i64);
                self.tags
                    .insert("traffic_lock_symbol_mod_frame", sym_in_frame as i64);
            }
            info!(
                "rc1_traffic_frame_aligner: locked esn=0x{:08X} chip_start={} absolute_chip_start={:?} rate={} layout={:?} chip_phase={} frame_phase={} lock_frame_idx={} lock_abs_chip={:?} lock_chip_mod_frame={:?} lock_chip_mod_symbol={:?} lock_chip_mod_pn={:?} lock_symbol_mod_frame={:?}",
                self.esn,
                self.chip_start,
                self.absolute_chip_start,
                lock.rate.rate_bps(),
                lock.layout,
                lock.chip_phase,
                lock.frame_phase,
                lock.lock_frame_index,
                lock_timing.map(|m| m.0),
                lock_timing.map(|m| m.1),
                lock_timing.map(|m| m.2),
                lock_timing.map(|m| m.3),
                lock_timing.map(|m| m.4),
            );
            out.extend(self.emit_frames());
            return out;
        }

        if let Some(best) = best_summary
            && (self.search_attempts <= 3 || self.search_attempts % 4 == 0)
        {
            info!(
                "rc1_traffic_frame_aligner: search attempt={} no lock yet chip_start={} absolute_chip_start={:?} buffered_chips={} search_frames={} best chip_phase={} frame_phase={} rate={} valid_full_frames={} valid_any_frames={}",
                self.search_attempts,
                self.chip_start,
                self.absolute_chip_start,
                self.chip_buf.len(),
                search_bucket,
                best.best_chip_phase,
                best.best_frame_phase,
                best.best_rate.rate_bps(),
                best.best_valid_full_frames,
                best.best_valid_any_frames,
            );
        }

        if self.search_attempts >= MAX_SEARCH_ATTEMPTS {
            info!(
                "rc1_traffic_frame_aligner: giving up after {} search attempts esn=0x{:08X} chip_start={} absolute_chip_start={:?}",
                self.search_attempts, self.esn, self.chip_start, self.absolute_chip_start,
            );
            self.state = FrameAlignerState::GaveUp;
            self.next_measurement_abs_pcg = None;
            self.chip_buf.clear();
            let mut block = SampleBlock::new(Vec::new(), self.chip_start)
                .with_sample_rate_hz(self.sample_rate_hz);
            block.tags.insert("traffic_search_gave_up", 1);
            out.push(block);
            return out;
        }

        if self.chip_buf.len() > MAX_SEARCH_BUFFER_CHIPS {
            debug!(
                "rc1_traffic_frame_aligner: sliding search window by one frame buffered_chips={} chip_start={}",
                self.chip_buf.len(),
                self.chip_start,
            );
            self.drain_front_chips(FRAME_CHIPS);
        }
        out
    }

    fn emit_frames(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.chip_buf.len() >= FRAME_CHIPS {
            let Some(decoded) = self.choose_best_frame_at_offset(0) else {
                info!(
                    "rc1_traffic_frame_aligner: lost lock at chip_start={} absolute_chip_start={:?}, returning to search",
                    self.chip_start, self.absolute_chip_start,
                );
                self.state = FrameAlignerState::Locking;
                self.next_measurement_abs_pcg = None;
                self.search_attempts = 0;
                self.last_search_frame_budget = 0;
                self.locked_rate = None;
                self.pcg_measurement_rate = None;
                self.locked_mux_layout = None;
                if !self.chip_buf.is_empty() {
                    self.drain_front_chips(1);
                }
                return out;
            };

            // (C) Update rate history for adaptive fast-path ordering.
            self.record_rate(decoded.rate);
            if decoded.validation.phy_valid {
                self.pcg_measurement_rate = Some(decoded.rate);
            }

            let is_preamble =
                decoded.rate == Rc1TrafficRate::Full && decoded.bits.iter().all(|bit| *bit == 0);

            debug!(
                "rc1_traffic_frame_aligner: emit frame rate={} chip_start={} absolute_chip_start={:?} fqi_valid={} tail_valid={} ml_tail_match={} preamble={}",
                decoded.rate.rate_bps(),
                self.chip_start,
                self.absolute_chip_start,
                decoded.validation.fqi_valid,
                decoded.validation.tail_valid,
                decoded.ml_terminal_matches_zero,
                is_preamble,
            );

            // Diagnostic: on sub-rate (non-Full) emissions, log what the
            // decoded frame actually looks like — including the tail
            // bits, which by construction are the 8 zero bits the
            // mobile encoder appends regardless of payload content.
            //
            // We also re-run the decode at *all four* rates on the
            // same raw soft symbols so we can see what each rate's
            // Viterbi output produces, whether its tail converges to
            // all-zero, and where its ML best terminal state lands.
            // The aligner picks one rate in `choose_best_frame_at_offset`
            // via scoring; this log shows the alternatives it rejected
            // so we can tell if it picked the "right" rate.
            if decoded.rate != Rc1TrafficRate::Full {
                self.log_subrate_trial_decode(&decoded);
            }

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
            // True iff the Viterbi forward pass's unconstrained ML best
            // terminal state equals 0. A clean frame naturally converges
            // there; on noisy or wrong-rate decodes the ML state drifts
            // off and this flag goes false. For Quarter/Eighth rates —
            // which have no FQI/CRC — this is the only real integrity
            // signal the RX can emit.
            tags.insert(
                "traffic_ml_tail_match",
                decoded.ml_terminal_matches_zero as i64,
            );
            tags.insert("traffic_is_preamble", is_preamble as i64);
            if let Some(layout) = self.locked_mux_layout {
                tags.insert("traffic_mux_signaling_layout", layout.tag_value());
            }
            if let Some(absolute_chip_start) = self.absolute_chip_start {
                tags.insert("absolute_chip_start", absolute_chip_start);
            }
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
            // Compute per-PCG Eb/Nt for closed-loop power control BEFORE
            // we drain the 20 ms frame of chips from the buffer.
            let pcg_snr_db = self.pcg_snr_db_for_frame(0);
            let active_pcg_mask = self.emitted_active_pcg_mask(decoded.rate, pcg_snr_db.as_deref());
            let mut block =
                SampleBlock::new(samples, self.chip_start).with_sample_rate_hz(self.sample_rate_hz);
            block.tags = tags;
            block.pcg_signal_snr_db = pcg_snr_db;
            block.active_pcg_mask = active_pcg_mask;
            out.push(block);

            self.drain_front_chips(FRAME_CHIPS);
        }
        out
    }
}

impl PipelineProcessor for Rc1TrafficFrameAligner {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let block_absolute_chip_start = block
            .tags
            .get("absolute_chip_start")
            .copied()
            .and_then(|value| u64::try_from(value).ok());

        if self.chip_buf.is_empty() {
            self.chip_start = block.chip_start;
            self.sample_rate_hz = block.sample_rate_hz;
            self.tags = block.tags.clone();
            self.refresh_absolute_chip_tags();
            self.last_search_frame_budget = 0;
            self.search_attempts = 0;
        } else {
            self.tags = block.tags.clone();
            self.sample_rate_hz = block.sample_rate_hz;
            // Do NOT call refresh_absolute_chip_tags() here —
            // absolute_chip_start must track the start of chip_buf, not the
            // latest block. It is set once when the buffer is first populated
            // and adjusted by drain_front_chips() when data is consumed.
        }
        if let Some(absolute_chip_start) = block_absolute_chip_start {
            self.last_processing_absolute_chip_end =
                Some(absolute_chip_start.saturating_add(block.samples.len() as u64));
        }

        self.chip_buf.extend_from_slice(&block.samples);

        match self.state {
            FrameAlignerState::SearchingPreamble => self.preamble_search_step(),
            FrameAlignerState::Locking => self.search_step(),
            FrameAlignerState::Locked => {
                let mut out = self.emit_ready_pcg_measurements();
                out.extend(self.emit_frames());
                out
            }
            FrameAlignerState::GaveUp => Vec::new(),
        }
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        match self.state {
            FrameAlignerState::GaveUp => Vec::new(),
            FrameAlignerState::SearchingPreamble => {
                self.last_search_frame_budget = 0;
                let out = self.preamble_search_step();
                if !out.is_empty() {
                    return out;
                }
                self.chip_buf.clear();
                Vec::new()
            }
            FrameAlignerState::Locking => {
                self.last_search_frame_budget = 0;
                let out = self.search_step();
                if !out.is_empty() {
                    return out;
                }
                self.chip_buf.clear();
                Vec::new()
            }
            FrameAlignerState::Locked => {
                let mut out = self.emit_ready_pcg_measurements();
                out.extend(self.emit_frames());
                out
            }
        }
    }

    fn name(&self) -> &'static str {
        "Rc1TrafficFrameAligner"
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;
    use cdma_common::crc::crc12;
    use num_complex::Complex32;

    use super::{
        FRAME_CHIPS, FrameAlignerState, PCG_CHIPS, RC1_SYMBOLS_PER_FRAME, Rc1TrafficFrameAligner,
        Rc1TrafficRate, ReverseMux1SignalingLayout, SOFT_BITS_PER_FRAME, SOFT_BITS_PER_PCG,
        SR1_PCGS_PER_FRAME, extract_reverse_mux1_full_rate_signaling_block,
    };
    use crate::phy::coding::block_interleaver::{
        Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
    };
    use crate::phy::coding::convolutional::get_1_3_k9_encoder;
    use crate::phy::walsh::WalshGenerator;
    use crate::receiver::access::DedicatedFrameReader;

    fn crc16(data: &[u8]) -> u16 {
        cdma_common::crc::crc16_ccitt(data)
    }

    #[test]
    fn rc1_per_pcg_measurements_skip_inactive_pcgs_and_tag_age() {
        let frame_abs_chip = FRAME_CHIPS * 1_000;
        let frame_abs_pcg = (frame_abs_chip / PCG_CHIPS) as u64;

        let mut aligner = Rc1TrafficFrameAligner::new(0x1234_5678);
        aligner.state = FrameAlignerState::Locked;
        aligner.chip_buf = vec![Complex32::new(0.0, 0.0); FRAME_CHIPS];
        aligner.chip_start = 0;
        aligner.sample_rate_hz = 1_228_800.0;
        aligner.absolute_chip_start = Some(frame_abs_chip as i64);
        aligner.pcg_measurement_rate = Some(Rc1TrafficRate::Eighth);
        aligner.next_measurement_abs_pcg = Some(frame_abs_pcg);
        aligner.last_processing_absolute_chip_end = Some((frame_abs_chip + FRAME_CHIPS) as u64);

        let expected_mask =
            aligner.exact_active_pcgs_for_rate(Rc1TrafficRate::Eighth, frame_abs_chip);
        let expected_count = expected_mask.iter().filter(|&&active| active).count();

        let out = aligner.emit_ready_pcg_measurements();

        assert_eq!(out.len(), expected_count);
        assert_eq!(
            aligner.next_measurement_abs_pcg,
            Some(frame_abs_pcg + SR1_PCGS_PER_FRAME as u64)
        );
        for block in out {
            assert_eq!(block.tags.get("traffic_pcg_measurement"), Some(&1));
            assert!(block.tags.contains_key("traffic_pcg_mobile_power_mdbfs"));
            assert_eq!(block.pcg_signal_snr_db.as_ref().map(Vec::len), Some(1));

            let measurement_abs_chip = *block.tags.get("absolute_chip_start").unwrap() as u64;
            let pcg_in_frame =
                ((measurement_abs_chip - frame_abs_chip as u64) / PCG_CHIPS as u64) as usize;
            assert!(expected_mask[pcg_in_frame]);

            let expected_age = (frame_abs_chip + FRAME_CHIPS) as u64 - measurement_abs_chip;
            assert_eq!(
                block.tags.get("traffic_measurement_age_chips").copied(),
                Some(expected_age as i64)
            );
        }
    }

    fn build_dedicated_fragments(
        payload_bits: &[u8],
        signaling_bits_per_frame: usize,
    ) -> Vec<Vec<u8>> {
        let mut crc_scope = Bitstream::new();
        let msg_len_octets = ((8 + payload_bits.len() + 16) / 8) as u8;
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&Bitstream::new_init(payload_bits));
        let crc = crc16(crc_scope.bits());

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&Bitstream::new_init(payload_bits));
        body.write_u32(crc as u32, 16);
        let body_bits = body.bits().to_vec();

        let fragment_capacity = signaling_bits_per_frame - 1;
        let mut fragments = Vec::new();
        let mut rem = body_bits.as_slice();
        let mut first = true;
        while !rem.is_empty() {
            let take = rem.len().min(fragment_capacity);
            let mut fragment = vec![if first { 1u8 } else { 0u8 }];
            fragment.extend_from_slice(&rem[..take]);
            if take < fragment_capacity {
                fragment.extend(std::iter::repeat_n(0u8, fragment_capacity - take));
            }
            fragments.push(fragment);
            rem = &rem[take..];
            first = false;
        }
        fragments
    }

    fn build_full_rate_rc1_frame_bits(signaling_fragment: &[u8]) -> Vec<u8> {
        assert_eq!(168, signaling_fragment.len());

        let mut info_bits = vec![1, 0, 1, 1];
        info_bits.extend_from_slice(signaling_fragment);
        assert_eq!(172, info_bits.len());

        let crc = crc12(&info_bits);
        let mut frame_bits = info_bits;
        for shift in (0..12).rev() {
            frame_bits.push(((crc >> shift) & 1) as u8);
        }
        frame_bits.extend(std::iter::repeat_n(0u8, 8));
        assert_eq!(192, frame_bits.len());
        frame_bits
    }

    fn rc1_full_rate_frame_to_chips(frame_bits: &[u8]) -> Vec<Complex32> {
        assert_eq!(192, frame_bits.len());

        let mut encoder = get_1_3_k9_encoder();
        let code_symbols = frame_bits
            .iter()
            .flat_map(|&bit| encoder.encode(bit))
            .collect::<Vec<_>>();
        assert_eq!(576, code_symbols.len());

        let interleaver = Rc12ReverseTrafficInterleaver::new(Rc12ReverseTrafficRate::Full);
        let interleaved = interleaver.encode(&code_symbols);
        assert_eq!(SOFT_BITS_PER_FRAME, interleaved.len());

        let walsh_matrix = WalshGenerator::generate_matrix::<64>();
        let mut chips = Vec::with_capacity(FRAME_CHIPS);
        for chunk in interleaved.chunks_exact(6) {
            let row = chunk[0] as usize
                + 2 * chunk[1] as usize
                + 4 * chunk[2] as usize
                + 8 * chunk[3] as usize
                + 16 * chunk[4] as usize
                + 32 * chunk[5] as usize;
            for &chip in &walsh_matrix[row] {
                for _ in 0..4 {
                    chips.push(Complex32::new(chip as f32, 0.0));
                }
            }
        }
        chips
    }

    #[test]
    fn rc1_half_rate_energy_selector_selects_eight_pcgs() {
        let mut soft = vec![0.1f32; SOFT_BITS_PER_FRAME];
        for pcg in [0usize, 2, 4, 6, 8, 10, 12, 14] {
            let start = pcg * SOFT_BITS_PER_PCG;
            let end = start + SOFT_BITS_PER_PCG;
            soft[start..end].fill(4.0);
        }
        let active = Rc1TrafficRate::Half.active_pcgs_from_soft(&soft);
        assert_eq!(active.iter().filter(|&&pcg| pcg).count(), 8);
        for pcg in [0usize, 2, 4, 6, 8, 10, 12, 14] {
            assert!(active[pcg]);
        }
    }

    #[test]
    fn rc1_synthetic_frames_round_trip_at_predicted_boundary() {
        let payload_bits = (0..240)
            .map(|idx| ((idx * 5 + 3) % 11 >= 5) as u8)
            .collect::<Vec<_>>();
        let fragments = build_dedicated_fragments(&payload_bits, 168);
        assert!(fragments.len() >= 2);

        // Sanity-check the synthetic signaling fragments themselves before
        // channel coding/modulation so later failures are clearly boundary-
        // alignment issues rather than SAR formatting issues.
        let mut direct_reader = DedicatedFrameReader::new();
        let mut direct_result = None;
        for fragment in &fragments {
            let mut bs = Bitstream::new_init(fragment);
            if let Some(frame) = direct_reader
                .process(&mut bs)
                .expect("synthetic fragment decode")
            {
                direct_result = Some((frame.crc_valid, frame.msg_length_octets, frame.data.len()));
            }
        }
        assert_eq!(Some((true, 33, payload_bits.len())), direct_result);

        let mut chips = vec![Complex32::new(0.0, 0.0); 37];
        for fragment in fragments {
            let frame_bits = build_full_rate_rc1_frame_bits(&fragment);
            chips.extend(rc1_full_rate_frame_to_chips(&frame_bits));
        }

        let mut aligner = Rc1TrafficFrameAligner::new(0x4CDC1D09);
        // absolute_chip_start chosen so predicted chip_phase=37 and frame_phase=0
        // matching the 37 zero-chip prefix in the test signal.
        aligner.absolute_chip_start = Some(1_792_012_995_354_587i64);
        aligner.chip_buf = chips;

        assert_eq!(Some(37), aligner.predicted_chip_phase());
        assert_eq!(vec![37], aligner.chip_phase_candidates());
        assert_eq!(vec![0], aligner.frame_phase_candidates(37));

        let symbols = aligner.demodulate_symbol_stream(37);
        let mut decoded_reader = DedicatedFrameReader::new();
        let mut decoded_crc_valid = false;
        for frame_idx in 0..2 {
            let symbol_start = frame_idx * RC1_SYMBOLS_PER_FRAME;
            let frame_soft =
                Rc1TrafficFrameAligner::frame_soft_from_symbols(&symbols, symbol_start).unwrap();
            let chip_offset = 37 + frame_idx * FRAME_CHIPS;
            let decoded = aligner.decode_frame_soft(&frame_soft, Rc1TrafficRate::Full, chip_offset);
            assert!(
                decoded.validation.phy_valid,
                "expected synthetic full-rate frame {} to decode PHY-valid",
                frame_idx
            );
            let signaling = extract_reverse_mux1_full_rate_signaling_block(
                &decoded.bits[..Rc1TrafficRate::Full.info_bits()],
                ReverseMux1SignalingLayout::Suffix,
            )
            .expect("synthetic full-rate signaling block");
            let mut bs = Bitstream::new_init(&signaling.bits);
            if let Some(frame) = decoded_reader
                .process(&mut bs)
                .expect("decoded fragment decode")
            {
                decoded_crc_valid = frame.crc_valid;
            }
        }
        assert!(decoded_crc_valid);
    }
}
