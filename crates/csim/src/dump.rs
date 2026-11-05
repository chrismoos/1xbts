//! A full record of what a CSIM tells a terminal about its files.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_yaml::{Mapping, Value};

use crate::card::{
    Card, CardError, FACTORY_ADM1, KEY_REF_ADM1, KEY_REF_PIN1, KEY_REF_PIN2,
    SW_AUTH_METHOD_BLOCKED, SW_FILE_DEACTIVATED, SW_FILE_NOT_FOUND, SW_OK, SW_REF_DATA_INVALIDATED,
    sw_verify_tries,
};
use crate::fcp::{self, get_bool, get_str, get_u64};
use crate::{CSIM_AID_PREFIX, Fid, ef_table, hex_upper, parse_hex};

const DUMP_FORMAT: u64 = 1;
const MF_LEVEL_LABELS: [&str; 3] = ["ICCID", "EF_DIR", "PL"];
const ARR_MF: &str = "2F06";
const ARR_ADF: &str = "6F06";
const TAG_APPLICATION_TEMPLATE: u32 = 0x61;
const TAG_AID: u32 = 0x4F;
const TAG_LABEL: u32 = 0x50;
const TAG_PATH: u32 = 0x51;

fn s(text: impl Into<String>) -> Value {
    Value::String(text.into())
}

fn set(m: &mut Mapping, key: &str, value: impl Into<Value>) {
    m.insert(s(key), value.into());
}

fn sw_text(sw: u16) -> String {
    format!("{sw:04X}")
}

/// Who to authenticate as before the files are read.
#[derive(Default)]
pub struct Credentials {
    pub pin1: Option<Vec<u8>>,
    pub pin2: Option<Vec<u8>>,
    pub adm1: Option<Vec<u8>>,
}

pub struct DumpOptions {
    pub credentials: Credentials,
    /// Also SELECT every FID 6F00-6FFF under the ADF and 2F00-2FFF at the
    /// MF.
    pub scan: bool,
    /// Load the EF table first, for a freshly installed applet. Needs the
    /// applet ADM1 (the factory ADM1 when none is given).
    pub init_meta: bool,
    pub progress: Option<Box<dyn Fn(&str)>>,
}

struct Dumper<'a> {
    card: &'a mut Card,
    arr: HashMap<String, Vec<Vec<u8>>>,
    absent: Vec<String>,
}

impl Dumper<'_> {
    /// SELECT asking for the FCP: the parsed FCP (empty when the card sent
    /// none) and the status word.
    fn select(&mut self, fid: Fid) -> (Mapping, u16) {
        match self.card.select_fid_fcp(fid) {
            Ok(r) => (
                if r.data.is_empty() {
                    Mapping::new()
                } else {
                    fcp::parse_fcp(&r.data)
                },
                r.sw,
            ),
            Err(e) => (Mapping::new(), e.sw().unwrap_or(0)),
        }
    }

    /// Reads the whole file into `raw` or `records`, or the status word of
    /// the failed read into `read_sw`.
    fn read_content(&mut self, info: &mut Mapping) {
        match get_str(info, "structure") {
            Some("transparent") => {
                let size = get_u64(info, "file_size").unwrap_or(0) as usize;
                if size == 0 {
                    return;
                }
                match self.card.read_binary(size, 0) {
                    Ok(data) => set(info, "raw", hex_upper(&data)),
                    Err(e) => set(info, "read_sw", sw_text(e.sw().unwrap_or(0))),
                }
            }
            Some("linear-fixed" | "cyclic") => {
                let rl = get_u64(info, "record_length").unwrap_or(0) as u8;
                let n = get_u64(info, "num_records").unwrap_or(0) as u8;
                let mut records = Vec::new();
                for i in 1..=n {
                    match self.card.read_record(i, rl) {
                        Ok(r) => records.push(s(hex_upper(&r))),
                        Err(e) => {
                            set(info, "read_sw", sw_text(e.sw().unwrap_or(0)));
                            break;
                        }
                    }
                }
                if !records.is_empty() {
                    set(info, "records", Value::Sequence(records));
                }
            }
            _ => {}
        }
    }

    fn records_of(info: &Mapping) -> Vec<Vec<u8>> {
        info.get("records")
            .and_then(Value::as_sequence)
            .map(|r| {
                r.iter()
                    .filter_map(|v| v.as_str().and_then(|h| parse_hex(h).ok()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ef_dir(&mut self) -> Mapping {
        let (mut info, sw) = self.select(Fid::EF_DIR);
        set(&mut info, "select_sw", sw_text(sw));
        if sw != SW_OK {
            return info;
        }
        self.read_content(&mut info);
        let mut apps = Vec::new();
        for rec in Dumper::records_of(&info) {
            if rec.first().is_none_or(|&b| b == 0xFF) {
                continue;
            }
            let tlvs = fcp::parse_tlv(&rec);
            let Some(&(TAG_APPLICATION_TEMPLATE, body)) = tlvs.first() else {
                continue;
            };
            let mut app = Mapping::new();
            for (t, v) in fcp::parse_tlv(body) {
                match t {
                    TAG_AID => set(&mut app, "aid", hex_upper(v)),
                    TAG_LABEL => set(
                        &mut app,
                        "label",
                        String::from_utf8(v.to_vec()).unwrap_or_else(|_| hex_upper(v)),
                    ),
                    TAG_PATH => set(&mut app, "path", hex_upper(v)),
                    _ => {}
                }
            }
            if app.contains_key("aid") {
                apps.push(Value::Mapping(app));
            }
        }
        set(&mut info, "applications", Value::Sequence(apps));
        info
    }

    fn arr_file(&mut self, fid: Fid) -> Option<Vec<Vec<u8>>> {
        let (mut info, sw) = self.select(fid);
        if sw != SW_OK || get_bool(&info, "is_df") {
            return None;
        }
        self.read_content(&mut info);
        Some(Dumper::records_of(&info)).filter(|r| !r.is_empty())
    }

    /// SELECT one FID, resolve its access rules and read it. `back`
    /// restores the current DF after a DF was selected by accident.
    fn visit(&mut self, into: &mut Mapping, label: &str, fid: Fid, default_arr: &str, back: Fid) {
        let (mut info, sw) = self.select(fid);
        if get_str(&info, "fid").is_none() {
            set(&mut info, "fid", fid.to_string());
        }
        set(&mut info, "select_sw", sw_text(sw));
        if sw == SW_FILE_NOT_FOUND {
            if !label.starts_with("EF_") || label == "EF_DIR" {
                self.absent.push(label.to_owned());
            }
            return;
        }
        if sw == SW_OK || sw == SW_FILE_DEACTIVATED {
            if get_bool(&info, "is_df") {
                let _ = self.card.select_fid(back);
            } else {
                let arr = &self.arr;
                let lookup = |f: Option<&str>, record: u64| {
                    let recs = arr.get(f.unwrap_or(default_arr))?;
                    recs.get(usize::try_from(record).ok()?.checked_sub(1)?)
                        .cloned()
                };
                fcp::resolve_access(&mut info, &lookup);
                if sw == SW_OK {
                    self.read_content(&mut info);
                }
            }
        }
        into.insert(s(label), Value::Mapping(info));
    }

    fn mf_level(&mut self, into: &mut Mapping, scan: bool) {
        let _ = self.card.select_fid(Fid::MF);
        for label in MF_LEVEL_LABELS {
            let fid = ef_table::by_name(label).map_or(Fid::EF_DIR, |e| e.fid);
            self.visit(into, label, fid, ARR_MF, Fid::MF);
        }
        if scan {
            for lo in 0..=u8::MAX {
                let fid = Fid::mf_ef(lo);
                if fid.label().is_some() || fid == Fid::EF_ARR_MF {
                    continue;
                }
                self.visit(into, &format!("EF_{fid}"), fid, ARR_MF, Fid::MF);
            }
        }
    }

    fn pin_state(&mut self, key_ref: u8) -> Mapping {
        let mut out = Mapping::new();
        match self.card.pin_status(key_ref) {
            Err(e) => set(&mut out, "error", e.to_string()),
            Ok(sw) => {
                set(&mut out, "sw", sw_text(sw));
                if let Some(tries) = sw_verify_tries(sw) {
                    set(&mut out, "tries", u64::from(tries));
                } else if sw == SW_AUTH_METHOD_BLOCKED {
                    set(&mut out, "tries", 0u64);
                    set(&mut out, "state", "blocked");
                } else if sw == SW_REF_DATA_INVALIDATED {
                    set(&mut out, "state", "disabled");
                }
            }
        }
        out
    }

    fn status(&mut self) -> Mapping {
        match self.card.status() {
            Err(e) => {
                let mut m = Mapping::new();
                set(&mut m, "error", e.to_string());
                m
            }
            Ok(r) => {
                let mut m = Mapping::new();
                set(&mut m, "sw", sw_text(r.sw));
                if !r.data.is_empty() {
                    m.extend(fcp::parse_fcp(&r.data));
                }
                m
            }
        }
    }
}

/// UTC now as 2026-10-30T12:00:00Z.
fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Dumps a card. The MF level is read first as seen before any
/// application is selected, which on a UICC is the platform's file
/// system, then the CSIM ADF with its PIN status, STATUS, the PIN retry
/// counters, EF_ARR and every EF, then the MF-level EFs again from inside
/// the CSIM session, since a card may answer them differently there.
pub fn dump_card(card: &mut Card, opts: &DumpOptions) -> Result<Mapping, CardError> {
    let say = |msg: &str| {
        if let Some(p) = &opts.progress {
            p(msg);
        }
    };
    let mut dump = Mapping::new();
    set(&mut dump, "format", DUMP_FORMAT);
    set(&mut dump, "tool", "csim dump");
    set(&mut dump, "time", utc_now());
    set(&mut dump, "atr", hex_upper(&card.atr()));
    let mut d = Dumper {
        card,
        arr: HashMap::new(),
        absent: Vec::new(),
    };

    say("MF level");
    let (mut mf, sw) = d.select(Fid::MF);
    set(&mut mf, "select_sw", sw_text(sw));
    let mut platform = Mapping::new();
    let mut platform_efs = Mapping::new();
    let mut platform_dir = None;
    if sw == SW_OK {
        platform_dir = Some(d.ef_dir());
        if let Some(recs) = d.arr_file(Fid::EF_ARR_MF) {
            d.arr.insert(ARR_MF.into(), recs);
        }
        if opts.scan {
            say("scanning 2F00..2FFF at the MF");
        }
        d.mf_level(&mut platform_efs, opts.scan);
    }
    set(&mut platform, "mf", Value::Mapping(mf));
    set(&mut platform, "efs", Value::Mapping(platform_efs));
    let csim_aid = platform_dir.as_ref().and_then(|dir| {
        dir.get("applications")?
            .as_sequence()?
            .iter()
            .find_map(|a| {
                let aid = a.get("aid")?.as_str()?;
                aid.starts_with(&hex_upper(&CSIM_AID_PREFIX))
                    .then(|| aid.to_owned())
            })
    });
    if let Some(dir) = platform_dir {
        set(&mut platform, "ef_dir", Value::Mapping(dir));
    }
    set(&mut dump, "platform", Value::Mapping(platform));

    let adf_fcp = match csim_aid.as_deref().map(parse_hex) {
        Some(Ok(aid)) => d
            .card
            .select_aid_partial(&aid)
            .or_else(|_| d.card.select_csim_partial())?,
        _ => d.card.select_csim_partial()?,
    };
    let mut adf = if adf_fcp.is_empty() {
        Mapping::new()
    } else {
        fcp::parse_fcp(&adf_fcp)
    };
    let aid = csim_aid
        .or_else(|| get_str(&adf, "df_name").map(str::to_owned))
        .unwrap_or_else(|| hex_upper(&CSIM_AID_PREFIX));
    set(&mut adf, "aid", aid);
    set(&mut dump, "adf", Value::Mapping(adf));
    set(&mut dump, "status", Value::Mapping(d.status()));

    if opts.init_meta {
        let table = ef_table::meta_bytes();
        say(&format!("loading EF table ({} bytes)", table.len()));
        let adm = opts
            .credentials
            .adm1
            .clone()
            .unwrap_or_else(|| FACTORY_ADM1.to_vec());
        d.card.verify(KEY_REF_ADM1, &adm, true)?;
        d.card.ensure_ef_table(&table)?;
    }

    let mut pins = Mapping::new();
    for (name, key_ref) in [
        ("PIN1", KEY_REF_PIN1),
        ("PIN2", KEY_REF_PIN2),
        ("ADM1", KEY_REF_ADM1),
    ] {
        pins.insert(s(name), Value::Mapping(d.pin_state(key_ref)));
    }
    let mut verified = Vec::new();
    let creds = &opts.credentials;
    for (name, key_ref, value) in [
        ("PIN1", KEY_REF_PIN1, &creds.pin1),
        ("PIN2", KEY_REF_PIN2, &creds.pin2),
        ("ADM1", KEY_REF_ADM1, &creds.adm1),
    ] {
        let Some(value) = value else { continue };
        match d.card.verify(key_ref, value, true) {
            Ok(_) => verified.push(s(name)),
            Err(e) => {
                if let Some(Value::Mapping(p)) = pins.get_mut(name) {
                    set(p, "verify_error", e.to_string());
                }
            }
        }
    }
    set(&mut dump, "pins", Value::Mapping(pins));
    set(&mut dump, "verified", Value::Sequence(verified));

    if let Some(recs) = d.arr_file(Fid::EF_ARR_ADF) {
        d.arr.insert(ARR_ADF.into(), recs);
    }

    // Back to the ADF by FID 7FFF (TS 102 221 §8.4) keeps the application
    // session and its PIN state. SELECT by AID would start a new session.
    let mut efs = Mapping::new();
    for entry in ef_table::entries() {
        if MF_LEVEL_LABELS.contains(&entry.name.as_str()) {
            continue;
        }
        say(&format!("{} {}", entry.name, entry.fid));
        d.visit(&mut efs, &entry.name, entry.fid, ARR_ADF, Fid::ADF);
    }
    if opts.scan {
        say("scanning 6F00..6FFF under the ADF");
        for lo in 0..=u8::MAX {
            let fid = Fid::adf_ef(lo);
            if fid.label().is_some() || fid == Fid::EF_ARR_ADF {
                continue;
            }
            d.visit(&mut efs, &format!("EF_{fid}"), fid, ARR_ADF, Fid::ADF);
        }
    }

    say("MF level from the CSIM session");
    let (mut mf, sw) = d.select(Fid::MF);
    set(&mut mf, "select_sw", sw_text(sw));
    set(&mut dump, "mf", Value::Mapping(mf));
    if sw == SW_OK {
        set(&mut dump, "ef_dir", Value::Mapping(d.ef_dir()));
        if !d.arr.contains_key(ARR_MF) {
            if let Some(recs) = d.arr_file(Fid::EF_ARR_MF) {
                d.arr.insert(ARR_MF.into(), recs);
            }
        }
        d.mf_level(&mut efs, false);
    }
    let _ = d.card.select_fid(Fid::ADF);

    let mut arr = Mapping::new();
    for key in [ARR_MF, ARR_ADF] {
        if let Some(recs) = d.arr.get(key) {
            arr.insert(
                s(key),
                Value::Sequence(recs.iter().map(|r| s(hex_upper(r))).collect()),
            );
        }
    }
    set(&mut dump, "arr", Value::Mapping(arr));
    set(&mut dump, "efs", Value::Mapping(efs));
    set(
        &mut dump,
        "absent",
        Value::Sequence(d.absent.iter().map(|a| s(a.clone())).collect()),
    );
    Ok(dump)
}

fn apps_line(dir: Option<&Value>) -> Option<String> {
    let apps = dir?.get("applications")?.as_sequence()?;
    if apps.is_empty() {
        return None;
    }
    let parts: Vec<String> = apps
        .iter()
        .map(|a| {
            let aid = a.get("aid").and_then(Value::as_str).unwrap_or("?");
            let label = a.get("label").and_then(Value::as_str).unwrap_or("");
            format!("{aid} {label:?}")
        })
        .collect();
    Some(parts.join(", "))
}

pub fn print_summary(dump: &Mapping) {
    let empty = Mapping::new();
    let adf = dump
        .get("adf")
        .and_then(Value::as_mapping)
        .unwrap_or(&empty);
    println!("ATR:      {}", get_str(dump, "atr").unwrap_or("?"));
    println!("CSIM AID: {}", get_str(adf, "aid").unwrap_or("?"));
    if let Some(line) = apps_line(dump.get("platform").and_then(|p| p.get("ef_dir"))) {
        println!("EF_DIR:   {line}");
    }
    if let Some(line) = apps_line(dump.get("ef_dir")) {
        println!("EF_DIR*:  {line}");
    }
    if let Some(keys) = adf
        .get("pin_status")
        .and_then(|p| p.get("keys"))
        .and_then(Value::as_sequence)
    {
        let parts: Vec<String> = keys
            .iter()
            .map(|k| {
                let name = k.get("name").and_then(Value::as_str).unwrap_or("?");
                let state = if k.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "enabled"
                } else {
                    "disabled"
                };
                format!("{name} {state}")
            })
            .collect();
        println!("PINs:     {}", parts.join(", "));
    }
    if let Some(pins) = dump.get("pins").and_then(Value::as_mapping) {
        let parts: Vec<String> = pins
            .iter()
            .map(|(name, v)| {
                let tries = v
                    .get("tries")
                    .and_then(Value::as_u64)
                    .map_or("?".into(), |t| t.to_string());
                let sw = v.get("sw").and_then(Value::as_str).unwrap_or("?");
                format!("{} {tries} ({sw})", name.as_str().unwrap_or("?"))
            })
            .collect();
        println!("tries:    {}", parts.join(", "));
    }
    if let Some(arr) = dump
        .get("arr")
        .and_then(Value::as_mapping)
        .filter(|a| !a.is_empty())
    {
        let parts: Vec<String> = arr
            .iter()
            .map(|(k, v)| {
                format!(
                    "{} {} records",
                    k.as_str().unwrap_or("?"),
                    v.as_sequence().map_or(0, Vec::len)
                )
            })
            .collect();
        println!("EF_ARR:   {}", parts.join(", "));
    }
    if let Some(v) = dump
        .get("verified")
        .and_then(Value::as_sequence)
        .filter(|v| !v.is_empty())
    {
        let names: Vec<&str> = v.iter().filter_map(Value::as_str).collect();
        println!("verified: {}", names.join(" "));
    }
    println!();
    let header = format!(
        "{:13} {:5} {:12} {:>7} {:>3}  ACCESS / CONTENT",
        "EF", "FID", "STRUCT", "SIZE", "SFI"
    );
    println!("{header}");
    println!("{}", "-".repeat(header.len()));
    let efs = dump
        .get("efs")
        .and_then(Value::as_mapping)
        .unwrap_or(&empty);
    for (label, info) in efs {
        let (Some(label), Some(info)) = (label.as_str(), info.as_mapping()) else {
            continue;
        };
        print_ef_row(label, info);
    }
    let absent: Vec<&str> = dump
        .get("absent")
        .and_then(Value::as_sequence)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    println!();
    let list = if absent.is_empty() {
        String::new()
    } else {
        format!(": {}", absent.join(" "))
    };
    println!(
        "{} EFs present, {} of the table absent{list}",
        efs.len(),
        absent.len()
    );
    println!(
        "EF_DIR* and the ICCID/EF_DIR/PL rows are the MF level as seen from inside the CSIM session."
    );
}

fn print_ef_row(label: &str, info: &Mapping) {
    let fid = get_str(info, "fid").unwrap_or("?");
    let sw = get_str(info, "select_sw").unwrap_or("?");
    if sw != "9000" && sw != "6283" {
        println!("{label:13} {fid:5} {:12} {:>7} {:>3}  SW={sw}", "", "", "");
        return;
    }
    let structure = get_str(info, "structure").unwrap_or("?");
    let num = |k: &str| get_u64(info, k).map_or("?".into(), |n| n.to_string());
    let size = match structure {
        "transparent" => num("file_size"),
        "linear-fixed" | "cyclic" => format!("{}x{}", num("num_records"), num("record_length")),
        _ => "-".into(),
    };
    let sfi = get_u64(info, "sfi")
        .filter(|&s| s != 0)
        .map_or(String::new(), |s| format!("{s:02X}"));
    let state = if sw == "6283" { " deactivated" } else { "" };
    let access = fcp::access_summary(info.get("access").and_then(Value::as_mapping));
    println!("{label:13} {fid:5} {structure:12} {size:>7} {sfi:>3}  {access}{state}");
    let pad = " ".repeat(44);
    let clip = |t: &str, n: usize| {
        if t.len() > n {
            format!("{}...", &t[..n])
        } else {
            t.to_owned()
        }
    };
    if let Some(raw) = get_str(info, "raw") {
        println!("{pad}{}", clip(raw, 48));
    } else if let Some(r0) = info
        .get("records")
        .and_then(Value::as_sequence)
        .and_then(|r| r.first())
        .and_then(Value::as_str)
    {
        println!("{pad}#1 {}", clip(r0, 44));
    } else if let Some(read_sw) = get_str(info, "read_sw") {
        println!("{pad}read SW={read_sw}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_format() {
        let t = super::utc_now();
        assert_eq!(t.len(), 20);
        assert!(t.ends_with('Z') && &t[4..5] == "-" && &t[10..11] == "T");
    }
}
