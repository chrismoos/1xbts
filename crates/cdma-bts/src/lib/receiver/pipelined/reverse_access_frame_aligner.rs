use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};

use log::{info, trace};
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock, raw_to_soft};
use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_576};
use crate::phy::coding::convolutional::{
    get_1_3_k9_soft_viterbi_decoder, get_1_3_k9_viterbi_decoder,
};
use crate::receiver::access::AccessFrameReader;
use cdma_common::bits::Bitstream;
use cdma_common::consts::{RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME};

const SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const PN_CHIPS_PER_SYMBOL: usize = 256;
/// Maximum soft bits to buffer while searching for a CRC-valid frame boundary.
/// Access messages can span multiple 20 ms fragments, so keep enough history
/// to reassemble at least the maximum-length Access message while sweeping the
/// 96-symbol frame phase.
const MAX_SEARCH_BUFFER: usize = SOFT_BITS_PER_FRAME * 16;
const MAX_REASSEMBLY_FRAMES: usize = 12;
// Rescanning the full 96-offset search window every few symbols is expensive
// on dead bursts and can starve live RX. Wait for a more meaningful amount of
// fresh soft bits before retrying the search after a miss.
const MIN_SEARCH_RETRY_BITS: usize = RC1_SOFT_BITS_PER_SYMBOL * 24;
// The frame boundary should land close to the first non-W0 transition. There
// is no value in running CRC-heavy frame search deep inside the W0 preamble,
// so once the Walsh stage provides a hint we keep at most the last 16 W0
// symbols ahead of that boundary.
const PRE_HINT_W0_SYMBOL_GUARD: usize = 16;
const SEARCH_DIAG_LOG_THRESHOLD_MS: u128 = 10;
const REPEAT_PAIR_COUNT: usize = SOFT_BITS_PER_FRAME / 2;
/// Access-channel encoded symbols are repeated twice before interleaving. A
/// valid frame should therefore have nearly all 288 repeated pairs agree, but
/// pure W0/preamble-like windows also match perfectly. The collapsed ones
/// guard rejects those constant frames before running Viterbi/CRC.
const REPEAT_PAIR_SCORE_MIN: usize = 287;
// Perfect repeated-pair candidates are the only ones worth fanout-searching.
// A 287/288 near-miss can still be real, but trying every near-miss offset is
// the live latency failure mode: 12 candidates × 12 reassembly frames × soft
// and hard Viterbi checks. Keep the near-miss path as a single top-ranked
// fallback so marginal captures are still covered without letting bad fingers
// dominate the RX budget.
const MAX_PERFECT_DECODE_CANDIDATES: usize = 4;
const MAX_NEAR_MISS_DECODE_CANDIDATES: usize = 1;
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

#[derive(Default)]
struct ReassemblyProbeStats {
    frame_checks: usize,
    soft_checks: usize,
    hard_checks: usize,
}

#[derive(Clone, Copy, Debug)]
struct ReassemblySuccess {
    frame_idx: usize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameAlignerState {
    Searching,
    Locked,
}

/// Finds the access-channel frame boundary within the soft-bit stream.
///
/// Sits between `ReverseAccessWalshSymbolDemodProcessor` and the
/// deinterleaver.  Buffers incoming soft bits (groups of 6 per Walsh
/// symbol) and slides by one symbol at a time, trying CRC-30 decode
/// at each candidate offset.  Once a valid CRC frame is found, the
/// processor locks on that frame boundary and emits properly aligned
/// 576-soft-bit frames from that point forward.
pub struct ReverseAccessFrameAligner {
    state: FrameAlignerState,
    soft_buf: VecDeque<f32>,
    tags: HashMap<&'static str, i64>,
    chip_start: usize,
    sample_rate_hz: f64,
    last_search_len: usize,
    /// When true, try soft-decision Viterbi in addition to hard-decision.
    /// Soft Viterbi is significantly more expensive and provides marginal
    /// gain at the SNR levels seen on the access channel.
    use_soft_viterbi: bool,
    frame_hint_symbols: Option<usize>,
    // Timing instrumentation
    rank_us: u64,
    locked_us: u64,
    search_calls: u64,
    total_hard_checks: u64,
    ranked_searches: u64,
    frame_aligner_diag: bool,
}

impl ReverseAccessFrameAligner {
    pub fn new() -> Self {
        Self {
            state: FrameAlignerState::Searching,
            soft_buf: VecDeque::new(),
            tags: HashMap::new(),
            chip_start: 0,
            sample_rate_hz: 0.0,
            last_search_len: 0,
            use_soft_viterbi: false,
            frame_hint_symbols: None,
            rank_us: 0,
            locked_us: 0,
            search_calls: 0,
            total_hard_checks: 0,
            ranked_searches: 0,
            frame_aligner_diag: false,
        }
    }

    pub fn with_soft_viterbi(mut self, enabled: bool) -> Self {
        self.use_soft_viterbi = enabled;
        self
    }

    fn decode_hard_bits(collapsed: &[f32]) -> Vec<u8> {
        let mut viterbi = get_1_3_k9_viterbi_decoder();
        let mut bits: Vec<u8> = Vec::with_capacity(collapsed.len() / 3 + 8);
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

    fn decode_soft_bits(collapsed: &[f32]) -> Vec<u8> {
        let peak = collapsed.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };

        let mut viterbi = get_1_3_k9_soft_viterbi_decoder();
        let mut bits: Vec<u8> = Vec::with_capacity(collapsed.len() / 3 + 8);
        for chunk in collapsed.chunks_exact(3) {
            let sym = [
                raw_to_soft(chunk[0], inv_peak),
                raw_to_soft(chunk[1], inv_peak),
                raw_to_soft(chunk[2], inv_peak),
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

        let soft: Vec<f32> = self
            .soft_buf
            .iter()
            .skip(bit_offset)
            .take(SOFT_BITS_PER_FRAME)
            .copied()
            .collect();

        let interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
        let deinterleaved = interleaver.decode_soft(&soft);
        let collapsed: Vec<f32> = deinterleaved
            .chunks_exact(2)
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect();

        let soft_bits = if self.use_soft_viterbi {
            Some(Self::decode_soft_bits(&collapsed))
        } else {
            None
        };

        Some((soft_bits, Self::decode_hard_bits(&collapsed)))
    }

    fn try_reassembly_at_offset(
        &self,
        bit_offset: usize,
        log_detail: bool,
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

            if let Some(ref mut reader) = soft_reader {
                if let Some(ref sb) = soft_bits {
                    if sb.len() >= 96 {
                        stats.soft_checks += 1;
                        let mut fragment = Bitstream::new_init(&sb[..88]);
                        if let Ok(Some(frame)) = reader.process(&mut fragment) {
                            if frame.crc_valid {
                                let repeat_pair_score = self
                                    .repeat_pair_score_at_offset(frame_bit_offset)
                                    .unwrap_or(0);
                                info!(
                                    "frame_aligner: offset={} frame_idx={} frame_bit_offset={} mode=soft crc_valid=true msg_len={} repeat_pair_score={}/{}",
                                    bit_offset,
                                    frame_idx,
                                    frame_bit_offset,
                                    frame.msg_length_octets,
                                    repeat_pair_score,
                                    REPEAT_PAIR_COUNT,
                                );
                                return Some(ReassemblySuccess { frame_idx });
                            } else if log_detail {
                                trace!(
                                    "frame_aligner: offset={} frame_idx={} mode=soft crc_valid=false msg_len={}",
                                    bit_offset, frame_idx, frame.msg_length_octets,
                                );
                            }
                        }
                    }
                }
            }

            if hard_bits.len() >= 96 {
                stats.hard_checks += 1;
                let mut fragment = Bitstream::new_init(&hard_bits[..88]);
                if let Ok(Some(frame)) = hard_reader.process(&mut fragment) {
                    if frame.crc_valid {
                        let repeat_pair_score = self
                            .repeat_pair_score_at_offset(frame_bit_offset)
                            .unwrap_or(0);
                        info!(
                            "frame_aligner: offset={} frame_idx={} frame_bit_offset={} mode=hard crc_valid=true msg_len={} repeat_pair_score={}/{}",
                            bit_offset,
                            frame_idx,
                            frame_bit_offset,
                            frame.msg_length_octets,
                            repeat_pair_score,
                            REPEAT_PAIR_COUNT,
                        );
                        return Some(ReassemblySuccess { frame_idx });
                    } else if log_detail {
                        trace!(
                            "frame_aligner: offset={} frame_idx={} mode=hard crc_valid=false msg_len={}",
                            bit_offset, frame_idx, frame.msg_length_octets,
                        );
                    }
                }
            }
        }

        None
    }

    fn drain_front(&mut self, n_bits: usize) {
        let n = n_bits.min(self.soft_buf.len());
        self.soft_buf.drain(..n);
        // Each symbol = 6 soft bits = 256 PN chips
        let symbols_drained = n / RC1_SOFT_BITS_PER_SYMBOL;
        self.chip_start = self
            .chip_start
            .saturating_add(symbols_drained * PN_CHIPS_PER_SYMBOL);
    }

    fn trim_pre_hint_w0(&mut self) -> usize {
        let Some(hint) = self.frame_hint_symbols else {
            return 0;
        };
        if hint <= PRE_HINT_W0_SYMBOL_GUARD {
            return 0;
        }

        let trim_symbols = hint - PRE_HINT_W0_SYMBOL_GUARD;
        let trim_bits = trim_symbols * RC1_SOFT_BITS_PER_SYMBOL;
        if self.soft_buf.len() < trim_bits {
            return 0;
        }

        self.drain_front(trim_bits);
        self.frame_hint_symbols = Some(PRE_HINT_W0_SYMBOL_GUARD);
        self.last_search_len = self.last_search_len.saturating_sub(trim_bits);
        trim_bits
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

        let hint = self.frame_hint_symbols;
        candidates.sort_by(|a, b| {
            b.repeat_pair_score
                .cmp(&a.repeat_pair_score)
                .then_with(|| {
                    let a_dist = hint.map(|h| a.sym_mod.abs_diff(h)).unwrap_or(usize::MAX);
                    let b_dist = hint.map(|h| b.sym_mod.abs_diff(h)).unwrap_or(usize::MAX);
                    a_dist.cmp(&b_dist)
                })
                .then_with(|| a.frame_idx.cmp(&b.frame_idx))
                .then_with(|| a.sym_mod.cmp(&b.sym_mod))
        });
        candidates
    }

    fn decode_candidates(ranked_candidates: &[ScoredFrameCandidate]) -> Vec<ScoredFrameCandidate> {
        let perfect_candidates: Vec<ScoredFrameCandidate> = ranked_candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.repeat_pair_score == REPEAT_PAIR_COUNT)
            .take(MAX_PERFECT_DECODE_CANDIDATES)
            .collect();
        if !perfect_candidates.is_empty() {
            return perfect_candidates;
        }

        ranked_candidates
            .iter()
            .copied()
            .take(MAX_NEAR_MISS_DECODE_CANDIDATES)
            .collect()
    }

    fn search_step(&mut self) -> Vec<SampleBlock> {
        let search_started = Instant::now();
        let mut stats = ReassemblyProbeStats::default();
        let mut trimmed_bits = self.trim_pre_hint_w0();

        // Need at least one full frame to try the first offset.
        if self.soft_buf.len() < SOFT_BITS_PER_FRAME {
            return Vec::new();
        }

        if self.last_search_len >= SOFT_BITS_PER_FRAME
            && self.soft_buf.len() < self.last_search_len + MIN_SEARCH_RETRY_BITS
        {
            return Vec::new();
        }

        self.search_calls += 1;

        let offsets_to_try = RC1_SYMBOLS_PER_FRAME.min(
            (self.soft_buf.len().saturating_sub(SOFT_BITS_PER_FRAME)) / RC1_SOFT_BITS_PER_SYMBOL
                + 1,
        );

        let rank_start = Instant::now();
        let ranked_candidates = self.ranked_frame_candidates(offsets_to_try);
        self.rank_us += rank_start.elapsed().as_micros() as u64;
        self.ranked_searches += 1;
        let ranked_candidates_scored = ranked_candidates.len();
        let decode_candidates = Self::decode_candidates(&ranked_candidates);
        let ranked_candidates_tried = decode_candidates.len();
        let max_pair_score = ranked_candidates
            .first()
            .map(|candidate| candidate.repeat_pair_score)
            .unwrap_or(0);
        let min_kept_pair_score = ranked_candidates
            .last()
            .map(|candidate| candidate.repeat_pair_score)
            .unwrap_or(0);
        let min_decode_pair_score = decode_candidates
            .last()
            .map(|candidate| candidate.repeat_pair_score)
            .unwrap_or(0);

        trace!(
            "frame_aligner: ranked search buffer_len={} offsets_to_try={} ranked_candidates_scored={} ranked_candidates_tried={} max_pair_score={} min_kept_pair_score={} min_decode_pair_score={}",
            self.soft_buf.len(),
            offsets_to_try,
            ranked_candidates_scored,
            ranked_candidates_tried,
            max_pair_score,
            min_kept_pair_score,
            min_decode_pair_score,
        );

        for candidate in decode_candidates {
            let log_detail = candidate.sym_mod < 3;
            if let Some(success) = self.try_reassembly_at_offset(
                candidate.reassembly_bit_offset,
                log_detail,
                &mut stats,
            ) {
                info!(
                    "frame_aligner: locked at symbol_offset={} candidate_frame_idx={} completion_frame_idx={} scored_bit_offset={} chip_start={} repeat_pair_score={}/{} collapsed_ones={} max_pair_score={}",
                    candidate.sym_mod,
                    candidate.frame_idx,
                    success.frame_idx,
                    candidate.scored_bit_offset,
                    self.chip_start
                        + (candidate.reassembly_bit_offset / RC1_SOFT_BITS_PER_SYMBOL)
                            * PN_CHIPS_PER_SYMBOL,
                    candidate.repeat_pair_score,
                    REPEAT_PAIR_COUNT,
                    candidate.collapsed_ones,
                    max_pair_score,
                );
                self.drain_front(candidate.reassembly_bit_offset);
                self.state = FrameAlignerState::Locked;
                self.tags.insert("access_frame_aligned", 1);
                self.last_search_len = 0;
                return self.emit_frames();
            }
        }

        self.last_search_len = self.soft_buf.len();

        // No lock yet. If the buffer is getting large, trim the oldest frame
        // so the search window advances.
        if self.soft_buf.len() > MAX_SEARCH_BUFFER {
            trace!(
                "frame_aligner: trimming {} bits from search buffer (total={})",
                SOFT_BITS_PER_FRAME,
                self.soft_buf.len()
            );
            trimmed_bits = SOFT_BITS_PER_FRAME;
            self.drain_front(SOFT_BITS_PER_FRAME);
            self.last_search_len = 0;
        }

        self.total_hard_checks += stats.hard_checks as u64;

        let elapsed_ms = search_started.elapsed().as_millis();
        let force_diag = self.frame_aligner_diag;
        if force_diag || elapsed_ms >= SEARCH_DIAG_LOG_THRESHOLD_MS {
            info!(
                "frame_aligner: ranked_search_diag chip_start={} buffer_len={} hint={:?} offsets_scored={} ranked_candidates_scored={} ranked_candidates_tried={} repeat_pair_score_min={} max_pair_score={} min_kept_pair_score={} min_decode_pair_score={} frame_checks={} soft_checks={} hard_checks={} last_search_len={} trimmed_bits={} elapsed={}ms",
                self.chip_start,
                self.soft_buf.len(),
                self.frame_hint_symbols,
                offsets_to_try,
                ranked_candidates_scored,
                ranked_candidates_tried,
                REPEAT_PAIR_SCORE_MIN,
                max_pair_score,
                min_kept_pair_score,
                min_decode_pair_score,
                stats.frame_checks,
                stats.soft_checks,
                stats.hard_checks,
                self.last_search_len,
                trimmed_bits,
                elapsed_ms,
            );
        }

        Vec::new()
    }

    fn emit_frames(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.soft_buf.len() >= SOFT_BITS_PER_FRAME {
            let soft_snapshot: Vec<f32> = self
                .soft_buf
                .iter()
                .take(SOFT_BITS_PER_FRAME)
                .copied()
                .collect();
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

            let frame_soft: Vec<Complex32> = self
                .soft_buf
                .drain(..SOFT_BITS_PER_FRAME)
                .map(|v| Complex32::new(v, 0.0))
                .collect();
            let mut block = SampleBlock::new(frame_soft, self.chip_start)
                .with_sample_rate_hz(self.sample_rate_hz);
            block.tags = self.tags.clone();
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
            out.push(block);
            self.chip_start = self
                .chip_start
                .saturating_add(RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL);
        }
        self.last_search_len = 0;
        out
    }
}

impl PipelineProcessor for ReverseAccessFrameAligner {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.soft_buf.is_empty() {
            self.chip_start = block.chip_start;
            self.sample_rate_hz = block.sample_rate_hz;
            self.last_search_len = 0;
            self.frame_hint_symbols = None;
        }
        self.tags = block.tags;
        if self.frame_hint_symbols.is_none() {
            if let Some(hint) = self
                .tags
                .get("access_frame_hint_symbols")
                .copied()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v < RC1_SYMBOLS_PER_FRAME)
            {
                self.frame_hint_symbols = Some(hint);
            }
        }
        if matches!(self.state, FrameAlignerState::Locked) {
            self.tags.insert("access_frame_aligned", 1);
        }

        for s in &block.samples {
            self.soft_buf.push_back(s.re);
        }

        let t = Instant::now();
        let result = match self.state {
            FrameAlignerState::Searching => self.search_step(),
            FrameAlignerState::Locked => {
                let r = self.emit_frames();
                self.locked_us += t.elapsed().as_micros() as u64;
                return r;
            }
        };
        result
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        match self.state {
            FrameAlignerState::Searching => {
                self.soft_buf.clear();
                self.last_search_len = 0;
                Vec::new()
            }
            FrameAlignerState::Locked => self.emit_frames(),
        }
    }

    fn name(&self) -> &'static str {
        "ReverseAccessFrameAligner"
    }

    fn metrics(&self) -> Vec<(&'static str, String)> {
        let state = match self.state {
            FrameAlignerState::Searching => "searching",
            FrameAlignerState::Locked => "locked",
        };
        vec![
            ("state", state.to_string()),
            ("rank_ms", format!("{:.1}", self.rank_us as f64 / 1000.0)),
            (
                "locked_ms",
                format!("{:.1}", self.locked_us as f64 / 1000.0),
            ),
            ("search_calls", format!("{}", self.search_calls)),
            ("hard_checks", format!("{}", self.total_hard_checks)),
            ("ranked_searches", format!("{}", self.ranked_searches)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;
    use num_complex::Complex32;

    use super::{
        FrameAlignerState, PRE_HINT_W0_SYMBOL_GUARD, PipelineProcessor, RC1_SOFT_BITS_PER_SYMBOL,
        RC1_SYMBOLS_PER_FRAME, ReverseAccessFrameAligner, SOFT_BITS_PER_FRAME, SampleBlock,
    };
    use crate::lac::crc30;
    use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_576};
    use crate::phy::coding::convolutional::get_1_3_k9_encoder;
    use crate::phy::coding::symbol_repeat::SymbolRepetition;

    /// Build one frame of interleaved soft bits from a known-valid PDU.
    fn make_valid_frame_soft_bits() -> Vec<f32> {
        let pdu_bits: Vec<u8> = vec![
            0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let msg_length_octets: u8 = 10;

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&pdu_bits));
        let crc = crc30(&crc_scope);

        let mut sar_body = Bitstream::new();
        sar_body.write_u8(msg_length_octets, 8);
        sar_body.extend(&Bitstream::new_init(&pdu_bits));
        sar_body.write_u32(crc, 30);

        let mut frame_bits = sar_body.bits().to_vec();
        frame_bits.extend(std::iter::repeat(0u8).take(16));
        assert_eq!(96, frame_bits.len());

        let mut conv_enc = get_1_3_k9_encoder();
        let mut code_symbols: Vec<u8> = Vec::with_capacity(288);
        for &bit in &frame_bits {
            code_symbols.extend_from_slice(&conv_enc.encode(bit));
        }

        let mut sr = SymbolRepetition::new(2);
        for &sym in &code_symbols {
            sr.feed(sym);
        }
        let repeated = sr.take_all();
        assert_eq!(576, repeated.len());

        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
        let interleaved = interleaver.encode(&repeated);

        // Convert to soft bits: 0 → +1.0, 1 → -1.0
        interleaved
            .iter()
            .map(|&b| if b == 0 { 1.0f32 } else { -1.0f32 })
            .collect()
    }

    fn make_valid_multiframe_soft_bits(msg_length_octets: u8) -> Vec<Vec<f32>> {
        assert!(
            msg_length_octets >= 12,
            "need more than one 88-bit fragment"
        );
        let payload_bits_len = msg_length_octets as usize * 8 - 8 - 30;
        let payload_bits: Vec<u8> = (0..payload_bits_len)
            .map(|i| ((i * 5 + 3) % 11 >= 5) as u8)
            .collect();

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&payload_bits));
        let crc = crc30(&crc_scope);

        let mut sar_body = Bitstream::new();
        sar_body.write_u8(msg_length_octets, 8);
        sar_body.extend(&Bitstream::new_init(&payload_bits));
        sar_body.write_u32(crc, 30);

        let body_bits = sar_body.bits().to_vec();
        assert_eq!(msg_length_octets as usize * 8, body_bits.len());

        let mut frames = Vec::new();
        let mut rem = body_bits.as_slice();
        while !rem.is_empty() {
            let take = rem.len().min(88);
            let mut frame_bits = rem[..take].to_vec();
            if take < 88 {
                frame_bits.extend(std::iter::repeat(0u8).take(88 - take));
            }
            frame_bits.extend(std::iter::repeat(0u8).take(8));
            assert_eq!(96, frame_bits.len());

            let mut conv_enc = get_1_3_k9_encoder();
            let mut code_symbols: Vec<u8> = Vec::with_capacity(288);
            for &bit in &frame_bits {
                code_symbols.extend_from_slice(&conv_enc.encode(bit));
            }

            let mut sr = SymbolRepetition::new(2);
            for &sym in &code_symbols {
                sr.feed(sym);
            }
            let repeated = sr.take_all();
            assert_eq!(576, repeated.len());

            let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
            let interleaved = interleaver.encode(&repeated);
            frames.push(
                interleaved
                    .iter()
                    .map(|&b| if b == 0 { 1.0f32 } else { -1.0f32 })
                    .collect(),
            );
            rem = &rem[take..];
        }

        assert!(frames.len() >= 2);
        frames
    }

    #[test]
    fn frame_aligner_locks_on_aligned_input() {
        let frame = make_valid_frame_soft_bits();
        assert_eq!(SOFT_BITS_PER_FRAME, frame.len());

        // Feed exactly one frame — should lock at offset 0.
        let mut aligner = ReverseAccessFrameAligner::new();
        let block = SampleBlock::new(
            frame.iter().map(|&v| Complex32::new(v, 0.0)).collect(),
            1000,
        );
        let out = aligner.process_block(block);

        assert_eq!(aligner.state, FrameAlignerState::Locked);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), SOFT_BITS_PER_FRAME);
        assert_eq!(out[0].chip_start, 1000);
    }

    #[test]
    fn frame_aligner_locks_with_offset() {
        let frame = make_valid_frame_soft_bits();
        let prefix_symbols = 13usize;
        let prefix_bits = prefix_symbols * RC1_SOFT_BITS_PER_SYMBOL;

        // Prepend random-ish prefix of 13 symbols (78 bits).
        let mut soft = vec![0.5f32; prefix_bits];
        soft.extend(&frame);

        let mut aligner = ReverseAccessFrameAligner::new();
        let block = SampleBlock::new(soft.iter().map(|&v| Complex32::new(v, 0.0)).collect(), 0);
        let out = aligner.process_block(block);

        assert_eq!(aligner.state, FrameAlignerState::Locked);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), SOFT_BITS_PER_FRAME);
        // chip_start should skip past the prefix
        assert_eq!(out[0].chip_start, prefix_symbols * 256);
    }

    #[test]
    fn frame_aligner_stays_searching_on_noise() {
        let noise: Vec<f32> = (0..SOFT_BITS_PER_FRAME * 2)
            .map(|i| if i % 3 == 0 { 0.7 } else { -0.3 })
            .collect();

        let mut aligner = ReverseAccessFrameAligner::new();
        let block = SampleBlock::new(noise.iter().map(|&v| Complex32::new(v, 0.0)).collect(), 0);
        let out = aligner.process_block(block);

        assert_eq!(aligner.state, FrameAlignerState::Searching);
        assert!(out.is_empty());
    }

    #[test]
    fn frame_aligner_rejects_constant_repeat_pair_candidates() {
        // A W0/preamble-like window has perfect repeated-pair agreement after
        // deinterleaving, but it collapses to a constant all-zero/all-one frame
        // and must not reach Viterbi/CRC search.
        let constant = vec![1.0f32; SOFT_BITS_PER_FRAME * 2];
        let mut aligner = ReverseAccessFrameAligner::new();
        let out = aligner.process_block(SampleBlock::new(
            constant.iter().map(|&v| Complex32::new(v, 0.0)).collect(),
            0,
        ));

        assert_eq!(aligner.state, FrameAlignerState::Searching);
        assert!(out.is_empty());
        assert_eq!(aligner.total_hard_checks, 0);
        assert!(
            aligner
                .ranked_frame_candidates(RC1_SYMBOLS_PER_FRAME)
                .is_empty()
        );
    }

    #[test]
    fn frame_aligner_locks_on_multiframe_access_message() {
        let frames = make_valid_multiframe_soft_bits(20);
        let mut aligner = ReverseAccessFrameAligner::new();
        let mut out = Vec::new();

        for (idx, frame) in frames.iter().enumerate() {
            out.extend(aligner.process_block(SampleBlock::new(
                frame.iter().map(|&v| Complex32::new(v, 0.0)).collect(),
                idx * 96 * 256,
            )));
        }

        assert_eq!(aligner.state, FrameAlignerState::Locked);
        assert_eq!(out.len(), frames.len());
        assert_eq!(out[0].chip_start, 0);
    }

    #[test]
    fn frame_aligner_locks_on_walsh_roundtrip() {
        use crate::phy::walsh::WalshGenerator;
        use crate::receiver::pipelined::reverse_access_walsh_symbol_demod::ReverseAccessWalshSymbolDemodProcessor;

        // Build a valid frame, Walsh-modulate, Walsh-demodulate, then check CRC lock.
        let pdu_bits: Vec<u8> = vec![
            0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let msg_length_octets: u8 = 10;

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&pdu_bits));
        let crc = crc30(&crc_scope);

        let mut sar_body = Bitstream::new();
        sar_body.write_u8(msg_length_octets, 8);
        sar_body.extend(&Bitstream::new_init(&pdu_bits));
        sar_body.write_u32(crc, 30);

        let mut frame_bits = sar_body.bits().to_vec();
        frame_bits.extend(std::iter::repeat(0u8).take(16));
        assert_eq!(96, frame_bits.len());

        let mut conv_enc = get_1_3_k9_encoder();
        let mut code_symbols: Vec<u8> = Vec::with_capacity(288);
        for &bit in &frame_bits {
            code_symbols.extend_from_slice(&conv_enc.encode(bit));
        }

        let mut sr = SymbolRepetition::new(2);
        for &sym in &code_symbols {
            sr.feed(sym);
        }
        let repeated = sr.take_all();
        assert_eq!(576, repeated.len());

        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
        let interleaved = interleaver.encode(&repeated);

        // Walsh-modulate: group 6 interleaved bits → Walsh row
        let walsh_matrix = WalshGenerator::generate_matrix::<64>();
        let mut walsh_symbols: Vec<Vec<Complex32>> = Vec::new();
        for group in interleaved.chunks_exact(6) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            let mut symbol = Vec::with_capacity(256);
            for &chip in &walsh_matrix[index] {
                for _ in 0..4 {
                    symbol.push(Complex32::new(chip as f32, 0.0));
                }
            }
            walsh_symbols.push(symbol);
        }
        assert_eq!(96, walsh_symbols.len());

        // Walsh-demodulate using the actual processor
        let mut demod =
            ReverseAccessWalshSymbolDemodProcessor::with_output_bits(SOFT_BITS_PER_FRAME);
        let mut demod_out = Vec::new();
        for (idx, symbol) in walsh_symbols.iter().enumerate() {
            let block = SampleBlock::new(symbol.clone(), idx * 256);
            demod_out.extend(demod.process_block(block));
        }
        demod_out.extend(demod.flush());

        assert_eq!(1, demod_out.len(), "demod should emit one 576-bit block");
        assert_eq!(SOFT_BITS_PER_FRAME, demod_out[0].samples.len());

        // Feed into frame aligner — should lock via CRC at offset 0.
        let mut aligner = ReverseAccessFrameAligner::new();
        let out = aligner.process_block(demod_out.remove(0));

        assert_eq!(
            aligner.state,
            FrameAlignerState::Locked,
            "frame aligner should CRC-lock on Walsh-demodulated frame"
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), SOFT_BITS_PER_FRAME);
    }

    #[test]
    fn debug_expected_walsh_rows() {
        // Must match the exact PDU bits from test_generic_rake_access_channel
        let pdu_bits: Vec<u8> = vec![
            0, 0, // PD = 00
            0, 0, 0, 0, 0, 1, // MSG_ID = 000001 (Registration)
            0, 0, 0, 1, // REG_TYPE = 0001 (power-up)
            0, 0, 0, // SLOT_CYCLE_INDEX = 000
            0, 0, 0, 0, 0, 1, 1, 0, // MOB_P_REV = 00000110
            0, 0, 1, 0, 0, 0, 0, 0, // SCM = 00100000
            1, // MOB_TERM = 1
            0, 0, 0, 0, // RETURN_CAUSE = 0000
            0, 0, 0, 0, 0, 0, // padding to 42 bits
        ];
        let msg_length_octets: u8 = 10;

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&pdu_bits));
        let crc = crc30(&crc_scope);

        let mut sar_body = Bitstream::new();
        sar_body.write_u8(msg_length_octets, 8);
        sar_body.extend(&Bitstream::new_init(&pdu_bits));
        sar_body.write_u32(crc, 30);

        let mut frame_bits = sar_body.bits().to_vec();
        frame_bits.extend(std::iter::repeat(0u8).take(16));
        assert_eq!(96, frame_bits.len());

        eprintln!("frame_bits[0..8] (msg_length=10): {:?}", &frame_bits[..8]);

        let mut conv_enc = get_1_3_k9_encoder();
        let mut code_symbols: Vec<u8> = Vec::with_capacity(288);
        for &bit in &frame_bits {
            code_symbols.extend_from_slice(&conv_enc.encode(bit));
        }

        let mut sr = SymbolRepetition::new(2);
        for &sym in &code_symbols {
            sr.feed(sym);
        }
        let repeated = sr.take_all();

        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
        let interleaved = interleaver.encode(&repeated);

        eprintln!("First 10 Walsh rows (expected from TX):");
        for (i, group) in interleaved.chunks_exact(6).enumerate().take(10) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            eprintln!("  sym {}: bits={:?} → row={}", i, group, index);
        }

        let hard_24: Vec<u8> = interleaved.iter().take(24).copied().collect();
        eprintln!("First 24 interleaved bits: {:?}", hard_24);
        eprintln!(
            "Trace from real chain:     [1,1,0,0,0,0, 1,0,0,0,1,0, 0,1,1,1,0,0, 0,0,1,0,1,0]"
        );
    }

    #[test]
    fn frame_aligner_emits_multiple_frames_after_lock() {
        let frame = make_valid_frame_soft_bits();

        // Two consecutive valid frames.
        let mut soft = frame.clone();
        soft.extend(&frame);

        let mut aligner = ReverseAccessFrameAligner::new();
        let block = SampleBlock::new(soft.iter().map(|&v| Complex32::new(v, 0.0)).collect(), 0);
        let out = aligner.process_block(block);

        assert_eq!(aligner.state, FrameAlignerState::Locked);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].samples.len(), SOFT_BITS_PER_FRAME);
        assert_eq!(out[1].samples.len(), SOFT_BITS_PER_FRAME);
    }

    #[test]
    fn frame_aligner_trusts_walsh_hint_without_full_search() {
        let frame = make_valid_frame_soft_bits();
        let prefix_symbols = 29usize;
        let prefix_bits = prefix_symbols * RC1_SOFT_BITS_PER_SYMBOL;

        let mut soft = vec![0.25f32; prefix_bits];
        soft.extend(&frame);

        let mut aligner = ReverseAccessFrameAligner::new();
        let mut block = SampleBlock::new(soft.iter().map(|&v| Complex32::new(v, 0.0)).collect(), 0);
        block
            .tags
            .insert("access_frame_hint_symbols", prefix_symbols as i64);
        let out = aligner.process_block(block);

        assert_eq!(aligner.state, FrameAlignerState::Locked);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chip_start, prefix_symbols * 256);
        assert_eq!(out[0].samples.len(), SOFT_BITS_PER_FRAME);
    }

    #[test]
    fn frame_aligner_trims_w0_history_before_hint_boundary() {
        let prefix_symbols = 29usize;
        let prefix_bits = prefix_symbols * RC1_SOFT_BITS_PER_SYMBOL;
        let mut soft = vec![0.25f32; prefix_bits];
        soft.extend(std::iter::repeat(0.1f32).take(RC1_SOFT_BITS_PER_SYMBOL * 8));

        let mut aligner = ReverseAccessFrameAligner::new();
        let mut block = SampleBlock::new(soft.iter().map(|&v| Complex32::new(v, 0.0)).collect(), 0);
        block
            .tags
            .insert("access_frame_hint_symbols", prefix_symbols as i64);
        let out = aligner.process_block(block);

        assert!(out.is_empty());
        assert_eq!(aligner.frame_hint_symbols, Some(PRE_HINT_W0_SYMBOL_GUARD));
        assert_eq!(
            aligner.chip_start,
            (prefix_symbols - PRE_HINT_W0_SYMBOL_GUARD) * 256
        );
        assert_eq!(
            aligner.soft_buf.len(),
            PRE_HINT_W0_SYMBOL_GUARD * RC1_SOFT_BITS_PER_SYMBOL + RC1_SOFT_BITS_PER_SYMBOL * 8
        );
    }
}
