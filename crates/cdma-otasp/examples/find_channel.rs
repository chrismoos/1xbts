use cdma_otasp::param::prl::AcquisitionBody;
use cdma_otasp::param::prl_ext::ExtAcquisitionBody;
use cdma_otasp::param::{prl, prl_ext};
use std::fs;

fn main() {
    let target: u16 = std::env::args().nth(1).unwrap().parse().unwrap();
    for path in std::env::args().skip(2) {
        let bytes = fs::read(&path).unwrap();
        let name = path.rsplit('/').next().unwrap_or(&path);
        if prl_ext::sniff_sspr_p_rev(&bytes) >= 0x02 {
            if let Ok(p) = prl_ext::decode(&bytes) {
                for (i, r) in p.acquisition_records.iter().enumerate() {
                    let chans: Vec<u16> = match &r.body {
                        ExtAcquisitionBody::CellularCdmaCustom { channels } => channels.clone(),
                        ExtAcquisitionBody::PcsCdmaUsingChannels { channels } => channels.clone(),
                        ExtAcquisitionBody::JtacsCdmaCustom { channels } => channels.clone(),
                        ExtAcquisitionBody::BandClass6UsingChannels { channels } => {
                            channels.clone()
                        }
                        _ => continue,
                    };
                    if chans.contains(&target) {
                        let users: Vec<usize> = p
                            .system_records
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| s.acq_index as usize == i)
                            .map(|(j, _)| j)
                            .take(5)
                            .collect();
                        println!(
                            "{} (ext) acq[{}] type=0x{:02x} chans={:?} used by sys idx {:?}",
                            name, i, r.acq_type_raw, chans, users
                        );
                    }
                }
            }
            continue;
        }
        if let Ok(p) = prl::decode(&bytes) {
            for (i, r) in p.acquisition_records.iter().enumerate() {
                let chans: Vec<u16> = match &r.body {
                    AcquisitionBody::CellularCdmaCustom { channels } => channels.clone(),
                    AcquisitionBody::PcsCdmaUsingChannels { channels } => channels.clone(),
                    AcquisitionBody::JtacsCdmaCustom { channels } => channels.clone(),
                    AcquisitionBody::BandClass6UsingChannels { channels } => channels.clone(),
                    _ => continue,
                };
                if chans.contains(&target) {
                    let users: Vec<usize> = p
                        .system_records
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| s.acq_index as usize == i)
                        .map(|(j, _)| j)
                        .take(5)
                        .collect();
                    println!(
                        "{} acq[{}] type=0x{:02x} chans={:?} used by sys idx {:?}",
                        name, i, r.acq_type_raw, chans, users
                    );
                }
            }
        }
    }
}
