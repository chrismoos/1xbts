//! UATI (Unicast Access Terminal Identifier).
//!
//! C.S0024-400 §8 Address Management Protocol: the UATI is a 128-bit identifier
//! composed of a 104-bit subnet portion and a 24-bit UATI024 portion. For
//! intra-subnet air-interface addressing, the AT uses the derived UATI ATI
//! `UATIColorCode | UATI[23:0]`.

use cdma_common::hrpd::uati::HrpdUati;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Canonical 128-bit UATI assigned to one HRPD session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Uati {
    compact: u32,
    full: HrpdUati,
}

impl Uati {
    /// Build a UATI from the spec-defined 24-bit `UATI024` host value.
    pub fn from_uati024(uati024: u32, uati104: [u8; 13], color_code: u8, subnet_mask: u8) -> Self {
        let compact = uati024 & 0x00ff_ffff;
        Uati {
            compact,
            full: HrpdUati::from_parts(uati104, compact, color_code, subnet_mask),
        }
    }

    pub fn from_compact(compact: u32, uati104: [u8; 13], color_code: u8, subnet_mask: u8) -> Self {
        Self::from_uati024(compact, uati104, color_code, subnet_mask)
    }

    /// Returns the allocator-local compact UATI032 value used by legacy code
    /// paths in this crate. This may differ from `full().uati032()` when the
    /// explicit UATI104 assignment is not derived from the allocator prefix.
    pub fn as_u32(self) -> u32 {
        self.compact
    }

    pub fn full(self) -> HrpdUati {
        self.full
    }

    pub fn receive_ati_u32(self) -> u32 {
        self.full.receive_ati_u32()
    }
}

impl fmt::Display for Uati {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UATI({})", self.full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_hex() {
        let u = Uati::from_compact(0xDEAD_BEEF, [0; 13], 0x1a, 26);
        assert_eq!(
            format!("{}", u),
            "UATI(00:00:00:00:00:00:00:00:00:00:00:00:00:AD:BE:EF)"
        );
    }

    #[test]
    fn from_uati024_masks_to_24_bits() {
        let u = Uati::from_uati024(0x12_3456_78, [0; 13], 0x1a, 26);
        assert_eq!(u.as_u32(), 0x0034_5678);
        assert_eq!(u.full().uati024(), 0x34_5678);
    }
}
