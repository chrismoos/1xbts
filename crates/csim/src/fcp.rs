//! FCP parsing and access rule decoding for UICC files.
//!
//! Parses SELECT and STATUS responses (TS 102 221 §11.1.1.3) into the
//! ordered maps a dump stores, and turns a file's security attributes
//! into operation names and access conditions, whichever of the three
//! formats the card uses: compact (tag 8C), expanded (tag AB) or
//! referenced (tag 8B into EF_ARR). The condition names are ALW, NEV,
//! PIN1, PIN2, ADM1 to ADM5, UPIN, or OR(...)/AND(...) of those, so that
//! the rules of two cards compare textually.

use serde_yaml::{Mapping, Value};

use crate::hex_upper;

const TAG_FCP: u8 = 0x62;
const TAG_FCI: u8 = 0x6F;
const TAG_FILE_SIZE: u32 = 0x80;
const TAG_TOTAL_SIZE: u32 = 0x81;
const TAG_FILE_DESCRIPTOR: u32 = 0x82;
const TAG_FILE_ID: u32 = 0x83;
const TAG_DF_NAME: u32 = 0x84;
const TAG_PROPRIETARY: u32 = 0xA5;
const TAG_LIFE_CYCLE: u32 = 0x8A;
const TAG_SEC_REFERENCED: u32 = 0x8B;
const TAG_SEC_COMPACT: u32 = 0x8C;
const TAG_SEC_EXPANDED: u32 = 0xAB;
const TAG_SFI: u32 = 0x88;
const TAG_PIN_STATUS_TEMPLATE: u32 = 0xC6;
const TAG_PS_DO: u32 = 0x90;
const TAG_USAGE_QUALIFIER: u32 = 0x95;
const TAG_KEY_REF: u32 = 0x83;
const TAG_UICC_CHARACTERISTICS: u32 = 0x80;

const AM_DO_BYTE: u32 = 0x80;
const AM_DO_INS: u32 = 0x84;
const AM_DO_CMD_FIRST: u32 = 0x81;
const AM_DO_CMD_LAST: u32 = 0x8F;
const AM_DO_STATE: u32 = 0x9C;
const SC_DO_ALWAYS: u32 = 0x90;
const SC_DO_NEVER: u32 = 0x97;
const SC_DO_OR: u32 = 0xA0;
const SC_DO_AND: u32 = 0xAF;
const SC_DO_AUTH: u32 = 0xA4;
const SC_DO_SM_TEMPLATES: [u32; 3] = [0xB4, 0xB6, 0xB8];
/// Usage qualifier 08 is user verification (TS 102 221 Table 9.4).
const USAGE_PIN_VERIFICATION: u8 = 0x08;

// File descriptor byte (TS 102 221 §11.1.1.4.3).
const FD_SHAREABLE: u8 = 0x40;
const FD_TYPE_MASK: u8 = 0x38;
const FD_TYPE_DF: u8 = 0x38;
const FD_TYPE_INTERNAL_EF: u8 = 0x08;
const FD_STRUCTURE_MASK: u8 = 0x07;
// SFI in b8..b4 of tag 88 (TS 102 221 §11.1.1.4.8).
const SFI_SHIFT: u8 = 3;
const SFI_MASK: u8 = 0x1F;

/// Operations compared between two files, in report order.
pub const COMPARED_OPERATIONS: [&str; 5] = ["READ", "UPDATE", "INCREASE", "DEACTIVATE", "ACTIVATE"];

// BER-TLV (ISO/IEC 8825-1): a first tag byte with b5..b1 all set is
// followed by a second tag byte. A length byte 81 or 82 is followed by one
// or two length bytes, and 83 or above is a longer form this parser does
// not read.
const TAG_NUMBER_MASK: u32 = 0x1F;
const LEN_ONE_BYTE: u8 = 0x81;
const LEN_TWO_BYTES: u8 = 0x82;
const LEN_UNSUPPORTED: u8 = 0x83;

/// BER-TLV with one or two byte tags and lengths up to two bytes. Stops
/// at the first malformed object.
pub fn parse_tlv(data: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let mut tag = u32::from(data[i]);
        i += 1;
        if tag & TAG_NUMBER_MASK == TAG_NUMBER_MASK {
            let Some(&next) = data.get(i) else { break };
            tag = (tag << 8) | u32::from(next);
            i += 1;
        }
        let Some(&first) = data.get(i) else { break };
        i += 1;
        let len = match first {
            LEN_ONE_BYTE => {
                let Some(&l) = data.get(i) else { break };
                i += 1;
                usize::from(l)
            }
            LEN_TWO_BYTES => {
                let (Some(&a), Some(&b)) = (data.get(i), data.get(i + 1)) else {
                    break;
                };
                i += 2;
                (usize::from(a) << 8) | usize::from(b)
            }
            l if l >= LEN_UNSUPPORTED => break,
            l => usize::from(l),
        };
        if i + len > data.len() {
            break;
        }
        out.push((tag, &data[i..i + len]));
        i += len;
    }
    out
}

/// The name of a key reference (TS 102 221 §9.5 Table 9.3).
pub fn key_ref_name(r: u8) -> String {
    match r {
        0x01 => "PIN1".into(),
        0x81 => "PIN2".into(),
        0x0A => "ADM1".into(),
        0x0B => "ADM2".into(),
        0x0C => "ADM3".into(),
        0x0D => "ADM4".into(),
        0x0E => "ADM5".into(),
        0x11 => "UPIN".into(),
        0x02..=0x08 => format!("PIN1_APP{r}"),
        0x82..=0x88 => format!("PIN2_APP{}", r - 0x80),
        _ => format!("KEY{r:02X}"),
    }
}

/// One SC_DO (ISO/IEC 7816-4 §5.4.3.3) to a condition name.
fn decode_sc_do(tag: u32, value: &[u8]) -> String {
    match tag {
        SC_DO_ALWAYS => "ALW".into(),
        SC_DO_NEVER => "NEV".into(),
        SC_DO_AUTH => {
            let mut key = None;
            let mut usage = None;
            for (t, v) in parse_tlv(value) {
                if t == TAG_KEY_REF && !v.is_empty() {
                    key = Some(v[0]);
                } else if t == TAG_USAGE_QUALIFIER && !v.is_empty() {
                    usage = Some(v[0]);
                }
            }
            let Some(key) = key else { return "AUTH".into() };
            let mut name = key_ref_name(key);
            if let Some(u) = usage.filter(|&u| u != USAGE_PIN_VERIFICATION) {
                name.push_str(&format!("[usage={u:02X}]"));
            }
            name
        }
        SC_DO_OR | SC_DO_AND => {
            let parts: Vec<String> = parse_tlv(value)
                .into_iter()
                .map(|(t, v)| decode_sc_do(t, v))
                .collect();
            let word = if tag == SC_DO_OR { "OR" } else { "AND" };
            format!("{word}({})", parts.join(","))
        }
        t if SC_DO_SM_TEMPLATES.contains(&t) => format!("SM[{t:02X}]"),
        t => format!("SC[{t:02X}:{}]", hex_upper(value)),
    }
}

// Compact-format SC byte (ISO/IEC 7816-4 §5.4.3.2): 00 always, FF never,
// otherwise b7 secure messaging, b6 external authentication, b5 user
// authentication, b8 set when all of them are needed and clear when any
// one is enough, and b4..b1 the security environment.
const SC_ALWAYS: u8 = 0x00;
const SC_NEVER: u8 = 0xFF;
const SC_ALL_CONDITIONS: u8 = 0x80;
const SC_SEID_MASK: u8 = 0x0F;
const SC_CONDITION_BITS: [(u8, &str); 3] = [(0x40, "SM"), (0x20, "EXT_AUTH"), (0x10, "USER_AUTH")];

/// One compact-format SC byte.
fn decode_sc_byte(sc: u8) -> String {
    match sc {
        SC_ALWAYS => return "ALW".into(),
        SC_NEVER => return "NEV".into(),
        _ => {}
    }
    let parts: Vec<&str> = SC_CONDITION_BITS
        .iter()
        .filter(|(m, _)| sc & m != 0)
        .map(|&(_, n)| n)
        .collect();
    if parts.is_empty() {
        return format!("SC[{sc:02X}]");
    }
    let mut name = parts.join(if sc & SC_ALL_CONDITIONS != 0 {
        "+"
    } else {
        "|"
    });
    let seid = sc & SC_SEID_MASK;
    if seid != 0 {
        name.push_str(&format!("(se{seid})"));
    }
    name
}

// ISO/IEC 7816-4 AM byte, b7 down to b1. b8 is the format bit.
const AM_BITS_EF: [(u8, &str); 7] = [
    (0x40, "DELETE"),
    (0x20, "TERMINATE"),
    (0x10, "ACTIVATE"),
    (0x08, "DEACTIVATE"),
    (0x04, "WRITE"),
    (0x02, "UPDATE"),
    (0x01, "READ"),
];
const AM_BITS_DF: [(u8, &str); 7] = [
    (0x40, "DELETE_SELF"),
    (0x20, "TERMINATE"),
    (0x10, "ACTIVATE"),
    (0x08, "DEACTIVATE"),
    (0x04, "CREATE_EF"),
    (0x02, "CREATE_DF"),
    (0x01, "DELETE_CHILD"),
];
const AM_PROPRIETARY: u8 = 0x80;
const AM_OPERATION_BITS: u8 = 0x7F;

fn am_ops(am: u8, is_df: bool) -> Vec<String> {
    let bits = if is_df { &AM_BITS_DF } else { &AM_BITS_EF };
    bits.iter()
        .filter(|(m, _)| am & m != 0)
        .map(|&(_, n)| n.to_owned())
        .collect()
}

fn ins_name(ins: u8) -> String {
    let name = match ins {
        0x32 => "INCREASE",
        0xA2 => "SEARCH",
        0xB0 | 0xB2 => "READ",
        0xD6 | 0xDC => "UPDATE",
        0x04 => "DEACTIVATE",
        0x44 => "ACTIVATE",
        0xE4 => "DELETE",
        0xE6 => "TERMINATE",
        _ => return format!("INS{ins:02X}"),
    };
    name.to_owned()
}

/// Access rules in the order the card states them.
pub type Rules = Vec<(String, String)>;

fn add_rule(rules: &mut Rules, op: &str, cond: &str) {
    match rules.iter_mut().find(|(o, _)| o == op) {
        Some((_, c)) if c != cond => *c = format!("OR({c},{cond})"),
        Some(_) => {}
        None => rules.push((op.to_owned(), cond.to_owned())),
    }
}

/// Tag 8C contents. Several AM and SC groups in one value are an OR
/// relation (TS 102 221 §9.2.5).
pub fn decode_compact(value: &[u8], is_df: bool) -> Rules {
    let mut rules = Rules::new();
    let mut i = 0;
    while i < value.len() {
        let am = value[i];
        i += 1;
        if am & AM_PROPRIETARY != 0 {
            let n = (am & AM_OPERATION_BITS).count_ones() as usize;
            let end = (i + n).min(value.len());
            add_rule(
                &mut rules,
                &format!("AM[{am:02X}]"),
                &hex_upper(&value[i..end]),
            );
            i = end;
            continue;
        }
        for op in am_ops(am, is_df) {
            let Some(&sc) = value.get(i) else { break };
            add_rule(&mut rules, &op, &decode_sc_byte(sc));
            i += 1;
        }
    }
    rules
}

/// Tag AB contents, or one EF_ARR record. A sequence of SC_DOs after one
/// AM_DO without an OR template is an AND relation (TS 102 221 Annex
/// E.3.2).
pub fn decode_expanded(value: &[u8], is_df: bool) -> Rules {
    let mut rules = Rules::new();
    let mut ops: Option<Vec<String>> = None;
    let mut pending: Vec<String> = Vec::new();
    let flush = |rules: &mut Rules, ops: &Option<Vec<String>>, pending: &[String]| {
        let Some(ops) = ops else { return };
        let cond = match pending {
            [] => "NEV".to_owned(),
            [one] => one.clone(),
            many => format!("AND({})", many.join(",")),
        };
        for op in ops {
            add_rule(rules, op, &cond);
        }
    };
    for (tag, v) in parse_tlv(value) {
        let new_ops = match tag {
            AM_DO_BYTE => Some(v.first().map(|&am| am_ops(am, is_df)).unwrap_or_default()),
            AM_DO_INS if !v.is_empty() => Some(vec![ins_name(v[0])]),
            AM_DO_CMD_FIRST..=AM_DO_CMD_LAST => {
                Some(vec![format!("CMD[{tag:02X}:{}]", hex_upper(v))])
            }
            AM_DO_STATE => Some(vec![format!("STATE[{}]", hex_upper(v))]),
            _ => None,
        };
        match new_ops {
            Some(o) => {
                flush(&mut rules, &ops, &pending);
                ops = Some(o);
                pending.clear();
            }
            None => pending.push(decode_sc_do(tag, v)),
        }
    }
    flush(&mut rules, &ops, &pending);
    rules
}

fn s(text: impl Into<String>) -> Value {
    Value::String(text.into())
}

fn n(v: impl Into<u64>) -> Value {
    Value::Number(v.into().into())
}

fn set(m: &mut Mapping, key: &str, value: Value) {
    m.insert(s(key), value);
}

/// Tag 8B contents (TS 102 221 §9.2.7, Annex E.3.3).
fn parse_arr_ref(v: &[u8]) -> Mapping {
    let mut m = Mapping::new();
    match v.len() {
        1 => set(&mut m, "record", n(v[0])),
        3 => {
            set(&mut m, "fid", s(hex_upper(&v[..2])));
            set(&mut m, "record", n(v[2]));
        }
        len if len >= 4 && len % 2 == 0 => {
            set(&mut m, "fid", s(hex_upper(&v[..2])));
            let pairs = v[2..]
                .chunks(2)
                .map(|p| {
                    let mut pair = Mapping::new();
                    set(&mut pair, "seid", n(p[0]));
                    set(&mut pair, "record", n(p[1]));
                    Value::Mapping(pair)
                })
                .collect();
            set(&mut m, "by_seid", Value::Sequence(pairs));
        }
        _ => set(&mut m, "raw", s(hex_upper(v))),
    }
    m
}

/// PS_DO bit of the first listed key. Each later key is the next lower bit.
const PS_DO_FIRST_KEY: u8 = 0x80;

/// Tag C6 contents (TS 102 221 §11.1.1.4.10).
fn parse_pin_status(value: &[u8]) -> Mapping {
    let mut keys = Vec::new();
    let mut usage = Vec::new();
    let mut ps_bits: Option<&[u8]> = None;
    let mut ps_hex = None;
    let mut key_index = 0usize;
    for (tag, v) in parse_tlv(value) {
        if tag == TAG_PS_DO {
            ps_bits = Some(v);
            ps_hex = Some(hex_upper(v));
            key_index = 0;
        } else if tag == TAG_USAGE_QUALIFIER && !v.is_empty() {
            usage.push(n(v[0]));
        } else if tag == TAG_KEY_REF && !v.is_empty() {
            let enabled = ps_bits
                .and_then(|bits| bits.get(key_index / 8))
                .map(|b| b & (PS_DO_FIRST_KEY >> (key_index % 8)) != 0);
            let mut k = Mapping::new();
            set(&mut k, "ref", s(format!("{:02X}", v[0])));
            set(&mut k, "name", s(key_ref_name(v[0])));
            set(&mut k, "enabled", enabled.map_or(Value::Null, Value::Bool));
            keys.push(Value::Mapping(k));
            key_index += 1;
        }
    }
    let mut m = Mapping::new();
    set(&mut m, "keys", Value::Sequence(keys));
    if let Some(ps) = ps_hex {
        set(&mut m, "ps_do", s(ps));
    }
    if !usage.is_empty() {
        set(&mut m, "usage_qualifiers", Value::Sequence(usage));
    }
    m
}

fn be_uint(v: &[u8]) -> u64 {
    v.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

fn lifecycle_name(lcs: u8) -> String {
    let name = match lcs {
        0x01 => "creation",
        0x03 => "initialization",
        0x05 | 0x07 => "activated",
        0x04 | 0x06 => "deactivated",
        0x0C..=0x0F => "terminated",
        _ => return format!("unknown({lcs:02X})"),
    };
    name.to_owned()
}

fn structure_name(bits: u8) -> String {
    match bits {
        0x01 => "transparent".into(),
        0x02 => "linear-fixed".into(),
        0x06 => "cyclic".into(),
        b => format!("unknown({b:02X})"),
    }
}

/// A SELECT or STATUS response as the ordered map a dump stores. Keys
/// that the response does not state are absent.
pub fn parse_fcp(data: &[u8]) -> Mapping {
    let mut info = Mapping::new();
    set(&mut info, "fcp", s(hex_upper(data)));
    let tlvs = parse_tlv(data);
    let Some(&(outer_tag, outer)) = tlvs.first() else {
        return info;
    };
    let outer_tag = u8::try_from(outer_tag).unwrap_or(0);
    let inner = if outer_tag == TAG_FCP || outer_tag == TAG_FCI {
        parse_tlv(outer)
    } else {
        tlvs.clone()
    };
    for (tag, v) in inner {
        match tag {
            TAG_FILE_SIZE if !v.is_empty() => set(&mut info, "file_size", n(be_uint(v))),
            TAG_TOTAL_SIZE if !v.is_empty() => set(&mut info, "total_size", n(be_uint(v))),
            TAG_FILE_DESCRIPTOR if !v.is_empty() => parse_descriptor(&mut info, v),
            TAG_FILE_ID if v.len() >= 2 => set(&mut info, "fid", s(hex_upper(&v[..2]))),
            TAG_DF_NAME if !v.is_empty() => set(&mut info, "df_name", s(hex_upper(v))),
            // Length 0 means the file has no SFI (TS 102 221 §11.1.1.4.8).
            TAG_SFI => set(
                &mut info,
                "sfi",
                n(v.first().map_or(0, |b| (b >> SFI_SHIFT) & SFI_MASK)),
            ),
            TAG_LIFE_CYCLE if !v.is_empty() => {
                set(&mut info, "lifecycle", n(v[0]));
                set(&mut info, "lifecycle_name", s(lifecycle_name(v[0])));
            }
            TAG_SEC_REFERENCED => {
                let mut sec = security("referenced", v);
                set(&mut sec, "arr", Value::Mapping(parse_arr_ref(v)));
                set(&mut info, "security", Value::Mapping(sec));
            }
            TAG_SEC_COMPACT => set(
                &mut info,
                "security",
                Value::Mapping(security("compact", v)),
            ),
            TAG_SEC_EXPANDED => set(
                &mut info,
                "security",
                Value::Mapping(security("expanded", v)),
            ),
            TAG_PIN_STATUS_TEMPLATE => {
                set(&mut info, "pin_status", Value::Mapping(parse_pin_status(v)))
            }
            TAG_PROPRIETARY => {
                let mut prop = Mapping::new();
                set(&mut prop, "raw", s(hex_upper(v)));
                for (t, pv) in parse_tlv(v) {
                    if t == TAG_UICC_CHARACTERISTICS && !pv.is_empty() {
                        set(
                            &mut prop,
                            "uicc_characteristics",
                            s(format!("{:02X}", pv[0])),
                        );
                    } else {
                        set(&mut prop, &format!("{t:02X}"), s(hex_upper(pv)));
                    }
                }
                set(&mut info, "proprietary", Value::Mapping(prop));
            }
            other => {
                let key = s("other_tags");
                if !info.contains_key(&key) {
                    info.insert(key.clone(), Value::Mapping(Mapping::new()));
                }
                if let Some(Value::Mapping(m)) = info.get_mut(&key) {
                    set(m, &format!("{other:02X}"), s(hex_upper(v)));
                }
            }
        }
    }
    info
}

fn security(format: &str, v: &[u8]) -> Mapping {
    let mut sec = Mapping::new();
    set(&mut sec, "format", s(format));
    set(&mut sec, "raw", s(hex_upper(v)));
    sec
}

fn parse_descriptor(info: &mut Mapping, v: &[u8]) {
    let fd = v[0];
    set(info, "file_descriptor", s(hex_upper(v)));
    set(info, "shareable", Value::Bool(fd & FD_SHAREABLE != 0));
    let kind = fd & FD_TYPE_MASK;
    let is_df = kind == FD_TYPE_DF;
    set(info, "is_df", Value::Bool(is_df));
    if is_df {
        set(info, "structure", s("df"));
    } else {
        set(info, "structure", s(structure_name(fd & FD_STRUCTURE_MASK)));
        if kind == FD_TYPE_INTERNAL_EF {
            set(info, "internal_ef", Value::Bool(true));
        }
    }
    if v.len() >= 2 {
        set(info, "data_coding", n(v[1]));
    }
    if v.len() >= 5 && !is_df {
        set(
            info,
            "record_length",
            n((u64::from(v[2]) << 8) | u64::from(v[3])),
        );
        set(info, "num_records", n(v[4]));
    }
}

/// Looks up an EF_ARR record: (FID or none for the default EF_ARR,
/// record number) to the record bytes.
pub type ArrLookup<'a> = &'a dyn Fn(Option<&str>, u64) -> Option<Vec<u8>>;

/// The SE that holds the default rules (TS 102 221 §9.2.7).
const DEFAULT_SEID: u64 = 1;

/// The access rules of a parsed FCP, stored under `access` and returned.
/// A referenced rule whose EF_ARR record is not available gives
/// `UNRESOLVED`. An operation an EF's rule does not mention is NEVer
/// allowed (TS 102 221 Annex E.1), so each compared operation is stated.
pub fn resolve_access(info: &mut Mapping, arr_lookup: ArrLookup) -> Rules {
    let is_df = get_bool(info, "is_df");
    let sec = info.get("security").and_then(Value::as_mapping).cloned();
    let mut rules = match sec {
        None => Rules::new(),
        Some(sec) => {
            let raw = crate::parse_hex(get_str(&sec, "raw").unwrap_or("")).unwrap_or_default();
            match get_str(&sec, "format") {
                Some("compact") => decode_compact(&raw, is_df),
                Some("expanded") => decode_expanded(&raw, is_df),
                _ => resolve_referenced(&sec, arr_lookup, is_df),
            }
        }
    };
    let unresolved = rules.iter().any(|(op, _)| op == "UNRESOLVED");
    if !rules.is_empty() && !unresolved && !is_df {
        let cyclic = get_str(info, "structure") == Some("cyclic");
        for op in COMPARED_OPERATIONS {
            if op == "INCREASE" && !cyclic {
                continue;
            }
            if !rules.iter().any(|(o, _)| o == op) {
                rules.push((op.to_owned(), "NEV".to_owned()));
            }
        }
    }
    let access: Mapping = rules
        .iter()
        .map(|(o, c)| (s(o.clone()), s(c.clone())))
        .collect();
    set(info, "access", Value::Mapping(access));
    rules
}

fn resolve_referenced(sec: &Mapping, arr_lookup: ArrLookup, is_df: bool) -> Rules {
    let empty = Mapping::new();
    let arr = sec.get("arr").and_then(Value::as_mapping).unwrap_or(&empty);
    let fid = get_str(arr, "fid");
    let mut record = arr.get("record").and_then(Value::as_u64);
    if record.is_none() {
        if let Some(pairs) = arr.get("by_seid").and_then(Value::as_sequence) {
            let pair_record = |p: &Value| p.get("record").and_then(Value::as_u64);
            record = pairs
                .iter()
                .find(|p| p.get("seid").and_then(Value::as_u64) == Some(DEFAULT_SEID))
                .and_then(pair_record)
                .or_else(|| pairs.first().and_then(pair_record));
        }
    }
    match record.and_then(|r| arr_lookup(fid, r)) {
        Some(rec) => decode_expanded(&rec, is_df),
        None => {
            let record = record.map_or("None".to_owned(), |r| r.to_string());
            vec![(
                "UNRESOLVED".into(),
                format!("ARR {} #{record}", fid.unwrap_or("implicit")),
            )]
        }
    }
}

pub fn get_str<'a>(m: &'a Mapping, key: &str) -> Option<&'a str> {
    m.get(key).and_then(Value::as_str)
}

pub fn get_u64(m: &Mapping, key: &str) -> Option<u64> {
    m.get(key).and_then(Value::as_u64)
}

pub fn get_bool(m: &Mapping, key: &str) -> bool {
    m.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// One line, e.g. `READ=PIN1 UPDATE=ADM1 DEACTIVATE=ADM1 ACTIVATE=ADM1`.
pub fn access_summary(access: Option<&Mapping>) -> String {
    let Some(access) = access.filter(|a| !a.is_empty()) else {
        return "(none)".into();
    };
    let cond = |op: &str| access.get(op).and_then(Value::as_str).map(str::to_owned);
    let mut parts: Vec<String> = COMPARED_OPERATIONS
        .iter()
        .filter_map(|op| cond(op).map(|c| format!("{op}={c}")))
        .collect();
    for (k, v) in access {
        let (Some(k), Some(v)) = (k.as_str(), v.as_str()) else {
            continue;
        };
        if !COMPARED_OPERATIONS.contains(&k) {
            parts.push(format!("{k}={v}"));
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_hex;

    fn rules(pairs: &[(&str, &str)]) -> Rules {
        pairs
            .iter()
            .map(|&(a, b)| (a.to_owned(), b.to_owned()))
            .collect()
    }

    #[test]
    fn compact_examples() {
        // TS 102 221 §9.2.5 examples and Annex E.2.3.
        assert_eq!(
            decode_compact(&parse_hex("01 00").unwrap(), false),
            rules(&[("READ", "ALW")])
        );
        assert_eq!(
            decode_compact(&parse_hex("03 10 00").unwrap(), false),
            rules(&[("UPDATE", "USER_AUTH"), ("READ", "ALW")])
        );
        assert_eq!(
            decode_compact(&parse_hex("1B 90 90 90 00").unwrap(), false),
            rules(&[
                ("ACTIVATE", "USER_AUTH"),
                ("DEACTIVATE", "USER_AUTH"),
                ("UPDATE", "USER_AUTH"),
                ("READ", "ALW")
            ])
        );
        assert_eq!(
            decode_compact(&parse_hex("03 FF 00").unwrap(), false),
            rules(&[("UPDATE", "NEV"), ("READ", "ALW")])
        );
    }

    #[test]
    fn expanded_examples() {
        // TS 102 221 Annex E.3.4, EF_PL.
        let pl = parse_hex(
            "80 01 02 A0 10 A4 06 83 01 01 95 01 08 A4 06 83 01 02 95 01 08 80 01 01 90 00",
        )
        .unwrap();
        assert_eq!(
            decode_expanded(&pl, false),
            rules(&[("UPDATE", "OR(PIN1,PIN1_APP2)"), ("READ", "ALW")])
        );
        let inc = parse_hex("84 01 32 A4 06 83 01 0A 95 01 08").unwrap();
        assert_eq!(decode_expanded(&inc, false), rules(&[("INCREASE", "ADM1")]));
        let and = parse_hex("80 01 01 A4 06 83 01 01 95 01 08 A4 06 83 01 81 95 01 08").unwrap();
        assert_eq!(
            decode_expanded(&and, false),
            rules(&[("READ", "AND(PIN1,PIN2)")])
        );
        let nev = parse_hex("80 01 01 90 00 80 01 02 97 00").unwrap();
        assert_eq!(
            decode_expanded(&nev, false),
            rules(&[("READ", "ALW"), ("UPDATE", "NEV")])
        );
        assert_eq!(
            decode_expanded(&parse_hex("80 01 40 97 00").unwrap(), true),
            rules(&[("DELETE_SELF", "NEV")])
        );
    }

    fn access_of(info: &Mapping) -> Rules {
        let a = info
            .get("access")
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        a.iter()
            .map(|(k, v)| {
                (
                    k.as_str().unwrap().to_owned(),
                    v.as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn applet_transparent_ef() {
        let fcp = parse_hex(
            "62 40 82 02 41 21 83 02 6F 22 8A 01 05 AB 2C 80 01 01 A4 06 83 01 01 95 01 08 \
             80 01 02 A4 06 83 01 0A 95 01 08 80 01 08 A4 06 83 01 0A 95 01 08 \
             80 01 10 A4 06 83 01 01 95 01 08 80 02 00 0A 88 01 20",
        )
        .unwrap();
        let mut info = parse_fcp(&fcp);
        assert_eq!(get_str(&info, "structure"), Some("transparent"));
        assert_eq!(get_str(&info, "fid"), Some("6F22"));
        assert_eq!(get_u64(&info, "file_size"), Some(10));
        assert_eq!(get_u64(&info, "sfi"), Some(4));
        assert_eq!(get_str(&info, "lifecycle_name"), Some("activated"));
        assert!(get_bool(&info, "shareable"));
        resolve_access(&mut info, &|_, _| None);
        assert_eq!(
            access_of(&info),
            rules(&[
                ("READ", "PIN1"),
                ("UPDATE", "ADM1"),
                ("DEACTIVATE", "ADM1"),
                ("ACTIVATE", "PIN1")
            ])
        );
    }

    #[test]
    fn linear_fixed_descriptor() {
        let info =
            parse_fcp(&parse_hex("62 0D 82 05 42 21 00 19 05 83 02 6F 28 8C 01 00").unwrap());
        assert_eq!(get_str(&info, "structure"), Some("linear-fixed"));
        assert_eq!(get_u64(&info, "record_length"), Some(25));
        assert_eq!(get_u64(&info, "num_records"), Some(5));
    }

    #[test]
    fn referenced_rules() {
        let mut info =
            parse_fcp(&parse_hex("62 0D 82 02 41 21 83 02 6F 22 8B 03 6F 06 02").unwrap());
        let arr =
            parse_hex("80 01 01 A4 06 83 01 01 95 01 08 80 01 1A A4 06 83 01 0A 95 01 08").unwrap();
        resolve_access(&mut info, &|fid, rec| {
            (fid == Some("6F06") && rec == 2).then(|| arr.clone())
        });
        let mut got = access_of(&info);
        got.sort();
        assert_eq!(
            got,
            rules(&[
                ("ACTIVATE", "ADM1"),
                ("DEACTIVATE", "ADM1"),
                ("READ", "PIN1"),
                ("UPDATE", "ADM1")
            ])
        );
        let mut missing = parse_fcp(&parse_hex("62 09 82 02 41 21 8B 03 6F 06 09").unwrap());
        assert_eq!(
            resolve_access(&mut missing, &|_, _| None),
            rules(&[("UNRESOLVED", "ARR 6F06 #9")])
        );
        let by_seid = parse_arr_ref(&parse_hex("6F 06 01 03 02 04").unwrap());
        assert_eq!(
            by_seid
                .get("by_seid")
                .and_then(Value::as_sequence)
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn df_with_pin_status() {
        let info = parse_fcp(
            &parse_hex(
                "62 30 82 02 78 21 83 02 7F FF 84 10 A0 00 00 03 43 10 02 F3 10 FF FF 89 02 00 00 FF \
                 A5 03 80 01 71 8A 01 05 C6 0C 90 01 60 83 01 01 83 01 81 83 01 0A",
            )
            .unwrap(),
        );
        assert!(get_bool(&info, "is_df"));
        assert_eq!(
            get_str(&info, "df_name"),
            Some("A0000003431002F310FFFF89020000FF")
        );
        let keys = info["pin_status"]["keys"].as_sequence().unwrap();
        let names: Vec<&str> = keys.iter().map(|k| k["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["PIN1", "PIN2", "ADM1"]);
        let enabled: Vec<bool> = keys
            .iter()
            .map(|k| k["enabled"].as_bool().unwrap())
            .collect();
        assert_eq!(enabled, [false, true, true]);
        assert_eq!(
            get_u64(
                &parse_fcp(&parse_hex("62 06 82 02 41 21 88 00").unwrap()),
                "sfi"
            ),
            Some(0)
        );
    }

    #[test]
    fn names_and_summary() {
        assert_eq!(key_ref_name(0x0B), "ADM2");
        assert_eq!(key_ref_name(0x11), "UPIN");
        assert_eq!(key_ref_name(0x83), "PIN2_APP3");
        let mut a = Mapping::new();
        for (k, v) in [("ACTIVATE", "ADM1"), ("READ", "ALW"), ("UPDATE", "PIN1")] {
            a.insert(s(k), s(v));
        }
        assert_eq!(
            access_summary(Some(&a)),
            "READ=ALW UPDATE=PIN1 ACTIVATE=ADM1"
        );
    }
}
