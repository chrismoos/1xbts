use cdma_common::bits::Bitstream;
use log::trace;
use num_complex::Complex32;

use crate::receiver::sync::{SyncChannelMessage, SyncFrameReader};

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

/// Sync frames and interleaver blocks start at the pilot PN roll (C.S0002-E §3.1.3.3.1).
pub struct SyncChannelProcessor {
    sync_reader: SyncFrameReader,
    bits: Vec<u8>,
    sync_message_count: usize,
    next_chip: Option<usize>,
    chips_per_bit: usize,
    input_sample_rate_hz: f64,
    reset_on_tag: Option<&'static str>,
    som_chip: Option<usize>,
    frames_since_som: usize,
}

const SYNC_FRAME_BITS: usize = 32;
/// A sync channel superframe is three frames (80 ms).
const SYNC_FRAMES_PER_SUPERFRAME: usize = 3;

impl SyncChannelProcessor {
    pub fn new() -> Self {
        Self {
            sync_reader: SyncFrameReader::new(),
            bits: Vec::new(),
            next_chip: None,
            chips_per_bit: 1,
            sync_message_count: 0,
            input_sample_rate_hz: 0.0,
            reset_on_tag: None,
            som_chip: None,
            frames_since_som: 0,
        }
    }

    pub fn with_reset_on_tag(mut self, tag: &'static str) -> Self {
        self.reset_on_tag = Some(tag);
        self
    }

    fn maybe_reset(&mut self, tags: &std::collections::HashMap<&'static str, i64>) {
        let should_reset = self
            .reset_on_tag
            .and_then(|tag| tags.get(tag))
            .copied()
            .unwrap_or(0)
            == 1;
        if !should_reset {
            return;
        }
        self.sync_reader = SyncFrameReader::new();
        self.bits.clear();
        self.next_chip = None;
        self.som_chip = None;
        self.frames_since_som = 0;
    }

    fn emit_sync_event(&mut self, chip_start: usize, msg: SyncChannelMessage) -> SampleBlock {
        self.sync_message_count += 1;
        let mut out = SampleBlock::new(vec![Complex32::new(1.0, 0.0)], chip_start)
            .with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("ms_sync_event", 1);
        out.tags
            .insert("ms_sync_message_count", self.sync_message_count as i64);
        out.tags.insert("sync_msg_type", msg.msg_type as i64);
        out.tags.insert("sync_pd", msg.pd as i64);
        out.tags.insert("sync_p_rev", msg.p_rev as i64);
        out.tags.insert("sync_min_p_rev", msg.min_p_rev as i64);
        out.tags.insert("sync_sid", msg.sid as i64);
        out.tags.insert("sync_nid", msg.nid as i64);
        out.tags.insert("sync_pilot_pn", msg.pilot_pn as i64);
        out.tags.insert("sync_lc_state", msg.lc_state as i64);
        out.tags.insert("sync_sys_time", msg.sys_time as i64);
        out.tags.insert("sync_lp_sec", msg.lp_sec as i64);
        out.tags.insert("sync_ltm_off", msg.ltm_off as i64);
        out.tags.insert("sync_daylt", msg.daylt as i64);
        out.tags.insert("sync_prat", msg.prat as i64);
        out.tags.insert("sync_cdma_freq", msg.cdma_freq as i64);
        out.tags
            .insert("sync_ext_cdma_freq", msg.ext_cdma_freq as i64);
        out
    }
}

impl PipelineProcessor for SyncChannelProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.maybe_reset(&block.tags);
        let mut next_chip = self.next_chip.unwrap_or(block.chip_start);
        self.input_sample_rate_hz = block.sample_rate_hz;
        self.chips_per_bit = chips_per_sample(block.sample_rate_hz);

        let block_bits = block
            .samples
            .iter()
            .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
            .collect::<Vec<_>>();
        trace!(
            "sync_channel_bits chip_start={} len={} bits={} cpb={}",
            block.chip_start,
            block_bits.len(),
            block_bits
                .iter()
                .map(|b| if *b == 0 { '0' } else { '1' })
                .collect::<String>(),
            self.chips_per_bit,
        );
        self.bits.extend(block_bits);

        let chips_per_frame = SYNC_FRAME_BITS * self.chips_per_bit;
        let mut out = Vec::new();
        while self.bits.len() >= SYNC_FRAME_BITS {
            let frame_bits: Vec<u8> = self.bits.drain(..SYNC_FRAME_BITS).collect();
            let frame_chip = next_chip;
            next_chip += chips_per_frame;

            let is_som = frame_bits[0] == 1;
            if is_som {
                self.som_chip = Some(frame_chip);
                self.frames_since_som = 1;
            } else if self.som_chip.is_some() {
                self.frames_since_som += 1;
            }

            let mut frame = Bitstream::new_init(&frame_bits);
            if let Ok(Some(sync_frame)) = self.sync_reader.process(&mut frame)
                && let Ok(Some(msg)) = SyncChannelMessage::parse_frame(sync_frame)
            {
                self.sync_reader = SyncFrameReader::new();
                let mut event = self.emit_sync_event(frame_chip, msg);

                // A sync message always starts on a superframe boundary.
                if let Some(som) = self.som_chip {
                    let num_superframes =
                        self.frames_since_som.div_ceil(SYNC_FRAMES_PER_SUPERFRAME);
                    let last_superframe_end =
                        som + num_superframes * SYNC_FRAMES_PER_SUPERFRAME * chips_per_frame;
                    event.tags.insert("sync_som_start_chip", som as i64);
                    event
                        .tags
                        .insert("sync_last_superframe_end_chip", last_superframe_end as i64);
                    event
                        .tags
                        .insert("sync_frame_count", self.frames_since_som as i64);
                }

                self.som_chip = None;
                self.frames_since_som = 0;

                out.push(event);
            }
        }

        self.next_chip = Some(next_chip);
        out
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;
    use num_complex::Complex32;

    use super::SyncChannelProcessor;
    use crate::{
        lac::crc30,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    fn build_sync_frames() -> Vec<Vec<u8>> {
        let mut payload = Bitstream::new();
        payload.write_u8(0, 2); // PD
        payload.write_u8(1, 6); // MSG_TYPE = Sync Channel Message
        payload.write_u8(6, 8); // P_REV
        payload.write_u8(6, 8); // MIN_P_REV
        payload.write_u64(42, 15); // SID
        payload.write_u64(7, 16); // NID
        payload.write_u64(123, 9); // PILOT_PN
        payload.write_u64(0x123456789ab, 42); // LC_STATE
        payload.write_u64(0xabcdef, 36); // SYS_TIME
        payload.write_u8(0, 8); // LP_SEC
        payload.write_u8(0, 6); // LTM_OFF
        payload.write_u8(0, 1); // DAYLT
        payload.write_u8(3, 2); // PRAT
        payload.write_u64(384, 11); // CDMA_FREQ
        payload.write_u64(0, 11); // EXT_CDMA_FREQ
        // Align so [MSG_LENGTH(8)+payload+CRC30] is octet-aligned.
        while payload.len() % 8 != 2 {
            payload.write_u8(0, 1);
        }

        let msg_len_octets = ((8 + payload.len() + 30) / 8) as u8;
        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&payload);
        let crc = crc30(&crc_scope);

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&payload);
        body.write_u32(crc, 30);

        let mut bits = body.bits().to_vec();
        let mut frames = Vec::new();
        let mut first = true;
        while !bits.is_empty() {
            let mut frame = Vec::with_capacity(32);
            frame.push(if first { 1 } else { 0 });
            first = false;
            for _ in 0..31 {
                frame.push(if bits.is_empty() { 0 } else { bits.remove(0) });
            }
            frames.push(frame);
        }
        frames
    }

    #[test]
    fn sync_channel_processor_emits_sync_event() {
        let mut p = SyncChannelProcessor::new();
        let frames = build_sync_frames();

        let mut out = Vec::new();
        let mut chip = 0usize;
        for frame_bits in frames {
            let block = SampleBlock::new(
                frame_bits
                    .into_iter()
                    .map(|b| Complex32::new(b as f32, 0.0))
                    .collect(),
                chip,
            );
            chip += 32;
            out.extend(p.process_block(block));
        }

        assert!(!out.is_empty(), "expected at least one parsed sync event");
        let evt = &out[0];
        assert_eq!(Some(&1), evt.tags.get("ms_sync_event"));
        assert_eq!(Some(&1), evt.tags.get("sync_msg_type"));
        assert_eq!(Some(&42), evt.tags.get("sync_sid"));
        assert_eq!(Some(&7), evt.tags.get("sync_nid"));
        assert_eq!(Some(&123), evt.tags.get("sync_pilot_pn"));
    }

    #[test]
    fn sync_channel_processor_ignores_noise() {
        let mut p = SyncChannelProcessor::new();

        let noise: Vec<u8> = (0..1024).map(|i| ((i * 7 + 3) % 2) as u8).collect();
        let block = SampleBlock::new(
            noise
                .into_iter()
                .map(|b| Complex32::new(b as f32, 0.0))
                .collect(),
            0,
        );
        let out = p.process_block(block);
        assert!(out.is_empty(), "noise should not produce sync events");
    }
}
