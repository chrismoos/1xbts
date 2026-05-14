//! Serialization of a sequence of pre-encoded [`VoiceFrame`]s.
//!
//! Used to store custom subscriber ringtones in the HLR: at upload time the
//! audio is encoded once into each supported [`VoiceCodec`] and the resulting
//! frame stream is serialized via [`encode_frames_to_bytes`] for storage. At
//! ringback time, an [`EncodedFrameReader`] reads frames back out and loops
//! at end-of-stream.
//!
//! Wire format (per frame):
//!   u8   rate code   (1 = Full, 2 = Half, 3 = Quarter, 4 = Eighth)
//!   u8[] packed bits, ceil(N/8) bytes, MSB-first, where
//!        N = rate.primary_traffic_bits()
//!
//! No file header — frames are concatenated.

use crate::{VoiceFrame, VoiceRate};

const RATE_FULL: u8 = 1;
const RATE_HALF: u8 = 2;
const RATE_QUARTER: u8 = 3;
const RATE_EIGHTH: u8 = 4;

fn rate_to_code(rate: VoiceRate) -> u8 {
    match rate {
        VoiceRate::Full => RATE_FULL,
        VoiceRate::Half => RATE_HALF,
        VoiceRate::Quarter => RATE_QUARTER,
        VoiceRate::Eighth => RATE_EIGHTH,
    }
}

fn code_to_rate(code: u8) -> Option<VoiceRate> {
    match code {
        RATE_FULL => Some(VoiceRate::Full),
        RATE_HALF => Some(VoiceRate::Half),
        RATE_QUARTER => Some(VoiceRate::Quarter),
        RATE_EIGHTH => Some(VoiceRate::Eighth),
        _ => None,
    }
}

/// Serialize a sequence of voice frames into the storage byte format.
pub fn encode_frames_to_bytes(frames: &[VoiceFrame]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.len() * 24);
    for frame in frames {
        let bit_count = frame.rate.primary_traffic_bits();
        let byte_count = bit_count.div_ceil(8);
        out.push(rate_to_code(frame.rate));
        let mut packed = vec![0u8; byte_count];
        for (i, &bit) in frame.bits.iter().enumerate().take(bit_count) {
            if bit & 1 != 0 {
                let byte_idx = i / 8;
                let bit_idx = 7 - (i % 8);
                packed[byte_idx] |= 1 << bit_idx;
            }
        }
        out.extend_from_slice(&packed);
    }
    out
}

/// Reader over a serialized frame stream. Loops back to the start on EOF.
pub struct EncodedFrameReader {
    bytes: Vec<u8>,
    cursor: usize,
    frame_count: usize,
}

impl EncodedFrameReader {
    /// Build a reader, scanning the stream once to compute `frame_count` and
    /// verify well-formedness. Returns an error on a malformed stream.
    pub fn new(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("encoded frame stream is empty".to_string());
        }
        let mut frame_count = 0usize;
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let code = bytes[cursor];
            let rate = code_to_rate(code)
                .ok_or_else(|| format!("invalid rate code {} at offset {}", code, cursor))?;
            let byte_count = rate.primary_traffic_bits().div_ceil(8);
            cursor += 1 + byte_count;
            if cursor > bytes.len() {
                return Err(format!(
                    "truncated frame at offset {} (need {} bytes)",
                    cursor - byte_count,
                    byte_count
                ));
            }
            frame_count += 1;
        }
        Ok(Self {
            bytes,
            cursor: 0,
            frame_count,
        })
    }

    /// Total number of frames in the stream.
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Read the next frame, looping back to the start at end-of-stream.
    pub fn next_frame(&mut self) -> VoiceFrame {
        if self.cursor >= self.bytes.len() {
            self.cursor = 0;
        }
        let code = self.bytes[self.cursor];
        // Reader was validated in `new`, so the code is always valid.
        let rate = code_to_rate(code).expect("validated in new");
        let bit_count = rate.primary_traffic_bits();
        let byte_count = bit_count.div_ceil(8);
        let start = self.cursor + 1;
        let end = start + byte_count;
        let mut bits = Vec::with_capacity(bit_count);
        for i in 0..bit_count {
            let byte = self.bytes[start + (i / 8)];
            let bit_idx = 7 - (i % 8);
            bits.push((byte >> bit_idx) & 1);
        }
        self.cursor = end;
        VoiceFrame { bits, rate }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(rate: VoiceRate, seed: u64) -> VoiceFrame {
        let n = rate.primary_traffic_bits();
        let mut bits = Vec::with_capacity(n);
        let mut s = seed;
        for _ in 0..n {
            // simple LCG for deterministic 0/1 bits
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            bits.push((s >> 33) as u8 & 1);
        }
        VoiceFrame { bits, rate }
    }

    #[test]
    fn round_trip_all_rates() {
        let frames = vec![
            make_frame(VoiceRate::Full, 1),
            make_frame(VoiceRate::Half, 2),
            make_frame(VoiceRate::Quarter, 3),
            make_frame(VoiceRate::Eighth, 4),
            make_frame(VoiceRate::Full, 5),
        ];
        let bytes = encode_frames_to_bytes(&frames);
        let mut reader = EncodedFrameReader::new(bytes).expect("reader");
        assert_eq!(reader.frame_count(), frames.len());
        for expected in &frames {
            let got = reader.next_frame();
            assert_eq!(got.rate, expected.rate);
            assert_eq!(got.bits, expected.bits);
        }
    }

    #[test]
    fn reader_loops_at_eof() {
        let frames = vec![make_frame(VoiceRate::Eighth, 7)];
        let bytes = encode_frames_to_bytes(&frames);
        let mut reader = EncodedFrameReader::new(bytes).expect("reader");
        let a = reader.next_frame();
        let b = reader.next_frame();
        let c = reader.next_frame();
        assert_eq!(a.bits, b.bits);
        assert_eq!(b.bits, c.bits);
    }

    #[test]
    fn invalid_rate_code_rejected() {
        let mut bytes = encode_frames_to_bytes(&[make_frame(VoiceRate::Full, 1)]);
        bytes[0] = 99;
        assert!(EncodedFrameReader::new(bytes).is_err());
    }

    #[test]
    fn truncated_stream_rejected() {
        let mut bytes = encode_frames_to_bytes(&[make_frame(VoiceRate::Full, 1)]);
        bytes.truncate(bytes.len() - 1);
        assert!(EncodedFrameReader::new(bytes).is_err());
    }

    #[test]
    fn empty_stream_rejected() {
        assert!(EncodedFrameReader::new(vec![]).is_err());
    }
}
