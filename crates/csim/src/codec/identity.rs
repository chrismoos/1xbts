//! Subscriber and card identities: ICCID, IMSI, MDN, CDMAHOME, ESN_ME,
//! RUIMID, SF_EUIMID and SPC.

use sha1::{Digest, Sha1};

use super::{CodecError, bcd_swap, bcd_swap_decode, err};

pub const ICCID_DIGITS: usize = 19;
pub const ICCID_MAX_DIGITS: usize = 20;
const ICCID_BYTES: usize = 10;

/// EF_ICCID: 19 or 20 digits including the check digit (ITU-T E.118,
/// C.S0065-B §5.2.91). A 19-digit ICCID ends with one F filler nibble.
pub fn encode_iccid(iccid: &str) -> Result<Vec<u8>, CodecError> {
    if iccid.len() != ICCID_DIGITS && iccid.len() != ICCID_MAX_DIGITS {
        return err(format!(
            "ICCID must be {ICCID_DIGITS} or {ICCID_MAX_DIGITS} digits, got {}",
            iccid.len()
        ));
    }
    bcd_swap(iccid, ICCID_BYTES)
}

pub fn decode_iccid(data: &[u8]) -> String {
    bcd_swap_decode(data)
}

// IMSI_M / IMSI_T, C.S0065-B §5.2.2, with the digit codings of
// C.S0005-E §2.3.1.

pub const IMSI_DIGITS: usize = 15;
const IMSI_EF_LEN: usize = 10;
const IMSI_PROGRAMMED: u8 = 0x80;

fn digit(c: u8) -> u16 {
    u16::from(c - b'0')
}

/// C.S0005 §2.3.1.1: digits 1-9 code as themselves and 0 as 10.
fn digit_zero_as_ten(c: u8) -> u16 {
    match digit(c) {
        0 => 10,
        n => n,
    }
}

/// 10-bit 100*D1 + 10*D2 + D3 - 111, each digit with 0 counted as 10.
fn encode_3digit_group(d: &[u8]) -> u16 {
    100 * digit_zero_as_ten(d[0]) + 10 * digit_zero_as_ten(d[1]) + digit_zero_as_ten(d[2]) - 111
}

/// C.S0005 Table 2.3.1.1-2: the thousands digit of MIN1, 0 coded as 1010.
fn encode_thousands_bcd(c: u8) -> u16 {
    digit_zero_as_ten(c)
}

/// 24-bit MIN1 from the last 7 digits of the IMSI_S.
pub fn encode_min1(seven: &str) -> u32 {
    let d = seven.as_bytes();
    let high10 = u32::from(encode_3digit_group(&d[0..3]));
    let mid4 = u32::from(encode_thousands_bcd(d[3]));
    let low10 = u32::from(encode_3digit_group(&d[4..7]));
    (high10 << 14) | (mid4 << 10) | low10
}

/// 10-bit MIN2 from the first 3 digits of the IMSI_S.
pub fn encode_min2(three: &str) -> u16 {
    encode_3digit_group(three.as_bytes())
}

/// C.S0005 §2.3.1.2: 7-bit 10*D11 + D12 - 11, digit 0 counted as 10.
pub fn encode_11_12(d11: u8, d12: u8) -> u8 {
    ((10 * digit_zero_as_ten(d11) + digit_zero_as_ten(d12) - 11) & 0x7F) as u8
}

/// C.S0005 §2.3.1.3: 10-bit MCC, coded like a 3-digit group.
pub fn encode_mcc(three: &str) -> u16 {
    encode_3digit_group(three.as_bytes())
}

/// 10-byte EF_IMSI_M or EF_IMSI_T for a class 0 IMSI (MCC, 11_12 and a
/// 10-digit IMSI_S). Class 1 IMSIs are rejected: they have fewer digits,
/// and padding one would change its MCC. Both EFs are written with the
/// programmed bit set.
pub fn encode_imsi(imsi: &str) -> Result<Vec<u8>, CodecError> {
    if !imsi.bytes().all(|b| b.is_ascii_digit()) {
        return err(format!("non-digit in IMSI {imsi:?}"));
    }
    if imsi.len() != IMSI_DIGITS {
        return err(format!(
            "IMSI must be {IMSI_DIGITS} digits (class 0), got {}",
            imsi.len()
        ));
    }
    let d = imsi.as_bytes();
    let mcc = encode_mcc(&imsi[0..3]);
    let s1112 = encode_11_12(d[3], d[4]);
    let min2 = encode_min2(&imsi[5..8]);
    let min1 = encode_min1(&imsi[8..15]);
    // C.S0005-E: IMSI_ADDR_NUM is 000 for a class 0 IMSI.
    let addr_num = 0u8;
    Ok(vec![
        0x00,
        min2 as u8,
        ((min2 >> 8) & 0x03) as u8,
        min1 as u8,
        (min1 >> 8) as u8,
        (min1 >> 16) as u8,
        s1112,
        addr_num | IMSI_PROGRAMMED,
        mcc as u8,
        ((mcc >> 8) & 0x03) as u8,
    ])
}

/// Inverse of the 3-digit group coding. A digit 0 was coded as 10, which
/// adds one to the next digit, so each digit is taken off the low end
/// with a remainder of 0 read as 10.
fn decode_3digit_group(value: u16) -> String {
    let mut v = value + 111;
    let d3 = match v % 10 {
        0 => 10,
        n => n,
    };
    v = (v - d3) / 10;
    let d2 = match v % 10 {
        0 => 10,
        n => n,
    };
    v = (v - d2) / 10;
    let d1 = if v == 0 { 10 } else { v };
    [d1, d2, d3].iter().map(|&d| digit_char(d)).collect()
}

fn digit_char(d: u16) -> char {
    if d == 10 {
        '0'
    } else {
        char::from(b'0' + d as u8)
    }
}

fn decode_min1(value: u32) -> String {
    let high10 = ((value >> 14) & 0x3FF) as u16;
    let mid4 = ((value >> 10) & 0x0F) as u16;
    let low10 = (value & 0x3FF) as u16;
    format!(
        "{}{}{}",
        decode_3digit_group(high10),
        digit_char(mid4),
        decode_3digit_group(low10)
    )
}

fn decode_11_12(value: u8) -> String {
    let v = u16::from(value & 0x7F) + 11;
    let d12 = match v % 10 {
        0 => 10,
        n => n,
    };
    let d11 = (v - d12) / 10;
    format!("{}{}", digit_char(d11), digit_char(d12))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imsi {
    pub class: u8,
    pub addr_num: u8,
    pub programmed: bool,
    pub mcc: String,
    pub digits_11_12: String,
    pub min_or_msin: String,
    /// The reconstituted 15-digit IMSI.
    pub imsi: String,
}

pub fn decode_imsi(data: &[u8]) -> Result<Imsi, CodecError> {
    if data.len() != IMSI_EF_LEN {
        return err(format!(
            "IMSI must be {IMSI_EF_LEN} bytes, got {}",
            data.len()
        ));
    }
    let min2 = u16::from(data[1]) | (u16::from(data[2] & 0x03) << 8);
    let min1 = u32::from(data[3]) | (u32::from(data[4]) << 8) | (u32::from(data[5]) << 16);
    let mcc = u16::from(data[8]) | (u16::from(data[9] & 0x03) << 8);
    let mcc = decode_3digit_group(mcc);
    let digits_11_12 = decode_11_12(data[6]);
    let min_or_msin = format!("{}{}", decode_3digit_group(min2), decode_min1(min1));
    Ok(Imsi {
        class: data[0] & 0x01,
        addr_num: data[7] & 0x07,
        programmed: data[7] & IMSI_PROGRAMMED != 0,
        imsi: format!("{mcc}{digits_11_12}{min_or_msin}"),
        mcc,
        digits_11_12,
        min_or_msin,
    })
}

// MDN, C.S0065-B §5.2.35.

pub const MDN_MAX_DIGITS: usize = 15;
pub const MDN_RECORD_LEN: usize = 11;
const MDN_DIGIT_BYTES: usize = 8;
const NUMBER_PLAN_ISDN: u8 = 1;

/// C.S0005-E Table 2.7.1.3.2.4-4: digits 1-9 code as themselves, 0 as
/// 1010, `*` as 1011 and `#` as 1100.
fn dtmf_code(c: char) -> Option<u8> {
    match c {
        '1'..='9' => Some(c as u8 - b'0'),
        '0' => Some(0xA),
        '*' => Some(0xB),
        '#' => Some(0xC),
        _ => None,
    }
}

fn dtmf_char(code: u8) -> char {
    match code {
        1..=9 => char::from(b'0' + code),
        0xA => '0',
        0xB => '*',
        0xC => '#',
        _ => '?',
    }
}

/// One 11-byte EF_MDN record: the digit count in b1..b4 of byte 1, the
/// digits as DTMF codes in bytes 2-9 (first digit of a pair in the low
/// nibble, unused nibbles F), NUMBER_TYPE unknown and NUMBER_PLAN ISDN
/// in byte 10, PI and SI 0 in byte 11.
pub fn encode_mdn_record(number: &str) -> Result<Vec<u8>, CodecError> {
    let codes: Option<Vec<u8>> = number.chars().map(dtmf_code).collect();
    let Some(codes) = codes else {
        return err(format!("MDN may contain only 0-9 * #, got {number:?}"));
    };
    if codes.len() > MDN_MAX_DIGITS {
        return err(format!("MDN > {MDN_MAX_DIGITS} digits: {number:?}"));
    }
    let mut rec = vec![0xFF; MDN_RECORD_LEN];
    rec[0] = codes.len() as u8 & 0x0F;
    for i in 0..MDN_DIGIT_BYTES {
        let lo = codes.get(2 * i).copied().unwrap_or(0xF);
        let hi = codes.get(2 * i + 1).copied().unwrap_or(0xF);
        rec[1 + i] = (hi << 4) | lo;
    }
    rec[9] = NUMBER_PLAN_ISDN << 3;
    rec[10] = 0x00;
    Ok(rec)
}

/// The digit string of an EF_MDN record.
pub fn decode_mdn_record(rec: &[u8]) -> Result<String, CodecError> {
    if rec.len() != MDN_RECORD_LEN {
        return err(format!(
            "MDN record must be {MDN_RECORD_LEN} bytes, got {}",
            rec.len()
        ));
    }
    let n = usize::from(rec[0] & 0x0F);
    Ok((0..n)
        .map(|i| {
            let b = rec[1 + i / 2];
            dtmf_char(if i % 2 == 0 { b & 0x0F } else { b >> 4 })
        })
        .collect())
}

// CDMAHOME, C.S0065-B §5.2.8.

pub const CDMAHOME_RECORD_LEN: usize = 5;
const SID_MAX: u32 = 1 << 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdmaHome {
    pub sid: u16,
    pub nid: u16,
    pub band: u8,
}

/// One 5-byte record: SID (15 bits) and NID little-endian, band class
/// in b1..b5 of byte 5.
pub fn encode_cdmahome_record(home: CdmaHome) -> Result<Vec<u8>, CodecError> {
    if u32::from(home.sid) >= SID_MAX {
        return err(format!("SID out of range: {}", home.sid));
    }
    let [sid_lo, sid_hi] = home.sid.to_le_bytes();
    let [nid_lo, nid_hi] = home.nid.to_le_bytes();
    Ok(vec![
        sid_lo,
        sid_hi & 0x7F,
        nid_lo,
        nid_hi,
        home.band & 0x1F,
    ])
}

pub fn decode_cdmahome_record(rec: &[u8]) -> Result<CdmaHome, CodecError> {
    if rec.len() != CDMAHOME_RECORD_LEN {
        return err(format!(
            "CDMAHOME record must be {CDMAHOME_RECORD_LEN} bytes"
        ));
    }
    Ok(CdmaHome {
        sid: u16::from(rec[0]) | (u16::from(rec[1] & 0x7F) << 8),
        nid: u16::from_le_bytes([rec[2], rec[3]]),
        band: rec[4] & 0x1F,
    })
}

// ESN_ME, C.S0065-B §5.2.24.

pub const ESN_BYTES: usize = 4;
pub const MEID_BYTES: usize = 7;
const ID_EF_LEN: usize = 8;

/// 8-byte EF_ESN_ME holding an ESN or MEID: byte 1 is the identifier
/// length, then the identifier lowest-order byte first, zero padded.
pub fn encode_esn_me(id: &[u8]) -> Result<Vec<u8>, CodecError> {
    if id.len() != ESN_BYTES && id.len() != MEID_BYTES {
        return err(format!(
            "an ESN is {ESN_BYTES} bytes and a MEID {MEID_BYTES}"
        ));
    }
    let mut out = vec![id.len() as u8];
    out.extend(id.iter().rev());
    out.resize(ID_EF_LEN, 0);
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredId {
    None,
    Esn(String),
    Meid(String),
    Other(usize, String),
}

pub fn decode_esn_me(data: &[u8]) -> Result<StoredId, CodecError> {
    if data.len() < ID_EF_LEN {
        return err(format!(
            "EF_ESN_ME must be {ID_EF_LEN} bytes, got {}",
            data.len()
        ));
    }
    let n = usize::from(data[0] & 0x0F);
    let value: String = data[1..1 + n.min(MEID_BYTES)]
        .iter()
        .rev()
        .map(|b| format!("{b:02X}"))
        .collect();
    Ok(match n {
        0 => StoredId::None,
        ESN_BYTES => StoredId::Esn(value),
        MEID_BYTES => StoredId::Meid(value),
        n => StoredId::Other(n, value),
    })
}

// RUIMID, C.S0065-B §5.2.17.

const PSEUDO_ID_PREFIX: u32 = 0x80;

/// 8-byte EF_RUIMID: length 4, then the UIM_ID lowest-order byte first.
pub fn encode_ruimid(uim_id: u32) -> Vec<u8> {
    let mut out = vec![ESN_BYTES as u8];
    out.extend(uim_id.to_le_bytes());
    out.resize(ID_EF_LEN, 0);
    out
}

/// The UIM_ID as 8 hex digits, or `None` when the EF holds none.
pub fn decode_ruimid(data: &[u8]) -> Option<String> {
    let n = usize::from(data.first()? & 0x0F);
    if n == 0 || data.len() < 1 + n {
        return None;
    }
    Some(
        data[1..1 + n]
            .iter()
            .rev()
            .map(|b| format!("{b:02X}"))
            .collect(),
    )
}

/// The 32-bit pseudo-UIMID: 80 in the top byte and the low 24 bits of
/// SHA-1 over the EUIMID digits (4 bits each, most significant first).
pub fn pseudo_uimid(euimid: &[u8]) -> u32 {
    let digest = Sha1::digest(euimid);
    let low24 =
        (u32::from(digest[17]) << 16) | (u32::from(digest[18]) << 8) | u32::from(digest[19]);
    (PSEUDO_ID_PREFIX << 24) | low24
}

pub fn pseudo_uimid_from_sf_euimid(sf_euimid: &[u8]) -> Result<u32, CodecError> {
    if sf_euimid.len() != MEID_BYTES {
        return err("SF_EUIMID must be 14 hex digits");
    }
    Ok(pseudo_uimid(sf_euimid))
}

/// The LF_EUIMID is the ICCID as EF_ICCID stores its digits, 20 nibbles
/// most significant first: 19 digits and an F filler, or 20 digits.
pub fn pseudo_uimid_from_iccid(iccid: &str) -> Result<u32, CodecError> {
    let ok_len = iccid.len() == ICCID_DIGITS || iccid.len() == ICCID_MAX_DIGITS;
    if !ok_len || !iccid.bytes().all(|b| b.is_ascii_digit()) {
        return err(format!(
            "ICCID must be {ICCID_DIGITS} or {ICCID_MAX_DIGITS} digits"
        ));
    }
    let mut nibbles: Vec<u8> = iccid.bytes().map(|b| b - b'0').collect();
    nibbles.resize(ICCID_MAX_DIGITS, 0xF);
    let euimid: Vec<u8> = nibbles.chunks(2).map(|p| (p[0] << 4) | p[1]).collect();
    Ok(pseudo_uimid(&euimid))
}

// SF_EUIMID, C.S0065-B §5.2.79.

/// 7-byte EF_SF_EUIMID: with the digits d1..d14 most significant first,
/// byte 1 holds d13 d14 and byte 7 holds d1 d2.
pub fn encode_sf_euimid(id: &[u8]) -> Result<Vec<u8>, CodecError> {
    if id.len() != MEID_BYTES {
        return err(format!(
            "SF_EUIMID must be {MEID_BYTES} bytes, got {}",
            id.len()
        ));
    }
    Ok(id.iter().rev().copied().collect())
}

pub fn decode_sf_euimid(data: &[u8]) -> String {
    data.iter()
        .take(MEID_BYTES)
        .rev()
        .map(|b| format!("{b:02X}"))
        .collect()
}

// SPC, C.S0065-B §5.2.19.

pub const SPC_DIGITS: usize = 6;

/// EF_SPC: six BCD digits, D1 in the high nibble of byte 1.
pub fn encode_spc(spc: &str) -> Result<Vec<u8>, CodecError> {
    if spc.len() != SPC_DIGITS || !spc.bytes().all(|b| b.is_ascii_digit()) {
        return err(format!("SPC must be {SPC_DIGITS} digits, got {spc:?}"));
    }
    let d = spc.as_bytes();
    Ok((0..3)
        .map(|i| ((d[2 * i] - b'0') << 4) | (d[2 * i + 1] - b'0'))
        .collect())
}

pub fn decode_spc(data: &[u8]) -> String {
    data.iter().take(3).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_coding() {
        // C.S0005 §2.3.1.1 worked example, MIN 012 345 6 789.
        assert_eq!(encode_min2("012"), 0b1110000101);
        let min1 = encode_min1("3456789");
        assert_eq!(min1, (234 << 14) | (0b0110 << 10) | 678);
        assert_eq!(encode_min2("000"), 999);
    }

    #[test]
    fn digits_11_12() {
        assert_eq!(encode_11_12(b'0', b'0'), 99);
        assert_eq!(encode_11_12(b'0', b'1'), 90);
        assert_eq!(encode_11_12(b'1', b'0'), 9);
        assert_eq!(encode_11_12(b'1', b'2'), 1);
        assert_eq!(encode_11_12(b'9', b'9'), 88);
    }

    #[test]
    fn mcc() {
        assert_eq!(encode_mcc("310"), 300 + 10 + 10 - 111);
        assert_eq!(encode_mcc("001"), 1000 + 100 + 1 - 111);
    }

    #[test]
    fn imsi_layout() {
        let out = encode_imsi("310012345678901").unwrap();
        assert_eq!(out.len(), 10);
        assert_eq!(out[0], 0);
        assert_eq!(out[7], 0x80);
        let min2 = encode_min2("234");
        assert_eq!((out[1], out[2]), (min2 as u8, (min2 >> 8) as u8 & 0x03));
        let min1 = encode_min1("5678901");
        assert_eq!(
            out[3..6],
            [min1 as u8, (min1 >> 8) as u8, (min1 >> 16) as u8]
        );
        // A BTS decoded byte 7 = 09 as digits 10.
        assert_eq!(encode_imsi("001100000000001").unwrap()[6], 0x09);
        assert_eq!(encode_imsi("001010000000001").unwrap()[6], 90);
        assert!(encode_imsi("31001234567890").is_err());
    }

    #[test]
    fn imsi_round_trip() {
        for imsi in [
            "310012345678901",
            "001010000000001",
            "000000000000000",
            "999999999999999",
            "001100000000001",
        ] {
            assert_eq!(decode_imsi(&encode_imsi(imsi).unwrap()).unwrap().imsi, imsi);
        }
    }

    #[test]
    fn iccid() {
        let out = encode_iccid("8901234567890123455").unwrap();
        assert_eq!(out[0], 0x98);
        assert_eq!(out[9] & 0xF0, 0xF0);
        assert_eq!(decode_iccid(&out), "8901234567890123455");
        let twenty = encode_iccid("89148000001973815516").unwrap();
        assert_eq!(crate::hex_upper(&twenty), "98410800009137185561");
        assert!(encode_iccid("89012345678901234").is_err());
    }

    #[test]
    fn mdn() {
        let rec = encode_mdn_record("5550100").unwrap();
        assert_eq!(rec[..5], [7, 0x55, 0xA5, 0xA1, 0xFA]);
        assert_eq!((rec[9], rec[10]), (0x08, 0x00));
        assert_eq!(encode_mdn_record("*#").unwrap()[1], 0xCB);
        assert_eq!(encode_mdn_record("1").unwrap()[1], 0xF1);
        assert!(encode_mdn_record("555-0100").is_err());
        for n in ["5550100", "19005551000", "0", "*228"] {
            assert_eq!(
                decode_mdn_record(&encode_mdn_record(n).unwrap()).unwrap(),
                n
            );
        }
    }

    #[test]
    fn cdmahome() {
        let rec = encode_cdmahome_record(CdmaHome {
            sid: 0x1234,
            nid: 0xABCD,
            band: 2,
        })
        .unwrap();
        assert_eq!(rec, [0x34, 0x12, 0xCD, 0xAB, 0x02]);
        let full = encode_cdmahome_record(CdmaHome {
            sid: 1,
            nid: 1,
            band: 0xFF,
        })
        .unwrap();
        assert_eq!(full[4], 0x1F);
        let home = CdmaHome {
            sid: 22,
            nid: 65535,
            band: 1,
        };
        assert_eq!(
            decode_cdmahome_record(&encode_cdmahome_record(home).unwrap()).unwrap(),
            home
        );
    }

    #[test]
    fn esn_me() {
        assert_eq!(
            encode_esn_me(&[0xF0, 0x34, 0x34, 0x02]).unwrap(),
            [4, 0x02, 0x34, 0x34, 0xF0, 0, 0, 0]
        );
        let meid = [0xA0, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78];
        assert_eq!(
            encode_esn_me(&meid).unwrap(),
            [7, 0x78, 0x56, 0x34, 0x12, 0, 0, 0xA0]
        );
        assert_eq!(
            decode_esn_me(&encode_esn_me(&meid).unwrap()).unwrap(),
            StoredId::Meid("A0000012345678".into())
        );
        assert_eq!(decode_esn_me(&[0; 8]).unwrap(), StoredId::None);
    }

    #[test]
    fn ruimid() {
        assert_eq!(
            encode_ruimid(0x80C5D564),
            [4, 0x64, 0xD5, 0xC5, 0x80, 0, 0, 0]
        );
        assert_eq!(
            decode_ruimid(&encode_ruimid(0x80C5D564)).as_deref(),
            Some("80C5D564")
        );
        // C.S0065-B §5.2.17 footnote 3.
        let sf = [0xFF, 0x00, 0x00, 0x01, 0x12, 0x34, 0x56];
        assert_eq!(pseudo_uimid_from_sf_euimid(&sf).unwrap(), 0x800737E1);
        let v = pseudo_uimid_from_iccid("8901010123456789014").unwrap();
        let euimid = crate::parse_hex("8901010123456789014F").unwrap();
        assert_eq!(v, pseudo_uimid(&euimid));
        // The values the two example profiles were written with.
        assert_eq!(
            pseudo_uimid_from_iccid("8901234567890123463").unwrap(),
            0x8075C0DF
        );
    }

    #[test]
    fn sf_euimid() {
        let id = crate::parse_hex("A1B2C3D4E5F607").unwrap();
        let ef = encode_sf_euimid(&id).unwrap();
        assert_eq!(crate::hex_upper(&ef), "07F6E5D4C3B2A1");
        assert_eq!(decode_sf_euimid(&ef), "A1B2C3D4E5F607");
    }

    #[test]
    fn spc() {
        assert_eq!(encode_spc("123456").unwrap(), [0x12, 0x34, 0x56]);
        assert_eq!(decode_spc(&[0x98, 0x76, 0x54]), "987654");
        for bad in ["12345", "1234567", "12345a"] {
            assert!(encode_spc(bad).is_err());
        }
    }
}
