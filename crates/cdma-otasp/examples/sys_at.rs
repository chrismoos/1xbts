use cdma_otasp::param::prl_ext::{self, ExtSystemId};
use std::fs;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let indices: Vec<usize> = std::env::args()
        .skip(2)
        .filter_map(|s| s.parse().ok())
        .collect();
    let bytes = fs::read(&path).unwrap();
    let p = prl_ext::decode(&bytes).unwrap();
    for i in indices {
        let s = &p.system_records[i];
        let sid = match &s.system_id {
            ExtSystemId::Cdma2000 { sid, .. } => format!("{}", sid),
            other => format!("{:?}", other),
        };
        println!(
            "sys[{}] sid={} pref={:?} acq={} roam={:?}",
            i, sid, s.pref_neg, s.acq_index, s.roaming_indicator
        );
    }
}
