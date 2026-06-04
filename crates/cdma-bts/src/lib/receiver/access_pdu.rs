use cdma_common::bits::Bitstream;

use crate::lac::message_types::{MessageId, WireChannel};
use crate::receiver::access_layer3::{
    AccessMessage, AccessMessageHeader, access_message_type_name,
};

#[derive(Debug, Clone)]
pub struct ReverseAccessPduHeader {
    pub pd: u8,
    pub msg_type: u8,
}

#[derive(Debug, Clone)]
pub struct RcschArqFields {
    pub raw: Bitstream,
    pub ack_seq: u8,
    pub msg_seq: u8,
    pub ack_req: bool,
    pub valid_ack: bool,
    pub ack_type: u8,
    pub ext_ack_type: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct RcschAddressingFields {
    pub raw: Bitstream,
    pub msid_type: u8,
    pub ext_msid_type: Option<u8>,
    pub msid_len_octets: u8,
    pub actual_msid_octets: usize,
    pub msid_raw: Bitstream,
}

#[derive(Debug, Clone)]
pub struct RcschAuthenticationFields {
    pub raw: Bitstream,
    pub maci_incl: bool,
    pub auth_incl: bool,
    pub authr: Option<u32>,
    pub randc: Option<u8>,
    pub count: Option<u8>,
    pub sdu_key_id: Option<u8>,
    pub sdu_integrity_algo: Option<u8>,
    pub sdu_sseq_or_sseqh: Option<bool>,
    pub sdu_sseq: Option<u8>,
    pub sdu_sseq_h: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RcschPRev6PilotRecord {
    pub pilot_pn_phase: u16,
    pub pilot_strength: u8,
    pub access_ho_en: bool,
    pub access_attempted: bool,
}

#[derive(Debug, Clone)]
pub struct RcschPRev6RadioEnvironmentReport {
    pub raw: Bitstream,
    pub active_pilot_strength: u8,
    pub first_is_active: bool,
    pub first_is_pta: bool,
    pub num_add_pilots: u8,
    pub additional_pilots: Vec<RcschPRev6PilotRecord>,
}

#[derive(Debug, Clone)]
pub struct RcschPRev6Pdu {
    pub header: ReverseAccessPduHeader,
    pub raw_pdu: Bitstream,
    pub lac_length_octets: u8,
    pub lac_region_raw: Bitstream,
    pub arq: Option<RcschArqFields>,
    pub addressing: Option<RcschAddressingFields>,
    pub authentication: Option<RcschAuthenticationFields>,
    pub lac_padding_raw: Bitstream,
    pub radio_environment_report: Option<RcschPRev6RadioEnvironmentReport>,
    pub sdu_plus_padding_raw: Bitstream,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RcschPd00Pdu {
    pub header: ReverseAccessPduHeader,
    pub raw_pdu: Bitstream,
    pub arq: Option<RcschArqFields>,
    pub addressing: Option<RcschAddressingFields>,
    pub authentication: Option<RcschAuthenticationFields>,
    pub sdu_plus_padding_raw: Bitstream,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ReverseAccessPdu {
    Pd00Legacy(RcschPd00Pdu),
    Pd01PRev6(RcschPRev6Pdu),
    Pd10Modern {
        header: ReverseAccessPduHeader,
        raw_pdu: Bitstream,
        raw_after_header: Bitstream,
    },
}

impl ReverseAccessPdu {
    pub fn decode(bits: &Bitstream) -> Result<Self, String> {
        if bits.len() < 8 {
            return Err(format!("PDU too short: {} bits", bits.len()));
        }

        let raw_pdu = bits.clone();
        let mut bs = bits.clone();
        let header_raw = bs.drain(0..8);
        let mut header_bits = header_raw.clone();
        let pd_and_type = header_bits.read_bits(8).map_err(|e| e.to_string())? as u8;
        let header = ReverseAccessPduHeader {
            pd: pd_and_type >> 6,
            msg_type: pd_and_type & 0x3f,
        };

        Ok(match header.pd {
            0b00 => ReverseAccessPdu::Pd00Legacy(decode_pd00_legacy(header, raw_pdu, bs)?),
            0b01 => ReverseAccessPdu::Pd01PRev6(decode_pd01_p_rev6(header, raw_pdu, bs)?),
            0b10 => {
                return Err(
                    "unsupported r-csch PD 0b10: P_REV_IN_USE >= 9 wrapper is not decoded"
                        .to_string(),
                );
            }
            _ => return Err(format!("unsupported r-csch PD 0b{:02b}", header.pd)),
        })
    }

    pub fn summary(&self) -> String {
        match self {
            ReverseAccessPdu::Pd00Legacy(pdu) => {
                let arq = pdu.arq.as_ref().map_or("arq=unparsed".to_string(), |arq| {
                    format!(
                        "arq=ack_seq:{} msg_seq:{} ack_req:{} valid_ack:{} ack_type=0b{:03b}{}",
                        arq.ack_seq,
                        arq.msg_seq,
                        arq.ack_req as u8,
                        arq.valid_ack as u8,
                        arq.ack_type,
                        arq.ext_ack_type
                            .map_or(String::new(), |v| format!(" ext_ack_type=0b{:03b}", v)),
                    )
                });
                let addressing = pdu
                    .addressing
                    .as_ref()
                    .map_or("address=unparsed".to_string(), |addr| {
                        format!("address={}", addr.summary())
                    });
                let authentication = pdu
                    .authentication
                    .as_ref()
                    .map_or("auth=unparsed".to_string(), |auth| {
                        format!("auth={}", auth.summary())
                    });
                let warnings = if pdu.warnings.is_empty() {
                    String::new()
                } else {
                    format!(" warnings={}", pdu.warnings.join("|"))
                };
                let sdu = summarize_rcsch_sdu(&pdu.header, &pdu.sdu_plus_padding_raw);
                format!(
                    "RcschPdu(pd={}, msg_type=0b{:06b} {}, wrapper=legacy_pd00, {}, {}, {}, sdu_with_padding_bits={}{}{})",
                    pdu.header.pd,
                    pdu.header.msg_type,
                    access_message_type_name(pdu.header.msg_type),
                    arq,
                    addressing,
                    authentication,
                    pdu.sdu_plus_padding_raw.len(),
                    sdu,
                    warnings,
                )
            }
            ReverseAccessPdu::Pd01PRev6(pdu) => {
                let arq = pdu.arq.as_ref().map_or("arq=unparsed".to_string(), |arq| {
                    format!(
                        "arq=ack_seq:{} msg_seq:{} ack_req:{} valid_ack:{} ack_type=0b{:03b}{}",
                        arq.ack_seq,
                        arq.msg_seq,
                        arq.ack_req as u8,
                        arq.valid_ack as u8,
                        arq.ack_type,
                        arq.ext_ack_type
                            .map_or(String::new(), |v| { format!(" ext_ack_type=0b{:03b}", v) }),
                    )
                });
                let addressing = pdu
                    .addressing
                    .as_ref()
                    .map_or("address=unparsed".to_string(), |addr| {
                        format!("address={}", addr.summary())
                    });
                let authentication = pdu
                    .authentication
                    .as_ref()
                    .map_or("auth=unparsed".to_string(), |auth| {
                        format!("auth={}", auth.summary())
                    });
                let lac_padding = format!(
                    "lac_padding_bits={} lac_padding_nonzero={}",
                    pdu.lac_padding_raw.len(),
                    pdu.lac_padding_raw.bits().iter().any(|b| *b != 0) as u8,
                );
                let rer = pdu.radio_environment_report.as_ref().map_or(
                    "rer=unparsed".to_string(),
                    |rer| {
                        format!(
                            "rer=active:{} first_active:{} first_pta:{} add_pilots:{}",
                            rer.active_pilot_strength,
                            rer.first_is_active as u8,
                            rer.first_is_pta as u8,
                            rer.num_add_pilots,
                        )
                    },
                );
                let warnings = if pdu.warnings.is_empty() {
                    String::new()
                } else {
                    format!(" warnings={}", pdu.warnings.join("|"))
                };
                let sdu = summarize_rcsch_sdu(&pdu.header, &pdu.sdu_plus_padding_raw);
                format!(
                    "RcschPdu(pd={}, msg_type=0b{:06b} {}, wrapper=p_rev_6, lac_length_octets={}, {}, {}, {}, {}, {}, sdu_with_padding_bits={}{}{})",
                    pdu.header.pd,
                    pdu.header.msg_type,
                    access_message_type_name(pdu.header.msg_type),
                    pdu.lac_length_octets,
                    arq,
                    addressing,
                    authentication,
                    lac_padding,
                    rer,
                    pdu.sdu_plus_padding_raw.len(),
                    sdu,
                    warnings,
                )
            }
            ReverseAccessPdu::Pd10Modern {
                header,
                raw_after_header,
                ..
            } => format!(
                "RcschPdu(pd={}, msg_type=0b{:06b} {}, wrapper=modern_pd10, body_bits={})",
                header.pd,
                header.msg_type,
                access_message_type_name(header.msg_type),
                raw_after_header.len(),
            ),
        }
    }
}

fn summarize_rcsch_sdu(header: &ReverseAccessPduHeader, sdu: &Bitstream) -> String {
    let Some(message_id) = MessageId::from_wire(WireChannel::ReverseCommon, header.msg_type) else {
        return format!(
            " sdu_decode_error=unsupported r-csch MSG_TAG 0x{:02X}",
            header.msg_type
        );
    };
    let header = AccessMessageHeader {
        pd: header.pd,
        message_id,
    };
    match AccessMessage::decode_sdu(header, sdu) {
        Ok(msg) => format!(" sdu={}", msg.summary()),
        Err(err) => format!(" sdu_decode_error={err}"),
    }
}

/// Decode PD=00 (P_REV_IN_USE < 6) LAC PDU per C.S0004-E Table 2.1.1.4.2-1/2:
/// Message Type | ARQ | Addressing | Auth | SDU | PDU Padding
fn decode_pd00_legacy(
    header: ReverseAccessPduHeader,
    raw_pdu: Bitstream,
    mut bs: Bitstream,
) -> Result<RcschPd00Pdu, String> {
    let mut warnings = Vec::new();
    let arq = parse_rcsch_arq_fields(&mut bs, &mut warnings);
    let addressing = parse_rcsch_addressing_fields(&mut bs, &mut warnings);
    let authentication = parse_rcsch_authentication_fields(&mut bs, &mut warnings);
    let sdu_plus_padding_raw = bs;

    Ok(RcschPd00Pdu {
        header,
        raw_pdu,
        arq,
        addressing,
        authentication,
        sdu_plus_padding_raw,
        warnings,
    })
}

fn decode_pd01_p_rev6(
    header: ReverseAccessPduHeader,
    raw_pdu: Bitstream,
    mut bs: Bitstream,
) -> Result<RcschPRev6Pdu, String> {
    let mut warnings = Vec::new();
    if bs.len() < 5 {
        return Err("EOF reading LAC_LENGTH (5 bits)".to_string());
    }

    let mut lac_length_raw = bs.drain(0..5);
    let lac_length_octets = lac_length_raw.read_bits(5).map_err(|e| e.to_string())? as u8;
    let lac_total_bits = lac_length_octets as usize * 8;
    if lac_total_bits < 5 {
        return Err(format!(
            "Invalid LAC_LENGTH={} octets for PD=01",
            lac_length_octets
        ));
    }

    let lac_region_bits = lac_total_bits - 5;
    if bs.len() < lac_region_bits {
        return Err(format!(
            "PDU truncated: need {} LAC bits after LAC_LENGTH, have {}",
            lac_region_bits,
            bs.len()
        ));
    }

    let lac_region_raw = bs.drain(0..lac_region_bits);
    let mut lac_region_parse = lac_region_raw.clone();
    let arq = parse_rcsch_arq_fields(&mut lac_region_parse, &mut warnings);
    let addressing = parse_rcsch_addressing_fields(&mut lac_region_parse, &mut warnings);
    let authentication = parse_rcsch_authentication_fields(&mut lac_region_parse, &mut warnings);
    let lac_padding_raw = lac_region_parse.clone();
    let radio_environment_report = parse_p_rev6_radio_environment_report(&mut bs, &mut warnings);
    let sdu_plus_padding_raw = bs;

    Ok(RcschPRev6Pdu {
        header,
        raw_pdu,
        lac_length_octets,
        lac_region_raw,
        arq,
        addressing,
        authentication,
        lac_padding_raw,
        radio_environment_report,
        sdu_plus_padding_raw,
        warnings,
    })
}

impl RcschAddressingFields {
    pub fn summary(&self) -> String {
        let ext = self.ext_msid_type.map_or(String::new(), |v| {
            format!(" ext_msid_type=0b{:03b}({})", v, ext_msid_type_name(v))
        });
        format!(
            "msid_type=0b{:03b}({}){} msid_len_octets={} actual_msid_octets={} msid={}",
            self.msid_type,
            msid_type_name(self.msid_type),
            ext,
            self.msid_len_octets,
            self.actual_msid_octets,
            format_msid(self),
        )
    }
}

impl RcschAuthenticationFields {
    pub fn summary(&self) -> String {
        format!(
            "maci_incl={} auth_incl={} authr={:?} randc={:?} count={:?} sdu_key_id={:?} sdu_integrity_algo={:?} sdu_sseq_or_sseqh={:?} sdu_sseq={:?} sdu_sseq_h={:?}",
            self.maci_incl as u8,
            self.auth_incl as u8,
            self.authr,
            self.randc,
            self.count,
            self.sdu_key_id,
            self.sdu_integrity_algo,
            self.sdu_sseq_or_sseqh.map(|v| v as u8),
            self.sdu_sseq,
            self.sdu_sseq_h,
        )
    }
}

fn parse_rcsch_addressing_fields(
    bs: &mut Bitstream,
    warnings: &mut Vec<String>,
) -> Option<RcschAddressingFields> {
    if bs.len() < 7 {
        warnings.push(format!("short_address_region={}bits", bs.len()));
        return None;
    }

    let mut raw = Bitstream::new();
    let prefix_raw = bs.drain(0..3);
    raw.extend(&prefix_raw);
    let mut prefix_bits = prefix_raw.clone();
    let msid_type = prefix_bits.read_bits(3).ok()? as u8;

    let ext_msid_type = if msid_type == 0b100 {
        if bs.len() < 3 {
            warnings.push("missing_ext_msid_type".to_string());
            return None;
        }
        let ext_raw = bs.drain(0..3);
        raw.extend(&ext_raw);
        let mut ext_bits = ext_raw.clone();
        Some(ext_bits.read_bits(3).ok()? as u8)
    } else {
        None
    };

    if bs.len() < 4 {
        warnings.push("missing_msid_len".to_string());
        return None;
    }
    let len_raw = bs.drain(0..4);
    raw.extend(&len_raw);
    let mut len_bits = len_raw.clone();
    let msid_len_octets = len_bits.read_bits(4).ok()? as u8;
    let actual_msid_octets = actual_msid_octets(msid_type, ext_msid_type, msid_len_octets);
    let msid_bits = actual_msid_octets.saturating_mul(8);
    if bs.len() < msid_bits {
        warnings.push(format!(
            "short_msid expected_bits={} remaining_bits={}",
            msid_bits,
            bs.len()
        ));
        return None;
    }
    let msid_raw = bs.drain(0..msid_bits);
    raw.extend(&msid_raw);

    Some(RcschAddressingFields {
        raw,
        msid_type,
        ext_msid_type,
        msid_len_octets,
        actual_msid_octets,
        msid_raw,
    })
}

fn parse_rcsch_authentication_fields(
    bs: &mut Bitstream,
    warnings: &mut Vec<String>,
) -> Option<RcschAuthenticationFields> {
    if bs.len() < 2 {
        warnings.push(format!("short_auth_region={}bits", bs.len()));
        return None;
    }

    let mut raw = Bitstream::new();
    let flags_raw = bs.drain(0..2);
    raw.extend(&flags_raw);
    let mut flags_bits = flags_raw.clone();
    let maci_incl = flags_bits.read_bits(1).ok()? == 1;
    let auth_incl = flags_bits.read_bits(1).ok()? == 1;

    let authr = if auth_incl {
        if bs.len() < 18 {
            warnings.push("missing_authr".to_string());
            return None;
        }
        let field = bs.drain(0..18);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(18).ok()? as u32)
    } else {
        None
    };

    let randc = if auth_incl || maci_incl {
        if bs.len() < 8 {
            warnings.push("missing_randc".to_string());
            return None;
        }
        let field = bs.drain(0..8);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(8).ok()? as u8)
    } else {
        None
    };

    let count = if auth_incl {
        if bs.len() < 6 {
            warnings.push("missing_count".to_string());
            return None;
        }
        let field = bs.drain(0..6);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(6).ok()? as u8)
    } else {
        None
    };

    let sdu_key_id = if maci_incl {
        if bs.len() < 2 {
            warnings.push("missing_sdu_key_id".to_string());
            return None;
        }
        let field = bs.drain(0..2);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(2).ok()? as u8)
    } else {
        None
    };

    let sdu_integrity_algo = if maci_incl {
        if bs.len() < 3 {
            warnings.push("missing_sdu_integrity_algo".to_string());
            return None;
        }
        let field = bs.drain(0..3);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(3).ok()? as u8)
    } else {
        None
    };

    let sdu_sseq_or_sseqh = if maci_incl {
        if bs.len() < 1 {
            warnings.push("missing_sdu_sseq_or_sseqh".to_string());
            return None;
        }
        let field = bs.drain(0..1);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(1).ok()? == 1)
    } else {
        None
    };

    let sdu_sseq = if maci_incl && sdu_sseq_or_sseqh == Some(false) {
        if bs.len() < 8 {
            warnings.push("missing_sdu_sseq".to_string());
            return None;
        }
        let field = bs.drain(0..8);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(8).ok()? as u8)
    } else {
        None
    };

    let sdu_sseq_h = if maci_incl && sdu_sseq_or_sseqh == Some(true) {
        if bs.len() < 24 {
            warnings.push("missing_sdu_sseq_h".to_string());
            return None;
        }
        let field = bs.drain(0..24);
        raw.extend(&field);
        let mut bits = field.clone();
        Some(bits.read_bits(24).ok()? as u32)
    } else {
        None
    };

    Some(RcschAuthenticationFields {
        raw,
        maci_incl,
        auth_incl,
        authr,
        randc,
        count,
        sdu_key_id,
        sdu_integrity_algo,
        sdu_sseq_or_sseqh,
        sdu_sseq,
        sdu_sseq_h,
    })
}

fn actual_msid_octets(msid_type: u8, ext_msid_type: Option<u8>, msid_len_octets: u8) -> usize {
    if msid_type == 0b100 && ext_msid_type == Some(0b010) {
        (msid_len_octets as usize) + 16
    } else {
        msid_len_octets as usize
    }
}

fn msid_type_name(msid_type: u8) -> &'static str {
    match msid_type {
        0b000 => "imsi_s+esn",
        0b001 => "esn",
        0b010 => "imsi",
        0b011 => "imsi+esn",
        0b100 => "extended_msid",
        0b101 => "tmsi",
        0b110 => "reserved_mc_map",
        0b111 => "reserved_mc_map",
        _ => "unknown",
    }
}

fn ext_msid_type_name(ext_msid_type: u8) -> &'static str {
    match ext_msid_type {
        0b000 => "meid",
        0b001 => "imsi+meid",
        0b010 => "imsi+esn+meid",
        _ => "reserved",
    }
}

fn format_msid(addr: &RcschAddressingFields) -> String {
    match (addr.msid_type, addr.ext_msid_type) {
        (0b001, _) if addr.msid_raw.len() >= 32 => {
            let mut bits = addr.msid_raw.clone();
            match bits.read_bits(32) {
                Ok(esn) => format!("esn=0x{esn:08x}"),
                Err(_) => format!("raw={}", bits_to_hex(addr.msid_raw.bits())),
            }
        }
        (0b000, _) if addr.msid_raw.len() >= 66 => {
            let mut bits = addr.msid_raw.clone();
            let imsi_s1 = bits.read_bits(24).ok();
            let imsi_s2 = bits.read_bits(10).ok();
            let esn = bits.read_bits(32).ok();
            match (imsi_s1, imsi_s2, esn) {
                (Some(imsi_s1), Some(imsi_s2), Some(esn)) => {
                    format!("imsi_s1=0x{imsi_s1:06x} imsi_s2=0x{imsi_s2:03x} esn=0x{esn:08x}")
                }
                _ => format!("raw={}", bits_to_hex(addr.msid_raw.bits())),
            }
        }
        (0b010, _) if addr.msid_raw.len() >= 1 => {
            let mut bits = addr.msid_raw.clone();
            let imsi_class = bits.read_bits(1).ok();
            match imsi_class {
                Some(imsi_class) => {
                    let class_summary = format_imsi_class_specific(imsi_class as u8, &mut bits);
                    format!("imsi_class={} {}", imsi_class, class_summary)
                }
                None => format!("raw={}", bits_to_hex(addr.msid_raw.bits())),
            }
        }
        (0b011, _) if addr.msid_raw.len() >= 33 => {
            let mut bits = addr.msid_raw.clone();
            let esn = bits.read_bits(32).ok();
            let imsi_class = bits.read_bits(1).ok();
            match (esn, imsi_class) {
                (Some(esn), Some(imsi_class)) => {
                    let class_summary = format_imsi_class_specific(imsi_class as u8, &mut bits);
                    format!(
                        "esn=0x{esn:08x} imsi_class={} {}",
                        imsi_class, class_summary
                    )
                }
                _ => format!("raw={}", bits_to_hex(addr.msid_raw.bits())),
            }
        }
        (0b100, Some(0b000)) if addr.msid_raw.len() >= 56 => {
            format!("meid={}", bits_to_hex(addr.msid_raw.bits()))
        }
        (0b100, Some(0b001 | 0b010)) => {
            let mut bits = addr.msid_raw.clone();
            let maybe_esn = if addr.ext_msid_type == Some(0b010) {
                bits.read_bits(32).ok()
            } else {
                None
            };
            let meid = bits.read_bits(56).ok();
            let imsi_class = bits.read_bits(1).ok();
            match (maybe_esn, meid, imsi_class) {
                (Some(esn), Some(meid), Some(imsi_class)) => {
                    let class_summary = format_imsi_class_specific(imsi_class as u8, &mut bits);
                    format!(
                        "esn=0x{esn:08x} meid={meid:014x} imsi_class={} {}",
                        imsi_class, class_summary
                    )
                }
                (None, Some(meid), Some(imsi_class)) => {
                    let class_summary = format_imsi_class_specific(imsi_class as u8, &mut bits);
                    format!(
                        "meid={meid:014x} imsi_class={} {}",
                        imsi_class, class_summary
                    )
                }
                _ => format!("raw={}", bits_to_hex(addr.msid_raw.bits())),
            }
        }
        (0b101, _) => format!("tmsi={}", bits_to_hex(addr.msid_raw.bits())),
        _ => format!("raw={}", bits_to_hex(addr.msid_raw.bits())),
    }
}

fn format_imsi_class_specific(imsi_class: u8, bits: &mut Bitstream) -> String {
    match imsi_class {
        0 => format_imsi_class_0(bits),
        1 => format_imsi_class_1(bits),
        _ => format!("class_specific_raw={}", bits_to_hex(bits.bits())),
    }
}

fn format_imsi_class_0(bits: &mut Bitstream) -> String {
    let mut clone = bits.clone();
    let Some(class_0_type) = clone.read_bits(2).ok().map(|v| v as u8) else {
        return format!("class0_raw={}", bits_to_hex(bits.bits()));
    };
    let summary = match class_0_type {
        0b00 => {
            let reserved = clone.read_bits(3).ok();
            let imsi_s = clone.read_bits(34).ok();
            match (reserved, imsi_s) {
                (Some(reserved), Some(imsi_s)) => {
                    format!("class0_type=0 reserved=0x{reserved:x} imsi_s=0x{imsi_s:09x}")
                }
                _ => format!("class0_raw={}", bits_to_hex(bits.bits())),
            }
        }
        0b01 => {
            let reserved = clone.read_bits(4).ok();
            let imsi_11_12 = clone.read_bits(7).ok();
            let imsi_s = clone.read_bits(34).ok();
            match (reserved, imsi_11_12, imsi_s) {
                (Some(reserved), Some(imsi_11_12), Some(imsi_s)) => format!(
                    "class0_type=1 reserved=0x{reserved:x} imsi_11_12=0x{imsi_11_12:02x} imsi_s=0x{imsi_s:09x}"
                ),
                _ => format!("class0_raw={}", bits_to_hex(bits.bits())),
            }
        }
        0b10 => {
            let reserved = clone.read_bits(1).ok();
            let mcc = clone.read_bits(10).ok();
            let imsi_s = clone.read_bits(34).ok();
            match (reserved, mcc, imsi_s) {
                (Some(reserved), Some(mcc), Some(imsi_s)) => format!(
                    "class0_type=2 reserved=0x{reserved:x} mcc=0x{mcc:03x} imsi_s=0x{imsi_s:09x}"
                ),
                _ => format!("class0_raw={}", bits_to_hex(bits.bits())),
            }
        }
        0b11 => {
            let reserved = clone.read_bits(2).ok();
            let mcc = clone.read_bits(10).ok();
            let imsi_11_12 = clone.read_bits(7).ok();
            let imsi_s = clone.read_bits(34).ok();
            match (reserved, mcc, imsi_11_12, imsi_s) {
                (Some(reserved), Some(mcc), Some(imsi_11_12), Some(imsi_s)) => format!(
                    "class0_type=3 reserved=0x{reserved:x} mcc=0x{mcc:03x} imsi_11_12=0x{imsi_11_12:02x} imsi_s=0x{imsi_s:09x}"
                ),
                _ => format!("class0_raw={}", bits_to_hex(bits.bits())),
            }
        }
        _ => format!("class0_raw={}", bits_to_hex(bits.bits())),
    };
    *bits = clone;
    summary
}

fn format_imsi_class_1(bits: &mut Bitstream) -> String {
    let mut clone = bits.clone();
    let Some(class_1_type) = clone.read_bits(1).ok().map(|v| v as u8) else {
        return format!("class1_raw={}", bits_to_hex(bits.bits()));
    };
    let summary = match class_1_type {
        0 => {
            let reserved = clone.read_bits(2).ok();
            let imsi_addr_num = clone.read_bits(3).ok();
            let imsi_11_12 = clone.read_bits(7).ok();
            let imsi_s = clone.read_bits(34).ok();
            match (reserved, imsi_addr_num, imsi_11_12, imsi_s) {
                (Some(reserved), Some(imsi_addr_num), Some(imsi_11_12), Some(imsi_s)) => format!(
                    "class1_type=0 reserved=0x{reserved:x} imsi_addr_num={} imsi_11_12=0x{imsi_11_12:02x} imsi_s=0x{imsi_s:09x}",
                    imsi_addr_num
                ),
                _ => format!("class1_raw={}", bits_to_hex(bits.bits())),
            }
        }
        1 => {
            let imsi_addr_num = clone.read_bits(3).ok();
            let mcc = clone.read_bits(10).ok();
            let imsi_11_12 = clone.read_bits(7).ok();
            let imsi_s = clone.read_bits(34).ok();
            match (imsi_addr_num, mcc, imsi_11_12, imsi_s) {
                (Some(imsi_addr_num), Some(mcc), Some(imsi_11_12), Some(imsi_s)) => format!(
                    "class1_type=1 imsi_addr_num={} mcc=0x{mcc:03x} imsi_11_12=0x{imsi_11_12:02x} imsi_s=0x{imsi_s:09x}",
                    imsi_addr_num
                ),
                _ => format!("class1_raw={}", bits_to_hex(bits.bits())),
            }
        }
        _ => format!("class1_raw={}", bits_to_hex(bits.bits())),
    };
    *bits = clone;
    summary
}

fn bits_to_hex(bits: &[u8]) -> String {
    if bits.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for chunk in bits.chunks(8) {
        let mut val = 0u8;
        for &b in chunk {
            val = (val << 1) | (b & 1);
        }
        if chunk.len() < 8 {
            val <<= 8 - chunk.len();
        }
        use std::fmt::Write;
        let _ = write!(&mut out, "{val:02x}");
    }
    out
}

fn parse_rcsch_arq_fields(
    bs: &mut Bitstream,
    warnings: &mut Vec<String>,
) -> Option<RcschArqFields> {
    if bs.len() < 11 {
        warnings.push(format!("short_arq_region={}bits", bs.len()));
        return None;
    }

    let mut raw = bs.drain(0..11);
    let mut parsed = raw.clone();
    let ack_seq = parsed.read_bits(3).ok()? as u8;
    let msg_seq = parsed.read_bits(3).ok()? as u8;
    let ack_req = parsed.read_bits(1).ok()? == 1;
    let valid_ack = parsed.read_bits(1).ok()? == 1;
    let ack_type = parsed.read_bits(3).ok()? as u8;
    let ext_ack_type = if ack_type == 0b100 {
        if bs.len() < 3 {
            warnings.push("missing_ext_ack_type".to_string());
            None
        } else {
            let ext_raw = bs.drain(0..3);
            let mut ext_bits = ext_raw.clone();
            let value = ext_bits.read_bits(3).ok()? as u8;
            raw.extend(&ext_raw);
            Some(value)
        }
    } else {
        None
    };

    Some(RcschArqFields {
        raw,
        ack_seq,
        msg_seq,
        ack_req,
        valid_ack,
        ack_type,
        ext_ack_type,
    })
}

fn parse_p_rev6_radio_environment_report(
    bs: &mut Bitstream,
    warnings: &mut Vec<String>,
) -> Option<RcschPRev6RadioEnvironmentReport> {
    if bs.len() < 11 {
        return None;
    }

    let mut raw = bs.drain(0..11);
    let mut parsed = raw.clone();
    let active_pilot_strength = parsed.read_bits(6).ok()? as u8;
    let first_is_active = parsed.read_bits(1).ok()? == 1;
    let first_is_pta = parsed.read_bits(1).ok()? == 1;
    let num_add_pilots = parsed.read_bits(3).ok()? as u8;
    let mut additional_pilots = Vec::with_capacity(num_add_pilots as usize);

    for idx in 0..num_add_pilots {
        if bs.len() < 23 {
            warnings.push(format!(
                "short_rer_pilot_record_at_index={} remaining_bits={}",
                idx,
                bs.len()
            ));
            break;
        }
        let record_raw = bs.drain(0..23);
        raw.extend(&record_raw);
        let mut rec = record_raw.clone();
        additional_pilots.push(RcschPRev6PilotRecord {
            pilot_pn_phase: rec.read_bits(15).ok()? as u16,
            pilot_strength: rec.read_bits(6).ok()? as u8,
            access_ho_en: rec.read_bits(1).ok()? == 1,
            access_attempted: rec.read_bits(1).ok()? == 1,
        });
    }

    Some(RcschPRev6RadioEnvironmentReport {
        raw,
        active_pilot_strength,
        first_is_active,
        first_is_pta,
        num_add_pilots,
        additional_pilots,
    })
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;

    use super::ReverseAccessPdu;
    use crate::lac::message_types::{MessageId, WireChannel};
    use crate::receiver::access_layer3::{AccessDecodeContext, AccessMessage, AccessMessageHeader};

    #[test]
    fn test_pd01_p_rev6_wrapper_parse() {
        let mut bits = Bitstream::new();
        bits.write_u8(0x44, 8);
        bits.write_u8(2, 5);
        bits.write_u8(0b101, 3);
        bits.write_u8(0b011, 3);
        bits.write_u8(1, 1);
        bits.write_u8(0, 1);
        bits.write_u8(0b010, 3);
        bits.write_u8(7, 6);
        bits.write_u8(1, 1);
        bits.write_u8(0, 1);
        bits.write_u8(0, 3);
        bits.write_u8(0xa5, 8);

        let pdu = ReverseAccessPdu::decode(&bits).expect("decode pd01 pdu");
        let ReverseAccessPdu::Pd01PRev6(pdu) = pdu else {
            panic!("expected P_REV 6 wrapper");
        };
        assert_eq!(1, pdu.header.pd);
        assert_eq!(0b000100, pdu.header.msg_type);
        assert_eq!(2, pdu.lac_length_octets);
        let arq = pdu.arq.expect("arq");
        assert_eq!(0b101, arq.ack_seq);
        assert_eq!(0b011, arq.msg_seq);
        assert!(arq.ack_req);
        assert!(!arq.valid_ack);
        assert_eq!(0b010, arq.ack_type);
        assert!(arq.ext_ack_type.is_none());
        let rer = pdu.radio_environment_report.expect("rer");
        assert_eq!(7, rer.active_pilot_strength);
        assert!(rer.first_is_active);
        assert!(!rer.first_is_pta);
        assert_eq!(0, rer.num_add_pilots);
        assert_eq!(8, pdu.sdu_plus_padding_raw.len());
    }

    #[test]
    fn test_pd01_trace_shape_parse() {
        let bytes = [
            0x44, 0x7f, 0x10, 0x76, 0x99, 0xb8, 0x3a, 0x12, 0xc3, 0x47, 0x1e, 0x4d, 0x23, 0x31,
            0x3c, 0x00, 0x1e, 0x14, 0x0c, 0x54, 0x60, 0x00, 0x60, 0x0c, 0x48, 0xc0, 0x18, 0x02,
            0x24, 0x00, 0x00, 0x08, 0xca, 0x63, 0x1c, 0xbe, 0x5e, 0x54, 0x72, 0x31, 0x00,
        ];
        let bits = Bitstream::new_bytes(&bytes);
        let pdu = ReverseAccessPdu::decode(&bits).expect("decode captured pdu");
        let ReverseAccessPdu::Pd01PRev6(pdu) = pdu else {
            panic!("expected P_REV 6 wrapper");
        };
        assert_eq!(1, pdu.header.pd);
        assert_eq!(0b000100, pdu.header.msg_type);
        assert_eq!(15, pdu.lac_length_octets);
        assert!(pdu.arq.is_some());
        assert!(pdu.radio_environment_report.is_some());
    }

    #[test]
    fn test_reverse_access_pdu_rejects_reserved_pd() {
        let mut bits = Bitstream::new();
        bits.write_u8(0xC0, 8);

        let err = ReverseAccessPdu::decode(&bits).unwrap_err();

        assert!(err.contains("unsupported r-csch PD 0b11"));
    }

    #[test]
    fn test_reverse_access_pdu_rejects_unsupported_pd10() {
        let mut bits = Bitstream::new();
        bits.write_u8(0x80, 8);

        let err = ReverseAccessPdu::decode(&bits).unwrap_err();

        assert!(err.contains("unsupported r-csch PD 0b10"));
        assert!(err.contains("P_REV_IN_USE >= 9 wrapper is not decoded"));
    }

    #[test]
    fn test_reverse_access_summary_rejects_unmapped_tag_without_gem_fallback() {
        let mut bits = Bitstream::new();
        bits.write_u8(0x0B, 8);
        bits.write_u8(0, 8);

        let pdu = ReverseAccessPdu::decode(&bits).expect("decode PD=00 wrapper");
        let summary = pdu.summary();

        assert!(summary.contains("unsupported r-csch MSG_TAG 0x0B"));
        assert!(!summary.contains("GeneralExtension"));
    }

    fn decode_origination_capture(hex: &str) -> AccessMessage {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect();
        let bits = Bitstream::new_bytes(&bytes);
        let pdu = ReverseAccessPdu::decode(&bits).expect("decode reverse access PDU");
        let (pd, msg_type, sdu) = match pdu {
            ReverseAccessPdu::Pd00Legacy(p) => {
                (p.header.pd, p.header.msg_type, p.sdu_plus_padding_raw)
            }
            ReverseAccessPdu::Pd01PRev6(p) => {
                (p.header.pd, p.header.msg_type, p.sdu_plus_padding_raw)
            }
            _ => panic!("unexpected wrapper variant"),
        };
        let message_id = MessageId::from_wire(WireChannel::ReverseCommon, msg_type)
            .expect("known r-csch MSG_TAG");
        let header = AccessMessageHeader { pd, message_id };
        let ctx = AccessDecodeContext::new(Some(0), None);
        AccessMessage::decode_sdu_with_context(header, &sdu, ctx).expect("decode L3 SDU")
    }

    #[test]
    fn capture_origination_p_rev3_so32768_a() {
        let msg =
            decode_origination_capture("04e20ed84c60ff9891e36215e2194a036a7800000e42a890e000");
        let AccessMessage::Origination(orig) = msg else {
            panic!("expected Origination");
        };
        assert_eq!(orig.mob_p_rev, 3);
        assert_eq!(orig.service_option, Some(32768));
        assert!(orig.special_service);
        assert_eq!(orig.for_rc_pref, None);
        assert_eq!(orig.rev_rc_pref, None);
        assert_eq!(orig.fch_supported, None);
        assert_eq!(orig.encryption_supported, None);
    }

    #[test]
    fn capture_origination_p_rev3_so32768_b() {
        let msg = decode_origination_capture("04e20ed95a83761891e3f9fe79104a036a7800000c42aa8a00");
        let AccessMessage::Origination(orig) = msg else {
            panic!("expected Origination");
        };
        assert_eq!(orig.mob_p_rev, 3);
        assert_eq!(orig.service_option, Some(32768));
        assert_eq!(orig.for_rc_pref, None);
        assert_eq!(orig.encryption_supported, None);
    }

    #[test]
    fn capture_origination_p_rev3_so3() {
        let msg = decode_origination_capture("04020ed3813faa59f3e3d1aa08f789036a7000300906f000");
        let AccessMessage::Origination(orig) = msg else {
            panic!("expected Origination");
        };
        assert_eq!(orig.mob_p_rev, 3);
        assert_eq!(orig.service_option, Some(3));
        assert_eq!(orig.encryption_supported, None);
    }

    #[test]
    fn capture_origination_p_rev6_so32768() {
        let msg = decode_origination_capture(
            "447f10762b3c3636c48e5b117da040001e140c507000001cdd51204004a6524be5e547231000",
        );
        let AccessMessage::Origination(orig) = msg else {
            panic!("expected Origination");
        };
        assert_eq!(orig.mob_p_rev, 6);
        assert_eq!(orig.service_option, Some(32768));
        // Valid RC values are 1..=12 per C.S0005-E §3.7.2.3.2.21-4.
        let for_rc = orig.for_rc_pref.expect("for_rc_pref present for P_REV=6");
        assert!(
            (1..=12).contains(&for_rc),
            "for_rc_pref={} out of range",
            for_rc
        );
        assert_eq!(orig.fch_supported, Some(true));
        let fch = orig.fch_capability.as_ref().expect("fch_capability");
        assert!(!fch.for_supported_rcs.is_empty());
    }

    #[test]
    fn capture_origination_p_rev6_so33() {
        let msg = decode_origination_capture(
            "447f107667f247b2c48f1fcff3ac44001a140cd4600420131ddc0051631cbe5e54723100",
        );
        let AccessMessage::Origination(orig) = msg else {
            panic!("expected Origination");
        };
        assert_eq!(orig.mob_p_rev, 6);
        assert_eq!(orig.service_option, Some(33));
        let for_rc = orig.for_rc_pref.expect("for_rc_pref present for P_REV=6");
        assert!(
            (1..=12).contains(&for_rc),
            "for_rc_pref={} out of range",
            for_rc
        );
        assert_eq!(orig.fch_supported, Some(true));
        let fch = orig.fch_capability.as_ref().expect("fch_capability");
        assert!(!fch.for_supported_rcs.is_empty());
    }
}
