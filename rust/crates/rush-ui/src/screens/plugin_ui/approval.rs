//! A plugin found installed, waiting on you: rush plugin approve without
//! a terminal, what describe says it may do, and Approve or Not now.
//! After Go's internal/ui/pluginapproval.go (checkPluginApprovals :17,
//! openPluginApproval :23, pluginApprovalSheet :35, body :51, key :84,
//! pick :103, done :123).

use rush_core::text::Line;
use rush_plugin::Plugin;

use super::{choice, settled, sheet_do};
use crate::frame::sheet::{sheet_row, sheet_title, window};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// Looks once, off the UI, for plugins never approved or changed since
/// (checkPluginApprovals); they're offered once the screen is free.
pub fn check(m: &mut Model) {
    let disk = m.plugins().disk.clone();
    m.later(
        move || disk.pending(),
        |m, pending| {
            m.plugins().pending = Vec::new();
            open(m, pending);
            Out::Draw
        },
    );
}

/// Offers the first of pending, unless something already has the screen;
/// the rest wait for the next look (openPluginApproval).
pub fn open(m: &mut Model, pending: Vec<Plugin>) {
    if pending.is_empty() {
        return;
    }
    if !m.screen_free() {
        m.plugins().pending = pending;
        return;
    }
    let mut pending = pending.into_iter();
    let p = pending.next().expect("one at least");
    let s = ApprovalSheet {
        p: p.clone(),
        rest: pending.collect(),
        cur: 0,
        busy: false,
        err: String::new(),
        changed: false,
    };
    m.open_sheet(Box::new(s));
    // Whether it was approved before is read off the UI.
    let disk = m.plugins().disk.clone();
    let name = p.name.clone();
    m.later(
        move || disk.approved_before(&name),
        |m, ok| {
            if let Some(s) = m.sheet_as::<ApprovalSheet>() {
                s.changed = ok;
            }
            Out::Draw
        },
    );
}

/// Approve or Not now for one plugin (pluginApprovalSheet).
pub struct ApprovalSheet {
    pub p: Plugin,
    /// More waiting, offered after this one.
    pub rest: Vec<Plugin>,
    /// 0 Approve, 1 Not now.
    pub cur: usize,
    pub busy: bool,
    pub err: String,
    /// It was approved before, and has changed since.
    pub changed: bool,
}

impl Sheet for ApprovalSheet {
    fn width(&self, _m: &Model) -> usize {
        100
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let status = if self.changed {
            "changed since it was approved"
        } else {
            "found, not approved"
        };
        let mut out = vec![
            sheet_title("Plugin", &format!("{} · {status}", self.p.name), w),
            Line::new(),
        ];
        let body = wrap(&rush_plugin::describe(&self.p), w.saturating_sub(2));
        let room = 3.max(h as isize - out.len() as isize - 5) as usize;
        let (from, to) = window(body.len(), 0, room);
        for l in &body[from..to] {
            out.push(line([plain("  "), plain(l.clone())]));
        }
        out.push(Line::new());
        let choices = [
            ("Approve", "let it run, sandboxed, as described above"),
            ("Not now", "leave it stopped · asks again if it changes"),
        ];
        for (i, (what, about)) in choices.iter().enumerate() {
            out.push(sheet_row(choice(what, about), i == self.cur, w));
        }
        if !self.err.is_empty() {
            out.push(Line::new());
            out.push(line([sp(self.err.clone(), RED)]));
        }
        let k = if self.busy {
            line([sp("⋯ working", YELLOW)])
        } else {
            keys_fit(
                w,
                &[("↑↓", "choose"), ("enter", "pick"), ("esc", "not now")],
            )
        };
        out.push(Line::new());
        out.push(k);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if self.busy {
            return Out::Idle;
        }
        match k.s.as_str() {
            "up" | "shift+tab" | "down" | "tab" => self.cur = 1 - self.cur,
            "y" => {
                self.cur = 0;
                return self.pick(m);
            }
            "n" | "esc" | "ctrl+c" => {
                self.cur = 1;
                return self.pick(m);
            }
            "enter" => return self.pick(m),
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

impl ApprovalSheet {
    /// Approves or declines it off the UI (pick).
    fn pick(&mut self, m: &mut Model) -> Out {
        self.busy = true;
        self.err.clear();
        let disk = m.plugins().disk.clone();
        let p = self.p.clone();
        let approve = self.cur == 0;
        sheet_do(
            m,
            move || {
                if approve {
                    disk.approve(&p).and_then(|_| disk.reload())
                } else {
                    disk.decline(&p)
                }
            },
            ApprovalSheet::done,
        );
        settled(self, m, ApprovalSheet::done).unwrap_or(Out::Draw)
    }

    /// On success the next waiting is offered; on failure it stays put
    /// so you can see why and try again (done).
    fn done(&mut self, m: &mut Model, r: Result<(), String>) -> Out {
        if let Err(e) = r {
            self.busy = false;
            self.err = e;
            return Out::Draw;
        }
        let said = if self.cur == 0 {
            format!(
                "{} approved · sessions pick it up as they next start",
                self.p.name
            )
        } else {
            format!("left {} stopped", self.p.name)
        };
        m.flash(&said);
        let rest = std::mem::take(&mut self.rest);
        m.sheet = None;
        open(m, rest);
        if m.sheet.is_some() {
            Out::Draw
        } else {
            Out::Close
        }
    }
}
