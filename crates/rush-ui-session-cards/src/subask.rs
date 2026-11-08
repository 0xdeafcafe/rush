//! A subagent the session started waiting on you gets its card in the
//! session's dock, as the session's own asks do, rather than a row of
//! its own in the list: who asks, what (a command a step to a line), and
//! buttons to allow it once, deny it, or open the subagent. y, n and o
//! press them, ←→ pick one and enter presses it, a click presses the one
//! under it; with the box typed in, enter denies it, telling it why.
//! What rush can't answer for it (a question, or an agent rush doesn't
//! host) has only the button to open it.
//!
//! Rust-only: there's no Go for it. Its look is the permission card's
//! (permcard.rs, after Go's permcard.go).

use rush_core::text::{Color, Line, width};

use super::card::*;
use super::permcard::{APPROVAL_ROWS, shell_lines};
use super::{Conn, SubAsk};
use crate::style::*;

/// The card's buttons for a (permBtns' look): allow and deny only when
/// rush can answer it.
pub fn btns(a: &SubAsk) -> Vec<Btn> {
    let b = |k: &str, n: &str| [k.to_string(), k.to_string(), n.to_string()];
    let mut bs = Vec::new();
    if a.answers() {
        bs.push(b("y", "Allow once"));
        bs.push(b("n", "Deny"));
    }
    bs.push(b("o", "Open it"));
    bs
}

/// The keys of a's buttons, in order.
pub fn btn_keys(a: &SubAsk) -> Vec<String> {
    btns(a).into_iter().map(|b| b[0].clone()).collect()
}

/// The call a's host says it asks about, split: its tool, what it does,
/// and where ("Bash: cargo test (in /src/app)").
pub fn split_call(call: &str) -> (&str, &str, &str) {
    let (tool, what) = call.split_once(": ").unwrap_or(("", call));
    let mut what = what.trim();
    let mut cwd = "";
    if what.ends_with(')')
        && let Some(i) = what.rfind(" (in ")
    {
        cwd = &what[i + 5..what.len() - 1];
        what = what[..i].trim_end();
    }
    (tool, what, cwd)
}

/// Whether a tool by this name runs a command.
fn shell_tool(tool: &str) -> bool {
    let t = tool.to_ascii_lowercase();
    ["bash", "shell", "exec", "command"]
        .iter()
        .any(|w| t.contains(w))
}

/// What the card asks (permTitle's words).
pub fn title(a: &SubAsk) -> String {
    if !a.answers() {
        return "A subagent needs you".into();
    }
    let (tool, _, _) = split_call(&a.call);
    let t = tool.to_ascii_lowercase();
    if shell_tool(tool) {
        "Let it run this command?".into()
    } else if ["edit", "write", "patch"].iter().any(|w| t.contains(w)) {
        "Let it change this file?".into()
    } else if ["fetch", "search", "web"].iter().any(|w| t.contains(w)) {
        "Let it go online?".into()
    } else if tool.is_empty() {
        "Let it go on?".into()
    } else {
        format!("Let it use {tool}?")
    }
}

/// What the card shows of what a asks, tw wide.
fn body(a: &SubAsk, tw: usize) -> Vec<Line> {
    let mut out = Vec::new();
    if !a.answers() {
        let needs = one_line(&a.needs);
        if !needs.is_empty() {
            for l in wrap(&needs, tw) {
                out.push(line([sp(l, TEXT)]));
            }
        }
        out.push(line([sp(
            "rush can't answer this for it: open it to answer",
            DIM,
        )]));
        return out;
    }
    let (tool, what, _) = split_call(&a.call);
    let mut rows = Vec::new();
    if shell_tool(tool) {
        for (n, step) in shell_lines(what).iter().enumerate() {
            for (j, l) in wrap(step, tw.saturating_sub(4)).into_iter().enumerate() {
                rows.push(match (n, j) {
                    (0, 0) => line([sp("$ ", BRIGHT), sp(l, TEXT)]),
                    (_, 0) => line([plain("  "), sp(l, TEXT)]),
                    _ => line([plain("    "), sp(l, TEXT)]),
                });
            }
        }
    } else {
        if !tool.is_empty() {
            rows.push(line([sp(tool.to_string(), BRIGHT)]));
        }
        for l in what.lines() {
            for p in wrap(l, tw) {
                rows.push(line([sp(p, TEXT)]));
            }
        }
    }
    let total = rows.len();
    for (n, l) in rows.into_iter().enumerate() {
        if n == APPROVAL_ROWS {
            out.push(line([sp(format!("  … {} more lines", total - n), DIM)]));
            break;
        }
        out.push(l);
    }
    let why = one_line(&strip_ansi(&a.reason));
    if !why.is_empty() {
        out.push(line([sp(
            rush_draw::truncate(&why, tw, "…").into_owned(),
            DIM,
        )]));
    }
    out
}

/// The card for asks[0], w wide and in at most max_h rows, and its
/// buttons, from the card's top (permissionCard's frame).
pub fn card(k: &Conn, asks: &[SubAsk], w: usize, max_h: usize) -> (Vec<Line>, Vec<CardBtn>) {
    let a = &asks[0];
    let edge: Color = if k.card_focus { ORANGE } else { YELLOW };
    let e = |t: String| sp(t, edge);
    let mut out: Vec<Line> = Vec::new();
    let row = |out: &mut Vec<Line>, l: Line| {
        let mut r = line([e("│".into())]);
        let mut inner = line([plain("  ")]);
        join(&mut inner, l);
        join(&mut r, fit(inner, w.saturating_sub(2)));
        r.spans.push(e("│".into()));
        out.push(on_bg(r, CARD_BG, w));
    };
    let tw = 20.max(w.saturating_sub(6));

    // The top edge: what it asks, then which subagent, where and how many
    // wait.
    let left = line([
        e("╭─ ".into()),
        sp("●", YELLOW),
        plain(" "),
        spb(title(a), TEXT),
        plain(" "),
    ]);
    let mut meta: Vec<rush_core::text::Span> = vec![sp(format!("⇉ from {}", a.who), BLUE)];
    let (_, _, cwd) = split_call(&a.call);
    if !cwd.is_empty() && a.answers() {
        meta.push(sp(" · ", DIM));
        meta.push(sp(format!("in {}", short_dir(&tildify(cwd), 36)), SUB));
    }
    if asks.len() > 1 {
        meta.push(sp(" · ", DIM));
        meta.push(sp(format!("1 of {}", asks.len()), SUB));
    }
    let mut right = Line::new();
    right.spans.push(plain(" "));
    right.spans.extend(meta);
    right.spans.push(plain(" "));
    let room = w as isize - left.width() as isize - 4;
    if right.width() as isize > room {
        right = rush_draw::truncate_line(&right, room.max(0) as usize, "… ");
    }
    let fill = (w as isize - left.width() as isize - right.width() as isize - 2).max(1) as usize;
    let mut top = left;
    top.spans.push(e("─".repeat(fill)));
    join(&mut top, right);
    top.spans.push(e("─╮".into()));
    out.push(on_bg(top, CARD_BG, w));
    row(&mut out, Line::new());

    let mut body = body(a, tw);
    let room = 3.max(max_h as isize - 9) as usize;
    if max_h > 0 && body.len() > room {
        let more = body.len() - room + 1;
        body.truncate(room - 1);
        body.push(line([sp(format!("  … {more} more lines"), DIM)]));
    }
    for l in body {
        row(&mut out, l);
    }
    row(&mut out, Line::new());

    let bs = btns(a);
    let pick = k.btn_pick.min(bs.len().saturating_sub(1));
    let (rows, btns) = card_buttons(&bs, k.card_focus, pick, &k.sub_hover, out.len());
    for r in rows {
        row(&mut out, r);
    }

    let hint = if k.card_focus {
        " ←→ pick · enter · esc back to typing "
    } else if a.answers() {
        " ↑ to answer · or type why not and enter "
    } else {
        " ↑ to open it "
    };
    let n = (w as isize - 3 - width(hint) as isize).max(1) as usize;
    out.push(on_bg(
        line([
            e(format!("╰{}", "─".repeat(n))),
            sp(hint, DIM),
            e("─╯".into()),
        ]),
        CARD_BG,
        w,
    ));
    (out, btns)
}
