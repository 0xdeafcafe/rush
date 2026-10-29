//! What enter does on an agent: asked the first time you press it, whether
//! enter renames agents or opens them. After Go's internal/ui/entersheet.go
//! (enterSheet :13, enterChoices :18, body :29, key :38).

use rush_core::text::Line;

use crate::frame::sheet::{sheet_row, sheet_title};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// The two answers, by the config's word for each.
pub const CHOICES: [(&str, &str, &str); 2] = [
    ("rename", "Rename it", "as in the Finder · ⌘↓ or tab opens it"),
    ("open", "Open it", "into its Session · ctrl+r renames it"),
];

/// The question, for one agent (enterSheet).
pub struct EnterSheet {
    pub agent: String,
    /// 0 renames, 1 opens.
    pub cur: usize,
}

/// Asks what enter should do, for agent key (askEnter).
pub fn ask_enter(m: &mut Model, key: &str) {
    m.open_sheet(Box::new(EnterSheet {
        agent: key.to_string(),
        cur: 0,
    }));
}

impl Sheet for EnterSheet {
    fn width(&self, _m: &Model) -> usize {
        64
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![sheet_title("Enter on an agent", "what should it do?", w), Line::new()];
        for (i, (_, what, about)) in CHOICES.iter().enumerate() {
            out.push(sheet_row(
                line([spb(*what, TEXT), plain("  "), sp(*about, DIM)]),
                i == self.cur,
                w,
            ));
        }
        out.push(Line::new());
        out.push(line([sp("Settings › General changes it", DIM)]));
        out.push(Line::new());
        out.push(keys_fit(w, &[("↑↓", "choose"), ("enter", "pick"), ("esc", "cancel")]));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" => Out::Close,
            "up" | "shift+tab" => {
                self.cur = 0;
                Out::Draw
            }
            "down" | "tab" => {
                self.cur = 1;
                Out::Draw
            }
            "enter" => {
                if !super::save_setting(m, "enterOn", CHOICES[self.cur].0) {
                    return Out::Close;
                }
                let agent = self.agent.clone();
                let open = self.cur == 1;
                super::act(m, &agent, super::Act::EnterOn(open));
                Out::Close
            }
            _ => Out::Idle,
        }
    }
}
