//! Pi's sessions on disk: where they are, which ran lately, and what a
//! list needs of each. Port of internal/adapters/pi/discover.go:1-278.
//!
//! Pi keeps a session in a file of its own, <dir>/sessions/--<cwd>--/
//! <time>_<id>.jsonl, the cwd's slashes made dashes. The first line is the
//! session's header; every other is an entry in a tree, each naming its
//! parent, the last written the tip.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use rush_core::adapter::{Profile, Session};
use rush_core::time::{Ms, now, parse_rfc3339};
use serde::Deserialize;

use crate::json::nd;
use crate::messages::Message;

/// Pi's config folder: PI_CODING_AGENT_DIR, else ~/.pi/agent.
pub fn home() -> String {
    if let Some(d) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|d| !d.is_empty()) {
        return d.to_string_lossy().into_owned();
    }
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(h) => Path::new(&h)
            .join(".pi")
            .join("agent")
            .to_string_lossy()
            .into_owned(),
        None => String::new(),
    }
}

/// Where p's sessions are: PI_CODING_AGENT_SESSION_DIR for the user's own
/// Pi, else the profile's sessions folder.
pub(crate) fn sessions_dir(p: &Profile) -> PathBuf {
    if let Some(d) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR").filter(|d| !d.is_empty())
        && p.dir == home()
    {
        return PathBuf::from(d);
    }
    Path::new(&p.dir).join("sessions")
}

/// Every file in dir, sorted by name, as Go's Glob lists them.
fn sorted(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    out.sort();
    out
}

/// Every session file of p's (sessions/*/*.jsonl).
pub(crate) fn session_files(p: &Profile) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in sorted(&sessions_dir(p)) {
        out.extend(
            sorted(&d)
                .into_iter()
                .filter(|f| f.to_string_lossy().ends_with(".jsonl")),
        );
    }
    out
}

/// Where session id is kept in p (sessions/*/*_<id>.jsonl), or "".
pub(crate) fn session_file(p: &Profile, id: &str) -> String {
    let suffix = format!("_{id}.jsonl");
    session_files(p)
        .into_iter()
        .rfind(|f| {
            f.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(&suffix))
        })
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The session id in a session file's name, <time>_<id>.jsonl; "" for
/// what isn't one.
pub fn id_of_file(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some(stem) = name.strip_suffix(".jsonl") else {
        return String::new();
    };
    stem.split_once('_')
        .map(|(_, id)| id.to_string())
        .unwrap_or_default()
}

/// How recently a session must have been written to count as running;
/// WORKING_FOR, to count as working.
const LIVE_FOR: Ms = 2 * 60 * 1000;
const WORKING_FOR: Ms = 15 * 1000;

pub(crate) fn mtime(m: &std::fs::Metadata) -> Ms {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as Ms)
}

/// The sessions Pi wrote to lately: a pi in a terminal, most often. Pi
/// keeps no record of which sessions run, so one written in the last two
/// minutes is taken as running, and in the last fifteen seconds as
/// working. Sessions rush runs itself are also here; the host's rows claim
/// them.
pub(crate) fn live(p: &Profile) -> Vec<Session> {
    let now = now();
    let mut out = Vec::new();
    for path in session_files(p) {
        let Ok(fi) = std::fs::metadata(&path) else {
            continue;
        };
        let m = mtime(&fi);
        if now - m > LIVE_FOR {
            continue;
        }
        let Some(mut s) = past_session(&path) else {
            continue;
        };
        (s.kind, s.profile, s.state) = (p.kind.clone(), p.clone(), "idle".into());
        if now - m < WORKING_FOR {
            s.state = "working".into();
        }
        out.push(s);
    }
    out
}

/// Every session of p's, newest first.
pub(crate) fn past(p: &Profile) -> Vec<Session> {
    let mut out: Vec<Session> = session_files(p)
        .iter()
        .filter_map(|path| past_session(path))
        .map(|mut s| {
            (s.kind, s.profile, s.state) = (p.kind.clone(), p.clone(), "done".into());
            s
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    out
}

/// A session file's head as last read: a file is read again only once it
/// has changed.
struct Head {
    modified: Ms,
    size: u64,
    s: Option<Session>,
}

/// Heads by path.
static HEADS: Mutex<Option<HashMap<PathBuf, Head>>> = Mutex::new(None);

/// What a list needs from a session file.
fn past_session(path: &Path) -> Option<Session> {
    let fi = std::fs::metadata(path).ok()?;
    let (m, size) = (mtime(&fi), fi.len());
    {
        let heads = HEADS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = heads.as_ref().and_then(|h| h.get(path))
            && h.modified == m
            && h.size == size
        {
            return h.s.clone();
        }
    }
    let s = read_head(path, m);
    HEADS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(
            path.to_path_buf(),
            Head {
                modified: m,
                size,
                s: s.clone(),
            },
        );
    s
}

/// One line of a session file, with the fields of every type rush reads:
/// session, message, model_change, compaction, session_info, ...
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Entry {
    #[serde(rename = "type", deserialize_with = "nd")]
    pub typ: String,
    #[serde(deserialize_with = "nd")]
    pub id: String,
    #[serde(rename = "parentId", deserialize_with = "nd")]
    pub parent_id: String,
    #[serde(deserialize_with = "nd")]
    pub timestamp: String,

    #[serde(deserialize_with = "nd")]
    pub cwd: String, // session
    pub message: Option<Message>, // message
    #[serde(rename = "modelId", deserialize_with = "nd")]
    pub model_id: String, // model_change
    #[serde(deserialize_with = "nd")]
    pub name: String, // session_info

    #[serde(deserialize_with = "nd")]
    pub summary: String, // compaction, branch_summary
    #[serde(rename = "tokensBefore", deserialize_with = "nd")]
    pub tokens_before: i64,
}

impl Entry {
    pub fn time(&self) -> Ms {
        if self.timestamp.is_empty() {
            return 0;
        }
        parse_rfc3339(&self.timestamp)
    }
}

/// How many lines of a session a list reads looking for its first prompt;
/// a name set later is looked for in the whole file.
const HEAD_LINES: usize = 200;

const NAME_MARK: &[u8] = br#""type":"session_info""#;
const USER_MARK: &[u8] = br#""role":"user""#;
const MODEL_MARK: &[u8] = br#""model_change""#;

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A session's header, model and name: the one it was given, else its
/// first prompt.
fn read_head(path: &Path, modified: Ms) -> Option<Session> {
    let f = File::open(path).ok()?;
    let p = path.to_string_lossy().into_owned();
    let mut s = Session {
        id: id_of_file(&p),
        transcript: p,
        updated_at: modified,
        ..Session::default()
    };
    let (mut n, mut header, mut named) = (0usize, false, false);
    let _ = read_lines(f, |b| {
        n += 1;
        // Only lines a head needs are decoded: most are messages, some big.
        let wanted = n == 1
            || contains(b, NAME_MARK)
            || (n < HEAD_LINES
                && ((s.model.is_empty() && contains(b, MODEL_MARK))
                    || (s.name.is_empty() && contains(b, USER_MARK))));
        if !wanted {
            return true;
        }
        let Ok(e) = serde_json::from_slice::<Entry>(b) else {
            return true;
        };
        match e.typ.as_str() {
            "session" => {
                header = true;
                if !e.id.is_empty() {
                    s.id = e.id.clone();
                }
                (s.cwd, s.created_at) = (e.cwd.clone(), e.time());
            }
            "model_change" => {
                if s.model.is_empty() {
                    s.model = e.model_id;
                }
            }
            "session_info" => {
                let t = one_line(&e.name);
                if !t.is_empty() {
                    (s.name, named) = (t, true);
                }
            }
            "message" => {
                if let Some(m) = &e.message
                    && m.role == "user"
                    && s.name.is_empty()
                    && !named
                {
                    s.name = one_line(&m.text());
                }
            }
            _ => {}
        }
        true
    });
    if !header {
        return None;
    }
    if s.updated_at == 0 {
        s.updated_at = s.created_at;
    }
    Some(s)
}

/// The longest session line read; a longer one ends the read.
const MAX_FILE_LINE: usize = 64 << 20;

/// Why a read stopped short.
#[derive(Debug)]
pub(crate) struct TooLong;

impl std::fmt::Display for TooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("bufio.Scanner: token too long")
    }
}

impl std::error::Error for TooLong {}

/// Calls f with each line of r, trimmed, until it says stop.
pub(crate) fn read_lines(
    r: impl Read,
    mut f: impl FnMut(&[u8]) -> bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut br = BufReader::with_capacity(64 << 10, r);
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = (&mut br)
            .take(MAX_FILE_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        if line.len() > MAX_FILE_LINE {
            return Err(Box::new(TooLong));
        }
        let b = line.trim_ascii();
        if !b.is_empty() && !f(b) {
            return Ok(());
        }
    }
}

/// How many characters of a prompt name a session.
const NAME_LEN: usize = 80;

/// text on one line, cut to NAME_LEN characters.
pub(crate) fn one_line(text: &str) -> String {
    let mut b = String::new();
    let (mut n, mut gap) = (0usize, false);
    for c in text.chars() {
        if c.is_whitespace() {
            gap = !b.is_empty();
            continue;
        }
        if gap {
            b.push(' ');
            (n, gap) = (n + 1, false);
        }
        b.push(c);
        n += 1;
        if n > NAME_LEN {
            break;
        }
    }
    if b.chars().count() > NAME_LEN {
        let cut: String = b.chars().take(NAME_LEN - 1).collect();
        return format!("{}…", cut.trim());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_of_files() {
        for (path, want) in [
            ("/a/2026-09-29T13-03-54-573Z_01a0ed43.jsonl", "01a0ed43"),
            ("/a/nounderscore.jsonl", ""),
            ("/a/x_y.json", ""),
        ] {
            assert_eq!(id_of_file(path), want, "{path}");
        }
    }

    #[test]
    fn one_line_squeezes_and_cuts() {
        assert_eq!(
            one_line("  Find the auth code\nand list it "),
            "Find the auth code and list it"
        );
        let long = "x".repeat(100);
        let got = one_line(&long);
        assert_eq!(got.chars().count(), 80);
        assert!(got.ends_with('…'));
        assert_eq!(one_line(&"y".repeat(80)), "y".repeat(80));
    }
}
