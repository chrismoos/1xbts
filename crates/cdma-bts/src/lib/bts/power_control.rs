//! Reverse-link closed-loop power control.
//!
//! RC1, RC2, and RC3 use absolute-PCG measurements without filtering or
//! prediction. Rate-gated channels schedule only PCBs guaranteed to be seen by
//! the mobile and use balanced commands between direct control opportunities.

use std::{collections::HashMap, collections::VecDeque, sync::Arc};

use parking_lot::Mutex;

use super::handle::{TrafficChannelPool, TrafficChannelWrapper};
use crate::channels::ftch_rc3::Rc3PcgPcbSchedulerSnapshot;

// Keep the RC1 target fixed to prevent multi-mobile noise-floor escalation.
const RC1_INITIAL_TARGET_DB: f32 = 13.0;
const RC1_AUTO_MIN_DB: f32 = 13.0;
const RC1_AUTO_MAX_DB: f32 = 13.0;
const RC1_MANUAL_MIN_DB: f32 = 0.0;
const RC1_MANUAL_MAX_DB: f32 = 40.0;
const RC2_INITIAL_TARGET_DB: f32 = 10.0;
const RC2_AUTO_MIN_DB: f32 = 8.0;
const RC2_AUTO_MAX_DB: f32 = 12.0;
const RC2_MANUAL_MIN_DB: f32 = 0.0;
const RC2_MANUAL_MAX_DB: f32 = 40.0;
const RC3_INITIAL_TARGET_DB: f32 = 0.0;
const DIRECT_THREE_STEP_ERROR_DB: f32 = 3.0;
const DIRECT_FIVE_STEP_ERROR_DB: f32 = 5.0;
const RC3_AUTO_MIN_DB: f32 = -2.0;
const RC3_AUTO_MAX_DB: f32 = 2.0;
const RC3_MANUAL_MIN_DB: f32 = -40.0;
const RC3_MANUAL_MAX_DB: f32 = 40.0;
const FER_WINDOW: usize = 50;
const RC1_RC2_OUTER_LOOP_MIN_VALID_FRAMES: u64 = 10;
const RC3_OUTER_LOOP_MIN_VALID_FRAMES: u64 = 50;
const RC1_RC2_TARGET_FER_PCT: f32 = 1.0;
const RC3_TARGET_FER_PCT: f32 = 0.5;
const TARGET_STEP_UP_DB: f32 = 0.25;
const TARGET_UNDERPOWER_ERROR_STEP_UP_DB: f32 = 0.5;
const TARGET_OVERPOWER_ERROR_STEP_DOWN_DB: f32 = 0.25;
const TARGET_OVERPOWER_CLEAN_STEP_DOWN_DB: f32 = 0.1;
pub(super) const DIRECT_CONTROL_MIN_LEAD_PCGS: u64 = 9;
#[cfg(test)]
const RC3_DIRECT_CONTROL_LEAD_PCGS: u64 = DIRECT_CONTROL_MIN_LEAD_PCGS;
const DIRECT_HOLD_PCBS: u64 = 8;
const DIRECT_EPOCH_PCBS: u64 = DIRECT_HOLD_PCBS + 1;
const DIRECT_MAX_MEASUREMENT_AGE_PCGS: u64 = 2;
#[cfg(test)]
const RC3_DIRECT_HOLD_PCBS: u64 = DIRECT_HOLD_PCBS;
#[cfg(test)]
const RC3_DIRECT_EPOCH_PCBS: u64 = DIRECT_EPOCH_PCBS;
#[cfg(test)]
const RC3_DIRECT_MAX_MEASUREMENT_AGE_PCGS: u64 = DIRECT_MAX_MEASUREMENT_AGE_PCGS;
// The command goes on the third PCG the mobile is certain to transmit in, not a
// fixed number of PCGs later, because the randomizer moves which ones those are.
// That works out to 19-33 PCGs of lead, always past the nine-PCG publication floor.
const RC12_DIRECT_DELAY_GUARANTEED_SLOTS: u64 = 3;
const RC12_NEUTRAL_LOOKAHEAD_PCGS: u64 = 16;
const RC3_RELEASE_NEUTRAL_FILL_PCGS: u64 = 16;
const SR1_CHIPS_PER_PCG: u64 = 1_536;
const SR1_PCGS_PER_SECOND: u64 = 800;
const CLIP_BEGIN_DBFS: f32 = -8.0;
const PCG_RAW_HOT_LIMIT_DBFS: f32 = -8.0;
const RC1_CLIP_BEGIN_DBFS: f32 = -2.0;
const RC1_PCG_RAW_HOT_LIMIT_DBFS: f32 = -2.0;
const RC2_PCG_RAW_HOT_LIMIT_DBFS: f32 = -38.0;
const OUTER_LOOP_OVERPOWER_CLIP_PCGS: usize = 4;
const RC1_OUTER_LOOP_OVERPOWER_CLIP_PCGS: usize = 1;
const RC2_OUTER_LOOP_OVERPOWER_HOT_PCGS: usize = 1;
const OUTER_LOOP_UNDERPOWER_ERROR_DB: f32 = 4.0;
const OUTER_LOOP_UNDERPOWER_MAX_RAW_DBFS: f32 = -20.0;
const OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS: usize = 8;
const RC12_OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS: usize = 2;

#[derive(Debug, Clone, Copy)]
pub struct BtsPowerControlTick {
    pub pcb: u8,
    /// Whether this PCG carries the forward power-control subchannel.
    pub command_slot_valid: bool,
    pub target_db: f32,
    pub control_metric_db: f32,
    pub raw_power_db: Option<f32>,
    pub raw_power_clamp_active: bool,
    pub control_epoch: bool,
    pub measurement_used: bool,
    pub control_steps: u8,
    pub safety_down: bool,
    pub valid_ordinal: Option<u64>,
    pub measurement_abs_pcg: Option<u64>,
    pub schedule_accepted: bool,
    pub rc3_scheduler: Option<Rc3PcgPcbSchedulerSnapshot>,
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
    pub measured_inner_loop_1s: Option<BtsPowerControlOneSecondMeasurement>,
}

#[derive(Debug, Clone, Copy)]
pub struct BtsPowerControlOneSecondMeasurement {
    pub timestamp_ms: u64,
    pub mean_db: f32,
}

#[derive(Debug, Clone, Copy)]
struct OneSecondMetricWindow {
    bucket_index: u64,
    last_abs_pcg: u64,
    covers_complete_second: bool,
    sum_db: f64,
    count: u32,
}

#[derive(Debug, Clone, Copy)]
struct BtsReversePowerSetpoint {
    target_db: f32,
    held: bool,
}

#[derive(Debug, Clone, Copy)]
struct RawPowerLimiterProfile {
    clip_begin_dbfs: Option<f32>,
    hot_limit_dbfs: f32,
    thresholds_follow_rx_power_adj: bool,
    overpowered_pcgs: usize,
}

impl RawPowerLimiterProfile {
    const RC3: Self = Self {
        clip_begin_dbfs: Some(CLIP_BEGIN_DBFS),
        hot_limit_dbfs: PCG_RAW_HOT_LIMIT_DBFS,
        thresholds_follow_rx_power_adj: true,
        overpowered_pcgs: OUTER_LOOP_OVERPOWER_CLIP_PCGS,
    };

    const RC1: Self = Self {
        clip_begin_dbfs: Some(RC1_CLIP_BEGIN_DBFS),
        hot_limit_dbfs: RC1_PCG_RAW_HOT_LIMIT_DBFS,
        thresholds_follow_rx_power_adj: true,
        overpowered_pcgs: RC1_OUTER_LOOP_OVERPOWER_CLIP_PCGS,
    };

    const RC2: Self = Self {
        clip_begin_dbfs: None,
        hot_limit_dbfs: RC2_PCG_RAW_HOT_LIMIT_DBFS,
        thresholds_follow_rx_power_adj: false,
        overpowered_pcgs: RC2_OUTER_LOOP_OVERPOWER_HOT_PCGS,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReverseTrafficRadioConfig {
    Rc1,
    Rc2,
    Rc3,
}

impl ReverseTrafficRadioConfig {
    fn from_channel(channel: &TrafficChannelWrapper) -> Self {
        match channel {
            TrafficChannelWrapper::Rc1(_) => Self::Rc1,
            TrafficChannelWrapper::Rc2(_) => Self::Rc2,
            TrafficChannelWrapper::Rc3(_) | TrafficChannelWrapper::SchRc3(_) => Self::Rc3,
        }
    }

    fn limiter_power_db(
        self,
        raw_power_db: Option<f32>,
        mobile_power_db: Option<f32>,
    ) -> Option<f32> {
        match self {
            Self::Rc2 => mobile_power_db,
            Self::Rc1 | Self::Rc3 => raw_power_db,
        }
    }

    const fn control_metric_name(self) -> &'static str {
        match self {
            Self::Rc1 | Self::Rc2 => "eb_nt_db",
            Self::Rc3 => "pilot_sinr_db",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bts::handle::{
        ChannelRegistry, WalshAllocator, allocate_traffic_channel, allocate_traffic_channel_rc2,
    };
    use crate::channels::ftch_rc3::gated_power_control_slot_ordinal;
    use crate::phy::coding::long_code::LongCodeGenerator;
    use cdma_common::consts::RC3_GATED_REV_PWR_CNTL_DELAY;

    #[test]
    fn production_rc3_state_adapts_after_one_second_warmup() {
        let mut state = BtsReversePowerControlState::new_rc3();
        assert_eq!(state.effective_target_db(), RC3_INITIAL_TARGET_DB);
        assert_eq!(state.held_setpoint_db, None);
        assert_eq!(state.auto_min_db, -2.0);
        assert_eq!(state.auto_max_db, 2.0);
        assert_eq!(state.target_fer_pct(), 0.5);

        for pcg in 0..16 {
            let _ = state.tick_rc3_direct(
                pcg,
                RC3_INITIAL_TARGET_DB,
                Some(-30.0),
                Some(pcg + RC3_DIRECT_CONTROL_LEAD_PCGS),
            );
        }
        let snapshot = state.outer_loop_tick(10, false);
        assert_eq!(snapshot.effective_target_eb_nt_db, RC3_INITIAL_TARGET_DB);

        for _ in 0..49 {
            let snapshot = state.outer_loop_tick(10, true);
            assert_eq!(snapshot.effective_target_eb_nt_db, RC3_INITIAL_TARGET_DB);
        }
        let snapshot = state.outer_loop_tick(10, true);

        assert!(
            (snapshot.effective_target_eb_nt_db
                - (RC3_INITIAL_TARGET_DB - state.target_step_down_db()))
            .abs()
                < f32::EPSILON
        );
    }

    fn epoch_target_pcgs() -> Vec<u64> {
        let first = (RC3_DIRECT_CONTROL_LEAD_PCGS..256)
            .find(|abs_pcg| {
                gated_power_control_slot_ordinal(*abs_pcg, RC3_GATED_REV_PWR_CNTL_DELAY)
                    .is_some_and(|ordinal| ordinal % RC3_DIRECT_EPOCH_PCBS == 0)
            })
            .expect("direct epoch starts in the search range");
        (first..256)
            .filter(|abs_pcg| {
                gated_power_control_slot_ordinal(*abs_pcg, RC3_GATED_REV_PWR_CNTL_DELAY).is_some()
            })
            .take(RC3_DIRECT_EPOCH_PCBS as usize)
            .collect()
    }

    fn gated_ordinal(tx_abs_pcg: u64) -> Option<u64> {
        gated_power_control_slot_ordinal(tx_abs_pcg, RC3_GATED_REV_PWR_CNTL_DELAY)
    }

    #[test]
    fn rc3_epoch_emits_eight_balanced_holds_then_one_control() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let target_pcgs = epoch_target_pcgs();
        assert_eq!(target_pcgs.len(), RC3_DIRECT_EPOCH_PCBS as usize);

        let mut ticks = Vec::new();
        for &tx_abs_pcg in &target_pcgs {
            let measured_abs_pcg = tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS;
            assert!(measured_abs_pcg % 4 >= 2);
            ticks.push(state.tick_rc3_direct(
                measured_abs_pcg,
                RC3_INITIAL_TARGET_DB + 2.0,
                Some(-30.0),
                gated_ordinal(tx_abs_pcg),
            ));
        }

        let hold_bits: Vec<u8> = ticks[..RC3_DIRECT_HOLD_PCBS as usize]
            .iter()
            .map(|tick| tick.pcb)
            .collect();
        assert_eq!(
            hold_bits,
            (0..RC3_DIRECT_HOLD_PCBS)
                .map(|offset| offset as u8 & 1)
                .collect::<Vec<_>>()
        );
        assert!(
            ticks[..RC3_DIRECT_HOLD_PCBS as usize]
                .iter()
                .all(|tick| !tick.control_epoch && !tick.measurement_used)
        );

        let control = ticks.last().expect("control tick exists");
        assert!(control.control_epoch);
        assert!(control.measurement_used);
        assert_eq!(control.control_steps, 1);
        assert_eq!(control.pcb, 1);
        assert_eq!(control.target_db, RC3_INITIAL_TARGET_DB);
    }

    #[test]
    fn rc3_non_gated_epoch_schedules_every_pcg_with_eight_holds_then_control() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let first_ordinal = 900;
        let ticks: Vec<_> = (first_ordinal..first_ordinal + RC3_DIRECT_EPOCH_PCBS)
            .map(|tx_abs_pcg| {
                state.tick_rc3_direct(
                    tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS,
                    RC3_INITIAL_TARGET_DB + 2.0,
                    Some(-30.0),
                    Some(tx_abs_pcg),
                )
            })
            .collect();

        assert!(ticks.iter().all(|tick| tick.command_slot_valid));
        assert_eq!(
            ticks[..RC3_DIRECT_HOLD_PCBS as usize]
                .iter()
                .map(|tick| tick.pcb)
                .collect::<Vec<_>>(),
            vec![0, 1, 0, 1, 0, 1, 0, 1]
        );
        assert!(
            ticks[..RC3_DIRECT_HOLD_PCBS as usize]
                .iter()
                .all(|tick| !tick.control_epoch && !tick.measurement_used)
        );
        let control = ticks.last().expect("control tick exists");
        assert!(control.control_epoch);
        assert!(control.measurement_used);
        assert_eq!(control.pcb, 1);
    }

    #[test]
    fn rc3_snapshot_reports_only_a_complete_one_second_mean() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let metric_for = |abs_pcg: u64| {
            if abs_pcg % 4 >= 2 { 2.5 } else { f32::NAN }
        };

        for abs_pcg in 400..=800 {
            state.tick_rc3_direct(
                abs_pcg,
                metric_for(abs_pcg),
                Some(-30.0),
                gated_ordinal(abs_pcg + RC3_DIRECT_CONTROL_LEAD_PCGS),
            );
        }
        assert!(state.snapshot(10).measured_inner_loop_1s.is_none());

        for abs_pcg in 801..=1_600 {
            state.tick_rc3_direct(
                abs_pcg,
                metric_for(abs_pcg),
                Some(-30.0),
                gated_ordinal(abs_pcg + RC3_DIRECT_CONTROL_LEAD_PCGS),
            );
            if abs_pcg == 1_000 {
                state.tick_rc3_direct(
                    abs_pcg,
                    100.0,
                    Some(-30.0),
                    gated_ordinal(abs_pcg + RC3_DIRECT_CONTROL_LEAD_PCGS),
                );
            }
        }
        let measurement = state
            .snapshot(10)
            .measured_inner_loop_1s
            .expect("complete one-second measurement exists");
        assert_eq!(measurement.mean_db, 2.5);
        assert!(measurement.timestamp_ms > 0);
    }

    #[test]
    fn rc3_control_uses_pilot_sinr_and_missing_measurement_holds() {
        let target_pcgs = epoch_target_pcgs();
        let tx_abs_pcg = *target_pcgs.last().expect("control PCG exists");
        let measured_abs_pcg = tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS;

        let mut up_state = BtsReversePowerControlState::new_rc3();
        let up = up_state.tick_rc3_direct(
            measured_abs_pcg,
            RC3_INITIAL_TARGET_DB - 2.0,
            Some(-30.0),
            gated_ordinal(tx_abs_pcg),
        );
        assert!(up.measurement_used);
        assert_eq!(up.control_steps, 1);
        assert_eq!(up.pcb, 0);
        assert_eq!(up.target_db, RC3_INITIAL_TARGET_DB);

        let mut down_state = BtsReversePowerControlState::new_rc3();
        let down = down_state.tick_rc3_direct(
            measured_abs_pcg,
            RC3_INITIAL_TARGET_DB + 2.0,
            Some(-30.0),
            gated_ordinal(tx_abs_pcg),
        );
        assert!(down.measurement_used);
        assert_eq!(down.control_steps, 1);
        assert_eq!(down.pcb, 1);

        let mut missing_state = BtsReversePowerControlState::new_rc3();
        let missing = missing_state.tick_rc3_direct(
            measured_abs_pcg,
            f32::NAN,
            Some(-30.0),
            gated_ordinal(tx_abs_pcg),
        );
        assert!(missing.control_epoch);
        assert!(!missing.measurement_used);
        assert_eq!(missing.control_steps, 0);
        assert_eq!(
            missing.pcb,
            (gated_power_control_slot_ordinal(tx_abs_pcg, RC3_GATED_REV_PWR_CNTL_DELAY)
                .expect("missing control has a valid ordinal")
                % RC3_DIRECT_EPOCH_PCBS) as u8
                & 1
        );
    }

    #[test]
    fn rc1_control_uses_fixed_despread_power_target() {
        let mut state = BtsReversePowerControlState::new_rc1();
        let tick = state.tick_direct(100, RC1_INITIAL_TARGET_DB - 1.0, Some(-20.0), Some(8));

        assert!(tick.control_epoch);
        assert!(tick.measurement_used);
        assert_eq!(tick.target_db, RC1_INITIAL_TARGET_DB);
        assert_eq!(tick.pcb, 0);

        let snapshot = state.outer_loop_tick(10, false);
        assert_eq!(snapshot.effective_target_eb_nt_db, RC1_INITIAL_TARGET_DB);
    }

    #[test]
    fn rc1_control_keeps_large_errors_to_one_step() {
        let mut state = BtsReversePowerControlState::new_rc1();
        let tick = state.tick_direct(100, RC1_INITIAL_TARGET_DB - 10.0, Some(-20.0), Some(8));

        assert!(tick.measurement_used);
        assert_eq!(tick.control_steps, 1);
        assert_eq!(tick.pcb, 0);
    }

    #[test]
    fn rc2_control_uses_eb_nt_target() {
        let mut state = BtsReversePowerControlState::new_rc2();
        let tick = state.tick_direct(100, RC2_INITIAL_TARGET_DB - 1.0, Some(-50.0), Some(8));

        assert!(tick.control_epoch);
        assert!(tick.measurement_used);
        assert_eq!(tick.target_db, RC2_INITIAL_TARGET_DB);
        assert_eq!(tick.pcb, 0);
    }

    #[test]
    fn rc1_and_rc2_schedule_each_guaranteed_measurement_once() {
        let long_code = LongCodeGenerator::new_traffic_channel(0xDEAD_BEEF);

        let rc1_channels = Arc::new(ChannelRegistry::new());
        let rc1_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
        let (rc1_walsh, rc1_channel) =
            allocate_traffic_channel(&rc1_allocator, &rc1_channels, long_code.clone(), 0, 0)
                .expect("RC1 traffic channel");
        let rc1_measured_abs_pcg = (0..16)
            .find(|pcg| {
                rc1_channel
                    .channel
                    .guaranteed_power_control_ordinal(*pcg)
                    .is_some()
            })
            .expect("RC1 guaranteed measurement");
        let rc1_tick = BtsPowerControlRegistry::default()
            .tick_and_schedule(
                &rc1_channels,
                rc1_walsh,
                rc1_measured_abs_pcg,
                rc1_measured_abs_pcg + DIRECT_CONTROL_MIN_LEAD_PCGS,
                RC1_INITIAL_TARGET_DB,
                Some(-20.0),
                Some(-58.0),
                None,
            )
            .expect("RC1 direct tick");
        assert!(rc1_tick.schedule_accepted);
        assert_eq!(rc1_tick.control_metric_db, RC1_INITIAL_TARGET_DB);
        assert_eq!(rc1_tick.target_db, RC1_INITIAL_TARGET_DB);
        assert_eq!(
            rc1_tick.valid_ordinal,
            rc1_channel
                .channel
                .guaranteed_power_control_ordinal(rc1_measured_abs_pcg)
                .map(|ordinal| ordinal + RC12_DIRECT_DELAY_GUARANTEED_SLOTS)
        );

        let rc2_channels = Arc::new(ChannelRegistry::new());
        let rc2_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
        let (rc2_walsh, rc2_channel) =
            allocate_traffic_channel_rc2(&rc2_allocator, &rc2_channels, long_code, 0, 0)
                .expect("RC2 traffic channel");
        let rc2_measured_abs_pcg = (0..16)
            .find(|pcg| {
                rc2_channel
                    .channel
                    .guaranteed_power_control_ordinal(*pcg)
                    .is_some()
            })
            .expect("RC2 guaranteed measurement");
        let rc2_tick = BtsPowerControlRegistry::default()
            .tick_and_schedule(
                &rc2_channels,
                rc2_walsh,
                rc2_measured_abs_pcg,
                rc2_measured_abs_pcg + DIRECT_CONTROL_MIN_LEAD_PCGS,
                RC2_INITIAL_TARGET_DB,
                None,
                Some(-50.0),
                None,
            )
            .expect("RC2 direct tick");
        assert!(rc2_tick.schedule_accepted);
        assert_eq!(
            rc2_tick.valid_ordinal,
            rc2_channel
                .channel
                .guaranteed_power_control_ordinal(rc2_measured_abs_pcg)
                .map(|ordinal| ordinal + RC12_DIRECT_DELAY_GUARANTEED_SLOTS)
        );
    }

    #[test]
    fn rc2_power_ceiling_forces_down_without_filtering() {
        let mut state = BtsReversePowerControlState::new_rc2();
        let tick = state.tick_direct(
            100,
            RC2_INITIAL_TARGET_DB - 10.0,
            Some(RC2_PCG_RAW_HOT_LIMIT_DBFS + 0.1),
            Some(0),
        );

        assert!(tick.safety_down);
        assert_eq!(tick.pcb, 1);
    }

    #[test]
    fn rc3_release_hold_freezes_outer_loop_and_alternates_neutral_pcbs() {
        let mut state = BtsReversePowerControlState::new_rc3();
        for pcg in 0..16 {
            let _ = state.tick_rc3_direct(
                pcg,
                RC3_INITIAL_TARGET_DB,
                Some(-30.0),
                Some(pcg + RC3_DIRECT_CONTROL_LEAD_PCGS),
            );
        }
        let before = state.outer_loop_tick(10, true);
        state.enter_release_hold();

        let ticks = (0..8)
            .map(|ordinal| state.tick_rc3_release_hold(100 + ordinal, Some(ordinal)))
            .collect::<Vec<_>>();
        assert_eq!(
            ticks.iter().map(|tick| tick.pcb).collect::<Vec<_>>(),
            vec![0, 1, 0, 1, 0, 1, 0, 1]
        );
        assert!(ticks.iter().all(|tick| {
            tick.command_slot_valid
                && !tick.control_epoch
                && !tick.measurement_used
                && tick.control_steps == 0
                && !tick.raw_power_clamp_active
        }));

        let after = state.outer_loop_tick(10, false);
        assert_eq!(after.frames_total, before.frames_total);
        assert_eq!(after.frames_crc_error, before.frames_crc_error);
        assert_eq!(after.fer_pct, before.fer_pct);
        assert_eq!(
            after.effective_target_eb_nt_db,
            before.effective_target_eb_nt_db
        );
    }

    #[test]
    fn rc3_control_uses_recent_valid_pilot_when_current_pcg_is_gated_off() {
        let tx_abs_pcg = 908;
        let measured_abs_pcg = tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS;
        let mut state = BtsReversePowerControlState::new_rc3();

        let _ = state.tick_rc3_direct(
            measured_abs_pcg - 2,
            RC3_INITIAL_TARGET_DB + 2.0,
            Some(-30.0),
            Some(tx_abs_pcg - 2),
        );
        let control =
            state.tick_rc3_direct(measured_abs_pcg, f32::NAN, Some(-30.0), Some(tx_abs_pcg));

        assert!(control.control_epoch);
        assert!(control.measurement_used);
        assert_eq!(control.measurement_abs_pcg, Some(measured_abs_pcg - 2));
        assert_eq!(control.control_metric_db, RC3_INITIAL_TARGET_DB + 2.0);
        assert_eq!(control.pcb, 1);
    }

    #[test]
    fn rc3_control_rejects_stale_cached_pilot() {
        let tx_abs_pcg = 908;
        let measured_abs_pcg = tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS;
        let mut state = BtsReversePowerControlState::new_rc3();

        let _ = state.tick_rc3_direct(
            measured_abs_pcg - RC3_DIRECT_MAX_MEASUREMENT_AGE_PCGS - 1,
            RC3_INITIAL_TARGET_DB + 2.0,
            Some(-30.0),
            Some(tx_abs_pcg - RC3_DIRECT_MAX_MEASUREMENT_AGE_PCGS - 1),
        );
        let control =
            state.tick_rc3_direct(measured_abs_pcg, f32::NAN, Some(-30.0), Some(tx_abs_pcg));

        assert!(control.control_epoch);
        assert!(!control.measurement_used);
        assert_eq!(control.measurement_abs_pcg, None);
    }

    #[test]
    fn rc3_large_error_encodes_five_net_steps_in_the_next_epoch() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let first_epoch = epoch_target_pcgs();
        let control_tx_abs_pcg = *first_epoch.last().expect("control PCG exists");
        let mut control = None;
        for &tx_abs_pcg in &first_epoch {
            control = Some(state.tick_rc3_direct(
                tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS,
                RC3_INITIAL_TARGET_DB + 10.0,
                Some(-30.0),
                gated_ordinal(tx_abs_pcg),
            ));
        }
        let control = control.expect("control tick exists");
        assert!(control.control_epoch);
        assert_eq!(control.control_steps, 5);

        let following_valid_pcgs = ((control_tx_abs_pcg + 1)..(control_tx_abs_pcg + 64))
            .filter(|abs_pcg| {
                gated_power_control_slot_ordinal(*abs_pcg, RC3_GATED_REV_PWR_CNTL_DELAY).is_some()
            })
            .take(RC3_DIRECT_HOLD_PCBS as usize);
        let mut interval_bits = vec![control.pcb];
        for tx_abs_pcg in following_valid_pcgs {
            interval_bits.push(
                state
                    .tick_rc3_direct(
                        tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS,
                        RC3_INITIAL_TARGET_DB + 10.0,
                        Some(-30.0),
                        gated_ordinal(tx_abs_pcg),
                    )
                    .pcb,
            );
        }
        assert_eq!(interval_bits, vec![1, 1, 1, 1, 1, 0, 1, 0, 1]);
        assert_eq!(
            interval_bits
                .iter()
                .map(|bit| if *bit == 0 { 1 } else { -1 })
                .sum::<i32>(),
            -5,
        );
    }

    #[test]
    fn rc3_recovery_strength_tracks_instantaneous_error() {
        let tx_abs_pcg = *epoch_target_pcgs().last().expect("control PCG exists");
        let measured_abs_pcg = tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS;
        for (offset_db, expected_steps, expected_pcb) in
            [(-2.0, 1, 0), (-4.0, 3, 0), (-6.0, 5, 0), (6.0, 5, 1)]
        {
            let mut state = BtsReversePowerControlState::new_rc3();
            let tick = state.tick_rc3_direct(
                measured_abs_pcg,
                RC3_INITIAL_TARGET_DB + offset_db,
                Some(-30.0),
                gated_ordinal(tx_abs_pcg),
            );
            assert!(tick.measurement_used);
            assert_eq!(tick.control_steps, expected_steps);
            assert_eq!(tick.pcb, expected_pcb);
        }
    }

    #[test]
    fn rc3_recovery_burst_stays_on_absolute_pcb_ordinals() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let control_tx_abs_pcg = *epoch_target_pcgs().last().expect("control PCG exists");
        let control = state.tick_rc3_direct(
            control_tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS,
            RC3_INITIAL_TARGET_DB + 10.0,
            Some(-30.0),
            gated_ordinal(control_tx_abs_pcg),
        );
        assert_eq!(control.control_steps, 5);

        let following_valid_pcgs: Vec<u64> = ((control_tx_abs_pcg + 1)..(control_tx_abs_pcg + 64))
            .filter(|abs_pcg| {
                gated_power_control_slot_ordinal(*abs_pcg, RC3_GATED_REV_PWR_CNTL_DELAY).is_some()
            })
            .take(6)
            .collect();
        let fourth_command = following_valid_pcgs[2];
        let after_burst = following_valid_pcgs[4];

        let recovered_after_gap = state.tick_rc3_direct(
            fourth_command - RC3_DIRECT_CONTROL_LEAD_PCGS,
            RC3_INITIAL_TARGET_DB + 10.0,
            Some(-30.0),
            gated_ordinal(fourth_command),
        );
        assert_eq!(recovered_after_gap.pcb, 1);

        let hold_after_gap = state.tick_rc3_direct(
            after_burst - RC3_DIRECT_CONTROL_LEAD_PCGS,
            RC3_INITIAL_TARGET_DB + 10.0,
            Some(-30.0),
            gated_ordinal(after_burst),
        );
        assert_eq!(hold_after_gap.pcb, 0);
    }

    #[test]
    fn rc3_wideband_clip_safety_overrides_hold() {
        let tx_abs_pcg = epoch_target_pcgs()[0];
        let measured_abs_pcg = tx_abs_pcg - RC3_DIRECT_CONTROL_LEAD_PCGS;
        let mut state = BtsReversePowerControlState::new_rc3();
        let tick = state.tick_rc3_direct(
            measured_abs_pcg,
            RC3_INITIAL_TARGET_DB - 10.0,
            Some(CLIP_BEGIN_DBFS + 0.1),
            gated_ordinal(tx_abs_pcg),
        );

        assert!(!tick.control_epoch);
        assert!(tick.safety_down);
        assert!(!tick.measurement_used);
        assert_eq!(tick.pcb, 1);
    }

    #[test]
    fn control_pcg_admission_rejects_duplicate_and_replayed_history() {
        let mut state = BtsReversePowerControlState::new_rc3();

        assert!(state.admit_control_abs_pcg(10_000));
        assert!(!state.admit_control_abs_pcg(10_000));
        assert!(!state.admit_control_abs_pcg(9_999));
        assert!(state.admit_control_abs_pcg(10_001));
    }

    #[test]
    fn mobile_power_ceiling_forces_down_at_startup() {
        let mut state = BtsReversePowerControlState::new_rc3();
        let tick = state.tick_rc3_direct(100, -30.0, Some(CLIP_BEGIN_DBFS + 0.1), Some(109));
        assert_eq!(tick.pcb, 1);
        assert!(tick.raw_power_clamp_active);
    }

    #[test]
    fn outer_loop_ignores_frames_before_pcg_observations_for_lifetime_fer() {
        let mut state = BtsReversePowerControlState::new_rc3();

        for _ in 0..10 {
            let snap = state.outer_loop_tick(10, false);
            assert_eq!(snap.frames_total, 0);
            assert_eq!(snap.frames_crc_error, 0);
            assert_eq!(snap.fer_pct, 0.0);
        }

        for pcg in 0..16 {
            let _ = state.tick_rc3_direct(pcg, -6.0, Some(-24.0), Some(pcg + 9));
        }
        let snap = state.outer_loop_tick(10, false);
        assert_eq!(snap.frames_total, 1);
        assert_eq!(snap.frames_crc_error, 1);
    }

    #[test]
    fn only_rc2_uses_despread_mobile_power_for_limiting() {
        let raw = Some(-5.0);
        let mobile = Some(-40.0);
        assert_eq!(
            ReverseTrafficRadioConfig::Rc1.limiter_power_db(raw, mobile),
            raw
        );
        assert_eq!(
            ReverseTrafficRadioConfig::Rc2.limiter_power_db(raw, mobile),
            mobile
        );
        assert_eq!(
            ReverseTrafficRadioConfig::Rc3.limiter_power_db(raw, mobile),
            raw
        );
    }

    #[test]
    fn rc2_observations_expire_at_the_next_frame() {
        let mut state = BtsReversePowerControlState::new_rc2();
        let _ = state.tick_direct(14, 10.0, Some(RC2_PCG_RAW_HOT_LIMIT_DBFS + 0.1), Some(0));
        let _ = state.tick_direct(15, 10.0, Some(-45.0), Some(1));
        assert!(state.recent_frame_overpowered());
        let _ = state.outer_loop_tick(10, true);

        let _ = state.tick_direct(16, 10.0, Some(-45.0), Some(2));
        let _ = state.tick_direct(17, 10.0, Some(-45.0), Some(3));
        state.age_rc12_observations();

        assert!(!state.last_pcg_raw_power_db[14].is_finite());
        assert!(!state.last_pcg_raw_power_db[15].is_finite());
        assert_eq!(state.recent_overpowered_pcgs(), 0);
        assert!(state.has_frame_observations());
    }

    #[test]
    fn rc1_outer_loop_accepts_two_guaranteed_pcg_observations() {
        let mut state = BtsReversePowerControlState::new_rc1();
        let _ = state.tick_direct(2, 10.0, Some(-20.0), Some(0));
        let _ = state.tick_direct(10, 10.0, Some(-20.0), Some(1));

        let snapshot = state.outer_loop_tick(10, true);

        assert_eq!(snapshot.frames_total, 1);
        assert_eq!(snapshot.frames_crc_error, 0);
    }

    #[test]
    fn rc1_outer_loop_detects_one_clipped_guaranteed_pcg() {
        let mut state = BtsReversePowerControlState::new_rc1();
        let _ = state.tick_direct(2, 10.0, Some(RC1_PCG_RAW_HOT_LIMIT_DBFS + 0.1), Some(0));
        let _ = state.tick_direct(10, 10.0, Some(-20.0), Some(1));

        assert!(state.recent_frame_overpowered());
    }

    #[test]
    fn changing_radio_config_reinitializes_all_power_control_state() {
        let mut states = HashMap::new();
        {
            let (state, reset) = BtsPowerControlRegistry::state_for_radio_config(
                &mut states,
                12,
                ReverseTrafficRadioConfig::Rc3,
                0.0,
            );
            assert!(reset);
            state.target_db = RC3_AUTO_MAX_DB;
            state.total_frames = 123;
            state.last_valid_metric_db = Some(7.0);
        }

        let (state, reset) = BtsPowerControlRegistry::state_for_radio_config(
            &mut states,
            12,
            ReverseTrafficRadioConfig::Rc2,
            0.0,
        );
        assert!(reset);
        assert_eq!(state.radio_config, ReverseTrafficRadioConfig::Rc2);
        assert_eq!(state.target_db, RC2_INITIAL_TARGET_DB);
        assert_eq!(state.auto_min_db, RC2_AUTO_MIN_DB);
        assert_eq!(state.auto_max_db, RC2_AUTO_MAX_DB);
        assert_eq!(state.total_frames, 0);
        assert!(state.last_valid_metric_db.is_none());
    }

    #[test]
    fn removing_walsh_state_prevents_same_rc_history_reuse() {
        let registry = BtsPowerControlRegistry::default();
        registry
            .states
            .lock()
            .insert(12, BtsReversePowerControlState::new_rc2());
        assert!(registry.snapshot(12).is_some());

        registry.remove(12);

        assert!(registry.snapshot(12).is_none());
    }
}

#[derive(Debug, Clone)]
struct BtsReversePowerControlState {
    radio_config: ReverseTrafficRadioConfig,
    target_db: f32,
    auto_min_db: f32,
    auto_max_db: f32,
    manual_min_db: f32,
    manual_max_db: f32,
    held_setpoint_db: Option<f32>,
    /// A command worth more than one step is sent as the same bit repeated over
    /// consecutive transmitted PCGs, since each carries only one step. The bit,
    /// where the repeat started, and how many PCGs it runs for.
    burst_bit: u8,
    burst_start_ordinal: Option<u64>,
    burst_steps: u8,
    /// Most recent finite control metric and the PCG it came from. A decision
    /// may use it while it is no older than `DIRECT_MAX_MEASUREMENT_AGE_PCGS`.
    last_valid_metric_db: Option<f32>,
    last_valid_metric_abs_pcg: Option<u64>,
    release_hold: bool,
    last_pcg_pilot_db: [f32; 16],
    last_pcg_control_metric_db: [f32; 16],
    last_pcg_raw_power_db: [f32; 16],
    last_pcg_overpowered: [bool; 16],
    last_pcg_raw_clamp_active: [bool; 16],
    last_pcg_command_slot_valid: [bool; 16],
    last_pcg_abs_pcg: [u64; 16],
    /// Last absolute PCG admitted to the controller. Replayed measurements must
    /// not advance it twice.
    last_control_abs_pcg: Option<u64>,
    last_pcg_observation_epoch: [Option<u64>; 16],
    observation_epoch: u64,
    last_pcbs: [u8; 16],
    crc_window: VecDeque<bool>,
    total_frames: u64,
    target_adaptation_valid_frames: u64,
    total_crc_errors: u64,
    last_fer_pct: f32,
    rx_power_adj_dbfs: f32,
    raw_power_profile: RawPowerLimiterProfile,
    one_second_metric_window: Option<OneSecondMetricWindow>,
    measured_inner_loop_1s: Option<BtsPowerControlOneSecondMeasurement>,
}

impl BtsReversePowerControlState {
    fn admit_control_abs_pcg(&mut self, abs_pcg: u64) -> bool {
        if self
            .last_control_abs_pcg
            .is_some_and(|last| abs_pcg <= last)
        {
            return false;
        }
        self.last_control_abs_pcg = Some(abs_pcg);
        true
    }

    fn new_rc1() -> Self {
        Self::with_params_and_profile(
            ReverseTrafficRadioConfig::Rc1,
            RC1_INITIAL_TARGET_DB,
            RC1_AUTO_MIN_DB,
            RC1_AUTO_MAX_DB,
            RC1_MANUAL_MIN_DB,
            RC1_MANUAL_MAX_DB,
            RawPowerLimiterProfile::RC1,
        )
    }

    fn new_rc2() -> Self {
        Self::with_params_and_profile(
            ReverseTrafficRadioConfig::Rc2,
            RC2_INITIAL_TARGET_DB,
            RC2_AUTO_MIN_DB,
            RC2_AUTO_MAX_DB,
            RC2_MANUAL_MIN_DB,
            RC2_MANUAL_MAX_DB,
            RawPowerLimiterProfile::RC2,
        )
    }

    fn new_rc3() -> Self {
        Self::with_params_and_profile(
            ReverseTrafficRadioConfig::Rc3,
            RC3_INITIAL_TARGET_DB,
            RC3_AUTO_MIN_DB,
            RC3_AUTO_MAX_DB,
            RC3_MANUAL_MIN_DB,
            RC3_MANUAL_MAX_DB,
            RawPowerLimiterProfile::RC3,
        )
    }

    fn with_params_and_profile(
        radio_config: ReverseTrafficRadioConfig,
        target_db: f32,
        auto_min_db: f32,
        auto_max_db: f32,
        manual_min_db: f32,
        manual_max_db: f32,
        raw_power_profile: RawPowerLimiterProfile,
    ) -> Self {
        Self {
            radio_config,
            target_db,
            auto_min_db,
            auto_max_db,
            manual_min_db,
            manual_max_db,
            held_setpoint_db: None,
            burst_bit: 0,
            burst_start_ordinal: None,
            burst_steps: 0,
            last_valid_metric_db: None,
            last_valid_metric_abs_pcg: None,
            release_hold: false,
            last_pcg_pilot_db: [f32::NAN; 16],
            last_pcg_control_metric_db: [f32::NAN; 16],
            last_pcg_raw_power_db: [f32::NAN; 16],
            last_pcg_overpowered: [false; 16],
            last_pcg_raw_clamp_active: [false; 16],
            last_pcg_command_slot_valid: [false; 16],
            last_pcg_abs_pcg: [0; 16],
            last_control_abs_pcg: None,
            last_pcg_observation_epoch: [None; 16],
            observation_epoch: 0,
            last_pcbs: [0; 16],
            crc_window: VecDeque::with_capacity(FER_WINDOW),
            total_frames: 0,
            target_adaptation_valid_frames: 0,
            total_crc_errors: 0,
            last_fer_pct: 0.0,
            rx_power_adj_dbfs: 0.0,
            raw_power_profile,
            one_second_metric_window: None,
            measured_inner_loop_1s: None,
        }
    }

    fn set_rx_power_adj_dbfs(&mut self, rx_power_adj_dbfs: f32) {
        self.rx_power_adj_dbfs = rx_power_adj_dbfs;
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
        self.last_pcg_pilot_db = [f32::NAN; 16];
        self.last_pcg_control_metric_db = [f32::NAN; 16];
        self.last_pcg_raw_power_db = [f32::NAN; 16];
        self.last_pcg_overpowered = [false; 16];
        self.last_pcg_raw_clamp_active = [false; 16];
        self.last_pcg_command_slot_valid = [false; 16];
        self.last_pcg_abs_pcg = [0; 16];
        self.last_control_abs_pcg = None;
        self.last_valid_metric_db = None;
        self.last_valid_metric_abs_pcg = None;
        self.last_pcg_observation_epoch = [None; 16];
        self.observation_epoch = 0;
        self.last_pcbs = [0; 16];
    }

    fn effective_target_db(&self) -> f32 {
        self.held_setpoint_db.unwrap_or(self.target_db)
    }

    fn enter_release_hold(&mut self) {
        self.release_hold = true;
        self.burst_start_ordinal = None;
        self.burst_steps = 0;
        self.last_valid_metric_db = None;
        self.last_valid_metric_abs_pcg = None;
    }

    fn record_one_second_metric(&mut self, abs_pcg: u64, metric_db: f32) {
        let bucket_index = abs_pcg / SR1_PCGS_PER_SECOND;
        if self
            .one_second_metric_window
            .is_some_and(|window| window.bucket_index != bucket_index)
        {
            let completed = self
                .one_second_metric_window
                .take()
                .expect("one-second metric window exists");
            let bucket_end = completed
                .bucket_index
                .saturating_add(1)
                .saturating_mul(SR1_PCGS_PER_SECOND)
                .saturating_sub(1);
            if completed.covers_complete_second
                && completed.last_abs_pcg == bucket_end
                && completed.count > 0
            {
                let timestamp_ms = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                self.measured_inner_loop_1s = Some(BtsPowerControlOneSecondMeasurement {
                    timestamp_ms,
                    mean_db: (completed.sum_db / f64::from(completed.count)) as f32,
                });
            }
        }

        let bucket_start = bucket_index.saturating_mul(SR1_PCGS_PER_SECOND);
        let mut record_sample = false;
        let window = self.one_second_metric_window.get_or_insert_with(|| {
            record_sample = true;
            OneSecondMetricWindow {
                bucket_index,
                last_abs_pcg: abs_pcg,
                covers_complete_second: abs_pcg == bucket_start,
                sum_db: 0.0,
                count: 0,
            }
        });
        if window.last_abs_pcg != abs_pcg {
            window.covers_complete_second &= window.last_abs_pcg.saturating_add(1) == abs_pcg;
            window.last_abs_pcg = abs_pcg;
            record_sample = true;
        }
        if record_sample && metric_db.is_finite() {
            window.sum_db += f64::from(metric_db);
            window.count = window.count.saturating_add(1);
        }
    }

    fn outer_loop_min_valid_frames(&self) -> u64 {
        if self.radio_config == ReverseTrafficRadioConfig::Rc3 {
            RC3_OUTER_LOOP_MIN_VALID_FRAMES
        } else {
            RC1_RC2_OUTER_LOOP_MIN_VALID_FRAMES
        }
    }

    fn target_fer_pct(&self) -> f32 {
        if self.radio_config == ReverseTrafficRadioConfig::Rc3 {
            RC3_TARGET_FER_PCT
        } else {
            RC1_RC2_TARGET_FER_PCT
        }
    }

    fn target_step_down_db(&self) -> f32 {
        let fer_frac = self.target_fer_pct() / 100.0;
        TARGET_STEP_UP_DB * fer_frac / (1.0 - fer_frac)
    }

    fn outer_loop_tick(&mut self, walsh_code: u8, crc_valid: bool) -> BtsPowerControlSnapshot {
        if self.release_hold {
            return self.snapshot(walsh_code);
        }
        self.age_rc12_observations();
        let has_frame_observations = self.has_frame_observations();
        let frame_overpowered = has_frame_observations && self.recent_frame_overpowered();
        let frame_underpowered =
            has_frame_observations && !frame_overpowered && self.recent_frame_underpowered();

        if !has_frame_observations {
            if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
                log::info!(
                    "power_frame[w{}]: crc={} counted=no_pcg_observation lifetime_frames={} fer={:.2}% target={:.2}",
                    walsh_code,
                    crc_valid as u8,
                    self.total_frames,
                    self.last_fer_pct,
                    self.effective_target_db(),
                );
            }
            return self.finish_outer_loop(walsh_code);
        }

        if self.radio_config == ReverseTrafficRadioConfig::Rc3
            && cdma_common::diagnostics::power_control_verbose_per_pcg_enabled_for_walsh(walsh_code)
        {
            self.log_batched_pcg_trace(walsh_code);
        }

        if !crc_valid {
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

        if crc_valid {
            self.target_adaptation_valid_frames =
                self.target_adaptation_valid_frames.saturating_add(1);
        }
        let target_adaptation_ready =
            self.target_adaptation_valid_frames >= self.outer_loop_min_valid_frames();

        if self.held_setpoint_db.is_none()
            && (self.radio_config != ReverseTrafficRadioConfig::Rc3 || target_adaptation_ready)
        {
            if crc_valid {
                if target_adaptation_ready {
                    let step_down_db = if frame_overpowered {
                        TARGET_OVERPOWER_CLEAN_STEP_DOWN_DB
                    } else {
                        self.target_step_down_db()
                    };
                    self.target_db = (self.target_db - step_down_db).max(self.auto_min_db);
                }
            } else {
                if frame_overpowered {
                    self.target_db = (self.target_db - TARGET_OVERPOWER_ERROR_STEP_DOWN_DB)
                        .max(self.auto_min_db);
                } else if frame_underpowered {
                    self.target_db = (self.target_db + TARGET_UNDERPOWER_ERROR_STEP_UP_DB)
                        .clamp(self.auto_min_db, self.auto_max_db);
                } else if target_adaptation_ready {
                    self.target_db = (self.target_db + TARGET_STEP_UP_DB)
                        .clamp(self.auto_min_db, self.auto_max_db);
                }
            }
        }

        if cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code) {
            self.log_frame_correlation(walsh_code, crc_valid);
        }

        self.finish_outer_loop(walsh_code)
    }

    fn finish_outer_loop(&mut self, walsh_code: u8) -> BtsPowerControlSnapshot {
        let snapshot = self.snapshot(walsh_code);
        if self.radio_config != ReverseTrafficRadioConfig::Rc3 {
            self.observation_epoch = self.observation_epoch.saturating_add(1);
        }
        snapshot
    }

    fn age_rc12_observations(&mut self) {
        if self.radio_config == ReverseTrafficRadioConfig::Rc3 {
            return;
        }
        for slot in 0..self.last_pcg_observation_epoch.len() {
            if self.last_pcg_observation_epoch[slot] == Some(self.observation_epoch) {
                continue;
            }
            self.last_pcg_pilot_db[slot] = f32::NAN;
            self.last_pcg_control_metric_db[slot] = f32::NAN;
            self.last_pcg_raw_power_db[slot] = f32::NAN;
            self.last_pcg_overpowered[slot] = false;
            self.last_pcg_raw_clamp_active[slot] = false;
            self.last_pcg_command_slot_valid[slot] = false;
            self.last_pcg_abs_pcg[slot] = 0;
            self.last_pcg_observation_epoch[slot] = None;
        }
    }

    fn recent_overpowered_pcgs(&self) -> usize {
        self.last_pcg_overpowered
            .iter()
            .filter(|overpowered| **overpowered)
            .count()
    }

    fn has_frame_observations(&self) -> bool {
        let raw_count = Self::finite_stats(&self.last_pcg_raw_power_db)
            .map(|(_, _, _, count)| count)
            .unwrap_or(0);
        let control_count = Self::finite_stats(&self.last_pcg_control_metric_db)
            .map(|(_, _, _, count)| count)
            .unwrap_or(0);
        let minimum = if self.radio_config != ReverseTrafficRadioConfig::Rc3 {
            RC12_OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS
        } else {
            OUTER_LOOP_MIN_FRAME_OBSERVATION_PCGS
        };
        raw_count >= minimum && control_count >= minimum
    }

    fn recent_frame_overpowered(&self) -> bool {
        self.recent_overpowered_pcgs() >= self.raw_power_profile.overpowered_pcgs
    }

    fn adjusted_dbfs(&self, threshold_dbfs: f32) -> f32 {
        if self.raw_power_profile.thresholds_follow_rx_power_adj {
            threshold_dbfs + self.rx_power_adj_dbfs
        } else {
            threshold_dbfs
        }
    }

    fn recent_frame_underpowered(&self) -> bool {
        if Self::finite_stats(&self.last_pcg_raw_power_db)
            .map(|(avg, _, _, _)| avg >= self.adjusted_dbfs(OUTER_LOOP_UNDERPOWER_MAX_RAW_DBFS))
            .unwrap_or(false)
        {
            return false;
        }
        if self.radio_config != ReverseTrafficRadioConfig::Rc3 {
            let pcb_up = self
                .last_pcbs
                .iter()
                .zip(self.last_pcg_observation_epoch)
                .filter(|(pcb, observed_epoch)| **pcb == 0 && observed_epoch.is_some())
                .count();
            let minimum_up_pcbs = self.last_pcg_observation_epoch.iter().flatten().count();
            if pcb_up < minimum_up_pcbs {
                return false;
            }
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
        if crc_valid && self.last_fer_pct < self.target_fer_pct() {
            return;
        }
        let metric = Self::finite_stats(&self.last_pcg_pilot_db);
        let control = Self::finite_stats(&self.last_pcg_control_metric_db);
        let raw = Self::finite_stats(&self.last_pcg_raw_power_db);
        let hot_pcgs = self.recent_overpowered_pcgs();
        let overpowered = self.recent_frame_overpowered();
        let pcb_up = self.last_pcbs.iter().filter(|&&pcb| pcb == 0).count();
        let valid_pcb_up = self
            .last_pcbs
            .iter()
            .zip(self.last_pcg_command_slot_valid)
            .filter(|(pcb, valid)| *valid && **pcb == 0)
            .count();
        let valid_pcb_count = self
            .last_pcg_command_slot_valid
            .iter()
            .filter(|valid| **valid)
            .count();
        let target = self.effective_target_db();
        let control_error = control
            .map(|(avg, _, _, _)| target - avg)
            .unwrap_or(f32::NAN);
        log::info!(
            "power_frame[w{}]: crc={} frame={} fer={:.2}% target={:.2} opwr={} err_avg={:+.2} metric={} control={} limiter_power={} hot_pcgs={}/16 pcb_up={}/16 valid_pcb_up={}/{}",
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
            hot_pcgs,
            pcb_up,
            valid_pcb_up,
            valid_pcb_count,
        );
    }

    /// One record per reverse frame rather than sixteen on the per-PCG path.
    /// Every PCG value is still carried, indexed by absolute PCG modulo 16.
    fn log_batched_pcg_trace(&self, walsh_code: u8) {
        let first_abs_pcg = self
            .last_pcg_abs_pcg
            .iter()
            .copied()
            .filter(|abs_pcg| *abs_pcg != 0)
            .min()
            .unwrap_or(0);
        let last_abs_pcg = self.last_pcg_abs_pcg.iter().copied().max().unwrap_or(0);
        log::info!(
            "power_pcgs[w{}]: abs_pcg={}..{} metric={} control={} limiter_power={} pcb={} clamp={} scheduled={}",
            walsh_code,
            first_abs_pcg,
            last_abs_pcg,
            Self::format_pcg_db(&self.last_pcg_pilot_db),
            Self::format_pcg_db(&self.last_pcg_control_metric_db),
            Self::format_pcg_db(&self.last_pcg_raw_power_db),
            Self::format_pcg_bits(&self.last_pcbs),
            Self::format_pcg_bools(&self.last_pcg_raw_clamp_active),
            Self::format_pcg_bools(&self.last_pcg_command_slot_valid),
        );
    }

    fn format_pcg_db(values: &[f32; 16]) -> String {
        values
            .iter()
            .map(|value| {
                if value.is_finite() {
                    format!("{value:.2}")
                } else {
                    "nan".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    fn format_pcg_bits(values: &[u8; 16]) -> String {
        values
            .iter()
            .map(|value| char::from(b'0' + (*value).min(1)))
            .collect()
    }

    fn format_pcg_bools(values: &[bool; 16]) -> String {
        values
            .iter()
            .map(|value| if *value { '1' } else { '0' })
            .collect()
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
            measured_inner_loop_1s: self.measured_inner_loop_1s,
        }
    }

    fn tick_direct(
        &mut self,
        measured_abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
        valid_ordinal: Option<u64>,
    ) -> BtsPowerControlTick {
        self.record_one_second_metric(measured_abs_pcg, metric_db);
        if metric_db.is_finite() {
            self.last_valid_metric_db = Some(metric_db);
            self.last_valid_metric_abs_pcg = Some(measured_abs_pcg);
        }
        let slot = (measured_abs_pcg % 16) as usize;
        self.last_pcg_observation_epoch[slot] = Some(self.observation_epoch);
        let raw_power_db = raw_power_db.filter(|db| db.is_finite());
        let target_db = self.effective_target_db();
        let command_slot_valid = valid_ordinal.is_some();
        let control_epoch =
            valid_ordinal.is_some_and(|ordinal| ordinal % DIRECT_EPOCH_PCBS == DIRECT_HOLD_PCBS);
        let hold_bit = valid_ordinal
            .map(|ordinal| (ordinal % DIRECT_EPOCH_PCBS) as u8 & 1)
            .unwrap_or(measured_abs_pcg as u8 & 1);
        let safety_threshold_dbfs = self
            .raw_power_profile
            .clip_begin_dbfs
            .unwrap_or(self.raw_power_profile.hot_limit_dbfs);
        let safety_down = raw_power_db
            .map(|db| db > self.adjusted_dbfs(safety_threshold_dbfs))
            .unwrap_or(false);
        let measurement_abs_pcg = self.last_valid_metric_abs_pcg.filter(|source_abs_pcg| {
            measured_abs_pcg.saturating_sub(*source_abs_pcg) <= DIRECT_MAX_MEASUREMENT_AGE_PCGS
        });
        let aged_metric_db = measurement_abs_pcg
            .and(self.last_valid_metric_db)
            .unwrap_or(f32::NAN);
        let measurement_used = control_epoch && aged_metric_db.is_finite() && !safety_down;
        let control_steps: u8 = if measurement_used {
            let error_db = (target_db - aged_metric_db).abs();
            if self.radio_config == ReverseTrafficRadioConfig::Rc1 {
                1
            } else if error_db >= DIRECT_FIVE_STEP_ERROR_DB {
                5
            } else if error_db >= DIRECT_THREE_STEP_ERROR_DB {
                3
            } else {
                1
            }
        } else {
            0
        };
        let continued_burst = valid_ordinal
            .zip(self.burst_start_ordinal)
            .and_then(|(ordinal, control_ordinal)| ordinal.checked_sub(control_ordinal))
            .is_some_and(|offset| offset > 0 && offset < u64::from(self.burst_steps));
        let pcb = if safety_down {
            self.burst_start_ordinal = None;
            self.burst_steps = 0;
            1
        } else if measurement_used {
            let pcb = u8::from(aged_metric_db >= target_db);
            self.burst_bit = pcb;
            self.burst_start_ordinal = valid_ordinal;
            self.burst_steps = control_steps;
            pcb
        } else if control_epoch {
            self.burst_start_ordinal = None;
            self.burst_steps = 0;
            hold_bit
        } else if continued_burst {
            self.burst_bit
        } else {
            if valid_ordinal.is_some() {
                self.burst_start_ordinal = None;
                self.burst_steps = 0;
            }
            hold_bit
        };

        if metric_db.is_finite() {
            self.last_pcg_pilot_db[slot] = metric_db;
        }
        if let Some(raw_power_db) = raw_power_db {
            self.last_pcg_raw_power_db[slot] = raw_power_db;
        }
        self.last_pcg_control_metric_db[slot] = metric_db;
        self.last_pcg_overpowered[slot] = safety_down;
        self.last_pcg_raw_clamp_active[slot] = safety_down;
        self.last_pcg_command_slot_valid[slot] = command_slot_valid;
        self.last_pcg_abs_pcg[slot] = measured_abs_pcg;
        self.last_pcbs[slot] = pcb;
        let measurement_abs_pcg = if measurement_used {
            measurement_abs_pcg
        } else {
            None
        };

        BtsPowerControlTick {
            pcb,
            command_slot_valid,
            target_db,
            control_metric_db: if control_epoch {
                aged_metric_db
            } else {
                metric_db
            },
            raw_power_db,
            raw_power_clamp_active: safety_down,
            control_epoch,
            measurement_used,
            control_steps,
            safety_down,
            valid_ordinal,
            measurement_abs_pcg,
            schedule_accepted: false,
            rc3_scheduler: None,
        }
    }

    #[cfg(test)]
    fn tick_rc3_direct(
        &mut self,
        measured_abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
        valid_ordinal: Option<u64>,
    ) -> BtsPowerControlTick {
        self.tick_direct(measured_abs_pcg, metric_db, raw_power_db, valid_ordinal)
    }

    fn tick_rc3_release_hold(
        &mut self,
        measured_abs_pcg: u64,
        valid_ordinal: Option<u64>,
    ) -> BtsPowerControlTick {
        let slot = (measured_abs_pcg % 16) as usize;
        let pcb = valid_ordinal
            .map(|ordinal| ordinal as u8 & 1)
            .unwrap_or(measured_abs_pcg as u8 & 1);
        self.last_pcg_pilot_db[slot] = f32::NAN;
        self.last_pcg_control_metric_db[slot] = f32::NAN;
        self.last_pcg_raw_power_db[slot] = f32::NAN;
        self.last_pcg_overpowered[slot] = false;
        self.last_pcg_raw_clamp_active[slot] = false;
        self.last_pcg_command_slot_valid[slot] = valid_ordinal.is_some();
        self.last_pcg_abs_pcg[slot] = measured_abs_pcg;
        self.last_pcbs[slot] = pcb;

        BtsPowerControlTick {
            pcb,
            command_slot_valid: valid_ordinal.is_some(),
            target_db: self.effective_target_db(),
            control_metric_db: f32::NAN,
            raw_power_db: None,
            raw_power_clamp_active: false,
            control_epoch: false,
            measurement_used: false,
            control_steps: 0,
            safety_down: false,
            valid_ordinal,
            measurement_abs_pcg: None,
            schedule_accepted: false,
            rc3_scheduler: None,
        }
    }
}

#[cfg(test)]
#[path = "power_control_sinr_tests.rs"]
mod sinr_tests;

#[derive(Clone)]
pub struct BtsPowerControlRegistry {
    states: Arc<Mutex<HashMap<u8, BtsReversePowerControlState>>>,
    rx_power_adj_dbfs: Arc<Mutex<f32>>,
}

impl Default for BtsPowerControlRegistry {
    fn default() -> Self {
        Self {
            states: Arc::default(),
            rx_power_adj_dbfs: Arc::default(),
        }
    }
}

impl BtsPowerControlRegistry {
    pub fn set_rx_power_adj_dbfs(&self, rx_power_adj_dbfs: f32) {
        let rx_power_adj_dbfs = if rx_power_adj_dbfs.is_finite() {
            rx_power_adj_dbfs
        } else {
            0.0
        };
        *self.rx_power_adj_dbfs.lock() = rx_power_adj_dbfs;
        for state in self.states.lock().values_mut() {
            state.set_rx_power_adj_dbfs(rx_power_adj_dbfs);
        }
    }

    pub fn rx_power_adj_dbfs(&self) -> f32 {
        *self.rx_power_adj_dbfs.lock()
    }

    fn state_for(
        radio_config: ReverseTrafficRadioConfig,
        rx_power_adj_dbfs: f32,
    ) -> BtsReversePowerControlState {
        let mut state = match radio_config {
            ReverseTrafficRadioConfig::Rc1 => BtsReversePowerControlState::new_rc1(),
            ReverseTrafficRadioConfig::Rc2 => BtsReversePowerControlState::new_rc2(),
            ReverseTrafficRadioConfig::Rc3 => BtsReversePowerControlState::new_rc3(),
        };
        state.set_rx_power_adj_dbfs(rx_power_adj_dbfs);
        state
    }

    fn traffic_channel_radio_config(
        traffic_channels: &TrafficChannelPool,
        walsh_code: u8,
    ) -> ReverseTrafficRadioConfig {
        traffic_channels
            .lookup(walsh_code)
            .map(|slot| ReverseTrafficRadioConfig::from_channel(&slot.channel))
            .unwrap_or(ReverseTrafficRadioConfig::Rc3)
    }

    fn state_for_radio_config(
        states: &mut HashMap<u8, BtsReversePowerControlState>,
        walsh_code: u8,
        radio_config: ReverseTrafficRadioConfig,
        rx_power_adj_dbfs: f32,
    ) -> (&mut BtsReversePowerControlState, bool) {
        let reset_state = states
            .get(&walsh_code)
            .map(|state| state.radio_config != radio_config)
            .unwrap_or(true);
        if reset_state {
            states.insert(walsh_code, Self::state_for(radio_config, rx_power_adj_dbfs));
        }
        (
            states
                .get_mut(&walsh_code)
                .expect("power-control state inserted above"),
            reset_state,
        )
    }

    pub fn set_target(&self, walsh_code: u8, target_db: f32, held: bool) {
        let rx_power_adj_dbfs = self.rx_power_adj_dbfs();
        let mut states = self.states.lock();
        let state = states
            .entry(walsh_code)
            .or_insert_with(|| Self::state_for(ReverseTrafficRadioConfig::Rc3, rx_power_adj_dbfs));
        state.apply_setpoint(BtsReversePowerSetpoint { target_db, held });
    }

    pub fn outer_loop_tick(
        &self,
        traffic_channels: Option<&TrafficChannelPool>,
        walsh_code: u8,
        frame_valid: bool,
    ) -> BtsPowerControlSnapshot {
        let radio_config = traffic_channels
            .map(|channels| Self::traffic_channel_radio_config(channels, walsh_code))
            .unwrap_or(ReverseTrafficRadioConfig::Rc3);
        let rx_power_adj_dbfs = self.rx_power_adj_dbfs();
        let mut states = self.states.lock();
        let (state, _) =
            Self::state_for_radio_config(&mut states, walsh_code, radio_config, rx_power_adj_dbfs);
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

    pub fn remove(&self, walsh_code: u8) {
        self.states.lock().remove(&walsh_code);
    }

    pub fn enter_rc3_release_hold(
        &self,
        traffic_channels: &TrafficChannelPool,
        walsh_code: u8,
    ) -> bool {
        let Some(slot) = traffic_channels.lookup(walsh_code) else {
            return false;
        };
        let TrafficChannelWrapper::Rc3(channel) = &slot.channel else {
            return false;
        };

        let snapshot = {
            let rx_power_adj_dbfs = self.rx_power_adj_dbfs();
            let mut states = self.states.lock();
            let (state, _) = Self::state_for_radio_config(
                &mut states,
                walsh_code,
                ReverseTrafficRadioConfig::Rc3,
                rx_power_adj_dbfs,
            );
            state.enter_release_hold();
            state.snapshot(walsh_code)
        };

        let scheduler_before = channel.channel.power_control_scheduler_snapshot();
        let mut neutral_start_abs_pcg = None;
        let mut neutral_scheduled = 0_u64;
        if let Some(last_emitted_abs_pcg) = scheduler_before.last_emitted_abs_pcg {
            let start_abs_pcg = last_emitted_abs_pcg.saturating_add(1);
            neutral_start_abs_pcg = Some(start_abs_pcg);
            for abs_pcg in
                start_abs_pcg..start_abs_pcg.saturating_add(RC3_RELEASE_NEUTRAL_FILL_PCGS)
            {
                let Some(ordinal) = channel.channel.power_control_slot_ordinal(abs_pcg) else {
                    continue;
                };
                if channel
                    .channel
                    .schedule_power_control_bit(abs_pcg, ordinal as u8 & 1)
                {
                    neutral_scheduled = neutral_scheduled.saturating_add(1);
                }
            }
        }
        log::info!(
            "power_release_hold[w{}]: target={:.2} fer={:.2}% frames={}/{}err neutral_start_abs_pcg={:?} neutral_scheduled={}",
            walsh_code,
            snapshot.effective_target_eb_nt_db,
            snapshot.fer_pct,
            snapshot.frames_total,
            snapshot.frames_crc_error,
            neutral_start_abs_pcg,
            neutral_scheduled,
        );
        true
    }

    pub fn tick_and_schedule(
        &self,
        traffic_channels: &TrafficChannelPool,
        walsh_code: u8,
        measured_abs_pcg: u64,
        tx_abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
        mobile_power_db: Option<f32>,
        despread_pilot_power_dbfs: Option<f32>,
    ) -> Option<BtsPowerControlTick> {
        let slot = traffic_channels.lookup(walsh_code)?;
        let radio_config = ReverseTrafficRadioConfig::from_channel(&slot.channel);
        let requested_tx_abs_pcg = tx_abs_pcg;
        let control_metric_db = metric_db;
        let (tx_abs_pcg, valid_ordinal) = match &slot.channel {
            TrafficChannelWrapper::Rc1(ch) => {
                let source_ordinal = ch
                    .channel
                    .guaranteed_power_control_ordinal(measured_abs_pcg)?;
                let target_ordinal =
                    source_ordinal.saturating_add(RC12_DIRECT_DELAY_GUARANTEED_SLOTS);
                let target_abs_pcg = ch
                    .channel
                    .power_control_abs_pcg_for_guaranteed_ordinal(target_ordinal);
                debug_assert!(target_abs_pcg >= requested_tx_abs_pcg);
                (target_abs_pcg, Some(target_ordinal))
            }
            TrafficChannelWrapper::Rc2(ch) => {
                let source_ordinal = ch
                    .channel
                    .guaranteed_power_control_ordinal(measured_abs_pcg)?;
                let target_ordinal =
                    source_ordinal.saturating_add(RC12_DIRECT_DELAY_GUARANTEED_SLOTS);
                let target_abs_pcg = ch
                    .channel
                    .power_control_abs_pcg_for_guaranteed_ordinal(target_ordinal);
                debug_assert!(target_abs_pcg >= requested_tx_abs_pcg);
                (target_abs_pcg, Some(target_ordinal))
            }
            TrafficChannelWrapper::Rc3(ch) => (
                tx_abs_pcg,
                ch.channel.power_control_slot_ordinal(tx_abs_pcg),
            ),
            TrafficChannelWrapper::SchRc3(_) => return None,
        };
        let mut tick = {
            let rx_power_adj_dbfs = self.rx_power_adj_dbfs();
            let mut states = self.states.lock();
            let (state, _) = Self::state_for_radio_config(
                &mut states,
                walsh_code,
                radio_config,
                rx_power_adj_dbfs,
            );
            // One command decision per absolute PCG. Replayed or duplicated
            // measurements must not advance controller state again.
            if !state.admit_control_abs_pcg(measured_abs_pcg) {
                return None;
            }
            if state.release_hold {
                state.tick_rc3_release_hold(measured_abs_pcg, valid_ordinal)
            } else {
                state.tick_direct(
                    measured_abs_pcg,
                    control_metric_db,
                    radio_config.limiter_power_db(raw_power_db, mobile_power_db),
                    valid_ordinal,
                )
            }
        };

        match &slot.channel {
            TrafficChannelWrapper::Rc1(ch) => {
                let neutral_count = tx_abs_pcg
                    .saturating_add(RC12_NEUTRAL_LOOKAHEAD_PCGS)
                    .saturating_sub(requested_tx_abs_pcg);
                for (offset, neutral) in ch
                    .channel
                    .power_control_slots(requested_tx_abs_pcg, neutral_count)
                    .into_iter()
                    .enumerate()
                {
                    if !neutral.guaranteed_valid() {
                        ch.channel.schedule_power_control_bit(
                            requested_tx_abs_pcg + offset as u64,
                            neutral.hold_bit,
                        );
                    }
                }
                tick.schedule_accepted =
                    ch.channel.schedule_power_control_bit(tx_abs_pcg, tick.pcb);
            }
            TrafficChannelWrapper::Rc2(ch) => {
                let neutral_count = tx_abs_pcg
                    .saturating_add(RC12_NEUTRAL_LOOKAHEAD_PCGS)
                    .saturating_sub(requested_tx_abs_pcg);
                for (offset, neutral) in ch
                    .channel
                    .power_control_slots(requested_tx_abs_pcg, neutral_count)
                    .into_iter()
                    .enumerate()
                {
                    if !neutral.guaranteed_valid() {
                        ch.channel.schedule_power_control_bit(
                            requested_tx_abs_pcg + offset as u64,
                            neutral.hold_bit,
                        );
                    }
                }
                tick.schedule_accepted =
                    ch.channel.schedule_power_control_bit(tx_abs_pcg, tick.pcb);
            }
            TrafficChannelWrapper::Rc3(ch) => {
                if ch.channel.power_control_slot_is_valid(tx_abs_pcg) {
                    tick.schedule_accepted =
                        ch.channel.schedule_power_control_bit(tx_abs_pcg, tick.pcb);
                }
                tick.rc3_scheduler = Some(ch.channel.power_control_scheduler_snapshot());
            }
            TrafficChannelWrapper::SchRc3(_) => unreachable!(),
        }
        if radio_config == ReverseTrafficRadioConfig::Rc3
            && (tick.control_epoch || tick.safety_down)
            && cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code)
        {
            let kind = if tick.safety_down {
                "safety_down"
            } else if tick.measurement_used {
                "control"
            } else {
                "skipped_control"
            };
            let schedule_result = if tick.schedule_accepted {
                "accepted"
            } else {
                "late_or_invalid"
            };
            let scheduler = tick.rc3_scheduler.unwrap_or_default();
            let schedule_headroom_pcgs = scheduler
                .last_emitted_abs_pcg
                .map(|last_emitted| tx_abs_pcg.saturating_sub(last_emitted));
            let control_measurement_age_pcgs = tick
                .measurement_abs_pcg
                .map(|source_abs_pcg| measured_abs_pcg.saturating_sub(source_abs_pcg));
            log::info!(
                "rc3_control[w{}]: measured_abs_pcg={} measured_abs_chip={} raw_pilot_sinr_db={:.2} control_pilot_sinr_db={:.2} control_measurement_abs_pcg={:?} control_measurement_age_pcgs={:?} despread_pilot_power_dbfs={:.2} pilot_sinr_target_db={:.2} kind={} control_steps={} pcb={} direction={} tx_abs_pcg={} tx_abs_chip={} lead_pcgs={} valid_pcb_ordinal={:?} schedule={} schedule_headroom_pcgs={:?} tx_last_emitted_abs_pcg={:?} scheduler_published={} tx_scheduled_emits={} tx_fallback_emits={} late_schedules={}",
                walsh_code,
                measured_abs_pcg,
                measured_abs_pcg.saturating_mul(SR1_CHIPS_PER_PCG),
                metric_db,
                tick.control_metric_db,
                tick.measurement_abs_pcg,
                control_measurement_age_pcgs,
                despread_pilot_power_dbfs.unwrap_or(f32::NAN),
                tick.target_db,
                kind,
                tick.control_steps,
                tick.pcb,
                if tick.pcb == 0 { "UP" } else { "DOWN" },
                tx_abs_pcg,
                tx_abs_pcg.saturating_mul(SR1_CHIPS_PER_PCG),
                tx_abs_pcg.saturating_sub(measured_abs_pcg),
                tick.valid_ordinal,
                schedule_result,
                schedule_headroom_pcgs,
                scheduler.last_emitted_abs_pcg,
                scheduler.published,
                scheduler.scheduled_emits,
                scheduler.fallback_emits,
                scheduler.late_schedules,
            );
        }
        if radio_config != ReverseTrafficRadioConfig::Rc3
            && (tick.control_epoch || tick.safety_down)
            && cdma_common::diagnostics::power_control_verbose_enabled_for_walsh(walsh_code)
        {
            let kind = if tick.safety_down {
                "safety_down"
            } else if tick.measurement_used {
                "control"
            } else {
                "skipped_control"
            };
            log::info!(
                "rc12_control[{:?} w{}]: measured_abs_pcg={} measured_abs_chip={} control_source={} measured_eb_nt_db={:.2} measured_mobile_power_dbfs={:.2} control_metric_db={:.2} target_db={:.2} kind={} control_steps={} pcb={} direction={} requested_tx_abs_pcg={} tx_abs_pcg={} tx_abs_chip={} lead_pcgs={} valid_pcb_ordinal={:?} schedule={}",
                radio_config,
                walsh_code,
                measured_abs_pcg,
                measured_abs_pcg.saturating_mul(SR1_CHIPS_PER_PCG),
                radio_config.control_metric_name(),
                metric_db,
                mobile_power_db.unwrap_or(f32::NAN),
                tick.control_metric_db,
                tick.target_db,
                kind,
                tick.control_steps,
                tick.pcb,
                if tick.pcb == 0 { "UP" } else { "DOWN" },
                requested_tx_abs_pcg,
                tx_abs_pcg,
                tx_abs_pcg.saturating_mul(SR1_CHIPS_PER_PCG),
                tx_abs_pcg.saturating_sub(measured_abs_pcg),
                tick.valid_ordinal,
                if tick.schedule_accepted {
                    "accepted"
                } else {
                    "late_or_invalid"
                },
            );
        }
        Some(tick)
    }
}
