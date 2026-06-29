use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use log::{debug, info, trace};
use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::pipelined::{PipelineProcessorShared, SampleBlock};
use crate::sdr::cdma2000_baseband_filter_taps_f64;

use super::super::cfo_tracker;
use super::super::gardner_timing_recovery::{
    GardnerTimingAdjustment, GardnerTimingConfig, GardnerTimingRecovery,
};
use super::super::generic_rake_receiver::{BaseFinger, FingerProgress, RakeFinger};
use super::interpolation::{interp_complex_clamped, interp_complex_contiguous};

// With Gardner enabled we keep only one neighbor beside the verified prompt to
// avoid doubling slow false-finger work. Captures with very strong correlation
// peaks have consistently benefited from the late-side prompt, while marginal
// captures need the early side that preserved the existing v60s decode count.
pub(super) const ADAPTIVE_FINGER_TIMING_LATE_SNR_THRESHOLD: f32 = 200.0;

/// Power gain (in dB) introduced by the RX matched filter
/// (`PulseMatchedFilterProcessor`, which uses the same CDMA2000 baseband
/// filter taps as TX). Reported `raw_power_db` is measured *after* the
/// matched filter, so this gain is subtracted to refer the result back to
/// the ADC input (dBFS).
pub(super) fn rx_matched_filter_power_gain_db() -> f32 {
    static GAIN_DB: OnceLock<f32> = OnceLock::new();
    *GAIN_DB.get_or_init(|| {
        let taps = cdma2000_baseband_filter_taps_f64();
        let g: f64 = taps.iter().map(|t| t * t).sum();
        (10.0 * g.log10()) as f32
    })
}
pub(super) const DEFAULT_REACQUIRE_SIGNAL_LOST_CHIPS: u64 = 6_144;
pub(super) const DEFAULT_REACQUIRE_CRC_MISS_COUNT: u64 = 8;
// Repeated access probes often expose the same delay every 80 ms. Waiting
// two probe intervals before same-delay reacquisition keeps timing diversity
// coverage while avoiding a large bank of stale unvalidated fingers.
pub(super) const DEFAULT_REACQUIRE_IDLE_CHIPS: u64 = 196_608;
pub(super) const MAX_PENDING_ATTEMPTS_WITHOUT_HIT: u32 = 10;
pub(super) const MAX_PENDING_ATTEMPTS_WITH_HIT: u32 = 10;
// Plain rectangular PN verification stays on the integer prompt, but a tiny
// fractional-only CFO correction recovers one marginal legacy capture. Larger
// corrections have proven harmful because they can overfit W0 preamble energy
// and rotate later data frames off their CRC-clean phase.
pub(super) const PLAIN_CFO_REFINE_MAX_DELTA_RAD_PER_CHIP: f32 = 3.0e-6;

// ---------------------------------------------------------------------------
// PnLcFinger — active receiver path
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) struct ActiveFingerState {
    pub(super) id: u64,
    pub(super) delay_samples: i32,
    pub(super) hard_validated: bool,
    pub(super) idle_chips: u64,
    pub(super) signal_lost_chips: u64,
    pub(super) crc_miss_count: u64,
    pub(super) post_walsh_no_event_ms: u64,
}

/// One active RAKE finger that despreads a PN×LC channel.
///
/// Created by [`PnLcCorrelator`] and managed by [`GenericRakeReceiver`].
pub struct PnLcFinger {
    pub(crate) base: BaseFinger,

    /// Shared PN conjugate reference (pre-computed by the correlator).
    pn_seq: Arc<Vec<Complex32>>,
    phase_period: usize,
    oversample: usize,
    center_offset: usize,

    /// PN cursor pointing at the next prompt's sample-rate index into
    /// `pn_seq`. Initialized at finger creation to the punctual cursor
    /// (`acquisition despread_phase + center_offset`) and advances by
    /// `oversample` per processed chip — never per sample. Sub-chip
    /// slews from the EPL loop mutate this by ±1.
    pub(crate) despread_phase: usize,
    /// The acquisition-time value of `despread_phase`, frozen at finger
    /// creation. Only used as the `pilot_phase` metadata tag on output
    /// blocks, downsampled to chip index. Never read for runtime logic.
    acquisition_phase: usize,
    /// Offset (in input samples) into the next received block where the
    /// next prompt sample lives. Replaces the old
    /// `samples_to_skip + center_offset + partial_sample_count` triple.
    /// On finger creation it is set to `samples_to_skip + center_offset`
    /// (the offset of the first prompt within the first block). After
    /// each `despread_block` call it carries any leftover sample count
    /// to the next block. Always satisfies `next_prompt_offset <
    /// oversample` once we're past the initial skip.
    pub(crate) next_prompt_offset: f32,
    /// Fractional prompt position in input samples selected during candidate
    /// verification. 0.0 preserves the legacy integer 4x-grid behavior.
    timing_mu_samples: f32,
    /// If true, keep all oversample phases in the emitted chip stream and tag
    /// the output with `access_oversample = oversample`.
    output_oversampled_chips: bool,
    /// Sum all PN-despread samples in the oversampled chip interval instead of
    /// using only the prompt sample.
    integrate_and_dump: bool,
    /// Optional per-finger Gardner timing loop. Runs on raw matched-filtered
    /// samples and nudges `next_prompt_offset` after each processed chip.
    gardner_timing: Option<GardnerTimingRecovery>,

    /// LC generator seeded at the finger's first TX chip.
    lc_gen: LongCodeGenerator,
    /// Absolute TX chip corresponding to the next LC chip to be consumed.
    pub(crate) lc_chip_counter: usize,
    /// Sub-chain is only fed chips at or after this TX chip (frame alignment).
    pub(crate) chain_start_chip: usize,

    /// PN+LC-removed chip-rate samples awaiting dispatch to the sub-chain.
    sample_buffer: VecDeque<Complex32>,
    /// Chips per sub-chain output block.
    chip_block_size: usize,
    /// Total chips sent to the sub-chain so far.
    chain_chips_output: usize,
    /// LC despreading value for the current chip.
    /// For IS-95/RC1: real ±1 (im=0). For HPSK/RC3+: complex conj(c_long).
    current_lc_conj: Complex32,
    /// Whether the current chip is at or after `chain_start_chip`.
    current_chip_enabled: bool,
    /// LC decimation factor (1 = IS-95/RC1/RC2, 2 = HPSK/RC3+).
    lc_decimation: usize,

    // HPSK (RC3+ reverse link) state tracking
    /// Previous LC chip value (for Q long code = I long code delayed 1 chip).
    pub(super) hpsk_prev_lc: f32,
    /// Chip counter for W12 parity (W12(n) = (-1)^n).
    pub(super) hpsk_chip_count: usize,
    /// Decimated PN_Q × LC_Q value from the even chip of each pair.
    hpsk_dec_q: f32,

    // CFO tracking
    prev_pilot: Option<Complex32>,
    cfo_rad_per_chip: f32,
    cfo_phase: f32,
    /// RC3 pilot CFO tracker.  When present, this is the sole CFO source;
    /// the legacy 256-chip tracker is disabled and `cfo_rad_per_chip` /
    /// `cfo_phase` are synced from the tracker for diagnostic use only.
    rc3_cfo: Option<cfo_tracker::CfoTracker>,
    /// Reverse access CFO tracker (256-chip Walsh-symbol observations,
    /// coherence-gated coasting during data).
    access_cfo: Option<cfo_tracker::CfoTracker>,
    /// Diagnostic: accumulate raw and derotated chips for per-PCG pilot
    /// phase logging during despread_block.
    rc3_diag_raw_accum: Complex32,
    rc3_diag_derot_accum: Complex32,
    rc3_diag_chip_count: usize,
    rc3_diag_pcg_count: usize,
    /// CFO pilot observation: accumulate ONLY 16-chip EPL pilot sums
    /// (Walsh-0 coherent) for feeding to the CfoTracker. Completely
    /// separate from the diagnostic accumulators.
    rc3_cfo_pilot_accum: Complex32,
    rc3_cfo_pilot_chips: usize,

    /// Propagated from the input block for sub-chain SampleBlock metadata.
    sample_rate_hz: f64,

    /// Acquisition SNR at detection time.
    detection_snr: f32,

    // Incoherent energy tracking for signal-loss pruning
    /// Peak per-chip incoherent energy observed (running max).
    peak_energy: f32,
    /// Consecutive chips where incoherent energy stayed below the loss threshold.
    low_energy_chip_count: u64,
    /// Total chips processed while energy was above the loss threshold.
    high_energy_chip_count: u64,

    /// Replay outputs generated before the finger is first polled live.
    pending_output: Vec<SampleBlock>,
    /// RC3 closed-loop power-control measurement state. Accumulates one
    /// pilot symbol SINR estimate per 1.25 ms PCG directly at the finger.
    rc3_pcg_measurement_abs_chip_start: Option<u64>,
    rc3_pcg_measurement_prompt_chip_power: f64,
    rc3_pcg_measurement_pilot_run_prompt: Complex32,
    rc3_pcg_measurement_pilot_prompt_power: f64,
    rc3_pcg_measurement_pilot_chip_idx: usize,
    rc3_pcg_measurement_chip_count: usize,
    /// Coherent sum of 16-chip pilot symbols within the current PCG.
    rc3_pcg_measurement_pilot_coherent_sum: Complex32,
    /// Number of 16-chip pilot symbols accumulated in the current PCG.
    rc3_pcg_measurement_pilot_symbol_count: usize,
    /// Sliding window of per-PCG pilot moment tuples `(|Σ pilot_sym|²,
    /// Σ |pilot_sym|², n_symbols)` over the last RC3_PCG_SMOOTH_WINDOW PCGs.
    rc3_pcg_measurement_smoothing_window: VecDeque<(f64, f64, usize)>,

    // Raw (pre-despread) input power accumulator for Rx Power reporting
    raw_input_power_accum: f64,
    raw_input_power_count: u64,

    // Timing instrumentation
    despread_ns: u64,
    drain_ns: u64,
    finger_block_count: u64,
    /// Per sub-chain stage: (accumulated_ns, name)
    sub_chain_ns: Vec<(u64, &'static str)>,

    // ----- Early/prompt/late chip-timing instrumentation (measurement only) -----
    //
    // Pure measurement: never moves `despread_phase`, never affects the
    // sub-chain output, never feeds back into anything. Activates when the
    // owning correlator has `cfg.enable_epl_tracking == true` AND the
    // finger has been hard-validated downstream.
    //
    // Two metrics are accumulated in parallel at three sub-chip offsets
    // (early = -0.25 chip, prompt = 0 = chip center, late = +0.25 chip):
    //
    //   1. Per-chip envelope      `Σ |despread × lc|²`
    //      PN-blind (|pn|=|lc|=1), reflects raw RF envelope shape.
    //   2. 4-chip coherent power  `Σ |Σ_{4 chips} despread × lc|²`
    //      PN+LC-aware: only peaks at the correct despread alignment.
    //      4 chips is chosen because RC1 reverse traffic uses 64-ary
    //      Walsh, and 4 consecutive PN chips = 1 Walsh chip = guaranteed
    //      same sign on user data.
    //
    // Accumulators roll up every `EPL_WINDOW_CHIPS` chips. A log line is
    // printed whenever ~`EPL_LOG_CHIP_INTERVAL` chips (≈1 s of signal
    // time at 1.2288 Mcps) have elapsed since the last log.
    /// Enable flag, copied from correlator cfg at finger construction.
    epl_enabled: bool,
    /// Chips accumulated in the current (N-chip) window. Rolls over at
    /// `EPL_WINDOW_CHIPS`.
    epl_chips_in_window: usize,
    /// Chips since the last log line was emitted.
    epl_chips_since_log: u64,
    /// Number of completed windows folded into the pending log line.
    epl_windows_in_log: u64,
    /// Lifetime count of EPL log lines emitted for this finger.
    epl_log_seq: u64,
    /// Per-tap envelope accumulator rolled up across all windows in the
    /// current log interval (resets at each `EPL_TRACK` emit).
    epl_env_early: f64,
    epl_env_prompt: f64,
    epl_env_late: f64,
    /// Per-tap envelope accumulator for the CURRENT rollup window (resets
    /// every `EPL_WINDOW_CHIPS` chips).
    epl_window_env_early: f64,
    epl_window_env_prompt: f64,
    epl_window_env_late: f64,
    /// Snapshot of the most recently completed window's envelope sums.
    /// This gives `EPL_TRACK` a one-window `env` view comparable in scope
    /// to what the slew loop sees via `coh4`.
    epl_last_window_env_early: f64,
    epl_last_window_env_prompt: f64,
    epl_last_window_env_late: f64,
    /// Per-tap 4-chip coherent running sum. Resets every 4 chips.
    epl_coh4_run_early: Complex32,
    epl_coh4_run_prompt: Complex32,
    epl_coh4_run_late: Complex32,
    /// Position within the current 4-chip group (0..4).
    epl_coh4_chip_idx: usize,
    /// Per-tap 4-chip coherent squared-magnitude accumulator for the
    /// CURRENT rollup window (resets every `EPL_WINDOW_CHIPS` chips).
    /// Used by the slew loop to compute a per-window discriminator.
    epl_window_coh4_pwr_early: f64,
    epl_window_coh4_pwr_prompt: f64,
    epl_window_coh4_pwr_late: f64,
    /// Snapshot of the most recently completed window's coh4 sums.
    /// This is the exact window-scale coherent metric the slew loop just
    /// used for its discriminator.
    epl_last_window_coh4_pwr_early: f64,
    epl_last_window_coh4_pwr_prompt: f64,
    epl_last_window_coh4_pwr_late: f64,
    /// Per-tap 4-chip coherent squared-magnitude accumulator rolled up
    /// across all windows in the current log interval (resets at each
    /// `EPL_TRACK` log emit).
    epl_coh4_pwr_early: f64,
    epl_coh4_pwr_prompt: f64,
    epl_coh4_pwr_late: f64,

    // ----- EPL pilot-coherent 16-chip Walsh 0 (RC3+) -----
    /// When true, EPL uses 16-chip pilot accumulation instead of 4-chip.
    epl_pilot_mode: bool,
    /// Running complex sums over 16 chips for E/P/L taps.
    epl_pilot_run_early: Complex32,
    epl_pilot_run_prompt: Complex32,
    epl_pilot_run_late: Complex32,
    /// Position within the current 16-chip Walsh symbol (0..16).
    epl_pilot_chip_idx: usize,
    /// Window-level pilot power accumulators.
    epl_window_pilot_pwr_early: f64,
    epl_window_pilot_pwr_prompt: f64,
    epl_window_pilot_pwr_late: f64,
    /// Snapshot of last completed window's pilot power (for slew loop).
    epl_last_window_pilot_pwr_early: f64,
    epl_last_window_pilot_pwr_prompt: f64,
    epl_last_window_pilot_pwr_late: f64,
    /// Log-interval pilot power accumulators.
    epl_pilot_pwr_early: f64,
    epl_pilot_pwr_prompt: f64,
    epl_pilot_pwr_late: f64,
    /// Previous 16-chip pilot prompt sum for inter-symbol phase delta.
    epl_pilot_prev_prompt: Option<Complex32>,
    /// Accumulated phase deltas (radians) within the current sub-window.
    epl_pilot_cfo_phase_accum: f64,
    /// Number of phase delta measurements in the current sub-window.
    epl_pilot_cfo_count: usize,
    /// Total number of CFO sub-window updates applied (for warmup gain).
    epl_pilot_cfo_updates: usize,

    // ----- CFO residual monitoring -----
    /// Sum of |delta| (absolute residual phase per 16-chip symbol) since
    /// last FINGER DIAG log.
    cfo_residual_abs_sum: f64,
    /// Sum of delta² for RMS computation.
    cfo_residual_sq_sum: f64,
    /// Max |delta| seen since last log.
    cfo_residual_max: f32,
    /// Count of delta measurements since last log.
    cfo_residual_count: u64,

    /// Last completed log-interval reverse pilot Ec/Io estimate in dB.
    /// This matches the `pilot Ec/Io` field printed in `EPL_TRACK`.
    epl_last_log_pilot_ec_io_db: Option<f32>,

    // ----- EPL active slew (closed-loop sub-chip timing correction) -----
    /// Enable flag, copied from correlator cfg.
    epl_slew_enabled: bool,
    /// IIR-smoothed coh4 (E-L)/P discriminator value.
    epl_slew_iir: f64,
    /// Fractional sub-sample accumulator in [-1, +1]. When it crosses
    /// ±1, a one-sub-sample slew fires and the accumulator decreases
    /// by ±1.
    epl_slew_frac: f64,
    /// Lifetime count of slew events (forward - backward).
    epl_slew_total: i64,
    /// Windows since the last slew fired (rate limiter).
    epl_slew_windows_since: u64,
    /// Total windows processed since finger validation (warmup guard).
    epl_slew_windows_total: u64,
    /// Pending slew applied at the start of the next despread loop
    /// iteration. `None` = no slew pending. Signed to allow forward
    /// (+1) and backward (-1).
    epl_slew_pending: Option<i32>,
}

#[derive(Debug, Clone)]
pub(super) struct PendingCandidate {
    pub(super) id: u64,
    pub(super) delay_samples: i32,
    pub(super) lc_phase_hint: i32,
    pub(super) snr: f32,
    pub(super) preamble_hits: u32,
    pub(super) attempts: u32,
    pub(super) first_verified_tx_chip: Option<usize>,
    pub(super) first_verified_sample_offset: Option<usize>,
    pub(super) pn_reference_kind: Option<PnReferenceKind>,
    pub(super) timing_mu_samples: f32,
    pub(super) timing_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PnReferenceKind {
    Plain,
    Oqpsk,
}

/// Number of chips accumulated into one EPL rollup window.
/// At 4096 chips × ~814 ns/chip ≈ 3.3 ms = 16 Walsh symbols worth.
const EPL_WINDOW_CHIPS: usize = 4096;

/// Number of chips between log lines, independent of window size.
/// 1.2288 M chips ≈ 1 s of signal time at the nominal chip rate.
const EPL_LOG_CHIP_INTERVAL: u64 = 1_228_800;

/// Number of EPL rollup windows to wait (after hard-validation) before
/// enabling active slew corrections. Gives the discriminator IIR time
/// to settle before it starts moving the needle.
const EPL_SLEW_WARMUP_WINDOWS: u64 = 100;

/// Pilot-mode warmup: longer because the 16-chip pilot discriminator
/// has higher per-window variance from multipath asymmetry.
const EPL_SLEW_WARMUP_WINDOWS_PILOT: u64 = 500;

/// Magnitude of the (E-L)/P discriminator below which no slew is
/// applied. Suppresses noise-driven slews when the finger is already
/// aligned.
const EPL_SLEW_DEAD_ZONE: f64 = 0.15;

/// Pilot-mode dead-zone: wider because the 16-chip pilot discriminator
/// IIR settles to ±0.15–0.25 even when well-aligned, due to multipath
/// asymmetry. Real clock drift pushes well past 0.25.
const EPL_SLEW_DEAD_ZONE_PILOT: f64 = 0.25;

/// IIR smoothing factor for the discriminator. A value of 0.2
/// means each new window replaces 20% of the smoothed value.
const EPL_SLEW_ALPHA: f64 = 0.20;

/// Pilot-mode IIR alpha: more smoothing to dampen per-window variance.
const EPL_SLEW_ALPHA_PILOT: f64 = 0.05;

/// Loop gain: fraction of (sub-sample) integrated per window. 0.05
/// means a persistent 1.0 discriminator drives 0.05 sub-sample per
/// window toward the fractional accumulator; 20 windows to cross the
/// ±1 threshold and fire a slew.
const EPL_SLEW_LOOP_GAIN: f64 = 0.05;

/// Pilot-mode loop gain: lower to prevent transient spikes from
/// accumulating enough to fire.
const EPL_SLEW_LOOP_GAIN_PILOT: f64 = 0.01;

/// Minimum number of EPL windows between consecutive slews. Prevents
/// runaway at high loop gain.
const EPL_SLEW_MIN_WINDOWS_BETWEEN: u64 = 50;

/// Pilot-mode min-between: wider spacing since clock drift is slow
/// (ppm-level) and the pilot discriminator is noisier per-window.
const EPL_SLEW_MIN_WINDOWS_BETWEEN_PILOT: u64 = 300;

/// Pilot CFO tracker sub-window size (steady state).
/// 128 symbols × 16 chips = 2048 chips ≈ 1.7 ms.
const PILOT_CFO_SUB_WINDOW_SYMBOLS: usize = 128;

/// Pilot CFO tracker sub-window size during warmup.
/// Shorter window (32 symbols = 512 chips ≈ 0.42 ms) for faster
/// convergence while the legacy tracker provides coarse correction.
const PILOT_CFO_SUB_WINDOW_SYMBOLS_WARMUP: usize = 32;

/// IIR gain per pilot CFO sub-window update (steady state).
const PILOT_CFO_GAIN: f32 = 0.05;

/// Higher gain during initial convergence.
const PILOT_CFO_GAIN_WARMUP: f32 = 0.30;

/// Number of sub-window CFO updates before switching from warmup to
/// steady-state gain.  With 32-symbol warmup windows at ~0.42 ms each,
/// 240 updates ≈ 100 ms wall-clock warmup.
const PILOT_CFO_WARMUP_UPDATES: usize = 240;
const RC3_PCG_CHIPS: usize = 1_536;
const RC3_PILOT_SYMBOL_CHIPS: usize = 16;
const RC3_PILOT_CHIPS_PER_PCG: usize = 1_152;
/// `N` factor in the per-symbol SINR formula. Always per-PCG, never K*N
/// across the smoothing window — passing K*N mis-reports SINR by 10·log10(K) dB.
const RC3_PILOT_SYMBOLS_PER_PCG: usize = RC3_PILOT_CHIPS_PER_PCG / RC3_PILOT_SYMBOL_CHIPS;

/// Sliding window length for per-PCG pilot moment aggregation.
const RC3_PCG_SMOOTH_WINDOW: usize = 8;

impl PnLcFinger {
    /// Per-PCG pilot symbol SINR (dB) from on-axis vs. off-axis decomposition
    /// of despread pilot symbols. Chosen over Ec/Io because Ec/Io saturates
    /// with Tx power; pilot symbol SINR scales 1:1 in the noise-limited regime.
    fn pilot_sym_sinr_db_from_metrics(
        pilot_norm_sq: f64,
        pilot_prompt_power: f64,
        n_symbols: usize,
    ) -> f32 {
        if n_symbols == 0 {
            return f32::NAN;
        }
        let n = n_symbols as f64;
        let denom = (n * pilot_prompt_power - pilot_norm_sq).max(1e-12);
        let lin = (pilot_norm_sq / denom).max(1e-12);
        10.0 * (lin as f32).log10()
    }

    /// Legacy pilot Ec/Io (dB) — diagnostic tag only, does not drive the loop.
    fn pilot_ec_io_db_from_prompt_power(pilot_prompt_power: f64, prompt_chip_power: f64) -> f32 {
        let linear = if prompt_chip_power > 1e-12 {
            (pilot_prompt_power / (16.0 * prompt_chip_power)).max(1e-12)
        } else {
            1e-12
        };
        10.0 * (linear as f32).log10()
    }

    fn reset_rc3_pcg_measurement(&mut self) {
        self.rc3_pcg_measurement_abs_chip_start = None;
        self.rc3_pcg_measurement_prompt_chip_power = 0.0;
        self.rc3_pcg_measurement_pilot_run_prompt = Complex32::new(0.0, 0.0);
        self.rc3_pcg_measurement_pilot_prompt_power = 0.0;
        self.rc3_pcg_measurement_pilot_chip_idx = 0;
        self.rc3_pcg_measurement_chip_count = 0;
        self.rc3_pcg_measurement_pilot_coherent_sum = Complex32::new(0.0, 0.0);
        self.rc3_pcg_measurement_pilot_symbol_count = 0;
    }

    fn emit_rc3_pcg_measurement(&mut self) {
        let Some(abs_chip_start) = self.rc3_pcg_measurement_abs_chip_start else {
            self.reset_rc3_pcg_measurement();
            return;
        };
        let hard_validated = self.base.is_hard_validated();
        let raw_power_dbfs = 10.0
            * ((self.rc3_pcg_measurement_prompt_chip_power / RC3_PCG_CHIPS as f64)
                .max(1e-15)
                .log10() as f32);
        let pilot_norm_sq = self.rc3_pcg_measurement_pilot_coherent_sum.norm_sqr() as f64;
        let n_symbols_this_pcg = self.rc3_pcg_measurement_pilot_symbol_count;
        let pilot_prompt_power_this_pcg = self.rc3_pcg_measurement_pilot_prompt_power;
        let (raw_sinr_db, sinr_db, ec_io_db, smoothing_window_len) = if hard_validated {
            let raw_sinr_db = Self::pilot_sym_sinr_db_from_metrics(
                pilot_norm_sq,
                pilot_prompt_power_this_pcg,
                n_symbols_this_pcg,
            );
            if self.rc3_pcg_measurement_smoothing_window.len() >= RC3_PCG_SMOOTH_WINDOW {
                self.rc3_pcg_measurement_smoothing_window.pop_front();
            }
            self.rc3_pcg_measurement_smoothing_window.push_back((
                pilot_norm_sq,
                pilot_prompt_power_this_pcg,
                n_symbols_this_pcg,
            ));
            // K factor cancels in the ratio: pass per-PCG N below (not K*N),
            // or SINR is mis-reported by 10·log10(K) dB low.
            let mut window_norm_sq = 0.0_f64;
            let mut window_prompt_pwr = 0.0_f64;
            let mut window_pcgs = 0_usize;
            for &(ns, pp, _) in &self.rc3_pcg_measurement_smoothing_window {
                window_norm_sq += ns;
                window_prompt_pwr += pp;
                window_pcgs += 1;
            }
            let avg_norm_sq = window_norm_sq / window_pcgs.max(1) as f64;
            let avg_prompt_pwr = window_prompt_pwr / window_pcgs.max(1) as f64;
            (
                Some(raw_sinr_db),
                Self::pilot_sym_sinr_db_from_metrics(
                    avg_norm_sq,
                    avg_prompt_pwr,
                    RC3_PILOT_SYMBOLS_PER_PCG,
                ),
                Some(Self::pilot_ec_io_db_from_prompt_power(
                    self.rc3_pcg_measurement_pilot_prompt_power,
                    self.rc3_pcg_measurement_prompt_chip_power,
                )),
                self.rc3_pcg_measurement_smoothing_window.len(),
            )
        } else {
            self.rc3_pcg_measurement_smoothing_window.clear();
            (None, f32::NAN, None, 0)
        };
        let chip_rate_hz = if self.oversample > 0 {
            self.sample_rate_hz / self.oversample as f64
        } else {
            self.sample_rate_hz
        };
        let mut block =
            SampleBlock::new(Vec::new(), abs_chip_start as usize).with_sample_rate_hz(chip_rate_hz);
        block
            .tags
            .insert("absolute_chip_start", abs_chip_start as i64);
        block.tags.insert("traffic_pcg_measurement", 1);
        block.tags.insert("traffic_measurement_age_chips", 0);
        block.tags.insert(
            "traffic_pcg_raw_power_mdb",
            (raw_power_dbfs * 1000.0) as i64,
        );
        if let Some(ec_io_db) = ec_io_db {
            block
                .tags
                .insert("traffic_pcg_pilot_ec_io_mdb", (ec_io_db * 1000.0) as i64);
        }
        if let Some(raw_sinr_db) = raw_sinr_db {
            block.tags.insert(
                "traffic_pcg_pilot_sinr_raw_mdb",
                (raw_sinr_db * 1000.0) as i64,
            );
        }
        block
            .tags
            .insert("traffic_pcg_smoothing_window", smoothing_window_len as i64);
        if !hard_validated {
            block.tags.insert("traffic_pcg_raw_only", 1);
        }
        block.tags.insert("finger_id", self.base.id as i64);
        block.pcg_signal_snr_db = Some(vec![sinr_db]);
        self.pending_output.push(block);
        self.reset_rc3_pcg_measurement();
    }

    fn update_rc3_pcg_measurement(&mut self, chip_tx: usize, prompt_chip: Complex32) {
        if !self.current_chip_enabled {
            self.reset_rc3_pcg_measurement();
            // Lock loss → smoothing window is no longer meaningful.
            self.rc3_pcg_measurement_smoothing_window.clear();
            return;
        }

        if self.rc3_pcg_measurement_chip_count == 0 {
            if chip_tx % RC3_PCG_CHIPS != 0 {
                return;
            }
            self.rc3_pcg_measurement_abs_chip_start = Some(chip_tx as u64);
        }

        self.rc3_pcg_measurement_prompt_chip_power += prompt_chip.norm_sqr() as f64;
        let pcg_chip_offset = self.rc3_pcg_measurement_chip_count % RC3_PCG_CHIPS;
        if pcg_chip_offset < RC3_PILOT_CHIPS_PER_PCG {
            self.rc3_pcg_measurement_pilot_run_prompt += prompt_chip;
            self.rc3_pcg_measurement_pilot_chip_idx += 1;
        }
        self.rc3_pcg_measurement_chip_count += 1;

        if self.rc3_pcg_measurement_pilot_chip_idx >= RC3_PILOT_SYMBOL_CHIPS {
            let pilot_sym = self.rc3_pcg_measurement_pilot_run_prompt;
            self.rc3_pcg_measurement_pilot_prompt_power += pilot_sym.norm_sqr() as f64;
            self.rc3_pcg_measurement_pilot_coherent_sum += pilot_sym;
            self.rc3_pcg_measurement_pilot_symbol_count += 1;
            self.rc3_pcg_measurement_pilot_run_prompt = Complex32::new(0.0, 0.0);
            self.rc3_pcg_measurement_pilot_chip_idx = 0;
        }

        if self.rc3_pcg_measurement_chip_count >= RC3_PCG_CHIPS {
            self.emit_rc3_pcg_measurement();
        }
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: u64,
        pn_seq: Arc<Vec<Complex32>>,
        phase_period: usize,
        oversample: usize,
        despread_phase: usize,
        center_offset: usize,
        lc_gen: LongCodeGenerator,
        lc_chip_counter: usize,
        chain_start_chip: usize,
        chip_block_size: usize,
        samples_to_skip: usize,
        detection_snr: f32,
        initial_cfo_rad_per_chip: f32,
        lc_decimation: usize,
        enable_epl_tracking: bool,
        enable_epl_slew: bool,
        epl_pilot: bool,
        access_cfo: bool,
        timing_mu_samples: f32,
        output_oversampled_chips: bool,
        integrate_and_dump: bool,
    ) -> Self {
        Self::new_with_gardner(
            id,
            pn_seq,
            phase_period,
            oversample,
            despread_phase,
            center_offset,
            lc_gen,
            lc_chip_counter,
            chain_start_chip,
            chip_block_size,
            samples_to_skip,
            detection_snr,
            initial_cfo_rad_per_chip,
            lc_decimation,
            enable_epl_tracking,
            enable_epl_slew,
            epl_pilot,
            access_cfo,
            timing_mu_samples,
            output_oversampled_chips,
            integrate_and_dump,
            GardnerTimingConfig::disabled(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_with_gardner(
        id: u64,
        pn_seq: Arc<Vec<Complex32>>,
        phase_period: usize,
        oversample: usize,
        despread_phase: usize,
        center_offset: usize,
        lc_gen: LongCodeGenerator,
        lc_chip_counter: usize,
        chain_start_chip: usize,
        chip_block_size: usize,
        samples_to_skip: usize,
        detection_snr: f32,
        initial_cfo_rad_per_chip: f32,
        lc_decimation: usize,
        enable_epl_tracking: bool,
        enable_epl_slew: bool,
        epl_pilot: bool,
        access_cfo: bool,
        timing_mu_samples: f32,
        output_oversampled_chips: bool,
        integrate_and_dump: bool,
        gardner_timing: GardnerTimingConfig,
    ) -> Self {
        // Fold `center_offset` into the stored cursor: `despread_phase`
        // now points directly at the prompt sample of the first chip.
        // Per-chip iteration in `despread_block` reads `pn_seq[dp]`,
        // applies LC, then advances `dp += oversample`. The caller
        // still passes the old-convention `(despread_phase,
        // center_offset)` pair for backward compatibility.
        let prompt_phase = (despread_phase + center_offset) % phase_period;
        Self {
            base: BaseFinger::new(id),
            pn_seq,
            phase_period,
            oversample,
            center_offset,
            despread_phase: prompt_phase,
            acquisition_phase: prompt_phase,
            next_prompt_offset: samples_to_skip as f32 + center_offset as f32 + timing_mu_samples,
            timing_mu_samples,
            output_oversampled_chips,
            integrate_and_dump,
            gardner_timing: GardnerTimingRecovery::new(
                gardner_timing.with_samples_per_symbol(oversample as f32),
                timing_mu_samples,
            ),
            lc_gen,
            lc_chip_counter,
            chain_start_chip,
            sample_buffer: VecDeque::new(),
            chip_block_size,
            chain_chips_output: 0,
            current_lc_conj: Complex32::new(1.0, 0.0),
            current_chip_enabled: false,
            lc_decimation: lc_decimation.max(1),
            hpsk_prev_lc: 1.0,
            hpsk_chip_count: 0,
            hpsk_dec_q: 1.0,
            prev_pilot: None,
            cfo_rad_per_chip: initial_cfo_rad_per_chip,
            cfo_phase: 0.0,
            rc3_cfo: if epl_pilot {
                Some(cfo_tracker::CfoTracker::new_rc3_traffic(
                    initial_cfo_rad_per_chip,
                ))
            } else {
                None
            },
            access_cfo: if access_cfo {
                Some(cfo_tracker::CfoTracker::new_reverse_access(
                    initial_cfo_rad_per_chip,
                ))
            } else {
                None
            },
            rc3_diag_raw_accum: Complex32::new(0.0, 0.0),
            rc3_diag_derot_accum: Complex32::new(0.0, 0.0),
            rc3_diag_chip_count: 0,
            rc3_diag_pcg_count: 0,
            rc3_cfo_pilot_accum: Complex32::new(0.0, 0.0),
            rc3_cfo_pilot_chips: 0,
            sample_rate_hz: 0.0,
            detection_snr,
            peak_energy: 0.0,
            low_energy_chip_count: 0,
            high_energy_chip_count: 0,
            pending_output: Vec::new(),
            rc3_pcg_measurement_abs_chip_start: None,
            rc3_pcg_measurement_prompt_chip_power: 0.0,
            rc3_pcg_measurement_pilot_run_prompt: Complex32::new(0.0, 0.0),
            rc3_pcg_measurement_pilot_prompt_power: 0.0,
            rc3_pcg_measurement_pilot_chip_idx: 0,
            rc3_pcg_measurement_chip_count: 0,
            rc3_pcg_measurement_pilot_coherent_sum: Complex32::new(0.0, 0.0),
            rc3_pcg_measurement_pilot_symbol_count: 0,
            rc3_pcg_measurement_smoothing_window: VecDeque::with_capacity(RC3_PCG_SMOOTH_WINDOW),
            raw_input_power_accum: 0.0,
            raw_input_power_count: 0,
            despread_ns: 0,
            drain_ns: 0,
            finger_block_count: 0,
            sub_chain_ns: Vec::new(),
            epl_enabled: enable_epl_tracking,
            epl_chips_in_window: 0,
            epl_chips_since_log: 0,
            epl_windows_in_log: 0,
            epl_log_seq: 0,
            epl_env_early: 0.0,
            epl_env_prompt: 0.0,
            epl_env_late: 0.0,
            epl_window_env_early: 0.0,
            epl_window_env_prompt: 0.0,
            epl_window_env_late: 0.0,
            epl_last_window_env_early: 0.0,
            epl_last_window_env_prompt: 0.0,
            epl_last_window_env_late: 0.0,
            epl_coh4_run_early: Complex32::new(0.0, 0.0),
            epl_coh4_run_prompt: Complex32::new(0.0, 0.0),
            epl_coh4_run_late: Complex32::new(0.0, 0.0),
            epl_coh4_chip_idx: 0,
            epl_window_coh4_pwr_early: 0.0,
            epl_window_coh4_pwr_prompt: 0.0,
            epl_window_coh4_pwr_late: 0.0,
            epl_last_window_coh4_pwr_early: 0.0,
            epl_last_window_coh4_pwr_prompt: 0.0,
            epl_last_window_coh4_pwr_late: 0.0,
            epl_coh4_pwr_early: 0.0,
            epl_coh4_pwr_prompt: 0.0,
            epl_coh4_pwr_late: 0.0,
            epl_pilot_mode: epl_pilot,
            epl_pilot_run_early: Complex32::new(0.0, 0.0),
            epl_pilot_run_prompt: Complex32::new(0.0, 0.0),
            epl_pilot_run_late: Complex32::new(0.0, 0.0),
            epl_pilot_chip_idx: 0,
            epl_window_pilot_pwr_early: 0.0,
            epl_window_pilot_pwr_prompt: 0.0,
            epl_window_pilot_pwr_late: 0.0,
            epl_last_window_pilot_pwr_early: 0.0,
            epl_last_window_pilot_pwr_prompt: 0.0,
            epl_last_window_pilot_pwr_late: 0.0,
            epl_pilot_pwr_early: 0.0,
            epl_pilot_pwr_prompt: 0.0,
            epl_pilot_pwr_late: 0.0,
            epl_pilot_prev_prompt: None,
            epl_pilot_cfo_phase_accum: 0.0,
            epl_pilot_cfo_count: 0,
            epl_pilot_cfo_updates: 0,
            cfo_residual_abs_sum: 0.0,
            cfo_residual_sq_sum: 0.0,
            cfo_residual_max: 0.0,
            cfo_residual_count: 0,
            epl_last_log_pilot_ec_io_db: None,
            epl_slew_enabled: enable_epl_slew && enable_epl_tracking,
            epl_slew_iir: 0.0,
            epl_slew_frac: 0.0,
            epl_slew_total: 0,
            epl_slew_windows_since: 0,
            epl_slew_windows_total: 0,
            epl_slew_pending: None,
        }
    }

    fn despread_chip_prompt(
        &self,
        samples: &[Complex32],
        prompt_idx: f32,
        prompt_val: Complex32,
        prompt_pn: Complex32,
    ) -> Complex32 {
        if !self.integrate_and_dump || self.oversample <= 1 {
            return prompt_pn * prompt_val;
        }

        let pp = self.phase_period;
        let center = self.center_offset % self.oversample;
        let chip_start_idx = prompt_idx - center as f32;
        let chip_start_phase = (self.despread_phase + pp - center) % pp;

        let mut acc = Complex32::new(0.0, 0.0);
        let mut count = 0usize;
        for sample_phase in 0..self.oversample {
            let sample_t = chip_start_idx + sample_phase as f32;
            let Some(sample) = interp_complex_contiguous(samples, sample_t) else {
                continue;
            };
            let pn = self.pn_seq[(chip_start_phase + sample_phase) % pp];
            acc += pn * sample;
            count += 1;
        }

        if count == 0 {
            prompt_pn * prompt_val
        } else {
            acc
        }
    }

    /// Advance the LC generator (and HPSK state) to the next chip, updating
    /// `current_lc_conj` and `current_chip_enabled`. Shared between the
    /// in-loop chip boundary and the finger construction pre-seed.
    ///
    /// `pn_at_chip_start` is `pn_seq[despread_phase]` at the moment this
    /// is called — i.e., the PN value at the first sample of the new chip.
    /// Only read for HPSK (lc_decimation >= 2); RC1 ignores it.
    fn advance_lc_for_new_chip(&mut self, pn_at_chip_start: Complex32) {
        let chip_tx = self.lc_chip_counter;
        if self.lc_decimation >= 2 {
            // HPSK (RC3+ reverse link): composite PN×LC despreading.
            let lc_i: f32 = if self.lc_gen.next_chip() == 1 {
                -1.0
            } else {
                1.0
            };
            let pn_i = pn_at_chip_start.re;
            // pn_seq stores conj(PN), so .im = -PN_Q; negate for true PN_Q.
            let pn_q = -pn_at_chip_start.im;
            // W12(n) = (-1)^n per spec (first chip of frame = even).
            let w12: f32 = if self.hpsk_chip_count % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            // Q long code decimation: at even chips, compute and store the
            // (PN_Q × LC_Q) value. At odd chips, reuse the stored value.
            if self.hpsk_chip_count % 2 == 0 {
                self.hpsk_dec_q = pn_q * self.hpsk_prev_lc;
            }
            let cross = w12 * pn_i * pn_q * self.hpsk_dec_q;
            let re = lc_i * (1.0 - cross) * 0.5;
            let im = lc_i * (w12 * self.hpsk_dec_q + pn_i * pn_q) * 0.5;
            self.current_lc_conj = Complex32::new(re, im);
            self.hpsk_prev_lc = lc_i;
            self.hpsk_chip_count += 1;
        } else {
            let bit = self.lc_gen.next_chip();
            self.current_lc_conj = Complex32::new(if bit == 1 { -1.0 } else { 1.0 }, 0.0);
        }
        self.current_chip_enabled = chip_tx >= self.chain_start_chip;
        self.lc_chip_counter += 1;
    }

    /// Despread an input block at chip rate, emitting one chip-rate
    /// sample per processed chip into `sample_buffer`.
    ///
    /// ## Per-chip iteration
    ///
    /// The loop iterates **once per chip**, not once per input sample.
    /// `despread_phase` points at the prompt sub-sample of the next
    /// chip and advances by `oversample` per iteration. The `(os-1)`
    /// in-between sub-samples are not iterated over at all (except by
    /// the optional EPL early/late peeks).
    ///
    /// ## State carried across blocks
    ///
    /// `next_prompt_offset` is the offset (in input samples) into the
    /// next received block where the next prompt sample lives. Set to
    /// `samples_to_skip + center_offset` at finger creation. After
    /// processing a block of length `L`, the leftover is
    /// `(idx + os) - L` if at least one chip was processed, or
    /// `next_prompt_offset - L` if the block was too small to reach
    /// the first prompt.
    ///
    /// ## Slewing
    ///
    /// EPL slewing bumps `despread_phase` by ±1 (PN slides one
    /// sub-sample) and the corresponding `next_prompt_offset` by ±1
    /// (the prompt input sample shifts in lockstep). Both shifts move
    /// the despreading reference together so PN and LC framing stay
    /// locked.
    pub(crate) fn despread_block(&mut self, samples: &[Complex32]) {
        let os = self.oversample;
        let pp = self.phase_period;
        let len = samples.len();
        let len_f = len as f32;

        // Accumulate raw input power for every received sample (used
        // by Rx Power reporting; doesn't depend on chip alignment).
        for &val in samples {
            self.raw_input_power_accum += val.norm_sqr() as f64;
        }
        self.raw_input_power_count += len as u64;

        // A negative fractional prompt can happen when acquisition chooses a
        // slightly-early `timing_mu` at the first replay sample. That prompt is
        // before the available input block, so skip that chip while keeping PN
        // and LC cursors in lockstep.
        while self.next_prompt_offset < 0.0 {
            let pn = self.pn_seq[self.despread_phase];
            self.advance_lc_for_new_chip(pn);
            self.despread_phase = (self.despread_phase + os) % pp;
            self.next_prompt_offset += os as f32;
        }

        // If the next prompt is past the end of this block, just
        // decrement the offset and return — no chips to process.
        if self.next_prompt_offset >= len_f {
            self.next_prompt_offset -= len_f;
            return;
        }

        let mut idx = self.next_prompt_offset;
        while idx < len_f {
            // Apply any pending EPL slew BEFORE reading PN / advancing
            // LC, so PN and chip framing stay locked to the new
            // reference position. A ±1 slew shifts both the PN cursor
            // and the prompt input sample by one sub-sample. If the
            // slew would push the prompt past the end of this block,
            // clamp and carry leftover into the next block.
            if let Some(slew) = self.epl_slew_pending.take() {
                let old_dp = self.despread_phase;
                let old_idx = idx;
                if slew >= 0 {
                    let delta = slew as usize;
                    self.despread_phase = (self.despread_phase + delta) % pp;
                    idx += delta as f32;
                } else {
                    let delta = (-slew) as usize;
                    // Backward slew: wrap through phase_period if needed.
                    self.despread_phase = (self.despread_phase + pp - delta) % pp;
                    idx = (idx - delta as f32).max(0.0);
                }
                info!(
                    "EPL_SLEW[finger={}] direction={:+} despread_phase={}->{} \
                     idx={}->{} total={}",
                    self.base.id,
                    slew,
                    old_dp,
                    self.despread_phase,
                    old_idx,
                    idx,
                    self.epl_slew_total,
                );
                if idx >= len_f {
                    // Slew pushed the prompt out of the current block;
                    // carry the shortfall to the next call.
                    self.next_prompt_offset = idx - len_f;
                    return;
                }
            }

            // Per-chip iteration: this iter is a prompt by construction.
            let Some(val) = interp_complex_contiguous(samples, idx) else {
                break;
            };
            let mut gardner_adjust = GardnerTimingAdjustment::default();
            let mut gardner_finished = false;
            let gardner_mid = self
                .gardner_timing
                .as_ref()
                .filter(|gardner| gardner.is_tracking_active() && gardner.needs_midpoint())
                .and_then(|_| interp_complex_contiguous(samples, idx - os as f32 * 0.5));
            if let Some(gardner) = self.gardner_timing.as_mut() {
                if gardner.is_tracking_active() {
                    gardner_adjust = gardner.observe(val, gardner_mid);
                    gardner_finished = !gardner.is_tracking_active();
                } else {
                    gardner_finished = true;
                }
            }
            if gardner_finished {
                self.gardner_timing = None;
            }

            // Advance LC for the new chip first, so `current_lc_conj`
            // is the LC for the chip whose prompt we're about to read.
            let pn = self.pn_seq[self.despread_phase];
            let despread = self.despread_chip_prompt(samples, idx, val, pn);
            self.advance_lc_for_new_chip(pn);

            if self.lc_decimation >= 2 {
                let chip_offset = self.hpsk_chip_count.wrapping_sub(self.chain_start_chip + 1);
                if chip_offset < 5 || (chip_offset >= 322 && chip_offset <= 328) {
                    trace!(
                        "HPSK finger chip[{}] offset={} lc_gen_state=0x{:010X} lc_conj=({:.3},{:.3}) despread_phase={}",
                        self.hpsk_chip_count.saturating_sub(1),
                        chip_offset,
                        self.lc_gen.state(),
                        self.current_lc_conj.re,
                        self.current_lc_conj.im,
                        self.despread_phase,
                    );
                }
            }

            let epl_active =
                self.epl_enabled && self.current_chip_enabled && self.base.is_hard_validated();

            if self.current_chip_enabled {
                let out = despread * self.current_lc_conj;
                let chip_tx = self.lc_chip_counter.saturating_sub(1);
                let mut pcg_measurement_prompt_chip = out;
                if self.lc_decimation >= 2 {
                    let chip_offset = self.lc_chip_counter.wrapping_sub(self.chain_start_chip + 1);
                    if chip_offset < 5 || (chip_offset >= 318 && chip_offset <= 326) {
                        trace!(
                            "HPSK output[{}] despread=({:.3},{:.3}) lc_conj=({:.3},{:.3}) out=({:.3},{:.3})",
                            chip_offset,
                            despread.re,
                            despread.im,
                            self.current_lc_conj.re,
                            self.current_lc_conj.im,
                            out.re,
                            out.im
                        );
                    }
                }
                if self.output_oversampled_chips && self.oversample > 1 {
                    let center = self.center_offset % self.oversample;
                    let chip_start_idx = idx - center as f32;
                    let chip_start_phase = (self.despread_phase + pp - center) % pp;
                    for sample_phase in 0..self.oversample {
                        let sample_t = chip_start_idx + sample_phase as f32;
                        let sample = interp_complex_clamped(samples, sample_t);
                        let pn = self.pn_seq[(chip_start_phase + sample_phase) % pp];
                        let mut chip = pn * sample * self.current_lc_conj;
                        if let Some(ref mut cfo) = self.rc3_cfo {
                            let mut slice = [chip];
                            cfo.derotate_chips(&mut slice, self.oversample);
                            chip = slice[0];
                        }
                        self.sample_buffer.push_back(chip);
                    }
                } else {
                    // Non-oversampled: derotate the single prompt chip
                    // (this advances cfo_phase by one chip step).
                    if let Some(ref mut cfo) = self.rc3_cfo {
                        let mut slice = [out];
                        cfo.derotate_chips(&mut slice, 1);
                        self.sample_buffer.push_back(slice[0]);
                        pcg_measurement_prompt_chip = slice[0];

                        // Diagnostic: accumulate raw and derotated chips.
                        self.rc3_diag_raw_accum += out;
                        self.rc3_diag_derot_accum += slice[0];
                        self.rc3_diag_chip_count += 1;
                        if self.rc3_diag_chip_count >= 1536 {
                            let raw_phase = self
                                .rc3_diag_raw_accum
                                .im
                                .atan2(self.rc3_diag_raw_accum.re)
                                .to_degrees();
                            let raw_norm = self.rc3_diag_raw_accum.norm();
                            let derot_phase = self
                                .rc3_diag_derot_accum
                                .im
                                .atan2(self.rc3_diag_derot_accum.re)
                                .to_degrees();
                            let derot_norm = self.rc3_diag_derot_accum.norm();
                            let cfo_hz = cfo.cfo_rad_per_chip() as f64 * 1_228_800.0
                                / (2.0 * std::f64::consts::PI);
                            if self.rc3_diag_pcg_count < 40 || self.rc3_diag_pcg_count % 800 == 0 {
                                debug!(
                                    "RC3_CFO_DIAG finger={} pcg={} | raw: phase={:.1}° norm={:.1} | derot: phase={:.1}° norm={:.1} | cfo={:.1}Hz warmup={}",
                                    self.base.id,
                                    self.rc3_diag_pcg_count,
                                    raw_phase,
                                    raw_norm,
                                    derot_phase,
                                    derot_norm,
                                    cfo_hz,
                                    cfo.in_warmup(),
                                );
                            }
                            self.rc3_diag_raw_accum = Complex32::new(0.0, 0.0);
                            self.rc3_diag_derot_accum = Complex32::new(0.0, 0.0);
                            self.rc3_diag_chip_count = 0;
                            self.rc3_diag_pcg_count += 1;
                        }
                    } else {
                        self.sample_buffer.push_back(out);
                    }
                }

                if self.epl_pilot_mode {
                    self.update_rc3_pcg_measurement(chip_tx, pcg_measurement_prompt_chip);
                }

                if epl_active {
                    // Envelope (non-coherent) — always accumulated.
                    self.epl_window_env_prompt += out.norm_sqr() as f64;

                    // Compute early/late despread values (shared by both modes).
                    let out_e = if idx >= 1.0 {
                        let val_e = interp_complex_contiguous(samples, idx - 1.0).unwrap_or(val);
                        let pn_e = self.pn_seq[(self.despread_phase + pp - 1) % pp];
                        let e = pn_e * val_e * self.current_lc_conj;
                        self.epl_window_env_early += e.norm_sqr() as f64;
                        Some(e)
                    } else {
                        None
                    };
                    let out_l = if idx + 1.0 < len_f {
                        let val_l = interp_complex_contiguous(samples, idx + 1.0).unwrap_or(val);
                        let pn_l = self.pn_seq[(self.despread_phase + 1) % pp];
                        let l = pn_l * val_l * self.current_lc_conj;
                        self.epl_window_env_late += l.norm_sqr() as f64;
                        Some(l)
                    } else {
                        None
                    };

                    // Coherent accumulation — branch by mode.
                    if self.epl_pilot_mode {
                        // Pilot-coherent: Walsh 0 (all +1) → identity.
                        // Open-loop: feed RAW (pre-derotation) chips to
                        // the pilot accumulator so the tracker measures
                        // the TOTAL phase rotation, not the residual.
                        self.epl_pilot_run_prompt += out;
                        if let Some(e) = out_e {
                            self.epl_pilot_run_early += e;
                        }
                        if let Some(l) = out_l {
                            self.epl_pilot_run_late += l;
                        }
                    } else {
                        // Generic 4-chip coherent (RC1).
                        self.epl_coh4_run_prompt += out;
                        if let Some(e) = out_e {
                            self.epl_coh4_run_early += e;
                        }
                        if let Some(l) = out_l {
                            self.epl_coh4_run_late += l;
                        }
                    }

                    self.epl_finalize_chip(chip_tx);
                }
            }

            let phase_step = os as i32 + gardner_adjust.integer_slew_samples;
            if phase_step >= 0 {
                self.despread_phase = (self.despread_phase + phase_step as usize) % pp;
            } else {
                self.despread_phase = (self.despread_phase + pp - (-phase_step) as usize % pp) % pp;
            }
            idx += os as f32 + gardner_adjust.step_adjust_samples;
        }

        // Carry leftover offset into the next block.
        self.next_prompt_offset = idx - len_f;
    }

    /// Called once per completed chip when EPL tracking is active. Advances
    /// the 4-chip coherent rollup, the window-chip counter, the per-log
    /// chip counter, runs the slew loop at window rollup, and emits a
    /// log line when the log interval elapses.
    fn epl_finalize_chip(&mut self, chip_tx: usize) {
        if self.epl_pilot_mode {
            // Pilot-coherent: dump every 16 chips (one Walsh 0 symbol).
            self.epl_pilot_chip_idx += 1;
            if self.epl_pilot_chip_idx >= 16 {
                let symbol_start = chip_tx.saturating_add(1).saturating_sub(16);
                let is_pilot_symbol = symbol_start % RC3_PCG_CHIPS < RC3_PILOT_CHIPS_PER_PCG;
                let prompt = self.epl_pilot_run_prompt;
                if is_pilot_symbol {
                    self.epl_window_pilot_pwr_early += self.epl_pilot_run_early.norm_sqr() as f64;
                    self.epl_window_pilot_pwr_prompt += prompt.norm_sqr() as f64;
                    self.epl_window_pilot_pwr_late += self.epl_pilot_run_late.norm_sqr() as f64;
                }

                // Pilot CFO tracking: accumulate 16-chip pilot sums over
                // 8 PCGs (12288 chips) for a high-SNR phase measurement.
                // Shorter windows (e.g. 1 PCG) widen the unambiguous range but
                // produce noise-dominated observations that degrade tracking.
                if is_pilot_symbol && let Some(ref mut cfo) = self.rc3_cfo {
                    self.rc3_cfo_pilot_accum += prompt;
                    self.rc3_cfo_pilot_chips += 16;
                    if self.rc3_cfo_pilot_chips >= 8 * RC3_PILOT_CHIPS_PER_PCG {
                        cfo.observe_pilot(self.rc3_cfo_pilot_accum, self.rc3_cfo_pilot_chips);
                        self.rc3_cfo_pilot_accum = Complex32::new(0.0, 0.0);
                        self.rc3_cfo_pilot_chips = 0;
                    }
                } else if is_pilot_symbol && prompt.norm_sqr() > 1e-12 {
                    // RC1 fallback: inline pilot CFO (kept for non-pilot
                    // modes that still want supplementary pilot CFO).
                    if let Some(prev) = self.epl_pilot_prev_prompt {
                        let cross = prompt * Complex32::new(prev.re, -prev.im);
                        let delta = cross.im.atan2(cross.re);
                        self.epl_pilot_cfo_phase_accum += delta as f64;
                        self.epl_pilot_cfo_count += 1;

                        let sub_window = if self.epl_pilot_cfo_updates < PILOT_CFO_WARMUP_UPDATES {
                            PILOT_CFO_SUB_WINDOW_SYMBOLS_WARMUP
                        } else {
                            PILOT_CFO_SUB_WINDOW_SYMBOLS
                        };
                        if self.epl_pilot_cfo_count >= sub_window {
                            let avg_delta =
                                self.epl_pilot_cfo_phase_accum / self.epl_pilot_cfo_count as f64;
                            let pilot_cfo = (avg_delta / 16.0) as f32;
                            let gain = if self.epl_pilot_cfo_updates < PILOT_CFO_WARMUP_UPDATES {
                                PILOT_CFO_GAIN_WARMUP
                            } else {
                                PILOT_CFO_GAIN
                            };
                            self.cfo_rad_per_chip =
                                (1.0 - gain) * self.cfo_rad_per_chip + gain * pilot_cfo;
                            self.epl_pilot_cfo_phase_accum = 0.0;
                            self.epl_pilot_cfo_count = 0;
                            self.epl_pilot_cfo_updates += 1;
                        }
                    }
                    self.epl_pilot_prev_prompt = Some(prompt);
                }

                self.epl_pilot_run_early = Complex32::new(0.0, 0.0);
                self.epl_pilot_run_prompt = Complex32::new(0.0, 0.0);
                self.epl_pilot_run_late = Complex32::new(0.0, 0.0);
                self.epl_pilot_chip_idx = 0;
            }
        } else {
            // Generic 4-chip coherent (RC1).
            self.epl_coh4_chip_idx += 1;
            if self.epl_coh4_chip_idx >= 4 {
                self.epl_window_coh4_pwr_early += self.epl_coh4_run_early.norm_sqr() as f64;
                self.epl_window_coh4_pwr_prompt += self.epl_coh4_run_prompt.norm_sqr() as f64;
                self.epl_window_coh4_pwr_late += self.epl_coh4_run_late.norm_sqr() as f64;
                self.epl_coh4_run_early = Complex32::new(0.0, 0.0);
                self.epl_coh4_run_prompt = Complex32::new(0.0, 0.0);
                self.epl_coh4_run_late = Complex32::new(0.0, 0.0);
                self.epl_coh4_chip_idx = 0;
            }
        }

        self.epl_chips_in_window += 1;
        self.epl_chips_since_log += 1;

        if self.epl_chips_in_window >= EPL_WINDOW_CHIPS {
            self.epl_windows_in_log += 1;

            // Run the slew loop once per window rollup (if enabled).
            if self.epl_slew_enabled {
                self.epl_run_slew_loop();
            }

            // Fold window-level env into log-level and preserve the most
            // recent completed window for logging.
            self.epl_last_window_env_early = self.epl_window_env_early;
            self.epl_last_window_env_prompt = self.epl_window_env_prompt;
            self.epl_last_window_env_late = self.epl_window_env_late;
            self.epl_env_early += self.epl_window_env_early;
            self.epl_env_prompt += self.epl_window_env_prompt;
            self.epl_env_late += self.epl_window_env_late;
            self.epl_window_env_early = 0.0;
            self.epl_window_env_prompt = 0.0;
            self.epl_window_env_late = 0.0;

            if self.epl_pilot_mode {
                // Fold window-level pilot into log-level, reset window-level.
                self.epl_last_window_pilot_pwr_early = self.epl_window_pilot_pwr_early;
                self.epl_last_window_pilot_pwr_prompt = self.epl_window_pilot_pwr_prompt;
                self.epl_last_window_pilot_pwr_late = self.epl_window_pilot_pwr_late;
                self.epl_pilot_pwr_early += self.epl_window_pilot_pwr_early;
                self.epl_pilot_pwr_prompt += self.epl_window_pilot_pwr_prompt;
                self.epl_pilot_pwr_late += self.epl_window_pilot_pwr_late;
                self.epl_window_pilot_pwr_early = 0.0;
                self.epl_window_pilot_pwr_prompt = 0.0;
                self.epl_window_pilot_pwr_late = 0.0;

                // CFO updates happen at sub-window cadence (every 32
                // pilot symbols) inside the 16-chip dump above. Reset
                // any leftover partial accumulation at window boundary
                // to prevent stale deltas from spanning windows.
                self.epl_pilot_cfo_phase_accum = 0.0;
                self.epl_pilot_cfo_count = 0;
            } else {
                // Fold window-level coh4 into log-level, reset window-level.
                self.epl_last_window_coh4_pwr_early = self.epl_window_coh4_pwr_early;
                self.epl_last_window_coh4_pwr_prompt = self.epl_window_coh4_pwr_prompt;
                self.epl_last_window_coh4_pwr_late = self.epl_window_coh4_pwr_late;
                self.epl_coh4_pwr_early += self.epl_window_coh4_pwr_early;
                self.epl_coh4_pwr_prompt += self.epl_window_coh4_pwr_prompt;
                self.epl_coh4_pwr_late += self.epl_window_coh4_pwr_late;
                self.epl_window_coh4_pwr_early = 0.0;
                self.epl_window_coh4_pwr_prompt = 0.0;
                self.epl_window_coh4_pwr_late = 0.0;
            }

            self.epl_chips_in_window = 0;
        }

        if self.epl_chips_since_log >= EPL_LOG_CHIP_INTERVAL {
            self.epl_emit_log();
        }
    }

    /// Evaluate the closed-loop EPL slew controller using the just-
    /// finished rollup window's coh4 power values. Updates the IIR,
    /// fractional accumulator, and sets `epl_slew_pending` if a ±1
    /// sub-sample slew is warranted. Applied at the start of the next
    /// despread iteration.
    fn epl_run_slew_loop(&mut self) {
        self.epl_slew_windows_total += 1;
        self.epl_slew_windows_since += 1;

        // Select the discriminator source based on mode.
        let (pe, pp, pl) = if self.epl_pilot_mode {
            (
                self.epl_window_pilot_pwr_early,
                self.epl_window_pilot_pwr_prompt,
                self.epl_window_pilot_pwr_late,
            )
        } else {
            (
                self.epl_window_coh4_pwr_early,
                self.epl_window_coh4_pwr_prompt,
                self.epl_window_coh4_pwr_late,
            )
        };
        if pp <= 1e-12 {
            return;
        }
        let disc = (pe - pl) / pp;

        // Select mode-appropriate loop constants.
        let alpha = if self.epl_pilot_mode {
            EPL_SLEW_ALPHA_PILOT
        } else {
            EPL_SLEW_ALPHA
        };
        let warmup = if self.epl_pilot_mode {
            EPL_SLEW_WARMUP_WINDOWS_PILOT
        } else {
            EPL_SLEW_WARMUP_WINDOWS
        };
        let gain = if self.epl_pilot_mode {
            EPL_SLEW_LOOP_GAIN_PILOT
        } else {
            EPL_SLEW_LOOP_GAIN
        };
        let min_between = if self.epl_pilot_mode {
            EPL_SLEW_MIN_WINDOWS_BETWEEN_PILOT
        } else {
            EPL_SLEW_MIN_WINDOWS_BETWEEN
        };

        // IIR smoothing. Positive disc ⇒ early > late ⇒ signal arrives
        // earlier than the prompt ⇒ receiver should slew BACKWARD (−1,
        // move the reference earlier in PN so the prompt reads a
        // slightly earlier sample). So the slew direction is
        // −sign(disc).
        self.epl_slew_iir = (1.0 - alpha) * self.epl_slew_iir + alpha * disc;

        // Warmup: accumulate IIR but don't integrate into frac yet.
        if self.epl_slew_windows_total < warmup {
            return;
        }

        // Dead-zone: suppress noise-driven slews near zero.
        let dead_zone = if self.epl_pilot_mode {
            EPL_SLEW_DEAD_ZONE_PILOT
        } else {
            EPL_SLEW_DEAD_ZONE
        };
        let effective = if self.epl_slew_iir.abs() < dead_zone {
            0.0
        } else {
            self.epl_slew_iir
        };

        // Integrate. Signed intentionally: forward slew should bump
        // despread_phase by +1 ⇒ PN reads shift forward in time. If
        // the signal is LATE (pl > pe ⇒ disc < 0), we want the
        // receiver to also move LATER (forward slew, +1). So the
        // direction of the integration matches −disc.
        self.epl_slew_frac += -gain * effective;

        // Rate limiter: don't fire slews closer together than the
        // min-windows-between guard.
        if self.epl_slew_windows_since < min_between {
            return;
        }

        // Check threshold: fire at most one sub-sample slew per window.
        if self.epl_slew_frac >= 1.0 {
            self.epl_slew_pending = Some(1);
            self.epl_slew_frac -= 1.0;
            self.epl_slew_total += 1;
            self.epl_slew_windows_since = 0;
        } else if self.epl_slew_frac <= -1.0 {
            self.epl_slew_pending = Some(-1);
            self.epl_slew_frac += 1.0;
            self.epl_slew_total -= 1;
            self.epl_slew_windows_since = 0;
        }
    }

    /// Emit one EPL stats line and reset the log-level accumulators.
    fn epl_emit_log(&mut self) {
        self.epl_log_seq += 1;

        let env_norm = self.epl_env_prompt.max(1e-12);
        let env_disc = (self.epl_env_early - self.epl_env_late) / env_norm;
        let env_window_norm = self.epl_last_window_env_prompt.max(1e-12);
        let env_window_disc =
            (self.epl_last_window_env_early - self.epl_last_window_env_late) / env_window_norm;

        if self.epl_pilot_mode {
            let pilot_window_norm = self.epl_last_window_pilot_pwr_prompt.max(1e-12);
            let pilot_window_disc = (self.epl_last_window_pilot_pwr_early
                - self.epl_last_window_pilot_pwr_late)
                / pilot_window_norm;

            let pilot_norm = self.epl_pilot_pwr_prompt.max(1e-12);
            let pilot_disc = (self.epl_pilot_pwr_early - self.epl_pilot_pwr_late) / pilot_norm;
            let pilot_window_ec_io_db = Self::pilot_ec_io_db_from_prompt_power(
                self.epl_last_window_pilot_pwr_prompt,
                self.epl_last_window_env_prompt,
            );
            let pilot_ec_io_db = Self::pilot_ec_io_db_from_prompt_power(
                self.epl_pilot_pwr_prompt,
                self.epl_env_prompt,
            );
            self.epl_last_log_pilot_ec_io_db = Some(pilot_ec_io_db);

            debug!(
                "EPL_TRACK[finger={}] sec#{} windows={} N={} chips={} mode=pilot | \
                 env: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 env_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 pilot_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} Ec/Io={:+.2}dB | \
                 pilot: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} Ec/Io={:+.2}dB | \
                 slew: iir={:+.4} frac={:+.4} total={} since={}",
                self.base.id,
                self.epl_log_seq,
                self.epl_windows_in_log,
                EPL_WINDOW_CHIPS,
                self.epl_chips_since_log,
                self.epl_env_early,
                self.epl_env_prompt,
                self.epl_env_late,
                env_disc,
                self.epl_last_window_env_early,
                self.epl_last_window_env_prompt,
                self.epl_last_window_env_late,
                env_window_disc,
                self.epl_last_window_pilot_pwr_early,
                self.epl_last_window_pilot_pwr_prompt,
                self.epl_last_window_pilot_pwr_late,
                pilot_window_disc,
                pilot_window_ec_io_db,
                self.epl_pilot_pwr_early,
                self.epl_pilot_pwr_prompt,
                self.epl_pilot_pwr_late,
                pilot_disc,
                pilot_ec_io_db,
                self.epl_slew_iir,
                self.epl_slew_frac,
                self.epl_slew_total,
                self.epl_slew_windows_since,
            );

            self.epl_pilot_pwr_early = 0.0;
            self.epl_pilot_pwr_prompt = 0.0;
            self.epl_pilot_pwr_late = 0.0;
        } else {
            let coh_window_norm = self.epl_last_window_coh4_pwr_prompt.max(1e-12);
            let coh_window_disc = (self.epl_last_window_coh4_pwr_early
                - self.epl_last_window_coh4_pwr_late)
                / coh_window_norm;

            let coh_norm = self.epl_coh4_pwr_prompt.max(1e-12);
            let coh_disc = (self.epl_coh4_pwr_early - self.epl_coh4_pwr_late) / coh_norm;

            debug!(
                "EPL_TRACK[finger={}] sec#{} windows={} N={} chips={} mode=coh4 | \
                 env: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 env_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 coh4_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 coh4: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 slew: iir={:+.4} frac={:+.4} total={} since={}",
                self.base.id,
                self.epl_log_seq,
                self.epl_windows_in_log,
                EPL_WINDOW_CHIPS,
                self.epl_chips_since_log,
                self.epl_env_early,
                self.epl_env_prompt,
                self.epl_env_late,
                env_disc,
                self.epl_last_window_env_early,
                self.epl_last_window_env_prompt,
                self.epl_last_window_env_late,
                env_window_disc,
                self.epl_last_window_coh4_pwr_early,
                self.epl_last_window_coh4_pwr_prompt,
                self.epl_last_window_coh4_pwr_late,
                coh_window_disc,
                self.epl_coh4_pwr_early,
                self.epl_coh4_pwr_prompt,
                self.epl_coh4_pwr_late,
                coh_disc,
                self.epl_slew_iir,
                self.epl_slew_frac,
                self.epl_slew_total,
                self.epl_slew_windows_since,
            );

            self.epl_coh4_pwr_early = 0.0;
            self.epl_coh4_pwr_prompt = 0.0;
            self.epl_coh4_pwr_late = 0.0;
        }

        // Reset shared log-level accumulators for the next log interval.
        self.epl_env_early = 0.0;
        self.epl_env_prompt = 0.0;
        self.epl_env_late = 0.0;
        self.epl_chips_since_log = 0;
        self.epl_windows_in_log = 0;
        // Keep the in-flight coherent running sums — they're partial
        // data and would be wasted if thrown away at log boundary.
        // Same for `epl_chips_in_window`: we let the next window finish
        // counting from wherever it left off.
    }

    /// Drain all available `chip_block_size`-aligned chips through the
    /// sub-chain.  CFO correction and pilot estimation are still performed
    /// in `chip_block_size` windows for tracking accuracy, but all corrected
    /// samples are batched into a single large block before being pushed
    /// through the sub-chain once.
    fn drain_to_chain(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        let output_oversample = if self.output_oversampled_chips {
            self.oversample.max(1)
        } else {
            1
        };
        let block_len_chips = self.chip_block_size;
        let block_len = block_len_chips * output_oversample;
        let n_blocks = self.sample_buffer.len() / block_len;
        if n_blocks == 0 {
            return out;
        }

        let total_len = n_blocks * block_len;
        let first_abs_chip = self.chain_start_chip + self.chain_chips_output;
        let mut all_samples = Vec::with_capacity(total_len);

        // Process in chip_block_size windows for CFO correction + pilot
        // estimation, but collect all corrected samples into one buffer.
        for _ in 0..n_blocks {
            let raw: Vec<Complex32> = self.sample_buffer.drain(..block_len).collect();

            let window_start = all_samples.len();
            if let Some(ref cfo) = self.rc3_cfo {
                // RC3 traffic: chips already derotated inline.
                all_samples.extend(raw);
                self.cfo_rad_per_chip = cfo.cfo_rad_per_chip();
                self.cfo_phase = cfo.cfo_phase();
            } else if let Some(ref mut cfo) = self.access_cfo {
                // Access: batch derotation using the tracker's CFO.
                let mut buf = raw;
                cfo.derotate_chips(&mut buf, output_oversample);
                all_samples.extend_from_slice(&buf);
                self.cfo_rad_per_chip = cfo.cfo_rad_per_chip();
                self.cfo_phase = cfo.cfo_phase();
            } else {
                // RC1: inline derotation using the legacy CFO state.
                let cfo_step = self.cfo_rad_per_chip / output_oversample as f32;
                for &s in &raw {
                    let (sin_p, cos_p) = self.cfo_phase.sin_cos();
                    all_samples.push(Complex32::new(
                        s.re * cos_p - s.im * sin_p,
                        s.re * sin_p + s.im * cos_p,
                    ));
                    self.cfo_phase += cfo_step;
                }
                self.cfo_phase %= 2.0 * std::f32::consts::PI;
            }

            // Pilot CFO estimation on the window just produced.
            let window = &all_samples[window_start..];
            let prompt_phase = self.center_offset.min(output_oversample - 1);
            let prompt_samples: Vec<Complex32> = if output_oversample > 1 {
                window
                    .chunks_exact(output_oversample)
                    .map(|chip| chip[prompt_phase])
                    .collect()
            } else {
                window.to_vec()
            };
            let pilot: Complex32 = prompt_samples.iter().copied().sum();
            let incoherent_energy: f32 = prompt_samples.iter().map(|s| s.norm()).sum();
            let incoherent_power: f32 = prompt_samples.iter().map(|s| s.norm_sqr()).sum();
            // Track per-chip energy for signal-loss detection.
            //
            // The peak is a decaying envelope, NOT a monotonic maximum: a
            // brief RF burst (e.g. TX→RX leakage at a PAPR peak, or a
            // transient interferer) can easily drive `per_chip_energy` two
            // orders of magnitude above the normal steady-state level, and
            // a monotonic max would latch at the burst level forever —
            // after which the 10%-of-peak "loss" threshold would become
            // higher than the real signal and every subsequent chip block
            // would be classified as "signal lost". The exponential decay
            // factor below lets the peak track upward instantly (via the
            // `max`) but fall back toward steady-state after a spike with
            // a ~1 s time constant at the usual block rate (~1250 blk/s
            // at 1.2288 Mcps, 1024-chip blocks). The loss threshold then
            // floats with the actual signal envelope instead of the
            // worst-ever sample.
            let per_chip_energy = incoherent_energy / block_len_chips as f32;
            const PEAK_DECAY_PER_BLOCK: f32 = 0.995;
            self.peak_energy = (self.peak_energy * PEAK_DECAY_PER_BLOCK).max(per_chip_energy);
            // Signal is "lost" when energy drops below 10% of peak.
            let loss_threshold = self.peak_energy * 0.10;
            if self.peak_energy > 1e-9 && per_chip_energy < loss_threshold {
                self.low_energy_chip_count += block_len as u64;
            } else {
                self.low_energy_chip_count = 0;
                self.high_energy_chip_count += block_len as u64;
            }

            // Periodic finger energy diagnostic (every 10000 symbols ≈ every ~34 seconds)
            let chips_out = self.chain_chips_output;
            let sym_idx = chips_out / 256;
            if sym_idx % 10000 == 0 && sym_idx > 0 {
                let raw_pwr = if self.raw_input_power_count > 0 {
                    self.raw_input_power_accum / self.raw_input_power_count as f64
                } else {
                    0.0
                };
                let pcn = if incoherent_energy > 1e-9 {
                    pilot.norm() / incoherent_energy
                } else {
                    0.0
                };
                let pilot_ec_io_db = Self::pilot_ec_io_db_from_prompt_power(
                    pilot.norm_sqr() as f64,
                    incoherent_power as f64,
                );
                // CFO residual stats from the tracker (or legacy counters).
                let (cfo_res_rms_hz, cfo_res_mean_hz, cfo_res_max_hz, cfo_res_n) =
                    if let Some(ref mut cfo) = self.rc3_cfo {
                        let s = cfo.take_residual_stats();
                        (s.rms_hz, s.mean_hz, s.max_hz, s.count)
                    } else if self.cfo_residual_count > 0 {
                        let n = self.cfo_residual_count as f64;
                        let mean = self.cfo_residual_abs_sum / n;
                        let rms = (self.cfo_residual_sq_sum / n).sqrt();
                        let max = self.cfo_residual_max as f64;
                        self.cfo_residual_abs_sum = 0.0;
                        self.cfo_residual_sq_sum = 0.0;
                        self.cfo_residual_max = 0.0;
                        self.cfo_residual_count = 0;
                        (rms, mean, max, self.cfo_residual_count)
                    } else {
                        (0.0, 0.0, 0.0, 0)
                    };
                log::debug!(
                    "FINGER DIAG sym={}: per_chip_energy={:.6} peak={:.6} pilot_coh={:.4} pilot_ec_io={:.2}dB raw_input_pwr={:.6} cfo={:.6} cfo_residual_rms={:.1}Hz cfo_residual_mean={:.1}Hz cfo_residual_max={:.1}Hz cfo_residual_n={}",
                    sym_idx,
                    per_chip_energy,
                    self.peak_energy,
                    pcn,
                    pilot_ec_io_db,
                    raw_pwr,
                    self.cfo_rad_per_chip,
                    cfo_res_rms_hz,
                    cfo_res_mean_hz,
                    cfo_res_max_hz,
                    cfo_res_n,
                );
            }

            let pilot_coh_norm = if incoherent_energy > 1e-9 {
                pilot.norm() / incoherent_energy
            } else {
                0.0
            };
            // CFO tracking uses a two-part policy:
            //
            // 1. Hard gate at 0.05 — blocks below this are noise-
            //    dominated (theoretical noise floor at 1024-chip blocks
            //    is 1/sqrt(1024) ≈ 0.031), so updating from them only
            //    injects random phase into the CFO state. We skip the
            //    update entirely but KEEP prev_pilot so a short dip
            //    doesn't force a bootstrap-restart when coherence
            //    recovers.
            //
            // 2. Magnitude-weighted loop gain above the gate — a fixed
            //    0.1 gain treats a pilot_coh of 0.06 the same as 0.30,
            //    so a weak noisy block moves the CFO as much as a
            //    strong clean one. We scale the gain by pilot_coh_norm
            //    normalized to 0.25, clamped to 1.0: a 0.25+ block
            //    updates at the full 0.1 rate, a 0.10 block at 0.04,
            //    a 0.05 block at 0.02. This lets weak-but-signal blocks
            //    contribute proportionally instead of either being
            //    rejected entirely (old 0.25 hard gate — caused 42 s
            //    lock-loss runaway when chip timing drift dropped coh
            //    below 0.25 permanently) or dominating the estimate
            //    with noise (previous 0.05 gate with fixed 0.1 gain —
            //    CFO swung ±0.004 rad/chip and calls died in ~13 s).
            // CFO tracking dispatch:
            // - RC3 traffic: fed from EPL pilot path (epl_finalize_chip)
            // - Access: fed here from 256-chip block sums, coherence-gated
            // - RC1: legacy 256-chip tracker
            if let Some(ref mut cfo) = self.access_cfo {
                const ACCESS_CFO_COH_GATE: f32 = 0.12;
                if pilot_coh_norm >= ACCESS_CFO_COH_GATE {
                    cfo.observe_pilot(pilot, block_len_chips);
                }
            } else if self.rc3_cfo.is_none() {
                let cfo_gate = 0.05f32;
                const CFO_COH_REFERENCE: f32 = 0.25;
                const CFO_MAX_LOOP_GAIN: f32 = 0.1;
                if pilot_coh_norm >= cfo_gate {
                    if let Some(prev) = self.prev_pilot {
                        let cross = pilot * Complex32::new(prev.re, -prev.im);
                        let delta = cross.im.atan2(cross.re);
                        let update = delta / self.chip_block_size as f32;
                        let trust = (pilot_coh_norm / CFO_COH_REFERENCE).min(1.0);
                        let loop_gain = CFO_MAX_LOOP_GAIN * trust;
                        self.cfo_rad_per_chip =
                            (1.0 - loop_gain) * self.cfo_rad_per_chip + loop_gain * update;
                    }
                    self.prev_pilot = Some(pilot);
                }
                // else: intentionally keep prev_pilot — do not clear.
            }

            self.chain_chips_output += block_len_chips;
        }

        let output_sample_rate_hz = if self.oversample > 0 {
            (self.sample_rate_hz / self.oversample as f64) * output_oversample as f64
        } else {
            self.sample_rate_hz
        };

        // Compute signal power before all_samples is moved into SampleBlock.
        // The despread samples have been multiplied by the PN conjugate reference
        // whose chips are (±1, ±1) with |pn|² = 2, so divide out that gain to
        // report power relative to the original input (0 dB = full-scale ±1.0).
        let signal_power = if all_samples.is_empty() {
            0.0
        } else {
            let raw_power =
                all_samples.iter().map(|s| s.norm_sqr()).sum::<f32>() / all_samples.len() as f32;
            raw_power / 2.0
        };
        let pilot_ec_io_db = self.epl_last_log_pilot_ec_io_db;

        let mut blk = SampleBlock::new(all_samples, first_abs_chip)
            .with_sample_rate_hz(output_sample_rate_hz);
        // pilot_phase is exported at chip granularity (not sample-rate).
        // `acquisition_phase` is the sample-rate cursor at the time the
        // finger was created; dividing by `oversample` yields the chip
        // index that downstream consumers expect.
        blk.tags.insert(
            "pilot_phase",
            (self.acquisition_phase / self.oversample) as i64,
        );
        blk.tags
            .insert("absolute_chip_start", first_abs_chip as i64);
        blk.tags.insert("finger_id", self.base.id as i64);
        blk.tags
            .insert("access_oversample", output_oversample as i64);

        // Signal quality tags for per-mobile reporting
        let snr_db = 10.0 * self.detection_snr.max(1e-9_f32).log10();
        blk.tags.insert("finger_snr_mdb", (snr_db * 1000.0) as i64);
        let signal_power_db = 10.0 * signal_power.max(1e-15_f32).log10();
        blk.tags
            .insert("finger_signal_power_mdb", (signal_power_db * 1000.0) as i64);
        // Export only the completed 1-second smoothed pilot Ec/Io. Before
        // the first EPL interval completes, leave the field absent so UI
        // and snapshots do not show a raw instantaneous value under the
        // smoothed label.
        if let Some(pilot_ec_io_db) = pilot_ec_io_db {
            blk.tags
                .insert("finger_pilot_ec_io_mdb", (pilot_ec_io_db * 1000.0) as i64);
        }
        // Raw input power (pre-despread, post matched filter) for Rx Level
        // reporting. Subtract the matched-filter passband power gain so the
        // reported value is referenced back to the ADC input (dBFS), not to
        // the matched-filter output.
        if self.raw_input_power_count > 0 {
            let raw_mean = (self.raw_input_power_accum / self.raw_input_power_count as f64) as f32;
            let raw_power_db =
                10.0 * raw_mean.max(1e-15_f32).log10() - rx_matched_filter_power_gain_db();
            blk.tags
                .insert("finger_raw_power_mdb", (raw_power_db * 1000.0) as i64);
        }
        // CFO tracker output (radians per chip). Tag in micro-radians so the
        // i64 tag map preserves enough precision for sub-Hz resolution at the
        // 1.2288 Mchip/s reverse rate.
        blk.tags.insert(
            "finger_cfo_urad_per_chip",
            (self.cfo_rad_per_chip as f64 * 1_000_000.0) as i64,
        );
        if let Some(gardner) = &self.gardner_timing {
            blk.tags.insert(
                "finger_gardner_offset_milli_samples",
                (gardner.offset_samples() * 1000.0) as i64,
            );
            blk.tags.insert(
                "finger_gardner_error_milli",
                (gardner.last_error() * 1000.0) as i64,
            );
            blk.tags
                .insert("finger_gardner_updates", gardner.updates() as i64);
            blk.tags
                .insert("finger_gardner_skipped", gardner.skipped() as i64);
            blk.tags.insert(
                "finger_gardner_update_interval_chips",
                gardner.update_interval_chips() as i64,
            );
        }

        // Diagnostic: verify sample magnitudes at finger output
        let sym_out = self.chain_chips_output / 256;
        if sym_out >= 748 && sym_out <= 760 && blk.samples.len() >= 4 {
            let s0 = blk.samples[0];
            let s1 = blk.samples[1];
            let avg = blk
                .samples
                .iter()
                .map(|s| (s.re * s.re + s.im * s.im).sqrt())
                .sum::<f32>()
                / blk.samples.len() as f32;
            log::trace!(
                "FINGER OUTPUT sym={}: n_samples={} avg_mag={:.5} s[0]=({:.5},{:.5}) s[1]=({:.5},{:.5})",
                sym_out,
                blk.samples.len(),
                avg,
                s0.re,
                s0.im,
                s1.re,
                s1.im,
            );
        }

        // Drive through the sub-chain once with the combined block.
        // Processors may emit latency-critical blocks (e.g. PCG
        // measurements) via the emitter; these are collected and
        // appended to the final output so the GenericRakeReceiver
        // can forward them to the real emitter.
        let mut emitter = crate::receiver::pipelined::VecEmitter::new();
        let mut chain_blocks = vec![blk];
        let mut progress = FingerProgress::default();
        for (si, proc) in chain.iter_mut().enumerate() {
            let t = std::time::Instant::now();
            let mut next = Vec::new();
            for b in chain_blocks {
                next.extend(proc.process_block_emitting(b, &mut emitter));
            }
            progress.observe_blocks(&next);
            let ns = t.elapsed().as_nanos() as u64;
            if self.sub_chain_ns.len() <= si {
                self.sub_chain_ns.resize(si + 1, (0u64, ""));
                self.sub_chain_ns[si].1 = proc.name();
            }
            self.sub_chain_ns[si].0 += ns;
            chain_blocks = next;
        }
        chain_blocks.extend(emitter.blocks);

        self.base
            .tick_with_progress(&progress, (n_blocks as u64) * (self.chip_block_size as u64));

        out.extend(chain_blocks);
        out
    }

    pub(super) fn replay_block(
        &mut self,
        block: &SampleBlock,
        chain: &mut Vec<PipelineProcessorShared>,
    ) {
        self.sample_rate_hz = block.sample_rate_hz;
        self.despread_block(&block.samples);
        let out = self.drain_to_chain(chain);
        self.pending_output.extend(out);
    }
}

impl RakeFinger for PnLcFinger {
    fn id(&self) -> u64 {
        self.base.id
    }

    fn spawn_chip_start(&self) -> Option<u64> {
        Some(self.chain_start_chip as u64)
    }

    fn describe(&self) -> String {
        format!(
            "snr={:.1}x despread_phase={} next_prompt_offset={:.3} timing_mu={:+.3} poly_os={} iad={} chain_start_chip={} \
             lc_chip_counter={} cfo={:.9} gardner={}",
            self.detection_snr,
            self.despread_phase,
            self.next_prompt_offset,
            self.timing_mu_samples,
            if self.output_oversampled_chips {
                self.oversample
            } else {
                1
            },
            self.integrate_and_dump,
            self.chain_start_chip,
            self.lc_chip_counter,
            self.cfo_rad_per_chip,
            self.gardner_timing.is_some(),
        )
    }

    fn process(
        &mut self,
        block: &SampleBlock,
        chain: &mut Vec<PipelineProcessorShared>,
    ) -> Vec<SampleBlock> {
        let mut out = std::mem::take(&mut self.pending_output);
        self.sample_rate_hz = block.sample_rate_hz;

        let t0 = std::time::Instant::now();
        self.despread_block(&block.samples);
        let despread_ns = t0.elapsed().as_nanos() as u64;
        out.extend(std::mem::take(&mut self.pending_output));

        let t1 = std::time::Instant::now();
        let live = self.drain_to_chain(chain);
        let drain_ns = t1.elapsed().as_nanos() as u64;

        self.despread_ns += despread_ns;
        self.drain_ns += drain_ns;
        self.finger_block_count += 1;

        if self.finger_block_count % 500 == 0 {
            let d_ms = self.despread_ns as f64 / 1e6;
            let c_ms = self.drain_ns as f64 / 1e6;
            let total = d_ms + c_ms;
            debug!(
                "  [finger {} blk={}] despread: {:.1}ms ({:.1}%) | sub-chain: {:.1}ms ({:.1}%)",
                self.base.id,
                self.finger_block_count,
                d_ms,
                if total > 0.0 {
                    d_ms / total * 100.0
                } else {
                    0.0
                },
                c_ms,
                if total > 0.0 {
                    c_ms / total * 100.0
                } else {
                    0.0
                },
            );
        }

        out.extend(live);
        out.extend(std::mem::take(&mut self.pending_output));
        out
    }

    fn flush(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        let mut out = std::mem::take(&mut self.pending_output);
        out.extend(BaseFinger::flush_chain(chain));
        out
    }

    fn is_hard_validated(&self) -> bool {
        self.base.is_hard_validated()
    }

    fn idle_blocks(&self) -> u64 {
        self.base.idle_blocks()
    }

    fn idle_chips(&self) -> u64 {
        self.base.idle_chips()
    }

    fn crc_miss_count(&self) -> u64 {
        self.base.crc_miss_count()
    }

    fn post_walsh_no_event_chips(&self) -> u64 {
        self.base.post_walsh_no_event_chips()
    }

    fn post_walsh_miss_count(&self) -> u64 {
        self.base.post_walsh_miss_count()
    }

    fn post_walsh_no_event_ms(&self) -> u64 {
        self.base.post_walsh_no_event_ms()
    }

    fn signal_lost_chips(&self) -> u64 {
        self.low_energy_chip_count
    }

    fn print_timing(&self) {
        let d_ms = self.despread_ns as f64 / 1e6;
        let c_ms = self.drain_ns as f64 / 1e6;
        let total = d_ms + c_ms;
        let high_e_syms = self.high_energy_chip_count / 256;
        debug!(
            "    finger {} breakdown ({} blocks, high_energy_chips={} ~{} syms): despread {:.1}ms ({:.1}%) | sub-chain {:.1}ms ({:.1}%)",
            self.base.id,
            self.finger_block_count,
            self.high_energy_chip_count,
            high_e_syms,
            d_ms,
            if total > 0.0 {
                d_ms / total * 100.0
            } else {
                0.0
            },
            c_ms,
            if total > 0.0 {
                c_ms / total * 100.0
            } else {
                0.0
            },
        );
        if !self.sub_chain_ns.is_empty() {
            let sc_total: f64 = self.sub_chain_ns.iter().map(|(ns, _)| *ns as f64).sum();
            for (si, (ns, name)) in self.sub_chain_ns.iter().enumerate() {
                let ms = *ns as f64 / 1e6;
                let pct = if sc_total > 0.0 {
                    *ns as f64 / sc_total * 100.0
                } else {
                    0.0
                };
                debug!("      [{si}] {name:<50} {ms:>8.1}ms  {pct:>5.1}%");
            }
        }
    }

    fn timing_report_lines(&self) -> Vec<String> {
        let d_ms = self.despread_ns as f64 / 1e6;
        let c_ms = self.drain_ns as f64 / 1e6;
        let total = d_ms + c_ms;
        let high_e_syms = self.high_energy_chip_count / 256;
        let mut lines = vec![format!(
            "  finger {} breakdown ({} blocks, high_energy_chips={} ~{} syms): despread {:.1}ms ({:.1}%) | sub-chain {:.1}ms ({:.1}%)",
            self.base.id,
            self.finger_block_count,
            self.high_energy_chip_count,
            high_e_syms,
            d_ms,
            if total > 0.0 {
                d_ms / total * 100.0
            } else {
                0.0
            },
            c_ms,
            if total > 0.0 {
                c_ms / total * 100.0
            } else {
                0.0
            },
        )];
        if !self.sub_chain_ns.is_empty() {
            let sc_total: f64 = self.sub_chain_ns.iter().map(|(ns, _)| *ns as f64).sum();
            for (si, (ns, name)) in self.sub_chain_ns.iter().enumerate() {
                let ms = *ns as f64 / 1e6;
                let pct = if sc_total > 0.0 {
                    *ns as f64 / sc_total * 100.0
                } else {
                    0.0
                };
                lines.push(format!("    [{si}] {name:<50} {ms:>8.1}ms  {pct:>5.1}%"));
            }
        }
        lines
    }
}
#[cfg(test)]
impl PnLcFinger {
    /// Expose the chip-rate buffer for test inspection.
    pub(super) fn chip_buffer_as_slice(&self) -> Vec<Complex32> {
        self.sample_buffer.iter().cloned().collect()
    }
}
