use std::collections::VecDeque;

use cdma_common::hrpd::air::HrpdTrafficEvent;

use super::subframe_harq::{REVERSE_INTERLACES, SubframeOutcome};
use super::subtype2_data::MAX_SUBPACKETS;

const SUBFRAME_SLOTS: u64 = 4;
// C.S0024-A §13.3.1.3.2.2.4: four subpackets on three interlaces.
const SUBPACKET_SPACING_SLOTS: u64 = REVERSE_INTERLACES as u64 * SUBFRAME_SLOTS;

#[derive(Clone, Copy)]
struct PendingPacket {
    first_slot: u64,
    expires_at_slot: u64,
}

struct QueuedData {
    first_slot: u64,
    event: HrpdTrafficEvent,
}

#[derive(Default)]
pub(super) struct ReverseDataOrdering {
    pending: [Option<PendingPacket>; REVERSE_INTERLACES],
    queue: VecDeque<QueuedData>,
}

impl ReverseDataOrdering {
    pub(super) fn expire(&mut self, start_slot: u64) {
        for pending in &mut self.pending {
            if pending.is_some_and(|packet| start_slot >= packet.expires_at_slot) {
                *pending = None;
            }
        }
    }

    pub(super) fn observe(&mut self, start_slot: u64, outcome: &SubframeOutcome) {
        self.pending[usize::from(outcome.interlace)] =
            if outcome.decoded || usize::from(outcome.subpacket_id) + 1 >= MAX_SUBPACKETS {
                None
            } else {
                outcome
                    .first_subpacket_start_slot
                    .map(|first_slot| PendingPacket {
                        first_slot,
                        expires_at_slot: start_slot.saturating_add(
                            (MAX_SUBPACKETS as u64 - 1 - u64::from(outcome.subpacket_id))
                                * SUBPACKET_SPACING_SLOTS
                                + SUBFRAME_SLOTS,
                        ),
                    })
            };
    }

    pub(super) fn push(&mut self, first_slot: u64, event: HrpdTrafficEvent) {
        let index = self
            .queue
            .partition_point(|queued| queued.first_slot <= first_slot);
        self.queue.insert(index, QueuedData { first_slot, event });
    }

    pub(super) fn pop_ready(&mut self) -> Option<HrpdTrafficEvent> {
        let first_slot = self.queue.front()?.first_slot;
        if self
            .pending
            .iter()
            .flatten()
            .any(|pending| pending.first_slot < first_slot)
        {
            return None;
        }
        self.queue.pop_front().map(|queued| queued.event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(interlace: u8, first_slot: u64, subpacket_id: u8) -> SubframeOutcome {
        SubframeOutcome {
            interlace,
            first_subpacket_start_slot: Some(first_slot),
            subpacket_id,
            ..Default::default()
        }
    }

    fn packet(sequence: u32) -> HrpdTrafficEvent {
        HrpdTrafficEvent::Stream1Packet {
            uati: 1,
            stream_id: 1,
            sequence,
            payload: vec![1],
            decoded_at: None,
            air_frame_end_received_at: None,
        }
    }

    #[test]
    fn later_data_waits_for_observed_harq_recovery() {
        let mut ordering = ReverseDataOrdering::default();
        ordering.observe(120, &outcome(0, 120, 0));
        ordering.push(124, packet(121));
        assert_eq!(ordering.pop_ready(), None);
        let recovered = SubframeOutcome {
            decoded: true,
            ..outcome(0, 120, 1)
        };
        ordering.observe(132, &recovered);
        ordering.push(120, packet(0));
        assert_eq!(ordering.pop_ready(), Some(packet(0)));
        assert_eq!(ordering.pop_ready(), Some(packet(121)));
        assert_eq!(ordering.pop_ready(), None);
    }

    #[test]
    fn missing_final_subpacket_cannot_block_data_indefinitely() {
        let mut ordering = ReverseDataOrdering::default();
        ordering.observe(120, &outcome(0, 120, 0));
        ordering.push(124, packet(121));
        ordering.expire(156);
        assert_eq!(ordering.pop_ready(), None);
        ordering.expire(160);
        assert_eq!(ordering.pop_ready(), Some(packet(121)));
    }

    #[test]
    fn missed_early_subpackets_do_not_extend_the_recovery_deadline() {
        let mut ordering = ReverseDataOrdering::default();
        ordering.observe(144, &outcome(0, 144, 2));
        ordering.push(148, packet(121));
        ordering.expire(156);
        assert_eq!(ordering.pop_ready(), None);
        ordering.expire(160);
        assert_eq!(ordering.pop_ready(), Some(packet(121)));
    }

    #[test]
    fn abandoned_and_exhausted_packets_release_later_data() {
        for replacement in [
            SubframeOutcome::default(),
            outcome(0, 120, 3),
            outcome(0, 132, 0),
        ] {
            let mut ordering = ReverseDataOrdering::default();
            ordering.observe(120, &outcome(0, 120, 0));
            ordering.push(124, packet(121));
            ordering.observe(156, &replacement);
            assert_eq!(ordering.pop_ready(), Some(packet(121)));
        }
    }

    #[test]
    fn independent_interlaces_only_block_later_packets() {
        let mut ordering = ReverseDataOrdering::default();
        ordering.observe(120, &outcome(0, 120, 0));
        ordering.observe(124, &outcome(1, 124, 0));
        ordering.push(128, packet(242));
        ordering.observe(
            136,
            &SubframeOutcome {
                decoded: true,
                ..outcome(0, 120, 1)
            },
        );
        ordering.push(120, packet(0));
        assert_eq!(ordering.pop_ready(), Some(packet(0)));
        assert_eq!(ordering.pop_ready(), None);
        ordering.observe(
            136,
            &SubframeOutcome {
                decoded: true,
                ..outcome(1, 124, 1)
            },
        );
        ordering.push(124, packet(121));
        assert_eq!(ordering.pop_ready(), Some(packet(121)));
        assert_eq!(ordering.pop_ready(), Some(packet(242)));
    }

    #[test]
    fn unobserved_losses_do_not_delay_delivery_and_timestamps_are_preserved() {
        let mut ordering = ReverseDataOrdering::default();
        let mut event = packet(121);
        if let HrpdTrafficEvent::Stream1Packet {
            decoded_at,
            air_frame_end_received_at,
            ..
        } = &mut event
        {
            *decoded_at = Some(std::time::Instant::now());
            *air_frame_end_received_at = *decoded_at;
        }
        ordering.push(124, event.clone());
        assert_eq!(ordering.pop_ready(), Some(event));
    }
}
