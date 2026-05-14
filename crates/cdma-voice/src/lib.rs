pub mod evrc;
pub mod evrc_b_wb;
pub mod tone_player;
pub mod wav_player;

use crate::evrc::EvrcEncoder;
use crate::evrc_b_wb::{EvrcBEncoder, EvrcWbEncoder};

/// Number of PCM samples per 20ms frame at 8 kHz.
pub(crate) const SAMPLES_PER_FRAME: usize = 160;

/// SO3: EVRC-A / IS-127 narrowband.
pub const SERVICE_OPTION_EVRC_A: u16 = 3;

/// SO68: EVRC-B narrowband.
pub const SERVICE_OPTION_EVRC_B: u16 = 68;

/// SO70: EVRC-WB.
pub const SERVICE_OPTION_EVRC_WB: u16 = 70;

/// Voice frame rate indicator, matching MuxPDU Rate Set 1 rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceRate {
    /// Full rate: 9600 bps, 171 primary traffic bits per 20ms frame.
    Full,
    /// Half rate: 4800 bps, 80 primary traffic bits per 20ms frame.
    Half,
    /// Quarter rate: 2400 bps, 40 primary traffic bits per 20ms frame.
    Quarter,
    /// Eighth rate: 1200 bps, 16 primary traffic bits per 20ms frame.
    Eighth,
}

/// Implemented CDMA voice codecs keyed by service option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCodec {
    /// SO3: EVRC-A / IS-127 narrowband.
    EvrcA,
    /// SO68: EVRC-B narrowband.
    EvrcB,
    /// SO70: EVRC-WB.
    EvrcWb,
}

impl VoiceCodec {
    pub fn from_service_option(service_option: u16) -> Option<Self> {
        match service_option {
            SERVICE_OPTION_EVRC_A => Some(Self::EvrcA),
            SERVICE_OPTION_EVRC_B => Some(Self::EvrcB),
            SERVICE_OPTION_EVRC_WB => Some(Self::EvrcWb),
            _ => None,
        }
    }

    pub fn service_option(self) -> u16 {
        match self {
            Self::EvrcA => SERVICE_OPTION_EVRC_A,
            Self::EvrcB => SERVICE_OPTION_EVRC_B,
            Self::EvrcWb => SERVICE_OPTION_EVRC_WB,
        }
    }
}

impl VoiceRate {
    /// Number of primary traffic bits in one 20ms MuxPDU frame at this rate.
    pub fn primary_traffic_bits(&self) -> usize {
        match self {
            VoiceRate::Full => 171,
            VoiceRate::Half => 80,
            VoiceRate::Quarter => 40,
            VoiceRate::Eighth => 16,
        }
    }
}

pub(crate) enum VoiceEncoder {
    EvrcA(EvrcEncoder),
    EvrcB(EvrcBEncoder),
    EvrcWb(EvrcWbEncoder),
}

impl VoiceEncoder {
    pub(crate) fn new(codec: VoiceCodec) -> Result<Self, String> {
        match codec {
            VoiceCodec::EvrcA => Ok(Self::EvrcA(EvrcEncoder::new()?)),
            VoiceCodec::EvrcB => Ok(Self::EvrcB(EvrcBEncoder::new()?)),
            VoiceCodec::EvrcWb => Ok(Self::EvrcWb(EvrcWbEncoder::new()?)),
        }
    }

    fn encode(&mut self, pcm: &[i16; SAMPLES_PER_FRAME]) -> Result<(VoiceRate, Vec<u8>), String> {
        match self {
            VoiceEncoder::EvrcA(encoder) => encoder.encode(pcm),
            VoiceEncoder::EvrcB(encoder) => encoder.encode(pcm),
            VoiceEncoder::EvrcWb(encoder) => encoder.encode_8k_input(pcm),
        }
    }
}

/// A single encoded voice frame ready for MuxPDU framing on the forward
/// traffic channel.
#[derive(Debug, Clone)]
pub struct VoiceFrame {
    /// The voice codec payload bits (length = rate.primary_traffic_bits()).
    /// Each element is 0 or 1.
    pub bits: Vec<u8>,
    /// The rate of this frame.
    pub rate: VoiceRate,
}

pub(crate) fn encode_pcm_frame(
    encoder: &mut VoiceEncoder,
    pcm: &[i16; SAMPLES_PER_FRAME],
) -> VoiceFrame {
    let (rate, packet_data) = match encoder.encode(pcm) {
        Ok(result) => result,
        Err(e) => {
            log::warn!("EVRC encode failed: {}, sending silence", e);
            (VoiceRate::Eighth, vec![0u8; 2])
        }
    };

    let traffic_bits = rate.primary_traffic_bits();
    let mut bits = Vec::with_capacity(traffic_bits);

    for &byte in &packet_data {
        for bit_idx in (0..8).rev() {
            bits.push((byte >> bit_idx) & 1);
        }
    }

    bits.truncate(traffic_bits);
    while bits.len() < traffic_bits {
        bits.push(0);
    }

    VoiceFrame { bits, rate }
}
