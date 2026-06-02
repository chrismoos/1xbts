use cdma_otasp::param::prl_ext::{self, ExtSystemId};
use std::collections::BTreeSet;
use std::fs;

fn main() {
    let mut all_acq_types: BTreeSet<u8> = BTreeSet::new();
    let mut all_sys_types: BTreeSet<u8> = BTreeSet::new();
    let mut any_common_subnet = false;

    for path in std::env::args().skip(1) {
        let bytes = fs::read(&path).unwrap();
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let p_rev = prl_ext::sniff_sspr_p_rev(&bytes);
        if p_rev < 0x02 {
            println!("== {:<32} CLASSIC (skipping)", name);
            continue;
        }
        match prl_ext::decode(&bytes) {
            Ok(p) => {
                let mut acq_types: BTreeSet<u8> = BTreeSet::new();
                let mut sys_types: BTreeSet<u8> = BTreeSet::new();
                for r in &p.acquisition_records {
                    acq_types.insert(r.acq_type_raw);
                    all_acq_types.insert(r.acq_type_raw);
                }
                for s in &p.system_records {
                    let raw_type = match &s.system_id {
                        ExtSystemId::Cdma2000 { .. } => 0x00,
                        ExtSystemId::Hrpd { .. } => 0x01,
                        ExtSystemId::MccMnc(_) => 0x03,
                        ExtSystemId::Raw {
                            sys_record_type, ..
                        } => *sys_record_type,
                    };
                    sys_types.insert(raw_type);
                    all_sys_types.insert(raw_type);
                }
                let subnet = p.common_subnet_records.len();
                if subnet > 0 {
                    any_common_subnet = true;
                }
                println!(
                    "== {:<32} acq={} sys={} subnet={} acq_types={:?} sys_types={:?}",
                    name,
                    p.acquisition_records.len(),
                    p.system_records.len(),
                    subnet,
                    acq_types
                        .iter()
                        .map(|v| format!("0x{:02x}", v))
                        .collect::<Vec<_>>(),
                    sys_types
                        .iter()
                        .map(|v| format!("0x{:02x}", v))
                        .collect::<Vec<_>>(),
                );
            }
            Err(e) => println!("== {:<32} EXT decode error: {}", name, e),
        }
    }
    println!();
    println!(
        "GLOBAL ACQ TYPES: {:?}",
        all_acq_types
            .iter()
            .map(|v| format!("0x{:02x}", v))
            .collect::<Vec<_>>()
    );
    println!(
        "GLOBAL SYS TYPES: {:?}",
        all_sys_types
            .iter()
            .map(|v| format!("0x{:02x}", v))
            .collect::<Vec<_>>()
    );
    println!("ANY COMMON SUBNET RECORDS: {}", any_common_subnet);

    // Coverage vs spec
    let spec_acq: &[(u8, &str)] = &[
        (0x01, "CellularAnalog"),
        (0x02, "CellularCdmaStandard"),
        (0x03, "CellularCdmaCustom"),
        (0x04, "CellularCdmaPreferred"),
        (0x05, "PcsCdmaUsingBlocks"),
        (0x06, "PcsCdmaUsingChannels"),
        (0x07, "JtacsCdmaStandard"),
        (0x08, "JtacsCdmaCustom"),
        (0x09, "BandClass6UsingChannels"),
        (0x0A, "Generic1xIs95"),
        (0x0B, "GenericHrpd"),
        (0x0F, "UmbCommonTable"),
        (0x10, "GenericUmb"),
    ];
    let spec_sys: &[(u8, &str)] = &[(0x00, "Cdma2000"), (0x01, "HRPD"), (0x03, "McсMnc")];
    println!("\nCOVERAGE GAPS:");
    for (t, name) in spec_acq {
        if !all_acq_types.contains(t) {
            println!("  ACQ 0x{:02x} {} — NOT COVERED", t, name);
        }
    }
    for (t, name) in spec_sys {
        if !all_sys_types.contains(t) {
            println!("  SYS 0x{:02x} {} — NOT COVERED", t, name);
        }
    }
    if !any_common_subnet {
        println!("  Common Subnet Table records — NOT COVERED");
    }
}
