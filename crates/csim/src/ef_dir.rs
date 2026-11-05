//! Registering the CSIM in the card's EF_DIR (TS 102 221 §13.1).
//!
//! Handsets build their application list from the card platform's
//! EF_DIR before selecting anything, and a CDMA stack activates a CSIM
//! only when it is listed there.

use crate::card::{Card, CardError, KEY_REF_ADM1, SW_OK};
use crate::fcp::{self, get_u64, parse_tlv};
use crate::{CSIM_AID_PREFIX, CSIM_INSTANCE_AID, Fid, hex_upper};

const TAG_APPLICATION_TEMPLATE: u8 = 0x61;
const TAG_AID: u8 = 0x4F;
const TAG_LABEL: u8 = 0x50;
const CSIM_LABEL: &str = "CSIM";

fn is_blank(rec: &[u8]) -> bool {
    rec.iter().all(|&b| b == 0xFF) || rec.iter().all(|&b| b == 0x00)
}

/// The AID of an EF_DIR application template record.
fn record_aid(rec: &[u8]) -> Option<&[u8]> {
    if rec.len() >= 4 && rec[0] == TAG_APPLICATION_TEMPLATE && rec[2] == TAG_AID {
        rec.get(4..4 + usize::from(rec[3]))
    } else {
        None
    }
}

/// The application template record for the CSIM, F padded.
pub fn csim_record(record_length: usize) -> Result<Vec<u8>, CardError> {
    let mut body = vec![TAG_AID, CSIM_INSTANCE_AID.len() as u8];
    body.extend(CSIM_INSTANCE_AID);
    body.extend([TAG_LABEL, CSIM_LABEL.len() as u8]);
    body.extend(CSIM_LABEL.as_bytes());
    let mut rec = vec![TAG_APPLICATION_TEMPLATE, body.len() as u8];
    rec.extend(body);
    if rec.len() > record_length {
        return Err(CardError::Invalid(format!(
            "EF_DIR record too small ({record_length} B) for the CSIM template ({} B)",
            rec.len()
        )));
    }
    rec.resize(record_length, 0xFF);
    Ok(rec)
}

/// Where the CSIM goes in EF_DIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Its existing record, or the first blank one.
    FirstFree,
    /// Record 1, with the other applications moved down in order.
    First,
}

/// The UPDATE RECORD writes that register the CSIM: (record number,
/// record). `records` are the current records, record 1 first. Existing
/// CSIM entries are replaced.
pub fn plan(
    records: &[Vec<u8>],
    csim: &[u8],
    placement: Placement,
) -> Result<Vec<(u8, Vec<u8>)>, CardError> {
    let blank = vec![0xFF; csim.len()];
    let mut others = Vec::new();
    let mut csim_slots = Vec::new();
    for (i, rec) in records.iter().enumerate() {
        match record_aid(rec) {
            Some(aid) if aid.starts_with(&CSIM_AID_PREFIX) => csim_slots.push(i),
            _ if !is_blank(rec) => others.push(rec.clone()),
            _ => {}
        }
    }
    let desired: Vec<Vec<u8>> = match placement {
        Placement::First => {
            if others.len() + 1 > records.len() {
                return Err(CardError::Invalid(format!(
                    "EF_DIR full: {} records, {} other applications",
                    records.len(),
                    others.len()
                )));
            }
            let mut d = vec![csim.to_vec()];
            d.extend(others);
            d.resize(records.len(), blank);
            d
        }
        Placement::FirstFree => {
            let target = match csim_slots.first() {
                Some(&i) => i,
                None => records.iter().position(|r| is_blank(r)).ok_or_else(|| {
                    CardError::Invalid(format!(
                        "EF_DIR full: all {} records populated by other applications",
                        records.len()
                    ))
                })?,
            };
            let mut d = records.to_vec();
            for &i in &csim_slots {
                d[i] = blank.clone();
            }
            d[target] = csim.to_vec();
            d
        }
    };
    Ok(records
        .iter()
        .zip(desired)
        .enumerate()
        .filter(|(_, (old, want))| *old != want)
        .map(|(i, (_, want))| ((i + 1) as u8, want))
        .collect())
}

pub struct Registration {
    /// Whether any record was written.
    pub changed: bool,
    /// The record number that holds the CSIM.
    pub record: u8,
    /// (AID hex, label) of every application listed afterwards.
    pub applications: Vec<(String, String)>,
}

/// Writes the CSIM template into the platform EF_DIR, on a logical channel
/// of its own so that the application selected on the basic channel does
/// not matter. The platform ADM1 (from the sysmocom key material) is
/// required.
pub fn register(
    card: &mut Card,
    platform_adm1: &[u8],
    placement: Placement,
) -> Result<Registration, CardError> {
    card.open_channel()?;
    let result = register_on_channel(card, platform_adm1, placement);
    let closed = card.close_channel();
    let registration = result?;
    closed?;
    Ok(registration)
}

fn register_on_channel(
    card: &mut Card,
    platform_adm1: &[u8],
    placement: Placement,
) -> Result<Registration, CardError> {
    card.select_fid(Fid::MF)?;
    card.verify(KEY_REF_ADM1, platform_adm1, true)?;
    let r = card.select_fid_fcp(Fid::EF_DIR)?;
    if r.sw != SW_OK {
        return Err(CardError::Status {
            op: "SELECT EF_DIR".into(),
            sw: r.sw,
        });
    }
    let info = fcp::parse_fcp(&r.data);
    let rl = get_u64(&info, "record_length").unwrap_or(0) as u8;
    let n = get_u64(&info, "num_records").unwrap_or(0) as u8;
    if rl == 0 || n == 0 {
        return Err(CardError::Invalid(
            "EF_DIR FCP has no record length or count".into(),
        ));
    }
    let mut records = (1..=n)
        .map(|i| card.read_record(i, rl))
        .collect::<Result<Vec<_>, _>>()?;
    let csim = csim_record(usize::from(rl))?;
    let mut writes = plan(&records, &csim, placement)?;
    // The moved applications are written before the record that receives
    // the CSIM, so a failure part way leaves every application listed.
    writes.sort_by_key(|(i, rec)| (*rec == csim, *i));
    for (i, rec) in &writes {
        card.update_record(*i, rec)?;
        records[usize::from(*i) - 1] = rec.clone();
    }
    let record = records
        .iter()
        .position(|r| *r == csim)
        .map_or(0, |i| (i + 1) as u8);
    let applications = records
        .iter()
        .filter_map(|rec| {
            let aid = record_aid(rec)?;
            let body = rec.get(2..2 + usize::from(rec[1]))?;
            let label = parse_tlv(body)
                .into_iter()
                .find(|(t, _)| *t == u32::from(TAG_LABEL))
                .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
                .unwrap_or_default();
            Some((hex_upper(aid), label))
        })
        .collect();
    Ok(Registration {
        changed: !writes.is_empty(),
        record,
        applications,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_hex;

    const RL: usize = 43;

    fn usim() -> Vec<u8> {
        parse_hex("61294F10A0000000871002FFFFFFFF890709000050055553696D31730EA00C80011781025F608203454150").unwrap()
    }

    fn isim() -> Vec<u8> {
        let mut r = parse_hex("61194F10A0000000871004FFFFFFFF890709000050054953696D31").unwrap();
        r.resize(RL, 0xFF);
        r
    }

    fn records() -> Vec<Vec<u8>> {
        let mut r = vec![usim(), isim()];
        r.resize(8, vec![0xFF; RL]);
        r
    }

    #[test]
    fn appends_to_first_blank() {
        let csim = csim_record(RL).unwrap();
        assert_eq!(
            plan(&records(), &csim, Placement::FirstFree).unwrap(),
            [(3, csim)]
        );
    }

    #[test]
    fn first_moves_others_down() {
        let csim = csim_record(RL).unwrap();
        let writes = plan(&records(), &csim, Placement::First).unwrap();
        assert_eq!(writes, [(1, csim), (2, usim()), (3, isim())]);
    }

    #[test]
    fn first_is_idempotent_and_replaces_later_entries() {
        let csim = csim_record(RL).unwrap();
        let mut done = vec![csim.clone(), usim(), isim()];
        done.resize(8, vec![0xFF; RL]);
        assert!(plan(&done, &csim, Placement::First).unwrap().is_empty());
        assert!(plan(&done, &csim, Placement::FirstFree).unwrap().is_empty());
        let mut late = vec![usim(), isim(), csim.clone()];
        late.resize(8, vec![0xFF; RL]);
        assert_eq!(
            plan(&late, &csim, Placement::First).unwrap(),
            [(1, csim), (2, usim()), (3, isim())]
        );
    }

    #[test]
    fn full_directory() {
        let csim = csim_record(RL).unwrap();
        assert!(plan(&vec![usim(); 8], &csim, Placement::FirstFree).is_err());
    }

    #[test]
    fn record_layout() {
        let rec = csim_record(RL).unwrap();
        assert_eq!(rec[..4], [0x61, 0x18, 0x4F, 0x10]);
        assert_eq!(&rec[20..26], b"\x50\x04CSIM");
        assert_eq!(rec.len(), RL);
    }
}
