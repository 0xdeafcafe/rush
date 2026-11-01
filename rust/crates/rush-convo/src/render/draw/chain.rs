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
        let round = |runs: usize| {
            if runs > 1 {
                format!(" · run {runs}")
            } else {
                String::new()
            }
        };
        let mut out = Vec::with_capacity(marks.len());
        for m in marks {
            out.push(match m {
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
            });
        }
        out
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
        let label = self.s.running_label(id);
        let words = label
            .split_once(' ')
            .map_or(rp.command.as_str(), |(_, w)| w);
        let words = words.split(" · run").next().unwrap_or(words);
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
