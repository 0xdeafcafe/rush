//! What the gate's commands ask of rush's sessions and plugins, which
//! other modules own: a session's log, its inbox, whether it runs, its
//! name; whether a process is an agent's program; and what search-guard
//! and the watch plugin say of a Bash call. [`FileHost`] answers from the
//! sessions' own files as Go's host package keeps them
//! (internal/host/events.go:18-31, inbox.go:40-118, client.go:179-200,
//! host.go:359-380, 2430-2445), and from the agent registry; search-guard
//! and the watch plugin are off until the program wires them in.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};

/// A line's fields, for a log.
pub type Fields = Map<String, Value>;

/// What the gate's commands ask of the rest of rush.
pub trait Host: Send + Sync {
    /// Records an event of session id's, in its own log (host.LogEvent).
    fn log_event(&self, id: &str, kind: &str, fields: Fields);
    /// Records an event in rush's own log (events.Log).
    fn log(&self, kind: &str, fields: Fields) {
        rush_events::log(kind, fields);
    }
    /// Leaves text for session id's main session as rush's word, when its
    /// host runs and its agent takes the inbox (host.Notice).
    fn notice(&self, id: &str, text: &str) -> Result<(), String>;
    /// Whether session id's host runs (host.SessionAlive).
    fn session_alive(&self, id: &str) -> bool;
    /// Session id's name, when its info says one (host.ReadInfo).
    fn session_name(&self, id: &str) -> Option<String>;
    /// Whether a process called comm is a registered agent's program
    /// (agent.IsProgram).
    fn is_agent_program(&self, comm: &str) -> bool;
    /// Why search-guard refuses command, while it's on; "" to let it go
    /// (searchguard.On, searchguard.Refusal).
    fn search_refusal(&self, _command: &str) -> String {
        String::new()
    }
    /// What the watch plugin, while it's on, makes of a Bash call: the
    /// command to run in its place, or why it's refused; both "" to let it
    /// go (prwatch.On, watch.Redirect).
    fn watch_redirect(
        &self,
        _command: &str,
        _background: bool,
        _timeout: Duration,
        _exe: &str,
    ) -> (String, String) {
        (String::new(), String::new())
    }
}

/// The sessions as their files have them, under rush's folder.
#[derive(Clone, Debug)]
pub struct FileHost {
    /// rush's folder.
    pub home: PathBuf,
}

impl Default for FileHost {
    fn default() -> Self {
        FileHost {
            home: rush_state::dir(),
        }
    }
}

/// What of a session's info.json the gate reads.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Info {
    name: String,
    #[serde(rename = "hostPid")]
    host_pid: i32,
    inbox: bool,
}

/// What a main session's inbox file is called (host.MainNotice).
const MAIN_NOTICE: &str = "rush-notice";

impl FileHost {
    fn dir(&self, id: &str) -> PathBuf {
        self.home.join("sessions").join(id)
    }

    fn info(&self, id: &str) -> Option<Info> {
        if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
            return None;
        }
        let b = std::fs::read(self.dir(id).join("info.json")).ok()?;
        rush_state::json::from_slice(&b).ok()
    }
}

/// Whether pid is a running process: a zombie isn't.
pub fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let ok = match rush_sys::proc::kill(pid, 0) {
        Ok(()) => true,
        Err(rush_sys::proc::KillError::Os(e)) => e == libc::EPERM,
        Err(_) => false,
    };
    ok && !rush_sys::proc::zombie(pid)
}

impl Host for FileHost {
    fn log_event(&self, id: &str, kind: &str, fields: Fields) {
        if id.is_empty() {
            return;
        }
        rush_events::session(id, &self.dir(id).join("events.jsonl"), kind, fields);
    }

    fn notice(&self, id: &str, text: &str) -> Result<(), String> {
        let Some(info) = self.info(id) else {
            return Err(format!("no session {id}"));
        };
        if !info.inbox || !alive(info.host_pid) {
            return Ok(());
        }
        let dir = self.dir(id).join("inbox");
        crate::gate::mkdir_all(&dir).map_err(|e| e.to_string())?;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(dir.join(MAIN_NOTICE))
            .map_err(|e| e.to_string())?;
        f.write_all(format!("{text}\n\n").as_bytes())
            .map_err(|e| e.to_string())
    }

    fn session_alive(&self, id: &str) -> bool {
        self.info(id).is_some_and(|i| alive(i.host_pid))
    }

    fn session_name(&self, id: &str) -> Option<String> {
        self.info(id).map(|i| i.name).filter(|n| !n.is_empty())
    }

    fn is_agent_program(&self, comm: &str) -> bool {
        rush_core::adapter::global().is_program(comm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_sessions_files() {
        let home = tempfile::tempdir().unwrap();
        let h = FileHost {
            home: home.path().to_path_buf(),
        };
        let d = home.path().join("sessions/s1");
        std::fs::create_dir_all(&d).unwrap();
        let me = std::process::id();
        std::fs::write(
            d.join("info.json"),
            format!(r#"{{"name":"Fix the build","hostPid":{me},"inbox":true}}"#),
        )
        .unwrap();
        assert_eq!(h.session_name("s1").as_deref(), Some("Fix the build"));
        assert!(h.session_alive("s1"));
        h.notice("s1", "rush paused your `tsc` run").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("inbox/rush-notice")).unwrap(),
            "rush paused your `tsc` run\n\n"
        );
        assert_eq!(h.session_name("../s1"), None);
        assert!(!h.session_alive("gone"));
    }
}
