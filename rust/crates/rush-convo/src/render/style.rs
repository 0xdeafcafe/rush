//! rush's conversation colours and the words that go with them. Port of
//! internal/convo/style.go (palette on the dark ground, spines, spinner,
//! musings, dur, money, folded) and highlight.go:15-30 (syntax colours).
//! On the dark ground photon's theme gives every colour back as it is, so
//! they're constants here.

use rush_core::text::{Color, Style};
use rush_core::time::Ms;

use super::txt::Txt;

const fn fg(r: u8, g: u8, b: u8) -> Style {
    Style::fg(Color::Rgb(r, g, b))
}

pub(crate) const PLAIN: Style = Style::PLAIN;
pub(crate) const C_TEXT: Style = fg(226, 221, 211);
pub(crate) const C_SUB: Style = fg(168, 162, 152);
pub(crate) const C_DIM: Style = fg(138, 132, 122);
pub(crate) const C_FAINT: Style = fg(94, 89, 82);
pub(crate) const C_WHITE: Style = fg(240, 236, 228);
/// What tools print: under faint text, over faint rules.
pub(crate) const C_OUT: Style = fg(119, 113, 106);
pub(crate) const C_ORANGE: Style = fg(217, 119, 87);
pub(crate) const C_YELLOW: Style = fg(229, 181, 103);
pub(crate) const C_BLUE: Style = fg(143, 179, 217);
pub(crate) const C_GREEN: Style = fg(127, 191, 138);
pub(crate) const C_RED: Style = fg(224, 104, 92);
/// A card's frame: rewrote or set something aside; threw away or failed.
pub(crate) const C_WARN_Q: Style = fg(168, 136, 82);
pub(crate) const C_LOST_Q: Style = fg(170, 86, 76);
/// A card's frame: made something. Its glyph when it threw away or failed.
pub(crate) const C_OK_Q: Style = fg(95, 138, 104);
pub(crate) const C_LOST: Style = C_RED;
/// A subagent's colour.
pub(crate) const C_HELPER: Style = fg(178, 160, 214);

// Surfaces: only what you said and diffs get a ground.
pub(crate) const BG_ADD: Color = Color::Rgb(0x16, 0x30, 0x1a);
pub(crate) const BG_DEL: Color = Color::Rgb(0x3a, 0x17, 0x14);
pub(crate) const BG_ADD_HI: Color = Color::Rgb(0x22, 0x52, 0x2b);
pub(crate) const BG_DEL_HI: Color = Color::Rgb(0x62, 0x24, 0x1e);
/// Output and code: none, as in Go (bgWell is "").
pub(crate) const BG_WELL: Color = Color::Default;

// Syntax colours, muted to sit under rush's text.
pub(crate) const HL_KW: Style = fg(204, 153, 205);
pub(crate) const HL_STR: Style = fg(163, 190, 140);
pub(crate) const HL_NUM: Style = fg(222, 165, 132);
pub(crate) const HL_FN: Style = fg(137, 180, 222);
pub(crate) const HL_TYPE: Style = fg(120, 190, 175);
pub(crate) const HL_COMMENT: Style = fg(122, 116, 108);
pub(crate) const HL_OUT_COMMENT: Style = fg(92, 87, 80);

/// The edge of a row: the agent's words, its work, you, a subagent.
pub(crate) const SPINE_BAR: Style = fg(176, 98, 72);
pub(crate) const SPINE_TOOL: Style = fg(110, 72, 58);

/// The bold colour for headings drawn in c: white over body text, c itself
/// over quieter text (render.go:1655).
pub(crate) fn strong(c: Style) -> Style {
    if c == C_TEXT {
        C_WHITE.bold()
    } else {
        c.bold()
    }
}

pub(crate) fn paint(c: Style, s: &str) -> Txt {
    Txt::styled(s, c)
}
pub(crate) fn text(s: &str) -> Txt {
    paint(C_TEXT, s)
}
pub(crate) fn sub(s: &str) -> Txt {
    paint(C_SUB, s)
}
pub(crate) fn dim(s: &str) -> Txt {
    paint(C_DIM, s)
}
pub(crate) fn faint(s: &str) -> Txt {
    paint(C_FAINT, s)
}

/// The keyboard's mark in the gutter.
pub(crate) fn cursor() -> Txt {
    paint(C_SUB, "▏")
}

pub(crate) fn plus_sign() -> Txt {
    paint(C_GREEN.bold(), "+")
}
pub(crate) fn minus_sign() -> Txt {
    paint(C_RED.bold(), "−")
}

/// The ref of every "ctrl+o shows all" row.
pub const SHOW_ALL_REF: &str = "showall";

/// n lines folded away, and the key that unfolds them.
pub(crate) fn folded(n: usize) -> Txt {
    let mut t = dim("… ");
    t.push(paint(C_TEXT.bold(), &n.to_string()));
    t.push(dim(" lines "));
    t.push(faint("·"));
    t.push_str(" ", PLAIN);
    t.push(paint(C_ORANGE.bold(), "ctrl+o"));
    t.push(dim(" shows all"));
    t
}

pub(crate) const SPINNER: [&str; 10] = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];

pub(crate) fn spin(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

/// What a turn thinking is called, one a stretch of thinking.
pub(crate) const MUSINGS: &[&str] = &[
    "thinking",
    "pondering",
    "mulling it over",
    "cogitating",
    "chin-stroking",
    "ruminating",
    "faffing about",
    "dillydallying",
    "pottering",
    "dithering",
    "chuntering",
    "noodling",
    "having a cuppa",
    "putting the kettle on",
    "rummaging",
    "tinkering",
    "bodging",
    "waffling",
    "processing",
    "inhaling",
    "taking a deep breath",
    "rigging",
    "tying knots",
    "getting tied up in knots",
    "tying up loose ends",
    "unpicking it",
    "gubbing",
    "wrangling",
    "watching the langs",
    "binding",
    "experiencing euphoria",
    "microgurning",
    "implicating",
    "golden godding",
    "consulting bird law",
    "wildcarding",
    "fighting the Nightman",
    "rum hamming",
    "boiling denim",
    "fixing the gas crisis",
    "opening Hormuz",
    "closing the Strait",
    "good boy",
    "consulting the rubber duck",
    "staring into the middle distance",
    "reticulating splines",
    "asking its mum",
    "counting sheep",
    "doing a little jig",
    "stroking an imaginary beard",
    "having a biscuit",
    "reading the manual for once",
    "pretending to understand",
    "feeding the hamsters",
    "spinning up the hamster wheel",
    "herding cats",
    "shaking the magic 8-ball",
];

/// How long a timer shows tenths of a second for.
pub(crate) const FAST_UNDER: Ms = 5_000;

/// A duration as rush says it: 1.2s, 14s, 3m 04s, 2h 05m.
pub fn dur(ms: Ms) -> String {
    if ms < 0 {
        String::new()
    } else if ms < FAST_UNDER {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else if ms < 3_600_000 {
        format!("{}m {:02}s", ms / 60_000, (ms / 1000) % 60)
    } else {
        format!("{}h {:02}m", ms / 3_600_000, (ms / 60_000) % 60)
    }
}

pub fn money(v: f64) -> String {
    if v <= 0.0 {
        String::new()
    } else {
        format!("${v:.2}")
    }
}

/// s on one line, single spaced (style.go:282).
pub(crate) fn one_line(s: &str) -> String {
    let b = s.as_bytes();
    let single = !b.is_empty()
        && b[0] != b' '
        && b[b.len() - 1] != b' '
        && b.iter()
            .enumerate()
            .all(|(i, &c)| !(c < 0x21 && (c != b' ' || b.get(i + 1) == Some(&b' ')) || c >= 0x7f));
    if single || s.is_empty() {
        return s.to_string();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Tokens as a count reads: 840, 12k, 1.2M (overview.go:168).
pub(crate) fn tokens(n: i64) -> String {
    if n >= 1_000_000 {
        let s = format!("{:.1}", n as f64 / 1e6);
        format!("{}M", s.strip_suffix(".0").unwrap_or(&s))
    } else if n >= 1000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// "15:04" of t, in the zone offset by `zone` seconds from UTC.
pub(crate) fn clock(t: Ms, zone: i64) -> String {
    let secs = (t / 1000 + zone).rem_euclid(86_400);
    format!("{:02}:{:02}", secs / 3600, (secs / 60) % 60)
}

/// A key's name as your keyboard says it: a Mac has no alt key, so alt+
/// is ⌥ there (style.go:404).
pub fn key_word(s: &str) -> String {
    if cfg!(target_os = "macos") {
        s.replace("alt+", "⌥")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(dur(1000), "1.0s");
        assert_eq!(dur(13_000), "13s");
        assert_eq!(dur(184_000), "3m 04s");
        assert_eq!(dur(99_420_000), "27h 37m");
        assert_eq!(dur(-1), "");
    }

    #[test]
    fn token_counts() {
        assert_eq!(tokens(93), "93");
        assert_eq!(tokens(12_400), "12k");
        assert_eq!(tokens(1_250_000), "1.2M");
        assert_eq!(tokens(50_000), "50k");
    }
}
