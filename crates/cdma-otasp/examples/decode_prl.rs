use cdma_otasp::param::prl::{
    self, AbSelection, AcquisitionBody, NidInclusion, PcsBlock, PrefNeg, Priority,
    RoamingIndicator, StandardChannelSelection,
};
use std::fs;

fn ab(a: AbSelection) -> &'static str {
    match a {
        AbSelection::SystemA => "A",
        AbSelection::SystemB => "B",
        AbSelection::Reserved => "rsv",
        AbSelection::EitherAOrB => "A|B",
    }
}
fn std_chan(s: StandardChannelSelection) -> &'static str {
    match s {
        StandardChannelSelection::Reserved => "rsv",
        StandardChannelSelection::Primary => "pri",
        StandardChannelSelection::Secondary => "sec",
        StandardChannelSelection::PrimaryOrSecondary => "pri|sec",
    }
}
fn pcs(b: PcsBlock) -> &'static str {
    match b {
        PcsBlock::A => "A",
        PcsBlock::B => "B",
        PcsBlock::C => "C",
        PcsBlock::D => "D",
        PcsBlock::E => "E",
        PcsBlock::F => "F",
        PcsBlock::Reserved => "rsv",
        PcsBlock::AnyBlock => "*",
    }
}
fn acq_brief(b: &AcquisitionBody) -> String {
    match b {
        AcquisitionBody::CellularAnalog { ab: a } => format!("CellAnalog/{}", ab(*a)),
        AcquisitionBody::CellularCdmaStandard { ab: a, pri_sec } => {
            format!("CellCDMA-Std/{}/{}", ab(*a), std_chan(*pri_sec))
        }
        AcquisitionBody::CellularCdmaCustom { channels } => format!(
            "CellCDMA-Custom {{{}}}",
            channels
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        AcquisitionBody::CellularCdmaPreferred { ab: a } => format!("CellCDMA-Pref/{}", ab(*a)),
        AcquisitionBody::PcsCdmaUsingBlocks { blocks } => format!(
            "PCS-Blocks {{{}}}",
            blocks.iter().map(|b| pcs(*b)).collect::<Vec<_>>().join(",")
        ),
        AcquisitionBody::PcsCdmaUsingChannels { channels } => format!(
            "PCS-Chans {{{}}}",
            channels
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        AcquisitionBody::JtacsCdmaStandard { ab: a, pri_sec } => {
            format!("JTACS-Std/{}/{}", ab(*a), std_chan(*pri_sec))
        }
        AcquisitionBody::JtacsCdmaCustom { channels } => format!(
            "JTACS-Custom {{{}}}",
            channels
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        AcquisitionBody::BandClass6UsingChannels { channels } => format!(
            "BC6-Chans {{{}}}",
            channels
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        AcquisitionBody::Unknown => "Unknown".into(),
    }
}
fn roam(r: RoamingIndicator) -> String {
    match r {
        RoamingIndicator::OnHome => "home".into(),
        RoamingIndicator::Roaming => "roam".into(),
        RoamingIndicator::InternationalRoaming => "intl".into(),
        RoamingIndicator::Lte => "LTE".into(),
        RoamingIndicator::Flashing => "flash".into(),
        RoamingIndicator::Other(v) => format!("#{}", v),
    }
}
fn nid_str(n: NidInclusion, v: Option<u16>) -> String {
    match (n, v) {
        (NidInclusion::AnyNid, _) => "any".into(),
        (NidInclusion::PublicNid, _) => "pub".into(),
        (NidInclusion::Reserved, _) => "rsv".into(),
        (NidInclusion::SingleNid, Some(0xFFFF)) => "any*".into(),
        (NidInclusion::SingleNid, Some(0)) => "pub*".into(),
        (NidInclusion::SingleNid, Some(n)) => n.to_string(),
        (NidInclusion::SingleNid, None) => "?".into(),
    }
}

fn main() {
    use cdma_otasp::param::prl_ext;
    let summary_only = std::env::args().any(|a| a == "--summary");
    let limit: usize = std::env::args()
        .position(|a| a == "--limit")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let paths: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--") && a.ends_with(".prl"))
        .collect();
    for path in paths {
        let bytes = fs::read(&path).unwrap();
        let name = path.rsplit('/').next().unwrap_or(&path);
        if prl_ext::sniff_sspr_p_rev(&bytes) >= 0x02 {
            match prl_ext::decode(&bytes) {
                Ok(p) => {
                    println!(
                        "\n=== {} ({} bytes) [EXTENDED p_rev={}] ===",
                        name,
                        bytes.len(),
                        p.cur_sspr_p_rev
                    );
                    println!(
                        "PR_LIST_ID={} pref_only={} def_roam={} acq={} subnet={} sys={} crc_ok={}",
                        p.pr_list_id,
                        p.pref_only,
                        roam(p.def_roam_ind),
                        p.acquisition_records.len(),
                        p.common_subnet_records.len(),
                        p.system_records.len(),
                        p.crc_ok(),
                    );
                    if !summary_only {
                        for (i, r) in p.acquisition_records.iter().take(limit).enumerate() {
                            println!(
                                "  acq[{}] type=0x{:02x} len={} {:?}",
                                i, r.acq_type_raw, r.length, r.body
                            );
                        }
                        for (i, s) in p.system_records.iter().take(limit).enumerate() {
                            println!(
                                "  sys[{}] {:?} pref={:?} acq={} {:?} roam={:?}",
                                i,
                                s.sys_record_type,
                                s.pref_neg,
                                s.acq_index,
                                s.system_id,
                                s.roaming_indicator
                            );
                        }
                    }
                }
                Err(e) => println!("\n=== {} === EXTENDED decode error: {}", name, e),
            }
            continue;
        }
        match prl::decode(&bytes) {
            Ok(p) => {
                println!("\n=== {} ({} bytes) ===", name, bytes.len());
                println!(
                    "PR_LIST_ID={} pref_only={} def_roam={} acq={} sys={} crc_ok={}",
                    p.pr_list_id,
                    p.pref_only,
                    roam(p.def_roam_ind),
                    p.acquisition_records.len(),
                    p.system_records.len(),
                    p.crc_ok()
                );
                if summary_only {
                    continue;
                }
                println!("ACQ_TABLE (first {}):", limit);
                for (i, r) in p.acquisition_records.iter().take(limit).enumerate() {
                    println!("  [{}] {}", i, acq_brief(&r.body));
                }
                println!("SYS_TABLE (first {}):", limit);
                println!(
                    "  {:>5} {:>6} {:>5} {:>4} {:>5} {:>5} {:>5}",
                    "idx", "SID", "NID", "mode", "pri", "acq", "roam"
                );
                for (i, s) in p.system_records.iter().take(limit).enumerate() {
                    let mode = match s.pref_neg {
                        PrefNeg::Preferred => "PREF",
                        PrefNeg::Negative => "NEG",
                    };
                    let pri = match s.priority {
                        Some(Priority::MoreDesirable) => "MORE",
                        Some(Priority::EquallyDesirable) => "EQUAL",
                        None => "-",
                    };
                    let sid = if s.sid == 0 {
                        "*".to_string()
                    } else {
                        s.sid.to_string()
                    };
                    println!(
                        "  [{:>3}] {:>6} {:>5} {:>4} {:>5} {:>5} {:>5}",
                        i,
                        sid,
                        nid_str(s.nid_incl, s.nid),
                        mode,
                        pri,
                        s.acq_index,
                        s.roaming_indicator.map(roam).unwrap_or_else(|| "-".into()),
                    );
                }
            }
            Err(e) => println!("\n=== {} === decode error: {}", name, e),
        }
    }
}
