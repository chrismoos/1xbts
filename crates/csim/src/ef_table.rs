//! The applet's EF table, parsed from `sim/ef-table.yaml`.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::Fid;

const TABLE_YAML: &str = include_str!("../../../sim/ef-table.yaml");

/// Bytes per INIT_META entry.
pub const ENTRY_LEN: usize = 8;

/// File structure, with the FileSystem code the applet stores
/// (TS 102 221 §11.1.1.4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Structure {
    Transparent = 1,
    LinearFixed = 2,
    Cyclic = 6,
}

impl Structure {
    /// The name `csim dump` writes for this structure.
    pub fn name(self) -> &'static str {
        match self {
            Structure::Transparent => "transparent",
            Structure::LinearFixed => "linear-fixed",
            Structure::Cyclic => "cyclic",
        }
    }
}

/// An access condition in the table, with the nibble code of INIT_META.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Access {
    #[serde(rename = "ALW")]
    Always = 0,
    #[serde(rename = "PIN1")]
    Pin1 = 1,
    #[serde(rename = "ADM")]
    Adm = 2,
    #[serde(rename = "NEV")]
    Never = 3,
    #[serde(rename = "PIN2")]
    Pin2 = 4,
}

impl Access {
    /// The condition name an FCP decode produces for this condition.
    pub fn name(self) -> &'static str {
        match self {
            Access::Always => "ALW",
            Access::Pin1 => "PIN1",
            Access::Adm => "ADM1",
            Access::Never => "NEV",
            Access::Pin2 => "PIN2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EfEntry {
    pub fid: Fid,
    pub structure: Structure,
    pub sfi: u8,
    pub size: u16,
    pub record_length: u8,
    pub read: Access,
    pub update: Access,
    pub name: String,
}

impl EfEntry {
    pub fn num_records(&self) -> u16 {
        if self.record_length == 0 {
            0
        } else {
            self.size / u16::from(self.record_length)
        }
    }

    fn to_bytes(&self) -> [u8; ENTRY_LEN] {
        let [fid_hi, fid_lo] = self.fid.0.to_be_bytes();
        let [size_hi, size_lo] = self.size.to_be_bytes();
        [
            fid_hi,
            fid_lo,
            self.structure as u8,
            self.sfi,
            size_hi,
            size_lo,
            self.record_length,
            ((self.read as u8) << 4) | self.update as u8,
        ]
    }
}

/// Every EF, in table order.
pub fn entries() -> &'static [EfEntry] {
    static ENTRIES: OnceLock<Vec<EfEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| parse(TABLE_YAML).expect("sim/ef-table.yaml is valid"))
}

/// The INIT_META image of the whole table.
pub fn meta_bytes() -> Vec<u8> {
    entries().iter().flat_map(EfEntry::to_bytes).collect()
}

pub fn by_name(name: &str) -> Option<&'static EfEntry> {
    entries().iter().find(|e| e.name == name)
}

pub fn by_fid(fid: Fid) -> Option<&'static EfEntry> {
    entries().iter().find(|e| e.fid == fid)
}

/// The size of a named EF. Panics on a name that is not in the table,
/// which is a programming error.
pub fn size_of(name: &str) -> usize {
    usize::from(
        by_name(name)
            .unwrap_or_else(|| panic!("{name} is not in the EF table"))
            .size,
    )
}

/// One entry as `sim/ef-table.yaml` writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    name: String,
    fid: String,
    structure: Structure,
    sfi: u8,
    size: u16,
    record_length: Option<u8>,
    read: Access,
    update: Access,
    #[serde(rename = "note")]
    _note: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TableError {
    #[error("ef-table.yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("ef-table.yaml: {0}")]
    Invalid(String),
}

fn parse(text: &str) -> Result<Vec<EfEntry>, TableError> {
    let rows: Vec<Row> = serde_yaml::from_str(text)?;
    let entries = rows
        .into_iter()
        .map(|r| {
            let fid = u16::from_str_radix(&r.fid, 16)
                .map_err(|e| TableError::Invalid(format!("{}: FID {}: {e}", r.name, r.fid)))?;
            Ok(EfEntry {
                fid: Fid(fid),
                structure: r.structure,
                sfi: r.sfi,
                size: r.size,
                record_length: r.record_length.unwrap_or(0),
                read: r.read,
                update: r.update,
                name: r.name,
            })
        })
        .collect::<Result<Vec<_>, TableError>>()?;
    validate(&entries).map_err(TableError::Invalid)?;
    Ok(entries)
}

/// The applet stores sizes in 15 bits.
const MAX_EF_SIZE: u16 = 0x7FFF;

fn validate(entries: &[EfEntry]) -> Result<(), String> {
    for (i, e) in entries.iter().enumerate() {
        if entries[..i]
            .iter()
            .any(|o| o.fid == e.fid || o.name == e.name)
        {
            return Err(format!("duplicate FID or name {}", e.name));
        }
        if e.size > MAX_EF_SIZE {
            return Err(format!("{} larger than {MAX_EF_SIZE} bytes", e.name));
        }
        let records_ok = match e.structure {
            Structure::Transparent => e.record_length == 0,
            _ => e.record_length > 0 && e.size % u16::from(e.record_length) == 0,
        };
        if !records_ok {
            return Err(format!("{} record length does not divide its size", e.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_every_ef() {
        assert_eq!(entries().len(), 104);
        assert_eq!(meta_bytes().len(), 104 * ENTRY_LEN);
    }

    #[test]
    fn entry_encoding() {
        // IMSI_M: FID 6F22, transparent, SFI 04, 10 bytes, READ PIN1, UPDATE ADM.
        let e = by_name("IMSI_M").unwrap();
        assert_eq!(
            e.to_bytes(),
            [0x6F, 0x22, 0x01, 0x04, 0x00, 0x0A, 0x00, 0x12]
        );
        let count = by_name("COUNT").unwrap();
        assert_eq!(count.structure, Structure::Cyclic);
        assert_eq!(count.num_records(), 10);
    }

    #[test]
    fn prl_sizes() {
        assert_eq!(size_of("PRL"), 256);
        assert_eq!(size_of("EPRL"), 12288);
    }
}
