//! Rust ↔ proto conversion for the PRL decoded-tree messages.
//!
//! Decode side: bytes → `cdma_otasp::param::{prl, prl_ext}` types →
//! `proto::PrlDecoded` (used by `GetPrl` to populate the editor).
//!
//! Encode side: receive a `proto::PrlDecoded` from the editor, convert
//! back to the cdma_otasp types, call `encode()` to produce canonical
//! bytes with a fresh CRC. (Editor write path: see `decode_prl_body`.)
//!
//! Both directions share a single set of conversion helpers. Where a
//! variant has no obvious default (e.g. `PrlPrefNeg::Unspecified`),
//! we return a typed error so the caller can reject the request
//! cleanly via `PrlValidationError`.

use crate::model::PrlValidationFailure;
use crate::proto;
use cdma_otasp::param::{prl, prl_ext};

/// Decode raw PRL bytes (classic or extended) into a populated
/// `proto::Prl` message.
///
/// `summary` is the cached row metadata; we add the decoded tree and
/// the raw bytes alongside.
pub fn proto_from_raw_bytes(
    summary: proto::PrlSummary,
    raw_bytes: Vec<u8>,
) -> Result<proto::Prl, PrlValidationFailure> {
    let decoded = decode_to_proto(&raw_bytes)?;
    Ok(proto::Prl {
        summary: Some(summary),
        raw_bytes,
        decoded: Some(decoded),
    })
}

/// Decode `bytes` (classic or extended) and convert to `proto::PrlDecoded`.
pub fn decode_to_proto(bytes: &[u8]) -> Result<proto::PrlDecoded, PrlValidationFailure> {
    // Try classic first; if it fails, try extended.
    match prl::decode(bytes) {
        Ok(c) => Ok(proto::PrlDecoded {
            body: Some(proto::prl_decoded::Body::Classic(classic_to_proto(&c))),
        }),
        Err(_) => match prl_ext::decode(bytes) {
            Ok(e) => Ok(proto::PrlDecoded {
                body: Some(proto::prl_decoded::Body::Extended(extended_to_proto(&e))),
            }),
            Err(e) => Err(PrlValidationFailure::DecodeFailed(e.to_string())),
        },
    }
}

/// Take a `proto::PrlDecoded` from the editor, encode it via the
/// cdma_otasp encoders, and return canonical on-wire bytes.
pub fn encode_proto_to_bytes(decoded: &proto::PrlDecoded) -> Result<Vec<u8>, PrlValidationFailure> {
    match &decoded.body {
        Some(proto::prl_decoded::Body::Classic(c)) => {
            let cp = proto_to_classic(c)?;
            cp.encode()
                .map_err(|e| PrlValidationFailure::EncodeFailed(e.to_string()))
        }
        Some(proto::prl_decoded::Body::Extended(e)) => {
            let ep = proto_to_extended(e)?;
            ep.encode()
                .map_err(|e| PrlValidationFailure::EncodeFailed(e.to_string()))
        }
        None => Err(PrlValidationFailure::DecodeFailed(
            "PrlDecoded body missing".into(),
        )),
    }
}

// ─── Classic ────────────────────────────────────────────────────────

fn classic_to_proto(c: &prl::ClassicPrl) -> proto::PrlClassicBody {
    proto::PrlClassicBody {
        pr_list_size: c.pr_list_size as u32,
        pr_list_id: c.pr_list_id as u32,
        pref_only: c.pref_only,
        def_roam_ind: Some(roam_to_proto(c.def_roam_ind)),
        pr_list_crc: c.pr_list_crc as u32,
        computed_crc: c.computed_crc as u32,
        crc_ok: c.crc_ok(),
        acquisition_records: c.acquisition_records.iter().map(acq_to_proto).collect(),
        system_records: c.system_records.iter().map(|s| sys_to_proto(*s)).collect(),
    }
}

fn proto_to_classic(body: &proto::PrlClassicBody) -> Result<prl::ClassicPrl, PrlValidationFailure> {
    Ok(prl::ClassicPrl {
        pr_list_size: body.pr_list_size as u16,
        pr_list_id: body.pr_list_id as u16,
        pref_only: body.pref_only,
        def_roam_ind: proto_to_roam(body.def_roam_ind.as_ref()),
        acquisition_records: body
            .acquisition_records
            .iter()
            .map(proto_to_acq)
            .collect::<Result<_, _>>()?,
        system_records: body
            .system_records
            .iter()
            .map(proto_to_sys)
            .collect::<Result<_, _>>()?,
        pr_list_crc: 0, // recomputed by encode()
        computed_crc: 0,
    })
}

fn acq_to_proto(r: &prl::AcquisitionRecord) -> proto::PrlAcqRecord {
    use proto::prl_acq_record::Body as B;
    let body = match &r.body {
        prl::AcquisitionBody::CellularAnalog { ab } => {
            B::CellularAnalog(proto::PrlAcqCellularAnalog {
                ab: ab_to_proto(*ab),
            })
        }
        prl::AcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
            B::CellularCdmaStandard(proto::PrlAcqCellularCdmaStandard {
                ab: ab_to_proto(*ab),
                pri_sec: std_chan_to_proto(*pri_sec),
            })
        }
        prl::AcquisitionBody::CellularCdmaCustom { channels } => {
            B::CellularCdmaCustom(proto::PrlAcqCellularCdmaCustom {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl::AcquisitionBody::CellularCdmaPreferred { ab } => {
            B::CellularCdmaPreferred(proto::PrlAcqCellularCdmaPreferred {
                ab: ab_to_proto(*ab),
            })
        }
        prl::AcquisitionBody::PcsCdmaUsingBlocks { blocks } => {
            B::PcsCdmaUsingBlocks(proto::PrlAcqPcsCdmaUsingBlocks {
                blocks: blocks.iter().map(|b| pcs_to_proto(*b)).collect(),
            })
        }
        prl::AcquisitionBody::PcsCdmaUsingChannels { channels } => {
            B::PcsCdmaUsingChannels(proto::PrlAcqPcsCdmaUsingChannels {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl::AcquisitionBody::JtacsCdmaStandard { ab, pri_sec } => {
            B::JtacsCdmaStandard(proto::PrlAcqJtacsCdmaStandard {
                ab: ab_to_proto(*ab),
                pri_sec: std_chan_to_proto(*pri_sec),
            })
        }
        prl::AcquisitionBody::JtacsCdmaCustom { channels } => {
            B::JtacsCdmaCustom(proto::PrlAcqJtacsCdmaCustom {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl::AcquisitionBody::BandClass6UsingChannels { channels } => {
            B::BandClass6UsingChannels(proto::PrlAcqBandClass6UsingChannels {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl::AcquisitionBody::Unknown => B::Unknown(proto::PrlAcqUnknown {}),
    };
    proto::PrlAcqRecord {
        acq_type_raw: r.acq_type_raw as u32,
        body: Some(body),
    }
}

fn proto_to_acq(r: &proto::PrlAcqRecord) -> Result<prl::AcquisitionRecord, PrlValidationFailure> {
    use proto::prl_acq_record::Body as B;
    let body = match r
        .body
        .as_ref()
        .ok_or_else(|| PrlValidationFailure::DecodeFailed("PrlAcqRecord.body missing".into()))?
    {
        B::CellularAnalog(b) => prl::AcquisitionBody::CellularAnalog {
            ab: proto_to_ab(b.ab)?,
        },
        B::CellularCdmaStandard(b) => prl::AcquisitionBody::CellularCdmaStandard {
            ab: proto_to_ab(b.ab)?,
            pri_sec: proto_to_std_chan(b.pri_sec)?,
        },
        B::CellularCdmaCustom(b) => prl::AcquisitionBody::CellularCdmaCustom {
            channels: b.channels.iter().map(|c| *c as u16).collect(),
        },
        B::CellularCdmaPreferred(b) => prl::AcquisitionBody::CellularCdmaPreferred {
            ab: proto_to_ab(b.ab)?,
        },
        B::PcsCdmaUsingBlocks(b) => prl::AcquisitionBody::PcsCdmaUsingBlocks {
            blocks: b
                .blocks
                .iter()
                .map(|v| proto_to_pcs(*v))
                .collect::<Result<_, _>>()?,
        },
        B::PcsCdmaUsingChannels(b) => prl::AcquisitionBody::PcsCdmaUsingChannels {
            channels: b.channels.iter().map(|c| *c as u16).collect(),
        },
        B::JtacsCdmaStandard(b) => prl::AcquisitionBody::JtacsCdmaStandard {
            ab: proto_to_ab(b.ab)?,
            pri_sec: proto_to_std_chan(b.pri_sec)?,
        },
        B::JtacsCdmaCustom(b) => prl::AcquisitionBody::JtacsCdmaCustom {
            channels: b.channels.iter().map(|c| *c as u16).collect(),
        },
        B::BandClass6UsingChannels(b) => prl::AcquisitionBody::BandClass6UsingChannels {
            channels: b.channels.iter().map(|c| *c as u16).collect(),
        },
        B::Unknown(_) => {
            return Err(PrlValidationFailure::EncodeFailed(
                "cannot encode Unknown acquisition record".into(),
            ));
        }
    };
    Ok(prl::AcquisitionRecord {
        acq_type_raw: r.acq_type_raw as u8,
        body,
    })
}

fn sys_to_proto(s: prl::SystemRecord) -> proto::PrlSysRecord {
    proto::PrlSysRecord {
        sid: s.sid as u32,
        nid_incl: nid_incl_to_proto(s.nid_incl),
        nid: s.nid.map(|v| v as u32),
        same_geo_as_prev: s.same_geo_as_prev,
        pref_neg: pref_neg_to_proto(s.pref_neg),
        acq_index: s.acq_index as u32,
        roaming_indicator: s.roaming_indicator.map(roam_to_proto),
        priority: s.priority.map(prio_to_proto),
    }
}

fn proto_to_sys(s: &proto::PrlSysRecord) -> Result<prl::SystemRecord, PrlValidationFailure> {
    Ok(prl::SystemRecord {
        sid: s.sid as u16,
        nid_incl: proto_to_nid_incl(s.nid_incl)?,
        nid: s.nid.map(|v| v as u16),
        same_geo_as_prev: s.same_geo_as_prev,
        pref_neg: proto_to_pref_neg(s.pref_neg)?,
        acq_index: s.acq_index as u16,
        roaming_indicator: s.roaming_indicator.as_ref().map(|r| proto_to_roam(Some(r))),
        priority: s.priority.map(proto_to_prio).transpose()?,
    })
}

// ─── Extended ────────────────────────────────────────────────────────

fn extended_to_proto(e: &prl_ext::ExtendedPrl) -> proto::PrlExtendedBody {
    proto::PrlExtendedBody {
        pr_list_size: e.pr_list_size as u32,
        pr_list_id: e.pr_list_id as u32,
        cur_sspr_p_rev: e.cur_sspr_p_rev as u32,
        pref_only: e.pref_only,
        def_roam_ind: Some(roam_to_proto(e.def_roam_ind)),
        pr_list_crc: e.pr_list_crc as u32,
        computed_crc: e.computed_crc as u32,
        crc_ok: e.crc_ok(),
        acquisition_records: e.acquisition_records.iter().map(ext_acq_to_proto).collect(),
        common_subnet_records: e
            .common_subnet_records
            .iter()
            .map(|r| proto::PrlCommonSubnetRecord {
                subnet_common_length_octets: r.subnet_common_length as u32,
                subnet_common_hex: bytes_to_hex(&r.subnet_common),
            })
            .collect(),
        system_records: e.system_records.iter().map(ext_sys_to_proto).collect(),
    }
}

fn proto_to_extended(
    body: &proto::PrlExtendedBody,
) -> Result<prl_ext::ExtendedPrl, PrlValidationFailure> {
    Ok(prl_ext::ExtendedPrl {
        pr_list_size: body.pr_list_size as u16,
        pr_list_id: body.pr_list_id as u16,
        cur_sspr_p_rev: body.cur_sspr_p_rev as u8,
        pref_only: body.pref_only,
        def_roam_ind: proto_to_roam(body.def_roam_ind.as_ref()),
        acquisition_records: body
            .acquisition_records
            .iter()
            .map(proto_to_ext_acq)
            .collect::<Result<_, _>>()?,
        common_subnet_records: body
            .common_subnet_records
            .iter()
            .map(|r| {
                let octets = r.subnet_common_length_octets as usize;
                let subnet_common = hex_to_bytes(&r.subnet_common_hex, octets)?;
                Ok(prl_ext::CommonSubnetRecord {
                    subnet_common_length: r.subnet_common_length_octets as u8,
                    subnet_common,
                })
            })
            .collect::<Result<_, _>>()?,
        system_records: body
            .system_records
            .iter()
            .map(proto_to_ext_sys)
            .collect::<Result<_, _>>()?,
        pr_list_crc: 0,
        computed_crc: 0,
    })
}

fn ext_acq_to_proto(r: &prl_ext::ExtAcquisitionRecord) -> proto::PrlExtAcqRecord {
    use proto::prl_ext_acq_record::Body as B;
    let body = match &r.body {
        prl_ext::ExtAcquisitionBody::CellularAnalog { ab } => {
            B::CellularAnalog(proto::PrlAcqCellularAnalog {
                ab: ab_to_proto(*ab),
            })
        }
        prl_ext::ExtAcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
            B::CellularCdmaStandard(proto::PrlAcqCellularCdmaStandard {
                ab: ab_to_proto(*ab),
                pri_sec: std_chan_to_proto(*pri_sec),
            })
        }
        prl_ext::ExtAcquisitionBody::CellularCdmaCustom { channels } => {
            B::CellularCdmaCustom(proto::PrlAcqCellularCdmaCustom {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::CellularCdmaPreferred { ab } => {
            B::CellularCdmaPreferred(proto::PrlAcqCellularCdmaPreferred {
                ab: ab_to_proto(*ab),
            })
        }
        prl_ext::ExtAcquisitionBody::PcsCdmaUsingBlocks { blocks } => {
            B::PcsCdmaUsingBlocks(proto::PrlAcqPcsCdmaUsingBlocks {
                blocks: blocks.iter().map(|b| pcs_to_proto(*b)).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::PcsCdmaUsingChannels { channels } => {
            B::PcsCdmaUsingChannels(proto::PrlAcqPcsCdmaUsingChannels {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::JtacsCdmaStandard { ab, pri_sec } => {
            B::JtacsCdmaStandard(proto::PrlAcqJtacsCdmaStandard {
                ab: ab_to_proto(*ab),
                pri_sec: std_chan_to_proto(*pri_sec),
            })
        }
        prl_ext::ExtAcquisitionBody::JtacsCdmaCustom { channels } => {
            B::JtacsCdmaCustom(proto::PrlAcqJtacsCdmaCustom {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::BandClass6UsingChannels { channels } => {
            B::BandClass6UsingChannels(proto::PrlAcqBandClass6UsingChannels {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::Generic1xIs95 { entries } => {
            B::Generic1xIs95(proto::PrlExtAcqGeneric1xIs95 {
                entries: entries.iter().map(bcc_to_proto).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::GenericHrpd { entries } => {
            B::GenericHrpd(proto::PrlExtAcqGenericHrpd {
                entries: entries.iter().map(bcc_to_proto).collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::UmbCommonTable { entries } => {
            B::UmbCommonTable(proto::PrlExtAcqUmbCommonTable {
                entries: entries
                    .iter()
                    .map(|e| proto::PrlUmbAcqProfile {
                        umb_acq_profile: e.umb_acq_profile as u32,
                        fft_size: e.fft_size as u32,
                        cyclic_prefix_length: e.cyclic_prefix_length as u32,
                        num_guard_subcarriers: e.num_guard_subcarriers as u32,
                    })
                    .collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::GenericUmb { blocks } => {
            B::GenericUmb(proto::PrlExtAcqGenericUmb {
                blocks: blocks
                    .iter()
                    .map(|b| proto::PrlUmbBlock {
                        band_class: b.band_class as u32,
                        channel_number: b.channel_number as u32,
                        umb_acq_table_profile: b.umb_acq_table_profile as u32,
                    })
                    .collect(),
            })
        }
        prl_ext::ExtAcquisitionBody::Other { raw } => {
            B::Other(proto::PrlExtAcqOther { raw: raw.clone() })
        }
    };
    proto::PrlExtAcqRecord {
        acq_type_raw: r.acq_type_raw as u32,
        length: r.length as u32,
        body: Some(body),
    }
}

fn proto_to_ext_acq(
    r: &proto::PrlExtAcqRecord,
) -> Result<prl_ext::ExtAcquisitionRecord, PrlValidationFailure> {
    use proto::prl_ext_acq_record::Body as B;
    let body =
        match r.body.as_ref().ok_or_else(|| {
            PrlValidationFailure::DecodeFailed("PrlExtAcqRecord.body missing".into())
        })? {
            B::CellularAnalog(b) => prl_ext::ExtAcquisitionBody::CellularAnalog {
                ab: proto_to_ab(b.ab)?,
            },
            B::CellularCdmaStandard(b) => prl_ext::ExtAcquisitionBody::CellularCdmaStandard {
                ab: proto_to_ab(b.ab)?,
                pri_sec: proto_to_std_chan(b.pri_sec)?,
            },
            B::CellularCdmaCustom(b) => prl_ext::ExtAcquisitionBody::CellularCdmaCustom {
                channels: b.channels.iter().map(|c| *c as u16).collect(),
            },
            B::CellularCdmaPreferred(b) => prl_ext::ExtAcquisitionBody::CellularCdmaPreferred {
                ab: proto_to_ab(b.ab)?,
            },
            B::PcsCdmaUsingBlocks(b) => prl_ext::ExtAcquisitionBody::PcsCdmaUsingBlocks {
                blocks: b
                    .blocks
                    .iter()
                    .map(|v| proto_to_pcs(*v))
                    .collect::<Result<_, _>>()?,
            },
            B::PcsCdmaUsingChannels(b) => prl_ext::ExtAcquisitionBody::PcsCdmaUsingChannels {
                channels: b.channels.iter().map(|c| *c as u16).collect(),
            },
            B::JtacsCdmaStandard(b) => prl_ext::ExtAcquisitionBody::JtacsCdmaStandard {
                ab: proto_to_ab(b.ab)?,
                pri_sec: proto_to_std_chan(b.pri_sec)?,
            },
            B::JtacsCdmaCustom(b) => prl_ext::ExtAcquisitionBody::JtacsCdmaCustom {
                channels: b.channels.iter().map(|c| *c as u16).collect(),
            },
            B::BandClass6UsingChannels(b) => prl_ext::ExtAcquisitionBody::BandClass6UsingChannels {
                channels: b.channels.iter().map(|c| *c as u16).collect(),
            },
            B::Generic1xIs95(b) => prl_ext::ExtAcquisitionBody::Generic1xIs95 {
                entries: b.entries.iter().map(proto_to_bcc).collect(),
            },
            B::GenericHrpd(b) => prl_ext::ExtAcquisitionBody::GenericHrpd {
                entries: b.entries.iter().map(proto_to_bcc).collect(),
            },
            B::UmbCommonTable(b) => prl_ext::ExtAcquisitionBody::UmbCommonTable {
                entries: b
                    .entries
                    .iter()
                    .map(|e| prl_ext::UmbAcqProfile {
                        umb_acq_profile: e.umb_acq_profile as u8,
                        fft_size: e.fft_size as u8,
                        cyclic_prefix_length: e.cyclic_prefix_length as u8,
                        num_guard_subcarriers: e.num_guard_subcarriers as u8,
                    })
                    .collect(),
            },
            B::GenericUmb(b) => prl_ext::ExtAcquisitionBody::GenericUmb {
                blocks: b
                    .blocks
                    .iter()
                    .map(|b| prl_ext::UmbBlock {
                        band_class: b.band_class as u8,
                        channel_number: b.channel_number as u16,
                        umb_acq_table_profile: b.umb_acq_table_profile as u8,
                    })
                    .collect(),
            },
            B::Other(b) => prl_ext::ExtAcquisitionBody::Other { raw: b.raw.clone() },
        };
    Ok(prl_ext::ExtAcquisitionRecord {
        acq_type_raw: r.acq_type_raw as u8,
        length: r.length as u8,
        body,
    })
}

fn bcc_to_proto(b: &prl_ext::BandClassChannel) -> proto::PrlBandClassChannel {
    proto::PrlBandClassChannel {
        band_class: b.band_class as u32,
        channel_number: b.channel_number as u32,
    }
}
fn proto_to_bcc(b: &proto::PrlBandClassChannel) -> prl_ext::BandClassChannel {
    prl_ext::BandClassChannel {
        band_class: b.band_class as u8,
        channel_number: b.channel_number as u16,
    }
}

fn ext_sys_to_proto(s: &prl_ext::ExtSystemRecord) -> proto::PrlExtSysRecord {
    use proto::prl_ext_sys_record::SystemId as SI;
    let (sys_record_type, sys_record_type_raw) = match s.sys_record_type {
        prl_ext::ExtSystemRecordType::Cdma2000 => (proto::PrlExtSysRecordType::Cdma2000, 0u8),
        prl_ext::ExtSystemRecordType::Hrpd => (proto::PrlExtSysRecordType::Hrpd, 1),
        prl_ext::ExtSystemRecordType::ReservedObsolete => {
            (proto::PrlExtSysRecordType::ReservedObsolete, 2)
        }
        prl_ext::ExtSystemRecordType::MccMnc => (proto::PrlExtSysRecordType::MccMnc, 3),
        prl_ext::ExtSystemRecordType::Reserved(v) => (proto::PrlExtSysRecordType::Reserved, v),
    };
    let system_id = match &s.system_id {
        prl_ext::ExtSystemId::Cdma2000 { nid_incl, sid, nid } => {
            SI::Cdma2000(proto::PrlExtSysIdCdma2000 {
                nid_incl: nid_incl_to_proto(*nid_incl),
                sid: *sid as u32,
                nid: nid.map(|v| v as u32),
            })
        }
        prl_ext::ExtSystemId::Hrpd {
            subnet_common_included,
            subnet_lsb_length,
            subnet_lsb,
            subnet_common_offset,
        } => SI::Hrpd(proto::PrlExtSysIdHrpd {
            subnet_common_included: *subnet_common_included,
            subnet_lsb_length_bits: *subnet_lsb_length as u32,
            subnet_lsb_hex: bytes_to_hex(subnet_lsb),
            subnet_common_offset: subnet_common_offset.map(|v| v as u32),
        }),
        prl_ext::ExtSystemId::MccMnc(sub) => SI::MccMnc(mccmnc_to_proto(sub)),
        prl_ext::ExtSystemId::Raw {
            sys_record_type,
            raw_bits,
            raw_bit_len,
        } => SI::Raw(proto::PrlExtSysIdRaw {
            sys_record_type: *sys_record_type as u32,
            raw_bits: raw_bits.clone(),
            raw_bit_len: *raw_bit_len as u32,
        }),
    };
    proto::PrlExtSysRecord {
        sys_record_length: s.sys_record_length as u32,
        sys_record_type: sys_record_type as i32,
        sys_record_type_raw: sys_record_type_raw as u32,
        pref_neg: pref_neg_to_proto(s.pref_neg),
        same_geo_as_prev: s.same_geo_as_prev,
        priority: prio_to_proto(s.priority),
        acq_index: s.acq_index as u32,
        system_id: Some(system_id),
        roaming_indicator: s.roaming_indicator.map(roam_to_proto),
        association: s.association.map(|a| proto::PrlExtSystemAssociation {
            association_tag: a.association_tag as u32,
            pn_association: a.pn_association,
            data_association: a.data_association,
        }),
    }
}

fn proto_to_ext_sys(
    s: &proto::PrlExtSysRecord,
) -> Result<prl_ext::ExtSystemRecord, PrlValidationFailure> {
    use proto::prl_ext_sys_record::SystemId as SI;
    let sys_record_type = match proto::PrlExtSysRecordType::try_from(s.sys_record_type)
        .unwrap_or(proto::PrlExtSysRecordType::Unspecified)
    {
        proto::PrlExtSysRecordType::Cdma2000 => prl_ext::ExtSystemRecordType::Cdma2000,
        proto::PrlExtSysRecordType::Hrpd => prl_ext::ExtSystemRecordType::Hrpd,
        proto::PrlExtSysRecordType::ReservedObsolete => {
            prl_ext::ExtSystemRecordType::ReservedObsolete
        }
        proto::PrlExtSysRecordType::MccMnc => prl_ext::ExtSystemRecordType::MccMnc,
        proto::PrlExtSysRecordType::Reserved => {
            prl_ext::ExtSystemRecordType::Reserved(s.sys_record_type_raw as u8)
        }
        proto::PrlExtSysRecordType::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlExtSysRecord.sys_record_type = UNSPECIFIED".into(),
            ));
        }
    };
    let system_id = match s.system_id.as_ref().ok_or_else(|| {
        PrlValidationFailure::DecodeFailed("PrlExtSysRecord.system_id missing".into())
    })? {
        SI::Cdma2000(b) => prl_ext::ExtSystemId::Cdma2000 {
            nid_incl: proto_to_nid_incl(b.nid_incl)?,
            sid: b.sid as u16,
            nid: b.nid.map(|v| v as u16),
        },
        SI::Hrpd(b) => {
            let expected_bytes = b.subnet_lsb_length_bits.div_ceil(8) as usize;
            let subnet_lsb = hex_to_bytes(&b.subnet_lsb_hex, expected_bytes)?;
            prl_ext::ExtSystemId::Hrpd {
                subnet_common_included: b.subnet_common_included,
                subnet_lsb_length: b.subnet_lsb_length_bits as u8,
                subnet_lsb,
                subnet_common_offset: b.subnet_common_offset.map(|v| v as u16),
            }
        }
        SI::MccMnc(b) => prl_ext::ExtSystemId::MccMnc(proto_to_mccmnc(b)?),
        SI::Raw(b) => prl_ext::ExtSystemId::Raw {
            sys_record_type: b.sys_record_type as u8,
            raw_bits: b.raw_bits.clone(),
            raw_bit_len: b.raw_bit_len as usize,
        },
    };
    Ok(prl_ext::ExtSystemRecord {
        sys_record_length: s.sys_record_length as u8,
        sys_record_type,
        pref_neg: proto_to_pref_neg(s.pref_neg)?,
        same_geo_as_prev: s.same_geo_as_prev,
        priority: proto_to_prio(s.priority)?,
        acq_index: s.acq_index as u16,
        system_id,
        roaming_indicator: s.roaming_indicator.as_ref().map(|r| proto_to_roam(Some(r))),
        association: s
            .association
            .as_ref()
            .map(|a| prl_ext::ExtSystemAssociation {
                association_tag: a.association_tag as u8,
                pn_association: a.pn_association,
                data_association: a.data_association,
            }),
    })
}

fn mccmnc_to_proto(sub: &prl_ext::MccMncSubtype) -> proto::PrlExtSysIdMccMnc {
    use proto::prl_ext_sys_id_mcc_mnc::Subtype as S;
    let subtype = match sub {
        prl_ext::MccMncSubtype::Subtype000 { mcc_bcd, mnc_bcd } => {
            S::Subtype000(proto::PrlMccMnc000 {
                mcc: bcd_to_mcc_string(*mcc_bcd),
                mnc: bcd_to_mnc_string(*mnc_bcd),
            })
        }
        prl_ext::MccMncSubtype::Subtype001 {
            mcc_bcd,
            mnc_bcd,
            sids,
        } => S::Subtype001(proto::PrlMccMnc001 {
            mcc: bcd_to_mcc_string(*mcc_bcd),
            mnc: bcd_to_mnc_string(*mnc_bcd),
            sids: sids.iter().map(|v| *v as u32).collect(),
        }),
        prl_ext::MccMncSubtype::Subtype010 {
            mcc_bcd,
            mnc_bcd,
            pairs,
        } => S::Subtype010(proto::PrlMccMnc010 {
            mcc: bcd_to_mcc_string(*mcc_bcd),
            mnc: bcd_to_mnc_string(*mnc_bcd),
            pairs: pairs
                .iter()
                .map(|p| proto::PrlSidNidPair {
                    sid: p.sid as u32,
                    nid: p.nid as u32,
                })
                .collect(),
        }),
        prl_ext::MccMncSubtype::Subtype011 {
            mcc_bcd,
            mnc_bcd,
            subnets,
        } => S::Subtype011(proto::PrlMccMnc011 {
            mcc: bcd_to_mcc_string(*mcc_bcd),
            mnc: bcd_to_mnc_string(*mnc_bcd),
            subnets: subnets
                .iter()
                .map(|s| proto::PrlMccMncSubnet {
                    subnet_length_bits: s.subnet_length as u32,
                    subnet_id_hex: bytes_to_hex(&s.subnet_id),
                })
                .collect(),
        }),
        prl_ext::MccMncSubtype::Reserved {
            subtype,
            raw_bits,
            raw_bit_len,
        } => S::Reserved(proto::PrlMccMncReserved {
            subtype: *subtype as u32,
            raw_bits: raw_bits.clone(),
            raw_bit_len: *raw_bit_len as u32,
        }),
    };
    proto::PrlExtSysIdMccMnc {
        subtype: Some(subtype),
    }
}

fn proto_to_mccmnc(
    m: &proto::PrlExtSysIdMccMnc,
) -> Result<prl_ext::MccMncSubtype, PrlValidationFailure> {
    use proto::prl_ext_sys_id_mcc_mnc::Subtype as S;
    Ok(
        match m.subtype.as_ref().ok_or_else(|| {
            PrlValidationFailure::DecodeFailed("PrlExtSysIdMccMnc.subtype missing".into())
        })? {
            S::Subtype000(b) => prl_ext::MccMncSubtype::Subtype000 {
                mcc_bcd: mcc_string_to_bcd(&b.mcc)?,
                mnc_bcd: mnc_string_to_bcd(&b.mnc)?,
            },
            S::Subtype001(b) => prl_ext::MccMncSubtype::Subtype001 {
                mcc_bcd: mcc_string_to_bcd(&b.mcc)?,
                mnc_bcd: mnc_string_to_bcd(&b.mnc)?,
                sids: b.sids.iter().map(|v| *v as u16).collect(),
            },
            S::Subtype010(b) => prl_ext::MccMncSubtype::Subtype010 {
                mcc_bcd: mcc_string_to_bcd(&b.mcc)?,
                mnc_bcd: mnc_string_to_bcd(&b.mnc)?,
                pairs: b
                    .pairs
                    .iter()
                    .map(|p| prl_ext::SidNidPair {
                        sid: p.sid as u16,
                        nid: p.nid as u16,
                    })
                    .collect(),
            },
            S::Subtype011(b) => prl_ext::MccMncSubtype::Subtype011 {
                mcc_bcd: mcc_string_to_bcd(&b.mcc)?,
                mnc_bcd: mnc_string_to_bcd(&b.mnc)?,
                subnets: b
                    .subnets
                    .iter()
                    .map(|s| {
                        let expected = s.subnet_length_bits.div_ceil(8) as usize;
                        let subnet_id = hex_to_bytes(&s.subnet_id_hex, expected)?;
                        Ok(prl_ext::MccMncSubnet {
                            subnet_length: s.subnet_length_bits as u8,
                            subnet_id,
                        })
                    })
                    .collect::<Result<_, _>>()?,
            },
            S::Reserved(b) => prl_ext::MccMncSubtype::Reserved {
                subtype: b.subtype as u8,
                raw_bits: b.raw_bits.clone(),
                raw_bit_len: b.raw_bit_len as usize,
            },
        },
    )
}

// ─── Enum helpers ───────────────────────────────────────────────────

fn roam_to_proto(r: prl::RoamingIndicator) -> proto::PrlRoamingIndicator {
    use proto::PrlRoamingIndicatorKind as K;
    let kind = match r {
        prl::RoamingIndicator::OnHome => K::OnHome,
        prl::RoamingIndicator::Roaming => K::Roaming,
        prl::RoamingIndicator::InternationalRoaming => K::International,
        prl::RoamingIndicator::Lte => K::Lte,
        prl::RoamingIndicator::Flashing => K::Flashing,
        prl::RoamingIndicator::Other(_) => K::Other,
    };
    proto::PrlRoamingIndicator {
        raw: r.raw() as u32,
        kind: kind as i32,
    }
}

fn proto_to_roam(p: Option<&proto::PrlRoamingIndicator>) -> prl::RoamingIndicator {
    match p {
        Some(p) => prl::RoamingIndicator::from_u8(p.raw as u8),
        None => prl::RoamingIndicator::OnHome,
    }
}

fn ab_to_proto(a: prl::AbSelection) -> i32 {
    use proto::PrlAbSelection as A;
    match a {
        prl::AbSelection::SystemA => A::SystemA as i32,
        prl::AbSelection::SystemB => A::SystemB as i32,
        prl::AbSelection::Reserved => A::Reserved as i32,
        prl::AbSelection::EitherAOrB => A::Either as i32,
    }
}
fn proto_to_ab(v: i32) -> Result<prl::AbSelection, PrlValidationFailure> {
    use proto::PrlAbSelection as A;
    Ok(match A::try_from(v).unwrap_or(A::Unspecified) {
        A::SystemA => prl::AbSelection::SystemA,
        A::SystemB => prl::AbSelection::SystemB,
        A::Reserved => prl::AbSelection::Reserved,
        A::Either => prl::AbSelection::EitherAOrB,
        A::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlAbSelection UNSPECIFIED".into(),
            ));
        }
    })
}

fn std_chan_to_proto(s: prl::StandardChannelSelection) -> i32 {
    use proto::PrlStandardChannel as S;
    match s {
        prl::StandardChannelSelection::Reserved => S::Reserved as i32,
        prl::StandardChannelSelection::Primary => S::Primary as i32,
        prl::StandardChannelSelection::Secondary => S::Secondary as i32,
        prl::StandardChannelSelection::PrimaryOrSecondary => S::PrimaryOrSecondary as i32,
    }
}
fn proto_to_std_chan(v: i32) -> Result<prl::StandardChannelSelection, PrlValidationFailure> {
    use proto::PrlStandardChannel as S;
    Ok(match S::try_from(v).unwrap_or(S::Unspecified) {
        S::Reserved => prl::StandardChannelSelection::Reserved,
        S::Primary => prl::StandardChannelSelection::Primary,
        S::Secondary => prl::StandardChannelSelection::Secondary,
        S::PrimaryOrSecondary => prl::StandardChannelSelection::PrimaryOrSecondary,
        S::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlStandardChannel UNSPECIFIED".into(),
            ));
        }
    })
}

fn pcs_to_proto(b: prl::PcsBlock) -> i32 {
    use proto::PrlPcsBlock as P;
    match b {
        prl::PcsBlock::A => P::A as i32,
        prl::PcsBlock::B => P::B as i32,
        prl::PcsBlock::C => P::C as i32,
        prl::PcsBlock::D => P::D as i32,
        prl::PcsBlock::E => P::E as i32,
        prl::PcsBlock::F => P::F as i32,
        prl::PcsBlock::Reserved => P::Reserved as i32,
        prl::PcsBlock::AnyBlock => P::Any as i32,
    }
}
fn proto_to_pcs(v: i32) -> Result<prl::PcsBlock, PrlValidationFailure> {
    use proto::PrlPcsBlock as P;
    Ok(match P::try_from(v).unwrap_or(P::Unspecified) {
        P::A => prl::PcsBlock::A,
        P::B => prl::PcsBlock::B,
        P::C => prl::PcsBlock::C,
        P::D => prl::PcsBlock::D,
        P::E => prl::PcsBlock::E,
        P::F => prl::PcsBlock::F,
        P::Reserved => prl::PcsBlock::Reserved,
        P::Any => prl::PcsBlock::AnyBlock,
        P::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlPcsBlock UNSPECIFIED".into(),
            ));
        }
    })
}

fn nid_incl_to_proto(n: prl::NidInclusion) -> i32 {
    use proto::PrlNidInclusion as N;
    match n {
        prl::NidInclusion::AnyNid => N::Any as i32,
        prl::NidInclusion::SingleNid => N::Single as i32,
        prl::NidInclusion::PublicNid => N::Public as i32,
        prl::NidInclusion::Reserved => N::Reserved as i32,
    }
}
fn proto_to_nid_incl(v: i32) -> Result<prl::NidInclusion, PrlValidationFailure> {
    use proto::PrlNidInclusion as N;
    Ok(match N::try_from(v).unwrap_or(N::Unspecified) {
        N::Any => prl::NidInclusion::AnyNid,
        N::Single => prl::NidInclusion::SingleNid,
        N::Public => prl::NidInclusion::PublicNid,
        N::Reserved => prl::NidInclusion::Reserved,
        N::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlNidInclusion UNSPECIFIED".into(),
            ));
        }
    })
}

fn pref_neg_to_proto(p: prl::PrefNeg) -> i32 {
    use proto::PrlPrefNeg as P;
    match p {
        prl::PrefNeg::Preferred => P::Preferred as i32,
        prl::PrefNeg::Negative => P::Negative as i32,
    }
}
fn proto_to_pref_neg(v: i32) -> Result<prl::PrefNeg, PrlValidationFailure> {
    use proto::PrlPrefNeg as P;
    Ok(match P::try_from(v).unwrap_or(P::Unspecified) {
        P::Preferred => prl::PrefNeg::Preferred,
        P::Negative => prl::PrefNeg::Negative,
        P::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlPrefNeg UNSPECIFIED".into(),
            ));
        }
    })
}

fn prio_to_proto(p: prl::Priority) -> i32 {
    use proto::PrlPriority as P;
    match p {
        prl::Priority::MoreDesirable => P::MoreDesirable as i32,
        prl::Priority::EquallyDesirable => P::EquallyDesirable as i32,
    }
}
fn proto_to_prio(v: i32) -> Result<prl::Priority, PrlValidationFailure> {
    use proto::PrlPriority as P;
    Ok(match P::try_from(v).unwrap_or(P::Unspecified) {
        P::MoreDesirable => prl::Priority::MoreDesirable,
        P::EquallyDesirable => prl::Priority::EquallyDesirable,
        P::Unspecified => {
            return Err(PrlValidationFailure::DecodeFailed(
                "PrlPriority UNSPECIFIED".into(),
            ));
        }
    })
}

// ─── MCC / MNC BCD helpers ──────────────────────────────────────────
//
// MCC is always 3 BCD digits packed in 12 bits (e.g. MCC 310 → 0x310).
// MNC is 2 or 3 BCD digits in 12 bits; 2-digit MNCs pad the LSB nibble
// with 0xF per §3.5.5.3.2.2 (e.g. MNC 23 → 0x23F).
//
// The proto carries the operator-friendly decimal strings; these
// helpers do the BCD round-trip so the UI never has to know about
// the wire encoding.

const MNC_F_PAD_NIBBLE: u16 = 0xF;

fn bcd_to_mcc_string(bcd: u16) -> String {
    let d1 = (bcd >> 8) & 0xF;
    let d2 = (bcd >> 4) & 0xF;
    let d3 = bcd & 0xF;
    format!("{}{}{}", d1, d2, d3)
}

fn bcd_to_mnc_string(bcd: u16) -> String {
    let d1 = (bcd >> 8) & 0xF;
    let d2 = (bcd >> 4) & 0xF;
    let d3 = bcd & 0xF;
    if d3 == MNC_F_PAD_NIBBLE {
        format!("{}{}", d1, d2)
    } else {
        format!("{}{}{}", d1, d2, d3)
    }
}

fn mcc_string_to_bcd(s: &str) -> Result<u16, PrlValidationFailure> {
    let trimmed = s.trim();
    if trimmed.len() != 3 || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Err(PrlValidationFailure::DecodeFailed(format!(
            "MCC must be 3 decimal digits (got {:?})",
            s
        )));
    }
    let digits: Vec<u16> = trimmed
        .chars()
        .map(|c| c.to_digit(10).unwrap() as u16)
        .collect();
    Ok((digits[0] << 8) | (digits[1] << 4) | digits[2])
}

fn mnc_string_to_bcd(s: &str) -> Result<u16, PrlValidationFailure> {
    let trimmed = s.trim();
    let valid_digits = trimmed.chars().all(|c| c.is_ascii_digit());
    match (trimmed.len(), valid_digits) {
        (2, true) => {
            let mut chars = trimmed.chars();
            let d1 = chars.next().unwrap().to_digit(10).unwrap() as u16;
            let d2 = chars.next().unwrap().to_digit(10).unwrap() as u16;
            Ok((d1 << 8) | (d2 << 4) | MNC_F_PAD_NIBBLE)
        }
        (3, true) => {
            let digits: Vec<u16> = trimmed
                .chars()
                .map(|c| c.to_digit(10).unwrap() as u16)
                .collect();
            Ok((digits[0] << 8) | (digits[1] << 4) | digits[2])
        }
        _ => Err(PrlValidationFailure::DecodeFailed(format!(
            "MNC must be 2 or 3 decimal digits (got {:?})",
            s
        ))),
    }
}

// ─── Hex byte string helpers ────────────────────────────────────────

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02X}", b));
    }
    out
}

fn hex_to_bytes(hex: &str, expected_len: usize) -> Result<Vec<u8>, PrlValidationFailure> {
    let trimmed: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if trimmed.len() != expected_len * 2 {
        return Err(PrlValidationFailure::DecodeFailed(format!(
            "hex string has {} chars; expected {} (for {} octets)",
            trimmed.len(),
            expected_len * 2,
            expected_len
        )));
    }
    let mut out = Vec::with_capacity(expected_len);
    for i in 0..expected_len {
        let pair = &trimmed[i * 2..i * 2 + 2];
        out.push(u8::from_str_radix(pair, 16).map_err(|e| {
            PrlValidationFailure::DecodeFailed(format!("invalid hex pair {:?}: {}", pair, e))
        })?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcd_mcc_round_trip() {
        assert_eq!(bcd_to_mcc_string(0x310), "310");
        assert_eq!(mcc_string_to_bcd("310").unwrap(), 0x310);
        assert_eq!(bcd_to_mcc_string(0x001), "001");
        assert_eq!(mcc_string_to_bcd("001").unwrap(), 0x001);
    }

    #[test]
    fn bcd_mnc_2_digit_round_trip() {
        // MNC 23 (Verizon historically) round-trips as 0x23F.
        assert_eq!(bcd_to_mnc_string(0x23F), "23");
        assert_eq!(mnc_string_to_bcd("23").unwrap(), 0x23F);
    }

    #[test]
    fn bcd_mnc_3_digit_round_trip() {
        // MNC 410 (T-Mobile US) — 3-digit, no F padding.
        assert_eq!(bcd_to_mnc_string(0x410), "410");
        assert_eq!(mnc_string_to_bcd("410").unwrap(), 0x410);
    }

    #[test]
    fn mcc_rejects_non_three_digit() {
        assert!(mcc_string_to_bcd("31").is_err());
        assert!(mcc_string_to_bcd("3100").is_err());
        assert!(mcc_string_to_bcd("ab1").is_err());
    }

    #[test]
    fn mnc_rejects_bad_length() {
        assert!(mnc_string_to_bcd("2").is_err());
        assert!(mnc_string_to_bcd("4100").is_err());
        assert!(mnc_string_to_bcd("ab").is_err());
    }

    #[test]
    fn hex_round_trip() {
        let bytes = vec![0xCA, 0xFE, 0x01];
        assert_eq!(bytes_to_hex(&bytes), "CAFE01");
        assert_eq!(hex_to_bytes("CAFE01", 3).unwrap(), bytes);
        assert_eq!(hex_to_bytes("ca fe 01", 3).unwrap(), bytes);
    }

    #[test]
    fn hex_rejects_wrong_length() {
        assert!(hex_to_bytes("CAFE", 3).is_err());
        assert!(hex_to_bytes("CAFE01FF", 3).is_err());
        assert!(hex_to_bytes("CAGE01", 3).is_err()); // 'G' not hex
    }
}
