//! A page must never yield a deck the handset rejects.
//!
//! A WAP browser refuses a whole document over one character XML does not
//! allow, reporting only that it contains invalid terms, so the transcoder has
//! to drop those before they reach a deck. Every case here is content a real
//! page can carry.

use gw_transcode::transcode::{Limits, html_to_deck};
use wap_gw::wml::to_wml;

/// Transcode a page and return its deck, failing if the result is not a
/// well-formed XML document.
fn wml_of(html: &str) -> String {
    let deck = html_to_deck(html, "http://example.com/", &Limits::default());
    let wml = to_wml(&deck);
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    if let Err(e) = roxmltree::Document::parse_with_options(&wml, opts) {
        panic!("page produced a document a browser would reject: {e}\n---\n{wml}");
    }
    wml
}

#[test]
fn control_characters_in_text_are_dropped() {
    let wml = wml_of("<p>before\u{0001}after</p>");
    assert!(wml.contains("beforeafter"), "{wml}");
}

#[test]
fn a_numeric_reference_to_a_control_character_is_dropped() {
    // The HTML parser resolves the reference, so the raw character is what
    // reaches the deck.
    wml_of("<p>a&#1;b</p>");
}

#[test]
fn bmp_noncharacters_are_dropped() {
    wml_of("<p>a\u{FFFE}b\u{FFFF}c</p>");
}

#[test]
fn control_characters_in_form_values_are_dropped() {
    wml_of(concat!(
        "<form action=\"/s\">",
        "<input name=\"q\" value=\"a\u{0002}b\">",
        "<input type=\"hidden\" name=\"h\" value=\"c\u{0003}d\">",
        "<select name=\"s\"><option value=\"e\u{0004}f\">g\u{0005}h</option></select>",
        "</form>"
    ));
}

#[test]
fn control_characters_in_a_plain_text_body_are_dropped() {
    let deck = gw_transcode::transcode::text_to_deck(
        "line one\u{0001}\nline\u{000e} two",
        "notes",
        &Limits::default(),
    );
    let wml = to_wml(&deck);
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    assert!(
        roxmltree::Document::parse_with_options(&wml, opts).is_ok(),
        "{wml}"
    );
}

#[test]
fn tab_newline_and_carriage_return_still_count_as_whitespace() {
    let wml = wml_of("<p>a\tb\nc\rd</p>");
    assert!(wml.contains("a b c d"), "{wml}");
}

#[test]
fn awkward_field_names_and_labels_still_produce_a_valid_deck() {
    wml_of(r#"<form action="/s"><input name="!!!"><input name="1a"><input name="日本"></form>"#);
    wml_of(r#"<form action="/s"><select name="s"><option value="v"></option></select></form>"#);
    wml_of("<html><head><title></title></head><body><p>x</p></body></html>");
}

/// Every link the transcoder emits must be a URL an HTTP request line can
/// carry. A space in the request target is rejected by the server's parser
/// before any handler runs, so the handset would get a bare 400 with no deck.
#[test]
fn emitted_links_are_never_malformed() {
    let html = r#"
        <a href="/a b/c">space in path</a>
        <a href="  http://spaced.example/  ">padded</a>
        <a href="/wap//">double slash</a>
        <a href="../up">relative</a>
        <a href="?q=a b">query with space</a>
        <a href="http://ex.example/x?a=1&amp;b=2">entity amp</a>
        <form action="/s b"><input name="q"></form>
    "#;
    let deck = html_to_deck(html, "http://gtrxac.fi/wap/", &Limits::default());
    let mut checked = 0;
    for block in &deck.blocks {
        let dests: Vec<&str> = match block {
            gw_transcode::doc::Block::Line(inlines) => inlines
                .iter()
                .filter_map(|i| match i {
                    gw_transcode::doc::Inline::Link { dest, .. } => Some(dest.as_str()),
                    gw_transcode::doc::Inline::Text(_) => None,
                })
                .collect(),
            gw_transcode::doc::Block::Form(f) => vec![f.action.as_str()],
            _ => vec![],
        };
        for d in dests {
            checked += 1;
            assert!(!d.contains(' '), "emitted a URL containing a space: {d}");
            assert!(
                d.parse::<http::Uri>().is_ok(),
                "emitted a URL a request line cannot carry: {d}"
            );
        }
    }
    assert!(checked >= 5, "expected several links, checked {checked}");
}

/// A site that already serves WML must reach the handset as it was written.
/// Transcoding it escapes the markup, so the browser renders the source
/// instead of the page.
#[test]
fn native_wap_content_is_recognised() {
    for ctype in [
        "text/vnd.wap.wml",
        "text/vnd.wap.wml; charset=utf-8",
        "TEXT/VND.WAP.WML",
        "application/vnd.wap.wmlc",
        "image/vnd.wap.wbmp",
        "text/vnd.wap.wmlscript",
    ] {
        assert!(
            wap_gw::is_handset_native(ctype),
            "{ctype} should go through untouched"
        );
    }
    for ctype in ["text/html", "text/plain", "application/json", ""] {
        assert!(
            !wap_gw::is_handset_native(ctype),
            "{ctype} needs transcoding"
        );
    }
}
