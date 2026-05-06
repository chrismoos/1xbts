use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use cdma_common::bits::Bitstream;
use log::{info, trace};
use num_complex::Complex32;

use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_576};
use crate::phy::coding::convolutional::{
    get_1_3_k9_soft_viterbi_decoder, get_1_3_k9_viterbi_decoder,
};
use crate::phy::walsh::WalshGenerator;
use crate::receiver::access::AccessFrameReader;

use super::{CDMA_CHIP_RATE, PipelineProcessor, SampleBlock, raw_to_soft};
use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME,
    RC1_WALSH_CHIPS_PER_SYMBOL,
};

const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;
const SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const REPEAT_PAIR_COUNT: usize = SOFT_BITS_PER_FRAME / 2;
const REPEAT_PAIR_SCORE_MIN: usize = 287;
const MAX_PERFECT_DECODE_CANDIDATES: usize = 4;
const MAX_NEAR_MISS_DECODE_CANDIDATES: usize = 1;
const MAX_REASSEMBLY_FRAMES: usize = 12;
const MAX_SEARCH_BUFFER: usize = SOFT_BITS_PER_FRAME * 16;
const MIN_SEARCH_RETRY_BITS: usize = RC1_SOFT_BITS_PER_SYMBOL * 24;
const SEARCH_DIAG_LOG_THRESHOLD_MS: u128 = 10;
const W0_PEAK_RATIO_MIN: f32 = 0.10;
const W0_MARGIN_MIN: f32 = 1.5;
const REPEAT_PAIR_RAW_INDICES: [(usize, usize); REPEAT_PAIR_COUNT] =
    build_repeat_pair_raw_indices();

const fn bit_reverse_m(m: usize, mut val: usize) -> usize {
    let mut out = 0usize;
    let mut i = 0usize;
    while i < m {
        out = (out << 1) | (val & 1);
        val >>= 1;
        i += 1;
    }
    out
}

const fn sr1_576_deinterleaved_index(raw_index: usize) -> usize {
    (1usize << SR1_PARAMS_576.m) * (raw_index % SR1_PARAMS_576.j)
        + bit_reverse_m(SR1_PARAMS_576.m, raw_index / SR1_PARAMS_576.j)
}

const fn build_repeat_pair_raw_indices() -> [(usize, usize); REPEAT_PAIR_COUNT] {
    let mut deint_to_raw = [0usize; SOFT_BITS_PER_FRAME];
    let mut raw_index = 0usize;
    while raw_index < SOFT_BITS_PER_FRAME {
        let deint_index = sr1_576_deinterleaved_index(raw_index);
        deint_to_raw[deint_index] = raw_index;
        raw_index += 1;
    }

    let mut pairs = [(0usize, 0usize); REPEAT_PAIR_COUNT];
    let mut pair_index = 0usize;
    while pair_index < REPEAT_PAIR_COUNT {
        pairs[pair_index] = (
            deint_to_raw[pair_index * 2],
            deint_to_raw[pair_index * 2 + 1],
        );
        pair_index += 1;
    }
    pairs
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecoderState {
    Searching,
    Acquiring,
    Locked,
}

#[derive(Clone, Copy, Debug)]
struct DecodedWalshSymbol {
    soft_bits: [f32; RC1_SOFT_BITS_PER_SYMBOL],
    phase: usize,
    row: usize,
    peak_ratio: f32,
    margin: f32,
}

impl DecodedWalshSymbol {
    fn is_w0_like(&self) -> bool {
        self.row == 0 && self.peak_ratio >= W0_PEAK_RATIO_MIN && self.margin >= W0_MARGIN_MIN
    }
}

#[derive(Default)]
struct ReassemblyProbeStats {
    frame_checks: usize,
    soft_checks: usize,
    hard_checks: usize,
}

#[derive(Clone, Copy, Debug)]
struct ReassemblySuccess {
    frame_idx: usize,
    frame_bit_offset: usize,
    frame_symbol_offset: usize,
    frame_chip_start: usize,
    repeat_pair_score: usize,
    mode: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct ScoredFrameCandidate {
    reassembly_bit_offset: usize,
    scored_bit_offset: usize,
    sym_mod: usize,
    frame_idx: usize,
    repeat_pair_score: usize,
    collapsed_ones: usize,
}

/// Reverse access decoder for chip-rate PN+LC-removed finger output.
///
/// The decoder keeps one simple timing invariant: access Walsh symbols begin at
/// absolute `chip % 256 == 0`. It slices the chip stream on that cadence,
/// demodulates each 256-chip Walsh symbol into six coded soft bits, counts W0
/// preamble frames internally, then uses the existing repeat-pair/CRC heuristic
/// to find and lock the access frame cadence.
pub struct ReverseAccessDecoder {
    state: DecoderState,
    oversample: usize,
    pending_samples: VecDeque<Complex32>,
    pending_chip_start: Option<usize>,
    input_sample_rate_hz: f64,
    decoded_bit_rate_hz: f64,
    tags: HashMap<&'static str, i64>,

    search_w0_soft: Vec<f32>,
    search_w0_chip_start: Option<usize>,
    preamble_frames_seen: usize,

    soft_buf: VecDeque<f32>,
    soft_chip_start: usize,
    last_search_len: usize,
    use_soft_viterbi: bool,

    symbols_decoded: u64,
    search_calls: u64,
    rank_us: u64,
    decode_us: u64,
    locked_us: u64,
}

impl ReverseAccessDecoder {
    pub fn new() -> Self {
        Self {
            state: DecoderState::Searching,
            oversample: 1,
            pending_samples: VecDeque::new(),
            pending_chip_start: None,
            input_sample_rate_hz: 0.0,
            decoded_bit_rate_hz: CDMA_CHIP_RATE / PN_CHIPS_PER_SYMBOL as f64,
            tags: HashMap::new(),
            search_w0_soft: Vec::new(),
            search_w0_chip_start: None,
            preamble_frames_seen: 0,
            soft_buf: VecDeque::new(),
            soft_chip_start: 0,
            last_search_len: 0,
            use_soft_viterbi: true,
            symbols_decoded: 0,
            search_calls: 0,
            rank_us: 0,
            decode_us: 0,
            locked_us: 0,
        }
    }

    pub fn with_soft_viterbi(mut self, enabled: bool) -> Self {
        self.use_soft_viterbi = enabled;
        self
    }

    fn block_oversample(block: &SampleBlock) -> usize {
        block
            .tags
            .get("access_oversample")
            .copied()
            .map(|v| v.max(1) as usize)
            .unwrap_or_else(|| (block.samples.len() / PN_CHIPS_PER_SYMBOL).max(1))
    }

    fn symbol_sample_len(&self) -> usize {
        PN_CHIPS_PER_SYMBOL * self.oversample.max(1)
    }

    fn update_rates(&mut self, sample_rate_hz: f64) {
        self.input_sample_rate_hz = sample_rate_hz;
        self.decoded_bit_rate_hz = if sample_rate_hz > 0.0 {
            sample_rate_hz / (self.oversample.max(1) * PN_CHIPS_PER_SYMBOL) as f64
        } else {
            CDMA_CHIP_RATE / PN_CHIPS_PER_SYMBOL as f64
        };
    }

    fn symbol_energies(
        chips: &[Complex32],
        oversample: usize,
        phase: usize,
    ) -> [f32; RC1_WALSH_CHIPS_PER_SYMBOL] {
        debug_assert_eq!(chips.len(), PN_CHIPS_PER_SYMBOL * oversample);

        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for walsh_chip_idx in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
            let mut acc = Complex32::new(0.0, 0.0);
            let base = walsh_chip_idx * RC1_PN_CHIPS_PER_WALSH_CHIP * oversample;
            for pn in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                acc += chips[base + pn * oversample + phase];
            }
            walsh_chips[walsh_chip_idx] = acc;
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

    fn peak_metrics(energies: &[f32; RC1_WALSH_CHIPS_PER_SYMBOL]) -> (usize, f32, f32) {
        let total_energy: f32 = energies.iter().sum();
        if total_energy <= 1e-9 {
            return (0, 0.0, 0.0);
        }

        let mut best_row = 0usize;
        let mut best_energy = energies[0];
        let mut second_energy = 0.0f32;
        for (row, &energy) in energies.iter().enumerate().skip(1) {
            if energy > best_energy {
                second_energy = best_energy;
                best_energy = energy;
                best_row = row;
            } else if energy > second_energy {
                second_energy = energy;
            }
        }

        (
            best_row,
            best_energy / total_energy,
            best_energy / second_energy.max(1e-9),
        )
    }

    fn demod_symbol(&self, chips: &[Complex32]) -> DecodedWalshSymbol {
        let mut best = DecodedWalshSymbol {
            soft_bits: [0.0; RC1_SOFT_BITS_PER_SYMBOL],
            phase: 0,
            row: 0,
            peak_ratio: f32::NEG_INFINITY,
            margin: f32::NEG_INFINITY,
        };

        for phase in 0..self.oversample.max(1) {
            let energies = Self::symbol_energies(chips, self.oversample.max(1), phase);
            let (row, peak_ratio, margin) = Self::peak_metrics(&energies);
            if peak_ratio > best.peak_ratio
                || (peak_ratio == best.peak_ratio && margin > best.margin)
            {
                best = DecodedWalshSymbol {
                    soft_bits: Self::soft_bits_from_energies(&energies),
                    phase,
                    row,
                    peak_ratio,
                    margin,
                };
            }
        }

        best
    }

    fn deinterleave_and_collapse(soft: &[f32]) -> Vec<f32> {
        let interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
        let deinterleaved = interleaver.decode_soft(soft);
        deinterleaved
            .chunks_exact(2)
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect()
    }

    fn decode_soft_bits(collapsed: &[f32]) -> Vec<u8> {
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

        get_1_3_k9_soft_viterbi_decoder().decode_block_from_state(&inputs, 0)
    }

    fn decode_hard_bits(collapsed: &[f32]) -> Vec<u8> {
        let mut viterbi = get_1_3_k9_viterbi_decoder();
        let mut bits = Vec::with_capacity(collapsed.len() / 3 + 8);
        for chunk in collapsed.chunks_exact(3) {
            let sym = [
                if chunk[0] >= 0.0 { 0u8 } else { 1u8 },
                if chunk[1] >= 0.0 { 0u8 } else { 1u8 },
                if chunk[2] >= 0.0 { 0u8 } else { 1u8 },
            ];
            if let Some(bit) = viterbi.process(&sym) {
                bits.push(bit);
            }
        }
        bits.extend(viterbi.finish());
        bits
    }

    fn decode_bits_at_offset(&self, bit_offset: usize) -> Option<(Option<Vec<u8>>, Vec<u8>)> {
        if self.soft_buf.len() < bit_offset + SOFT_BITS_PER_FRAME {
            return None;
        }

        let soft = self
            .soft_buf
            .iter()
            .skip(bit_offset)
            .take(SOFT_BITS_PER_FRAME)
            .copied()
            .collect::<Vec<_>>();
        let collapsed = Self::deinterleave_and_collapse(&soft);
        let soft_bits = if self.use_soft_viterbi {
            Some(Self::decode_soft_bits(&collapsed))
        } else {
            None
        };

        Some((soft_bits, Self::decode_hard_bits(&collapsed)))
    }

    fn bit_offset_chip_start(&self, bit_offset: usize) -> usize {
        self.soft_chip_start
            .saturating_add((bit_offset / RC1_SOFT_BITS_PER_SYMBOL) * PN_CHIPS_PER_SYMBOL)
    }

    fn repeat_pair_score_at_offset(&self, bit_offset: usize) -> Option<usize> {
        if self.soft_buf.len() < bit_offset + SOFT_BITS_PER_FRAME {
            return None;
        }

        Some(
            REPEAT_PAIR_RAW_INDICES
                .iter()
                .filter(|(a, b)| {
                    let a = self.soft_buf[bit_offset + *a];
                    let b = self.soft_buf[bit_offset + *b];
                    (a >= 0.0) == (b >= 0.0)
                })
                .count(),
        )
    }

    fn collapsed_ones_at_offset(&self, bit_offset: usize) -> Option<usize> {
        if self.soft_buf.len() < bit_offset + SOFT_BITS_PER_FRAME {
            return None;
        }

        let mut ones = 0usize;
        for (a, b) in REPEAT_PAIR_RAW_INDICES {
            let collapsed = (self.soft_buf[bit_offset + a] + self.soft_buf[bit_offset + b]) * 0.5;
            if collapsed < 0.0 {
                ones += 1;
            }
        }

        Some(ones)
    }

    fn ranked_frame_candidates(&self, offsets_to_try: usize) -> Vec<ScoredFrameCandidate> {
        let mut candidates = Vec::new();
        for sym_offset in 0..offsets_to_try {
            let base_bit_offset = sym_offset * RC1_SOFT_BITS_PER_SYMBOL;
            let available_frames = ((self.soft_buf.len().saturating_sub(base_bit_offset))
                / SOFT_BITS_PER_FRAME)
                .min(MAX_REASSEMBLY_FRAMES);
            for frame_idx in 0..available_frames {
                let bit_offset = base_bit_offset + frame_idx * SOFT_BITS_PER_FRAME;
                let Some(repeat_pair_score) = self.repeat_pair_score_at_offset(bit_offset) else {
                    continue;
                };
                if repeat_pair_score < REPEAT_PAIR_SCORE_MIN {
                    continue;
                }
                let Some(collapsed_ones) = self.collapsed_ones_at_offset(bit_offset) else {
                    continue;
                };
                if collapsed_ones == 0 || collapsed_ones == REPEAT_PAIR_COUNT {
                    continue;
                }
                let candidate = ScoredFrameCandidate {
                    reassembly_bit_offset: base_bit_offset,
                    scored_bit_offset: bit_offset,
                    sym_mod: sym_offset,
                    frame_idx,
                    repeat_pair_score,
                    collapsed_ones,
                };
                match candidates
                    .iter()
                    .position(|existing: &ScoredFrameCandidate| {
                        existing.reassembly_bit_offset == base_bit_offset
                    }) {
                    Some(pos)
                        if candidate.repeat_pair_score > candidates[pos].repeat_pair_score =>
                    {
                        candidates[pos] = candidate;
                    }
                    Some(_) => {}
                    None => candidates.push(candidate),
                }
            }
        }

        candidates.sort_by(|a, b| {
            b.repeat_pair_score
                .cmp(&a.repeat_pair_score)
                .then_with(|| a.frame_idx.cmp(&b.frame_idx))
                .then_with(|| a.sym_mod.cmp(&b.sym_mod))
        });
        candidates
    }

    fn decode_candidates(ranked_candidates: &[ScoredFrameCandidate]) -> Vec<ScoredFrameCandidate> {
        let perfect_candidates = ranked_candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.repeat_pair_score == REPEAT_PAIR_COUNT)
            .take(MAX_PERFECT_DECODE_CANDIDATES)
            .collect::<Vec<_>>();
        if !perfect_candidates.is_empty() {
            return perfect_candidates;
        }

        ranked_candidates
            .iter()
            .copied()
            .take(MAX_NEAR_MISS_DECODE_CANDIDATES)
            .collect()
    }

    fn try_reassembly_at_offset(
        &self,
        bit_offset: usize,
        stats: &mut ReassemblyProbeStats,
    ) -> Option<ReassemblySuccess> {
        let available_frames = ((self.soft_buf.len().saturating_sub(bit_offset))
            / SOFT_BITS_PER_FRAME)
            .min(MAX_REASSEMBLY_FRAMES);
        if available_frames == 0 {
            return None;
        }

        let mut soft_reader = if self.use_soft_viterbi {
            Some(AccessFrameReader::new())
        } else {
            None
        };
        let mut hard_reader = AccessFrameReader::new();

        for frame_idx in 0..available_frames {
            stats.frame_checks += 1;
            let frame_bit_offset = bit_offset + frame_idx * SOFT_BITS_PER_FRAME;
            let Some((soft_bits, hard_bits)) = self.decode_bits_at_offset(frame_bit_offset) else {
                break;
            };

            if let Some(ref mut reader) = soft_reader
                && let Some(ref sb) = soft_bits
                && sb.len() >= 96
            {
                stats.soft_checks += 1;
                let mut fragment = Bitstream::new_init(&sb[..88]);
                if let Ok(Some(frame)) = reader.process(&mut fragment)
                    && frame.crc_valid
                {
                    let repeat_pair_score = self
                        .repeat_pair_score_at_offset(frame_bit_offset)
                        .unwrap_or(0);
                    let frame_chip_start = self.bit_offset_chip_start(frame_bit_offset);
                    return Some(ReassemblySuccess {
                        frame_idx,
                        frame_bit_offset,
                        frame_symbol_offset: frame_bit_offset / RC1_SOFT_BITS_PER_SYMBOL,
                        frame_chip_start,
                        repeat_pair_score,
                        mode: "soft",
                    });
                }
            }

            if hard_bits.len() >= 96 {
                stats.hard_checks += 1;
                let mut fragment = Bitstream::new_init(&hard_bits[..88]);
                if let Ok(Some(frame)) = hard_reader.process(&mut fragment)
                    && frame.crc_valid
                {
                    let repeat_pair_score = self
                        .repeat_pair_score_at_offset(frame_bit_offset)
                        .unwrap_or(0);
                    let frame_chip_start = self.bit_offset_chip_start(frame_bit_offset);
                    return Some(ReassemblySuccess {
                        frame_idx,
                        frame_bit_offset,
                        frame_symbol_offset: frame_bit_offset / RC1_SOFT_BITS_PER_SYMBOL,
                        frame_chip_start,
                        repeat_pair_score,
                        mode: "hard",
                    });
                }
            }
        }

        None
    }

    fn drain_soft_front(&mut self, n_bits: usize) {
        let n = n_bits.min(self.soft_buf.len());
        self.soft_buf.drain(..n);
        let symbols_drained = n / RC1_SOFT_BITS_PER_SYMBOL;
        self.soft_chip_start = self
            .soft_chip_start
            .saturating_add(symbols_drained * PN_CHIPS_PER_SYMBOL);
    }

    fn append_soft_symbol(
        &mut self,
        chip_start: usize,
        soft_bits: &[f32; RC1_SOFT_BITS_PER_SYMBOL],
    ) {
        if self.soft_buf.is_empty() {
            self.soft_chip_start = chip_start;
        }
        self.soft_buf.extend(soft_bits);
    }

    fn append_soft_slice(&mut self, chip_start: usize, soft_bits: &[f32]) {
        if soft_bits.is_empty() {
            return;
        }
        if self.soft_buf.is_empty() {
            self.soft_chip_start = chip_start;
        }
        self.soft_buf.extend(soft_bits.iter().copied());
    }

    fn note_preamble_frame(&mut self) {
        self.preamble_frames_seen = self.preamble_frames_seen.saturating_add(1);
    }

    fn emit_decoded_frames(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.soft_buf.len() >= SOFT_BITS_PER_FRAME {
            let frame_started = Instant::now();
            let soft_snapshot = self
                .soft_buf
                .iter()
                .take(SOFT_BITS_PER_FRAME)
                .copied()
                .collect::<Vec<_>>();
            let collapsed = Self::deinterleave_and_collapse(&soft_snapshot);
            let decoded_bits = if self.use_soft_viterbi {
                Self::decode_soft_bits(&collapsed)
            } else {
                Self::decode_hard_bits(&collapsed)
            };
            self.decode_us = self
                .decode_us
                .saturating_add(frame_started.elapsed().as_micros() as u64);

            let frame_peak_abs = soft_snapshot.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
            let frame_avg_abs = if soft_snapshot.is_empty() {
                0.0
            } else {
                soft_snapshot.iter().map(|v| v.abs()).sum::<f32>() / soft_snapshot.len() as f32
            };
            let weak_threshold = frame_peak_abs * 0.25;
            let frame_weak_soft_bits = soft_snapshot
                .iter()
                .filter(|v| v.abs() <= weak_threshold)
                .count();

            let frame_chip_start = self.soft_chip_start;
            let samples = decoded_bits
                .into_iter()
                .take(96)
                .map(|bit| Complex32::new(bit as f32, 0.0))
                .collect::<Vec<_>>();
            let mut block = SampleBlock::new(samples, frame_chip_start)
                .with_sample_rate_hz(self.decoded_bit_rate_hz);
            block.tags = self.tags.clone();
            block
                .tags
                .insert("absolute_chip_start", frame_chip_start as i64);
            block.tags.insert("access_frame_aligned", 1);
            block.tags.insert(
                "access_frame_soft_avg_abs_milli",
                (frame_avg_abs * 1000.0) as i64,
            );
            block.tags.insert(
                "access_frame_soft_peak_abs_milli",
                (frame_peak_abs * 1000.0) as i64,
            );
            block
                .tags
                .insert("access_frame_weak_soft_bits", frame_weak_soft_bits as i64);
            block.tags.insert("reverse_access_decoder_frame", 1);
            out.push(block);

            self.drain_soft_front(SOFT_BITS_PER_FRAME);
            debug_assert_eq!(self.soft_chip_start % PN_CHIPS_PER_SYMBOL, 0);
        }
        self.last_search_len = 0;
        out
    }

    fn search_step(&mut self) -> Vec<SampleBlock> {
        let search_started = Instant::now();
        let mut stats = ReassemblyProbeStats::default();

        if self.soft_buf.len() < SOFT_BITS_PER_FRAME {
            return Vec::new();
        }
        if self.last_search_len >= SOFT_BITS_PER_FRAME
            && self.soft_buf.len() < self.last_search_len + MIN_SEARCH_RETRY_BITS
        {
            return Vec::new();
        }

        self.search_calls = self.search_calls.saturating_add(1);
        let offsets_to_try = RC1_SYMBOLS_PER_FRAME.min(
            (self.soft_buf.len().saturating_sub(SOFT_BITS_PER_FRAME)) / RC1_SOFT_BITS_PER_SYMBOL
                + 1,
        );
        let rank_started = Instant::now();
        let ranked_candidates = self.ranked_frame_candidates(offsets_to_try);
        self.rank_us = self
            .rank_us
            .saturating_add(rank_started.elapsed().as_micros() as u64);
        let decode_candidates = Self::decode_candidates(&ranked_candidates);
        let max_pair_score = ranked_candidates
            .first()
            .map(|candidate| candidate.repeat_pair_score)
            .unwrap_or(0);

        for candidate in decode_candidates {
            if let Some(success) =
                self.try_reassembly_at_offset(candidate.reassembly_bit_offset, &mut stats)
            {
                info!(
                    "reverse_access_decoder: locked symbol_offset={} candidate_frame_idx={} completion_frame_idx={} scored_bit_offset={} chip_start={} crc_frame_bit_offset={} crc_frame_symbol_offset={} crc_frame_chip_start={} crc_frame_mod256={} crc_frame_pn_mod32768={} crc_mode={} crc_repeat_pair_score={}/{} repeat_pair_score={}/{} collapsed_ones={} max_pair_score={}",
                    candidate.sym_mod,
                    candidate.frame_idx,
                    success.frame_idx,
                    candidate.scored_bit_offset,
                    self.bit_offset_chip_start(candidate.reassembly_bit_offset),
                    success.frame_bit_offset,
                    success.frame_symbol_offset,
                    success.frame_chip_start,
                    success.frame_chip_start % PN_CHIPS_PER_SYMBOL,
                    success.frame_chip_start % 32768,
                    success.mode,
                    success.repeat_pair_score,
                    REPEAT_PAIR_COUNT,
                    candidate.repeat_pair_score,
                    REPEAT_PAIR_COUNT,
                    candidate.collapsed_ones,
                    max_pair_score,
                );
                self.drain_soft_front(candidate.reassembly_bit_offset);
                self.state = DecoderState::Locked;
                self.last_search_len = 0;
                return self.emit_decoded_frames();
            }
        }

        self.last_search_len = self.soft_buf.len();
        if self.soft_buf.len() > MAX_SEARCH_BUFFER {
            trace!(
                "reverse_access_decoder: trimming {} bits from search buffer (total={})",
                SOFT_BITS_PER_FRAME,
                self.soft_buf.len(),
            );
            self.drain_soft_front(SOFT_BITS_PER_FRAME);
            self.last_search_len = 0;
        }

        let elapsed_ms = search_started.elapsed().as_millis();
        if elapsed_ms >= SEARCH_DIAG_LOG_THRESHOLD_MS {
            info!(
                "reverse_access_decoder: search_diag chip_start={} buffer_len={} offsets_to_try={} ranked_candidates={} frame_checks={} soft_checks={} hard_checks={} max_pair_score={} elapsed={}ms",
                self.soft_chip_start,
                self.soft_buf.len(),
                offsets_to_try,
                ranked_candidates.len(),
                stats.frame_checks,
                stats.soft_checks,
                stats.hard_checks,
                max_pair_score,
                elapsed_ms,
            );
        }

        Vec::new()
    }

    fn handle_symbol(&mut self, chip_start: usize, symbol: DecodedWalshSymbol) -> Vec<SampleBlock> {
        self.symbols_decoded = self.symbols_decoded.saturating_add(1);
        self.tags
            .insert("access_selected_phase", symbol.phase as i64);
        self.tags.insert("access_selected_row", symbol.row as i64);
        self.tags.insert(
            "access_selected_peak_ratio_milli",
            (symbol.peak_ratio * 1000.0) as i64,
        );
        self.tags.insert(
            "access_selected_margin_milli",
            (symbol.margin * 1000.0) as i64,
        );

        match self.state {
            DecoderState::Searching => {
                if symbol.is_w0_like() {
                    if self.search_w0_soft.is_empty() {
                        self.search_w0_chip_start = Some(chip_start);
                    }
                    self.search_w0_soft.extend(symbol.soft_bits);
                    if self.search_w0_soft.len() >= SOFT_BITS_PER_FRAME {
                        self.search_w0_soft.clear();
                        self.search_w0_chip_start = None;
                        self.note_preamble_frame();
                        return Vec::new();
                    }
                    return Vec::new();
                }

                if self.preamble_frames_seen == 0 {
                    self.search_w0_soft.clear();
                    self.search_w0_chip_start = None;
                    return Vec::new();
                }

                self.state = DecoderState::Acquiring;
                if let Some(w0_chip_start) = self.search_w0_chip_start {
                    let guard = std::mem::take(&mut self.search_w0_soft);
                    self.append_soft_slice(w0_chip_start, &guard);
                }
                self.search_w0_chip_start = None;
                self.append_soft_symbol(chip_start, &symbol.soft_bits);
                self.search_step()
            }
            DecoderState::Acquiring => {
                self.append_soft_symbol(chip_start, &symbol.soft_bits);
                self.search_step()
            }
            DecoderState::Locked => {
                let t = Instant::now();
                self.append_soft_symbol(chip_start, &symbol.soft_bits);
                let out = self.emit_decoded_frames();
                self.locked_us = self
                    .locked_us
                    .saturating_add(t.elapsed().as_micros() as u64);
                out
            }
        }
    }
}

impl PipelineProcessor for ReverseAccessDecoder {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.pending_chip_start.is_none() {
            self.pending_chip_start = Some(block.chip_start);
            self.oversample = Self::block_oversample(&block);
        }
        self.oversample = Self::block_oversample(&block);
        self.update_rates(block.sample_rate_hz);
        self.tags = block.tags.clone();
        self.pending_samples.extend(block.samples);

        let mut out = Vec::new();
        if let Some(chip_start) = self.pending_chip_start {
            let rem = chip_start % PN_CHIPS_PER_SYMBOL;
            if rem != 0 {
                let skip_chips = PN_CHIPS_PER_SYMBOL - rem;
                let skip_samples = skip_chips * self.oversample.max(1);
                if self.pending_samples.len() < skip_samples {
                    return out;
                }
                trace!(
                    "reverse_access_decoder: aligning chip_start={} to {}-chip grid by skipping {} chips",
                    chip_start, PN_CHIPS_PER_SYMBOL, skip_chips,
                );
                self.pending_samples.drain(..skip_samples);
                self.pending_chip_start = Some(chip_start + skip_chips);
            }
        }

        let symbol_len = self.symbol_sample_len();
        while self.pending_samples.len() >= symbol_len {
            let chip_start = self.pending_chip_start.unwrap_or(0);
            let samples = self.pending_samples.drain(..symbol_len).collect::<Vec<_>>();
            let symbol = self.demod_symbol(&samples);
            out.extend(self.handle_symbol(chip_start, symbol));
            self.pending_chip_start = Some(chip_start + PN_CHIPS_PER_SYMBOL);
        }

        out
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        match self.state {
            DecoderState::Locked => self.emit_decoded_frames(),
            DecoderState::Searching | DecoderState::Acquiring => {
                self.pending_samples.clear();
                self.search_w0_soft.clear();
                self.soft_buf.clear();
                Vec::new()
            }
        }
    }

    fn name(&self) -> &'static str {
        "ReverseAccessDecoder"
    }

    fn metrics(&self) -> Vec<(&'static str, String)> {
        let state = match self.state {
            DecoderState::Searching => "searching",
            DecoderState::Acquiring => "acquiring",
            DecoderState::Locked => "locked",
        };
        vec![
            ("state", state.to_string()),
            ("symbols", self.symbols_decoded.to_string()),
            ("preamble_frames", self.preamble_frames_seen.to_string()),
            ("search_calls", self.search_calls.to_string()),
            ("rank_ms", format!("{:.1}", self.rank_us as f64 / 1000.0)),
            (
                "decode_ms",
                format!("{:.1}", self.decode_us as f64 / 1000.0),
            ),
            (
                "locked_ms",
                format!("{:.1}", self.locked_us as f64 / 1000.0),
            ),
        ]
    }
}
