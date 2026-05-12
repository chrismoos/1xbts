//! Reverse-link closed-loop power control.
//!
//! RC3 inner loop controls on per-PCG pilot symbol SINR. Setpoints are
//! lock-step with `rlgain_adj` in `paging_messages.rs` — re-run
//! `rc3_pilot_sinr_at_1pct_fer_calibration` before changing either.

use std::{collections::HashMap, collections::VecDeque, sync::Arc};

use parking_lot::Mutex;

use super::handle::{TrafficChannelPool, TrafficChannelWrapper};

const RC1_INITIAL_TARGET_DB: f32 = 10.0;
const RC1_AUTO_MIN_DB: f32 = 8.0;
const RC1_AUTO_MAX_DB: f32 = 12.0;
const RC1_MANUAL_MIN_DB: f32 = 0.0;
const RC1_MANUAL_MAX_DB: f32 = 40.0;
const RC3_INITIAL_TARGET_DB: f32 = -10.0;
const RC3_AUTO_MIN_DB: f32 = -15.0;
const RC3_AUTO_MAX_DB: f32 = -8.0;
const RC3_MANUAL_MIN_DB: f32 = -40.0;
const RC3_MANUAL_MAX_DB: f32 = 40.0;
const PCG_METRIC_FILTER_ALPHA: f32 = 0.05;
const PCG_HOLD_BAND_DB: f32 = 0.15;
const PCG_RESPONSE_GAIN_DB_PER_DB: f32 = 1.0;
const PCG_DESIRED_STEP_CLAMP_DB: f32 = 3.0;
const PCG_RESIDUAL_CLAMP_DB: f32 = 1.0;
const RAW_POWER_FILTER_ALPHA: f32 = 0.05;
const RC3_STARTUP_SEED_PCGS: usize = 32;
const FER_WINDOW: usize = 50;
const OUTER_LOOP_MIN_VALID_FRAMES: u64 = 10;
const TARGET_FER_PCT: f32 = 0.5;
const TARGET_STEP_UP_DB: f32 = 0.25;
const ADAPTIVE_FLOOR_TRIGGER_BAND_DB: f32 = 0.25;
const ADAPTIVE_FLOOR_STEP_UP_DB: f32 = 0.1;
const ADAPTIVE_FLOOR_DECAY_FRAMES: u64 = 250;
const ADAPTIVE_FLOOR_DECAY_STEP_DB: f32 = 0.1;
const METRIC_HISTORY_LEN: usize = 16;
/// PCGs the inner loop predicts ahead before scheduling the PCB.
/// Must exceed metric arrival age or the TX scheduler runs late.
pub(super) const PCG_PREDICTION_LEAD_PCGS: u32 = 12;
const PCG_PREDICTION_CLAMP_DB: f32 = 2.0;
// Subtracted from the PCB error as filtered Rx power ramps from
// BRAKE_BEGIN_DBFS to BRAKE_FULL_DBFS, to stop UP-driven clipping.
const BRAKE_BEGIN_DBFS: f32 = -8.0;
const BRAKE_FULL_DBFS: f32 = 0.0;
const BRAKE_MAX_OFFSET_DB: f32 = 15.0;

#[derive(Debug, Clone, Copy)]
pub struct BtsPowerControlTick {
    pub pcb: u8,
    pub target_db: f32,
    pub control_metric_db: f32,
    pub raw_power_db: Option<f32>,
    pub filtered_raw_power_db: Option<f32>,
    pub raw_power_clamp_active: bool,
}

#[derive(Debug, Clone)]
pub struct BtsPowerControlSnapshot {
    pub walsh_code: u8,
    pub target_eb_nt_db: f32,
    pub effective_target_eb_nt_db: f32,
    pub manual_target_override_db: Option<f32>,
    pub last_pcg_pilot_ec_nt_db: [f32; 16],
    pub last_pcbs: [u8; 16],
    pub fer_pct: f32,
    pub frames_total: u64,
    pub frames_crc_error: u64,
    pub last_brake_offset_db: f32,
}

#[derive(Debug, Clone, Copy)]
struct BtsReversePowerSetpoint {
    target_db: f32,
    held: bool,
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
        if self.seeded || !measurement_db.is_finite() {
            return;
        }
        self.samples.push(measurement_db);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rc3_state_without_startup_seed() -> BtsReversePowerControlState {
        BtsReversePowerControlState::with_params(
            RC3_INITIAL_TARGET_DB,
            RC3_AUTO_MIN_DB,
            RC3_AUTO_MAX_DB,
            RC3_MANUAL_MIN_DB,
            RC3_MANUAL_MAX_DB,
            None,
        )
    }

    #[test]
    fn raw_power_filter_updates_after_first_pcg() {
        let mut state = rc3_state_without_startup_seed();

        let first = state.tick_single_pcg(10, 100, -20.0, Some(-10.0), 0);
        assert_eq!(first.filtered_raw_power_db, Some(-10.0));
        assert!(!first.raw_power_clamp_active);

        let second = state.tick_single_pcg(10, 101, -20.0, Some(-30.0), 0);
        assert!(second.filtered_raw_power_db.is_some_and(|db| db < -10.0));
        assert!(!second.raw_power_clamp_active);
    }

    fn rc3_state_for_predictor() -> BtsReversePowerControlState {
        BtsReversePowerControlState::with_params(
            RC3_INITIAL_TARGET_DB,
            RC3_AUTO_MIN_DB,
            RC3_AUTO_MAX_DB,
            RC3_MANUAL_MIN_DB,
            RC3_MANUAL_MAX_DB,
            None,
        )
    }

    fn lsq_from_samples(samples: &[f32]) -> (f32, f32) {
        let mut q = VecDeque::with_capacity(samples.len());
        for &s in samples {
            q.push_back(s);
        }
        BtsReversePowerControlState::lsq_intercept_and_slope_at_newest(&q)
    }

    #[test]
    fn lsq_returns_zero_slope_and_mean_when_flat() {
        let samples = vec![-12.5_f32; METRIC_HISTORY_LEN];
        let (intercept, slope) = lsq_from_samples(&samples);
        assert!((intercept - -12.5).abs() < 1e-4);
        assert!(slope.abs() < 1e-6);
    }

    #[test]
    fn lsq_recovers_known_slope_and_intercept_at_now() {
        let samples: Vec<f32> = (0..METRIC_HISTORY_LEN as i32)
            .map(|t| 0.3 * t as f32 + 2.0)
            .collect();
        let (intercept, slope) = lsq_from_samples(&samples);
        let expected_intercept_at_newest = 2.0 + 0.3 * (METRIC_HISTORY_LEN as f32 - 1.0);
        assert!(
            (intercept - expected_intercept_at_newest).abs() < 1e-3,
            "intercept_at_now: got {intercept}, expected {expected_intercept_at_newest}",
        );
        assert!(
            (slope - 0.3).abs() < 1e-4,
            "slope: got {slope}, expected 0.3"
        );
    }

    #[test]
    fn prediction_matches_level_in_steady_state() {
        let mut state = rc3_state_for_predictor();
        let level = -12.0_f32;
        for pcg in 0..(METRIC_HISTORY_LEN as u64 * 2) {
            let _ = state.tick_single_pcg(10, pcg, level, None, PCG_PREDICTION_LEAD_PCGS);
        }
        assert!(state.last_slope_db_per_pcg.abs() < 1e-4);
        let pred = state.last_predicted_metric_db.unwrap();
        assert!(
            (pred - level).abs() < 1e-3,
            "steady-state prediction should match level: pred={pred} level={level}",
        );
    }

    #[test]
    fn prediction_anticipates_downward_trend() {
        let mut state = rc3_state_for_predictor();
        for pcg in 0..(METRIC_HISTORY_LEN as u64 * 2) {
            let m = -10.0 - 0.1 * pcg as f32;
            let _ = state.tick_single_pcg(10, pcg, m, None, PCG_PREDICTION_LEAD_PCGS);
        }
        assert!(
            (state.last_slope_db_per_pcg - -0.1).abs() < 1e-3,
            "slope should be ~-0.1: got {}",
            state.last_slope_db_per_pcg,
        );
        // Prediction should lead the LSQ intercept downward.
        let (intercept, _) =
            BtsReversePowerControlState::lsq_intercept_and_slope_at_newest(&state.metric_history);
        let pred = state.last_predicted_metric_db.unwrap();
        assert!(
            pred < intercept,
            "downward trend → predicted < intercept: pred={pred} intercept={intercept}",
        );
    }

    #[test]
    fn prediction_anticipates_upward_trend() {
        let mut state = rc3_state_for_predictor();
        for pcg in 0..(METRIC_HISTORY_LEN as u64 * 2) {
            let m = -15.0 + 0.1 * pcg as f32;
            let _ = state.tick_single_pcg(10, pcg, m, None, PCG_PREDICTION_LEAD_PCGS);
        }
        assert!((state.last_slope_db_per_pcg - 0.1).abs() < 1e-3);
        let (intercept, _) =
            BtsReversePowerControlState::lsq_intercept_and_slope_at_newest(&state.metric_history);
        let pred = state.last_predicted_metric_db.unwrap();
        assert!(
            pred > intercept,
            "upward trend → predicted > intercept: pred={pred} intercept={intercept}",
        );
    }

    #[test]
    fn prediction_clamp_bounds_extrapolation() {
        let mut state = rc3_state_for_predictor();
        for pcg in 0..(METRIC_HISTORY_LEN as u64) {
            let m = -10.0 - pcg as f32;
            let _ = state.tick_single_pcg(10, pcg, m, None, PCG_PREDICTION_LEAD_PCGS);
        }
        let (intercept, _) =
            BtsReversePowerControlState::lsq_intercept_and_slope_at_newest(&state.metric_history);
        let pred = state.last_predicted_metric_db.unwrap();
        let delta = (pred - intercept).abs();
        assert!(
            delta <= PCG_PREDICTION_CLAMP_DB + 1e-6,
            "clamp should bound prediction excursion: delta={delta} clamp={PCG_PREDICTION_CLAMP_DB}",
        );
    }

    #[test]
    fn held_metric_uses_last_prediction() {
        let mut state = rc3_state_for_predictor();
        for pcg in 0..(METRIC_HISTORY_LEN as u64) {
            let _ = state.tick_single_pcg(10, pcg, -12.0, None, PCG_PREDICTION_LEAD_PCGS);
        }
        let last_pred = state.last_predicted_metric_db.unwrap();
        let tick = state.tick_single_pcg(
            10,
            METRIC_HISTORY_LEN as u64,
            f32::NAN,
            None,
            PCG_PREDICTION_LEAD_PCGS,
        );
        assert!(
            (tick.control_metric_db - last_pred).abs() < 1e-6,
            "held branch should re-use last_predicted: control={} last={}",
            tick.control_metric_db,
            last_pred,
        );
    }

    #[test]
    fn brake_offset_zero_below_begin() {
        assert_eq!(
            BtsReversePowerControlState::brake_offset_db(BRAKE_BEGIN_DBFS - 5.0),
            0.0
        );
        assert_eq!(
            BtsReversePowerControlState::brake_offset_db(BRAKE_BEGIN_DBFS),
            0.0
        );
    }

    #[test]
    fn brake_offset_ramps_linearly_through_zone() {
        let midpoint = (BRAKE_BEGIN_DBFS + BRAKE_FULL_DBFS) * 0.5;
        let got = BtsReversePowerControlState::brake_offset_db(midpoint);
        let expected = BRAKE_MAX_OFFSET_DB * 0.5;
        assert!(
            (got - expected).abs() < 1e-4,
            "midpoint brake: got {got}, expected {expected}",
        );
    }

    #[test]
    fn brake_offset_saturates_above_full() {
        assert_eq!(
            BtsReversePowerControlState::brake_offset_db(BRAKE_FULL_DBFS + 10.0),
            BRAKE_MAX_OFFSET_DB
        );
    }

    #[test]
    fn brake_converts_up_to_down_when_filt_hot() {
        let mut state = rc3_state_for_predictor();
        state.filtered_raw_power_db = Some(BRAKE_FULL_DBFS);
        let mut up = 0;
        let mut down = 0;
        for pcg in 0..200u64 {
            let tick = state.tick_single_pcg(10, pcg, -14.0, None, 0);
            if tick.pcb == 0 {
                up += 1;
            } else {
                down += 1;
            }
        }
        assert!(
            down > up,
            "hot filt should brake UP→DOWN: up={up} down={down}",
        );
    }

    #[test]
    fn brake_is_inert_when_filt_cold() {
        let mut state = rc3_state_for_predictor();
        state.filtered_raw_power_db = Some(BRAKE_BEGIN_DBFS - 5.0);
        let mut up = 0;
        let mut down = 0;
        for pcg in 0..200u64 {
            let tick = state.tick_single_pcg(10, pcg, -14.0, None, 0);
            if tick.pcb == 0 {
                up += 1;
            } else {
                down += 1;
            }
        }
        assert!(
            up > down,
            "cold filt should not engage brake: up={up} down={down}",
        );
        assert_eq!(state.last_brake_offset_db, 0.0);
    }
}

#[derive(Debug, Clone)]
struct BtsReversePowerControlState {
    target_db: f32,
    auto_min_db: f32,
    auto_max_db: f32,
    manual_min_db: f32,
    manual_max_db: f32,
    held_setpoint_db: Option<f32>,
    filtered_metric_db: Option<f32>,
    filtered_raw_power_db: Option<f32>,
    residual_db: f32,
    last_pcg_pilot_db: [f32; 16],
    last_pcbs: [u8; 16],
    crc_window: VecDeque<bool>,
    total_frames: u64,
    total_valid_frames: u64,
    total_crc_errors: u64,
    last_fer_pct: f32,
    adaptive_auto_floor_db: Option<f32>,
    adaptive_floor_clean_frames: u64,
    metric_history: VecDeque<f32>,
    last_predicted_metric_db: Option<f32>,
    last_slope_db_per_pcg: f32,
    last_brake_offset_db: f32,
    startup_seed: Option<StartupTargetSeedState>,
}

impl BtsReversePowerControlState {
    fn new_rc1() -> Self {
        Self::with_params(
            RC1_INITIAL_TARGET_DB,
            RC1_AUTO_MIN_DB,
            RC1_AUTO_MAX_DB,
            RC1_MANUAL_MIN_DB,
            RC1_MANUAL_MAX_DB,
            None,
        )
    }

    fn new_rc3() -> Self {
        Self::with_params(
            RC3_INITIAL_TARGET_DB,
            RC3_AUTO_MIN_DB,
            RC3_AUTO_MAX_DB,
            RC3_MANUAL_MIN_DB,
            RC3_MANUAL_MAX_DB,
            Some(StartupTargetSeedState::new(RC3_STARTUP_SEED_PCGS)),
        )
    }

    fn with_params(
        target_db: f32,
        auto_min_db: f32,
        auto_max_db: f32,
        manual_min_db: f32,
        manual_max_db: f32,
        startup_seed: Option<StartupTargetSeedState>,
    ) -> Self {
        Self {
            target_db,
            auto_min_db,
            auto_max_db,
            manual_min_db,
            manual_max_db,
            held_setpoint_db: None,
            filtered_metric_db: None,
            filtered_raw_power_db: None,
            residual_db: 0.0,
            last_pcg_pilot_db: [f32::NAN; 16],
            last_pcbs: [0; 16],
            crc_window: VecDeque::with_capacity(FER_WINDOW),
            total_frames: 0,
            total_valid_frames: 0,
            total_crc_errors: 0,
            last_fer_pct: 0.0,
            adaptive_auto_floor_db: None,
            adaptive_floor_clean_frames: 0,
            metric_history: VecDeque::with_capacity(METRIC_HISTORY_LEN),
            last_predicted_metric_db: None,
            last_slope_db_per_pcg: 0.0,
            last_brake_offset_db: 0.0,
            startup_seed,
        }
    }

    fn apply_setpoint(&mut self, setpoint: BtsReversePowerSetpoint) {
        let clamped = if setpoint.held {
            setpoint
                .target_db
                .clamp(self.manual_min_db, self.manual_max_db)
        } else {
            setpoint.target_db.clamp(self.auto_min_db, self.auto_max_db)
        };
        self.target_db = clamped;
        self.held_setpoint_db = setpoint.held.then_some(clamped);
        self.filtered_metric_db = None;
        self.filtered_raw_power_db = None;
        self.residual_db = 0.0;
        self.metric_history.clear();
        self.last_predicted_metric_db = None;
        self.last_slope_db_per_pcg = 0.0;
        self.last_brake_offset_db = 0.0;
        if setpoint.held
            && let Some(seed) = self.startup_seed.as_mut()
        {
            seed.seeded = true;
        }
    }

    fn effective_target_db(&self) -> f32 {
        self.held_setpoint_db.unwrap_or(self.target_db)
    }

    fn effective_auto_min_db(&self) -> f32 {
        self.adaptive_auto_floor_db
            .unwrap_or(self.auto_min_db)
            .clamp(self.auto_min_db, self.auto_max_db)
    }

    fn target_step_down_db() -> f32 {
        let fer_frac = TARGET_FER_PCT / 100.0;
        TARGET_STEP_UP_DB * fer_frac / (1.0 - fer_frac)
    }

    fn outer_loop_tick(&mut self, walsh_code: u8, crc_valid: bool) -> BtsPowerControlSnapshot {
        self.total_frames = self.total_frames.saturating_add(1);
        if crc_valid {
            self.total_valid_frames = self.total_valid_frames.saturating_add(1);
        } else {
            self.total_crc_errors = self.total_crc_errors.saturating_add(1);
        }
        if self.crc_window.len() == FER_WINDOW {
            self.crc_window.pop_front();
        }
        self.crc_window.push_back(crc_valid);
        if self.crc_window.len() >= FER_WINDOW {
            let errors = self.crc_window.iter().filter(|valid| !**valid).count();
            self.last_fer_pct = 100.0 * errors as f32 / self.crc_window.len() as f32;
        }

        if self.held_setpoint_db.is_none() && self.total_valid_frames >= OUTER_LOOP_MIN_VALID_FRAMES
        {
            if crc_valid {
                self.adaptive_floor_clean_frames =
                    self.adaptive_floor_clean_frames.saturating_add(1);
                if self.adaptive_floor_clean_frames >= ADAPTIVE_FLOOR_DECAY_FRAMES {
                    self.adaptive_floor_clean_frames = 0;
                    if let Some(floor) = self.adaptive_auto_floor_db {
                        let decayed = (floor - ADAPTIVE_FLOOR_DECAY_STEP_DB).max(self.auto_min_db);
                        self.adaptive_auto_floor_db =
                            (decayed > self.auto_min_db).then_some(decayed);
                    }
                }
                self.target_db = (self.target_db - Self::target_step_down_db())
                    .max(self.effective_auto_min_db());
            } else {
                self.adaptive_floor_clean_frames = 0;
                let step_up_db = if self.target_db
                    <= self.effective_auto_min_db() + ADAPTIVE_FLOOR_TRIGGER_BAND_DB
                {
                    let learned = (self.target_db + ADAPTIVE_FLOOR_STEP_UP_DB)
                        .clamp(self.auto_min_db, self.auto_max_db);
                    self.adaptive_auto_floor_db = (learned > self.auto_min_db).then_some(learned);
                    ADAPTIVE_FLOOR_STEP_UP_DB
                } else {
                    TARGET_STEP_UP_DB
                };
                self.target_db = (self.target_db + step_up_db)
                    .clamp(self.effective_auto_min_db(), self.auto_max_db);
            }
        }

        self.snapshot(walsh_code)
    }

    fn snapshot(&self, walsh_code: u8) -> BtsPowerControlSnapshot {
        BtsPowerControlSnapshot {
            walsh_code,
            target_eb_nt_db: self.target_db,
            effective_target_eb_nt_db: self.effective_target_db(),
            manual_target_override_db: self.held_setpoint_db,
            last_pcg_pilot_ec_nt_db: self.last_pcg_pilot_db,
            last_pcbs: self.last_pcbs,
            fer_pct: self.last_fer_pct,
            frames_total: self.total_frames,
            frames_crc_error: self.total_crc_errors,
            last_brake_offset_db: self.last_brake_offset_db,
        }
    }

    fn fallback_pcb_for_abs_pcg(abs_pcg: u64) -> u8 {
        (abs_pcg as u8) & 1
    }

    /// dB to subtract from the PCB error to brake UP commands at high Rx power.
    fn brake_offset_db(filtered_raw_power_db: f32) -> f32 {
        if !filtered_raw_power_db.is_finite() || filtered_raw_power_db <= BRAKE_BEGIN_DBFS {
            return 0.0;
        }
        let span = BRAKE_FULL_DBFS - BRAKE_BEGIN_DBFS;
        let frac = ((filtered_raw_power_db - BRAKE_BEGIN_DBFS) / span).clamp(0.0, 1.0);
        BRAKE_MAX_OFFSET_DB * frac
    }

    /// Least-squares fit of `y = a*t + b` over evenly-spaced samples
    /// `t = 0..n-1`. Returns `(intercept_at_newest_sample, slope_db_per_pcg)`.
    fn lsq_intercept_and_slope_at_newest(samples: &VecDeque<f32>) -> (f32, f32) {
        let n = samples.len();
        if n == 0 {
            return (f32::NAN, 0.0);
        }
        if n == 1 {
            return (samples[0], 0.0);
        }
        let nf = n as f32;
        let t_mean = (nf - 1.0) * 0.5;
        let y_mean: f32 = samples.iter().sum::<f32>() / nf;
        let mut num = 0.0_f32;
        let mut den = 0.0_f32;
        for (i, &y) in samples.iter().enumerate() {
            let dt = i as f32 - t_mean;
            num += dt * (y - y_mean);
            den += dt * dt;
        }
        let slope = if den > 0.0 { num / den } else { 0.0 };
        let intercept_at_newest = y_mean + slope * ((nf - 1.0) - t_mean);
        (intercept_at_newest, slope)
    }

    fn tick_single_pcg(
        &mut self,
        walsh_code: u8,
        abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
        lead_pcgs: u32,
    ) -> BtsPowerControlTick {
        let slot = (abs_pcg % 16) as usize;
        let measurement_valid = metric_db.is_finite();
        if measurement_valid {
            self.last_pcg_pilot_db[slot] = metric_db;
        }

        let raw_power_db = raw_power_db.filter(|db| db.is_finite());
        if let Some(raw_power_db) = raw_power_db {
            let filtered = match self.filtered_raw_power_db {
                Some(prev) => prev + RAW_POWER_FILTER_ALPHA * (raw_power_db - prev),
                None => raw_power_db,
            };
            self.filtered_raw_power_db = Some(filtered);
        }

        if self.held_setpoint_db.is_none() {
            if let Some(seed) = self.startup_seed.as_mut()
                && !seed.seeded
            {
                if measurement_valid {
                    seed.push(metric_db);
                    if seed.ready() {
                        if let Some(mean_db) = seed.robust_mean_db() {
                            let target_db = mean_db.clamp(self.auto_min_db, self.auto_max_db);
                            self.target_db = target_db;
                            self.filtered_metric_db = Some(target_db);
                            self.residual_db = 0.0;
                            log::info!(
                                "bts_power_control[w{}]: seeded RC3 target from {} PCGs: mean={:.2} dB target={:.2} dB",
                                walsh_code,
                                seed.samples.len(),
                                mean_db,
                                target_db,
                            );
                        }
                        seed.seeded = true;
                    }
                }
                let pcb = Self::fallback_pcb_for_abs_pcg(abs_pcg);
                self.last_pcbs[slot] = pcb;
                return BtsPowerControlTick {
                    pcb,
                    target_db: self.effective_target_db(),
                    control_metric_db: metric_db,
                    raw_power_db,
                    filtered_raw_power_db: self.filtered_raw_power_db,
                    raw_power_clamp_active: false,
                };
            }
        }

        let effective_target_db = self.effective_target_db();
        let (pcb, control_metric_db) = if measurement_valid {
            let first_measurement = self.filtered_metric_db.is_none();
            // Diagnostic EMA for the per-PCG log line — does not feed the loop.
            let filtered_metric_db = match self.filtered_metric_db {
                Some(prev) => prev + PCG_METRIC_FILTER_ALPHA * (metric_db - prev),
                None => metric_db,
            };
            self.filtered_metric_db = Some(filtered_metric_db);
            if first_measurement && self.held_setpoint_db.is_none() && self.startup_seed.is_none() {
                self.target_db = metric_db.clamp(self.auto_min_db, self.auto_max_db);
            }

            if self.metric_history.len() == METRIC_HISTORY_LEN {
                self.metric_history.pop_front();
            }
            self.metric_history.push_back(metric_db);

            let (intercept_at_now, slope) =
                Self::lsq_intercept_and_slope_at_newest(&self.metric_history);
            self.last_slope_db_per_pcg = slope;
            let raw_prediction_db = intercept_at_now + (lead_pcgs as f32) * slope;
            let predicted_metric_db = raw_prediction_db.clamp(
                intercept_at_now - PCG_PREDICTION_CLAMP_DB,
                intercept_at_now + PCG_PREDICTION_CLAMP_DB,
            );
            self.last_predicted_metric_db = Some(predicted_metric_db);

            let brake = self
                .filtered_raw_power_db
                .map(Self::brake_offset_db)
                .unwrap_or(0.0);
            self.last_brake_offset_db = brake;
            (
                self.quantize_pcb(effective_target_db - predicted_metric_db - brake),
                predicted_metric_db,
            )
        } else if let Some(held_metric) = self.last_predicted_metric_db {
            let brake = self
                .filtered_raw_power_db
                .map(Self::brake_offset_db)
                .unwrap_or(0.0);
            self.last_brake_offset_db = brake;
            (
                self.quantize_pcb(effective_target_db - held_metric - brake),
                held_metric,
            )
        } else {
            self.last_brake_offset_db = 0.0;
            (Self::fallback_pcb_for_abs_pcg(abs_pcg), f32::NAN)
        };

        self.last_pcbs[slot] = pcb;
        BtsPowerControlTick {
            pcb,
            target_db: effective_target_db,
            control_metric_db,
            raw_power_db,
            filtered_raw_power_db: self.filtered_raw_power_db,
            raw_power_clamp_active: false,
        }
    }

    fn quantize_pcb(&mut self, filtered_error_db: f32) -> u8 {
        let effective_error_db = if filtered_error_db.abs() <= PCG_HOLD_BAND_DB {
            0.0
        } else {
            filtered_error_db - PCG_HOLD_BAND_DB * filtered_error_db.signum()
        };
        let desired_step_db = (effective_error_db * PCG_RESPONSE_GAIN_DB_PER_DB)
            .clamp(-PCG_DESIRED_STEP_CLAMP_DB, PCG_DESIRED_STEP_CLAMP_DB);
        let residual = (self.residual_db + desired_step_db)
            .clamp(-PCG_RESIDUAL_CLAMP_DB, PCG_RESIDUAL_CLAMP_DB);
        let (pcb, applied_step_db) = if residual >= 0.0 { (0, 1.0) } else { (1, -1.0) };
        self.residual_db =
            (residual - applied_step_db).clamp(-PCG_RESIDUAL_CLAMP_DB, PCG_RESIDUAL_CLAMP_DB);
        pcb
    }
}

#[cfg(test)]
#[path = "power_control_sinr_tests.rs"]
mod sinr_tests;

#[derive(Clone, Default)]
pub struct BtsPowerControlRegistry {
    states: Arc<Mutex<HashMap<u8, BtsReversePowerControlState>>>,
}

impl BtsPowerControlRegistry {
    fn traffic_channel_uses_rc3(traffic_channels: &TrafficChannelPool, walsh_code: u8) -> bool {
        traffic_channels
            .lock()
            .iter()
            .find(|slot| slot.walsh_code == walsh_code)
            .map(|slot| matches!(slot.channel, TrafficChannelWrapper::Rc3(_)))
            .unwrap_or(true)
    }

    pub fn set_target(&self, walsh_code: u8, target_db: f32, held: bool) {
        let mut states = self.states.lock();
        let state = states
            .entry(walsh_code)
            .or_insert_with(BtsReversePowerControlState::new_rc3);
        state.apply_setpoint(BtsReversePowerSetpoint { target_db, held });
    }

    pub fn outer_loop_tick(
        &self,
        traffic_channels: Option<&TrafficChannelPool>,
        walsh_code: u8,
        frame_valid: bool,
    ) -> BtsPowerControlSnapshot {
        let use_rc3 = traffic_channels
            .map(|channels| Self::traffic_channel_uses_rc3(channels, walsh_code))
            .unwrap_or(true);
        let mut states = self.states.lock();
        let state = states.entry(walsh_code).or_insert_with(|| {
            if use_rc3 {
                BtsReversePowerControlState::new_rc3()
            } else {
                BtsReversePowerControlState::new_rc1()
            }
        });
        state.outer_loop_tick(walsh_code, frame_valid)
    }

    pub fn snapshot(&self, walsh_code: u8) -> Option<BtsPowerControlSnapshot> {
        self.states
            .lock()
            .get(&walsh_code)
            .map(|state| state.snapshot(walsh_code))
    }

    pub fn snapshots(&self) -> Vec<BtsPowerControlSnapshot> {
        self.states
            .lock()
            .iter()
            .map(|(&walsh_code, state)| state.snapshot(walsh_code))
            .collect()
    }

    pub fn tick_and_schedule(
        &self,
        traffic_channels: &TrafficChannelPool,
        walsh_code: u8,
        measured_abs_pcg: u64,
        tx_abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
    ) -> Option<BtsPowerControlTick> {
        let use_rc3 = {
            let slots = traffic_channels.lock();
            let slot = slots.iter().find(|slot| slot.walsh_code == walsh_code)?;
            matches!(slot.channel, TrafficChannelWrapper::Rc3(_))
        };
        let tick = {
            let mut states = self.states.lock();
            let state = states.entry(walsh_code).or_insert_with(|| {
                if use_rc3 {
                    BtsReversePowerControlState::new_rc3()
                } else {
                    BtsReversePowerControlState::new_rc1()
                }
            });
            let lead_pcgs = tx_abs_pcg
                .saturating_sub(measured_abs_pcg)
                .min(PCG_PREDICTION_LEAD_PCGS as u64) as u32;
            state.tick_single_pcg(
                walsh_code,
                measured_abs_pcg,
                metric_db,
                raw_power_db,
                lead_pcgs,
            )
        };

        let slots = traffic_channels.lock();
        let slot = slots.iter().find(|slot| slot.walsh_code == walsh_code)?;
        match &slot.channel {
            TrafficChannelWrapper::Rc1(ch) => {
                ch.channel.schedule_power_control_bit(tx_abs_pcg, tick.pcb)
            }
            TrafficChannelWrapper::Rc3(ch) => {
                ch.channel.schedule_power_control_bit(tx_abs_pcg, tick.pcb)
            }
            TrafficChannelWrapper::SchRc3(_) => return None,
        }
        Some(tick)
    }
}
