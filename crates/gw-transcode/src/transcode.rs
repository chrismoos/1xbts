//! HTML → deck transcoding.
//!
//! A handset renders a card deck, not HTML, so the gateway scans a fetched
//! document in reading order and emits a small deck: headings become their own
//! lines, block elements break lines, anchors become links with their targets
//! resolved to absolute URLs (so the follow-up request returns here with a
//! fetchable URL), forms become submittable field lists, and images collapse to
//! their alt text. The result is deliberately shallow — a phone screen shows a
//! few lines.

use std::collections::HashMap;

use scraper::Html;
use scraper::node::Node;
use url::Url;

use crate::doc::{Block, Deck, Field, Form, FormMethod, Inline, is_renderable, sanitize};

/// Caps on what a single page may produce. A large page must not yield a deck
/// too big for the handset or the datagram path, and the ceiling differs per
/// gateway: HDML and WML have different per-element overhead, and each protocol
/// carries its own transport limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_blocks: usize,
    pub max_fields: usize,
    pub max_select_options: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_blocks: 400,
            max_fields: 16,
            max_select_options: 32,
        }
    }
}

/// Transcode an HTML document to a deck. `base_url` is the absolute URL the
/// document was fetched from, used to resolve relative links and form actions.
pub fn html_to_deck(html: &str, base_url: &str, limits: &Limits) -> Deck {
    let doc = Html::parse_document(html);
    let base = Url::parse(base_url).ok();
    let mut ctx = Walker {
        base,
        deck: Deck::new(),
        line: Vec::new(),
        limits: *limits,
    };
    walk(doc.tree.root(), &mut ctx);
    ctx.flush_line();
    if ctx.deck.title.is_none() {
        ctx.deck.title = Some("Page".to_string());
    }
    ctx.deck
}

/// Wrap arbitrary text (e.g. a `text/plain` body) in a minimal deck.
pub fn text_to_deck(text: &str, title: &str, limits: &Limits) -> Deck {
    let mut deck = Deck::new();
    deck.title = Some(title.to_string());
    for raw_line in text.lines().take(limits.max_blocks) {
        let line = sanitize(raw_line.trim_end());
        let line = line.as_str();
        if line.is_empty() {
            deck.push(Block::Break);
        } else {
            deck.push(Block::Line(vec![Inline::Text(line.to_string())]));
        }
    }
    deck
}

struct Walker {
    base: Option<Url>,
    deck: Deck,
    line: Vec<Inline>,
    limits: Limits,
}

impl Walker {
    fn push_text(&mut self, text: &str) {
        // Preserve boundary whitespace as a single separating space so text
        // running up to an element (e.g. an anchor) does not fuse with it.
        let lead = text.starts_with(char::is_whitespace);
        let trail = text.ends_with(char::is_whitespace);
        let core = collapse_ws(text);
        if core.is_empty() {
            if lead || trail {
                self.ensure_trailing_space();
            }
            return;
        }
        if lead {
            self.ensure_trailing_space();
        }
        match self.line.last_mut() {
            Some(Inline::Text(prev)) => prev.push_str(&core),
            _ => self.line.push(Inline::Text(core)),
        }
        if trail {
            self.ensure_trailing_space();
        }
    }

    /// Guarantee the current line ends with a separating space, without starting
    /// a line with one.
    fn ensure_trailing_space(&mut self) {
        match self.line.last_mut() {
            Some(Inline::Text(t)) => {
                if !t.ends_with(' ') {
                    t.push(' ');
                }
            }
            Some(Inline::Link { .. }) => self.line.push(Inline::Text(" ".to_string())),
            None => {}
        }
    }

    fn push_link(&mut self, label: String, href: &str) {
        let label = collapse_ws(&label);
        let dest = resolve(&self.base, href);
        match dest {
            Some(dest) if !label.is_empty() => self.line.push(Inline::Link { label, dest }),
            // Unresolvable or empty-label link degrades to its text.
            _ if !label.is_empty() => self.line.push(Inline::Text(label)),
            _ => {}
        }
    }

    fn flush_line(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let line = std::mem::take(&mut self.line);
        if self.deck.blocks.len() < self.limits.max_blocks {
            self.deck.push(Block::Line(line));
        }
    }

    fn heading(&mut self, text: String) {
        self.flush_line();
        let text = collapse_ws(&text);
        if !text.is_empty() && self.deck.blocks.len() < self.limits.max_blocks {
            self.deck.push(Block::Heading(text));
        }
    }

    fn push_form(&mut self, form: Form) {
        if self.deck.blocks.len() < self.limits.max_blocks {
            self.deck.push(Block::Form(form));
        }
    }
}

fn resolve(base: &Option<Url>, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
        return None;
    }
    match base {
        Some(base) => base.join(href).ok().map(|u| u.to_string()),
        None => Url::parse(href).ok().map(|u| u.to_string()),
    }
}

fn walk(node: ego_tree::NodeRef<'_, Node>, ctx: &mut Walker) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => ctx.push_text(t),
            Node::Element(el) => {
                let name = el.name();
                match name {
                    "script" | "style" | "noscript" | "svg" | "template" => {}
                    // Control and label text belongs to the form block, not
                    // the prose.
                    "input" | "select" | "option" | "textarea" | "button" | "label" => {}
                    "title" => {
                        if ctx.deck.title.is_none() {
                            let t = collapse_ws(&collect_text(child));
                            if !t.is_empty() {
                                ctx.deck.title = Some(t);
                            }
                        }
                    }
                    "br" => ctx.flush_line(),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        ctx.heading(collect_text(child));
                    }
                    "a" => {
                        let label = collect_text(child);
                        match el.attr("href") {
                            Some(href) => ctx.push_link(label, href),
                            None => ctx.push_text(&label),
                        }
                    }
                    "img" => {
                        if let Some(alt) = el.attr("alt")
                            && !alt.trim().is_empty()
                        {
                            ctx.push_text(&format!("[{}]", alt.trim()));
                        }
                    }
                    "form" => {
                        ctx.flush_line();
                        walk(child, ctx);
                        ctx.flush_line();
                        if let Some(form) = collect_form(child, el, &ctx.base, &ctx.limits) {
                            ctx.push_form(form);
                        }
                    }
                    "p" | "div" | "li" | "tr" | "ul" | "ol" | "table" | "section" | "article"
                    | "header" | "footer" | "nav" | "blockquote" | "dd" | "dt" => {
                        ctx.flush_line();
                        walk(child, ctx);
                        ctx.flush_line();
                    }
                    _ => walk(child, ctx),
                }
            }
            _ => {}
        }
    }
}

/// Gather a form's action, method and inputs. Returns `None` unless the action
/// resolves and at least one field is shown, since a form the handset cannot
/// fill in is better rendered as nothing.
fn collect_form(
    node: ego_tree::NodeRef<'_, Node>,
    el: &scraper::node::Element,
    base: &Option<Url>,
    limits: &Limits,
) -> Option<Form> {
    let action = match el.attr("action") {
        Some(a) => resolve(base, a)?,
        // An action-less form submits back to the page it came from.
        None => base.as_ref().map(std::string::ToString::to_string)?,
    };
    let method = match el.attr("method") {
        Some(m) if m.eq_ignore_ascii_case("post") => FormMethod::Post,
        _ => FormMethod::Get,
    };

    let labels = collect_labels(node);
    let mut fields: Vec<Field> = Vec::new();
    let mut submit_label = String::new();

    for d in node.descendants() {
        let Node::Element(e) = d.value() else {
            continue;
        };
        // The field cap stops collecting inputs, not scanning: the submit
        // control usually comes last and its label is still wanted.
        let room = fields.len() < limits.max_fields;
        match e.name() {
            "input" => {
                let ty = e.attr("type").unwrap_or("text").to_ascii_lowercase();
                let name = e.attr("name").unwrap_or("").to_string();
                let value = sanitize(e.attr("value").unwrap_or(""));
                match ty.as_str() {
                    "submit" | "image" => {
                        if submit_label.is_empty() && !value.is_empty() {
                            submit_label = collapse_ws(&value);
                        }
                    }
                    "hidden" => {
                        if room && !name.is_empty() {
                            fields.push(Field::Hidden { name, value });
                        }
                    }
                    // Checkbox and radio need grouped state the deck model does
                    // not carry, so they are left out rather than mis-submitted.
                    "checkbox" | "radio" | "button" | "reset" | "file" => {}
                    _ => {
                        if room && !name.is_empty() {
                            fields.push(Field::Text {
                                title: field_title(e, &name, &labels),
                                name,
                                value,
                                secret: ty == "password",
                            });
                        }
                    }
                }
            }
            "textarea" => {
                let name = e.attr("name").unwrap_or("").to_string();
                if room && !name.is_empty() {
                    fields.push(Field::Text {
                        title: field_title(e, &name, &labels),
                        name,
                        value: collapse_ws(&collect_text(d)),
                        secret: false,
                    });
                }
            }
            "select" => {
                let name = e.attr("name").unwrap_or("").to_string();
                if !room || name.is_empty() {
                    continue;
                }
                let mut options = Vec::new();
                for o in d.descendants() {
                    if options.len() >= limits.max_select_options {
                        break;
                    }
                    let Node::Element(oe) = o.value() else {
                        continue;
                    };
                    if oe.name() != "option" {
                        continue;
                    }
                    let label = collapse_ws(&collect_text(o));
                    let value = sanitize(oe.attr("value").unwrap_or(&label));
                    if !label.is_empty() {
                        options.push((value, label));
                    }
                }
                if !options.is_empty() {
                    fields.push(Field::Select {
                        title: field_title(e, &name, &labels),
                        name,
                        options,
                    });
                }
            }
            "button" => {
                let ty = e.attr("type").unwrap_or("submit").to_ascii_lowercase();
                if ty == "submit" && submit_label.is_empty() {
                    let label = collapse_ws(&collect_text(d));
                    if !label.is_empty() {
                        submit_label = label;
                    }
                }
            }
            _ => {}
        }
    }

    if !fields.iter().any(Field::is_visible) {
        return None;
    }
    if submit_label.is_empty() {
        submit_label = "Submit".to_string();
    }
    Some(Form {
        action,
        method,
        fields,
        submit_label,
    })
}

/// Label text for a form's controls, keyed by control `id` (from
/// `<label for>`) and by control name (from a `<label>` that wraps the
/// control).
#[derive(Default)]
struct Labels {
    by_id: HashMap<String, String>,
    by_name: HashMap<String, String>,
}

fn collect_labels(form: ego_tree::NodeRef<'_, Node>) -> Labels {
    let mut labels = Labels::default();
    for d in form.descendants() {
        let Node::Element(e) = d.value() else {
            continue;
        };
        if e.name() != "label" {
            continue;
        }
        let mut text = String::new();
        label_text(d, &mut text);
        let text = collapse_ws(&text);
        if text.is_empty() {
            continue;
        }
        if let Some(id) = e.attr("for") {
            labels.by_id.insert(id.to_string(), text);
        } else if let Some(name) = d.descendants().find_map(|c| match c.value() {
            Node::Element(ce) if matches!(ce.name(), "input" | "select" | "textarea") => {
                ce.attr("name").filter(|n| !n.is_empty())
            }
            _ => None,
        }) {
            labels.by_name.insert(name.to_string(), text);
        }
    }
    labels
}

/// A label's own words: its text minus anything inside a control it wraps.
fn label_text(node: ego_tree::NodeRef<'_, Node>, out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e)
                if matches!(
                    e.name(),
                    "select" | "textarea" | "button" | "script" | "style"
                ) => {}
            Node::Element(_) => label_text(child, out),
            _ => {}
        }
    }
}

/// A human label for a control: its `<label>`, else its own descriptive
/// attributes, else its form name.
fn field_title(el: &scraper::node::Element, name: &str, labels: &Labels) -> String {
    if let Some(id) = el.attr("id")
        && let Some(t) = labels.by_id.get(id)
    {
        return t.clone();
    }
    if let Some(t) = labels.by_name.get(name) {
        return t.clone();
    }
    for attr in ["title", "placeholder", "aria-label"] {
        if let Some(v) = el.attr(attr) {
            let v = collapse_ws(v);
            if !v.is_empty() {
                return v;
            }
        }
    }
    name.to_string()
}

/// Concatenate all descendant text of a node.
fn collect_text(node: ego_tree::NodeRef<'_, Node>) -> String {
    let mut out = String::new();
    for d in node.descendants() {
        if let Node::Text(t) = d.value() {
            out.push_str(t);
        }
    }
    out
}

/// Collapse runs of ASCII whitespace to a single space and trim the ends.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if !is_renderable(c) {
            continue;
        }
        if c.is_whitespace() {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = false;
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck_of(html: &str, base: &str) -> Deck {
        html_to_deck(html, base, &Limits::default())
    }

    #[test]
    fn extracts_title_headings_text_and_links() {
        let html = r#"
            <html><head><title>Hello World</title></head>
            <body>
              <h1>Welcome</h1>
              <p>Some intro text with a <a href="/page2">link</a> inside.</p>
              <p>Second paragraph.</p>
              <script>ignored()</script>
            </body></html>
        "#;
        let deck = deck_of(html, "http://example.com/dir/index.html");
        assert_eq!(deck.title.as_deref(), Some("Hello World"));
        assert!(
            deck.blocks
                .iter()
                .any(|b| matches!(b, Block::Heading(h) if h == "Welcome"))
        );
        assert!(deck.blocks.iter().any(|b| matches!(
            b,
            Block::Line(l) if l.iter().any(|i| matches!(
                i, Inline::Link { dest, .. } if dest == "http://example.com/page2"
            ))
        )));
    }

    #[test]
    fn drops_fragment_and_js_links_to_text() {
        let html = r##"<a href="#top">Top</a> <a href="javascript:void(0)">JS</a>"##;
        let deck = deck_of(html, "http://x/");
        assert!(!deck.blocks.iter().any(|b| matches!(
            b, Block::Line(l) if l.iter().any(|i| matches!(i, Inline::Link { .. }))
        )));
    }

    #[test]
    fn max_blocks_is_configurable() {
        let html = "<p>a</p><p>b</p><p>c</p><p>d</p>";
        let limits = Limits {
            max_blocks: 2,
            ..Limits::default()
        };
        let deck = html_to_deck(html, "http://x/", &limits);
        assert_eq!(deck.blocks.len(), 2);
    }

    #[test]
    fn collects_a_post_form_with_text_hidden_and_select() {
        let html = r#"
            <form action="/search" method="POST">
              <input type="text" name="q" title="Query">
              <input type="hidden" name="src" value="wap">
              <select name="lang"><option value="en">English</option><option>Suomi</option></select>
              <input type="submit" value="Go">
            </form>
        "#;
        let deck = deck_of(html, "http://example.com/");
        let form = deck
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Form(f) => Some(f),
                _ => None,
            })
            .expect("form block");
        assert_eq!(form.action, "http://example.com/search");
        assert_eq!(form.method, FormMethod::Post);
        assert_eq!(form.submit_label, "Go");
        assert_eq!(form.fields.len(), 3);
        assert!(matches!(&form.fields[0], Field::Text { title, .. } if title == "Query"));
        assert!(matches!(&form.fields[1], Field::Hidden { value, .. } if value == "wap"));
        match &form.fields[2] {
            Field::Select { options, .. } => {
                // An option without a value attribute submits its own label.
                assert_eq!(options[0], ("en".into(), "English".into()));
                assert_eq!(options[1], ("Suomi".into(), "Suomi".into()));
            }
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn form_method_defaults_to_get() {
        let html = r#"<form action="/s"><input name="q"></form>"#;
        let deck = deck_of(html, "http://example.com/");
        let Some(Block::Form(f)) = deck.blocks.iter().find(|b| matches!(b, Block::Form(_))) else {
            panic!("form block");
        };
        assert_eq!(f.method, FormMethod::Get);
        assert_eq!(f.submit_label, "Submit");
    }

    #[test]
    fn form_action_resolves_like_a_link() {
        for (action, want) in [
            ("../up?a=1", "http://example.com/up?a=1"),
            ("sibling", "http://example.com/dir/sibling"),
            ("//other/x", "http://other/x"),
            ("http://abs/y", "http://abs/y"),
        ] {
            let html = format!(r#"<form action="{action}"><input name="q"></form>"#);
            let deck = deck_of(&html, "http://example.com/dir/page.html");
            let Some(Block::Form(f)) = deck.blocks.iter().find(|b| matches!(b, Block::Form(_)))
            else {
                panic!("form block for {action}");
            };
            assert_eq!(f.action, want, "resolving {action}");
        }
    }

    #[test]
    fn form_without_an_action_submits_back_to_the_page() {
        let html = r#"<form method="post"><input name="q"></form>"#;
        let deck = deck_of(html, "http://example.com/dir/page.html");
        let Some(Block::Form(f)) = deck.blocks.iter().find(|b| matches!(b, Block::Form(_))) else {
            panic!("form block");
        };
        assert_eq!(f.action, "http://example.com/dir/page.html");
    }

    #[test]
    fn field_and_option_counts_are_capped() {
        let inputs: String = (0..40).map(|i| format!("<input name=\"f{i}\">")).collect();
        let options: String = (0..40).map(|i| format!("<option>o{i}</option>")).collect();
        let html =
            format!(r#"<form action="/s">{inputs}<select name="s">{options}</select></form>"#);
        let limits = Limits {
            max_fields: 5,
            max_select_options: 3,
            ..Limits::default()
        };
        let deck = html_to_deck(&html, "http://example.com/", &limits);
        let Some(Block::Form(f)) = deck.blocks.iter().find(|b| matches!(b, Block::Form(_))) else {
            panic!("form block");
        };
        assert!(f.fields.len() <= 5, "got {} fields", f.fields.len());
        for field in &f.fields {
            if let Field::Select { options, .. } = field {
                assert!(options.len() <= 3, "got {} options", options.len());
            }
        }
    }

    #[test]
    fn labels_title_their_controls_and_leave_the_prose() {
        let html = r#"
            <form action="/s">
              <label for="q">Search terms</label> <input id="q" name="q">
              <label>Language <select name="lang"><option>en</option></select></label>
              <input name="bare" placeholder="Bare">
            </form>
        "#;
        let deck = deck_of(html, "http://example.com/");
        let Some(Block::Form(f)) = deck.blocks.iter().find(|b| matches!(b, Block::Form(_))) else {
            panic!("form block");
        };
        let titles: Vec<&str> = f
            .fields
            .iter()
            .map(|fld| match fld {
                Field::Text { title, .. } | Field::Select { title, .. } => title.as_str(),
                Field::Hidden { .. } => "",
            })
            .collect();
        assert_eq!(titles, ["Search terms", "Language", "Bare"]);
        // The label text is carried by the field, not repeated as prose.
        assert!(!deck.blocks.iter().any(|b| matches!(
            b, Block::Line(l) if l.iter().any(|i| matches!(i, Inline::Text(t) if t.contains("Search terms")))
        )));
    }

    #[test]
    fn submit_label_is_still_found_past_the_field_cap() {
        let inputs: String = (0..10).map(|i| format!("<input name=\"f{i}\">")).collect();
        let html =
            format!(r#"<form action="/s">{inputs}<input type="submit" value="Send it"></form>"#);
        let limits = Limits {
            max_fields: 3,
            ..Limits::default()
        };
        let deck = html_to_deck(&html, "http://example.com/", &limits);
        let Some(Block::Form(f)) = deck.blocks.iter().find(|b| matches!(b, Block::Form(_))) else {
            panic!("form block");
        };
        assert_eq!(f.fields.len(), 3);
        assert_eq!(f.submit_label, "Send it");
    }

    #[test]
    fn non_ascii_text_is_preserved() {
        let raw = "Suomi — ÅÄÖ — 日本語";
        let html = format!("<p>{raw}</p>");
        let deck = deck_of(&html, "http://x/");
        assert!(deck.blocks.iter().any(|b| matches!(
            b, Block::Line(l) if l.iter().any(|i| matches!(i, Inline::Text(t) if t == raw))
        )));
    }

    #[test]
    fn html_entities_are_decoded_once() {
        let deck = deck_of("<p>a &amp; b &lt;c&gt; &quot;d&quot;</p>", "http://x/");
        assert!(deck.blocks.iter().any(|b| matches!(
            b, Block::Line(l) if l.iter().any(|i| matches!(i, Inline::Text(t) if t == r#"a & b <c> "d""#))
        )));
    }

    #[test]
    fn form_without_visible_fields_is_dropped() {
        let html = r#"<form action="/x"><input type="hidden" name="a" value="1"></form>"#;
        let deck = deck_of(html, "http://example.com/");
        assert!(!deck.blocks.iter().any(|b| matches!(b, Block::Form(_))));
    }

    #[test]
    fn control_text_does_not_leak_into_prose() {
        let html = r#"<form action="/x"><select name="s"><option>Choice</option></select></form>"#;
        let deck = deck_of(html, "http://example.com/");
        assert!(!deck.blocks.iter().any(|b| matches!(
            b, Block::Line(l) if l.iter().any(|i| matches!(i, Inline::Text(t) if t.contains("Choice")))
        )));
    }

    #[test]
    fn plain_text_wraps() {
        let deck = text_to_deck("line one\n\nline two", "notes", &Limits::default());
        assert_eq!(deck.title.as_deref(), Some("notes"));
        assert_eq!(deck.blocks.len(), 3);
    }
}
