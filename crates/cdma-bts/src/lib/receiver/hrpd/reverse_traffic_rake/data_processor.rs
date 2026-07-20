//! Per-frame reverse Data Channel decoder for the HRPD reverse-traffic
//! sub-chain. Decodes at the RRI-indicated rate and pushes any decoded
//! `HrpdTrafficEvent`s onto the supplied event channel.

use std::time::Instant;

use tokio::sync::mpsc as tokio_mpsc;

use crate::receiver::hrpd::data_decoder::{
    ReverseDataDecoder, ReverseDataRate, traffic_events_from_mac_packet_for_reverse_mac_subtype,
};
use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};
use cdma_common::hrpd::air::HrpdTrafficEvent;
use cdma_common::hrpd::traffic::REVERSE_TRAFFIC_MAC_SUBTYPE3;

use super::despread::{HRPD_SLOT_CHIPS, HRPD_TRAFFIC_SLOTS_PER_FRAME};
use super::finger::{
    TAG_FRAME_OFFSET, TAG_FRAME_START_CHIP, TAG_MAC_INDEX, TAG_PHYSICAL_LAYER_SUBTYPE,
    TAG_PILOT_COHERENCE_X1000, TAG_PILOT_SNR_DB_TENTHS, TAG_Q_SIGN_X1000,
    TAG_REVERSE_TRAFFIC_MAC_SUBTYPE, TAG_UATI,
};
use super::rri_processor::{TAG_RRI_MARGIN_DB_TENTHS, TAG_RRI_RATE_BPS};

pub struct HrpdReverseTrafficDataProcessor {
    event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    frames_seen: u64,
    q_polarity_locked: bool,
    // Reverse frame-error-rate accounting. A frame counts as good when any
    // rate/polarity decodes CRC-clean, and as erased when the Q arm carries
    // data (`data_arm_present`) but no candidate decodes. Pilot-only frames
    // carry no data and are excluded. Window counters reset on each log; total
    // counters are cumulative for the connection.
    fer_window_ok: u32,
    fer_window_erased: u32,
    fer_total_ok: u64,
    fer_total_erased: u64,
    timing_report_start: Instant,
    decode_attempts: u64,
    decode_total_us: u64,
    decode_max_us: u64,
    decode_skipped: u64,
}

const DATA_ARM_PRESENT_DB: f32 = -6.0;
// Once Q polarity is CRC-established, a very low W0 pilot metric means the
// frame timing/phase estimate is not reliable enough for useful FCS decoding.
// Keep setup permissive before lock; skip only established traffic during loss.
const DATA_DECODE_MIN_COHERENCE_AFTER_Q_LOCK: f32 = 0.35;
/// Number of data-bearing reverse frames between reverse-FER log lines (~1.7 s
/// at the 26.67 ms reverse frame period).
const FER_LOG_WINDOW_FRAMES: u32 = 64;
const REVERSE_DATA_DECODE_RATES: [ReverseDataRate; 5] = [
    ReverseDataRate::Kbps9_6,
    ReverseDataRate::Kbps19_2,
    ReverseDataRate::Kbps38_4,
    ReverseDataRate::Kbps76_8,
    ReverseDataRate::Kbps153_6,
];

impl HrpdReverseTrafficDataProcessor {
    pub fn new(event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>) -> Self {
        Self {
            event_tx,
            frames_seen: 0,
            q_polarity_locked: false,
            fer_window_ok: 0,
            fer_window_erased: 0,
            fer_total_ok: 0,
            fer_total_erased: 0,
            timing_report_start: Instant::now(),
            decode_attempts: 0,
            decode_total_us: 0,
            decode_max_us: 0,
            decode_skipped: 0,
        }
    }

    /// Cumulative `(crc_ok, erased)` reverse-frame counts for the connection.
    #[cfg(test)]
    pub(super) fn fer_totals(&self) -> (u64, u64) {
        (self.fer_total_ok, self.fer_total_erased)
    }

    /// Test seam: drive one frame through the reverse-FER accounting.
    #[cfg(test)]
    pub(super) fn test_record_reverse_fer(&mut self, crc_ok: bool, data_present: bool) {
        self.record_reverse_fer(0, 0, crc_ok, data_present, 1.0, 0.0);
    }

    fn record_decode_timing(&mut self, elapsed_us: u64) {
        self.decode_attempts = self.decode_attempts.saturating_add(1);
        self.decode_total_us = self.decode_total_us.saturating_add(elapsed_us);
        self.decode_max_us = self.decode_max_us.max(elapsed_us);
    }

    fn maybe_report_decode_timing(&mut self, mac_index: u8, uati: u32) {
        if self.timing_report_start.elapsed().as_secs_f32() < 1.0 {
            return;
        }
        if self.decode_attempts > 0 || self.decode_skipped > 0 {
            log::trace!(
                "rx_hrpd_traffic[m{}]: data_timing uati=0x{:08x} decode_attempts={} decode_avg_us={} decode_max_us={} skipped={}",
                mac_index,
                uati,
                self.decode_attempts,
                self.decode_total_us / self.decode_attempts.max(1),
                self.decode_max_us,
                self.decode_skipped,
            );
        }
        self.timing_report_start = Instant::now();
        self.decode_attempts = 0;
        self.decode_total_us = 0;
        self.decode_max_us = 0;
        self.decode_skipped = 0;
    }

    /// Account one processed reverse frame toward the reverse FER and emit a
    /// rolling summary every `FER_LOG_WINDOW_FRAMES` data-bearing frames.
    fn record_reverse_fer(
        &mut self,
        mac_index: u8,
        uati: u32,
        crc_ok: bool,
        data_present: bool,
        pilot_coh: f32,
        pilot_snr_db: f32,
    ) {
        if crc_ok {
            self.fer_window_ok = self.fer_window_ok.saturating_add(1);
            self.fer_total_ok = self.fer_total_ok.saturating_add(1);
        } else if data_present {
            self.fer_window_erased = self.fer_window_erased.saturating_add(1);
            self.fer_total_erased = self.fer_total_erased.saturating_add(1);
        } else {
            // Pilot-only frame, no reverse data transmitted; not part of FER.
            return;
        }
        let window = self.fer_window_ok + self.fer_window_erased;
        if window >= FER_LOG_WINDOW_FRAMES {
            let total = self.fer_total_ok + self.fer_total_erased;
            log::info!(
                "rx_hrpd_traffic[m{}]: reverse_fer uati=0x{:08x} window_fer={:.1}% ({}/{}) total_fer={:.1}% ({}/{}) pilot_coh={:.3} pilot_snr={:.1}dB",
                mac_index,
                uati,
                100.0 * f64::from(self.fer_window_erased) / f64::from(window.max(1)),
                self.fer_window_erased,
                window,
                100.0 * self.fer_total_erased as f64 / (total.max(1)) as f64,
                self.fer_total_erased,
                total,
                pilot_coh,
                pilot_snr_db,
            );
            self.fer_window_ok = 0;
            self.fer_window_erased = 0;
        }
    }
}

fn rate_from_bps(bps: i64) -> Option<ReverseDataRate> {
    match bps {
        9_600 => Some(ReverseDataRate::Kbps9_6),
        19_200 => Some(ReverseDataRate::Kbps19_2),
        38_400 => Some(ReverseDataRate::Kbps38_4),
        76_800 => Some(ReverseDataRate::Kbps76_8),
        153_600 => Some(ReverseDataRate::Kbps153_6),
        _ => None,
    }
}

fn reverse_data_decode_candidate_rates(
    rri_rate: Option<ReverseDataRate>,
    physical_layer_subtype: u16,
    data_arm_present: bool,
) -> Vec<ReverseDataRate> {
    // The RRI declares the rate (§13.2.1.3.1.1): when it decodes, trust it
    // and try nothing else. The full-rate scan runs only on subtype-2 frames
    // whose data arm carries energy while the RRI is undetected — the legacy
    // Rev 0 TDM RRI does not exist on the subtype-2 waveform, so the rate is
    // otherwise unknowable there.
    if let Some(rate) = rri_rate {
        return vec![rate];
    }
    if physical_layer_subtype == 2 && data_arm_present {
        return REVERSE_DATA_DECODE_RATES.to_vec();
    }
    Vec::new()
}

fn uses_subtype3_subframe_decoder(
    physical_layer_subtype: u16,
    reverse_traffic_mac_subtype: u16,
) -> bool {
    physical_layer_subtype == 2 && reverse_traffic_mac_subtype == REVERSE_TRAFFIC_MAC_SUBTYPE3
}

/// Per-slot W2/Q-arm power profile in dB relative to the pilot arm. The time
/// structure separates the candidate sources of unexplained quadrature
/// energy: the Data Channel transmits across the whole frame uniformly,
/// while the ACK Channel is gated to the half-slots that answer forward
/// transmissions.
fn data_arm_slot_profile_db(chips: &[num_complex::Complex32]) -> String {
    const W2_4: [f32; 4] = [1.0, 1.0, -1.0, -1.0];
    let mut pilot_power = 0.0f64;
    let mut pilot_n = 0u32;
    for group in chips.chunks_exact(16) {
        let mut acc = 0.0f32;
        for chip in group {
            acc += chip.re;
        }
        pilot_power += f64::from(acc * acc) / 16.0;
        pilot_n += 1;
    }
    let pilot_mean = (pilot_power / f64::from(pilot_n.max(1))).max(1e-12);
    let mut out = Vec::with_capacity(HRPD_TRAFFIC_SLOTS_PER_FRAME);
    for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
        let base = slot * HRPD_SLOT_CHIPS;
        let mut power = 0.0f64;
        let mut n = 0u32;
        for group in chips[base..base + HRPD_SLOT_CHIPS].chunks_exact(4) {
            let mut acc = 0.0f32;
            for (chip, w) in group.iter().zip(W2_4) {
                acc += chip.im * w;
            }
            power += f64::from(acc * acc) / 4.0;
            n += 1;
        }
        let mean = power / f64::from(n.max(1));
        out.push(format!("{:+.0}", 10.0 * (mean / pilot_mean).log10()));
    }
    out.join(",")
}

/// Decode-free presence detector for the reverse Data Channel: the per-4-chip
/// W2 cover on the Q arm of the despread frame, as a power ratio against the
/// W0 pilot arm on I. An AT transmitting data shows up well above unity
/// (data-to-pilot gain is positive at every rate); a pilot-only frame sits
/// near the noise floor. This distinguishes "AT never transmits data" from
/// "data is on the air but the decoder fails" without any FCS decode.
fn data_arm_to_pilot_ratio_db(chips: &[num_complex::Complex32]) -> f32 {
    const W2_4: [f32; 4] = [1.0, 1.0, -1.0, -1.0];
    const W0_16: usize = 16;
    let mut data_power = 0.0f64;
    let mut data_n = 0u32;
    let mut pilot_power = 0.0f64;
    let mut pilot_n = 0u32;
    for group in chips.chunks_exact(4) {
        let mut acc = 0.0f32;
        for (chip, w) in group.iter().zip(W2_4) {
            acc += chip.im * w;
        }
        // Divide by the integration length so a noise-only arm reads the
        // same power on both sides (a pilot-only frame sits near 0 dB minus
        // the pilot's own contribution; real data pushes well positive).
        data_power += f64::from(acc * acc) / 4.0;
        data_n += 1;
    }
    for group in chips.chunks_exact(W0_16) {
        let mut acc = 0.0f32;
        for chip in group {
            acc += chip.re;
        }
        pilot_power += f64::from(acc * acc) / W0_16 as f64;
        pilot_n += 1;
    }
    let data_mean = data_power / f64::from(data_n.max(1));
    let pilot_mean = pilot_power / f64::from(pilot_n.max(1));
    (10.0 * (data_mean / pilot_mean.max(1e-12)).log10()) as f32
}

struct ReverseMacDecode {
    events: Vec<HrpdTrafficEvent>,
    subtype: u16,
}

fn decode_events_with_reverse_mac_candidates(
    uati: u32,
    mac_index: u8,
    payload: &[u8],
    configured_subtype: u16,
    rate: ReverseDataRate,
    polarity_label: &str,
) -> ReverseMacDecode {
    match traffic_events_from_mac_packet_for_reverse_mac_subtype(
        uati,
        mac_index,
        payload,
        configured_subtype,
    ) {
        Ok(events) => ReverseMacDecode {
            events,
            subtype: configured_subtype,
        },
        Err(err) => {
            log::debug!(
                "rx_hrpd_traffic[m{}]: data_mac_parse_miss uati=0x{:08x} rate={:?} polarity={} rtc_mac_subtype=0x{:04x} err={:?}",
                mac_index,
                uati,
                rate,
                polarity_label,
                configured_subtype,
                err,
            );
            ReverseMacDecode {
                events: Vec::new(),
                subtype: configured_subtype,
            }
        }
    }
}

impl PipelineProcessor for HrpdReverseTrafficDataProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let Some(&uati_tag) = block.tags.get(TAG_UATI) else {
            return vec![block];
        };
        let Some(&mac_index_tag) = block.tags.get(TAG_MAC_INDEX) else {
            return vec![block];
        };
        let uati = uati_tag as u32;
        let mac_index = mac_index_tag as u8;
        let physical_layer_subtype = block
            .tags
            .get(TAG_PHYSICAL_LAYER_SUBTYPE)
            .copied()
            .unwrap_or(0) as u16;
        let reverse_traffic_mac_subtype = block
            .tags
            .get(TAG_REVERSE_TRAFFIC_MAC_SUBTYPE)
            .copied()
            .unwrap_or(0) as u16;
        self.frames_seen = self.frames_seen.saturating_add(1);
        let frame_start_chip = block.tags.get(TAG_FRAME_START_CHIP).copied().unwrap_or(0) as u64;
        let frame_start_slot = frame_start_chip / super::despread::HRPD_SLOT_CHIPS as u64;
        let samples_per_chip = (block.sample_rate_hz / 1_228_800.0).round().max(1.0) as u64;
        let air_frame_end_received_at = block.rx_sample_time.and_then(|anchor| {
            anchor.received_at_sample(
                frame_start_chip
                    .saturating_add(super::despread::HRPD_TRAFFIC_FRAME_CHIPS as u64)
                    .saturating_mul(samples_per_chip),
                block.sample_rate_hz,
            )
        });
        let frame_offset = block
            .tags
            .get(TAG_FRAME_OFFSET)
            .copied()
            .unwrap_or((frame_start_slot % 16) as i64) as u8;
        let pilot_coh = block
            .tags
            .get(TAG_PILOT_COHERENCE_X1000)
            .map(|v| *v as f32 / 1000.0)
            .unwrap_or(0.0);
        let pilot_snr_db = block
            .tags
            .get(TAG_PILOT_SNR_DB_TENTHS)
            .map(|v| *v as f32 / 10.0)
            .unwrap_or(f32::NAN);
        let rri_rate = block
            .tags
            .get(TAG_RRI_RATE_BPS)
            .copied()
            .and_then(rate_from_bps);
        let rri_bps = block.tags.get(TAG_RRI_RATE_BPS).copied().unwrap_or(-1);
        let rri_margin_db = block
            .tags
            .get(TAG_RRI_MARGIN_DB_TENTHS)
            .map(|v| *v as f32 / 10.0)
            .unwrap_or(f32::NEG_INFINITY);
        let q_sign = block
            .tags
            .get(TAG_Q_SIGN_X1000)
            .copied()
            .map(|v| if v < 0 { -1.0 } else { 1.0 })
            .unwrap_or(1.0);

        if uses_subtype3_subframe_decoder(physical_layer_subtype, reverse_traffic_mac_subtype) {
            if self.frames_seen <= 8 || self.frames_seen % 64 == 0 {
                log::debug!(
                    "rx_hrpd_traffic[m{}]: skipping legacy reverse data frame decoder for subtype2/rtc-mac-subtype3 uati=0x{:08x}; using subframe HARQ decoder",
                    mac_index,
                    uati,
                );
            }
            return vec![block];
        }

        let data_arm_db = data_arm_to_pilot_ratio_db(&block.samples);
        let pilot_locked = pilot_coh >= super::despread::HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE;
        let data_arm_present = pilot_locked && data_arm_db > DATA_ARM_PRESENT_DB;
        if self.q_polarity_locked && pilot_coh < DATA_DECODE_MIN_COHERENCE_AFTER_Q_LOCK {
            self.decode_skipped = self.decode_skipped.saturating_add(1);
            self.record_reverse_fer(mac_index, uati, false, false, pilot_coh, pilot_snr_db);
            self.maybe_report_decode_timing(mac_index, uati);
            return vec![block];
        }

        let try_alternate_q_polarity = !self.q_polarity_locked;
        let inverted_samples;
        let mut sample_polarities: Vec<(&[num_complex::Complex32], &'static str)> =
            Vec::with_capacity(if try_alternate_q_polarity { 2 } else { 1 });
        if q_sign < 0.0 {
            inverted_samples = invert_q_arm(&block.samples);
            sample_polarities.push((inverted_samples.as_slice(), "qnorm"));
            if try_alternate_q_polarity {
                sample_polarities.push((block.samples.as_slice(), "qraw"));
            }
        } else {
            inverted_samples = invert_q_arm(&block.samples);
            sample_polarities.push((block.samples.as_slice(), "qraw"));
            if try_alternate_q_polarity {
                sample_polarities.push((inverted_samples.as_slice(), "qinv"));
            }
        }

        let candidate_rates =
            reverse_data_decode_candidate_rates(rri_rate, physical_layer_subtype, data_arm_present);
        if candidate_rates.is_empty() {
            self.record_reverse_fer(
                mac_index,
                uati,
                false,
                data_arm_present,
                pilot_coh,
                pilot_snr_db,
            );
            if data_arm_present {
                log::info!(
                    "rx_hrpd_traffic[m{}]: data_decode_skipped_invalid_rri uati=0x{:08x} physical_subtype=0x{:04x} rtc_mac_subtype=0x{:04x} frame_chip={} pilot_coh={:.3} pilot_snr={:.1}dB rri_bps={} rri_margin={:.1}dB data_arm_db={:+.1}",
                    mac_index,
                    uati,
                    physical_layer_subtype,
                    reverse_traffic_mac_subtype,
                    frame_start_chip,
                    pilot_coh,
                    pilot_snr_db,
                    rri_bps,
                    rri_margin_db,
                    data_arm_db,
                );
            }
            self.maybe_report_decode_timing(mac_index, uati);
            return vec![block];
        }

        let mut fcs_results: Vec<(ReverseDataRate, &'static str, bool)> = Vec::new();
        for candidate_rate in candidate_rates {
            for (samples, polarity_label) in &sample_polarities {
                let decode_start = Instant::now();
                let frame = ReverseDataDecoder::for_physical_layer_subtype(
                    candidate_rate,
                    physical_layer_subtype,
                )
                .decode_data_frame_with_timing(
                    samples,
                    frame_start_slot,
                    frame_offset,
                );
                self.record_decode_timing(decode_start.elapsed().as_micros() as u64);
                fcs_results.push((candidate_rate, *polarity_label, frame.crc_ok));
                if frame.crc_ok {
                    self.record_reverse_fer(
                        mac_index,
                        uati,
                        true,
                        data_arm_present,
                        pilot_coh,
                        pilot_snr_db,
                    );
                    if rri_rate != Some(candidate_rate) {
                        log::info!(
                            "rx_hrpd_traffic[m{}]: subtype2 data fallback decoded uati=0x{:08x} physical_subtype=0x{:04x} rtc_mac_subtype=0x{:04x} fallback_rate={:?} rri_bps={} rri_margin={:.1}dB polarity={} pilot_coh={:.3} pilot_snr={:.1}dB data_arm_db={:+.1}",
                            mac_index,
                            uati,
                            physical_layer_subtype,
                            reverse_traffic_mac_subtype,
                            candidate_rate,
                            rri_bps,
                            rri_margin_db,
                            polarity_label,
                            pilot_coh,
                            pilot_snr_db,
                            data_arm_db,
                        );
                    }
                    if !self.q_polarity_locked {
                        log::debug!(
                            "rx_hrpd_traffic[m{}]: q_polarity_lock uati=0x{:08x} polarity={} rate={:?}",
                            mac_index,
                            uati,
                            polarity_label,
                            candidate_rate,
                        );
                    }
                    self.q_polarity_locked = true;
                    let mut decoded = decode_events_with_reverse_mac_candidates(
                        uati,
                        mac_index,
                        &frame.payload,
                        reverse_traffic_mac_subtype,
                        candidate_rate,
                        polarity_label,
                    );
                    for event in &mut decoded.events {
                        if let cdma_common::hrpd::air::HrpdTrafficEvent::Stream1Packet {
                            air_frame_end_received_at: event_air_time,
                            ..
                        } = event
                        {
                            *event_air_time = air_frame_end_received_at;
                        }
                    }
                    if !decoded.events.is_empty() {
                        log::trace!(
                            "rx_hrpd_traffic[m{}]: decoded {} reverse traffic event(s) uati=0x{:08x} physical_subtype=0x{:04x} rtc_mac_subtype=0x{:04x} rate={:?} rri_bps={} polarity={} pilot_coh={:.3} pilot_snr={:.1}dB data_arm_db={:+.1}",
                            mac_index,
                            decoded.events.len(),
                            uati,
                            physical_layer_subtype,
                            decoded.subtype,
                            candidate_rate,
                            rri_bps,
                            polarity_label,
                            pilot_coh,
                            pilot_snr_db,
                            data_arm_db,
                        );
                        if let Some(tx) = self.event_tx.as_ref() {
                            for ev in decoded.events {
                                let _ = tx.send(ev);
                            }
                        }
                    }
                    self.maybe_report_decode_timing(mac_index, uati);
                    return vec![block];
                }
            }
        }
        // No candidate decoded this frame: account it toward reverse FER
        // (erased if the Q arm carried data, otherwise pilot-only/excluded).
        self.record_reverse_fer(
            mac_index,
            uati,
            false,
            data_arm_present,
            pilot_coh,
            pilot_snr_db,
        );
        if (self.frames_seen <= 8 || self.frames_seen % 32 == 0)
            && !fcs_results.iter().any(|(_, _, ok)| *ok)
        {
            let summary = fcs_results
                .iter()
                .map(|(rate, polarity, ok)| {
                    format!("{:?}/{}={}", rate, polarity, if *ok { "ok" } else { "bad" })
                })
                .collect::<Vec<_>>()
                .join(",");
            let line = format!(
                "rx_hrpd_traffic[m{}]: data_fcs_miss uati=0x{:08x} physical_subtype=0x{:04x} rtc_mac_subtype=0x{:04x} frame_chip={} pilot_coh={:.3} pilot_snr={:.1}dB rri_bps={} rri_margin={:.1}dB data_arm_db={:+.1} tried=[{}]",
                mac_index,
                uati,
                physical_layer_subtype,
                reverse_traffic_mac_subtype,
                frame_start_chip,
                pilot_coh,
                pilot_snr_db,
                rri_bps,
                rri_margin_db,
                data_arm_db,
                summary
            );
            if data_arm_present {
                log::info!("{line}");
            } else {
                log::debug!("{line}");
            }
            // Per-slot profile when the arm is hot: uniform across slots
            // points at the Data Channel, half-slot gating at the ACK
            // Channel (which would prove the AT is receiving our forward
            // packets).
            if data_arm_present {
                log::info!(
                    "rx_hrpd_traffic[m{}]: data_arm_slots uati=0x{:08x} frame_chip={} [{}]",
                    mac_index,
                    uati,
                    frame_start_chip,
                    data_arm_slot_profile_db(&block.samples),
                );
            }
        }

        // Pass the block through so any subsequent stages (none today) can
        // still observe the tags.
        self.maybe_report_decode_timing(mac_index, uati);
        vec![block]
    }

    fn name(&self) -> &'static str {
        "HrpdReverseTrafficDataProcessor"
    }
}

fn invert_q_arm(samples: &[num_complex::Complex32]) -> Vec<num_complex::Complex32> {
    samples
        .iter()
        .map(|s| num_complex::Complex32::new(s.re, -s.im))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtype2_data_arm_fallback_tries_all_rates_without_rri() {
        assert_eq!(
            reverse_data_decode_candidate_rates(None, 2, true),
            REVERSE_DATA_DECODE_RATES
        );
    }

    #[test]
    fn detected_rri_rate_is_trusted_without_fallback_scan() {
        assert_eq!(
            reverse_data_decode_candidate_rates(Some(ReverseDataRate::Kbps38_4), 2, true),
            vec![ReverseDataRate::Kbps38_4]
        );
    }

    #[test]
    fn fallback_rates_are_scoped_to_subtype2_data_arm_frames() {
        assert!(reverse_data_decode_candidate_rates(None, 0, true).is_empty());
        assert!(reverse_data_decode_candidate_rates(None, 2, false).is_empty());
        assert_eq!(
            reverse_data_decode_candidate_rates(Some(ReverseDataRate::Kbps9_6), 0, true),
            vec![ReverseDataRate::Kbps9_6]
        );
    }

    #[test]
    fn subtype2_rtc_mac_subtype3_uses_subframe_decoder_not_legacy_frame_decoder() {
        assert!(uses_subtype3_subframe_decoder(
            2,
            REVERSE_TRAFFIC_MAC_SUBTYPE3
        ));
        assert!(!uses_subtype3_subframe_decoder(2, 0));
        assert!(!uses_subtype3_subframe_decoder(
            0,
            REVERSE_TRAFFIC_MAC_SUBTYPE3
        ));
    }
}
