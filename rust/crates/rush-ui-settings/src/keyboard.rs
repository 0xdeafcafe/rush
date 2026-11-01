//! The keyboard Settings › Keys draws, a binding lit where your fingers
//! go, and keys drawn as keycaps. After Go's internal/ui/keyboard.go
//! (kbRows :21, kbWide :52, shifted :55, keyParts :62, kbFaces :87,
//! kbCells :101, keyboard :129, kbLegend :138, kbGrid :146) and
//! question.go:24-38 (qCapOn, qInk, keycap). The burst (kbBurst, kbCheer)
//! is drawn nowhere in Go, so isn't here.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::LazyLock;

use rush_core::text::{Color, Line, Span, Style, width};

use crate::style::*;

/// A lit keycap's ground (qCapOn).
pub const CAP_ON: Color = Color::Rgb(217, 119, 87);
/// A keycap's ground (qCap).
pub const CAP: Color = Color::Rgb(48, 44, 40);
/// A card's ground (qCard: bgChrome).
pub const CARD: Color = Color::Rgb(30, 28, 26);
/// Dark text on a lit chip or keycap (qInk).
pub const INK: Color = Color::Rgb(33, 28, 22);
/// The key just pressed's cap (kbHitBG).
pub const HIT: Color = Color::Rgb(250, 236, 214);

/// A key drawn as a key: " 1 " on its own ground, lit when it's the one
/// under the cursor (keycap).
pub fn keycap(k: &str, on: bool) -> Span {
    let k = rush_convo::render::key_word(k);
    if on {
        return Span::new(format!(" {k} "), on_bg(INK, CAP_ON, true));
    }
    Span::new(format!(" {k} "), on_bg(SUB, CAP, false))
}

fn on_bg(fg: Color, bg: Color, b: bool) -> Style {
    let mut s = Style::fg(fg);
    s.bg = bg;
    if b { s.bold() } else { s }
}

/// A key on the drawn keyboard: its name as keymap spells it, what its
/// cap says, and how wide the cap is (kbKey).
struct KbKey {
    name: &'static str,
    label: &'static str,
    w: usize,
}

/// The keyboard's rows, each staggered as a real one's are (kbRows).
static KB_ROWS: LazyLock<Vec<Vec<KbKey>>> = LazyLock::new(|| {
    let plain = |ks: &'static str| -> Vec<KbKey> {
        ks.split(' ')
            .map(|k| KbKey {
                name: k,
                label: k,
                w: 3,
            })
            .collect()
    };
    let (alt, sup) = if cfg!(target_os = "macos") {
        ("⌥", "⌘")
    } else {
        ("alt", "super")
    };
    let k = |name, label, w| KbKey { name, label, w };
    let cat = |parts: Vec<Vec<KbKey>>| parts.into_iter().flatten().collect::<Vec<_>>();
    vec![
        vec![k("esc", "esc", 5)],
        cat(vec![
            plain("` 1 2 3 4 5 6 7 8 9 0 - ="),
            vec![k("backspace", "⌫", 6)],
        ]),
        cat(vec![
            vec![k("tab", "tab", 5)],
            plain("q w e r t y u i o p [ ]"),
            vec![k("\\", "\\", 4)],
        ]),
        cat(vec![
            vec![k("", "", 6)],
            plain("a s d f g h j k l ; '"),
            vec![k("enter", "enter", 7)],
        ]),
        cat(vec![
            vec![k("shift", "shift", 8)],
            plain("z x c v b n m , . /"),
            vec![k("shift", "shift", 9)],
        ]),
        vec![
            k("ctrl", "ctrl", 5),
            k("alt", alt, 3),
            k("super", sup, 5),
            k("space", "space", 17),
            k("super", sup, 5),
            k("alt", alt, 3),
            k("left", "←", 3),
            k("up", "↑", 3),
            k("down", "↓", 3),
            k("right", "→", 3),
        ],
    ]
});

/// The keyboard's width in cells (kbWide).
pub const KB_WIDE: usize = 58;

/// The key each shifted character is typed on (shifted).
fn shifted(c: &str) -> Option<&'static str> {
    static M: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
        [
            ("~", "`"),
            ("!", "1"),
            ("@", "2"),
            ("#", "3"),
            ("$", "4"),
            ("%", "5"),
            ("^", "6"),
            ("&", "7"),
            ("*", "8"),
            ("(", "9"),
            (")", "0"),
            ("_", "-"),
            ("+", "="),
            ("{", "["),
            ("}", "]"),
            ("|", "\\"),
            (":", ";"),
            ("\"", "'"),
            ("<", ","),
            (">", "."),
            ("?", "/"),
        ]
        .into_iter()
        .collect()
    });
    M.get(c).copied()
}

/// The keyboard's keys a binding is pressed with, a chord's keys all
/// together (keyParts).
pub fn key_parts(seq: &[String]) -> HashSet<String> {
    let mut out: HashMap<String, bool> = HashMap::new();
    for k in seq {
        let (mut base, mods) = {
            // The last + before the final character: "ctrl++" is ctrl and +.
            let head = &k[..k.char_indices().last().map_or(0, |(i, _)| i)];
            match head.rfind('+') {
                Some(i) => (k[i + 1..].to_string(), &k[..i]),
                None => (k.clone(), ""),
            }
        };
        for m in mods.split('+') {
            out.insert(m.to_string(), !mods.is_empty());
        }
        if let Some(b) = shifted(&base) {
            out.insert("shift".into(), true);
            base = b.into();
        } else if base.chars().count() == 1 && base != base.to_lowercase() {
            out.insert("shift".into(), true);
            base = base.to_lowercase();
        }
        out.insert(base, true);
    }
    out.remove("");
    out.into_iter()
        .filter(|(_, v)| *v)
        .map(|(k, _)| k)
        .collect()
}

/// The keycaps' faces: the key just pressed, the keys lit, those bound
/// here, and the rest, dimmer than any cap in use (kbFaces).
fn faces() -> [Style; 4] {
    [
        on_bg(INK, HIT, true),
        on_bg(INK, CAP_ON, true),
        on_bg(TEXT, CAP, false),
        on_bg(FAINT, CARD, false),
    ]
}

/// One cell of a drawn keycap (kbCell).
struct Cell {
    x: usize,
    y: usize,
    c: char,
    face: Style,
}

/// The keyboard's cells, each key's face picked as keyboard draws it
/// (kbCells).
fn cells(hit: &HashSet<String>, lit: &HashSet<String>, used: &HashSet<String>) -> Vec<Cell> {
    let [f_hit, f_lit, f_used, f_free] = faces();
    let mut out = Vec::new();
    for (y, row) in KB_ROWS.iter().enumerate() {
        let mut x = 0;
        for k in row {
            let face = if hit.contains(k.name) {
                f_hit
            } else if lit.contains(k.name) {
                f_lit
            } else if used.contains(k.name) {
                f_used
            } else {
                f_free
            };
            let pad = k.w.saturating_sub(width(k.label));
            let s = format!(
                "{}{}{}",
                " ".repeat(pad / 2),
                k.label,
                " ".repeat(pad - pad / 2)
            );
            for c in s.chars() {
                out.push(Cell { x, y, c, face });
                x += 1;
            }
            x += 1;
        }
    }
    out
}

/// The keyboard: the key just pressed (hit) brightest, lit's keys lit,
/// used's plainly and the rest faint (keyboard).
pub fn keyboard(hit: &HashSet<String>, lit: &HashSet<String>, used: &HashSet<String>) -> Vec<Line> {
    let h = KB_ROWS.len();
    let mut grid: Vec<Vec<Option<(char, Style)>>> = vec![vec![None; KB_WIDE]; h];
    for c in cells(hit, lit, used) {
        if c.x < KB_WIDE && c.y < h {
            grid[c.y][c.x] = Some((c.c, c.face));
        }
    }
    grid.into_iter()
        .map(|row| {
            let last = row.iter().rposition(Option::is_some);
            let mut l = Line::new();
            for cell in row.into_iter().take(last.map_or(0, |i| i + 1)) {
                match cell {
                    None => l.spans.push(plain(" ")),
                    Some((r, face)) => {
                        // Runs of one face go in one span.
                        if let Some(s) = l.spans.last_mut()
                            && s.style == face
                        {
                            s.text.push(r);
                        } else {
                            l.spans.push(Span::new(r.to_string(), face));
                        }
                    }
                }
            }
            l
        })
        .collect()
}

/// What the keyboard's faces mean (kbLegend).
pub fn legend() -> Line {
    let [_, f_lit, f_used, f_free] = faces();
    let mut l = Line::new();
    for (i, (face, what)) in [
        (f_lit, "this one's keys"),
        (f_used, "bound here"),
        (f_free, "free"),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            l.spans.push(plain("   "));
        }
        l.spans.push(Span::new("   ", face));
        l.spans.push(plain(" "));
        l.spans.push(sp(what, DIM));
    }
    l.spans.push(sp("   · press a key to find it", FAINT));
    l
}
