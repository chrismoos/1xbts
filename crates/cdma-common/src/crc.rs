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

/// CRC-16 for supplemental channels: g(x) = x^16 + x^15 + x^13 + ... (poly 0xBE07, init 0xFFFF).
/// Per C.S0002-E 2.1.3.1.4.1.1. Used for SCH frames (≥360 bits).
pub fn crc16_sch(data: &[u8]) -> u16 {
    let poly: u16 = 0xBE07;
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
