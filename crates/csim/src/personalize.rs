//! Writing a profile to a card.

use serde_yaml::{Mapping, Value};

use crate::card::{
    Card, CardError, KEY_REF_PIN1, KEY_REF_PIN2, SW_CONDITIONS_NOT_SATISFIED, SW_FILE_NOT_FOUND,
    SW_NO_DIAGNOSIS, SW_SECURITY_NOT_SATISFIED,
};
use crate::codec::{self, CdmaHome, CodecError, HrpdCap, UsageIndicator};
use crate::profile::{self, Profile};
use crate::{Fid, akey, ef_table, hex_spaced, parse_hex};

/// What a profile write does to a card. [`Card`] performs it and
/// [`Plan`] records it.
pub trait Writer {
    fn select_fid(&mut self, fid: Fid) -> Result<(), CardError>;
    fn update_binary(&mut self, data: &[u8]) -> Result<(), CardError>;
    fn update_record(&mut self, record: u8, data: &[u8]) -> Result<(), CardError>;
    fn preperso_update(&mut self, fid: Fid, data: &[u8]) -> Result<(), CardError>;
    fn load_a_key(&mut self, key: u64) -> Result<(), CardError>;
    fn load_ssd(&mut self, ssd: &[u8]) -> Result<(), CardError>;
    fn load_aka_key(&mut self, p1: u8, key: &[u8]) -> Result<(), CardError>;
    fn load_ip_secret(&mut self, class: u8, index: u8, secret: &[u8]) -> Result<(), CardError>;
    fn set_puk(&mut self, key_ref: u8, puk: &[u8]) -> Result<(), CardError>;
}

impl Writer for Card {
    fn select_fid(&mut self, fid: Fid) -> Result<(), CardError> {
        Card::select_fid(self, fid)
    }
    fn update_binary(&mut self, data: &[u8]) -> Result<(), CardError> {
        Card::update_binary(self, data, 0)
    }
    fn update_record(&mut self, record: u8, data: &[u8]) -> Result<(), CardError> {
        Card::update_record(self, record, data)
    }
    fn preperso_update(&mut self, fid: Fid, data: &[u8]) -> Result<(), CardError> {
        Card::preperso_update(self, fid, data)
    }
    fn load_a_key(&mut self, key: u64) -> Result<(), CardError> {
        Card::load_a_key(self, key)
    }
    fn load_ssd(&mut self, ssd: &[u8]) -> Result<(), CardError> {
        Card::load_ssd(self, ssd)
    }
    fn load_aka_key(&mut self, p1: u8, key: &[u8]) -> Result<(), CardError> {
        Card::load_aka_key(self, p1, key)
    }
    fn load_ip_secret(&mut self, class: u8, index: u8, secret: &[u8]) -> Result<(), CardError> {
        Card::load_ip_secret(self, class, index, secret)
    }
    fn set_puk(&mut self, key_ref: u8, puk: &[u8]) -> Result<(), CardError> {
        Card::set_puk(self, key_ref, puk)
    }
}

/// Where a planned write lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Bytes from offset 0 of a transparent EF.
    Binary,
    /// One record, numbered from 1.
    Record(u8),
}

#[derive(Debug, Clone)]
pub struct PlannedWrite {
    pub fid: Fid,
    pub target: Target,
    pub data: Vec<u8>,
}

/// Collects the EF writes of a profile without a card. Key loads write
/// internal applet state, not EFs, so the plan leaves them out.
#[derive(Default)]
pub struct Plan {
    pub writes: Vec<PlannedWrite>,
    fid: Option<Fid>,
}

impl Writer for Plan {
    fn select_fid(&mut self, fid: Fid) -> Result<(), CardError> {
        self.fid = Some(fid);
        Ok(())
    }
    fn update_binary(&mut self, data: &[u8]) -> Result<(), CardError> {
        let fid = self
            .fid
            .ok_or_else(|| CardError::Invalid("no EF selected".into()))?;
        self.writes.push(PlannedWrite {
            fid,
            target: Target::Binary,
            data: data.to_vec(),
        });
        Ok(())
    }
    fn update_record(&mut self, record: u8, data: &[u8]) -> Result<(), CardError> {
        let fid = self
            .fid
            .ok_or_else(|| CardError::Invalid("no EF selected".into()))?;
        self.writes.push(PlannedWrite {
            fid,
            target: Target::Record(record),
            data: data.to_vec(),
        });
        Ok(())
    }
    fn preperso_update(&mut self, fid: Fid, data: &[u8]) -> Result<(), CardError> {
        self.writes.push(PlannedWrite {
            fid,
            target: Target::Binary,
            data: data.to_vec(),
        });
        Ok(())
    }
    fn load_a_key(&mut self, _key: u64) -> Result<(), CardError> {
        Ok(())
    }
    fn load_ssd(&mut self, _ssd: &[u8]) -> Result<(), CardError> {
        Ok(())
    }
    fn load_aka_key(&mut self, _p1: u8, _key: &[u8]) -> Result<(), CardError> {
        Ok(())
    }
    fn load_ip_secret(&mut self, _class: u8, _index: u8, _secret: &[u8]) -> Result<(), CardError> {
        Ok(())
    }
    fn set_puk(&mut self, _key_ref: u8, _puk: &[u8]) -> Result<(), CardError> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(transparent)]
    Card(#[from] CardError),
    #[error("{0}")]
    Profile(String),
}

impl From<CodecError> for WriteError {
    fn from(e: CodecError) -> Self {
        WriteError::Profile(e.0)
    }
}

/// One line of the write summary.
pub struct Written {
    pub label: String,
    pub bytes: usize,
    /// What was written, as the card's decoders read it back.
    pub value: String,
}

#[derive(Default)]
pub struct Summary {
    pub items: Vec<Written>,
}

impl Summary {
    fn add(&mut self, label: impl Into<String>, bytes: usize, value: impl Into<String>) {
        self.items.push(Written {
            label: label.into(),
            bytes,
            value: value.into(),
        });
    }

    pub fn print(&self) {
        if self.items.is_empty() {
            println!("(nothing written)");
            return;
        }
        println!("personalized:");
        for w in &self.items {
            println!("  {:21} {:5} B  {}", w.label, w.bytes, w.value);
        }
    }
}

/// Secrets are never echoed.
const SECRET_LOADED: &str = "loaded (not readable back)";

/// The written bytes of an EF, decoded. `raw` entries are labelled
/// `NAME (raw)` and decode by their EF name.
fn describe(label: &str, data: &[u8]) -> String {
    crate::read::describe(label.trim_end_matches(" (raw)"), data)
}

/// SELECT failures that mean the EF is not on this card. The EF is
/// skipped rather than the write aborted: the applet answers 6F00 for an
/// EF it does not declare, and carrier cards omit some spec EFs.
const SKIP_SWS: [u16; 3] = [
    SW_FILE_NOT_FOUND,
    SW_NO_DIAGNOSIS,
    SW_CONDITIONS_NOT_SATISFIED,
];
/// EFs whose UPDATE condition is PIN2. They are skipped when PIN2 is not
/// verified, and the rest of the profile still applies.
const PIN2_LABELS: [&str; 4] = ["EST", "FDN", "EXT2", "MMSUCP"];

fn fid(name: &str) -> Fid {
    ef_table::by_name(name)
        .map(|e| e.fid)
        .unwrap_or_else(|| panic!("{name} is not in the EF table"))
}

struct Run<'a, W: Writer> {
    w: &'a mut W,
    summary: Summary,
}

impl<W: Writer> Run<'_, W> {
    fn select_or_skip(&mut self, label: &str, fid: Fid) -> Result<bool, CardError> {
        match self.w.select_fid(fid) {
            Ok(()) => Ok(true),
            Err(e) if e.sw().is_some_and(|sw| SKIP_SWS.contains(&sw)) => {
                let sw = e.sw().unwrap_or(0);
                println!("  skip {label}: SW={sw:04X} (EF not present)");
                self.summary
                    .add(label, 0, format!("skipped, SW={sw:04X} (EF not present)"));
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    fn pin2_skip(&mut self, label: &str, e: &CardError) -> bool {
        if PIN2_LABELS.contains(&label) && e.sw() == Some(SW_SECURITY_NOT_SATISFIED) {
            println!("  skip {label}: SW=6982 (PIN2 not verified)");
            self.summary
                .add(label, 0, "skipped, SW=6982 (PIN2 not verified)");
            return true;
        }
        false
    }

    fn transparent(&mut self, label: &str, fid: Fid, data: &[u8]) -> Result<(), CardError> {
        if !self.select_or_skip(label, fid)? {
            return Ok(());
        }
        match self.w.update_binary(data) {
            Err(e) if self.pin2_skip(label, &e) => Ok(()),
            Err(e) => Err(e),
            Ok(()) => {
                self.summary.add(label, data.len(), describe(label, data));
                Ok(())
            }
        }
    }

    fn records(&mut self, label: &str, fid: Fid, records: &[Vec<u8>]) -> Result<(), CardError> {
        if !self.select_or_skip(label, fid)? {
            return Ok(());
        }
        for (i, rec) in records.iter().enumerate() {
            if let Err(e) = self.w.update_record((i + 1) as u8, rec) {
                if self.pin2_skip(label, &e) {
                    return Ok(());
                }
                return Err(e);
            }
        }
        let total = records.iter().map(Vec::len).sum();
        let values: Vec<String> = records.iter().map(|r| describe(label, r)).collect();
        self.summary.add(label, total, values.join(" | "));
        Ok(())
    }

    /// For EFs whose UPDATE condition is NEVER, written with the applet's
    /// pre-personalization command.
    fn preperso(&mut self, label: &str, fid: Fid, data: &[u8]) -> Result<(), CardError> {
        if !self.select_or_skip(label, fid)? {
            return Ok(());
        }
        self.w.preperso_update(fid, data)?;
        self.summary.add(label, data.len(), describe(label, data));
        Ok(())
    }
}

const SERVICE_HRPD: u32 = 8;
const SERVICE_SPN: u32 = 10;
const SERVICE_SIMPLE_IP: u32 = 14;
const SERVICE_MOBILE_IP: u32 = 15;
const SERVICE_MMS_AUTH: u32 = 22;
const SERVICE_GID1: u32 = 23;
const SERVICE_GID2: u32 = 24;
const SERVICE_EST: u32 = 32;
const SERVICE_SF_EUIMID: u32 = 34;
const GID_WRITE_LEN: usize = 4;
const NID_WILDCARD: u16 = 0xFFFF;

fn bad(msg: impl Into<String>) -> WriteError {
    WriteError::Profile(msg.into())
}

fn hex(key: &str, text: &str) -> Result<Vec<u8>, WriteError> {
    parse_hex(text).map_err(|e| bad(format!("{key}: {e}")))
}

/// Writes a profile. The profile must already validate.
pub fn apply_profile<W: Writer>(w: &mut W, p: &Profile) -> Result<Summary, WriteError> {
    let mut run = Run {
        w,
        summary: Summary::default(),
    };
    let iccid = p.iccid.as_ref().map(profile::Scalar::text);

    if let Some(iccid) = &iccid {
        run.preperso("ICCID", fid("ICCID"), &codec::encode_iccid(iccid)?)?;
    }
    for (label, value) in [("IMSI_M", &p.imsi_m), ("IMSI_T", &p.imsi_t)] {
        if let Some(imsi) = value {
            run.transparent(label, fid(label), &codec::encode_imsi(&imsi.text())?)?;
        }
    }
    if let Some(mdn) = &p.mdn {
        run.records("MDN", fid("MDN"), &[codec::encode_mdn_record(&mdn.text())?])?;
    }
    if let Some(homes) = &p.cdmahome {
        let records = homes
            .iter()
            .map(|e| {
                let home = CdmaHome {
                    sid: e.sid.unwrap_or(0) as u16,
                    nid: e.nid.map_or(NID_WILDCARD, |n| n as u16),
                    band: e.band.unwrap_or(0) as u8,
                };
                codec::encode_cdmahome_record(home)
            })
            .collect::<Result<Vec<_>, _>>()?;
        run.records("CDMAHOME", fid("CDMAHOME"), &records)?;
    }
    if let Some(ecc) = &p.ecc {
        let codes: Vec<String> = ecc.iter().map(profile::Scalar::text).collect();
        run.transparent(
            "ECC",
            fid("ECC"),
            &codec::encode_ecc(&codes, ef_table::size_of("ECC"))?,
        )?;
    }
    if let Some(accolc) = p.accolc {
        run.transparent("ACCOLC", fid("ACCOLC"), &[(accolc & 0x0F) as u8])?;
    }
    // EF_AuthCapability is present only with service n22, so it is written
    // only when the profile sets it.
    if let Some(cap) = p.mms_auth_capability {
        run.records(
            "AUTH_CAP",
            fid("AUTH_CAP"),
            &[codec::encode_auth_capability_record((cap & 0xFFFF) as u16)],
        )?;
    }
    if let Some(prl) = &p.prl {
        run.transparent("PRL", fid("PRL"), &hex("prl", prl)?)?;
    }
    if let Some(eprl) = &p.eprl {
        run.transparent("EPRL", fid("EPRL"), &hex("eprl", eprl)?)?;
    }
    let maxprl = codec::encode_maxprl(
        ef_table::size_of("PRL") as u16,
        ef_table::size_of("EPRL") as u16,
    );
    run.transparent("MAXPRL", fid("MAXPRL"), &maxprl)?;
    if let Some(ah) = p.ah {
        run.transparent("AH", fid("AH"), &(ah as u16).to_le_bytes())?;
    }
    run.transparent(
        "LI",
        fid("LI"),
        &codec::encode_li(&p.languages(), ef_table::size_of("LI"))?,
    )?;
    let mode = p.operation_mode.unwrap_or(0) as u8;
    run.transparent(
        "AD",
        fid("AD"),
        &codec::encode_ad(mode, ef_table::size_of("AD")),
    )?;

    // EF_CSIM_ST: services the applet cannot back (the local phone book
    // n1, FDN n2, multimedia storage n30, images n31) must not be listed.
    // SPN, EST, SF_EUIMID, GID, MMS authentication, HRPD, SimpleIP and
    // MobileIP follow from the EFs that are written.
    let mut services: Vec<u32> = profile::service_numbers(&p.services)
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    let mut add = |n: u32, on: bool| {
        if on {
            services.push(n);
        }
    };
    add(
        SERVICE_SPN,
        p.spn.as_ref().is_some_and(profile::SpnValue::is_set),
    );
    add(SERVICE_EST, true);
    add(SERVICE_HRPD, p.hrpd_access.is_some());
    add(SERVICE_SIMPLE_IP, p.simple_ip.is_some());
    add(SERVICE_MOBILE_IP, p.mobile_ip.is_some());
    add(SERVICE_SF_EUIMID, p.sf_euimid.is_some());
    add(SERVICE_GID1, p.gid1.is_some());
    add(SERVICE_GID2, p.gid2.is_some());
    add(SERVICE_MMS_AUTH, p.mms_auth_capability.is_some());
    services.sort_unstable();
    services.dedup();
    run.transparent(
        "CSIM_ST",
        fid("CSIM_ST"),
        &codec::encode_service_table(&services, ef_table::size_of("CSIM_ST"))?,
    )?;
    let enabled: Vec<u32> = profile::service_numbers(&p.enabled_services)
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    run.transparent(
        "EST",
        fid("EST"),
        &codec::encode_service_table(&enabled, ef_table::size_of("EST"))?,
    )?;

    // EF_SPC needs ADM to read and update. The applet keeps EF_SPCS at 0
    // while the SPC is 000000 and 1 otherwise.
    if let Some(spc) = &p.spc {
        let spc = format!("{:0>width$}", spc.text(), width = codec::SPC_DIGITS);
        run.transparent("SPC", fid("SPC"), &codec::encode_spc(&spc)?)?;
    }
    let usg = p.usgind.clone().unwrap_or_default();
    let usg = UsageIndicator {
        use_uim_id: usg.use_uim_id,
        use_sf_euimid: usg.use_sf_euimid,
    };
    run.transparent("USGIND", fid("USGIND"), &codec::encode_usgind(usg))?;

    if let Some(spn) = p.spn.as_ref().filter(|s| s.is_set()) {
        run.transparent("SPN", fid("SPN"), &codec::encode_spn(&spn.to_spn())?)?;
    }
    for (label, gid) in [("GID1", p.gid1), ("GID2", p.gid2)] {
        if let Some(gid) = gid {
            run.transparent(
                label,
                fid(label),
                &codec::encode_gid(gid as u32, GID_WRITE_LEN)?,
            )?;
        }
    }
    let sf_euimid = p
        .sf_euimid
        .as_ref()
        .map(|v| hex("sf_euimid", &v.text()))
        .transpose()?;
    if let Some(sf) = &sf_euimid {
        run.preperso("SF_EUIMID", fid("SF_EUIMID"), &codec::encode_sf_euimid(sf)?)?;
    }

    // EF_ESN_ME is written by the handset with Store ESN_MEID_ME at every
    // power-up. A profile value preloads it.
    let id = match (&p.meid, &p.esn) {
        (Some(meid), _) => Some(hex("meid", &meid.text())?),
        (None, Some(esn)) => Some(hex("esn", &esn.text())?),
        _ => None,
    };
    if let Some(id) = id {
        run.transparent("ESN_ME", fid("ESN_ME"), &codec::encode_esn_me(&id)?)?;
    }

    // EF_RUIMID: without an explicit value, the pseudo-UIMID of C.S0065-B
    // §5.2.17 from the SF_EUIMID when it is set, otherwise from the ICCID.
    let uim_id = if let Some(r) = &p.ruimid {
        Some(
            u32::from_str_radix(&r.text().replace(' ', ""), 16)
                .map_err(|_| bad("ruimid: not a 32-bit hex value"))?,
        )
    } else if let Some(sf) = &sf_euimid {
        Some(codec::pseudo_uimid_from_sf_euimid(sf)?)
    } else if let Some(iccid) = &iccid {
        Some(codec::pseudo_uimid_from_iccid(iccid)?)
    } else {
        None
    };
    if let Some(uim_id) = uim_id {
        run.preperso("RUIMID", fid("RUIMID"), &codec::encode_ruimid(uim_id))?;
    }

    if let Some(h) = &p.hrpdcap {
        let d = HrpdCap::default();
        let cap = HrpdCap {
            max_nai_length: h.max_nai_length.unwrap_or(d.max_nai_length),
            max_ss_length: h.max_ss_length.unwrap_or(d.max_ss_length),
            chap_supported: h.chap_supported.unwrap_or(d.chap_supported),
        };
        run.transparent("HRPDCAP", fid("HRPDCAP"), &codec::encode_hrpdcap(cap)?)?;
    }

    // 3GPD user profiles. The first NAI listed is the active one in the
    // status EF.
    if let Some(list) = &p.simple_ip {
        let nais = list
            .iter()
            .map(profile::SimpleIpNai::to_codec)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| bad(format!("simple_ip: {e}")))?;
        run.transparent("SIPUPP", fid("SIPUPP"), &codec::encode_sipupp(&nais)?)?;
        if let Some(first) = nais.first() {
            run.transparent("SIPSP", fid("SIPSP"), &codec::encode_sipsp(first.index))?;
        }
    }
    if let Some(m) = &p.mobile_ip {
        let m = m.to_codec().map_err(|e| bad(format!("mobile_ip: {e}")))?;
        run.transparent("MIPUPP", fid("MIPUPP"), &codec::encode_mipupp(&m)?)?;
        if let Some(first) = m.nais.first() {
            run.transparent("MIPSP", fid("MIPSP"), &codec::encode_mipsp(first.index))?;
        }
    }
    if let Some(h) = &p.hrpd_access {
        let h = h.to_codec().map_err(|e| bad(format!("hrpd_access: {e}")))?;
        run.transparent("HRPDUPP", fid("HRPDUPP"), &codec::encode_hrpdupp(&h)?)?;
    }

    for (name, value) in p.raw_entries() {
        let Some(entry) = ef_table::by_name(&name) else {
            return Err(bad(format!("unknown EF in raw: {name:?}")));
        };
        run.transparent(&format!("{name} (raw)"), entry.fid, &hex(&name, &value)?)?;
    }

    // Secrets are internal applet state loaded by ADM commands, not EFs,
    // so a read-back does not check them.
    if let Some(key) = &p.a_key {
        let key = akey::parse_a_key(&key.text()).map_err(|e| bad(e.to_string()))?;
        run.w.load_a_key(key)?;
        run.summary.add("A-key", 8, SECRET_LOADED);
    }
    if let Some(ssd) = &p.ssd {
        run.w.load_ssd(&hex("ssd", ssd)?)?;
        run.summary.add("SSD", 16, SECRET_LOADED);
    }
    for (label, p1, key) in [
        ("AKA root key", 0u8, &p.aka_root_key),
        ("WLAN root key", 1u8, &p.wlan_root_key),
    ] {
        if let Some(key) = key {
            run.w.load_aka_key(p1, &hex(label, key)?)?;
            run.summary.add(label, 16, SECRET_LOADED);
        }
    }
    for s in p.ip_secrets.iter().flatten() {
        let class = s
            .class_number()
            .ok_or_else(|| bad("ip_secrets: unknown class"))?;
        let secret = s.bytes().map_err(|e| bad(format!("ip_secrets: {e}")))?;
        run.w.load_ip_secret(class, s.index, &secret)?;
        run.summary.add(
            format!("IP secret {}[{}]", s.class.text(), s.index),
            secret.len(),
            SECRET_LOADED,
        );
    }
    for (label, key_ref, puk) in [
        ("PUK1", KEY_REF_PIN1, &p.puk1),
        ("PUK2", KEY_REF_PIN2, &p.puk2),
    ] {
        if let Some(puk) = puk {
            let puk = puk.text();
            run.w.set_puk(key_ref, puk.as_bytes())?;
            run.summary.add(label, puk.len(), SECRET_LOADED);
        }
    }
    Ok(run.summary)
}

/// The EF writes `csim write` performs for a profile.
pub fn plan_profile(p: &Profile) -> Result<Vec<PlannedWrite>, WriteError> {
    let mut plan = Plan::default();
    apply_profile(&mut plan, p)?;
    Ok(plan.writes)
}

pub struct WriteCheck {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

fn short_hex(data: &[u8]) -> String {
    const SHOWN: usize = 16;
    let more = if data.len() > SHOWN { " ..." } else { "" };
    format!("{}{more}", hex_spaced(&data[..data.len().min(SHOWN)]))
}

/// Checks every byte a profile writes against a dump of the card taken
/// afterwards.
pub fn compare_profile(dump: &Mapping, p: &Profile) -> Result<Vec<WriteCheck>, WriteError> {
    let empty = Mapping::new();
    let efs = dump
        .get("efs")
        .and_then(Value::as_mapping)
        .unwrap_or(&empty);
    let mut out = Vec::new();
    for w in plan_profile(p)? {
        let label = w.fid.display_label();
        let check = |ok: bool, detail: String| WriteCheck {
            label: label.clone(),
            ok,
            detail,
        };
        let entry = efs.get(label.as_str()).and_then(Value::as_mapping);
        let read_sw = entry
            .and_then(|e| crate::fcp::get_str(e, "read_sw"))
            .unwrap_or("?")
            .to_owned();
        let present = entry
            .and_then(|e| crate::fcp::get_str(e, "select_sw"))
            .is_some_and(|sw| sw == "9000" || sw == "6283");
        let Some(entry) = entry.filter(|_| present) else {
            out.push(check(false, "not on the card".into()));
            continue;
        };
        let (got, what) = match w.target {
            Target::Binary => {
                match crate::fcp::get_str(entry, "raw").and_then(|r| parse_hex(r).ok()) {
                    Some(raw) => (
                        raw.into_iter().take(w.data.len()).collect::<Vec<u8>>(),
                        String::new(),
                    ),
                    None => {
                        out.push(check(false, format!("unreadable (SW={read_sw})")));
                        continue;
                    }
                }
            }
            Target::Record(n) => {
                let rec = entry
                    .get("records")
                    .and_then(Value::as_sequence)
                    .and_then(|r| r.get(usize::from(n) - 1))
                    .and_then(Value::as_str)
                    .and_then(|r| parse_hex(r).ok());
                match rec {
                    Some(rec) => (
                        rec.into_iter().take(w.data.len()).collect(),
                        format!("#{n}"),
                    ),
                    None => {
                        out.push(check(
                            false,
                            format!("record {n} unreadable (SW={read_sw})"),
                        ));
                        continue;
                    }
                }
            }
        };
        if got == w.data {
            out.push(check(
                true,
                format!("{what} {} B {}", w.data.len(), short_hex(&w.data))
                    .trim()
                    .to_owned(),
            ));
        } else {
            out.push(check(
                false,
                format!(
                    "{what} wrote {} read {}",
                    hex_spaced(&w.data),
                    hex_spaced(&got)
                ),
            ));
        }
    }
    Ok(out)
}

/// The report of a profile comparison and how many writes failed.
pub fn format_profile_comparison(checks: &[WriteCheck]) -> (String, usize) {
    let mut lines: Vec<String> = checks
        .iter()
        .map(|c| {
            format!(
                "  [{}] {:14} {}",
                if c.ok { "OK " } else { "FAIL" },
                c.label,
                c.detail
            )
        })
        .collect();
    let failed = checks.iter().filter(|c| !c.ok).count();
    lines.push(String::new());
    if failed > 0 {
        lines.push(format!(
            "profile check FAILED: {failed} of {} writes differ",
            checks.len()
        ));
    } else {
        lines.push(format!(
            "profile check OK: all {} writes read back equal",
            checks.len()
        ));
    }
    (lines.join("\n"), failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(yaml: &str) -> Profile {
        Profile::from_yaml(yaml).unwrap()
    }

    const PROFILE: &str = "iccid: '8901234567890123455'\nimsi_m: '001010123456789'\nmdn: '5550100'\n\
                           cdmahome: [{sid: 1, nid: 65535, band: 0}]\nlanguage: en\nservices: []\nraw: {TERM: '07'}\n";

    fn dump_from_plan(writes: &[PlannedWrite]) -> Mapping {
        let mut efs = Mapping::new();
        for w in writes {
            let key = Value::String(w.fid.display_label());
            let mut e = efs
                .get(&key)
                .and_then(Value::as_mapping)
                .cloned()
                .unwrap_or_else(|| {
                    let mut m = Mapping::new();
                    m.insert("select_sw".into(), "9000".into());
                    m
                });
            match w.target {
                Target::Binary => {
                    let mut data = w.data.clone();
                    data.extend([0xFF; 4]);
                    e.insert("raw".into(), crate::hex_upper(&data).into());
                }
                Target::Record(n) => {
                    let mut recs = e
                        .get("records")
                        .and_then(Value::as_sequence)
                        .cloned()
                        .unwrap_or_default();
                    while recs.len() < usize::from(n) {
                        recs.push(Value::String("FF".repeat(w.data.len())));
                    }
                    recs[usize::from(n) - 1] = Value::String(crate::hex_upper(&w.data));
                    e.insert("records".into(), Value::Sequence(recs));
                }
            }
            efs.insert(key, Value::Mapping(e));
        }
        let mut dump = Mapping::new();
        dump.insert("efs".into(), Value::Mapping(efs));
        dump
    }

    #[test]
    fn plan_lists_writes() {
        let writes = plan_profile(&profile(PROFILE)).unwrap();
        for f in [0x2FE2, 0x6F22, 0x6F44, 0x6F2D] {
            assert!(writes.iter().any(|w| w.fid == Fid(f)), "{f:04X}");
        }
        assert!(
            writes
                .iter()
                .any(|w| w.fid == Fid(0x6F44) && w.target == Target::Record(1))
        );
    }

    #[test]
    fn compare_matching_and_differing_dumps() {
        let p = profile(PROFILE);
        let writes = plan_profile(&p).unwrap();
        let dump = dump_from_plan(&writes);
        let checks = compare_profile(&dump, &p).unwrap();
        assert!(!checks.is_empty() && checks.iter().all(|c| c.ok));

        let mut bad = dump.clone();
        let efs = bad.get_mut("efs").and_then(Value::as_mapping_mut).unwrap();
        efs.get_mut("TERM")
            .and_then(Value::as_mapping_mut)
            .unwrap()
            .insert("raw".into(), "05FFFFFFFF".into());
        efs.remove("MDN");
        let checks = compare_profile(&bad, &p).unwrap();
        let term = checks.iter().find(|c| c.label == "TERM").unwrap();
        assert!(
            !term.ok && term.detail.contains("wrote 07 read 05"),
            "{}",
            term.detail
        );
        assert!(!checks.iter().find(|c| c.label == "MDN").unwrap().ok);
    }

    #[test]
    fn pseudo_uimid_follows_the_iccid() {
        let writes = plan_profile(&profile("iccid: '8901234567890123455'")).unwrap();
        let ruimid = writes.iter().find(|w| w.fid == Fid(0x6F31)).unwrap();
        assert_eq!(ruimid.data, [0x04, 0xC6, 0xF6, 0xEF, 0x80, 0, 0, 0]);
    }
}
