use std::collections::HashMap;

use cdma_common::crc::{crc8, crc12};
use log::{debug, info};
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};
use crate::phy::coding::block_interleaver::{
    Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
};
use crate::phy::coding::convolutional::get_1_3_k9_viterbi_decoder;
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::pipelined::traffic_channel_processor::{
    ReverseMux1SignalingLayout, extract_reverse_mux1_full_rate_signaling_block,
    parse_reverse_mux1_full_rate_format,
};

use cdma_common::consts::{
    RC1_SOFT_BITS_PER_SYMBOL, RC1_SYMBOLS_PER_FRAME, RC1_SYMBOLS_PER_PCG, SR1_PCGS_PER_FRAME,
};

const SOFT_BITS_PER_FRAME: usize = RC1_SYMBOLS_PER_FRAME * RC1_SOFT_BITS_PER_SYMBOL;
const SOFT_BITS_PER_PCG: usize = RC1_SYMBOLS_PER_PCG * RC1_SOFT_BITS_PER_SYMBOL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rc1TrafficRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl Rc1TrafficRate {
    const fn to_interleaver_rate(self) -> Rc12ReverseTrafficRate {
        match self {
            Self::Full => Rc12ReverseTrafficRate::Full,
            Self::Half => Rc12ReverseTrafficRate::Half,
            Self::Quarter => Rc12ReverseTrafficRate::Quarter,
            Self::Eighth => Rc12ReverseTrafficRate::Eighth,
        }
    }

    const fn repetition_factor(self) -> usize {
        self.to_interleaver_rate().repetition_factor()
    }

    const fn frame_bits(self) -> usize {
        match self {
            Self::Full => 192,
            Self::Half => 96,
            Self::Quarter => 48,
            Self::Eighth => 24,
        }
    }

    const fn info_bits(self) -> usize {
        match self {
            Self::Full => 172,
            Self::Half => 80,
            Self::Quarter => 40,
            Self::Eighth => 16,
        }
    }

    const fn fqi_bits(self) -> usize {
        match self {
            Self::Full => 12,
            Self::Half => 8,
            Self::Quarter | Self::Eighth => 0,
        }
    }

    const fn tail_bits(self) -> usize {
        8
    }

    const fn rate_bps(self) -> usize {
        match self {
            Self::Full => 9600,
            Self::Half => 4800,
            Self::Quarter => 2400,
            Self::Eighth => 1200,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameValidation {
    fqi_valid: bool,
    tail_valid: bool,
    phy_valid: bool,
}

#[derive(Clone, Debug)]
struct DecodedTrafficFrame {
    rate: Rc1TrafficRate,
    bits: Vec<u8>,
    validation: FrameValidation,
    metric: f32,
}

pub struct Rc1TrafficMultiRateDecoder {
    esn: u32,
}

impl Rc1TrafficMultiRateDecoder {
    pub fn new(esn: u32) -> Self {
        Self { esn }
    }

    /// Extract 14 long code bits for the data burst randomizer.
    ///
    /// Per C.S0002-E 2.1.3.1.14.2: "These 14 bits shall be the last 14 bits
    /// of the long code used for spreading in the next to last power control
    /// group of the previous frame ... the 14 bits that occur exactly one
    /// power control group (1.25 ms) before each Reverse Fundamental Channel
    /// frame boundary."
    ///
    /// 1 PCG = 1536 chips, so the 14 bits end at frame_chip_start - 1536
    /// and start at frame_chip_start - 1536 - 14 + 1 = frame_chip_start - 1549.
    fn lc_randomizer_bits(&self, frame_chip_start: usize) -> [u8; 14] {
        let mut generator = LongCodeGenerator::new_traffic_channel(self.esn);
        // Advance to the start of the 14-bit window: 1536 + 14 - 1 = 1549 chips before frame
        let offset = frame_chip_start.saturating_sub(1536 + 13);
        generator.advance_chips(offset);
        let mut bits = [0u8; 14];
        for bit in &mut bits {
            *bit = generator.next_chip();
        }
        bits
    }

    fn active_pcgs_for_rate(
        &self,
        rate: Rc1TrafficRate,
        frame_chip_start: usize,
    ) -> [bool; SR1_PCGS_PER_FRAME] {
        let mut active = [false; SR1_PCGS_PER_FRAME];
        if rate == Rc1TrafficRate::Full {
            active.fill(true);
            return active;
        }

        let b = self.lc_randomizer_bits(frame_chip_start);
        match rate {
            Rc1TrafficRate::Half => {
                for i in 0..8usize {
                    active[2 * i + b[i] as usize] = true;
                }
            }
            Rc1TrafficRate::Quarter => {
                active[if b[8] == 0 { b[0] } else { 2 + b[1] } as usize] = true;
                active[(if b[9] == 0 { 4 + b[2] } else { 6 + b[3] }) as usize] = true;
                active[(if b[10] == 0 { 8 + b[4] } else { 10 + b[5] }) as usize] = true;
                active[(if b[11] == 0 { 12 + b[6] } else { 14 + b[7] }) as usize] = true;
            }
            Rc1TrafficRate::Eighth => {
                // C.S0002-E 2.1.3.1.14.2: lower half uses (b8,b12) and (b9,b12)
                let lower = if b[12] == 0 {
                    if b[8] == 0 {
                        b[0] as usize
                    } else {
                        2 + b[1] as usize
                    }
                } else {
                    if b[9] == 0 {
                        4 + b[2] as usize
                    } else {
                        6 + b[3] as usize
                    }
                };
                // Upper half uses (b10,b13) and (b11,b13)
                let upper = if b[13] == 0 {
                    if b[10] == 0 {
                        8 + b[4] as usize
                    } else {
                        10 + b[5] as usize
                    }
                } else {
                    if b[11] == 0 {
                        12 + b[6] as usize
                    } else {
                        14 + b[7] as usize
                    }
                };
                active[lower] = true;
                active[upper] = true;
            }
            Rc1TrafficRate::Full => {}
        }
        active
    }

    fn apply_pcg_mask(
        &self,
        frame_soft: &[f32],
        rate: Rc1TrafficRate,
        frame_chip_start: usize,
    ) -> Vec<f32> {
        if rate == Rc1TrafficRate::Full {
            return frame_soft.to_vec();
        }

        let mut masked = frame_soft.to_vec();
        let active = self.active_pcgs_for_rate(rate, frame_chip_start);
        for (pcg_idx, enabled) in active.iter().copied().enumerate() {
            if enabled {
                continue;
            }
            let start = pcg_idx * SOFT_BITS_PER_PCG;
            let end = start + SOFT_BITS_PER_PCG;
            masked[start..end].fill(0.0);
        }
        masked
    }

    fn collapse_repetition(deinterleaved: &[f32], repetition_factor: usize) -> Vec<f32> {
        deinterleaved
            .chunks_exact(repetition_factor)
            .map(|chunk| chunk.iter().sum::<f32>())
            .collect()
    }

    fn decode_bits(collapsed: &[f32]) -> Vec<u8> {
        // Hard decision: positive soft value → code symbol 0, negative → 1
        // (Walsh demod: soft_bits[i] = max_zero_energy - max_one_energy)
        let hard: Vec<[u8; 3]> = collapsed
            .chunks_exact(3)
            .map(|chunk| {
                [
                    if chunk[0] >= 0.0 { 0u8 } else { 1u8 },
                    if chunk[1] >= 0.0 { 0u8 } else { 1u8 },
                    if chunk[2] >= 0.0 { 0u8 } else { 1u8 },
                ]
            })
            .collect();
        let mut decoder = get_1_3_k9_viterbi_decoder();
        decoder.decode_block_from_state(&hard, 0)
    }

    fn validate(rate: Rc1TrafficRate, bits: &[u8]) -> FrameValidation {
        if bits.len() < rate.frame_bits() {
            return FrameValidation {
                fqi_valid: false,
                tail_valid: false,
                phy_valid: false,
            };
        }

        let tail_start = rate.frame_bits() - rate.tail_bits();
        let tail_valid = bits[tail_start..rate.frame_bits()]
            .iter()
            .all(|bit| *bit == 0);
        if !tail_valid {
            return FrameValidation {
                fqi_valid: false,
                tail_valid: false,
                phy_valid: false,
            };
        }

        let fqi_valid = match rate {
            Rc1TrafficRate::Full => {
                let computed = crc12(&bits[..rate.info_bits()]);
                let mut received: u16 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit as u16 & 1);
                }
                computed == received
            }
            Rc1TrafficRate::Half => {
                let computed = crc8(&bits[..rate.info_bits()]);
                let mut received: u8 = 0;
                for &bit in &bits[rate.info_bits()..rate.info_bits() + rate.fqi_bits()] {
                    received = (received << 1) | (bit & 1);
                }
                computed == received
            }
            Rc1TrafficRate::Quarter | Rc1TrafficRate::Eighth => true,
        };

        FrameValidation {
            fqi_valid,
            tail_valid,
            phy_valid: tail_valid && fqi_valid,
        }
    }

    fn decode_frame(
        &self,
        frame_soft: &[f32],
        rate: Rc1TrafficRate,
        frame_chip_start: usize,
    ) -> DecodedTrafficFrame {
        let masked = self.apply_pcg_mask(frame_soft, rate, frame_chip_start);
        let interleaver = Rc12ReverseTrafficInterleaver::new(rate.to_interleaver_rate());
        let deinterleaved = interleaver.decode_soft(&masked);
        let collapsed = Self::collapse_repetition(&deinterleaved, rate.repetition_factor());
        let metric = collapsed.iter().map(|v| v.abs()).sum::<f32>() / collapsed.len().max(1) as f32;
        let bits = Self::decode_bits(&collapsed);
        let validation = Self::validate(rate, &bits);

        DecodedTrafficFrame {
            rate,
            bits,
            validation,
            metric,
        }
    }

    fn choose_best_frame(
        &self,
        frame_soft: &[f32],
        frame_chip_start: usize,
    ) -> Option<DecodedTrafficFrame> {
        let full = self.decode_frame(frame_soft, Rc1TrafficRate::Full, frame_chip_start);
        if full.validation.phy_valid {
            return Some(full);
        }

        // Log full-rate diagnostics when it fails
        if full.bits.len() >= 192 {
            let tail = &full.bits[184..192];
            let computed_crc = crc12(&full.bits[..172]);
            let mut received_crc: u16 = 0;
            for &bit in &full.bits[172..184] {
                received_crc = (received_crc << 1) | (bit as u16 & 1);
            }
            debug!(
                "rc1_full_rate_fail: chip={} tail={:?} computed_crc=0x{:03X} received_crc=0x{:03X} first8={:?}",
                frame_chip_start,
                tail,
                computed_crc,
                received_crc,
                &full.bits[..8],
            );
        }

        let half = self.decode_frame(frame_soft, Rc1TrafficRate::Half, frame_chip_start);
        if half.validation.phy_valid {
            return Some(half);
        }

        let quarter = self.decode_frame(frame_soft, Rc1TrafficRate::Quarter, frame_chip_start);
        let eighth = self.decode_frame(frame_soft, Rc1TrafficRate::Eighth, frame_chip_start);

        // Log tail bits for full rate to diagnose decode quality
        if !full.validation.tail_valid && full.bits.len() >= 192 {
            let tail = &full.bits[184..192];
            debug!(
                "rc1_multi_rate: chip={} full_tail_bits={:?} full(tail={} fqi={}) half(tail={} fqi={}) qtr(tail={}) 8th(tail={})",
                frame_chip_start,
                tail,
                full.validation.tail_valid,
                full.validation.fqi_valid,
                half.validation.tail_valid,
                half.validation.fqi_valid,
                quarter.validation.tail_valid,
                eighth.validation.tail_valid,
            );
        } else {
            debug!(
                "rc1_multi_rate: chip={} full(tail={} fqi={}) half(tail={} fqi={}) qtr(tail={}) 8th(tail={})",
                frame_chip_start,
                full.validation.tail_valid,
                full.validation.fqi_valid,
                half.validation.tail_valid,
                half.validation.fqi_valid,
                quarter.validation.tail_valid,
                eighth.validation.tail_valid,
            );
        }

        match (quarter.validation.tail_valid, eighth.validation.tail_valid) {
            (true, true) => {
                if quarter.metric >= eighth.metric * 0.9 {
                    Some(quarter)
                } else {
                    Some(eighth)
                }
            }
            (true, false) => Some(quarter),
            (false, true) => Some(eighth),
            (false, false) => None,
        }
    }

    fn emit_decoded_frame(
        &self,
        decoded: DecodedTrafficFrame,
        frame_chip_start: usize,
        sample_rate_hz: f64,
        upstream_tags: &HashMap<&'static str, i64>,
    ) -> SampleBlock {
        let mut tags = upstream_tags.clone();
        tags.insert("traffic_decoded_frame", 1);
        tags.insert("traffic_rate_bps", decoded.rate.rate_bps() as i64);
        tags.insert("traffic_info_bits", decoded.rate.info_bits() as i64);
        tags.insert("traffic_fqi_bits", decoded.rate.fqi_bits() as i64);
        tags.insert("traffic_tail_bits", decoded.rate.tail_bits() as i64);
        tags.insert("traffic_fqi_valid", decoded.validation.fqi_valid as i64);
        tags.insert("traffic_tail_valid", decoded.validation.tail_valid as i64);
        tags.insert("traffic_phy_valid", decoded.validation.phy_valid as i64);
        tags.insert("traffic_is_preamble", 0);
        tags.insert("traffic_walsh_locked", 1);
        tags.insert("traffic_frame_aligned", 1);

        if decoded.rate == Rc1TrafficRate::Full
            && decoded.bits.len() >= decoded.rate.info_bits()
            && let Some(format) =
                parse_reverse_mux1_full_rate_format(&decoded.bits[..decoded.rate.info_bits()])
        {
            tags.insert("traffic_mux_header", format.mux_header as i64);
            tags.insert("traffic_mux_header_bits", format.header_bits as i64);
            tags.insert("traffic_mux_primary_bits", format.primary_bits as i64);
            tags.insert("traffic_mux_signaling_bits", format.signaling_bits as i64);

            for layout in ReverseMux1SignalingLayout::SEARCH_ORDER {
                if extract_reverse_mux1_full_rate_signaling_block(
                    &decoded.bits[..decoded.rate.info_bits()],
                    layout,
                )
                .is_some()
                {
                    tags.insert("traffic_mux_signaling_layout", layout.tag_value());
                    break;
                }
            }
        }

        let samples = decoded
            .bits
            .iter()
            .take(decoded.rate.frame_bits())
            .map(|&bit| Complex32::new(bit as f32, 0.0))
            .collect::<Vec<_>>();
        let mut out =
            SampleBlock::new(samples, frame_chip_start).with_sample_rate_hz(sample_rate_hz);
        out.tags = tags;
        out
    }
}

impl PipelineProcessor for Rc1TrafficMultiRateDecoder {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if block.tags.get("traffic_symbol_frame").copied().unwrap_or(0) != 1 {
            return vec![block];
        }

        let frame_soft = block
            .samples
            .iter()
            .map(|sample| sample.re)
            .take(SOFT_BITS_PER_FRAME)
            .collect::<Vec<_>>();
        if frame_soft.len() < SOFT_BITS_PER_FRAME {
            return Vec::new();
        }

        let frame_chip_start = block.chip_start;
        let Some(decoded) = self.choose_best_frame(&frame_soft, frame_chip_start) else {
            return Vec::new();
        };

        // Pack info bits into hex for logging
        let info_len = decoded.rate.info_bits().min(decoded.bits.len());
        let hex: String = decoded.bits[..info_len]
            .chunks(8)
            .map(|byte_bits| {
                let mut val = 0u8;
                for (i, &bit) in byte_bits.iter().enumerate() {
                    val |= (bit & 1) << (7 - i);
                }
                format!("{:02x}", val)
            })
            .collect::<Vec<_>>()
            .join("");
        // Log FQI-valid frames (full/half rate with CRC) at info, others at debug
        if decoded.validation.fqi_valid && decoded.rate.fqi_bits() > 0 {
            info!(
                "rc1_traffic_multi_rate_decoder: frame chip={} rate={} fqi_valid=true hex={}",
                frame_chip_start,
                decoded.rate.rate_bps(),
                hex,
            );
        } else {
            debug!(
                "rc1_traffic_multi_rate_decoder: frame chip={} rate={} fqi_valid={} tail_valid={} hex={}",
                frame_chip_start,
                decoded.rate.rate_bps(),
                decoded.validation.fqi_valid,
                decoded.validation.tail_valid,
                hex,
            );
        }

        vec![self.emit_decoded_frame(decoded, frame_chip_start, block.sample_rate_hz, &block.tags)]
    }

    fn name(&self) -> &'static str {
        "Rc1TrafficMultiRateDecoder"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::coding::block_interleaver::{
        Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
    };
    use crate::phy::coding::convolutional::get_1_3_k9_encoder;

    #[test]
    fn test_full_rate_encode_decode_roundtrip() {
        // Create a known 172-bit info payload
        let mut info_bits = vec![0u8; 172];
        // Put some recognizable data in
        for i in 0..172 {
            info_bits[i] = ((i * 7 + 3) % 2) as u8;
        }

        // Step 1: Compute CRC-12
        let crc = crc12(&info_bits);
        let mut crc_bits = [0u8; 12];
        for i in 0..12 {
            crc_bits[i] = ((crc >> (11 - i)) & 1) as u8;
        }

        // Step 2: Append CRC + 8 tail bits = 192 frame bits
        let mut frame_bits = Vec::with_capacity(192);
        frame_bits.extend_from_slice(&info_bits);
        frame_bits.extend_from_slice(&crc_bits);
        frame_bits.extend_from_slice(&[0u8; 8]); // tail bits
        assert_eq!(frame_bits.len(), 192);

        // Step 3: Convolutional encode R=1/3 → 576 code symbols
        let mut encoder = get_1_3_k9_encoder();
        let encoded: Vec<[u8; 3]> = frame_bits.iter().map(|&b| encoder.encode(b)).collect();
        let flat_encoded: Vec<u8> = encoded.iter().flat_map(|s| s.iter().copied()).collect();
        assert_eq!(flat_encoded.len(), 576);

        // Step 4: Interleave
        let interleaver = Rc12ReverseTrafficInterleaver::new(Rc12ReverseTrafficRate::Full);
        let interleaved = interleaver.encode(&flat_encoded);
        assert_eq!(interleaved.len(), 576);

        // Step 5: Convert to soft values (0 → +1.0, 1 → -1.0)
        let soft: Vec<f32> = interleaved
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();

        // Step 6: Decode (deinterleave → collapse → Viterbi)
        let deinterleaved = interleaver.decode_soft(&soft);
        let collapsed = Rc1TrafficMultiRateDecoder::collapse_repetition(&deinterleaved, 1);
        let decoded = Rc1TrafficMultiRateDecoder::decode_bits(&collapsed);

        // Step 7: Verify
        assert_eq!(decoded.len(), 192, "decoded length should be 192");
        assert_eq!(&decoded[..172], &info_bits[..], "info bits should match");
        assert_eq!(&decoded[172..184], &crc_bits[..], "CRC bits should match");
        assert_eq!(&decoded[184..192], &[0u8; 8], "tail bits should be zero");

        // Step 8: Validate CRC
        let validation = Rc1TrafficMultiRateDecoder::validate(Rc1TrafficRate::Full, &decoded);
        assert!(validation.tail_valid, "tail should be valid");
        assert!(validation.fqi_valid, "CRC should pass");
        assert!(validation.phy_valid, "phy should be valid");
        eprintln!("roundtrip test PASSED");
    }
}
