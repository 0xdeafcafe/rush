//! The drawn keyboard Settings, Keys shows a binding on: where your
//! fingers go, the key just pressed brightest, a binding's keys lit, the
//! keys bound here plain and the rest faint. Port of
//! internal/ui/keyboard.go:1-178 (kbRows :22, keyParts :68, kbFaces :91,
//! kbCells :111, keyboard :138, kbLegend :148, kbGrid :155). The burst
//! typed on Keys (kbBurst :197) is left to Settings' port.

use std::collections::HashSet;

use rush_core::text::{Color, Line, Span, Style, width};

use crate::style::*;

/// A key on the drawn keyboard: its name as the keymap spells it, what its
/// cap says, and how wide the cap is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KbKey {
    pub name: &'static str,
    pub label: &'static str,
    pub w: usize,
}

const fn k(name: &'static str, label: &'static str, w: usize) -> KbKey {
    KbKey { name, label, w }
}

/// The keyboard's width in cells.
pub const KB_WIDE: usize = 58;

/// The keyboard's rows, each staggered as a real one's are (kbRows).
pub fn kb_rows() -> Vec<Vec<KbKey>> {
    let plain = |ks: &'static str| ks.split(' ').map(|x| k(x, x, 3)).collect::<Vec<_>>();
    let (alt, sup) = if cfg!(target_os = "macos") {
        ("⌥", "⌘")
    } else {
        ("alt", "super")
    };
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
}

/// The key each shifted character is typed on.
fn shifted(c: &str) -> Option<&'static str> {
    const S: &[(&str, &str)] = &[
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
    ];
    S.iter().find(|(a, _)| *a == c).map(|(_, b)| *b)
}

/// The keyboard's keys a binding is pressed with, a chord's keys all
/// together (keyParts).
pub fn key_parts(seq: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for key in seq {
        let cut = key.len().saturating_sub(1);
        let (mut base, mods) = match key[..cut].rfind('+') {
            Some(i) => (key[i + 1..].to_string(), &key[..i]),
            None => (key.clone(), ""),
        };
        if !mods.is_empty() {
            for m in mods.split('+') {
                out.insert(m.to_string());
            }
        }
        if let Some(b) = shifted(&base) {
            out.insert("shift".into());
            base = b.into();
        } else if base.chars().count() == 1 && base != base.to_lowercase() {
            out.insert("shift".into());
            base = base.to_lowercase();
        }
        out.insert(base);
    }
    out.remove("");
    out
}

/// The keycaps' faces: the key just pressed, the keys lit, those bound
/// here, and the rest (kbFaces).
pub fn kb_faces() -> [Style; 4] {
    let ink = Color::Rgb(33, 28, 22);
    let mut hit = Style::fg(ink).bold();
    hit.bg = Color::Rgb(250, 236, 214);
    let mut lit = Style::fg(ink).bold();
    lit.bg = ORANGE;
    let mut used = Style::fg(TEXT);
    used.bg = Color::Rgb(48, 44, 40);
    let mut free = Style::fg(FAINT);
    free.bg = CHROME_BG;
    [hit, lit, used, free]
}

/// One cell of a drawn keycap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KbCell {
    pub x: usize,
    pub y: usize,
    pub c: char,
    pub face: Style,
}

/// The keyboard's cells, each key's face picked (kbCells).
pub fn kb_cells(
    hit: &HashSet<String>,
    lit: &HashSet<String>,
    used: &HashSet<String>,
) -> Vec<KbCell> {
    let [f_hit, f_lit, f_used, f_free] = kb_faces();
    let mut out = Vec::new();
    for (y, row) in kb_rows().iter().enumerate() {
        let mut x = 0;
        for key in row {
            let face = if hit.contains(key.name) {
                f_hit
            } else if lit.contains(key.name) {
                f_lit
            } else if used.contains(key.name) {
                f_used
            } else {
                f_free
            };
            let pad = key.w.saturating_sub(width(key.label));
            let text = format!("{}{}{}", blanks(pad / 2), key.label, blanks(pad - pad / 2));
            for c in text.chars() {
                out.push(KbCell { x, y, c, face });
                x += 1;
            }
            x += 1;
        }
    }
    out
}

/// The keyboard drawn (keyboard).
pub fn keyboard(hit: &HashSet<String>, lit: &HashSet<String>, used: &HashSet<String>) -> Vec<Line> {
    let rows = kb_rows().len();
    let mut grid: Vec<Vec<Option<(char, Style)>>> = vec![vec![None; KB_WIDE]; rows];
    for c in kb_cells(hit, lit, used) {
        if c.x < KB_WIDE && c.y < rows {
            grid[c.y][c.x] = Some((c.c, c.face));
        }
    }
    grid.into_iter()
        .map(|row| {
            let last = row.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
            let mut l = Line::new();
            for cell in &row[..last] {
                match cell {
                    Some((c, f)) => l.spans.push(Span::new(c.to_string(), *f)),
                    None => l.spans.push(plain(" ")),
                }
            }
            l
        })
        .collect()
}

/// What the keyboard's faces mean (kbLegend).
pub fn kb_legend() -> Line {
    let [_, lit, used, free] = kb_faces();
    let sw = |face: Style, what: &str| [Span::new("   ", face), sp(format!(" {what}"), DIM)];
    let mut l = Line::new();
    l.spans.extend(sw(lit, "this one's keys"));
    l.spans.push(plain("   "));
    l.spans.extend(sw(used, "bound here"));
    l.spans.push(plain("   "));
    l.spans.extend(sw(free, "free"));
    l.spans.push(sp("   · press a key to find it", FAINT));
    l
}
