use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cdma_common::diagnostics::power_control_verbose_summary_every;
use log::info;

/// Single 500ms-windowed power control measurement for the time-series
/// chart. Accumulated from per-PCG (800 Hz) inner-loop measurements.
#[derive(Debug, Clone)]
pub struct PowerControlHistoryEntry {
    /// Wall-clock timestamp at window close, Unix milliseconds.
    pub timestamp_ms: u64,
    /// Mean measured Eb/Nt (or pilot Ec/Nt for RC3) over the 500ms window.
    pub measured_mean_db: f32,
    /// Effective inner-loop target at window close.
    pub target_db: f32,
    /// Forward gain offset at window close.
    pub forward_gain_db: f32,
    /// Sliding-window FER percentage at window close.
    pub fer_pct: f32,
}

/// Snapshot of closed-loop power control state for a mobile's active
/// traffic channel — both reverse and forward loops. Surfaced via gRPC
/// for UI display.
#[derive(Debug, Clone)]
pub struct TrafficChannelPowerSnapshot {
    // ---- Reverse loop (BTS measures, sends PCBs) ---------------------
    /// Automatic outer-loop target Eb/Nt in dB.
    pub target_eb_nt_db: f32,
    /// Effective inner-loop target Eb/Nt in dB. Equals
    /// `target_eb_nt_db` in auto mode, or the operator-pinned manual
    /// target while an override is active.
    pub effective_target_eb_nt_db: f32,
    /// Operator-pinned reverse Eb/Nt target in dB. `None` when the
    /// channel is following the automatic outer loop.
    pub manual_target_override_db: Option<f32>,
    /// Most recent Eb/Nt estimate for each modulo-16 PCG slot, in dB.
    /// Updated live by per-PCG measurements and refreshed by full-frame
    /// snapshots when a decoded 20 ms frame is available.
    pub last_pcg_snr_db: Option<[f32; 16]>,
    /// Exact or best-available active-PCG mask for the most recent reverse
    /// traffic frame. `true` means the MS transmitted in that PCG.
    pub last_active_pcg_mask: Option<[bool; 16]>,
    /// Last PCB committed for each modulo-16 PCG slot on the absolute
    /// power-control timeline. `0` = UP, `1` = DOWN.
    pub last_pcbs: [u8; 16],
    /// Most recent reverse pilot Ec/Io estimate in dB from the validated
    /// reverse traffic finger. `None` when the receiver does not export it.
    pub reverse_pilot_ec_io_db: Option<f32>,
    pub fer_pct: f32,
    pub frames_total: u64,
    pub frames_crc_error: u64,
    // ---- Forward loop (mobile measures, sends PMRMs) -----------------
    /// Current forward-link gain offset in dB relative to the channel's
    /// initial allocation, walked by the outer loop.
    pub forward_gain_offset_db: f32,
    /// Most recent forward FER percentage reported by the mobile via
    /// PMRM. `None` before the first PMRM arrives.
    pub forward_last_fer_pct: Option<f32>,
    /// Raw counts from the most recent PMRM (errors / frames).
    pub forward_last_pmrm_errors: u32,
    pub forward_last_pmrm_frames: u32,
    /// Lifetime PMRM count for this channel.
    pub forward_pmrm_count: u64,
    /// Most recent Active Set pilot Ec/Io values (in dB) reported by
    /// the mobile in a PMRM, decoded from the raw 6-bit PILOT_STRENGTH
    /// fields via `-raw/2.0`. Empty if no PMRM received or the mobile
    /// reported 0 pilots.
    pub forward_pilot_ec_io_db: Vec<f32>,
    /// Per-PCG pilot Ec/Nt (RC3) or data Eb/Nt (RC1) used by the
    /// inner-loop power control. Updated live by per-PCG measurements.
    /// Separate from `last_pcg_snr_db` which carries the per-frame
    /// data Eb/Nt snapshot only.
    pub last_pcg_pilot_ec_nt_db: Option<[f32; 16]>,
    /// Reverse radio configuration (1 = RC1, 3 = RC3, etc.).
    pub reverse_radio_config: u32,
    /// Time-series history of 500ms-windowed power control measurements
    /// for the UI chart.
    pub power_history: Vec<PowerControlHistoryEntry>,
}

/// Closed-loop power control state for a single reverse traffic channel.
///
/// Production reverse-link control runs through `inner_loop_tick_single_pcg`,
/// which consumes one per-PCG Eb/Nt estimate and schedules one PCB on the
/// absolute-PCG TX timeline. The older frame-batched `inner_loop_tick` path is
/// retained for tests and reference, but it is no longer the live control path.
///
/// The outer loop remains a sliding FER tracker driven by decoded reverse
/// traffic frames. It adjusts the Eb/Nt target asymmetrically to chase a 1%
/// FER setpoint within the RC-specific target range.

#[derive(Debug, Clone, Default)]
pub(crate) struct PowerControlVerbosePcgCounters {
    pub(crate) total_ticks: u64,
    pub(crate) window_ticks: u64,
    pub(crate) window_up: u64,
    pub(crate) window_down: u64,
    pub(crate) window_eb_nt_sum_db: f64,
    pub(crate) window_control_metric_sum_db: f64,
    pub(crate) window_align_age_chips_sum: u64,
    pub(crate) window_align_age_max_chips: u64,
    pub(crate) window_ready_age_chips_sum: u64,
    pub(crate) window_ready_age_max_chips: u64,
    pub(crate) window_queue_wall_us_sum: u64,
    pub(crate) window_queue_wall_us_max: u64,
    pub(crate) window_over_align_delay: u64,
    pub(crate) window_over_ready_delay: u64,
    pub(crate) last_measure_abs_pcg: u64,
    pub(crate) last_tx_abs_pcg: u64,
}

impl PowerControlVerbosePcgCounters {
    pub(crate) fn record(
        &mut self,
        measured_abs_pcg: u64,
        tx_abs_pcg: u64,
        pcb: u8,
        eb_nt_db: f32,
        control_metric_db: f32,
        align_age_chips: u64,
        queue_wall_us: u64,
        ready_age_chips: u64,
        delay_pcgs: u64,
    ) -> bool {
        self.total_ticks = self.total_ticks.saturating_add(1);
        self.window_ticks = self.window_ticks.saturating_add(1);
        if pcb == 0 {
            self.window_up = self.window_up.saturating_add(1);
        } else {
            self.window_down = self.window_down.saturating_add(1);
        }
        self.window_eb_nt_sum_db += eb_nt_db as f64;
        self.window_control_metric_sum_db += control_metric_db as f64;
        self.window_align_age_chips_sum = self
            .window_align_age_chips_sum
            .saturating_add(align_age_chips);
        self.window_align_age_max_chips = self.window_align_age_max_chips.max(align_age_chips);
        self.window_ready_age_chips_sum = self
            .window_ready_age_chips_sum
            .saturating_add(ready_age_chips);
        self.window_ready_age_max_chips = self.window_ready_age_max_chips.max(ready_age_chips);
        self.window_queue_wall_us_sum = self.window_queue_wall_us_sum.saturating_add(queue_wall_us);
        self.window_queue_wall_us_max = self.window_queue_wall_us_max.max(queue_wall_us);
        if delay_pcgs > 0 && align_age_chips >= delay_pcgs.saturating_mul(1536) {
            self.window_over_align_delay = self.window_over_align_delay.saturating_add(1);
        }
        if delay_pcgs > 0 && ready_age_chips >= delay_pcgs.saturating_mul(1536) {
            self.window_over_ready_delay = self.window_over_ready_delay.saturating_add(1);
        }
        self.last_measure_abs_pcg = measured_abs_pcg;
        self.last_tx_abs_pcg = tx_abs_pcg;
        self.window_ticks >= power_control_verbose_summary_every()
    }

    pub(crate) fn log_and_reset(
        &mut self,
        walsh_code: u8,
        effective_target_db: f32,
        auto_target_db: f32,
        mode: &str,
        fer_window_pct: f32,
        fer_lifetime_pct: f32,
        total_frames_received: u64,
        total_frames_crc_error: u64,
        active_mask: String,
    ) {
        if self.window_ticks == 0 {
            return;
        }
        let avg_raw_ec_io_db = self.window_eb_nt_sum_db / self.window_ticks as f64;
        let avg_control_metric_db = self.window_control_metric_sum_db / self.window_ticks as f64;
        let align_age_avg_pcgs =
            self.window_align_age_chips_sum as f64 / self.window_ticks as f64 / 1536.0;
        let align_age_max_pcgs = self.window_align_age_max_chips as f64 / 1536.0;
        let ready_age_avg_pcgs =
            self.window_ready_age_chips_sum as f64 / self.window_ticks as f64 / 1536.0;
        let ready_age_max_pcgs = self.window_ready_age_max_chips as f64 / 1536.0;
        let queue_avg_us = self.window_queue_wall_us_sum as f64 / self.window_ticks as f64;
        info!(
            "BSC: [power counters walsh={}] pcg_ticks_total={} window_ticks={} up={} down={} raw_ec_io_avg={:.2}(all_pcgs) control_avg={:.2}(active_only) target={:.2} auto_target={:.2} mode={} align_age_avg_pcgs={:.2} align_age_max_pcgs={:.2} ready_age_avg_pcgs={:.2} ready_age_max_pcgs={:.2} queue_avg_us={:.0} queue_max_us={} over_align_delay={} over_ready_delay={} last_measure_abs_pcg={} last_tx_abs_pcg={} active_mask={} fer_window={:.2}% fer_lifetime={:.2}% frames={} errors={}",
            walsh_code,
            self.total_ticks,
            self.window_ticks,
            self.window_up,
            self.window_down,
            avg_raw_ec_io_db,
            avg_control_metric_db,
            effective_target_db,
            auto_target_db,
            mode,
            align_age_avg_pcgs,
            align_age_max_pcgs,
            ready_age_avg_pcgs,
            ready_age_max_pcgs,
            queue_avg_us,
            self.window_queue_wall_us_max,
            self.window_over_align_delay,
            self.window_over_ready_delay,
            self.last_measure_abs_pcg,
            self.last_tx_abs_pcg,
            active_mask,
            fer_window_pct,
            fer_lifetime_pct,
            total_frames_received,
            total_frames_crc_error,
        );
        self.window_ticks = 0;
        self.window_up = 0;
        self.window_down = 0;
        self.window_eb_nt_sum_db = 0.0;
        self.window_control_metric_sum_db = 0.0;
        self.window_align_age_chips_sum = 0;
        self.window_align_age_max_chips = 0;
        self.window_ready_age_chips_sum = 0;
        self.window_ready_age_max_chips = 0;
        self.window_queue_wall_us_sum = 0;
        self.window_queue_wall_us_max = 0;
        self.window_over_align_delay = 0;
        self.window_over_ready_delay = 0;
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PowerControlVerboseFrameCounters {
    pub(crate) total_frames: u64,
    pub(crate) window_frames: u64,
    pub(crate) window_valid: u64,
    pub(crate) window_invalid: u64,
    pub(crate) window_signaling: u64,
    pub(crate) window_target_up: u64,
    pub(crate) window_target_down: u64,
    pub(crate) window_target_same: u64,
}

impl PowerControlVerboseFrameCounters {
    pub(crate) fn summary_interval() -> u64 {
        (power_control_verbose_summary_every() / 16).max(1)
    }

    pub(crate) fn record(
        &mut self,
        frame_valid: bool,
        signaling: bool,
        target_before: f32,
        target_after: f32,
    ) -> bool {
        self.total_frames = self.total_frames.saturating_add(1);
        self.window_frames = self.window_frames.saturating_add(1);
        if frame_valid {
            self.window_valid = self.window_valid.saturating_add(1);
        } else {
            self.window_invalid = self.window_invalid.saturating_add(1);
        }
        if signaling {
            self.window_signaling = self.window_signaling.saturating_add(1);
        }
        if target_after > target_before {
            self.window_target_up = self.window_target_up.saturating_add(1);
        } else if target_after < target_before {
            self.window_target_down = self.window_target_down.saturating_add(1);
        } else {
            self.window_target_same = self.window_target_same.saturating_add(1);
        }
        self.window_frames >= Self::summary_interval()
    }

    pub(crate) fn log_and_reset(
        &mut self,
        walsh_code: u8,
        target_before: f32,
        target_after: f32,
        mode: &str,
        fer_window_pct: f32,
        fer_lifetime_pct: f32,
        frame_metric: Option<f32>,
        active_mask: String,
        primary_rate: u32,
    ) {
        if self.window_frames == 0 {
            return;
        }
        let frame_metric = frame_metric
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());
        let fer_log_window_pct = if self.window_frames > 0 {
            100.0 * self.window_invalid as f32 / self.window_frames as f32
        } else {
            0.0
        };
        info!(
            "BSC: [power frame counters walsh={}] total_frames={} window_frames={} valid={} invalid={} signaling={} target_up={} target_down={} target_same={} target={:.2}->{:.2} mode={} fer_log_window={:.2}% fer_window={:.2}% fer_lifetime={:.2}% frame_metric={} primary_rate={} active_mask={}",
            walsh_code,
            self.total_frames,
            self.window_frames,
            self.window_valid,
            self.window_invalid,
            self.window_signaling,
            self.window_target_up,
            self.window_target_down,
            self.window_target_same,
            target_before,
            target_after,
            mode,
            fer_log_window_pct,
            fer_window_pct,
            fer_lifetime_pct,
            frame_metric,
            primary_rate,
            active_mask,
        );
        self.window_frames = 0;
        self.window_valid = 0;
        self.window_invalid = 0;
        self.window_signaling = 0;
        self.window_target_up = 0;
        self.window_target_down = 0;
        self.window_target_same = 0;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PowerControlState {
    /// Outer-loop target Eb/Nt in dB, adjusted by the FER tracker.
    pub(crate) target_eb_nt_db: f32,
    /// Operator-pinned reverse Eb/Nt target in dB. When present, the
    /// inner loop compares against this manual target and the automatic
    /// outer loop stops adjusting `target_eb_nt_db` until the override
    /// is cleared.
    pub(crate) manual_target_override_db: Option<f32>,
    /// Automatic outer-loop target bounds. These apply only when the
    /// controller is in auto mode.
    pub(crate) auto_target_min_db: f32,
    pub(crate) auto_target_max_db: f32,
    /// Operator-facing manual override bounds. These are intentionally
    /// wider than the automatic range so a pinned target can be used as
    /// a diagnostic forcing function without widening the auto loop.
    pub(crate) manual_target_min_db: f32,
    pub(crate) manual_target_max_db: f32,
    /// Most recent per-PCG Eb/Nt values (16 entries, one per PCG) from
    /// the last decoded traffic frame. `None` until the first frame
    /// arrives.
    pub(crate) last_pcg_snr_db: Option<[f32; 16]>,
    /// Per-PCG inner-loop control metric: pilot Ec/Nt for RC3, data
    /// Eb/Nt for RC1. Updated live by `inner_loop_tick_single_pcg`.
    /// Separate from `last_pcg_snr_db` which is overwritten by the
    /// per-frame data Eb/Nt snapshot.
    pub(crate) last_pcg_pilot_ec_nt_db: Option<[f32; 16]>,
    /// Most recent reverse pilot Ec/Io estimate in dB from the reverse
    /// traffic receiver, when available.
    pub(crate) reverse_pilot_ec_io_db: Option<f32>,
    /// Exact or best-available active-PCG mask from the most recent decoded
    /// reverse traffic frame.
    pub(crate) last_active_pcg_mask: Option<[bool; 16]>,
    /// Short filtered per-PCG control metric. The live loop uses this
    /// rather than the raw instantaneous Eb/Nt estimate so small
    /// estimation noise does not flip the command sign every PCG.
    pub(crate) filtered_pcg_metric_db: Option<f32>,
    /// PCBs queued for injection into the *next* outgoing F-FCH frame.
    /// 16 entries, one per Power Control Group (1.25 ms each).
    pub(crate) pending_pcbs: [u8; 16],
    /// Most recent committed PCBs (already pushed to the framer). Kept for
    /// UI display so the operator can see the up/down pattern.
    pub(crate) last_committed_pcbs: [u8; 16],
    /// Sliding window of CRC-pass(true)/CRC-fail(false) results.
    pub(crate) crc_window: VecDeque<bool>,
    pub(crate) total_frames_received: u64,
    pub(crate) total_frames_crc_valid: u64,
    pub(crate) total_frames_crc_error: u64,
    /// Most recent FER over `crc_window`, as a percentage (0..100).
    pub(crate) last_fer_pct: f32,
    /// Learned automatic target floor. Raised when bad frames occur at the
    /// current floor, then decayed slowly after sustained clean operation.
    pub(crate) adaptive_auto_floor_db: Option<f32>,
    adaptive_floor_clean_frames: u64,
    /// Residual desired closed-loop change, in dB, carried across frame ticks.
    /// Because 16 PCBs can only realize an even net dB delta over a frame,
    /// we accumulate fractional / odd desired deltas here and quantize them
    /// over time.
    pub(crate) residual_frame_delta_db: f32,
    /// Aggregate Eb/Nt metric used for the most recent frame-level inner-loop
    /// update. Computed from the strongest active PCGs of the frame.
    pub(crate) last_frame_metric_db: Option<f32>,
    /// Rotating phase used when distributing UP/DOWN commands across the 16
    /// PCGs so corrections do not always land on the same PCG indices.
    pub(crate) distribution_phase: usize,
    /// Periodic log throttle — only print a status line every Nth
    /// inner-loop tick to avoid spamming the log on every frame.
    pub(crate) inner_ticks_since_last_log: u64,
    /// Sigma-delta residual for the per-PCG controller. Because the
    /// handset only supports ±1 dB changes per valid PCB, this residual
    /// lets us realize finer average motion by duty-cycling UP/DOWN
    /// commands over time.
    pub(crate) pcg_command_residual_db: f32,
    pub(crate) verbose_pcg_counters: PowerControlVerbosePcgCounters,
    pub(crate) verbose_frame_counters: PowerControlVerboseFrameCounters,
    // ---- 500ms history accumulator for UI chart -----------------------
    /// Running sum of finite Eb/Nt measurements in the current 500ms window.
    history_window_sum_db: f64,
    /// Number of finite measurements accumulated in the current window.
    history_window_count: u32,
    /// Absolute PCG at which the current 500ms window started. `None`
    /// until the first measurement arrives.
    history_window_start_pcg: Option<u64>,
    /// Rolling buffer of completed 500ms windows (up to 5 min = 600 entries).
    pub(crate) power_history: VecDeque<PowerControlHistoryEntry>,
    /// Cached forward gain offset from `ForwardPowerControlState`, updated
    /// by the BSC before each inner-loop tick so the history entry captures
    /// the forward gain at window close without needing cross-struct access.
    pub(crate) cached_forward_gain_db: f32,
    /// Optional RC3 startup state. While active, the inner loop emits
    /// alternating UP/DOWN PCBs and collects valid PCG metrics before seeding
    /// the automatic target from their average.
    startup_seed: Option<StartupTargetSeedState>,
}

#[derive(Debug, Clone)]
struct StartupTargetSeedState {
    required_measurements: usize,
    samples: Vec<f32>,
    seeded: bool,
}

impl StartupTargetSeedState {
    fn new(required_measurements: usize) -> Self {
        Self {
            required_measurements,
            samples: Vec::with_capacity(required_measurements),
            seeded: false,
        }
    }

    fn push(&mut self, measurement_db: f32) {
        if self.samples.len() < self.required_measurements {
            self.samples.push(measurement_db);
        }
    }

    fn ready(&self) -> bool {
        !self.seeded && self.samples.len() >= self.required_measurements
    }

    fn robust_mean_db(&self) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut values = self.samples.clone();
        values.sort_by(|a, b| a.total_cmp(b));
        let trim = (values.len() / 8).min(values.len().saturating_sub(1) / 2);
        let kept = &values[trim..values.len() - trim];
        Some(kept.iter().copied().sum::<f32>() / kept.len() as f32)
    }
}

impl PowerControlState {
    /// Number of frames retained in the FER sliding window (monitoring only;
    /// the outer-loop control action is per-frame, not threshold-based).
    pub(crate) const FER_WINDOW: usize = 50;
    /// Minimum CRC-valid frames before the outer loop engages. During
    /// channel startup the receiver may not have locked yet, producing
    /// meaningless frame errors that would ramp the target to max.
    /// Suppress outer-loop adjustments until this many good frames confirm
    /// the channel is actually decodable.
    const OUTER_LOOP_MIN_VALID_FRAMES: u64 = 10;
    /// Target FER the outer loop converges to (percent).  The per-frame
    /// step-down is derived automatically from `TARGET_STEP_UP_DB` so
    /// that at equilibrium the up and down adjustments cancel:
    ///   step_down = step_up × (target_fer / (1 - target_fer))
    pub(crate) const TARGET_FER_PCT: f32 = 1.0;
    /// Legacy constant kept for display; no longer drives control action.
    pub(crate) const CLEAN_FER_PCT: f32 = 0.5;
    /// Per-erasure Eb/Nt target increase.  On every CRC-invalid frame the
    /// outer loop raises the target by this amount.  With 1% target FER
    /// the derived step-down is ~0.00505 dB per good frame, giving
    /// ~0.25 dB/sec descent at 0% FER and ~3 dB recovery in 6 erasures.
    pub(crate) const TARGET_STEP_UP_DB: f32 = 0.5;
    /// If an erasure occurs this close to the active automatic floor, treat the
    /// floor as too low and learn a higher one.
    const ADAPTIVE_FLOOR_TRIGGER_BAND_DB: f32 = 0.25;
    /// Small upward probe used for floor-near erasures.  Keep this below the
    /// normal erasure step so a single floor touch does not overshoot the
    /// eventual steady operating point.
    const ADAPTIVE_FLOOR_STEP_UP_DB: f32 = 0.1;
    /// Clean frames required before probing the learned floor downward again.
    const ADAPTIVE_FLOOR_DECAY_FRAMES: u64 = 250;
    /// Downward probe step for the learned floor after a clean interval.
    const ADAPTIVE_FLOOR_DECAY_STEP_DB: f32 = 0.1;
    /// Inner-loop proportional gain converting frame-level Eb/Nt error to a
    /// desired net dB change for the next 20 ms worth of PCBs.
    const FRAME_RESPONSE_GAIN_DB_PER_DB: f32 = 0.5;
    /// Bound the requested net change per frame so one noisy estimate cannot
    /// demand an unrealistic burst of identical PCBs.
    const FRAME_DELTA_CLAMP_DB: f32 = 4.0;
    /// Log a status line roughly once per second in the per-PCG loop.
    const LOG_EVERY_N_TICKS: u64 = 800;
    /// First-order filter weight for per-PCG Eb/Nt control.
    /// 0.30 → time constant ~3.3 PCGs (~4 ms), faster convergence after
    /// rate changes and step inputs while still rejecting per-PCG noise.
    const PCG_METRIC_FILTER_ALPHA: f32 = 0.05;
    /// Do not react to filtered errors smaller than this band; the
    /// sigma-delta residual will still realize a neutral alternating
    /// pattern, which keeps the average change at 0 dB/PCG.
    const PCG_HOLD_BAND_DB: f32 = 0.15;
    /// Convert filtered Eb/Nt error into a desired average dB change
    /// per valid PCB. A value below 1 keeps the loop from saturating on
    /// modest errors while still allowing full-rate recovery on large
    /// misses.
    const PCG_RESPONSE_GAIN_DB_PER_DB: f32 = 0.5;
    /// Bound the desired average dB change per PCG to what one PCB can
    /// physically realize.
    const PCG_DESIRED_STEP_CLAMP_DB: f32 = 1.0;
    /// Prevent the sigma-delta accumulator from winding up if the
    /// handset is pinned or measurements go stale.
    const PCG_RESIDUAL_CLAMP_DB: f32 = 2.0;
    /// Number of PCGs per 100ms history window (100 / 1.25 = 80).
    const HISTORY_WINDOW_PCGS: u64 = 80;
    /// Maximum history entries retained (~5 minutes at 100ms per entry).
    const HISTORY_MAX_ENTRIES: usize = 3000;
    /// RC3 startup hold window. 32 PCGs is 40 ms at 1.25 ms/PCG.
    const RC3_STARTUP_SEED_PCGS: usize = 32;

    /// Create a PowerControlState with separate automatic and manual
    /// target ranges.
    pub(crate) fn with_params(
        initial_target_db: f32,
        auto_target_min_db: f32,
        auto_target_max_db: f32,
        manual_target_min_db: f32,
        manual_target_max_db: f32,
    ) -> Self {
        Self {
            target_eb_nt_db: initial_target_db,
            manual_target_override_db: None,
            auto_target_min_db,
            auto_target_max_db,
            manual_target_min_db,
            manual_target_max_db,
            last_pcg_snr_db: None,
            last_pcg_pilot_ec_nt_db: None,
            reverse_pilot_ec_io_db: None,
            last_active_pcg_mask: None,
            filtered_pcg_metric_db: None,
            pending_pcbs: [0u8; 16],
            last_committed_pcbs: [0u8; 16],
            crc_window: VecDeque::with_capacity(Self::FER_WINDOW),
            total_frames_received: 0,
            total_frames_crc_valid: 0,
            total_frames_crc_error: 0,
            last_fer_pct: 0.0,
            adaptive_auto_floor_db: None,
            adaptive_floor_clean_frames: 0,
            residual_frame_delta_db: 0.0,
            last_frame_metric_db: None,
            distribution_phase: 0,
            inner_ticks_since_last_log: 0,
            pcg_command_residual_db: 0.0,
            verbose_pcg_counters: PowerControlVerbosePcgCounters::default(),
            verbose_frame_counters: PowerControlVerboseFrameCounters::default(),
            history_window_sum_db: 0.0,
            history_window_count: 0,
            history_window_start_pcg: None,
            power_history: VecDeque::with_capacity(Self::HISTORY_MAX_ENTRIES),
            cached_forward_gain_db: 0.0,
            startup_seed: None,
        }
    }

    /// RC1 defaults: keep the automatic loop tightly bounded while
    /// allowing a wider manual diagnostic range.
    pub(crate) fn new_rc1() -> Self {
        Self::with_params(10.0, 8.0, 12.0, 0.0, 40.0)
    }

    /// RC3 defaults: inner loop targets pilot Ec/Io (per-symbol coherent
    /// pilot energy over per-chip wideband power).  This metric includes
    /// the Walsh-16 processing gain (+12 dB above textbook per-chip Ec/Io).
    ///
    /// Operating points from live captures:
    ///   ~2 dB  → ~2-3% FER (aggressive)
    ///   ~5 dB  → ~0.1% FER (comfortable)
    ///
    /// Initial -5 dB, auto range [-10, -3] dB, wide manual range for
    /// diagnostics. Startup holds the MS with alternating PCBs for the first
    /// 32 valid PCGs, then seeds the automatic target from a trimmed average
    /// clamped to the auto range.
    pub(crate) fn new_rc3() -> Self {
        let mut state = Self::with_params(-5.0, -10.0, -3.0, -15.0, 40.0);
        state.startup_seed = Some(StartupTargetSeedState::new(Self::RC3_STARTUP_SEED_PCGS));
        state
    }

    /// Backward-compatible constructor using RC1 defaults.
    pub(crate) fn new() -> Self {
        Self::new_rc1()
    }

    pub(crate) fn effective_target_eb_nt_db(&self) -> f32 {
        self.manual_target_override_db
            .unwrap_or(self.target_eb_nt_db)
    }

    pub(crate) fn effective_auto_target_min_db(&self) -> f32 {
        self.adaptive_auto_floor_db
            .unwrap_or(self.auto_target_min_db)
            .clamp(self.auto_target_min_db, self.auto_target_max_db)
    }

    /// Per-good-frame Eb/Nt target decrease, derived from
    /// `TARGET_STEP_UP_DB` and `TARGET_FER_PCT` so that the outer loop
    /// converges to exactly `TARGET_FER_PCT`:
    ///   step_down = step_up × (fer / (1 - fer))
    pub(crate) fn target_step_down_db() -> f32 {
        let fer_frac = Self::TARGET_FER_PCT / 100.0;
        Self::TARGET_STEP_UP_DB * fer_frac / (1.0 - fer_frac)
    }

    fn floor_near_erasure(&self) -> bool {
        let floor = self.effective_auto_target_min_db();
        self.target_eb_nt_db <= floor + Self::ADAPTIVE_FLOOR_TRIGGER_BAND_DB
    }

    fn learn_adaptive_floor_from_erasure(&mut self) {
        let learned = (self.target_eb_nt_db + Self::ADAPTIVE_FLOOR_STEP_UP_DB)
            .clamp(self.auto_target_min_db, self.auto_target_max_db);
        let next_floor = self
            .adaptive_auto_floor_db
            .map(|current| current.max(learned))
            .unwrap_or(learned);
        let previous_floor = self.effective_auto_target_min_db();
        self.adaptive_auto_floor_db = (next_floor > self.auto_target_min_db).then_some(next_floor);
        if next_floor > previous_floor {
            info!(
                "BSC: reverse power adaptive floor raised target={:.2} floor={:.2}->{:.2}",
                self.target_eb_nt_db, previous_floor, next_floor
            );
        }
    }

    fn decay_adaptive_floor_after_clean_frames(&mut self) {
        if self.adaptive_floor_clean_frames < Self::ADAPTIVE_FLOOR_DECAY_FRAMES {
            return;
        }
        self.adaptive_floor_clean_frames = 0;

        if let Some(floor) = self.adaptive_auto_floor_db {
            let decayed = (floor - Self::ADAPTIVE_FLOOR_DECAY_STEP_DB).max(self.auto_target_min_db);
            self.adaptive_auto_floor_db = (decayed > self.auto_target_min_db).then_some(decayed);
            info!(
                "BSC: reverse power adaptive floor decayed floor={:.2}->{:.2}",
                floor,
                self.effective_auto_target_min_db()
            );
        }
    }

    pub(crate) fn control_mode_label(&self) -> &'static str {
        if self.manual_target_override_db.is_some() {
            "manual"
        } else {
            "auto"
        }
    }

    pub(crate) fn set_manual_target_override_db(&mut self, target_db: f32) -> f32 {
        let clamped = target_db.clamp(self.manual_target_min_db, self.manual_target_max_db);
        self.manual_target_override_db = Some(clamped);
        if let Some(seed) = self.startup_seed.as_mut() {
            seed.seeded = true;
        }
        self.residual_frame_delta_db = 0.0;
        self.reset_live_pcg_controller_state();
        clamped
    }

    pub(crate) fn clear_manual_target_override_db(&mut self) -> Option<f32> {
        let pinned = self.manual_target_override_db.take();
        if let Some(target_db) = pinned {
            self.target_eb_nt_db =
                target_db.clamp(self.effective_auto_target_min_db(), self.auto_target_max_db);
        }
        self.residual_frame_delta_db = 0.0;
        self.reset_live_pcg_controller_state();
        pinned
    }

    pub(crate) fn reset_live_pcg_controller_state(&mut self) {
        self.filtered_pcg_metric_db = None;
        self.pcg_command_residual_db = 0.0;
    }

    fn record_history_measurement(
        &mut self,
        abs_pcg: u64,
        measurement_valid: bool,
        eb_nt_db: f32,
        target_db: f32,
    ) {
        if measurement_valid {
            self.history_window_sum_db += eb_nt_db as f64;
            self.history_window_count += 1;
        }
        let window_start = *self.history_window_start_pcg.get_or_insert(abs_pcg);
        if abs_pcg.saturating_sub(window_start) >= Self::HISTORY_WINDOW_PCGS {
            if self.history_window_count > 0 {
                let mean = (self.history_window_sum_db / self.history_window_count as f64) as f32;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if self.power_history.len() >= Self::HISTORY_MAX_ENTRIES {
                    self.power_history.pop_front();
                }
                self.power_history.push_back(PowerControlHistoryEntry {
                    timestamp_ms: now_ms,
                    measured_mean_db: mean,
                    target_db,
                    forward_gain_db: self.cached_forward_gain_db,
                    fer_pct: self.last_fer_pct,
                });
            }
            self.history_window_sum_db = 0.0;
            self.history_window_count = 0;
            self.history_window_start_pcg = Some(abs_pcg);
        }
    }

    pub(crate) fn format_active_pcg_mask(mask: Option<[bool; 16]>) -> String {
        mask.map(|bits| {
            bits.iter()
                .map(|active| if *active { '1' } else { '0' })
                .collect()
        })
        .unwrap_or_else(|| "????????????????".to_string())
    }

    pub(crate) fn alternating_pcbs(&self) -> [u8; 16] {
        let start = (self.distribution_phase & 1) as u8;
        core::array::from_fn(|idx| (start ^ ((idx & 1) as u8)) & 1)
    }

    pub(crate) fn fallback_pcb_for_abs_pcg(abs_pcg: u64) -> u8 {
        (abs_pcg as u8) & 1
    }

    pub(crate) fn record_frame_measurements(
        &mut self,
        pcg_snr_db: [f32; 16],
        active_pcg_mask: Option<[bool; 16]>,
    ) {
        self.last_pcg_snr_db = Some(pcg_snr_db);
        self.last_active_pcg_mask = active_pcg_mask;
        let active_indices: Vec<usize> = active_pcg_mask
            .map(|mask| {
                mask.iter()
                    .enumerate()
                    .filter_map(|(idx, active)| active.then_some(idx))
                    .collect()
            })
            .unwrap_or_else(|| (0..16).collect());
        self.last_frame_metric_db = if active_indices.is_empty() {
            None
        } else {
            Some(
                active_indices
                    .iter()
                    .map(|&idx| pcg_snr_db[idx])
                    .fold(f32::NEG_INFINITY, f32::max),
            )
        };
    }

    pub(crate) fn inner_loop_tick_single_pcg(
        &mut self,
        walsh_code: u8,
        abs_pcg: u64,
        eb_nt_db: f32,
    ) -> u8 {
        let pcg_slot = (abs_pcg % 16) as usize;
        self.inner_ticks_since_last_log += 1;

        // Gated PCGs (reverse pilot channel gating) have no mobile signal —
        // the Ec/Io measurement is just noise.  Treat them like non-finite:
        // hold the IIR filter at the last active-PCG value but keep the
        // sigma-delta quantizer ticking so the loop continues driving
        // toward the target during gated slots.
        let pcg_is_gated = self
            .last_active_pcg_mask
            .map(|mask| !mask[pcg_slot])
            .unwrap_or(false);
        let measurement_valid = eb_nt_db.is_finite() && !pcg_is_gated;

        // Only store valid (non-gated) measurements in the per-PCG array
        // exported to the UI.  Gated PCGs are explicitly set to NaN so
        // the UI average isn't dragged down by noise-floor garbage
        // (including stale values from before the gating mask was known).
        let mut pilot_arr = self.last_pcg_pilot_ec_nt_db.unwrap_or([f32::NAN; 16]);
        if measurement_valid {
            pilot_arr[pcg_slot] = eb_nt_db;
        } else if pcg_is_gated {
            pilot_arr[pcg_slot] = f32::NAN;
        }
        self.last_pcg_pilot_ec_nt_db = Some(pilot_arr);

        let mut startup_pcb = None;
        let mut startup_seeded = None;
        if self.manual_target_override_db.is_none() {
            let auto_target_min_db = self.auto_target_min_db;
            let auto_target_max_db = self.auto_target_max_db;
            if let Some(seed) = self.startup_seed.as_mut()
                && !seed.seeded
            {
                if measurement_valid {
                    seed.push(eb_nt_db);
                    if seed.ready() {
                        if let Some(mean_db) = seed.robust_mean_db() {
                            let target_db = mean_db.clamp(auto_target_min_db, auto_target_max_db);
                            startup_seeded = Some((mean_db, target_db, seed.samples.len()));
                        }
                        seed.seeded = true;
                    }
                }
                startup_pcb = Some(Self::fallback_pcb_for_abs_pcg(abs_pcg));
            }
        }
        if let Some((mean_db, target_db, sample_count)) = startup_seeded {
            self.target_eb_nt_db = target_db;
            self.filtered_pcg_metric_db = Some(target_db);
            self.pcg_command_residual_db = 0.0;
            log::info!(
                "power_control: seeded RC3 startup target from {} PCGs: mean={:.2} dB target={:.2} dB (clamped to [{:.1}, {:.1}])",
                sample_count,
                mean_db,
                target_db,
                self.auto_target_min_db,
                self.auto_target_max_db,
            );
        }
        if let Some(pcb) = startup_pcb {
            self.last_committed_pcbs[pcg_slot] = pcb;
            self.last_frame_metric_db = measurement_valid.then_some(eb_nt_db);
            self.record_history_measurement(
                abs_pcg,
                measurement_valid,
                eb_nt_db,
                self.effective_target_eb_nt_db(),
            );
            return pcb;
        }

        let effective_target_db = self.effective_target_eb_nt_db();
        let (pcb, control_metric_db, control_error_db, residual_db) = if !measurement_valid {
            // No valid input — hold the IIR but keep the sigma-delta
            // ticking from the held control error so the loop continues
            // converging toward the target.
            let held_metric = self.filtered_pcg_metric_db.unwrap_or(f32::NAN);
            if held_metric.is_finite() {
                let filtered_error_db = effective_target_db - held_metric;
                let effective_error_db = if filtered_error_db.abs() <= Self::PCG_HOLD_BAND_DB {
                    0.0
                } else {
                    filtered_error_db - Self::PCG_HOLD_BAND_DB * filtered_error_db.signum()
                };
                let desired_step_db = (effective_error_db * Self::PCG_RESPONSE_GAIN_DB_PER_DB)
                    .clamp(
                        -Self::PCG_DESIRED_STEP_CLAMP_DB,
                        Self::PCG_DESIRED_STEP_CLAMP_DB,
                    );
                let mut residual_db = (self.pcg_command_residual_db + desired_step_db)
                    .clamp(-Self::PCG_RESIDUAL_CLAMP_DB, Self::PCG_RESIDUAL_CLAMP_DB);
                let (pcb, applied_step_db) = if residual_db >= 0.0 {
                    (0, 1.0)
                } else {
                    (1, -1.0)
                };
                residual_db = (residual_db - applied_step_db)
                    .clamp(-Self::PCG_RESIDUAL_CLAMP_DB, Self::PCG_RESIDUAL_CLAMP_DB);
                self.pcg_command_residual_db = residual_db;
                (pcb, held_metric, filtered_error_db, residual_db)
            } else {
                // No prior valid measurement at all — pure fallback.
                (
                    Self::fallback_pcb_for_abs_pcg(abs_pcg),
                    f32::NAN,
                    f32::NAN,
                    self.pcg_command_residual_db,
                )
            }
        } else {
            let first_measurement = self.filtered_pcg_metric_db.is_none();
            let filtered_metric_db = match self.filtered_pcg_metric_db {
                Some(prev) => prev + Self::PCG_METRIC_FILTER_ALPHA * (eb_nt_db - prev),
                None => eb_nt_db,
            };
            self.filtered_pcg_metric_db = Some(filtered_metric_db);

            // Legacy non-startup path: seed the auto target from the first
            // valid measurement so the inner loop begins balanced.
            if first_measurement
                && self.manual_target_override_db.is_none()
                && self.startup_seed.is_none()
            {
                let seeded = eb_nt_db.clamp(self.auto_target_min_db, self.auto_target_max_db);
                log::info!(
                    "power_control: seeding initial target from first measurement: {:.2} dB (clamped to [{:.1}, {:.1}])",
                    seeded,
                    self.auto_target_min_db,
                    self.auto_target_max_db,
                );
                self.target_eb_nt_db = seeded;
            }

            let filtered_error_db = effective_target_db - filtered_metric_db;
            let effective_error_db = if filtered_error_db.abs() <= Self::PCG_HOLD_BAND_DB {
                0.0
            } else {
                filtered_error_db - Self::PCG_HOLD_BAND_DB * filtered_error_db.signum()
            };
            let desired_step_db = (effective_error_db * Self::PCG_RESPONSE_GAIN_DB_PER_DB).clamp(
                -Self::PCG_DESIRED_STEP_CLAMP_DB,
                Self::PCG_DESIRED_STEP_CLAMP_DB,
            );
            let mut residual_db = (self.pcg_command_residual_db + desired_step_db)
                .clamp(-Self::PCG_RESIDUAL_CLAMP_DB, Self::PCG_RESIDUAL_CLAMP_DB);
            let (pcb, applied_step_db) = if residual_db >= 0.0 {
                (0, 1.0)
            } else {
                (1, -1.0)
            };
            residual_db = (residual_db - applied_step_db)
                .clamp(-Self::PCG_RESIDUAL_CLAMP_DB, Self::PCG_RESIDUAL_CLAMP_DB);
            self.pcg_command_residual_db = residual_db;
            (pcb, filtered_metric_db, filtered_error_db, residual_db)
        };
        self.last_committed_pcbs[pcg_slot] = pcb;
        self.last_frame_metric_db = Some(control_metric_db);

        if self.inner_ticks_since_last_log >= Self::LOG_EVERY_N_TICKS {
            self.inner_ticks_since_last_log = 0;
            let dir = if pcb == 0 { "up" } else { "down" };
            let active_mask = Self::format_active_pcg_mask(self.last_active_pcg_mask);
            info!(
                "BSC: [power walsh={}] mode={} abs_pcg={} slot={} raw_metric={:.2} dB control_metric={:.2} dB target={:.2} dB error={:+.2} dB residual={:+.2} dB pcb={}({}) active_mask={} fer_window={:.2}% fer_lifetime={:.2}% frames={} errors={}",
                walsh_code,
                self.control_mode_label(),
                abs_pcg,
                pcg_slot,
                eb_nt_db,
                control_metric_db,
                effective_target_db,
                control_error_db,
                residual_db,
                pcb,
                dir,
                active_mask,
                self.last_fer_pct,
                self.lifetime_fer_pct(),
                self.total_frames_received,
                self.total_frames_crc_error,
            );
        }

        self.record_history_measurement(abs_pcg, measurement_valid, eb_nt_db, effective_target_db);

        pcb
    }

    pub(crate) fn distribute_pcbs(
        &self,
        up_count: usize,
        active_mask: Option<[bool; 16]>,
    ) -> [u8; 16] {
        let mut out = self.alternating_pcbs();
        let active_indices: Vec<usize> = active_mask
            .map(|mask| {
                mask.iter()
                    .enumerate()
                    .filter_map(|(idx, active)| active.then_some(idx))
                    .collect()
            })
            .unwrap_or_else(|| (0..16).collect());
        let slot_count = active_indices.len();

        if slot_count == 0 {
            return out;
        }
        if up_count == 0 {
            for idx in active_indices {
                out[idx] = 1;
            }
            return out;
        }
        if up_count >= slot_count {
            for idx in active_indices {
                out[idx] = 0;
            }
            return out;
        }

        let mut accumulator = self.distribution_phase % slot_count;
        for idx in active_indices {
            accumulator += up_count;
            if accumulator >= slot_count {
                out[idx] = 0;
                accumulator -= slot_count;
            } else {
                out[idx] = 1;
            }
        }
        out
    }

    pub(crate) fn quantize_delta_toward_zero(delta_db: f32, slot_count: usize) -> i32 {
        if slot_count == 0 {
            return 0;
        }

        let mut best = -(slot_count as i32);
        let mut best_diff = (delta_db - best as f32).abs();
        for candidate in (-(slot_count as i32) + 1)..=(slot_count as i32) {
            if ((candidate + slot_count as i32) & 1) != 0 {
                continue;
            }
            let diff = (delta_db - candidate as f32).abs();
            if diff < best_diff || (diff == best_diff && candidate.abs() < best.abs()) {
                best = candidate;
                best_diff = diff;
            }
        }
        best
    }

    pub(crate) fn lifetime_fer_pct(&self) -> f32 {
        if self.total_frames_received == 0 {
            0.0
        } else {
            100.0 * self.total_frames_crc_error as f32 / self.total_frames_received as f32
        }
    }

    /// Inner-loop tick. Takes 16 per-PCG Eb/Nt measurements from the most
    /// recent reverse traffic frame and produces the next frame's 16 PCBs from
    /// one aggregate power error:
    ///
    ///   * Use the strongest PCG in the frame as the control metric.
    ///   * Convert the frame error into a desired net dB change for the next
    ///     frame's worth of PCBs.
    ///   * Quantize that desired change to the net dB deltas achievable by the
    ///     number of valid reverse PCGs in the frame, carrying the residual
    ///     across frames.
    ///   * Distribute the required count of UP and DOWN bits only across valid
    ///     reverse PCGs, leaving non-transmitted PCGs at a neutral hold pattern.
    ///
    /// Returns `true` when this tick also emits a periodic status log.
    pub(crate) fn inner_loop_tick(
        &mut self,
        walsh_code: u8,
        pcg_snr_db: [f32; 16],
        active_pcg_mask: Option<[bool; 16]>,
    ) -> bool {
        self.last_pcg_snr_db = Some(pcg_snr_db);
        self.last_active_pcg_mask = active_pcg_mask;
        self.inner_ticks_since_last_log += 1;

        let active_indices: Vec<usize> = active_pcg_mask
            .map(|mask| {
                mask.iter()
                    .enumerate()
                    .filter_map(|(idx, active)| active.then_some(idx))
                    .collect()
            })
            .unwrap_or_else(|| (0..16).collect());
        let active_count = active_indices.len();
        let effective_target_db = self.effective_target_eb_nt_db();
        let frame_max = if active_count > 0 {
            active_indices
                .iter()
                .map(|&idx| pcg_snr_db[idx])
                .fold(f32::NEG_INFINITY, f32::max)
        } else {
            pcg_snr_db.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
        };
        self.last_frame_metric_db = if active_count > 0 {
            Some(frame_max)
        } else {
            None
        };
        let active_mean = if active_count > 0 {
            active_indices
                .iter()
                .map(|&idx| pcg_snr_db[idx])
                .sum::<f32>()
                / active_count as f32
        } else {
            0.0
        };

        if active_count == 0 {
            self.residual_frame_delta_db *= 0.5;
            self.pending_pcbs = self.alternating_pcbs();
        } else {
            let frame_error_db = frame_max - effective_target_db;
            let desired_frame_delta_db = (-Self::FRAME_RESPONSE_GAIN_DB_PER_DB * frame_error_db)
                .clamp(-Self::FRAME_DELTA_CLAMP_DB, Self::FRAME_DELTA_CLAMP_DB);
            self.residual_frame_delta_db += desired_frame_delta_db;
            let net_frame_delta_db =
                Self::quantize_delta_toward_zero(self.residual_frame_delta_db, active_count)
                    .clamp(-(active_count as i32), active_count as i32);
            self.residual_frame_delta_db -= net_frame_delta_db as f32;

            let up_count = ((active_count as i32 + net_frame_delta_db) / 2) as usize;
            self.pending_pcbs = self.distribute_pcbs(up_count, active_pcg_mask);
        }
        self.distribution_phase = (self.distribution_phase + 1) % 16;

        if self.inner_ticks_since_last_log >= Self::LOG_EVERY_N_TICKS {
            self.inner_ticks_since_last_log = 0;
            let ups = self.pending_pcbs.iter().filter(|b| **b == 0).count();
            let downs = 16 - ups;
            let active_mask = Self::format_active_pcg_mask(self.last_active_pcg_mask);
            let mode = if self.manual_target_override_db.is_some() {
                "manual"
            } else {
                "auto"
            };
            info!(
                "BSC: [power walsh={}] mode={} auto_target={:.2} dB effective_target={:.2} dB frame_max={:.2} dB control_metric={:.2} dB active_pcgs={}/16 active_mask={} active_mean={:.2} dB ups={} downs={} residual={:+.2}dB fer_window={:.2}% fer_lifetime={:.2}% frames={} errors={}",
                walsh_code,
                mode,
                self.target_eb_nt_db,
                effective_target_db,
                frame_max,
                frame_max,
                active_count,
                active_mask,
                active_mean,
                ups,
                downs,
                self.residual_frame_delta_db,
                self.last_fer_pct,
                self.lifetime_fer_pct(),
                self.total_frames_received,
                self.total_frames_crc_error,
            );
            return true;
        }
        false
    }

    /// Outer-loop tick. Run on every accepted reverse traffic frame. The
    /// caller supplies the best available per-frame integrity signal for the
    /// decoded rate: explicit FQI/CRC where the rate carries one, or the RC1
    /// ML terminal-state diagnostic for RC1 quarter/eighth-rate frames.
    /// `crc_valid=false` is treated as an erasure and drives the outer-loop
    /// target Eb/Nt tracker.
    pub(crate) fn outer_loop_tick(&mut self, crc_valid: bool) {
        self.total_frames_received += 1;
        if crc_valid {
            self.total_frames_crc_valid += 1;
        } else {
            self.total_frames_crc_error += 1;
        }
        // Maintain the sliding window for FER monitoring / display.
        if self.crc_window.len() == Self::FER_WINDOW {
            self.crc_window.pop_front();
        }
        self.crc_window.push_back(crc_valid);
        if self.crc_window.len() >= Self::FER_WINDOW {
            let errors = self.crc_window.iter().filter(|v| !**v).count();
            self.last_fer_pct = 100.0 * errors as f32 / self.crc_window.len() as f32;
        }
        if self.manual_target_override_db.is_some() {
            return;
        }
        // Gate: don't adjust target until the channel has proven decodable.
        // During startup the receiver may not have locked, producing frame
        // errors that aren't meaningful FER — avoid ramping target to max.
        if self.total_frames_crc_valid < Self::OUTER_LOOP_MIN_VALID_FRAMES {
            return;
        }
        // Per-frame asymmetric step (IS-2000 outer loop).  Every erasure
        // nudges the target up; every good frame nudges it down.  The
        // step ratio is chosen so that at exactly TARGET_FER_PCT the up
        // and down contributions cancel and the target holds steady.
        if crc_valid {
            self.adaptive_floor_clean_frames = self.adaptive_floor_clean_frames.saturating_add(1);
            self.decay_adaptive_floor_after_clean_frames();
            self.target_eb_nt_db = (self.target_eb_nt_db - Self::target_step_down_db())
                .max(self.effective_auto_target_min_db());
        } else {
            self.adaptive_floor_clean_frames = 0;
            let step_up_db = if self.floor_near_erasure() {
                self.learn_adaptive_floor_from_erasure();
                Self::ADAPTIVE_FLOOR_STEP_UP_DB
            } else {
                Self::TARGET_STEP_UP_DB
            };
            self.target_eb_nt_db = (self.target_eb_nt_db + step_up_db)
                .clamp(self.effective_auto_target_min_db(), self.auto_target_max_db);
        }
    }

    /// Take the pending PCBs and mark them committed. Called by the BTS
    /// hook just before generating the next forward F-FCH frame.
    pub(crate) fn take_pending_pcbs(&mut self) -> [u8; 16] {
        let pcbs = self.pending_pcbs;
        self.last_committed_pcbs = pcbs;
        self.pending_pcbs = [0u8; 16];
        pcbs
    }
}

/// Closed-loop **forward** traffic channel power control state. The mobile
/// reports its observed forward FER in periodic Power Measurement Report
/// Messages (PMRMs); the BSC's outer loop walks the F-FCH composite gain
/// up or down to keep that FER near the target.
///
/// Mirrors the structure of the reverse [`PowerControlState`] outer loop:
/// asymmetric step (fast climb on bad FER, slow decay on clean FER),
/// bounded gain offset, and a sliding window of recent measurements for
/// display. There is no inner loop on the forward link — the spec
/// equivalent (FPC subchannel on RC3+) is out of scope until RC3 reverse
/// traffic is solid.
///
/// See `docs/power-control.md` for the full algorithm and follow-ups.
#[derive(Debug, Clone)]
pub(crate) struct ForwardPowerControlState {
    /// Initial linear amplitude gain captured at slot allocation. The
    /// outer loop adjusts a dB offset relative to this baseline; the
    /// applied slot gain is `initial_gain_linear * 10^(offset_db / 20)`.
    pub(crate) initial_gain_linear: f32,
    /// Current outer-loop gain offset, in dB relative to
    /// `initial_gain_linear`. Bounded to `[GAIN_MIN_DB, GAIN_MAX_DB]`.
    pub(crate) gain_offset_db: f32,
    /// Most recent FER reported in a PMRM, percentage 0..100. `None`
    /// before the first PMRM arrives.
    pub(crate) last_reported_fer_pct: Option<f32>,
    /// Most recent (errors, frames) raw counts from a PMRM. For UI
    /// display so an operator can sanity-check the loop input.
    pub(crate) last_pmrm_errors: u8,
    pub(crate) last_pmrm_frames: u16,
    /// Most recent Active Set pilot strengths reported by the mobile
    /// in a PMRM. Raw 6-bit values per pilot, in the Active Set order
    /// the BS configured (serving pilot first when present). Each raw
    /// value converts to Ec/Io via `-raw/2.0` dB per C.S0005-E
    /// §2.7.2.3.2.6. Empty if no PMRM received yet, or if the mobile
    /// reported `NUM_PILOTS=0` (common when `LAST_HDM_SEQ=3`).
    pub(crate) last_pmrm_pilot_strengths: Vec<u8>,
    /// Lifetime PMRM count for this channel. Useful for diagnostics.
    pub(crate) total_pmrm_count: u64,
    /// Wall-clock time of the previous PMRM, so the handler can log the
    /// inter-arrival delta alongside the FER (helpful for sanity-
    /// checking whether PMRMs are periodic or threshold-triggered).
    pub(crate) last_pmrm_at: Option<Instant>,
    /// Set to `true` once the loop has seen its first PMRM with
    /// `fer_pct <= TARGET_FER_PCT`. Until then, the loop uses the
    /// larger `FAST_START_STEP_UP_DB` to escape a bad-forward-link
    /// bootstrap quickly. After the first clean PMRM the loop falls
    /// back to the steady-state `GAIN_STEP_UP_DB`.
    pub(crate) seen_clean_pmrm: bool,
}

impl ForwardPowerControlState {
    /// Target forward FER for the outer loop. PMRM-reported FER above
    /// this triggers a gain raise.
    pub(crate) const TARGET_FER_PCT: f32 = 1.0;
    /// FER below this triggers a slow gain decay.
    pub(crate) const CLEAN_FER_PCT: f32 = 0.5;
    /// Outer-loop step sizes (asymmetric: fast climb, slow decay), in dB.
    pub(crate) const GAIN_STEP_UP_DB: f32 = 0.5;
    pub(crate) const GAIN_STEP_DOWN_DB: f32 = 0.1;
    /// Initial fast-climb step used until the loop has seen its first
    /// clean PMRM. At allocation time the forward link can be badly
    /// enough degraded that PCBs don't reach the mobile, and the
    /// reverse loop fails to converge until we boost F-FCH gain; use a
    /// bigger initial step so that escape happens in 2-3 PMRMs rather
    /// than 5-6.
    pub(crate) const FAST_START_STEP_UP_DB: f32 = 1.0;
    /// Absolute bounds on the gain offset (dB relative to initial). Total
    /// 12 dB swing room is conservative for a forward closed loop and
    /// keeps any single mobile from monopolizing the composite power
    /// budget.
    pub(crate) const GAIN_MIN_DB: f32 = -6.0;
    pub(crate) const GAIN_MAX_DB: f32 = 6.0;

    pub(crate) fn new(initial_gain_linear: f32) -> Self {
        Self {
            initial_gain_linear,
            gain_offset_db: 0.0,
            last_reported_fer_pct: None,
            last_pmrm_errors: 0,
            last_pmrm_frames: 0,
            total_pmrm_count: 0,
            last_pmrm_at: None,
            seen_clean_pmrm: false,
            last_pmrm_pilot_strengths: Vec::new(),
        }
    }

    /// Convert a raw 6-bit PILOT_STRENGTH value from a PMRM to a pilot
    /// Ec/Io in dB. Per C.S0005-E §2.7.2.3.2.6, the mobile sets the
    /// field to `-2 × 10·log10(PS)` clamped to `[0, 63]`. Inverting:
    /// `Ec/Io = -raw/2.0` dB. The 6-bit field covers -31.5 .. 0 dB in
    /// 0.5 dB steps.
    pub(crate) fn pilot_strength_raw_to_ec_io_db(raw: u8) -> f32 {
        -(raw as f32) / 2.0
    }

    /// Compute the slot gain to apply for the current `gain_offset_db`.
    pub(crate) fn current_gain_linear(&self) -> f32 {
        self.initial_gain_linear * 10f32.powf(self.gain_offset_db / 20.0)
    }

    /// Outer-loop tick. Run on every received PMRM. Returns the new
    /// linear gain to apply to the channel slot, or `None` if the PMRM
    /// reported a frame count of zero (which would otherwise produce a
    /// divide-by-zero in the FER calculation).
    ///
    /// ---------------------------------------------------------------
    /// NOTE: Forward power adjustment is DISABLED. The gain_offset_db
    /// is pinned at 0.0 (initial allocation power). The previous
    /// outer-loop implementation ramped the forward traffic channel
    /// power down by -0.1 dB on every clean PMRM without bound,
    /// eventually dropping F-FCH power by -6 dB and causing the MS to
    /// release with reason=0x02 ("no service").
    ///
    /// DO NOT re-enable forward gain adjustment until proper closed-loop
    /// forward power control is implemented with:
    ///   1. Erasure Indicator Bit (EIB) feedback from the MS
    ///   2. A dead zone around the target FER (no adjustment when FER
    ///      is within acceptable range)
    ///   3. Pilot Ec/Io-based floor so the traffic channel never drops
    ///      below what the pilot can support
    ///   4. Integration testing proving the loop is stable over minutes
    /// ---------------------------------------------------------------
    ///
    /// Field semantics per **C.S0005-E §2.7.2.3.2.6** (Power Measurement
    /// Report Message):
    ///
    ///   * `errors_detected` is a direct 5-bit count of bad forward FCH
    ///     frames in the measurement window, **saturating at 31**. If
    ///     the mobile saw more than 31 bad frames in the window it
    ///     reports `'11111'`, so very high FERs will be under-reported
    ///     by this loop. For our 1% target this only matters when
    ///     `pwr_meas_frames > 3100`, which won't happen in practice.
    ///   * `pwr_meas_frames` is a direct 10-bit count of total forward
    ///     FCH frames the mobile included in the report — no `-1`
    ///     encoding. A value of 0 means no frames were measured (an
    ///     invalid report), and we return `None` rather than dividing.
    pub(crate) fn outer_loop_tick(
        &mut self,
        errors_detected: u8,
        pwr_meas_frames: u16,
        pilot_strengths: &[u8],
        now: Instant,
    ) -> Option<OuterTickResult> {
        let delta_since_prev = self
            .last_pmrm_at
            .map(|prev| now.saturating_duration_since(prev));
        self.last_pmrm_at = Some(now);
        self.total_pmrm_count += 1;
        self.last_pmrm_errors = errors_detected;
        self.last_pmrm_frames = pwr_meas_frames;
        self.last_pmrm_pilot_strengths = pilot_strengths.to_vec();
        if pwr_meas_frames == 0 {
            return None;
        }
        let fer_pct = 100.0 * (errors_detected as f32) / (pwr_meas_frames as f32);
        self.last_reported_fer_pct = Some(fer_pct);

        // Forward gain adjustment disabled — hold at initial power.
        // See doc comment above for rationale.
        if fer_pct <= Self::TARGET_FER_PCT {
            self.seen_clean_pmrm = true;
        }
        Some(OuterTickResult {
            new_gain_linear: self.current_gain_linear(),
            gain_offset_db: self.gain_offset_db,
            fer_pct,
            delta_since_prev,
            fast_start: !self.seen_clean_pmrm,
        })
    }
}

/// Result of a forward-loop outer tick. Carries everything the caller
/// needs to log + push the new gain, including the wall-clock interval
/// since the previous PMRM (useful for distinguishing periodic vs
/// threshold-triggered reports).
#[derive(Debug, Clone, Copy)]
pub(crate) struct OuterTickResult {
    pub(crate) new_gain_linear: f32,
    pub(crate) gain_offset_db: f32,
    pub(crate) fer_pct: f32,
    pub(crate) delta_since_prev: Option<Duration>,
    /// True if this tick was still in fast-start mode (i.e. no clean
    /// PMRM has been seen yet on this channel). Purely informational;
    /// included in the log line so it's obvious when we leave fast
    /// start.
    pub(crate) fast_start: bool,
}

#[cfg(test)]
mod forward_power_control_tests {
    use super::*;

    fn tick(
        state: &mut ForwardPowerControlState,
        errs: u8,
        frames: u16,
    ) -> Option<OuterTickResult> {
        state.outer_loop_tick(errs, frames, &[], Instant::now())
    }

    /// With gain adjustment disabled, clean PMRMs should NOT change the
    /// gain offset — it must stay pinned at 0.0.
    #[test]
    fn clean_pmrm_does_not_change_gain_when_disabled() {
        let mut state = ForwardPowerControlState::new(0.42);
        for _ in 0..200 {
            tick(&mut state, 0, 100);
        }
        assert_eq!(state.gain_offset_db, 0.0); // pinned, not ramped down
        assert_eq!(state.last_reported_fer_pct, Some(0.0));
        assert_eq!(state.total_pmrm_count, 200);
        assert!(state.seen_clean_pmrm);
    }

    /// With gain adjustment disabled, bad PMRMs should NOT change the
    /// gain offset either.
    #[test]
    fn bad_pmrm_does_not_change_gain_when_disabled() {
        let mut state = ForwardPowerControlState::new(0.42);
        for _ in 0..50 {
            tick(&mut state, 10, 100);
        }
        assert_eq!(state.gain_offset_db, 0.0); // pinned, not ramped up
        assert!((state.last_reported_fer_pct.unwrap() - 10.0).abs() < 1e-3);
        // Never saw a clean PMRM, so fast-start flag stays unset.
        assert!(!state.seen_clean_pmrm);
    }

    /// A clean PMRM should still flip the seen_clean_pmrm flag even
    /// though gain is not adjusted.
    #[test]
    fn clean_pmrm_sets_seen_clean_flag() {
        let mut state = ForwardPowerControlState::new(0.42);
        // Bad then clean.
        tick(&mut state, 10, 100);
        assert!(!state.seen_clean_pmrm);
        tick(&mut state, 0, 100);
        assert!(state.seen_clean_pmrm);
        assert_eq!(state.gain_offset_db, 0.0);
    }

    /// FER inside the deadband should leave gain at 0.0.
    #[test]
    fn deadband_pmrm_leaves_gain_alone() {
        let mut state = ForwardPowerControlState::new(0.42);
        // 0.75% FER: above CLEAN (0.5%), at-or-below TARGET (1.0%).
        tick(&mut state, 3, 400);
        assert_eq!(state.gain_offset_db, 0.0);
        assert!((state.last_reported_fer_pct.unwrap() - 0.75).abs() < 1e-3);
        assert!(state.seen_clean_pmrm);
    }

    /// A PMRM with zero measured frames should not divide-by-zero, and
    /// should return None so the caller can skip the gain update.
    #[test]
    fn zero_frame_pmrm_is_a_noop() {
        let mut state = ForwardPowerControlState::new(0.42);
        let result = tick(&mut state, 0, 0);
        assert!(result.is_none());
        assert_eq!(state.gain_offset_db, 0.0);
        assert_eq!(state.last_reported_fer_pct, None);
        assert_eq!(state.total_pmrm_count, 1);
        assert!(!state.seen_clean_pmrm);
    }

    /// `current_gain_linear` should equal initial * 10^(offset/20).
    #[test]
    fn current_gain_linear_applies_db_offset_correctly() {
        let mut state = ForwardPowerControlState::new(1.0);
        state.gain_offset_db = 6.0;
        // 10^(6/20) ≈ 1.9953
        assert!((state.current_gain_linear() - 1.9952623).abs() < 1e-4);
        state.gain_offset_db = -6.0;
        assert!((state.current_gain_linear() - 0.5011872).abs() < 1e-4);
    }

    /// The inter-arrival delta should be None on the first PMRM and
    /// Some on every subsequent PMRM.
    #[test]
    fn inter_arrival_delta_populated_after_first_pmrm() {
        let mut state = ForwardPowerControlState::new(0.42);
        let r1 = tick(&mut state, 2, 100).unwrap();
        assert!(r1.delta_since_prev.is_none());
        let r2 = tick(&mut state, 2, 100).unwrap();
        assert!(r2.delta_since_prev.is_some());
    }

    /// Pilot strength raw -> Ec/Io conversion must match the spec
    /// formula `-raw/2.0` in dB. Verify the endpoints and a middle
    /// sample.
    #[test]
    fn pilot_strength_conversion_matches_spec() {
        assert_eq!(
            ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(0),
            0.0
        );
        assert_eq!(
            ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(16),
            -8.0
        );
        assert_eq!(
            ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(24),
            -12.0
        );
        assert_eq!(
            ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(48),
            -24.0
        );
        assert_eq!(
            ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(63),
            -31.5
        );
    }

    /// A PMRM carrying a non-empty pilot strength list should store
    /// the raw values on the state for display / snapshot use.
    #[test]
    fn pilot_strengths_are_persisted_on_state() {
        let mut state = ForwardPowerControlState::new(0.42);
        state
            .outer_loop_tick(2, 100, &[16, 32, 48], Instant::now())
            .expect("nonzero frames -> Some result");
        assert_eq!(state.last_pmrm_pilot_strengths, vec![16, 32, 48]);
    }
}

#[cfg(test)]
mod reverse_power_control_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};

    use cdma_bts::receiver::pipelined::{
        PipelinedReceiver, ReverseTrafficSettings, reverse_traffic_chain_rc3,
    };
    use num_complex::Complex32;

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

    fn count_ups(bits: &[u8; 16]) -> usize {
        bits.iter().filter(|b| **b == 0).count()
    }

    fn complete_rc3_startup(state: &mut PowerControlState, walsh_code: u8, target_db: f32) {
        for idx in 0..PowerControlState::RC3_STARTUP_SEED_PCGS as u64 {
            state.inner_loop_tick_single_pcg(walsh_code, idx, target_db);
        }
        assert!(
            state
                .startup_seed
                .as_ref()
                .map(|seed| seed.seeded)
                .unwrap_or(false),
            "RC3 startup should be seeded after the warmup window"
        );
    }

    fn engage_reverse_outer_loop(state: &mut PowerControlState) {
        for _ in 0..PowerControlState::OUTER_LOOP_MIN_VALID_FRAMES {
            state.outer_loop_tick(true);
        }
    }

    #[derive(Debug)]
    struct Rc3PowerReplaySummary {
        measurement_count: usize,
        measurement_slot_mean_db: [f32; 16],
        slot_up_count: [usize; 16],
        slot_down_count: [usize; 16],
        first_measurement_db: f32,
        seeded_target_db: f32,
        target_min_db: f32,
        target_max_db: f32,
        final_target_db: f32,
        frame_count: usize,
        frame_valid_count: usize,
        rate_counts: BTreeMap<i64, usize>,
        strong_below_count: usize,
        strong_below_up_count: usize,
        strong_above_count: usize,
        strong_above_down_count: usize,
        near_target_count: usize,
        near_target_up_count: usize,
        near_target_down_count: usize,
        history_count: usize,
        history_mean_min_db: Option<f32>,
        history_mean_max_db: Option<f32>,
        last_active_mask: Option<[bool; 16]>,
    }

    fn read_iq_wav(
        mut reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
    ) -> (u32, Vec<Complex32>) {
        let sample_rate = reader.spec().sample_rate;
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let iq_samples = samples
            .chunks_exact(2)
            .map(|a| Complex32::new(a[0] as f32 / i16::MAX as f32, a[1] as f32 / i16::MAX as f32))
            .collect::<Vec<_>>();
        (sample_rate, iq_samples)
    }

    fn traffic_tag_bool(
        tags: &std::collections::HashMap<&'static str, i64>,
        key: &'static str,
    ) -> Option<bool> {
        tags.get(key).copied().map(|v| v != 0)
    }

    fn rc3_frame_valid_from_block(blk: &cdma_bts::receiver::pipelined::SampleBlock) -> bool {
        let fqi_bits = blk.tags.get("traffic_fqi_bits").copied();
        let tail_valid = traffic_tag_bool(&blk.tags, "traffic_tail_valid").unwrap_or(false);
        if let Some(bits) = fqi_bits {
            if bits > 0 {
                tail_valid && traffic_tag_bool(&blk.tags, "traffic_fqi_valid").unwrap_or(false)
            } else {
                tail_valid && traffic_tag_bool(&blk.tags, "traffic_phy_valid").unwrap_or(true)
            }
        } else {
            traffic_tag_bool(&blk.tags, "traffic_phy_valid").unwrap_or(true)
        }
    }

    fn replay_rc3_capture_through_power_control(
        wav_filename: &str,
        chip_start: u64,
        esn: u32,
        walsh_code: u8,
    ) -> Option<Rc3PowerReplaySummary> {
        let wav_path = test_capture_path(wav_filename);
        if !wav_path.exists() {
            eprintln!(
                "skipping RC3 power replay: {} not found",
                wav_path.display()
            );
            return None;
        }

        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let oversample = sample_rate as usize / 1_228_800;
        let pipeline = reverse_traffic_chain_rc3(ReverseTrafficSettings {
            oversample,
            walsh_code,
            esn,
            reanchor_origin: true,
            snr_threshold: None,
            preamble_num_pcgs: None,
            epl_pilot: true,
            rev_fch_gating_mode: false,
        });

        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32_768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(chip_start * oversample as u64);
        let out_rx = receiver.add_pipeline(pipeline);
        receiver.run_pipeline().unwrap();

        let mut state = PowerControlState::new_rc3();
        let mut measurement_count = 0usize;
        let mut frame_count = 0usize;
        let mut frame_valid_count = 0usize;
        let mut rate_counts = BTreeMap::new();
        let mut slot_sum_db = [0.0f64; 16];
        let mut slot_count = [0usize; 16];
        let mut slot_up_count = [0usize; 16];
        let mut slot_down_count = [0usize; 16];
        let mut first_measurement_db = None;
        let mut seeded_target_db = None;
        let mut target_min_db = state.effective_target_eb_nt_db();
        let mut target_max_db = state.effective_target_eb_nt_db();
        let mut strong_below_count = 0usize;
        let mut strong_below_up_count = 0usize;
        let mut strong_above_count = 0usize;
        let mut strong_above_down_count = 0usize;
        let mut near_target_count = 0usize;
        let mut near_target_up_count = 0usize;
        let mut near_target_down_count = 0usize;

        for blocks in out_rx {
            for blk in blocks {
                if blk.tags.get("traffic_pcg_measurement") == Some(&1) {
                    let Some(metric_db) = blk
                        .pcg_signal_snr_db
                        .as_ref()
                        .and_then(|values| values.first())
                        .copied()
                    else {
                        continue;
                    };
                    let Some(abs_chip) = blk
                        .tags
                        .get("absolute_chip_start")
                        .copied()
                        .and_then(|chip| u64::try_from(chip).ok())
                    else {
                        continue;
                    };
                    let abs_pcg = abs_chip / 1_536;
                    let slot = (abs_pcg % 16) as usize;
                    let target_before = state.effective_target_eb_nt_db();
                    let pcb = state.inner_loop_tick_single_pcg(walsh_code, abs_pcg, metric_db);

                    if first_measurement_db.is_none() {
                        first_measurement_db = Some(metric_db);
                        seeded_target_db = Some(state.effective_target_eb_nt_db());
                    }

                    measurement_count += 1;
                    slot_sum_db[slot] += metric_db as f64;
                    slot_count[slot] += 1;
                    if pcb == 0 {
                        slot_up_count[slot] += 1;
                    } else {
                        slot_down_count[slot] += 1;
                    }

                    let raw_error_db = target_before - metric_db;
                    if raw_error_db >= 1.0 {
                        strong_below_count += 1;
                        if pcb == 0 {
                            strong_below_up_count += 1;
                        }
                    } else if raw_error_db <= -1.0 {
                        strong_above_count += 1;
                        if pcb == 1 {
                            strong_above_down_count += 1;
                        }
                    } else if raw_error_db.abs() <= PowerControlState::PCG_HOLD_BAND_DB {
                        near_target_count += 1;
                        if pcb == 0 {
                            near_target_up_count += 1;
                        } else {
                            near_target_down_count += 1;
                        }
                    }

                    target_min_db = target_min_db.min(state.effective_target_eb_nt_db());
                    target_max_db = target_max_db.max(state.effective_target_eb_nt_db());
                } else if blk.tags.get("traffic_phy_frame") == Some(&1)
                    || blk.tags.get("traffic_phy_status") == Some(&1)
                {
                    frame_count += 1;
                    let frame_valid = rc3_frame_valid_from_block(&blk);
                    if frame_valid {
                        frame_valid_count += 1;
                    }
                    if let Some(rate) = blk.tags.get("traffic_rate_bps").copied() {
                        *rate_counts.entry(rate).or_default() += 1;
                    }
                    state.outer_loop_tick(frame_valid);
                    if let Some(pcg_snr_db) = blk.pcg_signal_snr_db.as_ref() {
                        if pcg_snr_db.len() == 16 {
                            let mut arr = [0.0f32; 16];
                            arr.copy_from_slice(&pcg_snr_db[..16]);
                            state.record_frame_measurements(arr, blk.active_pcg_mask);
                        }
                    }
                    target_min_db = target_min_db.min(state.effective_target_eb_nt_db());
                    target_max_db = target_max_db.max(state.effective_target_eb_nt_db());
                }
            }
        }

        let measurement_slot_mean_db = core::array::from_fn(|idx| {
            if slot_count[idx] > 0 {
                (slot_sum_db[idx] / slot_count[idx] as f64) as f32
            } else {
                f32::NAN
            }
        });
        let history_mean_min_db = state
            .power_history
            .iter()
            .map(|entry| entry.measured_mean_db)
            .reduce(f32::min);
        let history_mean_max_db = state
            .power_history
            .iter()
            .map(|entry| entry.measured_mean_db)
            .reduce(f32::max);

        Some(Rc3PowerReplaySummary {
            measurement_count,
            measurement_slot_mean_db,
            slot_up_count,
            slot_down_count,
            first_measurement_db: first_measurement_db
                .expect("expected at least one RC3 PCG measurement from real capture"),
            seeded_target_db: seeded_target_db
                .expect("expected initial target seeding from first RC3 PCG measurement"),
            target_min_db,
            target_max_db,
            final_target_db: state.effective_target_eb_nt_db(),
            frame_count,
            frame_valid_count,
            rate_counts,
            strong_below_count,
            strong_below_up_count,
            strong_above_count,
            strong_above_down_count,
            near_target_count,
            near_target_up_count,
            near_target_down_count,
            history_count: state.power_history.len(),
            history_mean_min_db,
            history_mean_max_db,
            last_active_mask: state.last_active_pcg_mask,
        })
    }

    #[test]
    fn delta_quantizer_breaks_ties_toward_zero() {
        assert_eq!(PowerControlState::quantize_delta_toward_zero(-1.0, 16), 0);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(1.0, 16), 0);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(-2.0, 16), -2);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(2.0, 16), 2);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(-3.1, 16), -4);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(3.1, 16), 4);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(-1.0, 5), -1);
        assert_eq!(PowerControlState::quantize_delta_toward_zero(1.0, 5), 1);
    }

    #[test]
    #[ignore = "legacy frame-batched fallback path retained for reference; production loop is per-PCG scheduled"]
    fn sparse_active_pcgs_only_consume_budget_on_masked_slots() {
        let mut state = PowerControlState::new();
        let pcgs = [
            13.0, 12.0, 30.0, -20.0, -20.0, -20.0, -20.0, -20.0, -20.0, -20.0, -20.0, -20.0, -20.0,
            -20.0, -20.0, -20.0,
        ];
        let mut mask = [false; 16];
        mask[0] = true;
        mask[1] = true;

        state.inner_loop_tick(10, pcgs, Some(mask));
        let first = state.take_pending_pcbs();
        assert_eq!(
            count_ups(&first),
            7,
            "first frame should drive net DOWN on 2 valid slots"
        );
        assert_eq!(first[0], 1);
        assert_eq!(first[1], 1);
        assert_eq!(state.last_frame_metric_db, Some(13.0));

        state.inner_loop_tick(10, pcgs, Some(mask));
        let second = state.take_pending_pcbs();
        assert_eq!(
            count_ups(&second),
            7,
            "subsequent frames remain net DOWN on masked slots"
        );
        assert_eq!(second[0], 1);
        assert_eq!(second[1], 1);
    }

    #[test]
    fn full_frame_low_power_drives_up_even_without_sparse_gating() {
        let mut state = PowerControlState::new();
        let below_target = state.target_eb_nt_db - 6.0;
        let pcgs = [below_target; 16];

        state.inner_loop_tick(11, pcgs, None);
        let first = state.take_pending_pcbs();
        assert!(count_ups(&first) > 8, "first frame should demand net UP");

        state.inner_loop_tick(11, pcgs, None);
        let second = state.take_pending_pcbs();
        assert!(count_ups(&second) > 8, "second frame should demand net UP");
    }

    #[test]
    fn distributed_pcbs_preserve_requested_net_delta() {
        let mut state = PowerControlState::new();
        state.distribution_phase = 0;

        let down_bias = state.distribute_pcbs(7, None);
        assert_eq!(count_ups(&down_bias), 7);

        let hold = state.distribute_pcbs(8, None);
        assert_eq!(count_ups(&hold), 8);

        let up_bias = state.distribute_pcbs(9, None);
        assert_eq!(count_ups(&up_bias), 9);
    }

    #[test]
    fn single_pcg_below_target_drives_up_on_that_absolute_slot() {
        let mut state = PowerControlState::new_rc1();
        let below_target = state.effective_target_eb_nt_db() - 2.0;
        let pcb = state.inner_loop_tick_single_pcg(10, 34, below_target);

        assert_eq!(pcb, 0);
        assert_eq!(state.last_committed_pcbs[34 % 16], 0);
        assert_eq!(
            state.last_pcg_pilot_ec_nt_db.unwrap()[34 % 16],
            below_target
        );
        assert_eq!(state.last_frame_metric_db, Some(below_target));
    }

    #[test]
    fn rc3_startup_holds_and_does_not_seed_on_first_measurement() {
        let mut state = PowerControlState::new_rc3();
        let initial_target = state.effective_target_eb_nt_db();
        let first_measurement = state.auto_target_min_db + 1.0;

        let pcb = state.inner_loop_tick_single_pcg(11, 0, first_measurement);

        assert_eq!(pcb, 0, "startup should emit alternating hold PCBs");
        assert_eq!(
            state.effective_target_eb_nt_db(),
            initial_target,
            "first RC3 PCG must not seed the target immediately"
        );
        assert_eq!(state.filtered_pcg_metric_db, None);
        assert_eq!(
            state
                .startup_seed
                .as_ref()
                .expect("RC3 should have startup state")
                .samples
                .len(),
            1
        );
    }

    #[test]
    fn rc3_startup_seeds_from_trimmed_mean_after_warmup() {
        let mut state = PowerControlState::new_rc3();
        let seed_measurement = -7.0;
        let outlier = 30.0;

        for idx in 0..PowerControlState::RC3_STARTUP_SEED_PCGS as u64 {
            let measurement = if idx == 0 { outlier } else { seed_measurement };
            let pcb = state.inner_loop_tick_single_pcg(11, idx, measurement);
            assert_eq!(
                pcb,
                PowerControlState::fallback_pcb_for_abs_pcg(idx),
                "startup PCBs should alternate while collecting"
            );
        }

        assert!(
            state
                .startup_seed
                .as_ref()
                .expect("RC3 should have startup state")
                .seeded
        );
        assert_eq!(state.effective_target_eb_nt_db(), seed_measurement);
        assert_eq!(state.filtered_pcg_metric_db, Some(seed_measurement));
        assert_eq!(state.pcg_command_residual_db, 0.0);
    }

    #[test]
    fn single_pcg_above_target_drives_down_on_that_absolute_slot() {
        let mut state = PowerControlState::new_rc3();
        complete_rc3_startup(&mut state, 11, -5.0);
        let above_target = state.auto_target_max_db + 4.5;
        let pcb = state.inner_loop_tick_single_pcg(11, 35, above_target);

        assert_eq!(pcb, 1);
        assert_eq!(state.last_committed_pcbs[35 % 16], 1);
        assert_eq!(
            state.last_pcg_pilot_ec_nt_db.unwrap()[35 % 16],
            above_target
        );
        assert!(
            state.last_frame_metric_db.unwrap() > state.effective_target_eb_nt_db(),
            "filtered control metric should remain above target"
        );
    }

    #[test]
    fn single_pcg_non_finite_measurement_falls_back_to_absolute_pcg_parity() {
        let mut state = PowerControlState::new_rc3();

        let even_pcb = state.inner_loop_tick_single_pcg(12, 40, f32::NAN);
        let odd_pcb = state.inner_loop_tick_single_pcg(12, 41, f32::NAN);

        assert_eq!(even_pcb, 0);
        assert_eq!(odd_pcb, 1);
        assert_eq!(state.last_committed_pcbs[40 % 16], 0);
        assert_eq!(state.last_committed_pcbs[41 % 16], 1);
    }

    #[test]
    fn single_pcg_exact_target_alternates_for_zero_net_change() {
        let mut state = PowerControlState::new_rc3();
        complete_rc3_startup(&mut state, 12, -5.0);
        // Feed exactly the seeded target Eb/Nt so the sigma-delta should
        // alternate UP/DOWN for zero net change.
        let target = state.effective_target_eb_nt_db();
        for idx in 32..40u64 {
            state.inner_loop_tick_single_pcg(12, idx, target);
        }
        let mut pcbs = [0u8; 8];
        for (idx, pcb) in pcbs.iter_mut().enumerate() {
            *pcb = state.inner_loop_tick_single_pcg(12, (40 + idx) as u64, target);
        }
        // After convergence, should alternate evenly (equal UPs and DOWNs)
        let ups = pcbs.iter().filter(|&&p| p == 0).count();
        let downs = pcbs.iter().filter(|&&p| p == 1).count();
        assert_eq!(ups, 4, "Expected 4 UPs in 8 PCGs at target, got {}", ups);
        assert_eq!(
            downs, 4,
            "Expected 4 DOWNs in 8 PCGs at target, got {}",
            downs
        );
    }

    #[test]
    fn single_pcg_small_error_uses_mixed_duty_cycle_not_all_same_direction() {
        let mut state = PowerControlState::new_rc3();
        complete_rc3_startup(&mut state, 12, -5.0);
        let target = state.effective_target_eb_nt_db();

        let above_target = target + 1.5;
        let mut up = 0;
        let mut down = 0;

        for idx in 32..48u64 {
            let pcb = state.inner_loop_tick_single_pcg(12, idx, above_target);
            if pcb == 0 {
                up += 1;
            } else {
                down += 1;
            }
        }

        assert!(down > up, "above-target error should bias toward DOWN");
        assert!(up > 0, "small error should still mix in some UP commands");
    }

    #[test]
    fn manual_target_override_resets_live_pcg_residual() {
        let mut state = PowerControlState::new_rc3();
        complete_rc3_startup(&mut state, 12, -5.0);
        let above_target = state.effective_target_eb_nt_db() + 5.0;

        let first = state.inner_loop_tick_single_pcg(12, 32, above_target);
        state.inner_loop_tick_single_pcg(12, 33, above_target);
        assert_eq!(first, 1, "above-target metric should initially drive DOWN");
        assert_ne!(
            state.pcg_command_residual_db, 0.0,
            "precondition: live PCG residual should be non-zero before retarget"
        );

        let pinned_target =
            (state.effective_target_eb_nt_db() - 7.0).max(state.manual_target_min_db);
        state.set_manual_target_override_db(pinned_target);
        assert_eq!(
            state.pcg_command_residual_db, 0.0,
            "manual retarget should clear prior residual bias"
        );

        let after_reset = state.inner_loop_tick_single_pcg(12, 2, pinned_target);
        assert_eq!(
            after_reset, 0,
            "manual retarget should clear prior residual bias"
        );
    }

    #[test]
    fn manual_target_override_freezes_outer_loop_target() {
        let mut state = PowerControlState::new_rc3();
        let original_auto_target = state.target_eb_nt_db;
        let pinned_target = state
            .auto_target_min_db
            .clamp(state.manual_target_min_db, state.manual_target_max_db);
        let pinned = state.set_manual_target_override_db(pinned_target);

        assert_eq!(pinned, pinned_target);
        assert_eq!(state.effective_target_eb_nt_db(), pinned_target);

        for _ in 0..PowerControlState::FER_WINDOW {
            state.outer_loop_tick(false);
        }

        assert_eq!(state.target_eb_nt_db, original_auto_target);
        assert_eq!(state.manual_target_override_db, Some(pinned_target));
        assert_eq!(state.effective_target_eb_nt_db(), pinned_target);
        assert_eq!(state.last_fer_pct, 100.0);
    }

    #[test]
    fn adaptive_floor_learns_when_erasure_occurs_at_auto_floor() {
        let mut state = PowerControlState::new_rc3();
        engage_reverse_outer_loop(&mut state);
        state.target_eb_nt_db = state.auto_target_min_db;

        state.outer_loop_tick(false);

        let expected_floor =
            state.auto_target_min_db + PowerControlState::ADAPTIVE_FLOOR_STEP_UP_DB;
        assert_eq!(state.adaptive_auto_floor_db, Some(expected_floor));
        assert_eq!(state.effective_auto_target_min_db(), expected_floor);
        assert_eq!(state.target_eb_nt_db, expected_floor);
    }

    #[test]
    fn adaptive_floor_does_not_learn_from_errors_above_floor_band() {
        let mut state = PowerControlState::new_rc3();
        engage_reverse_outer_loop(&mut state);
        let target_before =
            state.auto_target_min_db + PowerControlState::ADAPTIVE_FLOOR_TRIGGER_BAND_DB + 1.0;
        state.target_eb_nt_db = target_before;

        state.outer_loop_tick(false);

        assert_eq!(state.adaptive_auto_floor_db, None);
        assert_eq!(
            state.target_eb_nt_db,
            target_before + PowerControlState::TARGET_STEP_UP_DB
        );
    }

    #[test]
    fn adaptive_floor_holds_clean_outer_loop_above_learned_floor() {
        let mut state = PowerControlState::new_rc3();
        engage_reverse_outer_loop(&mut state);
        state.target_eb_nt_db = state.auto_target_min_db;
        state.outer_loop_tick(false);

        let learned_floor = state.effective_auto_target_min_db();
        for _ in 0..(PowerControlState::ADAPTIVE_FLOOR_DECAY_FRAMES - 1) {
            state.outer_loop_tick(true);
        }

        assert_eq!(state.effective_auto_target_min_db(), learned_floor);
        assert_eq!(state.target_eb_nt_db, learned_floor);
    }

    #[test]
    fn adaptive_floor_decays_after_sustained_clean_frames() {
        let mut state = PowerControlState::new_rc3();
        engage_reverse_outer_loop(&mut state);
        state.target_eb_nt_db = state.auto_target_min_db;
        state.outer_loop_tick(false);

        let learned_floor = state.effective_auto_target_min_db();
        for _ in 0..PowerControlState::ADAPTIVE_FLOOR_DECAY_FRAMES {
            state.outer_loop_tick(true);
        }

        let expected_floor = learned_floor - PowerControlState::ADAPTIVE_FLOOR_DECAY_STEP_DB;
        assert!(
            (state.effective_auto_target_min_db() - expected_floor).abs() < f32::EPSILON,
            "learned floor should decay after a clean interval"
        );
        assert!(state.target_eb_nt_db >= state.effective_auto_target_min_db());
        assert!(state.target_eb_nt_db < learned_floor);
    }

    #[test]
    fn manual_target_override_uses_wide_diagnostic_range() {
        let mut state = PowerControlState::new_rc3();

        // Value within manual range but below auto range.
        let within_manual = state.manual_target_min_db + 1.0;
        assert_eq!(
            state.set_manual_target_override_db(within_manual),
            within_manual
        );
        assert_eq!(state.effective_target_eb_nt_db(), within_manual);

        // Values below manual_target_min clamp to the floor.
        let below_floor = state.manual_target_min_db - 5.0;
        assert_eq!(
            state.set_manual_target_override_db(below_floor),
            state.manual_target_min_db
        );
        assert_eq!(
            state.effective_target_eb_nt_db(),
            state.manual_target_min_db
        );

        // Values above auto range but within manual range.
        let mid_manual = (state.manual_target_min_db + state.manual_target_max_db) / 2.0;
        assert_eq!(state.set_manual_target_override_db(mid_manual), mid_manual);
        assert_eq!(state.effective_target_eb_nt_db(), mid_manual);

        // Values above manual_target_max clamp to the ceiling.
        let above_ceil = state.manual_target_max_db + 5.0;
        assert_eq!(
            state.set_manual_target_override_db(above_ceil),
            state.manual_target_max_db
        );
        assert_eq!(
            state.effective_target_eb_nt_db(),
            state.manual_target_max_db
        );
    }

    #[test]
    fn clearing_manual_target_override_snaps_back_into_auto_band() {
        let mut state = PowerControlState::new_rc3();
        let requested_target = if state.manual_target_min_db < state.auto_target_min_db {
            state.auto_target_min_db - 0.5
        } else if state.manual_target_max_db > state.auto_target_max_db {
            state.auto_target_max_db + 0.5
        } else {
            state.target_eb_nt_db
        };
        let pinned_target = state.set_manual_target_override_db(requested_target);
        let expected_auto_target =
            pinned_target.clamp(state.auto_target_min_db, state.auto_target_max_db);

        let cleared = state
            .clear_manual_target_override_db()
            .expect("override should be present");
        assert_eq!(cleared, pinned_target);
        assert_eq!(state.manual_target_override_db, None);
        assert_eq!(state.target_eb_nt_db, expected_auto_target);
        assert_eq!(state.effective_target_eb_nt_db(), expected_auto_target);

        let before_clean_window = state.target_eb_nt_db;
        for _ in 0..PowerControlState::FER_WINDOW {
            state.outer_loop_tick(true);
        }

        let expected_after_clean_window = (before_clean_window
            - PowerControlState::target_step_down_db())
        .max(state.auto_target_min_db);
        assert!(
            (state.target_eb_nt_db - expected_after_clean_window).abs() < f32::EPSILON,
            "clean FER window should step down by configured amount within auto band (got {:.2}, expected {:.2})",
            state.target_eb_nt_db,
            expected_after_clean_window,
        );
        assert!(
            state.target_eb_nt_db >= state.auto_target_min_db
                && state.target_eb_nt_db <= state.auto_target_max_db,
            "auto mode should resume inside the automatic band (got {:.2})",
            state.target_eb_nt_db,
        );
        assert_eq!(state.effective_target_eb_nt_db(), state.target_eb_nt_db);
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "release-only real-WAV RC3 power replay; run with --release -- --nocapture"
    )]
    fn capture_rc3_real_power_controller_replay_reports_summary() {
        for (
            label,
            wav_filename,
            chip_start,
            esn,
            walsh_code,
            min_measurements,
            min_valid_frames,
        ) in [
            (
                "w11",
                "1793960586090657.wav",
                1793960586090657,
                0x80857E58,
                11u8,
                15_000usize,
                900usize,
            ),
            (
                "w12",
                "1793967987133603.wav",
                1793967987133603,
                0x80857E58,
                12u8,
                12_000usize,
                600usize,
            ),
        ] {
            let Some(summary) =
                replay_rc3_capture_through_power_control(wav_filename, chip_start, esn, walsh_code)
            else {
                continue;
            };

            let slot_line = summary
                .measurement_slot_mean_db
                .iter()
                .enumerate()
                .map(|(idx, db)| format!("pcg{idx:02}={db:.2}"))
                .collect::<Vec<_>>()
                .join(" ");
            let pcb_line = (0..16usize)
                .map(|idx| {
                    format!(
                        "pcg{idx:02}=u{}/d{}",
                        summary.slot_up_count[idx], summary.slot_down_count[idx]
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            let finite_slot_means = summary
                .measurement_slot_mean_db
                .iter()
                .copied()
                .filter(|db| db.is_finite())
                .collect::<Vec<_>>();
            let slot_spread_db = finite_slot_means
                .iter()
                .copied()
                .reduce(f32::max)
                .unwrap_or(0.0)
                - finite_slot_means
                    .iter()
                    .copied()
                    .reduce(f32::min)
                    .unwrap_or(0.0);
            let below_up_pct = if summary.strong_below_count > 0 {
                100.0 * summary.strong_below_up_count as f32 / summary.strong_below_count as f32
            } else {
                0.0
            };
            let above_down_pct = if summary.strong_above_count > 0 {
                100.0 * summary.strong_above_down_count as f32 / summary.strong_above_count as f32
            } else {
                0.0
            };

            eprintln!(
                "RC3 power replay [{label}] summary: measurements={} frames={}/{} valid first_meas={:.2}dB seeded_target={:.2}dB target_range=[{:.2},{:.2}] final_target={:.2}dB rates={:?}",
                summary.measurement_count,
                summary.frame_valid_count,
                summary.frame_count,
                summary.first_measurement_db,
                summary.seeded_target_db,
                summary.target_min_db,
                summary.target_max_db,
                summary.final_target_db,
                summary.rate_counts,
            );
            eprintln!("RC3 power replay [{label}] slot means: {slot_line}");
            eprintln!("RC3 power replay [{label}] pcb mix: {pcb_line}");
            eprintln!(
                "RC3 power replay [{label}] assessment: slot_spread={:.2}dB below_up={:.1}% above_down={:.1}% near={} near_up={} near_down={} history={} history_range={:?}..{:?} active_mask={}",
                slot_spread_db,
                below_up_pct,
                above_down_pct,
                summary.near_target_count,
                summary.near_target_up_count,
                summary.near_target_down_count,
                summary.history_count,
                summary.history_mean_min_db,
                summary.history_mean_max_db,
                PowerControlState::format_active_pcg_mask(summary.last_active_mask),
            );

            assert!(summary.measurement_count > min_measurements);
            assert!(summary.frame_valid_count >= min_valid_frames);
            assert!(summary.history_count > 0);
            assert_eq!(summary.last_active_mask, Some([true; 16]));
        }
    }
}
