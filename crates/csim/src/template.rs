//! Profile templates: a profile YAML with placeholders that `csim profile`
//! fills for one subscriber.
//!
//! | Placeholder | Value |
//! |---|---|
//! | `{imsi}` | the 15-digit IMSI |
//! | `{min}` | the last 10 digits of the IMSI |
//! | `{mdn}` | the MDN, default the MIN |
//! | `{luhn}` | the Luhn check digit of the digits directly before it |
//! | `{a_key}` | a random 64-bit A-key, 16 hex digits |
//! | `{secret}` | a random 16-byte secret, 32 hex digits, new at each use |
//!
//! Comment lines are copied without substitution. The template's leading
//! comment block is dropped.

use thiserror::Error;

pub const IMSI_LEN: usize = 15;
pub const MIN_LEN: usize = 10;
pub const MDN_MAX_LEN: usize = 15;
pub const A_KEY_LEN: usize = 8;
pub const SECRET_LEN: usize = 16;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("IMSI must be {IMSI_LEN} digits, got {0:?}")]
    Imsi(String),
    #[error("MDN must be 1 to {MDN_MAX_LEN} digits, got {0:?}")]
    Mdn(String),
    #[error("line {line}: unknown placeholder {{{name}}}")]
    Unknown { line: usize, name: String },
    #[error("line {line}: {{luhn}} needs digits directly before it")]
    LuhnNoDigits { line: usize },
}

#[derive(Debug, Clone)]
pub struct Subscriber {
    imsi: String,
    mdn: String,
}

impl Subscriber {
    /// `mdn` defaults to the MIN.
    pub fn new(imsi: &str, mdn: Option<&str>) -> Result<Subscriber, TemplateError> {
        if imsi.len() != IMSI_LEN || !is_digits(imsi) {
            return Err(TemplateError::Imsi(imsi.to_owned()));
        }
        let mdn = match mdn {
            Some(m) if m.is_empty() || m.len() > MDN_MAX_LEN || !is_digits(m) => {
                return Err(TemplateError::Mdn(m.to_owned()));
            }
            Some(m) => m.to_owned(),
            None => imsi[IMSI_LEN - MIN_LEN..].to_owned(),
        };
        Ok(Subscriber {
            imsi: imsi.to_owned(),
            mdn,
        })
    }

    pub fn imsi(&self) -> &str {
        &self.imsi
    }

    pub fn min(&self) -> &str {
        &self.imsi[IMSI_LEN - MIN_LEN..]
    }

    pub fn mdn(&self) -> &str {
        &self.mdn
    }
}

/// `random` fills a buffer with random bytes.
pub fn fill(
    template: &str,
    sub: &Subscriber,
    mut random: impl FnMut(&mut [u8]),
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    let body = template
        .lines()
        .enumerate()
        .skip_while(|(_, l)| is_comment(l) || l.trim().is_empty());
    for (i, line) in body {
        if is_comment(line) {
            out.push_str(line);
        } else {
            fill_line(line, i + 1, sub, &mut random, &mut out)?;
        }
        out.push('\n');
    }
    Ok(out)
}

fn fill_line(
    line: &str,
    line_no: usize,
    sub: &Subscriber,
    random: &mut impl FnMut(&mut [u8]),
    out: &mut String,
) -> Result<(), TemplateError> {
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(name) = placeholder_name(after) else {
            out.push('{');
            rest = after;
            continue;
        };
        match name {
            "imsi" => out.push_str(sub.imsi()),
            "min" => out.push_str(sub.min()),
            "mdn" => out.push_str(sub.mdn()),
            "a_key" => out.push_str(&random_hex(A_KEY_LEN, random)),
            "secret" => out.push_str(&random_hex(SECRET_LEN, random)),
            "luhn" => {
                let digits: String = out
                    .chars()
                    .rev()
                    .take_while(char::is_ascii_digit)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if digits.is_empty() {
                    return Err(TemplateError::LuhnNoDigits { line: line_no });
                }
                out.push(luhn_digit(&digits));
            }
            _ => {
                return Err(TemplateError::Unknown {
                    line: line_no,
                    name: name.to_owned(),
                });
            }
        }
        rest = &after[name.len() + 1..];
    }
    out.push_str(rest);
    Ok(())
}

/// The digit that makes `digits` followed by it pass the Luhn check.
pub fn luhn_digit(digits: &str) -> char {
    (b'0'..=b'9')
        .map(char::from)
        .find(|d| crate::profile::luhn_ok(&format!("{digits}{d}")))
        .expect("one of ten digits satisfies the Luhn check")
}

/// The name of a placeholder starting at `after`, the text past a `{`.
/// Other braces, such as YAML flow mappings, are not placeholders.
fn placeholder_name(after: &str) -> Option<&str> {
    let end = after.find('}')?;
    let name = &after[..end];
    let is_name = !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    is_name.then_some(name)
}

fn random_hex(len: usize, random: &mut impl FnMut(&mut [u8])) -> String {
    let mut buf = vec![0u8; len];
    random(&mut buf);
    crate::hex_upper(&buf)
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

fn is_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter() -> impl FnMut(&mut [u8]) {
        let mut n = 0u8;
        move |buf: &mut [u8]| {
            for b in buf {
                *b = n;
                n = n.wrapping_add(1);
            }
        }
    }

    fn sub() -> Subscriber {
        Subscriber::new("001010000000042", None).unwrap()
    }

    #[test]
    fn subscriber_defaults_and_checks() {
        let s = sub();
        assert_eq!(s.min(), "0000000042");
        assert_eq!(s.mdn(), "0000000042");
        assert_eq!(
            Subscriber::new("00101000000004", None).unwrap_err(),
            TemplateError::Imsi("00101000000004".into())
        );
        assert!(matches!(
            Subscriber::new("001010000000042", Some("555-0100")),
            Err(TemplateError::Mdn(_))
        ));
        assert_eq!(
            Subscriber::new("001010000000042", Some("5550142"))
                .unwrap()
                .mdn(),
            "5550142"
        );
    }

    #[test]
    fn fills_placeholders() {
        let t = "# header\n\nimsi_m: \"{imsi}\"\nmdn: \"{mdn}\"\n\
                 iccid: \"89010100{min}{luhn}\"\n# keep {imsi}\n\
                 nai: \"{imsi}@1xbts.local\"\na_key: \"{a_key}\"\n\
                 s1: \"{secret}\"\ns2: \"{secret}\"\n";
        let out = fill(t, &sub(), counter()).unwrap();
        let iccid = "890101000000000042";
        let want = format!(
            "imsi_m: \"001010000000042\"\nmdn: \"0000000042\"\n\
             iccid: \"{iccid}{}\"\n# keep {{imsi}}\n\
             nai: \"001010000000042@1xbts.local\"\n\
             a_key: \"0001020304050607\"\n\
             s1: \"08090A0B0C0D0E0F1011121314151617\"\n\
             s2: \"18191A1B1C1D1E1F2021222324252627\"\n",
            luhn_digit(iccid)
        );
        assert_eq!(out, want);
        let filled = out.lines().nth(2).unwrap();
        assert!(crate::profile::luhn_ok(&filled[8..27]));
    }

    #[test]
    fn luhn_digits() {
        assert_eq!(luhn_digit("890123456789012345"), '5');
        assert_eq!(luhn_digit("7992739871"), '3');
    }

    #[test]
    fn errors() {
        let s = sub();
        assert_eq!(
            fill("a: {nope}\n", &s, counter()).unwrap_err(),
            TemplateError::Unknown {
                line: 1,
                name: "nope".into()
            }
        );
        assert_eq!(
            fill("- { sid: 22, band: 0 }\na: {imsi\n", &s, counter()).unwrap(),
            "- { sid: 22, band: 0 }\na: {imsi\n"
        );
        assert_eq!(
            fill("a: \"{luhn}\"\n", &s, counter()).unwrap_err(),
            TemplateError::LuhnNoDigits { line: 1 }
        );
    }
}
