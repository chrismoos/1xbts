//! Text fields: EF_SPN, EF_LI and 7-bit ASCII packing, with the encoding
//! and language values of C.R1001-G.

use super::{CodecError, err};

/// C.R1001-G Table 9.1-1 MSG_ENCODING values used by EF_SPN and EF_LI.
pub const ENC_OCTET: u8 = 0x00;
pub const ENC_ASCII7: u8 = 0x02;
pub const ENC_UNICODE: u8 = 0x04;

pub fn encoding_name(code: u8) -> String {
    let name = match code {
        0x00 => "octet",
        0x01 => "extended-protocol",
        0x02 => "7-bit-ascii",
        0x03 => "ia5",
        0x04 => "unicode",
        0x05 => "shift-jis",
        0x06 => "korean",
        0x07 => "latin-hebrew",
        0x08 => "latin",
        0x09 => "gsm-7bit",
        0x0A => "gsm-dcs",
        _ => return format!("0x{code:02X}"),
    };
    name.to_owned()
}

/// C.R1001-G Table 9.2-1 language indicators, keyed by ISO 639-1 code.
pub const LANGUAGES: [(&str, u8); 8] = [
    ("unknown", 0x00),
    ("en", 0x01),
    ("fr", 0x02),
    ("es", 0x03),
    ("ja", 0x04),
    ("ko", 0x05),
    ("zh", 0x06),
    ("he", 0x07),
];

pub fn language_indicator(code: &str) -> Option<u8> {
    let code = code.to_ascii_lowercase();
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|&(_, v)| v)
}

pub fn language_code(indicator: u8) -> String {
    LANGUAGES
        .iter()
        .find(|&&(_, v)| v == indicator)
        .map(|&(c, _)| c.to_owned())
        .unwrap_or_else(|| format!("0x{indicator:02X}"))
}

/// Languages whose text fits 7-bit ASCII. The others need UNICODE.
const ASCII_LANGUAGES: [&str; 3] = ["en", "fr", "es"];

const ASCII7_BITS: u32 = 7;

/// Packs 7-bit ASCII most significant bit first, as CDMA SMS user data
/// does. Trailing bits of the last byte are zero.
pub fn pack_ascii7(text: &str) -> Result<Vec<u8>, CodecError> {
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = Vec::new();
    for ch in text.chars() {
        if !ch.is_ascii() {
            return err(format!("non-ASCII character {ch:?}"));
        }
        bits = (bits << ASCII7_BITS) | ch as u32;
        nbits += ASCII7_BITS;
        while nbits >= 8 {
            out.push((bits >> (nbits - 8)) as u8);
            nbits -= 8;
        }
        bits &= (1 << nbits) - 1;
    }
    if nbits > 0 {
        out.push((bits << (8 - nbits)) as u8);
    }
    Ok(out)
}

pub fn unpack_ascii7(data: &[u8], n_chars: usize) -> String {
    let bit = |i: usize| (data[i / 8] >> (7 - i % 8)) & 1;
    (0..n_chars)
        .map(|c| {
            let start = c * ASCII7_BITS as usize;
            let code = (0..ASCII7_BITS as usize).fold(0u8, |acc, k| (acc << 1) | bit(start + k));
            char::from(code)
        })
        .collect()
}

// SPN, C.S0065-B §5.2.32.

pub const SPN_SIZE: usize = 35;
pub const SPN_NAME_BYTES: usize = 32;

/// The name length limit of an SPN encoding, in characters.
pub fn spn_name_limit(char_encoding: u8) -> Option<usize> {
    match char_encoding {
        ENC_OCTET => Some(SPN_NAME_BYTES),
        ENC_ASCII7 => Some(SPN_NAME_BYTES * 8 / ASCII7_BITS as usize),
        ENC_UNICODE => Some(SPN_NAME_BYTES / 2),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spn {
    pub name: String,
    /// b1 of byte 1: display of the registered system required.
    pub display_condition: u8,
    pub char_encoding: u8,
    pub language_indicator: u8,
}

/// The 35-byte EF_SPN: display condition, encoding, language, then the
/// name left justified and F padded. Octet, 7-bit ASCII and UNICODE
/// (UCS-2 big-endian) names are encoded.
pub fn encode_spn(spn: &Spn) -> Result<Vec<u8>, CodecError> {
    let encoded = match spn.char_encoding {
        ENC_UNICODE => spn.name.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        ENC_ASCII7 => pack_ascii7(&spn.name)?,
        ENC_OCTET => {
            if !spn.name.is_ascii() {
                return err(format!("SPN name {:?} is not ASCII", spn.name));
            }
            spn.name.as_bytes().to_vec()
        }
        other => {
            return err(format!(
                "unsupported SPN character encoding 0x{other:02X} (use 0x00 octet, 0x02 7-bit ASCII or 0x04 UNICODE)"
            ));
        }
    };
    if encoded.len() > SPN_NAME_BYTES {
        return err(format!(
            "SPN name does not fit in {SPN_NAME_BYTES} bytes: {:?}",
            spn.name
        ));
    }
    let mut out = vec![0xFF; SPN_SIZE];
    out[0] = spn.display_condition & 0x01;
    out[1] = spn.char_encoding;
    out[2] = spn.language_indicator;
    out[3..3 + encoded.len()].copy_from_slice(&encoded);
    Ok(out)
}

pub fn decode_spn(data: &[u8]) -> Result<Spn, CodecError> {
    if data.len() < 4 {
        return err(format!("SPN data too short: {} bytes", data.len()));
    }
    let char_encoding = data[1];
    let body = &data[3..];
    let end = body.iter().rposition(|&b| b != 0xFF).map_or(0, |i| i + 1);
    let name_bytes = &body[..end];
    let name = match char_encoding {
        ENC_UNICODE => {
            let units: Vec<u16> = name_bytes
                .chunks(2)
                .map(|p| u16::from_be_bytes([p[0], *p.get(1).unwrap_or(&0)]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        ENC_ASCII7 => {
            let n = name_bytes.len() * 8 / ASCII7_BITS as usize;
            unpack_ascii7(name_bytes, n)
                .trim_end_matches('\0')
                .to_owned()
        }
        _ => {
            let end = name_bytes
                .iter()
                .rposition(|&b| b != 0)
                .map_or(0, |i| i + 1);
            String::from_utf8_lossy(&name_bytes[..end]).into_owned()
        }
    };
    Ok(Spn {
        name,
        display_condition: data[0] & 0x01,
        char_encoding,
        language_indicator: data[2],
    })
}

// LI, C.S0065-B §5.2.26.

/// One EF_LI entry: an ISO 639-1 code, or an explicit encoding and
/// indicator pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Language {
    Code(String),
    Pair { char_encoding: u8, indicator: u8 },
}

/// EF_LI: 2 bytes per language (encoding, indicator), highest priority
/// first, unused bytes F.
pub fn encode_li(languages: &[Language], file_size: usize) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    for lang in languages {
        match lang {
            Language::Code(code) => {
                let Some(ind) = language_indicator(code) else {
                    return err(format!("language {code:?} has no C.R1001 indicator"));
                };
                let lower = code.to_ascii_lowercase();
                let enc = if ASCII_LANGUAGES.contains(&lower.as_str()) {
                    ENC_ASCII7
                } else {
                    ENC_UNICODE
                };
                out.extend([enc, ind]);
            }
            Language::Pair {
                char_encoding,
                indicator,
            } => out.extend([*char_encoding, *indicator]),
        }
    }
    if out.len() > file_size {
        return err(format!("EF_LI holds {} languages", file_size / 2));
    }
    out.resize(file_size, 0xFF);
    Ok(out)
}

/// (encoding name, language code) for each used entry.
pub fn decode_li(data: &[u8]) -> Vec<(String, String)> {
    data.chunks_exact(2)
        .filter(|p| p != &[0xFF, 0xFF])
        .map(|p| (encoding_name(p[0]), language_code(p[1])))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spn(name: &str, enc: u8) -> Spn {
        Spn {
            name: name.into(),
            display_condition: 0,
            char_encoding: enc,
            language_indicator: 1,
        }
    }

    #[test]
    fn ascii7() {
        assert_eq!(pack_ascii7("AB").unwrap(), [0x83, 0x08]);
        assert_eq!(unpack_ascii7(&pack_ascii7("1xBTS").unwrap(), 5), "1xBTS");
    }

    #[test]
    fn spn_octet() {
        let b = encode_spn(&Spn {
            display_condition: 1,
            ..spn("MyCarrier", ENC_OCTET)
        })
        .unwrap();
        assert_eq!(b.len(), 35);
        assert_eq!(b[..3], [0x01, 0x00, 0x01]);
        assert_eq!(&b[3..12], b"MyCarrier");
        assert!(b[12..].iter().all(|&x| x == 0xFF));
        assert_eq!(decode_spn(&b).unwrap().name, "MyCarrier");
        assert_eq!(
            encode_spn(&Spn {
                display_condition: 0xFF,
                ..spn("x", 0)
            })
            .unwrap()[0],
            1
        );
    }

    #[test]
    fn spn_other_encodings() {
        let u = encode_spn(&spn("1xBTS", ENC_UNICODE)).unwrap();
        assert_eq!(u[1], 0x04);
        assert_eq!(u[3..13], [0, 0x31, 0, 0x78, 0, 0x42, 0, 0x54, 0, 0x53]);
        assert_eq!(decode_spn(&u).unwrap().name, "1xBTS");
        let a = encode_spn(&spn("1xBTS", ENC_ASCII7)).unwrap();
        assert_eq!(a[3..8], pack_ascii7("1xBTS").unwrap()[..]);
        assert_eq!(decode_spn(&a).unwrap().name, "1xBTS");
        assert!(encode_spn(&spn("x", 0x01)).is_err());
        assert!(encode_spn(&spn(&"x".repeat(33), ENC_OCTET)).is_err());
        assert!(encode_spn(&spn(&"x".repeat(17), ENC_UNICODE)).is_err());
    }

    #[test]
    fn li() {
        let en = Language::Code("en".into());
        assert_eq!(
            encode_li(std::slice::from_ref(&en), 4).unwrap(),
            [0x02, 0x01, 0xFF, 0xFF]
        );
        assert_eq!(
            encode_li(&[Language::Code("ja".into()), en], 4).unwrap(),
            [0x04, 0x04, 0x02, 0x01]
        );
        let pair = Language::Pair {
            char_encoding: 0,
            indicator: 3,
        };
        assert_eq!(encode_li(&[pair], 4).unwrap(), [0x00, 0x03, 0xFF, 0xFF]);
        assert!(encode_li(&[Language::Code("de".into())], 4).is_err());
        assert_eq!(
            decode_li(&[0x02, 0x01, 0xFF, 0xFF]),
            [("7-bit-ascii".to_owned(), "en".to_owned())]
        );
    }
}
