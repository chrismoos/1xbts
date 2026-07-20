//! HRPD (1xEV-DO Rev 0) forward link slot chip allocation.
//!
//! Per C.S0024-200 §9.3 (Forward Channel Structure). A slot is 2048 chips
//! (1.667 ms at 1.2288 Mcps), composed of two symmetric 1024-chip half-slots.
//! Within each half-slot the chip allocation is:
//!
//! | chips      | channel                              |
//! |------------|--------------------------------------|
//! | 0..400     | Forward Traffic or Control (data)    |
//! | 400..464   | MAC                                  |
//! | 464..560   | Pilot                                |
//! | 560..624   | MAC                                  |
//! | 624..1024  | Forward Traffic or Control (data)    |
//!
//! Forward Traffic and Forward Control are TDM in the data regions: a given
//! slot carries one or the other, never both.

/// Chips per HRPD forward slot.
pub const SLOT_CHIPS: u64 = 2_048;
/// Chips per HRPD forward half-slot.
pub const HALF_SLOT_CHIPS: u64 = 1_024;
/// Chips in one MAC burst (4 bursts per slot).
pub const MAC_BURST_CHIPS: u64 = 64;
/// Chips in one Pilot burst (2 bursts per slot).
pub const PILOT_BURST_CHIPS: u64 = 96;
/// Chips of data region per half-slot edge (400 + 400 = 800 / half-slot).
pub const DATA_EDGE_CHIPS: u64 = 400;

/// Logical channel occupying a given chip within a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotChannel {
    /// Forward Traffic or Forward Control Channel (TDM, decided per packet).
    Data,
    /// MAC channel (RPC/RA/DRCLock).
    Mac,
    /// Pilot channel.
    Pilot,
}

/// Return which logical channel owns chip `chip` within the current slot.
///
/// `chip` is taken modulo `SLOT_CHIPS`, so absolute or slot-relative chip
/// indices are both accepted.
#[inline]
pub fn channel_for_chip(chip: u64) -> SlotChannel {
    let half = chip % HALF_SLOT_CHIPS;
    if half < DATA_EDGE_CHIPS {
        SlotChannel::Data
    } else if half < DATA_EDGE_CHIPS + MAC_BURST_CHIPS {
        SlotChannel::Mac
    } else if half < DATA_EDGE_CHIPS + MAC_BURST_CHIPS + PILOT_BURST_CHIPS {
        SlotChannel::Pilot
    } else if half < DATA_EDGE_CHIPS + 2 * MAC_BURST_CHIPS + PILOT_BURST_CHIPS {
        SlotChannel::Mac
    } else {
        SlotChannel::Data
    }
}

/// True if `chip` lies inside a Pilot burst. Matches the existing
/// `HrpdIdlePilot::is_pilot_burst_chip` predicate.
#[inline]
pub fn is_pilot_chip(chip: u64) -> bool {
    matches!(channel_for_chip(chip), SlotChannel::Pilot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_slot_regions_match_spec_offsets() {
        // Per C.S0024-200 Table 9.3-1 (first half-slot).
        for chip in 0..400 {
            assert_eq!(channel_for_chip(chip), SlotChannel::Data, "chip {chip}");
        }
        for chip in 400..464 {
            assert_eq!(channel_for_chip(chip), SlotChannel::Mac, "chip {chip}");
        }
        for chip in 464..560 {
            assert_eq!(channel_for_chip(chip), SlotChannel::Pilot, "chip {chip}");
        }
        for chip in 560..624 {
            assert_eq!(channel_for_chip(chip), SlotChannel::Mac, "chip {chip}");
        }
        for chip in 624..1024 {
            assert_eq!(channel_for_chip(chip), SlotChannel::Data, "chip {chip}");
        }
    }

    #[test]
    fn second_half_slot_mirrors_first() {
        for chip in 0..HALF_SLOT_CHIPS {
            assert_eq!(
                channel_for_chip(chip),
                channel_for_chip(chip + HALF_SLOT_CHIPS),
                "chip {chip}"
            );
        }
    }

    #[test]
    fn slot_modulo_wraps_cleanly() {
        let base = 7 * SLOT_CHIPS + 470;
        assert_eq!(channel_for_chip(base), SlotChannel::Pilot);
    }

    #[test]
    fn pilot_burst_predicate_matches_legacy_layout() {
        // Legacy HrpdIdlePilot: burst at (HALF_SLOT_CHIPS - PILOT_BURST_CHIPS)/2.
        let legacy_start = (HALF_SLOT_CHIPS - PILOT_BURST_CHIPS) / 2;
        let legacy_end = legacy_start + PILOT_BURST_CHIPS;
        for chip in 0..SLOT_CHIPS {
            let half_chip = chip % HALF_SLOT_CHIPS;
            let legacy = (legacy_start..legacy_end).contains(&half_chip);
            assert_eq!(is_pilot_chip(chip), legacy, "chip {chip}");
        }
    }

    #[test]
    fn channel_counts_sum_to_slot() {
        let mut data = 0u64;
        let mut mac = 0u64;
        let mut pilot = 0u64;
        for chip in 0..SLOT_CHIPS {
            match channel_for_chip(chip) {
                SlotChannel::Data => data += 1,
                SlotChannel::Mac => mac += 1,
                SlotChannel::Pilot => pilot += 1,
            }
        }
        assert_eq!(data, 2 * 2 * DATA_EDGE_CHIPS);
        assert_eq!(mac, 4 * MAC_BURST_CHIPS);
        assert_eq!(pilot, 2 * PILOT_BURST_CHIPS);
        assert_eq!(data + mac + pilot, SLOT_CHIPS);
    }
}
