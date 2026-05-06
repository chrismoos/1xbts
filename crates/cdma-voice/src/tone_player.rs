//! Synthetic tone generators for forward traffic-channel media.

use crate::{SAMPLES_PER_FRAME, VoiceCodec, VoiceEncoder, VoiceFrame, encode_pcm_frame};
use std::f32::consts::PI;

const SAMPLE_RATE_HZ: f32 = 8000.0;
const RINGBACK_AMPLITUDE: f32 = 7000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingbackToneKind {
    Nanp,
    Etsi,
}

impl RingbackToneKind {
    fn profile(self) -> RingbackToneProfile {
        match self {
            RingbackToneKind::Nanp => RingbackToneProfile {
                freq_a_hz: 440.0,
                freq_b_hz: Some(480.0),
                on_frames: 100,  // 2.0s at 20ms/frame
                off_frames: 200, // 4.0s at 20ms/frame
            },
            RingbackToneKind::Etsi => RingbackToneProfile {
                freq_a_hz: 425.0,
                freq_b_hz: None,
                on_frames: 50,   // 1.0s at 20ms/frame
                off_frames: 200, // 4.0s at 20ms/frame
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RingbackToneProfile {
    freq_a_hz: f32,
    freq_b_hz: Option<f32>,
    on_frames: u64,
    off_frames: u64,
}

/// Generates a synthesized ringback tone and EVRC-encodes it frame by frame
/// for forward traffic-channel playback.
pub struct RingbackTonePlayer {
    kind: RingbackToneKind,
    frame_index: u64,
    phase_a: f32,
    phase_b: f32,
    encoder: VoiceEncoder,
}

impl RingbackTonePlayer {
    pub fn new(kind: RingbackToneKind) -> Result<Self, String> {
        Self::new_with_codec(kind, VoiceCodec::EvrcA)
    }

    pub fn new_with_codec(kind: RingbackToneKind, codec: VoiceCodec) -> Result<Self, String> {
        Ok(Self {
            kind,
            frame_index: 0,
            phase_a: 0.0,
            phase_b: 0.0,
            encoder: VoiceEncoder::new(codec)?,
        })
    }

    pub fn next_frame(&mut self) -> VoiceFrame {
        let pcm = self.synthesize_pcm_frame();
        self.frame_index += 1;
        encode_pcm_frame(&mut self.encoder, &pcm)
    }

    fn synthesize_pcm_frame(&mut self) -> [i16; SAMPLES_PER_FRAME] {
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        let profile = self.kind.profile();
        let cycle_frames = profile.on_frames + profile.off_frames;
        let in_tone_window = self.frame_index % cycle_frames < profile.on_frames;
        if !in_tone_window {
            return pcm;
        }

        if self.frame_index.is_multiple_of(cycle_frames) {
            self.phase_a = 0.0;
            self.phase_b = 0.0;
        }

        let step_a = 2.0 * PI * profile.freq_a_hz / SAMPLE_RATE_HZ;
        let step_b = profile
            .freq_b_hz
            .map(|freq| 2.0 * PI * freq / SAMPLE_RATE_HZ)
            .unwrap_or(0.0);

        for sample in &mut pcm {
            let mixed = if profile.freq_b_hz.is_some() {
                (self.phase_a.sin() + self.phase_b.sin()) * RINGBACK_AMPLITUDE
            } else {
                self.phase_a.sin() * RINGBACK_AMPLITUDE
            };
            *sample = mixed.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            self.phase_a = (self.phase_a + step_a) % (2.0 * PI);
            if profile.freq_b_hz.is_some() {
                self.phase_b = (self.phase_b + step_b) % (2.0 * PI);
            }
        }

        pcm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn na_ringback_starts_tone_on() {
        let mut player = RingbackTonePlayer::new(RingbackToneKind::Nanp).expect("player init");
        let pcm = player.synthesize_pcm_frame();
        assert!(pcm.iter().any(|&s| s != 0));
    }

    #[test]
    fn na_ringback_goes_silent_after_two_seconds() {
        let profile = RingbackToneKind::Nanp.profile();
        let mut player = RingbackTonePlayer::new(RingbackToneKind::Nanp).expect("player init");
        for _ in 0..profile.on_frames {
            let _ = player.synthesize_pcm_frame();
            player.frame_index += 1;
        }
        let pcm = player.synthesize_pcm_frame();
        assert!(pcm.iter().all(|&s| s == 0));
    }

    #[test]
    fn na_ringback_returns_after_full_cycle() {
        let profile = RingbackToneKind::Nanp.profile();
        let mut player = RingbackTonePlayer::new(RingbackToneKind::Nanp).expect("player init");
        player.frame_index = profile.on_frames + profile.off_frames;
        let pcm = player.synthesize_pcm_frame();
        assert!(pcm.iter().any(|&s| s != 0));
    }

    #[test]
    fn european_ringback_goes_silent_after_one_second() {
        let profile = RingbackToneKind::Etsi.profile();
        let mut player = RingbackTonePlayer::new(RingbackToneKind::Etsi).expect("player init");
        for _ in 0..profile.on_frames {
            let _ = player.synthesize_pcm_frame();
            player.frame_index += 1;
        }
        let pcm = player.synthesize_pcm_frame();
        assert!(pcm.iter().all(|&s| s == 0));
    }
}
