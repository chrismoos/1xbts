mod modulation;
mod status;

use modulation::ACCESS_INFO_BITS;
pub use modulation::{
    AccessChannelConfig, default_lc_state_at, lc_state_at, modulate_access_probe, pulse_shape,
};
pub use status::build_status_response_capsule;
pub(crate) use status::status_response_sdu;

use cdma_bts::lac::crc30;
use cdma_common::access::{
    AccessMessage, AccessMessageHeader, FchTypeSpecificFields, OriginationMessage,
};
use cdma_common::bits::Bitstream;
use cdma_common::lac::message_types::MessageId;
use cdma_common::paging::{imsi_11_12_from_digits, imsi_s_from_imsi, mcc_from_digits};

/// r-csch reverse common signaling protocol discriminator for the legacy
/// (P_REV_IN_USE < 6) access PDU wrapper: PD = 00.
const PD_LEGACY: u8 = 0b00;

/// Reverse-common wire message type for a Registration Message (C.S0004-E).
const MSG_TYPE_REGISTRATION: u8 = 0x01;

/// MSID_TYPE for ESN-only addressing (C.S0004-E 2.1.1.4.2 addressing).
const MSID_TYPE_ESN: u8 = 0b001;

/// CRC-30 field width and its byte-aligned capsule accounting.
const CRC30_BITS: usize = 30;
const MSG_LENGTH_BITS: usize = 8;

#[derive(Debug, Clone)]
pub struct MsAccessIdentity {
    pub esn: u32,
    /// IMSI_S (the 10-digit MIN-equivalent), conveyed as IMSI_M_S1/S2 in the
    /// P_REV6 addressing so the network can identify the subscriber.
    pub imsi_s: u64,
    pub mcc: String,
    pub imsi_11_12: String,
    /// Mobile protocol revision (6 for IS-2000 Rev A signaling).
    pub mob_p_rev: u8,
    pub scm: u8,
    pub slot_cycle_index: u8,
    pub mob_term: bool,
    /// Mobile Equipment Identifier, 14 hexadecimal digits, when the mobile has
    /// one to report. The station class mark claims MEID support, so a base
    /// station may ask for it by name.
    pub meid: Option<String>,
}

impl Default for MsAccessIdentity {
    fn default() -> Self {
        MsAccessIdentity {
            esn: 0,
            imsi_s: 0,
            mcc: DEFAULT_MCC.to_string(),
            imsi_11_12: DEFAULT_IMSI_11_12.to_string(),
            mob_p_rev: 6,
            scm: SCM_SLOTTED | SCM_MEID_SUPPORT | SCM_25MHZ_BANDWIDTH,
            slot_cycle_index: 0,
            mob_term: true,
            meid: None,
        }
    }
}

/// The ARQ fields of an r-csch PDU (C.S0004-E §2.1.1.2.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccessArq {
    /// MSG_SEQ of the base station PDU this one acknowledges, meaningful
    /// when `valid_ack` is set.
    pub ack_seq: u8,
    pub msg_seq: u8,
    pub ack_req: bool,
    pub valid_ack: bool,
}

/// ACK_SEQ value of a PDU that acknowledges nothing.
const ACK_SEQ_NONE: u8 = 0b111;

impl AccessArq {
    pub fn assured(msg_seq: u8) -> Self {
        AccessArq {
            ack_seq: 0,
            msg_seq,
            ack_req: true,
            valid_ack: false,
        }
    }

    fn write(&self, bs: &mut Bitstream) {
        // ACK_SEQ(3) MSG_SEQ(3) ACK_REQ(1) VALID_ACK(1) ACK_TYPE(3). A PDU
        // that acknowledges nothing carries ACK_SEQ '111' (C.S0004-E
        // §2.1.1.2.1.1).
        let ack_seq = if self.valid_ack {
            self.ack_seq & 0x07
        } else {
            ACK_SEQ_NONE
        };
        bs.write_u8(ack_seq, 3);
        bs.write_u8(self.msg_seq & 0x07, 3);
        bs.write_u8(self.ack_req as u8, 1);
        bs.write_u8(self.valid_ack as u8, 1);
        bs.write_u8(0, 3);
    }
}

/// Encode an r-csch legacy PD=00 access PDU wrapping a message SDU.
///
/// Layout (C.S0004-E 2.1.1.4.2, the inverse of `ReverseAccessPdu::decode`):
/// `PD(2) MSG_TYPE(6) | ARQ(11) | ADDRESSING(ESN) | AUTH(2) | SDU`.
pub fn encode_access_pdu(msg_type: u8, esn: u32, arq: &AccessArq, sdu: &Bitstream) -> Bitstream {
    let mut bs = Bitstream::new();

    bs.write_u8((PD_LEGACY << 6) | (msg_type & 0x3f), 8);
    arq.write(&mut bs);

    // Addressing: MSID_TYPE(3) MSID_LEN(4, octets) ESN(32).
    bs.write_u8(MSID_TYPE_ESN, 3);
    bs.write_u8(4, 4);
    bs.write_u32(esn, 32);

    // Authentication: MACI_INCL(1)=0 AUTH_INCL(1)=0 (no authentication).
    bs.write_u8(0, 1);
    bs.write_u8(0, 1);

    bs.extend(sdu);
    bs
}

/// r-csch protocol discriminator for the P_REV≥6 access PDU wrapper: PD = 01.
const PD_PREV6: u8 = 0b01;

/// First protocol revision that uses the PD=01 wrapper and carries the
/// IS-2000 fields in access message bodies.
const P_REV_IS2000: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServingSystem {
    pub p_rev_in_use: u8,
    /// ACTIVE_PILOT_STRENGTH for the radio environment report, in 0.5 dB
    /// steps of pilot Ec/Io below 0 dB (C.S0004-E §2.1.1.4.1.2).
    pub active_pilot_strength: u8,
    /// Whether the serving band class takes the extended station class mark
    /// (SCM bit 7), which band class 1 and 4 do (C.S0005-E Table 2.3.3-1).
    pub extended_scm: bool,
    /// MCC and IMSI_11_12 the base station broadcasts, in wire encoding. The
    /// mobile leaves out of its address whichever of them match its own
    /// (C.S0004-E §2.1.1.3.1.3). `None` until the Extended System Parameters
    /// Message has supplied them.
    pub base_mcc: Option<u16>,
    pub base_imsi_11_12: Option<u8>,
}

impl ServingSystem {
    pub fn at_p_rev(p_rev_in_use: u8) -> Self {
        ServingSystem {
            p_rev_in_use,
            active_pilot_strength: 0,
            extended_scm: false,
            base_mcc: None,
            base_imsi_11_12: None,
        }
    }

    fn scm(&self, id: &MsAccessIdentity) -> u8 {
        if self.extended_scm {
            id.scm | SCM_EXTENDED
        } else {
            id.scm
        }
    }
}

/// Slotted-mode capable, SCM bit 5.
const SCM_SLOTTED: u8 = 0x20;
/// MEID support indicator, required in access messages at P_REV 6.
const SCM_MEID_SUPPORT: u8 = 0x10;
/// 25 MHz bandwidth, SCM bit 3, which C.S0005-E Table 2.3.3-1 fixes at 1.
const SCM_25MHZ_BANDWIDTH: u8 = 0x08;
/// Extended SCM indicator, SCM bit 7.
const SCM_EXTENDED: u8 = 0x80;
const PILOT_STRENGTH_MAX: u8 = 63;

/// ACTIVE_PILOT_STRENGTH for a pilot measured at `ec_io_db`:
/// `floor(-2 * Ec/Io)`, clamped to the field. An unmeasured pilot reports 0.
pub fn pilot_strength(ec_io_db: f32) -> u8 {
    if ec_io_db.is_nan() {
        return 0;
    }
    (-2.0 * ec_io_db)
        .floor()
        .clamp(0.0, PILOT_STRENGTH_MAX as f32) as u8
}

/// P_REV_IN_USE is the lower of mobile and serving P_REV (C.S0005-E §2.6.2.2.5).
pub fn p_rev_in_use(base_station_p_rev: u8, mob_p_rev: u8) -> u8 {
    base_station_p_rev.min(mob_p_rev)
}

/// Encode an r-csch access PDU in the wrapper `p_rev_in_use` calls for: PD=00
/// with ESN addressing below P_REV 6, PD=01 with IMSI and ESN addressing from
/// P_REV 6 on.
pub fn encode_r_csch_pdu(
    serving: &ServingSystem,
    msg_type: u8,
    id: &MsAccessIdentity,
    arq: &AccessArq,
    sdu: &Bitstream,
) -> Bitstream {
    if serving.p_rev_in_use >= P_REV_IS2000 {
        encode_pd01_p_rev6_pdu(msg_type, id, arq, sdu, serving)
    } else {
        encode_access_pdu(msg_type, id.esn, arq, sdu)
    }
}

pub const DEFAULT_MCC: &str = "310";
pub const DEFAULT_IMSI_11_12: &str = "00";

/// MSID_TYPE for IMSI + ESN addressing (C.S0004-E). The MSID is
/// `ESN(32) | IMSI_CLASS(1) | class-specific IMSI fields`.
const MSID_TYPE_IMSI_ESN: u8 = 0b011;

/// Encode a P_REV≥6 r-csch access PDU (PD=01) wrapping a message SDU, addressing
/// the mobile by IMSI_S + ESN so the network learns the subscriber identity.
///
/// Layout (the inverse of `decode_pd01_p_rev6`): `PD(2)=01 MSG_TYPE(6) |
/// LAC_LENGTH(5) | LAC region [ARQ(11) | ADDRESSING | AUTH(2) | pad] |
/// RADIO_ENV_REPORT(11) | SDU`. LAC_LENGTH counts the whole LAC region in octets
/// (including its own 5 bits).
pub fn encode_pd01_p_rev6_pdu(
    msg_type: u8,
    id: &MsAccessIdentity,
    arq: &AccessArq,
    sdu: &Bitstream,
    serving: &ServingSystem,
) -> Bitstream {
    let (imsi_m_s1, imsi_m_s2) = imsi_s_from_imsi(&format!("{:010}", id.imsi_s)).unwrap_or((0, 0));
    let mcc = mcc_from_digits(&id.mcc).unwrap_or(0);
    let imsi_11_12 = imsi_11_12_from_digits(&id.imsi_11_12).unwrap_or(0);
    let mcc_matches = serving.base_mcc == Some(mcc);
    let imsi_11_12_matches = serving.base_imsi_11_12 == Some(imsi_11_12);

    let mut lac = Bitstream::new();
    arq.write(&mut lac);
    // MSID = ESN(32) IMSI_CLASS(1)=0 IMSI_CLASS_0_TYPE(2) RESERVED [MCC(10)]
    // [IMSI_11_12(7)] IMSI_S(34), with RESERVED sized to end on an octet.
    // IMSI_S = IMSI_M_S2(10) << 24 | IMSI_M_S1(24).
    let mut msid = Bitstream::new();
    msid.write_u32(id.esn, 32);
    msid.write_u8(0, 1);
    match (mcc_matches, imsi_11_12_matches) {
        (true, true) => {
            msid.write_u8(0b00, 2);
            msid.write_u8(0, 3);
        }
        (true, false) => {
            msid.write_u8(0b01, 2);
            msid.write_u8(0, 4);
            msid.write_u8(imsi_11_12, 7);
        }
        (false, true) => {
            msid.write_u8(0b10, 2);
            msid.write_u8(0, 1);
            msid.write_u32(mcc as u32, 10);
        }
        (false, false) => {
            msid.write_u8(0b11, 2);
            msid.write_u8(0, 2);
            msid.write_u32(mcc as u32, 10);
            msid.write_u8(imsi_11_12, 7);
        }
    }
    msid.write_u32(imsi_m_s2 as u32, 10);
    msid.write_u32(imsi_m_s1, 24);
    lac.write_u8(MSID_TYPE_IMSI_ESN, 3);
    lac.write_u8((msid.len() / 8) as u8, 4);
    lac.extend(&msid);
    lac.write_u8(0, 1);
    lac.write_u8(0, 1);

    // LAC_LENGTH covers the 5-bit length field plus the LAC region body, rounded
    // up to a whole octet.
    let lac_total_bits = 5 + lac.len();
    let lac_length_octets = lac_total_bits.div_ceil(8) as u32;
    let lac_pad = lac_length_octets as usize * 8 - 5 - lac.len();

    let mut bs = Bitstream::new();
    bs.write_u8((PD_PREV6 << 6) | (msg_type & 0x3f), 8);
    bs.write_u32(lac_length_octets, 5);
    bs.extend(&lac);
    for _ in 0..lac_pad {
        bs.write_u8(0, 1);
    }
    // Radio environment report: ACTIVE_PILOT_STRENGTH(6) FIRST_IS_ACTIVE(1)
    // FIRST_IS_PTA(1) NUM_ADD_PILOTS(3)=0.
    bs.write_u8(serving.active_pilot_strength.min(PILOT_STRENGTH_MAX), 6);
    bs.write_u8(1, 1);
    bs.write_u8(0, 1);
    bs.write_u8(0, 3);

    bs.extend(sdu);
    bs
}

/// Registration Message SDU (C.S0005-E 2.7.1.3.2.1):
/// `REG_TYPE(4) SLOT_CYCLE_INDEX(3) MOB_P_REV(8) SCM(8) MOB_TERM(1)
/// RETURN_CAUSE(4)`, then `QPCH_SUPPORTED(1) ENHANCED_RC(1) UZID_INCL(1)` when
/// P_REV_IN_USE is 6 or higher.
fn registration_sdu(id: &MsAccessIdentity, reg_type: u8, serving: &ServingSystem) -> Bitstream {
    let mut sdu = Bitstream::new();
    sdu.write_u8(reg_type & 0x0f, 4);
    sdu.write_u8(id.slot_cycle_index & 0x07, 3);
    sdu.write_u8(id.mob_p_rev, 8);
    sdu.write_u8(serving.scm(id), 8);
    sdu.write_u8(u8::from(id.mob_term), 1);
    sdu.write_u8(0, 4);
    if serving.p_rev_in_use >= P_REV_IS2000 {
        sdu.write_u8(0, 1);
        sdu.write_u8(1, 1);
        sdu.write_u8(0, 1);
    }
    sdu
}

pub fn encode_registration_pdu(
    id: &MsAccessIdentity,
    reg_type: u8,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    encode_r_csch_pdu(
        serving,
        MSG_TYPE_REGISTRATION,
        id,
        arq,
        &registration_sdu(id, reg_type, serving),
    )
}

/// Reverse-common wire message type for an Order Message (C.S0004-E).
const MSG_TYPE_ORDER: u8 = 0x02;
/// Mobile Station Acknowledgment Order (C.S0005-E Table 2.7.3-1).
pub const ORDER_MS_ACK: u8 = 0b010000;

pub fn encode_ms_ack_order_pdu(
    id: &MsAccessIdentity,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    let mut sdu = Bitstream::new();
    sdu.write_u8(ORDER_MS_ACK, 6);
    sdu.write_u8(0, 3);
    encode_r_csch_pdu(serving, MSG_TYPE_ORDER, id, arq, &sdu)
}

pub fn build_ms_ack_order_capsule(
    id: &MsAccessIdentity,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    sar_encapsulate(&encode_ms_ack_order_pdu(id, arq, serving))
}

/// Wrap a raw access PDU in the SAR capsule the access frame reader expects:
/// `MSG_LENGTH(8) | PDU | pad | CRC30(30)`, octet-aligned. CRC-30 covers
/// `MSG_LENGTH | PDU | pad`.
pub fn sar_encapsulate(pdu: &Bitstream) -> Bitstream {
    let body_bits = MSG_LENGTH_BITS + pdu.len() + CRC30_BITS;
    let pad = (8 - (body_bits % 8)) % 8;
    let msg_length_octets = ((body_bits + pad) / 8) as u32;

    let mut scope = Bitstream::new();
    scope.write_u32(msg_length_octets, MSG_LENGTH_BITS);
    scope.extend(pdu);
    for _ in 0..pad {
        scope.write_u8(0, 1);
    }

    let crc = crc30(&scope);
    let mut capsule = scope;
    capsule.write_u32(crc, CRC30_BITS);
    capsule
}

pub fn build_registration_capsule(
    id: &MsAccessIdentity,
    reg_type: u8,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    let pdu = encode_registration_pdu(id, reg_type, arq, serving);
    sar_encapsulate(&pdu)
}

/// Access Channel frames a capsule of `bits` occupies (88 information bits
/// per frame).
pub fn capsule_frames(bits: usize) -> usize {
    bits.div_ceil(ACCESS_INFO_BITS).max(1)
}

/// Reverse-common wire message type for a Page Response Message (C.S0004-E).
const MSG_TYPE_PAGE_RESPONSE: u8 = 0x05;

/// CH_IND value requesting a Fundamental Channel.
const CH_IND_FCH: u8 = 0b01;
const PREFERRED_RC: u8 = 3;

/// Page Response Message SDU (C.S0005-E 2.7.1.3.2.5):
/// `MOB_TERM(1) SLOT_CYCLE_INDEX(3) MOB_P_REV(8) SCM(8) REQUEST_MODE(3)
/// SERVICE_OPTION(16) PM(1) NAR_AN_CAP(1) NUM_ALT_SO(3)`, then the channel and
/// radio-configuration capability fields when P_REV_IN_USE is 6 or higher.
fn page_response_sdu(
    id: &MsAccessIdentity,
    service_option: u16,
    serving: &ServingSystem,
) -> Bitstream {
    let mut sdu = Bitstream::new();
    sdu.write_u8(u8::from(id.mob_term), 1);
    sdu.write_u8(id.slot_cycle_index & 0x07, 3);
    sdu.write_u8(id.mob_p_rev, 8);
    sdu.write_u8(serving.scm(id), 8);
    sdu.write_u8(0, 3);
    sdu.write_u32(service_option as u32, 16);
    sdu.write_u8(0, 1);
    sdu.write_u8(1, 1);
    sdu.write_u8(0, 3);
    if serving.p_rev_in_use >= P_REV_IS2000 {
        let fch = fch_rc3_capability();
        // UZID_INCL, CH_IND, OTD_SUPPORTED, QPCH_SUPPORTED, ENHANCED_RC.
        sdu.write_u8(0, 1);
        sdu.write_u8(CH_IND_FCH, 2);
        sdu.write_u8(0, 1);
        sdu.write_u8(0, 1);
        sdu.write_u8(1, 1);
        sdu.write_u8(PREFERRED_RC, 5);
        sdu.write_u8(PREFERRED_RC, 5);
        // FCH_SUPPORTED and its type-specific fields.
        sdu.write_u8(1, 1);
        sdu.write_u8(u8::from(fch.frame_size_5ms_supported), 1);
        sdu.write_u8(fch.for_fch_len, 3);
        sdu.extend(&fch.for_fch_rc_map_raw);
        sdu.write_u8(fch.rev_fch_len, 3);
        sdu.extend(&fch.rev_fch_rc_map_raw);
        // DCCH_SUPPORTED, REV_FCH_GATING_REQ.
        sdu.write_u8(0, 1);
        sdu.write_u8(0, 1);
    }
    sdu
}

pub fn encode_page_response_pdu(
    id: &MsAccessIdentity,
    service_option: u16,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    encode_r_csch_pdu(
        serving,
        MSG_TYPE_PAGE_RESPONSE,
        id,
        arq,
        &page_response_sdu(id, service_option, serving),
    )
}

pub fn build_page_response_capsule(
    id: &MsAccessIdentity,
    service_option: u16,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    sar_encapsulate(&encode_page_response_pdu(id, service_option, arq, serving))
}

/// Reverse-common wire message type for an Origination Message (C.S0004-E).
const MSG_TYPE_ORIGINATION: u8 = 0x04;

/// SMS teleservice service option (IS-707 SMS on Rate Set 1).
pub const SERVICE_OPTION_SMS: u16 = 6;

/// FCH capability advertising Radio Configuration 3 (plus RC1/RC2) on both
/// links, so the network can grant an RC3 traffic channel. The RC map is a
/// 3-bit field where positions map to RCs [1, 2, 3]. `0b111` sets all three.
fn fch_rc3_capability() -> FchTypeSpecificFields {
    let mut for_map = Bitstream::new();
    for_map.write_u8(0b111, 3);
    let mut rev_map = Bitstream::new();
    rev_map.write_u8(0b111, 3);
    FchTypeSpecificFields {
        frame_size_5ms_supported: false,
        for_fch_len: 1,
        for_fch_rc_map_raw: for_map,
        for_supported_rcs: vec![1, 2, 3],
        rev_fch_len: 1,
        rev_fch_rc_map_raw: rev_map,
        rev_supported_rcs: vec![1, 2, 3],
    }
}

/// Origination Message advertising P_REV 6 + RC3 FCH for `service_option`. The
/// P_REV≥7 tail fields stay absent (`mob_p_rev = 6` gates them off in both the
/// encoder and decoder). `special_service` carries the explicit service option.
fn origination_message(
    id: &MsAccessIdentity,
    service_option: u16,
    dialed_digits: &str,
) -> OriginationMessage {
    let dtmf_digits: Option<Vec<u8>> = dialed_digits
        .bytes()
        .map(|digit| match digit {
            b'1'..=b'9' => Some(digit - b'0'),
            b'0' => Some(0x0a),
            b'*' => Some(0x0b),
            b'#' => Some(0x0c),
            _ => None,
        })
        .collect();
    let digit_mode = dtmf_digits.is_none();
    let digits = dtmf_digits.unwrap_or_else(|| dialed_digits.as_bytes().to_vec());
    OriginationMessage {
        header: AccessMessageHeader {
            pd: PD_LEGACY,
            message_id: MessageId::Origination,
        },
        mob_term: id.mob_term,
        slot_cycle_index: id.slot_cycle_index & 0x07,
        mob_p_rev: 6,
        scm: id.scm,
        request_mode: 0,
        special_service: true,
        service_option: Some(service_option),
        pm: false,
        digit_mode,
        number_type: digit_mode.then_some(0),
        number_plan: digit_mode.then_some(0),
        more_fields: false,
        num_fields: digits.len() as u8,
        digits,
        nar_an_cap: false,
        paca_reorig: false,
        return_cause: 0,
        more_records: false,
        encryption_supported: None,
        paca_supported: false,
        num_alt_so: 0,
        alt_service_options: Vec::new(),
        drs: Some(true),
        uzid_incl: Some(false),
        uzid: None,
        ch_ind: Some(0b01),
        sr_id: Some(1),
        otd_supported: Some(false),
        qpch_supported: Some(false),
        enhanced_rc: Some(true),
        for_rc_pref: Some(3),
        rev_rc_pref: Some(3),
        fch_supported: Some(true),
        fch_capability: Some(fch_rc3_capability()),
        dcch_supported: Some(false),
        dcch_capability: None,
        geo_loc_incl: Some(false),
        geo_loc_type: None,
        rev_fch_gating_req: Some(false),
        orig_reason: None,
        orig_count: None,
        sts_supported: None,
        cch_3x_supported: None,
        wll_incl: None,
        wll_device_type: None,
        global_emergency_call: None,
        ms_init_pos_loc_ind: None,
        qos_parms_incl: None,
        qos_parms_len: None,
        qos_parms: Vec::new(),
        enc_info_incl: None,
        sig_encrypt_sup: None,
        d_sig_encrypt_req: None,
        c_sig_encrypt_req: None,
        new_sseq_h: None,
        new_sseq_h_sig: None,
        ui_encrypt_req: None,
        ui_encrypt_sup: None,
        sync_id_incl: None,
        sync_id_len: None,
        sync_id: None,
        prev_sid_incl: None,
        prev_sid: None,
        prev_nid_incl: None,
        prev_nid: None,
        prev_pzid_incl: None,
        prev_pzid: None,
        so_bitmap_ind: None,
        so_group_num: None,
        so_bitmap: None,
        sdb_desired_only: None,
        alt_band_class_sup: None,
        msg_int_info_incl: None,
        sig_integrity_sup_incl: None,
        sig_integrity_sup: None,
        sig_integrity_req: None,
        new_key_id: None,
        new_sseq_h_incl: None,
        for_pdch_supported: None,
        for_pdch_capability: None,
        ext_ch_ind: None,
        sign_slot_cycle_index: None,
        add_serv_instance_incl: None,
        add_service_instances: Vec::new(),
        bcmc_incl: None,
        bcmc: None,
        rev_pdch_supported: None,
        rev_pdch_capability: None,
        band_sub_rep_incl: None,
        num_band_subclass: None,
        band_subclass_sup: Vec::new(),
        add_geo_loc_incl: None,
        add_geo_loc_type_len_ind: None,
        add_geo_loc_type: None,
        remaining_bits: 0,
    }
}

pub fn encode_origination_pdu(
    id: &MsAccessIdentity,
    service_option: u16,
    dialed_digits: &str,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    let sdu = AccessMessage::Origination(origination_message(id, service_option, dialed_digits))
        .to_sdu()
        .expect("encode origination SDU");
    encode_r_csch_pdu(serving, MSG_TYPE_ORIGINATION, id, arq, &sdu)
}

pub fn build_origination_capsule(
    id: &MsAccessIdentity,
    service_option: u16,
    dialed_digits: &str,
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    sar_encapsulate(&encode_origination_pdu(
        id,
        service_option,
        dialed_digits,
        arq,
        serving,
    ))
}

#[cfg(test)]
mod tests;
