//! Safe Rust wrappers around the EVRC-B and EVRC-WB C++ reference codec.
//!
//! The C++ reference codec is wrapped with per-instance native state, so
//! independent encoders and decoders may run concurrently.

use std::ffi::c_void;

use crate::{SAMPLES_PER_FRAME, VoiceRate};

const EVRCB_MODE: i32 = 1;
const EVRCWB_MODE: i32 = 2;
const BITSTREAM_WORDS: usize = 11;
const WB_SAMPLES_PER_FRAME: usize = 320;

unsafe extern "C" {
    fn evrcbw_encoder_init(mode: i32) -> *mut c_void;
    fn evrcbw_encoder_init_with_operating_point(mode: i32, operating_point: i32) -> *mut c_void;
    fn evrcbw_encoder_uninit(c: *mut c_void);
    fn evrcbw_encoder_encode_to_words(
        c: *mut c_void,
        speech: *const i16,
        speech_samples: usize,
        rate: *mut i16,
        words: *mut i16,
        words_capacity: usize,
    ) -> i32;

    fn evrcbw_decoder_init(mode: i32) -> *mut c_void;
    fn evrcbw_decoder_uninit(c: *mut c_void);
    fn evrcbw_decoder_decode_from_words(
        c: *mut c_void,
        rate: i16,
        words: *const i16,
        words_count: usize,
        speech: *mut i16,
        speech_max_samples: usize,
    ) -> i32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvrcBOperatingPoint {
    Op0,
    Op1,
    Op2,
}

impl EvrcBOperatingPoint {
    fn as_native(self) -> i32 {
        match self {
            Self::Op0 => 0,
            Self::Op1 => 1,
            Self::Op2 => 2,
        }
    }
}

fn native_rate_to_voice_rate(rate: i16) -> Option<VoiceRate> {
    match rate {
        4 => Some(VoiceRate::Full),
        3 => Some(VoiceRate::Half),
        2 => Some(VoiceRate::Quarter),
        1 => Some(VoiceRate::Eighth),
        _ => None,
    }
}

fn voice_rate_to_native_rate(rate: VoiceRate) -> i16 {
    match rate {
        VoiceRate::Full => 4,
        VoiceRate::Half => 3,
        VoiceRate::Quarter => 2,
        VoiceRate::Eighth => 1,
    }
}

fn words_to_payload(rate: VoiceRate, words: &[i16; BITSTREAM_WORDS]) -> Vec<u8> {
    let bit_count = rate.primary_traffic_bits();
    let byte_count = bit_count.div_ceil(8);
    let mut payload = vec![0u8; byte_count];

    for bit_index in 0..bit_count {
        let word_index = bit_index / 16;
        let word_bit = 15 - (bit_index % 16);
        let bit = ((words[word_index] as u16 >> word_bit) & 1) as u8;
        payload[bit_index / 8] |= bit << (7 - (bit_index % 8));
    }

    payload
}

fn payload_to_words(rate: VoiceRate, payload: &[u8]) -> [i16; BITSTREAM_WORDS] {
    let bit_count = rate.primary_traffic_bits();
    let mut words = [0i16; BITSTREAM_WORDS];

    for bit_index in 0..bit_count {
        let byte = payload.get(bit_index / 8).copied().unwrap_or(0);
        let bit = (byte >> (7 - (bit_index % 8))) & 1;
        if bit != 0 {
            let word_index = bit_index / 16;
            let word_bit = 15 - (bit_index % 16);
            words[word_index] = (words[word_index] as u16 | (1u16 << word_bit)) as i16;
        }
    }

    words
}

fn upsample_8k_to_16k(pcm: &[i16; SAMPLES_PER_FRAME]) -> [i16; WB_SAMPLES_PER_FRAME] {
    let mut wide = [0i16; WB_SAMPLES_PER_FRAME];
    for (i, pair) in wide.chunks_exact_mut(2).enumerate() {
        let current = pcm[i] as i32;
        let next = pcm.get(i + 1).copied().unwrap_or(pcm[i]) as i32;
        pair[0] = pcm[i];
        pair[1] = ((current + next) / 2) as i16;
    }
    wide
}

fn downsample_16k_to_8k(pcm: &[i16; WB_SAMPLES_PER_FRAME]) -> [i16; SAMPLES_PER_FRAME] {
    let mut narrow = [0i16; SAMPLES_PER_FRAME];
    for (i, sample) in narrow.iter_mut().enumerate() {
        let a = pcm[2 * i] as i32;
        let b = pcm[2 * i + 1] as i32;
        *sample = ((a + b) / 2) as i16;
    }
    narrow
}

pub struct EvrcBEncoder {
    handle: *mut c_void,
}

unsafe impl Send for EvrcBEncoder {}

impl EvrcBEncoder {
    pub fn new() -> Result<Self, String> {
        Self::new_with_operating_point(EvrcBOperatingPoint::Op0)
    }

    pub fn new_with_operating_point(operating_point: EvrcBOperatingPoint) -> Result<Self, String> {
        let handle = unsafe {
            evrcbw_encoder_init_with_operating_point(EVRCB_MODE, operating_point.as_native())
        };
        if handle.is_null() {
            return Err("evrcbw_encoder_init(EVRC-B) returned null".into());
        }
        Ok(Self { handle })
    }

    pub fn encode(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
    ) -> Result<(VoiceRate, Vec<u8>), String> {
        encode_native(self.handle, pcm)
    }
}

impl Drop for EvrcBEncoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { evrcbw_encoder_uninit(self.handle) };
        }
    }
}

pub struct EvrcWbEncoder {
    handle: *mut c_void,
}

unsafe impl Send for EvrcWbEncoder {}

impl EvrcWbEncoder {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { evrcbw_encoder_init(EVRCWB_MODE) };
        if handle.is_null() {
            return Err("evrcbw_encoder_init(EVRC-WB) returned null".into());
        }
        Ok(Self { handle })
    }

    pub fn encode_8k_input(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
    ) -> Result<(VoiceRate, Vec<u8>), String> {
        let wide = upsample_8k_to_16k(pcm);
        let mut native_rate = 0i16;
        let mut words = [0i16; BITSTREAM_WORDS];
        let ret = unsafe {
            evrcbw_encoder_encode_to_words(
                self.handle,
                wide.as_ptr(),
                WB_SAMPLES_PER_FRAME,
                &mut native_rate,
                words.as_mut_ptr(),
                BITSTREAM_WORDS,
            )
        };
        if ret != BITSTREAM_WORDS as i32 {
            return Err(format!("evrcbw_encoder_encode_to_words returned {}", ret));
        }
        let rate = native_rate_to_voice_rate(native_rate)
            .ok_or_else(|| format!("unexpected EVRC-WB rate {}", native_rate))?;
        Ok((rate, words_to_payload(rate, &words)))
    }
}

impl Drop for EvrcWbEncoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { evrcbw_encoder_uninit(self.handle) };
        }
    }
}

fn encode_native(
    handle: *mut c_void,
    pcm: &[i16; SAMPLES_PER_FRAME],
) -> Result<(VoiceRate, Vec<u8>), String> {
    let mut native_rate = 0i16;
    let mut words = [0i16; BITSTREAM_WORDS];
    let ret = unsafe {
        evrcbw_encoder_encode_to_words(
            handle,
            pcm.as_ptr(),
            SAMPLES_PER_FRAME,
            &mut native_rate,
            words.as_mut_ptr(),
            BITSTREAM_WORDS,
        )
    };
    if ret != BITSTREAM_WORDS as i32 {
        return Err(format!("evrcbw_encoder_encode_to_words returned {}", ret));
    }
    let rate = native_rate_to_voice_rate(native_rate)
        .ok_or_else(|| format!("unexpected EVRC-B rate {}", native_rate))?;
    Ok((rate, words_to_payload(rate, &words)))
}

pub struct EvrcBDecoder {
    handle: *mut c_void,
}

unsafe impl Send for EvrcBDecoder {}

impl EvrcBDecoder {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { evrcbw_decoder_init(EVRCB_MODE) };
        if handle.is_null() {
            return Err("evrcbw_decoder_init(EVRC-B) returned null".into());
        }
        Ok(Self { handle })
    }

    pub fn decode(
        &mut self,
        rate: VoiceRate,
        payload: &[u8],
    ) -> Result<[i16; SAMPLES_PER_FRAME], String> {
        decode_native_8k(self.handle, rate, payload)
    }
}

impl Drop for EvrcBDecoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { evrcbw_decoder_uninit(self.handle) };
        }
    }
}

pub struct EvrcWbDecoder {
    handle: *mut c_void,
}

unsafe impl Send for EvrcWbDecoder {}

impl EvrcWbDecoder {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { evrcbw_decoder_init(EVRCWB_MODE) };
        if handle.is_null() {
            return Err("evrcbw_decoder_init(EVRC-WB) returned null".into());
        }
        Ok(Self { handle })
    }

    pub fn decode_to_8k(
        &mut self,
        rate: VoiceRate,
        payload: &[u8],
    ) -> Result<[i16; SAMPLES_PER_FRAME], String> {
        let words = payload_to_words(rate, payload);
        let mut wide = [0i16; WB_SAMPLES_PER_FRAME];
        let ret = unsafe {
            evrcbw_decoder_decode_from_words(
                self.handle,
                voice_rate_to_native_rate(rate),
                words.as_ptr(),
                BITSTREAM_WORDS,
                wide.as_mut_ptr(),
                WB_SAMPLES_PER_FRAME,
            )
        };
        if ret <= 0 {
            return Err(format!("evrcbw_decoder_decode_from_words returned {}", ret));
        }
        if ret as usize == WB_SAMPLES_PER_FRAME {
            Ok(downsample_16k_to_8k(&wide))
        } else {
            let mut narrow = [0i16; SAMPLES_PER_FRAME];
            narrow.copy_from_slice(&wide[..SAMPLES_PER_FRAME]);
            Ok(narrow)
        }
    }
}

impl Drop for EvrcWbDecoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { evrcbw_decoder_uninit(self.handle) };
        }
    }
}

fn decode_native_8k(
    handle: *mut c_void,
    rate: VoiceRate,
    payload: &[u8],
) -> Result<[i16; SAMPLES_PER_FRAME], String> {
    let words = payload_to_words(rate, payload);
    let mut speech = [0i16; SAMPLES_PER_FRAME];
    let ret = unsafe {
        evrcbw_decoder_decode_from_words(
            handle,
            voice_rate_to_native_rate(rate),
            words.as_ptr(),
            BITSTREAM_WORDS,
            speech.as_mut_ptr(),
            SAMPLES_PER_FRAME,
        )
    };
    if ret <= 0 {
        return Err(format!("evrcbw_decoder_decode_from_words returned {}", ret));
    }
    Ok(speech)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    const EVRC_B_VECTOR_INPUT: &[u8] = include_bytes!("../tests/evrc_b_vectors/input.pcm");
    const EVRC_B_VECTOR_OP0_PACKET: &[u8] = include_bytes!("../tests/evrc_b_vectors/enc.op0.pkt");
    const EVRC_B_VECTOR_OP1_PACKET: &[u8] = include_bytes!("../tests/evrc_b_vectors/enc.op1.pkt");
    const EVRC_B_VECTOR_OP2_PACKET: &[u8] = include_bytes!("../tests/evrc_b_vectors/enc.op2.pkt");
    const EVRC_B_VECTOR_OP0_DECODED: &[u8] = include_bytes!("../tests/evrc_b_vectors/dec.op0.out");
    const EVRC_B_VECTOR_OP1_DECODED: &[u8] = include_bytes!("../tests/evrc_b_vectors/dec.op1.out");
    const EVRC_B_VECTOR_OP2_DECODED: &[u8] = include_bytes!("../tests/evrc_b_vectors/dec.op2.out");

    #[test]
    fn evrc_b_encodes_silence() {
        let mut enc = EvrcBEncoder::new().expect("encoder init");
        let silence = [0i16; SAMPLES_PER_FRAME];
        let (rate, data) = enc.encode(&silence).expect("encode");
        assert_eq!(data.len(), rate.primary_traffic_bits().div_ceil(8));
    }

    #[test]
    fn evrc_wb_encodes_8k_input() {
        let mut enc = EvrcWbEncoder::new().expect("encoder init");
        let silence = [0i16; SAMPLES_PER_FRAME];
        let (rate, data) = enc.encode_8k_input(&silence).expect("encode");
        assert!(matches!(
            rate,
            VoiceRate::Full | VoiceRate::Half | VoiceRate::Eighth
        ));
        assert_eq!(data.len(), rate.primary_traffic_bits().div_ceil(8));
    }

    #[test]
    fn evrc_b_decodes_reference_packet_vectors() {
        for (packet, expected) in [
            (EVRC_B_VECTOR_OP0_PACKET, EVRC_B_VECTOR_OP0_DECODED),
            (EVRC_B_VECTOR_OP1_PACKET, EVRC_B_VECTOR_OP1_DECODED),
            (EVRC_B_VECTOR_OP2_PACKET, EVRC_B_VECTOR_OP2_DECODED),
        ] {
            let decoded = decode_reference_packet_vector(packet, expected);
            assert_eq!(decoded.len(), pcm_bytes_to_samples(expected).len());
        }
    }

    #[test]
    #[ignore = "vB EVRC-B v1.5 PCM vectors are not bit-exact with the vC EVRC-B/WB v2.0 reference"]
    fn evrc_b_decoded_reference_packets_match_v1_5_pcm_vectors() {
        let mut failures = Vec::new();
        for (packet, expected) in [
            (EVRC_B_VECTOR_OP0_PACKET, EVRC_B_VECTOR_OP0_DECODED),
            (EVRC_B_VECTOR_OP1_PACKET, EVRC_B_VECTOR_OP1_DECODED),
            (EVRC_B_VECTOR_OP2_PACKET, EVRC_B_VECTOR_OP2_DECODED),
        ] {
            let decoded = decode_reference_packet_vector(packet, expected);
            let expected = pcm_bytes_to_samples(expected);
            let stats = pcm_diff_stats(&decoded, &expected);
            if stats.mismatch_count != 0 {
                failures.push(stats);
            }
        }

        assert!(
            failures.is_empty(),
            "EVRC-B decoded packet vector mismatches: {failures:?}"
        );
    }

    #[test]
    fn evrc_b_operating_points_encode_reference_input() {
        let input = pcm_bytes_to_samples(EVRC_B_VECTOR_INPUT);
        let frame_count = EVRC_B_VECTOR_OP0_PACKET.len() / ((BITSTREAM_WORDS + 1) * 2);

        for operating_point in [
            EvrcBOperatingPoint::Op0,
            EvrcBOperatingPoint::Op1,
            EvrcBOperatingPoint::Op2,
        ] {
            let mut encoder =
                EvrcBEncoder::new_with_operating_point(operating_point).expect("encoder init");

            for frame in input[40..]
                .chunks_exact(SAMPLES_PER_FRAME)
                .take(frame_count)
            {
                let mut pcm = [0i16; SAMPLES_PER_FRAME];
                pcm.copy_from_slice(frame);
                let (rate, payload) = encoder.encode(&pcm).expect("encode vector frame");
                assert_eq!(payload.len(), rate.primary_traffic_bits().div_ceil(8));
            }
        }
    }

    #[test]
    fn evrc_b_independent_encoders_can_run_concurrently() {
        let expected_a = encode_evrc_b_reference_frames();
        let expected_b = encode_evrc_b_reference_frames();
        assert_eq!(expected_b, expected_a);

        thread::scope(|scope| {
            let left = scope.spawn(encode_evrc_b_reference_frames);
            let right = scope.spawn(encode_evrc_b_reference_frames);

            assert_eq!(left.join().expect("left thread"), expected_a);
            assert_eq!(right.join().expect("right thread"), expected_b);
        });
    }

    #[test]
    fn evrc_b_independent_decoders_can_run_concurrently() {
        let expected_a = decode_evrc_b_reference_frames();
        let expected_b = decode_evrc_b_reference_frames();
        assert_eq!(expected_b, expected_a);

        thread::scope(|scope| {
            let left = scope.spawn(decode_evrc_b_reference_frames);
            let right = scope.spawn(decode_evrc_b_reference_frames);

            assert_eq!(left.join().expect("left thread"), expected_a);
            assert_eq!(right.join().expect("right thread"), expected_b);
        });
    }

    #[test]
    fn evrc_wb_independent_encoders_can_run_concurrently() {
        let expected_a = encode_evrc_wb_frames();
        let expected_b = encode_evrc_wb_frames();
        assert_eq!(expected_b, expected_a);

        thread::scope(|scope| {
            let left = scope.spawn(encode_evrc_wb_frames);
            let right = scope.spawn(encode_evrc_wb_frames);

            assert_eq!(left.join().expect("left thread"), expected_a);
            assert_eq!(right.join().expect("right thread"), expected_b);
        });
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PcmDiffStats {
        sample_count: usize,
        mismatch_count: usize,
        max_abs_diff: i32,
        first_mismatch: Option<(usize, i16, i16)>,
    }

    fn decode_reference_packet_vector(packet_bytes: &[u8], expected_bytes: &[u8]) -> Vec<i16> {
        let expected = pcm_bytes_to_samples(expected_bytes);
        let frame_bytes = (BITSTREAM_WORDS + 1) * 2;
        assert_eq!(packet_bytes.len() % frame_bytes, 0);
        assert_eq!(
            packet_bytes.len() / frame_bytes,
            expected.len() / SAMPLES_PER_FRAME
        );

        let mut decoder = EvrcBDecoder::new().expect("decoder init");
        let mut decoded = Vec::with_capacity(expected.len());
        for packet in packet_bytes.chunks_exact(frame_bytes) {
            let native_rate = i16::from_be_bytes([packet[0], packet[1]]);
            let rate = native_rate_to_voice_rate(native_rate)
                .unwrap_or_else(|| panic!("unexpected EVRC-B vector rate {}", native_rate));
            let mut words = [0i16; BITSTREAM_WORDS];
            for (word, bytes) in words.iter_mut().zip(packet[2..].chunks_exact(2)) {
                *word = i16::from_be_bytes([bytes[0], bytes[1]]);
            }
            let payload = words_to_payload(rate, &words);
            let speech = decoder.decode(rate, &payload).expect("decode vector frame");
            decoded.extend_from_slice(&speech);
        }

        decoded
    }

    fn encode_evrc_b_reference_frames() -> Vec<(VoiceRate, Vec<u8>)> {
        let input = pcm_bytes_to_samples(EVRC_B_VECTOR_INPUT);
        let mut encoder = EvrcBEncoder::new().expect("encoder init");
        input[40..]
            .chunks_exact(SAMPLES_PER_FRAME)
            .take(6)
            .map(|frame| {
                let mut pcm = [0i16; SAMPLES_PER_FRAME];
                pcm.copy_from_slice(frame);
                encoder.encode(&pcm).expect("encode vector frame")
            })
            .collect()
    }

    fn decode_evrc_b_reference_frames() -> Vec<[i16; SAMPLES_PER_FRAME]> {
        let frame_bytes = (BITSTREAM_WORDS + 1) * 2;
        let mut decoder = EvrcBDecoder::new().expect("decoder init");
        EVRC_B_VECTOR_OP0_PACKET
            .chunks_exact(frame_bytes)
            .take(6)
            .map(|packet| {
                let native_rate = i16::from_be_bytes([packet[0], packet[1]]);
                let rate = native_rate_to_voice_rate(native_rate)
                    .unwrap_or_else(|| panic!("unexpected EVRC-B vector rate {}", native_rate));
                let mut words = [0i16; BITSTREAM_WORDS];
                for (word, bytes) in words.iter_mut().zip(packet[2..].chunks_exact(2)) {
                    *word = i16::from_be_bytes([bytes[0], bytes[1]]);
                }
                let payload = words_to_payload(rate, &words);
                decoder.decode(rate, &payload).expect("decode vector frame")
            })
            .collect()
    }

    fn encode_evrc_wb_frames() -> Vec<(VoiceRate, Vec<u8>)> {
        let mut encoder = EvrcWbEncoder::new().expect("encoder init");
        let frames = [
            [0i16; SAMPLES_PER_FRAME],
            tone_frame(350.0, 5000.0),
            tone_frame(950.0, 7000.0),
            tone_frame(1250.0, 4000.0),
        ];
        frames
            .iter()
            .map(|frame| encoder.encode_8k_input(frame).expect("encode wb frame"))
            .collect()
    }

    fn tone_frame(hz: f64, amplitude: f64) -> [i16; SAMPLES_PER_FRAME] {
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        for (i, sample) in pcm.iter_mut().enumerate() {
            *sample =
                (amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / 8000.0).sin()) as i16;
        }
        pcm
    }

    fn pcm_diff_stats(actual: &[i16], expected: &[i16]) -> PcmDiffStats {
        assert_eq!(actual.len(), expected.len());
        let mut mismatch_count = 0usize;
        let mut max_abs_diff = 0i32;
        let mut first_mismatch = None;
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            let diff = i32::from(*actual) - i32::from(*expected);
            let abs_diff = diff.abs();
            if abs_diff != 0 {
                mismatch_count += 1;
                max_abs_diff = max_abs_diff.max(abs_diff);
                first_mismatch.get_or_insert((index, *actual, *expected));
            }
        }
        PcmDiffStats {
            sample_count: actual.len(),
            mismatch_count,
            max_abs_diff,
            first_mismatch,
        }
    }

    fn pcm_bytes_to_samples(bytes: &[u8]) -> Vec<i16> {
        assert_eq!(bytes.len() % 2, 0);
        bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_be_bytes([chunk[0], chunk[1]]))
            .collect()
    }
}
