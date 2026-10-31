//! Background sessions as Claude Code records them under jobs/<short>/.
//! Port of internal/adapters/claude/claude/jobs.go:17-158.

use std::path::Path;

use rush_core::agent::{Job, State, Task, Todo};
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::account::{Account, path_str};
use crate::json::{Str, ms, nd, str_of};

/// One background job: the row rush shows, plus what only Claude Code's
/// record says.
#[derive(Clone, Debug, Default)]
pub struct ClaudeJob {
    pub job: Job,
    /// The state as the file has it: working, blocked, done, stopped, or
    /// running (a turn under way that only a busy session makes work).
    pub raw_state: String,
    pub name_source: String,
    pub tempo: String,
    pub children: u32,
    pub todos_done: u32,
    pub cli_version: String,
    pub respawn_flags: Vec<String>,
}

impl ClaudeJob {
    pub fn live(&self) -> bool {
        self.raw_state == "working" || self.raw_state == "blocked"
    }
}

/// A job file's state word as rush's State; "running" and anything
/// unknown read as Done and Stopped.
pub fn state_of(s: &str) -> State {
    State::from_word(s)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JobFile<'a> {
    #[serde(borrow)]
    state: Str<'a>,
    #[serde(borrow)]
    detail: Str<'a>,
    #[serde(borrow)]
    tempo: Str<'a>,
    #[serde(borrow)]
    needs: Option<&'a RawValue>,
    #[serde(borrow)]
    intent: Str<'a>,
    #[serde(rename = "displayIntent", borrow)]
    display_intent: Str<'a>,
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(rename = "nameSource", borrow)]
    name_source: Str<'a>,
    #[serde(borrow)]
    cwd: Str<'a>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Str<'a>,
    #[serde(rename = "linkScanPath", borrow)]
    link_scan_path: Str<'a>,
    #[serde(rename = "cliVersion", borrow)]
    cli_version: Str<'a>,
    #[serde(rename = "respawnFlags", deserialize_with = "nd")]
    respawn_flags: Vec<String>,
    #[serde(rename = "worktreePath", borrow)]
    worktree_path: Str<'a>,
    #[serde(rename = "worktreeBranch", borrow)]
    worktree_branch: Str<'a>,
    #[serde(borrow)]
    children: Option<&'a RawValue>,
    #[serde(rename = "inFlight")]
    in_flight: Option<InFlight>,
    #[serde(borrow, deserialize_with = "nd")]
    fan: Vec<Fan<'a>>,
    #[serde(rename = "createdAt", borrow)]
    created_at: Str<'a>,
    #[serde(rename = "updatedAt", borrow)]
    updated_at: Str<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct InFlight {
    #[serde(deserialize_with = "nd")]
    tasks: u32,
    #[serde(deserialize_with = "nd")]
    queued: u32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Fan<'a> {
    #[serde(borrow)]
    kind: Str<'a>,
    #[serde(borrow)]
    label: Str<'a>,
    #[serde(rename = "startedAt", deserialize_with = "nd")]
    started_at: i64,
    #[serde(rename = "doneAt", deserialize_with = "nd")]
    done_at: i64,
}

/// Reads job id's state.json; `mod_time` is the file's.
pub fn load_job(a: &Account, id: &str) -> Option<ClaudeJob> {
    let p = a.jobs_dir().join(id).join("state.json");
    let mod_time = mtime(&p)?;
    let b = std::fs::read(&p).ok()?;
    parse_job(a, id, &b, mod_time)
}

/// A job file's bytes as a job.
pub fn parse_job(a: &Account, id: &str, b: &[u8], mod_time: Ms) -> Option<ClaudeJob> {
    let f: JobFile = serde_json::from_slice(b).ok()?;
    let mut j = Job {
        id: id.to_string(),
        account: a.name.clone(),
        name: f.name.into_owned(),
        state: state_of(&f.state),
        detail: f.detail.into_owned(),
        intent: if f.display_intent.is_empty() {
            f.intent.into_owned()
        } else {
            f.display_intent.into_owned()
        },
        cwd: f.cwd.into_owned(),
        session_id: f.session_id.into_owned(),
        transcript_path: f.link_scan_path.into_owned(),
        worktree_path: f.worktree_path.into_owned(),
        worktree_branch: f.worktree_branch.into_owned(),
        created_at: ms(&f.created_at),
        updated_at: ms(&f.updated_at),
        mod_time,
        ..Job::default()
    };
    if let Some(n) = str_of(f.needs) {
        j.needs = n.into_owned();
    }
    let mut children = 0;
    if let Some(c) = f.children
        && c.get().starts_with('[')
        && let Ok(v) = serde_json::from_str::<Vec<&RawValue>>(c.get())
    {
        children = v.len() as u32;
    }
    if let Some(x) = f.in_flight {
        j.in_flight = x.tasks + x.queued;
    }
    let mut todos_done = 0;
    for x in &f.fan {
        if &*x.kind == "todo" {
            if x.done_at > 0 {
                todos_done += 1;
            }
            j.todos.push(Todo {
                label: x.label.to_string(),
                done: x.done_at > 0,
                started: x.started_at > 0,
            });
            continue;
        }
        if x.done_at == 0 && !x.label.is_empty() {
            j.running.push(Task {
                id: String::new(),
                kind: x.kind.to_string(),
                label: x.label.to_string(),
                started_at: x.started_at,
            });
            j.background.push(format!("{}\0{}", &*x.kind, &*x.label));
            if &*x.kind == "agent" {
                j.subagents += 1;
            }
        }
    }
    if j.name.is_empty() {
        j.name = id.to_string();
    }
    if j.transcript_path.is_empty() && !j.session_id.is_empty() {
        j.transcript_path = path_str(&a.transcript_path(&j.cwd, &j.session_id));
    }
    Some(ClaudeJob {
        job: j,
        raw_state: f.state.into_owned(),
        name_source: f.name_source.into_owned(),
        tempo: f.tempo.into_owned(),
        children,
        todos_done,
        cli_version: f.cli_version.into_owned(),
        respawn_flags: f.respawn_flags,
    })
}

/// The account's job ids: its eight-letter folders under jobs/, sorted.
pub fn list_job_ids(a: &Account) -> Vec<String> {
    let Ok(ents) = std::fs::read_dir(a.jobs_dir()) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = ents
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.len() == 8)
        .collect();
    ids.sort();
    ids
}

/// How Claude Code names a folder's project directory: every character
/// but a letter or digit becomes a dash.
pub fn project_slug(dir: &str) -> String {
    let real = real_dir(dir);
    let mut b = real.into_bytes();
    for c in b.iter_mut() {
        if !c.is_ascii_alphanumeric() {
            *c = b'-';
        }
    }
    // Every byte is ASCII now.
    String::from_utf8(b).unwrap_or_default()
}

/// dir as Claude Code files it, by its real path: on a Mac /tmp, /var and
/// /etc are links into /private. Only the system's own links, as it takes
/// no disk reads.
fn real_dir(dir: &str) -> String {
    if cfg!(target_os = "macos") {
        for p in ["/tmp", "/var", "/etc"] {
            if dir == p || (dir.starts_with(p) && dir.as_bytes().get(p.len()) == Some(&b'/')) {
                return format!("/private{dir}");
            }
        }
    }
    dir.to_string()
}

/// A file's modification time in Unix ms, or None when it isn't there.
pub(crate) fn mtime(p: &Path) -> Option<Ms> {
    let m = std::fs::metadata(p).ok()?;
    Some(ms_of(m.modified().ok()?))
}

pub(crate) fn ms_of(t: std::time::SystemTime) -> Ms {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as Ms,
        Err(e) => -(e.duration().as_millis() as Ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Claude Code files a session begun in /tmp under its real path.
    #[test]
    fn project_slug_real_dir() {
        if cfg!(target_os = "macos") {
            assert_eq!(project_slug("/tmp/spawnprobe"), "-private-tmp-spawnprobe");
        }
        assert_eq!(project_slug("/Users/me/x"), "-Users-me-x");
        assert_eq!(project_slug("/tmpx/a"), "-tmpx-a");
    }

    #[test]
    fn project_slug_cases() {
        assert_eq!(
            project_slug("/Users/lw/Source/github.com/langwatch/langwatch"),
            "-Users-lw-Source-github-com-langwatch-langwatch"
        );
        assert_eq!(
            project_slug("/private/var/folders/zg/3jfp8q9100s_wlx/T/a b"),
            "-private-var-folders-zg-3jfp8q9100s-wlx-T-a-b"
        );
    }

    #[test]
    fn parses_a_job_file() {
        let a = Account::at("default", "/c");
        let b = br#"{"state":"running","detail":"d","needs":"you","displayIntent":"shown","intent":"raw",
            "cwd":"/r","sessionId":"sid","children":[1,2],"inFlight":{"tasks":1,"queued":2},
            "fan":[{"kind":"todo","label":"t1","startedAt":1,"doneAt":2},{"kind":"todo","label":"t2"},
                   {"kind":"agent","label":"explore","startedAt":5},{"kind":"shell","label":"","startedAt":5}],
            "createdAt":"1970-01-01T00:00:01Z","updatedAt":null,"respawnFlags":["--model","opus"]}"#;
        let j = parse_job(&a, "abcd1234", b, 9).unwrap();
        assert_eq!(j.raw_state, "running");
        assert_eq!(j.job.state, State::Done);
        assert_eq!(j.job.name, "abcd1234");
        assert_eq!(j.job.intent, "shown");
        assert_eq!(j.job.needs, "you");
        assert_eq!(j.children, 2);
        assert_eq!(j.job.in_flight, 3);
        assert_eq!((j.job.todos.len(), j.todos_done), (2, 1));
        assert_eq!(j.job.background, vec!["agent\0explore".to_string()]);
        assert_eq!(j.job.subagents, 1);
        assert_eq!(j.job.created_at, 1000);
        assert_eq!(j.job.transcript_path, "/c/projects/-r/sid.jsonl");
        assert_eq!(j.respawn_flags, vec!["--model", "opus"]);
        // needs that isn't a string is ignored
        let j = parse_job(&a, "x", br#"{"needs":{"a":1}}"#, 0).unwrap();
        assert_eq!(j.job.needs, "");
    }
}
