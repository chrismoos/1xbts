pub mod encoded_stream;
pub mod evrc;
pub mod evrc_b_wb;
pub mod qcelp13k;
pub mod resample;
pub mod ringtone_player;
pub mod ringtone_preencode;
pub mod tia96;
pub mod tone_player;
pub mod wav_player;

use crate::evrc::{EvrcDecoder, EvrcEncoder};
use crate::evrc_b_wb::{EvrcBDecoder, EvrcBEncoder, EvrcWbDecoder, EvrcWbEncoder};
use crate::qcelp13k::{Qcelp13kDecoder, Qcelp13kEncoder};
use crate::tia96::{Tia96Decoder, Tia96Encoder};

/// Number of PCM samples per 20ms frame at 8 kHz.
pub const SAMPLES_PER_FRAME: usize = 160;

/// SO1: TIA-96 basic variable-rate voice.
pub const SERVICE_OPTION_BASIC_VOICE: u16 = 1;

/// SO3: EVRC-A / IS-127 narrowband.
pub const SERVICE_OPTION_EVRC_A: u16 = 3;

/// SO68: EVRC-B narrowband.
pub const SERVICE_OPTION_EVRC_B: u16 = 68;

/// SO70: EVRC-WB.
pub const SERVICE_OPTION_EVRC_WB: u16 = 70;

/// SO 32768: QCELP 13k variable-rate speech codec (TIA/ANSI-733).
pub const SERVICE_OPTION_QCELP_13K: u16 = 32768;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceCodec {
    /// SO1: TIA-96 / IS-96A basic variable-rate voice.
    Tia96,
    /// SO3: EVRC-A / IS-127 narrowband.
    EvrcA,
    /// SO68: EVRC-B narrowband.
    EvrcB,
    /// SO70: EVRC-WB.
    EvrcWb,
    /// SO 32768: QCELP-13K (TIA/ANSI-733).
    Qcelp13k,
}

impl VoiceCodec {
    pub fn from_service_option(service_option: u16) -> Option<Self> {
        match service_option {
            SERVICE_OPTION_BASIC_VOICE => Some(Self::Tia96),
            SERVICE_OPTION_EVRC_A => Some(Self::EvrcA),
            SERVICE_OPTION_EVRC_B => Some(Self::EvrcB),
            SERVICE_OPTION_EVRC_WB => Some(Self::EvrcWb),
            SERVICE_OPTION_QCELP_13K => Some(Self::Qcelp13k),
            _ => None,
        }
    }

    pub fn service_option(self) -> u16 {
        match self {
            Self::Tia96 => SERVICE_OPTION_BASIC_VOICE,
            Self::EvrcA => SERVICE_OPTION_EVRC_A,
            Self::EvrcB => SERVICE_OPTION_EVRC_B,
            Self::EvrcWb => SERVICE_OPTION_EVRC_WB,
            Self::Qcelp13k => SERVICE_OPTION_QCELP_13K,
        }
    }

    /// Information bits per 20 ms frame at the given rate, as a function of
    /// the codec. EVRC variants share RS1 (9600/4800/2400/1200 bps =
    /// 171/80/40/16 info bits). QCELP-13K is RS2 (14400/7200/3600/1800 bps =
    /// 266/124/54/20 info bits).
    pub fn primary_traffic_bits(self, rate: VoiceRate) -> usize {
        match self {
            Self::Tia96 | Self::EvrcA | Self::EvrcB | Self::EvrcWb => match rate {
                VoiceRate::Full => 171,
                VoiceRate::Half => 80,
                VoiceRate::Quarter => 40,
                VoiceRate::Eighth => 16,
            },
            Self::Qcelp13k => match rate {
                VoiceRate::Full => 266,
                VoiceRate::Half => 124,
                VoiceRate::Quarter => 54,
                VoiceRate::Eighth => 20,
            },
        }
    }

    /// Encoded payload bytes per 20 ms frame at the given rate.
    pub fn packet_data_bytes(self, rate: VoiceRate) -> usize {
        match self {
            Self::Tia96 | Self::EvrcA | Self::EvrcB | Self::EvrcWb => match rate {
                VoiceRate::Full => 22,
                VoiceRate::Half => 10,
                VoiceRate::Quarter => 5,
                VoiceRate::Eighth => 2,
            },
            Self::Qcelp13k => match rate {
                VoiceRate::Full => 34,
                VoiceRate::Half => 16,
                VoiceRate::Quarter => 7,
                VoiceRate::Eighth => 3,
            },
        }
    }

    /// Air-interface gross rate in bps for the given frame rate.
    pub fn rate_bps(self, rate: VoiceRate) -> u32 {
        match self {
            Self::Tia96 | Self::EvrcA | Self::EvrcB | Self::EvrcWb => match rate {
                VoiceRate::Full => 9_600,
                VoiceRate::Half => 4_800,
                VoiceRate::Quarter => 2_400,
                VoiceRate::Eighth => 1_200,
            },
            Self::Qcelp13k => match rate {
                VoiceRate::Full => 14_400,
                VoiceRate::Half => 7_200,
                VoiceRate::Quarter => 3_600,
                VoiceRate::Eighth => 1_800,
            },
        }
    }

    /// Inverse of [`rate_bps`]: recover a [`VoiceRate`] from a reported gross
    /// rate in bps. Accepts the RC3-style names (2700/1500) as aliases for
    /// Quarter/Eighth on Rate Set 1; QCELP-13K uses the RS2 rates only.
    pub fn rate_from_bps(self, bps: u32) -> Option<VoiceRate> {
        match self {
            Self::Tia96 | Self::EvrcA | Self::EvrcB | Self::EvrcWb => match bps {
                9_600 => Some(VoiceRate::Full),
                4_800 => Some(VoiceRate::Half),
                2_400 | 2_700 => Some(VoiceRate::Quarter),
                1_200 | 1_500 => Some(VoiceRate::Eighth),
                _ => None,
            },
            Self::Qcelp13k => match bps {
                14_400 => Some(VoiceRate::Full),
                7_200 => Some(VoiceRate::Half),
                3_600 => Some(VoiceRate::Quarter),
                1_800 => Some(VoiceRate::Eighth),
                _ => None,
            },
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

pub enum VoiceEncoder {
    Tia96(Tia96Encoder),
    EvrcA(EvrcEncoder),
    EvrcB(EvrcBEncoder),
    EvrcWb(EvrcWbEncoder),
    Qcelp13k(Qcelp13kEncoder),
}

impl VoiceEncoder {
    pub fn new(codec: VoiceCodec) -> Result<Self, String> {
        match codec {
            VoiceCodec::Tia96 => Ok(Self::Tia96(Tia96Encoder::new()?)),
            VoiceCodec::EvrcA => Ok(Self::EvrcA(EvrcEncoder::new()?)),
            VoiceCodec::EvrcB => Ok(Self::EvrcB(EvrcBEncoder::new()?)),
            VoiceCodec::EvrcWb => Ok(Self::EvrcWb(EvrcWbEncoder::new()?)),
            VoiceCodec::Qcelp13k => Ok(Self::Qcelp13k(Qcelp13kEncoder::new()?)),
        }
    }

    pub fn codec(&self) -> VoiceCodec {
        match self {
            VoiceEncoder::Tia96(_) => VoiceCodec::Tia96,
            VoiceEncoder::EvrcA(_) => VoiceCodec::EvrcA,
            VoiceEncoder::EvrcB(_) => VoiceCodec::EvrcB,
            VoiceEncoder::EvrcWb(_) => VoiceCodec::EvrcWb,
            VoiceEncoder::Qcelp13k(_) => VoiceCodec::Qcelp13k,
        }
    }

    pub fn encode(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
    ) -> Result<(VoiceRate, Vec<u8>), String> {
        match self {
            VoiceEncoder::Tia96(encoder) => encoder.encode(pcm),
            VoiceEncoder::EvrcA(encoder) => encoder.encode(pcm),
            VoiceEncoder::EvrcB(encoder) => encoder.encode(pcm),
            VoiceEncoder::EvrcWb(encoder) => encoder.encode_8k_input(pcm),
            VoiceEncoder::Qcelp13k(encoder) => encoder.encode(pcm),
        }
    }
}

pub enum VoiceDecoder {
    Tia96(Tia96Decoder),
    EvrcA(EvrcDecoder),
    EvrcB(EvrcBDecoder),
    EvrcWb(EvrcWbDecoder),
    Qcelp13k(Qcelp13kDecoder),
}

impl VoiceDecoder {
    pub fn new(codec: VoiceCodec) -> Result<Self, String> {
        match codec {
            VoiceCodec::Tia96 => Ok(Self::Tia96(Tia96Decoder::new()?)),
            VoiceCodec::EvrcA => Ok(Self::EvrcA(EvrcDecoder::new()?)),
            VoiceCodec::EvrcB => Ok(Self::EvrcB(EvrcBDecoder::new()?)),
            VoiceCodec::EvrcWb => Ok(Self::EvrcWb(EvrcWbDecoder::new()?)),
            VoiceCodec::Qcelp13k => Ok(Self::Qcelp13k(Qcelp13kDecoder::new()?)),
        }
    }

    pub fn decode(
        &mut self,
        rate: VoiceRate,
        payload: &[u8],
    ) -> Result<[i16; SAMPLES_PER_FRAME], String> {
        match self {
            Self::Tia96(decoder) => decoder.decode(rate, payload),
            Self::EvrcA(decoder) if rate == VoiceRate::Quarter => decoder.decode_erasure(),
            Self::EvrcA(decoder) => decoder.decode(payload),
            Self::EvrcB(decoder) => decoder.decode(rate, payload),
            Self::EvrcWb(decoder) => decoder.decode_to_8k(rate, payload),
            Self::Qcelp13k(decoder) => decoder.decode(rate, payload),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVoiceFrame {
    pub rate_bps: u32,
    pub payload: Vec<u8>,
}

pub struct VoiceTranscoder {
    source_codec: VoiceCodec,
    target_codec: VoiceCodec,
    decoder: VoiceDecoder,
    encoder: VoiceEncoder,
}

impl VoiceTranscoder {
    pub fn new(source_codec: VoiceCodec, target_codec: VoiceCodec) -> Result<Self, String> {
        if source_codec == target_codec {
            return Err("transcoder requires different source and target codecs".to_string());
        }
        Ok(Self {
            source_codec,
            target_codec,
            decoder: VoiceDecoder::new(source_codec)?,
            encoder: VoiceEncoder::new(target_codec)?,
        })
    }

    pub fn transcode(
        &mut self,
        rate_bps: u32,
        payload: &[u8],
    ) -> Result<EncodedVoiceFrame, String> {
        let source_rate = self.source_codec.rate_from_bps(rate_bps).ok_or_else(|| {
            format!(
                "rate {rate_bps} is invalid for source codec {:?}",
                self.source_codec
            )
        })?;
        let expected_bytes = self.source_codec.packet_data_bytes(source_rate);
        if payload.len() != expected_bytes {
            return Err(format!(
                "{:?} {:?} frame has {} bytes, expected {}",
                self.source_codec,
                source_rate,
                payload.len(),
                expected_bytes
            ));
        }
        let pcm = self.decoder.decode(source_rate, payload)?;
        let (target_rate, target_payload) = self.encoder.encode(&pcm)?;
        Ok(EncodedVoiceFrame {
            rate_bps: self.target_codec.rate_bps(target_rate),
            payload: target_payload,
        })
    }
}

/// A single encoded voice frame ready for MuxPDU framing on the forward
/// traffic channel.
#[derive(Debug, Clone)]
pub struct VoiceFrame {
    /// The voice codec payload bits. Length depends on the codec that
    /// produced the frame (`VoiceCodec::primary_traffic_bits(rate)`):
    /// TIA-96 and EVRC variants yield 171/80/40/16; QCELP-13K yields
    /// 266/124/54/20.
    /// Each element is 0 or 1.
    pub bits: Vec<u8>,
    /// The rate of this frame.
    pub rate: VoiceRate,
}

pub fn pack_voice_bits(bits: &[u8], bit_count: usize) -> Vec<u8> {
    let mut packed = vec![0u8; bit_count.div_ceil(8)];
    for (bit_index, bit) in bits.iter().copied().take(bit_count).enumerate() {
        packed[bit_index / 8] |= (bit & 1) << (7 - (bit_index % 8));
    }
    packed
}

pub fn unpack_voice_bits(payload: &[u8], bit_count: usize) -> Vec<u8> {
    (0..bit_count)
        .map(|bit_index| {
            payload
                .get(bit_index / 8)
                .map(|byte| (byte >> (7 - (bit_index % 8))) & 1)
                .unwrap_or(0)
        })
        .collect()
}

impl VoiceFrame {
    pub fn packet_data(&self, codec: VoiceCodec) -> Vec<u8> {
        pack_voice_bits(&self.bits, codec.primary_traffic_bits(self.rate))
    }
}

pub(crate) fn encode_pcm_frame(
    encoder: &mut VoiceEncoder,
    pcm: &[i16; SAMPLES_PER_FRAME],
) -> VoiceFrame {
    let codec = encoder.codec();
    let (rate, packet_data) = match encoder.encode(pcm) {
        Ok(result) => result,
        Err(e) => {
            log::warn!("voice encode failed ({:?}): {}, sending silence", codec, e);
            let silence_bytes = codec.packet_data_bytes(VoiceRate::Eighth);
            (VoiceRate::Eighth, vec![0u8; silence_bytes])
        }
    };

    let traffic_bits = codec.primary_traffic_bits(rate);
    let bits = unpack_voice_bits(&packet_data, traffic_bits);

    VoiceFrame { bits, rate }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_frame(phase: f64) -> [i16; SAMPLES_PER_FRAME] {
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        for (index, sample) in pcm.iter_mut().enumerate() {
            let angle = 2.0 * std::f64::consts::PI * 440.0 * index as f64 / 8_000.0 + phase;
            *sample = (angle.sin() * 8_000.0) as i16;
        }
        pcm
    }

    #[test]
    fn bit_packing_preserves_non_octet_aligned_voice_frames() {
        let bits = (0..266)
            .map(|index| (index % 3 == 0) as u8)
            .collect::<Vec<_>>();
        let packed = pack_voice_bits(&bits, bits.len());
        assert_eq!(packed.len(), 34);
        assert_eq!(unpack_voice_bits(&packed, bits.len()), bits);
    }

    #[test]
    fn basic_voice_maps_to_tia96_rate_set_one() {
        let codec = VoiceCodec::from_service_option(SERVICE_OPTION_BASIC_VOICE);
        assert_eq!(codec, Some(VoiceCodec::Tia96));
        assert_eq!(
            VoiceCodec::Tia96.service_option(),
            SERVICE_OPTION_BASIC_VOICE
        );
        assert_eq!(VoiceCodec::Tia96.primary_traffic_bits(VoiceRate::Full), 171);
        assert_eq!(VoiceCodec::Tia96.packet_data_bytes(VoiceRate::Full), 22);
    }

    #[test]
    fn tia96_and_evrc_transcode_in_both_directions() {
        let mut tia96_encoder = VoiceEncoder::new(VoiceCodec::Tia96).expect("TIA-96 encoder");
        let mut tia96_to_evrc = VoiceTranscoder::new(VoiceCodec::Tia96, VoiceCodec::EvrcA)
            .expect("TIA-96 to EVRC transcoder");
        let mut evrc_to_tia96 = VoiceTranscoder::new(VoiceCodec::EvrcA, VoiceCodec::Tia96)
            .expect("EVRC to TIA-96 transcoder");

        for index in 0..12 {
            let pcm = tone_frame(index as f64 * 0.1);
            let (tia96_rate, tia96_payload) = tia96_encoder.encode(&pcm).expect("encode TIA-96");
            let evrc = tia96_to_evrc
                .transcode(VoiceCodec::Tia96.rate_bps(tia96_rate), &tia96_payload)
                .expect("transcode to EVRC");
            let evrc_rate = VoiceCodec::EvrcA
                .rate_from_bps(evrc.rate_bps)
                .expect("valid EVRC rate");
            assert_eq!(
                evrc.payload.len(),
                VoiceCodec::EvrcA.packet_data_bytes(evrc_rate)
            );

            let tia96 = evrc_to_tia96
                .transcode(evrc.rate_bps, &evrc.payload)
                .expect("transcode to TIA-96");
            let returned_rate = VoiceCodec::Tia96
                .rate_from_bps(tia96.rate_bps)
                .expect("valid TIA-96 rate");
            assert_eq!(
                tia96.payload.len(),
                VoiceCodec::Tia96.packet_data_bytes(returned_rate)
            );
        }
    }

    #[test]
    fn every_voice_codec_pair_transcodes_in_both_directions() {
        let codecs = [
            VoiceCodec::Tia96,
            VoiceCodec::EvrcA,
            VoiceCodec::EvrcB,
            VoiceCodec::EvrcWb,
            VoiceCodec::Qcelp13k,
        ];
        for source in codecs {
            for target in codecs {
                if source == target {
                    continue;
                }
                let mut source_encoder = VoiceEncoder::new(source).expect("source encoder");
                let mut transcoder =
                    VoiceTranscoder::new(source, target).expect("pairwise transcoder");
                for index in 0..6 {
                    let pcm = tone_frame(index as f64 * 0.1);
                    let (source_rate, source_payload) =
                        source_encoder.encode(&pcm).expect("source encode");
                    let output = transcoder
                        .transcode(source.rate_bps(source_rate), &source_payload)
                        .expect("pairwise transcode");
                    let output_rate = target.rate_from_bps(output.rate_bps).expect("target rate");
                    assert_eq!(output.payload.len(), target.packet_data_bytes(output_rate));
                }
            }
        }
    }

    #[test]
    fn evrc_and_qcelp_transcode_in_both_directions() {
        let mut evrc_encoder = VoiceEncoder::new(VoiceCodec::EvrcA).expect("EVRC encoder");
        let mut evrc_to_qcelp = VoiceTranscoder::new(VoiceCodec::EvrcA, VoiceCodec::Qcelp13k)
            .expect("EVRC to QCELP transcoder");
        let mut qcelp_to_evrc = VoiceTranscoder::new(VoiceCodec::Qcelp13k, VoiceCodec::EvrcA)
            .expect("QCELP to EVRC transcoder");

        for index in 0..12 {
            let pcm = tone_frame(index as f64 * 0.1);
            let (evrc_rate, evrc_payload) = evrc_encoder.encode(&pcm).expect("encode EVRC");
            let qcelp = evrc_to_qcelp
                .transcode(VoiceCodec::EvrcA.rate_bps(evrc_rate), &evrc_payload)
                .expect("transcode to QCELP");
            let qcelp_rate = VoiceCodec::Qcelp13k
                .rate_from_bps(qcelp.rate_bps)
                .expect("valid QCELP rate");
            assert_eq!(
                qcelp.payload.len(),
                VoiceCodec::Qcelp13k.packet_data_bytes(qcelp_rate)
            );

            let evrc = qcelp_to_evrc
                .transcode(qcelp.rate_bps, &qcelp.payload)
                .expect("transcode to EVRC");
            let returned_rate = VoiceCodec::EvrcA
                .rate_from_bps(evrc.rate_bps)
                .expect("valid EVRC rate");
            assert_eq!(
                evrc.payload.len(),
                VoiceCodec::EvrcA.packet_data_bytes(returned_rate)
            );
        }
    }

    #[test]
    fn evrc_a_quarter_rate_is_transcoded_as_an_erasure() {
        let mut transcoder = VoiceTranscoder::new(VoiceCodec::EvrcA, VoiceCodec::Qcelp13k)
            .expect("EVRC to QCELP transcoder");
        let output = transcoder
            .transcode(2_700, &[0x5a; 5])
            .expect("quarter-rate EVRC-A erasure");
        let rate = VoiceCodec::Qcelp13k
            .rate_from_bps(output.rate_bps)
            .expect("QCELP rate");
        assert_eq!(
            output.payload.len(),
            VoiceCodec::Qcelp13k.packet_data_bytes(rate)
        );
    }
}
