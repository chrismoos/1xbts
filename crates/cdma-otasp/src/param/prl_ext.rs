//! Extended Preferred Roaming List (SSPR_P_REV = 3) — C.S0016-D §3.5.5.
//!
//! Full decode + encode for every spec-defined acquisition record type,
//! system record type, and the Common Subnet Table. The classic-PRL
//! decoder is a separate module ([`crate::param::prl`]).
//!
//! Round-trip guarantee: every spec-conformant Extended PRL must
//! satisfy `encode(decode(b)) == b`. Verified against 5+ real-carrier
//! Extended PRLs in `tests/real_prl.rs` + `tests/encoder_round_trip.rs`,
//! plus synthetic coverage in `tests/prl_ext_encoder.rs` for the type
//! variants no real fixture exercises (JTACS / UMB / MCC-MNC / Common
//! Subnet Table).
//!
//! BCD note: MCC and MNC in MCC-MNC system records are 12-bit fields
//! holding three BCD nibbles. We store the raw BCD packed value as
//! `u16` (e.g. MCC = 310 → `0x310`, MNC = 23 → `0x23F` with `F`
//! padding for 2-digit MNCs per [31] / spec §3.5.5.3.2.2). Helpers
//! decode the digits for display.
//!
//! References (all C.S0016-D unless noted):
//! - §3.5.5 Extended PRL header (SSPR_P_REV = 3 layout)
//! - §3.5.5.1 PR_LIST_CRC (CRC-16/GENIBUS, see [`crate::param::prl::compute_prl_crc`])
//! - §3.5.5.2.2 Extended Acquisition Record formats
//! - §3.5.5.3.2 Extended System Record format
//! - §3.5.5.3.2.1 Common Subnet Table
//! - §3.5.5.3.2.2 MCC-MNC type-specific system ID

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8, read_u16, read_u32};
use crate::param::prl::{
    AbSelection, NidInclusion, PcsBlock, PrefNeg, Priority, RoamingIndicator,
    StandardChannelSelection, compute_prl_crc,
};

// ---------------------------------------------------------------------------
// Spec-defined constants
// ---------------------------------------------------------------------------

/// Type codes and bit widths from C.S0016-D §3.5.5.
mod wire {
    // CUR_SSPR_P_REV values (§3.5.5)
    pub(super) const CUR_SSPR_P_REV_3: u8 = 0x03;

    // Header field widths (§3.5.5 Extended PRL header table)
    pub(super) const BITS_PR_LIST_SIZE: usize = 16;
    pub(super) const BITS_PR_LIST_ID: usize = 16;
    pub(super) const BITS_CUR_SSPR_P_REV: usize = 8;
    pub(super) const BITS_PREF_ONLY: usize = 1;
    pub(super) const BITS_DEF_ROAM_IND: usize = 8;
    pub(super) const BITS_NUM_ACQ_RECS: usize = 9;
    pub(super) const BITS_NUM_COMMON_SUBNET_RECS: usize = 9;
    pub(super) const BITS_NUM_SYS_RECS: usize = 14;
    pub(super) const BITS_HEADER_RESERVED: usize = 7;
    pub(super) const BITS_CRC: usize = 16;

    // Minimum-bytes guard for the top-level decoder. Header through
    // the post-NUM_SYS_RECS RESERVED is at least 11 octets.
    pub(super) const MIN_PRL_BYTES: usize = 11;

    // Extended Acquisition Record framing (§3.5.5.2.2 general format)
    pub(super) const BITS_ACQ_TYPE: usize = 8;
    pub(super) const BITS_ACQ_LENGTH: usize = 8;

    // ACQ_TYPE values (Table 3.5.5.2-2)
    pub(super) const ACQ_TYPE_CELLULAR_ANALOG: u8 = 0x01;
    pub(super) const ACQ_TYPE_CELLULAR_CDMA_STANDARD: u8 = 0x02;
    pub(super) const ACQ_TYPE_CELLULAR_CDMA_CUSTOM: u8 = 0x03;
    pub(super) const ACQ_TYPE_CELLULAR_CDMA_PREFERRED: u8 = 0x04;
    pub(super) const ACQ_TYPE_PCS_CDMA_USING_BLOCKS: u8 = 0x05;
    pub(super) const ACQ_TYPE_PCS_CDMA_USING_CHANNELS: u8 = 0x06;
    pub(super) const ACQ_TYPE_JTACS_CDMA_STANDARD: u8 = 0x07;
    pub(super) const ACQ_TYPE_JTACS_CDMA_CUSTOM: u8 = 0x08;
    pub(super) const ACQ_TYPE_BAND_CLASS_6: u8 = 0x09;
    pub(super) const ACQ_TYPE_GENERIC_1X_IS95: u8 = 0x0A;
    pub(super) const ACQ_TYPE_GENERIC_HRPD: u8 = 0x0B;
    pub(super) const ACQ_TYPE_UMB_COMMON_TABLE: u8 = 0x0F;
    pub(super) const ACQ_TYPE_GENERIC_UMB: u8 = 0x10;

    // Per-type bit widths
    pub(super) const BITS_AB: usize = 2; // Table 3.5.5.2.1.1-1
    pub(super) const BITS_PRI_SEC: usize = 2; // Tables 3.5.5.2.1.2-1 / 3.5.5.2.1.7-1
    pub(super) const BITS_NUM_BLOCKS: usize = 3; // §3.5.5.2.2.5
    pub(super) const BITS_PCS_BLOCK: usize = 3; // Table 3.5.5.2.1.5-1
    pub(super) const BITS_NUM_CHANS: usize = 5; // §3.5.5.2.2.3 / .6 / .8 / .9
    pub(super) const BITS_CHAN_NUMBER_11: usize = 11; // 11-bit cellular/PCS channels
    pub(super) const BITS_BAND_CLASS_5: usize = 5; // Generic 1x / HRPD
    pub(super) const BITS_BC_CHAN_PAIR: usize = 16; // 5+11 — §3.5.5.2.2.10, .11
    pub(super) const BITS_UMB_ACQ_PROFILE: usize = 6;
    pub(super) const BITS_UMB_FFT_SIZE: usize = 4;
    pub(super) const BITS_UMB_CYCLIC_PREFIX_LENGTH: usize = 3;
    pub(super) const BITS_UMB_NUM_GUARD_SUBCARRIERS: usize = 7;
    pub(super) const BITS_UMB_PROFILE_ENTRY: usize = BITS_UMB_ACQ_PROFILE
        + BITS_UMB_FFT_SIZE
        + BITS_UMB_CYCLIC_PREFIX_LENGTH
        + BITS_UMB_NUM_GUARD_SUBCARRIERS; // 20
    pub(super) const BITS_UMB_NUM_BLOCKS: usize = 6;
    pub(super) const BITS_UMB_BAND_CLASS: usize = 8;
    pub(super) const BITS_UMB_CHAN_NUMBER: usize = 16;
    pub(super) const BITS_UMB_ACQ_TABLE_PROFILE: usize = 6;

    // Common Subnet Table (§3.5.5.3.2.1)
    pub(super) const BITS_COMMON_SUBNET_RESERVED: usize = 4;
    pub(super) const BITS_SUBNET_COMMON_LENGTH: usize = 4;

    // Extended System Record framing (§3.5.5.3.2 general layout)
    pub(super) const BITS_SYS_RECORD_LENGTH: usize = 5;
    pub(super) const BITS_SYS_RECORD_TYPE: usize = 4;
    pub(super) const BITS_PREF_NEG: usize = 1;
    pub(super) const BITS_GEO: usize = 1;
    pub(super) const BITS_PRI: usize = 1;
    pub(super) const BITS_ACQ_INDEX: usize = 9;
    pub(super) const BITS_ROAM_IND: usize = 8;
    pub(super) const BITS_ASSOCIATION_INC: usize = 1;
    pub(super) const BITS_ASSOCIATION_TAG: usize = 8;
    pub(super) const BITS_PN_ASSOCIATION: usize = 1;
    pub(super) const BITS_DATA_ASSOCIATION: usize = 1;

    // SYS_RECORD_TYPE values (Table 3.5.5.3.2-1)
    pub(super) const SYS_RECORD_TYPE_CDMA2000: u8 = 0x0;
    pub(super) const SYS_RECORD_TYPE_HRPD: u8 = 0x1;
    pub(super) const SYS_RECORD_TYPE_RESERVED_OBSOLETE: u8 = 0x2;
    pub(super) const SYS_RECORD_TYPE_MCC_MNC: u8 = 0x3;

    // CDMA2000 system-id subrecord (Table 3.5.5.3.2-2)
    pub(super) const BITS_CDMA2000_RESERVED: usize = 1;
    pub(super) const BITS_NID_INCL: usize = 2;
    pub(super) const BITS_SID_15: usize = 15; // classic and Cdma2000-subrecord
    pub(super) const BITS_NID_16: usize = 16;

    // HRPD system-id subrecord (Table 3.5.5.3.2-4)
    pub(super) const BITS_HRPD_RESERVED: usize = 3;
    pub(super) const BITS_SUBNET_COMMON_INCLUDED: usize = 1;
    pub(super) const BITS_SUBNET_LSB_LENGTH: usize = 7;
    pub(super) const BITS_SUBNET_COMMON_OFFSET: usize = 12;

    // MCC-MNC system-id subrecord (§3.5.5.3.2.2)
    pub(super) const BITS_SYS_RECORD_SUBTYPE: usize = 3;
    pub(super) const BITS_MCC: usize = 12;
    pub(super) const BITS_MNC: usize = 12;
    pub(super) const BITS_MCC_MNC_RESERVED: usize = 4;
    pub(super) const BITS_NUM_SID: usize = 4;
    pub(super) const BITS_NUM_SID_NID: usize = 4;
    pub(super) const BITS_NUM_SUBNET_ID: usize = 4;
    pub(super) const BITS_MCC_MNC_SID_16: usize = 16; // 16-bit per spec for these subtypes
    pub(super) const BITS_MCC_MNC_NID_16: usize = 16;
    pub(super) const BITS_MCC_MNC_SUBNET_LENGTH: usize = 8;

    // MCC-MNC subtype values (Table 3.5.5.3.2.2-2)
    pub(super) const MCC_MNC_SUBTYPE_000: u8 = 0b000;
    pub(super) const MCC_MNC_SUBTYPE_001: u8 = 0b001;
    pub(super) const MCC_MNC_SUBTYPE_010: u8 = 0b010;
    pub(super) const MCC_MNC_SUBTYPE_011: u8 = 0b011;
}

use wire::*;

const BITS_PER_OCTET: usize = 8;

// ---------------------------------------------------------------------------
// Top-level structure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedPrl {
    pub pr_list_size: u16,
    pub pr_list_id: u16,
    /// Always [`wire::CUR_SSPR_P_REV_3`] for the format we decode.
    pub cur_sspr_p_rev: u8,
    pub pref_only: bool,
    pub def_roam_ind: RoamingIndicator,
    pub acquisition_records: Vec<ExtAcquisitionRecord>,
    pub common_subnet_records: Vec<CommonSubnetRecord>,
    pub system_records: Vec<ExtSystemRecord>,
    pub pr_list_crc: u16,
    pub computed_crc: u16,
}

impl ExtendedPrl {
    pub fn crc_ok(&self) -> bool {
        self.pr_list_crc == self.computed_crc
    }
}

/// Sniff the SSPR_P_REV byte (offset 4). Note: a classic PRL has
/// bit-packed PREF_ONLY + DEF_ROAM_IND at this offset, so a value of
/// `0x80` or other bit pattern can falsely look like an Extended P_REV.
/// Callers that need a reliable classify-or-decode should try classic
/// first and fall back to extended (or check both CRCs).
pub fn sniff_sspr_p_rev(bytes: &[u8]) -> u8 {
    const SSPR_P_REV_OFFSET: usize = 4;
    bytes.get(SSPR_P_REV_OFFSET).copied().unwrap_or(0)
}

pub fn decode(bytes: &[u8]) -> Result<ExtendedPrl, Error> {
    if bytes.len() < MIN_PRL_BYTES {
        return Err("Extended PRL too short".into());
    }
    let mut bs = from_bytes(bytes);
    let pr_list_size = read_u16(&mut bs, BITS_PR_LIST_SIZE)?;
    let pr_list_id = read_u16(&mut bs, BITS_PR_LIST_ID)?;
    let cur_sspr_p_rev = read_u8(&mut bs, BITS_CUR_SSPR_P_REV)?;
    if cur_sspr_p_rev != CUR_SSPR_P_REV_3 {
        return Err(format!(
            "Extended PRL CUR_SSPR_P_REV=0x{:02x} not supported (only 0x{:02x})",
            cur_sspr_p_rev, CUR_SSPR_P_REV_3
        )
        .into());
    }
    let pref_only = read_bool(&mut bs)?;
    let def_roam_ind = RoamingIndicator::from_u8(read_u8(&mut bs, BITS_DEF_ROAM_IND)?);
    let num_acq_recs = read_u16(&mut bs, BITS_NUM_ACQ_RECS)? as usize;
    let num_common_subnet_recs = read_u16(&mut bs, BITS_NUM_COMMON_SUBNET_RECS)? as usize;
    let num_sys_recs = read_u32(&mut bs, BITS_NUM_SYS_RECS)? as usize;
    let _reserved = read_u8(&mut bs, BITS_HEADER_RESERVED)?;

    let mut acquisition_records = Vec::with_capacity(num_acq_recs);
    for _ in 0..num_acq_recs {
        acquisition_records.push(ExtAcquisitionRecord::decode_from(&mut bs)?);
    }
    let mut common_subnet_records = Vec::with_capacity(num_common_subnet_recs);
    for _ in 0..num_common_subnet_recs {
        common_subnet_records.push(CommonSubnetRecord::decode_from(&mut bs)?);
    }
    let mut system_records = Vec::with_capacity(num_sys_recs);
    for _ in 0..num_sys_recs {
        system_records.push(ExtSystemRecord::decode_from(&mut bs)?);
    }

    let remaining = bs.len();
    if remaining < BITS_CRC {
        return Err("Extended PRL truncated before CRC".into());
    }
    let pad = remaining - BITS_CRC;
    if pad >= BITS_PER_OCTET {
        return Err("Extended PRL padding too large".into());
    }
    if pad != 0 {
        let _ = read_u8(&mut bs, pad)?;
    }
    let pr_list_crc = read_u16(&mut bs, BITS_CRC)?;
    let computed_crc = compute_prl_crc(bytes);

    Ok(ExtendedPrl {
        pr_list_size,
        pr_list_id,
        cur_sspr_p_rev,
        pref_only,
        def_roam_ind,
        acquisition_records,
        common_subnet_records,
        system_records,
        pr_list_crc,
        computed_crc,
    })
}

impl ExtendedPrl {
    /// Encode back to on-wire bytes with a freshly computed CRC. Round-trip
    /// guarantee: `decode(bytes).encode() == bytes` for any spec-conformant
    /// input.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let max_acq = 1usize << BITS_NUM_ACQ_RECS;
        let max_subnet = 1usize << BITS_NUM_COMMON_SUBNET_RECS;
        let max_sys = 1usize << BITS_NUM_SYS_RECS;
        if self.acquisition_records.len() >= max_acq {
            return Err("NUM_ACQ_RECS overflow".into());
        }
        if self.common_subnet_records.len() >= max_subnet {
            return Err("NUM_COMMON_SUBNET_RECS overflow".into());
        }
        if self.system_records.len() >= max_sys {
            return Err("NUM_SYS_RECS overflow".into());
        }

        let mut bs = Bitstream::new();
        // PR_LIST_SIZE placeholder; patched at the end.
        bs.write_u32(0, BITS_PR_LIST_SIZE);
        bs.write_u32(self.pr_list_id as u32, BITS_PR_LIST_ID);
        bs.write_u8(self.cur_sspr_p_rev, BITS_CUR_SSPR_P_REV);
        bs.write_u8(self.pref_only as u8, BITS_PREF_ONLY);
        bs.write_u8(self.def_roam_ind.raw(), BITS_DEF_ROAM_IND);
        bs.write_u32(self.acquisition_records.len() as u32, BITS_NUM_ACQ_RECS);
        bs.write_u32(
            self.common_subnet_records.len() as u32,
            BITS_NUM_COMMON_SUBNET_RECS,
        );
        bs.write_u32(self.system_records.len() as u32, BITS_NUM_SYS_RECS);
        bs.write_u8(0, BITS_HEADER_RESERVED);

        for r in &self.acquisition_records {
            r.encode_into(&mut bs)?;
        }
        for r in &self.common_subnet_records {
            r.encode_into(&mut bs)?;
        }
        for r in &self.system_records {
            r.encode_into(&mut bs)?;
        }

        // Trailing RESERVED so PR_LIST_SIZE + body + CRC totals an
        // integer number of octets.
        let pad = (BITS_PER_OCTET - (bs.len() + BITS_CRC) % BITS_PER_OCTET) % BITS_PER_OCTET;
        if pad != 0 {
            bs.write_u32(0, pad);
        }

        // CRC placeholder.
        bs.write_u32(0, BITS_CRC);

        let mut bytes = bs.to_packed_bytes();
        let total = bytes.len() as u16;
        bytes[0..2].copy_from_slice(&total.to_be_bytes());

        let crc = compute_prl_crc(&bytes);
        let n = bytes.len();
        bytes[n - 2..n].copy_from_slice(&crc.to_be_bytes());

        Ok(bytes)
    }
}

// ---------------------------------------------------------------------------
// Extended Acquisition Records (§3.5.5.2.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtAcquisitionRecord {
    pub acq_type_raw: u8,
    /// `LENGTH` field from §3.5.5.2.2 (in octets).
    pub length: u8,
    pub body: ExtAcquisitionBody,
}

/// A BAND_CLASS(5) + CHANNEL_NUMBER(11) pair as used by Generic1xIs95
/// (§3.5.5.2.2.10) and GenericHrpd (§3.5.5.2.2.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandClassChannel {
    pub band_class: u8,
    pub channel_number: u16,
}

/// One UMB Acquisition Profile entry (§3.5.5.2.2.13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UmbAcqProfile {
    pub umb_acq_profile: u8,
    pub fft_size: u8,
    pub cyclic_prefix_length: u8,
    pub num_guard_subcarriers: u8,
}

/// One UMB block entry (§3.5.5.2.2.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UmbBlock {
    pub band_class: u8,
    pub channel_number: u16,
    pub umb_acq_table_profile: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtAcquisitionBody {
    /// `ACQ_TYPE_CELLULAR_ANALOG` (§3.5.5.2.2.1)
    CellularAnalog { ab: AbSelection },
    /// `ACQ_TYPE_CELLULAR_CDMA_STANDARD` (§3.5.5.2.2.2)
    CellularCdmaStandard {
        ab: AbSelection,
        pri_sec: StandardChannelSelection,
    },
    /// `ACQ_TYPE_CELLULAR_CDMA_CUSTOM` (§3.5.5.2.2.3)
    CellularCdmaCustom { channels: Vec<u16> },
    /// `ACQ_TYPE_CELLULAR_CDMA_PREFERRED` (§3.5.5.2.2.4)
    CellularCdmaPreferred { ab: AbSelection },
    /// `ACQ_TYPE_PCS_CDMA_USING_BLOCKS` (§3.5.5.2.2.5)
    PcsCdmaUsingBlocks { blocks: Vec<PcsBlock> },
    /// `ACQ_TYPE_PCS_CDMA_USING_CHANNELS` (§3.5.5.2.2.6)
    PcsCdmaUsingChannels { channels: Vec<u16> },
    /// `ACQ_TYPE_JTACS_CDMA_STANDARD` (§3.5.5.2.2.7)
    JtacsCdmaStandard {
        ab: AbSelection,
        pri_sec: StandardChannelSelection,
    },
    /// `ACQ_TYPE_JTACS_CDMA_CUSTOM` (§3.5.5.2.2.8)
    JtacsCdmaCustom { channels: Vec<u16> },
    /// `ACQ_TYPE_BAND_CLASS_6` (§3.5.5.2.2.9)
    BandClass6UsingChannels { channels: Vec<u16> },
    /// `ACQ_TYPE_GENERIC_1X_IS95` (§3.5.5.2.2.10)
    Generic1xIs95 { entries: Vec<BandClassChannel> },
    /// `ACQ_TYPE_GENERIC_HRPD` (§3.5.5.2.2.11)
    GenericHrpd { entries: Vec<BandClassChannel> },
    /// `ACQ_TYPE_UMB_COMMON_TABLE` (§3.5.5.2.2.13)
    UmbCommonTable { entries: Vec<UmbAcqProfile> },
    /// `ACQ_TYPE_GENERIC_UMB` (§3.5.5.2.2.14)
    GenericUmb { blocks: Vec<UmbBlock> },
    /// Any other ACQ_TYPE (e.g. Reserved Obsolete 0x0C–0x0E, future
    /// reserved). Bytes preserved verbatim.
    Other { raw: Vec<u8> },
}

impl ExtAcquisitionRecord {
    fn decode_from(bs: &mut Bitstream) -> Result<Self, Error> {
        let acq_type_raw = read_u8(bs, BITS_ACQ_TYPE)?;
        let length = read_u8(bs, BITS_ACQ_LENGTH)?;
        let total_body_bits = (length as usize) * BITS_PER_OCTET;

        // Read body bits into a buffered sub-stream so we can enforce
        // the LENGTH frame regardless of body shape.
        let body_bits = drain_bits(bs, total_body_bits)?;
        let mut body_bs = bits_to_stream(&body_bits);

        let body = match acq_type_raw {
            ACQ_TYPE_CELLULAR_ANALOG => ExtAcquisitionBody::CellularAnalog {
                ab: AbSelection::from_u8(read_u8(&mut body_bs, BITS_AB)?),
            },
            ACQ_TYPE_CELLULAR_CDMA_STANDARD => ExtAcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::from_u8(read_u8(&mut body_bs, BITS_AB)?),
                pri_sec: StandardChannelSelection::from_u8(read_u8(&mut body_bs, BITS_PRI_SEC)?),
            },
            ACQ_TYPE_CELLULAR_CDMA_CUSTOM => ExtAcquisitionBody::CellularCdmaCustom {
                channels: read_chan_list(&mut body_bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_CELLULAR_CDMA_PREFERRED => ExtAcquisitionBody::CellularCdmaPreferred {
                ab: AbSelection::from_u8(read_u8(&mut body_bs, BITS_AB)?),
            },
            ACQ_TYPE_PCS_CDMA_USING_BLOCKS => {
                let n = read_u8(&mut body_bs, BITS_NUM_BLOCKS)? as usize;
                let mut blocks = Vec::with_capacity(n);
                for _ in 0..n {
                    blocks.push(PcsBlock::from_u8(read_u8(&mut body_bs, BITS_PCS_BLOCK)?));
                }
                ExtAcquisitionBody::PcsCdmaUsingBlocks { blocks }
            }
            ACQ_TYPE_PCS_CDMA_USING_CHANNELS => ExtAcquisitionBody::PcsCdmaUsingChannels {
                channels: read_chan_list(&mut body_bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_JTACS_CDMA_STANDARD => ExtAcquisitionBody::JtacsCdmaStandard {
                ab: AbSelection::from_u8(read_u8(&mut body_bs, BITS_AB)?),
                pri_sec: StandardChannelSelection::from_u8(read_u8(&mut body_bs, BITS_PRI_SEC)?),
            },
            ACQ_TYPE_JTACS_CDMA_CUSTOM => ExtAcquisitionBody::JtacsCdmaCustom {
                channels: read_chan_list(&mut body_bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_BAND_CLASS_6 => ExtAcquisitionBody::BandClass6UsingChannels {
                channels: read_chan_list(&mut body_bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_GENERIC_1X_IS95 => ExtAcquisitionBody::Generic1xIs95 {
                entries: read_band_class_channel_list(&mut body_bs, total_body_bits)?,
            },
            ACQ_TYPE_GENERIC_HRPD => ExtAcquisitionBody::GenericHrpd {
                entries: read_band_class_channel_list(&mut body_bs, total_body_bits)?,
            },
            ACQ_TYPE_UMB_COMMON_TABLE => ExtAcquisitionBody::UmbCommonTable {
                entries: read_umb_acq_profile_list(&mut body_bs, total_body_bits)?,
            },
            ACQ_TYPE_GENERIC_UMB => {
                let n = read_u8(&mut body_bs, BITS_UMB_NUM_BLOCKS)? as usize;
                let mut blocks = Vec::with_capacity(n);
                for _ in 0..n {
                    blocks.push(UmbBlock {
                        band_class: read_u8(&mut body_bs, BITS_UMB_BAND_CLASS)?,
                        channel_number: read_u16(&mut body_bs, BITS_UMB_CHAN_NUMBER)?,
                        umb_acq_table_profile: read_u8(&mut body_bs, BITS_UMB_ACQ_TABLE_PROFILE)?,
                    });
                }
                ExtAcquisitionBody::GenericUmb { blocks }
            }
            _ => ExtAcquisitionBody::Other {
                raw: pack_bits_to_bytes(&body_bits),
            },
        };
        Ok(Self {
            acq_type_raw,
            length,
            body,
        })
    }

    fn encode_into(&self, bs: &mut Bitstream) -> Result<(), Error> {
        let mut body_bs = Bitstream::new();
        match &self.body {
            ExtAcquisitionBody::CellularAnalog { ab } => {
                body_bs.write_u8(ab.to_u8(), BITS_AB);
            }
            ExtAcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
                body_bs.write_u8(ab.to_u8(), BITS_AB);
                body_bs.write_u8(pri_sec.to_u8(), BITS_PRI_SEC);
            }
            ExtAcquisitionBody::CellularCdmaCustom { channels } => {
                write_chan_list(&mut body_bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            ExtAcquisitionBody::CellularCdmaPreferred { ab } => {
                body_bs.write_u8(ab.to_u8(), BITS_AB);
            }
            ExtAcquisitionBody::PcsCdmaUsingBlocks { blocks } => {
                let max = 1usize << BITS_NUM_BLOCKS;
                if blocks.len() >= max {
                    return Err("NUM_BLOCKS overflow".into());
                }
                body_bs.write_u8(blocks.len() as u8, BITS_NUM_BLOCKS);
                for b in blocks {
                    body_bs.write_u8(b.to_u8(), BITS_PCS_BLOCK);
                }
            }
            ExtAcquisitionBody::PcsCdmaUsingChannels { channels } => {
                write_chan_list(&mut body_bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            ExtAcquisitionBody::JtacsCdmaStandard { ab, pri_sec } => {
                body_bs.write_u8(ab.to_u8(), BITS_AB);
                body_bs.write_u8(pri_sec.to_u8(), BITS_PRI_SEC);
            }
            ExtAcquisitionBody::JtacsCdmaCustom { channels } => {
                write_chan_list(&mut body_bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            ExtAcquisitionBody::BandClass6UsingChannels { channels } => {
                write_chan_list(&mut body_bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            ExtAcquisitionBody::Generic1xIs95 { entries }
            | ExtAcquisitionBody::GenericHrpd { entries } => {
                for e in entries {
                    body_bs.write_u8(e.band_class, BITS_BAND_CLASS_5);
                    body_bs.write_u32(e.channel_number as u32, BITS_CHAN_NUMBER_11);
                }
            }
            ExtAcquisitionBody::UmbCommonTable { entries } => {
                for e in entries {
                    body_bs.write_u8(e.umb_acq_profile, BITS_UMB_ACQ_PROFILE);
                    body_bs.write_u8(e.fft_size, BITS_UMB_FFT_SIZE);
                    body_bs.write_u8(e.cyclic_prefix_length, BITS_UMB_CYCLIC_PREFIX_LENGTH);
                    body_bs.write_u8(e.num_guard_subcarriers, BITS_UMB_NUM_GUARD_SUBCARRIERS);
                }
            }
            ExtAcquisitionBody::GenericUmb { blocks } => {
                let max = 1usize << BITS_UMB_NUM_BLOCKS;
                if blocks.len() >= max {
                    return Err("NUM_UMB_BLOCKS overflow".into());
                }
                body_bs.write_u8(blocks.len() as u8, BITS_UMB_NUM_BLOCKS);
                for b in blocks {
                    body_bs.write_u8(b.band_class, BITS_UMB_BAND_CLASS);
                    body_bs.write_u32(b.channel_number as u32, BITS_UMB_CHAN_NUMBER);
                    body_bs.write_u8(b.umb_acq_table_profile, BITS_UMB_ACQ_TABLE_PROFILE);
                }
            }
            ExtAcquisitionBody::Other { raw } => {
                for &byte in raw {
                    body_bs.write_u8(byte, BITS_PER_OCTET);
                }
            }
        }
        let target_bits = (self.length as usize) * BITS_PER_OCTET;
        if body_bs.len() > target_bits {
            return Err(format!(
                "Extended acq body ({} bits) exceeds LENGTH={} octets",
                body_bs.len(),
                self.length
            )
            .into());
        }
        let pad = target_bits - body_bs.len();
        if pad > 0 {
            body_bs.write_u64(0, pad);
        }

        bs.write_u8(self.acq_type_raw, BITS_ACQ_TYPE);
        bs.write_u8(self.length, BITS_ACQ_LENGTH);
        bs.extend(&body_bs);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Common Subnet Table (§3.5.5.3.2.1)
// ---------------------------------------------------------------------------

/// One row of the Common Subnet Table — referenced by HRPD system
/// records via `SUBNET_COMMON_OFFSET` (offset in octets into the table).
///
/// Layout: `RESERVED(4) + SUBNET_COMMON_LENGTH(4) + SUBNET_COMMON(8 × SCL)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonSubnetRecord {
    /// Number of octets of `subnet_common` that follow on the wire.
    /// Bounded by the `BITS_SUBNET_COMMON_LENGTH`-wide field.
    pub subnet_common_length: u8,
    pub subnet_common: Vec<u8>,
}

impl CommonSubnetRecord {
    fn decode_from(bs: &mut Bitstream) -> Result<Self, Error> {
        let _reserved = read_u8(bs, BITS_COMMON_SUBNET_RESERVED)?;
        let subnet_common_length = read_u8(bs, BITS_SUBNET_COMMON_LENGTH)?;
        let mut subnet_common = Vec::with_capacity(subnet_common_length as usize);
        for _ in 0..subnet_common_length {
            subnet_common.push(read_u8(bs, BITS_PER_OCTET)?);
        }
        Ok(Self {
            subnet_common_length,
            subnet_common,
        })
    }

    fn encode_into(&self, bs: &mut Bitstream) -> Result<(), Error> {
        let max = 1u8 << BITS_SUBNET_COMMON_LENGTH;
        if self.subnet_common_length >= max {
            return Err("SUBNET_COMMON_LENGTH overflow".into());
        }
        if self.subnet_common.len() != self.subnet_common_length as usize {
            return Err(format!(
                "Common subnet: declared length {} != actual byte count {}",
                self.subnet_common_length,
                self.subnet_common.len()
            )
            .into());
        }
        bs.write_u8(0, BITS_COMMON_SUBNET_RESERVED);
        bs.write_u8(self.subnet_common_length, BITS_SUBNET_COMMON_LENGTH);
        for &b in &self.subnet_common {
            bs.write_u8(b, BITS_PER_OCTET);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Extended System Records (§3.5.5.3.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtSystemRecord {
    /// `SYS_RECORD_LENGTH` in octets.
    pub sys_record_length: u8,
    pub sys_record_type: ExtSystemRecordType,
    pub pref_neg: PrefNeg,
    pub same_geo_as_prev: bool,
    /// Per spec, PRI is always present in extended records (unlike
    /// classic where it's omitted when PREF_NEG = Negative).
    pub priority: Priority,
    pub acq_index: u16,
    pub system_id: ExtSystemId,
    /// Roaming indicator: only present when `pref_neg == Preferred`.
    pub roaming_indicator: Option<RoamingIndicator>,
    pub association: Option<ExtSystemAssociation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSystemRecordType {
    Cdma2000,
    Hrpd,
    /// `0010` — Obsolete identification (MS should ignore per spec).
    ReservedObsolete,
    MccMnc,
    /// `0100`–`1111` — reserved future.
    Reserved(u8),
}

impl ExtSystemRecordType {
    fn from_u8(v: u8) -> Self {
        const FIELD_MASK: u8 = 0x0F;
        match v & FIELD_MASK {
            SYS_RECORD_TYPE_CDMA2000 => Self::Cdma2000,
            SYS_RECORD_TYPE_HRPD => Self::Hrpd,
            SYS_RECORD_TYPE_RESERVED_OBSOLETE => Self::ReservedObsolete,
            SYS_RECORD_TYPE_MCC_MNC => Self::MccMnc,
            other => Self::Reserved(other),
        }
    }

    fn to_u8(self) -> u8 {
        const FIELD_MASK: u8 = 0x0F;
        match self {
            Self::Cdma2000 => SYS_RECORD_TYPE_CDMA2000,
            Self::Hrpd => SYS_RECORD_TYPE_HRPD,
            Self::ReservedObsolete => SYS_RECORD_TYPE_RESERVED_OBSOLETE,
            Self::MccMnc => SYS_RECORD_TYPE_MCC_MNC,
            Self::Reserved(v) => v & FIELD_MASK,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtSystemAssociation {
    pub association_tag: u8,
    pub pn_association: bool,
    pub data_association: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtSystemId {
    /// `SYS_RECORD_TYPE_CDMA2000` (Table 3.5.5.3.2-2).
    Cdma2000 {
        nid_incl: NidInclusion,
        sid: u16,
        nid: Option<u16>,
    },
    /// `SYS_RECORD_TYPE_HRPD` (Table 3.5.5.3.2-4).
    Hrpd {
        subnet_common_included: bool,
        /// Length in bits of the HRPD subnet LSB segment.
        subnet_lsb_length: u8,
        /// `subnet_lsb_length` bits, packed MSB-first into bytes.
        subnet_lsb: Vec<u8>,
        /// Offset (in octets) into the Common Subnet Table. Present
        /// iff `subnet_common_included = true`.
        subnet_common_offset: Option<u16>,
    },
    /// `SYS_RECORD_TYPE_MCC_MNC` (Table 3.5.5.3.2.2-1).
    MccMnc(MccMncSubtype),
    /// Any other SYS_RECORD_TYPE — bit-stream captured for fidelity.
    Raw {
        sys_record_type: u8,
        /// Body bits between ACQ_INDEX and ROAM_IND, MSB-first packed.
        raw_bits: Vec<u8>,
        raw_bit_len: usize,
    },
}

/// MCC-MNC system record subtype (§3.5.5.3.2.2 Table 3.5.5.3.2.2-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MccMncSubtype {
    /// `MCC_MNC_SUBTYPE_000` — MCC + MNC only.
    Subtype000 { mcc_bcd: u16, mnc_bcd: u16 },
    /// `MCC_MNC_SUBTYPE_001` — MCC + MNC + list of 16-bit SIDs.
    Subtype001 {
        mcc_bcd: u16,
        mnc_bcd: u16,
        sids: Vec<u16>,
    },
    /// `MCC_MNC_SUBTYPE_010` — MCC + MNC + list of {SID, NID} pairs.
    Subtype010 {
        mcc_bcd: u16,
        mnc_bcd: u16,
        pairs: Vec<SidNidPair>,
    },
    /// `MCC_MNC_SUBTYPE_011` — MCC + MNC + list of {SUBNET_LENGTH, SUBNET_ID} pairs.
    Subtype011 {
        mcc_bcd: u16,
        mnc_bcd: u16,
        subnets: Vec<MccMncSubnet>,
    },
    /// Reserved future subtypes — body bits captured.
    Reserved {
        subtype: u8,
        raw_bits: Vec<u8>,
        raw_bit_len: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidNidPair {
    pub sid: u16,
    pub nid: u16,
}

/// One subnet entry inside an MCC-MNC subtype-`011` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MccMncSubnet {
    pub subnet_length: u8,
    pub subnet_id: Vec<u8>,
}

impl ExtSystemRecord {
    fn decode_from(bs: &mut Bitstream) -> Result<Self, Error> {
        let sys_record_length = read_u8(bs, BITS_SYS_RECORD_LENGTH)?;
        let total_bits_in_record = (sys_record_length as usize) * BITS_PER_OCTET;
        if total_bits_in_record < BITS_SYS_RECORD_LENGTH {
            return Err("Extended sys record too short".into());
        }
        // Drain the rest of the record into a sub-stream so we can
        // enforce the SYS_RECORD_LENGTH frame regardless of body shape.
        let record_body_bits = drain_bits(bs, total_bits_in_record - BITS_SYS_RECORD_LENGTH)?;
        let mut rbs = bits_to_stream(&record_body_bits);

        let sys_record_type_raw = read_u8(&mut rbs, BITS_SYS_RECORD_TYPE)?;
        let sys_record_type = ExtSystemRecordType::from_u8(sys_record_type_raw);
        let pref_neg = if read_bool(&mut rbs)? {
            PrefNeg::Preferred
        } else {
            PrefNeg::Negative
        };
        let same_geo_as_prev = read_bool(&mut rbs)?;
        let priority = if read_bool(&mut rbs)? {
            Priority::MoreDesirable
        } else {
            Priority::EquallyDesirable
        };
        let acq_index = read_u16(&mut rbs, BITS_ACQ_INDEX)?;

        let system_id = match sys_record_type {
            ExtSystemRecordType::Cdma2000 => {
                let _reserved = read_u8(&mut rbs, BITS_CDMA2000_RESERVED)?;
                let nid_incl = NidInclusion::from_u8(read_u8(&mut rbs, BITS_NID_INCL)?);
                let sid = read_u16(&mut rbs, BITS_SID_15)?;
                let nid = if matches!(nid_incl, NidInclusion::SingleNid) {
                    Some(read_u16(&mut rbs, BITS_NID_16)?)
                } else {
                    None
                };
                ExtSystemId::Cdma2000 { nid_incl, sid, nid }
            }
            ExtSystemRecordType::Hrpd => {
                let _reserved = read_u8(&mut rbs, BITS_HRPD_RESERVED)?;
                let subnet_common_included = read_bool(&mut rbs)?;
                let subnet_lsb_length = read_u8(&mut rbs, BITS_SUBNET_LSB_LENGTH)?;
                let lsb_bits = drain_bits(&mut rbs, subnet_lsb_length as usize)?;
                let subnet_lsb = pack_bits_to_bytes(&lsb_bits);
                let subnet_common_offset = if subnet_common_included {
                    Some(read_u16(&mut rbs, BITS_SUBNET_COMMON_OFFSET)?)
                } else {
                    None
                };
                ExtSystemId::Hrpd {
                    subnet_common_included,
                    subnet_lsb_length,
                    subnet_lsb,
                    subnet_common_offset,
                }
            }
            ExtSystemRecordType::MccMnc => {
                ExtSystemId::MccMnc(MccMncSubtype::decode_from(&mut rbs)?)
            }
            ExtSystemRecordType::ReservedObsolete | ExtSystemRecordType::Reserved(_) => {
                // For unknown system-record types we don't know where
                // the type-specific record ends, so capture the rest
                // of the record bits up to the trailing tail. The tail
                // for these is impossible to recover deterministically;
                // we capture EVERY remaining body bit as raw and skip
                // ROAM_IND/ASSOC. This means a re-encode preserves the
                // bytes exactly but loses field-level introspection.
                let remaining = rbs.len();
                let raw_bit_len = remaining;
                let bits = drain_bits(&mut rbs, remaining)?;
                let raw_bits = pack_bits_to_bytes(&bits);
                let final_priority = priority;
                return Ok(Self {
                    sys_record_length,
                    sys_record_type,
                    pref_neg,
                    same_geo_as_prev,
                    priority: final_priority,
                    acq_index,
                    system_id: ExtSystemId::Raw {
                        sys_record_type: sys_record_type_raw,
                        raw_bits,
                        raw_bit_len,
                    },
                    roaming_indicator: None,
                    association: None,
                });
            }
        };

        // Spec tail: ROAM_IND (when pref_neg = Preferred), then
        // ASSOCIATION_INC (+ optional ASSOC_TAG/PN/DATA), then trailing
        // RESERVED to fill out the record.
        let roaming_indicator = if matches!(pref_neg, PrefNeg::Preferred) {
            Some(RoamingIndicator::from_u8(read_u8(&mut rbs, BITS_ROAM_IND)?))
        } else {
            None
        };
        let association = if read_bool(&mut rbs)? {
            let association_tag = read_u8(&mut rbs, BITS_ASSOCIATION_TAG)?;
            let pn_association = read_bool(&mut rbs)?;
            let data_association = read_bool(&mut rbs)?;
            Some(ExtSystemAssociation {
                association_tag,
                pn_association,
                data_association,
            })
        } else {
            None
        };
        // Trailing RESERVED — consume whatever's left in the record frame.
        let trailing = rbs.len();
        if trailing > 0 {
            let _ = drain_bits(&mut rbs, trailing)?;
        }

        Ok(Self {
            sys_record_length,
            sys_record_type,
            pref_neg,
            same_geo_as_prev,
            priority,
            acq_index,
            system_id,
            roaming_indicator,
            association,
        })
    }

    fn encode_into(&self, bs: &mut Bitstream) -> Result<(), Error> {
        let mut body = Bitstream::new();
        body.write_u8(self.sys_record_type.to_u8(), BITS_SYS_RECORD_TYPE);
        body.write_u8(
            match self.pref_neg {
                PrefNeg::Preferred => 1,
                PrefNeg::Negative => 0,
            },
            BITS_PREF_NEG,
        );
        body.write_u8(self.same_geo_as_prev as u8, BITS_GEO);
        body.write_u8(
            match self.priority {
                Priority::MoreDesirable => 1,
                Priority::EquallyDesirable => 0,
            },
            BITS_PRI,
        );
        body.write_u32(self.acq_index as u32, BITS_ACQ_INDEX);

        match &self.system_id {
            ExtSystemId::Cdma2000 { nid_incl, sid, nid } => {
                body.write_u8(0, BITS_CDMA2000_RESERVED);
                body.write_u8(nid_incl.to_u8(), BITS_NID_INCL);
                body.write_u32(*sid as u32, BITS_SID_15);
                if let Some(n) = nid {
                    body.write_u32(*n as u32, BITS_NID_16);
                }
            }
            ExtSystemId::Hrpd {
                subnet_common_included,
                subnet_lsb_length,
                subnet_lsb,
                subnet_common_offset,
            } => {
                body.write_u8(0, BITS_HRPD_RESERVED);
                body.write_u8(*subnet_common_included as u8, BITS_SUBNET_COMMON_INCLUDED);
                body.write_u8(*subnet_lsb_length, BITS_SUBNET_LSB_LENGTH);
                let want_bytes = subnet_lsb_length.div_ceil(BITS_PER_OCTET as u8) as usize;
                if subnet_lsb.len() != want_bytes {
                    return Err(format!(
                        "HRPD sys record: subnet_lsb has {} bytes but subnet_lsb_length={} expects {} bytes",
                        subnet_lsb.len(),
                        subnet_lsb_length,
                        want_bytes
                    ).into());
                }
                write_packed_bits(&mut body, subnet_lsb, *subnet_lsb_length as usize);
                if let Some(off) = subnet_common_offset {
                    if !subnet_common_included {
                        return Err("HRPD sys record: subnet_common_offset set without subnet_common_included".into());
                    }
                    body.write_u32(*off as u32, BITS_SUBNET_COMMON_OFFSET);
                } else if *subnet_common_included {
                    return Err("HRPD sys record: subnet_common_included but offset missing".into());
                }
            }
            ExtSystemId::MccMnc(subtype) => subtype.encode_into(&mut body)?,
            ExtSystemId::Raw {
                sys_record_type: _,
                raw_bits,
                raw_bit_len,
            } => {
                write_packed_bits(&mut body, raw_bits, *raw_bit_len);
                let total_bits_in_record = (self.sys_record_length as usize) * BITS_PER_OCTET;
                let used = BITS_SYS_RECORD_LENGTH + body.len();
                if used > total_bits_in_record {
                    return Err(
                        "Extended sys record (raw subtype): body exceeds SYS_RECORD_LENGTH".into(),
                    );
                }
                let pad = total_bits_in_record - used;
                if pad > 0 {
                    body.write_u64(0, pad);
                }
                bs.write_u8(self.sys_record_length, BITS_SYS_RECORD_LENGTH);
                bs.extend(&body);
                return Ok(());
            }
        }

        if let Some(roam) = self.roaming_indicator {
            body.write_u8(roam.raw(), BITS_ROAM_IND);
        }
        match &self.association {
            Some(a) => {
                body.write_u8(1, BITS_ASSOCIATION_INC);
                body.write_u8(a.association_tag, BITS_ASSOCIATION_TAG);
                body.write_u8(a.pn_association as u8, BITS_PN_ASSOCIATION);
                body.write_u8(a.data_association as u8, BITS_DATA_ASSOCIATION);
            }
            None => {
                body.write_u8(0, BITS_ASSOCIATION_INC);
            }
        }

        // Pad body so SYS_RECORD_LENGTH + body = sys_record_length octets.
        let total_bits_in_record = (self.sys_record_length as usize) * BITS_PER_OCTET;
        let used = BITS_SYS_RECORD_LENGTH + body.len();
        if used > total_bits_in_record {
            return Err(format!(
                "Extended sys record body ({} bits) exceeds SYS_RECORD_LENGTH={} octets",
                used, self.sys_record_length
            )
            .into());
        }
        let pad = total_bits_in_record - used;
        if pad > 0 {
            body.write_u64(0, pad);
        }
        bs.write_u8(self.sys_record_length, BITS_SYS_RECORD_LENGTH);
        bs.extend(&body);
        Ok(())
    }
}

impl MccMncSubtype {
    fn decode_from(bs: &mut Bitstream) -> Result<Self, Error> {
        let subtype = read_u8(bs, BITS_SYS_RECORD_SUBTYPE)?;
        match subtype {
            MCC_MNC_SUBTYPE_000 => {
                let mcc_bcd = read_u16(bs, BITS_MCC)?;
                let mnc_bcd = read_u16(bs, BITS_MNC)?;
                Ok(Self::Subtype000 { mcc_bcd, mnc_bcd })
            }
            MCC_MNC_SUBTYPE_001 => {
                let mcc_bcd = read_u16(bs, BITS_MCC)?;
                let mnc_bcd = read_u16(bs, BITS_MNC)?;
                let _reserved = read_u8(bs, BITS_MCC_MNC_RESERVED)?;
                let n = read_u8(bs, BITS_NUM_SID)? as usize;
                let mut sids = Vec::with_capacity(n);
                for _ in 0..n {
                    sids.push(read_u16(bs, BITS_MCC_MNC_SID_16)?);
                }
                Ok(Self::Subtype001 {
                    mcc_bcd,
                    mnc_bcd,
                    sids,
                })
            }
            MCC_MNC_SUBTYPE_010 => {
                let mcc_bcd = read_u16(bs, BITS_MCC)?;
                let mnc_bcd = read_u16(bs, BITS_MNC)?;
                let _reserved = read_u8(bs, BITS_MCC_MNC_RESERVED)?;
                let n = read_u8(bs, BITS_NUM_SID_NID)? as usize;
                let mut pairs = Vec::with_capacity(n);
                for _ in 0..n {
                    let sid = read_u16(bs, BITS_MCC_MNC_SID_16)?;
                    let nid = read_u16(bs, BITS_MCC_MNC_NID_16)?;
                    pairs.push(SidNidPair { sid, nid });
                }
                Ok(Self::Subtype010 {
                    mcc_bcd,
                    mnc_bcd,
                    pairs,
                })
            }
            MCC_MNC_SUBTYPE_011 => {
                let mcc_bcd = read_u16(bs, BITS_MCC)?;
                let mnc_bcd = read_u16(bs, BITS_MNC)?;
                let _reserved = read_u8(bs, BITS_MCC_MNC_RESERVED)?;
                let n = read_u8(bs, BITS_NUM_SUBNET_ID)? as usize;
                let mut subnets = Vec::with_capacity(n);
                for _ in 0..n {
                    let subnet_length = read_u8(bs, BITS_MCC_MNC_SUBNET_LENGTH)?;
                    let bits = drain_bits(bs, subnet_length as usize)?;
                    let subnet_id = pack_bits_to_bytes(&bits);
                    subnets.push(MccMncSubnet {
                        subnet_length,
                        subnet_id,
                    });
                }
                Ok(Self::Subtype011 {
                    mcc_bcd,
                    mnc_bcd,
                    subnets,
                })
            }
            other => {
                // Reserved subtype — capture remaining frame bits.
                let raw_bit_len = bs.len();
                let bits = drain_bits(bs, raw_bit_len)?;
                let raw_bits = pack_bits_to_bytes(&bits);
                Ok(Self::Reserved {
                    subtype: other,
                    raw_bits,
                    raw_bit_len,
                })
            }
        }
    }

    fn encode_into(&self, bs: &mut Bitstream) -> Result<(), Error> {
        match self {
            Self::Subtype000 { mcc_bcd, mnc_bcd } => {
                bs.write_u8(MCC_MNC_SUBTYPE_000, BITS_SYS_RECORD_SUBTYPE);
                bs.write_u32(*mcc_bcd as u32, BITS_MCC);
                bs.write_u32(*mnc_bcd as u32, BITS_MNC);
            }
            Self::Subtype001 {
                mcc_bcd,
                mnc_bcd,
                sids,
            } => {
                bs.write_u8(MCC_MNC_SUBTYPE_001, BITS_SYS_RECORD_SUBTYPE);
                bs.write_u32(*mcc_bcd as u32, BITS_MCC);
                bs.write_u32(*mnc_bcd as u32, BITS_MNC);
                bs.write_u8(0, BITS_MCC_MNC_RESERVED);
                let max = 1usize << BITS_NUM_SID;
                if sids.len() >= max {
                    return Err("MCC-MNC subtype 001: NUM_SID overflow".into());
                }
                bs.write_u8(sids.len() as u8, BITS_NUM_SID);
                for sid in sids {
                    bs.write_u32(*sid as u32, BITS_MCC_MNC_SID_16);
                }
            }
            Self::Subtype010 {
                mcc_bcd,
                mnc_bcd,
                pairs,
            } => {
                bs.write_u8(MCC_MNC_SUBTYPE_010, BITS_SYS_RECORD_SUBTYPE);
                bs.write_u32(*mcc_bcd as u32, BITS_MCC);
                bs.write_u32(*mnc_bcd as u32, BITS_MNC);
                bs.write_u8(0, BITS_MCC_MNC_RESERVED);
                let max = 1usize << BITS_NUM_SID_NID;
                if pairs.len() >= max {
                    return Err("MCC-MNC subtype 010: NUM_SID_NID overflow".into());
                }
                bs.write_u8(pairs.len() as u8, BITS_NUM_SID_NID);
                for p in pairs {
                    bs.write_u32(p.sid as u32, BITS_MCC_MNC_SID_16);
                    bs.write_u32(p.nid as u32, BITS_MCC_MNC_NID_16);
                }
            }
            Self::Subtype011 {
                mcc_bcd,
                mnc_bcd,
                subnets,
            } => {
                bs.write_u8(MCC_MNC_SUBTYPE_011, BITS_SYS_RECORD_SUBTYPE);
                bs.write_u32(*mcc_bcd as u32, BITS_MCC);
                bs.write_u32(*mnc_bcd as u32, BITS_MNC);
                bs.write_u8(0, BITS_MCC_MNC_RESERVED);
                let max = 1usize << BITS_NUM_SUBNET_ID;
                if subnets.len() >= max {
                    return Err("MCC-MNC subtype 011: NUM_SUBNET_ID overflow".into());
                }
                bs.write_u8(subnets.len() as u8, BITS_NUM_SUBNET_ID);
                for s in subnets {
                    bs.write_u8(s.subnet_length, BITS_MCC_MNC_SUBNET_LENGTH);
                    let want_bytes = s.subnet_length.div_ceil(BITS_PER_OCTET as u8) as usize;
                    if s.subnet_id.len() != want_bytes {
                        return Err(format!(
                            "MCC-MNC subnet: subnet_id has {} bytes but subnet_length={} expects {} bytes",
                            s.subnet_id.len(),
                            s.subnet_length,
                            want_bytes
                        ).into());
                    }
                    write_packed_bits(bs, &s.subnet_id, s.subnet_length as usize);
                }
            }
            Self::Reserved {
                subtype,
                raw_bits,
                raw_bit_len,
            } => {
                const FIELD_MASK: u8 = 0b111;
                bs.write_u8(subtype & FIELD_MASK, BITS_SYS_RECORD_SUBTYPE);
                write_packed_bits(bs, raw_bits, *raw_bit_len);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_chan_list(
    bs: &mut Bitstream,
    count_bits: usize,
    chan_bits: usize,
) -> Result<Vec<u16>, Error> {
    let n = read_u32(bs, count_bits)? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(read_u16(bs, chan_bits)?);
    }
    Ok(out)
}

fn write_chan_list(
    bs: &mut Bitstream,
    channels: &[u16],
    count_bits: usize,
    chan_bits: usize,
) -> Result<(), Error> {
    let max_n = 1usize << count_bits;
    if channels.len() >= max_n {
        return Err(format!(
            "channel count {} exceeds {} bits",
            channels.len(),
            count_bits
        )
        .into());
    }
    bs.write_u32(channels.len() as u32, count_bits);
    for c in channels {
        bs.write_u32(*c as u32, chan_bits);
    }
    Ok(())
}

/// Read BAND_CLASS + CHANNEL_NUMBER pairs covering `total_body_bits`.
/// Spec phrasing: "LENGTH/2 occurrences"; with one pair = 16 bits, the
/// pair count is `total_body_bits / 16`.
fn read_band_class_channel_list(
    bs: &mut Bitstream,
    total_body_bits: usize,
) -> Result<Vec<BandClassChannel>, Error> {
    let pairs = total_body_bits / BITS_BC_CHAN_PAIR;
    let mut out = Vec::with_capacity(pairs);
    for _ in 0..pairs {
        let band_class = read_u8(bs, BITS_BAND_CLASS_5)?;
        let channel_number = read_u16(bs, BITS_CHAN_NUMBER_11)?;
        out.push(BandClassChannel {
            band_class,
            channel_number,
        });
    }
    Ok(out)
}

fn read_umb_acq_profile_list(
    bs: &mut Bitstream,
    total_body_bits: usize,
) -> Result<Vec<UmbAcqProfile>, Error> {
    let entries = total_body_bits / BITS_UMB_PROFILE_ENTRY;
    let mut out = Vec::with_capacity(entries);
    for _ in 0..entries {
        let umb_acq_profile = read_u8(bs, BITS_UMB_ACQ_PROFILE)?;
        let fft_size = read_u8(bs, BITS_UMB_FFT_SIZE)?;
        let cyclic_prefix_length = read_u8(bs, BITS_UMB_CYCLIC_PREFIX_LENGTH)?;
        let num_guard_subcarriers = read_u8(bs, BITS_UMB_NUM_GUARD_SUBCARRIERS)?;
        out.push(UmbAcqProfile {
            umb_acq_profile,
            fft_size,
            cyclic_prefix_length,
            num_guard_subcarriers,
        });
    }
    Ok(out)
}

/// Drain exactly `n` bits from `bs` and return them as 0/1 values.
fn drain_bits(bs: &mut Bitstream, n: usize) -> Result<Vec<u8>, Error> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if n > bs.len() {
        return Err("drain_bits: EOF".into());
    }
    let drained = bs.drain(0..n);
    Ok(drained.bits().to_vec())
}

fn bits_to_stream(bits: &[u8]) -> Bitstream {
    let mut bs = Bitstream::new();
    const BIT_MASK: u8 = 1;
    for &b in bits {
        bs.write_u8(b & BIT_MASK, 1);
    }
    bs
}

fn pack_bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    let bs = bits_to_stream(bits);
    bs.to_packed_bytes()
}

/// Write the first `bit_len` bits of `bytes` (MSB-first, byte-by-byte)
/// onto `bs`.
fn write_packed_bits(bs: &mut Bitstream, bytes: &[u8], bit_len: usize) {
    let mut left = bit_len;
    for &byte in bytes {
        let take = left.min(BITS_PER_OCTET);
        if take == 0 {
            break;
        }
        let val = byte >> (BITS_PER_OCTET - take);
        bs.write_u8(val, take);
        left -= take;
    }
}

// ---------------------------------------------------------------------------
// to_u8 helpers for enums shared with classic prl.rs
// ---------------------------------------------------------------------------

impl AbSelection {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::SystemA => 0b00,
            Self::SystemB => 0b01,
            Self::Reserved => 0b10,
            Self::EitherAOrB => 0b11,
        }
    }
}

impl StandardChannelSelection {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::Reserved => 0b00,
            Self::Primary => 0b01,
            Self::Secondary => 0b10,
            Self::PrimaryOrSecondary => 0b11,
        }
    }
}

impl PcsBlock {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::A => 0b000,
            Self::B => 0b001,
            Self::C => 0b010,
            Self::D => 0b011,
            Self::E => 0b100,
            Self::F => 0b101,
            Self::Reserved => 0b110,
            Self::AnyBlock => 0b111,
        }
    }
}

impl NidInclusion {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::AnyNid => 0b00,
            Self::SingleNid => 0b01,
            Self::PublicNid => 0b10,
            Self::Reserved => 0b11,
        }
    }
}
