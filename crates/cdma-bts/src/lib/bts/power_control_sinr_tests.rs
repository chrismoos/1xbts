//! Characterization tests for the per-PCG pilot-symbol SINR metric:
//! linearity vs Tx amplitude, response to interference, fading, and CFO.

use num_complex::Complex32;

use crate::phy::walsh::WalshGenerator;
use crate::receiver::pipelined::{PipelineProcessor, Rc3BpskDespread, SampleBlock};

const WALSH_LENGTH: usize = 16;
const SYMBOLS_PER_PCG: usize = 96;
const PILOT_SYMBOLS_PER_PCG: usize = 72;
const CHIPS_PER_PCG: usize = WALSH_LENGTH * SYMBOLS_PER_PCG; // 1536
const PILOT_CHIPS_PER_PCG: usize = WALSH_LENGTH * PILOT_SYMBOLS_PER_PCG; // 1152

#[derive(Clone)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(6364136223846793005).wrapping_add(1),
        }
    }
    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }
    fn next_f32_uniform(&mut self) -> f32 {
        // Avoid 0.0 so the log in Box-Muller is finite.
        ((self.next_u32() | 1) as f32) / (u32::MAX as f32)
    }
    /// One pair of standard-normal samples, mean 0, variance 1.
    fn next_gaussian_pair(&mut self) -> (f32, f32) {
        let u1 = self.next_f32_uniform();
        let u2 = self.next_f32_uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = std::f32::consts::TAU * u2;
        (r * theta.cos(), r * theta.sin())
    }
}

/// Per-PCG pilot symbol SINR (dB) from despreader accumulators
/// `pilot_norm_sq` (coherent) and `pilot_sym_power_sum` (incoherent).
fn pilot_sym_sinr_db(pilot_norm_sq: f32, pilot_sym_power_sum: f32) -> f32 {
    let n = PILOT_SYMBOLS_PER_PCG as f32;
    let mean_sq = pilot_norm_sq / (n * n);
    let sample_var = (pilot_sym_power_sum / n - mean_sq).max(1e-12);
    let sinr_lin = (mean_sq / sample_var).max(1e-12);
    10.0 * sinr_lin.log10()
}

/// Legacy pilot Ec/Io (dB) — saturates with Tx power; kept for comparison.
fn pilot_ec_io_db_legacy(pilot_norm_sq: f32, chip_power_sum: f32) -> f32 {
    if chip_power_sum > 1e-12 {
        let n = PILOT_SYMBOLS_PER_PCG as f32;
        let n_chips = PILOT_CHIPS_PER_PCG as f32;
        let ec_io = pilot_norm_sq * n_chips / (n * n * chip_power_sum);
        10.0 * ec_io.max(1e-12).log10()
    } else {
        40.0
    }
}

/// Analytical predictor for [`pilot_sym_sinr_db`] under the test signal model.
fn predicted_pilot_sym_sinr_db(
    target_amp: f32,
    interferer_amps: &[f32],
    sigma_n_per_axis: f32,
) -> f32 {
    let denom = sigma_n_per_axis * sigma_n_per_axis
        + interferer_amps.iter().map(|a| a * a / 2.0).sum::<f32>();
    let sinr_lin = 8.0 * target_amp * target_amp / denom.max(1e-30);
    10.0 * sinr_lin.max(1e-12).log10()
}

#[derive(Clone, Copy)]
struct TargetSpec {
    amplitude: f32,
    /// If false, send pilot only.
    include_traffic: bool,
}

impl TargetSpec {
    fn pilot_only(amplitude: f32) -> Self {
        Self {
            amplitude,
            include_traffic: false,
        }
    }
}

/// Build `pcg_count` PCGs (1536 chips each) of post-despread target
/// chips, optionally with random ±1 traffic on Walsh-4.
fn synth_target_chips(spec: TargetSpec, pcg_count: usize, traffic_seed: u64) -> Vec<Complex32> {
    let walsh4 = WalshGenerator::generate_matrix::<WALSH_LENGTH>()[4];
    let total_chips = pcg_count * CHIPS_PER_PCG;
    let mut chips = Vec::with_capacity(total_chips);
    let mut rng = Rng::new(traffic_seed);
    let symbols_total = total_chips / WALSH_LENGTH;
    let traffic_syms: Vec<f32> = (0..symbols_total)
        .map(|_| {
            if spec.include_traffic {
                if rng.next_u32() & 1 == 0 { 1.0 } else { -1.0 }
            } else {
                0.0
            }
        })
        .collect();
    for chip_idx in 0..total_chips {
        let sym_idx = chip_idx / WALSH_LENGTH;
        let walsh_chip = walsh4[chip_idx % WALSH_LENGTH] as f32;
        let traffic = -spec.amplitude * traffic_syms[sym_idx] * walsh_chip;
        chips.push(Complex32::new(spec.amplitude, traffic));
    }
    chips
}

fn add_thermal_awgn(chips: &mut [Complex32], sigma_per_axis: f32, seed: u64) {
    if sigma_per_axis <= 0.0 {
        return;
    }
    let mut rng = Rng::new(seed);
    for chip in chips.iter_mut() {
        let (n_re, n_im) = rng.next_gaussian_pair();
        chip.re += n_re * sigma_per_axis;
        chip.im += n_im * sigma_per_axis;
    }
}

/// Complex Gaussian interferer with per-axis std = amplitude/√2.
/// Models per-chip variance after target-PN despread; phase is uniform.
fn add_interferer(chips: &mut [Complex32], amplitude: f32, seed: u64) {
    if amplitude <= 0.0 {
        return;
    }
    let std_per_axis = amplitude / std::f32::consts::SQRT_2;
    let mut rng = Rng::new(seed);
    for chip in chips.iter_mut() {
        let (n_re, n_im) = rng.next_gaussian_pair();
        chip.re += n_re * std_per_axis;
        chip.im += n_im * std_per_axis;
    }
}

fn apply_static_cfo(chips: &mut [Complex32], cfo_hz: f32) {
    if cfo_hz == 0.0 {
        return;
    }
    let chip_rate = 1_228_800.0_f32;
    let omega = std::f32::consts::TAU * cfo_hz / chip_rate;
    for (k, chip) in chips.iter_mut().enumerate() {
        let theta = omega * k as f32;
        let (s, c) = theta.sin_cos();
        let rotated = Complex32::new(chip.re * c - chip.im * s, chip.re * s + chip.im * c);
        *chip = rotated;
    }
}

/// Coarse Rician fading proxy with LOS + slow AR(1) scatter at ~`doppler_hz`.
/// Not a true Jakes model.
fn apply_rician_fading(chips: &mut [Complex32], k_factor_db: f32, doppler_hz: f32, seed: u64) {
    let k_lin = 10f32.powf(k_factor_db / 10.0);
    let los_gain = (k_lin / (k_lin + 1.0)).sqrt();
    let scatter_gain = (1.0 / (k_lin + 1.0)).sqrt();
    let chip_rate = 1_228_800.0_f32;
    // AR(1) coefficient tuned so the autocorrelation envelope decays
    // at ~doppler_hz: α = exp(-2π f_d / f_chip).
    let alpha = (-std::f32::consts::TAU * doppler_hz / chip_rate).exp();
    let drive_std = (1.0 - alpha * alpha).sqrt() / std::f32::consts::SQRT_2;
    let mut rng = Rng::new(seed);
    let (mut h_re, mut h_im) = rng.next_gaussian_pair();
    h_re /= std::f32::consts::SQRT_2;
    h_im /= std::f32::consts::SQRT_2;
    for chip in chips.iter_mut() {
        let (d_re, d_im) = rng.next_gaussian_pair();
        h_re = alpha * h_re + drive_std * d_re;
        h_im = alpha * h_im + drive_std * d_im;
        let g_re = los_gain + scatter_gain * h_re;
        let g_im = scatter_gain * h_im;
        let new_re = chip.re * g_re - chip.im * g_im;
        let new_im = chip.re * g_im + chip.im * g_re;
        chip.re = new_re;
        chip.im = new_im;
    }
}

#[derive(Debug, Clone, Copy)]
struct PcgMetric {
    legacy_ec_io_db: f32,
    sinr_db: f32,
}

fn run_despreader(chips: Vec<Complex32>) -> Vec<PcgMetric> {
    let pcg_count = chips.len() / CHIPS_PER_PCG;
    let mut despreader = Rc3BpskDespread::with_output_symbols(SYMBOLS_PER_PCG);
    let block = SampleBlock::new(chips, 0).with_sample_rate_hz(1_228_800.0);
    let outputs = despreader.process_block(block);
    let mut metrics = Vec::with_capacity(pcg_count);
    for blk in outputs {
        let Some(per_pcg) = blk.pcg_pilot_metrics else {
            continue;
        };
        for (pilot_norm_sq, pilot_sym_power_sum, _traffic_power_sum, chip_power_sum) in per_pcg {
            metrics.push(PcgMetric {
                legacy_ec_io_db: pilot_ec_io_db_legacy(pilot_norm_sq, chip_power_sum),
                sinr_db: pilot_sym_sinr_db(pilot_norm_sq, pilot_sym_power_sum),
            });
        }
    }
    assert!(
        !metrics.is_empty(),
        "despreader emitted no PCG metrics for {} chips ({} PCGs)",
        pcg_count * CHIPS_PER_PCG,
        pcg_count
    );
    metrics
}

fn mean(xs: &[f32]) -> f32 {
    xs.iter().copied().sum::<f32>() / xs.len() as f32
}

fn std_dev(xs: &[f32]) -> f32 {
    let m = mean(xs);
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f32>() / xs.len() as f32;
    var.sqrt()
}

fn percentile(sorted: &[f32], pct: f32) -> f32 {
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f32).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ---------------------------------------------------------------------------
// Scenario tests
// ---------------------------------------------------------------------------

const PCG_COUNT: usize = 64;
const SIGMA_N: f32 = 0.05;

/// Per-axis thermal noise std that puts a unit-amplitude target at
/// roughly +10 dB pilot SINR. Used by fading / CFO tests that need a
/// realistic operating point (the linearity sweep deliberately runs at
/// lab-grade SNR to expose the legacy Ec/Io saturation).
const REALISTIC_SIGMA_N: f32 = 0.9; // 10·log10(8 / (2·0.81)) ≈ 6.9 dB SINR

/// 1. Linearity sweep: single MS in AWGN, Tx amplitude swept across
///    30 dB. The new metric must track Tx 1:1; the legacy Ec/Io must
///    saturate at high Tx (≪ 1 dB/dB slope above +5 dB).
#[test]
fn scenario_01_linearity_single_ms_awgn() {
    let amps_db: [f32; 7] = [-15.0, -10.0, -5.0, 0.0, 5.0, 10.0, 15.0];
    let mut sinr_results = Vec::new();
    let mut legacy_results = Vec::new();
    for (i, db) in amps_db.iter().enumerate() {
        let amp = 10f32.powf(db / 20.0);
        let mut chips = synth_target_chips(TargetSpec::pilot_only(amp), PCG_COUNT, 1);
        add_thermal_awgn(&mut chips, SIGMA_N, 1000 + i as u64);
        let metrics = run_despreader(chips);
        let sinr_mean = mean(&metrics.iter().map(|m| m.sinr_db).collect::<Vec<_>>());
        let legacy_mean = mean(
            &metrics
                .iter()
                .map(|m| m.legacy_ec_io_db)
                .collect::<Vec<_>>(),
        );
        let predicted = predicted_pilot_sym_sinr_db(amp, &[], SIGMA_N);
        eprintln!(
            "  amp={:+5.1} dB  measured_sinr={:+6.2} dB  predicted={:+6.2} dB  legacy_ec_io={:+6.2} dB",
            db, sinr_mean, predicted, legacy_mean,
        );
        assert!(
            (sinr_mean - predicted).abs() < 0.6,
            "SINR at amp {} dB: measured {:.2} vs predicted {:.2}",
            db,
            sinr_mean,
            predicted,
        );
        sinr_results.push((*db, sinr_mean));
        legacy_results.push((*db, legacy_mean));
    }
    let sinr_slope = (sinr_results.last().unwrap().1 - sinr_results.first().unwrap().1)
        / (amps_db.last().unwrap() - amps_db.first().unwrap());
    assert!(
        (sinr_slope - 1.0).abs() < 0.05,
        "pilot SINR slope vs Tx amplitude must be ~1.0 dB/dB, got {:.3}",
        sinr_slope,
    );
    let legacy_slope_high =
        (legacy_results[6].1 - legacy_results[4].1) / (legacy_results[6].0 - legacy_results[4].0);
    assert!(
        legacy_slope_high < 0.3,
        "legacy Ec/Io must saturate at high Tx (slope <0.3 dB/dB above +5 dB), got {:.3}",
        legacy_slope_high,
    );
}

/// 5–7. Multi-MS, equal power: SINR must drop in line with the
///      analytical predictor for 1, 2, 4 equal-power interferers added
///      on top of a thermal floor.
#[test]
fn scenario_05_07_multi_ms_equal_power_floor() {
    let target_amp = 1.0_f32;
    for &n_interferers in &[1usize, 2, 4] {
        let mut chips = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 2);
        add_thermal_awgn(&mut chips, SIGMA_N, 2000);
        let interferer_amps: Vec<f32> = vec![target_amp; n_interferers];
        for (i, amp) in interferer_amps.iter().enumerate() {
            add_interferer(&mut chips, *amp, 3000 + i as u64);
        }
        let metrics = run_despreader(chips);
        let sinr_mean = mean(&metrics.iter().map(|m| m.sinr_db).collect::<Vec<_>>());
        let predicted = predicted_pilot_sym_sinr_db(target_amp, &interferer_amps, SIGMA_N);
        eprintln!(
            "  n_interferers={}  measured_sinr={:+6.2} dB  predicted={:+6.2} dB",
            n_interferers, sinr_mean, predicted,
        );
        assert!(
            (sinr_mean - predicted).abs() < 0.7,
            "{} equal-power interferers: measured {:.2} dB vs predicted {:.2} dB",
            n_interferers,
            sinr_mean,
            predicted,
        );
    }
}

/// 8. Near-far stressed: target weaker than a single interferer by 3,
///    6, 10 dB. The estimator must still report a meaningfully lower
///    SINR than the equal-power case (loop should signal UP).
#[test]
fn scenario_08_near_far_target_weaker() {
    let interferer_amp = 1.0_f32;
    for &offset_db in &[-3.0_f32, -6.0, -10.0] {
        let target_amp = 10f32.powf(offset_db / 20.0);
        let mut chips = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 4);
        add_thermal_awgn(&mut chips, SIGMA_N, 4000);
        add_interferer(&mut chips, interferer_amp, 5000);
        let metrics = run_despreader(chips);
        let sinr_mean = mean(&metrics.iter().map(|m| m.sinr_db).collect::<Vec<_>>());
        let predicted = predicted_pilot_sym_sinr_db(target_amp, &[interferer_amp], SIGMA_N);
        eprintln!(
            "  target_offset={:+5.1} dB  measured_sinr={:+6.2} dB  predicted={:+6.2} dB",
            offset_db, sinr_mean, predicted,
        );
        assert!(
            (sinr_mean - predicted).abs() < 0.7,
            "target {} dB below interferer: measured {:.2} vs predicted {:.2}",
            offset_db,
            sinr_mean,
            predicted,
        );
    }
}

/// 9. Target stronger than a single interferer by 6, 10 dB — SINR
///    must approach the single-MS-only value as target dominates.
#[test]
fn scenario_09_near_far_target_stronger() {
    let interferer_amp = 1.0_f32;
    for &offset_db in &[6.0_f32, 10.0] {
        let target_amp = 10f32.powf(offset_db / 20.0);
        let mut chips = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 6);
        add_thermal_awgn(&mut chips, SIGMA_N, 6000);
        add_interferer(&mut chips, interferer_amp, 7000);
        let metrics = run_despreader(chips);
        let sinr_mean = mean(&metrics.iter().map(|m| m.sinr_db).collect::<Vec<_>>());
        let predicted_with = predicted_pilot_sym_sinr_db(target_amp, &[interferer_amp], SIGMA_N);
        let predicted_alone = predicted_pilot_sym_sinr_db(target_amp, &[], SIGMA_N);
        eprintln!(
            "  target_offset={:+5.1} dB  measured={:+6.2}  predicted_with_int={:+6.2}  predicted_alone={:+6.2}",
            offset_db, sinr_mean, predicted_with, predicted_alone,
        );
        assert!(
            (sinr_mean - predicted_with).abs() < 0.7,
            "stronger target {} dB: measured {:.2} vs predicted {:.2}",
            offset_db,
            sinr_mean,
            predicted_with,
        );
    }
}

/// 10. Cell loading sweep: 8 MS at random uniform powers in a 6 dB
///     window. Documents and asserts the percentile range that the
///     loop must operate over.
#[test]
fn scenario_10_cell_loading_8ms() {
    let target_amp = 1.0_f32;
    let mut rng = Rng::new(8001);
    let interferer_amps: Vec<f32> = (0..7)
        .map(|_| {
            let db = -3.0 + 6.0 * rng.next_f32_uniform(); // -3..+3 dB
            10f32.powf(db / 20.0)
        })
        .collect();
    let mut chips = synth_target_chips(TargetSpec::pilot_only(target_amp), 200, 8);
    add_thermal_awgn(&mut chips, SIGMA_N, 8100);
    for (i, amp) in interferer_amps.iter().enumerate() {
        add_interferer(&mut chips, *amp, 8200 + i as u64);
    }
    let metrics = run_despreader(chips);
    let mut sinrs: Vec<f32> = metrics.iter().map(|m| m.sinr_db).collect();
    sinrs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p5 = percentile(&sinrs, 5.0);
    let p50 = percentile(&sinrs, 50.0);
    let p95 = percentile(&sinrs, 95.0);
    let predicted = predicted_pilot_sym_sinr_db(target_amp, &interferer_amps, SIGMA_N);
    eprintln!(
        "  cell loading 8 MS  p5={:+6.2}  p50={:+6.2}  p95={:+6.2}  predicted_mean={:+6.2}",
        p5, p50, p95, predicted,
    );
    assert!(
        (p50 - predicted).abs() < 0.7,
        "median SINR {:.2} should be within 0.7 dB of analytical {:.2}",
        p50,
        predicted,
    );
    assert!(
        (p95 - p5).abs() < 3.0,
        "p5..p95 SINR spread is wider than expected: {:.2} dB",
        p95 - p5,
    );
}

/// 2–3. Multipath / Doppler — Rician with K=10 dB at realistic
///      operating SINR (~7 dB). Asserts the metric remains usable
///      (positive SINR, finite, std-dev grows with Doppler) and
///      documents the per-Doppler mean/std for Phase 2 setpoint
///      design. We deliberately do NOT assert a tight mean band: the
///      coherent pilot integration legitimately loses energy to the
///      scattered Rayleigh component (mean(g) = los_gain ≈ 0.95 at
///      K=10 dB), and that loss is part of the channel, not a metric
///      bug.
#[test]
fn scenario_02_03_rician_fading_doppler() {
    let target_amp = 1.0_f32;
    let static_metrics = {
        let mut chips = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 9);
        add_thermal_awgn(&mut chips, REALISTIC_SIGMA_N, 9100);
        run_despreader(chips)
    };
    let static_sinrs: Vec<f32> = static_metrics.iter().map(|m| m.sinr_db).collect();
    let static_mean = mean(&static_sinrs);
    let static_std = std_dev(&static_sinrs);
    eprintln!(
        "  static (no fading)   mean_sinr={:+6.2} dB  std={:.2} dB",
        static_mean, static_std,
    );

    let mut prev_std = static_std;
    for &doppler in &[5.0_f32, 80.0] {
        let mut chips = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 9);
        apply_rician_fading(&mut chips, 10.0, doppler, 9300 + doppler as u64);
        add_thermal_awgn(&mut chips, REALISTIC_SIGMA_N, 9400 + doppler as u64);
        let metrics = run_despreader(chips);
        let sinrs: Vec<f32> = metrics.iter().map(|m| m.sinr_db).collect();
        let m = mean(&sinrs);
        let s = std_dev(&sinrs);
        eprintln!(
            "  doppler={:>5.1} Hz   mean_sinr={:+6.2} dB  std={:.2} dB  Δmean={:+5.2} dB",
            doppler,
            m,
            s,
            m - static_mean,
        );
        assert!(
            m.is_finite() && s.is_finite(),
            "fading {} Hz produced non-finite SINR statistics",
            doppler,
        );
        // Mean SINR may drop a few dB under fading (LOS energy loss); it
        // must not collapse to a useless value.
        assert!(
            m > static_mean - 6.0,
            "fading at {} Hz collapsed SINR by >6 dB ({:.2} → {:.2})",
            doppler,
            static_mean,
            m,
        );
        // Doppler should not *narrow* the SINR distribution below the
        // static reference — only widen it.
        assert!(
            s + 0.2 >= prev_std,
            "std-dev did not widen with Doppler: {:.2} dB → {:.2} dB at {} Hz",
            prev_std,
            s,
            doppler,
        );
        prev_std = s;
    }
}

/// 4. Static residual CFO within typical post-acquisition budget
///    (±10 Hz) at realistic operating SINR (~7 dB). SINR must be
///    essentially unbiased. (Lab-grade SNR amplifies CFO bias because
///    the off-axis variance from a phase ramp dominates over thermal
///    noise; the Phase 1 diagnostic test 04b documents that knee.)
#[test]
fn scenario_04_residual_cfo_within_budget() {
    let target_amp = 1.0_f32;
    let baseline_chips = {
        let mut c = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 10);
        add_thermal_awgn(&mut c, REALISTIC_SIGMA_N, 10100);
        c
    };
    let baseline_mean = mean(
        &run_despreader(baseline_chips.clone())
            .iter()
            .map(|m| m.sinr_db)
            .collect::<Vec<_>>(),
    );
    for &cfo in &[-10.0_f32, 10.0] {
        let mut chips = baseline_chips.clone();
        apply_static_cfo(&mut chips, cfo);
        let m = mean(
            &run_despreader(chips)
                .iter()
                .map(|m| m.sinr_db)
                .collect::<Vec<_>>(),
        );
        eprintln!(
            "  cfo={:+5.1} Hz  mean_sinr={:+6.2} dB  baseline={:+6.2} dB",
            cfo, m, baseline_mean,
        );
        assert!(
            (m - baseline_mean).abs() < 0.3,
            "residual CFO {} Hz biased SINR by >{:.2} dB",
            cfo,
            (m - baseline_mean).abs(),
        );
    }
}

/// Diagnostic (not asserted as a hard pass/fail): characterize how
/// large CFO drives the metric down so Phase 2 can decide whether to
/// require a tighter CFO budget than the despreader assumes.
#[test]
fn scenario_04b_large_cfo_diagnostic() {
    let target_amp = 1.0_f32;
    let mut baseline = synth_target_chips(TargetSpec::pilot_only(target_amp), PCG_COUNT, 11);
    add_thermal_awgn(&mut baseline, SIGMA_N, 11100);
    let baseline_mean = mean(
        &run_despreader(baseline.clone())
            .iter()
            .map(|m| m.sinr_db)
            .collect::<Vec<_>>(),
    );
    eprintln!("  baseline (no CFO) sinr={:+6.2} dB", baseline_mean);
    for &cfo in &[50.0_f32, 100.0, 200.0, 400.0] {
        let mut chips = baseline.clone();
        apply_static_cfo(&mut chips, cfo);
        let m = mean(
            &run_despreader(chips)
                .iter()
                .map(|m| m.sinr_db)
                .collect::<Vec<_>>(),
        );
        eprintln!(
            "  cfo={:>5.0} Hz  sinr={:+6.2} dB  bias={:+5.2} dB",
            cfo,
            m,
            m - baseline_mean
        );
    }
}
