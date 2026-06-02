use cdma_otasp::param::{prl, prl_ext};
use std::fs;

fn main() {
    let sid: u16 = std::env::args().nth(1).unwrap().parse().unwrap();
    for path in std::env::args().skip(2) {
        let bytes = fs::read(&path).unwrap();
        let name = path.rsplit('/').next().unwrap_or(&path);
        if prl_ext::sniff_sspr_p_rev(&bytes) >= 0x02 {
            if let Ok(p) = prl_ext::decode(&bytes) {
                for (i, s) in p.system_records.iter().enumerate() {
                    if let prl_ext::ExtSystemId::Cdma2000 {
                        sid: rs,
                        nid,
                        nid_incl,
                    } = &s.system_id
                        && *rs == sid
                    {
                        println!(
                            "{} (ext) sys[{}] sid={} nid_incl={:?} nid={:?} pref={:?} acq={} roam={:?}",
                            name,
                            i,
                            rs,
                            nid_incl,
                            nid,
                            s.pref_neg,
                            s.acq_index,
                            s.roaming_indicator
                        );
                    }
                }
            }
            continue;
        }
        if let Ok(p) = prl::decode(&bytes) {
            for (i, s) in p.system_records.iter().enumerate() {
                if s.sid == sid {
                    println!(
                        "{} sys[{}] sid={} nid_incl={:?} nid={:?} pref={:?} pri={:?} acq={} roam={:?}",
                        name,
                        i,
                        s.sid,
                        s.nid_incl,
                        s.nid,
                        s.pref_neg,
                        s.priority,
                        s.acq_index,
                        s.roaming_indicator
                    );
                }
            }
        }
    }
}
