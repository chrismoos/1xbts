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
const RC3_INITIAL_TARGET_DB: f32 = -5.5;
const RC3_AUTO_MIN_DB: f32 = -8.0;
const RC3_AUTO_MAX_DB: f32 = -1.5;
const RC3_MANUAL_MIN_DB: f32 = -40.0;
const RC3_MANUAL_MAX_DB: f32 = 40.0;
const PCG_METRIC_FILTER_ALPHA: f32 = 0.05;
const PCG_HOLD_BAND_DB: f32 = 0.25;
const PCG_RESPONSE_GAIN_DB_PER_DB: f32 = 0.6;
const PCG_DESIRED_STEP_CLAMP_DB: f32 = 1.0;
const PCG_RESIDUAL_CLAMP_DB: f32 = 1.0;
const RAW_POWER_FILTER_ALPHA: f32 = 0.05;
/// Attack/release EMA alphas for the brake-input raw-power filter.
const BRAKE_RAW_POWER_ATTACK_ALPHA: f32 = 1.0 / 40.0;
const BRAKE_RAW_POWER_RELEASE_ALPHA: f32 = 1.0 / 16.0;
const FER_WINDOW: usize = 50;
const OUTER_LOOP_MIN_VALID_FRAMES: u64 = 10;
const TARGET_FER_PCT: f32 = 1.0;
const TARGET_STEP_UP_DB: f32 = 0.25;
const TARGET_UNDERPOWER_ERROR_STEP_UP_DB: f32 = 0.5;
const TARGET_OVERPOWER_ERROR_STEP_DOWN_DB: f32 = 0.25;
const TARGET_OVERPOWER_CLEAN_STEP_DOWN_DB: f32 = 0.1;
const ADAPTIVE_FLOOR_TRIGGER_BAND_DB: f32 = 0.25;
const ADAPTIVE_FLOOR_STEP_UP_DB: f32 = 0.1;
const ADAPTIVE_FLOOR_DECAY_FRAMES: u64 = 250;
const ADAPTIVE_FLOOR_DECAY_STEP_DB: f32 = 0.1;
const METRIC_HISTORY_LEN: usize = 24;
/// PCGs the inner loop predicts ahead before scheduling the PCB.
/// Must exceed metric arrival age or the TX scheduler runs late.
pub(super) const PCG_PREDICTION_LEAD_PCGS: u32 = 12;
const PCG_PREDICTION_CLAMP_DB: f32 = 1.0;
// Brake offset subtracted from the PCB error in the pre-clip region to keep
// the reverse link below the ADC knee.
const BRAKE_BEGIN_DBFS: f32 = -22.0;
const BRAKE_FULL_DBFS: f32 = -12.0;
const BRAKE_MAX_OFFSET_DB: f32 = 8.0;
const CLIP_BEGIN_DBFS: f32 = -8.0;
const PCG_CLIP_COOLDOWN_PCGS: u8 = 32;
const PCG_FORCE_DOWN_BRAKE_DB: f32 = 5.0;
const PCG_PRECLIP_GUARD_DBFS: f32 = -16.0;
const OUTER_LOOP_OVERPOWER_CLIP_PCGS: usize = 4;
const OUTER_LOOP_UNDERPOWER_ERROR_DB: f32 = 4.0;
const OUTER_LOOP_UNDERPOWER_MIN_UP_PCBS: usize = 12;
const OUTER_LOOP_UNDERPOWER_MAX_RAW_DBFS: f32 = -20.0;
const OUTER_LOOP_UNDERPOWER_MAX_BRAKE_DB: f32 = 2.0;
const OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS: usize = 8;
const OUTER_LOOP_CELL_SETTLING_FRAMES: u64 = 0;
const OUTER_LOOP_TRANSIENT_RECOVERY_FRAMES: u64 = 0;
const HOT_START_PREAMBLE_RAW_DBFS: f32 = -30.0;
const HOT_START_BOOTSTRAP_DOWN_PCGS: u64 = 40;
const HOT_START_HARD_RAW_DBFS: f32 = -16.0;
const HOT_START_SOFT_RAW_DBFS: f32 = -18.0;
const HOT_START_RELEASE_RAW_DBFS: f32 = -21.0;
const HOT_START_HARD_BRAKE_DB: f32 = 5.0;
const HOT_START_SOFT_BRAKE_DB: f32 = 3.0;
const HOT_START_RELEASE_BRAKE_DB: f32 = 1.5;
const HOT_START_SOFT_EXTEND_PCGS: u16 = 48;
const HOT_START_HARD_EXTEND_PCGS: u16 = 160;
const HOT_START_RELEASE_DECAY_PCGS: u16 = 8;

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
        )
    }

    #[test]
    fn raw_power_filter_updates_after_first_pcg() {
        let mut state = rc3_state_without_startup_seed();

        let first = state.tick_single_pcg(10, 100, -20.0, Some(-20.0), 0);
        assert_eq!(first.filtered_raw_power_db, Some(-20.0));
        assert!(!first.raw_power_clamp_active);

        let second = state.tick_single_pcg(10, 101, -20.0, Some(-30.0), 0);
        assert!(second.filtered_raw_power_db.is_some_and(|db| db < -20.0));
        assert!(!second.raw_power_clamp_active);
    }

    #[test]
    fn pre_clip_brake_engages_without_hard_clamp() {
        let mut state = rc3_state_without_startup_seed();
        let tick = state.tick_single_pcg(
            10,
            100,
            -20.0,
            Some((BRAKE_BEGIN_DBFS + PCG_PRECLIP_GUARD_DBFS) * 0.5),
            0,
        );
        assert!(state.last_brake_offset_db > 0.0);
        assert!(!tick.raw_power_clamp_active);
    }

    #[test]
    fn startup_clip_forces_down_and_reports_clamp() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let tick = state.tick_single_pcg(10, 100, -30.0, Some(CLIP_BEGIN_DBFS + 1.0), 0);
        assert_eq!(tick.pcb, 1);
        assert!(tick.raw_power_clamp_active);
    }

    fn rc3_state_for_predictor() -> BtsReversePowerControlState {
        BtsReversePowerControlState::with_params(
            RC3_INITIAL_TARGET_DB,
            RC3_AUTO_MIN_DB,
            RC3_AUTO_MAX_DB,
            RC3_MANUAL_MIN_DB,
            RC3_MANUAL_MAX_DB,
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
        state.brake_filtered_raw_power_db = Some(BRAKE_FULL_DBFS);
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
        state.brake_filtered_raw_power_db = Some(BRAKE_BEGIN_DBFS - 5.0);
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
    brake_filtered_raw_power_db: Option<f32>,
    residual_db: f32,
    last_pcg_pilot_db: [f32; 16],
    last_pcg_control_metric_db: [f32; 16],
    last_pcg_raw_power_db: [f32; 16],
    last_pcg_brake_offset_db: [f32; 16],
    last_pcg_clipped: [bool; 16],
    last_pcbs: [u8; 16],
    crc_window: VecDeque<bool>,
    total_frames: u64,
    total_valid_frames: u64,
    target_adaptation_valid_frames: u64,
    total_crc_errors: u64,
    last_fer_pct: f32,
    adaptive_auto_floor_db: Option<f32>,
    adaptive_floor_clean_frames: u64,
    metric_history: VecDeque<f32>,
    last_predicted_metric_db: Option<f32>,
    last_slope_db_per_pcg: f32,
    last_brake_offset_db: f32,
    clip_cooldown_pcgs: u8,
    hot_start_guard_remaining_pcgs: u16,
    transient_recovery_frames_remaining: u64,
}

impl BtsReversePowerControlState {
    fn new_rc1() -> Self {
        Self::with_params(
            RC1_INITIAL_TARGET_DB,
            RC1_AUTO_MIN_DB,
            RC1_AUTO_MAX_DB,
            RC1_MANUAL_MIN_DB,
            RC1_MANUAL_MAX_DB,
        )
    }

    fn new_rc3() -> Self {
        Self::with_params(
            RC3_INITIAL_TARGET_DB,
            RC3_AUTO_MIN_DB,
            RC3_AUTO_MAX_DB,
            RC3_MANUAL_MIN_DB,
            RC3_MANUAL_MAX_DB,
        )
    }

    fn with_params(
        target_db: f32,
        auto_min_db: f32,
        auto_max_db: f32,
        manual_min_db: f32,
        manual_max_db: f32,
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
            brake_filtered_raw_power_db: None,
            residual_db: 0.0,
            last_pcg_pilot_db: [f32::NAN; 16],
            last_pcg_control_metric_db: [f32::NAN; 16],
            last_pcg_raw_power_db: [f32::NAN; 16],
            last_pcg_brake_offset_db: [0.0; 16],
            last_pcg_clipped: [false; 16],
            last_pcbs: [0; 16],
            crc_window: VecDeque::with_capacity(FER_WINDOW),
            total_frames: 0,
            total_valid_frames: 0,
            target_adaptation_valid_frames: 0,
            total_crc_errors: 0,
            last_fer_pct: 0.0,
            adaptive_auto_floor_db: None,
            adaptive_floor_clean_frames: 0,
            metric_history: VecDeque::with_capacity(METRIC_HISTORY_LEN),
            last_predicted_metric_db: None,
            last_slope_db_per_pcg: 0.0,
            last_brake_offset_db: 0.0,
            clip_cooldown_pcgs: 0,
            hot_start_guard_remaining_pcgs: 0,
            transient_recovery_frames_remaining: 0,
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
        self.brake_filtered_raw_power_db = None;
        self.residual_db = 0.0;
        self.last_pcg_pilot_db = [f32::NAN; 16];
        self.last_pcg_control_metric_db = [f32::NAN; 16];
        self.last_pcg_raw_power_db = [f32::NAN; 16];
        self.last_pcg_brake_offset_db = [0.0; 16];
        self.last_pcg_clipped = [false; 16];
        self.last_pcbs = [0; 16];
        self.metric_history.clear();
        self.last_predicted_metric_db = None;
        self.last_slope_db_per_pcg = 0.0;
        self.last_brake_offset_db = 0.0;
        self.clip_cooldown_pcgs = 0;
        self.hot_start_guard_remaining_pcgs = 0;
        self.transient_recovery_frames_remaining = 0;
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

    fn outer_loop_tick(
        &mut self,
        walsh_code: u8,
        crc_valid: bool,
        cell_settling_frames_remaining: Option<u64>,
    ) -> BtsPowerControlSnapshot {
        let has_frame_observations = self.has_frame_observations();
        let frame_overpowered = has_frame_observations && self.recent_frame_overpowered();
        let frame_underpowered =
            has_frame_observations && !frame_overpowered && self.recent_frame_underpowered();

        if crc_valid {
            self.total_valid_frames = self.total_valid_frames.saturating_add(1);
        } else {
            self.total_crc_errors = self.total_crc_errors.saturating_add(1);
        }
        self.total_frames = self.total_frames.saturating_add(1);
        if self.crc_window.len() == FER_WINDOW {
            self.crc_window.pop_front();
        }
        self.crc_window.push_back(crc_valid);
        if self.crc_window.len() >= FER_WINDOW {
            let errors = self.crc_window.iter().filter(|valid| !**valid).count();
            self.last_fer_pct = 100.0 * errors as f32 / self.crc_window.len() as f32;
        }

        if !has_frame_observations {
            if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
                log::info!(
                    "power_frame[w{}]: crc={} counted=no_pcg_observation frame={} fer={:.2}% target={:.2}",
                    walsh_code,
                    crc_valid as u8,
                    self.total_frames,
                    self.last_fer_pct,
                    self.effective_target_db(),
                );
            }
            return self.snapshot(walsh_code);
        }
        if crc_valid {
            self.target_adaptation_valid_frames =
                self.target_adaptation_valid_frames.saturating_add(1);
        }

        if self.held_setpoint_db.is_none() && cell_settling_frames_remaining.is_none() {
            if crc_valid {
                if self.target_adaptation_valid_frames >= OUTER_LOOP_MIN_VALID_FRAMES {
                    self.adaptive_floor_clean_frames =
                        self.adaptive_floor_clean_frames.saturating_add(1);
                    if self.adaptive_floor_clean_frames >= ADAPTIVE_FLOOR_DECAY_FRAMES {
                        self.adaptive_floor_clean_frames = 0;
                        if let Some(floor) = self.adaptive_auto_floor_db {
                            let decayed =
                                (floor - ADAPTIVE_FLOOR_DECAY_STEP_DB).max(self.auto_min_db);
                            self.adaptive_auto_floor_db =
                                (decayed > self.auto_min_db).then_some(decayed);
                        }
                    }
                    let step_down_db = if frame_overpowered {
                        TARGET_OVERPOWER_CLEAN_STEP_DOWN_DB
                    } else {
                        Self::target_step_down_db()
                    };
                    self.target_db =
                        (self.target_db - step_down_db).max(self.effective_auto_min_db());
                }
            } else {
                self.adaptive_floor_clean_frames = 0;
                if frame_overpowered {
                    self.relax_adaptive_floor(TARGET_OVERPOWER_ERROR_STEP_DOWN_DB);
                    self.target_db = (self.target_db - TARGET_OVERPOWER_ERROR_STEP_DOWN_DB)
                        .max(self.effective_auto_min_db());
                } else if frame_underpowered {
                    self.target_db = (self.target_db + TARGET_UNDERPOWER_ERROR_STEP_UP_DB)
                        .clamp(self.effective_auto_min_db(), self.auto_max_db);
                } else if self.target_adaptation_valid_frames >= OUTER_LOOP_MIN_VALID_FRAMES {
                    let step_up_db = if self.target_db
                        <= self.effective_auto_min_db() + ADAPTIVE_FLOOR_TRIGGER_BAND_DB
                    {
                        let learned = (self.target_db + ADAPTIVE_FLOOR_STEP_UP_DB)
                            .clamp(self.auto_min_db, self.auto_max_db);
                        self.adaptive_auto_floor_db =
                            (learned > self.auto_min_db).then_some(learned);
                        ADAPTIVE_FLOOR_STEP_UP_DB
                    } else {
                        TARGET_STEP_UP_DB
                    };
                    self.target_db = (self.target_db + step_up_db)
                        .clamp(self.effective_auto_min_db(), self.auto_max_db);
                }
            }
        }

        if !crc_valid && frame_overpowered {
            self.residual_db = -PCG_RESIDUAL_CLAMP_DB;
            self.metric_history.clear();
            self.last_predicted_metric_db = None;
            self.filtered_metric_db = None;
            self.transient_recovery_frames_remaining = OUTER_LOOP_TRANSIENT_RECOVERY_FRAMES;
        }
        let transient_recovery_remaining = if self.transient_recovery_frames_remaining > 0 {
            let remaining = self.transient_recovery_frames_remaining;
            self.transient_recovery_frames_remaining =
                self.transient_recovery_frames_remaining.saturating_sub(1);
            Some(remaining)
        } else {
            None
        };
        if let Some(remaining) = cell_settling_frames_remaining {
            if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
                log::info!(
                    "power_frame[w{}]: crc={} counted=cell_settling remaining={} frame={} fer={:.2}% target={:.2} opwr={}",
                    walsh_code,
                    crc_valid as u8,
                    remaining,
                    self.total_frames,
                    self.last_fer_pct,
                    self.effective_target_db(),
                    frame_overpowered as u8,
                );
            }
            return self.snapshot(walsh_code);
        }

        if let Some(remaining) = transient_recovery_remaining {
            if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
                log::info!(
                    "power_frame[w{}]: crc={} counted=transient_recovery remaining={} target={:.2} opwr={}",
                    walsh_code,
                    crc_valid as u8,
                    remaining,
                    self.effective_target_db(),
                    frame_overpowered as u8,
                );
            }
        }

        if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
            self.log_frame_correlation(walsh_code, crc_valid);
        }

        self.snapshot(walsh_code)
    }

    fn relax_adaptive_floor(&mut self, step_down_db: f32) {
        if let Some(floor) = self.adaptive_auto_floor_db {
            let decayed = (floor - step_down_db).max(self.auto_min_db);
            self.adaptive_auto_floor_db = (decayed > self.auto_min_db).then_some(decayed);
        }
    }

    fn recent_clip_pcgs(&self) -> usize {
        self.last_pcg_clipped
            .iter()
            .filter(|clipped| **clipped)
            .count()
    }

    fn has_frame_observations(&self) -> bool {
        let raw_count = Self::finite_stats(&self.last_pcg_raw_power_db)
            .map(|(_, _, _, count)| count)
            .unwrap_or(0);
        let control_count = Self::finite_stats(&self.last_pcg_control_metric_db)
            .map(|(_, _, _, count)| count)
            .unwrap_or(0);
        raw_count >= OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS
            && control_count >= OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS
    }

    fn recent_frame_overpowered(&self) -> bool {
        self.recent_clip_pcgs() >= OUTER_LOOP_OVERPOWER_CLIP_PCGS
    }

    fn recent_frame_underpowered(&self) -> bool {
        if Self::finite_stats(&self.last_pcg_raw_power_db)
            .map(|(avg, _, _, _)| avg >= OUTER_LOOP_UNDERPOWER_MAX_RAW_DBFS)
            .unwrap_or(false)
        {
            return false;
        }
        if Self::finite_stats(&self.last_pcg_brake_offset_db)
            .map(|(avg, _, _, _)| avg >= OUTER_LOOP_UNDERPOWER_MAX_BRAKE_DB)
            .unwrap_or(false)
        {
            return false;
        }
        let pcb_up = self.last_pcbs.iter().filter(|&&pcb| pcb == 0).count();
        if pcb_up < OUTER_LOOP_UNDERPOWER_MIN_UP_PCBS {
            return false;
        }
        let Some((control_avg, _, _, _)) = Self::finite_stats(&self.last_pcg_control_metric_db)
        else {
            return false;
        };
        self.effective_target_db() - control_avg >= OUTER_LOOP_UNDERPOWER_ERROR_DB
    }

    fn finite_stats(values: &[f32]) -> Option<(f32, f32, f32, usize)> {
        let mut count = 0usize;
        let mut sum = 0.0_f32;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for &value in values {
            if !value.is_finite() {
                continue;
            }
            count += 1;
            sum += value;
            min = min.min(value);
            max = max.max(value);
        }
        (count > 0).then_some((sum / count as f32, min, max, count))
    }

    fn log_frame_correlation(&self, walsh_code: u8, crc_valid: bool) {
        if crc_valid && self.last_fer_pct < TARGET_FER_PCT {
            return;
        }
        let metric = Self::finite_stats(&self.last_pcg_pilot_db);
        let control = Self::finite_stats(&self.last_pcg_control_metric_db);
        let raw = Self::finite_stats(&self.last_pcg_raw_power_db);
        let brake = Self::finite_stats(&self.last_pcg_brake_offset_db);
        let clip_pcgs = self.recent_clip_pcgs();
        let overpowered = self.recent_frame_overpowered();
        let pcb_up = self.last_pcbs.iter().filter(|&&pcb| pcb == 0).count();
        let target = self.effective_target_db();
        let control_error = control
            .map(|(avg, _, _, _)| target - avg)
            .unwrap_or(f32::NAN);
        log::info!(
            "power_frame[w{}]: crc={} frame={} fer={:.2}% target={:.2} opwr={} err_avg={:+.2} metric={} control={} raw={} clip_pcgs={}/16 brake={} pcb_up={}/16",
            walsh_code,
            crc_valid as u8,
            self.total_frames,
            self.last_fer_pct,
            target,
            overpowered as u8,
            control_error,
            Self::format_stats(metric),
            Self::format_stats(control),
            Self::format_stats(raw),
            clip_pcgs,
            Self::format_stats(brake),
            pcb_up,
        );
    }

    fn format_stats(stats: Option<(f32, f32, f32, usize)>) -> String {
        stats
            .map(|(avg, min, max, count)| {
                format!("avg={avg:.2}/min={min:.2}/max={max:.2}/n={count}")
            })
            .unwrap_or_else(|| "none".to_string())
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

    fn brake_input_raw_power_db(
        filtered_raw_power_db: Option<f32>,
        instant_raw_power_db: Option<f32>,
    ) -> Option<f32> {
        match (filtered_raw_power_db, instant_raw_power_db) {
            (Some(filtered), Some(instant)) => Some(filtered.max(instant)),
            (Some(filtered), None) => Some(filtered),
            (None, Some(instant)) => Some(instant),
            (None, None) => None,
        }
    }

    fn extend_hot_start_guard(&mut self, pcgs: u16) {
        self.hot_start_guard_remaining_pcgs = self.hot_start_guard_remaining_pcgs.max(pcgs);
    }

    fn prime_hot_start_guard(&mut self, raw_power_db: f32) {
        if !raw_power_db.is_finite() || raw_power_db < HOT_START_PREAMBLE_RAW_DBFS {
            return;
        }
        self.filtered_raw_power_db = Some(
            self.filtered_raw_power_db
                .map(|filtered| filtered.max(raw_power_db))
                .unwrap_or(raw_power_db),
        );
        self.brake_filtered_raw_power_db = Some(
            self.brake_filtered_raw_power_db
                .map(|filtered| filtered.max(raw_power_db))
                .unwrap_or(raw_power_db),
        );
        self.extend_hot_start_guard(HOT_START_SOFT_EXTEND_PCGS);
        if raw_power_db >= HOT_START_HARD_RAW_DBFS {
            self.clip_cooldown_pcgs = self.clip_cooldown_pcgs.max(PCG_CLIP_COOLDOWN_PCGS);
        }
    }

    fn update_hot_start_guard(
        &mut self,
        raw_power_db: Option<f32>,
        brake_offset_db: f32,
        is_clipping: bool,
    ) -> bool {
        let hard_raw = raw_power_db
            .map(|db| db >= HOT_START_HARD_RAW_DBFS)
            .unwrap_or(false);
        let soft_raw = raw_power_db
            .map(|db| db >= HOT_START_SOFT_RAW_DBFS)
            .unwrap_or(false);
        let released_raw = raw_power_db
            .map(|db| db <= HOT_START_RELEASE_RAW_DBFS)
            .unwrap_or(true);

        if is_clipping || hard_raw || brake_offset_db >= HOT_START_HARD_BRAKE_DB {
            self.extend_hot_start_guard(HOT_START_HARD_EXTEND_PCGS);
        } else if self.hot_start_guard_remaining_pcgs > 0
            && (soft_raw || brake_offset_db >= HOT_START_SOFT_BRAKE_DB)
        {
            self.extend_hot_start_guard(HOT_START_SOFT_EXTEND_PCGS);
        } else if self.hot_start_guard_remaining_pcgs > 0 {
            let decay = if released_raw && brake_offset_db <= HOT_START_RELEASE_BRAKE_DB {
                HOT_START_RELEASE_DECAY_PCGS
            } else {
                1
            };
            self.hot_start_guard_remaining_pcgs =
                self.hot_start_guard_remaining_pcgs.saturating_sub(decay);
        }

        self.hot_start_guard_remaining_pcgs > 0
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

        let raw_power_db = raw_power_db.filter(|db| db.is_finite());
        if let Some(raw_power_db) = raw_power_db {
            self.last_pcg_raw_power_db[slot] = raw_power_db;
            let filtered = match self.filtered_raw_power_db {
                Some(prev) => prev + RAW_POWER_FILTER_ALPHA * (raw_power_db - prev),
                None => raw_power_db,
            };
            self.filtered_raw_power_db = Some(filtered);
            let brake_filtered = match self.brake_filtered_raw_power_db {
                Some(prev) => {
                    let alpha = if raw_power_db > prev {
                        BRAKE_RAW_POWER_ATTACK_ALPHA
                    } else {
                        BRAKE_RAW_POWER_RELEASE_ALPHA
                    };
                    prev + alpha * (raw_power_db - prev)
                }
                None => raw_power_db,
            };
            self.brake_filtered_raw_power_db = Some(brake_filtered);
        }

        // Clipping inflates pilot variance and fakes a low-SINR reading, so
        // reject the measurement and force DOWN while in the clipping zone.
        let is_clipping = raw_power_db.map(|db| db > CLIP_BEGIN_DBFS).unwrap_or(false);
        let preclip_guard_active = raw_power_db
            .map(|db| db >= PCG_PRECLIP_GUARD_DBFS)
            .unwrap_or(false);
        let clip_guard_active = if is_clipping {
            self.clip_cooldown_pcgs = PCG_CLIP_COOLDOWN_PCGS;
            true
        } else if self.clip_cooldown_pcgs > 0 {
            self.clip_cooldown_pcgs = self.clip_cooldown_pcgs.saturating_sub(1);
            true
        } else {
            false
        };
        self.last_pcg_clipped[slot] = is_clipping;
        let measurement_valid = metric_db.is_finite() && !is_clipping;
        if measurement_valid {
            self.last_pcg_pilot_db[slot] = metric_db;
        }

        let effective_target_db = self.effective_target_db();
        let brake = Self::brake_input_raw_power_db(self.brake_filtered_raw_power_db, raw_power_db)
            .map(Self::brake_offset_db)
            .unwrap_or(0.0);
        self.last_brake_offset_db = brake;
        let hot_start_guard_active = self.update_hot_start_guard(raw_power_db, brake, is_clipping);
        let (pcb, control_metric_db) = if measurement_valid {
            // Diagnostic EMA for the per-PCG log line — does not feed the loop.
            let filtered_metric_db = match self.filtered_metric_db {
                Some(prev) => prev + PCG_METRIC_FILTER_ALPHA * (metric_db - prev),
                None => metric_db,
            };
            self.filtered_metric_db = Some(filtered_metric_db);

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

            (
                self.quantize_pcb(effective_target_db - predicted_metric_db - brake),
                predicted_metric_db,
            )
        } else if let Some(held_metric) = self.last_predicted_metric_db {
            (
                self.quantize_pcb(effective_target_db - held_metric - brake),
                held_metric,
            )
        } else {
            (Self::fallback_pcb_for_abs_pcg(abs_pcg), f32::NAN)
        };

        // Force DOWN in the clipping/guard zone, overriding the loop error.
        let raw_power_clamp_active = preclip_guard_active
            || clip_guard_active
            || hot_start_guard_active
            || self.last_brake_offset_db >= PCG_FORCE_DOWN_BRAKE_DB;
        let pcb = if raw_power_clamp_active { 1 } else { pcb };

        self.last_pcg_control_metric_db[slot] = control_metric_db;
        self.last_pcg_brake_offset_db[slot] = self.last_brake_offset_db;

        if cdma_common::diagnostics::power_control_verbose_per_pcg_enabled_for_walsh(walsh_code) {
            log::info!(
                "pcg[w{}] abs_pcg={} raw={:?} brake_filt={:?} clip={} hot_guard={} metric_db={:.2} pred={:.2} target={:.2} brake={:.2} residual={:+.2} pcb={}",
                walsh_code,
                abs_pcg,
                raw_power_db,
                self.brake_filtered_raw_power_db,
                is_clipping as u8,
                self.hot_start_guard_remaining_pcgs,
                metric_db,
                control_metric_db,
                effective_target_db,
                self.last_brake_offset_db,
                self.residual_db,
                pcb,
            );
        }

        self.last_pcbs[slot] = pcb;
        BtsPowerControlTick {
            pcb,
            target_db: effective_target_db,
            control_metric_db,
            raw_power_db,
            filtered_raw_power_db: self.filtered_raw_power_db,
            raw_power_clamp_active,
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

#[derive(Clone)]
pub struct BtsPowerControlRegistry {
    states: Arc<Mutex<HashMap<u8, BtsReversePowerControlState>>>,
    cell_settling_frames_remaining: Arc<Mutex<u64>>,
}

impl Default for BtsPowerControlRegistry {
    fn default() -> Self {
        Self {
            states: Arc::default(),
            cell_settling_frames_remaining: Arc::default(),
        }
    }
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
        let is_new_state = !states.contains_key(&walsh_code);
        let state = states.entry(walsh_code).or_insert_with(|| {
            if use_rc3 {
                BtsReversePowerControlState::new_rc3()
            } else {
                BtsReversePowerControlState::new_rc1()
            }
        });
        if is_new_state && use_rc3 {
            self.start_cell_settling(walsh_code);
        }
        let cell_settling_frames_remaining = state
            .has_frame_observations()
            .then(|| self.take_cell_settling_frame())
            .flatten();
        state.outer_loop_tick(walsh_code, frame_valid, cell_settling_frames_remaining)
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
            let is_new_state = !states.contains_key(&walsh_code);
            let state = states.entry(walsh_code).or_insert_with(|| {
                if use_rc3 {
                    BtsReversePowerControlState::new_rc3()
                } else {
                    BtsReversePowerControlState::new_rc1()
                }
            });
            if is_new_state && use_rc3 {
                self.start_cell_settling(walsh_code);
            }
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

    pub fn schedule_down_burst(
        &self,
        traffic_channels: &TrafficChannelPool,
        walsh_code: u8,
        start_abs_pcg: u64,
        pcgs: u64,
    ) -> bool {
        let slots = traffic_channels.lock();
        let Some(slot) = slots.iter().find(|slot| slot.walsh_code == walsh_code) else {
            return false;
        };
        for offset in 0..pcgs {
            let abs_pcg = start_abs_pcg.saturating_add(offset);
            match &slot.channel {
                TrafficChannelWrapper::Rc1(ch) => ch.channel.schedule_power_control_bit(abs_pcg, 1),
                TrafficChannelWrapper::Rc3(ch) => ch.channel.schedule_power_control_bit(abs_pcg, 1),
                TrafficChannelWrapper::SchRc3(_) => return false,
            }
        }
        true
    }

    pub fn note_hot_preamble(&self, walsh_code: u8, raw_power_db: f32) -> bool {
        if !raw_power_db.is_finite() || raw_power_db < HOT_START_PREAMBLE_RAW_DBFS {
            return false;
        }
        let mut states = self.states.lock();
        let state = states
            .entry(walsh_code)
            .or_insert_with(BtsReversePowerControlState::new_rc3);
        state.prime_hot_start_guard(raw_power_db);
        true
    }

    pub fn hot_start_bootstrap_down_pcgs() -> u64 {
        HOT_START_BOOTSTRAP_DOWN_PCGS
    }

    fn start_cell_settling(&self, walsh_code: u8) {
        *self.cell_settling_frames_remaining.lock() = OUTER_LOOP_CELL_SETTLING_FRAMES;
        if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
            log::info!(
                "bts_power_control[w{}]: cell settling for {} reverse frames",
                walsh_code,
                OUTER_LOOP_CELL_SETTLING_FRAMES,
            );
        }
    }

    fn take_cell_settling_frame(&self) -> Option<u64> {
        let mut remaining = self.cell_settling_frames_remaining.lock();
        if *remaining == 0 {
            return None;
        }
        *remaining = (*remaining).saturating_sub(1);
        Some(*remaining)
    }
}
