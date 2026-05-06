use std::collections::{HashMap, VecDeque};

use cdma_common::bits::Bitstream;
use log::{info, trace};
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};
use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_576};
use crate::phy::coding::convolutional::get_1_3_k9_viterbi_decoder;
use crate::phy::walsh::WalshGenerator;
use crate::receiver::access::AccessFrameReader;
use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME,
    RC1_WALSH_CHIPS_PER_SYMBOL,
};

const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;
const DEFAULT_MIN_PREAMBLE_SYMBOLS: usize = 8;
const W0_ENERGY_RATIO_MIN: f32 = 0.15;
const COARSE_W0_ENERGY_RATIO_MIN: f32 = 0.22;
const COARSE_POST_SYMBOLS: usize = 32;
const ACCESS_COARSE_MIN_POST_NON_W0_SYMBOLS: usize = 1;
const ACCESS_COARSE_MIN_NON_W0_RUN: usize = 1;
const ACCESS_FINE_MIN_POST_NON_W0_SYMBOLS: usize = 4;
const ACCESS_FINE_MIN_NON_W0_RUN: usize = 4;
const TRAFFIC_COARSE_MIN_POST_NON_W0_SYMBOLS: usize = 1;
const TRAFFIC_COARSE_MIN_NON_W0_RUN: usize = 1;
const TRAFFIC_FINE_MIN_POST_NON_W0_SYMBOLS: usize = 2;
const TRAFFIC_FINE_MIN_NON_W0_RUN: usize = 2;
// Search far enough back from the live buffer tail to include the first
// post-preamble data transition of a normal access burst. The reverse access
// message capsule can occupy up to 10 frames = 960 symbols, so 1024 symbols
// comfortably spans "rest of burst after first non-W0" without scanning the
// full 3072-symbol window on every update.
const COARSE_SEARCH_TAIL_SYMBOLS: usize = 1024;
/// Threshold for the sliding raw-chip W0 detector.  This metric does NOT
/// benefit from 4-chip Walsh-chip grouping, so it is much lower than
/// `W0_ENERGY_RATIO_MIN` at the same SNR.  Noise gives ~0.004; even a
/// weak W0 signal at SNR ≈ 0.08 gives ~0.06.
/// Maximum number of symbols the sliding window will hold.
/// Real reverse-link access bursts in our captures can carry well over
/// 700 preamble symbols before the first non-W0 data symbol appears.
/// Keep enough history to preserve that first transition instead of
/// trimming it out of the buffer before it enters view.
const MAX_WINDOW_SYMBOLS: usize = 3072;
const ACCESS_SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const ACCESS_PROBE_MAX_FRAMES: usize = 10;
const ACCESS_PROBE_TOP_CANDIDATES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlignerState {
    Acquiring,
    Locked,
}

#[derive(Clone, Copy, Debug)]
struct SymbolMetrics {
    peak_row: usize,
    peak_ratio: f32,
    margin_ratio: f32,
}

#[derive(Clone, Debug)]
struct TransitionMetrics {
    pre_w0: usize,
    post_non_w0: usize,
    longest_non_w0_run: usize,
    first_non_w0_symbol_offset: usize,
    avg_post_peak_ratio: f32,
    first_non_w0_row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReverseAccessWalshAlignerLockConfig {
    coarse_min_post_non_w0_symbols: usize,
    coarse_min_non_w0_run: usize,
    fine_min_post_non_w0_symbols: usize,
    fine_min_non_w0_run: usize,
    lock_on_preamble_only: bool,
    search_full_buffer: bool,
}

impl ReverseAccessWalshAlignerLockConfig {
    pub const fn access() -> Self {
        Self {
            coarse_min_post_non_w0_symbols: ACCESS_COARSE_MIN_POST_NON_W0_SYMBOLS,
            coarse_min_non_w0_run: ACCESS_COARSE_MIN_NON_W0_RUN,
            fine_min_post_non_w0_symbols: ACCESS_FINE_MIN_POST_NON_W0_SYMBOLS,
            fine_min_non_w0_run: ACCESS_FINE_MIN_NON_W0_RUN,
            lock_on_preamble_only: false,
            search_full_buffer: false,
        }
    }

    pub const fn traffic() -> Self {
        Self {
            coarse_min_post_non_w0_symbols: TRAFFIC_COARSE_MIN_POST_NON_W0_SYMBOLS,
            coarse_min_non_w0_run: TRAFFIC_COARSE_MIN_NON_W0_RUN,
            fine_min_post_non_w0_symbols: TRAFFIC_FINE_MIN_POST_NON_W0_SYMBOLS,
            fine_min_non_w0_run: TRAFFIC_FINE_MIN_NON_W0_RUN,
            // A pure RC1 preamble is ambiguous at arbitrary chip offsets after
            // long-code despreading; traffic mode still needs the first
            // preamble-to-data transition to pin the 256-chip symbol boundary.
            lock_on_preamble_only: false,
            // Reverse traffic can carry a long preamble before the first
            // useful non-W0 symbols. Keep the earliest transition candidate in
            // view instead of searching only the most recent access-like tail.
            search_full_buffer: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PreambleAlignmentMetrics {
    w0_symbols: usize,
    avg_peak_ratio: f32,
    avg_margin_ratio: f32,
}

/// Pre-computed Walsh-chip partial sums over a contiguous sample region.
/// Each entry `[j]` holds the sum of `RC1_PN_CHIPS_PER_WALSH_CHIP` consecutive
/// samples starting at sample index `start + j`, enabling `metrics_at` to
/// skip the inner 4-sample accumulation loop (≈4× fewer operations).
struct WcCache {
    re: Vec<f32>,
    im: Vec<f32>,
    start: usize,
}

/// Finds the preamble→data transition and establishes Walsh symbol alignment.
///
/// Input blocks from the preamble detector are **not** symbol-aligned — they
/// arrive at arbitrary chip boundaries.  This processor accumulates samples,
/// detects the W0→non-W0 transition, and emits symbol-aligned 256-chip blocks
/// from the transition point onward.
pub struct ReverseAccessWalshAligner {
    min_preamble_symbols: usize,
    state: AlignerState,
    /// Set once we've observed enough consecutive W0-high chip offsets.
    /// Survives buffer trimming so long preambles don't reset progress.
    preamble_confirmed: bool,
    /// Symbol index where the confirmed preamble run began, relative to the
    /// current symbol buffer.
    preamble_confirmed_symbol_start: Option<usize>,
    /// Next coarse-search symbol index to evaluate. Advanced incrementally so
    /// we do not rescan the entire buffered preamble/data history on every
    /// block after preamble confirmation.
    coarse_search_cursor_sym: usize,
    /// Sliding sample buffer. Grows as blocks arrive; trimmed from the front
    /// when it exceeds `MAX_WINDOW_SYMBOLS * PN_CHIPS_PER_SYMBOL`.
    samples: VecDeque<Complex32>,
    symbol_metrics: VecDeque<SymbolMetrics>,
    chip_start: Option<usize>,
    sample_rate_hz: f64,
    oversample: usize,
    tags: HashMap<&'static str, i64>,
    /// Temporary cache of pre-computed Walsh-chip partial sums used during
    /// the fine search to avoid redundant inner-loop computation.
    wc_cache: Option<WcCache>,
    lock_config: ReverseAccessWalshAlignerLockConfig,
    best_rejected_transition: Option<(usize, TransitionMetrics)>,
    acquiring_us: u64,
    locked_us: u64,
    process_calls: u64,
}

impl ReverseAccessWalshAligner {
    pub fn new() -> Self {
        Self::with_min_preamble_symbols(DEFAULT_MIN_PREAMBLE_SYMBOLS)
    }

    pub fn with_min_preamble_symbols(min_preamble_symbols: usize) -> Self {
        assert!(min_preamble_symbols > 0, "min_preamble_symbols must be > 0");
        Self {
            min_preamble_symbols,
            state: AlignerState::Acquiring,
            preamble_confirmed: false,
            preamble_confirmed_symbol_start: None,
            coarse_search_cursor_sym: 0,
            samples: VecDeque::new(),
            symbol_metrics: VecDeque::new(),
            chip_start: None,
            sample_rate_hz: 0.0,
            oversample: 1,
            tags: HashMap::new(),
            wc_cache: None,
            lock_config: ReverseAccessWalshAlignerLockConfig::access(),
            best_rejected_transition: None,
            acquiring_us: 0,
            locked_us: 0,
            process_calls: 0,
        }
    }

    pub fn with_lock_config(mut self, lock_config: ReverseAccessWalshAlignerLockConfig) -> Self {
        self.lock_config = lock_config;
        self
    }

    fn block_oversample(block: &SampleBlock) -> usize {
        block
            .tags
            .get("access_oversample")
            .copied()
            .map(|v| v.max(1) as usize)
            .unwrap_or(1)
    }

    fn symbol_sample_len(&self) -> usize {
        PN_CHIPS_PER_SYMBOL * self.oversample
    }

    fn ingest_block(&mut self, block: SampleBlock) {
        if self.chip_start.is_none() {
            self.chip_start = Some(block.chip_start);
            self.sample_rate_hz = block.sample_rate_hz;
            self.oversample = Self::block_oversample(&block);
        }
        self.tags = block.tags;
        self.samples.extend(block.samples);
        self.refresh_symbol_metrics();
    }

    fn refresh_symbol_metrics(&mut self) {
        let available_symbols = self.samples.len() / self.symbol_sample_len();
        while self.symbol_metrics.len() < available_symbols {
            let offset = self.symbol_metrics.len() * self.symbol_sample_len();
            match self.metrics_at(offset) {
                Some(metrics) => self.symbol_metrics.push_back(metrics),
                None => break,
            }
        }
    }

    /// Build a Walsh-chip partial-sum cache covering `[start, end)` in
    /// `self.samples`.  Each entry `re[j]`/`im[j]` holds the sum of
    /// `RC1_PN_CHIPS_PER_WALSH_CHIP` consecutive samples starting at
    /// sample index `start + j`.
    fn build_wc_cache(&self, start: usize, end: usize) -> WcCache {
        let os = self.oversample;
        let raw_len = end - start;
        let wcs_count = raw_len.saturating_sub((RC1_PN_CHIPS_PER_WALSH_CHIP - 1) * os);
        let mut re = vec![0.0f32; wcs_count];
        let mut im = vec![0.0f32; wcs_count];
        for j in 0..wcs_count {
            let base = start + j;
            let mut r = 0.0f32;
            let mut i = 0.0f32;
            for pn in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                let s = self.samples[base + pn * os];
                r += s.re;
                i += s.im;
            }
            re[j] = r;
            im[j] = i;
        }
        WcCache { re, im, start }
    }

    fn metrics_at(&self, offset: usize) -> Option<SymbolMetrics> {
        let symbol_len = self.symbol_sample_len();
        if self.samples.len() < offset + symbol_len {
            return None;
        }
        let os = self.oversample;

        // Check if the pre-computed Walsh-chip cache covers this offset.
        let cache_hit = self.wc_cache.as_ref().and_then(|c| {
            let local = offset.checked_sub(c.start)?;
            let max_j =
                local + (RC1_WALSH_CHIPS_PER_SYMBOL - 1) * RC1_PN_CHIPS_PER_WALSH_CHIP * os + os
                    - 1;
            if max_j < c.re.len() {
                Some((c, local))
            } else {
                None
            }
        });

        let mut best: Option<SymbolMetrics> = None;
        for phase in 0..os {
            // Accumulate 64 Walsh-chip bins (each is the sum of 4 PN-chip
            // repetitions), then run a Fast Walsh-Hadamard Transform to get
            // all 64 row correlations in O(N log N) instead of O(N²).
            let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];

            if let Some((cache, local)) = cache_hit {
                for wc in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
                    let j = local + wc * RC1_PN_CHIPS_PER_WALSH_CHIP * os + phase;
                    walsh_chips[wc] = Complex32::new(cache.re[j], cache.im[j]);
                }
            } else {
                for wc in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
                    let base = offset + wc * RC1_PN_CHIPS_PER_WALSH_CHIP * os;
                    let mut acc = Complex32::new(0.0, 0.0);
                    for pn in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                        acc += self.samples[base + pn * os + phase];
                    }
                    walsh_chips[wc] = acc;
                }
            }

            WalshGenerator::fwht_fixed(&mut walsh_chips);

            let energies: [f32; RC1_WALSH_CHIPS_PER_SYMBOL] =
                std::array::from_fn(|i| walsh_chips[i].norm_sqr());

            let total_energy: f32 = energies.iter().sum();
            if total_energy <= 1e-9 {
                continue;
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
            if best_row == 0 {
                second_energy = energies
                    .iter()
                    .enumerate()
                    .skip(1)
                    .map(|(_, &energy)| energy)
                    .fold(0.0f32, f32::max);
            }

            let metrics = SymbolMetrics {
                peak_row: best_row,
                peak_ratio: best_energy / total_energy,
                margin_ratio: best_energy / second_energy.max(1e-9),
            };
            match best {
                Some(prev)
                    if prev.peak_ratio > metrics.peak_ratio
                        || (prev.peak_ratio == metrics.peak_ratio
                            && prev.margin_ratio >= metrics.margin_ratio) => {}
                _ => best = Some(metrics),
            }
        }

        best
    }

    fn symbol_energies_at(
        &self,
        offset: usize,
        phase: usize,
    ) -> Option<[f32; RC1_WALSH_CHIPS_PER_SYMBOL]> {
        let symbol_len = self.symbol_sample_len();
        if self.samples.len() < offset + symbol_len {
            return None;
        }
        let os = self.oversample;
        let cache_hit = self.wc_cache.as_ref().and_then(|c| {
            let local = offset.checked_sub(c.start)?;
            let max_j =
                local + (RC1_WALSH_CHIPS_PER_SYMBOL - 1) * RC1_PN_CHIPS_PER_WALSH_CHIP * os + phase;
            if max_j < c.re.len() {
                Some((c, local))
            } else {
                None
            }
        });

        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        if let Some((cache, local)) = cache_hit {
            for wc in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
                let j = local + wc * RC1_PN_CHIPS_PER_WALSH_CHIP * os + phase;
                walsh_chips[wc] = Complex32::new(cache.re[j], cache.im[j]);
            }
        } else {
            for wc in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
                let base = offset + wc * RC1_PN_CHIPS_PER_WALSH_CHIP * os;
                let mut acc = Complex32::new(0.0, 0.0);
                for pn in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                    acc += self.samples[base + pn * os + phase];
                }
                walsh_chips[wc] = acc;
            }
        }
        WalshGenerator::fwht_fixed(&mut walsh_chips);
        let energies = std::array::from_fn(|i| walsh_chips[i].norm_sqr());
        Some(energies)
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

    fn peak_metrics_from_energies(
        energies: &[f32; RC1_WALSH_CHIPS_PER_SYMBOL],
    ) -> (usize, f32, f32) {
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

    fn demod_soft_bits_at_phase(
        &self,
        offset: usize,
        phase: usize,
    ) -> Option<[f32; RC1_SOFT_BITS_PER_SYMBOL]> {
        let energies = self.symbol_energies_at(offset, phase)?;
        Some(Self::soft_bits_from_energies(&energies))
    }

    fn demod_soft_bits_at(&self, offset: usize) -> Option<[f32; RC1_SOFT_BITS_PER_SYMBOL]> {
        let os = self.oversample;
        let mut best_peak_ratio = f32::NEG_INFINITY;
        let mut best_margin = f32::NEG_INFINITY;
        let mut best_soft = [0.0f32; RC1_SOFT_BITS_PER_SYMBOL];

        for phase in 0..os {
            let energies = self.symbol_energies_at(offset, phase)?;
            let (_, peak_ratio, margin) = Self::peak_metrics_from_energies(&energies);
            if peak_ratio > best_peak_ratio
                || (peak_ratio == best_peak_ratio && margin > best_margin)
            {
                best_peak_ratio = peak_ratio;
                best_margin = margin;
                best_soft = Self::soft_bits_from_energies(&energies);
            }
        }

        Some(best_soft)
    }

    fn decode_access_hard_bits(soft_frame: &[f32]) -> Vec<u8> {
        let interleaver = BitReversalInterleaver::new(SR1_PARAMS_576);
        let deinterleaved = interleaver.decode_soft(soft_frame);
        let collapsed: Vec<f32> = deinterleaved
            .chunks_exact(2)
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect();

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

    fn probe_access_decode_score(
        &self,
        candidate_offset: usize,
    ) -> Option<(usize, usize, Option<usize>)> {
        let sym_len = self.symbol_sample_len();
        if self.samples.len() < candidate_offset + sym_len * RC1_SYMBOLS_PER_FRAME {
            return None;
        }

        let available_symbols = ((self.samples.len() - candidate_offset) / sym_len)
            .min(RC1_SYMBOLS_PER_FRAME * ACCESS_PROBE_MAX_FRAMES);
        if available_symbols < RC1_SYMBOLS_PER_FRAME {
            return None;
        }

        let offsets_to_try = RC1_SYMBOLS_PER_FRAME.min(
            available_symbols
                .saturating_sub(RC1_SYMBOLS_PER_FRAME)
                .saturating_add(1),
        );
        let mut best: Option<(usize, usize, Option<usize>)> = None;

        for fixed_phase in (0..self.oversample).map(Some).chain(std::iter::once(None)) {
            let mut soft_bits = Vec::with_capacity(available_symbols * RC1_SOFT_BITS_PER_SYMBOL);
            for sym_idx in 0..available_symbols {
                let sym_offset = candidate_offset + sym_idx * sym_len;
                let soft = match fixed_phase {
                    Some(phase) => self.demod_soft_bits_at_phase(sym_offset, phase)?,
                    None => self.demod_soft_bits_at(sym_offset)?,
                };
                soft_bits.extend_from_slice(&soft);
            }

            for sym_offset in 0..offsets_to_try {
                let bit_offset = sym_offset * RC1_SOFT_BITS_PER_SYMBOL;
                let available_frames = ((soft_bits.len().saturating_sub(bit_offset))
                    / ACCESS_SOFT_BITS_PER_FRAME)
                    .min(ACCESS_PROBE_MAX_FRAMES);
                if available_frames == 0 {
                    continue;
                }

                let mut reader = AccessFrameReader::new();
                let mut crc_valid_frames = 0usize;
                let mut parsed_frames = 0usize;
                for frame_idx in 0..available_frames {
                    let frame_bit_offset = bit_offset + frame_idx * ACCESS_SOFT_BITS_PER_FRAME;
                    let frame_soft =
                        &soft_bits[frame_bit_offset..frame_bit_offset + ACCESS_SOFT_BITS_PER_FRAME];
                    let bits = Self::decode_access_hard_bits(frame_soft);
                    if bits.len() < 96 {
                        continue;
                    }
                    let mut fragment = Bitstream::new_init(&bits[..88]);
                    if let Ok(Some(frame)) = reader.process(&mut fragment) {
                        parsed_frames += 1;
                        if frame.crc_valid {
                            crc_valid_frames += 1;
                        }
                    }
                }

                let score = crc_valid_frames * 100 + parsed_frames * 10;
                if score == 0 {
                    continue;
                }
                match best {
                    Some((best_score, best_offset, best_phase))
                        if best_score > score
                            || (best_score == score
                                && (best_offset < sym_offset
                                    || (best_offset == sym_offset
                                        && best_phase.is_none()
                                        && fixed_phase.is_some()))) => {}
                    _ => best = Some((score, sym_offset, fixed_phase)),
                }
            }
        }

        best
    }

    fn push_top_transition_candidate(
        candidates: &mut Vec<(usize, TransitionMetrics)>,
        candidate: (usize, TransitionMetrics),
    ) {
        if candidates.iter().any(|(offset, _)| *offset == candidate.0) {
            return;
        }
        candidates.push(candidate);
        candidates.sort_by(Self::transition_better);
        candidates.truncate(ACCESS_PROBE_TOP_CANDIDATES);
    }

    fn chunk_from_offset(&self, offset: usize) -> SampleBlock {
        let chip_start = self.chip_start.unwrap_or(0) + offset / self.oversample;
        let samples = self
            .samples
            .iter()
            .skip(offset)
            .take(self.symbol_sample_len())
            .copied()
            .collect::<Vec<_>>();
        let mut block =
            SampleBlock::new(samples, chip_start).with_sample_rate_hz(self.sample_rate_hz);
        block.tags = self.tags.clone();
        block
            .tags
            .insert("access_oversample", self.oversample as i64);
        block
    }

    fn discard_front(&mut self, count: usize) {
        let n = count.min(self.samples.len());
        let symbol_len = self.symbol_sample_len();
        let symbols_removed = n / symbol_len;
        self.samples.drain(..n);
        self.chip_start = self
            .chip_start
            .map(|start| start + n / self.oversample.max(1));
        if n % symbol_len == 0 {
            self.symbol_metrics
                .drain(..symbols_removed.min(self.symbol_metrics.len()));
            if let Some(sym_start) = self.preamble_confirmed_symbol_start.as_mut() {
                *sym_start = sym_start.saturating_sub(symbols_removed);
            }
            self.coarse_search_cursor_sym = self
                .coarse_search_cursor_sym
                .saturating_sub(symbols_removed);
        } else {
            self.symbol_metrics.clear();
            if let Some(sym_start) = self.preamble_confirmed_symbol_start.as_mut() {
                *sym_start = sym_start.saturating_sub(symbols_removed);
            }
            self.coarse_search_cursor_sym = self
                .coarse_search_cursor_sym
                .saturating_sub(symbols_removed);
            self.refresh_symbol_metrics();
        }
    }

    fn is_w0_like(&self, metrics: SymbolMetrics) -> bool {
        metrics.peak_row == 0 && metrics.peak_ratio >= COARSE_W0_ENERGY_RATIO_MIN
    }

    fn is_non_w0_like(&self, metrics: SymbolMetrics) -> bool {
        metrics.peak_row != 0 && metrics.peak_ratio >= W0_ENERGY_RATIO_MIN
    }

    /// Cheap W0-vs-not classifier at a single offset.  Only computes W0
    /// energy and total energy (skipping the other 63 Walsh rows), making it
    /// ~64× faster than a full `metrics_at` call.  Returns `Some(true)` for
    /// W0-like, `Some(false)` for non-W0-like, `None` if insufficient data.
    fn is_w0_fast(&self, offset: usize) -> Option<bool> {
        let symbol_len = self.symbol_sample_len();
        if self.samples.len() < offset + symbol_len {
            return None;
        }
        let os = self.oversample;

        let mut best_w0_ratio = 0.0f32;
        for phase in 0..os {
            let mut w0_re = 0.0f32;
            let mut w0_im = 0.0f32;
            let mut total_energy = 0.0f32;
            // W0 row is all +1, so W0 correlation = sum of all Walsh chips.
            for wc in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
                let mut chip_re = 0.0f32;
                let mut chip_im = 0.0f32;
                let base = offset + wc * RC1_PN_CHIPS_PER_WALSH_CHIP * os;
                for pn in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                    let s = self.samples[base + pn * os + phase];
                    chip_re += s.re;
                    chip_im += s.im;
                }
                w0_re += chip_re;
                w0_im += chip_im;
                total_energy += chip_re * chip_re + chip_im * chip_im;
            }
            let w0_energy = w0_re * w0_re + w0_im * w0_im;
            // Total energy across all 64 rows equals sum of per-Walsh-chip
            // energies (Parseval), so we already have it.
            if total_energy > 1e-9 {
                let ratio = w0_energy / total_energy;
                if ratio > best_w0_ratio {
                    best_w0_ratio = ratio;
                }
            }
        }
        Some(best_w0_ratio >= COARSE_W0_ENERGY_RATIO_MIN)
    }

    fn transition_metrics_at(
        &self,
        offset: usize,
        pre_symbols: usize,
        post_symbols: usize,
    ) -> Option<TransitionMetrics> {
        let sym_len = self.symbol_sample_len();
        if offset < pre_symbols * sym_len || offset + post_symbols * sym_len > self.samples.len() {
            return None;
        }

        // Use cheap W0-only check for pre-symbols (we only count W0 hits).
        let mut pre_w0 = 0usize;
        for idx in 0..pre_symbols {
            let sym_offset = offset - (pre_symbols - idx) * sym_len;
            if self.is_w0_fast(sym_offset) == Some(true) {
                pre_w0 += 1;
            }
        }

        // Use full metrics_at for post-symbols (need to confirm non-W0).
        let mut post_non_w0 = 0usize;
        let mut longest_non_w0_run = 0usize;
        let mut cur_run = 0usize;
        let mut peak_sum = 0.0f32;
        let mut first_non_w0_row = 0usize;
        let mut first_non_w0_symbol_offset = post_symbols;
        for idx in 0..post_symbols {
            let sym_offset = offset + idx * sym_len;
            let metrics = self.metrics_at(sym_offset)?;
            peak_sum += metrics.peak_ratio;
            if self.is_non_w0_like(metrics) {
                post_non_w0 += 1;
                cur_run += 1;
                longest_non_w0_run = longest_non_w0_run.max(cur_run);
                if first_non_w0_row == 0 {
                    first_non_w0_row = metrics.peak_row;
                }
                if first_non_w0_symbol_offset == post_symbols {
                    first_non_w0_symbol_offset = idx;
                }
            } else {
                cur_run = 0;
            }
        }

        Some(TransitionMetrics {
            pre_w0,
            post_non_w0,
            longest_non_w0_run,
            first_non_w0_symbol_offset,
            avg_post_peak_ratio: peak_sum / post_symbols as f32,
            first_non_w0_row,
        })
    }

    fn transition_metrics_at_symbol(
        &self,
        sym_idx: usize,
        pre_symbols: usize,
        post_symbols: usize,
    ) -> Option<TransitionMetrics> {
        if sym_idx < pre_symbols || sym_idx + post_symbols > self.symbol_metrics.len() {
            return None;
        }

        let mut pre_w0 = 0usize;
        for idx in (sym_idx - pre_symbols)..sym_idx {
            if self.is_w0_like(self.symbol_metrics[idx]) {
                pre_w0 += 1;
            }
        }

        let mut post_non_w0 = 0usize;
        let mut longest_non_w0_run = 0usize;
        let mut cur_run = 0usize;
        let mut peak_sum = 0.0f32;
        let mut first_non_w0_row = 0usize;
        let mut first_non_w0_symbol_offset = post_symbols;
        for idx in sym_idx..(sym_idx + post_symbols) {
            let metrics = self.symbol_metrics[idx];
            peak_sum += metrics.peak_ratio;
            if self.is_non_w0_like(metrics) {
                post_non_w0 += 1;
                cur_run += 1;
                longest_non_w0_run = longest_non_w0_run.max(cur_run);
                if first_non_w0_row == 0 {
                    first_non_w0_row = metrics.peak_row;
                }
                if first_non_w0_symbol_offset == post_symbols {
                    first_non_w0_symbol_offset = idx - sym_idx;
                }
            } else {
                cur_run = 0;
            }
        }

        Some(TransitionMetrics {
            pre_w0,
            post_non_w0,
            longest_non_w0_run,
            first_non_w0_symbol_offset,
            avg_post_peak_ratio: peak_sum / post_symbols as f32,
            first_non_w0_row,
        })
    }

    fn transition_better(
        lhs: &(usize, TransitionMetrics),
        rhs: &(usize, TransitionMetrics),
    ) -> std::cmp::Ordering {
        rhs.1
            .longest_non_w0_run
            .cmp(&lhs.1.longest_non_w0_run)
            .then_with(|| rhs.1.post_non_w0.cmp(&lhs.1.post_non_w0))
            .then_with(|| rhs.1.pre_w0.cmp(&lhs.1.pre_w0))
            .then_with(|| {
                rhs.1
                    .avg_post_peak_ratio
                    .partial_cmp(&lhs.1.avg_post_peak_ratio)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                lhs.1
                    .first_non_w0_symbol_offset
                    .cmp(&rhs.1.first_non_w0_symbol_offset)
            })
            .then_with(|| lhs.0.cmp(&rhs.0))
    }

    fn preamble_alignment_metrics_at(
        &self,
        offset: usize,
        confirm_symbols: usize,
    ) -> Option<PreambleAlignmentMetrics> {
        let sym_len = self.symbol_sample_len();
        if offset + confirm_symbols * sym_len > self.samples.len() {
            return None;
        }

        let mut w0_symbols = 0usize;
        let mut peak_sum = 0.0f32;
        let mut margin_sum = 0.0f32;
        for idx in 0..confirm_symbols {
            let metrics = self.metrics_at(offset + idx * sym_len)?;
            peak_sum += metrics.peak_ratio;
            margin_sum += metrics.margin_ratio;
            if self.is_w0_like(metrics) {
                w0_symbols += 1;
            }
        }

        Some(PreambleAlignmentMetrics {
            w0_symbols,
            avg_peak_ratio: peak_sum / confirm_symbols as f32,
            avg_margin_ratio: margin_sum / confirm_symbols as f32,
        })
    }

    fn preamble_alignment_better(
        lhs: &(usize, PreambleAlignmentMetrics),
        rhs: &(usize, PreambleAlignmentMetrics),
    ) -> std::cmp::Ordering {
        rhs.1
            .w0_symbols
            .cmp(&lhs.1.w0_symbols)
            .then_with(|| {
                rhs.1
                    .avg_peak_ratio
                    .partial_cmp(&lhs.1.avg_peak_ratio)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                rhs.1
                    .avg_margin_ratio
                    .partial_cmp(&lhs.1.avg_margin_ratio)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| lhs.0.cmp(&rhs.0))
    }

    fn lock_on_preamble_step(&mut self) -> Vec<SampleBlock> {
        let n = self.samples.len();
        let sym_len = self.symbol_sample_len();
        let confirm_symbols = self.min_preamble_symbols.max(16);
        if n < confirm_symbols * sym_len {
            return Vec::new();
        }

        let chip_step = self.oversample.max(1);
        let search_end = sym_len.min(n.saturating_sub(confirm_symbols * sym_len));
        let mut best: Option<(usize, PreambleAlignmentMetrics)> = None;
        for candidate in (0..=search_end).step_by(chip_step) {
            let Some(metrics) = self.preamble_alignment_metrics_at(candidate, confirm_symbols)
            else {
                continue;
            };
            if metrics.w0_symbols < self.min_preamble_symbols {
                continue;
            }
            let scored = (candidate, metrics);
            match &best {
                Some(prev) if Self::preamble_alignment_better(prev, &scored).is_le() => {}
                _ => best = Some(scored),
            }
        }

        let Some((best_offset, best_metrics)) = best else {
            let max_samples = MAX_WINDOW_SYMBOLS * sym_len;
            if self.samples.len() > max_samples {
                let trim = self.samples.len() - max_samples;
                trace!(
                    "walsh_aligner: no preamble-only lock candidate, trimming {} samples at chip_start={}",
                    trim,
                    self.chip_start.unwrap_or(0),
                );
                self.discard_front(trim);
            }
            return Vec::new();
        };

        info!(
            "walsh_aligner: locked on preamble at offset {} chip_start={} w0_symbols={} avg_peak={:.4} avg_margin={:.4}",
            best_offset / self.oversample,
            self.chip_start.unwrap_or(0) + best_offset / self.oversample,
            best_metrics.w0_symbols,
            best_metrics.avg_peak_ratio,
            best_metrics.avg_margin_ratio,
        );

        self.discard_front(best_offset);
        self.state = AlignerState::Locked;
        self.best_rejected_transition = None;
        self.tags.insert("access_walsh_locked", 1);
        self.tags.insert(
            "access_walsh_lock_avg_post_peak_milli",
            (best_metrics.avg_peak_ratio * 1000.0) as i64,
        );
        self.locked_step()
    }

    /// Scan the sliding buffer for a W0 → non-W0 transition.
    ///
    /// No alignment assumptions: the search uses a sliding W0 energy
    /// detector at 1-chip resolution to find the approximate transition,
    /// then refines at 1-chip resolution with full Walsh correlation to
    /// lock onto the exact symbol boundary.
    ///
    /// The preamble confirmation state (`preamble_confirmed`) persists
    /// across calls so that long preambles that exceed the buffer window
    /// don't reset progress.
    fn acquiring_step(&mut self) -> Vec<SampleBlock> {
        let n = self.samples.len();
        let sym_len = self.symbol_sample_len();
        if n < sym_len * (self.min_preamble_symbols + COARSE_POST_SYMBOLS) {
            return Vec::new();
        }

        // ── Phase 1: Symbol-level preamble confirmation ──
        //
        // The preamble detector upstream already gates on W0, so the aligner
        // only needs enough consecutive symbol-level W0 evidence to avoid
        // latching onto pure garbage. Symbol metrics already score all sample
        // phases, so keep this confirmation in chip units and let the Walsh
        // demod make the final per-symbol phase choice later.
        if !self.preamble_confirmed {
            let mut consecutive_high = 0usize;
            let mut confirmed_chip_start = None;
            for (sym_idx, metrics) in self.symbol_metrics.iter().copied().enumerate() {
                if self.is_w0_like(metrics) {
                    consecutive_high += 1;
                    if consecutive_high >= self.min_preamble_symbols {
                        confirmed_chip_start = Some(
                            self.chip_start.unwrap_or(0)
                                + (sym_idx + 1 - consecutive_high) * PN_CHIPS_PER_SYMBOL,
                        );
                        self.preamble_confirmed = true;
                        self.preamble_confirmed_symbol_start = Some(sym_idx + 1 - consecutive_high);
                        self.coarse_search_cursor_sym =
                            (sym_idx + 1 - consecutive_high) + self.min_preamble_symbols;
                        break;
                    }
                } else {
                    consecutive_high = 0;
                }
            }
            if let Some(chip_start) = confirmed_chip_start {
                info!(
                    "walsh_aligner: preamble confirmed after {} consecutive W0 symbols, chip_start={}",
                    self.min_preamble_symbols, chip_start,
                );
            }
        }

        if !self.preamble_confirmed {
            let max_samples = MAX_WINDOW_SYMBOLS * sym_len;
            if self.samples.len() > max_samples {
                let trim = self.samples.len() - max_samples;
                trace!(
                    "walsh_aligner: trimming {} samples, buffered={} preamble_confirmed={} chip_start={}",
                    trim,
                    self.samples.len(),
                    self.preamble_confirmed,
                    self.chip_start.unwrap_or(0),
                );
                self.discard_front(trim);
            }
            return Vec::new();
        }

        if self.lock_config.lock_on_preamble_only {
            return self.lock_on_preamble_step();
        }

        // ── Phase 2: Coarse symbol-level transition search ──
        //
        // Search the whole buffered extent for the strongest "enough W0
        // behind us + enough non-W0 ahead of us" transition candidate.
        let pre_symbols = self.min_preamble_symbols;
        let post_symbols = COARSE_POST_SYMBOLS;
        let min_pre_w0 = (pre_symbols + 1) / 2;
        let max_symbol_index = self.symbol_metrics.len();
        let min_transition_sym = self
            .preamble_confirmed_symbol_start
            .map(|s| s.saturating_add(pre_symbols))
            .unwrap_or(pre_symbols);
        let coarse_start_sym = if self.lock_config.search_full_buffer {
            pre_symbols
        } else {
            pre_symbols
                .max(max_symbol_index.saturating_sub(post_symbols + COARSE_SEARCH_TAIL_SYMBOLS))
        };
        let incremental_start_sym = self
            .coarse_search_cursor_sym
            .saturating_sub(post_symbols + 1)
            .max(min_transition_sym);
        let search_start_sym = coarse_start_sym.max(incremental_start_sym);
        let mut coarse_best: Option<(usize, TransitionMetrics)> = None;
        let mut rejected_best: Option<(usize, TransitionMetrics)> = None;
        for sym_idx in search_start_sym..=max_symbol_index.saturating_sub(post_symbols) {
            let offset = sym_idx * sym_len;
            let metrics =
                match self.transition_metrics_at_symbol(sym_idx, pre_symbols, post_symbols) {
                    Some(metrics) => metrics,
                    None => continue,
                };
            let candidate = (offset, metrics.clone());
            match &rejected_best {
                Some(best) if Self::transition_better(best, &candidate).is_le() => {}
                _ => rejected_best = Some(candidate),
            }
            if metrics.pre_w0 < min_pre_w0
                || metrics.post_non_w0 < self.lock_config.coarse_min_post_non_w0_symbols
                || metrics.longest_non_w0_run < self.lock_config.coarse_min_non_w0_run
            {
                continue;
            }
            let candidate = (offset, metrics);
            match &coarse_best {
                Some(best) if Self::transition_better(best, &candidate).is_le() => {}
                _ => coarse_best = Some(candidate),
            }
        }
        self.coarse_search_cursor_sym = max_symbol_index.saturating_sub(post_symbols);

        let (transition_approx, coarse_metrics) = match coarse_best {
            Some(best) => best,
            None => {
                if let Some(candidate) = rejected_best.clone() {
                    match &self.best_rejected_transition {
                        Some(best) if Self::transition_better(best, &candidate).is_le() => {}
                        _ => self.best_rejected_transition = Some(candidate),
                    }
                }
                if let Some((offset, metrics)) = rejected_best {
                    trace!(
                        "walsh_aligner: no coarse transition candidate at chip_start={} best_rejected_offset={} pre_w0={} post_non_w0={} longest_non_w0_run={} first_non_w0_offset={} avg_post_peak={:.4}",
                        self.chip_start.unwrap_or(0),
                        offset / self.oversample,
                        metrics.pre_w0,
                        metrics.post_non_w0,
                        metrics.longest_non_w0_run,
                        metrics.first_non_w0_symbol_offset,
                        metrics.avg_post_peak_ratio,
                    );
                }
                let max_samples = MAX_WINDOW_SYMBOLS * sym_len;
                if self.samples.len() > max_samples {
                    let trim = self.samples.len() - max_samples;
                    trace!(
                        "walsh_aligner: trimming {} samples, buffered={} preamble_confirmed={} chip_start={}",
                        trim,
                        self.samples.len(),
                        self.preamble_confirmed,
                        self.chip_start.unwrap_or(0),
                    );
                    self.discard_front(trim);
                }
                return Vec::new();
            }
        };

        trace!(
            "walsh_aligner: coarse transition at offset {} chip_start={} pre_w0={} post_non_w0={} longest_non_w0_run={} first_non_w0_offset={} avg_post_peak={:.4}",
            transition_approx / self.oversample,
            self.chip_start.unwrap_or(0) + transition_approx / self.oversample,
            coarse_metrics.pre_w0,
            coarse_metrics.post_non_w0,
            coarse_metrics.longest_non_w0_run,
            coarse_metrics.first_non_w0_symbol_offset,
            coarse_metrics.avg_post_peak_ratio,
        );

        // ── Phase 3: Fine search around the coarse transition candidate.
        // Pre-compute Walsh-chip partial sums for the entire fine-search
        // region so that metrics_at skips the inner 4-sample loop.
        let refine_start = transition_approx.saturating_sub(sym_len);
        let refine_end =
            (transition_approx + sym_len).min(n.saturating_sub(post_symbols * sym_len));
        let cache_end = (refine_end + post_symbols * sym_len + sym_len).min(n);
        self.wc_cache = Some(self.build_wc_cache(refine_start, cache_end));

        // Stage 1: Sweep at 4-chip (Walsh-chip) resolution.
        let coarse_step = (RC1_PN_CHIPS_PER_WALSH_CHIP * self.oversample).max(1);
        let chip_step = self.oversample.max(1);
        let mut fine_best: Option<(usize, TransitionMetrics)> = None;
        let mut probe_candidates: Vec<(usize, TransitionMetrics)> = Vec::new();
        for candidate in (refine_start..=refine_end).step_by(coarse_step) {
            let metrics = match self.transition_metrics_at(candidate, pre_symbols, post_symbols) {
                Some(metrics) => metrics,
                None => continue,
            };
            if metrics.pre_w0 < min_pre_w0
                || metrics.post_non_w0 < self.lock_config.coarse_min_post_non_w0_symbols
                || metrics.longest_non_w0_run < self.lock_config.coarse_min_non_w0_run
            {
                continue;
            }
            let scored = (candidate, metrics);
            Self::push_top_transition_candidate(&mut probe_candidates, scored.clone());
            if scored.1.post_non_w0 < self.lock_config.fine_min_post_non_w0_symbols
                || scored.1.longest_non_w0_run < self.lock_config.fine_min_non_w0_run
            {
                continue;
            }
            match &fine_best {
                Some(best) if Self::transition_better(best, &scored).is_le() => {}
                _ => fine_best = Some(scored),
            }
        }

        // Stage 2: Refine at 1-chip resolution around the stage-1 winner.
        if let Some((coarse_winner, _)) = fine_best {
            // The 4-chip sweep can pick the right transition neighborhood but
            // still miss the best exact chip boundary by multiple Walsh-chip
            // buckets on marginal bursts. Search a wider radius at 1-chip
            // resolution before committing the lock.
            let fine_radius = coarse_step * 4;
            let pixel_start = coarse_winner.saturating_sub(fine_radius).max(refine_start);
            let pixel_end = (coarse_winner + fine_radius).min(refine_end);
            for candidate in (pixel_start..=pixel_end).step_by(chip_step) {
                let metrics = match self.transition_metrics_at(candidate, pre_symbols, post_symbols)
                {
                    Some(metrics) => metrics,
                    None => continue,
                };
                if metrics.pre_w0 < min_pre_w0
                    || metrics.post_non_w0 < self.lock_config.coarse_min_post_non_w0_symbols
                    || metrics.longest_non_w0_run < self.lock_config.coarse_min_non_w0_run
                {
                    continue;
                }
                let scored = (candidate, metrics);
                Self::push_top_transition_candidate(&mut probe_candidates, scored.clone());
                if scored.1.post_non_w0 < self.lock_config.fine_min_post_non_w0_symbols
                    || scored.1.longest_non_w0_run < self.lock_config.fine_min_non_w0_run
                {
                    continue;
                }
                match &fine_best {
                    Some(best) if Self::transition_better(best, &scored).is_le() => {}
                    _ => fine_best = Some(scored),
                }
            }
        }
        self.wc_cache = None;

        let (mut best_offset, mut best_metrics) = match fine_best {
            Some(best) => best,
            None => {
                // Don't discard the preamble — wait for more data so the
                // lookahead window can extend further into data.
                trace!(
                    "walsh_aligner: no fine lock around coarse transition {} chip_start={}, waiting for more data",
                    transition_approx / self.oversample,
                    self.chip_start.unwrap_or(0) + transition_approx / self.oversample,
                );
                let max_samples = MAX_WINDOW_SYMBOLS * sym_len;
                if self.samples.len() > max_samples {
                    let trim = self.samples.len() - max_samples;
                    self.discard_front(trim);
                }
                return Vec::new();
            }
        };

        let mut chosen_hint_symbols = best_metrics.first_non_w0_symbol_offset;
        let mut best_probe: Option<(usize, usize, usize, Option<usize>, TransitionMetrics)> = None;
        for (candidate_offset, candidate_metrics) in &probe_candidates {
            let Some((score, hint_symbols, fixed_phase)) =
                self.probe_access_decode_score(*candidate_offset)
            else {
                continue;
            };
            match best_probe {
                Some((best_score, best_offset_probe, best_hint, best_phase, _))
                    if best_score > score
                        || (best_score == score
                            && (best_offset_probe < *candidate_offset
                                || (best_offset_probe == *candidate_offset
                                    && (best_hint < hint_symbols
                                        || (best_hint == hint_symbols
                                            && best_phase.is_none()
                                            && fixed_phase.is_some()))))) => {}
                _ => {
                    best_probe = Some((
                        score,
                        *candidate_offset,
                        hint_symbols,
                        fixed_phase,
                        candidate_metrics.clone(),
                    ));
                }
            }
            if score >= 100 {
                break;
            }
        }
        let mut chosen_fixed_phase: Option<usize> = None;
        if let Some((
            probe_score,
            probe_offset,
            probe_hint_symbols,
            probe_fixed_phase,
            probe_metrics,
        )) = best_probe
        {
            info!(
                "walsh_aligner: access decode probe chose offset {} hint={} fixed_phase={:?} score={} chip_start={}",
                probe_offset / self.oversample,
                probe_hint_symbols,
                probe_fixed_phase,
                probe_score,
                self.chip_start.unwrap_or(0) + probe_offset / self.oversample,
            );
            best_offset = probe_offset;
            best_metrics = probe_metrics;
            chosen_hint_symbols = probe_hint_symbols;
            chosen_fixed_phase = probe_fixed_phase;
        }

        if best_metrics.post_non_w0 < self.lock_config.fine_min_post_non_w0_symbols
            || best_metrics.longest_non_w0_run < self.lock_config.fine_min_non_w0_run
        {
            trace!(
                "walsh_aligner: fine search at {} (coarse {}) failed confirmation, waiting for more data",
                best_offset, transition_approx,
            );
            let max_samples = MAX_WINDOW_SYMBOLS * sym_len;
            if self.samples.len() > max_samples {
                let trim = self.samples.len() - max_samples;
                self.discard_front(trim);
            }
            return Vec::new();
        }

        let first_non_w0_offset = best_offset + best_metrics.first_non_w0_symbol_offset * sym_len;

        info!(
            "walsh_aligner: locked at offset {} first_non_w0_offset={} (approx {}, delta={}) first_non_w0_row={} first_non_w0_offset_syms={} post_non_w0={} longest_non_w0_run={} avg_post_peak={:.4} chip_start={}",
            best_offset / self.oversample,
            first_non_w0_offset / self.oversample,
            transition_approx / self.oversample,
            (best_offset as isize - transition_approx as isize) / self.oversample as isize,
            best_metrics.first_non_w0_row,
            best_metrics.first_non_w0_symbol_offset,
            best_metrics.post_non_w0,
            best_metrics.longest_non_w0_run,
            best_metrics.avg_post_peak_ratio,
            self.chip_start.unwrap_or(0) + best_offset / self.oversample,
        );

        self.discard_front(best_offset);
        self.state = AlignerState::Locked;
        self.tags.insert("access_walsh_locked", 1);
        self.tags.insert(
            "access_walsh_lock_avg_post_peak_milli",
            (best_metrics.avg_post_peak_ratio * 1000.0) as i64,
        );
        self.tags
            .insert("access_frame_hint_symbols", chosen_hint_symbols as i64);
        if let Some(phase) = chosen_fixed_phase {
            self.tags.insert("access_fixed_phase", phase as i64);
        }
        self.locked_step()
    }

    fn locked_step(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.samples.len() >= self.symbol_sample_len() {
            out.push(self.chunk_from_offset(0));
            self.discard_front(self.symbol_sample_len());
        }
        out
    }
}

impl PipelineProcessor for ReverseAccessWalshAligner {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.ingest_block(block);
        let t = std::time::Instant::now();
        let result = match self.state {
            AlignerState::Acquiring => self.acquiring_step(),
            AlignerState::Locked => self.locked_step(),
        };
        let elapsed_us = t.elapsed().as_micros() as u64;
        match self.state {
            AlignerState::Acquiring => self.acquiring_us += elapsed_us,
            AlignerState::Locked => self.locked_us += elapsed_us,
        }
        self.process_calls += 1;
        result
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        match self.state {
            AlignerState::Acquiring => {
                if let Some((offset, metrics)) = &self.best_rejected_transition {
                    info!(
                        "walsh_aligner: flush without lock best_rejected_offset={} chip_start={} pre_w0={} post_non_w0={} longest_non_w0_run={} first_non_w0_offset={} avg_post_peak={:.4} first_non_w0_row={}",
                        offset / self.oversample.max(1),
                        self.chip_start.unwrap_or(0) + offset / self.oversample.max(1),
                        metrics.pre_w0,
                        metrics.post_non_w0,
                        metrics.longest_non_w0_run,
                        metrics.first_non_w0_symbol_offset,
                        metrics.avg_post_peak_ratio,
                        metrics.first_non_w0_row,
                    );
                }
                self.samples.clear();
                self.best_rejected_transition = None;
                Vec::new()
            }
            AlignerState::Locked => self.locked_step(),
        }
    }

    fn name(&self) -> &'static str {
        "ReverseAccessWalshAligner"
    }

    fn metrics(&self) -> Vec<(&'static str, String)> {
        let acq_ms = self.acquiring_us as f64 / 1000.0;
        let lock_ms = self.locked_us as f64 / 1000.0;
        let state = match self.state {
            AlignerState::Acquiring => "acquiring",
            AlignerState::Locked => "locked",
        };
        vec![
            ("state", state.to_string()),
            ("acquiring_ms", format!("{:.1}", acq_ms)),
            ("locked_ms", format!("{:.1}", lock_ms)),
            ("calls", format!("{}", self.process_calls)),
            ("symbols", format!("{}", self.symbol_metrics.len())),
        ]
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;
    use num_complex::Complex32;

    use super::{
        COARSE_POST_SYMBOLS, PN_CHIPS_PER_SYMBOL, PipelineProcessor, RC1_SYMBOLS_PER_FRAME,
        ReverseAccessWalshAligner, SampleBlock,
    };
    use crate::lac::crc30;
    use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_576};
    use crate::phy::coding::convolutional::get_1_3_k9_encoder;
    use crate::phy::coding::symbol_repeat::SymbolRepetition;
    use crate::phy::walsh::WalshGenerator;

    fn make_symbol(row: usize) -> Vec<Complex32> {
        let walsh = WalshGenerator::generate_matrix::<64>();
        let mut out = Vec::with_capacity(PN_CHIPS_PER_SYMBOL);
        for &chip in &walsh[row] {
            for _ in 0..4 {
                out.push(Complex32::new(chip as f32, 0.0));
            }
        }
        out
    }

    fn make_valid_access_frame_symbols() -> Vec<Vec<Complex32>> {
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

        let _walsh_matrix = WalshGenerator::generate_matrix::<64>();
        interleaved
            .chunks_exact(6)
            .map(|group| {
                let index = group[0] as usize
                    + 2 * group[1] as usize
                    + 4 * group[2] as usize
                    + 8 * group[3] as usize
                    + 16 * group[4] as usize
                    + 32 * group[5] as usize;
                make_symbol(index)
            })
            .collect()
    }

    fn extend_with_symbols(dst: &mut Vec<Complex32>, symbols: &[Vec<Complex32>]) {
        for symbol in symbols {
            dst.extend(symbol.iter().copied());
        }
    }

    fn split_blocks(samples: Vec<Complex32>, sizes: &[usize]) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for &size in sizes {
            if offset >= samples.len() {
                break;
            }
            let end = (offset + size).min(samples.len());
            out.push(SampleBlock::new(samples[offset..end].to_vec(), offset));
            offset = end;
        }
        if offset < samples.len() {
            out.push(SampleBlock::new(samples[offset..].to_vec(), offset));
        }
        out
    }

    #[test]
    fn aligner_locks_on_aligned_preamble_to_data_transition() {
        let mut samples = Vec::new();
        for _ in 0..4 {
            samples.extend(make_symbol(0));
        }
        let data_symbols = make_valid_access_frame_symbols();
        extend_with_symbols(&mut samples, &data_symbols);
        for _ in 0..20 {
            samples.extend(make_symbol(0));
        }

        let mut p = ReverseAccessWalshAligner::with_min_preamble_symbols(4);
        let mut out = p.process_block(SampleBlock::new(samples, 0));
        out.extend(p.flush());

        assert!(out.len() >= 96);
        assert_eq!(out[0].chip_start, 4 * PN_CHIPS_PER_SYMBOL);
        assert_eq!(out[0].samples.len(), PN_CHIPS_PER_SYMBOL);
    }

    #[test]
    fn aligner_does_not_lock_without_non_w0_transition() {
        let mut samples = Vec::new();
        for _ in 0..6 {
            samples.extend(make_symbol(0));
        }

        let mut p = ReverseAccessWalshAligner::with_min_preamble_symbols(4);
        let mut out = Vec::new();
        for block in split_blocks(samples, &[19, 333, 901, 511]) {
            out.extend(p.process_block(block));
        }
        out.extend(p.flush());

        assert!(out.is_empty());
    }

    #[test]
    fn aligner_slides_past_garbage_prefix_before_locking() {
        let mut samples = vec![Complex32::new(0.125, -0.375); PN_CHIPS_PER_SYMBOL];
        for _ in 0..4 {
            samples.extend(make_symbol(0));
        }
        let data_symbols = make_valid_access_frame_symbols();
        extend_with_symbols(&mut samples, &data_symbols);
        for _ in 0..20 {
            samples.extend(make_symbol(0));
        }

        let mut p = ReverseAccessWalshAligner::with_min_preamble_symbols(4);
        let mut out = p.process_block(SampleBlock::new(samples, 0));
        out.extend(p.flush());

        assert!(out.len() >= 96);
        assert_eq!(out[0].samples.len(), PN_CHIPS_PER_SYMBOL);
    }

    #[test]
    fn aligner_locks_with_symbol_aligned_blocks() {
        // The aligner receives pre-aligned 256-chip blocks from the
        // preamble detector upstream, so test with symbol-aligned input.
        let mut samples = Vec::new();
        for _ in 0..60 {
            samples.extend(make_symbol(0));
        }
        let data_symbols = make_valid_access_frame_symbols();
        extend_with_symbols(&mut samples, &data_symbols);
        for _ in 0..20 {
            samples.extend(make_symbol(0));
        }

        let mut p = ReverseAccessWalshAligner::with_min_preamble_symbols(48);
        let mut out = Vec::new();
        // Feed one symbol at a time, like the real pipeline.
        for block in split_blocks(samples, &[PN_CHIPS_PER_SYMBOL; 200]) {
            out.extend(p.process_block(block));
        }
        out.extend(p.flush());

        assert!(!out.is_empty());
        assert!(out[0].chip_start > 0);
        assert_eq!(out[0].samples.len(), PN_CHIPS_PER_SYMBOL);
    }

    #[test]
    fn aligner_handles_very_long_preamble_before_data_transition() {
        let total_preamble_symbols = 900usize;
        let symbols_already_elapsed = 72usize;
        let visible_preamble_symbols = total_preamble_symbols - symbols_already_elapsed;

        let mut samples = Vec::new();
        for _ in 0..visible_preamble_symbols {
            samples.extend(make_symbol(0));
        }
        let data_symbols = make_valid_access_frame_symbols();
        extend_with_symbols(&mut samples, &data_symbols);
        for _ in 0..32 {
            samples.extend(make_symbol(0));
        }

        let mut p = ReverseAccessWalshAligner::with_min_preamble_symbols(8);
        let mut out = Vec::new();

        // Feed mostly one symbol at a time to mimic the real finger path,
        // where the transition may sit far to the right of the initial window.
        let mut block_sizes =
            vec![PN_CHIPS_PER_SYMBOL; visible_preamble_symbols + data_symbols.len() + 16];
        block_sizes.extend([113, 257, 511, 1024]);
        for block in split_blocks(samples, &block_sizes) {
            out.extend(p.process_block(block));
        }
        out.extend(p.flush());

        assert!(
            out.len() >= RC1_SYMBOLS_PER_FRAME,
            "expected at least one full aligned frame, got {} symbols",
            out.len()
        );
        let expected_transition_chip = visible_preamble_symbols * PN_CHIPS_PER_SYMBOL;
        assert!(
            out[0].chip_start <= expected_transition_chip,
            "expected aligner to retain symbols leading into the transition: out[0]={} transition={}",
            out[0].chip_start,
            expected_transition_chip,
        );
        assert!(
            expected_transition_chip
                < out[0].chip_start + COARSE_POST_SYMBOLS * PN_CHIPS_PER_SYMBOL,
            "expected transition chip {} to remain inside the emitted search window starting at {}",
            expected_transition_chip,
            out[0].chip_start,
        );
        assert_eq!(out[0].samples.len(), PN_CHIPS_PER_SYMBOL);
    }
}
