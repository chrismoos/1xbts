//! Verify SPC parameter block — C.S0016-D §4.5.4.1.
//!
//! Fixed-length 24-bit BCD-encoded 6-digit Service Programming Code.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::digit::{bcd_digit_from_char, char_from_bcd_digit};

/// Verify SPC block payload. Holds the 6 BCD digits as a `String` for
/// caller convenience; the wire form is always 24 bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifySpc {
    pub spc: String,
}

impl VerifySpc {
    pub fn new(spc: impl Into<String>) -> Self {
        Self { spc: spc.into() }
    }

    /// Encode to the 3-byte PARAM_DATA. SPC must be exactly 6 ASCII digits.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        encode_spc(&self.spc)
    }

    /// Decode from a 3-byte PARAM_DATA.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            spc: decode_spc(bytes)?,
        })
    }
}

pub(crate) fn encode_spc(spc: &str) -> Result<Vec<u8>, Error> {
    if spc.len() != 6 {
        return Err(format!("SPC must be 6 digits, got {}", spc.len()).into());
    }
    let mut bs = Bitstream::new();
    for c in spc.chars() {
        bs.write_u8(bcd_digit_from_char(c)?, 4);
    }
    Ok(bs.to_packed_bytes())
}

pub(crate) fn decode_spc(bytes: &[u8]) -> Result<String, Error> {
    if bytes.len() != 3 {
        return Err(format!("SPC PARAM_DATA must be 3 bytes, got {}", bytes.len()).into());
    }
    let mut bs = Bitstream::new_bytes(bytes);
    let mut out = String::with_capacity(6);
    for _ in 0..6 {
        let nibble = bs.read_bits(4)? as u8;
        out.push(char_from_bcd_digit(nibble)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spc_000000_encodes_to_three_zero_bytes() {
        let v = VerifySpc::new("000000");
        assert_eq!(v.encode().unwrap(), vec![0x00, 0x00, 0x00]);
    }

    #[test]
    fn spc_123456_encodes_to_bcd_bytes() {
        let v = VerifySpc::new("123456");
        assert_eq!(v.encode().unwrap(), vec![0x12, 0x34, 0x56]);
    }

    #[test]
    fn spc_round_trip() {
        let v = VerifySpc::new("987654");
        let bytes = v.encode().unwrap();
        assert_eq!(VerifySpc::decode(&bytes).unwrap(), v);
    }

    #[test]
    fn spc_wrong_length_errors() {
        assert!(VerifySpc::new("12345").encode().is_err());
        assert!(VerifySpc::decode(&[0u8; 2]).is_err());
    }
}
