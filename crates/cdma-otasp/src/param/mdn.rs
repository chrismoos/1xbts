//! Mobile Directory Number parameter block — C.S0016-D §3.5.2.2.
//!
//! Wire layout:
//!   N_DIGITS (4)
//!   DIGIT_n (4) × N_DIGITS
//!   RESERVED (0 or 4)  — pads to whole octet
//!
//! DIGIT_n uses the C.S0005-E dialed-digit table (Table 2.7.1.3.2.4-4) —
//! note that decimal `0` encodes as `1010`, not `0000`.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_u8};
use crate::digit::{char_from_dialed_digit, dialed_digit_from_char};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDirectoryNumber {
    /// MDN digits as ASCII characters. May contain `'0'..='9'`, `'*'`, `'#'`.
    pub digits: String,
}

impl MobileDirectoryNumber {
    pub fn new(digits: impl Into<String>) -> Self {
        Self {
            digits: digits.into(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let n = self.digits.chars().count();
        if n > 15 {
            return Err(format!("MDN N_DIGITS too large: {}", n).into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(n as u8, 4);
        for c in self.digits.chars() {
            bs.write_u8(dialed_digit_from_char(c)?, 4);
        }
        // RESERVED: 0 if odd N_DIGITS (4 + 4*N is already a multiple of 8),
        // 4 bits if even N_DIGITS (4 + 4*N is 4 mod 8). Easier expressed as
        // "pad until byte boundary". `to_packed_bytes` pads on the right.
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        let n = read_u8(&mut bs, 4)? as usize;
        if n > 15 {
            return Err(format!("MDN N_DIGITS out of range: {}", n).into());
        }
        let mut digits = String::with_capacity(n);
        for _ in 0..n {
            let v = read_u8(&mut bs, 4)?;
            digits.push(char_from_dialed_digit(v)?);
        }
        Ok(Self { digits })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdn_ten_digits_no_zero_pad() {
        // 10 digits + 4-bit N_DIGITS = 44 bits → 6 bytes with 4 bits of
        // RESERVED pad at the end.
        let m = MobileDirectoryNumber::new("5551234567");
        let b = m.encode().unwrap();
        // 4-bit N_DIGITS=10 (0b1010), then digits: 5 5 5 1 2 3 4 5 6 7
        //   dialed: 5=0101, 1=0001, 2=0010, 3=0011, 4=0100, 6=0110, 7=0111
        // Stream: 1010 0101 0101 0101 0001 0010 0011 0100 0101 0110 0111 0000
        //         A    5    5    5    1    2    3    4    5    6    7    0
        assert_eq!(b, vec![0xA5, 0x55, 0x12, 0x34, 0x56, 0x70]);
    }

    #[test]
    fn mdn_round_trip_with_zero_digit() {
        let m = MobileDirectoryNumber::new("5550010001");
        let b = m.encode().unwrap();
        let back = MobileDirectoryNumber::decode(&b).unwrap();
        assert_eq!(back, m);
        // Zero is encoded as 1010, not 0000.
        // First two digit bits after N_DIGITS=10 (1010): digit '5'=0101.
        // Combined: byte 1 = 1010_0101 = 0xA5.
        assert_eq!(b[0], 0xA5);
    }

    #[test]
    fn mdn_zero_is_not_zero_bits() {
        // All-zero digits: N=1, DIGIT='0'=1010. 4+4=8 bits = 0xA0 then nothing.
        let m = MobileDirectoryNumber::new("0");
        let b = m.encode().unwrap();
        assert_eq!(b, vec![0b0001_1010]);
        assert_eq!(MobileDirectoryNumber::decode(&b).unwrap(), m);
    }

    #[test]
    fn mdn_odd_length_round_trips() {
        // 7-digit number — 4 + 7*4 = 32 bits, no padding.
        let m = MobileDirectoryNumber::new("1234567");
        let b = m.encode().unwrap();
        assert_eq!(b.len(), 4);
        assert_eq!(MobileDirectoryNumber::decode(&b).unwrap(), m);
    }

    #[test]
    fn mdn_even_length_padded_to_octet() {
        // 4 digits = 4 + 16 = 20 bits → 3 bytes (24 bits) with 4-bit pad.
        let m = MobileDirectoryNumber::new("1234");
        let b = m.encode().unwrap();
        assert_eq!(b.len(), 3);
        // N_DIGITS=4 (0100), then 0001 0010 0011 0100, pad 0000.
        // 0100_0001 0010_0011 0100_0000 = 0x41 0x23 0x40
        assert_eq!(b, vec![0x41, 0x23, 0x40]);
    }
}
