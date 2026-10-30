//! A session's subagent runs, followed from its transcript and theirs.
//! Port of internal/adapters/claude/claude/subruns.go (workflowMod at
//! :617) and adapters/claude/subagents.go.
//!
//! A subagent run's transcript says little about whether it is still
//! going: one busy with a long command, or a long think, writes nothing for
//! minutes. Its parent's transcript says more. The Agent (or Task) call
//! that started it has no result while a run the turn waits on works; one
//! launched in the background has a result at once, and a task
//! notification when it finishes. SubagentRuns follows those.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use memchr::memmem;
use rush_core::adapter::{self, RunState, SubagentRun, SubagentStats};
use rush_core::time::{Ms, now, parse_rfc3339};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::jobs::{ms_of, mtime};

/// t in nanoseconds since the epoch, as Go's time.Time keeps it.
fn ns_of(t: std::time::SystemTime) -> i64 {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}
use crate::json::{Str, contains, nd};

/// How long a run the transcripts still call running may go without
/// writing before it's taken to have died with its process.
pub const RUN_STALE: Ms = 15 * 60_000;

/// How recently a run the transcripts say nothing about must have
/// written to count as running.
const RUN_QUIET: Ms = 90_000;

/// How often the folder, and quiet runs, are looked at again.
const LOOK_AGAIN: Ms = 10_000;

/// Follows a session's transcript, and its runs' own when one run started
/// another, reading only what each has gained since.
#[derive(Default)]
pub struct SubagentRuns {
    /// The session's own process is known to have exited: a run the
    /// transcripts left unfinished ended with it, at once, rather than
    /// after RUN_STALE. Left false when that can't be told.
    pub gone: bool,
    /// When the session's agent process last (re)started, its host with
    /// it or alone; 0 unknown. A run the transcripts call running, last
    /// written before it, was cut off by the restart: none goes on in a
    /// new process (a restart's 'revived' and 'host-started').
    pub since: Ms,

    path: String,
    /// How far each transcript has been read; None until reset.
    files: Option<HashMap<PathBuf, u64>>,
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
    dir_mod: Option<std::time::SystemTime>,
    /// The meta files as of dir_mod.
    names: Vec<PathBuf>,
    listed: Ms,
    /// Quiet runs' transcripts last looked at.
    quiet_at: Ms,
    buf: Vec<u8>,
    /// When a workflow still running last had an agent write, as of
    /// wf_at; wf_done the runs that have ended, by their folder's time
    /// then: not looked at again until a rerun adds an agent to it.
    wf_mod: Ms,
    wf_at: Ms,
    wf_done: HashMap<String, std::time::SystemTime>,
    /// Whether the runs were looked at since gone was set.
    gone_looked: bool,
    /// The runs stats last found still working, for running.
    running: Vec<SubagentRun>,
}

#[derive(Clone, Default)]
struct AgentCall {
    /// When it was made.
    seq: u64,
    /// When its result came.
    res_seq: u64,
    /// It has its result.
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
    modified: Option<std::time::SystemTime>,
    size: u64,
    id: String,
    tool_use: String,
    depth: i64,
    agent_type: String,
    description: String,
    /// When the run's transcript was last written, as last looked;
    /// run_seen is whether it has been.
    run_mod: Ms,
    run_seen: bool,
}

impl SubagentRuns {
    pub fn new() -> SubagentRuns {
        SubagentRuns::default()
    }

    fn reset(&mut self, path: &str) {
        *self = SubagentRuns {
            gone: self.gone,
            since: self.since,
            path: path.to_string(),
            files: Some(HashMap::new()),
            ..SubagentRuns::default()
        };
    }

    fn ready(&mut self, path: &str) {
        if self.files.is_none() || self.path != path {
            self.reset(path);
        }
    }

    /// Reads what the session's transcript at path, and its runs' when
    /// needed, have gained, and returns its runs.
    pub fn update(&mut self, path: &str) -> Vec<SubagentRun> {
        self.ready(path);
        let (runs, _) = self.list(false, now());
        if runs.is_empty() {
            return Vec::new();
        }
        self.read_for(path, &runs);
        runs
    }

    /// update for runs listed, from their meta files, by the caller: they
    /// aren't listed, nor their transcripts looked at, again.
    pub fn update_runs(&mut self, path: &str, runs: &[SubagentRun]) {
        self.ready(path);
        if !runs.is_empty() {
            self.read_for(path, runs);
        }
    }

    /// Reads line, the next of the session's transcript at path, as read
    /// by another from its start: update goes on from after it.
    pub fn took(&mut self, path: &str, line: &[u8]) {
        self.ready(path);
        if let Some(f) = &mut self.files {
            *f.entry(PathBuf::from(path)).or_default() += line.len() as u64;
        }
        self.line(line);
    }

    /// Reads what the session's transcript, and its runs' when needed,
    /// have gained.
    fn read_for(&mut self, path: &str, runs: &[SubagentRun]) {
        self.read(Path::new(path));
        // A run a run started has its call, and word of its end, in that
        // run's transcript: those are read only while such a run hasn't
        // ended, and only those that were going as it began.
        let kids: Vec<&SubagentRun> = runs
            .iter()
            .filter(|x| x.depth > 1 && self.state(&x.id, &x.tool_use_id).0 != RunState::Done)
            .collect();
        let dir = self.dir();
        let parents: Vec<PathBuf> = runs
            .iter()
            .filter(|x| {
                kids.iter()
                    .any(|k| k.depth == x.depth + 1 && may_have_started(x, k))
            })
            .map(|x| dir.join(format!("agent-{}.jsonl", x.id)))
            .collect();
        for p in parents {
            self.read(&p);
        }
    }

    /// A copy that answers state and going as self does now, for the UI
    /// to ask while self goes on reading elsewhere.
    pub fn clone_state(&self) -> SubagentRuns {
        SubagentRuns {
            gone: self.gone,
            since: self.since,
            path: self.path.clone(),
            seq: self.seq,
            calls: self.calls.clone(),
            ends: self.ends.clone(),
            woken: self.woken.clone(),
            ..SubagentRuns::default()
        }
    }

    fn dir(&self) -> PathBuf {
        crate::transcript::subagents_dir(Path::new(&self.path))
    }

    /// Finds the runs from their meta files, reading each only when it's
    /// new or changed, and the folder only when something was added to it
    /// (or every 10s). lazy leaves unread the meta files of runs quiet past
    /// RUN_STALE, which none of stats' counting needs: unread says some
    /// were.
    fn list(&mut self, lazy: bool, now: Ms) -> (Vec<SubagentRun>, bool) {
        let dir = self.dir();
        let Ok(st) = std::fs::metadata(&dir) else {
            self.names.clear();
            return (Vec::new(), false);
        };
        // Runs work in their session's process: once it's gone, none
        // starts or writes, so each is looked at once more, then not until
        // it's back.
        let still = self.gone && self.gone_looked;
        self.gone_looked = self.gone;
        let dir_mod = st.modified().ok();
        let fresh = dir_mod == self.dir_mod && (still || now - self.listed <= LOOK_AGAIN);
        if !fresh {
            self.names = meta_files(&dir);
            self.dir_mod = dir_mod;
            self.listed = now;
        }
        // The folder changes with every file a run writes in it, several a
        // second: quiet runs are looked at again on a clock of their own.
        let mut quiet = now - self.quiet_at > LOOK_AGAIN;
        if quiet {
            self.quiet_at = now;
        }
        quiet = quiet || self.gone && !still;
        let mut out = Vec::with_capacity(self.names.len());
        let mut unread = false;
        for p in self.names.clone() {
            let mut m = self.metas.get(&p).cloned().unwrap_or_else(|| RunMeta {
                depth: 1,
                id: meta_id(&p),
                ..RunMeta::default()
            });
            let run_path = dir.join(format!("agent-{}.jsonl", m.id));
            // A run quiet past RUN_STALE is looked at again only every
            // 10s: a session can have hundreds, and each stat on every
            // reading was most of what reading the fleet cost.
            if !still && (quiet || now - m.run_mod < RUN_STALE) || !m.run_seen {
                m.run_seen = true;
                m.run_mod = mtime(&run_path).unwrap_or(0);
            }
            if lazy && !m.read && m.run_mod != 0 && now - m.run_mod >= RUN_STALE {
                unread = true;
                out.push(SubagentRun {
                    id: m.id.clone(),
                    depth: 1,
                    path: run_path.to_string_lossy().into_owned(),
                    mod_time: m.run_mod,
                    ..SubagentRun::default()
                });
                self.metas.insert(p, m);
                continue;
            }
            // A meta file is written once, as its run starts: one read
            // whole isn't looked at again.
            if !m.read || m.tool_use.is_empty() {
                let Ok(fi) = std::fs::metadata(&p) else {
                    continue;
                };
                let fmod = fi.modified().ok();
                if !m.read || m.modified != fmod || m.size != fi.len() {
                    (m.modified, m.size, m.read) = (fmod, fi.len(), true);
                    (m.tool_use, m.depth) = (String::new(), 1);
                    (m.agent_type, m.description) = (String::new(), String::new());
                    if let Some(v) = std::fs::read(&p)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Meta>(&b).ok())
                    {
                        m.tool_use = v.tool_use;
                        m.depth = v.depth.max(1);
                        m.agent_type = v.agent_type;
                        m.description = v.description;
                    }
                }
            }
            out.push(SubagentRun {
                id: m.id.clone(),
                tool_use_id: m.tool_use.clone(),
                depth: m.depth,
                kind: m.agent_type.clone(),
                description: m.description.clone(),
                born: m.modified.map(ms_of).unwrap_or(0),
                born_ns: m.modified.map(ns_of).unwrap_or(0),
                path: run_path.to_string_lossy().into_owned(),
                mod_time: m.run_mod,
            });
            self.metas.insert(p, m);
        }
        (out, unread)
    }

    /// Takes in the complete lines a transcript has gained.
    fn read(&mut self, path: &Path) {
        let Ok(f) = File::open(path) else {
            return;
        };
        let Ok(st) = f.metadata() else {
            return;
        };
        let files = self.files.get_or_insert_default();
        let mut off = files.get(path).copied().unwrap_or(0);
        if st.len() < off {
            off = 0; // rewritten
        }
        if st.len() == off {
            return;
        }
        let mut f = f;
        if f.seek(SeekFrom::Start(off)).is_err() {
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
        if buf.capacity() <= 1 << 20 {
            self.buf = buf;
        }
        self.files
            .get_or_insert_default()
            .insert(path.to_path_buf(), off);
    }

    fn line(&mut self, b: &[u8]) {
        self.seq += 1;
        if contains(b, br#""tool_use""#)
            && (contains(b, br#""name":"Agent""#)
                || contains(b, br#""name":"Task""#)
                || contains(b, br#""name":"SendMessage""#))
        {
            for bl in line_blocks(b) {
                if &*bl.typ != "tool_use" {
                    continue;
                }
                match &*bl.name {
                    "Agent" | "Task" if !self.calls.contains_key(&*bl.id) => {
                        self.calls.insert(
                            bl.id.to_string(),
                            AgentCall {
                                seq: self.seq,
                                ..AgentCall::default()
                            },
                        );
                        self.unmet += 1;
                    }
                    // A message to a finished run wakes it, until it next
                    // ends.
                    "SendMessage" => {
                        let to = send_to(bl.input);
                        if !to.is_empty() {
                            self.woken.insert(to, self.seq);
                        }
                    }
                    _ => {}
                }
            }
        }
        if self.unmet > 0 && contains(b, br#""tool_result""#) {
            for bl in line_blocks(b) {
                let seq = self.seq;
                let Some(c) = self.calls.get_mut(&*bl.tool_use_id) else {
                    continue;
                };
                if &*bl.typ != "tool_result" || c.result {
                    continue;
                }
                (c.result, c.at, c.res_seq) = (true, line_time(b), seq);
                self.unmet -= 1;
                c.is_async =
                    contains(b, br#""async_launched""#) || contains(b, b"Async agent launched");
                c.status = if interrupted(bl.content) {
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
        while let Some(i) = memmem::find(rest, b"<task-notification>") {
            rest = &rest[i + "<task-notification>".len()..];
            let seg = match memmem::find(rest, b"</task-notification>") {
                Some(j) => &rest[..j],
                None => rest,
            };
            let status = tag_texts(seg, "status");
            let Some(status) = status.first() else {
                continue;
            };
            let end = RunEnd {
                status: if status == "killed" {
                    "stopped".into()
                } else {
                    status.clone()
                },
                at: line_time(b),
                seq: self.seq,
            };
            // One notice can tell of several runs ending (a session that
            // ended with them running); the same one comes again as it's
            // queued and delivered, which changes nothing.
            let mut ids = tag_texts(seg, "task-id");
            ids.extend(tag_texts(seg, "tool-use-id"));
            for id in ids {
                let woke = self.woken.get(&id).copied().unwrap_or(0);
                match self.ends.get(&id) {
                    Some(e) if e.status == end.status && woke <= e.seq => {}
                    _ => {
                        self.ends.insert(id, end.clone());
                    }
                }
            }
        }
    }

    /// How the run with this agent id, started by this call, stands: and
    /// when it has ended, how and when. Whatever the transcripts said last
    /// counts: a run launched, or woken by a message, runs until a notice
    /// says it ended.
    pub fn state(&self, id: &str, tool_use_id: &str) -> (RunState, String, Ms) {
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
            if !k.is_empty()
                && let Some(e) = self.ends.get(k)
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

    /// Whether a run last written at mod_time was cut off by its
    /// session's agent starting again since: its call left without a
    /// result by the restart, it never writes again.
    pub fn cut(&self, mod_time: Ms) -> bool {
        self.since != 0 && mod_time != 0 && mod_time < self.since
    }

    /// Whether a run is still working, last written at mod_time, and how
    /// it ended if it has. The transcripts' word counts: a run they call
    /// running is, unless its session's process is gone ("ended") or
    /// started again since it last wrote ("stopped": the restart cut it
    /// off), or, when that can't be told, it's been silent past RUN_STALE
    /// (it died with its process); one they call done is, unless it's
    /// written since (a message sent to it woke it). With no word, it's
    /// running while it writes.
    pub fn going(&self, id: &str, tool_use_id: &str, mod_time: Ms, now: Ms) -> (bool, String) {
        let (st, status, at) = self.state(id, tool_use_id);
        let recent = mod_time != 0 && now - mod_time < RUN_QUIET;
        match st {
            RunState::Running => {
                if self.gone {
                    return (false, "ended".into());
                }
                if self.cut(mod_time) {
                    return (false, "stopped".into());
                }
                (mod_time == 0 || now - mod_time < RUN_STALE, String::new())
            }
            RunState::Done => {
                if recent && mod_time > at + 5_000 {
                    return (true, String::new());
                }
                (false, status)
            }
            RunState::Unknown => (recent, String::new()),
        }
    }

    /// Counts the session's runs, and those still working, directly and at
    /// any depth.
    pub fn stats(&mut self, path: &str, now: Ms) -> SubagentStats {
        self.ready(path);
        let (mut runs, unread) = self.list(true, now);
        let mut st = SubagentStats {
            spawned: runs.len() as u32,
            workflow: self.workflow_mod(now),
            ..SubagentStats::default()
        };
        // Only a run written in the last RUN_STALE can be working: when
        // none has been, what the transcripts say needn't be read, nor its
        // runs' meta files. An old session's can be tens of megabytes, and
        // every one was read as rush started.
        if !runs
            .iter()
            .any(|x| x.mod_time == 0 || now - x.mod_time < RUN_STALE)
        {
            self.running.clear();
            return st;
        }
        if unread {
            // One works: every run's depth and call count.
            runs = self.list(false, now).0;
        }
        self.read_for(path, &runs);
        self.running.clear();
        for x in runs {
            if !self.going(&x.id, &x.tool_use_id, x.mod_time, now).0 {
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

    /// The runs stats last found still working: for drawing each as its
    /// own tile rather than behind the count alone.
    pub fn running(&self) -> &[SubagentRun] {
        &self.running
    }

    /// When an agent of one of the session's workflows still running last
    /// wrote. Their transcripts are a folder down, in workflows/<run>/, and
    /// none is a run of the session's own: without them, a session busy
    /// with a workflow looked silent. A run has ended once its
    /// workflows/<run>.json is written after the last agent joined its
    /// folder: the same script run again keeps its run id, so the record of
    /// the first run stays while the second one's agents write. Looked at
    /// every 10s at most.
    fn workflow_mod(&mut self, now: Ms) -> Ms {
        if now - self.wf_at < LOOK_AGAIN {
            return self.wf_mod;
        }
        self.wf_at = now;
        self.wf_mod = 0;
        let session = PathBuf::from(self.path.strip_suffix(".jsonl").unwrap_or(&self.path));
        let Ok(rd) = std::fs::read_dir(self.dir().join("workflows")) else {
            return 0;
        };
        for e in rd.flatten() {
            let run = e.file_name().to_string_lossy().into_owned();
            if !run.starts_with("wf_") {
                continue;
            }
            let d = e.path();
            let Some(dmod) = std::fs::metadata(&d).and_then(|m| m.modified()).ok() else {
                continue;
            };
            if self.wf_done.get(&run) == Some(&dmod) {
                continue;
            }
            let rec = session.join("workflows").join(format!("{run}.json"));
            if let Ok(rmod) = std::fs::metadata(&rec).and_then(|m| m.modified())
                && rmod >= dmod
            {
                self.wf_done.insert(run, dmod);
                continue;
            }
            // Its agents' transcripts and its journal.
            let Ok(files) = std::fs::read_dir(&d) else {
                continue;
            };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|x| x == "jsonl")
                    && let Some(m) = mtime(&p)
                    && m > self.wf_mod
                {
                    self.wf_mod = m;
                }
            }
        }
        self.wf_mod
    }
}

impl adapter::SubagentRuns for SubagentRuns {
    fn set_gone(&mut self, gone: bool) {
        self.gone = gone;
    }
    fn gone(&self) -> bool {
        self.gone
    }
    fn set_since(&mut self, since: Ms) {
        self.since = since;
    }
    fn cut(&self, mod_time: Ms) -> bool {
        SubagentRuns::cut(self, mod_time)
    }
    fn update_runs(&mut self, path: &str, runs: &[SubagentRun]) {
        SubagentRuns::update_runs(self, path, runs);
    }
    fn took(&mut self, path: &str, line: &[u8]) {
        SubagentRuns::took(self, path, line);
    }
    fn stats(&mut self, path: &str, now: Ms) -> SubagentStats {
        SubagentRuns::stats(self, path, now)
    }
    fn running(&self) -> Vec<SubagentRun> {
        self.running.clone()
    }
    fn state(&self, id: &str, tool_use_id: &str) -> (RunState, String, Ms) {
        SubagentRuns::state(self, id, tool_use_id)
    }
    fn going(&self, id: &str, tool_use_id: &str, mod_time: Ms, now: Ms) -> (bool, String) {
        SubagentRuns::going(self, id, tool_use_id, mod_time, now)
    }
    fn clone_runs(&self) -> Box<dyn adapter::SubagentRuns> {
        Box::new(self.clone_state())
    }
}

/// Whether run p could have started run k: it had begun, and was still
/// writing, as k began. Unknown times say it could.
fn may_have_started(p: &SubagentRun, k: &SubagentRun) -> bool {
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

#[derive(Deserialize, Default)]
#[serde(default)]
struct Meta {
    #[serde(rename = "toolUseId", deserialize_with = "nd")]
    tool_use: String,
    #[serde(rename = "spawnDepth", deserialize_with = "nd")]
    depth: i64,
    #[serde(rename = "agentType", deserialize_with = "nd")]
    agent_type: String,
    #[serde(deserialize_with = "nd")]
    description: String,
}

/// The agent-*.meta.json files in dir, by name.
pub(crate) fn meta_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("agent-") && n.ends_with(".meta.json"))
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// A meta file's run id: agent-<id>.meta.json.
pub(crate) fn meta_id(p: &Path) -> String {
    let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let n = n.strip_prefix("agent-").unwrap_or(n);
    n.strip_suffix(".meta.json").unwrap_or(n).to_string()
}

/// A content block, its input and content looked at where they lie:
/// they can be a whole prompt or a tool's whole output, and a run's lines
/// are only asked two things of them.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Block<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    id: Str<'a>,
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow)]
    tool_use_id: Str<'a>,
    #[serde(deserialize_with = "nd")]
    is_error: bool,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

/// A SendMessage call's input: who it's to.
fn send_to(input: Option<&RawValue>) -> String {
    let Some(v) = input else {
        return String::new();
    };
    if !v.get().contains(r#""to""#) {
        return String::new();
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct To {
        #[serde(deserialize_with = "nd")]
        to: String,
    }
    serde_json::from_str::<To>(v.get())
        .map(|t| t.to)
        .unwrap_or_default()
}

/// Whether a tool result's content says the request was interrupted.
fn interrupted(content: Option<&RawValue>) -> bool {
    content.is_some_and(|v| v.get().contains("[Request interrupted"))
}

/// A transcript line's message's content blocks; none for one written as
/// plain text.
fn line_blocks(b: &[u8]) -> Vec<Block<'_>> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Msg<'a> {
        #[serde(borrow)]
        content: Option<&'a RawValue>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Line<'a> {
        #[serde(borrow)]
        message: Option<Msg<'a>>,
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
    serde_json::from_str(c.get()).unwrap_or_default()
}

/// A transcript line's time, or now.
pub(crate) fn line_time(b: &[u8]) -> Ms {
    const MARK: &[u8] = br#""timestamp":""#;
    if let Some(i) = memmem::find(b, MARK) {
        let s = &b[i + MARK.len()..];
        if let Some(j) = memchr::memchr(b'"', s)
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
pub(crate) fn tag_texts(mut s: &[u8], tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let end = format!("</{tag}>");
    let mut out = Vec::new();
    loop {
        let Some(i) = memmem::find(s, open.as_bytes()) else {
            return out;
        };
        s = &s[i + open.len()..];
        let Some(j) = memmem::find(s, end.as_bytes()) else {
            return out;
        };
        let t = String::from_utf8_lossy(&s[..j]).trim().to_string();
        if !t.is_empty() {
            out.push(t);
        }
        s = &s[j + end.len()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn set_mtime(p: &Path, at: SystemTime) {
        File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    fn ms(t: SystemTime) -> Ms {
        ms_of(t)
    }

    fn append(p: &Path, lines: &[String]) {
        let mut f = File::options().create(true).append(true).open(p).unwrap();
        f.write_all((lines.join("\n") + "\n").as_bytes()).unwrap();
    }

    fn use_(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-27T20:00:00Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn launched(id: &str, agent: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-09-27T20:00:01Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":[{{"type":"text","text":"Async agent launched successfully.\nagentId: {agent}"}}]}}]}},"toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{agent}"}}}}"#
        )
    }

    // subruns_test.go:13 (TestSubagentRunsFromTheTranscripts): a run the
    // turn waits on, two launched in the background (one of which starts a
    // run of its own), one notice telling of two ending, and a message
    // that wakes one again.
    #[test]
    fn subagent_runs_from_the_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let subs = dir.path().join("s/subagents");
        fs::create_dir_all(&subs).unwrap();
        let now_t = SystemTime::now();
        let run = |id: &str, call: &str, depth: i64, quiet: Duration, lines: &[String]| {
            let meta = format!(
                r#"{{"agentType":"general-purpose","toolUseId":"{call}","spawnDepth":{depth}}}"#
            );
            fs::write(subs.join(format!("agent-{id}.meta.json")), meta).unwrap();
            let p = subs.join(format!("agent-{id}.jsonl"));
            let mut all = vec![r#"{"type":"user","isSidechain":true}"#.to_string()];
            all.extend_from_slice(lines);
            fs::write(&p, all.join("\n") + "\n").unwrap();
            set_mtime(&p, now_t - quiet);
        };
        let min = |m: u64| Duration::from_secs(60 * m);
        run("fg", "t1", 1, min(10), &[]);
        run(
            "bg1",
            "t2",
            1,
            min(5),
            &[use_("t4", "Agent", r#"{"description":"deeper"}"#)],
        );
        run("bg2", "t3", 1, min(5), &[]);
        run("deep", "t4", 2, min(5), &[]);
        run("old", "t9", 1, min(60), &[]);
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
                launched("t2", "bg1"),
                use_(
                    "t3",
                    "Task",
                    r#"{"description":"two","run_in_background":true}"#,
                ),
                launched("t3", "bg2"),
                use_("t9", "Agent", r#"{"description":"forgotten"}"#),
            ],
        );
        let ms_main = main.to_string_lossy().into_owned();
        let mut r = SubagentRuns::new();
        let going = |r: &mut SubagentRuns, id: &str| -> (bool, String) {
            let runs = r.update(&ms_main);
            let x = runs
                .iter()
                .find(|x| x.id == id)
                .unwrap_or_else(|| panic!("no run {id}"));
            r.going(&x.id, &x.tool_use_id, x.mod_time, now())
        };
        for id in ["fg", "bg1", "bg2", "deep"] {
            assert!(
                going(&mut r, id).0,
                "{id}: quiet for minutes with no word it ended, yet not running"
            );
        }
        assert!(
            !going(&mut r, "old").0,
            "a run silent past RUN_STALE still counts as running"
        );
        let st = r.stats(&ms_main, now());
        assert_eq!((st.spawned, st.direct, st.nested), (5, 3, 1), "{st:?}");

        // The deeper run answers the run that started it.
        let bg1 = subs.join("agent-bg1.jsonl");
        append(&bg1, &[r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t4","content":"found it"}]}}"#.into()]);
        set_mtime(&bg1, now_t - min(5));
        assert_eq!(going(&mut r, "deep"), (false, "completed".into()));

        // The one waited on answers; one notice tells of both background
        // runs.
        append(
            &main,
            &[
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"done"}]}}"#.into(),
                r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-09-27T20:05:00Z","content":"<task-notification>\n<task-id>bg1</task-id>\n<task-id>bg2</task-id>\n<status>stopped</status>\n<summary>2 background agents didn't finish</summary>\n</task-notification>"}"#.into(),
            ],
        );
        for id in ["fg", "bg1", "bg2"] {
            let (g, how) = going(&mut r, id);
            assert!(
                !g && !how.is_empty(),
                "{id}: after its end: going {g}, {how:?}"
            );
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
        assert!(going(&mut r, "bg2").0, "a woken run isn't running");
        append(&main, &[r#"{"type":"user","timestamp":"2026-09-27T20:09:00Z","message":{"role":"user","content":"<task-notification>\n<task-id>bg2</task-id>\n<tool-use-id>t3</tool-use-id>\n<status>completed</status>\n</task-notification>"}}"#.into()]);
        assert_eq!(going(&mut r, "bg2"), (false, "completed".into()));
    }

    // subruns_test.go:96 (TestSubagentRunsEndWithTheirProcess)
    #[test]
    fn subagent_runs_end_with_their_process() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let subs = dir.path().join("s/subagents");
        fs::create_dir_all(&subs).unwrap();
        fs::write(subs.join("agent-a1.meta.json"), r#"{"toolUseId":"t1"}"#).unwrap();
        let p = subs.join("agent-a1.jsonl");
        fs::write(&p, "{}\n").unwrap();
        let quiet = SystemTime::now() - Duration::from_secs(120);
        set_mtime(&p, quiet);
        fs::write(&main, r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Agent","input":{}}]}}"#.to_string() + "\n").unwrap();
        let m = main.to_string_lossy();
        let mut r = SubagentRuns::new();
        assert_eq!(r.stats(&m, now()).direct, 1, "can't tell");
        r.gone = true;
        let st = r.stats(&m, now());
        assert_eq!((st.direct, st.spawned), (0, 1), "process gone");
        assert_eq!(
            r.going("a1", "t1", ms(quiet), now()),
            (false, "ended".into())
        );
    }

    // A host restarted after a run's last write killed it: the call its
    // parent made has no result, and never will. It's stopped, not quiet,
    // however recently it wrote; one written since the restart runs.
    #[test]
    fn subagent_runs_end_with_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let subs = dir.path().join("s/subagents");
        fs::create_dir_all(&subs).unwrap();
        fs::write(subs.join("agent-a1.meta.json"), r#"{"toolUseId":"t1"}"#).unwrap();
        let p = subs.join("agent-a1.jsonl");
        fs::write(&p, "{}\n").unwrap();
        let last = SystemTime::now() - Duration::from_secs(60);
        set_mtime(&p, last);
        fs::write(&main, r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Agent","input":{}}]}}"#.to_string() + "\n").unwrap();
        let m = main.to_string_lossy();
        let mut r = SubagentRuns::new();
        assert_eq!(r.stats(&m, now()).direct, 1, "no restart known");
        r.since = ms(last) + 13_000;
        assert_eq!(r.stats(&m, now()).direct, 0, "restarted after it wrote");
        assert_eq!(
            r.going("a1", "t1", ms(last), now()),
            (false, "stopped".into())
        );
        assert!(r.clone_state().cut(ms(last)), "a copy forgets the restart");
        r.since = ms(last) - 60_000;
        assert_eq!(r.stats(&m, now()).direct, 1, "written since the restart");
    }

    /// A run's start is its meta file's time to the nanosecond, as Go's
    /// Born is (subruns.go:278), so two begun in one millisecond keep
    /// their order in the list.
    #[test]
    fn runs_begin_to_the_nanosecond() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let subs = dir.path().join("s/subagents");
        fs::create_dir_all(&subs).unwrap();
        let meta = subs.join("agent-a1.meta.json");
        fs::write(&meta, r#"{"toolUseId":"t1"}"#).unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::new(1_793_291_686, 720_721_196);
        set_mtime(&meta, at);
        let mut r = SubagentRuns::new();
        r.path = main.to_string_lossy().into_owned();
        let (runs, _) = r.list(false, now());
        assert_eq!(
            (runs[0].born, runs[0].born_ns),
            (1_793_291_686_720, 1_793_291_686_720_721_196)
        );
    }

    // subruns_test.go:122 (TestSubagentStatsSkipsQuietSessions)
    #[test]
    fn subagent_stats_skip_quiet_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let subs = dir.path().join("s/subagents");
        fs::create_dir_all(&subs).unwrap();
        let meta = subs.join("agent-a.meta.json");
        fs::write(&meta, r#"{"toolUseId":"t1"}"#).unwrap();
        let run = subs.join("agent-a.jsonl");
        fs::write(&run, "{\"type\":\"user\",\"isSidechain\":true}\n").unwrap();
        set_mtime(
            &run,
            SystemTime::now() - Duration::from_millis((RUN_STALE + 60_000) as u64),
        );
        // Its call has no result: it would be running, were it not so quiet.
        fs::write(&main, r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"prompt":"go"}}]}}"#.to_string() + "\n").unwrap();
        let m = main.to_string_lossy();
        let mut r = SubagentRuns::new();
        let st = r.stats(&m, now());
        assert_eq!((st.spawned, st.direct), (1, 0), "quiet");
        assert!(
            r.files.as_ref().unwrap().get(&main).is_none(),
            "the transcript was read"
        );
        assert!(!r.metas[&meta].read, "a quiet run's meta file was read");
        let n = SystemTime::now();
        set_mtime(&run, n);
        r.quiet_at = 0; // quiet runs' 10s are up
        assert_eq!(r.stats(&m, ms(n)).direct, 1, "written again");
        assert_eq!(
            r.metas[&meta].tool_use, "t1",
            "written again, its meta file is unread"
        );
    }

    // subruns_test.go:157 (TestGoneSessionsRunsAreLookedAtOnce)
    #[test]
    fn gone_sessions_runs_are_looked_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let subs = dir.path().join("s/subagents");
        fs::create_dir_all(&subs).unwrap();
        let meta = subs.join("agent-a1.meta.json");
        fs::write(&meta, r#"{"toolUseId":"t1"}"#).unwrap();
        let p = subs.join("agent-a1.jsonl");
        fs::write(&p, "{}\n").unwrap();
        fs::write(&main, "{}\n").unwrap();
        let n = SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
        set_mtime(&p, n - Duration::from_secs(3600));
        let m = main.to_string_lossy();
        let mut r = SubagentRuns::new();
        r.stats(&m, ms(n));
        let last = n - Duration::from_secs(60);
        set_mtime(&p, last); // its last words, as its process ended
        r.gone = true;
        r.stats(&m, ms(n));
        assert_eq!(
            r.metas[&meta].run_mod,
            ms(last),
            "gone: not looked at once more"
        );
        set_mtime(&p, n);
        r.quiet_at = 0;
        r.stats(&m, ms(n));
        assert_eq!(r.metas[&meta].run_mod, ms(last), "gone: looked at again");
        r.gone = false;
        r.stats(&m, ms(n));
        assert_eq!(r.metas[&meta].run_mod, ms(n), "back: not looked at");
    }

    // subruns_test.go:191 (TestMayHaveStarted)
    #[test]
    fn may_have_started_by_times() {
        let at = |m: i64| m * 60_000;
        let run = |born, m| SubagentRun {
            born,
            mod_time: m,
            ..SubagentRun::default()
        };
        let kid = run(at(10), at(20));
        for (i, (p, want)) in [
            (run(at(5), at(30)), true),     // going as it began
            (run(at(1), at(3)), false),     // done before
            (run(at(12), at(30)), false),   // began after it
            (SubagentRun::default(), true), // times unknown
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(may_have_started(&p, &kid), want, "{i}");
        }
    }

    // subruns_test.go:210 (TestTookLines)
    #[test]
    fn took_lines() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let b = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{}}]}}"#.to_string() + "\n";
        fs::write(&main, &b).unwrap();
        let m = main.to_string_lossy();
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
        assert_eq!(r.state("a", "t1").0, RunState::Running);
        assert_eq!(r.files.as_ref().unwrap()[&main], b.len() as u64);
        assert_eq!(r.seq, 1);
    }

    // subruns_test.go:234 (TestWorkflowAgentsWriting): a workflow's agents
    // work for the session though none is a run of its own: while the
    // workflow runs, their writing is when the session was last heard
    // from. Once it has written its workflows/<run>.json, it's over, until
    // the same script runs again under its run id and adds an agent.
    #[test]
    fn workflow_agents_writing() {
        // Whole seconds, as Go's test truncates.
        let secs_ago = |s: u64| {
            let t = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            SystemTime::UNIX_EPOCH + Duration::from_secs(t - s)
        };
        // A folder's time too: opened to read, as one can't be to write.
        let touch = |p: &Path, at: SystemTime| File::open(p).unwrap().set_modified(at).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let wf = dir.path().join("s/subagents/workflows/wf_1");
        fs::create_dir_all(&wf).unwrap();
        fs::write(&main, "{}\n").unwrap();
        fs::write(
            wf.join("agent-a.meta.json"),
            r#"{"agentType":"workflow-subagent"}"#,
        )
        .unwrap();
        let p = wf.join("agent-a.jsonl");
        fs::write(&p, "{}\n").unwrap();
        let at = secs_ago(60);
        touch(&p, at);
        let m = main.to_string_lossy();
        let mut r = SubagentRuns::new();
        let st = r.stats(&m, now());
        assert_eq!(
            (st.workflow, st.spawned),
            (ms_of(at), 0),
            "running: want its agent's write"
        );
        fs::create_dir_all(dir.path().join("s/workflows")).unwrap();
        let rec = dir.path().join("s/workflows/wf_1.json");
        fs::write(&rec, "{}").unwrap();
        let ended = secs_ago(30);
        touch(&wf, ended - Duration::from_secs(1));
        touch(&rec, ended);
        r.wf_at = 0; // its 10s are up
        assert_eq!(r.stats(&m, now()).workflow, 0, "ended");
        let again = wf.join("agent-b.jsonl");
        fs::write(&again, "{}\n").unwrap();
        let t = secs_ago(0);
        touch(&again, t);
        touch(&wf, t);
        r.wf_at = 0;
        assert_eq!(
            r.stats(&m, now()).workflow,
            ms_of(t),
            "run again: want its new agent's write"
        );
    }
}
