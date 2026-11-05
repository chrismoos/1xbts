//! Personalization and inspection of CSIM cards (C.S0065-B), and the host
//! side of the 1xBTS CSIM applet.
//!
//! The library holds everything that does not touch a reader: the EF
//! table, the EF codecs, profiles, FCP decoding, dumps and comparisons,
//! and the [`card::Card`] command layer over a [`card::Transport`]. The
//! PC/SC transport and the `csim` binary need the `csim` feature.

pub mod akey;
pub mod card;
pub mod codec;
pub mod compare;
pub mod dump;
pub mod ef_dir;
pub mod ef_table;
pub mod fcp;
pub mod personalize;
pub mod prl;
pub mod profile;
pub mod read;
pub mod sim;
pub mod template;

use std::fmt;

/// A file identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fid(pub u16);

impl Fid {
    pub const MF: Fid = Fid(0x3F00);
    pub const ADF: Fid = Fid(0x7FFF);
    pub const EF_DIR: Fid = Fid(0x2F00);
    pub const EF_ARR_MF: Fid = Fid(0x2F06);
    pub const EF_ARR_ADF: Fid = Fid(0x6F06);

    // FID high bytes (TS 102 221 §8.2): 2F for EFs under the MF, 6F for
    // EFs under an ADF, 5F and 7F for DFs.
    const HI_MF_EF: u8 = 0x2F;
    const HI_ADF_EF: u8 = 0x6F;
    const HI_DF_LEVEL2: u8 = 0x5F;
    const HI_DF_LEVEL1: u8 = 0x7F;

    pub fn is_mf_ef(self) -> bool {
        self.hi() == Fid::HI_MF_EF
    }

    pub fn is_adf_ef(self) -> bool {
        self.hi() == Fid::HI_ADF_EF
    }

    pub fn is_df(self) -> bool {
        self.hi() == Fid::HI_DF_LEVEL1 || self.hi() == Fid::HI_DF_LEVEL2
    }

    /// The FID `lo` among the EFs under the MF (`2Fxx`).
    pub fn mf_ef(lo: u8) -> Fid {
        Fid(u16::from_be_bytes([Fid::HI_MF_EF, lo]))
    }

    /// The FID `lo` among the EFs under an ADF (`6Fxx`).
    pub fn adf_ef(lo: u8) -> Fid {
        Fid(u16::from_be_bytes([Fid::HI_ADF_EF, lo]))
    }

    pub fn hi(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub fn lo(self) -> u8 {
        self.0 as u8
    }

    /// The EF table name of this FID, if it has one.
    pub fn label(self) -> Option<&'static str> {
        ef_table::by_fid(self).map(|e| e.name.as_str())
    }

    /// The EF table name, or `EF_xxxx` for a FID outside the table.
    pub fn display_label(self) -> String {
        self.label()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("EF_{:04X}", self.0))
    }
}

impl fmt::Display for Fid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}", self.0)
    }
}

/// The instance AID of the 1xBTS CSIM applet.
pub const CSIM_INSTANCE_AID: [u8; 16] = [
    0xA0, 0x00, 0x00, 0x03, 0x43, 0x10, 0x02, 0xF3, 0x10, 0xFF, 0xFF, 0x89, 0x02, 0x00, 0x00, 0xFF,
];

/// RID A000000343 and PIX 1002, the prefix of any CSIM AID (C.S0065-B §5.1).
pub const CSIM_AID_PREFIX: [u8; 7] = [0xA0, 0x00, 0x00, 0x03, 0x43, 0x10, 0x02];

/// Uppercase hex with no separators, the form dumps store.
pub fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Uppercase hex with a space between bytes, the form reports print.
pub fn hex_spaced(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, thiserror::Error)]
#[error("{text:?} is not hex")]
pub struct HexError {
    text: String,
}

/// Parses hex, ignoring spaces.
pub fn parse_hex(text: &str) -> Result<Vec<u8>, HexError> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bad = || HexError {
        text: text.to_owned(),
    };
    if clean.len() % 2 != 0 {
        return Err(bad());
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|_| bad()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(parse_hex("00 11 22").unwrap(), [0x00, 0x11, 0x22]);
        assert_eq!(parse_hex("001122").unwrap(), [0x00, 0x11, 0x22]);
        assert!(parse_hex("0").is_err());
        assert!(parse_hex("zz").is_err());
        assert_eq!(hex_upper(&[0xAB, 0x01]), "AB01");
        assert_eq!(hex_spaced(&[0xAB, 0x01]), "AB 01");
    }

    #[test]
    fn fid_labels() {
        assert_eq!(Fid(0x6F22).label(), Some("IMSI_M"));
        assert_eq!(Fid(0x6FAA).display_label(), "EF_6FAA");
    }
}
