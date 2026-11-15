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
//! - Orchestrator, "who's doing what?": a lead's session as the agents it
//!   hands work to. What you said and what it answered, a few rows each;
//!   the turns its agents' news woke draw nothing. Under the last turn, a
//!   board of every agent it started (its subagents, and those started
//!   through rush's spawn_agent), at work first, each with the last word
//!   on it, the lead's own where it said one.
//!
//! Every view is drawn from the turn as the session already holds it, from
//! the generic model only (Kind, Status, exit codes, the shell's command),
//! whichever agent ran the session. New in the Rust port: Go rush had no
//! such views.

use super::super::Preset;
use super::super::decisions::{decisions_of, open_list};
use super::*;
use crate::decisions::decisions_in;
use crate::model::runs::{AGENT_SEND, agent_id, input_str, rush_tool};
use crate::render::spawn::{asked, tool_run};
use crate::shellbox::{Piece, ShellBlock, ShellRun, pieces};

/// Rows of lines a file's diff shows in Code until opened.
const CODE_CAP: usize = 40;
/// The width of an Orchestrator label ("You ", the lead's name).
const LABEL_W: usize = 14;
/// The rows what you said takes in Orchestrator, and what the lead said
/// back, before they're cut.
const EXCHANGE_YOU: usize = 2;
const EXCHANGE_SAID: usize = 3;
/// The agents done that the board shows, the newest.
const BOARD_DONE: usize = 8;

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
            | "swiftformat" | "clang-format" | "oxfmt" => Some("fmt"),
            "tsc" | "vue-tsc" | "mypy" | "pyright" => Some("typecheck"),
            "xcodebuild" => match crate::render::tools::xcode_action(rest) {
                "test" | "test-without-building" => Some("tests"),
                "build" | "build-for-testing" | "archive" | "analyze" => Some("build"),
                _ => None,
            },
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

/// How a command you ran stands, as a check's mark: ✱ running, ✓ exit 0,
/// ✕ stopped or ended while rush was closed, ✗ failed.
fn run_mark(run: &ShellRun) -> Txt {
    if run.running() {
        paint(C_ORANGE, "✱")
    } else if run.stopped || run.lost {
        paint(C_YELLOW, "✕")
    } else if run.exit == Some(0) {
        paint(C_GREEN, "✓")
    } else {
        paint(C_RED, "✗")
    }
}

/// What came of a command you ran, in a few words: running, exit 0,
/// stopped, ended while rush was closed, exit 2.
fn run_came(run: &ShellRun) -> Txt {
    let w = run.outcome();
    if run.running() || run.stopped || run.lost || run.exit == Some(0) {
        dim(&w)
    } else {
        paint(C_RED, &w)
    }
}

/// A command you ran on one line, room wide at most: a faint $ before
/// it, as in its box, and how many more lines it has.
fn run_words(run: &ShellRun, room: usize) -> Txt {
    let n = run.command.trim().lines().count();
    let mut t = faint("$ ").and(text(&one_line(first_line(run.command.trim()))));
    if n > 1 {
        t.push(faint(&format!(" +{} more", n - 1)));
    }
    if t.width() > room {
        t = t.truncate(room, &faint("…"));
    }
    t
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

/// What a chat shows of a turn after your message, in order.
enum ChatSeg {
    /// The agent's words, a bubble.
    Said(Vec<(String, bool)>),
    /// A message you sent while it worked, and its images.
    You(String, Vec<String>),
    /// A drawing shown with rush's show tool.
    Shown(StepId),
    /// The k-th shell block of item i, as its box.
    Block(usize, usize, ShellBlock),
    /// A command you ran after `!`, by its box's ref.
    Bang(String),
}

/// How an agent on the Orchestrator board is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneState {
    Working,
    /// It asks something of the lead.
    Waiting,
    Done,
    Failed,
    Stopped,
}

impl LaneState {
    /// As the step that started it stands.
    fn of(st: Status) -> LaneState {
        match st {
            Status::Running => LaneState::Working,
            Status::Waiting => LaneState::Waiting,
            Status::Failed => LaneState::Failed,
            // Its turn ended without a word of how it went.
            Status::Lost => LaneState::Stopped,
            _ => LaneState::Done,
        }
    }
}

/// An agent the session handed work to, on the Orchestrator board.
#[derive(Clone, Debug)]
pub(crate) struct Lane {
    /// What it was given: a subagent's description, what a started agent
    /// was asked.
    pub name: String,
    /// What runs it: the subagent's type, the agent and its model.
    pub what: String,
    pub state: LaneState,
    pub start: Ms,
    pub end: Ms,
    /// The last word on it: what the lead said when its news came, else
    /// its own; and when.
    pub news: String,
    pub news_at: Ms,
    /// The step that started it, which the row opens.
    pub r: String,
    /// The turn it started in.
    pub turn: usize,
    /// A subagent of the session's own, not an agent started through rush.
    pub sub: bool,
}

/// The agent a message reports for: "Codex (agent 21bcc40e) finished:"
/// is 21bcc40e's report. None for any other message.
fn reported_by(prompt: &str) -> Option<String> {
    let first = prompt.trim_start().lines().next()?;
    let (_, rest) = first.split_once("(agent ")?;
    let (id, tail) = rest.split_once(')')?;
    (tail.trim_start().starts_with("finished") && !id.is_empty()).then(|| id.to_string())
}

/// A report's words after its "… finished:" line.
fn after_report(prompt: &str) -> &str {
    let p = prompt.trim_start();
    match p.split_once('\n') {
        Some((first, rest)) if reported_by(first).is_some() => rest.trim(),
        _ => p,
    }
}

/// The agent a spawn_agent call started, by id, from what it answered:
/// "… in the background as agent 21bcc40e." None when it started none.
fn started_id(out: &str) -> Option<String> {
    let (_, rest) = out.split_once("as agent ")?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    (!id.is_empty()).then_some(id)
}

/// A card's label, LABEL_W wide.
fn card_label(s: &str, c: Style) -> Txt {
    paint(c, &format!("{s:<w$}", w = LABEL_W))
}

/// A card's rows for what needs you, each with what it opens.
fn needs_rows(needs: &[(Txt, String)], room: usize) -> Vec<(Txt, String)> {
    needs
        .iter()
        .map(|(w, r)| {
            let action = paint(C_ORANGE, "[answer]");
            let room = room.saturating_sub(action.width() + 3);
            let l = card_label("? needs you", C_YELLOW)
                .and(w.truncate(room, &faint("…")))
                .and_raw("   ")
                .and(action);
            (l, r.clone())
        })
        .collect()
}

impl<'a> Drawer<'a> {
    /// The turn drawn as p has it.
    pub(crate) fn preset(&mut self, p: Preset) {
        self.no_hooks = true;
        self.rule = false;
        match p {
            Preset::Chat => self.chat(),
            Preset::Code => self.code_view(),
            Preset::Orchestrator => self.orchestrator(),
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
        // you sent while it worked, the drawings it showed and the shell
        // blocks of its replies, and the commands you ran after `!` where
        // you ran them.
        let mut segs: Vec<ChatSeg> = Vec::new();
        let mut said: Vec<(String, bool)> = Vec::new();
        let flush = |segs: &mut Vec<ChatSeg>, said: &mut Vec<(String, bool)>| {
            if !said.is_empty() {
                segs.push(ChatSeg::Said(std::mem::take(said)));
            }
        };
        for (i, it) in t.items.iter().enumerate() {
            let due = self.take_bangs(i);
            if !due.is_empty() {
                flush(&mut segs, &mut said);
                segs.extend(due.into_iter().map(|(k, _)| ChatSeg::Bang(k)));
            }
            match it {
                Item::Text { text, answer } if !text.trim().is_empty() => {
                    for p in pieces(text) {
                        match p {
                            Piece::Words(w) => said.push((w, *answer)),
                            // A shell block is a box in its place.
                            Piece::Block(k, b) => {
                                flush(&mut segs, &mut said);
                                segs.push(ChatSeg::Block(i, k, b));
                            }
                        }
                    }
                }
                Item::Interject(x) => {
                    flush(&mut segs, &mut said);
                    segs.push(ChatSeg::You(
                        x.text.trim().to_string(),
                        image_names(&x.images),
                    ));
                }
                // A drawing shown with rush's show tool is part of what it
                // said: drawn in its place between the bubbles.
                Item::Step(id) if shown(self.s.step(*id)) => {
                    flush(&mut segs, &mut said);
                    segs.push(ChatSeg::Shown(*id));
                }
                _ => {}
            }
        }
        flush(&mut segs, &mut said);
        let in_turn = segs.len();
        let after = self.take_bangs(usize::MAX);
        segs.extend(after.into_iter().map(|(k, _)| ChatSeg::Bang(k)));
        // The last of its words carry the turn's foot, unless you said
        // something after them; else an empty bubble does, when there's
        // something to say on it.
        let last = segs[..in_turn]
            .iter()
            .rposition(|s| matches!(s, ChatSeg::Said(_)))
            .filter(|&j| {
                !segs[j + 1..in_turn]
                    .iter()
                    .any(|s| matches!(s, ChatSeg::You(..)))
            });
        let foot_alone =
            last.is_none() && (t.live || self.steps_counted() > 0 || self.needs_here());
        let room = self.bubble_room();
        let total = segs.len();
        for (j, seg) in segs.into_iter().enumerate() {
            if j == in_turn && foot_alone {
                self.agent_bubble(&[], true);
            }
            match seg {
                ChatSeg::Said(said) => self.agent_bubble(&said, last == Some(j)),
                ChatSeg::You(text, imgs) => self.you_bubble(&text, &imgs, "while it worked"),
                ChatSeg::Shown(id) => self.step(id, 0),
                ChatSeg::Block(i, k, b) => {
                    self.box_place = Some((room, false));
                    self.block_box(i, k, &b);
                    self.box_place = None;
                }
                ChatSeg::Bang(k) => {
                    let o = self.o;
                    if let Some(run) = o.shells.get(&k) {
                        self.box_place = Some((room, true));
                        self.run_box(&k, run);
                        self.box_place = None;
                    }
                }
            }
        }
        if foot_alone && in_turn == total {
            self.agent_bubble(&[], true);
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
    fn agent_bubble(&mut self, said: &[(String, bool)], last: bool) {
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
            let n = self.steps_counted();
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
        let open = open_list(self.s, self.ti, &self.o.open);
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
        // The commands you ran in it (▶ run or `!`), a check's line each;
        // enter opens one's box under it.
        for (r, run) in self.turn_runs() {
            self.run_row(&r, run);
        }
        // Its replies' shell blocks not run, as their boxes.
        for (i, it) in t.items.iter().enumerate() {
            let Item::Text { text, .. } = it else {
                continue;
            };
            for (k, b) in crate::shellbox::shell_blocks(text).into_iter().enumerate() {
                let r = format!("{}:sh:{i}:{k}", self.r);
                let o = self.o;
                if o.shells
                    .get(&r)
                    .is_some_and(|x| x.command == b.script() || x.edited)
                {
                    continue;
                }
                self.block_box(i, k, &b);
            }
        }
        // A drawing shown with rush's show tool is part of the reply, never
        // left out: under the turn's changes.
        for it in &t.items {
            if let Item::Step(id) = it
                && shown(self.s.step(*id))
            {
                self.step(*id, 0);
            }
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

    /// A file's diff in its box, the box's top edge its row: CODE_CAP rows
    /// of lines until opened (its row's ref, or "▸ show all"), whole once
    /// it is.
    fn code_file(&mut self, k: usize, f: &FileEdits) {
        let r = format!("{}:file:{k}", self.r);
        let open = self.o.open.get(&r).copied().unwrap_or(false);
        let all = format!("{r}:all");
        let from = self.lines.len();
        self.diff_box(&super::diffbox::BoxSpec {
            ids: &f.ids,
            indent: 2,
            head_ref: &r,
            fold_ref: &r,
            all_ref: &all,
            bare: true,
            note: f.note,
            counts: Some((f.add, f.del)),
            cap: CODE_CAP,
            whole: open,
        });
        self.own(&r, from);
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

    /// A command you ran, as a check's line: how it came out, its
    /// command, its code, and how long it took. Opened (enter), its box
    /// under it.
    fn run_row(&mut self, r: &str, run: &ShellRun) {
        let lead = self.sp(1).and(run_mark(run)).and_raw(" ");
        let mut detail = run_came(run);
        if run.running() {
            let label = run.label();
            if !label.is_empty() {
                detail.push(faint(" · "));
                detail.push(paint(C_SUB, &label));
            }
        }
        let right = if run.running() {
            let s = self.since(run.start);
            dim(&s).and_raw("  ")
        } else if run.end - run.start >= 100 {
            dim(&dur(run.end - run.start)).and_raw("  ")
        } else {
            Txt::new()
        };
        let room = self.cw as isize
            - lead.width() as isize
            - detail.width() as isize
            - right.width() as isize
            - 6;
        let words = run_words(run, room.max(12) as usize);
        let l = lead.and(words).and(blanks(3)).and(detail);
        let from = self.lines.len();
        self.add(r, Color::Default, l, right);
        self.box_under(r, run);
        self.own(r, from);
    }

    /// Run r's box under its line in another view, while it's opened.
    fn box_under(&mut self, r: &str, run: &ShellRun) {
        if !self.box_open(r) && self.o.shell_edit.as_ref().is_none_or(|e| e.r != r) {
            return;
        }
        self.box_under_row = true;
        self.run_box(r, run);
        self.box_under_row = false;
    }

    // ---- Orchestrator ----

    /// A lead's session as the work it hands out: what you said and what
    /// it answered, a few rows each; the turns its agents woke draw
    /// nothing, as the board under the last turn says what came of them.
    fn orchestrator(&mut self) {
        if Self::yours(self.t) && reported_by(&self.t.prompt).is_none() {
            self.exchange_rows();
        }
        if self.ti + 1 == self.s.turns.len() {
            self.board();
        }
    }

    /// What you said, then what the lead said back, each a few lines on
    /// a label: the turn's own rows, which open it.
    fn exchange_rows(&mut self) {
        let t = self.t;
        let r = self.r.clone();
        if self.ti > 0 {
            self.bare();
        }
        let from = self.lines.len();
        let inner = self.cw.saturating_sub(GUTTER + 2).max(20);
        let room = inner.saturating_sub(LABEL_W);
        let when = if t.start != 0 {
            dim(&clock(t.start, self.o.offset(t.start)))
        } else {
            Txt::new()
        };
        let lead = if self.o.speaker.is_empty() {
            "Agent".to_string()
        } else {
            self.o.speaker.clone()
        };
        let asked = flat(&fold_pastes(&t.prompt));
        let said = t.answer().map(|a| flat(decisions_in(a).0));
        let said = match said {
            Some(w) if !w.is_empty() => sub(&w),
            _ if t.live => paint(
                C_ORANGE,
                &format!("working · {}", self.since(t.start.max(1))),
            ),
            _ => faint(first_line(unasked(t))),
        };
        let parts = [
            (card_label("You", C_SUB), text(&asked), EXCHANGE_YOU, when),
            (card_label(&lead, C_SUB), said, EXCHANGE_SAID, Txt::new()),
        ];
        for (label, words, most, right) in parts {
            let room = room.saturating_sub(right.width() + 2).max(8);
            let ws = wrap(&words, room);
            let cut = ws.len() > most;
            for (i, w) in ws.into_iter().take(most).enumerate() {
                let mut w = w;
                if cut && i + 1 == most && !w.plain().ends_with('…') {
                    w = w.truncate(room.saturating_sub(1), &faint("…"));
                    w.push(faint("…"));
                }
                let l = if i == 0 {
                    label.clone()
                } else {
                    blanks(LABEL_W)
                };
                let right = if i == 0 { right.clone() } else { Txt::new() };
                let row = self.sp(1).and(l).and(w);
                self.add(&r, Color::Default, row, right);
            }
        }
        self.own(&r, from);
    }

    /// Every agent the session handed work to, as it stands: its
    /// subagents and the agents it started through rush (spawn_agent),
    /// each with how it's doing and the last word on it, the lead's own
    /// where it said one. Read from every turn, so it costs what the
    /// session's steps do; drawn with the last turn only.
    pub(crate) fn lanes(&mut self) -> Vec<Lane> {
        let s: &'a Session = self.s;
        let o: &'a Options = self.o;
        // The last turn each subagent's call woke, and each agent's
        // report, by its id: what the lead made of it.
        // And the last turn each was sent more work in (agent_send): at
        // work again till its next report.
        let mut woke: HashMap<&str, usize> = HashMap::new();
        let mut came: HashMap<String, usize> = HashMap::new();
        let mut sent: HashMap<String, usize> = HashMap::new();
        for (i, t) in s.turns.iter().enumerate() {
            if !t.woke_call.is_empty() {
                woke.insert(t.woke_call.as_str(), i);
            }
            if let Some(id) = reported_by(&t.prompt) {
                came.insert(id, i);
            }
            for &id in &t.steps {
                let st = s.step(id);
                if rush_tool(&st.tool) == Some(AGENT_SEND)
                    && st.status != Status::Failed
                    && let Some(a) = agent_id(st)
                {
                    sent.insert(a, i);
                }
            }
        }
        let news_of = |i: usize| -> (String, Ms) {
            let t = &s.turns[i];
            let w = t
                .answer()
                .map(|a| first_sentence(decisions_in(a).0))
                .unwrap_or_default();
            (w, t.end.max(t.start))
        };
        let mut out: Vec<Lane> = Vec::new();
        for (ti, t) in s.turns.iter().enumerate() {
            for &id in &t.steps {
                let st = s.step(id);
                if st.parent.is_some() {
                    continue; // a subagent's own work: its lane's
                }
                if st.kind == Kind::Subagent {
                    let job = s.jobs().iter().find(|j| j.tool_use_id == *st.id);
                    let mut name = input_str(st, "description");
                    if name.is_empty() {
                        name = job.map(|j| one_line(&j.label)).unwrap_or_default();
                    }
                    if name.is_empty() {
                        name = self.label(id).plain();
                    }
                    let what = job
                        .map(|j| j.agent.clone())
                        .filter(|a| !a.is_empty())
                        .unwrap_or_else(|| "subagent".into());
                    let state = match job {
                        Some(j) if j.running() => LaneState::Working,
                        Some(j) if j.status == "failed" => LaneState::Failed,
                        Some(j) if j.status == "stopped" => LaneState::Stopped,
                        Some(_) => LaneState::Done,
                        None => LaneState::of(st.status),
                    };
                    let end = job.map_or(st.end, |j| j.end);
                    let (mut news, mut at) = woke
                        .get(&*st.id)
                        .map_or_else(|| (String::new(), 0), |&i| news_of(i));
                    if news.is_empty()
                        && let Some(j) = job
                    {
                        news = one_line(&j.summary);
                        at = j.progress_at.max(j.end);
                    }
                    if news.is_empty() && !st.output.is_empty() && state != LaneState::Working {
                        news = first_sentence(&st.output);
                    }
                    out.push(Lane {
                        name,
                        what,
                        state,
                        start: job.map_or(st.start, |j| j.start).max(st.start),
                        end,
                        news,
                        news_at: at,
                        r: self.step_ref(st),
                        turn: ti,
                        sub: true,
                    });
                } else if let Some(sp) = tool_run(st) {
                    let kid = o.kids.get(&*st.id).and_then(|ks| ks.first());
                    let agent = kid.map(|k| k.id.clone()).or_else(|| started_id(&st.output));
                    let mut state = LaneState::of(st.status);
                    if state == LaneState::Done && agent.is_some() {
                        // Started in the background: at work till its
                        // report comes.
                        state = LaneState::Working;
                    }
                    let (mut news, mut at, mut end) = (String::new(), 0, st.end);
                    if let Some(i) = agent.as_ref().and_then(|a| came.get(a)) {
                        state = LaneState::Done;
                        (news, at) = news_of(*i);
                        end = s.turns[*i].start;
                        if news.is_empty() {
                            news = first_sentence(after_report(&s.turns[*i].prompt));
                        }
                        // Sent more since: at work on it.
                        if agent
                            .as_ref()
                            .and_then(|a| sent.get(a))
                            .is_some_and(|j| j >= i)
                        {
                            state = LaneState::Working;
                        }
                    }
                    if let Some(k) = kid {
                        state = if k.live {
                            LaneState::Working
                        } else if state == LaneState::Working {
                            LaneState::Done
                        } else {
                            state
                        };
                    }
                    let mut what = sp.name.clone();
                    if !sp.model.is_empty() && sp.model != sp.name {
                        what = format!("{what} · {}", sp.model);
                    }
                    out.push(Lane {
                        name: asked(&sp),
                        what,
                        state,
                        start: st.start,
                        end,
                        news,
                        news_at: at,
                        r: self.step_ref(st),
                        turn: ti,
                        sub: false,
                    });
                }
            }
        }
        // A subagent started again on the same work (its description)
        // is the same lane: the newest stands for it.
        let mut seen: HashSet<String> = HashSet::new();
        let mut kept: Vec<Lane> = Vec::new();
        for l in out.into_iter().rev() {
            if l.sub && !seen.insert(l.name.clone()) {
                continue;
            }
            kept.push(l);
        }
        kept.reverse();
        kept
    }

    /// The board: a heading counting the agents by how they're doing, what
    /// needs you, then the agents at work, oldest first, and those done,
    /// newest first, BOARD_DONE of them and how many more.
    fn board(&mut self) {
        let s = self.s;
        let lanes = self.lanes();
        let inner = self.cw.saturating_sub(GUTTER + 2).max(20);
        let room = inner.saturating_sub(LABEL_W);
        let first_r = format!("t{}", s.turns[0].n);
        let count = |f: &dyn Fn(LaneState) -> bool| lanes.iter().filter(|l| f(l.state)).count();
        let working = count(&|x| x == LaneState::Working);
        let waiting = count(&|x| x == LaneState::Waiting);
        let failed = count(&|x| matches!(x, LaneState::Failed | LaneState::Stopped));
        let done = count(&|x| x == LaneState::Done);
        let mut head = text("Agents");
        let mut said = Vec::new();
        for (n, w) in [
            (working, "working"),
            (waiting, "waiting"),
            (failed, "stopped or failed"),
            (done, "done"),
        ] {
            if n > 0 {
                said.push(format!("{n} {w}"));
            }
        }
        if said.is_empty() {
            said.push("none started yet".into());
        }
        head.push(dim(&format!(" · {}", said.join(" · "))));
        let cost: f64 = s.turns.iter().map(|t| t.cost).sum();
        let m = money(cost);
        if !m.is_empty() {
            head.push(dim(&format!(" · {m}")));
        }
        // What needs you: the last turn's questions and permissions, and
        // the choices its answer left you.
        let last = &s.turns[self.ti];
        let mut needs: Vec<(Txt, String)> = Vec::new();
        let ds = open_list(s, self.ti, &self.o.open);
        if let Some(d) = ds.first() {
            let mut w = one_line(&d.topic);
            if ds.len() > 1 {
                w = format!("{w} and {} more", ds.len() - 1);
            }
            needs.push((text(&w), format!("t{}", last.n)));
        }
        for &id in &last.steps {
            let st = s.step(id);
            if st.status == Status::Waiting {
                needs.push((text(&self.label(id).plain()), self.step_ref(st)));
            }
        }
        if needs.is_empty() && self.needs_here() {
            let reason = if self.o.needs.reason.is_empty() {
                "it asks something of you".to_string()
            } else {
                one_line(&self.o.needs.reason)
            };
            needs.push((text(&reason), self.r.clone()));
        }
        let mut rows: Vec<(Txt, String)> = needs_rows(&needs, room);
        if !needs.is_empty() {
            rows.push((Txt::new(), String::new()));
        }
        let mut at_work: Vec<&Lane> = lanes
            .iter()
            .filter(|l| matches!(l.state, LaneState::Working | LaneState::Waiting))
            .collect();
        at_work.sort_by_key(|l| (l.state != LaneState::Waiting, l.start));
        let mut ended: Vec<&Lane> = lanes
            .iter()
            .filter(|l| !matches!(l.state, LaneState::Working | LaneState::Waiting))
            .collect();
        ended.sort_by_key(|l| std::cmp::Reverse((l.end.max(l.news_at), l.turn)));
        let more = ended.len().saturating_sub(BOARD_DONE);
        for l in at_work.iter().chain(ended.iter().take(BOARD_DONE)) {
            rows.extend(self.lane_rows(l, inner));
        }
        if more > 0 {
            let l = blanks(2).and(dim(&format!("and {more} more before them")));
            rows.push((l, String::new()));
        }
        if lanes.is_empty() {
            rows.push((
                faint("no agents yet: its subagents and the agents it starts show here"),
                String::new(),
            ));
        }
        let rail_c = if !needs.is_empty() {
            C_YELLOW
        } else if working > 0 || last.live {
            C_ORANGE
        } else {
            C_FAINT
        };
        let rail = || paint(rail_c, "┃").and_raw(" ");
        self.bare();
        let from_row = self.lines.len();
        let l = self.sp(1).and(head.truncate(inner + 2, &faint("…")));
        self.add(&first_r, Color::Default, l, Txt::new());
        for (row, r) in rows {
            let l = self.sp(1).and(rail()).and(row);
            self.add(&r, Color::Default, l, Txt::new());
        }
        self.own(&first_r, from_row);
    }

    /// One agent on the board: its mark, what it was given, what runs it
    /// and for how long; under it, at work or done, the last word on it.
    fn lane_rows(&mut self, l: &Lane, inner: usize) -> Vec<(Txt, String)> {
        let (mark, c) = match l.state {
            LaneState::Working => ("●", C_ORANGE),
            LaneState::Waiting => ("?", C_YELLOW),
            LaneState::Done => ("✓", C_GREEN),
            LaneState::Failed => ("✗", C_RED),
            LaneState::Stopped => ("✕", C_FAINT),
        };
        let took = match l.state {
            LaneState::Working | LaneState::Waiting if l.start != 0 => self.since(l.start),
            _ if l.start != 0 && l.end > l.start => dur(l.end - l.start),
            _ => String::new(),
        };
        let mut right = dim(&l.what);
        if !took.is_empty() {
            right.push(dim(&format!(" · {took}")));
        }
        let room = inner.saturating_sub(right.width() + 5).max(8);
        let name = if matches!(l.state, LaneState::Working | LaneState::Waiting) {
            text(&l.name)
        } else {
            sub(&l.name)
        };
        let head = paint(c, mark)
            .and_raw(" ")
            .and(name.truncate(room, &faint("…")))
            .and_raw("  ")
            .and(right);
        let mut out = vec![(head, l.r.clone())];
        if !l.news.is_empty() {
            let w = faint(&l.news).truncate(inner.saturating_sub(2), &faint("…"));
            out.push((blanks(2).and(w), l.r.clone()));
        }
        out
    }
}
