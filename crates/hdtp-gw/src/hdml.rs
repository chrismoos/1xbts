//! The HDML serialization of a deck.
//!
//! UP.Browser renders HDML card decks. The document model itself is shared with
//! the WAP gateway and lives in [`gw_transcode::doc`]. This module turns one
//! into a `text/x-hdml` document.

pub use gw_transcode::doc::{Block, Deck, Field, Form, Inline, notice_deck};

/// Serialize a deck to a `text/x-hdml` document.
pub fn to_hdml(deck: &Deck) -> String {
    let mut s = String::from("<HDML VERSION=2.0>\n");
    s.push_str("<DISPLAY");
    if let Some(t) = &deck.title {
        s.push_str(&format!(" TITLE=\"{}\"", escape_attr(t)));
    }
    s.push_str(">\n");
    for block in &deck.blocks {
        match block {
            Block::Heading(text) => {
                s.push_str("<LINE><CENTER>");
                s.push_str(&escape_text(text));
                s.push('\n');
            }
            Block::Line(inlines) => {
                s.push_str("<LINE>");
                push_inlines(&mut s, inlines);
                s.push('\n');
            }
            Block::Break => s.push_str("<BR>\n"),
            Block::Form(form) => push_form(&mut s, form),
        }
    }
    s.push_str("</DISPLAY>\n</HDML>\n");
    s
}

fn push_inlines(s: &mut String, inlines: &[Inline]) {
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
}

/// Render a form as its field labels plus a link to the action.
///
/// Filling fields in HDML needs `<ENTRY>` cards, a second card model this deck
/// serializer does not emit, so the submission carries no field values and the
/// handset reaches the action with a plain Go.
fn push_form(s: &mut String, form: &Form) {
    for field in &form.fields {
        let title = match field {
            Field::Text { title, .. } | Field::Select { title, .. } => title,
            Field::Hidden { .. } => continue,
        };
        s.push_str("<LINE>");
        s.push_str(&escape_text(title));
        s.push_str(":\n");
    }
    s.push_str("<LINE><A TASK=GO DEST=\"");
    s.push_str(&escape_attr(&form.action));
    s.push_str("\">");
    s.push_str(&escape_text(&form.submit_label));
    s.push_str("</A>\n");
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

#[cfg(test)]
mod tests {
    use super::*;
    use gw_transcode::doc::FormMethod;

    #[test]
    fn deck_wraps_display_card() {
        let mut d = Deck::new();
        d.title = Some("Home".into());
        d.push(Block::Heading("1xBTS".into()));
        d.push(Block::Line(vec![Inline::Link {
            label: "Speedtest".into(),
            dest: "http://speed/".into(),
        }]));
        let out = to_hdml(&d);
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

    #[test]
    fn dollar_in_a_url_is_doubled_in_the_attribute() {
        let mut d = Deck::new();
        d.push(Block::Line(vec![Inline::Link {
            label: "Pay".into(),
            dest: "http://x/?amt=$100".into(),
        }]));
        // A bare `$` would start a variable reference on the handset.
        assert!(to_hdml(&d).contains("DEST=\"http://x/?amt=$$100\""));
    }

    #[test]
    fn form_degrades_to_labels_and_an_action_link() {
        let mut d = Deck::new();
        d.push(Block::Form(Form {
            action: "http://example.com/search".into(),
            method: FormMethod::Post,
            fields: vec![
                Field::Text {
                    name: "q".into(),
                    title: "Query".into(),
                    value: String::new(),
                    secret: false,
                },
                Field::Hidden {
                    name: "src".into(),
                    value: "hdml".into(),
                },
            ],
            submit_label: "Go".into(),
        }));
        let out = to_hdml(&d);
        assert!(out.contains("<LINE>Query:"));
        // The hidden field is never shown.
        assert!(!out.contains("src"));
        assert!(out.contains("<A TASK=GO DEST=\"http://example.com/search\">Go</A>"));
    }
}
