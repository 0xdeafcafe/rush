//! Claude's questions as one form, a decisions block as the same card,
//! and the keys and clicks that answer whichever card waits. After Go's
//! internal/ui/question.go (questionCard :50, drawQuestion :73, ownAnswer
//! :373, splitAsk :389, questionStrip :414, recWord :475, escWord :483,
//! clickQuestion :546), rushmode.go (question :5328, isQuestion :5335,
//! decisionsCard :5341, questions :5424, syncQuestion :5432, pickedAt
//! :5446, picks :5455, questionKey :5476, goQuestion :5617,
//! answerQuestion :5662, sendAnswers :5679, dismissDecisions :5700,
//! cardKind :5718, cardID :5737, cardKey :5754, cardRows :2948,
//! answerHost :4424, allowYolo :4434), modal.go:32 (cardModal's arrival),
//! host/decisions.go (DecisionsOf :377, DecisionAnswer :400) and
//! host/answer.go (AnswerInput).

use std::collections::HashMap;

use rush_core::event::{Ask, Choice, Question};
use rush_core::text::{Color, Line, Span, Style, width};
use serde_json::{Map, Value, json};

use super::card::*;
use super::permcard::{perm_btns, permission_card};
use super::permissions::{bypass_mode, set_permission};
use super::{Act, Conn, HostInfo, act, info, open_key, pending, session, set_input};
use crate::frame::overlayclick::hint_key;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// One question of those the agent asks (question).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Q {
    pub question: String,
    pub header: String,
    pub multi: bool,
    pub options: Vec<Choice>,
}

/// req's title and its questions (questions).
pub fn questions(req: &Question) -> (String, Vec<Q>) {
    let qs = req
        .asks
        .iter()
        .map(|a| Q {
            question: a.text.clone(),
            header: a.header.clone(),
            multi: a.multi,
            options: a.options.clone(),
        })
        .collect();
    (req.title.clone(), qs)
}

/// The option the agent picked, else the first (pickedAt).
pub fn picked_at(opts: &[Choice]) -> usize {
    opts.iter().position(|o| o.picked).unwrap_or(0)
}

impl Conn {
    /// Starts answering afresh when a new question comes (syncQuestion);
    /// decisions is the session's open decisions block.
    pub fn sync_question(&mut self, req: &Question, decisions: Option<&Question>) {
        if self.q_for == req.id {
            return;
        }
        self.q_for = req.id.clone();
        self.q_idx = 0;
        self.q_cursor = 0;
        self.q_picks.clear();
        self.q_answer.clear();
        self.q_click = 0;
        self.q_typed.clear();
        self.q_full = false;
        self.q_own = false;
        self.q_decide = self.q_again.as_ref().is_some_and(|q| q.id == req.id)
            || decisions.is_some_and(|d| d.id == req.id);
        if let Some(a) = req.asks.first() {
            self.q_cursor = picked_at(&a.options);
        }
    }

    /// What's ticked on multi-select question i (picks).
    pub fn picks(&mut self, i: usize) -> &mut HashMap<usize, bool> {
        self.q_picks.entry(i).or_default()
    }

    fn picked(&self, i: usize, o: usize) -> bool {
        self.q_picks
            .get(&i)
            .and_then(|p| p.get(&o))
            .copied()
            .unwrap_or(false)
    }
}

/// The decisions block the agent's last turn ended with, while nothing
/// else waits on you and you haven't answered or set it aside
/// (decisionsCard).
pub fn decisions_card(c: &Conn, i: &HostInfo, pending: usize) -> Option<Question> {
    let d = c.q_again.clone().or_else(|| i.decisions.clone())?;
    if pending > 0 || d.id == c.q_gone {
        return None;
    }
    Some(d)
}

/// The kind of card waiting: "question", "approval", "decisions" or ""
/// (cardKind; the limit, sign-in and finish cards are other screens').
pub fn card_kind(m: &Model, key: &str, c: &Conn, i: &HostInfo) -> &'static str {
    let p = pending(m, key);
    if let Some(&sid) = p.first() {
        let s = session(m, key).expect("pending steps are the open session's");
        if s.step(sid)
            .approval
            .as_ref()
            .is_some_and(|a| a.question.is_some())
        {
            return "question";
        }
        return "approval";
    }
    if decisions_card(c, i, 0).is_some() {
        return "decisions";
    }
    ""
}

/// Names the card waiting, to tell a new one from the last (cardID).
pub fn card_id(m: &Model, key: &str, c: &Conn, i: &HostInfo) -> String {
    match card_kind(m, key, c, i) {
        "question" | "approval" => {
            let s = session(m, key).expect("open");
            let p = s.pending();
            s.step(p[0])
                .approval
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_default()
        }
        "decisions" => decisions_card(c, i, 0).map(|d| d.id).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Whether a card of kind takes the keys only from ↑ or a click: a
/// question's and decisions' letters and digits answer, so the start of a
/// message would (upOnly).
fn up_only(kind: &str) -> bool {
    matches!(kind, "finish" | "question" | "decisions")
}

/// The cards waiting on you, w wide and a question folded to fit max_h
/// rows; empty when nothing waits (cardRows). A card just come takes the
/// keys unless you were typing or it's up-only (cardModal).
pub fn card_rows(m: &mut Model, key: &str, i: &HostInfo, w: usize, max_h: usize) -> Vec<Line> {
    let mut c = take(m, key);
    let id = card_id(m, key, &c, i);
    if id != c.card_shown {
        c.card_shown = id.clone();
        c.btn_pick = 0;
        let now = m.clock.unwrap_or_else(rush_core::time::now);
        c.card_typing = now.saturating_sub(m.cards().last_key) < 1_000;
        c.card_at = now;
        // What waits on you takes the keys as it comes, unless you were
        // typing: then ↑ gives it them (modal.go:32 cardModal).
        if !id.is_empty()
            && !up_only(card_kind(m, key, &c, i))
            && !c.card_typing
            && i.input.is_empty()
        {
            c.card_focus = true;
            if let Some(v) = m.session.as_mut() {
                v.sel.clear();
            }
        }
    }
    c.btns.clear();
    c.q_rows.clear();
    c.q_hit.clear();
    let out = match card_kind(m, key, &c, i) {
        "question" => {
            let s = session(m, key).expect("open");
            let sid = s.pending()[0];
            let req = s
                .step(sid)
                .approval
                .as_ref()
                .and_then(|a| a.question.clone());
            match req {
                Some(req) => question_card(&mut c, &req, i.decisions.as_ref(), w, max_h),
                None => Vec::new(),
            }
        }
        "decisions" => match decisions_card(&c, i, 0) {
            Some(d) => question_card(&mut c, &d, i.decisions.as_ref(), w, max_h),
            None => Vec::new(),
        },
        "approval" => {
            let s = session(m, key).expect("open");
            let p = s.pending();
            let cwd = agent_cwd(m, key, i);
            let (rows, btns) = permission_card(m, s, i, &c, &p, &cwd, w, max_h);
            c.btns = btns;
            rows
        }
        _ => Vec::new(),
    };
    put(m, key, c);
    out
}

/// Where agent key works: the list's word, else the session's.
fn agent_cwd(m: &Model, key: &str, i: &HostInfo) -> String {
    m.agents
        .iter()
        .find(|a| a.key == key)
        .map(|a| a.job.cwd.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| i.cwd.clone())
}

fn take(m: &mut Model, key: &str) -> Conn {
    m.cards().conns.remove(key).unwrap_or_default()
}

fn put(m: &mut Model, key: &str, c: Conn) {
    m.cards().conns.insert(key.to_string(), c);
}

/// Claude's questions drawn as one form: a heading, the question as
/// prose with the ask set apart, each option as a block (a numbered
/// keycap, its label and its description), and beside them the preview
/// of the option under the cursor. It keeps to max_h rows by shortening
/// the options not under the cursor first, then the question's lead-in
/// (questionCard).
pub fn question_card(
    c: &mut Conn,
    req: &Question,
    decisions: Option<&Question>,
    w: usize,
    max_h: usize,
) -> Vec<Line> {
    c.sync_question(req, decisions);
    let (title, qs) = questions(req);
    if qs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for fold in 0..=3 {
        out = draw_question(c, &title, &qs, w, fold, max_h);
        if max_h == 0 || out.len() <= max_h {
            break;
        }
    }
    c.q_rows = out.clone();
    out
}

/// One option row of the list, and what a click on it presses.
struct ORow {
    text: Line,
    sel: bool,
    hit: String,
}

/// The card, folded: 0 shows everything, and each step up shows less of
/// what isn't under the cursor. In full view it's the option under the
/// cursor alone. It notes in q_hit what a click on each row presses
/// (drawQuestion).
pub fn draw_question(
    c: &mut Conn,
    title: &str,
    qs: &[Q],
    w: usize,
    fold: usize,
    max_h: usize,
) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    c.q_hit.clear();
    let edge = sp("▍", if c.card_focus { ORANGE } else { YELLOW });
    let cl = |out: &mut Vec<Line>, txt: Line| {
        let mut l = line([edge.clone()]);
        join(&mut l, txt);
        out.push(on_bg(l, CARD_BG, w));
    };
    let review = c.q_idx >= qs.len();
    let text_w = 20.max(w.saturating_sub(8).min(108));

    // The heading: what it's about, and where you are among several.
    let mut head = Line::new();
    if c.q_decide && qs.len() == 1 && !qs[0].header.is_empty() {
        head.spans.extend([
            chip(&format!("? {}", one_line(&qs[0].header))),
            plain("  "),
            sp("yours to decide", SUB),
        ]);
    } else if c.q_decide {
        head.spans.extend([
            chip("? decide"),
            plain("  "),
            sp(format!("{} decisions, yours to make", qs.len()), SUB),
        ]);
    } else if qs.len() == 1 && !qs[0].header.is_empty() {
        head.spans.extend([
            chip(&format!("? {}", one_line(&qs[0].header))),
            plain("  "),
            sp("Claude asks", SUB),
        ]);
    } else {
        head.spans
            .extend([chip("?"), plain(" "), spb("Claude asks", YELLOW)]);
    }
    if !title.is_empty() {
        head.spans.extend([
            plain("  "),
            sp("·", DIM),
            plain("  "),
            sp(one_line(title), TEXT),
        ]);
    }
    let mut right = Line::new();
    if qs.len() > 1 {
        right.spans.push(sp(
            format!("{} of {} answered", c.q_answer.len(), qs.len()),
            SUB,
        ));
    }
    right.spans.push(plain("  "));
    let mut h = line([plain(" ")]);
    join(&mut h, head);
    cl(&mut out, spread(h, right, w.saturating_sub(1)));
    if qs.len() > 1 {
        cl(&mut out, Line::new());
        let mut l = line([plain("  ")]);
        join(&mut l, question_strip(c, qs, w.saturating_sub(4)));
        cl(&mut out, l);
    }
    cl(&mut out, Line::new());

    if review {
        let names: Vec<String> = qs
            .iter()
            .enumerate()
            .map(|(i, q)| first_non_empty(&[&q.header, &format!("Question {}", i + 1)]).to_string())
            .collect();
        let label_w = names.iter().map(|n| width(n)).max().unwrap_or(0).min(24);
        let (ask, send, change) = if qs.len() == 1 {
            ("Send this answer?", " ⏎ Send ", "←".to_string())
        } else {
            (
                "Send these answers?",
                " ⏎ Send answers ",
                format!("← · 1–{}", qs.len()),
            )
        };
        cl(&mut out, line([plain("  "), spb(ask, TEXT)]));
        cl(&mut out, Line::new());
        for (i, q) in qs.iter().enumerate() {
            let name = rush_draw::truncate(&names[i], label_w, "…").into_owned();
            let a = match c.q_answer.get(&q.question) {
                Some(ans) => line([
                    sp("✓ ", GREEN),
                    spb(
                        rush_draw::truncate(
                            &shown_answer(ans),
                            10.max(w.saturating_sub(label_w + 16)),
                            "…",
                        )
                        .into_owned(),
                        TEXT,
                    ),
                ]),
                None => line([sp("○ not answered", YELLOW)]),
            };
            c.q_hit.insert(out.len(), (i + 1).to_string());
            let pad = label_w - width(&name) + 3;
            let mut l = line([
                plain("  "),
                keycap(&(i + 1).to_string(), false),
                plain("  "),
                sp(name, SUB),
                plain(blanks(pad)),
            ]);
            join(&mut l, a);
            cl(&mut out, l);
        }
        cl(&mut out, Line::new());
        // The button enter presses: lit while the card has the keys.
        c.q_hit.insert(out.len(), "enter".into());
        let face = if c.card_focus { TAB_ON } else { TAB_OFF };
        cl(&mut out, line([plain("  "), Span::new(send, face)]));
        cl(&mut out, Line::new());
        let ww = w.saturating_sub(6);
        let esc = esc_word();
        let hint = if c.card_focus && c.q_decide && qs.len() == 1 {
            keys_fit(
                ww,
                &[
                    ("enter", "sends it"),
                    (&change, "changes it"),
                    ("s", "sends nothing"),
                    ("esc", esc),
                ],
            )
        } else if c.card_focus && c.q_decide {
            keys_fit(
                ww,
                &[
                    ("enter", "sends your answers"),
                    (&change, "changes one"),
                    ("s", "sends nothing"),
                    ("esc", esc),
                ],
            )
        } else if c.card_focus {
            keys_fit(
                ww,
                &[
                    ("enter", "sends your answers"),
                    (&change, "changes one"),
                    ("s", "skips them all"),
                    ("esc", esc),
                ],
            )
        } else if qs.len() == 1 {
            line([sp("↑ to send or change your answer", DIM)])
        } else {
            line([sp("↑ to send or change your answers", DIM)])
        };
        let mut l = line([plain("  ")]);
        join(&mut l, hint);
        cl(&mut out, l);
        return out;
    }

    // The question: a lead-in as prose, then the ask itself in bold.
    let q = &qs[c.q_idx];
    let (mut lead, mut ask) = split_ask(&q.question);
    if c.q_decide
        && let Some(i) = q.question.rfind("\n\n")
        && i > 0
    {
        // What's at stake, then the decision.
        lead = q.question[..i].trim().to_string();
        ask = q.question[i + 2..].trim().to_string();
    }
    if !lead.is_empty() {
        let mut lines = inline_wrap(&lead, Style::fg(TEXT), text_w);
        if fold >= 3 && lines.len() > 4 {
            lines.truncate(4);
            lines[3].spans.push(sp(" …", TEXT));
        }
        for l in lines {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            cl(&mut out, r);
        }
        cl(&mut out, Line::new());
    }
    if !c.q_decide || qs.len() > 1 {
        // One decision's ask is the card's title.
        for l in inline_wrap(&ask, Style::fg(TEXT).bold(), text_w) {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            cl(&mut out, r);
        }
        cl(&mut out, Line::new());
    }

    if c.q_full && c.card_focus && c.q_cursor < q.options.len() {
        let o = &q.options[c.q_cursor];
        let (label, rec) = option_label(&o.label);
        let mut name = line([
            keycap(&(c.q_cursor + 1).to_string(), true),
            plain(" "),
            spb(label.clone(), TEXT),
        ]);
        if rec || o.picked {
            name.spans.extend([plain("  "), sp(rec_word(c), GREEN)]);
        }
        c.q_hit.insert(out.len(), format!("@{}", c.q_cursor));
        let mut left = line([plain(" "), sp("▌", ORANGE), plain(" ")]);
        join(&mut left, name);
        let right = line([
            sp(format!("{} of {}", c.q_cursor + 1, q.options.len()), DIM),
            plain("  "),
        ]);
        cl(&mut out, spread(left, right, w.saturating_sub(1)));
        let d = o.description.trim();
        if !d.is_empty() {
            for l in inline_wrap(d, Style::fg(TEXT), text_w) {
                let mut r = line([plain("     ")]);
                join(&mut r, l);
                cl(&mut out, r);
            }
        }
        cl(&mut out, Line::new());
        if o.preview.is_empty() {
            cl(
                &mut out,
                line([plain("     "), sp("no preview for this option", DIM)]),
            );
        } else {
            let rows = if max_h > 0 {
                6.max(max_h as isize - out.len() as isize - 3) as usize
            } else {
                1 << 20
            };
            for r in preview_box(&label, &o.preview, w.saturating_sub(6), rows) {
                let mut l = line([plain("     ")]);
                join(&mut l, r);
                cl(&mut out, l);
            }
        }
        cl(&mut out, Line::new());
        let mut l = line([plain("  ")]);
        join(
            &mut l,
            keys_fit(
                w.saturating_sub(6),
                &[
                    ("↑↓", "other options"),
                    ("enter", "picks"),
                    ("v", "back to all"),
                    ("esc", "back to all"),
                ],
            ),
        );
        cl(&mut out, l);
        return out;
    }

    // The options, then the preview beside them when there's room, or
    // under them when there isn't.
    let cursor = if c.card_focus { Some(c.q_cursor) } else { None };
    let (preview, preview_of) = match q.options.get(c.q_cursor) {
        Some(o) => (o.preview.clone(), o.label.clone()),
        None => (String::new(), String::new()),
    };
    let side = !preview.is_empty() && w >= 96;
    let lw = if side {
        60.min(w.saturating_sub(1) / 2)
    } else {
        w.saturating_sub(1)
    };
    let mut rows: Vec<ORow> = Vec::new();
    let desc_w = 16.max(lw.saturating_sub(10).min(104));
    let prev = c.q_answer.get(&q.question).cloned().unwrap_or_default();
    let n_opts = q.options.len();
    for (i, o) in q.options.iter().enumerate() {
        let on = cursor == Some(i);
        let (label, rec) = option_label(&o.label);
        let bar = if on { sp("▌", ORANGE) } else { plain(" ") };
        let mut name = line([keycap(&(i + 1).to_string(), on), plain(" ")]);
        if q.multi {
            if c.picked(c.q_idx, i) {
                name.spans.push(sp("☑ ", GREEN));
            } else {
                name.spans.push(sp("☐ ", DIM));
            }
        }
        name.spans.push(if on {
            spb(label, TEXT)
        } else {
            sp(label, TEXT)
        });
        if rec || o.picked {
            name.spans.extend([plain("  "), sp(rec_word(c), GREEN)]);
        }
        if !q.multi && prev == o.label {
            name.spans.extend([plain("  "), sp("✓ your answer", GREEN)]);
        }
        if !o.preview.is_empty() && !side && o.preview != preview {
            name.spans.extend([plain("  "), sp("◇ preview", DIM)]);
        }
        let hit = format!("@{i}");
        let mut t = line([plain(" "), bar.clone(), plain(" ")]);
        join(&mut t, name);
        rows.push(ORow {
            text: t,
            sel: on,
            hit: hit.clone(),
        });
        let d = o.description.trim();
        if !d.is_empty() {
            let ink = if on { TEXT } else { SUB };
            let mut lines = inline_wrap(d, Style::fg(ink), desc_w);
            let keep = if on { 12 } else { [6, 2, 1, 0][fold] };
            if lines.len() > keep {
                lines.truncate(keep);
                if keep > 0 {
                    lines[keep - 1].spans.push(sp(" …", ink));
                }
            }
            for l in lines {
                let mut t = line([plain(" "), bar.clone(), plain("     ")]);
                join(&mut t, l);
                rows.push(ORow {
                    text: t,
                    sel: on,
                    hit: hit.clone(),
                });
            }
        }
        if fold < 2 {
            rows.push(ORow {
                text: Line::new(),
                sel: false,
                hit: String::new(),
            });
        }
    }
    let own = cursor == Some(n_opts);
    let bar = if own { sp("▌", ORANGE) } else { plain(" ") };
    let mut own_text = if own {
        line([
            spb("Something else", TEXT),
            plain("  "),
            sp("enter, then type it below", SUB),
        ])
    } else {
        line([
            sp("Something else", TEXT),
            plain("  "),
            sp("type your own answer below", DIM),
        ])
    };
    if own_answer(q, &prev) {
        own_text.spans.extend([
            plain("  "),
            sp("✓ ", GREEN),
            sp(
                rush_draw::truncate(&shown_answer(&prev), 8.max(lw.saturating_sub(40)), "…")
                    .into_owned(),
                TEXT,
            ),
        ]);
    }
    let mut t = line([plain(" "), bar, plain(" "), keycap("✎", own), plain(" ")]);
    join(&mut t, own_text);
    rows.push(ORow {
        text: t,
        sel: own,
        hit: format!("@{n_opts}"),
    });
    if q.multi {
        // Enter on an option only ticks it; moving on is this button.
        let btn = cursor == Some(n_opts + 1);
        let (bar, face) = if btn {
            (sp("▌", ORANGE), TAB_ON)
        } else {
            (plain(" "), TAB_OFF)
        };
        let n = c
            .q_picks
            .get(&c.q_idx)
            .map_or(0, |p| p.values().filter(|v| **v).count());
        let note = if n > 0 {
            sp(format!("with {n} ticked"), SUB)
        } else {
            sp("tick at least one first", DIM)
        };
        rows.push(ORow {
            text: Line::new(),
            sel: false,
            hit: String::new(),
        });
        rows.push(ORow {
            text: line([
                plain(" "),
                bar,
                plain(" "),
                Span::new(" ⏎ Continue ", face),
                plain("  "),
                note,
            ]),
            sel: btn,
            hit: format!("@{}", n_opts + 1),
        });
    }

    let mut pv: Vec<Line> = Vec::new();
    if !preview.is_empty() {
        let pw = if side {
            w.saturating_sub(1 + lw + 3)
        } else {
            w.saturating_sub(1 + 5)
        };
        let (name, _) = option_label(&preview_of);
        pv = preview_box(&name, &preview, pw, 14);
    }
    let mut n = rows.len();
    if side {
        n = n.max(pv.len());
    }
    for i in 0..n {
        let (left, sel) = match rows.get(i) {
            Some(r) => {
                if !r.hit.is_empty() {
                    c.q_hit.insert(out.len(), r.hit.clone());
                }
                (r.text.clone(), r.sel)
            }
            None => (Line::new(), false),
        };
        let bg: Color = if sel { CARD_SEL_BG } else { CARD_BG };
        if !side {
            let mut l = on_bg(left, bg, w.saturating_sub(3));
            l.spans.push(plain("  "));
            cl(&mut out, l);
            continue;
        }
        let mut l = on_bg(left, bg, lw);
        l.spans.push(plain("  "));
        if let Some(r) = pv.get(i) {
            join(&mut l, r.clone());
        }
        cl(&mut out, l);
    }
    if !side && !pv.is_empty() {
        cl(&mut out, Line::new());
        for r in pv {
            let mut l = line([plain("     ")]);
            join(&mut l, r);
            cl(&mut out, l);
        }
    }
    cl(&mut out, Line::new());
    let ww = w.saturating_sub(6);
    if c.card_focus {
        let range = format!("1–{n_opts}");
        let mut pairs: Vec<(&str, &str)> = if q.multi {
            vec![("↑↓", "choose"), ("enter", "ticks"), (&range, "tick one")]
        } else {
            vec![("↑↓", "choose"), ("enter", "picks"), (&range, "pick one")]
        };
        if qs.len() > 1 {
            pairs.push(("←→", "questions"));
        }
        if c.q_decide {
            // Its own keys first: they're what a narrow card keeps.
            let mut p = vec![("a", "takes its picks"), ("s", "sends nothing")];
            p.extend(pairs);
            pairs = p;
            if q.options.iter().any(|o| !o.preview.is_empty()) {
                pairs.push(("v", "full view"));
            }
        } else {
            pairs.extend([("v", "full view"), ("s", "skips"), ("esc", esc_word())]);
        }
        let mut l = line([plain("  ")]);
        join(&mut l, keys_fit(ww, &pairs));
        cl(&mut out, l);
    } else if c.q_decide {
        cl(
            &mut out,
            line([
                plain("  "),
                sp("↑", SUB),
                sp(" to decide   ·   a message typed below goes as ever", DIM),
            ]),
        );
    } else {
        cl(
            &mut out,
            line([
                plain("  "),
                sp("↑", SUB),
                sp(
                    " to choose   ·   or type your own answer below and press ",
                    DIM,
                ),
                sp("enter", SUB),
            ]),
        );
    }
    out
}

/// Whether a is an answer in your own words, not an option (ownAnswer).
pub fn own_answer(q: &Q, a: &str) -> bool {
    if a.is_empty() || q.multi {
        return false;
    }
    !q.options.iter().any(|o| o.label == a)
}

/// A question parted into its lead-in and the ask: the ask is the last
/// sentence when it's a question and there's something before it;
/// otherwise it's the whole text (splitAsk).
pub fn split_ask(q: &str) -> (String, String) {
    let q = q.trim();
    if !q.ends_with('?') {
        return (String::new(), q.to_string());
    }
    if let Some(i) = q.rfind("\n\n")
        && i > 0
    {
        return (q[..i].trim().to_string(), q[i + 2..].trim().to_string());
    }
    let b = q.as_bytes();
    let mut i = b.len().saturating_sub(2);
    while i > 0 {
        if b[i] != b' ' && b[i] != b'\n' || !b".!?".contains(&b[i - 1]) {
            i -= 1;
            continue;
        }
        let rest = q[i..].trim();
        if rest.chars().next().is_some_and(char::is_uppercase) {
            return (q[..i].trim().to_string(), rest.to_string());
        }
        break;
    }
    (String::new(), q.to_string())
}

/// A row naming every question: the one at hand lit, ✓ and its answer
/// once answered, ○ the rest, then send (questionStrip).
fn question_strip(c: &Conn, qs: &[Q], w: usize) -> Line {
    let mut l = Line::new();
    for (i, q) in qs.iter().enumerate() {
        if i > 0 {
            l.spans.push(sp("  ›  ", FAINT));
        }
        let name = first_non_empty(&[&q.header, &format!("Question {}", i + 1)]).to_string();
        if i == c.q_idx {
            l.spans.push(chip(&name));
        } else if let Some(ans) = c.q_answer.get(&q.question) {
            l.spans.extend([
                sp("✓ ", GREEN),
                sp(name, SUB),
                plain(" "),
                sp(
                    rush_draw::truncate(&shown_answer(ans), 18, "…").into_owned(),
                    DIM,
                ),
            ]);
        } else {
            l.spans.push(sp(format!("○ {name}"), DIM));
        }
    }
    l.spans.push(sp("  ›  ", FAINT));
    if c.q_idx >= qs.len() {
        l.spans.push(chip("send"));
    } else {
        l.spans.push(sp("○ send", DIM));
    }
    rush_draw::truncate_line(&l, w, "…")
}

/// Marks the option the agent recommends: on a decisions block, the one
/// it picked (recWord).
fn rec_word(c: &Conn) -> &'static str {
    if c.q_decide {
        "★ its pick"
    } else {
        "★ recommended"
    }
}

/// What esc does on a question: back to typing (escWord; the modal's
/// "later" is the modal's).
fn esc_word() -> &'static str {
    "type instead"
}

// ---- answering ----

/// What a key did on a question.
#[derive(Clone, Debug, PartialEq)]
pub enum Res {
    /// Not the card's.
    No,
    /// Taken, nothing sent.
    Yes,
    /// Taken, and this goes to the session: the card is done.
    Do(Act),
}

impl Res {
    fn used(&self) -> bool {
        *self != Res::No
    }
}

/// Answers Claude's questions (or a decisions block). While the card has
/// the keys ↑↓ choose, a digit or enter picks (ticks, on a multi-select,
/// whose Continue button confirms), ←→ move between questions; typed text
/// and enter answer in your own words. With several questions the last
/// step is a review, where enter sends. input is the message box's text,
/// changed as the card moves (questionKey).
pub fn question_key(
    c: &mut Conn,
    req: &Question,
    decisions: Option<&Question>,
    s: &str,
    empty: bool,
    input: &mut String,
) -> Res {
    c.sync_question(req, decisions);
    let (_, qs) = questions(req);
    if qs.is_empty() {
        return Res::No;
    }
    let digit = |s: &str| -> Option<usize> {
        let b = s.as_bytes();
        (b.len() == 1 && (b'1'..=b'9').contains(&b[0])).then(|| (b[0] - b'1') as usize)
    };
    if c.q_idx >= qs.len() {
        // The review: enter sends, a digit or ← goes back to one.
        if s == "enter" {
            return send_answers(c, req);
        }
        if s == "left" {
            go_question(c, &qs, qs.len() - 1, input);
            return Res::Yes;
        }
        if empty && let Some(i) = digit(s).filter(|&i| i < qs.len()) {
            go_question(c, &qs, i, input);
            return Res::Yes;
        }
        return Res::No;
    }
    let qi = c.q_idx;
    let q = qs[qi].clone();
    if c.card_focus && empty {
        match s {
            "up" => {
                if c.q_cursor == 0 {
                    return Res::No; // above the first option: off the card
                }
                c.q_cursor -= 1;
                c.q_click = 0;
                return Res::Yes;
            }
            "down" => {
                let last = q.options.len() + usize::from(q.multi); // the Continue button
                if c.q_cursor >= last {
                    return Res::No; // past the last row: back to the box
                }
                c.q_cursor += 1;
                c.q_click = 0;
                return Res::Yes;
            }
            "v" => {
                c.q_full = !c.q_full;
                return Res::Yes;
            }
            "esc" if c.q_full => {
                c.q_full = false;
                return Res::Yes;
            }
            "left" | "right" if qs.len() > 1 => {
                let to = c.q_idx as isize + if s == "right" { 1 } else { -1 };
                if to >= 0 {
                    go_question(c, &qs, to as usize, input);
                }
                return Res::Yes;
            }
            "space" if q.multi && c.q_cursor < q.options.len() => {
                let cur = c.q_cursor;
                let p = c.picks(qi);
                let v = !p.get(&cur).copied().unwrap_or(false);
                p.insert(cur, v);
                return Res::Yes;
            }
            "enter" => {
                if c.q_cursor == q.options.len() {
                    // "your own words": type in the box
                    c.card_focus = false;
                    c.q_own = true;
                    return Res::Yes;
                }
                if !q.multi {
                    let l = q.options[c.q_cursor].label.clone();
                    return answer_question(c, req, &qs, l, input);
                }
                if c.q_cursor < q.options.len() {
                    let cur = c.q_cursor;
                    let p = c.picks(qi);
                    let v = !p.get(&cur).copied().unwrap_or(false);
                    p.insert(cur, v);
                    return Res::Yes;
                }
                // On Continue: on to confirm what's ticked.
            }
            _ => {}
        }
    }
    if empty && let Some(i) = digit(s) {
        if i >= q.options.len() {
            return Res::Yes;
        }
        if q.multi {
            let p = c.picks(qi);
            let v = !p.get(&i).copied().unwrap_or(false);
            p.insert(i, v);
            return Res::Yes;
        }
        let l = q.options[i].label.clone();
        return answer_question(c, req, &qs, l, input);
    }
    if s == "enter" {
        if !empty {
            let text = input.trim().to_string();
            input.clear();
            if qs.len() == 1 {
                // Typed and sent: that's the confirming, so no review.
                c.q_answer.insert(q.question.clone(), text);
                return send_answers(c, req);
            }
            return answer_question(c, req, &qs, text, input);
        }
        if q.multi && (0..q.options.len()).any(|i| c.picked(qi, i)) {
            let labels: Vec<&str> = q
                .options
                .iter()
                .enumerate()
                .filter(|(i, _)| c.picked(qi, *i))
                .map(|(_, o)| o.label.as_str())
                .collect();
            let l = labels.join(", ");
            return answer_question(c, req, &qs, l, input);
        }
        return Res::Yes;
    }
    Res::No
}

/// Moves to question i (qs.len() is the review), the cursor on the option
/// already chosen there (goQuestion).
fn go_question(c: &mut Conn, qs: &[Q], mut i: usize, input: &mut String) {
    if qs.len() < 2 && !c.q_decide {
        i = i.min(qs.len() - 1); // a lone question of Claude's has no review
    }
    // What's typed goes with its question, and comes back with it.
    if c.q_idx < qs.len() {
        c.q_typed.insert(c.q_idx, input.clone());
    }
    c.q_idx = i.min(qs.len());
    c.q_cursor = 0;
    c.q_click = 0;
    input.clear();
    if c.q_idx == qs.len() {
        // The review: Send answers is picked, so enter sends them.
        c.card_focus = true;
    }
    if c.q_decide {
        // Typing goes back to being a message; the card has the keys.
        c.q_own = false;
        c.card_focus = true;
    }
    if c.q_idx < qs.len() {
        let q = &qs[c.q_idx];
        c.q_cursor = picked_at(&q.options);
        *input = c.q_typed.get(&c.q_idx).cloned().unwrap_or_default();
        let a = c.q_answer.get(&q.question).cloned().unwrap_or_default();
        if input.is_empty() && own_answer(q, &a) {
            *input = a.clone();
        }
        if let Some(j) = q.options.iter().position(|o| o.label == a) {
            c.q_cursor = j;
        }
    }
}

/// Records one answer, then moves to the next question unanswered; a
/// lone question of Claude's sends at once, several (or a decision) end
/// on the review (answerQuestion).
fn answer_question(
    c: &mut Conn,
    req: &Question,
    qs: &[Q],
    answer: String,
    input: &mut String,
) -> Res {
    c.q_answer.insert(qs[c.q_idx].question.clone(), answer);
    if qs.len() == 1 && !c.q_decide {
        return send_answers(c, req);
    }
    let mut next = qs.len();
    for k in 1..qs.len() {
        let j = (c.q_idx + k) % qs.len();
        if !c.q_answer.contains_key(&qs[j].question) {
            next = j;
            break;
        }
    }
    go_question(c, qs, next, input);
    Res::Yes
}

/// The reply: the answers as the tool's input, or for a decisions block
/// your next message in its own numbering (sendAnswers).
fn send_answers(c: &mut Conn, req: &Question) -> Res {
    if c.q_decide {
        let text = decision_answer(req, &c.q_answer);
        let again = c.q_again.as_ref().is_some_and(|q| q.id == req.id);
        if !text.is_empty() {
            c.dec_answered = req.id.clone();
        }
        c.q_for.clear();
        c.q_gone = req.id.clone();
        c.q_own = false;
        c.q_again = None;
        if text.is_empty() && again {
            return Res::Yes;
        }
        if text.is_empty() {
            return Res::Do(Act::DismissDecisions(req.id.clone()));
        }
        return Res::Do(Act::Send(text));
    }
    let b = answer_input(req, &c.q_answer);
    c.q_for.clear();
    Res::Do(Act::Allow {
        id: req.id.clone(),
        input: Some(b),
        always: false,
    })
}

/// Text's decisions block as a question card, None when it has none: each
/// decision an ask keyed by its number, each option a choice keyed by its
/// letter, the picks marked (DecisionsOf). id names it.
pub fn decisions_of(text: &str, id: &str) -> Option<Question> {
    let (_, ds, _) = rush_convo::decisions::decisions_in(text);
    if ds.is_empty() {
        return None;
    }
    let mut q = Question {
        id: id.to_string(),
        ..Question::default()
    };
    for d in ds.iter().take(9) {
        let mut a = Ask {
            id: d.r#ref.clone(),
            header: d.topic.clone(),
            text: format!("{}. {}", d.r#ref, d.topic),
            ..Ask::default()
        };
        if !d.context.is_empty() {
            a.text = format!("{}\n\n{}", d.context, a.text); // the card's lead-in, then the ask
        }
        for o in &d.options {
            a.options.push(Choice {
                key: o.key.clone(),
                label: o.label.clone(),
                description: o.detail.clone(),
                preview: o.preview.clone(),
                picked: o.recommended,
            });
        }
        q.asks.push(a);
    }
    Some(q)
}

/// The message that answers q, in its own numbering, one paragraph a
/// decision: "1a. Topic: label" for an option, "2: text" for words of
/// your own; one left unanswered is left out (DecisionAnswer).
pub fn decision_answer(q: &Question, answers: &HashMap<String, String>) -> String {
    let mut out = Vec::new();
    for a in &q.asks {
        let ans = answers.get(&a.text).map_or("", |s| s.trim());
        if ans.is_empty() {
            continue;
        }
        let mut l = format!("{}: {ans}", a.id);
        if let Some(o) = a.options.iter().find(|o| o.label == ans) {
            l = format!("{}{}. {}: {ans}", a.id, o.key, a.header);
        }
        out.push(l);
    }
    out.join("\n\n")
}

/// The input that answers q through an Allow: the questions as Claude
/// Code's AskUserQuestion takes them, the answers keyed by question text,
/// and the preview of each chosen option beside it (AnswerInput).
pub fn answer_input(q: &Question, answers: &HashMap<String, String>) -> String {
    let mut notes = Map::new();
    let qs: Vec<Value> = q
        .asks
        .iter()
        .map(|a| {
            let opts: Vec<Value> = a
                .options
                .iter()
                .map(|o| {
                    if !o.preview.is_empty() && answers.get(&a.text) == Some(&o.label) {
                        notes.insert(a.text.clone(), json!({ "preview": o.preview }));
                    }
                    let mut v = json!({ "label": o.label, "description": o.description });
                    if !o.preview.is_empty() {
                        v["preview"] = json!(o.preview);
                    }
                    v
                })
                .collect();
            json!({ "question": a.text, "header": a.header, "multiSelect": a.multi, "options": opts })
        })
        .collect();
    let mut sorted: Vec<(&String, &String)> = answers.iter().collect();
    sorted.sort();
    let ans: Map<String, Value> = sorted
        .into_iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    let mut v = Map::new();
    if !q.title.is_empty() {
        v.insert("title".into(), json!(q.title));
    }
    v.insert("questions".into(), Value::Array(qs));
    v.insert("answers".into(), Value::Object(ans));
    if !notes.is_empty() {
        v.insert("annotations".into(), Value::Object(notes));
    }
    Value::Object(v).to_string()
}

// ---- the card's keys ----

/// The open Session's card keys (a session_keys hook).
pub fn card_key_hook(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(i) = info(m, &key) else {
        return Out::Pass;
    };
    let empty = i.input.is_empty();
    card_key(m, &key, &i, &k.s, empty)
}

/// Answers a waiting card. Plain letters and digits answer only once ↑
/// has put the keys on the card, so typing a message that starts with
/// "yes" or "1." never answers by accident; alt+y, alt+a and alt+n answer
/// from anywhere. Pass when the key isn't the card's (cardKey).
pub fn card_key(m: &mut Model, key: &str, i: &HostInfo, s: &str, empty: bool) -> Out {
    let mut c = take(m, key);
    let out = card_key_on(m, key, i, &mut c, s, empty);
    put(m, key, c);
    out
}

fn card_key_on(m: &mut Model, key: &str, i: &HostInfo, c: &mut Conn, s: &str, empty: bool) -> Out {
    let kind = card_kind(m, key, c, i);
    if kind.is_empty() {
        c.card_focus = false; // a key between cards: you've moved on
        c.card_again.clear();
        return Out::Pass;
    }
    let sel_empty = m.session.as_ref().is_none_or(|v| v.sel.is_empty());
    let mut s = s.to_string();
    match kind {
        "approval" => {
            let sess = session(m, key).expect("open");
            let sid = sess.pending()[0];
            let st = sess.step(sid);
            let req_id = st
                .approval
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_default();
            // The card's buttons: ←→ (or tab) pick one, enter presses it.
            if c.card_focus {
                let bs = perm_btns(m, i, st, usize::MAX);
                c.btn_pick = c.btn_pick.min(bs.len() - 1);
                let d: isize = match s.as_str() {
                    "left" | "shift+tab" | "h" => -1,
                    "right" | "tab" | "l" => 1,
                    _ => 0,
                };
                if d != 0 {
                    c.btn_pick = (c.btn_pick as isize + d).rem_euclid(bs.len() as isize) as usize;
                    return Out::Draw;
                }
                if s == "enter" {
                    s = bs[c.btn_pick][0].clone();
                }
            }
            // With the box empty its letters answer it, no ↑ needed,
            // unless it opened on you mid-word.
            let direct = c.card_focus
                || empty
                    && sel_empty
                    && !c.card_guarded(m.clock.unwrap_or_else(rush_core::time::now));
            let a = if s == "alt+y" || direct && s == "y" || c.card_focus && s == "enter" {
                Some(Act::Allow {
                    id: req_id.clone(),
                    input: None,
                    always: false,
                })
            } else if s == "alt+a" || direct && s == "a" {
                Some(Act::Allow {
                    id: req_id.clone(),
                    input: None,
                    always: true,
                })
            } else if s == "alt+n" || direct && s == "n" {
                Some(Act::Deny {
                    id: req_id.clone(),
                    why: String::new(),
                })
            } else {
                None
            };
            if let Some(a) = a {
                return done(m, key, c, vec![a]);
            }
            if c.card_focus && s == "o" {
                // Only focused: a typed "okay…" must never switch the session.
                let mode = bypass_mode(m, &i.kind, Some(i));
                if mode.is_empty() {
                    let name = m.reg.harness_label(&i.kind);
                    m.flash_err(&format!("{name} advertises no don't-ask mode"));
                    return Out::Draw;
                }
                let out = done(
                    m,
                    key,
                    c,
                    vec![Act::Allow {
                        id: req_id,
                        input: None,
                        always: false,
                    }],
                );
                set_permission(m, key, &mode);
                return out;
            }
            if !empty && !c.card_focus && s == "enter" {
                // What's typed is why not: it's denied, and the agent's told.
                let why = i.input.trim().to_string();
                set_input(m, key, "");
                return done(m, key, c, vec![Act::Deny { id: req_id, why }]);
            }
        }
        "question" => {
            let sess = session(m, key).expect("open");
            let sid = sess.pending()[0];
            let Some(req) = sess
                .step(sid)
                .approval
                .as_ref()
                .and_then(|a| a.question.clone())
            else {
                return Out::Pass;
            };
            if c.card_focus && s == "s" {
                return done(
                    m,
                    key,
                    c,
                    vec![Act::Deny {
                        id: req.id.clone(),
                        why: "The user skipped the question; carry on with your best judgement."
                            .into(),
                    }],
                );
            }
            // Typed text and enter answer in your own words; digits pick
            // only while the card has the keys.
            if !empty && s == "enter" || c.card_focus {
                let r = keyed(m, key, i, c, &req, &s, empty);
                if r.used() {
                    return finish(m, key, c, r);
                }
            }
        }
        "decisions" => {
            let Some(req) = decisions_card(c, i, 0) else {
                return Out::Pass;
            };
            c.sync_question(&req, i.decisions.as_ref());
            if c.card_focus && s == "s" {
                // Set aside: nothing goes to the agent.
                c.q_gone = req.id.clone();
                c.q_for.clear();
                if c.q_again.as_ref().is_some_and(|q| q.id == req.id) {
                    c.q_again = None; // opened again here: nothing to dismiss
                    return done(m, key, c, Vec::new());
                }
                return done(m, key, c, vec![Act::DismissDecisions(req.id.clone())]);
            }
            if c.card_focus && s == "a" {
                // Its picks for whatever you haven't answered yourself.
                for a in &req.asks {
                    if !c.q_answer.contains_key(&a.text) && !a.options.is_empty() {
                        let o = &a.options[picked_at(&a.options)];
                        if o.picked {
                            c.q_answer.insert(a.text.clone(), o.label.clone());
                        }
                    }
                }
                let r = send_answers(c, &req);
                return finish(m, key, c, r);
            }
            // What's typed is a message as ever, unless you chose to
            // answer one in your own words.
            if !empty && s == "enter" && c.q_own || c.card_focus {
                let r = keyed(m, key, i, c, &req, &s, empty);
                if r.used() {
                    return finish(m, key, c, r);
                }
            }
        }
        _ => {}
    }
    // The card sits just above the box: ↑ from the box goes straight onto
    // it, and ↑ again on up into the conversation; ↓ off the last of its
    // rows comes back onto it, and ↓ off it to the box.
    let above = if s == "up" || s == "down" {
        m.session
            .as_ref()
            .and_then(|v| v.shown.iter().rev().find(|r| !r.is_empty()).cloned())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    if !c.card_focus && empty && s == "up" && sel.is_empty() {
        c.card_focus = true;
        return Out::Draw;
    }
    if !c.card_focus && empty && s == "down" && !sel.is_empty() && sel == above {
        c.card_focus = true;
        if let Some(v) = m.session.as_mut() {
            v.sel.clear();
        }
        return Out::Draw;
    }
    if c.card_focus && s == "down" {
        c.card_focus = false;
        return Out::Draw;
    }
    if c.card_focus && s == "up" && !above.is_empty() {
        c.card_focus = false;
        if let Some(v) = m.session.as_mut() {
            v.sel = above;
        }
        return Out::Draw;
    }
    if c.card_focus && s == "up" {
        return Out::Draw; // nothing above it: stay
    }
    if c.card_focus && s == "esc" {
        c.card_focus = false;
        return Out::Draw;
    }
    if c.card_focus {
        c.card_focus = false; // anything else goes back to typing
    }
    Out::Pass
}

/// question_key with the session's box as its input.
fn keyed(
    m: &mut Model,
    key: &str,
    i: &HostInfo,
    c: &mut Conn,
    req: &Question,
    s: &str,
    empty: bool,
) -> Res {
    let mut input = i.input.clone();
    let r = question_key(c, req, i.decisions.as_ref(), s, empty, &mut input);
    if input != i.input {
        set_input(m, key, &input);
    }
    r
}

/// A question's key done: what it sends goes, and the card lets go.
fn finish(m: &mut Model, key: &str, c: &mut Conn, r: Res) -> Out {
    match r {
        Res::Do(a) => done(m, key, c, vec![a]),
        _ => Out::Draw,
    }
}

/// Sends acts and gives the keys back to the box (cardKey's done).
fn done(m: &mut Model, key: &str, c: &mut Conn, acts: Vec<Act>) -> Out {
    if c.card_focus {
        c.card_again = c.card_shown.clone();
    }
    c.card_focus = false;
    for a in acts {
        act(m, key, a);
    }
    Out::Draw
}

/// A click at x, y on screen on the open Session's card: a button presses
/// its key; on a question's option it puts the cursor there, and a second
/// click on it picks it; on a key in its hints, it presses that key. One
/// click never answers a question (clickQuestion, clickCard).
pub fn click(m: &mut Model, key: &str, i: &HostInfo, x: usize, y: usize) -> Out {
    let c = take(m, key);
    let top = c.card_top;
    let pane_x = 2;
    if y < top || c.q_rows.is_empty() && c.btns.is_empty() {
        put(m, key, c);
        return Out::Pass;
    }
    let dy = y - top;
    let dx = x.saturating_sub(pane_x);
    let mut c = c;
    // A button.
    if let Some(b) = c
        .btns
        .iter()
        .find(|b| dy >= b.y && dy < b.y + 3 && dx >= b.x && dx < b.x + b.w)
        .cloned()
    {
        c.card_focus = true;
        put(m, key, c);
        let k = if b.key == "enter" {
            "enter"
        } else {
            b.key.as_str()
        };
        return card_key(m, key, i, k, true).or_idle();
    }
    if dy >= c.q_rows.len() {
        put(m, key, c);
        return Out::Pass;
    }
    let mut k = c.q_hit.get(&dy).cloned().unwrap_or_default();
    if let Some(n) = k.strip_prefix('@') {
        let at: usize = n.parse().unwrap_or(0);
        if !c.card_focus || c.q_cursor != at || c.q_click != at + 1 {
            c.card_focus = true;
            c.q_cursor = at;
            c.q_click = at + 1;
            put(m, key, c);
            return Out::Draw;
        }
        k = "enter".into();
    }
    if k.is_empty() {
        k = hint_key(&c.q_rows[dy], dx).unwrap_or_default();
    }
    if k.is_empty() {
        put(m, key, c);
        return Out::Pass;
    }
    c.card_focus = true;
    put(m, key, c);
    card_key(m, key, i, &k, true).or_idle()
}
