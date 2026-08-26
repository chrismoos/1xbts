use std::collections::VecDeque;
use std::sync::Arc;

use log::{info, trace, warn};
use num_complex::Complex32;

use super::super::cfo_tracker;
use super::super::gardner_timing_recovery::{
    GardnerTimingAdjustment, GardnerTimingConfig, GardnerTimingRecovery,
};
use super::super::generic_rake_receiver::{BaseFinger, FingerProgress, RakeFinger};
use super::interpolation::{interp_complex_clamped, interp_complex_contiguous};
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::walsh::WalshGenerator;
use crate::receiver::pipelined::{PipelineProcessorShared, SampleBlock};

// With Gardner enabled we keep only one neighbor beside the verified prompt to
// avoid doubling slow false-finger work. Captures with very strong correlation
// peaks have consistently benefited from the late-side prompt, while marginal
// captures need the early side that preserved the existing v60s decode count.
pub(super) const ADAPTIVE_FINGER_TIMING_LATE_SNR_THRESHOLD: f32 = 200.0;

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

    /// PN/LC sample-rate cursor for the next prompt. Advances by `oversample`
    /// per chip and is independent of received-sample timing corrections.
    pub(crate) despread_phase: usize,
    /// Initial `despread_phase`, used only for the output `pilot_phase` tag.
    acquisition_phase: usize,
    /// Received-sample offset of the next prompt; residual offsets carry
    /// across input blocks.
    pub(crate) next_prompt_offset: f32,
    /// Last sample from the preceding input block. Fractional timing can move
    /// the next prompt slightly before the current block boundary.
    previous_input_sample: Option<Complex32>,
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
    /// Optional explicit Q LC generator for HRPD-style HPSK where UQ has its
    /// own mask. When absent, legacy RC3 behavior derives Q from delayed I.
    q_lc_gen: Option<LongCodeGenerator>,
    /// Optional chip period at which LC generators are reloaded.
    lc_period_chips: Option<usize>,
    lc_period_initial_state: u64,
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
    /// True for legacy/conjugated HPSK sample convention (`I-jQ`), false for
    /// ordinary IQ samples (`I+jQ`) as used by HRPD reverse access captures.
    hpsk_signal_conjugated: bool,

    // CFO tracking
    cfo_rad_per_chip: f32,
    cfo_phase: f32,
    /// RC3 pilot CFO tracker.  When present, this is the sole CFO source;
    /// the legacy 256-chip tracker is disabled and `cfo_rad_per_chip` /
    /// `cfo_phase` are synced from the tracker for diagnostic use only.
    rc3_cfo: Option<cfo_tracker::CfoTracker>,
    /// Whether eighth-rate reverse pilot gating is active for this bearer.
    rc3_pilot_gating_mode: bool,
    /// Reverse access CFO tracker (256-chip Walsh-symbol observations,
    /// coherence-gated coasting during data).
    access_cfo: Option<cfo_tracker::CfoTracker>,
    /// RC1 carrier tracker driven by decision-directed 64-ary Walsh symbols.
    rc1_cfo: Option<cfo_tracker::CfoTracker>,
    rc1_cfo_walsh_chips: [Complex32; 64],
    rc1_cfo_walsh_chip_sum: Complex32,
    rc1_cfo_pn_chip_count: usize,
    rc1_cfo_walsh_chip_count: usize,
    rc1_cfo_prev_symbol: Option<Complex32>,
    rc1_cfo_cross_sum: Complex32,
    rc1_cfo_cross_magnitude_sum: f32,
    rc1_cfo_cross_count: usize,
    rc1_cfo_last_coherence: f32,
    rc1_cfo_accepted_windows: u64,
    rc1_cfo_rejected_windows: u64,
    /// CFO pilot observation: accumulate ONLY 16-chip EPL pilot sums
    /// (Walsh-0 coherent) for feeding to the CfoTracker. Completely
    /// separate from the diagnostic accumulators.
    rc3_cfo_pilot_accum: Complex32,
    rc3_cfo_pilot_chips: usize,
    /// Absolute PCG owning the current pilot accumulator.  Keeping this
    /// explicit prevents a finger that starts mid-PCG from combining pieces
    /// of two different PCGs into one phase vector.
    rc3_cfo_pilot_abs_pcg: Option<usize>,
    /// Previous complete-PCG pilot vector used for the short-baseline CFO
    /// phase difference.
    rc3_cfo_prev_pcg_pilot: Option<Complex32>,
    /// Circular average of adjacent-PCG pilot cross-products. Eight terms
    /// provide steady-state noise averaging without reducing capture range.
    rc3_cfo_pcg_cross_accum: Complex32,
    rc3_cfo_pcg_cross_count: usize,

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
    rc3_pcg_measurement_raw_input_power: f64,
    rc3_pcg_measurement_prompt_chip_power: f64,
    rc3_pcg_measurement_pilot_run_prompt: Complex32,
    rc3_pcg_measurement_pilot_prompt_power: f64,
    rc3_pcg_measurement_pilot_chip_idx: usize,
    rc3_pcg_measurement_traffic_run_prompt: Complex32,
    rc3_pcg_measurement_traffic_prompt_power: f64,
    rc3_pcg_measurement_traffic_chip_idx: usize,
    rc3_pcg_measurement_chip_count: usize,
    /// Coherent sum of 16-chip pilot symbols within the current PCG.
    rc3_pcg_measurement_pilot_coherent_sum: Complex32,
    /// Number of 16-chip pilot symbols accumulated in the current PCG.
    rc3_pcg_measurement_pilot_symbol_count: usize,
    /// Per-PCG power moments over eight PCGs. Pilot SINR and the mobile-power
    /// metric both come from this window. One PCG is too noisy to drive PCBs.
    rc3_pcg_measurement_smoothing_window: VecDeque<(f64, f64, usize, f64, usize)>,

    // Raw (pre-despread) input power accumulator for Rx Power reporting
    raw_input_power_accum: f64,
    raw_input_power_count: u64,

    // Timing instrumentation
    despread_ns: u64,
    drain_ns: u64,
    finger_block_count: u64,
    /// Per sub-chain stage: (accumulated_ns, name)
    sub_chain_ns: Vec<(u64, &'static str)>,

    // ----- Early/prompt/late chip-timing tracking -----
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
    // Delay-power profile: where the correlation peak sits, so the prompt can
    // follow it.
    /// Per-hypothesis running sum over the 4 PN chips of one Walsh chip.
    delay_profile_run: [Complex32; DELAY_PROFILE_TAPS],
    /// Per-hypothesis buffer of the 64 Walsh chips of one symbol.
    delay_profile_walsh: [[Complex32; RC1_WALSH_CHIPS]; DELAY_PROFILE_TAPS],
    /// Per-hypothesis coherent power for the PCG under judgement.
    delay_profile_pcg: [f64; DELAY_PROFILE_TAPS],
    /// Closed-loop delay tracking: repositions the prompt onto the delay that
    /// actually carries the correlation peak.
    delay_track_enabled: bool,
    /// Per-hypothesis coherent power for the current tracking decision.
    delay_track_acc: [f64; DELAY_PROFILE_TAPS],
    /// Transmitted PCGs accumulated into `delay_track_acc` so far.
    delay_track_pcgs: usize,
    /// Whole chips the prompt has been moved over this finger's life.
    delay_track_total_chips: isize,
    /// Prompt shift the tracker has asked for, owned separately from the EPL
    /// slew so the two actuators cannot silently overwrite each other.
    delay_slew_pending: Option<f32>,
    /// Admitted PCGs since the last reposition, for the settling interval.
    delay_track_pcgs_since_move: usize,
    /// Frame the PCG quota is being counted against, and how many PCGs of that
    /// frame have already been fed to the profile.
    delay_track_frame: usize,
    delay_track_frame_pcgs: usize,
    /// Whether the PCG in progress has been counted against the frame quota.
    delay_track_pcg_counted: bool,
    /// Prompt envelope power and chip count for the PCG the delay tracker is
    /// judging, plus its own idle-floor estimate. Kept separate from the EPL
    /// accumulators so tracking does not depend on that instrumentation.
    delay_gate_env_prompt: f64,
    delay_gate_chips: usize,
    delay_gate_floor: f64,
    delay_gate_on_pcgs: u64,
    delay_gate_off_pcgs: u64,
    /// Fast envelope estimate used to skip profile correlation while the mobile
    /// is silent. A gated channel transmits a small fraction of its chips, and
    /// correlating 9 hypotheses through the rest is pure waste.
    delay_burst_env: f64,
    /// Chips of the symbol under construction that were inside a burst. Only a
    /// symbol wholly inside one contributes, so a burst edge cannot enter a
    /// partially-filled symbol whose peak would be noise.
    delay_symbol_on_chips: usize,
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
    /// Pilot or frame-integrated RC1 timing discriminator IIR.
    epl_slew_iir: f64,
    /// Initial RC1 discriminator bias at the acquired path timing.
    epl_rc1_bias: Option<f64>,
    /// RC1 E/P/L power accumulated across one exact 20 ms traffic frame.
    epl_rc1_frame_early: f64,
    epl_rc1_frame_prompt: f64,
    epl_rc1_frame_late: f64,
    epl_rc1_frame_windows: u8,
    /// Fractional timing-error accumulator.
    epl_slew_frac: f64,
    /// Lifetime received-sample correction (forward - backward).
    epl_slew_total: f64,
    /// Windows since the last slew fired (rate limiter).
    epl_slew_windows_since: u64,
    /// Total windows processed since finger validation (warmup guard).
    epl_slew_windows_total: u64,
    /// Pending slew applied at the start of the next despread loop
    /// iteration. `None` = no slew pending. Signed to allow forward
    /// (positive) and backward (negative).
    epl_slew_pending: Option<f32>,
    epl_nonpilot_phy_frames: u64,
    epl_nonpilot_phy_invalid: u64,
    epl_nonpilot_fer_bits: u64,
    epl_nonpilot_fer_frames: u8,
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

const DELAY_GATE_PCG_CHIPS: usize = 1536;

/// Wide enough to see a delay that has walked whole chips. An early/late gate
/// spans a quarter chip and reads zero either side of that.
const DELAY_PROFILE_HALF_CHIPS: usize = 4;

const DELAY_PROFILE_TAPS: usize = 2 * DELAY_PROFILE_HALF_CHIPS + 1;

/// Each hypothesis demodulates the way the decoder does. A 4-chip coherent
/// window has almost no contrast at RC1 chip-level Ec/Nt.
const RC1_WALSH_CHIPS: usize = 64;
const RC1_PN_CHIPS_PER_WALSH_CHIP: usize = 4;
const RC1_SYMBOL_CHIPS: usize = RC1_WALSH_CHIPS * RC1_PN_CHIPS_PER_WALSH_CHIP;

/// Decaying average rather than accumulate-and-reset, which would keep deciding
/// on pre-move data for the rest of each block.
const DELAY_TRACK_ALPHA: f64 = 1.0 / 8.0;

/// A move renames every hypothesis, so the average has to refill before the
/// profile is trusted again.
const DELAY_TRACK_WARMUP_PCGS: usize = 8;

/// 2 dB. A whole-chip reposition onto noise is destructive.
const DELAY_TRACK_MARGIN: f64 = 1.6;

/// Capping the profile at two PCGs a frame pins tracking cost at its 1/8-rate
/// value instead of letting it swing with the traffic rate: +5% of receive time
/// against +25% ungated.
const DELAY_TRACK_PCGS_PER_FRAME: usize = 2;

const DELAY_TRACK_PCGS_PER_FRAME_TOTAL: usize = 16;

/// The argmax oscillates while two paths sit within a decibel of each other, so
/// a move is followed by a settling period rather than chasing the flicker.
const DELAY_TRACK_MIN_PCGS_BETWEEN_MOVES: usize = 16;

/// Past this the finger has lost what it acquired and should be pruned rather
/// than slewed further. Tracking follows a path, it does not hunt for one.
const DELAY_TRACK_MAX_TOTAL_CHIPS: isize = 24;

/// A burst runs 1536 chips, so a 64-chip time constant settles well inside one.
const DELAY_BURST_ENV_ALPHA: f64 = 1.0 / 64.0;

/// 6 dB above the idle floor counts as transmitting, against 9-19 dB of
/// contrast between a transmitted PCG and a gated-off one.
const DELAY_GATE_ON_FACTOR: f64 = 4.0;

/// Small enough that a burst barely moves the idle floor, large enough to track
/// a genuine noise-floor rise.
const DELAY_GATE_FLOOR_LEAK: f64 = 0.001;

/// Number of chips accumulated into one EPL rollup window.
/// At 4096 chips × ~814 ns/chip ≈ 3.3 ms = 16 Walsh symbols worth.
const EPL_WINDOW_CHIPS: usize = 4096;

/// Number of chips between log lines, independent of window size.
/// 1.2288 M chips ≈ 1 s of signal time at the nominal chip rate.
const EPL_LOG_CHIP_INTERVAL: u64 = 1_228_800;

const EPL_SLEW_WARMUP_WINDOWS: u64 = 3000;

const EPL_SLEW_DEAD_ZONE: f64 = 0.015;

const EPL_SLEW_ALPHA: f64 = 0.02;

/// RC1 loop gain per 20 ms frame discriminator.
const EPL_SLEW_LOOP_GAIN: f64 = 0.20;

const EPL_RC1_SLEW_TRIGGER: f64 = 1.0;

/// Pilot-mode warmup: longer because the 16-chip pilot discriminator
/// has higher per-window variance from multipath asymmetry.
const EPL_SLEW_WARMUP_WINDOWS_PILOT: u64 = 500;

/// Pilot-mode dead-zone: wider because the 16-chip pilot discriminator
/// IIR settles to ±0.15–0.25 even when well-aligned, due to multipath
/// asymmetry. Real clock drift pushes well past 0.25.
const EPL_SLEW_DEAD_ZONE_PILOT: f64 = 0.25;

/// Pilot-mode IIR alpha: more smoothing to dampen per-window variance.
const EPL_SLEW_ALPHA_PILOT: f64 = 0.05;

/// Pilot-mode loop gain: lower to prevent transient spikes from
/// accumulating enough to fire.
const EPL_SLEW_LOOP_GAIN_PILOT: f64 = 0.01;

/// Pilot-mode min-between: wider spacing since clock drift is slow
/// (ppm-level) and the pilot discriminator is noisier per-window.
const EPL_SLEW_MIN_WINDOWS_BETWEEN_PILOT: u64 = 300;

const EPL_SLEW_MIN_WINDOWS_BETWEEN: u64 = 900;

/// EPL actuator step in ADC samples. At the normal 4× chip-rate input this is
/// 1/16 chip. The prompt interpolator supports fractional sample positions, so
/// using its resolution avoids a decode-disrupting whole-sample timing jump.
const EPL_SLEW_STEP_SAMPLES: f32 = 0.25;

/// Six 4096-chip windows are exactly one 20 ms RC1 traffic frame.
const EPL_RC1_WINDOWS_PER_FRAME: u8 = 6;

const EPL_NONPILOT_FER_WINDOW_FRAMES: u8 = 50;
const EPL_NONPILOT_FER_WINDOW_MASK: u64 = (1u64 << EPL_NONPILOT_FER_WINDOW_FRAMES) - 1;
const EPL_RC1_MAX_INVALID_FRAMES_FOR_SLEW: u32 = 5;

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
    #[cfg(test)]
    pub(crate) fn test_tick_and_validate(&mut self, output: &[SampleBlock], processed_chips: u64) {
        self.base.tick_and_validate(output, processed_chips);
    }

    pub(crate) fn set_delay_tracking_enabled(&mut self, enable: bool) {
        self.delay_track_enabled = enable;
    }

    pub(crate) fn set_nonpilot_cfo_tracking(&mut self, enable: bool) {
        if self.rc3_cfo.is_none() && self.access_cfo.is_none() {
            self.rc1_cfo =
                enable.then(|| cfo_tracker::CfoTracker::new_rc1_traffic(self.cfo_rad_per_chip));
        }
    }

    pub(super) fn set_rc3_pilot_gating_mode(&mut self, enable: bool) {
        self.rc3_pilot_gating_mode = enable;
    }

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

    /// Unbiased reverse-pilot Ec/Io from same-PCG coherent moments.
    ///
    /// For N independent L-chip pilot sums, A=|Σp|² and P=Σ|p|².
    /// (A-P)/(N(N-1)L²) removes the coherent numerator's own noise term and
    /// estimates desired pilot energy per chip. Io is the total prompt power
    /// per chip over the complete 1,536-chip PCG.
    fn true_pilot_ec_io_db_from_metrics(
        pilot_norm_sq: f64,
        pilot_prompt_power: f64,
        n_symbols: usize,
        prompt_chip_power: f64,
    ) -> Option<f32> {
        if n_symbols < 2 || prompt_chip_power <= 1e-12 {
            return None;
        }
        let n = n_symbols as f64;
        let l = RC3_PILOT_SYMBOL_CHIPS as f64;
        let pilot_ec = (pilot_norm_sq - pilot_prompt_power) / (n * (n - 1.0) * l * l);
        let io = prompt_chip_power / RC3_PCG_CHIPS as f64;
        if pilot_ec <= 0.0 || io <= 0.0 {
            return None;
        }
        Some(10.0 * (pilot_ec / io).log10() as f32)
    }

    fn reset_rc3_pcg_measurement(&mut self) {
        self.rc3_pcg_measurement_abs_chip_start = None;
        self.rc3_pcg_measurement_raw_input_power = 0.0;
        self.rc3_pcg_measurement_prompt_chip_power = 0.0;
        self.rc3_pcg_measurement_pilot_run_prompt = Complex32::new(0.0, 0.0);
        self.rc3_pcg_measurement_pilot_prompt_power = 0.0;
        self.rc3_pcg_measurement_pilot_chip_idx = 0;
        self.rc3_pcg_measurement_traffic_run_prompt = Complex32::new(0.0, 0.0);
        self.rc3_pcg_measurement_traffic_prompt_power = 0.0;
        self.rc3_pcg_measurement_traffic_chip_idx = 0;
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
        let raw_power_dbfs = super::super::adc_referenced_power_dbfs(
            self.rc3_pcg_measurement_raw_input_power / RC3_PCG_CHIPS as f64,
        );
        let pilot_norm_sq = self.rc3_pcg_measurement_pilot_coherent_sum.norm_sqr() as f64;
        let n_symbols_this_pcg = self.rc3_pcg_measurement_pilot_symbol_count;
        let pilot_prompt_power_this_pcg = self.rc3_pcg_measurement_pilot_prompt_power;
        let traffic_symbols_this_pcg = RC3_PCG_CHIPS / RC3_PILOT_SYMBOL_CHIPS;
        let instant_mobile_power_dbfs = super::super::rc3_mobile_power_dbfs(
            pilot_norm_sq,
            pilot_prompt_power_this_pcg,
            n_symbols_this_pcg,
            self.rc3_pcg_measurement_traffic_prompt_power,
            traffic_symbols_this_pcg,
        );
        let instant_pilot_power_dbfs = super::super::rc3_pilot_power_dbfs(
            pilot_norm_sq,
            pilot_prompt_power_this_pcg,
            n_symbols_this_pcg,
        );
        let (
            raw_sinr_db,
            sinr_db,
            mobile_power_dbfs,
            pilot_power_dbfs,
            true_ec_io_db,
            legacy_ec_io_db,
            smoothing_window_len,
        ) = if hard_validated {
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
                self.rc3_pcg_measurement_traffic_prompt_power,
                traffic_symbols_this_pcg,
            ));
            // K factor cancels in the ratio: pass per-PCG N below (not K*N),
            // or SINR is mis-reported by 10·log10(K) dB low.
            let mut window_norm_sq = 0.0_f64;
            let mut window_prompt_pwr = 0.0_f64;
            let mut window_traffic_pwr = 0.0_f64;
            let mut window_pcgs = 0_usize;
            for &(ns, pp, _, tp, _) in &self.rc3_pcg_measurement_smoothing_window {
                window_norm_sq += ns;
                window_prompt_pwr += pp;
                window_traffic_pwr += tp;
                window_pcgs += 1;
            }
            let avg_norm_sq = window_norm_sq / window_pcgs.max(1) as f64;
            let avg_prompt_pwr = window_prompt_pwr / window_pcgs.max(1) as f64;
            let avg_traffic_pwr = window_traffic_pwr / window_pcgs.max(1) as f64;
            (
                Some(raw_sinr_db),
                Self::pilot_sym_sinr_db_from_metrics(
                    avg_norm_sq,
                    avg_prompt_pwr,
                    RC3_PILOT_SYMBOLS_PER_PCG,
                ),
                super::super::rc3_mobile_power_dbfs(
                    avg_norm_sq,
                    avg_prompt_pwr,
                    RC3_PILOT_SYMBOLS_PER_PCG,
                    avg_traffic_pwr,
                    traffic_symbols_this_pcg,
                ),
                super::super::rc3_pilot_power_dbfs(
                    avg_norm_sq,
                    avg_prompt_pwr,
                    RC3_PILOT_SYMBOLS_PER_PCG,
                ),
                Self::true_pilot_ec_io_db_from_metrics(
                    pilot_norm_sq,
                    self.rc3_pcg_measurement_pilot_prompt_power,
                    n_symbols_this_pcg,
                    self.rc3_pcg_measurement_prompt_chip_power,
                ),
                Some(Self::pilot_ec_io_db_from_prompt_power(
                    self.rc3_pcg_measurement_pilot_prompt_power,
                    self.rc3_pcg_measurement_prompt_chip_power,
                )),
                self.rc3_pcg_measurement_smoothing_window.len(),
            )
        } else {
            self.rc3_pcg_measurement_smoothing_window.clear();
            (
                None,
                f32::NAN,
                instant_mobile_power_dbfs,
                super::super::rc3_pilot_power_dbfs(
                    pilot_norm_sq,
                    pilot_prompt_power_this_pcg,
                    n_symbols_this_pcg,
                ),
                None,
                None,
                0,
            )
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
        block.tags.insert(
            "traffic_pcg_mobile_power_mdbfs",
            (mobile_power_dbfs * 1000.0) as i64,
        );
        block.tags.insert(
            "traffic_pcg_mobile_power_instant_mdbfs",
            (instant_mobile_power_dbfs * 1000.0) as i64,
        );
        if instant_pilot_power_dbfs.is_finite() {
            block.tags.insert(
                "traffic_pcg_pilot_power_instant_mdbfs",
                (instant_pilot_power_dbfs * 1000.0) as i64,
            );
        }
        // A noise-subtracted pilot estimate is intentionally NaN when the
        // coherent term is not measurable.  Never cast that NaN to an integer:
        // Rust maps it to zero, which looks like a 0 dBFS (extremely hot)
        // mobile and makes closed-loop power control command the mobile down
        // precisely when its pilot has disappeared.  Absence of the tag keeps
        // the observation invalid so the controller can hold its predictor.
        if pilot_power_dbfs.is_finite() {
            block.tags.insert(
                "traffic_pcg_pilot_power_mdbfs",
                (pilot_power_dbfs * 1000.0) as i64,
            );
        }
        if let Some(ec_io_db) = true_ec_io_db {
            block.tags.insert(
                "traffic_pcg_pilot_ec_io_true_mdb",
                (ec_io_db * 1000.0) as i64,
            );
        }
        if let Some(ec_io_db) = legacy_ec_io_db {
            block.tags.insert(
                "traffic_pcg_pilot_ec_io_legacy_mdb",
                (ec_io_db * 1000.0) as i64,
            );
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

    fn update_rc3_pcg_measurement(
        &mut self,
        chip_tx: usize,
        raw_input_chip: Complex32,
        prompt_chip: Complex32,
    ) {
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

        self.rc3_pcg_measurement_raw_input_power += raw_input_chip.norm_sqr() as f64;
        self.rc3_pcg_measurement_prompt_chip_power += prompt_chip.norm_sqr() as f64;
        let pcg_chip_offset = self.rc3_pcg_measurement_chip_count % RC3_PCG_CHIPS;
        const WALSH_4_16: [f32; 16] = [
            1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0, 1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0,
        ];
        self.rc3_pcg_measurement_traffic_run_prompt +=
            prompt_chip * WALSH_4_16[pcg_chip_offset % WALSH_4_16.len()];
        self.rc3_pcg_measurement_traffic_chip_idx += 1;
        if self.rc3_pcg_measurement_traffic_chip_idx == WALSH_4_16.len() {
            self.rc3_pcg_measurement_traffic_prompt_power +=
                self.rc3_pcg_measurement_traffic_run_prompt.norm_sqr() as f64;
            self.rc3_pcg_measurement_traffic_run_prompt = Complex32::new(0.0, 0.0);
            self.rc3_pcg_measurement_traffic_chip_idx = 0;
        }
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
    pub(crate) fn new(
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
            None,
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
            None,
            1u64 << 41,
            true,
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
        q_lc_gen: Option<LongCodeGenerator>,
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
        lc_period_chips: Option<usize>,
        lc_period_initial_state: u64,
        hpsk_signal_conjugated: bool,
        gardner_timing: GardnerTimingConfig,
    ) -> Self {
        // Fold `center_offset` into the stored cursor: `despread_phase`
        // now points directly at the prompt sample of the first chip.
        // Per-chip iteration in `despread_block` reads `pn_seq[dp]`,
        // applies LC, then advances `dp += oversample`. The caller
        // still passes the old-convention `(despread_phase,
        // center_offset)` pair for backward compatibility.
        let prompt_phase = (despread_phase + center_offset) % phase_period;
        let effective_initial_cfo = if epl_pilot {
            cfo_tracker::CfoTracker::rc3_principal_alias(initial_cfo_rad_per_chip)
        } else {
            initial_cfo_rad_per_chip
        };
        Self {
            base: BaseFinger::new(id),
            pn_seq,
            phase_period,
            oversample,
            center_offset,
            despread_phase: prompt_phase,
            acquisition_phase: prompt_phase,
            next_prompt_offset: samples_to_skip as f32 + center_offset as f32 + timing_mu_samples,
            previous_input_sample: None,
            timing_mu_samples,
            output_oversampled_chips,
            integrate_and_dump,
            gardner_timing: GardnerTimingRecovery::new(
                gardner_timing.with_samples_per_symbol(oversample as f32),
                timing_mu_samples,
            ),
            lc_gen,
            q_lc_gen,
            lc_period_chips,
            lc_period_initial_state,
            lc_chip_counter,
            chain_start_chip,
            sample_buffer: VecDeque::new(),
            chip_block_size,
            chain_chips_output: 0,
            current_lc_conj: Complex32::new(1.0, 0.0),
            current_chip_enabled: false,
            lc_decimation: lc_decimation.max(1),
            hpsk_prev_lc: 1.0,
            hpsk_chip_count: lc_chip_counter,
            hpsk_dec_q: 1.0,
            hpsk_signal_conjugated,
            cfo_rad_per_chip: effective_initial_cfo,
            cfo_phase: 0.0,
            rc3_cfo: if epl_pilot {
                Some(cfo_tracker::CfoTracker::new_rc3_traffic(
                    effective_initial_cfo,
                ))
            } else {
                None
            },
            rc3_pilot_gating_mode: false,
            access_cfo: if access_cfo {
                Some(cfo_tracker::CfoTracker::new_reverse_access(
                    initial_cfo_rad_per_chip,
                ))
            } else {
                None
            },
            rc1_cfo: None,
            rc1_cfo_walsh_chips: [Complex32::new(0.0, 0.0); 64],
            rc1_cfo_walsh_chip_sum: Complex32::new(0.0, 0.0),
            rc1_cfo_pn_chip_count: 0,
            rc1_cfo_walsh_chip_count: 0,
            rc1_cfo_prev_symbol: None,
            rc1_cfo_cross_sum: Complex32::new(0.0, 0.0),
            rc1_cfo_cross_magnitude_sum: 0.0,
            rc1_cfo_cross_count: 0,
            rc1_cfo_last_coherence: 0.0,
            rc1_cfo_accepted_windows: 0,
            rc1_cfo_rejected_windows: 0,
            rc3_cfo_pilot_accum: Complex32::new(0.0, 0.0),
            rc3_cfo_pilot_chips: 0,
            rc3_cfo_pilot_abs_pcg: None,
            rc3_cfo_prev_pcg_pilot: None,
            rc3_cfo_pcg_cross_accum: Complex32::new(0.0, 0.0),
            rc3_cfo_pcg_cross_count: 0,
            sample_rate_hz: 0.0,
            detection_snr,
            peak_energy: 0.0,
            low_energy_chip_count: 0,
            high_energy_chip_count: 0,
            pending_output: Vec::new(),
            rc3_pcg_measurement_abs_chip_start: None,
            rc3_pcg_measurement_raw_input_power: 0.0,
            rc3_pcg_measurement_prompt_chip_power: 0.0,
            rc3_pcg_measurement_pilot_run_prompt: Complex32::new(0.0, 0.0),
            rc3_pcg_measurement_pilot_prompt_power: 0.0,
            rc3_pcg_measurement_pilot_chip_idx: 0,
            rc3_pcg_measurement_traffic_run_prompt: Complex32::new(0.0, 0.0),
            rc3_pcg_measurement_traffic_prompt_power: 0.0,
            rc3_pcg_measurement_traffic_chip_idx: 0,
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
            delay_profile_run: [Complex32::new(0.0, 0.0); DELAY_PROFILE_TAPS],
            delay_profile_walsh: [[Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS]; DELAY_PROFILE_TAPS],
            delay_profile_pcg: [0.0; DELAY_PROFILE_TAPS],
            delay_track_enabled: false,
            delay_track_acc: [0.0; DELAY_PROFILE_TAPS],
            delay_track_pcgs: 0,
            delay_track_total_chips: 0,
            delay_slew_pending: None,
            delay_track_pcgs_since_move: DELAY_TRACK_MIN_PCGS_BETWEEN_MOVES,
            delay_track_frame: usize::MAX,
            delay_track_frame_pcgs: 0,
            delay_track_pcg_counted: false,
            delay_gate_env_prompt: 0.0,
            delay_gate_chips: 0,
            delay_gate_floor: 0.0,
            delay_gate_on_pcgs: 0,
            delay_gate_off_pcgs: 0,
            delay_burst_env: 0.0,
            delay_symbol_on_chips: 0,
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
            epl_rc1_bias: None,
            epl_rc1_frame_early: 0.0,
            epl_rc1_frame_prompt: 0.0,
            epl_rc1_frame_late: 0.0,
            epl_rc1_frame_windows: 0,
            epl_slew_frac: 0.0,
            epl_slew_total: 0.0,
            epl_slew_windows_since: 0,
            epl_slew_windows_total: 0,
            epl_slew_pending: None,
            epl_nonpilot_phy_frames: 0,
            epl_nonpilot_phy_invalid: 0,
            epl_nonpilot_fer_bits: 0,
            epl_nonpilot_fer_frames: 0,
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

    fn interp_input(&self, samples: &[Complex32], t: f32) -> Option<Complex32> {
        if t >= 0.0 {
            return interp_complex_contiguous(samples, t);
        }
        if t < -1.0 || samples.is_empty() {
            return None;
        }

        let previous = self.previous_input_sample?;
        let mu = t + 1.0;
        Some(previous + (samples[0] - previous) * mu)
    }

    fn remember_input_tail(&mut self, samples: &[Complex32]) {
        if let Some(&last) = samples.last() {
            self.previous_input_sample = Some(last);
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
        if let Some(period) = self.lc_period_chips
            && chip_tx % period == 0
        {
            self.lc_gen.set_state(self.lc_period_initial_state);
            if let Some(q_lc) = self.q_lc_gen.as_mut() {
                q_lc.set_state(self.lc_period_initial_state);
            }
        }
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
            if let Some(q_lc_gen) = self.q_lc_gen.as_mut() {
                let lc_q: f32 = if q_lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
                if self.hpsk_chip_count % 2 == 0 {
                    self.hpsk_dec_q = pn_q * lc_q;
                }
            } else if self.hpsk_chip_count % 2 == 0 {
                self.hpsk_dec_q = pn_q * self.hpsk_prev_lc;
            }
            let e = w12 * self.hpsk_dec_q;
            let ab = pn_i * pn_q;
            let cross = ab * e;
            let (re, im) = if self.hpsk_signal_conjugated {
                (lc_i * (1.0 - cross) * 0.5, lc_i * (e + ab) * 0.5)
            } else {
                (lc_i * (1.0 + cross) * 0.5, lc_i * (ab - e) * 0.5)
            };
            self.current_lc_conj = Complex32::new(re, im);
            if self.q_lc_gen.is_none() {
                self.hpsk_prev_lc = lc_i;
            }
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
    /// EPL slewing moves only received-sample time. Its PN/LC chip-time
    /// reference remains fixed.
    pub(crate) fn despread_block(&mut self, samples: &[Complex32]) {
        let os = self.oversample;
        let pp = self.phase_period;
        let len = samples.len();
        let len_f = len as f32;

        // Accumulate raw input power until the next emitted block.
        for &val in samples {
            self.raw_input_power_accum += val.norm_sqr() as f64;
        }
        self.raw_input_power_count += len as u64;

        // A negative fractional prompt can happen when acquisition chooses a
        // slightly-early `timing_mu` at the first replay sample. That prompt is
        // before the available input block, so skip that chip while keeping PN
        // and LC cursors in lockstep.
        while self.next_prompt_offset < 0.0
            && self
                .interp_input(samples, self.next_prompt_offset)
                .is_none()
        {
            let pn = self.pn_seq[self.despread_phase];
            self.advance_lc_for_new_chip(pn);
            self.despread_phase = (self.despread_phase + os) % pp;
            self.next_prompt_offset += os as f32;
        }

        // If the next prompt is past the end of this block, just
        // decrement the offset and return — no chips to process.
        if self.next_prompt_offset >= len_f {
            self.next_prompt_offset -= len_f;
            self.remember_input_tail(samples);
            return;
        }

        let mut idx = self.next_prompt_offset;
        while idx < len_f {
            // Delay tracking owns its own prompt shift. Both actuators move
            // received-sample time the same way, but keeping them in separate
            // slots means neither can silently discard the other's request.
            if let Some(shift) = self.delay_slew_pending.take() {
                let old_idx = idx;
                idx += shift;
                info!(
                    "DELAY_SLEW[finger={}] shift={:+.2}sample idx={:.3}->{:.3} total={:+} chip",
                    self.base.id, shift, old_idx, idx, self.delay_track_total_chips,
                );
                if idx >= len_f {
                    self.next_prompt_offset = idx - len_f;
                    self.remember_input_tail(samples);
                    return;
                }
            }

            // Apply any pending EPL slew before reading the prompt sample.
            // Timing moves received-sample time relative to a fixed PN/LC
            // reference.
            if let Some(slew) = self.epl_slew_pending.take() {
                let old_dp = self.despread_phase;
                let old_idx = idx;
                idx += slew;
                info!(
                    "EPL_SLEW[finger={}] direction={:+.2}sample despread_phase={}->{} \
                     idx={:.3}->{:.3} total={:+.2}sample",
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
                    self.remember_input_tail(samples);
                    return;
                }
            }

            // Per-chip iteration: this iter is a prompt by construction.
            let Some(val) = self.interp_input(samples, idx) else {
                break;
            };
            let mut gardner_adjust = GardnerTimingAdjustment::default();
            let mut gardner_finished = false;
            let gardner_mid = self
                .gardner_timing
                .as_ref()
                .filter(|gardner| gardner.is_tracking_active() && gardner.needs_midpoint())
                .and_then(|_| self.interp_input(samples, idx - os as f32 * 0.5));
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

            let epl_active =
                self.epl_enabled && self.current_chip_enabled && self.base.is_hard_validated();
            // Delay tracking stands on its own: it must run on production
            // configurations that leave the EPL instrumentation switched off.
            let delay_active = self.delay_track_enabled
                && self.current_chip_enabled
                && self.base.is_hard_validated();

            if self.current_chip_enabled {
                let out = despread * self.current_lc_conj;
                let chip_tx = self.lc_chip_counter.saturating_sub(1);
                if delay_active {
                    let power = out.norm_sqr() as f64;
                    self.delay_gate_env_prompt += power;
                    self.delay_gate_chips += 1;
                    self.delay_burst_env += DELAY_BURST_ENV_ALPHA * (power - self.delay_burst_env);
                    if chip_tx % RC1_SYMBOL_CHIPS == 0 {
                        self.delay_symbol_on_chips = 0;
                    }
                    if chip_tx % DELAY_GATE_PCG_CHIPS == 0 {
                        self.delay_track_start_pcg(chip_tx);
                    }
                    // Correlating the hypotheses is the expensive part, so do it
                    // only while the mobile is transmitting, and only for this
                    // frame's quota of PCGs.
                    if self.delay_burst_env > DELAY_GATE_ON_FACTOR * self.delay_gate_floor
                        && self.delay_track_frame_quota_available()
                    {
                        self.delay_symbol_on_chips += 1;
                        self.accumulate_delay_profile_chip(samples, idx, chip_tx);
                    }
                    if (chip_tx + 1) % DELAY_GATE_PCG_CHIPS == 0 {
                        self.delay_commit_gated_pcg();
                    }
                }
                let mut pcg_measurement_prompt_chip = out;
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
                    } else {
                        self.sample_buffer.push_back(out);
                    }
                }

                if self.epl_pilot_mode {
                    self.update_rc3_pcg_measurement(chip_tx, val, pcg_measurement_prompt_chip);
                }

                if epl_active {
                    // Envelope (non-coherent) — always accumulated.
                    self.epl_window_env_prompt += out.norm_sqr() as f64;

                    // RC3 measures adjacent received-sample hypotheses against
                    // the same HPSK PN/LC reference. RC1 measures them against
                    // adjacent PN phases instead, because it has no pilot to
                    // hold a common reference against.
                    let out_e = self.interp_input(samples, idx - 1.0).map(|val_e| {
                        let pn_e = if self.epl_pilot_mode {
                            pn
                        } else {
                            self.pn_seq[(self.despread_phase + pp - 1) % pp]
                        };
                        let e = pn_e * val_e * self.current_lc_conj;
                        self.epl_window_env_early += e.norm_sqr() as f64;
                        e
                    });
                    let out_l = if idx + 1.0 < len_f {
                        let val_l = interp_complex_contiguous(samples, idx + 1.0).unwrap_or(val);
                        let pn_l = if self.epl_pilot_mode {
                            pn
                        } else {
                            self.pn_seq[(self.despread_phase + 1) % pp]
                        };
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

            if gardner_adjust.integer_slew_samples != 0
                && let Some(gardner) = self.gardner_timing.as_ref()
            {
                info!(
                    "GARDNER_SLEW[finger={}] direction={:+}sample offset={:+.3}sample error={:+.4} updates={} skipped={} chip_tx={}",
                    self.base.id,
                    gardner_adjust.integer_slew_samples,
                    gardner.offset_samples(),
                    gardner_adjust.error,
                    gardner.updates(),
                    gardner.skipped(),
                    self.lc_chip_counter,
                );
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
        self.remember_input_tail(samples);
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

                // One pilot vector per PCG. Warmup updates every PCG, steady
                // state averages eight cross products first.
                if is_pilot_symbol && self.rc3_cfo.is_some() {
                    let abs_pcg = symbol_start / RC3_PCG_CHIPS;
                    if self.rc3_cfo_pilot_abs_pcg != Some(abs_pcg) {
                        // Discard an incomplete vector left by a mid-PCG
                        // finger start, which has no well-defined phase center.
                        self.rc3_cfo_pilot_abs_pcg = Some(abs_pcg);
                        self.rc3_cfo_pilot_accum = Complex32::new(0.0, 0.0);
                        self.rc3_cfo_pilot_chips = 0;
                    }
                    self.rc3_cfo_pilot_accum += prompt;
                    self.rc3_cfo_pilot_chips += 16;
                    if self.rc3_cfo_pilot_chips >= RC3_PILOT_CHIPS_PER_PCG {
                        let pcg_pilot = self.rc3_cfo_pilot_accum;
                        let transmitted = !self.rc3_pilot_gating_mode || abs_pcg % 4 >= 2;
                        if transmitted {
                            let warmup = self.rc3_cfo.as_ref().is_some_and(|cfo| cfo.in_warmup());
                            if warmup {
                                if let Some(ref mut cfo) = self.rc3_cfo {
                                    cfo.observe_pilot(pcg_pilot, RC3_PCG_CHIPS);
                                }
                            } else if let Some(previous) = self.rc3_cfo_prev_pcg_pilot {
                                self.rc3_cfo_pcg_cross_accum +=
                                    pcg_pilot * Complex32::new(previous.re, -previous.im);
                                self.rc3_cfo_pcg_cross_count += 1;
                                if self.rc3_cfo_pcg_cross_count >= 8 {
                                    if let Some(ref mut cfo) = self.rc3_cfo {
                                        cfo.observe_pilot_cross_sum(
                                            self.rc3_cfo_pcg_cross_accum,
                                            RC3_PCG_CHIPS,
                                        );
                                    }
                                    self.rc3_cfo_pcg_cross_accum = Complex32::new(0.0, 0.0);
                                    self.rc3_cfo_pcg_cross_count = 0;
                                }
                            }
                            self.rc3_cfo_prev_pcg_pilot = Some(pcg_pilot);
                        } else {
                            // Do not let the next active phase compare against
                            // either receiver noise or an active vector three
                            // PCGs old while still dividing by one-PCG time.
                            if let Some(ref mut cfo) = self.rc3_cfo {
                                cfo.clear_pilot_baseline();
                            }
                            self.rc3_cfo_prev_pcg_pilot = None;
                        }
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

    fn observe_rc1_cfo_chips(&mut self, chips: &[Complex32], first_abs_chip: usize) {
        const CHIPS_PER_WALSH_CHIP: usize = 4;
        const CHIPS_PER_SYMBOL: usize = 256;
        const CROSS_PRODUCTS_PER_UPDATE: usize = 32;
        const MIN_PEAK_TO_REST: f32 = 4.0;
        const MIN_CIRCULAR_COHERENCE: f32 = 0.85;

        if self.rc1_cfo.is_none() {
            return;
        }

        for (offset, &chip) in chips.iter().enumerate() {
            let abs_chip = first_abs_chip.saturating_add(offset);
            if abs_chip % CHIPS_PER_SYMBOL == 0 {
                self.rc1_cfo_walsh_chips.fill(Complex32::new(0.0, 0.0));
                self.rc1_cfo_walsh_chip_sum = Complex32::new(0.0, 0.0);
                self.rc1_cfo_pn_chip_count = 0;
                self.rc1_cfo_walsh_chip_count = 0;
            }
            self.rc1_cfo_walsh_chip_sum += chip;
            self.rc1_cfo_pn_chip_count += 1;

            if abs_chip % CHIPS_PER_WALSH_CHIP != CHIPS_PER_WALSH_CHIP - 1 {
                continue;
            }
            if self.rc1_cfo_pn_chip_count == CHIPS_PER_WALSH_CHIP
                && self.rc1_cfo_walsh_chip_count < self.rc1_cfo_walsh_chips.len()
            {
                self.rc1_cfo_walsh_chips[self.rc1_cfo_walsh_chip_count] =
                    self.rc1_cfo_walsh_chip_sum;
                self.rc1_cfo_walsh_chip_count += 1;
            }
            self.rc1_cfo_walsh_chip_sum = Complex32::new(0.0, 0.0);
            self.rc1_cfo_pn_chip_count = 0;

            if abs_chip % CHIPS_PER_SYMBOL != CHIPS_PER_SYMBOL - 1
                || self.rc1_cfo_walsh_chip_count != self.rc1_cfo_walsh_chips.len()
            {
                continue;
            }

            WalshGenerator::fwht_fixed(&mut self.rc1_cfo_walsh_chips);
            let (peak_index, peak_power) = self
                .rc1_cfo_walsh_chips
                .iter()
                .enumerate()
                .map(|(index, value)| (index, value.norm_sqr()))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .expect("64 Walsh bins");
            let total_power = self
                .rc1_cfo_walsh_chips
                .iter()
                .map(|value| value.norm_sqr())
                .sum::<f32>();
            let mean_rest = ((total_power - peak_power)
                / (self.rc1_cfo_walsh_chips.len() - 1) as f32)
                .max(1e-12);
            let symbol = self.rc1_cfo_walsh_chips[peak_index];

            if peak_power / mean_rest >= MIN_PEAK_TO_REST {
                if let Some(previous) = self.rc1_cfo_prev_symbol {
                    let cross = symbol * previous.conj();
                    self.rc1_cfo_cross_sum += cross;
                    self.rc1_cfo_cross_magnitude_sum += cross.norm();
                    self.rc1_cfo_cross_count += 1;
                }
                self.rc1_cfo_prev_symbol = Some(symbol);
            } else {
                self.rc1_cfo_prev_symbol = None;
            }

            if self.rc1_cfo_cross_count < CROSS_PRODUCTS_PER_UPDATE {
                continue;
            }

            let coherence = if self.rc1_cfo_cross_magnitude_sum > 1e-12 {
                self.rc1_cfo_cross_sum.norm() / self.rc1_cfo_cross_magnitude_sum
            } else {
                0.0
            };
            self.rc1_cfo_last_coherence = coherence;
            if coherence >= MIN_CIRCULAR_COHERENCE {
                if self
                    .rc1_cfo
                    .as_mut()
                    .is_some_and(|cfo| cfo.observe_rc1_walsh_cross_sum(self.rc1_cfo_cross_sum))
                {
                    self.rc1_cfo_accepted_windows += 1;
                } else {
                    self.rc1_cfo_rejected_windows += 1;
                }
            } else {
                self.rc1_cfo_rejected_windows += 1;
            }
            self.rc1_cfo_cross_sum = Complex32::new(0.0, 0.0);
            self.rc1_cfo_cross_magnitude_sum = 0.0;
            self.rc1_cfo_cross_count = 0;
        }
    }

    /// Despread this chip at every delay hypothesis.
    ///
    /// A hypothesis moves the arrival time only. Shifting the PN and long code
    /// with it would leave the alignment unchanged and every tap identical.
    fn accumulate_delay_profile_chip(&mut self, samples: &[Complex32], idx: f32, chip_tx: usize) {
        let os = self.oversample as isize;
        let pn = self.pn_seq[self.despread_phase];
        let lc_conj = self.current_lc_conj;
        for tap in 0..DELAY_PROFILE_TAPS {
            let chip_shift = tap as isize - DELAY_PROFILE_HALF_CHIPS as isize;
            let Some(val) = self.interp_input(samples, idx + (chip_shift * os) as f32) else {
                continue;
            };
            self.delay_profile_run[tap] += pn * val * lc_conj;
        }

        let sub_chip = chip_tx % RC1_PN_CHIPS_PER_WALSH_CHIP;
        if sub_chip != RC1_PN_CHIPS_PER_WALSH_CHIP - 1 {
            return;
        }
        let walsh_chip = (chip_tx % RC1_SYMBOL_CHIPS) / RC1_PN_CHIPS_PER_WALSH_CHIP;
        for tap in 0..DELAY_PROFILE_TAPS {
            self.delay_profile_walsh[tap][walsh_chip] = self.delay_profile_run[tap];
            self.delay_profile_run[tap] = Complex32::new(0.0, 0.0);
        }
        if walsh_chip != RC1_WALSH_CHIPS - 1 {
            return;
        }
        // A symbol only partly inside a burst has a noise peak, so drop it.
        if self.delay_symbol_on_chips < RC1_SYMBOL_CHIPS {
            self.delay_profile_run = [Complex32::new(0.0, 0.0); DELAY_PROFILE_TAPS];
            self.delay_profile_walsh =
                [[Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS]; DELAY_PROFILE_TAPS];
            return;
        }
        for tap in 0..DELAY_PROFILE_TAPS {
            let bins = &mut self.delay_profile_walsh[tap];
            WalshGenerator::fwht_fixed(bins);
            let peak = bins.iter().map(|bin| bin.norm_sqr()).fold(0.0f32, f32::max);
            self.delay_profile_pcg[tap] += peak as f64;
            bins.fill(Complex32::new(0.0, 0.0));
        }
    }

    /// Open a new PCG for the delay tracker, rolling the per-frame quota over
    /// when the frame changes.
    fn delay_track_start_pcg(&mut self, chip_tx: usize) {
        let frame = chip_tx / (DELAY_GATE_PCG_CHIPS * DELAY_TRACK_PCGS_PER_FRAME_TOTAL);
        if frame != self.delay_track_frame {
            self.delay_track_frame = frame;
            self.delay_track_frame_pcgs = 0;
        }
        self.delay_track_pcg_counted = false;
    }

    /// Whether this PCG may be fed to the profile, consuming a frame quota slot
    /// the first time the PCG is admitted.
    fn delay_track_frame_quota_available(&mut self) -> bool {
        if self.delay_track_pcg_counted {
            return true;
        }
        if self.delay_track_frame_pcgs >= DELAY_TRACK_PCGS_PER_FRAME {
            return false;
        }
        self.delay_track_frame_pcgs += 1;
        self.delay_track_pcg_counted = true;
        true
    }

    /// Drop the accumulated profile. Called when the sample origin moves, which
    /// leaves it referring to a timing reference that is gone.
    pub(crate) fn reset_delay_tracking(&mut self) {
        self.delay_track_acc = [0.0; DELAY_PROFILE_TAPS];
        self.delay_profile_pcg = [0.0; DELAY_PROFILE_TAPS];
        self.delay_profile_run = [Complex32::new(0.0, 0.0); DELAY_PROFILE_TAPS];
        self.delay_profile_walsh =
            [[Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS]; DELAY_PROFILE_TAPS];
        self.delay_track_pcgs = 0;
        self.delay_symbol_on_chips = 0;
        self.delay_slew_pending = None;
    }

    /// Fold one PCG into the tracking decision, if the mobile transmitted during
    /// it. Silence contributes equally to every hypothesis and hides the peak.
    fn delay_commit_gated_pcg(&mut self) {
        let chips = self.delay_gate_chips;
        if chips == 0 {
            return;
        }
        let per_chip_prompt = self.delay_gate_env_prompt / chips as f64;
        if self.delay_gate_floor <= 0.0 || per_chip_prompt < self.delay_gate_floor {
            self.delay_gate_floor = per_chip_prompt;
        } else {
            self.delay_gate_floor +=
                DELAY_GATE_FLOOR_LEAK * (per_chip_prompt - self.delay_gate_floor);
        }

        if per_chip_prompt > DELAY_GATE_ON_FACTOR * self.delay_gate_floor {
            self.delay_gate_on_pcgs += 1;
            for tap in 0..DELAY_PROFILE_TAPS {
                self.delay_track_acc[tap] +=
                    DELAY_TRACK_ALPHA * (self.delay_profile_pcg[tap] - self.delay_track_acc[tap]);
            }
            self.delay_track_pcgs += 1;
            self.delay_track_pcgs_since_move = self.delay_track_pcgs_since_move.saturating_add(1);
            if self.delay_track_enabled && self.delay_track_pcgs >= DELAY_TRACK_WARMUP_PCGS {
                self.delay_track_decide();
            }
        } else {
            self.delay_gate_off_pcgs += 1;
        }

        self.delay_profile_pcg = [0.0; DELAY_PROFILE_TAPS];
        self.delay_gate_env_prompt = 0.0;
        self.delay_gate_chips = 0;
    }

    /// Reposition the prompt onto the delay carrying the peak. Path delay steps
    /// by whole chips during a call, and an early/late gate spanning a quarter
    /// chip reads zero error for a peak a chip away.
    fn delay_track_decide(&mut self) {
        if self.delay_track_pcgs_since_move < DELAY_TRACK_MIN_PCGS_BETWEEN_MOVES {
            return;
        }
        let (best_tap, best) = self
            .delay_track_acc
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(tap, power)| (tap, *power))
            .expect("profile taps");
        let prompt = self.delay_track_acc[DELAY_PROFILE_HALF_CHIPS];
        let chip_shift = best_tap as isize - DELAY_PROFILE_HALF_CHIPS as isize;

        if chip_shift == 0 || best <= prompt * DELAY_TRACK_MARGIN {
            return;
        }

        let total = self.delay_track_total_chips + chip_shift;
        if total.abs() > DELAY_TRACK_MAX_TOTAL_CHIPS {
            // This far from where it acquired, the finger no longer holds what
            // it locked onto. Leave it to the prune policy.
            warn!(
                "DELAY_TRACK[finger={}] refusing reposition={:+} chip: total {:+} chip \
                 would exceed the {} chip excursion limit",
                self.base.id, chip_shift, total, DELAY_TRACK_MAX_TOTAL_CHIPS,
            );
            self.delay_track_acc = [0.0; DELAY_PROFILE_TAPS];
            self.delay_track_pcgs = 0;
            return;
        }

        // Only received-sample time moves. The PN and long code cursors keep
        // their chip time, which makes this a delay change and not a chip slip.
        self.delay_slew_pending = Some((chip_shift * self.oversample as isize) as f32);
        self.delay_track_total_chips = total;
        info!(
            "DELAY_TRACK[finger={}] chip_tx={} reposition={:+} chip \
             peak_over_prompt={:+.2}dB total={:+} chip pcgs={}",
            self.base.id,
            self.lc_chip_counter,
            chip_shift,
            10.0 * (best / prompt.max(1e-12)).max(1e-12).log10(),
            self.delay_track_total_chips,
            self.delay_track_pcgs,
        );
        // The move renames every hypothesis, so refill before deciding again.
        self.delay_track_acc = [0.0; DELAY_PROFILE_TAPS];
        self.delay_track_pcgs = 0;
        self.delay_track_pcgs_since_move = 0;
    }

    fn epl_run_slew_loop(&mut self) {
        self.epl_slew_windows_total += 1;
        self.epl_slew_windows_since += 1;

        // RC1 is gated, so sum exactly one 20 ms frame before normalizing or
        // gated noise dominates the discriminator. Pilot updates per window.
        let (pe, pp, pl) = if self.epl_pilot_mode {
            (
                self.epl_window_pilot_pwr_early,
                self.epl_window_pilot_pwr_prompt,
                self.epl_window_pilot_pwr_late,
            )
        } else {
            self.epl_rc1_frame_early += self.epl_window_coh4_pwr_early;
            self.epl_rc1_frame_prompt += self.epl_window_coh4_pwr_prompt;
            self.epl_rc1_frame_late += self.epl_window_coh4_pwr_late;
            self.epl_rc1_frame_windows += 1;
            if self.epl_rc1_frame_windows < EPL_RC1_WINDOWS_PER_FRAME {
                return;
            }

            let frame = (
                self.epl_rc1_frame_early,
                self.epl_rc1_frame_prompt,
                self.epl_rc1_frame_late,
            );
            self.epl_rc1_frame_early = 0.0;
            self.epl_rc1_frame_prompt = 0.0;
            self.epl_rc1_frame_late = 0.0;
            self.epl_rc1_frame_windows = 0;
            frame
        };
        if pp <= 1e-12 {
            return;
        }
        if !self.epl_pilot_mode
            && self.epl_nonpilot_fer_frames == EPL_NONPILOT_FER_WINDOW_FRAMES
            && self.epl_nonpilot_fer_bits.count_ones() > EPL_RC1_MAX_INVALID_FRAMES_FOR_SLEW
        {
            return;
        }

        let disc = (pe - pl) / pp;
        let alpha = if self.epl_pilot_mode {
            EPL_SLEW_ALPHA_PILOT
        } else {
            EPL_SLEW_ALPHA
        };
        self.epl_slew_iir = (1.0 - alpha) * self.epl_slew_iir + alpha * disc;
        let warmup = if self.epl_pilot_mode {
            EPL_SLEW_WARMUP_WINDOWS_PILOT
        } else {
            EPL_SLEW_WARMUP_WINDOWS
        };
        if self.epl_slew_windows_total < warmup {
            return;
        }
        let timing_error = if self.epl_pilot_mode {
            self.epl_slew_iir
        } else {
            let bias = self.epl_rc1_bias.get_or_insert(self.epl_slew_iir);
            self.epl_slew_iir - *bias
        };
        let dead_zone = if self.epl_pilot_mode {
            EPL_SLEW_DEAD_ZONE_PILOT
        } else {
            EPL_SLEW_DEAD_ZONE
        };
        let effective = if timing_error.abs() < dead_zone {
            0.0
        } else {
            timing_error
        };
        let loop_gain = if self.epl_pilot_mode {
            EPL_SLEW_LOOP_GAIN_PILOT
        } else {
            EPL_SLEW_LOOP_GAIN
        };
        self.epl_slew_frac = (self.epl_slew_frac - loop_gain * effective).clamp(-1.0, 1.0);
        let min_between = if self.epl_pilot_mode {
            EPL_SLEW_MIN_WINDOWS_BETWEEN_PILOT
        } else {
            EPL_SLEW_MIN_WINDOWS_BETWEEN
        };
        if self.epl_slew_windows_since < min_between {
            return;
        }

        let positive_allowed = !self.epl_pilot_mode || pl > pp;
        let negative_allowed = !self.epl_pilot_mode || pe > pp;
        let trigger = if self.epl_pilot_mode {
            1.0
        } else {
            EPL_RC1_SLEW_TRIGGER
        };
        if self.epl_slew_frac >= trigger && positive_allowed {
            self.epl_slew_pending = Some(EPL_SLEW_STEP_SAMPLES);
            self.epl_slew_frac = 0.0;
            self.epl_slew_total += EPL_SLEW_STEP_SAMPLES as f64;
            self.epl_slew_windows_since = 0;
        } else if self.epl_slew_frac <= -trigger && negative_allowed {
            self.epl_slew_pending = Some(-EPL_SLEW_STEP_SAMPLES);
            self.epl_slew_frac = 0.0;
            self.epl_slew_total -= EPL_SLEW_STEP_SAMPLES as f64;
            self.epl_slew_windows_since = 0;
        }
    }

    fn epl_observe_nonpilot_phy(&mut self, output: &[SampleBlock]) {
        if !self.epl_slew_enabled || self.epl_pilot_mode {
            return;
        }
        for block in output {
            if block.tags.get("traffic_phy_frame") != Some(&1)
                && block.tags.get("traffic_phy_status") != Some(&1)
            {
                continue;
            }
            let Some(&phy_valid) = block.tags.get("traffic_phy_valid") else {
                continue;
            };
            self.epl_nonpilot_phy_frames = self.epl_nonpilot_phy_frames.saturating_add(1);
            let invalid = phy_valid != 1;
            if invalid {
                self.epl_nonpilot_phy_invalid = self.epl_nonpilot_phy_invalid.saturating_add(1);
            }
            self.epl_nonpilot_fer_bits = ((self.epl_nonpilot_fer_bits << 1) | u64::from(invalid))
                & EPL_NONPILOT_FER_WINDOW_MASK;
            self.epl_nonpilot_fer_frames = self
                .epl_nonpilot_fer_frames
                .saturating_add(1)
                .min(EPL_NONPILOT_FER_WINDOW_FRAMES);
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
            let cfo_hz = self.cfo_rad_per_chip as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
            self.epl_last_log_pilot_ec_io_db = Some(pilot_ec_io_db);

            // Pilot timing is part of the live RC3 tracking loop.  Keep its
            // one-line-per-second state visible at the normal log level so a
            // field run shows the discriminator and applied slews directly.
            info!(
                "EPL_TRACK[finger={}] sec#{} windows={} N={} chips={} mode=pilot | \
                 env: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 env_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 pilot_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} Ec/Io={:+.2}dB | \
                 pilot: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} Ec/Io={:+.2}dB | \
                 slew: iir={:+.4} frac={:+.4} total={} since={} | cfo={:+.2}Hz",
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
                cfo_hz,
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
            let cfo_hz = self.cfo_rad_per_chip as f64 * 1_228_800.0 / (2.0 * std::f64::consts::PI);
            let recent_frames = u64::from(self.epl_nonpilot_fer_frames);
            let recent_invalid = u64::from(self.epl_nonpilot_fer_bits.count_ones());

            info!(
                "EPL_TRACK[finger={}] sec#{} windows={} N={} chips={} mode=coh4 | \
                 env: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 env_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 coh4_win: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 coh4: E={:.3e} P={:.3e} L={:.3e} (E-L)/P={:+.4} | \
                 slew: iir={:+.4} bias={:+.4} delta={:+.4} frac={:+.4} total={} since={} | \
                 cfo={:+.2}Hz cfo_coh={:.3} accepted={} rejected={} \
                 phy_invalid={}/{} recent_invalid={}/{}",
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
                self.epl_rc1_bias.unwrap_or(self.epl_slew_iir),
                self.epl_slew_iir - self.epl_rc1_bias.unwrap_or(self.epl_slew_iir),
                self.epl_slew_frac,
                self.epl_slew_total,
                self.epl_slew_windows_since,
                cfo_hz,
                self.rc1_cfo_last_coherence,
                self.rc1_cfo_accepted_windows,
                self.rc1_cfo_rejected_windows,
                self.epl_nonpilot_phy_invalid,
                self.epl_nonpilot_phy_frames,
                recent_invalid,
                recent_frames,
            );

            self.epl_coh4_pwr_early = 0.0;
            self.epl_coh4_pwr_prompt = 0.0;
            self.epl_coh4_pwr_late = 0.0;
            self.epl_nonpilot_phy_frames = 0;
            self.epl_nonpilot_phy_invalid = 0;
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
        let buffered_chips = self.sample_buffer.len() / output_oversample;
        let first_abs_chip = self.lc_chip_counter.saturating_sub(buffered_chips);
        let mut all_samples = Vec::with_capacity(total_len);

        // Process in chip_block_size windows for CFO correction + pilot
        // estimation, but collect all corrected samples into one buffer.
        for block_idx in 0..n_blocks {
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
            } else if self.rc1_cfo.is_some() {
                let block_first_abs_chip =
                    first_abs_chip.saturating_add(block_idx.saturating_mul(block_len_chips));
                if output_oversample > 1 {
                    let prompt_phase = self.center_offset.min(output_oversample - 1);
                    let prompt_samples = raw
                        .chunks_exact(output_oversample)
                        .map(|chip| chip[prompt_phase])
                        .collect::<Vec<_>>();
                    self.observe_rc1_cfo_chips(&prompt_samples, block_first_abs_chip);
                } else {
                    self.observe_rc1_cfo_chips(&raw, block_first_abs_chip);
                }

                let mut buf = raw;
                let cfo = self.rc1_cfo.as_mut().expect("RC1 CFO tracker present");
                cfo.derotate_chips(&mut buf, output_oversample);
                all_samples.extend_from_slice(&buf);
                self.cfo_rad_per_chip = cfo.cfo_rad_per_chip();
                self.cfo_phase = cfo.cfo_phase();
            } else {
                // Non-pilot traffic retains the acquisition CFO correction.
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
                    } else if let Some(ref mut cfo) = self.rc1_cfo {
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
            // Access uses its separate Walsh-0 tracker. RC3 is fed by the
            // reverse pilot path.
            if let Some(ref mut cfo) = self.access_cfo {
                const ACCESS_CFO_COH_GATE: f32 = 0.12;
                if pilot_coh_norm >= ACCESS_CFO_COH_GATE {
                    cfo.observe_pilot(pilot, block_len_chips);
                }
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
            let raw_power_db = super::super::adc_referenced_power_dbfs(raw_mean as f64);
            blk.tags
                .insert("finger_raw_power_mdb", (raw_power_db * 1000.0) as i64);
            self.raw_input_power_accum = 0.0;
            self.raw_input_power_count = 0;
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
    fn reset_timing_measurements(&mut self) {
        self.reset_delay_tracking();
    }

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
        self.epl_observe_nonpilot_phy(&live);
        let drain_ns = t1.elapsed().as_nanos() as u64;

        self.despread_ns += despread_ns;
        self.drain_ns += drain_ns;
        self.finger_block_count += 1;

        if self.finger_block_count % 500 == 0 {
            let d_ms = self.despread_ns as f64 / 1e6;
            let c_ms = self.drain_ns as f64 / 1e6;
            let total = d_ms + c_ms;
            trace!(
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
        trace!(
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
                trace!("      [{si}] {name:<50} {ms:>8.1}ms  {pct:>5.1}%");
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

#[cfg(test)]
mod raw_power_tests {
    use std::sync::Arc;

    use num_complex::Complex32;

    use super::{
        EPL_NONPILOT_FER_WINDOW_FRAMES, EPL_RC1_WINDOWS_PER_FRAME, EPL_SLEW_MIN_WINDOWS_BETWEEN,
        EPL_SLEW_STEP_SAMPLES, EPL_SLEW_WARMUP_WINDOWS, PnLcFinger, RC3_PILOT_SYMBOL_CHIPS,
        RC3_PILOT_SYMBOLS_PER_PCG,
    };
    use crate::phy::coding::long_code::LongCodeGenerator;
    use crate::receiver::pipelined::{
        SampleBlock, adc_referenced_power_dbfs, rx_matched_filter_power_gain_db,
    };

    fn test_nonpilot_finger() -> PnLcFinger {
        let phase_period = 64;
        let pn = Arc::new(vec![Complex32::new(1.0, 0.0); phase_period]);
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        PnLcFinger::new(
            1,
            pn,
            phase_period,
            4,
            0,
            0,
            lc,
            0,
            0,
            64,
            0,
            0.0,
            0.0,
            1,
            true,
            true,
            false,
            false,
            0.0,
            false,
            false,
        )
    }

    #[test]
    fn adc_power_reference_removes_matched_filter_gain() {
        let filter_gain = 10f64.powf(rx_matched_filter_power_gain_db() as f64 / 10.0);

        assert!(adc_referenced_power_dbfs(filter_gain).abs() < 1e-4);
        assert!((adc_referenced_power_dbfs(filter_gain * 0.5) + 3.0103).abs() < 1e-3);
    }

    #[test]
    fn true_pilot_ec_io_removes_coherent_self_noise_term() {
        let n = RC3_PILOT_SYMBOLS_PER_PCG as f64;
        let l = RC3_PILOT_SYMBOL_CHIPS as f64;
        let pilot_norm_sq = (n * l).powi(2);
        let pilot_prompt_power = n * l * l;
        let prompt_chip_power = 1536.0;

        let measured = PnLcFinger::true_pilot_ec_io_db_from_metrics(
            pilot_norm_sq,
            pilot_prompt_power,
            RC3_PILOT_SYMBOLS_PER_PCG,
            prompt_chip_power,
        )
        .expect("clean pilot must produce Ec/Io");
        assert!(
            measured.abs() < 1e-5,
            "unit pilot over unit Io should be 0 dB, got {measured}"
        );

        assert!(
            PnLcFinger::true_pilot_ec_io_db_from_metrics(
                pilot_prompt_power,
                pilot_prompt_power,
                RC3_PILOT_SYMBOLS_PER_PCG,
                prompt_chip_power,
            )
            .is_none(),
            "A=P contains no positive unbiased coherent pilot estimate"
        );
    }

    #[test]
    fn epl_slew_moves_sample_prompt_without_moving_pn_lc_reference() {
        let oversample = 4;
        let phase_period = 64;
        let center_offset = 2;
        let pn = Arc::new(vec![Complex32::new(1.0, 0.0); phase_period]);
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut finger = PnLcFinger::new(
            1,
            pn,
            phase_period,
            oversample,
            0,
            center_offset,
            lc,
            0,
            0,
            64,
            0,
            0.0,
            0.0,
            2,
            true,
            true,
            true,
            false,
            0.0,
            false,
            false,
        );

        // The initial prompt is input sample 2 and PN phase 2. A fractional
        // EPL correction selects 2.25, but PN/LC must remain at phase 2.
        finger.epl_slew_pending = Some(EPL_SLEW_STEP_SAMPLES);
        finger.despread_block(&vec![Complex32::new(1.0, 0.0); 16]);

        assert_eq!(finger.despread_phase, center_offset + 4 * oversample);
        assert_eq!(finger.lc_chip_counter, 4);
        assert!((finger.next_prompt_offset - 2.25).abs() < f32::EPSILON);
    }

    #[test]
    fn nonpilot_epl_slew_interpolates_across_block_boundary() {
        let oversample = 4;
        let phase_period = 64;
        let pn = Arc::new(vec![Complex32::new(1.0, 0.0); phase_period]);
        let lc = LongCodeGenerator::new_access_channel_with_state(0, 0, 0, 0, 1u64 << 41);
        let mut finger = PnLcFinger::new(
            1,
            pn,
            phase_period,
            oversample,
            0,
            0,
            lc,
            0,
            0,
            64,
            0,
            0.0,
            0.0,
            1,
            true,
            true,
            false,
            false,
            0.0,
            false,
            false,
        );

        finger.despread_block(&vec![Complex32::new(1.0, 0.0); 16]);
        assert_eq!(finger.lc_chip_counter, 4);
        finger.sample_buffer.clear();

        finger.epl_slew_pending = Some(-EPL_SLEW_STEP_SAMPLES);
        finger.despread_block(&vec![Complex32::new(1.0, 0.0); 16]);

        assert_eq!(finger.lc_chip_counter, 9);
        assert_eq!(finger.despread_phase, 9 * oversample);
        assert_eq!(finger.sample_buffer.len(), 5);
        assert!((finger.next_prompt_offset - 3.75).abs() < f32::EPSILON);
    }

    #[test]
    fn rc1_epl_integrates_gated_energy_over_a_traffic_frame() {
        let mut finger = test_nonpilot_finger();
        finger.epl_slew_windows_total = EPL_SLEW_WARMUP_WINDOWS;
        finger.epl_slew_windows_since = EPL_SLEW_MIN_WINDOWS_BETWEEN;

        for _ in 0..EPL_RC1_WINDOWS_PER_FRAME {
            finger.epl_window_coh4_pwr_early = 1.0;
            finger.epl_window_coh4_pwr_prompt = 1.0;
            finger.epl_window_coh4_pwr_late = 1.0;
            finger.epl_run_slew_loop();
        }
        assert_eq!(finger.epl_rc1_bias, Some(0.0));

        // Model an eighth-rate frame: one useful interval consistently says
        // the signal is late, followed by five gated noise intervals.
        for _ in 0..80 {
            for window in 0..EPL_RC1_WINDOWS_PER_FRAME {
                if window == 0 {
                    finger.epl_window_coh4_pwr_early = 0.85;
                    finger.epl_window_coh4_pwr_prompt = 1.00;
                    finger.epl_window_coh4_pwr_late = 1.10;
                } else {
                    finger.epl_window_coh4_pwr_early = 0.01;
                    finger.epl_window_coh4_pwr_prompt = 0.01;
                    finger.epl_window_coh4_pwr_late = 0.01;
                }
                finger.epl_run_slew_loop();
            }
            if finger.epl_slew_pending.is_some() {
                break;
            }
        }

        assert_eq!(finger.epl_slew_pending, Some(EPL_SLEW_STEP_SAMPLES));
        assert_eq!(finger.epl_slew_total, EPL_SLEW_STEP_SAMPLES as f64);
    }

    #[test]
    fn rc1_epl_fer_ignores_unclassified_phy_status_events() {
        let mut finger = test_nonpilot_finger();
        let mut status = SampleBlock::new(Vec::new(), 0);
        status.tags.insert("traffic_phy_status", 1);
        let mut invalid = SampleBlock::new(Vec::new(), 0);
        invalid.tags.insert("traffic_phy_status", 1);
        invalid.tags.insert("traffic_phy_valid", 0);
        let mut valid = SampleBlock::new(Vec::new(), 0);
        valid.tags.insert("traffic_phy_frame", 1);
        valid.tags.insert("traffic_phy_valid", 1);

        finger.epl_observe_nonpilot_phy(&[status, invalid, valid]);

        assert_eq!(finger.epl_nonpilot_phy_frames, 2);
        assert_eq!(finger.epl_nonpilot_phy_invalid, 1);
        assert_eq!(finger.epl_nonpilot_fer_frames, 2);
        assert_eq!(finger.epl_nonpilot_fer_bits, 0b10);
    }

    #[test]
    fn rc1_epl_freezes_when_recent_fer_exceeds_ten_percent() {
        let mut finger = test_nonpilot_finger();
        finger.epl_slew_windows_total = EPL_SLEW_WARMUP_WINDOWS;
        finger.epl_slew_windows_since = EPL_SLEW_MIN_WINDOWS_BETWEEN;
        finger.epl_rc1_bias = Some(0.0);
        finger.epl_slew_frac = 0.75;
        finger.epl_nonpilot_fer_frames = EPL_NONPILOT_FER_WINDOW_FRAMES;
        finger.epl_nonpilot_fer_bits = 0b11_1111;

        for _ in 0..EPL_RC1_WINDOWS_PER_FRAME {
            finger.epl_window_coh4_pwr_early = 2.0;
            finger.epl_window_coh4_pwr_prompt = 1.0;
            finger.epl_window_coh4_pwr_late = 0.5;
            finger.epl_run_slew_loop();
        }

        assert_eq!(finger.epl_slew_frac, 0.75);
        assert_eq!(finger.epl_slew_pending, None);
        assert_eq!(finger.epl_slew_total, 0.0);
    }
}
