//! HRPD physical-layer FCS helpers.
//!
//! The HRPD forward Control/Data PHY packets and reverse Access PHY packets
//! use the same 16-bit generator polynomial (`x^16 + x^12 + x^5 + 1`) with a
//! zero initial state, processed MSB-first.

/// Compute the direct CRC-CCITT register value over MSB-first bits.
pub fn physical_crc16(bits: &[u8]) -> u16 {
    let poly = 0x1021u16;
    let mut reg = 0u16;
    for &bit in bits {
        let feedback = ((reg >> 15) & 1) ^ u16::from(bit & 1);
        reg <<= 1;
        if feedback != 0 {
            reg ^= poly;
        }
    }
    reg
}
