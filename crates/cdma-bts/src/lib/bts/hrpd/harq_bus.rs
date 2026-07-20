//! Lock-free per-MAC H-ARQ event bus between the forward scheduler (running
//! on the BTS synth/TX thread) and the reverse traffic RX worker.
//!
//! Two unidirectional flows are needed:
//!
//! - **Emission**: when the scheduler ships a forward traffic slot, it
//!   publishes an [`HarqEmissionEvent`] so the RX worker knows in which
//!   reverse slot the AT is expected to transmit the corresponding ACK
//!   Channel bit. Per C.S0024-0 v4.0 §9.2.1.3.3.4, the AT acknowledges a
//!   forward physical-layer packet transmitted in slot `n` in reverse slot
//!   `n + 3`.
//!
//! - **Feedback**: when the RX worker decodes an ACK/NAK on a reverse slot it
//!   was expecting, it publishes an [`HarqFeedbackEvent`] so the scheduler
//!   can advance (`Ack`) or retransmit (`Nak`) the H-ARQ state for that
//!   subpacket on its next `next_slot` call.
//!
//! ## TX hot-path safety
//!
//! The scheduler runs on the BTS synth thread, which the project CLAUDE.md
//! forbids from blocking locks. Each per-MAC queue is a
//! `crossbeam_queue::ArrayQueue` and is therefore lock-free for both
//! producers and consumers. The bus stores one queue per MAC index 0..63
//! (HRPD Rev 0 caps MACIndex at 6 bits).
//!
//! ## Backpressure
//!
//! Each queue is bounded at [`HARQ_BUS_CAPACITY`]. When full, the producer
//! drops the oldest entry to make room (drop-oldest). The hot path must not
//! block — this is the only way to bound queue depth without dropping the
//! newest signal. The drop-oldest policy is benign because:
//!   - Stale emission events that never received feedback time out on the
//!     scheduler side via the existing `unknown_retx` path.
//!   - Stale feedback events refer to a subpacket the scheduler has already
//!     retired; `handle_ack` ignores them.

use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::scheduler::HarqResponse;

/// Maximum HRPD MAC index space. Subtype 2 Physical Layer MACIndex is a
/// 7-bit field per C.S0024-A v3.0 §13.3.1.3.2.2 (Rev 0 uses the low 64).
pub const MAC_INDEX_COUNT: usize = 128;

/// Bounded per-MAC queue depth. The scheduler can keep up to 64 completed
/// one-slot DRC 0xc packets waiting for their reverse ACK bits; leave room for
/// those spec-timed `n + 3` expectations plus RX worker frame latency.
pub const HARQ_BUS_CAPACITY: usize = 128;

/// One forward traffic-slot emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarqEmissionEvent {
    /// Scheduler-local identifier for the parent forward physical packet.
    pub packet_id: u64,
    /// Subpacket index within the parent physical-layer packet.
    pub subpacket: u8,
    /// First forward slot used by the parent physical packet.
    pub packet_start_slot: u64,
    /// Forward slot that caused this ACK expectation.
    pub forward_slot: u64,
    /// Reverse slot index (absolute, in slots from the chip-rate epoch) on
    /// which the AT is expected to transmit the ACK response. Equals
    /// `forward_slot + ACK_FORWARD_TO_REVERSE_SLOT_OFFSET`.
    pub expected_ack_reverse_slot: u64,
    /// True when this emission is for the final slot of the Rev 0 physical
    /// packet. Intermediate NAKs are expected while the AT is still combining
    /// a multi-slot packet; only ACKs and final-slot NAKs should close the
    /// scheduler state.
    pub terminal: bool,
}

/// One H-ARQ decision published by the RX worker after decoding the ACK
/// Channel for an expected reverse slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarqFeedbackEvent {
    /// Scheduler-local identifier copied from the emission event being
    /// acknowledged. Needed when multiple 1-slot packets are outstanding for
    /// the same MAC index.
    pub packet_id: u64,
    pub subpacket: u8,
    pub response: HarqResponse,
}

pub const RPC_RECORD_UNSET: u64 = u64::MAX;
const RPC_SCHEDULE_RING_SLOTS: usize = 256;
const DRC_HISTORY_RING_SLOTS: usize = 512;
const ARQ_SCHEDULE_RING_SLOTS: usize = 256;
const ARQ_RECORD_UNSET: u64 = u64::MAX;

/// Signal level for one forward ARQ channel bit (C.S0024-A §13.3.1.3.2.2.4).
/// `Off` is the OOK "transmit nothing" state used by ACK-oriented and
/// NAK-oriented on-off keying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArqLevel {
    Plus,
    Off,
    Minus,
}

impl ArqLevel {
    fn code(self) -> u64 {
        match self {
            ArqLevel::Plus => 0b01,
            ArqLevel::Off => 0b10,
            ArqLevel::Minus => 0b11,
        }
    }

    fn from_code(code: u64) -> Option<Self> {
        match code & 0b11 {
            0b01 => Some(ArqLevel::Plus),
            0b10 => Some(ArqLevel::Off),
            0b11 => Some(ArqLevel::Minus),
            _ => None,
        }
    }

    /// BPSK/OOK amplitude on the MAC channel phase.
    pub fn amplitude(self) -> f32 {
        match self {
            ArqLevel::Plus => 1.0,
            ArqLevel::Off => 0.0,
            ArqLevel::Minus => -1.0,
        }
    }
}

/// One slot's forward ARQ channel content for a MAC index: the H-ARQ or
/// L-ARQ level (transmitted on the same phase as RPC) and the P-ARQ level
/// (opposite phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArqSlot {
    pub h_or_l: ArqLevel,
    pub p: ArqLevel,
}

/// Sentinel packed value meaning "no DRC recorded for this MAC yet".
const DRC_RECORD_UNSET: u64 = u64::MAX;

pub struct HarqBus {
    emissions: [Arc<ArrayQueue<HarqEmissionEvent>>; MAC_INDEX_COUNT],
    feedback: [Arc<ArrayQueue<HarqFeedbackEvent>>; MAC_INDEX_COUNT],
    /// Per-MAC slot-tagged reverse DRC, written lock-free by the
    /// reverse-traffic DRC processor as the AT reports new rates, read by
    /// the forward scheduler when it starts a packet so the transmitted
    /// rate honors the DRC that governs the packet's start slot per
    /// C.S0024-0 v4.0 §8.4.6.1.4.1.2. The `u64` packs the absolute system-
    /// time slot the DRC was decoded for in the high 56 bits and the 4-bit
    /// DRC value in the low 8 bits. `DRC_RECORD_UNSET` until the first
    /// confirmed DRC arrives.
    current_drc: [AtomicU64; MAC_INDEX_COUNT],
    /// Per-MAC slot-tagged DRC history. Forward packet rate selection must
    /// use the DRC whose reception completed in the exact governing slot, not
    /// the freshest nearby DRC, because the AT only decodes a packet at the
    /// rate it requested for that slot.
    drc_history: Box<[[AtomicU64; DRC_HISTORY_RING_SLOTS]; MAC_INDEX_COUNT]>,
    /// Per-MAC absolute-slot reverse power-control decisions. 0 commands AT
    /// power up, 1 commands power down; missing slots leave the MAC encoder's
    /// install-time fallback in force. The ring keeps the TX hot path lock-free
    /// while allowing the RX worker to schedule decisions far enough ahead for
    /// the synth lookahead.
    scheduled_rpc: Box<[[AtomicU64; RPC_SCHEDULE_RING_SLOTS]; MAC_INDEX_COUNT]>,
    /// Per-MAC forward-MAC RPC lookup counters. These are diagnostic only; a
    /// high miss rate means the transmitted RPC slots are mostly using the
    /// assignment fallback instead of measured reverse-pilot decisions.
    rpc_lookup_hits: [AtomicU64; MAC_INDEX_COUNT],
    rpc_lookup_misses: [AtomicU64; MAC_INDEX_COUNT],
    /// Per-MAC absolute-slot forward ARQ channel levels scheduled by the
    /// reverse traffic RX worker after each sub-packet decode attempt
    /// (C.S0024-A §13.3.1.3.2.2.4: H/L-ARQ and P-ARQ in the three slots per
    /// interlace cycle where RPC/DRCLock are not transmitted).
    scheduled_arq: Box<[[AtomicU64; ARQ_SCHEDULE_RING_SLOTS]; MAC_INDEX_COUNT]>,
    /// ARQ deadline diagnostics. RX compares each scheduled response slot to
    /// the latest slot already requested by the TX MAC encoder; a late write
    /// can remain in the ring but can no longer reach the AT.
    latest_arq_tx_slot: [AtomicU64; MAC_INDEX_COUNT],
    arq_schedule_on_time: [AtomicU64; MAC_INDEX_COUNT],
    arq_schedule_late: [AtomicU64; MAC_INDEX_COUNT],
    arq_lookup_hits: [AtomicU64; MAC_INDEX_COUNT],
}

impl std::fmt::Debug for HarqBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarqBus")
            .field("mac_index_count", &MAC_INDEX_COUNT)
            .field("capacity", &HARQ_BUS_CAPACITY)
            .finish()
    }
}

impl Default for HarqBus {
    fn default() -> Self {
        Self::new()
    }
}

fn boxed_ring_array<const RING: usize>(init: u64) -> Box<[[AtomicU64; RING]; MAC_INDEX_COUNT]> {
    let rows: Vec<[AtomicU64; RING]> = (0..MAC_INDEX_COUNT)
        .map(|_| std::array::from_fn(|_| AtomicU64::new(init)))
        .collect();
    rows.into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("row count is MAC_INDEX_COUNT by construction"))
}

impl HarqBus {
    pub fn new() -> Self {
        // Cannot `[Arc::new(ArrayQueue::new(N)); 64]` because Arc is not
        // Copy. Build with a helper. `from_fn` is the idiomatic way.
        let emissions = std::array::from_fn(|_| Arc::new(ArrayQueue::new(HARQ_BUS_CAPACITY)));
        let feedback = std::array::from_fn(|_| Arc::new(ArrayQueue::new(HARQ_BUS_CAPACITY)));
        let current_drc = std::array::from_fn(|_| AtomicU64::new(DRC_RECORD_UNSET));
        let rpc_lookup_hits = std::array::from_fn(|_| AtomicU64::new(0));
        let rpc_lookup_misses = std::array::from_fn(|_| AtomicU64::new(0));
        let latest_arq_tx_slot = std::array::from_fn(|_| AtomicU64::new(0));
        let arq_schedule_on_time = std::array::from_fn(|_| AtomicU64::new(0));
        let arq_schedule_late = std::array::from_fn(|_| AtomicU64::new(0));
        let arq_lookup_hits = std::array::from_fn(|_| AtomicU64::new(0));
        // The three per-MAC rings are ~1 MB combined at 128 MAC indexes;
        // build them on the heap so construction never lands a full copy on
        // a caller's (or test thread's) stack.
        let drc_history = boxed_ring_array::<DRC_HISTORY_RING_SLOTS>(DRC_RECORD_UNSET);
        let scheduled_rpc = boxed_ring_array::<RPC_SCHEDULE_RING_SLOTS>(RPC_RECORD_UNSET);
        let scheduled_arq = boxed_ring_array::<ARQ_SCHEDULE_RING_SLOTS>(ARQ_RECORD_UNSET);
        Self {
            emissions,
            feedback,
            current_drc,
            drc_history,
            scheduled_rpc,
            rpc_lookup_hits,
            rpc_lookup_misses,
            scheduled_arq,
            latest_arq_tx_slot,
            arq_schedule_on_time,
            arq_schedule_late,
            arq_lookup_hits,
        }
    }

    /// Schedule the forward ARQ channel content for one absolute TX slot.
    /// Callers write each of the three response slots (m, m+1, m+2)
    /// individually; a later write to the same slot merges by overriding
    /// only the channel(s) not set to `Off`.
    pub fn schedule_arq_at_slot(&self, mac: u8, slot: u64, h_or_l: ArqLevel, p: ArqLevel) {
        let Some(ring) = self.scheduled_arq.get(mac as usize) else {
            return;
        };
        let latest_tx_slot = self.latest_arq_tx_slot[mac as usize].load(Ordering::Relaxed);
        let timing_counter = if latest_tx_slot != 0 && slot <= latest_tx_slot {
            &self.arq_schedule_late[mac as usize]
        } else {
            &self.arq_schedule_on_time[mac as usize]
        };
        timing_counter.fetch_add(1, Ordering::Relaxed);
        let idx = (slot as usize) % ARQ_SCHEDULE_RING_SLOTS;
        let cell = &ring[idx];
        loop {
            let current = cell.load(Ordering::Relaxed);
            let (mut h_code, mut p_code) = (h_or_l.code(), p.code());
            if current != ARQ_RECORD_UNSET && (current >> 4) == slot {
                let cur_h = ArqLevel::from_code(current >> 2);
                let cur_p = ArqLevel::from_code(current);
                if h_or_l == ArqLevel::Off {
                    if let Some(cur) = cur_h {
                        h_code = cur.code();
                    }
                }
                if p == ArqLevel::Off {
                    if let Some(cur) = cur_p {
                        p_code = cur.code();
                    }
                }
            }
            let packed = (slot << 4) | (h_code << 2) | p_code;
            match cell.compare_exchange_weak(current, packed, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }

    /// Read the forward ARQ channel content for one absolute TX slot, or
    /// `None` when nothing is scheduled (both channels stay off).
    pub fn arq_at_slot(&self, mac: u8, slot: u64) -> Option<ArqSlot> {
        let ring = self.scheduled_arq.get(mac as usize)?;
        self.latest_arq_tx_slot[mac as usize].fetch_max(slot, Ordering::Relaxed);
        let idx = (slot as usize) % ARQ_SCHEDULE_RING_SLOTS;
        let packed = ring[idx].load(Ordering::Relaxed);
        if packed == ARQ_RECORD_UNSET || (packed >> 4) != slot {
            return None;
        }
        self.arq_lookup_hits[mac as usize].fetch_add(1, Ordering::Relaxed);
        Some(ArqSlot {
            h_or_l: ArqLevel::from_code(packed >> 2)?,
            p: ArqLevel::from_code(packed)?,
        })
    }

    /// Cumulative `(scheduled_on_time, scheduled_late, tx_lookup_hits,
    /// latest_tx_slot)` for one reverse-link ARQ channel.
    pub fn arq_schedule_stats(&self, mac: u8) -> (u64, u64, u64, u64) {
        let idx = mac as usize;
        let Some(latest) = self.latest_arq_tx_slot.get(idx) else {
            return (0, 0, 0, 0);
        };
        (
            self.arq_schedule_on_time[idx].load(Ordering::Relaxed),
            self.arq_schedule_late[idx].load(Ordering::Relaxed),
            self.arq_lookup_hits[idx].load(Ordering::Relaxed),
            latest.load(Ordering::Relaxed),
        )
    }

    pub fn schedule_rpc_at_slot(&self, mac: u8, slot: u64, rpc_bit: u8) {
        let Some(ring) = self.scheduled_rpc.get(mac as usize) else {
            return;
        };
        let idx = (slot as usize) % RPC_SCHEDULE_RING_SLOTS;
        ring[idx].store((slot << 1) | u64::from(rpc_bit & 0x01), Ordering::Relaxed);
    }

    pub fn rpc_at_slot(&self, mac: u8, slot: u64) -> Option<u8> {
        let ring = self.scheduled_rpc.get(mac as usize)?;
        let idx = (slot as usize) % RPC_SCHEDULE_RING_SLOTS;
        let packed = ring[idx].load(Ordering::Relaxed);
        if packed == RPC_RECORD_UNSET || (packed >> 1) != slot {
            self.record_rpc_lookup(mac, false);
            return None;
        }
        self.record_rpc_lookup(mac, true);
        Some((packed & 0x01) as u8)
    }

    // Pure atomics: this runs on the TX synth thread, so no logging (the
    // logger takes a lock). The RX-side rpc_summary line reports the
    // counters via `rpc_lookup_stats`.
    fn record_rpc_lookup(&self, mac: u8, hit: bool) {
        let cells = if hit {
            &self.rpc_lookup_hits
        } else {
            &self.rpc_lookup_misses
        };
        if let Some(cell) = cells.get(mac as usize) {
            cell.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Cumulative TX-side RPC schedule lookups since install/purge:
    /// `(scheduled_hits, fallback_misses)`. A rising miss count means the TX
    /// thread substituted the alternating fallback bit for slots the RX side
    /// never scheduled in time.
    pub fn rpc_lookup_stats(&self, mac: u8) -> (u64, u64) {
        let hits = self
            .rpc_lookup_hits
            .get(mac as usize)
            .map_or(0, |cell| cell.load(Ordering::Relaxed));
        let misses = self
            .rpc_lookup_misses
            .get(mac as usize)
            .map_or(0, |cell| cell.load(Ordering::Relaxed));
        (hits, misses)
    }

    /// Record a confirmed reverse DRC index decoded for absolute system-time
    /// slot `slot`. Called by the reverse-traffic DRC processor. Spec-valid
    /// writer must filter null/reserved values against the negotiated
    /// forward-traffic rate table before storing them here.
    pub fn set_current_drc_at_slot(&self, mac: u8, slot: u64, drc_index: u8) {
        if let Some(cell) = self.current_drc.get(mac as usize) {
            let packed = (slot << 8) | u64::from(drc_index);
            loop {
                let current = cell.load(Ordering::Relaxed);
                if current != DRC_RECORD_UNSET && (current >> 8) > slot {
                    break;
                }
                match cell.compare_exchange_weak(
                    current,
                    packed,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            if let Some(ring) = self.drc_history.get(mac as usize) {
                let idx = (slot as usize) % DRC_HISTORY_RING_SLOTS;
                ring[idx].store(packed, Ordering::Relaxed);
            }
        }
    }

    /// Read the most recent confirmed reverse DRC index reported by the AT
    /// on `mac`, ignoring slot, or `None` if none observed yet.
    pub fn current_drc(&self, mac: u8) -> Option<u8> {
        self.current_drc_record(mac).map(|(_, drc)| drc)
    }

    /// Read the freshest confirmed reverse DRC and the absolute slot it was
    /// decoded for.
    pub fn current_drc_record(&self, mac: u8) -> Option<(u64, u8)> {
        let packed = self.current_drc.get(mac as usize)?.load(Ordering::Relaxed);
        if packed == DRC_RECORD_UNSET {
            return None;
        }
        Some((packed >> 8, (packed & 0xFF) as u8))
    }

    /// Read the confirmed reverse DRC whose reception completed in absolute
    /// system-time slot `slot`.
    pub fn drc_at_slot(&self, mac: u8, slot: u64) -> Option<u8> {
        let ring = self.drc_history.get(mac as usize)?;
        let idx = (slot as usize) % DRC_HISTORY_RING_SLOTS;
        let packed = ring[idx].load(Ordering::Relaxed);
        if packed == DRC_RECORD_UNSET || (packed >> 8) != slot {
            return None;
        }
        Some((packed & 0xFF) as u8)
    }

    /// Read the DRC governing a forward packet that begins at `start_slot`,
    /// per C.S0024-0 v4.0 §8.4.6.1.4.1.2: the rate is the one requested by the
    /// DRC whose reception completed in slot
    /// `start_slot - 1 - ((start_slot - frame_offset) mod drc_length)`.
    /// Returns `None` unless that exact slot has a confirmed DRC record.
    pub fn governing_drc(
        &self,
        mac: u8,
        start_slot: u64,
        frame_offset: u64,
        drc_length: u64,
    ) -> Option<u8> {
        let drc_length = drc_length.max(1);
        let governing_slot = start_slot
            .saturating_sub(1)
            .saturating_sub((start_slot.saturating_sub(frame_offset)) % drc_length);
        self.drc_at_slot(mac, governing_slot)
    }

    /// Publish a forward-subpacket emission for MAC index `mac`. Drops the
    /// oldest queued emission if the queue is full (see module docs).
    pub fn publish_emission(&self, mac: u8, event: HarqEmissionEvent) {
        let Some(q) = self.emissions.get(mac as usize) else {
            return;
        };
        while q.push(event).is_err() {
            let _ = q.pop();
        }
    }

    /// Drain all queued emission events for MAC index `mac`.
    pub fn drain_emissions(&self, mac: u8) -> Vec<HarqEmissionEvent> {
        let Some(q) = self.emissions.get(mac as usize) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Some(ev) = q.pop() {
            out.push(ev);
        }
        out
    }

    /// Publish a decoded ACK/NAK for MAC index `mac`. Drop-oldest on full.
    pub fn publish_feedback(&self, mac: u8, event: HarqFeedbackEvent) {
        let Some(q) = self.feedback.get(mac as usize) else {
            return;
        };
        while q.push(event).is_err() {
            let _ = q.pop();
        }
    }

    /// Drain all queued feedback events for MAC index `mac`.
    pub fn drain_feedback(&self, mac: u8) -> Vec<HarqFeedbackEvent> {
        let Some(q) = self.feedback.get(mac as usize) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Some(ev) = q.pop() {
            out.push(ev);
        }
        out
    }

    /// Drop queued emission, feedback, and DRC state for a released MAC index.
    /// This is used when an HRPD traffic assignment is torn down so a later
    /// connection that reuses the same MAC cannot inherit stale H-ARQ/DRC
    /// state.
    pub fn purge_mac_queues(&self, mac: u8) -> (usize, usize) {
        let emissions = self.drain_emissions(mac).len();
        let feedback = self.drain_feedback(mac).len();
        if let Some(cell) = self.current_drc.get(mac as usize) {
            cell.store(DRC_RECORD_UNSET, Ordering::Relaxed);
        }
        if let Some(ring) = self.scheduled_rpc.get(mac as usize) {
            for cell in ring {
                cell.store(RPC_RECORD_UNSET, Ordering::Relaxed);
            }
        }
        if let Some(cell) = self.rpc_lookup_hits.get(mac as usize) {
            cell.store(0, Ordering::Relaxed);
        }
        if let Some(cell) = self.rpc_lookup_misses.get(mac as usize) {
            cell.store(0, Ordering::Relaxed);
        }
        if let Some(ring) = self.scheduled_arq.get(mac as usize) {
            for cell in ring {
                cell.store(ARQ_RECORD_UNSET, Ordering::Relaxed);
            }
        }
        for counters in [
            &self.latest_arq_tx_slot,
            &self.arq_schedule_on_time,
            &self.arq_schedule_late,
            &self.arq_lookup_hits,
        ] {
            if let Some(cell) = counters.get(mac as usize) {
                cell.store(0, Ordering::Relaxed);
            }
        }
        (emissions, feedback)
    }

    /// Queue handle for direct emission consumption by per-MAC RX workers.
    /// The worker owns this `Arc` for its lifetime so it can drain without
    /// re-resolving by index every frame.
    pub fn emission_queue(&self, mac: u8) -> Option<Arc<ArrayQueue<HarqEmissionEvent>>> {
        self.emissions.get(mac as usize).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emission_round_trip() {
        let bus = HarqBus::new();
        bus.publish_emission(
            7,
            HarqEmissionEvent {
                packet_id: 10,
                subpacket: 0,
                packet_start_slot: 7,
                forward_slot: 8,
                expected_ack_reverse_slot: 11,
                terminal: false,
            },
        );
        bus.publish_emission(
            7,
            HarqEmissionEvent {
                packet_id: 11,
                subpacket: 1,
                packet_start_slot: 11,
                forward_slot: 12,
                expected_ack_reverse_slot: 15,
                terminal: true,
            },
        );
        let events = bus.drain_emissions(7);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].subpacket, 0);
        assert_eq!(events[1].subpacket, 1);
        // Drained queue is empty.
        assert!(bus.drain_emissions(7).is_empty());
    }

    #[test]
    fn feedback_round_trip() {
        let bus = HarqBus::new();
        bus.publish_feedback(
            2,
            HarqFeedbackEvent {
                packet_id: 1,
                subpacket: 0,
                response: HarqResponse::Ack,
            },
        );
        let events = bus.drain_feedback(2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].response, HarqResponse::Ack);
    }

    #[test]
    fn emission_drop_oldest_on_overflow() {
        let bus = HarqBus::new();
        // Fill to capacity + 2 to force two drops.
        for i in 0..(HARQ_BUS_CAPACITY + 2) {
            bus.publish_emission(
                3,
                HarqEmissionEvent {
                    packet_id: i as u64,
                    subpacket: (i & 0xff) as u8,
                    packet_start_slot: i as u64,
                    forward_slot: i as u64,
                    expected_ack_reverse_slot: i as u64,
                    terminal: i == HARQ_BUS_CAPACITY + 1,
                },
            );
        }
        let events = bus.drain_emissions(3);
        assert_eq!(events.len(), HARQ_BUS_CAPACITY);
        // The two oldest entries (subpackets 0 and 1) were dropped.
        assert_eq!(events[0].subpacket, 2);
        assert_eq!(
            events.last().unwrap().subpacket,
            (HARQ_BUS_CAPACITY + 1) as u8
        );
    }

    #[test]
    fn out_of_range_mac_is_ignored() {
        let bus = HarqBus::new();
        // 128 is out of range (subtype-2 MACIndex is 7 bits, 0..=127).
        bus.publish_emission(
            128,
            HarqEmissionEvent {
                packet_id: 0,
                subpacket: 0,
                packet_start_slot: 0,
                forward_slot: 0,
                expected_ack_reverse_slot: 0,
                terminal: true,
            },
        );
        assert!(bus.drain_emissions(128).is_empty());
        assert!(bus.emission_queue(128).is_none());

        // The subtype-2 MAC index space above the Rev 0 range is live.
        bus.publish_emission(
            127,
            HarqEmissionEvent {
                packet_id: 7,
                subpacket: 0,
                packet_start_slot: 0,
                forward_slot: 0,
                expected_ack_reverse_slot: 0,
                terminal: true,
            },
        );
        assert_eq!(bus.drain_emissions(127).len(), 1);
    }

    #[test]
    fn rpc_schedule_is_absolute_slot_tagged() {
        let bus = HarqBus::new();
        bus.schedule_rpc_at_slot(5, 100, 1);
        assert_eq!(bus.rpc_at_slot(5, 100), Some(1));
        assert_eq!(bus.rpc_at_slot(5, 99), None);
        assert_eq!(bus.rpc_at_slot(5, 101), None);

        bus.schedule_rpc_at_slot(5, 100 + RPC_SCHEDULE_RING_SLOTS as u64, 0);
        assert_eq!(bus.rpc_at_slot(5, 100), None);
        assert_eq!(
            bus.rpc_at_slot(5, 100 + RPC_SCHEDULE_RING_SLOTS as u64),
            Some(0)
        );
    }

    #[test]
    fn arq_schedule_stats_distinguish_on_time_and_late_writes() {
        let bus = HarqBus::new();
        bus.schedule_arq_at_slot(6, 108, ArqLevel::Plus, ArqLevel::Off);
        assert_eq!(bus.arq_schedule_stats(6), (1, 0, 0, 0));

        assert!(bus.arq_at_slot(6, 108).is_some());
        bus.schedule_arq_at_slot(6, 107, ArqLevel::Minus, ArqLevel::Off);
        assert_eq!(bus.arq_schedule_stats(6), (1, 1, 1, 108));
    }

    #[test]
    fn purge_mac_clears_rpc_schedule() {
        let bus = HarqBus::new();
        bus.schedule_rpc_at_slot(5, 10, 1);
        assert_eq!(bus.rpc_at_slot(5, 10), Some(1));
        let _ = bus.purge_mac_queues(5);
        assert_eq!(bus.rpc_at_slot(5, 10), None);
    }

    #[test]
    fn distinct_macs_have_distinct_queues() {
        let bus = HarqBus::new();
        bus.publish_emission(
            1,
            HarqEmissionEvent {
                packet_id: 0,
                subpacket: 0,
                packet_start_slot: 0,
                forward_slot: 0,
                expected_ack_reverse_slot: 0,
                terminal: true,
            },
        );
        bus.publish_emission(
            2,
            HarqEmissionEvent {
                packet_id: 9,
                subpacket: 9,
                packet_start_slot: 0,
                forward_slot: 0,
                expected_ack_reverse_slot: 0,
                terminal: true,
            },
        );
        assert_eq!(bus.drain_emissions(1).len(), 1);
        assert_eq!(bus.drain_emissions(2).len(), 1);
        // Both drained.
        assert!(bus.drain_emissions(1).is_empty());
        assert!(bus.drain_emissions(2).is_empty());
    }

    #[test]
    fn future_drc_does_not_govern_packet_start() {
        let bus = HarqBus::new();
        bus.set_current_drc_at_slot(5, 104, 0x3);

        // For start slot 102, FrameOffset 0, DRCLength 2, the governing DRC
        // completed in slot 101. A DRC completed at 104 is from the future for
        // this packet and must not be accepted with zero saturating age.
        assert_eq!(bus.governing_drc(5, 102, 0, 2), None);

        bus.set_current_drc_at_slot(5, 100, 0x2);
        assert_eq!(bus.governing_drc(5, 102, 0, 2), None);

        bus.set_current_drc_at_slot(5, 101, 0x2);
        assert_eq!(bus.governing_drc(5, 102, 0, 2), Some(0x2));
    }

    #[test]
    fn older_drc_history_does_not_replace_latest_record() {
        let bus = HarqBus::new();
        bus.set_current_drc_at_slot(5, 200, 0x2);
        bus.set_current_drc_at_slot(5, 198, 0x3);

        assert_eq!(bus.current_drc_record(5), Some((200, 0x2)));
        assert_eq!(bus.drc_at_slot(5, 198), Some(0x3));
    }
}
