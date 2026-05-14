//! Plays a pre-encoded subscriber ringtone on the forward traffic channel.
//!
//! Unlike [`WavVoicePlayer`](crate::wav_player::WavVoicePlayer), this player
//! does not re-encode PCM at runtime — it reads frames directly from a
//! serialized stream produced at upload time by
//! [`ringtone_preencode`](crate::ringtone_preencode).

use crate::encoded_stream::EncodedFrameReader;
use crate::{VoiceCodec, VoiceFrame};

pub struct EncodedRingtonePlayer {
    reader: EncodedFrameReader,
    codec: VoiceCodec,
}

impl EncodedRingtonePlayer {
    pub fn new(bytes: Vec<u8>, codec: VoiceCodec) -> Result<Self, String> {
        let reader = EncodedFrameReader::new(bytes)?;
        Ok(Self { reader, codec })
    }

    pub fn codec(&self) -> VoiceCodec {
        self.codec
    }

    pub fn frame_count(&self) -> usize {
        self.reader.frame_count()
    }

    /// Return the next 20 ms frame, looping at end-of-stream.
    pub fn next_frame(&mut self) -> VoiceFrame {
        self.reader.next_frame()
    }
}
