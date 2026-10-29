//! A remote session is Copilot's coding agent working on GitHub: given an
//! issue or a task, it works in a runner and opens a pull request. rush
//! lists them and reads their logs; it doesn't start or steer them.
//!
//! Ported from Go's internal/adapters/copilot/remote.go:1-346. The log is
//! read and handed on a slice at a time, as HistoryReader asks.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use rush_core::adapter::{Kind, Pr, Profile, Result, Session};
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::time::{Ms, parse_rfc3339};
use rush_core::tool::{self, Call, Output, Replace};
use rush_core::wire::RawJson;

use crate::{Inner, KIND, err, lock};

/// One of Copilot's agent sessions, as its API gives it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RemoteSession {
    pub id: String,
    pub name: String,
    pub state: String,
    pub repo_id: i64,
    pub resource_type: String,
    #[serde(rename = "resource_number")]
    pub resource_num: i64,
    pub resource_state: String,
    pub model: String,
    pub head_ref: String,
    pub created_at: String,
    #[serde(rename = "last_updated_at")]
    pub updated_at: String,
    pub completed_at: String,
}

/// The states a session ends in.
fn done(state: &str) -> bool {
    matches!(
        state,
        "completed" | "cancelled" | "failed" | "timed_out" | "error"
    )
}

/// How often the sessions are asked for again: the list is read on every
/// refresh of rush's.
const LIST_EVERY: Duration = Duration::from_secs(20);

/// The sessions as last listed, with the API they came from and their
/// repositories' names.
#[derive(Default)]
pub(crate) struct Listing {
    at: Option<Instant>,
    listing: bool,
    list: Arc<[RemoteSession]>,
    base: String,
    repos: HashMap<i64, String>,
}

impl Inner {
    /// Your latest remote sessions as last listed. It never waits on the
    /// network: rush lists agents on every refresh, the first before it
    /// draws anything, so a stale listing asks for a new one in the
    /// background and the next refresh has it.
    fn sessions(self: &Arc<Self>) -> (Arc<[RemoteSession]>, String, HashMap<i64, String>) {
        let mut l = lock(&self.listing);
        if !l.listing && l.at.is_none_or(|at| at.elapsed() >= LIST_EVERY) {
            l.listing = true;
            l.at = Some(Instant::now());
            let me = self.clone();
            std::thread::spawn(move || me.relist());
        }
        (l.list.clone(), l.base.clone(), l.repos.clone())
    }

    /// Asks GitHub for your remote sessions, and names their repositories,
    /// while the network is up.
    fn relist(&self) {
        let got = rush_net::netwatch::do_job("copilot sessions", || {
            let base = self.api_base()?;
            #[derive(Deserialize)]
            struct Page {
                #[serde(default)]
                sessions: Vec<RemoteSession>,
            }
            let page: Page = self.get(&format!(
                "{base}/agents/sessions?page_number=1&page_size=50&sort=last_updated_at%2Cdesc"
            ))?;
            let mut repos = HashMap::new();
            for s in &page.sessions {
                repos
                    .entry(s.repo_id)
                    .or_insert_with(|| self.repo_name(s.repo_id));
            }
            Ok((page.sessions, base, repos))
        });
        let mut l = lock(&self.listing);
        l.listing = false;
        if let Ok((list, base, repos)) = got {
            l.list = list.into();
            l.base = base;
            l.repos = repos;
        }
    }

    /// Your remote sessions that have ended, or are still working.
    pub(crate) fn remote(self: &Arc<Self>, p: &Profile, ended: bool) -> Vec<Session> {
        let (list, base, repos) = self.sessions();
        list.iter()
            .filter(|r| done(&r.state) == ended)
            .map(|r| r.session(p, &base, repos.get(&r.repo_id).map_or("", String::as_str)))
            .collect()
    }

    /// A remote session's log as the events a session sends, a slice at a
    /// time. The task it was given isn't in the log: its name stands for it.
    pub(crate) fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> Result<()> {
        if !s.remote && !s.transcript.starts_with("https://") {
            return Err(err(format!(
                "copilot: session {} has no log rush can read",
                s.id
            )));
        }
        let url = if s.transcript.is_empty() {
            format!("{}/agents/sessions/{}/logs", self.api_base()?, s.id)
        } else {
            s.transcript.clone()
        };
        let body = self.fetch(&url, Duration::from_secs(30))?;
        let task = if s.name.is_empty() {
            "a task on GitHub"
        } else {
            &s.name
        };
        let first = vec![Event::Message(Message {
            role: "user".into(),
            parts: vec![text(task)],
            ..Message::default()
        })];
        let mut carry = Some(first);
        let mut go = true;
        read_log(body, before, most, &mut |mut evs| {
            if let Some(mut f) = carry.take() {
                f.append(&mut evs);
                evs = f;
            }
            go = each(evs);
            go
        })?;
        if !go {
            return Ok(());
        }
        let mut last = carry.unwrap_or_default();
        if s.state == "done" || s.state == "stopped" {
            last.push(Event::TurnEnd(TurnEnd {
                reason: "done".into(),
                ..TurnEnd::default()
            }));
        }
        if !last.is_empty() {
            each(last);
        }
        Ok(())
    }
}

impl RemoteSession {
    /// A remote session as rush's own.
    pub fn session(&self, p: &Profile, base: &str, repo: &str) -> Session {
        let mut s = Session {
            kind: Kind::from(KIND),
            profile: p.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            model: self
                .model
                .strip_prefix("sweagent-capi:")
                .unwrap_or(&self.model)
                .to_string(),
            remote: true,
            repo: repo.to_string(),
            transcript: format!("{base}/agents/sessions/{}/logs", self.id),
            created_at: parse_rfc3339(&self.created_at),
            updated_at: parse_rfc3339(&self.updated_at),
            ..Session::default()
        };
        if !done(&self.state) {
            s.state = "working".into();
            s.detail = "working on GitHub".into();
        } else if self.state == "completed" {
            s.state = "done".into();
        } else {
            s.state = "stopped".into();
            s.detail = self.state.replace('_', " ");
        }
        if self.resource_type == "pull" && self.resource_num > 0 && !s.repo.is_empty() {
            s.prs = vec![Pr {
                url: format!("https://github.com/{}/pull/{}", s.repo, self.resource_num),
                number: self.resource_num,
                title: self.name.clone(),
                state: self.resource_state.to_uppercase(),
                ..Pr::default()
            }];
        }
        s
    }
}

fn text(t: &str) -> Part {
    Part {
        kind: PartKind::Text,
        text: t.to_string(),
        ..Part::default()
    }
}

/// One line of a session's log: a chat completion chunk, whose tool calls
/// come twice, as made and then with what they returned.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Chunk {
    id: String,
    created: Value,
    choices: Vec<Choice>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Choice {
    delta: Delta,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Delta {
    content: String,
    tool_calls: Vec<ToolCall>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ToolCall {
    id: String,
    function: Function,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Function {
    name: String,
    arguments: String,
}

/// Turns a session's log into events, up to before when it isn't zero,
/// handing them to each a slice at a time: each slice read from at most
/// most bytes of the log (a longer line is a slice of its own). each
/// returning false stops it.
pub fn read_log(
    r: impl Read,
    before: Ms,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> Result<()> {
    let mut out = Vec::new();
    let mut read = 0usize;
    let mut seen = HashSet::new();
    let mut br = BufReader::new(r);
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = br.read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        read += n;
        let l = line.strip_suffix(b"\n").unwrap_or(&line);
        let l = l.strip_suffix(b"\r").unwrap_or(l);
        if let Some(data) = l.strip_prefix(b"data: ")
            && let Ok(c) = serde_json::from_slice::<Chunk>(data)
        {
            let at = created(&c.created);
            if before != 0 && at != 0 && at >= before {
                break;
            }
            chunk_events(&c, &mut seen, &mut out);
        }
        if read >= most && !out.is_empty() {
            read = 0;
            if !each(std::mem::take(&mut out)) {
                return Ok(());
            }
        }
    }
    if !out.is_empty() {
        each(out);
    }
    Ok(())
}

fn chunk_events(c: &Chunk, seen: &mut HashSet<String>, out: &mut Vec<Event>) {
    for ch in &c.choices {
        let d = &ch.delta;
        if d.tool_calls.is_empty() {
            if !d.content.trim().is_empty() {
                out.push(Event::Message(Message {
                    role: "assistant".into(),
                    id: c.id.clone(),
                    parts: vec![text(&d.content)],
                    ..Message::default()
                }));
            }
            continue;
        }
        // A chunk with calls carries what they returned, when it carries
        // words at all: what the agent says comes on its own.
        for tc in &d.tool_calls {
            if seen.insert(tc.id.clone()) {
                let call = call_of(&tc.id, &tc.function.name, &tc.function.arguments);
                out.push(Event::Message(Message {
                    role: "assistant".into(),
                    id: c.id.clone(),
                    parts: vec![Part {
                        kind: PartKind::ToolCall,
                        call: Some(Box::new(call)),
                        ..Part::default()
                    }],
                    ..Message::default()
                }));
            }
            if !d.content.is_empty() {
                out.push(Event::Message(Message {
                    role: "user".into(),
                    parts: vec![Part {
                        kind: PartKind::ToolResult,
                        output: Some(Box::new(Output {
                            call_id: tc.id.clone(),
                            text: d.content.clone(),
                            ..Output::default()
                        })),
                        ..Part::default()
                    }],
                    ..Message::default()
                }));
            }
        }
    }
}

/// A chunk's time: seconds or milliseconds, as a number or a string; 0
/// when it has none.
fn created(raw: &Value) -> Ms {
    let n: i64 = match raw {
        Value::Number(n) => match n.as_i64() {
            Some(n) => n,
            None => return 0,
        },
        Value::String(s) => {
            let s = s.trim_start();
            let end = s
                .char_indices()
                .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
                .map_or(s.len(), |(i, _)| i);
            s[..end].parse().unwrap_or(0)
        }
        _ => return 0,
    };
    if n > 1_000_000_000_000 {
        n
    } else if n > 0 {
        n * 1000
    } else {
        0
    }
}

/// One of the coding agent's tool calls as rush's own.
pub fn call_of(id: &str, name: &str, arguments: &str) -> Call {
    let v: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let path = in_repo(&field("path"));
    let paths = in_repo(&field("paths"));
    let mut command = field("command");
    command = command.replace(&format!("cd {} && ", runner_dir(&command)), "");
    let mut c = Call {
        id: id.to_string(),
        name: name.to_string(),
        raw: RawJson::new(arguments).ok(),
        ..Call::default()
    };
    let x = &mut c.input;
    x.description = field("description");
    match name {
        "bash" => {
            c.kind = tool::Kind::Shell;
            x.command = command;
        }
        "view" => {
            c.kind = tool::Kind::Read;
            x.path = path;
        }
        "create" => {
            c.kind = tool::Kind::Write;
            x.path = path;
            x.content = field("file_text");
        }
        "edit" | "str_replace" | "str_replace_editor" => {
            c.kind = tool::Kind::Edit;
            x.path = path;
            x.edits = vec![Replace {
                old: field("old_str"),
                new: field("new_str"),
                ..Replace::default()
            }];
        }
        "grep" => {
            c.kind = tool::Kind::Search;
            x.pattern = field("pattern");
            x.path = paths;
        }
        "glob" => {
            c.kind = tool::Kind::Glob;
            x.pattern = field("pattern");
            x.path = paths;
        }
        "think" => c.kind = tool::Kind::Think,
        "search_code_subagent" => {
            c.kind = tool::Kind::Search;
            x.pattern = field("query");
        }
        "run_setup" | "run_custom_setup_step" => c.title = field("name"),
        _ => {
            if let Some((server, t)) = name.split_once("-server-") {
                c.kind = tool::Kind::Mcp;
                x.server = server.to_string();
                x.tool = t.to_string();
            }
        }
    }
    c
}

/// The checkout a runner works in: /home/runner/work/<repo>/<repo>/.
static RUNNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/home/runner/work/[^/]+/[^/]+/?").unwrap());

/// A command changing into the runner's checkout first.
static CD_RUNNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^cd (/home/runner/work/[^/ ]+/[^/ ]+) && ").unwrap());

/// A path in the runner's checkout, as a path in the repository.
fn in_repo(p: &str) -> String {
    let r = RUNNER.replace(p, "");
    if r.is_empty() {
        p.to_string()
    } else {
        r.into_owned()
    }
}

/// The checkout a command changes into first, if it does.
fn runner_dir(cmd: &str) -> String {
    CD_RUNNER
        .captures(cmd)
        .map_or_else(|| "\x00".to_string(), |m| m[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log as the coding agent writes it, trimmed.
    const LOG: &str = r#"data: {"id":"clone-repo","created":1776968719202,"object":"chat.completion.chunk","choices":[{"finish_reason":"tool_calls","index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"clone-repo","function":{"name":"run_setup","arguments":"{\"name\":\"Clone repository o/r\"}"}}]}}]}

data: {"id":"clone-repo","created":1776968724426,"object":"chat.completion.chunk","choices":[{"finish_reason":"tool_calls","index":0,"delta":{"content":"Cloned o/r","role":"assistant","tool_calls":[{"index":0,"id":"clone-repo","function":{"name":"run_setup","arguments":"{\"name\":\"Clone repository o/r\"}"}}]}}]}

data: {"id":"mcp","created":"1776968730","object":"chat.completion.chunk","choices":[{"finish_reason":"tool_calls","index":0,"delta":{"content":"MCP server started","role":"assistant","tool_calls":[{"index":0,"id":"mcp","function":{"name":"run_setup","arguments":"{\"name\":\"Start MCP\"}"}}]}}]}

data: {"id":"msg_1","created":1776968740000,"object":"chat.completion.chunk","choices":[{"finish_reason":"tool_calls","index":0,"delta":{"content":"Let me look.","role":"assistant"}}]}

data: {"id":"msg_1","created":1776968741000,"object":"chat.completion.chunk","choices":[{"finish_reason":"tool_calls","index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"t1","function":{"name":"bash","arguments":"{\"command\":\"cd /home/runner/work/r/r && go test ./...\",\"description\":\"Run tests\"}"}}]}}]}

data: {"id":"msg_1","created":1776968745000,"object":"chat.completion.chunk","choices":[{"finish_reason":"tool_calls","index":0,"delta":{"content":"ok","role":"assistant","tool_calls":[{"index":0,"id":"t1","function":{"name":"bash","arguments":"{}"}}]}}]}

data: {"id":"msg_2","created":1776968750000,"object":"chat.completion.chunk","choices":[{"finish_reason":"stop","index":0,"delta":{"content":"Done: tests pass.","role":"assistant"}}]}
"#;

    fn all(before: Ms, most: usize) -> (Vec<Event>, usize) {
        let mut out = Vec::new();
        let mut slices = 0;
        read_log(LOG.as_bytes(), before, most, &mut |evs| {
            slices += 1;
            out.extend(evs);
            true
        })
        .unwrap();
        (out, slices)
    }

    #[test]
    fn reads_log() {
        let (evs, slices) = all(0, 1 << 20);
        assert_eq!(slices, 1);
        let (mut calls, mut results, mut texts) = (vec![], vec![], vec![]);
        for ev in &evs {
            let Event::Message(m) = ev else {
                panic!("not a message: {ev:?}")
            };
            for p in &m.parts {
                match p.kind {
                    PartKind::ToolCall => calls.push(p.call.as_ref().unwrap().id.clone()),
                    PartKind::ToolResult => {
                        let o = p.output.as_ref().unwrap();
                        results.push(format!("{}={}", o.call_id, o.text));
                    }
                    PartKind::Text => texts.push(p.text.clone()),
                    _ => {}
                }
            }
        }
        assert_eq!(calls.join(","), "clone-repo,mcp,t1");
        assert_eq!(
            results.join(","),
            "clone-repo=Cloned o/r,mcp=MCP server started,t1=ok"
        );
        assert_eq!(texts.join("|"), "Let me look.|Done: tests pass.");
        // Up to a time: the run that finished at 1776968745 isn't in yet.
        let (early, _) = all(1_776_968_742_000, 1 << 20);
        assert!(
            !early.is_empty() && early.len() < evs.len(),
            "{}",
            early.len()
        );
        // A flood comes a slice at a time, and the slices make the whole.
        let (sliced, slices) = all(0, 1);
        assert_eq!(sliced, evs);
        assert!(slices > 3, "{slices} slices");
    }

    #[test]
    fn call_of_reads_its_tools() {
        let c = call_of(
            "t1",
            "bash",
            r#"{"command":"cd /home/runner/work/r/r && go test ./...","description":"Run tests"}"#,
        );
        assert_eq!(c.kind, tool::Kind::Shell);
        assert_eq!(c.input.command, "go test ./...");
        assert_eq!(c.input.description, "Run tests");
        let c = call_of(
            "t2",
            "edit",
            r#"{"path":"/home/runner/work/r/r/app/x.ts","old_str":"a","new_str":"b"}"#,
        );
        assert_eq!(c.kind, tool::Kind::Edit);
        assert_eq!(c.input.path, "app/x.ts");
        assert_eq!(c.input.edits[0].new, "b");
        let c = call_of("t3", "github-mcp-server-get_job_logs", "{}");
        assert_eq!(c.kind, tool::Kind::Mcp);
        assert_eq!(c.input.server, "github-mcp");
        assert_eq!(c.input.tool, "get_job_logs");
        let c = call_of("t4", "run_setup", r#"{"name":"Clone"}"#);
        assert_eq!(c.title, "Clone");
        assert_eq!(c.raw.unwrap().get(), r#"{"name":"Clone"}"#);
    }

    #[test]
    fn times() {
        for (raw, want) in [
            (serde_json::json!(1776968719202i64), 1776968719202),
            (serde_json::json!("1776968730"), 1776968730000),
            (serde_json::json!(1776968730), 1776968730000),
            (serde_json::json!(null), 0),
            (serde_json::json!("soon"), 0),
        ] {
            assert_eq!(created(&raw), want, "{raw}");
        }
    }

    #[test]
    fn sessions_as_rushs() {
        let r = RemoteSession {
            id: "s1".into(),
            name: "Fix the build".into(),
            state: "timed_out".into(),
            resource_type: "pull".into(),
            resource_num: 7,
            resource_state: "open".into(),
            model: "sweagent-capi:claude-sonnet-4".into(),
            created_at: "2026-10-01T00:00:00Z".into(),
            ..RemoteSession::default()
        };
        let s = r.session(&Profile::default(), "https://api.x", "o/r");
        assert_eq!(s.state, "stopped");
        assert_eq!(s.detail, "timed out");
        assert_eq!(s.model, "claude-sonnet-4");
        assert_eq!(s.transcript, "https://api.x/agents/sessions/s1/logs");
        assert_eq!(s.prs[0].url, "https://github.com/o/r/pull/7");
        assert_eq!(s.prs[0].state, "OPEN");
        assert!(s.created_at > 0);
        let working = RemoteSession {
            state: "in_progress".into(),
            ..r
        };
        let s = working.session(&Profile::default(), "b", "");
        assert_eq!(
            (s.state.as_str(), s.detail.as_str()),
            ("working", "working on GitHub")
        );
        assert!(s.prs.is_empty());
    }
}
