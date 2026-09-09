//! The WML serialization of a deck.
//!
//! A WAP browser renders WML card decks. The document model is shared with the
//! HDTP gateway and lives in [`gw_transcode::doc`]. This module turns one into a
//! `text/vnd.wap.wml` document, which Kannel then compiles to WBXML.
//!
//! Forms carry through: a field becomes a WML `<input>` or `<select>` bound to a
//! browser variable, and the submit becomes an `<anchor>` wrapping a `<go>` that
//! posts those variables back as `<postfield>` entries.

use gw_transcode::doc::{Block, Deck, Field, Form, FormMethod, Inline};

/// MIME type of the documents this module emits. The charset is stated because
/// a gateway that has to guess at one falls back to ISO-8859-1 and mangles any
/// text outside it.
pub const CTYPE_WML: &str = "text/vnd.wap.wml; charset=utf-8";

/// The WML 1.1 document type declaration. The public and system identifiers are
/// fixed by the WML specification: a browser and Kannel's WBXML encoder both
/// select the tag-code table from them, so neither can be altered or dropped.
const WML_DOCTYPE: &str = concat!(
    "<!DOCTYPE wml PUBLIC \"-//WAPFORUM//DTD WML 1.1//EN\" ",
    "\"http://www.wapforum.org/DTD/wml_1.1.xml\">"
);

/// Serialize a deck to a `text/vnd.wap.wml` document.
pub fn to_wml(deck: &Deck) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(WML_DOCTYPE);
    s.push_str("\n<wml>\n<card id=\"main\"");
    if let Some(t) = &deck.title {
        s.push_str(&format!(" title=\"{}\"", escape_attr(t)));
    }
    s.push_str(">\n");
    for block in &deck.blocks {
        match block {
            Block::Heading(text) => {
                s.push_str("<p align=\"center\"><b>");
                s.push_str(&escape_text(text));
                s.push_str("</b></p>\n");
            }
            Block::Line(inlines) => {
                s.push_str("<p>");
                push_inlines(&mut s, inlines);
                s.push_str("</p>\n");
            }
            Block::Break => s.push_str("<p><br/></p>\n"),
            Block::Form(form) => push_form(&mut s, form),
        }
    }
    s.push_str("</card>\n</wml>\n");
    s
}

fn push_inlines(s: &mut String, inlines: &[Inline]) {
    for inl in inlines {
        match inl {
            Inline::Text(t) => s.push_str(&escape_text(t)),
            Inline::Link { label, dest } => {
                s.push_str("<a href=\"");
                s.push_str(&escape_attr(dest));
                s.push_str("\">");
                s.push_str(&escape_text(label));
                s.push_str("</a>");
            }
        }
    }
}

fn push_form(s: &mut String, form: &Form) {
    // A field's HTML name is not necessarily a legal WML variable name, so each
    // visible field gets a sanitized variable and the postfield carries the
    // original name on the wire.
    let vars: Vec<String> = assign_vars(form);

    for (field, var) in form.fields.iter().zip(&vars) {
        match field {
            Field::Text {
                title,
                value,
                secret,
                ..
            } => {
                s.push_str("<p>");
                s.push_str(&escape_text(title));
                s.push_str(": <input name=\"");
                s.push_str(var);
                s.push('"');
                if !value.is_empty() {
                    s.push_str(&format!(" value=\"{}\"", escape_attr(value)));
                }
                if *secret {
                    s.push_str(" type=\"password\"");
                }
                s.push_str("/></p>\n");
            }
            Field::Select { title, options, .. } => {
                s.push_str("<p>");
                s.push_str(&escape_text(title));
                s.push_str(": <select name=\"");
                s.push_str(var);
                s.push_str("\">\n");
                for (value, label) in options {
                    s.push_str("<option value=\"");
                    s.push_str(&escape_attr(value));
                    s.push_str("\">");
                    s.push_str(&escape_text(label));
                    s.push_str("</option>\n");
                }
                s.push_str("</select></p>\n");
            }
            Field::Hidden { .. } => {}
        }
    }

    let method = match form.method {
        FormMethod::Post => "post",
        FormMethod::Get => "get",
    };
    s.push_str("<p><anchor>");
    s.push_str(&escape_text(&form.submit_label));
    s.push_str("<go href=\"");
    s.push_str(&escape_attr(&form.action));
    s.push_str(&format!("\" method=\"{method}\">\n"));
    for (field, var) in form.fields.iter().zip(&vars) {
        s.push_str("<postfield name=\"");
        s.push_str(&escape_attr(field.name()));
        s.push_str("\" value=\"");
        match field {
            // A literal value needs no variable indirection.
            Field::Hidden { value, .. } => s.push_str(&escape_attr(value)),
            // `$(name)` is a variable reference, so the sigil stays unescaped.
            _ => s.push_str(&format!("$({var})")),
        }
        s.push_str("\"/>\n");
    }
    s.push_str("</go></anchor></p>\n");
}

/// Give each field a distinct, legal WML variable name derived from its HTML
/// name. WML variables allow letters, digits and underscore, and may not start
/// with a digit.
fn assign_vars(form: &Form) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    let mut out = Vec::with_capacity(form.fields.len());
    for (i, field) in form.fields.iter().enumerate() {
        let mut v: String = field
            .name()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        if v.is_empty() || v.starts_with(|c: char| c.is_ascii_digit()) {
            v.insert(0, 'f');
        }
        while used.contains(&v) {
            v.push_str(&i.to_string());
        }
        used.push(v.clone());
        out.push(v);
    }
    out
}

/// Escape WML text content. WML shares HTML's `&`, `<`, `>` entities and treats
/// `$` as the variable sigil, escaped by doubling.
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

/// Escape a quoted attribute value (adds `"` and `'` to text escaping).
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_wraps_a_single_card() {
        let mut d = Deck::new();
        d.title = Some("Home".into());
        d.push(Block::Heading("1xBTS".into()));
        d.push(Block::Line(vec![Inline::Link {
            label: "Speedtest".into(),
            dest: "http://speed/".into(),
        }]));
        let out = to_wml(&d);
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(out.contains("-//WAPFORUM//DTD WML 1.1//EN"));
        assert!(out.contains("<card id=\"main\" title=\"Home\">"));
        assert!(out.contains("<a href=\"http://speed/\">Speedtest</a>"));
        assert!(out.trim_end().ends_with("</wml>"));
    }

    #[test]
    fn escaping_covers_wml_specials() {
        assert_eq!(
            escape_text("a & b < c > d $e"),
            "a &amp; b &lt; c &gt; d $$e"
        );
        assert_eq!(escape_attr("x\"y'z"), "x&quot;y&apos;z");
    }

    #[test]
    fn post_form_binds_inputs_to_postfields() {
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
                    value: "wap".into(),
                },
            ],
            submit_label: "Go".into(),
        }));
        let out = to_wml(&d);
        assert!(out.contains("<input name=\"q\"/>"));
        assert!(out.contains("<go href=\"http://example.com/search\" method=\"post\">"));
        // The typed value reaches the server through its variable.
        assert!(out.contains("<postfield name=\"q\" value=\"$(q)\"/>"));
        // A hidden field is submitted literally and never rendered.
        assert!(out.contains("<postfield name=\"src\" value=\"wap\"/>"));
        assert!(!out.contains("<input name=\"src\""));
    }

    #[test]
    fn field_names_become_legal_wml_variables() {
        let mut d = Deck::new();
        d.push(Block::Form(Form {
            action: "http://x/".into(),
            method: FormMethod::Get,
            fields: vec![Field::Text {
                name: "user[name]".into(),
                title: "Name".into(),
                value: String::new(),
                secret: false,
            }],
            submit_label: "Send".into(),
        }));
        let out = to_wml(&d);
        assert!(out.contains("<input name=\"user_name_\"/>"));
        // The wire name keeps the original spelling.
        assert!(out.contains("<postfield name=\"user[name]\" value=\"$(user_name_)\"/>"));
    }

    #[test]
    fn password_fields_are_masked() {
        let mut d = Deck::new();
        d.push(Block::Form(Form {
            action: "http://x/".into(),
            method: FormMethod::Post,
            fields: vec![Field::Text {
                name: "pw".into(),
                title: "Password".into(),
                value: String::new(),
                secret: true,
            }],
            submit_label: "Log in".into(),
        }));
        assert!(to_wml(&d).contains("type=\"password\""));
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::*;

    /// Parse an emitted deck, failing the test if it is not well-formed XML.
    /// Every deck carries the WML doctype, so the parser has to accept a DTD.
    fn parse(wml: &str) -> roxmltree::Document<'_> {
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        };
        match roxmltree::Document::parse_with_options(wml, opts) {
            Ok(d) => d,
            Err(e) => panic!("emitted WML is not well-formed: {e}\n---\n{wml}"),
        }
    }

    /// Rendered text of the document, in order. Only paragraph content counts,
    /// since the newlines separating elements are layout rather than content.
    fn text_of(doc: &roxmltree::Document<'_>) -> String {
        doc.descendants()
            .filter(|n| n.has_tag_name("p"))
            .flat_map(|p| {
                p.descendants()
                    .filter(roxmltree::Node::is_text)
                    .filter_map(|n| n.text().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .concat()
    }

    /// Undo the WML-level `$` doubling an XML parser leaves behind.
    fn undouble(s: &str) -> String {
        s.replace("$$", "$")
    }

    fn rich_deck() -> Deck {
        let mut d = Deck::new();
        d.title = Some("A & B \"quoted\" <tag>".into());
        d.push(Block::Heading("Heading & more".into()));
        d.push(Block::Break);
        d.push(Block::Line(vec![
            Inline::Text("before ".into()),
            Inline::Link {
                label: "click <me>".into(),
                dest: "http://x/?a=1&b=2".into(),
            },
            Inline::Text(" after".into()),
        ]));
        d.push(Block::Form(Form {
            action: "http://x/go?q=1&r=2".into(),
            method: FormMethod::Post,
            fields: vec![
                Field::Text {
                    name: "q".into(),
                    title: "Query & such".into(),
                    value: "pre\"set".into(),
                    secret: false,
                },
                Field::Hidden {
                    name: "amt".into(),
                    value: "$100 & rising".into(),
                },
                Field::Select {
                    name: "lang".into(),
                    title: "Language".into(),
                    options: vec![("a&b".into(), "A & B".into())],
                },
            ],
            submit_label: "Go <now>".into(),
        }));
        d
    }

    #[test]
    fn emitted_deck_is_well_formed_xml() {
        let wml = to_wml(&rich_deck());
        let doc = parse(&wml);
        assert_eq!(doc.root_element().tag_name().name(), "wml");
    }

    #[test]
    fn empty_deck_is_well_formed() {
        let wml = to_wml(&Deck::new());
        let doc = parse(&wml);
        assert_eq!(doc.root_element().tag_name().name(), "wml");
    }

    #[test]
    fn text_escaping_round_trips_through_a_parser() {
        let mut d = Deck::new();
        let raw = "amp & lt < gt > quote \" apos ' dollar $ done";
        d.push(Block::Line(vec![Inline::Text(raw.into())]));
        let wml = to_wml(&d);
        let doc = parse(&wml);
        assert_eq!(undouble(&text_of(&doc)), raw);
    }

    #[test]
    fn attribute_escaping_round_trips_through_a_parser() {
        let mut d = Deck::new();
        d.title = Some("t \" & < > ' $".into());
        d.push(Block::Line(vec![Inline::Link {
            label: "l".into(),
            dest: "http://x/?a=1&b=2\"c'd".into(),
        }]));
        let wml = to_wml(&d);
        let doc = parse(&wml);
        let card = doc
            .descendants()
            .find(|n| n.has_tag_name("card"))
            .expect("card");
        assert_eq!(undouble(card.attribute("title").unwrap()), "t \" & < > ' $");
        let a = doc.descendants().find(|n| n.has_tag_name("a")).expect("a");
        assert_eq!(
            undouble(a.attribute("href").unwrap()),
            "http://x/?a=1&b=2\"c'd"
        );
    }

    #[test]
    fn a_literal_dollar_is_doubled_but_a_variable_reference_is_not() {
        let wml = to_wml(&rich_deck());
        // The hidden field's literal value is WML-escaped, so the browser shows
        // a dollar sign instead of dereferencing a variable named `100`.
        assert!(wml.contains(r#"<postfield name="amt" value="$$100 &amp; rising"/>"#));
        // The typed field's value is a variable reference and stays bare.
        assert!(wml.contains(r#"<postfield name="q" value="$(q)"/>"#));
        parse(&wml);
    }

    #[test]
    fn select_option_values_and_labels_are_escaped() {
        let doc_src = to_wml(&rich_deck());
        let doc = parse(&doc_src);
        let opt = doc
            .descendants()
            .find(|n| n.has_tag_name("option"))
            .expect("option");
        assert_eq!(opt.attribute("value").unwrap(), "a&b");
        assert_eq!(opt.text().unwrap(), "A & B");
    }

    #[test]
    fn non_ascii_text_survives() {
        let mut d = Deck::new();
        let raw = "Suomi — ÅÄÖ — 日本語 — emoji 🛰";
        d.title = Some(raw.into());
        d.push(Block::Line(vec![Inline::Text(raw.into())]));
        let wml = to_wml(&d);
        let doc = parse(&wml);
        assert_eq!(text_of(&doc), raw);
        let card = doc
            .descendants()
            .find(|n| n.has_tag_name("card"))
            .expect("card");
        assert_eq!(card.attribute("title").unwrap(), raw);
    }

    #[test]
    fn colliding_field_names_get_distinct_variables() {
        let mut d = Deck::new();
        d.push(Block::Form(Form {
            action: "http://x/".into(),
            method: FormMethod::Get,
            fields: vec![
                Field::Text {
                    name: "a[1]".into(),
                    title: "One".into(),
                    value: String::new(),
                    secret: false,
                },
                Field::Text {
                    name: "a.1".into(),
                    title: "Two".into(),
                    value: String::new(),
                    secret: false,
                },
            ],
            submit_label: "Go".into(),
        }));
        let wml = to_wml(&d);
        let doc = parse(&wml);
        let vars: Vec<String> = doc
            .descendants()
            .filter(|n| n.has_tag_name("input"))
            .map(|n| n.attribute("name").unwrap().to_string())
            .collect();
        assert_eq!(vars.len(), 2);
        assert_ne!(vars[0], vars[1], "each field needs its own WML variable");
        // Both wire names survive untouched.
        let wire: Vec<String> = doc
            .descendants()
            .filter(|n| n.has_tag_name("postfield"))
            .map(|n| n.attribute("name").unwrap().to_string())
            .collect();
        assert_eq!(wire, vec!["a[1]".to_string(), "a.1".to_string()]);
    }

    #[test]
    fn every_input_variable_is_a_legal_wml_name() {
        let wml = to_wml(&rich_deck());
        let doc = parse(&wml);
        for n in doc
            .descendants()
            .filter(|n| n.has_tag_name("input") || n.has_tag_name("select"))
        {
            let name = n.attribute("name").unwrap();
            assert!(!name.is_empty());
            assert!(
                !name.starts_with(|c: char| c.is_ascii_digit()),
                "variable {name} starts with a digit"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "variable {name} has characters WML does not allow"
            );
        }
    }

    #[test]
    fn get_and_post_forms_emit_their_method() {
        for (method, want) in [(FormMethod::Get, "get"), (FormMethod::Post, "post")] {
            let mut d = Deck::new();
            d.push(Block::Form(Form {
                action: "http://x/".into(),
                method,
                fields: vec![Field::Text {
                    name: "q".into(),
                    title: "Q".into(),
                    value: String::new(),
                    secret: false,
                }],
                submit_label: "Go".into(),
            }));
            let wml = to_wml(&d);
            let doc = parse(&wml);
            let go = doc
                .descendants()
                .find(|n| n.has_tag_name("go"))
                .expect("go");
            assert_eq!(go.attribute("method"), Some(want));
        }
    }
}
