//! Status information record encodings from C.S0005-E §2.7.4.

use crate::bits::Bitstream;
use crate::error::Error;

use super::{AccessInfoRecord, FchTypeSpecificFields, write_fch_type_specific_fields};

pub const INFO_RECORD_TERMINAL: u8 = 0x08;
pub const INFO_RECORD_BAND_CLASS: u8 = 0x0E;
pub const INFO_RECORD_POWER_CONTROL: u8 = 0x17;
pub const INFO_RECORD_CAPABILITY: u8 = 0x1A;
pub const INFO_RECORD_CHANNEL_CONFIG: u8 = 0x1B;
pub const INFO_RECORD_EXT_MULTIPLEX: u8 = 0x1C;
pub const INFO_RECORD_MEID: u8 = 0x27;

const OCTET_BITS: usize = 8;
const FLAG_BITS: usize = 1;
const SLOT_CYCLE_INDEX_BITS: usize = 3;
const OPTION_BITS: usize = 16;
const OPTION_COUNT_BITS: usize = 4;
const BASE_EMPTY_CHANNEL_CLASSES: usize = 4;
const PDCH_CHANNEL_CLASSES: usize = 2;
const FIRST_PDCH_CAPABILITY_P_REV: u8 = 9;
const LAST_BASE_CAPABILITY_P_REV: u8 = 6;
const LATER_CAPABILITY_FLAGS: usize = 13;
const MEID_HEX_DIGITS: usize = 14;
const MEID_BYTES: usize = 7;

fn packed_record(record_type: u8, mut bits: Bitstream) -> AccessInfoRecord {
    while bits.len() % OCTET_BITS != 0 {
        bits.write_u8(0, FLAG_BITS);
    }
    AccessInfoRecord {
        record_type,
        data: bits.to_packed_bytes(),
    }
}

/// C.S0005-E §2.7.4.7. Slot cycle indices are nonnegative before P_REV 11.
#[derive(Debug, Clone)]
pub struct TerminalInformation {
    pub mob_p_rev: u8,
    pub mob_mfg_code: u8,
    pub mob_model: u8,
    pub mob_firm_rev: u16,
    pub scm: u8,
    pub local_ctrl: bool,
    pub slot_cycle_index: u8,
    pub service_options: Vec<u16>,
}

impl TerminalInformation {
    pub fn to_record(&self) -> AccessInfoRecord {
        let mut bits = Bitstream::new();
        bits.write_u8(self.mob_p_rev, OCTET_BITS);
        bits.write_u8(self.mob_mfg_code, OCTET_BITS);
        bits.write_u8(self.mob_model, OCTET_BITS);
        bits.write_u32(u32::from(self.mob_firm_rev), OPTION_BITS);
        bits.write_u8(self.scm, OCTET_BITS);
        bits.write_u8(u8::from(self.local_ctrl), FLAG_BITS);
        bits.write_u8(self.slot_cycle_index, SLOT_CYCLE_INDEX_BITS);
        for option in &self.service_options {
            bits.write_u32(u32::from(*option), OPTION_BITS);
        }
        packed_record(INFO_RECORD_TERMINAL, bits)
    }
}

/// C.S0005-E §2.7.4.25 without extended, RLP, flexible-rate, or variable-rate capabilities.
#[derive(Debug, Clone, Default)]
pub struct BasicCapabilityInformation {
    pub access_entry_ho: bool,
    pub access_probe_ho: bool,
    pub analog_search: bool,
    pub hopping_beacon: bool,
    pub mahho: bool,
    pub puf: bool,
    pub analog_553a: bool,
    pub qpch: bool,
    pub slotted_timer: bool,
    pub gating_rate_set: Option<u8>,
}

impl BasicCapabilityInformation {
    pub fn to_record(&self, p_rev_in_use: u8) -> AccessInfoRecord {
        let mut bits = Bitstream::new();
        for supported in [
            self.access_entry_ho,
            self.access_probe_ho,
            self.analog_search,
            self.hopping_beacon,
            self.mahho,
            self.puf,
            self.analog_553a,
            self.qpch,
            self.slotted_timer,
            self.gating_rate_set.is_some(),
        ] {
            bits.write_u8(u8::from(supported), FLAG_BITS);
        }
        if let Some(gating_rate_set) = self.gating_rate_set {
            const GATING_RATE_BITS: usize = 2;
            bits.write_u8(gating_rate_set, GATING_RATE_BITS);
        }
        bits.write_u8(0, FLAG_BITS);
        bits.write_u8(0, SLOT_CYCLE_INDEX_BITS);
        if p_rev_in_use > LAST_BASE_CAPABILITY_P_REV {
            for _ in 0..LATER_CAPABILITY_FLAGS {
                bits.write_u8(0, FLAG_BITS);
            }
        }
        packed_record(INFO_RECORD_CAPABILITY, bits)
    }
}

/// C.S0005-E §2.7.4.28 FCH multiplex option and its supported frame-size bitmap.
#[derive(Debug, Clone)]
pub struct FchMultiplexOption {
    pub multiplex_option: u16,
    pub num_bits: u8,
}

/// C.S0005-E §2.7.4.28 with DCCH, SCH, and PDCH option lists empty.
#[derive(Debug, Clone, Default)]
pub struct FchMultiplexInformation {
    pub forward: Vec<FchMultiplexOption>,
    pub reverse: Vec<FchMultiplexOption>,
}

impl FchMultiplexInformation {
    pub fn to_record(&self, p_rev_in_use: u8) -> Result<AccessInfoRecord, Error> {
        const MAX_OPTIONS: usize = 15;
        let mut bits = Bitstream::new();
        for options in [&self.forward, &self.reverse] {
            if options.len() > MAX_OPTIONS {
                return Err("FCH multiplex option count exceeds its four-bit field".into());
            }
            bits.write_u8(options.len() as u8, OPTION_COUNT_BITS);
            for option in options {
                bits.write_u32(u32::from(option.multiplex_option), OPTION_BITS);
                bits.write_u8(option.num_bits, OCTET_BITS);
            }
        }
        let empty_classes = BASE_EMPTY_CHANNEL_CLASSES
            + usize::from(p_rev_in_use >= FIRST_PDCH_CAPABILITY_P_REV) * PDCH_CHANNEL_CLASSES;
        for _ in 0..empty_classes {
            bits.write_u8(0, OPTION_COUNT_BITS);
        }
        Ok(packed_record(INFO_RECORD_EXT_MULTIPLEX, bits))
    }
}

/// C.S0005-E §2.7.4.27 FCH-only channel configuration for P_REV 6.
pub fn fch_channel_configuration_record(
    fch: &FchTypeSpecificFields,
) -> Result<AccessInfoRecord, Error> {
    const OTHER_CHANNEL_FLAGS: usize = 8;
    let mut bits = Bitstream::new();
    bits.write_u8(0, FLAG_BITS);
    bits.write_u8(1, FLAG_BITS);
    write_fch_type_specific_fields(&mut bits, fch).map_err(|error| -> Error { error.into() })?;
    for _ in 0..OTHER_CHANNEL_FLAGS {
        bits.write_u8(0, FLAG_BITS);
    }
    Ok(packed_record(INFO_RECORD_CHANNEL_CONFIG, bits))
}

/// C.S0005-E §2.7.4.38. The seven MEID octets follow a four-bit length and precede padding.
pub fn meid_record(meid: &[u8; MEID_BYTES]) -> AccessInfoRecord {
    let mut bits = Bitstream::new();
    bits.write_u8(MEID_BYTES as u8, OPTION_COUNT_BITS);
    for octet in meid {
        bits.write_u8(*octet, OCTET_BITS);
    }
    packed_record(INFO_RECORD_MEID, bits)
}

pub fn parse_meid(meid: &str) -> Option<[u8; MEID_BYTES]> {
    let digits: String = meid.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() != MEID_HEX_DIGITS || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u64::from_str_radix(&digits, 16).ok()?;
    value.to_be_bytes()[1..].try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_record_encodes_caller_identity_and_service_options() {
        let record = TerminalInformation {
            mob_p_rev: 6,
            mob_mfg_code: 0x12,
            mob_model: 0x34,
            mob_firm_rev: 0x5678,
            scm: 0x9a,
            local_ctrl: true,
            slot_cycle_index: 3,
            service_options: vec![0x1234, 0xabcd],
        }
        .to_record();
        assert_eq!(
            record,
            AccessInfoRecord {
                record_type: INFO_RECORD_TERMINAL,
                data: vec![
                    0x06, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xb1, 0x23, 0x4a, 0xbc, 0xd0
                ],
            }
        );
    }

    #[test]
    fn multiplex_record_preserves_different_forward_and_reverse_options() {
        let record = FchMultiplexInformation {
            forward: vec![FchMultiplexOption {
                multiplex_option: 2,
                num_bits: 0xa0,
            }],
            reverse: vec![FchMultiplexOption {
                multiplex_option: 1,
                num_bits: 0xf0,
            }],
        }
        .to_record(6)
        .unwrap();
        assert_eq!(record.data, vec![0x10, 0, 0x2a, 0x01, 0, 1, 0xf0, 0, 0]);
        let too_many = FchMultiplexInformation {
            forward: vec![
                FchMultiplexOption {
                    multiplex_option: 1,
                    num_bits: 0xf0
                };
                16
            ],
            reverse: vec![],
        };
        assert!(too_many.to_record(6).is_err());
    }

    #[test]
    fn meid_record_preserves_leading_zero_octets_and_rejects_invalid_identifiers() {
        let meid = parse_meid("00123456789abc").unwrap();
        assert_eq!(
            meid_record(&meid).data,
            vec![0x70, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xc0]
        );
        assert!(parse_meid("00123456789ab").is_none());
        assert!(parse_meid("00123456789abz").is_none());
    }
}
