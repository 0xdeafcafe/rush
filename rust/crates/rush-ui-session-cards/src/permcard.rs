//! A tool call waiting for your yes gets a framed card: what it asks in
//! the top edge, with where it runs and how many wait; the command, the
//! file's change or the address in the middle; and buttons to allow it
//! once, always, switch the session to don't-ask, or deny it. y, a, o and
//! n press them; ←→ pick one and enter presses it; a click presses the
//! one under it. After Go's internal/ui/permcard.go (permBtns :20,
//! alwaysLabel :47, permTitle :53, permissionCard :67, shellLines :140)
//! and rushmode.go (approvalBody :3706, approvalRows :3704, alwaysWords
//! :5948).

use rush_convo::Step;
use rush_core::text::{Color, Line, width};
use rush_core::tool::Kind;

use super::card::*;
use super::permissions::bypass_mode;
use super::{Conn, HostInfo};
use crate::model::Model;
use crate::style::*;

/// As much of a command as a permission card shows (approvalRows).
pub const APPROVAL_ROWS: usize = 10;

/// The permission card's buttons for st, w wide at most (permBtns).
pub fn perm_btns(m: &Model, c: &HostInfo, st: &Step, w: usize) -> Vec<Btn> {
    let b = |k: &str, s: &str, n: &str| [k.to_string(), s.to_string(), n.to_string()];
    let mut bs = vec![b("y", "y", "Allow once")];
    if let Some(a) = &st.approval
        && a.always
    {
        bs.push(b("a", "a", &always_label(&a.always_as)));
    }
    let mode = bypass_mode(m, &c.kind, Some(c));
    if !mode.is_empty() && c.permission_mode != mode {
        bs.push(b("o", "o", "Yolo"));
    }
    bs.push(b("n", "n", "Deny"));
    // Too wide, and always allow's rule goes first.
    let all = |bs: &[Btn]| -> usize {
        bs.iter()
            .map(|b| width(&format!("{} {}", b[1], b[2])) + 7)
            .sum()
    };
    for i in 0..bs.len() {
        let over = all(&bs) as isize - w as isize;
        if bs[i][0] == "a" && over > 0 {
            let keep = "Always allow"
                .len()
                .max(width(&bs[i][2]).saturating_sub(over as usize));
            bs[i][2] = rush_draw::truncate(&bs[i][2], keep, "…").into_owned();
        }
    }
    bs
}

/// Always allow's button: "Always allow Bash(pnpm lint:*)" (alwaysLabel).
pub fn always_label(r#as: &str) -> String {
    let w = always_words(r#as);
    let mut cs = w.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().chain(cs).collect(),
        None => w,
    }
}

/// The always key's label, from what the agent says allowing for good
/// does: "always allow Bash(pnpm lint:*)", "allow for this session"
/// (alwaysWords).
pub fn always_words(r#as: &str) -> String {
    if let Some(rest) = r#as.strip_prefix("Always allow") {
        return format!("always allow{rest}");
    }
    if let Some(rest) = r#as.strip_prefix("Yes, ") {
        return format!("allow {rest}");
    }
    "always allow".into()
}

/// What the card asks (permTitle).
pub fn perm_title(st: &Step) -> String {
    match st.call().kind {
        Kind::Shell => "Run this command?".into(),
        Kind::Edit | Kind::Write | Kind::Notebook => "Change this file?".into(),
        Kind::Fetch | Kind::WebSearch => "Go online?".into(),
        _ => format!("Use {}?", st.tool),
    }
}

/// The card for the tool call p[0] waits on, w wide and in at most max_h
/// rows, and the buttons on it, from the card's top (permissionCard).
#[allow(clippy::too_many_arguments)]
pub fn permission_card(
    m: &Model,
    s: &rush_convo::Session,
    c: &HostInfo,
    k: &Conn,
    p: &[rush_convo::StepId],
    cwd: &str,
    w: usize,
    max_h: usize,
) -> (Vec<Line>, Vec<CardBtn>) {
    let st = s.step(p[0]);
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

    // The top edge: what it asks, then whose it is, where it runs and how
    // many wait.
    let left = line([
        e("╭─ ".into()),
        sp("●", YELLOW),
        plain(" "),
        spb(perm_title(st), TEXT),
        plain(" "),
    ]);
    let mut meta: Vec<rush_core::text::Span> = Vec::new();
    let push_meta = |meta: &mut Vec<rush_core::text::Span>, sp_: rush_core::text::Span| {
        if !meta.is_empty() {
            meta.push(sp(" · ", DIM));
        }
        meta.push(sp_);
    };
    if let Some(under) = st.parent {
        // A subagent's: say whose, since it's not the agent you're talking to.
        let uid = s.step(under).id.to_string();
        let mut who = "a subagent".to_string();
        for sa in &c.subs {
            if sa.tool_use_id == uid {
                who = format!("{} · {}", sa.r#type, one_line(&sa.description))
                    .trim()
                    .to_string();
            }
        }
        push_meta(&mut meta, sp(format!("⇉ from {who}"), BLUE));
    }
    if st.call().kind == Kind::Shell && !cwd.is_empty() {
        push_meta(
            &mut meta,
            sp(format!("in {}", short_dir(&tildify(cwd), 36)), SUB),
        );
    }
    if p.len() > 1 {
        push_meta(&mut meta, sp(format!("1 of {}", p.len()), SUB));
    }
    let mut right = Line::new();
    if !meta.is_empty() {
        right.spans.push(plain(" "));
        right.spans.extend(meta);
        right.spans.push(plain(" "));
        let room = w as isize - left.width() as isize - 4;
        if right.width() as isize > room {
            right = rush_draw::truncate_line(&right, room.max(0) as usize, "… ");
        }
    }
    let fill = (w as isize - left.width() as isize - right.width() as isize - 2).max(1) as usize;
    let mut top = left;
    top.spans.push(e("─".repeat(fill)));
    join(&mut top, right);
    top.spans.push(e("─╮".into()));
    out.push(on_bg(top, CARD_BG, w));
    row(&mut out, Line::new());

    // What it does: it keeps to max_h, the body giving way, the rest (9
    // rows) not.
    let mut body = approval_body(st, cwd, tw);
    let room = 3.max(max_h as isize - 9) as usize;
    if max_h > 0 && body.len() > room {
        let more = body.len() - room + 1;
        body.truncate(room - 1);
        body.push(line([sp(
            format!("  … {more} more lines · ctrl+o shows all"),
            DIM,
        )]));
    }
    for l in body {
        row(&mut out, l);
    }
    row(&mut out, Line::new());

    let bs = perm_btns(m, c, st, w.saturating_sub(6));
    let pick = k.btn_pick.min(bs.len().saturating_sub(1));
    let (rows, btns) = card_buttons(&bs, k.card_focus, pick, &k.sub_hover, out.len());
    for r in rows {
        row(&mut out, r);
    }

    // The bottom edge says how to get to it, or back from it, and that
    // what you type is why not.
    let hint = if k.card_focus {
        " ←→ pick · enter · esc back to typing "
    } else {
        " ↑ to answer · or type why not and enter "
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

/// What the card shows of a call: the command a step to a line, the file
/// and what changes in it, the address or the query, and why the agent
/// asks when that says more (approvalBody).
pub fn approval_body(st: &Step, cwd: &str, w: usize) -> Vec<Line> {
    let c = st.call();
    let i = &c.input;
    let rel = |p: &str| -> String {
        for base in [cwd.to_string(), format!("/private{cwd}")] {
            if !cwd.is_empty()
                && let Some(r) = p.strip_prefix(&format!("{base}/"))
            {
                return r.to_string();
            }
        }
        tildify(p)
    };
    let path = if matches!(c.kind, Kind::Search | Kind::Glob) {
        "" // where it looks, not what it's about
    } else {
        i.path.as_str()
    };
    let mut out: Vec<Line> = Vec::new();
    if !i.command.is_empty() {
        // All of it, wrapped, so you see what you allow; past a screenful,
        // the step's ctrl+o has the rest.
        let mut cmd = i.command.clone();
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy();
            if !home.is_empty() {
                cmd = cmd.replace(&format!("{home}/"), "~/");
            }
        }
        // A step of a chain to a line, a long one wrapped in under it.
        let mut rows: Vec<Line> = Vec::new();
        for (n, step) in shell_lines(&cmd).iter().enumerate() {
            for (j, l) in wrap(step, w.saturating_sub(6)).into_iter().enumerate() {
                rows.push(match (n, j) {
                    (0, 0) => line([sp("$ ", BRIGHT), sp(l, TEXT)]),
                    (_, 0) => line([plain("  "), sp(l, TEXT)]),
                    _ => line([plain("    "), sp(l, TEXT)]),
                });
            }
        }
        let total = rows.len();
        for (n, l) in rows.into_iter().enumerate() {
            if n == APPROVAL_ROWS {
                out.push(line([sp(
                    format!("  … {} more lines · ctrl+o shows all", total - n),
                    DIM,
                )]));
                break;
            }
            out.push(l);
        }
    } else if !path.is_empty() {
        out.push(line([sp(rel(path), TEXT)]));
        // An edit shows what it changes, so you can judge it here.
        let add = |out: &mut Vec<Line>, prefix: &str, col: Color, text: &str, limit: usize| {
            let ls: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
            for (n, l) in ls.iter().enumerate() {
                if n >= limit {
                    out.push(line([sp(format!("  … {} more", ls.len() - n), DIM)]));
                    return;
                }
                let l = l.replace('\t', "  ");
                out.push(line([
                    sp(format!("{prefix} "), col),
                    sp(
                        rush_draw::truncate(&l, w.saturating_sub(4), "…").into_owned(),
                        TEXT,
                    ),
                ]));
            }
        };
        if i.edits.first().is_some_and(|e| !e.old.is_empty()) {
            add(&mut out, "−", RED, &i.edits[0].old, 4);
            add(&mut out, "+", GREEN, &i.edits[0].new, 4);
        } else if !i.content.is_empty() {
            add(&mut out, "+", GREEN, &i.content, 4);
        }
    } else if !i.url.is_empty() {
        out.push(line([sp(i.url.clone(), TEXT)]));
    } else if !i.query.is_empty() {
        out.push(line([sp(i.query.clone(), TEXT)]));
    }
    let why = st
        .approval
        .as_ref()
        .map(|a| one_line(&strip_ansi(&a.reason)))
        .unwrap_or_default();
    let said: Vec<String> = out.iter().map(Line::plain).collect();
    if !why.is_empty() && !said.join(" ").contains(&why) && !path.ends_with(&why) {
        out.push(line([sp(
            rush_draw::truncate(&why, w, "…").into_owned(),
            DIM,
        )]));
    }
    out
}

/// A command a step per line, so a chain reads down rather than wrapping
/// across: it breaks after &&, ||, ; and | outside quotes and brackets,
/// and at the command's own line breaks (shellLines).
pub fn shell_lines(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let l = cur.trim();
        if !l.is_empty() {
            out.push(l.to_string());
        }
        cur.clear();
    };
    let rs: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < rs.len() {
        let r = rs[i];
        if r == '\\' && quote != Some('\'') && i + 1 < rs.len() && rs[i + 1] == '\n' {
            i += 2;
            continue; // a continued line goes on as one
        }
        cur.push(r);
        if r == '\\' && quote != Some('\'') && i + 1 < rs.len() {
            i += 1;
            cur.push(rs[i]);
        } else if let Some(q) = quote {
            if r == q {
                quote = None;
            }
        } else if r == '\'' || r == '"' || r == '`' {
            quote = Some(r);
        } else if r == '(' || r == '{' {
            depth += 1;
        } else if (r == ')' || r == '}') && depth > 0 {
            depth -= 1;
        } else if r == '\n' {
            flush(&mut cur, &mut out);
        } else if depth > 0 {
        } else if (r == '&' || r == '|') && i + 1 < rs.len() && rs[i + 1] == r {
            cur.push(rs[i + 1]);
            i += 1;
            flush(&mut cur, &mut out);
        } else if r == ';' || r == '|' && (i == 0 || rs[i - 1] != '>') {
            flush(&mut cur, &mut out);
        }
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}
