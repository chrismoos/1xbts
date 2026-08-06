//! # PnLcCorrelator — Joint PN×LC FFT Correlator
//!
//! Implements [`Correlator`] for the [`GenericRakeReceiver`].  Searches the
//! signal for a composite PN×LC spreading sequence using FFT cross-correlation
//! and emits [`PnLcFinger`]s for each detection.
//!
//! ## Search algorithm
//!
//! Every `search_interval_windows` input windows the correlator:
//!
//! 1. FFTs the most recent `coherent_chips × oversample` input samples.
//! 2. Sweeps `lc_phase` in `[-lc_half_span, +lc_half_span]` chips:
//!    - Builds composite reference `pn_iq[k] × lc_sign[k]` for that LC phase.
//!    - FFT cross-correlates with the signal → inverse FFT → magnitude².
//!    - Tracks the global `(delay, lc_phase)` maximum.
//! 3. Computes SNR = best_power / average_power.
//! 4. If SNR ≥ `snr_threshold` **and** the delay is not already tracked by an
//!    active finger, emits a new [`PnLcFinger`].
//!
//! ## Finger despreading
//!
//! Each [`PnLcFinger`] independently despreads the **raw** oversampled IQ
//! stream it receives via [`RakeFinger::process`]:
//!
//! - PN conjugate multiplication at every sample.
//! - Estimate one chip-rate value either from the prompt sample or by
//!   integrate-and-dump across the oversampled chip interval.
//! - LC sign removal at each chip boundary (generator advances in lockstep).
//! - CFO tracking via pilot coherence across 256-chip blocks.
//! - Output in 256-chip blocks to the sub-chain (Walsh demod → Viterbi → …).

mod config;
mod finger;
mod interpolation;

pub use config::PnLcConfig;
pub use finger::PnLcFinger;

use std::collections::VecDeque;
use std::sync::Arc;

use log::{debug, info, trace};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::pipelined::{
    PipelineProcessorShared, SampleBlock, build_fft_search_pn_samples_with_kind,
    build_oqpsk_pn_samples_with_kind,
};

use super::generic_rake_receiver::Correlator;

use finger::{
    ADAPTIVE_FINGER_TIMING_LATE_SNR_THRESHOLD, ActiveFingerState, DEFAULT_REACQUIRE_CRC_MISS_COUNT,
    DEFAULT_REACQUIRE_IDLE_CHIPS, DEFAULT_REACQUIRE_SIGNAL_LOST_CHIPS,
    MAX_PENDING_ATTEMPTS_WITH_HIT, MAX_PENDING_ATTEMPTS_WITHOUT_HIT,
    PLAIN_CFO_REFINE_MAX_DELTA_RAD_PER_CHIP, PendingCandidate, PnReferenceKind,
};
use interpolation::interp_complex_wrapped;

// PnLcConfig → config.rs

// ---------------------------------------------------------------------------
// PnLcCorrelator
// ---------------------------------------------------------------------------

/// Searches for a PN×LC spreading sequence using joint FFT correlation and
/// emits [`PnLcFinger`]s for each new detection.
///
/// Plug into [`GenericRakeReceiver`]:
///
/// ```rust,ignore
/// let correlator = PnLcCorrelator::new(
///     PnLcConfig::default_4x(),
///     LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41),
///     chain_builder,
/// );
/// let rake = GenericRakeReceiver::new(correlator).with_max_fingers(4);
/// ```
pub struct PnLcCorrelator {
    cfg: PnLcConfig,

    /// Shared PN conjugate reference for the coarse FFT search.
    pn_fft_seq: Arc<Vec<Complex32>>,
    /// Shared PN conjugate reference for stage-2 verify and finger despread.
    pn_despread_seq: Arc<Vec<Complex32>>,
    phase_period: usize,

    /// Template LC generator used to seed independent cursors and fingers.
    lc_template: LongCodeGenerator,
    /// Search-side LC cursor used to generate PN×LC reference signs
    /// incrementally across searches.
    search_lc_gen: LongCodeGenerator,
    search_lc_next_chip: usize,
    /// Optional explicit Q long-code generator template for HPSK channels
    /// whose Q mask is not simply recovered from the previous I-code chip.
    q_lc_template: Option<LongCodeGenerator>,

    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,

    /// Segment-sized FFT plans for noncoherent accumulation.
    /// Only used when `cfg.noncoherent_segments > 1`.
    seg_fft_fwd: Arc<dyn Fft<f32>>,
    seg_fft_inv: Arc<dyn Fft<f32>>,
    seg_fft_scratch: Vec<Complex32>,
    seg_len: usize,

    /// Pre-allocated scratch buffers for `run_joint_search()`.
    search_ref_buf: Vec<Complex32>,
    search_nc_power: Vec<f32>,
    search_ref_seg: Vec<Complex32>,
    search_result_buf: Vec<Complex32>,
    search_lc_signs: Vec<f32>,
    search_cfo_hypotheses: Vec<f32>,
    search_cfo_phasors: Vec<Vec<Complex32>>,
    search_cfo_signal_ffts: Vec<Vec<Complex32>>,

    /// Builds the signal-processing chain for each newly spawned finger.
    chain_builder: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,

    /// Raw IQ sample accumulation buffer.
    buffer: Vec<Complex32>,
    /// Recent raw IQ history so newly spawned fingers can replay from the
    /// first verified PN+LC hit instead of starting only on live input.
    recent_samples: VecDeque<Complex32>,
    recent_start_offset: usize,
    /// Total samples consumed from the stream (for abs-chip calculation).
    samples_consumed: usize,
    /// Input window count (gates search interval).
    window_counter: u64,

    /// Absolute sample index of the stream origin (from block tags).
    absolute_origin_sample: Option<usize>,

    /// Active finger ids and their detected delays (for deduplication).
    active_fingers: Vec<ActiveFingerState>,
    pending_candidates: Vec<PendingCandidate>,
    next_finger_id: u64,

    sample_rate_hz: f64,

    /// When true, skip FFT search and candidate verification (a finger has
    /// been hard-validated so there is no need to keep searching).
    search_paused: bool,

    reacquire_signal_lost_chips: u64,
    reacquire_crc_miss_count: u64,
    reacquire_idle_chips: u64,
}

impl PnLcCorrelator {
    /// Create a new correlator.
    ///
    /// - `cfg`: search parameters
    /// - `lc_template`: LC generator at state 0; the correlator clones and
    ///   advances it for each search hypothesis
    /// - `chain_builder`: factory called once per detection to create the
    ///   sub-pipeline (Walsh demod → Viterbi → …) for the new finger
    pub fn new(
        cfg: PnLcConfig,
        lc_template: LongCodeGenerator,
        chain_builder: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,
    ) -> Self {
        let os = cfg.oversample;
        let phase_period = 32768 * os;
        let pn_fft_seq: Arc<Vec<Complex32>> = Arc::new(
            build_fft_search_pn_samples_with_kind(phase_period, os, cfg.short_code_reference)
                .into_iter()
                .map(|s| Complex32::new(s.re, -s.im))
                .collect(),
        );
        let pn_despread_seq: Arc<Vec<Complex32>> = Arc::new(if cfg.split_pn_reference {
            build_oqpsk_pn_samples_with_kind(phase_period, os, cfg.short_code_reference)
                .into_iter()
                .map(|s| Complex32::new(s.re, -s.im))
                .collect()
        } else {
            build_fft_search_pn_samples_with_kind(phase_period, os, cfg.short_code_reference)
                .into_iter()
                .map(|s| Complex32::new(s.re, -s.im))
                .collect()
        });

        let window_len = cfg.coherent_chips * os;
        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(window_len);
        let fft_inv = planner.plan_fft_inverse(window_len);
        let scratch_len = fft_fwd
            .get_inplace_scratch_len()
            .max(fft_inv.get_inplace_scratch_len());

        // Segment-sized FFT plans for noncoherent accumulation.
        let n_seg = cfg.noncoherent_segments.max(1);
        let seg_chips = cfg.coherent_chips / n_seg;
        assert_eq!(
            seg_chips * n_seg,
            cfg.coherent_chips,
            "coherent_chips must be divisible by noncoherent_segments"
        );
        let seg_len = seg_chips * os;
        let seg_fft_fwd = planner.plan_fft_forward(seg_len);
        let seg_fft_inv = planner.plan_fft_inverse(seg_len);
        let seg_scratch_len = seg_fft_fwd
            .get_inplace_scratch_len()
            .max(seg_fft_inv.get_inplace_scratch_len());

        // With large coherent windows, carrier frequency offset destroys the
        // coherent sum. Keep the same fixed CFO grid as the search path, but
        // precompute phasors and allocate the rotated signal FFT buffers once.
        let cfo_step: f32 = 0.0005;
        let cfo_half_count = if cfg.coherent_chips > 1024 { 1i32 } else { 0 };
        let search_cfo_hypotheses: Vec<f32> = ((-cfo_half_count)..=cfo_half_count)
            .map(|i| i as f32 * cfo_step)
            .collect();
        let search_cfo_phasors: Vec<Vec<Complex32>> = search_cfo_hypotheses
            .iter()
            .map(|&cfo| {
                if cfo == 0.0 {
                    Vec::new()
                } else {
                    (0..window_len)
                        .map(|n| {
                            let angle = cfo * n as f32;
                            Complex32::new(angle.cos(), angle.sin())
                        })
                        .collect()
                }
            })
            .collect();
        let search_cfo_signal_ffts =
            vec![vec![Complex32::new(0.0, 0.0); window_len]; search_cfo_hypotheses.len()];
        let search_lc_signs_capacity = cfg.coherent_chips + 2 * cfg.lc_half_span as usize + 2;

        Self {
            cfg,
            pn_fft_seq,
            pn_despread_seq,
            phase_period,
            search_lc_gen: lc_template.clone(),
            search_lc_next_chip: 0,
            lc_template,
            q_lc_template: None,
            fft_fwd,
            fft_inv,
            fft_scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            seg_fft_fwd,
            seg_fft_inv,
            seg_fft_scratch: vec![Complex32::new(0.0, 0.0); seg_scratch_len],
            seg_len,
            search_ref_buf: vec![Complex32::new(0.0, 0.0); window_len],
            search_nc_power: vec![0.0f32; seg_len],
            search_ref_seg: vec![Complex32::new(0.0, 0.0); seg_len],
            search_result_buf: vec![Complex32::new(0.0, 0.0); window_len.max(seg_len)],
            search_lc_signs: Vec::with_capacity(search_lc_signs_capacity),
            search_cfo_hypotheses,
            search_cfo_phasors,
            search_cfo_signal_ffts,
            chain_builder,
            buffer: Vec::new(),
            recent_samples: VecDeque::new(),
            recent_start_offset: 0,
            samples_consumed: 0,
            window_counter: 0,
            absolute_origin_sample: None,
            active_fingers: Vec::new(),
            pending_candidates: Vec::new(),
            next_finger_id: 1,
            sample_rate_hz: 0.0,
            search_paused: false,
            reacquire_signal_lost_chips: DEFAULT_REACQUIRE_SIGNAL_LOST_CHIPS,
            reacquire_crc_miss_count: DEFAULT_REACQUIRE_CRC_MISS_COUNT,
            reacquire_idle_chips: DEFAULT_REACQUIRE_IDLE_CHIPS,
        }
    }

    /// Set an explicit Q long-code template for HPSK composite spreading.
    /// Leave unset for legacy 1x RC3 behavior where the Q branch is modeled
    /// from the I-code delay already used by existing traffic tests.
    pub fn with_q_lc_template(mut self, q_lc_template: LongCodeGenerator) -> Self {
        self.q_lc_template = Some(q_lc_template);
        self
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// PN phase at the start of a window, compensated for filter delay.
    fn base_phase(&self, window_offset: usize) -> usize {
        let abs = self.absolute_origin_sample.unwrap_or(0);
        let pp = self.phase_period;
        let delay = self.cfg.composite_filter_delay % pp;
        (abs + window_offset + pp - delay) % pp
    }

    /// Absolute TX chip at a given sample position, accounting for filter delay.
    fn abs_chip_at(&self, sample_offset: usize) -> usize {
        let abs = self.absolute_origin_sample.unwrap_or(0);
        (abs + sample_offset).saturating_sub(self.cfg.composite_filter_delay) / self.cfg.oversample
    }

    fn seed_lc_from_template(
        &self,
        template: &LongCodeGenerator,
        abs_chip: usize,
    ) -> LongCodeGenerator {
        let mut lc = template.clone();
        let advance = if let Some(period) = self.cfg.lc_period_chips {
            lc.set_state(self.cfg.lc_period_initial_state);
            abs_chip % period
        } else {
            abs_chip
        };
        lc.advance_chips(advance);
        lc
    }

    fn seed_i_lc(&self, abs_chip: usize) -> LongCodeGenerator {
        self.seed_lc_from_template(&self.lc_template, abs_chip)
    }

    fn seed_q_lc(&self, abs_chip: usize) -> LongCodeGenerator {
        let template = self.q_lc_template.as_ref().unwrap_or(&self.lc_template);
        self.seed_lc_from_template(template, abs_chip)
    }

    fn lc_signs_from_template(
        &self,
        template: &LongCodeGenerator,
        abs_chip_start: usize,
        count: usize,
    ) -> Vec<f32> {
        let Some(period) = self.cfg.lc_period_chips else {
            let mut lc = self.seed_lc_from_template(template, abs_chip_start);
            return (0..count)
                .map(|_| if lc.next_chip() == 1 { -1.0 } else { 1.0 })
                .collect();
        };

        let mut out = Vec::with_capacity(count);
        let mut produced = 0usize;
        while produced < count {
            let abs_chip = abs_chip_start + produced;
            let phase = abs_chip % period;
            let run = (period - phase).min(count - produced);
            let mut lc = template.clone();
            lc.set_state(self.cfg.lc_period_initial_state);
            lc.advance_chips(phase);
            for _ in 0..run {
                out.push(if lc.next_chip() == 1 { -1.0 } else { 1.0 });
            }
            produced += run;
        }
        out
    }

    fn i_lc_signs_from(&self, abs_chip_start: usize, count: usize) -> Vec<f32> {
        self.lc_signs_from_template(&self.lc_template, abs_chip_start, count)
    }

    fn q_lc_signs_from(&self, abs_chip_start: usize, count: usize) -> Vec<f32> {
        if let Some(template) = self.q_lc_template.as_ref() {
            return self.lc_signs_from_template(template, abs_chip_start, count);
        }

        // Legacy 1x RC3 HPSK has no independent Q-mask in this correlator.
        // Match the mainline behavior: the Q branch uses the previous I long
        // code chip, with an all-zero/+1 boundary before chip 0.
        if count == 0 {
            return Vec::new();
        }
        if abs_chip_start == 0 {
            let mut signs = Vec::with_capacity(count);
            signs.push(1.0);
            signs.extend(self.lc_signs_from_template(&self.lc_template, 0, count - 1));
            return signs;
        }
        self.lc_signs_from_template(&self.lc_template, abs_chip_start - 1, count)
    }

    /// Reset correlator state that cannot safely span a hardware-sample
    /// discontinuity.
    ///
    /// When the SDR drops samples, `absolute_sample_start` jumps forward
    /// relative to the correlator's internal sample counter.  Any buffered
    /// IQ, replay history, or candidate verification state that straddles the
    /// seam is no longer phase-consistent, so it must be discarded before we
    /// continue searching with the re-anchored origin.
    fn handle_stream_discontinuity(&mut self, buffer_leftover: &mut usize) {
        let discarded = self.buffer.len();
        self.buffer.clear();
        self.samples_consumed += discarded;
        self.recent_samples.clear();
        self.pending_candidates.clear();
        self.active_fingers.clear();
        self.search_paused = false;
        // Force an immediate post-gap search opportunity on the next full
        // window instead of waiting for the previous cadence to come around.
        self.window_counter = 1;
        *buffer_leftover = 0;
    }

    /// Sub-sample center within each oversample period.
    #[cfg(test)]
    fn center_offset(&self) -> usize {
        self.cfg
            .center_offset_override
            .unwrap_or(self.cfg.composite_filter_delay % self.cfg.oversample)
    }

    fn pn_despread_with_reference(
        &self,
        pn_reference: &[Complex32],
        block: &[Complex32],
        delay_samples: i32,
        base_phase: usize,
        n_chips: usize,
    ) -> Vec<Complex32> {
        let os = self.cfg.oversample;
        let window_len = block.len();
        let pp = self.phase_period;
        let mut out = Vec::with_capacity(n_chips);
        for k in 0..n_chips {
            let sample_idx =
                modulo(k as i32 * os as i32 + delay_samples, window_len as i32) as usize;
            let pn_idx = (base_phase + k * os) % pp;
            let pn = pn_reference[pn_idx];
            out.push(block[sample_idx] * pn);
        }
        out
    }

    fn pn_despread_with_reference_fractional(
        &self,
        pn_reference: &[Complex32],
        block: &[Complex32],
        delay_samples: i32,
        timing_mu_samples: f32,
        base_phase: usize,
        n_chips: usize,
    ) -> Vec<Complex32> {
        let os = self.cfg.oversample;
        let window_len = block.len() as f32;
        let pp = self.phase_period;
        let mut out = Vec::with_capacity(n_chips);
        for k in 0..n_chips {
            let sample_t = k as f32 * os as f32 + delay_samples as f32 + timing_mu_samples;
            let sample = interp_complex_wrapped(block, sample_t.rem_euclid(window_len));
            let pn_idx = (base_phase + k * os) % pp;
            let pn = pn_reference[pn_idx];
            out.push(sample * pn);
        }
        out
    }

    fn fractional_timing_offsets(&self) -> Vec<f32> {
        if !self.cfg.fractional_timing_recovery {
            if self.cfg.gardner_timing.enabled {
                // Gardner handles closed-loop timing after acquisition, but
                // the verifier still needs a small set of pull-in basins so a
                // weak preamble can spawn a finger close enough for the loop
                // to track. This replaces the old dense 0.125-sample sweep.
                return vec![0.0, -0.5, 0.5, -1.0, 1.0, -1.5, 1.5];
            }
            return vec![0.0];
        }
        let half = self.cfg.fractional_timing_half_samples.max(0.0);
        let step = self.cfg.fractional_timing_step_samples.max(1e-3);
        let mut offsets = vec![0.0f32];
        let mut delta = step;
        while delta <= half + step * 0.5 {
            offsets.push(delta);
            offsets.push(-delta);
            delta += step;
        }
        offsets
    }

    fn finger_timing_offsets(&self, center: f32, detection_snr: f32) -> Vec<f32> {
        let half = self.cfg.finger_timing_half_samples.max(0.0);
        if half <= f32::EPSILON {
            return vec![center];
        }

        let step = self.cfg.finger_timing_step_samples.max(1e-3);
        let mut offsets = vec![center];
        let mut delta = step;
        let prefer_late = !self.cfg.finger_timing_symmetric
            && detection_snr >= ADAPTIVE_FINGER_TIMING_LATE_SNR_THRESHOLD;
        while delta <= half + step * 0.5 {
            offsets.push(if self.cfg.finger_timing_symmetric || !prefer_late {
                center - delta
            } else {
                center + delta
            });
            if self.cfg.finger_timing_symmetric {
                offsets.push(center + delta);
            }
            delta += step;
        }
        offsets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        offsets.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        offsets.sort_by(|a, b| {
            (a - center)
                .abs()
                .partial_cmp(&(b - center).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        offsets
    }

    fn verification_references(
        &self,
        preferred: Option<PnReferenceKind>,
    ) -> Vec<(PnReferenceKind, &[Complex32])> {
        match preferred {
            Some(PnReferenceKind::Plain) => vec![(PnReferenceKind::Plain, &self.pn_fft_seq)],
            Some(PnReferenceKind::Oqpsk) => vec![(PnReferenceKind::Oqpsk, &self.pn_despread_seq)],
            None if self.cfg.split_pn_reference => vec![
                (PnReferenceKind::Plain, &self.pn_fft_seq),
                (PnReferenceKind::Oqpsk, &self.pn_despread_seq),
            ],
            None => vec![(PnReferenceKind::Plain, &self.pn_fft_seq)],
        }
    }

    fn fill_search_lc_signs(&mut self, start_chip: usize, count: usize) {
        if start_chip < self.search_lc_next_chip {
            self.search_lc_gen = self.lc_template.clone();
            self.search_lc_next_chip = 0;
        }

        self.search_lc_gen
            .advance_chips(start_chip - self.search_lc_next_chip);
        self.search_lc_next_chip = start_chip;

        self.search_lc_signs.clear();
        self.search_lc_signs.reserve(count);
        for _ in 0..count {
            let sign = if self.search_lc_gen.next_chip() == 1 {
                -1.0
            } else {
                1.0
            };
            self.search_lc_signs.push(sign);
        }
        self.search_lc_next_chip += count;
    }

    fn search_lc_with_signs(
        &self,
        pn_despread: &[Complex32],
        expected_abs_chip: usize,
    ) -> (i32, f32, f32, f32, f32) {
        let half = self.cfg.lc_half_span;
        let n = pn_despread.len();
        let lc_dec = self.cfg.lc_decimation.max(1);
        let hpsk = lc_dec >= 2;
        let lc_start = (expected_abs_chip as i64 - half as i64).max(0) as usize;
        let lc_total = (2 * half as usize) + n + 1;
        let mut lc = self.seed_i_lc(lc_start);

        if hpsk {
            // HPSK (RC3+): generate complex LC conjugate values per 2.1.3.1.17.
            //
            // The reverse RC3 complex spreading sequence (after PN removal) is:
            //   h_I(n) = (-1)^LC[2n]
            //   h_Q(n) = W(1,2)[n] × (-1)^LC[2n] × (-1)^LC[2n+1]
            //
            // where W(1,2)[n] = [+1, -1, +1, -1, ...] is the alternating sign
            // from the Walsh cover in the Q-branch spreading structure, and
            // LC[2n]/LC[2n+1] are consecutive long-code chips (I-LC and Q-LC
            // where Q-LC = I-LC delayed 1 chip, decimated by 2).
            //
            // To despread, we apply the conjugate: conj(h_I + j·h_Q).
            let lc_complex: Vec<Complex32> = (0..lc_total)
                .enumerate()
                .map(|(n, _)| {
                    let bit_i = lc.next_chip();
                    let bit_q = lc.next_chip();
                    let c_i: f32 = if bit_i == 1 { -1.0 } else { 1.0 };
                    let c_q_lc: f32 = if bit_q == 1 { -1.0 } else { 1.0 };
                    // W(1,2)[n] = +1 for even n, -1 for odd n
                    let w12: f32 = if n % 2 == 0 { 1.0 } else { -1.0 };
                    let c_q = w12 * c_i * c_q_lc;
                    // conj(c_I + j·c_Q) = c_I - j·c_Q
                    Complex32::new(c_i, -c_q)
                })
                .collect();

            return self.search_lc_complex(&lc_complex, pn_despread, half);
        }

        // IS-95/RC1/RC2: real-valued LC signs at full chip rate.
        let lc_signs: Vec<f32> = (0..lc_total)
            .map(|_| if lc.next_chip() == 1 { -1.0 } else { 1.0 })
            .collect();

        let n_seg = 4usize;
        let seg_len = n.max(1) / n_seg.max(1);
        let mut best_phase = 0i32;
        let mut best_score = f32::MIN;
        let mut second_score = f32::MIN;
        let mut best_coh_norm = 0.0f32;
        let mut best_nc_coh_norm = 0.0f32;
        let mut best_seg_pilots: Vec<Complex32> = Vec::new();

        for phase in -half..=half {
            let lc_idx_base = (phase + half) as usize;
            let mut abs_sum = 0.0f32;
            let mut coh = Complex32::new(0.0, 0.0);
            let mut nc_power_sum = 0.0f32;
            let mut seg_coh = Complex32::new(0.0, 0.0);
            let mut seg_count = 0usize;
            let mut seg_pilots = Vec::with_capacity(n_seg);

            for (i, &chip) in pn_despread.iter().enumerate() {
                let lc_idx = lc_idx_base + i;
                if lc_idx >= lc_signs.len() {
                    break;
                }
                let lc_sign = lc_signs[lc_idx];
                let d = Complex32::new(chip.re * lc_sign, chip.im * lc_sign);
                abs_sum += d.re.abs() + d.im.abs();
                coh += d;
                seg_coh += d;
                seg_count += 1;
                if seg_count >= seg_len.max(1) {
                    nc_power_sum += seg_coh.norm_sqr();
                    seg_pilots.push(seg_coh);
                    seg_coh = Complex32::new(0.0, 0.0);
                    seg_count = 0;
                }
            }
            if seg_count > 0 {
                nc_power_sum += seg_coh.norm_sqr();
                seg_pilots.push(seg_coh);
            }

            let abs_sum_safe = abs_sum.max(1e-6);
            let coh_norm = coh.norm() / abs_sum_safe;
            let nc_coh_norm = nc_power_sum.sqrt() / abs_sum_safe;
            let score = nc_coh_norm;

            if score > best_score {
                second_score = best_score;
                best_score = score;
                best_phase = phase;
                best_coh_norm = coh_norm;
                best_nc_coh_norm = nc_coh_norm;
                best_seg_pilots = seg_pilots;
            } else if score > second_score {
                second_score = score;
            }
        }

        let ratio = if second_score > 1e-6 {
            best_score / second_score
        } else {
            f32::INFINITY
        };

        // Estimate CFO from inter-segment phase rotation of best LC phase.
        let cfo_rad_per_chip = if best_seg_pilots.len() >= 2 {
            let mut total_delta = 0.0f32;
            let mut count = 0u32;
            for pair in best_seg_pilots.windows(2) {
                let cross = pair[1] * Complex32::new(pair[0].re, -pair[0].im);
                if cross.norm_sqr() > 1e-12 {
                    total_delta += cross.im.atan2(cross.re);
                    count += 1;
                }
            }
            if count > 0 {
                total_delta / (count as f32 * seg_len as f32)
            } else {
                0.0
            }
        } else {
            0.0
        };

        (
            best_phase,
            ratio,
            best_coh_norm,
            best_nc_coh_norm,
            cfo_rad_per_chip,
        )
    }

    /// HPSK complex LC despreading (RC3+ reverse link).
    ///
    /// Instead of multiplying by real ±1 signs, multiplies each PN-despread chip
    /// by conj(c_long) where c_long = c_I + j·c_Q from paired LC bits.
    fn search_lc_complex(
        &self,
        lc_complex: &[Complex32],
        pn_despread: &[Complex32],
        half: i32,
    ) -> (i32, f32, f32, f32, f32) {
        let n = pn_despread.len();
        let n_seg = 4usize;
        let seg_len = n.max(1) / n_seg.max(1);
        let mut best_phase = 0i32;
        let mut best_score = f32::MIN;
        let mut second_score = f32::MIN;
        let mut best_coh_norm = 0.0f32;
        let mut best_nc_coh_norm = 0.0f32;
        let mut best_seg_pilots: Vec<Complex32> = Vec::new();

        for phase in -half..=half {
            let lc_idx_base = (phase + half) as usize;
            let mut abs_sum = 0.0f32;
            let mut coh = Complex32::new(0.0, 0.0);
            let mut nc_power_sum = 0.0f32;
            let mut seg_coh = Complex32::new(0.0, 0.0);
            let mut seg_count = 0usize;
            let mut seg_pilots = Vec::with_capacity(n_seg);

            for (i, &chip) in pn_despread.iter().enumerate() {
                let lc_idx = lc_idx_base + i;
                if lc_idx >= lc_complex.len() {
                    break;
                }
                // Complex multiply: chip × conj(c_long)
                let lc_conj = lc_complex[lc_idx];
                let d = chip * lc_conj;
                abs_sum += d.re.abs() + d.im.abs();
                coh += d;
                seg_coh += d;
                seg_count += 1;
                if seg_count >= seg_len.max(1) {
                    nc_power_sum += seg_coh.norm_sqr();
                    seg_pilots.push(seg_coh);
                    seg_coh = Complex32::new(0.0, 0.0);
                    seg_count = 0;
                }
            }
            if seg_count > 0 {
                nc_power_sum += seg_coh.norm_sqr();
                seg_pilots.push(seg_coh);
            }

            let abs_sum_safe = abs_sum.max(1e-6);
            let coh_norm = coh.norm() / abs_sum_safe;
            let nc_coh_norm = nc_power_sum.sqrt() / abs_sum_safe;
            let score = nc_coh_norm;

            if score > best_score {
                second_score = best_score;
                best_score = score;
                best_phase = phase;
                best_coh_norm = coh_norm;
                best_nc_coh_norm = nc_coh_norm;
                best_seg_pilots = seg_pilots;
            } else if score > second_score {
                second_score = score;
            }
        }

        let ratio = if second_score > 1e-6 {
            best_score / second_score
        } else {
            f32::INFINITY
        };

        let cfo_rad_per_chip = if best_seg_pilots.len() >= 2 {
            let mut total_delta = 0.0f32;
            let mut count = 0u32;
            for pair in best_seg_pilots.windows(2) {
                let cross = pair[1] * Complex32::new(pair[0].re, -pair[0].im);
                if cross.norm_sqr() > 1e-12 {
                    total_delta += cross.im.atan2(cross.re);
                    count += 1;
                }
            }
            if count > 0 {
                total_delta / (count as f32 * seg_len as f32)
            } else {
                0.0
            }
        } else {
            0.0
        };

        (
            best_phase,
            ratio,
            best_coh_norm,
            best_nc_coh_norm,
            cfo_rad_per_chip,
        )
    }

    /// HPSK composite verification: builds the full PN×LC composite reference
    /// at each LC phase hypothesis and correlates directly with the raw signal.
    ///
    /// This replaces the two-stage (PN despread → LC sweep) approach which does
    /// not work for HPSK because PN and LC are intertwined in the Q branch.
    fn verify_lc_hpsk_composite(
        &self,
        window: &[Complex32],
        delay_samples: i32,
        base_phase: usize,
        expected_abs_chip: usize,
    ) -> (i32, f32, f32, f32, f32) {
        let os = self.cfg.oversample;
        let pp = self.phase_period;
        let half = self.cfg.lc_half_span;
        let n_chips = self.cfg.coherent_chips;
        let window_len = window.len();
        let n_seg = 4usize;
        let seg_len = n_chips.max(1) / n_seg.max(1);

        // Pre-compute PN_I and PN_Q at chip centers.
        let pn_i_chips: Vec<f32> = (0..n_chips)
            .map(|k| self.pn_fft_seq[(base_phase + k * os) % pp].re)
            .collect();
        // pn_fft_seq stores conj(PN) so .im = -PN_Q; negate to get true PN_Q.
        let pn_q_chips: Vec<f32> = (0..n_chips)
            .map(|k| -self.pn_fft_seq[(base_phase + k * os) % pp].im)
            .collect();

        // Pre-generate LC at chip rate over the whole hypothesis span.
        let n_lc = (2 * half + 1) as usize;
        let lc_global_start = (expected_abs_chip as i64 - half as i64).max(0) as usize;
        let lc_total = n_lc + n_chips + 1;
        let lc_i_chips = self.i_lc_signs_from(lc_global_start, lc_total);
        let lc_q_chips = self.q_lc_signs_from(lc_global_start, lc_total);

        let lc_base_offset = (expected_abs_chip as i64 - half as i64) - lc_global_start as i64;

        let mut best_phase = 0i32;
        let mut best_score = f32::MIN;
        let mut second_score = f32::MIN;
        let mut best_coh_norm = 0.0f32;
        let mut best_nc_coh_norm = 0.0f32;
        let mut best_seg_pilots: Vec<Complex32> = Vec::new();

        for phase in -half..=half {
            let slice_start = ((phase + half) as i64 - lc_base_offset) as usize;
            let abs_chip_base = (expected_abs_chip as i64 + phase as i64).max(0) as usize;
            let mut abs_sum = 0.0f32;
            let mut coh = Complex32::new(0.0, 0.0);
            let mut nc_power_sum = 0.0f32;
            let mut seg_coh = Complex32::new(0.0, 0.0);
            let mut seg_count = 0usize;
            let mut seg_pilots = Vec::with_capacity(n_seg);

            for k in 0..n_chips {
                // Sample from the window at the candidate's delay.
                let sample_idx =
                    modulo(k as i32 * os as i32 + delay_samples, window_len as i32) as usize;
                let sig = window[sample_idx];

                let abs_chip = abs_chip_base + k;

                // Build composite reference for this chip.
                let pn_i = pn_i_chips[k];
                let lc_i = lc_i_chips[slice_start + k];
                let s_i = pn_i * lc_i;

                // W12 and decimation based on absolute chip index.
                let w12: f32 = if abs_chip % 2 == 0 { 1.0 } else { -1.0 };
                let abs_even = abs_chip & !1;
                let even_k = abs_even as isize - abs_chip_base as isize;
                let pn_q_dec = if even_k >= 0 && (even_k as usize) < n_chips {
                    pn_q_chips[even_k as usize]
                } else {
                    // pn_fft_seq stores conj(PN), negate .im for true PN_Q.
                    let phase = (base_phase as isize + even_k * os as isize).rem_euclid(pp as isize)
                        as usize;
                    -self.pn_fft_seq[phase].im
                };
                let lc_q_dec_idx = slice_start as isize + even_k;
                let lc_q_dec = lc_q_chips[lc_q_dec_idx.max(0) as usize];
                let dec_q = pn_q_dec * lc_q_dec;
                let s_q = w12 * s_i * dec_q;

                // Despread: signal × conj(composite). Existing 1x HPSK paths
                // use the conjugated `I-jQ` sample convention; HRPD reverse
                // access captures use ordinary `I+jQ` IQ samples.
                let ref_conj = if self.cfg.hpsk_signal_conjugated {
                    Complex32::new(s_i, s_q)
                } else {
                    Complex32::new(s_i, -s_q)
                };
                let d = sig * ref_conj;

                abs_sum += d.re.abs() + d.im.abs();
                coh += d;
                seg_coh += d;
                seg_count += 1;
                if seg_count >= seg_len.max(1) {
                    nc_power_sum += seg_coh.norm_sqr();
                    seg_pilots.push(seg_coh);
                    seg_coh = Complex32::new(0.0, 0.0);
                    seg_count = 0;
                }
            }
            if seg_count > 0 {
                nc_power_sum += seg_coh.norm_sqr();
                seg_pilots.push(seg_coh);
            }

            let abs_sum_safe = abs_sum.max(1e-6);
            let coh_norm = coh.norm() / abs_sum_safe;
            let nc_coh_norm = nc_power_sum.sqrt() / abs_sum_safe;
            let score = nc_coh_norm;

            if score > best_score {
                second_score = best_score;
                best_score = score;
                best_phase = phase;
                best_coh_norm = coh_norm;
                best_nc_coh_norm = nc_coh_norm;
                best_seg_pilots = seg_pilots;
            } else if score > second_score {
                second_score = score;
            }
        }

        let ratio = if second_score > 1e-6 {
            best_score / second_score
        } else {
            f32::INFINITY
        };

        let cfo_rad_per_chip = if best_seg_pilots.len() >= 2 {
            let mut total_delta = 0.0f32;
            let mut count = 0u32;
            for pair in best_seg_pilots.windows(2) {
                let cross = pair[1] * Complex32::new(pair[0].re, -pair[0].im);
                if cross.norm_sqr() > 1e-12 {
                    total_delta += cross.im.atan2(cross.re);
                    count += 1;
                }
            }
            if count > 0 {
                total_delta / (count as f32 * seg_len as f32)
            } else {
                0.0
            }
        } else {
            0.0
        };

        (
            best_phase,
            ratio,
            best_coh_norm,
            best_nc_coh_norm,
            cfo_rad_per_chip,
        )
    }

    fn overlapping_active_fingers(&self, delay_samples: i32) -> Vec<&ActiveFingerState> {
        let suppress = self.cfg.peak_suppress_samples;
        self.active_fingers
            .iter()
            .filter(|f| (f.delay_samples - delay_samples).abs() <= suppress)
            .collect()
    }

    fn has_overlapping_active_finger(&self, delay_samples: i32) -> bool {
        if !self.cfg.suppress_active_finger_delay_overlap {
            return false;
        }
        let suppress = self.cfg.active_finger_delay_suppress_samples;
        self.active_fingers.iter().any(|f| {
            let delta = (f.delay_samples - delay_samples).abs();
            delta == 0 || (!f.hard_validated && delta <= suppress)
        })
    }

    fn can_reacquire_over_active(&self, delay_samples: i32) -> bool {
        let overlapping_active = self.overlapping_active_fingers(delay_samples);
        if overlapping_active.is_empty() {
            return true;
        }

        let reacquire_signal_lost_chips = self.reacquire_signal_lost_chips;
        let reacquire_crc_miss_count = self.reacquire_crc_miss_count;
        let reacquire_idle_chips = self.reacquire_idle_chips;
        overlapping_active.iter().all(|f| {
            // Allow reacquisition if the finger is idle long enough, regardless
            // of validation status.  Unvalidated fingers that never decoded
            // should not block subsequent bursts at the same delay.
            f.idle_chips >= reacquire_idle_chips
                || f.signal_lost_chips >= reacquire_signal_lost_chips
                || (f.hard_validated && f.crc_miss_count >= reacquire_crc_miss_count)
        })
    }

    fn upsert_candidate(&mut self, delay_samples: i32, lc_phase_hint: i32, snr: f32) {
        let match_radius = self
            .cfg
            .peak_suppress_samples
            .max(self.cfg.oversample as i32);
        if let Some(existing) = self
            .pending_candidates
            .iter_mut()
            .find(|c| (c.delay_samples - delay_samples).abs() <= match_radius)
        {
            // Repeated access probes often return on nearly the same delay as
            // the previous burst. Once we have a pending candidate in that
            // neighborhood, let it follow the strongest nearby peak instead of
            // freezing on the first admitted delay while later windows are
            // suppressed as "already active".
            //
            // After a candidate has started accumulating verification hits,
            // keep its delay stable so it can mature into a finger. Letting a
            // partially verified candidate bounce across nearby 95/96/101
            // peaks is enough to starve full-chain captures that need three
            // consistent hits before promotion.
            if existing.preamble_hits == 0
                && (snr > existing.snr || existing.delay_samples != delay_samples)
            {
                trace!(
                    "PnLcCorrelator: candidate updated id={} delay={}=>{} lc_phase_hint={}=>{} snr={:.1}x=>{:.1}x hits={} attempts={}",
                    existing.id,
                    existing.delay_samples,
                    delay_samples,
                    existing.lc_phase_hint,
                    lc_phase_hint,
                    existing.snr,
                    snr,
                    existing.preamble_hits,
                    existing.attempts
                );
                existing.delay_samples = delay_samples;
                existing.lc_phase_hint = lc_phase_hint;
                existing.snr = existing.snr.max(snr);
            }
            return;
        }
        let id = self.next_finger_id;
        self.next_finger_id += 1;
        trace!(
            "PnLcCorrelator: candidate inserted id={} delay={} lc_phase_hint={} snr={:.1}x pending_before={}",
            id,
            delay_samples,
            lc_phase_hint,
            snr,
            self.pending_candidates.len()
        );
        self.pending_candidates.push(PendingCandidate {
            id,
            delay_samples,
            lc_phase_hint,
            snr,
            preamble_hits: 0,
            attempts: 0,
            first_verified_tx_chip: None,
            first_verified_sample_offset: None,
            pn_reference_kind: None,
            timing_mu_samples: 0.0,
            timing_score: f32::MIN,
        });
    }

    fn verify_candidates(
        &mut self,
        window: &[Complex32],
        window_offset: usize,
        block_sample_offset: usize,
    ) -> Vec<(PnLcFinger, Vec<PipelineProcessorShared>)> {
        let os_i = self.cfg.oversample as i32;
        let base_phase = self.base_phase(window_offset);
        let mut detections = Vec::new();
        let mut keep = Vec::new();
        let pending = std::mem::take(&mut self.pending_candidates);

        for mut cand in pending {
            let aligned_delay = cand.delay_samples;
            if !self.can_reacquire_over_active(aligned_delay) {
                keep.push(cand);
                continue;
            }
            let shift_chips = -aligned_delay.div_euclid(os_i);
            let verify_delay = aligned_delay + shift_chips * os_i;
            // The signal is delayed by aligned_delay samples relative to the
            // PN epoch.  abs_chip_at only subtracts composite_filter_delay, so
            // we must also subtract the signal propagation delay to recover
            // the true TX chip at the verification point.
            let verify_sample_offset = window_offset + verify_delay.max(0) as usize;
            let expected_chip = {
                let abs = self.absolute_origin_sample.unwrap_or(0) as i64;
                let s = verify_sample_offset as i64;
                let cfd = self.cfg.composite_filter_delay as i64;
                let delay = aligned_delay as i64;
                ((abs + s - cfd - delay) / self.cfg.oversample as i64).max(0) as usize
            };
            let shift_samples = verify_delay - aligned_delay;
            let verify_base_phase = (base_phase
                + modulo(shift_samples, self.phase_period as i32) as usize)
                % self.phase_period;
            let verify_center_offset = modulo(verify_delay, os_i) as usize;
            let lc_dec = self.cfg.lc_decimation.max(1);
            let hpsk_verify = lc_dec >= 2;
            let mut best_result: Option<(PnReferenceKind, i32, f32, f32, f32, f32, bool, f32)> =
                None;

            if hpsk_verify {
                // HPSK: use composite PN×LC verification (cannot separate PN and LC).
                // Fractional timing refinement for HPSK needs a composite
                // interpolated verifier, so keep HPSK on the legacy integer
                // timing path for now.
                let (best_phase, ratio, coh_norm, nc_coh_norm, est_cfo) = self
                    .verify_lc_hpsk_composite(
                        window,
                        verify_delay,
                        verify_base_phase,
                        expected_chip,
                    );
                let valid = coh_norm >= self.cfg.preamble_coh_norm_min
                    && ratio >= self.cfg.lc_best_over_second_min;
                best_result = Some((
                    PnReferenceKind::Plain,
                    best_phase,
                    ratio,
                    coh_norm,
                    nc_coh_norm,
                    est_cfo,
                    valid,
                    0.0,
                ));
            }

            let plain_timing_offsets = [0.0f32];
            let fractional_timing_offsets = self.fractional_timing_offsets();
            for (pn_reference_kind, pn_reference) in if hpsk_verify {
                vec![] // skip two-stage approach for HPSK
            } else {
                self.verification_references(cand.pn_reference_kind)
            } {
                let timing_offsets: &[f32] = match pn_reference_kind {
                    // Keep the rectangular Plain PN verifier on the integer
                    // prompt. Fractional sample interpolation belongs with
                    // the pulse-shaped OQPSK reference; mixing it with Plain
                    // can promote preamble hits that do not decode cleanly.
                    PnReferenceKind::Plain => &plain_timing_offsets,
                    PnReferenceKind::Oqpsk => &fractional_timing_offsets,
                };
                for &timing_mu in timing_offsets {
                    let despread = if timing_mu.abs() <= f32::EPSILON {
                        self.pn_despread_with_reference(
                            pn_reference,
                            window,
                            verify_delay,
                            verify_base_phase,
                            self.cfg.coherent_chips,
                        )
                    } else {
                        self.pn_despread_with_reference_fractional(
                            pn_reference,
                            window,
                            verify_delay,
                            timing_mu,
                            verify_base_phase,
                            self.cfg.coherent_chips,
                        )
                    };
                    let (best_phase, ratio, coh_norm, nc_coh_norm, mut est_cfo) =
                        self.search_lc_with_signs(&despread, expected_chip);
                    let valid = coh_norm >= self.cfg.preamble_coh_norm_min
                        && ratio >= self.cfg.lc_best_over_second_min;
                    if valid
                        && pn_reference_kind == PnReferenceKind::Plain
                        && self.cfg.fractional_timing_recovery
                        && verify_center_offset != 0
                    {
                        let base_cfo = est_cfo;
                        let mut cfo_score = nc_coh_norm;
                        for &refine_mu in fractional_timing_offsets
                            .iter()
                            .filter(|&&mu| mu.abs() > f32::EPSILON)
                        {
                            let refine_despread = self.pn_despread_with_reference_fractional(
                                pn_reference,
                                window,
                                verify_delay,
                                refine_mu,
                                verify_base_phase,
                                self.cfg.coherent_chips,
                            );
                            let (refine_phase, _, _, refine_nc_coh_norm, refine_cfo) =
                                self.search_lc_with_signs(&refine_despread, expected_chip);
                            if refine_phase == best_phase
                                && refine_nc_coh_norm > cfo_score
                                && (refine_cfo - base_cfo).abs()
                                    <= PLAIN_CFO_REFINE_MAX_DELTA_RAD_PER_CHIP
                            {
                                cfo_score = refine_nc_coh_norm;
                                est_cfo = refine_cfo;
                            }
                        }
                    }
                    match best_result {
                        Some((_, _, _, _, _, _, best_valid, _)) if best_valid && !valid => {}
                        Some((_, _, best_ratio, best_coh, best_nc, _, best_valid, _))
                            if best_valid == valid
                                && (best_nc > nc_coh_norm
                                    || (best_nc == nc_coh_norm
                                        && (best_ratio > ratio
                                            || (best_ratio == ratio && best_coh >= coh_norm)))) => {
                        }
                        _ => {
                            best_result = Some((
                                pn_reference_kind,
                                best_phase,
                                ratio,
                                coh_norm,
                                nc_coh_norm,
                                est_cfo,
                                valid,
                                timing_mu,
                            ));
                        }
                    }
                }
            }
            cand.attempts += 1;
            let Some((
                pn_reference_kind,
                best_phase,
                ratio,
                coh_norm,
                nc_coh_norm,
                est_cfo,
                valid,
                timing_mu,
            )) = best_result
            else {
                keep.push(cand);
                continue;
            };

            if valid {
                cand.preamble_hits += 1;
                let tx_chip = (expected_chip as i64 + best_phase as i64).max(0) as usize;
                cand.first_verified_tx_chip.get_or_insert(tx_chip);
                if nc_coh_norm >= cand.timing_score {
                    cand.timing_score = nc_coh_norm;
                    cand.timing_mu_samples = timing_mu;
                    cand.pn_reference_kind = Some(pn_reference_kind);
                }
                let verified_sample_offset = window_offset + verify_delay.max(0) as usize;
                cand.first_verified_sample_offset = Some(
                    cand.first_verified_sample_offset
                        .map(|existing| existing.min(verified_sample_offset))
                        .unwrap_or(verified_sample_offset),
                );
                trace!(
                    "PnLcCorrelator: verified candidate id={} delay={} lc_phase={} timing_mu={:+.3} ratio={:.2} coh={:.3} nc_coh={:.3} cfo={:.6} hits={}",
                    cand.id,
                    aligned_delay,
                    best_phase,
                    timing_mu,
                    ratio,
                    coh_norm,
                    nc_coh_norm,
                    est_cfo,
                    cand.preamble_hits
                );
            }

            if cand.preamble_hits >= self.cfg.preamble_hits_required {
                let snr = cand.snr;
                let center_offset = modulo(verify_delay, os_i) as usize;
                let os = self.cfg.oversample;
                let verified_sample = cand
                    .first_verified_sample_offset
                    .unwrap_or(window_offset + verify_delay.max(0) as usize);
                let verified_tx_chip = cand
                    .first_verified_tx_chip
                    .unwrap_or((expected_chip as i64 + best_phase as i64).max(0) as usize);

                // Seed the finger from the first verified PN+LC hit rather than
                // the last verification point. Back up by a few Walsh symbols so
                // downstream preamble detection still sees the W0 run that
                // caused this candidate to mature.
                let verify_chip_start = verified_sample.saturating_sub(center_offset);
                let replay_chips = self
                    .cfg
                    .replay_preamble_symbols
                    .saturating_mul(self.cfg.chip_block_size);
                let desired_start =
                    verify_chip_start.saturating_sub(replay_chips.saturating_mul(os));
                let aligned_history_start = align_up_to_residue(
                    self.recent_start_offset,
                    verify_chip_start % os.max(1),
                    os.max(1),
                );
                let finger_start_sample = desired_start.max(aligned_history_start);
                let sample_delta_chips =
                    (verify_chip_start as i64 - finger_start_sample as i64) / os as i64;
                let tx_chip = (verified_tx_chip as i64 - sample_delta_chips).max(0) as usize;
                let samples_to_skip = finger_start_sample.saturating_sub(block_sample_offset);

                // PN phase: (abs + S - filter_delay - signal_delay) % pp.
                // The -aligned_delay accounts for the signal's PN timing
                // offset found by the FFT correlator.
                let despread_phase = {
                    let abs = self.absolute_origin_sample.unwrap_or(0) as i64;
                    let pp = self.phase_period as i64;
                    let raw = abs + finger_start_sample as i64
                        - self.cfg.composite_filter_delay as i64
                        - aligned_delay as i64;
                    ((raw % pp + pp) % pp) as usize
                };

                let lc_dec = self.cfg.lc_decimation.max(1);
                let finger_pn_kind = cand.pn_reference_kind.unwrap_or(PnReferenceKind::Plain);
                let timing_offsets = match finger_pn_kind {
                    // The plain PN reference is an integer-grid rectangular
                    // sequence used for legacy-style despreading. Fractional
                    // sample interpolation against that reference can win the
                    // W0 preamble score while moving data frames off the
                    // integer prompt; keep fractional timing on the
                    // pulse-shaped OQPSK reference only.
                    PnReferenceKind::Plain
                        if self.cfg.gardner_timing.enabled
                            && !self.cfg.fractional_timing_recovery =>
                    {
                        self.finger_timing_offsets(cand.timing_mu_samples, snr)
                    }
                    PnReferenceKind::Plain => vec![0.0],
                    PnReferenceKind::Oqpsk => {
                        self.finger_timing_offsets(cand.timing_mu_samples, snr)
                    }
                };
                let timing_total = timing_offsets.len();

                for (timing_idx, timing_mu_samples) in timing_offsets.into_iter().enumerate() {
                    if self.has_overlapping_active_finger(aligned_delay) {
                        trace!(
                            "PnLcCorrelator: suppressing finger spawn at delay={} within active-finger threshold={}",
                            aligned_delay, self.cfg.active_finger_delay_suppress_samples
                        );
                        continue;
                    }

                    let finger_id = if timing_idx == 0 {
                        cand.id
                    } else {
                        let id = self.next_finger_id;
                        self.next_finger_id += 1;
                        id
                    };

                    let lc_gen = self.seed_i_lc(tx_chip);
                    let q_lc_gen = self.q_lc_template.as_ref().map(|_| self.seed_q_lc(tx_chip));

                    let finger_pn_reference = match finger_pn_kind {
                        PnReferenceKind::Plain => Arc::clone(&self.pn_fft_seq),
                        PnReferenceKind::Oqpsk => Arc::clone(&self.pn_despread_seq),
                    };

                    debug!(
                        "PnLcCorrelator: PREAMBLE DETECTED id={} delay={} lc_phase={} tx_chip={} \
                         first_verified_tx_chip={} first_verified_sample={} despread_phase={} \
                         center_offset={} ref={:?} timing_mu={:+.3} timing_hyp={}/{} finger_start={} replay_samples={} skip={} cfo={:.6}",
                        finger_id,
                        aligned_delay,
                        best_phase,
                        tx_chip,
                        verified_tx_chip,
                        verified_sample,
                        despread_phase,
                        center_offset,
                        finger_pn_kind,
                        timing_mu_samples,
                        timing_idx + 1,
                        timing_total,
                        finger_start_sample,
                        block_sample_offset.saturating_sub(finger_start_sample),
                        samples_to_skip,
                        est_cfo
                    );

                    self.active_fingers.push(ActiveFingerState {
                        id: finger_id,
                        delay_samples: aligned_delay,
                        hard_validated: false,
                        idle_chips: 0,
                        signal_lost_chips: 0,
                        crc_miss_count: 0,
                        post_walsh_no_event_ms: 0,
                    });
                    let mut chain = (self.chain_builder)();
                    let mut finger = PnLcFinger::new_with_gardner(
                        finger_id,
                        finger_pn_reference,
                        self.phase_period,
                        os,
                        despread_phase,
                        center_offset,
                        lc_gen,
                        q_lc_gen,
                        tx_chip,
                        tx_chip,
                        self.cfg.chip_block_size,
                        samples_to_skip,
                        snr,
                        est_cfo,
                        lc_dec,
                        self.cfg.enable_epl_tracking,
                        self.cfg.enable_epl_slew,
                        self.cfg.epl_pilot,
                        self.cfg.access_cfo,
                        timing_mu_samples,
                        self.cfg.output_oversampled_chips,
                        self.cfg.integrate_and_dump,
                        self.cfg.lc_period_chips,
                        self.cfg.lc_period_initial_state,
                        self.cfg.hpsk_signal_conjugated,
                        self.cfg.gardner_timing,
                    );
                    finger.set_nonpilot_cfo_tracking(self.cfg.nonpilot_cfo_tracking);
                    // Seed HPSK state: need LC(tx_chip - 1) for the Q delay.
                    // The despread loop calls `advance_lc_for_new_chip` at
                    // the start of every chip iter (including the first),
                    // which reads these fields to compute chip tx_chip's
                    // composite LC value.
                    if lc_dec >= 2 && tx_chip > 0 && self.q_lc_template.is_none() {
                        let mut prev_gen = self.seed_i_lc(tx_chip - 1);
                        finger.hpsk_prev_lc = if prev_gen.next_chip() == 1 { -1.0 } else { 1.0 };
                        // Seed W12 parity from the absolute chip index.
                        finger.hpsk_chip_count = tx_chip;
                    } else if lc_dec >= 2 {
                        finger.hpsk_chip_count = tx_chip;
                    }
                    if finger_start_sample < block_sample_offset {
                        self.replay_recent_history(
                            &mut finger,
                            &mut chain,
                            finger_start_sample,
                            block_sample_offset,
                        );
                    }
                    detections.push((finger, chain));
                }
            } else if cand.attempts
                < if cand.preamble_hits == 0 {
                    MAX_PENDING_ATTEMPTS_WITHOUT_HIT
                } else {
                    MAX_PENDING_ATTEMPTS_WITH_HIT
                }
            {
                keep.push(cand);
            } else {
                trace!(
                    "PnLcCorrelator: dropping candidate id={} delay={} hits={} attempts={}",
                    cand.id, cand.delay_samples, cand.preamble_hits, cand.attempts
                );
            }
        }

        self.pending_candidates = keep;
        detections
    }

    fn replay_recent_history(
        &self,
        finger: &mut PnLcFinger,
        chain: &mut Vec<PipelineProcessorShared>,
        start_sample_offset: usize,
        end_sample_offset: usize,
    ) {
        if start_sample_offset >= end_sample_offset {
            return;
        }
        let history_end = self.recent_start_offset + self.recent_samples.len();
        if start_sample_offset < self.recent_start_offset || end_sample_offset > history_end {
            debug!(
                "PnLcCorrelator: replay range unavailable start={} end={} history=[{}, {})",
                start_sample_offset, end_sample_offset, self.recent_start_offset, history_end
            );
            return;
        }

        let start_idx = start_sample_offset - self.recent_start_offset;
        let sample_count = end_sample_offset - start_sample_offset;
        let samples: Vec<Complex32> = self
            .recent_samples
            .iter()
            .skip(start_idx)
            .take(sample_count)
            .copied()
            .collect();
        if samples.is_empty() {
            return;
        }

        let replay_block = SampleBlock::new(samples, 0).with_sample_rate_hz(self.sample_rate_hz);
        finger.replay_block(&replay_block, chain);
    }

    // ------------------------------------------------------------------
    // Joint PN×LC FFT search
    // ------------------------------------------------------------------

    fn run_joint_search(
        &mut self,
        window: &[Complex32],
        window_offset: usize,
        _block_sample_offset: usize,
    ) -> Vec<(PnLcFinger, Vec<PipelineProcessorShared>)> {
        let os = self.cfg.oversample;
        let n_chips = self.cfg.coherent_chips;
        let window_len = n_chips * os;
        let pp = self.phase_period;
        let half = self.cfg.lc_half_span;
        let base_phase = self.base_phase(window_offset);
        let expected_chip = self.abs_chip_at(window_offset);

        let n_seg = self.cfg.noncoherent_segments.max(1);
        let seg_len = self.seg_len; // samples per segment
        let _seg_chips = n_chips / n_seg;

        // --- Pre-FFT the signal segments (reused across all LC hypotheses) ---
        // For the coherent path: one FFT per CFO hypothesis.
        // For the segmented path: one set of segment FFTs (CFO is less critical).
        let signal_seg_ffts: Vec<Vec<Complex32>> = if n_seg > 1 {
            (0..n_seg)
                .map(|s| {
                    let mut buf = window[s * seg_len..(s + 1) * seg_len].to_vec();
                    self.seg_fft_fwd
                        .process_with_scratch(&mut buf, &mut self.seg_fft_scratch);
                    buf
                })
                .collect()
        } else {
            // Will use cfo_signal_ffts instead for the coherent path.
            vec![]
        };

        // Pre-compute CFO-rotated signal FFTs for the coherent path. Buffers
        // and phasors are owned by the correlator so each search avoids heap
        // allocation and per-sample trig.
        if n_seg <= 1 {
            for cfo_idx in 0..self.search_cfo_hypotheses.len() {
                let buf = &mut self.search_cfo_signal_ffts[cfo_idx];
                if self.search_cfo_hypotheses[cfo_idx] == 0.0 {
                    buf[..window_len].copy_from_slice(&window[..window_len]);
                } else {
                    let phasors = &self.search_cfo_phasors[cfo_idx];
                    for i in 0..window_len {
                        buf[i] = window[i] * phasors[i];
                    }
                }
                self.fft_fwd
                    .process_with_scratch(&mut buf[..window_len], &mut self.fft_scratch);
            }
        }

        // For coherent (n_seg==1): delay space = window_len.
        // For segmented: delay space = seg_len (each segment gives seg_len delay bins).
        let _delay_space = if n_seg > 1 { seg_len } else { window_len };
        let norm = if n_seg > 1 {
            // Each segment's power is norm_sqr / seg_len², sum over n_seg segments.
            1.0 / (seg_len as f32 * seg_len as f32)
        } else {
            1.0 / (window_len as f32 * window_len as f32)
        };

        let mut best_delay = 0i32;
        let mut best_lc_phase = 0i32;
        let mut best_power = 0.0f32;
        let mut total_power = 0.0f64;
        let mut total_count = 0usize;
        let mut lc0_peak_power = 0.0f32;
        let mut lc0_peak_delay = 0i32;

        let n_lc = (2 * half + 1) as usize;
        let mut lc_peak_power = vec![0.0f32; n_lc];
        let mut lc_peak_delay = vec![0i32; n_lc];

        let lc_dec = self.cfg.lc_decimation.max(1);
        let hpsk = lc_dec >= 2;

        // Pre-compute PN conjugate values at chip centers (reused across all LC hypotheses).
        let pn_chips: Vec<Complex32> = (0..n_chips)
            .map(|k| self.pn_fft_seq[(base_phase + k * os) % pp])
            .collect();

        // Pre-generate LC values for the full hypothesis span in one pass.
        // We need chips from (expected_chip - half) to (expected_chip + half + n_chips - 1).
        let lc_total_chips = n_lc + n_chips - 1;
        let lc_global_start = (expected_chip as i64 - half as i64).max(0) as usize;

        // --- HPSK: chip-rate I/Q long codes + separate PN_I/PN_Q arrays ---
        let hpsk_lc_i: Vec<f32> = if hpsk {
            self.i_lc_signs_from(lc_global_start, lc_total_chips + 2)
        } else {
            vec![]
        };
        let hpsk_lc_q: Vec<f32> = if hpsk {
            self.q_lc_signs_from(lc_global_start, lc_total_chips + 2)
        } else {
            vec![]
        };

        let hpsk_pn_i: Vec<f32> = if hpsk {
            (0..n_chips)
                .map(|k| self.pn_fft_seq[(base_phase + k * os) % pp].re)
                .collect()
        } else {
            vec![]
        };
        let hpsk_pn_q: Vec<f32> = if hpsk {
            (0..n_chips)
                .map(|k| -self.pn_fft_seq[(base_phase + k * os) % pp].im)
                .collect()
        } else {
            vec![]
        };

        if !hpsk {
            self.fill_search_lc_signs(lc_global_start, lc_total_chips);
        }

        let lc_signs_base_offset = (expected_chip as i64 - half as i64) - lc_global_start as i64;

        for lc_phase in -half..=half {
            let lc_idx = (lc_phase + half) as usize;

            // Slice into pre-generated LC values for this hypothesis.
            let slice_start = (lc_phase + half) as i64 - lc_signs_base_offset;
            let slice_start = slice_start as usize;

            self.search_ref_buf[..window_len].fill(Complex32::new(0.0, 0.0));
            if hpsk {
                // HPSK: build composite matched-filter reference per 2.1.3.1.17.
                //
                // Signal model (complex baseband, I − jQ convention):
                //   s_I(n) = PN_I(n) × LC(n)
                //   s_Q(n) = W12(n) × s_I(n) × Decim₂[PN_Q × UQ(n)]
                //
                // W12 and decimation pair boundaries are based on absolute chip
                // indices (aligned to the PN/frame epoch), NOT window-relative k.
                // Getting the parity wrong negates the Q reference → ~zero correlation.
                //
                // Matched-filter reference = s(n) = s_I − j·s_Q.
                let abs_chip_base = (expected_chip as i64 + lc_phase as i64).max(0) as usize;
                for k in 0..n_chips {
                    let pn_i = hpsk_pn_i[k];

                    // I branch: LC at chip rate
                    let lc_i = hpsk_lc_i[slice_start + k];
                    let s_i = pn_i * lc_i;

                    // Absolute chip index determines W12 parity and pair boundaries.
                    let abs_chip = abs_chip_base + k;

                    // W12(n) = (-1)^n based on absolute chip parity.
                    let w12: f32 = if abs_chip % 2 == 0 { 1.0 } else { -1.0 };

                    // Q branch: decimated PN_Q × LC_Q from even chip of pair.
                    // Pairs are (0,1),(2,3),... in absolute chip indices.
                    let abs_even = abs_chip & !1; // pair start (round down to even)
                    let even_k = abs_even as isize - abs_chip_base as isize; // index in window
                    let pn_q_dec = if even_k >= 0 && (even_k as usize) < n_chips {
                        hpsk_pn_q[even_k as usize]
                    } else {
                        // Pair start is before the window; compute directly.
                        // pn_fft_seq stores conj(PN), negate .im for true PN_Q.
                        let phase = (base_phase as isize + even_k * os as isize)
                            .rem_euclid(pp as isize) as usize;
                        -self.pn_fft_seq[phase].im
                    };
                    let lc_q_dec_idx = slice_start as isize + even_k;
                    let lc_q_dec = hpsk_lc_q[lc_q_dec_idx.max(0) as usize];
                    let dec_q = pn_q_dec * lc_q_dec;

                    let s_q = w12 * s_i * dec_q;

                    self.search_ref_buf[k * os] = if self.cfg.hpsk_signal_conjugated {
                        // Matched-filter reference for legacy conjugated
                        // sample convention: s_I − j·s_Q.
                        Complex32::new(s_i, -s_q)
                    } else {
                        // HRPD reverse captures are ordinary IQ: s_I + j·s_Q.
                        Complex32::new(s_i, s_q)
                    };
                }
            } else {
                // IS-95: real LC sign × PN (original path, unchanged)
                for k in 0..n_chips {
                    let lc_sign = self.search_lc_signs[slice_start + k];
                    let pn_conj = pn_chips[k];
                    self.search_ref_buf[k * os] =
                        Complex32::new(pn_conj.re * lc_sign, -pn_conj.im * lc_sign);
                }
            }

            if n_seg > 1 {
                // --- Noncoherent segmented FFT accumulation ---
                // For each delay bin, sum |corr_segment|² across all segments.
                self.search_nc_power[..seg_len].fill(0.0);
                for seg_idx in 0..n_seg {
                    let seg_start = seg_idx * seg_len;
                    self.search_ref_seg[..seg_len]
                        .copy_from_slice(&self.search_ref_buf[seg_start..seg_start + seg_len]);
                    self.seg_fft_fwd.process_with_scratch(
                        &mut self.search_ref_seg[..seg_len],
                        &mut self.seg_fft_scratch,
                    );

                    // Cross-correlate: signal_seg × conj(ref_seg) → IFFT.
                    for i in 0..seg_len {
                        let s = signal_seg_ffts[seg_idx][i];
                        let r = self.search_ref_seg[i];
                        self.search_result_buf[i] = s * Complex32::new(r.re, -r.im);
                    }
                    self.seg_fft_inv.process_with_scratch(
                        &mut self.search_result_buf[..seg_len],
                        &mut self.seg_fft_scratch,
                    );

                    for d in 0..seg_len {
                        self.search_nc_power[d] += self.search_result_buf[d].norm_sqr() * norm;
                    }
                }

                // Scan the noncoherent power surface for this LC phase.
                let mut lc_peak = lc_peak_power[lc_idx];
                let mut lc_peak_delay_signed = lc_peak_delay[lc_idx];
                for (d, &power) in self.search_nc_power[..seg_len].iter().enumerate() {
                    total_power += power as f64;
                    if power > lc_peak {
                        lc_peak = power;
                        lc_peak_delay_signed = signed_delay_bin(d, seg_len);
                    }
                }
                total_count += seg_len;
                lc_peak_power[lc_idx] = lc_peak;
                lc_peak_delay[lc_idx] = lc_peak_delay_signed;
            } else {
                // --- Fully coherent path with CFO hypothesis grid ---
                // FFT the reference in-place; cross-correlate with each pre-rotated
                // signal FFT and keep the best power across all CFO hypotheses.
                self.fft_fwd.process_with_scratch(
                    &mut self.search_ref_buf[..window_len],
                    &mut self.fft_scratch,
                );

                let mut lc_peak = lc_peak_power[lc_idx];
                let mut lc_peak_delay_signed = lc_peak_delay[lc_idx];
                for cfo_sig_fft in &self.search_cfo_signal_ffts[..self.search_cfo_hypotheses.len()]
                {
                    for i in 0..window_len {
                        let s = cfo_sig_fft[i];
                        let r = self.search_ref_buf[i];
                        self.search_result_buf[i] = s * Complex32::new(r.re, -r.im);
                    }
                    self.fft_inv.process_with_scratch(
                        &mut self.search_result_buf[..window_len],
                        &mut self.fft_scratch,
                    );

                    for (d, sample) in self.search_result_buf[..window_len].iter().enumerate() {
                        let power = sample.norm_sqr() * norm;
                        total_power += power as f64;
                        if power > lc_peak {
                            lc_peak = power;
                            lc_peak_delay_signed = signed_delay_bin(d, window_len);
                        }
                    }
                    total_count += window_len;
                }
                lc_peak_power[lc_idx] = lc_peak;
                lc_peak_delay[lc_idx] = lc_peak_delay_signed;
            }

            // Both are maxima over the bins this phase just scanned, so fold
            // once here instead of testing every bin. Strict `>` keeps the
            // first-wins tie-break: earliest bin, then earliest phase.
            let lc_peak = lc_peak_power[lc_idx];
            if lc_peak > best_power {
                best_power = lc_peak;
                best_delay = lc_peak_delay[lc_idx];
                best_lc_phase = lc_phase;
            }
            if lc_phase == 0 {
                lc0_peak_power = lc_peak;
                lc0_peak_delay = lc_peak_delay[lc_idx];
            }
        }

        // Second-best LC phase: highest power from a different LC phase.
        let best_lc_idx = (best_lc_phase + half) as usize;
        let mut second_lc_power = 0.0f32;
        for (idx, &pwr) in lc_peak_power.iter().enumerate() {
            if idx != best_lc_idx && pwr > second_lc_power {
                second_lc_power = pwr;
            }
        }
        let lc_best_over_second = best_power / second_lc_power.max(1e-20);

        // CFAR-style noise estimate: exclude the peak bin so strong signals
        // don't inflate the average and compress the SNR reading.
        let noise_count = total_count.saturating_sub(1).max(1);
        let noise_power =
            ((total_power - best_power as f64) / noise_count as f64).max(1e-20) as f32;
        let snr = best_power / noise_power;

        let lc0_snr = lc0_peak_power / noise_power;

        // ------------------------------------------------------------------
        // Time-domain coherence verification at the winning (delay, lc_phase).
        //
        // The FFT gives us the coherent sum (numerator of coh_norm) but not the
        // per-chip normalization.  A single time-domain pass over the n_chips
        // at the winning candidate is cheap and gives us the same coh_norm and
        // ratio metrics used by the verification gate below.
        // ------------------------------------------------------------------

        trace!(
            "PnLcCorrelator: window={} offset={} delay={} lc_phase={} \
             snr={:.1}x lc_ratio={:.2} \
             best_pwr={:.3e} noise_pwr={:.3e} \
             lc0_pwr={:.3e} lc0_delay={} lc0_snr={:.1}x",
            self.window_counter,
            window_offset,
            best_delay,
            best_lc_phase,
            snr,
            lc_best_over_second,
            best_power,
            noise_power,
            lc0_peak_power,
            lc0_peak_delay,
            lc0_snr,
        );

        // Gate: SNR (matched-filter coherence) + LC phase uniqueness.
        //
        // The FFT SNR is itself a coherence metric — the matched-filter peak
        // normalized by the noise floor.  The lc_best_over_second ratio
        // discriminates the true LC phase from noise/sidelobes, equivalent
        // to the dual coh_norm + ratio gate used by the LC verifier.
        if snr < self.cfg.snr_threshold {
            return Vec::new();
        }
        if lc_best_over_second < self.cfg.lc_best_over_second_min {
            trace!(
                "PnLcCorrelator: LC ratio gate failed: {:.2} < {:.2}",
                lc_best_over_second, self.cfg.lc_best_over_second_min,
            );
            return Vec::new();
        }

        // Let an existing pending candidate track the stronger nearby delay
        // even if a stale validated finger still occupies the suppress radius.
        if self
            .pending_candidates
            .iter()
            .any(|c| (c.delay_samples - best_delay).abs() <= self.cfg.peak_suppress_samples)
        {
            self.upsert_candidate(best_delay, best_lc_phase, snr);
            return Vec::new();
        }

        if !self.can_reacquire_over_active(best_delay) {
            trace!(
                "PnLcCorrelator: delay={} already active, skipping",
                best_delay
            );
            return Vec::new();
        }
        self.upsert_candidate(best_delay, best_lc_phase, snr);
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Test-only visibility helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
impl PnLcCorrelator {
    /// Expose base_phase for test verification.
    fn test_base_phase(&self, window_offset: usize) -> usize {
        self.base_phase(window_offset)
    }

    /// Expose abs_chip_at for test verification.
    fn test_abs_chip_at(&self, sample_offset: usize) -> usize {
        self.abs_chip_at(sample_offset)
    }

    /// Expose center_offset for test verification.
    fn test_center_offset(&self) -> usize {
        self.center_offset()
    }
}

// ---------------------------------------------------------------------------
// Correlator impl
// ---------------------------------------------------------------------------

impl Correlator for PnLcCorrelator {
    type Finger = PnLcFinger;

    fn correlate(
        &mut self,
        block: &SampleBlock,
    ) -> Vec<(PnLcFinger, Vec<PipelineProcessorShared>)> {
        self.sample_rate_hz = block.sample_rate_hz;

        // Latch the absolute sample origin from the first block.
        if self.absolute_origin_sample.is_none() {
            self.absolute_origin_sample = block
                .tags
                .get("absolute_sample_start")
                .copied()
                .map(|v| v.max(0) as usize);
        }

        // Record the stream position of block sample 0 BEFORE extending the
        // buffer.  Any leftover samples already in the buffer from previous
        // calls are older than the current block, so the current block starts
        // at `samples_consumed + buffer_leftover`.
        let mut buffer_leftover = self.buffer.len();

        // When enabled, re-anchor absolute_origin_sample using the
        // hardware-timestamp-derived tag.  This corrects drift caused by
        // SDR sample drops (overflow) that make samples_consumed fall
        // behind the real hardware clock position.
        if self.cfg.reanchor_origin {
            if let Some(&block_abs) = block.tags.get("absolute_sample_start") {
                let block_abs = block_abs.max(0) as usize;
                let internal_pos = self.samples_consumed + buffer_leftover;
                if block_abs < internal_pos {
                    debug!(
                        "PnLcCorrelator: ignoring non-monotonic absolute_sample_start={} \
                         (internal_pos={})",
                        block_abs, internal_pos
                    );
                } else {
                    let new_origin = block_abs - internal_pos;

                    if let Some(old_origin) = self.absolute_origin_sample {
                        let delta = new_origin as i64 - old_origin as i64;
                        if delta > 4 {
                            debug!(
                                "PnLcCorrelator: sample gap detected \
                                 (delta={} samples, ~{} chips), flushing buffer",
                                delta,
                                delta / self.cfg.oversample as i64
                            );
                            self.handle_stream_discontinuity(&mut buffer_leftover);
                            // Buffer was flushed — the new block starts at
                            // the current samples_consumed with no leftover.
                            self.absolute_origin_sample = Some(new_origin);
                        } else if delta.abs() > 4 {
                            self.absolute_origin_sample = Some(new_origin);
                        }
                    } else {
                        self.absolute_origin_sample = Some(new_origin);
                    }
                }
            }
        }

        self.buffer.extend_from_slice(&block.samples);
        let block_sample_offset = self.samples_consumed + buffer_leftover;

        if self.recent_samples.is_empty() {
            self.recent_start_offset = block_sample_offset;
        }
        self.recent_samples.extend(block.samples.iter().copied());
        let max_recent_samples = self.cfg.coherent_chips * self.cfg.oversample * 64;
        while self.recent_samples.len() > max_recent_samples {
            self.recent_samples.pop_front();
            self.recent_start_offset += 1;
        }

        let os = self.cfg.oversample;
        let window_len = self.cfg.coherent_chips * os;
        let mut detections = Vec::new();

        while self.buffer.len() >= window_len {
            let window: Vec<Complex32> = self.buffer.drain(..window_len).collect();
            let window_offset = self.samples_consumed;
            self.samples_consumed += window_len;

            // Run the joint PN×LC search at configured intervals.
            // When suppress_search_when_locked is enabled (traffic channels),
            // skip the expensive FFT search once a hard-validated finger exists.
            // The search resumes automatically if the finger is pruned.
            let search_suppressed = self.search_suppressed();
            if self.window_counter % self.cfg.search_interval_windows == 0
                && self.window_counter > 0
                && !search_suppressed
            {
                detections.extend(self.run_joint_search(
                    &window,
                    window_offset,
                    block_sample_offset,
                ));
                detections.extend(self.verify_candidates(
                    &window,
                    window_offset,
                    block_sample_offset,
                ));
            }

            self.window_counter += 1;
        }

        detections
    }

    fn notify_hard_validated(&mut self, finger_id: u64) {
        if let Some(active) = self.active_fingers.iter_mut().find(|f| f.id == finger_id) {
            active.hard_validated = true;
        }
        debug!("PnLcCorrelator: finger {} hard-validated", finger_id);
    }

    fn search_suppressed(&self) -> bool {
        self.cfg.suppress_search_when_locked
            && self.active_fingers.iter().any(|finger| {
                finger.hard_validated && finger.signal_lost_chips < self.reacquire_signal_lost_chips
            })
    }

    fn notify_finger_state(
        &mut self,
        finger_id: u64,
        hard_validated: bool,
        idle_chips: u64,
        signal_lost_chips: u64,
        crc_miss_count: u64,
        post_walsh_no_event_ms: u64,
    ) {
        if let Some(active) = self.active_fingers.iter_mut().find(|f| f.id == finger_id) {
            active.hard_validated = hard_validated;
            active.idle_chips = idle_chips;
            active.signal_lost_chips = signal_lost_chips;
            active.crc_miss_count = crc_miss_count;
            active.post_walsh_no_event_ms = post_walsh_no_event_ms;
        }
    }

    fn notify_finger_removed(&mut self, finger_id: u64) {
        let before = self.active_fingers.len();
        self.active_fingers.retain(|f| f.id != finger_id);
        if self.active_fingers.len() < before {
            info!(
                "PnLcCorrelator: finger {} removed, releasing delay suppression",
                finger_id
            );
        }
    }
}

fn modulo(x: i32, m: i32) -> i32 {
    ((x % m) + m) % m
}

/// Map an IFFT output bin to a signed delay: bins past the midpoint wrap to
/// negative delays.
#[inline]
fn signed_delay_bin(bin: usize, len: usize) -> i32 {
    if bin > len / 2 {
        bin as i32 - len as i32
    } else {
        bin as i32
    }
}

fn align_up_to_residue(x: usize, residue: usize, modulus: usize) -> usize {
    if modulus <= 1 {
        return x;
    }
    let current = x % modulus;
    let residue = residue % modulus;
    if current == residue {
        x
    } else {
        x + (residue + modulus - current) % modulus
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use num_complex::Complex32;

    use super::{ActiveFingerState, PendingCandidate, PnLcConfig, PnLcCorrelator, PnLcFinger};
    use crate::phy::coding::long_code::LongCodeGenerator;
    use crate::receiver::pipelined::generic_rake_receiver::{Correlator, RakeFinger};
    use crate::receiver::pipelined::{
        PipelineProcessorShared, SampleBlock, build_fft_search_pn_samples,
    };

    /// Build a PN conjugate sequence (same as the correlator does internally).
    fn build_pn_conj(phase_period: usize, oversample: usize) -> Arc<Vec<Complex32>> {
        Arc::new(
            build_fft_search_pn_samples(phase_period, oversample)
                .into_iter()
                .map(|s| Complex32::new(s.re, -s.im))
                .collect(),
        )
    }

    /// Generate a TX signal: PN × LC at chip rate, with `oversample` repeated
    /// samples per chip.  No pulse shaping — raw NRZ.
    ///
    /// Returns `(samples, lc_gen_after)` where `lc_gen_after` is the LC state
    /// at the end of the generated signal.
    fn generate_tx_signal(
        pn_samples: &[Complex32],
        lc_gen: &mut LongCodeGenerator,
        chip_start_pn_sample: usize,
        num_chips: usize,
        oversample: usize,
    ) -> Vec<Complex32> {
        let pp = pn_samples.len();
        let mut out = Vec::with_capacity(num_chips * oversample);
        for k in 0..num_chips {
            let lc_chip = lc_gen.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let pn_idx = (chip_start_pn_sample + k * oversample + s) % pp;
                // TX sends pn_iq (not conjugated) × lc_sign
                let pn_iq = Complex32::new(pn_samples[pn_idx].re, -pn_samples[pn_idx].im);
                out.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }
        out
    }

    /// Helper: create a no-op chain builder (empty sub-chain).
    fn noop_chain_builder() -> Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send> {
        Box::new(|| Vec::new())
    }

    fn lc_signs_from(mut lc: LongCodeGenerator, abs_chip_start: usize, count: usize) -> Vec<f32> {
        lc.advance_chips(abs_chip_start);
        (0..count)
            .map(|_| if lc.next_chip() == 1 { -1.0 } else { 1.0 })
            .collect()
    }

    #[test]
    fn q_lc_signs_default_to_legacy_delayed_i_long_code() {
        let i_lc = LongCodeGenerator::new_traffic_channel(0x1234_5678);
        let correlator =
            PnLcCorrelator::new(PnLcConfig::default_4x(), i_lc.clone(), noop_chain_builder());

        let mut expected_at_zero = vec![1.0];
        expected_at_zero.extend(lc_signs_from(i_lc.clone(), 0, 5));
        assert_eq!(correlator.q_lc_signs_from(0, 6), expected_at_zero);

        assert_eq!(
            correlator.q_lc_signs_from(128, 8),
            lc_signs_from(i_lc, 127, 8)
        );
    }

    #[test]
    fn q_lc_signs_use_explicit_q_template_without_delay() {
        let i_lc = LongCodeGenerator::new_traffic_channel(0x1234_5678);
        let q_lc = LongCodeGenerator::new_traffic_channel(0x8765_4321);
        let correlator = PnLcCorrelator::new(PnLcConfig::default_4x(), i_lc, noop_chain_builder())
            .with_q_lc_template(q_lc.clone());

        assert_eq!(
            correlator.q_lc_signs_from(128, 8),
            lc_signs_from(q_lc, 128, 8)
        );
    }

    // =====================================================================
    // 1. PN despreading — verify PN conjugate multiply recovers a DC signal
    // =====================================================================

    #[test]
    fn finger_pn_despread_recovers_dc_from_pn_modulated_signal() {
        // A signal that is just PN (no LC, no Walsh modulation) should
        // despread to a constant DC level at chip rate.
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        // TX: pn_iq[k] at all sample positions (preamble with LC=all-zeros ≡ lc_sign=+1)
        // Since we want to test PN only, we build the signal as pn_iq directly.
        let num_chips = 512;
        let despread_phase = 0usize;
        let center_offset = 0usize;

        let mut signal = Vec::with_capacity(num_chips * oversample);
        for k in 0..num_chips {
            for s in 0..oversample {
                let idx = (k * oversample + s) % phase_period;
                // Signal is pn_iq (unconjugated)
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(pn_iq);
            }
        }

        // Create a trivial LC generator that outputs 0 (lc_sign = +1) every chip.
        // We use a real LC but set chain_start_chip = lc_chip_counter so gating
        // doesn't block output.  The LC sign will be applied, but since we
        // constructed the signal without LC, the output will show the LC effect.
        // Instead, let's use a finger with an LC gen but also multiply the TX
        // signal by the same LC.  For this test, use a known LC seed.
        let lc_seed_chip = 0usize;
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        lc_tx.advance_chips(lc_seed_chip);

        // Re-generate signal with LC applied
        signal.clear();
        let mut lc_tx_clone = lc_tx.clone();
        for k in 0..num_chips {
            let lc_chip = lc_tx_clone.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        let lc_rx = lc_tx.clone();

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            despread_phase,
            center_offset,
            lc_rx,
            lc_seed_chip, // lc_chip_counter
            lc_seed_chip, // chain_start_chip (no gating)
            256,          // chip_block_size
            0,            // samples_to_skip
            0.0,          // detection_snr
            0.0,          // initial_cfo_rad_per_chip
            1,            // lc_decimation
            false,        // enable_epl_tracking
            false,        // enable_epl_slew
            false,        // epl_pilot
            false,        // access_cfo
            0.0,          // timing_mu_samples
            false,        // output_oversampled_chips
            false,        // integrate_and_dump
        );

        finger.despread_block(&signal);
        let chips = finger.chip_buffer_as_slice();

        assert_eq!(
            chips.len(),
            num_chips,
            "expected {} chips, got {}",
            num_chips,
            chips.len()
        );

        // After PN despread + LC removal, each chip should be ≈ +1+0j
        // (pn_conj × pn_iq = |pn|² = 1+1 = 2 for I²+Q²=2, but actually
        //  conj(pn) × pn = pn.re² + pn.im² + j*0 = 2.0 for ±1 I,Q)
        // The exact value depends on the PN convention.  Let's just verify
        // the chips are real-valued and positive (or have consistent sign).
        let first_chip = chips[0];
        eprintln!("first despreaded chip: {:?}", first_chip);

        // All chips should have the same real value (DC) after PN+LC removal
        for (i, chip) in chips.iter().enumerate() {
            assert!(
                (chip.re - first_chip.re).abs() < 0.01,
                "chip {} real part {:.4} differs from first {:.4}",
                i,
                chip.re,
                first_chip.re
            );
            assert!(
                chip.im.abs() < 0.01,
                "chip {} imaginary part {:.4} should be ~0",
                i,
                chip.im
            );
        }
        // The despreaded value should be substantial (≈2.0 for ±1 PN)
        assert!(
            first_chip.re.abs() > 1.5,
            "despreaded chip magnitude {:.4} too small",
            first_chip.re
        );
    }

    // =====================================================================
    // 2. Verify center_offset selects the correct sub-sample
    // =====================================================================

    #[test]
    fn finger_center_offset_selects_correct_subsample() {
        // With oversample=4 and center_offset=3, the finger should only
        // output chips from sub-sample 3 within each oversample period.
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let num_chips = 64;
        let center_offset = 3usize;

        // Build a signal where sub-sample 3 has a distinct value
        let mut signal = Vec::with_capacity(num_chips * oversample);
        for k in 0..num_chips {
            for s in 0..oversample {
                let idx = (k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(pn_iq);
            }
        }

        // Use an LC gen that returns consistent values
        let lc_gen = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);

        // Also generate the expected LC signs for verification
        let mut lc_verify = lc_gen.clone();

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0, // despread_phase
            center_offset,
            lc_gen,
            0,
            0,     // lc_chip_counter, chain_start_chip
            64,    // chip_block_size
            0,     // samples_to_skip
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );

        finger.despread_block(&signal);
        let chips = finger.chip_buffer_as_slice();

        // Should get exactly num_chips chips (one per chip period)
        assert_eq!(chips.len(), num_chips);

        // Verify each chip was taken from sub-sample `center_offset` by
        // manually computing the expected PN despread at that position.
        for k in 0..num_chips {
            let sample_idx = k * oversample + center_offset;
            let pn_idx = sample_idx % phase_period;
            let pn = pn_conj[pn_idx];
            let pn_iq = Complex32::new(pn.re, -pn.im);
            let expected_despread = pn * pn_iq; // conj(pn) × pn = |pn|²

            let lc_chip = lc_verify.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            let expected = Complex32::new(
                expected_despread.re * lc_sign,
                expected_despread.im * lc_sign,
            );

            assert!(
                (chips[k].re - expected.re).abs() < 0.001
                    && (chips[k].im - expected.im).abs() < 0.001,
                "chip {}: got ({:.4}, {:.4}), expected ({:.4}, {:.4})",
                k,
                chips[k].re,
                chips[k].im,
                expected.re,
                expected.im
            );
        }
    }

    // =====================================================================
    // 3. samples_to_skip drops the correct number of leading samples
    // =====================================================================

    #[test]
    fn finger_samples_to_skip_drops_leading_samples() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let num_chips = 64;
        let skip_samples = 10usize; // skip first 10 samples
        let center_offset = 0usize;

        // Build signal: PN × LC
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut signal = Vec::with_capacity((num_chips + 4) * oversample);
        for k in 0..(num_chips + 4) {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // Finger WITHOUT skip — process same signal
        let lc_rx_no_skip =
            LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut finger_no_skip = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0, // despread_phase
            center_offset,
            lc_rx_no_skip,
            0,
            0,
            256,
            0,     // no skip
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );
        finger_no_skip.despread_block(&signal);
        let chips_no_skip = finger_no_skip.chip_buffer_as_slice();

        // Finger WITH skip — LC must be seeded at the chip boundary AFTER
        // the skipped samples.  With skip=10 and center_offset=0, the first
        // chip boundary after sample 10 is at sample 12 (chip 3).
        let _first_chip_after_skip = (skip_samples + oversample - 1) / oversample;
        // But actually, partial_sample_count starts at 0 after skip, and
        // the first chip boundary is when partial_sample_count % os == center_offset.
        // Since center_offset=0, the first chip is at partial_sample_count=0,
        // which is sample index skip_samples.  The corresponding PN chip is
        // skip_samples / oversample = 2 (for skip=10, os=4: 10/4 = 2.5, so
        // partial_sample_count=0 is NOT at a chip boundary relative to the
        // original PN... actually partial_sample_count just counts from 0
        // and hits center_offset=0 at partial_sample_count=0, so the FIRST
        // sample after the skip IS treated as a chip boundary).
        //
        // The despread_phase should be skip_samples (so the PN index for the
        // first sample after skip is correct).
        let despread_phase_with_skip = skip_samples;
        // First chip boundary: partial_sample_count=0, center_offset=0
        // PN index: despread_phase + 0 = skip_samples
        // LC chip: first_tx_chip
        // We need LC seeded at the chip that corresponds to sample skip_samples.
        // That chip is skip_samples / oversample = 2 (sample 8 is chip 2, but
        // skip_samples=10 means we start partway through chip 2).
        // Actually the finger fires LC at partial_count % os == center_offset.
        // With center_offset=0, first LC fire is at partial_count=0 = first
        // sample after skip.  So the chip index is skip_samples / os = 2
        // (integer division: 10/4 = 2).
        let first_chip = skip_samples / oversample;
        let mut lc_rx_skip =
            LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        lc_rx_skip.advance_chips(first_chip);

        let mut finger_skip = PnLcFinger::new(
            2,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            despread_phase_with_skip,
            center_offset,
            lc_rx_skip,
            first_chip, // lc_chip_counter
            first_chip, // chain_start_chip
            256,
            skip_samples, // samples_to_skip
            0.0,          // detection_snr
            0.0,          // initial_cfo_rad_per_chip
            1,            // lc_decimation
            false,        // enable_epl_tracking
            false,        // enable_epl_slew
            false,        // epl_pilot
            false,        // access_cfo
            0.0,          // timing_mu_samples
            false,        // output_oversampled_chips
            false,        // integrate_and_dump
        );
        finger_skip.despread_block(&signal);
        let chips_skip = finger_skip.chip_buffer_as_slice();

        // The skipped finger should produce chips starting from chip index
        // `first_chip`, matching the no-skip finger's output at that offset.
        assert!(chips_skip.len() > 0, "finger with skip produced no chips");
        assert!(
            chips_no_skip.len() > chips_skip.len(),
            "no-skip should produce more chips"
        );

        // Compare: chips_skip[0] should match chips_no_skip[first_chip]
        // (they process the same PN phase and LC phase at that chip)
        for i in 0..chips_skip.len().min(chips_no_skip.len() - first_chip) {
            let expected = chips_no_skip[first_chip + i];
            let got = chips_skip[i];
            assert!(
                (got.re - expected.re).abs() < 0.01 && (got.im - expected.im).abs() < 0.01,
                "chip {}: skip=({:.4},{:.4}) vs no_skip[{}]=({:.4},{:.4})",
                i,
                got.re,
                got.im,
                first_chip + i,
                expected.re,
                expected.im
            );
        }
    }

    // =====================================================================
    // 4. LC removal — verify that after PN+LC despread, a Walsh-0 preamble
    //    yields all-positive (DC) chips
    // =====================================================================

    #[test]
    fn finger_lc_removal_produces_dc_for_w0_preamble() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let chip_start = 0usize;
        let num_chips = 256;

        // TX: PN × LC (Walsh-0 preamble = LC sign only, no Walsh modulation)
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_tx.advance_chips(chip_start);

        let mut signal = Vec::with_capacity(num_chips * oversample);
        let mut lc_tx_copy = lc_tx.clone();
        for k in 0..num_chips {
            let lc_chip = lc_tx_copy.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (chip_start * oversample + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // RX finger with matching LC seed
        let mut lc_rx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_rx.advance_chips(chip_start);

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            chip_start * oversample, // despread_phase
            0,                       // center_offset
            lc_rx,
            chip_start, // lc_chip_counter
            chip_start, // chain_start_chip
            256,
            0,     // samples_to_skip
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );

        finger.despread_block(&signal);
        let chips = finger.chip_buffer_as_slice();

        assert_eq!(chips.len(), num_chips);

        let expected_re = chips[0].re;
        eprintln!(
            "W0 preamble despread: first chip = ({:.4}, {:.4}), expected constant",
            expected_re, chips[0].im
        );
        assert!(
            expected_re.abs() > 1.0,
            "despreaded chip magnitude too small: {:.4}",
            expected_re
        );

        for (i, chip) in chips.iter().enumerate() {
            assert!(
                (chip.re - expected_re).abs() < 0.01,
                "chip {} re={:.4} differs from expected {:.4}",
                i,
                chip.re,
                expected_re
            );
            assert!(
                chip.im.abs() < 0.01,
                "chip {} im={:.4} should be ~0",
                i,
                chip.im
            );
        }
    }

    // =====================================================================
    // 5. LC mismatch — wrong LC phase produces uncorrelated output
    // =====================================================================

    #[test]
    fn finger_wrong_lc_phase_produces_low_energy_output() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let chip_start = 0usize;
        let num_chips = 256;

        // TX with correct LC
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_tx.advance_chips(chip_start);
        let mut signal = Vec::with_capacity(num_chips * oversample);
        let mut lc_tx_copy = lc_tx.clone();
        for k in 0..num_chips {
            let lc_chip = lc_tx_copy.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // RX with WRONG LC phase (offset by 100 chips)
        let wrong_offset = 100usize;
        let mut lc_rx_wrong =
            LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_rx_wrong.advance_chips(chip_start + wrong_offset);

        let mut finger_wrong = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0,
            0,
            lc_rx_wrong,
            chip_start,
            chip_start,
            256,
            0,
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );
        finger_wrong.despread_block(&signal);
        let chips_wrong = finger_wrong.chip_buffer_as_slice();

        // RX with CORRECT LC phase
        let mut lc_rx_correct =
            LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_rx_correct.advance_chips(chip_start);
        let mut finger_correct = PnLcFinger::new(
            2,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0,
            0,
            lc_rx_correct,
            chip_start,
            chip_start,
            256,
            0,
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );
        finger_correct.despread_block(&signal);
        let chips_correct = finger_correct.chip_buffer_as_slice();

        // Correct: coherent sum should be large
        let correct_sum: Complex32 = chips_correct.iter().sum();
        let correct_energy = correct_sum.norm_sqr();

        // Wrong: incoherent sum should be much smaller
        let wrong_sum: Complex32 = chips_wrong.iter().sum();
        let wrong_energy = wrong_sum.norm_sqr();

        eprintln!(
            "correct energy: {:.2}, wrong energy: {:.2}, ratio: {:.1}",
            correct_energy,
            wrong_energy,
            correct_energy / wrong_energy.max(1e-9)
        );

        assert!(
            correct_energy > wrong_energy * 10.0,
            "correct LC should produce >10x more coherent energy: correct={:.2} wrong={:.2}",
            correct_energy,
            wrong_energy
        );
    }

    // =====================================================================
    // 6. base_phase / abs_chip_at / center_offset computations
    // =====================================================================

    #[test]
    fn helper_base_phase_accounts_for_filter_delay() {
        let mut cfg = PnLcConfig::default_4x();
        cfg.composite_filter_delay = 47;
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);

        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());
        correlator.absolute_origin_sample = Some(1000);

        let os = 4;
        let pp = 32768 * os;

        // base_phase = (abs + window_offset + pp - delay) % pp
        let bp = correlator.test_base_phase(0);
        assert_eq!(bp, (1000 + pp - 47) % pp);

        let bp2 = correlator.test_base_phase(2048);
        assert_eq!(bp2, (1000 + 2048 + pp - 47) % pp);
    }

    #[test]
    fn helper_abs_chip_at_divides_by_oversample() {
        let mut cfg = PnLcConfig::default_4x();
        cfg.composite_filter_delay = 47;
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);

        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());
        correlator.absolute_origin_sample = Some(400);

        // abs_chip_at = ((400 + offset) - 47) / 4
        assert_eq!(correlator.test_abs_chip_at(0), (400usize - 47) / 4); // 88
        assert_eq!(
            correlator.test_abs_chip_at(100),
            (400usize + 100usize - 47) / 4
        ); // 113
    }

    #[test]
    fn helper_center_offset_is_delay_mod_oversample() {
        let mut cfg = PnLcConfig::default_4x();
        cfg.composite_filter_delay = 47;
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());
        assert_eq!(correlator.test_center_offset(), 47 % 4); // = 3
    }

    #[test]
    fn suppresses_spawn_when_active_finger_delay_overlaps() {
        let mut cfg = PnLcConfig::default_4x();
        cfg = cfg.with_active_finger_delay_suppression(true, 8);
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());

        correlator.active_fingers.push(ActiveFingerState {
            id: 1,
            delay_samples: 654,
            hard_validated: false,
            idle_chips: 0,
            signal_lost_chips: 0,
            crc_miss_count: 0,
            post_walsh_no_event_ms: 0,
        });

        assert!(correlator.has_overlapping_active_finger(654));
        assert!(correlator.has_overlapping_active_finger(646));
        assert!(correlator.has_overlapping_active_finger(662));
        assert!(!correlator.has_overlapping_active_finger(663));
    }

    #[test]
    fn active_finger_delay_overlap_suppression_defaults_off() {
        let cfg = PnLcConfig::default_4x();
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());

        correlator.active_fingers.push(ActiveFingerState {
            id: 1,
            delay_samples: 654,
            hard_validated: false,
            idle_chips: 0,
            signal_lost_chips: 0,
            crc_miss_count: 0,
            post_walsh_no_event_ms: 0,
        });

        assert!(!correlator.has_overlapping_active_finger(654));
    }

    #[test]
    fn validated_finger_signal_loss_resumes_suppressed_search() {
        let cfg = PnLcConfig::default_4x().with_suppress_search_when_locked(true);
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());
        correlator.active_fingers.push(ActiveFingerState {
            id: 1,
            delay_samples: 654,
            hard_validated: true,
            idle_chips: 0,
            signal_lost_chips: 0,
            crc_miss_count: 0,
            post_walsh_no_event_ms: 0,
        });

        assert!(correlator.search_suppressed());

        correlator.active_fingers[0].signal_lost_chips = correlator.reacquire_signal_lost_chips;

        assert!(!correlator.search_suppressed());
        assert!(correlator.can_reacquire_over_active(654));
    }

    #[test]
    fn reanchor_ignores_small_forward_jitter() {
        let mut cfg = PnLcConfig::default_4x();
        cfg.reanchor_origin = true;
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());

        correlator.absolute_origin_sample = Some(1000);
        correlator.samples_consumed = 2048;
        correlator.buffer = vec![Complex32::new(0.0, 0.0); 32];
        correlator.recent_samples = vec![Complex32::new(0.0, 0.0); 16].into();
        correlator.recent_start_offset = 2000;
        correlator.pending_candidates.push(PendingCandidate {
            id: 7,
            delay_samples: 8,
            lc_phase_hint: 0,
            snr: 12.0,
            preamble_hits: 1,
            attempts: 1,
            first_verified_tx_chip: Some(300),
            first_verified_sample_offset: Some(1200),
            pn_reference_kind: None,
            timing_mu_samples: 0.0,
            timing_score: f32::MIN,
        });
        correlator.active_fingers.push(ActiveFingerState {
            id: 9,
            delay_samples: 8,
            hard_validated: true,
            idle_chips: 0,
            signal_lost_chips: 0,
            crc_miss_count: 0,
            post_walsh_no_event_ms: 0,
        });

        let mut block = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 8], 0)
            .with_sample_rate_hz(4.0 * 1_228_800.0);
        block
            .tags
            .insert("absolute_sample_start", (1000 + 2048 + 32 + 4) as i64);
        let detections = correlator.correlate(&block);

        assert!(detections.is_empty());
        assert_eq!(correlator.absolute_origin_sample, Some(1000));
        assert_eq!(correlator.buffer.len(), 40);
        assert_eq!(correlator.recent_samples.len(), 24);
        assert_eq!(correlator.pending_candidates.len(), 1);
        assert_eq!(correlator.active_fingers.len(), 1);
    }

    #[test]
    fn reanchor_flushes_seam_state_on_forward_gap() {
        let mut cfg = PnLcConfig::default_4x();
        cfg.reanchor_origin = true;
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut correlator = PnLcCorrelator::new(cfg, lc, noop_chain_builder());

        correlator.absolute_origin_sample = Some(1000);
        correlator.samples_consumed = 2048;
        correlator.buffer = vec![Complex32::new(0.0, 0.0); 32];
        correlator.recent_samples = vec![Complex32::new(0.0, 0.0); 16].into();
        correlator.recent_start_offset = 2000;
        correlator.search_paused = true;
        correlator.pending_candidates.push(PendingCandidate {
            id: 7,
            delay_samples: 8,
            lc_phase_hint: 0,
            snr: 12.0,
            preamble_hits: 1,
            attempts: 1,
            first_verified_tx_chip: Some(300),
            first_verified_sample_offset: Some(1200),
            pn_reference_kind: None,
            timing_mu_samples: 0.0,
            timing_score: f32::MIN,
        });
        correlator.active_fingers.push(ActiveFingerState {
            id: 9,
            delay_samples: 8,
            hard_validated: true,
            idle_chips: 0,
            signal_lost_chips: 0,
            crc_miss_count: 0,
            post_walsh_no_event_ms: 0,
        });

        let mut block = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 8], 0)
            .with_sample_rate_hz(4.0 * 1_228_800.0);
        block
            .tags
            .insert("absolute_sample_start", (1000 + 2048 + 32 + 20) as i64);
        let detections = correlator.correlate(&block);

        assert!(detections.is_empty());
        assert_eq!(correlator.absolute_origin_sample, Some(1020));
        assert_eq!(correlator.samples_consumed, 2080);
        assert_eq!(correlator.buffer.len(), 8);
        assert_eq!(correlator.recent_samples.len(), 8);
        assert_eq!(correlator.recent_start_offset, 2080);
        assert!(correlator.pending_candidates.is_empty());
        assert!(correlator.active_fingers.is_empty());
        assert!(!correlator.search_paused);
        assert_eq!(correlator.window_counter, 1);
    }

    #[test]
    #[ignore = "needs re-evaluation: reanchor LC coherence test may need updating after recent correlator changes"]
    fn reanchor_keeps_lc_phase_coherent_across_dropped_samples() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let coherent_chips = 256usize;
        let window_len = coherent_chips * oversample;
        let gap_samples = 40usize; // 10 chips; larger than the default lc_half_span
        let signal_windows = 3usize;
        let _signal_len = signal_windows * window_len;
        let block1_abs_start = window_len + gap_samples;
        let tx_chip_start = block1_abs_start / oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let lc_template = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut lc_tx = lc_template.clone();
        lc_tx.advance_chips(tx_chip_start);
        let signal = generate_tx_signal(
            &pn_conj,
            &mut lc_tx,
            block1_abs_start,
            coherent_chips * signal_windows,
            oversample,
        );

        let build_correlator = |reanchor_origin: bool| {
            let mut cfg = PnLcConfig::default_4x();
            cfg.composite_filter_delay = 0;
            cfg.search_interval_windows = 1;
            cfg.snr_threshold = 5.0;
            cfg.reanchor_origin = reanchor_origin;
            PnLcCorrelator::new(cfg, lc_template.clone(), noop_chain_builder())
        };

        let run = |correlator: &mut PnLcCorrelator| {
            let mut detections = Vec::new();

            let mut block0 = SampleBlock::new(vec![Complex32::new(0.0, 0.0); window_len], 0)
                .with_sample_rate_hz(4.0 * 1_228_800.0);
            block0.tags.insert("absolute_sample_start", 0);
            detections.extend(correlator.correlate(&block0));

            for window_idx in 0..signal_windows {
                let start = window_idx * window_len;
                let end = start + window_len;
                let mut block = SampleBlock::new(signal[start..end].to_vec(), 0)
                    .with_sample_rate_hz(4.0 * 1_228_800.0);
                block
                    .tags
                    .insert("absolute_sample_start", (block1_abs_start + start) as i64);
                detections.extend(correlator.correlate(&block));
            }

            detections
        };

        let mut without_reanchor = build_correlator(false);
        let no_reanchor_detections = run(&mut without_reanchor);
        let mut with_reanchor = build_correlator(true);
        let reanchor_detections = run(&mut with_reanchor);

        assert!(
            !reanchor_detections.is_empty(),
            "reanchor should preserve coherent LC search after a {}-sample drop",
            gap_samples
        );

        let no_reanchor_best_chip_err = no_reanchor_detections
            .iter()
            .map(|(finger, _)| finger.chain_start_chip.abs_diff(tx_chip_start))
            .min()
            .unwrap_or(usize::MAX);
        let reanchor_best_chip_err = reanchor_detections
            .iter()
            .map(|(finger, _)| finger.chain_start_chip.abs_diff(tx_chip_start))
            .min()
            .unwrap_or(usize::MAX);

        assert!(
            no_reanchor_best_chip_err > gap_samples / oversample / 2,
            "without reanchor the promoted finger should land on the wrong LC \
             origin after the drop; expected error > {} chips, got {}",
            gap_samples / oversample / 2,
            no_reanchor_best_chip_err
        );
        assert!(
            reanchor_best_chip_err <= 2,
            "reanchor should recover the correct LC-aligned chip start; \
             expected <= 2 chips error, got {}",
            reanchor_best_chip_err
        );
    }

    // =====================================================================
    // 7. Joint FFT search — find the correct (delay, lc_phase) for a known
    //    signal at zero filter delay (no pulse shaping)
    // =====================================================================

    #[test]
    #[ignore = "synthetic joint-search diagnostic; run explicitly when tuning PN+LC acquisition"]
    fn joint_search_finds_correct_delay_and_lc_phase() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let coherent_chips = 256usize;
        let window_len = coherent_chips * oversample;

        let pn_conj = build_pn_conj(phase_period, oversample);

        // Pick a known TX chip start and signal delay (in samples).
        let tx_chip_start = 100usize;
        let signal_delay = 20usize; // signal peak at sample 20

        let mut cfg = PnLcConfig::default_4x();
        cfg.coherent_chips = coherent_chips;
        cfg.composite_filter_delay = 0; // no filter delay for this test
        cfg.snr_threshold = 5.0;
        cfg.search_interval_windows = 1; // search every window

        let lc_template = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);

        // Build TX signal: insert `signal_delay` zeros then PN×LC.
        // Generate enough signal to cover at least 2 full windows, since the
        // correlator skips window 0 (window_counter > 0 guard).
        let mut lc_tx = lc_template.clone();
        lc_tx.advance_chips(tx_chip_start);

        let signal_chips = coherent_chips * 3; // enough for windows 0, 1, and 2
        let mut tx_samples = vec![Complex32::new(0.0, 0.0); signal_delay];
        let pn_start_sample = tx_chip_start * oversample;
        for k in 0..signal_chips {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (pn_start_sample + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                tx_samples.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }
        // Pad to at least 4 windows so the correlator has search opportunities
        while tx_samples.len() < window_len * 4 {
            tx_samples.push(Complex32::new(0.0, 0.0));
        }

        let mut correlator = PnLcCorrelator::new(cfg, lc_template, noop_chain_builder());

        // Feed the signal in window-sized chunks, mirroring the normal search
        // cadence. The stream starts `signal_delay` samples before the
        // PN-aligned TX onset, so absolute sample 0 is `signal_delay` samples
        // earlier than `tx_chip_start * oversample`.
        let abs_sample_start = tx_chip_start * oversample - signal_delay;
        let mut detections = Vec::new();
        for (i, chunk) in tx_samples.chunks(window_len).enumerate() {
            let mut block = SampleBlock::new(chunk.to_vec(), i * window_len)
                .with_sample_rate_hz(4.0 * 1_228_800.0);
            if i == 0 {
                block
                    .tags
                    .insert("absolute_sample_start", abs_sample_start as i64);
            }
            detections.extend(correlator.correlate(&block));
        }

        eprintln!("detections: {}", detections.len());
        for (finger, _chain) in &detections {
            eprintln!(
                "  finger id={} despread_phase={} next_prompt_offset={} chain_start_chip={}",
                finger.base.id,
                finger.despread_phase,
                finger.next_prompt_offset,
                finger.chain_start_chip,
            );
        }

        // We should get at least one detection
        assert!(
            !detections.is_empty(),
            "joint search should detect the signal"
        );
    }

    // =====================================================================
    // 8. End-to-end: correlator detects signal, finger despreads to W0
    // =====================================================================

    #[test]
    fn end_to_end_finger_produces_w0_from_preamble() {
        // Build a clean preamble signal (PN × LC, Walsh-0), feed it through
        // the full correlator → finger pipeline, and verify the output chips
        // are coherent (all same sign = W0).
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let chip_start = 0usize;
        let preamble_chips = 2048usize; // 8 Walsh symbols

        // TX: PN × LC (W0 preamble)
        let lc_template = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut lc_tx = lc_template.clone();
        lc_tx.advance_chips(chip_start);

        let mut tx_signal = Vec::with_capacity(preamble_chips * oversample);
        for k in 0..preamble_chips {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (chip_start * oversample + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                tx_signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // Manually create a finger at the known correct parameters
        let mut lc_rx = lc_template.clone();
        lc_rx.advance_chips(chip_start);

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            chip_start * oversample, // despread_phase
            0,                       // center_offset (no filter delay)
            lc_rx,
            chip_start, // lc_chip_counter
            chip_start, // chain_start_chip
            256,        // chip_block_size
            0,          // samples_to_skip
            0.0,        // detection_snr
            0.0,        // initial_cfo_rad_per_chip
            1,          // lc_decimation
            false,      // enable_epl_tracking
            false,      // enable_epl_slew
            false,      // epl_pilot
            false,      // access_cfo
            0.0,        // timing_mu_samples
            false,      // output_oversampled_chips
            false,      // integrate_and_dump
        );

        // Process through the finger (no sub-chain, just accumulate chips)
        let mut chain: Vec<PipelineProcessorShared> = Vec::new();
        let block =
            SampleBlock::new(tx_signal, 0).with_sample_rate_hz(oversample as f64 * 1_228_800.0);
        let out = finger.process(&block, &mut chain);

        // The finger now batches all available 256-chip windows into one
        // downstream block per process() call.
        let expected_blocks = 1usize;
        assert_eq!(
            out.len(),
            expected_blocks,
            "expected {} output blocks, got {}",
            expected_blocks,
            out.len()
        );

        // The single block should contain the full preamble as coherent DC (W0).
        for (blk_idx, blk) in out.iter().enumerate() {
            assert_eq!(blk.samples.len(), preamble_chips);

            let sum: Complex32 = blk.samples.iter().sum();
            let mean_re = sum.re / preamble_chips as f32;
            let mean_im = sum.im / preamble_chips as f32;

            // For perfect W0, all chips have the same sign → mean ≈ chip value
            // The individual chip values should be ~2.0 (|pn|² = 1² + 1² = 2)
            eprintln!(
                "block {}: mean=({:.4}, {:.4}), |sum|={:.2}",
                blk_idx,
                mean_re,
                mean_im,
                sum.norm()
            );

            // Coherent sum should be large (2048 × ~2.0 = ~4096)
            assert!(
                sum.norm() > 2000.0,
                "block {} coherent sum {:.2} too small for W0",
                blk_idx,
                sum.norm()
            );

            // Check that all chips have the same sign (W0 = all +1)
            let first_sign = blk.samples[0].re.signum();
            for (i, chip) in blk.samples.iter().enumerate() {
                assert!(
                    chip.re.signum() == first_sign,
                    "block {} chip {} sign mismatch: {:.4} vs first {:.4}",
                    blk_idx,
                    i,
                    chip.re,
                    blk.samples[0].re
                );
            }
        }
    }

    // =====================================================================
    // 9. Verify that a non-zero despread_phase correctly aligns PN
    // =====================================================================

    #[test]
    fn finger_nonzero_despread_phase_aligns_pn() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let chip_offset = 500usize; // start partway into the PN period
        let num_chips = 256;
        let despread_phase = chip_offset * oversample;

        // TX signal starting at PN phase = chip_offset
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_tx.advance_chips(chip_offset);

        let mut signal = Vec::with_capacity(num_chips * oversample);
        let mut lc_tx_copy = lc_tx.clone();
        for k in 0..num_chips {
            let lc_chip = lc_tx_copy.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (despread_phase + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        let mut lc_rx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_rx.advance_chips(chip_offset);

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            despread_phase, // non-zero phase
            0,              // center_offset
            lc_rx,
            chip_offset,
            chip_offset,
            256,
            0,
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );

        finger.despread_block(&signal);
        let chips = finger.chip_buffer_as_slice();

        assert_eq!(chips.len(), num_chips);

        // All chips should be the same positive real value (W0 despread)
        let first = chips[0];
        assert!(
            first.re.abs() > 1.5,
            "chip magnitude too small: {:.4}",
            first.re
        );
        assert!(
            first.im.abs() < 0.01,
            "imaginary should be ~0: {:.4}",
            first.im
        );

        for (i, chip) in chips.iter().enumerate() {
            assert!(
                (chip.re - first.re).abs() < 0.01,
                "chip {} re={:.4} differs from first {:.4}",
                i,
                chip.re,
                first.re
            );
        }
    }

    // =====================================================================
    // 10. chain_start_chip gate — chips before gate are suppressed
    // =====================================================================

    #[test]
    fn finger_chain_start_chip_gates_output() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let num_chips = 64;
        let gate_chip = 10usize; // only output chips >= 10

        // Simple signal: all ones (PN will modulate it but we just care about count)
        let signal = vec![Complex32::new(1.0, 0.0); num_chips * oversample];

        let lc_gen = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0,
            0, // center_offset
            lc_gen,
            0,         // lc_chip_counter starts at 0
            gate_chip, // chain_start_chip = 10
            256,
            0,
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );

        finger.despread_block(&signal);
        let chips = finger.chip_buffer_as_slice();

        // Should get num_chips - gate_chip = 54 chips
        assert_eq!(
            chips.len(),
            num_chips - gate_chip,
            "expected {} chips (gated), got {}",
            num_chips - gate_chip,
            chips.len()
        );
    }

    // =====================================================================
    // 11. Multiple blocks — verify state continuity across calls
    // =====================================================================

    #[test]
    fn finger_state_continuous_across_multiple_blocks() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);

        let total_chips = 512;

        // TX signal
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut signal = Vec::with_capacity(total_chips * oversample);
        for k in 0..total_chips {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                signal.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // Process in one shot
        let lc_rx1 = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut finger1 = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0,
            0,
            lc_rx1,
            0,
            0,
            256,
            0,
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );
        finger1.despread_block(&signal);
        let chips_one_shot = finger1.chip_buffer_as_slice();

        // Process in 4 blocks of 128 chips each
        let lc_rx2 = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut finger2 = PnLcFinger::new(
            2,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            0,
            0,
            lc_rx2,
            0,
            0,
            256,
            0,
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );
        let block_samples = 128 * oversample;
        for chunk in signal.chunks(block_samples) {
            finger2.despread_block(chunk);
        }
        let chips_multi = finger2.chip_buffer_as_slice();

        assert_eq!(chips_one_shot.len(), chips_multi.len());

        for (i, (a, b)) in chips_one_shot.iter().zip(chips_multi.iter()).enumerate() {
            assert!(
                (a.re - b.re).abs() < 1e-6 && (a.im - b.im).abs() < 1e-6,
                "chip {} mismatch: one_shot=({:.6},{:.6}) multi=({:.6},{:.6})",
                i,
                a.re,
                a.im,
                b.re,
                b.im
            );
        }
    }

    #[test]
    fn finger_raw_power_tracks_each_emitted_input_window() {
        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);
        let mut lc_tx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let signal = generate_tx_signal(&pn_conj, &mut lc_tx, 0, 512, oversample);
        let lc_rx = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut finger = PnLcFinger::new(
            1,
            pn_conj,
            phase_period,
            oversample,
            0,
            0,
            lc_rx,
            0,
            0,
            256,
            0,
            0.0,
            0.0,
            1,
            false,
            false,
            false,
            false,
            0.0,
            false,
            false,
        );
        let mut chain = Vec::new();
        let samples_per_window = 256 * oversample;
        let first = SampleBlock::new(signal[..samples_per_window].to_vec(), 0)
            .with_sample_rate_hz(4.0 * 1_228_800.0);
        let second = SampleBlock::new(
            signal[samples_per_window..]
                .iter()
                .map(|sample| sample * 0.1)
                .collect(),
            samples_per_window,
        )
        .with_sample_rate_hz(4.0 * 1_228_800.0);

        let first_power_mdb = finger.process(&first, &mut chain)[0].tags["finger_raw_power_mdb"];
        let second_power_mdb = finger.process(&second, &mut chain)[0].tags["finger_raw_power_mdb"];

        assert!(
            (19_990..=20_010).contains(&(first_power_mdb - second_power_mdb)),
            "expected each output to report its own input window: first={first_power_mdb} second={second_power_mdb}"
        );
    }

    // =====================================================================
    // 12. Pulse-shaped signal — verify FFT cross-correlation peak
    //
    // This is the critical test: generate a PN×LC signal, apply TX FIR +
    // RX matched filter (exactly as the real pipeline does), then manually
    // run the FFT cross-correlation at the known-correct LC phase and
    // verify the peak is dramatically stronger than wrong LC phases.
    // =====================================================================

    #[test]
    fn pulse_shaped_cross_correlation_has_clear_peak() {
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};
        use rustfft::FftPlanner;

        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let coherent_chips = 256usize;
        let window_len = coherent_chips * oversample; // 1024

        let pn_conj = build_pn_conj(phase_period, oversample);
        let taps = cdma2000_baseband_filter_taps_f64();
        let filter_len = taps.len(); // 48
        let composite_delay = filter_len - 1; // 47

        // --- TX signal: PN × LC (W0 preamble), then pulse shaped (1 FIR pass) ---
        let chip_start = 0usize;
        // Generate extra chips to account for filter transients
        let extra_chips = (composite_delay / oversample) + 2;
        let gen_chips = coherent_chips + extra_chips * 2;

        let lc_template = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut lc_tx = lc_template.clone();
        lc_tx.advance_chips(chip_start);

        let mut tx_raw = Vec::with_capacity(gen_chips * oversample);
        for k in 0..gen_chips {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (chip_start * oversample + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // TX pulse shaping (1 FIR pass)
        let tx_shaped = ComplexFir32::new(&taps).process_block(&tx_raw);

        // RX matched filter (1 FIR pass)
        let rx_signal = ComplexFir32::new(&taps).process_block(&tx_shaped);

        // Extract the window starting after the filter settling time.
        // After 2 FIR passes the peak is delayed by composite_delay samples.
        // We offset to align the known PN×LC start.
        let window_start = composite_delay;
        assert!(
            rx_signal.len() >= window_start + window_len,
            "signal too short: {} < {}",
            rx_signal.len(),
            window_start + window_len
        );
        let window: Vec<Complex32> = rx_signal[window_start..window_start + window_len].to_vec();

        // --- FFT cross-correlation (mirroring run_joint_search logic) ---
        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(window_len);
        let fft_inv = planner.plan_fft_inverse(window_len);
        let mut scratch = vec![
            Complex32::new(0.0, 0.0);
            fft_fwd
                .get_inplace_scratch_len()
                .max(fft_inv.get_inplace_scratch_len())
        ];

        let mut signal_fft = window.clone();
        fft_fwd.process_with_scratch(&mut signal_fft, &mut scratch);

        let norm = 1.0 / (window_len as f32 * window_len as f32);

        // The PN phase at the start of the window (after filter delay):
        // The TX signal starts at chip_start=0 in PN space.  After
        // composite_delay samples of filter settling, the window starts at
        // sample index composite_delay.  Compensating for the filter delay:
        // base_phase = (0 + composite_delay - composite_delay) % pp = 0
        let base_phase = 0usize;

        // --- Test correct LC phase (lc_phase = 0) ---
        let expected_chip = chip_start; // no filter delay offset since base_phase=0
        let mut lc_correct = lc_template.clone();
        lc_correct.advance_chips(expected_chip);

        let mut ref_correct = vec![Complex32::new(0.0, 0.0); window_len];
        for k in 0..coherent_chips {
            let lc_sign: f32 = if lc_correct.next_chip() == 1 {
                -1.0
            } else {
                1.0
            };
            for s in 0..oversample {
                let pn_c = pn_conj[(base_phase + k * oversample + s) % phase_period];
                ref_correct[k * oversample + s] =
                    Complex32::new(pn_c.re * lc_sign, -pn_c.im * lc_sign);
            }
        }
        fft_fwd.process_with_scratch(&mut ref_correct, &mut scratch);

        let mut xcorr_correct: Vec<Complex32> = signal_fft
            .iter()
            .zip(ref_correct.iter())
            .map(|(&s, &r)| s * Complex32::new(r.re, -r.im))
            .collect();
        fft_inv.process_with_scratch(&mut xcorr_correct, &mut scratch);

        let correct_powers: Vec<f32> = xcorr_correct.iter().map(|c| c.norm_sqr() * norm).collect();
        let correct_peak = correct_powers.iter().cloned().fold(0.0f32, f32::max);
        let correct_peak_idx = correct_powers
            .iter()
            .position(|&p| p == correct_peak)
            .unwrap();
        let correct_avg: f32 = correct_powers.iter().sum::<f32>() / correct_powers.len() as f32;

        eprintln!(
            "correct LC: peak={:.3e} at delay={} avg={:.3e} SNR={:.1}x",
            correct_peak,
            correct_peak_idx,
            correct_avg,
            correct_peak / correct_avg.max(1e-20)
        );

        // --- Test a WRONG LC phase (lc_phase = +50) ---
        let wrong_lc_offset = 50i32;
        let wrong_chip = (expected_chip as i64 + wrong_lc_offset as i64).max(0) as usize;
        let mut lc_wrong = lc_template.clone();
        lc_wrong.advance_chips(wrong_chip);

        let mut ref_wrong = vec![Complex32::new(0.0, 0.0); window_len];
        for k in 0..coherent_chips {
            let lc_sign: f32 = if lc_wrong.next_chip() == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let pn_c = pn_conj[(base_phase + k * oversample + s) % phase_period];
                ref_wrong[k * oversample + s] =
                    Complex32::new(pn_c.re * lc_sign, -pn_c.im * lc_sign);
            }
        }
        fft_fwd.process_with_scratch(&mut ref_wrong, &mut scratch);

        let mut xcorr_wrong: Vec<Complex32> = signal_fft
            .iter()
            .zip(ref_wrong.iter())
            .map(|(&s, &r)| s * Complex32::new(r.re, -r.im))
            .collect();
        fft_inv.process_with_scratch(&mut xcorr_wrong, &mut scratch);

        let wrong_powers: Vec<f32> = xcorr_wrong.iter().map(|c| c.norm_sqr() * norm).collect();
        let wrong_peak = wrong_powers.iter().cloned().fold(0.0f32, f32::max);
        let wrong_peak_idx = wrong_powers.iter().position(|&p| p == wrong_peak).unwrap();
        let wrong_avg: f32 = wrong_powers.iter().sum::<f32>() / wrong_powers.len() as f32;

        eprintln!(
            "wrong LC (+{}): peak={:.3e} at delay={} avg={:.3e} SNR={:.1}x",
            wrong_lc_offset,
            wrong_peak,
            wrong_peak_idx,
            wrong_avg,
            wrong_peak / wrong_avg.max(1e-20)
        );

        let ratio = correct_peak / wrong_peak.max(1e-20);
        eprintln!("correct/wrong peak ratio: {:.1}x", ratio);

        // The correct LC phase should produce a dramatically higher peak.
        // With 256 chips of coherent integration and a noiseless signal,
        // the ratio should be at least 10x (typically >> 100x).
        assert!(
            ratio > 10.0,
            "correct LC peak ({:.3e}) should be >10x the wrong LC peak ({:.3e}), got {:.1}x",
            correct_peak,
            wrong_peak,
            ratio
        );

        // Also verify the correct-LC SNR (peak vs own average) is high
        let correct_snr = correct_peak / correct_avg.max(1e-20);
        assert!(
            correct_snr > 50.0,
            "correct LC SNR {:.1}x too low (expected >50x)",
            correct_snr
        );
    }

    // =====================================================================
    // 13. Pulse-shaped signal through full correlator pipeline —
    //     verify detection at correct LC phase
    // =====================================================================

    #[test]
    fn correlator_detects_pulse_shaped_signal_at_correct_lc_phase() {
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);
        let taps = cdma2000_baseband_filter_taps_f64();
        let filter_len = taps.len();
        let composite_delay = filter_len - 1; // 47

        let chip_start = 0usize;
        let preamble_chips = 4096usize; // enough for multiple search windows

        let lc_template = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut lc_tx = lc_template.clone();
        lc_tx.advance_chips(chip_start);

        // TX: NRZ PN×LC
        let mut tx_raw = Vec::with_capacity(preamble_chips * oversample);
        for k in 0..preamble_chips {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (chip_start * oversample + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // TX pulse shape (1 FIR pass)
        let tx_shaped = ComplexFir32::new(&taps).process_block(&tx_raw);

        // RX matched filter (1 FIR pass)
        let rx_signal = ComplexFir32::new(&taps).process_block(&tx_shaped);

        // Set up correlator with composite_filter_delay = 47
        let mut cfg = PnLcConfig::default_4x();
        cfg.composite_filter_delay = composite_delay;
        cfg.snr_threshold = 20.0; // high threshold — only the real signal should pass
        cfg.search_interval_windows = 1; // search every window
        let max_replay_chips = cfg.replay_preamble_symbols * cfg.chip_block_size + 2; // small margin

        let mut correlator = PnLcCorrelator::new(cfg, lc_template, noop_chain_builder());

        let sample_rate_hz = oversample as f64 * 1_228_800.0;
        let abs_sample_start = chip_start * oversample;

        // Feed in blocks of 2048 samples
        let block_size = 2048usize;
        let mut all_detections = Vec::new();

        for (i, chunk) in rx_signal.chunks(block_size).enumerate() {
            let mut block = SampleBlock::new(chunk.to_vec(), i * block_size)
                .with_sample_rate_hz(sample_rate_hz);
            if i == 0 {
                block
                    .tags
                    .insert("absolute_sample_start", abs_sample_start as i64);
            }
            let detections = correlator.correlate(&block);
            for (finger, _chain) in &detections {
                eprintln!(
                    "  detection: finger id={} despread_phase={} next_prompt_offset={} \
                     chain_start_chip={} lc_chip_counter={}",
                    finger.base.id,
                    finger.despread_phase,
                    finger.next_prompt_offset,
                    finger.chain_start_chip,
                    finger.lc_chip_counter,
                );
            }
            all_detections.extend(detections);
        }

        eprintln!("total detections: {}", all_detections.len());
        assert!(
            !all_detections.is_empty(),
            "correlator should detect the pulse-shaped signal"
        );

        // Verify the detected finger was found at lc_phase ≈ 0.
        // chain_start_chip = tx_chip (the absolute LC chip at finger start).
        // lc_chip_counter advances as the finger despreads — it includes any
        // replay_preamble_symbols worth of history replayed at spawn time.
        // The maximum replay is replay_preamble_symbols × chip_block_size.
        // Derive the bound from the config under test so the assertion stays
        // aligned with the actual finger seeding policy.
        for (finger, _) in &all_detections {
            let chip_adj = finger.lc_chip_counter as i64 - finger.chain_start_chip as i64;
            eprintln!(
                "  finger {} chain_start_chip={} lc_chip_counter={} chip_adj={}",
                finger.base.id, finger.chain_start_chip, finger.lc_chip_counter, chip_adj
            );
            assert!(
                chip_adj >= 0 && chip_adj <= max_replay_chips as i64,
                "finger {} chip_adjustment {} out of range (expected 0..={})",
                finger.base.id,
                chip_adj,
                max_replay_chips
            );
        }
    }

    // =====================================================================
    // 14. Pulse-shaped finger despread — verify that a finger with correct
    //     center_offset produces clean W0 from a pulse-shaped signal
    // =====================================================================

    #[test]
    fn finger_despreads_pulse_shaped_signal_to_w0() {
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

        let oversample = 4usize;
        let phase_period = 32768 * oversample;
        let pn_conj = build_pn_conj(phase_period, oversample);
        let taps = cdma2000_baseband_filter_taps_f64();
        let composite_delay = taps.len() - 1; // 47

        let chip_start = 0usize;
        let num_chips = 512; // 2 Walsh symbols

        // TX: NRZ PN×LC (W0 preamble)
        let lc_template = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        let mut lc_tx = lc_template.clone();
        lc_tx.advance_chips(chip_start);

        let mut tx_raw = Vec::with_capacity(num_chips * oversample);
        for k in 0..num_chips {
            let lc_chip = lc_tx.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for s in 0..oversample {
                let idx = (chip_start * oversample + k * oversample + s) % phase_period;
                let pn_iq = Complex32::new(pn_conj[idx].re, -pn_conj[idx].im);
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        // TX FIR + RX matched filter
        let tx_shaped = ComplexFir32::new(&taps).process_block(&tx_raw);
        let rx_signal = ComplexFir32::new(&taps).process_block(&tx_shaped);

        // The finger receives the RX signal starting at sample 0.
        // The filter delay means the first clean chip is at sample
        // composite_delay.  The despread_phase should account for this:
        //   despread_phase = (0 + 0 - composite_delay) mod pp
        //                  = (pp - composite_delay) mod pp
        let _despread_phase = (phase_period - composite_delay) % phase_period;
        let center_offset = composite_delay % oversample; // = 47 % 4 = 3

        // The first chip boundary is at partial_sample_count = center_offset = 3.
        // At that point, the absolute sample index is 3, and the PN phase is:
        //   (despread_phase + 3) % pp = (pp - 47 + 3) % pp = pp - 44
        // The corresponding TX chip is: (pp - 44) / os = ... depends on pp.
        // But for chip_start=0, the first TX chip the finger produces is:
        //   ((0 - composite_delay + center_offset) / oversample)
        //   = (-47 + 3) / 4 = -44/4 = -11 ... negative means no valid chip
        // Actually, with the filter settling the first few chips are corrupted.
        // The finger will start outputting from sample 0 (no skip), but the
        // first chip will be at sample index center_offset=3 in the finger's
        // internal counter.

        // Skip the filter settling time and start the finger at sample
        // composite_delay.  despread_phase=0 because the TX chip that peaks
        // at rx_signal[composite_delay] is chip 0 (delay fully compensated).
        // center_offset=composite_delay%os=3 picks the correct sub-sample.
        //
        // The first chip boundary is at partial_sample_count=center_offset=3,
        // corresponding to TX sample 3 = chip 0.  So LC starts at chip 0.
        let skip = composite_delay;
        let despread_phase = 0usize;
        let first_chip = (despread_phase + center_offset) / oversample; // (0+3)/4 = 0

        let mut lc_rx = lc_template.clone();
        lc_rx.advance_chips(chip_start + first_chip);

        let mut finger = PnLcFinger::new(
            1,
            Arc::clone(&pn_conj),
            phase_period,
            oversample,
            despread_phase,
            center_offset,
            lc_rx,
            chip_start + first_chip,
            chip_start + first_chip,
            256,
            skip,  // skip the filter settling samples
            0.0,   // detection_snr
            0.0,   // initial_cfo_rad_per_chip
            1,     // lc_decimation
            false, // enable_epl_tracking
            false, // enable_epl_slew
            false, // epl_pilot
            false, // access_cfo
            0.0,   // timing_mu_samples
            false, // output_oversampled_chips
            false, // integrate_and_dump
        );

        finger.despread_block(&rx_signal);
        let chips = finger.chip_buffer_as_slice();

        // We should get about (num_chips - first_chip - edge_chips) chips.
        // Some chips at the end will be corrupted by filter edge effects.
        eprintln!(
            "pulse-shaped finger: {} chips, first chip = ({:.4}, {:.4})",
            chips.len(),
            chips.first().map(|c| c.re).unwrap_or(0.0),
            chips.first().map(|c| c.im).unwrap_or(0.0),
        );
        assert!(
            chips.len() >= 256,
            "expected at least 256 chips, got {}",
            chips.len()
        );

        // Check coherent sum of first 256 chips (should be strong W0)
        let first_block: Vec<Complex32> = chips[..256].to_vec();
        let sum: Complex32 = first_block.iter().sum();
        let mean = Complex32::new(sum.re / 256.0, sum.im / 256.0);

        eprintln!(
            "  first 256 chips: sum=({:.2}, {:.2}) mean=({:.4}, {:.4}) |sum|={:.2}",
            sum.re,
            sum.im,
            mean.re,
            mean.im,
            sum.norm()
        );

        // For W0 preamble, all chips should have the same sign (coherent).
        // The pulse shaping changes the amplitude envelope but not the sign
        // at the chip boundary (center_offset).  However, the inter-chip
        // samples are smoothed.
        //
        // Check that the coherent sum is large relative to the incoherent sum.
        let incoh_sum: f32 = first_block.iter().map(|c| c.norm_sqr()).sum();
        let coh_energy = sum.norm_sqr();
        let ratio = coh_energy / incoh_sum.max(1e-9);

        eprintln!(
            "  coherent energy: {:.2}, incoherent sum: {:.2}, ratio: {:.4}",
            coh_energy, incoh_sum, ratio
        );

        // For perfect W0, coh_energy / incoh = 256 (all chips same sign).
        // With pulse shaping the amplitude varies, but the sign should be
        // consistent so the ratio should be high (>100).
        assert!(
            ratio > 100.0,
            "coherent/incoherent ratio {:.2} too low for W0 (expected >100)",
            ratio
        );
    }
}
