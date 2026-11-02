//! Which command of a running chain runs now: the shells the agent's
//! Bash calls run in, matched to the calls still running, and each
//! shell's processes to the commands of its chain. Port of
//! internal/convo/chainrun.go (Shell :15, ShellProc :22, partRun :31,
//! PartProc :43, WatchShells :50, watch :117, wrappers :172, bare :176,
//! partOf :199, partMarks :243, runningPart :321, RunningPart :346,
//! live :358, RunningPart() :380).

use std::collections::{BTreeMap, HashMap};

use rush_core::time::Ms;
use rush_core::tool::Kind;

use super::{Item, Session, Status, StepId};
use crate::render::{Segment, fields_of, run_text, segments, shell_lines, tool_run, unquote};

/// How many of a script's commands are kept, the latest.
const INNER_KEPT: usize = 50;

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
    /// A script file's commands (bash build.sh), which the call's text
    /// can't name: each process seen run under it, oldest first.
    inner: Vec<InnerRun>,
}

/// One command a script was seen run: its first words, its process, and
/// when it started and ended (0 while it runs).
#[derive(Clone, Debug, Default)]
struct InnerRun {
    words: String,
    proc_: PartProc,
    end: Ms,
}

/// What an opened script call says of each command seen run under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScriptRun {
    pub words: String,
    pub mark: PartMark,
}

/// The command of a Bash call's chain that runs now (RunningPart).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunningPart {
    /// As written: go test ./internal/ui
    pub command: String,
    /// The processes running it.
    pub procs: Vec<PartProc>,
    /// What the chain does once it's gone: "stops" (the next command runs
    /// only if it succeeded), "carries on", "ends" (it was the last), or
    /// "script" (a script file's command: the script goes on after it).
    pub then: String,
    /// Its place in the chain: 2 of 5.
    pub at: usize,
    pub of: usize,
}

/// What an opened call says of one command of its chain: how long it ran,
/// or runs (partMarks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PartMark {
    /// Not seen, and not known to have run.
    Unknown,
    /// Running now, since then; runs counts a loop's rounds.
    Running { since: Ms, runs: usize },
    /// Running now, but rush paused it.
    Paused,
    /// Ran this long; failed when it's where the failed call stopped.
    Ran { took: Ms, runs: usize, failed: bool },
    /// Came and went between looks: ran in a gap this long (None when
    /// unknown), all of it, or at most it when shared (less).
    Between { took: Option<Ms>, less: bool },
}

impl Session {
    /// The Bash calls running now, a subagent's among them, and those sent
    /// to the background whose shell runs on.
    fn running_shells(&self) -> Vec<StepId> {
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
        steps
    }

    /// Whether a Bash call runs now: the agent's shells are worth a look,
    /// to say which command of a chain runs.
    pub fn shell_running(&self) -> bool {
        !self.running_shells().is_empty()
    }

    /// Matches the shells the agent has running to the Bash calls still
    /// running, and each shell's processes to the commands of its chain,
    /// so the call can say which of them runs now (WatchShells).
    pub fn watch_shells(&mut self, shells: &[Shell], now: Ms) {
        let steps = self.running_shells();
        if steps.is_empty() || shells.is_empty() {
            return;
        }
        // A shell's command line holds the call's command as eval quotes
        // it; one a hook rewrote goes to the call it started soonest after.
        let mut used = vec![false; shells.len()];
        let mut left = Vec::new();
        for &id in &steps {
            let st = self.step(id);
            let cmd = st.call().input.command.clone();
            let call = st.id.to_string();
            let found =
                (0..shells.len()).find(|&i| !used[i] && runs_call(&shells[i], &cmd, &call));
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
            // A script file's commands aren't in the call's text: the
            // processes under it say which runs.
            if script_call(&segs) {
                self.watch_script(id, &segs, sh, now);
            }
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

    /// Notes which commands the script step id runs (bash build.sh,
    /// ./build.sh, source env.sh) sh has running under it now: the
    /// processes right under the script's own, through its subshells.
    fn watch_script(&mut self, id: StepId, segs: &[Segment], sh: &Shell, now: Ms) {
        fn find<'a>(ps: &'a [ShellProc], segs: &[Segment]) -> Option<&'a ShellProc> {
            for p in ps {
                if part_of(segs, &p.args, 0) == Some(0) {
                    return Some(p);
                }
                if let Some(f) = find(&p.kids, segs) {
                    return Some(f);
                }
            }
            None
        }
        fn cmds(ps: &[ShellProc], fork: &str, seen: &mut Vec<(String, PartProc)>) {
            for p in ps {
                // A subshell of the script's own (a pipe, $(…)) runs as it.
                if p.args.join(" ") == fork {
                    cmds(&p.kids, fork, seen);
                    continue;
                }
                seen.push((
                    short_words(&p.args),
                    PartProc {
                        pid: p.pid,
                        start: p.start,
                    },
                ));
            }
        }
        let mut seen = Vec::new();
        if sourced(segs) {
            // source env.sh: the shell runs it itself.
            cmds(&sh.kids, &sh.cmd, &mut seen);
        } else {
            let Some(mut p) = find(&sh.kids, segs) else {
                return;
            };
            // rtk bash build.sh: the script's own process is under its
            // wrapper's.
            loop {
                match p.kids.as_slice() {
                    [k] if bare(&k.args) == bare(&p.args) => p = k,
                    _ => break,
                }
            }
            cmds(&p.kids, &p.args.join(" "), &mut seen);
        }
        let ch = self.chains.entry(id).or_default();
        for (words, pp) in &seen {
            if !ch.inner.iter().any(|r| r.proc_ == *pp) {
                ch.inner.push(InnerRun {
                    words: words.clone(),
                    proc_: *pp,
                    end: 0,
                });
            }
        }
        // One no longer seen ended when a later one started, or else
        // between the last look and this one.
        for r in ch.inner.iter_mut().filter(|r| r.end == 0) {
            if seen.iter().any(|(_, p)| *p == r.proc_) {
                continue;
            }
            r.end = seen
                .iter()
                .map(|(_, p)| p.start)
                .filter(|&t| t > r.proc_.start)
                .min()
                .unwrap_or(now);
        }
        ch.inner.sort_by_key(|r| r.proc_.start);
        let n = ch.inner.len();
        if n > INNER_KEPT {
            ch.inner = ch.inner.split_off(n - INNER_KEPT);
        }
    }

    /// Whether a call's command still runs: the call itself, or the task
    /// it went on as in the background (live).
    pub(crate) fn call_live(&self, id: StepId) -> bool {
        let st = self.step(id);
        st.status == Status::Running || self.job_running(&st.id)
    }

    /// The commands seen run under script call id, oldest first, each
    /// with how long it ran or runs; empty for a call that isn't a script
    /// or none seen. paused: rush has the one running now stopped.
    pub(crate) fn script_runs(&self, id: StepId, now: Ms, paused: bool) -> Vec<ScriptRun> {
        let Some(ch) = self.chains.get(&id) else {
            return Vec::new();
        };
        let st = self.step(id);
        let live = self.call_live(id);
        let end = if live || st.end == 0 { now } else { st.end };
        ch.inner
            .iter()
            .map(|r| {
                let mark = match r.end {
                    0 if live && paused => PartMark::Paused,
                    0 if live => PartMark::Running {
                        since: r.proc_.start,
                        runs: 1,
                    },
                    e => PartMark::Ran {
                        took: ((if e == 0 { end } else { e }) - r.proc_.start).max(0),
                        runs: 1,
                        failed: false,
                    },
                };
                ScriptRun {
                    words: r.words.clone(),
                    mark,
                }
            })
            .collect()
    }

    /// The command of step id's chain that runs now, as its place and its
    /// first words: "2/4 go test"; a loop's one command, "cargo test · run
    /// 2"; a script's, "now cargo test"; "" when none is known
    /// (runningPart).
    pub(crate) fn running_label(&self, id: StepId) -> String {
        match self.running_now(id) {
            Some((p, w)) if p.is_empty() => w,
            Some((p, w)) => format!("{p} {w}"),
            None => String::new(),
        }
    }

    /// The command of step id's chain that runs now: its place ("2/4",
    /// "now" for a script's, "" when it's the chain's only command) and
    /// its first words, with a loop's round ("go test · run 2").
    pub(crate) fn running_now(&self, id: StepId) -> Option<(String, String)> {
        let ch = self.chains.get(&id).filter(|_| self.call_live(id))?;
        if let Some(r) = ch.inner.iter().rfind(|r| r.end == 0) {
            return Some(("now".into(), r.words.clone()));
        }
        let (&k, r) = ch.parts.iter().rfind(|(_, r)| r.end == 0)?;
        let segs = segments(&self.step(id).call().input.command);
        let sg = segs.get(k)?;
        let w = bare(&fields_of(
            &run_text(&sg.text).unwrap_or_else(|| sg.text.clone()),
        ));
        let (at, of) = place(&segs, k);
        let p = if of > 1 {
            format!("{at}/{of}")
        } else {
            String::new()
        };
        let mut out = w.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
        if r.runs > 1 {
            out.push_str(&format!(" · run {}", r.runs));
        }
        Some((p, out))
    }

    /// The command of step id's chain running now, while the call is: the
    /// one an opened call draws brighter.
    pub(crate) fn part_now(&self, id: StepId) -> Option<usize> {
        let ch = self.chains.get(&id)?;
        let r = ch.parts.get(&ch.at)?;
        (r.end == 0 && self.call_live(id)).then_some(ch.at)
    }

    /// What the opened call says of each of the n commands of step id's
    /// chain: how long it ran, or runs; empty when none was seen to
    /// (partMarks). paused: rush has its command running now stopped.
    pub(crate) fn part_marks(&self, id: StepId, n: usize, now: Ms, paused: bool) -> Vec<PartMark> {
        let Some(ch) = self.chains.get(&id).filter(|c| !c.parts.is_empty()) else {
            return Vec::new();
        };
        if n < 2 {
            return Vec::new();
        }
        let st = self.step(id);
        let live = self.call_live(id);
        let end = if live || st.end == 0 { now } else { st.end };
        let mut marks = vec![PartMark::Unknown; n];
        for (&k, r) in ch.parts.iter().filter(|(k, _)| **k < n) {
            let mut stop = r.end;
            if stop == 0 && live {
                marks[k] = if paused {
                    PartMark::Paused
                } else {
                    PartMark::Running {
                        since: r.start,
                        runs: r.runs,
                    }
                };
                continue;
            }
            if stop == 0 {
                stop = end;
            }
            marks[k] = PartMark::Ran {
                took: (stop - r.start).max(0),
                runs: r.runs,
                failed: st.status == Status::Failed && k == ch.at,
            };
        }
        // Commands that came and went between looks, before one that was
        // seen, ran; so did those after the last when the whole chain did.
        // They ran in the gap between the ones seen either side: all of it
        // when alone there, a share of it when not.
        let last = if st.status == Status::Ok {
            n - 1
        } else {
            ch.at
        };
        let mut k = 0;
        while k <= last && k < n {
            if marks[k] != PartMark::Unknown {
                k += 1;
                continue;
            }
            let (mut from, mut to, mut gap) = (st.start, end, k);
            if let Some((_, r)) = ch.parts.range(..k).next_back() {
                from = r.end;
            }
            while gap + 1 < n && !ch.parts.contains_key(&(gap + 1)) && gap < last {
                gap += 1;
            }
            if let Some(r) = ch.parts.get(&(gap + 1)) {
                to = r.start;
            }
            let took = (from != 0 && to >= from).then_some(to - from);
            for m in marks.iter_mut().take(gap + 1).skip(k) {
                *m = PartMark::Between {
                    took,
                    less: gap > k,
                };
            }
            k = gap + 1;
        }
        // A comment, a loop's head or its done runs nothing: no time.
        let segs = segments(&st.call().input.command);
        if segs.len() == n {
            for (m, sg) in marks.iter_mut().zip(&segs) {
                if run_text(&sg.text).is_none() {
                    *m = PartMark::Unknown;
                }
            }
        }
        marks
    }

    /// What the Bash call with this tool call id runs now, if it's a chain
    /// seen running (RunningPart).
    pub fn running_part(&self, call_id: &str) -> Option<RunningPart> {
        let id = self.step_by_call(call_id)?;
        if !self.call_live(id) || tool_run(self.step(id)).is_some() {
            return None;
        }
        let ch = self.chains.get(&id)?;
        // A script's command running now: killed, the script goes on as
        // it would after it failed.
        if let Some(r) = ch.inner.iter().rfind(|r| r.end == 0) {
            return Some(RunningPart {
                command: r.words.clone(),
                procs: ch
                    .inner
                    .iter()
                    .filter(|r| r.end == 0)
                    .map(|r| r.proc_)
                    .collect(),
                then: "script".into(),
                at: ch.inner.len(),
                of: 0,
            });
        }
        let (&k, r) = ch
            .parts
            .iter()
            .rfind(|(_, r)| r.end == 0 && !r.procs.is_empty())?;
        let cmd = &self.step(id).call().input.command;
        let segs = segments(cmd);
        let sg = segs.get(k)?;
        let (at, of) = place(&segs, k);
        let mut rp = RunningPart {
            command: run_text(&sg.text).unwrap_or_else(|| sg.text.clone()),
            procs: r.procs.clone(),
            then: "ends".into(),
            at,
            of,
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

/// Whether shell sh runs the call with this command and tool call id: its
/// -c argument is the command; or its eval wrapper's is, Claude Code's
/// `… && eval 'cargo test' < /dev/null && pwd -P >| …` (a quote in it
/// written '"'"', or '\''); or it runs the script rush's gate wrote the
/// call to (rush-script …/<call id>.sh).
pub(crate) fn runs_call(sh: &Shell, cmd: &str, call: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    if !sh.command.is_empty() && sh.command == cmd {
        return true;
    }
    if eval_arg(&sh.cmd).is_some_and(|e| e == cmd) {
        return true;
    }
    let q = format!("'{}'", cmd.replace('\'', r"'\''"));
    if sh.cmd.contains(&q) {
        return true;
    }
    !call.is_empty()
        && !call.contains(['/', '\\', '.'])
        && sh.cmd.contains("rush-script")
        && sh.cmd.contains(&format!("/{call}.sh"))
}

/// The command a shell's eval wrapper runs, unquoted from its command
/// line: `source … && eval 'echo it'"'"'s' < /dev/null` is `echo it's`;
/// None when it has no eval of a quoted word.
pub(crate) fn eval_arg(line: &str) -> Option<String> {
    let at = match line.find(" eval '") {
        Some(i) => i + " eval ".len(),
        None if line.starts_with("eval '") => "eval ".len(),
        None => return None,
    };
    let b = line.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = at;
    loop {
        match b.get(i) {
            Some(b'\'') => {
                let end = i + 1 + line[i + 1..].find('\'')?;
                out.extend_from_slice(&b[i + 1..end]);
                i = end + 1;
            }
            Some(b'"') => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' && matches!(b.get(j + 1), Some(b'"' | b'\\' | b'$' | b'`')) {
                        j += 1;
                    }
                    out.push(b[j]);
                    j += 1;
                }
                if j >= b.len() {
                    return None;
                }
                i = j + 1;
            }
            Some(b'\\') if i + 1 < b.len() => {
                out.push(b[i + 1]);
                i += 2;
            }
            _ => break,
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Whether a call's one command runs a script file, whose commands the
/// call's text can't name: bash build.sh, ./build.sh, scripts/check.sh,
/// source env.sh.
fn script_call(segs: &[Segment]) -> bool {
    let [sg] = segs else {
        return false;
    };
    if !sg.body.is_empty() {
        return false; // fed its lines by a heredoc: they're in the call
    }
    let Some(t) = run_text(&sg.text) else {
        return false;
    };
    let words = fields_of(&t);
    let w = bare(&words);
    let Some(p) = w.first() else {
        return false;
    };
    // The program as written, its folder kept: ./check is a file.
    let written = words
        .iter()
        .find(|a| !(a.contains('=') && !a.starts_with('=')) && wrapper(base(a)).is_none())
        .map(String::as_str)
        .unwrap_or("");
    match p.as_str() {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => {
            // bash -lc 'make test' runs its words, not a file.
            let inline = w[1..]
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'));
            !inline && w[1..].iter().any(|a| !a.starts_with('-'))
        }
        "source" | "." => w.len() > 1,
        _ => {
            [".sh", ".bash", ".zsh"].iter().any(|x| p.ends_with(*x))
                || written.starts_with("./")
                || written.starts_with("../")
        }
    }
}

/// Whether a script call runs its file in the shell itself: source env.sh.
fn sourced(segs: &[Segment]) -> bool {
    segs.first()
        .and_then(|sg| run_text(&sg.text))
        .is_some_and(|t| {
            let w = bare(&fields_of(&t));
            matches!(w.first().map(String::as_str), Some("source" | "."))
        })
}

/// A process's first words, as a row names what runs: its program, then
/// up to two words that aren't flags (cargo test rush-convo).
fn short_words(args: &[String]) -> String {
    let w = bare(args);
    let mut out: Vec<&str> = Vec::new();
    for (i, a) in w.iter().enumerate() {
        if i == 0 {
            out.push(a);
        } else if !a.starts_with('-') && !a.is_empty() {
            out.push(base(a));
        }
        if out.len() == 3 {
            break;
        }
    }
    out.join(" ")
}

/// Command k's place among the chain's commands that run something, and
/// how many do: a comment, a loop's head or its done isn't counted.
fn place(segs: &[Segment], k: usize) -> (usize, usize) {
    let runs: Vec<bool> = segs.iter().map(|s| run_text(&s.text).is_some()).collect();
    let at = runs.iter().take(k + 1).filter(|r| **r).count().max(1);
    let of = runs.iter().filter(|r| **r).count().max(at);
    (at, of)
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
            // do cargo test $x: the loop's words aren't the program's.
            let Some(t) = run_text(stage) else { continue };
            let w = bare(&fields_of(&t));
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

    /// Commands no look caught running still say how long they took: the
    /// gap they ran in, alone, or at most it when they shared it
    /// (TestPartMarksFillGaps).
    #[test]
    fn part_marks_fill_gaps() {
        let mut s = session("a && b && c && d");
        let id = s.step_by_call("b1").unwrap();
        s.chains.insert(
            id,
            Chain {
                parts: BTreeMap::from([(
                    2,
                    PartRun {
                        start: 12_000,
                        end: 17_000,
                        runs: 1,
                        ..PartRun::default()
                    },
                )]),
                at: 0,
                ..Chain::default()
            },
        );
        let st = &mut s.steps[id as usize];
        (st.status, st.start, st.end) = (Status::Ok, 10_000, 20_000);
        let shared = PartMark::Between {
            took: Some(2_000),
            less: true,
        };
        assert_eq!(
            s.part_marks(id, 4, 30_000, false),
            [
                shared,
                shared,
                PartMark::Ran {
                    took: 5_000,
                    runs: 1,
                    failed: false
                },
                PartMark::Between {
                    took: Some(3_000),
                    less: false
                },
            ]
        );
        // A start not known: no time for the gap before the first seen.
        s.steps[id as usize].start = 0;
        let unknown = PartMark::Between {
            took: None,
            less: true,
        };
        assert_eq!(s.part_marks(id, 4, 30_000, false)[..2], [unknown, unknown]);
        // One command alone is no chain.
        assert!(s.part_marks(id, 1, 30_000, false).is_empty());
    }

    /// The row's word for the command running now: its place and first
    /// words, and a loop's round (runningPart).
    #[test]
    fn running_label_names_the_place() {
        let cmd = "go build ./... && go test ./internal/ui; echo done";
        let mut s = session(cmd);
        let id = s.step_by_call("b1").unwrap();
        s.watch_shells(
            &[Shell {
                command: cmd.into(),
                start: 1_000,
                kids: vec![proc_(12, "go test ./internal/ui", 3_000)],
                ..Shell::default()
            }],
            4_000,
        );
        assert_eq!(s.running_label(id), "2/3 go test");
        assert_eq!(s.part_now(id), Some(1));
        let marks = s.part_marks(id, 3, 9_000, false);
        assert_eq!(
            marks[1],
            PartMark::Running {
                since: 3_000,
                runs: 1
            }
        );
        assert_eq!(s.part_marks(id, 3, 9_000, true)[1], PartMark::Paused);
    }

    #[test]
    fn bare_drops_wrappers_and_assignments() {
        assert_eq!(
            bare(&args("FOO=1 sudo -E timeout 5 /usr/bin/make test")),
            ["make", "test"]
        );
        assert!(bare(&args("A=1")).is_empty());
    }

    /// Claude Code's own wrapper round a Bash call, as `ps` shows a
    /// rush-hosted session's shell on macOS: the snapshot sourced, the
    /// session's env file, then the call's command eval'd, a quote in it
    /// written '"'"'.
    fn wrapped(cmd: &str) -> String {
        format!(
            "/bin/zsh -c source /Users/me/.claude/shell-snapshots/snapshot-zsh-1793565349825-mfe161.sh 2>/dev/null || true && export RUSH_SESSION='e57fb8f4'\nexport PATH='/Users/me/.config/rush/gate/bin'\":$PATH\"\n: && setopt NO_EXTENDED_GLOB NO_BARE_GLOB_QUAL 2>/dev/null || true && {{ \\builtin unalias -- 'which'; \\builtin unset -f -- 'which'; }} >/dev/null 2>&1 || true && eval '{}' < /dev/null && pwd -P >| /tmp/claude-7e3a-cwd",
            cmd.replace('\'', r#"'"'"'"#)
        )
    }

    /// The shell Claude Code runs cmd in, as rush reads it: an eval
    /// wrapper's -c command isn't the call's, so it's left empty.
    fn eval_shell(cmd: &str, start: Ms, kids: Vec<ShellProc>) -> Shell {
        Shell {
            cmd: wrapped(cmd),
            command: String::new(),
            start,
            kids,
        }
    }

    /// The command the wrapper evals, unquoted, whatever quotes it holds:
    /// one written '"'"' (Claude Code's) or '\''.
    #[test]
    fn eval_arg_unquotes_the_wrapper() {
        for cmd in [
            "cargo test",
            "rg -n 'fn main' src && echo it's done",
            "echo one\ncat <<'EOF'\nit's here\nEOF\nwc -l x",
            "",
        ] {
            assert_eq!(eval_arg(&wrapped(cmd)).as_deref(), Some(cmd), "{cmd:?}");
        }
        assert_eq!(
            eval_arg(r"bash -c eval 'it'\''s' && pwd").as_deref(),
            Some("it's")
        );
        assert_eq!(eval_arg("/bin/zsh -c ls -la"), None);
    }

    /// A command with a quote in it is found in its own shell, not in an
    /// eval shell of a task started a little earlier (a dev server in
    /// the background), which matching by start alone would pick.
    #[test]
    fn a_quoted_command_finds_its_own_shell() {
        let cmd = "rg -n 'fn main' src && cargo test --workspace";
        let mut s = session(cmd);
        let other = eval_shell("npm run dev", 1_100, vec![proc_(7, "node vite", 1_100)]);
        let mine = eval_shell(
            cmd,
            1_300,
            vec![proc_(12, "cargo test --workspace", 1_500)],
        );
        s.watch_shells(&[other, mine], 2_000);
        let rp = s.running_part("b1").expect("found in its own shell");
        assert_eq!(
            (rp.command.as_str(), rp.at, rp.of),
            ("cargo test --workspace", 2, 2)
        );
    }

    /// A long call rush's gate ran as a script (rush-script <state>/
    /// scripts/<call id>.sh) is found in the shell that runs it, and its
    /// commands under the script runner.
    #[test]
    fn a_call_rush_ran_as_a_script_finds_its_shell() {
        let cmd = "cargo build\ncargo test\ncargo clippy\necho done";
        let mut s = session(cmd);
        let id = s.step_by_call("b1").unwrap();
        let run = "/opt/homebrew/bin/bash --noprofile --norc -c '__rush_s=$1' rush-script '/state/scripts/b1.sh'";
        let runner = ShellProc {
            pid: 20,
            args: args(
                "/opt/homebrew/bin/bash --noprofile --norc -c __rush_s=$1 rush-script /state/scripts/b1.sh",
            ),
            start: 1_300,
            kids: vec![proc_(21, "cargo test", 1_600)],
        };
        let other = eval_shell("npm run dev", 1_100, Vec::new());
        let mine = eval_shell(run, 1_300, vec![runner]);
        s.watch_shells(&[other, mine], 2_000);
        assert_eq!(s.running_label(id), "2/4 cargo test");
    }

    /// A loop's command, written after its do, is found; its round counts
    /// up; the loop's head and its done have no place and no time.
    #[test]
    fn a_loops_command_is_found() {
        let cmd = "for p in rush-core rush-convo; do cargo test -p $p; done";
        let mut s = session(cmd);
        let id = s.step_by_call("b1").unwrap();
        let sh = |kids| eval_shell(cmd, 1_000, kids);
        s.watch_shells(&[sh(vec![proc_(30, "cargo test -p rush-core", 1_100)])], 2_000);
        assert_eq!(s.running_label(id), "cargo test");
        s.watch_shells(&[sh(vec![proc_(31, "cargo test -p rush-convo", 5_000)])], 6_000);
        assert_eq!(s.running_label(id), "cargo test · run 2");
        let rp = s.running_part("b1").expect("running");
        assert_eq!(
            (rp.command.as_str(), rp.at, rp.of),
            ("cargo test -p $p", 1, 1)
        );
        assert_eq!(
            s.part_marks(id, 3, 9_000, false),
            [
                PartMark::Unknown,
                PartMark::Running {
                    since: 5_000,
                    runs: 2
                },
                PartMark::Unknown,
            ]
        );
    }

    /// A comment with a quote in it ("# let's") opens no quote: the
    /// commands after it are still the chain's, and the comment isn't
    /// counted among them.
    #[test]
    fn a_comment_with_a_quote_splits_nothing() {
        let cmd = "# let's build it first; then test\ncargo build --release\ncargo test";
        let mut s = session(cmd);
        let id = s.step_by_call("b1").unwrap();
        s.watch_shells(
            &[eval_shell(cmd, 1_000, vec![proc_(5, "cargo test", 1_500)])],
            2_000,
        );
        assert_eq!(s.running_label(id), "2/2 cargo test");
        let marks = s.part_marks(id, 3, 3_000, false);
        assert_eq!(marks[0], PartMark::Unknown, "a comment takes no time");
    }

    /// A script file's commands aren't in the call: those seen run under
    /// it say which runs now and how long each took; k kills the one
    /// running now and the script goes on.
    #[test]
    fn a_script_files_commands() {
        let cmd = "bash scripts/check.sh";
        let mut s = session(cmd);
        let id = s.step_by_call("b1").unwrap();
        let script = |kids| ShellProc {
            pid: 40,
            args: args("/bin/bash scripts/check.sh"),
            start: 1_100,
            kids,
        };
        s.watch_shells(
            &[eval_shell(
                cmd,
                1_000,
                vec![script(vec![proc_(41, "cargo build", 1_200)])],
            )],
            2_000,
        );
        assert_eq!(s.running_label(id), "now cargo build");
        // A subshell of the script's own (a pipe) is looked through.
        let fork = ShellProc {
            pid: 43,
            args: args("/bin/bash scripts/check.sh"),
            start: 5_100,
            kids: vec![proc_(
                42,
                "/Users/me/.cargo/bin/cargo test --workspace",
                5_200,
            )],
        };
        s.watch_shells(
            &[eval_shell(cmd, 1_000, vec![script(vec![fork])])],
            6_000,
        );
        assert_eq!(s.running_label(id), "now cargo test");
        assert_eq!(
            s.script_runs(id, 9_000, false),
            [
                ScriptRun {
                    words: "cargo build".into(),
                    mark: PartMark::Ran {
                        took: 4_000,
                        runs: 1,
                        failed: false
                    },
                },
                ScriptRun {
                    words: "cargo test".into(),
                    mark: PartMark::Running {
                        since: 5_200,
                        runs: 1
                    },
                },
            ]
        );
        let rp = s.running_part("b1").expect("running");
        assert_eq!(
            (rp.command.as_str(), rp.then.as_str(), rp.of),
            ("cargo test", "script", 0)
        );
        assert_eq!(
            rp.procs,
            [PartProc {
                pid: 42,
                start: 5_200
            }]
        );
        // ./build.sh and source env.sh are scripts too; a plain command
        // or one fed its lines by a heredoc isn't.
        for (c, want) in [
            ("./build.sh --fast", true),
            ("scripts/check.sh", true),
            ("source env.sh", true),
            ("sh -c 'make'", false),
            ("cargo test", false),
            ("bash <<'EOF'\necho hi\nEOF", false),
        ] {
            assert_eq!(script_call(&segments(c)), want, "{c}");
        }
    }

    /// What each line of a chain runs, past the words that shape a script.
    #[test]
    fn what_a_line_runs() {
        for (line, want) in [
            ("cargo test", Some("cargo test")),
            ("do cargo test -p $p", Some("cargo test -p $p")),
            ("then make", Some("make")),
            ("if cargo build; then", Some("cargo build")),
            ("while ! curl -s x; do", Some("curl -s x")),
            ("(cd rust", Some("cd rust")),
            ("cargo test)", Some("cargo test")),
            ("cargo test \\\n  --workspace", Some("cargo test    --workspace")),
            ("cargo test # all of it", Some("cargo test")),
            ("# a comment", None),
            ("for p in a b", None),
            ("done", None),
            ("fi", None),
            ("}", None),
            ("do", None),
            ("main() {", None),
        ] {
            assert_eq!(run_text(line).as_deref(), want, "{line:?}");
        }
    }
}
