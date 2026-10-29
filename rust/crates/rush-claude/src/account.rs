//! Where Claude Code keeps an account's things. Port of
//! internal/adapters/claude/claude/account.go:13-81, login.go:50-58
//! (SignedInAs), forkcopy.go:73-95 and
//! adapters/claude/settings.go:55 (CLAUDE_CONFIG_DIR).

use std::path::{Path, PathBuf};

use crate::jobs::project_slug;

/// One Claude Code config home. The default account is ~/.claude with its
/// state in ~/.claude.json; any other lives wherever CLAUDE_CONFIG_DIR
/// points and keeps its state file inside that directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub config_dir: PathBuf,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

impl Account {
    /// ~/.claude, where every session rush lists runs: the list reads it
    /// whatever CLAUDE_CONFIG_DIR says, as Go's does.
    pub fn home() -> Account {
        Account {
            name: "default".into(),
            config_dir: home_dir().join(".claude"),
        }
    }

    /// The folder Claude Code itself runs in from this environment:
    /// CLAUDE_CONFIG_DIR's, else ~/.claude.
    pub fn from_env() -> Account {
        match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(d) if !d.is_empty() => Account {
                name: String::new(),
                config_dir: PathBuf::from(d),
            },
            _ => Account::home(),
        }
    }

    /// An account in any folder: tests use a temp dir.
    pub fn at(name: &str, dir: impl Into<PathBuf>) -> Account {
        Account {
            name: name.into(),
            config_dir: dir.into(),
        }
    }

    pub fn is_default(&self) -> bool {
        self.config_dir == home_dir().join(".claude")
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.config_dir.join("jobs")
    }
    pub fn projects_dir(&self) -> PathBuf {
        self.config_dir.join("projects")
    }
    pub fn sessions_dir(&self) -> PathBuf {
        self.config_dir.join("sessions")
    }
    pub fn roster_path(&self) -> PathBuf {
        self.config_dir.join("daemon").join("roster.json")
    }

    /// Whether this account's background daemon is up: its control
    /// socket is there (daemon/daemon.go:29-40, SockDir and Running).
    pub fn daemon_running(&self) -> bool {
        use sha2::Digest;
        use std::fmt::Write as _;
        let sum = sha2::Sha256::digest(self.config_dir.as_os_str().as_encoded_bytes());
        let mut hex = String::with_capacity(8);
        for b in &sum[..4] {
            let _ = write!(hex, "{b:02x}");
        }
        // SAFETY: getuid can't fail.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/cc-daemon-{uid}"))
            .join(hex)
            .join("control.sock")
            .exists()
    }

    /// Its state file: ~/.claude.json for the default, else inside it.
    pub fn state_path(&self) -> PathBuf {
        if self.is_default() {
            let mut s = self.config_dir.clone().into_os_string();
            s.push(".json");
            return s.into();
        }
        self.config_dir.join(".claude.json")
    }

    /// What a child claude process needs to act as this account: this
    /// process's environment without another account's folder or the
    /// markers of a Claude Code session rush was started from, and the
    /// folder when it isn't ~/.claude.
    pub fn env(&self) -> Vec<String> {
        let mut env: Vec<String> = std::env::vars_os()
            .filter_map(|(k, v)| {
                let k = k.into_string().ok()?;
                if k == "CLAUDE_CONFIG_DIR" || INHERITED.contains(&k.as_str()) {
                    return None; // another account's, or the session rush was started from
                }
                Some(format!("{k}={}", v.to_string_lossy()))
            })
            .collect();
        if !self.is_default() {
            env.push(format!("CLAUDE_CONFIG_DIR={}", self.config_dir.display()));
        }
        env
    }

    /// The uuid of the login this folder is signed in as, from its state
    /// file alone; "" when it isn't.
    pub fn signed_in_as(&self) -> String {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct State {
            #[serde(rename = "oauthAccount")]
            oauth: Option<Login>,
        }
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Login {
            #[serde(rename = "accountUuid")]
            uuid: Option<String>,
        }
        std::fs::read(self.state_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<State>(&b).ok())
            .and_then(|s| s.oauth)
            .and_then(|p| p.uuid)
            .unwrap_or_default()
    }

    /// Where session sid keeps its conversation now: under the folder it
    /// started in, or, once entering a worktree moved it, under whichever
    /// project has it; where it would be when none does.
    pub fn find_transcript(&self, cwd: &str, sid: &str) -> PathBuf {
        let at = self.transcript_path(cwd, sid);
        if at.exists() || sid.is_empty() || sid.contains(['/', '*', '?', '[', '\\']) {
            return at;
        }
        let name = format!("{sid}.jsonl");
        std::fs::read_dir(self.projects_dir())
            .ok()
            .and_then(|r| {
                r.flatten()
                    .map(|e| e.path().join(&name))
                    .find(|p| p.exists())
            })
            .unwrap_or(at)
    }

    /// The session's transcript in this account's projects folder.
    pub fn transcript_path(&self, cwd: &str, session_id: &str) -> PathBuf {
        let mut p = self.projects_dir().join(project_slug(cwd));
        p.push(format!("{session_id}.jsonl"));
        p
    }
}

/// The markers a Claude Code session sets for what it runs. rush started
/// from inside one would pass them on, and the sessions rush starts would
/// think they're that session's children: Claude Code then saves no
/// transcript for them, among other things.
const INHERITED: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CLAUDE_JOB_DIR",
    "CLAUDE_CODE_VERSION",
];

/// p as a string, for the records rush_core keeps paths in.
pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders() {
        let a = Account::at("x", "/c");
        assert_eq!(a.jobs_dir(), PathBuf::from("/c/jobs"));
        assert_eq!(a.state_path(), PathBuf::from("/c/.claude.json"));
        assert_eq!(
            a.transcript_path("/Users/me/x", "s1"),
            PathBuf::from("/c/projects/-Users-me-x/s1.jsonl")
        );
        let h = Account::home();
        assert!(h.is_default());
        assert!(h.state_path().to_string_lossy().ends_with("/.claude.json"));
    }
}
