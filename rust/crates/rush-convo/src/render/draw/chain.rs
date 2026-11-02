//! A running chain in the conversation: how long each of its commands ran,
//! or runs (render.go:3165's marks, chainrun.go:243 partMarks), and the
//! keys that skip the command running now or stop the whole chain
//! (rushmode.go:3800's hint, drawn under the command itself so it shows
//! without picking the step first). Works from the generic model only:
//! the shells under whichever agent's process, matched to its Bash calls.

use super::*;
use crate::model::PartMark;

impl Drawer<'_> {
    /// What the opened call says beside each of the n commands of step
    /// id's chain: ✓ 2.1s, the spinner and how long the one running now
    /// has run, ✗ where it failed; empty when none was seen running.
    pub(crate) fn chain_marks(&mut self, id: StepId, n: usize) -> Vec<Txt> {
        let st = self.s.step(id);
        let paused = !st.id.is_empty() && self.o.paused.contains(&*st.id);
        let marks = self.s.part_marks(id, n, self.o.now, paused);
        let mut out = Vec::with_capacity(marks.len());
        for m in marks {
            out.push(self.mark_txt(m));
        }
        out
    }

    /// One command's time as its row's right side says it.
    fn mark_txt(&mut self, m: PartMark) -> Txt {
        let round = |runs: usize| {
            if runs > 1 {
                format!(" · run {runs}")
            } else {
                String::new()
            }
        };
        match m {
            PartMark::Unknown => Txt::new(),
            PartMark::Paused => paint(C_YELLOW.bold(), "⏸ paused"),
            PartMark::Running { since, runs } => {
                let t = self.since(since);
                paint(
                    C_ORANGE,
                    &format!("{} {t}{}", spin(self.o.tick), round(runs)),
                )
            }
            PartMark::Ran { took, runs, failed } => {
                let t = format!("{}{}", dur(took), round(runs));
                if failed {
                    paint(C_RED, &format!("✗ {t}"))
                } else {
                    paint(C_GREEN, &format!("✓ {t}"))
                }
            }
            PartMark::Between { took, less } => {
                let t = match took {
                    Some(t) if less => format!("<{}", dur(t)),
                    Some(t) => dur(t),
                    None => String::new(),
                };
                paint(C_GREEN, format!("✓ {t}").trim())
            }
        }
    }

    /// Under a script file's call (bash build.sh, ./build.sh), whose
    /// commands its text can't show: those seen run under it, the latest
    /// few, each with how long it took, the one running now marked ▸ with
    /// the spinner and how long it has run.
    pub(crate) fn script_rows(&mut self, id: StepId, indent: usize) {
        const SHOWN: usize = 6;
        let st = self.s.step(id);
        let paused = !st.id.is_empty() && self.o.paused.contains(&*st.id);
        let runs = self.s.script_runs(id, self.o.now, paused);
        if runs.is_empty() {
            return;
        }
        let pad = self.sp(indent - 1);
        let from = runs.len().saturating_sub(SHOWN);
        if from > 0 {
            let l = pad
                .clone()
                .and(faint(&format!("  ⋯ {} before", plural(from, "command"))));
            self.add("", BG_WELL, l, Txt::new());
        }
        let room = self.cw.saturating_sub(indent + 18).max(8);
        for r in &runs[from..] {
            let now = matches!(r.mark, PartMark::Running { .. } | PartMark::Paused);
            let words = truncate_cells(&r.words, room);
            let (lead, text) = if now {
                (paint(C_ORANGE, "▸ "), paint(C_TEXT.bold(), &words))
            } else {
                (faint("  "), dim(&words))
            };
            let right = self.mark_txt(r.mark).and_raw("  ");
            self.add("", BG_WELL, pad.clone().and(lead).and(text), right);
        }
    }

    /// Under a running chain, the keys that act on it: k skips the command
    /// running now (the chain goes on as after a failure), x stops the
    /// whole chain, p pauses the command or lets it go on. Not picked, it
    /// says how to pick it first. Nothing while rush hasn't seen which
    /// command runs.
    pub(crate) fn chain_keys(&mut self, id: StepId, indent: usize) {
        let st = self.s.step(id);
        if st.id.is_empty() {
            return;
        }
        let Some(rp) = self.s.running_part(&st.id) else {
            return;
        };
        // Its first words, as the step's row names it: 2/3 cargo test.
        let words = self
            .s
            .running_now(id)
            .map(|(_, w)| w)
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| rp.command.clone());
        let words = words.split(" · run").next().unwrap_or(&words);
        let word = truncate_cells(words, 24);
        let pause = if self.o.paused.contains(&*st.id) {
            "let it go on"
        } else {
            "pause it"
        };
        let picked = self.o.selected == self.step_ref(st);
        let mut l = self.sp(indent - 1);
        if !picked {
            l.push(key_hint("↑", "pick this step, then"));
            l.push_str("  ", Style::default());
        }
        l.push(key_hint("k", &format!("skip {word}")));
        l.push(faint(" · "));
        l.push(key_hint("x", "stop the whole chain"));
        l.push(faint(" · "));
        l.push(key_hint("p", pause));
        let room = self.cw.saturating_sub(2);
        let l = if l.width() > room {
            l.truncate(room, &faint("…"))
        } else {
            l
        };
        self.add("", Color::Default, l, Txt::new());
    }
}
