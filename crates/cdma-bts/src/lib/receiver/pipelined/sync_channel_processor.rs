use cdma_common::bits::Bitstream;
use log::{debug, trace};
use num_complex::Complex32;

use crate::receiver::sync::{SyncChannelMessage, SyncFrameReader};

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

/// Sync Channel processor that decodes Sync Channel messages from
/// the decoded bit stream produced by the Viterbi decoder.
///
/// **Frame alignment**: The sync channel transmits 32-bit frames at 1200 bps.
/// The processor accumulates decoded bits and searches for the correct 32-bit
/// frame boundary by trying all 32 offsets and selecting the one that produces
/// CRC30-valid sync frames.  A single CRC30 match (false-positive probability
/// ~10^-9) is sufficient to lock alignment.
///
/// **Bit polarity**: Expected to be handled upstream (e.g. deinterleaver/viterbi lock hints).
pub struct SyncChannelProcessor {
    /// Stateful frame reassembler fed after alignment is found.
    sync_reader: SyncFrameReader,
    /// Accumulated decoded bits waiting for alignment or frame processing.
    bits: Vec<u8>,
    /// Whether the 32-bit frame boundary has been determined.
    aligned: bool,
    /// Whether decoded bits should be inverted before frame parsing.
    aligned_invert: bool,
    /// Running count of successfully parsed sync messages.
    sync_message_count: usize,
    /// Chip-start value for the next frame to be emitted (in chip-rate units).
    next_chip: usize,
    /// Chip-rate chips per decoded bit (derived from sample_rate_hz).
    chips_per_bit: usize,
    /// Effective sample rate of the input bit stream.
    input_sample_rate_hz: f64,
    /// Optional upstream tag that triggers a full decoder reset.
    reset_on_tag: Option<&'static str>,
    /// Whether an upstream lock has been observed in current run.
    upstream_locked: bool,
    /// One-shot frame shift to apply immediately after first upstream lock.
    pending_upstream_shift: Option<usize>,
    /// Enable downstream rescue alignment when upstream lock does not decode.
    rescue_alignment: bool,
    /// Bit budget before rescue alignment attempt.
    rescue_after_bits: usize,
    /// Bits consumed since upstream lock for rescue trigger.
    bits_since_upstream_lock: usize,
    /// Optional compatibility mode: allow local alignment search if no upstream lock hints exist.
    allow_self_alignment: bool,
    /// Pipeline chip_start of the SOM (Start of Message) frame.
    som_chip: Option<usize>,
    /// Number of 32-bit frames consumed since the SOM frame (inclusive).
    frames_since_som: usize,

    frame_buf: Vec<Bitstream>,
}

/// Minimum accumulated bits before attempting frame alignment.
/// A complete Sync Channel Message occupies ~8 frames = 256 bits;
/// we need at least that many to have a chance of seeing a CRC-valid frame
/// at the correct offset.
//const ALIGN_MIN_BITS: usize = 32 * 8;
const ALIGN_MIN_BITS: usize = 32 * 2;

/// Maximum bits retained when alignment has not yet been found.
/// Prevents unbounded memory growth while the processor waits for
/// decodable data.
const ALIGN_MAX_RETAIN: usize = 32 * 128;

impl SyncChannelProcessor {
    pub fn new() -> Self {
        Self {
            sync_reader: SyncFrameReader::new(),
            bits: Vec::new(),
            aligned: false,
            aligned_invert: false,
            next_chip: 0,
            chips_per_bit: 1,
            sync_message_count: 0,
            input_sample_rate_hz: 0.0,
            reset_on_tag: None,
            upstream_locked: false,
            pending_upstream_shift: None,
            rescue_alignment: false,
            rescue_after_bits: 32 * 8,
            bits_since_upstream_lock: 0,
            allow_self_alignment: false,
            som_chip: None,
            frames_since_som: 0,
            frame_buf: vec![],
        }
    }

    /// Reset internal framing/alignment state when `tag` is present with value 1.
    ///
    /// Useful when upstream timing lock is lost/reacquired.
    pub fn with_reset_on_tag(mut self, tag: &'static str) -> Self {
        self.reset_on_tag = Some(tag);
        self
    }

    /// Enable legacy local shift search (disabled by default).
    pub fn with_self_alignment(mut self, enable: bool) -> Self {
        self.allow_self_alignment = enable;
        self
    }

    /// Enable/disable rescue alignment search while upstream lock is active.
    pub fn with_rescue_alignment(mut self, enable: bool) -> Self {
        self.rescue_alignment = enable;
        self
    }

    /// Set rescue trigger window in decoded bits.
    pub fn with_rescue_after_bits(mut self, bits: usize) -> Self {
        self.rescue_after_bits = bits.max(32);
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
        self.aligned = false;
        self.aligned_invert = false;
        self.next_chip = 0;
        self.upstream_locked = false;
        self.pending_upstream_shift = None;
        self.bits_since_upstream_lock = 0;
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

    /// Evaluate a candidate frame alignment.
    ///
    /// Returns `(sync_messages_parsed, crc_valid_frames)` for the given
    /// `shift` (bit offset into the stream) and optional `invert` (XOR all bits).
    pub fn evaluate_alignment(bits: &[u8], shift: usize, invert: bool) -> (usize, usize) {
        let mut reader = SyncFrameReader::new();
        let mut crc_valid_frames = 0usize;
        let mut sync_messages = 0usize;

        for chunk in bits[shift..].chunks_exact(32) {
            let frame_bits = if invert {
                chunk.iter().map(|b| *b ^ 1).collect::<Vec<_>>()
            } else {
                chunk.to_vec()
            };
            let mut frame = Bitstream::new_init(&frame_bits);
            if let Ok(Some(sync_frame)) = reader.process(&mut frame) {
                if sync_frame.crc_valid {
                    crc_valid_frames += 1;
                    if let Ok(Some(_)) = SyncChannelMessage::parse_frame(sync_frame) {
                        sync_messages += 1;
                    }
                }
            }
        }
        (sync_messages, crc_valid_frames)
    }

    /// Search all 32 possible frame offsets and pick the one that maximises
    /// CRC-valid frame count.  A single CRC30 match is sufficient evidence
    /// of correct alignment (false-positive ~10^-9).
    fn find_alignment(&mut self) -> bool {
        if self.bits.len() < ALIGN_MIN_BITS {
            return false;
        }

        let mut best_shift = 0usize;
        let mut best_invert = false;
        let mut best_valid = 0usize;
        let mut best_msgs = 0usize;

        for invert in [false, true] {
            for shift in 0..32 {
                let (msgs, valid) = Self::evaluate_alignment(&self.bits, shift, invert);
                // Prefer more CRC-valid frames; break ties by parsed message count.
                if valid > best_valid || (valid == best_valid && msgs > best_msgs) {
                    best_valid = valid;
                    best_msgs = msgs;
                    best_shift = shift;
                    best_invert = invert;
                }
            }
        }

        if best_valid > 0 {
            self.bits.drain(..best_shift);
            self.aligned = true;
            self.aligned_invert = best_invert;
            self.sync_reader = SyncFrameReader::new();
            return true;
        }

        // Prevent unbounded growth while waiting for valid data.
        if self.bits.len() > ALIGN_MAX_RETAIN {
            let drop = self.bits.len() - ALIGN_MAX_RETAIN / 2;
            self.bits.drain(..drop);
        }

        false
    }
}

impl PipelineProcessor for SyncChannelProcessor {
    /*fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.next_chip == 0 {
            self.next_chip = block.chip_start;
        }
        self.input_sample_rate_hz = block.sample_rate_hz;
        if let Some(&shift_raw) = block.tags.get("deinterleaver_locked_shift")
            && !self.upstream_locked
        {
            self.upstream_locked = true;
            self.pending_upstream_shift = Some((shift_raw as isize).rem_euclid(32) as usize);
            self.bits.clear();
            self.sync_reader = SyncFrameReader::new();
            self.aligned = true;
            self.aligned_invert = false;
            self.next_chip = block.chip_start;
            self.bits_since_upstream_lock = 0;
        }

        // Convert soft decisions to hard bits (Viterbi output is 0.0 / 1.0).
        let block_bits = block
            .samples
            .iter()
            .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
            .collect::<Vec<_>>();
        let bit_string = block_bits
            .iter()
            .map(|b| if *b == 0 { '0' } else { '1' })
            .collect::<String>();
        eprintln!(
            "mobile_station_bits chip_start={} len={} bits={}",
            block.chip_start,
            block_bits.len(),
            bit_string
        );
        self.bits.extend(block_bits);

        let mut out = Vec::new();
        while self.bits.len() >= 32 {
            let frame_bits: Vec<u8> = self.bits.drain(..32).collect();
            let frame_chip = self.next_chip;
            self.next_chip += 32;

            let mut frame = Bitstream::new_init(&frame_bits);
            if let Ok(Some(sync_frame)) = self.sync_reader.process(&mut frame)
                && let Ok(Some(msg)) = SyncChannelMessage::parse_frame(sync_frame)
            {
                out.push(self.emit_sync_event(frame_chip, msg));
            }
        }

        out
    }*/
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.maybe_reset(&block.tags);
        if self.next_chip == 0 {
            self.next_chip = block.chip_start;
        }
        self.input_sample_rate_hz = block.sample_rate_hz;
        self.chips_per_bit = chips_per_sample(block.sample_rate_hz);
        if let Some(&shift_raw) = block.tags.get("deinterleaver_locked_shift")
            && !self.upstream_locked
        {
            self.upstream_locked = true;
            self.pending_upstream_shift = Some((shift_raw as isize).rem_euclid(32) as usize);
            self.bits.clear();
            self.sync_reader = SyncFrameReader::new();
            self.aligned = true;
            self.aligned_invert = false;
            self.next_chip = block.chip_start;
            self.bits_since_upstream_lock = 0;
            self.som_chip = None;
            self.frames_since_som = 0;
        }

        // Convert soft decisions to hard bits (Viterbi output is 0.0 / 1.0).
        let block_bits = block
            .samples
            .iter()
            .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
            .collect::<Vec<_>>();
        let bit_string = block_bits
            .iter()
            .map(|b| if *b == 0 { '0' } else { '1' })
            .collect::<String>();
        let pn_phase = block.tags.get("pn_phase").copied().unwrap_or(-1);
        trace!(
            "mobile_station_bits_sync chip_start={} len={} bits={},cpb={},pn_phase={}",
            block.chip_start,
            block_bits.len(),
            bit_string,
            self.chips_per_bit,
            pn_phase
        );
        self.bits.extend(block_bits);
        if let Some(shift) = self.pending_upstream_shift
            && self.bits.len() >= shift
        {
            self.bits.drain(..shift);
            self.next_chip += shift * self.chips_per_bit;
            self.pending_upstream_shift = None;
        }

        let rescue_snapshot = if self.upstream_locked && self.rescue_alignment {
            Some((self.bits.clone(), self.next_chip))
        } else {
            None
        };

        if !self.aligned && self.allow_self_alignment {
            self.find_alignment();
        }

        let chips_per_frame = 32 * self.chips_per_bit;

        let mut out = Vec::new();
        let mut consumed_bits = 0usize;
        while self.aligned && self.bits.len() >= 32 {
            let frame_bits: Vec<u8> = self.bits.drain(..32).collect();
            consumed_bits += 32;
            let frame_chip = self.next_chip;
            self.next_chip += chips_per_frame;

            let frame_bits = if self.aligned_invert {
                frame_bits.into_iter().map(|b| b ^ 1).collect()
            } else {
                frame_bits
            };

            // Track SOM (Start of Message) for superframe computation.
            let is_som = frame_bits[0] == 1;
            if is_som {
                self.som_chip = Some(frame_chip);
                self.frames_since_som = 1;
            } else if self.som_chip.is_some() {
                self.frames_since_som += 1;
            }

            let mut frame = Bitstream::new_init(&frame_bits);
            self.frame_buf.push(frame.clone());
            if let Ok(Some(sync_frame)) = self.sync_reader.process(&mut frame)
                && let Ok(Some(msg)) = SyncChannelMessage::parse_frame(sync_frame)
            {
                self.sync_reader = SyncFrameReader::new();
                let mut event = self.emit_sync_event(frame_chip, msg);

                debug!("GOOD FRAME");
                for f in &self.frame_buf {
                    debug!("{}", f);
                }
                self.frame_buf.clear();

                // Emit last superframe end chip (in chip-rate units).
                // Sync messages always start at superframe boundaries (3 frames).
                if let Some(som) = self.som_chip {
                    let num_superframes = (self.frames_since_som + 2) / 3;
                    let last_superframe_end = som + num_superframes * 3 * chips_per_frame;
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

        if self.upstream_locked && self.rescue_alignment {
            println!("rescue!");
            self.bits_since_upstream_lock =
                self.bits_since_upstream_lock.saturating_add(consumed_bits);
            if out.is_empty() && self.bits_since_upstream_lock >= self.rescue_after_bits {
                if let Some((snapshot_bits, snapshot_chip)) = rescue_snapshot {
                    self.bits = snapshot_bits;
                    self.next_chip = snapshot_chip;
                }
                self.sync_reader = SyncFrameReader::new();
                self.aligned = false;
                self.aligned_invert = false;
                self.bits_since_upstream_lock = 0;
                self.som_chip = None;
                self.frames_since_som = 0;
                if self.find_alignment() {
                    while self.aligned && self.bits.len() >= 32 {
                        let frame_bits: Vec<u8> = self.bits.drain(..32).collect();
                        let frame_chip = self.next_chip;
                        self.next_chip += chips_per_frame;
                        let frame_bits = if self.aligned_invert {
                            frame_bits.into_iter().map(|b| b ^ 1).collect()
                        } else {
                            frame_bits
                        };

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

                            if let Some(som) = self.som_chip {
                                let num_superframes = (self.frames_since_som + 2) / 3;
                                let last_superframe_end =
                                    som + num_superframes * 3 * chips_per_frame;
                                event.tags.insert(
                                    "sync_last_superframe_end_chip",
                                    last_superframe_end as i64,
                                );
                                event
                                    .tags
                                    .insert("sync_frame_count", self.frames_since_som as i64);
                            }

                            self.som_chip = None;
                            self.frames_since_som = 0;

                            out.push(event);
                        }
                    }
                } else {
                    // Resume trusting upstream framing on next block.
                    self.aligned = true;
                    self.aligned_invert = false;
                }
            }
        }

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
    fn test_mobile_station_processor_emits_sync_event() {
        let mut p = SyncChannelProcessor::new().with_self_alignment(true);
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
    fn test_mobile_station_processor_aligns_with_offset() {
        let mut p = SyncChannelProcessor::new().with_self_alignment(true);
        let frames = build_sync_frames();

        // Flatten and prepend 5 junk bits to simulate a non-zero frame offset.
        let junk: Vec<u8> = vec![0, 1, 0, 1, 0];
        let all_bits: Vec<u8> = junk
            .iter()
            .copied()
            .chain(frames.into_iter().flatten())
            .collect();

        let block = SampleBlock::new(
            all_bits
                .into_iter()
                .map(|b| Complex32::new(b as f32, 0.0))
                .collect(),
            100,
        );
        let out = p.process_block(block);

        assert!(
            !out.is_empty(),
            "expected sync event after alignment search"
        );
        assert_eq!(Some(&1), out[0].tags.get("ms_sync_event"));
        assert_eq!(Some(&42), out[0].tags.get("sync_sid"));
    }

    #[test]
    fn test_mobile_station_processor_no_lock_on_noise() {
        let mut p = SyncChannelProcessor::new();

        // Feed random-ish bits -- should never produce a sync event.
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

    #[test]
    fn test_mobile_station_processor_applies_upstream_shift_31() {
        let mut p = SyncChannelProcessor::new();
        let frames = build_sync_frames();

        // Create a stream that is one bit early: prepend one extra bit.
        let all_bits: Vec<u8> = std::iter::once(0u8)
            .chain(frames.into_iter().flatten())
            .collect();
        let mut block = SampleBlock::new(
            all_bits
                .into_iter()
                .map(|b| Complex32::new(b as f32, 0.0))
                .collect(),
            0,
        );
        block.tags.insert("deinterleaver_locked_shift", 31);

        let _ = p.process_block(block);
        assert!(p.upstream_locked, "expected upstream lock to latch");
        assert_eq!(None, p.pending_upstream_shift, "shift should be applied");
        assert!(p.aligned, "upstream lock should force framed mode");
        assert!(
            p.bits.len() < 32,
            "framed parser should consume full 32-bit chunks after shift"
        );
    }
}
