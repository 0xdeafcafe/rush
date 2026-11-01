//! A turn drawn in one of the conversation's other views (Options.preset):
//! Chat keeps what you and the agent said and what it asked; Code shows
//! each edit as its diff and each check as a line; Catch-up is a card a
//! turn; Trail is a line a step. Every view is drawn from the turn as the
//! session already holds it, and stretches it leaves out are a quiet row
//! that opens them in place, so nothing is lost. New in the Rust port: Go
//! rush had no such views. Works from the generic model only (Kind, Status,
//! exit codes, the shell's command), whichever agent ran the session.

use super::super::Preset;
use super::super::decisions::open_in_turn;
use super::*;
use crate::decisions::decisions_in;

/// Agent prose Code keeps of each message, in lines.
const CODE_PROSE: usize = 3;
/// The width of a catch-up card's labels ("changed ").
const LABEL_W: usize = 9;

/// The checks a shell command runs, by kind ("tests", "clippy", "fmt",
/// "lint", "build", "check", "typecheck", "vet"), in the order it runs
/// them: read from the command's words, the common tools of any language,
/// so it holds whichever agent ran it.
pub fn check_names(cmd: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for seg in cmd.split(['\n', ';', '|', '&']) {
        let words: Vec<&str> = seg
            .split_whitespace()
            .map(|w| w.trim_matches(['(', ')', '"', '\'', '`']))
            .filter(|w| !w.is_empty())
            .collect();
        if let Some(n) = check_in(&words)
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

/// The check a command's words run, if any: the first tool that is one.
fn check_in(ws: &[&str]) -> Option<&'static str> {
    for (i, w) in ws.iter().enumerate() {
        let base = w.rsplit('/').next().unwrap_or(w);
        let rest = &ws[i + 1..];
        let mut args = rest
            .iter()
            .copied()
            .filter(|x| !x.starts_with('-') && !x.starts_with('+'));
        let next = args.next().unwrap_or("");
        let found = match base {
            "cargo" => match next {
                "test" | "t" | "nextest" => Some("tests"),
                "clippy" => Some("clippy"),
                "fmt" => Some("fmt"),
                "build" | "b" => Some("build"),
                "check" | "c" => Some("check"),
                _ => None,
            },
            "go" => match next {
                "test" => Some("tests"),
                "vet" => Some("vet"),
                "build" => Some("build"),
                _ => None,
            },
            "npm" | "pnpm" | "yarn" | "bun" | "deno" => {
                let n = if next == "run" {
                    args.next().unwrap_or("")
                } else {
                    next
                };
                script_check(n)
            }
            "make" | "just" | "task" | "mise" => script_check(next),
            "pytest" | "jest" | "vitest" | "mocha" | "rspec" | "phpunit" | "ctest" | "tox"
            | "nox" | "playwright" => Some("tests"),
            "ruff" if next == "format" => Some("fmt"),
            "eslint" | "ruff" | "flake8" | "pylint" | "golangci-lint" | "shellcheck"
            | "rubocop" | "biome" | "oxlint" | "stylelint" | "swiftlint" | "ktlint" => Some("lint"),
            "prettier" | "gofmt" | "rustfmt" | "black" | "isort" | "dprint" | "goimports"
            | "swiftformat" | "clang-format" => Some("fmt"),
            "tsc" | "vue-tsc" | "mypy" | "pyright" => Some("typecheck"),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The check a package script or make target is, by its name.
fn script_check(n: &str) -> Option<&'static str> {
    let n = n.trim_start_matches(':');
    if n.starts_with("test") || n == "e2e" {
        Some("tests")
    } else if n.starts_with("lint") {
        Some("lint")
    } else if n.starts_with("fmt") || n.starts_with("format") {
        Some("fmt")
    } else if n.starts_with("typecheck") || n.starts_with("type-check") || n == "tsc" {
        Some("typecheck")
    } else if n.starts_with("build") {
        Some("build")
    } else if n == "check" || n == "ci" {
        Some("check")
    } else {
        None
    }
}

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Came {
    Running,
    Passed,
    Failed,
}

impl Drawer<'_> {
    /// The turn drawn as p has it.
    pub(crate) fn preset(&mut self, p: Preset) {
        self.no_hooks = true;
        match p {
            Preset::Chat => self.chat(),
            Preset::Code => self.code_view(),
            Preset::CatchUp => self.catch_up(),
            Preset::Trail => self.trail(),
            Preset::Normal => self.open(),
        }
    }

    // ---- what each step is to a view ----

    /// A step that asks you something, or did: a question, a plan, a
    /// permission waiting or refused.
    fn asks(&self, st: &Step) -> bool {
        matches!(st.kind, Kind::Question | Kind::PlanMode)
            || matches!(st.status, Status::Waiting | Status::Denied)
            || st.approval.is_some()
    }

    /// A step that changed files.
    fn edits(st: &Step) -> bool {
        matches!(
            st.kind,
            Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move
        )
    }

    /// The lines an edit added and took away; None for a step that isn't
    /// one or didn't go.
    fn edit_size(&self, id: StepId) -> Option<(usize, usize)> {
        let st = self.s.step(id);
        if st.status != Status::Ok || !matches!(st.kind, Kind::Edit | Kind::Write) {
            return None;
        }
        let i = self.info(id);
        let (mut add, mut del) = (0, 0);
        if i.out.created {
            add = count_lines(&i.call.input.content);
        } else if !i.out.patches.is_empty() {
            for p in &i.out.patches {
                for l in &p.lines {
                    if l.starts_with('+') {
                        add += 1;
                    } else if l.starts_with('-') {
                        del += 1;
                    }
                }
            }
        } else if !i.call.input.edits.is_empty() {
            for e in &i.call.input.edits {
                add += count_lines(&e.new);
                del += count_lines(&e.old);
            }
        } else {
            add = count_lines(&i.call.input.content);
        }
        Some((add, del))
    }

    /// Whether a step failed: refused by its tool, a command's exit code,
    /// or tests that ran and failed.
    fn bad(&self, id: StepId) -> bool {
        let st = self.s.step(id);
        st.status == Status::Failed
            || st.kind == Kind::Shell && st.exit > 0
            || st.kind == Kind::Shell && self.tests_failed(id)
    }

    /// The tests a step's output counted: passed and failed.
    fn tests_counted(&self, id: StepId) -> Option<(usize, usize)> {
        self.step_cards(id)
            .iter()
            .find(|c| c.kind == "tests")
            .map(|c| (c.passed, c.failed))
    }

    /// The checks a shell step ran, and how each came out: all passed when
    /// it did; when it failed, the one that failed is the tests when they
    /// say so, else the last, and those before it passed.
    fn checks_of(&self, id: StepId) -> Vec<(&'static str, Came)> {
        let st = self.s.step(id);
        if st.kind != Kind::Shell {
            return Vec::new();
        }
        let names = check_names(&self.info(id).call.input.command);
        if names.is_empty() {
            return Vec::new();
        }
        if matches!(st.status, Status::Running | Status::Waiting) {
            return names.into_iter().map(|n| (n, Came::Running)).collect();
        }
        if !self.bad(id) {
            return names.into_iter().map(|n| (n, Came::Passed)).collect();
        }
        let failing = if self.tests_failed(id) && names.contains(&"tests") {
            "tests"
        } else {
            names[names.len() - 1]
        };
        let mut out = Vec::new();
        for n in names {
            if n == failing {
                out.push((n, Came::Failed));
                break;
            }
            out.push((n, Came::Passed));
        }
        out
    }

    // ---- what each view leaves out ----

    /// A stretch of steps a view leaves out, as one quiet row; opened by
    /// hand (its ref), the steps themselves under it.
    fn left_out(&mut self, run: &mut Vec<(usize, StepId)>) {
        if run.is_empty() {
            return;
        }
        let at = run[0].0;
        let ids: Vec<StepId> = run.drain(..).map(|x| x.1).collect();
        let r = format!("{}:hid:{at}", self.r);
        let n = ids.len();
        if self.o.open.get(&r).copied().unwrap_or(false) {
            self.railed(GUTTER - 2, true, |d| {
                d.work(|d| {
                    let l = d
                        .sp(GUTTER - 1)
                        .and(faint(&format!("▾ hide {}", plural(n, "step"))));
                    d.add(&r, Color::Default, l, Txt::new());
                });
                for &id in &ids {
                    d.step(id, 1);
                }
            });
            return;
        }
        let failed = ids.iter().filter(|&&id| self.bad(id)).count();
        let running = ids
            .iter()
            .any(|&id| self.s.step(id).status == Status::Running);
        let mut l = self
            .sp(GUTTER - 1)
            .and(faint(&format!("▸ {} hidden", plural(n, "step"))));
        if failed > 0 {
            l.push(faint(" · "));
            l.push(paint(C_RED, &format!("{failed} failed")));
        }
        if running {
            l.push(faint(" · "));
            l.push(paint(C_ORANGE, "✱ running"));
            // Which command of a running chain runs now: 2/3 cargo test.
            if let Some(p) = ids
                .iter()
                .map(|&id| self.s.running_label(id))
                .find(|p| !p.is_empty())
            {
                l.push(faint(" "));
                l.push(paint(C_SUB, &p));
            }
        }
        l.push(faint(" · enter shows"));
        self.work(|d| d.add(&r, Color::Default, l, Txt::new()));
    }

    /// The end of a turn in Chat or Code: the live line, how it stopped,
    /// what it asks of you, and (with meta) its steps, time and cost.
    fn ending(&mut self, meta: bool) {
        let t = self.t;
        if t.live && !self.o.hide_activity {
            if self.o.dock_activity {
                self.docked_live_line();
            } else {
                self.live_line();
            }
        }
        if t.stopped {
            let l = self.spine().and_raw("   ").and(dim("⏹ stopped"));
            self.add("", Color::Default, l, Txt::new());
        } else if !t.err.is_empty() && !t.live {
            let e = t.err.clone();
            self.err_card(&e);
        }
        if self.needs_here() {
            self.needs_section();
        }
        while self.lines.len() > 1 {
            let l = self.lines.last().unwrap();
            if l.r#ref.is_empty() && trimmed_is(&l.line, self.spine_text()) {
                self.lines.pop();
            } else {
                break;
            }
        }
        let m = self.meta();
        if meta && !m.is_empty() && !t.live && t.step_count() > 0 {
            let sp = self.spine();
            self.add("", Color::Default, sp, dim(&m).and_raw("  "));
        }
    }

    // ---- Chat ----

    /// What you said, what the agent said to you, and what it asked; each
    /// stretch of work between them one quiet row.
    fn chat(&mut self) {
        self.head();
        self.speaker();
        let t = self.t;
        let mut run = Vec::new();
        for (i, it) in t.items.iter().enumerate() {
            match it {
                Item::Text { .. } | Item::Interject(_) | Item::Exchange(_) => {
                    self.left_out(&mut run);
                    self.item(i);
                }
                Item::Step(id) => {
                    let st = self.s.step(*id);
                    if hidden(st) {
                        continue;
                    }
                    if self.asks(st) || message_tool(&st.tool) {
                        self.left_out(&mut run);
                        self.step(*id, 0);
                    } else {
                        run.push((i, *id));
                    }
                }
                _ => {}
            }
        }
        self.left_out(&mut run);
        self.ending(false);
    }

    // ---- Code ----

    /// The code written: each edit open on its diff, each check a line,
    /// the agent's words cut short; reading and searching one quiet row.
    fn code_view(&mut self) {
        self.head();
        self.speaker();
        let t = self.t;
        let mut run = Vec::new();
        for (i, it) in t.items.iter().enumerate() {
            match it {
                Item::Text { text, answer } => {
                    self.left_out(&mut run);
                    if decisions_in(text).1.is_empty() {
                        self.short_prose(text, *answer);
                    } else {
                        self.item(i);
                    }
                }
                Item::Interject(_) | Item::Exchange(_) => {
                    self.left_out(&mut run);
                    self.item(i);
                }
                Item::Step(id) => {
                    let st = self.s.step(*id);
                    if hidden(st) {
                        continue;
                    }
                    if Self::edits(st) {
                        self.left_out(&mut run);
                        self.force_open = true;
                        self.step(*id, 0);
                        self.force_open = false;
                    } else if self.asks(st) {
                        self.left_out(&mut run);
                        self.step(*id, 0);
                    } else if st.kind == Kind::Shell
                        && (!self.checks_of(*id).is_empty() || self.bad(*id))
                    {
                        self.left_out(&mut run);
                        self.check_row(*id);
                    } else {
                        run.push((i, *id));
                    }
                }
                _ => {}
            }
        }
        self.left_out(&mut run);
        self.ending(true);
    }

    /// The agent's words, their first lines only, and how many more.
    fn short_prose(&mut self, text: &str, answer: bool) {
        let lines: Vec<&str> = text
            .trim()
            .split('\n')
            .filter(|l| !l.trim().is_empty())
            .collect();
        if lines.is_empty() {
            return;
        }
        let mut head = lines[..lines.len().min(CODE_PROSE)].join("\n");
        if head.matches("```").count() % 2 == 1 {
            head.push_str("\n```");
        }
        self.gap();
        let c = if answer { C_TEXT } else { C_SUB };
        self.markdown(&head, 2, c, answer);
        let more = lines.len().saturating_sub(CODE_PROSE);
        if more > 0 {
            let l = self
                .sp(1)
                .and(faint(&format!("… {}", plural(more, "more line"))));
            self.add("", Color::Default, l, Txt::new());
        }
        self.gap();
    }

    /// A check, or a command that failed, as one line: ✓ tests pass,
    /// ✗ tests · 1 failed, ✗ cargo build · exit 101.
    fn check_row(&mut self, id: StepId) {
        let st = self.s.step(id);
        let r = self.step_ref(st);
        let checks = self.checks_of(id);
        let running = matches!(st.status, Status::Running | Status::Waiting);
        let bad = !running && self.bad(id);
        let mark = if running {
            paint(C_ORANGE, "✱")
        } else if bad {
            paint(C_RED, "✗")
        } else {
            paint(C_GREEN, "✓")
        };
        let what = if checks.is_empty() {
            one_line(first_line(&self.info(id).call.input.command))
        } else {
            checks.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")
        };
        let part = self.s.running_label(id);
        let detail = if running && !part.is_empty() {
            dim("running").and(faint(" · ")).and(paint(C_SUB, &part))
        } else if running {
            dim("running")
        } else if let Some((p, f)) = self.tests_counted(id) {
            if f > 0 {
                paint(C_RED, &format!("{f} failed"))
            } else {
                dim(&format!("{p} pass"))
            }
        } else if bad {
            paint(
                C_RED,
                &match st.exit {
                    124 => "timed out".to_string(),
                    137 | 143 => "killed".to_string(),
                    n if n > 0 => format!("exit {n}"),
                    _ => "failed".to_string(),
                },
            )
        } else {
            dim("pass")
        };
        let right = if running && st.start != 0 {
            dim(&self.since(st.start)).and_raw("  ")
        } else if st.end != 0 && st.start != 0 && st.end - st.start >= 100 {
            dim(&dur(st.end - st.start)).and_raw("  ")
        } else {
            Txt::new()
        };
        let lead = self.sp(GUTTER - 1).and(mark).and_raw(" ");
        let words = if bad {
            paint(C_RED, &what)
        } else {
            text(&what)
        };
        let tail = faint(" · ").and(detail);
        let room = self.cw as isize
            - lead.width() as isize
            - tail.width() as isize
            - right.width() as isize
            - 3;
        let words = if room >= 12 && words.width() > room as usize {
            words.truncate(room as usize, &Txt::raw("…"))
        } else {
            words
        };
        let from = self.lines.len();
        self.work(|d| {
            d.add(&r, Color::Default, lead.and(words).and(tail), right);
            if running {
                d.chain_keys(id, GUTTER + 2);
            }
        });
        self.own(&r, from);
    }

    // ---- Catch-up ----

    /// The turn as one card: what was asked and said, what changed, the
    /// checks run, what's left open, and its time and cost.
    fn catch_up(&mut self) {
        let t = self.t;
        let r = self.r.clone();
        let inner = (self.cw.min(CAP_ROW) as isize - GUTTER as isize - 4).clamp(24, 110) as usize;
        let room = inner.saturating_sub(LABEL_W);
        let label = |s: &str| dim(&format!("{s:<w$}", w = LABEL_W));
        let mut rows: Vec<Txt> = Vec::new();
        // asked
        let prompt = fold_pastes(&t.prompt);
        let asked = prompt
            .split('\n')
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(one_line)
            .unwrap_or_else(|| unasked(t).to_string());
        rows.push(label("asked").and(text(&asked).truncate(room, &faint("…"))));
        // said: the last words, their first two rows
        let said = t.items.iter().rev().find_map(|it| match it {
            Item::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        });
        match said {
            Some(s) => {
                let flat = one_line(decisions_in(s).0.trim());
                let flat = if flat.is_empty() { one_line(s) } else { flat };
                let ws = wrap(&sub(&flat), room);
                for (k, w) in ws.iter().take(2).enumerate() {
                    let mut w = w.clone();
                    if k == 1 && ws.len() > 2 {
                        w = w.truncate(room, &faint("…"));
                    }
                    let lead = if k == 0 {
                        label("said")
                    } else {
                        blanks(LABEL_W)
                    };
                    rows.push(lead.and(w));
                }
            }
            None if t.live => rows.push(label("said").and(dim("working…"))),
            None => rows.push(label("said").and(faint("nothing"))),
        }
        // changed
        let mut files: Vec<String> = Vec::new();
        let (mut add, mut del) = (0, 0);
        let mut checks: Vec<(&'static str, Came)> = Vec::new();
        let mut waiting_on = 0;
        for &id in &t.steps {
            let st = self.s.step(id);
            if st.status == Status::Waiting {
                waiting_on += 1;
            }
            if let Some((a, d)) = self.edit_size(id) {
                let p = self.info(id).call.input.path.clone();
                if !files.contains(&p) {
                    files.push(p);
                }
                add += a;
                del += d;
            }
            // Only the main agent's checks: a subagent's are its own.
            if st.parent.is_none() {
                for (n, c) in self.checks_of(id) {
                    match checks.iter_mut().find(|x| x.0 == n) {
                        Some(x) => x.1 = c,
                        None => checks.push((n, c)),
                    }
                }
            }
        }
        if !files.is_empty() {
            let mut l = label("changed")
                .and(text(&plural(files.len(), "file")))
                .and_raw("  ")
                .and(paint(C_GREEN, &format!("+{add}")));
            if del > 0 {
                l.push(paint(C_RED, &format!(" −{del}")));
            }
            let names: Vec<String> = files
                .iter()
                .map(|f| base_after_slash(f).to_string())
                .collect();
            l.push(faint(&format!("  {}", names.join(", "))));
            rows.push(l.truncate(inner, &faint("…")));
        }
        if !checks.is_empty() {
            let mut l = label("checks");
            for (k, (n, c)) in checks.iter().enumerate() {
                if k > 0 {
                    l.push_str("  ", Style::default());
                }
                l.push(match c {
                    Came::Running => paint(C_ORANGE, &format!("✱ {n}")),
                    Came::Passed => paint(C_GREEN, "✓").and_raw(" ").and(sub(n)),
                    Came::Failed => paint(C_RED, &format!("✗ {n}")),
                });
            }
            rows.push(l.truncate(inner, &faint("…")));
        }
        // open
        let decisions = open_in_turn(self.s, self.ti);
        let mut open: Vec<Txt> = Vec::new();
        if decisions > 0 {
            open.push(paint(
                C_YELLOW,
                &format!("? {} yours to make", plural(decisions, "decision")),
            ));
        }
        if waiting_on > 0 {
            open.push(paint(
                C_YELLOW,
                &format!("● {} waiting on you", plural(waiting_on, "step")),
            ));
        }
        if self.needs_here() && decisions == 0 {
            let reason = if self.o.needs.reason.is_empty() {
                "needs you".to_string()
            } else {
                self.o.needs.reason.clone()
            };
            open.push(paint(C_YELLOW, &format!("! {reason}")));
        }
        let is_open = !open.is_empty();
        if is_open {
            let mut l = label("open");
            for (k, o) in open.into_iter().enumerate() {
                if k > 0 {
                    l.push(faint(" · "));
                }
                l.push(o);
            }
            rows.push(l.truncate(inner, &faint("…")));
        }
        // Its heading: when, how it ended; its time and cost on the right.
        let mut head = Txt::new();
        if t.start != 0 {
            head.push(sub(&clock(t.start, self.o.offset(t.start))));
            head.push_str(" ", Style::default());
        }
        head.push(self.mark());
        if t.live {
            head.push(paint(C_ORANGE, "✱ working"));
        } else {
            head.push(dim(&format!("turn {}", t.n)));
        }
        let end = if t.live { self.o.now } else { t.end };
        let mut facts = Vec::new();
        let n = t.step_count();
        if n > 0 {
            facts.push(plural(n, "step"));
        }
        if t.start != 0 && end - t.start >= 100 {
            facts.push(if t.live {
                self.since(t.start)
            } else {
                dur(end - t.start)
            });
        }
        let m = money(t.cost);
        if !m.is_empty() {
            facts.push(m);
        }
        let head_r = dim(&facts.join(" · "));
        let picked = self.o.selected == r;
        let frame = if picked {
            C_ORANGE
        } else if is_open {
            C_YELLOW
        } else if t.live {
            C_SUB
        } else {
            C_FAINT
        };
        let foot = if picked {
            key_hint("enter", "opens it in Normal")
        } else {
            Txt::new()
        };
        self.frame_box(&r, GUTTER, head, head_r, rows, foot, inner, frame, true);
        self.bare();
    }

    // ---- Trail ----

    /// The turn's steps, a line each, under a quiet row naming the turn.
    fn trail(&mut self) {
        let t = self.t;
        let r = self.r.clone();
        let ask = fold_pastes(&t.prompt);
        let ask = ask
            .split('\n')
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(one_line)
            .unwrap_or_else(|| unasked(t).to_string());
        let mut head = Txt::raw(" ").and(faint("── "));
        if t.start != 0 {
            head.push(dim(&clock(t.start, self.o.offset(t.start))));
            head.push_str(" ", Style::default());
        }
        head.push(self.mark());
        let m = self.meta().replace("   ", " · ");
        let right = if m.is_empty() {
            Txt::new()
        } else {
            dim(&m).and_raw("  ")
        };
        let room = self.cw as isize - head.width() as isize - right.width() as isize - 6;
        head.push(sub(&ask).truncate(room.max(8) as usize, &faint("…")));
        let fill = self.cw as isize - head.width() as isize - right.width() as isize - 3;
        if fill > 1 {
            head.push_str(" ", Style::default());
            head.push(faint(&"─".repeat(fill as usize - 1)));
        }
        if !self.lines.is_empty() || self.ti > 0 {
            self.bare();
        }
        self.add(&r, Color::Default, head, right);
        for it in &t.items {
            if let Item::Step(id) = it
                && !hidden(self.s.step(*id))
            {
                self.trail_row(*id);
            }
        }
        if t.stopped {
            let l = Txt::raw("   ").and(dim("⏹ stopped"));
            self.add("", Color::Default, l, Txt::new());
        } else if !t.err.is_empty() && !t.live {
            let l = Txt::raw("   ").and(paint(C_RED, &format!("✗ {}", one_line(&t.err))));
            self.add("", Color::Default, l, Txt::new());
        }
    }

    /// A step as one line: when it began, how it went, what it was, a
    /// detail (+a −b, the exit code), how long it took.
    fn trail_row(&mut self, id: StepId) {
        let st = self.s.step(id);
        let r = self.step_ref(st);
        let when = if st.start != 0 {
            clock(st.start, self.o.offset(st.start))
        } else {
            "     ".to_string()
        };
        let bad = !matches!(st.status, Status::Running | Status::Waiting) && self.bad(id);
        let mark = match st.status {
            Status::Running => paint(C_ORANGE, "✱"),
            _ if bad => paint(C_RED, "✗"),
            Status::Ok => paint(C_GREEN, "✓"),
            Status::Waiting => paint(C_YELLOW, "●"),
            Status::Denied => dim("⊘"),
            Status::Lost => dim("◌"),
            Status::Failed => paint(C_RED, "✗"),
        };
        let mut label = self.label(id);
        if bad {
            label = paint(C_RED, &label.plain());
        }
        let mut cells = Txt::new();
        if let Some((a, d)) = self.edit_size(id) {
            cells.push(paint(C_GREEN, &format!("+{a}")));
            if d > 0 {
                cells.push(paint(C_RED, &format!(" −{d}")));
            }
        }
        if st.status == Status::Waiting {
            cells.push(paint(C_YELLOW, "waiting on you"));
        }
        let part = self.s.running_label(id);
        if !part.is_empty() {
            cells.push(paint(C_SUB, &part));
        }
        if bad {
            let why = match self.tests_counted(id) {
                Some((_, f)) if f > 0 => format!("{f} failed"),
                _ => match st.exit {
                    124 => "timed out".to_string(),
                    137 | 143 => "killed".to_string(),
                    n if n > 0 => format!("exit {n}"),
                    _ => "failed".to_string(),
                },
            };
            if !cells.is_empty() {
                cells.push(faint(" · "));
            }
            cells.push(paint(C_RED, &why));
        }
        if !cells.is_empty() {
            cells = faint("  · ").and(cells);
        }
        let right = if st.status == Status::Running && st.start != 0 {
            dim(&self.since(st.start)).and_raw("  ")
        } else if st.end != 0 && st.start != 0 && st.end - st.start >= 100 {
            dim(&dur(st.end - st.start)).and_raw("  ")
        } else {
            Txt::new()
        };
        let lead = Txt::raw("   ")
            .and(dim(&when))
            .and_raw("  ")
            .and(mark)
            .and_raw(" ");
        let room = self.cw as isize
            - lead.width() as isize
            - cells.width() as isize
            - right.width() as isize
            - 3;
        if room >= 12 && label.width() > room as usize {
            label = label.truncate(room as usize, &Txt::raw("…"));
        }
        let picked = self.o.selected == r;
        self.add(&r, Color::Default, lead.and(label).and(cells), right);
        if picked && st.status == Status::Running {
            self.chain_keys(id, 12);
        }
    }
}
