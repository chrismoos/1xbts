//! Pure-function NAM block assembly for the OTASP Download Request.
//!
//! W fields are sourced from the HLR subscriber (full 15-digit IMSI,
//! phone number → ACCOLC), the cell's `bts_overhead` (SID, NID,
//! FIRSTCHP), and `OtaspConfig.nam_defaults` (MT flags). RO/RT
//! fields are echoed verbatim from the MS Configuration Response
//! read-back.
//!
//! IMSI uses `IMSI_M_CLASS = 0` (15-digit IMSI represented as a
//! 10-digit MIN in `IMSI_M_S`). For class-0 the spec requires
//! `IMSI_M_ADDR_NUM = 0`. `N_SID_NID = 1`. `EX` and `LOCAL_CONTROL`
//! are echoed from the read-back.

use cdma_otasp::imsi::{imsi_11_12_from_digits, imsi_s_from_imsi, mcc_from_digits};
use cdma_otasp::param::home_system_tag::HomeSystemTag;
use cdma_otasp::param::mdn::MobileDirectoryNumber;
use cdma_otasp::param::nam_cdma::NamCdma;
use cdma_otasp::param::nam_cdma_analog::{NamCdmaAnalog, SidNidPair};

use crate::config::{BtsOverheadConfig, OtaspConfig};

/// HLR-provided subscriber facts the session driver hands to NAM assembly.
#[derive(Debug, Clone)]
pub struct ResolvedSubscriberInput {
    /// 15-digit IMSI string.
    pub imsi: String,
    /// MDN (phone number) digits, no `+`/dashes.
    pub phone_number: String,
    /// PRL bytes to push to the MS during this session, resolved by the
    /// runtime: subscriber's override first, then the system default,
    /// `None` if neither is set.
    pub prl_bytes: Option<Vec<u8>>,
    /// PR_LIST_ID + SSPR_P_REV of the resolved PRL, included on events.
    pub prl_meta: Option<ResolvedPrlMeta>,
    /// Custom 6-digit Service Programming Code for this subscriber's
    /// device. `None` means the device uses the IS-95 default "000000".
    pub service_programming_code: Option<String>,
}

/// Lightweight metadata about the PRL chosen for this OTASP session.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedPrlMeta {
    pub pr_list_id: u16,
    pub sspr_p_rev: u8,
}

/// Read-back values pulled from the MS Configuration Response. Echoed verbatim
/// in the Download Request so vendor firmware that sanity-checks RO/RT fields
/// doesn't reject the block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamReadback {
    pub scm: u8,
    pub mob_p_rev: u8,
    pub max_sid_nid: u8,
    pub slotted_mode: bool,
    /// CDMA/Analog NAM only (BLOCK_ID 0x00). Echoed back on write.
    pub ex: bool,
    pub local_control: bool,
}

/// Assembled NAM blocks ready for `cdma_otasp::param::*::encode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledNam {
    pub cdma_analog: NamCdmaAnalog,
    pub mdn: MobileDirectoryNumber,
    pub cdma: NamCdma,
    pub home_system_tag: HomeSystemTag,
}

/// Errors produced by NAM assembly. All represent a programmer error in the
/// session driver or unrecoverable bad operator config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamAssemblyError {
    InvalidMcc(String),
    InvalidImsi1112(String),
    InvalidImsi(String),
    InvalidMdn(String),
    InvalidSystemTag(String),
}

impl std::fmt::Display for NamAssemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMcc(s) => write!(f, "invalid MCC: {s}"),
            Self::InvalidImsi1112(s) => write!(f, "invalid IMSI_11_12: {s}"),
            Self::InvalidImsi(s) => write!(f, "invalid IMSI: {s}"),
            Self::InvalidMdn(s) => write!(f, "invalid MDN: {s}"),
            Self::InvalidSystemTag(s) => write!(f, "invalid system tag: {s}"),
        }
    }
}

impl std::error::Error for NamAssemblyError {}

/// Build the four NAM-related parameter blocks. Pure function.
pub fn assemble_nam(
    hlr: &ResolvedSubscriberInput,
    bts_overhead: &BtsOverheadConfig,
    otasp_cfg: &OtaspConfig,
    ms_readback: &NamReadback,
) -> Result<AssembledNam, NamAssemblyError> {
    // IMSI is sourced entirely from the HLR subscriber's record: MCC
    // from digits 1-3, IMSI_11_12 from digits 4-5, IMSI_M_S from
    // digits 6-15. Operators keep the prefix aligned with the cell
    // overhead via the "Generate" button on the subscriber edit page;
    // an HLR record with an off-cell prefix is honored, which is what
    // the operator asked for.
    if hlr.imsi.len() != 15 || !hlr.imsi.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NamAssemblyError::InvalidImsi(hlr.imsi.clone()));
    }
    let mcc_m = mcc_from_digits(&hlr.imsi[0..3])
        .ok_or_else(|| NamAssemblyError::InvalidMcc(hlr.imsi[0..3].to_string()))?;
    let imsi_m_11_12 = imsi_11_12_from_digits(&hlr.imsi[3..5])
        .ok_or_else(|| NamAssemblyError::InvalidImsi1112(hlr.imsi[3..5].to_string()))?;
    let (s1, s2) = imsi_s_from_imsi(&hlr.imsi)
        .ok_or_else(|| NamAssemblyError::InvalidImsi(hlr.imsi.clone()))?;
    let imsi_m_s = ((s2 as u64) << 24) | (s1 as u64);

    let accolc = accolc_from_mdn(&hlr.phone_number)
        .ok_or_else(|| NamAssemblyError::InvalidMdn(hlr.phone_number.clone()))?;

    let home_sid = bts_overhead.sid;
    let nid = bts_overhead.nid;
    let firstchp = bts_overhead.paging_channel_number;

    let sid_nid_pairs = vec![SidNidPair { sid: home_sid, nid }];

    let cdma_analog = NamCdmaAnalog {
        firstchp,
        home_sid,
        ex: ms_readback.ex,
        scm: ms_readback.scm,
        mob_p_rev: ms_readback.mob_p_rev,
        imsi_m_class: false,
        imsi_m_addr_num: 0,
        mcc_m,
        imsi_m_11_12,
        imsi_m_s,
        accolc,
        local_control: ms_readback.local_control,
        mob_term_home: otasp_cfg.nam_defaults.mob_term_home,
        mob_term_for_sid: otasp_cfg.nam_defaults.mob_term_for_sid,
        mob_term_for_nid: otasp_cfg.nam_defaults.mob_term_for_nid,
        max_sid_nid: ms_readback.max_sid_nid,
        sid_nid_pairs: sid_nid_pairs.clone(),
    };

    let cdma = NamCdma {
        slotted_mode: ms_readback.slotted_mode,
        mob_p_rev: ms_readback.mob_p_rev,
        imsi_m_class: false,
        imsi_m_addr_num: 0,
        mcc_m,
        imsi_m_11_12,
        imsi_m_s,
        accolc,
        local_control: ms_readback.local_control,
        mob_term_home: otasp_cfg.nam_defaults.mob_term_home,
        mob_term_for_sid: otasp_cfg.nam_defaults.mob_term_for_sid,
        mob_term_for_nid: otasp_cfg.nam_defaults.mob_term_for_nid,
        max_sid_nid: ms_readback.max_sid_nid,
        sid_nid_pairs,
    };

    if !hlr.phone_number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NamAssemblyError::InvalidMdn(hlr.phone_number.clone()));
    }
    let mdn = MobileDirectoryNumber::new(&hlr.phone_number);

    let home_system_tag = HomeSystemTag::new_ascii(&otasp_cfg.system_tag.name)
        .map_err(|e| NamAssemblyError::InvalidSystemTag(e.to_string()))?;

    Ok(AssembledNam {
        cdma_analog,
        mdn,
        cdma,
        home_system_tag,
    })
}

/// `ACCOLC = (last MDN digit) mod 10` per IS-95 / phase-1 plan.
pub(crate) fn accolc_from_mdn(mdn: &str) -> Option<u8> {
    let last = mdn.chars().rev().find(|c| c.is_ascii_digit())?;
    let d = last.to_digit(10)? as u8;
    Some(d % 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MmsConfig, NamDefaultsConfig, OtaspWritesConfig, SystemTagConfig};

    fn cfg() -> OtaspConfig {
        OtaspConfig {
            enabled: true,
            feature_codes: vec!["*228".to_string()],
            spc_policy: "leave_default".to_string(),
            system_tag: SystemTagConfig {
                name: "1xBTS".to_string(),
                tag_p_rev: 1,
            },
            nam_defaults: NamDefaultsConfig {
                mob_term_home: true,
                mob_term_for_sid: true,
                mob_term_for_nid: false,
            },
            mms: MmsConfig::default(),
            writes: OtaspWritesConfig::default(),
        }
    }

    fn bts_overhead() -> BtsOverheadConfig {
        BtsOverheadConfig {
            mcc: "310".to_string(),
            imsi_11_12: "55".to_string(),
            sid: 22,
            nid: 1,
            paging_channel_number: 1,
        }
    }

    fn hlr() -> ResolvedSubscriberInput {
        ResolvedSubscriberInput {
            imsi: "310550123456789".to_string(),
            phone_number: "5551234567".to_string(),
            prl_bytes: None,
            prl_meta: None,
            service_programming_code: None,
        }
    }

    fn readback() -> NamReadback {
        NamReadback {
            scm: 0x52,
            mob_p_rev: 6,
            max_sid_nid: 4,
            slotted_mode: true,
            ex: false,
            local_control: false,
        }
    }

    #[test]
    fn assemble_produces_valid_blocks() {
        let nam = assemble_nam(&hlr(), &bts_overhead(), &cfg(), &readback()).unwrap();
        assert_eq!(nam.cdma_analog.mcc_m, 209); // "310" -> 209
        assert_eq!(nam.cdma_analog.imsi_m_11_12, 44); // "55" -> 44
        assert_eq!(nam.cdma_analog.home_sid, 22);
        assert_eq!(nam.cdma_analog.firstchp, 1);
        assert_eq!(nam.cdma_analog.scm, 0x52);
        assert_eq!(nam.cdma_analog.mob_p_rev, 6);
        assert!(!nam.cdma_analog.imsi_m_class);
        assert_eq!(nam.cdma_analog.imsi_m_addr_num, 0);
        assert!(!nam.cdma_analog.ex);
        assert!(!nam.cdma_analog.local_control);
        assert_eq!(nam.cdma_analog.sid_nid_pairs.len(), 1);
        assert!(nam.cdma_analog.mob_term_home);
        assert!(!nam.cdma_analog.mob_term_for_nid);
        // Last MDN digit = 7, ACCOLC = 7.
        assert_eq!(nam.cdma_analog.accolc, 7);
        assert_eq!(nam.cdma.accolc, 7);
        assert_eq!(nam.mdn.digits, "5551234567");
    }

    #[test]
    fn mdn_round_trips_through_codec() {
        let nam = assemble_nam(&hlr(), &bts_overhead(), &cfg(), &readback()).unwrap();
        let bytes = nam.mdn.encode().unwrap();
        let back = MobileDirectoryNumber::decode(&bytes).unwrap();
        assert_eq!(back, nam.mdn);
    }

    #[test]
    fn accolc_uses_last_mdn_digit() {
        assert_eq!(accolc_from_mdn("5551234567"), Some(7));
        assert_eq!(accolc_from_mdn("5551234560"), Some(0));
        assert_eq!(accolc_from_mdn("5551234561"), Some(1));
        assert_eq!(accolc_from_mdn(""), None);
    }

    #[test]
    fn assemble_rejects_non_digit_mdn() {
        let mut h = hlr();
        h.phone_number = "+1-555-1234".to_string();
        let err = assemble_nam(&h, &bts_overhead(), &cfg(), &readback()).unwrap_err();
        assert!(matches!(err, NamAssemblyError::InvalidMdn(_)));
    }

    #[test]
    fn assemble_rejects_overlong_system_tag() {
        let mut c = cfg();
        c.system_tag.name = "X".repeat(50);
        let err = assemble_nam(&hlr(), &bts_overhead(), &c, &readback()).unwrap_err();
        assert!(matches!(err, NamAssemblyError::InvalidSystemTag(_)));
    }
}
