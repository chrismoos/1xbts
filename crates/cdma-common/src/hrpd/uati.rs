//! HRPD UATI identity helpers.
//!
//! C.S0024-0 §5.3 Address Management assigns a 128-bit UATI. The AT then uses
//! a 32-bit UATI ATI on the air interface, formed as
//! `UATIColorCode | UATI[23:0]`. Keep those two concepts distinct: the full
//! value is the session/routing identity, while the ATI form is a derived radio
//! address.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HrpdUatiParseError {
    #[error("HRPD UATI must contain 32 hexadecimal digits")]
    InvalidLength,
    #[error("HRPD UATI contains non-hexadecimal characters")]
    InvalidHex,
}

/// Canonical HRPD UATI plus the addressing metadata needed to derive its
/// on-air ATI form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HrpdUati {
    value: [u8; 16],
    color_code: u8,
    subnet_mask: u8,
}

impl HrpdUati {
    pub const fn new(value: [u8; 16], color_code: u8, subnet_mask: u8) -> Self {
        Self {
            value,
            color_code,
            subnet_mask,
        }
    }

    pub fn from_parts(uati104: [u8; 13], uati024: u32, color_code: u8, subnet_mask: u8) -> Self {
        let mut value = [0u8; 16];
        value[..13].copy_from_slice(&uati104);
        let low = (uati024 & 0x00ff_ffff).to_be_bytes();
        value[13..].copy_from_slice(&low[1..]);
        Self::new(value, color_code, subnet_mask)
    }

    pub fn from_uati032(uati032: u32, color_code: u8, subnet_mask: u8) -> Self {
        let mut value = [0u8; 16];
        value[12..].copy_from_slice(&uati032.to_be_bytes());
        Self::new(value, color_code, subnet_mask)
    }

    pub const fn value(self) -> [u8; 16] {
        self.value
    }

    pub const fn color_code(self) -> u8 {
        self.color_code
    }

    pub const fn subnet_mask(self) -> u8 {
        self.subnet_mask
    }

    pub fn uati024(self) -> u32 {
        u32::from_be_bytes([0, self.value[13], self.value[14], self.value[15]])
    }

    /// Lower 32 bits of the full UATI. This is a compact local/session view,
    /// not the color-coded UATI ATI used in MAC headers.
    pub fn uati032(self) -> u32 {
        u32::from_be_bytes([
            self.value[12],
            self.value[13],
            self.value[14],
            self.value[15],
        ])
    }

    /// ATI value used when HRPD Address Management sets
    /// `TransmitATI/ReceiveATIList` to `UATIColorCode | UATI[23:0]`.
    pub fn receive_ati_u32(self) -> u32 {
        (u32::from(self.color_code) << 24) | self.uati024()
    }

    /// Current A8/A10 GRE bearer key. A* specs define this as a 4-octet key,
    /// so it deliberately remains a derived 32-bit value.
    pub fn a8_gre_key_u32(self) -> u32 {
        self.receive_ati_u32()
    }

    pub fn colon_hex(self) -> String {
        self.value
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

impl fmt::Display for HrpdUati {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.colon_hex())
    }
}

impl FromStr for HrpdUati {
    type Err = HrpdUatiParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let hex = input
            .trim()
            .strip_prefix("0x")
            .or_else(|| input.trim().strip_prefix("0X"))
            .unwrap_or_else(|| input.trim())
            .replace(':', "");
        if hex.len() != 32 {
            return Err(HrpdUatiParseError::InvalidLength);
        }
        let mut value = [0u8; 16];
        for (idx, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
            let part = std::str::from_utf8(chunk).map_err(|_| HrpdUatiParseError::InvalidHex)?;
            value[idx] =
                u8::from_str_radix(part, 16).map_err(|_| HrpdUatiParseError::InvalidHex)?;
        }
        Ok(Self::new(value, 0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_from_assignment_parts() {
        let u = HrpdUati::from_parts([0x11; 13], 0x05_8001, 0x1a, 26);
        assert_eq!(u.uati024(), 0x05_8001);
        assert_eq!(u.uati032(), 0x1105_8001);
        assert_eq!(u.receive_ati_u32(), 0x1a05_8001);
        assert_eq!(u.a8_gre_key_u32(), 0x1a05_8001);
    }

    #[test]
    fn formats_and_parses_colon_hex() {
        let u = HrpdUati::from_uati032(0x8005_8001, 0x1a, 26);
        assert_eq!(
            u.colon_hex(),
            "00:00:00:00:00:00:00:00:00:00:00:00:80:05:80:01"
        );
        let parsed = u.colon_hex().parse::<HrpdUati>().unwrap();
        assert_eq!(parsed.value(), u.value());
    }
}
