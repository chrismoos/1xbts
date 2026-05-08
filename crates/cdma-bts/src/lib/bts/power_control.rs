use std::{collections::HashMap, collections::VecDeque, sync::Arc};

use parking_lot::Mutex;

use super::handle::{TrafficChannelPool, TrafficChannelWrapper};

const RC1_INITIAL_TARGET_DB: f32 = 10.0;
const RC1_AUTO_MIN_DB: f32 = 8.0;
const RC1_AUTO_MAX_DB: f32 = 12.0;
const RC1_MANUAL_MIN_DB: f32 = 0.0;
const RC1_MANUAL_MAX_DB: f32 = 40.0;
const RC3_INITIAL_TARGET_DB: f32 = -5.0;
const RC3_AUTO_MIN_DB: f32 = -10.0;
const RC3_AUTO_MAX_DB: f32 = -3.0;
const RC3_MANUAL_MIN_DB: f32 = -15.0;
const RC3_MANUAL_MAX_DB: f32 = 40.0;
const PCG_METRIC_FILTER_ALPHA: f32 = 0.05;
const PCG_HOLD_BAND_DB: f32 = 0.15;
const PCG_RESPONSE_GAIN_DB_PER_DB: f32 = 0.5;
const PCG_DESIRED_STEP_CLAMP_DB: f32 = 1.0;
const PCG_RESIDUAL_CLAMP_DB: f32 = 2.0;
const RAW_POWER_FILTER_ALPHA: f32 = 0.05;
const RAW_POWER_DOWN_CLAMP_THRESHOLD_DBFS: f32 = -23.0;
const RAW_POWER_CLAMP_WINDOW_PCGS: u64 = 2 * 800;
const PCG_CHIPS: u64 = 1_536;
const RC3_STARTUP_SEED_PCGS: usize = 32;
const FER_WINDOW: usize = 50;
const OUTER_LOOP_MIN_VALID_FRAMES: u64 = 10;
const TARGET_FER_PCT: f32 = 1.0;
const TARGET_STEP_UP_DB: f32 = 0.5;
const ADAPTIVE_FLOOR_TRIGGER_BAND_DB: f32 = 0.25;
const ADAPTIVE_FLOOR_STEP_UP_DB: f32 = 0.1;
const ADAPTIVE_FLOOR_DECAY_FRAMES: u64 = 250;
const ADAPTIVE_FLOOR_DECAY_STEP_DB: f32 = 0.1;

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
    fn raw_power_clamp_window_uses_first_three_seconds_of_channel_age() {
        let start_pcg = 10_000;
        let start_chip = start_pcg * PCG_CHIPS;

        assert!(BtsPowerControlRegistry::raw_power_clamp_window_active(
            Some(start_chip),
            start_pcg
        ));
        assert!(BtsPowerControlRegistry::raw_power_clamp_window_active(
            Some(start_chip),
            start_pcg + RAW_POWER_CLAMP_WINDOW_PCGS - 1
        ));
        assert!(!BtsPowerControlRegistry::raw_power_clamp_window_active(
            Some(start_chip),
            start_pcg + RAW_POWER_CLAMP_WINDOW_PCGS
        ));
        assert!(!BtsPowerControlRegistry::raw_power_clamp_window_active(
            None, start_pcg
        ));
    }

    #[test]
    fn raw_power_clamp_forces_down_only_inside_start_window() {
        let mut state = rc3_state_without_startup_seed();

        let clamped = state.tick_single_pcg(10, 100, -20.0, Some(-10.0), true);
        assert_eq!(clamped.pcb, 1);
        assert!(clamped.raw_power_clamp_active);

        let unclamped = state.tick_single_pcg(10, 101, -20.0, Some(-10.0), false);
        assert_eq!(unclamped.pcb, 0);
        assert!(!unclamped.raw_power_clamp_active);
    }

    #[test]
    fn raw_power_filter_updates_after_clamp_window_closes() {
        let mut state = rc3_state_without_startup_seed();

        let first = state.tick_single_pcg(10, 100, -20.0, Some(-10.0), false);
        assert_eq!(first.filtered_raw_power_db, Some(-10.0));
        assert!(!first.raw_power_clamp_active);

        let second = state.tick_single_pcg(10, 101, -20.0, Some(-30.0), false);
        assert!(second.filtered_raw_power_db.is_some_and(|db| db < -10.0));
        assert!(!second.raw_power_clamp_active);
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
        }
    }

    fn fallback_pcb_for_abs_pcg(abs_pcg: u64) -> u8 {
        (abs_pcg as u8) & 1
    }

    fn tick_single_pcg(
        &mut self,
        walsh_code: u8,
        abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
        raw_power_clamp_window_active: bool,
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

        if raw_power_clamp_window_active
            && raw_power_db.is_some()
            && self
                .filtered_raw_power_db
                .is_some_and(|db| db > RAW_POWER_DOWN_CLAMP_THRESHOLD_DBFS)
        {
            let pcb = 1;
            self.last_pcbs[slot] = pcb;
            return BtsPowerControlTick {
                pcb,
                target_db: self.effective_target_db(),
                control_metric_db: self.filtered_metric_db.unwrap_or(metric_db),
                raw_power_db,
                filtered_raw_power_db: self.filtered_raw_power_db,
                raw_power_clamp_active: true,
            };
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
            let filtered_metric_db = match self.filtered_metric_db {
                Some(prev) => prev + PCG_METRIC_FILTER_ALPHA * (metric_db - prev),
                None => metric_db,
            };
            self.filtered_metric_db = Some(filtered_metric_db);
            if first_measurement && self.held_setpoint_db.is_none() && self.startup_seed.is_none() {
                self.target_db = metric_db.clamp(self.auto_min_db, self.auto_max_db);
            }
            (
                self.quantize_pcb(effective_target_db - filtered_metric_db),
                filtered_metric_db,
            )
        } else if let Some(held_metric) = self.filtered_metric_db {
            (
                self.quantize_pcb(effective_target_db - held_metric),
                held_metric,
            )
        } else {
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

    fn raw_power_clamp_window_active(start_chip: Option<u64>, measured_abs_pcg: u64) -> bool {
        let Some(start_chip) = start_chip else {
            return false;
        };
        let start_abs_pcg = start_chip / PCG_CHIPS;
        measured_abs_pcg.saturating_sub(start_abs_pcg) < RAW_POWER_CLAMP_WINDOW_PCGS
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
        let (use_rc3, raw_power_clamp_window_active) = {
            let slots = traffic_channels.lock();
            let slot = slots.iter().find(|slot| slot.walsh_code == walsh_code)?;
            (
                matches!(slot.channel, TrafficChannelWrapper::Rc3(_)),
                Self::raw_power_clamp_window_active(slot.start_chip, measured_abs_pcg),
            )
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
            state.tick_single_pcg(
                walsh_code,
                measured_abs_pcg,
                metric_db,
                raw_power_db,
                raw_power_clamp_window_active,
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
