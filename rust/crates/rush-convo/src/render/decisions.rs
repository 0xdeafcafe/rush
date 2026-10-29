//! An answer that ends with a decisions block draws it as a list rather
//! than prose: each decision's number and topic, what's at stake, then its
//! options one a line, the agent's pick marked. Port of
//! internal/convo/decisions.go (DecisionsRef, DecisionsOnCard,
//! DecisionsSent, answered, answerItem, decisions, decisionPreview,
//! pickedLine :211, answerRows :221, lastDecisions :241, answeredRows :262).

use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::Color;

use super::draw::Drawer;
use super::markdown::inline;
use super::style::*;
use super::txt::{Txt, blanks, width, wrap};
use crate::decisions::{Decision, decisions_in};
use crate::model::{Item, Session, Turn};

/// The ref of the decisions block in turn ref's answer.
pub fn decisions_ref(turn: &str) -> String {
    format!("{turn}:dec")
}

/// The Open key saying block ref's card is up: the block is then one
/// quiet row.
pub fn decisions_on_card(r: &str) -> String {
    format!("{r}:card")
}

/// The Open key saying an answer to block ref has gone and hasn't landed
/// as a turn yet.
pub fn decisions_sent(r: &str) -> String {
    format!("{r}:sent")
}

/// How a message answering a decisions block starts: "1a. Topic: yes",
/// "1a 2b", or "2: …".
static ANSWER_OPENS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+)(?:[a-z]\b|:)").expect("a valid pattern"));

/// How many rows of an option's preview the block shows.
const PREVIEW_MOST: usize = 14;

/// The decisions block t's answer ends with, if it has one.
fn decisions_of(t: &Turn) -> Vec<Decision> {
    for it in t.items.iter().rev() {
        if let Item::Text { text, answer: true } = it {
            return decisions_in(text).1;
        }
    }
    Vec::new()
}

/// Whether a message of yours after turn ti answers its block ds, and so
/// whether its picks were chosen or only suggested.
fn answered(s: &Session, ti: usize, ds: &[Decision]) -> bool {
    for u in &s.turns[ti + 1..] {
        if u.from.is_empty()
            && u.cause.is_empty()
            && let Some(m) = ANSWER_OPENS.captures(&u.prompt)
            && ds.iter().any(|d| d.r#ref == m[1])
        {
            return true;
        }
        if !decisions_of(u).is_empty() {
            return false;
        }
    }
    false
}

impl Drawer<'_> {
    /// An answer, its decisions block drawn as a list (decisions.go:94).
    pub(crate) fn answer_item(&mut self, text: &str) {
        let (before, ds, after) = decisions_in(text);
        if ds.is_empty() {
            self.answer(text);
            return;
        }
        self.gap();
        let b = before.trim();
        if !b.is_empty() {
            self.markdown(b, 2, C_TEXT, true);
            self.gap();
        }
        self.decisions(&ds);
        let a = after.trim();
        if !a.is_empty() {
            self.gap();
            self.markdown(a, 2, C_TEXT, true);
        }
    }

    /// A heading, then each decision's number and topic, its context, and
    /// its options as bullets, the pick ★ and green (decisions.go:114).
    fn decisions(&mut self, ds: &[Decision]) {
        let r = decisions_ref(&self.r);
        let pad = self.sp(1);
        let w = (self.cw as isize - 3).min(self.o.prose_cap() as isize);
        let opened = |k: &str| self.o.open.get(k).copied().unwrap_or(false);
        if opened(&decisions_on_card(&r)) {
            let n = plural(ds.len(), "decision");
            let l = pad
                .clone()
                .and(paint(C_SUB, "Decisions"))
                .and(dim(&format!(" · {n} · on the card below")));
            self.add(&r, Color::Default, l, Txt::new());
            return;
        }
        // Its picks were chosen only if you answered it.
        let ti = self.s.turns.iter().position(|t| t.n == self.t.n);
        let passed = !self.t.live
            && !opened(&decisions_sent(&r))
            && !ti.is_some_and(|ti| answered(self.s, ti, ds));
        let mut head = pad.clone().and(paint(strong(C_TEXT), "Decisions"));
        if passed {
            head.push(dim(" · not answered · d opens the card"));
        }
        self.add(&r, Color::Default, head, Txt::new());
        for (i, dc) in ds.iter().enumerate() {
            if i > 0 {
                self.add(&r, Color::Default, pad.clone(), Txt::new());
            }
            let l = pad
                .clone()
                .and(dim(&format!("{}.", dc.r#ref)))
                .and_raw(" ")
                .and(paint(C_WHITE.bold(), &dc.topic));
            self.add(&r, Color::Default, l, Txt::new());
            let iw = dc.r#ref.len() + 2;
            if !dc.context.is_empty() {
                for l in wrap(
                    &inline(&dc.context, C_SUB).over(C_SUB),
                    (w - iw as isize).max(8) as usize,
                ) {
                    let l = pad.clone().and(blanks(iw)).and(l);
                    self.add(&r, Color::Default, l, Txt::new());
                }
            }
            // Labels line up, unless one is long.
            let mut lw = dc
                .options
                .iter()
                .map(|o| width(&o.label))
                .max()
                .unwrap_or(0);
            if lw > 12 {
                lw = 0;
            }
            for o in &dc.options {
                let (mut mark, mut name, mut ink) =
                    (dim("•"), paint(C_TEXT.bold(), &o.label), C_SUB);
                if o.recommended && passed {
                    mark = dim("☆"); // only suggested: nothing was picked
                } else if o.recommended {
                    mark = paint(C_GREEN, "★");
                    name = paint(C_GREEN.bold(), &o.label);
                    ink = C_TEXT;
                }
                let ow = width(&o.label);
                let lead = mark
                    .and_raw(" ")
                    .and(name)
                    .and(blanks(lw.saturating_sub(ow)));
                let lead_w = 2 + lw.max(ow) + 2;
                let mut rows = vec![Txt::new()];
                if !o.detail.is_empty() {
                    rows = wrap(
                        &inline(&o.detail, ink).over(ink),
                        (w - iw as isize - lead_w as isize).max(8) as usize,
                    );
                }
                let tag = if o.recommended && passed {
                    dim("its suggestion")
                } else if o.recommended {
                    paint(C_GREEN, "its pick")
                } else {
                    Txt::new()
                };
                if !tag.is_empty() {
                    let last = rows.pop().unwrap_or_default();
                    rows.push(trim_end(last).and_raw("  ").and(tag));
                }
                for (k, row) in rows.into_iter().enumerate() {
                    if k == 0 {
                        let l = pad
                            .clone()
                            .and(blanks(iw))
                            .and(lead.clone())
                            .and_raw("  ")
                            .and(row);
                        self.add(&r, Color::Default, l, Txt::new());
                        continue;
                    }
                    let l = pad.clone().and(blanks(iw + lead_w)).and(row);
                    self.add(&r, Color::Default, l, Txt::new());
                    self.wrapped();
                }
                let ppad = pad.clone().and(blanks(iw + 2));
                self.decision_preview(&r, &ppad, &o.preview, (w - iw as isize - 2).max(0) as usize);
            }
        }
    }

    /// What an option would look like under it, behind a quiet rule.
    fn decision_preview(&mut self, r: &str, pad: &Txt, preview: &str, w: usize) {
        if preview.trim().is_empty() {
            return;
        }
        let lines: Vec<&str> = preview.split('\n').collect();
        let more = lines.len().saturating_sub(PREVIEW_MOST);
        for l in &lines[..lines.len().min(PREVIEW_MOST)] {
            let t = paint(C_TEXT, &l.replace('\t', "    "))
                .truncate(w.saturating_sub(2).max(8), &Txt::raw("…"));
            let row = pad.clone().and(dim("│ ")).and(t);
            self.add(r, Color::Default, row, Txt::new());
        }
        if more > 0 {
            let row = pad
                .clone()
                .and(dim(&format!("│ … {more} more, on the card")));
            self.add(r, Color::Default, row, Txt::new());
        }
    }
}

/// t with trailing spaces dropped.
fn trim_end(mut t: Txt) -> Txt {
    while let Some(s) = t.0.last_mut() {
        let n = s.text.trim_end_matches(' ').len();
        if n > 0 {
            s.text.truncate(n);
            break;
        }
        t.0.pop();
    }
    t
}

/// One paragraph of a message the card sent: "1a. Topic: label" for an
/// option, "2: words" for words of your own.
static PICKED_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d+)(?:([a-z])\. (.+?): (.+)|: (.+))$").expect("a valid pattern")
});

/// One decision a message answered.
#[derive(Debug, PartialEq)]
struct AnswerRow {
    r#ref: String,
    topic: String,
    said: String,
    /// Your own words, not one of its options.
    own: bool,
}

/// ask read as an answer to a decisions block, empty when it's anything
/// else: every paragraph one decision, as the card writes it.
fn answer_rows(ask: &str) -> Vec<AnswerRow> {
    let mut out = Vec::new();
    for p in ask.trim().split('\n') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let Some(m) = PICKED_LINE.captures(p) else {
            return Vec::new();
        };
        let g = |i| m.get(i).map_or("", |x| x.as_str()).to_string();
        if !g(5).is_empty() {
            out.push(AnswerRow {
                r#ref: g(1),
                topic: String::new(),
                said: g(5),
                own: true,
            });
        } else {
            out.push(AnswerRow {
                r#ref: g(1),
                topic: g(3),
                said: g(4),
                own: false,
            });
        }
    }
    out
}

impl Drawer<'_> {
    /// The decisions block a message of yours in this turn answers: the
    /// last one before it; a message sent while it worked answers this
    /// turn's own.
    fn last_decisions(&self) -> Vec<Decision> {
        let Some(i) = self.s.turns.iter().position(|u| std::ptr::eq(u, self.t)) else {
            return Vec::new();
        };
        if self.while_now() {
            return decisions_of(self.t);
        }
        self.s.turns[..i]
            .iter()
            .rev()
            .map(decisions_of)
            .find(|ds| !ds.is_empty())
            .unwrap_or_default()
    }

    /// A message the decisions card sent as a short list: each decision's
    /// number and topic, then what you chose, green with a tick for one of
    /// its options, a pencil for words of your own. None when ask isn't
    /// such a message.
    pub(crate) fn answered_rows(&self, ask: &str, w: usize) -> Option<Vec<Txt>> {
        let mut rows_in = answer_rows(ask);
        if rows_in.is_empty() {
            return None;
        }
        let ds = self.last_decisions();
        for a in rows_in.iter_mut().filter(|a| a.topic.is_empty()) {
            if let Some(dc) = ds.iter().find(|dc| dc.r#ref == a.r#ref) {
                a.topic = dc.topic.clone();
            }
        }
        let tw = rows_in
            .iter()
            .map(|a| a.r#ref.len() + 2 + width(&a.topic))
            .max()
            .unwrap_or(0)
            .min(w / 3);
        let mut rows = Vec::new();
        for a in &rows_in {
            let (mut head, mut hw) = if a.topic.is_empty() {
                (dim(&format!("{}.", a.r#ref)), a.r#ref.len() + 1)
            } else {
                (
                    dim(&format!("{}.", a.r#ref))
                        .and_raw(" ")
                        .and(paint(C_WHITE.bold(), &a.topic)),
                    a.r#ref.len() + 2 + width(&a.topic),
                )
            };
            let (mark, said) = if a.own {
                (dim("✎ "), inline(&a.said, C_WHITE).over(C_WHITE))
            } else {
                (
                    paint(C_GREEN, "✓ "),
                    inline(&a.said, C_GREEN).over(C_GREEN.bold()),
                )
            };
            let mut lead = tw + 2;
            if hw > tw {
                // A long topic: what you chose goes under it.
                rows.push(head);
                lead = a.r#ref.len() + 2;
                head = blanks(lead);
                hw = lead;
            }
            for (k, b) in wrap(&said, 8.max(w.saturating_sub(lead + 2)))
                .into_iter()
                .enumerate()
            {
                if k == 0 {
                    rows.push(head.clone().and(blanks(lead - hw)).and(mark.clone()).and(b));
                } else {
                    rows.push(blanks(lead + 2).and(b));
                }
            }
        }
        Some(rows)
    }
}
