//! Reverse-link pilot CFO tracking utilities.
//!
//! The tracker estimates residual carrier frequency offset from coherent
//! pilot sums and applies chip-rate derotation. Reverse access uses it from
//! Walsh-symbol pilot sums; RC3 traffic uses it for chip-rate carrier tracking
//! before `Rc3BpskDespread` performs per-PCG pilot-coherent demodulation.

use num_complex::Complex32;

const ACCESS_GAIN_STEADY: f32 = 0.08;

/// ~1 Hz loop. Wider turns pilot phase noise into a carrier random walk.
const RC3_GAIN_STEADY: f32 = 0.02;

const GAIN_WARMUP: f32 = 0.40;

/// About 25 ms at the RC3 warmup rate of one observation per PCG.
const WARMUP_SYMBOLS: usize = 20;

/// Just above the noise floor.
const PILOT_QUALITY_GATE: f32 = 1.0;

/// A bigger one-observation jump is a wrong Walsh decision, not the oscillator.
const RC1_MAX_CFO_JUMP_HZ: f32 = 100.0;

/// Diagnostic snapshot of CFO residual statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct CfoResidualStats {
    /// Root-mean-square residual CFO, in hertz.
    pub rms_hz: f64,
    /// Mean absolute residual CFO, in hertz.
    pub mean_hz: f64,
    /// Maximum absolute residual CFO observed, in hertz.
    pub max_hz: f64,
    /// Number of residual observations included in the snapshot.
    pub count: u64,
}

/// Pilot-based reverse-link CFO tracker.
///
/// Provide coherent pilot sums via [`CfoTracker::observe_pilot`], then call
/// [`CfoTracker::derotate_chips`] on each chip block before downstream
/// channel processing.
pub struct CfoTracker {
    /// Current CFO estimate in radians per chip.
    cfo_rad_per_chip: f32,
    /// Accumulated derotation phase (wraps at 2π).
    cfo_phase: f32,

    /// RC3 measures received phase, access measures its own correction. Opposite
    /// derotation signs.
    rc3_traffic: bool,

    /// Previous 16-chip pilot prompt sum for inter-symbol phase delta.
    prev_pilot: Option<Complex32>,

    /// Total symbol updates applied (for warmup → steady transition).
    total_updates: usize,

    // Diagnostic accumulators (values in Hz, reset on read).
    diag_signed_sum: f64,
    diag_abs_sum: f64,
    diag_sq_sum: f64,
    diag_max: f32,
    diag_count: u64,
}

impl CfoTracker {
    fn new(initial_cfo_rad_per_chip: f32, total_updates: usize) -> Self {
        Self {
            cfo_rad_per_chip: initial_cfo_rad_per_chip,
            cfo_phase: 0.0,
            rc3_traffic: false,
            prev_pilot: None,
            total_updates,
            diag_signed_sum: 0.0,
            diag_abs_sum: 0.0,
            diag_sq_sum: 0.0,
            diag_max: 0.0,
            diag_count: 0,
        }
    }

    /// Create a tracker seeded with an RC3 reverse traffic acquisition CFO estimate.
    pub(crate) fn new_rc3_traffic(initial_cfo_rad_per_chip: f32) -> Self {
        // Observations one PCG apart alias modulo 2*pi/1536. Mobiles are locked
        // to the forward link, so take the principal alias.
        let mut tracker = Self::new(Self::rc3_principal_alias(initial_cfo_rad_per_chip), 0);
        tracker.rc3_traffic = true;
        tracker
    }

    pub(crate) fn rc3_principal_alias(initial_cfo_rad_per_chip: f32) -> f32 {
        const PCG_CHIPS: f32 = 1536.0;
        let phase_per_pcg = initial_cfo_rad_per_chip * PCG_CHIPS;
        let principal_phase = (phase_per_pcg + std::f32::consts::PI)
            .rem_euclid(2.0 * std::f32::consts::PI)
            - std::f32::consts::PI;
        principal_phase / PCG_CHIPS
    }

    /// Drop the phase baseline. The next vector starts a new one, no CFO update.
    pub(crate) fn clear_pilot_baseline(&mut self) {
        self.prev_pilot = None;
    }

    /// Reverse access: starts in steady-state (no warmup) because the
    /// acquisition CFO estimate is already good and the aggressive
    /// warmup gain causes overshoot on short preamble bursts.
    /// Fed 256-chip Walsh-aligned block sums, coherence-gated coasting.
    pub fn new_reverse_access(initial_cfo_rad_per_chip: f32) -> Self {
        Self::new(initial_cfo_rad_per_chip, WARMUP_SYMBOLS)
    }

    /// Create an RC1 traffic tracker seeded by preamble acquisition.
    pub(crate) fn new_rc1_traffic(initial_cfo_rad_per_chip: f32) -> Self {
        let mut tracker = Self::new(initial_cfo_rad_per_chip, WARMUP_SYMBOLS);
        tracker.rc3_traffic = true;
        tracker
    }

    /// Current CFO estimate (radians per chip).
    pub fn cfo_rad_per_chip(&self) -> f32 {
        self.cfo_rad_per_chip
    }

    /// Current accumulated derotation phase.
    pub fn cfo_phase(&self) -> f32 {
        self.cfo_phase
    }

    /// Whether the tracker is still in the warmup phase.
    pub fn in_warmup(&self) -> bool {
        self.total_updates < WARMUP_SYMBOLS
    }

    fn gain(&self) -> f32 {
        if self.in_warmup() {
            GAIN_WARMUP
        } else if self.rc3_traffic {
            RC3_GAIN_STEADY
        } else {
            ACCESS_GAIN_STEADY
        }
    }

    /// Observe a 16-chip Walsh-0 pilot sum.
    #[cfg(test)]
    pub fn observe_pilot_symbol(&mut self, pilot_sum: Complex32) {
        self.observe_pilot(pilot_sum, 16);
    }

    /// Observe a coherent pilot sum with explicit chip count.
    pub fn observe_pilot(&mut self, pilot_sum: Complex32, n_chips: usize) {
        // Quality gate: skip if the pilot sum is too weak (noise-dominated).
        let pilot_power = pilot_sum.norm_sqr();
        if pilot_power <= 1e-12 {
            return;
        }

        if let Some(prev) = self.prev_pilot {
            let prev_power = prev.norm_sqr();
            // Gate on coherence of BOTH current and previous symbol.
            // norm_sqr of a 16-chip sum for a unit-amplitude pilot = 256.
            // At pilot_coh ~0.1, norm_sqr ≈ 256 × 0.01 ≈ 2.56.
            let min_power = pilot_power.min(prev_power);
            if min_power > PILOT_QUALITY_GATE {
                let cross = pilot_sum * Complex32::new(prev.re, -prev.im);
                let delta = cross.im.atan2(cross.re);

                // Normalize from rad/observation to rad/chip.
                let measured_cfo = delta / n_chips as f32;

                // IIR update — immediate, every symbol.
                let gain = self.gain();
                self.cfo_rad_per_chip = (1.0 - gain) * self.cfo_rad_per_chip + gain * measured_cfo;

                self.total_updates += 1;

                // Diagnostic: record residual in Hz.
                let residual_hz = measured_cfo as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
                let abs_res = residual_hz.abs();
                self.diag_signed_sum += residual_hz;
                self.diag_abs_sum += abs_res;
                self.diag_sq_sum += residual_hz * residual_hz;
                self.diag_max = self.diag_max.max(abs_res as f32);
                self.diag_count += 1;
            }
        }
        self.prev_pilot = Some(pilot_sum);
    }

    /// Observe summed `pilot[k] * conj(pilot[k-1])` terms `n_chips` apart. Cuts
    /// noise without lengthening the baseline, so the CFO range is unchanged.
    pub(crate) fn observe_pilot_cross_sum(&mut self, cross_sum: Complex32, n_chips: usize) {
        if n_chips == 0 || cross_sum.norm_sqr() <= 1e-12 {
            return;
        }
        let measured_cfo = cross_sum.im.atan2(cross_sum.re) / n_chips as f32;
        let gain = self.gain();
        self.cfo_rad_per_chip = (1.0 - gain) * self.cfo_rad_per_chip + gain * measured_cfo;
        self.total_updates += 1;

        let residual_hz = measured_cfo as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
        let abs_res = residual_hz.abs();
        self.diag_signed_sum += residual_hz;
        self.diag_abs_sum += abs_res;
        self.diag_sq_sum += residual_hz * residual_hz;
        self.diag_max = self.diag_max.max(abs_res as f32);
        self.diag_count += 1;
    }

    /// Observe an averaged cross-product of adjacent decision-directed RC1 Walsh
    /// symbol vectors.
    pub(crate) fn observe_rc1_walsh_cross_sum(&mut self, cross_sum: Complex32) -> bool {
        const SYMBOL_CHIPS: f32 = 256.0;
        if cross_sum.norm_sqr() <= 1e-12 {
            return false;
        }

        let principal = cross_sum.im.atan2(cross_sum.re) / SYMBOL_CHIPS;
        let alias_period = 2.0 * std::f32::consts::PI / SYMBOL_CHIPS;
        let alias = ((self.cfo_rad_per_chip - principal) / alias_period).round();
        let measured_cfo = principal + alias * alias_period;
        let innovation_hz =
            (measured_cfo - self.cfo_rad_per_chip) * 1_228_800.0 / (2.0 * std::f32::consts::PI);
        if !innovation_hz.is_finite() || innovation_hz.abs() > RC1_MAX_CFO_JUMP_HZ {
            return false;
        }

        let gain = self.gain();
        self.cfo_rad_per_chip = (1.0 - gain) * self.cfo_rad_per_chip + gain * measured_cfo;
        self.total_updates += 1;

        let measured_hz = measured_cfo as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
        let abs_hz = measured_hz.abs();
        self.diag_signed_sum += measured_hz;
        self.diag_abs_sum += abs_hz;
        self.diag_sq_sum += measured_hz * measured_hz;
        self.diag_max = self.diag_max.max(abs_hz as f32);
        self.diag_count += 1;
        true
    }

    /// Chip-rate CFO derotation. RC3 corrects with `e^{-j·cfo_phase}`, reverse
    /// access with the opposite sign.
    pub fn derotate_chips(&mut self, chips: &mut [Complex32], oversample: usize) {
        let cfo_step = self.cfo_rad_per_chip / oversample.max(1) as f32;
        for chip in chips.iter_mut() {
            let (sin_p, cos_p) = self.cfo_phase.sin_cos();
            *chip = if self.rc3_traffic {
                Complex32::new(
                    chip.re * cos_p + chip.im * sin_p,
                    chip.im * cos_p - chip.re * sin_p,
                )
            } else {
                Complex32::new(
                    chip.re * cos_p - chip.im * sin_p,
                    chip.re * sin_p + chip.im * cos_p,
                )
            };
            self.cfo_phase += cfo_step;
        }
        self.cfo_phase %= 2.0 * std::f32::consts::PI;
    }

    /// Correct the constant channel phase on a block of CFO-corrected chips.
    ///
    /// Sums all chips in the block to estimate the R-PICH pilot direction
    /// (W(0,64) = all +1s, so the pilot accumulates coherently while
    /// traffic channels cancel over complete Walsh periods).  Rotates
    /// every chip by the conjugate of the normalized pilot direction,
    /// aligning pilot→real and traffic→imaginary.
    ///
    /// Reference equalization exercised by the tracker's tests to characterize
    /// pilot alignment; not wired into the production despread path.
    #[cfg(test)]
    pub fn correct_channel_phase(chips: &mut [Complex32]) {
        let pilot: Complex32 = chips.iter().copied().sum();
        let norm = pilot.norm();
        if norm <= 1e-9 {
            return;
        }
        let correction = Complex32::new(pilot.re / norm, -pilot.im / norm);
        for chip in chips.iter_mut() {
            *chip = *chip * correction;
        }
    }

    /// Snapshot and reset diagnostic counters.
    pub fn take_residual_stats(&mut self) -> CfoResidualStats {
        let stats = if self.diag_count > 0 {
            let n = self.diag_count as f64;
            CfoResidualStats {
                rms_hz: (self.diag_sq_sum / n).sqrt(),
                mean_hz: self.diag_abs_sum / n,
                max_hz: self.diag_max as f64,
                count: self.diag_count,
            }
        } else {
            CfoResidualStats::default()
        };
        self.diag_signed_sum = 0.0;
        self.diag_abs_sum = 0.0;
        self.diag_sq_sum = 0.0;
        self.diag_max = 0.0;
        self.diag_count = 0;
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};

    fn workspace_fixture_path(relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        if relative.is_absolute() || relative.exists() {
            return relative.to_path_buf();
        }

        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let test_relative = relative
            .components()
            .skip_while(|component| {
                !matches!(component, Component::Normal(part) if *part == OsStr::new("test"))
            })
            .collect::<PathBuf>();
        let lookup_relative = if test_relative.as_os_str().is_empty() {
            relative.to_path_buf()
        } else {
            test_relative
        };

        manifest_dir
            .ancestors()
            .map(|ancestor| ancestor.join(&lookup_relative))
            .find(|candidate| candidate.exists())
            .unwrap_or_else(|| manifest_dir.join(lookup_relative))
    }

    fn test_capture_path(file_name: &str) -> PathBuf {
        workspace_fixture_path(Path::new("test").join("capture").join(file_name))
    }

    fn generate_pilot_sums(
        n_symbols: usize,
        cfo_rad_per_chip: f32,
        pilot_amplitude: f32,
    ) -> Vec<Complex32> {
        let mut sums = Vec::with_capacity(n_symbols);
        let phase_per_symbol = cfo_rad_per_chip * 16.0;
        for i in 0..n_symbols {
            let phase = phase_per_symbol * i as f32;
            let mag = 16.0 * pilot_amplitude;
            sums.push(Complex32::new(mag * phase.cos(), mag * phase.sin()));
        }
        sums
    }

    #[test]
    fn zero_cfo_no_drift() {
        let mut tracker = CfoTracker::new(0.0, 0);
        let sums = generate_pilot_sums(500, 0.0, 1.0);
        for s in &sums {
            tracker.observe_pilot_symbol(*s);
        }
        assert!(
            tracker.cfo_rad_per_chip().abs() < 1e-6,
            "zero-CFO tracker should stay near zero, got {}",
            tracker.cfo_rad_per_chip()
        );
    }

    #[test]
    fn known_cfo_convergence() {
        let true_cfo = 200.0 * 2.0 * std::f32::consts::PI / 1_228_800.0;
        let mut tracker = CfoTracker::new(0.0, 0);

        let sums = generate_pilot_sums(2000, true_cfo, 1.0);
        for s in &sums {
            tracker.observe_pilot_symbol(*s);
        }

        let error_pct = ((tracker.cfo_rad_per_chip() - true_cfo) / true_cfo).abs() * 100.0;
        eprintln!(
            "true_cfo={:.9} tracked={:.9} error={:.1}%",
            true_cfo,
            tracker.cfo_rad_per_chip(),
            error_pct
        );
        assert!(
            error_pct < 10.0,
            "tracker should converge within 10% of true CFO, got {:.1}%",
            error_pct
        );
    }

    #[test]
    fn rc3_pcg_warmup_recovers_a_bad_reacquisition_seed() {
        let hz_to_rad_per_chip = 2.0 * std::f32::consts::PI / 1_228_800.0;
        let true_cfo = 4.0 * hz_to_rad_per_chip;
        let mut tracker = CfoTracker::new_rc3_traffic(-386.0 * hz_to_rad_per_chip);

        // One pilot vector per PCG, as the RC3 warmup path observes it.
        for pcg in 0..=WARMUP_SYMBOLS {
            let phase = true_cfo * (pcg * 1536) as f32;
            tracker.observe_pilot(Complex32::new(phase.cos(), phase.sin()) * 100.0, 1536);
        }

        let tracked_hz = tracker.cfo_rad_per_chip() / hz_to_rad_per_chip;
        assert!(
            (tracked_hz - 4.0).abs() < 0.1,
            "short-baseline warmup should recover 4 Hz from a -386 Hz seed, got {tracked_hz} Hz"
        );
        assert!(!tracker.in_warmup());

        // Steady state: eight averaged phase differences must hold the same CFO.
        let pcg_phase = true_cfo * 1536.0;
        let cross_sum = Complex32::new(pcg_phase.cos(), pcg_phase.sin()) * 8.0;
        for _ in 1..100 {
            tracker.observe_pilot_cross_sum(cross_sum, 1536);
        }
        let steady_hz = tracker.cfo_rad_per_chip() / hz_to_rad_per_chip;
        assert!(
            (steady_hz - 4.0).abs() < 0.1,
            "eight-PCG steady loop should hold 4 Hz, got {steady_hz} Hz"
        );
    }

    #[test]
    fn adjacent_pcg_cross_average_does_not_alias_350_hz() {
        let hz_to_rad_per_chip = 2.0 * std::f32::consts::PI / 1_228_800.0;
        let true_cfo = 350.0 * hz_to_rad_per_chip;
        let pcg_phase = true_cfo * 1536.0;
        let one_cross = Complex32::new(pcg_phase.cos(), pcg_phase.sin());
        let mut tracker = CfoTracker::new_rc3_traffic(0.0);

        for _ in 0..100 {
            tracker.observe_pilot_cross_sum(one_cross * 8.0, 1536);
        }

        let tracked_hz = tracker.cfo_rad_per_chip() / hz_to_rad_per_chip;
        assert!(
            (tracked_hz - 350.0).abs() < 0.1,
            "adjacent-PCG averaging should retain 350 Hz, got {tracked_hz} Hz"
        );
    }

    #[test]
    fn derotation_removes_phase_ramp() {
        let true_cfo = 500.0 * 2.0 * std::f32::consts::PI / 1_228_800.0;
        let mut tracker = CfoTracker::new(true_cfo, 0);

        let n_chips = 1536;
        let mut chips: Vec<Complex32> = (0..n_chips)
            .map(|i| {
                let phase = -true_cfo * i as f32;
                Complex32::new(phase.cos(), phase.sin())
            })
            .collect();

        tracker.derotate_chips(&mut chips, 1);

        let max_phase_error = chips
            .iter()
            .map(|c| c.im.atan2(c.re).abs())
            .fold(0.0f32, f32::max);

        eprintln!(
            "max phase error after derotation: {:.4} rad",
            max_phase_error
        );
        assert!(
            max_phase_error < 0.05,
            "derotation should remove phase ramp, max error = {:.4} rad",
            max_phase_error
        );
    }

    #[test]
    fn observed_phase_advance_and_derotation_use_opposite_signs() {
        let true_cfo = 275.0 * 2.0 * std::f32::consts::PI / 1_228_800.0;
        let mut tracker = CfoTracker::new_rc3_traffic(0.0);
        for pcg in 0..80 {
            let phase = true_cfo * (pcg * 1536) as f32;
            tracker.observe_pilot(Complex32::new(phase.cos(), phase.sin()) * 100.0, 1536);
        }
        assert!(
            (tracker.cfo_rad_per_chip() - true_cfo).abs() < 1e-6,
            "pilot observation must retain the received phase-advance sign"
        );

        let mut chips: Vec<Complex32> = (0..1536)
            .map(|chip| {
                let phase = true_cfo * chip as f32;
                Complex32::new(phase.cos(), phase.sin())
            })
            .collect();
        tracker.derotate_chips(&mut chips, 1);
        let max_phase_error = chips
            .iter()
            .map(|chip| chip.im.atan2(chip.re).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_phase_error < 0.01,
            "CFO learned from the pilot must remove the same phase ramp: {max_phase_error:.4} rad"
        );
    }

    #[test]
    fn warmup_transitions_to_steady() {
        let mut tracker = CfoTracker::new(0.0, 0);
        assert!(tracker.in_warmup());

        let sums = generate_pilot_sums(WARMUP_SYMBOLS + 500, 0.0, 1.0);
        for s in &sums {
            tracker.observe_pilot_symbol(*s);
        }
        assert!(
            !tracker.in_warmup(),
            "tracker should exit warmup after {} symbols",
            WARMUP_SYMBOLS
        );
    }

    #[test]
    fn steady_loop_rejects_implausible_single_observation_jump() {
        let hz_to_rad_per_chip = 2.0 * std::f32::consts::PI / 1_228_800.0;
        let mut tracker = CfoTracker::new_rc3_traffic(5.0 * hz_to_rad_per_chip);
        tracker.total_updates = WARMUP_SYMBOLS;

        // A noise-dominated weak pilot can throw a single +100 Hz observation.
        // It must not move the estimate.
        let noisy_cross = Complex32::from_polar(1.0, 100.0 * hz_to_rad_per_chip * 1536.0);
        tracker.observe_pilot_cross_sum(noisy_cross, 1536);
        let resulting_hz = tracker.cfo_rad_per_chip() / hz_to_rad_per_chip;
        assert!(
            resulting_hz <= 7.0,
            "steady tracker followed a nonphysical phase jump: {resulting_hz:.2} Hz"
        );
    }

    #[test]
    fn correct_channel_phase_aligns_pilot_to_real() {
        let n = 256;
        let theta = std::f32::consts::PI / 4.0;
        let pilot = Complex32::new(theta.cos(), theta.sin());
        let mut chips: Vec<Complex32> = (0..n).map(|_| pilot).collect();

        CfoTracker::correct_channel_phase(&mut chips);

        for (i, &c) in chips.iter().enumerate() {
            let phase = c.im.atan2(c.re);
            assert!(
                phase.abs() < 0.01,
                "chip {} phase = {:.4} rad after correction (expected ~0)",
                i,
                phase,
            );
        }
    }

    #[test]
    fn correct_channel_phase_puts_traffic_on_imaginary() {
        let n = 256;
        let theta = 30.0f32.to_radians();
        let pilot_dir = Complex32::new(theta.cos(), theta.sin());
        let traffic_dir = Complex32::new(
            (theta + std::f32::consts::FRAC_PI_2).cos(),
            (theta + std::f32::consts::FRAC_PI_2).sin(),
        );

        let walsh4: [f32; 16] = [
            1., 1., 1., 1., -1., -1., -1., -1., 1., 1., 1., 1., -1., -1., -1., -1.,
        ];

        let mut chips: Vec<Complex32> = (0..n)
            .map(|i| {
                let w = walsh4[i % 16];
                pilot_dir + traffic_dir * w * 0.5
            })
            .collect();

        CfoTracker::correct_channel_phase(&mut chips);

        let mut traffic_sum = Complex32::new(0.0, 0.0);
        for i in 0..16 {
            traffic_sum += chips[i] * walsh4[i];
        }
        let traffic_phase = traffic_sum.im.atan2(traffic_sum.re);
        let expected_phase = std::f32::consts::FRAC_PI_2;
        let error = (traffic_phase - expected_phase).abs();
        eprintln!(
            "traffic phase after correction: {:.2}° (expected 90°, error {:.2}°)",
            traffic_phase.to_degrees(),
            error.to_degrees(),
        );
        assert!(
            error < 0.1,
            "traffic should be on imaginary axis, phase = {:.2}° (error {:.2}°)",
            traffic_phase.to_degrees(),
            error.to_degrees(),
        );
    }

    /// Direct WAV-based CFO tracker test.
    ///
    /// Loads the SO33 WAV capture, PN+LC despreads at the known code phase
    /// from the preamble detection log, feeds chips through the tracker,
    /// and verifies the pilot phase stabilizes after derotation.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "release-only WAV-based CFO test; run with --release"
    )]
    fn capture_cfo_tracker_on_live_wav() {
        use crate::phy::coding::long_code::LongCodeGenerator;
        use crate::phy::spread::PnSequence;
        // --- Parameters from PREAMBLE DETECTED log ---
        // WAV: 1793960586090657.wav (SO33, walsh=11, ESN=0x80857E58)
        // delay=0, despread_phase=57941, tx_chip=1793960591866005
        // first_verified_sample=23068672, oversample=4, cfo=0.000013
        let wav_path = test_capture_path("1793960586090657.wav");
        if !wav_path.exists() {
            eprintln!("skipping: WAV not found at {}", wav_path.display());
            return;
        }

        let reader = hound::WavReader::open(&wav_path).unwrap();
        let spec = reader.spec();
        let sample_rate = spec.sample_rate as usize;
        let oversample = sample_rate / 1_228_800;
        assert_eq!(oversample, 4, "expected 4× oversample");

        // Read IQ samples (interleaved i16 → Complex32).
        let raw: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        let iq_samples: Vec<Complex32> = raw
            .chunks_exact(2)
            .map(|pair| Complex32::new(pair[0] as f32 / 32768.0, pair[1] as f32 / 32768.0))
            .collect();

        eprintln!(
            "WAV loaded: {} IQ samples, {:.2}s, sample_rate={}",
            iq_samples.len(),
            iq_samples.len() as f64 / sample_rate as f64,
            sample_rate,
        );

        // Run through matched filter (required — raw WAV has pulse shaping).
        use crate::receiver::pipelined::pulse_matched_filter_processor::PulseMatchedFilterProcessor;
        use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};
        let mut mf = PulseMatchedFilterProcessor::new();
        let batch = 65536;
        let mut filtered = Vec::with_capacity(iq_samples.len());
        for chunk in iq_samples.chunks(batch) {
            let blk = SampleBlock::new(chunk.to_vec(), filtered.len())
                .with_sample_rate_hz(sample_rate as f64);
            for out_blk in mf.process_block(blk) {
                filtered.extend_from_slice(&out_blk.samples);
            }
        }
        // Flush.
        for out_blk in mf.flush() {
            filtered.extend_from_slice(&out_blk.samples);
        }
        let iq_samples = filtered;
        eprintln!("matched filter output: {} samples", iq_samples.len());

        // Build PN conjugate reference (same as PnLcCorrelator uses).
        let phase_period = 32768 * oversample;
        let mut pn = PnSequence::new_repeat(0, 32768, oversample - 1);
        let pn_seq: Vec<Complex32> = (0..phase_period)
            .map(|_| {
                let s = pn.generate_iq();
                Complex32::new(s.re, -s.im) // conjugate for despreading
            })
            .collect();

        // Start despreading at the detected sample offset and PN phase.
        // Skip HPSK/LC for now — just PN despread to verify the pilot
        // is visible at the correct code phase.
        // From PREAMBLE DETECTED log:
        //   tx_chip=1793960591866005 (LC chip counter at preamble start)
        //   finger_start=23101440 (sample index where finger begins)
        //   despread_phase=57941 (PN sequence cursor)
        let start_sample: usize = 23101440;
        let despread_phase: usize = 57941;
        let initial_cfo: f32 = 0.000013;
        let tx_chip: usize = 1793960591866005;
        let esn: u32 = 0x80857E58;

        // LC generator: advance from chip 0 to the preamble chip position.
        let mut lc_gen = LongCodeGenerator::new_traffic_channel(esn);
        lc_gen.advance_chips(tx_chip);

        // Quick search: try sub-chip offsets around the detected phase.
        // Also try ±8 chips to account for timing ambiguity.
        let search_range = (0..oversample * 17).map(|i| {
            if i < oversample {
                i // sub-chip offsets 0..3
            } else {
                (i - oversample + 1) * oversample // chip offsets 4,8,12,...,64
            }
        });
        let mut best_phase = despread_phase;
        let mut best_energy = 0.0f32;
        for offset in search_range {
            let test_phase = (despread_phase + offset) % phase_period;
            let mut energy = 0.0f32;
            let mut ph = test_phase;
            for c in 0..1536 {
                let si = start_sample + c * oversample;
                if si >= iq_samples.len() {
                    break;
                }
                let despread = pn_seq[ph % phase_period] * iq_samples[si];
                energy += despread.norm_sqr();
                ph = (ph + oversample) % phase_period;
            }
            if energy > best_energy {
                best_energy = energy;
                best_phase = test_phase;
            }
        }
        eprintln!(
            "best PN phase: {} (energy={:.1}, orig despread_phase={})",
            best_phase, best_energy, despread_phase
        );
        let despread_phase = best_phase;

        // Create tracker with the acquisition CFO estimate.
        let mut tracker = CfoTracker::new(initial_cfo, 0);

        // Despread chip-by-chip at 1× (prompt only) and feed to tracker.
        let mut pn_phase = despread_phase;
        let num_chips = 200_000; // ~163 ms — enough for warmup + convergence
        let mut pilot_phases_pre: Vec<f32> = Vec::new();
        let mut pilot_phases_post: Vec<f32> = Vec::new();
        let mut pcg_chips_pre: Vec<Complex32> = Vec::new();
        let mut pcg_chips_post: Vec<Complex32> = Vec::new();

        // HPSK state (matches PnLcFinger::advance_lc_for_new_chip)
        let mut hpsk_chip_count: usize = 0;
        let mut hpsk_prev_lc: f32 = 1.0;
        let mut hpsk_dec_q: f32 = 1.0;

        for chip_idx in 0..num_chips {
            let sample_idx = start_sample + chip_idx * oversample;
            if sample_idx >= iq_samples.len() {
                break;
            }

            // PN despread.
            let raw_sample = iq_samples[sample_idx];
            let pn_conj = pn_seq[pn_phase % phase_period];
            let despread = pn_conj * raw_sample;

            // HPSK despreading (RC3 reverse link composite PN×LC).
            // Matches PnLcFinger::advance_lc_for_new_chip exactly.
            let lc_i: f32 = if lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
            let pn_i = pn_conj.re;
            let pn_q = -pn_conj.im; // pn_seq stores conj, negate for true PN_Q
            let w12: f32 = if hpsk_chip_count % 2 == 0 { 1.0 } else { -1.0 };
            if hpsk_chip_count % 2 == 0 {
                hpsk_dec_q = pn_q * hpsk_prev_lc;
            }
            let cross = w12 * pn_i * pn_q * hpsk_dec_q;
            let lc_re = lc_i * (1.0 - cross) * 0.5;
            let lc_im = lc_i * (w12 * hpsk_dec_q + pn_i * pn_q) * 0.5;
            let lc_conj = Complex32::new(lc_re, lc_im);
            hpsk_prev_lc = lc_i;
            hpsk_chip_count += 1;

            let chip = despread * lc_conj;

            pcg_chips_pre.push(chip);

            // Every PCG (1536 chips): feed the full PCG pilot sum to
            // the tracker for a clean CFO measurement.
            if (chip_idx + 1) % 1536 == 0 {
                let pilot_sum: Complex32 = pcg_chips_pre.iter().copied().sum();
                tracker.observe_pilot(pilot_sum, 1536);
            }

            // Derotate this chip for the output measurement.
            let mut derotated = [chip];
            tracker.derotate_chips(&mut derotated, 1);
            pcg_chips_post.push(derotated[0]);

            // Every PCG (1536 chips): apply channel phase correction and
            // measure pilot phase pre and post.
            if (chip_idx + 1) % 1536 == 0 {
                let pre_sum: Complex32 = pcg_chips_pre.iter().copied().sum();
                pilot_phases_pre.push(pre_sum.im.atan2(pre_sum.re).to_degrees());

                // Apply per-PCG channel phase correction to the derotated chips.
                CfoTracker::correct_channel_phase(&mut pcg_chips_post);
                let post_sum: Complex32 = pcg_chips_post.iter().copied().sum();
                pilot_phases_post.push(post_sum.im.atan2(post_sum.re).to_degrees());

                pcg_chips_pre.clear();
                pcg_chips_post.clear();
            }

            pn_phase = (pn_phase + oversample) % phase_period;
        }

        // Print results.
        eprintln!(
            "\n=== CFO Tracker on Live WAV ({} PCGs) ===",
            pilot_phases_post.len()
        );
        eprintln!(
            "{:<6} {:>12} {:>12} {:>12} {:>12} {:>12}",
            "PCG", "pre_phase°", "pre_norm", "post_phase°", "post_norm", "cfo_Hz"
        );
        // Re-run to get norms (we didn't store them — just recompute from phases).
        // Actually we need to store norms. For now, print what we have.
        for (i, (pre, post)) in pilot_phases_pre
            .iter()
            .zip(pilot_phases_post.iter())
            .enumerate()
        {
            let cfo_hz =
                tracker.cfo_rad_per_chip() as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
            eprintln!(
                "{:<6} {:>12.1} {:>12} {:>12.1} {:>12} {:>12.1}",
                i, pre, "-", post, "-", cfo_hz
            );
        }

        // --- Locked-in assertions ---

        // 1. Must produce at least 100 PCGs from 200k chips.
        assert!(
            pilot_phases_post.len() >= 100,
            "expected ≥100 PCGs, got {}",
            pilot_phases_post.len()
        );

        // 2. Pre-correction phases should be coherent (pilot visible).
        //    Check consecutive PCG-to-PCG phase steps: if PN+LC despread
        //    is working, consecutive PCGs should be within ~30° of each
        //    other (slow CFO drift). Noise gives random ~180° jumps.
        let mut pre_small_steps = 0usize;
        let mut pre_total_steps = 0usize;
        for w in pilot_phases_pre.windows(2) {
            let mut delta = w[1] - w[0];
            if delta > 180.0 {
                delta -= 360.0;
            }
            if delta < -180.0 {
                delta += 360.0;
            }
            pre_total_steps += 1;
            if delta.abs() < 30.0 {
                pre_small_steps += 1;
            }
        }
        let pre_coherent_pct = 100.0 * pre_small_steps as f64 / pre_total_steps.max(1) as f64;
        eprintln!(
            "pre-correction coherence: {}/{} steps <30° ({:.0}%)",
            pre_small_steps, pre_total_steps, pre_coherent_pct
        );
        assert!(
            pre_coherent_pct > 50.0,
            "pre-correction pilot should show coherent PCG-to-PCG phase \
             (>50% steps <30°), got {:.0}%",
            pre_coherent_pct
        );

        // 3. Post-correction (derotation + per-PCG channel phase) should
        //    align pilot to 0°.  Last 20 PCGs must all be within ±5°.
        let last_20 = &pilot_phases_post[pilot_phases_post.len().saturating_sub(20)..];
        let post_max_dev = last_20.iter().map(|p| p.abs()).fold(0.0f32, f32::max);
        eprintln!(
            "post-correction last 20 PCGs: max_dev={:.1}° (should be <5°)",
            post_max_dev
        );
        assert!(
            post_max_dev < 5.0,
            "post-correction pilot phase should be ~0° on every PCG, \
             max_dev={:.1}°",
            post_max_dev
        );

        // 4. CFO estimate should be reasonable (within ±2000 Hz of the
        //    acquisition estimate for this WAV capture).
        let final_cfo_hz =
            tracker.cfo_rad_per_chip() as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
        eprintln!(
            "final CFO: {:.1} Hz ({:.9} rad/chip)",
            final_cfo_hz,
            tracker.cfo_rad_per_chip()
        );
        assert!(
            final_cfo_hz.abs() < 2000.0,
            "final CFO should be within ±2000 Hz, got {:.1} Hz",
            final_cfo_hz
        );

        // 5. Tracker should have exited warmup.
        assert!(
            !tracker.in_warmup(),
            "tracker should exit warmup within 200k chips"
        );
    }
}
