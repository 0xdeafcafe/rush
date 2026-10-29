//! What the session's last message asks of you, as a section of its own
//! under the last answer. Port of internal/convo/needs.go (Needs,
//! NeedsItem, NeedsKinds, NeedsRef, NeedsOf, key, foldWords, HasQuote,
//! needsHere, needsSection) and render.go:416, :811 (its cache key and
//! where it's drawn).

use rush_core::text::Color;

use super::draw::Drawer;
use super::style::*;
use super::txt::{Txt, blanks, wrap};
use super::{CAP_ROW, GUTTER};

/// What the session's last message asks of you, as the list read it: what
/// kind of ask it is (kind: "you", "offer", "ask" or "problem"), what it
/// wants in a few words (reason), the words in the message that ask it
/// (quote, as the message has them), and the rows the model laid out for
/// it (items). The last turn ends with it as a section of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    pub kind: String,
    pub reason: String,
    pub quote: String,
    pub items: Vec<NeedsItem>,
}

/// One row of the Needs-you section: kind is "do" (a step only you can
/// take), "run" (a command for you to run), "copy" (a value to paste
/// somewhere: a host, an ID), "reply" (a reply you could send), "link" (a
/// page to open) or "note" (a fact to decide by).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NeedsItem {
    pub kind: String,
    pub text: String,
}

/// The kinds of row the section draws.
pub const NEEDS_KINDS: [&str; 6] = ["do", "run", "copy", "reply", "link", "note"];

/// The ref of a row of the section a click acts on: a command copied, a
/// reply put in your message box, a link opened.
pub fn needs_ref(kind: &str, text: &str) -> String {
    format!("needs:{kind}:{text}")
}

/// The kind and text of a needs_ref.
pub fn needs_of(r: &str) -> Option<(&str, &str)> {
    r.strip_prefix("needs:")?.split_once(':')
}

impl Needs {
    /// n for a cache key: "" when nothing is asked.
    pub(crate) fn key(&self) -> String {
        if self.reason.is_empty() && self.items.is_empty() {
            return String::new();
        }
        let mut b = format!("{}\x00{}\x00{}", self.kind, self.reason, self.quote);
        for it in &self.items {
            b.push_str(&format!("\x00{}:{}", it.kind, it.text));
        }
        b
    }
}

/// s as a quote is compared: markdown's marks gone, every run of space one
/// space, lower case. A quote copied from a message matches whether or not
/// it kept the message's backticks, and across wrapped rows.
fn fold_words(s: &str) -> String {
    let mut b = String::new();
    let mut space = false;
    for r in s.chars() {
        if r == '`' || r == '*' || r == '_' {
            continue;
        }
        if r.is_whitespace() {
            space = !b.is_empty();
            continue;
        }
        if space {
            b.push(' ');
            space = false;
        }
        b.extend(r.to_lowercase());
    }
    b
}

/// Whether text holds q, as fold_words compares them.
pub fn has_quote(text: &str, q: &str) -> bool {
    let q = fold_words(q);
    !q.is_empty() && fold_words(text).contains(&q)
}

impl Drawer<'_> {
    /// Whether the turn being drawn ends with the Needs-you section: the
    /// session's last, finished, with something asked.
    pub(crate) fn needs_here(&self) -> bool {
        !self.o.needs.key().is_empty()
            && self.s.turns.last().is_some_and(|t| std::ptr::eq(t, self.t))
            && !self.t.live
    }

    /// What the last message asks of you as a section of its own under it:
    /// a heading saying what, then the rows the model laid out: steps to
    /// take, a command, values to paste, replies, links, a note. The words
    /// that asked aren't repeated: they're just above. A command, value,
    /// reply or link is clicked to act on it.
    pub(crate) fn needs_section(&mut self) {
        let n = self.o.needs.clone();
        let (frame, mark) = match n.kind.as_str() {
            "you" => (C_YELLOW, paint(C_YELLOW.bold(), "!")),
            "problem" => (C_RED, paint(C_RED.bold(), "!")),
            _ => (C_YELLOW, paint(C_YELLOW.bold(), "?")),
        };
        let pad0 = self.spine().and(blanks(GUTTER - 1));
        let pad = || pad0.clone();
        let w = 16.max(self.cw.min(CAP_ROW).saturating_sub(GUTTER + 6));
        self.gap();
        let mut head = mark.and_raw(" ").and(paint(strong(C_TEXT), "Needs you"));
        if !n.reason.is_empty() {
            head = head.and(dim(" · ")).and(paint(C_TEXT, &n.reason));
        }
        let l = pad().and(paint(frame, "╭")).and_raw(" ").and(head);
        self.add("", Color::Default, l, Txt::new());
        let mut rows: Vec<(String, Txt, Txt)> = Vec::new();
        for it in &n.items {
            let (lead, ink, hint, r) = match it.kind.as_str() {
                "do" => (paint(C_TEXT, "☐"), C_TEXT, "", String::new()),
                "run" => (
                    paint(C_GREEN, "$"),
                    C_WHITE,
                    "click copies",
                    needs_ref(&it.kind, &it.text),
                ),
                "copy" => (
                    paint(C_GREEN, "⧉"),
                    C_WHITE,
                    "click copies",
                    needs_ref(&it.kind, &it.text),
                ),
                "reply" => (
                    paint(C_BLUE, "↩"),
                    C_TEXT,
                    "click to reply",
                    needs_ref(&it.kind, &it.text),
                ),
                "link" => (
                    paint(C_BLUE, "↗"),
                    C_BLUE,
                    "click opens",
                    needs_ref(&it.kind, &it.text),
                ),
                _ => (dim("·"), C_SUB, "", String::new()),
            };
            let mut body = paint(ink, &it.text);
            if it.kind == "copy"
                && let Some((v, what)) = it.text.split_once(" — ")
            {
                // The value, then quietly what it is.
                body = paint(ink, v).and(dim(&format!(" — {what}")));
            }
            for (k, r_) in wrap(&body, w).into_iter().enumerate() {
                let l = if k == 0 {
                    lead.clone().and_raw(" ")
                } else {
                    blanks(2)
                };
                let right = if k == 0 && !hint.is_empty() {
                    dim(hint).and_raw("  ")
                } else {
                    Txt::new()
                };
                rows.push((r.clone(), l.and(r_), right));
            }
        }
        // The last row closes the frame.
        let last = rows.len().saturating_sub(1);
        if rows.is_empty() {
            let l = pad().and(paint(frame, "╰"));
            self.add("", Color::Default, l, Txt::new());
        }
        for (i, (r, body, right)) in rows.into_iter().enumerate() {
            let edge = if i == last { "╰" } else { "│" };
            let l = pad().and(paint(frame, edge)).and_raw(" ").and(body);
            self.add(&r, Color::Default, l, right);
        }
    }
}
