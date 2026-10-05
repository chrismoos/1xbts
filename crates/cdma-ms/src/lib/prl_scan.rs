//! PRL channel selection (C.S0016 §3.5.5).

use std::collections::HashSet;
use std::path::Path;

use cdma_common::band_class::{BandClass, ChannelPlan};
use cdma_common::error::Error;
use cdma_otasp::param::prl::{
    self, AbSelection, AcquisitionBody, NidInclusion, PcsBlock, PrefNeg, Priority,
    RoamingIndicator, StandardChannelSelection,
};
use cdma_otasp::param::prl_ext::{self, ExtAcquisitionBody, ExtSystemId};

/// BC0 CDMA primary and secondary channels for the A and B cellular
/// carriers (C.S0057 §2.1.1).
pub const BC0_A_PRIMARY: u16 = 283;
pub const BC0_A_SECONDARY: u16 = 691;
pub const BC0_B_PRIMARY: u16 = 384;
pub const BC0_B_SECONDARY: u16 = 777;

/// BC1 preferred CDMA channels per PCS block (C.S0057 §2.1.2).
const BC1_BLOCK_A: &[u16] = &[25, 50, 75, 100, 125, 150, 175, 200, 225, 250, 275];
const BC1_BLOCK_D: &[u16] = &[325, 350, 375];
const BC1_BLOCK_B: &[u16] = &[425, 450, 475, 500, 525, 550, 575, 600, 625, 650, 675];
const BC1_BLOCK_E: &[u16] = &[725, 750, 775];
const BC1_BLOCK_F: &[u16] = &[825, 850, 875];
const BC1_BLOCK_C: &[u16] = &[
    925, 950, 975, 1000, 1025, 1050, 1075, 1100, 1125, 1150, 1175,
];

/// Extended PRL SSPR_P_REV that `prl_ext` decodes.
const EXTENDED_SSPR_P_REV: u8 = 3;

const GENERIC_BAND_CLASS_BC0: u8 = 0;
const GENERIC_BAND_CLASS_BC1: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrlFormat {
    Classic,
    Extended,
}

impl PrlFormat {
    pub fn label(self) -> &'static str {
        match self {
            PrlFormat::Classic => "classic",
            PrlFormat::Extended => "extended",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanChannel {
    pub band_class: BandClass,
    pub channel: u16,
    pub frequency_hz: f64,
    pub acq_index: u16,
}

impl ScanChannel {
    pub fn reverse_hz(&self) -> f64 {
        ChannelPlan::new(self.band_class, 0, self.channel).uplink_hz() as f64
    }

    pub fn label(&self) -> String {
        format!(
            "{} ch{} ({:.3} MHz)",
            self.band_class.as_str().to_ascii_lowercase(),
            self.channel,
            self.frequency_hz / 1e6
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NidMatch {
    Any,
    Public,
    Exact(u16),
}

impl NidMatch {
    fn matches(self, nid: u16) -> bool {
        match self {
            NidMatch::Any => true,
            NidMatch::Public => nid == 0,
            NidMatch::Exact(n) => nid == n,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrlSystem {
    pub index: usize,
    pub sid: u16,
    pub nid: NidMatch,
    pub preferred: bool,
    /// GEO region number, counted from 1 in table order.
    pub geo: u16,
    pub acq_index: u16,
    pub roaming_indicator: Option<u8>,
    pub more_desirable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrlVerdict {
    Permitted {
        record: Option<usize>,
        geo: Option<u16>,
        roaming_indicator: Option<u8>,
    },
    Negative {
        record: usize,
    },
    Unlisted,
}

impl PrlVerdict {
    pub fn permitted(&self) -> bool {
        matches!(self, PrlVerdict::Permitted { .. })
    }

    pub fn reason(&self) -> &'static str {
        match self {
            PrlVerdict::Permitted {
                record: Some(_), ..
            } => "preferred",
            PrlVerdict::Permitted { record: None, .. } => "unlisted_allowed",
            PrlVerdict::Negative { .. } => "negative",
            PrlVerdict::Unlisted => "unlisted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PrlScanPlan {
    pub prl_id: u16,
    pub format: PrlFormat,
    pub pref_only: bool,
    pub default_roaming_indicator: u8,
    pub channels: Vec<ScanChannel>,
    pub systems: Vec<PrlSystem>,
    pub skipped_records: usize,
    pub invalid_channels: usize,
}

impl PrlScanPlan {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let bytes = std::fs::read(path)
            .map_err(|e| Error::from(format!("read PRL {}: {}", path.display(), e)))?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if prl_ext::sniff_sspr_p_rev(bytes) == EXTENDED_SSPR_P_REV {
            let ext = prl_ext::decode(bytes)?;
            Ok(Self::from_extended(&ext))
        } else {
            let classic = prl::decode(bytes)?;
            Ok(Self::from_classic(&classic))
        }
    }

    pub fn from_classic(p: &prl::ClassicPrl) -> Self {
        let mut expander = Expander::default();
        for (i, rec) in p.acquisition_records.iter().enumerate() {
            expander.classic_record(i as u16, &rec.body);
        }
        let mut systems = Vec::with_capacity(p.system_records.len());
        let mut geo = 0u16;
        for (index, s) in p.system_records.iter().enumerate() {
            if !s.same_geo_as_prev {
                geo += 1;
            }
            systems.push(PrlSystem {
                index,
                sid: s.sid,
                nid: nid_match(s.nid_incl, s.nid),
                preferred: s.pref_neg == PrefNeg::Preferred,
                geo,
                acq_index: s.acq_index,
                roaming_indicator: s.roaming_indicator.map(RoamingIndicator::raw),
                more_desirable: s.priority == Some(Priority::MoreDesirable),
            });
        }
        PrlScanPlan {
            prl_id: p.pr_list_id,
            format: PrlFormat::Classic,
            pref_only: p.pref_only,
            default_roaming_indicator: p.def_roam_ind.raw(),
            channels: expander.channels,
            systems,
            skipped_records: expander.skipped,
            invalid_channels: expander.invalid,
        }
    }

    pub fn from_extended(p: &prl_ext::ExtendedPrl) -> Self {
        let mut expander = Expander::default();
        for (i, rec) in p.acquisition_records.iter().enumerate() {
            expander.extended_record(i as u16, &rec.body);
        }
        let mut systems = Vec::new();
        let mut geo = 0u16;
        for (index, s) in p.system_records.iter().enumerate() {
            if !s.same_geo_as_prev {
                geo += 1;
            }
            let ExtSystemId::Cdma2000 { nid_incl, sid, nid } = &s.system_id else {
                continue;
            };
            systems.push(PrlSystem {
                index,
                sid: *sid,
                nid: nid_match(*nid_incl, *nid),
                preferred: s.pref_neg == PrefNeg::Preferred,
                geo,
                acq_index: s.acq_index,
                roaming_indicator: s.roaming_indicator.map(RoamingIndicator::raw),
                more_desirable: s.priority == Priority::MoreDesirable,
            });
        }
        PrlScanPlan {
            prl_id: p.pr_list_id,
            format: PrlFormat::Extended,
            pref_only: p.pref_only,
            default_roaming_indicator: p.def_roam_ind.raw(),
            channels: expander.channels,
            systems,
            skipped_records: expander.skipped,
            invalid_channels: expander.invalid,
        }
    }

    pub fn from_parts(
        prl_id: u16,
        pref_only: bool,
        channels: &[(BandClass, u16)],
        systems: Vec<PrlSystem>,
    ) -> Self {
        let mut expander = Expander::default();
        for (i, (bc, ch)) in channels.iter().enumerate() {
            expander.push(i as u16, *bc, *ch);
        }
        PrlScanPlan {
            prl_id,
            format: PrlFormat::Classic,
            pref_only,
            default_roaming_indicator: 0,
            channels: expander.channels,
            systems,
            skipped_records: 0,
            invalid_channels: expander.invalid,
        }
    }

    pub fn retain_band_classes(&mut self, allowed: &[BandClass]) {
        self.channels.retain(|c| allowed.contains(&c.band_class));
    }

    pub fn channels_in(&self, band_class: BandClass) -> usize {
        self.channels
            .iter()
            .filter(|c| c.band_class == band_class)
            .count()
    }

    /// The first system record matching `sid`/`nid` decides, in table order.
    pub fn verdict(&self, sid: u16, nid: u16) -> PrlVerdict {
        match self
            .systems
            .iter()
            .find(|s| s.sid == sid && s.nid.matches(nid))
        {
            Some(s) if s.preferred => PrlVerdict::Permitted {
                record: Some(s.index),
                geo: Some(s.geo),
                roaming_indicator: s.roaming_indicator,
            },
            Some(s) => PrlVerdict::Negative { record: s.index },
            None if self.pref_only => PrlVerdict::Unlisted,
            None => PrlVerdict::Permitted {
                record: None,
                geo: None,
                roaming_indicator: Some(self.default_roaming_indicator),
            },
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "prl id={} format={} pref_only={} systems={} scan_channels={} (bc0={} bc1={}) skipped_records={} invalid_channels={}",
            self.prl_id,
            self.format.label(),
            self.pref_only,
            self.systems.len(),
            self.channels.len(),
            self.channels_in(BandClass::Bc0),
            self.channels_in(BandClass::Bc1),
            self.skipped_records,
            self.invalid_channels,
        )
    }
}

fn nid_match(incl: NidInclusion, nid: Option<u16>) -> NidMatch {
    match (incl, nid) {
        (NidInclusion::SingleNid, Some(n)) => NidMatch::Exact(n),
        (NidInclusion::PublicNid, _) => NidMatch::Public,
        _ => NidMatch::Any,
    }
}

#[derive(Default)]
struct Expander {
    channels: Vec<ScanChannel>,
    seen: HashSet<(u8, u16)>,
    skipped: usize,
    invalid: usize,
}

impl Expander {
    fn push(&mut self, acq_index: u16, band_class: BandClass, channel: u16) {
        if !self.seen.insert((band_class.field_value(), channel)) {
            return;
        }
        let plan = ChannelPlan::new(band_class, 0, channel);
        if let Err(e) = plan.validate() {
            log::warn!(
                "ms_scan: acq#{} {} ch{} rejected by band-class validator: {}",
                acq_index,
                band_class.as_str(),
                channel,
                e
            );
            self.invalid += 1;
            return;
        }
        self.channels.push(ScanChannel {
            band_class,
            channel,
            frequency_hz: plan.downlink_hz() as f64,
            acq_index,
        });
    }

    fn cellular_standard(
        &mut self,
        acq_index: u16,
        ab: AbSelection,
        sel: StandardChannelSelection,
    ) {
        let carriers: &[(u16, u16)] = match ab {
            AbSelection::SystemA => &[(BC0_A_PRIMARY, BC0_A_SECONDARY)],
            AbSelection::SystemB => &[(BC0_B_PRIMARY, BC0_B_SECONDARY)],
            AbSelection::EitherAOrB | AbSelection::Reserved => &[
                (BC0_A_PRIMARY, BC0_A_SECONDARY),
                (BC0_B_PRIMARY, BC0_B_SECONDARY),
            ],
        };
        for (primary, secondary) in carriers {
            match sel {
                StandardChannelSelection::Primary => self.push(acq_index, BandClass::Bc0, *primary),
                StandardChannelSelection::Secondary => {
                    self.push(acq_index, BandClass::Bc0, *secondary)
                }
                StandardChannelSelection::PrimaryOrSecondary
                | StandardChannelSelection::Reserved => {
                    self.push(acq_index, BandClass::Bc0, *primary);
                    self.push(acq_index, BandClass::Bc0, *secondary);
                }
            }
        }
    }

    fn pcs_blocks(&mut self, acq_index: u16, blocks: &[PcsBlock]) {
        for block in blocks {
            let list: &[&[u16]] = match block {
                PcsBlock::A => &[BC1_BLOCK_A],
                PcsBlock::B => &[BC1_BLOCK_B],
                PcsBlock::C => &[BC1_BLOCK_C],
                PcsBlock::D => &[BC1_BLOCK_D],
                PcsBlock::E => &[BC1_BLOCK_E],
                PcsBlock::F => &[BC1_BLOCK_F],
                PcsBlock::AnyBlock => &[
                    BC1_BLOCK_A,
                    BC1_BLOCK_D,
                    BC1_BLOCK_B,
                    BC1_BLOCK_E,
                    BC1_BLOCK_F,
                    BC1_BLOCK_C,
                ],
                PcsBlock::Reserved => &[],
            };
            for channels in list {
                for ch in channels.iter() {
                    self.push(acq_index, BandClass::Bc1, *ch);
                }
            }
        }
    }

    fn channel_list(&mut self, acq_index: u16, band_class: BandClass, channels: &[u16]) {
        for ch in channels {
            self.push(acq_index, band_class, *ch);
        }
    }

    fn skip(&mut self, acq_index: u16, what: &str) {
        log::debug!("ms_scan: acq#{} skipped ({})", acq_index, what);
        self.skipped += 1;
    }

    fn classic_record(&mut self, acq_index: u16, body: &AcquisitionBody) {
        match body {
            AcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
                self.cellular_standard(acq_index, *ab, *pri_sec)
            }
            AcquisitionBody::CellularCdmaPreferred { ab } => {
                self.cellular_standard(acq_index, *ab, StandardChannelSelection::PrimaryOrSecondary)
            }
            AcquisitionBody::CellularCdmaCustom { channels } => {
                self.channel_list(acq_index, BandClass::Bc0, channels)
            }
            AcquisitionBody::PcsCdmaUsingBlocks { blocks } => self.pcs_blocks(acq_index, blocks),
            AcquisitionBody::PcsCdmaUsingChannels { channels } => {
                self.channel_list(acq_index, BandClass::Bc1, channels)
            }
            AcquisitionBody::CellularAnalog { .. } => self.skip(acq_index, "analog"),
            AcquisitionBody::JtacsCdmaStandard { .. } | AcquisitionBody::JtacsCdmaCustom { .. } => {
                self.skip(acq_index, "jtacs")
            }
            AcquisitionBody::BandClass6UsingChannels { .. } => self.skip(acq_index, "bc6"),
            AcquisitionBody::Unknown => self.skip(acq_index, "unknown type"),
        }
    }

    fn extended_record(&mut self, acq_index: u16, body: &ExtAcquisitionBody) {
        match body {
            ExtAcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
                self.cellular_standard(acq_index, *ab, *pri_sec)
            }
            ExtAcquisitionBody::CellularCdmaPreferred { ab } => {
                self.cellular_standard(acq_index, *ab, StandardChannelSelection::PrimaryOrSecondary)
            }
            ExtAcquisitionBody::CellularCdmaCustom { channels } => {
                self.channel_list(acq_index, BandClass::Bc0, channels)
            }
            ExtAcquisitionBody::PcsCdmaUsingBlocks { blocks } => self.pcs_blocks(acq_index, blocks),
            ExtAcquisitionBody::PcsCdmaUsingChannels { channels } => {
                self.channel_list(acq_index, BandClass::Bc1, channels)
            }
            ExtAcquisitionBody::Generic1xIs95 { entries } => {
                for e in entries {
                    match e.band_class {
                        GENERIC_BAND_CLASS_BC0 => {
                            self.push(acq_index, BandClass::Bc0, e.channel_number)
                        }
                        GENERIC_BAND_CLASS_BC1 => {
                            self.push(acq_index, BandClass::Bc1, e.channel_number)
                        }
                        other => self.skip(acq_index, &format!("generic 1x band class {other}")),
                    }
                }
            }
            ExtAcquisitionBody::CellularAnalog { .. } => self.skip(acq_index, "analog"),
            ExtAcquisitionBody::JtacsCdmaStandard { .. }
            | ExtAcquisitionBody::JtacsCdmaCustom { .. } => self.skip(acq_index, "jtacs"),
            ExtAcquisitionBody::BandClass6UsingChannels { .. } => self.skip(acq_index, "bc6"),
            ExtAcquisitionBody::GenericHrpd { .. } => self.skip(acq_index, "hrpd"),
            ExtAcquisitionBody::UmbCommonTable { .. } | ExtAcquisitionBody::GenericUmb { .. } => {
                self.skip(acq_index, "umb")
            }
            ExtAcquisitionBody::Other { .. } => self.skip(acq_index, "unknown type"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERIZON_CLASSIC: &[u8] =
        include_bytes!("../../../cdma-otasp/tests/fixtures/verizon_50408.prl");
    const VERIZON_EXTENDED: &[u8] =
        include_bytes!("../../../cdma-otasp/tests/fixtures/verizon_51611.prl");

    #[test]
    fn classic_verizon_expands_in_acquisition_table_order() {
        let plan = PrlScanPlan::from_bytes(VERIZON_CLASSIC).unwrap();
        assert_eq!(plan.format, PrlFormat::Classic);
        assert_eq!(plan.prl_id, 50408);
        assert!(plan.pref_only);
        let head: Vec<(BandClass, u16)> = plan
            .channels
            .iter()
            .take(4)
            .map(|c| (c.band_class, c.channel))
            .collect();
        assert_eq!(
            head,
            vec![
                (BandClass::Bc0, BC0_B_PRIMARY),
                (BandClass::Bc0, BC0_B_SECONDARY),
                (BandClass::Bc0, BC0_A_PRIMARY),
                (BandClass::Bc0, BC0_A_SECONDARY),
            ]
        );
        assert_eq!(plan.channels[0].acq_index, 0);
        assert_eq!(plan.channels[2].acq_index, 1);
        assert!((plan.channels[0].frequency_hz - 881_520_000.0).abs() < 1.0);
        assert_eq!(plan.channels_in(BandClass::Bc1), 40);
        assert_eq!(plan.channels_in(BandClass::Bc0), 15);
        assert_eq!(plan.skipped_records, 2);
        assert_eq!(plan.systems.len(), 794);
    }

    #[test]
    fn classic_verizon_verdicts() {
        let plan = PrlScanPlan::from_bytes(VERIZON_CLASSIC).unwrap();
        assert!(matches!(
            plan.verdict(5269, 7),
            PrlVerdict::Permitted {
                record: Some(0),
                geo: Some(1),
                roaming_indicator: Some(66)
            }
        ));
        assert_eq!(plan.systems.iter().filter(|s| !s.preferred).count(), 15);
        assert_eq!(plan.verdict(65000, 1), PrlVerdict::Unlisted);
    }

    #[test]
    fn extended_verizon_expands_generic_and_pcs_records() {
        let plan = PrlScanPlan::from_bytes(VERIZON_EXTENDED).unwrap();
        assert_eq!(plan.format, PrlFormat::Extended);
        assert_eq!(plan.prl_id, 51611);
        assert_eq!(
            (plan.channels[0].band_class, plan.channels[0].channel),
            (BandClass::Bc0, BC0_B_PRIMARY)
        );
        assert!(plan.channels_in(BandClass::Bc1) > 40);
        assert_eq!(plan.skipped_records, 18);
        assert_eq!(plan.channels_in(BandClass::Bc0), 18);
        assert!(plan.systems.len() > 700);
        assert!(plan.verdict(5269, 0).permitted());
    }

    #[test]
    fn unlisted_system_is_allowed_when_pref_only_is_clear() {
        let plan = PrlScanPlan::from_parts(1, false, &[(BandClass::Bc0, 384)], vec![]);
        assert!(matches!(
            plan.verdict(22, 65535),
            PrlVerdict::Permitted { record: None, .. }
        ));
        let strict = PrlScanPlan::from_parts(1, true, &[(BandClass::Bc0, 384)], vec![]);
        assert_eq!(strict.verdict(22, 65535), PrlVerdict::Unlisted);
    }

    #[test]
    fn nid_rules_pick_the_first_matching_record() {
        let systems = vec![
            PrlSystem {
                index: 0,
                sid: 42,
                nid: NidMatch::Exact(7),
                preferred: false,
                geo: 1,
                acq_index: 0,
                roaming_indicator: None,
                more_desirable: false,
            },
            PrlSystem {
                index: 1,
                sid: 42,
                nid: NidMatch::Any,
                preferred: true,
                geo: 1,
                acq_index: 0,
                roaming_indicator: Some(1),
                more_desirable: false,
            },
        ];
        let plan = PrlScanPlan::from_parts(1, true, &[(BandClass::Bc0, 384)], systems);
        assert!(matches!(
            plan.verdict(42, 7),
            PrlVerdict::Negative { record: 0 }
        ));
        assert!(matches!(
            plan.verdict(42, 8),
            PrlVerdict::Permitted {
                record: Some(1),
                ..
            }
        ));
    }

    #[test]
    fn invalid_channels_are_dropped_and_duplicates_collapse() {
        let plan = PrlScanPlan::from_parts(
            1,
            true,
            &[
                (BandClass::Bc0, 384),
                (BandClass::Bc0, 384),
                (BandClass::Bc0, 2000),
            ],
            vec![],
        );
        assert_eq!(plan.channels.len(), 1);
        assert_eq!(plan.invalid_channels, 1);
    }
}
