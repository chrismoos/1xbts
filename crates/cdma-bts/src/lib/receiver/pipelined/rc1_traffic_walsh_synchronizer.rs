use std::collections::HashMap;

use log::{debug, info};
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};
use crate::phy::walsh::WalshGenerator;
use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME,
    RC1_SYMBOLS_PER_PCG, RC1_WALSH_CHIPS_PER_SYMBOL,
};

/// 64-chip Walsh function repeated 4 times per 256-chip symbol.
const PN_CHIPS_PER_SYMBOL: usize = RC1_WALSH_CHIPS_PER_SYMBOL * RC1_PN_CHIPS_PER_WALSH_CHIP;
const FRAME_CHIPS: usize = RC1_SYMBOLS_PER_FRAME * PN_CHIPS_PER_SYMBOL;
const PREAMBLE_CONFIRM_SYMBOLS: usize = 24;
const SEARCH_LOOKAHEAD_SYMBOLS: usize = RC1_SYMBOLS_PER_PCG;
const MAX_SEARCH_BUFFER_CHIPS: usize = FRAME_CHIPS * 200;
const FINE_SEARCH_HALF_SPAN: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncState {
    Searching,
    Locked,
}

#[derive(Clone, Copy, Debug)]
struct WalshSymbolOutput {
    soft_bits: [f32; RC1_SOFT_BITS_PER_SYMBOL],
    peak_energy: f32,
    total_energy: f32,
    peak_walsh_index: u8,
    peak_margin: f32,
}

impl WalshSymbolOutput {
    fn concentration(self) -> f32 {
        if self.total_energy > 1e-9 {
            self.peak_energy / self.total_energy
        } else {
            0.0
        }
    }

    fn is_w0_like(self) -> bool {
        self.peak_walsh_index == 0 && self.peak_margin > 3.0 && self.concentration() > 0.15
    }
}

#[derive(Clone, Copy, Debug)]
struct PreambleCandidate {
    chip_phase: usize,
    run_start_symbol: usize,
    run_len_symbols: usize,
    data_symbol_start: usize,
    score: f32,
    preamble_only: bool,
}

pub struct Rc1TrafficWalshSynchronizer {
    state: SyncState,
    chip_buf: Vec<Complex32>,
    tags: HashMap<&'static str, i64>,
    chip_start: usize,
    sample_rate_hz: f64,
    absolute_chip_start: Option<i64>,
    search_attempts: usize,
}

impl Rc1TrafficWalshSynchronizer {
    pub fn new() -> Self {
        Self {
            state: SyncState::Searching,
            chip_buf: Vec::new(),
            tags: HashMap::new(),
            chip_start: 0,
            sample_rate_hz: 0.0,
            absolute_chip_start: None,
            search_attempts: 0,
        }
    }

    fn refresh_absolute_chip_tags(&mut self) {
        if let Some(absolute_chip_start) = self.tags.get("absolute_chip_start").copied() {
            self.absolute_chip_start = Some(absolute_chip_start);
        }
    }

    fn demodulate_symbol(chips: &[Complex32]) -> WalshSymbolOutput {
        debug_assert_eq!(chips.len(), PN_CHIPS_PER_SYMBOL);

        // Each Walsh chip is spread by 4 consecutive PN chips.
        // Walsh chip k corresponds to PN chips [4k, 4k+1, 4k+2, 4k+3].
        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for k in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
            for j in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                walsh_chips[k] += chips[k * RC1_PN_CHIPS_PER_WALSH_CHIP + j];
            }
        }

        WalshGenerator::fwht_fixed(&mut walsh_chips);

        let mut energies = [0.0f32; RC1_WALSH_CHIPS_PER_SYMBOL];
        for (idx, corr) in walsh_chips.iter().enumerate() {
            energies[idx] = corr.re * corr.re + corr.im * corr.im;
        }

        let mut best_row = 0usize;
        let mut best_energy = f32::NEG_INFINITY;
        let mut second_best = f32::NEG_INFINITY;
        let mut total_energy = 0.0f32;
        for (idx, &energy) in energies.iter().enumerate() {
            total_energy += energy;
            if energy > best_energy {
                second_best = best_energy;
                best_energy = energy;
                best_row = idx;
            } else if energy > second_best {
                second_best = energy;
            }
        }
        let peak_margin = if second_best > 1e-9 {
            best_energy / second_best
        } else {
            best_energy
        };

        let mut soft_bits = [0.0f32; RC1_SOFT_BITS_PER_SYMBOL];
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
            soft_bits[bit] = max_zero - max_one;
        }

        WalshSymbolOutput {
            soft_bits,
            peak_energy: best_energy.max(0.0),
            total_energy,
            peak_walsh_index: best_row as u8,
            peak_margin,
        }
    }

    fn demodulate_symbol_stream(&self, chip_phase: usize) -> Vec<WalshSymbolOutput> {
        let total_symbols = self.chip_buf.len().saturating_sub(chip_phase) / PN_CHIPS_PER_SYMBOL;
        let mut out = Vec::with_capacity(total_symbols);
        for symbol_idx in 0..total_symbols {
            let start = chip_phase + symbol_idx * PN_CHIPS_PER_SYMBOL;
            let end = start + PN_CHIPS_PER_SYMBOL;
            out.push(Self::demodulate_symbol(&self.chip_buf[start..end]));
        }
        out
    }

    /// Predict the symbol boundary phase from system time.
    ///
    /// Symbol boundaries occur every 256 chips aligned to system time 0.
    /// Given our buffer starts at `self.chip_start`, the predicted phase
    /// offset into the buffer is `(256 - (chip_start % 256)) % 256`.
    fn predicted_chip_phase(&self) -> usize {
        (PN_CHIPS_PER_SYMBOL - (self.chip_start % PN_CHIPS_PER_SYMBOL)) % PN_CHIPS_PER_SYMBOL
    }

    fn find_best_preamble_candidate(&self) -> Option<PreambleCandidate> {
        let min_symbols = PREAMBLE_CONFIRM_SYMBOLS + SEARCH_LOOKAHEAD_SYMBOLS;
        if self.chip_buf.len() < min_symbols * PN_CHIPS_PER_SYMBOL {
            return None;
        }

        let predicted = self.predicted_chip_phase();

        let mut best: Option<PreambleCandidate> = None;
        // ±4 chip fine search around the system-time predicted phase
        for delta in 0..=(2 * FINE_SEARCH_HALF_SPAN) {
            let chip_phase = (predicted + PN_CHIPS_PER_SYMBOL + delta - FINE_SEARCH_HALF_SPAN)
                % PN_CHIPS_PER_SYMBOL;
            let symbols = self.demodulate_symbol_stream(chip_phase);
            if symbols.len() < min_symbols {
                continue;
            }

            let mut run_start = 0usize;
            while run_start < symbols.len() {
                while run_start < symbols.len() && !symbols[run_start].is_w0_like() {
                    run_start += 1;
                }
                if run_start >= symbols.len() {
                    break;
                }

                let mut run_end = run_start;
                while run_end < symbols.len() && symbols[run_end].is_w0_like() {
                    run_end += 1;
                }

                let run_len = run_end - run_start;
                if run_len >= PREAMBLE_CONFIRM_SYMBOLS {
                    let (score, data_symbol_start, preamble_only) =
                        if run_end + SEARCH_LOOKAHEAD_SYMBOLS <= symbols.len() {
                            (
                                symbols[run_end..run_end + SEARCH_LOOKAHEAD_SYMBOLS]
                                    .iter()
                                    .map(|symbol| symbol.concentration())
                                    .sum::<f32>(),
                                run_end,
                                false,
                            )
                        } else {
                            (
                                run_len as f32
                                    + symbols[run_start..run_end]
                                        .iter()
                                        .map(|symbol| symbol.concentration())
                                        .sum::<f32>(),
                                run_start,
                                true,
                            )
                        };

                    let candidate = PreambleCandidate {
                        chip_phase,
                        run_start_symbol: run_start,
                        run_len_symbols: run_len,
                        data_symbol_start,
                        score,
                        preamble_only,
                    };
                    let replace = best
                        .map(|current| {
                            (!candidate.preamble_only && current.preamble_only)
                                || (candidate.preamble_only == current.preamble_only
                                    && (candidate.score > current.score
                                        || (candidate.score == current.score
                                            && candidate.run_len_symbols
                                                > current.run_len_symbols)))
                        })
                        .unwrap_or(true);
                    if replace {
                        best = Some(candidate);
                    }
                }

                run_start = run_end.saturating_add(1);
            }
        }

        best
    }

    fn drain_front_chips(&mut self, n_chips: usize) {
        let n = n_chips.min(self.chip_buf.len());
        self.chip_buf.drain(..n);
        self.chip_start = self.chip_start.saturating_add(n);
        if let Some(absolute_chip_start) = &mut self.absolute_chip_start {
            *absolute_chip_start = absolute_chip_start.saturating_add(n as i64);
        }
    }

    fn emit_preamble_event(&self, candidate: PreambleCandidate) -> Option<SampleBlock> {
        let preamble_frames = (candidate.run_len_symbols / RC1_SYMBOLS_PER_FRAME).max(1);
        let mut tags = self.tags.clone();
        tags.insert("traffic_preamble_detected", 1);
        tags.insert("traffic_preamble_frames", preamble_frames as i64);
        tags.insert("traffic_walsh_locked", 1);
        tags.insert("traffic_frame_aligned", 1);
        let chip_delta = candidate.chip_phase + candidate.run_start_symbol * PN_CHIPS_PER_SYMBOL;
        if let Some(absolute_chip_start) = self.absolute_chip_start {
            tags.insert(
                "absolute_chip_start",
                absolute_chip_start.saturating_add(chip_delta as i64),
            );
        }
        Some(
            SampleBlock::new(Vec::new(), self.chip_start.saturating_add(chip_delta))
                .with_sample_rate_hz(self.sample_rate_hz)
                .with_tags(tags),
        )
    }

    fn emit_frames(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.chip_buf.len() >= FRAME_CHIPS {
            let mut soft = Vec::with_capacity(RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL);
            let mut all_w0_like = true;
            for symbol_idx in 0..RC1_SYMBOLS_PER_FRAME {
                let start = symbol_idx * PN_CHIPS_PER_SYMBOL;
                let end = start + PN_CHIPS_PER_SYMBOL;
                let symbol = Self::demodulate_symbol(&self.chip_buf[start..end]);
                all_w0_like &= symbol.is_w0_like();
                for &bit in &symbol.soft_bits {
                    soft.push(Complex32::new(bit, 0.0));
                }
            }

            let mut tags = self.tags.clone();
            tags.insert("traffic_symbol_frame", 1);
            tags.insert("traffic_frame_aligned", 1);
            tags.insert("traffic_walsh_locked", 1);
            tags.insert("traffic_is_preamble", all_w0_like as i64);
            if let Some(absolute_chip_start) = self.absolute_chip_start {
                tags.insert("absolute_chip_start", absolute_chip_start);
            }

            let mut block =
                SampleBlock::new(soft, self.chip_start).with_sample_rate_hz(self.sample_rate_hz);
            block.tags = tags;
            out.push(block);
            self.drain_front_chips(FRAME_CHIPS);
        }
        out
    }

    fn search_step(&mut self) -> Vec<SampleBlock> {
        let Some(candidate) = self.find_best_preamble_candidate() else {
            self.search_attempts = self.search_attempts.saturating_add(1);
            if self.chip_buf.len() > MAX_SEARCH_BUFFER_CHIPS {
                debug!(
                    "rc1_traffic_walsh_synchronizer: sliding search window buffered_chips={} chip_start={}",
                    self.chip_buf.len(),
                    self.chip_start,
                );
                self.drain_front_chips(FRAME_CHIPS);
            }
            return Vec::new();
        };

        info!(
            "rc1_traffic_walsh_synchronizer: locked chip_start={} absolute_chip_start={:?} chip_phase={} preamble_symbols={} data_symbol_start={} score={:.3} preamble_only={}",
            self.chip_start,
            self.absolute_chip_start,
            candidate.chip_phase,
            candidate.run_len_symbols,
            candidate.data_symbol_start,
            candidate.score,
            candidate.preamble_only,
        );

        let mut out = Vec::new();
        if let Some(preamble_event) = self.emit_preamble_event(candidate) {
            out.push(preamble_event);
        }

        let start_symbol = if candidate.preamble_only {
            candidate.run_start_symbol
        } else {
            candidate.data_symbol_start
        };
        let chip_offset = candidate.chip_phase + start_symbol * PN_CHIPS_PER_SYMBOL;
        self.drain_front_chips(chip_offset);

        // Align to the next 20ms frame boundary using absolute system time.
        // Frame boundaries occur at absolute_chip_time mod 24576 == 0.
        // We must skip in whole symbols (multiples of 256 chips) to preserve
        // Walsh symbol alignment established by preamble detection.
        if let Some(abs) = self.absolute_chip_start {
            // Compute symbol-level offset within a frame
            let abs_sym = abs / PN_CHIPS_PER_SYMBOL as i64;
            let sym_in_frame = abs_sym.rem_euclid(RC1_SYMBOLS_PER_FRAME as i64) as usize;
            if sym_in_frame > 0 {
                let skip_syms = RC1_SYMBOLS_PER_FRAME - sym_in_frame;
                let skip = skip_syms * PN_CHIPS_PER_SYMBOL;
                debug!(
                    "rc1_traffic_walsh_synchronizer: frame align skip {} chips ({} symbols) abs_chip={} sym_in_frame={}",
                    skip, skip_syms, abs, sym_in_frame,
                );
                self.drain_front_chips(skip);
            }
        }

        self.state = SyncState::Locked;
        out.extend(self.emit_frames());
        out
    }
}

impl PipelineProcessor for Rc1TrafficWalshSynchronizer {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.chip_buf.is_empty() {
            self.chip_start = block.chip_start;
        }
        self.sample_rate_hz = block.sample_rate_hz;
        self.tags = block.tags.clone();
        self.refresh_absolute_chip_tags();

        self.chip_buf.extend_from_slice(&block.samples);

        match self.state {
            SyncState::Searching => self.search_step(),
            SyncState::Locked => self.emit_frames(),
        }
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        match self.state {
            SyncState::Searching => self.search_step(),
            SyncState::Locked => self.emit_frames(),
        }
    }

    fn name(&self) -> &'static str {
        "Rc1TrafficWalshSynchronizer"
    }
}
