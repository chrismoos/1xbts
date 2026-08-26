//! HTML → HDML transcoding.
//!
//! UP.Browser renders HDML card decks, not HTML, so the gateway walks a fetched
//! HTML document in reading order and emits a single `<DISPLAY>` card: headings
//! become centered lines, block elements break lines, anchors become HDML `<A>`
//! links with their targets resolved to absolute URLs (so the follow-up Get
//! returns here with a fetchable URL), and images collapse to their alt text.
//! The result is deliberately shallow — a phone screen shows a few lines.

use scraper::Html;
use scraper::node::Node;
use url::Url;

use crate::hdml::{Block, Deck, Inline};

/// Upper bound on emitted blocks so a large page cannot produce a deck too big
/// for the handset or the datagram path.
const MAX_BLOCKS: usize = 400;

/// Transcode an HTML document to an HDML deck. `base_url` is the absolute URL
/// the document was fetched from, used to resolve relative links.
pub fn html_to_hdml(html: &str, base_url: &str) -> Deck {
    let doc = Html::parse_document(html);
    let base = Url::parse(base_url).ok();
    let mut ctx = Walker {
        base,
        deck: Deck::new(),
        line: Vec::new(),
    };
    walk(doc.tree.root(), &mut ctx);
    ctx.flush_line();
    if ctx.deck.title.is_none() {
        ctx.deck.title = Some("Page".to_string());
    }
    ctx.deck
}

/// Wrap arbitrary text (e.g. a `text/plain` body) in a minimal deck.
pub fn text_to_hdml(text: &str, title: &str) -> Deck {
    let mut deck = Deck::new();
    deck.title = Some(title.to_string());
    for raw_line in text.lines().take(MAX_BLOCKS) {
        let line = raw_line.trim_end();
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
        let dest = self.resolve(href);
        match dest {
            Some(dest) if !label.is_empty() => self.line.push(Inline::Link { label, dest }),
            // Unresolvable or empty-label link degrades to its text.
            _ if !label.is_empty() => self.line.push(Inline::Text(label)),
            _ => {}
        }
    }

    fn resolve(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
            return None;
        }
        match &self.base {
            Some(base) => base.join(href).ok().map(|u| u.to_string()),
            None => Url::parse(href).ok().map(|u| u.to_string()),
        }
    }

    fn flush_line(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let line = std::mem::take(&mut self.line);
        if self.deck.blocks.len() < MAX_BLOCKS {
            self.deck.push(Block::Line(line));
        }
    }

    fn heading(&mut self, text: String) {
        self.flush_line();
        let text = collapse_ws(&text);
        if !text.is_empty() && self.deck.blocks.len() < MAX_BLOCKS {
            self.deck.push(Block::Heading(text));
        }
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
                    "p" | "div" | "li" | "tr" | "ul" | "ol" | "table" | "section" | "article"
                    | "header" | "footer" | "nav" | "blockquote" | "form" | "dd" | "dt" => {
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
        let deck = html_to_hdml(html, "http://example.com/dir/index.html");
        assert_eq!(deck.title.as_deref(), Some("Hello World"));
        // Heading present.
        assert!(
            deck.blocks
                .iter()
                .any(|b| matches!(b, Block::Heading(h) if h == "Welcome"))
        );
        // Link resolved against the base URL.
        let rendered = deck.to_hdml();
        assert!(rendered.contains("DEST=\"http://example.com/page2\""));
        assert!(rendered.contains("Second paragraph."));
        assert!(!rendered.contains("ignored"));
        // Boundary whitespace around the anchor is preserved as a separator.
        assert!(rendered.contains("with a <A TASK=GO"));
        assert!(rendered.contains("</A> inside."));
    }

    #[test]
    fn drops_fragment_and_js_links_to_text() {
        let html = r##"<a href="#top">Top</a> <a href="javascript:void(0)">JS</a>"##;
        let deck = html_to_hdml(html, "http://x/");
        let out = deck.to_hdml();
        assert!(out.contains("Top"));
        assert!(!out.contains("TASK=GO"));
    }

    #[test]
    fn plain_text_wraps() {
        let deck = text_to_hdml("line one\n\nline two", "notes");
        assert_eq!(deck.title.as_deref(), Some("notes"));
        let out = deck.to_hdml();
        assert!(out.contains("line one"));
        assert!(out.contains("line two"));
    }
}
