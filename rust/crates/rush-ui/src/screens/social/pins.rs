//! What you asked that you're reading the answer to stays pinned atop a
//! conversation, one row, while it's scrolled off its top: at the end
//! that's the latest; scrolled up, the current one. Clicking it goes to
//! it. Port of Go's promptpins.go (yourPrompt :26, pinText :29,
//! pinPrompts :34), drawn over the open conversation's first body row.
//! A paste in a pinned prompt shows as its chip, as convo draws it
//! (render/draw.rs fold_pastes, paste_chip).

use std::sync::LazyLock;

use regex::Regex;
use rush_convo::Turn;
use rush_core::text::Line;

use crate::frame::{Mouse, Out};
use crate::model::Model;
use crate::session::Pos;
use crate::style::*;

/// A pin's row's ref.
pub const PIN_REF_PREFIX: &str = "pin:";

/// Whether turn t began with something you said (yourPrompt).
pub fn your_prompt(t: &Turn) -> bool {
    t.from.is_empty() && !t.prompt.trim().is_empty()
}

static PASTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)\s*<pasted_content(?: id="[^"]*")?>\n?(.*?)\n?</pasted_content(?: id="[^"]*")?>\s*"#,
    )
    .expect("a fixed pattern")
});

/// Each paste in a message as the chip it was in the box.
pub fn fold_pastes(s: &str) -> String {
    if !s.contains("<pasted_content") {
        return s.trim().to_string();
    }
    let mut n = 0;
    let out = PASTED.replace_all(s, |c: &regex::Captures| {
        n += 1;
        format!(" {} ", paste_chip(n, &c[1]))
    });
    out.trim().to_string()
}

/// A paste as its chip: number, lines, how it starts and ends.
fn paste_chip(n: usize, text: &str) -> String {
    let lines = text.trim_end_matches('\n').matches('\n').count() + 1;
    let unit = if lines == 1 { "line" } else { "lines" };
    let flat: Vec<char> = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| *c != '[' && *c != ']')
        .collect();
    const EACH: usize = 8;
    let mut preview: String = flat.iter().collect();
    if flat.len() > 2 * EACH + 3 {
        let mut start = &flat[..EACH];
        let mut end = &flat[flat.len() - EACH..];
        for i in (0..start.len()).rev() {
            if start[i] == ' ' && i >= EACH / 2 {
                start = &start[..i];
                break;
            }
        }
        if let Some(i) = end.iter().position(|c| *c == ' ')
            && end.len() - i > EACH / 2
        {
            end = &end[i + 1..];
        }
        preview = format!(
            "{} … {}",
            start.iter().collect::<String>().trim(),
            end.iter().collect::<String>().trim()
        );
    }
    if preview.is_empty() {
        return format!("[pasted text #{n} · {lines} {unit}]");
    }
    format!("[pasted text #{n} · {lines} {unit}: {preview}]")
}

/// A prompt as its pinned row shows it: one line, pastes folded
/// (pinText).
pub fn pin_text(t: &Turn) -> String {
    one_line(&fold_pastes(&t.prompt))
}

/// The prompt to pin, by turn index, and its label: none when nothing
/// you asked has scrolled off the top of rows [start, end); turn_row
/// is where each turn's first row is (pinPrompts).
pub fn pin_of(
    turns: &[Turn],
    turn_row: impl Fn(usize) -> usize,
    start: usize,
    end: usize,
) -> Option<(usize, &'static str)> {
    let first = turns.iter().position(your_prompt);
    // Pinned only once scrolled off the top: one still below is reached
    // by going on down, and a pin for it would sit over what's at the top.
    let off = |i: usize| turn_row(i) < start;
    let (mut latest, mut current) = (None, None);
    let lo = first.map_or(0, |f| f + 1);
    for i in (lo..turns.len()).rev() {
        if !your_prompt(&turns[i]) {
            continue;
        }
        if latest.is_none() {
            latest = Some(i);
        }
        if off(i) {
            current = Some(i);
            break;
        }
    }
    let mut label = "current";
    if current == latest {
        label = "latest ";
    } else if latest.is_some_and(|l| turn_row(l) < end) {
        current = None; // the latest is on screen: that's what it's on
    }
    current.filter(|&i| off(i)).map(|i| (i, label))
}

/// A pin's row, w wide.
pub fn pin_row(label: &str, text: &str, w: usize) -> Line {
    on_ground(
        line([
            sp("▍", BLUE),
            plain(" "),
            sp(label, FAINT),
            plain(" "),
            sp(text, TEXT),
        ]),
        w,
        CHROME_BG,
    )
}

/// Pins the prompt over the open conversation's first body row, when one
/// has scrolled off its top and the body has rows to spare.
#[allow(clippy::ptr_arg)] // the Overlay hook's own signature
pub(super) fn over(m: &mut Model, out: &mut Vec<Line>) {
    let w = m.w;
    let Some(v) = m.session.as_mut() else {
        return;
    };
    let rows = v.shown.len();
    if rows == 0 || v.sess.turns.is_empty() {
        return;
    }
    let total = v.r.count(&v.sess, &v.o);
    let start = match v.pos {
        Pos::End => total.saturating_sub(rows),
        Pos::At { n, off } => match v.sess.turns.iter().position(|t| t.n == n) {
            Some(i) => v.r.turn_row(i) + off,
            None => total.saturating_sub(rows),
        },
    };
    let end = start + rows;
    let r = &v.r;
    let Some((i, label)) = pin_of(&v.sess.turns, |i| r.turn_row(i), start, end) else {
        return;
    };
    // A short pane keeps its rows for the conversation.
    if rows < 1 + 6 {
        return;
    }
    let text = pin_text(&v.sess.turns[i]);
    let top = v.top;
    if top < out.len() {
        let mut row = line([plain("  ")]);
        join(&mut row, pin_row(label, &text, w.saturating_sub(3)));
        out[top] = fit(row, w);
        v.shown[0] = format!("{PIN_REF_PREFIX}{}", v.sess.turns[i].n);
    }
}

/// A click on a pin goes to its turn.
pub(super) fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if e.kind != rush_draw::MouseKind::Down(rush_draw::Button::Left) || m.sheet.is_some() {
        return Out::Pass;
    }
    let Some(v) = m.session.as_mut() else {
        return Out::Pass;
    };
    let y = e.y as usize;
    let Some(r) = y.checked_sub(v.top).and_then(|i| v.shown.get(i)) else {
        return Out::Pass;
    };
    let Some(n) = r
        .strip_prefix(PIN_REF_PREFIX)
        .and_then(|n| n.parse::<u32>().ok())
    else {
        return Out::Pass;
    };
    v.pos = Pos::At { n, off: 0 };
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(n: u32, prompt: &str, from: &str) -> Turn {
        let mut t = Turn::default();
        t.n = n;
        t.prompt = prompt.into();
        t.from = from.into();
        t
    }

    // Scrolled into a long conversation, the prompt you're reading under is
    // pinned once it's scrolled off the top; at the end, with the latest
    // on screen, nothing is; nor at the very top.
    #[test]
    fn pins() {
        let turns: Vec<Turn> = (0..10)
            .map(|i| turn(i + 1, &format!("turn {}: do it", i + 1), ""))
            .collect();
        let row = |i: usize| i * 20; // each turn 20 rows
        // At the end: the latest's start is on screen.
        assert_eq!(pin_of(&turns, row, 170, 200), None);
        // Partway down the latest: it's pinned as the latest.
        assert_eq!(pin_of(&turns, row, 185, 200), Some((9, "latest ")));
        // Mid-way: the current one.
        assert_eq!(pin_of(&turns, row, 105, 125), Some((5, "current")));
        // At the top: nothing.
        assert_eq!(pin_of(&turns, row, 0, 20), None);
        // A turn that isn't yours is never pinned.
        let mut t2 = turns.clone();
        t2[5].from = "Opus".into();
        assert_eq!(pin_of(&t2, row, 105, 125), Some((4, "current")));
    }

    #[test]
    fn folds_pastes() {
        let t = turn(
            1,
            "look at <pasted_content>one two</pasted_content> please",
            "",
        );
        assert_eq!(
            pin_text(&t),
            "look at [pasted text #1 · 1 line: one two] please"
        );
        let l = pin_row("latest ", "hello", 30);
        assert!(l.plain().starts_with("▍ latest  hello") && l.width() == 30);
    }
}
