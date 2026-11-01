//! A turn drawn in one of the conversation's other views (Options.preset),
//! each answering one question:
//!
//! - Chat, "what did we say?": your messages in bubbles on the right, the
//!   agent's words (markdown) in bubbles on the left, and what it asks of
//!   you as a line in its bubble; no steps, no output. The bubble's foot
//!   says how many steps the turn took, and opens it in Normal.
//! - Code, "what did it change?": each turn's edits as their diffs, file by
//!   file, a few rows each until opened, and a line for each check (tests,
//!   builds, linters) with how it came out and how long it took.
//! - Catch-up, "what happened while I was away?": one card for every turn
//!   since you last looked at the session (Options.since), what needs you
//!   first; the turns before it a row each.
//! - Trail, "where did the time go?": a line a step on a time axis, quick
//!   steps of a kind merged, a bar for how long each took, and the turn's
//!   time split into thinking, tools and waiting on you.
//!
//! Every view is drawn from the turn as the session already holds it, from
//! the generic model only (Kind, Status, exit codes, the shell's command),
//! whichever agent ran the session. New in the Rust port: Go rush had no
//! such views.

use super::super::Preset;
use super::super::decisions::{decisions_of, open_list};
use super::*;
use crate::decisions::decisions_in;

/// Diff rows a file shows in Code before "enter opens the whole diff".
const CODE_CAP: usize = 12;
/// The width of a catch-up card's labels ("Said to you ").
const LABEL_W: usize = 14;
/// The most turns a catch-up card covers.
const CATCH_MOST: usize = 200;
/// The turns a catch-up card covers when you've not looked before.
const CATCH_FIRST: usize = 4;
/// A step quicker than this merges with the steps of its kind beside it
/// on the trail.
const QUICK: Ms = 10_000;

/// The checks a shell command runs, by kind ("tests", "clippy", "fmt",
/// "lint", "build", "check", "typecheck", "vet"), in the order it runs
/// them: read from the command's words, the common tools of any language,
/// so it holds whichever agent ran it.
pub fn check_names(cmd: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for seg in segments_of(cmd) {
        if let Some(n) = check_in(&words_of(seg))
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

/// The command's parts as the shell runs them, split at ; | & and new
/// lines; a heredoc's body is the program's input, not commands, and is
/// left out.
fn segments_of(cmd: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut until: Option<String> = None;
    for line in cmd.split('\n') {
        if let Some(end) = &until {
            if line.trim() == end {
                until = None;
            }
            continue;
        }
        if let Some(at) = line.find("<<") {
            let w = line[at + 2..]
                .trim_start_matches(['-', '~'])
                .trim_start()
                .split(|c: char| c.is_whitespace() || c == ';' || c == ')')
                .next()
                .unwrap_or("")
                .trim_matches(['\'', '"']);
            if !w.is_empty() && !w.starts_with('<') {
                until = Some(w.to_string());
            }
        }
        // At ; | and &, but not the & of a redirect (2>&1, &>).
        let b = line.as_bytes();
        let mut from = 0;
        for (i, &c) in b.iter().enumerate() {
            let redirect = c == b'&' && (i > 0 && b[i - 1] == b'>' || b.get(i + 1) == Some(&b'>'));
            if matches!(c, b';' | b'|' | b'&') && !redirect {
                out.push(&line[from..i]);
                from = i + 1;
            }
        }
        out.push(&line[from..]);
    }
    out
}

/// A command without the cd it starts with (cd dir && or cd dir;): what
/// it runs, said short.
fn no_cd(cmd: &str) -> &str {
    static CD: LazyLock<Regex> =
        LazyLock::new(|| re(r#"^cd\s+("[^"]+"|'[^']+'|\S+?)\s*(?:&&|;)\s*"#));
    let cmd = cmd.trim();
    match CD.find(cmd) {
        Some(m) => &cmd[m.end()..],
        None => cmd,
    }
}

/// A part's words, quotes and brackets trimmed off.
fn words_of(seg: &str) -> Vec<&str> {
    seg.split_whitespace()
        .map(|w| w.trim_matches(['(', ')', '"', '\'', '`']))
        .filter(|w| !w.is_empty())
        .collect()
}

/// The part of a command that runs its first check, as a check line
/// names it (cargo test -p x, not the cd before it); the first line when
/// none does.
fn check_command(cmd: &str) -> String {
    segments_of(cmd)
        .into_iter()
        .find(|seg| check_in(&words_of(seg)).is_some())
        .map(|seg| {
            let seg = seg.trim();
            one_line(seg.strip_suffix("2>").unwrap_or(seg).trim_end())
        })
        .unwrap_or_else(|| one_line(first_line(no_cd(cmd))))
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

/// The first turn a catch-up card covers, and whether any turn ended
/// after since: the turns since you last looked (at most CATCH_MOST);
/// the last CATCH_FIRST when you've never looked (since 0); the last one
/// when nothing is new. Read from the end, so it costs what the card does.
pub(crate) fn catch_from(s: &Session, since: Ms) -> (usize, bool) {
    let n = s.turns.len();
    if n == 0 {
        return (0, false);
    }
    if since <= 0 {
        return (n.saturating_sub(CATCH_FIRST), true);
    }
    let mut i = n;
    while i > 0 && n - i < CATCH_MOST {
        let t = &s.turns[i - 1];
        if t.live || last_heard(s, t) > since {
            i -= 1;
        } else {
            break;
        }
    }
    if i == n { (n - 1, false) } else { (i, true) }
}

/// When turn t last did anything: its end, unless that came over an hour
/// after its last step (a turn left open, ended when rush read it), then
/// its last step's.
fn last_heard(s: &Session, t: &Turn) -> Ms {
    let steps = t
        .steps
        .iter()
        .map(|&id| {
            let st = s.step(id);
            st.end.max(st.start)
        })
        .max()
        .unwrap_or(0)
        .max(t.start);
    if t.end > steps + 3_600_000 && !t.steps.is_empty() {
        steps
    } else {
        t.end.max(t.start)
    }
}

/// The first sentence of s as plain words: its first line up to the end
/// of the sentence that starts it.
fn first_sentence(s: &str) -> String {
    let line = crate::text::first_plain(s);
    let b = line.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if matches!(c, b'.' | b'!' | b'?') && (i + 1 == b.len() || b[i + 1] == b' ') {
            return line[..=i].to_string();
        }
    }
    line.trim_end_matches(':').to_string()
}

/// s as plain words on one line: markdown's marks, list bullets and
/// headings' hashes taken off.
fn flat(s: &str) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|l| {
            let l = l.trim_start();
            let l = l.trim_start_matches('#').trim_start();
            let l = ["- ", "* ", "• ", "> "]
                .iter()
                .find_map(|b| l.strip_prefix(b))
                .unwrap_or(l);
            crate::text::strip_marks(l)
        })
        .collect();
    one_line(&lines.join(" "))
}

/// What follows the first sentence of s, on one line.
fn after_first(s: &str) -> String {
    let flat = flat(s.trim());
    let first = first_sentence(s);
    match flat.find(first.trim_end_matches(':')) {
        Some(at) => flat[at + first.trim_end_matches(':').len()..]
            .trim_start_matches(':')
            .trim()
            .to_string(),
        None => String::new(),
    }
}

/// n with its thousands apart: 1,240.
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A bar frac of w cells long, in eighths, never shorter than one.
fn bar(frac: f64, w: usize) -> String {
    const PART: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let e = ((frac.clamp(0.0, 1.0) * (w * 8) as f64).round() as usize).max(1);
    let mut s = "█".repeat(e / 8);
    s.push_str(PART[e % 8]);
    s
}

/// How long ms is, said short: 0 for nothing.
fn took(ms: Ms) -> String {
    if ms <= 0 { "0".into() } else { dur(ms) }
}

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Came {
    Running,
    Passed,
    Failed,
}

/// A file a turn's edits changed: the edits, in order, and the lines they
/// added and took away; or what happened to it (deleted, moved).
struct FileEdits {
    path: String,
    ids: Vec<StepId>,
    add: usize,
    del: usize,
    note: &'static str,
}

/// A row of the trail: one step, or quick steps of a kind side by side.
struct Leg {
    ids: Vec<StepId>,
    verb: &'static str,
    names: Vec<String>,
    start: Ms,
    end: Ms,
    running: bool,
    waiting: bool,
    bad: bool,
}

impl Drawer<'_> {
    /// The turn drawn as p has it.
    pub(crate) fn preset(&mut self, p: Preset) {
        self.no_hooks = true;
        self.rule = false;
        match p {
            Preset::Chat => self.chat(),
            Preset::Code => self.code_view(),
            Preset::CatchUp => self.catch_up(),
            Preset::Trail => self.trail(),
            Preset::Normal => self.open(),
        }
    }

    // ---- what each step is to a view ----

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

    /// The tests a step's output counted: passed and failed. A run that
    /// failed has its card; one that passed is counted from the tallies
    /// test runners print ("108 passed", "Tests: 12 passed"), summed over
    /// the runs a command made.
    fn tests_counted(&self, id: StepId) -> Option<(usize, usize)> {
        if let Some(c) = self.step_cards(id).iter().find(|c| c.kind == "tests") {
            return Some((c.passed, c.failed));
        }
        static PASSED: LazyLock<Regex> = LazyLock::new(|| re(r"(\d+) (?:tests? )?passed"));
        let st = self.s.step(id);
        if st.kind != Kind::Shell
            || !check_names(&self.info(id).call.input.command).contains(&"tests")
        {
            return None;
        }
        let out = bash_out(st, &self.info(id).out);
        let mut n = None;
        for m in PASSED.captures_iter(&out) {
            let k: usize = m[1].parse().unwrap_or(0);
            n = Some(n.unwrap_or(0) + k);
        }
        n.map(|p| (p, 0))
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

    /// The main agent's checks in turn t, in order: its shell steps that
    /// run tests, builds, linters.
    fn check_steps(&self, t: &Turn) -> Vec<StepId> {
        t.items
            .iter()
            .filter_map(|it| match it {
                Item::Step(id) => Some(*id),
                _ => None,
            })
            .filter(|&id| {
                let st = self.s.step(id);
                !hidden(st) && st.kind == Kind::Shell && !self.checks_of(id).is_empty()
            })
            .collect()
    }

    /// The files turn t's edits changed, its subagents' too, in the order
    /// first changed.
    fn changed_files(&self, t: &Turn) -> Vec<FileEdits> {
        let mut out: Vec<FileEdits> = Vec::new();
        for &id in &t.steps {
            let st = self.s.step(id);
            if st.status != Status::Ok {
                continue;
            }
            let note = match st.kind {
                Kind::Edit | Kind::Write => "",
                Kind::Notebook => "notebook",
                Kind::Delete => "deleted",
                Kind::Move => "moved",
                _ => continue,
            };
            let path = self.info(id).call.input.path.clone();
            if path.is_empty() {
                continue;
            }
            let (a, d) = self.edit_size(id).unwrap_or((0, 0));
            match out.iter_mut().find(|f| f.path == path) {
                Some(f) => {
                    f.ids.push(id);
                    f.add += a;
                    f.del += d;
                    if !note.is_empty() {
                        f.note = note;
                    }
                }
                None => out.push(FileEdits {
                    path,
                    ids: vec![id],
                    add: a,
                    del: d,
                    note,
                }),
            }
        }
        out
    }

    /// What you asked, on one line; what woke the turn when you didn't.
    fn asked_line(t: &Turn) -> String {
        fold_pastes(&t.prompt)
            .split('\n')
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(one_line)
            .unwrap_or_else(|| unasked(t).to_string())
    }

    /// Whether you started turn t with words or images of your own.
    fn yours(t: &Turn) -> bool {
        let (_, _, wake) = woke(t);
        t.from.is_empty()
            && !wake
            && rush_ask(t).is_empty()
            && (!t.prompt.trim().is_empty() || !t.images.is_empty())
    }

    /// "+a −d" in green and red.
    fn add_del(add: usize, del: usize) -> Txt {
        let mut l = paint(C_GREEN, &format!("+{add}"));
        l.push(paint(C_RED, &format!(" −{del}")));
        l
    }

    // ---- Chat ----

    /// What you said on the right, what the agent said on the left, each
    /// in a bubble; what it asks of you a line in its bubble.
    fn chat(&mut self) {
        let t = self.t;
        if self.ti > 0 {
            self.bare();
        }
        let ask = t.prompt.trim();
        if Self::yours(t) && !t.command.contains('\n') {
            let mut imgs = Vec::new();
            if !t.images.is_empty() && !t.prompt.contains("[Image #") {
                imgs = image_names(&t.images);
            }
            let when = if t.start != 0 {
                clock(t.start, self.o.offset(t.start))
            } else {
                String::new()
            };
            self.you_bubble(ask, &imgs, &when);
        } else {
            let l = self
                .sp(1)
                .and(faint("◇ "))
                .and(dim(&Self::asked_line(t)))
                .truncate(self.cw.saturating_sub(2), &faint("…"));
            let r = self.r.clone();
            self.add(&r, Color::Default, l, Txt::new());
        }
        // The agent's words, a bubble for each stretch between messages
        // you sent while it worked.
        let mut said: Vec<(&str, bool)> = Vec::new();
        for it in &t.items {
            match it {
                Item::Text { text, answer } if !text.trim().is_empty() => {
                    said.push((text.as_str(), *answer));
                }
                Item::Interject(x) => {
                    if !said.is_empty() {
                        self.agent_bubble(&said, false);
                        said.clear();
                    }
                    let imgs = image_names(&x.images);
                    self.you_bubble(x.text.trim(), &imgs, "while it worked");
                }
                _ => {}
            }
        }
        if !said.is_empty() || t.live || t.step_count() > 0 || self.needs_here() {
            self.agent_bubble(&said, true);
        }
    }

    /// The widest a bubble's words may run.
    fn bubble_room(&self) -> usize {
        let cw = self.cw.max(20);
        let w = (cw * 3 / 5).clamp(24, 100).min(cw.saturating_sub(4));
        w.saturating_sub(4).max(10)
    }

    /// Your message as a bubble on the right: its words and chips, when
    /// on its foot.
    fn you_bubble(&mut self, ask: &str, imgs: &[String], when: &str) {
        let room = self.bubble_room();
        let r = self.r.clone();
        let mut chips: Vec<Chip> = Vec::new();
        let mut rows: Vec<Txt> = Vec::new();
        for (text, paste) in paste_parts(ask) {
            if paste {
                rows.extend(paste_rows(&r, &text, room));
                continue;
            }
            let mut rest = String::new();
            let mut last = 0;
            let marked = text.contains('[');
            for m in ATTACH.find_iter(if marked { &text } else { "" }) {
                rest.push_str(&text[last..m.start()]);
                chips.push(chip_of(m.as_str()));
                last = m.end();
            }
            rest.push_str(&text[last..]);
            rows.extend(ask_rows(&rest, room, |s| styled_ask(s, C_WHITE)));
        }
        for (i, name) in imgs.iter().enumerate() {
            chips.push(Chip {
                shown: format!("▣ {name}"),
                href: format!("/image/{}", i + 1),
            });
        }
        rows.extend(chip_rows(&r, &chips, room));
        if rows.is_empty() {
            return;
        }
        let foot = dim(when);
        self.bubble(true, rows, foot, "", C_BLUE, room);
    }

    /// The agent's words as a bubble on the left; the last of the turn
    /// carries what it asks of you and, on its foot, who said it, when,
    /// and the steps it took (enter opens the turn in Normal).
    fn agent_bubble(&mut self, said: &[(&str, bool)], last: bool) {
        let t = self.t;
        let room = self.bubble_room();
        let mut rows: Vec<Txt> = Vec::new();
        for (text, answer) in said {
            let (before, ds, after) = decisions_in(text);
            let words = if ds.is_empty() {
                text.to_string()
            } else {
                format!("{}\n\n{}", before.trim_end(), after.trim_start())
            };
            if !rows.is_empty() {
                rows.push(Txt::new());
            }
            let c = if *answer { C_TEXT } else { C_SUB };
            rows.extend(self.bubble_md(&words, room, c));
        }
        if last {
            let asks = self.chat_asks(room);
            if !asks.is_empty() && !rows.is_empty() {
                rows.push(Txt::new());
            }
            rows.extend(asks);
        }
        if rows.is_empty() {
            rows.push(if t.live {
                dim("working…")
            } else {
                faint("said nothing")
            });
        }
        let who = if self.o.speaker.is_empty() {
            "Agent"
        } else {
            &self.o.speaker
        };
        let mut foot = paint(C_ORANGE.bold(), who);
        let r = if last { self.r.clone() } else { String::new() };
        if last {
            // Which command of a running chain runs now: 2/3 cargo test.
            let part = t
                .items
                .iter()
                .filter_map(|it| match it {
                    Item::Step(id) => Some(self.s.running_label(*id)),
                    _ => None,
                })
                .find(|p| !p.is_empty());
            if let Some(p) = part.filter(|_| t.live) {
                foot.push(faint(" · "));
                foot.push(paint(C_ORANGE, "✱ running "));
                foot.push(paint(C_SUB, &p));
            } else if t.live {
                foot.push(faint(" · "));
                foot.push(paint(C_ORANGE, "✱ working"));
                if t.start != 0 {
                    let s = self.since(t.start);
                    foot.push(dim(&format!(" {s}")));
                }
            } else if t.end != 0 {
                foot.push(dim(&format!(" · {}", clock(t.end, self.o.offset(t.end)))));
            }
            if t.stopped {
                foot.push(faint(" · "));
                foot.push(dim("⏹ stopped"));
            } else if !t.err.is_empty() && !t.live {
                foot.push(faint(" · "));
                foot.push(paint(C_RED, "✗ failed"));
            }
            let n = t.step_count();
            if n > 0 {
                foot.push(faint(" · "));
                foot.push(sub(&plural(n, "step")));
                let files = self.changed_files(t).len();
                if files > 0 {
                    foot.push(dim(&format!("  {}", plural(files, "file"))));
                }
            }
        }
        self.bubble(false, rows, foot, &r, SPINE_TOOL, room);
    }

    /// What the turn asks of you, as lines for its bubble: its open
    /// decisions, its questions and waiting permissions, and what the
    /// last message needs of you.
    fn chat_asks(&mut self, room: usize) -> Vec<Txt> {
        let t = self.t;
        let mut out = Vec::new();
        let open = open_list(self.s, self.ti);
        let all = decisions_of(t).len();
        if !open.is_empty() {
            let mut l = paint(
                C_YELLOW,
                &format!("? {} for you", plural(open.len(), "decision")),
            );
            if let Some(d) = open.first() {
                l.push(dim(&format!(": {}", one_line(&d.topic))));
            }
            out.push(l.truncate(room, &faint("…")));
        } else if all > 0 {
            out.push(dim(&format!("✓ {} made", plural(all, "decision"))));
        }
        for it in &t.items {
            let Item::Step(id) = it else { continue };
            let st = self.s.step(*id);
            if hidden(st) {
                continue;
            }
            let waiting = st.status == Status::Waiting;
            if !(waiting || matches!(st.kind, Kind::Question | Kind::PlanMode)) {
                continue;
            }
            let what = self.label(*id).plain();
            let l = if waiting {
                paint(C_YELLOW, "? ").and(text(&what))
            } else {
                dim("✓ asked ").and(sub(&what))
            };
            out.push(l.truncate(room, &faint("…")));
        }
        if self.needs_here() && open.is_empty() {
            let reason = if self.o.needs.reason.is_empty() {
                "needs you".to_string()
            } else {
                one_line(&self.o.needs.reason)
            };
            out.push(paint(C_YELLOW, &format!("! {reason}")).truncate(room, &faint("…")));
        }
        out
    }

    /// Markdown laid out room wide in colour c, for a bubble.
    fn bubble_md(&self, body: &str, room: usize, c: Style) -> Vec<Txt> {
        let o = Options {
            width: room + 2,
            verbose: self.o.verbose,
            zone: self.o.zone,
            zone_at: self.o.zone_at,
            ..Options::default()
        };
        let mut child = Drawer::new(self.s, self.t, &o, self.p);
        child.cw = room + 2;
        child.markdown(body.trim(), 1, c, true);
        let mut rows: Vec<Txt> = child
            .lines
            .into_iter()
            .map(|l| {
                let mut t = Txt(l.line.spans).skip(1);
                while let Some(last) = t.0.last_mut() {
                    let n = last.text.trim_end_matches(' ').len();
                    if n == 0 {
                        t.0.pop();
                        continue;
                    }
                    last.text.truncate(n);
                    break;
                }
                t
            })
            .collect();
        while rows.first().is_some_and(Txt::is_blank) {
            rows.remove(0);
        }
        while rows.last().is_some_and(Txt::is_blank) {
            rows.pop();
        }
        rows
    }

    /// A bubble of rows, on the right when mine, framed in frame, its foot
    /// on the bottom edge: at its right end for you, its left for the
    /// agent. The bottom edge carries r, the rest is owned by it.
    fn bubble(
        &mut self,
        mine: bool,
        rows: Vec<Txt>,
        foot: Txt,
        r: &str,
        frame: Style,
        room: usize,
    ) {
        let mut foot = foot;
        let widest = rows.iter().map(Txt::width).max().unwrap_or(0);
        let inner = widest.max(foot.width() + 2).min(room).max(4);
        if foot.width() + 2 > inner {
            foot = foot.truncate(inner.saturating_sub(2), &faint("…"));
        }
        let outer = inner + 4;
        let lead = if mine {
            self.cw.saturating_sub(outer + 1).max(1)
        } else {
            1
        };
        let pad = || Txt::raw(" ").and(blanks(lead - 1));
        let from = self.lines.len();
        let top = pad().and(paint(frame, &format!("╭{}╮", "─".repeat(inner + 2))));
        self.add("", Color::Default, top, Txt::new());
        for row in rows {
            let row = if row.width() > inner {
                row.truncate(inner, &faint("…"))
            } else {
                row
            };
            let w = row.width();
            let l = pad()
                .and(paint(frame, "│"))
                .and_raw(" ")
                .and(row)
                .and(blanks(inner - w))
                .and_raw(" ")
                .and(paint(frame, "│"));
            self.add("", Color::Default, l, Txt::new());
        }
        let fw = foot.width();
        let fill = (inner + 2).saturating_sub(fw + 2);
        let bottom = if mine {
            pad()
                .and(paint(frame, &format!("╰{}", "─".repeat(fill + 1))))
                .and_raw(" ")
                .and(foot)
                .and(paint(frame, "╯"))
        } else {
            pad()
                .and(paint(frame, "╰"))
                .and_raw(" ")
                .and(foot)
                .and_raw(" ")
                .and(paint(frame, &format!("{}╯", "─".repeat(fill))))
        };
        self.add(r, Color::Default, bottom, Txt::new());
        if !r.is_empty() {
            self.own(r, from);
        }
    }

    // ---- Code ----

    /// The turn's code changes: a heading naming the turn and what it
    /// changed, each file's diff (a few rows of it until opened), and a
    /// line for each check it ran.
    fn code_view(&mut self) {
        let t = self.t;
        let r = self.r.clone();
        let files = self.changed_files(t);
        let checks = self.check_steps(t);
        if self.ti > 0 {
            self.bare();
        }
        let (add, del) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.add, d + f.del));
        let right = if files.is_empty() {
            dim("no code changes").and(faint(" ──"))
        } else {
            sub(&plural(files.len(), "file"))
                .and_raw("  ")
                .and(Self::add_del(add, del))
                .and(faint(" ──"))
        };
        let mut head = faint("── ")
            .and(self.mark())
            .and(dim(&format!("turn {}", t.n)))
            .and(faint(" · "));
        let room = self.cw as isize - head.width() as isize - right.width() as isize - 8;
        let ask = format!("\"{}\"", Self::asked_line(t));
        head.push(sub(&ask).truncate(room.max(8) as usize, &faint("…\"")));
        let fill = self.cw as isize - head.width() as isize - right.width() as isize - 5;
        head.push_str(" ", Style::default());
        head.push(faint(&"─".repeat(fill.max(1) as usize)));
        let l = self.sp(0).and(head);
        self.add(&r, Color::Default, l, right.and_raw(" "));
        for (k, f) in files.iter().enumerate() {
            self.code_file(k, f);
        }
        for id in &checks {
            self.check_row(*id, &checks);
        }
        if t.live {
            let mut l = self.sp(1).and(paint(C_ORANGE, "✱ working"));
            if t.start != 0 {
                let s = self.since(t.start);
                l.push(dim(&format!(" · {s}")));
            }
            self.add("", Color::Default, l, Txt::new());
        }
    }

    /// A file's row and its diff: CODE_CAP rows of it until opened (its
    /// row's ref), whole once it is.
    fn code_file(&mut self, k: usize, f: &FileEdits) {
        let r = format!("{}:file:{k}", self.r);
        let open = self.o.open.get(&r).copied().unwrap_or(false);
        let name = self.file_link(&f.path, text(&self.p.rel(&f.path)));
        let right = if f.note.is_empty() {
            Self::add_del(f.add, f.del).and_raw("  ")
        } else {
            dim(f.note).and_raw("  ")
        };
        let l = self.sp(1).and(name);
        let room = self.cw.saturating_sub(right.width() + 3);
        let l = l.truncate(room, &faint("…"));
        let from = self.lines.len();
        self.add(&r, Color::Default, l, right);
        if !f.note.is_empty() && f.note != "notebook" {
            return;
        }
        let rows_at = self.lines.len();
        self.force_open = open;
        for (j, &id) in f.ids.iter().enumerate() {
            if j > 0 {
                let t = self.sp(2).and(faint("  ⋯"));
                self.add("", BG_WELL, t, Txt::new());
            }
            if !self.diff(id, 3) {
                self.bare_edit(id);
            }
        }
        self.force_open = false;
        let n = self.lines.len() - rows_at;
        if n > CODE_CAP && !open {
            self.lines.truncate(rows_at + CODE_CAP);
            let l = self
                .sp(2)
                .and(faint(&format!(
                    "  … {} more line{}",
                    n - CODE_CAP,
                    if n - CODE_CAP == 1 { "" } else { "s" }
                )))
                .and(dim("  (enter opens the whole diff)"));
            self.add(&r, Color::Default, l, Txt::new());
        } else if open && n > CODE_CAP {
            let l = self.sp(2).and(faint("  ▴ enter folds it"));
            self.add(&r, Color::Default, l, Txt::new());
        }
        self.own(&r, from);
    }

    /// An edit with no patch to show: what it replaced, old over new.
    fn bare_edit(&mut self, id: StepId) {
        let info = self.info(id);
        let lg = lang_for(&info.call.input.path);
        let pad = self.sp(2);
        let w = self.cw.saturating_sub(12);
        let put = |d: &mut Self, sign: char, s: &str| {
            let (bg, mark) = if sign == '-' {
                (BG_DEL, minus_sign())
            } else {
                (BG_ADD, plus_sign())
            };
            let mut hs = HlState::default();
            for l in s.trim_end_matches('\n').split('\n') {
                let hl = super::super::code::paint_code(
                    lg,
                    &mut hs,
                    &expand_tabs(l),
                    C_TEXT,
                    None,
                    false,
                );
                let row = hl.truncate(w, &faint("›"));
                let t = pad
                    .clone()
                    .and(blanks(6))
                    .and(mark.clone())
                    .and_raw(" ")
                    .and(row);
                d.add("", bg, t, Txt::new());
            }
        };
        for e in &info.call.input.edits {
            put(self, '-', &e.old);
            put(self, '+', &e.new);
        }
        if info.call.input.edits.is_empty() && !info.call.input.content.is_empty() {
            put(self, '+', &info.call.input.content);
        }
    }

    /// A check as one line: how it came out, its command, what it said
    /// (tests counted, an exit code), and how long it took; a failure the
    /// same check passed after says so.
    fn check_row(&mut self, id: StepId, all: &[StepId]) {
        let st = self.s.step(id);
        let r = self.step_ref(st);
        let running = matches!(st.status, Status::Running | Status::Waiting);
        let bad = !running && self.bad(id);
        let mark = if running {
            paint(C_ORANGE, "✱")
        } else if bad {
            paint(C_RED, "✗")
        } else {
            paint(C_GREEN, "✓")
        };
        let cmd = check_command(&self.info(id).call.input.command);
        let part = self.s.running_label(id);
        let detail = if running && !part.is_empty() {
            dim("running").and(faint(" · ")).and(paint(C_SUB, &part))
        } else if running {
            dim("running")
        } else if let Some((p, f)) = self.tests_counted(id) {
            if f > 0 {
                paint(C_RED, &format!("{f} failed"))
            } else {
                dim(&format!("{} passed", grouped(p)))
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
            dim("passed")
        };
        let mut detail = detail;
        if bad {
            let names = check_names(&self.info(id).call.input.command);
            let at = all.iter().position(|x| *x == id).unwrap_or(0);
            let fixed = all[at + 1..].iter().any(|&later| {
                !self.bad(later)
                    && self.s.step(later).status == Status::Ok
                    && check_names(&self.info(later).call.input.command)
                        .iter()
                        .any(|n| names.contains(n))
            });
            if fixed {
                detail.push(faint(" → passed next run"));
            }
        }
        let right = if running && st.start != 0 {
            let s = self.since(st.start);
            dim(&s).and_raw("  ")
        } else if st.end != 0 && st.start != 0 && st.end - st.start >= 100 {
            dim(&dur(st.end - st.start)).and_raw("  ")
        } else {
            Txt::new()
        };
        let lead = self.sp(1).and(mark).and_raw(" ");
        let room = self.cw as isize
            - lead.width() as isize
            - detail.width() as isize
            - right.width() as isize
            - 6;
        let mut words = if bad { paint(C_RED, &cmd) } else { text(&cmd) };
        if room >= 12 && words.width() > room as usize {
            words = words.truncate(room as usize, &Txt::raw("…"));
        }
        let l = lead.and(words).and(blanks(3)).and(detail);
        self.add(&r, Color::Default, l, right);
        // A running chain's keys: skip the command, stop it all, pause.
        if running {
            self.chain_keys(id, 4);
        }
    }

    // ---- Catch-up ----

    /// One card for every turn since you last looked, drawn with the last
    /// of them; the turns in it before the last draw nothing, and those
    /// before the card a row each.
    fn catch_up(&mut self) {
        let n = self.s.turns.len();
        let (from, fresh) = catch_from(self.s, self.o.since);
        if self.ti < from {
            self.older_row();
        } else if self.ti + 1 == n {
            self.catch_card(from, fresh);
        }
    }

    /// A turn before the card, as one row: when, what you asked, and what
    /// it took.
    fn older_row(&mut self) {
        let t = self.t;
        let r = self.r.clone();
        let mut right = Vec::new();
        let n = t.step_count();
        if n > 0 {
            right.push(plural(n, "step"));
        }
        let files = self.changed_files(t).len();
        if files > 0 {
            right.push(plural(files, "file"));
        }
        let m = money(t.cost);
        if !m.is_empty() {
            right.push(m);
        }
        let right = dim(&right.join(" · ")).and_raw("  ");
        let mut l = self.sp(1);
        if t.start != 0 {
            l.push(dim(&clock(t.start, self.o.offset(t.start))));
            l.push_str("  ", Style::default());
        }
        l.push(self.mark());
        l.push(faint(&format!("turn {:<4} ", t.n)));
        let room = self.cw.saturating_sub(l.width() + right.width() + 3);
        l.push(sub(&Self::asked_line(t)).truncate(room.max(8), &faint("…")));
        self.add(&r, Color::Default, l, right);
    }

    /// The card: a heading saying since when, what needs you first with
    /// its action, then what was done, changed and checked, what it said
    /// to you last, and what's still going.
    fn catch_card(&mut self, from: usize, fresh: bool) {
        let s = self.s;
        let turns = &s.turns[from..];
        let first_r = format!("t{}", turns[0].n);
        let last_r = self.r.clone();
        // The heading.
        let k = turns.len();
        let since = self.o.since;
        let mut head = if since <= 0 && k == 1 {
            text("The last turn")
        } else if since <= 0 {
            text(&format!("The last {k} turns"))
        } else if fresh {
            text("Since you last looked")
        } else {
            text("Nothing new since you last looked")
        };
        if since > 0 {
            head.push(dim(&format!(" · {} ago", dur(self.o.now - since))));
        }
        if fresh && since > 0 {
            head.push(dim(&format!(" · {}", plural(k, "turn"))));
        }
        let cost: f64 = turns.iter().map(|t| t.cost).sum();
        let m = money(cost);
        if !m.is_empty() {
            head.push(dim(&format!(" · {m}")));
        }
        // What needs you, each row with what it opens.
        let mut needs: Vec<(Txt, String)> = Vec::new();
        for (j, t) in turns.iter().enumerate() {
            let ds = open_list(s, from + j);
            if let Some(d) = ds.first() {
                let mut w = one_line(&d.topic);
                if ds.len() > 1 {
                    w = format!("{w} and {} more", ds.len() - 1);
                }
                needs.push((text(&w), format!("t{}", t.n)));
            }
        }
        let last = &turns[k - 1];
        for &id in &last.steps {
            let st = s.step(id);
            if st.status == Status::Waiting {
                let w = self.label(id).plain();
                needs.push((text(&w), self.step_ref(st)));
            }
        }
        if needs.is_empty() && self.needs_here() {
            let reason = if self.o.needs.reason.is_empty() {
                "it asks something of you".to_string()
            } else {
                one_line(&self.o.needs.reason)
            };
            needs.push((text(&reason), last_r.clone()));
        }
        // Done: the first sentence of each answer; said: the rest of the
        // last one.
        let answers: Vec<&str> = turns.iter().filter_map(|t| t.answer()).collect();
        let done: Vec<String> = answers
            .iter()
            .map(|a| first_sentence(decisions_in(a).0))
            .filter(|x| !x.is_empty())
            .collect();
        let said = answers
            .last()
            .map(|a| after_first(decisions_in(a).0))
            .unwrap_or_default();
        // Changed and checked, over every turn.
        let mut files: Vec<String> = Vec::new();
        let (mut add, mut del) = (0, 0);
        let mut checks: Vec<(&'static str, Came, bool, Option<usize>)> = Vec::new();
        for t in turns {
            for f in self.changed_files(t) {
                add += f.add;
                del += f.del;
                if !files.contains(&f.path) {
                    files.push(f.path);
                }
            }
            for id in self.check_steps(t) {
                // How many passed, from a run that passed and counted them.
                let passed = self
                    .tests_counted(id)
                    .map(|c| c.0)
                    .filter(|&p| p > 0 && !self.bad(id));
                for (n, c) in self.checks_of(id) {
                    match checks.iter_mut().find(|x| x.0 == n) {
                        Some(x) => {
                            x.2 |= x.1 == Came::Failed;
                            x.1 = c;
                            if n == "tests" && c == Came::Passed {
                                x.3 = passed;
                            }
                        }
                        None => {
                            let tests = if n == "tests" { passed } else { None };
                            checks.push((n, c, false, tests));
                        }
                    }
                }
            }
        }
        // Still going: steps running, tasks in the background.
        let mut going: Vec<String> = Vec::new();
        for &id in &last.steps {
            let st = s.step(id);
            if st.status == Status::Running && st.parent.is_none() {
                let part = s.running_label(id);
                let w = if part.is_empty() {
                    self.label(id).plain()
                } else {
                    part
                };
                let at = if st.start != 0 {
                    format!(" · {}", self.since(st.start))
                } else {
                    String::new()
                };
                going.push(format!("{w}{at}"));
            }
        }
        for j in s.running_jobs() {
            if !j.background {
                continue;
            }
            let at = if j.start != 0 {
                format!(" · {}", self.since(j.start))
            } else {
                String::new()
            };
            going.push(format!("{}{at}", one_line(&j.label)));
        }
        if going.is_empty() && last.live {
            let at = if last.start != 0 {
                format!(" · {}", self.since(last.start))
            } else {
                String::new()
            };
            going.push(format!("working{at}"));
        }
        // Laid out.
        let rail_c = if !needs.is_empty() {
            C_YELLOW
        } else if last.live {
            C_ORANGE
        } else {
            C_FAINT
        };
        let inner = self.cw.saturating_sub(GUTTER + 2).max(20);
        let room = inner.saturating_sub(LABEL_W);
        let rail = || paint(rail_c, "┃").and_raw(" ");
        let label = |s: &str, c: Style| paint(c, &format!("{s:<w$}", w = LABEL_W));
        let mut rows: Vec<(Txt, String)> = Vec::new();
        for (w, r) in &needs {
            let action = paint(C_ORANGE, "[answer]");
            let room = room.saturating_sub(action.width() + 3);
            let l = label("? needs you", C_YELLOW)
                .and(w.truncate(room, &faint("…")))
                .and_raw("   ")
                .and(action);
            rows.push((l, r.clone()));
        }
        if !needs.is_empty() {
            rows.push((Txt::new(), String::new()));
        }
        if !done.is_empty() {
            // Sentences run on; words with no stop of their own get one.
            let mut all = String::new();
            for d in &done {
                if !all.is_empty() {
                    all.push_str(if all.ends_with(['.', '!', '?']) {
                        " "
                    } else {
                        "; "
                    });
                }
                all.push_str(d);
            }
            let ws = wrap(&text(&all), room);
            for (i, w) in ws.iter().take(3).enumerate() {
                let mut w = w.clone();
                if i == 2 && ws.len() > 3 {
                    w = w.truncate(room.saturating_sub(1), &faint("…"));
                    if !w.plain().ends_with('…') {
                        w.push(faint("…"));
                    }
                }
                let lead = if i == 0 {
                    label("Done", C_SUB)
                } else {
                    blanks(LABEL_W)
                };
                rows.push((lead.and(w), String::new()));
            }
        }
        if !files.is_empty() {
            let names: Vec<&str> = files.iter().map(|f| base_after_slash(f)).collect();
            let l = label("Changed", C_SUB)
                .and(text(&plural(files.len(), "file")))
                .and_raw("  ")
                .and(Self::add_del(add, del))
                .and(faint(&format!("   ({})", names.join(", "))));
            rows.push((l.truncate(inner, &faint("…)")), String::new()));
        }
        if !checks.is_empty() {
            let mut l = label("Checks", C_SUB);
            for (i, (n, c, failed_once, tests)) in checks.iter().enumerate() {
                if i > 0 {
                    l.push_str("   ", Style::default());
                }
                let word = match tests {
                    Some(p) if *n == "tests" && *c == Came::Passed => {
                        format!("{n} {}", grouped(*p))
                    }
                    _ => n.to_string(),
                };
                l.push(match c {
                    Came::Running => paint(C_ORANGE, &format!("✱ {word}")),
                    Came::Passed => paint(C_GREEN, "✓").and_raw(" ").and(text(&word)),
                    Came::Failed => paint(C_RED, &format!("✗ {word}")),
                });
                if *failed_once && *c == Came::Passed {
                    l.push(faint(" (failed, rerun passed)"));
                }
            }
            rows.push((l.truncate(inner, &faint("…")), String::new()));
        }
        if !said.is_empty() {
            let q = format!("\"{said}\"");
            let l = label("Said to you", C_SUB).and(sub(&q).truncate(room, &faint("…\"")));
            rows.push((l, String::new()));
        }
        if !going.is_empty() {
            let l = label("Still going", C_SUB)
                .and(paint(C_ORANGE, &going.join(" · ")).truncate(room, &faint("…")));
            rows.push((l, String::new()));
        }
        if rows.is_empty() {
            rows.push((faint("nothing to say yet"), String::new()));
        }
        // The heading, the rows on their rail.
        if self.ti > 0 {
            self.bare();
        }
        let from_row = self.lines.len();
        let l = self.sp(1).and(head.truncate(inner + 2, &faint("…")));
        self.add(&first_r, Color::Default, l, Txt::new());
        for (row, r) in rows {
            let l = self.sp(1).and(rail()).and(row);
            self.add(&r, Color::Default, l, Txt::new());
        }
        self.own(&first_r, from_row);
    }

    // ---- Trail ----

    /// The turn on a time axis: what you said, a line a step (quick ones
    /// of a kind merged) with a bar as long as it took against the
    /// slowest, how it ended, and where its time went.
    fn trail(&mut self) {
        let t = self.t;
        let r = self.r.clone();
        if self.ti > 0 {
            self.bare();
        }
        let when = |d: &Self, at: Ms| {
            if at != 0 {
                clock(at, d.o.offset(at))
            } else {
                "     ".to_string()
            }
        };
        // Who started it.
        let mut l = self.sp(1).and(dim(&when(self, t.start))).and_raw("  ");
        if Self::yours(t) {
            l.push(paint(C_BLUE.bold(), "You"));
            l.push(blanks(8));
        } else {
            l.push(faint("◇"));
            l.push(blanks(10));
        }
        let room = self.cw.saturating_sub(l.width() + 3);
        l.push(text(&Self::asked_line(t)).truncate(room.max(8), &faint("…")));
        self.add(&r, Color::Default, l, Txt::new());
        // Its steps.
        let legs = self.legs();
        let now = self.o.now;
        let len = |g: &Leg| {
            let d = if g.running || g.waiting {
                now - g.start
            } else {
                g.end - g.start
            };
            d.max(0)
        };
        let longest = legs.iter().map(len).max().unwrap_or(0);
        let total = (if t.live { now } else { t.end }) - t.start;
        let slow = legs
            .iter()
            .enumerate()
            .max_by_key(|&(_, g)| len(g))
            .filter(|&(_, g)| len(g) >= QUICK && len(g) * 10 >= total.max(1) * 3)
            .map(|(i, _)| i);
        let bw = if self.cw >= 110 {
            16
        } else if self.cw >= 90 {
            12
        } else {
            8
        };
        let ending = self.trail_end();
        for (i, g) in legs.iter().enumerate() {
            let tree = if i + 1 == legs.len() && ending.is_empty() {
                "└ "
            } else {
                "├ "
            };
            let gl = len(g);
            let colour = if g.running {
                C_ORANGE
            } else if g.bad {
                C_RED
            } else if Some(i) == slow {
                C_YELLOW
            } else {
                C_DIM
            };
            let b = if longest > 0 {
                bar(gl as f64 / longest as f64, bw)
            } else {
                bar(0.0, bw)
            };
            let bwid = width(&b);
            let d = if g.running {
                self.since(g.start)
            } else {
                took(gl)
            };
            let mut right = paint(colour, &b)
                .and(blanks(bw.saturating_sub(bwid)))
                .and(dim(&format!(" {d:>7}")));
            right.push(if Some(i) == slow {
                paint(C_YELLOW, "   ← slow")
            } else {
                blanks(9)
            });
            right.push_str("  ", Style::default());
            let lead = self
                .sp(1)
                .and(dim(&when(self, g.start)))
                .and_raw("  ")
                .and(faint(tree));
            let first = g.ids[0];
            let mut tail = Txt::new();
            if g.bad {
                tail.push(paint(C_RED, " ✗"));
            }
            if g.waiting {
                tail.push(paint(C_YELLOW, " · waiting on you"));
            }
            let part = self.s.running_label(first);
            if g.running && !part.is_empty() {
                tail.push(faint(" · "));
                tail.push(paint(C_SUB, &part));
            }
            let room = self
                .cw
                .saturating_sub(lead.width() + right.width() + tail.width() + 3);
            let what = self
                .leg_words(g)
                .truncate(room.max(8), &faint("…"))
                .and(tail);
            let sr = self.step_ref(self.s.step(first));
            let picked = self.o.selected == sr;
            self.add(&sr, Color::Default, lead.and(what), right);
            // Picked, a running chain's keys under it.
            if picked && g.running {
                self.chain_keys(first, 12);
            }
        }
        // How it ended.
        if !ending.is_empty() {
            let end_at = if t.live { now } else { t.end };
            let lead = self
                .sp(1)
                .and(dim(&when(self, end_at)))
                .and_raw("  ")
                .and(faint("└ "));
            let room = self.cw.saturating_sub(lead.width() + 3);
            let l = lead.and(ending.truncate(room.max(8), &faint("…")));
            self.add("", Color::Default, l, Txt::new());
        }
        // Where the time went.
        if t.start != 0 && total > 0 {
            let (tools, waiting) = self.tool_time(&legs);
            let thinking = (total - tools - waiting).max(0);
            let l = self.sp(1).and(blanks(7)).and(dim(&format!(
                "turn {} · thinking {} · tools {} · waiting on you {}",
                took(total),
                took(thinking),
                took(tools),
                took(waiting)
            )));
            self.add("", Color::Default, l, Txt::new());
        }
    }

    /// How the turn ended, for the trail's last line: its reply's first
    /// sentence, still working, stopped, or failed.
    fn trail_end(&mut self) -> Txt {
        let t = self.t;
        if t.live {
            let mut l = paint(C_ORANGE, "✱ working");
            if t.start != 0 {
                let s = self.since(t.start);
                l.push(dim(&format!(" · {s}")));
            }
            return l;
        }
        if t.stopped {
            return dim("⏹ stopped");
        }
        if !t.err.is_empty() {
            return paint(C_RED, &format!("✗ {}", one_line(&t.err)));
        }
        match t.answer() {
            Some(a) => {
                let first = first_sentence(decisions_in(a).0);
                let first = if first.is_empty() {
                    first_sentence(a)
                } else {
                    first
                };
                sub(&format!("{:<9}", "reply")).and(text(&format!("\"{first}\"")))
            }
            None => Txt::new(),
        }
    }

    /// The turn's steps as the trail's rows: a row a step, steps of a
    /// kind quicker than QUICK side by side as one.
    fn legs(&mut self) -> Vec<Leg> {
        let mut out: Vec<Leg> = Vec::new();
        let t = self.t;
        for it in &t.items {
            let Item::Step(id) = it else { continue };
            let st = self.s.step(*id);
            if hidden(st) {
                continue;
            }
            let (verb, name) = self.leg_of(*id);
            let running = st.status == Status::Running;
            let waiting = st.status == Status::Waiting;
            let bad = !running && !waiting && self.bad(*id);
            let end = if st.end != 0 { st.end } else { st.start };
            let quick = !running && !waiting && !bad && end - st.start < QUICK && verb != "shell";
            if quick
                && let Some(g) = out.last_mut()
                && g.verb == verb
                && !g.running
                && !g.waiting
                && !g.bad
                && g.end - g.start < QUICK
                && g.verb != "shell"
            {
                g.ids.push(*id);
                if !g.names.contains(&name) {
                    g.names.push(name);
                }
                g.end = g.end.max(end);
                continue;
            }
            out.push(Leg {
                ids: vec![*id],
                verb,
                names: vec![name],
                start: st.start,
                end,
                running,
                waiting,
                bad,
            });
        }
        out
    }

    /// A step's kind on the trail and what it was on: read and a file,
    /// search and its pattern, a shell step's command.
    fn leg_of(&mut self, id: StepId) -> (&'static str, String) {
        let st = self.s.step(id);
        let info = self.info(id);
        let x = &info.call.input;
        let base = |p: &str| base_after_slash(p).to_string();
        match st.kind {
            Kind::Read => ("read", base(&x.path)),
            Kind::Search | Kind::Glob => {
                let p = if x.pattern.is_empty() {
                    &x.query
                } else {
                    &x.pattern
                };
                ("search", format!("\"{}\"", one_line(p)))
            }
            Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move => {
                ("edit", base(&x.path))
            }
            // A shell step by what the agent said it does, else its command.
            Kind::Shell if !x.description.trim().is_empty() => ("shell", one_line(&x.description)),
            Kind::Shell => ("shell", one_line(first_line(no_cd(&x.command)))),
            Kind::Fetch | Kind::WebSearch => {
                let w = if x.url.is_empty() { &x.query } else { &x.url };
                ("web", one_line(w))
            }
            Kind::Subagent => {
                let w = if x.description.is_empty() {
                    &x.prompt
                } else {
                    &x.description
                };
                ("agent", one_line(first_line(w)))
            }
            Kind::Question | Kind::PlanMode => ("ask", self.label(id).plain()),
            _ => ("tool", st.tool.to_string()),
        }
    }

    /// A trail row's words: a shell step's command as it is; the rest a
    /// verb and what it was on, merged steps counted or named.
    fn leg_words(&self, g: &Leg) -> Txt {
        if g.verb == "shell" {
            return text(&g.names[0]);
        }
        let n = g.ids.len();
        let what = if n == 1 {
            g.names[0].clone()
        } else {
            match g.verb {
                "read" | "edit" => {
                    let joined = g.names.join(", ");
                    if width(&joined) <= 32 {
                        joined
                    } else {
                        plural(g.names.len(), "file")
                    }
                }
                "search" => plural(n, "search").replace("searchs", "searches"),
                "web" => plural(n, "lookup"),
                _ => plural(n, "call"),
            }
        };
        sub(&format!("{:<9}", g.verb)).and(text(&what))
    }

    /// The turn's time in its tools (as they ran, overlaps once) and
    /// waiting on you (questions asked, permissions waiting).
    fn tool_time(&self, legs: &[Leg]) -> (Ms, Ms) {
        let now = self.o.now;
        let mut spans: Vec<(Ms, Ms)> = Vec::new();
        let mut waiting = 0;
        for g in legs {
            for &id in &g.ids {
                let st = self.s.step(id);
                let end = match st.status {
                    Status::Running | Status::Waiting => now,
                    _ if st.end != 0 => st.end,
                    _ => st.start,
                };
                if st.start == 0 || end <= st.start {
                    continue;
                }
                if st.status == Status::Waiting || st.kind == Kind::Question {
                    waiting += end - st.start;
                } else {
                    spans.push((st.start, end));
                }
            }
        }
        spans.sort_unstable();
        let mut tools = 0;
        let mut cur: Option<(Ms, Ms)> = None;
        for (a, b) in spans {
            match cur {
                Some((ca, cb)) if a <= cb => cur = Some((ca, cb.max(b))),
                Some((ca, cb)) => {
                    tools += cb - ca;
                    cur = Some((a, b));
                }
                None => cur = Some((a, b)),
            }
        }
        if let Some((a, b)) = cur {
            tools += b - a;
        }
        (tools, waiting)
    }
}
