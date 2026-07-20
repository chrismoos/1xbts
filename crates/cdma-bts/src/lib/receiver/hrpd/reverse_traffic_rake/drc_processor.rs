//! Per-frame DRC decoder for the HRPD reverse-traffic sub-chain.
//!
//! Runs [`DrcDecoder`] over each `DRCLength`-slot window within a 16-slot
//! reverse Traffic Channel frame. The decoded DRC values are packed into a
//! single tag for downstream consumers: 4 bits per slot, low-bit-first.

use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;

use cdma_common::hrpd::air::HrpdTrafficEvent;
use cdma_common::hrpd::traffic::{
    forward_traffic_payload_bits_for_drc, implemented_forward_traffic_payload_bits_for_drc,
};

use crate::bts::hrpd::HarqBus;
use crate::receiver::hrpd::drc_decoder::{DRC_CHIPS_PER_SLOT, DrcDecoder, DrcSymbol};
use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

use super::despread::{
    HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE, HRPD_SLOT_CHIPS, HRPD_TRAFFIC_SLOTS_PER_FRAME,
};
use super::finger::{
    TAG_DRC_COVER, TAG_DRC_LENGTH, TAG_FRAME_OFFSET, TAG_FRAME_START_CHIP, TAG_MAC_INDEX,
    TAG_PHYSICAL_LAYER_SUBTYPE, TAG_PILOT_COHERENCE_X1000, TAG_Q_SIGN_X1000, TAG_UATI,
};

/// Packed per-slot DRC values: 4 bits per slot (slot 0 = bits 0..3, ...,
/// slot 15 = bits 60..63). A slot that did not carry a decoded DRC reads as
/// 0xF (out of range for DRC values 0..15) — see [`DRC_SLOT_GATED_VALUE`].
pub const TAG_DRC_PACKED: &str = "hrpd_reverse_drc_packed";

/// Sentinel nibble for slots that gated off or fell in a DRC-length window
/// that wasn't decoded. DRC values are spec-bounded to 0..=15 and exclude
/// 0xF as a valid decoded value when interpreted as "no decode".
pub const DRC_SLOT_GATED_VALUE: u8 = 0xF;
const DRC_MID_SLOT_OFFSET_CHIPS: usize = DRC_CHIPS_PER_SLOT / 2;
const DRC_EVENT_MIN_CONFIDENCE: f32 = 4.0;
const SCHEDULER_DRC_MIN_CONFIDENCE: f32 = DRC_EVENT_MIN_CONFIDENCE;
const SCHEDULER_DRC_MIN_STABLE_WINDOWS: u8 = 1;
const LOW_PILOT_DRC_MIN_CONFIDENCE: f32 = 4.0;
const LOW_PILOT_DRC_MIN_STABLE_WINDOWS: u8 = 1;

pub struct HrpdReverseTrafficDrcProcessor {
    decoder: DrcDecoder,
    /// Cached cover so we can rebuild the decoder if the tag changes
    /// across frames (e.g. reassignment).
    drc_cover: u8,
    /// AN-bridge event sink. Each decoded DRC value within a frame emits an
    /// `HrpdTrafficEvent::Drc` keyed on the slot where reception of that
    /// DRCLength window completed.
    event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    /// Forward scheduler bus. Each valid decoded DRC writes the current
    /// rate into the per-MAC atomic register so the scheduler honors the
    /// AT's requested rate when it starts encoding a new forward packet
    /// (see C.S0024-0 v4.0 §9.3.1.2).
    bus: Option<Arc<HarqBus>>,
    /// Last DRC value logged for trajectory tracking (log on change only).
    last_logged_value: Option<u8>,
    /// Last high-confidence DRC value decoded, tracked for diagnostics. A
    /// confidence-gated DRC is published immediately because C.S0024-0 lets
    /// the AT change DRC every DRCLength boundary.
    last_confirmed_value: Option<u8>,
    last_confirmed_run: u8,
    last_published_value: Option<u8>,
}

impl HrpdReverseTrafficDrcProcessor {
    /// Build a DRC processor with a default cover. The actual cover comes
    /// from the per-block tag; this initial value just sets up the decoder
    /// before the first block arrives.
    pub fn new(
        event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
        bus: Option<Arc<HarqBus>>,
    ) -> Self {
        Self {
            decoder: DrcDecoder::new(0),
            drc_cover: 0,
            event_tx,
            last_logged_value: None,
            last_confirmed_value: None,
            last_confirmed_run: 0,
            last_published_value: None,
            bus,
        }
    }
}

impl PipelineProcessor for HrpdReverseTrafficDrcProcessor {
    fn process_block(&mut self, mut block: SampleBlock) -> Vec<SampleBlock> {
        let Some(&drc_cover_tag) = block.tags.get(TAG_DRC_COVER) else {
            return vec![block];
        };
        let Some(&drc_length_tag) = block.tags.get(TAG_DRC_LENGTH) else {
            return vec![block];
        };
        let drc_cover = drc_cover_tag as u8;
        let drc_length = drc_length_tag as u8;
        if !matches!(drc_length, 1 | 2 | 4 | 8) {
            return vec![block];
        }
        if drc_cover != self.drc_cover {
            self.decoder.set_drc_cover(drc_cover);
            self.drc_cover = drc_cover;
        }

        let frame_start_chip = block.tags.get(TAG_FRAME_START_CHIP).copied().unwrap_or(0) as u64;
        let mac_index = block.tags.get(TAG_MAC_INDEX).copied().unwrap_or(0) as u8;
        let uati = block.tags.get(TAG_UATI).copied().unwrap_or(0) as u32;
        let frame_offset = block.tags.get(TAG_FRAME_OFFSET).copied().unwrap_or(0) as u8;
        let physical_layer_subtype = block
            .tags
            .get(TAG_PHYSICAL_LAYER_SUBTYPE)
            .copied()
            .unwrap_or(0) as u16;
        let frame_start_slot = frame_start_chip / HRPD_SLOT_CHIPS as u64;
        let pilot_coherence = block
            .tags
            .get(TAG_PILOT_COHERENCE_X1000)
            .map(|v| *v as f32 / 1000.0)
            .unwrap_or(HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE);
        let q_sign = block
            .tags
            .get(TAG_Q_SIGN_X1000)
            .copied()
            .map(|v| if v < 0 { -1.0 } else { 1.0 })
            .unwrap_or(1.0);
        let window_chips = (drc_length as usize) * DRC_CHIPS_PER_SLOT;
        // Initialize every slot nibble to "gated" so unfilled windows are
        // visible.
        let mut packed: u64 = 0;
        for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
            packed |= (DRC_SLOT_GATED_VALUE as u64) << (slot * 4);
        }
        let pilot_locked = pilot_coherence >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE;
        // DRC symbols start at the mid-slot point with boundaries at
        // T == FrameOffset - 1 (mod DRCLength). The despread frame is
        // slot-aligned, so a FrameOffset-aligned decode mixes two adjacent DRC
        // symbols and can publish a plausible but wrong rate to the scheduler.
        //
        // Do not gate the DRC decoder itself on the W0 pilot coherence metric:
        // DRC is a separate Q-arm reverse MAC channel, and live frames can keep
        // producing FCS-valid Q-arm data after the pilot metric falls below the
        // receiver's lock threshold. Use the DRC codeword confidence below for
        // AN/A8 events and scheduler publication.
        let first_window_abs_slot =
            drc_window_start_slot_at_or_after(frame_start_slot, frame_offset, drc_length);
        let mut window_slot = first_window_abs_slot.saturating_sub(frame_start_slot) as usize;
        let mut window_start = window_slot
            .saturating_mul(DRC_CHIPS_PER_SLOT)
            .saturating_add(DRC_MID_SLOT_OFFSET_CHIPS);
        while window_start + window_chips <= block.samples.len()
            && window_slot < HRPD_TRAFFIC_SLOTS_PER_FRAME
        {
            let window = &block.samples[window_start..window_start + window_chips];
            if let Some(DrcSymbol { value, confidence }) = self.decoder.decode(window, drc_length) {
                let value = normalize_drc_polarity(value, q_sign);
                // The decoded DRC value is valid for the *entire*
                // `drc_length`-slot window. Replicate the value across every
                // slot the window covers so downstream consumers can read
                // any individual slot nibble.
                for slot_offset in 0..drc_length as usize {
                    let slot = window_slot + slot_offset;
                    if slot >= HRPD_TRAFFIC_SLOTS_PER_FRAME {
                        break;
                    }
                    // Clear the gated nibble then OR in the decoded value.
                    packed &= !(0xFu64 << (slot * 4));
                    packed |= (value as u64 & 0xF) << (slot * 4);
                }
                let completion_slot = drc_completion_slot_for_repetition(
                    frame_start_slot + window_slot as u64,
                    frame_offset,
                    drc_length,
                );
                let has_forward_rate =
                    forward_traffic_payload_bits_for_drc_in_subtype(value, physical_layer_subtype)
                        .is_some();
                let has_implemented_forward_rate =
                    implemented_forward_traffic_payload_bits_for_drc_in_subtype(
                        value,
                        physical_layer_subtype,
                    )
                    .is_some();
                let required_confidence = if pilot_locked {
                    SCHEDULER_DRC_MIN_CONFIDENCE
                } else {
                    LOW_PILOT_DRC_MIN_CONFIDENCE
                };
                let required_stable_windows = if pilot_locked {
                    SCHEDULER_DRC_MIN_STABLE_WINDOWS
                } else {
                    LOW_PILOT_DRC_MIN_STABLE_WINDOWS
                };
                if has_implemented_forward_rate && confidence >= required_confidence {
                    if self.last_confirmed_value == Some(value) {
                        self.last_confirmed_run = self.last_confirmed_run.saturating_add(1);
                    } else {
                        self.last_confirmed_value = Some(value);
                        self.last_confirmed_run = 1;
                    }
                } else {
                    self.last_confirmed_value = None;
                    self.last_confirmed_run = 0;
                }
                let scheduler_gate = has_implemented_forward_rate
                    && confidence >= required_confidence
                    && self.last_confirmed_run >= required_stable_windows;
                let event_gate = scheduler_gate;
                if event_gate && let Some(tx) = &self.event_tx {
                    let _ = tx.send(HrpdTrafficEvent::Drc {
                        uati,
                        mac_index,
                        slot: completion_slot,
                        drc_index: value,
                    });
                }
                // Publish live-supported forward rates once the DRC codeword
                // is credible. A completed DRC governs the next DRCLength
                // packet-start slots, so waiting for a second same-valued
                // DRC can hide the exact governing slot from the scheduler.
                if scheduler_gate {
                    if let Some(bus) = self.bus.as_ref() {
                        bus.set_current_drc_at_slot(mac_index, completion_slot, value);
                    }
                    if self.last_published_value != Some(value) {
                        log::trace!(
                            "rx_hrpd_traffic[m{}]: drc_scheduler_publish uati=0x{:08x} slot={} drc=0x{:x} stable_windows={} confidence={:.2} pilot_locked={}",
                            mac_index,
                            uati,
                            completion_slot,
                            value,
                            self.last_confirmed_run,
                            confidence,
                            pilot_locked,
                        );
                        self.last_published_value = Some(value);
                    }
                }
                // The AT's rate trajectory matters for diagnosing wrong-rate
                // packet decode failures: the scheduler paces off this value
                // through a queue that can lag seconds behind the air.
                if self.last_logged_value != Some(value) {
                    let msg = format!(
                        "rx_hrpd_traffic[m{}]: drc_value_change uati=0x{:08x} slot={} drc=0x{:x} valid_rate={} confidence={:.2} event_gate={} scheduler_gate={} pilot_locked={} (was {:?})",
                        mac_index,
                        uati,
                        completion_slot,
                        value,
                        has_forward_rate,
                        confidence,
                        event_gate,
                        scheduler_gate,
                        pilot_locked,
                        self.last_logged_value,
                    );
                    log::trace!("{msg}");
                    self.last_logged_value = Some(value);
                }
            }
            window_start += window_chips;
            window_slot += drc_length as usize;
        }

        block.tags.insert(TAG_DRC_PACKED, packed as i64);
        vec![block]
    }

    fn name(&self) -> &'static str {
        "HrpdReverseTrafficDrcProcessor"
    }
}

fn forward_traffic_payload_bits_for_drc_in_subtype(
    drc_index: u8,
    physical_layer_subtype: u16,
) -> Option<usize> {
    if physical_layer_subtype < 2 && drc_index >= 0x0d {
        // C.S0024-0 v4.0 Table 8.4.6.1.4.1-1 marks 0xd/0xe/0xf invalid for
        // the default FTC MAC used during initial setup. Enhanced subtype
        // paths add 0xd/0xe, but only after that subtype is actually in use.
        return None;
    }
    forward_traffic_payload_bits_for_drc(drc_index)
}

fn implemented_forward_traffic_payload_bits_for_drc_in_subtype(
    drc_index: u8,
    physical_layer_subtype: u16,
) -> Option<usize> {
    if physical_layer_subtype < 2 && drc_index >= 0x0d {
        return None;
    }
    implemented_forward_traffic_payload_bits_for_drc(drc_index)
}

pub(super) fn drc_window_start_slot_at_or_after(
    slot: u64,
    frame_offset: u8,
    drc_length: u8,
) -> u64 {
    let drc_length = u64::from(drc_length.max(1));
    let offset = drc_boundary_phase_offset(frame_offset, drc_length);
    let phase = (slot + drc_length - offset) % drc_length;
    if phase == 0 {
        slot
    } else {
        slot + (drc_length - phase)
    }
}

/// Boundary slot ending the DRC window that contains `repetition_slot`. DRC
/// records are published at this slot, and the forward scheduler's exact
/// governing-DRC lookup expects records on exactly these boundary slots.
/// Strictly, §10.7.6.1 has the value take effect one slot later (the DRC
/// transmission ends mid-slot of the boundary slot), so each DRC change is
/// honored one slot early; changing that convention requires moving the
/// scheduler's governing lookup in the same step.
pub(super) fn drc_completion_slot_for_repetition(
    repetition_slot: u64,
    frame_offset: u8,
    drc_length: u8,
) -> u64 {
    let drc_length = u64::from(drc_length.max(1));
    let offset = drc_boundary_phase_offset(frame_offset, drc_length);
    let phase_from_window_start = (repetition_slot + drc_length - offset) % drc_length;
    repetition_slot + (drc_length - phase_from_window_start)
}

#[inline]
fn drc_boundary_phase_offset(frame_offset: u8, drc_length: u64) -> u64 {
    (u64::from(frame_offset) + drc_length - 1) % drc_length
}

#[inline]
fn normalize_drc_polarity(value: u8, q_sign: f32) -> u8 {
    if q_sign < 0.0 {
        // The finger's pilot reference can use the alternate reverse Q-arm
        // convention. DRC codewords are bi-orthogonal complement pairs, so a
        // Q sign inversion flips only the low polarity bit.
        value ^ 1
    } else {
        value
    }
}
