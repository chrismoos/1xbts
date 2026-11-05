//! Bit-exact EF content encoders and decoders.
//!
//! Bit numbering in the specs: b1 is the least significant bit of a byte
//! and b8 the most significant.

mod identity;
mod packet_data;
mod tables;
mod text;

pub use identity::*;
pub use packet_data::*;
pub use tables::*;
pub use text::*;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CodecError(pub String);

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T, CodecError> {
    Err(CodecError(msg.into()))
}

/// Packs a digit string as nibble-swapped BCD (first digit in the low
/// nibble), padded with F to `length` bytes (TS 102 221 §13.2).
pub fn bcd_swap(digits: &str, length: usize) -> Result<Vec<u8>, CodecError> {
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return err(format!("non-digit in {digits:?}"));
    }
    if digits.len() > length * 2 {
        return err(format!("{digits:?} too long for {length} bytes"));
    }
    let nibbles: Vec<u8> = digits.bytes().map(|b| b - b'0').collect();
    let mut out: Vec<u8> = nibbles
        .chunks(2)
        .map(|pair| {
            let lo = pair[0];
            let hi = pair.get(1).copied().unwrap_or(0xF);
            (hi << 4) | lo
        })
        .collect();
    out.resize(length, 0xFF);
    Ok(out)
}

/// The inverse of [`bcd_swap`]: stops at the first F nibble.
pub fn bcd_swap_decode(data: &[u8]) -> String {
    let mut out = String::new();
    for b in data {
        for nibble in [b & 0x0F, b >> 4] {
            if nibble == 0xF {
                return out;
            }
            out.push(char::from(b'0' + nibble));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcd() {
        assert_eq!(bcd_swap("1234", 2).unwrap(), [0x21, 0x43]);
        assert_eq!(bcd_swap("123", 2).unwrap(), [0x21, 0xF3]);
        assert_eq!(bcd_swap_decode(&[0x21, 0xF3]), "123");
    }
}
