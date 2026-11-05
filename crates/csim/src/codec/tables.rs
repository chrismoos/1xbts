//! Service tables, flags and small fixed-layout EFs.

use super::{CodecError, bcd_swap, bcd_swap_decode, err};

// ECC, C.S0065-B §5.2.38.

const ECC_CODE_BYTES: usize = 3;

/// EF_ECC: one emergency code per 3 bytes, up to 6 BCD digits each (first
/// digit in the low nibble), unused slots F.
pub fn encode_ecc(codes: &[String], file_size: usize) -> Result<Vec<u8>, CodecError> {
    if codes.len() > file_size / ECC_CODE_BYTES {
        return err(format!(
            "more than {} emergency codes",
            file_size / ECC_CODE_BYTES
        ));
    }
    let mut out = Vec::new();
    for code in codes {
        if code.len() > 2 * ECC_CODE_BYTES {
            return err(format!(
                "emergency code > {} digits: {code:?}",
                2 * ECC_CODE_BYTES
            ));
        }
        out.extend(bcd_swap(code, ECC_CODE_BYTES)?);
    }
    out.resize(file_size, 0xFF);
    Ok(out)
}

pub fn decode_ecc(data: &[u8]) -> Vec<String> {
    data.chunks_exact(ECC_CODE_BYTES)
        .map(bcd_swap_decode)
        .filter(|c| !c.is_empty())
        .collect()
}

// CSIM_ST and EST, C.S0065-B §5.2.18 and §5.2.80.

/// One bit per service: service n is bit (n-1)%8 of byte (n-1)/8, so b1
/// of byte 1 is n1. A set bit is available (CSIM_ST) or activated (EST).
pub fn encode_service_table(services: &[u32], n_bytes: usize) -> Result<Vec<u8>, CodecError> {
    let mut out = vec![0u8; n_bytes];
    for &n in services {
        if n < 1 || n as usize > 8 * n_bytes {
            return err(format!("service n{n} does not fit in {n_bytes} bytes"));
        }
        let i = (n - 1) as usize;
        out[i / 8] |= 1 << (i % 8);
    }
    Ok(out)
}

pub fn decode_service_table(data: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    for (byte_idx, b) in data.iter().enumerate() {
        for bit in 0..8 {
            if b & (1 << bit) != 0 {
                out.push((byte_idx * 8 + bit + 1) as u32);
            }
        }
    }
    out
}

pub fn csim_service_name(n: u32) -> &'static str {
    match n {
        1 => "Local Phone Book",
        2 => "Fixed Dialing Numbers (FDN)",
        3 => "Extension 2",
        4 => "Service Dialing Numbers (SDN)",
        5 => "Extension 3",
        6 => "Short Message Storage (SMS)",
        7 => "Short Message Parameters",
        8 => "HRPD",
        9 => "Service Category Program for BC-SMS",
        10 => "CDMA Home Service Provider Name",
        11 => "Data Download via SMS Broadcast",
        12 => "Data Download via SMS-PP",
        13 => "Call Control",
        14 => "3GPD-SIP",
        15 => "3GPD-MIP",
        16 => "AKA",
        17 => "IP-based Location Services (LCS)",
        18 => "BCMCS",
        19 => "Multimedia Messaging Service (MMS)",
        20 => "Extension 8",
        21 => "MMS User Connectivity Parameters",
        22 => "Application Authentication",
        23 => "Group Identifier Level 1",
        24 => "Group Identifier Level 2",
        25 => "De-Personalization Control Keys",
        26 => "Cooperative Network List",
        27 => "Outgoing Call Information (OCI)",
        28 => "Incoming Call Information (ICI)",
        29 => "Extension 5",
        30 => "Multimedia Storage",
        31 => "Image (EF_IMG)",
        32 => "Enabled Services Table",
        33 => "Capability Configuration Parameters (CCP)",
        34 => "SF_EUIMID-based EUIMID",
        35 => "Messaging and 3GPD Extensions",
        36 => "Root Certificates",
        37 => "WAP Browser",
        38 => "Java",
        39 | 40 => "Reserved for CDG",
        41 => "IPv6",
        _ => "?",
    }
}

// AD, C.S0065-B §5.2.34.

pub fn encode_ad(operation_mode: u8, file_size: usize) -> Vec<u8> {
    let mut out = vec![0u8; file_size];
    out[0] = operation_mode;
    out
}

pub fn operation_mode_name(mode: u8) -> String {
    let name = match mode {
        0x00 => "normal",
        0x80 => "type approval",
        0x01 => "normal + specific facilities",
        0x81 => "type approval + specific facilities",
        0x02 => "maintenance (off line)",
        0x04 => "cell test",
        _ => return format!("0x{mode:02X}"),
    };
    name.to_owned()
}

// USGIND, C.S0065-B §5.2.33.

const USGIND_UIM_ID: u8 = 0x01;
const USGIND_SF_EUIMID: u8 = 0x02;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageIndicator {
    /// b1: the UIM_ID is used in place of the handset ESN.
    pub use_uim_id: bool,
    /// b2: the SF_EUIMID is used in place of the handset MEID.
    pub use_sf_euimid: bool,
}

pub fn encode_usgind(u: UsageIndicator) -> Vec<u8> {
    let mut b = 0;
    if u.use_uim_id {
        b |= USGIND_UIM_ID;
    }
    if u.use_sf_euimid {
        b |= USGIND_SF_EUIMID;
    }
    vec![b]
}

pub fn decode_usgind(data: &[u8]) -> UsageIndicator {
    let b = data.first().copied().unwrap_or(0);
    UsageIndicator {
        use_uim_id: b & USGIND_UIM_ID != 0,
        use_sf_euimid: b & USGIND_SF_EUIMID != 0,
    }
}

// HRPDCAP, C.S0065-B §5.2.53 (C.S0016-C §3.5.8.12).

pub const HRPD_MIN_NAI_LENGTH: u8 = 72;
pub const HRPD_MIN_SS_LENGTH: u8 = 16;
const SS_LENGTH_MAX: u8 = 0x1F;
const SS_LENGTH_SHIFT: u8 = 3;
const AUTH_ALGORITHM_CHAP: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HrpdCap {
    pub max_nai_length: u8,
    pub max_ss_length: u8,
    pub chap_supported: bool,
}

impl Default for HrpdCap {
    fn default() -> Self {
        HrpdCap {
            max_nai_length: HRPD_MIN_NAI_LENGTH,
            max_ss_length: HRPD_MIN_SS_LENGTH,
            chap_supported: true,
        }
    }
}

/// MAX_NAI_LENGTH (8 bits), MAX_SS_LENGTH (5 bits) and 3 reserved bits,
/// then AUTH_ALGORITHM with b8 for PPP CHAP.
pub fn encode_hrpdcap(cap: HrpdCap) -> Result<Vec<u8>, CodecError> {
    if cap.max_ss_length > SS_LENGTH_MAX {
        return err(format!("max_ss_length must be 0..{SS_LENGTH_MAX}"));
    }
    let auth = if cap.chap_supported {
        AUTH_ALGORITHM_CHAP
    } else {
        0
    };
    Ok(vec![
        cap.max_nai_length,
        cap.max_ss_length << SS_LENGTH_SHIFT,
        auth,
    ])
}

pub fn decode_hrpdcap(data: &[u8]) -> Result<HrpdCap, CodecError> {
    let [nai, ss, auth, ..] = data[..] else {
        return err("HRPDCAP is 3 bytes");
    };
    Ok(HrpdCap {
        max_nai_length: nai,
        max_ss_length: (ss >> SS_LENGTH_SHIFT) & SS_LENGTH_MAX,
        chap_supported: auth & AUTH_ALGORITHM_CHAP != 0,
    })
}

// AuthCapability, C.S0065-B §5.2.69.

const AUTH_CAP_APP_MMS: u8 = 0x00;

/// 5-byte record: application ID (MMS), the 16-bit capability low byte
/// first, 2 reserved bytes.
pub fn encode_auth_capability_record(capability: u16) -> Vec<u8> {
    let [lo, hi] = capability.to_le_bytes();
    vec![AUTH_CAP_APP_MMS, lo, hi, 0, 0]
}

// MAXPRL, C.S0065-B §5.2.36.

/// MAX_PR_LIST_SIZE of EF_PRL then EF_EPRL, each big-endian.
pub fn encode_maxprl(prl_size: u16, eprl_size: u16) -> Vec<u8> {
    let mut out = prl_size.to_be_bytes().to_vec();
    out.extend(eprl_size.to_be_bytes());
    out
}

// GID1 / GID2, C.S0065-B §5.2.72 and §5.2.73.

pub fn encode_gid(value: u32, length: usize) -> Result<Vec<u8>, CodecError> {
    if length < 4 && u64::from(value) >= 1u64 << (8 * length) {
        return err(format!("GID {value} doesn't fit in {length} bytes"));
    }
    let be = value.to_be_bytes();
    let mut out = vec![0u8; length.saturating_sub(4)];
    out.extend(&be[4usize.saturating_sub(length)..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecc() {
        let blob = encode_ecc(&["911".into(), "112".into()], 15).unwrap();
        assert_eq!(blob[..6], [0x19, 0xF1, 0xFF, 0x11, 0xF2, 0xFF]);
        assert!(blob[6..].iter().all(|&b| b == 0xFF));
        assert_eq!(decode_ecc(&blob), ["911", "112"]);
        let six: Vec<String> = (1..=6).map(|i| i.to_string()).collect();
        assert!(encode_ecc(&six, 15).is_err());
    }

    #[test]
    fn service_table() {
        assert_eq!(encode_service_table(&[1], 2).unwrap(), [0x01, 0x00]);
        assert_eq!(encode_service_table(&[8], 2).unwrap(), [0x80, 0x00]);
        assert_eq!(encode_service_table(&[9], 2).unwrap(), [0x00, 0x01]);
        assert_eq!(
            encode_service_table(&[10, 32], 10).unwrap(),
            [0, 0x02, 0, 0x80, 0, 0, 0, 0, 0, 0]
        );
        let svcs = [2, 3, 10, 32, 34, 41];
        assert_eq!(
            decode_service_table(&encode_service_table(&svcs, 10).unwrap()),
            svcs
        );
        assert!(encode_service_table(&[9], 1).is_err());
    }

    #[test]
    fn flags() {
        assert_eq!(encode_ad(0, 3), [0, 0, 0]);
        assert_eq!(encode_usgind(UsageIndicator::default()), [0]);
        assert_eq!(
            encode_usgind(UsageIndicator {
                use_uim_id: true,
                use_sf_euimid: false
            }),
            [1]
        );
        assert_eq!(
            decode_usgind(&[3]),
            UsageIndicator {
                use_uim_id: true,
                use_sf_euimid: true
            }
        );
    }

    #[test]
    fn hrpdcap() {
        assert_eq!(
            encode_hrpdcap(HrpdCap::default()).unwrap(),
            [0x48, 0x80, 0x80]
        );
        let no_chap = HrpdCap {
            chap_supported: false,
            ..HrpdCap::default()
        };
        assert_eq!(encode_hrpdcap(no_chap).unwrap(), [0x48, 0x80, 0x00]);
        let vz = decode_hrpdcap(&[0x50, 0xF8, 0x80]).unwrap();
        assert_eq!(
            vz,
            HrpdCap {
                max_nai_length: 80,
                max_ss_length: 31,
                chap_supported: true
            }
        );
        assert!(
            encode_hrpdcap(HrpdCap {
                max_ss_length: 32,
                ..HrpdCap::default()
            })
            .is_err()
        );
    }

    #[test]
    fn small_files() {
        assert_eq!(encode_auth_capability_record(0x0103), [0, 0x03, 0x01, 0, 0]);
        assert_eq!(encode_maxprl(256, 0x3000), [0x01, 0x00, 0x30, 0x00]);
        assert_eq!(encode_gid(0x12345678, 4).unwrap(), [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(
            encode_gid(0x12345678, 8).unwrap(),
            [0, 0, 0, 0, 0x12, 0x34, 0x56, 0x78]
        );
        assert!(encode_gid(0x1234, 1).is_err());
    }
}
