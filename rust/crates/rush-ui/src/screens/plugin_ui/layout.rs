//! Which layout: asked the first time rush opens, Agents and the
//! Session side by side, the Session alone, or Agents alone. After Go's
//! internal/ui/viewsheet.go (viewSheet :11, viewChoices :13, body :25,
//! key :34, pick :53).

use rush_core::text::Line;

use super::{choice, settled, sheet_do};
use crate::frame::picker::round_move;
use crate::frame::sheet::{sheet_row, sheet_title};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// The layouts, as config keeps them, named and said (viewChoices).
pub const CHOICES: [(&str, &str, &str); 3] = [
    (
        "split",
        "Side by side",
        "Agents, and the picked agent's Session beside them",
    ),
    (
        "agent",
        "The Session alone",
        "one agent with the whole screen · esc for Agents",
    ),
    (
        "list",
        "Agents alone",
        "every agent at a glance · { } opens one",
    ),
];

/// Asks which layout to keep (askView).
pub fn ask(m: &mut Model) {
    m.open_sheet(Box::new(LayoutSheet::default()));
}

/// The layout question (viewSheet).
#[derive(Default)]
pub struct LayoutSheet {
    pub cur: usize,
}

impl Sheet for LayoutSheet {
    fn width(&self, _m: &Model) -> usize {
        76
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title("Layout", "how should rush open?", w),
            Line::new(),
        ];
        for (i, (_, what, about)) in CHOICES.iter().enumerate() {
            out.push(sheet_row(choice(what, about), i == self.cur, w));
        }
        out.push(Line::new());
        out.push(line([sp(
            "#view or Settings › General changes it · shift+← → too",
            DIM,
        )]));
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[("↑↓", "choose"), ("enter", "pick"), ("esc", "side by side")],
        ));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "up" | "shift+tab" => self.cur = round_move(self.cur, -1, CHOICES.len()),
            "down" | "tab" => self.cur = round_move(self.cur, 1, CHOICES.len()),
            s @ ("1" | "2" | "3") => {
                self.cur = (s.as_bytes()[0] - b'1') as usize;
                return self.pick(m);
            }
            "enter" => return self.pick(m),
            "esc" | "ctrl+c" => {
                self.cur = 0;
                return self.pick(m);
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

impl LayoutSheet {
    /// Keeps the layout chosen, off the UI (pick).
    fn pick(&mut self, m: &mut Model) -> Out {
        let disk = m.plugins().disk.clone();
        let v = CHOICES[self.cur].0;
        m.cfg.set_view(v); // the list's split reads it at once
        sheet_do(m, move || disk.set_view(v), LayoutSheet::done);
        settled(self, m, LayoutSheet::done);
        Out::Close
    }

    fn done(&mut self, _m: &mut Model, _: ()) -> Out {
        Out::Idle
    }
}
