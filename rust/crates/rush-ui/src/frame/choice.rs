//! A list of values to pick one of, the one in use marked: the frame of
//! Go's choice sheet (/model, /effort with nothing named). What it lists
//! and what a pick does are the caller's. After Go's
//! internal/ui/choicesheet.go (choiceSheet :127, width :172, body :174,
//! key :214) and fleetslash.go:352 (pickerMove).

use rush_core::text::Line;

use super::picker::round_move;
use super::sheet::{Sheet, sheet_row, sheet_title, window};
use super::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// One value and what it says beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    /// "" is the default.
    pub id: String,
    pub note: String,
}

/// What a pick does with the value picked; the sheet has closed by then.
pub type OnChoose = Box<dyn FnMut(&mut Model, &Choice) -> Out>;
/// A key of the caller's own (r for "bring it into rush"): Pass when it
/// isn't one.
pub type OnKey = Box<dyn FnMut(&mut Model, &str) -> Out>;

pub struct ChoiceSheet {
    /// "model", "effort": its title, capitalised, and the /name hint.
    pub name: String,
    pub about: String,
    pub opts: Vec<Choice>,
    /// The row in use, marked ✓; None when not known.
    pub now: Option<usize>,
    pub cur: usize,
    pub on_choose: OnChoose,
    pub on_key: Option<OnKey>,
}

impl ChoiceSheet {
    pub fn new(
        name: impl Into<String>,
        about: impl Into<String>,
        opts: Vec<Choice>,
        on_choose: OnChoose,
    ) -> ChoiceSheet {
        ChoiceSheet {
            name: name.into(),
            about: about.into(),
            opts,
            now: None,
            cur: 0,
            on_choose,
            on_key: None,
        }
    }

    /// Marks the row whose id is now as in use, and puts the cursor on it.
    pub fn running(mut self, now: &str) -> ChoiceSheet {
        if let Some(i) = self.opts.iter().position(|o| o.id == now) {
            self.now = Some(i);
            self.cur = i;
        }
        self
    }
}

fn label(o: &Choice) -> &str {
    if o.id.is_empty() { "default" } else { &o.id }
}

impl Sheet for ChoiceSheet {
    fn width(&self, _m: &Model) -> usize {
        96
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut title = self.name.clone();
        if let Some(f) = title.get_mut(..1) {
            f.make_ascii_uppercase();
        }
        let mut out = vec![sheet_title(&title, &self.about, w), Line::new()];
        let lw = self
            .opts
            .iter()
            .map(|o| rush_core::text::width(label(o)))
            .max()
            .unwrap_or(0);
        let (from, to) = window(self.opts.len(), self.cur, 1.max(h.saturating_sub(6)));
        for i in from..to {
            let o = &self.opts[i];
            let mark = if self.now == Some(i) {
                sp("✓ ", GREEN)
            } else {
                plain("  ")
            };
            let l = label(o);
            let row = line([
                mark,
                spb(fit_str(l, lw), TEXT),
                plain("  "),
                sp(
                    rush_draw::truncate(&o.note, w.saturating_sub(lw + 8), "…").into_owned(),
                    DIM,
                ),
            ]);
            out.push(sheet_row(row, i == self.cur, w));
        }
        out.push(Line::new());
        out.push(line([sp(
            format!("/{} <name> takes one not listed", self.name),
            DIM,
        )]));
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[("↑↓", "choose"), ("enter", "switch"), ("esc", "cancel")],
        ));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if let Some(f) = self.on_key.as_mut() {
            let o = f(m, &k.s);
            if o.used() {
                return o;
            }
        }
        let n = self.opts.len();
        match k.s.as_str() {
            "esc" | "ctrl+c" => Out::Close,
            "up" | "shift+tab" => {
                self.cur = round_move(self.cur, -1, n);
                Out::Draw
            }
            "down" | "tab" => {
                self.cur = round_move(self.cur, 1, n);
                Out::Draw
            }
            "enter" => {
                let Some(c) = self.opts.get(self.cur).cloned() else {
                    return Out::Close;
                };
                let o = (self.on_choose)(m, &c);
                if o == Out::Quit { o } else { Out::Close }
            }
            _ => Out::Idle,
        }
    }
}
