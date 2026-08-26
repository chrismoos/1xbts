//! A small HDML 2.0 document model and serializer.
//!
//! HDML (Handheld Device Markup Language) is the card/deck markup UP.Browser
//! renders. A deck is `<HDML VERSION=2.0> ... </HDML>` wrapping one or more
//! cards; this gateway emits a single `<DISPLAY>` card per transcoded page.
//! Output is uncompiled `text/x-hdml`, which UP.Browser accepts in place of
//! compiled HDMLc.

/// Inline content within a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// A navigable link. `dest` must be an absolute URL so the follow-up Get
    /// returns to the gateway with a resolvable target.
    Link {
        label: String,
        dest: String,
    },
}

/// A block-level element in a display card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A line of inline content, started with `<LINE>` so long text truncates
    /// rather than wrapping unpredictably.
    Line(Vec<Inline>),
    /// A centered heading line.
    Heading(String),
    /// A blank line.
    Break,
}

/// A rendered HDML deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deck {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    /// `PUBLIC=TRUE`: the deck is reachable from any other deck. The gateway
    /// serves decks from many origins and links freely between them, so without
    /// this the handset raises an access-control error on cross-origin
    /// navigation. Defaults to true.
    pub public: bool,
}

impl Default for Deck {
    fn default() -> Self {
        Deck {
            title: None,
            blocks: Vec::new(),
            public: true,
        }
    }
}

impl Deck {
    pub fn new() -> Self {
        Deck::default()
    }

    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// Serialize to a `text/x-hdml` document.
    pub fn to_hdml(&self) -> String {
        let mut s = String::from("<HDML VERSION=2.0>\n");
        s.push_str("<DISPLAY");
        if let Some(t) = &self.title {
            s.push_str(&format!(" TITLE=\"{}\"", escape_attr(t)));
        }
        s.push_str(">\n");
        for block in &self.blocks {
            match block {
                Block::Heading(text) => {
                    s.push_str("<LINE><CENTER>");
                    s.push_str(&escape_text(text));
                    s.push('\n');
                }
                Block::Line(inlines) => {
                    s.push_str("<LINE>");
                    for inl in inlines {
                        match inl {
                            Inline::Text(t) => s.push_str(&escape_text(t)),
                            Inline::Link { label, dest } => {
                                s.push_str("<A TASK=GO DEST=\"");
                                s.push_str(&escape_attr(dest));
                                s.push_str("\">");
                                s.push_str(&escape_text(label));
                                s.push_str("</A>");
                            }
                        }
                    }
                    s.push('\n');
                }
                Block::Break => s.push_str("<BR>\n"),
            }
        }
        s.push_str("</DISPLAY>\n</HDML>\n");
        s
    }
}

/// Escape HDML text content. HDML shares HTML's `&`, `<`, `>` entities and
/// treats `$` as the variable sigil, escaped by doubling.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a quoted attribute value (adds `"` handling to text escaping).
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

/// Build a minimal single-message deck (used for errors and notices).
pub fn notice_deck(title: &str, message: &str) -> Deck {
    let mut deck = Deck::new();
    deck.title = Some(title.to_string());
    deck.push(Block::Heading(title.to_string()));
    deck.push(Block::Break);
    deck.push(Block::Line(vec![Inline::Text(message.to_string())]));
    deck
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_wraps_display_card() {
        let mut d = Deck::new();
        d.title = Some("Home".into());
        d.push(Block::Heading("1xBTS".into()));
        d.push(Block::Line(vec![Inline::Link {
            label: "Speedtest".into(),
            dest: "http://speed/".into(),
        }]));
        let out = d.to_hdml();
        assert!(out.starts_with("<HDML VERSION=2.0>"));
        assert!(out.contains("<DISPLAY TITLE=\"Home\">"));
        assert!(out.contains("<A TASK=GO DEST=\"http://speed/\">Speedtest</A>"));
        assert!(out.trim_end().ends_with("</HDML>"));
    }

    #[test]
    fn escaping_covers_hdml_specials() {
        assert_eq!(
            escape_text("a & b < c > d $e"),
            "a &amp; b &lt; c &gt; d $$e"
        );
        assert_eq!(escape_attr("x\"y"), "x&quot;y");
    }
}
