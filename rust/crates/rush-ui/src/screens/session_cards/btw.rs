//! /btw: side questions in a floating panel. A btw thread is a side
//! conversation with Claude about the agent's work, answered from the
//! conversation so far (Claude Code's side_question), without adding to
//! it or stopping the turn under way. It floats over the Session's top
//! right; ctrl+b moves the keys between it and the box. Pulled out
//! (ctrl+f), it becomes a fork whose first message carries the thread
//! on. After Go's internal/ui/btwpanel.go (btwThread :27, openBtw :48,
//! ask :66, btwKey :103, forkBtw :166, clickBtw :196, cellAt :214, drag
//! :222, endBtwDrag :231, copyBtwSel :245, btwOverlay :279, lines :317).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};

use super::sel::{Cell, Sel, paint_cols, selected_text};
use super::{ask, info, open_key, set_input, setting};
use crate::frame::dialog::paste_at;
use crate::frame::editor::edit;
use crate::frame::sheet::text_field;
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// A raised surface: it floats over the conversation (bgBtw).
pub const BTW_BG: Color = Color::Rgb(36, 33, 30);

/// One side question and its answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Qa {
    pub question: String,
    pub response: String,
}

/// A side thread: /btw's, or the quick ask's (btwThread).
#[derive(Clone, Debug, Default)]
pub struct Thread {
    pub qa: Vec<Qa>,
    pub input: Vec<char>,
    pub pos: usize,
    /// When the question out was asked; 0 when none.
    pub waiting: Ms,
    /// Which ask the answer waited on is for, so one lost isn't taken.
    pub asked: u64,
    pub err: String,
    /// Rows up from the latest.
    pub scroll: usize,
    /// It has the keys, not the message box.
    pub focused: bool,
    /// Where it was drawn: x, y, w, h on screen.
    pub at: [usize; 4],
    /// Each row drawn, inside the edge.
    pub shown: Vec<Line>,
    /// Text dragged over, in rows of shown.
    pub sel: Sel,
}

impl Thread {
    /// The cell of the panel's text under the pointer, clamped to it
    /// (cellAt).
    pub fn cell_at(&self, x: usize, y: usize) -> Cell {
        Cell {
            row: y
                .saturating_sub(self.at[1])
                .min(self.at[3].saturating_sub(1)),
            // Past the edge and its space.
            col: x
                .saturating_sub(self.at[0] + 2)
                .min(self.at[2].saturating_sub(3)),
        }
    }

    fn inside(&self, x: usize, y: usize) -> bool {
        self.at[2] > 0
            && x >= self.at[0]
            && x < self.at[0] + self.at[2]
            && y >= self.at[1]
            && y < self.at[1] + self.at[3]
    }

    /// The text dragged over, as it reads; None when nothing is.
    pub fn sel_text(&self) -> Option<String> {
        if !self.sel.on {
            return None;
        }
        Some(selected_text(
            &self.shown,
            self.sel.a,
            self.sel.b,
            self.at[2].saturating_sub(3),
        ))
    }

    /// Scrolls with ↑↓ and pgup/pgdown; true when s was one.
    pub fn scroll_key(&mut self, s: &str) -> bool {
        match s {
            "up" => self.scroll += 1,
            "pgup" => self.scroll += 8,
            "down" => self.scroll = self.scroll.saturating_sub(1),
            "pgdown" => self.scroll = self.scroll.saturating_sub(8),
            _ => return false,
        }
        self.sel = Sel::default();
        true
    }
}

/// Every agent's side thread, by agent key, and the asks made.
#[derive(Default)]
pub struct Btws {
    pub threads: HashMap<String, Thread>,
    pub asks: u64,
}

/// Agent key's side thread, if it has one open (btwFor).
pub fn btw_for<'a>(m: &'a mut Model, key: &str) -> Option<&'a mut Thread> {
    m.kept::<Btws>().threads.get_mut(key)
}

/// Opens agent key's side thread (or the one already open), with the
/// keys, and asks q when there is one (openBtw).
pub fn open_btw(m: &mut Model, key: &str, q: &str) -> Out {
    let t = m.kept::<Btws>().threads.entry(key.to_string()).or_default();
    t.focused = true;
    let q = q.trim();
    if !q.is_empty() {
        ask_btw(m, key, q);
    }
    Out::Draw
}

/// Asks q in agent key's thread, its answered questions as the history
/// (ask).
fn ask_btw(m: &mut Model, key: &str, q: &str) {
    let history: Vec<serde_json::Value> = btw_for(m, key)
        .map(|t| {
            t.qa.iter()
                .filter(|x| !x.response.is_empty())
                .map(|x| serde_json::json!({"question": x.question, "response": x.response}))
                .collect()
        })
        .unwrap_or_default();
    let n = {
        let b = m.kept::<Btws>();
        b.asks += 1;
        b.asks
    };
    let k = key.to_string();
    // Taken in first: an answer from a host with no loop comes at once.
    let now = super::now(m);
    if let Some(t) = btw_for(m, key) {
        t.qa.push(Qa {
            question: q.into(),
            response: String::new(),
        });
        t.waiting = now;
        t.asked = n;
        t.err.clear();
        t.scroll = 0;
        t.sel = Sel::default();
    }
    let asked = ask(
        m,
        key,
        serde_json::json!({"subtype": "side_question", "question": q, "history": history}),
        Box::new(move |m, r| {
            let Some(t) = btw_for(m, &k).filter(|t| t.asked == n && t.waiting != 0) else {
                return Out::Idle;
            };
            t.waiting = 0;
            // The rows under it moved.
            t.sel = Sel::default();
            match r {
                Err(e) => t.err = e,
                Ok(v) => match v.get("response").and_then(|r| r.as_str()).map(str::trim) {
                    Some(a) if !a.is_empty() => {
                        if let Some(last) = t.qa.last_mut() {
                            last.response = a.to_string();
                        }
                    }
                    _ => {
                        t.err = "no answer came back: ask again, or ask in the conversation".into()
                    }
                },
            }
            Out::Draw
        }),
    );
    if !asked && let Some(t) = btw_for(m, key) {
        // askClaude said why.
        t.qa.pop();
        t.waiting = 0;
    }
}

/// Keys meant for the open session's side thread: all of them while it
/// has the keys, and ctrl+b, which moves them there, while it hasn't
/// (btwKey).
pub fn btw_key_hook(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if info(m, &key).is_none() {
        return Out::Pass;
    }
    let s = k.s.as_str();
    let focused = btw_for(m, &key).map(|t| t.focused);
    if focused != Some(true) {
        let empty = info(m, &key).is_some_and(|i| i.input.is_empty());
        return match s {
            "ctrl+b" => open_btw(m, &key, ""),
            "ctrl+d" if focused.is_some() && empty => {
                // Tucked away, it closes from the chat too.
                m.kept::<Btws>().threads.remove(&key);
                Out::Draw
            }
            _ => Out::Pass,
        };
    }
    match s {
        "super+c" | "ctrl+c" => {
            // A selection in the panel is copied; ctrl+c with none still
            // reaches the Session.
            if let Some(txt) = btw_for(m, &key).and_then(|t| t.sel_text()) {
                if !txt.is_empty() {
                    m.copy_text(txt);
                }
                return Out::Draw;
            }
            if s == "super+c" { Out::Idle } else { Out::Pass }
        }
        "esc" | "ctrl+b" => {
            // Back to the conversation. A thread with something in it
            // stays, tucked away; one never asked anything goes.
            let t = btw_for(m, &key).expect("focused");
            t.focused = false;
            t.sel = Sel::default();
            if t.qa.is_empty() {
                m.kept::<Btws>().threads.remove(&key);
            }
            Out::Draw
        }
        "ctrl+d" => {
            m.kept::<Btws>().threads.remove(&key);
            Out::Draw
        }
        "enter" => {
            let t = btw_for(m, &key).expect("focused");
            let q: String = t.input.iter().collect::<String>().trim().to_string();
            if q.is_empty() {
                return Out::Idle;
            }
            if t.waiting != 0 {
                m.flash_err("one question at a time: the last one's still being answered");
                return Out::Draw;
            }
            t.input.clear();
            t.pos = 0;
            ask_btw(m, &key, &q);
            Out::Draw
        }
        "ctrl+f" => fork_btw(m, &key),
        "ctrl+s" => {
            // The last answer into the message box, to send after all.
            let t = btw_for(m, &key).expect("focused");
            if let Some(last) = t.qa.last().filter(|x| !x.response.is_empty()).cloned() {
                t.focused = false;
                t.sel = Sel::default();
                set_input(
                    m,
                    &key,
                    &format!(
                        "About my side question (\"{}\"), you said:\n\n{}\n\n",
                        last.question, last.response
                    ),
                );
            }
            Out::Draw
        }
        _ => {
            let t = btw_for(m, &key).expect("focused");
            if t.scroll_key(s) {
                return Out::Draw;
            }
            match edit(&t.input, t.pos, k) {
                Some((b, p)) => {
                    t.input = b;
                    t.pos = p;
                    Out::Draw
                }
                // ctrl+x and the like still reach the Session.
                None => Out::Pass,
            }
        }
    }
}

/// Pulls the side thread out into an agent of its own: /fork of the
/// whole conversation, its first message the side thread so far
/// (forkBtw).
fn fork_btw(m: &mut Model, key: &str) -> Out {
    let Some(t) = btw_for(m, key).filter(|t| !t.qa.is_empty()).cloned() else {
        return Out::Idle;
    };
    let mut name = one_line(&t.qa[0].question);
    if name.chars().count() > 40 {
        name = format!("{}…", name.chars().take(39).collect::<String>());
    }
    super::fork::open_fork(m, key, &format!("btw: {name}"));
    let Some(f) = m.sheet_as::<super::fork::ForkSheet>() else {
        return Out::Draw;
    };
    let mut b = String::from(
        "While you were working I asked some side questions. Let's carry that thread on here.\n",
    );
    for x in &t.qa {
        b.push_str(&format!("\nMe: {}\n", x.question));
        if !x.response.is_empty() {
            b.push_str(&format!("You: {}\n", x.response));
        }
    }
    f.first = b.chars().collect();
    f.first_pos = f.first.len();
    f.row = super::fork::FIRST;
    if let Some(t) = btw_for(m, key) {
        t.focused = false;
    }
    Out::Draw
}

/// Whether a release copies what was dragged over (CopiesOnSelect).
pub fn copies_on_select(m: &Model) -> bool {
    setting(m, "copyOnSelect") != "false"
}

/// The mouse on the open session's side thread: a press gives it the
/// keys and starts a drag there (a click elsewhere drops what was
/// dragged over), a drag moves its end, a release copies it
/// (clickBtw, drag, endBtwDrag).
pub fn btw_mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.sheet.is_some() || m.confirm.is_some() {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let copies = copies_on_select(m);
    let (x, y) = (e.x as usize, e.y as usize);
    let Some(t) = btw_for(m, &key) else {
        return Out::Pass;
    };
    match e.kind {
        MouseKind::Down(Button::Left) => {
            if !t.inside(x, y) {
                t.sel = Sel::default();
                return Out::Pass;
            }
            t.focused = true;
            let at = t.cell_at(x, y);
            t.sel = Sel::at(at);
            Out::Draw
        }
        MouseKind::Drag(Button::Left) if t.sel.drag => {
            let at = t.cell_at(x, y);
            t.sel.drag_to(at);
            Out::Draw
        }
        MouseKind::Up(Button::Left) if t.sel.drag => {
            t.sel.drag = false;
            if !t.sel.moved {
                t.sel = Sel::default();
                return Out::Draw;
            }
            if copies && let Some(txt) = t.sel_text().filter(|s| !s.is_empty()) {
                m.copy_text(txt);
            }
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// Draws the open session's side thread over the conversation's rows at
/// its right (btwOverlay).
// The frame's Overlay hook hands the rows as a Vec.
#[allow(clippy::ptr_arg)]
pub fn btw_overlay(m: &mut Model, rows: &mut Vec<Line>) {
    let Some(key) = open_key(m) else { return };
    let Some(top) = super::session_view(m, &key).map(|v| v.top) else {
        return;
    };
    let now = super::now(m);
    let (mw, mh) = (m.w, m.h);
    let Some(t) = btw_for(m, &key) else { return };
    if !t.focused && t.qa.is_empty() {
        // Nothing asked, and you've gone back to the chat.
        m.kept::<Btws>().threads.remove(&key);
        return;
    }
    // The conversation is indented two cells, its edge one.
    let w = mw.saturating_sub(3);
    // The rows above the message box and the status line.
    let room = mh.saturating_sub(top + 6);
    let pw = if w < 70 {
        w.saturating_sub(2)
    } else {
        w.saturating_sub(2).min(44.max(w * 45 / 100))
    };
    let max_h = if t.focused {
        room.saturating_sub(1)
    } else {
        room.saturating_sub(1).min(11)
    };
    let waiting = t.waiting;
    let mut t = std::mem::take(btw_for(m, &key).expect("there"));
    let panel = thread_lines(&mut t, pw, max_h, now);
    let left_w = w.saturating_sub(pw + 1);
    if !panel.is_empty() && room >= 5 {
        let mut b = Vec::with_capacity(panel.len());
        for p in panel {
            let mut l = line([plain(" ")]);
            join(&mut l, p);
            b.push(l);
        }
        let n = b.len().min(rows.len().saturating_sub(top));
        paste_at(rows, &b[..n], top, 2 + left_w);
        t.at = [2 + left_w + 1, top, pw, n];
    }
    *btw_for(m, &key).expect("there") = t;
    if waiting != 0 {
        // The thinking line counts the seconds.
        m.ask_wake(Instant::now() + Duration::from_secs(1));
    }
}

/// The panel pw wide and at most max_h tall: a title, the thread (the
/// latest at the bottom), the question box and its keys (lines).
pub(super) fn thread_lines(t: &mut Thread, pw: usize, max_h: usize, now: Ms) -> Vec<Line> {
    let on = t.focused;
    let edge = if on {
        sp("▍", ORANGE)
    } else {
        sp("▏", FAINT)
    };
    // Inside the edge and a space either side.
    let iw = pw.saturating_sub(3);
    let mut thread: Vec<Line> = Vec::new();
    let qa: Vec<Qa> = if !t.focused && t.qa.len() > 1 {
        // Tucked away, it shows the latest only.
        t.qa[t.qa.len() - 1..].to_vec()
    } else {
        t.qa.clone()
    };
    for (i, x) in qa.iter().enumerate() {
        if i > 0 {
            thread.push(Line::new());
        }
        for (j, l) in wrap(&x.question, iw.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            let lead = if j == 0 {
                sp("❯ ", ORANGE)
            } else {
                plain("  ")
            };
            thread.push(line([lead, spb(l, TEXT)]));
        }
        let last = i == qa.len() - 1;
        if !x.response.is_empty() {
            thread.extend(rush_convo::render::answer_lines(&x.response, iw));
        } else if last && t.waiting != 0 {
            thread.push(line([sp(
                format!("  thinking · {}", dur(now.saturating_sub(t.waiting))),
                DIM,
            )]));
        } else if last && !t.err.is_empty() {
            for l in wrap(&t.err, iw.saturating_sub(2)) {
                thread.push(line([plain("  "), sp(l, RED)]));
            }
        }
    }
    if t.qa.is_empty() {
        thread.push(line([sp(
            "ask about what's going on: it doesn't go into the conversation",
            DIM,
        )]));
    }
    let head = line([
        spb("btw", ORANGE),
        sp("  side question · not in the conversation", DIM),
    ]);
    let mut foot = Vec::new();
    if t.focused {
        let mut f = line([sp("❯ ", ORANGE)]);
        join(
            &mut f,
            text_field(&t.input, t.pos, on, "ask…", iw.saturating_sub(2)),
        );
        foot.push(f);
        foot.push(keys_fit(
            iw,
            &[
                ("enter", "ask"),
                ("↑↓", "scroll"),
                ("esc", "back to the chat"),
            ],
        ));
        if t.qa.is_empty() {
            foot.push(keys_fit(iw, &[("ctrl+d", "close")]));
        } else {
            foot.push(keys_fit(
                iw,
                &[
                    ("ctrl+f", "its own chat"),
                    ("ctrl+s", "into the box"),
                    ("ctrl+d", "close"),
                ],
            ));
        }
    } else {
        foot.push(keys_fit(iw, &[("ctrl+b", "ask more"), ("ctrl+d", "close")]));
    }
    let room = 1.max(max_h as isize - 2 - foot.len() as isize) as usize;
    if thread.len() > room {
        if !t.focused {
            // Tucked away: the latest question and the start of its answer.
            thread.truncate(room.saturating_sub(1));
            thread.push(line([sp("  … ctrl+b reads the rest", DIM)]));
        } else {
            // Anchored to its latest line; ↑ scrolls back.
            t.scroll = t.scroll.min(thread.len() - room);
            let end = thread.len() - t.scroll;
            thread = thread[end.saturating_sub(room)..end].to_vec();
        }
    }
    t.shown = vec![head, Line::new()];
    t.shown.extend(thread);
    t.shown.extend(foot);
    t.shown
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let mut r = line([edge.clone(), plain(" ")]);
            join(&mut r, l.clone());
            let mut r = on_ground(r, pw, BTW_BG);
            if let Some((from, to)) = t.sel.cols(i, iw) {
                // Past the edge and its space.
                r = paint_cols(&r, from + 2, to + 2);
            }
            r
        })
        .collect()
}
