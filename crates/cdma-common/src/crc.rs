/// CRC-6: g(x) = x^6 + x^5 + x^2 + x + 1 (poly 0x27, init 0x3F).
/// Per C.S0002-E 3.1.3.1.4.1.5. Used for RC3 quarter/eighth rate.
pub fn crc6(data: &[u8]) -> u8 {
    let poly: u8 = 0x27;
    let mut register: u8 = 0x3F;
    for &bit in data {
        let feedback = ((register >> 5) & 1) ^ (bit & 1);
        register = (register << 1) & 0x3F;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

/// CRC-8: g(x) = x^8 + x^7 + x^4 + x^3 + x + 1 (poly 0x9B, init 0xFF).
/// Per C.S0002-E 3.1.3.1.4.1.4. Used for half-rate frames.
pub fn crc8(data: &[u8]) -> u8 {
    let poly: u8 = 0x9B;
    let mut register: u8 = 0xFF;
    for &bit in data {
        let feedback = ((register >> 7) & 1) ^ (bit & 1);
        register <<= 1;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

/// CRC-12: g(x) = x^12 + x^11 + x^10 + x^9 + x^8 + x^4 + x + 1 (poly 0x0F13, init 0x0FFF).
/// Per C.S0002-E 2.1.3.1.4.1.2. Used for full-rate frames.
pub fn crc12(data: &[u8]) -> u16 {
    let poly: u16 = 0x0F13;
    let mut register: u16 = 0x0FFF;
    for &bit in data {
        let feedback = ((register >> 11) & 1) ^ (bit as u16 & 1);
        register = (register << 1) & 0x0FFF;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

/// CRC-16 CCITT: g(x) = x^16 + x^12 + x^5 + 1 (poly 0x1021, init 0xFFFF, final XOR 0xFFFF).
/// Per C.S0004-E 2.2.1.3.1.2. Used for f-dsch/r-dsch signaling PDUs and dedicated channels.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let poly: u16 = 0x1021;
    let mut register: u16 = 0xFFFF;
    for &bit in data {
        let feedback = ((register >> 15) & 1) ^ (bit as u16 & 1);
        register <<= 1;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register ^ 0xFFFF
}

/// Air-interface CRC-16 FQI per C.S0002-E §2.1.3.1.4.1.1.
/// Polynomial 0xC867, init 0xFFFF, no final XOR.
pub fn crc16_sch(data: &[u8]) -> u16 {
    let poly: u16 = 0xC867;
    let mut register: u16 = 0xFFFF;
    for &bit in data {
        let feedback = ((register >> 15) & 1) ^ (bit as u16 & 1);
        register <<= 1;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First-bit check independent of the polynomial.
    #[test]
    fn crc16_sch_one_bit_input_is_polynomial_independent() {
        assert_eq!(crc16_sch(&[1u8]), 0xFFFE);
    }

    /// Pins the SCH polynomial tap constant.
    #[test]
    fn crc16_sch_zero_bit_input_pins_spec_polynomial() {
        assert_eq!(crc16_sch(&[0u8]), 0xFFFE ^ 0xC867);
        assert_eq!(crc16_sch(&[0u8]), 0x3799);
    }

    /// Empty-input edge case: register passes through untouched.
    #[test]
    fn crc16_sch_empty_input_returns_init() {
        assert_eq!(crc16_sch(&[]), 0xFFFF);
    }

    /// Pins the 360-bit all-zero F-SCH frame size.
    #[test]
    fn crc16_sch_full_360_zero_frame_pins_value() {
        let bits = vec![0u8; 360];
        let crc = crc16_sch(&bits);
        // If this constant ever changes, the polynomial or shift-register
        // logic was modified — cross-check with an external reference
        // before bumping the pin.
        const PINNED_360_ZERO_CRC: u16 = 0x42FB;
        assert_eq!(crc, PINNED_360_ZERO_CRC, "got 0x{:04X}", crc);
    }
}
