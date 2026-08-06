//! Safe Rust wrappers around the EVRC C reference codec.
//!
//! Provides [`EvrcEncoder`] and [`EvrcDecoder`] that call through to the
//! TIA IS-127 bit-exact fixed-point C implementation vendored in `csrc/`.
//!
//! # Thread safety
//!
//! The vendored C reference codec is wrapped with per-instance native state,
//! so independent encoders and decoders may run concurrently.

use std::ffi::c_void;

use crate::VoiceRate;

// ---------------------------------------------------------------------------
// FFI declarations matching evrcc.h
// ---------------------------------------------------------------------------
unsafe extern "C" {
    fn evrc_encoder_init(min_rate: i16, max_rate: i16, noise_suppression: i16) -> *mut c_void;
    fn evrc_encoder_uninit(c: *mut c_void);
    fn evrc_encoder_encode_to_packet(
        c: *mut c_void,
        speech: *mut i16,
        speech_samples: usize,
        packet: *mut u8,
        packet_max_bytes: usize,
    ) -> i32;

    fn evrc_decoder_init() -> *mut c_void;
    fn evrc_decoder_uninit(c: *mut c_void);
    fn evrc_decoder_decode_from_packet(
        c: *mut c_void,
        packet: *const u8,
        packet_bytes: usize,
        speech: *mut i16,
        speech_max_samples: usize,
    ) -> i32;
    fn evrc_decoder_decode_erasure(
        c: *mut c_void,
        speech: *mut i16,
        speech_max_samples: usize,
    ) -> i32;
}

// ---------------------------------------------------------------------------
// EVRC packet sizes per rate (single-frame packet = 1-byte header + data)
//   Full  (Rate 1)   : 22 data bytes  -> packet = 1 + 1 + 22 = 24 (header byte + rate-table + data)
//   Half  (Rate 1/2) : 10 data bytes
//   Quarter (Rate 1/4): NOT used by EVRC (rate index 2 is unused)
//   Eighth (Rate 1/8): 2 data bytes
//
// The packet format from evrc_encoder_encode_to_packet uses the compact
// "EVRC 8K packet" format: 1 byte frame-count + ceil(frame_count/4) bytes
// rate header + data.  For a single frame the total packet sizes are:
//   Full  : 1 + 1 + 22 = 24
//   Half  : 1 + 1 + 10 = 12
//   Eighth: 1 + 1 + 2  = 4
//
// EVRC Rate Set 1 mapping to CDMA MuxPDU rates:
//   EVRC Full rate   (171 bits / 22 bytes) -> VoiceRate::Full   (9600 bps, 171 traffic bits)
//   EVRC Half rate   (80 bits / 10 bytes)  -> VoiceRate::Half   (4800 bps, 80 traffic bits)
//   EVRC Eighth rate (16 bits / 2 bytes)   -> VoiceRate::Eighth (1200 bps, 16 traffic bits)
// ---------------------------------------------------------------------------

/// Maps an EVRC packet data size to the corresponding voice rate.
fn data_size_to_rate(data_bytes: usize) -> Option<VoiceRate> {
    match data_bytes {
        22 => Some(VoiceRate::Full),
        10 => Some(VoiceRate::Half),
        2 => Some(VoiceRate::Eighth),
        _ => None,
    }
}

/// Maximum single-frame packet size (full rate).
const MAX_PACKET_BYTES: usize = 64;

/// Samples per 20 ms frame at 8 kHz.
const FRAME_SAMPLES: usize = 160;

// ---------------------------------------------------------------------------
// EvrcEncoder
// ---------------------------------------------------------------------------

/// EVRC speech encoder wrapping the C reference codec.
pub struct EvrcEncoder {
    handle: *mut c_void,
}

// The native handle owns its codec state and is only used through &mut self.
unsafe impl Send for EvrcEncoder {}

impl EvrcEncoder {
    /// Create a new encoder. Uses min_rate=1, max_rate=4 (full rate),
    /// noise_suppression=0 (disabled -- the evrcc.c wrapper handles the
    /// shift_r fallback when NS is off).
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { evrc_encoder_init(1, 4, 0) };
        if handle.is_null() {
            return Err("evrc_encoder_init returned null".into());
        }
        Ok(Self { handle })
    }

    /// Encode one 160-sample PCM frame.
    ///
    /// Returns `(rate, packet_data)` where `packet_data` is the raw EVRC
    /// codec bits for this frame (22, 10, or 2 bytes depending on rate).
    pub fn encode(&mut self, pcm: &[i16; FRAME_SAMPLES]) -> Result<(VoiceRate, Vec<u8>), String> {
        let mut speech = *pcm; // mutable copy -- the C encoder may modify in-place
        let mut packet = [0u8; MAX_PACKET_BYTES];

        let ret = unsafe {
            evrc_encoder_encode_to_packet(
                self.handle,
                speech.as_mut_ptr(),
                FRAME_SAMPLES,
                packet.as_mut_ptr(),
                MAX_PACKET_BYTES,
            )
        };

        if ret <= 0 {
            return Err(format!("evrc_encoder_encode_to_packet returned {}", ret));
        }

        let packet_len = ret as usize;

        // The packet format for a single frame is:
        //   byte 0: frame_count (1)
        //   byte 1: 2-bit rate in upper bits (packed rate header)
        //   bytes 2..: frame data
        // So the data size = packet_len - 2 (1 byte count + 1 byte rate header for 1 frame)
        if packet_len < 2 {
            return Err(format!("packet too small: {} bytes", packet_len));
        }
        let data_size = packet_len - 2;
        let rate = data_size_to_rate(data_size)
            .ok_or_else(|| format!("unexpected EVRC data size: {} bytes", data_size))?;

        // Extract just the frame data (skip the packet header)
        let data = packet[2..packet_len].to_vec();

        Ok((rate, data))
    }
}

impl Drop for EvrcEncoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                evrc_encoder_uninit(self.handle);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EvrcDecoder
// ---------------------------------------------------------------------------

/// EVRC speech decoder wrapping the C reference codec.
pub struct EvrcDecoder {
    handle: *mut c_void,
}

unsafe impl Send for EvrcDecoder {}

impl EvrcDecoder {
    /// Create a new decoder.
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { evrc_decoder_init() };
        if handle.is_null() {
            return Err("evrc_decoder_init returned null".into());
        }
        Ok(Self { handle })
    }

    /// Decode a single EVRC packet frame into 160 PCM samples.
    ///
    /// `packet_data` should be the raw EVRC frame data (22, 10, or 2 bytes)
    /// as produced by [`EvrcEncoder::encode`].  The rate is inferred from the
    /// data length.
    pub fn decode(&mut self, packet_data: &[u8]) -> Result<[i16; FRAME_SAMPLES], String> {
        let rate = data_size_to_rate(packet_data.len())
            .ok_or_else(|| format!("unexpected packet size: {} bytes", packet_data.len()))?;

        // Build a single-frame EVRC 8K packet:
        //   byte 0: frame_count = 1
        //   byte 1: 2-bit rate in top bits
        //   bytes 2..: frame data
        let packet_rate = match rate {
            VoiceRate::Full => 3u8, // EVRC8K_RATE_FULL
            VoiceRate::Half => 2u8, // EVRC8K_RATE_HALF
            VoiceRate::Quarter => unreachable!("EVRC-A has no quarter-rate speech packet"),
            VoiceRate::Eighth => 0u8, // EVRC8K_RATE_EIGHT
        };

        let mut packet = Vec::with_capacity(2 + packet_data.len());
        packet.push(1u8); // frame_count = 1
        packet.push(packet_rate << 6); // rate in top 2 bits of header byte
        packet.extend_from_slice(packet_data);

        let mut speech = [0i16; FRAME_SAMPLES];

        let ret = unsafe {
            evrc_decoder_decode_from_packet(
                self.handle,
                packet.as_ptr(),
                packet.len(),
                speech.as_mut_ptr(),
                FRAME_SAMPLES,
            )
        };

        if ret <= 0 {
            return Err(format!("evrc_decoder_decode_from_packet returned {}", ret));
        }

        Ok(speech)
    }

    pub fn decode_erasure(&mut self) -> Result<[i16; FRAME_SAMPLES], String> {
        let mut speech = [0i16; FRAME_SAMPLES];
        let ret =
            unsafe { evrc_decoder_decode_erasure(self.handle, speech.as_mut_ptr(), FRAME_SAMPLES) };
        if ret <= 0 {
            return Err(format!("evrc_decoder_decode_erasure returned {}", ret));
        }
        Ok(speech)
    }
}

impl Drop for EvrcDecoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                evrc_decoder_uninit(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn tone_frame(hz: f64, amplitude: f64) -> [i16; FRAME_SAMPLES] {
        let mut pcm = [0i16; FRAME_SAMPLES];
        for (i, sample) in pcm.iter_mut().enumerate() {
            *sample =
                (amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / 8000.0).sin()) as i16;
        }
        pcm
    }

    fn encode_sequence() -> Vec<(VoiceRate, Vec<u8>)> {
        let mut encoder = EvrcEncoder::new().expect("encoder init");
        let frames = [
            [0i16; FRAME_SAMPLES],
            tone_frame(300.0, 6000.0),
            tone_frame(700.0, 9000.0),
            tone_frame(1100.0, 5000.0),
        ];
        frames
            .iter()
            .map(|frame| encoder.encode(frame).expect("encode frame"))
            .collect()
    }

    #[test]
    fn test_encode_silence() {
        let mut enc = EvrcEncoder::new().expect("encoder init");
        let silence = [0i16; 160];
        let (rate, data) = enc.encode(&silence).expect("encode silence");
        // Silence should encode to eighth rate (2 bytes) or possibly higher
        // for the very first frame due to encoder startup transients.
        assert!(!data.is_empty(), "encoded data should not be empty");
        assert!(
            matches!(rate, VoiceRate::Full | VoiceRate::Half | VoiceRate::Eighth),
            "rate should be a valid EVRC rate, got {:?}",
            rate
        );
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let mut enc = EvrcEncoder::new().expect("encoder init");
        let mut dec = EvrcDecoder::new().expect("decoder init");

        let pcm = tone_frame(400.0, 8000.0);

        let (_rate, data) = enc.encode(&pcm).expect("encode");
        assert!(!data.is_empty(), "encoded data should not be empty");

        let decoded = dec.decode(&data).expect("decode");
        // We can't expect bit-exact roundtrip (lossy codec), but the output
        // should not be all zeros for a tone input.
        let energy: i64 = decoded.iter().map(|&s| (s as i64) * (s as i64)).sum();
        assert!(energy > 0, "decoded output should have non-zero energy");
    }

    #[test]
    fn decoder_accepts_explicit_erasure() {
        let mut decoder = EvrcDecoder::new().expect("decoder init");
        decoder.decode_erasure().expect("decode erasure");
    }

    #[test]
    fn independent_encoders_can_run_concurrently() {
        let expected_a = encode_sequence();
        let expected_b = encode_sequence();
        assert_eq!(expected_b, expected_a);

        thread::scope(|scope| {
            let left = scope.spawn(encode_sequence);
            let right = scope.spawn(encode_sequence);

            assert_eq!(left.join().expect("left thread"), expected_a);
            assert_eq!(right.join().expect("right thread"), expected_b);
        });
    }

    #[test]
    fn independent_decoders_can_run_concurrently() {
        let packets = encode_sequence();
        let decode_sequence = || {
            let mut decoder = EvrcDecoder::new().expect("decoder init");
            packets
                .iter()
                .map(|(_, payload)| decoder.decode(payload).expect("decode frame"))
                .collect::<Vec<_>>()
        };
        let expected_a = decode_sequence();
        let expected_b = decode_sequence();
        assert_eq!(expected_b, expected_a);

        thread::scope(|scope| {
            let left = scope.spawn(decode_sequence);
            let right = scope.spawn(decode_sequence);

            assert_eq!(left.join().expect("left thread"), expected_a);
            assert_eq!(right.join().expect("right thread"), expected_b);
        });
    }
}
