//! HDTP header encoding.
//!
//! HDTP headers are the same key/value meta-information as HTTP, encoded
//! compactly (HDTP 1.1 draft, "Headers"): each pair is two NUL-terminated
//! byte strings, key then value. A well-known key or value is encoded as a
//! single octet in `0x80..=0xFF` instead of its literal name; the assigned
//! codes are the Well-known Header Key table (Table A-5) and the Well-known
//! Content-Type table (Table A-6).
//!
//! ```text
//! Content-Type: application/x-hdmlc   ->   9A 00 81 00
//! Name: Bob                           ->   'N' 'a' 'm' 'e' 00 'B' 'o' 'b' 00
//! ```

/// Well-known content types with an assigned single-octet value (Table A-6).
pub const CTYPE_X_UP_DIGEST: &str = "application/x-up-digest";
pub const CTYPE_X_HDMLC: &str = "application/x-hdmlc";
/// The uncompiled HDML source type. It has no assigned single-octet code, so it
/// is transmitted literally; UP.Browser accepts it in place of compiled HDMLc.
pub const CTYPE_X_HDML: &str = "text/x-hdml";

// Well-known content types encode as a WSP short-integer (`0x80 | assigned
// number`). The assigned numbers are the WSP content-type registry values that
// the browser itself uses: `text/x-hdml` = 0x04, `application/x-hdmlc` = 0x13.
const CTYPE_HDML_CODE: u8 = 0x84;
const CTYPE_HDMLC_CODE: u8 = 0x93;

/// Well-known header keys, indexed by `code - 0x80` (Table A-5). Two entries map
/// the name "Location": `0x80` is the UP push-location header and `0xA6` the
/// HTTP one; both decode to the same name, and encoding prefers `0xA6`.
const WELL_KNOWN_KEYS: &[&str] = &[
    "Location",            // 0x80
    "x-up-time",           // 0x81
    "x-up-notify",         // 0x82
    "x-up-retry",          // 0x83
    "x-up-errlmt",         // 0x84
    "x-up-maxpdu",         // 0x85
    "x-up-disp",           // 0x86
    "home",                // 0x87
    "Accept-Charset",      // 0x88
    "Accept-Language",     // 0x89
    "x-up-cap",            // 0x8A
    "x-up-devtype",        // 0x8B
    "Accept",              // 0x8C
    "Accept-Encoding",     // 0x8D
    "Accept-Ranges",       // 0x8E
    "Age",                 // 0x8F
    "Allow",               // 0x90
    "Authorization",       // 0x91
    "Cache-Control",       // 0x92
    "Connection",          // 0x93
    "Content-Base",        // 0x94
    "Content-Encoding",    // 0x95
    "Content-Language",    // 0x96
    "Content-Location",    // 0x97
    "Content-MD5",         // 0x98
    "Content-Range",       // 0x99
    "Content-Type",        // 0x9A
    "Date",                // 0x9B
    "ETag",                // 0x9C
    "Expires",             // 0x9D
    "From",                // 0x9E
    "Host",                // 0x9F
    "If-Modified-Since",   // 0xA0
    "If-Match",            // 0xA1
    "If-None-Match",       // 0xA2
    "If-Range",            // 0xA3
    "If-Unmodified-Since", // 0xA4
    "Last-Modified",       // 0xA5
    "Location",            // 0xA6
    "Max-Forwards",        // 0xA7
    "Pragma",              // 0xA8
    "Proxy-Authenticate",  // 0xA9
    "Proxy-Authorization", // 0xAA
    "Public",              // 0xAB
    "Range",               // 0xAC
    "Referer",             // 0xAD
    "Retry-After",         // 0xAE
    "Server",              // 0xAF
    "Transfer-Encoding",   // 0xB0
    "Upgrade",             // 0xB1
    "User-Agent",          // 0xB2
    "Vary",                // 0xB3
    "Via",                 // 0xB4
    "Warning",             // 0xB5
    "WWW-Authenticate",    // 0xB6
];

const WELL_KNOWN_BASE: u8 = 0x80;

/// Canonical name for a well-known key code, or `None` if unassigned.
pub fn well_known_key_name(code: u8) -> Option<&'static str> {
    if code < WELL_KNOWN_BASE {
        return None;
    }
    WELL_KNOWN_KEYS
        .get((code - WELL_KNOWN_BASE) as usize)
        .copied()
}

/// Single-octet code for a header key name (case-insensitive), preferring the
/// HTTP assignment where a name is duplicated.
pub fn well_known_key_code(name: &str) -> Option<u8> {
    let mut found = None;
    for (i, k) in WELL_KNOWN_KEYS.iter().enumerate() {
        if k.eq_ignore_ascii_case(name) {
            found = Some(WELL_KNOWN_BASE + i as u8);
        }
    }
    found
}

/// A `"0xNN"` name encodes to the raw single-octet key it names. Handset
/// capability keys (e.g. the SUGP version key `0x91`) do not all match this
/// crate's HTTP-derived key table, so a reply that must echo one by its exact
/// wire code carries it under a `"0xNN"` name.
fn raw_key_code(name: &str) -> Option<u8> {
    let hex = name
        .strip_prefix("0x")
        .or_else(|| name.strip_prefix("0X"))?;
    (hex.len() == 2)
        .then(|| u8::from_str_radix(hex, 16).ok())
        .flatten()
}

fn well_known_ctype_code(value: &str) -> Option<u8> {
    if value.eq_ignore_ascii_case(CTYPE_X_HDML) {
        Some(CTYPE_HDML_CODE)
    } else if value.eq_ignore_ascii_case(CTYPE_X_HDMLC) {
        Some(CTYPE_HDMLC_CODE)
    } else {
        None
    }
}

fn well_known_ctype_name(code: u8) -> Option<&'static str> {
    match code {
        CTYPE_HDML_CODE => Some(CTYPE_X_HDML),
        CTYPE_HDMLC_CODE => Some(CTYPE_X_HDMLC),
        _ => None,
    }
}

/// A decoded header key/value pair. Names decode to their canonical HTTP
/// spelling; unassigned single-octet keys decode to `"0xNN"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// An ordered list of HDTP headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(pub Vec<Header>);

impl Headers {
    pub fn new() -> Self {
        Headers(Vec::new())
    }

    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.push(Header::new(name, value));
    }

    /// First value for a header name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Encode the header block to wire bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for h in &self.0 {
            match well_known_key_code(&h.name).or_else(|| raw_key_code(&h.name)) {
                Some(code) => out.push(code),
                None => out.extend_from_slice(h.name.as_bytes()),
            }
            out.push(0);
            // Content-Type carries a well-known single-octet value set.
            let is_ctype = h.name.eq_ignore_ascii_case("Content-Type");
            match is_ctype.then(|| well_known_ctype_code(&h.value)).flatten() {
                Some(code) => out.push(code),
                None => out.extend_from_slice(h.value.as_bytes()),
            }
            out.push(0);
        }
        out
    }

    /// Decode a header block. `buf` must contain only the header bytes. The
    /// final value may be unterminated (bounded by the enclosing length).
    pub fn decode(buf: &[u8]) -> Headers {
        let mut headers = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let (name, next) = read_key(buf, i);
            i = next;
            let (value, next) = read_value(buf, i, &name);
            i = next;
            headers.push(Header { name, value });
        }
        Headers(headers)
    }
}

/// Read a byte string up to the next NUL. If there is no NUL before the end of
/// the buffer (the handset omits the terminator on the final header value,
/// relying on the enclosing HeadersLen), the rest of the buffer is the string.
fn read_cstr(buf: &[u8], start: usize) -> (&[u8], usize) {
    match buf[start..].iter().position(|&b| b == 0) {
        Some(p) => (&buf[start..start + p], start + p + 1),
        None => (&buf[start..], buf.len()),
    }
}

fn read_key(buf: &[u8], start: usize) -> (String, usize) {
    let (raw, next) = read_cstr(buf, start);
    if raw.len() == 1 && raw[0] >= WELL_KNOWN_BASE {
        let name = well_known_key_name(raw[0])
            .map(str::to_owned)
            .unwrap_or_else(|| format!("0x{:02x}", raw[0]));
        (name, next)
    } else {
        (String::from_utf8_lossy(raw).into_owned(), next)
    }
}

fn read_value(buf: &[u8], start: usize, key: &str) -> (String, usize) {
    let (raw, next) = read_cstr(buf, start);
    if raw.len() == 1
        && raw[0] >= WELL_KNOWN_BASE
        && key.eq_ignore_ascii_case("Content-Type")
        && let Some(name) = well_known_ctype_name(raw[0])
    {
        return (name.to_owned(), next);
    }
    (String::from_utf8_lossy(raw).into_owned(), next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_encodes_wsp_short_integer() {
        // Content-Type value is the WSP short-integer code, not a literal.
        let mut h = Headers::new();
        h.push("Content-Type", CTYPE_X_HDMLC);
        assert_eq!(h.encode(), vec![0x9A, 0x00, 0x93, 0x00]); // application/x-hdmlc = 0x13
        let mut h = Headers::new();
        h.push("Content-Type", CTYPE_X_HDML);
        assert_eq!(h.encode(), vec![0x9A, 0x00, 0x84, 0x00]); // text/x-hdml = 0x04
    }

    #[test]
    fn literal_pair_matches_spec_example() {
        let mut h = Headers::new();
        h.push("Name", "Bob");
        assert_eq!(h.encode(), b"Name\x00Bob\x00");
    }

    #[test]
    fn roundtrip_mixed() {
        let mut h = Headers::new();
        h.push("Content-Type", CTYPE_X_HDMLC);
        h.push("Accept-Language", "en-us");
        h.push("X-Thing", "1,2,3");
        let bytes = h.encode();
        let back = Headers::decode(&bytes);
        assert_eq!(back.get("Content-Type").unwrap(), CTYPE_X_HDMLC);
        assert_eq!(back.get("Accept-Language").unwrap(), "en-us");
        assert_eq!(back.get("X-Thing").unwrap(), "1,2,3");
    }

    #[test]
    fn decodes_captured_capability_block() {
        // The 80-byte header block from the live SessionRequest capture: 15
        // well-known-key pairs, Accept-Language="en-us" among them.
        let block = [
            0x91u8, 0x00, b'3', b'.', b'1', b',', b'2', b'.', b'0', 0x00, 0x8c, 0x00, 0x00, 0xa0,
            0x00, b'3', 0x00, 0x89, 0x00, b'e', b'n', b'-', b'u', b's', 0x00, 0x93, 0x00, b'2',
            0x00, 0x94, 0x00, b'5', 0x00, 0x95, 0x00, b'1', b'2', b',', b'3', 0x00, 0x96, 0x00,
            b'1', b'2', b',', b'2', 0x00, 0x97, 0x00, b'1', 0x00, 0x98, 0x00, b'0', 0x00, 0x99,
            0x00, b'1', 0x00, 0x9a, 0x00, b'0', 0x00, 0x9b, 0x00, b'1', b'4', b'9', b'2', 0x00,
            0x85, 0x00, b'0', b'2', 0x00, 0x9c, 0x00, b'1', 0x00,
        ];
        let h = Headers::decode(&block);
        assert_eq!(h.0.len(), 15);
        assert_eq!(h.get("Accept-Language").unwrap(), "en-us");
        assert_eq!(h.get("Authorization").unwrap(), "3.1,2.0");
        assert_eq!(h.get("x-up-maxpdu").unwrap(), "02");
    }
}
