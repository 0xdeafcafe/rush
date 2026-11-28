//! A running chain in the conversation: how long each of its commands ran,
//! or runs (render.go:3165's marks, chainrun.go:243 partMarks), and the
//! keys that skip the command running now or stop the whole chain
//! (rushmode.go:3800's hint, drawn under the command itself so it shows
//! without picking the step first). Works from the generic model only:
//! the shells under whichever agent's process, matched to its Bash calls.

use super::*;
use crate::model::PartMark;
use crate::render::shell::Segment;

/// How many digits n has.
fn digits(n: usize) -> usize {
    n.to_string().len()
}

/// A command of a chain in a few words, for its row of the list: the
/// command its pipeline starts with, the filters after it left out, and a
/// heredoc fed to it as how many lines it has: python3 (59-line script).
fn part_name(sg: &Segment) -> String {
    let text = super::super::shell::run_text(&sg.text).unwrap_or_else(|| sg.text.clone());
    let text = one_line(&text);
    if sg.body.is_empty() {
        return text;
    }
    // The word that ends the heredoc isn't one of its lines.
    let n = sg.body.len().saturating_sub(1).max(1);
    let head = text.split("<<").next().unwrap_or("").trim();
    let head = head.strip_suffix(" -").unwrap_or(head).trim();
    format!("{head} ({n}-line script)").trim().to_string()
}

impl Drawer<'_> {
    /// What the opened call says beside each of the n commands of step
    /// id's chain: ✓ 2.1s, the spinner and how long the one running now
    /// has run, ✗ where it failed; empty when none was seen running.
    pub(crate) fn chain_marks(&mut self, id: StepId, n: usize) -> Vec<Txt> {
        let st = self.s.step(id);
        let paused = !st.id.is_empty() && self.o.paused.contains(&*st.id);
        let marks = self.s.part_marks(id, n, self.o.now, paused);
        self.marks_txt(marks)
    }

    /// The marks of a chain's commands as drawn beside them. An agent's
    /// Bash call and a command rush runs itself (shellbox.rs) both draw
    /// theirs so.
    pub(crate) fn marks_txt(&mut self, marks: Vec<PartMark>) -> Vec<Txt> {
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

    /// Whether step id's chain is drawn as a list of its steps
    /// (chain_list): a chain of two commands or more, written a command
    /// after another, seen running. A loop or an if keeps its text, and a
    /// script file its own rows (script_rows).
    pub(crate) fn chain_listed(&self, id: StepId) -> bool {
        if self.o.verbose || self.force_open || self.s.running_label(id).is_empty() {
            return false;
        }
        let st = self.s.step(id);
        let info = self.info(id);
        let cmd = &info.call.input.command;
        let n = segments(cmd).len();
        let paused = !st.id.is_empty() && self.o.paused.contains(&*st.id);
        n >= 2
            && !compound(&shell_lines(cmd))
            && !self.s.part_marks(id, n, self.o.now, paused).is_empty()
    }

    /// A running chain, under its step's row: about five rows round the
    /// command running now. Those done before it a row each, the two
    /// latest (✓ and how long it took; one that failed always has its
    /// row, ✗), the earlier ones counted on a row of their own; the one
    /// running, with the spinner, how long it has run and the process
    /// busy under it, and the last two lines it printed; then those not
    /// started, named on one row with how many more there are
    /// (· update-buck --check  · git merge origin/master  +4). A chain of
    /// fourteen commands drawn a row each took the conversation's page
    /// to say where it was. Picked, a row of its keys closes it. A
    /// heredoc's lines and a pipeline's filters are folded into the
    /// command's name. Enter opens the call as written (shell_body),
    /// which is how a finished one is drawn. Whether it drew. Not Go's,
    /// which drew the command's whole text with a time beside each line,
    /// then the keys, what runs now and the output as three blocks under
    /// it.
    pub(crate) fn chain_list(&mut self, id: StepId, r: &str, indent: usize) -> bool {
        if !self.chain_listed(id) {
            return false;
        }
        let (s, o) = (self.s, self.o);
        let st = s.step(id);
        let info = self.info(id);
        let segs = segments(&info.call.input.command);
        let paused = !st.id.is_empty() && o.paused.contains(&*st.id);
        let marks = s.part_marks(id, segs.len(), self.o.now, paused);
        let busy = o
            .live
            .get(&*st.id)
            .map(|l| l.now.clone())
            .unwrap_or_default();
        // The last of what it printed: the running command's.
        let mut tail: Vec<String> = match self.live_out_of(id) {
            Some(out) => out
                .lines()
                .iter()
                .map(|l| expand_tabs(&super::liveout::plain_text(l)))
                .collect(),
            None => o
                .live
                .get(&*st.id)
                .map(|l| l.tail.clone())
                .unwrap_or_default(),
        };
        tail.retain(|l| !l.trim().is_empty());
        let tail = &tail[tail.len().saturating_sub(2)..];
        let running = |m: &PartMark| matches!(m, PartMark::Running { .. } | PartMark::Paused);
        let last_running = marks.iter().rposition(running);
        let pad = self.sp(indent - 1);
        // Where the chain is: the command running, else the last one
        // seen. Before it, the two latest rows and any that failed; after
        // it, one row for all that hasn't started.
        let at = last_running
            .or_else(|| marks.iter().rposition(|m| !matches!(m, PartMark::Unknown)))
            .unwrap_or(0);
        let runs = |k: usize| super::super::shell::run_text(&segs[k].text).is_some();
        let failed = |k: usize| matches!(marks.get(k), Some(PartMark::Ran { failed: true, .. }));
        let before: Vec<usize> = (0..at.min(segs.len())).filter(|&k| runs(k)).collect();
        const BEFORE: usize = 2;
        let folded: Vec<usize> = before
            .iter()
            .rev()
            .skip(BEFORE)
            .copied()
            .filter(|&k| !failed(k))
            .collect();
        if !folded.is_empty() {
            let l = pad
                .clone()
                .and(paint(C_GREEN, "✓ "))
                .and(dim(&format!("{} before", plural(folded.len(), "command"))));
            self.add(r, Color::Default, l, Txt::new());
        }
        let mut later: Vec<String> = Vec::new();
        for (k, sg) in segs.iter().enumerate() {
            // A comment, a loop's head: it runs nothing.
            if !runs(k) || folded.contains(&k) {
                continue;
            }
            let m = marks.get(k).copied().unwrap_or(PartMark::Unknown);
            if k > at && matches!(m, PartMark::Unknown) {
                later.push(part_name(sg));
                continue;
            }
            let round = |runs: usize| {
                if runs > 1 {
                    format!(" · run {runs}")
                } else {
                    String::new()
                }
            };
            let (lead, bright, right) = match m {
                PartMark::Unknown => (faint("○ "), false, Txt::new()),
                PartMark::Paused => (
                    paint(C_YELLOW, "⏸ "),
                    true,
                    paint(C_YELLOW.bold(), "paused"),
                ),
                PartMark::Running { since, runs } => {
                    let mut t = paint(C_ORANGE, &format!("{}{}", self.since(since), round(runs)));
                    if !busy.is_empty() {
                        t.push(faint(" · "));
                        t.push(dim(&truncate_cells(&busy, 28)));
                    }
                    (paint(C_ORANGE, &format!("{} ", spin(self.o.tick))), true, t)
                }
                PartMark::Ran {
                    took,
                    runs,
                    failed: true,
                } => (
                    paint(C_RED, "✗ "),
                    false,
                    paint(C_RED, &format!("{}{}", dur(took), round(runs))),
                ),
                PartMark::Ran { took, runs, .. } => (
                    paint(C_GREEN, "✓ "),
                    false,
                    dim(&format!("{}{}", dur(took), round(runs))),
                ),
                PartMark::Between { took, less } => (
                    paint(C_GREEN, "✓ "),
                    false,
                    match took {
                        Some(t) if less => dim(&format!("<{}", dur(t))),
                        Some(t) => dim(&dur(t)),
                        None => Txt::new(),
                    },
                ),
            };
            let room = self
                .cw
                .saturating_sub(indent + 2 + right.width() + 6)
                .max(8);
            let name = truncate_cells(&part_name(sg), room);
            let name = if bright {
                paint(C_TEXT.bold(), &name)
            } else {
                dim(&name)
            };
            let right = if right.is_empty() {
                right
            } else {
                right.and_raw("  ")
            };
            self.add(r, Color::Default, pad.clone().and(lead).and(name), right);
            if Some(k) == last_running {
                let room = self.cw.saturating_sub(indent + 8).max(8);
                for l in tail {
                    let row = pad
                        .clone()
                        .and_raw("    ")
                        .and(dim(&truncate_cells(l.trim_end(), room)));
                    self.add(r, Color::Default, row, Txt::new());
                }
            }
        }
        // What hasn't started, on one row: as many names as fit, then how
        // many more.
        if !later.is_empty() {
            let room = self.cw.saturating_sub(indent + 8).max(16);
            let mut l = pad.clone();
            let mut shown = 0;
            for name in &later {
                let name = truncate_cells(name, 28);
                let more = later.len() - shown - 1;
                // Room kept for the count of those left out.
                let tail = if more > 0 { 2 + 1 + digits(more) } else { 0 };
                let sep = if shown > 0 { 3 } else { 0 };
                if shown > 0 && l.width() + sep + 2 + width(&name) + tail > room + indent {
                    break;
                }
                if shown > 0 {
                    l.push_str("   ", Style::default());
                }
                l.push(faint("○ "));
                l.push(dim(&name));
                shown += 1;
            }
            if shown < later.len() {
                l.push(faint(&format!("  +{}", later.len() - shown)));
            }
            self.add(r, Color::Default, l, Txt::new());
        }
        // Picked, one row of keys; ▸ says the row can open (enter). k and
        // p are in the hints at the foot of the screen. Not picked, no
        // row: "↑ pick this step, then…" under every running chain said
        // how to use a list on each of them, and the foot of the screen
        // has ↑ steps.
        let picked = o.selected == r;
        if !picked {
            return true;
        }
        let mut l = pad.and(faint("▸ "));
        l.push(key_hint("enter", "opens it"));
        l.push(faint(" · "));
        l.push(key_hint("x", "stops the chain"));
        let room = self.cw.saturating_sub(2);
        let l = if l.width() > room {
            l.truncate(room, &faint("…"))
        } else {
            l
        };
        self.add(r, Color::Default, l, Txt::new());
        true
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

    /// Under a running chain, picked, the keys that act on it: k skips the
    /// command running now (the chain goes on as after a failure), x stops
    /// the whole chain, p pauses the command or lets it go on. Not picked,
    /// nothing: the keys do nothing then, and the row said how to pick it
    /// first under every running chain (Rust-only; Go drew it always).
    /// Nothing while rush hasn't seen which command runs.
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
        if self.o.selected != self.step_ref(st) {
            return;
        }
        let mut l = self.sp(indent - 1);
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
