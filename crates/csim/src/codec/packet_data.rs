//! 3GPD user profile and status EFs, C.S0065-B §5.2.43-§5.2.46 and
//! §5.2.54. Each holds a C.S0016-D §3.5.8 parameter block, packed from
//! the MSB of the first byte, with a multi-byte integer MSB first.

use std::net::Ipv4Addr;

use super::{CodecError, err};

/// SimpleIP AUTH_ALGORITHM, C.S0016-D Table 3.5.8.5-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SipAuth {
    None = 0,
    Chap = 1,
    Pap = 2,
    ChapPapFallback = 3,
}

impl SipAuth {
    pub const NAMES: [(&'static str, SipAuth); 4] = [
        ("none", SipAuth::None),
        ("chap", SipAuth::Chap),
        ("pap", SipAuth::Pap),
        ("chap_pap", SipAuth::ChapPapFallback),
    ];

    pub fn from_name(name: &str) -> Option<SipAuth> {
        lookup(&Self::NAMES, name)
    }

    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    fn from_code(code: u8) -> Result<SipAuth, CodecError> {
        from_code(&Self::NAMES, code, "SimpleIP AUTH_ALGORITHM")
    }
}

/// HRPD access authentication AUTH_ALGORITHM, C.S0016-D Table 3.5.8.13-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrpdAuth {
    None = 0,
    Chap = 1,
}

impl HrpdAuth {
    pub const NAMES: [(&'static str, HrpdAuth); 2] =
        [("none", HrpdAuth::None), ("chap", HrpdAuth::Chap)];

    pub fn from_name(name: &str) -> Option<HrpdAuth> {
        lookup(&Self::NAMES, name)
    }

    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    fn from_code(code: u8) -> Result<HrpdAuth, CodecError> {
        from_code(&Self::NAMES, code, "HRPD AUTH_ALGORITHM")
    }
}

/// MN-AAA and MN-HA authentication algorithm, C.S0016-D Table 3.5.8.6-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MipAuth {
    None = 0,
    Md5 = 1,
}

impl MipAuth {
    pub const NAMES: [(&'static str, MipAuth); 2] =
        [("none", MipAuth::None), ("md5", MipAuth::Md5)];

    pub fn from_name(name: &str) -> Option<MipAuth> {
        lookup(&Self::NAMES, name)
    }

    pub fn name(self) -> &'static str {
        name_of(&Self::NAMES, self)
    }

    fn from_code(code: u8) -> Result<MipAuth, CodecError> {
        from_code(&Self::NAMES, code, "MobileIP AUTH_ALGORITHM")
    }
}

fn lookup<T: Copy>(names: &[(&str, T)], name: &str) -> Option<T> {
    let name = name.to_ascii_lowercase();
    names.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
}

fn name_of<T: Copy + PartialEq>(names: &[(&'static str, T)], v: T) -> &'static str {
    names.iter().find(|(_, x)| *x == v).map_or("?", |&(n, _)| n)
}

fn from_code<T: Copy + Into<u8>>(
    names: &[(&str, T)],
    code: u8,
    what: &str,
) -> Result<T, CodecError> {
    match names.iter().find(|(_, v)| (*v).into() == code) {
        Some(&(_, v)) => Ok(v),
        None => err(format!("{what} {code} is reserved")),
    }
}

macro_rules! code_into_u8 {
    ($($t:ty),*) => {$(impl From<$t> for u8 { fn from(v: $t) -> u8 { v as u8 } })*};
}
code_into_u8!(SipAuth, HrpdAuth, MipAuth);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipNai {
    pub index: u8,
    pub nai: String,
    pub auth: SipAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MipRetry {
    /// 0-3 retries.
    pub max_retries: u8,
    /// 250-2000 ms in 250 ms steps.
    pub first_retry_timeout_ms: u16,
    /// 0-63 minutes before the registration lifetime expires.
    pub rereg_threshold_min: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MipNai {
    pub index: u8,
    pub nai: String,
    pub reverse_tunneling: bool,
    pub home_address: Ipv4Addr,
    pub primary_home_agent: Ipv4Addr,
    pub secondary_home_agent: Ipv4Addr,
    pub mn_aaa_auth: MipAuth,
    pub mn_aaa_spi: Option<u32>,
    pub mn_ha_auth: MipAuth,
    pub mn_ha_spi: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MipProfile {
    pub retry: Option<MipRetry>,
    pub nais: Vec<MipNai>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdUpp {
    pub nai: String,
    pub auth: HrpdAuth,
}

const INDEX_BITS: u32 = 4;
const NUM_NAI_BITS: u32 = 4;
const AUTH_BITS: u32 = 4;
const MAX_NUM_RETRY_BITS: u32 = 2;
const FIRST_RETRY_TIMEOUT_BITS: u32 = 3;
const REREG_THRESHOLD_BITS: u32 = 6;
const RETRY_TIMEOUT_STEP_MS: u16 = 250;
const ADDRESS_BITS: u32 = 32;
const SPI_BITS: u32 = 32;
/// The largest NAI_ENTRY_INDEX and NUM_NAI.
pub const NAI_INDEX_MAX: u8 = (1 << INDEX_BITS) - 1;
pub const MAX_NUM_RETRY_MAX: u8 = (1 << MAX_NUM_RETRY_BITS) - 1;
pub const REREG_THRESHOLD_MAX: u8 = (1 << REREG_THRESHOLD_BITS) - 1;
pub const FIRST_RETRY_TIMEOUT_MAX_MS: u16 = RETRY_TIMEOUT_STEP_MS * (1 << FIRST_RETRY_TIMEOUT_BITS);
pub const FIRST_RETRY_TIMEOUT_STEP_MS: u16 = RETRY_TIMEOUT_STEP_MS;

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bits: u32,
}

impl BitWriter {
    fn put(&mut self, value: u64, width: u32) {
        for i in (0..width).rev() {
            if self.bits % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> i) & 1 != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    fn put_nai(&mut self, nai: &str) -> Result<(), CodecError> {
        check_nai(nai)?;
        self.put(nai.len() as u64, 8);
        for b in nai.bytes() {
            self.put(u64::from(b), 8);
        }
        Ok(())
    }

    /// The parameter block with its length byte, the EF content of
    /// C.S0065-B §5.2.43. The reserved padding bits stay 0.
    fn into_ef(self) -> Result<Vec<u8>, CodecError> {
        let Ok(len) = u8::try_from(self.bytes.len()) else {
            return err(format!(
                "parameter block of {} bytes exceeds 255",
                self.bytes.len()
            ));
        };
        let mut out = vec![len];
        out.extend(self.bytes);
        Ok(out)
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn get(&mut self, width: u32) -> Result<u64, CodecError> {
        let mut v = 0;
        for _ in 0..width {
            let Some(byte) = self.bytes.get(self.bit / 8) else {
                return err("parameter block ends early");
            };
            v = (v << 1) | u64::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(v)
    }

    fn get_u8(&mut self, width: u32) -> Result<u8, CodecError> {
        Ok(self.get(width)? as u8)
    }

    fn get_address(&mut self) -> Result<Ipv4Addr, CodecError> {
        Ok(Ipv4Addr::from(self.get(ADDRESS_BITS)? as u32))
    }

    fn get_nai(&mut self) -> Result<String, CodecError> {
        let len = self.get(8)? as usize;
        let bytes = (0..len)
            .map(|_| self.get_u8(8))
            .collect::<Result<Vec<u8>, _>>()?;
        String::from_utf8(bytes).map_err(|_| CodecError("NAI is not ASCII".into()))
    }
}

/// The parameter block after the length byte of the EF.
fn block(data: &[u8]) -> Result<BitReader<'_>, CodecError> {
    let Some((&len, rest)) = data.split_first() else {
        return err("EF is empty");
    };
    match rest.get(..usize::from(len)) {
        Some(bytes) => Ok(BitReader { bytes, bit: 0 }),
        None => err(format!("length byte {len} exceeds the EF")),
    }
}

/// An NAI is a non-empty printable ASCII user@realm of up to 255 bytes
/// (C.S0016-D §3.5.8.5).
pub fn check_nai(nai: &str) -> Result<(), CodecError> {
    if nai.is_empty() || nai.len() > usize::from(u8::MAX) {
        return err(format!(
            "NAI must be 1 to 255 characters, got {}",
            nai.len()
        ));
    }
    if !nai.bytes().all(|b| b.is_ascii_graphic()) {
        return err(format!(
            "NAI {nai:?} must be printable ASCII without spaces"
        ));
    }
    Ok(())
}

fn check_entries(count: usize, indexes: impl Iterator<Item = u8>) -> Result<(), CodecError> {
    if count > usize::from(NAI_INDEX_MAX) {
        return err(format!("at most {NAI_INDEX_MAX} NAI entries"));
    }
    let mut seen = Vec::new();
    for index in indexes {
        if index > NAI_INDEX_MAX {
            return err(format!("NAI index {index} exceeds {NAI_INDEX_MAX}"));
        }
        if seen.contains(&index) {
            return err(format!("NAI index {index} appears twice"));
        }
        seen.push(index);
    }
    Ok(())
}

/// EF_SIPUPP, C.S0016-D §3.5.8.5.
pub fn encode_sipupp(nais: &[SipNai]) -> Result<Vec<u8>, CodecError> {
    check_entries(nais.len(), nais.iter().map(|n| n.index))?;
    let mut w = BitWriter::default();
    w.put(nais.len() as u64, NUM_NAI_BITS);
    for n in nais {
        w.put(u64::from(n.index), INDEX_BITS);
        w.put_nai(&n.nai)?;
        w.put(n.auth as u64, AUTH_BITS);
    }
    w.into_ef()
}

pub fn decode_sipupp(data: &[u8]) -> Result<Vec<SipNai>, CodecError> {
    let mut r = block(data)?;
    let count = r.get(NUM_NAI_BITS)?;
    (0..count)
        .map(|_| {
            Ok(SipNai {
                index: r.get_u8(INDEX_BITS)?,
                nai: r.get_nai()?,
                auth: SipAuth::from_code(r.get_u8(AUTH_BITS)?)?,
            })
        })
        .collect()
}

/// EF_MIPUPP, C.S0016-D §3.5.8.6.
pub fn encode_mipupp(p: &MipProfile) -> Result<Vec<u8>, CodecError> {
    check_entries(p.nais.len(), p.nais.iter().map(|n| n.index))?;
    let mut w = BitWriter::default();
    w.put(u64::from(p.retry.is_some()), 1);
    if let Some(r) = p.retry {
        if r.max_retries > MAX_NUM_RETRY_MAX {
            return err(format!("max_retries must be 0-{MAX_NUM_RETRY_MAX}"));
        }
        let step = RETRY_TIMEOUT_STEP_MS;
        if r.first_retry_timeout_ms < step
            || r.first_retry_timeout_ms > FIRST_RETRY_TIMEOUT_MAX_MS
            || r.first_retry_timeout_ms % step != 0
        {
            return err(format!(
                "first_retry_timeout_ms must be {step}-{FIRST_RETRY_TIMEOUT_MAX_MS} in steps of {step}"
            ));
        }
        if r.rereg_threshold_min > REREG_THRESHOLD_MAX {
            return err(format!(
                "rereg_threshold_min must be 0-{REREG_THRESHOLD_MAX}"
            ));
        }
        w.put(u64::from(r.max_retries), MAX_NUM_RETRY_BITS);
        w.put(
            u64::from(r.first_retry_timeout_ms / step - 1),
            FIRST_RETRY_TIMEOUT_BITS,
        );
        w.put(u64::from(r.rereg_threshold_min), REREG_THRESHOLD_BITS);
    }
    w.put(p.nais.len() as u64, NUM_NAI_BITS);
    for n in &p.nais {
        w.put(u64::from(n.index), INDEX_BITS);
        w.put_nai(&n.nai)?;
        w.put(u64::from(n.reverse_tunneling), 1);
        for a in [n.home_address, n.primary_home_agent, n.secondary_home_agent] {
            w.put(u64::from(u32::from(a)), ADDRESS_BITS);
        }
        for (auth, spi) in [(n.mn_aaa_auth, n.mn_aaa_spi), (n.mn_ha_auth, n.mn_ha_spi)] {
            w.put(auth as u64, AUTH_BITS);
            w.put(u64::from(spi.is_some()), 1);
            if let Some(spi) = spi {
                w.put(u64::from(spi), SPI_BITS);
            }
        }
    }
    w.into_ef()
}

pub fn decode_mipupp(data: &[u8]) -> Result<MipProfile, CodecError> {
    let mut r = block(data)?;
    let retry = if r.get(1)? == 1 {
        Some(MipRetry {
            max_retries: r.get_u8(MAX_NUM_RETRY_BITS)?,
            first_retry_timeout_ms: (r.get(FIRST_RETRY_TIMEOUT_BITS)? as u16 + 1)
                * RETRY_TIMEOUT_STEP_MS,
            rereg_threshold_min: r.get_u8(REREG_THRESHOLD_BITS)?,
        })
    } else {
        None
    };
    let count = r.get(NUM_NAI_BITS)?;
    let mut nais = Vec::new();
    for _ in 0..count {
        let index = r.get_u8(INDEX_BITS)?;
        let nai = r.get_nai()?;
        let reverse_tunneling = r.get(1)? == 1;
        let home_address = r.get_address()?;
        let primary_home_agent = r.get_address()?;
        let secondary_home_agent = r.get_address()?;
        let mut auth = || -> Result<(MipAuth, Option<u32>), CodecError> {
            let alg = MipAuth::from_code(r.get_u8(AUTH_BITS)?)?;
            let spi = if r.get(1)? == 1 {
                Some(r.get(SPI_BITS)? as u32)
            } else {
                None
            };
            Ok((alg, spi))
        };
        let (mn_aaa_auth, mn_aaa_spi) = auth()?;
        let (mn_ha_auth, mn_ha_spi) = auth()?;
        nais.push(MipNai {
            index,
            nai,
            reverse_tunneling,
            home_address,
            primary_home_agent,
            secondary_home_agent,
            mn_aaa_auth,
            mn_aaa_spi,
            mn_ha_auth,
            mn_ha_spi,
        });
    }
    Ok(MipProfile { retry, nais })
}

/// EF_HRPDUPP, C.S0016-D §3.5.8.13.
pub fn encode_hrpdupp(u: &HrpdUpp) -> Result<Vec<u8>, CodecError> {
    let mut w = BitWriter::default();
    w.put_nai(&u.nai)?;
    w.put(u.auth as u64, AUTH_BITS);
    w.into_ef()
}

pub fn decode_hrpdupp(data: &[u8]) -> Result<HrpdUpp, CodecError> {
    let mut r = block(data)?;
    Ok(HrpdUpp {
        nai: r.get_nai()?,
        auth: HrpdAuth::from_code(r.get_u8(AUTH_BITS)?)?,
    })
}

/// EF_SIPSP, C.S0016-D §3.5.8.7: ACT_NAI_ENTRY_INDEX in b8..b5.
pub fn encode_sipsp(active: u8) -> Vec<u8> {
    vec![active << INDEX_BITS]
}

/// EF_MIPSP, C.S0016-D §3.5.8.8: ACT_NAI_ENTRY_INDEX in b8..b5 and no
/// additional active NAIs.
pub fn encode_mipsp(active: u8) -> Vec<u8> {
    vec![active << INDEX_BITS]
}

/// ACT_NAI_ENTRY_INDEX of EF_SIPSP or EF_MIPSP.
pub fn decode_active_nai(data: &[u8]) -> Option<u8> {
    data.first().map(|b| b >> INDEX_BITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sipupp() {
        let nais = vec![SipNai {
            index: 0,
            nai: "a@b".into(),
            auth: SipAuth::Chap,
        }];
        // NUM_NAI 1, index 0, NAI_LENGTH 3, "a@b", CHAP, 4 reserved bits.
        let ef = encode_sipupp(&nais).unwrap();
        assert_eq!(ef, [0x06, 0x10, 0x03, 0x61, 0x40, 0x62, 0x10]);
        assert_eq!(decode_sipupp(&ef).unwrap(), nais);
    }

    #[test]
    fn mipupp_round_trip() {
        let p = MipProfile {
            retry: Some(MipRetry {
                max_retries: 2,
                first_retry_timeout_ms: 1000,
                rereg_threshold_min: 5,
            }),
            nais: vec![MipNai {
                index: 1,
                nai: "user@realm".into(),
                reverse_tunneling: true,
                home_address: Ipv4Addr::UNSPECIFIED,
                primary_home_agent: Ipv4Addr::new(10, 0, 0, 1),
                secondary_home_agent: Ipv4Addr::new(10, 0, 0, 2),
                mn_aaa_auth: MipAuth::Md5,
                mn_aaa_spi: Some(2),
                mn_ha_auth: MipAuth::Md5,
                mn_ha_spi: None,
            }],
        };
        let ef = encode_mipupp(&p).unwrap();
        assert_eq!(usize::from(ef[0]), ef.len() - 1);
        // RETRY_INFO_INCLUDED 1, MAX_NUM_RETRY 10, FIRST_RETRY_TIMEOUT 011,
        // REREG_THRESHOLD 00 of 000101.
        assert_eq!(ef[1], 0b1100_1100);
        assert_eq!(decode_mipupp(&ef).unwrap(), p);
    }

    #[test]
    fn mipupp_rejects_bad_retry() {
        let mut p = MipProfile {
            retry: Some(MipRetry {
                max_retries: 0,
                first_retry_timeout_ms: 300,
                rereg_threshold_min: 0,
            }),
            nais: Vec::new(),
        };
        assert!(encode_mipupp(&p).is_err());
        p.retry = None;
        assert_eq!(encode_mipupp(&p).unwrap(), [0x01, 0x00]);
    }

    #[test]
    fn hrpdupp() {
        let u = HrpdUpp {
            nai: "a@b".into(),
            auth: HrpdAuth::Chap,
        };
        let ef = encode_hrpdupp(&u).unwrap();
        assert_eq!(ef, [0x05, 0x03, 0x61, 0x40, 0x62, 0x10]);
        assert_eq!(decode_hrpdupp(&ef).unwrap(), u);
    }

    #[test]
    fn nai_rules() {
        assert!(check_nai("").is_err());
        assert!(check_nai("a b@c").is_err());
        assert!(check_nai(&"a".repeat(256)).is_err());
        let twice = vec![
            SipNai {
                index: 0,
                nai: "a@b".into(),
                auth: SipAuth::Chap,
            };
            2
        ];
        assert!(encode_sipupp(&twice).is_err());
    }

    #[test]
    fn status() {
        assert_eq!(encode_sipsp(2), [0x20]);
        assert_eq!(decode_active_nai(&encode_mipsp(3)), Some(3));
    }
}
