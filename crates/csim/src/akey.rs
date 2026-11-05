//! The 64-bit CAVE A-key in its two written forms.

/// The A-key from 16 hex digits (the raw key) or 20 decimal digits (the
/// handset keypad form, the key as an unsigned decimal number, S.S0053
/// §2.2.1). Spaces are ignored.
pub fn parse_a_key(text: &str) -> Result<u64, AKeyError> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let all_digits = clean.bytes().all(|b| b.is_ascii_digit());
    match clean.len() {
        HEX_DIGITS if clean.bytes().all(|b| b.is_ascii_hexdigit()) => {
            u64::from_str_radix(&clean, 16).map_err(|_| AKeyError::Format)
        }
        DECIMAL_DIGITS if all_digits => {
            clean.parse::<u64>().map_err(|_| AKeyError::TooLarge(clean))
        }
        _ => Err(AKeyError::Format),
    }
}

const HEX_DIGITS: usize = 16;
const DECIMAL_DIGITS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum AKeyError {
    #[error("a_key must be 16 hex digits or 20 decimal digits")]
    Format,
    #[error("a_key {0} is larger than 64 bits")]
    TooLarge(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_decimal_agree() {
        // S.S0053 §3: the digits 14142135623730950488 are the A-key C442F56BE9E17158.
        let key = 0xC442F56BE9E17158;
        assert_eq!(parse_a_key("C442F56BE9E17158").unwrap(), key);
        assert_eq!(parse_a_key("14142135623730950488").unwrap(), key);
        assert_eq!(parse_a_key("c442 f56b e9e1 7158").unwrap(), key);
    }

    #[test]
    fn rejects_bad_keys() {
        for bad in ["C442F56B", "12345", "notthehexdigits0"] {
            assert!(parse_a_key(bad).is_err(), "{bad}");
        }
        assert!(matches!(
            parse_a_key("99999999999999999999"),
            Err(AKeyError::TooLarge(_))
        ));
    }
}
