//! Shell commands rush runs itself, on your click or key: a block of
//! shell in an agent's reply (```sh, ```bash, ```zsh, ```shell,
//! ```console, or one whose lines start with `$ `), and a command typed
//! after `!` in the message box. What's here is the plain part: which
//! blocks are shell and what of them runs, and a run as rush keeps it
//! (its output, how it ended, which command of its chain runs now, read
//! with the chain machinery an agent's Bash calls use, chainrun.rs). The
//! terminal box it's drawn as is render/draw/shellbox.rs; starting,
//! stopping and keeping runs is the UI's (rush-ui-session-core's
//! shellbox.rs). Rust-only: Go's rush had neither.

use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

use crate::model::{Chain, RunningPart, Shell};

/// The fences whose blocks are shell.
pub const SHELL_TAGS: [&str; 5] = ["sh", "bash", "zsh", "shell", "console"];

/// How much of a run's output is kept: its last 256 KiB.
pub const OUT_KEEP: usize = 256 * 1024;

/// How many of a run's last lines its box shows until it's opened.
pub const OUT_SHOWN: usize = 12;

/// What a run sent to the agent keeps of its output, at most: its last
/// lines, up to this many characters.
pub const SEND_LINES: usize = 80;
pub const SEND_CHARS: usize = 6000;

/// A fenced block of shell in a reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellBlock {
    /// A console session: only its `$ ` lines are commands, the rest is
    /// what they printed, as an example, and doesn't run.
    pub console: bool,
    /// Its lines as written, the fence's own left out.
    pub lines: Vec<String>,
}

impl ShellBlock {
    /// The block a fence tagged tag holding body is, when it's shell:
    /// tagged so, or untagged (or tagged as something rush doesn't run)
    /// with its first line a `$ ` prompt.
    pub fn from_fence(tag: &str, body: &[&str]) -> Option<ShellBlock> {
        let lang = tag.split_whitespace().next().unwrap_or("").to_lowercase();
        let first = body.iter().find(|l| !l.trim().is_empty())?;
        let prompt = first.trim_start().starts_with("$ ") || first.trim() == "$";
        let console = if SHELL_TAGS.contains(&lang.as_str()) {
            lang == "console"
        } else if prompt && !is_code_tag(&lang) {
            true
        } else {
            return None;
        };
        let lines: Vec<String> = body.iter().map(|l| l.trim_end().to_string()).collect();
        let b = ShellBlock { console, lines };
        (!b.script().trim().is_empty()).then_some(b)
    }

    /// Its rows as the box shows them: each line, whether it's a command,
    /// and its text with the `$ ` the agent wrote taken off. Blank lines
    /// at either end are left out.
    pub fn rows(&self) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for l in &self.lines {
            match strip_prompt(l) {
                Some(c) => out.push((true, c.to_string())),
                None if self.console => out.push((false, l.clone())),
                None => out.push((true, l.clone())),
            }
        }
        while out.first().is_some_and(|(_, t)| t.trim().is_empty()) {
            out.remove(0);
        }
        while out.last().is_some_and(|(_, t)| t.trim().is_empty()) {
            out.pop();
        }
        out
    }

    /// What runs: its commands, a line each, as one script.
    pub fn script(&self) -> String {
        self.rows()
            .into_iter()
            .filter(|(cmd, _)| *cmd)
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }
}

/// Languages a fence names that are code, not a terminal: a `$` there is
/// part of the code.
fn is_code_tag(lang: &str) -> bool {
    !matches!(
        lang,
        "" | "text" | "txt" | "plain" | "plaintext" | "terminal"
    )
}

/// A line's command when it starts with a `$ ` prompt.
fn strip_prompt(l: &str) -> Option<&str> {
    let t = l.trim_start();
    if t == "$" {
        return Some("");
    }
    t.strip_prefix("$ ")
}

/// The shell blocks of a reply's text, in order, as the conversation
/// draws them: the k-th is the one whose box is `<turn>:sh:<item>:<k>`.
/// Fences are found as rush's markdown finds them (render/markdown.rs).
pub fn shell_blocks(text: &str) -> Vec<ShellBlock> {
    scan(text).into_iter().map(|(_, _, b)| b).collect()
}

/// A part of a reply's text cut at its shell blocks: the words before,
/// between or after them, or a block with its number among the reply's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Words(String),
    Block(usize, ShellBlock),
}

/// text as its words and shell blocks, in order; words that are only
/// blank lines are left out.
pub fn pieces(text: &str) -> Vec<Piece> {
    let lines: Vec<&str> = text.split('\n').collect();
    let words = |out: &mut Vec<Piece>, a: usize, b: usize| {
        let w = lines[a.min(lines.len())..b.min(lines.len())].join("\n");
        if !w.trim().is_empty() {
            out.push(Piece::Words(w));
        }
    };
    let mut out = Vec::new();
    let mut from = 0;
    for (k, (start, end, b)) in scan(text).into_iter().enumerate() {
        words(&mut out, from, start);
        out.push(Piece::Block(k, b));
        from = end + 1;
    }
    words(&mut out, from, lines.len());
    out
}

/// The shell blocks of text with the lines they take: their opening
/// fence's and their closing one's (the last line when it's unclosed).
fn scan(text: &str) -> Vec<(usize, usize, ShellBlock)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trim = lines[i].trim();
        if let Some(tag) = trim.strip_prefix("```") {
            let mut end = i + 1;
            while end < lines.len() && !lines[end].trim().starts_with("```") {
                end += 1;
            }
            if let Some(b) = ShellBlock::from_fence(tag.trim(), &lines[i + 1..end.min(lines.len())])
            {
                out.push((i, end.min(lines.len() - 1), b));
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Whether any of runs is a `!` run made before the session had a turn
/// (`t0:bang:<start>`): drawn above the turns, or alone.
pub fn unturned<V>(runs: &std::collections::HashMap<String, V>) -> bool {
    runs.keys().any(|k| k.starts_with("t0:"))
}

/// The ref of the box of the k-th shell block of item i of turn n.
pub fn block_ref(turn: u32, item: usize, k: usize) -> String {
    format!("t{turn}:sh:{item}:{k}")
}

/// The ref of the box of a `!` run in turn n, started at start.
pub fn bang_ref(turn: u32, start: Ms) -> String {
    format!("t{turn}:bang:{start}")
}

/// What a box's ref names: a block (turn, item, k) or a `!` run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxRef {
    Block { turn: u32, item: usize, k: usize },
    Bang { turn: u32, start: Ms },
}

impl BoxRef {
    /// Reads a box's ref; None for any other row's.
    pub fn parse(r: &str) -> Option<BoxRef> {
        let rest = r.strip_prefix('t')?;
        let (n, rest) = rest.split_once(':')?;
        let turn = n.parse().ok()?;
        if let Some(b) = rest.strip_prefix("sh:") {
            let (i, k) = b.split_once(':')?;
            return Some(BoxRef::Block {
                turn,
                item: i.parse().ok()?,
                k: k.parse().ok()?,
            });
        }
        let start = rest.strip_prefix("bang:")?.parse().ok()?;
        Some(BoxRef::Bang { turn, start })
    }

    /// The turn it's in, by number: 0 for a `!` run before any.
    pub fn turn(&self) -> u32 {
        match *self {
            BoxRef::Block { turn, .. } | BoxRef::Bang { turn, .. } => turn,
        }
    }
}

/// A command rush ran itself, and what came of it so far.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellRun {
    /// The script, as it was given to the shell.
    pub command: String,
    /// The folder it ran in.
    pub cwd: String,
    /// Typed after `!` in the message box, not a block of a reply.
    pub bang: bool,
    /// A `!` run's place: the turn it was run in, by number, and how many
    /// of that turn's items there were then; its box is drawn after them.
    pub turn: u32,
    pub after: usize,
    pub start: Ms,
    /// 0 while it runs.
    pub end: Ms,
    /// How it exited; None when a signal ended it.
    pub exit: Option<i32>,
    /// You stopped it.
    pub stopped: bool,
    /// What it wrote, stdout and stderr as they came, colours kept: the
    /// last OUT_KEEP bytes of it.
    pub out: String,
    /// Lines let go from the front to keep it under OUT_KEEP.
    pub dropped: usize,
    /// Its output was put in the message box for the agent.
    pub sent: bool,
    /// The process running it, and when that started, to know it again
    /// when rush is opened while it still runs (0 for none).
    pub pid: i32,
    pub pid_start: Ms,
    /// The file its output is written to as it runs, read back from
    /// there; "" when it isn't written to one.
    pub log: String,
    /// It ended while rush was closed, how isn't known.
    pub lost: bool,
    /// Its command running now is paused (p).
    #[serde(skip)]
    pub paused: bool,
    /// Which command of its chain runs now, as its processes were seen.
    #[serde(skip)]
    pub chain: Chain,
    /// Bumped with every change, for what draws it.
    #[serde(skip)]
    pub ver: u64,
}

impl ShellRun {
    /// A run of command in cwd starting now.
    pub fn new(command: &str, cwd: &str, now: Ms) -> ShellRun {
        ShellRun {
            command: command.to_string(),
            cwd: cwd.to_string(),
            start: now,
            ver: 1,
            ..ShellRun::default()
        }
    }

    pub fn running(&self) -> bool {
        self.start != 0 && self.end == 0
    }

    /// Adds what it wrote, letting go of its oldest lines past OUT_KEEP.
    pub fn push(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.out.push_str(s);
        if self.out.len() > OUT_KEEP {
            let mut cut = self.out.len() - OUT_KEEP;
            while !self.out.is_char_boundary(cut) {
                cut += 1;
            }
            if let Some(nl) = self.out[cut..].find('\n') {
                cut += nl + 1;
            }
            self.dropped += self.out[..cut].matches('\n').count();
            self.out.drain(..cut);
        }
        self.ver += 1;
    }

    /// It ended: with code exit, or by a signal (None).
    pub fn finish(&mut self, exit: Option<i32>, now: Ms) {
        if self.end != 0 {
            return;
        }
        self.end = now.max(self.start);
        self.exit = exit;
        self.paused = false;
        self.ver += 1;
    }

    /// Its output a line each, as a terminal leaves it: a line redrawn
    /// with carriage returns is its last state; colours kept.
    pub fn lines(&self) -> Vec<String> {
        let out = self.out.strip_suffix('\n').unwrap_or(&self.out);
        if out.is_empty() {
            return Vec::new();
        }
        out.split('\n')
            .map(|l| {
                let l = l.strip_suffix('\r').unwrap_or(l);
                match l.rfind('\r') {
                    Some(i) => l[i + 1..].to_string(),
                    None => l.to_string(),
                }
            })
            .collect()
    }

    /// How many lines it wrote, those let go included.
    pub fn line_count(&self) -> usize {
        self.dropped + self.lines().len()
    }

    /// Notes which command of its chain sh, its shell as read from the
    /// machine, has running now.
    pub fn watch(&mut self, sh: &Shell, now: Ms) {
        if !self.running() {
            return;
        }
        self.chain.watch(&self.command, sh, now);
        self.ver += 1;
    }

    /// The command of its chain running now, while it runs.
    pub fn running_part(&self) -> Option<RunningPart> {
        if !self.running() {
            return None;
        }
        self.chain.running(&self.command)
    }

    /// Its chain's place now, "2/3 cargo test"; "" when none is known.
    pub fn label(&self) -> String {
        if !self.running() {
            return String::new();
        }
        self.chain.label(&self.command)
    }

    /// How it stands, for its chain's marks.
    pub(crate) fn chain_ends(&self) -> crate::model::ChainEnds {
        let ok = !self.running() && !self.stopped && self.exit == Some(0);
        crate::model::ChainEnds {
            live: self.running(),
            start: self.start,
            end: self.end,
            ok,
            failed: !self.running() && !ok,
        }
    }

    /// How it ended, in a few words: "exit 0", "exit 2", "stopped",
    /// "killed"; "running" while it runs.
    pub fn outcome(&self) -> String {
        if self.running() {
            return "running".into();
        }
        if self.stopped {
            return "stopped".into();
        }
        if self.lost {
            return "ended while rush was closed".into();
        }
        match self.exit {
            Some(c) => format!("exit {c}"),
            None => "killed".into(),
        }
    }

    /// The command and what it printed as text for the message box, for
    /// you to send the agent: its last SEND_LINES lines, at most
    /// SEND_CHARS characters, colours taken out.
    pub fn send_text(&self) -> String {
        let cmd = self.command.trim();
        let mut s = if cmd.contains('\n') || cmd.contains('`') {
            format!("I ran this ({}):\n```sh\n{cmd}\n```\n", self.outcome())
        } else {
            format!("I ran `{cmd}` ({}):\n", self.outcome())
        };
        let lines: Vec<String> = self.lines().iter().map(|l| plain_text(l)).collect();
        let mut kept: Vec<&str> = Vec::new();
        let mut chars = 0;
        for l in lines.iter().rev() {
            if kept.len() == SEND_LINES || chars + l.len() + 1 > SEND_CHARS {
                break;
            }
            chars += l.len() + 1;
            kept.push(l);
        }
        kept.reverse();
        let left = self.dropped + lines.len() - kept.len();
        if lines.iter().all(|l| l.trim().is_empty()) {
            s.push_str("(no output)");
            return s;
        }
        s.push_str("```\n");
        if left > 0 {
            s.push_str(&format!("… {left} earlier lines left out\n"));
        }
        s.push_str(&kept.join("\n"));
        s.push_str("\n```");
        s
    }
}

/// s without its escape sequences.
pub fn plain_text(s: &str) -> String {
    sgr_spans(s).into_iter().map(|(t, _)| t).collect()
}

/// A colour or attribute an SGR escape set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sgr {
    pub fg: Option<rush_core::text::Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A line of output as runs of text and the SGR colours each was written
/// in: 16 and 256 colours and true colour, bold, dim, italic, underline
/// and resets. Every other escape, and control characters, are dropped.
pub fn sgr_spans(s: &str) -> Vec<(String, Sgr)> {
    use rush_core::text::Color;
    let mut out: Vec<(String, Sgr)> = Vec::new();
    let mut st = Sgr::default();
    let mut text = String::new();
    let mut it = s.chars().peekable();
    let flush = |out: &mut Vec<(String, Sgr)>, text: &mut String, st: Sgr| {
        if !text.is_empty() {
            out.push((std::mem::take(text), st));
        }
    };
    while let Some(c) = it.next() {
        if c == '\t' {
            let w: usize =
                text.chars().count() + out.iter().map(|(t, _)| t.chars().count()).sum::<usize>();
            text.push_str(&" ".repeat(8 - w % 8));
            continue;
        }
        if c != '\x1b' {
            if !c.is_control() {
                text.push(c);
            }
            continue;
        }
        match it.peek() {
            Some('[') => {
                it.next();
                let mut params = String::new();
                let mut end = ' ';
                for c in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        end = c;
                        break;
                    }
                    params.push(c);
                }
                if end != 'm' {
                    continue;
                }
                flush(&mut out, &mut text, st);
                let nums: Vec<u16> = params
                    .split([';', ':'])
                    .map(|p| p.parse().unwrap_or(0))
                    .collect();
                let mut i = 0;
                while i < nums.len() {
                    match nums[i] {
                        0 => st = Sgr::default(),
                        1 => st.bold = true,
                        2 => st.dim = true,
                        3 => st.italic = true,
                        4 => st.underline = true,
                        22 => (st.bold, st.dim) = (false, false),
                        23 => st.italic = false,
                        24 => st.underline = false,
                        39 => st.fg = None,
                        n @ 30..=37 => st.fg = Some(Color::Indexed((n - 30) as u8)),
                        n @ 90..=97 => st.fg = Some(Color::Indexed((n - 90 + 8) as u8)),
                        38 if nums.get(i + 1) == Some(&5) && i + 2 < nums.len() => {
                            st.fg = Some(Color::Indexed(nums[i + 2] as u8));
                            i += 2;
                        }
                        38 if nums.get(i + 1) == Some(&2) && i + 4 < nums.len() => {
                            st.fg = Some(Color::Rgb(
                                nums[i + 2] as u8,
                                nums[i + 3] as u8,
                                nums[i + 4] as u8,
                            ));
                            i += 4;
                        }
                        48 if nums.get(i + 1) == Some(&5) => i += 2,
                        48 if nums.get(i + 1) == Some(&2) => i += 4,
                        _ => {}
                    }
                    i += 1;
                }
            }
            Some(']') => {
                // An OSC (a title, a link): to its BEL or ST.
                it.next();
                while let Some(c) = it.next() {
                    if c == '\x07' || c == '\x1b' && it.peek() == Some(&'\\') {
                        if c == '\x1b' {
                            it.next();
                        }
                        break;
                    }
                }
            }
            Some(_) => {
                it.next();
            }
            None => {}
        }
    }
    flush(&mut out, &mut text, st);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each block: whether it's a console's, its rows, its script.
    type Found = (bool, Vec<(bool, String)>, String);

    fn blocks(s: &str) -> Vec<Found> {
        shell_blocks(s)
            .into_iter()
            .map(|b| (b.console, b.rows(), b.script()))
            .collect()
    }

    /// Which fences are shell, and what of each runs.
    #[test]
    fn shell_fences_and_what_runs() {
        let text = "Try:\n\n```sh\n$ rm ~/x.p12\n```\n\n```rust\nlet x = 1;\n```\n\n```console\n$ ls\na.txt\n$ echo hi\nhi\n```\n\n```\n$ make test\n```\n\n```python\n$ not shell\n```\n\n```bash\ncargo build\ncargo test\n```";
        let got = blocks(text);
        assert_eq!(got.len(), 4, "{got:?}");
        assert_eq!(got[0].2, "rm ~/x.p12");
        assert!(!got[0].0);
        assert!(got[1].0, "console");
        assert_eq!(
            got[1].1,
            vec![
                (true, "ls".to_string()),
                (false, "a.txt".to_string()),
                (true, "echo hi".to_string()),
                (false, "hi".to_string()),
            ]
        );
        assert_eq!(got[1].2, "ls\necho hi");
        assert_eq!(got[2].2, "make test");
        assert_eq!(got[3].2, "cargo build\ncargo test");
    }

    /// A reply cut at its shell blocks: the words around them kept, each
    /// block numbered as shell_blocks numbers it.
    #[test]
    fn pieces_cut_at_the_blocks() {
        let text =
            "Try:\n\n```sh\nls\n```\n\n```rust\nlet x = 1;\n```\n\nthen\n\n```bash\nmake\n```";
        let got = pieces(text);
        assert_eq!(got.len(), 4, "{got:?}");
        assert_eq!(got[0], Piece::Words("Try:\n".into()));
        assert!(matches!(&got[1], Piece::Block(0, b) if b.script() == "ls"));
        assert!(
            matches!(&got[2], Piece::Words(w) if w.contains("```rust") && w.contains("then")),
            "{got:?}"
        );
        assert!(matches!(&got[3], Piece::Block(1, b) if b.script() == "make"));
        assert_eq!(
            pieces("no shell here"),
            [Piece::Words("no shell here".into())]
        );
    }

    #[test]
    fn refs_read_back() {
        assert_eq!(
            BoxRef::parse(&block_ref(3, 2, 1)),
            Some(BoxRef::Block {
                turn: 3,
                item: 2,
                k: 1
            })
        );
        assert_eq!(
            BoxRef::parse(&bang_ref(4, 99)),
            Some(BoxRef::Bang { turn: 4, start: 99 })
        );
        assert_eq!(BoxRef::parse("t1:s:b1"), None);
        assert_eq!(BoxRef::parse("t1"), None);
    }

    /// Output past OUT_KEEP lets its oldest lines go, and counts them.
    #[test]
    fn output_is_capped() {
        let mut r = ShellRun::new("yes", "/tmp", 1);
        let line = "y".repeat(1023) + "\n";
        for _ in 0..300 {
            r.push(&line);
        }
        assert!(r.out.len() <= OUT_KEEP);
        assert_eq!(r.line_count(), 300);
        assert!(r.dropped > 0);
        assert!(r.out.starts_with('y'));
    }

    #[test]
    fn carriage_returns_leave_the_last_state() {
        let mut r = ShellRun::new("x", "/", 1);
        r.push("10%\r50%\r100%\ndone\r\n");
        assert_eq!(r.lines(), ["100%", "done"]);
    }

    #[test]
    fn sgr_colours_are_read() {
        let s = sgr_spans("\x1b[31mred\x1b[0m \x1b[1;38;5;208mhot\x1b[m\x1b]0;title\x07!");
        assert_eq!(s.len(), 4, "{s:?}");
        assert_eq!(s[0].0, "red");
        assert_eq!(s[0].1.fg, Some(rush_core::text::Color::Indexed(1)));
        assert_eq!(s[2].0, "hot");
        assert!(s[2].1.bold);
        assert_eq!(s[2].1.fg, Some(rush_core::text::Color::Indexed(208)));
        assert_eq!(s[3].0, "!");
        assert_eq!(plain_text("\x1b[32mok\x1b[0m"), "ok");
    }

    /// What goes in the box for the agent: the command, how it ended,
    /// and the end of its output.
    #[test]
    fn send_text_trims() {
        let mut r = ShellRun::new("ls", "/tmp", 1);
        r.push("\x1b[34ma\x1b[0m\nb\n");
        r.finish(Some(0), 2);
        assert_eq!(r.send_text(), "I ran `ls` (exit 0):\n```\na\nb\n```");
        let mut r = ShellRun::new("seq 200", "/tmp", 1);
        for i in 1..=200 {
            r.push(&format!("{i}\n"));
        }
        r.finish(Some(1), 2);
        let t = r.send_text();
        assert!(
            t.starts_with("I ran `seq 200` (exit 1):\n```\n… 120 earlier lines left out\n121\n")
        );
        assert!(t.ends_with("200\n```"));
        let mut r = ShellRun::new("a\nb", "/tmp", 1);
        r.finish(None, 2);
        r.stopped = true;
        assert_eq!(
            r.send_text(),
            "I ran this (stopped):\n```sh\na\nb\n```\n(no output)"
        );
    }
}
