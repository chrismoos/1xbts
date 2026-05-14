//! Pre-encode a WAV ringtone into every supported voice codec.
//!
//! Used at upload time by the HLR's `SetSubscriberRingtone` RPC. Reads a
//! WAV upload, normalizes it to 8 kHz mono 16-bit PCM, appends 200 ms of
//! silence (so loop-wrap discontinuities sit in silence), then for each
//! [`VoiceCodec`] runs a single stateful encoder over the whole stream and
//! serializes the frames via [`encode_frames_to_bytes`].

use std::io::Cursor;

use crate::encoded_stream::encode_frames_to_bytes;
use crate::resample::resample_linear_mono;
use crate::{SAMPLES_PER_FRAME, VoiceCodec, VoiceEncoder, encode_pcm_frame};

/// Output sample rate / framing parameters are fixed at 8 kHz / 20 ms / 160
/// samples per frame.
const TARGET_SAMPLE_RATE: u32 = 8000;
/// Trailing silence appended before encoding so the loop-wrap point falls
/// inside silence; see module docs.
const TRAILING_SILENCE_MS: usize = 200;

#[derive(Debug)]
pub enum PreencodeError {
    /// File could not be parsed as a WAV.
    UnsupportedOrCorrupt(String),
    /// Input is non-PCM or otherwise unusable.
    Unsupported(String),
    /// WAV contained no audio samples.
    Empty,
    /// Encoded output for at least one codec exceeded the size cap.
    TooLong { codec: VoiceCodec, bytes: usize },
}

impl std::fmt::Display for PreencodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedOrCorrupt(s) => write!(f, "unsupported or corrupt WAV: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported WAV: {s}"),
            Self::Empty => write!(f, "WAV is empty"),
            Self::TooLong { codec, bytes } => {
                write!(f, "encoded ringtone too long for {codec:?} ({bytes} bytes)")
            }
        }
    }
}

impl std::error::Error for PreencodeError {}

#[derive(Debug)]
pub struct PreencodedRingtone {
    pub codec: VoiceCodec,
    pub bytes: Vec<u8>,
    pub frame_count: usize,
    pub duration_ms: u32,
}

const ALL_CODECS: [VoiceCodec; 3] = [VoiceCodec::EvrcA, VoiceCodec::EvrcB, VoiceCodec::EvrcWb];

/// Preencode the WAV into every supported codec. `max_encoded_bytes_per_codec`
/// is the maximum allowed serialized blob size per codec; encoding stops with
/// `TooLong` if any codec exceeds it.
pub fn preencode_wav_all_codecs(
    wav_bytes: &[u8],
    max_encoded_bytes_per_codec: usize,
) -> Result<Vec<PreencodedRingtone>, PreencodeError> {
    let pcm = decode_wav_to_8k_mono_i16(wav_bytes)?;
    if pcm.is_empty() {
        return Err(PreencodeError::Empty);
    }

    let mut framed = pad_to_frame_multiple(pcm);
    let trailing_silence = TRAILING_SILENCE_MS * (TARGET_SAMPLE_RATE as usize) / 1000;
    framed.extend(std::iter::repeat_n(0i16, trailing_silence));

    let frame_count = framed.len() / SAMPLES_PER_FRAME;
    let duration_ms = (frame_count * 20) as u32;

    let mut out = Vec::with_capacity(ALL_CODECS.len());
    for &codec in &ALL_CODECS {
        let mut encoder = VoiceEncoder::new(codec)
            .map_err(|e| PreencodeError::Unsupported(format!("encoder init {codec:?}: {e}")))?;
        let mut frames = Vec::with_capacity(frame_count);
        let mut pcm_frame = [0i16; SAMPLES_PER_FRAME];
        for chunk in framed.chunks_exact(SAMPLES_PER_FRAME) {
            pcm_frame.copy_from_slice(chunk);
            frames.push(encode_pcm_frame(&mut encoder, &pcm_frame));
        }
        let bytes = encode_frames_to_bytes(&frames);
        if bytes.len() > max_encoded_bytes_per_codec {
            return Err(PreencodeError::TooLong {
                codec,
                bytes: bytes.len(),
            });
        }
        out.push(PreencodedRingtone {
            codec,
            bytes,
            frame_count,
            duration_ms,
        });
    }
    Ok(out)
}

/// Read a WAV from bytes and return 8 kHz mono i16 samples.
fn decode_wav_to_8k_mono_i16(wav_bytes: &[u8]) -> Result<Vec<i16>, PreencodeError> {
    let reader = hound::WavReader::new(Cursor::new(wav_bytes))
        .map_err(|e| PreencodeError::UnsupportedOrCorrupt(e.to_string()))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int {
        return Err(PreencodeError::Unsupported(
            "only integer PCM WAV is supported".to_string(),
        ));
    }
    if !(spec.bits_per_sample == 8
        || spec.bits_per_sample == 16
        || spec.bits_per_sample == 24
        || spec.bits_per_sample == 32)
    {
        return Err(PreencodeError::Unsupported(format!(
            "unsupported bits_per_sample {}",
            spec.bits_per_sample
        )));
    }
    if spec.channels == 0 || spec.channels > 2 {
        return Err(PreencodeError::Unsupported(format!(
            "unsupported channel count {}",
            spec.channels
        )));
    }
    if spec.sample_rate < 4000 || spec.sample_rate > 96_000 {
        return Err(PreencodeError::Unsupported(format!(
            "unsupported sample_rate {}",
            spec.sample_rate
        )));
    }

    let channels = spec.channels as usize;
    let bits = spec.bits_per_sample;

    // Read interleaved samples as i32, then downmix and convert to i16.
    let samples_i32: Vec<i32> = reader
        .into_samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| PreencodeError::UnsupportedOrCorrupt(format!("read samples: {e}")))?;

    let shift = match bits {
        8 => |x: i32| ((x.clamp(0, 255) - 128) << 8) as i16, // 8-bit is unsigned
        16 => |x: i32| x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        24 => |x: i32| (x >> 8).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        32 => |x: i32| (x >> 16).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        _ => unreachable!(),
    };
    // Note: hound returns 8-bit PCM as unsigned 0..=255 in i32 (per format spec).

    let mono: Vec<i16> = if channels == 1 {
        samples_i32.into_iter().map(shift).collect()
    } else {
        samples_i32
            .chunks_exact(channels)
            .map(|c| {
                let sum: i64 = c.iter().map(|&v| v as i64).sum();
                let avg = (sum / channels as i64) as i32;
                shift(avg)
            })
            .collect()
    };

    let resampled = if spec.sample_rate == TARGET_SAMPLE_RATE {
        mono
    } else {
        resample_linear_mono(&mono, spec.sample_rate, TARGET_SAMPLE_RATE)
    };

    Ok(resampled)
}

fn pad_to_frame_multiple(mut samples: Vec<i16>) -> Vec<i16> {
    let rem = samples.len() % SAMPLES_PER_FRAME;
    if rem != 0 {
        samples.resize(samples.len() + (SAMPLES_PER_FRAME - rem), 0);
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_wav(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut buf, spec).unwrap();
            for &s in samples {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn rejects_non_wav() {
        let err = preencode_wav_all_codecs(b"not a wav file", 256 * 1024).unwrap_err();
        matches!(err, PreencodeError::UnsupportedOrCorrupt(_));
    }

    #[test]
    fn rejects_empty_audio() {
        let bytes = make_wav(8000, 1, &[]);
        let err = preencode_wav_all_codecs(&bytes, 256 * 1024).unwrap_err();
        matches!(err, PreencodeError::Empty);
    }

    #[test]
    fn encodes_8k_mono_identity_path() {
        // 100 ms of silence at 8 kHz mono.
        let samples = vec![0i16; 800];
        let bytes = make_wav(8000, 1, &samples);
        let out = preencode_wav_all_codecs(&bytes, 256 * 1024).expect("preencode");
        assert_eq!(out.len(), 3);
        // 800 samples = 5 frames; 200 ms of silence at 8 kHz = 1600 samples = 10 frames.
        let expected_frames = 5 + 10;
        for r in &out {
            assert_eq!(r.frame_count, expected_frames);
            assert_eq!(r.duration_ms as usize, expected_frames * 20);
            assert!(!r.bytes.is_empty());
        }
    }

    #[test]
    fn encodes_44100_stereo_resample_path() {
        // 250 ms of silence at 44.1 kHz stereo.
        let n = 11_025 * 2; // 250 ms * 2 channels
        let samples = vec![0i16; n];
        let bytes = make_wav(44100, 2, &samples);
        let out = preencode_wav_all_codecs(&bytes, 256 * 1024).expect("preencode");
        // Resampled to 8 kHz: ~2000 samples = ~12.5 frames -> padded to 13 + 10 silence = 23 frames
        for r in &out {
            assert!(r.frame_count >= 13 + 10);
            assert!(r.frame_count <= 13 + 10 + 1);
            assert!(!r.bytes.is_empty());
        }
    }

    #[test]
    fn enforces_size_cap() {
        // Long enough to blow the cap.
        let samples = vec![0i16; 8000 * 10]; // 10 seconds at 8k mono
        let bytes = make_wav(8000, 1, &samples);
        let err = preencode_wav_all_codecs(&bytes, 64).unwrap_err(); // absurdly small cap
        matches!(err, PreencodeError::TooLong { .. });
    }
}
