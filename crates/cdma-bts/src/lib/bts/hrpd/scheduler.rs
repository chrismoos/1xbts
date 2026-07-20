//! HRPD (1xEV-DO Rev 0) Forward Traffic Channel scheduler + H-ARQ.
//!
//! Per C.S0024-0 v4.0:
//!   - §9.3 Forward Channel Structure (slot layout, MAC/Pilot/Data regions).
//!   - §9.3.1.3 Forward Traffic Channel coding and modulation chain
//!     (turbo → scrambler → channel interleave → modulation symbol mapper →
//!     repetition/puncturing → 16-way Walsh cover).
//!   - §9.3 4-slot interlace H-ARQ on the forward link: a multi-slot
//!     physical-layer packet is transmitted on one 4-slot interlace; its slots
//!     are separated by three intervening slots. The AT acknowledges (ACK) or
//!     fails-to-acknowledge (NAK) each received slot on reverse slot n + 3.
//!
//! The scheduler returns the per-slot Data-region chip stream and MAC bits as
//! plain values; `HrpdForwardSlotModulator` owns it and calls `next_slot()`
//! once per slot to build the forward waveform.

use num_complex::Complex32;
use std::sync::Arc;

use crate::bts::hrpd::harq_bus::{HarqBus, HarqEmissionEvent};
use crate::phy::hrpd::interleaver::forward_channel_interleave;
use crate::phy::hrpd::rates::{ForwardRate, HrpdModulation, by_drc};
use crate::phy::hrpd::scrambler::HrpdForwardScrambler;
use crate::phy::hrpd::turbo::HrpdTurboEncoder;
use crate::receiver::hrpd::ack_decoder::ACK_FORWARD_TO_REVERSE_SLOT_OFFSET;
use cdma_common::diagnostics::hrpd_harq_verbose;
use cdma_common::hrpd::messages::DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE;
use cdma_common::hrpd::traffic::{
    DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE,
    DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
    DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE, ENHANCED_PHYSICAL_FCS_BITS,
    FORWARD_TRAFFIC_MAC_PACKET_BITS, FORWARD_TRAFFIC_MAC_PAD_BITS,
    FORWARD_TRAFFIC_MAC_TRAILER_BITS, PHYSICAL_FCS_BITS, PHYSICAL_TAIL_BITS,
    REVERSE_TRAFFIC_CHANNEL_MAC_GRANT_MESSAGE_ID, REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID,
    default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype,
    default_signaling_ftc_payload_bits_with_ack_for_mac_subtype,
    enhanced_forward_traffic_mac_packet_bits, format_b_ftc_payload_bits_for_mac_subtype,
    forward_traffic_security_capacity_bits_for_mac_subtype,
    implemented_forward_traffic_payload_bits_for_drc_for_mac_subtype,
    parse_connection_format_b_packets, parse_default_packet_rlp_packet_bits,
    parse_stream_layer_packet_bytes, physical_crc16, physical_crc24,
};

mod forward_codec;
use forward_codec::{
    DefaultSignalingPacket, default_signaling_packet, forward_format_b_session_packets,
    rebuild_or_split_format_b_ftc_payloads, reliable_rtc_ack_sequence_number,
};

/// Total Data-region chips per HRPD forward slot (2 × 400 chips × 2 half-slots
/// edges, see `phy::hrpd::slot`).
pub const DATA_CHIPS_PER_SLOT: usize = 1600;

/// Total MAC bits emitted per slot: 4 MAC bursts × 64 chips, BPSK on the MAC
/// channel — one bit per chip. Real RPC/RA encoding is not wired in here
/// (C.S0024-0 v4.0 §9.3.1.2).
pub const MAC_BITS_PER_SLOT: usize = 256;

/// Forward-traffic interlace count. C.S0024-0 v4.0 §9.3 fixes a 4-slot
/// interlace on the forward link.
const INTERLACE: u64 = 4;

/// Mask for the 3-bit SLP sequence-number / ACK-sequence-number field in the
/// Default Signaling Protocol header.
const SLP_SEQUENCE_MASK: u8 = 0x07;

/// FrameOffset and DRCLength used by the AT to time its DRC, per the values
/// the AN writes into the TrafficChannelAssignment (FrameOffset 0, DRCLength
/// 8 slots, encoded as `drc_length` field 3). These govern the
/// §8.4.6.1.4.1.2 rate selection and must track the TCA builder in
/// `cdma_common::hrpd::air`.
const FORWARD_FRAME_OFFSET_SLOTS: u64 = 0;
const FORWARD_DRC_LENGTH_SLOTS: u64 = 8;

/// Extra slots to wait after the ACK-channel response slot before retiring a
/// forward packet with no decoded feedback. Reverse ACK decode runs through a
/// separate worker and can arrive tens of slots after the terminal forward
/// slot; recycling immediately on the next interlace floods setup RTCAck
/// copies before the AT's response can affect scheduling.
const HARQ_FEEDBACK_GRACE_SLOTS: u64 = 64;

/// Bound completed-but-not-yet-ACKed packets per MAC while allowing one-slot
/// rates to pipeline. The ACK for a one-slot packet necessarily arrives after
/// the packet is over, so it cannot gate the next packet. Keep this above the
/// 64-slot feedback grace window so missed feedback does not throttle DRC 0xc
/// back-to-back packet starts.
const MAX_OUTSTANDING_PACKETS_PER_MAC: usize = 64;

/// Info-level scheduler visibility without per-packet hot-path logging.
/// HRPD has 600 slots/s, so this is approximately one live second.
const SCHEDULER_STATS_WINDOW_SLOTS: u64 = 600;

/// One queued forward-traffic packet to schedule.
///
/// `payload` is one queued physical-payload representation (one bit per byte,
/// 0/1), including MAC, CRC, and 6-bit TAIL fields the turbo encoder discards.
/// The scheduler, not the caller, binds the packet to the AT's governing DRC
/// at the packet start slot and rebuilds recognized payloads when the selected
/// DRC uses a different payload size.
#[derive(Debug, Clone)]
pub struct ForwardTrafficPacket {
    pub mac_index: u8,
    pub physical_layer_subtype: u16,
    pub forward_traffic_mac_subtype: u16,
    /// RLP retransmissions have priority 60 over first transmissions at 70.
    pub high_priority: bool,
    /// One bit per byte (0 or 1).
    pub payload: Vec<u8>,
}

/// AT response to a transmitted subpacket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarqResponse {
    Ack,
    Nak,
    /// No decision yet — keep waiting.
    Unknown,
}

/// What the scheduler decided to put in a given slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotKind {
    /// Forward Traffic slot serving the given user MAC index.
    Traffic { active_mac: u8 },
    /// Slot is owned by the Forward Control Channel — caller fills the
    /// Data region from the Control Channel pipeline.
    Control,
    /// No packet was due and no Control Channel slot was scheduled.
    Idle,
}

/// One slot of scheduler output.
#[derive(Debug, Clone)]
pub struct ForwardSlotOutput {
    pub channel: SlotKind,
    /// Modulation symbols for the slot's Data regions. Length is
    /// `DATA_CHIPS_PER_SLOT` for Traffic slots; empty for Control / Idle.
    pub data_chips: Vec<Complex32>,
    /// MAC channel bits for the slot (4 bursts × 64 chips). Always populated.
    pub mac_bits: Vec<u8>,
}

/// Per-MAC-index H-ARQ state machine.
#[derive(Debug, Clone)]
struct HarqState {
    packet_id: u64,
    packet: ForwardTrafficPacket,
    /// Pre-computed TDM Data-region chips for the whole physical-layer
    /// packet: MACIndex-coded preamble followed by encoded data symbols.
    packet_chips: Vec<Complex32>,
    /// Number of subpackets the packet is divided into (one per interlace).
    subpacket_count: u8,
    /// Index of the subpacket currently in flight (or next to transmit).
    current_subpacket: u8,
    /// True once this subpacket has been emitted on its 4-slot run and the
    /// scheduler is waiting for an ACK/NAK on the next interlace.
    awaiting_ack: bool,
    /// Slot index at which the next attempt of `current_subpacket` becomes
    /// eligible (used for retransmit pacing).
    next_eligible_slot: u64,
    /// Last slot used by this packet. ACK/NAK transitions keep subsequent
    /// attempts on the same 4-slot H-ARQ interlace.
    last_tx_slot: Option<u64>,
    /// First slot of the current physical-layer packet.
    first_tx_slot: Option<u64>,
    /// Slot count consumed within the current subpacket (0..slots_per_subpacket()).
    slot_in_subpacket: u8,
    rate: ForwardRate,
}

impl HarqState {
    fn slots_per_subpacket(&self) -> u8 {
        // Rev 0 has no H-ARQ subpackets: the whole physical packet is
        // transmitted across `rate.slots` slots at 4-slot interlace spacing
        // with no response wait in between (C.S0024-0 §9.3.1.3.1). The AT
        // combines the packet's slots at exactly n, n+4, ..., so pausing
        // mid-packet for an ACK makes the packet undecodable.
        self.rate.slots
    }
}

/// Forward Traffic Channel scheduler with simple FIFO arbitration and
/// per-MAC-index 4-interlace H-ARQ.
#[derive(Debug)]
pub struct HrpdForwardScheduler {
    queue: Vec<ForwardTrafficPacket>,
    /// In-flight packets. One-slot rates may have several completed packets
    /// waiting for ACK feedback for the same MAC index; multi-slot packets
    /// still keep exact 4-slot continuation timing through `pick_active`.
    active: Vec<HarqState>,
    last_unknown_harq_log_slot: u64,
    last_no_governing_drc_log_slot: u64,
    next_packet_id: u64,
    /// Optional shared H-ARQ event bus to the reverse traffic RX worker.
    /// When present, the scheduler:
    ///   - drains decoded ACK/NAK feedback at the top of each slot and
    ///     applies it via `handle_ack`, and
    ///   - publishes an `HarqEmissionEvent` for each transmitted traffic
    ///     slot so the RX worker can decode the ACK Channel at the correct
    ///     reverse slot (forward slot `n` -> reverse slot `n + 3`,
    ///     C.S0024-0 v4.0 §9.2.1.3.3.4).
    harq_bus: Option<Arc<HarqBus>>,
    stats_window_start_slot: Option<u64>,
    stats_traffic_slots: u32,
    stats_control_slots: u32,
    stats_idle_slots: u32,
    stats_harq_no_response: u32,
    stats_info_bits: u64,
    stats_drc: [u32; 16],
    stats_phase: [u32; 4],
    stats_idle_reasons: [u32; IdleReason::COUNT],
}

impl Default for HrpdForwardScheduler {
    fn default() -> Self {
        Self {
            queue: Vec::new(),
            active: Vec::new(),
            last_unknown_harq_log_slot: 0,
            last_no_governing_drc_log_slot: 0,
            next_packet_id: 1,
            harq_bus: None,
            stats_window_start_slot: None,
            stats_traffic_slots: 0,
            stats_control_slots: 0,
            stats_idle_slots: 0,
            stats_harq_no_response: 0,
            stats_info_bits: 0,
            stats_drc: [0; 16],
            stats_phase: [0; 4],
            stats_idle_reasons: [0; IdleReason::COUNT],
        }
    }
}

impl HrpdForwardScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a shared H-ARQ event bus. The same bus handle must be passed
    /// to every reverse traffic RX worker for the corresponding MAC indices
    /// so emission and feedback events are routed correctly.
    pub fn set_harq_bus(&mut self, bus: Arc<HarqBus>) {
        self.harq_bus = Some(bus);
    }

    pub fn enqueue(&mut self, pkt: ForwardTrafficPacket) {
        if let Some(sequence_number) = rtc_ack_sequence_number(&pkt.payload)
            && self.has_pending_rtc_ack(pkt.mac_index, sequence_number)
        {
            log::info!(
                "rx_hrpd_traffic[m{}]: coalescing duplicate RTCAck seq={} already queued or active",
                pkt.mac_index,
                sequence_number
            );
            return;
        }

        if let Some(queued_signaling) = default_signaling_packet(&pkt.payload)
            && !is_default_packet_data_payload(&queued_signaling)
        {
            // Stream-0 control, especially subtype-3 RTCMAC Grant refreshes,
            // has an air-interface deadline. Keep it ahead of bulk RLP while
            // preserving FIFO order among control packets.
            let insert_at = self
                .queue
                .iter()
                .position(|queued| default_signaling_packet(&queued.payload).is_none())
                .unwrap_or(self.queue.len());
            if is_rtc_mac_grant_signaling(&queued_signaling)
                && let Some(stale) = self.queue[..insert_at].iter_mut().find(|queued| {
                    queued.mac_index == pkt.mac_index && is_rtc_mac_grant(&queued.payload)
                })
            {
                *stale = pkt;
                return;
            }
            self.queue.insert(insert_at, pkt);
        } else if pkt.high_priority {
            // C.S0024-500-C 2.4.4.5 assigns RLP retransmissions priority 60
            // and first transmissions priority 70. Keep control first and
            // FIFO order within the retransmission class.
            let insert_at = self
                .queue
                .iter()
                .position(|queued| {
                    default_signaling_packet(&queued.payload).is_none() && !queued.high_priority
                })
                .unwrap_or(self.queue.len());
            log::debug!(
                "rx_hrpd_traffic[m{}]: enqueueing priority DefaultPacket RLP repair ranges=[{}] insert_at={} queued_before={}",
                pkt.mac_index,
                format_default_packet_rlp_ranges(&pkt.payload),
                insert_at,
                self.queue.len(),
            );
            self.queue.insert(insert_at, pkt);
        } else {
            self.queue.push(pkt);
        }
    }

    fn has_pending_rtc_ack(&self, mac_index: u8, sequence_number: u8) -> bool {
        self.queue.iter().any(|queued| {
            queued.mac_index == mac_index
                && rtc_ack_sequence_number(&queued.payload) == Some(sequence_number)
        }) || self.active.iter().any(|active| {
            active.packet.mac_index == mac_index
                && rtc_ack_sequence_number(&active.packet.payload) == Some(sequence_number)
        })
    }

    /// Purge all forward scheduler and H-ARQ bus state for a released MAC.
    pub fn purge_mac(&mut self, mac_index: u8) -> (usize, usize, usize, usize) {
        let queued_before = self.queue.len();
        self.queue.retain(|pkt| pkt.mac_index != mac_index);
        let queued = queued_before - self.queue.len();

        let active_before = self.active.len();
        self.active
            .retain(|state| state.packet.mac_index != mac_index);
        let active = active_before - self.active.len();

        let (emissions, feedback) = self
            .harq_bus
            .as_ref()
            .map(|bus| bus.purge_mac_queues(mac_index))
            .unwrap_or((0, 0));

        (queued, active, emissions, feedback)
    }

    /// True when a physical-layer packet has already started or is waiting for
    /// feedback. Callers may use this to avoid holding a continuation slot.
    pub fn has_active_packets(&self) -> bool {
        !self.active.is_empty()
    }

    /// Apply an AT H-ARQ decision for `(mac_index, packet_id, subpacket)`.
    ///
    /// - `Ack`: advance to the next subpacket (or complete the packet).
    /// - `Nak`: keep `current_subpacket` and retransmit on the next
    ///   interlace boundary.
    /// - `Unknown`: no state change.
    pub fn handle_ack(
        &mut self,
        mac_index: u8,
        packet_id: u64,
        subpacket: u8,
        response: HarqResponse,
    ) {
        let Some(idx) = self.active.iter().position(|s| {
            s.packet.mac_index == mac_index
                && s.packet_id == packet_id
                && s.current_subpacket == subpacket
        }) else {
            return;
        };
        match response {
            HarqResponse::Ack | HarqResponse::Nak => {
                // Rev 0 ACK Channel bits are emitted per transmitted traffic
                // slot. A positive ACK before the final DRC1/DRC2/etc. slot
                // means the AT has already decoded the physical packet, so
                // stop sending the remaining interlace continuations. A NAK
                // also retires the packet; upper-layer reliable SLP resends.
                let state = self.active.remove(idx);
                log::log!(
                    if response == HarqResponse::Nak {
                        log::Level::Debug
                    } else {
                        log::Level::Trace
                    },
                    "HRPD forward scheduler: harq_feedback mac={} packet_id={} drc=0x{:x} payload_bits={} slots={} subtype=0x{:04x} response={:?}",
                    mac_index,
                    packet_id,
                    state.rate.drc_index,
                    state.rate.payload_bits,
                    state.rate.slots,
                    state.packet.forward_traffic_mac_subtype,
                    response,
                );
            }
            HarqResponse::Unknown => {}
        }
    }

    /// Produce the scheduler decision for one slot.
    ///
    /// `is_control_slot` comes from the Forward Control Channel scheduler —
    /// when true, Traffic yields and the caller fills the Data region from the
    /// Control pipeline. MAC bits are still emitted.
    pub fn next_slot(&mut self, slot_index: u64, is_control_slot: bool) -> ForwardSlotOutput {
        let mac_bits = default_mac_bits();

        // Drain decoded ACK/NAK feedback published by the reverse traffic
        // RX worker since the last call. Each event updates one in-flight
        // H-ARQ state by scheduler-local packet id, which is required once
        // one-slot packets are pipelined for the same MAC index.
        if let Some(bus) = self.harq_bus.clone() {
            for mac in 0u8..crate::bts::hrpd::harq_bus::MAC_INDEX_COUNT as u8 {
                for event in bus.drain_feedback(mac) {
                    self.handle_ack(mac, event.packet_id, event.subpacket, event.response);
                }
            }
        }
        self.retire_expired_feedback_waits(slot_index);

        if is_control_slot {
            self.record_slot_stats(slot_index, SlotStatsKind::Control);
            return ForwardSlotOutput {
                channel: SlotKind::Control,
                data_chips: Vec::new(),
                mac_bits,
            };
        }

        // Bind a queued packet only when this slot is free to start it. The
        // governing DRC is defined for the actual packet start slot.
        if !self.has_due_continuation(slot_index) {
            self.promote_queue(slot_index);
        }

        let Some(idx) = self.pick_active(slot_index) else {
            self.record_slot_stats(
                slot_index,
                SlotStatsKind::Idle {
                    reason: self.idle_reason(slot_index),
                },
            );
            return ForwardSlotOutput {
                channel: SlotKind::Idle,
                data_chips: Vec::new(),
                mac_bits,
            };
        };

        let active_mac;
        let data_chips;
        let tx_drc;
        let tx_info_bits_per_slot;
        let done;
        let retired_without_emit;
        let mut emission_events: Vec<(u8, HarqEmissionEvent)> = Vec::new();
        {
            let state = &mut self.active[idx];
            active_mac = state.packet.mac_index;
            tx_drc = state.rate.drc_index;
            tx_info_bits_per_slot =
                u64::from(state.rate.payload_bits) / u64::from(state.rate.slots.max(1));
            let missed_continuation = !state.awaiting_ack
                && state.slot_in_subpacket > 0
                && slot_index > state.next_eligible_slot;
            if missed_continuation {
                // A continuation slot was lost (displaced by another
                // consumer). The AT combines slots at fixed positions, so
                // the packet is already undecodable; drop the remainder and
                // let SLP retransmission resend the payload.
                log::info!(
                    "HRPD forward scheduler: dropping packet mac={} — continuation slot {} missed (now {})",
                    state.packet.mac_index,
                    state.next_eligible_slot,
                    slot_index
                );
                done = true;
                retired_without_emit = true;
                data_chips = Vec::new();
            } else if state.current_subpacket >= state.subpacket_count {
                // All subpackets exhausted; retire the packet and yield Idle
                // this slot.
                done = true;
                retired_without_emit = true;
                data_chips = Vec::new();
            } else {
                retired_without_emit = false;
                data_chips = build_subpacket_slot_chips(state);
                if state.first_tx_slot.is_none() {
                    state.first_tx_slot = Some(slot_index);
                }
                let packet_start_slot = state.first_tx_slot.unwrap_or(slot_index);
                if state.last_tx_slot.is_none() && state.slot_in_subpacket == 0 {
                    log::trace!(
                        "HRPD forward scheduler: starting packet id={} mac={} drc=0x{:x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} slots={} preamble_chips={} slot={}",
                        state.packet_id,
                        state.packet.mac_index,
                        state.rate.drc_index,
                        state.packet.physical_layer_subtype,
                        state.packet.forward_traffic_mac_subtype,
                        state.rate.slots,
                        state.rate.preamble_chips,
                        slot_index
                    );
                }
                state.last_tx_slot = Some(slot_index);

                let terminal_slot = state.slot_in_subpacket + 1 >= state.slots_per_subpacket();
                emission_events.push((
                    state.packet.mac_index,
                    HarqEmissionEvent {
                        packet_id: state.packet_id,
                        subpacket: state.current_subpacket,
                        packet_start_slot,
                        forward_slot: slot_index,
                        expected_ack_reverse_slot: slot_index
                            + ACK_FORWARD_TO_REVERSE_SLOT_OFFSET as u64,
                        terminal: terminal_slot,
                    },
                ));

                state.slot_in_subpacket += 1;
                if terminal_slot {
                    state.awaiting_ack = true;
                    state.next_eligible_slot = slot_index + INTERLACE + HARQ_FEEDBACK_GRACE_SLOTS;
                } else {
                    // Multi-slot HRPD physical packets occupy one H-ARQ interlace:
                    // slots are spaced by 4, not transmitted contiguously.
                    state.next_eligible_slot = slot_index + INTERLACE;
                }
                done = state.current_subpacket >= state.subpacket_count;
            }
        }

        if done {
            self.active.remove(idx);
        }
        if let Some(bus) = self.harq_bus.as_ref() {
            for (mac, event) in emission_events {
                bus.publish_emission(mac, event);
            }
        }
        if retired_without_emit {
            self.record_slot_stats(
                slot_index,
                SlotStatsKind::Idle {
                    reason: IdleReason::RetiredWithoutEmit,
                },
            );
            return ForwardSlotOutput {
                channel: SlotKind::Idle,
                data_chips: Vec::new(),
                mac_bits,
            };
        }

        self.record_slot_stats(
            slot_index,
            SlotStatsKind::Traffic {
                drc: tx_drc,
                info_bits: tx_info_bits_per_slot,
            },
        );
        ForwardSlotOutput {
            channel: SlotKind::Traffic { active_mac },
            data_chips,
            mac_bits,
        }
    }

    fn promote_queue(&mut self, slot_index: u64) {
        let mut i = 0;
        while i < self.queue.len() {
            let mac = self.queue[i].mac_index;
            if self
                .active
                .iter()
                .filter(|s| s.packet.mac_index == mac)
                .count()
                >= MAX_OUTSTANDING_PACKETS_PER_MAC
            {
                i += 1;
                continue;
            }
            let mut pkt = self.queue.remove(i);
            // Transmit at the rate the AT requested for this packet's start
            // slot, per C.S0024-0 v4.0 §8.4.6.1.4.1.2. Higher layers do not
            // provide a DRC. If the governing DRC uses a different payload
            // size, rebuild recognized HRPD payloads before HARQ encoding.
            let latest_drc_record = self
                .harq_bus
                .as_ref()
                .and_then(|bus| bus.current_drc_record(mac));
            let latest_drc = latest_drc_record.map(|(_, drc)| drc);
            let exact_governing_drc = self.harq_bus.as_ref().and_then(|bus| {
                bus.governing_drc(
                    mac,
                    slot_index,
                    FORWARD_FRAME_OFFSET_SLOTS,
                    FORWARD_DRC_LENGTH_SLOTS,
                )
            });
            let governing_slot = governing_drc_slot(
                slot_index,
                FORWARD_FRAME_OFFSET_SLOTS,
                FORWARD_DRC_LENGTH_SLOTS,
            );
            // C.S0024-A §1.6.6.1.4.1.2 binds a packet start to the DRC
            // completed in one exact window. Reusing an older DRC during an
            // AT tune-away removes packets from the queue while the AT is not
            // listening and turns a short pause into a large RLP repair burst.
            let governing_drc = exact_governing_drc;
            let Some(at_drc) = governing_drc else {
                if slot_index.saturating_sub(self.last_no_governing_drc_log_slot) >= 128 {
                    log::debug!(
                        "rx_hrpd_traffic[m{}]: deferring forward traffic packet; no fresh DRC record start_slot={} governing_slot={} latest_drc={} latest_slot={} latest_age_slots={} payload_bits={}",
                        mac,
                        slot_index,
                        governing_slot,
                        latest_drc.map_or("none".to_string(), |d| format!("0x{d:x}")),
                        latest_drc_record
                            .map(|(slot, _)| slot.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        latest_drc_record
                            .and_then(|(slot, _)| slot_index.checked_sub(slot))
                            .map(|age| age.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        pkt.payload.len(),
                    );
                    self.last_no_governing_drc_log_slot = slot_index;
                }
                self.queue.insert(i, pkt);
                i += 1;
                continue;
            };
            let Some(at_rate) =
                implemented_forward_rate_by_drc(at_drc, pkt.forward_traffic_mac_subtype)
            else {
                log::warn!(
                    "rx_hrpd_traffic[m{}]: dropping forward traffic packet; no implemented scheduler rate payload_bits={} governing_drc={} ftc_mac_subtype=0x{:04x}",
                    mac,
                    pkt.payload.len(),
                    governing_drc.map_or("none".to_string(), |d| format!("0x{d:x}")),
                    pkt.forward_traffic_mac_subtype,
                );
                continue;
            };
            if slot_index % INTERLACE == 0 && at_rate.slots > 1 {
                self.queue.insert(i, pkt);
                i += 1;
                continue;
            }
            let rtc_ack_seq = rtc_ack_sequence_number(&pkt.payload);
            if at_rate.payload_bits as usize != pkt.payload.len() {
                if let Some(sequence_number) = rtc_ack_seq {
                    match default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
                        at_rate.payload_bits as usize,
                        sequence_number,
                        pkt.forward_traffic_mac_subtype,
                    ) {
                        Ok(payload) => {
                            pkt.payload = payload;
                        }
                        Err(err) => {
                            log::warn!(
                                "rx_hrpd_traffic[m{}]: failed to rebuild RTCAck for drc=0x{:x} payload_bits={}: {:?}",
                                mac,
                                at_rate.drc_index,
                                at_rate.payload_bits,
                                err
                            );
                            continue;
                        }
                    }
                } else if let Some(signaling) = default_signaling_packet(&pkt.payload) {
                    let rebuilt = default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
                        at_rate.payload_bits as usize,
                        signaling.protocol_type,
                        &signaling.payload,
                        signaling.reliable_sequence_number,
                        signaling.in_configuration,
                        signaling.ack_sequence_number,
                        pkt.forward_traffic_mac_subtype,
                    );
                    match rebuilt {
                        Ok(payload) => {
                            log::debug!(
                                "rx_hrpd_traffic[m{}]: rebuilt Stream0 signaling for governing drc=0x{:x} protocol=0x{:02x} seq={:?} ack_seq={:?} payload_bits={}",
                                mac,
                                at_rate.drc_index,
                                signaling.protocol_type,
                                signaling.reliable_sequence_number,
                                signaling.ack_sequence_number,
                                at_rate.payload_bits
                            );
                            pkt.payload = payload;
                        }
                        Err(err) => {
                            log::warn!(
                                "rx_hrpd_traffic[m{}]: failed to rebuild Stream0 signaling for drc=0x{:x} payload_bits={}: {:?}",
                                mac,
                                at_rate.drc_index,
                                at_rate.payload_bits,
                                err
                            );
                            continue;
                        }
                    }
                } else if let Some(mut payloads) = rebuild_or_split_format_b_ftc_payloads(
                    &pkt.payload,
                    at_rate.payload_bits as usize,
                    pkt.forward_traffic_mac_subtype,
                ) {
                    if payloads.len() > 1 {
                        log::trace!(
                            "rx_hrpd_traffic[m{}]: split Format-B FTC payload for governing drc=0x{:x} payload_bits={} packets={}",
                            mac,
                            at_rate.drc_index,
                            at_rate.payload_bits,
                            payloads.len()
                        );
                    } else {
                        log::trace!(
                            "rx_hrpd_traffic[m{}]: rebuilt Format-B FTC payload for governing drc=0x{:x} payload_bits={}",
                            mac,
                            at_rate.drc_index,
                            at_rate.payload_bits
                        );
                    }
                    pkt.payload = payloads.remove(0);
                    for (offset, payload) in payloads.into_iter().enumerate() {
                        let mut remainder = pkt.clone();
                        remainder.payload = payload;
                        self.queue.insert(i + offset, remainder);
                    }
                } else {
                    log::warn!(
                        "rx_hrpd_traffic[m{}]: dropping forward traffic packet; cannot rebuild payload_bits={} for governing drc=0x{:x} payload_bits={}",
                        mac,
                        pkt.payload.len(),
                        at_rate.drc_index,
                        at_rate.payload_bits
                    );
                    continue;
                }
            }
            let packet_id = self.next_packet_id;
            self.next_packet_id = self.next_packet_id.wrapping_add(1).max(1);
            let scheduled_rtc_ack_seq = rtc_ack_sequence_number(&pkt.payload);
            let scheduled_signaling = default_signaling_packet(&pkt.payload);
            let scheduled_priority_ranges = pkt
                .high_priority
                .then(|| format_default_packet_rlp_ranges(&pkt.payload));
            if let Some(state) = build_harq_state(pkt, at_rate, packet_id) {
                if let Some(ranges) = scheduled_priority_ranges {
                    log::debug!(
                        "rx_hrpd_traffic[m{}]: scheduling priority DefaultPacket RLP repair packet_id={} ranges=[{}] start_slot={} drc=0x{:x} payload_bits={} slots={} queue_remaining={}",
                        state.packet.mac_index,
                        packet_id,
                        ranges,
                        slot_index,
                        at_rate.drc_index,
                        at_rate.payload_bits,
                        at_rate.slots,
                        self.queue.len(),
                    );
                }
                if let Some(sequence_number) = scheduled_rtc_ack_seq {
                    log::info!(
                        "rx_hrpd_traffic[m{}]: scheduling RTCAck packet_id={} seq={} start_slot={} drc=0x{:x} payload_bits={} slots={} exact_drc={} latest_drc={}",
                        state.packet.mac_index,
                        packet_id,
                        sequence_number,
                        slot_index,
                        at_rate.drc_index,
                        at_rate.payload_bits,
                        at_rate.slots,
                        exact_governing_drc
                            .map(|drc| format!("0x{drc:x}"))
                            .unwrap_or_else(|| "none".to_string()),
                        latest_drc_record
                            .map(|(slot, drc)| format!("0x{drc:x}@{slot}"))
                            .unwrap_or_else(|| "none".to_string()),
                    );
                }
                if let Some(signaling) = scheduled_signaling.as_ref() {
                    let scheduling_line = format!(
                        "rx_hrpd_traffic[m{}]: scheduling FTC packet_id={} protocol=0x{:02x} in_config={} reliable_seq={:?} ack_seq={:?} start_slot={} drc=0x{:x} payload_bits={} slots={} exact_drc={} latest_drc={}",
                        state.packet.mac_index,
                        packet_id,
                        signaling.protocol_type,
                        signaling.in_configuration,
                        signaling.reliable_sequence_number,
                        signaling.ack_sequence_number,
                        slot_index,
                        at_rate.drc_index,
                        at_rate.payload_bits,
                        at_rate.slots,
                        exact_governing_drc
                            .map(|drc| format!("0x{drc:x}"))
                            .unwrap_or_else(|| "none".to_string()),
                        latest_drc_record
                            .map(|(slot, drc)| format!("0x{drc:x}@{slot}"))
                            .unwrap_or_else(|| "none".to_string()),
                    );
                    if is_default_packet_data_payload(signaling) {
                        log::trace!("{scheduling_line}");
                    } else {
                        log::debug!("{scheduling_line}");
                    }
                }
                self.active.push(state);
                return;
            }
            // Malformed packets are dropped silently.
        }
    }

    fn has_due_continuation(&self, slot_index: u64) -> bool {
        self.active.iter().any(|s| {
            !s.awaiting_ack && s.last_tx_slot.is_some() && slot_index >= s.next_eligible_slot
        })
    }

    fn retire_expired_feedback_waits(&mut self, slot_index: u64) {
        let mut i = 0;
        while i < self.active.len() {
            let expired =
                self.active[i].awaiting_ack && slot_index >= self.active[i].next_eligible_slot;
            if expired {
                let state = self.active.remove(i);
                self.stats_harq_no_response = self.stats_harq_no_response.saturating_add(1);
                if hrpd_harq_verbose()
                    && slot_index.saturating_sub(self.last_unknown_harq_log_slot) >= 128
                {
                    log::info!(
                        "HRPD forward scheduler: no H-ARQ response for mac={} packet_id={} subpacket={} by slot={}, retiring",
                        state.packet.mac_index,
                        state.packet_id,
                        state.current_subpacket,
                        slot_index
                    );
                    self.last_unknown_harq_log_slot = slot_index;
                }
            } else {
                i += 1;
            }
        }
    }

    fn pick_active(&self, slot_index: u64) -> Option<usize> {
        // A mid-flight packet's continuation owns its exact slot: the AT
        // combines the packet's slots at precisely 4-slot spacing, so a
        // started packet that is due always wins over starting a new one.
        if let Some(idx) = self.active.iter().position(|s| {
            !s.awaiting_ack && s.last_tx_slot.is_some() && slot_index >= s.next_eligible_slot
        }) {
            return Some(idx);
        }
        // Control Channel capsules are packet-based: the modulator passes
        // `is_control_slot` when a real Control Channel physical packet owns
        // this slot. Idle slots on the same 4-slot phase are available for
        // one-slot Forward Traffic packets. Multi-slot packets still avoid
        // this phase because their fixed 4-slot continuations could collide
        // with a later synchronous/asynchronous control capsule.
        let is_control_phase = slot_index % INTERLACE == 0;
        self.active.iter().position(|s| {
            s.last_tx_slot.is_none()
                && slot_index >= s.next_eligible_slot
                && (!is_control_phase || s.slots_per_subpacket() == 1)
        })
    }

    fn idle_reason(&self, slot_index: u64) -> IdleReason {
        if self.queue.is_empty() {
            if self
                .active
                .iter()
                .any(|state| !state.awaiting_ack && state.next_eligible_slot > slot_index)
            {
                return IdleReason::WaitingContinuation;
            }
            return IdleReason::NoWork;
        }

        let is_control_phase = slot_index % INTERLACE == 0;
        let mut saw_candidate = false;
        let mut saw_no_drc = false;
        let mut saw_control_phase = false;

        for pkt in &self.queue {
            let mac = pkt.mac_index;
            if self
                .active
                .iter()
                .filter(|state| state.packet.mac_index == mac)
                .count()
                >= MAX_OUTSTANDING_PACKETS_PER_MAC
            {
                continue;
            }
            saw_candidate = true;
            let Some(drc) = self.harq_bus.as_ref().and_then(|bus| {
                bus.governing_drc(
                    mac,
                    slot_index,
                    FORWARD_FRAME_OFFSET_SLOTS,
                    FORWARD_DRC_LENGTH_SLOTS,
                )
            }) else {
                saw_no_drc = true;
                continue;
            };
            let Some(rate) = implemented_forward_rate_by_drc(drc, pkt.forward_traffic_mac_subtype)
            else {
                continue;
            };
            if is_control_phase && rate.slots > 1 {
                saw_control_phase = true;
            }
        }

        if saw_no_drc {
            IdleReason::NoGoverningDrc
        } else if saw_control_phase {
            IdleReason::ControlPhase
        } else if !saw_candidate {
            IdleReason::MaxOutstanding
        } else if self.active.iter().any(|state| state.awaiting_ack) {
            IdleReason::AwaitingHarq
        } else {
            IdleReason::QueuedBlocked
        }
    }

    fn record_slot_stats(&mut self, slot_index: u64, kind: SlotStatsKind) {
        let start_slot = match self.stats_window_start_slot {
            Some(start_slot) => start_slot,
            None => {
                self.reset_slot_stats(slot_index);
                slot_index
            }
        };
        if slot_index.saturating_sub(start_slot) >= SCHEDULER_STATS_WINDOW_SLOTS {
            self.log_slot_stats(slot_index, start_slot);
            self.reset_slot_stats(slot_index);
        }

        match kind {
            SlotStatsKind::Traffic { drc, info_bits } => {
                self.stats_traffic_slots = self.stats_traffic_slots.saturating_add(1);
                if let Some(entry) = self.stats_drc.get_mut(drc as usize) {
                    *entry = entry.saturating_add(1);
                }
                self.stats_info_bits = self.stats_info_bits.saturating_add(info_bits);
                let phase = (slot_index % INTERLACE) as usize;
                if let Some(entry) = self.stats_phase.get_mut(phase) {
                    *entry = entry.saturating_add(1);
                }
            }
            SlotStatsKind::Control => {
                self.stats_control_slots = self.stats_control_slots.saturating_add(1);
            }
            SlotStatsKind::Idle { reason } => {
                self.stats_idle_slots = self.stats_idle_slots.saturating_add(1);
                self.stats_idle_reasons[reason as usize] =
                    self.stats_idle_reasons[reason as usize].saturating_add(1);
            }
        }
    }

    fn reset_slot_stats(&mut self, start_slot: u64) {
        self.stats_window_start_slot = Some(start_slot);
        self.stats_traffic_slots = 0;
        self.stats_control_slots = 0;
        self.stats_idle_slots = 0;
        self.stats_harq_no_response = 0;
        self.stats_info_bits = 0;
        self.stats_drc = [0; 16];
        self.stats_phase = [0; 4];
        self.stats_idle_reasons = [0; IdleReason::COUNT];
    }

    fn log_slot_stats(&self, slot_index: u64, start_slot: u64) {
        let window_slots = slot_index.saturating_sub(start_slot).max(1);
        let drc = self
            .stats_drc
            .iter()
            .enumerate()
            .filter(|(_, count)| **count != 0)
            .map(|(idx, count)| format!("0x{idx:x}:{count}"))
            .collect::<Vec<_>>()
            .join(",");
        let phase = self
            .stats_phase
            .iter()
            .enumerate()
            .map(|(idx, count)| format!("{idx}:{count}"))
            .collect::<Vec<_>>()
            .join(",");
        let phy_kbps = self.stats_info_bits as f64 / (window_slots as f64 / 600.0) / 1000.0;
        let idle_reasons = IdleReason::ALL
            .iter()
            .filter_map(|reason| {
                let count = self.stats_idle_reasons[*reason as usize];
                (count != 0).then(|| format!("{}:{count}", reason.label()))
            })
            .collect::<Vec<_>>()
            .join(",");
        log::debug!(
            "HRPD forward scheduler stats: slot={} window_slots={} traffic={} control={} idle={} phy_kbps={:.1} drc=[{}] phase=[{}] idle_reason=[{}] queued={} active={} harq_no_response={}",
            slot_index,
            window_slots,
            self.stats_traffic_slots,
            self.stats_control_slots,
            self.stats_idle_slots,
            phy_kbps,
            drc,
            phase,
            idle_reasons,
            self.queue.len(),
            self.active.len(),
            self.stats_harq_no_response
        );
    }
}

fn is_default_packet_data_payload(signaling: &DefaultSignalingPacket) -> bool {
    matches!(
        signaling.protocol_type,
        DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE
            | DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE
            | DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE
    ) && !signaling.in_configuration
        && signaling.reliable_sequence_number.is_none()
        && signaling.ack_sequence_number.is_none()
}

fn format_default_packet_rlp_ranges(payload: &[u8]) -> String {
    let Some(session_packets) = forward_format_b_session_packets(payload) else {
        return "unparsed".to_string();
    };
    let ranges = session_packets
        .iter()
        .filter_map(|packet| parse_stream_layer_packet_bytes(packet).ok())
        .filter_map(|stream| {
            parse_default_packet_rlp_packet_bits(&stream.application_packet_bits).ok()
        })
        .map(|rlp| format!("{}+{}", rlp.sequence, rlp.payload.len()))
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        "unparsed".to_string()
    } else {
        ranges.join(",")
    }
}

#[derive(Clone, Copy)]
enum SlotStatsKind {
    Traffic { drc: u8, info_bits: u64 },
    Control,
    Idle { reason: IdleReason },
}

#[derive(Clone, Copy)]
enum IdleReason {
    NoWork = 0,
    NoGoverningDrc = 1,
    MaxOutstanding = 2,
    ControlPhase = 3,
    AwaitingHarq = 4,
    WaitingContinuation = 5,
    QueuedBlocked = 6,
    RetiredWithoutEmit = 7,
}

impl IdleReason {
    const COUNT: usize = 8;
    const ALL: [Self; Self::COUNT] = [
        Self::NoWork,
        Self::NoGoverningDrc,
        Self::MaxOutstanding,
        Self::ControlPhase,
        Self::AwaitingHarq,
        Self::WaitingContinuation,
        Self::QueuedBlocked,
        Self::RetiredWithoutEmit,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::NoWork => "no_work",
            Self::NoGoverningDrc => "no_drc",
            Self::MaxOutstanding => "max_outstanding",
            Self::ControlPhase => "control_phase",
            Self::AwaitingHarq => "awaiting_harq",
            Self::WaitingContinuation => "waiting_continuation",
            Self::QueuedBlocked => "queued_blocked",
            Self::RetiredWithoutEmit => "retired",
        }
    }
}

/// Default MAC bits emitted per slot.
//
// Real RPC/RA encoding is not wired in here (C.S0024-0 v4.0 §9.3.1.2).
fn default_mac_bits() -> Vec<u8> {
    vec![0u8; MAC_BITS_PER_SLOT]
}

fn governing_drc_slot(start_slot: u64, frame_offset: u64, drc_length: u64) -> u64 {
    let drc_length = drc_length.max(1);
    start_slot
        .saturating_sub(1)
        .saturating_sub((start_slot.saturating_sub(frame_offset)) % drc_length)
}

fn rtc_ack_sequence_number(payload: &[u8]) -> Option<u8> {
    let session_packets = forward_format_b_session_packets(payload)?;
    let packet = session_packets
        .iter()
        .find(|packet| packet.len() == 4 && reliable_rtc_ack_sequence_number(packet).is_some())?;
    reliable_rtc_ack_sequence_number(packet)
}

fn is_rtc_mac_grant(payload: &[u8]) -> bool {
    default_signaling_packet(payload)
        .as_ref()
        .is_some_and(is_rtc_mac_grant_signaling)
}

fn is_rtc_mac_grant_signaling(signaling: &DefaultSignalingPacket) -> bool {
    signaling.protocol_type == DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE
        && signaling.payload.first() == Some(&REVERSE_TRAFFIC_CHANNEL_MAC_GRANT_MESSAGE_ID)
}

fn bytes_to_bits(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for shift in (0..8).rev() {
            out.push((byte >> shift) & 1);
        }
    }
    out
}

fn pack_bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, bit| (acc << 1) | (bit & 1)))
        .collect()
}

fn read_bits(bits: &[u8], offset: &mut usize, width: usize) -> Option<u32> {
    if *offset + width > bits.len() {
        return None;
    }
    let mut value = 0u32;
    for bit in &bits[*offset..*offset + width] {
        value = (value << 1) | u32::from(bit & 1);
    }
    *offset += width;
    Some(value)
}

fn implemented_forward_rate_by_drc(
    drc_index: u8,
    forward_traffic_mac_subtype: u16,
) -> Option<ForwardRate> {
    let rate = *by_drc(drc_index)?;
    if implemented_forward_traffic_payload_bits_for_drc_for_mac_subtype(
        drc_index,
        forward_traffic_mac_subtype,
    ) == Some(rate.payload_bits as usize)
    {
        Some(rate)
    } else {
        None
    }
}

fn build_harq_state(
    pkt: ForwardTrafficPacket,
    rate: ForwardRate,
    packet_id: u64,
) -> Option<HarqState> {
    if implemented_forward_traffic_payload_bits_for_drc_for_mac_subtype(
        rate.drc_index,
        pkt.forward_traffic_mac_subtype,
    ) != Some(rate.payload_bits as usize)
    {
        log::warn!(
            "HRPD forward scheduler: dropping unsupported live FTC DRC mac={} drc=0x{:x} payload_bits={} ftc_mac_subtype=0x{:04x}",
            pkt.mac_index,
            rate.drc_index,
            rate.payload_bits,
            pkt.forward_traffic_mac_subtype
        );
        return None;
    }
    if pkt.payload.len() != rate.payload_bits as usize {
        return None;
    }
    let encoder = HrpdTurboEncoder::new(rate.payload_bits)?;
    let mut coded = encoder.encode(&pkt.payload, rate.code_rate_num, rate.code_rate_den);

    // C.S0024-0 v4.0 §9.3.1.3.2.3.3/4: scramble the turbo encoder output
    // first, then perform the forward Traffic channel symbol reordering and
    // permutation. Reversing these steps changes the bit seen by the QPSK
    // mapper and makes the packet undecodable by an AT.
    let mut scrambler = forward_traffic_scrambler(pkt.physical_layer_subtype, pkt.mac_index, rate);
    scrambler.apply_bits(&mut coded);

    let bits = forward_channel_interleave(rate.payload_bits as usize, rate.code_rate_den, &coded);

    let data_chips = build_packet_data_chips(&bits, rate);
    let packet_chips =
        build_packet_tdm_chips(pkt.mac_index, pkt.physical_layer_subtype, rate, &data_chips);

    // One transmission unit per packet: Rev 0 transmits every slot of the
    // physical packet back-to-back on its interlace (early termination on a
    // decoded ACK is the only legal interruption, and SLP handles loss).
    let subpacket_count = 1u8;

    Some(HarqState {
        packet_id,
        packet: pkt,
        packet_chips,
        subpacket_count,
        current_subpacket: 0,
        awaiting_ack: false,
        next_eligible_slot: 0,
        last_tx_slot: None,
        first_tx_slot: None,
        slot_in_subpacket: 0,
        rate,
    })
}

/// Build the Data-region chip stream (1600 modulation symbols) for one
/// Traffic slot of the current subpacket of `state`.
///
/// The packet chip stream already includes the TDM preamble and sequence
/// repetition over the Data portion. The scheduler slices that stream across
/// the packet's slots.
fn build_subpacket_slot_chips(state: &HarqState) -> Vec<Complex32> {
    let chips_per_slot = DATA_CHIPS_PER_SLOT;
    if state.packet_chips.is_empty() {
        return vec![Complex32::new(0.0, 0.0); chips_per_slot];
    }

    let packet_slot = state.current_subpacket as usize * state.slots_per_subpacket() as usize
        + state.slot_in_subpacket as usize;
    let start = packet_slot * chips_per_slot;
    let end = start + chips_per_slot;
    if end <= state.packet_chips.len() {
        return state.packet_chips[start..end].to_vec();
    }

    let mut out = Vec::with_capacity(chips_per_slot);
    out.extend_from_slice(&state.packet_chips[start.min(state.packet_chips.len())..]);
    out.resize(chips_per_slot, Complex32::new(0.0, 0.0));
    out
}

fn build_packet_data_chips(bits: &[u8], rate: ForwardRate) -> Vec<Complex32> {
    let mapped = match rate.modulation {
        HrpdModulation::Qpsk => map_qpsk(bits),
        HrpdModulation::Psk8 => map_8psk(bits),
        HrpdModulation::Qam16 => map_16qam(bits),
    };
    let total_packet_chips = rate.slots as usize * DATA_CHIPS_PER_SLOT;
    let data_chip_count = total_packet_chips.saturating_sub(rate.preamble_chips as usize);
    let repeated = repeat_chips(&mapped, data_chip_count);
    walsh16_cover_symbols(&repeated)
}

fn build_packet_tdm_chips(
    mac_index: u8,
    physical_layer_subtype: u16,
    rate: ForwardRate,
    data_chips: &[Complex32],
) -> Vec<Complex32> {
    let total_packet_chips = rate.slots as usize * DATA_CHIPS_PER_SLOT;
    let mut out = Vec::with_capacity(total_packet_chips);
    out.extend(traffic_preamble_chips(
        mac_index,
        physical_layer_subtype,
        rate.preamble_chips as usize,
    ));
    out.extend_from_slice(data_chips);
    out.resize(total_packet_chips, Complex32::new(0.0, 0.0));
    out
}

fn traffic_preamble_chips(
    mac_index: u8,
    physical_layer_subtype: u16,
    preamble_chips: usize,
) -> Vec<Complex32> {
    let cover_len = match physical_layer_subtype {
        2 => 64,
        3.. => 128,
        _ => 32,
    };
    let row = usize::from(mac_index >> 1);
    let complement = (mac_index & 1) != 0;
    (0..preamble_chips)
        .map(|idx| {
            let mut sign = walsh_biorthogonal(row, idx % cover_len);
            if complement {
                sign = -sign;
            }
            Complex32::new(sign, 0.0)
        })
        .collect()
}

/// Passing `b=111`, `d=drc_index` is correct because the scheduler only
/// transmits canonical full-size formats (Table 13.3.1.3.2.3.3-1); short
/// and multi-user formats need their own b/d codes before they ship. For
/// MACIndex < 64 canonical formats the subtype-2 seed (with its
/// complemented r̄6) is bit-identical to the Rev 0 seed, so this selection
/// does not change the waveform for today's MACIndex 5..63 sessions.
fn forward_traffic_scrambler(
    physical_layer_subtype: u16,
    mac_index: u8,
    rate: ForwardRate,
) -> HrpdForwardScrambler {
    match physical_layer_subtype {
        2 => HrpdForwardScrambler::new_forward_subtype2(mac_index, 0b111, rate.drc_index),
        3.. => HrpdForwardScrambler::new_forward_subtype3_plus(mac_index, 0b111, rate.drc_index),
        _ => HrpdForwardScrambler::new_forward(mac_index, rate.drc_index),
    }
}

fn repeat_chips(chips: &[Complex32], len: usize) -> Vec<Complex32> {
    if chips.is_empty() {
        return vec![Complex32::new(0.0, 0.0); len];
    }
    (0..len).map(|idx| chips[idx % chips.len()]).collect()
}

fn walsh16_cover_symbols(symbols: &[Complex32]) -> Vec<Complex32> {
    let mut out = Vec::with_capacity(symbols.len());
    for group in symbols.chunks_exact(16) {
        for col in 0..16 {
            let mut chip = Complex32::new(0.0, 0.0);
            for (row, symbol) in group.iter().enumerate() {
                chip += *symbol * walsh_biorthogonal(row, col) * 0.25;
            }
            out.push(chip);
        }
    }
    out
}

fn walsh_biorthogonal(row: usize, col: usize) -> f32 {
    if ((row & col).count_ones() & 1) == 0 {
        1.0
    } else {
        -1.0
    }
}

// ---------------------------------------------------------------------------
// Modulation symbol mappers.
//
// All constellations are unit-energy (E_s = 1) so per-channel power
// normalization stays at the slot modulator. The bit tuple order and signal
// points follow C.S0024-0 v4.0 §9.3.1.3.2.3.5 tables 9.3.1.3.2.3.5.1-1,
// 9.3.1.3.2.3.5.2-1, and 9.3.1.3.2.3.5.3-1.
// ---------------------------------------------------------------------------

fn map_qpsk(bits: &[u8]) -> Vec<Complex32> {
    // Gray-coded QPSK: bit pair (b_i, b_q) → ((1 - 2*b_i)/sqrt(2),
    // (1 - 2*b_q)/sqrt(2)). Unit energy.
    let scale = 1.0_f32 / 2.0_f32.sqrt();
    bits.chunks_exact(2)
        .map(|c| {
            let i = if c[0] == 0 { scale } else { -scale };
            let q = if c[1] == 0 { scale } else { -scale };
            Complex32::new(i, q)
        })
        .collect()
}

fn map_8psk(bits: &[u8]) -> Vec<Complex32> {
    let cos_pi_8 = (std::f32::consts::FRAC_PI_8).cos();
    let sin_pi_8 = (std::f32::consts::FRAC_PI_8).sin();
    bits.chunks_exact(3)
        .map(|chunk| {
            // Input order is s0=x(3k), s1=x(3k+1), s2=x(3k+2). The spec
            // table is printed as s2,s1,s0.
            match ((chunk[2] & 1) << 2) | ((chunk[1] & 1) << 1) | (chunk[0] & 1) {
                0b000 => Complex32::new(cos_pi_8, sin_pi_8),
                0b001 => Complex32::new(sin_pi_8, cos_pi_8),
                0b011 => Complex32::new(-sin_pi_8, cos_pi_8),
                0b010 => Complex32::new(-cos_pi_8, sin_pi_8),
                0b110 => Complex32::new(-cos_pi_8, -sin_pi_8),
                0b111 => Complex32::new(-sin_pi_8, -cos_pi_8),
                0b101 => Complex32::new(sin_pi_8, -cos_pi_8),
                0b100 => Complex32::new(cos_pi_8, -sin_pi_8),
                _ => unreachable!("3-bit selector is 0..=7"),
            }
        })
        .collect()
}

fn map_16qam(bits: &[u8]) -> Vec<Complex32> {
    // Input order is s0=x(4k), s1=x(4k+1), s2=x(4k+2), s3=x(4k+3). The I
    // level is selected by s1,s0 and Q by s3,s2.
    let level = |msb: u8, lsb: u8| -> f32 {
        match ((msb & 1) << 1) | (lsb & 1) {
            0b00 => 3.0,
            0b01 => 1.0,
            0b11 => -1.0,
            0b10 => -3.0,
            _ => unreachable!("2-bit selector is 0..=3"),
        }
    };
    let scale = 1.0_f32 / 10.0_f32.sqrt();
    bits.chunks_exact(4)
        .map(|c| Complex32::new(level(c[1], c[0]) * scale, level(c[3], c[2]) * scale))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bts::hrpd::HarqFeedbackEvent;

    fn dummy_payload(bits: u32) -> Vec<u8> {
        (0..bits).map(|i| (i % 2) as u8).collect()
    }

    fn assert_complex_near(actual: Complex32, expected: Complex32) {
        assert!(
            (actual.re - expected.re).abs() < 1.0e-6,
            "real mismatch: actual={actual:?} expected={expected:?}"
        );
        assert!(
            (actual.im - expected.im).abs() < 1.0e-6,
            "imag mismatch: actual={actual:?} expected={expected:?}"
        );
    }

    /// Test-only DRC injection for exact-rate scheduler fixtures. Production
    /// code must learn this from the reverse-link DRC path through HarqBus,
    /// not from `ForwardTrafficPacket`.
    fn attach_test_drc(
        sched: &mut HrpdForwardScheduler,
        mac_index: u8,
        drc_index: u8,
    ) -> Arc<HarqBus> {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(mac_index, 0, drc_index);
        sched.set_harq_bus(bus.clone());
        bus
    }

    #[test]
    fn map_8psk_matches_c_s0024_table() {
        let cos_pi_8 = (std::f32::consts::FRAC_PI_8).cos();
        let sin_pi_8 = (std::f32::consts::FRAC_PI_8).sin();
        let cases = [
            ([0, 0, 0], Complex32::new(cos_pi_8, sin_pi_8)),
            ([1, 0, 0], Complex32::new(sin_pi_8, cos_pi_8)),
            ([1, 1, 0], Complex32::new(-sin_pi_8, cos_pi_8)),
            ([0, 1, 0], Complex32::new(-cos_pi_8, sin_pi_8)),
            ([0, 1, 1], Complex32::new(-cos_pi_8, -sin_pi_8)),
            ([1, 1, 1], Complex32::new(-sin_pi_8, -cos_pi_8)),
            ([1, 0, 1], Complex32::new(sin_pi_8, -cos_pi_8)),
            ([0, 0, 1], Complex32::new(cos_pi_8, -sin_pi_8)),
        ];
        for (bits, expected) in cases {
            assert_complex_near(map_8psk(&bits)[0], expected);
        }
    }

    #[test]
    fn map_16qam_matches_c_s0024_table() {
        let a = 1.0_f32 / 10.0_f32.sqrt();
        let cases = [
            ([0, 0, 0, 0], Complex32::new(3.0 * a, 3.0 * a)),
            ([1, 0, 0, 0], Complex32::new(a, 3.0 * a)),
            ([1, 1, 0, 0], Complex32::new(-a, 3.0 * a)),
            ([0, 1, 0, 0], Complex32::new(-3.0 * a, 3.0 * a)),
            ([0, 0, 1, 0], Complex32::new(3.0 * a, a)),
            ([1, 1, 1, 1], Complex32::new(-a, -a)),
            ([0, 1, 0, 1], Complex32::new(-3.0 * a, -3.0 * a)),
        ];
        for (bits, expected) in cases {
            assert_complex_near(map_16qam(&bits)[0], expected);
        }
    }

    #[test]
    fn drc1_16_slot_packet_yields_16_traffic_slots() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 7, 0x1);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 7,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });

        let mut total_chips = 0usize;
        let mut traffic_slots = 0usize;
        let mut traffic_slot_indices = Vec::new();
        let mut slot_idx = 0u64;
        // Rev 0: all 16 slots transmit back-to-back at 4-slot interlace
        // spacing with no response wait in between. Multi-slot starts are
        // gated off the synchronous Control Channel phase until the scheduler
        // can reserve future control-capsule slots, so the run is 1, 5, ..., 61.
        while slot_idx < 128 && traffic_slots < 16 {
            let out = sched.next_slot(slot_idx, false);
            match out.channel {
                SlotKind::Traffic { active_mac } => {
                    assert_eq!(active_mac, 7);
                    assert_eq!(out.data_chips.len(), DATA_CHIPS_PER_SLOT);
                    assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);
                    total_chips += out.data_chips.len();
                    traffic_slots += 1;
                    traffic_slot_indices.push(slot_idx);
                }
                SlotKind::Idle => {}
                SlotKind::Control => panic!("did not request a control slot"),
            }
            slot_idx += 1;
        }
        assert_eq!(traffic_slots, 16);
        assert_eq!(total_chips, 16 * DATA_CHIPS_PER_SLOT);
        let expected: Vec<u64> = (0..16).map(|k| 1 + 4 * k).collect();
        assert_eq!(traffic_slot_indices, expected);
    }

    #[test]
    fn stream1_packet_payload_is_accepted_by_scheduler() {
        let payload = cdma_common::hrpd::traffic::default_packet_stream1_ftc_payload_bits(
            0,
            &[0x45, 0x00, 0x00, 0x14],
            1024,
        )
        .expect("spec-shaped Stream 1 FTC payload");
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 5, 0x1);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload,
        });

        // Slot 0 is on the control interlace phase, so packets start at 1.
        let out = sched.next_slot(0, false);
        assert_eq!(out.channel, SlotKind::Idle);
        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        assert_eq!(out.data_chips.len(), DATA_CHIPS_PER_SLOT);
        assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);
    }

    #[test]
    fn rlp_retransmissions_are_queued_ahead_of_first_transmissions() {
        let packet = |sequence: u32, high_priority: bool| ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority,
            payload: cdma_common::hrpd::traffic::default_packet_ftc_payload_bits_for_mac_subtype(
                cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_ID,
                sequence,
                &[sequence as u8],
                1024,
                1,
            )
            .unwrap(),
        };
        let mut sched = HrpdForwardScheduler::new();
        let first_a = packet(1, false);
        let first_b = packet(2, false);
        let repair_a = packet(3, true);
        let repair_b = packet(4, true);
        assert_eq!(format_default_packet_rlp_ranges(&repair_a.payload), "3+1");
        let expected = [
            repair_a.payload.clone(),
            repair_b.payload.clone(),
            first_a.payload.clone(),
            first_b.payload.clone(),
        ];

        sched.enqueue(first_a);
        sched.enqueue(first_b);
        sched.enqueue(repair_a);
        sched.enqueue(repair_b);

        assert_eq!(sched.queue.len(), expected.len());
        for (queued, expected_payload) in sched.queue.iter().zip(expected) {
            assert_eq!(queued.payload, expected_payload);
        }
    }

    #[test]
    fn drc6_packet_starts_with_mac_index_preamble() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 5, 0x6);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });

        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        assert_eq!(out.data_chips.len(), DATA_CHIPS_PER_SLOT);

        let rate = crate::phy::hrpd::rates::by_drc(0x6).unwrap();
        let expected = traffic_preamble_chips(5, 0, rate.preamble_chips as usize);
        assert_eq!(&out.data_chips[..expected.len()], expected.as_slice());
        assert_ne!(
            &out.data_chips[expected.len()..expected.len() + 32],
            &expected[..32],
            "data symbols must start after, not overwrite, the TDM preamble"
        );
    }

    #[test]
    fn drcd_subtype2_5120_packet_emits_two_interlace_slots() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 5, 0x0d);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload: dummy_payload(5120),
        });

        for s in [1u64, 5] {
            let out = sched.next_slot(s, false);
            assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
            assert_eq!(out.data_chips.len(), DATA_CHIPS_PER_SLOT);
            assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);
        }
        let out = sched.next_slot(9, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn drce_subtype2_5120_packet_emits_one_slot() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 6, 0x0e);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 6,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload: dummy_payload(5120),
        });

        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 6 });
        assert_eq!(out.data_chips.len(), DATA_CHIPS_PER_SLOT);
        assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);

        let out = sched.next_slot(5, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn enqueue_coalesces_duplicate_rtc_ack_for_same_mac_and_sequence() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 5, 0x1);
        let rtc_ack0 =
            default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(1024, 0, 0)
                .expect("test RTCAck payload");
        let rtc_ack1 =
            default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(1024, 1, 0)
                .expect("test RTCAck payload");

        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: rtc_ack0.clone(),
        });
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: rtc_ack0.clone(),
        });
        assert_eq!(sched.queue.len(), 1);

        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        assert_eq!(sched.queue.len(), 0);
        assert_eq!(sched.active.len(), 1);

        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: rtc_ack0,
        });
        assert_eq!(
            sched.queue.len(),
            0,
            "same RTCAck sequence should not be queued while active"
        );

        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: rtc_ack1,
        });
        assert_eq!(
            sched.queue.len(),
            1,
            "a different RTCAck sequence remains a distinct reliable SLP message"
        );
    }

    #[test]
    fn enqueue_prioritizes_stream0_and_coalesces_stale_rtc_mac_grants() {
        let mut sched = HrpdForwardScheduler::new();
        let data = cdma_common::hrpd::traffic::default_packet_ftc_payload_bits_for_mac_subtype(
            cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_ID,
            0,
            &[0x45, 0x00, 0x00, 0x14],
            5120,
            cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .expect("test DefaultPacket payload");
        let grant = |bucket_level| {
            cdma_common::hrpd::traffic::default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
                5120,
                &[cdma_common::hrpd::traffic::MacFlowGrant {
                    mac_flow_id: 1,
                    t2p_inflow: 0x50,
                    bucket_level,
                    tt2p_hold: 0x0f,
                }],
                cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
            )
            .expect("test RTCMAC Grant payload")
        };
        let first_grant = grant(0x50);
        let refreshed_grant = grant(0x6c);

        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload: data.clone(),
        });
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload: first_grant,
        });

        assert_eq!(sched.queue.len(), 2);
        assert!(is_rtc_mac_grant(&sched.queue[0].payload));
        assert_eq!(sched.queue[1].payload, data);

        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload: refreshed_grant.clone(),
        });

        assert_eq!(sched.queue.len(), 2);
        assert_eq!(sched.queue[0].payload, refreshed_grant);
        assert_eq!(sched.queue[1].payload, data);
    }

    #[test]
    fn control_slot_yields_empty_data_region() {
        let mut sched = HrpdForwardScheduler::new();
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 3,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        let out = sched.next_slot(0, true);
        assert_eq!(out.channel, SlotKind::Control);
        assert!(out.data_chips.is_empty());
        assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);
    }

    #[test]
    fn one_slot_packet_can_use_idle_control_phase() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 5, 0x0c);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(4096),
        });

        let out = sched.next_slot(0, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        assert_eq!(out.data_chips.len(), DATA_CHIPS_PER_SLOT);
        assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);
    }

    #[test]
    fn empty_queue_is_idle() {
        let mut sched = HrpdForwardScheduler::new();
        let out = sched.next_slot(0, false);
        assert_eq!(out.channel, SlotKind::Idle);
        assert!(out.data_chips.is_empty());
        assert_eq!(out.mac_bits.len(), MAC_BITS_PER_SLOT);
    }

    #[test]
    fn nak_retires_packet() {
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 1, 0x3);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 1,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });

        // 4-slot packet transmits contiguously on its interlace: 1, 5, 9, 13.
        for s in [1u64, 5, 9, 13] {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Traffic { .. }), "slot {s}");
        }
        // NAK: the AT detected but failed to decode. Rev 0 has no PHY
        // retransmission, so the packet retires (SLP resends the payload).
        sched.handle_ack(1, 1, 0, HarqResponse::Nak);
        for s in [17u64, 21, 25] {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Idle), "slot {s}");
        }
    }

    #[test]
    fn packet_slots_are_contiguous_on_interlace_without_feedback() {
        // No reverse ACK decoder feedback at all: the whole 16-slot packet
        // still transmits back-to-back on its interlace. Rev 0 ATs combine
        // the packet's slots at exactly 4-slot spacing; pausing mid-packet
        // for a response would make the packet undecodable.
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 5, 0x1);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });

        let mut first_chips: Vec<Complex32> = Vec::new();
        for k in 0..16u64 {
            let slot = 1 + 4 * k;
            let out = sched.next_slot(slot, false);
            assert_eq!(
                out.channel,
                SlotKind::Traffic { active_mac: 5 },
                "slot {slot}"
            );
            if k == 0 {
                first_chips = out.data_chips.clone();
            } else {
                assert_ne!(out.data_chips, first_chips, "slot {slot} repeats slot 1");
            }
            // In-between slots stay idle for this single in-flight packet.
            for off in 1..4u64 {
                let out = sched.next_slot(slot + off, false);
                assert_eq!(out.channel, SlotKind::Idle, "slot {}", slot + off);
            }
        }
    }

    #[test]
    fn unknown_harq_retires_packet_after_response_window() {
        // For DRC 0x6 (1 slot) the packet should retire after the response
        // window elapses without an ACK, freeing the MAC index for the next
        // forward traffic packet.
        let mut sched = HrpdForwardScheduler::new();
        attach_test_drc(&mut sched, 7, 0x6);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 7,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 7 });
        // ACK window passes with no response. Packet retires.
        for s in 2..16 {
            let out = sched.next_slot(s, false);
            assert_eq!(out.channel, SlotKind::Idle, "slot {s}");
        }
    }

    #[test]
    fn qpsk_consumes_two_bits_per_chip() {
        let bits = vec![0, 0, 0, 1, 1, 0, 1, 1];
        let chips = map_qpsk(&bits);
        assert_eq!(chips.len(), 4);
        assert!((chips[0].norm() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn psk8_consumes_three_bits_per_chip() {
        let bits = vec![0, 0, 0, 0, 0, 1, 1, 1, 1];
        let chips = map_8psk(&bits);
        assert_eq!(chips.len(), 3);
        for c in &chips {
            assert!((c.norm() - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn qam16_consumes_four_bits_per_chip() {
        let bits = vec![0, 0, 0, 0, 1, 1, 1, 1];
        let chips = map_16qam(&bits);
        assert_eq!(chips.len(), 2);
        let expected_outer = (18.0_f32 / 10.0_f32).sqrt();
        assert!((chips[0].norm() - expected_outer).abs() < 1e-6);
    }

    #[test]
    fn purge_mac_clears_active_queue_and_harq_bus_state() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x1);
        bus.set_current_drc_at_slot(6, 0, 0x1);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 6,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        bus.publish_feedback(
            5,
            HarqFeedbackEvent {
                packet_id: 1,
                subpacket: 0,
                response: HarqResponse::Ack,
            },
        );
        bus.set_current_drc_at_slot(5, 10, 0xb);
        assert_eq!(bus.current_drc(5), Some(0xb));

        let (queued, active, emissions, feedback) = sched.purge_mac(5);

        assert_eq!(queued, 1);
        assert_eq!(active, 1);
        assert_eq!(emissions, 1);
        assert_eq!(feedback, 1);
        assert!(sched.queue.iter().all(|pkt| pkt.mac_index != 5));
        assert!(sched.active.iter().all(|state| state.packet.mac_index != 5));
        assert!(bus.drain_emissions(5).is_empty());
        assert!(bus.drain_feedback(5).is_empty());
        assert_eq!(bus.current_drc(5), None);
    }

    #[test]
    fn scheduler_publishes_emission_for_each_packet_slot() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(4, 0, 0x1);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 4,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        let mut seen_at = Vec::new();
        for k in 0..16u64 {
            let s = 1 + 4 * k;
            let _ = sched.next_slot(s, false);
            for ev in bus.drain_emissions(4) {
                seen_at.push((s, ev));
            }
        }
        assert_eq!(seen_at.len(), 16, "one emission per traffic slot");
        for (idx, (slot, ev)) in seen_at.iter().enumerate() {
            let expected_slot = 1 + 4 * idx as u64;
            assert_eq!(*slot, expected_slot);
            assert_eq!(ev.subpacket, 0);
            assert_eq!(
                ev.expected_ack_reverse_slot,
                expected_slot + ACK_FORWARD_TO_REVERSE_SLOT_OFFSET as u64
            );
            assert_eq!(ev.terminal, idx == 15);
        }
    }

    #[test]
    fn scheduler_consumes_feedback_ack_completes_packet() {
        use crate::bts::hrpd::harq_bus::HarqFeedbackEvent;
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(6, 0, 0x3);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 6,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        for s in [1u64, 5, 9, 13] {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Traffic { active_mac: 6 }));
        }
        // Publish a decoded ACK via the bus: the packet completes and the
        // MAC index frees up.
        bus.publish_feedback(
            6,
            HarqFeedbackEvent {
                packet_id: 1,
                subpacket: 0,
                response: HarqResponse::Ack,
            },
        );
        for s in [17u64, 21, 25] {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Idle), "slot {s}");
        }
    }

    #[test]
    fn scheduler_consumes_early_ack_before_packet_terminal_slot() {
        use crate::bts::hrpd::harq_bus::HarqFeedbackEvent;
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x1);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });

        let out = sched.next_slot(1, false);
        assert!(matches!(out.channel, SlotKind::Traffic { active_mac: 5 }));
        bus.publish_feedback(
            5,
            HarqFeedbackEvent {
                packet_id: 1,
                subpacket: 0,
                response: HarqResponse::Ack,
            },
        );

        // The next due interlace slot would normally carry the second DRC1
        // continuation. An early ACK means the AT decoded the packet, so the
        // scheduler retires it instead of sending the rest of the packet.
        let out = sched.next_slot(5, false);
        assert_eq!(out.channel, SlotKind::Idle);
        for s in [9u64, 13, 17] {
            let out = sched.next_slot(s, false);
            assert_eq!(out.channel, SlotKind::Idle, "slot {s}");
        }
    }

    #[test]
    fn scheduler_consumes_feedback_nak_retires_packet() {
        use crate::bts::hrpd::harq_bus::HarqFeedbackEvent;
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(6, 0, 0x3);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 6,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        for s in [1u64, 5, 9, 13] {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Traffic { active_mac: 6 }));
        }
        bus.publish_feedback(
            6,
            HarqFeedbackEvent {
                packet_id: 1,
                subpacket: 0,
                response: HarqResponse::Nak,
            },
        );
        // NAK retires the packet (no PHY retransmission in Rev 0).
        for s in [17u64, 21, 25] {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Idle), "slot {s}");
        }
    }

    #[test]
    fn continuation_owns_its_slot_over_new_packet() {
        // MAC 5's in-flight packet owns its interlace slots; MAC 6's packet
        // enqueued mid-flight starts on a different phase instead of
        // stealing a continuation slot.
        let mut sched = HrpdForwardScheduler::new();
        let bus = attach_test_drc(&mut sched, 5, 0x3);
        bus.set_current_drc_at_slot(6, 0, 0x3);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });

        sched.enqueue(ForwardTrafficPacket {
            mac_index: 6,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        // MAC 6 starts on the next free non-control phase (slot 2)...
        let out = sched.next_slot(2, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 6 });
        // ...and both packets keep their own interlace phases.
        for k in 1..4u64 {
            let out = sched.next_slot(1 + 4 * k, false);
            assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
            let out = sched.next_slot(2 + 4 * k, false);
            assert_eq!(out.channel, SlotKind::Traffic { active_mac: 6 });
        }
    }

    #[test]
    fn current_drc_selects_rate_when_payload_matches() {
        // When the AT reports a different reverse DRC via the bus before
        // the scheduler promotes the queued packet, the encoded rate
        // follows the AT's report. DRC 0x1 (16 slots) and 0x6 (1 slot)
        // both use 1024-bit payloads -> binding is legal. Verify the packet
        // emits only ONE traffic slot (0x6's signature), not 16 (0x1's).
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        // DRC 0x6: 1 slot total → traffic on slot 1 only (slot 0 is on the
        // control interlace phase).
        let out = sched.next_slot(1, false);
        assert!(matches!(out.channel, SlotKind::Traffic { active_mac: 5 }));
        // Slot 2..=4 are within the response window (next_eligible_slot=5)
        // so the active packet doesn't emit, but it's still alive.
        for s in 2..=4 {
            let out = sched.next_slot(s, false);
            assert!(matches!(out.channel, SlotKind::Idle), "slot {s}");
        }
        // Drain the response window and the retire emit.
        for s in 5..=20 {
            let _ = sched.next_slot(s, false);
        }
        // DRC 0x1's 16-slot run would still be emitting interlace slots
        // here; with the override to 0x6 the queue is fully drained.
        let after = sched.next_slot(65, false);
        assert!(matches!(after.channel, SlotKind::Idle));
    }

    #[test]
    fn stale_governing_drc_defers_non_signaling_queued_packet() {
        // A DRC recorded long before the packet's start slot is past the
        // governing-DRC age window (§8.4.6.1.4.1.2), so the scheduler must
        // not transmit opaque packet data at the stale queued rate.
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        // Start far past the recorded DRC's slot (well beyond the
        // governing-DRC age window), so the queued packet waits.
        let start = 1201u64; // 1201 % 4 == 1, a non-control phase
        let out = sched.next_slot(start, false);
        assert_eq!(out.channel, SlotKind::Idle);

        bus.set_current_drc_at_slot(
            5,
            governing_drc_slot(
                start + 1,
                FORWARD_FRAME_OFFSET_SLOTS,
                FORWARD_DRC_LENGTH_SLOTS,
            ),
            0x6,
        );
        let out = sched.next_slot(start + 1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
    }

    #[test]
    fn queued_packets_bind_drc_at_actual_start_slot() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        bus.set_current_drc_at_slot(5, 7, 0xc);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        for _ in 0..7 {
            sched.enqueue(ForwardTrafficPacket {
                mac_index: 5,
                physical_layer_subtype: 0,
                forward_traffic_mac_subtype: 0,
                high_priority: false,
                payload: dummy_payload(1024),
            });
        }
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(4096),
        });

        for slot in 1..=7 {
            let out = sched.next_slot(slot, false);
            assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        }

        let out = sched.next_slot(8, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
        assert!(
            sched
                .active
                .iter()
                .any(|state| state.first_tx_slot == Some(8) && state.rate.drc_index == 0xc),
            "eighth packet must bind to the DRC governing slot 8, not the DRC from slot 1"
        );
    }

    #[test]
    fn stale_governing_drc_defers_stream0_signaling() {
        // C.S0024-0 v4.0 §8.4.6.1.4.1.2 applies to the Forward Traffic
        // Channel packet, regardless of whether the payload is Stream-0
        // signaling. Without a governing DRC, the scheduler must wait rather
        // than guess at the queued rate.
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        let payload = cdma_common::hrpd::traffic::default_signaling_ftc_payload_bits_with_ack(
            1024,
            0x0c,
            &[0x51, 0x19, 0x02, 0x00, 0x19],
            Some(2),
            true,
            Some(4),
        )
        .unwrap();
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload,
        });

        let start = 1201u64;
        let out = sched.next_slot(start, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn missed_governing_drc_window_defers_until_exact_record_arrives() {
        let payload = || {
            cdma_common::hrpd::traffic::enhanced_signaling_ftc_payload_bits_with_ack(
                4096,
                cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
                &[0x0b, 0x00],
                None,
                false,
                None,
            )
            .unwrap()
        };
        let start = 129u64; // 129 % 4 == 1, a non-control phase.

        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, start - 12, 0xc);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload: payload(),
        });
        let out = sched.next_slot(start, false);
        assert_eq!(out.channel, SlotKind::Idle);

        bus.set_current_drc_at_slot(
            5,
            governing_drc_slot(
                start + 1,
                FORWARD_FRAME_OFFSET_SLOTS,
                FORWARD_DRC_LENGTH_SLOTS,
            ),
            0xc,
        );
        let out = sched.next_slot(start + 1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
    }

    #[test]
    fn reliable_stream0_signaling_waits_for_exact_governing_drc() {
        let payload = cdma_common::hrpd::traffic::default_signaling_ftc_payload_bits_with_ack(
            4096,
            0x12,
            &[0x51, 0x19, 0x04, 0x00, 0x00, 0x00],
            Some(1),
            false,
            Some(0),
        )
        .unwrap();
        let start = 129u64;
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, start - 12, 0xc);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload,
        });

        let out = sched.next_slot(start, false);
        assert_eq!(out.channel, SlotKind::Idle);

        let exact_slot = governing_drc_slot(
            start + 1,
            FORWARD_FRAME_OFFSET_SLOTS,
            FORWARD_DRC_LENGTH_SLOTS,
        );
        bus.set_current_drc_at_slot(5, exact_slot, 0xc);
        let out = sched.next_slot(start + 1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
    }

    #[test]
    fn format_b_rebuild_preserves_rev0_multi_mac_payloads() {
        let p0 = [0x45, 0x00];
        let p1 = [0x00, 0x14];
        let p2 = [0xc0, 0x21];
        let p3 = [0x7e, 0x7e];
        let packets: [(u32, &[u8]); 4] = [
            (0, p0.as_slice()),
            (2, p1.as_slice()),
            (4, p2.as_slice()),
            (6, p3.as_slice()),
        ];
        let payload =
            cdma_common::hrpd::traffic::default_packet_ftc_payload_bits_many_for_mac_subtype(
                cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_ID,
                &packets,
                4096,
                cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
            )
            .unwrap();

        let parsed = forward_format_b_session_packets(&payload).unwrap();
        assert_eq!(parsed.len(), 4);

        let rebuilt = rebuild_or_split_format_b_ftc_payloads(
            &payload,
            4096,
            cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
        )
        .unwrap();
        assert_eq!(rebuilt.len(), 1);
        let reparsed = forward_format_b_session_packets(&rebuilt[0]).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn format_b_rebuild_splits_rev0_payload_when_governing_drc_shrinks() {
        let p0 = [0x45, 0x00];
        let p1 = [0x00, 0x14];
        let p2 = [0xc0, 0x21];
        let p3 = [0x7e, 0x7e];
        let packets: [(u32, &[u8]); 4] = [
            (0, p0.as_slice()),
            (2, p1.as_slice()),
            (4, p2.as_slice()),
            (6, p3.as_slice()),
        ];
        let payload =
            cdma_common::hrpd::traffic::default_packet_ftc_payload_bits_many_for_mac_subtype(
                cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_ID,
                &packets,
                4096,
                cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
            )
            .unwrap();
        let parsed = forward_format_b_session_packets(&payload).unwrap();

        let split = rebuild_or_split_format_b_ftc_payloads(
            &payload,
            1024,
            cdma_common::hrpd::traffic::FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
        )
        .unwrap();

        assert_eq!(split.len(), 4);
        let mut reparsed = Vec::new();
        for payload in split {
            assert_eq!(payload.len(), 1024);
            reparsed.extend(forward_format_b_session_packets(&payload).unwrap());
        }
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn old_drc_still_defers_default_packet_flow_control() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0xc);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        let payload = cdma_common::hrpd::traffic::enhanced_signaling_ftc_payload_bits_with_ack(
            4096,
            cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
            &[0x0b, 0x00],
            None,
            false,
            None,
        )
        .unwrap();
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload,
        });

        let out = sched.next_slot(1201, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn stale_governing_drc_defers_default_packet_rlp_payload() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0xc);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        let payload = cdma_common::hrpd::traffic::default_packet_ftc_payload_bits(
            cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_ID,
            0,
            &[0x45, 0x00],
            4096,
        )
        .unwrap();
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 2,
            forward_traffic_mac_subtype: 1,
            high_priority: false,
            payload,
        });

        let out = sched.next_slot(1201, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn stale_governing_drc_defers_rtc_ack() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        let payload =
            cdma_common::hrpd::traffic::default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(
                1024, 0,
            )
            .unwrap();
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload,
        });

        let out = sched.next_slot(1201, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn stream0_signaling_parser_preserves_piggyback_ack() {
        let payload = cdma_common::hrpd::traffic::default_signaling_ftc_payload_bits_with_ack(
            1024,
            0x12,
            &[0x04, 0x7a, 0x00, 0x00, 0x00],
            Some(0),
            true,
            Some(3),
        )
        .unwrap();
        let signaling = default_signaling_packet(&payload).expect("Stream0 signaling");
        assert_eq!(signaling.protocol_type, 0x12);
        assert_eq!(signaling.reliable_sequence_number, Some(0));
        assert_eq!(signaling.ack_sequence_number, Some(3));
        assert!(signaling.in_configuration);

        let rebuilt = cdma_common::hrpd::traffic::default_signaling_ftc_payload_bits_with_ack(
            3072,
            signaling.protocol_type,
            &signaling.payload,
            signaling.reliable_sequence_number,
            signaling.in_configuration,
            signaling.ack_sequence_number,
        )
        .unwrap();
        assert_eq!(rebuilt.len(), 3072);

        let reparsed = default_signaling_packet(&rebuilt).expect("rebuilt Stream0 signaling");
        assert_eq!(reparsed.protocol_type, signaling.protocol_type);
        assert_eq!(
            reparsed.reliable_sequence_number,
            signaling.reliable_sequence_number
        );
        assert_eq!(reparsed.ack_sequence_number, signaling.ack_sequence_number);
        assert_eq!(reparsed.payload, signaling.payload);
        assert_eq!(reparsed.in_configuration, signaling.in_configuration);
    }

    #[test]
    fn generic_same_size_packet_can_use_current_drc_when_allowed() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });

        let out = sched.next_slot(1, false);
        assert!(
            matches!(out.channel, SlotKind::Traffic { active_mac: 5 }),
            "DRC 0x6 should transmit a 1024-bit packet in one slot"
        );
        let out = sched.next_slot(2, false);
        assert!(
            matches!(out.channel, SlotKind::Idle),
            "same-size DRC override should retire after one slot"
        );
    }

    #[test]
    fn setup_packet_uses_current_same_size_drc() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x6);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload:
                cdma_common::hrpd::traffic::default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(
                    1024, 0,
                )
                .unwrap(),
        });

        let out = sched.next_slot(1, false);
        assert!(matches!(out.channel, SlotKind::Traffic { active_mac: 5 }));
        // DRC 0x6 is a one-slot packet. The setup packet must follow the
        // AT's governing DRC, so slot 5 is already idle/retired.
        let out = sched.next_slot(5, false);
        assert!(matches!(out.channel, SlotKind::Idle));
    }

    #[test]
    fn rtc_ack_rebuilds_when_current_drc_payload_mismatches() {
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x7);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload:
                cdma_common::hrpd::traffic::default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(
                    1024, 0,
                )
                .unwrap(),
        });

        for s in [1u64, 5] {
            let out = sched.next_slot(s, false);
            assert!(
                matches!(out.channel, SlotKind::Traffic { active_mac: 5 }),
                "slot {s} channel {:?}",
                out.channel
            );
        }

        let out = sched.next_slot(9, false);
        assert!(
            matches!(out.channel, SlotKind::Idle),
            "RTCAck should rebuild to the AT's 2-slot DRC 0x7 packet size"
        );
    }

    #[test]
    fn rtc_ack_waits_until_drc_governs_packet_slot() {
        let bus = Arc::new(HarqBus::new());
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus.clone());
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload:
                cdma_common::hrpd::traffic::default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(
                    1024, 0,
                )
                .unwrap(),
        });

        let start = 1u64;
        let out = sched.next_slot(start, false);
        assert_eq!(out.channel, SlotKind::Idle);

        bus.set_current_drc_at_slot(
            5,
            governing_drc_slot(
                start + 1,
                FORWARD_FRAME_OFFSET_SLOTS,
                FORWARD_DRC_LENGTH_SLOTS,
            ),
            0x7,
        );
        let out = sched.next_slot(start + 1, false);
        assert_eq!(out.channel, SlotKind::Traffic { active_mac: 5 });
    }

    #[test]
    fn current_drc_payload_mismatch_drops_opaque_packet() {
        // The AT reports DRC 0x5 (2048-bit payload) but the queued packet
        // has a 1024-bit opaque payload. Without a parser that can rebuild the
        // packet for the governing DRC, the scheduler must drop it instead of
        // falling back to a caller-selected rate.
        let bus = Arc::new(HarqBus::new());
        bus.set_current_drc_at_slot(7, 0, 0x5);
        let mut sched = HrpdForwardScheduler::new();
        sched.set_harq_bus(bus);
        sched.enqueue(ForwardTrafficPacket {
            mac_index: 7,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: dummy_payload(1024),
        });
        let out = sched.next_slot(1, false);
        assert_eq!(out.channel, SlotKind::Idle);
    }

    #[test]
    fn rate_chip_count_math_is_consistent() {
        // For each Rev 0 rate, the encoded modulation chip count is a
        // multiple of bits/symbol. Whether the per-packet chip count under-
        // or over-fills the per-slot Data-chip budget determines whether
        // the spec uses sequence repetition (low DRCs) or symbol selection
        // / incremental redundancy across H-ARQ retransmissions (high DRCs).
        for rate in crate::phy::hrpd::rates::FORWARD_RATES {
            let coded_bits = rate.payload_bits as usize * rate.code_rate_den as usize
                / rate.code_rate_num as usize;
            let bps = match rate.modulation {
                HrpdModulation::Qpsk => 2,
                HrpdModulation::Psk8 => 3,
                HrpdModulation::Qam16 => 4,
            };
            assert_eq!(coded_bits % bps, 0, "DRC 0x{:x}", rate.drc_index);
        }
    }
}
