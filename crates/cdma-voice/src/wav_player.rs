//! WAV file voice player for CDMA2000 forward traffic channel.
//!
//! Reads an 8 kHz mono 16-bit PCM WAV file and produces encoded voice
//! frames suitable for transmission on the forward traffic channel.

use crate::{SAMPLES_PER_FRAME, VoiceCodec, VoiceEncoder, VoiceFrame, encode_pcm_frame};
use log::info;

/// Reads a WAV file and produces voice frames for forward traffic channel
/// transmission. Each call to `next_frame()` returns the next 20ms frame,
/// or `None` when the file is exhausted.
pub struct WavVoicePlayer {
    /// All PCM samples from the WAV file, resampled/converted to 8 kHz mono i16.
    samples: Vec<i16>,
    /// Current read position in samples.
    position: usize,
    /// Voice encoder instance.
    encoder: VoiceEncoder,
}

impl WavVoicePlayer {
    /// Open a WAV file and prepare for playback.
    ///
    /// The WAV file should be 8 kHz, mono, 16-bit PCM. Other formats will
    /// be rejected with an error.
    pub fn open(path: &str) -> Result<Self, String> {
        Self::open_with_codec(path, VoiceCodec::EvrcA)
    }

    /// Open a WAV file and encode playback with the selected CDMA voice codec.
    pub fn open_with_codec(path: &str, codec: VoiceCodec) -> Result<Self, String> {
        let reader =
            hound::WavReader::open(path).map_err(|e| format!("failed to open WAV: {}", e))?;

        let spec = reader.spec();
        if spec.sample_rate != 8000 {
            return Err(format!("WAV must be 8000 Hz, got {} Hz", spec.sample_rate));
        }
        if spec.channels != 1 {
            return Err(format!(
                "WAV must be mono (1 channel), got {} channels",
                spec.channels
            ));
        }
        if spec.bits_per_sample != 16 {
            return Err(format!(
                "WAV must be 16-bit, got {}-bit",
                spec.bits_per_sample
            ));
        }
        if spec.sample_format != hound::SampleFormat::Int {
            return Err("WAV must be integer (PCM) format".to_string());
        }

        let samples: Vec<i16> = reader
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to read WAV samples: {}", e))?;

        let duration_ms = (samples.len() as f64 / 8.0) as u64;
        info!(
            "WavVoicePlayer: loaded {} samples ({} ms) from {}",
            samples.len(),
            duration_ms,
            path
        );

        let encoder = VoiceEncoder::new(codec)?;

        Ok(Self {
            samples,
            position: 0,
            encoder,
        })
    }

    /// Return the next 20ms voice frame, looping back to the start when
    /// the WAV is exhausted. Always returns `Some` — callers should tear
    /// down the call externally (e.g. via Release Order from the MS).
    ///
    /// Each frame is EVRC-encoded at the codec's chosen variable rate.
    /// The returned [`VoiceFrame`] contains the EVRC packet bits (each
    /// element is 0 or 1) and the corresponding [`VoiceRate`].
    pub fn next_frame(&mut self) -> Option<VoiceFrame> {
        if self.position >= self.samples.len() {
            // Loop back to the beginning of the WAV file.  Keep the existing
            // encoder — its filter memories provide a smooth transition.
            // Re-creating the encoder would cause ~200ms of eighth-rate
            // "warmup" frames (audible silence gap).
            self.position = 0;
            info!("WavVoicePlayer: looping WAV playback");
        }

        // Extract up to 160 samples for this 20ms frame
        let end = std::cmp::min(self.position + SAMPLES_PER_FRAME, self.samples.len());
        let frame_samples = &self.samples[self.position..end];
        self.position = end;

        // Build a full 160-sample buffer, zero-padding if the last frame is short
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        pcm[..frame_samples.len()].copy_from_slice(frame_samples);

        Some(encode_pcm_frame(&mut self.encoder, &pcm))
    }

    /// Returns true if all samples have been consumed.
    pub fn is_exhausted(&self) -> bool {
        self.position >= self.samples.len()
    }

    /// Total duration of the WAV file in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        (self.samples.len() as f64 / 8.0) as u64
    }

    /// Number of remaining 20ms frames.
    pub fn remaining_frames(&self) -> usize {
        let remaining_samples = self.samples.len().saturating_sub(self.position);
        remaining_samples.div_ceil(SAMPLES_PER_FRAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_player(samples: Vec<i16>) -> WavVoicePlayer {
        WavVoicePlayer {
            samples,
            position: 0,
            encoder: VoiceEncoder::new(VoiceCodec::EvrcA).expect("encoder init"),
        }
    }

    #[test]
    fn test_voice_frame_bits_length() {
        let mut player = make_player(vec![0i16; 320]); // 2 frames worth

        let frame1 = player.next_frame().unwrap();
        assert_eq!(
            frame1.bits.len(),
            frame1.rate.primary_traffic_bits(),
            "bits length should match rate"
        );

        let frame2 = player.next_frame().unwrap();
        assert_eq!(frame2.bits.len(), frame2.rate.primary_traffic_bits(),);

        // After exhausting 2 frames, player should loop back and produce more
        let frame3 = player.next_frame().unwrap();
        assert_eq!(frame3.bits.len(), frame3.rate.primary_traffic_bits());
    }

    #[test]
    fn test_partial_last_frame() {
        let mut player = make_player(vec![0i16; 200]); // 1.25 frames

        let _frame1 = player.next_frame().unwrap();
        let frame2 = player.next_frame().unwrap(); // partial frame (40 samples, zero-padded)
        assert_eq!(frame2.bits.len(), frame2.rate.primary_traffic_bits());
        // Player loops — third call should produce a valid frame from the start
        let frame3 = player.next_frame().unwrap();
        assert_eq!(frame3.bits.len(), frame3.rate.primary_traffic_bits());
    }
}
