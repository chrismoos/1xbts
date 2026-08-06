//! Fast-DRC decode and publication methods for `HrpdReverseTrafficFinger`.
//!
//! Split out of the primary finger impl to keep it focused; these methods
//! run on the same struct and access its state directly.

use super::*;

impl HrpdReverseTrafficFinger {
    pub(super) fn process_pending_drc_windows(&mut self) {
        let drc_length = self.config.drc_length.max(1);
        if !matches!(drc_length, 1 | 2 | 4 | 8) {
            return;
        }
        let oversample = self.config.oversample.max(1) as u64;
        let window_chips = u64::from(drc_length) * HRPD_SLOT_CHIPS as u64;
        let window_step_chips = window_chips;
        loop {
            let Some(buf_abs) = self.buffer_abs_sample else {
                return;
            };
            let window_chip = self.next_drc_window_chip;
            let window_start_sample = (window_chip * oversample) as i64;
            let sample_delay = self.next_params.sample_delay;
            let sample_delay_fraction = self.next_params.sample_delay_fraction;
            let earliest = window_start_sample + sample_delay as i64;
            let latest = window_start_sample
                + (window_chips as i64 - 1) * oversample as i64
                + sample_delay as i64
                + 2;
            if earliest < buf_abs as i64 {
                self.next_drc_window_chip = window_chip.saturating_add(window_step_chips);
                continue;
            }
            if latest >= (buf_abs + self.buffer.len() as u64) as i64 {
                return;
            }

            let ref_conj = self.reference_for_region(window_chip, window_chips as usize);
            let Some(despread) = despread_chips_with_reference(
                &self.buffer,
                buf_abs,
                oversample as usize,
                window_chip,
                sample_delay,
                sample_delay_fraction,
                self.next_params.pilot_phase.conj(),
                &ref_conj,
            ) else {
                return;
            };
            let window_start_slot =
                window_chip.saturating_sub(DRC_MID_SLOT_OFFSET_CHIPS) / HRPD_SLOT_CHIPS as u64;
            let completion_slot = drc_completion_slot_for_repetition(
                window_start_slot,
                self.config.frame_offset,
                drc_length,
            );
            self.fast_drc_stats.window_attempts =
                self.fast_drc_stats.window_attempts.saturating_add(1);
            if let Some(symbol) = self
                .fast_drc_decoder
                .decode_pilot_derotated(&despread, drc_length)
            {
                self.publish_confirmed_drc(completion_slot, symbol, true);
            } else {
                self.fast_drc_stats.window_none = self.fast_drc_stats.window_none.saturating_add(1);
            }
            self.maybe_log_fast_drc_stats(completion_slot);
            self.next_drc_window_chip = window_chip.saturating_add(window_step_chips);
        }
    }

    pub(super) fn pending_fast_drc_retention_sample(
        &self,
        oversample: u64,
        retention_back_margin: u64,
    ) -> Option<u64> {
        if !matches!(self.config.drc_length, 1 | 2 | 4 | 8) {
            return None;
        }
        let pending_chip = self.next_drc_repetition_chip.min(self.next_drc_window_chip);
        Some(
            pending_chip
                .saturating_mul(oversample)
                .saturating_sub(retention_back_margin),
        )
    }

    pub(super) fn process_pending_drc_repetitions(&mut self) {
        let drc_length = self.config.drc_length.max(1);
        if !matches!(drc_length, 1 | 2 | 4 | 8) {
            return;
        }
        let oversample = self.config.oversample.max(1) as u64;
        let slot_chips = HRPD_SLOT_CHIPS as u64;
        loop {
            let Some(buf_abs) = self.buffer_abs_sample else {
                return;
            };
            let repetition_chip = self.next_drc_repetition_chip;
            let repetition_start_sample = (repetition_chip * oversample) as i64;
            let sample_delay = self.next_params.sample_delay;
            let sample_delay_fraction = self.next_params.sample_delay_fraction;
            let earliest = repetition_start_sample + sample_delay as i64;
            let latest = repetition_start_sample
                + (slot_chips as i64 - 1) * oversample as i64
                + sample_delay as i64
                + 2;
            if earliest < buf_abs as i64 {
                self.next_drc_repetition_chip = repetition_chip.saturating_add(slot_chips);
                continue;
            }
            if latest >= (buf_abs + self.buffer.len() as u64) as i64 {
                return;
            }

            let ref_conj = self.reference_for_region(repetition_chip, slot_chips as usize);
            let Some(despread) = despread_chips_with_reference(
                &self.buffer,
                buf_abs,
                oversample as usize,
                repetition_chip,
                sample_delay,
                sample_delay_fraction,
                self.next_params.pilot_phase.conj(),
                &ref_conj,
            ) else {
                return;
            };
            let repetition_slot =
                repetition_chip.saturating_sub(DRC_MID_SLOT_OFFSET_CHIPS) / slot_chips;
            let completion_slot = drc_completion_slot_for_repetition(
                repetition_slot,
                self.config.frame_offset,
                drc_length,
            );
            self.fast_drc_stats.repetition_attempts =
                self.fast_drc_stats.repetition_attempts.saturating_add(1);
            if let Some(symbol) = self.fast_drc_decoder.decode_pilot_derotated(&despread, 1) {
                self.publish_repeated_drc(completion_slot, symbol);
            } else {
                self.fast_drc_stats.repetition_none =
                    self.fast_drc_stats.repetition_none.saturating_add(1);
            }
            self.maybe_log_fast_drc_stats(completion_slot);
            self.next_drc_repetition_chip = repetition_chip.saturating_add(slot_chips);
        }
    }

    fn reference_for_region(&self, start_chip: u64, len: usize) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(len);
        let period = HRPD_TRAFFIC_FRAME_CHIPS as u64;
        for offset in 0..len {
            let idx = ((start_chip
                .saturating_sub(self.spawn_chip)
                .saturating_add(offset as u64))
                % period) as usize;
            out.push(self.ref_conj[idx]);
        }
        out
    }

    pub(super) fn publish_repeated_drc(&mut self, completion_slot: u64, symbol: DrcSymbol) {
        let Some(value) = self.accepted_drc_value(symbol, FastDrcSource::Repetition) else {
            self.fast_drc_candidate_slot = None;
            self.fast_drc_candidate_value = None;
            self.fast_drc_candidate_run = 0;
            return;
        };
        self.fast_drc_stats.repetition_candidates =
            self.fast_drc_stats.repetition_candidates.saturating_add(1);

        if self.fast_drc_candidate_slot == Some(completion_slot)
            && self.fast_drc_candidate_value == Some(value)
        {
            self.fast_drc_candidate_run = self.fast_drc_candidate_run.saturating_add(1);
        } else {
            self.fast_drc_candidate_slot = Some(completion_slot);
            self.fast_drc_candidate_value = Some(value);
            self.fast_drc_candidate_run = 1;
        }

        if self.fast_drc_candidate_run < FAST_DRC_MIN_CONFIRMED_REPETITIONS {
            return;
        }

        if self.fast_drc_last_published_slot == Some(completion_slot) {
            self.fast_drc_stats.repetition_duplicates =
                self.fast_drc_stats.repetition_duplicates.saturating_add(1);
            return;
        }

        self.publish_confirmed_drc(completion_slot, symbol, false);
    }

    pub(super) fn publish_confirmed_drc(
        &mut self,
        completion_slot: u64,
        symbol: DrcSymbol,
        full_window: bool,
    ) {
        let source = if full_window {
            FastDrcSource::Window
        } else {
            FastDrcSource::Repetition
        };
        let Some(value) = self.accepted_drc_value(symbol, source) else {
            return;
        };
        if let Some(bus) = self.config.harq_bus.as_ref() {
            bus.set_current_drc_at_slot(self.config.mac_index, completion_slot, value);
        }
        if let Some(tx) = &self.config.event_tx {
            let _ = tx.send(HrpdTrafficEvent::Drc {
                uati: self.config.uati,
                mac_index: self.config.mac_index,
                slot: completion_slot,
                drc_index: value,
            });
        }
        self.record_drc_publish(source, completion_slot);
        trace!(
            "rx_hrpd_traffic[m{}]: fast_drc_publish uati=0x{:08x} slot={} drc=0x{:x} confirmed_repetitions={} confidence={:.2} source={}",
            self.config.mac_index,
            self.config.uati,
            completion_slot,
            value,
            if full_window {
                u8::from(self.config.drc_length.max(1))
            } else {
                self.fast_drc_candidate_run
            },
            symbol.confidence,
            if full_window { "window" } else { "slot" },
        );
        self.fast_drc_last_published_slot = Some(completion_slot);
        self.fast_drc_last_published_value = Some(value);
    }

    fn accepted_drc_value(&mut self, symbol: DrcSymbol, source: FastDrcSource) -> Option<u8> {
        let value = normalize_drc_polarity(symbol.value, self.mask.q_sign);
        if implemented_forward_traffic_payload_bits_for_drc_in_subtype(
            value,
            self.config.physical_layer_subtype,
        )
        .is_none()
        {
            self.record_drc_reject(source, FastDrcReject::Invalid, value, symbol.confidence);
            return None;
        }
        if symbol.confidence < DRC_EVENT_MIN_CONFIDENCE {
            self.record_drc_reject(
                source,
                FastDrcReject::LowConfidence,
                value,
                symbol.confidence,
            );
            return None;
        }
        Some(value)
    }

    fn record_drc_reject(
        &mut self,
        source: FastDrcSource,
        reject: FastDrcReject,
        value: u8,
        confidence: f32,
    ) {
        match (source, reject) {
            (FastDrcSource::Repetition, FastDrcReject::Invalid) => {
                self.fast_drc_stats.repetition_invalid =
                    self.fast_drc_stats.repetition_invalid.saturating_add(1);
            }
            (FastDrcSource::Repetition, FastDrcReject::LowConfidence) => {
                self.fast_drc_stats.repetition_low_confidence = self
                    .fast_drc_stats
                    .repetition_low_confidence
                    .saturating_add(1);
                if self.fast_drc_last_published_value == Some(value) {
                    self.fast_drc_stats.repetition_low_confidence_same_as_last = self
                        .fast_drc_stats
                        .repetition_low_confidence_same_as_last
                        .saturating_add(1);
                }
                self.fast_drc_stats.repetition_low_confidence_min = Some(
                    self.fast_drc_stats
                        .repetition_low_confidence_min
                        .map_or(confidence, |current| current.min(confidence)),
                );
                self.fast_drc_stats.repetition_low_confidence_max = Some(
                    self.fast_drc_stats
                        .repetition_low_confidence_max
                        .map_or(confidence, |current| current.max(confidence)),
                );
                self.fast_drc_stats.repetition_low_confidence_values[value as usize] =
                    self.fast_drc_stats.repetition_low_confidence_values[value as usize]
                        .saturating_add(1);
            }
            (FastDrcSource::Window, FastDrcReject::Invalid) => {
                self.fast_drc_stats.window_invalid =
                    self.fast_drc_stats.window_invalid.saturating_add(1);
            }
            (FastDrcSource::Window, FastDrcReject::LowConfidence) => {
                self.fast_drc_stats.window_low_confidence =
                    self.fast_drc_stats.window_low_confidence.saturating_add(1);
                if self.fast_drc_last_published_value == Some(value) {
                    self.fast_drc_stats.window_low_confidence_same_as_last = self
                        .fast_drc_stats
                        .window_low_confidence_same_as_last
                        .saturating_add(1);
                }
                self.fast_drc_stats.window_low_confidence_min = Some(
                    self.fast_drc_stats
                        .window_low_confidence_min
                        .map_or(confidence, |current| current.min(confidence)),
                );
                self.fast_drc_stats.window_low_confidence_max = Some(
                    self.fast_drc_stats
                        .window_low_confidence_max
                        .map_or(confidence, |current| current.max(confidence)),
                );
                self.fast_drc_stats.window_low_confidence_values[value as usize] =
                    self.fast_drc_stats.window_low_confidence_values[value as usize]
                        .saturating_add(1);
            }
        }
    }

    fn record_drc_publish(&mut self, source: FastDrcSource, completion_slot: u64) {
        match source {
            FastDrcSource::Repetition => {
                self.fast_drc_stats.repetition_published =
                    self.fast_drc_stats.repetition_published.saturating_add(1);
            }
            FastDrcSource::Window => {
                self.fast_drc_stats.window_published =
                    self.fast_drc_stats.window_published.saturating_add(1);
            }
        }
        if let Some(last_slot) = self.fast_drc_last_published_slot {
            self.fast_drc_stats.max_publish_gap_slots = self
                .fast_drc_stats
                .max_publish_gap_slots
                .max(completion_slot.saturating_sub(last_slot));
        }
    }

    fn maybe_log_fast_drc_stats(&mut self, slot: u64) {
        let Some(start_slot) = self.fast_drc_stats.window_start_slot else {
            self.fast_drc_stats.window_start_slot = Some(slot);
            return;
        };
        if slot.saturating_sub(start_slot) < HRPD_FAST_DRC_SUMMARY_INTERVAL_SLOTS {
            return;
        }
        debug!(
            "rx_hrpd_traffic[m{}]: fast_drc_summary uati=0x{:08x} slots={} rep_attempts={} rep_none={} rep_invalid={} rep_low_conf={} rep_low_conf_same_last={} rep_low_conf_range={:.2}..{:.2} rep_low_conf_values={:?} rep_candidates={} rep_published={} rep_duplicates={} window_attempts={} window_none={} window_invalid={} window_low_conf={} window_low_conf_same_last={} window_low_conf_range={:.2}..{:.2} window_low_conf_values={:?} window_published={} last_published={} last_value={} max_publish_gap_slots={}",
            self.config.mac_index,
            self.config.uati,
            slot.saturating_sub(start_slot),
            self.fast_drc_stats.repetition_attempts,
            self.fast_drc_stats.repetition_none,
            self.fast_drc_stats.repetition_invalid,
            self.fast_drc_stats.repetition_low_confidence,
            self.fast_drc_stats.repetition_low_confidence_same_as_last,
            self.fast_drc_stats
                .repetition_low_confidence_min
                .unwrap_or(f32::NAN),
            self.fast_drc_stats
                .repetition_low_confidence_max
                .unwrap_or(f32::NAN),
            self.fast_drc_stats.repetition_low_confidence_values,
            self.fast_drc_stats.repetition_candidates,
            self.fast_drc_stats.repetition_published,
            self.fast_drc_stats.repetition_duplicates,
            self.fast_drc_stats.window_attempts,
            self.fast_drc_stats.window_none,
            self.fast_drc_stats.window_invalid,
            self.fast_drc_stats.window_low_confidence,
            self.fast_drc_stats.window_low_confidence_same_as_last,
            self.fast_drc_stats
                .window_low_confidence_min
                .unwrap_or(f32::NAN),
            self.fast_drc_stats
                .window_low_confidence_max
                .unwrap_or(f32::NAN),
            self.fast_drc_stats.window_low_confidence_values,
            self.fast_drc_stats.window_published,
            self.fast_drc_last_published_slot
                .map(|last| last.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.fast_drc_last_published_value
                .map(|value| format!("0x{value:x}"))
                .unwrap_or_else(|| "none".to_string()),
            self.fast_drc_stats.max_publish_gap_slots,
        );
        self.fast_drc_stats = FastDrcStats {
            window_start_slot: Some(slot),
            ..FastDrcStats::default()
        };
    }
}
