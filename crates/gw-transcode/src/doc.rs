//! The markup-neutral document model both gateways render.
//!
//! A deck is a title and a short list of blocks in reading order. It carries
//! only what a phone screen can show and a handset can act on: lines of text,
//! links, headings, and forms. Anything a backend cannot express degrades in
//! that backend rather than being dropped here.

/// Whether a character may appear in a deck.
///
/// XML 1.0 rejects the C0 controls other than tab, newline and carriage
/// return, and the two noncharacters at the end of the BMP. A WAP browser
/// refuses the whole document over one of them, and none of them mean anything
/// on a handset display, so page text carrying them must not reach a deck.
pub fn is_renderable(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r')
        || matches!(c, ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

/// Drop everything [`is_renderable`] rejects.
pub fn sanitize(s: &str) -> String {
    if s.chars().all(is_renderable) {
        return s.to_string();
    }
    s.chars().filter(|c| is_renderable(*c)).collect()
}

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

/// How a form's fields reach the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormMethod {
    Get,
    Post,
}

/// A single input within a form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// Free text bound to `name`. `secret` hides the entry as it is typed.
    Text {
        name: String,
        title: String,
        value: String,
        secret: bool,
    },
    /// Submitted verbatim and never shown.
    Hidden { name: String, value: String },
    /// One choice from `options`, each an `(value, label)` pair.
    Select {
        name: String,
        title: String,
        options: Vec<(String, String)>,
    },
}

impl Field {
    pub fn name(&self) -> &str {
        match self {
            Field::Text { name, .. } | Field::Hidden { name, .. } | Field::Select { name, .. } => {
                name
            }
        }
    }

    /// Whether the field is shown to the user, as opposed to submitted silently.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Field::Hidden { .. })
    }
}

/// A submittable form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Absolute URL the submission is sent to.
    pub action: String,
    pub method: FormMethod,
    pub fields: Vec<Field>,
    pub submit_label: String,
}

/// A block-level element in a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A line of inline content.
    Line(Vec<Inline>),
    /// A centered heading line.
    Heading(String),
    /// A blank line.
    Break,
    /// A form with at least one visible field.
    Form(Form),
}

/// A rendered deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deck {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    /// `PUBLIC=TRUE` in HDML: the deck is reachable from any other deck. The
    /// gateway serves decks from many origins and links freely between them, so
    /// without this the handset raises an access-control error on cross-origin
    /// navigation. WML has no equivalent and ignores it. Defaults to true.
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
