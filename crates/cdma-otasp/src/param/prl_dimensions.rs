//! Preferred Roaming List Dimensions parameter block — C.S0016-D §3.5.3.1.
//!
//! Returned by the MS in an SSPR Configuration Response when the BS asks
//! for `BLOCK_ID = 0x00`. Tells the BS how big the PRL is and the count
//! of acquisition / system records, so the BS can pace the segmented
//! fetch and validate the assembled PRL.
//!
//! Extended Dimensions (`BLOCK_ID = 0x02`, §3.5.3.3) lives in the
//! `ExtendedPrlDimensions` type below. An MS that stores
//! `SSPR_P_REV >= 3` rejects a classic Dimensions request with result
//! code `0x23` ("PRL format mismatch") per §3.5.1.8, and the BS must
//! re-issue with Extended Dimensions.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_u8, read_u16};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrlDimensions {
    /// Maximum PRL size the MS can store, in octets.
    pub max_pr_list_size: u16,
    /// Size of the currently stored PRL, in octets. `0` means the MS has
    /// no PRL programmed.
    pub cur_pr_list_size: u16,
    /// PRL identifier as assigned by the operator that wrote it.
    pub pr_list_id: u16,
    /// Number of acquisition records inside the PRL's ACQ_TABLE.
    pub num_acq_recs: u16,
    /// Number of system records inside the PRL's SYS_TABLE.
    pub num_sys_recs: u16,
}

impl PrlDimensions {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.num_acq_recs >= (1 << 9) {
            return Err("NUM_ACQ_RECS exceeds 9 bits".into());
        }
        if self.num_sys_recs >= (1 << 14) {
            return Err("NUM_SYS_RECS exceeds 14 bits".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u32(self.max_pr_list_size as u32, 16);
        bs.write_u32(self.cur_pr_list_size as u32, 16);
        bs.write_u32(self.pr_list_id as u32, 16);
        bs.write_u8(0, 1); // RESERVED
        bs.write_u32(self.num_acq_recs as u32, 9);
        bs.write_u32(self.num_sys_recs as u32, 14);
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(param_data: &[u8]) -> Result<Self, Error> {
        if param_data.len() < 9 {
            return Err("PrlDimensions too short".into());
        }
        let mut bs = from_bytes(param_data);
        let max_pr_list_size = read_u16(&mut bs, 16)?;
        let cur_pr_list_size = read_u16(&mut bs, 16)?;
        let pr_list_id = read_u16(&mut bs, 16)?;
        let _reserved = read_u16(&mut bs, 1)?;
        let num_acq_recs = read_u16(&mut bs, 9)?;
        let num_sys_recs = read_u16(&mut bs, 14)?;
        Ok(Self {
            max_pr_list_size,
            cur_pr_list_size,
            pr_list_id,
            num_acq_recs,
            num_sys_recs,
        })
    }
}

/// Extended PRL Dimensions parameter block — C.S0016-D §3.5.3.3.
///
/// The `CUR_SSPR_P_REV`-specific tail is decoded into either the
/// classic record counts (`SSPR_P_REV = 1`) or the extended counts
/// (`SSPR_P_REV = 3`). Both share the leading
/// `MAX_PR_LIST_SIZE / CUR_PR_LIST_SIZE / PR_LIST_ID / CUR_SSPR_P_REV`
/// header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedPrlDimensions {
    pub max_pr_list_size: u16,
    pub cur_pr_list_size: u16,
    pub pr_list_id: u16,
    /// SSPR protocol revision of the stored PRL: 1 (classic) or 3
    /// (extended). Other values are reserved per §3.5.3.3.
    pub cur_sspr_p_rev: u8,
    pub counts: ExtendedDimsCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendedDimsCounts {
    /// `CUR_SSPR_P_REV = 1` — classic tail.
    Classic {
        num_acq_recs: u16,
        num_sys_recs: u16,
    },
    /// `CUR_SSPR_P_REV = 3` — extended tail.
    Extended {
        num_acq_recs: u16,
        num_common_subnet_recs: u16,
        num_ext_sys_recs: u16,
    },
}

impl ExtendedPrlDimensions {
    pub fn decode(param_data: &[u8]) -> Result<Self, Error> {
        // 16+16+16+8 = 56 bits = 7 bytes of header, then 3 bytes of tail.
        if param_data.len() < 10 {
            return Err("ExtendedPrlDimensions too short".into());
        }
        let mut bs = from_bytes(param_data);
        let max_pr_list_size = read_u16(&mut bs, 16)?;
        let cur_pr_list_size = read_u16(&mut bs, 16)?;
        let pr_list_id = read_u16(&mut bs, 16)?;
        let cur_sspr_p_rev = read_u8(&mut bs, 8)?;
        let counts = match cur_sspr_p_rev {
            1 => {
                let _reserved = read_u16(&mut bs, 1)?;
                let num_acq_recs = read_u16(&mut bs, 9)?;
                let num_sys_recs = read_u16(&mut bs, 14)?;
                ExtendedDimsCounts::Classic {
                    num_acq_recs,
                    num_sys_recs,
                }
            }
            3 => {
                let num_acq_recs = read_u16(&mut bs, 9)?;
                let num_common_subnet_recs = read_u16(&mut bs, 9)?;
                let num_ext_sys_recs = read_u16(&mut bs, 14)?;
                ExtendedDimsCounts::Extended {
                    num_acq_recs,
                    num_common_subnet_recs,
                    num_ext_sys_recs,
                }
            }
            other => {
                return Err(
                    format!("ExtendedPrlDimensions: unsupported CUR_SSPR_P_REV {other}").into(),
                );
            }
        };
        Ok(Self {
            max_pr_list_size,
            cur_pr_list_size,
            pr_list_id,
            cur_sspr_p_rev,
            counts,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bs = Bitstream::new();
        bs.write_u32(self.max_pr_list_size as u32, 16);
        bs.write_u32(self.cur_pr_list_size as u32, 16);
        bs.write_u32(self.pr_list_id as u32, 16);
        bs.write_u8(self.cur_sspr_p_rev, 8);
        match self.counts {
            ExtendedDimsCounts::Classic {
                num_acq_recs,
                num_sys_recs,
            } => {
                bs.write_u8(0, 1); // RESERVED
                bs.write_u32(num_acq_recs as u32, 9);
                bs.write_u32(num_sys_recs as u32, 14);
            }
            ExtendedDimsCounts::Extended {
                num_acq_recs,
                num_common_subnet_recs,
                num_ext_sys_recs,
            } => {
                bs.write_u32(num_acq_recs as u32, 9);
                bs.write_u32(num_common_subnet_recs as u32, 9);
                bs.write_u32(num_ext_sys_recs as u32, 14);
            }
        }
        Ok(bs.to_packed_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_round_trip() {
        let d = PrlDimensions {
            max_pr_list_size: 4096,
            cur_pr_list_size: 312,
            pr_list_id: 0x1234,
            num_acq_recs: 5,
            num_sys_recs: 12,
        };
        let bytes = d.encode().unwrap();
        assert_eq!(bytes.len(), 9);
        assert_eq!(PrlDimensions::decode(&bytes).unwrap(), d);
    }

    #[test]
    fn extended_dimensions_classic_tail_round_trip() {
        let d = ExtendedPrlDimensions {
            max_pr_list_size: 8192,
            cur_pr_list_size: 1024,
            pr_list_id: 0xABCD,
            cur_sspr_p_rev: 1,
            counts: ExtendedDimsCounts::Classic {
                num_acq_recs: 7,
                num_sys_recs: 800,
            },
        };
        let bytes = d.encode().unwrap();
        assert_eq!(ExtendedPrlDimensions::decode(&bytes).unwrap(), d);
    }

    #[test]
    fn extended_dimensions_extended_tail_round_trip() {
        let d = ExtendedPrlDimensions {
            max_pr_list_size: 16384,
            cur_pr_list_size: 6882,
            pr_list_id: 51611,
            cur_sspr_p_rev: 3,
            counts: ExtendedDimsCounts::Extended {
                num_acq_recs: 25,
                num_common_subnet_recs: 0,
                num_ext_sys_recs: 1200,
            },
        };
        let bytes = d.encode().unwrap();
        assert_eq!(ExtendedPrlDimensions::decode(&bytes).unwrap(), d);
    }

    #[test]
    fn extended_dimensions_rejects_unsupported_rev() {
        let mut bytes = vec![
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // sizes + id
            0x02, // CUR_SSPR_P_REV = 2 (not supported by spec text)
            0x00, 0x00, 0x00,
        ];
        bytes.extend_from_slice(&[0u8; 4]);
        let err = ExtendedPrlDimensions::decode(&bytes).unwrap_err();
        assert!(err.to_string().contains("CUR_SSPR_P_REV"));
    }

    #[test]
    fn dimensions_field_layout_pin() {
        let d = PrlDimensions {
            max_pr_list_size: 0x0102,
            cur_pr_list_size: 0x0304,
            pr_list_id: 0x0506,
            num_acq_recs: 0x0FF, // 9-bit max minus a bit
            num_sys_recs: 0x3FFE,
        };
        let bytes = d.encode().unwrap();
        // 16+16+16 = 48 bits = 6 bytes for the IDs; then 1+9+14 = 24 bits = 3 bytes more.
        assert_eq!(bytes.len(), 9);
        let back = PrlDimensions::decode(&bytes).unwrap();
        assert_eq!(back, d);
    }
}
