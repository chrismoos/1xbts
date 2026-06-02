//! Home System Tag parameter block — `BLOCK_ID = 0x00`.
//!
//! The Configuration Response (§3.5.10.1) and Download Request
//! (§4.5.9.1) layouts differ:
//! - Response: CALL_PRMPT_INCL(1) + CALL_PRMPT(0 or 5) +
//!   RESERVED(0 or 5) + TAG_ENCODING(5) + TAG_LEN(5) + TAG
//! - Download: RESERVED(6) + TAG_ENCODING(5) + TAG_LEN(5) + TAG
//!   (no CALL_PRMPT — that's a response-only field)
//!
//! `decode` parses the Response. `encode` emits a Download Request,
//! ignoring `call_prompt`.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8};

/// `TAG_ENCODING` values per [4] (C.R1001 — Administration of Parameter Value
/// Assignments for cdma2000 Spread Spectrum Standards). The full table is
/// large; we expose the two commonly relevant codings and a raw passthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagEncoding {
    /// 7-bit ASCII (also called Latin / extended-protocol-message string
    /// encoding). Value `0b00010` per C.R1001.
    Latin = 0b00010,
    /// UTF-16BE Unicode. Value `0b00100` per C.R1001.
    Unicode = 0b00100,
}

impl TagEncoding {
    pub fn raw(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSystemTag {
    /// `CALL_PRMPT`. `Some(id)` sets `CALL_PRMPT_INCL = 1`.
    pub call_prompt: Option<u8>,
    /// `TAG_ENCODING`. Stored raw (5 bits) so unknown values can pass through.
    pub tag_encoding: u8,
    /// Tag bytes. Length must fit in 5 bits (0..=31).
    pub tag: Vec<u8>,
}

impl HomeSystemTag {
    pub fn new_ascii(name: &str) -> Result<Self, Error> {
        if !name.is_ascii() {
            return Err("Home System Tag (Latin encoding) requires ASCII".into());
        }
        if name.len() > 31 {
            return Err(format!("tag too long: {} bytes (max 31)", name.len()).into());
        }
        Ok(Self {
            call_prompt: None,
            tag_encoding: TagEncoding::Latin.raw(),
            tag: name.as_bytes().to_vec(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.tag_encoding >= 1 << 5 {
            return Err("tag_encoding >= 32".into());
        }
        if self.tag.len() > 31 {
            return Err(format!("tag too long: {} bytes (max 31)", self.tag.len()).into());
        }

        let mut bs = Bitstream::new();
        bs.write_u8(0, 6); // RESERVED
        bs.write_u8(self.tag_encoding, 5);
        bs.write_u8(self.tag.len() as u8, 5);
        for &b in &self.tag {
            bs.write_u8(b, 8);
        }
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        let incl = read_bool(&mut bs)?;
        let call_prompt = if incl {
            Some(read_u8(&mut bs, 5)?)
        } else {
            let _ = read_u8(&mut bs, 5)?; // RESERVED
            None
        };
        let tag_encoding = read_u8(&mut bs, 5)?;
        let tag_len = read_u8(&mut bs, 5)? as usize;
        let mut tag = Vec::with_capacity(tag_len);
        for _ in 0..tag_len {
            tag.push(read_u8(&mut bs, 8)?);
        }
        Ok(Self {
            call_prompt,
            tag_encoding,
            tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_tag_download_byte_layout_pin() {
        // Download (§4.5.9.1): RESERVED(6) + TAG_ENCODING(00010) +
        // TAG_LEN(00101) = 000000_00010_00101 → 0000_0000_0100_0101 = 0x00 0x45.
        let v = HomeSystemTag::new_ascii("1xBTS").unwrap();
        let bytes = v.encode().unwrap();
        assert_eq!(&bytes[..2], &[0x00, 0x45]);
        assert_eq!(&bytes[2..], b"1xBTS");
    }

    #[test]
    fn encode_ignores_call_prompt() {
        // call_prompt is response-only; encode must produce identical
        // Download bytes regardless of its value.
        let a = HomeSystemTag {
            call_prompt: None,
            tag_encoding: TagEncoding::Latin.raw(),
            tag: b"X".to_vec(),
        };
        let b = HomeSystemTag {
            call_prompt: Some(7),
            tag_encoding: TagEncoding::Latin.raw(),
            tag: b"X".to_vec(),
        };
        assert_eq!(a.encode().unwrap(), b.encode().unwrap());
    }

    #[test]
    fn tag_too_long_errors() {
        let v = HomeSystemTag {
            call_prompt: None,
            tag_encoding: 2,
            tag: vec![0; 32],
        };
        assert!(v.encode().is_err());
    }
}
