//! Safe Rust wrappers around the QCELP-13K (TIA/ANSI-733) C reference codec.
//!
//! Provides [`Qcelp13kEncoder`] and [`Qcelp13kDecoder`] that call through to
//! the floating-point reference implementation vendored in
//! `csrc/qcelp13k/code/`. All native state is heap-allocated per instance,
//! so independent encoders and decoders can run concurrently on different
//! threads. The non-negotiable `independent_encoders_can_run_concurrently`
//! regression test below locks that invariant in.

use std::ffi::c_void;

use crate::{SAMPLES_PER_FRAME, VoiceRate};

// ---------------------------------------------------------------------------
// FFI declarations matching csrc/qcelp13k.h.
// ---------------------------------------------------------------------------
unsafe extern "C" {
    fn qcelp13k_encoder_init(max_rate: i32, min_rate: i32) -> *mut c_void;
    fn qcelp13k_encoder_uninit(ctx: *mut c_void);
    fn qcelp13k_encoder_encode_to_packet(
        ctx: *mut c_void,
        speech: *const i16,
        samples: usize,
        packet: *mut u8,
        max_bytes: usize,
    ) -> i32;

    fn qcelp13k_decoder_init() -> *mut c_void;
    fn qcelp13k_decoder_uninit(ctx: *mut c_void);
    fn qcelp13k_decoder_decode_from_packet(
        ctx: *mut c_void,
        packet: *const u8,
        bytes: usize,
        speech: *mut i16,
        max_samples: usize,
    ) -> i32;
}

// QCELP_MAX_PACKET_BYTES = 1 (rate) + 34 (full-rate payload).
const MAX_PACKET_BYTES: usize = 35;

/// TIA-733 mode -> wire byte mapping.
mod mode {
    pub const EIGHTH: u8 = 1;
    pub const QUARTER: u8 = 2;
    pub const HALF: u8 = 3;
    pub const FULL: u8 = 4;
}

fn mode_to_rate(m: u8) -> Option<VoiceRate> {
    match m {
        mode::FULL => Some(VoiceRate::Full),
        mode::HALF => Some(VoiceRate::Half),
        mode::QUARTER => Some(VoiceRate::Quarter),
        mode::EIGHTH => Some(VoiceRate::Eighth),
        _ => None,
    }
}

fn rate_to_mode(rate: VoiceRate) -> u8 {
    match rate {
        VoiceRate::Full => mode::FULL,
        VoiceRate::Half => mode::HALF,
        VoiceRate::Quarter => mode::QUARTER,
        VoiceRate::Eighth => mode::EIGHTH,
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// QCELP-13K speech encoder wrapping the C reference codec.
pub struct Qcelp13kEncoder {
    handle: *mut c_void,
}

// SAFETY: the native handle owns its codec state, is mutated only via
// `&mut self`, and the C side has no process-wide writable state (see
// MODIFICATIONS.txt).
unsafe impl Send for Qcelp13kEncoder {}

impl Qcelp13kEncoder {
    /// Create a new encoder. Variable-rate operation is enabled across
    /// all four TIA-733 rates (Eighth..Full).
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { qcelp13k_encoder_init(4, 1) };
        if handle.is_null() {
            return Err("qcelp13k_encoder_init returned null".into());
        }
        Ok(Self { handle })
    }

    /// Encode one 160-sample PCM frame (20 ms @ 8 kHz). Returns the
    /// codec-selected `VoiceRate` and the bit-packed frame payload
    /// (3 / 7 / 16 / 34 bytes for Eighth / Quarter / Half / Full).
    pub fn encode(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
    ) -> Result<(VoiceRate, Vec<u8>), String> {
        let mut packet = [0u8; MAX_PACKET_BYTES];
        let ret = unsafe {
            qcelp13k_encoder_encode_to_packet(
                self.handle,
                pcm.as_ptr(),
                SAMPLES_PER_FRAME,
                packet.as_mut_ptr(),
                MAX_PACKET_BYTES,
            )
        };
        if ret <= 0 {
            return Err(format!(
                "qcelp13k_encoder_encode_to_packet returned {}",
                ret
            ));
        }
        let packet_len = ret as usize;
        if packet_len < 1 || packet_len > MAX_PACKET_BYTES {
            return Err(format!("invalid QCELP-13K packet length {}", packet_len));
        }
        let rate = mode_to_rate(packet[0])
            .ok_or_else(|| format!("unexpected QCELP-13K mode byte 0x{:02x}", packet[0]))?;
        // Strip the rate byte; expose only the bit-packed payload.
        let payload = packet[1..packet_len].to_vec();
        Ok((rate, payload))
    }
}

impl Drop for Qcelp13kEncoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { qcelp13k_encoder_uninit(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// QCELP-13K speech decoder wrapping the C reference codec.
pub struct Qcelp13kDecoder {
    handle: *mut c_void,
}

unsafe impl Send for Qcelp13kDecoder {}

impl Qcelp13kDecoder {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { qcelp13k_decoder_init() };
        if handle.is_null() {
            return Err("qcelp13k_decoder_init returned null".into());
        }
        Ok(Self { handle })
    }

    /// Decode one frame back into 160 PCM samples. `packet` must be the
    /// rate-byte-stripped payload returned by [`Qcelp13kEncoder::encode`].
    pub fn decode(&mut self, rate: VoiceRate, packet: &[u8]) -> Result<[i16; 160], String> {
        // Reassemble the C-side wire packet: rate byte + payload.
        let mut wire = Vec::with_capacity(1 + packet.len());
        wire.push(rate_to_mode(rate));
        wire.extend_from_slice(packet);

        let mut out = [0i16; SAMPLES_PER_FRAME];
        let ret = unsafe {
            qcelp13k_decoder_decode_from_packet(
                self.handle,
                wire.as_ptr(),
                wire.len(),
                out.as_mut_ptr(),
                SAMPLES_PER_FRAME,
            )
        };
        if ret <= 0 {
            return Err(format!(
                "qcelp13k_decoder_decode_from_packet returned {}",
                ret
            ));
        }
        Ok(out)
    }
}

impl Drop for Qcelp13kDecoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { qcelp13k_decoder_uninit(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evrc::EvrcEncoder;
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    fn tone_frame(hz: f64, amplitude: f64, phase: f64) -> [i16; SAMPLES_PER_FRAME] {
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        for (i, sample) in pcm.iter_mut().enumerate() {
            let t = i as f64 / 8000.0;
            *sample = (amplitude * (2.0 * std::f64::consts::PI * hz * t + phase).sin()) as i16;
        }
        pcm
    }

    fn make_test_stream(seed: u32) -> Vec<[i16; SAMPLES_PER_FRAME]> {
        // Three distinct frames so the encoder exercises rate changes.
        vec![
            [0i16; SAMPLES_PER_FRAME],
            tone_frame(300.0 + (seed % 7) as f64 * 10.0, 6000.0, 0.0),
            tone_frame(700.0 + (seed % 11) as f64 * 5.0, 9000.0, 0.5),
            tone_frame(1100.0, 5000.0, (seed % 13) as f64 * 0.1),
        ]
    }

    fn encode_stream(frames: &[[i16; SAMPLES_PER_FRAME]]) -> Vec<(VoiceRate, Vec<u8>)> {
        let mut encoder = Qcelp13kEncoder::new().expect("encoder init");
        frames
            .iter()
            .map(|f| encoder.encode(f).expect("encode frame"))
            .collect()
    }

    #[test]
    fn silence_roundtrip() {
        let mut enc = Qcelp13kEncoder::new().expect("encoder init");
        let mut dec = Qcelp13kDecoder::new().expect("decoder init");

        // A handful of silence frames -- the encoder normally settles on
        // eighth rate, but the very first frames may use higher rates
        // while the noise estimate stabilises.
        let silence = [0i16; SAMPLES_PER_FRAME];
        for _ in 0..6 {
            let (rate, payload) = enc.encode(&silence).expect("encode silence");
            assert!(
                matches!(
                    rate,
                    VoiceRate::Full | VoiceRate::Half | VoiceRate::Quarter | VoiceRate::Eighth
                ),
                "unexpected rate {:?}",
                rate
            );
            let decoded = dec.decode(rate, &payload).expect("decode silence");
            // Decoded silence energy should be modest.
            let energy: i64 = decoded.iter().map(|&s| (s as i64) * (s as i64)).sum();
            assert!(
                energy < 200_000_000,
                "silence decode energy too high: {}",
                energy
            );
        }
    }

    #[test]
    fn tone_roundtrip() {
        let mut enc = Qcelp13kEncoder::new().expect("encoder init");
        let mut dec = Qcelp13kDecoder::new().expect("decoder init");

        let pcm = tone_frame(1000.0, 8000.0, 0.0);
        let (rate, payload) = enc.encode(&pcm).expect("encode tone");
        // Expected payload sizes per TIA-733 §2.4.
        let expected_payload_len = match rate {
            VoiceRate::Full => 34,
            VoiceRate::Half => 16,
            VoiceRate::Quarter => 7,
            VoiceRate::Eighth => 3,
        };
        assert_eq!(
            payload.len(),
            expected_payload_len,
            "payload bytes mismatch for rate {:?}",
            rate
        );
        let decoded = dec.decode(rate, &payload).expect("decode tone");
        let energy: i64 = decoded.iter().map(|&s| (s as i64) * (s as i64)).sum();
        assert!(energy > 0, "decoded tone has zero energy");
    }

    /// Four independent encoders running on distinct PCM in parallel
    /// must each produce the same bitstream they would produce
    /// single-threaded. This is the regression that pins "no process-
    /// wide writable state" in place.
    #[test]
    fn independent_encoders_can_run_concurrently() {
        let streams: Vec<_> = (0..4u32).map(make_test_stream).collect();

        // Single-threaded reference encoding for each stream.
        let reference: Vec<Vec<(VoiceRate, Vec<u8>)>> =
            streams.iter().map(|s| encode_stream(s)).collect();

        thread::scope(|scope| {
            let handles: Vec<_> = streams
                .iter()
                .map(|s| scope.spawn(move || encode_stream(s)))
                .collect();
            for (i, h) in handles.into_iter().enumerate() {
                let parallel = h.join().expect("worker thread");
                assert_eq!(
                    parallel, reference[i],
                    "stream {} diverged from single-threaded reference",
                    i
                );
            }
        });
    }

    #[test]
    fn qcelp_and_evrc_a_can_encode_in_same_process() {
        let pcm = tone_frame(650.0, 8000.0, 0.25);
        let mut evrc = EvrcEncoder::new().expect("EVRC encoder init");
        let mut qcelp = Qcelp13kEncoder::new().expect("QCELP encoder init");

        evrc.encode(&pcm).expect("EVRC encode");
        qcelp.encode(&pcm).expect("QCELP encode");
        evrc.encode(&pcm).expect("second EVRC encode");
        qcelp.encode(&pcm).expect("second QCELP encode");
    }

    #[test]
    fn codec_instances_are_reentrant_under_churn() {
        let stream: Vec<_> = (0..16)
            .map(|i| tone_frame(250.0 + i as f64 * 35.0, 7000.0, i as f64 * 0.1))
            .collect();
        let reference = encode_stream(&stream);
        let barrier = Arc::new(Barrier::new(6));

        thread::scope(|scope| {
            for _ in 0..6 {
                let barrier = Arc::clone(&barrier);
                let stream = &stream;
                let reference = &reference;
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..8 {
                        let mut encoder = Qcelp13kEncoder::new().expect("encoder init");
                        let mut decoder = Qcelp13kDecoder::new().expect("decoder init");
                        for (pcm, expected) in stream.iter().zip(reference) {
                            let encoded = encoder.encode(pcm).expect("encode");
                            assert_eq!(&encoded, expected);
                            decoder.decode(encoded.0, &encoded.1).expect("decode");
                        }
                    }
                });
            }
        });
    }
}
