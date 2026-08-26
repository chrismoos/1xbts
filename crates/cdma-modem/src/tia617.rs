//! TIA/EIA-617 in-band control-channel codec for the async modem interface
//! (IS-707-A.3 §4.4).
//!
//! Control information (reflected AT commands, response text, result codes,
//! the return-to-online-command "Cellular Escape") is multiplexed into the
//! transparent modem-server octet stream using constructs escaped by `0x19`:
//!
//! ```text
//! <0x19> <extend> <length> <type> <string...>
//!   EM     role     N+0x20   'B'..  0..94 bytes
//! ```
//!
//! The length octet encodes the string length as `len + 0x20` (0x20 = empty …
//! 0x7e = 94 bytes); the type byte is the first payload byte and is *not*
//! counted in the length. Strings longer than 94 bytes are segmented into
//! maximum-size messages followed by a shorter (possibly empty) terminator.

/// Escape byte introducing a construct (`<EM>`).
pub const ESCAPE: u8 = 0x19;
/// Length-octet bias: `length_octet = string_len + LEN_BIAS`.
pub const LEN_BIAS: u8 = 0x20;
/// Maximum string bytes per construct.
pub const MAX_STRING: usize = 94;

// `extend` role bytes, by direction.
pub const EXTEND_IWF0: u8 = 0x60;
pub const EXTEND_IWF1: u8 = 0x61;
pub const EXTEND_MS0: u8 = 0x40;
pub const EXTEND_MS1: u8 = 0x41;

// Type bytes (first payload byte).
pub const TYPE_B: u8 = 0x42; // reflected AT command (IWF→MS) / cellular escape (MS→IWF)
pub const TYPE_C: u8 = 0x43; // response/info text (IWF→MS) / voice request (MS→IWF)
pub const TYPE_D: u8 = 0x44; // MS→IWF result: command unrecognized
pub const TYPE_E: u8 = 0x45; // MS→IWF result: illegal parameter
pub const TYPE_F: u8 = 0x46; // MS→IWF result: command valid
pub const TYPE_G: u8 = 0x47; // MS→IWF info text
pub const TYPE_STATUS: u8 = 0x62; // IWF→MS final result code (STATUS report)

/// A decoded item from the modem-server stream: either transparent bytes or a
/// control construct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Transparent data (AT command bytes in command state, user data online).
    Raw(Vec<u8>),
    /// A TIA-617 control construct.
    Construct {
        extend: u8,
        type_byte: u8,
        string: Vec<u8>,
    },
}

/// Encode one construct. `string` must be ≤ [`MAX_STRING`] bytes.
pub fn encode_construct(extend: u8, type_byte: u8, string: &[u8]) -> Vec<u8> {
    debug_assert!(string.len() <= MAX_STRING);
    let mut out = Vec::with_capacity(4 + string.len());
    out.push(ESCAPE);
    out.push(extend);
    out.push(LEN_BIAS + string.len() as u8);
    out.push(type_byte);
    out.extend_from_slice(string);
    out
}

/// Encode a (possibly long) string as one or more constructs of the same
/// type. Strings ≥ [`MAX_STRING`] are split into max-size segments followed by
/// a shorter (possibly empty) terminating segment.
pub fn encode_message(extend: u8, type_byte: u8, string: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if string.len() < MAX_STRING {
        out.extend(encode_construct(extend, type_byte, string));
        return out;
    }
    for chunk in string.chunks(MAX_STRING) {
        out.extend(encode_construct(extend, type_byte, chunk));
    }
    // If the string length is an exact multiple of MAX_STRING the last chunk
    // was full-size, so a shorter terminator (empty) is required.
    if string.len() % MAX_STRING == 0 {
        out.extend(encode_construct(extend, type_byte, &[]));
    }
    out
}

/// Streaming decoder that separates transparent bytes from control constructs.
///
/// Handles constructs split across `feed` calls. Transparent bytes are grouped
/// into `Item::Raw` runs.
#[derive(Debug, Default)]
pub struct Decoder {
    /// Partial construct bytes buffered after an ESCAPE (excluding ESCAPE).
    partial: Vec<u8>,
    in_construct: bool,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Item> {
        let mut items = Vec::new();
        let mut raw = Vec::new();
        for &b in bytes {
            if self.in_construct {
                self.partial.push(b);
                if let Some(item) = self.try_complete() {
                    if !raw.is_empty() {
                        items.push(Item::Raw(std::mem::take(&mut raw)));
                    }
                    items.push(item);
                    self.in_construct = false;
                    self.partial.clear();
                }
            } else if b == ESCAPE {
                self.in_construct = true;
                self.partial.clear();
            } else {
                raw.push(b);
            }
        }
        if !raw.is_empty() {
            items.push(Item::Raw(raw));
        }
        items
    }

    /// Attempt to parse a complete construct from `self.partial`
    /// (extend, length, type, string). Returns None until fully buffered.
    fn try_complete(&self) -> Option<Item> {
        // Need at least extend + length + type.
        if self.partial.len() < 3 {
            return None;
        }
        let extend = self.partial[0];
        let str_len = self.partial[1].wrapping_sub(LEN_BIAS) as usize;
        let type_byte = self.partial[2];
        let total = 3 + str_len;
        if self.partial.len() < total {
            return None;
        }
        Some(Item::Construct {
            extend,
            type_byte,
            string: self.partial[3..total].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_spec_examples() {
        // Reflected "S0?" (IWF→MS): 0x19 0x60 0x23 0x42 'S' '0' '?'
        assert_eq!(
            encode_construct(EXTEND_IWF0, TYPE_B, b"S0?"),
            vec![0x19, 0x60, 0x23, 0x42, b'S', b'0', b'?']
        );
        // Verbose "OK" STATUS report: 0x19 0x60 0x22 0x62 'O' 'K'
        assert_eq!(
            encode_construct(EXTEND_IWF0, TYPE_STATUS, b"OK"),
            vec![0x19, 0x60, 0x22, 0x62, b'O', b'K']
        );
        // Empty cellular escape (MS→IWF): 0x19 0x41 0x20 0x42
        assert_eq!(
            encode_construct(EXTEND_MS1, TYPE_B, b""),
            vec![0x19, 0x41, 0x20, 0x42]
        );
    }

    #[test]
    fn decode_single_construct() {
        let mut d = Decoder::new();
        let items = d.feed(&[0x19, 0x60, 0x22, 0x62, b'O', b'K']);
        assert_eq!(
            items,
            vec![Item::Construct {
                extend: 0x60,
                type_byte: 0x62,
                string: b"OK".to_vec(),
            }]
        );
    }

    #[test]
    fn decode_raw_and_construct_interleaved() {
        let mut d = Decoder::new();
        let mut stream = b"ATE0".to_vec();
        stream.extend(encode_construct(EXTEND_MS1, TYPE_B, b"")); // escape
        stream.extend_from_slice(b"data");
        let items = d.feed(&stream);
        assert_eq!(
            items,
            vec![
                Item::Raw(b"ATE0".to_vec()),
                Item::Construct {
                    extend: 0x41,
                    type_byte: 0x42,
                    string: vec![],
                },
                Item::Raw(b"data".to_vec()),
            ]
        );
    }

    #[test]
    fn decode_construct_split_across_feeds() {
        let mut d = Decoder::new();
        let full = encode_construct(EXTEND_IWF0, TYPE_B, b"S0?");
        let (a, b) = full.split_at(2);
        assert!(d.feed(a).is_empty());
        assert_eq!(
            d.feed(b),
            vec![Item::Construct {
                extend: 0x60,
                type_byte: 0x42,
                string: b"S0?".to_vec(),
            }]
        );
    }

    #[test]
    fn long_message_segments_and_reassembles() {
        let payload: Vec<u8> = std::iter::repeat_n(b'Q', 200).collect();
        let encoded = encode_message(EXTEND_IWF1, TYPE_C, &payload);
        let mut d = Decoder::new();
        let items = d.feed(&encoded);
        // 200 = 94 + 94 + 12 → three constructs, no empty terminator needed.
        let mut reassembled = Vec::new();
        for it in &items {
            if let Item::Construct { string, .. } = it {
                reassembled.extend_from_slice(string);
            }
        }
        assert_eq!(reassembled, payload);
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn exact_multiple_length_gets_empty_terminator() {
        let payload: Vec<u8> = std::iter::repeat_n(b'X', MAX_STRING).collect();
        let encoded = encode_message(EXTEND_IWF1, TYPE_C, &payload);
        let mut d = Decoder::new();
        let items = d.feed(&encoded);
        // One full 94-byte segment + one empty terminator.
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[1],
            Item::Construct {
                extend: EXTEND_IWF1,
                type_byte: TYPE_C,
                string: vec![],
            }
        );
    }
}
