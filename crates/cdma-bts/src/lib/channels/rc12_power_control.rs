use cdma_common::phy::data_burst_randomizer::{
    RC12_CHIPS_PER_PCG, RC12_PCGS_PER_FRAME, Rc12ReverseRate, active_pcgs,
};

use crate::phy::coding::long_code::LongCodeGenerator;

// An RC1/RC2 PCB is observed two PCGs after its reverse randomizer source PCG.
pub(crate) const RC12_PCB_VALIDITY_DELAY_PCGS: u64 = 2;
const GUARANTEED_SLOTS_PER_FRAME: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rc12PowerControlSlot {
    pub guaranteed_ordinal: Option<u64>,
    pub hold_bit: u8,
}

impl Rc12PowerControlSlot {
    pub(crate) fn guaranteed_valid(self) -> bool {
        self.guaranteed_ordinal.is_some()
    }
}

#[derive(Clone)]
pub(crate) struct Rc12PowerControlCadence {
    reverse_long_code_origin: LongCodeGenerator,
}

impl Rc12PowerControlCadence {
    pub(crate) fn new(reverse_long_code_origin: LongCodeGenerator) -> Self {
        Self {
            reverse_long_code_origin,
        }
    }

    fn source_frame_slots(
        &self,
        frame_start_abs_pcg: u64,
    ) -> [Rc12PowerControlSlot; RC12_PCGS_PER_FRAME] {
        let frame_chip_start = frame_start_abs_pcg * RC12_CHIPS_PER_PCG;
        let half = active_pcgs(
            &self.reverse_long_code_origin,
            frame_chip_start,
            Rc12ReverseRate::Half,
        );
        let quarter = active_pcgs(
            &self.reverse_long_code_origin,
            frame_chip_start,
            Rc12ReverseRate::Quarter,
        );
        let eighth = active_pcgs(
            &self.reverse_long_code_origin,
            frame_chip_start,
            Rc12ReverseRate::Eighth,
        );
        let frame_index = frame_start_abs_pcg / RC12_PCGS_PER_FRAME as u64;
        let mut tier_ordinals = [0u8; 4];
        let mut guaranteed_in_frame = 0u64;

        std::array::from_fn(|pcg| {
            let tier = if eighth[pcg] {
                0
            } else if quarter[pcg] {
                1
            } else if half[pcg] {
                2
            } else {
                3
            };
            let hold_bit = tier_ordinals[tier] & 1;
            tier_ordinals[tier] += 1;
            let guaranteed_ordinal = (tier == 0).then(|| {
                let ordinal = frame_index * GUARANTEED_SLOTS_PER_FRAME + guaranteed_in_frame;
                guaranteed_in_frame += 1;
                ordinal
            });
            Rc12PowerControlSlot {
                guaranteed_ordinal,
                hold_bit,
            }
        })
    }

    pub(crate) fn power_control_slots(
        &self,
        start_abs_pcg: u64,
        count: u64,
    ) -> Vec<Rc12PowerControlSlot> {
        let mut cached_frame = None;
        let mut cached_slots = [Rc12PowerControlSlot {
            guaranteed_ordinal: None,
            hold_bit: 0,
        }; RC12_PCGS_PER_FRAME];

        (0..count)
            .map(|offset| {
                let pcb_abs_pcg = start_abs_pcg + offset;
                let Some(source_abs_pcg) = pcb_abs_pcg.checked_sub(RC12_PCB_VALIDITY_DELAY_PCGS)
                else {
                    return Rc12PowerControlSlot {
                        guaranteed_ordinal: None,
                        hold_bit: (pcb_abs_pcg & 1) as u8,
                    };
                };
                let frame_start_abs_pcg =
                    source_abs_pcg / RC12_PCGS_PER_FRAME as u64 * RC12_PCGS_PER_FRAME as u64;
                if cached_frame != Some(frame_start_abs_pcg) {
                    cached_slots = self.source_frame_slots(frame_start_abs_pcg);
                    cached_frame = Some(frame_start_abs_pcg);
                }
                cached_slots[(source_abs_pcg % RC12_PCGS_PER_FRAME as u64) as usize]
            })
            .collect()
    }

    pub(crate) fn guaranteed_ordinal_for_measurement(&self, measured_abs_pcg: u64) -> Option<u64> {
        self.power_control_slots(
            measured_abs_pcg.saturating_add(RC12_PCB_VALIDITY_DELAY_PCGS),
            1,
        )[0]
        .guaranteed_ordinal
    }

    pub(crate) fn pcb_abs_pcg_for_guaranteed_ordinal(&self, ordinal: u64) -> u64 {
        let frame_index = ordinal / GUARANTEED_SLOTS_PER_FRAME;
        let ordinal_in_frame = ordinal % GUARANTEED_SLOTS_PER_FRAME;
        let frame_start_abs_pcg = frame_index * RC12_PCGS_PER_FRAME as u64;
        let slots = self.source_frame_slots(frame_start_abs_pcg);
        let source_pcg = slots
            .iter()
            .position(|slot| {
                slot.guaranteed_ordinal
                    == Some(frame_index * GUARANTEED_SLOTS_PER_FRAME + ordinal_in_frame)
            })
            .expect("every RC1/RC2 frame has two guaranteed power-control slots");
        frame_start_abs_pcg + source_pcg as u64 + RC12_PCB_VALIDITY_DELAY_PCGS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cadence() -> Rc12PowerControlCadence {
        Rc12PowerControlCadence::new(LongCodeGenerator::new_traffic_channel(0xDEAD_BEEF))
    }

    #[test]
    fn guaranteed_ordinals_are_contiguous_across_frames() {
        let cadence = cadence();
        let slots = cadence.power_control_slots(2, 64 * RC12_PCGS_PER_FRAME as u64);
        let ordinals: Vec<u64> = slots
            .iter()
            .filter_map(|slot| slot.guaranteed_ordinal)
            .collect();

        assert_eq!(ordinals, (0..128).collect::<Vec<_>>());
    }

    #[test]
    fn guaranteed_ordinal_round_trips_to_absolute_pcg() {
        let cadence = cadence();
        for ordinal in 0..128 {
            let abs_pcg = cadence.pcb_abs_pcg_for_guaranteed_ordinal(ordinal);
            assert_eq!(
                cadence.power_control_slots(abs_pcg, 1)[0].guaranteed_ordinal,
                Some(ordinal)
            );
        }
    }

    #[test]
    fn three_guaranteed_slots_always_exceed_nine_pcg_scheduling_floor() {
        let cadence = cadence();
        for source_ordinal in 0..512 {
            let measured_abs_pcg = cadence
                .pcb_abs_pcg_for_guaranteed_ordinal(source_ordinal)
                .saturating_sub(RC12_PCB_VALIDITY_DELAY_PCGS);
            let target_abs_pcg = cadence.pcb_abs_pcg_for_guaranteed_ordinal(source_ordinal + 3);
            let lead_pcgs = target_abs_pcg - measured_abs_pcg;

            assert!(lead_pcgs >= 9, "ordinal={source_ordinal} lead={lead_pcgs}");
        }
    }

    #[test]
    fn hold_bits_are_neutral_for_every_reverse_rate() {
        let cadence = cadence();
        for frame in 0..32u64 {
            let frame_start_abs_pcg = frame * RC12_PCGS_PER_FRAME as u64;
            let slots = cadence.power_control_slots(
                frame_start_abs_pcg + RC12_PCB_VALIDITY_DELAY_PCGS,
                RC12_PCGS_PER_FRAME as u64,
            );
            for rate in [
                Rc12ReverseRate::Full,
                Rc12ReverseRate::Half,
                Rc12ReverseRate::Quarter,
                Rc12ReverseRate::Eighth,
            ] {
                let active = active_pcgs(
                    &cadence.reverse_long_code_origin,
                    frame_start_abs_pcg * RC12_CHIPS_PER_PCG,
                    rate,
                );
                let net_steps: i32 = slots
                    .iter()
                    .zip(active)
                    .filter_map(|(slot, active)| {
                        active.then_some(if slot.hold_bit == 0 { 1 } else { -1 })
                    })
                    .sum();
                assert_eq!(net_steps, 0, "frame={frame} rate={rate:?}");
            }
        }
    }
}
