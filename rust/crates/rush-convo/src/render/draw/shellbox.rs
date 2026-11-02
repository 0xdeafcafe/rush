//! A command rush runs itself, drawn as a terminal box: a shell block of
//! a reply (with ▶ run and ⧉ copy on its top edge before it's run), or a
//! command typed after `!` in the message box, drawn after the items its
//! turn had when it ran. Its output streams in under a rule, the newest
//! rows; the foot says how long it runs, then how it ended, and offers
//! it again, to the agent, or gone. A chain's commands each say how long
//! they ran, as an agent's do (chain.rs, through the same Chain). The
//! buttons are links (rush:shell/<act>/<ref>) a click on the
//! conversation follows. Rust-only; the model is crate::shellbox.

use std::hash::{DefaultHasher, Hash, Hasher};

use super::*;
use crate::render::Preset;
use crate::shellbox::{OUT_SHOWN, Sgr, ShellBlock, ShellRun, sgr_spans};

/// How wide a box's inside may get.
const BOX_MOST: usize = 96;

/// What a click on one of a box's buttons opens: rush:shell/<act>/<ref>.
pub fn button_url(act: &str, r: &str) -> String {
    format!("rush:shell/{act}/{r}")
}

/// What of turn prefix's runs (refs starting "t<n>:") its drawing reads.
pub(crate) fn turn_sig(o: &Options, prefix: &str) -> u64 {
    let mut h = DefaultHasher::new();
    let mut runs: Vec<_> = o
        .shells
        .iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .collect();
    runs.sort_by(|a, b| a.0.cmp(b.0));
    let mut running = false;
    for (k, r) in runs {
        (k, r.ver, r.end, r.exit, r.stopped, r.paused, r.sent).hash(&mut h);
        running |= r.running();
    }
    if running {
        // Its clock and spinner.
        (o.tick, o.now / 100).hash(&mut h);
    }
    h.finish()
}

impl Drawer<'_> {
    /// Whether rush draws shell blocks as boxes here: in the conversation
    /// itself, a reply's own text, as Normal shows it.
    fn boxes(&self) -> bool {
        self.text_item.is_some() && self.o.preset == Preset::Normal && self.t.n > 0
    }

    /// Item i of the turn is a reply with a run of one of its blocks.
    pub(crate) fn has_runs(&self, i: usize) -> bool {
        if self.o.shells.is_empty() {
            return false;
        }
        let p = format!("{}:sh:{i}:", self.r);
        self.o.shells.keys().any(|k| k.starts_with(&p))
    }

    /// The fence tagged tag holding body as a shell block, when it's one
    /// and drawn as a box here.
    pub(crate) fn boxed_shell(&self, tag: &str, body: &[&str]) -> Option<ShellBlock> {
        if !self.boxes() {
            return None;
        }
        ShellBlock::from_fence(tag, body)
    }

    /// The next shell block of the reply being drawn, as its box.
    pub(crate) fn shell_block(&mut self, b: &ShellBlock) {
        let Some(i) = self.text_item else { return };
        let r = format!("{}:sh:{i}:{}", self.r, self.shell_k);
        self.shell_k += 1;
        let script = b.script();
        let o = self.o;
        // A run is the block's only while it ran what the block says.
        let run = o
            .shells
            .get(&r)
            .filter(|run| run.command == script)
            .map(|r| &**r);
        self.gap();
        self.shell_box(&r, &b.rows(), &script, run);
        self.gap();
    }

    /// The `!` runs of the turn anchored at or before item i, in the
    /// order they ran, not yet drawn.
    pub(crate) fn bangs_upto(&mut self, i: usize) {
        let o = self.o;
        if o.shells.is_empty() || o.preset != Preset::Normal {
            return;
        }
        let p = format!("{}:bang:", self.r);
        let mut due: Vec<(&String, &ShellRun)> = o
            .shells
            .iter()
            .filter(|(k, r)| k.starts_with(&p) && r.bang && r.after <= i)
            .filter(|(k, _)| !self.bangs_done.contains(*k))
            .map(|(k, r)| (k, &**r))
            .collect();
        due.sort_by_key(|(_, r)| (r.after, r.start));
        for (k, run) in due {
            self.bangs_done.insert(k.clone());
            let rows: Vec<(bool, String)> =
                run.command.lines().map(|l| (true, l.to_string())).collect();
            self.gap();
            self.shell_box(k, &rows, &run.command, Some(run));
            self.gap();
        }
    }

    /// The box itself: the folder and buttons on its top edge, the
    /// commands, under a rule what it wrote, and how it stands on its foot.
    fn shell_box(
        &mut self,
        r: &str,
        rows: &[(bool, String)],
        script: &str,
        run: Option<&ShellRun>,
    ) {
        let indent = 2;
        let inner = self.cw.saturating_sub(indent + 5).clamp(20, BOX_MOST);
        let picked = self.o.selected == r;
        let bang = run.is_some_and(|x| x.bang);
        let frame = if picked { C_ORANGE } else { C_FAINT };
        let open = self.o.open.get(r).copied().unwrap_or(false);
        let from = self.lines.len();

        // Its top edge: where it runs, and what can be done before it ends.
        let cwd = match run {
            Some(x) if !x.cwd.is_empty() => x.cwd.as_str(),
            _ if !self.o.shell_cwd.is_empty() => self.o.shell_cwd.as_str(),
            _ if !self.p.cwd.is_empty() => self.p.cwd.as_str(),
            _ => self.s.cwd.as_str(),
        };
        let mut label = if bang {
            paint(C_ORANGE.bold(), "! ")
        } else {
            Txt::new()
        };
        label.push(dim("shell · "));
        label.push(sub(&self.p.short_abs(cwd)));
        let copy = button(sub("⧉ copy"), "copy", r);
        let top_r = match run {
            None => button(paint(C_ORANGE, "▶ run"), "run", r)
                .and_raw("  ")
                .and(copy),
            Some(_) => copy,
        };
        let l = self
            .sp(indent - 1)
            .and(edge(frame, "╭", "╮", inner, label, top_r));
        self.add(r, Color::Default, l, Txt::new());

        // The commands, each with how long it ran when it's a chain.
        let running = run.is_some_and(ShellRun::running);
        let lines = shell_lines(script);
        let n = segments(script).len();
        let marks = match run {
            Some(x) if n >= 2 && !compound(&lines) => {
                let marks = x
                    .chain
                    .marks(script, n, self.o.now, x.paused, x.chain_ends());
                self.marks_txt(marks)
            }
            _ => Vec::new(),
        };
        let now_part = run
            .filter(|_| !marks.is_empty())
            .and_then(|x| x.chain.part_now(x.running()));
        let mark_w = marks.iter().map(Txt::width).max().unwrap_or(0);
        let fit = inner.saturating_sub(2 + if mark_w > 0 { mark_w + 2 } else { 0 });
        let mut part = 0usize;
        let mut body: Vec<Txt> = Vec::new();
        for (cmd, t) in rows {
            let t = expand_tabs(t);
            if !*cmd {
                // A console block's example output.
                for w in wrap(&dim(&t), inner.saturating_sub(2)) {
                    body.push(Txt::raw("  ").and(w));
                }
                continue;
            }
            // The commands this line holds: its marks are theirs.
            let k = segments(&t).len().max(1);
            let range = part..part + k;
            part += k;
            let now = now_part.is_some_and(|p| range.contains(&p));
            let mark = range
                .clone()
                .rev()
                .filter_map(|p| marks.get(p))
                .find(|m| !m.is_empty())
                .cloned()
                .unwrap_or_default();
            let lead = if now {
                paint(C_ORANGE, "▸ ")
            } else {
                faint("$ ")
            };
            // A pipeline of a running chain: its filters under it, each
            // behind a faint bar.
            let mut stages = if k == 1 && !marks.is_empty() {
                super::super::shell::pipeline_stages(&t)
            } else {
                vec![t.clone()]
            };
            if stages.is_empty() {
                stages.push(t.clone());
            }
            let t = stages.remove(0);
            let tint = if now {
                paint(C_TEXT.bold(), &t)
            } else {
                quiet_tint(&t)
            };
            for (j, w) in wrap(&tint, fit.max(8)).into_iter().enumerate() {
                let mut row = if j == 0 {
                    lead.clone().and(w)
                } else {
                    Txt::raw("  ").and(w)
                };
                if j == 0 && !mark.is_empty() {
                    let pad = inner.saturating_sub(row.width() + mark.width());
                    row = row.and(blanks(pad)).and(mark.clone());
                }
                body.push(row);
            }
            for st in &stages {
                let tint = if now {
                    paint(C_TEXT.bold(), st)
                } else {
                    quiet_tint(st)
                };
                for (j, w) in wrap(&tint, fit.saturating_sub(2).max(8))
                    .into_iter()
                    .enumerate()
                {
                    let bar = if j == 0 {
                        faint("│ ")
                    } else {
                        Txt::raw("  ")
                    };
                    body.push(Txt::raw("  ").and(bar).and(w));
                }
            }
        }
        // A chain running: which command, and the keys that act on it.
        if let Some(x) = run.filter(|x| x.running() && !marks.is_empty())
            && let Some(rp) = x.running_part()
        {
            let word = truncate_cells(&rp.command, 24);
            let mut k = Txt::new();
            if !picked {
                k.push(key_hint("↑", "pick this, then"));
                k.push_str("  ", PLAIN);
            }
            k.push(key_hint("k", &format!("skip {word}")));
            k.push(faint(" · "));
            k.push(key_hint(
                "p",
                if x.paused { "let it go on" } else { "pause it" },
            ));
            body.push(k);
        }
        let mid = body.len();

        // What it wrote: the newest rows, all of them once opened.
        if let Some(x) = run {
            let out = x.lines();
            let total = x.dropped + out.len();
            if out.iter().all(|l| l.trim().is_empty()) {
                body.push(dim(if x.running() { "…" } else { "(no output)" }));
            } else {
                let shown = if open || out.len() <= OUT_SHOWN + 1 {
                    out.len()
                } else {
                    OUT_SHOWN
                };
                let hidden = total - shown;
                if hidden > 0 {
                    let more = if open && x.dropped > 0 {
                        format!("… {} earlier lines not kept", x.dropped)
                    } else if picked {
                        format!("… {hidden} more lines · enter opens all")
                    } else {
                        format!("… {hidden} more lines")
                    };
                    body.push(dim(&more));
                }
                for l in &out[out.len() - shown..] {
                    body.push(sgr_txt(l));
                }
                if open && picked && out.len() > OUT_SHOWN + 1 {
                    body.push(dim("enter folds it"));
                }
            }
        }
        for (j, row) in body.into_iter().enumerate() {
            if j == mid {
                let l = self
                    .sp(indent - 1)
                    .and(paint(frame, &format!("├{}┤", "─".repeat(inner + 2))));
                self.add("", Color::Default, l, Txt::new());
            }
            let row = if row.width() > inner {
                row.take(inner - 1).and(faint("…"))
            } else {
                row
            };
            let w = row.width();
            let l = self
                .sp(indent - 1)
                .and(paint(frame, "│"))
                .and_raw(" ")
                .and(row)
                .and(blanks(inner - w))
                .and_raw(" ")
                .and(paint(frame, "│"));
            self.add("", Color::Default, l, Txt::new());
        }

        // Its foot: how it stands, and what can be done with it now.
        let (foot_l, foot_r) = match run {
            None if picked => (
                key_hint("enter", "runs it")
                    .and(faint(" · "))
                    .and(key_hint("c", "copies it")),
                Txt::new(),
            ),
            None => (Txt::new(), Txt::new()),
            Some(x) if running => {
                let mut l = if x.paused {
                    paint(C_YELLOW.bold(), "⏸ paused")
                } else {
                    let t = self.since(x.start);
                    paint(C_ORANGE, &format!("{} {t}", spin(self.o.tick)))
                };
                let label = x.label();
                if !label.is_empty() {
                    l.push(faint(" · "));
                    l.push(dim(&label));
                }
                (l, button(sub("✕ stop"), "stop", r))
            }
            Some(x) => {
                let took = dur((x.end - x.start).max(0));
                let how = if x.stopped {
                    paint(C_YELLOW, "✕ stopped")
                } else {
                    match x.exit {
                        Some(0) => paint(C_GREEN, "✓ exit 0"),
                        Some(c) => paint(C_RED, &format!("✗ exit {c}")),
                        None => paint(C_RED, "✗ killed"),
                    }
                };
                let l = how.and(dim(&format!(" · {took}")));
                let send = if x.sent {
                    dim("✓ in your message")
                } else {
                    button(paint(C_BLUE, "→ send to agent"), "send", r)
                };
                let again = button(sub("↻ again"), "again", r);
                let gone = button(sub("✕"), "clear", r);
                let mut rt = send
                    .and_raw("  ")
                    .and(again)
                    .and_raw("  ")
                    .and(gone.clone());
                if l.width() + rt.width() + 8 > inner {
                    // Short of room: the short words.
                    let send = if x.sent {
                        dim("✓ sent")
                    } else {
                        button(paint(C_BLUE, "→ send"), "send", r)
                    };
                    rt = send
                        .and_raw("  ")
                        .and(button(sub("↻"), "again", r))
                        .and_raw("  ")
                        .and(gone);
                }
                (l, rt)
            }
        };
        let l = self
            .sp(indent - 1)
            .and(edge(frame, "╰", "╯", inner, foot_l, foot_r));
        self.add("", Color::Default, l, Txt::new());
        self.own(r, from);
    }
}

/// t opening rush:shell/<act>/<r> when clicked.
fn button(t: Txt, act: &str, r: &str) -> Txt {
    crate::render::markdown::linked(t, &button_url(act, r))
}

/// A box's top or bottom edge inner wide inside, its label after the
/// corner and its buttons before the other: ╭─ label ───── right ─╮.
fn edge(frame: Style, l: &str, rc: &str, inner: usize, label: Txt, right: Txt) -> Txt {
    if label.is_empty() && right.is_empty() {
        return paint(frame, &format!("{l}{}{rc}", "─".repeat(inner + 2)));
    }
    // ╭─ label ─…─ right ─╮ is inner + 4 wide; without right, ╭─ label ─…─╮.
    let fixed = if right.is_empty() {
        6
    } else {
        8 + right.width()
    };
    let mut label = label;
    let room = (inner + 4).saturating_sub(fixed + 1);
    if label.width() > room {
        label = label.take(room.saturating_sub(1)).and(faint("…"));
    }
    let rule = (inner + 4).saturating_sub(fixed + label.width()).max(1);
    let mut out = paint(frame, &format!("{l}─"));
    if !label.is_empty() {
        out = out.and_raw(" ").and(label).and_raw(" ");
    } else {
        out = out.and(paint(frame, "──"));
    }
    out = out.and(paint(frame, &"─".repeat(rule)));
    if !right.is_empty() {
        out = out.and_raw(" ").and(right).and_raw(" ");
    }
    out.and(paint(frame, &format!("─{rc}")))
}

/// A line of output in the colours it was written in, over the colour
/// tool output is drawn in.
fn sgr_txt(l: &str) -> Txt {
    let mut t = Txt::new();
    for (s, g) in sgr_spans(l) {
        let mut st = match g.fg {
            Some(c) => Style::fg(c),
            None => C_OUT,
        };
        st = with_mods(st, g);
        t.push_str(&s, st);
    }
    t
}

fn with_mods(mut st: Style, g: Sgr) -> Style {
    use rush_core::text::Mods;
    for (on, m) in [
        (g.bold, Mods::BOLD),
        (g.dim, Mods::DIM),
        (g.italic, Mods::ITALIC),
        (g.underline, Mods::UNDERLINE),
    ] {
        if on {
            st.mods = st.mods.with(m);
        }
    }
    st
}
