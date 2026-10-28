//! Styled text: what renderers produce and the drawing layer puts on screen.
//! Kept small on purpose: a `Style` is 12 bytes and a plain span is a
//! `String` plus a style.

use unicode_width::UnicodeWidthStr;

/// A colour: the terminal's own default, 24-bit RGB, or a palette index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    #[default]
    Default,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

/// Text attributes as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const BOLD: Mods = Mods(1);
    pub const DIM: Mods = Mods(1 << 1);
    pub const ITALIC: Mods = Mods(1 << 2);
    pub const UNDERLINE: Mods = Mods(1 << 3);
    pub const REVERSE: Mods = Mods(1 << 4);
    pub const STRIKE: Mods = Mods(1 << 5);

    pub const fn contains(self, o: Mods) -> bool {
        self.0 & o.0 == o.0
    }
    pub const fn with(self, o: Mods) -> Mods {
        Mods(self.0 | o.0)
    }
}

/// How a span looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub mods: Mods,
}

impl Style {
    pub const fn fg(c: Color) -> Style {
        Style {
            fg: c,
            bg: Color::Default,
            mods: Mods::NONE,
        }
    }
    pub const fn on(self, bg: Color) -> Style {
        Style { bg, ..self }
    }
    pub const fn bold(self) -> Style {
        Style {
            mods: self.mods.with(Mods::BOLD),
            ..self
        }
    }
    pub const fn italic(self) -> Style {
        Style {
            mods: self.mods.with(Mods::ITALIC),
            ..self
        }
    }
    pub const fn underline(self) -> Style {
        Style {
            mods: self.mods.with(Mods::UNDERLINE),
            ..self
        }
    }
}

/// A run of text in one style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
        }
    }
    pub fn plain(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            style: Style::default(),
        }
    }
    /// Cells it takes on screen.
    pub fn width(&self) -> usize {
        width(&self.text)
    }
}

/// One screen line of spans. `bg` fills the rest of the row, when set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    pub bg: Color,
}

impl Line {
    pub fn new() -> Line {
        Line::default()
    }
    pub fn from_spans(spans: Vec<Span>) -> Line {
        Line {
            spans,
            bg: Color::Default,
        }
    }
    pub fn push(&mut self, text: impl Into<String>, style: Style) -> &mut Line {
        self.spans.push(Span::new(text, style));
        self
    }
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }
    /// The text without styles.
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// Cells `s` takes on screen.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}
