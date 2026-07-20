//! Per-frame RRI rate detection for the HRPD reverse-traffic sub-chain.
//!
//! Decodes the 3-bit Reverse Rate Indicator codeword per
//! C.S0024-0 v4.0 §9.2.1.3.3.2 and tags the block with the resulting rate
//! (in bps, 0 for "no transmission detected") plus the decode margin.

use num_complex::Complex32;

use crate::receiver::hrpd::data_decoder::ReverseDataRate;
use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

use super::despread::{HRPD_SLOT_CHIPS, HRPD_TRAFFIC_FRAME_CHIPS, HRPD_TRAFFIC_SLOTS_PER_FRAME};

pub const TAG_RRI_RATE_BPS: &str = "hrpd_reverse_rri_rate_bps";
pub const TAG_RRI_MARGIN_DB_TENTHS: &str = "hrpd_reverse_rri_margin_db_tenths";

/// RRI codewords from C.S0024-0 v4.0 Table 9.2.1.3.3.2-1.
const RRI_CODEWORDS: &[(u8, Option<ReverseDataRate>, [u8; 7])] = &[
    (0b000, None, [0, 0, 0, 0, 0, 0, 0]),
    (0b001, Some(ReverseDataRate::Kbps9_6), [1, 0, 1, 0, 1, 0, 1]),
    (
        0b010,
        Some(ReverseDataRate::Kbps19_2),
        [0, 1, 1, 0, 0, 1, 1],
    ),
    (
        0b011,
        Some(ReverseDataRate::Kbps38_4),
        [1, 1, 0, 0, 1, 1, 0],
    ),
    (
        0b100,
        Some(ReverseDataRate::Kbps76_8),
        [0, 0, 0, 1, 1, 1, 1],
    ),
    (
        0b101,
        Some(ReverseDataRate::Kbps153_6),
        [1, 0, 1, 1, 0, 1, 0],
    ),
];

#[derive(Debug, Clone, Copy)]
pub struct HrpdRriDetection {
    pub rate: Option<ReverseDataRate>,
    pub symbol: u8,
    pub margin_db: f32,
    pub best_score: f32,
    pub second_score: f32,
}

/// Decode the RRI codeword from a despread reverse-traffic frame.
///
/// `chips` is the full 16-slot post-despread chip stream (length
/// `HRPD_TRAFFIC_FRAME_CHIPS`). The RRI sits on the first 256 chips of each
/// slot as W0^16-spread copies of a 7-bit simplex codeword repeated and
/// punctured to 256 symbols.
pub fn detect_hrpd_reverse_rri_rate(chips: &[Complex32]) -> HrpdRriDetection {
    if chips.len() < HRPD_TRAFFIC_FRAME_CHIPS {
        return HrpdRriDetection {
            rate: None,
            symbol: 0,
            margin_db: 0.0,
            best_score: 0.0,
            second_score: 0.0,
        };
    }
    // Each slot contributes 16 length-16 W0 symbols across the 256-chip RRI
    // burst; sum the W0 output (which is the chip mean) into soft symbols.
    let mut soft_symbols = Vec::with_capacity(256);
    for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
        let slot_base = slot * HRPD_SLOT_CHIPS;
        for symbol in 0..16 {
            let base = slot_base + symbol * 16;
            let mut acc = 0.0f32;
            for chip in 0..16 {
                acc += chips[base + chip].re;
            }
            soft_symbols.push(acc);
        }
    }

    let mut scored = RRI_CODEWORDS
        .iter()
        .map(|&(symbol, rate, code)| {
            let mut score = 0.0f32;
            for (idx, &soft) in soft_symbols.iter().enumerate() {
                let bit = code[idx % 7];
                let sign = if bit == 0 { 1.0 } else { -1.0 };
                score += soft * sign;
            }
            (symbol, rate, score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.2.total_cmp(&a.2));
    let (symbol, rate, best) = scored[0];
    let second = scored.get(1).map(|(_, _, score)| *score).unwrap_or(0.0);
    let denom = (soft_symbols.iter().map(|v| v.abs()).sum::<f32>() / 8.0).max(1.0e-6);
    let margin_db = 10.0 * ((best - second).abs() / denom).max(1.0e-6).log10();
    HrpdRriDetection {
        rate,
        symbol,
        margin_db,
        best_score: best,
        second_score: second,
    }
}

fn rate_to_bps(rate: ReverseDataRate) -> u32 {
    // Per C.S0024-0 v4.0 §9.2.1.3.1: a reverse Traffic Channel physical-layer
    // packet is 16 slots of 1.6667 ms = 26.6667 ms; the rate label is the
    // enum's data-rate name.
    match rate {
        ReverseDataRate::Kbps9_6 => 9_600,
        ReverseDataRate::Kbps19_2 => 19_200,
        ReverseDataRate::Kbps38_4 => 38_400,
        ReverseDataRate::Kbps76_8 => 76_800,
        ReverseDataRate::Kbps153_6 => 153_600,
    }
}

/// `PipelineProcessor` that decodes the per-frame RRI and tags the block with
/// the detected bps + margin. Passes the despread chips through unmodified.
#[derive(Debug, Default)]
pub struct HrpdReverseTrafficRriProcessor;

impl HrpdReverseTrafficRriProcessor {
    pub fn new() -> Self {
        Self
    }
}

impl PipelineProcessor for HrpdReverseTrafficRriProcessor {
    fn process_block(&mut self, mut block: SampleBlock) -> Vec<SampleBlock> {
        let detection = detect_hrpd_reverse_rri_rate(&block.samples);
        let rate_bps = match detection.rate {
            Some(rate) => rate_to_bps(rate) as i64,
            None => 0,
        };
        block.tags.insert(TAG_RRI_RATE_BPS, rate_bps);
        block.tags.insert(
            TAG_RRI_MARGIN_DB_TENTHS,
            (detection.margin_db * 10.0).round() as i64,
        );
        vec![block]
    }

    fn name(&self) -> &'static str {
        "HrpdReverseTrafficRriProcessor"
    }
}
