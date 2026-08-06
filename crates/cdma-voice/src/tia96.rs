//! TIA-96 / IS-96A Service Option 1 speech codec.

use std::ffi::c_void;

use crate::{SAMPLES_PER_FRAME, VoiceRate};

unsafe extern "C" {
    fn tia96_encoder_init() -> *mut c_void;
    fn tia96_encoder_uninit(ctx: *mut c_void);
    fn tia96_encoder_encode_to_packet(
        ctx: *mut c_void,
        speech: *const i16,
        samples: usize,
        packet: *mut u8,
        max_bytes: usize,
    ) -> i32;

    fn tia96_decoder_init() -> *mut c_void;
    fn tia96_decoder_uninit(ctx: *mut c_void);
    fn tia96_decoder_decode_from_packet(
        ctx: *mut c_void,
        packet: *const u8,
        bytes: usize,
        speech: *mut i16,
        max_samples: usize,
    ) -> i32;
}

const MAX_PACKET_BYTES: usize = 23;

mod mode {
    pub const EIGHTH: u8 = 1;
    pub const QUARTER: u8 = 2;
    pub const HALF: u8 = 3;
    pub const FULL: u8 = 4;
}

fn mode_to_rate(mode: u8) -> Option<VoiceRate> {
    match mode {
        mode::FULL => Some(VoiceRate::Full),
        mode::HALF => Some(VoiceRate::Half),
        mode::QUARTER => Some(VoiceRate::Quarter),
        mode::EIGHTH => Some(VoiceRate::Eighth),
        _ => None,
    }
}

fn rate_to_mode(rate: VoiceRate) -> u8 {
    match rate {
        VoiceRate::Full => 4,
        VoiceRate::Half => 3,
        VoiceRate::Quarter => 2,
        VoiceRate::Eighth => 1,
    }
}

/// TIA-96 speech encoder wrapping the Qualcomm reference codec.
pub struct Tia96Encoder {
    handle: *mut c_void,
}

// SAFETY: all mutable native state is owned by this handle and accessed
// through &mut self. Reference lookup tables are immutable.
unsafe impl Send for Tia96Encoder {}

impl Tia96Encoder {
    /// Create a variable-rate encoder.
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { tia96_encoder_init() };
        if handle.is_null() {
            return Err("tia96_encoder_init returned null".into());
        }
        Ok(Self { handle })
    }

    /// Encode one 20 ms PCM frame.
    ///
    /// The codec needs 60 samples from the following input block. The first
    /// call buffers input and returns a valid eighth-rate silence frame.
    pub fn encode(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
    ) -> Result<(VoiceRate, Vec<u8>), String> {
        let mut packet = [0u8; MAX_PACKET_BYTES];
        let result = unsafe {
            tia96_encoder_encode_to_packet(
                self.handle,
                pcm.as_ptr(),
                pcm.len(),
                packet.as_mut_ptr(),
                packet.len(),
            )
        };
        if result <= 0 || result as usize > packet.len() {
            return Err(format!("tia96_encoder_encode_to_packet returned {result}"));
        }
        let rate = mode_to_rate(packet[0])
            .ok_or_else(|| format!("unexpected TIA-96 mode byte 0x{:02x}", packet[0]))?;
        Ok((rate, packet[1..result as usize].to_vec()))
    }
}

impl Drop for Tia96Encoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { tia96_encoder_uninit(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// TIA-96 speech decoder wrapping the Qualcomm reference codec.
pub struct Tia96Decoder {
    handle: *mut c_void,
}

// SAFETY: the native handle owns its mutable state and is accessed through
// &mut self. Reference lookup tables are immutable.
unsafe impl Send for Tia96Decoder {}

impl Tia96Decoder {
    /// Create a decoder with post-filtering enabled.
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { tia96_decoder_init() };
        if handle.is_null() {
            return Err("tia96_decoder_init returned null".into());
        }
        Ok(Self { handle })
    }

    /// Decode a rate-byte-stripped TIA-96 payload into one PCM frame.
    pub fn decode(
        &mut self,
        rate: VoiceRate,
        payload: &[u8],
    ) -> Result<[i16; SAMPLES_PER_FRAME], String> {
        let mut packet = Vec::with_capacity(1 + payload.len());
        packet.push(rate_to_mode(rate));
        packet.extend_from_slice(payload);
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        let result = unsafe {
            tia96_decoder_decode_from_packet(
                self.handle,
                packet.as_ptr(),
                packet.len(),
                pcm.as_mut_ptr(),
                pcm.len(),
            )
        };
        if result != SAMPLES_PER_FRAME as i32 {
            return Err(format!(
                "tia96_decoder_decode_from_packet returned {result}"
            ));
        }
        Ok(pcm)
    }
}

impl Drop for Tia96Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { tia96_decoder_uninit(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    const INPUT: &[u8] = include_bytes!("../csrc/tia96/vectors/m01v0000.raw");
    const PACKETS: &[u8] = include_bytes!("../csrc/tia96/vectors/m01v0000.pkt");
    const DECODED: &[u8] = include_bytes!("../csrc/tia96/vectors/m01v0000.mstr.raw");

    fn pcm_samples(bytes: &[u8]) -> Vec<i16> {
        bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect()
    }

    fn reference_packets() -> Vec<(VoiceRate, Vec<u8>)> {
        PACKETS
            .chunks_exact(24)
            .map(|frame| {
                let mode = u16::from_le_bytes([frame[0], frame[1]]) as u8;
                let rate = mode_to_rate(mode).expect("reference rate");
                let payload_len = match rate {
                    VoiceRate::Full => 22,
                    VoiceRate::Half => 10,
                    VoiceRate::Quarter => 5,
                    VoiceRate::Eighth => 2,
                };
                let mut payload = Vec::with_capacity(payload_len);
                for word in frame[2..].chunks_exact(2) {
                    let value = u16::from_le_bytes([word[0], word[1]]);
                    payload.extend_from_slice(&value.to_be_bytes());
                }
                payload.truncate(payload_len);
                (rate, payload)
            })
            .collect()
    }

    fn encode_reference_input() -> Vec<(VoiceRate, Vec<u8>)> {
        let references = reference_packets();
        let mut samples = pcm_samples(INPUT);
        samples.resize(references.len() * SAMPLES_PER_FRAME, 0);
        let mut encoder = Tia96Encoder::new().expect("encoder");
        let mut encoded = Vec::with_capacity(references.len());
        for (index, frame) in samples.chunks_exact(SAMPLES_PER_FRAME).enumerate() {
            let pcm: &[i16; SAMPLES_PER_FRAME] = frame.try_into().expect("PCM frame");
            let packet = encoder.encode(pcm).expect("encode");
            if index != 0 {
                encoded.push(packet);
            }
        }
        encoded.push(
            encoder
                .encode(&[0; SAMPLES_PER_FRAME])
                .expect("flush look-ahead"),
        );
        encoded
    }

    fn decode_reference_packets() -> Vec<i16> {
        let mut decoder = Tia96Decoder::new().expect("decoder");
        reference_packets()
            .into_iter()
            .flat_map(|(rate, payload)| decoder.decode(rate, &payload).expect("decode"))
            .collect()
    }

    #[test]
    fn encoder_tracks_qualcomm_reference_vector() {
        let actual = encode_reference_input();
        let expected = reference_packets();
        assert_eq!(actual.len(), expected.len());
        assert_eq!(&actual[..16], &expected[..16]);
        assert!(actual.iter().zip(&expected).all(|(a, e)| a.0 == e.0));
    }

    #[test]
    fn decoder_tracks_qualcomm_reference_vector() {
        let decoded = decode_reference_packets();
        let expected = pcm_samples(DECODED);
        assert_eq!(decoded.len(), expected.len());
        assert_eq!(
            &decoded[..16 * SAMPLES_PER_FRAME],
            &expected[..16 * SAMPLES_PER_FRAME]
        );
        let (signal_energy, error_energy) = decoded.iter().zip(&expected).fold(
            (0.0f64, 0.0f64),
            |(signal, error), (actual, expected)| {
                let expected = f64::from(*expected);
                let delta = f64::from(*actual) - expected;
                (signal + expected * expected, error + delta * delta)
            },
        );
        let snr_db = 10.0 * (signal_energy / error_energy).log10();
        assert!(snr_db > 30.0, "reference decode SNR was {snr_db:.2} dB");
    }

    #[test]
    fn independent_instances_are_reentrant() {
        let encoded_reference = encode_reference_input();
        let decoded_reference = decode_reference_packets();
        thread::scope(|scope| {
            let workers = (0..4)
                .map(|_| scope.spawn(|| (encode_reference_input(), decode_reference_packets())))
                .collect::<Vec<_>>();
            for worker in workers {
                let (encoded, decoded) = worker.join().expect("codec worker");
                assert_eq!(encoded, encoded_reference);
                assert_eq!(decoded, decoded_reference);
            }
        });
    }

    #[test]
    fn encoded_silence_decodes_quietly() {
        let mut encoder = Tia96Encoder::new().expect("encoder");
        let mut decoder = Tia96Decoder::new().expect("decoder");
        for _ in 0..3 {
            let (rate, payload) = encoder
                .encode(&[0; SAMPLES_PER_FRAME])
                .expect("encode silence");
            let decoded = decoder.decode(rate, &payload).expect("decode silence");
            let energy = decoded
                .iter()
                .map(|sample| i64::from(*sample).pow(2))
                .sum::<i64>();
            assert!(energy < 10_000);
        }
    }
}
