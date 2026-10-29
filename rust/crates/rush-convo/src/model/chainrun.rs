//! Which command of a running chain runs now: the shells the agent's
//! Bash calls run in, matched to the calls still running, and each
//! shell's processes to the commands of its chain. Port of
//! internal/convo/chainrun.go (Shell :15, ShellProc :22, partRun :31,
//! PartProc :43, WatchShells :50, watch :117, wrappers :172, bare :176,
//! partOf :199, RunningPart :346, live :358, RunningPart() :380).
//! What the opened call says of each command (partMarks) isn't drawn yet.

use std::collections::{BTreeMap, HashMap};

use rush_core::time::Ms;
use rush_core::tool::Kind;

use super::{Item, Session, Status, StepId};
use crate::render::{Segment, fields_of, segments, shell_lines, tool_run, unquote};

/// A shell running under the agent's process (Shell). command is the
/// exact -c argument when known; cmd its whole command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shell {
    pub cmd: String,
    pub command: String,
    pub start: Ms,
    pub kids: Vec<ShellProc>,
}

/// A process under a Bash call's shell, and its own children (ShellProc).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellProc {
    pub pid: i32,
    pub args: Vec<String>,
    pub start: Ms,
    pub kids: Vec<ShellProc>,
}

/// A process running a command of a chain, and its start, which tells it
/// from a later process given the same pid (PartProc).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartProc {
    pub pid: i32,
    pub start: Ms,
}

/// When one command of a chain was seen running: from its process's
/// start until it was gone, or the next command's start (partRun).
#[derive(Clone, Debug, Default)]
struct PartRun {
    start: Ms,
    end: Ms,
    seen: Ms,
    procs: Vec<PartProc>,
    /// How many times it was seen start: more than once in a loop.
    runs: usize,
}

/// What's known of a step's chain as it runs: each command seen, and the
/// furthest one seen (st.parts, st.at).
#[derive(Clone, Debug, Default)]
pub(super) struct Chain {
    parts: BTreeMap<usize, PartRun>,
    at: usize,
}

/// The command of a Bash call's chain that runs now (RunningPart).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunningPart {
    /// As written: go test ./internal/ui
    pub command: String,
    /// The processes running it.
    pub procs: Vec<PartProc>,
    /// What the chain does once it's gone: "stops" (the next command runs
    /// only if it succeeded), "carries on", or "ends" (it was the last).
    pub then: String,
    /// Its place in the chain: 2 of 5.
    pub at: usize,
    pub of: usize,
}

impl Session {
    /// Matches the shells the agent has running to the Bash calls still
    /// running, and each shell's processes to the commands of its chain,
    /// so the call can say which of them runs now (WatchShells).
    pub fn watch_shells(&mut self, shells: &[Shell], now: Ms) {
        let mut steps: Vec<StepId> = Vec::new();
        if let Some(t) = self.live() {
            let mut stack: Vec<StepId> = t
                .items
                .iter()
                .rev()
                .filter_map(|it| match it {
                    Item::Step(id) => Some(*id),
                    _ => None,
                })
                .collect();
            while let Some(id) = stack.pop() {
                let st = self.step(id);
                if st.status == Status::Running && st.kind == Kind::Shell {
                    steps.push(id);
                }
                stack.extend(st.children.iter().rev());
            }
        }
        // A call sent to the background has returned; its shell runs on.
        for j in &self.jobs {
            if let Some(&id) = self.by_id.get(j.tool_use_id.as_str()) {
                let st = self.step(id);
                if j.running() && st.status != Status::Running && st.kind == Kind::Shell {
                    steps.push(id);
                }
            }
        }
        if steps.is_empty() || shells.is_empty() {
            return;
        }
        // A shell's command line holds the call's command as eval quotes
        // it; one a hook rewrote goes to the call it started soonest after.
        let mut used = vec![false; shells.len()];
        let mut left = Vec::new();
        for &id in &steps {
            let cmd = self.step(id).call().input.command.clone();
            let q = format!("'{}'", cmd.replace('\'', r"'\''"));
            let found = shells.iter().enumerate().position(|(i, sh)| {
                !used[i]
                    && (!sh.command.is_empty() && sh.command == cmd
                        || sh.command.is_empty() && sh.cmd.contains(&q))
            });
            match found {
                Some(i) => {
                    used[i] = true;
                    self.watch(id, &shells[i], now);
                }
                None => left.push(id),
            }
        }
        for id in left {
            let start = self.step(id).start;
            let mut best: Option<usize> = None;
            for (i, sh) in shells.iter().enumerate() {
                if used[i] || start == 0 || !sh.command.is_empty() {
                    continue;
                }
                if sh.start > start - 2000 && best.is_none_or(|b| sh.start < shells[b].start) {
                    best = Some(i);
                }
            }
            if let Some(b) = best {
                used[b] = true;
                self.watch(id, &shells[b], now);
            }
        }
    }

    /// Notes which commands of step id's chain sh has running now (watch).
    fn watch(&mut self, id: StepId, sh: &Shell, now: Ms) {
        let segs = segments(&self.step(id).call().input.command);
        if segs.len() < 2 {
            return;
        }
        let ch = self.chains.entry(id).or_default();
        // A process that is one of the chain's commands stands for all it
        // starts: make's git is make's, not the chain's git status.
        let mut seen: HashMap<usize, Ms> = HashMap::new();
        let mut procs: HashMap<usize, Vec<PartProc>> = HashMap::new();
        fn walk(
            ps: &[ShellProc],
            segs: &[Segment],
            from: usize,
            seen: &mut HashMap<usize, Ms>,
            procs: &mut HashMap<usize, Vec<PartProc>>,
        ) {
            for p in ps {
                if let Some(k) = part_of(segs, &p.args, from) {
                    let t = seen.entry(k).or_insert(p.start);
                    if p.start < *t {
                        *t = p.start;
                    }
                    procs.entry(k).or_default().push(PartProc {
                        pid: p.pid,
                        start: p.start,
                    });
                    continue;
                }
                walk(&p.kids, segs, from, seen, procs);
            }
        }
        walk(&sh.kids, &segs, ch.at, &mut seen, &mut procs);
        for (&k, &t) in &seen {
            let r = ch.parts.entry(k).or_insert(PartRun {
                start: t,
                runs: 1,
                ..PartRun::default()
            });
            if t < r.start {
                r.start = t;
            } else if t > r.start {
                // A new process for it: a loop has come round to it again.
                r.start = t;
                r.runs += 1;
            }
            r.seen = now;
            r.end = 0;
            r.procs = procs.remove(&k).unwrap_or_default();
            ch.at = ch.at.max(k);
        }
        // A command no longer seen ended when the next one started, or
        // else between the last look and this one.
        for (k, r) in ch.parts.iter_mut() {
            if seen.contains_key(k) || r.end != 0 {
                continue;
            }
            r.end = now;
            r.procs.clear();
            for (&j, &t) in &seen {
                if j > *k && t > r.seen && t < r.end {
                    r.end = t;
                }
            }
        }
    }

    /// Whether a call's command still runs: the call itself, or the task
    /// it went on as in the background (live).
    fn call_live(&self, id: StepId) -> bool {
        let st = self.step(id);
        st.status == Status::Running || self.job_running(&st.id)
    }

    /// What the Bash call with this tool call id runs now, if it's a chain
    /// seen running (RunningPart).
    pub fn running_part(&self, call_id: &str) -> Option<RunningPart> {
        let id = self.step_by_call(call_id)?;
        if !self.call_live(id) || tool_run(self.step(id)).is_some() {
            return None;
        }
        let ch = self.chains.get(&id)?;
        let (&k, r) = ch
            .parts
            .iter()
            .rfind(|(_, r)| r.end == 0 && !r.procs.is_empty())?;
        let cmd = &self.step(id).call().input.command;
        let segs = segments(cmd);
        let sg = segs.get(k)?;
        let mut rp = RunningPart {
            command: sg.text.clone(),
            procs: r.procs.clone(),
            then: "ends".into(),
            at: k + 1,
            of: segs.len(),
        };
        // The next command's line starts with what joins it on.
        let mut n: isize = -1;
        for l in shell_lines(cmd) {
            if l.verbatim || l.depth > 0 {
                continue;
            }
            n += 1;
            if n == k as isize + 1 {
                rp.then = if l.text.starts_with("&& ") {
                    "stops"
                } else {
                    "carries on"
                }
                .into();
                break;
            }
        }
        Some(rp)
    }
}

/// The wrappers that run the command after them, and how many words of
/// their own come first (wrappers).
fn wrapper(w: &str) -> Option<usize> {
    match w {
        "sudo" | "env" | "time" | "nice" | "nohup" | "command" | "exec" | "rtk" | "caffeinate" => {
            Some(0)
        }
        "timeout" => Some(1),
        _ => None,
    }
}

/// A path's last element (filepath.Base).
fn base(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { "." } else { "/" };
    }
    t.rsplit('/').next().unwrap_or(t)
}

/// A command's words from its program on: past NAME=value assignments and
/// the wrappers that run it, with the program's folder gone (bare).
fn bare(words: &[String]) -> Vec<String> {
    let mut w = words;
    while let Some(first) = w.first() {
        if first.contains('=') && !first.starts_with('=') {
            w = &w[1..];
        } else if let Some(n) = wrapper(base(first)) {
            w = &w[1..];
            while w.first().is_some_and(|a| a.starts_with('-')) {
                w = &w[1..];
            }
            if w.len() > n {
                w = &w[n..];
            }
        } else {
            let mut out = vec![base(first).to_string()];
            out.extend(w[1..].iter().cloned());
            return out;
        }
    }
    Vec::new()
}

/// Which command of the chain a process runs: the one whose program it is
/// and that shares most words with it, the earliest from part from on
/// when several do equally; None for none (partOf).
fn part_of(segs: &[Segment], args: &[String], from: usize) -> Option<usize> {
    let argv = bare(args);
    if argv.is_empty() {
        return None;
    }
    // A script runs as its interpreter: bash ./build.sh.
    let mut progs = vec![argv[0].clone()];
    if argv.len() > 1 && !argv[1].starts_with('-') {
        progs.push(base(&argv[1]).to_string());
    }
    let have: std::collections::HashSet<&str> = argv[1..].iter().map(String::as_str).collect();
    let (mut best, mut score): (Option<usize>, usize) = (None, 0);
    for (k, sg) in segs.iter().enumerate() {
        let mut sc = 0;
        for stage in std::iter::once(&sg.text).chain(sg.filters.iter()) {
            let w = bare(&fields_of(stage));
            if w.is_empty() || !progs.contains(&w[0]) {
                continue;
            }
            let n = 1 + w[1..].iter().filter(|a| have.contains(unquote(a))).count();
            sc = sc.max(n);
        }
        let before = best.is_none_or(|b| b < from);
        if sc > score || sc == score && sc > 0 && before && k >= from {
            best = Some(k);
            score = sc;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::event::{Event, Message, Part, PartKind};
    use rush_core::tool::{Call, Input};

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn proc_(pid: i32, a: &str, start: Ms) -> ShellProc {
        ShellProc {
            pid,
            args: args(a),
            start,
            kids: Vec::new(),
        }
    }

    /// A running Bash call b1 running cmd.
    fn session(cmd: &str) -> Session {
        let mut s = Session::new();
        let call = Call {
            id: "b1".into(),
            name: "Bash".into(),
            kind: Kind::Shell,
            input: Input {
                command: cmd.into(),
                ..Input::default()
            },
            ..Call::default()
        };
        s.apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: "m1".into(),
                parts: vec![Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(call)),
                    ..Part::default()
                }],
                ..Message::default()
            }),
            1_000,
        );
        s
    }

    /// The command of a chain that runs now, its place and what the chain
    /// does after it; then the next once it starts (chainrun_test.go).
    #[test]
    fn the_command_running_now() {
        let cmd = "go build ./... && go test ./internal/ui; echo done";
        let mut s = session(cmd);
        let sh = |kids| Shell {
            command: cmd.into(),
            start: 1_000,
            kids,
            ..Shell::default()
        };
        s.watch_shells(&[sh(vec![proc_(11, "go build ./...", 1_100)])], 2_000);
        let rp = s.running_part("b1").expect("running");
        assert_eq!(
            (rp.command.as_str(), rp.at, rp.of, rp.then.as_str()),
            ("go build ./...", 1, 3, "stops")
        );
        s.watch_shells(
            &[sh(vec![proc_(
                12,
                "/usr/local/go/bin/go test ./internal/ui",
                3_000,
            )])],
            4_000,
        );
        let rp = s.running_part("b1").expect("running");
        assert_eq!(
            (rp.command.as_str(), rp.at, rp.then.as_str()),
            ("go test ./internal/ui", 2, "carries on")
        );
        assert_eq!(
            rp.procs,
            [PartProc {
                pid: 12,
                start: 3_000
            }]
        );
        // One command alone is no chain.
        let mut s = session("sleep 10");
        s.watch_shells(
            &[Shell {
                command: "sleep 10".into(),
                kids: vec![proc_(5, "sleep 10", 1_100)],
                ..Shell::default()
            }],
            2_000,
        );
        assert_eq!(s.running_part("b1"), None);
    }

    #[test]
    fn bare_drops_wrappers_and_assignments() {
        assert_eq!(
            bare(&args("FOO=1 sudo -E timeout 5 /usr/bin/make test")),
            ["make", "test"]
        );
        assert!(bare(&args("A=1")).is_empty());
    }
}
