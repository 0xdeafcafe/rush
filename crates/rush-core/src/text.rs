//! Styled text: what renderers produce and the drawing layer puts on screen.
//! Kept small on purpose: a `Style` is 16 bytes and a plain span is a
//! `String` plus a style. A run of text that links somewhere carries the
//! link in its style as a number ([`Link`]), so cutting, wrapping and
//! recolouring a row keeps its links where Go keeps its OSC 8 escapes.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

pub use compact_str::{CompactString, format_compact};
use unicode_segmentation::{GraphemeIndices, UnicodeSegmentation};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
/// cell (ui/linkmenu.go:31 linkAt) with [`link_at`]. A quiet link
/// ([`Link::quiet`]) is rush's alone: read back for a click like any
/// other, but never written to the terminal as OSC 8, so the terminal
/// neither underlines it nor offers its own menu for it (a picture).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Link(pub u32);

/// The bit a quiet link's number carries.
const QUIET: u32 = 1 << 31;

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

    /// The link to url that only rush follows: clicked, it's read back
    /// as any link is, but it's never written as OSC 8 (see [`Link`]).
    pub fn quiet(url: &str) -> Link {
        match Link::to(url) {
            Link::NONE => Link::NONE,
            Link(id) => Link(id | QUIET),
        }
    }

    /// Whether it's rush's alone, never written to the terminal.
    pub const fn is_quiet(self) -> bool {
        self.0 & QUIET != 0
    }

    /// The URL it links to; None for NONE.
    pub fn url(self) -> Option<Arc<str>> {
        let id = self.0 & !QUIET;
        if id == 0 {
            return None;
        }
        let l = LINKS.lock().unwrap_or_else(|e| e.into_inner());
        l.urls.get(id as usize - 1).cloned()
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
    /// Its text: up to 24 bytes held in place, so most of what a frame
    /// draws (a glyph, a word, a time) is made and copied without the
    /// heap (Rust-only).
    pub text: CompactString,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<CompactString>, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
        }
    }
    pub fn plain(text: impl Into<CompactString>) -> Span {
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
    pub fn push(&mut self, text: impl Into<CompactString>, style: Style) -> &mut Line {
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

/// Cells one grapheme takes, as the screen draws it (rush-draw's canvas):
/// a control takes none; "⚠️" and "👍🏽" take two, the whole grapheme
/// measured, never its chars one by one. Layout that cuts or wraps text
/// counts with this so that what it measures is what is drawn.
pub fn grapheme_width(g: &str) -> usize {
    match g.as_bytes() {
        [b] if (0x20..0x7f).contains(b) => 1,
        _ if g.chars().any(char::is_control) => 0,
        _ => UnicodeWidthStr::width(g),
    }
}

/// Whether c is a grapheme of its own wherever it stands: it joins neither
/// the char before it (marks, joiners) nor the one after (prepended
/// signs), nor pairs (flags, Hangul, CR LF). These ranges hold none of
/// those: printable ASCII; Latin-1, Latin Extended and the spacing
/// modifiers (up to the combining marks at U+0300); general punctuation
/// but the zero-width and direction marks; super- and subscripts and
/// currency (up to the combining marks at U+20D0); letterlike symbols
/// through arrows, maths, technical symbols, box drawing, blocks, shapes,
/// dingbats and more arrows. Checked against the segmenter in
/// rush-draw's tests.
pub fn lone(c: char) -> bool {
    matches!(
        c as u32,
        0x20..=0x7e | 0xa0..=0x2ff | 0x2010..=0x2027 | 0x2030..=0x205e | 0x2070..=0x20cf | 0x2100..=0x2bff
    )
}

/// The graphemes of s with where each starts and the cells it takes
/// ([`grapheme_width`]); text of lone chars is gone through a char at a
/// time, with no segmenting.
pub fn graphemes(s: &str) -> Graphemes<'_> {
    if s.chars().all(lone) {
        Graphemes::Lone(s, s.char_indices())
    } else {
        Graphemes::Segmented(s.grapheme_indices(true))
    }
}

/// What [`graphemes`] goes through: (byte offset, grapheme, cells).
pub enum Graphemes<'a> {
    Lone(&'a str, std::str::CharIndices<'a>),
    Segmented(GraphemeIndices<'a>),
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = (usize, &'a str, usize);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Graphemes::Lone(s, cs) => {
                let (i, c) = cs.next()?;
                let w = UnicodeWidthChar::width(c).unwrap_or(0);
                Some((i, &s[i..i + c.len_utf8()], w))
            }
            Graphemes::Segmented(gs) => {
                let (i, g) = gs.next()?;
                Some((i, g, grapheme_width(g)))
            }
        }
    }
}

/// Cells s takes counted a grapheme at a time, as it's drawn: unlike
/// [`width`], a control takes none.
pub fn graphemes_width(s: &str) -> usize {
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return s.len();
    }
    graphemes(s).map(|(_, _, w)| w).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quiet link reads back as its URL, as a click does, and is told
    /// apart from the same URL's loud one.
    #[test]
    fn quiet_links_read_back() {
        let url = "rush:message/t1/image/1";
        let (loud, quiet) = (Link::to(url), Link::quiet(url));
        assert_ne!(loud, quiet);
        assert!(quiet.is_quiet() && !loud.is_quiet());
        assert_eq!(quiet.url(), loud.url());
        assert_eq!(Link::quiet(""), Link::NONE);
        let l = Line::from_spans(vec![
            Span::new("ab", Style::PLAIN),
            Span::new(
                "cd",
                Style {
                    link: quiet,
                    ..Style::PLAIN
                },
            ),
        ]);
        assert_eq!(link_at(&l, 2).as_deref(), Some(url));
        assert_eq!(link_at(&l, 1), None);
    }

    /// Drawn rows are kept by the thousand (a conversation's renderer keeps
    /// up to 20,000): (type, its size, the most it may be).
    #[test]
    fn spans_stay_small() {
        let cases = [
            ("Style", size_of::<Style>(), 16),
            ("Span", size_of::<Span>(), 40),
            ("Line", size_of::<Line>(), 32),
            ("Color", size_of::<Color>(), 4),
        ];
        for (name, size, most) in cases {
            assert!(size <= most, "{name} is {size} B, more than {most}");
        }
    }

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

    // A grapheme's cells are the whole grapheme's, as the screen draws
    // it, never the sum of its chars'.
    #[test]
    fn graphemes_take_their_whole_width() {
        for (s, want) in [
            ("", 0),
            ("abc", 3),
            ("héllo", 5),
            ("e\u{301}", 1),
            ("日本", 4),
            ("⚠", 1),
            ("⚠\u{fe0f}", 2),
            ("👍", 2),
            ("👍🏽", 2),
            ("👨\u{200d}👩\u{200d}👧", 2),
            ("🇬🇧", 2),
            ("a\tb", 2),
            ("→ ok", 4),
            ("ok ⚠\u{fe0f} 👍🏽!", 9),
        ] {
            assert_eq!(graphemes_width(s), want, "{s:?}");
            let gs: Vec<(usize, &str, usize)> = graphemes(s).collect();
            assert_eq!(gs.iter().map(|g| g.2).sum::<usize>(), want, "{s:?}");
            let back: String = gs.iter().map(|g| g.1).collect();
            assert_eq!(back, s, "{s:?}: the graphemes are s");
        }
        for (g, want) in [
            ("a", 1),
            ("\t", 0),
            ("\r\n", 0),
            ("⚠\u{fe0f}", 2),
            ("👍🏽", 2),
        ] {
            assert_eq!(grapheme_width(g), want, "{g:?}");
        }
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
