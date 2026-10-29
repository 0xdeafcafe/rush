//! What the cards in a Session's dock are drawn with: their grounds, a
//! key drawn as a key, the lit heading chip, a card's buttons, an option's
//! preview box. After Go's internal/ui/question.go (qCard :20, keycap :30,
//! chip :39, previewBox :366), memcard.go (cardBtn :26, btnTone :349,
//! cardButtons :371), style.go:66 (btnFill), rushmode.go (onBg :2170,
//! spread :2175, optionLabel :5464, shownAnswer :5709) and view.go:2502
//! (shortDir).

use rush_core::text::{Color, Line, Span, Style, width};

use crate::style::*;

/// The card's own ground (qCard: the chrome's).
pub const CARD_BG: Color = CHROME_BG;
/// The option under the cursor (qSel).
pub const CARD_SEL_BG: Color = SEL_BG;
/// A keycap's ground (qCap).
pub const CAP_BG: Color = Color::Rgb(48, 44, 40);
/// Dark text on a lit chip or keycap (qInk).
pub const INK: Color = Color::Rgb(33, 28, 22);
/// The ground rush's colours are made for: a filled button's words.
pub const GROUND: Color = Color::Rgb(17, 16, 14);
/// The tab look: lit (tabOn), and quiet (tabOff).
pub const TAB_ON: Style = Style {
    fg: TAB_ON_FG,
    bg: ORANGE,
    mods: rush_core::text::Mods::BOLD,
};
pub const TAB_OFF: Style = Style {
    fg: SUB,
    bg: TAB_OFF_BG,
    mods: rush_core::text::Mods::NONE,
};

/// s in colour c on ground bg.
pub fn on(s: impl Into<String>, c: Color, bg: Color) -> Span {
    Span::new(s, Style::fg(c).on(bg))
}

/// A key drawn as a key: " 1 " on its own ground, lit when it's the one
/// under the cursor (keycap).
pub fn keycap(k: &str, lit: bool) -> Span {
    let k = rush_convo::render::key_word(k);
    if lit {
        return Span::new(format!(" {k} "), Style::fg(INK).on(ORANGE).bold());
    }
    on(format!(" {k} "), SUB, CAP_BG)
}

/// A lit label: the card's heading (chip).
pub fn chip(s: &str) -> Span {
    Span::new(format!(" {s} "), Style::fg(INK).on(YELLOW).bold())
}

/// l fit to w on ground bg (onBg).
pub fn on_bg(l: Line, bg: Color, w: usize) -> Line {
    on_ground(l, w, bg)
}

/// left, then right at the far end of w; just left when they don't both
/// fit (spread).
pub fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut l = left;
    l.spans.push(plain(blanks(gap as usize)));
    join(&mut l, right);
    l
}

/// A button on a card, where a click presses it: x and y from the card's
/// top left, three rows high (cardBtn).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardBtn {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub key: String,
}

/// A button: the key it presses, what shows for its key, its name.
pub type Btn = [String; 3];

/// A button's colour, by what its key does: yes green, no red, for good
/// blue, don't-ask yellow, edit first purple (btnTone).
pub fn btn_tone(key: &str) -> Color {
    match key {
        "y" | "enter" => GREEN,
        "n" => RED,
        "a" => BLUE,
        "o" => YELLOW,
        "e" => QUEUE,
        _ => ORANGE,
    }
}

/// A card's buttons, three rows starting y rows into the card, and where
/// each is: each outlined in the colour of what it does, its key bold in
/// it. The picked one is enter's: while the card has the keys it's filled
/// solid, as is one under the pointer (cardButtons).
pub fn card_buttons(
    bs: &[Btn],
    focus: bool,
    pick: usize,
    hover: &str,
    y: usize,
) -> ([Line; 3], Vec<CardBtn>) {
    let mut rows: [Line; 3] = Default::default();
    let mut btns = Vec::with_capacity(bs.len());
    let mut x = 3; // past the edge and its two spaces
    for (i, b) in bs.iter().enumerate() {
        let iw = width(&format!(" {} {}  ", b[1], b[2]));
        let tone = btn_tone(&b[0]);
        if i > 0 {
            for r in rows.iter_mut() {
                r.spans.push(plain("  "));
            }
            x += 2;
        }
        if focus && i == pick || hover == format!("btn:{}", b[0]) {
            // Solid: half blocks above and below, the middle filled.
            rows[0].spans.push(sp("▄".repeat(iw + 2), tone));
            rows[1].spans.push(Span::new(
                fit_str(&format!("  {} {}", b[1], b[2]), iw + 2),
                Style::fg(GROUND).on(tone).bold(),
            ));
            rows[2].spans.push(sp("▀".repeat(iw + 2), tone));
        } else {
            let name = if i == pick {
                spb(b[2].clone(), BRIGHT)
            } else {
                sp(b[2].clone(), TEXT)
            };
            rows[0]
                .spans
                .push(sp(format!("╭{}╮", "─".repeat(iw)), tone));
            rows[1].spans.extend([
                sp("│", tone),
                plain(" "),
                spb(b[1].clone(), tone),
                plain(" "),
                name,
                plain("  "),
                sp("│", tone),
            ]);
            rows[2]
                .spans
                .push(sp(format!("╰{}╯", "─".repeat(iw)), tone));
        }
        btns.push(CardBtn {
            x,
            y,
            w: iw + 2,
            key: b[0].clone(),
        });
        x += iw + 2;
    }
    (rows, btns)
}

/// An option's preview framed under its name: Claude writes it as
/// markdown, most often a mockup or code, so its lines are kept as they
/// are (fences dropped), cut to the width and to max_rows rows
/// (previewBox).
pub fn preview_box(name: &str, md: &str, w: usize, max_rows: usize) -> Vec<Line> {
    let md = md.replace('\t', "    ");
    let mut body: Vec<String> = md
        .split('\n')
        .filter(|l| !l.trim().starts_with("```"))
        .map(|l| strip_ansi(l.trim_end_matches([' ', '\r'])))
        .collect();
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    let mut more = 0;
    if body.len() > max_rows {
        more = body.len() - max_rows + 1;
        body.truncate(max_rows.saturating_sub(1));
    }
    let mut inner = 8;
    for l in &body {
        inner = inner.max(width(l));
    }
    inner = inner.min(8.max(w.saturating_sub(4)));
    let label =
        rush_draw::truncate(&one_line(name), 4.max(inner.saturating_sub(2)), "…").into_owned();
    let mut out = vec![line([
        sp("╭─", FAINT),
        plain(" "),
        sp(label.clone(), SUB),
        plain(" "),
        sp(
            format!("{}╮", "─".repeat(inner.saturating_sub(width(&label) + 1))),
            FAINT,
        ),
    ])];
    for l in body {
        let mut l = l;
        let mut more_mark = None;
        if width(&l) > inner {
            l = rush_draw::truncate(&l, inner - 1, "").into_owned();
            more_mark = Some(sp("›", FAINT));
        }
        let lw = width(&l) + usize::from(more_mark.is_some());
        let mut r = line([sp("│", FAINT), plain(" "), sp(l, TEXT)]);
        if let Some(m) = more_mark {
            r.spans.push(m);
        }
        r.spans.push(plain(blanks(inner.saturating_sub(lw))));
        r.spans.push(plain(" "));
        r.spans.push(sp("│", FAINT));
        out.push(r);
    }
    if more > 0 {
        let t = format!("… {more} more lines");
        let tw = width(&t);
        out.push(line([
            sp("│", FAINT),
            plain(" "),
            sp(t, DIM),
            plain(blanks(inner.saturating_sub(tw))),
            plain(" "),
            sp("│", FAINT),
        ]));
    }
    out.push(line([sp(format!("╰{}╯", "─".repeat(inner + 2)), FAINT)]));
    out
}

/// s without escape sequences (ansi.Strip).
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('[') => {
                it.next();
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                it.next();
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {
                it.next();
            }
        }
    }
    out
}

/// An option's label without "(Recommended)", and whether it had it
/// (optionLabel).
pub fn option_label(l: &str) -> (String, bool) {
    let l = l.trim();
    let low = l.to_lowercase();
    match low.find("(recommended)") {
        // Lowercasing keeps ASCII offsets; anything else falls back.
        Some(i) if low.len() == l.len() => {
            let rest = format!("{}{}", &l[..i], &l[i + "(recommended)".len()..]);
            (rest.trim().to_string(), true)
        }
        _ => (l.to_string(), false),
    }
}

/// An answer as the card shows it (shownAnswer).
pub fn shown_answer(a: &str) -> String {
    one_line(&option_label(a).0)
}

/// A path in at most n cells, its middle left out, so the folder's own
/// name always shows: ~/…/langwatch/.worktrees/go-ports (shortDir).
pub fn short_dir(p: &str, n: usize) -> String {
    if width(p) <= n {
        return p.to_string();
    }
    let parts: Vec<&str> = p.split('/').collect();
    let mut tail = parts[parts.len() - 1].to_string();
    for i in (1..parts.len().saturating_sub(1)).rev() {
        if width(&format!("{}/…/{}/{}", parts[0], parts[i], tail)) > n {
            break;
        }
        tail = format!("{}/{tail}", parts[i]);
    }
    rush_draw::truncate(&format!("{}/…/{tail}", parts[0]), n, "…").into_owned()
}

/// s's inline markdown drawn in colour c (and bold when b), wrapped to w
/// cells (wrap(convo.Inline(s, c), w)).
pub fn inline_wrap(s: &str, st: Style, w: usize) -> Vec<Line> {
    let l = rush_convo::render::inline(s, st);
    let mut out = Vec::new();
    // Inline draws one paragraph; each of s's lines wraps on its own.
    if s.contains('\n') {
        for para in s.split('\n') {
            if para.trim().is_empty() {
                out.push(Line::new());
                continue;
            }
            out.extend(wrap_line(&rush_convo::render::inline(para, st), w));
        }
        return out;
    }
    wrap_line(&l, w)
}

/// The first of ss that isn't empty.
pub fn first_non_empty<'a>(ss: &[&'a str]) -> &'a str {
    ss.iter().copied().find(|s| !s.is_empty()).unwrap_or("")
}
