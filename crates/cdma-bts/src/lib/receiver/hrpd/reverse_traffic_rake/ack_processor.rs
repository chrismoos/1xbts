//! Per-frame reverse ACK Channel decoder for the HRPD reverse-traffic
//! sub-chain. Drains pending forward-subpacket emissions from the shared
//! `HarqBus`, decodes one ACK bit per reverse slot in the frame, and
//! publishes matching `HarqFeedbackEvent`s back to the scheduler.

use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;

use cdma_common::diagnostics::hrpd_harq_verbose;
use cdma_common::hrpd::air::HrpdTrafficEvent;

use crate::bts::hrpd::scheduler::HarqResponse;
use crate::bts::hrpd::{HarqBus, HarqEmissionEvent, HarqFeedbackEvent};
use crate::receiver::hrpd::ack_decoder::{
    ACK_CHIPS_PER_BIT, AckDecoder, AckSymbol, slot_expected_in_mask,
};
use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

use super::despread::{
    HRPD_PILOT_CLEAN_START_CHIPS, HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE, HRPD_SLOT_CHIPS,
    HRPD_TRAFFIC_SLOTS_PER_FRAME,
};
use super::finger::{
    TAG_FRAME_START_CHIP, TAG_MAC_INDEX, TAG_PHYSICAL_LAYER_SUBTYPE, TAG_PILOT_COHERENCE_X1000,
    TAG_UATI,
};

pub const TAG_ACK_PATTERN_PACKED: &str = "hrpd_reverse_ack_pattern_packed";

/// Two-bit packed ACK state per slot for [`TAG_ACK_PATTERN_PACKED`].
const ACK_PACK_NOT_EXPECTED: u32 = 0b00;
const ACK_PACK_GATED: u32 = 0b01;
const ACK_PACK_ACK: u32 = 0b10;
const ACK_PACK_NAK: u32 = 0b11;
const HARQ_ACK_ISSUE_LOG_WINDOW_SLOTS: u64 = 600;

#[derive(Clone, Copy)]
enum HarqAckIssue {
    Gated,
    Nak,
    PilotUnlocked,
    ShortFrame,
}

impl HarqAckIssue {
    fn label(self) -> &'static str {
        match self {
            Self::Gated => "missing",
            Self::Nak => "nak",
            Self::PilotUnlocked => "pilot_unlocked",
            Self::ShortFrame => "short_frame",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct HarqAckIssueStats {
    missing: u32,
    nak: u32,
    pilot_unlocked: u32,
    short_frame: u32,
}

impl HarqAckIssueStats {
    fn record(&mut self, issue: HarqAckIssue) {
        match issue {
            HarqAckIssue::Gated => self.missing = self.missing.saturating_add(1),
            HarqAckIssue::Nak => self.nak = self.nak.saturating_add(1),
            HarqAckIssue::PilotUnlocked => {
                self.pilot_unlocked = self.pilot_unlocked.saturating_add(1);
            }
            HarqAckIssue::ShortFrame => self.short_frame = self.short_frame.saturating_add(1),
        }
    }

    fn total(self) -> u32 {
        self.missing
            .saturating_add(self.nak)
            .saturating_add(self.pilot_unlocked)
            .saturating_add(self.short_frame)
    }
}

#[derive(Clone, Copy, Default)]
struct HarqAckFeedbackStats {
    window_start_slot: Option<u64>,
    expected: u32,
    ack: u32,
    nak_terminal: u32,
    nak_nonterminal: u32,
    gated: u32,
    short_frame: u32,
    ack_soft_sum: f64,
    nak_terminal_soft_sum: f64,
    nak_nonterminal_soft_sum: f64,
    gated_abs_max: f32,
    pilot_amp_min: f32,
    pilot_amp_max: f32,
    pilot_coh_min: f32,
}

impl HarqAckFeedbackStats {
    fn record_pilot(&mut self, pilot_amplitude: f32, pilot_coherence: f32) {
        if self.pilot_amp_min == 0.0 || pilot_amplitude < self.pilot_amp_min {
            self.pilot_amp_min = pilot_amplitude;
        }
        if pilot_amplitude > self.pilot_amp_max {
            self.pilot_amp_max = pilot_amplitude;
        }
        if self.pilot_coh_min == 0.0 || pilot_coherence < self.pilot_coh_min {
            self.pilot_coh_min = pilot_coherence;
        }
    }

    fn record_expected(
        &mut self,
        symbol: AckSymbol,
        terminal: bool,
        soft_ratio: Option<f32>,
        short_frame: bool,
    ) {
        self.expected = self.expected.saturating_add(1);
        if short_frame {
            self.short_frame = self.short_frame.saturating_add(1);
            return;
        }
        match symbol {
            AckSymbol::Ack => {
                self.ack = self.ack.saturating_add(1);
                if let Some(ratio) = soft_ratio {
                    self.ack_soft_sum += f64::from(ratio);
                }
            }
            AckSymbol::Nak => {
                if terminal {
                    self.nak_terminal = self.nak_terminal.saturating_add(1);
                    if let Some(ratio) = soft_ratio {
                        self.nak_terminal_soft_sum += f64::from(ratio);
                    }
                } else {
                    self.nak_nonterminal = self.nak_nonterminal.saturating_add(1);
                    if let Some(ratio) = soft_ratio {
                        self.nak_nonterminal_soft_sum += f64::from(ratio);
                    }
                }
            }
            AckSymbol::Gated => {
                self.gated = self.gated.saturating_add(1);
                if let Some(ratio) = soft_ratio {
                    self.gated_abs_max = self.gated_abs_max.max(ratio.abs());
                }
            }
        }
    }
}

/// PipelineProcessor that decodes ACKs for each expected reverse slot and
/// pushes feedback events onto the per-MAC queue of the shared `HarqBus`.
pub struct HrpdReverseTrafficAckProcessor {
    bus: Option<Arc<HarqBus>>,
    /// Pending forward-subpacket emissions awaiting matching reverse ACKs.
    /// Drained from the bus per-frame; pruned once their expected reverse
    /// slot is older than the frame's last slot.
    pending_emissions: Vec<HarqEmissionEvent>,
    /// AN-bridge sink for matched reverse ACK activity.
    event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    frames_seen: u64,
    issue_window_start_slot: Option<u64>,
    issue_stats: HarqAckIssueStats,
    issue_last_frame_slot: u64,
    issue_last_pending: usize,
    issue_last_pilot_amplitude: Option<f32>,
    issue_last_pilot_coherence: f32,
    feedback_stats: HarqAckFeedbackStats,
}

impl HrpdReverseTrafficAckProcessor {
    pub fn new(
        bus: Option<Arc<HarqBus>>,
        event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    ) -> Self {
        Self {
            bus,
            pending_emissions: Vec::new(),
            event_tx,
            frames_seen: 0,
            issue_window_start_slot: None,
            issue_stats: HarqAckIssueStats::default(),
            issue_last_frame_slot: 0,
            issue_last_pending: 0,
            issue_last_pilot_amplitude: None,
            issue_last_pilot_coherence: f32::NAN,
            feedback_stats: HarqAckFeedbackStats::default(),
        }
    }

    fn drain_emissions(&mut self, mac_index: u8) {
        let Some(bus) = self.bus.as_ref() else { return };
        let Some(queue) = bus.emission_queue(mac_index) else {
            return;
        };
        while let Some(event) = queue.pop() {
            self.pending_emissions.push(event);
        }
    }

    fn push_feedback(&self, mac_index: u8, event: HarqFeedbackEvent) {
        if let Some(bus) = self.bus.as_ref() {
            bus.publish_feedback(mac_index, event);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_harq_ack_issue(
        issue: HarqAckIssue,
        mac_index: u8,
        physical_layer_subtype: u16,
        decoder: &AckDecoder,
        frame_start_slot: u64,
        reverse_slot: u64,
        emission: &HarqEmissionEvent,
        soft_ratio: Option<f32>,
        pilot_amplitude: Option<f32>,
        pilot_coherence: f32,
    ) {
        let slot_idx = reverse_slot.saturating_sub(frame_start_slot);
        let soft = soft_ratio
            .map(|ratio| format!("{ratio:+.3}"))
            .unwrap_or_else(|| "n/a".to_string());
        let pilot_amp = pilot_amplitude
            .map(|amp| format!("{amp:.2e}"))
            .unwrap_or_else(|| "n/a".to_string());
        log::warn!(
            "rx_hrpd_traffic[m{}]: harq_ack_{} packet_id={} packet_start_slot={} forward_slot={} expected_reverse_slot={} frame_slot={} slot_idx={} terminal={} physical_subtype=0x{:04x} ack_walsh=W{}_{} soft={} gate={:.3} pilot_amp={} pilot_coh={:.3}",
            mac_index,
            issue.label(),
            emission.packet_id,
            emission.packet_start_slot,
            emission.forward_slot,
            emission.expected_ack_reverse_slot,
            frame_start_slot,
            slot_idx,
            emission.terminal,
            physical_layer_subtype,
            decoder.walsh_index(),
            decoder.walsh_len(),
            soft,
            decoder.threshold(),
            pilot_amp,
            pilot_coherence
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_harq_ack_issue(
        &mut self,
        issue: HarqAckIssue,
        mac_index: u8,
        physical_layer_subtype: u16,
        decoder: &AckDecoder,
        frame_start_slot: u64,
        reverse_slot: u64,
        emission: &HarqEmissionEvent,
        soft_ratio: Option<f32>,
        pilot_amplitude: Option<f32>,
        pilot_coherence: f32,
    ) {
        if self.issue_window_start_slot.is_none() {
            self.issue_window_start_slot = Some(frame_start_slot);
        }
        self.issue_stats.record(issue);
        self.issue_last_frame_slot = frame_start_slot;
        self.issue_last_pending = self.pending_emissions.len();
        self.issue_last_pilot_amplitude = pilot_amplitude;
        self.issue_last_pilot_coherence = pilot_coherence;
        if hrpd_harq_verbose() {
            Self::emit_harq_ack_issue(
                issue,
                mac_index,
                physical_layer_subtype,
                decoder,
                frame_start_slot,
                reverse_slot,
                emission,
                soft_ratio,
                pilot_amplitude,
                pilot_coherence,
            );
        }
    }

    fn maybe_report_harq_ack_issues(&mut self, mac_index: u8, frame_start_slot: u64) {
        let Some(start_slot) = self.issue_window_start_slot else {
            return;
        };
        if self.issue_stats.total() == 0 {
            return;
        }
        let window_slots = frame_start_slot.saturating_sub(start_slot);
        if window_slots < HARQ_ACK_ISSUE_LOG_WINDOW_SLOTS {
            return;
        }
        let pilot_amp = self
            .issue_last_pilot_amplitude
            .map(|amp| format!("{amp:.2e}"))
            .unwrap_or_else(|| "n/a".to_string());
        log::warn!(
            "rx_hrpd_traffic[m{}]: harq_ack_issues window_start_slot={} window_slots={} missing={} nak={} pilot_unlocked={} short_frame={} last_frame_slot={} pending={} pilot_amp={} pilot_coh={:.3}",
            mac_index,
            start_slot,
            window_slots,
            self.issue_stats.missing,
            self.issue_stats.nak,
            self.issue_stats.pilot_unlocked,
            self.issue_stats.short_frame,
            self.issue_last_frame_slot,
            self.issue_last_pending,
            pilot_amp,
            self.issue_last_pilot_coherence,
        );
        self.issue_window_start_slot = Some(frame_start_slot);
        self.issue_stats = HarqAckIssueStats::default();
    }

    fn maybe_report_harq_ack_feedback(&mut self, mac_index: u8, frame_start_slot: u64) {
        let Some(start_slot) = self.feedback_stats.window_start_slot else {
            self.feedback_stats.window_start_slot = Some(frame_start_slot);
            return;
        };
        let window_slots = frame_start_slot.saturating_sub(start_slot);
        if window_slots < HARQ_ACK_ISSUE_LOG_WINDOW_SLOTS {
            return;
        }
        if self.feedback_stats.expected != 0 {
            let ack_avg = avg_soft(self.feedback_stats.ack_soft_sum, self.feedback_stats.ack);
            let nak_terminal_avg = avg_soft(
                self.feedback_stats.nak_terminal_soft_sum,
                self.feedback_stats.nak_terminal,
            );
            let nak_nonterminal_avg = avg_soft(
                self.feedback_stats.nak_nonterminal_soft_sum,
                self.feedback_stats.nak_nonterminal,
            );
            log::debug!(
                "rx_hrpd_traffic[m{}]: harq_ack_summary window_start_slot={} window_slots={} expected={} ack={} nak_terminal={} nak_nonterminal={} gated={} short_frame={} ack_soft_avg={} nak_terminal_soft_avg={} nak_nonterminal_soft_avg={} gated_abs_max={:.3} pilot_amp_min={:.2e} pilot_amp_max={:.2e} pilot_coh_min={:.3}",
                mac_index,
                start_slot,
                window_slots,
                self.feedback_stats.expected,
                self.feedback_stats.ack,
                self.feedback_stats.nak_terminal,
                self.feedback_stats.nak_nonterminal,
                self.feedback_stats.gated,
                self.feedback_stats.short_frame,
                ack_avg,
                nak_terminal_avg,
                nak_nonterminal_avg,
                self.feedback_stats.gated_abs_max,
                self.feedback_stats.pilot_amp_min,
                self.feedback_stats.pilot_amp_max,
                self.feedback_stats.pilot_coh_min,
            );
        }
        self.feedback_stats = HarqAckFeedbackStats {
            window_start_slot: Some(frame_start_slot),
            ..HarqAckFeedbackStats::default()
        };
    }
}

fn avg_soft(sum: f64, count: u32) -> String {
    if count == 0 {
        "n/a".to_string()
    } else {
        format!("{:+.3}", sum / f64::from(count))
    }
}

/// Mean despread pilot amplitude over each slot's ACK-free W0 pilot tail.
/// This is an unbiased amplitude reference in the same units as the despread
/// chips, which keep the raw capture amplitude.
pub(crate) fn frame_pilot_amplitude(chips: &[num_complex::Complex32]) -> f32 {
    let mut sum = 0.0f64;
    let mut n = 0u32;
    for (idx, chip) in chips.iter().enumerate() {
        if idx % HRPD_SLOT_CHIPS < HRPD_PILOT_CLEAN_START_CHIPS {
            continue;
        }
        sum += f64::from(chip.re);
        n += 1;
    }
    ((sum / f64::from(n.max(1))).abs()) as f32
}

impl PipelineProcessor for HrpdReverseTrafficAckProcessor {
    fn process_block(&mut self, mut block: SampleBlock) -> Vec<SampleBlock> {
        let Some(&frame_start_chip) = block.tags.get(TAG_FRAME_START_CHIP) else {
            return vec![block];
        };
        let Some(&mac_index_tag) = block.tags.get(TAG_MAC_INDEX) else {
            return vec![block];
        };
        let mac_index = mac_index_tag as u8;
        let uati = block.tags.get(TAG_UATI).copied().unwrap_or(0) as u32;
        let physical_layer_subtype = block
            .tags
            .get(TAG_PHYSICAL_LAYER_SUBTYPE)
            .copied()
            .unwrap_or(0) as u16;
        let decoder = AckDecoder::for_physical_layer_subtype(physical_layer_subtype);
        let frame_start_slot = (frame_start_chip as u64) / HRPD_SLOT_CHIPS as u64;
        let frame_end_slot = frame_start_slot + HRPD_TRAFFIC_SLOTS_PER_FRAME as u64;
        self.frames_seen = self.frames_seen.saturating_add(1);

        self.drain_emissions(mac_index);

        // Build expected-slot mask for this frame. C.S0024-0
        // §9.2.1.3.3.4 fixes the ACK response slot at forward slot n + 3;
        // off-by-one energy is diagnostic only and must not close H-ARQ or
        // notify the AN that RTCAck was delivered.
        let mut expected_mask: u16 = 0;
        for ev in &self.pending_emissions {
            let slot = ev.expected_ack_reverse_slot;
            if slot >= frame_start_slot && slot < frame_end_slot {
                let bit = (slot - frame_start_slot) as u32;
                expected_mask |= 1u16 << bit;
            }
        }
        // If no bus is wired, decode every slot and rely on the magnitude
        // gate to reject silent slots.
        let mask = if self.bus.is_some() {
            expected_mask
        } else {
            0xffffu16
        };
        let pilot_coherence = block
            .tags
            .get(TAG_PILOT_COHERENCE_X1000)
            .map(|v| *v as f32 / 1000.0)
            .unwrap_or(HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE);
        if pilot_coherence < HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE {
            let mut keep = Vec::with_capacity(self.pending_emissions.len());
            let mut pilot_unlocked = Vec::new();
            for ev in self.pending_emissions.drain(..) {
                if ev.expected_ack_reverse_slot >= frame_start_slot
                    && ev.expected_ack_reverse_slot < frame_end_slot
                {
                    pilot_unlocked.push(ev.clone());
                }
                if ev.expected_ack_reverse_slot >= frame_end_slot {
                    keep.push(ev);
                }
            }
            self.pending_emissions = keep;
            for ev in &pilot_unlocked {
                self.record_harq_ack_issue(
                    HarqAckIssue::PilotUnlocked,
                    mac_index,
                    physical_layer_subtype,
                    &decoder,
                    frame_start_slot,
                    ev.expected_ack_reverse_slot,
                    ev,
                    None,
                    None,
                    pilot_coherence,
                );
            }
            self.maybe_report_harq_ack_issues(mac_index, frame_start_slot);
            self.maybe_report_harq_ack_feedback(mac_index, frame_start_slot);
            block.tags.insert(TAG_ACK_PATTERN_PACKED, 0);
            return vec![block];
        }

        let pilot_amplitude = frame_pilot_amplitude(&block.samples);
        self.feedback_stats
            .record_pilot(pilot_amplitude, pilot_coherence);

        let mut packed: u32 = 0;
        let mut expected_gated = 0u32;
        let mut expected_seen = 0u32;
        let mut unexpected_seen: Vec<(u32, bool)> = Vec::new();
        let mut expected_issues: Vec<(HarqAckIssue, HarqEmissionEvent, Option<f32>)> = Vec::new();
        // Soft despread-to-pilot ratio for every slot. A real NAK pattern
        // appears only in the slots tied to our forward transmissions (and
        // their 4-slot interlace companions); pilot/DRC leakage from a
        // timing or phase offset is uniform across all 16 slots. Logging
        // the full profile separates the two even below the gate.
        let mut soft_profile = [f32::NAN; HRPD_TRAFFIC_SLOTS_PER_FRAME];
        for slot_idx in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME as u32 {
            let start = (slot_idx as usize) * HRPD_SLOT_CHIPS;
            let end = start + ACK_CHIPS_PER_BIT;
            let despread = if end <= block.samples.len() {
                decoder.despread_slot(&block.samples[start..end])
            } else {
                None
            };
            if let Some(avg) = despread {
                soft_profile[slot_idx as usize] = avg.re / pilot_amplitude.max(1e-12);
            }
            let abs_reverse_slot = frame_start_slot + slot_idx as u64;
            let expected_emission = if slot_expected_in_mask(mask, slot_idx) {
                self.pending_emissions
                    .iter()
                    .find(|ev| ev.expected_ack_reverse_slot == abs_reverse_slot)
                    .cloned()
            } else {
                None
            };
            let symbol = despread
                .map(|avg| decoder.classify(avg, pilot_amplitude))
                .unwrap_or(AckSymbol::Gated);
            if let Some(emission) = expected_emission.as_ref() {
                // Soft value is avg.re/pilot: a real ACK lands near the AT's
                // ACK-channel-to-pilot gain (well above threshold); a value
                // hugging the gate is a false positive from an AT that did not
                // actually decode the forward packet. Logs whether the handset
                // is really decoding our subtype-2 forward or the ACK is noise.
                log::log!(
                    if symbol == AckSymbol::Nak {
                        log::Level::Info
                    } else {
                        log::Level::Trace
                    },
                    "HRPD ACK decode: slot={slot_idx} soft={:.3} threshold={:.3} symbol={symbol:?} terminal={}",
                    soft_profile[slot_idx as usize],
                    decoder.threshold(),
                    emission.terminal,
                );
                self.feedback_stats.record_expected(
                    symbol,
                    emission.terminal,
                    despread.map(|_| soft_profile[slot_idx as usize]),
                    despread.is_none(),
                );
            }
            let slot_state = if despread.is_none() {
                ACK_PACK_NOT_EXPECTED
            } else if !slot_expected_in_mask(mask, slot_idx) {
                match symbol {
                    AckSymbol::Ack => unexpected_seen.push((slot_idx, true)),
                    AckSymbol::Nak => unexpected_seen.push((slot_idx, false)),
                    AckSymbol::Gated => {}
                }
                ACK_PACK_NOT_EXPECTED
            } else {
                match symbol {
                    AckSymbol::Ack => ACK_PACK_ACK,
                    AckSymbol::Nak => ACK_PACK_NAK,
                    AckSymbol::Gated => ACK_PACK_GATED,
                }
            };
            packed |= slot_state << (slot_idx * 2);

            // Forward to scheduler if we have an emission match, and emit
            // an AN-bridge event so the operator can observe the ACK.
            if matches!(slot_state, ACK_PACK_ACK | ACK_PACK_NAK) {
                expected_seen = expected_seen.saturating_add(1);
                let ack_bool = slot_state == ACK_PACK_ACK;
                // Remove the matched emission so this reverse slot can't
                // produce duplicate feedback for one forward transmission.
                //
                // Multi-slot packets generate an ACK Channel bit after each
                // transmitted traffic slot, and an early ACK means the AT has
                // already decoded the packet: the AN "does not transmit the
                // remaining slots" and the AT may gate the rest of its ACK
                // channel, so waiting for the terminal slot would retire the
                // packet on timeout. Forward the early ACK (the scheduler
                // retires the packet, ending the remaining continuations);
                // an early NAK stays diagnostic — mid-packet NAKs are normal
                // before the final slot arrives.
                if let Some(pos) = self
                    .pending_emissions
                    .iter()
                    .position(|ev| ev.expected_ack_reverse_slot == abs_reverse_slot)
                {
                    let emission = self.pending_emissions.remove(pos);
                    if emission.terminal || ack_bool {
                        self.pending_emissions
                            .retain(|ev| ev.packet_id != emission.packet_id);
                    }
                    let pending_after = self.pending_emissions.len();
                    let log_line = format!(
                        "rx_hrpd_traffic[m{}]: ack_match packet_id={} packet_start_slot={} forward_slot={} expected_reverse_slot={} actual_reverse_slot={} response={} terminal={} pending_after={}",
                        mac_index,
                        emission.packet_id,
                        emission.packet_start_slot,
                        emission.forward_slot,
                        emission.expected_ack_reverse_slot,
                        abs_reverse_slot,
                        if ack_bool { "ACK" } else { "NAK" },
                        emission.terminal,
                        pending_after,
                    );
                    if !ack_bool {
                        self.record_harq_ack_issue(
                            HarqAckIssue::Nak,
                            mac_index,
                            physical_layer_subtype,
                            &decoder,
                            frame_start_slot,
                            abs_reverse_slot,
                            &emission,
                            Some(soft_profile[slot_idx as usize]),
                            Some(pilot_amplitude),
                            pilot_coherence,
                        );
                        log::debug!("{log_line}");
                    } else {
                        log::trace!("{log_line}");
                    }
                    if ack_bool {
                        self.push_feedback(
                            mac_index,
                            HarqFeedbackEvent {
                                packet_id: emission.packet_id,
                                subpacket: emission.subpacket,
                                response: HarqResponse::Ack,
                            },
                        );
                    } else if emission.terminal {
                        self.push_feedback(
                            mac_index,
                            HarqFeedbackEvent {
                                packet_id: emission.packet_id,
                                subpacket: emission.subpacket,
                                response: HarqResponse::Nak,
                            },
                        );
                    }
                }
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(HrpdTrafficEvent::Ack {
                        uati,
                        mac_index,
                        slot: abs_reverse_slot,
                        ack: ack_bool,
                    });
                }
            } else if slot_state == ACK_PACK_GATED {
                expected_gated = expected_gated.saturating_add(1);
                for emission in self
                    .pending_emissions
                    .iter()
                    .filter(|ev| ev.expected_ack_reverse_slot == abs_reverse_slot)
                {
                    expected_issues.push((
                        if despread.is_some() {
                            HarqAckIssue::Gated
                        } else {
                            HarqAckIssue::ShortFrame
                        },
                        emission.clone(),
                        despread.map(|_| soft_profile[slot_idx as usize]),
                    ));
                }
            }
        }

        for (issue, emission, soft_ratio) in &expected_issues {
            self.record_harq_ack_issue(
                *issue,
                mac_index,
                physical_layer_subtype,
                &decoder,
                frame_start_slot,
                emission.expected_ack_reverse_slot,
                emission,
                *soft_ratio,
                Some(pilot_amplitude),
                pilot_coherence,
            );
        }
        self.maybe_report_harq_ack_issues(mac_index, frame_start_slot);
        self.maybe_report_harq_ack_feedback(mac_index, frame_start_slot);

        if expected_mask != 0 && (expected_gated != 0 || expected_seen != 0) {
            let soft = soft_profile
                .iter()
                .map(|ratio| {
                    if ratio.is_nan() {
                        "·".to_string()
                    } else {
                        format!("{ratio:+.2}")
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            let log_line = format!(
                "rx_hrpd_traffic[m{}]: ack_slots frame_slot={} physical_subtype=0x{:04x} ack_walsh=W{}_{} expected_mask=0x{:04x} decoded={} gated={} pending={} pilot_amp={:.2e} soft=[{}]",
                mac_index,
                frame_start_slot,
                physical_layer_subtype,
                decoder.walsh_index(),
                decoder.walsh_len(),
                expected_mask,
                expected_seen,
                expected_gated,
                self.pending_emissions.len(),
                pilot_amplitude,
                soft
            );
            log::trace!("{log_line}");
        }
        if hrpd_harq_verbose()
            && !unexpected_seen.is_empty()
            && (self.frames_seen <= 8 || self.frames_seen % 32 == 0)
        {
            let summary = unexpected_seen
                .iter()
                .map(|(slot, ack)| format!("{}:{}", slot, if *ack { "ACK" } else { "NAK" }))
                .collect::<Vec<_>>()
                .join(",");
            log::info!(
                "rx_hrpd_traffic[m{}]: unplanned_ack_energy frame_slot={} slots=[{}]",
                mac_index,
                frame_start_slot,
                summary
            );
        }

        // Retire any emissions whose exact expected slot is no longer ahead
        // of us.
        self.pending_emissions
            .retain(|ev| ev.expected_ack_reverse_slot >= frame_end_slot);

        block.tags.insert(TAG_ACK_PATTERN_PACKED, packed as i64);
        vec![block]
    }

    fn name(&self) -> &'static str {
        "HrpdReverseTrafficAckProcessor"
    }
}
