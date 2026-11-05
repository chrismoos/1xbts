//! Subscriber profiles: the YAML a card is personalized from.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::codec::{self, Language, SPC_DIGITS};
use crate::{akey, ef_table, parse_hex, prl};

/// A YAML scalar read as text, so that an unquoted number and a quoted
/// string give the same value.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Scalar {
    Text(String),
    Int(i64),
    UInt(u64),
}

impl Scalar {
    pub fn text(&self) -> String {
        match self {
            Scalar::Text(s) => s.clone(),
            Scalar::Int(n) => n.to_string(),
            Scalar::UInt(n) => n.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CdmaHomeEntry {
    pub sid: Option<i64>,
    pub nid: Option<i64>,
    pub band: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LanguageEntry {
    Code(String),
    Pair(u8, u8),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Languages {
    One(String),
    Many(Vec<LanguageEntry>),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpnFields {
    pub name: Option<String>,
    pub display_condition: Option<u8>,
    pub char_encoding: Option<u8>,
    pub language_indicator: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SpnValue {
    Name(String),
    Fields(SpnFields),
}

impl SpnValue {
    /// Whether the profile asks for EF_SPN: a non-empty name, or a map
    /// with any field.
    pub fn is_set(&self) -> bool {
        match self {
            SpnValue::Name(name) => !name.is_empty(),
            SpnValue::Fields(f) => {
                f.name.is_some()
                    || f.display_condition.is_some()
                    || f.char_encoding.is_some()
                    || f.language_indicator.is_some()
            }
        }
    }

    pub fn to_spn(&self) -> codec::Spn {
        let f = match self {
            SpnValue::Name(name) => SpnFields {
                name: Some(name.clone()),
                ..SpnFields::default()
            },
            SpnValue::Fields(f) => f.clone(),
        };
        codec::Spn {
            name: f.name.unwrap_or_default(),
            display_condition: f.display_condition.unwrap_or(0),
            char_encoding: f.char_encoding.unwrap_or(codec::ENC_OCTET),
            language_indicator: f.language_indicator.unwrap_or(LANGUAGE_EN),
        }
    }
}

const LANGUAGE_EN: u8 = 0x01;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsgInd {
    #[serde(default)]
    pub use_uim_id: bool,
    #[serde(default)]
    pub use_sf_euimid: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HrpdCapFields {
    pub max_nai_length: Option<u8>,
    pub max_ss_length: Option<u8>,
    pub chap_supported: Option<bool>,
}

/// A shared secret is given as hex in `secret` or as text in
/// `secret_text`, whose UTF-8 bytes are the secret.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IpSecret {
    pub class: Scalar,
    #[serde(default)]
    pub index: u8,
    pub secret: Option<String>,
    pub secret_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimpleIpNai {
    #[serde(default)]
    pub index: u8,
    pub nai: String,
    /// none, chap, pap or chap_pap. Default chap.
    pub auth: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileIpRetry {
    pub max_retries: u8,
    pub first_retry_timeout_ms: u16,
    pub rereg_threshold_min: u8,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileIpNai {
    #[serde(default)]
    pub index: u8,
    pub nai: String,
    #[serde(default)]
    pub reverse_tunneling: bool,
    pub home_address: Option<String>,
    pub primary_home_agent: Option<String>,
    pub secondary_home_agent: Option<String>,
    /// none or md5. Default md5.
    pub mn_aaa_auth: Option<String>,
    pub mn_aaa_spi: Option<u32>,
    /// none or md5. Default md5.
    pub mn_ha_auth: Option<String>,
    pub mn_ha_spi: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileIp {
    pub retry: Option<MobileIpRetry>,
    pub nais: Vec<MobileIpNai>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HrpdAccess {
    pub nai: String,
    /// none or chap. Default chap.
    pub auth: Option<String>,
}

/// Packet-data shared-secret classes of the applet's LOAD_IPSEC command.
const IP_SECRET_CLASSES: [(&str, u8); 4] =
    [("sip_chap", 0), ("mn_ha", 1), ("mn_aaa", 2), ("hrpd", 3)];

impl IpSecret {
    /// The class number, from a class name or 0-3.
    pub fn class_number(&self) -> Option<u8> {
        let text = self.class.text().to_ascii_lowercase();
        IP_SECRET_CLASSES
            .iter()
            .find(|(name, _)| *name == text)
            .map(|&(_, n)| n)
            .or_else(|| text.parse::<u8>().ok().filter(|n| *n <= 3))
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        match (&self.secret, &self.secret_text) {
            (Some(hex), None) => parse_hex(hex).map_err(|e| format!("secret: {e}")),
            (None, Some(text)) => Ok(text.as_bytes().to_vec()),
            (Some(_), Some(_)) => Err("set one of 'secret' and 'secret_text'".into()),
            (None, None) => Err("missing 'secret' or 'secret_text'".into()),
        }
    }
}

impl SimpleIpNai {
    pub fn to_codec(&self) -> Result<codec::SipNai, String> {
        let auth = self.auth.as_deref().unwrap_or("chap");
        Ok(codec::SipNai {
            index: self.index,
            nai: self.nai.clone(),
            auth: codec::SipAuth::from_name(auth)
                .ok_or_else(|| format!("auth must be none, chap, pap or chap_pap, got {auth:?}"))?,
        })
    }
}

fn address(key: &str, v: &Option<String>) -> Result<Ipv4Addr, String> {
    match v {
        None => Ok(Ipv4Addr::UNSPECIFIED),
        Some(a) => a
            .parse()
            .map_err(|_| format!("{key} must be an IPv4 address, got {a:?}")),
    }
}

fn mip_auth(key: &str, v: &Option<String>) -> Result<codec::MipAuth, String> {
    let name = v.as_deref().unwrap_or("md5");
    codec::MipAuth::from_name(name)
        .ok_or_else(|| format!("{key} must be none or md5, got {name:?}"))
}

impl MobileIp {
    pub fn to_codec(&self) -> Result<codec::MipProfile, String> {
        let nais = self
            .nais
            .iter()
            .map(|n| {
                Ok(codec::MipNai {
                    index: n.index,
                    nai: n.nai.clone(),
                    reverse_tunneling: n.reverse_tunneling,
                    home_address: address("home_address", &n.home_address)?,
                    primary_home_agent: address("primary_home_agent", &n.primary_home_agent)?,
                    secondary_home_agent: address("secondary_home_agent", &n.secondary_home_agent)?,
                    mn_aaa_auth: mip_auth("mn_aaa_auth", &n.mn_aaa_auth)?,
                    mn_aaa_spi: n.mn_aaa_spi,
                    mn_ha_auth: mip_auth("mn_ha_auth", &n.mn_ha_auth)?,
                    mn_ha_spi: n.mn_ha_spi,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(codec::MipProfile {
            retry: self.retry.as_ref().map(|r| codec::MipRetry {
                max_retries: r.max_retries,
                first_retry_timeout_ms: r.first_retry_timeout_ms,
                rereg_threshold_min: r.rereg_threshold_min,
            }),
            nais,
        })
    }
}

impl HrpdAccess {
    pub fn to_codec(&self) -> Result<codec::HrpdUpp, String> {
        let auth = self.auth.as_deref().unwrap_or("chap");
        Ok(codec::HrpdUpp {
            nai: self.nai.clone(),
            auth: codec::HrpdAuth::from_name(auth)
                .ok_or_else(|| format!("auth must be none or chap, got {auth:?}"))?,
        })
    }
}

/// One subscriber profile. Unknown keys are an error.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub pin1: Option<Scalar>,
    pub pin2: Option<Scalar>,
    pub puk1: Option<Scalar>,
    pub puk2: Option<Scalar>,
    pub iccid: Option<Scalar>,
    pub imsi_m: Option<Scalar>,
    pub imsi_t: Option<Scalar>,
    pub mdn: Option<Scalar>,
    pub cdmahome: Option<Vec<CdmaHomeEntry>>,
    pub accolc: Option<i64>,
    pub ecc: Option<Vec<Scalar>>,
    pub ah: Option<i64>,
    pub language: Option<Languages>,
    pub operation_mode: Option<i64>,
    pub services: Option<Vec<serde_yaml::Value>>,
    pub enabled_services: Option<Vec<serde_yaml::Value>>,
    pub spn: Option<SpnValue>,
    pub usgind: Option<UsgInd>,
    pub esn: Option<Scalar>,
    pub meid: Option<Scalar>,
    pub ruimid: Option<Scalar>,
    pub sf_euimid: Option<Scalar>,
    pub gid1: Option<i64>,
    pub gid2: Option<i64>,
    pub hrpdcap: Option<HrpdCapFields>,
    pub mms_auth_capability: Option<i64>,
    pub spc: Option<Scalar>,
    pub prl: Option<String>,
    pub eprl: Option<String>,
    pub prl_file: Option<PathBuf>,
    pub eprl_file: Option<PathBuf>,
    pub raw: Option<serde_yaml::Mapping>,
    pub a_key: Option<Scalar>,
    pub ssd: Option<String>,
    pub aka_root_key: Option<String>,
    pub wlan_root_key: Option<String>,
    pub ip_secrets: Option<Vec<IpSecret>>,
    pub simple_ip: Option<Vec<SimpleIpNai>>,
    pub mobile_ip: Option<MobileIp>,
    pub hrpd_access: Option<HrpdAccess>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_yaml::Error,
    },
}

impl Profile {
    /// Reads a profile. `prl_file` and `eprl_file` name binary PRL files,
    /// relative to the profile, and are loaded into `prl` and `eprl`.
    pub fn load(path: &Path) -> Result<Profile, ProfileError> {
        let io = |p: &Path, source| ProfileError::Io {
            path: p.to_owned(),
            source,
        };
        let text = std::fs::read_to_string(path).map_err(|e| io(path, e))?;
        let mut profile = Profile::from_yaml(&text).map_err(|source| ProfileError::Yaml {
            path: path.to_owned(),
            source,
        })?;
        let dir = path.parent().unwrap_or(Path::new("."));
        for (file, target) in [
            (profile.prl_file.take(), &mut profile.prl),
            (profile.eprl_file.take(), &mut profile.eprl),
        ] {
            if let Some(file) = file {
                let file = if file.is_absolute() {
                    file
                } else {
                    dir.join(file)
                };
                let bytes = std::fs::read(&file).map_err(|e| io(&file, e))?;
                *target = Some(crate::hex_upper(&bytes));
            }
        }
        Ok(profile)
    }

    pub fn from_yaml(text: &str) -> Result<Profile, serde_yaml::Error> {
        if text.trim().is_empty() {
            return Ok(Profile::default());
        }
        serde_yaml::from_str(text)
    }

    /// The EF_LI entries, English when the profile names none.
    pub fn languages(&self) -> Vec<Language> {
        match &self.language {
            None => vec![Language::Code("en".into())],
            Some(Languages::One(code)) => vec![Language::Code(code.clone())],
            Some(Languages::Many(list)) => list
                .iter()
                .map(|e| match e {
                    LanguageEntry::Code(c) => Language::Code(c.clone()),
                    LanguageEntry::Pair(enc, ind) => Language::Pair {
                        char_encoding: *enc,
                        indicator: *ind,
                    },
                })
                .collect(),
        }
    }

    /// The `raw` entries as (EF name, hex) in profile order.
    pub fn raw_entries(&self) -> Vec<(String, String)> {
        self.raw
            .iter()
            .flatten()
            .map(|(k, v)| (yaml_text(k), yaml_text(v)))
            .collect()
    }
}

pub(crate) fn yaml_text(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

/// Service numbers from a profile list. Entries that are not integers
/// come back as `Err` with the entry as written.
pub(crate) fn service_numbers(list: &Option<Vec<serde_yaml::Value>>) -> Vec<Result<u32, String>> {
    list.iter()
        .flatten()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{v:?}"))
        })
        .collect()
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn hex_len_problem(key: &str, value: &Scalar, bytes: usize) -> Option<String> {
    match parse_hex(&value.text()) {
        Ok(b) if b.len() == bytes => None,
        _ => Some(format!(
            "{key}: must be {} hex digits, got {:?}",
            bytes * 2,
            value.text()
        )),
    }
}

pub fn luhn_ok(digits: &str) -> bool {
    let parity = digits.len() % 2;
    let sum: u32 = digits
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            let mut n = u32::from(b - b'0');
            if i % 2 == parity {
                n *= 2;
                if n > 9 {
                    n -= 9;
                }
            }
            n
        })
        .sum();
    sum % 10 == 0
}

const SID_LIMIT: i64 = 1 << 15;
const NID_LIMIT: i64 = 1 << 16;
const ACCOLC_MAX: i64 = 15;
const IP_SECRET_MAX: usize = 32;
/// NAI indexes the applet keeps a secret for, per class.
const IP_SECRET_INDEXES: u8 = 4;
const IP_SECRET_SIP_CHAP: u8 = 0;
const IP_SECRET_MN_HA: u8 = 1;
const IP_SECRET_MN_AAA: u8 = 2;
const IP_SECRET_HRPD: u8 = 3;
const SECRET_KEY_LEN: usize = 16;
const PIN_MAX_LEN: usize = 8;
const PIN_MIN_LEN: usize = 4;
const PUK_LEN: usize = 8;

/// Every problem with a profile. An empty list means it can be written.
pub fn validate(p: &Profile) -> Vec<String> {
    let mut errs = Vec::new();

    if let Some(iccid) = &p.iccid {
        let iccid = iccid.text();
        let (short, long) = (codec::ICCID_DIGITS, codec::ICCID_MAX_DIGITS);
        if !is_digits(&iccid) {
            errs.push(format!("iccid: non-digit characters in {iccid:?}"));
        } else if iccid.len() != short && iccid.len() != long {
            errs.push(format!(
                "iccid: an ICCID is {short} or {long} digits including the check digit (C.S0065-B §5.2.91), got {}",
                iccid.len()
            ));
        } else if !luhn_ok(&iccid) {
            errs.push(format!("iccid: Luhn checksum fails ({iccid})"));
        }
    }

    for (key, v) in [("imsi_m", &p.imsi_m), ("imsi_t", &p.imsi_t)] {
        let Some(v) = v else { continue };
        let v = v.text();
        if !is_digits(&v) {
            errs.push(format!("{key}: non-digit in {v:?}"));
        } else if v.len() != codec::IMSI_DIGITS {
            errs.push(format!(
                "{key}: must be {} digits (class 0 IMSI), got {}",
                codec::IMSI_DIGITS,
                v.len()
            ));
        }
    }

    if let Some(mdn) = &p.mdn {
        let mdn = mdn.text();
        if !mdn
            .chars()
            .all(|c| c.is_ascii_digit() || c == '*' || c == '#')
        {
            errs.push(format!("mdn: non-digit in {mdn:?}"));
        } else if mdn.len() > codec::MDN_MAX_DIGITS {
            errs.push(format!("mdn: > {} digits", codec::MDN_MAX_DIGITS));
        }
    }

    if p.language.is_some() {
        for lang in p.languages() {
            if let Language::Code(code) = lang {
                if codec::language_indicator(&code).is_none() {
                    errs.push(format!(
                        "language: {code:?} has no C.R1001 language indicator"
                    ));
                }
            }
        }
    }

    if let Some(spc) = &p.spc {
        let spc = spc.text();
        if spc.len() != SPC_DIGITS || !is_digits(&spc) {
            errs.push(format!(
                "spc: must be {SPC_DIGITS} digits (C.S0065-B §5.2.19), got {spc:?}"
            ));
        }
    }

    for (key, ef, value) in [("prl", "PRL", &p.prl), ("eprl", "EPRL", &p.eprl)] {
        let Some(value) = value else { continue };
        match parse_hex(value) {
            Ok(data) => errs.extend(prl::check_prl_bytes(key, &data, ef_table::size_of(ef))),
            Err(_) => errs.push(format!("{key}: not hex")),
        }
    }

    for (key, ef, list) in [
        ("services", "CSIM_ST", &p.services),
        ("enabled_services", "EST", &p.enabled_services),
    ] {
        let max = 8 * ef_table::size_of(ef) as u32;
        for n in service_numbers(list) {
            match n {
                Ok(n) if (1..=max).contains(&n) => {}
                Ok(n) => errs.push(format!(
                    "{key}: entry {n} is not a service number n1..n{max}"
                )),
                Err(v) => errs.push(format!(
                    "{key}: entry {v} is not a service number n1..n{max}"
                )),
            }
        }
    }

    if p.esn.is_some() && p.meid.is_some() {
        errs.push("esn and meid are exclusive: a mobile has one or the other".into());
    }
    for (key, value, bytes) in [
        ("esn", &p.esn, codec::ESN_BYTES),
        ("meid", &p.meid, codec::MEID_BYTES),
        ("sf_euimid", &p.sf_euimid, codec::MEID_BYTES),
    ] {
        if let Some(problem) = value.as_ref().and_then(|v| hex_len_problem(key, v, bytes)) {
            errs.push(problem);
        }
    }
    if let Some(ruimid) = &p.ruimid {
        if u32::from_str_radix(&ruimid.text().replace(' ', ""), 16).is_err() {
            errs.push(format!(
                "ruimid: must be a 32-bit UIM_ID in hex, got {:?}",
                ruimid.text()
            ));
        }
    }

    for entry in p.cdmahome.iter().flatten() {
        let Some(sid) = entry.sid else {
            errs.push("cdmahome: entry missing 'sid'".into());
            continue;
        };
        if !(0..SID_LIMIT).contains(&sid) {
            errs.push(format!("cdmahome: sid {sid} out of 15-bit range"));
        }
        let nid = entry.nid.unwrap_or(0);
        if !(0..NID_LIMIT).contains(&nid) {
            errs.push(format!("cdmahome: nid {nid} out of 16-bit range"));
        }
    }

    if let Some(accolc) = p.accolc {
        if !(0..=ACCOLC_MAX).contains(&accolc) {
            errs.push(format!("accolc: must be 0-{ACCOLC_MAX}, got {accolc}"));
        }
    }

    for code in p.ecc.iter().flatten() {
        if !is_digits(&code.text()) {
            errs.push(format!("ecc: non-digit entry {:?}", code.text()));
        }
    }

    if let Some(key) = &p.a_key {
        if let Err(e) = akey::parse_a_key(&key.text()) {
            errs.push(format!("a_key: {e}"));
        }
    }
    for (key, value) in [
        ("ssd", &p.ssd),
        ("aka_root_key", &p.aka_root_key),
        ("wlan_root_key", &p.wlan_root_key),
    ] {
        let Some(value) = value else { continue };
        match parse_hex(value) {
            Ok(b) if b.len() == SECRET_KEY_LEN => {}
            Ok(b) => errs.push(format!(
                "{key}: must be {SECRET_KEY_LEN} bytes, got {}",
                b.len()
            )),
            Err(e) => errs.push(format!("{key}: parse error: {e}")),
        }
    }
    let hrpdcap = p.hrpdcap.clone().unwrap_or_default();
    let max_ss = hrpdcap.max_ss_length.unwrap_or(codec::HRPD_MIN_SS_LENGTH);
    let max_nai = hrpdcap.max_nai_length.unwrap_or(codec::HRPD_MIN_NAI_LENGTH);
    for (i, s) in p.ip_secrets.iter().flatten().enumerate() {
        if s.class_number().is_none() {
            errs.push(format!(
                "ip_secrets[{i}]: class must be one of hrpd, mn_aaa, mn_ha, sip_chap or 0-3, got {:?}",
                s.class.text()
            ));
        }
        if s.index >= IP_SECRET_INDEXES {
            errs.push(format!(
                "ip_secrets[{i}]: index must be 0-{}",
                IP_SECRET_INDEXES - 1
            ));
        }
        match s.bytes() {
            Err(e) => errs.push(format!("ip_secrets[{i}]: {e}")),
            Ok(b) if b.is_empty() => errs.push(format!("ip_secrets[{i}]: secret is empty")),
            Ok(b) if b.len() > IP_SECRET_MAX => errs.push(format!(
                "ip_secrets[{i}]: secret longer than {IP_SECRET_MAX} bytes"
            )),
            Ok(b) if s.class_number() == Some(IP_SECRET_HRPD) && b.len() > usize::from(max_ss) => {
                errs.push(format!(
                    "ip_secrets[{i}]: the hrpd secret is longer than hrpdcap max_ss_length {max_ss}"
                ))
            }
            Ok(_) => {}
        }
        // The secret's index names the NAI entry it authenticates.
        let nai_indexes: Option<Vec<u8>> = match s.class_number() {
            Some(IP_SECRET_SIP_CHAP) => p
                .simple_ip
                .as_ref()
                .map(|l| l.iter().map(|n| n.index).collect()),
            Some(IP_SECRET_MN_HA | IP_SECRET_MN_AAA) => p
                .mobile_ip
                .as_ref()
                .map(|m| m.nais.iter().map(|n| n.index).collect()),
            _ => None,
        };
        if let Some(indexes) = nai_indexes.filter(|l| !l.contains(&s.index)) {
            errs.push(format!(
                "ip_secrets[{i}]: no {} NAI has index {} (NAI indexes {indexes:?})",
                s.class.text(),
                s.index
            ));
        }
    }

    let check = |errs: &mut Vec<String>, key: &str, ef: &str, encoded: Result<Vec<u8>, String>| {
        match encoded {
            Err(e) => errs.push(format!("{key}: {e}")),
            Ok(d) if d.len() > ef_table::size_of(ef) => errs.push(format!(
                "{key}: {} bytes do not fit EF_{ef} ({} bytes)",
                d.len(),
                ef_table::size_of(ef)
            )),
            Ok(_) => {}
        }
    };
    if let Some(list) = &p.simple_ip {
        let encoded = list
            .iter()
            .map(SimpleIpNai::to_codec)
            .collect::<Result<Vec<_>, _>>()
            .and_then(|l| codec::encode_sipupp(&l).map_err(|e| e.0));
        check(&mut errs, "simple_ip", "SIPUPP", encoded);
        if list.is_empty() {
            errs.push("simple_ip: lists no NAI".into());
        }
    }
    if let Some(m) = &p.mobile_ip {
        let encoded = m
            .to_codec()
            .and_then(|m| codec::encode_mipupp(&m).map_err(|e| e.0));
        check(&mut errs, "mobile_ip", "MIPUPP", encoded);
        if m.nais.is_empty() {
            errs.push("mobile_ip: lists no NAI".into());
        }
    }
    if let Some(h) = &p.hrpd_access {
        let encoded = h
            .to_codec()
            .and_then(|h| codec::encode_hrpdupp(&h).map_err(|e| e.0));
        check(&mut errs, "hrpd_access", "HRPDUPP", encoded);
        if h.nai.len() > usize::from(max_nai) {
            errs.push(format!(
                "hrpd_access: the NAI is longer than hrpdcap max_nai_length {max_nai}"
            ));
        }
    }

    for (key, pin) in [("pin1", &p.pin1), ("pin2", &p.pin2)] {
        let Some(pin) = pin else { continue };
        let pin = pin.text();
        if pin.len() > PIN_MAX_LEN {
            errs.push(format!("{key}: > {PIN_MAX_LEN} chars"));
        } else if pin.len() < PIN_MIN_LEN {
            errs.push(format!("{key}: < {PIN_MIN_LEN} digits"));
        } else if !is_digits(&pin) {
            errs.push(format!("{key}: should be all digits (got {pin:?})"));
        }
    }

    for (key, puk) in [("puk1", &p.puk1), ("puk2", &p.puk2)] {
        let Some(puk) = puk else { continue };
        let puk = puk.text();
        if puk.len() != PUK_LEN || !is_digits(&puk) {
            errs.push(format!("{key}: must be {PUK_LEN} digits (got {puk:?})"));
        }
    }

    if let Some(spn) = &p.spn {
        let spn = spn.to_spn();
        match codec::spn_name_limit(spn.char_encoding) {
            None => errs.push(format!(
                "spn.char_encoding: 0x{:02X} not supported (0x00 octet, 0x02 7-bit ASCII, 0x04 UNICODE per C.R1001-G)",
                spn.char_encoding
            )),
            Some(limit) if spn.name.chars().count() > limit => errs.push(format!(
                "spn.name: > {limit} chars for encoding 0x{:02X} ({})",
                spn.char_encoding,
                spn.name.chars().count()
            )),
            Some(_) => {}
        }
    }

    for (key, gid) in [("gid1", p.gid1), ("gid2", p.gid2)] {
        if let Some(gid) = gid {
            if u32::try_from(gid).is_err() {
                errs.push(format!("{key}: must fit in 4 bytes (got {gid})"));
            }
        }
    }

    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problems(yaml: &str) -> Vec<String> {
        validate(&Profile::from_yaml(yaml).unwrap())
    }

    fn assert_ok(yaml: &str) {
        let p = problems(yaml);
        assert!(p.is_empty(), "{yaml}: {p:?}");
    }

    fn assert_problem(yaml: &str, needle: &str) {
        let p = problems(yaml);
        assert!(
            p.iter().any(|e| e.contains(needle)),
            "{yaml}: expected {needle:?} in {p:?}"
        );
    }

    #[test]
    fn luhn() {
        assert!(luhn_ok("8901234567890123455"));
        assert!(!luhn_ok("8901234567890123456"));
        assert!(luhn_ok("79927398713"));
        assert!(!luhn_ok("79927398714"));
    }

    #[test]
    fn identities() {
        assert_ok("");
        assert_ok("iccid: '8901234567890123455'");
        assert_ok("iccid: '89148000001973815516'");
        assert_problem("iccid: '8901234567890123456'", "Luhn");
        assert_problem("iccid: '890123XYZ'", "non-digit");
        assert_problem("iccid: '12345'", "19 or 20 digits");
        assert_problem("iccid: '891480000019738155160'", "19 or 20 digits");
        assert_problem("imsi_m: '1111111111111111'", "15 digits");
        assert_problem("imsi_m: '1'", "15 digits");
        assert_ok("mdn: '*228'");
        assert_problem("mdn: '555-0100'", "non-digit");
    }

    #[test]
    fn fields() {
        assert_problem("language: de", "language indicator");
        assert_ok("services: [10, 32]");
        assert_problem("services: [0]", "service number");
        assert_problem("services: ['10']", "service number");
        assert_problem("esn: F0343402\nmeid: A0000012345678", "exclusive");
        assert_problem("esn: F034", "8 hex digits");
        assert_ok("esn: F0343402");
        assert_problem("meid: A000001234", "14 hex digits");
        assert_problem("sf_euimid: A0", "14 hex digits");
        assert_ok("ruimid: 80C5D564");
        assert_problem("ruimid: 180C5D564", "32-bit");
        assert_problem("spn: {name: x, char_encoding: 1}", "not supported");
        assert_ok("spn: {name: xxxxxxxxxxxxxxxx, char_encoding: 4}");
        assert_problem(
            "spn: {name: xxxxxxxxxxxxxxxxx, char_encoding: 4}",
            "> 16 chars",
        );
        assert_problem("accolc: 16", "0-15");
        assert_problem("accolc: -1", "0-15");
        assert_problem("cdmahome: [{nid: 1}]", "missing 'sid'");
        assert_problem("cdmahome: [{sid: 65536}]", "out of 15-bit range");
        assert_problem("pin1: '123456789'", "> 8 chars");
        assert_problem("pin1: abcd", "should be all digits");
        assert_problem("pin1: '123'", "< 4 digits");
        assert_problem("ecc: [abc]", "non-digit");
        assert_problem("prl: '0012000181004001'", "PR_LIST_SIZE");
        assert_problem("a_key: '1234'", "a_key");
    }

    #[test]
    fn packet_data() {
        assert_ok("ip_secrets: [{class: sip_chap, secret_text: hello}]");
        assert_ok("ip_secrets: [{class: mn_ha, secret: '0011'}]");
        assert_problem(
            "ip_secrets: [{class: sip_chap, secret: '00', secret_text: a}]",
            "set one of",
        );
        assert_problem("ip_secrets: [{class: hrpd}]", "missing 'secret'");
        assert_problem(
            "ip_secrets: [{class: hrpd, index: 4, secret: '00'}]",
            "index must be 0-3",
        );
        assert_problem(
            "ip_secrets: [{class: hrpd, secret_text: '12345678901234567'}]",
            "max_ss_length 16",
        );
        assert_ok("simple_ip: [{nai: a@b}]");
        assert_problem("simple_ip: [{nai: a@b, auth: md5}]", "auth must be");
        assert_problem("simple_ip: [{nai: 'a b'}]", "printable ASCII");
        assert_problem(
            "simple_ip: [{index: 1, nai: a@b}]\nip_secrets: [{class: sip_chap, secret_text: x}]",
            "no sip_chap NAI has index 0",
        );
        assert_ok("mobile_ip: {nais: [{nai: a@b, primary_home_agent: 10.0.0.1, mn_ha_spi: 300}]}");
        assert_problem("mobile_ip: {nais: [{nai: a@b, home_address: x}]}", "IPv4");
        assert_problem(
            "mobile_ip: {retry: {max_retries: 4, first_retry_timeout_ms: 250, rereg_threshold_min: 1}, nais: [{nai: a@b}]}",
            "max_retries",
        );
        assert_ok("hrpd_access: {nai: a@b}");
        assert_problem(
            &format!("hrpd_access: {{nai: {}@b}}", "a".repeat(72)),
            "max_nai_length 72",
        );
        assert_problem(
            &format!("simple_ip: [{{nai: {}@b}}]", "a".repeat(200)),
            "do not fit EF_SIPUPP",
        );
    }

    /// The commented packet-data and secret blocks of the example, with
    /// the YAML lines below each block's key uncommented.
    #[test]
    fn example_packet_data_blocks_validate() {
        const KEYS: [&str; 4] = ["simple_ip:", "mobile_ip:", "hrpd_access:", "ip_secrets:"];
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sim/examples/card.yaml");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut out = Vec::new();
        let mut on = false;
        for line in text.lines() {
            let body = line.strip_prefix("# ");
            if body.is_some_and(|b| KEYS.contains(&b)) {
                on = true;
                out.push(body.unwrap_or_default());
            } else if on && line.starts_with("#  ") && !line.starts_with("#   #") {
                out.push(&line[2..]);
            } else {
                on = on && line.starts_with("#   #");
                out.push(line);
            }
        }
        let p = Profile::from_yaml(&out.join("\n")).unwrap();
        assert!(p.simple_ip.is_some() && p.mobile_ip.is_some() && p.hrpd_access.is_some());
        assert_eq!(p.ip_secrets.as_ref().map(Vec::len), Some(4));
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Profile::from_yaml("auth_capability: 0").is_err());
    }

    #[test]
    fn example_profile_loads_and_validates() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sim/examples/card.yaml");
        let p = Profile::load(&path).unwrap();
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
        assert!(p.eprl.is_some(), "eprl_file is loaded into eprl");
    }
}
