use crate::bits::Bitstream;
use serde::{Deserialize, Serialize};

use crate::mac::ChannelType;

use super::{DataRequest, MessageControlStatusBlock, message_types::MessageId};

mod serde_mcc {
    use crate::paging::{mcc_from_digits, mcc_to_digits};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u16, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match mcc_to_digits(*value) {
            Some(s) => serializer.serialize_str(&s),
            None => serializer.serialize_u16(*value),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u16, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s == "wildcard" {
            return Ok(0x03ff);
        }
        mcc_from_digits(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid MCC \"{s}\": expected 3-digit string like \"310\" or \"wildcard\""
            ))
        })
    }
}

mod serde_imsi_11_12 {
    use crate::paging::{imsi_11_12_from_digits, imsi_11_12_to_digits};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u8, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match imsi_11_12_to_digits(*value) {
            Some(s) => serializer.serialize_str(&s),
            None if *value == 0x7f => serializer.serialize_str("wildcard"),
            None => serializer.serialize_u8(*value),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u8, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s == "wildcard" {
            return Ok(0x7f);
        }
        imsi_11_12_from_digits(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid IMSI_11_12 \"{s}\": expected 2-digit string like \"55\" or \"wildcard\""
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Forward-link directed-PDU addressing (C.S0004-E 3.1.2.2.1.3.1-1)
// ---------------------------------------------------------------------------

/// Forward-link directed-PDU address types per C.S0004-E 3.1.2.2.1.3.1-1.
/// For directed PDUs, the BS shall use ESN, IMSI, or TMSI (C.S0004-E 3.1.2.2.2).
#[derive(Clone, Debug, PartialEq)]
pub enum MsAddress {
    /// ADDR_TYPE = 000, IMSI_S: IMSI_M_S1(24) + IMSI_M_S2(10) + RESERVED(6)
    ImsiS { imsi_m_s1: u32, imsi_m_s2: u16 },
    /// ADDR_TYPE = 001, ESN(32)
    Esn(u32),
    /// ADDR_TYPE = 010, IMSI class 0.
    ///
    /// Always stores the fully resolved MCC and IMSI_11_12 values — never
    /// the compressed OTA form. OTA compression (selecting IMSI_CLASS_0_TYPE
    /// per C.S0004-E 3.1.2.2.1.3.3) happens at encoding time in `write_to`,
    /// which compares these values against the current overhead parameters.
    ImsiClass0 {
        imsi_m_s1: u32,
        imsi_m_s2: u16,
        mcc: u16,
        imsi_11_12: u8,
    },
}

impl MsAddress {
    /// Determine the IMSI_CLASS_0_TYPE and ADDR_LEN for OTA encoding by
    /// comparing resolved values against current overhead per C.S0004-E
    /// Table 2.1.1.3.1.1-2.
    fn imsi_class_0_ota_type_and_len(
        mcc: u16,
        imsi_11_12: u8,
        overhead_mcc: u16,
        overhead_imsi_11_12: u8,
    ) -> (u8, u8) {
        let mcc_implied = overhead_mcc == 0x03ff || overhead_mcc == mcc;
        let imsi_11_12_implied = overhead_imsi_11_12 == 0x7f || overhead_imsi_11_12 == imsi_11_12;
        match (mcc_implied, imsi_11_12_implied) {
            (true, true) => (0b00, 5),
            (true, false) => (0b01, 6),
            (false, true) => (0b10, 6),
            (false, false) => (0b11, 7),
        }
    }

    /// 3-bit ADDR_TYPE value.
    pub fn addr_type(&self) -> u8 {
        match self {
            MsAddress::ImsiS { .. } => 0b000,
            MsAddress::Esn(_) => 0b001,
            MsAddress::ImsiClass0 { .. } => 0b010,
        }
    }

    /// ADDR_LEN in octets for OTA encoding. For IMSI class 0, overhead
    /// parameters determine OTA compression (which fields are included).
    pub fn addr_len(&self, overhead_mcc: u16, overhead_imsi_11_12: u8) -> u8 {
        match self {
            MsAddress::ImsiS { .. } => 5,
            MsAddress::Esn(_) => 4,
            MsAddress::ImsiClass0 {
                mcc, imsi_11_12, ..
            } => {
                Self::imsi_class_0_ota_type_and_len(
                    *mcc,
                    *imsi_11_12,
                    overhead_mcc,
                    overhead_imsi_11_12,
                )
                .1
            }
        }
    }

    /// Write ADDRESS body to bitstream, compressing IMSI class 0 fields
    /// against current overhead per C.S0004-E 3.1.2.2.1.3.3.
    fn write_address_body(&self, bs: &mut Bitstream, overhead_mcc: u16, overhead_imsi_11_12: u8) {
        match self {
            MsAddress::ImsiS {
                imsi_m_s1,
                imsi_m_s2,
            } => {
                bs.write_u32(*imsi_m_s1, 24);
                bs.write_u32(*imsi_m_s2 as u32, 10);
                bs.write_u8(0, 6); // RESERVED
            }
            MsAddress::Esn(esn) => {
                bs.write_u32(*esn, 32);
            }
            MsAddress::ImsiClass0 {
                imsi_m_s1,
                imsi_m_s2,
                mcc,
                imsi_11_12,
            } => {
                let (imsi_class_0_type, _) = Self::imsi_class_0_ota_type_and_len(
                    *mcc,
                    *imsi_11_12,
                    overhead_mcc,
                    overhead_imsi_11_12,
                );
                let mcc_implied = overhead_mcc == 0x03ff || overhead_mcc == *mcc;
                let imsi_11_12_implied =
                    overhead_imsi_11_12 == 0x7f || overhead_imsi_11_12 == *imsi_11_12;

                bs.write_u8(0, 1); // IMSI_CLASS = class 0
                bs.write_u8(imsi_class_0_type, 2);
                match (mcc_implied, imsi_11_12_implied) {
                    (true, true) => {
                        bs.write_u8(0, 3); // RESERVED
                    }
                    (true, false) => {
                        bs.write_u8(0, 4); // RESERVED
                        bs.write_u8(*imsi_11_12, 7);
                    }
                    (false, true) => {
                        bs.write_u8(0, 1); // RESERVED
                        bs.write_u32(*mcc as u32, 10);
                    }
                    (false, false) => {
                        bs.write_u8(0, 2); // RESERVED
                        bs.write_u32(*mcc as u32, 10);
                        bs.write_u8(*imsi_11_12, 7);
                    }
                }
                bs.write_u32(*imsi_m_s2 as u32, 10); // IMSI_S2
                bs.write_u32(*imsi_m_s1, 24); // IMSI_S1
            }
        }
    }

    /// Write ADDR_TYPE(3) + ADDR_LEN(4) + ADDRESS to the bitstream,
    /// compressing IMSI class 0 against current overhead.
    pub fn write_to(&self, bs: &mut Bitstream, overhead_mcc: u16, overhead_imsi_11_12: u8) {
        bs.write_u8(self.addr_type(), 3);
        bs.write_u8(self.addr_len(overhead_mcc, overhead_imsi_11_12), 4);
        self.write_address_body(bs, overhead_mcc, overhead_imsi_11_12);
    }

    /// Canonical identity key for MSG_SEQ tracking: (addr_type, identity bytes).
    ///
    /// For IMSI class 0, always includes all resolved fields (MCC, IMSI_11_12,
    /// S2, S1) regardless of overhead — the identity is stable across overhead
    /// changes.
    pub fn tracking_key(&self) -> (u8, Vec<u8>) {
        let mut body = Bitstream::new();
        match self {
            MsAddress::ImsiS {
                imsi_m_s1,
                imsi_m_s2,
            } => {
                body.write_u32(*imsi_m_s1, 24);
                body.write_u32(*imsi_m_s2 as u32, 10);
            }
            MsAddress::Esn(esn) => {
                body.write_u32(*esn, 32);
            }
            MsAddress::ImsiClass0 {
                imsi_m_s1,
                imsi_m_s2,
                mcc,
                imsi_11_12,
            } => {
                body.write_u32(*mcc as u32, 10);
                body.write_u8(*imsi_11_12, 7);
                body.write_u32(*imsi_m_s2 as u32, 10);
                body.write_u32(*imsi_m_s1, 24);
            }
        }
        (self.addr_type(), bitstream_to_bytes(&body))
    }

    /// Resolve to the full IMSI class-0 form using overhead parameters.
    ///
    /// `ImsiS` is expanded to `ImsiClass0` by filling in the overhead MCC and
    /// IMSI_11_12 (per C.S0005-E 2.6.2.2.5, omitted fields equal overhead).
    /// `ImsiClass0` and `Esn` are returned unchanged.
    pub fn resolve_full(&self, overhead_mcc: u16, overhead_imsi_11_12: u8) -> MsAddress {
        match self {
            MsAddress::ImsiS {
                imsi_m_s1,
                imsi_m_s2,
            } => MsAddress::ImsiClass0 {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: *imsi_m_s2,
                mcc: overhead_mcc,
                imsi_11_12: overhead_imsi_11_12,
            },
            other => other.clone(),
        }
    }

    /// Identity key for ACK matching: always uses the full IMSI class-0 form.
    ///
    /// Unlike `tracking_key()` (which preserves the OTA address variant),
    /// this key resolves `ImsiS` → `ImsiClass0` using overhead so that
    /// paging-side (compressed) and access-channel-side (full) addresses
    /// for the same mobile produce identical keys.
    pub fn ack_identity_key(&self, overhead_mcc: u16, overhead_imsi_11_12: u8) -> (u8, Vec<u8>) {
        self.resolve_full(overhead_mcc, overhead_imsi_11_12)
            .tracking_key()
    }
}

/// Build an IMSI class-0 forward-link address with fully resolved identity.
///
/// All inputs must be fully resolved — the caller is responsible for
/// reconstructing `ms_mcc` / `ms_imsi_11_12` from overhead when the
/// mobile omitted them (per C.S0005-E 2.6.2.2.5: `None` in the access
/// message means "equals overhead," not "unknown").
///
/// The returned `MsAddress` stores the actual MCC and IMSI_11_12 values.
/// OTA compression (selecting IMSI_CLASS_0_TYPE per C.S0004-E Table
/// 2.1.1.3.1.1-2) is deferred to `write_to()` at encoding time.
pub fn select_imsi_class0_forward_address(
    imsi_m_s1: u32,
    imsi_m_s2: u16,
    ms_mcc: u16,
    ms_imsi_11_12: u8,
) -> MsAddress {
    MsAddress::ImsiClass0 {
        imsi_m_s1,
        imsi_m_s2,
        mcc: ms_mcc,
        imsi_11_12: ms_imsi_11_12,
    }
}

/// Resolve a forward-link `MsAddress` from access probe addressing fields.
///
/// Follows the same fallback chain as the BSC's `extract_fwd_address`:
/// class-0 IMSI → ESN → IMSI_S.
pub fn forward_address_from_access_fields(
    imsi_class: Option<u8>,
    imsi_m_s1: Option<u32>,
    imsi_m_s2: Option<u16>,
    imsi_mcc: Option<u16>,
    imsi_11_12: Option<u8>,
    esn: Option<u32>,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> Option<MsAddress> {
    if imsi_class == Some(0) {
        if let (Some(s1), Some(s2)) = (imsi_m_s1, imsi_m_s2) {
            let resolved_mcc = imsi_mcc.unwrap_or(overhead_mcc);
            let resolved_imsi_11_12 = imsi_11_12.unwrap_or(overhead_imsi_11_12);
            return Some(select_imsi_class0_forward_address(
                s1,
                s2,
                resolved_mcc,
                resolved_imsi_11_12,
            ));
        }
    }

    if let Some(esn) = esn {
        Some(MsAddress::Esn(esn))
    } else if let (Some(s1), Some(s2)) = (imsi_m_s1, imsi_m_s2) {
        Some(MsAddress::ImsiS {
            imsi_m_s1: s1,
            imsi_m_s2: s2,
        })
    } else {
        None
    }
}

/// GPM page-record address -- separate scheme from ADDR_TYPE.
/// Per C.S0004-E 3.1.2.2.1.1, General Page Message records use PAGE_CLASS and
/// PAGE_SUBCLASS to identify the target MS.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MsPageAddress {
    /// Class 0 page record using IMSI_S (34 bits).
    /// Optional mcc/imsi_11_12 allow higher subclass variants.
    ImsiS {
        imsi_m_s1: u32,
        imsi_m_s2: u16,
        mcc: Option<u16>,
        imsi_11_12: Option<u8>,
    },
    /// Class 1 page record using ESN.
    Esn(u32),
}

impl From<&MsAddress> for MsPageAddress {
    fn from(addr: &MsAddress) -> Self {
        match addr {
            MsAddress::ImsiS {
                imsi_m_s1,
                imsi_m_s2,
            } => MsPageAddress::ImsiS {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: *imsi_m_s2,
                mcc: None,
                imsi_11_12: None,
            },
            MsAddress::ImsiClass0 {
                imsi_m_s1,
                imsi_m_s2,
                mcc,
                imsi_11_12,
            } => MsPageAddress::ImsiS {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: *imsi_m_s2,
                mcc: Some(*mcc),
                imsi_11_12: Some(*imsi_11_12),
            },
            MsAddress::Esn(esn) => MsPageAddress::Esn(*esn),
        }
    }
}

impl GeneralPageRecord {
    /// Derive the page address from this record for dedup/cancellation.
    pub fn page_address(&self) -> Option<MsPageAddress> {
        match self {
            GeneralPageRecord::Class0 {
                imsi_s,
                imsi_m_s1,
                imsi_m_s2,
                mcc,
                imsi_11_12,
                ..
            } => {
                if let (Some(s1), Some(s2)) = (imsi_m_s1, imsi_m_s2) {
                    Some(MsPageAddress::ImsiS {
                        imsi_m_s1: *s1,
                        imsi_m_s2: *s2,
                        mcc: *mcc,
                        imsi_11_12: *imsi_11_12,
                    })
                } else if let Some(imsi_s) = imsi_s {
                    let s1 = (*imsi_s & 0xFF_FFFF) as u32;
                    let s2 = ((*imsi_s >> 24) & 0x3FF) as u16;
                    Some(MsPageAddress::ImsiS {
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        mcc: *mcc,
                        imsi_11_12: *imsi_11_12,
                    })
                } else {
                    None
                }
            }
            GeneralPageRecord::Class1 { esn, .. } => Some(MsPageAddress::Esn(*esn)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Forward Data Burst Message (C.S0005-E 3.7.2.3.2.9)
// ---------------------------------------------------------------------------

/// Forward Data Burst Message Layer 3 body for f-csch (paging channel).
#[derive(Clone, Debug)]
pub struct ForwardDataBurstMessage {
    pub msg_number: u8,
    pub burst_type: u8,
    pub num_msgs: u8,
    pub fields: Vec<u8>,
}

impl ForwardDataBurstMessage {
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u8(self.msg_number, 8);
        bs.write_u8(self.burst_type, 6);
        bs.write_u8(self.num_msgs, 8);
        bs.write_u8(self.fields.len() as u8, 8);
        for &b in &self.fields {
            bs.write_u8(b, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, String> {
        let r = |bs: &mut Bitstream, n| bs.read_bits(n).map_err(|e| format!("DataBurst: {e}"));
        let msg_number = r(bs, 8)? as u8;
        let burst_type = r(bs, 6)? as u8;
        let num_msgs = r(bs, 8)? as u8;
        let num_fields = r(bs, 8)? as usize;
        let mut fields = Vec::with_capacity(num_fields);
        for _ in 0..num_fields {
            fields.push(r(bs, 8)? as u8);
        }
        Ok(Self {
            msg_number,
            burst_type,
            num_msgs,
            fields,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PagingMessageKind {
    SystemParameters,
    AccessParameters,
    NeighborList,
    CdmaChannelList,
    ExtendedSystemParameters,
    GeneralPage,
    Order,
    AlternativeTechnologiesInformation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PagingMessageDefaults {
    pub schedule: Vec<PagingMessageKind>,
    pub system_parameters: SystemParametersDefaults,
    pub access_parameters: AccessParametersDefaults,
    pub neighbor_list: NeighborListDefaults,
    pub cdma_channel_list: CdmaChannelListDefaults,
    pub extended_system_parameters: ExtendedSystemParametersDefaults,
    pub general_page: GeneralPageDefaults,
    pub order: OrderDefaults,
}

impl Default for PagingMessageDefaults {
    fn default() -> Self {
        Self {
            // GPMs are emitted structurally as the first message in every
            // 80ms paging slot (per C.S0005-E §3.6.2.3), so they are not
            // part of the overhead rotation schedule.
            schedule: vec![
                PagingMessageKind::SystemParameters,
                PagingMessageKind::NeighborList,
                PagingMessageKind::CdmaChannelList,
                PagingMessageKind::ExtendedSystemParameters,
                PagingMessageKind::AccessParameters,
                PagingMessageKind::Order,
            ],
            system_parameters: SystemParametersDefaults::default(),
            access_parameters: AccessParametersDefaults::default(),
            neighbor_list: NeighborListDefaults::default(),
            cdma_channel_list: CdmaChannelListDefaults::default(),
            extended_system_parameters: ExtendedSystemParametersDefaults::default(),
            general_page: GeneralPageDefaults::default(),
            order: OrderDefaults::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemParametersDefaults {
    pub mult_sids: bool,
    pub mult_nids: bool,
    pub base_class: u8,
    pub home_reg: bool,
    pub for_sid_reg: bool,
    pub for_nid_reg: bool,
    pub power_down_reg: bool,
    pub reg_prd: u8,
    pub base_lat: u32,
    pub base_long: u32,
    pub reg_dist: u16,
    pub srch_win_a: u8,
    pub srch_win_n: u8,
    pub srch_win_r: u8,
    pub nghbr_max_age: u8,
    pub pwr_rep_thresh: u8,
    pub pwr_rep_frames: u8,
    pub pwr_thresh_enable: bool,
    pub pwr_period_enable: bool,
    pub pwr_rep_delay: u8,
    pub rescan: bool,
    pub t_add: u8,
    pub t_drop: u8,
    pub t_comp: u8,
    pub t_tdrop: u8,
    pub ext_sys_parameter: bool,
    pub ext_nghbr_lst: bool,
    pub gen_nghbr_lst: bool,
    pub global_redirect: bool,
    pub pri_nghbr_lst: bool,
    pub user_zone_id: bool,
    pub ext_global_redirect: bool,
    pub ext_chan_lst: bool,
}

impl Default for SystemParametersDefaults {
    fn default() -> Self {
        Self {
            mult_sids: false,
            mult_nids: false,
            base_class: 0,
            home_reg: true,
            for_sid_reg: true,
            for_nid_reg: true,
            power_down_reg: false,
            reg_prd: 0,
            base_lat: 0,
            base_long: 0,
            reg_dist: 0,
            srch_win_a: 8,
            srch_win_n: 10,
            srch_win_r: 10,
            nghbr_max_age: 0,
            pwr_rep_thresh: 0,
            pwr_rep_frames: 12,
            pwr_thresh_enable: false,
            pwr_period_enable: false,
            pwr_rep_delay: 0,
            rescan: false,
            t_add: 28,
            t_drop: 32,
            t_comp: 5,
            t_tdrop: 3,
            ext_sys_parameter: true,
            ext_nghbr_lst: false,
            gen_nghbr_lst: false,
            global_redirect: false,
            pri_nghbr_lst: false,
            user_zone_id: false,
            ext_global_redirect: false,
            ext_chan_lst: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessParametersDefaults {
    pub acc_chan: u8,
    /// NOM_PWR open-loop offset in dB. Wire is 4-bit signed (-8..+7 dB,
    /// Band Class 0). See `AccessParametersMessage::nom_pwr` for the full
    /// open-loop reverse TX power formula.
    pub nom_pwr: i8,
    /// INIT_PWR open-loop offset in dB. Wire is 5-bit signed (-16..+15 dB).
    pub init_pwr: i8,
    pub pwr_step: u8,
    pub num_step: u8,
    pub max_cap_sz: u8,
    pub pam_sz: u8,
    pub psist_0_9: u8,
    pub psist_10: u8,
    pub psist_11: u8,
    pub psist_12: u8,
    pub psist_13: u8,
    pub psist_14: u8,
    pub psist_15: u8,
    pub msg_psist: u8,
    pub reg_psist: u8,
    pub probe_pn_ran: u8,
    pub acc_tmo: u8,
    pub probe_bkoff: u8,
    pub bkoff: u8,
    pub max_req_seq: u8,
    pub max_rsp_seq: u8,
    pub auth: u8,
    pub rand: u32,
    pub nom_pwr_ext: u8,
    pub psist_emg_incl: bool,
    pub psist_emg: u8,
    pub acct_incl: bool,
    pub acct_incl_emg: bool,
    pub acct_aoc_bitmap_incl: bool,
    pub acct_so_records: Vec<AcctServiceOptionRecord>,
    pub acct_so_grp_records: Vec<AcctServiceOptionGroupRecord>,
}

impl Default for AccessParametersDefaults {
    fn default() -> Self {
        Self {
            acc_chan: 0,
            nom_pwr: 0,
            init_pwr: 0,
            pwr_step: 1,
            num_step: 15,
            max_cap_sz: 7,
            pam_sz: 15,
            psist_0_9: 0,
            psist_10: 0,
            psist_11: 0,
            psist_12: 0,
            psist_13: 0,
            psist_14: 0,
            psist_15: 0,
            msg_psist: 0,
            reg_psist: 0,
            probe_pn_ran: 9,
            acc_tmo: 3,
            probe_bkoff: 0,
            bkoff: 1,
            max_req_seq: 15,
            max_rsp_seq: 15,
            auth: 0,
            rand: 0,
            nom_pwr_ext: 0,
            psist_emg_incl: false,
            psist_emg: 0,
            acct_incl: false,
            acct_incl_emg: false,
            acct_aoc_bitmap_incl: false,
            acct_so_records: Vec::new(),
            acct_so_grp_records: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AcctServiceOptionRecord {
    pub aoc_bitmap: u8,
    pub service_option: u16,
}

impl Default for AcctServiceOptionRecord {
    fn default() -> Self {
        Self {
            aoc_bitmap: 0,
            service_option: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AcctServiceOptionGroupRecord {
    pub aoc_bitmap: u8,
    pub service_option_group: u8,
}

impl Default for AcctServiceOptionGroupRecord {
    fn default() -> Self {
        Self {
            aoc_bitmap: 0,
            service_option_group: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NeighborListDefaults {
    pub pilot_inc: u8,
    pub neighbors: Vec<u16>,
}

impl Default for NeighborListDefaults {
    fn default() -> Self {
        Self {
            pilot_inc: 0,
            neighbors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CdmaChannelListDefaults {
    pub channels: Vec<u16>,
}

impl Default for CdmaChannelListDefaults {
    fn default() -> Self {
        Self {
            channels: vec![384],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtendedSystemParametersDefaults {
    pub delete_for_tmsi: bool,
    pub use_tmsi: bool,
    pub pref_msid_type: u8,
    pub ext_pref_msid_type: Option<u8>,
    pub meid_reqd: Option<bool>,
    #[serde(
        serialize_with = "serde_mcc::serialize",
        deserialize_with = "serde_mcc::deserialize"
    )]
    pub mcc: u16,
    #[serde(
        serialize_with = "serde_imsi_11_12::serialize",
        deserialize_with = "serde_imsi_11_12::deserialize"
    )]
    pub imsi_11_12: u8,
    pub tmsi_zone: Vec<u8>,
    pub bcast_index: u8,
    pub imsi_t_supported: bool,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub soft_slope: u8,
    pub add_intercept: u8,
    pub drop_intercept: u8,
    pub packet_zone_id: u8,
    pub max_num_alt_so: u8,
    pub reselect_included: bool,
    pub ec_thresh: u8,
    pub ec_io_thresh: u8,
    pub pilot_report: bool,
    pub nghbr_set_entry_info: bool,
    pub acc_ent_ho_order: bool,
    pub nghbr_set_access_info: bool,
    pub access_ho: bool,
    pub access_ho_msg_rsp: bool,
    pub access_probe_ho: bool,
    pub acc_ho_list_upd: bool,
    pub acc_probe_ho_other_msg: bool,
    pub max_num_probe_ho: u8,
    pub nghbr_set_size: u8,
    pub access_entry_ho: Vec<bool>,
    pub access_ho_allowed: Vec<bool>,
    pub broadcast_gps_asst: bool,
    pub qpch_supported: bool,
    pub num_qpch: u8,
    pub qpch_rate: u8,
    pub qpch_power_level_page: u8,
    pub qpch_cci_supported: bool,
    pub qpch_power_level_config: u8,
    pub sdb_supported: bool,
    pub rlgain_traffic_pilot: u8,
    pub rev_pwr_cntl_delay_incl: bool,
    pub rev_pwr_cntl_delay: u8,
    pub auto_msg_supported: bool,
    pub auto_msg_interval: u8,
    pub mob_qos: bool,
    pub enc_supported: bool,
    pub sig_encrypt_sup: u8,
    pub ui_encrypt_sup: u8,
    pub use_sync_id: bool,
    pub cs_supported: bool,
    pub bcch_supported: bool,
    pub ms_init_pos_loc_sup_ind: bool,
    pub pilot_info_req_supported: bool,
}

impl Default for ExtendedSystemParametersDefaults {
    fn default() -> Self {
        Self {
            delete_for_tmsi: false,
            // Prefer full IMSI+ESN on access so the network can retain richer identity state.
            use_tmsi: false,
            pref_msid_type: 3,
            ext_pref_msid_type: Some(1),
            meid_reqd: Some(true),
            mcc: 0x03ff,
            imsi_11_12: 0x7f,
            tmsi_zone: vec![0],
            bcast_index: 0,
            imsi_t_supported: false,
            p_rev: 11,
            min_p_rev: 3,
            soft_slope: 0,
            add_intercept: 0,
            drop_intercept: 0,
            packet_zone_id: 1,
            max_num_alt_so: 7,
            reselect_included: false,
            ec_thresh: 0,
            ec_io_thresh: 0,
            pilot_report: false,
            nghbr_set_entry_info: false,
            acc_ent_ho_order: false,
            nghbr_set_access_info: false,
            access_ho: false,
            access_ho_msg_rsp: false,
            access_probe_ho: false,
            acc_ho_list_upd: false,
            acc_probe_ho_other_msg: false,
            max_num_probe_ho: 0,
            nghbr_set_size: 0,
            access_entry_ho: Vec::new(),
            access_ho_allowed: Vec::new(),
            broadcast_gps_asst: false,
            qpch_supported: false,
            num_qpch: 0,
            qpch_rate: 0,
            qpch_power_level_page: 0,
            qpch_cci_supported: false,
            qpch_power_level_config: 0,
            sdb_supported: false,
            rlgain_traffic_pilot: 0,
            rev_pwr_cntl_delay_incl: false,
            rev_pwr_cntl_delay: 0,
            auto_msg_supported: false,
            auto_msg_interval: 0,
            mob_qos: false,
            enc_supported: false,
            sig_encrypt_sup: 0,
            ui_encrypt_sup: 0,
            use_sync_id: false,
            cs_supported: false,
            bcch_supported: false,
            ms_init_pos_loc_sup_ind: false,
            pilot_info_req_supported: false,
        }
    }
}

impl ExtendedSystemParametersDefaults {
    /// Validate ESPM overhead consistency.
    ///
    /// Per C.S0005-E 2.6.2.2.5, MCC and IMSI_11_12 must both be wildcard
    /// (0x03ff / 0x7f) or both non-wildcard. A mixed configuration would
    /// cause IMSI class-0 OTA compression to silently lose identity fields.
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        let mcc_wildcard = self.mcc == 0x03ff;
        let imsi_11_12_wildcard = self.imsi_11_12 == 0x7f;
        if mcc_wildcard != imsi_11_12_wildcard {
            return Err(format!(
                "extended_system_parameters: mcc ({}) and imsi_11_12 ({}) must both be wildcard \
                 (0x03ff/0x7f) or both non-wildcard; mixed configuration breaks IMSI class-0 \
                 OTA compression",
                self.mcc, self.imsi_11_12,
            )
            .into());
        }
        if self.mcc > 0x03ff {
            return Err(format!(
                "extended_system_parameters: mcc ({}) exceeds 10-bit maximum (0x03ff)",
                self.mcc,
            )
            .into());
        }
        if self.imsi_11_12 > 0x7f {
            return Err(format!(
                "extended_system_parameters: imsi_11_12 ({}) exceeds 7-bit maximum (0x7f)",
                self.imsi_11_12,
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GeneralPageRecord {
    Class0 {
        page_subclass: u8,
        msg_seq: u8,
        imsi_s: Option<u64>,
        imsi_11_12: Option<u8>,
        mcc: Option<u16>,
        imsi_addr_num: Option<u8>,
        imsi_m_s1: Option<u32>,
        imsi_m_s2: Option<u16>,
        special_service: bool,
        service_option: Option<u16>,
    },
    Class1 {
        msg_seq: u8,
        esn: u32,
        special_service: bool,
        service_option: Option<u16>,
    },
    Tmsi {
        msg_seq: u8,
        tmsi_code_addr: u32,
        special_service: bool,
        service_option: Option<u16>,
    },
    Broadcast {
        bc_addr: u16,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralPageDefaults {
    pub class_0_done: bool,
    pub class_1_done: bool,
    pub tmsi_done: bool,
    pub ordered_tmsis: bool,
    pub broadcast_done: bool,
    pub reserved: u8,
    pub add_pfield: Vec<u8>,
    pub page_records: Vec<GeneralPageRecord>,
}

impl Default for GeneralPageDefaults {
    fn default() -> Self {
        Self {
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OrderDefaults {
    pub order: u8,
    pub ordq: u8,
}

impl Default for OrderDefaults {
    fn default() -> Self {
        Self { order: 0, ordq: 0 }
    }
}

#[derive(Clone, Debug)]
pub struct SystemParametersMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub sid: u16,
    pub nid: u16,
    pub reg_zone: u16,
    pub total_zones: u8,
    pub zone_timer: u8,
    pub mult_sids: bool,
    pub mult_nids: bool,
    pub base_id: u16,
    pub base_class: u8,
    pub page_chan: u8,
    pub max_slot_cycle_index: u8,
    pub home_reg: bool,
    pub for_sid_reg: bool,
    pub for_nid_reg: bool,
    pub power_up_reg: bool,
    pub power_down_reg: bool,
    pub parameter_reg: bool,
    pub reg_prd: u8,
    pub base_lat: u32,
    pub base_long: u32,
    pub reg_dist: u16,
    pub srch_win_a: u8,
    pub srch_win_n: u8,
    pub srch_win_r: u8,
    pub nghbr_max_age: u8,
    pub pwr_rep_thresh: u8,
    pub pwr_rep_frames: u8,
    pub pwr_thresh_enable: bool,
    pub pwr_period_enable: bool,
    pub pwr_rep_delay: u8,
    pub rescan: bool,
    pub t_add: u8,
    pub t_drop: u8,
    pub t_comp: u8,
    pub t_tdrop: u8,
    pub ext_sys_parameter: bool,
    pub ext_nghbr_lst: bool,
    pub gen_nghbr_lst: bool,
    pub global_redirect: bool,
    pub pri_nghbr_lst: bool,
    pub user_zone_id: bool,
    pub ext_global_redirect: bool,
    pub ext_chan_lst: bool,
    // C.S0005-E Table 3.7.2.3.2.1 mandatory tail at P_REV >= 6/7/8.
    pub t_tdrop_range_incl: bool,
    pub t_tdrop_range: u8,
    pub neg_slot_cycle_index_sup: bool,
    pub crrm_msg_ind: bool,
    /// Count of the optional-overhead-message flag bits that follow
    /// (AP_PILOT_INFO, AP_IDT, AP_ID_TEXT, GEN_OVHD_INF_IND, FD_CHAN_LST_IND,
    /// ATIM_IND) plus their reserved tail.
    pub num_opt_msg_bits: u8,
    pub ap_pilot_info: bool,
    pub ap_idt: bool,
    pub ap_id_text: bool,
    pub gen_ovhd_inf_ind: bool,
    pub fd_chan_lst_ind: bool,
    pub atim_ind: bool,
    pub appim_period_index: u8,
    pub gen_ovhd_cycle_index: u8,
    pub atim_cycle_index: u8,
    pub add_loc_info_incl: bool,
}

#[derive(Clone, Debug)]
pub struct AccessParametersMessage {
    pub pilot_pn: u16,
    pub acc_msg_seq: u8,
    pub acc_chan: u8,
    /// NOM_PWR open-loop offset in dB. Wire is 4-bit signed (-8..+7 dB
    /// for Band Class 0). The mobile uses this in its open-loop reverse
    /// TX power formula:
    ///
    ///   Tx_dBm = -Rx_dBm - 73 + NOM_PWR + INIT_PWR + (n-1)*PWR_STEP
    ///
    /// where `n` is the access probe number (1..NUM_STEP).
    pub nom_pwr: i8,
    /// INIT_PWR open-loop offset in dB. Wire is 5-bit signed (-16..+15 dB).
    pub init_pwr: i8,
    pub pwr_step: u8,
    pub num_step: u8,
    pub max_cap_sz: u8,
    pub pam_sz: u8,
    pub psist_0_9: u8,
    pub psist_10: u8,
    pub psist_11: u8,
    pub psist_12: u8,
    pub psist_13: u8,
    pub psist_14: u8,
    pub psist_15: u8,
    pub msg_psist: u8,
    pub reg_psist: u8,
    pub probe_pn_ran: u8,
    pub acc_tmo: u8,
    pub probe_bkoff: u8,
    pub bkoff: u8,
    pub max_req_seq: u8,
    pub max_rsp_seq: u8,
    pub auth: u8,
    pub rand: u32,
    pub nom_pwr_ext: u8,
    pub psist_emg_incl: bool,
    pub psist_emg: u8,
    pub acct_incl: bool,
    pub acct_incl_emg: bool,
    pub acct_aoc_bitmap_incl: bool,
    pub acct_so_records: Vec<AcctServiceOptionRecord>,
    pub acct_so_grp_records: Vec<AcctServiceOptionGroupRecord>,
}

#[derive(Clone, Debug)]
pub struct NeighborListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub pilot_inc: u8,
    pub neighbors: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct CdmaChannelListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub channels: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct ExtendedSystemParametersMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub delete_for_tmsi: bool,
    pub use_tmsi: bool,
    pub pref_msid_type: u8,
    pub mcc: u16,
    pub imsi_11_12: u8,
    pub tmsi_zone: Vec<u8>,
    pub bcast_index: u8,
    pub imsi_t_supported: bool,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub soft_slope: u8,
    pub add_intercept: u8,
    pub drop_intercept: u8,
    pub packet_zone_id: u8,
    pub max_num_alt_so: u8,
    pub reselect_included: bool,
    pub ec_thresh: u8,
    pub ec_io_thresh: u8,
    pub pilot_report: bool,
    pub nghbr_set_entry_info: bool,
    pub acc_ent_ho_order: bool,
    pub nghbr_set_access_info: bool,
    pub access_ho: bool,
    pub access_ho_msg_rsp: bool,
    pub access_probe_ho: bool,
    pub acc_ho_list_upd: bool,
    pub acc_probe_ho_other_msg: bool,
    pub max_num_probe_ho: u8,
    pub nghbr_set_size: u8,
    pub access_entry_ho: Vec<bool>,
    pub access_ho_allowed: Vec<bool>,
    pub broadcast_gps_asst: bool,
    pub qpch_supported: bool,
    pub num_qpch: u8,
    pub qpch_rate: u8,
    pub qpch_power_level_page: u8,
    pub qpch_cci_supported: bool,
    pub qpch_power_level_config: u8,
    pub sdb_supported: bool,
    pub rlgain_traffic_pilot: u8,
    pub rev_pwr_cntl_delay_incl: bool,
    pub rev_pwr_cntl_delay: u8,
    pub auto_msg_supported: bool,
    pub auto_msg_interval: u8,
    pub mob_qos: bool,
    pub enc_supported: bool,
    pub sig_encrypt_sup: u8,
    pub ui_encrypt_sup: u8,
    pub use_sync_id: bool,
    pub cs_supported: bool,
    pub bcch_supported: bool,
    pub ms_init_pos_loc_sup_ind: bool,
    pub pilot_info_req_supported: bool,
    pub ext_pref_msid_type: Option<u8>,
    pub meid_reqd: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct GeneralPageMessage {
    pub config_msg_seq: u8,
    pub acc_msg_seq: u8,
    pub class_0_done: bool,
    pub class_1_done: bool,
    pub tmsi_done: bool,
    pub ordered_tmsis: bool,
    pub broadcast_done: bool,
    pub reserved: u8,
    pub add_pfield: Vec<u8>,
    pub page_records: Vec<GeneralPageRecord>,
}

#[derive(Clone, Debug)]
pub struct OrderMessage {
    pub order: u8,
    pub ordq: u8,
    pub order_specific_fields: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationAcceptedOrder {
    pub roam_indi: Option<u8>,
    pub c_sig_encrypt_mode: Option<u8>,
    pub enc_key_size: Option<u8>,
    pub msg_int_info_incl: Option<bool>,
    pub change_keys: Option<bool>,
    pub use_uak: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryOrder {
    pub retry_type: u8,
    pub retry_delay: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseStationRejectOrder {
    pub reject_reason: u8,
    pub rejected_msg_type: u8,
    pub rejected_msg_seq: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeriodicPilotMeasurementRequestOrder {
    pub ordq: u8,
    pub min_pilot_pwr_thresh: u8,
    pub min_pilot_ec_i0_thresh: u8,
    pub incl_setpt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardOrderDetail {
    NoAdditionalFields { order: u8 },
    QualificationOnly { order: u8, ordq: u8 },
    BaseStationChallengeConfirmation { authbs: u32 },
    ServiceOptionRequest { service_option: u16 },
    ServiceOptionResponse { service_option: u16 },
    StatusRequest { information_record_type: u8 },
    RegistrationAccepted(RegistrationAcceptedOrder),
    Retry(RetryOrder),
    BaseStationReject(BaseStationRejectOrder),
    PeriodicPilotMeasurementRequest(PeriodicPilotMeasurementRequestOrder),
}

#[derive(Clone, Debug)]
pub struct AuthenticationChallengeMessage {
    pub randu: u32,
    pub gen_cmea_key: bool,
}

#[derive(Clone, Debug)]
pub struct SsdUpdateMessage {
    pub randssd: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InformationRecord {
    pub record_type: u8,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageWaitingRecord {
    pub msg_count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeterPulsesRecord {
    pub pulse_frequency: u16,
    pub pulse_on_time: u8,
    pub pulse_off_time: u8,
    pub pulse_count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallWaitingIndicatorRecord {
    pub call_waiting: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParametricAlertingRecord {
    pub cadence_count: u8,
    pub groups: Vec<ParametricAlertingGroup>,
    pub cadence_type: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParametricAlertingGroup {
    pub amplitude: u8,
    pub freq_1: u16,
    pub freq_2: u16,
    pub on_time: u8,
    pub off_time: u8,
    pub repeat: u8,
    pub delay: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineControlRecord {
    pub polarity: Option<LineControlPolarity>,
    pub power_denial_time: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineControlPolarity {
    Set { reverse_polarity: bool },
    Toggle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartyNumberRecord {
    pub number_type: u8,
    pub number_plan: u8,
    pub presentation_indicator: Option<u8>,
    pub screening_indicator: Option<u8>,
    pub redirection_reason: Option<u8>,
    pub digits: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartySubaddressRecord {
    pub subaddress_type: u8,
    pub odd_even_indicator: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedDisplayRecord {
    pub display_type: u8,
    pub segments: Vec<ExtendedDisplaySegment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedDisplaySegment {
    pub display_tag: u8,
    pub display_len: u8,
    pub chars: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCharExtendedDisplayRecord {
    pub display_type: u8,
    pub displays: Vec<MultiCharDisplay>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedMultiCharExtendedDisplayRecord {
    pub display_type: u8,
    pub displays: Vec<MultiCharDisplay>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCharDisplay {
    pub display_tag: u8,
    pub records: Vec<MultiCharDisplayTextRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCharDisplayTextRecord {
    pub display_encoding: u8,
    pub num_fields: u8,
    pub char_bits: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternationalExtendedRecord {
    pub mcc: u16,
    pub country_record_type: u8,
    pub data: Vec<u8>,
}

impl MultiCharDisplayTextRecord {
    pub fn text(&self) -> String {
        crate::sms::decode_user_data(
            self.display_encoding,
            None,
            self.num_fields,
            &self.char_bits,
        )
    }
}

impl InformationRecord {
    pub fn signal(signal: SignalInfoRecord) -> Self {
        let mut bits = Bitstream::new();
        bits.write_u8(signal.signal_type, 2);
        bits.write_u8(signal.alert_pitch, 2);
        bits.write_u8(signal.signal, 6);
        bits.write_u8(0, 6);
        let data = bits.to_packed_bytes();
        decode_signal_information_record(&data).expect("Signal information record must be valid");
        Self {
            record_type: InfoRecordType::Signal as u8,
            data,
        }
    }

    pub fn display_text(&self) -> Result<Option<String>, crate::error::Error> {
        if self.record_type != 0x01 {
            return Ok(None);
        }
        decode_display_information_record(&self.data).map(Some)
    }

    pub fn signal_info(&self) -> Result<Option<SignalInfoRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::Signal as u8 {
            return Ok(None);
        }
        decode_signal_information_record(&self.data).map(Some)
    }

    pub fn message_waiting(record: MessageWaitingRecord) -> Self {
        Self {
            record_type: InfoRecordType::MessageWaiting as u8,
            data: vec![record.msg_count],
        }
    }

    pub fn message_waiting_info(
        &self,
    ) -> Result<Option<MessageWaitingRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::MessageWaiting as u8 {
            return Ok(None);
        }
        decode_message_waiting_record(&self.data).map(Some)
    }

    pub fn meter_pulses(record: MeterPulsesRecord) -> Self {
        let mut bits = Bitstream::new();
        bits.write_u32(record.pulse_frequency as u32, 11);
        bits.write_u8(record.pulse_on_time, 8);
        bits.write_u8(record.pulse_off_time, 8);
        bits.write_u8(record.pulse_count, 4);
        bits.write_u8(0, 1);
        Self {
            record_type: InfoRecordType::MeterPulses as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn meter_pulses_info(&self) -> Result<Option<MeterPulsesRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::MeterPulses as u8 {
            return Ok(None);
        }
        decode_meter_pulses_record(&self.data).map(Some)
    }

    pub fn call_waiting_indicator(record: CallWaitingIndicatorRecord) -> Self {
        Self {
            record_type: InfoRecordType::CallWaitingIndicator as u8,
            data: vec![(record.call_waiting as u8) << 7],
        }
    }

    pub fn call_waiting_indicator_info(
        &self,
    ) -> Result<Option<CallWaitingIndicatorRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::CallWaitingIndicator as u8 {
            return Ok(None);
        }
        decode_call_waiting_indicator_record(&self.data).map(Some)
    }

    pub fn parametric_alerting(record: ParametricAlertingRecord) -> Self {
        let mut bits = Bitstream::new();
        assert!(
            record.groups.len() <= 15,
            "Parametric Alerting NUM_GROUPS must fit in 4 bits"
        );
        assert!(
            record.cadence_type <= 0b10,
            "Parametric Alerting CADENCE_TYPE 0b11 is reserved"
        );
        bits.write_u8(record.cadence_count, 8);
        bits.write_u8(record.groups.len() as u8, 4);
        for group in &record.groups {
            assert!(
                group.freq_1 <= 0x03ff && group.freq_2 <= 0x03ff,
                "Parametric Alerting frequencies must fit in 10 bits"
            );
            assert!(
                group.repeat <= 0x0f,
                "Parametric Alerting REPEAT must fit in 4 bits"
            );
            bits.write_u8(group.amplitude, 8);
            bits.write_u32(group.freq_1 as u32, 10);
            bits.write_u32(group.freq_2 as u32, 10);
            bits.write_u8(group.on_time, 8);
            bits.write_u8(group.off_time, 8);
            bits.write_u8(group.repeat, 4);
            bits.write_u8(group.delay, 8);
        }
        bits.write_u8(record.cadence_type, 2);
        bits.write_u8(0, 2);
        Self {
            record_type: InfoRecordType::ParametricAlerting as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn parametric_alerting_info(
        &self,
    ) -> Result<Option<ParametricAlertingRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::ParametricAlerting as u8 {
            return Ok(None);
        }
        decode_parametric_alerting_record(&self.data).map(Some)
    }

    pub fn line_control(record: LineControlRecord) -> Self {
        let mut bits = Bitstream::new();
        if let Some(polarity) = &record.polarity {
            bits.write_u8(1, 1);
            match polarity {
                LineControlPolarity::Set { reverse_polarity } => {
                    bits.write_u8(0, 1);
                    bits.write_u8(*reverse_polarity as u8, 1);
                }
                LineControlPolarity::Toggle => {
                    bits.write_u8(1, 1);
                    bits.write_u8(0, 1);
                }
            }
        } else {
            bits.write_u8(0, 1);
        }
        bits.write_u8(record.power_denial_time, 8);
        pad_to_octet(&mut bits);
        Self {
            record_type: InfoRecordType::LineControl as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn line_control_info(&self) -> Result<Option<LineControlRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::LineControl as u8 {
            return Ok(None);
        }
        decode_line_control_record(&self.data).map(Some)
    }

    pub fn party_number(record_type: InfoRecordType, record: PartyNumberRecord) -> Self {
        assert!(
            matches!(
                record_type,
                InfoRecordType::CalledPartyNumber
                    | InfoRecordType::CallingPartyNumber
                    | InfoRecordType::ConnectedNumber
                    | InfoRecordType::RedirectingNumber
            ),
            "information record type is not a party number"
        );
        assert_valid_number_record(record_type, &record);
        let mut bits = Bitstream::new();
        match record_type {
            InfoRecordType::CalledPartyNumber => {
                bits.write_u8(record.number_type, 3);
                bits.write_u8(record.number_plan, 4);
                write_ascii_digits(&mut bits, &record.digits);
                bits.write_u8(0, 1);
            }
            InfoRecordType::CallingPartyNumber | InfoRecordType::ConnectedNumber => {
                bits.write_u8(record.number_type, 3);
                bits.write_u8(record.number_plan, 4);
                bits.write_u8(record.presentation_indicator.unwrap(), 2);
                bits.write_u8(record.screening_indicator.unwrap(), 2);
                write_ascii_digits(&mut bits, &record.digits);
                bits.write_u8(0, 5);
            }
            InfoRecordType::RedirectingNumber => {
                let has_pi_si = record.presentation_indicator.is_some();
                bits.write_u8(!has_pi_si as u8, 1);
                bits.write_u8(record.number_type, 3);
                bits.write_u8(record.number_plan, 4);
                if has_pi_si {
                    let has_reason = record.redirection_reason.is_some();
                    bits.write_u8(!has_reason as u8, 1);
                    bits.write_u8(record.presentation_indicator.unwrap(), 2);
                    bits.write_u8(0, 3);
                    bits.write_u8(record.screening_indicator.unwrap(), 2);
                    if has_reason {
                        bits.write_u8(1, 1);
                        bits.write_u8(0, 3);
                        bits.write_u8(record.redirection_reason.unwrap(), 4);
                    }
                }
                write_ascii_digits(&mut bits, &record.digits);
            }
            _ => unreachable!(),
        }
        Self {
            record_type: record_type as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn party_number_info(&self) -> Result<Option<PartyNumberRecord>, crate::error::Error> {
        let Some(record_type) = InfoRecordType::from_wire(self.record_type) else {
            return Ok(None);
        };
        if !matches!(
            record_type,
            InfoRecordType::CalledPartyNumber
                | InfoRecordType::CallingPartyNumber
                | InfoRecordType::ConnectedNumber
                | InfoRecordType::RedirectingNumber
        ) {
            return Ok(None);
        }
        decode_party_number_record(record_type, &self.data).map(Some)
    }

    pub fn party_subaddress(record_type: InfoRecordType, record: PartySubaddressRecord) -> Self {
        assert!(
            matches!(
                record_type,
                InfoRecordType::CalledPartySubaddress
                    | InfoRecordType::CallingPartySubaddress
                    | InfoRecordType::ConnectedSubaddress
                    | InfoRecordType::RedirectingSubaddress
            ),
            "information record type is not a party subaddress"
        );
        assert_valid_subaddress_record(&record);
        let mut bits = Bitstream::new();
        bits.write_u8(1, 1);
        bits.write_u8(record.subaddress_type, 3);
        bits.write_u8(record.odd_even_indicator as u8, 1);
        bits.write_u8(0, 3);
        for byte in &record.data {
            bits.write_u8(*byte, 8);
        }
        Self {
            record_type: record_type as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn party_subaddress_info(
        &self,
    ) -> Result<Option<PartySubaddressRecord>, crate::error::Error> {
        let Some(record_type) = InfoRecordType::from_wire(self.record_type) else {
            return Ok(None);
        };
        if !matches!(
            record_type,
            InfoRecordType::CalledPartySubaddress
                | InfoRecordType::CallingPartySubaddress
                | InfoRecordType::ConnectedSubaddress
                | InfoRecordType::RedirectingSubaddress
        ) {
            return Ok(None);
        }
        decode_party_subaddress_record(record_type, &self.data).map(Some)
    }

    pub fn extended_display(record: ExtendedDisplayRecord) -> Self {
        assert_valid_extended_display_record(&record);
        let mut bits = Bitstream::new();
        bits.write_u8(1, 1);
        bits.write_u8(record.display_type, 7);
        for segment in &record.segments {
            bits.write_u8(segment.display_tag, 8);
            bits.write_u8(segment.display_len, 8);
            if !is_blank_or_skip_display_tag(segment.display_tag) {
                for ch in &segment.chars {
                    bits.write_u8(*ch, 8);
                }
            }
        }
        Self {
            record_type: InfoRecordType::ExtendedDisplay as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn extended_display_info(
        &self,
    ) -> Result<Option<ExtendedDisplayRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::ExtendedDisplay as u8 {
            return Ok(None);
        }
        decode_extended_display_record(&self.data).map(Some)
    }

    pub fn multi_char_extended_display(record: MultiCharExtendedDisplayRecord) -> Self {
        assert_valid_multi_char_display_record(&record.displays, record.display_type);
        let mut bits = Bitstream::new();
        bits.write_u8(1, 1);
        bits.write_u8(record.display_type, 7);
        write_multi_char_display_records(&mut bits, &record.displays, false);
        pad_to_octet(&mut bits);
        Self {
            record_type: InfoRecordType::MultiCharExtendedDisplay as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn multi_char_extended_display_info(
        &self,
    ) -> Result<Option<MultiCharExtendedDisplayRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::MultiCharExtendedDisplay as u8 {
            return Ok(None);
        }
        decode_multi_char_extended_display_record(&self.data).map(Some)
    }

    pub fn enhanced_multi_char_extended_display(
        record: EnhancedMultiCharExtendedDisplayRecord,
    ) -> Self {
        assert_valid_multi_char_display_record(&record.displays, record.display_type);
        assert!(
            record.displays.len() <= 256,
            "Enhanced Multiple Character Extended Display NUM_DISPLAYS must fit in one octet"
        );
        let mut bits = Bitstream::new();
        bits.write_u8(record.display_type, 7);
        bits.write_u8((record.displays.len() - 1) as u8, 8);
        write_multi_char_display_records(&mut bits, &record.displays, true);
        pad_to_octet(&mut bits);
        Self {
            record_type: InfoRecordType::EnhMultiCharExtendedDisplay as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn enhanced_multi_char_extended_display_info(
        &self,
    ) -> Result<Option<EnhancedMultiCharExtendedDisplayRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::EnhMultiCharExtendedDisplay as u8 {
            return Ok(None);
        }
        decode_enhanced_multi_char_extended_display_record(&self.data).map(Some)
    }

    pub fn international_extended_record(record: InternationalExtendedRecord) -> Self {
        assert!(
            record.mcc <= 0x03ff,
            "Extended Record Type - International MCC must fit in 10 bits"
        );
        assert!(
            record.country_record_type <= 0x3f,
            "Extended Record Type - International country-specific record type must fit in 6 bits"
        );
        let mut bits = Bitstream::new();
        bits.write_u32(record.mcc as u32, 10);
        bits.write_u8(record.country_record_type, 6);
        for byte in &record.data {
            bits.write_u8(*byte, 8);
        }
        Self {
            record_type: InfoRecordType::ExtendedRecordTypeIntl as u8,
            data: bits.to_packed_bytes(),
        }
    }

    pub fn international_extended_record_info(
        &self,
    ) -> Result<Option<InternationalExtendedRecord>, crate::error::Error> {
        if self.record_type != InfoRecordType::ExtendedRecordTypeIntl as u8 {
            return Ok(None);
        }
        decode_international_extended_record(&self.data).map(Some)
    }
}

fn decode_display_information_record(data: &[u8]) -> Result<String, crate::error::Error> {
    if data.is_empty() {
        return Err("Display information record requires at least one CHARi".into());
    }
    let mut text = String::with_capacity(data.len());
    for &ch in data {
        if ch & 0x80 != 0 {
            return Err("Display information record CHARi MSB must be zero".into());
        }
        text.push(char::from(ch));
    }
    Ok(text)
}

fn decode_signal_information_record(data: &[u8]) -> Result<SignalInfoRecord, crate::error::Error> {
    if data.len() != 2 {
        return Err("Signal information record must be exactly two octets".into());
    }
    let mut bits = Bitstream::new_bytes(data);
    let signal_type = bits.read_bits(2)? as u8;
    let alert_pitch = bits.read_bits(2)? as u8;
    let signal = bits.read_bits(6)? as u8;
    let reserved = bits.read_bits(6)? as u8;
    if signal_type == 0b11 {
        return Err("Signal information record SIGNAL_TYPE 0b11 is reserved".into());
    }
    if signal_type == 0b10 {
        if alert_pitch == 0b11 {
            return Err("Signal information record ALERT_PITCH 0b11 is reserved".into());
        }
    } else if alert_pitch != 0 {
        return Err(
            "Signal information record ALERT_PITCH must be 00 unless SIGNAL_TYPE=10".into(),
        );
    }
    let signal_valid = match signal_type {
        0b00 => matches!(signal, 0..=10 | 0b11_1111),
        0b01 => matches!(signal, 0 | 1 | 2 | 4 | 15),
        0b10 => signal <= 12,
        _ => false,
    };
    if !signal_valid {
        return Err("Signal information record SIGNAL value is reserved".into());
    }
    if reserved != 0 {
        return Err("Signal information record RESERVED bits must be zero".into());
    }
    Ok(SignalInfoRecord {
        signal_type,
        alert_pitch,
        signal,
    })
}

fn decode_message_waiting_record(data: &[u8]) -> Result<MessageWaitingRecord, crate::error::Error> {
    if data.len() != 1 {
        return Err("Message Waiting information record must be exactly one octet".into());
    }
    Ok(MessageWaitingRecord { msg_count: data[0] })
}

fn decode_meter_pulses_record(data: &[u8]) -> Result<MeterPulsesRecord, crate::error::Error> {
    if data.len() != 4 {
        return Err("Meter Pulses information record must be exactly four octets".into());
    }
    let mut bits = Bitstream::new_bytes(data);
    let pulse_frequency = bits.read_bits(11)? as u16;
    let pulse_on_time = bits.read_bits(8)? as u8;
    let pulse_off_time = bits.read_bits(8)? as u8;
    let pulse_count = bits.read_bits(4)? as u8;
    let reserved = bits.read_bits(1)? as u8;
    if reserved != 0 {
        return Err("Meter Pulses information record RESERVED bit must be zero".into());
    }
    Ok(MeterPulsesRecord {
        pulse_frequency,
        pulse_on_time,
        pulse_off_time,
        pulse_count,
    })
}

fn decode_call_waiting_indicator_record(
    data: &[u8],
) -> Result<CallWaitingIndicatorRecord, crate::error::Error> {
    if data.len() != 1 {
        return Err("Call Waiting Indicator information record must be exactly one octet".into());
    }
    let mut bits = Bitstream::new_bytes(data);
    let call_waiting = bits.read_bits(1)? != 0;
    let reserved = bits.read_bits(7)? as u8;
    if reserved != 0 {
        return Err("Call Waiting Indicator information record RESERVED bits must be zero".into());
    }
    Ok(CallWaitingIndicatorRecord { call_waiting })
}

fn decode_parametric_alerting_record(
    data: &[u8],
) -> Result<ParametricAlertingRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 16 {
        return Err("Parametric Alerting information record is truncated".into());
    }
    let cadence_count = bits.read_bits(8)? as u8;
    let num_groups = bits.read_bits(4)? as usize;
    let required_bits = num_groups
        .checked_mul(56)
        .and_then(|group_bits| group_bits.checked_add(4))
        .ok_or("Parametric Alerting information record length overflow")?;
    if bits.len() != required_bits {
        return Err(
            "Parametric Alerting information record length does not match NUM_GROUPS".into(),
        );
    }
    let mut groups = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        groups.push(ParametricAlertingGroup {
            amplitude: bits.read_bits(8)? as u8,
            freq_1: bits.read_bits(10)? as u16,
            freq_2: bits.read_bits(10)? as u16,
            on_time: bits.read_bits(8)? as u8,
            off_time: bits.read_bits(8)? as u8,
            repeat: bits.read_bits(4)? as u8,
            delay: bits.read_bits(8)? as u8,
        });
    }
    let cadence_type = bits.read_bits(2)? as u8;
    if cadence_type == 0b11 {
        return Err("Parametric Alerting CADENCE_TYPE 0b11 is reserved".into());
    }
    let reserved = bits.read_bits(2)? as u8;
    if reserved != 0 {
        return Err("Parametric Alerting RESERVED bits must be zero".into());
    }
    Ok(ParametricAlertingRecord {
        cadence_count,
        groups,
        cadence_type,
    })
}

fn decode_line_control_record(data: &[u8]) -> Result<LineControlRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 9 {
        return Err("Line Control information record is truncated".into());
    }
    let polarity = if bits.read_bits(1)? != 0 {
        let toggle_mode = bits.read_bits(1)? != 0;
        let reverse_polarity = bits.read_bits(1)? != 0;
        if toggle_mode {
            if reverse_polarity {
                return Err("Line Control REVERSE_POLARITY must be zero when TOGGLE_MODE=1".into());
            }
            Some(LineControlPolarity::Toggle)
        } else {
            Some(LineControlPolarity::Set { reverse_polarity })
        }
    } else {
        None
    };
    let power_denial_time = bits.read_bits(8)? as u8;
    if bits.len() > 7 {
        return Err("Line Control information record has excess reserved bits".into());
    }
    while !bits.is_empty() {
        if bits.read_bits(1)? != 0 {
            return Err("Line Control RESERVED bits must be zero".into());
        }
    }
    Ok(LineControlRecord {
        polarity,
        power_denial_time,
    })
}

fn assert_valid_number_record(record_type: InfoRecordType, record: &PartyNumberRecord) {
    assert!(
        matches!(record.number_type, 0 | 1 | 2 | 3 | 4 | 6),
        "party number NUMBER_TYPE is reserved"
    );
    assert!(
        matches!(record.number_plan, 0 | 1 | 3 | 4 | 9),
        "party number NUMBER_PLAN is reserved"
    );
    assert!(
        record.digits.as_bytes().iter().all(|ch| *ch <= 0x7f),
        "party number CHARi must be 7-bit ASCII"
    );
    match record_type {
        InfoRecordType::CalledPartyNumber => {
            assert!(
                record.presentation_indicator.is_none()
                    && record.screening_indicator.is_none()
                    && record.redirection_reason.is_none(),
                "Called Party Number does not carry PI, SI, or REDIRECTION_REASON"
            );
        }
        InfoRecordType::CallingPartyNumber | InfoRecordType::ConnectedNumber => {
            assert!(
                matches!(record.presentation_indicator, Some(0 | 1 | 2)),
                "party number PI is reserved"
            );
            assert!(
                record.screening_indicator.is_some(),
                "party number SI is required"
            );
            assert!(
                record.redirection_reason.is_none(),
                "Calling/Connected Number does not carry REDIRECTION_REASON"
            );
        }
        InfoRecordType::RedirectingNumber => {
            assert_eq!(
                record.presentation_indicator.is_some(),
                record.screening_indicator.is_some(),
                "Redirecting Number carries PI and SI together"
            );
            if let Some(pi) = record.presentation_indicator {
                assert!(matches!(pi, 0 | 1 | 2), "party number PI is reserved");
            }
            if let Some(reason) = record.redirection_reason {
                assert!(
                    record.presentation_indicator.is_some(),
                    "Redirecting Number requires PI/SI when REDIRECTION_REASON is present"
                );
                assert!(
                    is_valid_redirection_reason(reason),
                    "Redirecting Number REDIRECTION_REASON is reserved"
                );
            }
        }
        _ => unreachable!(),
    }
}

fn assert_valid_subaddress_record(record: &PartySubaddressRecord) {
    assert!(
        matches!(record.subaddress_type, 0 | 2),
        "party subaddress SUBADDRESS_TYPE is reserved"
    );
    assert!(
        record.subaddress_type != 2 || record.data.len() <= 20,
        "user-specified party subaddress CHARi data must not exceed 20 octets"
    );
}

fn write_ascii_digits(bits: &mut Bitstream, digits: &str) {
    for byte in digits.as_bytes() {
        bits.write_u8(*byte, 8);
    }
}

fn decode_party_number_record(
    record_type: InfoRecordType,
    data: &[u8],
) -> Result<PartyNumberRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 8 {
        return Err(format!("{record_type:?} information record is truncated").into());
    }
    if record_type == InfoRecordType::RedirectingNumber {
        let extension_bit_1 = bits.read_bits(1)? as u8;
        let number_type = bits.read_bits(3)? as u8;
        validate_number_type(record_type, number_type)?;
        let number_plan = bits.read_bits(4)? as u8;
        validate_number_plan(record_type, number_plan)?;
        let (presentation_indicator, screening_indicator, redirection_reason) = if extension_bit_1
            == 0
        {
            if bits.len() < 8 {
                return Err("Redirecting Number information record is truncated".into());
            }
            let extension_bit_2 = bits.read_bits(1)? as u8;
            let pi = bits.read_bits(2)? as u8;
            validate_presentation_indicator(record_type, pi)?;
            let reserved = bits.read_bits(3)? as u8;
            if reserved != 0 {
                return Err("Redirecting Number RESERVED bits after PI must be zero".into());
            }
            let si = bits.read_bits(2)? as u8;
            let reason = if extension_bit_2 == 0 {
                if bits.len() < 8 {
                    return Err("Redirecting Number REDIRECTION_REASON is truncated".into());
                }
                let extension_bit_3 = bits.read_bits(1)? as u8;
                if extension_bit_3 != 1 {
                    return Err("Redirecting Number EXTENSION_BIT_3 must be one".into());
                }
                let reserved = bits.read_bits(3)? as u8;
                if reserved != 0 {
                    return Err(
                        "Redirecting Number RESERVED bits before REDIRECTION_REASON must be zero"
                            .into(),
                    );
                }
                let reason = bits.read_bits(4)? as u8;
                if !is_valid_redirection_reason(reason) {
                    return Err("Redirecting Number REDIRECTION_REASON value is reserved".into());
                }
                Some(reason)
            } else {
                None
            };
            (Some(pi), Some(si), reason)
        } else {
            (None, None, None)
        };
        let digits = read_ascii_until_reserved(&mut bits, 0, record_type)?;
        return Ok(PartyNumberRecord {
            number_type,
            number_plan,
            presentation_indicator,
            screening_indicator,
            redirection_reason,
            digits,
        });
    }
    let number_type = bits.read_bits(3)? as u8;
    validate_number_type(record_type, number_type)?;
    let number_plan = bits.read_bits(4)? as u8;
    validate_number_plan(record_type, number_plan)?;
    match record_type {
        InfoRecordType::CalledPartyNumber => {
            let digits = read_ascii_until_reserved(&mut bits, 1, record_type)?;
            let reserved = bits.read_bits(1)? as u8;
            if reserved != 0 {
                return Err("Called Party Number RESERVED bit must be zero".into());
            }
            Ok(PartyNumberRecord {
                number_type,
                number_plan,
                presentation_indicator: None,
                screening_indicator: None,
                redirection_reason: None,
                digits,
            })
        }
        InfoRecordType::CallingPartyNumber | InfoRecordType::ConnectedNumber => {
            if bits.len() < 9 {
                return Err(format!("{record_type:?} information record is truncated").into());
            }
            let presentation_indicator = bits.read_bits(2)? as u8;
            validate_presentation_indicator(record_type, presentation_indicator)?;
            let screening_indicator = bits.read_bits(2)? as u8;
            let digits = read_ascii_until_reserved(&mut bits, 5, record_type)?;
            let reserved = bits.read_bits(5)? as u8;
            if reserved != 0 {
                return Err(format!("{record_type:?} RESERVED bits must be zero").into());
            }
            Ok(PartyNumberRecord {
                number_type,
                number_plan,
                presentation_indicator: Some(presentation_indicator),
                screening_indicator: Some(screening_indicator),
                redirection_reason: None,
                digits,
            })
        }
        InfoRecordType::RedirectingNumber => unreachable!(),
        _ => Err(format!("{record_type:?} is not a party number information record").into()),
    }
}

fn decode_party_subaddress_record(
    record_type: InfoRecordType,
    data: &[u8],
) -> Result<PartySubaddressRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 8 {
        return Err(format!("{record_type:?} information record is truncated").into());
    }
    let extension_bit = bits.read_bits(1)? as u8;
    if extension_bit != 1 {
        return Err(format!("{record_type:?} EXTENSION_BIT must be one").into());
    }
    let subaddress_type = bits.read_bits(3)? as u8;
    if !matches!(subaddress_type, 0 | 2) {
        return Err(format!("{record_type:?} SUBADDRESS_TYPE value is reserved").into());
    }
    let odd_even_indicator = bits.read_bits(1)? != 0;
    let reserved = bits.read_bits(3)? as u8;
    if reserved != 0 {
        return Err(format!("{record_type:?} RESERVED bits must be zero").into());
    }
    let mut data = Vec::with_capacity(bits.len() / 8);
    while bits.len() >= 8 {
        data.push(bits.read_bits(8)? as u8);
    }
    if !bits.is_empty() {
        return Err(format!("{record_type:?} has non-octet trailing bits").into());
    }
    if subaddress_type == 2 && data.len() > 20 {
        return Err(format!("{record_type:?} exceeds 20 octets").into());
    }
    Ok(PartySubaddressRecord {
        subaddress_type,
        odd_even_indicator,
        data,
    })
}

fn read_ascii_until_reserved(
    bits: &mut Bitstream,
    reserved_bits: usize,
    record_type: InfoRecordType,
) -> Result<String, crate::error::Error> {
    let mut bytes = Vec::with_capacity(bits.len() / 8);
    while bits.len() > reserved_bits {
        if bits.len() - reserved_bits < 8 {
            return Err(format!("{record_type:?} has partial CHARi field").into());
        }
        let ch = bits.read_bits(8)? as u8;
        if ch & 0x80 != 0 {
            return Err(format!("{record_type:?} CHARi MSB must be zero").into());
        }
        bytes.push(ch);
    }
    String::from_utf8(bytes)
        .map_err(|err| format!("{record_type:?} CHARi is not UTF-8: {err}").into())
}

fn validate_number_type(
    record_type: InfoRecordType,
    number_type: u8,
) -> Result<(), crate::error::Error> {
    if !matches!(number_type, 0 | 1 | 2 | 3 | 4 | 6) {
        return Err(format!("{record_type:?} NUMBER_TYPE value is reserved").into());
    }
    Ok(())
}

fn validate_number_plan(
    record_type: InfoRecordType,
    number_plan: u8,
) -> Result<(), crate::error::Error> {
    if !matches!(number_plan, 0 | 1 | 3 | 4 | 9) {
        return Err(format!("{record_type:?} NUMBER_PLAN value is reserved").into());
    }
    Ok(())
}

fn validate_presentation_indicator(
    record_type: InfoRecordType,
    presentation_indicator: u8,
) -> Result<(), crate::error::Error> {
    if presentation_indicator == 0b11 {
        return Err(format!("{record_type:?} PI value is reserved").into());
    }
    Ok(())
}

fn is_valid_redirection_reason(reason: u8) -> bool {
    matches!(reason, 0 | 1 | 2 | 9 | 10 | 15)
}

fn assert_valid_extended_display_record(record: &ExtendedDisplayRecord) {
    assert_eq!(
        record.display_type, 0,
        "Extended Display DISPLAY_TYPE values other than Normal are reserved"
    );
    assert!(
        !record.segments.is_empty(),
        "Extended Display requires one or more display records"
    );
    for segment in &record.segments {
        assert!(
            is_valid_extended_display_tag(segment.display_tag),
            "Extended Display DISPLAY_TAG is reserved"
        );
        if is_blank_or_skip_display_tag(segment.display_tag) {
            assert!(
                segment.chars.is_empty(),
                "Extended Display Blank/Skip tags do not carry CHARi fields"
            );
        } else {
            assert_eq!(
                segment.display_len as usize,
                segment.chars.len(),
                "Extended Display DISPLAY_LEN must match CHARi count"
            );
            assert!(
                segment.chars.iter().all(|ch| *ch <= 0x7f),
                "Extended Display CHARi MSB must be zero"
            );
        }
    }
}

fn decode_extended_display_record(
    data: &[u8],
) -> Result<ExtendedDisplayRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 24 {
        return Err("Extended Display information record is truncated".into());
    }
    let ext_display_ind = bits.read_bits(1)? as u8;
    if ext_display_ind != 1 {
        return Err("Extended Display EXT_DISPLAY_IND must be one".into());
    }
    let display_type = bits.read_bits(7)? as u8;
    if display_type != 0 {
        return Err("Extended Display DISPLAY_TYPE value is reserved".into());
    }
    let mut segments = Vec::new();
    while !bits.is_empty() {
        if bits.len() < 16 {
            return Err("Extended Display display record is truncated".into());
        }
        let display_tag = bits.read_bits(8)? as u8;
        if !is_valid_extended_display_tag(display_tag) {
            return Err("Extended Display DISPLAY_TAG value is reserved".into());
        }
        let display_len = bits.read_bits(8)? as u8;
        let chars = if is_blank_or_skip_display_tag(display_tag) {
            Vec::new()
        } else {
            if bits.len() < display_len as usize * 8 {
                return Err("Extended Display CHARi fields are truncated".into());
            }
            let mut chars = Vec::with_capacity(display_len as usize);
            for _ in 0..display_len {
                let ch = bits.read_bits(8)? as u8;
                if ch & 0x80 != 0 {
                    return Err("Extended Display CHARi MSB must be zero".into());
                }
                chars.push(ch);
            }
            chars
        };
        segments.push(ExtendedDisplaySegment {
            display_tag,
            display_len,
            chars,
        });
    }
    if segments.is_empty() {
        return Err("Extended Display requires one or more display records".into());
    }
    Ok(ExtendedDisplayRecord {
        display_type,
        segments,
    })
}

fn is_blank_or_skip_display_tag(display_tag: u8) -> bool {
    matches!(display_tag, 0x80 | 0x81)
}

fn is_valid_extended_display_tag(display_tag: u8) -> bool {
    matches!(display_tag, 0x80..=0x9a | 0x9e)
}

fn assert_valid_multi_char_display_record(displays: &[MultiCharDisplay], display_type: u8) {
    assert_eq!(
        display_type, 0,
        "Multiple Character Extended Display DISPLAY_TYPE values other than Normal are reserved"
    );
    assert!(
        !displays.is_empty(),
        "Multiple Character Extended Display requires one or more display records"
    );
    for display in displays {
        assert!(
            is_valid_extended_display_tag(display.display_tag),
            "Multiple Character Extended Display DISPLAY_TAG is reserved"
        );
        if is_blank_or_skip_display_tag(display.display_tag) {
            assert!(
                display.records.is_empty(),
                "Multiple Character Extended Display Blank/Skip tags require NUM_RECORD=0"
            );
        } else {
            assert!(
                !display.records.is_empty(),
                "Multiple Character Extended Display text tags require one or more text records"
            );
        }
        assert!(
            display.records.len() <= u8::MAX as usize,
            "Multiple Character Extended Display NUM_RECORD must fit in one octet"
        );
        for record in &display.records {
            assert_valid_multi_char_text_record(record);
        }
    }
}

fn assert_valid_multi_char_text_record(record: &MultiCharDisplayTextRecord) {
    assert!(
        record.display_encoding <= 0x1f,
        "Multiple Character Extended Display DISPLAY_ENCODING top three bits must be zero"
    );
    assert!(
        record.char_bits.iter().all(|bit| *bit <= 1),
        "Multiple Character Extended Display CHARi bits must be 0 or 1"
    );
    assert_eq!(
        record.char_bits.len(),
        cdma_text_char_bits(record.display_encoding, record.num_fields),
        "Multiple Character Extended Display CHARi bit length must match DISPLAY_ENCODING and NUM_FIELDS"
    );
}

fn write_multi_char_display_records(
    bits: &mut Bitstream,
    displays: &[MultiCharDisplay],
    enhanced: bool,
) {
    for display in displays {
        bits.write_u8(display.display_tag, 8);
        bits.write_u8(display.records.len() as u8, 8);
        for record in &display.records {
            if enhanced {
                let record_bits = 24 + record.char_bits.len();
                let record_len = record_bits.div_ceil(8);
                assert!(
                    record_len <= u8::MAX as usize,
                    "Enhanced Multiple Character Extended Display RECORD_LENGTH must fit in one octet"
                );
                bits.write_u8(record_len as u8, 8);
            }
            bits.write_u8(record.display_encoding, 8);
            bits.write_u8(record.num_fields, 8);
            for bit in &record.char_bits {
                bits.write_u8(*bit, 1);
            }
            if enhanced {
                pad_to_octet(bits);
            }
        }
    }
}

fn decode_multi_char_extended_display_record(
    data: &[u8],
) -> Result<MultiCharExtendedDisplayRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 24 {
        return Err("Multiple Character Extended Display information record is truncated".into());
    }
    let mc_ext_display_ind = bits.read_bits(1)? as u8;
    if mc_ext_display_ind != 1 {
        return Err("Multiple Character Extended Display MC_EXT_DISPLAY_IND must be one".into());
    }
    let display_type = bits.read_bits(7)? as u8;
    validate_multi_char_display_type(display_type, "Multiple Character Extended Display")?;
    let displays = decode_multi_char_display_records_until_padding(
        &mut bits,
        "Multiple Character Extended Display",
    )?;
    Ok(MultiCharExtendedDisplayRecord {
        display_type,
        displays,
    })
}

fn decode_enhanced_multi_char_extended_display_record(
    data: &[u8],
) -> Result<EnhancedMultiCharExtendedDisplayRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 23 {
        return Err(
            "Enhanced Multiple Character Extended Display information record is truncated".into(),
        );
    }
    let display_type = bits.read_bits(7)? as u8;
    validate_multi_char_display_type(display_type, "Enhanced Multiple Character Extended Display")?;
    let num_displays = bits.read_bits(8)? as usize + 1;
    let mut displays = Vec::with_capacity(num_displays);
    for _ in 0..num_displays {
        displays.push(decode_one_multi_char_display_record(
            &mut bits,
            "Enhanced Multiple Character Extended Display",
            true,
        )?);
    }
    validate_reserved_padding(
        &mut bits,
        "Enhanced Multiple Character Extended Display RESERVED_1",
    )?;
    Ok(EnhancedMultiCharExtendedDisplayRecord {
        display_type,
        displays,
    })
}

fn decode_multi_char_display_records_until_padding(
    bits: &mut Bitstream,
    label: &str,
) -> Result<Vec<MultiCharDisplay>, crate::error::Error> {
    let mut displays = Vec::new();
    while bits.len() > 7 {
        displays.push(decode_one_multi_char_display_record(bits, label, false)?);
    }
    validate_reserved_padding(bits, &format!("{label} RESERVED"))?;
    if displays.is_empty() {
        return Err(format!("{label} requires one or more display records").into());
    }
    Ok(displays)
}

fn decode_one_multi_char_display_record(
    bits: &mut Bitstream,
    label: &str,
    enhanced: bool,
) -> Result<MultiCharDisplay, crate::error::Error> {
    if bits.len() < 16 {
        return Err(format!("{label} display record is truncated").into());
    }
    let display_tag = bits.read_bits(8)? as u8;
    if !is_valid_extended_display_tag(display_tag) {
        return Err(format!("{label} DISPLAY_TAG value is reserved").into());
    }
    let num_record = bits.read_bits(8)? as usize;
    if is_blank_or_skip_display_tag(display_tag) && num_record != 0 {
        return Err(format!("{label} Blank/Skip DISPLAY_TAG requires NUM_RECORD=0").into());
    }
    if !is_blank_or_skip_display_tag(display_tag) && num_record == 0 {
        return Err(format!("{label} text DISPLAY_TAG requires NUM_RECORD > 0").into());
    }
    let mut records = Vec::with_capacity(num_record);
    for _ in 0..num_record {
        records.push(if enhanced {
            decode_enhanced_multi_char_text_record(bits, label)?
        } else {
            decode_multi_char_text_record(bits, label)?
        });
    }
    Ok(MultiCharDisplay {
        display_tag,
        records,
    })
}

fn decode_multi_char_text_record(
    bits: &mut Bitstream,
    label: &str,
) -> Result<MultiCharDisplayTextRecord, crate::error::Error> {
    if bits.len() < 16 {
        return Err(format!("{label} text record is truncated").into());
    }
    let display_encoding = bits.read_bits(8)? as u8;
    validate_display_encoding(display_encoding, label)?;
    let num_fields = bits.read_bits(8)? as u8;
    let char_bit_len = cdma_text_char_bits(display_encoding, num_fields);
    if bits.len() < char_bit_len {
        return Err(format!("{label} CHARi fields are truncated").into());
    }
    let char_bits = bits.drain(0..char_bit_len).bits().to_vec();
    Ok(MultiCharDisplayTextRecord {
        display_encoding,
        num_fields,
        char_bits,
    })
}

fn decode_enhanced_multi_char_text_record(
    bits: &mut Bitstream,
    label: &str,
) -> Result<MultiCharDisplayTextRecord, crate::error::Error> {
    if bits.len() < 24 {
        return Err(format!("{label} enhanced text record is truncated").into());
    }
    let record_length = bits.read_bits(8)? as usize;
    if record_length < 3 {
        return Err(format!(
            "{label} RECORD_LENGTH must include length, DISPLAY_ENCODING, and NUM_FIELDS"
        )
        .into());
    }
    let payload_bits = (record_length - 1) * 8;
    if bits.len() < payload_bits {
        return Err(format!("{label} RECORD_LENGTH exceeds remaining record").into());
    }
    let mut record_bits = bits.drain(0..payload_bits);
    let display_encoding = record_bits.read_bits(8)? as u8;
    validate_display_encoding(display_encoding, label)?;
    let num_fields = record_bits.read_bits(8)? as u8;
    let char_bit_len = cdma_text_char_bits(display_encoding, num_fields);
    if record_bits.len() < char_bit_len {
        return Err(format!("{label} RECORD_LENGTH is too short for CHARi fields").into());
    }
    let char_bits = record_bits.drain(0..char_bit_len).bits().to_vec();
    if record_bits.len() > 7 {
        return Err(format!("{label} RECORD_LENGTH is not minimally octet-aligned").into());
    }
    validate_reserved_padding(&mut record_bits, &format!("{label} text RESERVED"))?;
    Ok(MultiCharDisplayTextRecord {
        display_encoding,
        num_fields,
        char_bits,
    })
}

fn validate_multi_char_display_type(
    display_type: u8,
    label: &str,
) -> Result<(), crate::error::Error> {
    if display_type != 0 {
        return Err(format!("{label} DISPLAY_TYPE value is reserved").into());
    }
    Ok(())
}

fn validate_display_encoding(display_encoding: u8, label: &str) -> Result<(), crate::error::Error> {
    if display_encoding & 0b1110_0000 != 0 {
        return Err(format!("{label} DISPLAY_ENCODING top three bits must be zero").into());
    }
    Ok(())
}

fn validate_reserved_padding(bits: &mut Bitstream, label: &str) -> Result<(), crate::error::Error> {
    while !bits.is_empty() {
        if bits.read_bits(1)? != 0 {
            return Err(format!("{label} bits must be zero").into());
        }
    }
    Ok(())
}

fn decode_international_extended_record(
    data: &[u8],
) -> Result<InternationalExtendedRecord, crate::error::Error> {
    let mut bits = Bitstream::new_bytes(data);
    if bits.len() < 16 {
        return Err("Extended Record Type - International requires at least two octets".into());
    }
    let mcc = bits.read_bits(10)? as u16;
    let country_record_type = bits.read_bits(6)? as u8;
    let mut data = Vec::with_capacity(bits.len() / 8);
    while bits.len() >= 8 {
        data.push(bits.read_bits(8)? as u8);
    }
    if !bits.is_empty() {
        return Err("Extended Record Type - International has non-octet trailing bits".into());
    }
    Ok(InternationalExtendedRecord {
        mcc,
        country_record_type,
        data,
    })
}

#[derive(Clone, Debug)]
pub struct FeatureNotificationMessage {
    pub release: bool,
    pub records: Vec<InformationRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedNeighborRecord {
    pub nghbr_config: u8,
    pub nghbr_pn: u16,
    pub search_priority: u8,
    pub nghbr_band: Option<u8>,
    pub nghbr_freq: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct ExtendedNeighborListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub pilot_inc: u8,
    pub neighbors: Vec<ExtendedNeighborRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusQualificationInfo {
    None,
    BandClass { band_class: u8 },
    BandClassAndOperatingMode { band_class: u8, op_mode: u8 },
}

#[derive(Clone, Debug)]
pub struct StatusRequestMessage {
    pub qual_info: StatusQualificationInfo,
    pub record_types: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ServiceRedirectionMessage {
    pub return_if_fail: bool,
    pub delete_tmsi: bool,
    pub redirect_type: bool,
    pub record_type: u8,
    pub record: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct GlobalServiceRedirectionMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub redirect_accolc: u16,
    pub return_if_fail: bool,
    pub delete_tmsi: bool,
    pub excl_p_rev_ms: bool,
    pub record_type: u8,
    pub record: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TmsiAssignmentMessage {
    pub tmsi_zone: Vec<u8>,
    pub tmsi_code: u32,
    pub tmsi_exp_time: u32,
}

#[derive(Clone, Debug)]
pub struct PacaMessage {
    pub purpose: u8,
    pub q_pos: u8,
    pub paca_timeout: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralNeighborGlobalTiming {
    pub tx_duration: u8,
    pub tx_period: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralNeighborTiming {
    pub tx_offset: u8,
    pub tx_duration: Option<u8>,
    pub tx_period: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralNeighborRecord {
    pub nghbr_config: Option<u8>,
    pub nghbr_pn: Option<u16>,
    pub search_priority: Option<u8>,
    pub srch_win_nghbr: Option<u8>,
    pub nghbr_band: Option<u8>,
    pub nghbr_freq: Option<u16>,
    pub timing: Option<GeneralNeighborTiming>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralAnalogNeighborRecord {
    pub band_class: u8,
    pub sys_a_b: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sr3AuxPilotInfo {
    pub qof: u8,
    pub walsh_length: u8,
    pub aux_pilot_walsh: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneralNeighborPilotRecord {
    OneXCommonWithTransmitDiversity {
        td_power_level: u8,
        td_mode: u8,
    },
    OneXAuxiliary {
        qof: u8,
        walsh_length: u8,
        aux_pilot_walsh: u16,
    },
    OneXAuxiliaryWithTransmitDiversity {
        qof: u8,
        walsh_length: u8,
        aux_walsh: u16,
        aux_td_power_level: u8,
        td_mode: u8,
    },
    ThreeXCommon {
        sr3_primary_pilot: u8,
        sr3_pilot_power1: u8,
        sr3_pilot_power2: u8,
    },
    ThreeXAuxiliary {
        sr3_primary_pilot: u8,
        sr3_pilot_power1: u8,
        sr3_pilot_power2: u8,
        primary_aux: Sr3AuxPilotInfo,
        lower_aux: Option<Sr3AuxPilotInfo>,
        upper_aux: Option<Sr3AuxPilotInfo>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralNeighborPilotInfo {
    pub pilot_record: Option<GeneralNeighborPilotRecord>,
    pub srch_offset_nghbr: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralNeighborResqInfo {
    pub delay_time: u8,
    pub allowed_time: u8,
    pub attempt_time: u8,
    pub code_chan: u16,
    pub qof: u8,
    pub min_period: Option<u8>,
    pub num_tot_trans_20ms: Option<u8>,
    pub num_tot_trans_5ms: Option<u8>,
    pub num_preamble_rc1_rc2: u8,
    pub num_preamble: u8,
    pub power_delta: u8,
    pub nghbr_resq_configured: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HrpdNeighborRecord {
    pub nghbr_pn: u16,
    pub nghbr_band: Option<u8>,
    pub nghbr_freq: Option<u16>,
    pub pn_association_ind: bool,
    pub data_association_ind: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalMcRadioInterface {
    pub pilot_inc: u8,
    pub nghbr_srch_mode: u8,
    pub srch_win_n: Option<u8>,
    pub srch_offset_incl: bool,
    pub freq_fields_incl: bool,
    pub use_timing: bool,
    pub global_timing: Option<UniversalMcGlobalTiming>,
    pub nghbr_set_entry_info: bool,
    pub nghbr_set_access_info: bool,
    pub neighbors: Vec<UniversalMcNeighborRecord>,
    pub resq: Option<UniversalMcResqParameters>,
    pub pdch_supported: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalMcGlobalTiming {
    pub tx_duration: u8,
    pub tx_period: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalMcNeighborRecord {
    pub nghbr_config: u8,
    pub nghbr_pn: u16,
    pub bcch_support: Option<bool>,
    pub pilot_record: Option<GeneralNeighborPilotRecord>,
    pub search_priority: Option<u8>,
    pub srch_win_nghbr: Option<u8>,
    pub srch_offset_nghbr: Option<u8>,
    pub nghbr_band: Option<u8>,
    pub nghbr_freq: Option<u16>,
    pub timing: Option<UniversalMcNeighborTiming>,
    pub access_entry_ho: Option<bool>,
    pub access_ho_allowed: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalMcNeighborTiming {
    pub tx_offset: u8,
    pub tx_duration: Option<u8>,
    pub tx_period: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalMcResqParameters {
    pub delay_time: u8,
    pub allowed_time: u8,
    pub attempt_time: u8,
    pub code_chan: u16,
    pub qof: u8,
    pub min_period: Option<u8>,
    pub num_tot_trans_20ms: Option<u8>,
    pub num_tot_trans_5ms: Option<u8>,
    pub num_preamble_rc1_rc2: u8,
    pub num_preamble: u8,
    pub power_delta: u8,
    pub neighbor_configured: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct GeneralNeighborListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub pilot_inc: u8,
    pub nghbr_srch_mode: u8,
    pub nghbr_config_pn_incl: bool,
    pub freq_fields_incl: bool,
    pub use_timing: bool,
    pub global_timing: Option<GeneralNeighborGlobalTiming>,
    pub neighbors: Vec<GeneralNeighborRecord>,
    pub analog_neighbors: Vec<GeneralAnalogNeighborRecord>,
    pub srch_offset_incl: bool,
    pub pilot_info: Vec<GeneralNeighborPilotInfo>,
    pub bcch_support: Option<Vec<bool>>,
    pub resq: Option<GeneralNeighborResqInfo>,
    pub pdch_supported: Vec<bool>,
    pub hrpd_neighbors: Option<Vec<HrpdNeighborRecord>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserZoneRecord {
    pub uzid: u16,
    pub uz_rev: u8,
    pub temp_sub: bool,
}

#[derive(Clone, Debug)]
pub struct UserZoneIdentificationMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub uz_exit: u8,
    pub zones: Vec<UserZoneRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateRadioInterfaceRecord {
    pub common_band_class: Option<u8>,
    pub common_nghbr_freq: Option<u16>,
    pub srch_win_pn: u8,
    pub neighbors: Vec<PrivateNeighborRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateNeighborRecord {
    pub sid: u16,
    pub nid: u16,
    pub pri_nghbr_pn: u16,
    pub pilot_record: Option<GeneralNeighborPilotRecord>,
    pub band_class: Option<u8>,
    pub nghbr_freq: Option<u16>,
    pub zones: Option<Vec<UserZoneRecord>>,
}

#[derive(Clone, Debug)]
pub struct PrivateNeighborListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub radio_interfaces: Vec<PrivateRadioInterfaceRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectPRevRange {
    pub exclude: bool,
    pub min: u8,
    pub max: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtendedRedirectionRecord {
    NdssOff,
    Cdma {
        band_class: u8,
        expected_sid: u16,
        expected_nid: u16,
        cdma_chans: Vec<u16>,
        redirect_subclasses: Option<Vec<bool>>,
    },
    Ds41(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedGlobalRedirectionTarget {
    pub redirect_accolc: u16,
    pub delete_tmsi: bool,
    pub p_rev: Option<RedirectPRevRange>,
    pub record: ExtendedRedirectionRecord,
    pub last_search_record_ind: bool,
}

#[derive(Clone, Debug)]
pub struct ExtendedGlobalServiceRedirectionMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub return_if_fail: bool,
    pub primary: ExtendedGlobalRedirectionTarget,
    pub additional_records: Vec<ExtendedGlobalRedirectionTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedCdmaTdFrequencySelection {
    pub td_hash_ind: bool,
    pub td_power_level: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedCdmaTdSelection {
    pub td_mode: u8,
    pub frequencies: Vec<ExtendedCdmaTdFrequencySelection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedCdmaAdditionalFrequency {
    pub add_cdma_freq: u16,
    pub add_rc_qpch_hash_ind: Option<bool>,
    pub add_td_hash_ind: Option<bool>,
    pub add_td_power_level: Option<u8>,
    pub add_cdma_freq_weight: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedCdmaAdditionalBand {
    pub add_cdma_band: u8,
    pub subclasses: Option<Vec<bool>>,
    pub add_td_mode: Option<u8>,
    pub bypass_sys_det_ind: bool,
    pub frequencies: Vec<ExtendedCdmaAdditionalFrequency>,
}

#[derive(Clone, Debug)]
pub struct ExtendedCdmaChannelListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub cdma_freqs: Vec<u16>,
    pub rc_qpch_hash_ind: Option<Vec<bool>>,
    pub td_selection: Option<ExtendedCdmaTdSelection>,
    pub cdma_band: u8,
    pub subclasses: Option<Vec<bool>>,
    pub cdma_freq_weights: Option<Vec<u8>>,
    pub additional_bands: Vec<ExtendedCdmaAdditionalBand>,
}

#[derive(Clone, Debug)]
pub struct UserZoneRejectMessage {
    pub reject_uzid: u16,
    pub reject_action_indi: u8,
    pub assign_uzid: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ansi41OtherInfo {
    pub base_id: u16,
    pub mcc: u16,
    pub imsi_11_12: u8,
    pub broadcast_gps_asst: bool,
    pub sig_encrypt_sup: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketZoneHysteresisInfo {
    pub list_len: u8,
    pub act_timer: u8,
    pub timer_mul: u8,
    pub timer_exp: u8,
}

#[derive(Clone, Debug)]
pub struct Ansi41SystemParametersMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub sid: u16,
    pub nid: u16,
    pub packet_zone_id: u8,
    pub reg_zone: u16,
    pub total_zones: u8,
    pub zone_timer: u8,
    pub mult_sids: bool,
    pub mult_nids: bool,
    pub home_reg: bool,
    pub for_sid_reg: bool,
    pub for_nid_reg: bool,
    pub power_up_reg: bool,
    pub power_down_reg: bool,
    pub parameter_reg: bool,
    pub reg_prd: u8,
    pub reg_dist: Option<u16>,
    pub delete_for_tmsi: bool,
    pub use_tmsi: bool,
    pub pref_msid_type: u8,
    pub tmsi_zone: Vec<u8>,
    pub imsi_t_supported: bool,
    pub max_num_alt_so: u8,
    pub auto_msg_interval: Option<u8>,
    pub other_info: Option<Ansi41OtherInfo>,
    pub cs_supported: bool,
    pub ms_init_pos_loc_sup_ind: bool,
    pub msg_integrity_sup: bool,
    pub sig_integrity_sup: Option<u8>,
    pub imsi_10: Option<u8>,
    pub max_add_serv_instance: Option<u8>,
    pub tkz_id: Option<u8>,
    pub pz_hyst_enabled: bool,
    pub pz_hyst_info: Option<PacketZoneHysteresisInfo>,
    pub ext_pref_msid_type: u8,
    pub meid_reqd: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McRrSr3Parameters {
    pub sr3_center_freq: Option<u16>,
    pub sr3_brat: u8,
    pub sr3_bcch_code_chan: u8,
    pub sr3_primary_pilot: u8,
    pub sr3_pilot_power1: u8,
    pub sr3_pilot_power2: u8,
}

#[derive(Clone, Debug)]
pub struct McRrParametersMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub base_id: u16,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub sr3: Option<McRrSr3Parameters>,
    pub srch_win_a: u8,
    pub srch_win_r: u8,
    pub t_add: u8,
    pub t_drop: u8,
    pub t_comp: u8,
    pub t_tdrop: u8,
    pub nghbr_max_age: u8,
    pub soft_slope: u8,
    pub add_intercept: u8,
    pub drop_intercept: u8,
    pub sig_encrypt_sup: Option<u8>,
    pub ui_encrypt_sup: Option<u8>,
    pub add_fields: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Ansi41RandMessage {
    pub pilot_pn: u16,
    pub acc_msg_seq: u8,
    pub rand: u32,
}

#[derive(Clone, Debug)]
pub struct EnhancedAccessParametersMessage {
    pub pilot_pn: u16,
    pub acc_msg_seq: u8,
    pub body_bits: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessParametersBody {
    pub psist: Option<EnhancedAccessPsistParameters>,
    pub lac: EnhancedAccessLacParameters,
    pub mode_selection_entries: Vec<EnhancedAccessModeSelectionEntry>,
    pub rlgain_common_pilot: u8,
    pub ic_thresh: u8,
    pub ic_max: u8,
    pub mode_parameter_records: Vec<EnhancedAccessModeParameterRecord>,
    pub basic_access: Option<EnhancedAccessBasicAccessParameters>,
    pub reservation_access: Option<EnhancedAccessReservationAccessParameters>,
    pub acct: Option<EnhancedAccessAcctParameters>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessPsistParameters {
    pub psist_0_9_each: u8,
    pub psist_10_each: u8,
    pub psist_11_each: u8,
    pub psist_12_each: u8,
    pub psist_13_each: u8,
    pub psist_14_each: u8,
    pub psist_15_each: u8,
    pub psist_emg: u8,
    pub msg_psist_each: u8,
    pub reg_psist_each: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessLacParameters {
    pub acc_tmo: u8,
    pub max_req_seq: u8,
    pub max_rsp_seq: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessModeSelectionEntry {
    pub access_mode: u8,
    pub min_duration: u16,
    pub max_duration: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessModeParameterRecord {
    pub applicable_modes: u8,
    pub each_nom_pwr: u8,
    pub each_init_pwr: u8,
    pub each_pwr_step: u8,
    pub each_num_step: u8,
    pub preamble: Option<EnhancedAccessPreambleParameters>,
    pub each_probe_bkoff: u8,
    pub each_bkoff: u8,
    pub each_slot: u8,
    pub each_slot_offset1: u8,
    pub each_slot_offset2: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessPreambleParameters {
    pub num_frac: u8,
    pub frac_duration: u8,
    pub off_duration: u8,
    pub add_duration: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessBasicAccessParameters {
    pub num_each_ba: u8,
    pub each_ba_rates_supported: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessReservationAccessParameters {
    pub num_each_ra: u8,
    pub num_cach: u8,
    pub cach_code_rate: bool,
    pub cach_code_chans: Vec<u8>,
    pub num_rccch: u8,
    pub rccch_rates_supported: u8,
    pub rccch_preamble: Option<EnhancedAccessPreambleParameters>,
    pub rccch_slot: u8,
    pub rccch_slot_offset1: u8,
    pub rccch_slot_offset2: u8,
    pub rccch_nom_pwr: u8,
    pub rccch_init_pwr: u8,
    pub ra_pc_delay: u8,
    pub eacam_cach_delay: u8,
    pub rccch_ho_thresh: Option<u8>,
    pub eacam_pccam_delay: u8,
    pub num_cpcch: u8,
    pub cpcch_rate: u8,
    pub cpcch_code_chans: Vec<u8>,
    pub num_pcsch_ra: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessAcctParameters {
    pub acct_incl_emg: bool,
    pub acct_aoc_bitmap_incl: bool,
    pub acct_so_records: Vec<EnhancedAccessAcctServiceOptionRecord>,
    pub acct_so_group_records: Vec<EnhancedAccessAcctServiceOptionGroupRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessAcctServiceOptionRecord {
    pub acct_aoc_bitmap: Option<u8>,
    pub acct_so: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnhancedAccessAcctServiceOptionGroupRecord {
    pub acct_aoc_bitmap: Option<u8>,
    pub acct_so_group: u8,
}

#[derive(Clone, Debug)]
pub struct UniversalNeighborListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub radio_interfaces: Vec<UniversalRadioInterfaceRecord>,
}

#[derive(Clone, Debug)]
pub enum UniversalRadioInterfaceRecord {
    Mc { fields: Vec<u8> },
    Hrpd { neighbors: Vec<HrpdNeighborRecord> },
}

#[derive(Clone, Debug)]
pub struct SecurityModeCommandMessage {
    pub c_sig_encrypt_mode: u8,
    pub enc_key_size: Option<u8>,
    pub change_keys: Option<bool>,
    pub use_uak: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct UniversalPageMessage {
    pub config_msg_seq: u8,
    pub acc_msg_seq: u8,
    pub read_next_slot: bool,
    pub read_next_slot_bcast: bool,
    pub block_body_bits: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalPageBlock {
    pub addresses: UniversalPageInterleavedAddresses,
    pub records: Vec<UniversalPageRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UniversalPageInterleavedAddresses {
    pub broadcasts: Vec<UniversalPageBroadcastAddress>,
    pub imsis: Vec<UniversalPagePartialAddress>,
    pub tmsis: Vec<UniversalPagePartialAddress>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalPageBroadcastAddress {
    pub burst_type: u8,
    /// Bit 0 is the first transmitted BC_ADDRESS_BIT occurrence.
    pub address_bits: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniversalPagePartialAddress {
    /// Bit 0 is the first transmitted IMSI_S_BIT or TMSI_CODE_ADDR_BIT occurrence.
    pub address_bits: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniversalPageRecord {
    MobileStation {
        address_type: UniversalPageMobileAddressType,
        msg_seq: u8,
        service_option: u16,
        add_record: Vec<u8>,
    },
    MessageAnnouncement {
        address_type: UniversalPageAnnouncementAddressType,
    },
    EnhancedBroadcast {
        addr_len: u8,
        bc_addr_remainder: Vec<u8>,
        bcn: u8,
        time_offset: u16,
        repeat_time_offset: Option<u8>,
        add_record: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniversalPageMobileAddressType {
    Class0 {
        imsi_s_33_16: u32,
        imsi_11_12: Option<u8>,
        mcc: Option<u16>,
    },
    Class1 {
        imsi_addr_num: u8,
        imsi_11_12: u8,
        mcc: Option<u16>,
        imsi_s_33_16: u32,
    },
    Tmsi {
        tmsi_zone: Option<Vec<u8>>,
        tmsi_code_addr_31_16: Option<u16>,
        tmsi_code_addr_23_16: Option<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniversalPageAnnouncementAddressType {
    Imsi,
    Tmsi,
}

#[derive(Clone, Debug)]
pub struct UniversalPageSegmentMessage {
    pub upm_segment_seq: Option<u8>,
    pub segment_bits: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AuthenticationRequestMessage {
    pub randa: Vec<u8>,
    pub con_sqn: Vec<u8>,
    pub amf: [u8; 2],
    pub mac_a: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AlternativeTechnologiesInformationMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub radio_interfaces: Vec<AlternativeTechnologyRadioInterfaceRecord>,
}

#[derive(Clone, Debug)]
pub enum AlternativeTechnologyRadioInterfaceRecord {
    Hrpd { fields: Vec<u8> },
    Eutran { fields: Vec<u8> },
    Wimax { fields: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeHrpdRadioInterface {
    pub subnet_color_code: Option<u8>,
    pub neighbors: Vec<AlternativeHrpdNeighborRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeHrpdNeighborRecord {
    pub nghbr_pn: u16,
    pub freq_same_as_prev: bool,
    pub nghbr_band: Option<u8>,
    pub nghbr_freq: Option<u16>,
    pub pn_association_ind: bool,
    pub data_association_ind: bool,
    pub subnet_color_code: AlternativeHrpdNeighborSubnetColorCode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlternativeHrpdNeighborSubnetColorCode {
    NotIncluded,
    SameAsCommon,
    Explicit(u8),
}

#[derive(Clone, Debug)]
pub struct ForwardGeneralExtensionMessage {
    pub records: Vec<ForwardGeneralExtensionRecord>,
    pub message_type: u8,
    pub message_rec_bits: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum ForwardGeneralExtensionRecord {
    ReverseChannelInfo { band_class: u8, rev_chan: u16 },
    RadioConfigurationParameters { fields: Vec<u8> },
}

#[derive(Clone, Debug)]
pub struct GeneralOverheadInformationMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub records: Vec<GeneralOverheadInformationRecord>,
}

#[derive(Clone, Debug)]
pub enum GeneralOverheadInformationRecord {
    OperatorName { fields: Vec<u8> },
    CellName { fields: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdmaTextFields {
    pub msg_encoding: u8,
    pub num_fields: u8,
    pub text: String,
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AccessPointIdentifierMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub asstn_type: u8,
    pub sid: u16,
    pub nid: u16,
    pub ap_id: Vec<u16>,
    pub ap_id_mask: u8,
    pub ios_msc_id: u32,
    pub ios_cell_id: u16,
    pub hrpd_acquisition: Option<AccessPointHrpdAcquisitionRecord>,
    pub location: AccessPointLocationRecord,
    pub intra_freq_ho_hys: Option<u8>,
    pub intra_freq_ho_slope: Option<u8>,
    pub inter_freq_ho_hys: Option<u8>,
    pub inter_freq_ho_slope: Option<u8>,
    pub inter_freq_srch_th: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPointHrpdAcquisitionRecord {
    pub hrpd_pn: u16,
    pub hrpd_band_class: u8,
    pub hrpd_channel: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessPointLocationRecord {
    None,
    BaseStation {
        base_lat: i32,
        base_long: i32,
        loc_unc_h: u8,
        base_height: u16,
        loc_unc_v: u8,
    },
}

#[derive(Clone, Debug)]
pub struct AccessPointPilotInformationMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub lifetime: u16,
    pub records: Vec<AccessPointPilotInformationRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPointPilotInformationRecord {
    pub ap_assn_type: u8,
    pub sid: u16,
    pub nid: u16,
    pub band: u8,
    pub freq: u16,
    pub pn_record: AccessPointPilotPnRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessPointPilotPnRecord {
    List { pns: Vec<u16> },
    Series { count: u8, start: u16, inc: u8 },
}

#[derive(Clone, Debug)]
pub struct FlexDuplexCdmaChannelListMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub cand_band_info_req: bool,
    pub candidate_bands: Vec<FlexDuplexCandidateBand>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlexDuplexCandidateBand {
    pub cand_band_class: u8,
    pub subclasses: Option<Vec<bool>>,
    pub bypass_sys_det_ind: bool,
    pub frequencies: Vec<FlexDuplexFrequencyRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlexDuplexFrequencyRecord {
    pub cdma_freq: u16,
    pub remaining: Option<FlexDuplexRemainingFields>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlexDuplexRemainingFields {
    pub rev_cdma_freq: u16,
    pub rc_qpch_hash_ind: Option<bool>,
    pub cdma_freq_weight: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct BroadcastServiceParametersMessage {
    pub pilot_pn: u16,
    pub bspm_msg_seq: u8,
    pub body_bits: Bitstream,
}

#[derive(Clone, Debug)]
pub struct AccessPointIdentifierTextMessage {
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub ap_id_text: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Information Record Types (C.S0005-E Table 3.7.5-1)
// ---------------------------------------------------------------------------

/// Information record type codes per C.S0005-E Table 3.7.5-1.
/// Used in AWIM, FWIM, FNM, and other messages that carry information records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InfoRecordType {
    Display = 0x01,                     // 00000001
    CalledPartyNumber = 0x02,           // 00000010
    CallingPartyNumber = 0x03,          // 00000011
    ConnectedNumber = 0x04,             // 00000100
    Signal = 0x05,                      // 00000101
    MessageWaiting = 0x06,              // 00000110
    ServiceConfiguration = 0x07,        // 00000111
    CalledPartySubaddress = 0x08,       // 00001000
    CallingPartySubaddress = 0x09,      // 00001001
    ConnectedSubaddress = 0x0A,         // 00001010
    RedirectingNumber = 0x0B,           // 00001011
    RedirectingSubaddress = 0x0C,       // 00001100
    MeterPulses = 0x0D,                 // 00001101
    ParametricAlerting = 0x0E,          // 00001110
    LineControl = 0x0F,                 // 00001111
    ExtendedDisplay = 0x10,             // 00010000
    NonNegServiceConfiguration = 0x13,  // 00010011
    MultiCharExtendedDisplay = 0x14,    // 00010100
    CallWaitingIndicator = 0x15,        // 00010101
    EnhMultiCharExtendedDisplay = 0x16, // 00010110
    ExtendedRecordTypeIntl = 0xFE,      // 11111110
}

impl InfoRecordType {
    pub fn from_wire(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::Display,
            0x02 => Self::CalledPartyNumber,
            0x03 => Self::CallingPartyNumber,
            0x04 => Self::ConnectedNumber,
            0x05 => Self::Signal,
            0x06 => Self::MessageWaiting,
            0x07 => Self::ServiceConfiguration,
            0x08 => Self::CalledPartySubaddress,
            0x09 => Self::CallingPartySubaddress,
            0x0A => Self::ConnectedSubaddress,
            0x0B => Self::RedirectingNumber,
            0x0C => Self::RedirectingSubaddress,
            0x0D => Self::MeterPulses,
            0x0E => Self::ParametricAlerting,
            0x0F => Self::LineControl,
            0x10 => Self::ExtendedDisplay,
            0x13 => Self::NonNegServiceConfiguration,
            0x14 => Self::MultiCharExtendedDisplay,
            0x15 => Self::CallWaitingIndicator,
            0x16 => Self::EnhMultiCharExtendedDisplay,
            0xFE => Self::ExtendedRecordTypeIntl,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Alert With Information Message (C.S0005-E 3.7.3.3.2.3)
// ---------------------------------------------------------------------------
//
// Sent on the forward dedicated traffic channel (f-dsch) to instruct the MS
// to play a call progress tone (e.g. ringback). Contains one or more
// information records; the key record for voice call setup is the Signal
// Information Record (RECORD_TYPE=0x05).

/// Signal Information Record for Alert With Information Message.
///
/// Per C.S0005-E 3.7.5.5 (Table 3.7.5.5-1 through 3.7.5.5-5):
/// - SIGNAL_TYPE (2 bits): 00=Tone, 01=ISDN Alerting, 10=IS-54B Alerting, 11=Reserved
/// - ALERT_PITCH (2 bits): 00=Medium, 01=High, 10=Low, 11=Reserved
/// - SIGNAL (6 bits): specific signal pattern
/// - RESERVED (6 bits): padding
///
/// Calling Party Number information record per C.S0005-E 3.7.5.3.
///
/// Encodes: NUMBER_TYPE(3) + NUMBER_PLAN(4) + PI(2) + SI(2)
/// + CHARi(8) × digits + RESERVED(5). The outer RECORD_LEN identifies
/// the number of octets in this type-specific payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallingPartyNumberRecord {
    /// 3-bit number type per ANSI T1.607: 0=unknown, 1=international,
    /// 2=national, 3=network, 4=subscriber, 6=abbreviated.
    pub number_type: u8,
    /// 4-bit numbering plan: 0=unknown, 1=ISDN/telephony (E.164),
    /// 3=data, 4=telex, 9=private.
    pub number_plan: u8,
    /// 2-bit presentation indicator: 0=allowed, 1=restricted, 2=number not available.
    pub presentation_indicator: u8,
    /// 2-bit screening indicator: 0=user not screened, 1=user passed,
    /// 2=user failed, 3=network provided.
    pub screening_indicator: u8,
    /// The calling party digits (ASCII).
    pub digits: String,
}

impl CallingPartyNumberRecord {
    /// Encode the record content (everything after RECORD_TYPE/RECORD_LEN)
    /// per C.S0005-E 3.7.5.3, padded to a whole number
    /// of octets. The returned `Vec<u8>` is the value carried inside the
    /// IOS A.S0014-D `MS Information Records` IE (§4.2.55) for a record
    /// whose Information Record Type field is `0x03`.
    pub fn encode_content_bytes(&self) -> Vec<u8> {
        InformationRecord::party_number(
            InfoRecordType::CallingPartyNumber,
            PartyNumberRecord {
                number_type: self.number_type,
                number_plan: self.number_plan,
                presentation_indicator: Some(self.presentation_indicator),
                screening_indicator: Some(self.screening_indicator),
                redirection_reason: None,
                digits: self.digits.clone(),
            },
        )
        .data
    }

    /// Decode the record content (everything after RECORD_TYPE/RECORD_LEN).
    /// Mirror of `encode_content_bytes`.
    pub fn decode_content_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        let record = decode_party_number_record(InfoRecordType::CallingPartyNumber, bytes)
            .map_err(|_| "invalid Calling Party Number record")?;
        Ok(Self {
            number_type: record.number_type,
            number_plan: record.number_plan,
            presentation_indicator: record.presentation_indicator.unwrap_or(0),
            screening_indicator: record.screening_indicator.unwrap_or(0),
            digits: record.digits,
        })
    }
}

/// Standard Alert = SIGNAL_TYPE='10', ALERT_PITCH='00', SIGNAL='000001'.
#[derive(Clone, Debug)]
pub struct SignalInfoRecord {
    /// 2-bit signal type per Table 3.7.5.5-1.
    pub signal_type: u8,
    /// 2-bit alert pitch per Table 3.7.5.5-2.
    pub alert_pitch: u8,
    /// 6-bit signal pattern code per Tables 3.7.5.5-3 through 3.7.5.5-5.
    pub signal: u8,
}

/// Alert With Information Message per C.S0005-E 3.7.3.3.2.3.
///
/// Sent on f-dsch to tell the MS what call progress tone to play
/// (e.g., ringback during MO call setup). The message carries one or
/// more information records.
#[derive(Clone, Debug)]
pub struct AlertWithInformationMessage {
    /// Signal info record (RECORD_TYPE=0x05). If present, the MS plays
    /// the indicated call progress tone.
    pub signal_info: Option<SignalInfoRecord>,
    /// Calling Party Number record (RECORD_TYPE=0x03). Delivers caller ID
    /// to the mobile station.
    pub calling_party: Option<CallingPartyNumberRecord>,
}

impl AlertWithInformationMessage {
    /// Create a standard ringback tone AWIM (normal IS-54B alerting,
    /// medium pitch).
    pub fn ringback() -> Self {
        Self {
            signal_info: Some(SignalInfoRecord {
                signal_type: 0x00, // Tone signal ('00') per Table 3.7.5.5-1
                alert_pitch: 0x00, // ignored for tone signals ('00')
                signal: 0x01,      // Ring back tone on ('000001') per Table 3.7.5.5-3
            }),
            calling_party: None,
        }
    }

    /// Encode the f-dsch Alert With Information Message SDU.
    ///
    /// Per C.S0005-E 3.7.3.3.2.3: zero or more information records,
    /// each: RECORD_TYPE(8) + RECORD_LEN(8) + type-specific fields(8×RECORD_LEN).
    /// No USE_TIME, no ACTION_TIME, no NUM_INFO_RECS — just raw records.
    pub fn to_ftch_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();

        if let Some(ref sig) = self.signal_info {
            bs.write_u8(InfoRecordType::Signal as u8, 8); // RECORD_TYPE
            bs.write_u8(0x02, 8); // RECORD_LEN = 2 octets
            bs.write_u8(sig.signal_type, 2); // SIGNAL_TYPE
            bs.write_u8(sig.alert_pitch, 2); // ALERT_PITCH
            bs.write_u8(sig.signal, 6); // SIGNAL
            bs.write_u8(0, 6); // RESERVED (pad to 16 bits = 2 octets)
        }

        if let Some(ref cpn) = self.calling_party {
            let content = cpn.encode_content_bytes();
            bs.write_u8(InfoRecordType::CallingPartyNumber as u8, 8);
            bs.write_u8(content.len() as u8, 8);
            for &byte in &content {
                bs.write_u8(byte, 8);
            }
        }

        bs
    }
}

#[cfg(test)]
mod calling_party_number_codec_tests {
    use super::CallingPartyNumberRecord;

    #[test]
    fn roundtrip_basic() {
        let rec = CallingPartyNumberRecord {
            number_type: 3,
            number_plan: 1,
            presentation_indicator: 0,
            screening_indicator: 3,
            digits: "5551234567".to_string(),
        };
        let bytes = rec.encode_content_bytes();
        let decoded = CallingPartyNumberRecord::decode_content_bytes(&bytes).unwrap();
        assert_eq!(decoded, rec);
    }

    #[test]
    fn roundtrip_international_15_digits() {
        let rec = CallingPartyNumberRecord {
            number_type: 1,
            number_plan: 1,
            presentation_indicator: 0,
            screening_indicator: 3,
            digits: "123456789012345".to_string(),
        };
        let bytes = rec.encode_content_bytes();
        let decoded = CallingPartyNumberRecord::decode_content_bytes(&bytes).unwrap();
        assert_eq!(decoded, rec);
    }

    #[test]
    fn encodes_spec_payload_without_number_length_field() {
        let rec = CallingPartyNumberRecord {
            number_type: 1,
            number_plan: 1,
            presentation_indicator: 0,
            screening_indicator: 3,
            digits: "1".to_string(),
        };
        assert_eq!(rec.encode_content_bytes(), vec![0x22, 0x66, 0x20]);
    }

    #[test]
    fn roundtrip_all_fields_distinct() {
        let rec = CallingPartyNumberRecord {
            number_type: 6,
            number_plan: 9,
            presentation_indicator: 2,
            screening_indicator: 1,
            digits: "411".to_string(),
        };
        let bytes = rec.encode_content_bytes();
        let decoded = CallingPartyNumberRecord::decode_content_bytes(&bytes).unwrap();
        assert_eq!(decoded, rec);
    }
}

// ---------------------------------------------------------------------------
// Channel Assignment Message (C.S0005-E 3.7.2.3.2.8)
// ---------------------------------------------------------------------------
//
// Supports ASSIGN_MODE=000 (IS-95 Band Class 0 Traffic Channel Assignment,
// RC1/RC2) and ASSIGN_MODE=100 (Extended Traffic Channel Assignment) with
// DEFAULT_CONFIG selection for RC1–RC4.
//
// The BSC selects the radio configuration from the mobile's declared
// FOR_FCH_RC / REV_FCH_RC capabilities. RC3 forward and reverse traffic
// channels are implemented (R=1/4 K=9 coding, I/Q demux, HPSK spreading).
//
// Limitations:
// - Only 20ms framing (no 5ms F-FCH or 5ms R-FCH support)
// - PLCM is ESN-only (PLCM_TYPE=0000)
// - RC5/RC6 not yet implemented
// - The ECAM encoder below is intentionally minimal: same-frequency,
//   one-pilot, F-FCH/R-FCH-only bring-up with explicit FOR_RC/REV_RC

/// Channel Assignment Message sent on the paging channel to assign a traffic
/// channel to a mobile station.
#[derive(Clone, Debug)]
pub struct ChannelAssignmentMessage {
    /// ASSIGN_MODE: 3 bits. 000 = traffic channel assignment (IS-95, Band Class 0 only).
    /// 100 = Extended Traffic Channel Assignment (IS-2000, required for p_rev >= 6).
    pub assign_mode: u8,
    /// FREQ_INCL: 1 bit. 0 = same frequency, no band/freq fields.
    pub freq_incl: bool,
    /// CODE_CHAN: 8 bits. Walsh code index for the forward traffic channel.
    pub code_chan: u8,
    /// FRAME_OFFSET: 4 bits. Traffic channel frame offset (0 for simplicity).
    pub frame_offset: u8,
    /// ENCRYPT_MODE: 2 bits. 00 = disabled.
    pub encrypt_mode: u8,
    /// BAND_CLASS: 5 bits (only if FREQ_INCL = 1).
    pub band_class: Option<u8>,
    /// CDMA_FREQ: 11 bits (only if FREQ_INCL = 1).
    pub cdma_freq: Option<u16>,
    /// BYPASS_ALERT_ANSWER: 1 bit (ASSIGN_MODE=100 only).
    /// When true, mobile bypasses alert/answer and goes directly to conversation.
    pub bypass_alert_answer: Option<bool>,
    /// DEFAULT_CONFIG: 3 bits (ASSIGN_MODE=100 only).
    /// 0b000 = RC1 fwd + RC1 rev (MuxOpt1, 9600 bps).
    /// 0b001 = RC2 fwd + RC2 rev (MuxOpt2, 14400 bps).
    /// 0b100 = use FOR_RC/REV_RC (requires ECAM, not basic CAM).
    pub default_config: Option<u8>,
    /// GRANTED_MODE: 2 bits (ASSIGN_MODE=100 only).
    /// 0b00 = use DEFAULT_CONFIG values.
    pub granted_mode: Option<u8>,
    /// PLCM_TYPE_INCL: 1 bit (ASSIGN_MODE=100 only).
    /// 1 = PLCM type and possibly PLCM_39 included.
    pub plcm_type_incl: Option<bool>,
    /// PLCM_TYPE: 4 bits (only if PLCM_TYPE_INCL=1).
    /// 0000 = ESN-based (no PLCM_39 needed).
    pub plcm_type: Option<u8>,
}

impl ChannelAssignmentMessage {
    /// Create a simple same-frequency traffic channel assignment.
    ///
    /// Uses ASSIGN_MODE=000 (IS-95, RC1/RC2 only). For IS-2000 mobiles that
    /// support RC3+, use `new_extended_traffic_assignment()` instead.
    pub fn new_traffic_assignment(walsh_code: u8, frame_offset: u8) -> Self {
        Self {
            assign_mode: 0b000,
            freq_incl: false,
            code_chan: walsh_code,
            frame_offset,
            encrypt_mode: 0b00,
            band_class: None,
            cdma_freq: None,
            bypass_alert_answer: None,
            default_config: None,
            granted_mode: None,
            plcm_type_incl: None,
            plcm_type: None,
        }
    }

    /// Create an Extended Traffic Channel Assignment (ASSIGN_MODE=100) for IS-2000 mobiles.
    ///
    /// Required for mobiles with p_rev >= 6 that only support RC3+.
    /// Uses DEFAULT_CONFIG to select the radio configuration:
    /// - `default_config=0b000` → RC1/RC1 (MuxOpt1, 9600 bps)
    /// - `default_config=0b001` → RC2/RC2 (MuxOpt2, 14400 bps)
    ///
    /// Note: DEFAULT_CONFIG=0b100 (explicit FOR_RC/REV_RC) requires the Extended
    /// Channel Assignment Message (ECAM, 3.7.2.3.2.21) which is a separate message
    /// type. The basic paging channel CAM does not carry FOR_RC/REV_RC fields.
    pub fn new_extended_traffic_assignment(
        walsh_code: u8,
        frame_offset: u8,
        default_config: u8,
    ) -> Self {
        Self {
            assign_mode: 0b100,
            freq_incl: false,
            code_chan: walsh_code,
            frame_offset,
            encrypt_mode: 0b00,
            band_class: None,
            cdma_freq: None,
            bypass_alert_answer: Some(true), // bypass alert/answer (direct to conversation for SMS)
            default_config: Some(default_config),
            granted_mode: Some(0b00), // use DEFAULT_CONFIG values
            plcm_type_incl: Some(true),
            plcm_type: Some(0), // 0000 = ESN-based
        }
    }

    pub fn to_sdu(&self) -> Bitstream {
        assert_eq!(
            self.encrypt_mode, 0,
            "CAM encoder currently supports only ENCRYPT_MODE=00"
        );
        let mut bs = Bitstream::new();
        let mut record = Bitstream::new();
        bs.write_u8(self.assign_mode, 3);
        if self.assign_mode == 0b000 {
            // ASSIGN_MODE=000: Traffic channel assignment (IS-95 / Band Class 0)
            record.write_u8(self.freq_incl as u8, 1);
            record.write_u8(self.code_chan, 8);
            if self.freq_incl {
                record.write_u32(self.cdma_freq.unwrap_or(0) as u32, 11);
            }
            record.write_u8(self.frame_offset, 4);
            record.write_u8(self.encrypt_mode, 2);
            record.write_u8(0, 1); // C_SIG_ENCRYPT_MODE_INCL
        } else if self.assign_mode == 0b100 {
            // ASSIGN_MODE=100: Extended Traffic Channel Assignment (IS-2000)
            // Per C.S0005-E 3.7.2.3.2.8:
            //   FREQ_INCL(1), RESERVED(3), BYPASS_ALERT_ANSWER(1),
            //   DEFAULT_CONFIG(3), GRANTED_MODE(2), CODE_CHAN(8),
            //   FRAME_OFFSET(4), ENCRYPT_MODE(2),
            //   [BAND_CLASS(5), CDMA_FREQ(11) if FREQ_INCL=1],
            //   [encryption fields if ENCRYPT_MODE != 00],
            //   RESERVED (pad to octet boundary)
            record.write_u8(self.freq_incl as u8, 1); // FREQ_INCL
            record.write_u8(0b000, 3); // RESERVED
            record.write_u8(self.bypass_alert_answer.unwrap_or(true) as u8, 1); // BYPASS_ALERT_ANSWER
            record.write_u8(self.default_config.unwrap_or(0b000), 3); // DEFAULT_CONFIG
            record.write_u8(self.granted_mode.unwrap_or(0b00), 2); // GRANTED_MODE
            record.write_u8(self.code_chan, 8); // CODE_CHAN
            record.write_u8(self.frame_offset, 4); // FRAME_OFFSET
            record.write_u8(self.encrypt_mode, 2); // ENCRYPT_MODE
            if self.freq_incl {
                record.write_u8(self.band_class.unwrap_or(0), 5);
                record.write_u32(self.cdma_freq.unwrap_or(0) as u32, 11);
            }
            record.write_u8(0, 1); // C_SIG_ENCRYPT_MODE_INCL
        }
        let remainder = record.len() % 8;
        if remainder != 0 {
            record.write_u8(0, 8 - remainder);
        }
        let add_record_len = record.len() / 8;
        assert!(
            add_record_len <= 7,
            "CAM ADD_RECORD_LEN exceeds 3-bit field"
        );
        bs.write_u8(add_record_len as u8, 3);
        bs.extend(&record);
        bs
    }

    /// Decode a Channel Assignment Message from an SDU bitstream.
    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, String> {
        let r = |bs: &mut Bitstream, n| bs.read_bits(n).map_err(|e| format!("CAM: {e}"));
        let assign_mode = r(bs, 3)? as u8;
        let add_record_len = r(bs, 3)? as usize;
        let record_bits = add_record_len * 8;
        if bs.len() < record_bits {
            return Err(format!(
                "CAM: ADD_RECORD_LEN={} exceeds remaining bits {}",
                add_record_len,
                bs.len()
            ));
        }
        let mut record = bs.drain(0..record_bits);
        let r = |bs: &mut Bitstream, n| bs.read_bits(n).map_err(|e| format!("CAM: {e}"));
        if assign_mode == 0b000 {
            let freq_incl = r(&mut record, 1)? != 0;
            let code_chan = r(&mut record, 8)? as u8;
            let cdma_freq = if freq_incl {
                Some(r(&mut record, 11)? as u16)
            } else {
                None
            };
            let frame_offset = r(&mut record, 4)? as u8;
            let encrypt_mode = r(&mut record, 2)? as u8;
            if encrypt_mode == 0b11 {
                let _d_sig_encrypt_mode = r(&mut record, 3)?;
            }
            if matches!(encrypt_mode, 0b10 | 0b11) {
                let _enc_key_size = r(&mut record, 3)?;
            }
            let c_sig_encrypt_mode_incl = r(&mut record, 1)? != 0;
            if c_sig_encrypt_mode_incl {
                let _c_sig_encrypt_mode = r(&mut record, 3)?;
            }
            Ok(Self {
                assign_mode,
                freq_incl,
                code_chan,
                frame_offset,
                encrypt_mode,
                band_class: None,
                cdma_freq,
                bypass_alert_answer: None,
                default_config: None,
                granted_mode: None,
                plcm_type_incl: None,
                plcm_type: None,
            })
        } else if assign_mode == 0b100 {
            let freq_incl = r(&mut record, 1)? != 0;
            let _reserved = r(&mut record, 3)?;
            let bypass_alert_answer = r(&mut record, 1)? != 0;
            let default_config = r(&mut record, 3)? as u8;
            let granted_mode = r(&mut record, 2)? as u8;
            let code_chan = r(&mut record, 8)? as u8;
            let frame_offset = r(&mut record, 4)? as u8;
            let encrypt_mode = r(&mut record, 2)? as u8;
            let (band_class, cdma_freq) = if freq_incl {
                (
                    Some(r(&mut record, 5)? as u8),
                    Some(r(&mut record, 11)? as u16),
                )
            } else {
                (None, None)
            };
            if encrypt_mode == 0b11 {
                let _d_sig_encrypt_mode = r(&mut record, 3)?;
            }
            if matches!(encrypt_mode, 0b10 | 0b11) {
                let _enc_key_size = r(&mut record, 3)?;
            }
            let c_sig_encrypt_mode_incl = r(&mut record, 1)? != 0;
            if c_sig_encrypt_mode_incl {
                let _c_sig_encrypt_mode = r(&mut record, 3)?;
            }
            Ok(Self {
                assign_mode,
                freq_incl,
                code_chan,
                frame_offset,
                encrypt_mode,
                band_class,
                cdma_freq,
                bypass_alert_answer: Some(bypass_alert_answer),
                default_config: Some(default_config),
                granted_mode: Some(granted_mode),
                plcm_type_incl: None,
                plcm_type: None,
            })
        } else {
            Err(format!("CAM: unsupported assign_mode={}", assign_mode))
        }
    }
}

#[cfg(test)]
mod channel_assignment_tests {
    use super::*;

    #[test]
    fn cam_assign_mode_000_round_trips_same_frequency_rc1_assignment() {
        let cam = ChannelAssignmentMessage::new_traffic_assignment(10, 3);
        let mut sdu = cam.to_sdu();

        assert_eq!(sdu.len(), 22);
        assert_eq!(sdu.read_bits(3).unwrap(), 0b000);
        assert_eq!(sdu.read_bits(3).unwrap(), 2); // ADD_RECORD_LEN
        assert_eq!(sdu.read_bits(1).unwrap(), 0);
        assert_eq!(sdu.read_bits(8).unwrap(), 10);
        assert_eq!(sdu.read_bits(4).unwrap(), 3);
        assert_eq!(sdu.read_bits(2).unwrap(), 0);
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // C_SIG_ENCRYPT_MODE_INCL

        let mut decode_sdu = cam.to_sdu();
        let decoded = ChannelAssignmentMessage::from_sdu(&mut decode_sdu).unwrap();
        assert_eq!(decoded.assign_mode, 0b000);
        assert!(!decoded.freq_incl);
        assert_eq!(decoded.code_chan, 10);
        assert_eq!(decoded.frame_offset, 3);
        assert_eq!(decoded.encrypt_mode, 0);
        assert_eq!(decoded.default_config, None);
    }

    #[test]
    fn cam_assign_mode_000_includes_frequency_when_requested() {
        let mut cam = ChannelAssignmentMessage::new_traffic_assignment(11, 0);
        cam.freq_incl = true;
        cam.cdma_freq = Some(384);

        let mut sdu = cam.to_sdu();
        assert_eq!(sdu.read_bits(3).unwrap(), 0b000);
        assert_eq!(sdu.read_bits(3).unwrap(), 4); // ADD_RECORD_LEN
        assert_eq!(sdu.read_bits(1).unwrap(), 1);
        assert_eq!(sdu.read_bits(8).unwrap(), 11);
        assert_eq!(sdu.read_bits(11).unwrap(), 384);

        let mut decode_sdu = cam.to_sdu();
        let decoded = ChannelAssignmentMessage::from_sdu(&mut decode_sdu).unwrap();
        assert_eq!(decoded.assign_mode, 0b000);
        assert!(decoded.freq_incl);
        assert_eq!(decoded.band_class, None);
        assert_eq!(decoded.cdma_freq, Some(384));
        assert_eq!(decoded.code_chan, 11);
    }

    #[test]
    fn cam_assign_mode_100_uses_add_record_len_and_no_plcm_fields() {
        let cam = ChannelAssignmentMessage::new_extended_traffic_assignment(12, 4, 0b000);
        let mut sdu = cam.to_sdu();

        assert_eq!(sdu.len(), 38);
        assert_eq!(sdu.read_bits(3).unwrap(), 0b100);
        assert_eq!(sdu.read_bits(3).unwrap(), 4); // ADD_RECORD_LEN
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // FREQ_INCL
        assert_eq!(sdu.read_bits(3).unwrap(), 0); // RESERVED
        assert_eq!(sdu.read_bits(1).unwrap(), 1); // BYPASS_ALERT_ANSWER
        assert_eq!(sdu.read_bits(3).unwrap(), 0); // DEFAULT_CONFIG
        assert_eq!(sdu.read_bits(2).unwrap(), 0); // GRANTED_MODE
        assert_eq!(sdu.read_bits(8).unwrap(), 12); // CODE_CHAN
        assert_eq!(sdu.read_bits(4).unwrap(), 4); // FRAME_OFFSET
        assert_eq!(sdu.read_bits(2).unwrap(), 0); // ENCRYPT_MODE
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // C_SIG_ENCRYPT_MODE_INCL
        assert_eq!(sdu.read_bits(7).unwrap(), 0); // RESERVED padding

        let mut decode_sdu = cam.to_sdu();
        let decoded = ChannelAssignmentMessage::from_sdu(&mut decode_sdu).unwrap();
        assert_eq!(decoded.assign_mode, 0b100);
        assert_eq!(decoded.code_chan, 12);
        assert_eq!(decoded.frame_offset, 4);
        assert_eq!(decoded.default_config, Some(0));
        assert_eq!(decoded.granted_mode, Some(0));
        assert_eq!(decoded.plcm_type_incl, None);
    }
}

#[derive(Clone, Debug)]
pub struct ExtendedTrafficPilotRecord {
    pub pilot_pn: u16,
    pub pilot_record: Option<ExtendedPilotInfoRecord>,
    pub pwr_comb_ind: bool,
    pub code_chan_fch: u16,
    pub qof_mask_id_fch: u8,
    pub code_chan_dcch: Option<u16>,
    pub qof_mask_id_dcch: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct ExtendedPilotInfoRecord {
    pub pilot_rec_type: u8,
    pub type_specific_fields: Bitstream,
}

#[derive(Clone, Debug)]
pub struct EcamMessageIntegrityInfo {
    pub change_keys: bool,
    pub use_uak: bool,
}

#[derive(Clone, Debug)]
pub struct ExtendedChannelAssignmentMessage {
    pub assign_mode: u8,
    pub direct_ch_assign_ind: bool,
    pub raw_additional_record_fields: Option<Bitstream>,
    pub freq_incl: bool,
    pub band_class: Option<u8>,
    pub cdma_freq: Option<u16>,
    pub bypass_alert_answer: bool,
    pub granted_mode: u8,
    pub sr_id_restore: Option<u8>,
    pub sr_id_restore_bitmap: Option<u8>,
    pub default_config: u8,
    pub for_rc: u8,
    pub rev_rc: u8,
    pub frame_offset: u8,
    pub encrypt_mode: u8,
    pub d_sig_encrypt_mode: Option<u8>,
    pub enc_key_size: Option<u8>,
    pub fpc_subchan_gain: u8,
    pub rlgain_adj: i8,
    pub ch_ind: u8,
    pub raw_ch_record_fields: Option<Bitstream>,
    pub fpc_fch_init_setpt: u8,
    pub fpc_fch_fer: u8,
    pub fpc_fch_min_setpt: u8,
    pub fpc_fch_max_setpt: u8,
    pub fpc_dcch_init_setpt: u8,
    pub fpc_dcch_fer: u8,
    pub fpc_dcch_min_setpt: u8,
    pub fpc_dcch_max_setpt: u8,
    pub fpc_pri_chan: bool,
    pub pilots: Vec<ExtendedTrafficPilotRecord>,
    pub rev_fch_gating_mode: bool,
    pub rev_pwr_cntl_delay: Option<u8>,
    pub c_sig_encrypt_mode: Option<u8>,
    pub one_xrl_freq_offset: Option<u8>,
    pub message_integrity: Option<EcamMessageIntegrityInfo>,
    pub plcm_type_incl: bool,
    pub plcm_type: u8,
    pub plcm_39: Option<u64>,
    pub sync_id: Option<Vec<u8>>,
    pub config_msg_seq: Option<u8>,
    pub rtc_nom_pwr: Option<i8>,
    pub respond_ind: Option<bool>,
    pub direct_ch_assign_recover_ind: Option<bool>,
    pub fixed_num_preamble: Option<u8>,
    pub early_rl_transmit_ind: bool,
    pub omit_tx_pwr_limit_incl_for_p_rev6_compat: bool,
    pub tx_pwr_limit: Option<u8>,
}

impl ExtendedChannelAssignmentMessage {
    /// Map RC pair to DEFAULT_CONFIG per C.S0005-E Table 3.7.2.3.2.21-2.
    ///
    /// With GRANTED_MODE='00':
    /// - DEFAULT_CONFIG 000-011: standard RC1/RC2 mux option + RC combos
    /// - DEFAULT_CONFIG 100: RC specified in FOR_RC/REV_RC fields, mux
    ///   option derived from RC via Table 3.7.2.3.2.21-3 (20ms frames)
    ///
    /// RC3+ always uses DEFAULT_CONFIG=100 since they have no dedicated
    /// DEFAULT_CONFIG entry.
    fn default_config_for_rcs(for_rc: u8, rev_rc: u8) -> u8 {
        match (for_rc, rev_rc) {
            (1, 1) => 0b000,
            (2, 2) => 0b001,
            (1, 2) => 0b010,
            (2, 1) => 0b011,
            _ => 0b100, // explicit RC from FOR_RC/REV_RC
        }
    }

    pub fn new_f_fch_r_fch_assignment(
        pilot_pn: u16,
        walsh_code: u8,
        frame_offset: u8,
        for_rc: u8,
        rev_rc: u8,
        early_rl_transmit_ind: bool,
    ) -> Self {
        Self {
            assign_mode: 0b100,
            direct_ch_assign_ind: false,
            raw_additional_record_fields: None,
            freq_incl: false,
            band_class: None,
            cdma_freq: None,
            bypass_alert_answer: false,
            // GRANTED_MODE=10: use FOR_RC/REV_RC fields, derive mux option
            // from RC per Table 3.7.2.3.2.21-3. Matches working Anritsu trace.
            granted_mode: 0b10,
            sr_id_restore: None,
            sr_id_restore_bitmap: None,
            default_config: Self::default_config_for_rcs(for_rc, rev_rc),
            for_rc,
            rev_rc,
            frame_offset,
            encrypt_mode: 0b00,
            d_sig_encrypt_mode: None,
            enc_key_size: None,
            // FPC values matched to working Anritsu MD8470A trace (RecNo 50).
            fpc_subchan_gain: 12,
            // R-FCH traffic-to-pilot gain adjustment, signed 4-bit two's
            // complement in 0.25 dB units. **LOCK-STEP** with the inner-loop
            // SINR setpoint in power_control.rs (calibrated via
            // `rc3_pilot_sinr_at_1pct_fer_calibration`). Re-run before changing.
            rlgain_adj: 0,
            ch_ind: 0b01,
            raw_ch_record_fields: None,
            fpc_fch_init_setpt: 0x20, // 32 (4.0 dB)
            fpc_fch_fer: 0b00010,     // 1% FER target
            fpc_fch_min_setpt: 0x00,  // 0.0 dB
            fpc_fch_max_setpt: 0x50,  // 80 (10.0 dB)
            fpc_dcch_init_setpt: 0,
            fpc_dcch_fer: 0,
            fpc_dcch_min_setpt: 0,
            fpc_dcch_max_setpt: 0,
            fpc_pri_chan: false,
            pilots: vec![ExtendedTrafficPilotRecord {
                pilot_pn,
                pilot_record: None,
                pwr_comb_ind: false,
                code_chan_fch: walsh_code as u16,
                qof_mask_id_fch: 0,
                code_chan_dcch: None,
                qof_mask_id_dcch: None,
            }],
            rev_fch_gating_mode: false,
            rev_pwr_cntl_delay: None,
            c_sig_encrypt_mode: None,
            one_xrl_freq_offset: None,
            message_integrity: None,
            plcm_type_incl: false,
            plcm_type: 0,
            plcm_39: None,
            sync_id: None,
            config_msg_seq: None,
            rtc_nom_pwr: None,
            respond_ind: None,
            direct_ch_assign_recover_ind: None,
            fixed_num_preamble: None,
            early_rl_transmit_ind,
            omit_tx_pwr_limit_incl_for_p_rev6_compat: false,
            tx_pwr_limit: None,
        }
    }

    /// Returns the assignment record SDU (L3 content).
    /// The LAC layer wraps this with RESERVED_1(1) + ADD_RECORD_LEN(8)
    /// per C.S0004-E 3.1.2.3.1 for ECAM PDUs.
    pub fn to_sdu(&self) -> Bitstream {
        self.try_to_sdu().expect("invalid ECAM")
    }

    pub fn try_to_sdu(&self) -> Result<Bitstream, crate::error::Error> {
        let mut bs = Bitstream::new();
        bs.write_u8(self.assign_mode, 3);

        if self.assign_mode == 0b100 || self.assign_mode == 0b101 {
            bs.write_u8(self.direct_ch_assign_ind as u8, 1);
            bs.write_u8(0, 4); // RESERVED_2
        } else {
            bs.write_u8(0, 5); // RESERVED_2
        }

        if self.assign_mode != 0b100 {
            let raw = self.raw_additional_record_fields.as_ref().ok_or_else(|| {
                format!(
                    "ECAM encoder requires raw_additional_record_fields for ASSIGN_MODE={:03b}",
                    self.assign_mode
                )
            })?;
            bs.extend(raw);
            pad_to_octet(&mut bs);
            return Ok(bs);
        }

        bs.write_u8(self.freq_incl as u8, 1);
        if self.freq_incl {
            bs.write_u8(self.band_class.unwrap_or(0), 5);
            bs.write_u32(self.cdma_freq.unwrap_or(0) as u32, 11);
        }

        // If the mobile station is to bypass the Waiting for Order
        // Substate and the Waiting for Mobile Station Answer Substate,
        //the base station shall set this field to ‘1’; otherwise, the base
        // station shall set this field to ‘0’.
        bs.write_u8(self.bypass_alert_answer as u8, 1);
        bs.write_u8(self.granted_mode, 2);

        if self.granted_mode == 0b11 {
            let sr_id_restore = self
                .sr_id_restore
                .ok_or("ECAM encoder requires SR_ID_RESTORE when GRANTED_MODE=11")?;
            bs.write_u8(sr_id_restore, 3);
            if sr_id_restore == 0 {
                bs.write_u8(self.sr_id_restore_bitmap.unwrap_or(0), 6);
            }
        }

        // takes effect if GRANTED_MODE = '00'
        bs.write_u8(self.default_config, 3);
        // FOR_RC and REV_RC are ALWAYS present for ASSIGN_MODE='100'
        // per C.S0005-E 3.7.2.3.2.21 page 3-219.
        // When GRANTED_MODE=00 and DEFAULT_CONFIG != 0b100, spec
        // requires FOR_RC to be '00001' (RC1) or '00010' (RC2).
        bs.write_u8(self.for_rc, 5);
        bs.write_u8(self.rev_rc, 5);
        bs.write_u8(self.frame_offset, 4);
        bs.write_u8(self.encrypt_mode, 2);
        bs.write_u8(self.fpc_subchan_gain, 5);
        bs.write_u8(encode_signed_nbits(self.rlgain_adj, 4), 4);

        let num_pilots = self
            .pilots
            .len()
            .checked_sub(1)
            .expect("ECAM requires at least one pilot");
        bs.write_u8(num_pilots as u8, 3);
        bs.write_u8(self.ch_ind, 2);

        let ch_record_fields = self.build_ch_record_fields();
        let ch_record_len_octets = ch_record_fields.len().div_ceil(8);
        bs.write_u8(ch_record_len_octets as u8, 5);
        bs.extend(&ch_record_fields);

        if self.ch_ind == 0b01 || self.ch_ind == 0b11 {
            bs.write_u8(self.rev_fch_gating_mode as u8, 1);
            if self.rev_fch_gating_mode {
                bs.write_u8(self.rev_pwr_cntl_delay.is_some() as u8, 1);
                if let Some(delay) = self.rev_pwr_cntl_delay {
                    bs.write_u8(delay, 2);
                }
            }
        }

        if self.encrypt_mode == 0b11 {
            bs.write_u8(self.d_sig_encrypt_mode.unwrap_or(0), 3);
        }
        if self.encrypt_mode == 0b10 || self.encrypt_mode == 0b11 {
            bs.write_u8(self.enc_key_size.unwrap_or(0), 3);
        }

        bs.write_u8(self.c_sig_encrypt_mode.is_some() as u8, 1); // C_SIG_ENCRYPT_MODE_INCL
        if let Some(mode) = self.c_sig_encrypt_mode {
            bs.write_u8(mode, 3);
        }
        bs.write_u8(self.one_xrl_freq_offset.is_some() as u8, 1); // 3XFL_1XRL_INCL
        if let Some(offset) = self.one_xrl_freq_offset {
            bs.write_u8(offset, 2);
        }
        bs.write_u8(self.message_integrity.is_some() as u8, 1); // MSG_INT_INFO_INCL
        if let Some(info) = &self.message_integrity {
            bs.write_u8(info.change_keys as u8, 1);
            bs.write_u8(info.use_uak as u8, 1);
        }
        bs.write_u8(self.plcm_type_incl as u8, 1); // PLCM_TYPE_INCL
        if self.plcm_type_incl {
            bs.write_u8(self.plcm_type, 4);
            if self.plcm_type == 0b0001 {
                bs.write_u64(self.plcm_39.unwrap_or(0), 39);
            }
        }
        if self.granted_mode == 0b11 {
            bs.write_u8(self.sync_id.is_some() as u8, 1);
            if let Some(sync_id) = &self.sync_id {
                bs.write_u8(sync_id.len() as u8, 4);
                for byte in sync_id {
                    bs.write_u8(*byte, 8);
                }
            }
        }
        if self.direct_ch_assign_ind {
            bs.write_u8(self.config_msg_seq.unwrap_or(0), 6);
            bs.write_u8(encode_signed_nbits(self.rtc_nom_pwr.unwrap_or(0), 5), 5);
            bs.write_u8(self.respond_ind.unwrap_or(false) as u8, 1);
            bs.write_u8(self.direct_ch_assign_recover_ind.unwrap_or(false) as u8, 1);
        }
        if self.granted_mode == 0b11 {
            bs.write_u8(self.fixed_num_preamble.is_some() as u8, 1);
            if let Some(num_preamble) = self.fixed_num_preamble {
                bs.write_u8(num_preamble, 3);
            }
        }
        bs.write_u8(self.early_rl_transmit_ind as u8, 1);

        // TX_PWR_LIMIT_INCL is a fixed ASSIGN_MODE=100 field in C.S0005-E.
        // Some P_REV 6 traces omit it; keep that as an explicit compatibility
        // mode instead of making the canonical encoder optional by default.
        if !self.omit_tx_pwr_limit_incl_for_p_rev6_compat {
            bs.write_u8(self.tx_pwr_limit.is_some() as u8, 1);
            if let Some(limit) = self.tx_pwr_limit {
                bs.write_u8(limit, 6);
            }
        }

        pad_to_octet(&mut bs);
        Ok(bs)
    }

    pub fn describe(&self) -> String {
        format!(
            concat!(
                "assign_mode=0b{:03b} direct_ch_assign_ind={} freq_incl={} ",
                "band_class={:?} cdma_freq={:?} bypass_alert_answer={} granted_mode=0b{:02b} ",
                "default_config=0b{:03b} for_rc={} rev_rc={} frame_offset={} encrypt_mode=0b{:02b} ",
                "fpc_subchan_gain={} rlgain_adj={} ch_ind=0b{:02b} ch_record_len_octets={} ",
                "fpc_fch_init_setpt=0x{:02X} fpc_fch_fer=0b{:05b} fpc_fch_min_setpt=0x{:02X} ",
                "fpc_fch_max_setpt=0x{:02X} rev_fch_gating_mode={} plcm_type_incl={} plcm_type={} early_rl_transmit_ind={} ",
                "tx_pwr_limit={:?} pilots=[{}]"
            ),
            self.assign_mode,
            self.direct_ch_assign_ind,
            self.freq_incl,
            self.band_class,
            self.cdma_freq,
            self.bypass_alert_answer,
            self.granted_mode,
            self.default_config,
            self.for_rc,
            self.rev_rc,
            self.frame_offset,
            self.encrypt_mode,
            self.fpc_subchan_gain,
            self.rlgain_adj,
            self.ch_ind,
            self.build_ch_record_fields().len().div_ceil(8),
            self.fpc_fch_init_setpt,
            self.fpc_fch_fer,
            self.fpc_fch_min_setpt,
            self.fpc_fch_max_setpt,
            self.rev_fch_gating_mode,
            self.plcm_type_incl,
            self.plcm_type,
            self.early_rl_transmit_ind,
            self.tx_pwr_limit,
            self.pilots
                .iter()
                .map(|pilot| {
                    format!(
                        "{{pilot_pn={} pwr_comb_ind={} code_chan_fch={} qof_mask_id_fch={}}}",
                        pilot.pilot_pn,
                        pilot.pwr_comb_ind,
                        pilot.code_chan_fch,
                        pilot.qof_mask_id_fch
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// Decode an ECAM assignment record from a bitstream (inverse of `to_sdu`).
    ///
    /// ASSIGN_MODE=100 is decoded into typed fields. Other assignment modes
    /// are preserved as raw additional record fields so they can be re-encoded
    /// byte-for-byte even when this struct does not model the mode-specific
    /// Layer 3 fields yet.
    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let assign_mode = bs.read_bits(3)? as u8;
        let direct_ch_assign_ind = if assign_mode == 0b100 || assign_mode == 0b101 {
            let direct = bs.read_bits(1)? != 0;
            Self::read_reserved_bits(bs, 4, "ECAM RESERVED_2")?;
            direct
        } else {
            Self::read_reserved_bits(bs, 5, "ECAM RESERVED_2")?;
            false
        };

        if assign_mode != 0b100 {
            let raw_additional_record_fields = Self::drain_remaining_bits(bs)?;
            let mut msg = Self::new_f_fch_r_fch_assignment(0, 0, 0, 1, 1, false);
            msg.assign_mode = assign_mode;
            msg.direct_ch_assign_ind = direct_ch_assign_ind;
            msg.raw_additional_record_fields = Some(raw_additional_record_fields);
            return Ok(msg);
        }

        let freq_incl = bs.read_bits(1)? != 0;
        let (band_class, cdma_freq) = if freq_incl {
            let bc = bs.read_bits(5)? as u8;
            let cf = bs.read_bits(11)? as u16;
            (Some(bc), Some(cf))
        } else {
            (None, None)
        };

        let bypass_alert_answer = bs.read_bits(1)? != 0;
        let granted_mode = bs.read_bits(2)? as u8;

        let mut sr_id_restore = None;
        let mut sr_id_restore_bitmap = None;
        if granted_mode == 0b11 {
            let sr_id = bs.read_bits(3)? as u8;
            sr_id_restore = Some(sr_id);
            if sr_id == 0 {
                sr_id_restore_bitmap = Some(bs.read_bits(6)? as u8);
            }
        }

        let default_config = bs.read_bits(3)? as u8;
        let for_rc = bs.read_bits(5)? as u8;
        let rev_rc = bs.read_bits(5)? as u8;
        let frame_offset = bs.read_bits(4)? as u8;
        let encrypt_mode = bs.read_bits(2)? as u8;
        let fpc_subchan_gain = bs.read_bits(5)? as u8;
        let rlgain_adj_raw = bs.read_bits(4)? as u8;
        let rlgain_adj = decode_signed_nbits(rlgain_adj_raw, 4);

        let num_pilots_minus1 = bs.read_bits(3)? as usize;
        let ch_ind = bs.read_bits(2)? as u8;

        let ch_record_len_octets = bs.read_bits(5)? as usize;
        let ch_record_len_bits = ch_record_len_octets * 8;

        let raw_ch_record_fields = Self::read_bitstream(bs, ch_record_len_bits)?;
        let mut ch_bs = raw_ch_record_fields.clone();
        let mut fpc_fch_init_setpt = 0u8;
        let mut fpc_fch_fer = 0u8;
        let mut fpc_fch_min_setpt = 0u8;
        let mut fpc_fch_max_setpt = 0u8;
        let mut fpc_dcch_init_setpt = 0u8;
        let mut fpc_dcch_fer = 0u8;
        let mut fpc_dcch_min_setpt = 0u8;
        let mut fpc_dcch_max_setpt = 0u8;
        let mut fpc_pri_chan = false;
        let mut pilots = Vec::new();

        if ch_ind == 0b01 {
            fpc_fch_init_setpt = ch_bs.read_bits(8)? as u8;
            fpc_fch_fer = ch_bs.read_bits(5)? as u8;
            fpc_fch_min_setpt = ch_bs.read_bits(8)? as u8;
            fpc_fch_max_setpt = ch_bs.read_bits(8)? as u8;

            for _ in 0..=num_pilots_minus1 {
                pilots.push(Self::read_ecam_fch_pilot(&mut ch_bs, false)?);
            }

            let three_x_fch_info_incl = ch_bs.read_bits(1)? != 0;
            if three_x_fch_info_incl {
                for _ in 0..=num_pilots_minus1 {
                    Self::skip_three_x_chan_record(&mut ch_bs)?;
                }
            }
        } else if ch_ind == 0b10 {
            fpc_dcch_init_setpt = ch_bs.read_bits(8)? as u8;
            fpc_dcch_fer = ch_bs.read_bits(5)? as u8;
            fpc_dcch_min_setpt = ch_bs.read_bits(8)? as u8;
            fpc_dcch_max_setpt = ch_bs.read_bits(8)? as u8;

            for _ in 0..=num_pilots_minus1 {
                pilots.push(Self::read_ecam_dcch_pilot(&mut ch_bs, false)?);
            }

            let three_x_dcch_info_incl = ch_bs.read_bits(1)? != 0;
            if three_x_dcch_info_incl {
                for _ in 0..=num_pilots_minus1 {
                    Self::skip_three_x_chan_record(&mut ch_bs)?;
                }
            }
            let fundicated_bcmc_ind = ch_bs.read_bits(1)? != 0;
            if fundicated_bcmc_ind {
                for _ in 0..=num_pilots_minus1 {
                    let _for_cpcch_walsh = ch_bs.read_bits(7)?;
                    let _for_cpcsch = ch_bs.read_bits(5)?;
                }
            }
        } else if ch_ind == 0b11 {
            fpc_fch_init_setpt = ch_bs.read_bits(8)? as u8;
            fpc_dcch_init_setpt = ch_bs.read_bits(8)? as u8;
            fpc_pri_chan = ch_bs.read_bits(1)? != 0;
            fpc_fch_fer = ch_bs.read_bits(5)? as u8;
            fpc_fch_min_setpt = ch_bs.read_bits(8)? as u8;
            fpc_fch_max_setpt = ch_bs.read_bits(8)? as u8;
            fpc_dcch_fer = ch_bs.read_bits(5)? as u8;
            fpc_dcch_min_setpt = ch_bs.read_bits(8)? as u8;
            fpc_dcch_max_setpt = ch_bs.read_bits(8)? as u8;

            for _ in 0..=num_pilots_minus1 {
                pilots.push(Self::read_ecam_fch_pilot(&mut ch_bs, true)?);
            }

            let three_x_fch_info_incl = ch_bs.read_bits(1)? != 0;
            if three_x_fch_info_incl {
                for _ in 0..=num_pilots_minus1 {
                    Self::skip_three_x_chan_record(&mut ch_bs)?;
                }
            }
            let three_x_dcch_info_incl = ch_bs.read_bits(1)? != 0;
            if three_x_dcch_info_incl {
                for _ in 0..=num_pilots_minus1 {
                    Self::skip_three_x_chan_record(&mut ch_bs)?;
                }
            }
            let fundicated_bcmc_ind = ch_bs.read_bits(1)? != 0;
            if fundicated_bcmc_ind {
                let _rev_fch_assigned = ch_bs.read_bits(1)?;
                let add_plcm_for_fch_incl = ch_bs.read_bits(1)? != 0;
                if add_plcm_for_fch_incl {
                    let _add_plcm_for_fch_type = ch_bs.read_bits(1)?;
                    let _add_plcm_for_fch_39 = ch_bs.read_bits(39)?;
                }
                let for_cpcch_info_incl = ch_bs.read_bits(1)? != 0;
                if for_cpcch_info_incl {
                    for _ in 0..=num_pilots_minus1 {
                        let _for_cpcch_walsh = ch_bs.read_bits(7)?;
                        let _for_cpcsch = ch_bs.read_bits(5)?;
                    }
                }
            }
        }
        Self::read_zero_tail(&mut ch_bs, "ECAM CH_RECORD_FIELDS RESERVED")?;

        let mut rev_fch_gating_mode = false;
        let mut rev_pwr_cntl_delay = None;
        if ch_ind == 0b01 || ch_ind == 0b11 {
            rev_fch_gating_mode = bs.read_bits(1)? != 0;
            if rev_fch_gating_mode {
                let rev_pwr_cntl_delay_incl = bs.read_bits(1)? != 0;
                if rev_pwr_cntl_delay_incl {
                    rev_pwr_cntl_delay = Some(bs.read_bits(2)? as u8);
                }
            }
        }

        let d_sig_encrypt_mode = if encrypt_mode == 0b11 {
            Some(bs.read_bits(3)? as u8)
        } else {
            None
        };
        let enc_key_size = if encrypt_mode == 0b10 || encrypt_mode == 0b11 {
            Some(bs.read_bits(3)? as u8)
        } else {
            None
        };

        let c_sig_encrypt_mode_incl = bs.read_bits(1)? != 0;
        let c_sig_encrypt_mode = if c_sig_encrypt_mode_incl {
            Some(bs.read_bits(3)? as u8)
        } else {
            None
        };
        let three_xfl_1xrl_incl = bs.read_bits(1)? != 0;
        let one_xrl_freq_offset = if three_xfl_1xrl_incl {
            Some(bs.read_bits(2)? as u8)
        } else {
            None
        };
        let msg_int_info_incl = bs.read_bits(1)? != 0;
        let message_integrity = if msg_int_info_incl {
            Some(EcamMessageIntegrityInfo {
                change_keys: bs.read_bits(1)? != 0,
                use_uak: bs.read_bits(1)? != 0,
            })
        } else {
            None
        };

        let plcm_type_incl = bs.read_bits(1)? != 0;
        let plcm_type = if plcm_type_incl {
            bs.read_bits(4)? as u8
        } else {
            0
        };
        let plcm_39 = if plcm_type_incl && plcm_type == 0b0001 {
            Some(bs.read_bits(39)?)
        } else {
            None
        };

        let sync_id = if granted_mode == 0b11 {
            let sync_id_incl = bs.read_bits(1)? != 0;
            if sync_id_incl {
                let sync_id_len = bs.read_bits(4)? as usize;
                let mut sync_id = Vec::with_capacity(sync_id_len);
                for _ in 0..sync_id_len {
                    sync_id.push(bs.read_bits(8)? as u8);
                }
                Some(sync_id)
            } else {
                None
            }
        } else {
            None
        };

        let (config_msg_seq, rtc_nom_pwr, respond_ind, direct_ch_assign_recover_ind) =
            if direct_ch_assign_ind {
                (
                    Some(bs.read_bits(6)? as u8),
                    Some(decode_signed_nbits(bs.read_bits(5)? as u8, 5)),
                    Some(bs.read_bits(1)? != 0),
                    Some(bs.read_bits(1)? != 0),
                )
            } else {
                (None, None, None, None)
            };

        let fixed_num_preamble = if granted_mode == 0b11 {
            let fixed_preamble_transmit_ind = bs.read_bits(1)? != 0;
            if fixed_preamble_transmit_ind {
                Some(bs.read_bits(3)? as u8)
            } else {
                None
            }
        } else {
            None
        };

        let early_rl_transmit_ind = bs.read_bits(1)? != 0;
        let mut omit_tx_pwr_limit_incl_for_p_rev6_compat = true;
        let mut tx_pwr_limit = None;
        if bs.len() >= 7 {
            omit_tx_pwr_limit_incl_for_p_rev6_compat = false;
            if bs.read_bits(1)? != 0 {
                tx_pwr_limit = Some(bs.read_bits(6)? as u8);
            }
        }
        Self::read_zero_tail(bs, "ECAM RESERVED")?;

        Ok(Self {
            assign_mode,
            direct_ch_assign_ind,
            raw_additional_record_fields: None,
            freq_incl,
            band_class,
            cdma_freq,
            bypass_alert_answer,
            granted_mode,
            sr_id_restore,
            sr_id_restore_bitmap,
            default_config,
            for_rc,
            rev_rc,
            frame_offset,
            encrypt_mode,
            d_sig_encrypt_mode,
            enc_key_size,
            fpc_subchan_gain,
            rlgain_adj,
            ch_ind,
            raw_ch_record_fields: Some(raw_ch_record_fields),
            fpc_fch_init_setpt,
            fpc_fch_fer,
            fpc_fch_min_setpt,
            fpc_fch_max_setpt,
            fpc_dcch_init_setpt,
            fpc_dcch_fer,
            fpc_dcch_min_setpt,
            fpc_dcch_max_setpt,
            fpc_pri_chan,
            pilots,
            rev_fch_gating_mode,
            rev_pwr_cntl_delay,
            c_sig_encrypt_mode,
            one_xrl_freq_offset,
            message_integrity,
            plcm_type_incl,
            plcm_type,
            plcm_39,
            sync_id,
            config_msg_seq,
            rtc_nom_pwr,
            respond_ind,
            direct_ch_assign_recover_ind,
            fixed_num_preamble,
            early_rl_transmit_ind,
            omit_tx_pwr_limit_incl_for_p_rev6_compat,
            tx_pwr_limit,
        })
    }

    fn drain_remaining_bits(bs: &mut Bitstream) -> Result<Bitstream, crate::error::Error> {
        Self::read_bitstream(bs, bs.len())
    }

    fn read_bitstream(bs: &mut Bitstream, bits: usize) -> Result<Bitstream, crate::error::Error> {
        let mut out = Bitstream::new();
        for _ in 0..bits {
            out.write_u8(bs.read_bits(1)? as u8, 1);
        }
        Ok(out)
    }

    fn read_reserved_bits(
        bs: &mut Bitstream,
        bits: usize,
        context: &str,
    ) -> Result<(), crate::error::Error> {
        if bits == 0 {
            return Ok(());
        }
        let value = bs.read_bits(bits)?;
        if value != 0 {
            return Err(format!("{context} bits must be zero").into());
        }
        Ok(())
    }

    fn read_zero_tail(bs: &mut Bitstream, context: &str) -> Result<(), crate::error::Error> {
        while !bs.is_empty() {
            if bs.read_bits(1)? != 0 {
                return Err(format!("{context} bits must be zero").into());
            }
        }
        Ok(())
    }

    fn read_pilot_info_record(
        bs: &mut Bitstream,
    ) -> Result<Option<ExtendedPilotInfoRecord>, crate::error::Error> {
        if bs.read_bits(1)? == 0 {
            return Ok(None);
        }
        let pilot_rec_type = bs.read_bits(3)? as u8;
        let record_len = bs.read_bits(3)? as usize;
        let type_specific_fields = Self::read_bitstream(bs, record_len * 8)?;
        Ok(Some(ExtendedPilotInfoRecord {
            pilot_rec_type,
            type_specific_fields,
        }))
    }

    fn write_pilot_info_record(bs: &mut Bitstream, pilot: &ExtendedTrafficPilotRecord) {
        if let Some(record) = &pilot.pilot_record {
            bs.write_u8(1, 1); // ADD_PILOT_REC_INCL
            bs.write_u8(record.pilot_rec_type, 3);
            let record_len_octets = record.type_specific_fields.len().div_ceil(8);
            bs.write_u8(record_len_octets as u8, 3);
            bs.extend(&record.type_specific_fields);
            let pad_bits = record_len_octets * 8 - record.type_specific_fields.len();
            if pad_bits > 0 {
                bs.write_u8(0, pad_bits);
            }
        } else {
            bs.write_u8(0, 1); // ADD_PILOT_REC_INCL
        }
    }

    fn read_ecam_fch_pilot(
        bs: &mut Bitstream,
        include_dcch: bool,
    ) -> Result<ExtendedTrafficPilotRecord, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let pilot_record = Self::read_pilot_info_record(bs)?;
        let pwr_comb_ind = bs.read_bits(1)? != 0;
        let code_chan_fch = bs.read_bits(11)? as u16;
        let qof_mask_id_fch = bs.read_bits(2)? as u8;
        let (code_chan_dcch, qof_mask_id_dcch) = if include_dcch {
            (Some(bs.read_bits(11)? as u16), Some(bs.read_bits(2)? as u8))
        } else {
            (None, None)
        };
        Ok(ExtendedTrafficPilotRecord {
            pilot_pn,
            pilot_record,
            pwr_comb_ind,
            code_chan_fch,
            qof_mask_id_fch,
            code_chan_dcch,
            qof_mask_id_dcch,
        })
    }

    fn read_ecam_dcch_pilot(
        bs: &mut Bitstream,
        include_fch: bool,
    ) -> Result<ExtendedTrafficPilotRecord, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let pilot_record = Self::read_pilot_info_record(bs)?;
        let pwr_comb_ind = bs.read_bits(1)? != 0;
        let (code_chan_fch, qof_mask_id_fch) = if include_fch {
            (bs.read_bits(11)? as u16, bs.read_bits(2)? as u8)
        } else {
            (0, 0)
        };
        let code_chan_dcch = Some(bs.read_bits(11)? as u16);
        let qof_mask_id_dcch = Some(bs.read_bits(2)? as u8);
        Ok(ExtendedTrafficPilotRecord {
            pilot_pn,
            pilot_record,
            pwr_comb_ind,
            code_chan_fch,
            qof_mask_id_fch,
            code_chan_dcch,
            qof_mask_id_dcch,
        })
    }

    fn skip_three_x_chan_record(bs: &mut Bitstream) -> Result<(), crate::error::Error> {
        if bs.read_bits(1)? != 0 {
            let _qof_mask_id_low = bs.read_bits(2)?;
            let _code_chan_low = bs.read_bits(11)?;
        }
        if bs.read_bits(1)? != 0 {
            let _qof_mask_id_high = bs.read_bits(2)?;
            let _code_chan_high = bs.read_bits(11)?;
        }
        Ok(())
    }

    pub fn encoded_sdu_hex(&self) -> String {
        bitstream_to_hex(&self.to_sdu())
    }

    pub fn ch_record_len_octets(&self) -> u8 {
        self.build_ch_record_fields().len().div_ceil(8) as u8
    }

    fn build_ch_record_fields(&self) -> Bitstream {
        if let Some(raw) = &self.raw_ch_record_fields {
            return raw.clone();
        }

        let mut bs = Bitstream::new();

        match self.ch_ind {
            0b01 => {
                bs.write_u8(self.fpc_fch_init_setpt, 8);
                bs.write_u8(self.fpc_fch_fer, 5);
                bs.write_u8(self.fpc_fch_min_setpt, 8);
                bs.write_u8(self.fpc_fch_max_setpt, 8);
                for pilot in &self.pilots {
                    bs.write_u32(pilot.pilot_pn as u32, 9);
                    Self::write_pilot_info_record(&mut bs, pilot);
                    bs.write_u8(pilot.pwr_comb_ind as u8, 1);
                    bs.write_u32(pilot.code_chan_fch as u32, 11);
                    bs.write_u8(pilot.qof_mask_id_fch, 2);
                }
                bs.write_u8(0, 1); // 3X_FCH_INFO_INCL
            }
            0b10 => {
                bs.write_u8(self.fpc_dcch_init_setpt, 8);
                bs.write_u8(self.fpc_dcch_fer, 5);
                bs.write_u8(self.fpc_dcch_min_setpt, 8);
                bs.write_u8(self.fpc_dcch_max_setpt, 8);
                for pilot in &self.pilots {
                    bs.write_u32(pilot.pilot_pn as u32, 9);
                    Self::write_pilot_info_record(&mut bs, pilot);
                    bs.write_u8(pilot.pwr_comb_ind as u8, 1);
                    bs.write_u32(pilot.code_chan_dcch.unwrap_or(0) as u32, 11);
                    bs.write_u8(pilot.qof_mask_id_dcch.unwrap_or(0), 2);
                }
                bs.write_u8(0, 1); // 3X_DCCH_INFO_INCL
                bs.write_u8(0, 1); // FUNDICATED_BCMC_IND
            }
            0b11 => {
                bs.write_u8(self.fpc_fch_init_setpt, 8);
                bs.write_u8(self.fpc_dcch_init_setpt, 8);
                bs.write_u8(self.fpc_pri_chan as u8, 1);
                bs.write_u8(self.fpc_fch_fer, 5);
                bs.write_u8(self.fpc_fch_min_setpt, 8);
                bs.write_u8(self.fpc_fch_max_setpt, 8);
                bs.write_u8(self.fpc_dcch_fer, 5);
                bs.write_u8(self.fpc_dcch_min_setpt, 8);
                bs.write_u8(self.fpc_dcch_max_setpt, 8);
                for pilot in &self.pilots {
                    bs.write_u32(pilot.pilot_pn as u32, 9);
                    Self::write_pilot_info_record(&mut bs, pilot);
                    bs.write_u8(pilot.pwr_comb_ind as u8, 1);
                    bs.write_u32(pilot.code_chan_fch as u32, 11);
                    bs.write_u8(pilot.qof_mask_id_fch, 2);
                    bs.write_u32(pilot.code_chan_dcch.unwrap_or(0) as u32, 11);
                    bs.write_u8(pilot.qof_mask_id_dcch.unwrap_or(0), 2);
                }
                bs.write_u8(0, 1); // 3X_FCH_INFO_INCL
                bs.write_u8(0, 1); // 3X_DCCH_INFO_INCL
                bs.write_u8(0, 1); // FUNDICATED_BCMC_IND
            }
            _ => {}
        }

        pad_to_octet(&mut bs);
        bs
    }
}

fn encode_signed_nbits(value: i8, bits: u8) -> u8 {
    let mask = (1u16 << bits) - 1;
    ((value as i16) & mask as i16) as u8
}

fn decode_signed_nbits(raw: u8, bits: u8) -> i8 {
    let sign_bit = 1u8 << (bits - 1);
    if raw & sign_bit != 0 {
        (raw as i8) | -(sign_bit as i8)
    } else {
        raw as i8
    }
}

fn encode_signed_i32_nbits(value: i32, bits: u8) -> u32 {
    let min = -(1i64 << (bits - 1));
    let max = (1i64 << (bits - 1)) - 1;
    assert!(
        (min..=max).contains(&(value as i64)),
        "signed value {value} out of {bits}-bit range"
    );
    let mask = (1u64 << bits) - 1;
    ((value as i64) & mask as i64) as u32
}

fn decode_signed_i32_nbits(raw: u32, bits: u8) -> i32 {
    let sign_bit = 1u32 << (bits - 1);
    if raw & sign_bit != 0 {
        (raw as i32) | -((sign_bit as i32) << 1)
    } else {
        raw as i32
    }
}

pub fn bitstream_to_bytes(bs: &Bitstream) -> Vec<u8> {
    bs.bits()
        .chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, bit| (acc << 1) | (bit & 1)) << (8 - chunk.len()))
        .collect()
}

fn bitstream_to_hex(bs: &Bitstream) -> String {
    bitstream_to_bytes(bs)
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<Vec<_>>()
        .join("")
}

fn pad_to_octet(bs: &mut Bitstream) {
    let remainder = bs.len() % 8;
    if remainder != 0 {
        bs.write_u8(0, 8 - remainder);
    }
}

#[derive(Clone, Debug)]
pub enum PagingChannelMessage {
    SystemParameters(SystemParametersMessage),
    AccessParameters(AccessParametersMessage),
    NeighborList(NeighborListMessage),
    CdmaChannelList(CdmaChannelListMessage),
    ExtendedSystemParameters(ExtendedSystemParametersMessage),
    GeneralPage(GeneralPageMessage),
    Order(OrderMessage),
    DataBurst(ForwardDataBurstMessage),
    AuthenticationChallenge(AuthenticationChallengeMessage),
    SsdUpdate(SsdUpdateMessage),
    FeatureNotification(FeatureNotificationMessage),
    ExtendedNeighborList(ExtendedNeighborListMessage),
    StatusRequest(StatusRequestMessage),
    ServiceRedirection(ServiceRedirectionMessage),
    GlobalServiceRedirection(GlobalServiceRedirectionMessage),
    TmsiAssignment(TmsiAssignmentMessage),
    Paca(PacaMessage),
    GeneralNeighborList(GeneralNeighborListMessage),
    UserZoneIdentification(UserZoneIdentificationMessage),
    PrivateNeighborList(PrivateNeighborListMessage),
    ExtendedGlobalServiceRedirection(ExtendedGlobalServiceRedirectionMessage),
    ExtendedCdmaChannelList(ExtendedCdmaChannelListMessage),
    UserZoneReject(UserZoneRejectMessage),
    Ansi41SystemParameters(Ansi41SystemParametersMessage),
    McRrParameters(McRrParametersMessage),
    Ansi41Rand(Ansi41RandMessage),
    EnhancedAccessParameters(EnhancedAccessParametersMessage),
    UniversalNeighborList(UniversalNeighborListMessage),
    SecurityModeCommand(SecurityModeCommandMessage),
    UniversalPage(UniversalPageMessage),
    UniversalPageFirstSegment(UniversalPageSegmentMessage),
    UniversalPageMiddleSegment(UniversalPageSegmentMessage),
    UniversalPageFinalSegment(UniversalPageSegmentMessage),
    AuthenticationRequest(AuthenticationRequestMessage),
    AlternativeTechnologiesInformation(AlternativeTechnologiesInformationMessage),
    GeneralExtension(ForwardGeneralExtensionMessage),
    GeneralOverheadInformation(GeneralOverheadInformationMessage),
    AccessPointIdentifier(AccessPointIdentifierMessage),
    AccessPointIdentifierText(AccessPointIdentifierTextMessage),
    AccessPointPilotInformation(AccessPointPilotInformationMessage),
    FlexDuplexCdmaChannelList(FlexDuplexCdmaChannelListMessage),
    BroadcastServiceParameters(BroadcastServiceParametersMessage),
    ChannelAssignment(ChannelAssignmentMessage),
    ExtendedChannelAssignment(ExtendedChannelAssignmentMessage),
}

impl PagingChannelMessage {
    pub fn message_id(&self) -> MessageId {
        match self {
            Self::SystemParameters(_) => MessageId::SystemParameters,
            Self::AccessParameters(_) => MessageId::AccessParameters,
            Self::NeighborList(_) => MessageId::NeighborList,
            Self::CdmaChannelList(_) => MessageId::CdmaChannelList,
            Self::ExtendedSystemParameters(_) => MessageId::ExtSystemParameters,
            Self::GeneralPage(_) => MessageId::GeneralPage,
            Self::Order(_) => MessageId::Order,
            Self::DataBurst(_) => MessageId::DataBurst,
            Self::AuthenticationChallenge(_) => MessageId::AuthChallenge,
            Self::SsdUpdate(_) => MessageId::SsdUpdate,
            Self::FeatureNotification(_) => MessageId::FeatureNotification,
            Self::ExtendedNeighborList(_) => MessageId::ExtNeighborList,
            Self::StatusRequest(_) => MessageId::StatusRequest,
            Self::ServiceRedirection(_) => MessageId::ServiceRedirection,
            Self::GlobalServiceRedirection(_) => MessageId::GlobalServiceRedirection,
            Self::TmsiAssignment(_) => MessageId::TmsiAssignment,
            Self::Paca(_) => MessageId::Paca,
            Self::GeneralNeighborList(_) => MessageId::GeneralNeighborList,
            Self::UserZoneIdentification(_) => MessageId::UserZoneIdentification,
            Self::PrivateNeighborList(_) => MessageId::PrivateNeighborList,
            Self::ExtendedGlobalServiceRedirection(_) => MessageId::ExtGlobalServiceRedirection,
            Self::ExtendedCdmaChannelList(_) => MessageId::ExtCdmaChannelList,
            Self::UserZoneReject(_) => MessageId::UserZoneReject,
            Self::Ansi41SystemParameters(_) => MessageId::Ansi41SystemParameters,
            Self::McRrParameters(_) => MessageId::McRrParameters,
            Self::Ansi41Rand(_) => MessageId::Ansi41Rand,
            Self::EnhancedAccessParameters(_) => MessageId::EnhancedAccessParameters,
            Self::UniversalNeighborList(_) => MessageId::UniversalNeighborList,
            Self::SecurityModeCommand(_) => MessageId::SecurityModeCommand,
            Self::UniversalPage(_) => MessageId::UniversalPage,
            Self::UniversalPageFirstSegment(_) => MessageId::UniversalPageFirstSegment,
            Self::UniversalPageMiddleSegment(_) => MessageId::UniversalPageMiddleSegment,
            Self::UniversalPageFinalSegment(_) => MessageId::UniversalPageFinalSegment,
            Self::AuthenticationRequest(_) => MessageId::AuthenticationRequest,
            Self::AlternativeTechnologiesInformation(_) => {
                MessageId::AlternativeTechnologiesInformation
            }
            Self::GeneralExtension(_) => MessageId::GeneralExtension,
            Self::GeneralOverheadInformation(_) => MessageId::GeneralOverheadInformation,
            Self::AccessPointIdentifier(_) => MessageId::AccessPointIdentifier,
            Self::AccessPointIdentifierText(_) => MessageId::AccessPointIdentifierText,
            Self::AccessPointPilotInformation(_) => MessageId::AccessPointPilotInformation,
            Self::FlexDuplexCdmaChannelList(_) => MessageId::FlexDuplexCdmaChannelList,
            Self::BroadcastServiceParameters(_) => MessageId::BroadcastServiceParameters,
            Self::ChannelAssignment(_) => MessageId::ChannelAssignment,
            Self::ExtendedChannelAssignment(_) => MessageId::ExtChannelAssignment,
        }
    }

    pub fn to_sdu(&self) -> Bitstream {
        match self {
            Self::SystemParameters(m) => m.to_sdu(),
            Self::AccessParameters(m) => m.to_sdu(),
            Self::NeighborList(m) => m.to_sdu(),
            Self::CdmaChannelList(m) => m.to_sdu(),
            Self::ExtendedSystemParameters(m) => m.to_sdu(),
            Self::GeneralPage(m) => m.to_sdu(),
            Self::Order(m) => m.to_sdu(),
            Self::DataBurst(m) => m.to_sdu(),
            Self::AuthenticationChallenge(m) => m.to_sdu(),
            Self::SsdUpdate(m) => m.to_sdu(),
            Self::FeatureNotification(m) => m.to_sdu(),
            Self::ExtendedNeighborList(m) => m.to_sdu(),
            Self::StatusRequest(m) => m.to_sdu(),
            Self::ServiceRedirection(m) => m.to_sdu(),
            Self::GlobalServiceRedirection(m) => m.to_sdu(),
            Self::TmsiAssignment(m) => m.to_sdu(),
            Self::Paca(m) => m.to_sdu(),
            Self::GeneralNeighborList(m) => m.to_sdu(),
            Self::UserZoneIdentification(m) => m.to_sdu(),
            Self::PrivateNeighborList(m) => m.to_sdu(),
            Self::ExtendedGlobalServiceRedirection(m) => m.to_sdu(),
            Self::ExtendedCdmaChannelList(m) => m.to_sdu(),
            Self::UserZoneReject(m) => m.to_sdu(),
            Self::Ansi41SystemParameters(m) => m.to_sdu(),
            Self::McRrParameters(m) => m.to_sdu(),
            Self::Ansi41Rand(m) => m.to_sdu(),
            Self::EnhancedAccessParameters(m) => m.to_sdu(),
            Self::UniversalNeighborList(m) => m.to_sdu(),
            Self::SecurityModeCommand(m) => m.to_sdu(),
            Self::UniversalPage(m) => m.to_sdu(),
            Self::UniversalPageFirstSegment(m) => m.to_first_segment_sdu(),
            Self::UniversalPageMiddleSegment(m) => m.to_middle_segment_sdu(),
            Self::UniversalPageFinalSegment(m) => m.to_final_segment_sdu(),
            Self::AuthenticationRequest(m) => m.to_sdu(),
            Self::AlternativeTechnologiesInformation(m) => m.to_sdu(),
            Self::GeneralExtension(m) => m.to_sdu(),
            Self::GeneralOverheadInformation(m) => m.to_sdu(),
            Self::AccessPointIdentifier(m) => m.to_sdu(),
            Self::AccessPointIdentifierText(m) => m.to_sdu(),
            Self::AccessPointPilotInformation(m) => m.to_sdu(),
            Self::FlexDuplexCdmaChannelList(m) => m.to_sdu(),
            Self::BroadcastServiceParameters(m) => m.to_sdu(),
            Self::ChannelAssignment(m) => m.to_sdu(),
            Self::ExtendedChannelAssignment(m) => m.to_sdu(),
        }
    }

    pub fn from_sdu(
        message_id: MessageId,
        bs: &mut Bitstream,
    ) -> Result<Self, crate::error::Error> {
        Ok(match message_id {
            MessageId::SystemParameters => {
                Self::SystemParameters(SystemParametersMessage::from_sdu(bs)?)
            }
            MessageId::AccessParameters => {
                Self::AccessParameters(AccessParametersMessage::from_sdu(bs)?)
            }
            MessageId::NeighborList => Self::NeighborList(NeighborListMessage::from_sdu(bs)?),
            MessageId::CdmaChannelList => {
                Self::CdmaChannelList(CdmaChannelListMessage::from_sdu(bs)?)
            }
            MessageId::ExtSystemParameters => {
                Self::ExtendedSystemParameters(ExtendedSystemParametersMessage::from_sdu(bs)?)
            }
            MessageId::GeneralPage => Self::GeneralPage(GeneralPageMessage::from_sdu(bs)?),
            MessageId::Order => Self::Order(OrderMessage::from_sdu(bs)?),
            MessageId::DataBurst => Self::DataBurst(
                ForwardDataBurstMessage::from_sdu(bs).map_err(|e| format!("DBM: {e}"))?,
            ),
            MessageId::AuthChallenge => {
                Self::AuthenticationChallenge(AuthenticationChallengeMessage::from_sdu(bs)?)
            }
            MessageId::SsdUpdate => Self::SsdUpdate(SsdUpdateMessage::from_sdu(bs)?),
            MessageId::FeatureNotification => {
                Self::FeatureNotification(FeatureNotificationMessage::from_sdu(bs)?)
            }
            MessageId::ExtNeighborList => {
                Self::ExtendedNeighborList(ExtendedNeighborListMessage::from_sdu(bs)?)
            }
            MessageId::StatusRequest => Self::StatusRequest(StatusRequestMessage::from_sdu(bs)?),
            MessageId::ServiceRedirection => {
                Self::ServiceRedirection(ServiceRedirectionMessage::from_sdu(bs)?)
            }
            MessageId::GlobalServiceRedirection => {
                Self::GlobalServiceRedirection(GlobalServiceRedirectionMessage::from_sdu(bs)?)
            }
            MessageId::TmsiAssignment => Self::TmsiAssignment(TmsiAssignmentMessage::from_sdu(bs)?),
            MessageId::Paca => Self::Paca(PacaMessage::from_sdu(bs)?),
            MessageId::GeneralNeighborList => {
                Self::GeneralNeighborList(GeneralNeighborListMessage::from_sdu(bs)?)
            }
            MessageId::UserZoneIdentification => {
                Self::UserZoneIdentification(UserZoneIdentificationMessage::from_sdu(bs)?)
            }
            MessageId::PrivateNeighborList => {
                Self::PrivateNeighborList(PrivateNeighborListMessage::from_sdu(bs)?)
            }
            MessageId::ExtGlobalServiceRedirection => Self::ExtendedGlobalServiceRedirection(
                ExtendedGlobalServiceRedirectionMessage::from_sdu(bs)?,
            ),
            MessageId::ExtCdmaChannelList => {
                Self::ExtendedCdmaChannelList(ExtendedCdmaChannelListMessage::from_sdu(bs)?)
            }
            MessageId::UserZoneReject => Self::UserZoneReject(UserZoneRejectMessage::from_sdu(bs)?),
            MessageId::Ansi41SystemParameters => {
                Self::Ansi41SystemParameters(Ansi41SystemParametersMessage::from_sdu(bs)?)
            }
            MessageId::McRrParameters => Self::McRrParameters(McRrParametersMessage::from_sdu(bs)?),
            MessageId::Ansi41Rand => Self::Ansi41Rand(Ansi41RandMessage::from_sdu(bs)?),
            MessageId::EnhancedAccessParameters => {
                Self::EnhancedAccessParameters(EnhancedAccessParametersMessage::from_sdu(bs)?)
            }
            MessageId::UniversalNeighborList => {
                Self::UniversalNeighborList(UniversalNeighborListMessage::from_sdu(bs)?)
            }
            MessageId::SecurityModeCommand => {
                Self::SecurityModeCommand(SecurityModeCommandMessage::from_sdu(bs)?)
            }
            MessageId::UniversalPage => Self::UniversalPage(UniversalPageMessage::from_sdu(bs)?),
            MessageId::UniversalPageFirstSegment => Self::UniversalPageFirstSegment(
                UniversalPageSegmentMessage::from_first_segment_sdu(bs)?,
            ),
            MessageId::UniversalPageMiddleSegment => Self::UniversalPageMiddleSegment(
                UniversalPageSegmentMessage::from_middle_segment_sdu(bs)?,
            ),
            MessageId::UniversalPageFinalSegment => Self::UniversalPageFinalSegment(
                UniversalPageSegmentMessage::from_final_segment_sdu(bs)?,
            ),
            MessageId::AuthenticationRequest => {
                Self::AuthenticationRequest(AuthenticationRequestMessage::from_sdu(bs)?)
            }
            MessageId::AlternativeTechnologiesInformation => {
                Self::AlternativeTechnologiesInformation(
                    AlternativeTechnologiesInformationMessage::from_sdu(bs)?,
                )
            }
            MessageId::GeneralExtension => {
                Self::GeneralExtension(ForwardGeneralExtensionMessage::from_sdu(bs)?)
            }
            MessageId::GeneralOverheadInformation => {
                Self::GeneralOverheadInformation(GeneralOverheadInformationMessage::from_sdu(bs)?)
            }
            MessageId::AccessPointIdentifier => {
                Self::AccessPointIdentifier(AccessPointIdentifierMessage::from_sdu(bs)?)
            }
            MessageId::AccessPointIdentifierText => {
                Self::AccessPointIdentifierText(AccessPointIdentifierTextMessage::from_sdu(bs)?)
            }
            MessageId::AccessPointPilotInformation => {
                Self::AccessPointPilotInformation(AccessPointPilotInformationMessage::from_sdu(bs)?)
            }
            MessageId::FlexDuplexCdmaChannelList => {
                Self::FlexDuplexCdmaChannelList(FlexDuplexCdmaChannelListMessage::from_sdu(bs)?)
            }
            MessageId::BroadcastServiceParameters => {
                Self::BroadcastServiceParameters(BroadcastServiceParametersMessage::from_sdu(bs)?)
            }
            MessageId::ChannelAssignment => Self::ChannelAssignment(
                ChannelAssignmentMessage::from_sdu(bs).map_err(|e| format!("CAM: {e}"))?,
            ),
            MessageId::ExtChannelAssignment => {
                Self::ExtendedChannelAssignment(ExtendedChannelAssignmentMessage::from_sdu(bs)?)
            }
            _ => {
                return Err(
                    format!("unsupported f-csch body decode for {}", message_id.tag()).into(),
                );
            }
        })
    }

    pub fn to_data_request(&self) -> DataRequest {
        let sdu = self.to_sdu();
        DataRequest {
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                length_bits: sdu.len(),
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: self.message_id(),
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 0,
                msg_seq: 0,
                ack_req: false,
                valid_ack: false,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
            sdu,
        }
    }
}

impl SystemParametersMessage {
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u32(self.sid as u32, 15);
        bs.write_u32(self.nid as u32, 16);
        bs.write_u32(self.reg_zone as u32, 12);
        bs.write_u8(self.total_zones, 3);
        bs.write_u8(self.zone_timer, 3);
        bs.write_u8(self.mult_sids as u8, 1);
        bs.write_u8(self.mult_nids as u8, 1);
        bs.write_u32(self.base_id as u32, 16);
        bs.write_u8(self.base_class, 4);
        bs.write_u8(self.page_chan, 3);
        bs.write_u8(self.max_slot_cycle_index, 3);
        bs.write_u8(self.home_reg as u8, 1);
        bs.write_u8(self.for_sid_reg as u8, 1);
        bs.write_u8(self.for_nid_reg as u8, 1);
        bs.write_u8(self.power_up_reg as u8, 1);
        bs.write_u8(self.power_down_reg as u8, 1);
        bs.write_u8(self.parameter_reg as u8, 1);
        bs.write_u8(self.reg_prd, 7);
        bs.write_u32(self.base_lat, 22);
        bs.write_u32(self.base_long, 23);
        bs.write_u32(self.reg_dist as u32, 11);
        bs.write_u8(self.srch_win_a, 4);
        bs.write_u8(self.srch_win_n, 4);
        bs.write_u8(self.srch_win_r, 4);
        bs.write_u8(self.nghbr_max_age, 4);
        bs.write_u8(self.pwr_rep_thresh, 5);
        bs.write_u8(self.pwr_rep_frames, 4);
        bs.write_u8(self.pwr_thresh_enable as u8, 1);
        bs.write_u8(self.pwr_period_enable as u8, 1);
        bs.write_u8(self.pwr_rep_delay, 5);
        bs.write_u8(self.rescan as u8, 1);
        bs.write_u8(self.t_add, 6);
        bs.write_u8(self.t_drop, 6);
        bs.write_u8(self.t_comp, 4);
        bs.write_u8(self.t_tdrop, 4);
        bs.write_u8(self.ext_sys_parameter as u8, 1);
        bs.write_u8(self.ext_nghbr_lst as u8, 1);
        bs.write_u8(self.gen_nghbr_lst as u8, 1);
        bs.write_u8(self.global_redirect as u8, 1);
        bs.write_u8(self.pri_nghbr_lst as u8, 1);
        bs.write_u8(self.user_zone_id as u8, 1);
        bs.write_u8(self.ext_global_redirect as u8, 1);
        bs.write_u8(self.ext_chan_lst as u8, 1);
        bs.write_u8(self.t_tdrop_range_incl as u8, 1);
        if self.t_tdrop_range_incl {
            bs.write_u8(self.t_tdrop_range, 4);
        }
        bs.write_u8(self.neg_slot_cycle_index_sup as u8, 1);
        bs.write_u8(self.crrm_msg_ind as u8, 1);
        assert!(
            self.num_opt_msg_bits <= 15,
            "SPM NUM_OPT_MSG_BITS must fit in 4 bits"
        );
        assert!(
            self.num_opt_msg_bits >= 1 || !self.ap_pilot_info,
            "SPM AP_PILOT_INFO requires NUM_OPT_MSG_BITS >= 1"
        );
        assert!(
            self.num_opt_msg_bits >= 2 || !self.ap_idt,
            "SPM AP_IDT requires NUM_OPT_MSG_BITS >= 2"
        );
        assert!(
            self.num_opt_msg_bits >= 3 || !self.ap_id_text,
            "SPM AP_ID_TEXT requires NUM_OPT_MSG_BITS >= 3"
        );
        assert!(
            self.num_opt_msg_bits >= 4 || !self.gen_ovhd_inf_ind,
            "SPM GEN_OVHD_INF_IND requires NUM_OPT_MSG_BITS >= 4"
        );
        assert!(
            self.num_opt_msg_bits >= 5 || !self.fd_chan_lst_ind,
            "SPM FD_CHAN_LST_IND requires NUM_OPT_MSG_BITS >= 5"
        );
        assert!(
            self.num_opt_msg_bits >= 6 || !self.atim_ind,
            "SPM ATIM_IND requires NUM_OPT_MSG_BITS >= 6"
        );
        bs.write_u8(self.num_opt_msg_bits, 4);
        if self.num_opt_msg_bits >= 1 {
            bs.write_u8(self.ap_pilot_info as u8, 1);
        }
        if self.num_opt_msg_bits >= 2 {
            bs.write_u8(self.ap_idt as u8, 1);
        }
        if self.num_opt_msg_bits >= 3 {
            bs.write_u8(self.ap_id_text as u8, 1);
        }
        if self.num_opt_msg_bits >= 4 {
            bs.write_u8(self.gen_ovhd_inf_ind as u8, 1);
        }
        if self.num_opt_msg_bits >= 5 {
            bs.write_u8(self.fd_chan_lst_ind as u8, 1);
        }
        if self.num_opt_msg_bits >= 6 {
            bs.write_u8(self.atim_ind as u8, 1);
        }
        for _ in 6..self.num_opt_msg_bits {
            bs.write_u8(0, 1);
        }
        if self.ap_pilot_info {
            assert!(
                self.appim_period_index <= 0b101,
                "SPM APPIM_PERIOD_INDEX must be in 0..=5"
            );
            bs.write_u8(self.appim_period_index, 3);
        }
        if self.gen_ovhd_inf_ind {
            assert!(
                self.gen_ovhd_cycle_index <= 0b101,
                "SPM GEN_OVHD_CYCLE_INDEX must be in 0..=5"
            );
            bs.write_u8(self.gen_ovhd_cycle_index, 3);
        }
        if self.atim_ind {
            assert!(
                self.atim_cycle_index <= 0b101,
                "SPM ATIM_CYCLE_INDEX must be in 0..=5"
            );
            bs.write_u8(self.atim_cycle_index, 3);
        }
        bs.write_u8(self.add_loc_info_incl as u8, 1);
        assert!(
            !self.add_loc_info_incl,
            "SPM ADD_LOC_INFO not yet implemented"
        );
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let sid = bs.read_bits(15)? as u16;
        let nid = bs.read_bits(16)? as u16;
        let reg_zone = bs.read_bits(12)? as u16;
        let total_zones = bs.read_bits(3)? as u8;
        let zone_timer = bs.read_bits(3)? as u8;
        let mult_sids = bs.read_bits(1)? != 0;
        let mult_nids = bs.read_bits(1)? != 0;
        let base_id = bs.read_bits(16)? as u16;
        let base_class = bs.read_bits(4)? as u8;
        let page_chan = bs.read_bits(3)? as u8;
        let max_slot_cycle_index = bs.read_bits(3)? as u8;
        let home_reg = bs.read_bits(1)? != 0;
        let for_sid_reg = bs.read_bits(1)? != 0;
        let for_nid_reg = bs.read_bits(1)? != 0;
        let power_up_reg = bs.read_bits(1)? != 0;
        let power_down_reg = bs.read_bits(1)? != 0;
        let parameter_reg = bs.read_bits(1)? != 0;
        let reg_prd = bs.read_bits(7)? as u8;
        let base_lat = bs.read_bits(22)? as u32;
        let base_long = bs.read_bits(23)? as u32;
        let reg_dist = bs.read_bits(11)? as u16;
        let srch_win_a = bs.read_bits(4)? as u8;
        let srch_win_n = bs.read_bits(4)? as u8;
        let srch_win_r = bs.read_bits(4)? as u8;
        let nghbr_max_age = bs.read_bits(4)? as u8;
        let pwr_rep_thresh = bs.read_bits(5)? as u8;
        let pwr_rep_frames = bs.read_bits(4)? as u8;
        let pwr_thresh_enable = bs.read_bits(1)? != 0;
        let pwr_period_enable = bs.read_bits(1)? != 0;
        let pwr_rep_delay = bs.read_bits(5)? as u8;
        let rescan = bs.read_bits(1)? != 0;
        let t_add = bs.read_bits(6)? as u8;
        let t_drop = bs.read_bits(6)? as u8;
        let t_comp = bs.read_bits(4)? as u8;
        let t_tdrop = bs.read_bits(4)? as u8;
        let ext_sys_parameter = bs.read_bits(1)? != 0;
        let ext_nghbr_lst = bs.read_bits(1)? != 0;
        let gen_nghbr_lst = bs.read_bits(1)? != 0;
        let global_redirect = bs.read_bits(1)? != 0;
        let pri_nghbr_lst = bs.read_bits(1)? != 0;
        let user_zone_id = bs.read_bits(1)? != 0;
        let ext_global_redirect = bs.read_bits(1)? != 0;
        let ext_chan_lst = bs.read_bits(1)? != 0;
        // Pre-P_REV-6 traces stop at EXT_CHAN_LST; default to absent.
        let t_tdrop_range_incl = !bs.is_empty() && bs.read_bits(1)? != 0;
        let t_tdrop_range = if t_tdrop_range_incl {
            bs.read_bits(4)? as u8
        } else {
            0
        };
        let neg_slot_cycle_index_sup = !bs.is_empty() && bs.read_bits(1)? != 0;
        let crrm_msg_ind = !bs.is_empty() && bs.read_bits(1)? != 0;
        let num_opt_msg_bits = if bs.is_empty() {
            0
        } else {
            bs.read_bits(4)? as u8
        };
        let ap_pilot_info = num_opt_msg_bits >= 1 && bs.read_bits(1)? != 0;
        let ap_idt = num_opt_msg_bits >= 2 && bs.read_bits(1)? != 0;
        let ap_id_text = num_opt_msg_bits >= 3 && bs.read_bits(1)? != 0;
        let gen_ovhd_inf_ind = num_opt_msg_bits >= 4 && bs.read_bits(1)? != 0;
        let fd_chan_lst_ind = num_opt_msg_bits >= 5 && bs.read_bits(1)? != 0;
        let atim_ind = num_opt_msg_bits >= 6 && bs.read_bits(1)? != 0;
        if num_opt_msg_bits > 6 {
            for _ in 6..num_opt_msg_bits {
                if bs.read_bits(1)? != 0 {
                    return Err("SPM optional overhead reserved bits must be zero".into());
                }
            }
        }
        let appim_period_index = if ap_pilot_info {
            bs.read_bits(3)? as u8
        } else {
            0
        };
        let gen_ovhd_cycle_index = if gen_ovhd_inf_ind {
            bs.read_bits(3)? as u8
        } else {
            0
        };
        let atim_cycle_index = if atim_ind { bs.read_bits(3)? as u8 } else { 0 };
        let add_loc_info_incl = !bs.is_empty() && bs.read_bits(1)? != 0;
        if add_loc_info_incl {
            return Err(
                "SPM ADD_LOC_INFO_INCL=1 (additional location info not implemented)".into(),
            );
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            sid,
            nid,
            reg_zone,
            total_zones,
            zone_timer,
            mult_sids,
            mult_nids,
            base_id,
            base_class,
            page_chan,
            max_slot_cycle_index,
            home_reg,
            for_sid_reg,
            for_nid_reg,
            power_up_reg,
            power_down_reg,
            parameter_reg,
            reg_prd,
            base_lat,
            base_long,
            reg_dist,
            srch_win_a,
            srch_win_n,
            srch_win_r,
            nghbr_max_age,
            pwr_rep_thresh,
            pwr_rep_frames,
            pwr_thresh_enable,
            pwr_period_enable,
            pwr_rep_delay,
            rescan,
            t_add,
            t_drop,
            t_comp,
            t_tdrop,
            ext_sys_parameter,
            ext_nghbr_lst,
            gen_nghbr_lst,
            global_redirect,
            pri_nghbr_lst,
            user_zone_id,
            ext_global_redirect,
            ext_chan_lst,
            t_tdrop_range_incl,
            t_tdrop_range,
            neg_slot_cycle_index_sup,
            crrm_msg_ind,
            num_opt_msg_bits,
            ap_pilot_info,
            ap_idt,
            ap_id_text,
            gen_ovhd_inf_ind,
            fd_chan_lst_ind,
            atim_ind,
            appim_period_index,
            gen_ovhd_cycle_index,
            atim_cycle_index,
            add_loc_info_incl,
        })
    }
}

impl AccessParametersMessage {
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.acc_msg_seq, 6);
        bs.write_u8(self.acc_chan, 5);
        debug_assert!(
            (-8..=7).contains(&self.nom_pwr),
            "NOM_PWR={} dB out of 4-bit signed range [-8, 7]",
            self.nom_pwr
        );
        debug_assert!(
            (-16..=15).contains(&self.init_pwr),
            "INIT_PWR={} dB out of 5-bit signed range [-16, 15]",
            self.init_pwr
        );
        bs.write_u8((self.nom_pwr as u8) & 0x0F, 4);
        bs.write_u8((self.init_pwr as u8) & 0x1F, 5);
        bs.write_u8(self.pwr_step, 3);
        bs.write_u8(self.num_step, 4);
        bs.write_u8(self.max_cap_sz, 3);
        bs.write_u8(self.pam_sz, 4);
        bs.write_u8(self.psist_0_9, 6);
        bs.write_u8(self.psist_10, 3);
        bs.write_u8(self.psist_11, 3);
        bs.write_u8(self.psist_12, 3);
        bs.write_u8(self.psist_13, 3);
        bs.write_u8(self.psist_14, 3);
        bs.write_u8(self.psist_15, 3);
        bs.write_u8(self.msg_psist, 3);
        bs.write_u8(self.reg_psist, 3);
        bs.write_u8(self.probe_pn_ran, 4);
        bs.write_u8(self.acc_tmo, 4);
        bs.write_u8(self.probe_bkoff, 4);
        bs.write_u8(self.bkoff, 4);
        bs.write_u8(self.max_req_seq, 4);
        bs.write_u8(self.max_rsp_seq, 4);
        bs.write_u8(self.auth, 2);
        if self.auth != 0 {
            bs.write_u32(self.rand, 32);
        }
        bs.write_u8(self.nom_pwr_ext, 1);
        bs.write_u8(self.psist_emg_incl as u8, 1);
        if self.psist_emg_incl {
            bs.write_u8(self.psist_emg, 3);
        }
        bs.write_u8(self.acct_incl as u8, 1);
        if self.acct_incl {
            let acct_so_incl = !self.acct_so_records.is_empty();
            let acct_so_grp_incl = !self.acct_so_grp_records.is_empty();
            assert!(
                acct_so_incl || acct_so_grp_incl,
                "ACCT_INCL requires at least one ACCT record"
            );
            bs.write_u8(self.acct_incl_emg as u8, 1);
            bs.write_u8(self.acct_aoc_bitmap_incl as u8, 1);
            bs.write_u8(acct_so_incl as u8, 1);
            if acct_so_incl {
                bs.write_u8((self.acct_so_records.len() - 1) as u8, 4);
                for record in &self.acct_so_records {
                    if self.acct_aoc_bitmap_incl {
                        bs.write_u8(record.aoc_bitmap, 5);
                    }
                    bs.write_u32(record.service_option as u32, 16);
                }
            }
            bs.write_u8(acct_so_grp_incl as u8, 1);
            if acct_so_grp_incl {
                bs.write_u8((self.acct_so_grp_records.len() - 1) as u8, 3);
                for record in &self.acct_so_grp_records {
                    if self.acct_aoc_bitmap_incl {
                        bs.write_u8(record.aoc_bitmap, 5);
                    }
                    bs.write_u8(record.service_option_group, 5);
                }
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let acc_msg_seq = bs.read_bits(6)? as u8;
        let acc_chan = bs.read_bits(5)? as u8;
        let nom_pwr = decode_signed_nbits(bs.read_bits(4)? as u8, 4);
        let init_pwr = decode_signed_nbits(bs.read_bits(5)? as u8, 5);
        let pwr_step = bs.read_bits(3)? as u8;
        let num_step = bs.read_bits(4)? as u8;
        let max_cap_sz = bs.read_bits(3)? as u8;
        let pam_sz = bs.read_bits(4)? as u8;
        let psist_0_9 = bs.read_bits(6)? as u8;
        let psist_10 = bs.read_bits(3)? as u8;
        let psist_11 = bs.read_bits(3)? as u8;
        let psist_12 = bs.read_bits(3)? as u8;
        let psist_13 = bs.read_bits(3)? as u8;
        let psist_14 = bs.read_bits(3)? as u8;
        let psist_15 = bs.read_bits(3)? as u8;
        let msg_psist = bs.read_bits(3)? as u8;
        let reg_psist = bs.read_bits(3)? as u8;
        let probe_pn_ran = bs.read_bits(4)? as u8;
        let acc_tmo = bs.read_bits(4)? as u8;
        let probe_bkoff = bs.read_bits(4)? as u8;
        let bkoff = bs.read_bits(4)? as u8;
        let max_req_seq = bs.read_bits(4)? as u8;
        let max_rsp_seq = bs.read_bits(4)? as u8;
        let auth = bs.read_bits(2)? as u8;
        let rand = if auth != 0 {
            bs.read_bits(32)? as u32
        } else {
            0
        };
        let nom_pwr_ext = bs.read_bits(1)? as u8;
        let psist_emg_incl = bs.read_bits(1)? != 0;
        let psist_emg = if psist_emg_incl {
            bs.read_bits(3)? as u8
        } else {
            0
        };
        let acct_incl = bs.read_bits(1)? != 0;
        let mut acct_incl_emg = false;
        let mut acct_aoc_bitmap_incl = false;
        let mut acct_so_records = Vec::new();
        let mut acct_so_grp_records = Vec::new();
        if acct_incl {
            acct_incl_emg = bs.read_bits(1)? != 0;
            acct_aoc_bitmap_incl = bs.read_bits(1)? != 0;
            let acct_so_incl = bs.read_bits(1)? != 0;
            if acct_so_incl {
                let num_acct_so = bs.read_bits(4)? as usize + 1;
                for _ in 0..num_acct_so {
                    let aoc_bitmap = if acct_aoc_bitmap_incl {
                        bs.read_bits(5)? as u8
                    } else {
                        0
                    };
                    let service_option = bs.read_bits(16)? as u16;
                    acct_so_records.push(AcctServiceOptionRecord {
                        aoc_bitmap,
                        service_option,
                    });
                }
            }
            let acct_so_grp_incl = bs.read_bits(1)? != 0;
            if acct_so_grp_incl {
                let num_acct_so_grp = bs.read_bits(3)? as usize + 1;
                for _ in 0..num_acct_so_grp {
                    let aoc_bitmap = if acct_aoc_bitmap_incl {
                        bs.read_bits(5)? as u8
                    } else {
                        0
                    };
                    let service_option_group = bs.read_bits(5)? as u8;
                    acct_so_grp_records.push(AcctServiceOptionGroupRecord {
                        aoc_bitmap,
                        service_option_group,
                    });
                }
            }
            if acct_so_records.is_empty() && acct_so_grp_records.is_empty() {
                return Err("ACCT_INCL requires at least one ACCT record".into());
            }
        }

        Ok(Self {
            pilot_pn,
            acc_msg_seq,
            acc_chan,
            nom_pwr,
            init_pwr,
            pwr_step,
            num_step,
            max_cap_sz,
            pam_sz,
            psist_0_9,
            psist_10,
            psist_11,
            psist_12,
            psist_13,
            psist_14,
            psist_15,
            msg_psist,
            reg_psist,
            probe_pn_ran,
            acc_tmo,
            probe_bkoff,
            bkoff,
            max_req_seq,
            max_rsp_seq,
            auth,
            rand,
            nom_pwr_ext,
            psist_emg_incl,
            psist_emg,
            acct_incl,
            acct_incl_emg,
            acct_aoc_bitmap_incl,
            acct_so_records,
            acct_so_grp_records,
        })
    }
}

impl NeighborListMessage {
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.pilot_inc, 4);
        for neighbor in &self.neighbors {
            bs.write_u32(*neighbor as u32, 9);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let pilot_inc = bs.read_bits(4)? as u8;
        let mut neighbors = Vec::new();
        while bs.len() >= 9 {
            neighbors.push(bs.read_bits(9)? as u16);
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            pilot_inc,
            neighbors,
        })
    }
}

impl CdmaChannelListMessage {
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        for channel in &self.channels {
            bs.write_u32(*channel as u32, 11);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let mut channels = Vec::new();
        while bs.len() >= 11 {
            channels.push(bs.read_bits(11)? as u16);
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            channels,
        })
    }
}

impl ExtendedSystemParametersMessage {
    fn meid_reqd_present(pref_msid_type: u8, ext_pref_msid_type: u8) -> bool {
        !(ext_pref_msid_type == 0b11 && matches!(pref_msid_type, 0b00 | 0b11))
    }

    fn valid_msid_selector(use_tmsi: bool, pref_msid_type: u8, ext_pref_msid_type: u8) -> bool {
        ext_pref_msid_type != 0b10
            && matches!(
                (use_tmsi, pref_msid_type, ext_pref_msid_type),
                (false, 0b00, 0b00)
                    | (false, 0b10, 0b00)
                    | (false, 0b11, 0b00)
                    | (true, 0b10, 0b00)
                    | (true, 0b11, 0b00)
                    | (false, 0b00, 0b01)
                    | (false, 0b10, 0b01)
                    | (false, 0b11, 0b01)
                    | (true, 0b10, 0b01)
                    | (true, 0b11, 0b01)
                    | (false, 0b00, 0b11)
                    | (false, 0b10, 0b11)
                    | (false, 0b11, 0b11)
                    | (true, 0b10, 0b11)
                    | (true, 0b11, 0b11)
            )
    }

    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        if let Some(ext_pref_msid_type) = self.ext_pref_msid_type {
            assert!(
                self.p_rev >= 11,
                "ESPM EXT_PREF_MSID_TYPE requires P_REV >= 11"
            );
            assert!(
                Self::valid_msid_selector(self.use_tmsi, self.pref_msid_type, ext_pref_msid_type),
                "ESPM reserved USE_TMSI/PREF_MSID_TYPE/EXT_PREF_MSID_TYPE combination"
            );
            assert_eq!(
                self.meid_reqd.is_some(),
                Self::meid_reqd_present(self.pref_msid_type, ext_pref_msid_type),
                "ESPM MEID_REQD presence must follow PREF_MSID_TYPE/EXT_PREF_MSID_TYPE"
            );
        }
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.delete_for_tmsi as u8, 1);
        bs.write_u8(self.use_tmsi as u8, 1);
        bs.write_u8(self.pref_msid_type, 2);
        bs.write_u32(self.mcc as u32, 10);
        bs.write_u8(self.imsi_11_12, 7);
        bs.write_u8(self.tmsi_zone.len() as u8, 4);
        for byte in &self.tmsi_zone {
            bs.write_u8(*byte, 8);
        }
        bs.write_u8(self.bcast_index, 3);
        bs.write_u8(self.imsi_t_supported as u8, 1);
        bs.write_u8(self.p_rev, 8);
        bs.write_u8(self.min_p_rev, 8);
        bs.write_u8(self.soft_slope, 6);
        bs.write_u8(self.add_intercept, 6);
        bs.write_u8(self.drop_intercept, 6);
        bs.write_u8(self.packet_zone_id, 8);
        bs.write_u8(self.max_num_alt_so, 3);
        bs.write_u8(self.reselect_included as u8, 1);
        if self.reselect_included {
            bs.write_u8(self.ec_thresh, 5);
            bs.write_u8(self.ec_io_thresh, 5);
        }
        bs.write_u8(self.pilot_report as u8, 1);
        bs.write_u8(self.nghbr_set_entry_info as u8, 1);
        if self.nghbr_set_entry_info {
            bs.write_u8(self.acc_ent_ho_order as u8, 1);
        }
        bs.write_u8(self.nghbr_set_access_info as u8, 1);
        if self.nghbr_set_access_info {
            bs.write_u8(self.access_ho as u8, 1);
            if self.access_ho {
                bs.write_u8(self.access_ho_msg_rsp as u8, 1);
            }
            bs.write_u8(self.access_probe_ho as u8, 1);
            if self.access_probe_ho {
                bs.write_u8(self.acc_ho_list_upd as u8, 1);
                bs.write_u8(self.acc_probe_ho_other_msg as u8, 1);
                bs.write_u8(self.max_num_probe_ho, 3);
            }
        }
        if self.nghbr_set_entry_info || self.nghbr_set_access_info {
            assert_eq!(
                self.access_entry_ho.len(),
                if self.nghbr_set_entry_info {
                    self.nghbr_set_size as usize
                } else {
                    0
                },
                "ACCESS_ENTRY_HO count must match NGHBR_SET_SIZE"
            );
            assert_eq!(
                self.access_ho_allowed.len(),
                if self.nghbr_set_access_info {
                    self.nghbr_set_size as usize
                } else {
                    0
                },
                "ACCESS_HO_ALLOWED count must match NGHBR_SET_SIZE"
            );
            bs.write_u8(self.nghbr_set_size, 6);
            for entry in &self.access_entry_ho {
                bs.write_u8(*entry as u8, 1);
            }
            for entry in &self.access_ho_allowed {
                bs.write_u8(*entry as u8, 1);
            }
        }
        bs.write_u8(self.broadcast_gps_asst as u8, 1);
        bs.write_u8(self.qpch_supported as u8, 1);
        if self.qpch_supported {
            bs.write_u8(self.num_qpch, 2);
            bs.write_u8(self.qpch_rate, 1);
            bs.write_u8(self.qpch_power_level_page, 3);
            bs.write_u8(self.qpch_cci_supported as u8, 1);
            if self.qpch_cci_supported {
                bs.write_u8(self.qpch_power_level_config, 3);
            }
        }
        bs.write_u8(self.sdb_supported as u8, 1);
        bs.write_u8(self.rlgain_traffic_pilot, 6);
        bs.write_u8(self.rev_pwr_cntl_delay_incl as u8, 1);
        if self.rev_pwr_cntl_delay_incl {
            bs.write_u8(self.rev_pwr_cntl_delay, 2);
        }
        bs.write_u8(self.auto_msg_supported as u8, 1);
        if self.auto_msg_supported {
            bs.write_u8(self.auto_msg_interval, 3);
        }
        bs.write_u8(self.mob_qos as u8, 1);
        bs.write_u8(self.enc_supported as u8, 1);
        if self.enc_supported {
            bs.write_u8(self.sig_encrypt_sup, 8);
            bs.write_u8(self.ui_encrypt_sup, 8);
        }
        bs.write_u8(self.use_sync_id as u8, 1);
        bs.write_u8(self.cs_supported as u8, 1);
        bs.write_u8(self.bcch_supported as u8, 1);
        bs.write_u8(self.ms_init_pos_loc_sup_ind as u8, 1);
        bs.write_u8(self.pilot_info_req_supported as u8, 1);
        if let Some(ext_pref_msid_type) = self.ext_pref_msid_type {
            if self.qpch_supported {
                bs.write_u8(0, 1); // QPCH_BI_SUPPORTED
            }
            bs.write_u8(0, 1); // BAND_CLASS_INFO_REQ
            bs.write_u8(0, 1); // CDMA_OFF_TIME_REP_SUP_IND
            bs.write_u8(0, 1); // CHM_SUPPORTED
            bs.write_u8(0, 1); // RELEASE_TO_IDLE_IND
            bs.write_u8(0, 1); // RECONNECT_MSG_IND
            bs.write_u8(0, 1); // MSG_INTEGRITY_SUP
            bs.write_u8(0, 1); // FOR_PDCH_SUPPORTED
            bs.write_u8(0, 1); // IMSI_10_INCL
            if self.cs_supported {
                bs.write_u8(0, 3); // MAX_ADD_SERV_INSTANCE
            }
            bs.write_u8(0, 1); // RER_MODE_SUPPORTED
            bs.write_u8(0, 1); // TKZ_MODE_SUPPORTED
            if self.packet_zone_id != 0 {
                bs.write_u8(0, 1); // PZ_HYST_ENABLED
            }
            bs.write_u8(ext_pref_msid_type, 2);
            if Self::meid_reqd_present(self.pref_msid_type, ext_pref_msid_type) {
                bs.write_u8(self.meid_reqd.expect("validated") as u8, 1);
            }
            bs.write_u8(0, 1); // AUTO_FCSO_ALLOWED
            bs.write_u8(0, 1); // SENDING_BSPM
            bs.write_u8(0, 1); // CAND_BAND_INFO_REQ
            bs.write_u8(0, 1); // TX_PWR_LIMIT_INCL
            bs.write_u8(0, 2); // BYPASS_REG_IND
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let delete_for_tmsi = bs.read_bits(1)? != 0;
        let use_tmsi = bs.read_bits(1)? != 0;
        let pref_msid_type = bs.read_bits(2)? as u8;
        let mcc = bs.read_bits(10)? as u16;
        let imsi_11_12 = bs.read_bits(7)? as u8;
        let tmsi_zone_len = bs.read_bits(4)? as usize;
        let mut tmsi_zone = Vec::with_capacity(tmsi_zone_len);
        for _ in 0..tmsi_zone_len {
            tmsi_zone.push(bs.read_bits(8)? as u8);
        }
        let bcast_index = bs.read_bits(3)? as u8;
        let imsi_t_supported = bs.read_bits(1)? != 0;
        let p_rev = bs.read_bits(8)? as u8;
        let min_p_rev = bs.read_bits(8)? as u8;
        let soft_slope = bs.read_bits(6)? as u8;
        let add_intercept = bs.read_bits(6)? as u8;
        let drop_intercept = bs.read_bits(6)? as u8;
        let packet_zone_id = bs.read_bits(8)? as u8;
        let max_num_alt_so = bs.read_bits(3)? as u8;
        let reselect_included = bs.read_bits(1)? != 0;
        let (ec_thresh, ec_io_thresh) = if reselect_included {
            (bs.read_bits(5)? as u8, bs.read_bits(5)? as u8)
        } else {
            (0, 0)
        };
        let pilot_report = bs.read_bits(1)? != 0;
        let nghbr_set_entry_info = bs.read_bits(1)? != 0;
        let acc_ent_ho_order = if nghbr_set_entry_info {
            bs.read_bits(1)? != 0
        } else {
            false
        };
        let nghbr_set_access_info = bs.read_bits(1)? != 0;
        let access_ho = if nghbr_set_access_info {
            bs.read_bits(1)? != 0
        } else {
            false
        };
        let access_ho_msg_rsp = if access_ho {
            bs.read_bits(1)? != 0
        } else {
            false
        };
        let access_probe_ho = if nghbr_set_access_info {
            bs.read_bits(1)? != 0
        } else {
            false
        };
        let (acc_ho_list_upd, acc_probe_ho_other_msg, max_num_probe_ho) = if access_probe_ho {
            (
                bs.read_bits(1)? != 0,
                bs.read_bits(1)? != 0,
                bs.read_bits(3)? as u8,
            )
        } else {
            (false, false, 0)
        };
        let mut nghbr_set_size = 0;
        let mut access_entry_ho = Vec::new();
        let mut access_ho_allowed = Vec::new();
        if nghbr_set_entry_info || nghbr_set_access_info {
            nghbr_set_size = bs.read_bits(6)? as u8;
            if nghbr_set_entry_info {
                for _ in 0..nghbr_set_size {
                    access_entry_ho.push(bs.read_bits(1)? != 0);
                }
            }
            if nghbr_set_access_info {
                for _ in 0..nghbr_set_size {
                    access_ho_allowed.push(bs.read_bits(1)? != 0);
                }
            }
        }
        let broadcast_gps_asst = bs.read_bits(1)? != 0;
        let qpch_supported = bs.read_bits(1)? != 0;
        let mut num_qpch = 0;
        let mut qpch_rate = 0;
        let mut qpch_power_level_page = 0;
        let mut qpch_cci_supported = false;
        let mut qpch_power_level_config = 0;
        if qpch_supported {
            num_qpch = bs.read_bits(2)? as u8;
            qpch_rate = bs.read_bits(1)? as u8;
            qpch_power_level_page = bs.read_bits(3)? as u8;
            qpch_cci_supported = bs.read_bits(1)? != 0;
            if qpch_cci_supported {
                qpch_power_level_config = bs.read_bits(3)? as u8;
            }
        }
        let sdb_supported = bs.read_bits(1)? != 0;
        let rlgain_traffic_pilot = bs.read_bits(6)? as u8;
        let rev_pwr_cntl_delay_incl = bs.read_bits(1)? != 0;
        let rev_pwr_cntl_delay = if rev_pwr_cntl_delay_incl {
            bs.read_bits(2)? as u8
        } else {
            0
        };
        let auto_msg_supported = bs.read_bits(1)? != 0;
        let auto_msg_interval = if auto_msg_supported {
            bs.read_bits(3)? as u8
        } else {
            0
        };
        let mob_qos = bs.read_bits(1)? != 0;
        let enc_supported = bs.read_bits(1)? != 0;
        let (sig_encrypt_sup, ui_encrypt_sup) = if enc_supported {
            (bs.read_bits(8)? as u8, bs.read_bits(8)? as u8)
        } else {
            (0, 0)
        };
        let use_sync_id = bs.read_bits(1)? != 0;
        let cs_supported = bs.read_bits(1)? != 0;
        let bcch_supported = bs.read_bits(1)? != 0;
        let ms_init_pos_loc_sup_ind = bs.read_bits(1)? != 0;
        let pilot_info_req_supported = bs.read_bits(1)? != 0;
        let (ext_pref_msid_type, meid_reqd) = if bs.is_empty() {
            (None, None)
        } else {
            if qpch_supported {
                let qpch_bi_supported = bs.read_bits(1)? != 0;
                if qpch_bi_supported {
                    let _qpch_power_level_bcast = bs.read_bits(3)?;
                }
            }
            let band_class_info_req = bs.read_bits(1)? != 0;
            if band_class_info_req {
                let _alt_band_class = bs.read_bits(5)?;
            }
            let cdma_off_time_rep_sup_ind = bs.read_bits(1)? != 0;
            if cdma_off_time_rep_sup_ind {
                let _cdma_off_time_rep_threshold_unit = bs.read_bits(1)?;
                let _cdma_off_time_rep_threshold = bs.read_bits(3)?;
            }
            let _chm_supported = bs.read_bits(1)? != 0;
            let _release_to_idle_ind = bs.read_bits(1)? != 0;
            let reconnect_msg_ind = bs.read_bits(1)? != 0;
            let msg_integrity_sup = bs.read_bits(1)? != 0;
            if msg_integrity_sup && bs.read_bits(1)? != 0 {
                let sig_integrity_sup = bs.read_bits(8)?;
                if sig_integrity_sup != 0 {
                    return Err("ESPM SIG_INTEGRITY_SUP reserved bits must be zero".into());
                }
            }
            let for_pdch_supported = bs.read_bits(1)? != 0;
            if for_pdch_supported {
                let pdch_chm_supported = bs.read_bits(1)? != 0;
                let pdch_parms_incl = bs.read_bits(1)? != 0;
                let for_pdch_rlgain_incl = bs.read_bits(1)? != 0;
                if for_pdch_rlgain_incl {
                    let _rlgain_ackch_pilot = bs.read_bits(6)?;
                    let _rlgain_cqich_pilot = bs.read_bits(6)?;
                }
                if pdch_chm_supported {
                    let _num_soft_switching_frames = bs.read_bits(4)?;
                    let _num_softer_switching_frames = bs.read_bits(4)?;
                    let _num_soft_switching_slots = bs.read_bits(2)?;
                    let _num_softer_switching_slots = bs.read_bits(2)?;
                    let _pdch_soft_switching_delay = bs.read_bits(8)?;
                    let _pdch_softer_switching_delay = bs.read_bits(8)?;
                }
                if pdch_parms_incl {
                    let _walsh_table_id = bs.read_bits(3)?;
                    let num_pdcch = bs.read_bits(3)? as usize;
                    for _ in 0..=num_pdcch {
                        let _for_pdcch_walsh = bs.read_bits(6)?;
                    }
                }
            }
            let imsi_10_incl = bs.read_bits(1)? != 0;
            if imsi_10_incl {
                let _imsi_10 = bs.read_bits(4)?;
            }
            if cs_supported {
                let _max_add_serv_instance = bs.read_bits(3)?;
            }
            let _rer_mode_supported = bs.read_bits(1)? != 0;
            let tkz_mode_supported = bs.read_bits(1)? != 0;
            if tkz_mode_supported {
                let _tkz_id = bs.read_bits(8)?;
            }
            if packet_zone_id != 0 {
                let pz_hyst_enabled = bs.read_bits(1)? != 0;
                if pz_hyst_enabled && bs.read_bits(1)? != 0 {
                    let pz_hyst_list_len = bs.read_bits(4)?;
                    let pz_hyst_act_timer = bs.read_bits(8)?;
                    let pz_hyst_timer_mul = bs.read_bits(3)?;
                    let pz_hyst_timer_exp = bs.read_bits(5)?;
                    if pz_hyst_list_len == 0
                        || pz_hyst_act_timer == 0
                        || pz_hyst_timer_mul == 0
                        || pz_hyst_timer_exp > 4
                    {
                        return Err("ESPM packet-zone hysteresis values out of spec".into());
                    }
                }
            }
            let ext_pref_msid_type = bs.read_bits(2)? as u8;
            if !Self::valid_msid_selector(use_tmsi, pref_msid_type, ext_pref_msid_type) {
                return Err(
                    "ESPM reserved USE_TMSI/PREF_MSID_TYPE/EXT_PREF_MSID_TYPE combination".into(),
                );
            }
            let meid_reqd = if Self::meid_reqd_present(pref_msid_type, ext_pref_msid_type) {
                Some(bs.read_bits(1)? != 0)
            } else {
                None
            };
            if !bs.is_empty() {
                let _auto_fcso_allowed = bs.read_bits(1)? != 0;
            }
            let rev_pdch_supported = if for_pdch_supported && !bs.is_empty() {
                bs.read_bits(1)? != 0
            } else {
                false
            };
            if rev_pdch_supported {
                let rev_pdch_parms_incl = bs.read_bits(1)? != 0;
                if rev_pdch_parms_incl {
                    let rev_pdch_rlgain_incl = bs.read_bits(1)? != 0;
                    if rev_pdch_rlgain_incl {
                        let _rlgain_spich_pilot = bs.read_bits(6)?;
                        let _rlgain_reqch_pilot = bs.read_bits(6)?;
                        let _rlgain_pdcch_pilot = bs.read_bits(6)?;
                    }
                    let rev_pdch_parms_1_incl = bs.read_bits(1)? != 0;
                    if rev_pdch_parms_1_incl {
                        let _rev_pdch_table_sel = bs.read_bits(1)?;
                        let _rev_pdch_max_auto_tpr = bs.read_bits(8)?;
                        let _rev_pdch_num_arq_rounds_normal = bs.read_bits(2)?;
                    }
                    let rev_pdch_oper_parms_incl = bs.read_bits(1)? != 0;
                    if rev_pdch_oper_parms_incl {
                        let _rev_pdch_max_size_allowed_encoder_packet = bs.read_bits(4)?;
                        let _rev_pdch_default_persistence = bs.read_bits(1)?;
                        let _rev_pdch_reset_persistence = bs.read_bits(1)?;
                        let _rev_pdch_grant_precedence = bs.read_bits(1)?;
                        let _rev_pdch_msib_supported = bs.read_bits(1)?;
                        let _rev_pdch_soft_switching_reset_ind = bs.read_bits(1)?;
                    }
                }
            }
            if reconnect_msg_ind && sdb_supported && !bs.is_empty() {
                let _sdb_in_rcnm_ind = bs.read_bits(1)? != 0;
            }
            if !bs.is_empty() {
                let sending_bspm = bs.read_bits(1)? != 0;
                if sending_bspm {
                    let _bspm_period_index = bs.read_bits(4)?;
                }
            }
            if !bs.is_empty() {
                let cand_band_info_req = bs.read_bits(1)? != 0;
                if cand_band_info_req {
                    let num_cand_band_class = bs.read_bits(3)? as usize;
                    for _ in 0..=num_cand_band_class {
                        let _cand_band_class = bs.read_bits(5)?;
                        let subclass_info_incl = bs.read_bits(1)? != 0;
                        if subclass_info_incl {
                            let subclass_rec_len = bs.read_bits(5)? as usize;
                            for _ in 0..=subclass_rec_len {
                                let _band_subclass_ind = bs.read_bits(1)?;
                            }
                        }
                    }
                }
            }
            if !bs.is_empty() {
                let tx_pwr_limit_incl = bs.read_bits(1)? != 0;
                if tx_pwr_limit_incl {
                    let _tx_pwr_limit = bs.read_bits(6)?;
                }
            }
            if !bs.is_empty() {
                let _bypass_reg_ind = bs.read_bits(2)?;
            }
            (Some(ext_pref_msid_type), meid_reqd)
        };

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            delete_for_tmsi,
            use_tmsi,
            pref_msid_type,
            mcc,
            imsi_11_12,
            tmsi_zone,
            bcast_index,
            imsi_t_supported,
            p_rev,
            min_p_rev,
            soft_slope,
            add_intercept,
            drop_intercept,
            packet_zone_id,
            max_num_alt_so,
            reselect_included,
            ec_thresh,
            ec_io_thresh,
            pilot_report,
            nghbr_set_entry_info,
            acc_ent_ho_order,
            nghbr_set_access_info,
            access_ho,
            access_ho_msg_rsp,
            access_probe_ho,
            acc_ho_list_upd,
            acc_probe_ho_other_msg,
            max_num_probe_ho,
            nghbr_set_size,
            access_entry_ho,
            access_ho_allowed,
            broadcast_gps_asst,
            qpch_supported,
            num_qpch,
            qpch_rate,
            qpch_power_level_page,
            qpch_cci_supported,
            qpch_power_level_config,
            sdb_supported,
            rlgain_traffic_pilot,
            rev_pwr_cntl_delay_incl,
            rev_pwr_cntl_delay,
            auto_msg_supported,
            auto_msg_interval,
            mob_qos,
            enc_supported,
            sig_encrypt_sup,
            ui_encrypt_sup,
            use_sync_id,
            cs_supported,
            bcch_supported,
            ms_init_pos_loc_sup_ind,
            pilot_info_req_supported,
            ext_pref_msid_type,
            meid_reqd,
        })
    }
}

impl GeneralPageMessage {
    pub fn to_sdu(&self) -> Bitstream {
        assert_eq!(self.reserved, 0, "GPM RESERVED bits must be zero");
        assert!(
            self.add_pfield.len() <= 7,
            "GPM ADD_PFIELD length must fit in 3 bits"
        );
        let mut bs = Bitstream::new();
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.acc_msg_seq, 6);
        bs.write_u8(self.class_0_done as u8, 1);
        bs.write_u8(self.class_1_done as u8, 1);
        bs.write_u8(self.tmsi_done as u8, 1);
        bs.write_u8(self.ordered_tmsis as u8, 1);
        bs.write_u8(self.broadcast_done as u8, 1);
        bs.write_u8(self.reserved, 4);
        bs.write_u8(self.add_pfield.len() as u8, 3);
        for byte in &self.add_pfield {
            bs.write_u8(*byte, 8);
        }
        for record in &self.page_records {
            write_general_page_record(&mut bs, record);
        }
        bs
    }

    /// Decode a GPM SDU (body after MSG_TYPE) into a GeneralPageMessage.
    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let config_msg_seq = bs.read_bits(6)? as u8;
        let acc_msg_seq = bs.read_bits(6)? as u8;
        let class_0_done = bs.read_bits(1)? != 0;
        let class_1_done = bs.read_bits(1)? != 0;
        let tmsi_done = bs.read_bits(1)? != 0;
        let ordered_tmsis = bs.read_bits(1)? != 0;
        let broadcast_done = bs.read_bits(1)? != 0;
        let reserved = bs.read_bits(4)? as u8;
        if reserved != 0 {
            return Err("GPM RESERVED bits must be zero".into());
        }
        let add_length = bs.read_bits(3)? as usize;
        let mut add_pfield = Vec::with_capacity(add_length);
        for _ in 0..add_length {
            add_pfield.push(bs.read_bits(8)? as u8);
        }
        let mut page_records = Vec::new();
        while bs.len() >= 2 {
            let page_class = bs.read_bits(2)? as u8;
            match page_class {
                0 => {
                    if bs.len() < 5 {
                        break;
                    }
                    let page_subclass = bs.read_bits(2)? as u8;
                    let msg_seq = bs.read_bits(3)? as u8;
                    let (imsi_s, imsi_11_12, mcc) = match page_subclass {
                        0 => {
                            if bs.len() < 34 {
                                break;
                            }
                            (Some(bs.read_bits(34)?), None, None)
                        }
                        1 => {
                            if bs.len() < 41 {
                                break;
                            }
                            let i12 = bs.read_bits(7)? as u8;
                            let s = bs.read_bits(34)?;
                            (Some(s), Some(i12), None)
                        }
                        2 => {
                            if bs.len() < 44 {
                                break;
                            }
                            let m = bs.read_bits(10)? as u16;
                            let s = bs.read_bits(34)?;
                            (Some(s), None, Some(m))
                        }
                        3 => {
                            if bs.len() < 51 {
                                break;
                            }
                            let m = bs.read_bits(10)? as u16;
                            let i12 = bs.read_bits(7)? as u8;
                            let s = bs.read_bits(34)?;
                            (Some(s), Some(i12), Some(m))
                        }
                        _ => break,
                    };
                    let special_service = if bs.len() >= 1 {
                        bs.read_bits(1)? != 0
                    } else {
                        false
                    };
                    let service_option = if special_service && bs.len() >= 16 {
                        Some(bs.read_bits(16)? as u16)
                    } else {
                        None
                    };
                    let (imsi_m_s1, imsi_m_s2) = imsi_s.map_or((None, None), |s| {
                        let s1 = (s & 0xFF_FFFF) as u32;
                        let s2 = ((s >> 24) & 0x3FF) as u16;
                        (Some(s1), Some(s2))
                    });
                    page_records.push(GeneralPageRecord::Class0 {
                        page_subclass,
                        msg_seq,
                        imsi_s,
                        imsi_11_12,
                        mcc,
                        imsi_addr_num: None,
                        imsi_m_s1,
                        imsi_m_s2,
                        special_service,
                        service_option,
                    });
                }
                1 => {
                    if bs.len() < 36 {
                        break;
                    }
                    let msg_seq = bs.read_bits(3)? as u8;
                    let esn = bs.read_bits(32)? as u32;
                    let special_service = if bs.len() >= 1 {
                        bs.read_bits(1)? != 0
                    } else {
                        false
                    };
                    let service_option = if special_service && bs.len() >= 16 {
                        Some(bs.read_bits(16)? as u16)
                    } else {
                        None
                    };
                    page_records.push(GeneralPageRecord::Class1 {
                        msg_seq,
                        esn,
                        special_service,
                        service_option,
                    });
                }
                2 => {
                    if bs.len() < 36 {
                        break;
                    }
                    let msg_seq = bs.read_bits(3)? as u8;
                    let tmsi_code_addr = bs.read_bits(32)? as u32;
                    let special_service = if bs.len() >= 1 {
                        bs.read_bits(1)? != 0
                    } else {
                        false
                    };
                    let service_option = if special_service && bs.len() >= 16 {
                        Some(bs.read_bits(16)? as u16)
                    } else {
                        None
                    };
                    page_records.push(GeneralPageRecord::Tmsi {
                        msg_seq,
                        tmsi_code_addr,
                        special_service,
                        service_option,
                    });
                }
                3 => {
                    if bs.len() < 16 {
                        break;
                    }
                    let bc_addr = bs.read_bits(16)? as u16;
                    page_records.push(GeneralPageRecord::Broadcast { bc_addr });
                }
                _ => break,
            }
        }
        Ok(Self {
            config_msg_seq,
            acc_msg_seq,
            class_0_done,
            class_1_done,
            tmsi_done,
            ordered_tmsis,
            broadcast_done,
            reserved,
            add_pfield,
            page_records,
        })
    }
}

impl AuthenticationChallengeMessage {
    /// Encode AUCM per C.S0005-E 3.7.2.3.2.10: RANDU(24) + GEN_CMEAKEY(1).
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(self.randu & 0x00ff_ffff, 24);
        bs.write_u8(self.gen_cmea_key as u8, 1);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        Ok(Self {
            randu: bs.read_bits(24)? as u32,
            gen_cmea_key: bs.read_bits(1)? != 0,
        })
    }
}

impl SsdUpdateMessage {
    /// Encode SSDUM per C.S0005-E 3.7.2.3.2.11: RANDSSD(56).
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u64(self.randssd & 0x00ff_ffff_ffff_ffff, 56);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        Ok(Self {
            randssd: bs.read_bits(56)?,
        })
    }
}

impl FeatureNotificationMessage {
    /// Encode FNM per C.S0005-E 3.7.2.3.2.12:
    /// RELEASE(1) + one or more information records.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            !self.records.is_empty(),
            "Feature Notification Message requires at least one information record"
        );

        let mut bs = Bitstream::new();
        bs.write_u8(self.release as u8, 1);
        for record in &self.records {
            record
                .validate_for_feature_notification()
                .expect("FNM information record invalid");
            assert!(
                record.data.len() <= u8::MAX as usize,
                "Feature Notification information record length must fit in one octet"
            );
            bs.write_u8(record.record_type, 8);
            bs.write_u8(record.data.len() as u8, 8);
            for byte in &record.data {
                bs.write_u8(*byte, 8);
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let release = bs.read_bits(1)? != 0;
        let mut records = Vec::new();
        while !bs.is_empty() {
            if bs.len() < 16 {
                return Err("FNM information record header truncated".into());
            }
            let record_type = bs.read_bits(8)? as u8;
            let record_len = bs.read_bits(8)? as usize;
            if bs.len() < record_len * 8 {
                return Err("FNM information record length exceeds remaining SDU".into());
            }
            let mut data = Vec::with_capacity(record_len);
            for _ in 0..record_len {
                data.push(bs.read_bits(8)? as u8);
            }
            let record = InformationRecord { record_type, data };
            record.validate_for_feature_notification()?;
            records.push(record);
        }
        if records.is_empty() {
            return Err("FNM requires at least one information record".into());
        }
        Ok(Self { release, records })
    }
}

impl InformationRecord {
    fn validate_for_feature_notification(&self) -> Result<(), crate::error::Error> {
        match InfoRecordType::from_wire(self.record_type) {
            Some(InfoRecordType::Display) => {
                decode_display_information_record(&self.data)?;
            }
            Some(InfoRecordType::Signal) => {
                decode_signal_information_record(&self.data)?;
            }
            Some(InfoRecordType::MessageWaiting) => {
                decode_message_waiting_record(&self.data)?;
            }
            Some(InfoRecordType::ParametricAlerting) => {
                decode_parametric_alerting_record(&self.data)?;
            }
            Some(
                record_type @ (InfoRecordType::CalledPartyNumber
                | InfoRecordType::CallingPartyNumber
                | InfoRecordType::RedirectingNumber),
            ) => {
                decode_party_number_record(record_type, &self.data)?;
            }
            Some(
                record_type @ (InfoRecordType::CalledPartySubaddress
                | InfoRecordType::CallingPartySubaddress
                | InfoRecordType::RedirectingSubaddress),
            ) => {
                decode_party_subaddress_record(record_type, &self.data)?;
            }
            Some(InfoRecordType::ExtendedDisplay) => {
                decode_extended_display_record(&self.data)?;
            }
            Some(InfoRecordType::MultiCharExtendedDisplay) => {
                decode_multi_char_extended_display_record(&self.data)?;
            }
            Some(InfoRecordType::EnhMultiCharExtendedDisplay) => {
                decode_enhanced_multi_char_extended_display_record(&self.data)?;
            }
            Some(InfoRecordType::ExtendedRecordTypeIntl) => {
                decode_international_extended_record(&self.data)?;
            }
            Some(
                InfoRecordType::ConnectedNumber
                | InfoRecordType::ServiceConfiguration
                | InfoRecordType::ConnectedSubaddress
                | InfoRecordType::MeterPulses
                | InfoRecordType::LineControl
                | InfoRecordType::NonNegServiceConfiguration
                | InfoRecordType::CallWaitingIndicator,
            ) => {
                return Err(format!(
                    "information record type 0x{:02x} is not valid for FNM",
                    self.record_type
                )
                .into());
            }
            None => {
                return Err(format!(
                    "FNM information record type 0x{:02x} is reserved",
                    self.record_type
                )
                .into());
            }
        }
        Ok(())
    }
}

impl ExtendedNeighborListMessage {
    /// Encode ENLM per C.S0005-E 3.7.2.3.2.14.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            (1..=15).contains(&self.pilot_inc),
            "ENLM PILOT_INC must be in the range 1..=15"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.pilot_inc, 4);
        for neighbor in &self.neighbors {
            assert!(
                neighbor.nghbr_config <= 0b011,
                "ENLM NGHBR_CONFIG 0b100..0b111 is reserved"
            );
            bs.write_u8(neighbor.nghbr_config, 3);
            bs.write_u32(neighbor.nghbr_pn as u32, 9);
            bs.write_u8(neighbor.search_priority, 2);
            match (neighbor.nghbr_band, neighbor.nghbr_freq) {
                (Some(band), Some(freq)) => {
                    bs.write_u8(1, 1);
                    bs.write_u8(band, 5);
                    bs.write_u32(freq as u32, 11);
                }
                (None, None) => bs.write_u8(0, 1),
                _ => panic!("ENLM frequency inclusion requires both NGHBR_BAND and NGHBR_FREQ"),
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let pilot_inc = bs.read_bits(4)? as u8;
        if pilot_inc == 0 {
            return Err("ENLM PILOT_INC must be in the range 1..=15".into());
        }

        let mut neighbors = Vec::new();
        while !bs.is_empty() {
            if bs.len() < 15 {
                return Err("ENLM neighbor record truncated".into());
            }
            let nghbr_config = bs.read_bits(3)? as u8;
            if nghbr_config > 0b011 {
                return Err("ENLM NGHBR_CONFIG 0b100..0b111 is reserved".into());
            }
            let nghbr_pn = bs.read_bits(9)? as u16;
            let search_priority = bs.read_bits(2)? as u8;
            let freq_incl = bs.read_bits(1)? != 0;
            let (nghbr_band, nghbr_freq) = if freq_incl {
                if bs.len() < 16 {
                    return Err("ENLM neighbor frequency fields truncated".into());
                }
                (Some(bs.read_bits(5)? as u8), Some(bs.read_bits(11)? as u16))
            } else {
                (None, None)
            };
            neighbors.push(ExtendedNeighborRecord {
                nghbr_config,
                nghbr_pn,
                search_priority,
                nghbr_band,
                nghbr_freq,
            });
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            pilot_inc,
            neighbors,
        })
    }
}

impl StatusRequestMessage {
    /// Encode STRQM per C.S0005-E 3.7.2.3.2.15.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.record_types.len() <= 15,
            "STRQM NUM_FIELDS must fit in 4 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u8(0, 4); // RESERVED
        match self.qual_info {
            StatusQualificationInfo::None => {
                bs.write_u8(0x00, 8);
                bs.write_u8(0, 3);
            }
            StatusQualificationInfo::BandClass { band_class } => {
                bs.write_u8(0x01, 8);
                bs.write_u8(1, 3);
                bs.write_u8(band_class, 5);
                bs.write_u8(0, 3); // RESERVED
            }
            StatusQualificationInfo::BandClassAndOperatingMode {
                band_class,
                op_mode,
            } => {
                bs.write_u8(0x02, 8);
                bs.write_u8(2, 3);
                bs.write_u8(band_class, 5);
                bs.write_u8(op_mode, 8);
                bs.write_u8(0, 3); // RESERVED
            }
        }
        bs.write_u8(self.record_types.len() as u8, 4);
        for record_type in &self.record_types {
            bs.write_u8(*record_type, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let reserved = bs.read_bits(4)? as u8;
        if reserved != 0 {
            return Err("STRQM RESERVED field must be zero".into());
        }
        let qual_info_type = bs.read_bits(8)? as u8;
        let qual_info_len = bs.read_bits(3)? as usize;
        let qual_info = match qual_info_type {
            0x00 => {
                if qual_info_len != 0 {
                    return Err("STRQM QUAL_INFO_TYPE=0 requires QUAL_INFO_LEN=0".into());
                }
                StatusQualificationInfo::None
            }
            0x01 => {
                if qual_info_len != 1 {
                    return Err("STRQM BAND_CLASS qualification requires QUAL_INFO_LEN=1".into());
                }
                let band_class = bs.read_bits(5)? as u8;
                let reserved = bs.read_bits(3)? as u8;
                if reserved != 0 {
                    return Err("STRQM BAND_CLASS reserved bits must be zero".into());
                }
                StatusQualificationInfo::BandClass { band_class }
            }
            0x02 => {
                if qual_info_len != 2 {
                    return Err(
                        "STRQM BAND_CLASS+OP_MODE qualification requires QUAL_INFO_LEN=2".into(),
                    );
                }
                let band_class = bs.read_bits(5)? as u8;
                let op_mode = bs.read_bits(8)? as u8;
                let reserved = bs.read_bits(3)? as u8;
                if reserved != 0 {
                    return Err("STRQM BAND_CLASS+OP_MODE reserved bits must be zero".into());
                }
                StatusQualificationInfo::BandClassAndOperatingMode {
                    band_class,
                    op_mode,
                }
            }
            _ => {
                return Err(format!("STRQM reserved QUAL_INFO_TYPE 0x{qual_info_type:02x}").into());
            }
        };

        let num_fields = bs.read_bits(4)? as usize;
        if bs.len() < num_fields * 8 {
            return Err("STRQM RECORD_TYPE list truncated".into());
        }
        let mut record_types = Vec::with_capacity(num_fields);
        for _ in 0..num_fields {
            record_types.push(bs.read_bits(8)? as u8);
        }
        if !bs.is_empty() {
            return Err("STRQM has trailing bits after RECORD_TYPE list".into());
        }

        Ok(Self {
            qual_info,
            record_types,
        })
    }
}

const fn valid_redirection_record_type(record_type: u8) -> bool {
    matches!(record_type, 0x00 | 0x02 | 0x05)
}

impl ServiceRedirectionMessage {
    /// Encode SRDM per C.S0005-E 3.7.2.3.2.16.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.record.len() <= u8::MAX as usize,
            "SRDM RECORD_LEN must fit in one octet"
        );
        assert!(
            self.record_type != 0 || self.record.is_empty(),
            "SRDM NDSS off indication requires RECORD_LEN=0"
        );
        assert!(
            valid_redirection_record_type(self.record_type),
            "SRDM RECORD_TYPE must be 0, 2, or 5"
        );

        let mut bs = Bitstream::new();
        bs.write_u8(self.return_if_fail as u8, 1);
        bs.write_u8(self.delete_tmsi as u8, 1);
        bs.write_u8(self.redirect_type as u8, 1);
        bs.write_u8(self.record_type, 8);
        bs.write_u8(self.record.len() as u8, 8);
        for byte in &self.record {
            bs.write_u8(*byte, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let return_if_fail = bs.read_bits(1)? != 0;
        let delete_tmsi = bs.read_bits(1)? != 0;
        let redirect_type = bs.read_bits(1)? != 0;
        let record_type = bs.read_bits(8)? as u8;
        let record_len = bs.read_bits(8)? as usize;
        if !valid_redirection_record_type(record_type) {
            return Err(format!("SRDM reserved RECORD_TYPE 0x{record_type:02x}").into());
        }
        if record_type == 0 && record_len != 0 {
            return Err("SRDM NDSS off indication requires RECORD_LEN=0".into());
        }
        if bs.len() < record_len * 8 {
            return Err("SRDM redirection record length exceeds remaining SDU".into());
        }
        let mut record = Vec::with_capacity(record_len);
        for _ in 0..record_len {
            record.push(bs.read_bits(8)? as u8);
        }
        if !bs.is_empty() {
            return Err("SRDM has trailing bits after redirection record".into());
        }

        Ok(Self {
            return_if_fail,
            delete_tmsi,
            redirect_type,
            record_type,
            record,
        })
    }

    pub fn redirection_record(&self) -> Result<ExtendedRedirectionRecord, crate::error::Error> {
        decode_redirection_record_from_parts(self.record_type, &self.record, "SRDM")
    }
}

impl GlobalServiceRedirectionMessage {
    /// Encode GSRDM per C.S0005-E 3.7.2.3.2.18.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.record.len() <= u8::MAX as usize,
            "GSRDM RECORD_LEN must fit in one octet"
        );
        assert!(
            self.record_type != 0 || self.record.is_empty(),
            "GSRDM NDSS off indication requires RECORD_LEN=0"
        );
        assert!(
            valid_redirection_record_type(self.record_type),
            "GSRDM RECORD_TYPE must be 0, 2, or 5"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u32(self.redirect_accolc as u32, 16);
        bs.write_u8(self.return_if_fail as u8, 1);
        bs.write_u8(self.delete_tmsi as u8, 1);
        bs.write_u8(self.excl_p_rev_ms as u8, 1);
        bs.write_u8(self.record_type, 8);
        bs.write_u8(self.record.len() as u8, 8);
        for byte in &self.record {
            bs.write_u8(*byte, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let redirect_accolc = bs.read_bits(16)? as u16;
        let return_if_fail = bs.read_bits(1)? != 0;
        let delete_tmsi = bs.read_bits(1)? != 0;
        let excl_p_rev_ms = bs.read_bits(1)? != 0;
        let record_type = bs.read_bits(8)? as u8;
        let record_len = bs.read_bits(8)? as usize;
        if !valid_redirection_record_type(record_type) {
            return Err(format!("GSRDM reserved RECORD_TYPE 0x{record_type:02x}").into());
        }
        if record_type == 0 && record_len != 0 {
            return Err("GSRDM NDSS off indication requires RECORD_LEN=0".into());
        }
        if bs.len() < record_len * 8 {
            return Err("GSRDM redirection record length exceeds remaining SDU".into());
        }
        let mut record = Vec::with_capacity(record_len);
        for _ in 0..record_len {
            record.push(bs.read_bits(8)? as u8);
        }
        if !bs.is_empty() {
            return Err("GSRDM has trailing bits after redirection record".into());
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            redirect_accolc,
            return_if_fail,
            delete_tmsi,
            excl_p_rev_ms,
            record_type,
            record,
        })
    }

    pub fn redirection_record(&self) -> Result<ExtendedRedirectionRecord, crate::error::Error> {
        decode_redirection_record_from_parts(self.record_type, &self.record, "GSRDM")
    }
}

fn decode_redirection_record_from_parts(
    record_type: u8,
    record: &[u8],
    context: &str,
) -> Result<ExtendedRedirectionRecord, crate::error::Error> {
    if !valid_redirection_record_type(record_type) {
        return Err(format!("{context} reserved RECORD_TYPE 0x{record_type:02x}").into());
    }
    if record_type == 0 {
        if !record.is_empty() {
            return Err(format!("{context} NDSS off indication requires RECORD_LEN=0").into());
        }
        return Ok(ExtendedRedirectionRecord::NdssOff);
    }

    match record_type {
        0x02 => {
            let mut bs = Bitstream::new_bytes(record);
            read_service_redirection_cdma_record(&mut bs, context)
        }
        0x05 => Ok(ExtendedRedirectionRecord::Ds41(record.to_vec())),
        _ => unreachable!("validated redirection record type"),
    }
}

fn read_service_redirection_cdma_record(
    bs: &mut Bitstream,
    context: &str,
) -> Result<ExtendedRedirectionRecord, crate::error::Error> {
    let band_class = bs.read_bits(5)? as u8;
    let expected_sid = bs.read_bits(15)? as u16;
    let expected_nid = bs.read_bits(16)? as u16;
    let reserved = bs.read_bits(4)? as u8;
    if reserved != 0 {
        return Err(format!("{context} CDMA redirection reserved field must be zero").into());
    }
    let num_chans = bs.read_bits(4)? as usize;
    let mut cdma_chans = Vec::with_capacity(num_chans);
    for _ in 0..num_chans {
        cdma_chans.push(bs.read_bits(11)? as u16);
    }
    GeneralNeighborListMessage::read_zero_tail(bs, "SRDM/GSRDM CDMA redirection record")?;
    Ok(ExtendedRedirectionRecord::Cdma {
        band_class,
        expected_sid,
        expected_nid,
        cdma_chans,
        redirect_subclasses: None,
    })
}

impl TmsiAssignmentMessage {
    /// Encode TASM per C.S0005-E 3.7.2.3.2.19.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            (1..=8).contains(&self.tmsi_zone.len()),
            "TASM TMSI_ZONE_LEN must be in the range 1..=8"
        );

        let mut bs = Bitstream::new();
        bs.write_u8(0, 5); // RESERVED
        bs.write_u8(self.tmsi_zone.len() as u8, 4);
        for byte in &self.tmsi_zone {
            bs.write_u8(*byte, 8);
        }
        bs.write_u32(self.tmsi_code, 32);
        bs.write_u32(self.tmsi_exp_time & 0x00ff_ffff, 24);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let reserved = bs.read_bits(5)? as u8;
        if reserved != 0 {
            return Err("TASM RESERVED field must be zero".into());
        }
        let tmsi_zone_len = bs.read_bits(4)? as usize;
        if !(1..=8).contains(&tmsi_zone_len) {
            return Err("TASM TMSI_ZONE_LEN must be in the range 1..=8".into());
        }
        if bs.len() < (tmsi_zone_len * 8) + 56 {
            return Err("TASM body truncated".into());
        }
        let mut tmsi_zone = Vec::with_capacity(tmsi_zone_len);
        for _ in 0..tmsi_zone_len {
            tmsi_zone.push(bs.read_bits(8)? as u8);
        }
        let tmsi_code = bs.read_bits(32)? as u32;
        let tmsi_exp_time = bs.read_bits(24)? as u32;
        if !bs.is_empty() {
            return Err("TASM has trailing bits after TMSI_EXP_TIME".into());
        }

        Ok(Self {
            tmsi_zone,
            tmsi_code,
            tmsi_exp_time,
        })
    }
}

impl PacaMessage {
    /// Encode PACAM per C.S0005-E 3.7.2.3.2.20.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.purpose <= 0b0011,
            "PACAM PURPOSE 0b0100..0b1111 is reserved"
        );
        assert!(
            self.purpose < 0b0010 || self.q_pos == 0,
            "PACAM Q_POS must be zero for PURPOSE 0b0010 or 0b0011"
        );
        let mut bs = Bitstream::new();
        bs.write_u8(0, 7); // RESERVED
        bs.write_u8(self.purpose, 4);
        bs.write_u8(self.q_pos, 8);
        bs.write_u8(self.paca_timeout, 3);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let reserved = bs.read_bits(7)? as u8;
        if reserved != 0 {
            return Err("PACAM RESERVED field must be zero".into());
        }
        let purpose = bs.read_bits(4)? as u8;
        if purpose > 0b0011 {
            return Err("PACAM PURPOSE 0b0100..0b1111 is reserved".into());
        }
        let q_pos = bs.read_bits(8)? as u8;
        if purpose >= 0b0010 && q_pos != 0 {
            return Err("PACAM Q_POS must be zero for PURPOSE 0b0010 or 0b0011".into());
        }
        let paca_timeout = bs.read_bits(3)? as u8;
        if !bs.is_empty() {
            return Err("PACAM has trailing bits after PACA_TIMEOUT".into());
        }
        Ok(Self {
            purpose,
            q_pos,
            paca_timeout,
        })
    }
}

impl GeneralNeighborListMessage {
    /// Encode GNLM per C.S0005-E 3.7.2.3.2.22.
    pub fn to_sdu(&self) -> Bitstream {
        self.validate();

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.pilot_inc, 4);
        bs.write_u8(self.nghbr_srch_mode, 2);
        bs.write_u8(self.nghbr_config_pn_incl as u8, 1);
        bs.write_u8(self.freq_fields_incl as u8, 1);
        bs.write_u8(self.use_timing as u8, 1);
        if self.use_timing {
            bs.write_u8(self.global_timing.is_some() as u8, 1);
            if let Some(global_timing) = &self.global_timing {
                bs.write_u8(global_timing.tx_duration, 4);
                bs.write_u8(global_timing.tx_period, 7);
            }
        }
        bs.write_u8(self.neighbors.len() as u8, 6);
        for neighbor in &self.neighbors {
            if self.nghbr_config_pn_incl {
                bs.write_u8(neighbor.nghbr_config.expect("validated") & 0x07, 3);
                bs.write_u32(neighbor.nghbr_pn.expect("validated") as u32, 9);
            }
            if Self::has_search_priority(self.nghbr_srch_mode) {
                bs.write_u8(neighbor.search_priority.expect("validated"), 2);
            }
            if Self::has_search_window(self.nghbr_srch_mode) {
                bs.write_u8(neighbor.srch_win_nghbr.expect("validated"), 4);
            }
            if self.freq_fields_incl {
                match (neighbor.nghbr_band, neighbor.nghbr_freq) {
                    (Some(band), Some(freq)) => {
                        bs.write_u8(1, 1);
                        bs.write_u8(band, 5);
                        bs.write_u32(freq as u32, 11);
                    }
                    (None, None) => bs.write_u8(0, 1),
                    _ => unreachable!("validated"),
                }
            }
            if self.use_timing {
                if let Some(timing) = &neighbor.timing {
                    bs.write_u8(1, 1);
                    bs.write_u8(timing.tx_offset, 7);
                    if self.global_timing.is_none() {
                        bs.write_u8(timing.tx_duration.expect("validated"), 4);
                        bs.write_u8(timing.tx_period.expect("validated"), 7);
                    }
                } else {
                    bs.write_u8(0, 1);
                }
            }
        }

        bs.write_u8(self.analog_neighbors.len() as u8, 3);
        for analog in &self.analog_neighbors {
            bs.write_u8(analog.band_class, 5);
            bs.write_u8(analog.sys_a_b, 2);
        }

        bs.write_u8(self.srch_offset_incl as u8, 1);
        for pilot in &self.pilot_info {
            if let Some(record) = &pilot.pilot_record {
                bs.write_u8(1, 1);
                bs.write_u8(record.record_type(), 3);
                let mut record_bs = Bitstream::new();
                Self::write_pilot_record(record, &mut record_bs);
                pad_to_octet(&mut record_bs);
                let record_len = record_bs.len() / 8;
                assert!(record_len <= 7, "GNLM pilot RECORD_LEN must fit in 3 bits");
                bs.write_u8(record_len as u8, 3);
                bs.extend(&record_bs);
            } else {
                bs.write_u8(0, 1);
            }
            if self.srch_offset_incl {
                bs.write_u8(pilot.srch_offset_nghbr.expect("validated"), 3);
            }
        }

        if let Some(bcch_support) = &self.bcch_support {
            bs.write_u8(1, 1);
            Self::write_bool_slice(&mut bs, bcch_support);
        } else {
            bs.write_u8(0, 1);
        }

        if let Some(resq) = &self.resq {
            bs.write_u8(1, 1);
            bs.write_u8(resq.delay_time, 6);
            bs.write_u8(resq.allowed_time, 6);
            bs.write_u8(resq.attempt_time, 6);
            bs.write_u32(resq.code_chan as u32, 11);
            bs.write_u8(resq.qof, 2);
            if let Some(min_period) = resq.min_period {
                bs.write_u8(1, 1);
                bs.write_u8(min_period, 5);
            } else {
                bs.write_u8(0, 1);
            }
            match (resq.num_tot_trans_20ms, resq.num_tot_trans_5ms) {
                (Some(trans_20ms), Some(trans_5ms)) => {
                    bs.write_u8(1, 1);
                    bs.write_u8(trans_20ms, 4);
                    bs.write_u8(trans_5ms, 4);
                }
                (None, None) => bs.write_u8(0, 1),
                _ => unreachable!("validated"),
            }
            bs.write_u8(resq.num_preamble_rc1_rc2, 3);
            bs.write_u8(resq.num_preamble, 3);
            bs.write_u8(resq.power_delta, 3);
            Self::write_bool_slice(&mut bs, &resq.nghbr_resq_configured);
        } else {
            bs.write_u8(0, 1);
        }

        Self::write_bool_slice(&mut bs, &self.pdch_supported);

        if let Some(hrpd_neighbors) = &self.hrpd_neighbors {
            bs.write_u8(1, 1);
            bs.write_u8(hrpd_neighbors.len() as u8, 6);
            for hrpd in hrpd_neighbors {
                let mut record_bs = Bitstream::new();
                Self::write_hrpd_neighbor_body(hrpd, &mut record_bs);
                pad_to_octet(&mut record_bs);
                let record_len = record_bs.len() / 8;
                assert!(
                    record_len <= u8::MAX as usize,
                    "GNLM HRPD_NGHBR_REC_LEN must fit in one octet"
                );
                bs.write_u8(record_len as u8, 8);
                bs.extend(&record_bs);
            }
        } else {
            bs.write_u8(0, 1);
        }

        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let pilot_inc = bs.read_bits(4)? as u8;
        if pilot_inc == 0 {
            return Err("GNLM PILOT_INC must be in the range 1..=15".into());
        }
        let nghbr_srch_mode = bs.read_bits(2)? as u8;
        let nghbr_config_pn_incl = bs.read_bits(1)? != 0;
        let freq_fields_incl = bs.read_bits(1)? != 0;
        let use_timing = bs.read_bits(1)? != 0;
        let global_timing = if use_timing && bs.read_bits(1)? != 0 {
            Some(GeneralNeighborGlobalTiming {
                tx_duration: bs.read_bits(4)? as u8,
                tx_period: bs.read_bits(7)? as u8,
            })
        } else {
            None
        };
        let num_nghbr = bs.read_bits(6)? as usize;
        let mut neighbors = Vec::with_capacity(num_nghbr);
        for _ in 0..num_nghbr {
            let (nghbr_config, nghbr_pn) = if nghbr_config_pn_incl {
                let nghbr_config = bs.read_bits(3)? as u8;
                if nghbr_config > 0b011 {
                    return Err("GNLM NGHBR_CONFIG 0b100..0b111 is reserved".into());
                }
                (Some(nghbr_config), Some(bs.read_bits(9)? as u16))
            } else {
                (None, None)
            };
            let search_priority = if Self::has_search_priority(nghbr_srch_mode) {
                Some(bs.read_bits(2)? as u8)
            } else {
                None
            };
            let srch_win_nghbr = if Self::has_search_window(nghbr_srch_mode) {
                Some(bs.read_bits(4)? as u8)
            } else {
                None
            };
            let (nghbr_band, nghbr_freq) = if freq_fields_incl && bs.read_bits(1)? != 0 {
                (Some(bs.read_bits(5)? as u8), Some(bs.read_bits(11)? as u16))
            } else {
                (None, None)
            };
            let timing = if use_timing && bs.read_bits(1)? != 0 {
                let tx_offset = bs.read_bits(7)? as u8;
                let (tx_duration, tx_period) = if global_timing.is_none() {
                    (Some(bs.read_bits(4)? as u8), Some(bs.read_bits(7)? as u8))
                } else {
                    (None, None)
                };
                Some(GeneralNeighborTiming {
                    tx_offset,
                    tx_duration,
                    tx_period,
                })
            } else {
                None
            };
            neighbors.push(GeneralNeighborRecord {
                nghbr_config,
                nghbr_pn,
                search_priority,
                srch_win_nghbr,
                nghbr_band,
                nghbr_freq,
                timing,
            });
        }

        let num_analog_nghbr = bs.read_bits(3)? as usize;
        let mut analog_neighbors = Vec::with_capacity(num_analog_nghbr);
        for _ in 0..num_analog_nghbr {
            analog_neighbors.push(GeneralAnalogNeighborRecord {
                band_class: bs.read_bits(5)? as u8,
                sys_a_b: bs.read_bits(2)? as u8,
            });
        }

        let srch_offset_incl = bs.read_bits(1)? != 0;
        if srch_offset_incl && !Self::has_search_window(nghbr_srch_mode) {
            return Err("GNLM SRCH_OFFSET_INCL requires search-window mode".into());
        }
        let mut pilot_info = Vec::with_capacity(num_nghbr);
        for _ in 0..num_nghbr {
            let pilot_record = if bs.read_bits(1)? != 0 {
                let record_type = bs.read_bits(3)? as u8;
                let record_len = bs.read_bits(3)? as usize;
                if record_len == 0 {
                    return Err(
                        "GNLM pilot RECORD_LEN must be non-zero when ADD_PILOT_REC_INCL is set"
                            .into(),
                    );
                }
                let record_bits = record_len * 8;
                if bs.len() < record_bits {
                    return Err("GNLM pilot record length exceeds remaining SDU".into());
                }
                let mut record_bs = bs.drain(0..record_bits);
                Some(Self::read_pilot_record(record_type, &mut record_bs)?)
            } else {
                None
            };
            let srch_offset_nghbr = if srch_offset_incl {
                Some(bs.read_bits(3)? as u8)
            } else {
                None
            };
            pilot_info.push(GeneralNeighborPilotInfo {
                pilot_record,
                srch_offset_nghbr,
            });
        }

        let bcch_support = if bs.read_bits(1)? != 0 {
            Some(Self::read_bool_vec(bs, num_nghbr)?)
        } else {
            None
        };

        let resq = if bs.read_bits(1)? != 0 {
            let delay_time = bs.read_bits(6)? as u8;
            let allowed_time = bs.read_bits(6)? as u8;
            let attempt_time = bs.read_bits(6)? as u8;
            let code_chan = bs.read_bits(11)? as u16;
            let qof = bs.read_bits(2)? as u8;
            let min_period = if bs.read_bits(1)? != 0 {
                Some(bs.read_bits(5)? as u8)
            } else {
                None
            };
            let (num_tot_trans_20ms, num_tot_trans_5ms) = if bs.read_bits(1)? != 0 {
                (Some(bs.read_bits(4)? as u8), Some(bs.read_bits(4)? as u8))
            } else {
                (None, None)
            };
            let num_preamble_rc1_rc2 = bs.read_bits(3)? as u8;
            let num_preamble = bs.read_bits(3)? as u8;
            let power_delta = bs.read_bits(3)? as u8;
            let nghbr_resq_configured = Self::read_bool_vec(bs, num_nghbr)?;
            if !nghbr_resq_configured.iter().any(|configured| *configured) {
                return Err("GNLM RESQ_ENABLED requires at least one NGHBR_RESQ_CONFIGURED".into());
            }
            Some(GeneralNeighborResqInfo {
                delay_time,
                allowed_time,
                attempt_time,
                code_chan,
                qof,
                min_period,
                num_tot_trans_20ms,
                num_tot_trans_5ms,
                num_preamble_rc1_rc2,
                num_preamble,
                power_delta,
                nghbr_resq_configured,
            })
        } else {
            None
        };

        let pdch_supported = Self::read_bool_vec(bs, num_nghbr)?;

        let hrpd_neighbors = if bs.read_bits(1)? != 0 {
            let num_hrpd_nghbr = bs.read_bits(6)? as usize;
            let mut records = Vec::with_capacity(num_hrpd_nghbr);
            for _ in 0..num_hrpd_nghbr {
                let record_len = bs.read_bits(8)? as usize;
                if record_len == 0 {
                    return Err("GNLM HRPD_NGHBR_REC_LEN must include at least one octet after the length field".into());
                }
                let record_bits = record_len * 8;
                if bs.len() < record_bits {
                    return Err("GNLM HRPD neighbor record length exceeds remaining SDU".into());
                }
                let mut record_bs = bs.drain(0..record_bits);
                records.push(Self::read_hrpd_neighbor_body(&mut record_bs)?);
            }
            Some(records)
        } else {
            None
        };

        if !bs.is_empty() {
            return Err("GNLM has trailing bits after HRPD neighbor section".into());
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            pilot_inc,
            nghbr_srch_mode,
            nghbr_config_pn_incl,
            freq_fields_incl,
            use_timing,
            global_timing,
            neighbors,
            analog_neighbors,
            srch_offset_incl,
            pilot_info,
            bcch_support,
            resq,
            pdch_supported,
            hrpd_neighbors,
        })
    }

    fn validate(&self) {
        assert!(
            (1..=15).contains(&self.pilot_inc),
            "GNLM PILOT_INC must be in the range 1..=15"
        );
        assert!(
            self.neighbors.len() <= 63,
            "GNLM NUM_NGHBR must fit in 6 bits"
        );
        assert!(
            self.analog_neighbors.len() <= 7,
            "GNLM NUM_ANALOG_NGHBR must fit in 3 bits"
        );
        assert!(
            self.pilot_info.len() == self.neighbors.len(),
            "GNLM pilot-info records must match NUM_NGHBR"
        );
        assert!(
            self.pdch_supported.len() == self.neighbors.len(),
            "GNLM PDCH support records must match NUM_NGHBR"
        );
        assert!(
            !self.srch_offset_incl || Self::has_search_window(self.nghbr_srch_mode),
            "GNLM SRCH_OFFSET_INCL requires search-window mode"
        );
        assert!(
            self.use_timing || self.global_timing.is_none(),
            "GNLM GLOBAL_TIMING_INCL requires USE_TIMING"
        );

        for neighbor in &self.neighbors {
            if self.nghbr_config_pn_incl {
                let config = neighbor
                    .nghbr_config
                    .expect("GNLM NGHBR_CONFIG must be present when NGHBR_CONFIG_PN_INCL=1");
                assert!(
                    config <= 0b011,
                    "GNLM NGHBR_CONFIG 0b100..0b111 is reserved"
                );
                neighbor
                    .nghbr_pn
                    .expect("GNLM NGHBR_PN must be present when NGHBR_CONFIG_PN_INCL=1");
            } else {
                assert!(
                    neighbor.nghbr_config.is_none() && neighbor.nghbr_pn.is_none(),
                    "GNLM NGHBR_CONFIG/NGHBR_PN must be absent when NGHBR_CONFIG_PN_INCL=0"
                );
            }
            assert!(
                Self::has_search_priority(self.nghbr_srch_mode)
                    == neighbor.search_priority.is_some(),
                "GNLM SEARCH_PRIORITY presence must match NGHBR_SRCH_MODE"
            );
            assert!(
                Self::has_search_window(self.nghbr_srch_mode) == neighbor.srch_win_nghbr.is_some(),
                "GNLM SRCH_WIN_NGHBR presence must match NGHBR_SRCH_MODE"
            );
            if self.freq_fields_incl {
                assert!(
                    neighbor.nghbr_band.is_some() == neighbor.nghbr_freq.is_some(),
                    "GNLM FREQ_INCL requires both NGHBR_BAND and NGHBR_FREQ"
                );
            } else {
                assert!(
                    neighbor.nghbr_band.is_none() && neighbor.nghbr_freq.is_none(),
                    "GNLM frequency fields must be absent when FREQ_FIELDS_INCL=0"
                );
            }
            if self.use_timing {
                if let Some(timing) = &neighbor.timing {
                    if self.global_timing.is_some() {
                        assert!(
                            timing.tx_duration.is_none() && timing.tx_period.is_none(),
                            "GNLM per-neighbor duration/period must be absent when GLOBAL_TIMING_INCL=1"
                        );
                    } else {
                        timing
                            .tx_duration
                            .expect("GNLM NGHBR_TX_DURATION required when TIMING_INCL=1 and GLOBAL_TIMING_INCL=0");
                        timing
                            .tx_period
                            .expect("GNLM NGHBR_TX_PERIOD required when TIMING_INCL=1 and GLOBAL_TIMING_INCL=0");
                    }
                }
            } else {
                assert!(
                    neighbor.timing.is_none(),
                    "GNLM TIMING_INCL must be absent when USE_TIMING=0"
                );
            }
        }
        for pilot in &self.pilot_info {
            assert!(
                self.srch_offset_incl == pilot.srch_offset_nghbr.is_some(),
                "GNLM SRCH_OFFSET_NGHBR presence must match SRCH_OFFSET_INCL"
            );
        }
        if let Some(bcch_support) = &self.bcch_support {
            assert!(
                bcch_support.len() == self.neighbors.len(),
                "GNLM BCCH_SUPPORT records must match NUM_NGHBR"
            );
        }
        if let Some(resq) = &self.resq {
            assert!(
                resq.nghbr_resq_configured.len() == self.neighbors.len(),
                "GNLM NGHBR_RESQ_CONFIGURED records must match NUM_NGHBR"
            );
            assert!(
                resq.nghbr_resq_configured
                    .iter()
                    .any(|configured| *configured),
                "GNLM RESQ_ENABLED requires at least one NGHBR_RESQ_CONFIGURED"
            );
            assert!(
                resq.num_tot_trans_20ms.is_some() == resq.num_tot_trans_5ms.is_some(),
                "GNLM RESQ_NUM_TOT_TRANS_INCL requires both 20ms and 5ms values"
            );
        }
        if let Some(hrpd_neighbors) = &self.hrpd_neighbors {
            assert!(
                hrpd_neighbors.len() <= 63,
                "GNLM NUM_HRPD_NGHBR must fit in 6 bits"
            );
        }
    }

    fn has_search_priority(nghbr_srch_mode: u8) -> bool {
        matches!(nghbr_srch_mode, 0b01 | 0b11)
    }

    fn has_search_window(nghbr_srch_mode: u8) -> bool {
        matches!(nghbr_srch_mode, 0b10 | 0b11)
    }

    fn write_bool_slice(bs: &mut Bitstream, values: &[bool]) {
        for value in values {
            bs.write_u8(*value as u8, 1);
        }
    }

    fn read_bool_vec(bs: &mut Bitstream, len: usize) -> Result<Vec<bool>, crate::error::Error> {
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(bs.read_bits(1)? != 0);
        }
        Ok(values)
    }

    fn read_zero_tail(bs: &mut Bitstream, context: &str) -> Result<(), crate::error::Error> {
        while !bs.is_empty() {
            if bs.read_bits(1)? != 0 {
                return Err(format!("GNLM {context} reserved padding bits must be zero").into());
            }
        }
        Ok(())
    }

    fn write_walsh_info(bs: &mut Bitstream, info: &Sr3AuxPilotInfo, field_name: &str) {
        assert!(
            info.walsh_length <= 0b011,
            "GNLM {field_name} WALSH_LENGTH 0b100..0b111 is reserved"
        );
        let walsh_bits = info.walsh_length as usize + 6;
        assert!(
            (info.aux_pilot_walsh as u32) < (1u32 << walsh_bits),
            "GNLM {field_name} AUX_PILOT_WALSH exceeds WALSH_LENGTH"
        );
        bs.write_u8(info.qof, 2);
        bs.write_u8(info.walsh_length, 3);
        bs.write_u32(info.aux_pilot_walsh as u32, walsh_bits);
    }

    fn read_walsh_info(
        bs: &mut Bitstream,
        field_name: &str,
    ) -> Result<Sr3AuxPilotInfo, crate::error::Error> {
        let qof = bs.read_bits(2)? as u8;
        let walsh_length = bs.read_bits(3)? as u8;
        if walsh_length > 0b011 {
            return Err(format!("GNLM {field_name} WALSH_LENGTH 0b100..0b111 is reserved").into());
        }
        let aux_pilot_walsh = bs.read_bits(walsh_length as usize + 6)? as u16;
        Ok(Sr3AuxPilotInfo {
            qof,
            walsh_length,
            aux_pilot_walsh,
        })
    }

    fn write_pilot_record(record: &GeneralNeighborPilotRecord, bs: &mut Bitstream) {
        match record {
            GeneralNeighborPilotRecord::OneXCommonWithTransmitDiversity {
                td_power_level,
                td_mode,
            } => {
                bs.write_u8(*td_power_level, 2);
                bs.write_u8(*td_mode, 2);
                bs.write_u8(0, 4);
            }
            GeneralNeighborPilotRecord::OneXAuxiliary {
                qof,
                walsh_length,
                aux_pilot_walsh,
            } => {
                Self::write_walsh_info(
                    bs,
                    &Sr3AuxPilotInfo {
                        qof: *qof,
                        walsh_length: *walsh_length,
                        aux_pilot_walsh: *aux_pilot_walsh,
                    },
                    "1X auxiliary",
                );
            }
            GeneralNeighborPilotRecord::OneXAuxiliaryWithTransmitDiversity {
                qof,
                walsh_length,
                aux_walsh,
                aux_td_power_level,
                td_mode,
            } => {
                Self::write_walsh_info(
                    bs,
                    &Sr3AuxPilotInfo {
                        qof: *qof,
                        walsh_length: *walsh_length,
                        aux_pilot_walsh: *aux_walsh,
                    },
                    "1X auxiliary TD",
                );
                bs.write_u8(*aux_td_power_level, 2);
                bs.write_u8(*td_mode, 2);
            }
            GeneralNeighborPilotRecord::ThreeXCommon {
                sr3_primary_pilot,
                sr3_pilot_power1,
                sr3_pilot_power2,
            } => {
                bs.write_u8(*sr3_primary_pilot, 2);
                bs.write_u8(*sr3_pilot_power1, 3);
                bs.write_u8(*sr3_pilot_power2, 3);
            }
            GeneralNeighborPilotRecord::ThreeXAuxiliary {
                sr3_primary_pilot,
                sr3_pilot_power1,
                sr3_pilot_power2,
                primary_aux,
                lower_aux,
                upper_aux,
            } => {
                bs.write_u8(*sr3_primary_pilot, 2);
                bs.write_u8(*sr3_pilot_power1, 3);
                bs.write_u8(*sr3_pilot_power2, 3);
                Self::write_walsh_info(bs, primary_aux, "3X primary auxiliary");
                if let Some(lower_aux) = lower_aux {
                    bs.write_u8(1, 1);
                    Self::write_walsh_info(bs, lower_aux, "3X lower auxiliary");
                } else {
                    bs.write_u8(0, 1);
                }
                if let Some(upper_aux) = upper_aux {
                    bs.write_u8(1, 1);
                    Self::write_walsh_info(bs, upper_aux, "3X upper auxiliary");
                } else {
                    bs.write_u8(0, 1);
                }
            }
        }
    }

    fn read_pilot_record(
        record_type: u8,
        bs: &mut Bitstream,
    ) -> Result<GeneralNeighborPilotRecord, crate::error::Error> {
        let record = match record_type {
            0b000 => {
                let td_power_level = bs.read_bits(2)? as u8;
                let td_mode = bs.read_bits(2)? as u8;
                let reserved = bs.read_bits(4)? as u8;
                if reserved != 0 {
                    return Err("GNLM 1X common TD pilot reserved bits must be zero".into());
                }
                GeneralNeighborPilotRecord::OneXCommonWithTransmitDiversity {
                    td_power_level,
                    td_mode,
                }
            }
            0b001 => {
                let info = Self::read_walsh_info(bs, "1X auxiliary")?;
                Self::read_zero_tail(bs, "1X auxiliary pilot record")?;
                return Ok(GeneralNeighborPilotRecord::OneXAuxiliary {
                    qof: info.qof,
                    walsh_length: info.walsh_length,
                    aux_pilot_walsh: info.aux_pilot_walsh,
                });
            }
            0b010 => {
                let info = Self::read_walsh_info(bs, "1X auxiliary TD")?;
                let aux_td_power_level = bs.read_bits(2)? as u8;
                let td_mode = bs.read_bits(2)? as u8;
                Self::read_zero_tail(bs, "1X auxiliary TD pilot record")?;
                return Ok(
                    GeneralNeighborPilotRecord::OneXAuxiliaryWithTransmitDiversity {
                        qof: info.qof,
                        walsh_length: info.walsh_length,
                        aux_walsh: info.aux_pilot_walsh,
                        aux_td_power_level,
                        td_mode,
                    },
                );
            }
            0b011 => GeneralNeighborPilotRecord::ThreeXCommon {
                sr3_primary_pilot: bs.read_bits(2)? as u8,
                sr3_pilot_power1: bs.read_bits(3)? as u8,
                sr3_pilot_power2: bs.read_bits(3)? as u8,
            },
            0b100 => {
                let sr3_primary_pilot = bs.read_bits(2)? as u8;
                let sr3_pilot_power1 = bs.read_bits(3)? as u8;
                let sr3_pilot_power2 = bs.read_bits(3)? as u8;
                let primary_aux = Self::read_walsh_info(bs, "3X primary auxiliary")?;
                let lower_aux = if bs.read_bits(1)? != 0 {
                    Some(Self::read_walsh_info(bs, "3X lower auxiliary")?)
                } else {
                    None
                };
                let upper_aux = if bs.read_bits(1)? != 0 {
                    Some(Self::read_walsh_info(bs, "3X upper auxiliary")?)
                } else {
                    None
                };
                Self::read_zero_tail(bs, "3X auxiliary pilot record")?;
                return Ok(GeneralNeighborPilotRecord::ThreeXAuxiliary {
                    sr3_primary_pilot,
                    sr3_pilot_power1,
                    sr3_pilot_power2,
                    primary_aux,
                    lower_aux,
                    upper_aux,
                });
            }
            _ => {
                return Err(
                    format!("GNLM reserved NGHBR_PILOT_REC_TYPE 0b{record_type:03b}").into(),
                );
            }
        };
        if !bs.is_empty() {
            return Err("GNLM pilot record has trailing bits".into());
        }
        Ok(record)
    }

    fn write_hrpd_neighbor_body(record: &HrpdNeighborRecord, bs: &mut Bitstream) {
        bs.write_u32(record.nghbr_pn as u32, 9);
        match (record.nghbr_band, record.nghbr_freq) {
            (Some(band), Some(freq)) => {
                bs.write_u8(1, 1);
                bs.write_u8(band, 5);
                bs.write_u32(freq as u32, 11);
            }
            (None, None) => bs.write_u8(0, 1),
            _ => panic!("GNLM HRPD frequency inclusion requires both NGHBR_BAND and NGHBR_FREQ"),
        }
        bs.write_u8(record.pn_association_ind as u8, 1);
        bs.write_u8(record.data_association_ind as u8, 1);
    }

    fn read_hrpd_neighbor_body(
        bs: &mut Bitstream,
    ) -> Result<HrpdNeighborRecord, crate::error::Error> {
        let nghbr_pn = bs.read_bits(9)? as u16;
        let (nghbr_band, nghbr_freq) = if bs.read_bits(1)? != 0 {
            (Some(bs.read_bits(5)? as u8), Some(bs.read_bits(11)? as u16))
        } else {
            (None, None)
        };
        let pn_association_ind = bs.read_bits(1)? != 0;
        let data_association_ind = bs.read_bits(1)? != 0;
        Self::read_zero_tail(bs, "HRPD neighbor record")?;
        Ok(HrpdNeighborRecord {
            nghbr_pn,
            nghbr_band,
            nghbr_freq,
            pn_association_ind,
            data_association_ind,
        })
    }
}

impl GeneralNeighborPilotRecord {
    fn record_type(&self) -> u8 {
        match self {
            Self::OneXCommonWithTransmitDiversity { .. } => 0b000,
            Self::OneXAuxiliary { .. } => 0b001,
            Self::OneXAuxiliaryWithTransmitDiversity { .. } => 0b010,
            Self::ThreeXCommon { .. } => 0b011,
            Self::ThreeXAuxiliary { .. } => 0b100,
        }
    }
}

impl UserZoneIdentificationMessage {
    /// Encode UZIM per C.S0005-E 3.7.2.3.2.23.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(self.zones.len() <= 15, "UZIM NUM_UZID must fit in 4 bits");

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.uz_exit, 4);
        bs.write_u8(self.zones.len() as u8, 4);
        for zone in &self.zones {
            bs.write_u32(zone.uzid as u32, 16);
            bs.write_u8(zone.uz_rev, 4);
            bs.write_u8(zone.temp_sub as u8, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let uz_exit = bs.read_bits(4)? as u8;
        let num_uzid = bs.read_bits(4)? as usize;
        if bs.len() < num_uzid * 21 {
            return Err("UZIM zone record list truncated".into());
        }
        let mut zones = Vec::with_capacity(num_uzid);
        for _ in 0..num_uzid {
            zones.push(UserZoneRecord {
                uzid: bs.read_bits(16)? as u16,
                uz_rev: bs.read_bits(4)? as u8,
                temp_sub: bs.read_bits(1)? != 0,
            });
        }
        if !bs.is_empty() {
            return Err("UZIM has trailing bits after zone records".into());
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            uz_exit,
            zones,
        })
    }
}

impl PrivateNeighborListMessage {
    /// Encode PNLM per C.S0005-E 3.7.2.3.2.24.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.radio_interfaces.len() <= 15,
            "PNLM NUM_RADIO_INTERFACE must fit in 4 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.radio_interfaces.len() as u8, 4);
        for radio_interface in &self.radio_interfaces {
            let mut body = Bitstream::new();
            Self::write_mc_radio_interface(radio_interface, &mut body);
            pad_to_octet(&mut body);
            let body_len = body.len() / 8;
            assert!(
                body_len <= u8::MAX as usize,
                "PNLM RADIO_INTERFACE_LEN must fit in one octet"
            );
            bs.write_u8(0, 4); // RADIO_INTERFACE_TYPE: MC system
            bs.write_u8(body_len as u8, 8);
            bs.extend(&body);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let num_radio_interface = bs.read_bits(4)? as usize;
        let mut radio_interfaces = Vec::with_capacity(num_radio_interface);
        for _ in 0..num_radio_interface {
            let radio_interface_type = bs.read_bits(4)? as u8;
            let radio_interface_len = bs.read_bits(8)? as usize;
            if radio_interface_type != 0 {
                return Err(format!(
                    "PNLM reserved RADIO_INTERFACE_TYPE 0b{radio_interface_type:04b}"
                )
                .into());
            }
            let body_bits = radio_interface_len * 8;
            if bs.len() < body_bits {
                return Err("PNLM radio-interface body length exceeds remaining SDU".into());
            }
            let mut body = bs.drain(0..body_bits);
            radio_interfaces.push(Self::read_mc_radio_interface(&mut body)?);
        }
        if !bs.is_empty() {
            return Err("PNLM has trailing bits after radio-interface records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            radio_interfaces,
        })
    }

    fn write_mc_radio_interface(record: &PrivateRadioInterfaceRecord, bs: &mut Bitstream) {
        assert!(
            record.neighbors.len() <= 63,
            "PNLM NUM_PRI_NGHBR must fit in 6 bits"
        );
        let common_incl = record.common_band_class.is_some() || record.common_nghbr_freq.is_some();
        assert!(
            record.common_band_class.is_some() == record.common_nghbr_freq.is_some(),
            "PNLM COMMON_INCL requires both COMMON_BAND_CLASS and COMMON_NGHBR_FREQ"
        );

        bs.write_u8(common_incl as u8, 1);
        if common_incl {
            bs.write_u8(record.common_band_class.expect("validated"), 5);
            bs.write_u32(record.common_nghbr_freq.expect("validated") as u32, 11);
        }
        bs.write_u8(record.srch_win_pn, 4);
        bs.write_u8(record.neighbors.len() as u8, 6);
        for neighbor in &record.neighbors {
            if common_incl {
                assert!(
                    neighbor.band_class.is_none() && neighbor.nghbr_freq.is_none(),
                    "PNLM per-neighbor frequency fields must be absent when COMMON_INCL=1"
                );
            } else {
                assert!(
                    neighbor.band_class.is_some() && neighbor.nghbr_freq.is_some(),
                    "PNLM per-neighbor frequency fields must be present when COMMON_INCL=0"
                );
            }
            bs.write_u32(neighbor.sid as u32, 15);
            bs.write_u32(neighbor.nid as u32, 16);
            bs.write_u32(neighbor.pri_nghbr_pn as u32, 9);
            if let Some(pilot_record) = &neighbor.pilot_record {
                bs.write_u8(1, 1);
                bs.write_u8(pilot_record.record_type(), 3);
                let mut pilot_bs = Bitstream::new();
                GeneralNeighborListMessage::write_pilot_record(pilot_record, &mut pilot_bs);
                pad_to_octet(&mut pilot_bs);
                let record_len = pilot_bs.len() / 8;
                assert!(record_len <= 7, "PNLM pilot RECORD_LEN must fit in 3 bits");
                bs.write_u8(record_len as u8, 3);
                bs.extend(&pilot_bs);
            } else {
                bs.write_u8(0, 1);
            }
            if !common_incl {
                bs.write_u8(neighbor.band_class.expect("validated"), 5);
                bs.write_u32(neighbor.nghbr_freq.expect("validated") as u32, 11);
            }
            if let Some(zones) = &neighbor.zones {
                assert!(zones.len() <= 15, "PNLM NUM_UZID must fit in 4 bits");
                bs.write_u8(1, 1);
                bs.write_u8(zones.len() as u8, 4);
                for zone in zones {
                    bs.write_u32(zone.uzid as u32, 16);
                    bs.write_u8(zone.uz_rev, 4);
                    bs.write_u8(zone.temp_sub as u8, 1);
                }
            } else {
                bs.write_u8(0, 1);
            }
        }
    }

    fn read_mc_radio_interface(
        bs: &mut Bitstream,
    ) -> Result<PrivateRadioInterfaceRecord, crate::error::Error> {
        let common_incl = bs.read_bits(1)? != 0;
        let (common_band_class, common_nghbr_freq) = if common_incl {
            (Some(bs.read_bits(5)? as u8), Some(bs.read_bits(11)? as u16))
        } else {
            (None, None)
        };
        let srch_win_pn = bs.read_bits(4)? as u8;
        let num_pri_nghbr = bs.read_bits(6)? as usize;
        let mut neighbors = Vec::with_capacity(num_pri_nghbr);
        for _ in 0..num_pri_nghbr {
            let sid = bs.read_bits(15)? as u16;
            let nid = bs.read_bits(16)? as u16;
            let pri_nghbr_pn = bs.read_bits(9)? as u16;
            let pilot_record = if bs.read_bits(1)? != 0 {
                let record_type = bs.read_bits(3)? as u8;
                let record_len = bs.read_bits(3)? as usize;
                if record_len == 0 {
                    return Err(
                        "PNLM pilot RECORD_LEN must be non-zero when ADD_PILOT_REC_INCL is set"
                            .into(),
                    );
                }
                let record_bits = record_len * 8;
                if bs.len() < record_bits {
                    return Err("PNLM pilot record length exceeds radio-interface body".into());
                }
                let mut pilot_bs = bs.drain(0..record_bits);
                Some(GeneralNeighborListMessage::read_pilot_record(
                    record_type,
                    &mut pilot_bs,
                )?)
            } else {
                None
            };
            let (band_class, nghbr_freq) = if common_incl {
                (None, None)
            } else {
                (Some(bs.read_bits(5)? as u8), Some(bs.read_bits(11)? as u16))
            };
            let zones = if bs.read_bits(1)? != 0 {
                let num_uzid = bs.read_bits(4)? as usize;
                let mut zones = Vec::with_capacity(num_uzid);
                for _ in 0..num_uzid {
                    zones.push(UserZoneRecord {
                        uzid: bs.read_bits(16)? as u16,
                        uz_rev: bs.read_bits(4)? as u8,
                        temp_sub: bs.read_bits(1)? != 0,
                    });
                }
                Some(zones)
            } else {
                None
            };
            neighbors.push(PrivateNeighborRecord {
                sid,
                nid,
                pri_nghbr_pn,
                pilot_record,
                band_class,
                nghbr_freq,
                zones,
            });
        }
        GeneralNeighborListMessage::read_zero_tail(bs, "private radio-interface record")?;
        Ok(PrivateRadioInterfaceRecord {
            common_band_class,
            common_nghbr_freq,
            srch_win_pn,
            neighbors,
        })
    }
}

impl ExtendedGlobalServiceRedirectionMessage {
    /// Encode EGSRDM per C.S0005-E 3.7.2.3.2.27.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.additional_records.len() <= 7,
            "EGSRDM NUM_ADD_RECORD must fit in 3 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        Self::write_target(&mut bs, &self.primary, true);
        bs.write_u8(self.return_if_fail as u8, 1);
        bs.write_u8(self.primary.delete_tmsi as u8, 1);
        Self::write_p_rev(&mut bs, &self.primary.p_rev, "EGSRDM");
        let (record_type, record) = Self::encode_redirection_record(&self.primary.record);
        bs.write_u8(record_type, 8);
        bs.write_u8(record.len() as u8, 8);
        for byte in &record {
            bs.write_u8(*byte, 8);
        }
        bs.write_u8(self.primary.last_search_record_ind as u8, 1);
        bs.write_u8(self.additional_records.len() as u8, 3);
        for target in &self.additional_records {
            Self::write_target(&mut bs, target, false);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let redirect_accolc = bs.read_bits(16)? as u16;
        let return_if_fail = bs.read_bits(1)? != 0;
        let delete_tmsi = bs.read_bits(1)? != 0;
        let p_rev = Self::read_p_rev(bs, "EGSRDM")?;
        let record = Self::read_redirection_record(bs, "EGSRDM")?;
        let last_search_record_ind = bs.read_bits(1)? != 0;
        let primary = ExtendedGlobalRedirectionTarget {
            redirect_accolc,
            delete_tmsi,
            p_rev,
            record,
            last_search_record_ind,
        };

        let num_add_record = bs.read_bits(3)? as usize;
        let mut additional_records = Vec::with_capacity(num_add_record);
        for _ in 0..num_add_record {
            let redirect_accolc = bs.read_bits(16)? as u16;
            let delete_tmsi = bs.read_bits(1)? != 0;
            let p_rev = Self::read_p_rev(bs, "EGSRDM additional")?;
            let record = Self::read_redirection_record(bs, "EGSRDM additional")?;
            let last_search_record_ind = bs.read_bits(1)? != 0;
            additional_records.push(ExtendedGlobalRedirectionTarget {
                redirect_accolc,
                delete_tmsi,
                p_rev,
                record,
                last_search_record_ind,
            });
        }
        if !bs.is_empty() {
            return Err("EGSRDM has trailing bits after additional records".into());
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            return_if_fail,
            primary,
            additional_records,
        })
    }

    fn write_target(bs: &mut Bitstream, target: &ExtendedGlobalRedirectionTarget, primary: bool) {
        bs.write_u32(target.redirect_accolc as u32, 16);
        if !primary {
            bs.write_u8(target.delete_tmsi as u8, 1);
            Self::write_p_rev(bs, &target.p_rev, "EGSRDM additional");
            let (record_type, record) = Self::encode_redirection_record(&target.record);
            bs.write_u8(record_type, 8);
            bs.write_u8(record.len() as u8, 8);
            for byte in &record {
                bs.write_u8(*byte, 8);
            }
            bs.write_u8(target.last_search_record_ind as u8, 1);
        }
    }

    fn write_p_rev(bs: &mut Bitstream, p_rev: &Option<RedirectPRevRange>, context: &str) {
        if let Some(p_rev) = p_rev {
            assert!(
                p_rev.min >= 6 && p_rev.max >= 6 && p_rev.min <= p_rev.max,
                "{context} redirect P_REV range must be >= 6 and ordered"
            );
            bs.write_u8(1, 1);
            bs.write_u8(p_rev.exclude as u8, 1);
            bs.write_u8(p_rev.min, 8);
            bs.write_u8(p_rev.max, 8);
        } else {
            bs.write_u8(0, 1);
        }
    }

    fn read_p_rev(
        bs: &mut Bitstream,
        context: &str,
    ) -> Result<Option<RedirectPRevRange>, crate::error::Error> {
        if bs.read_bits(1)? == 0 {
            return Ok(None);
        }
        let exclude = bs.read_bits(1)? != 0;
        let min = bs.read_bits(8)? as u8;
        let max = bs.read_bits(8)? as u8;
        if min < 6 || max < 6 || min > max {
            return Err(format!("{context} redirect P_REV range must be >= 6 and ordered").into());
        }
        Ok(Some(RedirectPRevRange { exclude, min, max }))
    }

    fn encode_redirection_record(record: &ExtendedRedirectionRecord) -> (u8, Vec<u8>) {
        match record {
            ExtendedRedirectionRecord::NdssOff => (0x00, Vec::new()),
            ExtendedRedirectionRecord::Cdma {
                band_class,
                expected_sid,
                expected_nid,
                cdma_chans,
                redirect_subclasses,
            } => {
                assert!(
                    cdma_chans.len() <= 15,
                    "EGSRDM NUM_CHANS must fit in 4 bits"
                );
                let mut bs = Bitstream::new();
                bs.write_u8(*band_class, 5);
                bs.write_u32(*expected_sid as u32, 15);
                bs.write_u32(*expected_nid as u32, 16);
                bs.write_u8(0, 4);
                bs.write_u8(cdma_chans.len() as u8, 4);
                for chan in cdma_chans {
                    bs.write_u32(*chan as u32, 11);
                }
                if let Some(subclasses) = redirect_subclasses {
                    assert!(
                        (1..=32).contains(&subclasses.len()),
                        "EGSRDM REDIRECT_SUBCLASS count must be in the range 1..=32"
                    );
                    bs.write_u8(1, 1);
                    bs.write_u8((subclasses.len() - 1) as u8, 5);
                    GeneralNeighborListMessage::write_bool_slice(&mut bs, subclasses);
                } else {
                    bs.write_u8(0, 1);
                }
                pad_to_octet(&mut bs);
                (0x02, bitstream_to_byte_vec(&bs))
            }
            ExtendedRedirectionRecord::Ds41(data) => {
                assert!(
                    data.len() <= u8::MAX as usize,
                    "EGSRDM DS-41 RECORD_LEN must fit in one octet"
                );
                (0x05, data.clone())
            }
        }
    }

    fn read_redirection_record(
        bs: &mut Bitstream,
        context: &str,
    ) -> Result<ExtendedRedirectionRecord, crate::error::Error> {
        let record_type = bs.read_bits(8)? as u8;
        let record_len = bs.read_bits(8)? as usize;
        if !valid_redirection_record_type(record_type) {
            return Err(format!("{context} reserved RECORD_TYPE 0x{record_type:02x}").into());
        }
        if record_type == 0 {
            if record_len != 0 {
                return Err(format!("{context} NDSS off indication requires RECORD_LEN=0").into());
            }
            return Ok(ExtendedRedirectionRecord::NdssOff);
        }
        if bs.len() < record_len * 8 {
            return Err(
                format!("{context} redirection record length exceeds remaining SDU").into(),
            );
        }
        let mut record_bs = bs.drain(0..record_len * 8);
        match record_type {
            0x02 => Self::read_cdma_redirection_record(&mut record_bs, context),
            0x05 => {
                let mut data = Vec::with_capacity(record_len);
                while !record_bs.is_empty() {
                    data.push(record_bs.read_bits(8)? as u8);
                }
                Ok(ExtendedRedirectionRecord::Ds41(data))
            }
            _ => unreachable!("validated redirection record type"),
        }
    }

    fn read_cdma_redirection_record(
        bs: &mut Bitstream,
        context: &str,
    ) -> Result<ExtendedRedirectionRecord, crate::error::Error> {
        let band_class = bs.read_bits(5)? as u8;
        let expected_sid = bs.read_bits(15)? as u16;
        let expected_nid = bs.read_bits(16)? as u16;
        let reserved = bs.read_bits(4)? as u8;
        if reserved != 0 {
            return Err(format!("{context} CDMA redirection reserved field must be zero").into());
        }
        let num_chans = bs.read_bits(4)? as usize;
        let mut cdma_chans = Vec::with_capacity(num_chans);
        for _ in 0..num_chans {
            cdma_chans.push(bs.read_bits(11)? as u16);
        }
        let redirect_subclasses = if bs.read_bits(1)? != 0 {
            let subclass_rec_len = bs.read_bits(5)? as usize;
            Some(GeneralNeighborListMessage::read_bool_vec(
                bs,
                subclass_rec_len + 1,
            )?)
        } else {
            None
        };
        GeneralNeighborListMessage::read_zero_tail(bs, "EGSRDM CDMA redirection record")?;
        Ok(ExtendedRedirectionRecord::Cdma {
            band_class,
            expected_sid,
            expected_nid,
            cdma_chans,
            redirect_subclasses,
        })
    }
}

impl ExtendedCdmaChannelListMessage {
    /// Encode ECCLM per C.S0005-E 3.7.2.3.2.28.
    pub fn to_sdu(&self) -> Bitstream {
        self.validate();
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.cdma_freqs.len() as u8, 4);
        for freq in &self.cdma_freqs {
            bs.write_u32(*freq as u32, 11);
        }
        if let Some(values) = &self.rc_qpch_hash_ind {
            bs.write_u8(1, 1);
            GeneralNeighborListMessage::write_bool_slice(&mut bs, values);
        } else {
            bs.write_u8(0, 1);
        }
        if let Some(td) = &self.td_selection {
            bs.write_u8(1, 1);
            bs.write_u8(td.td_mode, 2);
            for freq in &td.frequencies {
                bs.write_u8(freq.td_hash_ind as u8, 1);
                if freq.td_hash_ind {
                    bs.write_u8(freq.td_power_level.expect("validated"), 2);
                }
            }
        } else {
            bs.write_u8(0, 1);
        }
        bs.write_u8(self.cdma_band, 5);
        Self::write_subclasses(&mut bs, &self.subclasses, "ECCLM CDMA_SUBCLASS");
        if let Some(weights) = &self.cdma_freq_weights {
            bs.write_u8(1, 1);
            for weight in weights {
                bs.write_u8(*weight, 3);
            }
        } else {
            bs.write_u8(0, 1);
        }
        bs.write_u8(self.additional_bands.len() as u8, 3);
        for band in &self.additional_bands {
            bs.write_u8(band.add_cdma_band, 5);
            Self::write_subclasses(&mut bs, &band.subclasses, "ECCLM ADD_CDMA_SUBCLASS");
            if let Some(td) = &self.td_selection {
                bs.write_u8(band.add_td_mode.expect("validated"), 2);
                assert!(td.td_mode <= 1, "ECCLM TD_MODE 0b10..0b11 is reserved");
            }
            bs.write_u8(band.bypass_sys_det_ind as u8, 1);
            bs.write_u8(band.frequencies.len() as u8, 4);
            for freq in &band.frequencies {
                bs.write_u32(freq.add_cdma_freq as u32, 11);
                if self.rc_qpch_hash_ind.is_some() {
                    bs.write_u8(freq.add_rc_qpch_hash_ind.expect("validated") as u8, 1);
                }
                if self.td_selection.is_some() {
                    let td_hash = freq.add_td_hash_ind.expect("validated");
                    bs.write_u8(td_hash as u8, 1);
                    if td_hash {
                        bs.write_u8(freq.add_td_power_level.expect("validated"), 2);
                    }
                }
                if self.cdma_freq_weights.is_some() {
                    bs.write_u8(freq.add_cdma_freq_weight.expect("validated"), 3);
                }
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let num_freq = bs.read_bits(4)? as usize;
        if num_freq == 0 {
            return Err("ECCLM NUM_FREQ must be non-zero".into());
        }
        let mut cdma_freqs = Vec::with_capacity(num_freq);
        for _ in 0..num_freq {
            cdma_freqs.push(bs.read_bits(11)? as u16);
        }
        let rc_qpch_hash_ind = if bs.read_bits(1)? != 0 {
            let values = GeneralNeighborListMessage::read_bool_vec(bs, num_freq)?;
            if !values.iter().any(|value| *value) {
                return Err("ECCLM RC_QPCH_SEL_INCL requires at least one RC_QPCH_HASH_IND".into());
            }
            Some(values)
        } else {
            None
        };
        if bs.read_bits(1)? != 0 {
            return Err("ECCLM TD_SEL_INCL must be 0 on the Paging Channel".into());
        }
        let td_selection = None;
        let cdma_band = bs.read_bits(5)? as u8;
        let subclasses = Self::read_subclasses(bs)?;
        let cdma_freq_weights = if bs.read_bits(1)? != 0 {
            let mut weights = Vec::with_capacity(num_freq);
            for _ in 0..num_freq {
                weights.push(bs.read_bits(3)? as u8);
            }
            Some(weights)
        } else {
            None
        };
        let num_band = bs.read_bits(3)? as usize;
        let mut additional_bands = Vec::with_capacity(num_band);
        for _ in 0..num_band {
            let add_cdma_band = bs.read_bits(5)? as u8;
            let subclasses = Self::read_subclasses(bs)?;
            let add_td_mode = if td_selection.is_some() {
                let mode = bs.read_bits(2)? as u8;
                if mode > 1 {
                    return Err("ECCLM ADD_TD_MODE 0b10..0b11 is reserved".into());
                }
                Some(mode)
            } else {
                None
            };
            let bypass_sys_det_ind = bs.read_bits(1)? != 0;
            let num_add_freq = bs.read_bits(4)? as usize;
            let mut frequencies = Vec::with_capacity(num_add_freq);
            for _ in 0..num_add_freq {
                let add_cdma_freq = bs.read_bits(11)? as u16;
                let add_rc_qpch_hash_ind = if rc_qpch_hash_ind.is_some() {
                    Some(bs.read_bits(1)? != 0)
                } else {
                    None
                };
                let (add_td_hash_ind, add_td_power_level) = if td_selection.is_some() {
                    let td_hash = bs.read_bits(1)? != 0;
                    let power = if td_hash {
                        Some(bs.read_bits(2)? as u8)
                    } else {
                        None
                    };
                    (Some(td_hash), power)
                } else {
                    (None, None)
                };
                let add_cdma_freq_weight = if cdma_freq_weights.is_some() {
                    Some(bs.read_bits(3)? as u8)
                } else {
                    None
                };
                frequencies.push(ExtendedCdmaAdditionalFrequency {
                    add_cdma_freq,
                    add_rc_qpch_hash_ind,
                    add_td_hash_ind,
                    add_td_power_level,
                    add_cdma_freq_weight,
                });
            }
            additional_bands.push(ExtendedCdmaAdditionalBand {
                add_cdma_band,
                subclasses,
                add_td_mode,
                bypass_sys_det_ind,
                frequencies,
            });
        }
        if !bs.is_empty() {
            return Err("ECCLM has trailing bits after additional band records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            cdma_freqs,
            rc_qpch_hash_ind,
            td_selection,
            cdma_band,
            subclasses,
            cdma_freq_weights,
            additional_bands,
        })
    }

    fn validate(&self) {
        assert!(
            (1..=15).contains(&self.cdma_freqs.len()),
            "ECCLM NUM_FREQ must be in the range 1..=15"
        );
        assert!(
            self.td_selection.is_none(),
            "ECCLM TD_SEL_INCL must be 0 on the Paging Channel"
        );
        if let Some(values) = &self.rc_qpch_hash_ind {
            assert_eq!(
                values.len(),
                self.cdma_freqs.len(),
                "ECCLM RC_QPCH_HASH_IND records must match NUM_FREQ"
            );
            assert!(
                values.iter().any(|value| *value),
                "ECCLM RC_QPCH_SEL_INCL requires at least one RC_QPCH_HASH_IND"
            );
        }
        if let Some(td) = &self.td_selection {
            assert!(td.td_mode <= 1, "ECCLM TD_MODE 0b10..0b11 is reserved");
            assert_eq!(
                td.frequencies.len(),
                self.cdma_freqs.len(),
                "ECCLM TD records must match NUM_FREQ"
            );
            assert!(
                td.frequencies.iter().any(|freq| freq.td_hash_ind),
                "ECCLM TD_SEL_INCL requires at least one TD_HASH_IND"
            );
            for freq in &td.frequencies {
                assert!(
                    freq.td_hash_ind == freq.td_power_level.is_some(),
                    "ECCLM TD_POWER_LEVEL presence must match TD_HASH_IND"
                );
            }
        }
        if let Some(weights) = &self.cdma_freq_weights {
            assert_eq!(
                weights.len(),
                self.cdma_freqs.len(),
                "ECCLM CDMA_FREQ_WEIGHT records must match NUM_FREQ"
            );
        }
        assert!(
            self.additional_bands.len() <= 7,
            "ECCLM NUM_BAND must fit in 3 bits"
        );
        for band in &self.additional_bands {
            if self.td_selection.is_some() {
                assert!(
                    band.add_td_mode.is_some_and(|mode| mode <= 1),
                    "ECCLM ADD_TD_MODE required and must not be reserved when TD_SEL_INCL=1"
                );
            } else {
                assert!(
                    band.add_td_mode.is_none(),
                    "ECCLM ADD_TD_MODE must be absent when TD_SEL_INCL=0"
                );
            }
            assert!(
                band.frequencies.len() <= 15,
                "ECCLM NUM_ADD_FREQ must fit in 4 bits"
            );
            for freq in &band.frequencies {
                assert!(
                    freq.add_rc_qpch_hash_ind.is_some() == self.rc_qpch_hash_ind.is_some(),
                    "ECCLM ADD_RC_QPCH_HASH_IND presence must match RC_QPCH_SEL_INCL"
                );
                if self.td_selection.is_some() {
                    let td_hash = freq
                        .add_td_hash_ind
                        .expect("ECCLM ADD_TD_HASH_IND required when TD_SEL_INCL=1");
                    assert!(
                        td_hash == freq.add_td_power_level.is_some(),
                        "ECCLM ADD_TD_POWER_LEVEL presence must match ADD_TD_HASH_IND"
                    );
                } else {
                    assert!(
                        freq.add_td_hash_ind.is_none() && freq.add_td_power_level.is_none(),
                        "ECCLM ADD_TD fields must be absent when TD_SEL_INCL=0"
                    );
                }
                assert!(
                    freq.add_cdma_freq_weight.is_some() == self.cdma_freq_weights.is_some(),
                    "ECCLM ADD_CDMA_FREQ_WEIGHT presence must match CDMA_FREQ_WEIGHT_INCL"
                );
            }
        }
    }

    fn write_subclasses(bs: &mut Bitstream, subclasses: &Option<Vec<bool>>, context: &str) {
        if let Some(subclasses) = subclasses {
            assert!(
                (1..=32).contains(&subclasses.len()),
                "{context} count must be in the range 1..=32"
            );
            bs.write_u8(1, 1);
            bs.write_u8((subclasses.len() - 1) as u8, 5);
            GeneralNeighborListMessage::write_bool_slice(bs, subclasses);
        } else {
            bs.write_u8(0, 1);
        }
    }

    fn read_subclasses(bs: &mut Bitstream) -> Result<Option<Vec<bool>>, crate::error::Error> {
        if bs.read_bits(1)? == 0 {
            return Ok(None);
        }
        let rec_len = bs.read_bits(5)? as usize;
        Ok(Some(GeneralNeighborListMessage::read_bool_vec(
            bs,
            rec_len + 1,
        )?))
    }
}

impl UserZoneRejectMessage {
    /// Encode UZRM per C.S0005-E 3.7.2.3.2.29.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.reject_action_indi <= 0b100,
            "UZRM REJECT_ACTION_INDI 0b101..0b111 is reserved"
        );
        let mut bs = Bitstream::new();
        bs.write_u32(self.reject_uzid as u32, 16);
        bs.write_u8(self.reject_action_indi, 3);
        if let Some(assign_uzid) = self.assign_uzid {
            bs.write_u8(1, 1);
            bs.write_u32(assign_uzid as u32, 16);
        } else {
            bs.write_u8(0, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let reject_uzid = bs.read_bits(16)? as u16;
        let reject_action_indi = bs.read_bits(3)? as u8;
        if reject_action_indi > 0b100 {
            return Err("UZRM REJECT_ACTION_INDI 0b101..0b111 is reserved".into());
        }
        let assign_uzid = if bs.read_bits(1)? != 0 {
            Some(bs.read_bits(16)? as u16)
        } else {
            None
        };
        if !bs.is_empty() {
            return Err("UZRM has trailing bits after ASSIGN_UZID".into());
        }
        Ok(Self {
            reject_uzid,
            reject_action_indi,
            assign_uzid,
        })
    }
}

impl Ansi41SystemParametersMessage {
    /// Encode A41SPM per C.S0005-E 3.7.2.3.2.30.
    pub fn to_sdu(&self) -> Bitstream {
        self.validate();

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u32(self.sid as u32, 15);
        bs.write_u32(self.nid as u32, 16);
        bs.write_u8(self.packet_zone_id, 8);
        bs.write_u32(self.reg_zone as u32, 12);
        bs.write_u8(self.total_zones, 3);
        bs.write_u8(self.zone_timer, 3);
        bs.write_u8(self.mult_sids as u8, 1);
        bs.write_u8(self.mult_nids as u8, 1);
        bs.write_u8(self.home_reg as u8, 1);
        bs.write_u8(self.for_sid_reg as u8, 1);
        bs.write_u8(self.for_nid_reg as u8, 1);
        bs.write_u8(self.power_up_reg as u8, 1);
        bs.write_u8(self.power_down_reg as u8, 1);
        bs.write_u8(self.parameter_reg as u8, 1);
        bs.write_u8(self.reg_prd, 7);
        if let Some(reg_dist) = self.reg_dist {
            bs.write_u8(1, 1);
            bs.write_u32(reg_dist as u32, 11);
        } else {
            bs.write_u8(0, 1);
        }
        bs.write_u8(self.delete_for_tmsi as u8, 1);
        bs.write_u8(self.use_tmsi as u8, 1);
        bs.write_u8(self.pref_msid_type, 2);
        bs.write_u8(self.tmsi_zone.len() as u8, 4);
        for byte in &self.tmsi_zone {
            bs.write_u8(*byte, 8);
        }
        bs.write_u8(self.imsi_t_supported as u8, 1);
        bs.write_u8(self.max_num_alt_so, 3);
        if let Some(interval) = self.auto_msg_interval {
            bs.write_u8(1, 1);
            bs.write_u8(interval, 3);
        } else {
            bs.write_u8(0, 1);
        }
        if let Some(other) = &self.other_info {
            bs.write_u8(1, 1);
            bs.write_u32(other.base_id as u32, 16);
            bs.write_u32(other.mcc as u32, 10);
            bs.write_u8(other.imsi_11_12, 7);
            bs.write_u8(other.broadcast_gps_asst as u8, 1);
            bs.write_u8(other.sig_encrypt_sup, 8);
        } else {
            bs.write_u8(0, 1);
        }
        bs.write_u8(self.cs_supported as u8, 1);
        bs.write_u8(self.ms_init_pos_loc_sup_ind as u8, 1);
        bs.write_u8(self.msg_integrity_sup as u8, 1);
        if self.msg_integrity_sup {
            if let Some(sig_integrity_sup) = self.sig_integrity_sup {
                bs.write_u8(1, 1);
                bs.write_u8(sig_integrity_sup, 8);
            } else {
                bs.write_u8(0, 1);
            }
        }
        if let Some(imsi_10) = self.imsi_10 {
            bs.write_u8(1, 1);
            bs.write_u8(imsi_10, 4);
        } else {
            bs.write_u8(0, 1);
        }
        if self.cs_supported {
            bs.write_u8(self.max_add_serv_instance.expect("validated"), 3);
        }
        if let Some(tkz_id) = self.tkz_id {
            bs.write_u8(1, 1);
            bs.write_u8(tkz_id, 8);
        } else {
            bs.write_u8(0, 1);
        }
        if self.packet_zone_id != 0 {
            bs.write_u8(self.pz_hyst_enabled as u8, 1);
            if self.pz_hyst_enabled {
                if let Some(pz_hyst) = &self.pz_hyst_info {
                    bs.write_u8(1, 1);
                    bs.write_u8(pz_hyst.list_len, 4);
                    bs.write_u8(pz_hyst.act_timer, 8);
                    bs.write_u8(pz_hyst.timer_mul, 3);
                    bs.write_u8(pz_hyst.timer_exp, 5);
                } else {
                    bs.write_u8(0, 1);
                }
            }
        }
        bs.write_u8(self.ext_pref_msid_type, 2);
        if Self::meid_reqd_present(self.pref_msid_type, self.ext_pref_msid_type) {
            bs.write_u8(self.meid_reqd.expect("validated") as u8, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let sid = bs.read_bits(15)? as u16;
        let nid = bs.read_bits(16)? as u16;
        let packet_zone_id = bs.read_bits(8)? as u8;
        let reg_zone = bs.read_bits(12)? as u16;
        let total_zones = bs.read_bits(3)? as u8;
        let zone_timer = bs.read_bits(3)? as u8;
        let mult_sids = bs.read_bits(1)? != 0;
        let mult_nids = bs.read_bits(1)? != 0;
        let home_reg = bs.read_bits(1)? != 0;
        let for_sid_reg = bs.read_bits(1)? != 0;
        let for_nid_reg = bs.read_bits(1)? != 0;
        let power_up_reg = bs.read_bits(1)? != 0;
        let power_down_reg = bs.read_bits(1)? != 0;
        let parameter_reg = bs.read_bits(1)? != 0;
        let reg_prd = bs.read_bits(7)? as u8;
        if !Self::valid_reg_prd(reg_prd) {
            return Err("A41SPM REG_PRD must be 0 or in the range 29..=85".into());
        }
        let reg_dist = if bs.read_bits(1)? != 0 {
            let reg_dist = bs.read_bits(11)? as u16;
            if reg_dist == 0 {
                return Err("A41SPM REG_DIST must be non-zero when DIST_REG_INCL=1".into());
            }
            Some(reg_dist)
        } else {
            None
        };
        let delete_for_tmsi = bs.read_bits(1)? != 0;
        let use_tmsi = bs.read_bits(1)? != 0;
        let pref_msid_type = bs.read_bits(2)? as u8;
        let tmsi_zone_len = bs.read_bits(4)? as usize;
        if !(1..=8).contains(&tmsi_zone_len) {
            return Err("A41SPM TMSI_ZONE_LEN must be in the range 1..=8".into());
        }
        let mut tmsi_zone = Vec::with_capacity(tmsi_zone_len);
        for _ in 0..tmsi_zone_len {
            tmsi_zone.push(bs.read_bits(8)? as u8);
        }
        let imsi_t_supported = bs.read_bits(1)? != 0;
        let max_num_alt_so = bs.read_bits(3)? as u8;
        let auto_msg_interval = if bs.read_bits(1)? != 0 {
            Some(bs.read_bits(3)? as u8)
        } else {
            None
        };
        let other_info = if bs.read_bits(1)? != 0 {
            let base_id = bs.read_bits(16)? as u16;
            let mcc = bs.read_bits(10)? as u16;
            let imsi_11_12 = bs.read_bits(7)? as u8;
            let broadcast_gps_asst = bs.read_bits(1)? != 0;
            let sig_encrypt_sup = bs.read_bits(8)? as u8;
            if sig_encrypt_sup & 0b0001_1111 != 0 {
                return Err("A41SPM SIG_ENCRYPT_SUP reserved bits must be zero".into());
            }
            Some(Ansi41OtherInfo {
                base_id,
                mcc,
                imsi_11_12,
                broadcast_gps_asst,
                sig_encrypt_sup,
            })
        } else {
            None
        };
        let cs_supported = bs.read_bits(1)? != 0;
        let ms_init_pos_loc_sup_ind = bs.read_bits(1)? != 0;
        let msg_integrity_sup = bs.read_bits(1)? != 0;
        let sig_integrity_sup = if msg_integrity_sup && bs.read_bits(1)? != 0 {
            let sig_integrity_sup = bs.read_bits(8)? as u8;
            if sig_integrity_sup != 0 {
                return Err("A41SPM SIG_INTEGRITY_SUP reserved bits must be zero".into());
            }
            Some(sig_integrity_sup)
        } else {
            None
        };
        let imsi_10 = if bs.read_bits(1)? != 0 {
            Some(bs.read_bits(4)? as u8)
        } else {
            None
        };
        let max_add_serv_instance = if cs_supported {
            Some(bs.read_bits(3)? as u8)
        } else {
            None
        };
        let tkz_id = if bs.read_bits(1)? != 0 {
            Some(bs.read_bits(8)? as u8)
        } else {
            None
        };
        let (pz_hyst_enabled, pz_hyst_info) = if packet_zone_id != 0 {
            let pz_hyst_enabled = bs.read_bits(1)? != 0;
            let pz_hyst_info = if pz_hyst_enabled && bs.read_bits(1)? != 0 {
                let list_len = bs.read_bits(4)? as u8;
                let act_timer = bs.read_bits(8)? as u8;
                let timer_mul = bs.read_bits(3)? as u8;
                let timer_exp = bs.read_bits(5)? as u8;
                if list_len == 0 || act_timer == 0 || timer_mul == 0 || timer_exp > 4 {
                    return Err("A41SPM packet-zone hysteresis values out of spec".into());
                }
                Some(PacketZoneHysteresisInfo {
                    list_len,
                    act_timer,
                    timer_mul,
                    timer_exp,
                })
            } else {
                None
            };
            (pz_hyst_enabled, pz_hyst_info)
        } else {
            (false, None)
        };
        let ext_pref_msid_type = bs.read_bits(2)? as u8;
        if !Self::valid_msid_selector(use_tmsi, pref_msid_type, ext_pref_msid_type) {
            return Err(
                "A41SPM reserved USE_TMSI/PREF_MSID_TYPE/EXT_PREF_MSID_TYPE combination".into(),
            );
        }
        let meid_reqd = if Self::meid_reqd_present(pref_msid_type, ext_pref_msid_type) {
            Some(bs.read_bits(1)? != 0)
        } else {
            None
        };
        if !bs.is_empty() {
            return Err("A41SPM has trailing bits after MEID_REQD".into());
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            sid,
            nid,
            packet_zone_id,
            reg_zone,
            total_zones,
            zone_timer,
            mult_sids,
            mult_nids,
            home_reg,
            for_sid_reg,
            for_nid_reg,
            power_up_reg,
            power_down_reg,
            parameter_reg,
            reg_prd,
            reg_dist,
            delete_for_tmsi,
            use_tmsi,
            pref_msid_type,
            tmsi_zone,
            imsi_t_supported,
            max_num_alt_so,
            auto_msg_interval,
            other_info,
            cs_supported,
            ms_init_pos_loc_sup_ind,
            msg_integrity_sup,
            sig_integrity_sup,
            imsi_10,
            max_add_serv_instance,
            tkz_id,
            pz_hyst_enabled,
            pz_hyst_info,
            ext_pref_msid_type,
            meid_reqd,
        })
    }

    fn validate(&self) {
        assert!(
            (1..=8).contains(&self.tmsi_zone.len()),
            "A41SPM TMSI_ZONE_LEN must be in the range 1..=8"
        );
        assert!(
            Self::valid_reg_prd(self.reg_prd),
            "A41SPM REG_PRD must be 0 or in the range 29..=85"
        );
        assert!(
            Self::valid_msid_selector(self.use_tmsi, self.pref_msid_type, self.ext_pref_msid_type),
            "A41SPM reserved USE_TMSI/PREF_MSID_TYPE/EXT_PREF_MSID_TYPE combination"
        );
        if let Some(reg_dist) = self.reg_dist {
            assert!(reg_dist != 0, "A41SPM REG_DIST must be non-zero");
        }
        if let Some(other) = &self.other_info {
            assert!(
                other.sig_encrypt_sup & 0b0001_1111 == 0,
                "A41SPM SIG_ENCRYPT_SUP reserved bits must be zero"
            );
        }
        if !self.msg_integrity_sup {
            assert!(
                self.sig_integrity_sup.is_none(),
                "A41SPM SIG_INTEGRITY_SUP_INCL requires MSG_INTEGRITY_SUP=1"
            );
        }
        if let Some(sig_integrity_sup) = self.sig_integrity_sup {
            assert!(
                sig_integrity_sup == 0,
                "A41SPM SIG_INTEGRITY_SUP reserved bits must be zero"
            );
        }
        assert!(
            self.cs_supported == self.max_add_serv_instance.is_some(),
            "A41SPM MAX_ADD_SERV_INSTANCE presence must match CS_SUPPORTED"
        );
        if self.packet_zone_id == 0 {
            assert!(
                !self.pz_hyst_enabled && self.pz_hyst_info.is_none(),
                "A41SPM packet-zone hysteresis fields require non-zero PACKET_ZONE_ID"
            );
        } else if !self.pz_hyst_enabled {
            assert!(
                self.pz_hyst_info.is_none(),
                "A41SPM PZ_HYST_INFO_INCL requires PZ_HYST_ENABLED=1"
            );
        }
        if let Some(pz_hyst) = &self.pz_hyst_info {
            assert!(
                pz_hyst.list_len != 0
                    && pz_hyst.act_timer != 0
                    && pz_hyst.timer_mul != 0
                    && pz_hyst.timer_exp <= 4,
                "A41SPM packet-zone hysteresis values out of spec"
            );
        }
        assert!(
            self.meid_reqd.is_some()
                == Self::meid_reqd_present(self.pref_msid_type, self.ext_pref_msid_type),
            "A41SPM MEID_REQD presence must follow PREF_MSID_TYPE/EXT_PREF_MSID_TYPE"
        );
    }

    fn meid_reqd_present(pref_msid_type: u8, ext_pref_msid_type: u8) -> bool {
        !(ext_pref_msid_type == 0b11 && matches!(pref_msid_type, 0b00 | 0b11))
    }

    fn valid_reg_prd(reg_prd: u8) -> bool {
        reg_prd == 0 || (29..=85).contains(&reg_prd)
    }

    fn valid_msid_selector(use_tmsi: bool, pref_msid_type: u8, ext_pref_msid_type: u8) -> bool {
        ext_pref_msid_type != 0b10
            && pref_msid_type != 0b01
            && !(use_tmsi && pref_msid_type == 0b00)
    }
}

impl McRrParametersMessage {
    /// Encode MCRRPM per C.S0005-E 3.7.2.3.2.31.
    pub fn to_sdu(&self) -> Bitstream {
        self.validate();

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u32(self.base_id as u32, 16);
        bs.write_u8(self.p_rev, 8);
        bs.write_u8(self.min_p_rev, 8);
        if let Some(sr3) = &self.sr3 {
            bs.write_u8(1, 1);
            if let Some(center_freq) = sr3.sr3_center_freq {
                bs.write_u8(1, 1);
                bs.write_u32(center_freq as u32, 11);
            } else {
                bs.write_u8(0, 1);
            }
            bs.write_u8(sr3.sr3_brat, 2);
            bs.write_u8(sr3.sr3_bcch_code_chan, 7);
            bs.write_u8(sr3.sr3_primary_pilot, 2);
            bs.write_u8(sr3.sr3_pilot_power1, 3);
            bs.write_u8(sr3.sr3_pilot_power2, 3);
        } else {
            bs.write_u8(0, 1);
        }
        bs.write_u8(self.srch_win_a, 4);
        bs.write_u8(self.srch_win_r, 4);
        bs.write_u8(self.t_add, 6);
        bs.write_u8(self.t_drop, 6);
        bs.write_u8(self.t_comp, 4);
        bs.write_u8(self.t_tdrop, 4);
        bs.write_u8(self.nghbr_max_age, 4);
        bs.write_u8(self.soft_slope, 6);
        bs.write_u8(self.add_intercept, 6);
        bs.write_u8(self.drop_intercept, 6);
        match (self.sig_encrypt_sup, self.ui_encrypt_sup) {
            (Some(sig), Some(ui)) => {
                bs.write_u8(1, 1);
                bs.write_u8(sig, 8);
                bs.write_u8(ui, 8);
            }
            (None, None) => bs.write_u8(0, 1),
            _ => unreachable!("validated"),
        }
        bs.write_u8(self.add_fields.len() as u8, 8);
        for byte in &self.add_fields {
            bs.write_u8(*byte, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let base_id = bs.read_bits(16)? as u16;
        let p_rev = bs.read_bits(8)? as u8;
        let min_p_rev = bs.read_bits(8)? as u8;
        let sr3 = if bs.read_bits(1)? != 0 {
            let sr3_center_freq = if bs.read_bits(1)? != 0 {
                Some(bs.read_bits(11)? as u16)
            } else {
                None
            };
            let sr3_brat = bs.read_bits(2)? as u8;
            if sr3_brat == 0b11 {
                return Err("MCRRPM SR3_BRAT 0b11 is reserved".into());
            }
            let sr3_bcch_code_chan = bs.read_bits(7)? as u8;
            let sr3_primary_pilot = bs.read_bits(2)? as u8;
            if sr3_primary_pilot == 0b11 {
                return Err("MCRRPM SR3_PRIMARY_PILOT 0b11 is reserved".into());
            }
            Some(McRrSr3Parameters {
                sr3_center_freq,
                sr3_brat,
                sr3_bcch_code_chan,
                sr3_primary_pilot,
                sr3_pilot_power1: bs.read_bits(3)? as u8,
                sr3_pilot_power2: bs.read_bits(3)? as u8,
            })
        } else {
            None
        };
        let srch_win_a = bs.read_bits(4)? as u8;
        let srch_win_r = bs.read_bits(4)? as u8;
        let t_add = bs.read_bits(6)? as u8;
        let t_drop = bs.read_bits(6)? as u8;
        let t_comp = bs.read_bits(4)? as u8;
        let t_tdrop = bs.read_bits(4)? as u8;
        let nghbr_max_age = bs.read_bits(4)? as u8;
        let soft_slope = bs.read_bits(6)? as u8;
        let add_intercept = bs.read_bits(6)? as u8;
        let drop_intercept = bs.read_bits(6)? as u8;
        let (sig_encrypt_sup, ui_encrypt_sup) = if bs.read_bits(1)? != 0 {
            let sig = bs.read_bits(8)? as u8;
            if sig & 0b0001_1111 != 0 {
                return Err("MCRRPM SIG_ENCRYPT_SUP reserved bits must be zero".into());
            }
            let ui = bs.read_bits(8)? as u8;
            if ui & 0b0011_1111 != 0 {
                return Err("MCRRPM UI_ENCRYPT_SUP reserved bits must be zero".into());
            }
            (Some(sig), Some(ui))
        } else {
            (None, None)
        };
        let add_fields_len = bs.read_bits(8)? as usize;
        if bs.len() < add_fields_len * 8 {
            return Err("MCRRPM ADD_FIELDS length exceeds remaining SDU".into());
        }
        let mut add_fields = Vec::with_capacity(add_fields_len);
        for _ in 0..add_fields_len {
            add_fields.push(bs.read_bits(8)? as u8);
        }
        if !bs.is_empty() {
            return Err("MCRRPM has trailing bits after ADD_FIELDS".into());
        }

        Ok(Self {
            pilot_pn,
            config_msg_seq,
            base_id,
            p_rev,
            min_p_rev,
            sr3,
            srch_win_a,
            srch_win_r,
            t_add,
            t_drop,
            t_comp,
            t_tdrop,
            nghbr_max_age,
            soft_slope,
            add_intercept,
            drop_intercept,
            sig_encrypt_sup,
            ui_encrypt_sup,
            add_fields,
        })
    }

    fn validate(&self) {
        if let Some(sr3) = &self.sr3 {
            assert!(sr3.sr3_brat <= 0b10, "MCRRPM SR3_BRAT 0b11 is reserved");
            assert!(
                sr3.sr3_primary_pilot <= 0b10,
                "MCRRPM SR3_PRIMARY_PILOT 0b11 is reserved"
            );
        }
        assert!(
            self.sig_encrypt_sup.is_some() == self.ui_encrypt_sup.is_some(),
            "MCRRPM ENC_SUPPORTED requires both SIG_ENCRYPT_SUP and UI_ENCRYPT_SUP"
        );
        if let Some(sig) = self.sig_encrypt_sup {
            assert!(
                sig & 0b0001_1111 == 0,
                "MCRRPM SIG_ENCRYPT_SUP reserved bits must be zero"
            );
        }
        if let Some(ui) = self.ui_encrypt_sup {
            assert!(
                ui & 0b0011_1111 == 0,
                "MCRRPM UI_ENCRYPT_SUP reserved bits must be zero"
            );
        }
        assert!(
            self.add_fields.len() <= u8::MAX as usize,
            "MCRRPM ADD_FIELDS_LEN must fit in one octet"
        );
    }
}

impl Ansi41RandMessage {
    /// Encode A41RANDM per C.S0005-E 3.7.2.3.2.32.
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.acc_msg_seq, 6);
        bs.write_u32(self.rand, 32);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let acc_msg_seq = bs.read_bits(6)? as u8;
        let rand = bs.read_bits(32)? as u32;
        if !bs.is_empty() {
            return Err("A41RANDM has trailing bits after RAND".into());
        }
        Ok(Self {
            pilot_pn,
            acc_msg_seq,
            rand,
        })
    }
}

impl EnhancedAccessParametersMessage {
    /// Encode EAPM per C.S0005-E 3.7.2.3.2.33.
    ///
    /// The nested EAPM parameter records are preserved bit-exactly after the
    /// fixed `PILOT_PN` and `ACC_MSG_SEQ` header.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.body_bits.len() >= 5,
            "EAPM body must include at least PSIST_PARMS_INCL and LAC_PARMS_LEN"
        );
        assert!(
            self.body_bits.iter().all(|bit| *bit <= 1),
            "EAPM body_bits must contain only 0/1 values"
        );
        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.acc_msg_seq, 6);
        for bit in &self.body_bits {
            bs.write_u8(*bit, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let acc_msg_seq = bs.read_bits(6)? as u8;
        if bs.len() < 5 {
            return Err(
                "EAPM body must include at least PSIST_PARMS_INCL and LAC_PARMS_LEN".into(),
            );
        }
        let body = bs.drain(0..bs.len());
        Self::decode_body(body.bits())?;
        Ok(Self {
            pilot_pn,
            acc_msg_seq,
            body_bits: body.bits().to_vec(),
        })
    }

    pub fn body(&self) -> Result<EnhancedAccessParametersBody, crate::error::Error> {
        Self::decode_body(&self.body_bits)
    }

    fn decode_body(bits: &[u8]) -> Result<EnhancedAccessParametersBody, crate::error::Error> {
        if bits.len() < 5 {
            return Err(
                "EAPM body must include at least PSIST_PARMS_INCL and LAC_PARMS_LEN".into(),
            );
        }
        if bits.iter().any(|bit| *bit > 1) {
            return Err("EAPM body_bits must contain only 0/1 values".into());
        }

        let mut bs = Bitstream::new_init(bits);
        let psist = if bs.read_bits(1)? != 0 {
            Some(Self::read_psist_parameters(&mut bs)?)
        } else {
            None
        };
        let lac = Self::read_lac_parameters(&mut bs)?;

        let num_mode_selection_entries = bs.read_bits(3)? as usize;
        let mut mode_selection_entries = Vec::with_capacity(num_mode_selection_entries + 1);
        for _ in 0..=num_mode_selection_entries {
            let access_mode = bs.read_bits(3)? as u8;
            if !matches!(access_mode, 0b000 | 0b001) {
                return Err(format!("EAPM reserved ACCESS_MODE 0b{access_mode:03b}").into());
            }
            mode_selection_entries.push(EnhancedAccessModeSelectionEntry {
                access_mode,
                min_duration: bs.read_bits(10)? as u16,
                max_duration: bs.read_bits(10)? as u16,
            });
        }

        let rlgain_common_pilot = bs.read_bits(6)? as u8;
        let ic_thresh = bs.read_bits(4)? as u8;
        let ic_max = bs.read_bits(4)? as u8;

        let num_mode_parm_rec = bs.read_bits(3)? as usize;
        let mut mode_parameter_records = Vec::with_capacity(num_mode_parm_rec + 1);
        for _ in 0..=num_mode_parm_rec {
            mode_parameter_records.push(Self::read_mode_parameter_record(&mut bs)?);
        }

        let basic_access = Self::read_basic_access_parameters(&mut bs)?;
        let reservation_access = Self::read_reservation_access_parameters(&mut bs)?;
        let acct = Self::read_acct_parameters(&mut bs)?;

        if !bs.is_empty() {
            return Err("EAPM has trailing bits after ACCT parameters".into());
        }

        Ok(EnhancedAccessParametersBody {
            psist,
            lac,
            mode_selection_entries,
            rlgain_common_pilot,
            ic_thresh,
            ic_max,
            mode_parameter_records,
            basic_access,
            reservation_access,
            acct,
        })
    }

    fn read_len_record(
        bs: &mut Bitstream,
        len_bits: usize,
        include_len_bits: bool,
        len_field: &str,
    ) -> Result<Bitstream, crate::error::Error> {
        let record_len = bs.read_bits(len_bits)? as usize;
        if record_len == 0 {
            return Ok(Bitstream::new());
        }
        let record_bits = record_len * 8;
        let body_bits = if include_len_bits {
            record_bits.checked_sub(len_bits)
        } else {
            Some(record_bits)
        }
        .ok_or_else(|| format!("EAPM {len_field} length is too short"))?;
        if bs.len() < body_bits {
            return Err(format!("EAPM {len_field} exceeds remaining body").into());
        }
        Ok(bs.drain(0..body_bits))
    }

    fn read_psist_parameters(
        bs: &mut Bitstream,
    ) -> Result<EnhancedAccessPsistParameters, crate::error::Error> {
        let mut record = Self::read_len_record(bs, 5, true, "PSIST_PARMS_LEN")?;
        if record.is_empty() {
            return Err("EAPM PSIST_PARMS_LEN must be non-zero when included".into());
        }
        if record.len() < 33 {
            return Err("EAPM PSIST parameters record is truncated".into());
        }
        let parsed = EnhancedAccessPsistParameters {
            psist_0_9_each: record.read_bits(6)? as u8,
            psist_10_each: record.read_bits(3)? as u8,
            psist_11_each: record.read_bits(3)? as u8,
            psist_12_each: record.read_bits(3)? as u8,
            psist_13_each: record.read_bits(3)? as u8,
            psist_14_each: record.read_bits(3)? as u8,
            psist_15_each: record.read_bits(3)? as u8,
            psist_emg: record.read_bits(3)? as u8,
            msg_psist_each: record.read_bits(3)? as u8,
            reg_psist_each: record.read_bits(3)? as u8,
        };
        Self::read_zero_tail(&mut record, "PSIST parameters")?;
        Ok(parsed)
    }

    fn read_lac_parameters(
        bs: &mut Bitstream,
    ) -> Result<EnhancedAccessLacParameters, crate::error::Error> {
        let mut record = Self::read_len_record(bs, 4, true, "LAC_PARMS_LEN")?;
        if record.len() < 18 {
            return Err("EAPM LAC parameters record is truncated".into());
        }
        let acc_tmo = record.read_bits(6)? as u8;
        let reserved_1 = record.read_bits(4)? as u8;
        if reserved_1 != 0 {
            return Err("EAPM LAC RESERVED_1 must be zero".into());
        }
        let max_req_seq = record.read_bits(4)? as u8;
        let max_rsp_seq = record.read_bits(4)? as u8;
        if max_req_seq == 0 {
            return Err("EAPM MAX_REQ_SEQ must be greater than zero".into());
        }
        if max_rsp_seq == 0 {
            return Err("EAPM MAX_RSP_SEQ must be greater than zero".into());
        }
        Self::read_zero_tail(&mut record, "LAC parameters")?;
        Ok(EnhancedAccessLacParameters {
            acc_tmo,
            max_req_seq,
            max_rsp_seq,
        })
    }

    fn read_mode_parameter_record(
        bs: &mut Bitstream,
    ) -> Result<EnhancedAccessModeParameterRecord, crate::error::Error> {
        let mut record = Self::read_len_record(bs, 4, true, "EACH_PARM_REC_LEN")?;
        if record.len() < 58 {
            return Err("EAPM mode-specific parameter record is truncated".into());
        }
        let applicable_modes = record.read_bits(8)? as u8;
        if applicable_modes & 0x3f != 0 {
            return Err("EAPM APPLICABLE_MODES reserved bits must be zero".into());
        }
        let each_nom_pwr = record.read_bits(5)? as u8;
        let each_init_pwr = record.read_bits(5)? as u8;
        let each_pwr_step = record.read_bits(3)? as u8;
        let each_num_step = record.read_bits(4)? as u8;
        let preamble = if record.read_bits(1)? != 0 {
            Some(Self::read_preamble_parameters(&mut record, "EACH")?)
        } else {
            None
        };
        let reserved = record.read_bits(6)? as u8;
        if reserved != 0 {
            return Err("EAPM EACH mode parameter RESERVED must be zero".into());
        }
        let each_probe_bkoff = record.read_bits(4)? as u8;
        let each_bkoff = record.read_bits(4)? as u8;
        let each_slot = record.read_bits(6)? as u8;
        let each_slot_offset1 = record.read_bits(6)? as u8;
        let each_slot_offset2 = record.read_bits(6)? as u8;
        Self::read_zero_tail(&mut record, "mode-specific parameter record")?;
        Ok(EnhancedAccessModeParameterRecord {
            applicable_modes,
            each_nom_pwr,
            each_init_pwr,
            each_pwr_step,
            each_num_step,
            preamble,
            each_probe_bkoff,
            each_bkoff,
            each_slot,
            each_slot_offset1,
            each_slot_offset2,
        })
    }

    fn read_preamble_parameters(
        bs: &mut Bitstream,
        context: &str,
    ) -> Result<EnhancedAccessPreambleParameters, crate::error::Error> {
        if bs.len() < 16 {
            return Err(format!("EAPM {context} preamble parameters are truncated").into());
        }
        Ok(EnhancedAccessPreambleParameters {
            num_frac: bs.read_bits(4)? as u8,
            frac_duration: bs.read_bits(4)? as u8,
            off_duration: bs.read_bits(4)? as u8,
            add_duration: bs.read_bits(4)? as u8,
        })
    }

    fn read_basic_access_parameters(
        bs: &mut Bitstream,
    ) -> Result<Option<EnhancedAccessBasicAccessParameters>, crate::error::Error> {
        let mut record = Self::read_len_record(bs, 3, false, "BA_PARMS_LEN")?;
        if record.is_empty() {
            return Ok(None);
        }
        if record.len() < 13 {
            return Err("EAPM BA parameters record is truncated".into());
        }
        let num_each_ba = record.read_bits(5)? as u8;
        let each_ba_rates_supported = record.read_bits(8)? as u8;
        if each_ba_rates_supported & 0b0000_0011 != 0 {
            return Err("EAPM EACH_BA_RATES_SUPPORTED reserved bits must be zero".into());
        }
        Self::read_zero_tail(&mut record, "BA parameters")?;
        Ok(Some(EnhancedAccessBasicAccessParameters {
            num_each_ba,
            each_ba_rates_supported,
        }))
    }

    fn read_reservation_access_parameters(
        bs: &mut Bitstream,
    ) -> Result<Option<EnhancedAccessReservationAccessParameters>, crate::error::Error> {
        let mut record = Self::read_len_record(bs, 5, false, "RA_PARMS_LEN")?;
        if record.is_empty() {
            return Ok(None);
        }
        if record.len() < 80 {
            return Err("EAPM RA parameters record is truncated".into());
        }
        let num_each_ra = record.read_bits(5)? as u8;
        let num_cach = record.read_bits(3)? as u8;
        let cach_code_rate = record.read_bits(1)? != 0;
        let mut cach_code_chans = Vec::with_capacity(num_cach as usize + 1);
        for _ in 0..=num_cach {
            let chan = record.read_bits(8)? as u8;
            if chan == 0 {
                return Err("EAPM CACH_CODE_CHAN must be in 1..=255".into());
            }
            cach_code_chans.push(chan);
        }
        let num_rccch = record.read_bits(5)? as u8;
        let rccch_rates_supported = record.read_bits(8)? as u8;
        if rccch_rates_supported & 0b0000_0011 != 0 {
            return Err("EAPM RCCCH_RATES_SUPPORTED reserved bits must be zero".into());
        }
        let rccch_preamble = if record.read_bits(1)? != 0 {
            Some(Self::read_preamble_parameters(&mut record, "RCCCH")?)
        } else {
            None
        };
        let rccch_slot = record.read_bits(6)? as u8;
        let rccch_slot_offset1 = record.read_bits(6)? as u8;
        let rccch_slot_offset2 = record.read_bits(6)? as u8;
        let rccch_nom_pwr = record.read_bits(5)? as u8;
        let rccch_init_pwr = record.read_bits(5)? as u8;
        let ra_pc_delay = record.read_bits(5)? as u8;
        let eacam_cach_delay = record.read_bits(4)? as u8;
        let rccch_ho_thresh = if record.read_bits(1)? != 0 {
            Some(record.read_bits(4)? as u8)
        } else {
            None
        };
        let eacam_pccam_delay = record.read_bits(5)? as u8;
        let num_cpcch = record.read_bits(2)? as u8;
        let cpcch_rate = record.read_bits(2)? as u8;
        let mut cpcch_code_chans = Vec::with_capacity(num_cpcch as usize + 1);
        for _ in 0..=num_cpcch {
            let chan = record.read_bits(8)? as u8;
            if chan == 0 {
                return Err("EAPM CPCCH_CODE_CHAN must be in 1..=255".into());
            }
            cpcch_code_chans.push(chan);
        }
        let num_pcsch_ra = record.read_bits(7)? as u8;
        Self::read_zero_tail(&mut record, "RA parameters")?;
        Ok(Some(EnhancedAccessReservationAccessParameters {
            num_each_ra,
            num_cach,
            cach_code_rate,
            cach_code_chans,
            num_rccch,
            rccch_rates_supported,
            rccch_preamble,
            rccch_slot,
            rccch_slot_offset1,
            rccch_slot_offset2,
            rccch_nom_pwr,
            rccch_init_pwr,
            ra_pc_delay,
            eacam_cach_delay,
            rccch_ho_thresh,
            eacam_pccam_delay,
            num_cpcch,
            cpcch_rate,
            cpcch_code_chans,
            num_pcsch_ra,
        }))
    }

    fn read_acct_parameters(
        bs: &mut Bitstream,
    ) -> Result<Option<EnhancedAccessAcctParameters>, crate::error::Error> {
        let acct_incl = bs.read_bits(1)? != 0;
        if !acct_incl {
            return Ok(None);
        }
        let acct_incl_emg = bs.read_bits(1)? != 0;
        let acct_aoc_bitmap_incl = bs.read_bits(1)? != 0;
        let acct_so_incl = bs.read_bits(1)? != 0;
        let mut acct_so_records = Vec::new();
        if acct_so_incl {
            let num_acct_so = bs.read_bits(4)? as usize;
            acct_so_records.reserve(num_acct_so + 1);
            for _ in 0..=num_acct_so {
                let acct_aoc_bitmap = if acct_aoc_bitmap_incl {
                    Some(bs.read_bits(5)? as u8)
                } else {
                    None
                };
                let acct_so = bs.read_bits(16)? as u16;
                acct_so_records.push(EnhancedAccessAcctServiceOptionRecord {
                    acct_aoc_bitmap,
                    acct_so,
                });
            }
        }
        let acct_so_grp_incl = bs.read_bits(1)? != 0;
        let mut acct_so_group_records = Vec::new();
        if acct_so_grp_incl {
            let num_acct_so_grp = bs.read_bits(3)? as usize;
            acct_so_group_records.reserve(num_acct_so_grp + 1);
            for _ in 0..=num_acct_so_grp {
                let acct_aoc_bitmap = if acct_aoc_bitmap_incl {
                    Some(bs.read_bits(5)? as u8)
                } else {
                    None
                };
                let acct_so_group = bs.read_bits(5)? as u8;
                acct_so_group_records.push(EnhancedAccessAcctServiceOptionGroupRecord {
                    acct_aoc_bitmap,
                    acct_so_group,
                });
            }
        }
        if acct_so_records.is_empty() && acct_so_group_records.is_empty() {
            return Err("EAPM ACCT_INCL requires ACCT_SO_INCL or ACCT_SO_GRP_INCL".into());
        }
        Ok(Some(EnhancedAccessAcctParameters {
            acct_incl_emg,
            acct_aoc_bitmap_incl,
            acct_so_records,
            acct_so_group_records,
        }))
    }

    fn read_zero_tail(bs: &mut Bitstream, context: &str) -> Result<(), crate::error::Error> {
        while !bs.is_empty() {
            if bs.read_bits(1)? != 0 {
                return Err(format!("EAPM {context} reserved padding bits must be zero").into());
            }
        }
        Ok(())
    }
}

impl UniversalRadioInterfaceRecord {
    pub fn mc(fields: &UniversalMcRadioInterface) -> Self {
        Self::Mc {
            fields: fields.to_fields(),
        }
    }

    pub fn mc_fields(&self) -> Result<Option<UniversalMcRadioInterface>, crate::error::Error> {
        match self {
            Self::Mc { fields } => Ok(Some(UniversalMcRadioInterface::from_fields(fields)?)),
            Self::Hrpd { .. } => Ok(None),
        }
    }
}

impl UniversalMcRadioInterface {
    pub fn to_fields(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        UniversalNeighborListMessage::write_mc_radio_interface(self, &mut bs);
        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn from_fields(fields: &[u8]) -> Result<Self, crate::error::Error> {
        let mut bs = Bitstream::new_bytes(fields);
        UniversalNeighborListMessage::read_mc_radio_interface(&mut bs)
    }
}

impl UniversalNeighborListMessage {
    /// Encode UNLM per C.S0005-E 3.7.2.3.2.34.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.radio_interfaces.len() <= 15,
            "UNLM NUM_RADIO_INTERFACE must fit in 4 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.radio_interfaces.len() as u8, 4);
        for radio_interface in &self.radio_interfaces {
            let (radio_interface_type, body) = match radio_interface {
                UniversalRadioInterfaceRecord::Mc { fields } => {
                    assert!(
                        !fields.is_empty(),
                        "UNLM MC RADIO_INTERFACE_LEN must be non-zero"
                    );
                    UniversalMcRadioInterface::from_fields(fields)
                        .expect("UNLM MC radio-interface fields must be spec-valid");
                    (0, Bitstream::new_bytes(fields))
                }
                UniversalRadioInterfaceRecord::Hrpd { neighbors } => {
                    let mut body = Bitstream::new();
                    Self::write_hrpd_radio_interface(neighbors, &mut body);
                    pad_to_octet(&mut body);
                    (2, body)
                }
            };
            let body_len = body.len() / 8;
            assert!(
                body_len <= u8::MAX as usize,
                "UNLM RADIO_INTERFACE_LEN must fit in one octet"
            );
            bs.write_u8(radio_interface_type, 4);
            bs.write_u8(body_len as u8, 8);
            bs.extend(&body);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let num_radio_interface = bs.read_bits(4)? as usize;
        let mut radio_interfaces = Vec::with_capacity(num_radio_interface);
        for _ in 0..num_radio_interface {
            let radio_interface_type = bs.read_bits(4)? as u8;
            let radio_interface_len = bs.read_bits(8)? as usize;
            if !matches!(radio_interface_type, 0 | 2) {
                return Err(format!(
                    "UNLM reserved RADIO_INTERFACE_TYPE 0b{radio_interface_type:04b}"
                )
                .into());
            }
            if radio_interface_len == 0 {
                return Err("UNLM RADIO_INTERFACE_LEN must be non-zero".into());
            }
            let body_bits = radio_interface_len * 8;
            if bs.len() < body_bits {
                return Err("UNLM radio-interface body length exceeds remaining SDU".into());
            }
            let mut body = bs.drain(0..body_bits);
            let radio_interface = match radio_interface_type {
                0 => {
                    let fields = body.to_packed_bytes();
                    Self::read_mc_radio_interface(&mut body)?;
                    UniversalRadioInterfaceRecord::Mc { fields }
                }
                2 => UniversalRadioInterfaceRecord::Hrpd {
                    neighbors: Self::read_hrpd_radio_interface(&mut body)?,
                },
                _ => unreachable!("validated radio-interface type"),
            };
            radio_interfaces.push(radio_interface);
        }
        if !bs.is_empty() {
            return Err("UNLM has trailing bits after radio-interface records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            radio_interfaces,
        })
    }

    fn write_mc_radio_interface(record: &UniversalMcRadioInterface, bs: &mut Bitstream) {
        Self::validate_mc_radio_interface(record);

        bs.write_u8(record.pilot_inc, 4);
        bs.write_u8(record.nghbr_srch_mode, 2);
        if Self::unlm_has_common_search_window(record.nghbr_srch_mode) {
            bs.write_u8(record.srch_win_n.expect("validated"), 4);
        }
        bs.write_u8(record.srch_offset_incl as u8, 1);
        bs.write_u8(record.freq_fields_incl as u8, 1);
        bs.write_u8(record.use_timing as u8, 1);
        if record.use_timing {
            bs.write_u8(record.global_timing.is_some() as u8, 1);
            if let Some(global_timing) = &record.global_timing {
                bs.write_u8(global_timing.tx_duration, 4);
                bs.write_u8(global_timing.tx_period, 7);
            }
        }
        bs.write_u8(record.nghbr_set_entry_info as u8, 1);
        bs.write_u8(record.nghbr_set_access_info as u8, 1);
        bs.write_u8(record.neighbors.len() as u8, 6);

        for neighbor in &record.neighbors {
            bs.write_u8(neighbor.nghbr_config, 3);
            bs.write_u32(neighbor.nghbr_pn as u32, 9);
            if neighbor.nghbr_config == 0b011 {
                bs.write_u8(neighbor.bcch_support.expect("validated") as u8, 1);
            }
            if let Some(pilot_record) = &neighbor.pilot_record {
                bs.write_u8(1, 1);
                bs.write_u8(pilot_record.record_type(), 3);
                let mut pilot_bs = Bitstream::new();
                GeneralNeighborListMessage::write_pilot_record(pilot_record, &mut pilot_bs);
                pad_to_octet(&mut pilot_bs);
                let record_len = pilot_bs.len() / 8;
                assert!(
                    record_len <= 7,
                    "UNLM MC pilot RECORD_LEN must fit in 3 bits"
                );
                bs.write_u8(record_len as u8, 3);
                bs.extend(&pilot_bs);
            } else {
                bs.write_u8(0, 1);
            }
            if GeneralNeighborListMessage::has_search_priority(record.nghbr_srch_mode) {
                bs.write_u8(neighbor.search_priority.expect("validated"), 2);
            }
            if GeneralNeighborListMessage::has_search_window(record.nghbr_srch_mode) {
                bs.write_u8(neighbor.srch_win_nghbr.expect("validated"), 4);
            }
            if record.srch_offset_incl {
                bs.write_u8(neighbor.srch_offset_nghbr.expect("validated"), 3);
            }
            if record.freq_fields_incl {
                match (neighbor.nghbr_band, neighbor.nghbr_freq) {
                    (Some(band), Some(freq)) => {
                        bs.write_u8(1, 1);
                        bs.write_u8(band, 5);
                        bs.write_u32(freq as u32, 11);
                    }
                    (None, None) => bs.write_u8(0, 1),
                    _ => unreachable!("validated"),
                }
            }
            if record.use_timing {
                if let Some(timing) = &neighbor.timing {
                    bs.write_u8(1, 1);
                    bs.write_u8(timing.tx_offset, 7);
                    if record.global_timing.is_none() {
                        bs.write_u8(timing.tx_duration.expect("validated"), 4);
                        bs.write_u8(timing.tx_period.expect("validated"), 7);
                    }
                } else {
                    bs.write_u8(0, 1);
                }
            }
            if record.nghbr_set_entry_info {
                bs.write_u8(neighbor.access_entry_ho.expect("validated") as u8, 1);
            }
            if record.nghbr_set_access_info {
                bs.write_u8(neighbor.access_ho_allowed.expect("validated") as u8, 1);
            }
        }

        if let Some(resq) = &record.resq {
            bs.write_u8(1, 1);
            bs.write_u8(resq.delay_time, 6);
            bs.write_u8(resq.allowed_time, 6);
            bs.write_u8(resq.attempt_time, 6);
            bs.write_u32(resq.code_chan as u32, 11);
            bs.write_u8(resq.qof, 2);
            if let Some(min_period) = resq.min_period {
                bs.write_u8(1, 1);
                bs.write_u8(min_period, 5);
            } else {
                bs.write_u8(0, 1);
            }
            match (resq.num_tot_trans_20ms, resq.num_tot_trans_5ms) {
                (Some(trans_20ms), Some(trans_5ms)) => {
                    bs.write_u8(1, 1);
                    bs.write_u8(trans_20ms, 4);
                    bs.write_u8(trans_5ms, 4);
                }
                (None, None) => bs.write_u8(0, 1),
                _ => unreachable!("validated"),
            }
            bs.write_u8(resq.num_preamble_rc1_rc2, 3);
            bs.write_u8(resq.num_preamble, 3);
            bs.write_u8(resq.power_delta, 3);
            GeneralNeighborListMessage::write_bool_slice(bs, &resq.neighbor_configured);
        } else {
            bs.write_u8(0, 1);
        }
        GeneralNeighborListMessage::write_bool_slice(bs, &record.pdch_supported);
    }

    fn read_mc_radio_interface(
        bs: &mut Bitstream,
    ) -> Result<UniversalMcRadioInterface, crate::error::Error> {
        let pilot_inc = bs.read_bits(4)? as u8;
        if pilot_inc == 0 {
            return Err("UNLM MC PILOT_INC must be 1..=15".into());
        }
        let nghbr_srch_mode = bs.read_bits(2)? as u8;
        let srch_win_n = if Self::unlm_has_common_search_window(nghbr_srch_mode) {
            Some(bs.read_bits(4)? as u8)
        } else {
            None
        };
        let srch_offset_incl = bs.read_bits(1)? != 0;
        if srch_offset_incl && !GeneralNeighborListMessage::has_search_window(nghbr_srch_mode) {
            return Err("UNLM MC SRCH_OFFSET_INCL requires search-window mode".into());
        }
        let freq_fields_incl = bs.read_bits(1)? != 0;
        let use_timing = bs.read_bits(1)? != 0;
        let global_timing = if use_timing && bs.read_bits(1)? != 0 {
            Some(UniversalMcGlobalTiming {
                tx_duration: bs.read_bits(4)? as u8,
                tx_period: bs.read_bits(7)? as u8,
            })
        } else {
            None
        };
        let nghbr_set_entry_info = bs.read_bits(1)? != 0;
        let nghbr_set_access_info = bs.read_bits(1)? != 0;
        let num_nghbr = bs.read_bits(6)? as usize;
        let mut neighbors = Vec::with_capacity(num_nghbr);
        for _ in 0..num_nghbr {
            let nghbr_config = bs.read_bits(3)? as u8;
            if nghbr_config > 0b100 {
                return Err(format!("UNLM MC reserved NGHBR_CONFIG 0b{nghbr_config:03b}").into());
            }
            let nghbr_pn = bs.read_bits(9)? as u16;
            let bcch_support = if nghbr_config == 0b011 {
                Some(bs.read_bits(1)? != 0)
            } else {
                None
            };
            let pilot_record = if bs.read_bits(1)? != 0 {
                let record_type = bs.read_bits(3)? as u8;
                let record_len = bs.read_bits(3)? as usize;
                if record_len == 0 {
                    return Err(
                        "UNLM MC pilot RECORD_LEN must be non-zero when ADD_PILOT_REC_INCL is set"
                            .into(),
                    );
                }
                let record_bits = record_len * 8;
                if bs.len() < record_bits {
                    return Err("UNLM MC pilot record length exceeds radio-interface body".into());
                }
                let mut pilot_bs = bs.drain(0..record_bits);
                Some(GeneralNeighborListMessage::read_pilot_record(
                    record_type,
                    &mut pilot_bs,
                )?)
            } else {
                None
            };
            let search_priority =
                if GeneralNeighborListMessage::has_search_priority(nghbr_srch_mode) {
                    Some(bs.read_bits(2)? as u8)
                } else {
                    None
                };
            let srch_win_nghbr = if GeneralNeighborListMessage::has_search_window(nghbr_srch_mode) {
                Some(bs.read_bits(4)? as u8)
            } else {
                None
            };
            let srch_offset_nghbr = if srch_offset_incl {
                Some(bs.read_bits(3)? as u8)
            } else {
                None
            };
            let (nghbr_band, nghbr_freq) = if freq_fields_incl && bs.read_bits(1)? != 0 {
                (Some(bs.read_bits(5)? as u8), Some(bs.read_bits(11)? as u16))
            } else {
                (None, None)
            };
            let timing = if use_timing && bs.read_bits(1)? != 0 {
                let tx_offset = bs.read_bits(7)? as u8;
                let (tx_duration, tx_period) = if global_timing.is_none() {
                    (Some(bs.read_bits(4)? as u8), Some(bs.read_bits(7)? as u8))
                } else {
                    (None, None)
                };
                Some(UniversalMcNeighborTiming {
                    tx_offset,
                    tx_duration,
                    tx_period,
                })
            } else {
                None
            };
            let access_entry_ho = if nghbr_set_entry_info {
                Some(bs.read_bits(1)? != 0)
            } else {
                None
            };
            let access_ho_allowed = if nghbr_set_access_info {
                Some(bs.read_bits(1)? != 0)
            } else {
                None
            };
            neighbors.push(UniversalMcNeighborRecord {
                nghbr_config,
                nghbr_pn,
                bcch_support,
                pilot_record,
                search_priority,
                srch_win_nghbr,
                srch_offset_nghbr,
                nghbr_band,
                nghbr_freq,
                timing,
                access_entry_ho,
                access_ho_allowed,
            });
        }

        let resq = if bs.read_bits(1)? != 0 {
            let delay_time = bs.read_bits(6)? as u8;
            let allowed_time = bs.read_bits(6)? as u8;
            let attempt_time = bs.read_bits(6)? as u8;
            let code_chan = bs.read_bits(11)? as u16;
            if code_chan == 0 {
                return Err("UNLM MC RESQ_CODE_CHAN must be non-zero".into());
            }
            let qof = bs.read_bits(2)? as u8;
            let min_period = if bs.read_bits(1)? != 0 {
                Some(bs.read_bits(5)? as u8)
            } else {
                None
            };
            let (num_tot_trans_20ms, num_tot_trans_5ms) = if bs.read_bits(1)? != 0 {
                (Some(bs.read_bits(4)? as u8), Some(bs.read_bits(4)? as u8))
            } else {
                (None, None)
            };
            let num_preamble_rc1_rc2 = bs.read_bits(3)? as u8;
            let num_preamble = bs.read_bits(3)? as u8;
            let power_delta = bs.read_bits(3)? as u8;
            let neighbor_configured = GeneralNeighborListMessage::read_bool_vec(bs, num_nghbr)?;
            if !neighbor_configured.iter().any(|configured| *configured) {
                return Err(
                    "UNLM MC RESQ_ENABLED requires at least one configured neighbor".into(),
                );
            }
            Some(UniversalMcResqParameters {
                delay_time,
                allowed_time,
                attempt_time,
                code_chan,
                qof,
                min_period,
                num_tot_trans_20ms,
                num_tot_trans_5ms,
                num_preamble_rc1_rc2,
                num_preamble,
                power_delta,
                neighbor_configured,
            })
        } else {
            None
        };
        let pdch_supported = GeneralNeighborListMessage::read_bool_vec(bs, num_nghbr)?;
        Self::read_unlm_zero_tail(bs, "MC radio-interface record")?;
        Ok(UniversalMcRadioInterface {
            pilot_inc,
            nghbr_srch_mode,
            srch_win_n,
            srch_offset_incl,
            freq_fields_incl,
            use_timing,
            global_timing,
            nghbr_set_entry_info,
            nghbr_set_access_info,
            neighbors,
            resq,
            pdch_supported,
        })
    }

    fn validate_mc_radio_interface(record: &UniversalMcRadioInterface) {
        assert!(
            (1..=15).contains(&record.pilot_inc),
            "UNLM MC PILOT_INC must be 1..=15"
        );
        assert!(
            record.neighbors.len() <= 63,
            "UNLM MC NUM_NGHBR must fit in 6 bits"
        );
        assert!(
            record.nghbr_srch_mode <= 0b11,
            "UNLM MC NGHBR_SRCH_MODE must fit in 2 bits"
        );
        assert!(
            Self::unlm_has_common_search_window(record.nghbr_srch_mode)
                == record.srch_win_n.is_some(),
            "UNLM MC SRCH_WIN_N presence must match NGHBR_SRCH_MODE"
        );
        assert!(
            !record.srch_offset_incl
                || GeneralNeighborListMessage::has_search_window(record.nghbr_srch_mode),
            "UNLM MC SRCH_OFFSET_INCL requires search-window mode"
        );
        assert!(
            record.use_timing || record.global_timing.is_none(),
            "UNLM MC GLOBAL_TIMING_INCL requires USE_TIMING"
        );
        assert!(
            record.pdch_supported.len() == record.neighbors.len(),
            "UNLM MC NGHBR_PDCH_SUPPORTED records must match NUM_NGHBR"
        );

        for neighbor in &record.neighbors {
            assert!(
                neighbor.nghbr_config <= 0b100,
                "UNLM MC NGHBR_CONFIG 0b101..0b111 is reserved"
            );
            assert!(
                (neighbor.nghbr_config == 0b011) == neighbor.bcch_support.is_some(),
                "UNLM MC BCCH_SUPPORT presence must match NGHBR_CONFIG=011"
            );
            assert!(
                GeneralNeighborListMessage::has_search_priority(record.nghbr_srch_mode)
                    == neighbor.search_priority.is_some(),
                "UNLM MC SEARCH_PRIORITY presence must match NGHBR_SRCH_MODE"
            );
            assert!(
                GeneralNeighborListMessage::has_search_window(record.nghbr_srch_mode)
                    == neighbor.srch_win_nghbr.is_some(),
                "UNLM MC SRCH_WIN_NGHBR presence must match NGHBR_SRCH_MODE"
            );
            assert!(
                record.srch_offset_incl == neighbor.srch_offset_nghbr.is_some(),
                "UNLM MC SRCH_OFFSET_NGHBR presence must match SRCH_OFFSET_INCL"
            );
            if record.freq_fields_incl {
                assert!(
                    neighbor.nghbr_band.is_some() == neighbor.nghbr_freq.is_some(),
                    "UNLM MC FREQ_INCL requires both NGHBR_BAND and NGHBR_FREQ"
                );
            } else {
                assert!(
                    neighbor.nghbr_band.is_none() && neighbor.nghbr_freq.is_none(),
                    "UNLM MC frequency fields must be absent when FREQ_FIELDS_INCL=0"
                );
            }
            if record.use_timing {
                if let Some(timing) = &neighbor.timing {
                    if record.global_timing.is_some() {
                        assert!(
                            timing.tx_duration.is_none() && timing.tx_period.is_none(),
                            "UNLM MC per-neighbor duration/period must be absent when GLOBAL_TIMING_INCL=1"
                        );
                    } else {
                        timing.tx_duration.expect(
                            "UNLM MC NGHBR_TX_DURATION required when TIMING_INCL=1 and GLOBAL_TIMING_INCL=0",
                        );
                        timing.tx_period.expect(
                            "UNLM MC NGHBR_TX_PERIOD required when TIMING_INCL=1 and GLOBAL_TIMING_INCL=0",
                        );
                    }
                }
            } else {
                assert!(
                    neighbor.timing.is_none(),
                    "UNLM MC TIMING_INCL must be absent when USE_TIMING=0"
                );
            }
            assert!(
                record.nghbr_set_entry_info == neighbor.access_entry_ho.is_some(),
                "UNLM MC ACCESS_ENTRY_HO presence must match NGHBR_SET_ENTRY_INFO"
            );
            assert!(
                record.nghbr_set_access_info == neighbor.access_ho_allowed.is_some(),
                "UNLM MC ACCESS_HO_ALLOWED presence must match NGHBR_SET_ACCESS_INFO"
            );
        }

        if let Some(resq) = &record.resq {
            assert!(
                resq.code_chan > 0,
                "UNLM MC RESQ_CODE_CHAN must be non-zero"
            );
            assert!(
                resq.num_tot_trans_20ms.is_some() == resq.num_tot_trans_5ms.is_some(),
                "UNLM MC RESQ_NUM_TOT_TRANS_INCL requires both 20ms and 5ms values"
            );
            assert!(
                resq.neighbor_configured.len() == record.neighbors.len(),
                "UNLM MC NGHBR_RESQ_CONFIGURED records must match NUM_NGHBR"
            );
            assert!(
                resq.neighbor_configured
                    .iter()
                    .any(|configured| *configured),
                "UNLM MC RESQ_ENABLED requires at least one configured neighbor"
            );
        }
    }

    fn unlm_has_common_search_window(nghbr_srch_mode: u8) -> bool {
        matches!(nghbr_srch_mode, 0b00 | 0b01)
    }

    fn read_unlm_zero_tail(bs: &mut Bitstream, context: &str) -> Result<(), crate::error::Error> {
        if bs.len() > 7 {
            return Err(format!("UNLM {context} has excess reserved bits").into());
        }
        while !bs.is_empty() {
            if bs.read_bits(1)? != 0 {
                return Err(format!("UNLM {context} reserved padding bits must be zero").into());
            }
        }
        Ok(())
    }

    fn write_hrpd_radio_interface(neighbors: &[HrpdNeighborRecord], bs: &mut Bitstream) {
        assert!(
            neighbors.len() <= 63,
            "UNLM NUM_HRPD_NGHBR must fit in 6 bits"
        );
        bs.write_u8(neighbors.len() as u8, 6);
        for neighbor in neighbors {
            assert!(
                neighbor.nghbr_band.is_some() == neighbor.nghbr_freq.is_some(),
                "UNLM HRPD NGHBR_FREQ_INCL requires both NGHBR_BAND and NGHBR_FREQ"
            );
            let mut record = Bitstream::new();
            record.write_u32(neighbor.nghbr_pn as u32, 9);
            let nghbr_freq_incl = neighbor.nghbr_band.is_some();
            record.write_u8(nghbr_freq_incl as u8, 1);
            if nghbr_freq_incl {
                record.write_u8(neighbor.nghbr_band.expect("validated"), 5);
                record.write_u32(neighbor.nghbr_freq.expect("validated") as u32, 11);
            }
            record.write_u8(neighbor.pn_association_ind as u8, 1);
            record.write_u8(neighbor.data_association_ind as u8, 1);
            pad_to_octet(&mut record);
            // The spec defines HRPD_NGHBR_REC_LEN as total record octets,
            // including this length field, minus one.
            let record_len = record.len() / 8;
            assert!(
                record_len <= u8::MAX as usize,
                "UNLM HRPD_NGHBR_REC_LEN must fit in one octet"
            );
            bs.write_u8(record_len as u8, 8);
            bs.extend(&record);
        }
    }

    fn read_hrpd_radio_interface(
        bs: &mut Bitstream,
    ) -> Result<Vec<HrpdNeighborRecord>, crate::error::Error> {
        let num_hrpd_nghbr = bs.read_bits(6)? as usize;
        let mut neighbors = Vec::with_capacity(num_hrpd_nghbr);
        for _ in 0..num_hrpd_nghbr {
            let record_len = bs.read_bits(8)? as usize;
            if record_len == 0 {
                return Err("UNLM HRPD_NGHBR_REC_LEN must be non-zero".into());
            }
            // HRPD_NGHBR_REC_LEN is total record octets including the length
            // field, minus one, so it is exactly the remaining body octets.
            let record_bits = record_len * 8;
            if bs.len() < record_bits {
                return Err("UNLM HRPD neighbor record length exceeds radio-interface body".into());
            }
            let mut record = bs.drain(0..record_bits);
            let nghbr_pn = record.read_bits(9)? as u16;
            let nghbr_freq_incl = record.read_bits(1)? != 0;
            let (nghbr_band, nghbr_freq) = if nghbr_freq_incl {
                (
                    Some(record.read_bits(5)? as u8),
                    Some(record.read_bits(11)? as u16),
                )
            } else {
                (None, None)
            };
            let pn_association_ind = record.read_bits(1)? != 0;
            let data_association_ind = record.read_bits(1)? != 0;
            if record.len() > 7 {
                return Err("UNLM HRPD neighbor record has excess reserved bits".into());
            }
            if record.bits().iter().any(|bit| *bit != 0) {
                return Err("UNLM HRPD_NGHBR_REC_RESERVED bits must be zero".into());
            }
            neighbors.push(HrpdNeighborRecord {
                nghbr_pn,
                nghbr_band,
                nghbr_freq,
                pn_association_ind,
                data_association_ind,
            });
        }
        if bs.len() > 7 {
            return Err("UNLM HRPD radio-interface record has excess reserved bits".into());
        }
        if bs.bits().iter().any(|bit| *bit != 0) {
            return Err("UNLM HRPD radio-interface RESERVED bits must be zero".into());
        }
        Ok(neighbors)
    }
}

impl SecurityModeCommandMessage {
    /// Encode SMCM per C.S0005-E 3.7.2.3.2.35.
    pub fn to_sdu(&self) -> Bitstream {
        Self::validate_c_sig_encrypt_mode(self.c_sig_encrypt_mode)
            .expect("SMCM C_SIG_ENCRYPT_MODE reserved");
        assert!(
            (self.c_sig_encrypt_mode == 1 || self.c_sig_encrypt_mode == 2)
                == self.enc_key_size.is_some(),
            "SMCM ENC_KEY_SIZE presence must match C_SIG_ENCRYPT_MODE 001/010"
        );
        if let Some(enc_key_size) = self.enc_key_size {
            Self::validate_enc_key_size(enc_key_size).expect("SMCM ENC_KEY_SIZE reserved");
        }
        assert!(
            self.change_keys.is_some() == self.use_uak.is_some(),
            "SMCM MSG_INT_INFO_INCL requires both CHANGE_KEYS and USE_UAK"
        );

        let mut bs = Bitstream::new();
        bs.write_u8(self.c_sig_encrypt_mode, 3);
        if let Some(enc_key_size) = self.enc_key_size {
            bs.write_u8(enc_key_size, 3);
        }
        bs.write_u8(self.change_keys.is_some() as u8, 1);
        if let (Some(change_keys), Some(use_uak)) = (self.change_keys, self.use_uak) {
            bs.write_u8(change_keys as u8, 1);
            bs.write_u8(use_uak as u8, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let c_sig_encrypt_mode = bs.read_bits(3)? as u8;
        Self::validate_c_sig_encrypt_mode(c_sig_encrypt_mode)?;
        let enc_key_size = if matches!(c_sig_encrypt_mode, 1 | 2) {
            let enc_key_size = bs.read_bits(3)? as u8;
            Self::validate_enc_key_size(enc_key_size)?;
            Some(enc_key_size)
        } else {
            None
        };
        let msg_int_info_incl = bs.read_bits(1)? != 0;
        let (change_keys, use_uak) = if msg_int_info_incl {
            (Some(bs.read_bits(1)? != 0), Some(bs.read_bits(1)? != 0))
        } else {
            (None, None)
        };
        if !bs.is_empty() {
            return Err("SMCM has trailing bits after integrity fields".into());
        }
        Ok(Self {
            c_sig_encrypt_mode,
            enc_key_size,
            change_keys,
            use_uak,
        })
    }

    fn validate_c_sig_encrypt_mode(mode: u8) -> Result<(), crate::error::Error> {
        if mode <= 2 {
            Ok(())
        } else {
            Err(format!("SMCM reserved C_SIG_ENCRYPT_MODE 0b{mode:03b}").into())
        }
    }

    fn validate_enc_key_size(enc_key_size: u8) -> Result<(), crate::error::Error> {
        if matches!(enc_key_size, 1 | 2) {
            Ok(())
        } else {
            Err(format!("SMCM reserved ENC_KEY_SIZE 0b{enc_key_size:03b}").into())
        }
    }
}

impl UniversalPageBlock {
    pub fn to_bits(&self) -> Vec<u8> {
        self.validate();
        let mut bs = Bitstream::new();
        Self::write_count(&mut bs, self.addresses.broadcasts.len(), 5, "NUM_BCAST");
        Self::write_count(&mut bs, self.addresses.imsis.len(), 6, "NUM_IMSI");
        Self::write_count(&mut bs, self.addresses.tmsis.len(), 6, "NUM_TMSI");
        bs.write_u8(0, 1); // RESERVED_TYPE_INCLUDED
        for broadcast in &self.addresses.broadcasts {
            assert!(
                broadcast.burst_type <= 0x3f,
                "UPM BURST_TYPE must fit in 6 bits"
            );
            bs.write_u8(broadcast.burst_type, 6);
        }
        for bit_index in 0..16 {
            for broadcast in &self.addresses.broadcasts {
                bs.write_u8(((broadcast.address_bits >> bit_index) as u8) & 1, 1);
            }
            for imsi in &self.addresses.imsis {
                bs.write_u8(((imsi.address_bits >> bit_index) as u8) & 1, 1);
            }
            for tmsi in &self.addresses.tmsis {
                bs.write_u8(((tmsi.address_bits >> bit_index) as u8) & 1, 1);
            }
        }
        for record in &self.records {
            Self::write_record(&mut bs, record);
        }
        bs.bits().to_vec()
    }

    pub fn from_bits(bits: &[u8]) -> Result<Self, crate::error::Error> {
        if bits.iter().any(|bit| *bit > 1) {
            return Err("UPM page block bits must contain only 0/1 values".into());
        }
        let mut bs = Bitstream::new_init(bits);
        let num_bcast = Self::read_count(&mut bs, 5, "NUM_BCAST")?;
        let num_imsi = Self::read_count(&mut bs, 6, "NUM_IMSI")?;
        let num_tmsi = Self::read_count(&mut bs, 6, "NUM_TMSI")?;
        let num_reserved = Self::read_count(&mut bs, 6, "NUM_RESERVED_TYPE")?;
        if num_reserved != 0 {
            return Err("UPM reserved address types are not valid page targets".into());
        }

        let mut broadcasts = Vec::with_capacity(num_bcast);
        for _ in 0..num_bcast {
            broadcasts.push(UniversalPageBroadcastAddress {
                burst_type: bs.read_bits(6)? as u8,
                address_bits: 0,
            });
        }
        let mut imsis = vec![UniversalPagePartialAddress { address_bits: 0 }; num_imsi];
        let mut tmsis = vec![UniversalPagePartialAddress { address_bits: 0 }; num_tmsi];
        for bit_index in 0..16 {
            for broadcast in &mut broadcasts {
                if bs.read_bits(1)? != 0 {
                    broadcast.address_bits |= 1u16 << bit_index;
                }
            }
            for imsi in &mut imsis {
                if bs.read_bits(1)? != 0 {
                    imsi.address_bits |= 1u16 << bit_index;
                }
            }
            for tmsi in &mut tmsis {
                if bs.read_bits(1)? != 0 {
                    tmsi.address_bits |= 1u16 << bit_index;
                }
            }
        }

        let mut records = Vec::with_capacity(num_bcast + num_imsi + num_tmsi);
        for _ in 0..num_bcast {
            records.push(Self::read_broadcast_record(&mut bs)?);
        }
        for _ in 0..num_imsi {
            records.push(Self::read_imsi_record(&mut bs)?);
        }
        for _ in 0..num_tmsi {
            records.push(Self::read_tmsi_record(&mut bs)?);
        }
        if !bs.is_empty() {
            return Err("UPM page block has trailing bits after page records".into());
        }

        Ok(Self {
            addresses: UniversalPageInterleavedAddresses {
                broadcasts,
                imsis,
                tmsis,
            },
            records,
        })
    }

    fn validate(&self) {
        assert!(
            self.addresses.broadcasts.len() <= 32,
            "UPM NUM_BCAST must fit in 5 bits"
        );
        assert!(
            self.addresses.imsis.len() <= 64,
            "UPM NUM_IMSI must fit in 6 bits"
        );
        assert!(
            self.addresses.tmsis.len() <= 64,
            "UPM NUM_TMSI must fit in 6 bits"
        );
        let expected_bcast = self.addresses.broadcasts.len();
        let expected_imsi = self.addresses.imsis.len();
        let expected_tmsi = self.addresses.tmsis.len();
        let mut seen_bcast = 0usize;
        let mut seen_imsi = 0usize;
        let mut seen_tmsi = 0usize;
        let mut phase = 0u8;
        for record in &self.records {
            match Self::record_category(record) {
                0 => {
                    assert!(phase == 0, "UPM broadcast records must be first");
                    seen_bcast += 1;
                }
                1 => {
                    assert!(phase <= 1, "UPM IMSI records must precede TMSI records");
                    phase = 1;
                    seen_imsi += 1;
                }
                2 => {
                    phase = 2;
                    seen_tmsi += 1;
                }
                _ => unreachable!(),
            }
        }
        assert!(
            seen_bcast == expected_bcast,
            "UPM broadcast record count must match NUM_BCAST"
        );
        assert!(
            seen_imsi == expected_imsi,
            "UPM IMSI record count must match NUM_IMSI"
        );
        assert!(
            seen_tmsi == expected_tmsi,
            "UPM TMSI record count must match NUM_TMSI"
        );
    }

    fn record_category(record: &UniversalPageRecord) -> u8 {
        match record {
            UniversalPageRecord::EnhancedBroadcast { .. } => 0,
            UniversalPageRecord::MobileStation {
                address_type:
                    UniversalPageMobileAddressType::Class0 { .. }
                    | UniversalPageMobileAddressType::Class1 { .. },
                ..
            }
            | UniversalPageRecord::MessageAnnouncement {
                address_type: UniversalPageAnnouncementAddressType::Imsi,
            } => 1,
            UniversalPageRecord::MobileStation {
                address_type: UniversalPageMobileAddressType::Tmsi { .. },
                ..
            }
            | UniversalPageRecord::MessageAnnouncement {
                address_type: UniversalPageAnnouncementAddressType::Tmsi,
            } => 2,
        }
    }

    fn write_count(bs: &mut Bitstream, count: usize, bits: usize, field_name: &str) {
        if count == 0 {
            bs.write_u8(0, 1);
        } else {
            assert!(
                count <= (1usize << bits),
                "UPM {field_name} count exceeds field capacity"
            );
            bs.write_u8(1, 1);
            bs.write_u32((count - 1) as u32, bits);
        }
    }

    fn read_count(
        bs: &mut Bitstream,
        bits: usize,
        _field_name: &str,
    ) -> Result<usize, crate::error::Error> {
        Ok(if bs.read_bits(1)? != 0 {
            bs.read_bits(bits)? as usize + 1
        } else {
            0
        })
    }

    fn write_record(bs: &mut Bitstream, record: &UniversalPageRecord) {
        match record {
            UniversalPageRecord::MobileStation {
                address_type,
                msg_seq,
                service_option,
                add_record,
            } => {
                Self::write_mobile_page_class(bs, address_type);
                bs.write_u8(*msg_seq, 3);
                Self::write_mobile_page_type(bs, address_type);
                Self::write_ms_sdu(bs, *service_option, add_record);
            }
            UniversalPageRecord::MessageAnnouncement { address_type } => match address_type {
                UniversalPageAnnouncementAddressType::Imsi => {
                    bs.write_u8(0b11, 2);
                    bs.write_u8(0b11, 2);
                    bs.write_u8(0b01, 2);
                }
                UniversalPageAnnouncementAddressType::Tmsi => {
                    bs.write_u8(0b11, 2);
                    bs.write_u8(0b11, 2);
                    bs.write_u8(0b10, 2);
                }
            },
            UniversalPageRecord::EnhancedBroadcast {
                addr_len,
                bc_addr_remainder,
                bcn,
                time_offset,
                repeat_time_offset,
                add_record,
            } => {
                assert!(*addr_len >= 2, "UPM broadcast ADDR_LEN must be >= 2");
                assert!(
                    bc_addr_remainder.len() == (*addr_len as usize).saturating_sub(2),
                    "UPM BC_ADDR_REMAINDER length must match ADDR_LEN"
                );
                assert!(add_record.len() <= 15, "UPM ADD_BCAST_RECORD too long");
                bs.write_u8(0b11, 2);
                bs.write_u8(0b11, 2);
                bs.write_u8(0b00, 2);
                bs.write_u8(*addr_len, 4);
                for byte in bc_addr_remainder {
                    bs.write_u8(*byte, 8);
                }
                let ext_ind = match (repeat_time_offset, add_record.is_empty()) {
                    (None, true) => 0b00,
                    (Some(_), true) => 0b01,
                    (None, false) => 0b10,
                    (Some(_), false) => 0b11,
                };
                bs.write_u8(ext_ind, 2);
                if !add_record.is_empty() {
                    bs.write_u8(add_record.len() as u8, 4);
                }
                bs.write_u8(*bcn, 3);
                bs.write_u32(*time_offset as u32, 10);
                if let Some(repeat_time_offset) = repeat_time_offset {
                    bs.write_u8(*repeat_time_offset, 5);
                }
                for byte in add_record {
                    bs.write_u8(*byte, 8);
                }
            }
        }
    }

    fn write_mobile_page_class(bs: &mut Bitstream, address_type: &UniversalPageMobileAddressType) {
        match address_type {
            UniversalPageMobileAddressType::Class0 {
                imsi_11_12, mcc, ..
            } => {
                bs.write_u8(0b00, 2);
                bs.write_u8(
                    match (imsi_11_12.is_some(), mcc.is_some()) {
                        (false, false) => 0b00,
                        (true, false) => 0b01,
                        (false, true) => 0b10,
                        (true, true) => 0b11,
                    },
                    2,
                );
            }
            UniversalPageMobileAddressType::Class1 { mcc, .. } => {
                bs.write_u8(0b01, 2);
                bs.write_u8(if mcc.is_some() { 0b01 } else { 0b00 }, 2);
            }
            UniversalPageMobileAddressType::Tmsi {
                tmsi_zone,
                tmsi_code_addr_31_16,
                tmsi_code_addr_23_16,
            } => {
                bs.write_u8(0b10, 2);
                bs.write_u8(
                    match (
                        tmsi_zone.is_some(),
                        tmsi_code_addr_31_16.is_some(),
                        tmsi_code_addr_23_16.is_some(),
                    ) {
                        (false, true, false) => 0b00,
                        (false, false, true) => 0b01,
                        (false, false, false) => 0b10,
                        (true, true, false) => 0b11,
                        _ => panic!("UPM invalid TMSI page format field presence"),
                    },
                    2,
                );
            }
        }
    }

    fn write_mobile_page_type(bs: &mut Bitstream, address_type: &UniversalPageMobileAddressType) {
        match address_type {
            UniversalPageMobileAddressType::Class0 {
                imsi_s_33_16,
                imsi_11_12,
                mcc,
            } => {
                if let Some(mcc) = mcc {
                    bs.write_u32(*mcc as u32, 10);
                }
                if let Some(imsi_11_12) = imsi_11_12 {
                    bs.write_u8(*imsi_11_12, 7);
                }
                bs.write_u32(*imsi_s_33_16, 18);
            }
            UniversalPageMobileAddressType::Class1 {
                imsi_addr_num,
                imsi_11_12,
                mcc,
                imsi_s_33_16,
            } => {
                bs.write_u8(*imsi_addr_num, 3);
                if let Some(mcc) = mcc {
                    bs.write_u32(*mcc as u32, 10);
                }
                bs.write_u8(*imsi_11_12, 7);
                bs.write_u32(*imsi_s_33_16, 18);
            }
            UniversalPageMobileAddressType::Tmsi {
                tmsi_zone,
                tmsi_code_addr_31_16,
                tmsi_code_addr_23_16,
            } => {
                if let Some(zone) = tmsi_zone {
                    assert!(
                        (1..=8).contains(&zone.len()),
                        "UPM TMSI_ZONE_LEN must be 1..=8"
                    );
                    bs.write_u8(zone.len() as u8, 4);
                    for byte in zone {
                        bs.write_u8(*byte, 8);
                    }
                }
                if let Some(value) = tmsi_code_addr_31_16 {
                    bs.write_u32(*value as u32, 16);
                }
                if let Some(value) = tmsi_code_addr_23_16 {
                    bs.write_u8(*value, 8);
                }
            }
        }
    }

    fn write_ms_sdu(bs: &mut Bitstream, service_option: u16, add_record: &[u8]) {
        assert!(add_record.len() <= 15, "UPM ADD_MS_RECORD too long");
        if add_record.is_empty() {
            bs.write_u8(0, 1);
        } else {
            bs.write_u8(1, 1);
            bs.write_u8(add_record.len() as u8, 4);
        }
        bs.write_u32(service_option as u32, 16);
        for byte in add_record {
            bs.write_u8(*byte, 8);
        }
    }

    fn read_page_class(bs: &mut Bitstream) -> Result<(u8, u8, Option<u8>), crate::error::Error> {
        let page_class = bs.read_bits(2)? as u8;
        let page_subclass = bs.read_bits(2)? as u8;
        let page_subclass_ext = if page_class == 0b11 && page_subclass != 0b00 {
            Some(bs.read_bits(2)? as u8)
        } else {
            None
        };
        Ok((page_class, page_subclass, page_subclass_ext))
    }

    fn read_broadcast_record(
        bs: &mut Bitstream,
    ) -> Result<UniversalPageRecord, crate::error::Error> {
        let (page_class, page_subclass, page_subclass_ext) = Self::read_page_class(bs)?;
        if (page_class, page_subclass, page_subclass_ext) != (0b11, 0b11, Some(0b00)) {
            return Err("UPM broadcast address requires page format 15.0".into());
        }
        let addr_len = bs.read_bits(4)? as u8;
        if addr_len < 2 {
            return Err("UPM broadcast ADDR_LEN must be >= 2".into());
        }
        let mut bc_addr_remainder = Vec::with_capacity(addr_len as usize - 2);
        for _ in 0..(addr_len as usize - 2) {
            bc_addr_remainder.push(bs.read_bits(8)? as u8);
        }
        let ext_ind = bs.read_bits(2)? as u8;
        let add_len = if matches!(ext_ind, 0b10 | 0b11) {
            let len = bs.read_bits(4)? as usize;
            if len == 0 {
                return Err("UPM EXT_BCAST_SDU_LENGTH must be non-zero when included".into());
            }
            len
        } else {
            0
        };
        let bcn = bs.read_bits(3)? as u8;
        let time_offset = bs.read_bits(10)? as u16;
        let repeat_time_offset = if matches!(ext_ind, 0b01 | 0b11) {
            Some(bs.read_bits(5)? as u8)
        } else {
            None
        };
        let mut add_record = Vec::with_capacity(add_len);
        for _ in 0..add_len {
            add_record.push(bs.read_bits(8)? as u8);
        }
        Ok(UniversalPageRecord::EnhancedBroadcast {
            addr_len,
            bc_addr_remainder,
            bcn,
            time_offset,
            repeat_time_offset,
            add_record,
        })
    }

    fn read_imsi_record(bs: &mut Bitstream) -> Result<UniversalPageRecord, crate::error::Error> {
        let (page_class, page_subclass, page_subclass_ext) = Self::read_page_class(bs)?;
        if (page_class, page_subclass, page_subclass_ext) == (0b11, 0b11, Some(0b01)) {
            return Ok(UniversalPageRecord::MessageAnnouncement {
                address_type: UniversalPageAnnouncementAddressType::Imsi,
            });
        }
        let msg_seq = bs.read_bits(3)? as u8;
        let address_type = match (page_class, page_subclass, page_subclass_ext) {
            (0b00, subclass @ 0b00..=0b11, None) => {
                let mcc = if matches!(subclass, 0b10 | 0b11) {
                    Some(bs.read_bits(10)? as u16)
                } else {
                    None
                };
                let imsi_11_12 = if matches!(subclass, 0b01 | 0b11) {
                    Some(bs.read_bits(7)? as u8)
                } else {
                    None
                };
                UniversalPageMobileAddressType::Class0 {
                    imsi_s_33_16: bs.read_bits(18)? as u32,
                    imsi_11_12,
                    mcc,
                }
            }
            (0b01, subclass @ 0b00..=0b01, None) => {
                let imsi_addr_num = bs.read_bits(3)? as u8;
                let mcc = if subclass == 0b01 {
                    Some(bs.read_bits(10)? as u16)
                } else {
                    None
                };
                UniversalPageMobileAddressType::Class1 {
                    imsi_addr_num,
                    imsi_11_12: bs.read_bits(7)? as u8,
                    imsi_s_33_16: bs.read_bits(18)? as u32,
                    mcc,
                }
            }
            (0b01, 0b10 | 0b11, None) => {
                return Err("UPM reserved IMSI page format 6/7".into());
            }
            (0b11, 0b01 | 0b10, Some(_)) | (0b11, 0b11, Some(0b11)) => {
                return Err("UPM reserved IMSI page format".into());
            }
            _ => return Err("UPM IMSI address count does not match page record class".into()),
        };
        let (service_option, add_record) = Self::read_ms_sdu(bs)?;
        Ok(UniversalPageRecord::MobileStation {
            address_type,
            msg_seq,
            service_option,
            add_record,
        })
    }

    fn read_tmsi_record(bs: &mut Bitstream) -> Result<UniversalPageRecord, crate::error::Error> {
        let (page_class, page_subclass, page_subclass_ext) = Self::read_page_class(bs)?;
        if (page_class, page_subclass, page_subclass_ext) == (0b11, 0b11, Some(0b10)) {
            return Ok(UniversalPageRecord::MessageAnnouncement {
                address_type: UniversalPageAnnouncementAddressType::Tmsi,
            });
        }
        if page_class != 0b10 || page_subclass_ext.is_some() {
            return Err("UPM TMSI address count does not match page record class".into());
        }
        let msg_seq = bs.read_bits(3)? as u8;
        let address_type = match page_subclass {
            0b00 => UniversalPageMobileAddressType::Tmsi {
                tmsi_zone: None,
                tmsi_code_addr_31_16: Some(bs.read_bits(16)? as u16),
                tmsi_code_addr_23_16: None,
            },
            0b01 => UniversalPageMobileAddressType::Tmsi {
                tmsi_zone: None,
                tmsi_code_addr_31_16: None,
                tmsi_code_addr_23_16: Some(bs.read_bits(8)? as u8),
            },
            0b10 => UniversalPageMobileAddressType::Tmsi {
                tmsi_zone: None,
                tmsi_code_addr_31_16: None,
                tmsi_code_addr_23_16: None,
            },
            0b11 => {
                let zone_len = bs.read_bits(4)? as usize;
                if !(1..=8).contains(&zone_len) {
                    return Err("UPM TMSI_ZONE_LEN must be 1..=8".into());
                }
                let mut zone = Vec::with_capacity(zone_len);
                for _ in 0..zone_len {
                    zone.push(bs.read_bits(8)? as u8);
                }
                UniversalPageMobileAddressType::Tmsi {
                    tmsi_zone: Some(zone),
                    tmsi_code_addr_31_16: Some(bs.read_bits(16)? as u16),
                    tmsi_code_addr_23_16: None,
                }
            }
            _ => unreachable!(),
        };
        let (service_option, add_record) = Self::read_ms_sdu(bs)?;
        Ok(UniversalPageRecord::MobileStation {
            address_type,
            msg_seq,
            service_option,
            add_record,
        })
    }

    fn read_ms_sdu(bs: &mut Bitstream) -> Result<(u16, Vec<u8>), crate::error::Error> {
        let add_len = if bs.read_bits(1)? != 0 {
            let len = bs.read_bits(4)? as usize;
            if len == 0 {
                return Err("UPM EXT_MS_SDU_LENGTH must be non-zero when included".into());
            }
            len
        } else {
            0
        };
        let service_option = bs.read_bits(16)? as u16;
        let mut add_record = Vec::with_capacity(add_len);
        for _ in 0..add_len {
            add_record.push(bs.read_bits(8)? as u8);
        }
        Ok((service_option, add_record))
    }
}

impl UniversalPageMessage {
    /// Encode an unsegmented or reassembled UPM Universal Page Block per
    /// C.S0004-E 3.1.2.3.2.4.2.1 / C.S0005-E 3.7.2.3.2.36.
    ///
    /// `block_body_bits` begins after the four UPM common fields and contains
    /// the interleaved address fields plus zero or more page records.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.block_body_bits.iter().all(|bit| *bit <= 1),
            "UPM block_body_bits must contain only 0/1 values"
        );
        UniversalPageBlock::from_bits(&self.block_body_bits)
            .expect("UPM Universal Page Block must be spec-valid");
        let mut bs = Bitstream::new();
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.acc_msg_seq, 6);
        bs.write_u8(self.read_next_slot as u8, 1);
        bs.write_u8(self.read_next_slot_bcast as u8, 1);
        for bit in &self.block_body_bits {
            bs.write_u8(*bit, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        if bs.len() < 14 {
            return Err("UPM common fields truncated".into());
        }
        let config_msg_seq = bs.read_bits(6)? as u8;
        let acc_msg_seq = bs.read_bits(6)? as u8;
        let read_next_slot = bs.read_bits(1)? != 0;
        let read_next_slot_bcast = bs.read_bits(1)? != 0;
        let block_body = bs.drain(0..bs.len());
        let block_body_bits = block_body.bits().to_vec();
        UniversalPageBlock::from_bits(&block_body_bits)?;
        Ok(Self {
            config_msg_seq,
            acc_msg_seq,
            read_next_slot,
            read_next_slot_bcast,
            block_body_bits,
        })
    }

    pub fn page_block(&self) -> Result<UniversalPageBlock, crate::error::Error> {
        UniversalPageBlock::from_bits(&self.block_body_bits)
    }

    pub fn from_page_block(
        config_msg_seq: u8,
        acc_msg_seq: u8,
        read_next_slot: bool,
        read_next_slot_bcast: bool,
        block: &UniversalPageBlock,
    ) -> Self {
        Self {
            config_msg_seq,
            acc_msg_seq,
            read_next_slot,
            read_next_slot_bcast,
            block_body_bits: block.to_bits(),
        }
    }
}

impl UniversalPageSegmentMessage {
    pub fn to_first_segment_sdu(&self) -> Bitstream {
        assert!(
            self.upm_segment_seq.is_none(),
            "UPM first segment must not include UPM_SEGMENT_SEQ"
        );
        self.segment_bits_to_sdu("first")
    }

    pub fn to_middle_segment_sdu(&self) -> Bitstream {
        self.to_continuation_segment_sdu(0b10, "middle")
    }

    pub fn to_final_segment_sdu(&self) -> Bitstream {
        self.to_continuation_segment_sdu(0b11, "final")
    }

    fn to_continuation_segment_sdu(&self, max_seq: u8, segment_name: &str) -> Bitstream {
        let upm_segment_seq = self
            .upm_segment_seq
            .expect("UPM continuation segment requires UPM_SEGMENT_SEQ");
        assert!(
            upm_segment_seq <= max_seq,
            "UPM {segment_name} segment UPM_SEGMENT_SEQ is out of range"
        );
        let mut bs = Bitstream::new();
        bs.write_u8(upm_segment_seq, 2);
        bs.extend(&self.segment_bits_to_sdu(segment_name));
        bs
    }

    pub fn from_first_segment_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        if bs.is_empty() {
            return Err("UPM first segment body is empty".into());
        }
        let segment = bs.drain(0..bs.len());
        Ok(Self {
            upm_segment_seq: None,
            segment_bits: segment.bits().to_vec(),
        })
    }

    pub fn from_middle_segment_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        Self::from_continuation_segment_sdu(bs, 0b10, "middle")
    }

    pub fn from_final_segment_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        Self::from_continuation_segment_sdu(bs, 0b11, "final")
    }

    fn from_continuation_segment_sdu(
        bs: &mut Bitstream,
        max_seq: u8,
        segment_name: &str,
    ) -> Result<Self, crate::error::Error> {
        if bs.len() < 3 {
            return Err(format!("UPM {segment_name} segment body truncated").into());
        }
        let upm_segment_seq = bs.read_bits(2)? as u8;
        if upm_segment_seq > max_seq {
            return Err(
                format!("UPM {segment_name} segment UPM_SEGMENT_SEQ is out of range").into(),
            );
        }
        let segment = bs.drain(0..bs.len());
        Ok(Self {
            upm_segment_seq: Some(upm_segment_seq),
            segment_bits: segment.bits().to_vec(),
        })
    }

    fn segment_bits_to_sdu(&self, segment_name: &str) -> Bitstream {
        assert!(
            !self.segment_bits.is_empty(),
            "UPM {segment_name} segment body must be non-empty"
        );
        assert!(
            self.segment_bits.iter().all(|bit| *bit <= 1),
            "UPM segment_bits must contain only 0/1 values"
        );
        let mut bs = Bitstream::new();
        for bit in &self.segment_bits {
            bs.write_u8(*bit, 1);
        }
        bs
    }
}

impl AuthenticationRequestMessage {
    /// Encode AUREQM per C.S0005-E 3.7.2.3.2.37.
    pub fn to_sdu(&self) -> Bitstream {
        assert_eq!(self.randa.len(), 16, "AUREQM RANDA must be 16 octets");
        assert_eq!(self.con_sqn.len(), 6, "AUREQM CON_SQN must be 6 octets");
        assert_eq!(self.mac_a.len(), 8, "AUREQM MAC_A must be 8 octets");

        let mut bs = Bitstream::new();
        for byte in &self.randa {
            bs.write_u8(*byte, 8);
        }
        for byte in &self.con_sqn {
            bs.write_u8(*byte, 8);
        }
        for byte in &self.amf {
            bs.write_u8(*byte, 8);
        }
        for byte in &self.mac_a {
            bs.write_u8(*byte, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        if bs.len() < 256 {
            return Err("AUREQM fixed authentication vector body truncated".into());
        }
        let mut randa = Vec::with_capacity(16);
        for _ in 0..16 {
            randa.push(bs.read_bits(8)? as u8);
        }
        let mut con_sqn = Vec::with_capacity(6);
        for _ in 0..6 {
            con_sqn.push(bs.read_bits(8)? as u8);
        }
        let amf = [bs.read_bits(8)? as u8, bs.read_bits(8)? as u8];
        let mut mac_a = Vec::with_capacity(8);
        for _ in 0..8 {
            mac_a.push(bs.read_bits(8)? as u8);
        }
        if !bs.is_empty() {
            return Err("AUREQM has trailing bits after MAC_A".into());
        }
        Ok(Self {
            randa,
            con_sqn,
            amf,
            mac_a,
        })
    }
}

impl AlternativeTechnologiesInformationMessage {
    /// Encode ATIM per C.S0005-E 3.7.2.3.2.45.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.radio_interfaces.len() <= 15,
            "ATIM NUM_RADIO_INTERFACE must fit in 4 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.radio_interfaces.len() as u8, 4);
        for radio_interface in &self.radio_interfaces {
            let (radio_interface_type, fields) = radio_interface.type_and_fields();
            assert!(
                fields.len() <= 1023,
                "ATIM RADIO_INTERFACE_LEN must fit in 10 bits"
            );
            bs.write_u8(radio_interface_type, 4);
            bs.write_u32(fields.len() as u32, 10);
            for byte in fields {
                bs.write_u8(*byte, 8);
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let num_radio_interface = bs.read_bits(4)? as usize;
        let mut radio_interfaces = Vec::with_capacity(num_radio_interface);
        for _ in 0..num_radio_interface {
            let radio_interface_type = bs.read_bits(4)? as u8;
            let radio_interface_len = bs.read_bits(10)? as usize;
            let record_bits = radio_interface_len * 8;
            if bs.len() < record_bits {
                return Err("ATIM radio-interface body length exceeds remaining SDU".into());
            }
            let mut fields = Vec::with_capacity(radio_interface_len);
            for _ in 0..radio_interface_len {
                fields.push(bs.read_bits(8)? as u8);
            }
            let radio_interface = match radio_interface_type {
                0b0010 => {
                    AlternativeHrpdRadioInterface::from_fields(&fields)?;
                    AlternativeTechnologyRadioInterfaceRecord::Hrpd { fields }
                }
                0b0011 => AlternativeTechnologyRadioInterfaceRecord::Eutran { fields },
                0b0100 => AlternativeTechnologyRadioInterfaceRecord::Wimax { fields },
                _ => {
                    return Err(format!(
                        "ATIM reserved RADIO_INTERFACE_TYPE 0b{radio_interface_type:04b}"
                    )
                    .into());
                }
            };
            radio_interfaces.push(radio_interface);
        }
        if !bs.is_empty() {
            return Err("ATIM has trailing bits after radio-interface records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            radio_interfaces,
        })
    }
}

impl AlternativeTechnologyRadioInterfaceRecord {
    pub fn hrpd(fields: &AlternativeHrpdRadioInterface) -> Self {
        Self::Hrpd {
            fields: fields.to_fields(),
        }
    }

    pub fn hrpd_fields(
        &self,
    ) -> Result<Option<AlternativeHrpdRadioInterface>, crate::error::Error> {
        match self {
            Self::Hrpd { fields } => Ok(Some(AlternativeHrpdRadioInterface::from_fields(fields)?)),
            Self::Eutran { .. } | Self::Wimax { .. } => Ok(None),
        }
    }

    fn type_and_fields(&self) -> (u8, &[u8]) {
        match self {
            Self::Hrpd { fields } => {
                AlternativeHrpdRadioInterface::from_fields(fields)
                    .expect("ATIM HRPD fields must be spec-valid");
                (0b0010, fields)
            }
            Self::Eutran { fields } => (0b0011, fields),
            Self::Wimax { fields } => (0b0100, fields),
        }
    }
}

impl AlternativeHrpdRadioInterface {
    pub fn to_fields(&self) -> Vec<u8> {
        self.validate();
        let mut bs = Bitstream::new();
        let mut common = Bitstream::new();
        common.write_u8(self.subnet_color_code.is_some() as u8, 1);
        if let Some(color_code) = self.subnet_color_code {
            common.write_u8(color_code, 8);
        }
        while (common.len() + 4) % 8 != 0 {
            common.write_u8(0, 1);
        }
        let common_octets = (common.len() + 4) / 8;
        bs.write_u8((common_octets - 1) as u8, 4);
        bs.extend(&common);
        bs.write_u8(self.neighbors.len() as u8, 6);

        let record_len = self
            .neighbors
            .iter()
            .map(Self::neighbor_record_octets)
            .max()
            .unwrap_or(1);
        assert!(
            record_len <= 32,
            "ATIM HRPD_NGHBR_REC_LEN must fit in 5 bits"
        );
        bs.write_u8((record_len - 1) as u8, 5);
        for (index, neighbor) in self.neighbors.iter().enumerate() {
            let mut record = Bitstream::new();
            Self::write_neighbor_record(index, neighbor, &mut record);
            pad_to_octet(&mut record);
            while record.len() < record_len * 8 {
                record.write_u8(0, 1);
            }
            bs.extend(&record);
        }
        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn from_fields(fields: &[u8]) -> Result<Self, crate::error::Error> {
        if fields.is_empty() {
            return Ok(Self {
                subnet_color_code: None,
                neighbors: Vec::new(),
            });
        }
        let mut bs = Bitstream::new_bytes(fields);
        let common_record_len = bs.read_bits(4)? as usize + 1;
        let common_body_bits = common_record_len
            .checked_mul(8)
            .and_then(|bits| bits.checked_sub(4))
            .ok_or("ATIM HRPD COMMON_RECORD_LEN invalid")?;
        if bs.len() < common_body_bits {
            return Err("ATIM HRPD common record length exceeds body".into());
        }
        let mut common = bs.drain(0..common_body_bits);
        let subnet_color_code = if common.read_bits(1)? != 0 {
            Some(common.read_bits(8)? as u8)
        } else {
            None
        };
        Self::read_zero_tail(&mut common, "common record")?;

        let num_hrpd_nghbr = bs.read_bits(6)? as usize;
        let hrpd_nghbr_rec_len = bs.read_bits(5)? as usize + 1;
        let mut neighbors = Vec::with_capacity(num_hrpd_nghbr);
        let mut previous_freq: Option<(u8, u16)> = None;
        for index in 0..num_hrpd_nghbr {
            let record_bits = hrpd_nghbr_rec_len * 8;
            if bs.len() < record_bits {
                return Err("ATIM HRPD neighbor record length exceeds body".into());
            }
            let mut record = bs.drain(0..record_bits);
            let nghbr_pn = record.read_bits(9)? as u16;
            let freq_same_as_prev = record.read_bits(1)? != 0;
            let (nghbr_band, nghbr_freq) = if freq_same_as_prev {
                if index == 0 || previous_freq.is_none() {
                    return Err(
                        "ATIM HRPD first neighbor cannot set NGHBR_FREQ_SAME_AS_PREV".into(),
                    );
                }
                (None, None)
            } else {
                let band = record.read_bits(5)? as u8;
                let freq = record.read_bits(11)? as u16;
                previous_freq = Some((band, freq));
                (Some(band), Some(freq))
            };
            let pn_association_ind = record.read_bits(1)? != 0;
            let data_association_ind = record.read_bits(1)? != 0;
            let subnet_color_code_ind = record.read_bits(2)? as u8;
            let subnet_color_code = match subnet_color_code_ind {
                0b00 => AlternativeHrpdNeighborSubnetColorCode::NotIncluded,
                0b01 => {
                    if subnet_color_code.is_none() {
                        return Err(
                            "ATIM HRPD neighbor color cannot reference absent common color".into(),
                        );
                    }
                    AlternativeHrpdNeighborSubnetColorCode::SameAsCommon
                }
                0b10 => {
                    AlternativeHrpdNeighborSubnetColorCode::Explicit(record.read_bits(8)? as u8)
                }
                _ => return Err("ATIM HRPD reserved NGHBR_SUBNET_COLOR_CODE_IND".into()),
            };
            Self::read_zero_tail(&mut record, "neighbor record")?;
            neighbors.push(AlternativeHrpdNeighborRecord {
                nghbr_pn,
                freq_same_as_prev,
                nghbr_band,
                nghbr_freq,
                pn_association_ind,
                data_association_ind,
                subnet_color_code,
            });
        }
        Self::read_zero_tail(&mut bs, "radio-interface record")?;
        Ok(Self {
            subnet_color_code,
            neighbors,
        })
    }

    fn validate(&self) {
        assert!(
            self.neighbors.len() <= 63,
            "ATIM NUM_HRPD_NGHBR must fit in 6 bits"
        );
        let mut previous_freq = false;
        for (index, neighbor) in self.neighbors.iter().enumerate() {
            assert!(
                neighbor.nghbr_pn <= 511,
                "ATIM HRPD NGHBR_PN must fit in 9 bits"
            );
            if neighbor.freq_same_as_prev {
                assert!(
                    index > 0 && previous_freq,
                    "ATIM HRPD first neighbor cannot set NGHBR_FREQ_SAME_AS_PREV"
                );
                assert!(
                    neighbor.nghbr_band.is_none() && neighbor.nghbr_freq.is_none(),
                    "ATIM HRPD same-as-previous frequency omits band/frequency"
                );
            } else {
                assert!(
                    neighbor.nghbr_band.is_some() && neighbor.nghbr_freq.is_some(),
                    "ATIM HRPD frequency requires both NGHBR_BAND and NGHBR_FREQ"
                );
                previous_freq = true;
            }
            if matches!(
                neighbor.subnet_color_code,
                AlternativeHrpdNeighborSubnetColorCode::SameAsCommon
            ) {
                assert!(
                    self.subnet_color_code.is_some(),
                    "ATIM HRPD same-as-common subnet color requires common color"
                );
            }
        }
    }

    fn neighbor_record_octets(neighbor: &AlternativeHrpdNeighborRecord) -> usize {
        let mut bits: usize = 9 + 1 + 1 + 1 + 2;
        if !neighbor.freq_same_as_prev {
            bits += 5 + 11;
        }
        if matches!(
            neighbor.subnet_color_code,
            AlternativeHrpdNeighborSubnetColorCode::Explicit(_)
        ) {
            bits += 8;
        }
        bits.div_ceil(8)
    }

    fn write_neighbor_record(
        index: usize,
        neighbor: &AlternativeHrpdNeighborRecord,
        bs: &mut Bitstream,
    ) {
        bs.write_u32(neighbor.nghbr_pn as u32, 9);
        bs.write_u8(neighbor.freq_same_as_prev as u8, 1);
        if !neighbor.freq_same_as_prev {
            bs.write_u8(neighbor.nghbr_band.expect("validated"), 5);
            bs.write_u32(neighbor.nghbr_freq.expect("validated") as u32, 11);
        } else {
            assert!(index > 0, "validated");
        }
        bs.write_u8(neighbor.pn_association_ind as u8, 1);
        bs.write_u8(neighbor.data_association_ind as u8, 1);
        match neighbor.subnet_color_code {
            AlternativeHrpdNeighborSubnetColorCode::NotIncluded => bs.write_u8(0b00, 2),
            AlternativeHrpdNeighborSubnetColorCode::SameAsCommon => bs.write_u8(0b01, 2),
            AlternativeHrpdNeighborSubnetColorCode::Explicit(color_code) => {
                bs.write_u8(0b10, 2);
                bs.write_u8(color_code, 8);
            }
        }
    }

    fn read_zero_tail(bs: &mut Bitstream, context: &str) -> Result<(), crate::error::Error> {
        while !bs.is_empty() {
            if bs.read_bits(1)? != 0 {
                return Err(format!("ATIM HRPD {context} reserved bits must be zero").into());
            }
        }
        Ok(())
    }
}

impl ForwardGeneralExtensionMessage {
    /// Encode forward GEM per C.S0005-E 3.7.2.3.2.44.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(!self.records.is_empty(), "GEM NUM_GE_REC must be non-zero");
        assert!(
            self.records.len() <= u8::MAX as usize,
            "GEM NUM_GE_REC must fit in one octet"
        );
        assert!(
            self.message_type & 0b1100_0000 == 0,
            "forward GEM MESSAGE_TYPE top two bits must be 00"
        );
        assert!(
            self.message_rec_bits.iter().all(|bit| *bit <= 1),
            "GEM message_rec_bits must contain only 0/1 values"
        );

        let mut bs = Bitstream::new();
        bs.write_u8(self.records.len() as u8, 8);
        for record in &self.records {
            match record {
                ForwardGeneralExtensionRecord::ReverseChannelInfo {
                    band_class,
                    rev_chan,
                } => {
                    bs.write_u8(0, 8);
                    bs.write_u8(2, 8);
                    bs.write_u8(*band_class, 5);
                    bs.write_u32(*rev_chan as u32, 11);
                }
                ForwardGeneralExtensionRecord::RadioConfigurationParameters { fields } => {
                    assert!(
                        fields.len() <= u8::MAX as usize,
                        "GEM GE_REC_LEN must fit in one octet"
                    );
                    bs.write_u8(1, 8);
                    bs.write_u8(fields.len() as u8, 8);
                    for byte in fields {
                        bs.write_u8(*byte, 8);
                    }
                }
            }
        }
        bs.write_u8(self.message_type, 8);
        for bit in &self.message_rec_bits {
            bs.write_u8(*bit, 1);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let num_ge_rec = bs.read_bits(8)? as usize;
        if num_ge_rec == 0 {
            return Err("GEM NUM_GE_REC must be non-zero".into());
        }
        let mut records = Vec::with_capacity(num_ge_rec);
        for _ in 0..num_ge_rec {
            let ge_rec_type = bs.read_bits(8)? as u8;
            let ge_rec_len = bs.read_bits(8)? as usize;
            if bs.len() < ge_rec_len * 8 {
                return Err("GEM GE_REC length exceeds remaining SDU".into());
            }
            let record = match ge_rec_type {
                0 => {
                    if ge_rec_len != 2 {
                        return Err("GEM reverse-channel GE_REC_LEN must be 2".into());
                    }
                    ForwardGeneralExtensionRecord::ReverseChannelInfo {
                        band_class: bs.read_bits(5)? as u8,
                        rev_chan: bs.read_bits(11)? as u16,
                    }
                }
                1 => {
                    let mut fields = Vec::with_capacity(ge_rec_len);
                    for _ in 0..ge_rec_len {
                        fields.push(bs.read_bits(8)? as u8);
                    }
                    ForwardGeneralExtensionRecord::RadioConfigurationParameters { fields }
                }
                _ => {
                    return Err(format!("GEM reserved GE_REC_TYPE 0x{ge_rec_type:02x}").into());
                }
            };
            records.push(record);
        }
        let message_type = bs.read_bits(8)? as u8;
        if message_type & 0b1100_0000 != 0 {
            return Err("forward GEM MESSAGE_TYPE top two bits must be 00".into());
        }
        let message_rec = bs.drain(0..bs.len());
        Ok(Self {
            records,
            message_type,
            message_rec_bits: message_rec.bits().to_vec(),
        })
    }
}

impl GeneralOverheadInformationMessage {
    /// Encode GOIM per C.S0005-E 3.7.2.3.2.42.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            !self.records.is_empty(),
            "GOIM NUM_GOI_REC must be non-zero"
        );
        assert!(
            self.records.len() <= 15,
            "GOIM NUM_GOI_REC must fit in 4 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.records.len() as u8, 4);
        for record in &self.records {
            let (record_type, fields) = record.type_and_fields();
            assert!(
                fields.len() >= 2,
                "GOIM text GOI_REC must include MSG_ENCODING and NUM_FIELDS"
            );
            assert!(
                fields.len() <= u8::MAX as usize,
                "GOIM GOI_REC_LEN must fit in one octet"
            );
            bs.write_u8(record_type, 8);
            bs.write_u8(fields.len() as u8, 8);
            for byte in fields {
                bs.write_u8(*byte, 8);
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let num_goi_rec = bs.read_bits(4)? as usize;
        if num_goi_rec == 0 {
            return Err("GOIM NUM_GOI_REC must be non-zero".into());
        }
        let mut records = Vec::with_capacity(num_goi_rec);
        for _ in 0..num_goi_rec {
            let record_type = bs.read_bits(8)? as u8;
            let record_len = bs.read_bits(8)? as usize;
            if record_len < 2 {
                return Err("GOIM text GOI_REC must include MSG_ENCODING and NUM_FIELDS".into());
            }
            if bs.len() < record_len * 8 {
                return Err("GOIM GOI_REC length exceeds remaining SDU".into());
            }
            let mut fields = Vec::with_capacity(record_len);
            for _ in 0..record_len {
                fields.push(bs.read_bits(8)? as u8);
            }
            let record = match record_type {
                0 => GeneralOverheadInformationRecord::OperatorName { fields },
                1 => GeneralOverheadInformationRecord::CellName { fields },
                _ => {
                    return Err(format!("GOIM reserved GOI_REC_TYPE 0x{record_type:02x}").into());
                }
            };
            records.push(record);
        }
        if !bs.is_empty() {
            return Err("GOIM has trailing bits after GOI records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            records,
        })
    }
}

impl GeneralOverheadInformationRecord {
    fn type_and_fields(&self) -> (u8, &[u8]) {
        match self {
            Self::OperatorName { fields } => (0, fields),
            Self::CellName { fields } => (1, fields),
        }
    }

    pub fn text_fields(&self) -> Result<CdmaTextFields, crate::error::Error> {
        decode_cdma_text_fields(self.type_and_fields().1, "GOIM GOI_REC")
    }
}

fn decode_cdma_text_fields(
    fields: &[u8],
    label: &str,
) -> Result<CdmaTextFields, crate::error::Error> {
    let mut bs = Bitstream::new_bytes(fields);
    if bs.len() < 13 {
        return Err(format!("{label} must include MSG_ENCODING and NUM_FIELDS").into());
    }
    let msg_encoding = bs.read_bits(5)? as u8;
    let num_fields = bs.read_bits(8)? as u8;
    let char_bits = cdma_text_char_bits(msg_encoding, num_fields);
    if bs.len() < char_bits {
        return Err(format!("{label} CHARi fields truncated").into());
    }
    let char_stream = bs.drain(0..char_bits);
    if bs.bits().iter().any(|bit| *bit != 0) {
        return Err(format!("{label} reserved padding bits must be zero").into());
    }
    let text = crate::sms::decode_user_data(msg_encoding, None, num_fields, char_stream.bits());
    Ok(CdmaTextFields {
        msg_encoding,
        num_fields,
        text,
        raw: fields.to_vec(),
    })
}

fn cdma_text_char_bits(msg_encoding: u8, num_fields: u8) -> usize {
    let n = num_fields as usize;
    match msg_encoding {
        0x02 | 0x03 | 0x09 => n * 7,
        0x04 => n * 16,
        _ => n * 8,
    }
}

impl AccessPointIdentifierMessage {
    /// Encode APIDM per C.S0005-E 3.7.2.3.2.39.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(self.pilot_pn <= 0x01ff, "APIDM PILOT_PN must fit in 9 bits");
        assert!(
            self.config_msg_seq <= 0x3f,
            "APIDM CONFIG_MSG_SEQ must fit in 6 bits"
        );
        assert!(self.asstn_type <= 0b010, "APIDM ASSTN_TYPE is reserved");
        assert!(self.sid <= 0x7fff, "APIDM SID must fit in 15 bits");
        assert!(self.ap_id.len() <= 15, "APIDM AP_ID_LEN must fit in 4 bits");
        assert!(
            self.ap_id_mask as usize <= self.ap_id.len() * 16,
            "APIDM AP_ID_MASK exceeds AP_ID length"
        );
        assert!(
            self.ios_msc_id <= 0x00ff_ffff,
            "APIDM IOS_MSC_ID must fit in 24 bits"
        );
        assert!(
            self.intra_freq_ho_slope.is_none() || self.intra_freq_ho_hys.is_some(),
            "APIDM INTRA_FREQ_HO_SLOPE requires INTRA_FREQ_HO_HYS"
        );
        assert!(
            self.inter_freq_ho_slope.is_none() || self.inter_freq_ho_hys.is_some(),
            "APIDM INTER_FREQ_HO_SLOPE requires INTER_FREQ_HO_HYS"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.asstn_type, 3);
        bs.write_u32(self.sid as u32, 15);
        bs.write_u32(self.nid as u32, 16);
        bs.write_u8(self.ap_id.len() as u8, 4);
        for word in &self.ap_id {
            bs.write_u32(*word as u32, 16);
        }
        bs.write_u8(self.ap_id_mask, 8);
        bs.write_u32(self.ios_msc_id, 24);
        bs.write_u32(self.ios_cell_id as u32, 16);

        bs.write_u8(self.hrpd_acquisition.is_some() as u8, 1);
        if let Some(hrpd) = &self.hrpd_acquisition {
            assert!(hrpd.hrpd_pn <= 0x01ff, "APIDM HRPD_PN must fit in 9 bits");
            assert!(
                hrpd.hrpd_band_class <= 0x1f,
                "APIDM HRPD_BAND_CLASS must fit in 5 bits"
            );
            assert!(
                hrpd.hrpd_channel <= 0x07ff,
                "APIDM HRPD_CHANNEL must fit in 11 bits"
            );
            bs.write_u32(hrpd.hrpd_pn as u32, 9);
            bs.write_u8(hrpd.hrpd_band_class, 5);
            bs.write_u32(hrpd.hrpd_channel as u32, 11);
        }

        self.location.write_to(&mut bs);
        write_optional_u8(&mut bs, self.intra_freq_ho_hys, 7);
        write_optional_u8(&mut bs, self.intra_freq_ho_slope, 6);
        write_optional_u8(&mut bs, self.inter_freq_ho_hys, 7);
        write_optional_u8(&mut bs, self.inter_freq_ho_slope, 6);
        write_optional_u8(&mut bs, self.inter_freq_srch_th, 5);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let asstn_type = bs.read_bits(3)? as u8;
        if asstn_type > 0b010 {
            return Err(format!("APIDM reserved ASSTN_TYPE 0b{asstn_type:03b}").into());
        }
        let sid = bs.read_bits(15)? as u16;
        let nid = bs.read_bits(16)? as u16;
        let ap_id_len = bs.read_bits(4)? as usize;
        let mut ap_id = Vec::with_capacity(ap_id_len);
        for _ in 0..ap_id_len {
            ap_id.push(bs.read_bits(16)? as u16);
        }
        let ap_id_mask = bs.read_bits(8)? as u8;
        if ap_id_mask as usize > ap_id_len * 16 {
            return Err("APIDM AP_ID_MASK exceeds AP_ID length".into());
        }
        let ios_msc_id = bs.read_bits(24)? as u32;
        let ios_cell_id = bs.read_bits(16)? as u16;

        let hrpd_acquisition = if bs.read_bits(1)? != 0 {
            Some(AccessPointHrpdAcquisitionRecord {
                hrpd_pn: bs.read_bits(9)? as u16,
                hrpd_band_class: bs.read_bits(5)? as u8,
                hrpd_channel: bs.read_bits(11)? as u16,
            })
        } else {
            None
        };

        let location = AccessPointLocationRecord::read_from(bs)?;
        let intra_freq_ho_hys = read_optional_u8(bs, 7)?;
        let intra_freq_ho_slope = read_optional_u8(bs, 6)?;
        if intra_freq_ho_slope.is_some() && intra_freq_ho_hys.is_none() {
            return Err("APIDM INTRA_FREQ_HO_SLOPE requires INTRA_FREQ_HO_HYS".into());
        }
        let inter_freq_ho_hys = read_optional_u8(bs, 7)?;
        let inter_freq_ho_slope = read_optional_u8(bs, 6)?;
        if inter_freq_ho_slope.is_some() && inter_freq_ho_hys.is_none() {
            return Err("APIDM INTER_FREQ_HO_SLOPE requires INTER_FREQ_HO_HYS".into());
        }
        let inter_freq_srch_th = read_optional_u8(bs, 5)?;

        if !bs.is_empty() {
            return Err("APIDM has trailing bits after INTER_FREQ_SRCH_TH".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            asstn_type,
            sid,
            nid,
            ap_id,
            ap_id_mask,
            ios_msc_id,
            ios_cell_id,
            hrpd_acquisition,
            location,
            intra_freq_ho_hys,
            intra_freq_ho_slope,
            inter_freq_ho_hys,
            inter_freq_ho_slope,
            inter_freq_srch_th,
        })
    }
}

impl AccessPointLocationRecord {
    fn write_to(&self, bs: &mut Bitstream) {
        match self {
            Self::None => {
                bs.write_u8(0, 3);
                bs.write_u8(0, 5);
            }
            Self::BaseStation {
                base_lat,
                base_long,
                loc_unc_h,
                base_height,
                loc_unc_v,
            } => {
                assert!(
                    (-1_296_000..=1_296_000).contains(base_lat),
                    "APIDM BASE_LAT out of C.S0005-E range"
                );
                assert!(
                    (-2_592_000..=2_592_000).contains(base_long),
                    "APIDM BASE_LONG out of C.S0005-E range"
                );
                assert!(*loc_unc_h <= 0x0f, "APIDM LOC_UNC_H must fit in 4 bits");
                assert!(
                    *base_height <= 0x3fff,
                    "APIDM BASE_HEIGHT must fit in 14 bits"
                );
                assert!(*loc_unc_v <= 0x0f, "APIDM LOC_UNC_V must fit in 4 bits");

                bs.write_u8(1, 3);
                bs.write_u8(9, 5);
                bs.write_u32(encode_signed_i32_nbits(*base_lat, 22), 22);
                bs.write_u32(encode_signed_i32_nbits(*base_long, 23), 23);
                bs.write_u8(*loc_unc_h, 4);
                bs.write_u32(*base_height as u32, 14);
                bs.write_u8(*loc_unc_v, 4);
                bs.write_u8(0, 5);
            }
        }
    }

    fn read_from(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let loc_rec_type = bs.read_bits(3)? as u8;
        let loc_rec_len = bs.read_bits(5)? as usize;
        match loc_rec_type {
            0 => {
                if loc_rec_len != 0 {
                    return Err("APIDM LOC_REC_TYPE 0 requires LOC_REC_LEN 0".into());
                }
                Ok(Self::None)
            }
            1 => {
                if loc_rec_len != 9 {
                    return Err("APIDM LOC_REC_TYPE 1 requires LOC_REC_LEN 9".into());
                }
                if bs.len() < loc_rec_len * 8 {
                    return Err("APIDM LOC_REC length exceeds remaining SDU".into());
                }
                let base_lat = decode_signed_i32_nbits(bs.read_bits(22)? as u32, 22);
                if !(-1_296_000..=1_296_000).contains(&base_lat) {
                    return Err("APIDM BASE_LAT out of C.S0005-E range".into());
                }
                let base_long = decode_signed_i32_nbits(bs.read_bits(23)? as u32, 23);
                if !(-2_592_000..=2_592_000).contains(&base_long) {
                    return Err("APIDM BASE_LONG out of C.S0005-E range".into());
                }
                let loc_unc_h = bs.read_bits(4)? as u8;
                let base_height = bs.read_bits(14)? as u16;
                let loc_unc_v = bs.read_bits(4)? as u8;
                let reserved = bs.read_bits(5)?;
                if reserved != 0 {
                    return Err("APIDM LOC_REC reserved bits must be zero".into());
                }
                Ok(Self::BaseStation {
                    base_lat,
                    base_long,
                    loc_unc_h,
                    base_height,
                    loc_unc_v,
                })
            }
            _ => Err(format!("APIDM reserved LOC_REC_TYPE 0b{loc_rec_type:03b}").into()),
        }
    }
}

fn write_optional_u8(bs: &mut Bitstream, value: Option<u8>, bits: usize) {
    bs.write_u8(value.is_some() as u8, 1);
    if let Some(value) = value {
        assert!(
            value < (1u8 << bits),
            "optional field must fit in {bits} bits"
        );
        bs.write_u8(value, bits);
    }
}

fn read_optional_u8(bs: &mut Bitstream, bits: usize) -> Result<Option<u8>, crate::error::Error> {
    Ok(if bs.read_bits(1)? != 0 {
        Some(bs.read_bits(bits)? as u8)
    } else {
        None
    })
}

impl AccessPointPilotInformationMessage {
    /// Encode APPIM per C.S0005-E 3.7.2.3.2.41.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(self.pilot_pn <= 0x01ff, "APPIM PILOT_PN must fit in 9 bits");
        assert!(
            self.config_msg_seq <= 0x3f,
            "APPIM CONFIG_MSG_SEQ must fit in 6 bits"
        );
        assert!(
            self.records.len() <= 0x01ff,
            "APPIM NUM_APPI_REC must fit in 9 bits"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u32(self.lifetime as u32, 16);
        bs.write_u32(self.records.len() as u32, 9);

        let mut previous: Option<&AccessPointPilotInformationRecord> = None;
        for record in &self.records {
            record.write_to(&mut bs, previous);
            previous = Some(record);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let lifetime = bs.read_bits(16)? as u16;
        let num_appi_rec = bs.read_bits(9)? as usize;
        let mut records = Vec::with_capacity(num_appi_rec);
        let mut previous: Option<AccessPointPilotInformationRecord> = None;
        for index in 0..num_appi_rec {
            let record =
                AccessPointPilotInformationRecord::read_from(bs, previous.as_ref(), index)?;
            previous = Some(record.clone());
            records.push(record);
        }
        if !bs.is_empty() {
            return Err("APPIM has trailing bits after APPI_REC records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            lifetime,
            records,
        })
    }
}

impl AccessPointPilotInformationRecord {
    fn write_to(&self, bs: &mut Bitstream, previous: Option<&Self>) {
        assert!(
            self.ap_assn_type <= 0b010 || self.ap_assn_type == 0b111,
            "APPIM AP_ASSN_TYPE is reserved"
        );
        assert!(self.sid <= 0x7fff, "APPIM AP_SID must fit in 15 bits");
        assert!(self.band <= 0x1f, "APPIM AP_BAND must fit in 5 bits");
        assert!(self.freq <= 0x07ff, "APPIM AP_FREQ must fit in 11 bits");

        let sid_same = previous.is_some_and(|record| record.sid == self.sid);
        let nid_same = previous.is_some_and(|record| record.nid == self.nid);
        let band_same = previous.is_some_and(|record| record.band == self.band);
        let freq_same = previous.is_some_and(|record| record.freq == self.freq);
        let pn_same = previous.is_some_and(|record| record.pn_record == self.pn_record);

        let mut record_bits = 8usize;
        bs.write_u8(self.ap_assn_type, 3);
        bs.write_u8(sid_same as u8, 1);
        bs.write_u8(nid_same as u8, 1);
        bs.write_u8(band_same as u8, 1);
        bs.write_u8(freq_same as u8, 1);
        bs.write_u8(pn_same as u8, 1);
        if !sid_same {
            bs.write_u32(self.sid as u32, 15);
            record_bits += 15;
        }
        if !nid_same {
            bs.write_u32(self.nid as u32, 16);
            record_bits += 16;
        }
        if !band_same {
            bs.write_u8(self.band, 5);
            record_bits += 5;
        }
        if !freq_same {
            bs.write_u32(self.freq as u32, 11);
            record_bits += 11;
        }
        if !pn_same {
            let (record_type, record_body) = self.pn_record.to_record_body();
            let record_len = record_body.len() / 8;
            assert!(record_len <= 31, "APPIM AP_PN_REC_LEN must fit in 5 bits");
            bs.write_u8(record_type, 3);
            bs.write_u8(record_len as u8, 5);
            bs.extend(&record_body);
            record_bits += 3 + 5 + record_len * 8;
        }

        let reserved_bits = (8 - (record_bits % 8)) % 8;
        if reserved_bits != 0 {
            bs.write_u8(0, reserved_bits);
        }
    }

    fn read_from(
        bs: &mut Bitstream,
        previous: Option<&Self>,
        index: usize,
    ) -> Result<Self, crate::error::Error> {
        let mut record_bits = 8usize;
        let ap_assn_type = bs.read_bits(3)? as u8;
        if ap_assn_type > 0b010 && ap_assn_type != 0b111 {
            return Err(format!("APPIM reserved AP_ASSN_TYPE 0b{ap_assn_type:03b}").into());
        }
        let sid_same = bs.read_bits(1)? != 0;
        let nid_same = bs.read_bits(1)? != 0;
        let band_same = bs.read_bits(1)? != 0;
        let freq_same = bs.read_bits(1)? != 0;
        let pn_same = bs.read_bits(1)? != 0;

        let sid = if sid_same {
            previous
                .map(|record| record.sid)
                .ok_or_else(|| format!("APPIM AP_SID same-as-previous set in record {index}"))?
        } else {
            record_bits += 15;
            bs.read_bits(15)? as u16
        };
        let nid = if nid_same {
            previous
                .map(|record| record.nid)
                .ok_or_else(|| format!("APPIM AP_NID same-as-previous set in record {index}"))?
        } else {
            record_bits += 16;
            bs.read_bits(16)? as u16
        };
        let band = if band_same {
            previous
                .map(|record| record.band)
                .ok_or_else(|| format!("APPIM AP_BAND same-as-previous set in record {index}"))?
        } else {
            record_bits += 5;
            bs.read_bits(5)? as u8
        };
        let freq = if freq_same {
            previous
                .map(|record| record.freq)
                .ok_or_else(|| format!("APPIM AP_FREQ same-as-previous set in record {index}"))?
        } else {
            record_bits += 11;
            bs.read_bits(11)? as u16
        };
        let pn_record = if pn_same {
            previous
                .map(|record| record.pn_record.clone())
                .ok_or_else(|| format!("APPIM AP_PN_REC same-as-previous set in record {index}"))?
        } else {
            record_bits += 8;
            let record_type = bs.read_bits(3)? as u8;
            let record_len = bs.read_bits(5)? as usize;
            if bs.len() < record_len * 8 {
                return Err("APPIM AP_PN_REC length exceeds remaining SDU".into());
            }
            let mut record_body = bs.drain(0..record_len * 8);
            let pn_record = AccessPointPilotPnRecord::read_from(record_type, &mut record_body)?;
            record_bits += record_len * 8;
            pn_record
        };

        let reserved_bits = (8 - (record_bits % 8)) % 8;
        read_zero_padding(bs, reserved_bits, "APPIM APPI_REC reserved bits")?;

        Ok(Self {
            ap_assn_type,
            sid,
            nid,
            band,
            freq,
            pn_record,
        })
    }
}

impl AccessPointPilotPnRecord {
    fn to_record_body(&self) -> (u8, Bitstream) {
        let mut body = Bitstream::new();
        match self {
            Self::List { pns } => {
                assert!(pns.len() <= 127, "APPIM AP_PN_COUNT must fit in 7 bits");
                body.write_u8(pns.len() as u8, 7);
                for pn in pns {
                    assert!(*pn <= 0x01ff, "APPIM AP_PN must fit in 9 bits");
                    body.write_u32(*pn as u32, 9);
                }
                pad_to_octet(&mut body);
                (0, body)
            }
            Self::Series { count, start, inc } => {
                assert!(*start <= 0x01ff, "APPIM AP_PN_START must fit in 9 bits");
                assert!(*inc <= 0x0f, "APPIM AP_PN_INC must fit in 4 bits");
                body.write_u8(*count, 8);
                body.write_u32(*start as u32, 9);
                body.write_u8(*inc, 4);
                pad_to_octet(&mut body);
                (1, body)
            }
        }
    }

    fn read_from(record_type: u8, body: &mut Bitstream) -> Result<Self, crate::error::Error> {
        match record_type {
            0 => {
                let count = body.read_bits(7)? as usize;
                let mut pns = Vec::with_capacity(count);
                for _ in 0..count {
                    pns.push(body.read_bits(9)? as u16);
                }
                if body.len() >= 8 {
                    return Err("APPIM AP_PN_REC_LEN exceeds list record body".into());
                }
                let reserved_bits = body.len();
                read_zero_padding(body, reserved_bits, "APPIM AP_PN_REC reserved bits")?;
                Ok(Self::List { pns })
            }
            1 => {
                let count = body.read_bits(8)? as u8;
                let start = body.read_bits(9)? as u16;
                let inc = body.read_bits(4)? as u8;
                if body.len() >= 8 {
                    return Err("APPIM AP_PN_REC_LEN exceeds series record body".into());
                }
                let reserved_bits = body.len();
                read_zero_padding(body, reserved_bits, "APPIM AP_PN_REC reserved bits")?;
                Ok(Self::Series { count, start, inc })
            }
            _ => Err(format!("APPIM reserved AP_PN_REC_TYPE 0b{record_type:03b}").into()),
        }
    }
}

impl FlexDuplexCdmaChannelListMessage {
    /// Encode FDCCLM per C.S0005-E 3.7.2.3.2.43 on the Paging Channel.
    pub fn to_sdu(&self) -> Bitstream {
        let rc_qpch_sel_incl = self.rc_qpch_sel_incl();
        let cdma_freq_weight_incl = self.cdma_freq_weight_incl();
        self.validate(rc_qpch_sel_incl, cdma_freq_weight_incl);

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.cand_band_info_req as u8, 1);
        bs.write_u8(rc_qpch_sel_incl as u8, 1);
        bs.write_u8(0, 1); // TD_SEL_INCL must be 0 on the Paging Channel.
        bs.write_u8(cdma_freq_weight_incl as u8, 1);
        bs.write_u8((self.candidate_bands.len() - 1) as u8, 3);
        for band in &self.candidate_bands {
            bs.write_u8(band.cand_band_class, 5);
            if self.cand_band_info_req {
                if let Some(subclasses) = &band.subclasses {
                    bs.write_u8(1, 1);
                    bs.write_u8((subclasses.len() - 1) as u8, 5);
                    GeneralNeighborListMessage::write_bool_slice(&mut bs, subclasses);
                } else {
                    bs.write_u8(0, 1);
                }
            }
            bs.write_u8(band.bypass_sys_det_ind as u8, 1);
            bs.write_u8(band.frequencies.len() as u8, 4);
            for freq in &band.frequencies {
                bs.write_u32(freq.cdma_freq as u32, 11);
                bs.write_u8(freq.remaining.is_some() as u8, 1);
                if let Some(remaining) = &freq.remaining {
                    bs.write_u32(remaining.rev_cdma_freq as u32, 11);
                    if rc_qpch_sel_incl {
                        bs.write_u8(remaining.rc_qpch_hash_ind.expect("validated") as u8, 1);
                    }
                    if cdma_freq_weight_incl {
                        bs.write_u8(remaining.cdma_freq_weight.expect("validated"), 3);
                    }
                }
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let cand_band_info_req = bs.read_bits(1)? != 0;
        let rc_qpch_sel_incl = bs.read_bits(1)? != 0;
        if bs.read_bits(1)? != 0 {
            return Err("FDCCLM TD_SEL_INCL must be 0 on the Paging Channel".into());
        }
        let cdma_freq_weight_incl = bs.read_bits(1)? != 0;
        let num_cand_band_class = bs.read_bits(3)? as usize + 1;
        let mut candidate_bands = Vec::with_capacity(num_cand_band_class);
        let mut any_rc_qpch_hash = false;

        for _ in 0..num_cand_band_class {
            let cand_band_class = bs.read_bits(5)? as u8;
            let subclasses = if cand_band_info_req {
                if bs.read_bits(1)? != 0 {
                    let rec_len = bs.read_bits(5)? as usize;
                    Some(GeneralNeighborListMessage::read_bool_vec(bs, rec_len + 1)?)
                } else {
                    None
                }
            } else {
                None
            };
            let bypass_sys_det_ind = bs.read_bits(1)? != 0;
            let num_freq = bs.read_bits(4)? as usize;
            let mut frequencies = Vec::with_capacity(num_freq);
            for _ in 0..num_freq {
                let cdma_freq = bs.read_bits(11)? as u16;
                let remaining = if bs.read_bits(1)? != 0 {
                    let rev_cdma_freq = bs.read_bits(11)? as u16;
                    let rc_qpch_hash_ind = if rc_qpch_sel_incl {
                        let value = bs.read_bits(1)? != 0;
                        any_rc_qpch_hash |= value;
                        Some(value)
                    } else {
                        None
                    };
                    let cdma_freq_weight = if cdma_freq_weight_incl {
                        Some(bs.read_bits(3)? as u8)
                    } else {
                        None
                    };
                    Some(FlexDuplexRemainingFields {
                        rev_cdma_freq,
                        rc_qpch_hash_ind,
                        cdma_freq_weight,
                    })
                } else {
                    None
                };
                frequencies.push(FlexDuplexFrequencyRecord {
                    cdma_freq,
                    remaining,
                });
            }
            candidate_bands.push(FlexDuplexCandidateBand {
                cand_band_class,
                subclasses,
                bypass_sys_det_ind,
                frequencies,
            });
        }

        if rc_qpch_sel_incl && !any_rc_qpch_hash {
            return Err("FDCCLM RC_QPCH_SEL_INCL requires at least one RC_QPCH_HASH_IND".into());
        }
        if !bs.is_empty() {
            return Err("FDCCLM has trailing bits after candidate band records".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            cand_band_info_req,
            candidate_bands,
        })
    }

    fn rc_qpch_sel_incl(&self) -> bool {
        self.candidate_bands.iter().any(|band| {
            band.frequencies.iter().any(|freq| {
                freq.remaining
                    .as_ref()
                    .is_some_and(|remaining| remaining.rc_qpch_hash_ind.is_some())
            })
        })
    }

    fn cdma_freq_weight_incl(&self) -> bool {
        self.candidate_bands.iter().any(|band| {
            band.frequencies.iter().any(|freq| {
                freq.remaining
                    .as_ref()
                    .is_some_and(|remaining| remaining.cdma_freq_weight.is_some())
            })
        })
    }

    fn validate(&self, rc_qpch_sel_incl: bool, cdma_freq_weight_incl: bool) {
        assert!(
            self.pilot_pn <= 0x01ff,
            "FDCCLM PILOT_PN must fit in 9 bits"
        );
        assert!(
            self.config_msg_seq <= 0x3f,
            "FDCCLM CONFIG_MSG_SEQ must fit in 6 bits"
        );
        assert!(
            (1..=8).contains(&self.candidate_bands.len()),
            "FDCCLM NUM_CAND_BAND_CLASS encodes 1..=8 records"
        );
        if rc_qpch_sel_incl {
            assert!(
                self.candidate_bands.iter().any(|band| {
                    band.frequencies.iter().any(|freq| {
                        freq.remaining
                            .as_ref()
                            .and_then(|remaining| remaining.rc_qpch_hash_ind)
                            .unwrap_or(false)
                    })
                }),
                "FDCCLM RC_QPCH_SEL_INCL requires at least one RC_QPCH_HASH_IND"
            );
        }
        if self.cand_band_info_req {
            let query_count: usize = self
                .candidate_bands
                .iter()
                .map(|band| {
                    band.subclasses
                        .as_ref()
                        .map(|subclasses| subclasses.iter().filter(|value| **value).count())
                        .unwrap_or(1)
                })
                .sum();
            assert!(
                query_count <= 16,
                "FDCCLM must not include more than 16 band class/subclass queries"
            );
        }
        for band in &self.candidate_bands {
            assert!(
                band.cand_band_class <= 0x1f,
                "FDCCLM CAND_BAND_CLASS must fit in 5 bits"
            );
            if self.cand_band_info_req {
                if let Some(subclasses) = &band.subclasses {
                    assert!(
                        (1..=32).contains(&subclasses.len()),
                        "FDCCLM SUBCLASS_REC_LEN encodes 1..=32 subclass indicators"
                    );
                }
            } else {
                assert!(
                    band.subclasses.is_none(),
                    "FDCCLM SUBCLASS_INFO_INCL is omitted when CAND_BAND_INFO_REQ=0"
                );
            }
            assert!(
                band.frequencies.len() <= 15,
                "FDCCLM NUM_FREQ must fit in 4 bits"
            );
            for freq in &band.frequencies {
                assert!(
                    freq.cdma_freq <= 0x07ff,
                    "FDCCLM CDMA_FREQ must fit in 11 bits"
                );
                if let Some(remaining) = &freq.remaining {
                    assert!(
                        remaining.rev_cdma_freq <= 0x07ff,
                        "FDCCLM REV_CDMA_FREQ must fit in 11 bits"
                    );
                    assert!(
                        remaining.rc_qpch_hash_ind.is_some() == rc_qpch_sel_incl,
                        "FDCCLM RC_QPCH_HASH_IND presence must match RC_QPCH_SEL_INCL"
                    );
                    assert!(
                        remaining.cdma_freq_weight.is_some() == cdma_freq_weight_incl,
                        "FDCCLM CDMA_FREQ_WEIGHT presence must match CDMA_FREQ_WEIGHT_INCL"
                    );
                    if let Some(weight) = remaining.cdma_freq_weight {
                        assert!(weight <= 0x07, "FDCCLM CDMA_FREQ_WEIGHT must fit in 3 bits");
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BspmCommonContext {
    num_fsch: usize,
    fsch_plcm_scheme_ind: u8,
    num_bcmc_programs: usize,
    registration_req_flag_incl: bool,
    bcmc_on_traffic_sup: bool,
    auth_signature_required: bool,
}

#[derive(Clone, Copy, Debug)]
struct BspmFschContext {
    tdm_structure: bool,
    tdm_super_period_mask_bits: Option<usize>,
    tdm_mega_period_mask_bits: Option<usize>,
    outercode_super_period_mask_bits: Option<usize>,
}

impl BroadcastServiceParametersMessage {
    /// Encode BSPM per C.S0005-E 3.7.2.3.2.38.
    ///
    /// The payload is preserved bit-exact, but is still parsed by the C.S0005-E field
    /// grammar before transmission so invalid length/count/padding combinations cannot be emitted.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(self.pilot_pn <= 0x01ff, "BSPM PILOT_PN must fit in 9 bits");
        assert!(self.bspm_msg_seq <= 0x3f, "BSPM_MSG_SEQ must fit in 6 bits");
        Self::validate_body(&self.body_bits).expect("BSPM body must match C.S0005-E grammar");

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.bspm_msg_seq, 6);
        bs.extend(&self.body_bits);
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let bspm_msg_seq = bs.read_bits(6)? as u8;
        let body_bits = bs.drain(0..bs.len());
        Self::validate_body(&body_bits)?;
        Ok(Self {
            pilot_pn,
            bspm_msg_seq,
            body_bits,
        })
    }

    fn validate_body(body: &Bitstream) -> Result<(), crate::error::Error> {
        let mut bs = body.clone();
        let context = Self::read_common_record(&mut bs)?;
        let mut fsch_contexts = Vec::with_capacity(context.num_fsch);
        for _ in 0..context.num_fsch {
            fsch_contexts.push(Self::read_fsch_record(
                &mut bs,
                context.fsch_plcm_scheme_ind,
            )?);
        }
        for _ in 0..context.num_bcmc_programs {
            Self::read_program_record(&mut bs, &context, &fsch_contexts)?;
        }
        Self::read_bcch_neighbor_records(&mut bs)?;
        if !bs.is_empty() {
            return Err("BSPM has trailing bits after BCCH neighbor records".into());
        }
        Ok(())
    }

    fn read_common_record(bs: &mut Bitstream) -> Result<BspmCommonContext, crate::error::Error> {
        let common_len_octets = bs.read_bits(4)? as usize + 1;
        let mut record = Self::read_len_record(bs, common_len_octets, 4, "BSPM common record")?;
        let _diff_bspm = record.read_bits(1)?;
        let _auto_req_allowed_ind = record.read_bits(1)?;
        let freq_chg_reg_required = record.read_bits(1)? != 0;
        if freq_chg_reg_required {
            let freq_timer_ind = record.read_bits(1)? != 0;
            if freq_timer_ind {
                let freq_chg_reg_timer = record.read_bits(3)? as u8;
                if freq_chg_reg_timer == 0 {
                    return Err("BSPM FREQ_CHG_REG_TIMER value 000 is reserved".into());
                }
            }
        }
        let registration_req_flag_incl = record.read_bits(1)? != 0;
        if registration_req_flag_incl {
            let _registration_req_timer_period = record.read_bits(8)?;
        }
        let bcmc_on_traffic_sup = record.read_bits(1)? != 0;
        let auth_signature_required = record.read_bits(1)? != 0;
        if auth_signature_required {
            let non_default_value_included = record.read_bits(1)? != 0;
            if non_default_value_included {
                let _ach_time_stamp_short_length = record.read_bits(8)?;
                let _time_stamp_long_length = record.read_bits(8)?;
                let _time_stamp_unit = record.read_bits(4)?;
            }
        }
        let num_fsch = record.read_bits(7)? as usize;
        let fsch_plcm_scheme_ind = record.read_bits(2)? as u8;
        if fsch_plcm_scheme_ind == 0b11 {
            return Err("BSPM FSCH_PLCM_SCHEME_IND 0b11 is reserved".into());
        }
        let num_bcmc_programs = record.read_bits(8)? as usize + 1;
        let use_time = record.read_bits(1)? != 0;
        if use_time {
            let _action_time = record.read_bits(6)?;
        }
        let framing_type = record.read_bits(2)? as u8;
        if framing_type > 0b01 {
            return Err("BSPM FRAMING_TYPE 0b10..0b11 is reserved".into());
        }
        if framing_type != 0 {
            let fcs_length = record.read_bits(2)? as u8;
            if fcs_length > 0b01 {
                return Err("BSPM FCS_LENGTH 0b10..0b11 is reserved".into());
            }
        }
        Self::read_remaining_zero(&mut record, "BSPM_COMMON_RECORD_RESERVED")?;

        Ok(BspmCommonContext {
            num_fsch,
            fsch_plcm_scheme_ind,
            num_bcmc_programs,
            registration_req_flag_incl,
            bcmc_on_traffic_sup,
            auth_signature_required,
        })
    }

    fn read_fsch_record(
        bs: &mut Bitstream,
        plcm_scheme: u8,
    ) -> Result<BspmFschContext, crate::error::Error> {
        let len_octets = bs.read_bits(4)? as usize + 1;
        let mut record = Self::read_len_record(bs, len_octets, 4, "BSPM FSCH record")?;
        if record.read_bits(1)? != 0 {
            let _fsch_band_class = record.read_bits(5)?;
        }
        if record.read_bits(1)? != 0 {
            let _fsch_cdma_freq = record.read_bits(11)?;
        }
        let _fsch_code_chan = record.read_bits(11)?;
        let fsch_plcm_ind = if plcm_scheme == 0b10 {
            Some(record.read_bits(1)? != 0)
        } else {
            None
        };
        if plcm_scheme == 0b01 || fsch_plcm_ind == Some(true) {
            let _fsch_plcm_index = record.read_bits(8)?;
        }
        let _fsch_mux_option = record.read_bits(16)?;
        let _fsch_rc = record.read_bits(5)?;
        let _fsch_coding = record.read_bits(1)?;
        let outercode_incl = record.read_bits(1)? != 0;
        let outercode_super_period_mask_bits = if outercode_incl {
            let rate = record.read_bits(3)? as u8;
            if rate > 0b011 {
                return Err("BSPM FSCH_OUTERCODE_RATE 0b100..0b111 is reserved".into());
            }
            let _fsch_outercode_offset = record.read_bits(6)?;
            Some(11 + rate as usize)
        } else {
            None
        };
        let _fsch_num_bits_idx = record.read_bits(4)?;
        let frame_40_used = record.read_bits(1)? != 0;
        let frame_80_used = record.read_bits(1)? != 0;
        if frame_40_used && frame_80_used {
            return Err("BSPM FSCH_FRAME_40_USED and FSCH_FRAME_80_USED cannot both be 1".into());
        }
        let tdm_structure = record.read_bits(1)? != 0;
        let (tdm_super_period_mask_bits, tdm_mega_period_mask_bits) =
            if tdm_structure && !outercode_incl {
                let slot_length = record.read_bits(2)? as u8;
                if slot_length == 0b11 {
                    return Err("BSPM TDM_SLOT_LENGTH 0b11 is reserved".into());
                }
                let super_len = Self::read_tdm_mask_len(&mut record, "TDM_SUPER_PERIOD_MASK_LEN")?;
                (Some(super_len), None)
            } else if tdm_structure {
                let mega_len = Self::read_tdm_mask_len(&mut record, "TDM_MEGA_PERIOD_MASK_LEN")?;
                (None, Some(mega_len))
            } else {
                (None, None)
            };
        Self::read_remaining_zero(&mut record, "FSCH_RECORD_RESERVED")?;
        Ok(BspmFschContext {
            tdm_structure,
            tdm_super_period_mask_bits,
            tdm_mega_period_mask_bits,
            outercode_super_period_mask_bits,
        })
    }

    fn read_program_record(
        bs: &mut Bitstream,
        context: &BspmCommonContext,
        fsch_contexts: &[BspmFschContext],
    ) -> Result<(), crate::error::Error> {
        let program_id_len = bs.read_bits(5)? as usize + 1;
        let _program_id = bs.read_bits(program_id_len)?;
        let discriminator_len = bs.read_bits(3)? as usize;
        let flow_count = if discriminator_len == 0 {
            1
        } else {
            bs.read_bits(discriminator_len)? as usize + 1
        };
        for flow_index in 0..flow_count {
            let num_lpm_entries =
                Self::read_flow_header(bs, context, discriminator_len, flow_index)?;
            for _ in 0..num_lpm_entries {
                Self::read_lpm_entry(bs, fsch_contexts)?;
            }
        }
        Ok(())
    }

    fn read_flow_header(
        bs: &mut Bitstream,
        context: &BspmCommonContext,
        discriminator_len: usize,
        flow_index: usize,
    ) -> Result<usize, crate::error::Error> {
        let len_octets = bs.read_bits(4)? as usize + 1;
        let mut record = Self::read_len_record(
            bs,
            len_octets,
            4,
            "BSPM BCMC flow discriminator header record",
        )?;
        if discriminator_len != 0 {
            let _flow_discriminator = record.read_bits(discriminator_len)?;
        }
        let flow_info_on_other_freq = record.read_bits(1)? != 0;
        let num_lpm_entries = if flow_info_on_other_freq {
            let same_as_prev = record.read_bits(1)? != 0;
            if same_as_prev && flow_index == 0 {
                return Err("BSPM BSPM_CDMA_FREQ_SAME_AS_PREV cannot be set on first flow".into());
            }
            if !same_as_prev {
                let _bspm_band_class = record.read_bits(5)?;
                let _bspm_cdma_freq = record.read_bits(11)?;
            }
            0
        } else {
            if context.registration_req_flag_incl {
                let _registration_req_flag = record.read_bits(1)?;
            }
            if context.auth_signature_required {
                let _auth_signature_req_ind = record.read_bits(1)?;
            }
            if context.bcmc_on_traffic_sup {
                let _bcmc_flow_on_traffic_ind = record.read_bits(1)?;
            }
            record.read_bits(3)? as usize
        };
        Self::read_remaining_zero(
            &mut record,
            "BCMC_FLOW_DISCRIMINATOR_HEADER_RECORD_RESERVED",
        )?;
        Ok(num_lpm_entries)
    }

    fn read_lpm_entry(
        bs: &mut Bitstream,
        fsch_contexts: &[BspmFschContext],
    ) -> Result<(), crate::error::Error> {
        let fsch_id = bs.read_bits(7)? as usize;
        let fsch = fsch_contexts
            .get(fsch_id)
            .ok_or_else(|| format!("BSPM FSCH_ID {fsch_id} exceeds NUM_FSCH"))?;
        if fsch.tdm_structure {
            let tdm_used = bs.read_bits(1)? != 0;
            if tdm_used {
                let _tdm_mask = bs.read_bits(4)?;
                let super_incl = bs.read_bits(1)? != 0;
                if super_incl {
                    let super_bits = fsch
                        .outercode_super_period_mask_bits
                        .or(fsch.tdm_super_period_mask_bits)
                        .ok_or("BSPM missing TDM super period mask length")?;
                    let _tdm_super_period_mask = bs.read_bits(super_bits)?;
                }
                let mega_incl = bs.read_bits(1)? != 0;
                if mega_incl {
                    let mega_bits = if let Some(bits) = fsch.tdm_mega_period_mask_bits {
                        bits
                    } else if super_incl {
                        4
                    } else {
                        8
                    };
                    let _tdm_mega_period_mask = bs.read_bits(mega_bits)?;
                }
            }
        }
        let bsr_id = bs.read_bits(3)? as u8;
        if bsr_id == 0 {
            return Err("BSPM BSR_ID must not be 0".into());
        }
        let num_nghbr = bs.read_bits(6)? as usize;
        for _ in 0..num_nghbr {
            Self::read_neighbor_record(bs)?;
        }
        Ok(())
    }

    fn read_neighbor_record(bs: &mut Bitstream) -> Result<(), crate::error::Error> {
        let len_octets = bs.read_bits(4)? as usize + 1;
        let mut record = Self::read_len_record(bs, len_octets, 4, "BSPM neighbor record")?;
        let _nghbr_pn = record.read_bits(9)?;
        let config = record.read_bits(3)? as u8;
        if config > 0b011 {
            return Err("BSPM NGHBR_BCMC_CONFIG 0b100..0b111 is reserved".into());
        }
        if config == 0b001 {
            let nghbr_bsr_id = record.read_bits(3)? as u8;
            if nghbr_bsr_id == 0 {
                return Err("BSPM NGHBR_BSR_ID must not be 0".into());
            }
            if record.read_bits(1)? != 0 {
                let _nghbr_fsch_band_class = record.read_bits(5)?;
            }
            if record.read_bits(1)? != 0 {
                let _nghbr_fsch_cdma_freq = record.read_bits(11)?;
            }
        }
        if matches!(config, 0b001 | 0b010) && record.read_bits(1)? != 0 {
            let _nghbr_fsch_code_chan = record.read_bits(11)?;
        }
        if config == 0b001 {
            let parms_incl = record.read_bits(1)? != 0;
            if parms_incl {
                let plcm_ind = record.read_bits(1)? != 0;
                if plcm_ind {
                    let _nghbr_fsch_plcm_index = record.read_bits(8)?;
                }
                let _nghbr_fsch_mux_option = record.read_bits(16)?;
                let _nghbr_fsch_rc = record.read_bits(5)?;
                let _nghbr_fsch_coding = record.read_bits(1)?;
                let outercode_incl = record.read_bits(1)? != 0;
                if outercode_incl {
                    let rate = record.read_bits(3)? as u8;
                    if rate > 0b011 {
                        return Err(
                            "BSPM NGHBR_FSCH_OUTERCODE_RATE 0b100..0b111 is reserved".into()
                        );
                    }
                    let _offset = record.read_bits(6)?;
                }
                let _nghbr_fsch_num_bits_idx = record.read_bits(4)?;
                let frame_40 = record.read_bits(1)? != 0;
                let frame_80 = record.read_bits(1)? != 0;
                if frame_40 && frame_80 {
                    return Err(
                        "BSPM NGHBR_FSCH_FRAME_40_USED and FRAME_80_USED cannot both be 1".into(),
                    );
                }
            }
        }
        Self::read_remaining_zero(&mut record, "NGHBR_RECORD_RESERVED")?;
        Ok(())
    }

    fn read_bcch_neighbor_records(bs: &mut Bitstream) -> Result<(), crate::error::Error> {
        let count = bs.read_bits(3)? as usize;
        for _ in 0..count {
            let _pn = bs.read_bits(9)?;
            let non_td_incl = bs.read_bits(1)? != 0;
            if non_td_incl {
                if bs.read_bits(1)? != 0 {
                    let _non_td_freq = bs.read_bits(11)?;
                }
                let brat = bs.read_bits(2)? as u8;
                if brat == 0b11 {
                    return Err("BSPM BCMC_SR1_BRAT_NON_TD 0b11 is reserved".into());
                }
                let _crat = bs.read_bits(1)?;
                let _code_chan = bs.read_bits(6)?;
            }
            let td_incl = bs.read_bits(1)? != 0;
            if td_incl {
                let _td_freq = bs.read_bits(11)?;
                let brat = bs.read_bits(2)? as u8;
                if brat == 0b11 {
                    return Err("BSPM BCMC_SR1_BRAT_TD 0b11 is reserved".into());
                }
                let _crat = bs.read_bits(1)?;
                let _code_chan = bs.read_bits(6)?;
                let td_mode = bs.read_bits(2)? as u8;
                if td_mode > 0b01 {
                    return Err("BSPM BCMC_SR1_TD_MODE 0b10..0b11 is reserved".into());
                }
                let _td_power_level = bs.read_bits(2)?;
            }
        }
        Ok(())
    }

    fn read_len_record(
        bs: &mut Bitstream,
        len_octets: usize,
        already_read_bits: usize,
        context: &str,
    ) -> Result<Bitstream, crate::error::Error> {
        let bits = len_octets
            .checked_mul(8)
            .and_then(|bits| bits.checked_sub(already_read_bits))
            .ok_or_else(|| format!("{context} length is too short"))?;
        if bs.len() < bits {
            return Err(format!("{context} length exceeds remaining SDU").into());
        }
        Ok(bs.drain(0..bits))
    }

    fn read_tdm_mask_len(bs: &mut Bitstream, context: &str) -> Result<usize, crate::error::Error> {
        match bs.read_bits(2)? as u8 {
            0b00 => Ok(4),
            0b01 => Ok(8),
            0b10 => Ok(16),
            _ => Err(format!("BSPM {context} 0b11 is reserved").into()),
        }
    }

    fn read_remaining_zero(bs: &mut Bitstream, context: &str) -> Result<(), crate::error::Error> {
        if bs.is_empty() {
            return Ok(());
        }
        let remaining = bs.len();
        read_zero_padding(bs, remaining, context)
    }
}

fn read_zero_padding(
    bs: &mut Bitstream,
    bits: usize,
    context: &str,
) -> Result<(), crate::error::Error> {
    if bits == 0 {
        return Ok(());
    }
    if bs.read_bits(bits)? != 0 {
        return Err(format!("{context} must be zero").into());
    }
    Ok(())
}

impl AccessPointIdentifierTextMessage {
    /// Encode APIDTM/APTIDM per C.S0005-E 3.7.2.3.2.40.
    pub fn to_sdu(&self) -> Bitstream {
        assert!(
            self.ap_id_text.len() >= 2,
            "APIDTM AP_ID_TEXT must include MSG_ENCODING and NUM_FIELDS"
        );
        assert!(
            self.ap_id_text.len() <= u8::MAX as usize,
            "APIDTM AP_ID_TEXT_LEN must fit in one octet"
        );

        let mut bs = Bitstream::new();
        bs.write_u32(self.pilot_pn as u32, 9);
        bs.write_u8(self.config_msg_seq, 6);
        bs.write_u8(self.ap_id_text.len() as u8, 8);
        for byte in &self.ap_id_text {
            bs.write_u8(*byte, 8);
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let pilot_pn = bs.read_bits(9)? as u16;
        let config_msg_seq = bs.read_bits(6)? as u8;
        let ap_id_text_len = bs.read_bits(8)? as usize;
        if ap_id_text_len < 2 {
            return Err("APIDTM AP_ID_TEXT must include MSG_ENCODING and NUM_FIELDS".into());
        }
        if bs.len() < ap_id_text_len * 8 {
            return Err("APIDTM AP_ID_TEXT length exceeds remaining SDU".into());
        }
        let mut ap_id_text = Vec::with_capacity(ap_id_text_len);
        for _ in 0..ap_id_text_len {
            ap_id_text.push(bs.read_bits(8)? as u8);
        }
        if !bs.is_empty() {
            return Err("APIDTM has trailing bits after AP_ID_TEXT".into());
        }
        Ok(Self {
            pilot_pn,
            config_msg_seq,
            ap_id_text,
        })
    }

    pub fn text_fields(&self) -> Result<CdmaTextFields, crate::error::Error> {
        decode_cdma_text_fields(&self.ap_id_text, "APIDTM AP_ID_TEXT")
    }
}

impl OrderMessage {
    /// Interpret f-csch/f-dsch order-specific fields for finite C.S0005-E
    /// forward orders whose bodies are not arbitrary service/application data.
    pub fn forward_detail(&self) -> Result<ForwardOrderDetail, crate::error::Error> {
        match self.order {
            0b000010 => self.parse_bs_challenge_confirmation(),
            0b010011 => self.parse_service_option_order(true),
            0b010100 => self.parse_service_option_order(false),
            0b010001 if !self.order_specific_fields.is_empty() => {
                self.parse_periodic_pilot_measurement_request()
            }
            0b011010 => self.parse_status_request(),
            0b011011 => self.parse_registration_accepted(),
            0b100000 => self.parse_retry_order(),
            0b100001 if self.ordq == 0b00000010 => self.parse_base_station_reject(),
            _ if self.order_specific_fields.is_empty() => {
                if self.ordq == 0 {
                    Ok(ForwardOrderDetail::NoAdditionalFields { order: self.order })
                } else {
                    Ok(ForwardOrderDetail::QualificationOnly {
                        order: self.order,
                        ordq: self.ordq,
                    })
                }
            }
            _ => Err(format!(
                "typed forward Order detail is not implemented for ORDER=0b{:06b} ({}) ORDQ=0x{:02X} with {} order-specific octets",
                self.order,
                crate::formatting::forward_order_name(self.order),
                self.ordq,
                self.order_specific_fields.len()
            )
            .into()),
        }
    }

    pub fn from_forward_detail(detail: &ForwardOrderDetail) -> Result<Self, crate::error::Error> {
        detail.to_order_message()
    }

    fn parse_bs_challenge_confirmation(&self) -> Result<ForwardOrderDetail, crate::error::Error> {
        ensure_ordq(self, 0, "Base Station Challenge Confirmation")?;
        ensure_order_specific_len(
            self,
            3,
            "Base Station Challenge Confirmation AUTHBS/RESERVED",
        )?;
        let mut bs = Bitstream::new_bytes(&self.order_specific_fields);
        let authbs = bs.read_bits(18)? as u32;
        ensure_reserved_zero(&mut bs, "Base Station Challenge Confirmation RESERVED")?;
        Ok(ForwardOrderDetail::BaseStationChallengeConfirmation { authbs })
    }

    fn parse_service_option_order(
        &self,
        request: bool,
    ) -> Result<ForwardOrderDetail, crate::error::Error> {
        ensure_ordq(
            self,
            0,
            if request {
                "Service Option Request"
            } else {
                "Service Option Response"
            },
        )?;
        ensure_order_specific_len(self, 2, "Service Option ORDER SERVICE_OPTION")?;
        let service_option =
            ((self.order_specific_fields[0] as u16) << 8) | self.order_specific_fields[1] as u16;
        if request {
            Ok(ForwardOrderDetail::ServiceOptionRequest { service_option })
        } else {
            Ok(ForwardOrderDetail::ServiceOptionResponse { service_option })
        }
    }

    fn parse_periodic_pilot_measurement_request(
        &self,
    ) -> Result<ForwardOrderDetail, crate::error::Error> {
        ensure_order_specific_len(
            self,
            2,
            "Periodic Pilot Measurement Request threshold fields",
        )?;
        let mut bs = Bitstream::new_bytes(&self.order_specific_fields);
        let min_pilot_pwr_thresh = bs.read_bits(5)? as u8;
        let min_pilot_ec_i0_thresh = bs.read_bits(5)? as u8;
        let incl_setpt = bs.read_bits(1)? != 0;
        ensure_reserved_zero(&mut bs, "Periodic Pilot Measurement Request RESERVED")?;
        Ok(ForwardOrderDetail::PeriodicPilotMeasurementRequest(
            PeriodicPilotMeasurementRequestOrder {
                ordq: self.ordq,
                min_pilot_pwr_thresh,
                min_pilot_ec_i0_thresh,
                incl_setpt,
            },
        ))
    }

    fn parse_status_request(&self) -> Result<ForwardOrderDetail, crate::error::Error> {
        ensure_no_order_specific_fields(self, "Status Request")?;
        if !matches!(self.ordq, 0x07..=0x0a | 0x0c..=0x0f) {
            return Err(format!("Status Request ORDQ 0x{:02X} is reserved", self.ordq).into());
        }
        Ok(ForwardOrderDetail::StatusRequest {
            information_record_type: self.ordq,
        })
    }

    fn parse_registration_accepted(&self) -> Result<ForwardOrderDetail, crate::error::Error> {
        let detail = match self.ordq {
            0x00 => {
                ensure_no_order_specific_fields(self, "Registration Accepted ORDQ=0")?;
                RegistrationAcceptedOrder {
                    roam_indi: None,
                    c_sig_encrypt_mode: None,
                    enc_key_size: None,
                    msg_int_info_incl: None,
                    change_keys: None,
                    use_uak: None,
                }
            }
            0x05 => {
                ensure_order_specific_len(self, 1, "Registration Accepted ROAM_INDI")?;
                RegistrationAcceptedOrder {
                    roam_indi: Some(self.order_specific_fields[0]),
                    c_sig_encrypt_mode: None,
                    enc_key_size: None,
                    msg_int_info_incl: None,
                    change_keys: None,
                    use_uak: None,
                }
            }
            0x07 => {
                if self.order_specific_fields.len() < 2 || self.order_specific_fields.len() > 3 {
                    return Err(
                        "Registration Accepted ORDQ=0x07 must carry 2 or 3 octets after ORDQ"
                            .into(),
                    );
                }
                let mut bs = Bitstream::new_bytes(&self.order_specific_fields);
                let roam_indi = bs.read_bits(8)? as u8;
                let c_sig_encrypt_mode = bs.read_bits(3)? as u8;
                if c_sig_encrypt_mode > 0b010 {
                    return Err(format!(
                        "Registration Accepted C_SIG_ENCRYPT_MODE {c_sig_encrypt_mode:#05b} is reserved"
                    )
                    .into());
                }
                let enc_key_size = if matches!(c_sig_encrypt_mode, 0b001 | 0b010) {
                    let key_size = bs.read_bits(3)? as u8;
                    if !matches!(key_size, 0b001 | 0b010) {
                        return Err(format!(
                            "Registration Accepted ENC_KEY_SIZE {key_size:#05b} is reserved"
                        )
                        .into());
                    }
                    Some(key_size)
                } else {
                    None
                };
                let msg_int_info_incl = bs.read_bits(1)? != 0;
                let (change_keys, use_uak) = if msg_int_info_incl {
                    (Some(bs.read_bits(1)? != 0), Some(bs.read_bits(1)? != 0))
                } else {
                    (None, None)
                };
                ensure_reserved_zero(&mut bs, "Registration Accepted RESERVED")?;
                RegistrationAcceptedOrder {
                    roam_indi: Some(roam_indi),
                    c_sig_encrypt_mode: Some(c_sig_encrypt_mode),
                    enc_key_size,
                    msg_int_info_incl: Some(msg_int_info_incl),
                    change_keys,
                    use_uak,
                }
            }
            _ => {
                return Err(format!(
                    "Registration Accepted ORDQ 0x{:02X} is not a Registration Accepted variant",
                    self.ordq
                )
                .into());
            }
        };
        Ok(ForwardOrderDetail::RegistrationAccepted(detail))
    }

    fn parse_retry_order(&self) -> Result<ForwardOrderDetail, crate::error::Error> {
        ensure_ordq(self, 0, "Retry")?;
        if self.order_specific_fields.is_empty() || self.order_specific_fields.len() > 2 {
            return Err("Retry Order must carry 1 or 2 octets after ORDQ".into());
        }
        let mut bs = Bitstream::new_bytes(&self.order_specific_fields);
        let retry_type = bs.read_bits(3)? as u8;
        if retry_type > 0b101 {
            return Err(format!("Retry Order RETRY_TYPE {retry_type:#05b} is reserved").into());
        }
        let retry_delay = if retry_type == 0 {
            if self.order_specific_fields.len() != 1 {
                return Err("Retry Order RETRY_TYPE=000 must omit RETRY_DELAY".into());
            }
            None
        } else {
            if self.order_specific_fields.len() != 2 {
                return Err("Retry Order RETRY_TYPE != 000 must include RETRY_DELAY".into());
            }
            Some(bs.read_bits(8)? as u8)
        };
        ensure_reserved_zero(&mut bs, "Retry Order RESERVED")?;
        Ok(ForwardOrderDetail::Retry(RetryOrder {
            retry_type,
            retry_delay,
        }))
    }

    fn parse_base_station_reject(&self) -> Result<ForwardOrderDetail, crate::error::Error> {
        ensure_order_specific_len(self, 2, "Base Station Reject reason/message fields")?;
        let mut bs = Bitstream::new_bytes(&self.order_specific_fields);
        let reject_reason = bs.read_bits(4)? as u8;
        if reject_reason > 0b0011 {
            return Err(format!(
                "Base Station Reject REJECT_REASON {reject_reason:#06b} is reserved"
            )
            .into());
        }
        let rejected_msg_type = bs.read_bits(8)? as u8;
        let rejected_msg_seq = bs.read_bits(3)? as u8;
        ensure_reserved_zero(&mut bs, "Base Station Reject RESERVED")?;
        Ok(ForwardOrderDetail::BaseStationReject(
            BaseStationRejectOrder {
                reject_reason,
                rejected_msg_type,
                rejected_msg_seq,
            },
        ))
    }

    /// Encode the f-csch Order Message SDU per C.S0005-E 3.7.2.3.2.7:
    /// ORDER(6) + ADD_RECORD_LEN(3) + [ORDQ(8) + order-specific fields if ADD_RECORD_LEN > 0].
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u8(self.order, 6);
        if self.ordq == 0 && self.order_specific_fields.is_empty() {
            bs.write_u8(0, 3); // ADD_RECORD_LEN = 0, no ORDQ written
        } else {
            let add_record_len = 1 + self.order_specific_fields.len();
            assert!(
                add_record_len <= 7,
                "Order ADD_RECORD_LEN must fit in 3 bits"
            );
            bs.write_u8(add_record_len as u8, 3);
            bs.write_u8(self.ordq, 8);
            for byte in &self.order_specific_fields {
                bs.write_u8(*byte, 8);
            }
        }
        bs
    }

    pub fn from_sdu(bs: &mut Bitstream) -> Result<Self, crate::error::Error> {
        let order = bs.read_bits(6)? as u8;
        let add_record_len = bs.read_bits(3)? as usize;
        let (ordq, order_specific_fields) = if add_record_len > 0 {
            if bs.len() < add_record_len * 8 {
                return Err("Order ADD_RECORD_LEN exceeds remaining SDU".into());
            }
            let ordq = bs.read_bits(8)? as u8;
            let mut order_specific_fields = Vec::with_capacity(add_record_len.saturating_sub(1));
            if add_record_len > 1 {
                for _ in 1..add_record_len {
                    order_specific_fields.push(bs.read_bits(8)? as u8);
                }
            }
            (ordq, order_specific_fields)
        } else {
            (0, Vec::new())
        };
        Ok(Self {
            order,
            ordq,
            order_specific_fields,
        })
    }

    /// Encode the f-dsch Order Message SDU per C.S0004-E 3.2.2.1.1.2:
    /// USE_TIME(1) + ACTION_TIME(6) + ORDER(6) + ADD_RECORD_LEN(3)
    /// + [ORDQ(8) + order-specific fields if ADD_RECORD_LEN > 0].
    pub fn to_ftch_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u8(0, 1); // USE_TIME = 0
        bs.write_u8(0, 6); // ACTION_TIME = 000000
        bs.write_u8(self.order, 6);
        if self.ordq == 0 && self.order_specific_fields.is_empty() {
            bs.write_u8(0, 3); // ADD_RECORD_LEN = 0
        } else {
            let add_record_len = 1 + self.order_specific_fields.len();
            assert!(
                add_record_len <= 7,
                "Order ADD_RECORD_LEN must fit in 3 bits"
            );
            bs.write_u8(add_record_len as u8, 3);
            bs.write_u8(self.ordq, 8);
            for byte in &self.order_specific_fields {
                bs.write_u8(*byte, 8);
            }
        }
        bs
    }
}

impl ForwardOrderDetail {
    pub fn to_order_message(&self) -> Result<OrderMessage, crate::error::Error> {
        match self {
            ForwardOrderDetail::NoAdditionalFields { order } => Ok(OrderMessage {
                order: *order,
                ordq: 0,
                order_specific_fields: Vec::new(),
            }),
            ForwardOrderDetail::QualificationOnly { order, ordq } => Ok(OrderMessage {
                order: *order,
                ordq: *ordq,
                order_specific_fields: Vec::new(),
            }),
            ForwardOrderDetail::BaseStationChallengeConfirmation { authbs } => {
                if *authbs >= (1 << 18) {
                    return Err("Base Station Challenge Confirmation AUTHBS exceeds 18 bits".into());
                }
                let mut bs = Bitstream::new();
                bs.write_u32(*authbs, 18);
                bs.write_u8(0, 6);
                Ok(OrderMessage {
                    order: 0b000010,
                    ordq: 0,
                    order_specific_fields: bs.to_packed_bytes(),
                })
            }
            ForwardOrderDetail::ServiceOptionRequest { service_option } => Ok(OrderMessage {
                order: 0b010011,
                ordq: 0,
                order_specific_fields: service_option.to_be_bytes().to_vec(),
            }),
            ForwardOrderDetail::ServiceOptionResponse { service_option } => Ok(OrderMessage {
                order: 0b010100,
                ordq: 0,
                order_specific_fields: service_option.to_be_bytes().to_vec(),
            }),
            ForwardOrderDetail::StatusRequest {
                information_record_type,
            } => {
                if !matches!(*information_record_type, 0x07..=0x0a | 0x0c..=0x0f) {
                    return Err(format!(
                        "Status Request ORDQ 0x{information_record_type:02X} is reserved"
                    )
                    .into());
                }
                Ok(OrderMessage {
                    order: 0b011010,
                    ordq: *information_record_type,
                    order_specific_fields: Vec::new(),
                })
            }
            ForwardOrderDetail::RegistrationAccepted(detail) => {
                encode_registration_accepted_order(detail)
            }
            ForwardOrderDetail::Retry(detail) => encode_retry_order(detail),
            ForwardOrderDetail::BaseStationReject(detail) => {
                encode_base_station_reject_order(detail)
            }
            ForwardOrderDetail::PeriodicPilotMeasurementRequest(detail) => {
                encode_periodic_pilot_measurement_request_order(detail)
            }
        }
    }
}

fn encode_registration_accepted_order(
    detail: &RegistrationAcceptedOrder,
) -> Result<OrderMessage, crate::error::Error> {
    match (
        detail.roam_indi,
        detail.c_sig_encrypt_mode,
        detail.msg_int_info_incl,
    ) {
        (None, None, None) => Ok(OrderMessage {
            order: 0b011011,
            ordq: 0,
            order_specific_fields: Vec::new(),
        }),
        (Some(roam_indi), None, None) => Ok(OrderMessage {
            order: 0b011011,
            ordq: 0x05,
            order_specific_fields: vec![roam_indi],
        }),
        (Some(roam_indi), Some(c_sig_encrypt_mode), Some(msg_int_info_incl)) => {
            if c_sig_encrypt_mode > 0b010 {
                return Err(format!(
                    "Registration Accepted C_SIG_ENCRYPT_MODE {c_sig_encrypt_mode:#05b} is reserved"
                )
                .into());
            }
            let enc_key_size = if matches!(c_sig_encrypt_mode, 0b001 | 0b010) {
                let key_size = detail
                    .enc_key_size
                    .ok_or("Registration Accepted encryption modes 001/010 require ENC_KEY_SIZE")?;
                if !matches!(key_size, 0b001 | 0b010) {
                    return Err(format!(
                        "Registration Accepted ENC_KEY_SIZE {key_size:#05b} is reserved"
                    )
                    .into());
                }
                Some(key_size)
            } else {
                if detail.enc_key_size.is_some() {
                    return Err(
                        "Registration Accepted ENC_KEY_SIZE must be omitted when encryption is disabled"
                            .into(),
                    );
                }
                None
            };
            let (change_keys, use_uak) = if msg_int_info_incl {
                (
                    detail
                        .change_keys
                        .ok_or("Registration Accepted MSG_INT_INFO_INCL=1 requires CHANGE_KEYS")?,
                    detail
                        .use_uak
                        .ok_or("Registration Accepted MSG_INT_INFO_INCL=1 requires USE_UAK")?,
                )
            } else {
                if detail.change_keys.is_some() || detail.use_uak.is_some() {
                    return Err(
                        "Registration Accepted CHANGE_KEYS/USE_UAK require MSG_INT_INFO_INCL=1"
                            .into(),
                    );
                }
                (false, false)
            };

            let mut bs = Bitstream::new();
            bs.write_u8(roam_indi, 8);
            bs.write_u8(c_sig_encrypt_mode, 3);
            if let Some(key_size) = enc_key_size {
                bs.write_u8(key_size, 3);
            }
            bs.write_u8(msg_int_info_incl as u8, 1);
            if msg_int_info_incl {
                bs.write_u8(change_keys as u8, 1);
                bs.write_u8(use_uak as u8, 1);
            }
            pad_reserved_to_octet(&mut bs);
            Ok(OrderMessage {
                order: 0b011011,
                ordq: 0x07,
                order_specific_fields: bs.to_packed_bytes(),
            })
        }
        _ => Err("Registration Accepted fields do not match ORDQ 0x00/0x05/0x07 grammar".into()),
    }
}

fn encode_retry_order(detail: &RetryOrder) -> Result<OrderMessage, crate::error::Error> {
    if detail.retry_type > 0b101 {
        return Err(format!(
            "Retry Order RETRY_TYPE {:#05b} is reserved",
            detail.retry_type
        )
        .into());
    }
    if detail.retry_type == 0 && detail.retry_delay.is_some() {
        return Err("Retry Order RETRY_TYPE=000 must omit RETRY_DELAY".into());
    }
    if detail.retry_type != 0 && detail.retry_delay.is_none() {
        return Err("Retry Order RETRY_TYPE != 000 requires RETRY_DELAY".into());
    }
    let mut bs = Bitstream::new();
    bs.write_u8(detail.retry_type, 3);
    if let Some(delay) = detail.retry_delay {
        bs.write_u8(delay, 8);
    }
    bs.write_u8(0, 5);
    Ok(OrderMessage {
        order: 0b100000,
        ordq: 0,
        order_specific_fields: bs.to_packed_bytes(),
    })
}

fn encode_base_station_reject_order(
    detail: &BaseStationRejectOrder,
) -> Result<OrderMessage, crate::error::Error> {
    if detail.reject_reason > 0b0011 {
        return Err(format!(
            "Base Station Reject REJECT_REASON {:#06b} is reserved",
            detail.reject_reason
        )
        .into());
    }
    if detail.rejected_msg_seq > 0b111 {
        return Err("Base Station Reject REJECTED_MSG_SEQ exceeds 3 bits".into());
    }
    let mut bs = Bitstream::new();
    bs.write_u8(detail.reject_reason, 4);
    bs.write_u8(detail.rejected_msg_type, 8);
    bs.write_u8(detail.rejected_msg_seq, 3);
    bs.write_u8(0, 1);
    Ok(OrderMessage {
        order: 0b100001,
        ordq: 0x02,
        order_specific_fields: bs.to_packed_bytes(),
    })
}

fn encode_periodic_pilot_measurement_request_order(
    detail: &PeriodicPilotMeasurementRequestOrder,
) -> Result<OrderMessage, crate::error::Error> {
    if detail.ordq == 0 {
        return Err("Periodic Pilot Measurement Request ORDQ must be nonzero".into());
    }
    if detail.min_pilot_pwr_thresh > 0b11111 || detail.min_pilot_ec_i0_thresh > 0b11111 {
        return Err("Periodic Pilot Measurement Request threshold exceeds 5 bits".into());
    }
    let mut bs = Bitstream::new();
    bs.write_u8(detail.min_pilot_pwr_thresh, 5);
    bs.write_u8(detail.min_pilot_ec_i0_thresh, 5);
    bs.write_u8(detail.incl_setpt as u8, 1);
    bs.write_u8(0, 5);
    Ok(OrderMessage {
        order: 0b010001,
        ordq: detail.ordq,
        order_specific_fields: bs.to_packed_bytes(),
    })
}

fn ensure_ordq(
    message: &OrderMessage,
    expected: u8,
    name: &str,
) -> Result<(), crate::error::Error> {
    if message.ordq != expected {
        return Err(format!(
            "{name} ORDQ must be 0x{expected:02X}, got 0x{:02X}",
            message.ordq
        )
        .into());
    }
    Ok(())
}

fn ensure_no_order_specific_fields(
    message: &OrderMessage,
    name: &str,
) -> Result<(), crate::error::Error> {
    ensure_order_specific_len(message, 0, name)
}

fn ensure_order_specific_len(
    message: &OrderMessage,
    expected: usize,
    name: &str,
) -> Result<(), crate::error::Error> {
    if message.order_specific_fields.len() != expected {
        return Err(format!(
            "{name} requires {expected} order-specific octets after ORDQ, got {}",
            message.order_specific_fields.len()
        )
        .into());
    }
    Ok(())
}

fn ensure_reserved_zero(bs: &mut Bitstream, name: &str) -> Result<(), crate::error::Error> {
    while !bs.is_empty() {
        if bs.read_bits(1)? != 0 {
            return Err(format!("{name} contains non-zero reserved bits").into());
        }
    }
    Ok(())
}

fn pad_reserved_to_octet(bs: &mut Bitstream) {
    let pad_bits = (8 - (bs.len() % 8)) % 8;
    if pad_bits > 0 {
        bs.write_u8(0, pad_bits);
    }
}

/// Connection record for a Service Connect Message.
#[derive(Debug, Clone)]
pub struct ServiceConnectConnectionRecord {
    pub con_ref: u8,
    pub service_option: u16,
    pub for_traffic: u8,
    pub rev_traffic: u8,
    pub ui_encrypt_mode: u8,
    pub sr_id: u8,
    pub rlp_info_incl: bool,
    pub rlp_blob: Option<Vec<u8>>,
    pub qos_parms: Option<Vec<u8>>,
}

/// Call-assignment entry carried at the end of a Service Connect Message.
#[derive(Debug, Clone)]
pub struct ServiceConnectCallAssignment {
    pub con_ref: u8,
    pub response_ind: bool,
    pub tag: Option<u8>,
    pub bypass_alert_answer: Option<bool>,
}

/// Non-Negotiable Service Configuration record fields per C.S0004-E
/// Table 3.7.2.3.2.21-3 (RECORD_TYPE = 0x13).
#[derive(Debug, Clone)]
pub struct NonNegServiceConfig {
    /// FPC_INCL — include forward power control fields.
    pub fpc_incl: bool,
    /// FPC_PRI_CHAN — primary channel for FPC (0=FCH, 1=DCCH).
    /// Only meaningful when fpc_incl=true.
    pub fpc_pri_chan: u8,
    /// FPC_MODE — forward power control mode (3 bits).
    pub fpc_mode: u8,
    /// Include outer-loop power control for FCH.
    pub fpc_olpc_fch_incl: bool,
    /// FPC_FCH_FER — target frame error rate for FCH (5 bits).
    pub fpc_fch_fer: u8,
    /// FPC_FCH_MIN_SETPT — minimum setpoint (8 bits, 0.125 dB units).
    pub fpc_fch_min_setpt: u8,
    /// FPC_FCH_MAX_SETPT — maximum setpoint (8 bits, 0.125 dB units).
    pub fpc_fch_max_setpt: u8,
    /// Include outer-loop power control for DCCH.
    pub fpc_olpc_dcch_incl: bool,
    /// FPC_DCCH_FER — target FER for DCCH (5 bits).
    pub fpc_dcch_fer: u8,
    /// FPC_DCCH_MIN_SETPT (8 bits).
    pub fpc_dcch_min_setpt: u8,
    /// FPC_DCCH_MAX_SETPT (8 bits).
    pub fpc_dcch_max_setpt: u8,
    /// Include SCH outer-loop power control records.
    /// When true, NUM_SUP and one SCH FER/min/max record are emitted.
    pub fpc_sch_incl: bool,
    /// FPC_SCH_FER — target FER for SCH (5 bits).
    pub fpc_sch_fer: u8,
    /// FPC_SCH_MIN_SETPT (8 bits, 0.125 dB units).
    pub fpc_sch_min_setpt: u8,
    /// FPC_SCH_MAX_SETPT (8 bits, 0.125 dB units).
    pub fpc_sch_max_setpt: u8,
    /// GATING_RATE_INCL — include pilot gating rate.
    pub gating_rate_incl: bool,
    /// PILOT_GATING_RATE (2 bits): 0=gating off, 1=1/2, 2=1/4.
    pub pilot_gating_rate: u8,
    /// LPM_IND — low power mode indicator (2 bits).
    pub lpm_ind: u8,
}

impl NonNegServiceConfig {
    /// Defaults for RC1: no FPC, no pilot gating.
    pub fn rc1_default() -> Self {
        Self {
            fpc_incl: false,
            fpc_pri_chan: 0,
            fpc_mode: 0,
            fpc_olpc_fch_incl: false,
            fpc_fch_fer: 0,
            fpc_fch_min_setpt: 0,
            fpc_fch_max_setpt: 0,
            fpc_olpc_dcch_incl: false,
            fpc_dcch_fer: 0,
            fpc_dcch_min_setpt: 0,
            fpc_dcch_max_setpt: 0,
            fpc_sch_incl: false,
            fpc_sch_fer: 0,
            fpc_sch_min_setpt: 0,
            fpc_sch_max_setpt: 0,
            gating_rate_incl: false,
            pilot_gating_rate: 0,
            lpm_ind: 0,
        }
    }

    /// Sensible defaults for RC3+ with FPC enabled.
    pub fn rc3_default() -> Self {
        Self {
            fpc_incl: true,
            fpc_pri_chan: 0, // FCH
            fpc_mode: 0,
            fpc_olpc_fch_incl: true,
            fpc_fch_fer: 2, // ~1% target FER
            fpc_fch_min_setpt: 0,
            fpc_fch_max_setpt: 80, // 10 dB (80 * 0.125)
            fpc_olpc_dcch_incl: false,
            fpc_dcch_fer: 0,
            fpc_dcch_min_setpt: 0,
            fpc_dcch_max_setpt: 0,
            fpc_sch_incl: false,
            fpc_sch_fer: 0,
            fpc_sch_min_setpt: 0,
            fpc_sch_max_setpt: 0,
            gating_rate_incl: true,
            pilot_gating_rate: 0, // gating off
            lpm_ind: 0,
        }
    }

    /// RC3 F-SCH defaults for SO33 Service Connect.
    pub fn rc3_fsch_default() -> Self {
        Self {
            fpc_sch_incl: true,
            fpc_sch_fer: 0b00010,    // 1% target FER (matches FCH default).
            fpc_sch_min_setpt: 0x00, // 0.0 dB
            fpc_sch_max_setpt: 0x50, // 10.0 dB (80 * 0.125)
            ..Self::rc3_default()
        }
    }

    fn validate(&self) {
        assert!(self.fpc_pri_chan <= 1, "FPC_PRI_CHAN exceeds 1 bit");
        assert!(self.fpc_mode <= 0b111, "FPC_MODE exceeds 3 bits");
        assert!(self.fpc_fch_fer != 0b11111, "FPC_FCH_FER=11111 is reserved");
        assert!(
            self.fpc_dcch_fer != 0b11111,
            "FPC_DCCH_FER=11111 is reserved"
        );
        assert!(self.fpc_sch_fer != 0b11111, "FPC_SCH_FER=11111 is reserved");
        assert!(
            self.fpc_sch_min_setpt <= self.fpc_sch_max_setpt,
            "FPC_SCH_MIN_SETPT must be <= FPC_SCH_MAX_SETPT"
        );
        assert!(
            self.pilot_gating_rate <= 0b11,
            "PILOT_GATING_RATE exceeds 2 bits"
        );
        assert!(self.lpm_ind <= 0b11, "LPM_IND exceeds 2 bits");
    }

    /// Encode to bytes per C.S0004-E Table 3.7.2.3.2.21-3.
    pub fn encode(&self) -> Vec<u8> {
        self.validate();
        let mut bs = Bitstream::new();

        bs.write_u8(self.fpc_incl as u8, 1);
        if self.fpc_incl {
            let mode = self.fpc_mode;
            bs.write_u8(self.fpc_pri_chan, 1);
            bs.write_u8(mode, 3);
            bs.write_u8(self.fpc_olpc_fch_incl as u8, 1);
            if self.fpc_olpc_fch_incl {
                bs.write_u8(self.fpc_fch_fer, 5);
                bs.write_u8(self.fpc_fch_min_setpt, 8);
                bs.write_u8(self.fpc_fch_max_setpt, 8);
            }
            bs.write_u8(self.fpc_olpc_dcch_incl as u8, 1);
            if self.fpc_olpc_dcch_incl {
                bs.write_u8(self.fpc_dcch_fer, 5);
                bs.write_u8(self.fpc_dcch_min_setpt, 8);
                bs.write_u8(self.fpc_dcch_max_setpt, 8);
            }
            if matches!(mode, 0b001 | 0b010 | 0b101 | 0b110) {
                bs.write_u8(0, 1); // FPC_SEC_CHAN
            }
            if self.fpc_sch_incl {
                bs.write_u8(1, 2); // NUM_SUP = one SCH FPC record.
                bs.write_u8(0, 1); // SCH_ID = SCH0.
                bs.write_u8(self.fpc_sch_fer, 5);
                bs.write_u8(self.fpc_sch_min_setpt, 8);
                bs.write_u8(self.fpc_sch_max_setpt, 8);
            } else {
                bs.write_u8(0, 2); // NUM_SUP = 0.
            }
        }

        bs.write_u8(self.gating_rate_incl as u8, 1);
        if self.gating_rate_incl {
            bs.write_u8(self.pilot_gating_rate & 0x03, 2);
        }

        // RESERVED (2 bits)
        bs.write_u8(0, 2);
        // LPM_IND (2 bits)
        bs.write_u8(self.lpm_ind & 0x03, 2);
        // Pad to byte boundary with RESERVED bits
        let remainder = bs.bits().len() % 8;
        if remainder != 0 {
            bs.write_u8(0, 8 - remainder);
        }

        bitstream_to_byte_vec(&bs)
    }
}

/// Service Connect Message parameters for the f-dsch.
///
/// Encodes per C.S0005-E 3.7.3.3.2.20 (Service Connect Message).
/// The SDU (everything after MSG_TYPE + ARQ + ENCRYPTION) contains:
///   USE_TIME(1) + ACTION_TIME(6) + SERV_CON_SEQ(3) + RESERVED(2) +
///   USE_OLD_SERV_CONFIG(2) + SYNC_ID_INCL(1) +
///   optional SCR/NNSCR records +
///   optional CC_INFO_INCL + call-assignment records +
///   USE_TYPE0_PLCM(1).
///
/// Current encoder support is intentionally limited to the normal
/// `USE_OLD_SERV_CONFIG = 00` path used by this codebase today. The stored
/// service configuration restore variants (`01`, `10`, `11`) are only
/// partially represented and are rejected when they would require restore /
/// release semantics we do not implement yet.
/// Forward supplemental channel configuration for Service Connect.
#[derive(Clone, Debug)]
pub struct ForSchConfig {
    /// SCH identifier (0 or 1).
    pub sch_id: u8,
    /// MUX option for SCH (Rate Set 1, MuxPDU Type 3 single).
    pub mux_option: u16,
    /// Radio configuration (3 = RC3).
    pub rc: u8,
    /// Coding type: 0 = convolutional, 1 = turbo.
    pub coding: u8,
    /// Rate index: 0=1x(9.6k), 1=2x(19.2k), 2=4x(38.4k), 3=8x(76.8k), 4=16x(153.6k).
    pub rate: u8,
}

#[derive(Debug, Clone)]
pub struct ServiceConnectParams {
    pub serv_con_seq: u8,
    pub use_old_serv_config: u8,
    pub for_mux_option: u16,
    pub rev_mux_option: u16,
    pub for_rates: u8,
    pub rev_rates: u8,
    pub sync_id: Option<Vec<u8>>,
    pub connections: Vec<ServiceConnectConnectionRecord>,
    pub fch_frame_size: u8,
    pub for_fch_rc: u8,
    pub rev_fch_rc: u8,
    pub call_assignments: Vec<ServiceConnectCallAssignment>,
    pub use_type0_plcm: bool,
    /// Optional Non-Negotiable Service Configuration record (type 0x13).
    pub non_neg: Option<NonNegServiceConfig>,
    /// Optional forward supplemental channel configuration.
    pub for_sch_config: Option<ForSchConfig>,
}

impl ServiceConnectParams {
    fn validate_for_sch_config(sch: &ForSchConfig) {
        assert!(sch.sch_id <= 1, "FOR_SCH_ID must be SCH0 or SCH1");
        assert!(
            matches!(sch.mux_option, 0x0809 | 0x0811 | 0x0821 | 0x0921),
            "F-SCH FOR_SCH_MUX must be Rate Set 1 MuxPDU Type 3"
        );
        assert!(sch.rc == 3, "Phase 1 F-SCH requires SCH_RC=3");
        assert!(sch.coding <= 1, "SCH CODING exceeds 1 bit");
        assert!(
            matches!(sch.rate, 0x1..=0x4),
            "F-SCH MAX_RATE must be 0x1..=0x4 for convolutional RC3"
        );
    }

    /// Encode the f-dsch Service Connect Message SDU.
    ///
    /// Format: USE_TIME(1) + ACTION_TIME(6) + SERV_CON_SEQ(3) + RESERVED(2) +
    ///   USE_OLD_SERV_CONFIG(2) + SYNC_ID_INCL(1) +
    ///   [RECORD_TYPE(8) + RECORD_LEN(8) + record data]... +
    ///   optional call assignments + USE_TYPE0_PLCM(1).
    pub fn to_ftch_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        let use_old_serv_config = self.use_old_serv_config & 0x03;
        let sync_id_incl = self.sync_id.is_some();
        let cc_info_incl = use_old_serv_config == 0 && !self.call_assignments.is_empty();

        // SDU header
        bs.write_u8(0, 1); // USE_TIME = 0
        bs.write_u8(0, 6); // ACTION_TIME = 000000
        bs.write_u8(self.serv_con_seq & 0x07, 3);
        bs.write_u8(0, 2); // RESERVED
        bs.write_u8(use_old_serv_config, 2);

        // Stored-config restore / release flows are still unsupported. We only
        // emit the normal `00` shape unless a future caller adds the rest of
        // the restore-state machine and matching tail fields.
        assert!(
            use_old_serv_config != 0b11,
            "ServiceConnect USE_OLD_SERV_CONFIG=11 restore/release flow is not supported"
        );
        assert!(
            !(use_old_serv_config == 0b10 && sync_id_incl),
            "ServiceConnect USE_OLD_SERV_CONFIG=10 with SYNC_ID restore semantics is not supported"
        );

        bs.write_u8(sync_id_incl as u8, 1);
        if let Some(sync_id) = &self.sync_id {
            assert!(
                !sync_id.is_empty() && sync_id.len() <= 0x0f,
                "ServiceConnect SYNC_ID length must fit in 4 bits and be non-zero"
            );
            bs.write_u8(sync_id.len() as u8, 4);
            for &byte in sync_id {
                bs.write_u8(byte, 8);
            }
        }

        if use_old_serv_config != 0b01 && use_old_serv_config != 0b11 {
            // Service Configuration record
            let svc_cfg = self.encode_service_config_record();
            bs.write_u8(InfoRecordType::ServiceConfiguration as u8, 8); // RECORD_TYPE
            bs.write_u8(svc_cfg.len() as u8, 8); // RECORD_LEN (bytes)
            bs.extend(&Bitstream::new_bytes(&svc_cfg));

            // Non-Negotiable Service Configuration record
            if let Some(ref non_neg) = self.non_neg {
                let raw = non_neg.encode();
                bs.write_u8(InfoRecordType::NonNegServiceConfiguration as u8, 8); // RECORD_TYPE
                bs.write_u8(raw.len() as u8, 8); // RECORD_LEN (bytes)
                bs.extend(&Bitstream::new_bytes(&raw));
            }
        }

        if use_old_serv_config == 0 {
            bs.write_u8(cc_info_incl as u8, 1);
            if cc_info_incl {
                assert!(
                    self.call_assignments.len() <= u8::MAX as usize,
                    "ServiceConnect call assignment count must fit in 8 bits"
                );
                bs.write_u8(self.call_assignments.len() as u8, 8);
                for assignment in &self.call_assignments {
                    bs.write_u8(assignment.con_ref, 8);
                    bs.write_u8(assignment.response_ind as u8, 1);
                    if assignment.response_ind {
                        let tag = assignment.tag.expect(
                            "ServiceConnect call assignment with RESPONSE_IND=1 requires TAG",
                        );
                        bs.write_u8(tag & 0x0f, 4);
                    } else {
                        bs.write_u8(assignment.bypass_alert_answer.unwrap_or(false) as u8, 1);
                    }
                }
            }
        }

        bs.write_u8(self.use_type0_plcm as u8, 1);

        bs
    }

    /// Encode the Service Configuration record body (the bytes after type+len).
    ///
    /// Field widths per C.S0004-E 3.7.2.3.2.21 / real trace:
    ///   FOR_MUX_OPTION(16) + REV_MUX_OPTION(16) + FOR_RATES(8) + REV_RATES(8) +
    ///   NUM_CON_REC(8) + [connection records] + channel config
    fn encode_service_config_record(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();

        bs.write_u32(self.for_mux_option as u32, 16);
        bs.write_u32(self.rev_mux_option as u32, 16);
        bs.write_u8(self.for_rates, 8);
        bs.write_u8(self.rev_rates, 8);
        bs.write_u8(self.connections.len() as u8, 8); // NUM_CON_REC(8)

        for conn in &self.connections {
            let rec = self.encode_connection_record(conn);
            let total_len = rec
                .len()
                .checked_add(1)
                .expect("ServiceConnect connection record length overflow");
            bs.write_u8(total_len as u8, 8); // RECORD_LEN includes this field.
            bs.extend(&Bitstream::new_bytes(&rec));
        }

        // Channel configuration
        bs.write_u8(1, 1); // FCH_CC_INCL = 1
        bs.write_u8(self.fch_frame_size & 0x01, 1); // FCH_FRAME_SIZE (1 bit)
        bs.write_u8(self.for_fch_rc & 0x1F, 5); // FOR_FCH_RC (5 bits)
        bs.write_u8(self.rev_fch_rc & 0x1F, 5); // REV_FCH_RC (5 bits)
        bs.write_u8(0, 1); // DCCH_CC_INCL = 0
        if let Some(ref sch) = self.for_sch_config {
            Self::validate_for_sch_config(sch);
            // FOR_SCH_CC block per C.S0005-E §3.7.5.7. This build sends a
            // single F-SCH (NUM_FOR_SCH=1) with the SCH_CC_Type-specific
            // subrecord formatted per §3.7.5.7.1 (16 bits / 2 octets).
            bs.write_u8(1, 1); // FOR_SCH_CC_INCL = 1
            bs.write_u8(1, 2); // NUM_FOR_SCH = 1 (spec forbids '00' when INCL=1)
            // Per-SCH 3-field record: FOR_SCH_ID + FOR_SCH_MUX + SCH_CC_Type-specific
            bs.write_u8(sch.sch_id, 2); // FOR_SCH_ID (Table 3.7.5.7-5: 00=SCH0, 01=SCH1)
            bs.write_u32(sch.mux_option as u32, 16); // FOR_SCH_MUX
            // SCH_CC_Type-specific subfields per §3.7.5.7.1:
            bs.write_u8(2, 4); // SCH_REC_LEN = 2 (record length in octets, includes this field)
            bs.write_u8(sch.rc, 5); // SCH_RC
            bs.write_u8(sch.coding, 1); // CODING (0 = convolutional)
            bs.write_u8(0, 1); // FRAME_40_USED = 0 (20 ms frames only)
            bs.write_u8(0, 1); // FRAME_80_USED = 0
            bs.write_u8(sch.rate, 4); // MAX_RATE
        } else {
            bs.write_u8(0, 1); // FOR_SCH_CC_INCL = 0
        }
        bs.write_u8(0, 1); // REV_SCH_CC_INCL = 0
        bs.write_u8(0, 1); // RESERVED

        bitstream_to_byte_vec(&bs)
    }

    /// Encode a single connection record body (the bytes after RECORD_LEN).
    ///
    /// Variable-length fields per C.S0005-E 3.7.5.7:
    ///   CON_REF(8) + SERVICE_OPTION(16) + FOR_TRAFFIC(4) + REV_TRAFFIC(4) +
    ///   UI_ENCRYPT_MODE(3) + SR_ID(3) + RLP_INFO_INCL(1) +
    ///   [RLP_BLOB_LEN(4) + RLP_BLOB(8 * len)] +
    ///   QOS_PARMS_INCL(1) + [QOS_PARMS_LEN(5) + QOS_PARMS(8 * len)] +
    ///   byte alignment padding.
    fn encode_connection_record(&self, conn: &ServiceConnectConnectionRecord) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(conn.con_ref, 8); // CON_REF (8 bits)
        bs.write_u32(conn.service_option as u32, 16); // SERVICE_OPTION (16 bits)
        bs.write_u8(conn.for_traffic & 0x0F, 4); // FOR_TRAFFIC (4 bits)
        bs.write_u8(conn.rev_traffic & 0x0F, 4); // REV_TRAFFIC (4 bits)
        bs.write_u8(conn.ui_encrypt_mode & 0x07, 3); // UI_ENCRYPT_MODE (3 bits)
        bs.write_u8(conn.sr_id & 0x07, 3); // SR_ID (3 bits)

        let rlp_blob = conn.rlp_blob.as_deref().unwrap_or(&[]);
        assert!(
            rlp_blob.len() <= 0x0f,
            "ServiceConnect RLP_BLOB must fit in 4 bits of length"
        );
        let rlp_info_incl = conn.rlp_blob.is_some();
        assert!(
            conn.rlp_info_incl == rlp_info_incl,
            "ServiceConnect RLP_INFO_INCL must match RLP_BLOB presence"
        );
        bs.write_u8(rlp_info_incl as u8, 1); // RLP_INFO_INCL (1 bit)
        if rlp_info_incl {
            bs.write_u8(rlp_blob.len() as u8, 4); // RLP_BLOB_LEN (4 bits)
            for &byte in rlp_blob {
                bs.write_u8(byte, 8);
            }
        }

        let qos_parms = conn.qos_parms.as_deref().unwrap_or(&[]);
        assert!(
            qos_parms.len() <= 0x1f,
            "ServiceConnect QOS_PARMS must fit in 5 bits of length"
        );
        let qos_parms_incl = conn.qos_parms.is_some();
        bs.write_u8(qos_parms_incl as u8, 1); // QOS_PARMS_INCL (1 bit)
        if qos_parms_incl {
            bs.write_u8(qos_parms.len() as u8, 5); // QOS_PARMS_LEN (5 bits)
            for &byte in qos_parms {
                bs.write_u8(byte, 8);
            }
        }

        let remainder = bs.bits().len() % 8;
        if remainder != 0 {
            bs.write_u8(0, 8 - remainder);
        }

        bitstream_to_byte_vec(&bs)
    }
}

/// Service Request Message parameters for the f-dsch.
///
/// Encodes per C.S0005-E 3.7.3.3.2.18 (Service Request Message).
/// The SDU contains:
///   SERV_REQ_SEQ(3) + REQ_PURPOSE(4) +
///   [RECORD_TYPE(8) + RECORD_LEN(8) + Service Config record] (when proposing)
#[derive(Debug, Clone)]
pub struct ServiceRequestParams {
    pub serv_req_seq: u8,
    /// REQ_PURPOSE: 0b0001 = reject, 0b0010 = propose
    pub req_purpose: u8,
    /// Service Config record fields, required when req_purpose == 0b0010 (propose).
    /// Uses the same encoding as Service Connect's service config record.
    pub service_config: Option<ServiceRequestConfig>,
}

/// Service configuration carried inside a Service Request (propose).
/// Reuses ServiceConnectParams-style fields for the config record.
#[derive(Debug, Clone)]
pub struct ServiceRequestConfig {
    pub for_mux_option: u16,
    pub rev_mux_option: u16,
    pub for_rates: u8,
    pub rev_rates: u8,
    pub connections: Vec<ServiceConnectConnectionRecord>,
    pub fch_frame_size: u8,
    pub for_fch_rc: u8,
    pub rev_fch_rc: u8,
}

impl ServiceRequestParams {
    /// Encode the f-dsch Service Request Message SDU.
    pub fn to_ftch_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u8(self.serv_req_seq & 0x07, 3); // SERV_REQ_SEQ
        bs.write_u8(self.req_purpose & 0x0F, 4); // REQ_PURPOSE

        if self.req_purpose == 0b0010 {
            // Propose — include Service Configuration record
            let cfg = self
                .service_config
                .as_ref()
                .expect("ServiceRequest propose (REQ_PURPOSE=0010) requires service_config");
            let svc_cfg = cfg.encode_service_config_record();
            bs.write_u8(InfoRecordType::ServiceConfiguration as u8, 8); // RECORD_TYPE
            bs.write_u8(svc_cfg.len() as u8, 8); // RECORD_LEN (bytes)
            bs.extend(&Bitstream::new_bytes(&svc_cfg));
        }

        bs
    }
}

impl ServiceRequestConfig {
    /// Encode the Service Configuration record body (same format as Service Connect).
    fn encode_service_config_record(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();

        bs.write_u32(self.for_mux_option as u32, 16);
        bs.write_u32(self.rev_mux_option as u32, 16);
        bs.write_u8(self.for_rates, 8);
        bs.write_u8(self.rev_rates, 8);
        bs.write_u8(self.connections.len() as u8, 8); // NUM_CON_REC(8)

        for conn in &self.connections {
            let rec = Self::encode_connection_record(conn);
            let total_len = rec
                .len()
                .checked_add(1)
                .expect("ServiceRequest connection record length overflow");
            bs.write_u8(total_len as u8, 8); // RECORD_LEN includes this field
            bs.extend(&Bitstream::new_bytes(&rec));
        }

        // Channel configuration
        bs.write_u8(1, 1); // FCH_CC_INCL = 1
        bs.write_u8(self.fch_frame_size & 0x01, 1); // FCH_FRAME_SIZE (1 bit)
        bs.write_u8(self.for_fch_rc & 0x1F, 5); // FOR_FCH_RC (5 bits)
        bs.write_u8(self.rev_fch_rc & 0x1F, 5); // REV_FCH_RC (5 bits)
        bs.write_u8(0, 1); // DCCH_CC_INCL = 0
        bs.write_u8(0, 1); // FOR_SCH_CC_INCL = 0 (Service Request doesn't carry SCH config)
        bs.write_u8(0, 1); // REV_SCH_CC_INCL = 0
        bs.write_u8(0, 1); // RESERVED

        bitstream_to_byte_vec(&bs)
    }

    /// Encode a single connection record body.
    fn encode_connection_record(conn: &ServiceConnectConnectionRecord) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(conn.con_ref, 8);
        bs.write_u32(conn.service_option as u32, 16);
        bs.write_u8(conn.for_traffic & 0x0F, 4);
        bs.write_u8(conn.rev_traffic & 0x0F, 4);
        bs.write_u8(conn.ui_encrypt_mode & 0x07, 3);
        bs.write_u8(conn.sr_id & 0x07, 3);

        let rlp_info_incl = conn.rlp_blob.is_some();
        bs.write_u8(rlp_info_incl as u8, 1);
        if let Some(ref blob) = conn.rlp_blob {
            bs.write_u8(blob.len() as u8, 4);
            for &byte in blob {
                bs.write_u8(byte, 8);
            }
        }

        let qos_parms_incl = conn.qos_parms.is_some();
        bs.write_u8(qos_parms_incl as u8, 1);
        if let Some(ref qos) = conn.qos_parms {
            bs.write_u8(qos.len() as u8, 5);
            for &byte in qos {
                bs.write_u8(byte, 8);
            }
        }

        let remainder = bs.bits().len() % 8;
        if remainder != 0 {
            bs.write_u8(0, 8 - remainder);
        }

        bitstream_to_byte_vec(&bs)
    }
}

/// Convert a Bitstream to a byte vector (packing 8 bits per byte, MSB first).
fn bitstream_to_byte_vec(bs: &Bitstream) -> Vec<u8> {
    let bits = bs.bits();
    bits.chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (j, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - j);
            }
            byte
        })
        .collect()
}

fn write_general_page_record(bs: &mut Bitstream, record: &GeneralPageRecord) {
    match record {
        GeneralPageRecord::Class0 {
            page_subclass,
            msg_seq,
            imsi_s,
            imsi_11_12,
            mcc,
            imsi_addr_num: _,
            imsi_m_s1: _,
            imsi_m_s2: _,
            special_service,
            service_option,
        } => {
            bs.write_u8(0, 2);
            bs.write_u8(*page_subclass, 2);
            bs.write_u8(*msg_seq, 3);
            match page_subclass {
                0 => {
                    bs.write_u64(imsi_s.unwrap_or(0), 34);
                }
                1 => {
                    bs.write_u8(imsi_11_12.unwrap_or(0), 7);
                    bs.write_u64(imsi_s.unwrap_or(0), 34);
                }
                2 => {
                    // Format 2: MCC(10) + IMSI_S(34)
                    bs.write_u32(mcc.unwrap_or(0) as u32, 10);
                    bs.write_u64(imsi_s.unwrap_or(0), 34);
                }
                3 => {
                    // Format 3: MCC(10) + IMSI_11_12(7) + IMSI_S(34)
                    bs.write_u32(mcc.unwrap_or(0) as u32, 10);
                    bs.write_u8(imsi_11_12.unwrap_or(0), 7);
                    bs.write_u64(imsi_s.unwrap_or(0), 34);
                }
                _ => {}
            }
            bs.write_u8(*special_service as u8, 1);
            if *special_service {
                bs.write_u32(service_option.unwrap_or(0) as u32, 16);
            }
        }
        GeneralPageRecord::Class1 {
            msg_seq,
            esn,
            special_service,
            service_option,
        } => {
            bs.write_u8(1, 2);
            bs.write_u8(*msg_seq, 3);
            bs.write_u32(*esn, 32);
            bs.write_u8(*special_service as u8, 1);
            if *special_service {
                bs.write_u32(service_option.unwrap_or(0) as u32, 16);
            }
        }
        GeneralPageRecord::Tmsi {
            msg_seq,
            tmsi_code_addr,
            special_service,
            service_option,
        } => {
            bs.write_u8(2, 2);
            bs.write_u8(*msg_seq, 3);
            bs.write_u32(*tmsi_code_addr, 32);
            bs.write_u8(*special_service as u8, 1);
            if *special_service {
                bs.write_u32(service_option.unwrap_or(0) as u32, 16);
            }
        }
        GeneralPageRecord::Broadcast { bc_addr } => {
            bs.write_u8(3, 2);
            bs.write_u32(*bc_addr as u32, 16);
        }
    }
}
// ---------------------------------------------------------------------------
// Extended Supplemental Channel Assignment Message (ESCAM)
// ---------------------------------------------------------------------------

/// Parameters for the ESCAM (sent on F-TCH to assign/release F-SCH).
///
/// Per C.S0005-E §3.7.3.3.2.37 — this encodes the complete message structure.
/// Many optional sections (reverse SCH, 3X, BCMC) are hardcoded to "not included":
/// this build sends only forward SCH, with no soft handoff.
#[derive(Clone, Debug)]
pub struct EscamParams {
    /// START_TIME_UNIT (3 bits): time unit for start times.
    /// 0 = 1 frame (20ms), 1 = 2 frames, etc.
    pub start_time_unit: u8,
    /// Forward SCH identifier (0 or 1).
    pub for_sch_id: u8,
    /// Supplemental Channel Code List index (4 bits).
    /// Indexes into the SCCL established during service negotiation.
    pub sccl_index: u8,
    /// FOR_SCH_NUM_BITS_IDX (4 bits): index into Table 3.7.3.3.2.37-4
    /// for the number of information bits per frame.
    /// 0x1=360 bits (19.2k), 0x2=744 (38.4k), 0x3=1512 (76.8k), etc.
    pub for_sch_num_bits_idx: u8,
    /// Pilot PN offset (9 bits, in 64-chip units). Our serving sector.
    pub pilot_pn: u16,
    /// CODE_CHAN_SCH (11 bits): Walsh code index for the SCH.
    pub code_chan_sch: u16,
    /// QOF mask (0 = standard Walsh, 1-3 = QOF set).
    pub qof_mask_id_sch: u8,
    /// FOR_SCH_DURATION (4 bits): 0=stop, 1-14=N frames, 15=infinite.
    pub for_sch_duration: u8,
    /// Include explicit start time (false = implicit start).
    pub for_sch_start_time_incl: bool,
    /// Start time in START_TIME_UNIT units mod 32 (5 bits). Only if start_time_incl.
    pub for_sch_start_time: u8,
    /// Include FPC (forward power control) parameters.
    pub fpc_incl: bool,
    /// FPC mode for SCH (3 bits). Only if fpc_incl.
    pub fpc_mode_sch: u8,
    /// FPC initial setpoint option (1 bit). Only if fpc_incl.
    pub fpc_sch_init_setpt_op: u8,
    /// Target FER (5 bits, per Table 3.7.3.3.2.25-2). Only if fpc_incl.
    pub fpc_sch_fer: u8,
    /// Initial Eb/Nt setpoint (8 bits, 0.125 dB/LSB). Only if fpc_incl.
    pub fpc_sch_init_setpt: u8,
    /// Min Eb/Nt setpoint (8 bits, 0.125 dB/LSB). Only if fpc_incl.
    pub fpc_sch_min_setpt: u8,
    /// Max Eb/Nt setpoint (8 bits, 0.125 dB/LSB). Only if fpc_incl.
    pub fpc_sch_max_setpt: u8,
}

impl EscamParams {
    fn validate(&self) {
        assert!(
            self.start_time_unit <= 0b111,
            "START_TIME_UNIT exceeds 3 bits"
        );
        assert!(self.for_sch_id <= 1, "FOR_SCH_ID exceeds 1 bit");
        assert!(self.sccl_index <= 0x0f, "SCCL_INDEX exceeds 4 bits");
        assert!(
            matches!(self.for_sch_num_bits_idx, 0x1..=0x4),
            "ESCAM FOR_SCH_NUM_BITS_IDX must be 0x1..=0x4 for convolutional RC3"
        );
        assert!(self.pilot_pn <= 0x1ff, "PILOT_PN exceeds 9 bits");
        assert!(self.code_chan_sch <= 0x7ff, "CODE_CHAN_SCH exceeds 11 bits");
        assert!(
            self.qof_mask_id_sch <= 0b11,
            "QOF_MASK_ID_SCH exceeds 2 bits"
        );
        assert!(
            self.for_sch_duration <= 0x0f,
            "FOR_SCH_DURATION exceeds 4 bits"
        );
        assert!(
            self.for_sch_duration == 0 || self.for_sch_start_time_incl,
            "FOR_SCH_START_TIME_INCL must be 1 when FOR_SCH_DURATION is non-zero"
        );
        assert!(
            !self.for_sch_start_time_incl || self.for_sch_start_time <= 0x1f,
            "FOR_SCH_START_TIME exceeds 5 bits"
        );
        assert!(self.fpc_mode_sch <= 0b111, "FPC_MODE_SCH exceeds 3 bits");
        assert!(
            self.fpc_sch_init_setpt_op <= 1,
            "FPC_SCH_INIT_SETPT_OP exceeds 1 bit"
        );
        assert!(self.fpc_sch_fer != 0b11111, "FPC_SCH_FER=11111 is reserved");
        assert!(
            self.fpc_sch_min_setpt <= self.fpc_sch_max_setpt,
            "FPC_SCH_MIN_SETPT must be <= FPC_SCH_MAX_SETPT"
        );
        if self.fpc_sch_init_setpt_op == 0 {
            assert!(
                self.fpc_sch_min_setpt <= self.fpc_sch_init_setpt
                    && self.fpc_sch_init_setpt <= self.fpc_sch_max_setpt,
                "absolute FPC_SCH_INIT_SETPT must be within min/max setpoints"
            );
        }
    }

    /// Encode the ESCAM as a traffic channel signaling SDU (bit vector).
    pub fn encode_sdu(&self) -> Vec<u8> {
        bitstream_to_byte_vec(&self.to_ftch_sdu())
    }

    /// Encode the ESCAM as a `Bitstream` SDU per C.S0005-E §3.7.3.3.2.37.
    ///
    /// Encodes the complete message structure. Unused sections (reverse SCH,
    /// 3X, BCMC, soft handoff) are set to "not included".
    pub fn to_ftch_sdu(&self) -> Bitstream {
        self.validate();
        let mut bs = Bitstream::new();

        // ---- Section 1: Timing and control ----
        bs.write_u8(self.start_time_unit, 3); // START_TIME_UNIT
        bs.write_u8(0, 4); // REV_SCH_DTX_DURATION = 0 (no reverse SCH)
        bs.write_u8(0, 1); // USE_T_ADD_ABORT = 0
        bs.write_u8(0, 1); // USE_SCRM_SEQ_NUM = 0 (no SCRM_SEQ_NUM)
        bs.write_u8(0, 1); // ADD_INFO_INCL = 0 (no FPC_PRI_CHAN)

        // ---- Section 2: Reverse SCH config (not included) ----
        bs.write_u8(0, 1); // REV_CFG_INCLUDED = 0

        // ---- Section 3: Reverse SCH assignments (none) ----
        bs.write_u8(0, 2); // NUM_REV_SCH = 0

        // ---- Section 4: Forward SCH config ----
        bs.write_u8(1, 1); // FOR_CFG_INCLUDED = 1
        bs.write_u8(0, 1); // FOR_SCH_FER_REP = 0

        // One forward config record
        bs.write_u8(0, 5); // NUM_FOR_CFG_RECS = 0 (means 1 record)

        // Forward config record:
        bs.write_u8(self.for_sch_id, 1); // FOR_SCH_ID
        bs.write_u8(self.sccl_index, 4); // SCCL_INDEX
        bs.write_u8(self.for_sch_num_bits_idx, 4); // FOR_SCH_NUM_BITS_IDX
        bs.write_u8(0, 3); // NUM_SUP_SHO = 0 (1 pilot, no soft handoff)

        // Single pilot record (our serving sector):
        bs.write_u32(self.pilot_pn as u32, 9); // PILOT_PN (9 bits)
        bs.write_u8(0, 1); // ADD_PILOT_REC_INCL = 0
        // No ACTIVE_PILOT_REC_TYPE or RECORD_LEN since ADD_PILOT_REC_INCL=0
        bs.write_u32(self.code_chan_sch as u32, 11); // CODE_CHAN_SCH
        bs.write_u8(self.qof_mask_id_sch, 2); // QOF_MASK_ID_SCH

        // ---- Section 5: Forward SCH assignments ----
        bs.write_u8(1, 2); // NUM_FOR_SCH = 1

        // Single forward SCH assignment:
        bs.write_u8(self.for_sch_id, 1); // FOR_SCH_ID
        bs.write_u8(self.for_sch_duration, 4); // FOR_SCH_DURATION
        bs.write_u8(if self.for_sch_start_time_incl { 1 } else { 0 }, 1);
        if self.for_sch_start_time_incl {
            bs.write_u8(self.for_sch_start_time, 5); // FOR_SCH_START_TIME
        }
        bs.write_u8(self.sccl_index, 4); // SCCL_INDEX

        // ---- Section 6: Forward power control ----
        bs.write_u8(if self.fpc_incl { 1 } else { 0 }, 1); // FPC_INCL
        if self.fpc_incl {
            let mode = self.fpc_mode_sch;
            bs.write_u8(mode, 3); // FPC_MODE_SCH
            bs.write_u8(self.fpc_sch_init_setpt_op, 1); // FPC_SCH_INIT_SETPT_OP
            // Present only for SCH FPC modes that use a secondary channel.
            if matches!(mode, 0b001 | 0b010 | 0b101 | 0b110) {
                bs.write_u8(0, 1); // FPC_SEC_CHAN — placeholder; we don't use FPC modes that need it.
            }
            bs.write_u8(1, 2); // NUM_SUP = 1, direct count.
            // FPC record for the single SCH:
            bs.write_u8(self.for_sch_id, 1); // SCH_ID
            bs.write_u8(self.fpc_sch_fer, 5); // FPC_SCH_FER
            bs.write_u8(self.fpc_sch_init_setpt, 8); // FPC_SCH_INIT_SETPT
            bs.write_u8(self.fpc_sch_min_setpt, 8); // FPC_SCH_MIN_SETPT
            bs.write_u8(self.fpc_sch_max_setpt, 8); // FPC_SCH_MAX_SETPT
            bs.write_u8(0, 1); // FPC_THRESH_SCH_INCL = 0
        }

        // ---- Section 7: Reverse power control (not included) ----
        bs.write_u8(0, 1); // RPC_INCL = 0

        // ---- Section 8: 3X rate (not included) ----
        bs.write_u8(0, 1); // 3X_SCH_INFO_INCL = 0

        // ---- Section 9: Code channel soft handoff (not included) ----
        bs.write_u8(0, 1); // CCSH_INCLUDED = 0

        // ---- Section 10: Forward SCH service config ----
        // Service Connect already supplied this; ESCAM only assigns timing.
        bs.write_u8(0, 1); // FOR_SCH_CC_INCL = 0

        // ---- Section 11: Reverse SCH service config (not included) ----
        bs.write_u8(0, 1); // REV_SCH_CC_INCL = 0

        // ---- Section 12: SCH BCMC / outer code extensions ----
        // Point-to-point F-SCH only.
        bs.write_u8(0, 1); // SCH_BCMC_IND = 0

        bs
    }
}

#[cfg(test)]
mod forward_overhead_decode_tests {
    use crate::consts::SERVICE_OPTION_EVRC_A;

    use super::*;

    fn common_roundtrip(message: PagingChannelMessage) -> PagingChannelMessage {
        let original_bits = message.to_sdu();
        let mut decode_bits = original_bits.clone();
        let decoded = PagingChannelMessage::from_sdu(message.message_id(), &mut decode_bits)
            .expect("common overhead decode should succeed");
        assert_eq!(decoded.to_sdu().bits(), original_bits.bits());
        decoded
    }

    fn cdma_7bit_text_fields(text: &str) -> Vec<u8> {
        let mut bits = Bitstream::new();
        bits.write_u8(0x02, 5); // MSG_ENCODING = C.R1001 7-bit ASCII
        bits.write_u8(text.len() as u8, 8);
        for byte in text.bytes() {
            bits.write_u8(byte, 7);
        }
        pad_to_octet(&mut bits);
        bits.to_packed_bytes()
    }

    fn cdma_7bit_char_bits(text: &str) -> Vec<u8> {
        let mut bits = Bitstream::new();
        for byte in text.bytes() {
            bits.write_u8(byte, 7);
        }
        bits.bits().to_vec()
    }

    fn fnm_bits_with_record(record_type: InfoRecordType, data: &[u8]) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(record_type as u8, 8);
        bits.write_u8(data.len() as u8, 8);
        for byte in data {
            bits.write_u8(*byte, 8);
        }
        bits
    }

    fn cdma_redirection_record_bytes() -> Vec<u8> {
        cdma_redirection_record_bytes_for_chans(&[384])
    }

    fn cdma_redirection_record_bytes_for_chans(chans: &[u16]) -> Vec<u8> {
        let mut bits = Bitstream::new();
        bits.write_u8(3, 5); // BAND_CLASS
        bits.write_u32(42, 15); // EXPECTED_SID
        bits.write_u32(65535, 16); // EXPECTED_NID
        bits.write_u8(0, 4); // RESERVED
        bits.write_u8(chans.len() as u8, 4); // NUM_CHANS
        for chan in chans {
            bits.write_u32(*chan as u32, 11); // CDMA_CHAN
        }
        bits.to_packed_bytes()
    }

    fn minimal_eapm_body_bits() -> Vec<u8> {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // PSIST_PARMS_INCL

        bits.write_u8(3, 4); // LAC_PARMS_LEN includes this field and padding
        bits.write_u8(10, 6); // ACC_TMO
        bits.write_u8(0, 4); // RESERVED_1
        bits.write_u8(1, 4); // MAX_REQ_SEQ
        bits.write_u8(1, 4); // MAX_RSP_SEQ
        bits.write_u8(0, 2); // RESERVED padding

        bits.write_u8(0, 3); // NUM_MODE_SELECTION_ENTRIES
        bits.write_u8(0, 3); // ACCESS_MODE = Basic Access
        bits.write_u32(1, 10); // ACCESS_MODE_MIN_DURATION
        bits.write_u32(2, 10); // ACCESS_MODE_MAX_DURATION
        bits.write_u8(0, 6); // RLGAIN_COMMON_PILOT
        bits.write_u8(0, 4); // IC_THRESH
        bits.write_u8(0, 4); // IC_MAX

        bits.write_u8(0, 3); // NUM_MODE_PARM_REC
        bits.write_u8(8, 4); // EACH_PARM_REC_LEN includes this field and padding
        bits.write_u8(0b1000_0000, 8); // APPLICABLE_MODES: Basic only
        bits.write_u8(0, 5); // EACH_NOM_PWR
        bits.write_u8(0, 5); // EACH_INIT_PWR
        bits.write_u8(1, 3); // EACH_PWR_STEP
        bits.write_u8(1, 4); // EACH_NUM_STEP
        bits.write_u8(0, 1); // EACH_PREAMBLE_ENABLED
        bits.write_u8(0, 6); // RESERVED
        bits.write_u8(1, 4); // EACH_PROBE_BKOFF
        bits.write_u8(1, 4); // EACH_BKOFF
        bits.write_u8(3, 6); // EACH_SLOT
        bits.write_u8(0, 6); // EACH_SLOT_OFFSET1
        bits.write_u8(1, 6); // EACH_SLOT_OFFSET2
        bits.write_u8(0, 2); // RESERVED padding

        bits.write_u8(0, 3); // BA_PARMS_LEN
        bits.write_u8(0, 5); // RA_PARMS_LEN
        bits.write_u8(0, 1); // ACCT_INCL
        bits.bits().to_vec()
    }

    #[test]
    fn common_order_from_sdu_preserves_add_record_bytes() {
        let message = OrderMessage {
            order: 0x15,
            ordq: 0x9a,
            order_specific_fields: vec![0xde, 0xad],
        };

        let mut bits = message.to_sdu();
        let decoded = OrderMessage::from_sdu(&mut bits).expect("decode forward order");

        assert_eq!(decoded.order, 0x15);
        assert_eq!(decoded.ordq, 0x9a);
        assert_eq!(decoded.order_specific_fields, vec![0xde, 0xad]);
        assert_eq!(decoded.to_sdu().bits(), message.to_sdu().bits());
    }

    #[test]
    fn common_paging_dispatch_decodes_order_add_record_bytes() {
        let original = PagingChannelMessage::Order(OrderMessage {
            order: 0x2a,
            ordq: 0x10,
            order_specific_fields: vec![0x55, 0xaa, 0x01],
        });

        let decoded = common_roundtrip(original);

        match decoded {
            PagingChannelMessage::Order(m) => {
                assert_eq!(m.order, 0x2a);
                assert_eq!(m.ordq, 0x10);
                assert_eq!(m.order_specific_fields, vec![0x55, 0xaa, 0x01]);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_order_ftch_encoder_preserves_add_record_bytes() {
        let message = OrderMessage {
            order: 0x10,
            ordq: 0x7f,
            order_specific_fields: vec![0x12, 0x34],
        };

        let sdu = message.to_ftch_sdu();

        assert_eq!(sdu.len(), 40);
        assert_eq!(
            sdu.bits(),
            &[
                0, 0, 0, 0, 0, 0, 0, // USE_TIME + ACTION_TIME
                0, 1, 0, 0, 0, 0, // ORDER = 0x10
                0, 1, 1, // ADD_RECORD_LEN = 3
                0, 1, 1, 1, 1, 1, 1, 1, // ORDQ = 0x7f
                0, 0, 0, 1, 0, 0, 1, 0, // 0x12
                0, 0, 1, 1, 0, 1, 0, 0, // 0x34
            ]
        );
    }

    #[test]
    fn forward_order_detail_roundtrips_registration_accepted_encryption() {
        let detail = ForwardOrderDetail::RegistrationAccepted(RegistrationAcceptedOrder {
            roam_indi: Some(0x03),
            c_sig_encrypt_mode: Some(0b001),
            enc_key_size: Some(0b001),
            msg_int_info_incl: Some(true),
            change_keys: Some(true),
            use_uak: Some(false),
        });

        let message = detail.to_order_message().expect("encode reg accepted");
        assert_eq!(message.order, 0b011011);
        assert_eq!(message.ordq, 0x07);
        assert_eq!(message.order_specific_fields, vec![0x03, 0x27, 0x00]);

        let mut sdu = message.to_sdu();
        let decoded = OrderMessage::from_sdu(&mut sdu).expect("decode order sdu");
        assert_eq!(
            decoded.forward_detail().expect("typed detail"),
            detail,
            "typed Registration Accepted detail should survive SDU roundtrip"
        );
    }

    #[test]
    fn forward_order_detail_roundtrips_base_station_reject() {
        let detail = ForwardOrderDetail::BaseStationReject(BaseStationRejectOrder {
            reject_reason: 0b0011,
            rejected_msg_type: 0x05,
            rejected_msg_seq: 0b101,
        });

        let message = detail.to_order_message().expect("encode bs reject");
        assert_eq!(message.order, 0b100001);
        assert_eq!(message.ordq, 0x02);
        assert_eq!(message.order_specific_fields, vec![0x30, 0x5a]);
        assert_eq!(message.forward_detail().expect("typed detail"), detail);
    }

    #[test]
    fn forward_order_detail_roundtrips_retry_and_challenge_confirmation() {
        let retry = ForwardOrderDetail::Retry(RetryOrder {
            retry_type: 0b001,
            retry_delay: Some(0x2a),
        });
        let retry_message = retry.to_order_message().expect("encode retry");
        assert_eq!(retry_message.order_specific_fields, vec![0x25, 0x40]);
        assert_eq!(retry_message.forward_detail().expect("retry detail"), retry);

        let challenge = ForwardOrderDetail::BaseStationChallengeConfirmation { authbs: 0x2aaaa };
        let challenge_message = challenge.to_order_message().expect("encode challenge");
        assert_eq!(
            challenge_message.order_specific_fields,
            vec![0xaa, 0xaa, 0x80]
        );
        assert_eq!(
            challenge_message
                .forward_detail()
                .expect("challenge detail"),
            challenge
        );
    }

    #[test]
    fn forward_order_detail_roundtrips_service_option_status_and_periodic_pilot() {
        let service_request = ForwardOrderDetail::ServiceOptionRequest { service_option: 33 };
        let service_request_message = service_request
            .to_order_message()
            .expect("encode service option request");
        assert_eq!(
            service_request_message.order_specific_fields,
            vec![0x00, 0x21]
        );
        assert_eq!(
            service_request_message
                .forward_detail()
                .expect("service option detail"),
            service_request
        );

        let status = ForwardOrderDetail::StatusRequest {
            information_record_type: 0x0d,
        };
        let status_message = status.to_order_message().expect("encode status request");
        assert_eq!(status_message.order, 0b011010);
        assert_eq!(status_message.ordq, 0x0d);
        assert!(status_message.order_specific_fields.is_empty());
        assert_eq!(
            status_message.forward_detail().expect("status detail"),
            status
        );

        let periodic = ForwardOrderDetail::PeriodicPilotMeasurementRequest(
            PeriodicPilotMeasurementRequestOrder {
                ordq: 0x14,
                min_pilot_pwr_thresh: 0x1f,
                min_pilot_ec_i0_thresh: 0x0a,
                incl_setpt: true,
            },
        );
        let periodic_message = periodic
            .to_order_message()
            .expect("encode periodic pilot request");
        assert_eq!(periodic_message.order_specific_fields, vec![0xfa, 0xa0]);
        assert_eq!(
            periodic_message.forward_detail().expect("periodic detail"),
            periodic
        );
    }

    #[test]
    fn forward_order_detail_rejects_nonzero_reserved_bits() {
        let message = OrderMessage {
            order: 0b100001,
            ordq: 0x02,
            order_specific_fields: vec![0x00, 0x01],
        };

        assert!(
            message.forward_detail().is_err(),
            "Base Station Reject reserved padding must be zero"
        );
    }

    #[test]
    fn common_general_page_from_sdu_roundtrip_zero_reserved() {
        let decoded = common_roundtrip(PagingChannelMessage::GeneralPage(GeneralPageMessage {
            config_msg_seq: 17,
            acc_msg_seq: 9,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_pfield: vec![],
            page_records: vec![],
        }));

        match decoded {
            PagingChannelMessage::GeneralPage(m) => {
                assert_eq!(m.config_msg_seq, 17);
                assert_eq!(m.acc_msg_seq, 9);
                assert_eq!(m.reserved, 0);
                assert!(m.page_records.is_empty());
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn general_page_class0_roundtrip_preserves_imsi_m_s1_s2() {
        // imsi_s = (512 << 24) | 7137214 = 0x200_6CE7BE
        let imsi_s: u64 = ((512_u64) << 24) | 7137214;
        let msg = GeneralPageMessage {
            config_msg_seq: 4,
            acc_msg_seq: 4,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_pfield: vec![],
            page_records: vec![GeneralPageRecord::Class0 {
                page_subclass: 1,
                msg_seq: 2,
                imsi_s: Some(imsi_s),
                imsi_11_12: Some(99),
                mcc: None,
                imsi_addr_num: None,
                imsi_m_s1: Some(7137214),
                imsi_m_s2: Some(512),
                special_service: false,
                service_option: None,
            }],
        };
        let mut bits = msg.to_sdu();
        let decoded = GeneralPageMessage::from_sdu(&mut bits).expect("decode should succeed");
        assert_eq!(decoded.page_records.len(), 1);
        match &decoded.page_records[0] {
            GeneralPageRecord::Class0 {
                imsi_m_s1,
                imsi_m_s2,
                imsi_s: decoded_imsi_s,
                imsi_11_12,
                ..
            } => {
                assert_eq!(
                    *imsi_m_s1,
                    Some(7137214),
                    "imsi_m_s1 lost in GPM round-trip"
                );
                assert_eq!(*imsi_m_s2, Some(512), "imsi_m_s2 lost in GPM round-trip");
                assert_eq!(*decoded_imsi_s, Some(imsi_s));
                assert_eq!(*imsi_11_12, Some(99));
            }
            other => panic!("expected Class0, got {:?}", other),
        }
    }

    #[test]
    fn common_general_page_rejects_reserved_bits() {
        let mut bits = Bitstream::new();
        bits.write_u8(17, 6); // CONFIG_MSG_SEQ
        bits.write_u8(9, 6); // ACC_MSG_SEQ
        bits.write_u8(1, 1); // CLASS_0_DONE
        bits.write_u8(1, 1); // CLASS_1_DONE
        bits.write_u8(1, 1); // TMSI_DONE
        bits.write_u8(0, 1); // ORDERED_TMSIS
        bits.write_u8(1, 1); // BROADCAST_DONE
        bits.write_u8(0b1000, 4); // RESERVED must be zero
        bits.write_u8(0, 3); // ADD_LENGTH

        let err = GeneralPageMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RESERVED"));
    }

    #[test]
    fn common_authentication_challenge_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::AuthenticationChallenge(
            AuthenticationChallengeMessage {
                randu: 0x00ab_cdef,
                gen_cmea_key: true,
            },
        ));

        match decoded {
            PagingChannelMessage::AuthenticationChallenge(m) => {
                assert_eq!(m.randu, 0x00ab_cdef);
                assert!(m.gen_cmea_key);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_ssd_update_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::SsdUpdate(SsdUpdateMessage {
            randssd: 0x0012_3456_789a_bcde,
        }));

        match decoded {
            PagingChannelMessage::SsdUpdate(m) => {
                assert_eq!(m.randssd, 0x0012_3456_789a_bcde);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::FeatureNotification(
            FeatureNotificationMessage {
                release: true,
                records: vec![
                    InformationRecord::signal(SignalInfoRecord {
                        signal_type: 0b10,
                        alert_pitch: 0b00,
                        signal: 0b000001,
                    }),
                    InformationRecord {
                        record_type: InfoRecordType::Display as u8,
                        data: b"OK".to_vec(),
                    },
                    InformationRecord::message_waiting(MessageWaitingRecord { msg_count: 3 }),
                    InformationRecord::parametric_alerting(ParametricAlertingRecord {
                        cadence_count: 2,
                        groups: vec![ParametricAlertingGroup {
                            amplitude: 3,
                            freq_1: 440,
                            freq_2: 0,
                            on_time: 4,
                            off_time: 5,
                            repeat: 6,
                            delay: 7,
                        }],
                        cadence_type: 0b01,
                    }),
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::FeatureNotification(m) => {
                assert!(m.release);
                assert_eq!(m.records.len(), 4);
                assert_eq!(m.records[0].record_type, InfoRecordType::Signal as u8);
                let signal = m.records[0].signal_info().unwrap().expect("Signal");
                assert_eq!(signal.signal_type, 0b10);
                assert_eq!(signal.alert_pitch, 0b00);
                assert_eq!(signal.signal, 0b000001);
                assert_eq!(m.records[1].record_type, InfoRecordType::Display as u8);
                assert_eq!(m.records[1].data, b"OK".to_vec());
                assert_eq!(
                    m.records[1].display_text().expect("decode Display"),
                    Some("OK".to_string())
                );
                assert_eq!(
                    m.records[2].message_waiting_info().unwrap(),
                    Some(MessageWaitingRecord { msg_count: 3 })
                );
                assert_eq!(
                    m.records[3].parametric_alerting_info().unwrap(),
                    Some(ParametricAlertingRecord {
                        cadence_count: 2,
                        groups: vec![ParametricAlertingGroup {
                            amplitude: 3,
                            freq_1: 440,
                            freq_2: 0,
                            on_time: 4,
                            off_time: 5,
                            repeat: 6,
                            delay: 7,
                        }],
                        cadence_type: 0b01,
                    })
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_number_and_subaddress_records_roundtrip() {
        let called = PartyNumberRecord {
            number_type: 1,
            number_plan: 1,
            presentation_indicator: None,
            screening_indicator: None,
            redirection_reason: None,
            digits: "18005551212".to_string(),
        };
        let calling = PartyNumberRecord {
            number_type: 2,
            number_plan: 1,
            presentation_indicator: Some(0),
            screening_indicator: Some(3),
            redirection_reason: None,
            digits: "6025550101".to_string(),
        };
        let redirecting = PartyNumberRecord {
            number_type: 2,
            number_plan: 1,
            presentation_indicator: Some(0),
            screening_indicator: Some(3),
            redirection_reason: Some(2),
            digits: "6025550103".to_string(),
        };
        let called_subaddress = PartySubaddressRecord {
            subaddress_type: 2,
            odd_even_indicator: false,
            data: vec![0x12, 0x34],
        };
        let calling_subaddress = PartySubaddressRecord {
            subaddress_type: 0,
            odd_even_indicator: false,
            data: vec![0xaa; 21],
        };
        let redirecting_subaddress = PartySubaddressRecord {
            subaddress_type: 0,
            odd_even_indicator: false,
            data: vec![0xde, 0xad],
        };

        let decoded = common_roundtrip(PagingChannelMessage::FeatureNotification(
            FeatureNotificationMessage {
                release: false,
                records: vec![
                    InformationRecord::party_number(
                        InfoRecordType::CalledPartyNumber,
                        called.clone(),
                    ),
                    InformationRecord::party_number(
                        InfoRecordType::CallingPartyNumber,
                        calling.clone(),
                    ),
                    InformationRecord::party_number(
                        InfoRecordType::RedirectingNumber,
                        redirecting.clone(),
                    ),
                    InformationRecord::party_subaddress(
                        InfoRecordType::CalledPartySubaddress,
                        called_subaddress.clone(),
                    ),
                    InformationRecord::party_subaddress(
                        InfoRecordType::CallingPartySubaddress,
                        calling_subaddress.clone(),
                    ),
                    InformationRecord::party_subaddress(
                        InfoRecordType::RedirectingSubaddress,
                        redirecting_subaddress.clone(),
                    ),
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::FeatureNotification(m) => {
                assert!(!m.release);
                assert_eq!(m.records.len(), 6);
                assert_eq!(m.records[0].party_number_info().unwrap(), Some(called));
                assert_eq!(m.records[1].party_number_info().unwrap(), Some(calling));
                assert_eq!(m.records[2].party_number_info().unwrap(), Some(redirecting));
                assert_eq!(
                    m.records[3].party_subaddress_info().unwrap(),
                    Some(called_subaddress)
                );
                assert_eq!(
                    m.records[4].party_subaddress_info().unwrap(),
                    Some(calling_subaddress)
                );
                assert_eq!(
                    m.records[5].party_subaddress_info().unwrap(),
                    Some(redirecting_subaddress)
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_extended_display_roundtrip() {
        let display = ExtendedDisplayRecord {
            display_type: 0,
            segments: vec![
                ExtendedDisplaySegment {
                    display_tag: 0x9e,
                    display_len: 5,
                    chars: b"HELLO".to_vec(),
                },
                ExtendedDisplaySegment {
                    display_tag: 0x80,
                    display_len: 3,
                    chars: Vec::new(),
                },
            ],
        };
        let decoded = common_roundtrip(PagingChannelMessage::FeatureNotification(
            FeatureNotificationMessage {
                release: false,
                records: vec![InformationRecord::extended_display(display.clone())],
            },
        ));

        match decoded {
            PagingChannelMessage::FeatureNotification(m) => {
                assert_eq!(m.records.len(), 1);
                assert_eq!(m.records[0].extended_display_info().unwrap(), Some(display));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_multi_char_extended_display_roundtrip() {
        let text_record = MultiCharDisplayTextRecord {
            display_encoding: 0x02,
            num_fields: 2,
            char_bits: cdma_7bit_char_bits("HI"),
        };
        let display = MultiCharExtendedDisplayRecord {
            display_type: 0,
            displays: vec![MultiCharDisplay {
                display_tag: 0x9e,
                records: vec![text_record.clone()],
            }],
        };
        let decoded = common_roundtrip(PagingChannelMessage::FeatureNotification(
            FeatureNotificationMessage {
                release: false,
                records: vec![InformationRecord::multi_char_extended_display(
                    display.clone(),
                )],
            },
        ));

        match decoded {
            PagingChannelMessage::FeatureNotification(m) => {
                assert_eq!(m.records.len(), 1);
                let decoded = m.records[0]
                    .multi_char_extended_display_info()
                    .unwrap()
                    .expect("MC extended display");
                assert_eq!(decoded, display);
                assert_eq!(decoded.displays[0].records[0].text(), "HI");
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_enhanced_multi_char_extended_display_roundtrip() {
        let text_record = MultiCharDisplayTextRecord {
            display_encoding: 0x02,
            num_fields: 2,
            char_bits: cdma_7bit_char_bits("OK"),
        };
        let display = EnhancedMultiCharExtendedDisplayRecord {
            display_type: 0,
            displays: vec![MultiCharDisplay {
                display_tag: 0x9e,
                records: vec![text_record.clone()],
            }],
        };
        let decoded = common_roundtrip(PagingChannelMessage::FeatureNotification(
            FeatureNotificationMessage {
                release: false,
                records: vec![InformationRecord::enhanced_multi_char_extended_display(
                    display.clone(),
                )],
            },
        ));

        match decoded {
            PagingChannelMessage::FeatureNotification(m) => {
                assert_eq!(m.records.len(), 1);
                let decoded = m.records[0]
                    .enhanced_multi_char_extended_display_info()
                    .unwrap()
                    .expect("Enhanced MC extended display");
                assert_eq!(decoded, display);
                assert_eq!(decoded.displays[0].records[0].text(), "OK");
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_international_extended_record_roundtrip() {
        let intl = InternationalExtendedRecord {
            mcc: 310,
            country_record_type: 0x12,
            data: vec![0xab, 0xcd],
        };
        let decoded = common_roundtrip(PagingChannelMessage::FeatureNotification(
            FeatureNotificationMessage {
                release: false,
                records: vec![InformationRecord::international_extended_record(
                    intl.clone(),
                )],
            },
        ));

        match decoded {
            PagingChannelMessage::FeatureNotification(m) => {
                assert_eq!(m.records.len(), 1);
                assert_eq!(
                    m.records[0].international_extended_record_info().unwrap(),
                    Some(intl)
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_feature_notification_rejects_empty_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1);

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("requires at least one"));
    }

    #[test]
    fn common_feature_notification_rejects_empty_display_record() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::Display as u8, 8);
        bits.write_u8(0, 8); // RECORD_LEN

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("requires at least one CHARi"));
    }

    #[test]
    fn common_feature_notification_rejects_display_char_msb() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::Display as u8, 8);
        bits.write_u8(1, 8); // RECORD_LEN
        bits.write_u8(0xC1, 8); // CHARi with MSB set

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("CHARi MSB"));
    }

    #[test]
    fn common_feature_notification_rejects_signal_reserved_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::Signal as u8, 8);
        bits.write_u8(2, 8);
        bits.write_u8(0b1100_0000, 8); // SIGNAL_TYPE=11
        bits.write_u8(0, 8);

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("SIGNAL_TYPE"));
    }

    #[test]
    fn common_feature_notification_rejects_signal_reserved_bits() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::Signal as u8, 8);
        bits.write_u8(2, 8);
        bits.write_u8(0b1000_0000, 8); // SIGNAL_TYPE=10, ALERT_PITCH=00
        bits.write_u8(0b0100_0001, 8); // SIGNAL=1, RESERVED non-zero

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RESERVED"));
    }

    #[test]
    fn common_feature_notification_rejects_message_waiting_bad_length() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::MessageWaiting as u8, 8);
        bits.write_u8(2, 8);
        bits.write_u8(1, 8);
        bits.write_u8(2, 8);

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("Message Waiting"));
    }

    #[test]
    fn common_feature_notification_rejects_non_fnm_record_types() {
        let connected_number = InformationRecord::party_number(
            InfoRecordType::ConnectedNumber,
            PartyNumberRecord {
                number_type: 4,
                number_plan: 1,
                presentation_indicator: Some(1),
                screening_indicator: Some(1),
                redirection_reason: None,
                digits: "6025550102".to_string(),
            },
        );
        let connected_subaddress = InformationRecord::party_subaddress(
            InfoRecordType::ConnectedSubaddress,
            PartySubaddressRecord {
                subaddress_type: 2,
                odd_even_indicator: true,
                data: vec![0x45],
            },
        );
        let records = [
            connected_number,
            InformationRecord::meter_pulses(MeterPulsesRecord {
                pulse_frequency: 44,
                pulse_on_time: 5,
                pulse_off_time: 6,
                pulse_count: 7,
            }),
            InformationRecord::line_control(LineControlRecord {
                polarity: Some(LineControlPolarity::Toggle),
                power_denial_time: 9,
            }),
            InformationRecord::call_waiting_indicator(CallWaitingIndicatorRecord {
                call_waiting: true,
            }),
            connected_subaddress,
            InformationRecord {
                record_type: InfoRecordType::ServiceConfiguration as u8,
                data: Vec::new(),
            },
            InformationRecord {
                record_type: InfoRecordType::NonNegServiceConfiguration as u8,
                data: Vec::new(),
            },
        ];

        for record in records {
            let record_type = record.record_type;
            let mut bits = Bitstream::new();
            bits.write_u8(0, 1); // RELEASE
            bits.write_u8(record_type, 8);
            bits.write_u8(record.data.len() as u8, 8);
            for byte in &record.data {
                bits.write_u8(*byte, 8);
            }

            let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

            assert!(
                err.to_string().contains("not valid for FNM"),
                "record type 0x{record_type:02x} error was {err}"
            );
        }
    }

    #[test]
    fn common_feature_notification_rejects_reserved_record_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(0xff, 8); // reserved RECORD_TYPE
        bits.write_u8(0, 8); // RECORD_LEN

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn common_feature_notification_rejects_parametric_alerting_reserved_cadence_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::ParametricAlerting as u8, 8);
        bits.write_u8(2, 8); // RECORD_LEN
        bits.write_u8(1, 8); // CADENCE_COUNT
        bits.write_u8(0, 4); // NUM_GROUPS
        bits.write_u8(0b11, 2); // CADENCE_TYPE reserved
        bits.write_u8(0, 2); // RESERVED

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("CADENCE_TYPE"));
    }

    #[test]
    fn common_feature_notification_rejects_parametric_alerting_bad_group_length() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::ParametricAlerting as u8, 8);
        bits.write_u8(3, 8); // RECORD_LEN too short for one group
        bits.write_u8(1, 8); // CADENCE_COUNT
        bits.write_u8(1, 4); // NUM_GROUPS
        bits.write_u8(0, 12); // truncated group data

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NUM_GROUPS"));
    }

    #[test]
    fn common_feature_notification_rejects_called_number_char_msb() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::CalledPartyNumber as u8, 8);
        bits.write_u8(2, 8); // RECORD_LEN
        bits.write_u8(1, 3); // NUMBER_TYPE
        bits.write_u8(1, 4); // NUMBER_PLAN
        bits.write_u8(0xB1, 8); // CHARi with MSB set
        bits.write_u8(0, 1); // RESERVED

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("CHARi MSB"));
    }

    #[test]
    fn common_feature_notification_rejects_calling_number_reserved_pi() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::CallingPartyNumber as u8, 8);
        bits.write_u8(2, 8); // RECORD_LEN
        bits.write_u8(1, 3); // NUMBER_TYPE
        bits.write_u8(1, 4); // NUMBER_PLAN
        bits.write_u8(0b11, 2); // PI reserved
        bits.write_u8(0, 2); // SI
        bits.write_u8(0, 5); // RESERVED

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("PI"));
    }

    #[test]
    fn common_feature_notification_rejects_redirecting_number_reserved_reason() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::RedirectingNumber as u8, 8);
        bits.write_u8(3, 8); // RECORD_LEN
        bits.write_u8(0, 1); // EXTENSION_BIT_1: PI/SI included
        bits.write_u8(1, 3); // NUMBER_TYPE
        bits.write_u8(1, 4); // NUMBER_PLAN
        bits.write_u8(0, 1); // EXTENSION_BIT_2: REDIRECTION_REASON included
        bits.write_u8(0, 2); // PI
        bits.write_u8(0, 3); // RESERVED
        bits.write_u8(0, 2); // SI
        bits.write_u8(1, 1); // EXTENSION_BIT_3
        bits.write_u8(0, 3); // RESERVED
        bits.write_u8(0b0011, 4); // REDIRECTION_REASON reserved

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("REDIRECTION_REASON"));
    }

    #[test]
    fn common_feature_notification_rejects_subaddress_bad_extension_bit() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::CalledPartySubaddress as u8, 8);
        bits.write_u8(1, 8); // RECORD_LEN
        bits.write_u8(0, 1); // EXTENSION_BIT must be one
        bits.write_u8(0, 3); // SUBADDRESS_TYPE
        bits.write_u8(0, 1); // ODD/EVEN_INDICATOR
        bits.write_u8(0, 3); // RESERVED

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("EXTENSION_BIT"));
    }

    #[test]
    fn common_feature_notification_rejects_user_specified_subaddress_over_20_octets() {
        let mut data = Bitstream::new();
        data.write_u8(1, 1); // EXTENSION_BIT
        data.write_u8(2, 3); // SUBADDRESS_TYPE = user specified
        data.write_u8(0, 1); // ODD/EVEN_INDICATOR
        data.write_u8(0, 3); // RESERVED
        for _ in 0..21 {
            data.write_u8(0xaa, 8);
        }
        let mut bits = fnm_bits_with_record(
            InfoRecordType::CalledPartySubaddress,
            &data.to_packed_bytes(),
        );

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("exceeds 20 octets"));
    }

    #[test]
    fn common_feature_notification_rejects_extended_display_bad_indicator() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::ExtendedDisplay as u8, 8);
        bits.write_u8(3, 8); // RECORD_LEN
        bits.write_u8(0, 1); // EXT_DISPLAY_IND must be one
        bits.write_u8(0, 7); // DISPLAY_TYPE
        bits.write_u8(0x9e, 8); // DISPLAY_TAG
        bits.write_u8(0, 8); // DISPLAY_LEN

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("EXT_DISPLAY_IND"));
    }

    #[test]
    fn common_feature_notification_rejects_extended_display_reserved_tag() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::ExtendedDisplay as u8, 8);
        bits.write_u8(3, 8); // RECORD_LEN
        bits.write_u8(1, 1); // EXT_DISPLAY_IND
        bits.write_u8(0, 7); // DISPLAY_TYPE
        bits.write_u8(0x9b, 8); // DISPLAY_TAG reserved by Table 3.7.5.16-2
        bits.write_u8(0, 8); // DISPLAY_LEN

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("DISPLAY_TAG"));
    }

    #[test]
    fn common_feature_notification_rejects_extended_display_char_msb() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RELEASE
        bits.write_u8(InfoRecordType::ExtendedDisplay as u8, 8);
        bits.write_u8(4, 8); // RECORD_LEN
        bits.write_u8(1, 1); // EXT_DISPLAY_IND
        bits.write_u8(0, 7); // DISPLAY_TYPE
        bits.write_u8(0x9e, 8); // DISPLAY_TAG Text
        bits.write_u8(1, 8); // DISPLAY_LEN
        bits.write_u8(0xC1, 8); // CHARi with MSB set

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("CHARi MSB"));
    }

    #[test]
    fn common_feature_notification_rejects_multi_char_display_encoding_reserved_bits() {
        let mut data = Bitstream::new();
        data.write_u8(1, 1); // MC_EXT_DISPLAY_IND
        data.write_u8(0, 7); // DISPLAY_TYPE
        data.write_u8(0x9e, 8); // DISPLAY_TAG
        data.write_u8(1, 8); // NUM_RECORD
        data.write_u8(0x20, 8); // DISPLAY_ENCODING top bits non-zero
        data.write_u8(0, 8); // NUM_FIELDS
        let mut bits = fnm_bits_with_record(
            InfoRecordType::MultiCharExtendedDisplay,
            &data.to_packed_bytes(),
        );

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("DISPLAY_ENCODING"));
    }

    #[test]
    fn common_feature_notification_rejects_multi_char_reserved_padding() {
        let mut record =
            InformationRecord::multi_char_extended_display(MultiCharExtendedDisplayRecord {
                display_type: 0,
                displays: vec![MultiCharDisplay {
                    display_tag: 0x9e,
                    records: vec![MultiCharDisplayTextRecord {
                        display_encoding: 0x02,
                        num_fields: 2,
                        char_bits: cdma_7bit_char_bits("HI"),
                    }],
                }],
            });
        *record.data.last_mut().unwrap() |= 0x01;
        let mut bits = fnm_bits_with_record(InfoRecordType::MultiCharExtendedDisplay, &record.data);

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RESERVED"));
    }

    #[test]
    fn common_feature_notification_rejects_enhanced_multi_char_short_record_length() {
        let mut data = Bitstream::new();
        data.write_u8(0, 7); // DISPLAY_TYPE
        data.write_u8(0, 8); // NUM_DISPLAYS: one display
        data.write_u8(0x9e, 8); // DISPLAY_TAG
        data.write_u8(1, 8); // NUM_RECORD
        data.write_u8(2, 8); // RECORD_LENGTH too short
        data.write_u8(0, 8); // DISPLAY_ENCODING placeholder
        data.write_u8(0, 8); // NUM_FIELDS placeholder
        pad_to_octet(&mut data);
        let mut bits = fnm_bits_with_record(
            InfoRecordType::EnhMultiCharExtendedDisplay,
            &data.to_packed_bytes(),
        );

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RECORD_LENGTH"));
    }

    #[test]
    fn common_feature_notification_rejects_international_extended_record_truncated() {
        let mut bits = fnm_bits_with_record(InfoRecordType::ExtendedRecordTypeIntl, &[0x4d]);

        let err = FeatureNotificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("at least two octets"));
    }

    #[test]
    fn common_extended_neighbor_list_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::ExtendedNeighborList(
            ExtendedNeighborListMessage {
                pilot_pn: 42,
                config_msg_seq: 17,
                pilot_inc: 3,
                neighbors: vec![
                    ExtendedNeighborRecord {
                        nghbr_config: 0,
                        nghbr_pn: 84,
                        search_priority: 1,
                        nghbr_band: None,
                        nghbr_freq: None,
                    },
                    ExtendedNeighborRecord {
                        nghbr_config: 2,
                        nghbr_pn: 126,
                        search_priority: 3,
                        nghbr_band: Some(5),
                        nghbr_freq: Some(384),
                    },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::ExtendedNeighborList(m) => {
                assert_eq!(m.pilot_pn, 42);
                assert_eq!(m.config_msg_seq, 17);
                assert_eq!(m.pilot_inc, 3);
                assert_eq!(m.neighbors.len(), 2);
                assert_eq!(m.neighbors[0].nghbr_pn, 84);
                assert_eq!(m.neighbors[0].nghbr_band, None);
                assert_eq!(m.neighbors[1].nghbr_config, 2);
                assert_eq!(m.neighbors[1].search_priority, 3);
                assert_eq!(m.neighbors[1].nghbr_band, Some(5));
                assert_eq!(m.neighbors[1].nghbr_freq, Some(384));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_extended_neighbor_list_rejects_zero_pilot_inc() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9);
        bits.write_u8(1, 6);
        bits.write_u8(0, 4);

        let err = ExtendedNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("PILOT_INC"));
    }

    #[test]
    fn common_extended_neighbor_list_rejects_reserved_neighbor_config() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(1, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // PILOT_INC
        bits.write_u8(0b100, 3); // NGHBR_CONFIG: reserved
        bits.write_u32(84, 9); // NGHBR_PN
        bits.write_u8(0, 2); // SEARCH_PRIORITY
        bits.write_u8(0, 1); // FREQ_INCL

        let err = ExtendedNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NGHBR_CONFIG"));
    }

    #[test]
    fn common_status_request_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::StatusRequest(StatusRequestMessage {
            qual_info: StatusQualificationInfo::BandClassAndOperatingMode {
                band_class: 5,
                op_mode: 0,
            },
            record_types: vec![0x01, 0x07, 0x10],
        }));

        match decoded {
            PagingChannelMessage::StatusRequest(m) => {
                assert_eq!(
                    m.qual_info,
                    StatusQualificationInfo::BandClassAndOperatingMode {
                        band_class: 5,
                        op_mode: 0,
                    }
                );
                assert_eq!(m.record_types, vec![0x01, 0x07, 0x10]);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_status_request_rejects_reserved_qual_info_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 4); // RESERVED
        bits.write_u8(0xff, 8); // QUAL_INFO_TYPE
        bits.write_u8(0, 3); // QUAL_INFO_LEN
        bits.write_u8(0, 4); // NUM_FIELDS

        let err = StatusRequestMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved QUAL_INFO_TYPE"));
    }

    #[test]
    fn common_service_redirection_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::ServiceRedirection(
            ServiceRedirectionMessage {
                return_if_fail: true,
                delete_tmsi: false,
                redirect_type: true,
                record_type: 0x02,
                record: cdma_redirection_record_bytes(),
            },
        ));

        match decoded {
            PagingChannelMessage::ServiceRedirection(m) => {
                assert!(m.return_if_fail);
                assert!(!m.delete_tmsi);
                assert!(m.redirect_type);
                assert_eq!(m.record_type, 0x02);
                assert_eq!(m.record, cdma_redirection_record_bytes());
                match m
                    .redirection_record()
                    .expect("decode SRDM redirection record")
                {
                    ExtendedRedirectionRecord::Cdma {
                        band_class,
                        expected_sid,
                        expected_nid,
                        cdma_chans,
                        redirect_subclasses,
                    } => {
                        assert_eq!(3, band_class);
                        assert_eq!(42, expected_sid);
                        assert_eq!(65535, expected_nid);
                        assert_eq!(vec![384], cdma_chans);
                        assert!(redirect_subclasses.is_none());
                    }
                    other => panic!("unexpected SRDM redirection record: {other:?}"),
                }
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_service_redirection_rejects_ndss_off_with_payload() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RETURN_IF_FAIL
        bits.write_u8(0, 1); // DELETE_TMSI
        bits.write_u8(0, 1); // REDIRECT_TYPE
        bits.write_u8(0, 8); // RECORD_TYPE: NDSS off
        bits.write_u8(1, 8); // RECORD_LEN
        bits.write_u8(0xaa, 8);

        let err = ServiceRedirectionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NDSS off"));
    }

    #[test]
    fn common_service_redirection_decodes_octet_aligned_cdma_record() {
        let message = ServiceRedirectionMessage {
            return_if_fail: true,
            delete_tmsi: false,
            redirect_type: true,
            record_type: 0x02,
            record: cdma_redirection_record_bytes_for_chans(&[384, 425, 450, 475]),
        };

        match message
            .redirection_record()
            .expect("decode SRDM CDMA redirection record")
        {
            ExtendedRedirectionRecord::Cdma {
                cdma_chans,
                redirect_subclasses,
                ..
            } => {
                assert_eq!(cdma_chans, vec![384, 425, 450, 475]);
                assert!(redirect_subclasses.is_none());
            }
            other => panic!("unexpected SRDM redirection record: {other:?}"),
        }
    }

    #[test]
    fn common_service_redirection_rejects_reserved_record_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 1); // RETURN_IF_FAIL
        bits.write_u8(0, 1); // DELETE_TMSI
        bits.write_u8(0, 1); // REDIRECT_TYPE
        bits.write_u8(0x03, 8); // RECORD_TYPE: reserved
        bits.write_u8(0, 8); // RECORD_LEN

        let err = ServiceRedirectionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved RECORD_TYPE"));
    }

    #[test]
    fn common_global_service_redirection_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::GlobalServiceRedirection(
            GlobalServiceRedirectionMessage {
                pilot_pn: 42,
                config_msg_seq: 3,
                redirect_accolc: 0x8001,
                return_if_fail: true,
                delete_tmsi: true,
                excl_p_rev_ms: false,
                record_type: 0x02,
                record: cdma_redirection_record_bytes(),
            },
        ));

        match decoded {
            PagingChannelMessage::GlobalServiceRedirection(m) => {
                assert_eq!(m.pilot_pn, 42);
                assert_eq!(m.config_msg_seq, 3);
                assert_eq!(m.redirect_accolc, 0x8001);
                assert!(m.return_if_fail);
                assert!(m.delete_tmsi);
                assert!(!m.excl_p_rev_ms);
                assert_eq!(m.record_type, 0x02);
                assert_eq!(m.record, cdma_redirection_record_bytes());
                match m
                    .redirection_record()
                    .expect("decode GSRDM redirection record")
                {
                    ExtendedRedirectionRecord::Cdma {
                        band_class,
                        expected_sid,
                        expected_nid,
                        cdma_chans,
                        redirect_subclasses,
                    } => {
                        assert_eq!(3, band_class);
                        assert_eq!(42, expected_sid);
                        assert_eq!(65535, expected_nid);
                        assert_eq!(vec![384], cdma_chans);
                        assert!(redirect_subclasses.is_none());
                    }
                    other => panic!("unexpected GSRDM redirection record: {other:?}"),
                }
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_global_service_redirection_rejects_ndss_off_with_payload() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(3, 6); // CONFIG_MSG_SEQ
        bits.write_u32(0xffff, 16); // REDIRECT_ACCOLC
        bits.write_u8(0, 1); // RETURN_IF_FAIL
        bits.write_u8(0, 1); // DELETE_TMSI
        bits.write_u8(0, 1); // EXCL_P_REV_MS
        bits.write_u8(0, 8); // RECORD_TYPE: NDSS off
        bits.write_u8(1, 8); // RECORD_LEN
        bits.write_u8(0xaa, 8);

        let err = GlobalServiceRedirectionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NDSS off"));
    }

    #[test]
    fn common_global_service_redirection_rejects_reserved_record_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(3, 6); // CONFIG_MSG_SEQ
        bits.write_u32(0xffff, 16); // REDIRECT_ACCOLC
        bits.write_u8(0, 1); // RETURN_IF_FAIL
        bits.write_u8(0, 1); // DELETE_TMSI
        bits.write_u8(0, 1); // EXCL_P_REV_MS
        bits.write_u8(0x04, 8); // RECORD_TYPE: reserved
        bits.write_u8(0, 8); // RECORD_LEN

        let err = GlobalServiceRedirectionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved RECORD_TYPE"));
    }

    #[test]
    fn common_tmsi_assignment_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::TmsiAssignment(
            TmsiAssignmentMessage {
                tmsi_zone: vec![0x01, 0x23, 0x45],
                tmsi_code: 0x89ab_cdef,
                tmsi_exp_time: 0x0012_3456,
            },
        ));

        match decoded {
            PagingChannelMessage::TmsiAssignment(m) => {
                assert_eq!(m.tmsi_zone, vec![0x01, 0x23, 0x45]);
                assert_eq!(m.tmsi_code, 0x89ab_cdef);
                assert_eq!(m.tmsi_exp_time, 0x0012_3456);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_tmsi_assignment_rejects_zero_zone_len() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 5); // RESERVED
        bits.write_u8(0, 4); // TMSI_ZONE_LEN
        bits.write_u32(0, 32);
        bits.write_u32(0, 24);

        let err = TmsiAssignmentMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("TMSI_ZONE_LEN"));
    }

    #[test]
    fn common_paca_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::Paca(PacaMessage {
            purpose: 0b0001,
            q_pos: 7,
            paca_timeout: 0b010,
        }));

        match decoded {
            PagingChannelMessage::Paca(m) => {
                assert_eq!(m.purpose, 0b0001);
                assert_eq!(m.q_pos, 7);
                assert_eq!(m.paca_timeout, 0b010);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_paca_rejects_reserved_purpose() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 7); // RESERVED
        bits.write_u8(0b0100, 4); // PURPOSE
        bits.write_u8(0, 8); // Q_POS
        bits.write_u8(0, 3); // PACA_TIMEOUT

        let err = PacaMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("PURPOSE"));
    }

    #[test]
    fn common_paca_rejects_nonzero_q_pos_for_reoriginate_or_cancel() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 7); // RESERVED
        bits.write_u8(0b0010, 4); // PURPOSE: re-originate
        bits.write_u8(7, 8); // Q_POS must be zero for this purpose
        bits.write_u8(0, 3); // PACA_TIMEOUT

        let err = PacaMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("Q_POS"));
    }

    #[test]
    fn common_general_neighbor_list_minimal_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::GeneralNeighborList(
            GeneralNeighborListMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                pilot_inc: 2,
                nghbr_srch_mode: 0b00,
                nghbr_config_pn_incl: false,
                freq_fields_incl: false,
                use_timing: false,
                global_timing: None,
                neighbors: vec![],
                analog_neighbors: vec![],
                srch_offset_incl: false,
                pilot_info: vec![],
                bcch_support: None,
                resq: None,
                pdch_supported: vec![],
                hrpd_neighbors: None,
            },
        ));

        match decoded {
            PagingChannelMessage::GeneralNeighborList(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.config_msg_seq, 9);
                assert!(m.neighbors.is_empty());
                assert!(m.hrpd_neighbors.is_none());
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_general_neighbor_list_full_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::GeneralNeighborList(
            GeneralNeighborListMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                pilot_inc: 2,
                nghbr_srch_mode: 0b11,
                nghbr_config_pn_incl: true,
                freq_fields_incl: true,
                use_timing: true,
                global_timing: Some(GeneralNeighborGlobalTiming {
                    tx_duration: 4,
                    tx_period: 72,
                }),
                neighbors: vec![
                    GeneralNeighborRecord {
                        nghbr_config: Some(0),
                        nghbr_pn: Some(84),
                        search_priority: Some(2),
                        srch_win_nghbr: Some(7),
                        nghbr_band: Some(5),
                        nghbr_freq: Some(384),
                        timing: Some(GeneralNeighborTiming {
                            tx_offset: 33,
                            tx_duration: None,
                            tx_period: None,
                        }),
                    },
                    GeneralNeighborRecord {
                        nghbr_config: Some(2),
                        nghbr_pn: Some(126),
                        search_priority: Some(1),
                        srch_win_nghbr: Some(5),
                        nghbr_band: None,
                        nghbr_freq: None,
                        timing: None,
                    },
                ],
                analog_neighbors: vec![GeneralAnalogNeighborRecord {
                    band_class: 0,
                    sys_a_b: 0b01,
                }],
                srch_offset_incl: true,
                pilot_info: vec![
                    GeneralNeighborPilotInfo {
                        pilot_record: Some(
                            GeneralNeighborPilotRecord::OneXCommonWithTransmitDiversity {
                                td_power_level: 2,
                                td_mode: 1,
                            },
                        ),
                        srch_offset_nghbr: Some(3),
                    },
                    GeneralNeighborPilotInfo {
                        pilot_record: Some(GeneralNeighborPilotRecord::ThreeXAuxiliary {
                            sr3_primary_pilot: 1,
                            sr3_pilot_power1: 2,
                            sr3_pilot_power2: 3,
                            primary_aux: Sr3AuxPilotInfo {
                                qof: 1,
                                walsh_length: 0,
                                aux_pilot_walsh: 0x12,
                            },
                            lower_aux: Some(Sr3AuxPilotInfo {
                                qof: 2,
                                walsh_length: 1,
                                aux_pilot_walsh: 0x45,
                            }),
                            upper_aux: None,
                        }),
                        srch_offset_nghbr: Some(4),
                    },
                ],
                bcch_support: Some(vec![true, false]),
                resq: Some(GeneralNeighborResqInfo {
                    delay_time: 7,
                    allowed_time: 8,
                    attempt_time: 9,
                    code_chan: 64,
                    qof: 2,
                    min_period: Some(5),
                    num_tot_trans_20ms: Some(3),
                    num_tot_trans_5ms: Some(4),
                    num_preamble_rc1_rc2: 2,
                    num_preamble: 3,
                    power_delta: 0b111,
                    nghbr_resq_configured: vec![true, false],
                }),
                pdch_supported: vec![false, true],
                hrpd_neighbors: Some(vec![HrpdNeighborRecord {
                    nghbr_pn: 200,
                    nghbr_band: Some(5),
                    nghbr_freq: Some(425),
                    pn_association_ind: true,
                    data_association_ind: false,
                }]),
            },
        ));

        match decoded {
            PagingChannelMessage::GeneralNeighborList(m) => {
                assert_eq!(m.neighbors.len(), 2);
                assert_eq!(m.neighbors[0].nghbr_pn, Some(84));
                assert_eq!(m.pilot_info[1].srch_offset_nghbr, Some(4));
                assert_eq!(m.bcch_support, Some(vec![true, false]));
                assert_eq!(m.resq.unwrap().nghbr_resq_configured, vec![true, false]);
                assert_eq!(m.pdch_supported, vec![false, true]);
                assert_eq!(m.hrpd_neighbors.unwrap()[0].nghbr_freq, Some(425));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_general_neighbor_list_rejects_reserved_neighbor_config() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(2, 4); // PILOT_INC
        bits.write_u8(0, 2); // NGHBR_SRCH_MODE
        bits.write_u8(1, 1); // NGHBR_CONFIG_PN_INCL
        bits.write_u8(0, 1); // FREQ_FIELDS_INCL
        bits.write_u8(0, 1); // USE_TIMING
        bits.write_u8(1, 6); // NUM_NGHBR
        bits.write_u8(0b100, 3); // NGHBR_CONFIG: reserved
        bits.write_u32(84, 9); // NGHBR_PN

        let err = GeneralNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NGHBR_CONFIG"));
    }

    #[test]
    fn common_general_neighbor_list_rejects_nonzero_pilot_padding() {
        let mut record = Bitstream::new();
        record.write_u8(1, 2); // QOF
        record.write_u8(0, 3); // WALSH_LENGTH: six Walsh bits follow
        record.write_u8(0x12, 6); // AUX_PILOT_WALSH
        record.write_u8(1, 5); // reserved padding must be zero

        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(2, 4); // PILOT_INC
        bits.write_u8(0, 2); // NGHBR_SRCH_MODE
        bits.write_u8(0, 1); // NGHBR_CONFIG_PN_INCL
        bits.write_u8(0, 1); // FREQ_FIELDS_INCL
        bits.write_u8(0, 1); // USE_TIMING
        bits.write_u8(1, 6); // NUM_NGHBR
        bits.write_u8(0, 3); // NUM_ANALOG_NGHBR
        bits.write_u8(0, 1); // SRCH_OFFSET_INCL
        bits.write_u8(1, 1); // ADD_PILOT_REC_INCL
        bits.write_u8(0b001, 3); // NGHBR_PILOT_REC_TYPE: 1X auxiliary
        bits.write_u8(2, 3); // RECORD_LEN
        bits.extend(&record);
        bits.write_u8(0, 1); // BCCH_IND_INCL
        bits.write_u8(0, 1); // RESQ_ENABLED
        bits.write_u8(0, 1); // NGHBR_PDCH_SUPPORTED
        bits.write_u8(0, 1); // HRPD_NGHBR_INCL

        let err = GeneralNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved padding"));
    }

    #[test]
    fn common_user_zone_identification_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::UserZoneIdentification(
            UserZoneIdentificationMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                uz_exit: 6,
                zones: vec![
                    UserZoneRecord {
                        uzid: 0x1234,
                        uz_rev: 3,
                        temp_sub: true,
                    },
                    UserZoneRecord {
                        uzid: 0xabcd,
                        uz_rev: 15,
                        temp_sub: false,
                    },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::UserZoneIdentification(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.config_msg_seq, 9);
                assert_eq!(m.uz_exit, 6);
                assert_eq!(m.zones.len(), 2);
                assert_eq!(m.zones[0].uzid, 0x1234);
                assert!(m.zones[0].temp_sub);
                assert_eq!(m.zones[1].uz_rev, 15);
                assert!(!m.zones[1].temp_sub);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_user_zone_identification_rejects_truncated_zone_record() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(6, 4); // UZ_EXIT
        bits.write_u8(1, 4); // NUM_UZID
        bits.write_u32(0x1234, 16); // UZID without UZ_REV/TEMP_SUB

        let err = UserZoneIdentificationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn common_private_neighbor_list_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::PrivateNeighborList(
            PrivateNeighborListMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                radio_interfaces: vec![PrivateRadioInterfaceRecord {
                    common_band_class: Some(5),
                    common_nghbr_freq: Some(384),
                    srch_win_pn: 7,
                    neighbors: vec![PrivateNeighborRecord {
                        sid: 42,
                        nid: 65535,
                        pri_nghbr_pn: 84,
                        pilot_record: Some(
                            GeneralNeighborPilotRecord::OneXAuxiliaryWithTransmitDiversity {
                                qof: 1,
                                walsh_length: 0,
                                aux_walsh: 0x12,
                                aux_td_power_level: 2,
                                td_mode: 1,
                            },
                        ),
                        band_class: None,
                        nghbr_freq: None,
                        zones: Some(vec![UserZoneRecord {
                            uzid: 0x1234,
                            uz_rev: 3,
                            temp_sub: true,
                        }]),
                    }],
                }],
            },
        ));

        match decoded {
            PagingChannelMessage::PrivateNeighborList(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.radio_interfaces.len(), 1);
                assert_eq!(m.radio_interfaces[0].common_nghbr_freq, Some(384));
                assert_eq!(m.radio_interfaces[0].neighbors[0].sid, 42);
                assert_eq!(
                    m.radio_interfaces[0].neighbors[0].zones.as_ref().unwrap()[0].uzid,
                    0x1234
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_private_neighbor_list_rejects_reserved_radio_interface_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0b0001, 4); // RADIO_INTERFACE_TYPE: reserved
        bits.write_u8(0, 8); // RADIO_INTERFACE_LEN

        let err = PrivateNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RADIO_INTERFACE_TYPE"));
    }

    #[test]
    fn common_private_neighbor_list_rejects_nonzero_radio_interface_padding() {
        let mut body = Bitstream::new();
        body.write_u8(0, 1); // COMMON_INCL
        body.write_u8(7, 4); // SRCH_WIN_PN
        body.write_u8(0, 6); // NUM_PRI_NGHBR
        body.write_u8(1, 5); // reserved padding must be zero

        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0, 4); // RADIO_INTERFACE_TYPE: MC
        bits.write_u8(2, 8); // RADIO_INTERFACE_LEN
        bits.extend(&body);

        let err = PrivateNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved padding"));
    }

    #[test]
    fn common_extended_global_service_redirection_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::ExtendedGlobalServiceRedirection(
            ExtendedGlobalServiceRedirectionMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                return_if_fail: true,
                primary: ExtendedGlobalRedirectionTarget {
                    redirect_accolc: 0x8001,
                    delete_tmsi: true,
                    p_rev: Some(RedirectPRevRange {
                        exclude: false,
                        min: 6,
                        max: 12,
                    }),
                    record: ExtendedRedirectionRecord::Cdma {
                        band_class: 5,
                        expected_sid: 42,
                        expected_nid: 65535,
                        cdma_chans: vec![384, 425],
                        redirect_subclasses: Some(vec![true, false, true]),
                    },
                    last_search_record_ind: false,
                },
                additional_records: vec![ExtendedGlobalRedirectionTarget {
                    redirect_accolc: 0x0002,
                    delete_tmsi: false,
                    p_rev: None,
                    record: ExtendedRedirectionRecord::Ds41(vec![0xaa, 0xbb]),
                    last_search_record_ind: true,
                }],
            },
        ));

        match decoded {
            PagingChannelMessage::ExtendedGlobalServiceRedirection(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.config_msg_seq, 9);
                assert!(m.return_if_fail);
                assert_eq!(m.primary.redirect_accolc, 0x8001);
                assert_eq!(
                    m.primary.p_rev,
                    Some(RedirectPRevRange {
                        exclude: false,
                        min: 6,
                        max: 12,
                    })
                );
                match m.primary.record {
                    ExtendedRedirectionRecord::Cdma {
                        cdma_chans,
                        redirect_subclasses,
                        ..
                    } => {
                        assert_eq!(cdma_chans, vec![384, 425]);
                        assert_eq!(redirect_subclasses, Some(vec![true, false, true]));
                    }
                    _ => panic!("unexpected primary redirection record"),
                }
                assert_eq!(m.additional_records.len(), 1);
                assert!(m.additional_records[0].last_search_record_ind);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_extended_global_service_redirection_rejects_invalid_p_rev_range() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(0xffff, 16); // REDIRECT_ACCOLC
        bits.write_u8(0, 1); // RETURN_IF_FAIL
        bits.write_u8(0, 1); // DELETE_TMSI
        bits.write_u8(1, 1); // REDIRECT_P_REV_INCL
        bits.write_u8(0, 1); // EXCL_P_REV_IND
        bits.write_u8(5, 8); // REDIRECT_P_MIN below spec minimum
        bits.write_u8(6, 8); // REDIRECT_P_MAX
        bits.write_u8(0, 8); // RECORD_TYPE: NDSS off
        bits.write_u8(0, 8); // RECORD_LEN
        bits.write_u8(1, 1); // LAST_SEARCH_RECORD_IND
        bits.write_u8(0, 3); // NUM_ADD_RECORD

        let err = ExtendedGlobalServiceRedirectionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("P_REV"));
    }

    #[test]
    fn common_extended_global_service_redirection_rejects_cdma_reserved_bits() {
        let mut record = Bitstream::new();
        record.write_u8(5, 5); // BAND_CLASS
        record.write_u32(42, 15); // EXPECTED_SID
        record.write_u32(65535, 16); // EXPECTED_NID
        record.write_u8(1, 4); // RESERVED must be zero
        record.write_u8(0, 4); // NUM_CHANS
        record.write_u8(0, 1); // SUBCLASS_INFO_INCL
        pad_to_octet(&mut record);
        let record_bytes = bitstream_to_byte_vec(&record);

        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(0xffff, 16); // REDIRECT_ACCOLC
        bits.write_u8(0, 1); // RETURN_IF_FAIL
        bits.write_u8(0, 1); // DELETE_TMSI
        bits.write_u8(0, 1); // REDIRECT_P_REV_INCL
        bits.write_u8(0x02, 8); // RECORD_TYPE: CDMA
        bits.write_u8(record_bytes.len() as u8, 8); // RECORD_LEN
        for byte in record_bytes {
            bits.write_u8(byte, 8);
        }
        bits.write_u8(1, 1); // LAST_SEARCH_RECORD_IND
        bits.write_u8(0, 3); // NUM_ADD_RECORD

        let err = ExtendedGlobalServiceRedirectionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved field"));
    }

    #[test]
    fn common_extended_cdma_channel_list_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::ExtendedCdmaChannelList(
            ExtendedCdmaChannelListMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                cdma_freqs: vec![384, 425],
                rc_qpch_hash_ind: Some(vec![true, false]),
                td_selection: None,
                cdma_band: 5,
                subclasses: Some(vec![true, false, true]),
                cdma_freq_weights: Some(vec![0, 3]),
                additional_bands: vec![ExtendedCdmaAdditionalBand {
                    add_cdma_band: 6,
                    subclasses: Some(vec![true, true]),
                    add_td_mode: None,
                    bypass_sys_det_ind: true,
                    frequencies: vec![ExtendedCdmaAdditionalFrequency {
                        add_cdma_freq: 500,
                        add_rc_qpch_hash_ind: Some(true),
                        add_td_hash_ind: None,
                        add_td_power_level: None,
                        add_cdma_freq_weight: Some(2),
                    }],
                }],
            },
        ));

        match decoded {
            PagingChannelMessage::ExtendedCdmaChannelList(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.cdma_freqs, vec![384, 425]);
                assert_eq!(m.rc_qpch_hash_ind, Some(vec![true, false]));
                assert_eq!(m.subclasses, Some(vec![true, false, true]));
                assert_eq!(m.cdma_freq_weights, Some(vec![0, 3]));
                assert_eq!(m.additional_bands.len(), 1);
                assert_eq!(m.additional_bands[0].frequencies[0].add_cdma_freq, 500);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_extended_cdma_channel_list_rejects_zero_num_freq() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 4); // NUM_FREQ

        let err = ExtendedCdmaChannelListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NUM_FREQ"));
    }

    #[test]
    fn common_extended_cdma_channel_list_rejects_td_selection_on_paging_channel() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_FREQ
        bits.write_u32(384, 11); // CDMA_FREQ
        bits.write_u8(0, 1); // RC_QPCH_SEL_INCL
        bits.write_u8(1, 1); // TD_SEL_INCL forbidden on the Paging Channel

        let err = ExtendedCdmaChannelListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("TD_SEL_INCL"));
    }

    #[test]
    fn common_user_zone_reject_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::UserZoneReject(
            UserZoneRejectMessage {
                reject_uzid: 0x1234,
                reject_action_indi: 0b011,
                assign_uzid: Some(0xabcd),
            },
        ));

        match decoded {
            PagingChannelMessage::UserZoneReject(m) => {
                assert_eq!(m.reject_uzid, 0x1234);
                assert_eq!(m.reject_action_indi, 0b011);
                assert_eq!(m.assign_uzid, Some(0xabcd));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_user_zone_reject_rejects_reserved_action() {
        let mut bits = Bitstream::new();
        bits.write_u32(0x1234, 16); // REJECT_UZID
        bits.write_u8(0b101, 3); // REJECT_ACTION_INDI reserved
        bits.write_u8(0, 1); // UZID_ASSIGN_INCL

        let err = UserZoneRejectMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("REJECT_ACTION_INDI"));
    }

    #[test]
    fn common_ansi41_system_parameters_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::Ansi41SystemParameters(
            Ansi41SystemParametersMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                sid: 42,
                nid: 65535,
                packet_zone_id: 7,
                reg_zone: 123,
                total_zones: 3,
                zone_timer: 4,
                mult_sids: true,
                mult_nids: false,
                home_reg: true,
                for_sid_reg: true,
                for_nid_reg: false,
                power_up_reg: true,
                power_down_reg: false,
                parameter_reg: true,
                reg_prd: 40,
                reg_dist: Some(55),
                delete_for_tmsi: true,
                use_tmsi: false,
                pref_msid_type: 0b10,
                tmsi_zone: vec![0x01, 0x23, 0x45],
                imsi_t_supported: true,
                max_num_alt_so: 4,
                auto_msg_interval: Some(3),
                other_info: Some(Ansi41OtherInfo {
                    base_id: 99,
                    mcc: 310,
                    imsi_11_12: 55,
                    broadcast_gps_asst: true,
                    sig_encrypt_sup: 0b1010_0000,
                }),
                cs_supported: true,
                ms_init_pos_loc_sup_ind: true,
                msg_integrity_sup: true,
                sig_integrity_sup: Some(0),
                imsi_10: Some(7),
                max_add_serv_instance: Some(5),
                tkz_id: Some(0xaa),
                pz_hyst_enabled: true,
                pz_hyst_info: Some(PacketZoneHysteresisInfo {
                    list_len: 3,
                    act_timer: 12,
                    timer_mul: 2,
                    timer_exp: 4,
                }),
                ext_pref_msid_type: 0b01,
                meid_reqd: Some(true),
            },
        ));

        match decoded {
            PagingChannelMessage::Ansi41SystemParameters(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.sid, 42);
                assert_eq!(m.tmsi_zone, vec![0x01, 0x23, 0x45]);
                assert_eq!(m.other_info.unwrap().mcc, 310);
                assert_eq!(m.sig_integrity_sup, Some(0));
                assert_eq!(m.pz_hyst_info.unwrap().timer_exp, 4);
                assert_eq!(m.meid_reqd, Some(true));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_ansi41_system_parameters_rejects_zero_tmsi_zone_len() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(42, 15); // SID
        bits.write_u32(65535, 16); // NID
        bits.write_u8(0, 8); // PACKET_ZONE_ID
        bits.write_u32(123, 12); // REG_ZONE
        bits.write_u8(3, 3); // TOTAL_ZONES
        bits.write_u8(4, 3); // ZONE_TIMER
        bits.write_u8(0, 8); // MULT_* through PARAMETER_REG
        bits.write_u8(0, 7); // REG_PRD
        bits.write_u8(0, 1); // DIST_REG_INCL
        bits.write_u8(0, 1); // DELETE_FOR_TMSI
        bits.write_u8(0, 1); // USE_TMSI
        bits.write_u8(0, 2); // PREF_MSID_TYPE
        bits.write_u8(0, 4); // TMSI_ZONE_LEN

        let err = Ansi41SystemParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("TMSI_ZONE_LEN"));
    }

    #[test]
    fn common_ansi41_system_parameters_rejects_invalid_reg_prd() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(42, 15); // SID
        bits.write_u32(65535, 16); // NID
        bits.write_u8(0, 8); // PACKET_ZONE_ID
        bits.write_u32(123, 12); // REG_ZONE
        bits.write_u8(3, 3); // TOTAL_ZONES
        bits.write_u8(4, 3); // ZONE_TIMER
        bits.write_u8(0, 8); // MULT_* through PARAMETER_REG
        bits.write_u8(28, 7); // REG_PRD reserved: non-zero below 29

        let err = Ansi41SystemParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("REG_PRD"));
    }

    #[test]
    fn common_ansi41_system_parameters_rejects_reserved_msid_selector() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(42, 15); // SID
        bits.write_u32(65535, 16); // NID
        bits.write_u8(0, 8); // PACKET_ZONE_ID
        bits.write_u32(123, 12); // REG_ZONE
        bits.write_u8(3, 3); // TOTAL_ZONES
        bits.write_u8(4, 3); // ZONE_TIMER
        bits.write_u8(0, 8); // MULT_* through PARAMETER_REG
        bits.write_u8(0, 7); // REG_PRD
        bits.write_u8(0, 1); // DIST_REG_INCL
        bits.write_u8(0, 1); // DELETE_FOR_TMSI
        bits.write_u8(1, 1); // USE_TMSI
        bits.write_u8(0b00, 2); // PREF_MSID_TYPE reserved when USE_TMSI=1
        bits.write_u8(1, 4); // TMSI_ZONE_LEN
        bits.write_u8(0x01, 8); // TMSI_ZONE
        bits.write_u8(0, 1); // IMSI_T_SUPPORTED
        bits.write_u8(0, 3); // MAX_NUM_ALT_SO
        bits.write_u8(0, 1); // AUTO_MSG_SUPPORTED
        bits.write_u8(0, 1); // OTHER_INFO_INCL
        bits.write_u8(0, 1); // CS_SUPPORTED
        bits.write_u8(0, 1); // MS_INIT_POS_LOC_SUP_IND
        bits.write_u8(0, 1); // MSG_INTEGRITY_SUP
        bits.write_u8(0, 1); // IMSI_10_INCL
        bits.write_u8(0, 1); // TKZ_MODE_SUPPORTED
        bits.write_u8(0b00, 2); // EXT_PREF_MSID_TYPE

        let err = Ansi41SystemParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved USE_TMSI"));
    }

    #[test]
    fn common_ansi41_system_parameters_rejects_reserved_sig_encrypt_bits() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(42, 15); // SID
        bits.write_u32(65535, 16); // NID
        bits.write_u8(0, 8); // PACKET_ZONE_ID
        bits.write_u32(123, 12); // REG_ZONE
        bits.write_u8(3, 3); // TOTAL_ZONES
        bits.write_u8(4, 3); // ZONE_TIMER
        bits.write_u8(0, 8); // MULT_* through PARAMETER_REG
        bits.write_u8(0, 7); // REG_PRD
        bits.write_u8(0, 1); // DIST_REG_INCL
        bits.write_u8(0, 1); // DELETE_FOR_TMSI
        bits.write_u8(0, 1); // USE_TMSI
        bits.write_u8(0, 2); // PREF_MSID_TYPE
        bits.write_u8(1, 4); // TMSI_ZONE_LEN
        bits.write_u8(0x01, 8); // TMSI_ZONE
        bits.write_u8(0, 1); // IMSI_T_SUPPORTED
        bits.write_u8(0, 3); // MAX_NUM_ALT_SO
        bits.write_u8(0, 1); // AUTO_MSG_SUPPORTED
        bits.write_u8(1, 1); // OTHER_INFO_INCL
        bits.write_u32(99, 16); // BASE_ID
        bits.write_u32(310, 10); // MCC
        bits.write_u8(55, 7); // IMSI_11_12
        bits.write_u8(0, 1); // BROADCAST_GPS_ASST
        bits.write_u8(0b0000_0001, 8); // SIG_ENCRYPT_SUP reserved bit set

        let err = Ansi41SystemParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("SIG_ENCRYPT_SUP"));
    }

    #[test]
    fn common_mc_rr_parameters_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::McRrParameters(
            McRrParametersMessage {
                pilot_pn: 21,
                config_msg_seq: 9,
                base_id: 99,
                p_rev: 12,
                min_p_rev: 6,
                sr3: Some(McRrSr3Parameters {
                    sr3_center_freq: Some(500),
                    sr3_brat: 0b10,
                    sr3_bcch_code_chan: 64,
                    sr3_primary_pilot: 0b01,
                    sr3_pilot_power1: 2,
                    sr3_pilot_power2: 3,
                }),
                srch_win_a: 7,
                srch_win_r: 8,
                t_add: 28,
                t_drop: 32,
                t_comp: 3,
                t_tdrop: 4,
                nghbr_max_age: 5,
                soft_slope: 6,
                add_intercept: 7,
                drop_intercept: 8,
                sig_encrypt_sup: Some(0b1010_0000),
                ui_encrypt_sup: Some(0b0100_0000),
                add_fields: vec![0xaa, 0xbb],
            },
        ));

        match decoded {
            PagingChannelMessage::McRrParameters(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.base_id, 99);
                assert_eq!(m.sr3.unwrap().sr3_center_freq, Some(500));
                assert_eq!(m.sig_encrypt_sup, Some(0b1010_0000));
                assert_eq!(m.add_fields, vec![0xaa, 0xbb]);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_mc_rr_parameters_rejects_reserved_sr3_primary_pilot() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(99, 16); // BASE_ID
        bits.write_u8(12, 8); // P_REV
        bits.write_u8(6, 8); // MIN_P_REV
        bits.write_u8(1, 1); // SR3_INCL
        bits.write_u8(0, 1); // SR3_CENTER_FREQ_INCL
        bits.write_u8(0, 2); // SR3_BRAT
        bits.write_u8(64, 7); // SR3_BCCH_CODE_CHAN
        bits.write_u8(0b11, 2); // SR3_PRIMARY_PILOT reserved

        let err = McRrParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("SR3_PRIMARY_PILOT"));
    }

    #[test]
    fn common_mc_rr_parameters_rejects_add_fields_overrun() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(99, 16); // BASE_ID
        bits.write_u8(12, 8); // P_REV
        bits.write_u8(6, 8); // MIN_P_REV
        bits.write_u8(0, 1); // SR3_INCL
        bits.write_u8(7, 4); // SRCH_WIN_A
        bits.write_u8(8, 4); // SRCH_WIN_R
        bits.write_u8(28, 6); // T_ADD
        bits.write_u8(32, 6); // T_DROP
        bits.write_u8(3, 4); // T_COMP
        bits.write_u8(4, 4); // T_TDROP
        bits.write_u8(5, 4); // NGHBR_MAX_AGE
        bits.write_u8(6, 6); // SOFT_SLOPE
        bits.write_u8(7, 6); // ADD_INTERCEPT
        bits.write_u8(8, 6); // DROP_INTERCEPT
        bits.write_u8(0, 1); // ENC_SUPPORTED
        bits.write_u8(2, 8); // ADD_FIELDS_LEN, but only one octet follows
        bits.write_u8(0xaa, 8);

        let err = McRrParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("ADD_FIELDS"));
    }

    #[test]
    fn common_mc_rr_parameters_rejects_reserved_ui_encrypt_bits() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // CONFIG_MSG_SEQ
        bits.write_u32(99, 16); // BASE_ID
        bits.write_u8(12, 8); // P_REV
        bits.write_u8(6, 8); // MIN_P_REV
        bits.write_u8(0, 1); // SR3_INCL
        bits.write_u8(7, 4); // SRCH_WIN_A
        bits.write_u8(8, 4); // SRCH_WIN_R
        bits.write_u8(28, 6); // T_ADD
        bits.write_u8(32, 6); // T_DROP
        bits.write_u8(3, 4); // T_COMP
        bits.write_u8(4, 4); // T_TDROP
        bits.write_u8(5, 4); // NGHBR_MAX_AGE
        bits.write_u8(6, 6); // SOFT_SLOPE
        bits.write_u8(7, 6); // ADD_INTERCEPT
        bits.write_u8(8, 6); // DROP_INTERCEPT
        bits.write_u8(1, 1); // ENC_SUPPORTED
        bits.write_u8(0b1000_0000, 8); // SIG_ENCRYPT_SUP
        bits.write_u8(0b0000_0001, 8); // UI_ENCRYPT_SUP reserved bit set

        let err = McRrParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("UI_ENCRYPT_SUP"));
    }

    #[test]
    fn common_ansi41_rand_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::Ansi41Rand(Ansi41RandMessage {
            pilot_pn: 21,
            acc_msg_seq: 9,
            rand: 0x1234_abcd,
        }));

        match decoded {
            PagingChannelMessage::Ansi41Rand(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.acc_msg_seq, 9);
                assert_eq!(m.rand, 0x1234_abcd);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_ansi41_rand_rejects_trailing_bits() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // ACC_MSG_SEQ
        bits.write_u32(0x1234_abcd, 32); // RAND
        bits.write_u8(1, 1); // trailing

        let err = Ansi41RandMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("trailing"));
    }

    #[test]
    fn common_enhanced_access_parameters_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::EnhancedAccessParameters(
            EnhancedAccessParametersMessage {
                pilot_pn: 21,
                acc_msg_seq: 9,
                body_bits: minimal_eapm_body_bits(),
            },
        ));

        match decoded {
            PagingChannelMessage::EnhancedAccessParameters(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.acc_msg_seq, 9);
                assert_eq!(m.body_bits, minimal_eapm_body_bits());
                let body = m.body().expect("decode EAPM body");
                assert!(body.psist.is_none());
                assert_eq!(10, body.lac.acc_tmo);
                assert_eq!(1, body.lac.max_req_seq);
                assert_eq!(1, body.lac.max_rsp_seq);
                assert_eq!(1, body.mode_selection_entries.len());
                assert_eq!(0, body.mode_selection_entries[0].access_mode);
                assert_eq!(1, body.mode_parameter_records.len());
                assert!(body.basic_access.is_none());
                assert!(body.reservation_access.is_none());
                assert!(body.acct.is_none());
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_enhanced_access_parameters_rejects_missing_body() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(9, 6); // ACC_MSG_SEQ
        bits.write_u8(0, 4); // truncated body

        let err = EnhancedAccessParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("EAPM body"));
    }

    #[test]
    fn common_enhanced_access_parameters_rejects_lac_reserved_bits() {
        let mut body = minimal_eapm_body_bits();
        body[11] = 1; // RESERVED_1 bit inside the LAC parameter record
        let msg = EnhancedAccessParametersMessage {
            pilot_pn: 21,
            acc_msg_seq: 9,
            body_bits: body,
        };

        let err = msg.body().unwrap_err();

        assert!(err.to_string().contains("RESERVED_1"));
    }

    #[test]
    fn common_enhanced_access_parameters_rejects_reserved_access_mode() {
        let mut body = minimal_eapm_body_bits();
        body[28] = 1; // ACCESS_MODE=010 is reserved
        let msg = EnhancedAccessParametersMessage {
            pilot_pn: 21,
            acc_msg_seq: 9,
            body_bits: body,
        };

        let err = msg.body().unwrap_err();

        assert!(err.to_string().contains("ACCESS_MODE"));
    }

    fn unlm_mc_radio_interface() -> UniversalMcRadioInterface {
        UniversalMcRadioInterface {
            pilot_inc: 3,
            nghbr_srch_mode: 0b01,
            srch_win_n: Some(6),
            srch_offset_incl: false,
            freq_fields_incl: true,
            use_timing: true,
            global_timing: None,
            nghbr_set_entry_info: true,
            nghbr_set_access_info: true,
            neighbors: vec![
                UniversalMcNeighborRecord {
                    nghbr_config: 0b011,
                    nghbr_pn: 77,
                    bcch_support: Some(true),
                    pilot_record: Some(
                        GeneralNeighborPilotRecord::OneXCommonWithTransmitDiversity {
                            td_power_level: 1,
                            td_mode: 2,
                        },
                    ),
                    search_priority: Some(0b10),
                    srch_win_nghbr: None,
                    srch_offset_nghbr: None,
                    nghbr_band: Some(3),
                    nghbr_freq: Some(384),
                    timing: Some(UniversalMcNeighborTiming {
                        tx_offset: 9,
                        tx_duration: Some(3),
                        tx_period: Some(16),
                    }),
                    access_entry_ho: Some(true),
                    access_ho_allowed: Some(false),
                },
                UniversalMcNeighborRecord {
                    nghbr_config: 0b000,
                    nghbr_pn: 78,
                    bcch_support: None,
                    pilot_record: None,
                    search_priority: Some(0b01),
                    srch_win_nghbr: None,
                    srch_offset_nghbr: None,
                    nghbr_band: None,
                    nghbr_freq: None,
                    timing: None,
                    access_entry_ho: Some(false),
                    access_ho_allowed: Some(true),
                },
            ],
            resq: Some(UniversalMcResqParameters {
                delay_time: 4,
                allowed_time: 5,
                attempt_time: 6,
                code_chan: 7,
                qof: 1,
                min_period: Some(8),
                num_tot_trans_20ms: Some(2),
                num_tot_trans_5ms: Some(3),
                num_preamble_rc1_rc2: 4,
                num_preamble: 5,
                power_delta: 0b111,
                neighbor_configured: vec![false, true],
            }),
            pdch_supported: vec![true, false],
        }
    }

    fn unlm_sdu_with_mc_fields(fields: Vec<u8>) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(11, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0, 4); // MC RADIO_INTERFACE_TYPE
        bits.write_u8(fields.len() as u8, 8); // RADIO_INTERFACE_LEN
        bits.extend(&Bitstream::new_bytes(&fields));
        bits
    }

    #[test]
    fn common_universal_neighbor_list_from_sdu_roundtrip() {
        let mc_interface = unlm_mc_radio_interface();
        let decoded = common_roundtrip(PagingChannelMessage::UniversalNeighborList(
            UniversalNeighborListMessage {
                pilot_pn: 42,
                config_msg_seq: 11,
                radio_interfaces: vec![
                    UniversalRadioInterfaceRecord::mc(&mc_interface),
                    UniversalRadioInterfaceRecord::Hrpd {
                        neighbors: vec![
                            HrpdNeighborRecord {
                                nghbr_pn: 12,
                                nghbr_band: None,
                                nghbr_freq: None,
                                pn_association_ind: true,
                                data_association_ind: false,
                            },
                            HrpdNeighborRecord {
                                nghbr_pn: 511,
                                nghbr_band: Some(3),
                                nghbr_freq: Some(384),
                                pn_association_ind: false,
                                data_association_ind: true,
                            },
                        ],
                    },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::UniversalNeighborList(m) => {
                assert_eq!(m.pilot_pn, 42);
                assert_eq!(m.config_msg_seq, 11);
                assert_eq!(m.radio_interfaces.len(), 2);
                match &m.radio_interfaces[0] {
                    UniversalRadioInterfaceRecord::Mc { fields } => {
                        assert_eq!(fields, &mc_interface.to_fields());
                        let parsed = m.radio_interfaces[0]
                            .mc_fields()
                            .unwrap()
                            .expect("MC fields");
                        assert_eq!(parsed.pilot_inc, 3);
                        assert_eq!(parsed.neighbors.len(), 2);
                        assert_eq!(parsed.neighbors[0].nghbr_config, 0b011);
                        assert_eq!(parsed.neighbors[0].bcch_support, Some(true));
                        assert_eq!(parsed.neighbors[1].search_priority, Some(0b01));
                        assert_eq!(
                            parsed.resq.as_ref().unwrap().neighbor_configured,
                            vec![false, true]
                        );
                        assert_eq!(parsed.pdch_supported, vec![true, false]);
                    }
                    _ => panic!("unexpected UNLM radio-interface record"),
                }
                match &m.radio_interfaces[1] {
                    UniversalRadioInterfaceRecord::Hrpd { neighbors } => {
                        assert_eq!(neighbors.len(), 2);
                        assert_eq!(neighbors[0].nghbr_pn, 12);
                        assert_eq!(neighbors[0].nghbr_band, None);
                        assert_eq!(neighbors[0].nghbr_freq, None);
                        assert!(neighbors[0].pn_association_ind);
                        assert!(!neighbors[0].data_association_ind);
                        assert_eq!(neighbors[1].nghbr_pn, 511);
                        assert_eq!(neighbors[1].nghbr_band, Some(3));
                        assert_eq!(neighbors[1].nghbr_freq, Some(384));
                        assert!(!neighbors[1].pn_association_ind);
                        assert!(neighbors[1].data_association_ind);
                    }
                    _ => panic!("unexpected UNLM radio-interface record"),
                }
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_universal_neighbor_list_rejects_reserved_interface_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(11, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(1, 4); // reserved RADIO_INTERFACE_TYPE
        bits.write_u8(1, 8); // RADIO_INTERFACE_LEN
        bits.write_u8(0, 8);

        let err = UniversalNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved RADIO_INTERFACE_TYPE"));
    }

    #[test]
    fn common_universal_neighbor_list_rejects_interface_overrun() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(11, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0, 4); // MC RADIO_INTERFACE_TYPE
        bits.write_u8(2, 8); // RADIO_INTERFACE_LEN
        bits.write_u8(0xaa, 8);

        let err = UniversalNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("exceeds remaining SDU"));
    }

    #[test]
    fn common_universal_neighbor_list_rejects_mc_zero_pilot_inc() {
        let mut fields = unlm_mc_radio_interface().to_fields();
        fields[0] &= 0x0f; // PILOT_INC is the high nibble of the MC body.
        let mut bits = unlm_sdu_with_mc_fields(fields);

        let err = UniversalNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("PILOT_INC"));
    }

    #[test]
    fn common_universal_neighbor_list_rejects_mc_reserved_neighbor_config() {
        let mut body = Bitstream::new();
        body.write_u8(1, 4); // PILOT_INC
        body.write_u8(0b00, 2); // NGHBR_SRCH_MODE: common SRCH_WIN_N only
        body.write_u8(4, 4); // SRCH_WIN_N
        body.write_u8(0, 1); // SRCH_OFFSET_INCL
        body.write_u8(0, 1); // FREQ_FIELDS_INCL
        body.write_u8(0, 1); // USE_TIMING
        body.write_u8(0, 1); // NGHBR_SET_ENTRY_INFO
        body.write_u8(0, 1); // NGHBR_SET_ACCESS_INFO
        body.write_u8(1, 6); // NUM_NGHBR
        body.write_u8(0b101, 3); // reserved NGHBR_CONFIG
        body.write_u32(12, 9); // NGHBR_PN
        body.write_u8(0, 1); // ADD_PILOT_REC_INCL
        body.write_u8(0, 1); // RESQ_ENABLED
        body.write_u8(0, 1); // NGHBR_PDCH_SUPPORTED
        pad_to_octet(&mut body);
        let mut bits = unlm_sdu_with_mc_fields(body.to_packed_bytes());

        let err = UniversalNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NGHBR_CONFIG"));
    }

    #[test]
    fn common_universal_neighbor_list_rejects_mc_nonzero_padding() {
        let mut fields = unlm_mc_radio_interface().to_fields();
        let last = fields.last_mut().expect("MC body");
        *last |= 0x01;
        let mut bits = unlm_sdu_with_mc_fields(fields);

        let err = UniversalNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved padding"));
    }

    #[test]
    fn common_universal_neighbor_list_rejects_hrpd_reserved_bits() {
        let mut bits = Bitstream::new();
        bits.write_u32(42, 9); // PILOT_PN
        bits.write_u8(11, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(2, 4); // HRPD RADIO_INTERFACE_TYPE
        bits.write_u8(1, 8); // RADIO_INTERFACE_LEN
        bits.write_u8(0, 6); // NUM_HRPD_NGHBR
        bits.write_u8(0b10, 2); // non-zero RESERVED

        let err = UniversalNeighborListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RESERVED bits"));
    }

    #[test]
    fn common_security_mode_command_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::SecurityModeCommand(
            SecurityModeCommandMessage {
                c_sig_encrypt_mode: 0b010,
                enc_key_size: Some(0b010),
                change_keys: Some(true),
                use_uak: Some(false),
            },
        ));

        match decoded {
            PagingChannelMessage::SecurityModeCommand(m) => {
                assert_eq!(m.c_sig_encrypt_mode, 0b010);
                assert_eq!(m.enc_key_size, Some(0b010));
                assert_eq!(m.change_keys, Some(true));
                assert_eq!(m.use_uak, Some(false));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_security_mode_command_roundtrips_no_optional_fields() {
        let decoded = common_roundtrip(PagingChannelMessage::SecurityModeCommand(
            SecurityModeCommandMessage {
                c_sig_encrypt_mode: 0,
                enc_key_size: None,
                change_keys: None,
                use_uak: None,
            },
        ));

        match decoded {
            PagingChannelMessage::SecurityModeCommand(m) => {
                assert_eq!(m.c_sig_encrypt_mode, 0);
                assert_eq!(m.enc_key_size, None);
                assert_eq!(m.change_keys, None);
                assert_eq!(m.use_uak, None);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_security_mode_command_rejects_reserved_encrypt_mode() {
        let mut bits = Bitstream::new();
        bits.write_u8(0b011, 3); // reserved C_SIG_ENCRYPT_MODE
        bits.write_u8(0, 1); // MSG_INT_INFO_INCL

        let err = SecurityModeCommandMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved C_SIG_ENCRYPT_MODE"));
    }

    #[test]
    fn common_security_mode_command_rejects_reserved_key_size() {
        let mut bits = Bitstream::new();
        bits.write_u8(0b001, 3); // C_SIG_ENCRYPT_MODE
        bits.write_u8(0b000, 3); // reserved ENC_KEY_SIZE
        bits.write_u8(0, 1); // MSG_INT_INFO_INCL

        let err = SecurityModeCommandMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved ENC_KEY_SIZE"));
    }

    #[test]
    fn common_security_mode_command_rejects_trailing_bits() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 3); // C_SIG_ENCRYPT_MODE
        bits.write_u8(0, 1); // MSG_INT_INFO_INCL
        bits.write_u8(1, 1); // trailing

        let err = SecurityModeCommandMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("trailing"));
    }

    #[test]
    fn common_universal_page_from_sdu_roundtrip() {
        let block = UniversalPageBlock {
            addresses: UniversalPageInterleavedAddresses {
                broadcasts: vec![UniversalPageBroadcastAddress {
                    burst_type: 0b000011,
                    address_bits: 0x1234,
                }],
                imsis: vec![UniversalPagePartialAddress {
                    address_bits: 0xabcd,
                }],
                tmsis: vec![UniversalPagePartialAddress {
                    address_bits: 0x0fed,
                }],
            },
            records: vec![
                UniversalPageRecord::EnhancedBroadcast {
                    addr_len: 3,
                    bc_addr_remainder: vec![0x55],
                    bcn: 2,
                    time_offset: 77,
                    repeat_time_offset: Some(4),
                    add_record: vec![0xaa],
                },
                UniversalPageRecord::MobileStation {
                    address_type: UniversalPageMobileAddressType::Class0 {
                        imsi_s_33_16: 0x2345,
                        imsi_11_12: Some(12),
                        mcc: None,
                    },
                    msg_seq: 5,
                    service_option: 6,
                    add_record: vec![0x01, 0x02],
                },
                UniversalPageRecord::MobileStation {
                    address_type: UniversalPageMobileAddressType::Tmsi {
                        tmsi_zone: Some(vec![0x12]),
                        tmsi_code_addr_31_16: Some(0x3456),
                        tmsi_code_addr_23_16: None,
                    },
                    msg_seq: 4,
                    service_option: 33,
                    add_record: vec![],
                },
            ],
        };
        let decoded = common_roundtrip(PagingChannelMessage::UniversalPage(
            UniversalPageMessage::from_page_block(17, 9, true, false, &block),
        ));

        match decoded {
            PagingChannelMessage::UniversalPage(m) => {
                assert_eq!(m.config_msg_seq, 17);
                assert_eq!(m.acc_msg_seq, 9);
                assert!(m.read_next_slot);
                assert!(!m.read_next_slot_bcast);
                assert_eq!(m.page_block().unwrap(), block);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_universal_page_segments_from_sdu_roundtrip() {
        let first = common_roundtrip(PagingChannelMessage::UniversalPageFirstSegment(
            UniversalPageSegmentMessage {
                upm_segment_seq: None,
                segment_bits: vec![1, 1, 0, 0],
            },
        ));
        let middle = common_roundtrip(PagingChannelMessage::UniversalPageMiddleSegment(
            UniversalPageSegmentMessage {
                upm_segment_seq: Some(2),
                segment_bits: vec![0, 1, 0],
            },
        ));
        let final_segment = common_roundtrip(PagingChannelMessage::UniversalPageFinalSegment(
            UniversalPageSegmentMessage {
                upm_segment_seq: Some(3),
                segment_bits: vec![1, 0, 1],
            },
        ));

        match first {
            PagingChannelMessage::UniversalPageFirstSegment(m) => {
                assert_eq!(m.upm_segment_seq, None);
                assert_eq!(m.segment_bits, vec![1, 1, 0, 0]);
            }
            _ => panic!("unexpected decoded message"),
        }
        match middle {
            PagingChannelMessage::UniversalPageMiddleSegment(m) => {
                assert_eq!(m.upm_segment_seq, Some(2));
                assert_eq!(m.segment_bits, vec![0, 1, 0]);
            }
            _ => panic!("unexpected decoded message"),
        }
        match final_segment {
            PagingChannelMessage::UniversalPageFinalSegment(m) => {
                assert_eq!(m.upm_segment_seq, Some(3));
                assert_eq!(m.segment_bits, vec![1, 0, 1]);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_universal_page_rejects_truncated_common_fields() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 6); // ACC_MSG_SEQ
        bits.write_u8(0, 1); // READ_NEXT_SLOT

        let err = UniversalPageMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("common fields truncated"));
    }

    #[test]
    fn common_universal_page_rejects_reserved_address_type() {
        let mut block = Bitstream::new();
        block.write_u8(0, 1); // BCAST_INCLUDED
        block.write_u8(0, 1); // IMSI_INCLUDED
        block.write_u8(0, 1); // TMSI_INCLUDED
        block.write_u8(1, 1); // RESERVED_TYPE_INCLUDED
        block.write_u8(0, 6); // NUM_RESERVED_TYPE

        let err = UniversalPageBlock::from_bits(block.bits()).unwrap_err();

        assert!(err.to_string().contains("reserved address types"));
    }

    #[test]
    fn common_universal_page_rejects_tmsi_zone_len_zero() {
        let mut block = Bitstream::new();
        block.write_u8(0, 1); // BCAST_INCLUDED
        block.write_u8(0, 1); // IMSI_INCLUDED
        block.write_u8(1, 1); // TMSI_INCLUDED
        block.write_u8(0, 6); // NUM_TMSI = one record
        block.write_u8(0, 1); // RESERVED_TYPE_INCLUDED
        for _ in 0..16 {
            block.write_u8(0, 1); // TMSI_CODE_ADDR_BIT
        }
        block.write_u8(0b10, 2); // PAGE_CLASS: TMSI
        block.write_u8(0b11, 2); // PAGE_SUBCLASS: TMSI zone included
        block.write_u8(0, 3); // MSG_SEQ
        block.write_u8(0, 4); // invalid TMSI_ZONE_LEN

        let err = UniversalPageBlock::from_bits(block.bits()).unwrap_err();

        assert!(err.to_string().contains("TMSI_ZONE_LEN"));
    }

    #[test]
    fn common_universal_page_rejects_empty_first_segment() {
        let mut bits = Bitstream::new();

        let err = UniversalPageSegmentMessage::from_first_segment_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("first segment body is empty"));
    }

    #[test]
    fn common_universal_page_rejects_truncated_continuation_segment() {
        let mut bits = Bitstream::new();
        bits.write_u8(1, 2); // UPM_SEGMENT_SEQ but no segment bits

        let err = UniversalPageSegmentMessage::from_middle_segment_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("middle segment body truncated"));
    }

    #[test]
    fn common_universal_page_rejects_middle_segment_seq_three() {
        let mut bits = Bitstream::new();
        bits.write_u8(0b11, 2); // reserved for middle segment
        bits.write_u8(1, 1); // segment body

        let err = UniversalPageSegmentMessage::from_middle_segment_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("UPM_SEGMENT_SEQ"));
    }

    #[test]
    fn common_authentication_request_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::AuthenticationRequest(
            AuthenticationRequestMessage {
                randa: (0..16).collect(),
                con_sqn: vec![0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5],
                amf: [0x12, 0x34],
                mac_a: vec![0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7],
            },
        ));

        match decoded {
            PagingChannelMessage::AuthenticationRequest(m) => {
                assert_eq!(m.randa, (0..16).collect::<Vec<u8>>());
                assert_eq!(m.con_sqn, vec![0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5]);
                assert_eq!(m.amf, [0x12, 0x34]);
                assert_eq!(
                    m.mac_a,
                    vec![0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7]
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_authentication_request_rejects_truncated_body() {
        let mut bits = Bitstream::new();
        for _ in 0..31 {
            bits.write_u8(0, 8);
        }

        let err = AuthenticationRequestMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn common_authentication_request_rejects_trailing_bits() {
        let mut bits = Bitstream::new();
        for _ in 0..32 {
            bits.write_u8(0, 8);
        }
        bits.write_u8(1, 1);

        let err = AuthenticationRequestMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("trailing"));
    }

    #[test]
    fn common_alternative_technologies_information_from_sdu_roundtrip() {
        let hrpd = AlternativeHrpdRadioInterface {
            subnet_color_code: Some(0x22),
            neighbors: vec![
                AlternativeHrpdNeighborRecord {
                    nghbr_pn: 12,
                    freq_same_as_prev: false,
                    nghbr_band: Some(3),
                    nghbr_freq: Some(384),
                    pn_association_ind: true,
                    data_association_ind: false,
                    subnet_color_code: AlternativeHrpdNeighborSubnetColorCode::SameAsCommon,
                },
                AlternativeHrpdNeighborRecord {
                    nghbr_pn: 13,
                    freq_same_as_prev: true,
                    nghbr_band: None,
                    nghbr_freq: None,
                    pn_association_ind: false,
                    data_association_ind: true,
                    subnet_color_code: AlternativeHrpdNeighborSubnetColorCode::Explicit(0x44),
                },
            ],
        };
        let decoded = common_roundtrip(PagingChannelMessage::AlternativeTechnologiesInformation(
            AlternativeTechnologiesInformationMessage {
                pilot_pn: 21,
                config_msg_seq: 17,
                radio_interfaces: vec![
                    AlternativeTechnologyRadioInterfaceRecord::hrpd(&hrpd),
                    AlternativeTechnologyRadioInterfaceRecord::Eutran { fields: vec![0xcc] },
                    AlternativeTechnologyRadioInterfaceRecord::Wimax { fields: vec![] },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::AlternativeTechnologiesInformation(m) => {
                assert_eq!(m.pilot_pn, 21);
                assert_eq!(m.config_msg_seq, 17);
                assert_eq!(m.radio_interfaces.len(), 3);
                match &m.radio_interfaces[0] {
                    AlternativeTechnologyRadioInterfaceRecord::Hrpd { fields } => {
                        assert_eq!(fields, &hrpd.to_fields());
                        let parsed = m.radio_interfaces[0]
                            .hrpd_fields()
                            .unwrap()
                            .expect("HRPD fields");
                        assert_eq!(parsed, hrpd);
                    }
                    _ => panic!("unexpected ATIM radio-interface record"),
                }
                match &m.radio_interfaces[1] {
                    AlternativeTechnologyRadioInterfaceRecord::Eutran { fields } => {
                        assert_eq!(fields, &vec![0xcc]);
                    }
                    _ => panic!("unexpected ATIM radio-interface record"),
                }
                match &m.radio_interfaces[2] {
                    AlternativeTechnologyRadioInterfaceRecord::Wimax { fields } => {
                        assert!(fields.is_empty());
                    }
                    _ => panic!("unexpected ATIM radio-interface record"),
                }
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    fn atim_sdu_with_hrpd_fields(fields: Vec<u8>) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(17, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0b0010, 4); // HRPD RADIO_INTERFACE_TYPE
        bits.write_u32(fields.len() as u32, 10); // RADIO_INTERFACE_LEN
        bits.extend(&Bitstream::new_bytes(&fields));
        bits
    }

    #[test]
    fn common_alternative_technologies_information_rejects_reserved_interface_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(17, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0b0001, 4); // reserved RADIO_INTERFACE_TYPE
        bits.write_u32(0, 10); // RADIO_INTERFACE_LEN

        let err = AlternativeTechnologiesInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved RADIO_INTERFACE_TYPE"));
    }

    #[test]
    fn common_alternative_technologies_information_rejects_interface_overrun() {
        let mut bits = Bitstream::new();
        bits.write_u32(21, 9); // PILOT_PN
        bits.write_u8(17, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_RADIO_INTERFACE
        bits.write_u8(0b0010, 4); // HRPD RADIO_INTERFACE_TYPE
        bits.write_u32(2, 10); // RADIO_INTERFACE_LEN
        bits.write_u8(0xaa, 8);

        let err = AlternativeTechnologiesInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("exceeds remaining SDU"));
    }

    #[test]
    fn common_alternative_technologies_information_rejects_hrpd_common_reserved_bits() {
        let mut fields = Bitstream::new();
        fields.write_u8(0, 4); // COMMON_RECORD_LEN: one octet
        fields.write_u8(0, 1); // SUBNET_COLOR_CODE_INCL
        fields.write_u8(0b100, 3); // non-zero COMMON_RECORD_RESERVED
        let mut bits = atim_sdu_with_hrpd_fields(fields.to_packed_bytes());

        let err = AlternativeTechnologiesInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved bits"));
    }

    #[test]
    fn common_alternative_technologies_information_rejects_hrpd_reserved_subnet_indicator() {
        let mut fields = Bitstream::new();
        fields.write_u8(0, 4); // COMMON_RECORD_LEN: one octet
        fields.write_u8(0, 1); // SUBNET_COLOR_CODE_INCL
        fields.write_u8(0, 3); // COMMON_RECORD_RESERVED
        fields.write_u8(1, 6); // NUM_HRPD_NGHBR
        fields.write_u8(3, 5); // HRPD_NGHBR_REC_LEN: four octets
        fields.write_u32(12, 9); // NGHBR_PN
        fields.write_u8(0, 1); // NGHBR_FREQ_SAME_AS_PREV
        fields.write_u8(3, 5); // NGHBR_BAND
        fields.write_u32(384, 11); // NGHBR_FREQ
        fields.write_u8(1, 1); // PN_ASSOCIATION_IND
        fields.write_u8(0, 1); // DATA_ASSOCIATION_IND
        fields.write_u8(0b11, 2); // reserved NGHBR_SUBNET_COLOR_CODE_IND
        pad_to_octet(&mut fields);
        let mut bits = atim_sdu_with_hrpd_fields(fields.to_packed_bytes());

        let err = AlternativeTechnologiesInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(
            err.to_string()
                .contains("reserved NGHBR_SUBNET_COLOR_CODE_IND")
        );
    }

    #[test]
    fn common_forward_general_extension_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::GeneralExtension(
            ForwardGeneralExtensionMessage {
                records: vec![
                    ForwardGeneralExtensionRecord::ReverseChannelInfo {
                        band_class: 3,
                        rev_chan: 384,
                    },
                    ForwardGeneralExtensionRecord::RadioConfigurationParameters {
                        fields: vec![0xaa, 0xbb],
                    },
                ],
                message_type: 0x15,
                message_rec_bits: vec![1, 0, 1, 1],
            },
        ));

        match decoded {
            PagingChannelMessage::GeneralExtension(m) => {
                assert_eq!(m.records.len(), 2);
                match &m.records[0] {
                    ForwardGeneralExtensionRecord::ReverseChannelInfo {
                        band_class,
                        rev_chan,
                    } => {
                        assert_eq!(*band_class, 3);
                        assert_eq!(*rev_chan, 384);
                    }
                    _ => panic!("unexpected GEM record"),
                }
                match &m.records[1] {
                    ForwardGeneralExtensionRecord::RadioConfigurationParameters { fields } => {
                        assert_eq!(fields, &vec![0xaa, 0xbb]);
                    }
                    _ => panic!("unexpected GEM record"),
                }
                assert_eq!(m.message_type, 0x15);
                assert_eq!(m.message_rec_bits, vec![1, 0, 1, 1]);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_forward_general_extension_rejects_empty_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(0, 8); // NUM_GE_REC

        let err = ForwardGeneralExtensionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NUM_GE_REC"));
    }

    #[test]
    fn common_forward_general_extension_rejects_reserved_record_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(1, 8); // NUM_GE_REC
        bits.write_u8(2, 8); // reserved GE_REC_TYPE
        bits.write_u8(0, 8); // GE_REC_LEN
        bits.write_u8(0x15, 8); // MESSAGE_TYPE

        let err = ForwardGeneralExtensionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved GE_REC_TYPE"));
    }

    #[test]
    fn common_forward_general_extension_rejects_forward_message_type_pd_bits() {
        let mut bits = Bitstream::new();
        bits.write_u8(1, 8); // NUM_GE_REC
        bits.write_u8(0, 8); // reverse-channel GE_REC_TYPE
        bits.write_u8(2, 8); // GE_REC_LEN
        bits.write_u8(3, 5); // BAND_CLASS
        bits.write_u32(384, 11); // REV_CHAN
        bits.write_u8(0b0101_0101, 8); // invalid f-csch MESSAGE_TYPE PD bits

        let err = ForwardGeneralExtensionMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("MESSAGE_TYPE"));
    }

    #[test]
    fn common_general_overhead_information_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::GeneralOverheadInformation(
            GeneralOverheadInformationMessage {
                pilot_pn: 17,
                config_msg_seq: 21,
                records: vec![
                    GeneralOverheadInformationRecord::OperatorName {
                        fields: cdma_7bit_text_fields("OK"),
                    },
                    GeneralOverheadInformationRecord::CellName {
                        fields: cdma_7bit_text_fields("A"),
                    },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::GeneralOverheadInformation(m) => {
                assert_eq!(m.pilot_pn, 17);
                assert_eq!(m.config_msg_seq, 21);
                assert_eq!(m.records.len(), 2);
                match &m.records[0] {
                    GeneralOverheadInformationRecord::OperatorName { fields } => {
                        assert_eq!(fields, &cdma_7bit_text_fields("OK"));
                        let text = m.records[0].text_fields().expect("decode operator text");
                        assert_eq!(0x02, text.msg_encoding);
                        assert_eq!(2, text.num_fields);
                        assert_eq!("OK", text.text);
                    }
                    _ => panic!("unexpected GOIM record"),
                }
                match &m.records[1] {
                    GeneralOverheadInformationRecord::CellName { fields } => {
                        assert_eq!(fields, &cdma_7bit_text_fields("A"));
                        let text = m.records[1].text_fields().expect("decode cell text");
                        assert_eq!("A", text.text);
                    }
                    _ => panic!("unexpected GOIM record"),
                }
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_general_overhead_information_rejects_empty_records() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 4); // NUM_GOI_REC

        let err = GeneralOverheadInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("NUM_GOI_REC"));
    }

    #[test]
    fn common_general_overhead_information_rejects_reserved_record_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_GOI_REC
        bits.write_u8(2, 8); // reserved GOI_REC_TYPE
        bits.write_u8(2, 8); // GOI_REC_LEN
        bits.write_u8(0x08, 8);
        bits.write_u8(0x40, 8);

        let err = GeneralOverheadInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved GOI_REC_TYPE"));
    }

    #[test]
    fn common_general_overhead_information_rejects_record_overrun() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 4); // NUM_GOI_REC
        bits.write_u8(0, 8); // GOI_REC_TYPE
        bits.write_u8(3, 8); // GOI_REC_LEN
        bits.write_u8(0x08, 8);
        bits.write_u8(0x40, 8);

        let err = GeneralOverheadInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("exceeds remaining SDU"));
    }

    #[test]
    fn common_access_point_identifier_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::AccessPointIdentifier(
            AccessPointIdentifierMessage {
                pilot_pn: 17,
                config_msg_seq: 21,
                asstn_type: 0b010,
                sid: 42,
                nid: 65535,
                ap_id: vec![0x1234, 0xabcd],
                ap_id_mask: 24,
                ios_msc_id: 0x00ab_cdef,
                ios_cell_id: 0x4567,
                hrpd_acquisition: Some(AccessPointHrpdAcquisitionRecord {
                    hrpd_pn: 84,
                    hrpd_band_class: 7,
                    hrpd_channel: 777,
                }),
                location: AccessPointLocationRecord::BaseStation {
                    base_lat: -12345,
                    base_long: 23456,
                    loc_unc_h: 3,
                    base_height: 600,
                    loc_unc_v: 4,
                },
                intra_freq_ho_hys: Some(12),
                intra_freq_ho_slope: Some(7),
                inter_freq_ho_hys: Some(10),
                inter_freq_ho_slope: Some(5),
                inter_freq_srch_th: Some(9),
            },
        ));

        match decoded {
            PagingChannelMessage::AccessPointIdentifier(m) => {
                assert_eq!(m.pilot_pn, 17);
                assert_eq!(m.config_msg_seq, 21);
                assert_eq!(m.asstn_type, 0b010);
                assert_eq!(m.ap_id, vec![0x1234, 0xabcd]);
                assert_eq!(m.ap_id_mask, 24);
                assert_eq!(m.ios_msc_id, 0x00ab_cdef);
                assert_eq!(m.ios_cell_id, 0x4567);
                assert_eq!(
                    m.hrpd_acquisition,
                    Some(AccessPointHrpdAcquisitionRecord {
                        hrpd_pn: 84,
                        hrpd_band_class: 7,
                        hrpd_channel: 777,
                    })
                );
                assert_eq!(
                    m.location,
                    AccessPointLocationRecord::BaseStation {
                        base_lat: -12345,
                        base_long: 23456,
                        loc_unc_h: 3,
                        base_height: 600,
                        loc_unc_v: 4,
                    }
                );
                assert_eq!(m.intra_freq_ho_slope, Some(7));
                assert_eq!(m.inter_freq_srch_th, Some(9));
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_access_point_identifier_rejects_reserved_association_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0b011, 3); // ASSTN_TYPE reserved

        let err = AccessPointIdentifierMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("ASSTN_TYPE"));
    }

    #[test]
    fn common_access_point_identifier_rejects_slope_without_hysteresis() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 3); // ASSTN_TYPE
        bits.write_u32(42, 15); // SID
        bits.write_u32(65535, 16); // NID
        bits.write_u8(0, 4); // AP_ID_LEN
        bits.write_u8(0, 8); // AP_ID_MASK
        bits.write_u32(0xabcdef, 24); // IOS_MSC_ID
        bits.write_u32(0x4567, 16); // IOS_CELL_ID
        bits.write_u8(0, 1); // HRPD_ACQ_REC_INCL
        bits.write_u8(0, 3); // LOC_REC_TYPE
        bits.write_u8(0, 5); // LOC_REC_LEN
        bits.write_u8(0, 1); // INTRA_FREQ_HO_HYS_INCL
        bits.write_u8(1, 1); // INTRA_FREQ_HO_SLOPE_INCL
        bits.write_u8(7, 6); // INTRA_FREQ_HO_SLOPE

        let err = AccessPointIdentifierMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("INTRA_FREQ_HO_SLOPE"));
    }

    #[test]
    fn common_access_point_identifier_rejects_location_reserved_bits() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 3); // ASSTN_TYPE
        bits.write_u32(42, 15); // SID
        bits.write_u32(65535, 16); // NID
        bits.write_u8(0, 4); // AP_ID_LEN
        bits.write_u8(0, 8); // AP_ID_MASK
        bits.write_u32(0xabcdef, 24); // IOS_MSC_ID
        bits.write_u32(0x4567, 16); // IOS_CELL_ID
        bits.write_u8(0, 1); // HRPD_ACQ_REC_INCL
        bits.write_u8(1, 3); // LOC_REC_TYPE
        bits.write_u8(9, 5); // LOC_REC_LEN
        bits.write_u32(0, 22); // BASE_LAT
        bits.write_u32(0, 23); // BASE_LONG
        bits.write_u8(0, 4); // LOC_UNC_H
        bits.write_u32(0, 14); // BASE_HEIGHT
        bits.write_u8(0, 4); // LOC_UNC_V
        bits.write_u8(1, 5); // reserved bits must be zero

        let err = AccessPointIdentifierMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("reserved bits"));
    }

    #[test]
    fn common_access_point_identifier_text_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::AccessPointIdentifierText(
            AccessPointIdentifierTextMessage {
                pilot_pn: 17,
                config_msg_seq: 21,
                ap_id_text: cdma_7bit_text_fields("AP"),
            },
        ));

        match decoded {
            PagingChannelMessage::AccessPointIdentifierText(m) => {
                assert_eq!(m.pilot_pn, 17);
                assert_eq!(m.config_msg_seq, 21);
                assert_eq!(m.ap_id_text, cdma_7bit_text_fields("AP"));
                let text = m.text_fields().expect("decode AP ID text");
                assert_eq!(0x02, text.msg_encoding);
                assert_eq!(2, text.num_fields);
                assert_eq!("AP", text.text);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_access_point_identifier_text_rejects_nonzero_text_padding() {
        let mut ap_id_text = Bitstream::new();
        ap_id_text.write_u8(0x02, 5); // MSG_ENCODING = C.R1001 7-bit ASCII
        ap_id_text.write_u8(1, 8); // NUM_FIELDS
        ap_id_text.write_u8(b'A', 7);
        ap_id_text.write_u8(1, 4); // reserved padding must be zero

        let msg = AccessPointIdentifierTextMessage {
            pilot_pn: 17,
            config_msg_seq: 21,
            ap_id_text: ap_id_text.to_packed_bytes(),
        };

        let err = msg.text_fields().unwrap_err();

        assert!(err.to_string().contains("reserved padding"));
    }

    #[test]
    fn common_access_point_identifier_text_rejects_short_text_record() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(1, 8); // AP_ID_TEXT_LEN
        bits.write_u8(0x08, 8);

        let err = AccessPointIdentifierTextMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("AP_ID_TEXT"));
    }

    #[test]
    fn common_access_point_identifier_text_rejects_text_overrun() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(3, 8); // AP_ID_TEXT_LEN
        bits.write_u8(0x08, 8);
        bits.write_u8(0x41, 8);

        let err = AccessPointIdentifierTextMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("exceeds remaining SDU"));
    }

    #[test]
    fn common_access_point_pilot_information_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::AccessPointPilotInformation(
            AccessPointPilotInformationMessage {
                pilot_pn: 17,
                config_msg_seq: 21,
                lifetime: 30,
                records: vec![
                    AccessPointPilotInformationRecord {
                        ap_assn_type: 0,
                        sid: 42,
                        nid: 65535,
                        band: 7,
                        freq: 384,
                        pn_record: AccessPointPilotPnRecord::List { pns: vec![84, 126] },
                    },
                    AccessPointPilotInformationRecord {
                        ap_assn_type: 0b111,
                        sid: 42,
                        nid: 65535,
                        band: 7,
                        freq: 384,
                        pn_record: AccessPointPilotPnRecord::Series {
                            count: 4,
                            start: 168,
                            inc: 2,
                        },
                    },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::AccessPointPilotInformation(m) => {
                assert_eq!(m.pilot_pn, 17);
                assert_eq!(m.config_msg_seq, 21);
                assert_eq!(m.lifetime, 30);
                assert_eq!(m.records.len(), 2);
                assert_eq!(m.records[0].sid, 42);
                assert_eq!(m.records[1].nid, 65535);
                assert_eq!(m.records[1].band, 7);
                assert_eq!(m.records[1].freq, 384);
                assert_eq!(
                    m.records[1].pn_record,
                    AccessPointPilotPnRecord::Series {
                        count: 4,
                        start: 168,
                        inc: 2,
                    }
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_access_point_pilot_information_rejects_first_record_same_previous() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u32(30, 16); // LIFETIME
        bits.write_u32(1, 9); // NUM_APPI_REC
        bits.write_u8(0, 3); // AP_ASSN_TYPE
        bits.write_u8(1, 1); // AP_SID_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_NID_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_BAND_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_FREQ_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_PN_REC_SAME_AS_PREVIOUS

        let err = AccessPointPilotInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("same-as-previous"));
    }

    #[test]
    fn common_access_point_pilot_information_rejects_reserved_pn_record_type() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u32(30, 16); // LIFETIME
        bits.write_u32(1, 9); // NUM_APPI_REC
        bits.write_u8(0, 3); // AP_ASSN_TYPE
        bits.write_u8(0, 1); // AP_SID_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_NID_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_BAND_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_FREQ_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_PN_REC_SAME_AS_PREVIOUS
        bits.write_u32(42, 15); // AP_SID
        bits.write_u32(65535, 16); // AP_NID
        bits.write_u8(7, 5); // AP_BAND
        bits.write_u32(384, 11); // AP_FREQ
        bits.write_u8(0b010, 3); // reserved AP_PN_REC_TYPE
        bits.write_u8(0, 5); // AP_PN_REC_LEN

        let err = AccessPointPilotInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("AP_PN_REC_TYPE"));
    }

    #[test]
    fn common_access_point_pilot_information_rejects_record_reserved_bits() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u32(30, 16); // LIFETIME
        bits.write_u32(1, 9); // NUM_APPI_REC
        bits.write_u8(0, 3); // AP_ASSN_TYPE
        bits.write_u8(0, 1); // AP_SID_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_NID_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_BAND_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_FREQ_SAME_AS_PREVIOUS
        bits.write_u8(0, 1); // AP_PN_REC_SAME_AS_PREVIOUS
        bits.write_u32(42, 15); // AP_SID
        bits.write_u32(65535, 16); // AP_NID
        bits.write_u8(7, 5); // AP_BAND
        bits.write_u32(384, 11); // AP_FREQ
        bits.write_u8(0b001, 3); // AP_PN_REC_TYPE series
        bits.write_u8(3, 5); // AP_PN_REC_LEN
        bits.write_u8(4, 8); // AP_PN_COUNT
        bits.write_u32(168, 9); // AP_PN_START
        bits.write_u8(2, 4); // AP_PN_INC
        bits.write_u8(0, 3); // AP_PN_REC reserved
        bits.write_u8(1, 1); // APPI_REC reserved bit must be zero

        let err = AccessPointPilotInformationMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("APPI_REC reserved bits"));
    }

    #[test]
    fn common_flex_duplex_cdma_channel_list_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::FlexDuplexCdmaChannelList(
            FlexDuplexCdmaChannelListMessage {
                pilot_pn: 17,
                config_msg_seq: 21,
                cand_band_info_req: true,
                candidate_bands: vec![
                    FlexDuplexCandidateBand {
                        cand_band_class: 3,
                        subclasses: Some(vec![true, false, true]),
                        bypass_sys_det_ind: true,
                        frequencies: vec![
                            FlexDuplexFrequencyRecord {
                                cdma_freq: 384,
                                remaining: Some(FlexDuplexRemainingFields {
                                    rev_cdma_freq: 425,
                                    rc_qpch_hash_ind: Some(true),
                                    cdma_freq_weight: Some(2),
                                }),
                            },
                            FlexDuplexFrequencyRecord {
                                cdma_freq: 777,
                                remaining: None,
                            },
                        ],
                    },
                    FlexDuplexCandidateBand {
                        cand_band_class: 7,
                        subclasses: None,
                        bypass_sys_det_ind: false,
                        frequencies: vec![FlexDuplexFrequencyRecord {
                            cdma_freq: 512,
                            remaining: Some(FlexDuplexRemainingFields {
                                rev_cdma_freq: 513,
                                rc_qpch_hash_ind: Some(false),
                                cdma_freq_weight: Some(0),
                            }),
                        }],
                    },
                ],
            },
        ));

        match decoded {
            PagingChannelMessage::FlexDuplexCdmaChannelList(m) => {
                assert_eq!(m.pilot_pn, 17);
                assert_eq!(m.config_msg_seq, 21);
                assert!(m.cand_band_info_req);
                assert_eq!(m.candidate_bands.len(), 2);
                assert_eq!(
                    m.candidate_bands[0].subclasses,
                    Some(vec![true, false, true])
                );
                assert_eq!(m.candidate_bands[0].frequencies.len(), 2);
                assert_eq!(
                    m.candidate_bands[1].frequencies[0].remaining,
                    Some(FlexDuplexRemainingFields {
                        rev_cdma_freq: 513,
                        rc_qpch_hash_ind: Some(false),
                        cdma_freq_weight: Some(0),
                    })
                );
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_flex_duplex_cdma_channel_list_rejects_td_on_paging_channel() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 1); // CAND_BAND_INFO_REQ
        bits.write_u8(0, 1); // RC_QPCH_SEL_INCL
        bits.write_u8(1, 1); // TD_SEL_INCL forbidden on Paging Channel

        let err = FlexDuplexCdmaChannelListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("TD_SEL_INCL"));
    }

    #[test]
    fn common_flex_duplex_cdma_channel_list_rejects_rc_qpch_without_hash_target() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // CONFIG_MSG_SEQ
        bits.write_u8(0, 1); // CAND_BAND_INFO_REQ
        bits.write_u8(1, 1); // RC_QPCH_SEL_INCL
        bits.write_u8(0, 1); // TD_SEL_INCL
        bits.write_u8(0, 1); // CDMA_FREQ_WEIGHT_INCL
        bits.write_u8(0, 3); // NUM_CAND_BAND_CLASS = 1 record
        bits.write_u8(3, 5); // CAND_BAND_CLASS
        bits.write_u8(0, 1); // BYPASS_SYS_DET_IND
        bits.write_u8(1, 4); // NUM_FREQ
        bits.write_u32(384, 11); // CDMA_FREQ
        bits.write_u8(1, 1); // REMAINING_FIELD_INCL
        bits.write_u32(425, 11); // REV_CDMA_FREQ
        bits.write_u8(0, 1); // RC_QPCH_HASH_IND, no target selected

        let err = FlexDuplexCdmaChannelListMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("RC_QPCH_SEL_INCL"));
    }

    fn minimal_valid_bspm_body() -> Bitstream {
        let mut body = Bitstream::new();
        body.write_u8(3, 4); // BSPM_COMMON_RECORD_LEN = 4 octets including this field
        body.write_u8(0, 1); // DIFF_BSPM
        body.write_u8(0, 1); // AUTO_REQ_ALLOWED_IND
        body.write_u8(0, 1); // FREQ_CHG_REG_REQUIRED
        body.write_u8(0, 1); // REGISTRATION_REQ_FLAG_INCL
        body.write_u8(0, 1); // BCMC_ON_TRAFFIC_SUP
        body.write_u8(0, 1); // AUTH_SIGNATURE_REQUIRED
        body.write_u8(0, 7); // NUM_FSCH
        body.write_u8(0, 2); // FSCH_PLCM_SCHEME_IND
        body.write_u8(0, 8); // NUM_BCMC_PROGRAMS = 1 program
        body.write_u8(0, 1); // USE_TIME
        body.write_u8(0, 2); // FRAMING_TYPE
        body.write_u8(0, 2); // BSPM_COMMON_RECORD_RESERVED

        body.write_u8(0, 5); // BCMC_PROGRAM_ID_LEN = one bit
        body.write_u8(1, 1); // BCMC_PROGRAM_ID
        body.write_u8(0, 3); // BCMC_FLOW_DISCRIMINATOR_LEN
        body.write_u8(0, 4); // header record length = 1 octet including this field
        body.write_u8(0, 1); // FLOW_INFO_ON_OTHER_FREQ
        body.write_u8(0, 3); // NUM_LPM_ENTRIES

        body.write_u8(0, 3); // BCMC_NUM_BCCH_NGHBR
        body
    }

    #[test]
    fn common_broadcast_service_parameters_from_sdu_roundtrip() {
        let body_bits = minimal_valid_bspm_body();
        let decoded = common_roundtrip(PagingChannelMessage::BroadcastServiceParameters(
            BroadcastServiceParametersMessage {
                pilot_pn: 17,
                bspm_msg_seq: 21,
                body_bits: body_bits.clone(),
            },
        ));

        match decoded {
            PagingChannelMessage::BroadcastServiceParameters(m) => {
                assert_eq!(m.pilot_pn, 17);
                assert_eq!(m.bspm_msg_seq, 21);
                assert_eq!(m.body_bits.bits(), body_bits.bits());
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_broadcast_service_parameters_rejects_common_length_overrun() {
        let mut bits = Bitstream::new();
        bits.write_u32(17, 9); // PILOT_PN
        bits.write_u8(21, 6); // BSPM_MSG_SEQ
        bits.write_u8(15, 4); // BSPM_COMMON_RECORD_LEN too large for remaining SDU

        let err = BroadcastServiceParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("common record length exceeds"));
    }

    #[test]
    fn common_broadcast_service_parameters_rejects_reserved_common_values() {
        let mut body = minimal_valid_bspm_body();
        body.drain(0..4); // remove original common length
        let mut invalid = Bitstream::new();
        invalid.write_u8(3, 4); // BSPM_COMMON_RECORD_LEN
        invalid.write_u8(0, 1); // DIFF_BSPM
        invalid.write_u8(0, 1); // AUTO_REQ_ALLOWED_IND
        invalid.write_u8(0, 1); // FREQ_CHG_REG_REQUIRED
        invalid.write_u8(0, 1); // REGISTRATION_REQ_FLAG_INCL
        invalid.write_u8(0, 1); // BCMC_ON_TRAFFIC_SUP
        invalid.write_u8(0, 1); // AUTH_SIGNATURE_REQUIRED
        invalid.write_u8(0, 7); // NUM_FSCH
        invalid.write_u8(0b11, 2); // reserved FSCH_PLCM_SCHEME_IND
        invalid.extend(&body);

        let mut bits = Bitstream::new();
        bits.write_u32(17, 9);
        bits.write_u8(21, 6);
        bits.extend(&invalid);

        let err = BroadcastServiceParametersMessage::from_sdu(&mut bits).unwrap_err();

        assert!(err.to_string().contains("FSCH_PLCM_SCHEME_IND"));
    }

    #[test]
    fn common_external_spec_forward_messages_return_unsupported_errors() {
        for message_id in [
            MessageId::McMapSyncChannel,
            MessageId::McMapSystemInformation,
            MessageId::McmapL3,
            MessageId::RTmsiAssignment,
            MessageId::McMapFlowRelease,
            MessageId::MeidExtChannelAssignment,
        ] {
            let mut bits = Bitstream::new();
            let err = PagingChannelMessage::from_sdu(message_id, &mut bits).unwrap_err();

            assert!(err.to_string().contains("unsupported f-csch body decode"));
            assert!(err.to_string().contains(message_id.tag()));
        }
    }

    #[test]
    fn common_system_parameters_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::SystemParameters(
            SystemParametersMessage {
                pilot_pn: 17,
                config_msg_seq: 23,
                sid: 22,
                nid: 65535,
                reg_zone: 3,
                total_zones: 2,
                zone_timer: 5,
                mult_sids: true,
                mult_nids: false,
                base_id: 41,
                base_class: 1,
                page_chan: 1,
                max_slot_cycle_index: 2,
                home_reg: true,
                for_sid_reg: true,
                for_nid_reg: true,
                power_up_reg: true,
                power_down_reg: false,
                parameter_reg: true,
                reg_prd: 7,
                base_lat: 0x155555,
                base_long: 0x2aaaaa,
                reg_dist: 123,
                srch_win_a: 8,
                srch_win_n: 10,
                srch_win_r: 10,
                nghbr_max_age: 4,
                pwr_rep_thresh: 7,
                pwr_rep_frames: 12,
                pwr_thresh_enable: true,
                pwr_period_enable: false,
                pwr_rep_delay: 3,
                rescan: true,
                t_add: 28,
                t_drop: 32,
                t_comp: 5,
                t_tdrop: 3,
                ext_sys_parameter: true,
                ext_nghbr_lst: true,
                gen_nghbr_lst: false,
                global_redirect: false,
                pri_nghbr_lst: true,
                user_zone_id: false,
                ext_global_redirect: true,
                ext_chan_lst: true,
                t_tdrop_range_incl: false,
                t_tdrop_range: 0,
                neg_slot_cycle_index_sup: false,
                crrm_msg_ind: false,
                num_opt_msg_bits: 0,
                ap_pilot_info: false,
                ap_idt: false,
                ap_id_text: false,
                gen_ovhd_inf_ind: false,
                fd_chan_lst_ind: false,
                atim_ind: false,
                appim_period_index: 0,
                gen_ovhd_cycle_index: 0,
                atim_cycle_index: 0,
                add_loc_info_incl: false,
            },
        ));
        match decoded {
            PagingChannelMessage::SystemParameters(m) => {
                assert_eq!(m.sid, 22);
                assert_eq!(m.nid, 65535);
                assert!(m.ext_sys_parameter);
                assert!(m.ext_chan_lst);
                assert!(!m.t_tdrop_range_incl);
                assert!(!m.neg_slot_cycle_index_sup);
                assert!(!m.crrm_msg_ind);
                assert_eq!(m.num_opt_msg_bits, 0);
                assert!(!m.add_loc_info_incl);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_system_parameters_from_sdu_roundtrip_with_t_tdrop_range() {
        let decoded = common_roundtrip(PagingChannelMessage::SystemParameters(
            SystemParametersMessage {
                pilot_pn: 0,
                config_msg_seq: 1,
                sid: 1,
                nid: 0,
                reg_zone: 0,
                total_zones: 1,
                zone_timer: 0,
                mult_sids: false,
                mult_nids: false,
                base_id: 1,
                base_class: 0,
                page_chan: 1,
                max_slot_cycle_index: 0,
                home_reg: true,
                for_sid_reg: true,
                for_nid_reg: true,
                power_up_reg: true,
                power_down_reg: false,
                parameter_reg: false,
                reg_prd: 0,
                base_lat: 0,
                base_long: 0,
                reg_dist: 0,
                srch_win_a: 0,
                srch_win_n: 0,
                srch_win_r: 0,
                nghbr_max_age: 0,
                pwr_rep_thresh: 0,
                pwr_rep_frames: 0,
                pwr_thresh_enable: false,
                pwr_period_enable: false,
                pwr_rep_delay: 0,
                rescan: false,
                t_add: 0,
                t_drop: 0,
                t_comp: 0,
                t_tdrop: 0,
                ext_sys_parameter: false,
                ext_nghbr_lst: false,
                gen_nghbr_lst: false,
                global_redirect: false,
                pri_nghbr_lst: false,
                user_zone_id: false,
                ext_global_redirect: false,
                ext_chan_lst: false,
                t_tdrop_range_incl: true,
                t_tdrop_range: 0b1010,
                neg_slot_cycle_index_sup: true,
                crrm_msg_ind: true,
                num_opt_msg_bits: 6,
                ap_pilot_info: false,
                ap_idt: false,
                ap_id_text: false,
                gen_ovhd_inf_ind: false,
                fd_chan_lst_ind: false,
                atim_ind: true,
                appim_period_index: 0,
                gen_ovhd_cycle_index: 0,
                atim_cycle_index: 0,
                add_loc_info_incl: false,
            },
        ));
        match decoded {
            PagingChannelMessage::SystemParameters(m) => {
                assert!(m.t_tdrop_range_incl);
                assert_eq!(m.t_tdrop_range, 0b1010);
                assert!(m.neg_slot_cycle_index_sup);
                assert!(m.crrm_msg_ind);
                assert_eq!(m.num_opt_msg_bits, 6);
                assert!(!m.ap_pilot_info);
                assert!(!m.fd_chan_lst_ind);
                assert!(m.atim_ind);
                assert_eq!(m.atim_cycle_index, 0);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_access_parameters_from_sdu_roundtrip_with_optional_records() {
        let decoded = common_roundtrip(PagingChannelMessage::AccessParameters(
            AccessParametersMessage {
                pilot_pn: 9,
                acc_msg_seq: 4,
                acc_chan: 1,
                nom_pwr: -8,
                init_pwr: -4,
                pwr_step: 3,
                num_step: 5,
                max_cap_sz: 6,
                pam_sz: 12,
                psist_0_9: 0x15,
                psist_10: 1,
                psist_11: 2,
                psist_12: 3,
                psist_13: 4,
                psist_14: 5,
                psist_15: 6,
                msg_psist: 7,
                reg_psist: 6,
                probe_pn_ran: 9,
                acc_tmo: 3,
                probe_bkoff: 4,
                bkoff: 5,
                max_req_seq: 6,
                max_rsp_seq: 7,
                auth: 1,
                rand: 0x1234_5678,
                nom_pwr_ext: 1,
                psist_emg_incl: true,
                psist_emg: 5,
                acct_incl: true,
                acct_incl_emg: true,
                acct_aoc_bitmap_incl: true,
                acct_so_records: vec![AcctServiceOptionRecord {
                    aoc_bitmap: 0b10101,
                    service_option: 3,
                }],
                acct_so_grp_records: vec![AcctServiceOptionGroupRecord {
                    aoc_bitmap: 0b01010,
                    service_option_group: 7,
                }],
            },
        ));
        match decoded {
            PagingChannelMessage::AccessParameters(m) => {
                assert_eq!(m.nom_pwr, -8);
                assert_eq!(m.init_pwr, -4);
                assert_eq!(m.rand, 0x1234_5678);
                assert_eq!(m.psist_emg, 5);
                assert_eq!(m.acct_so_records[0].service_option, SERVICE_OPTION_EVRC_A);
                assert_eq!(m.acct_so_grp_records[0].service_option_group, 7);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_neighbor_and_cdma_channel_lists_from_sdu_roundtrip() {
        let decoded = common_roundtrip(PagingChannelMessage::NeighborList(NeighborListMessage {
            pilot_pn: 3,
            config_msg_seq: 5,
            pilot_inc: 2,
            neighbors: vec![1, 33, 511],
        }));
        match decoded {
            PagingChannelMessage::NeighborList(m) => assert_eq!(m.neighbors, vec![1, 33, 511]),
            _ => panic!("unexpected decoded message"),
        }

        let decoded = common_roundtrip(PagingChannelMessage::CdmaChannelList(
            CdmaChannelListMessage {
                pilot_pn: 3,
                config_msg_seq: 5,
                channels: vec![384, 425, 2047],
            },
        ));
        match decoded {
            PagingChannelMessage::CdmaChannelList(m) => {
                assert_eq!(m.channels, vec![384, 425, 2047])
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_extended_system_parameters_from_sdu_roundtrip_with_optional_fields() {
        let decoded = common_roundtrip(PagingChannelMessage::ExtendedSystemParameters(
            ExtendedSystemParametersMessage {
                pilot_pn: 4,
                config_msg_seq: 23,
                delete_for_tmsi: false,
                use_tmsi: true,
                pref_msid_type: 3,
                mcc: 310,
                imsi_11_12: 0x7f,
                tmsi_zone: vec![0, 1],
                bcast_index: 2,
                imsi_t_supported: true,
                p_rev: 6,
                min_p_rev: 6,
                soft_slope: 7,
                add_intercept: 8,
                drop_intercept: 9,
                packet_zone_id: 1,
                max_num_alt_so: 7,
                reselect_included: true,
                ec_thresh: 12,
                ec_io_thresh: 13,
                pilot_report: true,
                nghbr_set_entry_info: true,
                acc_ent_ho_order: true,
                nghbr_set_access_info: true,
                access_ho: true,
                access_ho_msg_rsp: true,
                access_probe_ho: true,
                acc_ho_list_upd: true,
                acc_probe_ho_other_msg: false,
                max_num_probe_ho: 3,
                nghbr_set_size: 2,
                access_entry_ho: vec![true, false],
                access_ho_allowed: vec![false, true],
                broadcast_gps_asst: true,
                qpch_supported: true,
                num_qpch: 2,
                qpch_rate: 1,
                qpch_power_level_page: 5,
                qpch_cci_supported: true,
                qpch_power_level_config: 6,
                sdb_supported: true,
                rlgain_traffic_pilot: 17,
                rev_pwr_cntl_delay_incl: true,
                rev_pwr_cntl_delay: 2,
                auto_msg_supported: true,
                auto_msg_interval: 5,
                mob_qos: true,
                enc_supported: true,
                sig_encrypt_sup: 0xaa,
                ui_encrypt_sup: 0x55,
                use_sync_id: true,
                cs_supported: true,
                bcch_supported: false,
                ms_init_pos_loc_sup_ind: true,
                pilot_info_req_supported: true,
                ext_pref_msid_type: None,
                meid_reqd: None,
            },
        ));
        match decoded {
            PagingChannelMessage::ExtendedSystemParameters(m) => {
                assert_eq!(m.tmsi_zone, vec![0, 1]);
                assert_eq!(m.ec_thresh, 12);
                assert_eq!(m.access_entry_ho, vec![true, false]);
                assert_eq!(m.access_ho_allowed, vec![false, true]);
                assert_eq!(m.sig_encrypt_sup, 0xaa);
                assert_eq!(m.ui_encrypt_sup, 0x55);
            }
            _ => panic!("unexpected decoded message"),
        }
    }

    #[test]
    fn common_extended_system_parameters_roundtrip_with_meid_request() {
        let decoded = common_roundtrip(PagingChannelMessage::ExtendedSystemParameters(
            ExtendedSystemParametersMessage {
                pilot_pn: 4,
                config_msg_seq: 23,
                delete_for_tmsi: false,
                use_tmsi: false,
                pref_msid_type: 3,
                mcc: 310,
                imsi_11_12: 55,
                tmsi_zone: vec![0],
                bcast_index: 0,
                imsi_t_supported: false,
                p_rev: 11,
                min_p_rev: 3,
                soft_slope: 0,
                add_intercept: 0,
                drop_intercept: 0,
                packet_zone_id: 0,
                max_num_alt_so: 0,
                reselect_included: false,
                ec_thresh: 0,
                ec_io_thresh: 0,
                pilot_report: false,
                nghbr_set_entry_info: false,
                acc_ent_ho_order: false,
                nghbr_set_access_info: false,
                access_ho: false,
                access_ho_msg_rsp: false,
                access_probe_ho: false,
                acc_ho_list_upd: false,
                acc_probe_ho_other_msg: false,
                max_num_probe_ho: 0,
                nghbr_set_size: 0,
                access_entry_ho: Vec::new(),
                access_ho_allowed: Vec::new(),
                broadcast_gps_asst: false,
                qpch_supported: false,
                num_qpch: 0,
                qpch_rate: 0,
                qpch_power_level_page: 0,
                qpch_cci_supported: false,
                qpch_power_level_config: 0,
                sdb_supported: false,
                rlgain_traffic_pilot: 0,
                rev_pwr_cntl_delay_incl: false,
                rev_pwr_cntl_delay: 0,
                auto_msg_supported: false,
                auto_msg_interval: 0,
                mob_qos: false,
                enc_supported: false,
                sig_encrypt_sup: 0,
                ui_encrypt_sup: 0,
                use_sync_id: false,
                cs_supported: false,
                bcch_supported: false,
                ms_init_pos_loc_sup_ind: false,
                pilot_info_req_supported: false,
                ext_pref_msid_type: Some(1),
                meid_reqd: Some(true),
            },
        ));
        match decoded {
            PagingChannelMessage::ExtendedSystemParameters(m) => {
                assert_eq!(m.ext_pref_msid_type, Some(1));
                assert_eq!(m.meid_reqd, Some(true));
            }
            _ => panic!("unexpected decoded message"),
        }
    }
}

#[cfg(test)]
mod escam_tests {
    use super::*;

    fn bits_to_u16(bits: &[u8]) -> u16 {
        bits.iter()
            .fold(0u16, |value, bit| (value << 1) | u16::from(*bit))
    }

    fn make_escam_19k2(w32_code: u16, pilot_pn: u16) -> EscamParams {
        EscamParams {
            start_time_unit: 0,
            for_sch_id: 0,
            sccl_index: 0,
            for_sch_num_bits_idx: 0x1, // 360 bits = 19.2 kbps
            pilot_pn,
            code_chan_sch: w32_code,
            qof_mask_id_sch: 0,
            for_sch_duration: 0x0F, // infinite
            for_sch_start_time_incl: true,
            for_sch_start_time: 0,
            fpc_incl: true,
            fpc_mode_sch: 0,
            fpc_sch_init_setpt_op: 0,
            fpc_sch_fer: 0b00010,   // 1% FER
            fpc_sch_init_setpt: 48, // 6.0 dB
            fpc_sch_min_setpt: 0,
            fpc_sch_max_setpt: 80, // 10.0 dB
        }
    }

    #[test]
    fn escam_encode_immediate_activation() {
        let params = make_escam_19k2(5, 0);
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty(), "ESCAM SDU should not be empty");
        // Full ESCAM with FPC should be substantial
        assert!(
            sdu.len() >= 8,
            "ESCAM SDU should be at least 8 bytes, got {}",
            sdu.len()
        );
    }

    #[test]
    fn escam_encode_with_start_time() {
        let mut params = make_escam_19k2(12, 9);
        params.for_sch_duration = 5;
        params.for_sch_start_time = 10;
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty());
        let without_start = make_escam_19k2(12, 9).encode_sdu();
        assert!(sdu.len() >= without_start.len());
    }

    #[test]
    fn escam_encode_release_sch() {
        let mut params = make_escam_19k2(5, 0);
        params.for_sch_duration = 0; // stop
        params.for_sch_start_time_incl = false;
        params.fpc_incl = false;
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty());
    }

    #[test]
    fn escam_bitstream_has_correct_leading_fields() {
        let params = make_escam_19k2(5, 0);
        let bs = params.to_ftch_sdu();
        let bits = bs.bits();
        // START_TIME_UNIT = 000 (3 bits)
        assert_eq!(bits[0], 0);
        assert_eq!(bits[1], 0);
        assert_eq!(bits[2], 0);
        // REV_SCH_DTX_DURATION = 0000 (4 bits)
        assert_eq!(bits[3], 0);
        // USE_T_ADD_ABORT = 0
        assert_eq!(bits[7], 0);
        // USE_SCRM_SEQ_NUM = 0
        assert_eq!(bits[8], 0);
        // ADD_INFO_INCL = 0
        assert_eq!(bits[9], 0);
        // REV_CFG_INCLUDED = 0
        assert_eq!(bits[10], 0);
        // NUM_REV_SCH = 00
        assert_eq!(bits[11], 0);
        assert_eq!(bits[12], 0);
        // FOR_CFG_INCLUDED = 1
        assert_eq!(bits[13], 1);
    }

    #[test]
    fn escam_code_chan_sch_is_11_bits() {
        let mut params = make_escam_19k2(300, 0);
        params.code_chan_sch = 300;
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty());
    }

    #[test]
    fn escam_assignment_start_time_required_for_nonzero_duration() {
        let params = make_escam_19k2(5, 0);
        let sdu = params.to_ftch_sdu();
        let bits = sdu.bits();

        assert_eq!(&bits[55..57], &[0, 1], "NUM_FOR_SCH = 1");
        assert_eq!(&bits[58..62], &[1, 1, 1, 1], "infinite duration");
        assert_eq!(bits[62], 1, "FOR_SCH_START_TIME_INCL must be 1");
        assert_eq!(&bits[63..68], &[0, 0, 0, 0, 0], "FOR_SCH_START_TIME = 0");
    }

    #[test]
    fn escam_fpc_mode_zero_omits_fpc_sec_chan() {
        let params = make_escam_19k2(5, 0);
        let sdu = params.to_ftch_sdu();
        let bits = sdu.bits();

        assert_eq!(bits[72], 1, "FPC_INCL");
        assert_eq!(&bits[73..76], &[0, 0, 0], "FPC_MODE_SCH = 000");
        assert_eq!(bits[76], 0, "FPC_SCH_INIT_SETPT_OP = 0");
        assert_eq!(&bits[77..79], &[0, 1], "NUM_SUP follows init-setpoint op");
        assert_eq!(bits[79], 0, "SCH_ID = SCH0");
        assert_eq!(&bits[80..85], &[0, 0, 0, 1, 0], "FPC_SCH_FER = 2");
    }

    #[test]
    fn escam_includes_sch_bcmc_ind_when_forward_sch_is_assigned() {
        let params = make_escam_19k2(6, 0);
        let sdu = params.to_ftch_sdu();
        let bits = sdu.bits();

        assert_eq!(bits.len(), 116, "ESCAM SDU bit length");
        assert_eq!(
            bits[113], 0,
            "FOR_SCH_CC_INCL = 0, using Service Connect SCH config"
        );
        assert_eq!(bits[114], 0, "REV_SCH_CC_INCL = 0");
        assert_eq!(bits[115], 0, "SCH_BCMC_IND = 0");
    }

    #[test]
    fn escam_assignment_rate_matches_requested_profile() {
        let mut params = make_escam_19k2(6, 0);
        params.for_sch_num_bits_idx = 0x2;
        let sdu = params.to_ftch_sdu();
        let bits = sdu.bits();

        assert_eq!(
            &bits[25..29],
            &[0, 0, 1, 0],
            "FOR_SCH_NUM_BITS_IDX = 38.4 kbps"
        );
        assert_eq!(bits[113], 0, "ESCAM does not duplicate SCH MAX_RATE");
    }

    #[test]
    fn ecam_and_escam_use_same_pilot_pn_units() {
        let pilot_pn = 0x12;
        let fch_walsh = 10;
        let sch_walsh = 6;

        let ecam = ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(
            pilot_pn, fch_walsh, 0, 3, 3, false,
        );
        let mut ecam_sdu = ecam.try_to_sdu().unwrap();
        let decoded_ecam = ExtendedChannelAssignmentMessage::from_sdu(&mut ecam_sdu).unwrap();

        assert_eq!(decoded_ecam.pilots.len(), 1);
        assert_eq!(decoded_ecam.pilots[0].pilot_pn, pilot_pn);
        assert_eq!(decoded_ecam.pilots[0].code_chan_fch, u16::from(fch_walsh));

        let escam = make_escam_19k2(sch_walsh, pilot_pn);
        let escam_sdu = escam.to_ftch_sdu();
        let escam_bits = escam_sdu.bits();

        assert_eq!(
            bits_to_u16(&escam_bits[32..41]),
            decoded_ecam.pilots[0].pilot_pn,
            "ESCAM PILOT_PN must reference the ECAM active-set PN"
        );
        assert_eq!(
            bits_to_u16(&escam_bits[42..53]),
            sch_walsh,
            "ESCAM CODE_CHAN_SCH is the W32 SCH code channel"
        );
    }

    #[test]
    #[should_panic(expected = "PILOT_PN exceeds 9 bits")]
    fn escam_rejects_pilot_pn_out_of_range() {
        let params = make_escam_19k2(5, 512);
        params.to_ftch_sdu();
    }

    #[test]
    #[should_panic(expected = "FOR_SCH_START_TIME_INCL must be 1")]
    fn escam_rejects_nonzero_duration_without_start_time() {
        let mut params = make_escam_19k2(5, 0);
        params.for_sch_start_time_incl = false;
        params.to_ftch_sdu();
    }

    // ---- select_imsi_class0_forward_address (core function) ----
    //
    // These tests validate the pure OTA compression function that
    // operates on fully-resolved IMSI fields.  The caller resolves
    // None→overhead; these tests only exercise the compression logic.
    //
    // Spec references:
    //   C.S0004-E Table 2.1.1.3.1.1-2 — IMSI_CLASS_0_TYPE encodings
    //   C.S0004-E 3.1.2.2.1.3.3        — BS forward address selection
    //   C.S0005-E 2.6.2.2.5            — ESPM wildcard rules

    #[test]
    fn core_type00_both_match_non_wildcard_overhead() {
        // Home subscriber: MCC=310, IMSI_11_12=15, overhead=(310,15).
        // Both match → type 00 (IMSI_S only).
        let addr = select_imsi_class0_forward_address(100, 200, 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn core_type00_both_wildcard_overhead() {
        // Any MCC/IMSI_11_12 is implied by wildcard overhead → type 00.
        let addr = select_imsi_class0_forward_address(100, 200, 450, 42);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn core_type01_mcc_implied_imsi_11_12_differs() {
        // MCC matches overhead, IMSI_11_12 differs → type 01.
        let addr = select_imsi_class0_forward_address(100, 200, 310, 42);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn core_type10_mcc_differs_imsi_11_12_implied() {
        // Per C.S0004-E 2.1.1.3.1.3 IMSI_CLASS_0_TYPE='10':
        // Roaming mobile (MCC=450) on cell with MCC=310.
        // IMSI_11_12 implied by wildcard → type 10 (IMSI_S + MCC).
        let addr = select_imsi_class0_forward_address(100, 200, 450, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn core_type10_mcc_differs_imsi_11_12_matches() {
        // Roamer MCC differs, IMSI_11_12 matches non-wildcard overhead.
        let addr = select_imsi_class0_forward_address(100, 200, 450, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn core_type11_both_differ() {
        // Roaming mobile: both MCC and IMSI_11_12 differ → type 11.
        let addr = select_imsi_class0_forward_address(100, 200, 450, 42);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn core_forward_address_from_access_fields_resolves_none_to_overhead() {
        // forward_address_from_access_fields resolves None→overhead
        // before calling the core function.  Class-0 mobile omits both
        // (None,None) on a non-wildcard cell (310,15) → type 00.
        let addr = forward_address_from_access_fields(
            Some(0),
            Some(100),
            Some(200),
            None,
            None,
            Some(999),
            310,
            15,
        );
        assert_eq!(
            addr,
            Some(MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            })
        );
    }

    #[test]
    fn core_forward_address_from_access_fields_roamer_sends_mcc() {
        // Roamer sends MCC=450 explicitly (type 10 or 11), omits IMSI_11_12.
        let addr = forward_address_from_access_fields(
            Some(0),
            Some(100),
            Some(200),
            Some(450),
            None,
            Some(999),
            310,
            0x7f,
        );
        assert_eq!(
            addr,
            Some(MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 0x7f,
            })
        );
    }

    #[test]
    fn ecam_assign_mode_100_granted_restore_and_encryption_roundtrip() {
        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(7, 11, 2, 3, 3, true);
        ecam.granted_mode = 0b11;
        ecam.sr_id_restore = Some(0);
        ecam.sr_id_restore_bitmap = Some(0b101010);
        ecam.encrypt_mode = 0b11;
        ecam.d_sig_encrypt_mode = Some(0b010);
        ecam.enc_key_size = Some(0b001);
        ecam.c_sig_encrypt_mode = Some(0b010);
        ecam.one_xrl_freq_offset = Some(0b01);
        ecam.message_integrity = Some(EcamMessageIntegrityInfo {
            change_keys: true,
            use_uak: false,
        });
        ecam.plcm_type_incl = true;
        ecam.plcm_type = 0b0001;
        ecam.plcm_39 = Some(0x12345);
        ecam.sync_id = Some(vec![0xaa, 0x55]);
        ecam.direct_ch_assign_ind = true;
        ecam.config_msg_seq = Some(0x2a);
        ecam.rtc_nom_pwr = Some(-3);
        ecam.respond_ind = Some(true);
        ecam.direct_ch_assign_recover_ind = Some(true);
        ecam.fixed_num_preamble = Some(0b011);
        ecam.tx_pwr_limit = Some(45);

        let encoded = ecam.try_to_sdu().unwrap();
        let mut decode_bits = encoded.clone();
        let decoded = ExtendedChannelAssignmentMessage::from_sdu(&mut decode_bits).unwrap();
        assert_eq!(decoded.granted_mode, 0b11);
        assert_eq!(decoded.sr_id_restore, Some(0));
        assert_eq!(decoded.sr_id_restore_bitmap, Some(0b101010));
        assert_eq!(decoded.d_sig_encrypt_mode, Some(0b010));
        assert_eq!(decoded.enc_key_size, Some(0b001));
        assert_eq!(decoded.c_sig_encrypt_mode, Some(0b010));
        assert_eq!(decoded.one_xrl_freq_offset, Some(0b01));
        assert_eq!(decoded.sync_id, Some(vec![0xaa, 0x55]));
        assert_eq!(decoded.rtc_nom_pwr, Some(-3));
        assert_eq!(decoded.tx_pwr_limit, Some(45));
        assert_eq!(decoded.to_sdu().bits(), encoded.bits());
    }

    #[test]
    fn ecam_dcch_only_channel_record_roundtrip() {
        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(3, 0, 1, 3, 3, false);
        ecam.ch_ind = 0b10;
        ecam.fpc_dcch_init_setpt = 0x21;
        ecam.fpc_dcch_fer = 0b00010;
        ecam.fpc_dcch_min_setpt = 0x01;
        ecam.fpc_dcch_max_setpt = 0x52;
        ecam.pilots[0].code_chan_dcch = Some(12);
        ecam.pilots[0].qof_mask_id_dcch = Some(1);

        let encoded = ecam.try_to_sdu().unwrap();
        let mut decode_bits = encoded.clone();
        let decoded = ExtendedChannelAssignmentMessage::from_sdu(&mut decode_bits).unwrap();
        assert_eq!(decoded.ch_ind, 0b10);
        assert_eq!(decoded.fpc_dcch_init_setpt, 0x21);
        assert_eq!(decoded.fpc_dcch_max_setpt, 0x52);
        assert_eq!(decoded.pilots[0].code_chan_dcch, Some(12));
        assert_eq!(decoded.pilots[0].qof_mask_id_dcch, Some(1));
        assert_eq!(decoded.to_sdu().bits(), encoded.bits());
    }

    #[test]
    fn ecam_pilot_info_record_does_not_pad_before_pwr_comb_ind() {
        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(3, 12, 1, 3, 3, false);
        let mut type_specific_fields = Bitstream::new();
        type_specific_fields.write_u8(0b10101, 5);
        ecam.pilots[0].pilot_record = Some(ExtendedPilotInfoRecord {
            pilot_rec_type: 0b001,
            type_specific_fields,
        });
        ecam.pilots[0].pwr_comb_ind = true;

        let encoded = ecam.try_to_sdu().unwrap();
        let mut decode_bits = encoded.clone();
        let decoded = ExtendedChannelAssignmentMessage::from_sdu(&mut decode_bits).unwrap();
        assert!(decoded.pilots[0].pwr_comb_ind);
        assert_eq!(
            decoded.pilots[0]
                .pilot_record
                .as_ref()
                .unwrap()
                .type_specific_fields
                .len(),
            8
        );
        assert_eq!(decoded.to_sdu().bits(), encoded.bits());
    }

    #[test]
    fn ecam_rejects_nonzero_reserved_2_bits() {
        let ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(3, 12, 1, 3, 3, false);
        let encoded = ecam.try_to_sdu().unwrap();
        let mut bits = encoded.bits().to_vec();
        bits[4] = 1; // ASSIGN_MODE(3), DIRECT_CH_ASSIGN_IND(1), then RESERVED_2.

        let err = ExtendedChannelAssignmentMessage::from_sdu(&mut Bitstream::new_init(&bits))
            .unwrap_err();

        assert!(
            err.to_string().contains("ECAM RESERVED_2"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ecam_rejects_nonzero_ch_record_reserved_padding() {
        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(3, 12, 1, 3, 3, false);
        let mut raw_ch_record_bits = ecam.build_ch_record_fields().bits().to_vec();
        *raw_ch_record_bits.last_mut().unwrap() = 1;
        ecam.raw_ch_record_fields = Some(Bitstream::new_init(&raw_ch_record_bits));

        let encoded = ecam.try_to_sdu().unwrap();
        let err = ExtendedChannelAssignmentMessage::from_sdu(&mut encoded.clone()).unwrap_err();

        assert!(
            err.to_string().contains("CH_RECORD_FIELDS"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ecam_fch_dcch_channel_record_roundtrip() {
        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(5, 10, 1, 3, 3, false);
        ecam.ch_ind = 0b11;
        ecam.fpc_dcch_init_setpt = 0x22;
        ecam.fpc_dcch_fer = 0b00011;
        ecam.fpc_dcch_min_setpt = 0x02;
        ecam.fpc_dcch_max_setpt = 0x53;
        ecam.fpc_pri_chan = true;
        ecam.pilots[0].code_chan_dcch = Some(14);
        ecam.pilots[0].qof_mask_id_dcch = Some(2);
        ecam.rev_fch_gating_mode = true;
        ecam.rev_pwr_cntl_delay = Some(0b10);

        let encoded = ecam.try_to_sdu().unwrap();
        let mut decode_bits = encoded.clone();
        let decoded = ExtendedChannelAssignmentMessage::from_sdu(&mut decode_bits).unwrap();
        assert_eq!(decoded.ch_ind, 0b11);
        assert!(decoded.fpc_pri_chan);
        assert_eq!(decoded.pilots[0].code_chan_fch, 10);
        assert_eq!(decoded.pilots[0].code_chan_dcch, Some(14));
        assert!(decoded.rev_fch_gating_mode);
        assert_eq!(decoded.rev_pwr_cntl_delay, Some(0b10));
        assert_eq!(decoded.to_sdu().bits(), encoded.bits());
    }

    #[test]
    fn ecam_non_enhanced_traffic_assignment_raw_roundtrip() {
        let mut raw = Bitstream::new();
        raw.write_u8(1, 1); // RESPOND
        raw.write_u8(0, 1); // FREQ_INCL
        raw.write_u8(0, 6); // NUM_PILOTS
        raw.write_u32(42, 9); // PILOT_PN

        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(0, 0, 0, 1, 1, false);
        ecam.assign_mode = 0b001;
        ecam.raw_additional_record_fields = Some(raw);

        let encoded = ecam.try_to_sdu().unwrap();
        let mut decode_bits = encoded.clone();
        let decoded = ExtendedChannelAssignmentMessage::from_sdu(&mut decode_bits).unwrap();
        assert_eq!(decoded.assign_mode, 0b001);
        assert_eq!(decoded.to_sdu().bits(), encoded.bits());
    }
}

#[cfg(test)]
mod nonneg_sch_fpc_tests {
    use super::*;

    /// Bit-walk an encoded NonNegServiceConfig and return the bits as a string
    /// of '0'/'1', stripping the trailing pad. Lets a test assert exact field
    /// boundaries in the SCH FPC block without fighting byte alignment.
    fn encoded_bit_string(cfg: &NonNegServiceConfig) -> String {
        let bytes = cfg.encode();
        let mut s = String::with_capacity(bytes.len() * 8);
        for b in bytes {
            for i in (0..8).rev() {
                s.push(if (b >> i) & 1 == 1 { '1' } else { '0' });
            }
        }
        s
    }

    #[test]
    fn rc3_default_omits_sch_fpc_block_for_backward_compat() {
        // The plain rc3_default (FCH-only) must keep producing the same wire
        // shape it did before SCH support was added. The shape we lock in
        // here matches the pre-SCH encoder: FPC_INCL + FPC_PRI_CHAN + FPC_MODE
        // + FPC_OLPC_FCH_INCL=1 + FCH triplet + FPC_OLPC_DCCH_INCL=0 +
        // GATING_RATE_INCL=1 + PILOT_GATING_RATE + RESERVED + LPM_IND.
        let baseline = NonNegServiceConfig::rc3_default();
        let bits = encoded_bit_string(&baseline);
        // Header + FCH block: 1+1+3+1+(5+8+8) = 27 bits,
        // DCCH gate = 1 bit, NUM_SUP = 2 bits, gating = 3 bits, then
        // reserved/LPM = 4 bits. Padded to 40 bits.
        assert_eq!(bits.len(), 40);
        assert_eq!(&bits[0..1], "1");
        assert_eq!(
            &bits[27..28],
            "0",
            "DCCH OLPC must remain 0 for the default"
        );
        assert_eq!(&bits[28..30], "00", "NUM_SUP must be 0 for the default");
        assert_eq!(&bits[30..31], "1", "GATING_RATE_INCL follows NUM_SUP");
    }

    #[test]
    fn rc3_fsch_default_emits_sch_fpc_block() {
        let cfg = NonNegServiceConfig::rc3_fsch_default();
        assert!(cfg.fpc_sch_incl);
        let bits = encoded_bit_string(&cfg);

        // With FPC_MODE=000 and no DCCH block, NUM_SUP sits right after the
        // DCCH OLPC bit. It is a direct count, not N-1.
        assert_eq!(&bits[28..30], "01", "NUM_SUP must be 1");
        assert_eq!(&bits[30..31], "0", "SCH_ID = SCH0");

        let fer = u8::from_str_radix(&bits[31..36], 2).unwrap();
        let min = u8::from_str_radix(&bits[36..44], 2).unwrap();
        let max = u8::from_str_radix(&bits[44..52], 2).unwrap();
        assert_eq!(fer, 0b00010, "1% FER target");
        assert_eq!(min, 0x00);
        assert_eq!(max, 0x50, "10.0 dB max setpoint");
        assert_eq!(&bits[52..53], "1", "GATING_RATE_INCL follows SCH record");
    }

    #[test]
    fn fsch_block_is_independent_of_dcch_state() {
        // Turning DCCH OLPC on must not displace the SCH bit interpretation
        // by the test. Re-derive positions and walk both blocks.
        let mut cfg = NonNegServiceConfig::rc3_fsch_default();
        cfg.fpc_olpc_dcch_incl = true;
        cfg.fpc_dcch_fer = 0b00011;
        cfg.fpc_dcch_min_setpt = 0x10;
        cfg.fpc_dcch_max_setpt = 0x40;

        let bits = encoded_bit_string(&cfg);
        // Header (1+1+3) + FCH OLPC (1) + FCH triplet (5+8+8) = 27
        // + DCCH OLPC (1) + DCCH triplet (5+8+8) = 49
        assert_eq!(&bits[27..28], "1", "DCCH OLPC bit");
        assert_eq!(&bits[49..51], "01", "NUM_SUP follows DCCH triplet");
        assert_eq!(&bits[51..52], "0", "SCH_ID follows NUM_SUP");
        let max = u8::from_str_radix(&bits[65..73], 2).unwrap();
        assert_eq!(max, 0x50);
    }

    #[test]
    #[should_panic(expected = "FPC_SCH_FER=11111 is reserved")]
    fn nonneg_rejects_reserved_sch_fer() {
        let mut cfg = NonNegServiceConfig::rc3_fsch_default();
        cfg.fpc_sch_fer = 0b11111;
        let _ = cfg.encode();
    }
}

#[cfg(test)]
mod service_connect_fsch_wire_tests {
    use super::*;

    fn minimal_sc_params(for_sch_config: Option<ForSchConfig>) -> ServiceConnectParams {
        ServiceConnectParams {
            serv_con_seq: 0,
            use_old_serv_config: 0,
            for_mux_option: 0x0001,
            rev_mux_option: 0x0001,
            for_rates: 0xF0,
            rev_rates: 0xF0,
            sync_id: None,
            connections: vec![],
            fch_frame_size: 0,
            for_fch_rc: 3,
            rev_fch_rc: 3,
            call_assignments: vec![],
            use_type0_plcm: false,
            non_neg: None,
            for_sch_config,
        }
    }

    fn bits_of(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 8);
        for b in bytes {
            for i in (0..8).rev() {
                s.push(if (b >> i) & 1 == 1 { '1' } else { '0' });
            }
        }
        s
    }

    #[test]
    fn service_config_record_no_sch_emits_for_sch_cc_incl_zero() {
        let params = minimal_sc_params(None);
        let bytes = params.encode_service_config_record();
        let bits = bits_of(&bytes);

        // Header: FOR_MUX(16) + REV_MUX(16) + FOR_RATES(8) + REV_RATES(8)
        // + NUM_CON_REC(8) = 56 bits, then channel config:
        //   FCH_CC_INCL(1) + FCH_FRAME_SIZE(1) + FOR_FCH_RC(5) + REV_FCH_RC(5)
        //   + DCCH_CC_INCL(1) = 13 bits → through bit position 56+13 = 69.
        // Bit 69 is FOR_SCH_CC_INCL.
        assert_eq!(
            &bits[69..70],
            "0",
            "FOR_SCH_CC_INCL must be 0 when no SCH config is provided"
        );
    }

    #[test]
    fn service_config_record_with_sch_emits_for_sch_cc_fields() {
        let params = minimal_sc_params(Some(ForSchConfig {
            sch_id: 0,
            mux_option: 0x0809,
            rc: 3,
            coding: 0,
            rate: 0x1, // 19.2 kbps for F-SCH RC3 per Table 2.7.4.27.3-2
        }));
        let bytes = params.encode_service_config_record();
        let bits = bits_of(&bytes);

        // Bit 69 = FOR_SCH_CC_INCL (after 56 header bits + 13 channel-cfg bits).
        assert_eq!(&bits[69..70], "1", "FOR_SCH_CC_INCL must be 1");

        // Per C.S0005-E §3.7.5.7 + §3.7.5.7.1, the SCH block layout when
        // FOR_SCH_CC_INCL=1 is:
        //   NUM_FOR_SCH         (2)   bits 70..72
        //   FOR_SCH_ID          (2)   bits 72..74    Table 3.7.5.7-5 (00=SCH0)
        //   FOR_SCH_MUX         (16)  bits 74..90
        //   SCH_CC_Type-specific subrecord (16) — per §3.7.5.7.1:
        //     SCH_REC_LEN       (4)   bits 90..94
        //     SCH_RC            (5)   bits 94..99
        //     CODING            (1)   bits 99..100
        //     FRAME_40_USED     (1)   bits 100..101
        //     FRAME_80_USED     (1)   bits 101..102
        //     MAX_RATE          (4)   bits 102..106  Table 2.7.4.27.3-2
        let num_for_sch = u8::from_str_radix(&bits[70..72], 2).unwrap();
        let for_sch_id = u8::from_str_radix(&bits[72..74], 2).unwrap();
        let for_sch_mux = u16::from_str_radix(&bits[74..90], 2).unwrap();
        let sch_rec_len = u8::from_str_radix(&bits[90..94], 2).unwrap();
        let sch_rc = u8::from_str_radix(&bits[94..99], 2).unwrap();
        let coding = u8::from_str_radix(&bits[99..100], 2).unwrap();
        let frame_40 = u8::from_str_radix(&bits[100..101], 2).unwrap();
        let frame_80 = u8::from_str_radix(&bits[101..102], 2).unwrap();
        let max_rate = u8::from_str_radix(&bits[102..106], 2).unwrap();

        assert_eq!(
            num_for_sch, 1,
            "NUM_FOR_SCH = 1 (one SCH; '00' is forbidden)"
        );
        assert_eq!(for_sch_id, 0, "FOR_SCH_ID = 0 (SCH0)");
        assert_eq!(
            for_sch_mux, 0x0809,
            "FOR_SCH_MUX = 360-bit Rate Set 1 Type 3 single"
        );
        assert_eq!(
            sch_rec_len, 2,
            "SCH_REC_LEN = 2 octets (the subrecord size)"
        );
        assert_eq!(sch_rc, 3, "SCH_RC = 3 (RC3)");
        assert_eq!(coding, 0, "CODING = 0 (convolutional)");
        assert_eq!(frame_40, 0, "FRAME_40_USED = 0 (Phase 1 = 20ms only)");
        assert_eq!(frame_80, 0, "FRAME_80_USED = 0");
        assert_eq!(max_rate, 0x1, "MAX_RATE = 0x1 (= 19.2 kbps for F-SCH RC3)");
    }

    #[test]
    fn service_config_record_with_153k6_sch_uses_0x0921_mux() {
        let params = minimal_sc_params(Some(ForSchConfig {
            sch_id: 0,
            mux_option: 0x0921,
            rc: 3,
            coding: 0,
            rate: 0x4,
        }));
        let bytes = params.encode_service_config_record();
        let bits = bits_of(&bytes);

        let for_sch_mux = u16::from_str_radix(&bits[74..90], 2).unwrap();
        let max_rate = u8::from_str_radix(&bits[102..106], 2).unwrap();

        assert_eq!(
            for_sch_mux, 0x0921,
            "FOR_SCH_MUX = 3048-bit Rate Set 1 Type 3 double"
        );
        assert_eq!(max_rate, 0x4, "MAX_RATE = 0x4 (= 153.6 kbps for F-SCH RC3)");
    }

    #[test]
    #[should_panic(expected = "F-SCH MAX_RATE must be 0x1..=0x4 for convolutional RC3")]
    fn service_config_record_rejects_out_of_range_phase1_rate() {
        let params = minimal_sc_params(Some(ForSchConfig {
            sch_id: 0,
            mux_option: 0x0809,
            rc: 3,
            coding: 0,
            rate: 0x5,
        }));
        let _ = params.encode_service_config_record();
    }
}
