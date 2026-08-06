//! HRPD reverse-link outer-loop power control.
//!
//! The slot-rate RPC loop controls reverse-pilot symbol SINR. This module owns
//! the slower packet-error feedback that moves that SINR setpoint toward the
//! physical-layer target PER.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;

pub const HRPD_INITIAL_TARGET_DB: f32 = 10.0;
pub const HRPD_AUTO_MIN_TARGET_DB: f32 = 8.0;
pub const HRPD_AUTO_MAX_TARGET_DB: f32 = 14.0;
pub const HRPD_TARGET_PER: f32 = 0.01;
const HRPD_ERASURE_STEP_UP_DB: f32 = 0.25;
const HRPD_OUTER_LOOP_MIN_SUCCESSES: u64 = 10;
const HRPD_TUNE_AWAY_RETURN_SUCCESSES: u8 = 10;
const HRPD_PER_WINDOW_PACKETS: usize = 500;
const HRPD_OUTER_SUMMARY_PACKETS: u64 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrpdTransmissionMode {
    HighCapacity,
    LowLatency,
}

impl HrpdTransmissionMode {
    pub fn from_low_latency(low_latency: bool) -> Self {
        if low_latency {
            Self::LowLatency
        } else {
            Self::HighCapacity
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::HighCapacity => "hicap",
            Self::LowLatency => "lolat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrpdPacketOutcome {
    Success,
    Erasure,
    Excluded(HrpdPacketExclusion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrpdPacketExclusion {
    MobilePowerLimited,
    ReceiverReacquiring,
    TuneAway,
    AbandonedHarq,
    UnknownTerminationTarget,
}

impl HrpdPacketExclusion {
    fn label(self) -> &'static str {
        match self {
            Self::MobilePowerLimited => "mobile_power_limited",
            Self::ReceiverReacquiring => "receiver_reacquiring",
            Self::TuneAway => "tune_away",
            Self::AbandonedHarq => "abandoned_harq",
            Self::UnknownTerminationTarget => "unknown_termination_target",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HrpdPacketObservation {
    pub outcome: HrpdPacketOutcome,
    pub payload_bits: Option<u32>,
    pub transmission_mode: Option<HrpdTransmissionMode>,
    /// One-based subpacket number at which the decode completed.
    pub decoded_subpacket: Option<u8>,
    pub termination_target_subpackets: Option<u8>,
    pub late_success: bool,
}

impl HrpdPacketObservation {
    pub fn rev0(outcome: HrpdPacketOutcome) -> Self {
        Self {
            outcome,
            payload_bits: None,
            transmission_mode: None,
            decoded_subpacket: None,
            termination_target_subpackets: Some(1),
            late_success: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HrpdPowerControlSnapshot {
    pub uati: u32,
    pub mac_index: u8,
    pub generation: u64,
    pub target_db: f32,
    pub target_per: f32,
    pub window_per: f32,
    pub window_packets: usize,
    pub packets_total: u64,
    pub packets_success: u64,
    pub packets_erased: u64,
    pub packets_excluded: u64,
    pub packets_tune_away_excluded: u64,
    pub packets_late_success: u64,
    pub tune_away_active: bool,
    pub return_successes_remaining: u8,
    pub target_saturated_low: bool,
    pub target_saturated_high: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct AssignmentKey {
    uati: u32,
    mac_index: u8,
}

#[derive(Debug)]
struct AssignmentState {
    generation: u64,
    target_db: f32,
    outcomes: VecDeque<bool>,
    packets_total: u64,
    packets_success: u64,
    packets_erased: u64,
    packets_excluded: u64,
    packets_tune_away_excluded: u64,
    packets_late_success: u64,
    tune_away_active: bool,
    return_successes_remaining: u8,
}

impl AssignmentState {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            target_db: HRPD_INITIAL_TARGET_DB,
            outcomes: VecDeque::with_capacity(HRPD_PER_WINDOW_PACKETS),
            packets_total: 0,
            packets_success: 0,
            packets_erased: 0,
            packets_excluded: 0,
            packets_tune_away_excluded: 0,
            packets_late_success: 0,
            tune_away_active: false,
            return_successes_remaining: 0,
        }
    }

    fn success_step_down_db() -> f32 {
        HRPD_ERASURE_STEP_UP_DB * HRPD_TARGET_PER / (1.0 - HRPD_TARGET_PER)
    }

    fn observe(&mut self, observation: HrpdPacketObservation) {
        if let HrpdPacketOutcome::Excluded(_) = observation.outcome {
            self.packets_excluded = self.packets_excluded.saturating_add(1);
            if observation.outcome == HrpdPacketOutcome::Excluded(HrpdPacketExclusion::TuneAway) {
                self.packets_tune_away_excluded = self.packets_tune_away_excluded.saturating_add(1);
            }
            return;
        }

        self.packets_total = self.packets_total.saturating_add(1);
        let success = observation.outcome == HrpdPacketOutcome::Success;
        if success {
            self.packets_success = self.packets_success.saturating_add(1);
        } else {
            self.packets_erased = self.packets_erased.saturating_add(1);
        }
        if observation.late_success {
            self.packets_late_success = self.packets_late_success.saturating_add(1);
        }
        if self.outcomes.len() == HRPD_PER_WINDOW_PACKETS {
            self.outcomes.pop_front();
        }
        self.outcomes.push_back(success);

        if success && self.return_successes_remaining > 0 {
            self.return_successes_remaining -= 1;
            return;
        }

        if self.packets_success >= HRPD_OUTER_LOOP_MIN_SUCCESSES {
            if success {
                self.target_db =
                    (self.target_db - Self::success_step_down_db()).max(HRPD_AUTO_MIN_TARGET_DB);
            } else {
                self.target_db =
                    (self.target_db + HRPD_ERASURE_STEP_UP_DB).min(HRPD_AUTO_MAX_TARGET_DB);
            }
        }
    }

    fn snapshot(&self, key: AssignmentKey) -> HrpdPowerControlSnapshot {
        let erased = self.outcomes.iter().filter(|success| !**success).count();
        let window_packets = self.outcomes.len();
        HrpdPowerControlSnapshot {
            uati: key.uati,
            mac_index: key.mac_index,
            generation: self.generation,
            target_db: self.target_db,
            target_per: HRPD_TARGET_PER,
            window_per: erased as f32 / window_packets.max(1) as f32,
            window_packets,
            packets_total: self.packets_total,
            packets_success: self.packets_success,
            packets_erased: self.packets_erased,
            packets_excluded: self.packets_excluded,
            packets_tune_away_excluded: self.packets_tune_away_excluded,
            packets_late_success: self.packets_late_success,
            tune_away_active: self.tune_away_active,
            return_successes_remaining: self.return_successes_remaining,
            target_saturated_low: self.target_db <= HRPD_AUTO_MIN_TARGET_DB,
            target_saturated_high: self.target_db >= HRPD_AUTO_MAX_TARGET_DB,
        }
    }
}

#[derive(Debug, Default)]
struct RegistryState {
    next_generation: u64,
    assignments: HashMap<AssignmentKey, AssignmentState>,
}

#[derive(Debug, Clone, Default)]
pub struct HrpdPowerControlRegistry {
    inner: Arc<Mutex<RegistryState>>,
}

impl HrpdPowerControlRegistry {
    pub fn install(&self, uati: u32, mac_index: u8) -> HrpdPowerControlHandle {
        let key = AssignmentKey { uati, mac_index };
        let mut registry = self.inner.lock();
        registry.next_generation = registry.next_generation.wrapping_add(1).max(1);
        let generation = registry.next_generation;
        registry
            .assignments
            .insert(key, AssignmentState::new(generation));
        log::info!(
            "hrpd_power[m{}]: installed uati=0x{:08x} generation={} target={:.2}dB range={:.1}..{:.1}dB target_per={:.2}%",
            mac_index,
            uati,
            generation,
            HRPD_INITIAL_TARGET_DB,
            HRPD_AUTO_MIN_TARGET_DB,
            HRPD_AUTO_MAX_TARGET_DB,
            100.0 * HRPD_TARGET_PER,
        );
        HrpdPowerControlHandle {
            registry: self.clone(),
            key,
            generation,
        }
    }

    pub fn release(&self, uati: u32, mac_index: u8) {
        let key = AssignmentKey { uati, mac_index };
        if let Some(state) = self.inner.lock().assignments.remove(&key) {
            let snapshot = state.snapshot(key);
            log::info!(
                "hrpd_power[m{}]: released uati=0x{:08x} generation={} target={:.2}dB per={:.2}% packets={} erased={} excluded={} tuneaway_excluded={} late={} tuneaway_active={} return_warmup={}",
                mac_index,
                uati,
                snapshot.generation,
                snapshot.target_db,
                100.0 * snapshot.window_per,
                snapshot.packets_total,
                snapshot.packets_erased,
                snapshot.packets_excluded,
                snapshot.packets_tune_away_excluded,
                snapshot.packets_late_success,
                snapshot.tune_away_active,
                snapshot.return_successes_remaining,
            );
        }
    }

    pub fn snapshots(&self) -> Vec<HrpdPowerControlSnapshot> {
        self.inner
            .lock()
            .assignments
            .iter()
            .map(|(key, state)| state.snapshot(*key))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct HrpdPowerControlHandle {
    registry: HrpdPowerControlRegistry,
    key: AssignmentKey,
    generation: u64,
}

impl HrpdPowerControlHandle {
    pub fn target_db(&self) -> f32 {
        self.with_state(|state| state.target_db)
            .unwrap_or(HRPD_INITIAL_TARGET_DB)
    }

    pub fn termination_target_subpackets(
        &self,
        payload_bits: u32,
        mode: HrpdTransmissionMode,
    ) -> Option<u8> {
        if !matches!(
            payload_bits,
            128 | 256 | 512 | 768 | 1024 | 1536 | 2048 | 3072 | 4096 | 6144 | 8192 | 12288
        ) {
            return None;
        }
        // C.S0024-A encodes one less than the number of subpackets; the
        // default attributes are 1 for Low Latency and 3 for High Capacity.
        Some(match mode {
            HrpdTransmissionMode::LowLatency => 2,
            HrpdTransmissionMode::HighCapacity => 4,
        })
    }

    pub fn report(&self, observation: HrpdPacketObservation) -> Option<HrpdPowerControlSnapshot> {
        let mut registry = self.registry.inner.lock();
        let state = registry.assignments.get_mut(&self.key)?;
        if state.generation != self.generation {
            return None;
        }
        let observation = if state.tune_away_active {
            HrpdPacketObservation {
                outcome: HrpdPacketOutcome::Excluded(HrpdPacketExclusion::TuneAway),
                ..observation
            }
        } else {
            observation
        };
        let previous_target = state.target_db;
        state.observe(observation);
        let snapshot = state.snapshot(self.key);
        let target_changed = (snapshot.target_db - previous_target).abs() > f32::EPSILON;
        let report_summary =
            snapshot.packets_total != 0 && snapshot.packets_total % HRPD_OUTER_SUMMARY_PACKETS == 0;
        match observation.outcome {
            HrpdPacketOutcome::Erasure if target_changed => log::info!(
                "hrpd_power[m{}]: packet outcome=erasure uati=0x{:08x} payload={:?} mode={} decoded_subpacket={:?} target_subpackets={:?} late={} target={:.3}->{:.3}dB per={:.2}% ({}/{}) saturated_high={}",
                self.key.mac_index,
                self.key.uati,
                observation.payload_bits,
                observation
                    .transmission_mode
                    .map_or("unknown", HrpdTransmissionMode::label),
                observation.decoded_subpacket,
                observation.termination_target_subpackets,
                observation.late_success,
                previous_target,
                snapshot.target_db,
                100.0 * snapshot.window_per,
                snapshot.packets_erased,
                snapshot.packets_total,
                snapshot.target_saturated_high,
            ),
            HrpdPacketOutcome::Erasure => log::debug!(
                "hrpd_power[m{}]: packet outcome=erasure uati=0x{:08x} payload={:?} mode={} decoded_subpacket={:?} target_subpackets={:?} late={} target={:.3}dB per={:.2}% ({}/{})",
                self.key.mac_index,
                self.key.uati,
                observation.payload_bits,
                observation
                    .transmission_mode
                    .map_or("unknown", HrpdTransmissionMode::label),
                observation.decoded_subpacket,
                observation.termination_target_subpackets,
                observation.late_success,
                snapshot.target_db,
                100.0 * snapshot.window_per,
                snapshot.packets_erased,
                snapshot.packets_total,
            ),
            HrpdPacketOutcome::Excluded(reason) => log::debug!(
                "hrpd_power[m{}]: packet outcome=excluded reason={} uati=0x{:08x} payload={:?} mode={} target={:.3}dB excluded={}",
                self.key.mac_index,
                reason.label(),
                self.key.uati,
                observation.payload_bits,
                observation
                    .transmission_mode
                    .map_or("unknown", HrpdTransmissionMode::label),
                snapshot.target_db,
                snapshot.packets_excluded,
            ),
            HrpdPacketOutcome::Success => {}
        }
        if report_summary {
            log::info!(
                "hrpd_power[m{}]: summary uati=0x{:08x} target={:.3}dB per={:.2}% window={} packets={} erased={} excluded={} tuneaway_excluded={} late={} tuneaway_active={} return_warmup={} saturated_low={}",
                self.key.mac_index,
                self.key.uati,
                snapshot.target_db,
                100.0 * snapshot.window_per,
                snapshot.window_packets,
                snapshot.packets_total,
                snapshot.packets_erased,
                snapshot.packets_excluded,
                snapshot.packets_tune_away_excluded,
                snapshot.packets_late_success,
                snapshot.tune_away_active,
                snapshot.return_successes_remaining,
                snapshot.target_saturated_low,
            );
        }
        Some(snapshot)
    }

    pub fn suspend_for_tune_away(&self) -> bool {
        let mut registry = self.registry.inner.lock();
        let Some(state) = registry.assignments.get_mut(&self.key) else {
            return false;
        };
        if state.generation != self.generation || state.tune_away_active {
            return false;
        }
        state.tune_away_active = true;
        state.return_successes_remaining = 0;
        log::info!(
            "hrpd_power[m{}]: suspended for tune-away uati=0x{:08x} target={:.3}dB",
            self.key.mac_index,
            self.key.uati,
            state.target_db,
        );
        true
    }

    pub fn resume_after_tune_away(&self) -> bool {
        let mut registry = self.registry.inner.lock();
        let Some(state) = registry.assignments.get_mut(&self.key) else {
            return false;
        };
        if state.generation != self.generation || !state.tune_away_active {
            return false;
        }
        state.tune_away_active = false;
        state.return_successes_remaining = HRPD_TUNE_AWAY_RETURN_SUCCESSES;
        log::info!(
            "hrpd_power[m{}]: resumed after tune-away uati=0x{:08x} target={:.3}dB downward_warmup_successes={}",
            self.key.mac_index,
            self.key.uati,
            state.target_db,
            state.return_successes_remaining,
        );
        true
    }

    pub fn snapshot(&self) -> Option<HrpdPowerControlSnapshot> {
        self.with_state(|state| state.snapshot(self.key))
    }

    fn with_state<T>(&self, f: impl FnOnce(&AssignmentState) -> T) -> Option<T> {
        let registry = self.registry.inner.lock();
        let state = registry.assignments.get(&self.key)?;
        (state.generation == self.generation).then(|| f(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success() -> HrpdPacketObservation {
        HrpdPacketObservation::rev0(HrpdPacketOutcome::Success)
    }

    fn erasure() -> HrpdPacketObservation {
        HrpdPacketObservation::rev0(HrpdPacketOutcome::Erasure)
    }

    #[test]
    fn asymmetric_steps_have_zero_expected_drift_at_target_per() {
        let down = AssignmentState::success_step_down_db();
        let expected = HRPD_TARGET_PER * HRPD_ERASURE_STEP_UP_DB - (1.0 - HRPD_TARGET_PER) * down;
        assert!(expected.abs() < 1.0e-7);
    }

    #[test]
    fn target_adaptation_starts_after_successful_packet_warmup() {
        let registry = HrpdPowerControlRegistry::default();
        let handle = registry.install(0x8005_8001, 5);
        handle.report(erasure());
        assert_eq!(handle.target_db(), HRPD_INITIAL_TARGET_DB);
        for _ in 0..9 {
            handle.report(success());
        }
        assert_eq!(handle.target_db(), HRPD_INITIAL_TARGET_DB);
        handle.report(success());
        let warmed_target = handle.target_db();
        assert!(warmed_target < HRPD_INITIAL_TARGET_DB);
        handle.report(erasure());
        assert_eq!(handle.target_db(), warmed_target + HRPD_ERASURE_STEP_UP_DB);
    }

    #[test]
    fn automatic_target_is_clamped_to_calibrated_range() {
        let registry = HrpdPowerControlRegistry::default();
        let handle = registry.install(0x8005_8001, 5);
        for _ in 0..HRPD_OUTER_LOOP_MIN_SUCCESSES {
            handle.report(success());
        }
        for _ in 0..100 {
            handle.report(erasure());
        }
        assert_eq!(handle.target_db(), HRPD_AUTO_MAX_TARGET_DB);
        for _ in 0..10_000 {
            handle.report(success());
        }
        assert_eq!(handle.target_db(), HRPD_AUTO_MIN_TARGET_DB);
    }

    #[test]
    fn excluded_packets_do_not_move_target_or_enter_per() {
        let registry = HrpdPowerControlRegistry::default();
        let handle = registry.install(0x8005_8001, 5);
        handle.report(HrpdPacketObservation::rev0(HrpdPacketOutcome::Excluded(
            HrpdPacketExclusion::MobilePowerLimited,
        )));
        let snapshot = handle.snapshot().unwrap();
        assert_eq!(snapshot.target_db, HRPD_INITIAL_TARGET_DB);
        assert_eq!(snapshot.packets_total, 0);
        assert_eq!(snapshot.packets_excluded, 1);
    }

    #[test]
    fn tune_away_freezes_adaptation_and_warms_downward_control_after_return() {
        let registry = HrpdPowerControlRegistry::default();
        let handle = registry.install(0x8005_8001, 5);
        for _ in 0..HRPD_OUTER_LOOP_MIN_SUCCESSES {
            handle.report(success());
        }
        let target_before_tune_away = handle.target_db();

        assert!(handle.suspend_for_tune_away());
        handle.report(success());
        handle.report(erasure());
        let suspended = handle.snapshot().unwrap();
        assert_eq!(suspended.target_db, target_before_tune_away);
        assert_eq!(suspended.packets_total, HRPD_OUTER_LOOP_MIN_SUCCESSES);
        assert_eq!(suspended.packets_excluded, 2);
        assert_eq!(suspended.packets_tune_away_excluded, 2);
        assert!(suspended.tune_away_active);

        assert!(handle.resume_after_tune_away());
        handle.report(erasure());
        let protected_target = target_before_tune_away + HRPD_ERASURE_STEP_UP_DB;
        assert_eq!(handle.target_db(), protected_target);
        for _ in 0..HRPD_TUNE_AWAY_RETURN_SUCCESSES {
            handle.report(success());
        }
        let warmed = handle.snapshot().unwrap();
        assert_eq!(warmed.target_db, protected_target);
        assert_eq!(warmed.return_successes_remaining, 0);
        assert!(!warmed.tune_away_active);

        handle.report(success());
        assert!(handle.target_db() < protected_target);
    }

    #[test]
    fn stale_assignment_handle_cannot_modify_reused_identity() {
        let registry = HrpdPowerControlRegistry::default();
        let stale = registry.install(0x8005_8001, 5);
        let current = registry.install(0x8005_8001, 5);
        assert!(stale.report(erasure()).is_none());
        assert_eq!(current.target_db(), HRPD_INITIAL_TARGET_DB);
        assert_eq!(current.snapshot().unwrap().packets_total, 0);
        current.report(erasure());
        assert_eq!(current.snapshot().unwrap().packets_total, 1);
    }

    #[test]
    fn rev_a_default_termination_targets_are_mode_specific() {
        let registry = HrpdPowerControlRegistry::default();
        let handle = registry.install(0x8005_8001, 5);
        assert_eq!(
            handle.termination_target_subpackets(1024, HrpdTransmissionMode::LowLatency),
            Some(2)
        );
        assert_eq!(
            handle.termination_target_subpackets(1024, HrpdTransmissionMode::HighCapacity),
            Some(4)
        );
        assert_eq!(
            handle.termination_target_subpackets(999, HrpdTransmissionMode::LowLatency),
            None
        );
    }
}
