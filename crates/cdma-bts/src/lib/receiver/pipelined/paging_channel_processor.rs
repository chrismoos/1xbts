use cdma_common::bits::Bitstream;
use num_complex::Complex32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::{
    layer3::{self, PagingMessage},
    paging::{PagingChannelRate, PagingFrame, PagingFrameReader},
};

use super::{PipelineProcessor, SampleBlock, chips_per_sample};
use log::{debug, trace};

pub struct PagingChannelProcessor {
    reader: PagingFrameReader,
    bits: Vec<u8>,
    half_frame_bits: usize,
    next_chip: usize,
    chips_per_bit: usize,
    decoded_through_chip: Option<Arc<AtomicU64>>,
    input_sample_rate_hz: f64,
    lc_anchor_state: Option<u64>,
    lc_anchor_chip: Option<usize>,
    paging_message_count: usize,
    pilot_energy_ema: f64,
    pilot_energy_samples: usize,
    total_half_frames: usize,
    good_half_frames: usize,
    bad_half_frames: usize,
    completed_messages: usize,
    crc_valid_messages: usize,
    total_bit_errors: usize,
}

impl PagingChannelProcessor {
    pub fn new() -> Self {
        Self::new_with_rate(PagingChannelRate::Rate9600)
    }

    pub fn new_with_rate(rate: PagingChannelRate) -> Self {
        let half_frame_bits = match rate {
            PagingChannelRate::Rate4800 => 48,
            PagingChannelRate::Rate9600 => 96,
        };
        Self {
            reader: PagingFrameReader::new_with_rate(rate),
            bits: Vec::new(),
            half_frame_bits,
            next_chip: 0,
            chips_per_bit: 1,
            decoded_through_chip: None,
            input_sample_rate_hz: 0.0,
            lc_anchor_state: None,
            lc_anchor_chip: None,
            paging_message_count: 0,
            pilot_energy_ema: 0.0,
            pilot_energy_samples: 0,
            total_half_frames: 0,
            good_half_frames: 0,
            bad_half_frames: 0,
            completed_messages: 0,
            crc_valid_messages: 0,
            total_bit_errors: 0,
        }
    }

    pub fn with_decode_progress(mut self, progress: Arc<AtomicU64>) -> Self {
        self.decoded_through_chip = Some(progress);
        self
    }

    fn bits_to_hex(bits: &[u8]) -> String {
        bits.chunks(8)
            .map(|byte| {
                let val = byte.iter().fold(0u8, |acc, &b| (acc << 1) | b);
                format!("{:02x}", val)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn count_ones(bits: &[u8]) -> usize {
        bits.iter().filter(|&&b| b == 1).count()
    }

    fn collect_reader_frames(
        reader: &mut PagingFrameReader,
        result: Result<Option<PagingFrame>, cdma_common::error::Error>,
    ) -> Vec<PagingFrame> {
        let mut frames = Vec::new();

        if let Ok(frame) = result {
            if let Some(frame) = frame {
                frames.push(frame);
            }
            while let Some(frame) = reader.take_completed_frame() {
                frames.push(frame);
            }
        }

        frames
    }

    fn validate_half_frame(frame_num: usize, bits: &[u8], reader_in_message: bool) -> usize {
        let sci = bits[0];
        let body = &bits[1..];

        if sci == 1 {
            // SCI=1: message start — validated later by CRC.
            // We can't check it here, just note it.
            return 0;
        }

        // SCI=0: either message continuation or idle fill.
        if reader_in_message {
            // Continuation of a multi-frame message — data is expected,
            // can't validate until CRC at message completion.
            return 0;
        }

        // SCI=0, no message in progress → should be all-zero fill.
        let bit_errors = Self::count_ones(body);
        if bit_errors > 0 {
            let hex = Self::bits_to_hex(bits);
            trace!(
                "half_frame #{}: BAD IDLE  {} bit errors  hex=[{}]",
                frame_num, bit_errors, hex
            );
        }
        bit_errors
    }

    fn print_paging_message(count: usize, frame: &PagingFrame, pilot_energy: f64) {
        let bits = frame.data.bits();
        let hex = Self::bits_to_hex(bits);

        // Extract raw PD + MSG_TYPE for header display
        let raw_type = if bits.len() >= 8 {
            bits[0..8].iter().fold(0u8, |acc, &b| (acc << 1) | b)
        } else {
            0
        };
        let pd = raw_type >> 6;
        let msg_type = raw_type & 0x3F;

        if !frame.crc_valid {
            debug!(
                "paging #{}: BAD CRC  PD={} MSG_TYPE={} ({}) len={} pilot={:.1} hex={}",
                count,
                pd,
                msg_type,
                layer3::msg_type_name(msg_type),
                bits.len(),
                pilot_energy,
                hex,
            );
            return;
        }

        debug!("========================================");
        debug!("PAGING MESSAGE #{} (pilot={:.1})", count, pilot_energy);
        debug!("  CRC valid: true");
        debug!(
            "  PDU length: {} bits ({} bytes)",
            bits.len(),
            bits.len() / 8
        );
        debug!(
            "  PD: {}  MSG_TYPE: {} ({})",
            pd,
            msg_type,
            layer3::msg_type_name(msg_type)
        );
        debug!("  PDU hex: {}", hex);

        match PagingMessage::decode(&frame.data) {
            Ok(msg) => msg.print(),
            Err(e) => debug!("  [decode error: {}]", e),
        }
        debug!("========================================");
    }

    fn emit_paging_event(&mut self, chip_start: usize, frame: PagingFrame) -> SampleBlock {
        self.paging_message_count += 1;
        let payload_samples = frame
            .data
            .bits()
            .iter()
            .map(|b| Complex32::new(*b as f32, 0.0))
            .collect::<Vec<_>>();
        let mut out = SampleBlock::new(payload_samples, chip_start)
            .with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("paging_event", 1);
        out.tags
            .insert("paging_message_count", self.paging_message_count as i64);
        out.tags.insert("paging_crc_valid", frame.crc_valid as i64);
        out.tags
            .insert("paging_payload_bits", frame.data.len() as i64);

        // Parse PD + MSG_TYPE from the PDU
        if frame.data.len() >= 8 {
            let mut data_copy = frame.data.clone();
            if let Ok(pd_and_type) = data_copy.read_bits(8) {
                let msg_type = (pd_and_type & 0x3F) as i64;
                out.tags.insert("paging_msg_type", msg_type);
            }
        }

        out
    }

    fn lc_state_at_chip(&self, chip: usize) -> Option<u64> {
        let anchor_state = self.lc_anchor_state?;
        let anchor_chip = self.lc_anchor_chip?;
        let delta_chips = chip.saturating_sub(anchor_chip);
        let mut lc_gen = LongCodeGenerator::new(0);
        lc_gen.set_state(anchor_state);
        lc_gen.advance_chips(delta_chips);
        Some(lc_gen.state())
    }
}

impl Drop for PagingChannelProcessor {
    fn drop(&mut self) {
        debug!("========== PagingChannelProcessor STATS ==========");
        debug!("  total half-frames:      {}", self.total_half_frames);
        debug!("  good half-frames:       {}", self.good_half_frames);
        debug!("  bad half-frames:        {}", self.bad_half_frames);
        debug!("  total bit errors:       {}", self.total_bit_errors);
        debug!(
            "  avg errors/bad frame:   {:.1}",
            if self.bad_half_frames > 0 {
                self.total_bit_errors as f64 / self.bad_half_frames as f64
            } else {
                0.0
            }
        );
        debug!("  completed messages:     {}", self.completed_messages);
        debug!("  CRC-valid messages:     {}", self.crc_valid_messages);
        debug!("  emitted paging events:  {}", self.paging_message_count);
        debug!("====================================================");
    }
}

impl PipelineProcessor for PagingChannelProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.next_chip == 0 {
            self.next_chip = block.chip_start;
        }
        self.input_sample_rate_hz = block.sample_rate_hz;
        self.chips_per_bit = chips_per_sample(block.sample_rate_hz).max(1);
        if let (Some(&state), Some(&chip)) = (
            block.tags.get("lc_state_at_chip"),
            block.tags.get("lc_state_chip_start"),
        ) {
            self.lc_anchor_state = Some(state as u64);
            self.lc_anchor_chip = Some(chip as usize);
        }

        if let Some(&pe) = block.tags.get("pilot_energy_x1000") {
            let energy = pe as f64 / 1000.0;
            if self.pilot_energy_samples == 0 {
                self.pilot_energy_ema = energy;
            } else {
                self.pilot_energy_ema = 0.1 * energy + 0.9 * self.pilot_energy_ema;
            }
            self.pilot_energy_samples += 1;
        }

        self.bits
            .extend(block.samples.iter().map(|s| u8::from(s.re >= 0.5)));

        let mut out = Vec::new();
        while self.bits.len() >= self.half_frame_bits {
            let half_frame: Vec<u8> = self.bits.drain(..self.half_frame_bits).collect();
            let frame_chip = self.next_chip;
            self.next_chip += self.half_frame_bits * self.chips_per_bit;
            self.total_half_frames += 1;
            let frame_num = self.total_half_frames;

            if half_frame[0] == 1 {
                if let Some(lc_state) = self.lc_state_at_chip(frame_chip) {
                    trace!(
                        "rx_fpch_boundary chip={} lc_state=0x{:x} chips_per_bit={}",
                        frame_chip, lc_state, self.chips_per_bit
                    );
                } else {
                    trace!(
                        "rx_fpch_boundary chip={} lc_state=unknown chips_per_bit={}",
                        frame_chip, self.chips_per_bit
                    );
                }
            }

            // Validate the half-frame before the reader consumes it: whether it
            // is idle fill depends on the reader not already being mid-message.
            let frame_errors =
                Self::validate_half_frame(frame_num, &half_frame, self.reader.in_message());
            if frame_errors > 0 {
                self.bad_half_frames += 1;
                self.total_bit_errors += frame_errors;
            } else {
                self.good_half_frames += 1;
            }

            let mut hf = Bitstream::new_init(&half_frame);
            trace!("paging half-frame: {}", hf);

            let result = self.reader.process(&mut hf);
            if let Some(progress) = &self.decoded_through_chip {
                progress.store(self.next_chip as u64, Ordering::Relaxed);
            }

            let frames = Self::collect_reader_frames(&mut self.reader, result);
            self.completed_messages += frames.len();
            self.crc_valid_messages += frames.iter().filter(|f| f.crc_valid).count();

            for paging_frame in frames {
                Self::print_paging_message(
                    self.paging_message_count + 1,
                    &paging_frame,
                    self.pilot_energy_ema,
                );
                if paging_frame.crc_valid {
                    out.push(self.emit_paging_event(frame_chip, paging_frame));
                }
            }
        }

        out
    }

    fn name(&self) -> &'static str {
        "PagingChannelProcessor"
    }
}

#[cfg(test)]
mod decode_progress_tests {
    use super::*;
    use num_complex::Complex32;

    #[test]
    fn progress_advances_only_after_a_complete_half_frame() {
        let progress = Arc::new(AtomicU64::new(0));
        let mut processor = PagingChannelProcessor::new_with_rate(PagingChannelRate::Rate9600)
            .with_decode_progress(progress.clone());
        let first =
            SampleBlock::new(vec![Complex32::new(0.0, 0.0); 95], 1280).with_sample_rate_hz(9600.0);
        processor.process_block(first);
        assert_eq!(progress.load(Ordering::Relaxed), 0);
        let last =
            SampleBlock::new(vec![Complex32::new(0.0, 0.0)], 13440).with_sample_rate_hz(9600.0);
        processor.process_block(last);
        assert_eq!(progress.load(Ordering::Relaxed), 1280 + 96 * 128);
    }
}
