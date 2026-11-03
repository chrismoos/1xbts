//! Per-PCG Eb/Nt for RC1 and RC2 reverse traffic.
//!
//! The non-peak Walsh bins of a transmitted group hold the mobile's own
//! multipath, so an interference estimate taken there rises with its power and
//! the ratio against it barely moves. A group the data burst randomizer left
//! silent holds only everything else, so the estimate comes from those.

use std::collections::VecDeque;

use cdma_common::consts::{RC1_SYMBOLS_PER_PCG, RC1_WALSH_CHIPS_PER_SYMBOL, SR1_PCGS_PER_FRAME};

/// The estimate is the median of this many recent frames. Another mobile's
/// access probe or loud call start lands in the silent groups as a burst far
/// above the real interference, and a median ignores it where a mean would
/// stay inflated for seconds and drive this mobile up.
const GATED_NOISE_WINDOW_FRAMES: usize = 32;

/// A full-rate frame leaves no silent group to measure, so a talk spurt holds
/// the estimate unrefreshed. Holding it is safer than the fallback to a
/// symbol's own non-peak bins, which rise with the mobile's power and can drive
/// it up. Stop trusting it after this many frames, about five seconds.
const GATED_NOISE_MAX_STALE_FRAMES: usize = 250;

/// A failed frame has no trusted rate, so it offers no silent group to
/// measure. While frames fail, interference can rise past the estimate, and
/// Eb/Nt then reads on target with the mobile too weak to decode. Once this many
/// frames fail in a row the estimate is discarded and rebuilt from the frames
/// that decode next. A probe from another phone corrupts about 13 frames, so
/// it does not reach this limit.
const GATED_NOISE_MAX_FAILED_FRAMES: usize = 25;

const NO_SIGNAL_EB_NT_DB: f32 = -30.0;

#[derive(Debug, Clone)]
pub(super) struct GatedNoiseEstimate {
    recent_per_bin: VecDeque<f32>,
    per_bin: Option<f32>,
    frames_since_update: usize,
    consecutive_failed_frames: usize,
}

impl Default for GatedNoiseEstimate {
    fn default() -> Self {
        Self {
            recent_per_bin: VecDeque::with_capacity(GATED_NOISE_WINDOW_FRAMES),
            per_bin: None,
            frames_since_update: usize::MAX,
            consecutive_failed_frames: 0,
        }
    }
}

impl GatedNoiseEstimate {
    /// Folds in one frame's silent groups. `active` must be the mask of a
    /// frame whose rate checked out: a misread rate marks transmitted groups
    /// silent and folds the mobile's own energy into the estimate.
    pub fn update(
        &mut self,
        symbol_energies: &[[f32; RC1_WALSH_CHIPS_PER_SYMBOL]],
        active: &[bool; SR1_PCGS_PER_FRAME],
    ) {
        let mut energy = 0.0f64;
        let mut bins = 0usize;
        for (pcg, transmitted) in active.iter().copied().enumerate() {
            if transmitted {
                continue;
            }
            let start = pcg * RC1_SYMBOLS_PER_PCG;
            let end = (start + RC1_SYMBOLS_PER_PCG).min(symbol_energies.len());
            for symbol in symbol_energies.get(start..end).unwrap_or_default() {
                energy += symbol.iter().map(|bin| *bin as f64).sum::<f64>();
                bins += RC1_WALSH_CHIPS_PER_SYMBOL;
            }
        }
        self.frames_since_update = self.frames_since_update.saturating_add(1);
        self.consecutive_failed_frames = 0;
        if bins == 0 {
            return;
        }
        if self.recent_per_bin.len() == GATED_NOISE_WINDOW_FRAMES {
            self.recent_per_bin.pop_front();
        }
        self.recent_per_bin.push_back((energy / bins as f64) as f32);
        let mut sorted: Vec<f32> = self.recent_per_bin.iter().copied().collect();
        let middle = sorted.len() / 2;
        let (_, median, _) = sorted.select_nth_unstable_by(middle, f32::total_cmp);
        self.per_bin = Some(*median);
        self.frames_since_update = 0;
    }

    pub fn frame_failed(&mut self) {
        self.frames_since_update = self.frames_since_update.saturating_add(1);
        self.consecutive_failed_frames = self.consecutive_failed_frames.saturating_add(1);
        if self.consecutive_failed_frames >= GATED_NOISE_MAX_FAILED_FRAMES {
            self.recent_per_bin.clear();
            self.per_bin = None;
        }
    }

    pub fn per_bin(&self) -> Option<f32> {
        self.per_bin
            .filter(|_| self.frames_since_update <= GATED_NOISE_MAX_STALE_FRAMES)
            .filter(|noise| *noise > 0.0)
    }
}

pub(super) fn walsh64_pcg_eb_nt_db(
    symbols: &[[f32; RC1_WALSH_CHIPS_PER_SYMBOL]],
    gated_noise_per_bin: Option<f32>,
) -> f32 {
    if symbols.is_empty() {
        return NO_SIGNAL_EB_NT_DB;
    }
    let mut linear_eb_nt = 0.0f32;
    for energies in symbols {
        let peak = energies.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let total: f32 = energies.iter().sum();
        let noise_mean = gated_noise_per_bin.unwrap_or(
            ((total - peak).max(0.0) / (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32).max(1e-12),
        );
        let es_nt = (peak / noise_mean - 1.0).max(0.0);
        linear_eb_nt += (es_nt * 0.5).max(1e-9);
    }
    let linear_mean = linear_eb_nt / symbols.len() as f32;
    10.0 * linear_mean.max(1e-9).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOISE_PER_BIN: f32 = 1.0;
    const PEAK: f32 = 41.0;

    fn frame(
        active: &[bool; SR1_PCGS_PER_FRAME],
        leakage_per_bin: f32,
    ) -> Vec<[f32; RC1_WALSH_CHIPS_PER_SYMBOL]> {
        let mut out = Vec::new();
        for transmitted in active {
            for _ in 0..RC1_SYMBOLS_PER_PCG {
                let mut bins = [NOISE_PER_BIN; RC1_WALSH_CHIPS_PER_SYMBOL];
                if *transmitted {
                    bins.iter_mut().for_each(|bin| *bin += leakage_per_bin);
                    bins[0] = PEAK;
                }
                out.push(bins);
            }
        }
        out
    }

    fn half_rate_mask_with_silent_level(
        level: f32,
    ) -> (
        [bool; SR1_PCGS_PER_FRAME],
        Vec<[f32; RC1_WALSH_CHIPS_PER_SYMBOL]>,
    ) {
        let active = half_rate_mask();
        let mut symbols = frame(&active, 0.0);
        for (pcg, transmitted) in active.iter().enumerate() {
            if !transmitted {
                for symbol in &mut symbols[pcg * RC1_SYMBOLS_PER_PCG..][..RC1_SYMBOLS_PER_PCG] {
                    symbol.fill(level);
                }
            }
        }
        (active, symbols)
    }

    #[test]
    fn a_short_burst_in_the_silent_groups_does_not_move_the_estimate() {
        let mut estimate = GatedNoiseEstimate::default();
        let (active, quiet) = half_rate_mask_with_silent_level(NOISE_PER_BIN);
        let (_, burst) = half_rate_mask_with_silent_level(NOISE_PER_BIN * 1000.0);
        for _ in 0..GATED_NOISE_WINDOW_FRAMES {
            estimate.update(&quiet, &active);
        }
        for _ in 0..10 {
            estimate.update(&burst, &active);
        }
        assert_eq!(estimate.per_bin(), Some(NOISE_PER_BIN));
    }

    #[test]
    fn a_lasting_rise_in_interference_is_followed() {
        let mut estimate = GatedNoiseEstimate::default();
        let (active, quiet) = half_rate_mask_with_silent_level(NOISE_PER_BIN);
        let (_, loud) = half_rate_mask_with_silent_level(NOISE_PER_BIN * 4.0);
        for _ in 0..GATED_NOISE_WINDOW_FRAMES {
            estimate.update(&quiet, &active);
        }
        for _ in 0..GATED_NOISE_WINDOW_FRAMES / 2 + 1 {
            estimate.update(&loud, &active);
        }
        assert_eq!(estimate.per_bin(), Some(NOISE_PER_BIN * 4.0));
    }

    fn half_rate_mask() -> [bool; SR1_PCGS_PER_FRAME] {
        std::array::from_fn(|pcg| pcg % 2 == 0)
    }

    #[test]
    fn silent_groups_set_the_noise_and_own_leakage_does_not() {
        let active = half_rate_mask();
        let symbols = frame(&active, 4.0);
        let mut estimate = GatedNoiseEstimate::default();
        estimate.update(&symbols, &active);
        assert_eq!(estimate.per_bin(), Some(NOISE_PER_BIN));

        let pcg = &symbols[..RC1_SYMBOLS_PER_PCG];
        let gated = walsh64_pcg_eb_nt_db(pcg, estimate.per_bin());
        let own = walsh64_pcg_eb_nt_db(pcg, None);
        let expected = 10.0 * ((PEAK / NOISE_PER_BIN - 1.0) * 0.5).log10();
        assert!((gated - expected).abs() < 1e-3);
        assert!(own < gated - 6.0);
    }

    #[test]
    fn a_run_of_failed_frames_discards_the_estimate() {
        let mut estimate = GatedNoiseEstimate::default();
        let (active, quiet) = half_rate_mask_with_silent_level(NOISE_PER_BIN);
        let (_, loud) = half_rate_mask_with_silent_level(NOISE_PER_BIN * 4.0);
        for _ in 0..GATED_NOISE_WINDOW_FRAMES {
            estimate.update(&quiet, &active);
        }
        for _ in 0..GATED_NOISE_MAX_FAILED_FRAMES - 1 {
            estimate.frame_failed();
        }
        assert_eq!(estimate.per_bin(), Some(NOISE_PER_BIN));
        estimate.frame_failed();
        assert_eq!(estimate.per_bin(), None);
        estimate.update(&loud, &active);
        assert_eq!(estimate.per_bin(), Some(NOISE_PER_BIN * 4.0));
    }

    #[test]
    fn full_rate_frames_age_the_estimate_out() {
        let half = half_rate_mask();
        let mut estimate = GatedNoiseEstimate::default();
        estimate.update(&frame(&half, 0.0), &half);
        let full = [true; SR1_PCGS_PER_FRAME];
        let full_frame = frame(&full, 0.0);
        for _ in 0..GATED_NOISE_MAX_STALE_FRAMES {
            estimate.update(&full_frame, &full);
        }
        assert!(estimate.per_bin().is_some());
        estimate.update(&full_frame, &full);
        assert_eq!(estimate.per_bin(), None);
    }
}
