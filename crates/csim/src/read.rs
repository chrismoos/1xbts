//! Reading and decoding the EFs a profile writes.

use crate::card::{Card, CardError};
use crate::codec::{self, StoredId};
use crate::profile::{Profile, SpnValue};
use crate::{Fid, hex_spaced};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Transparent(usize),
    Record(u8),
}

/// The EFs `csim read` and `csim verify` read: (FID, name, how).
const READ_TABLE: [(u16, &str, Kind); 29] = [
    (0x2FE2, "ICCID", Kind::Transparent(10)),
    (0x6F22, "IMSI_M", Kind::Transparent(10)),
    (0x6F23, "IMSI_T", Kind::Transparent(10)),
    (0x6F25, "AH", Kind::Transparent(2)),
    (0x6F28, "CDMAHOME", Kind::Record(5)),
    (0x6F2C, "ACCOLC", Kind::Transparent(1)),
    (0x6F31, "RUIMID", Kind::Transparent(8)),
    (0x6F32, "CSIM_ST", Kind::Transparent(10)),
    (0x6F38, "ESN_ME", Kind::Transparent(8)),
    (0x6F3A, "LI", Kind::Transparent(4)),
    (0x6F42, "USGIND", Kind::Transparent(1)),
    (0x6F43, "AD", Kind::Transparent(3)),
    (0x6F44, "MDN", Kind::Record(11)),
    (0x6F45, "MAXPRL", Kind::Transparent(4)),
    (0x6F33, "SPC", Kind::Transparent(3)),
    (0x6F46, "SPCS", Kind::Transparent(1)),
    (0x6F47, "ECC", Kind::Transparent(15)),
    (0x6F41, "SPN", Kind::Transparent(35)),
    (0x6F4C, "SIPUPP", Kind::Transparent(128)),
    (0x6F4D, "MIPUPP", Kind::Transparent(192)),
    (0x6F4E, "SIPSP", Kind::Transparent(1)),
    (0x6F4F, "MIPSP", Kind::Transparent(2)),
    (0x6F56, "HRPDCAP", Kind::Transparent(3)),
    (0x6F57, "HRPDUPP", Kind::Transparent(83)),
    (0x6F6A, "AUTH_CAP", Kind::Record(5)),
    (0x6F6D, "GID1", Kind::Transparent(4)),
    (0x6F6E, "GID2", Kind::Transparent(4)),
    (0x6F74, "SF_EUIMID", Kind::Transparent(7)),
    (0x6F75, "EST", Kind::Transparent(1)),
];

pub struct ReadEntry {
    pub label: &'static str,
    /// The content, or the status word that refused it.
    pub content: Result<Vec<u8>, u16>,
}

impl ReadEntry {
    /// Decoded fields as (name, value) for printing.
    pub fn decoded(&self) -> Vec<(String, String)> {
        let Ok(d) = &self.content else {
            return Vec::new();
        };
        decode(self.label, d).unwrap_or_else(|e| vec![("decode error".into(), e)])
    }
}

/// A user profile EF that was never written: all FF, or an empty block.
fn user_profile_unset(d: &[u8]) -> bool {
    d.iter().all(|&b| b == 0xFF) || d.first() == Some(&0)
}

fn kv(k: &str, v: impl ToString) -> (String, String) {
    (k.to_owned(), v.to_string())
}

/// Contents longer than this are shown by size, not as hex.
const DESCRIBE_HEX_MAX: usize = 24;

/// One line describing an EF's content: the decoded fields where there is
/// a decoder, otherwise the hex, or the size for long content.
pub fn describe(label: &str, d: &[u8]) -> String {
    match decode(label, d) {
        Ok(fields) if !fields.is_empty() => match label {
            "IMSI_M" | "IMSI_T" => fields
                .iter()
                .find(|(k, _)| k == "imsi")
                .map_or(String::new(), |(_, v)| v.clone()),
            _ if fields.len() == 1 => fields[0].1.clone(),
            _ => fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" "),
        },
        _ if d.len() <= DESCRIBE_HEX_MAX => hex_spaced(d),
        _ => format!("{} bytes", d.len()),
    }
}

fn decode(label: &str, d: &[u8]) -> Result<Vec<(String, String)>, String> {
    let err = |e: codec::CodecError| e.0;
    Ok(match label {
        "ICCID" => vec![kv("decoded", codec::decode_iccid(d))],
        "IMSI_M" | "IMSI_T" => {
            let i = codec::decode_imsi(d).map_err(err)?;
            vec![
                kv("class", i.class),
                kv("addr_num", i.addr_num),
                kv("programmed", i.programmed),
                kv("mcc", &i.mcc),
                kv("digits_11_12", &i.digits_11_12),
                kv("min_or_msin", &i.min_or_msin),
                kv("imsi", &i.imsi),
            ]
        }
        "CDMAHOME" => {
            let h = codec::decode_cdmahome_record(d).map_err(err)?;
            vec![kv("sid", h.sid), kv("nid", h.nid), kv("band", h.band)]
        }
        "RUIMID" => vec![kv(
            "uim_id",
            codec::decode_ruimid(d).unwrap_or_else(|| "None".into()),
        )],
        "CSIM_ST" | "EST" => {
            let services = codec::decode_service_table(d);
            let text = if services.is_empty() {
                "none".to_owned()
            } else {
                services
                    .iter()
                    .map(|n| format!("n{n}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            vec![kv("decoded", text)]
        }
        "ACCOLC" if !d.is_empty() => vec![kv("decoded", d[0] & 0x0F)],
        "AH" if d.len() >= 2 => vec![kv("decoded", u16::from_le_bytes([d[0], d[1]]))],
        "ESN_ME" => match codec::decode_esn_me(d).map_err(err)? {
            StoredId::None => vec![kv("stored", "none")],
            StoredId::Esn(v) => vec![kv("stored", "ESN"), kv("value", v)],
            StoredId::Meid(v) => vec![kv("stored", "MEID"), kv("value", v)],
            StoredId::Other(n, v) => vec![kv("stored", format!("{n}-byte id")), kv("value", v)],
        },
        "LI" => codec::decode_li(d)
            .into_iter()
            .map(|(enc, lang)| kv(&lang, enc))
            .collect(),
        "USGIND" => {
            let u = codec::decode_usgind(d);
            vec![
                kv("esn_source", if u.use_uim_id { "UIM_ID" } else { "ESN_ME" }),
                kv(
                    "meid_source",
                    if u.use_sf_euimid { "SF_EUIMID" } else { "MEID" },
                ),
            ]
        }
        "AD" => vec![kv(
            "operation_mode",
            codec::operation_mode_name(d.first().copied().unwrap_or(0)),
        )],
        "MDN" => vec![kv("digits", codec::decode_mdn_record(d).map_err(err)?)],
        "MAXPRL" if d.len() >= 4 => {
            vec![
                kv("prl", u16::from_be_bytes([d[0], d[1]])),
                kv("eprl", u16::from_be_bytes([d[2], d[3]])),
            ]
        }
        "SPC" => vec![kv("decoded", codec::decode_spc(d))],
        "ECC" => vec![kv("decoded", codec::decode_ecc(d).join(" "))],
        "SPN" => {
            let s = codec::decode_spn(d).map_err(err)?;
            vec![
                kv("display_condition", s.display_condition),
                kv("char_encoding", codec::encoding_name(s.char_encoding)),
                kv("language_indicator", s.language_indicator),
                kv("name", s.name),
            ]
        }
        "HRPDCAP" => {
            let h = codec::decode_hrpdcap(d).map_err(err)?;
            vec![
                kv("max_nai_length", h.max_nai_length),
                kv("max_ss_length", h.max_ss_length),
                kv("chap_supported", h.chap_supported),
            ]
        }
        "SF_EUIMID" => vec![kv("decoded", codec::decode_sf_euimid(d))],
        "SIPUPP" | "MIPUPP" | "HRPDUPP" if user_profile_unset(d) => vec![kv("decoded", "not set")],
        "SIPUPP" => vec![kv(
            "decoded",
            codec::decode_sipupp(d)
                .map_err(err)?
                .iter()
                .map(|n| format!("[{}] {} {}", n.index, n.nai, n.auth.name()))
                .collect::<Vec<_>>()
                .join(" | "),
        )],
        "MIPUPP" => {
            let m = codec::decode_mipupp(d).map_err(err)?;
            let mut text: Vec<String> = m
                .nais
                .iter()
                .map(|n| {
                    let spi = |auth: codec::MipAuth, spi: Option<u32>| match spi {
                        Some(spi) => format!("{} spi {spi}", auth.name()),
                        None => auth.name().to_owned(),
                    };
                    format!(
                        "[{}] {} home {} ha {}/{} rt {} mn_aaa {} mn_ha {}",
                        n.index,
                        n.nai,
                        n.home_address,
                        n.primary_home_agent,
                        n.secondary_home_agent,
                        if n.reverse_tunneling { "on" } else { "off" },
                        spi(n.mn_aaa_auth, n.mn_aaa_spi),
                        spi(n.mn_ha_auth, n.mn_ha_spi),
                    )
                })
                .collect();
            if let Some(r) = m.retry {
                text.push(format!(
                    "retry {} first {} ms rereg {} min",
                    r.max_retries, r.first_retry_timeout_ms, r.rereg_threshold_min
                ));
            }
            vec![kv("decoded", text.join(" | "))]
        }
        "HRPDUPP" => {
            let h = codec::decode_hrpdupp(d).map_err(err)?;
            vec![kv("decoded", format!("{} {}", h.nai, h.auth.name()))]
        }
        "SIPSP" | "MIPSP" => match codec::decode_active_nai(d) {
            Some(i) => vec![kv("decoded", format!("active NAI [{i}]"))],
            None => Vec::new(),
        },
        _ => Vec::new(),
    })
}

pub fn read_all(card: &mut Card) -> Vec<ReadEntry> {
    READ_TABLE
        .iter()
        .map(|&(fid, label, kind)| {
            let read = |card: &mut Card| -> Result<Vec<u8>, CardError> {
                card.select_fid(Fid(fid))?;
                match kind {
                    Kind::Transparent(len) => card.read_binary(len, 0),
                    Kind::Record(len) => card.read_record(1, len),
                }
            };
            ReadEntry {
                label,
                content: read(card).map_err(|e| e.sw().unwrap_or(0)),
            }
        })
        .collect()
}

pub fn print_read_results(entries: &[ReadEntry]) {
    for e in entries {
        match &e.content {
            Err(sw) => println!("  {:14} SW={sw:04X}", e.label),
            Ok(data) => {
                println!("  {:14} raw: {}", e.label, hex_spaced(data));
                for (k, v) in e.decoded() {
                    println!("  {:14}   {k}: {v}", "");
                }
            }
        }
    }
}

fn content<'a>(entries: &'a [ReadEntry], label: &str) -> Option<&'a [u8]> {
    entries
        .iter()
        .find(|e| e.label == label)
        .and_then(|e| e.content.as_deref().ok())
}

pub struct FieldCheck {
    pub label: &'static str,
    pub wrote: String,
    pub read: Option<String>,
}

impl FieldCheck {
    pub fn ok(&self) -> bool {
        self.read.as_deref() == Some(self.wrote.as_str())
    }
}

/// The profile values `csim verify` compares with what it read back.
pub fn verify_checks(p: &Profile, entries: &[ReadEntry]) -> Vec<FieldCheck> {
    let imsi = |label: &str| {
        content(entries, label)
            .and_then(|d| codec::decode_imsi(d).ok())
            .map(|i| i.imsi)
    };
    let gid = |label: &str| {
        content(entries, label).map(|d| {
            d.iter()
                .fold(0u64, |a, &b| (a << 8) | u64::from(b))
                .to_string()
        })
    };
    let spn_name = p.spn.as_ref().and_then(|s| match s {
        SpnValue::Name(n) => Some(n.clone()),
        SpnValue::Fields(f) => f.name.clone(),
    });
    let join = |nais: Vec<&str>| nais.join(" ");
    let sip_nais = content(entries, "SIPUPP")
        .filter(|d| !user_profile_unset(d))
        .and_then(|d| codec::decode_sipupp(d).ok())
        .map(|l| join(l.iter().map(|n| n.nai.as_str()).collect()));
    let mip_nais = content(entries, "MIPUPP")
        .filter(|d| !user_profile_unset(d))
        .and_then(|d| codec::decode_mipupp(d).ok())
        .map(|m| join(m.nais.iter().map(|n| n.nai.as_str()).collect()));
    let candidates: [(&'static str, Option<String>, Option<String>); 11] = [
        (
            "ICCID",
            p.iccid.as_ref().map(|v| v.text()),
            content(entries, "ICCID").map(codec::decode_iccid),
        ),
        (
            "IMSI_M",
            p.imsi_m.as_ref().map(|v| v.text()),
            imsi("IMSI_M"),
        ),
        (
            "IMSI_T",
            p.imsi_t.as_ref().map(|v| v.text()),
            imsi("IMSI_T"),
        ),
        (
            "MDN",
            p.mdn.as_ref().map(|v| v.text()),
            content(entries, "MDN").and_then(|d| codec::decode_mdn_record(d).ok()),
        ),
        (
            "CDMAHOME-sid",
            p.cdmahome
                .as_ref()
                .and_then(|h| h.first())
                .and_then(|e| e.sid)
                .map(|s| s.to_string()),
            content(entries, "CDMAHOME")
                .and_then(|d| codec::decode_cdmahome_record(d).ok())
                .map(|h| h.sid.to_string()),
        ),
        (
            "SPN-name",
            spn_name,
            content(entries, "SPN")
                .and_then(|d| codec::decode_spn(d).ok())
                .map(|s| s.name),
        ),
        ("GID1", p.gid1.map(|g| g.to_string()), gid("GID1")),
        ("GID2", p.gid2.map(|g| g.to_string()), gid("GID2")),
        (
            "SIPUPP-nai",
            p.simple_ip
                .as_ref()
                .map(|l| join(l.iter().map(|n| n.nai.as_str()).collect())),
            sip_nais,
        ),
        (
            "MIPUPP-nai",
            p.mobile_ip
                .as_ref()
                .map(|m| join(m.nais.iter().map(|n| n.nai.as_str()).collect())),
            mip_nais,
        ),
        (
            "HRPDUPP-nai",
            p.hrpd_access.as_ref().map(|h| h.nai.clone()),
            content(entries, "HRPDUPP")
                .filter(|d| !user_profile_unset(d))
                .and_then(|d| codec::decode_hrpdupp(d).ok())
                .map(|h| h.nai),
        ),
    ];
    candidates
        .into_iter()
        .filter_map(|(label, wrote, read)| wrote.map(|wrote| FieldCheck { label, wrote, read }))
        .collect()
}

/// A profile rebuilt from what was read, as YAML.
pub fn read_back_profile(entries: &[ReadEntry]) -> serde_yaml::Mapping {
    use serde_yaml::Value;
    let mut out = serde_yaml::Mapping::new();
    let mut put = |k: &str, v: Value| {
        out.insert(k.into(), v);
    };
    if let Some(d) = content(entries, "ICCID") {
        put("iccid", codec::decode_iccid(d).into());
    }
    for (label, key) in [("IMSI_M", "imsi_m"), ("IMSI_T", "imsi_t")] {
        if let Some(i) = content(entries, label).and_then(|d| codec::decode_imsi(d).ok()) {
            put(key, i.imsi.into());
        }
    }
    if let Some(m) = content(entries, "MDN").and_then(|d| codec::decode_mdn_record(d).ok()) {
        put("mdn", m.into());
    }
    if let Some(h) =
        content(entries, "CDMAHOME").and_then(|d| codec::decode_cdmahome_record(d).ok())
    {
        let mut e = serde_yaml::Mapping::new();
        e.insert("sid".into(), u64::from(h.sid).into());
        e.insert("nid".into(), u64::from(h.nid).into());
        e.insert("band".into(), u64::from(h.band).into());
        put("cdmahome", Value::Sequence(vec![Value::Mapping(e)]));
    }
    if let Some(d) = content(entries, "ECC") {
        put(
            "ecc",
            Value::Sequence(codec::decode_ecc(d).into_iter().map(Value::from).collect()),
        );
    }
    if let Some(d) = content(entries, "AH").filter(|d| d.len() == 2) {
        put("ah", u64::from(u16::from_le_bytes([d[0], d[1]])).into());
    }
    for (label, key) in [("CSIM_ST", "services"), ("EST", "enabled_services")] {
        if let Some(d) = content(entries, label) {
            put(
                key,
                Value::Sequence(
                    codec::decode_service_table(d)
                        .into_iter()
                        .map(|n| Value::from(u64::from(n)))
                        .collect(),
                ),
            );
        }
    }
    match content(entries, "ESN_ME").and_then(|d| codec::decode_esn_me(d).ok()) {
        Some(StoredId::Esn(v)) => put("esn", v.into()),
        Some(StoredId::Meid(v)) => put("meid", v.into()),
        _ => {}
    }
    if let Some(r) = content(entries, "RUIMID").and_then(codec::decode_ruimid) {
        put("ruimid", r.into());
    }
    if let Some(d) = content(entries, "SPC") {
        put("spc", codec::decode_spc(d).into());
    }
    if let Some(s) = content(entries, "SPN")
        .and_then(|d| codec::decode_spn(d).ok())
        .filter(|s| !s.name.is_empty())
    {
        let mut m = serde_yaml::Mapping::new();
        m.insert("name".into(), s.name.into());
        m.insert(
            "display_condition".into(),
            u64::from(s.display_condition).into(),
        );
        put("spn", Value::Mapping(m));
    }
    let text = |v: &str| Value::from(v.to_owned());
    if let Some(list) = content(entries, "SIPUPP")
        .filter(|d| !user_profile_unset(d))
        .and_then(|d| codec::decode_sipupp(d).ok())
    {
        let list = list
            .iter()
            .map(|n| {
                let mut m = serde_yaml::Mapping::new();
                m.insert("index".into(), u64::from(n.index).into());
                m.insert("nai".into(), text(&n.nai));
                m.insert("auth".into(), text(n.auth.name()));
                Value::Mapping(m)
            })
            .collect();
        put("simple_ip", Value::Sequence(list));
    }
    if let Some(mip) = content(entries, "MIPUPP")
        .filter(|d| !user_profile_unset(d))
        .and_then(|d| codec::decode_mipupp(d).ok())
    {
        let mut out = serde_yaml::Mapping::new();
        if let Some(r) = mip.retry {
            let mut m = serde_yaml::Mapping::new();
            m.insert("max_retries".into(), u64::from(r.max_retries).into());
            m.insert(
                "first_retry_timeout_ms".into(),
                u64::from(r.first_retry_timeout_ms).into(),
            );
            m.insert(
                "rereg_threshold_min".into(),
                u64::from(r.rereg_threshold_min).into(),
            );
            out.insert("retry".into(), Value::Mapping(m));
        }
        let nais = mip
            .nais
            .iter()
            .map(|n| {
                let mut m = serde_yaml::Mapping::new();
                m.insert("index".into(), u64::from(n.index).into());
                m.insert("nai".into(), text(&n.nai));
                m.insert("reverse_tunneling".into(), n.reverse_tunneling.into());
                m.insert("home_address".into(), text(&n.home_address.to_string()));
                m.insert(
                    "primary_home_agent".into(),
                    text(&n.primary_home_agent.to_string()),
                );
                m.insert(
                    "secondary_home_agent".into(),
                    text(&n.secondary_home_agent.to_string()),
                );
                m.insert("mn_aaa_auth".into(), text(n.mn_aaa_auth.name()));
                if let Some(spi) = n.mn_aaa_spi {
                    m.insert("mn_aaa_spi".into(), u64::from(spi).into());
                }
                m.insert("mn_ha_auth".into(), text(n.mn_ha_auth.name()));
                if let Some(spi) = n.mn_ha_spi {
                    m.insert("mn_ha_spi".into(), u64::from(spi).into());
                }
                Value::Mapping(m)
            })
            .collect();
        out.insert("nais".into(), Value::Sequence(nais));
        put("mobile_ip", Value::Mapping(out));
    }
    if let Some(h) = content(entries, "HRPDUPP")
        .filter(|d| !user_profile_unset(d))
        .and_then(|d| codec::decode_hrpdupp(d).ok())
    {
        let mut m = serde_yaml::Mapping::new();
        m.insert("nai".into(), text(&h.nai));
        m.insert("auth".into(), text(h.auth.name()));
        put("hrpd_access", Value::Mapping(m));
    }
    out
}
