//! Live Claude Code processes as they register under sessions/<pid>.json,
//! and the daemon's roster of background workers. Port of
//! internal/adapters/claude/claude/sessions.go and roster.go:12-70.

use std::path::Path;

use rush_core::time::Ms;
use serde::Deserialize;

use crate::account::Account;
use crate::json::nd;

/// A live Claude Code process as it registers itself. Interactive
/// terminals only appear here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Session {
    #[serde(deserialize_with = "nd")]
    pub pid: i32,
    #[serde(rename = "sessionId", deserialize_with = "nd")]
    pub session_id: String,
    #[serde(deserialize_with = "nd")]
    pub cwd: String,
    /// bg or interactive
    #[serde(deserialize_with = "nd")]
    pub kind: String,
    #[serde(deserialize_with = "nd")]
    pub name: String,
    /// busy, idle, shell
    #[serde(deserialize_with = "nd")]
    pub status: String,
    #[serde(rename = "jobId", deserialize_with = "nd")]
    pub job_id: String,
    #[serde(rename = "startedAt", deserialize_with = "nd")]
    pub started_ms: i64,
    #[serde(rename = "updatedAt", deserialize_with = "nd")]
    pub updated_ms: i64,
    #[serde(rename = "statusUpdatedAt", deserialize_with = "nd")]
    pub status_ms: i64,
}

impl Session {
    pub fn started_at(&self) -> Ms {
        self.started_ms
    }
    pub fn updated_at(&self) -> Ms {
        if self.updated_ms == 0 {
            self.started_ms
        } else {
            self.updated_ms
        }
    }
    pub fn status_at(&self) -> Ms {
        self.status_ms
    }
}

/// Reads one session file, whether or not its process lives.
pub fn read_session(path: &Path) -> Option<Session> {
    let b = std::fs::read(path).ok()?;
    let s: Session = serde_json::from_slice(&b).ok()?;
    (s.pid > 0).then_some(s)
}

/// The account's sessions whose process is still alive.
pub fn read_sessions(a: &Account) -> Vec<Session> {
    let Ok(ents) = std::fs::read_dir(a.sessions_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in ents.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().ends_with(".json") {
            continue;
        }
        if let Some(s) = read_session(&e.path())
            && session_alive(&s)
        {
            out.push(s);
        }
    }
    out
}

/// How long after a process starts it may write its session file.
const STARTUP: Ms = 30_000;

/// Whether session s's process still runs: its pid is there, not a zombie,
/// and started by the time the session did, so a pid reused by a later
/// process isn't taken for it.
pub fn session_alive(s: &Session) -> bool {
    if !alive(s.pid) || rush_sys::proc::zombie(s.pid) {
        return false;
    }
    match rush_sys::proc::started(s.pid) {
        Some(t) if s.started_at() > 0 => crate::jobs::ms_of(t) <= s.started_at() + STARTUP,
        _ => true,
    }
}

/// Whether a process exists.
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks the process is there.
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// A background session as the daemon tracks it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Worker {
    #[serde(deserialize_with = "nd")]
    pub pid: i32,
    #[serde(rename = "sessionId", deserialize_with = "nd")]
    pub session_id: String,
    #[serde(deserialize_with = "nd")]
    pub cwd: String,
    #[serde(deserialize_with = "nd")]
    dispatch: Dispatch,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Dispatch {
    #[serde(deserialize_with = "nd")]
    short: String,
}

impl Worker {
    /// The job it runs, by its short id.
    pub fn short(&self) -> &str {
        if !self.dispatch.short.is_empty() {
            return &self.dispatch.short;
        }
        self.session_id.get(..8).unwrap_or(&self.session_id)
    }
}

/// The daemon's workers by their job's short id, from its roster; a list
/// or a map, as either has been written.
pub fn read_workers(a: &Account) -> Vec<(String, i32)> {
    #[derive(Deserialize)]
    struct Raw<'a> {
        #[serde(borrow)]
        workers: Option<&'a serde_json::value::RawValue>,
    }
    let Ok(b) = std::fs::read(a.roster_path()) else {
        return Vec::new();
    };
    let Ok(raw) = serde_json::from_slice::<Raw>(&b) else {
        return Vec::new();
    };
    let Some(w) = raw.workers else {
        return Vec::new();
    };
    let list: Vec<Worker> = serde_json::from_str(w.get()).unwrap_or_else(|_| {
        serde_json::from_str::<std::collections::HashMap<String, Worker>>(w.get())
            .map(|m| m.into_values().collect())
            .unwrap_or_default()
    });
    list.into_iter()
        .map(|w| (w.short().to_string(), w.pid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sessions_whose_process_lives() {
        let dir = std::env::temp_dir().join(format!("rush-claude-sess-{}", std::process::id()));
        let sd = dir.join("sessions");
        std::fs::create_dir_all(&sd).unwrap();
        let me = std::process::id();
        let at = rush_core::time::now();
        std::fs::write(
            sd.join("1.json"),
            format!(r#"{{"pid":{me},"sessionId":"s-live","cwd":"/r","kind":"interactive","status":"busy","startedAt":{at},"name":null}}"#),
        )
        .unwrap();
        std::fs::write(sd.join("2.json"), r#"{"pid":0,"sessionId":"bad"}"#).unwrap();
        std::fs::write(sd.join("3.txt"), "x").unwrap();
        let got = read_sessions(&Account::at("t", &dir));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session_id, "s-live");
        assert_eq!(got[0].updated_at(), at);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn roster_as_list_or_map() {
        let dir = std::env::temp_dir().join(format!("rush-claude-roster-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("daemon")).unwrap();
        let a = Account::at("t", &dir);
        std::fs::write(
            a.roster_path(),
            r#"{"workers":[{"pid":7,"sessionId":"abcdef123456","dispatch":{"short":""}}]}"#,
        )
        .unwrap();
        assert_eq!(read_workers(&a), vec![("abcdef12".to_string(), 7)]);
        std::fs::write(
            a.roster_path(),
            r#"{"workers":{"x":{"pid":8,"dispatch":{"short":"j1"}}}}"#,
        )
        .unwrap();
        assert_eq!(read_workers(&a), vec![("j1".to_string(), 8)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A session's pid counts only while the process started by the time
    /// the session did: a pid reused by a later process isn't it.
    #[test]
    fn a_reused_pid_is_not_the_session() {
        let pid = std::process::id() as i32;
        let started = crate::jobs::ms_of(rush_sys::proc::started(pid).unwrap());
        let s = |at: Ms| Session {
            pid,
            started_ms: at,
            ..Session::default()
        };
        assert!(session_alive(&s(started + 1000)));
        assert!(session_alive(&s(0)), "no start to go by");
        assert!(
            !session_alive(&s(started - 3_600_000)),
            "a session an hour older"
        );
        assert!(!session_alive(&Session::default()));
    }
}
