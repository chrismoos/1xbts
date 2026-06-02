//! Digit encodings used by OTASP parameter blocks.
//!
//! - Dialed-digit encoding (C.S0005-E Table 2.7.1.3.2.4-4): used by MDN. Note
//!   `0` maps to `1010`, not `0000`. Used by the Mobile Directory Number block.
//! - BCD encoding (C.S0016-D Table 4.5.4.1-1): used by Verify SPC / Change SPC.
//!   Straight 4-bit BCD with `0` -> `0000`.

use crate::Error;

/// Encode an ASCII digit (`'0'..='9'`, `'*'`, `'#'`) per C.S0005-E
/// Table 2.7.1.3.2.4-4 (Dialed Digit). Returns the 4-bit value.
pub fn dialed_digit_from_char(c: char) -> Result<u8, Error> {
    match c {
        '1' => Ok(0b0001),
        '2' => Ok(0b0010),
        '3' => Ok(0b0011),
        '4' => Ok(0b0100),
        '5' => Ok(0b0101),
        '6' => Ok(0b0110),
        '7' => Ok(0b0111),
        '8' => Ok(0b1000),
        '9' => Ok(0b1001),
        '0' => Ok(0b1010),
        '*' => Ok(0b1011),
        '#' => Ok(0b1100),
        _ => Err(format!("invalid dialed digit '{}'", c).into()),
    }
}

/// Inverse of [`dialed_digit_from_char`]. Returns `None` for the reserved
/// Pause / Wait / etc. codes (0xD..0xF).
pub fn char_from_dialed_digit(v: u8) -> Result<char, Error> {
    match v & 0xF {
        0b0001 => Ok('1'),
        0b0010 => Ok('2'),
        0b0011 => Ok('3'),
        0b0100 => Ok('4'),
        0b0101 => Ok('5'),
        0b0110 => Ok('6'),
        0b0111 => Ok('7'),
        0b1000 => Ok('8'),
        0b1001 => Ok('9'),
        0b1010 => Ok('0'),
        0b1011 => Ok('*'),
        0b1100 => Ok('#'),
        other => Err(format!("invalid dialed digit code {:#x}", other).into()),
    }
}

/// Encode an ASCII digit `'0'..='9'` as straight 4-bit BCD per C.S0016-D
/// Table 4.5.4.1-1. SPC encoding.
pub fn bcd_digit_from_char(c: char) -> Result<u8, Error> {
    if !c.is_ascii_digit() {
        return Err(format!("invalid BCD digit '{}'", c).into());
    }
    Ok((c as u8) - b'0')
}

/// Inverse of [`bcd_digit_from_char`].
pub fn char_from_bcd_digit(v: u8) -> Result<char, Error> {
    if v > 9 {
        return Err(format!("invalid BCD value {:#x}", v).into());
    }
    Ok((b'0' + v) as char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialed_zero_maps_to_1010_not_0000() {
        assert_eq!(dialed_digit_from_char('0').unwrap(), 0b1010);
        assert_eq!(char_from_dialed_digit(0b1010).unwrap(), '0');
    }

    #[test]
    fn dialed_one_through_nine_pass_through() {
        for d in 1..=9u8 {
            let c = char::from_digit(d as u32, 10).unwrap();
            assert_eq!(dialed_digit_from_char(c).unwrap(), d);
            assert_eq!(char_from_dialed_digit(d).unwrap(), c);
        }
    }

    #[test]
    fn dialed_star_and_pound() {
        assert_eq!(dialed_digit_from_char('*').unwrap(), 0b1011);
        assert_eq!(dialed_digit_from_char('#').unwrap(), 0b1100);
    }

    #[test]
    fn bcd_zero_is_0000_unlike_dialed() {
        assert_eq!(bcd_digit_from_char('0').unwrap(), 0b0000);
        assert_eq!(char_from_bcd_digit(0).unwrap(), '0');
    }
}
