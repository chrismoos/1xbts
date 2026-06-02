//! Preferred Roaming List Parameter Block (segment) — C.S0016-D §3.5.3.2.
//!
//! Carried inside an SSPR Configuration Response when the BS asks for
//! `BLOCK_ID = 0x01`. The MS returns one chunk of the on-wire PRL bytes
//! per request; the BS reassembles the full PRL across multiple
//! Configuration Request rounds, walking `REQUEST_OFFSET` until the MS
//! sets `LAST_SEGMENT = 1`.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8, read_u16};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrlSegment {
    pub last_segment: bool,
    /// Byte offset within the PRL where this segment begins.
    pub segment_offset: u16,
    /// Bytes of the PRL contained in `segment_data`.
    pub segment_data: Vec<u8>,
}

impl PrlSegment {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.segment_data.len() > u8::MAX as usize {
            return Err("PRL segment SEGMENT_SIZE exceeds 8 bits".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(0, 7); // RESERVED
        bs.write_u8(self.last_segment as u8, 1);
        bs.write_u32(self.segment_offset as u32, 16);
        bs.write_u8(self.segment_data.len() as u8, 8);
        for &b in &self.segment_data {
            bs.write_u8(b, 8);
        }
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(param_data: &[u8]) -> Result<Self, Error> {
        if param_data.len() < 4 {
            return Err("PrlSegment too short".into());
        }
        let mut bs = from_bytes(param_data);
        let _reserved = read_u8(&mut bs, 7)?;
        let last_segment = read_bool(&mut bs)?;
        let segment_offset = read_u16(&mut bs, 16)?;
        let segment_size = read_u8(&mut bs, 8)? as usize;
        if param_data.len() < 4 + segment_size {
            return Err("PrlSegment SEGMENT_DATA truncated".into());
        }
        let mut segment_data = Vec::with_capacity(segment_size);
        for _ in 0..segment_size {
            segment_data.push(read_u8(&mut bs, 8)?);
        }
        Ok(Self {
            last_segment,
            segment_offset,
            segment_data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_round_trip() {
        let s = PrlSegment {
            last_segment: true,
            segment_offset: 0x1234,
            segment_data: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x55],
        };
        let bytes = s.encode().unwrap();
        assert_eq!(PrlSegment::decode(&bytes).unwrap(), s);
    }

    #[test]
    fn last_segment_bit_is_in_position() {
        let s = PrlSegment {
            last_segment: true,
            segment_offset: 0,
            segment_data: vec![],
        };
        let bytes = s.encode().unwrap();
        // RESERVED 7'b0 + LAST_SEGMENT=1 → first byte = 0x01.
        assert_eq!(bytes[0], 0x01);
    }
}
