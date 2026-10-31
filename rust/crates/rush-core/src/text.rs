//! Styled text: what renderers produce and the drawing layer puts on screen.
//! Kept small on purpose: a `Style` is 16 bytes and a plain span is a
//! `String` plus a style. A run of text that links somewhere carries the
//! link in its style as a number ([`Link`]), so cutting, wrapping and
//! recolouring a row keeps its links where Go keeps its OSC 8 escapes.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

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

/// Where a run of text links to: a number for its URL, kept once for the
/// life of the process (links are few, and the same ones come again);
/// [`Link::NONE`] for none. Go draws a link as an OSC 8 escape around its
/// words (convo/render.go:1891 link); a row's links are read back by
/// cell (ui/linkmenu.go:31 linkAt) with [`link_at`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Link(pub u32);

/// Every URL a Link has been made for, by its number less one.
struct Links {
    urls: Vec<Arc<str>>,
    ids: HashMap<Arc<str>, u32>,
}

static LINKS: LazyLock<Mutex<Links>> = LazyLock::new(|| {
    Mutex::new(Links {
        urls: Vec::new(),
        ids: HashMap::new(),
    })
});

impl Link {
    pub const NONE: Link = Link(0);

    /// The link to url; NONE for an empty one.
    pub fn to(url: &str) -> Link {
        if url.is_empty() {
            return Link::NONE;
        }
        let mut l = LINKS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = l.ids.get(url) {
            return Link(id);
        }
        let u: Arc<str> = Arc::from(url);
        l.urls.push(u.clone());
        let id = l.urls.len() as u32;
        l.ids.insert(u, id);
        Link(id)
    }

    /// The URL it links to; None for NONE.
    pub fn url(self) -> Option<Arc<str>> {
        if self.0 == 0 {
            return None;
        }
        let l = LINKS.lock().unwrap_or_else(|e| e.into_inner());
        l.urls.get(self.0 as usize - 1).cloned()
    }

    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// How a span looks, and where it links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub mods: Mods,
    pub link: Link,
}

impl Style {
    /// No colour, no attributes, no link (a const Style::default()).
    pub const PLAIN: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        mods: Mods::NONE,
        link: Link::NONE,
    };

    pub const fn fg(c: Color) -> Style {
        Style {
            fg: c,
            bg: Color::Default,
            mods: Mods::NONE,
            link: Link::NONE,
        }
    }
    /// The same look, linking to url (Go's OSC 8 around the words).
    pub fn linked(self, url: &str) -> Style {
        Style {
            link: Link::to(url),
            ..self
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

/// The URL of the link drawn at cell col of l, or None where there's none
/// (linkmenu.go:31 linkAt).
pub fn link_at(l: &Line, col: usize) -> Option<Arc<str>> {
    let mut at = 0;
    for s in &l.spans {
        let w = s.width();
        if col >= at && col < at + w {
            return s.style.link.url();
        }
        at += w;
        if col < at {
            break;
        }
    }
    None
}

/// Cells `s` takes on screen.
pub fn width(s: &str) -> usize {
    // Printable ASCII is a cell a byte; most of what's drawn is.
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return s.len();
    }
    UnicodeWidthStr::width(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // linkmenu_test.go TestLinkAt: the link drawn at a cell, none before
    // or after it; the same URL is the same link.
    #[test]
    fn links_by_cell() {
        let u = "https://example.com/a";
        let l = Line::from_spans(vec![
            Span::plain("see "),
            Span::new("the docs", Style::fg(Color::Indexed(4)).linked(u)),
            Span::plain(" ok"),
        ]);
        for (col, want) in [
            (0, None),
            (3, None),
            (4, Some(u)),
            (11, Some(u)),
            (12, None),
            (40, None),
        ] {
            assert_eq!(link_at(&l, col).as_deref(), want, "col {col}");
        }
        assert_eq!(Link::to(u), Link::to(u));
        assert_ne!(Link::to(u), Link::to("file:///tmp/x"));
        assert!(Link::to("").is_none());
        assert_eq!(Style::default(), Style::PLAIN);
    }

    // The ASCII shortcut agrees with unicode-width.
    #[test]
    fn width_as_unicode_width_has_it() {
        for s in [
            "",
            " ",
            "hello, world ~",
            "a\tb",
            "\r\n",
            "\x1b[0m",
            "\x7f",
            "héllo",
            "日本",
            "✻ Go anywhere",
            "👍🏽",
        ] {
            assert_eq!(width(s), UnicodeWidthStr::width(s), "{s:?}");
        }
    }
}
