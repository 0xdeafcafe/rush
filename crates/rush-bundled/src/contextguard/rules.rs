//! The context guard's rules, as pure functions over what it keeps of a
//! session: each agent's reads, the files it was told were too big to
//! read whole, and its last shell command's run of identical failures.
//! The hook (hook.rs) reads the disk and the clock; nothing here does.
//! Rust-only: Go rush has no context guard.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Below this many bytes a file is read again without a word: refusing
/// costs about what reading does.
pub const SMALL_FILE: u64 = 2 * 1024;
/// A whole read of a text file past this many bytes is refused once.
pub const BIG_FILE_BYTES: u64 = 48 * 1024;
/// A whole read of a text file past this many lines is refused once.
pub const BIG_FILE_LINES: u64 = 1000;
/// How many lines a read with no limit gets (Claude Code's Read).
pub const READ_LINES: u64 = 2000;
/// How many identical failures in a row before the next run is refused.
pub const REPEATS: u32 = 3;
/// How much of a failure's output, from its end, says whether two
/// failures were the same.
const TAIL_LINES: usize = 12;

/// Which rules are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub reread: bool,
    pub big_read: bool,
    pub repeat: bool,
    pub squeeze: bool,
}

impl Rules {
    /// Every rule on.
    pub const ALL: Rules = Rules {
        reread: true,
        big_read: true,
        repeat: true,
        squeeze: true,
    };

    pub fn any(&self) -> bool {
        self.reread || self.big_read || self.repeat || self.squeeze
    }
}

/// What the guard keeps of a session, by agent: "main", or a subagent's
/// id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub agents: BTreeMap<String, Agent>,
}

/// What it keeps of one agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// The files it read and hasn't edited since, by path.
    pub reads: BTreeMap<String, Seen>,
    /// The files it was told once not to read whole.
    pub warned: BTreeSet<String>,
    /// Its last shell command's run of identical failures.
    pub bash: Option<Streak>,
}

/// A read: the lines it covered, and the file as it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    /// The first line read, from 1.
    pub start: u64,
    /// The line after the last read; None when it read to the end.
    pub end: Option<u64>,
    pub file: FileStat,
    /// When, as the agent's clock says it (14:03:12).
    pub at: String,
}

/// A file as the disk has it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileStat {
    pub size: u64,
    /// Its modification time, in nanoseconds since 1970.
    pub mtime: i128,
    /// Its lines and its content's hash, when it was cheap to read.
    pub lines: Option<u64>,
    pub hash: Option<u64>,
    /// Whether it's text: no NUL in its first bytes, and not an image, a
    /// PDF or a notebook.
    pub text: bool,
}

impl FileStat {
    /// Whether other is the same file, unchanged: same size and time, and
    /// the same content where both were hashed.
    pub fn same(&self, other: &FileStat) -> bool {
        self.size == other.size
            && self.mtime == other.mtime
            && match (self.hash, other.hash) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

/// A shell command that failed the same way, again and again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Streak {
    pub command: String,
    pub exit: String,
    pub tail: String,
    pub count: u32,
    /// Whether its next run was refused already.
    pub refused: bool,
}

/// How a shell call ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// It failed: its exit ("1", or "error" when none was said) and its
    /// output's end, normalised.
    Failed {
        exit: String,
        tail: String,
    },
    /// You stopped it: says nothing about the command.
    Interrupted,
}

/// A refused call: which rule, and what the agent is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub rule: &'static str,
    pub why: String,
}

/// The lines a read asks for: from start (1-based) to end (exclusive),
/// None for the file's end. With no limit it gets READ_LINES lines, which
/// is the end when the file has no more.
pub fn span(offset: Option<u64>, limit: Option<u64>, lines: Option<u64>) -> (u64, Option<u64>) {
    let start = offset.filter(|&o| o > 0).unwrap_or(1);
    let end = start + limit.filter(|&l| l > 0).unwrap_or(READ_LINES);
    match lines {
        // The file ends inside the read.
        Some(n) if n < end => (start, None),
        _ => (start, Some(end)),
    }
}

/// Whether a read of (start, end) holds all of the asked (start, end).
pub fn covers(had: (u64, Option<u64>), asked: (u64, Option<u64>)) -> bool {
    had.0 <= asked.0
        && match (had.1, asked.1) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(h), Some(a)) => a <= h,
        }
}

fn lines_label(start: u64, end: Option<u64>) -> String {
    match end {
        None if start <= 1 => "all of it".into(),
        None => format!("lines {start} to the end"),
        Some(e) => format!("lines {start}-{}", e.saturating_sub(1)),
    }
}

/// What the guard says of a Read of path, before it runs: refused, or let
/// through (None). file is the file now; a refusal to read it whole is
/// given once per file, so the agent's asking again reads it.
pub fn read(
    agent: &mut Agent,
    rules: Rules,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    file: &FileStat,
) -> Option<Refusal> {
    if !file.text {
        return None;
    }
    let whole = offset.is_none_or(|o| o <= 1) && limit.is_none_or(|l| l == 0);
    if rules.reread
        && file.size >= SMALL_FILE
        && let Some(seen) = agent.reads.get(path)
        && seen.file.same(file)
        && covers(
            (seen.start, seen.end),
            span(offset, limit, file.lines.or(seen.file.lines)),
        )
    {
        return Some(Refusal {
            rule: "reread",
            why: format!(
                "context-guard: {path} is unchanged since you read it at {} ({}). Use what that read gave you; \
                 it's still in your context. Once the file changes, or you edit it, it can be read again.",
                seen.at,
                lines_label(seen.start, seen.end)
            ),
        });
    }
    if rules.big_read && whole && !agent.warned.contains(path) {
        let big_bytes = file.size > BIG_FILE_BYTES;
        let big_lines = file.lines.is_some_and(|n| n > BIG_FILE_LINES);
        if big_bytes || big_lines {
            agent.warned.insert(path.to_string());
            let size = match file.lines {
                Some(n) => format!("{n} lines, {} KB", file.size.div_ceil(1024)),
                None => format!("{} KB", file.size.div_ceil(1024)),
            };
            return Some(Refusal {
                rule: "big-read",
                why: format!(
                    "context-guard: {path} is {size}: read whole, all of it stays in your context. \
                     Find the part you need first (`rg -n <pattern> {path}`), then read it with offset and limit. \
                     If you do need all of it, ask again: the next whole read goes through."
                ),
            });
        }
    }
    None
}

/// Takes note of a Read of path that went through.
pub fn read_done(
    agent: &mut Agent,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    file: FileStat,
    at: &str,
) {
    if !file.text {
        return;
    }
    let (start, end) = span(offset, limit, file.lines);
    // A smaller read inside one the agent already has adds nothing.
    if let Some(s) = agent.reads.get(path)
        && s.file.same(&file)
        && covers((s.start, s.end), (start, end))
    {
        return;
    }
    agent.reads.insert(
        path.to_string(),
        Seen {
            start,
            end,
            file,
            at: at.to_string(),
        },
    );
}

/// The agent changed path (or tried to): its earlier read no longer
/// stands, and its failing command may now go differently.
pub fn edited(agent: &mut Agent, path: &str) {
    agent.reads.remove(path);
    agent.bash = None;
}

/// The agent's context was compacted or cleared: it no longer has what it
/// read.
pub fn forgot(agent: &mut Agent) {
    agent.reads.clear();
}

/// A command as it's compared: its words, single spaced.
pub fn normalise_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the guard says of a shell call, before it runs.
pub fn bash(agent: &mut Agent, rules: Rules, command: &str) -> Option<Refusal> {
    if !rules.repeat {
        return None;
    }
    let cmd = normalise_command(command);
    let s = agent.bash.as_mut()?;
    if s.command != cmd || s.count < REPEATS {
        return None;
    }
    if s.refused {
        // Asked again straight after: the agent knows something changed
        // that rush can't see.
        agent.bash = None;
        return None;
    }
    s.refused = true;
    let exit = if s.exit == "error" {
        "an error".to_string()
    } else {
        format!("exit {}", s.exit)
    };
    Some(Refusal {
        rule: "repeat-fail",
        why: format!(
            "context-guard: this command failed the same way {} times in a row ({exit}, the same output), with no edit \
             in between. Running it again will fail again: change something first (the code, the command, its \
             inputs), or read its output above. If something changed that rush can't see, run it again.",
            s.count
        ),
    })
}

/// Takes note of how a shell call ended.
pub fn bash_done(agent: &mut Agent, command: &str, outcome: Outcome) {
    let cmd = normalise_command(command);
    match outcome {
        Outcome::Interrupted => {}
        Outcome::Ok => agent.bash = None,
        Outcome::Failed { exit, tail } => match agent.bash.as_mut() {
            Some(s) if s.command == cmd && s.exit == exit && s.tail == tail => {
                s.count += 1;
                s.refused = false;
            }
            _ => {
                agent.bash = Some(Streak {
                    command: cmd,
                    exit,
                    tail,
                    count: 1,
                    refused: false,
                })
            }
        },
    }
}

/// How a failed shell call ended, from the error its harness gave: Claude
/// Code's starts "Exit code N".
pub fn failure(error: &str) -> Outcome {
    let text = crate::contextguard::squeeze::strip_ansi(error);
    let mut exit = "error".to_string();
    let mut body = text.as_str();
    if let Some(first) = text.lines().next()
        && let Some(n) = first.trim().strip_prefix("Exit code ")
        && n.trim().parse::<i64>().is_ok()
    {
        exit = n.trim().to_string();
        body = &text[first.len()..];
    }
    Outcome::Failed {
        exit,
        tail: tail(body),
    }
}

/// The end of a command's output, so two failures that differ only in
/// timings, ids or temporary names compare the same: its last lines, digits
/// as 0, spaces single.
pub fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(TAIL_LINES);
    lines[from..]
        .iter()
        .map(|l| {
            let mut out = String::with_capacity(l.len());
            let mut digit = false;
            for c in l.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
                if c.is_ascii_digit() {
                    if !digit {
                        out.push('0');
                    }
                    digit = true;
                } else {
                    digit = false;
                    out.push(c);
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u64, mtime: i128, lines: Option<u64>) -> FileStat {
        FileStat {
            size,
            mtime,
            lines,
            hash: Some(size ^ 7),
            text: true,
        }
    }

    #[test]
    fn spans() {
        let cases: &[(Option<u64>, Option<u64>, Option<u64>, (u64, Option<u64>))] = &[
            (None, None, Some(100), (1, None)),
            (None, None, Some(5000), (1, Some(2001))),
            (None, None, None, (1, Some(2001))),
            (Some(0), Some(0), Some(10), (1, None)),
            (Some(10), Some(20), Some(100), (10, Some(30))),
            (Some(90), Some(20), Some(100), (90, None)),
            (Some(10), None, Some(100), (10, None)),
        ];
        for &(o, l, n, want) in cases {
            assert_eq!(span(o, l, n), want, "span({o:?}, {l:?}, {n:?})");
        }
    }

    #[test]
    fn coverage() {
        let cases: &[((u64, Option<u64>), (u64, Option<u64>), bool)] = &[
            ((1, None), (1, None), true),
            ((1, None), (50, Some(60)), true),
            ((1, Some(100)), (1, Some(100)), true),
            ((1, Some(100)), (10, Some(50)), true),
            ((1, Some(100)), (10, Some(101)), false),
            ((10, Some(100)), (1, Some(50)), false),
            ((1, Some(100)), (1, None), false),
        ];
        for &(had, asked, want) in cases {
            assert_eq!(covers(had, asked), want, "covers({had:?}, {asked:?})");
        }
    }

    /// One step of a re-read story: a read asked (offset, limit) of the file
    /// as it is, whether it's refused, and whether it then went through.
    struct Step {
        what: &'static str,
        offset: Option<u64>,
        limit: Option<u64>,
        file: FileStat,
        edit_first: bool,
        forget_first: bool,
        refused: Option<&'static str>,
    }

    fn step(what: &'static str, file: FileStat, refused: Option<&'static str>) -> Step {
        Step {
            what,
            offset: None,
            limit: None,
            file,
            edit_first: false,
            forget_first: false,
            refused,
        }
    }

    #[test]
    fn rereads() {
        let f = file(10_000, 1, Some(300));
        let stories: Vec<(&str, Vec<Step>)> = vec![
            (
                "unchanged whole file read again",
                vec![
                    step("first", f.clone(), None),
                    step("again", f.clone(), Some("reread")),
                ],
            ),
            (
                "a part of a file read whole",
                vec![
                    step("whole", f.clone(), None),
                    Step {
                        offset: Some(20),
                        limit: Some(40),
                        ..step("part", f.clone(), Some("reread"))
                    },
                ],
            ),
            (
                "more than the part read before",
                vec![
                    Step {
                        offset: Some(20),
                        limit: Some(40),
                        ..step("part", f.clone(), None)
                    },
                    Step {
                        offset: Some(20),
                        limit: Some(80),
                        ..step("more", f.clone(), None)
                    },
                    Step {
                        offset: Some(30),
                        limit: Some(10),
                        ..step("inside the more", f.clone(), Some("reread"))
                    },
                ],
            ),
            (
                "changed on disk",
                vec![
                    step("first", f.clone(), None),
                    step("touched", file(10_000, 2, Some(300)), None),
                ],
            ),
            (
                "same size and time, other content",
                vec![
                    step("first", f.clone(), None),
                    step(
                        "rewritten",
                        FileStat {
                            hash: Some(1),
                            ..f.clone()
                        },
                        None,
                    ),
                ],
            ),
            (
                "edited by the agent",
                vec![
                    step("first", f.clone(), None),
                    Step {
                        edit_first: true,
                        ..step("after its edit", f.clone(), None)
                    },
                ],
            ),
            (
                "after a compaction",
                vec![
                    step("first", f.clone(), None),
                    Step {
                        forget_first: true,
                        ..step("after compact", f.clone(), None)
                    },
                ],
            ),
            (
                "small files go through",
                vec![
                    step("first", file(1500, 1, Some(40)), None),
                    step("again", file(1500, 1, Some(40)), None),
                ],
            ),
            (
                "not text",
                vec![
                    step(
                        "image",
                        FileStat {
                            text: false,
                            ..f.clone()
                        },
                        None,
                    ),
                    step(
                        "image again",
                        FileStat {
                            text: false,
                            ..f.clone()
                        },
                        None,
                    ),
                ],
            ),
        ];
        for (story, steps) in stories {
            let mut a = Agent::default();
            for s in steps {
                if s.edit_first {
                    edited(&mut a, "/w/f.rs");
                }
                if s.forget_first {
                    forgot(&mut a);
                }
                let got = read(&mut a, Rules::ALL, "/w/f.rs", s.offset, s.limit, &s.file);
                assert_eq!(
                    got.as_ref().map(|r| r.rule),
                    s.refused,
                    "{story}: {}: {got:?}",
                    s.what
                );
                if got.is_none() {
                    read_done(&mut a, "/w/f.rs", s.offset, s.limit, s.file, "14:03:12");
                }
            }
        }
    }

    #[test]
    fn reread_says_when() {
        let mut a = Agent::default();
        let f = file(10_000, 1, Some(300));
        read_done(&mut a, "/w/f.rs", None, None, f.clone(), "14:03:12");
        let r = read(&mut a, Rules::ALL, "/w/f.rs", None, None, &f).unwrap();
        assert_eq!(
            r.why,
            "context-guard: /w/f.rs is unchanged since you read it at 14:03:12 (all of it). Use what that read gave \
             you; it's still in your context. Once the file changes, or you edit it, it can be read again."
        );
        let off = Rules {
            reread: false,
            ..Rules::ALL
        };
        assert_eq!(read(&mut a, off, "/w/f.rs", None, None, &f), None);
    }

    #[test]
    fn big_reads() {
        let cases: Vec<(&str, FileStat, Option<u64>, Option<u64>, Option<&str>)> = vec![
            (
                "many lines",
                file(40_000, 1, Some(1500)),
                None,
                None,
                Some("big-read"),
            ),
            (
                "many bytes",
                file(200_000, 1, None),
                None,
                None,
                Some("big-read"),
            ),
            ("small", file(40_000, 1, Some(900)), None, None, None),
            (
                "a part",
                file(200_000, 1, Some(5000)),
                Some(1),
                Some(100),
                None,
            ),
            (
                "from an offset",
                file(200_000, 1, Some(5000)),
                Some(200),
                None,
                None,
            ),
            (
                "a PDF",
                FileStat {
                    text: false,
                    ..file(900_000, 1, None)
                },
                None,
                None,
                None,
            ),
        ];
        for (what, f, o, l, want) in cases {
            let mut a = Agent::default();
            let got = read(&mut a, Rules::ALL, "/w/big.log", o, l, &f);
            assert_eq!(got.as_ref().map(|r| r.rule), want, "{what}: {got:?}");
            // Asked again, it goes through.
            assert_eq!(
                read(&mut a, Rules::ALL, "/w/big.log", o, l, &f),
                None,
                "{what} again"
            );
        }
        let mut a = Agent::default();
        let r = read(
            &mut a,
            Rules::ALL,
            "/w/big.log",
            None,
            None,
            &file(40_000, 1, Some(1500)),
        )
        .unwrap();
        assert_eq!(
            r.why,
            "context-guard: /w/big.log is 1500 lines, 40 KB: read whole, all of it stays in your context. Find the \
             part you need first (`rg -n <pattern> /w/big.log`), then read it with offset and limit. If you do need \
             all of it, ask again: the next whole read goes through."
        );
    }

    fn failed(exit: &str, tail: &str) -> Outcome {
        Outcome::Failed {
            exit: exit.into(),
            tail: tail.into(),
        }
    }

    enum Ev {
        Run(&'static str, Outcome),
        Edit,
        Ask(&'static str, bool),
    }

    #[test]
    fn repeat_failures() {
        use Ev::*;
        let f = || failed("1", "error: x");
        let stories: Vec<(&str, Vec<Ev>)> = vec![
            (
                "three the same, the fourth refused, the fifth goes",
                vec![
                    Run("cargo test", f()),
                    Run("cargo  test", f()),
                    Ask("cargo test", false),
                    Run("cargo test\n", f()),
                    Ask("cargo test", true),
                    Ask("cargo   test", false),
                ],
            ),
            (
                "an edit between",
                vec![
                    Run("cargo test", f()),
                    Run("cargo test", f()),
                    Edit,
                    Run("cargo test", f()),
                    Ask("cargo test", false),
                ],
            ),
            (
                "a different output",
                vec![
                    Run("cargo test", f()),
                    Run("cargo test", failed("1", "error: y")),
                    Run("cargo test", f()),
                    Ask("cargo test", false),
                ],
            ),
            (
                "a different exit",
                vec![
                    Run("make", failed("2", "x")),
                    Run("make", failed("1", "x")),
                    Run("make", failed("1", "x")),
                    Ask("make", false),
                ],
            ),
            (
                "another command between",
                vec![
                    Run("cargo test", f()),
                    Run("cargo test", f()),
                    Run("ls", Outcome::Ok),
                    Run("cargo test", f()),
                    Ask("cargo test", false),
                ],
            ),
            (
                "an interrupt counts for nothing",
                vec![
                    Run("cargo test", f()),
                    Run("cargo test", f()),
                    Run("cargo test", Outcome::Interrupted),
                    Run("cargo test", f()),
                    Ask("ls", false),
                    Ask("cargo test", true),
                ],
            ),
            (
                "a success clears it",
                vec![
                    Run("cargo test", f()),
                    Run("cargo test", f()),
                    Run("cargo test", Outcome::Ok),
                    Run("cargo test", f()),
                    Ask("cargo test", false),
                ],
            ),
        ];
        for (story, evs) in stories {
            let mut a = Agent::default();
            for (i, e) in evs.into_iter().enumerate() {
                match e {
                    Run(c, o) => bash_done(&mut a, c, o),
                    Edit => edited(&mut a, "/w/x.rs"),
                    Ask(c, want) => {
                        let got = bash(&mut a, Rules::ALL, c);
                        assert_eq!(got.is_some(), want, "{story}: step {i}: {got:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn failures_and_tails() {
        let cases: &[(&str, Outcome)] = &[
            (
                "Exit code 101\nerror: test failed, to rerun pass `--lib`",
                failed("101", "error: test failed, to rerun pass `--lib`"),
            ),
            (
                "Exit code 1\n\x1b[31mFAILED\x1b[0m in 0.53s (pid 4411)",
                failed("1", "FAILED in 0.0s (pid 0)"),
            ),
            (
                "command not found: foo",
                failed("error", "command not found: foo"),
            ),
        ];
        for (error, want) in cases {
            assert_eq!(&failure(error), want, "failure({error:?})");
        }
        // Timings and ids don't make two failures different.
        assert_eq!(
            failure("Exit code 1\ntest result: FAILED. 3 passed; 1 failed; finished in 1.20s"),
            failure("Exit code 1\ntest result: FAILED. 3 passed; 1 failed; finished in 0.97s")
        );
    }
}
