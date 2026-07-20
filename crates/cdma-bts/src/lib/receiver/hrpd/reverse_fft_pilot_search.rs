//! Signal-shape-agnostic FFT pilot/preamble search for HRPD reverse-link
//! acquisition.
//!
//! This primitive runs a single-frame matched-filter correlation against an
//! oversampled IQ window using FFT/IFFT, returns the top peaks above an SNR
//! threshold, and is intended to be wrapped by channel-specific correlators
//! (access today, reverse traffic tomorrow). The primitive owns FFT planners
//! and reusable buffers; the wrapper owns channel-specific concepts like
//! access cycle numbers, sector IDs, etc., and supplies the reference chip
//! template through the [`HrpdReversePilotReference`] trait.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use num_complex::Complex32;

/// The HRPD reverse pilot reference (short PN × long code, C.S0024
/// §9.2.1.3.8) repeats every short-PN period: `reverse_spread` reduces both
/// the PN and long-code phase `mod 32768`, so a reference built for chip `c`
/// is byte-identical to one for `c + 32768`. The reference-spectrum cache is
/// therefore keyed on `window_start_chip % HRPD_REVERSE_PILOT_PERIOD_CHIPS`.
const HRPD_REVERSE_PILOT_PERIOD_CHIPS: u64 = 32768;
use rustfft::{Fft, FftPlanner};

/// Per-section timing counters for the FFT pilot searcher. Accumulated by
/// `scan_top_hits` / `scan_window` / `reference_spectrum_for_window` so
/// callers can profile where the heat goes without external profilers. All
/// counts are in nanoseconds; divide by the matching `*_calls` to get averages.
#[derive(Debug, Default, Clone, Copy)]
pub struct HrpdReverseFftPilotSearcherStats {
    /// Reference template build (chip generator + upsample + reference FFT).
    pub ref_setup_ns: u64,
    pub ref_setup_calls: u64,
    /// Signal copy + zero-pad + forward FFT.
    pub signal_fft_ns: u64,
    /// Pointwise multiply + IFFT.
    pub ifft_mult_ns: u64,
    /// Linear peak find + top-N selection.
    pub peak_find_ns: u64,
    /// Number of `scan_window` invocations.
    pub scan_window_calls: u64,
}

impl HrpdReverseFftPilotSearcherStats {
    pub fn ref_setup_avg_us(&self) -> u64 {
        if self.ref_setup_calls == 0 {
            return 0;
        }
        self.ref_setup_ns / self.ref_setup_calls / 1_000
    }

    pub fn signal_fft_avg_us(&self) -> u64 {
        if self.scan_window_calls == 0 {
            return 0;
        }
        self.signal_fft_ns / self.scan_window_calls / 1_000
    }

    pub fn ifft_mult_avg_us(&self) -> u64 {
        if self.scan_window_calls == 0 {
            return 0;
        }
        self.ifft_mult_ns / self.scan_window_calls / 1_000
    }

    pub fn peak_find_avg_us(&self) -> u64 {
        if self.scan_window_calls == 0 {
            return 0;
        }
        self.peak_find_ns / self.scan_window_calls / 1_000
    }
}

/// Generator for the per-window FFT reference template.
///
/// The primitive calls this once per window with the absolute starting chip
/// of the IQ window so the implementor can pick channel-specific keying
/// (e.g. HRPD access cycle number from the chip timeline) before building
/// the template. The returned slice is treated as one chip-rate frame at
/// logical phase 0 of the template; the search produces peaks at sample
/// delays where the window aligns with the template.
pub trait HrpdReversePilotReference: Send + Sync {
    /// Build `len` chip-rate reference values for the FFT template given
    /// the window's absolute starting chip.
    fn template_chips(&self, window_start_chip: u64, len: usize) -> Vec<Complex32>;

    /// Cache key for the template at this window position. Two windows may
    /// share a cached spectrum only when this key matches. The default suits
    /// references that depend solely on the short-PN phase; a reference with
    /// additional window-position dependence (e.g. the access cycle number)
    /// must fold it in.
    fn reference_cache_key(&self, window_start_chip: u64) -> u64 {
        window_start_chip % HRPD_REVERSE_PILOT_PERIOD_CHIPS
    }
}

#[derive(Clone, Debug)]
pub struct HrpdReverseFftPilotSearchConfig {
    /// Samples per chip.
    pub oversample: usize,
    /// Frame length in chips. The reference template is one frame; the
    /// search window is an integer multiple of frames.
    pub frame_chips: usize,
    /// Number of frames in the search window.
    pub search_window_frames: usize,
    /// Frames advanced between successive windows.
    pub search_step_frames: usize,
    /// Peak/mean linear ratio threshold for emitting a hit.
    pub snr_threshold: f32,
    /// Maximum number of hits returned per window scan.
    pub max_hits_per_window: usize,
    /// Minimum sample-delay separation between retained hits, in chips.
    /// Collisions are resolved by keeping the higher-power peak.
    pub hit_suppression_chips: usize,
}

impl Default for HrpdReverseFftPilotSearchConfig {
    fn default() -> Self {
        Self {
            oversample: 4,
            frame_chips: 1024,
            search_window_frames: 4,
            search_step_frames: 1,
            snr_threshold: 20.0,
            max_hits_per_window: 8,
            hit_suppression_chips: 512,
        }
    }
}

/// One FFT search hit. Channel-agnostic — wrappers add their own keying
/// (ACN, mask identifiers, etc.) outside this primitive.
#[derive(Clone, Debug)]
pub struct HrpdReverseFftPilotHit {
    pub snr: f32,
    pub snr_db: f32,
    pub peak_power: f32,
    pub mean_power: f32,
    pub window_index: usize,
    pub window_sample_offset: usize,
    pub delay_samples: usize,
    pub preamble_start_sample: u64,
    pub preamble_start_chip: u64,
    pub sample_phase: usize,
    pub frame_phase_chips: u64,
}

/// FFT search engine. Construct once per RX worker and reuse across blocks.
pub struct HrpdReverseFftPilotSearcher {
    cfg: HrpdReverseFftPilotSearchConfig,
    fft_len: usize,
    frame_samples: usize,
    window_samples: usize,
    step_samples: usize,
    valid_delay_samples: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    /// Pre-FFT'd reference spectra keyed by `window_start_chip % period`. The
    /// reference repeats every short-PN period, so this collapses to a small
    /// finite set of entries and the per-window rebuild becomes one build per
    /// distinct phase.
    ref_cache: HashMap<u64, Arc<Vec<Complex32>>>,
    signal_spectrum: Vec<Complex32>,
    corr: Vec<Complex32>,
    scratch: Vec<Complex32>,
    stats: HrpdReverseFftPilotSearcherStats,
}

impl HrpdReverseFftPilotSearcher {
    pub fn new(cfg: HrpdReverseFftPilotSearchConfig) -> Self {
        assert!(cfg.oversample > 0, "oversample must be nonzero");
        assert!(cfg.frame_chips > 0, "frame_chips must be nonzero");
        assert!(
            cfg.search_window_frames > 0,
            "search_window_frames must be nonzero"
        );
        assert!(
            cfg.search_step_frames > 0,
            "search_step_frames must be nonzero"
        );
        let frame_samples = cfg.frame_chips * cfg.oversample;
        let window_samples = frame_samples * cfg.search_window_frames;
        let fft_len = window_samples.next_power_of_two();
        let step_samples = frame_samples * cfg.search_step_frames;
        let valid_delay_samples = window_samples.saturating_sub(frame_samples);

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_len);
        let ifft = planner.plan_fft_inverse(fft_len);
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());

        Self {
            cfg,
            fft_len,
            frame_samples,
            window_samples,
            step_samples,
            valid_delay_samples,
            fft,
            ifft,
            ref_cache: HashMap::new(),
            signal_spectrum: vec![Complex32::new(0.0, 0.0); fft_len],
            corr: vec![Complex32::new(0.0, 0.0); fft_len],
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            stats: HrpdReverseFftPilotSearcherStats::default(),
        }
    }

    pub fn stats(&self) -> HrpdReverseFftPilotSearcherStats {
        self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = HrpdReverseFftPilotSearcherStats::default();
    }

    pub fn config(&self) -> &HrpdReverseFftPilotSearchConfig {
        &self.cfg
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    pub fn window_samples(&self) -> usize {
        self.window_samples
    }

    pub fn fft_len(&self) -> usize {
        self.fft_len
    }

    pub fn snr_threshold(&self) -> f32 {
        self.cfg.snr_threshold
    }

    /// Scan all windows in `samples` and return up to `top_n` hits ranked
    /// by SNR (highest first). Hits below `snr_threshold` are dropped.
    pub fn scan_top_hits<R: HrpdReversePilotReference + ?Sized>(
        &mut self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        top_n: usize,
        reference: &R,
    ) -> Vec<HrpdReverseFftPilotHit> {
        let top_n = top_n.max(1);
        if samples.len() < self.window_samples {
            return Vec::new();
        }

        let mut hits = Vec::new();
        let mut window_index = 0usize;
        let mut offset = 0usize;
        while offset + self.window_samples <= samples.len() {
            let window_abs_sample = absolute_sample_start + offset as u64;
            let window_start_chip = window_abs_sample / self.cfg.oversample as u64;
            let reference_spectrum =
                self.reference_spectrum_for_window(window_start_chip, reference);
            hits.extend(self.scan_window(
                samples,
                absolute_sample_start,
                window_index,
                offset,
                &reference_spectrum,
            ));
            window_index += 1;
            offset += self.step_samples;
        }

        hits.sort_by(|a, b| b.snr.total_cmp(&a.snr));
        hits.truncate(top_n);
        if let Some(best) = hits.first() {
            log::trace!(
                "HRPD reverse FFT pilot search: best_snr={:.2}x/{:.2}dB threshold={:.2}x peak={:.3e} mean={:.3e} window={} delay_samples={} start_sample={} start_chip={} sample_phase={} frame_phase={}",
                best.snr,
                best.snr_db,
                self.cfg.snr_threshold,
                best.peak_power,
                best.mean_power,
                best.window_index,
                best.delay_samples,
                best.preamble_start_sample,
                best.preamble_start_chip,
                best.sample_phase,
                best.frame_phase_chips,
            );
        }
        hits
    }

    /// Reference spectrum for a window, built on first use at each phase and
    /// cached thereafter. The reference chooses the cache key; the default is
    /// `window_start_chip % period` for phase-periodic references, and
    /// references whose template also depends on window position (e.g. the
    /// access preamble's access cycle number) must fold that into the key so
    /// two windows never share a spectrum built for different sequences.
    fn reference_spectrum_for_window<R: HrpdReversePilotReference + ?Sized>(
        &mut self,
        window_start_chip: u64,
        reference: &R,
    ) -> Arc<Vec<Complex32>> {
        let key = reference.reference_cache_key(window_start_chip);
        if let Some(spectrum) = self.ref_cache.get(&key) {
            return spectrum.clone();
        }
        let t = Instant::now();
        let template = reference.template_chips(window_start_chip, self.cfg.frame_chips);
        // Build the zero-padded oversampled time-domain template, then FFT it.
        let mut spectrum = vec![Complex32::new(0.0, 0.0); self.fft_len];
        for (chip_idx, chip) in template.into_iter().enumerate().take(self.cfg.frame_chips) {
            let sample_base = chip_idx * self.cfg.oversample;
            for sample_offset in 0..self.cfg.oversample {
                spectrum[sample_base + sample_offset] = chip;
            }
        }
        self.fft
            .process_with_scratch(&mut spectrum, &mut self.scratch);
        self.stats.ref_setup_ns = self
            .stats
            .ref_setup_ns
            .saturating_add(t.elapsed().as_nanos() as u64);
        self.stats.ref_setup_calls = self.stats.ref_setup_calls.saturating_add(1);
        let spectrum = Arc::new(spectrum);
        self.ref_cache.insert(key, spectrum.clone());
        spectrum
    }

    /// Drop all cached reference spectra. Call when the wrapper's channel
    /// parameters change (e.g. a new long-code mask) so stale references are
    /// not reused.
    pub fn invalidate_reference(&mut self) {
        self.ref_cache.clear();
    }

    fn scan_window(
        &mut self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        window_index: usize,
        window_sample_offset: usize,
        reference_spectrum: &[Complex32],
    ) -> Vec<HrpdReverseFftPilotHit> {
        self.stats.scan_window_calls = self.stats.scan_window_calls.saturating_add(1);

        let t_sig = Instant::now();
        self.signal_spectrum.fill(Complex32::new(0.0, 0.0));
        self.signal_spectrum[..self.window_samples].copy_from_slice(
            &samples[window_sample_offset..window_sample_offset + self.window_samples],
        );
        self.fft
            .process_with_scratch(&mut self.signal_spectrum, &mut self.scratch);
        self.stats.signal_fft_ns = self
            .stats
            .signal_fft_ns
            .saturating_add(t_sig.elapsed().as_nanos() as u64);

        let t_ifft = Instant::now();
        for ((dst, signal), reference) in self
            .corr
            .iter_mut()
            .zip(&self.signal_spectrum)
            .zip(reference_spectrum)
        {
            *dst = *signal * reference.conj();
        }
        self.ifft
            .process_with_scratch(&mut self.corr, &mut self.scratch);
        self.stats.ifft_mult_ns = self
            .stats
            .ifft_mult_ns
            .saturating_add(t_ifft.elapsed().as_nanos() as u64);

        let t_peak = Instant::now();
        let valid = self.valid_delay_samples + 1;
        let mut peak_power = 0.0f32;
        let mut sum_power = 0.0f32;
        for sample in self.corr.iter().take(valid) {
            let power = sample.norm_sqr();
            sum_power += power;
            if power > peak_power {
                peak_power = power;
            }
        }
        let mean_power = sum_power / valid as f32;
        if !(mean_power > 0.0 && peak_power.is_finite() && mean_power.is_finite()) {
            self.stats.peak_find_ns = self
                .stats
                .peak_find_ns
                .saturating_add(t_peak.elapsed().as_nanos() as u64);
            return Vec::new();
        }

        let min_power = self.cfg.snr_threshold * mean_power;
        let suppression_samples =
            self.cfg.hit_suppression_chips.max(1) * self.cfg.oversample.max(1);
        let max_hits = self.cfg.max_hits_per_window.max(1);
        let mut selected_peaks: Vec<(usize, f32)> = Vec::with_capacity(max_hits);
        for (peak_delay, sample) in self.corr.iter().take(valid).enumerate() {
            let power = sample.norm_sqr();
            if !(power >= min_power && power.is_finite()) {
                continue;
            }

            if let Some((idx, prior)) =
                selected_peaks
                    .iter_mut()
                    .enumerate()
                    .find(|(_, (prior_delay, _))| {
                        prior_delay.abs_diff(peak_delay) < suppression_samples
                    })
            {
                if power > prior.1 {
                    selected_peaks[idx] = (peak_delay, power);
                }
                continue;
            }

            if selected_peaks.len() < max_hits {
                selected_peaks.push((peak_delay, power));
                continue;
            }

            if let Some((weakest_idx, (_, weakest_power))) = selected_peaks
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.1.total_cmp(&b.1))
            {
                if power > *weakest_power {
                    selected_peaks[weakest_idx] = (peak_delay, power);
                }
            }
        }
        selected_peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.stats.peak_find_ns = self
            .stats
            .peak_find_ns
            .saturating_add(t_peak.elapsed().as_nanos() as u64);
        selected_peaks
            .into_iter()
            .map(|(peak_delay, power)| {
                self.make_hit(
                    absolute_sample_start,
                    window_index,
                    window_sample_offset,
                    peak_delay,
                    power,
                    mean_power,
                )
            })
            .collect()
    }

    fn make_hit(
        &self,
        absolute_sample_start: u64,
        window_index: usize,
        window_sample_offset: usize,
        peak_delay: usize,
        peak_power: f32,
        mean_power: f32,
    ) -> HrpdReverseFftPilotHit {
        let snr = peak_power / mean_power;
        let snr_db = 10.0 * snr.max(1.0e-12).log10();
        let preamble_start_sample =
            absolute_sample_start + window_sample_offset as u64 + peak_delay as u64;
        let preamble_start_chip = preamble_start_sample / self.cfg.oversample as u64;
        let frame_phase_chips = preamble_start_chip % self.cfg.frame_chips as u64;
        HrpdReverseFftPilotHit {
            snr,
            snr_db,
            peak_power,
            mean_power,
            window_index,
            window_sample_offset,
            delay_samples: peak_delay,
            preamble_start_sample,
            preamble_start_chip,
            sample_phase: (preamble_start_sample % self.cfg.oversample as u64) as usize,
            frame_phase_chips,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference impl: a fixed BPSK code reused across all windows.
    struct FixedBpskReference {
        chips: Vec<Complex32>,
    }

    impl FixedBpskReference {
        fn new(len: usize, seed: u64) -> Self {
            let mut state = seed | 1;
            let chips = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let bit = (state >> 33) & 1;
                    if bit == 0 {
                        Complex32::new(1.0, 0.0)
                    } else {
                        Complex32::new(-1.0, 0.0)
                    }
                })
                .collect();
            Self { chips }
        }
    }

    impl HrpdReversePilotReference for FixedBpskReference {
        fn template_chips(&self, _window_start_chip: u64, len: usize) -> Vec<Complex32> {
            assert_eq!(len, self.chips.len());
            self.chips.clone()
        }
    }

    fn build_oversampled_signal(
        reference_chips: &[Complex32],
        oversample: usize,
        preamble_start_sample: usize,
        total_samples: usize,
    ) -> Vec<Complex32> {
        let mut signal = vec![Complex32::new(0.0, 0.0); total_samples];
        for (chip_idx, chip) in reference_chips.iter().enumerate() {
            for s in 0..oversample {
                let idx = preamble_start_sample + chip_idx * oversample + s;
                if idx < signal.len() {
                    signal[idx] = *chip;
                }
            }
        }
        signal
    }

    #[test]
    fn finds_zero_noise_peak_at_correct_delay() {
        let frame_chips = 128;
        let oversample = 4;
        let cfg = HrpdReverseFftPilotSearchConfig {
            oversample,
            frame_chips,
            search_window_frames: 4,
            search_step_frames: 1,
            snr_threshold: 10.0,
            max_hits_per_window: 4,
            hit_suppression_chips: 32,
        };
        let mut searcher = HrpdReverseFftPilotSearcher::new(cfg);
        let reference = FixedBpskReference::new(frame_chips, 0xdeadbeef);
        let window_samples = searcher.window_samples();
        let preamble_offset_samples = frame_chips * oversample; // start at chip 128
        let signal = build_oversampled_signal(
            &reference.chips,
            oversample,
            preamble_offset_samples,
            window_samples,
        );

        let hits = searcher.scan_top_hits(&signal, 0, 4, &reference);
        assert!(!hits.is_empty(), "expected at least one hit");
        let best = &hits[0];
        assert_eq!(best.delay_samples, preamble_offset_samples);
        assert!(
            best.snr > 100.0,
            "snr should be very high, got {}",
            best.snr
        );
        assert_eq!(best.preamble_start_sample, preamble_offset_samples as u64);
        assert_eq!(
            best.preamble_start_chip,
            (preamble_offset_samples / oversample) as u64
        );
    }

    #[test]
    fn threshold_gating_suppresses_low_snr_hits() {
        let frame_chips = 128;
        let oversample = 4;
        let cfg = HrpdReverseFftPilotSearchConfig {
            oversample,
            frame_chips,
            search_window_frames: 4,
            search_step_frames: 1,
            // Set a threshold higher than the achievable peak/mean for a
            // pure-noise input.
            snr_threshold: 1.0e6,
            max_hits_per_window: 4,
            hit_suppression_chips: 32,
        };
        let mut searcher = HrpdReverseFftPilotSearcher::new(cfg);
        let reference = FixedBpskReference::new(frame_chips, 0xfeedface);
        let window_samples = searcher.window_samples();
        let signal = build_oversampled_signal(
            &reference.chips,
            oversample,
            frame_chips * oversample,
            window_samples,
        );

        let hits = searcher.scan_top_hits(&signal, 0, 4, &reference);
        assert!(
            hits.is_empty(),
            "no hit should clear the very high threshold"
        );
    }

    #[test]
    fn max_hits_per_window_caps_returned_hits() {
        let frame_chips = 64;
        let oversample = 4;
        let cfg = HrpdReverseFftPilotSearchConfig {
            oversample,
            frame_chips,
            search_window_frames: 8,
            search_step_frames: 1,
            snr_threshold: 5.0,
            max_hits_per_window: 2,
            hit_suppression_chips: 4,
        };
        let mut searcher = HrpdReverseFftPilotSearcher::new(cfg);
        let reference = FixedBpskReference::new(frame_chips, 0x12345);
        let window_samples = searcher.window_samples();
        let mut signal = vec![Complex32::new(0.0, 0.0); window_samples];
        // Plant the same reference at three well-separated delays. Without
        // the cap there would be three above-threshold peaks; with the cap
        // we should see only two.
        let starts = [
            frame_chips * oversample,
            3 * frame_chips * oversample,
            5 * frame_chips * oversample,
        ];
        for start in starts {
            for (chip_idx, chip) in reference.chips.iter().enumerate() {
                for s in 0..oversample {
                    let idx = start + chip_idx * oversample + s;
                    if idx < signal.len() {
                        signal[idx] += *chip;
                    }
                }
            }
        }
        let hits = searcher.scan_top_hits(&signal, 0, 8, &reference);
        // top_n is 8 but max_hits_per_window=2, single window → at most 2
        assert!(
            hits.len() <= 2,
            "expected at most 2 hits, got {}",
            hits.len()
        );
        assert!(!hits.is_empty(), "expected at least one hit");
    }

    #[test]
    fn hit_suppression_collapses_nearby_peaks() {
        let frame_chips = 64;
        let oversample = 4;
        let cfg = HrpdReverseFftPilotSearchConfig {
            oversample,
            frame_chips,
            search_window_frames: 4,
            search_step_frames: 1,
            snr_threshold: 5.0,
            max_hits_per_window: 8,
            // Very large suppression: peaks within `frame_chips` chips
            // collapse to a single hit.
            hit_suppression_chips: frame_chips,
        };
        let mut searcher = HrpdReverseFftPilotSearcher::new(cfg);
        let reference = FixedBpskReference::new(frame_chips, 0xabcdef);
        let window_samples = searcher.window_samples();
        let mut signal = vec![Complex32::new(0.0, 0.0); window_samples];
        // Two preamble plants spaced 1 sample apart should collapse to one
        // retained hit under aggressive suppression.
        let base = frame_chips * oversample;
        for offset in [0usize, 1usize] {
            let start = base + offset;
            for (chip_idx, chip) in reference.chips.iter().enumerate() {
                for s in 0..oversample {
                    let idx = start + chip_idx * oversample + s;
                    if idx < signal.len() {
                        signal[idx] += *chip;
                    }
                }
            }
        }
        let hits = searcher.scan_top_hits(&signal, 0, 8, &reference);
        assert_eq!(hits.len(), 1, "expected suppression to keep one hit");
    }

    #[test]
    #[ignore = "release-only production-size timing benchmark"]
    fn benchmark_production_traffic_search() {
        const FRAME_CHIPS: usize = 32768;
        const OVERSAMPLE: usize = 4;
        const ITERATIONS: u32 = 8;
        let cfg = HrpdReverseFftPilotSearchConfig {
            oversample: OVERSAMPLE,
            frame_chips: FRAME_CHIPS,
            search_window_frames: 2,
            search_step_frames: 2,
            snr_threshold: 10.0,
            max_hits_per_window: 1,
            hit_suppression_chips: FRAME_CHIPS / 4,
        };
        let mut searcher = HrpdReverseFftPilotSearcher::new(cfg);
        let reference = FixedBpskReference::new(FRAME_CHIPS, 0xfeed_face_dead_beef);
        let signal = build_oversampled_signal(
            &reference.chips,
            OVERSAMPLE,
            FRAME_CHIPS * OVERSAMPLE / 2,
            searcher.window_samples(),
        );

        // Warm the FFT plans and reference-spectrum cache before measuring.
        let _ = searcher.scan_top_hits(&signal, 0, 1, &reference);
        searcher.reset_stats();
        let started = Instant::now();
        for _ in 0..ITERATIONS {
            let hits = searcher.scan_top_hits(&signal, 0, 1, &reference);
            assert!(!hits.is_empty());
        }
        let elapsed = started.elapsed();
        let avg_us = elapsed.as_micros() as f64 / f64::from(ITERATIONS);
        let air_step_us = 2.0 * FRAME_CHIPS as f64 / 1_228_800.0 * 1_000_000.0;
        let stats = searcher.stats();
        eprintln!(
            "production traffic FFT: avg={avg_us:.0}us realtime={:.2}x signal_fft={}us ifft_mult={}us peak_find={}us fft_len={}",
            air_step_us / avg_us,
            stats.signal_fft_avg_us(),
            stats.ifft_mult_avg_us(),
            stats.peak_find_avg_us(),
            searcher.fft_len(),
        );
    }
}
