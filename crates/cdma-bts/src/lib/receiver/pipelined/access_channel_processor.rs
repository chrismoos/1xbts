use cdma_common::bits::Bitstream;
use log::{debug, info};
use num_complex::Complex32;

use crate::receiver::access::{AccessFrame, AccessFrameReader};
use crate::receiver::access_pdu::ReverseAccessPdu;

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

/// Access Channel processor for decoded R-ACH bit streams.
///
/// Expects decoded bits (0.0/1.0 samples) at 96 bits per 20 ms frame.
/// Each frame is split into:
/// - 88 information bits (SAR fragment)
/// - 8 tail bits (encoder termination)
pub struct AccessChannelProcessor {
    reader: AccessFrameReader,
    bits: Vec<u8>,
    next_chip: usize,
    input_sample_rate_hz: f64,
    chips_per_bit: usize,
    access_message_count: usize,
    preamble_frames: usize,
    near_preamble_logs: usize,
}

impl AccessChannelProcessor {
    fn format_access_pdu(bits: &Bitstream) -> Result<String, String> {
        ReverseAccessPdu::decode(bits).map(|pdu| pdu.summary())
    }

    fn copy_context_tags(
        out: &mut SampleBlock,
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) {
        for key in [
            "finger_id",
            "pilot_phase",
            "pn_phase",
            "absolute_chip_start",
            "absolute_sample_start",
            "reverse_access_lc_acquired",
            "reverse_access_lc_chip_delta",
            "access_frame_soft_avg_abs_milli",
            "access_frame_soft_peak_abs_milli",
            "access_frame_weak_soft_bits",
            "finger_snr_mdb",
            "finger_signal_power_mdb",
            "finger_raw_power_mdb",
        ] {
            if let Some(value) = upstream_tags.get(key).copied() {
                out.tags.insert(key, value);
            }
        }
    }

    fn adjust_absolute_chip_tag(
        out: &mut SampleBlock,
        upstream_chip_start: usize,
        event_chip_start: usize,
    ) {
        let Some(absolute_chip_start) = out.tags.get_mut("absolute_chip_start") else {
            return;
        };
        let chip_delta = event_chip_start as i64 - upstream_chip_start as i64;
        *absolute_chip_start = absolute_chip_start.saturating_add(chip_delta);
    }

    pub fn new() -> Self {
        Self {
            reader: AccessFrameReader::new(),
            bits: Vec::new(),
            next_chip: 0,
            input_sample_rate_hz: 0.0,
            chips_per_bit: 1,
            access_message_count: 0,
            preamble_frames: 0,
            near_preamble_logs: 0,
        }
    }

    fn initialize_timing_from_first_block(&mut self, block: &SampleBlock) {
        if self.next_chip == 0 {
            self.next_chip = block.chip_start;
        }
    }

    fn emit_access_event(
        &mut self,
        chip_start: usize,
        upstream_chip_start: usize,
        frame: AccessFrame,
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) -> SampleBlock {
        self.access_message_count += 1;
        let payload_samples = frame
            .data
            .bits()
            .iter()
            .map(|b| Complex32::new(*b as f32, 0.0))
            .collect::<Vec<_>>();
        let mut out = SampleBlock::new(payload_samples, chip_start)
            .with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("access_event", 1);
        out.tags
            .insert("access_message_count", self.access_message_count as i64);
        out.tags.insert("access_crc_valid", frame.crc_valid as i64);
        out.tags
            .insert("access_msg_length_octets", frame.msg_length_octets as i64);
        out.tags
            .insert("access_payload_bits", frame.data.len() as i64);
        out.tags
            .insert("access_frame_quality", if frame.crc_valid { 1 } else { 0 });
        out.tags
            .insert("access_preamble_frames", self.preamble_frames as i64);
        Self::copy_context_tags(&mut out, upstream_tags);
        Self::adjust_absolute_chip_tag(&mut out, upstream_chip_start, chip_start);

        if frame.data.len() >= 8 {
            let mut data = frame.data.clone();
            if let Ok(pd_and_type) = data.read_bits(8) {
                out.tags.insert("access_pd", (pd_and_type >> 6) as i64);
                out.tags
                    .insert("access_msg_type", (pd_and_type & 0x3f) as i64);
            }
        }

        out
    }

    fn emit_preamble_event(
        &self,
        chip_start: usize,
        upstream_chip_start: usize,
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
        info_bits: &[u8],
    ) -> SampleBlock {
        let payload_samples = info_bits
            .iter()
            .map(|b| Complex32::new(*b as f32, 0.0))
            .collect::<Vec<_>>();
        let mut out = SampleBlock::new(payload_samples, chip_start)
            .with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("access_preamble_detected", 1);
        out.tags
            .insert("access_preamble_frames", self.preamble_frames as i64);
        out.tags.insert("access_frame_quality", 0);
        out.tags.insert(
            "access_preamble_info_ones",
            info_bits.iter().filter(|b| **b != 0).count() as i64,
        );
        Self::copy_context_tags(&mut out, upstream_tags);
        Self::adjust_absolute_chip_tag(&mut out, upstream_chip_start, chip_start);
        out
    }
}

impl PipelineProcessor for AccessChannelProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.input_sample_rate_hz = block.sample_rate_hz;
        self.chips_per_bit = chips_per_sample(block.sample_rate_hz);
        self.initialize_timing_from_first_block(&block);
        //assert_eq!(0, block.samples.len());

        self.bits.extend(
            block
                .samples
                .iter()
                .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 }),
        );

        let mut out = Vec::new();
        while self.bits.len() >= 96 {
            let frame_bits: Vec<u8> = self.bits.drain(..96).collect();
            let frame_chip = self.next_chip;
            self.next_chip += 96 * self.chips_per_bit;

            let info_bits = &frame_bits[..88];
            let ones = info_bits.iter().filter(|b| **b != 0).count();
            if self.reader.is_idle()
                && ((ones > 0 && ones <= 8) || ones >= 80)
                && self.near_preamble_logs < 32
            {
                let preview = info_bits[..info_bits.len().min(32)]
                    .iter()
                    .map(|b| if *b == 0 { '0' } else { '1' })
                    .collect::<String>();
                let mode = if ones >= 80 {
                    "near_all_ones"
                } else {
                    "near_all_zero"
                };
                info!(
                    "access_preamble_candidate: chip={} mode={} ones={} zeros={} preamble_frames={} lc_delta={} pilot_phase={} lc_acquired={} preview={}",
                    frame_chip,
                    mode,
                    ones,
                    info_bits.len().saturating_sub(ones),
                    self.preamble_frames,
                    block
                        .tags
                        .get("reverse_access_lc_chip_delta")
                        .copied()
                        .unwrap_or(0),
                    block.tags.get("pilot_phase").copied().unwrap_or(-1),
                    block
                        .tags
                        .get("reverse_access_lc_acquired")
                        .copied()
                        .unwrap_or(0),
                    preview,
                );
                self.near_preamble_logs = self.near_preamble_logs.saturating_add(1);
            }
            if self.reader.is_idle() && info_bits.iter().all(|b| *b == 0) {
                self.preamble_frames = self.preamble_frames.saturating_add(1);
                self.reader.reset();
                let hex = bits_to_hex(&frame_bits);
                debug!(
                    "access_preamble_frame: chip={} preamble_frames={} hex={}",
                    frame_chip, self.preamble_frames, hex,
                );
                out.push(self.emit_preamble_event(
                    frame_chip,
                    block.chip_start,
                    &block.tags,
                    info_bits,
                ));
                continue;
            }

            let mut info = Bitstream::new_init(info_bits);
            if let Ok(Some(access_frame)) = self.reader.process(&mut info) {
                let payload_hex = bits_to_hex(access_frame.data.bits());
                let decoded = if access_frame.crc_valid {
                    match Self::format_access_pdu(&access_frame.data) {
                        Ok(summary) => Some(summary),
                        Err(err) => Some(format!("decode_error={err}")),
                    }
                } else {
                    None
                };
                if let Some(decoded) = decoded {
                    info!(
                        "access_data_frame: chip={} crc={} msg_len={} payload_bits={} frame_avg_abs={} frame_peak_abs={} weak_soft_bits={} hex={} decoded={}",
                        frame_chip,
                        access_frame.crc_valid,
                        access_frame.msg_length_octets,
                        access_frame.data.len(),
                        block
                            .tags
                            .get("access_frame_soft_avg_abs_milli")
                            .copied()
                            .unwrap_or(-1),
                        block
                            .tags
                            .get("access_frame_soft_peak_abs_milli")
                            .copied()
                            .unwrap_or(-1),
                        block
                            .tags
                            .get("access_frame_weak_soft_bits")
                            .copied()
                            .unwrap_or(-1),
                        payload_hex,
                        decoded,
                    );
                } else {
                    debug!(
                        "access_data_frame: chip={} crc={} msg_len={} payload_bits={} frame_avg_abs={} frame_peak_abs={} weak_soft_bits={} hex={}",
                        frame_chip,
                        access_frame.crc_valid,
                        access_frame.msg_length_octets,
                        access_frame.data.len(),
                        block
                            .tags
                            .get("access_frame_soft_avg_abs_milli")
                            .copied()
                            .unwrap_or(-1),
                        block
                            .tags
                            .get("access_frame_soft_peak_abs_milli")
                            .copied()
                            .unwrap_or(-1),
                        block
                            .tags
                            .get("access_frame_weak_soft_bits")
                            .copied()
                            .unwrap_or(-1),
                        payload_hex,
                    );
                }
                out.push(self.emit_access_event(
                    frame_chip,
                    block.chip_start,
                    access_frame,
                    &block.tags,
                ));
                self.preamble_frames = 0;
            }
        }

        out
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.bits.clear();
        self.reader.reset();
        self.preamble_frames = 0;
        self.near_preamble_logs = 0;
        Vec::new()
    }
}

/// Pack a slice of bit values (0/1 u8s) into a hex string.
fn bits_to_hex(bits: &[u8]) -> String {
    let bytes: Vec<u8> = bits
        .chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &b)| acc | ((b & 1) << (7 - i)))
        })
        .collect();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;
    use num_complex::Complex32;

    use super::AccessChannelProcessor;
    use crate::{
        lac::crc30,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_access_channel_processor_emits_event_for_valid_crc() {
        let payload_bits = vec![
            0, 0, 1, 1, 0, 1, 0, 1, // PD+MSG_TYPE
            1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1,
        ];
        let msg_len_octets = ((8 + payload_bits.len() + 30) / 8) as u8;

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&payload_bits));
        let crc = crc30(&crc_scope);

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&Bitstream::new_init(&payload_bits));
        body.write_u32(crc, 30);
        let body_bits = body.bits().to_vec();

        let mut framed_bits = Vec::new();
        let mut rem = body_bits.as_slice();
        while !rem.is_empty() {
            let take = rem.len().min(88);
            let mut frame_info = rem[..take].to_vec();
            if take < 88 {
                frame_info.extend(std::iter::repeat(0u8).take(88 - take));
            }
            // Append 8 tail bits.
            frame_info.extend(std::iter::repeat(0u8).take(8));
            framed_bits.extend(frame_info);
            rem = &rem[take..];
        }

        let mut p = AccessChannelProcessor::new();
        let block = SampleBlock::new(
            framed_bits
                .into_iter()
                .map(|b| Complex32::new(b as f32, 0.0))
                .collect(),
            0,
        );
        let out = p.process_block(block);

        assert_eq!(1, out.len());
        assert_eq!(Some(&1), out[0].tags.get("access_event"));
        assert_eq!(Some(&1), out[0].tags.get("access_crc_valid"));
        assert_eq!(
            Some(&(msg_len_octets as i64)),
            out[0].tags.get("access_msg_length_octets")
        );
        assert_eq!(
            Some(&(payload_bits.len() as i64)),
            out[0].tags.get("access_payload_bits")
        );
        assert_eq!(payload_bits.len(), out[0].samples.len());
    }

    #[test]
    fn test_access_channel_processor_emits_preamble_detection_event() {
        let mut p = AccessChannelProcessor::new();
        let block =
            SampleBlock::new(vec![Complex32::new(0.0, 0.0); 96], 0).with_sample_rate_hz(4_800.0);

        let out = p.process_block(block);

        assert_eq!(1, out.len());
        assert_eq!(Some(&1), out[0].tags.get("access_preamble_detected"));
        assert_eq!(Some(&1), out[0].tags.get("access_preamble_frames"));
        assert_eq!(88, out[0].samples.len());
    }
}
