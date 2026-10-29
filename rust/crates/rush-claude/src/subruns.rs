//! Which of a session's subagent runs are still working, read from the
//! transcripts. Port of internal/adapters/claude/claude/subruns.go (and
//! adapters/claude/subagents.go, which hands it out as the adapter's
//! RunFollower::subagent_runs).
//!
//! A subagent run's transcript says little about whether it is still going:
//! one busy with a long command, or a long think, writes nothing for
//! minutes. Its parent's transcript says more. The Agent (or Task) call
//! that started it has no result while a run the turn waits on works; one
//! launched in the background has a result at once, and a task
//! notification when it finishes. SubagentRuns follows those.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rush_core::adapter::{RunState, SubagentRun, SubagentRuns as Runs, SubagentStats};
use rush_core::time::{Ms, now, parse_rfc3339};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::jobs::mtime;

/// How long a run the transcripts still call running may go without
/// writing before it's taken to have died with its process.
pub const RUN_STALE: Ms = 15 * 60_000;

/// How recently a run the transcripts say nothing about must have written
/// to count as running.
const RUN_QUIET: Ms = 90_000;

/// Follows a session's transcript, and its runs' own when one run started
/// another, reading only what each has gained since.
#[derive(Default)]
pub struct SubagentRuns {
    /// The session's own process is known to have exited: a run the
    /// transcripts left unfinished ended with it, at once, rather than
    /// after RUN_STALE. Left false when that can't be told.
    pub gone: bool,

    path: String,
    /// How far each transcript has been read.
    files: HashMap<String, u64>,
    /// Agent calls, by tool_use id.
    calls: HashMap<String, AgentCall>,
    /// Finished runs, by agent id and by tool_use id.
    ends: HashMap<String, RunEnd>,
    /// Runs a message was sent to, by agent id: when.
    woken: HashMap<String, u64>,
    /// Lines read, which orders what they say.
    seq: u64,
    /// Calls still without their result: only then are results looked at.
    unmet: usize,
    /// By meta file.
    metas: HashMap<PathBuf, RunMeta>,
    dir_mod: Ms,
    /// The meta files as of dir_mod.
    names: Vec<PathBuf>,
    listed: Ms,
    /// Quiet runs' transcripts last looked at.
    quiet_at: Ms,
    buf: Vec<u8>,
    /// Whether the runs were looked at since gone was set.
    gone_looked: bool,
    /// The runs stats last found still working.
    running: Vec<SubagentRun>,
}

#[derive(Clone, Default)]
struct AgentCall {
    /// When it was made.
    seq: u64,
    /// When its result came.
    res_seq: u64,
    result: bool,
    /// The result was the launch of a background run.
    is_async: bool,
    /// How a run the turn waited on ended.
    status: String,
    at: Ms,
}

#[derive(Clone, Default)]
struct RunEnd {
    status: String,
    at: Ms,
    seq: u64,
}

#[derive(Clone, Default)]
struct RunMeta {
    /// The meta file has been read (see list's lazy).
    read: bool,
    modified: Ms,
    size: u64,
    id: String,
    tool_use: String,
    depth: i64,
    agent_type: String,
    description: String,
    /// When the run's transcript was last written, as last looked; seen
    /// is whether it has been.
    run_mod: Ms,
    run_seen: bool,
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl SubagentRuns {
    pub fn new() -> SubagentRuns {
        SubagentRuns::default()
    }

    fn reset(&mut self, path: &str) {
        let gone = self.gone;
        *self = SubagentRuns {
            gone,
            path: path.to_string(),
            ..SubagentRuns::default()
        };
    }

    fn fresh_for(&mut self, path: &str) {
        if self.path != path || self.path.is_empty() {
            self.reset(path);
        }
    }

    /// Reads what the session's transcript at path, and its runs' when
    /// needed, have gained, and returns its runs.
    pub fn update(&mut self, path: &str) -> Vec<SubagentRun> {
        self.fresh_for(path);
        let (runs, _) = self.list(false);
        if runs.is_empty() {
            return Vec::new();
        }
        self.read_for(path, &runs);
        runs
    }

    fn read_for(&mut self, path: &str, runs: &[SubagentRun]) {
        self.read(path);
        // A run a run started has its call, and word of its end, in that
        // run's transcript: those are read only while such a run hasn't
        // ended, and only those that were going as it began.
        let kids: Vec<&SubagentRun> = runs
            .iter()
            .filter(|x| x.depth > 1 && self.run_state(&x.id, &x.tool_use_id).0 != RunState::Done)
            .collect();
        let dir = self.dir();
        let parents: Vec<String> = runs
            .iter()
            .filter(|x| {
                kids.iter()
                    .any(|k| k.depth == x.depth + 1 && may_have_started(x, k))
            })
            .map(|x| path_str(&dir.join(format!("agent-{}.jsonl", x.id))))
            .collect();
        for p in parents {
            self.read(&p);
        }
    }

    fn dir(&self) -> PathBuf {
        Path::new(self.path.strip_suffix(".jsonl").unwrap_or(&self.path)).join("subagents")
    }

    /// Finds the runs from their meta files, reading each only when it's
    /// new or changed, and the folder only when something was added to it
    /// (or every 10s). lazy leaves unread the meta files of runs quiet past
    /// RUN_STALE, which none of stats' counting needs: unread says some
    /// were.
    fn list(&mut self, lazy: bool) -> (Vec<SubagentRun>, bool) {
        let dir = self.dir();
        let Some(dmod) = mtime(&dir) else {
            self.names.clear();
            return (Vec::new(), false);
        };
        let t = now();
        // Runs work in their session's process: once it's gone, none starts
        // or writes, so each is looked at once more, then not until it's
        // back.
        let still = self.gone && self.gone_looked;
        self.gone_looked = self.gone;
        let fresh = dmod == self.dir_mod && (still || t - self.listed <= 10_000);
        if !fresh {
            let mut names = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    if n.starts_with("agent-") && n.ends_with(".meta.json") {
                        names.push(e.path());
                    }
                }
            }
            names.sort(); // as Go's Glob lists them
            self.names = names;
            self.dir_mod = dmod;
            self.listed = t;
        }
        // The folder changes with every file a run writes in it, several a
        // second: quiet runs are looked at again on a clock of their own.
        let mut quiet = t - self.quiet_at > 10_000;
        if quiet {
            self.quiet_at = t;
        }
        quiet = quiet || self.gone && !still;
        let mut unread = false;
        let mut out = Vec::with_capacity(self.names.len());
        for p in &self.names {
            let mut m = self.metas.get(p).cloned().unwrap_or_else(|| {
                let base = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let id = base
                    .strip_prefix("agent-")
                    .unwrap_or(&base)
                    .strip_suffix(".meta.json")
                    .unwrap_or(&base)
                    .to_string();
                RunMeta {
                    depth: 1,
                    id,
                    ..RunMeta::default()
                }
            });
            let run_path = dir.join(format!("agent-{}.jsonl", m.id));
            // A run quiet past RUN_STALE is looked at again only every 10s:
            // a session can have hundreds, and each stat on every reading
            // was most of what reading the fleet cost.
            if !still && (quiet || t - m.run_mod < RUN_STALE) || !m.run_seen {
                m.run_mod = mtime(&run_path).unwrap_or(0);
                m.run_seen = true;
            }
            if lazy && !m.read && m.run_mod != 0 && t - m.run_mod >= RUN_STALE {
                unread = true;
                out.push(SubagentRun {
                    id: m.id.clone(),
                    depth: 1,
                    path: path_str(&run_path),
                    mod_time: m.run_mod,
                    ..SubagentRun::default()
                });
                self.metas.insert(p.clone(), m);
                continue;
            }
            // A meta file is written once, as its run starts: one read
            // whole isn't looked at again.
            if !m.read || m.tool_use.is_empty() {
                let Ok(md) = std::fs::metadata(p) else {
                    continue;
                };
                let fmod = md.modified().map(crate::jobs::ms_of).unwrap_or(0);
                if !m.read || m.modified != fmod || m.size != md.len() {
                    m.modified = fmod;
                    m.size = md.len();
                    m.read = true;
                    m.tool_use.clear();
                    m.depth = 1;
                    m.agent_type.clear();
                    m.description.clear();
                    #[derive(Deserialize, Default)]
                    #[serde(default)]
                    struct V {
                        #[serde(rename = "toolUseId")]
                        tool_use: Option<String>,
                        #[serde(rename = "spawnDepth")]
                        depth: Option<i64>,
                        #[serde(rename = "agentType")]
                        agent_type: Option<String>,
                        description: Option<String>,
                    }
                    if let Ok(b) = std::fs::read(p)
                        && let Ok(v) = serde_json::from_slice::<V>(&b)
                    {
                        m.tool_use = v.tool_use.unwrap_or_default();
                        m.depth = v.depth.unwrap_or(0).max(1);
                        m.agent_type = v.agent_type.unwrap_or_default();
                        m.description = v.description.unwrap_or_default();
                    }
                }
            }
            out.push(SubagentRun {
                id: m.id.clone(),
                tool_use_id: m.tool_use.clone(),
                depth: m.depth,
                kind: m.agent_type.clone(),
                description: m.description.clone(),
                born: m.modified,
                path: path_str(&run_path),
                mod_time: m.run_mod,
            });
            self.metas.insert(p.clone(), m);
        }
        (out, unread)
    }

    /// Takes in the complete lines a transcript has gained.
    fn read(&mut self, path: &str) {
        let Ok(mut f) = std::fs::File::open(path) else {
            return;
        };
        let Ok(md) = f.metadata() else { return };
        let size = md.len();
        let mut off = self.files.get(path).copied().unwrap_or(0);
        if size < off {
            off = 0; // rewritten
        }
        if size == off || f.seek(SeekFrom::Start(off)).is_err() {
            return;
        }
        let mut br = BufReader::with_capacity(64 << 10, f);
        let mut buf = std::mem::take(&mut self.buf);
        loop {
            buf.clear();
            match br.read_until(b'\n', &mut buf) {
                Ok(n) if n > 0 && buf.last() == Some(&b'\n') => {}
                _ => break, // a line still being written is read whole next time
            }
            off += buf.len() as u64;
            self.line(&buf);
        }
        self.files.insert(path.to_string(), off);
        if buf.capacity() <= 1 << 20 {
            self.buf = buf;
        }
    }

    fn line(&mut self, b: &[u8]) {
        self.seq += 1;
        if contains(b, b"\"tool_use\"")
            && (contains(b, b"\"name\":\"Agent\"")
                || contains(b, b"\"name\":\"Task\"")
                || contains(b, b"\"name\":\"SendMessage\""))
        {
            for bl in line_blocks(b) {
                if bl.kind != "tool_use" {
                    continue;
                }
                if (bl.name == "Agent" || bl.name == "Task") && !self.calls.contains_key(&bl.id) {
                    self.calls.insert(
                        bl.id.clone(),
                        AgentCall {
                            seq: self.seq,
                            ..AgentCall::default()
                        },
                    );
                    self.unmet += 1;
                } else if bl.name == "SendMessage" && !bl.to.is_empty() {
                    // A message to a finished run wakes it, until it next
                    // ends.
                    self.woken.insert(bl.to.clone(), self.seq);
                }
            }
        }
        if self.unmet > 0 && contains(b, b"\"tool_result\"") {
            for bl in line_blocks(b) {
                if bl.kind != "tool_result" {
                    continue;
                }
                let Some(c) = self.calls.get_mut(&bl.tool_use_id) else {
                    continue;
                };
                if c.result {
                    continue;
                }
                c.result = true;
                c.at = line_time(b);
                c.res_seq = self.seq;
                self.unmet -= 1;
                c.is_async =
                    contains(b, b"\"async_launched\"") || contains(b, b"Async agent launched");
                c.status = if bl.interrupted {
                    "stopped"
                } else if bl.is_error {
                    "failed"
                } else {
                    "completed"
                }
                .into();
            }
        }
        let mut rest = b;
        while let Some(i) = find(rest, b"<task-notification>") {
            rest = &rest[i + b"<task-notification>".len()..];
            let seg = match find(rest, b"</task-notification>") {
                Some(j) => &rest[..j],
                None => rest,
            };
            let status = tag_texts(seg, "status");
            let Some(st) = status.first() else { continue };
            let mut end = RunEnd {
                status: st.clone(),
                at: line_time(b),
                seq: self.seq,
            };
            if end.status == "killed" {
                end.status = "stopped".into();
            }
            // One notice can tell of several runs ending (a session that
            // ended with them running); the same one comes again as it's
            // queued and delivered, which changes nothing.
            let mut ids = tag_texts(seg, "task-id");
            ids.extend(tag_texts(seg, "tool-use-id"));
            for id in ids {
                let woken = self.woken.get(&id).copied().unwrap_or(0);
                match self.ends.get(&id) {
                    Some(e) if e.status == end.status && woken <= e.seq => {}
                    _ => {
                        self.ends.insert(id, end.clone());
                    }
                }
            }
        }
    }

    fn run_state(&self, id: &str, tool_use_id: &str) -> (RunState, String, Ms) {
        let (mut run, mut done): (i64, i64) = (-1, -1);
        let mut end = RunEnd::default();
        if !tool_use_id.is_empty()
            && let Some(c) = self.calls.get(tool_use_id)
        {
            if !c.result {
                run = c.seq as i64;
            } else if c.is_async {
                run = c.res_seq as i64;
            } else {
                done = c.res_seq as i64;
                end = RunEnd {
                    status: c.status.clone(),
                    at: c.at,
                    seq: 0,
                };
            }
        }
        for k in [id, tool_use_id] {
            if k.is_empty() {
                continue;
            }
            if let Some(e) = self.ends.get(k)
                && e.seq as i64 > done
            {
                done = e.seq as i64;
                end = e.clone();
            }
        }
        if !id.is_empty()
            && let Some(&w) = self.woken.get(id)
            && w as i64 > run
        {
            run = w as i64;
        }
        if run < 0 && done < 0 {
            return (RunState::Unknown, String::new(), 0);
        }
        if run > done {
            return (RunState::Running, String::new(), 0);
        }
        (RunState::Done, end.status, end.at)
    }

    fn run_going(&self, id: &str, tool_use_id: &str, modt: Ms, now: Ms) -> (bool, String) {
        let (st, status, at) = self.run_state(id, tool_use_id);
        let recent = modt != 0 && now - modt < RUN_QUIET;
        match st {
            RunState::Running => {
                if self.gone {
                    return (false, "ended".into());
                }
                (modt == 0 || now - modt < RUN_STALE, String::new())
            }
            RunState::Done => {
                if recent && modt > at + 5_000 {
                    return (true, String::new());
                }
                (false, status)
            }
            RunState::Unknown => (recent, String::new()),
        }
    }

    fn run_stats(&mut self, path: &str, now: Ms) -> SubagentStats {
        self.fresh_for(path);
        let (mut runs, unread) = self.list(true);
        let mut st = SubagentStats {
            spawned: runs.len() as u32,
            ..SubagentStats::default()
        };
        // Only a run written in the last RUN_STALE can be working: when none
        // has been, what the transcripts say needn't be read, nor its runs'
        // meta files.
        if !runs
            .iter()
            .any(|x| x.mod_time == 0 || now - x.mod_time < RUN_STALE)
        {
            self.running.clear();
            return st;
        }
        if unread {
            runs = self.list(false).0; // one works: every run's depth and call
        }
        self.read_for(path, &runs);
        self.running.clear();
        for x in runs {
            if !self.run_going(&x.id, &x.tool_use_id, x.mod_time, now).0 {
                continue;
            }
            if x.depth <= 1 {
                st.direct += 1;
            } else {
                st.nested += 1;
            }
            self.running.push(x);
        }
        st
    }

    fn copy(&self) -> SubagentRuns {
        SubagentRuns {
            gone: self.gone,
            path: self.path.clone(),
            seq: self.seq,
            calls: self.calls.clone(),
            ends: self.ends.clone(),
            woken: self.woken.clone(),
            ..SubagentRuns::default()
        }
    }
}

impl Runs for SubagentRuns {
    fn set_gone(&mut self, gone: bool) {
        self.gone = gone;
    }
    fn gone(&self) -> bool {
        self.gone
    }
    fn update_runs(&mut self, path: &str, runs: &[SubagentRun]) {
        self.fresh_for(path);
        if !runs.is_empty() {
            self.read_for(path, runs);
        }
    }
    fn took(&mut self, path: &str, line: &[u8]) {
        self.fresh_for(path);
        *self.files.entry(path.to_string()).or_insert(0) += line.len() as u64;
        self.line(line);
    }
    fn stats(&mut self, path: &str, now: Ms) -> SubagentStats {
        self.run_stats(path, now)
    }
    fn running(&self) -> Vec<SubagentRun> {
        self.running.clone()
    }
    fn state(&self, id: &str, tool_use_id: &str) -> (RunState, String, Ms) {
        self.run_state(id, tool_use_id)
    }
    fn going(&self, id: &str, tool_use_id: &str, mod_time: Ms, now: Ms) -> (bool, String) {
        self.run_going(id, tool_use_id, mod_time, now)
    }
    fn clone_runs(&self) -> Box<dyn Runs> {
        Box::new(self.copy())
    }
}

/// Whether run p could have started run k: it had begun, and was still
/// writing, as k began. Unknown times say it could.
pub fn may_have_started(p: &SubagentRun, k: &SubagentRun) -> bool {
    // By its meta, or its writing if that's earlier.
    let began = |x: &SubagentRun| {
        if x.mod_time != 0 && x.mod_time < x.born {
            x.mod_time
        } else {
            x.born
        }
    };
    let (kb, pb) = (began(k), began(p));
    kb == 0 || (pb == 0 || pb <= kb) && (p.mod_time == 0 || p.mod_time >= kb - 60_000)
}

/// One content block as SubagentRuns reads it: a SendMessage's input only
/// for whom it's to, a result's content only for whether it says the
/// request was interrupted.
#[derive(Default)]
struct Block {
    kind: String,
    id: String,
    name: String,
    tool_use_id: String,
    is_error: bool,
    to: String,
    interrupted: bool,
}

/// A transcript line's message's content blocks.
fn line_blocks(b: &[u8]) -> Vec<Block> {
    #[derive(Deserialize)]
    struct Line<'a> {
        #[serde(borrow, default)]
        message: Option<Msg<'a>>,
    }
    #[derive(Deserialize)]
    struct Msg<'a> {
        #[serde(borrow, default)]
        content: Option<&'a RawValue>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Raw<'a> {
        #[serde(rename = "type")]
        kind: Option<String>,
        id: Option<String>,
        name: Option<String>,
        tool_use_id: Option<String>,
        is_error: Option<bool>,
        #[serde(borrow)]
        input: Option<&'a RawValue>,
        #[serde(borrow)]
        content: Option<&'a RawValue>,
    }
    let Ok(l) = serde_json::from_slice::<Line>(b) else {
        return Vec::new();
    };
    let Some(c) = l.message.and_then(|m| m.content) else {
        return Vec::new();
    };
    if !c.get().starts_with('[') {
        return Vec::new();
    }
    let Ok(raw) = serde_json::from_str::<Vec<Raw>>(c.get()) else {
        return Vec::new();
    };
    raw.into_iter()
        .map(|r| {
            let mut to = String::new();
            if let Some(i) = r.input
                && i.get().contains("\"to\"")
            {
                #[derive(Deserialize)]
                struct In {
                    #[serde(default)]
                    to: Option<String>,
                }
                if let Ok(v) = serde_json::from_str::<In>(i.get()) {
                    to = v.to.unwrap_or_default();
                }
            }
            Block {
                kind: r.kind.unwrap_or_default(),
                id: r.id.unwrap_or_default(),
                name: r.name.unwrap_or_default(),
                tool_use_id: r.tool_use_id.unwrap_or_default(),
                is_error: r.is_error.unwrap_or(false),
                to,
                interrupted: r
                    .content
                    .is_some_and(|c| c.get().contains("[Request interrupted")),
            }
        })
        .collect()
}

/// A transcript line's time, or now.
fn line_time(b: &[u8]) -> Ms {
    const MARK: &[u8] = b"\"timestamp\":\"";
    if let Some(i) = find(b, MARK) {
        let s = &b[i + MARK.len()..];
        if let Some(j) = s.iter().position(|&c| c == b'"')
            && j > 0
            && let Ok(t) = std::str::from_utf8(&s[..j])
        {
            let ms = parse_rfc3339(t);
            if ms != 0 {
                return ms;
            }
        }
    }
    now()
}

/// What's inside each <tag>…</tag> in s.
fn tag_texts(mut s: &[u8], tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let end = format!("</{tag}>");
    let mut out = Vec::new();
    while let Some(i) = find(s, open.as_bytes()) {
        s = &s[i + open.len()..];
        let Some(j) = find(s, end.as_bytes()) else {
            return out;
        };
        let t = String::from_utf8_lossy(&s[..j]).trim().to_string();
        if !t.is_empty() {
            out.push(t);
        }
        s = &s[j + end.len()..];
    }
    out
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    memchr::memmem::find(h, n)
}

fn contains(h: &[u8], n: &[u8]) -> bool {
    find(h, n).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn set_mtime(p: &Path, at: Ms) {
        let f = fs::File::options().write(true).open(p).unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_millis(at as u64);
        f.set_times(fs::FileTimes::new().set_modified(t).set_accessed(t))
            .unwrap();
    }

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Dir {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "rush-subruns-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn going(r: &mut SubagentRuns, main: &str, id: &str) -> (bool, String) {
        for x in r.update(main) {
            if x.id == id {
                return r.run_going(&x.id, &x.tool_use_id, x.mod_time, now());
            }
        }
        panic!("no run {id}");
    }

    fn use_(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-27T20:00:00Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn async_(id: &str, agent: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-09-27T20:00:01Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":[{{"type":"text","text":"Async agent launched successfully.\nagentId: {agent}"}}]}}]}},"toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{agent}"}}}}"#
        )
    }

    fn append(p: &Path, lines: &[String]) {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap();
        f.write_all((lines.join("\n") + "\n").as_bytes()).unwrap();
    }

    // Lines as Claude Code writes them: a run the turn waits on, two
    // launched in the background (one of which starts a run of its own),
    // one notice telling of two ending, and a message that wakes one again.
    #[test]
    fn subagent_runs_from_the_transcripts() {
        let d = Dir::new();
        let main = d.0.join("s.jsonl");
        let subs = d.0.join("s").join("subagents");
        fs::create_dir_all(&subs).unwrap();
        let t = now();
        let run = |id: &str, call: &str, depth: i64, quiet: Ms, lines: &[String]| {
            let meta = format!(
                r#"{{"agentType":"general-purpose","toolUseId":"{call}","spawnDepth":{depth}}}"#
            );
            fs::write(subs.join(format!("agent-{id}.meta.json")), meta).unwrap();
            let p = subs.join(format!("agent-{id}.jsonl"));
            let mut all = vec![r#"{"type":"user","isSidechain":true}"#.to_string()];
            all.extend_from_slice(lines);
            fs::write(&p, all.join("\n") + "\n").unwrap();
            set_mtime(&p, t - quiet);
        };
        run("fg", "t1", 1, 10 * 60_000, &[]);
        run(
            "bg1",
            "t2",
            1,
            5 * 60_000,
            &[use_("t4", "Agent", r#"{"description":"deeper"}"#)],
        );
        run("bg2", "t3", 1, 5 * 60_000, &[]);
        run("deep", "t4", 2, 5 * 60_000, &[]);
        run("old", "t9", 1, 60 * 60_000, &[]);
        append(
            &main,
            &[
                r#"{"type":"user","message":{"role":"user","content":"go"}}"#.into(),
                use_("t1", "Agent", r#"{"description":"waited on"}"#),
                use_(
                    "t2",
                    "Agent",
                    r#"{"description":"one","run_in_background":true}"#,
                ),
                async_("t2", "bg1"),
                use_(
                    "t3",
                    "Task",
                    r#"{"description":"two","run_in_background":true}"#,
                ),
                async_("t3", "bg2"),
                use_("t9", "Agent", r#"{"description":"forgotten"}"#),
            ],
        );
        let m = path_str(&main);
        let mut r = SubagentRuns::new();
        for id in ["fg", "bg1", "bg2", "deep"] {
            assert!(
                going(&mut r, &m, id).0,
                "{id}: quiet with no word it ended, yet not running"
            );
        }
        assert!(
            !going(&mut r, &m, "old").0,
            "a run silent past RUN_STALE still runs"
        );
        let st = r.run_stats(&m, now());
        assert_eq!((st.spawned, st.direct, st.nested), (5, 3, 1), "{st:?}");

        // The deeper run answers the run that started it.
        let bg1 = subs.join("agent-bg1.jsonl");
        append(
            &bg1,
            &[r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t4","content":"found it"}]}}"#.into()],
        );
        set_mtime(&bg1, t - 5 * 60_000);
        assert_eq!(going(&mut r, &m, "deep"), (false, "completed".into()));

        // The one waited on answers; one notice tells of both background runs.
        append(
            &main,
            &[
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"done"}]}}"#.into(),
                r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-09-27T20:05:00Z","content":"<task-notification>\n<task-id>bg1</task-id>\n<task-id>bg2</task-id>\n<status>stopped</status>\n<summary>2 background agents didn't finish</summary>\n</task-notification>"}"#.into(),
            ],
        );
        for id in ["fg", "bg1", "bg2"] {
            let (g, how) = going(&mut r, &m, id);
            assert!(!g && !how.is_empty(), "{id}: after its end: {g} {how:?}");
        }
        // A message wakes one; it runs until its next notice.
        append(
            &main,
            &[use_(
                "t5",
                "SendMessage",
                r#"{"to":"bg2","message":"carry on"}"#,
            )],
        );
        assert!(going(&mut r, &m, "bg2").0, "a woken run isn't running");
        append(
            &main,
            &[r#"{"type":"user","timestamp":"2026-09-27T20:09:00Z","message":{"role":"user","content":"<task-notification>\n<task-id>bg2</task-id>\n<tool-use-id>t3</tool-use-id>\n<status>completed</status>\n</task-notification>"}}"#.into()],
        );
        assert_eq!(going(&mut r, &m, "bg2"), (false, "completed".into()));
    }

    // When the session's own process is known to have exited, a run the
    // transcripts left unfinished ended with it: at once.
    #[test]
    fn subagent_runs_end_with_their_process() {
        let d = Dir::new();
        let main = d.0.join("s.jsonl");
        let subs = d.0.join("s").join("subagents");
        fs::create_dir_all(&subs).unwrap();
        fs::write(subs.join("agent-a1.meta.json"), r#"{"toolUseId":"t1"}"#).unwrap();
        let p = subs.join("agent-a1.jsonl");
        fs::write(&p, "{}\n").unwrap();
        let quiet = now() - 2 * 60_000;
        set_mtime(&p, quiet);
        fs::write(
            &main,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Agent","input":{}}]}}"#.to_string() + "\n",
        )
        .unwrap();
        let m = path_str(&main);
        let mut r = SubagentRuns::new();
        assert_eq!(r.run_stats(&m, now()).direct, 1);
        r.gone = true;
        let st = r.run_stats(&m, now());
        assert_eq!((st.direct, st.spawned), (0, 1));
        assert_eq!(
            r.run_going("a1", "t1", quiet, now()),
            (false, "ended".into())
        );
    }

    // Runs all quiet past RUN_STALE can't be working, whatever the
    // transcript says: counted without reading it, or their meta files.
    #[test]
    fn subagent_stats_skips_quiet_sessions() {
        let d = Dir::new();
        let main = d.0.join("s.jsonl");
        let subs = d.0.join("s").join("subagents");
        fs::create_dir_all(&subs).unwrap();
        let meta = subs.join("agent-a.meta.json");
        fs::write(&meta, r#"{"toolUseId":"t1"}"#).unwrap();
        let run = subs.join("agent-a.jsonl");
        fs::write(&run, "{\"type\":\"user\",\"isSidechain\":true}\n").unwrap();
        set_mtime(&run, now() - RUN_STALE - 60_000);
        fs::write(
            &main,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"prompt":"go"}}]}}"#.to_string() + "\n",
        )
        .unwrap();
        let m = path_str(&main);
        let mut r = SubagentRuns::new();
        let st = r.run_stats(&m, now());
        assert_eq!((st.spawned, st.direct), (1, 0));
        assert_eq!(r.files.get(&m), None, "the transcript was read");
        assert!(!r.metas[&meta].read, "a quiet run's meta file was read");
        let t = now();
        set_mtime(&run, t);
        r.quiet_at = 0; // quiet runs' 10s are up
        assert_eq!(r.run_stats(&m, t).direct, 1);
        assert_eq!(r.metas[&meta].tool_use, "t1");
    }

    // Once its process is gone, a session's runs are looked at once more
    // and then left alone. Back, they're looked at again.
    #[test]
    fn gone_sessions_runs_are_looked_at_once() {
        let d = Dir::new();
        let main = d.0.join("s.jsonl");
        let subs = d.0.join("s").join("subagents");
        fs::create_dir_all(&subs).unwrap();
        let meta = subs.join("agent-a1.meta.json");
        fs::write(&meta, r#"{"toolUseId":"t1"}"#).unwrap();
        let p = subs.join("agent-a1.jsonl");
        fs::write(&p, "{}\n").unwrap();
        fs::write(&main, "{}\n").unwrap();
        let t = now() / 1000 * 1000;
        set_mtime(&p, t - 3_600_000);
        let m = path_str(&main);
        let mut r = SubagentRuns::new();
        r.run_stats(&m, t);
        let last = t - 60_000;
        set_mtime(&p, last);
        r.gone = true;
        r.run_stats(&m, t);
        assert_eq!(
            r.metas[&meta].run_mod, last,
            "gone: not looked at once more"
        );
        set_mtime(&p, t);
        r.quiet_at = 0;
        r.run_stats(&m, t);
        assert_eq!(r.metas[&meta].run_mod, last, "gone: looked at again");
        r.gone = false;
        r.run_stats(&m, t);
        assert_eq!(r.metas[&meta].run_mod, t, "back: not looked at");
    }

    // A nested run's call is looked for only in the runs going as it began.
    #[test]
    fn may_have_started_table() {
        let at = |m: i64| m * 60_000;
        let run = |born, m| SubagentRun {
            born,
            mod_time: m,
            ..SubagentRun::default()
        };
        let kid = run(at(10), at(20));
        for (i, (p, want)) in [
            (run(at(5), at(30)), true),   // going as it began
            (run(at(1), at(3)), false),   // done before
            (run(at(12), at(30)), false), // began after it
            (run(0, 0), true),            // times unknown
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(may_have_started(&p, &kid), want, "{i}");
        }
    }

    // Lines read elsewhere count as read.
    #[test]
    fn took_lines() {
        let d = Dir::new();
        let main = d.0.join("s.jsonl");
        let b = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{}}]}}"#.to_string() + "\n";
        fs::write(&main, &b).unwrap();
        let m = path_str(&main);
        let mut r = SubagentRuns::new();
        r.took(&m, b.as_bytes());
        r.update_runs(
            &m,
            &[SubagentRun {
                id: "a".into(),
                tool_use_id: "t1".into(),
                depth: 1,
                ..SubagentRun::default()
            }],
        );
        assert_eq!(r.run_state("a", "t1").0, RunState::Running);
        assert_eq!(r.files[&m], b.len() as u64);
        assert_eq!(r.seq, 1);
    }
}
