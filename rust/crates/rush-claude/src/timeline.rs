//! What a session and its subagents have done, in time order. Port of
//! internal/adapters/claude/claude/timeline.go and
//! adapters/claude/timeline.go.
//!
//! A Timeline is what you asked, tasks planned and ticked off, runs
//! started and ended, turns finished, questions asked, commits made, PRs
//! opened and turns that died on an error. With it, what each run last
//! said and is doing. It reads only the tail of a transcript the first
//! time (a day's work is rarely more), then only what each has gained.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use rush_core::adapter::{self, EventKind, Happening};
use rush_core::time::{Ms, parse_rfc3339};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::jobs::mtime;
use crate::json::{contains, nd};
use crate::subruns::{meta_files, meta_id, tag_texts};
use crate::tools;

/// How much of a transcript is read the first time.
const TIMELINE_TAIL: u64 = 4 << 20;

/// How many events a session keeps, the newest.
const TIMELINE_CAP: usize = 600;

/// A subagent as the timeline knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub tool_use_id: String,
    pub depth: i64,
    /// Its transcript last written.
    pub mod_time: Ms,
    /// The last thing it wrote.
    pub said: String,
    pub said_at: Ms,
    /// Its last tool call, in words.
    pub doing: String,
    pub doing_at: Ms,
    pub ended: bool,
    pub status: String,
    pub ended_at: Ms,
    pub ticked: i64,
    pub tasks: i64,
    /// A report just handed back: what it writes after ("I've sent the
    /// report") doesn't replace it.
    held: bool,
}

impl Run {
    /// As near as a run knows to when it began: its first words.
    fn started(&self) -> Ms {
        if self.said_at != 0 && (self.doing_at == 0 || self.said_at < self.doing_at) {
            self.said_at
        } else if self.doing_at != 0 {
            self.doing_at
        } else {
            self.mod_time
        }
    }
}

/// Follows one session's transcript and its subagents'.
#[derive(Default)]
pub struct Timeline {
    path: String,
    files: HashMap<PathBuf, TlFile>,
    runs: HashMap<String, Run>,
    /// Meta files read, by path: their mtime.
    metas: HashMap<PathBuf, Ms>,
    /// Agent calls' descriptions, by tool_use id.
    calls: HashMap<String, String>,
    notified: HashSet<String>,
    events: Vec<Happening>,
    started: bool,
}

#[derive(Default)]
struct TlFile {
    off: u64,
    run: String,
    /// Task subjects, by id.
    tasks: HashMap<String, String>,
    /// TaskCreate calls' subjects, by tool_use id.
    creates: HashMap<String, String>,
    /// TodoWrite items' statuses, by content.
    todos: HashMap<String, String>,
}

/// A copy of a timeline, safe to hold while it reads on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelineView {
    /// Oldest first.
    pub events: Vec<Happening>,
    /// Oldest first.
    pub runs: Vec<Run>,
}

impl Timeline {
    pub fn new() -> Timeline {
        Timeline::default()
    }

    /// Reads what the session's transcript at path, and its subagents',
    /// have gained. Subagents last written before since aren't read.
    pub fn update(&mut self, path: &str, since: Ms) {
        if self.path != path || !self.started {
            *self = Timeline {
                path: path.to_string(),
                started: true,
                ..Timeline::default()
            };
        }
        let dir = crate::transcript::subagents_dir(Path::new(path));
        for p in meta_files(&dir) {
            let id = meta_id(&p);
            let Some(run_mod) = mtime(&dir.join(format!("agent-{id}.jsonl"))) else {
                continue;
            };
            if run_mod < since {
                continue;
            }
            let r = self.runs.entry(id.clone()).or_insert_with(|| Run {
                id: id.clone(),
                depth: 1,
                ..Run::default()
            });
            if let Some(mm) = mtime(&p)
                && self.metas.get(&p) != Some(&mm)
            {
                self.metas.insert(p.clone(), mm);
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct M {
                    #[serde(rename = "agentType", deserialize_with = "nd")]
                    kind: String,
                    #[serde(deserialize_with = "nd")]
                    description: String,
                    #[serde(rename = "toolUseId", deserialize_with = "nd")]
                    tool_use: String,
                    #[serde(rename = "spawnDepth", deserialize_with = "nd")]
                    depth: i64,
                }
                if let Some(m) = std::fs::read(&p)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<M>(&b).ok())
                {
                    (r.name, r.kind, r.tool_use_id) = (m.description, m.kind, m.tool_use);
                    r.depth = m.depth.max(1);
                }
            }
            if r.name.is_empty() {
                r.name = self.calls.get(&r.tool_use_id).cloned().unwrap_or_default();
            }
            r.mod_time = run_mod;
        }
        // The runs are read before the session: word of their ends comes
        // in its transcript, and says less than their own last words.
        let mut ids: Vec<String> = self.runs.keys().cloned().collect();
        ids.sort();
        for id in ids {
            self.read(&dir.join(format!("agent-{id}.jsonl")), &id);
        }
        self.read(Path::new(path), "");
        for r in self.runs.values_mut() {
            if r.name.is_empty() {
                r.name = self.calls.get(&r.tool_use_id).cloned().unwrap_or_default();
            }
        }
    }

    /// Copies the timeline, dropping events before since.
    pub fn view(&self, since: Ms) -> TimelineView {
        let mut events: Vec<Happening> = self.events.iter().filter(|e| e.at >= since).cloned().collect();
        events.sort_by_key(|e| e.at);
        let mut runs: Vec<Run> = self.runs.values().cloned().collect();
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        runs.sort_by_key(Run::started);
        TimelineView { events, runs }
    }

    fn read(&mut self, path: &Path, run: &str) {
        let Ok(mut f) = File::open(path) else {
            return;
        };
        let Ok(st) = f.metadata() else {
            return;
        };
        let size = st.len();
        let mut skip = false; // the tail starts mid-line
        let mut tf = match self.files.remove(path) {
            Some(tf) if size >= tf.off => tf,
            _ => {
                let mut tf = TlFile {
                    run: run.to_string(),
                    ..TlFile::default()
                };
                if size > TIMELINE_TAIL {
                    tf.off = size - TIMELINE_TAIL;
                    skip = true;
                }
                tf
            }
        };
        if size == tf.off || f.seek(SeekFrom::Start(tf.off)).is_err() {
            self.files.insert(path.to_path_buf(), tf);
            return;
        }
        let mut br = BufReader::with_capacity(64 << 10, f);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match br.read_until(b'\n', &mut buf) {
                Ok(n) if n > 0 && buf.last() == Some(&b'\n') => {}
                _ => break,
            }
            tf.off += buf.len() as u64;
            if skip {
                skip = false;
                continue;
            }
            self.line(&mut tf, &buf);
        }
        self.files.insert(path.to_path_buf(), tf);
        let n = self.events.len();
        if n > TIMELINE_CAP {
            self.events.drain(..n - TIMELINE_CAP);
        }
    }

    fn add(&mut self, tf: &TlFile, at: Ms, mut e: Happening) -> usize {
        e.at = at;
        e.run = tf.run.clone();
        self.events.push(e);
        self.events.len() - 1
    }

    fn line(&mut self, tf: &mut TlFile, b: &[u8]) {
        let user = contains(b, br#""type":"user""#);
        let asst = contains(b, br#""type":"assistant""#);
        let compact = contains(b, br#""compact_boundary""#);
        if !user && !asst && !compact {
            return;
        }
        // A user line that is a tool's result matters only for these.
        if user
            && contains(b, br#""tool_result""#)
            && ![&br#""statusChange""#[..], br#""task":{"#, b"/pull/", b"<task-notification>", b"] "]
                .iter()
                .any(|m| contains(b, m))
        {
            return;
        }
        let Ok(l) = serde_json::from_slice::<TlLine>(b) else {
            return;
        };
        let at = parse_rfc3339(&l.timestamp);
        match l.typ.as_str() {
            "system" if l.subtype == "compact_boundary" => {
                self.add(tf, at, ev(EventKind::Compact));
            }
            "assistant" => self.assistant(tf, &l, at),
            "user" if !l.is_meta => self.user(tf, &l, at),
            _ => {}
        }
    }

    fn assistant(&mut self, tf: &mut TlFile, l: &TlLine, at: Ms) {
        let blocks: Vec<TlBlock> = l
            .message
            .content
            .as_ref()
            .and_then(|c| serde_json::from_str(c.get()).ok())
            .unwrap_or_default();
        let run = tf.run.clone();
        let mut text = String::new();
        for bl in &blocks {
            match bl.typ.as_str() {
                "text" => {
                    let s = bl.text.trim();
                    if !s.is_empty() {
                        text = s.to_string();
                    }
                }
                "tool_use" => {
                    self.tool_use(tf, bl, at);
                    if let Some(r) = self.runs.get_mut(&run)
                        && !bl.name.is_empty()
                    {
                        let input = bl.input.as_ref().map_or("", |v| v.get());
                        r.doing = tools::doing(&bl.name, input);
                        r.doing_at = at;
                        r.held = false;
                        if bl.name == "SubagentHandback" || bl.name == "SendMessage" {
                            let m: Said = from_raw(&bl.input);
                            if !m.message.is_empty() {
                                (r.said, r.said_at, r.held) = (said(&m.message), at, true);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if l.api_error {
            if !l.error.is_empty() {
                let t = text.strip_prefix("API Error: ").unwrap_or(&text).to_string();
                self.add(tf, at, Happening { text: t, ..ev(EventKind::Error) });
            }
            return;
        }
        if text.is_empty() {
            return;
        }
        let mut text = said(&text);
        if text == "No response requested." {
            return;
        }
        if let Some(r) = self.runs.get_mut(&run) {
            if r.held {
                text = r.said.clone();
            } else {
                (r.said, r.said_at) = (text.clone(), at);
            }
        }
        if l.message.stop_reason != "end_turn" {
            return;
        }
        let Some(r) = self.runs.get_mut(&run) else {
            self.add(tf, at, Happening { text, ..ev(EventKind::Turn) });
            return;
        };
        // A run's turn ending is the run ending, unless a message wakes it.
        if !r.ended || r.ended_at < at {
            (r.ended, r.status, r.ended_at) = (true, "completed".into(), at);
            self.add(
                tf,
                at,
                Happening {
                    status: "completed".into(),
                    text,
                    ..ev(EventKind::End)
                },
            );
        }
    }

    fn tool_use(&mut self, tf: &mut TlFile, bl: &TlBlock, at: Ms) {
        match bl.name.as_str() {
            "TaskCreate" => {
                if let Some(v) = parse_raw::<Subject>(&bl.input) {
                    tf.creates.insert(bl.id.clone(), v.subject);
                }
            }
            "TaskUpdate" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct U {
                    #[serde(rename = "taskId", deserialize_with = "nd")]
                    task_id: String,
                    #[serde(deserialize_with = "nd")]
                    status: String,
                }
                if let Some(u) = parse_raw::<U>(&bl.input)
                    && u.status == "completed"
                {
                    let mut s = tf.tasks.get(&u.task_id).cloned().unwrap_or_default();
                    if s.is_empty() {
                        s = format!("task #{}", u.task_id);
                    }
                    self.add(tf, at, Happening { text: s, ..ev(EventKind::Tick) });
                    if let Some(r) = self.runs.get_mut(&tf.run) {
                        r.ticked += 1;
                    }
                }
            }
            "TodoWrite" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Todo {
                    #[serde(alias = "Content", deserialize_with = "nd")]
                    content: String,
                    #[serde(alias = "Status", deserialize_with = "nd")]
                    status: String,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct T {
                    #[serde(deserialize_with = "nd")]
                    todos: Vec<Todo>,
                }
                let Some(t) = parse_raw::<T>(&bl.input) else {
                    return;
                };
                let fresh = tf.todos.is_empty();
                let mut next = HashMap::new();
                for td in &t.todos {
                    next.insert(td.content.clone(), td.status.clone());
                    if td.status == "completed"
                        && tf.todos.get(&td.content).map(String::as_str) != Some("completed")
                        && !fresh
                    {
                        self.add(tf, at, Happening { text: td.content.clone(), ..ev(EventKind::Tick) });
                    }
                }
                if fresh && let Some(first) = t.todos.first() {
                    self.add(
                        tf,
                        at,
                        Happening {
                            n: t.todos.len() as i64,
                            text: first.content.clone(),
                            ..ev(EventKind::Plan)
                        },
                    );
                }
                tf.todos = next;
            }
            "Agent" | "Task" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    #[serde(alias = "Description", deserialize_with = "nd")]
                    description: String,
                }
                if let Some(d) = parse_raw::<D>(&bl.input) {
                    self.calls.insert(bl.id.clone(), d.description.clone());
                    self.add(tf, at, Happening { text: d.description, ..ev(EventKind::Start) });
                }
            }
            "AskUserQuestion" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Q {
                    #[serde(alias = "Question", deserialize_with = "nd")]
                    question: String,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct A {
                    #[serde(deserialize_with = "nd")]
                    questions: Vec<Q>,
                }
                if let Some(a) = parse_raw::<A>(&bl.input)
                    && let Some(q) = a.questions.into_iter().next()
                {
                    self.add(tf, at, Happening { text: q.question, ..ev(EventKind::Ask) });
                }
            }
            _ => {}
        }
    }

    fn user(&mut self, tf: &mut TlFile, l: &TlLine, at: Ms) {
        let mut s = String::new();
        let mut blocks: Vec<TlBlock> = Vec::new();
        if let Some(c) = &l.message.content {
            match serde_json::from_str::<String>(c.get()) {
                Ok(v) => s = v,
                Err(_) => blocks = serde_json::from_str(c.get()).unwrap_or_default(),
            }
        }
        for bl in &blocks {
            match bl.typ.as_str() {
                "text" => s.push_str(&bl.text),
                "tool_result" => self.tool_result(tf, bl, l, at),
                _ => {}
            }
        }
        let s = s.trim();
        if s.is_empty() {
        } else if s.contains("<task-notification>") {
            self.notification(tf, s, at);
        } else if !tf.run.is_empty() || s.starts_with('<') || s.starts_with("[Request interrupted") {
            // a run's brief, or a command's echo, rather than you
        } else {
            self.add(tf, at, Happening { text: said(s), ..ev(EventKind::Prompt) });
        }
    }

    fn tool_result(&mut self, tf: &mut TlFile, bl: &TlBlock, l: &TlLine, at: Ms) {
        if let Some(mut subj) = tf.creates.remove(&bl.tool_use_id) {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Task {
                #[serde(alias = "ID", alias = "Id", deserialize_with = "nd")]
                id: String,
                #[serde(alias = "Subject", deserialize_with = "nd")]
                subject: String,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Res {
                #[serde(deserialize_with = "nd")]
                task: Task,
            }
            if let Some(res) = parse_raw::<Res>(&l.tool_use_result)
                && !res.task.id.is_empty()
            {
                if !res.task.subject.is_empty() {
                    subj = res.task.subject;
                }
                tf.tasks.insert(res.task.id, subj.clone());
            }
            if let Some(r) = self.runs.get_mut(&tf.run) {
                r.tasks += 1;
            }
            // Tasks are planned in a burst: one event says how many.
            if let Some(e) = self.events.last_mut()
                && e.kind == EventKind::Plan
                && e.run == tf.run
                && at - e.at < 5 * 60_000
            {
                e.n += 1;
                return;
            }
            self.add(tf, at, Happening { n: 1, text: subj, ..ev(EventKind::Plan) });
            return;
        }
        let text = match &bl.content {
            Some(c) => match serde_json::from_str::<String>(c.get()) {
                Ok(s) => s,
                Err(_) => serde_json::from_str::<Vec<TlBlock>>(c.get())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| p.text)
                    .collect(),
            },
            None => String::new(),
        };
        if let Some(m) = COMMIT_LINE.captures(&text) {
            let hash = &m[1][..7];
            self.add(tf, at, Happening { text: format!("{hash} {}", &m[2]), ..ev(EventKind::Commit) });
        }
        if let Some(u) = PR_LINE.find(&text)
            && text.len() < 400
        {
            self.add(tf, at, Happening { text: u.as_str().trim().to_string(), ..ev(EventKind::Pr) });
        }
        if text.contains("<task-notification>") {
            self.notification(tf, &text, at);
        }
    }

    /// Word that a run, or a background command, ended.
    fn notification(&mut self, tf: &TlFile, s: &str, at: Ms) {
        let mut rest = s;
        while let Some(i) = rest.find("<task-notification>") {
            rest = &rest[i + "<task-notification>".len()..];
            let seg = match rest.find("</task-notification>") {
                Some(j) => &rest[..j],
                None => rest,
            };
            let b = seg.as_bytes();
            let mut status = tag_texts(b, "status").into_iter().next().unwrap_or_default();
            match status.as_str() {
                "completed" | "failed" | "stopped" => {}
                "killed" => status = "stopped".into(),
                _ => continue,
            }
            let mut ids = tag_texts(b, "task-id");
            ids.extend(tag_texts(b, "tool-use-id"));
            let key = ids.join(",") + &status;
            if !self.notified.insert(key) {
                continue; // the same notice, queued then delivered
            }
            let sum = tag_texts(b, "summary").into_iter().next().unwrap_or_default();
            let res = tag_texts(b, "result").into_iter().next().unwrap_or_default();
            let run = self
                .runs
                .values()
                .find(|r| ids.iter().any(|id| !id.is_empty() && (*id == r.id || *id == r.tool_use_id)))
                .map(|r| r.id.clone());
            let m = NOTIFY_SUM.captures(&sum);
            let mut text = sum.clone();
            if let Some(m) = &m {
                text = m.get(3).map_or("", |x| x.as_str()).to_string(); // the name is the run's own
                if text.is_empty() && !res.is_empty() {
                    text = said(&res);
                }
            }
            let mut e = Happening {
                status: status.clone(),
                text,
                ..ev(EventKind::End)
            };
            if let Some(id) = run {
                let r = self.runs.get_mut(&id).expect("found above");
                if r.ended && r.status == status {
                    continue; // its own transcript said so already
                }
                (r.ended, r.status) = (true, status.clone());
                if status == "completed" && !r.said.is_empty() {
                    e.text = r.said.clone();
                }
                let n = self.add(tf, at, e);
                self.events[n].run = id.clone();
                let ended_at = self.events[n].at;
                if let Some(r) = self.runs.get_mut(&id) {
                    r.ended_at = ended_at;
                }
                continue;
            }
            if let Some(m) = &m {
                if e.text.is_empty() {
                    e.text = m[1].to_string();
                } else {
                    e.text = format!("{}: {}", &m[1], e.text);
                }
            }
            self.add(tf, at, e);
        }
    }
}

impl adapter::Timeline for Timeline {
    fn update(&mut self, path: &str, since: Ms) {
        Timeline::update(self, path, since);
    }
    fn events(&self, since: Ms) -> Vec<Happening> {
        self.view(since).events
    }
}

fn ev(kind: EventKind) -> Happening {
    Happening {
        kind,
        ..Happening::default()
    }
}

static COMMIT_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\[[\w./-]+(?: \(root-commit\))? ([0-9a-f]{7,40})\] (.+)$").unwrap()
});
static PR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^https://github\.com/[^\s/]+/[^\s/]+/pull/\d+\s*$").unwrap()
});
static NOTIFY_SUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^Agent "(.*)" (finished|completed|failed|was stopped|stopped)(?:: (.*))?$"#).unwrap()
});

#[derive(Deserialize, Default)]
#[serde(default)]
struct TlLine {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    subtype: String,
    #[serde(deserialize_with = "nd")]
    timestamp: String,
    #[serde(rename = "isMeta", deserialize_with = "nd")]
    is_meta: bool,
    #[serde(rename = "isApiErrorMessage", deserialize_with = "nd")]
    api_error: bool,
    #[serde(deserialize_with = "error_str")]
    error: String,
    #[serde(deserialize_with = "nd")]
    message: TlMessage,
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<Box<RawValue>>,
}

/// A line's error, which only needs to be there: a string, or anything
/// else said in its place.
fn error_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = Option::<Box<RawValue>>::deserialize(d)?;
    Ok(match v {
        None => String::new(),
        Some(v) => serde_json::from_str::<String>(v.get()).unwrap_or_else(|_| v.get().to_string()),
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TlMessage {
    content: Option<Box<RawValue>>,
    #[serde(deserialize_with = "nd")]
    stop_reason: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TlBlock {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    text: String,
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    name: String,
    input: Option<Box<RawValue>>,
    #[serde(deserialize_with = "nd")]
    tool_use_id: String,
    content: Option<Box<RawValue>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Subject {
    #[serde(alias = "Subject", deserialize_with = "nd")]
    subject: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Said {
    #[serde(alias = "Message", deserialize_with = "nd")]
    message: String,
}

fn parse_raw<T: for<'de> Deserialize<'de>>(v: &Option<Box<RawValue>>) -> Option<T> {
    serde_json::from_str(v.as_ref()?.get()).ok()
}

fn from_raw<T: for<'de> Deserialize<'de> + Default>(v: &Option<Box<RawValue>>) -> T {
    parse_raw(v).unwrap_or_default()
}

/// The gist of something written: its first paragraph, without
/// markdown's marks, and no longer than a line could show.
pub fn said(s: &str) -> String {
    let mut s = s.trim().to_string();
    if let Some((p, rest)) = s.split_once("\n\n") {
        let mut first = p.to_string();
        if p.ends_with(':') {
            // a lead-in says nothing without what it leads to
            let rest = rest.trim();
            let next = rest.split_once("\n\n").map_or(rest, |(n, _)| n);
            first = format!("{p} {next}");
        }
        s = first;
    }
    let s = s.replace("**", "").replace('`', "").replace('\n', " ");
    let s = s.trim_start_matches(['#', '>', ' ', '-']);
    let mut out: String = s.to_string();
    if s.chars().count() > 300 {
        out = s.chars().take(300).collect::<String>() + "…";
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::time::{format_rfc3339, now};
    use std::fs;

    // timeline_test.go (TestTimelineRunEndsWithItsReport): a run's end
    // reads as what it reported, once, however many times the notice of
    // it comes; a background command's end says what it was.
    #[test]
    fn timeline_run_ends_with_its_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let n = now();
        let at = |m: i64| format_rfc3339(n + (m - 60) * 60_000);
        let notice = r#"<task-notification><task-id>x1</task-id><tool-use-id>a1</tool-use-id><status>completed</status><summary>Agent \"Logo lane\" finished</summary><result>I've sent the report.</result></task-notification>"#;
        let lines = [
            format!(r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"tool_use","id":"a1","name":"Agent","input":{{"description":"Logo lane"}}}}]}}}}"#, at(1)),
            format!(r#"{{"type":"user","timestamp":"{}","message":{{"content":"{notice}"}}}}"#, at(5)),
            format!(r#"{{"type":"user","timestamp":"{}","message":{{"content":"{notice}"}}}}"#, at(6)),
            format!(r#"{{"type":"user","timestamp":"{}","message":{{"content":"<task-notification><task-id>b9</task-id><status>completed</status><summary>Background command \"pnpm test\" completed (exit code 0)</summary></task-notification>"}}}}"#, at(7)),
            format!(r#"{{"type":"assistant","timestamp":"{}","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"No response requested."}}]}}}}"#, at(8)),
        ];
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        let sub = dir.path().join("s/subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("agent-x1.meta.json"), r#"{"agentType":"lane","toolUseId":"a1"}"#).unwrap();
        let run = [
            format!(r###"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"tool_use","id":"h","name":"SubagentHandback","input":{{"message":"## The logo is fixed\n\nDetails."}}}}]}}}}"###, at(3)),
            format!(r#"{{"type":"assistant","timestamp":"{}","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"I've sent the report."}}]}}}}"#, at(4)),
        ];
        fs::write(sub.join("agent-x1.jsonl"), run.join("\n") + "\n").unwrap();

        let mut tl = Timeline::new();
        tl.update(&path.to_string_lossy(), n - 61 * 60_000);
        let v = tl.view(n - 120 * 60_000);
        let mut ends = Vec::new();
        for e in &v.events {
            if e.kind == EventKind::End {
                ends.push(format!("{}|{}", e.run, e.text));
            }
            assert_ne!(e.kind, EventKind::Turn, "a synthetic turn shows: {:?}", e.text);
        }
        assert_eq!(
            ends,
            vec![
                "x1|The logo is fixed".to_string(),
                r#"|Background command "pnpm test" completed (exit code 0)"#.into()
            ]
        );
        assert_eq!(v.runs.len(), 1);
        let r = &v.runs[0];
        assert_eq!((r.name.as_str(), r.ended, r.said.as_str()), ("Logo lane", true, "The logo is fixed"));
    }

    #[test]
    fn timeline_plans_ticks_commits_and_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let lines = [
            r#"{"type":"user","timestamp":"2026-10-01T10:00:00Z","message":{"content":"tidy the list"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:01Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"Read it"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"ok"}]},"toolUseResult":{"task":{"id":"1","subject":"Read it"}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:03Z","message":{"content":[{"type":"tool_use","id":"c2","name":"TaskCreate","input":{"subject":"Fix it"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"c2","content":"ok"}]},"toolUseResult":{"task":{"id":"2","subject":"Fix it"}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:05Z","message":{"content":[{"type":"tool_use","id":"u1","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:00:06Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","content":"[main 1234567abc] fix: the list\n 1 file changed"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:07Z","isApiErrorMessage":true,"error":"overloaded","message":{"content":[{"type":"text","text":"API Error: 529 overloaded"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:08Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"**Done.**\n\nMore."}]}}"#,
        ];
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        let mut tl = Timeline::new();
        tl.update(&path.to_string_lossy(), 0);
        let got: Vec<String> = tl
            .view(0)
            .events
            .iter()
            .map(|e| format!("{:?} {} {}", e.kind, e.n, e.text))
            .collect();
        assert_eq!(
            got,
            vec![
                "Prompt 0 tidy the list",
                "Plan 2 Read it",
                "Tick 0 Read it",
                "Commit 0 1234567 fix: the list",
                "Error 0 529 overloaded",
                "Turn 0 Done.",
            ]
        );
    }

    #[test]
    fn said_is_the_gist() {
        assert_eq!(said("## Title\n\nBody"), "Title");
        assert_eq!(said("Here's what:\n\n- one\n\n- two"), "Here's what: - one");
        assert_eq!(said(&"x".repeat(310)), "x".repeat(300) + "…");
    }
}
