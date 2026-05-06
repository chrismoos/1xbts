use cdma_common::bits::Bitstream;
use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::{
    layer3::{self, PagingMessage},
    paging::{PagingChannelRate, PagingFrame, PagingFrameReader},
};

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Polarity {
    Normal,
    Inverted,
}

const SCORE_DECAY: f64 = 0.90;
const SCORE_CRC_VALID: f64 = 8.0;
const SCORE_CRC_INVALID: f64 = -4.0;
const SCORE_IDLE_BONUS: f64 = 2.0;
const SCORE_IDLE_ERR_WEIGHT: f64 = 0.20;
const SWITCH_THRESHOLD: f64 = 6.0;
const SWITCH_CONFIRM_FRAMES: usize = 1;
const SWITCH_HOLDOFF_FRAMES: usize = 12;

/// Paging Channel processor that decodes paging messages from the decoded
/// bit stream produced by the Viterbi decoder.
///
/// Accumulates decoded bits into 96-bit (9600 bps) or 48-bit (4800 bps)
/// half-frames, feeds them through `PagingFrameReader`, and emits parsed
/// paging message events as tagged `SampleBlock`s.
///
/// Handles the convolutional code 180° phase ambiguity by running two
/// `PagingFrameReader` instances in parallel — one with normal polarity
/// and one with inverted bits.  Whichever produces a CRC-valid message
/// wins.  This is simple and handles mid-stream polarity flips without
/// detection heuristics.
pub struct PagingChannelProcessor {
    reader_normal: PagingFrameReader,
    reader_inverted: PagingFrameReader,
    /// Bits from the normal-polarity Viterbi decoder.
    bits_normal: Vec<u8>,
    /// Bits from the inverted-polarity Viterbi decoder.
    bits_inverted: Vec<u8>,
    half_frame_bits: usize,
    next_chip: usize,
    chips_per_bit: usize,
    input_sample_rate_hz: f64,
    lc_anchor_state: Option<u64>,
    lc_anchor_chip: Option<usize>,
    paging_message_count: usize,
    /// Exponential moving average of pilot energy (smoothed over blocks).
    pilot_energy_ema: f64,
    pilot_energy_samples: usize,
    /// Whether upstream provides dual-Viterbi interleaved output.
    dual_viterbi: bool,
    // --- Stats ---
    total_half_frames: usize,
    good_half_frames: usize,
    bad_half_frames: usize,
    completed_messages: usize,
    crc_valid_messages: usize,
    total_bit_errors: usize,
    // --- Polarity tracking ---
    active_polarity: Option<Polarity>,
    score_normal: f64,
    score_inverted: f64,
    switch_pending: Option<Polarity>,
    switch_pending_count: usize,
    switch_holdoff: usize,
    polarity_switches: usize,
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
            reader_normal: PagingFrameReader::new_with_rate(rate),
            reader_inverted: PagingFrameReader::new_with_rate(rate),
            bits_normal: Vec::new(),
            bits_inverted: Vec::new(),
            half_frame_bits,
            next_chip: 0,
            chips_per_bit: 1,
            input_sample_rate_hz: 0.0,
            lc_anchor_state: None,
            lc_anchor_chip: None,
            paging_message_count: 0,
            pilot_energy_ema: 0.0,
            pilot_energy_samples: 0,
            dual_viterbi: false,
            total_half_frames: 0,
            good_half_frames: 0,
            bad_half_frames: 0,
            completed_messages: 0,
            crc_valid_messages: 0,
            total_bit_errors: 0,
            active_polarity: None,
            score_normal: 0.0,
            score_inverted: 0.0,
            switch_pending: None,
            switch_pending_count: 0,
            switch_holdoff: 0,
            polarity_switches: 0,
        }
    }

    /// Evaluate a candidate frame alignment for paging channel.
    ///
    /// Feeds decoded bits through a fresh PagingFrameReader and counts
    /// CRC-valid messages. Returns `(messages_parsed, crc_valid_frames)`.
    pub fn evaluate_alignment(
        bits: &[u8],
        shift: usize,
        invert: bool,
        rate: PagingChannelRate,
    ) -> (usize, usize) {
        let half_frame_bits = match rate {
            PagingChannelRate::Rate4800 => 48,
            PagingChannelRate::Rate9600 => 96,
        };
        let mut reader = PagingFrameReader::new_with_rate(rate);
        let mut crc_valid = 0usize;
        let mut messages = 0usize;

        let data = if shift < bits.len() {
            &bits[shift..]
        } else {
            return (0, 0);
        };

        for chunk in data.chunks_exact(half_frame_bits) {
            let frame_bits: Vec<u8> = if invert {
                chunk.iter().map(|b| b ^ 1).collect()
            } else {
                chunk.to_vec()
            };
            let mut half_frame = Bitstream::new_init(&frame_bits);
            let result = reader.process(&mut half_frame);
            for paging_frame in Self::collect_reader_frames(&mut reader, result) {
                messages += 1;
                if paging_frame.crc_valid {
                    crc_valid += 1;
                }
            }
        }
        (messages, crc_valid)
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

    fn polarity_label(pol: Polarity) -> &'static str {
        match pol {
            Polarity::Normal => "normal",
            Polarity::Inverted => "inverted",
        }
    }

    fn score_for_polarity(&self, pol: Polarity) -> f64 {
        match pol {
            Polarity::Normal => self.score_normal,
            Polarity::Inverted => self.score_inverted,
        }
    }

    fn best_scored_polarity(&self) -> Polarity {
        if self.score_inverted > self.score_normal {
            Polarity::Inverted
        } else {
            Polarity::Normal
        }
    }

    fn set_active_polarity(&mut self, pol: Polarity, reason: &str) {
        if self.active_polarity != Some(pol) {
            println!(
                "paging polarity: {} -> {} ({}) scores normal={:.2} inverted={:.2}",
                self.active_polarity
                    .map(Self::polarity_label)
                    .unwrap_or("unknown"),
                Self::polarity_label(pol),
                reason,
                self.score_normal,
                self.score_inverted
            );
        }
        self.active_polarity = Some(pol);
    }

    fn update_polarity_scores(
        &mut self,
        errs_normal: usize,
        errs_inverted: usize,
        normal_frame: Option<&PagingFrame>,
        inverted_frame: Option<&PagingFrame>,
    ) {
        let mut frame_score_normal = -(errs_normal as f64) * SCORE_IDLE_ERR_WEIGHT;
        let mut frame_score_inverted = -(errs_inverted as f64) * SCORE_IDLE_ERR_WEIGHT;

        if errs_normal < errs_inverted {
            frame_score_normal += SCORE_IDLE_BONUS;
        } else if errs_inverted < errs_normal {
            frame_score_inverted += SCORE_IDLE_BONUS;
        }

        if let Some(frame) = normal_frame {
            frame_score_normal += if frame.crc_valid {
                SCORE_CRC_VALID
            } else {
                SCORE_CRC_INVALID
            };
        }
        if let Some(frame) = inverted_frame {
            frame_score_inverted += if frame.crc_valid {
                SCORE_CRC_VALID
            } else {
                SCORE_CRC_INVALID
            };
        }

        self.score_normal = self.score_normal * SCORE_DECAY + frame_score_normal;
        self.score_inverted = self.score_inverted * SCORE_DECAY + frame_score_inverted;
    }

    fn maybe_switch_active_polarity(
        &mut self,
        safe_switch_boundary: bool,
        normal_crc_valid: bool,
        inverted_crc_valid: bool,
    ) {
        if self.switch_holdoff > 0 {
            self.switch_holdoff -= 1;
            return;
        }

        let Some(active) = self.active_polarity else {
            return;
        };
        let target = match active {
            Polarity::Normal => Polarity::Inverted,
            Polarity::Inverted => Polarity::Normal,
        };
        let target_crc_valid = match target {
            Polarity::Normal => normal_crc_valid,
            Polarity::Inverted => inverted_crc_valid,
        };
        if !target_crc_valid {
            self.switch_pending = None;
            self.switch_pending_count = 0;
            return;
        }

        let active_score = self.score_for_polarity(active);
        let target_score = self.score_for_polarity(target);
        if target_score - active_score <= SWITCH_THRESHOLD {
            self.switch_pending = None;
            self.switch_pending_count = 0;
            return;
        }
        if !safe_switch_boundary {
            return;
        }

        if self.switch_pending == Some(target) {
            self.switch_pending_count += 1;
        } else {
            self.switch_pending = Some(target);
            self.switch_pending_count = 1;
        }

        if self.switch_pending_count >= SWITCH_CONFIRM_FRAMES {
            self.set_active_polarity(target, "hysteresis switch");
            self.switch_pending = None;
            self.switch_pending_count = 0;
            self.switch_holdoff = SWITCH_HOLDOFF_FRAMES;
            self.polarity_switches += 1;
        }
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

    /// Validate a half-frame and log if it's bad.
    /// Returns the number of bit errors detected in this half-frame.
    fn validate_half_frame(
        frame_num: usize,
        bits: &[u8],
        reader_in_message: bool,
        label: &str,
    ) -> usize {
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
            println!(
                "half_frame #{} [{}]: BAD IDLE  {} bit errors  hex=[{}]",
                frame_num, label, bit_errors, hex
            );
        }
        bit_errors
    }

    fn print_paging_message(count: usize, frame: &PagingFrame, inverted: bool, pilot_energy: f64) {
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
            // Brief summary for corrupted frames
            println!(
                "paging #{}: BAD CRC  PD={} MSG_TYPE={} ({}) len={} pilot={:.1} hex={}{}",
                count,
                pd,
                msg_type,
                layer3::msg_type_name(msg_type),
                bits.len(),
                pilot_energy,
                hex,
                if inverted { " [inv]" } else { "" },
            );
            return;
        }

        let inv_label = if inverted { " [inverted polarity]" } else { "" };
        println!("========================================");
        println!(
            "PAGING MESSAGE #{}{} (pilot={:.1})",
            count, inv_label, pilot_energy
        );
        println!("  CRC valid: true");
        println!(
            "  PDU length: {} bits ({} bytes)",
            bits.len(),
            bits.len() / 8
        );
        println!(
            "  PD: {}  MSG_TYPE: {} ({})",
            pd,
            msg_type,
            layer3::msg_type_name(msg_type)
        );
        println!("  PDU hex: {}", hex);

        // Decode and print structured fields
        match PagingMessage::decode(&frame.data) {
            Ok(msg) => msg.print(),
            Err(e) => println!("  [decode error: {}]", e),
        }
        println!("========================================");
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
        println!("========== PagingChannelProcessor STATS ==========");
        println!("  total half-frames:      {}", self.total_half_frames);
        println!("  good half-frames:       {}", self.good_half_frames);
        println!("  bad half-frames:        {}", self.bad_half_frames);
        println!("  total bit errors:       {}", self.total_bit_errors);
        println!(
            "  avg errors/bad frame:   {:.1}",
            if self.bad_half_frames > 0 {
                self.total_bit_errors as f64 / self.bad_half_frames as f64
            } else {
                0.0
            }
        );
        println!("  completed messages:     {}", self.completed_messages);
        println!("  CRC-valid messages:     {}", self.crc_valid_messages);
        println!("  emitted paging events:  {}", self.paging_message_count);
        println!(
            "  polarity state:         {}",
            self.active_polarity
                .map(Self::polarity_label)
                .unwrap_or("unknown")
        );
        println!(
            "  polarity scores:        normal={:.2} inverted={:.2}",
            self.score_normal, self.score_inverted
        );
        println!("  polarity switches:      {}", self.polarity_switches);
        println!("====================================================");
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

        // Detect dual-Viterbi mode from upstream tag.
        if block.tags.get("viterbi_dual") == Some(&1) {
            self.dual_viterbi = true;
        }

        // Track upstream pilot energy with exponential moving average.
        if let Some(&pe) = block.tags.get("pilot_energy_x1000") {
            let energy = pe as f64 / 1000.0;
            if self.pilot_energy_samples == 0 {
                self.pilot_energy_ema = energy;
            } else {
                self.pilot_energy_ema = 0.1 * energy + 0.9 * self.pilot_energy_ema;
            }
            self.pilot_energy_samples += 1;
        }

        // Convert samples to hard bits (Viterbi output is 0.0 / 1.0).
        if self.dual_viterbi {
            // Dual-Viterbi: interleaved [normal, inverted, normal, inverted, ...]
            for chunk in block.samples.chunks_exact(2) {
                self.bits_normal
                    .push(if chunk[0].re >= 0.5 { 1u8 } else { 0u8 });
                self.bits_inverted
                    .push(if chunk[1].re >= 0.5 { 1u8 } else { 0u8 });
            }
        } else {
            // Single Viterbi: invert bits manually for the second reader.
            let block_bits: Vec<u8> = block
                .samples
                .iter()
                .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
                .collect();
            self.bits_inverted.extend(block_bits.iter().map(|b| b ^ 1));
            self.bits_normal.extend(block_bits);
        }

        let mut out = Vec::new();
        while self.bits_normal.len() >= self.half_frame_bits
            && self.bits_inverted.len() >= self.half_frame_bits
        {
            let normal_bits: Vec<u8> = self.bits_normal.drain(..self.half_frame_bits).collect();
            let inverted_bits: Vec<u8> = self.bits_inverted.drain(..self.half_frame_bits).collect();
            let frame_chip = self.next_chip;
            self.next_chip += self.half_frame_bits * self.chips_per_bit;
            self.total_half_frames += 1;
            let frame_num = self.total_half_frames;

            let preferred_pol = self
                .active_polarity
                .unwrap_or_else(|| self.best_scored_polarity());
            let preferred_sci = match preferred_pol {
                Polarity::Normal => normal_bits[0],
                Polarity::Inverted => inverted_bits[0],
            };
            if preferred_sci == 1 {
                if let Some(lc_state) = self.lc_state_at_chip(frame_chip) {
                    eprintln!(
                        "rx_fpch_boundary chip={} lc_state=0x{:x} preferred={} sci_normal={} sci_inverted={} chips_per_bit={}",
                        frame_chip,
                        lc_state,
                        Self::polarity_label(preferred_pol),
                        normal_bits[0],
                        inverted_bits[0],
                        self.chips_per_bit
                    );
                } else {
                    eprintln!(
                        "rx_fpch_boundary chip={} lc_state=unknown preferred={} sci_normal={} sci_inverted={} chips_per_bit={}",
                        frame_chip,
                        Self::polarity_label(preferred_pol),
                        normal_bits[0],
                        inverted_bits[0],
                        self.chips_per_bit
                    );
                }
            }

            // --- Validate half-frame BEFORE feeding to readers ---
            // Check normal side: is the reader currently accumulating a message?
            let normal_in_msg = self.reader_normal.in_message();
            let inverted_in_msg = self.reader_inverted.in_message();

            let errs_normal =
                Self::validate_half_frame(frame_num, &normal_bits, normal_in_msg, "normal");
            let errs_inverted =
                Self::validate_half_frame(frame_num, &inverted_bits, inverted_in_msg, "inverted");

            // Use the side with fewer errors for tracking (they're complementary,
            // so the correct polarity should have ~0 errors in idle).
            let frame_errors = errs_normal.min(errs_inverted);
            if frame_errors > 0 {
                self.bad_half_frames += 1;
                self.total_bit_errors += frame_errors;
            } else {
                self.good_half_frames += 1;
            }

            // --- Feed to PagingFrameReaders ---
            let mut hf_normal = Bitstream::new_init(&normal_bits);
            let mut hf_inverted = Bitstream::new_init(&inverted_bits);
            println!("paging half-frame: {}", hf_normal);

            let result_normal = self.reader_normal.process(&mut hf_normal);
            let result_inverted = self.reader_inverted.process(&mut hf_inverted);

            let mut normal_frames =
                Self::collect_reader_frames(&mut self.reader_normal, result_normal);
            let mut inverted_frames =
                Self::collect_reader_frames(&mut self.reader_inverted, result_inverted);

            self.completed_messages += normal_frames.len() + inverted_frames.len();
            self.crc_valid_messages += normal_frames.iter().filter(|f| f.crc_valid).count()
                + inverted_frames.iter().filter(|f| f.crc_valid).count();

            let normal_frame = normal_frames.first();
            let inverted_frame = inverted_frames.first();

            let normal_crc_valid = normal_frames.iter().any(|f| f.crc_valid);
            let inverted_crc_valid = inverted_frames.iter().any(|f| f.crc_valid);

            self.update_polarity_scores(errs_normal, errs_inverted, normal_frame, inverted_frame);

            if self.active_polarity.is_none() {
                if normal_crc_valid && !inverted_crc_valid {
                    self.set_active_polarity(Polarity::Normal, "initial CRC lock");
                } else if inverted_crc_valid && !normal_crc_valid {
                    self.set_active_polarity(Polarity::Inverted, "initial CRC lock");
                }
            }

            let safe_switch_boundary = match self.active_polarity {
                Some(Polarity::Normal) => {
                    normal_bits[0] == 1 || (!normal_in_msg && !inverted_in_msg)
                }
                Some(Polarity::Inverted) => {
                    inverted_bits[0] == 1 || (!normal_in_msg && !inverted_in_msg)
                }
                None => true,
            };
            self.maybe_switch_active_polarity(
                safe_switch_boundary,
                normal_crc_valid,
                inverted_crc_valid,
            );

            let both_bad_crc = !normal_frames.is_empty()
                && !inverted_frames.is_empty()
                && !normal_crc_valid
                && !inverted_crc_valid;
            if both_bad_crc {
                for nf in &normal_frames {
                    Self::print_paging_message(
                        self.paging_message_count + 1,
                        nf,
                        false,
                        self.pilot_energy_ema,
                    );
                }
                for inv_f in &inverted_frames {
                    Self::print_paging_message(
                        self.paging_message_count + 1,
                        inv_f,
                        true,
                        self.pilot_energy_ema,
                    );
                }
                continue;
            }

            let preferred = self
                .active_polarity
                .unwrap_or_else(|| self.best_scored_polarity());
            let other = match preferred {
                Polarity::Normal => Polarity::Inverted,
                Polarity::Inverted => Polarity::Normal,
            };

            let preferred_crc_valid = match preferred {
                Polarity::Normal => normal_crc_valid,
                Polarity::Inverted => inverted_crc_valid,
            };
            let other_crc_valid = match other {
                Polarity::Normal => normal_crc_valid,
                Polarity::Inverted => inverted_crc_valid,
            };

            let chosen = if preferred_crc_valid {
                Some(match preferred {
                    Polarity::Normal => (std::mem::take(&mut normal_frames), false),
                    Polarity::Inverted => (std::mem::take(&mut inverted_frames), true),
                })
            } else if other_crc_valid {
                Some(match other {
                    Polarity::Normal => (std::mem::take(&mut normal_frames), false),
                    Polarity::Inverted => (std::mem::take(&mut inverted_frames), true),
                })
            } else {
                match preferred {
                    Polarity::Normal if !normal_frames.is_empty() => {
                        Some((std::mem::take(&mut normal_frames), false))
                    }
                    Polarity::Inverted if !inverted_frames.is_empty() => {
                        Some((std::mem::take(&mut inverted_frames), true))
                    }
                    _ => match other {
                        Polarity::Normal if !normal_frames.is_empty() => {
                            Some((std::mem::take(&mut normal_frames), false))
                        }
                        Polarity::Inverted if !inverted_frames.is_empty() => {
                            Some((std::mem::take(&mut inverted_frames), true))
                        }
                        _ => None,
                    },
                }
            };

            if let Some((paging_frames, is_inverted)) = chosen {
                for paging_frame in paging_frames {
                    Self::print_paging_message(
                        self.paging_message_count + 1,
                        &paging_frame,
                        is_inverted,
                        self.pilot_energy_ema,
                    );
                    if paging_frame.crc_valid {
                        out.push(self.emit_paging_event(frame_chip, paging_frame));
                    }
                }
            }
        }

        out
    }

    fn name(&self) -> &'static str {
        "PagingChannelProcessor"
    }
}
