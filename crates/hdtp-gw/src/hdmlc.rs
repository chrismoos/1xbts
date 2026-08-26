//! HDMLc: the compiled (tokenized) HDML the UP.Browser renders.
//!
//! UP.Browser accepts `application/x-hdmlc`, not HDML source. The compiled form
//! ("TIL") is a proprietary Phone.com tokenization that was never published; the
//! encoding here is validated byte-for-byte in the tests below.
//!
//! ```text
//! deck = cf 01 03 00  <len BE, ff-terminated>  80 <ver>  <card>  89
//! DISPLAY card = 81 <text-line...> 8a
//! CHOICE  card = 82 <text-line...> <choice-entry...> 8b
//! text line    = 99 [b1] <text> 00              (b1 = centered)
//! choice entry = 92 d7 f2 d8 <dest> 00 9a <label> 00   (a TASK=GO link)
//! ```
//! `<len>` is the byte count from the `ff` separator through the trailing `89`,
//! big-endian minimal width (`0f`, or `01 37` for a 311-byte deck).

use crate::hdml::{Block, Deck, Inline};

mod tok {
    pub const DECK_HDR: [u8; 4] = [0xcf, 0x01, 0x03, 0x00];
    pub const LEN_SEP: [u8; 2] = [0xff, 0x00];
    pub const DECK_VER: u8 = 0x80;
    pub const VER_2_0: u8 = 0x10;
    /// `PUBLIC=TRUE`, emitted right after the deck version. Marks the deck
    /// reachable from any origin so cross-deck navigation avoids an
    /// access-control error.
    pub const ACCESS_PUBLIC: [u8; 2] = [0xde, 0xec];
    pub const CARD_DISPLAY: u8 = 0x81;
    pub const CARD_CHOICE: u8 = 0x82;
    pub const DISPLAY_CLOSE: u8 = 0x8a;
    pub const CHOICE_CLOSE: u8 = 0x8b;
    pub const DECK_CLOSE: u8 = 0x89;
    pub const TEXT_LINE: u8 = 0x99;
    pub const CENTER: u8 = 0xb1;
    /// Choice entry with a `TASK=GO DEST=` link.
    pub const CE_GO_DEST: [u8; 4] = [0x92, 0xd7, 0xf2, 0xd8];
    pub const CE_LABEL: u8 = 0x9a;
}

fn push_line(out: &mut Vec<u8>, text: &str, center: bool) {
    out.push(tok::TEXT_LINE);
    if center {
        out.push(tok::CENTER);
    }
    out.extend(text.bytes().filter(|&b| b != 0));
    out.push(0x00);
}

fn push_ce(out: &mut Vec<u8>, label: &str, dest: &str) {
    out.extend_from_slice(&tok::CE_GO_DEST);
    out.extend(dest.bytes().filter(|&b| b != 0));
    out.push(0x00);
    out.push(tok::CE_LABEL);
    out.extend(label.bytes().filter(|&b| b != 0));
    out.push(0x00);
}

/// Compile a [`Deck`] to an HDMLc document. If the deck has links it becomes a
/// CHOICE card (its text is the prompt, its links are navigable entries);
/// otherwise a DISPLAY card.
pub fn compile_deck(deck: &Deck) -> Vec<u8> {
    let mut lines: Vec<(String, bool)> = Vec::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for block in &deck.blocks {
        match block {
            Block::Heading(t) => lines.push((t.clone(), true)),
            Block::Break => lines.push((String::new(), false)),
            Block::Line(inlines) => {
                // Text stays in the line; links become navigable choice entries.
                let mut text = String::new();
                for inl in inlines {
                    match inl {
                        Inline::Text(t) => text.push_str(t),
                        Inline::Link { label, dest } => links.push((label.clone(), dest.clone())),
                    }
                }
                lines.push((text, false));
            }
        }
    }

    let mut body = vec![tok::DECK_VER, tok::VER_2_0];
    if deck.public {
        body.extend_from_slice(&tok::ACCESS_PUBLIC);
    }
    if links.is_empty() {
        body.push(tok::CARD_DISPLAY);
        for (text, center) in &lines {
            push_line(&mut body, text, *center);
        }
        body.push(tok::DISPLAY_CLOSE);
    } else {
        body.push(tok::CARD_CHOICE);
        // Prompt lines: text that is not itself a link label.
        for (text, center) in &lines {
            if !text.is_empty() && !links.iter().any(|(l, _)| l == text) {
                push_line(&mut body, text, *center);
            }
        }
        for (label, dest) in &links {
            push_ce(&mut body, label, dest);
        }
        body.push(tok::CHOICE_CLOSE);
    }
    body.push(tok::DECK_CLOSE);

    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&tok::DECK_HDR);
    out.extend_from_slice(&encode_len(2 + body.len()));
    out.extend_from_slice(&tok::LEN_SEP);
    out.extend_from_slice(&body);
    out
}

/// The deck length: big-endian, minimal width (the `ff` separator that follows
/// terminates it).
fn encode_len(len: usize) -> Vec<u8> {
    if len == 0 {
        return vec![0];
    }
    let mut v = Vec::new();
    let mut n = len;
    while n > 0 {
        v.push((n & 0xff) as u8);
        n >>= 8;
    }
    v.reverse();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn line(t: &str) -> Block {
        Block::Line(vec![Inline::Text(t.into())])
    }
    fn deck(blocks: Vec<Block>) -> Deck {
        // The byte-exact goldens below were compiled from `<HDML VERSION=2.0>`
        // without PUBLIC, so the helper keeps public off; the PUBLIC token has
        // its own golden in `public_inserts_access_token`.
        Deck {
            title: None,
            blocks,
            public: false,
        }
    }

    // All expectations are exact HDMLc byte sequences.

    #[test]
    fn display_single_text() {
        assert_eq!(
            compile_deck(&deck(vec![line("PORTALDECK1X")])),
            hex("cf01030015ff0080108199504f5254414c4445434b3158008a89")
        );
        assert_eq!(
            compile_deck(&deck(vec![line("MKAAAA")])),
            hex("cf0103000fff00801081994d4b41414141008a89")
        );
        assert_eq!(
            compile_deck(&deck(vec![line("MZPLAIN")])),
            hex("cf01030010ff00801081994d5a504c41494e008a89")
        );
    }

    #[test]
    fn display_break_and_center() {
        assert_eq!(
            compile_deck(&deck(vec![line("MKBR"), line("SECOND")])),
            hex("cf01030015ff00801081994d4b425200995345434f4e44008a89")
        );
        assert_eq!(
            compile_deck(&deck(vec![Block::Heading("MKCEN".into())])),
            hex("cf0103000fff0080108199b14d4b43454e008a89")
        );
    }

    #[test]
    fn choice_with_one_link() {
        // <CHOICE>MKCHO<CE TASK=GO DEST="http://h/">OPT</CE></CHOICE>
        let d = deck(vec![Block::Line(vec![
            Inline::Text("MKCHO".into()),
            Inline::Link {
                label: "OPT".into(),
                dest: "http://h/".into(),
            },
        ])]);
        assert_eq!(
            compile_deck(&d),
            hex("cf01030021ff00801082994d4b43484f0092d7f2d8687474703a2f2f682f009a4f5054008b89")
        );
    }

    #[test]
    fn choice_with_two_links() {
        // <CHOICE>QCHO2<CE ...http://a/>OPTONE</CE><CE ...http://b/>OPTTWO</CE></CHOICE>
        let d = deck(vec![
            Block::Line(vec![Inline::Text("QCHO2".into())]),
            Block::Line(vec![Inline::Link {
                label: "OPTONE".into(),
                dest: "http://a/".into(),
            }]),
            Block::Line(vec![Inline::Link {
                label: "OPTTWO".into(),
                dest: "http://b/".into(),
            }]),
        ]);
        assert_eq!(
            compile_deck(&d),
            hex(
                "cf0103003aff00801082995143484f320092d7f2d8687474703a2f2f612f009a4f50544f4e450092d7f2d8687474703a2f2f622f009a4f505454574f008b89"
            )
        );
    }

    #[test]
    fn choice_home_deck_with_real_urls() {
        // The gateway's portal deck shape, validated against the real compiler.
        let d = deck(vec![
            Block::Line(vec![Inline::Text("1xBTSPortal".into())]),
            Block::Line(vec![Inline::Link {
                label: "Example".into(),
                dest: "http://example.com/".into(),
            }]),
            Block::Line(vec![Inline::Link {
                label: "Wikipedia".into(),
                dest: "http://en.wikipedia.org/wiki/WAP".into(),
            }]),
        ]);
        assert_eq!(
            compile_deck(&d),
            hex(
                "cf01030065ff00801082993178425453506f7274616c0092d7f2d8687474703a2f2f6578616d706c652e636f6d2f009a4578616d706c650092d7f2d8687474703a2f2f656e2e77696b6970656469612e6f72672f77696b692f574150009a57696b697065646961008b89"
            )
        );
    }

    #[test]
    fn public_inserts_access_token() {
        // PUBLIC=TRUE inserts `de ec` after the deck version; length grows by 2.
        // Golden from the real compiler for
        // `<HDML VERSION=2.0 PUBLIC=TRUE><DISPLAY>MZPUBAB</DISPLAY></HDML>`.
        let mut d = deck(vec![line("MZPUBAB")]);
        d.public = true;
        assert_eq!(
            compile_deck(&d),
            hex("cf01030012ff008010deec81994d5a5055424142008a89")
        );
    }

    #[test]
    fn length_is_big_endian_minimal() {
        assert_eq!(encode_len(0x0f), vec![0x0f]);
        assert_eq!(encode_len(0x0137), vec![0x01, 0x37]);
    }
}
