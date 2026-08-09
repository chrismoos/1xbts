//! Rust -> proto conversions for OTASP events and session records.
//!
//! Keeps proto detail out of the coordinator/history modules so the core OTASP
//! types can stay free of `tonic`/`prost` dependencies.

use crate::grpc::events_proto::v1 as events_proto;
use crate::otasp::event::{
    BlockFeature, HardwareIdentity, OtaspEvent, PrlOutcome, PrlReadback, ScmDualMode, ScmExtended,
    ScmMeidSupport, ScmSlottedClass, ScmTransmission, SessionOutcomeKind, StationClassMark,
};
use cdma_otasp::param::prl::{
    AbSelection, AcquisitionBody, AcquisitionRecord, ClassicPrl, NidInclusion, PcsBlock, PrefNeg,
    Priority, RoamingIndicator, StandardChannelSelection, SystemRecord,
};
use cdma_otasp::param::prl_ext::ExtendedPrl;

fn proto_hwid(d: &HardwareIdentity) -> events_proto::OtaspHardwareIdentity {
    events_proto::OtaspHardwareIdentity {
        esn: d.esn.unwrap_or(0),
        meid: d.meid.clone().unwrap_or_default(),
    }
}

fn proto_scm(scm: &StationClassMark) -> events_proto::OtaspStationClassMark {
    events_proto::OtaspStationClassMark {
        raw: scm.raw as u32,
        extended: match scm.extended {
            ScmExtended::StandardBands => events_proto::OtaspScmExtended::StandardBands.into(),
            ScmExtended::PcsFamily => events_proto::OtaspScmExtended::PcsFamily.into(),
        },
        dual_mode: match scm.dual_mode {
            ScmDualMode::CdmaOnly => events_proto::OtaspScmDualMode::CdmaOnly.into(),
            ScmDualMode::Dual => events_proto::OtaspScmDualMode::Dual.into(),
        },
        slotted_class: match scm.slotted_class {
            ScmSlottedClass::NonSlotted => events_proto::OtaspScmSlottedClass::NonSlotted.into(),
            ScmSlottedClass::Slotted => events_proto::OtaspScmSlottedClass::Slotted.into(),
        },
        meid_support: match scm.meid_support {
            ScmMeidSupport::NotConfigured => {
                events_proto::OtaspScmMeidSupport::NotConfigured.into()
            }
            ScmMeidSupport::Configured => events_proto::OtaspScmMeidSupport::Configured.into(),
        },
        bandwidth_25mhz: scm.bandwidth_25mhz,
        transmission: match scm.transmission {
            ScmTransmission::Continuous => events_proto::OtaspScmTransmission::Continuous.into(),
            ScmTransmission::Discontinuous => {
                events_proto::OtaspScmTransmission::Discontinuous.into()
            }
        },
        analog_power_class: scm.analog_power_class as u32,
    }
}

pub fn to_proto_event(ev: &OtaspEvent) -> events_proto::OtaspEvent {
    use events_proto::otasp_event::Kind;
    let kind = match ev {
        OtaspEvent::SessionStart {
            device,
            feature_code,
            service_option,
        } => Kind::SessionStart(events_proto::OtaspSessionStart {
            device: Some(proto_hwid(device)),
            feature_code: feature_code.clone(),
            service_option: *service_option as u32,
        }),
        OtaspEvent::ProtocolCapabilityReceived {
            mob_firm_rev,
            mob_model,
            band_mode_cap,
            otasp_p_rev,
            features,
        } => Kind::ProtocolCapabilityReceived(events_proto::OtaspProtocolCapabilityReceived {
            mob_firm_rev: *mob_firm_rev as u32,
            mob_model: *mob_model as u32,
            band_mode_cap: Some(events_proto::OtaspBandModeCap {
                raw: band_mode_cap.raw as u32,
                band_class_0_analog: band_mode_cap.band_class_0_analog,
                band_class_0_cdma: band_mode_cap.band_class_0_cdma,
                band_class_1_cdma: band_mode_cap.band_class_1_cdma,
                band_class_3_cdma: band_mode_cap.band_class_3_cdma,
                band_class_6_cdma: band_mode_cap.band_class_6_cdma,
                reserved: band_mode_cap.reserved as u32,
            }),
            otasp_p_rev: otasp_p_rev.map(|v| v as u32),
            features: features
                .iter()
                .map(|(id, rev)| events_proto::OtaspFeatureCapability {
                    feature_id: *id as i32,
                    feature_p_rev: *rev as u32,
                    feature_id_raw: *id as u32,
                })
                .collect(),
        }),
        OtaspEvent::SpcMismatch => Kind::SpcMismatch(events_proto::OtaspSpcMismatch {}),
        OtaspEvent::HlrMiss { device } => Kind::HlrMiss(events_proto::OtaspHlrMiss {
            device: Some(proto_hwid(device)),
        }),
        OtaspEvent::NoNamCapacity { block_id, feature } => {
            Kind::NoNamCapacity(events_proto::OtaspNoNamCapacity {
                block_id: *block_id as u32,
                feature: to_proto_block_feature(*feature).into(),
            })
        }
        OtaspEvent::BlockDownloaded {
            block_id,
            result_code,
            feature,
            fields,
        } => Kind::BlockDownloaded(events_proto::OtaspBlockDownloaded {
            block_id: *block_id as u32,
            result_code: *result_code as u32,
            feature: to_proto_block_feature(*feature).into(),
            fields: fields
                .iter()
                .map(|(n, v)| events_proto::OtaspNamReadbackField {
                    name: n.clone(),
                    value: v.clone(),
                })
                .collect(),
        }),
        OtaspEvent::BlockRejected {
            block_id,
            result_code,
            feature,
        } => Kind::BlockRejected(events_proto::OtaspBlockRejected {
            block_id: *block_id as u32,
            result_code: *result_code as u32,
            feature: to_proto_block_feature(*feature).into(),
        }),
        OtaspEvent::CommitResult { result_code } => {
            Kind::CommitResult(events_proto::OtaspCommitResult {
                result_code: *result_code as u32,
            })
        }
        OtaspEvent::SessionEnded {
            completed_blocks,
            outcome,
        } => Kind::SessionEnded(events_proto::OtaspSessionEnded {
            completed_blocks: *completed_blocks,
            outcome: to_proto_outcome(*outcome).into(),
        }),
        OtaspEvent::SpcVerified => Kind::SpcVerified(events_proto::OtaspSpcVerified {}),
        OtaspEvent::BlockSkipped {
            block_id,
            reason,
            feature,
        } => Kind::BlockSkipped(events_proto::OtaspBlockSkipped {
            block_id: *block_id as u32,
            reason: reason.clone(),
            feature: to_proto_block_feature(*feature).into(),
        }),
        OtaspEvent::Timeout { phase } => Kind::Timeout(events_proto::OtaspTimeout {
            phase: phase.clone(),
        }),
        OtaspEvent::NamReadback {
            block_id,
            label,
            fields,
            feature,
        } => Kind::NamReadback(events_proto::OtaspNamReadback {
            block_id: *block_id as u32,
            label: label.clone(),
            fields: fields
                .iter()
                .map(|(name, value)| events_proto::OtaspNamReadbackField {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            feature: to_proto_block_feature(*feature).into(),
        }),
        OtaspEvent::StationClassMark(scm) => Kind::StationClassMark(proto_scm(scm)),
        OtaspEvent::PrlReadback(rb) => Kind::PrlReadback(proto_prl_readback(rb)),
    };
    events_proto::OtaspEvent { kind: Some(kind) }
}

fn proto_prl_readback(rb: &PrlReadback) -> events_proto::OtaspPrlReadback {
    use events_proto::otasp_prl_readback::Outcome as ProtoOutcome;
    let outcome = match &rb.outcome {
        PrlOutcome::Decoded(p) => ProtoOutcome::Decoded(proto_prl_decoded(p)),
        PrlOutcome::DecodedExtended { prl, raw_bytes } => {
            ProtoOutcome::DecodedExtended(proto_prl_decoded_extended(prl, raw_bytes.clone()))
        }
        PrlOutcome::Absent => ProtoOutcome::Absent(events_proto::OtaspPrlAbsent {}),
        PrlOutcome::FeatureNotAdvertised => {
            ProtoOutcome::FeatureNotAdvertised(events_proto::OtaspPrlFeatureNotAdvertised {})
        }
        PrlOutcome::Rejected {
            block_id,
            result_code,
        } => ProtoOutcome::Rejected(events_proto::OtaspPrlRejected {
            block_id: *block_id as u32,
            result_code: *result_code as u32,
        }),
        PrlOutcome::DecodeFailed { reason, raw_bytes } => {
            ProtoOutcome::DecodeFailed(events_proto::OtaspPrlDecodeFailed {
                reason: reason.clone(),
                raw_bytes: raw_bytes.clone(),
            })
        }
    };
    events_proto::OtaspPrlReadback {
        max_pr_list_size: rb.max_pr_list_size as u32,
        cur_pr_list_size: rb.cur_pr_list_size as u32,
        pr_list_id: rb.pr_list_id.map(|v| v as u32),
        segment_count: rb.segment_count,
        outcome: Some(outcome),
    }
}

fn proto_prl_decoded(p: &ClassicPrl) -> events_proto::OtaspPrlDecoded {
    // Re-encode for the download bytes. `encode()` is round-trip
    // tested against every real-carrier fixture so the output equals
    // what the MS sent on the air.
    let raw_bytes = p.encode().unwrap_or_default();
    events_proto::OtaspPrlDecoded {
        pr_list_size: p.pr_list_size as u32,
        pr_list_id: p.pr_list_id as u32,
        pref_only: p.pref_only,
        def_roam_ind: Some(proto_roam(p.def_roam_ind)),
        pr_list_crc: p.pr_list_crc as u32,
        computed_crc: p.computed_crc as u32,
        crc_ok: p.crc_ok(),
        acquisition_records: p.acquisition_records.iter().map(proto_acq).collect(),
        system_records: p.system_records.iter().map(proto_sys).collect(),
        raw_bytes,
    }
}

fn proto_prl_decoded_extended(
    p: &ExtendedPrl,
    raw_bytes: Vec<u8>,
) -> events_proto::OtaspPrlDecodedExtended {
    events_proto::OtaspPrlDecodedExtended {
        pr_list_size: p.pr_list_size as u32,
        pr_list_id: p.pr_list_id as u32,
        cur_sspr_p_rev: p.cur_sspr_p_rev as u32,
        pref_only: p.pref_only,
        def_roam_ind: Some(proto_roam(p.def_roam_ind)),
        pr_list_crc: p.pr_list_crc as u32,
        computed_crc: p.computed_crc as u32,
        crc_ok: p.crc_ok(),
        num_acq_records: p.acquisition_records.len() as u32,
        num_common_subnet_records: p.common_subnet_records.len() as u32,
        num_ext_sys_records: p.system_records.len() as u32,
        raw_bytes,
    }
}

fn proto_roam(r: RoamingIndicator) -> events_proto::OtaspPrlRoamingIndicator {
    use events_proto::OtaspPrlRoamingIndicatorKind as K;
    let kind = match r {
        RoamingIndicator::IndicatorOn => K::IndicatorOn,
        RoamingIndicator::IndicatorOff => K::IndicatorOff,
        RoamingIndicator::IndicatorFlashing => K::IndicatorFlashing,
        RoamingIndicator::OutOfNeighborhood => K::OutOfNeighborhood,
        RoamingIndicator::OutOfBuilding => K::OutOfBuilding,
        RoamingIndicator::PreferredSystem => K::PreferredSystem,
        RoamingIndicator::AvailableSystem => K::AvailableSystem,
        RoamingIndicator::AlliancePartner => K::AlliancePartner,
        RoamingIndicator::PremiumPartner => K::PremiumPartner,
        RoamingIndicator::FullService => K::FullService,
        RoamingIndicator::PartialService => K::PartialService,
        RoamingIndicator::BannerOn => K::BannerOn,
        RoamingIndicator::BannerOff => K::BannerOff,
        RoamingIndicator::Other(_) => K::Other,
    };
    events_proto::OtaspPrlRoamingIndicator {
        raw: r.raw() as u32,
        kind: kind.into(),
    }
}

fn proto_ab(a: AbSelection) -> i32 {
    use events_proto::OtaspPrlAbSelection as A;
    match a {
        AbSelection::SystemA => A::SystemA,
        AbSelection::SystemB => A::SystemB,
        AbSelection::Reserved => A::Reserved,
        AbSelection::EitherAOrB => A::Either,
    }
    .into()
}

fn proto_std_chan(s: StandardChannelSelection) -> i32 {
    use events_proto::OtaspPrlStandardChannel as S;
    match s {
        StandardChannelSelection::Reserved => S::Reserved,
        StandardChannelSelection::Primary => S::Primary,
        StandardChannelSelection::Secondary => S::Secondary,
        StandardChannelSelection::PrimaryOrSecondary => S::PrimaryOrSecondary,
    }
    .into()
}

fn proto_pcs_block(b: PcsBlock) -> i32 {
    use events_proto::OtaspPrlPcsBlock as B;
    match b {
        PcsBlock::A => B::A,
        PcsBlock::B => B::B,
        PcsBlock::C => B::C,
        PcsBlock::D => B::D,
        PcsBlock::E => B::E,
        PcsBlock::F => B::F,
        PcsBlock::Reserved => B::Reserved,
        PcsBlock::AnyBlock => B::Any,
    }
    .into()
}

fn proto_acq(r: &AcquisitionRecord) -> events_proto::OtaspPrlAcqRecord {
    use events_proto::otasp_prl_acq_record::Body as B;
    let body = match &r.body {
        AcquisitionBody::CellularAnalog { ab } => {
            B::CellularAnalog(events_proto::OtaspPrlAcqCellularAnalog { ab: proto_ab(*ab) })
        }
        AcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
            B::CellularCdmaStandard(events_proto::OtaspPrlAcqCellularCdmaStandard {
                ab: proto_ab(*ab),
                pri_sec: proto_std_chan(*pri_sec),
            })
        }
        AcquisitionBody::CellularCdmaCustom { channels } => {
            B::CellularCdmaCustom(events_proto::OtaspPrlAcqCellularCdmaCustom {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        AcquisitionBody::CellularCdmaPreferred { ab } => {
            B::CellularCdmaPreferred(events_proto::OtaspPrlAcqCellularCdmaPreferred {
                ab: proto_ab(*ab),
            })
        }
        AcquisitionBody::PcsCdmaUsingBlocks { blocks } => {
            B::PcsCdmaUsingBlocks(events_proto::OtaspPrlAcqPcsCdmaUsingBlocks {
                blocks: blocks.iter().map(|b| proto_pcs_block(*b)).collect(),
            })
        }
        AcquisitionBody::PcsCdmaUsingChannels { channels } => {
            B::PcsCdmaUsingChannels(events_proto::OtaspPrlAcqPcsCdmaUsingChannels {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        AcquisitionBody::JtacsCdmaStandard { ab, pri_sec } => {
            B::JtacsCdmaStandard(events_proto::OtaspPrlAcqJtacsCdmaStandard {
                ab: proto_ab(*ab),
                pri_sec: proto_std_chan(*pri_sec),
            })
        }
        AcquisitionBody::JtacsCdmaCustom { channels } => {
            B::JtacsCdmaCustom(events_proto::OtaspPrlAcqJtacsCdmaCustom {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        AcquisitionBody::BandClass6UsingChannels { channels } => {
            B::BandClass6UsingChannels(events_proto::OtaspPrlAcqBandClass6UsingChannels {
                channels: channels.iter().map(|c| *c as u32).collect(),
            })
        }
        AcquisitionBody::Unknown => B::Unknown(events_proto::OtaspPrlAcqUnknown {}),
    };
    events_proto::OtaspPrlAcqRecord {
        acq_type_raw: r.acq_type_raw as u32,
        body: Some(body),
    }
}

fn proto_sys(s: &SystemRecord) -> events_proto::OtaspPrlSysRecord {
    use events_proto::OtaspPrlNidInclusion as N;
    use events_proto::OtaspPrlPrefNeg as P;
    use events_proto::OtaspPrlPriority as Pr;
    events_proto::OtaspPrlSysRecord {
        sid: s.sid as u32,
        nid_incl: match s.nid_incl {
            NidInclusion::AnyNid => N::Any,
            NidInclusion::SingleNid => N::Single,
            NidInclusion::PublicNid => N::Public,
            NidInclusion::Reserved => N::Reserved,
        }
        .into(),
        nid: s.nid.map(|v| v as u32),
        same_geo_as_prev: s.same_geo_as_prev,
        pref_neg: match s.pref_neg {
            PrefNeg::Preferred => P::Preferred,
            PrefNeg::Negative => P::Negative,
        }
        .into(),
        acq_index: s.acq_index as u32,
        roaming_indicator: s.roaming_indicator.map(proto_roam),
        priority: s.priority.map(|p| {
            match p {
                Priority::MoreDesirable => Pr::MoreDesirable,
                Priority::EquallyDesirable => Pr::EquallyDesirable,
            }
            .into()
        }),
    }
}

pub fn to_proto_block_feature(f: BlockFeature) -> events_proto::OtaspBlockFeature {
    match f {
        BlockFeature::Nam => events_proto::OtaspBlockFeature::Nam,
        BlockFeature::SystemTag => events_proto::OtaspBlockFeature::SystemTag,
        BlockFeature::MmsUri => events_proto::OtaspBlockFeature::MmsUri,
        BlockFeature::Prl => events_proto::OtaspBlockFeature::Prl,
    }
}

pub fn to_proto_outcome(outcome: SessionOutcomeKind) -> events_proto::OtaspSessionOutcome {
    match outcome {
        SessionOutcomeKind::Committed => events_proto::OtaspSessionOutcome::Committed,
        SessionOutcomeKind::NothingToCommit => events_proto::OtaspSessionOutcome::NothingToCommit,
        SessionOutcomeKind::SpcRejected => events_proto::OtaspSessionOutcome::SpcRejected,
        SessionOutcomeKind::HlrUnknown => events_proto::OtaspSessionOutcome::HlrUnknown,
        SessionOutcomeKind::Rejected => events_proto::OtaspSessionOutcome::Rejected,
        SessionOutcomeKind::NoCapacity => events_proto::OtaspSessionOutcome::NoCapacity,
        SessionOutcomeKind::ProtocolError => events_proto::OtaspSessionOutcome::ProtocolError,
        SessionOutcomeKind::TimedOut => events_proto::OtaspSessionOutcome::TimedOut,
    }
}

pub fn to_proto_msc_event(ev: &OtaspEvent) -> events_proto::MscNetworkEvent {
    events_proto::MscNetworkEvent {
        event: Some(events_proto::msc_network_event::Event::Otasp(
            to_proto_event(ev),
        )),
    }
}
