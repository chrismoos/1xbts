//! Framing helper for Abis signaling carried over a byte stream.
//!
//! The architecture plan uses a simple transport envelope for TCP-carried
//! signaling: a fixed `0xF634` flag followed by a 16-bit big-endian payload
//! length and the raw Abis payload bytes.

use crate::{Error, Result};

/// Fixed 16-bit flag used at the start of each framed signaling payload.
pub const FRAME_FLAG: u16 = 0xf634;

/// Fixed signaling frame header length in octets.
pub const HEADER_LEN: usize = 4;

/// Exact framed signaling payload used for TCP carriage of Abis control bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalingFrame {
    /// Raw Abis control payload bytes carried inside the frame.
    pub payload: Vec<u8>,
}

impl SignalingFrame {
    /// Creates a framed signaling payload from raw Abis control bytes.
    pub fn new(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            payload: payload.into(),
        }
    }

    /// Encodes the frame as `flag | 16-bit length | payload`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.payload.len() > u16::MAX as usize {
            return Err(Error::InvalidLength {
                context: "Abis TCP signaling payload",
                expected: u16::MAX as usize,
                actual: self.payload.len(),
            });
        }
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&FRAME_FLAG.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Decodes a complete frame from exact input bytes.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < HEADER_LEN {
            return Err(Error::Truncated {
                context: "Abis TCP signaling header",
                needed: HEADER_LEN,
                actual: input.len(),
            });
        }

        let flag = u16::from_be_bytes([input[0], input[1]]);
        if flag != FRAME_FLAG {
            return Err(Error::InvalidValue {
                context: "Abis TCP signaling flag",
                reason: "expected 0xf634",
            });
        }

        let payload_len = u16::from_be_bytes([input[2], input[3]]) as usize;
        let expected_len = HEADER_LEN + payload_len;
        if input.len() < expected_len {
            return Err(Error::Truncated {
                context: "Abis TCP signaling payload",
                needed: expected_len,
                actual: input.len(),
            });
        }
        if input.len() != expected_len {
            return Err(Error::InvalidLength {
                context: "Abis TCP signaling frame",
                expected: expected_len,
                actual: input.len(),
            });
        }

        Ok(Self {
            payload: input[HEADER_LEN..].to_vec(),
        })
    }

    /// Decodes the leading frame from a larger byte slice and returns bytes consumed.
    pub fn decode_prefix(input: &[u8]) -> Result<(Self, usize)> {
        if input.len() < HEADER_LEN {
            return Err(Error::Truncated {
                context: "Abis TCP signaling header",
                needed: HEADER_LEN,
                actual: input.len(),
            });
        }

        let flag = u16::from_be_bytes([input[0], input[1]]);
        if flag != FRAME_FLAG {
            return Err(Error::InvalidValue {
                context: "Abis TCP signaling flag",
                reason: "expected 0xf634",
            });
        }

        let payload_len = u16::from_be_bytes([input[2], input[3]]) as usize;
        let frame_len = HEADER_LEN + payload_len;
        if input.len() < frame_len {
            return Err(Error::Truncated {
                context: "Abis TCP signaling payload",
                needed: frame_len,
                actual: input.len(),
            });
        }

        Ok((
            Self {
                payload: input[HEADER_LEN..frame_len].to_vec(),
            },
            frame_len,
        ))
    }
}

/// Incremental stream decoder for Abis signaling frames over TCP.
///
/// The decoder keeps a byte buffer, discards unsynchronized bytes until the
/// fixed `0xF634` flag is found, and emits fully reassembled frames when enough
/// bytes are present. It does not perform socket I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignalingFrameStreamDecoder {
    buffer: Vec<u8>,
}

impl SignalingFrameStreamDecoder {
    /// Creates an empty frame-stream decoder.
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Appends raw stream bytes to the internal buffer.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns the number of buffered bytes still awaiting frame completion.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// Attempts to extract the next framed signaling payload.
    ///
    /// Leading bytes that do not begin with the framing flag are discarded so
    /// the decoder can recover synchronization after stream corruption.
    pub fn next_frame(&mut self) -> Result<Option<SignalingFrame>> {
        self.resynchronize();
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }
        let payload_len = u16::from_be_bytes([self.buffer[2], self.buffer[3]]) as usize;
        let frame_len = HEADER_LEN + payload_len;
        if self.buffer.len() < frame_len {
            return Ok(None);
        }
        let frame = SignalingFrame {
            payload: self.buffer[HEADER_LEN..frame_len].to_vec(),
        };
        self.buffer.drain(..frame_len);
        Ok(Some(frame))
    }

    fn resynchronize(&mut self) {
        let mut offset = 0usize;
        while offset + 1 < self.buffer.len() {
            if self.buffer[offset] == FRAME_FLAG.to_be_bytes()[0]
                && self.buffer[offset + 1] == FRAME_FLAG.to_be_bytes()[1]
            {
                break;
            }
            offset += 1;
        }
        if offset > 0 {
            self.buffer.drain(..offset);
        }
        if self.buffer.len() == 1 && self.buffer[0] != FRAME_FLAG.to_be_bytes()[0] {
            self.buffer.clear();
        }
    }
}
