//! Under a running shell step, what it has printed so far: its last
//! lines, dim, with a fold above them for the rest. The agent sees the
//! output only once the step ends; this is so you see something happen.
//! Works from the generic model only: what its harness streamed
//! (Session::live_output), else what rush read itself from the file its
//! shell writes to (Options::live). Gone once the step ends: its real
//! result is drawn then. New in the Rust port.

use super::*;
use crate::model::LiveOut;

/// Lines shown under a running step; the rest fold.
pub(crate) const LIVE_SHOWN: usize = 8;
/// Lines shown once the fold is opened, at most.
const LIVE_OPEN_MOST: usize = 400;

/// The ref of a running step's output fold: enter opens it.
pub fn live_ref(step: &str) -> String {
    format!("{step}:live")
}

impl<'a> Drawer<'a> {
    /// What running step id has printed so far, from wherever it's known.
    pub(crate) fn live_out_of(&self, id: StepId) -> Option<&'a LiveOut> {
        let (s, o) = (self.s, self.o);
        let st = s.step(id);
        if st.status != Status::Running || st.kind != Kind::Shell {
            return None;
        }
        s.live_output(id).or_else(|| {
            o.live
                .get(&*st.id)
                .and_then(|l| l.out.as_deref())
                .filter(|o| !o.out.is_empty())
        })
    }

    /// The running step's last lines, dim, each on the gutter its output
    /// is drawn on, and above them how many more there are: ▸ opens them.
    pub(crate) fn live_out_rows(&mut self, id: StepId, indent: usize) {
        let Some(out) = self.live_out_of(id) else {
            return;
        };
        let mut lines = out.lines();
        while lines
            .last()
            .is_some_and(|l| plain_text(l).trim().is_empty())
        {
            lines.pop();
        }
        if lines.is_empty() {
            return;
        }
        let r = live_ref(&self.step_ref(self.s.step(id)));
        let open = self.o.open.get(&r).copied().unwrap_or(false);
        let most = if open { LIVE_OPEN_MOST } else { LIVE_SHOWN };
        // One more line is shown rather than folded away.
        let shown = if lines.len() <= most + 1 {
            lines.len()
        } else {
            most
        };
        let more = out.dropped + lines.len() - shown;
        let pad = self.sp(indent - 1);
        let noun = |n: usize| if n == 1 { "line" } else { "lines" };
        if open {
            let mut l = pad.clone().and(faint("▾ ")).and(dim("fewer lines"));
            if more > 0 {
                l = l.and(faint(&format!(
                    " · {more} earlier {} not shown",
                    noun(more)
                )));
            }
            self.add(&r, Color::Default, l, Txt::new());
        } else if more > 0 {
            let l = pad
                .clone()
                .and(faint("▸ "))
                .and(dim(&format!("{more} more {}", noun(more))));
            self.add(&r, Color::Default, l, Txt::new());
        }
        let room = self.cw.saturating_sub(indent + 4).max(8);
        for l in &lines[lines.len() - shown..] {
            let text = expand_tabs(&plain_text(l));
            let row = pad
                .clone()
                .and(faint("│ "))
                .and(dim(truncate_cells(text.trim_end(), room).as_str()));
            self.add("", Color::Default, row, Txt::new());
        }
    }
}

/// A line of output without its escapes: its colours are dropped, as the
/// tail is drawn dim.
fn plain_text(l: &str) -> String {
    if !l.contains('\x1b') && !l.chars().any(char::is_control) {
        return l.to_string();
    }
    crate::shellbox::sgr_spans(l)
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}
