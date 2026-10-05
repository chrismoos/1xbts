//! Directed paging messages carry ARQ and addressing. Broadcast overhead does not (C.S0004-E §3.1.2.3.2, C.S0005-E §3.7.2.3.2).

use cdma_common::bits::Bitstream;
use cdma_common::lac::message_types::{MessageId, WireChannel};
use cdma_common::lac::paging_messages::{
    ChannelAssignmentMessage, ExtendedChannelAssignmentMessage,
};

use crate::forward_traffic_rx::FORWARD_WALSH_CODES;
use crate::ms::ChannelAssignment;

/// ADDR_TYPE values (C.S0004-E Table 3.1.2.2.1.3.1-1).
const ADDR_TYPE_IMSI_S: u8 = 0b000;
const ADDR_TYPE_ESN: u8 = 0b001;
const ADDR_TYPE_IMSI_CLASS_0: u8 = 0b010;
const ADDR_TYPE_IMSI_ESN: u8 = 0b011;
const ADDR_TYPE_TMSI: u8 = 0b101;

/// Order codes on the forward channels (C.S0005-E Table 3.7.4-1).
pub const ORDER_BS_ACK: u8 = 0b010000;
pub const ORDER_LOCK_UNTIL_POWER_CYCLED: u8 = 0b010010;
pub const ORDER_MAINTENANCE_REQUIRED: u8 = 0b010011;
pub const ORDER_UNLOCK: u8 = 0b010100;
pub const ORDER_RELEASE: u8 = 0b010101;
pub const ORDER_REGISTRATION: u8 = 0b011011;
pub const ORDER_SERVICE_OPTION_REQUEST: u8 = 0b000101;
pub const ORDER_SERVICE_OPTION_RESPONSE: u8 = 0b000110;
pub const ORDER_REORDER: u8 = 0b000011;
pub const ORDER_INTERCEPT: u8 = 0b000010;
pub const ORDER_AUDIT: u8 = 0b000111;
pub const ORDER_BASE_STATION_CHALLENGE_CONFIRMATION: u8 = 0b001000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsAddressing {
    pub esn: u32,
    pub imsi_s1: u32,
    pub imsi_s2: u16,
    pub mcc: u16,
    pub imsi_11_12: u8,
}

impl MsAddressing {
    /// Whether an IMSI class 0 address with the given explicit fields (an
    /// omitted field is implied by overhead and taken as matching) is ours.
    fn matches_class0(&self, mcc: Option<u16>, imsi_11_12: Option<u8>, s1: u32, s2: u16) -> bool {
        s1 == self.imsi_s1
            && s2 == self.imsi_s2
            && mcc.is_none_or(|m| m == self.mcc)
            && imsi_11_12.is_none_or(|d| d == self.imsi_11_12)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DirectedBody {
    Order {
        order: u8,
        ordq: u8,
        fields: Vec<u8>,
    },
    DataBurst {
        msg_number: u8,
        burst_type: u8,
        num_msgs: u8,
        fields: Vec<u8>,
    },
    StatusRequest {
        qual_info_type: u8,
        qual_info: Vec<u8>,
        record_types: Vec<u8>,
    },
    ChannelAssignment(ChannelAssignment),
    Unparsed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectedPdu {
    pub message_id: MessageId,
    pub msg_type: u8,
    pub ack_seq: u8,
    pub msg_seq: u8,
    pub ack_req: bool,
    pub valid_ack: bool,
    pub body: DirectedBody,
}

impl DirectedPdu {
    pub fn name(&self) -> &'static str {
        self.message_id.tag()
    }
}

pub fn is_directed(msg_type: u8) -> bool {
    matches!(
        MessageId::from_wire(WireChannel::ForwardCommon, msg_type),
        Some(
            MessageId::Order
                | MessageId::ChannelAssignment
                | MessageId::DataBurst
                | MessageId::AuthChallenge
                | MessageId::SsdUpdate
                | MessageId::FeatureNotification
                | MessageId::StatusRequest
                | MessageId::ServiceRedirection
                | MessageId::TmsiAssignment
                | MessageId::Paca
                | MessageId::ExtChannelAssignment
                | MessageId::UserZoneReject
                | MessageId::SecurityModeCommand
                | MessageId::AuthenticationRequest
                | MessageId::MeidExtChannelAssignment
        )
    )
}

/// STRQM body: the qualification the records are asked about, then the record
/// types themselves (C.S0005-E §3.7.2.3.2.15).
fn decode_status_request_body(bs: &mut Bitstream) -> Result<DirectedBody, String> {
    read(bs, 4, "RESERVED")?;
    let qual_info_type = read(bs, 8, "QUAL_INFO_TYPE")? as u8;
    let qual_info_len = read(bs, 3, "QUAL_INFO_LEN")? as usize;
    let mut qual_info = Vec::with_capacity(qual_info_len);
    for _ in 0..qual_info_len {
        qual_info.push(read(bs, 8, "QUAL_INFO")? as u8);
    }
    let num_fields = read(bs, 4, "NUM_FIELDS")? as usize;
    let mut record_types = Vec::with_capacity(num_fields);
    for _ in 0..num_fields {
        record_types.push(read(bs, 8, "RECORD_TYPE")? as u8);
    }
    Ok(DirectedBody::StatusRequest {
        qual_info_type,
        qual_info,
        record_types,
    })
}

fn read(bs: &mut Bitstream, bits: usize, what: &str) -> Result<u32, String> {
    bs.read_bits(bits)
        .map(|v| v as u32)
        .map_err(|e| format!("{what}: {e}"))
}

fn read_address(bs: &mut Bitstream, me: &MsAddressing) -> Result<bool, String> {
    let addr_type = read(bs, 3, "ADDR_TYPE")? as u8;
    let addr_len = read(bs, 4, "ADDR_LEN")? as usize;
    let addr_bits = addr_len * 8;
    let mut addr = Bitstream::new();
    let mut bits = Vec::with_capacity(addr_bits);
    for _ in 0..addr_bits {
        let bit = read(bs, 1, "ADDRESS")? as u8;
        bits.push(bit);
        addr.write_u8(bit, 1);
    }
    let mut a = addr;
    let matched = match addr_type {
        ADDR_TYPE_IMSI_S => {
            let s1 = read(&mut a, 24, "IMSI_S1")?;
            let s2 = read(&mut a, 10, "IMSI_S2")? as u16;
            s1 == me.imsi_s1 && s2 == me.imsi_s2
        }
        ADDR_TYPE_ESN => read(&mut a, 32, "ESN")? == me.esn,
        ADDR_TYPE_IMSI_CLASS_0 => read_imsi_class0(&mut a, me)?,
        ADDR_TYPE_IMSI_ESN => {
            let esn = read(&mut a, 32, "ESN")?;
            esn == me.esn && read_imsi_class0(&mut a, me)?
        }
        ADDR_TYPE_TMSI => false,
        _ => false,
    };
    if !matched {
        let mut value: u64 = 0;
        for bit in bits.iter().take(64) {
            value = (value << 1) | u64::from(*bit);
        }
        log::debug!(
            "ms_rx: directed address type={} len={} value=0x{:X} not this mobile",
            addr_type,
            addr_len,
            value
        );
    }
    Ok(matched)
}

/// IMSI_CLASS(1) then the class 0 record (C.S0004-E §3.1.2.2.1.3.3).
fn read_imsi_class0(a: &mut Bitstream, me: &MsAddressing) -> Result<bool, String> {
    let imsi_class = read(a, 1, "IMSI_CLASS")?;
    if imsi_class != 0 {
        return Ok(false);
    }
    let class0_type = read(a, 2, "IMSI_CLASS_0_TYPE")? as u8;
    let (mcc, imsi_11_12) = match class0_type {
        0b00 => {
            read(a, 3, "RESERVED")?;
            (None, None)
        }
        0b01 => {
            read(a, 4, "RESERVED")?;
            (None, Some(read(a, 7, "IMSI_11_12")? as u8))
        }
        0b10 => {
            read(a, 1, "RESERVED")?;
            (Some(read(a, 10, "MCC")? as u16), None)
        }
        _ => {
            read(a, 2, "RESERVED")?;
            let mcc = read(a, 10, "MCC")? as u16;
            let d = read(a, 7, "IMSI_11_12")? as u8;
            (Some(mcc), Some(d))
        }
    };
    let s2 = read(a, 10, "IMSI_S2")? as u16;
    let s1 = read(a, 24, "IMSI_S1")?;
    Ok(me.matches_class0(mcc, imsi_11_12, s1, s2))
}

/// Decode a directed PDU. `Ok(None)` means it addresses another mobile.
pub fn decode_directed(bits: &[u8], me: &MsAddressing) -> Result<Option<DirectedPdu>, String> {
    let mut bs = Bitstream::new_init(bits);
    let msg_type = read(&mut bs, 8, "MSG_TYPE")? as u8 & 0x3f;
    let message_id = MessageId::from_wire(WireChannel::ForwardCommon, msg_type)
        .ok_or_else(|| format!("unsupported f-csch MSG_TYPE 0x{msg_type:02X}"))?;
    let ack_seq = read(&mut bs, 3, "ACK_SEQ")? as u8;
    let msg_seq = read(&mut bs, 3, "MSG_SEQ")? as u8;
    let ack_req = read(&mut bs, 1, "ACK_REQ")? != 0;
    let valid_ack = read(&mut bs, 1, "VALID_ACK")? != 0;
    if !read_address(&mut bs, me)? {
        return Ok(None);
    }
    let body = match message_id {
        MessageId::Order => decode_order_body(&mut bs)?,
        MessageId::DataBurst => decode_data_burst_body(&mut bs)?,
        MessageId::StatusRequest => decode_status_request_body(&mut bs)?,
        MessageId::ChannelAssignment => {
            let cam = ChannelAssignmentMessage::from_sdu(&mut bs)?;
            // ASSIGN_MODE 000 and 100 are CDMA traffic assignments on the
            // current or a named frequency. Only RC1/RC2 are expressible.
            match receivable_walsh_code(u16::from(cam.code_chan), message_id) {
                Some(walsh_code) => DirectedBody::ChannelAssignment(ChannelAssignment {
                    walsh_code,
                    frame_offset: cam.frame_offset,
                    for_rc: 1,
                    rev_rc: 1,
                    pilot_pn: 0,
                }),
                None => DirectedBody::Unparsed,
            }
        }
        MessageId::ExtChannelAssignment | MessageId::MeidExtChannelAssignment => {
            // ECAM records prefix the SDU with RESERVED_1(1) and
            // ADD_RECORD_LEN(8) (C.S0004-E §3.1.2.3.2.2).
            read(&mut bs, 1, "RESERVED_1")?;
            read(&mut bs, 8, "ADD_RECORD_LEN")?;
            let ecam =
                ExtendedChannelAssignmentMessage::from_sdu(&mut bs).map_err(|e| e.to_string())?;
            log::info!("ms_rx: ECAM {}", ecam.describe());
            let pilot = ecam
                .pilots
                .first()
                .ok_or_else(|| "ECAM names no pilot".to_string())?;
            match receivable_walsh_code(pilot.code_chan_fch, message_id) {
                Some(walsh_code) => DirectedBody::ChannelAssignment(ChannelAssignment {
                    walsh_code,
                    frame_offset: ecam.frame_offset,
                    for_rc: ecam.for_rc,
                    rev_rc: ecam.rev_rc,
                    pilot_pn: pilot.pilot_pn,
                }),
                None => DirectedBody::Unparsed,
            }
        }
        _ => DirectedBody::Unparsed,
    };
    Ok(Some(DirectedPdu {
        message_id,
        msg_type,
        ack_seq,
        msg_seq,
        ack_req,
        valid_ack,
        body,
    }))
}

/// CODE_CHAN is 8 bits in a CAM and CODE_CHAN_FCH 11 bits in an ECAM
/// (C.S0005-E §3.7.2.3.2.8, §3.7.2.3.2.21).
fn receivable_walsh_code(code_chan: u16, message_id: MessageId) -> Option<u8> {
    let walsh_code = u8::try_from(code_chan)
        .ok()
        .filter(|&code| usize::from(code) < FORWARD_WALSH_CODES);
    if walsh_code.is_none() {
        log::warn!(
            "ms_rx: ignoring {:?} with code_chan={} outside the {}-ary forward Walsh set",
            message_id,
            code_chan,
            FORWARD_WALSH_CODES
        );
    }
    walsh_code
}

/// ORDER(6) ADD_RECORD_LEN(3) then ADD_RECORD_LEN octets, the first of which
/// is ORDQ when present (C.S0005-E §3.7.2.3.2.3).
fn decode_order_body(bs: &mut Bitstream) -> Result<DirectedBody, String> {
    let order = read(bs, 6, "ORDER")? as u8;
    let add_record_len = read(bs, 3, "ADD_RECORD_LEN")? as usize;
    let mut octets = Vec::with_capacity(add_record_len);
    for _ in 0..add_record_len {
        octets.push(read(bs, 8, "ORDER_FIELD")? as u8);
    }
    let ordq = octets.first().copied().unwrap_or(0);
    let fields = octets.get(1..).map(|f| f.to_vec()).unwrap_or_default();
    Ok(DirectedBody::Order {
        order,
        ordq,
        fields,
    })
}

/// MSG_NUMBER(8) BURST_TYPE(6) NUM_MSGS(8) NUM_FIELDS(8) CHARi×NUM_FIELDS
/// (C.S0005-E §3.7.2.3.2.9).
fn decode_data_burst_body(bs: &mut Bitstream) -> Result<DirectedBody, String> {
    let msg_number = read(bs, 8, "MSG_NUMBER")? as u8;
    let burst_type = read(bs, 6, "BURST_TYPE")? as u8;
    let num_msgs = read(bs, 8, "NUM_MSGS")? as u8;
    let num_fields = read(bs, 8, "NUM_FIELDS")? as usize;
    let mut fields = Vec::with_capacity(num_fields);
    for _ in 0..num_fields {
        fields.push(read(bs, 8, "CHARi")? as u8);
    }
    Ok(DirectedBody::DataBurst {
        msg_number,
        burst_type,
        num_msgs,
        fields,
    })
}

/// The name of a forward order code (C.S0005-E Table 3.7.4-1).
pub fn order_name(order: u8) -> &'static str {
    match order {
        0b000001 => "Abbreviated Alert",
        ORDER_INTERCEPT => "Intercept",
        ORDER_REORDER => "Reorder",
        ORDER_SERVICE_OPTION_REQUEST => "Service Option Request",
        ORDER_SERVICE_OPTION_RESPONSE => "Service Option Response",
        ORDER_AUDIT => "Audit",
        ORDER_BASE_STATION_CHALLENGE_CONFIRMATION => "Base Station Challenge Confirmation",
        ORDER_BS_ACK => "Base Station Acknowledgment",
        0b010001 => "Pilot Measurement Request",
        ORDER_LOCK_UNTIL_POWER_CYCLED => "Lock Until Power-Cycled",
        ORDER_MAINTENANCE_REQUIRED => "Maintenance Required",
        ORDER_UNLOCK => "Unlock",
        ORDER_RELEASE => "Release",
        0b010110 => "Outer Loop Report Request",
        0b010111 => "Long Code Transition",
        0b011001 => "Continuous DTMF Tone",
        0b011010 => "Status Request",
        ORDER_REGISTRATION => "Registration",
        0b011110 => "Local Control",
        0b100001 => "Connect",
        _ => "Unknown Order",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> MsAddressing {
        MsAddressing {
            esn: 0x1234_5678,
            imsi_s1: 0x00AB_CDEF,
            imsi_s2: 0x123,
            mcc: 310,
            imsi_11_12: 15,
        }
    }

    fn header(msg_type: u8, ack_seq: u8, msg_seq: u8, ack_req: bool, valid_ack: bool) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u8(msg_type, 8);
        bs.write_u8(ack_seq, 3);
        bs.write_u8(msg_seq, 3);
        bs.write_u8(ack_req as u8, 1);
        bs.write_u8(valid_ack as u8, 1);
        bs
    }

    fn esn_address(bs: &mut Bitstream, esn: u32) {
        bs.write_u8(ADDR_TYPE_ESN, 3);
        bs.write_u8(4, 4);
        bs.write_u32(esn, 32);
    }

    #[test]
    fn registration_accepted_order_for_this_mobile() {
        let mut bs = header(0x07, 5, 2, false, true);
        esn_address(&mut bs, 0x1234_5678);
        bs.write_u8(ORDER_REGISTRATION, 6);
        bs.write_u8(1, 3);
        bs.write_u8(0x00, 8);
        let pdu = decode_directed(bs.bits(), &me())
            .expect("decodes")
            .expect("addressed to us");
        assert_eq!(pdu.message_id, MessageId::Order);
        assert_eq!(pdu.ack_seq, 5);
        assert_eq!(pdu.msg_seq, 2);
        assert!(pdu.valid_ack);
        assert!(!pdu.ack_req);
        assert_eq!(
            pdu.body,
            DirectedBody::Order {
                order: ORDER_REGISTRATION,
                ordq: 0,
                fields: vec![]
            }
        );
    }

    #[test]
    fn order_for_another_esn_is_ignored() {
        let mut bs = header(0x07, 0, 0, true, false);
        esn_address(&mut bs, 0x0000_0001);
        bs.write_u8(ORDER_BS_ACK, 6);
        bs.write_u8(0, 3);
        assert_eq!(decode_directed(bs.bits(), &me()).expect("decodes"), None);
    }

    #[test]
    fn data_burst_by_imsi_class0_with_implied_prefix() {
        let mut bs = header(0x09, 0, 3, true, false);
        // IMSI class 0 type 00: RESERVED(3) IMSI_S2(10) IMSI_S1(24) = 40 bits.
        bs.write_u8(ADDR_TYPE_IMSI_CLASS_0, 3);
        bs.write_u8(5, 4);
        bs.write_u8(0, 1);
        bs.write_u8(0b00, 2);
        bs.write_u8(0, 3);
        bs.write_u32(0x123, 10);
        bs.write_u32(0x00AB_CDEF, 24);
        bs.write_u8(1, 8);
        bs.write_u8(3, 6);
        bs.write_u8(1, 8);
        bs.write_u8(2, 8);
        bs.write_u8(0xAA, 8);
        bs.write_u8(0x55, 8);
        let pdu = decode_directed(bs.bits(), &me())
            .expect("decodes")
            .expect("addressed to us");
        assert!(pdu.ack_req);
        assert_eq!(
            pdu.body,
            DirectedBody::DataBurst {
                msg_number: 1,
                burst_type: 3,
                num_msgs: 1,
                fields: vec![0xAA, 0x55]
            }
        );
    }

    #[test]
    fn imsi_class0_with_wrong_mcc_is_not_ours() {
        let mut bs = header(0x07, 0, 0, false, false);
        bs.write_u8(ADDR_TYPE_IMSI_CLASS_0, 3);
        bs.write_u8(6, 4);
        bs.write_u8(0, 1);
        bs.write_u8(0b10, 2);
        bs.write_u8(0, 1);
        bs.write_u32(311, 10);
        bs.write_u32(0x123, 10);
        bs.write_u32(0x00AB_CDEF, 24);
        bs.write_u8(ORDER_BS_ACK, 6);
        bs.write_u8(0, 3);
        assert_eq!(decode_directed(bs.bits(), &me()).expect("decodes"), None);
    }

    /// CODE_CHAN follows ASSIGN_MODE(3) ADD_RECORD_LEN(3) FREQ_INCL(1) in an
    /// ASSIGN_MODE 000 record.
    const CAM_CODE_CHAN_OFFSET: usize = 7;
    const CAM_CODE_CHAN_BITS: usize = 8;

    fn cam_pdu(code_chan: u8) -> Bitstream {
        let mut sdu = ChannelAssignmentMessage::new_traffic_assignment(1, 0)
            .to_sdu()
            .bits()
            .to_vec();
        for (i, bit) in sdu[CAM_CODE_CHAN_OFFSET..CAM_CODE_CHAN_OFFSET + CAM_CODE_CHAN_BITS]
            .iter_mut()
            .enumerate()
        {
            *bit = (code_chan >> (CAM_CODE_CHAN_BITS - 1 - i)) & 1;
        }
        let msg_type = MessageId::ChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let mut bs = header(msg_type, 0, 1, true, false);
        esn_address(&mut bs, 0x1234_5678);
        bs.extend(&Bitstream::new_init(&sdu));
        bs
    }

    fn ecam_pdu(code_chan_fch: u16) -> Bitstream {
        let mut ecam =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(0, 1, 0, 3, 3, false);
        ecam.pilots[0].code_chan_fch = code_chan_fch;
        let sdu = ecam.to_sdu();
        let msg_type = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let mut bs = header(msg_type, 0, 1, true, false);
        esn_address(&mut bs, 0x1234_5678);
        bs.write_u8(0, 1);
        bs.write_u8(sdu.len().div_ceil(8) as u8, 8);
        bs.extend(&sdu);
        bs
    }

    fn assigned_walsh(bs: &Bitstream) -> Option<u8> {
        let pdu = decode_directed(bs.bits(), &me())
            .expect("decodes")
            .expect("addressed to us");
        match pdu.body {
            DirectedBody::ChannelAssignment(assignment) => Some(assignment.walsh_code),
            DirectedBody::Unparsed => None,
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn cam_with_code_chan_outside_the_walsh_set_is_ignored() {
        assert_eq!(assigned_walsh(&cam_pdu(63)), Some(63));
        assert_eq!(assigned_walsh(&cam_pdu(64)), None);
        assert_eq!(assigned_walsh(&cam_pdu(200)), None);
    }

    #[test]
    fn ecam_code_chan_fch_keeps_all_eleven_bits() {
        assert_eq!(assigned_walsh(&ecam_pdu(12)), Some(12));
        // 268 truncated to eight bits is the valid-looking W12.
        assert_eq!(assigned_walsh(&ecam_pdu(268)), None);
        assert_eq!(assigned_walsh(&ecam_pdu(64)), None);
    }
}
