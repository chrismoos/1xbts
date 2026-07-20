//! HRPD overhead message schedule.
//!
//! Spec references:
//! - C.S0024-300 §9 — Control Channel MAC, 256-slot Control Channel cycle.
//! - C.S0024-0 v4.0 §9.4 — Forward Control Channel; SectorParameters,
//!   AccessParameters, SyncMessage are sent in asynchronous capsules.
//!
//! This module owns *which* overhead messages should be transmitted at the
//! start of a given Control Channel cycle. The actual on-air bit layout of
//! each message body lives in `cdma-common` (`cdma_common::hrpd::messages`).
//! To keep this crate decoupled from that source, the schedule returns trait
//! objects (`&dyn OverheadMessage`); the cdma-common types only need a
//! one-line `impl OverheadMessage` adapter.
//!
//! Default overhead schedule (see `defaults`):
//! - SyncMessage + AccessParameters: every 3 cycles (~1.28 s), phase 0.
//! - BroadcastReverseRateLimit: every 4 cycles (~1.71 s), phase 1.
//! - QuickConfig: every cycle. C.S0024-0 requires it in every synchronous
//!   Sleep State capsule; it carries frequently changing public data such as
//!   ForwardTrafficValid.
//! - SectorParameters: every 4 cycles (~1.71 s), phase 2.

/// Source of an encoded overhead message body. Implemented by the
/// `cdma-common` HRPD message types via a thin adapter; this trait avoids a
/// direct dependency on those types.
pub trait OverheadMessage {
    /// Bit-packed message body, ready to be wrapped by
    /// `ControlChannelCapsule::frame`.
    fn encode(&self) -> Vec<u8>;
}

/// Overhead schedule, in slot units. Each message has a period and phase
/// offset, both expressed in slots. Periods are normally multiples of one
/// Control Channel cycle.
#[derive(Debug, Clone, Copy)]
pub struct OverheadSchedule {
    pub quick_config_period_slots: u32,
    pub quick_config_offset_slots: u32,
    pub sector_params_period_slots: u32,
    pub sector_params_offset_slots: u32,
    pub access_params_period_slots: u32,
    pub access_params_offset_slots: u32,
    pub sync_period_slots: u32,
    pub sync_offset_slots: u32,
    pub reverse_rate_period_slots: u32,
    pub reverse_rate_offset_slots: u32,
}

impl OverheadSchedule {
    /// Default overhead schedule. See module docs for rationale.
    pub fn defaults() -> Self {
        let cycle = super::control_channel::CTRL_CH_CYCLE_SLOTS;
        Self {
            quick_config_period_slots: cycle,
            quick_config_offset_slots: 0,
            sector_params_period_slots: cycle * 4,
            sector_params_offset_slots: cycle * 2,
            access_params_period_slots: cycle * 3,
            access_params_offset_slots: 0,
            sync_period_slots: cycle * 3,
            sync_offset_slots: 0,
            reverse_rate_period_slots: cycle * 4,
            reverse_rate_offset_slots: cycle,
        }
    }

    /// Which overhead message slots are populated at the start of Control
    /// Channel cycle `cycle_index`. Returns booleans for
    /// `(quick_config, sector_params, access_params, sync)` so that callers
    /// can plug in the appropriate `&dyn OverheadMessage` from their own
    /// message store.
    ///
    /// This is the trait-free core; `messages_for_cycle` wraps it with a
    /// `&dyn OverheadMessage` view over a `Sources` struct.
    pub fn slots_for_cycle(&self, cycle_index: u64) -> ScheduleSlots {
        let cycle = super::control_channel::CTRL_CH_CYCLE_SLOTS as u64;
        // The schedule periods are expressed in slots. Convert to cycles by
        // integer division; if a period isn't a clean multiple of the cycle
        // length we still fire whenever slot index % period == 0, which is the
        // same rule used by the scheduler tick.
        let slot_index = cycle_index.saturating_mul(cycle);
        let fires = |period: u32, offset: u32| -> bool {
            let p = period as u64;
            p != 0 && slot_index >= offset as u64 && (slot_index - offset as u64) % p == 0
        };
        ScheduleSlots {
            quick_config: fires(
                self.quick_config_period_slots,
                self.quick_config_offset_slots,
            ),
            sector_params: fires(
                self.sector_params_period_slots,
                self.sector_params_offset_slots,
            ),
            access_params: fires(
                self.access_params_period_slots,
                self.access_params_offset_slots,
            ),
            sync: fires(self.sync_period_slots, self.sync_offset_slots),
            reverse_rate: fires(
                self.reverse_rate_period_slots,
                self.reverse_rate_offset_slots,
            ),
        }
    }

    /// Resolve the schedule against a concrete set of message bodies. Any
    /// `None` source is simply skipped (e.g. SyncMessage may not be wired up
    /// yet).
    pub fn messages_for_cycle<'a>(
        &self,
        cycle_index: u64,
        sources: &'a OverheadSources<'a>,
    ) -> Vec<&'a dyn OverheadMessage> {
        let slots = self.slots_for_cycle(cycle_index);
        let mut out: Vec<&'a dyn OverheadMessage> = Vec::new();
        if slots.sync {
            if let Some(m) = sources.sync {
                out.push(m);
            }
        }
        if slots.quick_config {
            if let Some(m) = sources.quick_config {
                out.push(m);
            }
        }
        if slots.sector_params {
            if let Some(m) = sources.sector_params {
                out.push(m);
            }
        }
        if slots.access_params {
            if let Some(m) = sources.access_params {
                out.push(m);
            }
        }
        if slots.reverse_rate {
            if let Some(m) = sources.reverse_rate {
                out.push(m);
            }
        }
        out
    }
}

/// Boolean view of which overhead messages fire at a given cycle boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleSlots {
    pub quick_config: bool,
    pub sector_params: bool,
    pub access_params: bool,
    pub sync: bool,
    pub reverse_rate: bool,
}

/// Trait-object handles to the current overhead message bodies. The scheduler
/// holds onto these between cycles; an upstream component updates them when a
/// SectorParameters/AccessParameters value changes.
#[derive(Default, Clone, Copy)]
pub struct OverheadSources<'a> {
    pub quick_config: Option<&'a dyn OverheadMessage>,
    pub sector_params: Option<&'a dyn OverheadMessage>,
    pub access_params: Option<&'a dyn OverheadMessage>,
    pub sync: Option<&'a dyn OverheadMessage>,
    pub reverse_rate: Option<&'a dyn OverheadMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bts::hrpd::control_channel::CTRL_CH_CYCLE_SLOTS;

    struct StubMessage(&'static str);
    impl OverheadMessage for StubMessage {
        fn encode(&self) -> Vec<u8> {
            self.0.as_bytes().to_vec()
        }
    }

    #[test]
    fn defaults_matches_documented_periods() {
        let s = OverheadSchedule::defaults();
        assert_eq!(s.quick_config_period_slots, CTRL_CH_CYCLE_SLOTS);
        assert_eq!(s.quick_config_offset_slots, 0);
        assert_eq!(s.sector_params_period_slots, CTRL_CH_CYCLE_SLOTS * 4);
        assert_eq!(s.sector_params_offset_slots, CTRL_CH_CYCLE_SLOTS * 2);
        assert_eq!(s.access_params_period_slots, CTRL_CH_CYCLE_SLOTS * 3);
        assert_eq!(s.access_params_offset_slots, 0);
        assert_eq!(s.sync_period_slots, CTRL_CH_CYCLE_SLOTS * 3);
        assert_eq!(s.sync_offset_slots, 0);
        assert_eq!(s.reverse_rate_period_slots, CTRL_CH_CYCLE_SLOTS * 4);
        assert_eq!(s.reverse_rate_offset_slots, CTRL_CH_CYCLE_SLOTS);
    }

    #[test]
    fn defaults_match_spec_overhead_cadence() {
        let s = OverheadSchedule::defaults();
        let fires: Vec<ScheduleSlots> = (0..12u64).map(|c| s.slots_for_cycle(c)).collect();
        let active = |slot: &ScheduleSlots| {
            (
                slot.sync,
                slot.quick_config,
                slot.sector_params,
                slot.access_params,
                slot.reverse_rate,
            )
        };
        assert_eq!(
            fires.iter().map(active).collect::<Vec<_>>(),
            vec![
                (true, true, false, true, false),
                (false, true, false, false, true),
                (false, true, true, false, false),
                (true, true, false, true, false),
                (false, true, false, false, false),
                (false, true, false, false, true),
                (true, true, true, true, false),
                (false, true, false, false, false),
                (false, true, false, false, false),
                (true, true, false, true, true),
                (false, true, true, false, false),
                (false, true, false, false, false),
            ]
        );
    }

    #[test]
    fn small_periods_fire_at_expected_cycles() {
        // Use a deliberately tiny schedule expressed in slots so the test math
        // is easy to read: SectorParameters every 2 cycles, AccessParameters
        // every 2 cycles, Sync every 4 cycles.
        let s = OverheadSchedule {
            quick_config_period_slots: CTRL_CH_CYCLE_SLOTS,
            quick_config_offset_slots: 0,
            sector_params_period_slots: CTRL_CH_CYCLE_SLOTS * 2,
            sector_params_offset_slots: 0,
            access_params_period_slots: CTRL_CH_CYCLE_SLOTS * 2,
            access_params_offset_slots: 0,
            sync_period_slots: CTRL_CH_CYCLE_SLOTS * 4,
            sync_offset_slots: 0,
            reverse_rate_period_slots: CTRL_CH_CYCLE_SLOTS * 2,
            reverse_rate_offset_slots: CTRL_CH_CYCLE_SLOTS,
        };
        let fires: Vec<ScheduleSlots> = (0..8u64).map(|c| s.slots_for_cycle(c)).collect();

        // QuickConfig: every cycle.
        assert!(fires.iter().all(|f| f.quick_config));

        // SectorParameters / AccessParameters: cycles 0, 2, 4, 6.
        for (cycle, f) in fires.iter().enumerate() {
            let expected = cycle % 2 == 0;
            assert_eq!(f.sector_params, expected, "sector cycle {cycle}");
            assert_eq!(f.access_params, expected, "access cycle {cycle}");
        }

        // Sync: cycles 0, 4.
        for (cycle, f) in fires.iter().enumerate() {
            let expected = cycle % 4 == 0;
            assert_eq!(f.sync, expected, "sync cycle {cycle}");
        }

        // ReverseRate: cycles 1, 3, 5, 7.
        for (cycle, f) in fires.iter().enumerate() {
            let expected = cycle % 2 == 1;
            assert_eq!(f.reverse_rate, expected, "reverse-rate cycle {cycle}");
        }
    }

    #[test]
    fn messages_for_cycle_resolves_via_sources() {
        let qc = StubMessage("qc");
        let sp = StubMessage("sp");
        let ap = StubMessage("ap");
        let sy = StubMessage("sy");
        let rr = StubMessage("rr");
        let sources = OverheadSources {
            quick_config: Some(&qc),
            sector_params: Some(&sp),
            access_params: Some(&ap),
            sync: Some(&sy),
            reverse_rate: Some(&rr),
        };
        let s = OverheadSchedule {
            quick_config_period_slots: CTRL_CH_CYCLE_SLOTS,
            quick_config_offset_slots: 0,
            sector_params_period_slots: CTRL_CH_CYCLE_SLOTS * 2,
            sector_params_offset_slots: 0,
            access_params_period_slots: CTRL_CH_CYCLE_SLOTS * 2,
            access_params_offset_slots: 0,
            sync_period_slots: CTRL_CH_CYCLE_SLOTS * 4,
            sync_offset_slots: 0,
            reverse_rate_period_slots: CTRL_CH_CYCLE_SLOTS * 2,
            reverse_rate_offset_slots: CTRL_CH_CYCLE_SLOTS,
        };

        let at_zero = s.messages_for_cycle(0, &sources);
        let encoded: Vec<Vec<u8>> = at_zero.iter().map(|m| m.encode()).collect();
        assert_eq!(
            encoded,
            vec![
                b"sy".to_vec(),
                b"qc".to_vec(),
                b"sp".to_vec(),
                b"ap".to_vec(),
            ]
        );

        // Cycle 1: QuickConfig + ReverseRate.
        let at_one = s.messages_for_cycle(1, &sources);
        let encoded: Vec<Vec<u8>> = at_one.iter().map(|m| m.encode()).collect();
        assert_eq!(encoded, vec![b"qc".to_vec(), b"rr".to_vec()]);

        // Cycle 2: QC + SP + AP, no Sync.
        let at_two = s.messages_for_cycle(2, &sources);
        let encoded: Vec<Vec<u8>> = at_two.iter().map(|m| m.encode()).collect();
        assert_eq!(
            encoded,
            vec![b"qc".to_vec(), b"sp".to_vec(), b"ap".to_vec()]
        );

        // Cycle 4: the non-reverse sources fire again.
        let at_four = s.messages_for_cycle(4, &sources);
        assert_eq!(at_four.len(), 4);
    }

    #[test]
    fn missing_sources_are_skipped() {
        let qc = StubMessage("qc");
        let sources = OverheadSources {
            quick_config: Some(&qc),
            sector_params: None,
            access_params: None,
            sync: None,
            reverse_rate: None,
        };
        let s = OverheadSchedule::defaults();
        let msgs = s.messages_for_cycle(2, &sources);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].encode(), b"qc".to_vec());
    }
}
