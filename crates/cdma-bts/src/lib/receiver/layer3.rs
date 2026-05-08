//! CDMA2000 Forward Link Layer 3 message decoder.
//!
//! Decodes paging channel messages per C.S0005-E Table 3.7.2.3.2.1-1.
//! The PDU format is: PD(2 bits) | MSG_TYPE(6 bits) | fields...

use crate::lac::message_types::{MessageId, WireChannel};
use cdma_common::bits::Bitstream;

pub fn msg_type_name(msg_type: u8) -> &'static str {
    MessageId::from_wire(WireChannel::ForwardCommon, msg_type).map_or("Unknown", |m| m.name())
}

// ---------------------------------------------------------------------------
// Decoded message types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PagingMessageHeader {
    pub pd: u8,
    pub msg_type: u8,
}

#[derive(Debug)]
pub struct SystemParametersMessage {
    pub header: PagingMessageHeader,
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
}

#[derive(Debug)]
pub struct AccessParametersMessage {
    pub header: PagingMessageHeader,
    pub pilot_pn: u16,
    pub acc_msg_seq: u8,
    pub acc_chan: u8,
    /// NOM_PWR offset in dB. 4-bit signed two's complement on the wire,
    /// canonical range -8..+7 dB for Band Class 0.
    pub nom_pwr: i8,
    /// INIT_PWR offset in dB. 5-bit signed two's complement on the wire,
    /// canonical range -16..+15 dB.
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
    pub psist_emg: Option<u8>,
    pub acct_incl: bool,
    pub acct_incl_emg: Option<bool>,
    pub acct_aoc_bitmap_incl: Option<bool>,
    pub acct_so_incl: Option<bool>,
    pub acct_so_records: Vec<AcctServiceOptionRecord>,
    pub acct_so_grp_incl: Option<bool>,
    pub acct_so_grp_records: Vec<AcctServiceOptionGroupRecord>,
}

#[derive(Debug)]
pub struct AcctServiceOptionRecord {
    pub aoc_bitmap: Option<u8>,
    pub service_option: u16,
}

#[derive(Debug)]
pub struct AcctServiceOptionGroupRecord {
    pub aoc_bitmap: Option<u8>,
    pub service_option_group: u8,
}

#[derive(Debug)]
pub struct NeighborListMessage {
    pub header: PagingMessageHeader,
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub pilot_inc: u8,
    pub neighbors: Vec<NeighborEntry>,
}

#[derive(Debug)]
pub struct NeighborEntry {
    pub nghbr_pn: u16,
}

#[derive(Debug)]
pub struct GeneralPageMessage {
    pub header: PagingMessageHeader,
    pub config_msg_seq: u8,
    pub acc_msg_seq: u8,
    pub class_0_done: bool,
    pub class_1_done: bool,
    pub tmsi_done: bool,
    pub ordered_tmsis: bool,
    pub broadcast_done: bool,
    pub reserved: u8,
    pub add_length: u8,
    pub add_pfield: Vec<u8>,
    pub page_records: Vec<PageRecord>,
}

#[derive(Debug)]
pub enum PageRecord {
    /// PAGE_CLASS = 0: IMSI-based page
    Class0 {
        page_subclass: u8,
        msg_seq: u8,
        /// Decoded IMSI fields depend on subclass
        imsi_s: Option<u64>,
        imsi_11_12: Option<u8>,
        mcc: Option<u16>,
        imsi_addr_num: Option<u8>,
        imsi_m_s1: Option<u32>,
        imsi_m_s2: Option<u16>,
        special_service: bool,
        service_option: Option<u16>,
    },
    /// PAGE_CLASS = 1: ESN-based page
    Class1 {
        msg_seq: u8,
        esn: u32,
        special_service: bool,
        service_option: Option<u16>,
    },
    /// PAGE_CLASS = 2: TMSI page
    Tmsi {
        msg_seq: u8,
        tmsi_code_addr: u32,
        special_service: bool,
        service_option: Option<u16>,
    },
    /// PAGE_CLASS = 3: Broadcast page
    Broadcast { bc_addr: u16 },
}

#[derive(Debug)]
pub struct OrderMessage {
    pub header: PagingMessageHeader,
    pub order: u8,
    pub ordq: u8,
}

#[derive(Debug)]
pub struct CdmaChannelListMessage {
    pub header: PagingMessageHeader,
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub channels: Vec<u16>,
}

#[derive(Debug)]
pub struct ExtendedSystemParametersMessage {
    pub header: PagingMessageHeader,
    pub pilot_pn: u16,
    pub config_msg_seq: u8,
    pub delete_for_tmsi: bool,
    pub use_tmsi: bool,
    pub pref_msid_type: u8,
    pub mcc: u16,
    pub imsi_11_12: u8,
    pub tmsi_zone_len: u8,
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
    pub ec_thresh: Option<u8>,
    pub ec_io_thresh: Option<u8>,
    pub pilot_report: bool,
    pub nghbr_set_entry_info: bool,
    pub acc_ent_ho_order: Option<bool>,
    pub nghbr_set_access_info: bool,
    pub access_ho: Option<bool>,
    pub access_ho_msg_rsp: Option<bool>,
    pub access_probe_ho: Option<bool>,
    pub acc_ho_list_upd: Option<bool>,
    pub acc_probe_ho_other_msg: Option<bool>,
    pub max_num_probe_ho: Option<u8>,
    pub nghbr_set_size: Option<u8>,
    pub access_entry_ho: Vec<bool>,
    pub access_ho_allowed: Vec<bool>,
    pub broadcast_gps_asst: bool,
    pub qpch_supported: bool,
    pub num_qpch: Option<u8>,
    pub qpch_rate: Option<u8>,
    pub qpch_power_level_page: Option<u8>,
    pub qpch_cci_supported: Option<bool>,
    pub qpch_power_level_config: Option<u8>,
    pub sdb_supported: bool,
    pub rlgain_traffic_pilot: u8,
    pub rev_pwr_cntl_delay_incl: bool,
    pub rev_pwr_cntl_delay: Option<u8>,
    pub auto_msg_supported: bool,
    pub auto_msg_interval: Option<u8>,
    pub mob_qos: bool,
    pub enc_supported: bool,
    pub sig_encrypt_sup: Option<u8>,
    pub ui_encrypt_sup: Option<u8>,
    pub use_sync_id: bool,
    pub cs_supported: bool,
    pub bcch_supported: bool,
    pub ms_init_pos_loc_sup_ind: bool,
    pub pilot_info_req_supported: bool,
}

#[derive(Debug)]
pub enum PagingMessage {
    SystemParameters(SystemParametersMessage),
    AccessParameters(AccessParametersMessage),
    NeighborList(NeighborListMessage),
    CdmaChannelList(CdmaChannelListMessage),
    ExtendedSystemParameters(ExtendedSystemParametersMessage),
    GeneralPage(GeneralPageMessage),
    Order(OrderMessage),
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

impl PagingMessage {
    /// Decode a paging channel PDU (payload after MSG_LENGTH, before CRC).
    /// The PDU starts with PD(2) | MSG_TYPE(6).
    pub fn decode(data: &Bitstream) -> Result<Self, String> {
        let mut bs = data.clone();

        if bs.len() < 8 {
            return Err(format!("PDU too short: {} bits", bs.len()));
        }

        let pd_and_type = bs.read_bits(8).map_err(|e| e.to_string())? as u8;
        let pd = pd_and_type >> 6;
        let msg_type = pd_and_type & 0x3F;
        let header = PagingMessageHeader { pd, msg_type };

        let message_id = MessageId::from_wire(WireChannel::ForwardCommon, msg_type)
            .ok_or_else(|| format!("unsupported f-csch MSG_TYPE 0x{msg_type:02X}"))?;

        match message_id {
            MessageId::SystemParameters => decode_system_parameters(header, &mut bs),
            MessageId::AccessParameters => decode_access_parameters(header, &mut bs),
            MessageId::NeighborList => decode_neighbor_list(header, &mut bs),
            MessageId::CdmaChannelList => decode_cdma_channel_list(header, &mut bs),
            MessageId::ExtSystemParameters => decode_extended_system_parameters(header, &mut bs),
            MessageId::GeneralPage => decode_general_page(header, &mut bs),
            MessageId::Order => decode_order(header, &mut bs),
            _ => Err(format!(
                "unsupported f-csch body decode for {}",
                message_id.tag()
            )),
        }
    }

    pub fn print(&self) {
        match self {
            PagingMessage::SystemParameters(m) => print_system_parameters(m),
            PagingMessage::AccessParameters(m) => print_access_parameters(m),
            PagingMessage::NeighborList(m) => print_neighbor_list(m),
            PagingMessage::CdmaChannelList(m) => print_cdma_channel_list(m),
            PagingMessage::ExtendedSystemParameters(m) => print_extended_system_parameters(m),
            PagingMessage::GeneralPage(m) => print_general_page(m),
            PagingMessage::Order(m) => print_order(m),
        }
    }
}

// ---------------------------------------------------------------------------
// Decoders
// ---------------------------------------------------------------------------

fn read(bs: &mut Bitstream, bits: usize, name: &str) -> Result<u64, String> {
    bs.read_bits(bits)
        .map_err(|_| format!("EOF reading {} ({} bits)", name, bits))
}

fn decode_system_parameters(
    header: PagingMessageHeader,
    bs: &mut Bitstream,
) -> Result<PagingMessage, String> {
    Ok(PagingMessage::SystemParameters(SystemParametersMessage {
        header,
        pilot_pn: read(bs, 9, "PILOT_PN")? as u16,
        config_msg_seq: read(bs, 6, "CONFIG_MSG_SEQ")? as u8,
        sid: read(bs, 15, "SID")? as u16,
        nid: read(bs, 16, "NID")? as u16,
        reg_zone: read(bs, 12, "REG_ZONE")? as u16,
        total_zones: read(bs, 3, "TOTAL_ZONES")? as u8,
        zone_timer: read(bs, 3, "ZONE_TIMER")? as u8,
        mult_sids: read(bs, 1, "MULT_SIDS")? == 1,
        mult_nids: read(bs, 1, "MULT_NIDS")? == 1,
        base_id: read(bs, 16, "BASE_ID")? as u16,
        base_class: read(bs, 4, "BASE_CLASS")? as u8,
        page_chan: read(bs, 3, "PAGE_CHAN")? as u8,
        max_slot_cycle_index: read(bs, 3, "MAX_SLOT_CYCLE_INDEX")? as u8,
        home_reg: read(bs, 1, "HOME_REG")? == 1,
        for_sid_reg: read(bs, 1, "FOR_SID_REG")? == 1,
        for_nid_reg: read(bs, 1, "FOR_NID_REG")? == 1,
        power_up_reg: read(bs, 1, "POWER_UP_REG")? == 1,
        power_down_reg: read(bs, 1, "POWER_DOWN_REG")? == 1,
        parameter_reg: read(bs, 1, "PARAMETER_REG")? == 1,
        reg_prd: read(bs, 7, "REG_PRD")? as u8,
        base_lat: read(bs, 22, "BASE_LAT")? as u32,
        base_long: read(bs, 23, "BASE_LONG")? as u32,
        reg_dist: read(bs, 11, "REG_DIST")? as u16,
        srch_win_a: read(bs, 4, "SRCH_WIN_A")? as u8,
        srch_win_n: read(bs, 4, "SRCH_WIN_N")? as u8,
        srch_win_r: read(bs, 4, "SRCH_WIN_R")? as u8,
        nghbr_max_age: read(bs, 4, "NGHBR_MAX_AGE")? as u8,
        pwr_rep_thresh: read(bs, 5, "PWR_REP_THRESH")? as u8,
        pwr_rep_frames: read(bs, 4, "PWR_REP_FRAMES")? as u8,
        pwr_thresh_enable: read(bs, 1, "PWR_THRESH_ENABLE")? == 1,
        pwr_period_enable: read(bs, 1, "PWR_PERIOD_ENABLE")? == 1,
        pwr_rep_delay: read(bs, 5, "PWR_REP_DELAY")? as u8,
        rescan: read(bs, 1, "RESCAN")? == 1,
        t_add: read(bs, 6, "T_ADD")? as u8,
        t_drop: read(bs, 6, "T_DROP")? as u8,
        t_comp: read(bs, 4, "T_COMP")? as u8,
        t_tdrop: read(bs, 4, "T_TDROP")? as u8,
        ext_sys_parameter: read(bs, 1, "EXT_SYS_PARAMETER")? == 1,
        ext_nghbr_lst: read(bs, 1, "EXT_NGHBR_LST")? == 1,
        gen_nghbr_lst: read(bs, 1, "GEN_NGHBR_LST")? == 1,
        global_redirect: read(bs, 1, "GLOBAL_REDIRECT")? == 1,
        pri_nghbr_lst: read(bs, 1, "PRI_NGHBR_LST")? == 1,
        user_zone_id: read(bs, 1, "USER_ZONE_ID")? == 1,
        ext_global_redirect: read(bs, 1, "EXT_GLOBAL_REDIRECT")? == 1,
        ext_chan_lst: read(bs, 1, "EXT_CHAN_LST")? == 1,
    }))
}

fn decode_access_parameters(
    header: PagingMessageHeader,
    bs: &mut Bitstream,
) -> Result<PagingMessage, String> {
    let pilot_pn = read(bs, 9, "PILOT_PN")? as u16;
    let acc_msg_seq = read(bs, 6, "ACC_MSG_SEQ")? as u8;
    let acc_chan = read(bs, 5, "ACC_CHAN")? as u8;
    let nom_pwr = sign_extend(read(bs, 4, "NOM_PWR")? as u32, 4) as i8;
    let init_pwr = sign_extend(read(bs, 5, "INIT_PWR")? as u32, 5) as i8;
    let pwr_step = read(bs, 3, "PWR_STEP")? as u8;
    let num_step = read(bs, 4, "NUM_STEP")? as u8;
    let max_cap_sz = read(bs, 3, "MAX_CAP_SZ")? as u8;
    let pam_sz = read(bs, 4, "PAM_SZ")? as u8;
    let psist_0_9 = read(bs, 6, "PSIST_0_9")? as u8;
    let psist_10 = read(bs, 3, "PSIST_10")? as u8;
    let psist_11 = read(bs, 3, "PSIST_11")? as u8;
    let psist_12 = read(bs, 3, "PSIST_12")? as u8;
    let psist_13 = read(bs, 3, "PSIST_13")? as u8;
    let psist_14 = read(bs, 3, "PSIST_14")? as u8;
    let psist_15 = read(bs, 3, "PSIST_15")? as u8;
    let msg_psist = read(bs, 3, "MSG_PSIST")? as u8;
    let reg_psist = read(bs, 3, "REG_PSIST")? as u8;
    let probe_pn_ran = read(bs, 4, "PROBE_PN_RAN")? as u8;
    let acc_tmo = read(bs, 4, "ACC_TMO")? as u8;
    let probe_bkoff = read(bs, 4, "PROBE_BKOFF")? as u8;
    let bkoff = read(bs, 4, "BKOFF")? as u8;
    let max_req_seq = read(bs, 4, "MAX_REQ_SEQ")? as u8;
    let max_rsp_seq = read(bs, 4, "MAX_RSP_SEQ")? as u8;
    let auth = read(bs, 2, "AUTH")? as u8;
    let rand = if auth != 0 {
        read(bs, 32, "RAND")? as u32
    } else {
        0
    };
    let nom_pwr_ext = bs.read_bits(1).unwrap_or(0) as u8;
    let psist_emg_incl = bs.read_bits(1).unwrap_or(0) == 1;
    let psist_emg = if psist_emg_incl {
        Some(read(bs, 3, "PSIST_EMG")? as u8)
    } else {
        None
    };
    let acct_incl = bs.read_bits(1).unwrap_or(0) == 1;
    let (
        acct_incl_emg,
        acct_aoc_bitmap_incl,
        acct_so_incl,
        acct_so_records,
        acct_so_grp_incl,
        acct_so_grp_records,
    ) = if acct_incl {
        let acct_incl_emg = read(bs, 1, "ACCT_INCL_EMG")? == 1;
        let acct_aoc_bitmap_incl = read(bs, 1, "ACCT_AOC_BITMAP_INCL")? == 1;
        let acct_so_incl = read(bs, 1, "ACCT_SO_INCL")? == 1;
        let mut acct_so_records = Vec::new();
        if acct_so_incl {
            let num_acct_so = read(bs, 4, "NUM_ACCT_SO")? as usize + 1;
            for _ in 0..num_acct_so {
                let aoc_bitmap = if acct_aoc_bitmap_incl {
                    Some(read(bs, 5, "ACCT_AOC_BITMAP1")? as u8)
                } else {
                    None
                };
                let service_option = read(bs, 16, "ACCT_SO")? as u16;
                acct_so_records.push(AcctServiceOptionRecord {
                    aoc_bitmap,
                    service_option,
                });
            }
        }
        let acct_so_grp_incl = read(bs, 1, "ACCT_SO_GRP_INCL")? == 1;
        let mut acct_so_grp_records = Vec::new();
        if acct_so_grp_incl {
            let num_acct_so_grp = read(bs, 3, "NUM_ACCT_SO_GRP")? as usize + 1;
            for _ in 0..num_acct_so_grp {
                let aoc_bitmap = if acct_aoc_bitmap_incl {
                    Some(read(bs, 5, "ACCT_AOC_BITMAP2")? as u8)
                } else {
                    None
                };
                let service_option_group = read(bs, 5, "ACCT_SO_GRP")? as u8;
                acct_so_grp_records.push(AcctServiceOptionGroupRecord {
                    aoc_bitmap,
                    service_option_group,
                });
            }
        }
        (
            Some(acct_incl_emg),
            Some(acct_aoc_bitmap_incl),
            Some(acct_so_incl),
            acct_so_records,
            Some(acct_so_grp_incl),
            acct_so_grp_records,
        )
    } else {
        (None, None, None, Vec::new(), None, Vec::new())
    };

    Ok(PagingMessage::AccessParameters(AccessParametersMessage {
        header,
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
        acct_so_incl,
        acct_so_records,
        acct_so_grp_incl,
        acct_so_grp_records,
    }))
}

fn decode_neighbor_list(
    header: PagingMessageHeader,
    bs: &mut Bitstream,
) -> Result<PagingMessage, String> {
    let pilot_pn = read(bs, 9, "PILOT_PN")? as u16;
    let config_msg_seq = read(bs, 6, "CONFIG_MSG_SEQ")? as u8;
    let pilot_inc = read(bs, 4, "PILOT_INC")? as u8;

    let mut neighbors = Vec::new();
    while bs.len() >= 9 {
        let nghbr_pn = read(bs, 9, "NGHBR_PN")? as u16;
        neighbors.push(NeighborEntry { nghbr_pn });
    }

    Ok(PagingMessage::NeighborList(NeighborListMessage {
        header,
        pilot_pn,
        config_msg_seq,
        pilot_inc,
        neighbors,
    }))
}

fn decode_cdma_channel_list(
    header: PagingMessageHeader,
    bs: &mut Bitstream,
) -> Result<PagingMessage, String> {
    let pilot_pn = read(bs, 9, "PILOT_PN")? as u16;
    let config_msg_seq = read(bs, 6, "CONFIG_MSG_SEQ")? as u8;

    let mut channels = Vec::new();
    while bs.len() >= 11 {
        let freq = read(bs, 11, "CDMA_FREQ")? as u16;
        channels.push(freq);
    }

    Ok(PagingMessage::CdmaChannelList(CdmaChannelListMessage {
        header,
        pilot_pn,
        config_msg_seq,
        channels,
    }))
}

fn decode_extended_system_parameters(
    header: PagingMessageHeader,
    bs: &mut Bitstream,
) -> Result<PagingMessage, String> {
    let pilot_pn = read(bs, 9, "PILOT_PN")? as u16;
    let config_msg_seq = read(bs, 6, "CONFIG_MSG_SEQ")? as u8;
    let delete_for_tmsi = read(bs, 1, "DELETE_FOR_TMSI")? == 1;
    let use_tmsi = read(bs, 1, "USE_TMSI")? == 1;
    let pref_msid_type = read(bs, 2, "PREF_MSID_TYPE")? as u8;
    let mcc = read(bs, 10, "MCC")? as u16;
    let imsi_11_12 = read(bs, 7, "IMSI_11_12")? as u8;
    let tmsi_zone_len = read(bs, 4, "TMSI_ZONE_LEN")? as u8;
    let mut tmsi_zone = Vec::new();
    for _ in 0..tmsi_zone_len {
        tmsi_zone.push(read(bs, 8, "TMSI_ZONE_BYTE")? as u8);
    }
    let bcast_index = read(bs, 3, "BCAST_INDEX")? as u8;
    let imsi_t_supported = read(bs, 1, "IMSI_T_SUPPORTED")? == 1;
    let p_rev = read(bs, 8, "P_REV")? as u8;
    let min_p_rev = read(bs, 8, "MIN_P_REV")? as u8;
    let soft_slope = read(bs, 6, "SOFT_SLOPE")? as u8;
    let add_intercept = read(bs, 6, "ADD_INTERCEPT")? as u8;
    let drop_intercept = read(bs, 6, "DROP_INTERCEPT")? as u8;
    let packet_zone_id = read(bs, 8, "PACKET_ZONE_ID")? as u8;
    let max_num_alt_so = read(bs, 3, "MAX_NUM_ALT_SO")? as u8;
    let reselect_included = read(bs, 1, "RESELECT_INCLUDED")? == 1;
    let ec_thresh = if reselect_included {
        Some(read(bs, 5, "EC_THRESH")? as u8)
    } else {
        None
    };
    let ec_io_thresh = if reselect_included {
        Some(read(bs, 5, "EC_I0_THRESH")? as u8)
    } else {
        None
    };
    let pilot_report = read(bs, 1, "PILOT_REPORT")? == 1;
    let nghbr_set_entry_info = read(bs, 1, "NGHBR_SET_ENTRY_INFO")? == 1;
    let acc_ent_ho_order = if nghbr_set_entry_info {
        Some(read(bs, 1, "ACC_ENT_HO_ORDER")? == 1)
    } else {
        None
    };
    let nghbr_set_access_info = read(bs, 1, "NGHBR_SET_ACCESS_INFO")? == 1;
    let access_ho = if nghbr_set_access_info {
        Some(read(bs, 1, "ACCESS_HO")? == 1)
    } else {
        None
    };
    let access_ho_msg_rsp = if access_ho == Some(true) {
        Some(read(bs, 1, "ACCESS_HO_MSG_RSP")? == 1)
    } else {
        None
    };
    let access_probe_ho = if nghbr_set_access_info {
        Some(read(bs, 1, "ACCESS_PROBE_HO")? == 1)
    } else {
        None
    };
    let acc_ho_list_upd = if access_probe_ho == Some(true) {
        Some(read(bs, 1, "ACC_HO_LIST_UPD")? == 1)
    } else {
        None
    };
    let acc_probe_ho_other_msg = if access_probe_ho == Some(true) {
        Some(read(bs, 1, "ACC_PROBE_HO_OTHER_MSG")? == 1)
    } else {
        None
    };
    let max_num_probe_ho = if access_probe_ho == Some(true) {
        Some(read(bs, 3, "MAX_NUM_PROBE_HO")? as u8)
    } else {
        None
    };
    let nghbr_set_size = if nghbr_set_entry_info || nghbr_set_access_info {
        Some(read(bs, 6, "NGHBR_SET_SIZE")? as u8)
    } else {
        None
    };
    let mut access_entry_ho = Vec::new();
    if nghbr_set_entry_info {
        for _ in 0..nghbr_set_size.unwrap_or(0) {
            access_entry_ho.push(read(bs, 1, "ACCESS_ENTRY_HO")? == 1);
        }
    }
    let mut access_ho_allowed = Vec::new();
    if nghbr_set_access_info {
        for _ in 0..nghbr_set_size.unwrap_or(0) {
            access_ho_allowed.push(read(bs, 1, "ACCESS_HO_ALLOWED")? == 1);
        }
    }
    let broadcast_gps_asst = read(bs, 1, "BROADCAST_GPS_ASST")? == 1;
    let qpch_supported = read(bs, 1, "QPCH_SUPPORTED")? == 1;
    let num_qpch = if qpch_supported {
        Some(read(bs, 2, "NUM_QPCH")? as u8)
    } else {
        None
    };
    let qpch_rate = if qpch_supported {
        Some(read(bs, 1, "QPCH_RATE")? as u8)
    } else {
        None
    };
    let qpch_power_level_page = if qpch_supported {
        Some(read(bs, 3, "QPCH_POWER_LEVEL_PAGE")? as u8)
    } else {
        None
    };
    let qpch_cci_supported = if qpch_supported {
        Some(read(bs, 1, "QPCH_CCI_SUPPORTED")? == 1)
    } else {
        None
    };
    let qpch_power_level_config = if qpch_cci_supported == Some(true) {
        Some(read(bs, 3, "QPCH_POWER_LEVEL_CONFIG")? as u8)
    } else {
        None
    };
    let sdb_supported = read(bs, 1, "SDB_SUPPORTED")? == 1;
    let rlgain_traffic_pilot = read(bs, 6, "RLGAIN_TRAFFIC_PILOT")? as u8;
    let rev_pwr_cntl_delay_incl = read(bs, 1, "REV_PWR_CNTL_DELAY_INCL")? == 1;
    let rev_pwr_cntl_delay = if rev_pwr_cntl_delay_incl {
        Some(read(bs, 2, "REV_PWR_CNTL_DELAY")? as u8)
    } else {
        None
    };
    let auto_msg_supported = read(bs, 1, "AUTO_MSG_SUPPORTED")? == 1;
    let auto_msg_interval = if auto_msg_supported {
        Some(read(bs, 3, "AUTO_MSG_INTERVAL")? as u8)
    } else {
        None
    };
    let mob_qos = read(bs, 1, "MOB_QOS")? == 1;
    let enc_supported = read(bs, 1, "ENC_SUPPORTED")? == 1;
    let sig_encrypt_sup = if enc_supported {
        Some(read(bs, 8, "SIG_ENCRYPT_SUP")? as u8)
    } else {
        None
    };
    let ui_encrypt_sup = if enc_supported {
        Some(read(bs, 8, "UI_ENCRYPT_SUP")? as u8)
    } else {
        None
    };
    let use_sync_id = read(bs, 1, "USE_SYNC_ID")? == 1;
    let cs_supported = read(bs, 1, "CS_SUPPORTED")? == 1;
    let bcch_supported = read(bs, 1, "BCCH_SUPPORTED")? == 1;
    let ms_init_pos_loc_sup_ind = read(bs, 1, "MS_INIT_POS_LOC_SUP_IND")? == 1;
    let pilot_info_req_supported = read(bs, 1, "PILOT_INFO_REQ_SUPPORTED")? == 1;

    Ok(PagingMessage::ExtendedSystemParameters(
        ExtendedSystemParametersMessage {
            header,
            pilot_pn,
            config_msg_seq,
            delete_for_tmsi,
            use_tmsi,
            pref_msid_type,
            mcc,
            imsi_11_12,
            tmsi_zone_len,
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
        },
    ))
}

fn decode_general_page(
    header: PagingMessageHeader,
    bs: &mut Bitstream,
) -> Result<PagingMessage, String> {
    let config_msg_seq = read(bs, 6, "CONFIG_MSG_SEQ")? as u8;
    let acc_msg_seq = read(bs, 6, "ACC_MSG_SEQ")? as u8;
    let class_0_done = read(bs, 1, "CLASS_0_DONE")? == 1;
    let class_1_done = read(bs, 1, "CLASS_1_DONE")? == 1;
    let tmsi_done = read(bs, 1, "TMSI_DONE")? == 1;
    let ordered_tmsis = read(bs, 1, "ORDERED_TMSIS")? == 1;
    let broadcast_done = read(bs, 1, "BROADCAST_DONE")? == 1;
    let reserved = read(bs, 4, "RESERVED")? as u8;
    let add_length = read(bs, 3, "ADD_LENGTH")? as u8;
    let add_pfield_bits = (add_length as usize) * 8;
    let mut add_pfield = Vec::new();
    if add_pfield_bits > 0 && bs.len() >= add_pfield_bits {
        for _ in 0..add_length {
            add_pfield.push(read(bs, 8, "ADD_PFIELD_BYTE")? as u8);
        }
    }

    let mut page_records = Vec::new();
    // Page records follow until remaining bits are all padding (zeros).
    // Each record starts with PAGE_CLASS(2).
    while bs.len() >= 2 {
        let page_class = read(bs, 2, "PAGE_CLASS")? as u8;

        match page_class {
            0 => {
                // Class 0: IMSI-based page
                if bs.len() < 5 {
                    break;
                }
                let page_subclass = read(bs, 2, "PAGE_SUBCLASS")? as u8;
                let msg_seq = read(bs, 3, "MSG_SEQ")? as u8;

                let (imsi_s, imsi_11_12, mcc) = match page_subclass {
                    0 => {
                        // IMSI_S (34 bits)
                        if bs.len() < 34 {
                            break;
                        }
                        (Some(read(bs, 34, "IMSI_S")?), None, None)
                    }
                    1 => {
                        // IMSI_11_12 (7) + IMSI_S (34)
                        if bs.len() < 41 {
                            break;
                        }
                        let imsi_11_12 = read(bs, 7, "IMSI_11_12")? as u8;
                        let imsi_s = read(bs, 34, "IMSI_S")?;
                        (Some(imsi_s), Some(imsi_11_12), None)
                    }
                    2 => {
                        // Format 2: MCC (10) + IMSI_S (34)
                        if bs.len() < 44 {
                            break;
                        }
                        let mcc = read(bs, 10, "MCC")? as u16;
                        let imsi_s = read(bs, 34, "IMSI_S")?;
                        (Some(imsi_s), None, Some(mcc))
                    }
                    3 => {
                        // Format 3: MCC (10) + IMSI_11_12 (7) + IMSI_S (34)
                        if bs.len() < 51 {
                            break;
                        }
                        let mcc = read(bs, 10, "MCC")? as u16;
                        let imsi_11_12 = read(bs, 7, "IMSI_11_12")? as u8;
                        let imsi_s = read(bs, 34, "IMSI_S")?;
                        (Some(imsi_s), Some(imsi_11_12), Some(mcc))
                    }
                    _ => break,
                };
                let imsi_addr_num = None;
                let imsi_m_s1 = None;
                let imsi_m_s2 = None;

                let special_service = if bs.len() >= 1 {
                    read(bs, 1, "SPECIAL_SERVICE")? == 1
                } else {
                    false
                };
                let service_option = if special_service && bs.len() >= 16 {
                    Some(read(bs, 16, "SERVICE_OPTION")? as u16)
                } else {
                    None
                };

                page_records.push(PageRecord::Class0 {
                    page_subclass,
                    msg_seq,
                    imsi_s,
                    imsi_11_12,
                    mcc,
                    imsi_addr_num,
                    imsi_m_s1,
                    imsi_m_s2,
                    special_service,
                    service_option,
                });
            }
            1 => {
                // Class 1: ESN-based page
                if bs.len() < 36 {
                    break;
                }
                let msg_seq = read(bs, 3, "MSG_SEQ")? as u8;
                let esn = read(bs, 32, "ESN")? as u32;

                let special_service = if bs.len() >= 1 {
                    read(bs, 1, "SPECIAL_SERVICE")? == 1
                } else {
                    false
                };
                let service_option = if special_service && bs.len() >= 16 {
                    Some(read(bs, 16, "SERVICE_OPTION")? as u16)
                } else {
                    None
                };

                page_records.push(PageRecord::Class1 {
                    msg_seq,
                    esn,
                    special_service,
                    service_option,
                });
            }
            2 => {
                // Class 2: TMSI page
                if bs.len() < 36 {
                    break;
                }
                let msg_seq = read(bs, 3, "MSG_SEQ")? as u8;
                let tmsi_code_addr = read(bs, 32, "TMSI_CODE_ADDR")? as u32;

                let special_service = if bs.len() >= 1 {
                    read(bs, 1, "SPECIAL_SERVICE")? == 1
                } else {
                    false
                };
                let service_option = if special_service && bs.len() >= 16 {
                    Some(read(bs, 16, "SERVICE_OPTION")? as u16)
                } else {
                    None
                };

                page_records.push(PageRecord::Tmsi {
                    msg_seq,
                    tmsi_code_addr,
                    special_service,
                    service_option,
                });
            }
            3 => {
                // Class 3: Broadcast page
                if bs.len() < 16 {
                    break;
                }
                let bc_addr = read(bs, 16, "BC_ADDR")? as u16;
                page_records.push(PageRecord::Broadcast { bc_addr });
            }
            _ => break,
        }
    }

    Ok(PagingMessage::GeneralPage(GeneralPageMessage {
        header,
        config_msg_seq,
        acc_msg_seq,
        class_0_done,
        class_1_done,
        tmsi_done,
        ordered_tmsis,
        broadcast_done,
        reserved,
        add_length,
        add_pfield,
        page_records,
    }))
}

fn decode_order(header: PagingMessageHeader, bs: &mut Bitstream) -> Result<PagingMessage, String> {
    // f-csch Order Message body: ORDER(6) + ADD_RECORD_LEN(3) + [ORDQ(8) + fields...]
    let order = if bs.len() >= 6 {
        read(bs, 6, "ORDER")? as u8
    } else {
        0
    };
    let add_record_len = if bs.len() >= 3 {
        read(bs, 3, "ADD_RECORD_LEN")? as u8
    } else {
        0
    };
    let ordq = if add_record_len > 0 && bs.len() >= 8 {
        read(bs, 8, "ORDQ")? as u8
    } else {
        0
    };
    // Skip remaining order-specific fields beyond ORDQ
    for _ in 1..add_record_len {
        if bs.len() >= 8 {
            let _ = read(bs, 8, "ORDFIELD");
        }
    }

    Ok(PagingMessage::Order(OrderMessage {
        header,
        order,
        ordq,
    }))
}

// ---------------------------------------------------------------------------
// Pretty printers
// ---------------------------------------------------------------------------

fn print_system_parameters(m: &SystemParametersMessage) {
    println!("  Message: System Parameters Message (SPM)");
    println!("  PD: {}", m.header.pd);
    println!(
        "  PILOT_PN: {} (offset = {} chips)",
        m.pilot_pn,
        m.pilot_pn as u32 * 64
    );
    println!("  CONFIG_MSG_SEQ: {}", m.config_msg_seq);
    println!("  SID: {}", m.sid);
    println!("  NID: {}", m.nid);
    println!("  REG_ZONE: {}", m.reg_zone);
    println!("  TOTAL_ZONES: {}", m.total_zones);
    let zone_timer_min = match m.zone_timer {
        0 => 1,
        1 => 2,
        2 => 5,
        3 => 10,
        4 => 20,
        5 => 30,
        6 => 45,
        7 => 60,
        _ => 0,
    };
    println!("  ZONE_TIMER: {} ({} min)", m.zone_timer, zone_timer_min);
    println!("  MULT_SIDS: {}", m.mult_sids);
    println!("  MULT_NIDS: {}", m.mult_nids);
    println!("  BASE_ID: {}", m.base_id);
    println!("  BASE_CLASS: {}", m.base_class);
    println!("  PAGE_CHAN: {}", m.page_chan);
    println!("  MAX_SLOT_CYCLE_INDEX: {}", m.max_slot_cycle_index);
    println!("  --- Registration ---");
    println!("  HOME_REG: {}", m.home_reg);
    println!("  FOR_SID_REG: {}", m.for_sid_reg);
    println!("  FOR_NID_REG: {}", m.for_nid_reg);
    println!("  POWER_UP_REG: {}", m.power_up_reg);
    println!("  POWER_DOWN_REG: {}", m.power_down_reg);
    println!("  PARAMETER_REG: {}", m.parameter_reg);
    println!("  REG_PRD: {}", m.reg_prd);
    println!("  --- Location ---");
    println!(
        "  BASE_LAT: {} ({:.6} deg)",
        m.base_lat,
        base_lat_to_degrees(m.base_lat)
    );
    println!(
        "  BASE_LONG: {} ({:.6} deg)",
        m.base_long,
        base_long_to_degrees(m.base_long)
    );
    println!("  REG_DIST: {}", m.reg_dist);
    println!("  --- Search Windows ---");
    println!(
        "  SRCH_WIN_A: {} ({} chips)",
        m.srch_win_a,
        srch_win_chips(m.srch_win_a)
    );
    println!(
        "  SRCH_WIN_N: {} ({} chips)",
        m.srch_win_n,
        srch_win_chips(m.srch_win_n)
    );
    println!(
        "  SRCH_WIN_R: {} ({} chips)",
        m.srch_win_r,
        srch_win_chips(m.srch_win_r)
    );
    println!("  NGHBR_MAX_AGE: {}", m.nghbr_max_age);
    println!("  --- Power ---");
    println!("  PWR_REP_THRESH: {}", m.pwr_rep_thresh);
    println!("  PWR_REP_FRAMES: {}", m.pwr_rep_frames);
    println!("  PWR_THRESH_ENABLE: {}", m.pwr_thresh_enable);
    println!("  PWR_PERIOD_ENABLE: {}", m.pwr_period_enable);
    println!("  PWR_REP_DELAY: {}", m.pwr_rep_delay);
    println!("  RESCAN: {}", m.rescan);
    println!("  --- Pilot Thresholds ---");
    println!("  T_ADD: {} ({:.1} dB)", m.t_add, m.t_add as f64 * 0.5);
    println!("  T_DROP: {} ({:.1} dB)", m.t_drop, m.t_drop as f64 * 0.5);
    println!("  T_COMP: {} ({:.1} dB)", m.t_comp, m.t_comp as f64 * 0.5);
    println!("  T_TDROP: {}", m.t_tdrop);
    println!("  --- Overhead Msg Flags ---");
    println!("  EXT_SYS_PARAMETER: {}", m.ext_sys_parameter);
    println!("  EXT_NGHBR_LST: {}", m.ext_nghbr_lst);
    println!("  GEN_NGHBR_LST: {}", m.gen_nghbr_lst);
    println!("  GLOBAL_REDIRECT: {}", m.global_redirect);
    println!("  PRI_NGHBR_LST: {}", m.pri_nghbr_lst);
    println!("  USER_ZONE_ID: {}", m.user_zone_id);
    println!("  EXT_GLOBAL_REDIRECT: {}", m.ext_global_redirect);
    println!("  EXT_CHAN_LST: {}", m.ext_chan_lst);
}

fn print_access_parameters(m: &AccessParametersMessage) {
    println!("  Message: Access Parameters Message (APM)");
    println!("  PD: {}", m.header.pd);
    println!(
        "  PILOT_PN: {} (offset = {} chips)",
        m.pilot_pn,
        m.pilot_pn as u32 * 64
    );
    println!("  ACC_MSG_SEQ: {}", m.acc_msg_seq);
    println!("  ACC_CHAN: {}", m.acc_chan);
    println!("  NOM_PWR: {} dB", m.nom_pwr);
    println!("  INIT_PWR: {} dB", m.init_pwr);
    println!("  PWR_STEP: {} dB", m.pwr_step);
    println!("  NUM_STEP: {}", m.num_step);
    println!("  MAX_CAP_SZ: {} frames", max_cap_sz_frames(m.max_cap_sz));
    println!("  PAM_SZ: {} frames", pam_sz_frames(m.pam_sz));
    println!("  PSIST(0-9): {}", m.psist_0_9);
    println!("  PSIST(10): {}", m.psist_10);
    println!("  PSIST(11): {}", m.psist_11);
    println!("  PSIST(12): {}", m.psist_12);
    println!("  PSIST(13): {}", m.psist_13);
    println!("  PSIST(14): {}", m.psist_14);
    println!("  PSIST(15): {}", m.psist_15);
    println!("  MSG_PSIST: {}", m.msg_psist);
    println!("  REG_PSIST: {}", m.reg_psist);
    println!("  PROBE_PN_RAN: {}", m.probe_pn_ran);
    println!("  ACC_TMO: {}", m.acc_tmo);
    println!("  PROBE_BKOFF: {}", m.probe_bkoff);
    println!("  BKOFF: {}", m.bkoff);
    println!("  MAX_REQ_SEQ: {}", m.max_req_seq);
    println!("  MAX_RSP_SEQ: {}", m.max_rsp_seq);
    println!("  AUTH: {}", m.auth);
    if m.auth != 0 {
        println!("  RAND: 0x{:08X}", m.rand);
    }
    println!("  NOM_PWR_EXT: {}", m.nom_pwr_ext);
    println!("  PSIST_EMG_INCL: {}", m.psist_emg_incl);
    if let Some(psist_emg) = m.psist_emg {
        println!("  PSIST_EMG: {}", psist_emg);
    }
    println!("  ACCT_INCL: {}", m.acct_incl);
}

fn print_neighbor_list(m: &NeighborListMessage) {
    println!("  Message: Neighbor List Message (NLM)");
    println!("  PD: {}", m.header.pd);
    println!(
        "  PILOT_PN: {} (offset = {} chips)",
        m.pilot_pn,
        m.pilot_pn as u32 * 64
    );
    println!("  CONFIG_MSG_SEQ: {}", m.config_msg_seq);
    println!("  PILOT_INC: {}", m.pilot_inc);
    println!("  Neighbors ({}):", m.neighbors.len());
    for (i, n) in m.neighbors.iter().enumerate() {
        println!(
            "    [{}] NGHBR_PN: {} (offset = {} chips)",
            i,
            n.nghbr_pn,
            n.nghbr_pn as u32 * 64
        );
    }
}

fn print_cdma_channel_list(m: &CdmaChannelListMessage) {
    println!("  Message: CDMA Channel List Message (CCLM)");
    println!("  PD: {}", m.header.pd);
    println!(
        "  PILOT_PN: {} (offset = {} chips)",
        m.pilot_pn,
        m.pilot_pn as u32 * 64
    );
    println!("  CONFIG_MSG_SEQ: {}", m.config_msg_seq);
    println!("  Channels ({}):", m.channels.len());
    for (i, ch) in m.channels.iter().enumerate() {
        println!(
            "    [{}] CDMA_FREQ: {} ({:.2} MHz)",
            i,
            ch,
            cdma_freq_to_mhz(*ch)
        );
    }
}

fn print_general_page(m: &GeneralPageMessage) {
    println!("  Message: General Page Message (GPM)");
    println!("  PD: {}", m.header.pd);
    println!("  --- Common Fields ---");
    println!("  CONFIG_MSG_SEQ: {}", m.config_msg_seq);
    println!("  ACC_MSG_SEQ: {}", m.acc_msg_seq);
    println!("  CLASS_0_DONE: {}", m.class_0_done);
    println!("  CLASS_1_DONE: {}", m.class_1_done);
    println!("  TMSI_DONE: {}", m.tmsi_done);
    println!("  ORDERED_TMSIS: {}", m.ordered_tmsis);
    println!("  BROADCAST_DONE: {}", m.broadcast_done);
    println!("  ADD_LENGTH: {}", m.add_length);
    if !m.add_pfield.is_empty() {
        let hex: String = m
            .add_pfield
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        println!("  ADD_PFIELD: {}", hex);
    }
    println!("  --- Page Records ({}) ---", m.page_records.len());
    if m.page_records.is_empty() {
        println!("  (none — overhead-only GPM)");
    }
    for (i, rec) in m.page_records.iter().enumerate() {
        match rec {
            PageRecord::Class0 {
                page_subclass,
                msg_seq,
                imsi_s,
                imsi_11_12,
                mcc,
                imsi_addr_num,
                imsi_m_s1,
                imsi_m_s2,
                special_service,
                service_option,
            } => {
                println!(
                    "    [{}] Class 0 (IMSI) subclass={} msg_seq={}",
                    i, page_subclass, msg_seq
                );
                if let Some(v) = imsi_s {
                    println!("        IMSI_S: 0x{:09x} ({})", v, imsi_s_to_min(*v));
                }
                if let Some(v) = imsi_11_12 {
                    println!("        IMSI_11_12: {}", v);
                }
                if let Some(v) = mcc {
                    println!("        MCC: {}", v);
                }
                if let Some(v) = imsi_addr_num {
                    println!("        IMSI_ADDR_NUM: {}", v);
                }
                if let Some(v) = imsi_m_s1 {
                    println!("        IMSI_M_S1: 0x{:06x}", v);
                }
                if let Some(v) = imsi_m_s2 {
                    println!("        IMSI_M_S2: 0x{:03x}", v);
                }
                print_special_service(*special_service, *service_option);
            }
            PageRecord::Class1 {
                msg_seq,
                esn,
                special_service,
                service_option,
            } => {
                println!("    [{}] Class 1 (ESN) msg_seq={}", i, msg_seq);
                println!("        ESN: 0x{:08x}", esn);
                print_special_service(*special_service, *service_option);
            }
            PageRecord::Tmsi {
                msg_seq,
                tmsi_code_addr,
                special_service,
                service_option,
            } => {
                println!("    [{}] Class 2 (TMSI) msg_seq={}", i, msg_seq);
                println!("        TMSI_CODE_ADDR: 0x{:08x}", tmsi_code_addr);
                print_special_service(*special_service, *service_option);
            }
            PageRecord::Broadcast { bc_addr } => {
                println!("    [{}] Class 3 (Broadcast)", i);
                println!("        BC_ADDR: 0x{:04x}", bc_addr);
            }
        }
    }
}

fn print_extended_system_parameters(m: &ExtendedSystemParametersMessage) {
    println!("  Message: Extended System Parameters Message (ESPM)");
    println!("  PD: {}", m.header.pd);
    println!(
        "  PILOT_PN: {} (offset = {} chips)",
        m.pilot_pn,
        m.pilot_pn as u32 * 64
    );
    println!("  CONFIG_MSG_SEQ: {}", m.config_msg_seq);
    println!("  DELETE_FOR_TMSI: {}", m.delete_for_tmsi as u8);
    println!("  USE_TMSI: {}", m.use_tmsi as u8);
    println!("  PREF_MSID_TYPE: {}", m.pref_msid_type);
    println!("  MCC: {}", m.mcc);
    println!("  IMSI_11_12: {}", m.imsi_11_12);
    println!("  TMSI_ZONE_LEN: {}", m.tmsi_zone_len);
    println!("  TMSI_ZONE: {:02x?}", m.tmsi_zone);
    println!("  BCAST_INDEX: {}", m.bcast_index);
    println!("  IMSI_T_SUPPORTED: {}", m.imsi_t_supported as u8);
    println!("  P_REV: {}", m.p_rev);
    println!("  MIN_P_REV: {}", m.min_p_rev);
    println!("  SOFT_SLOPE: {}", m.soft_slope);
    println!("  ADD_INTERCEPT: {}", m.add_intercept);
    println!("  DROP_INTERCEPT: {}", m.drop_intercept);
    println!("  PACKET_ZONE_ID: {}", m.packet_zone_id);
    println!("  MAX_NUM_ALT_SO: {}", m.max_num_alt_so);
    println!("  RESELECT_INCLUDED: {}", m.reselect_included);
    println!("  PILOT_REPORT: {}", m.pilot_report);
    println!("  NGHBR_SET_ENTRY_INFO: {}", m.nghbr_set_entry_info);
    println!("  NGHBR_SET_ACCESS_INFO: {}", m.nghbr_set_access_info);
    println!("  BROADCAST_GPS_ASST: {}", m.broadcast_gps_asst);
    println!("  QPCH_SUPPORTED: {}", m.qpch_supported);
    println!("  SDB_SUPPORTED: {}", m.sdb_supported);
    println!("  RLGAIN_TRAFFIC_PILOT: {}", m.rlgain_traffic_pilot);
    println!("  REV_PWR_CNTL_DELAY_INCL: {}", m.rev_pwr_cntl_delay_incl);
    println!("  AUTO_MSG_SUPPORTED: {}", m.auto_msg_supported);
    println!("  MOB_QOS: {}", m.mob_qos);
    println!("  ENC_SUPPORTED: {}", m.enc_supported);
    println!("  USE_SYNC_ID: {}", m.use_sync_id);
    println!("  CS_SUPPORTED: {}", m.cs_supported);
    println!("  BCCH_SUPPORTED: {}", m.bcch_supported);
    println!("  MS_INIT_POS_LOC_SUP_IND: {}", m.ms_init_pos_loc_sup_ind);
    println!("  PILOT_INFO_REQ_SUPPORTED: {}", m.pilot_info_req_supported);
}

fn print_special_service(special_service: bool, service_option: Option<u16>) {
    if special_service {
        print!("        SPECIAL_SERVICE: true");
        if let Some(so) = service_option {
            println!(" SERVICE_OPTION: {} ({})", so, service_option_name(so));
        } else {
            println!();
        }
    }
}

fn service_option_name(so: u16) -> &'static str {
    match so {
        1 => "Basic Variable Rate Voice (8k)",
        2 => "Mobile Station Loopback (8k)",
        3 => "Enhanced Variable Rate Voice (EVRC)",
        4 => "Async Data (9.6 kbps)",
        5 => "Group 3 Fax (9.6 kbps)",
        6 => "SMS (Rate Set 1)",
        7 => "PPP Packet Data",
        9 => "Mobile Station Loopback (13k)",
        12 => "Async Data (14.4 kbps)",
        13 => "Group 3 Fax (14.4 kbps)",
        17 => "High Rate Voice (13k)",
        32 => "TDSO",
        33 => "FDSO",
        54 => "EVRC-NW",
        68 => "EVRC-B",
        73 => "EVRC-WB",
        _ => "Unknown",
    }
}

/// Decode IMSI_S (34 bits) into a MIN-like decimal string.
/// IMSI_S is split into IMSI_S2 (10 bits, upper) and IMSI_S1 (24 bits, lower).
fn imsi_s_to_min(imsi_s: u64) -> String {
    let imsi_s1 = (imsi_s & 0xFFFFFF) as u32; // lower 24 bits
    let imsi_s2 = ((imsi_s >> 24) & 0x3FF) as u16; // upper 10 bits
    format!("S2={} S1={}", imsi_s2, imsi_s1)
}

fn print_order(m: &OrderMessage) {
    println!("  Message: Order Message");
    println!("  PD: {}", m.header.pd);
    println!("  ORDER: {} ({})", m.order, order_name(m.order));
    println!("  ORDQ: {}", m.ordq);
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// BASE_LAT is in units of 0.25 arc-seconds.
fn base_lat_to_degrees(raw: u32) -> f64 {
    // Signed 22-bit two's complement
    let val = if raw & (1 << 21) != 0 {
        (raw as i32) - (1 << 22)
    } else {
        raw as i32
    };
    val as f64 * 0.25 / 3600.0
}

/// BASE_LONG is in units of 0.25 arc-seconds.
fn base_long_to_degrees(raw: u32) -> f64 {
    // Signed 23-bit two's complement
    let val = if raw & (1 << 22) != 0 {
        (raw as i32) - (1 << 23)
    } else {
        raw as i32
    };
    val as f64 * 0.25 / 3600.0
}

/// Search window size table (C.S0005-E Table 2.6.6.2.1-1).
fn srch_win_chips(idx: u8) -> u32 {
    match idx {
        0 => 4,
        1 => 6,
        2 => 8,
        3 => 10,
        4 => 14,
        5 => 20,
        6 => 28,
        7 => 40,
        8 => 60,
        9 => 80,
        10 => 114,
        11 => 160,
        12 => 226,
        13 => 320,
        14 => 452,
        15 => 1023, // 226*2 rounded / full window
        _ => 0,
    }
}

/// Sign-extend a `bits`-wide unsigned value to a 32-bit signed integer.
fn sign_extend(value: u32, bits: u32) -> i32 {
    let shift = 32 - bits;
    ((value << shift) as i32) >> shift
}

fn max_cap_sz_frames(val: u8) -> u16 {
    // MAX_CAP_SZ is encoded as "maximum capsule frames minus 3".
    val as u16 + 3
}

fn pam_sz_frames(val: u8) -> u16 {
    // PAM_SZ is encoded as "preamble frames minus 1".
    val as u16 + 1
}

/// CDMA_FREQ to MHz (cellular band).
fn cdma_freq_to_mhz(freq: u16) -> f64 {
    // CDMA_FREQ * 0.050 + 0.000 for band class 0 (cellular 800 MHz)
    freq as f64 * 0.050
}

fn order_name(order: u8) -> &'static str {
    match order {
        0 => "Abbreviated Alert",
        1 => "Base Station Acknowledgment",
        2 => "Base Station Challenge Confirmation",
        0b011011 => "Registration Accepted",
        0b011100 => "Registration Rejected",
        5 => "Maintenance Required",
        6 => "Lock Until Power-Cycled",
        7 => "Unlock",
        8 => "Parameter Update",
        18 => "Release",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;

    use super::{PagingMessage, max_cap_sz_frames, pam_sz_frames};

    #[test]
    fn test_access_parameters_frame_counts_are_offset_encoded() {
        assert_eq!(max_cap_sz_frames(0), 3);
        assert_eq!(max_cap_sz_frames(7), 10);
        assert_eq!(pam_sz_frames(0), 1);
        assert_eq!(pam_sz_frames(15), 16);
    }

    #[test]
    fn test_paging_decode_rejects_unmapped_message_type() {
        let mut bits = Bitstream::new();
        bits.write_u8(0x00, 8);

        let err = PagingMessage::decode(&bits).unwrap_err();

        assert!(err.contains("unsupported f-csch MSG_TYPE 0x00"));
    }

    #[test]
    fn test_paging_decode_rejects_unsupported_mapped_body() {
        let mut bits = Bitstream::new();
        bits.write_u8(0x0C, 8); // Feature Notification Message

        let err = PagingMessage::decode(&bits).unwrap_err();

        assert!(err.contains("unsupported f-csch body decode for FNM"));
    }
}
