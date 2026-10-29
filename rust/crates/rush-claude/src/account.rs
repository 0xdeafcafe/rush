//! Where Claude Code keeps an account's things. Port of
//! internal/adapters/claude/claude/account.go:13-53, forkcopy.go:73 and
//! adapters/claude/settings.go:55 (CLAUDE_CONFIG_DIR), account.go:55-80
//! (Env, inherited) and usagefetch.go:26-33 (keychainService).

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

    /// Claude Code's cache of each PR's status.
    pub fn pr_cache_path(&self) -> PathBuf {
        self.config_dir.join("gh-pr-status-cache.json")
    }

    /// What a child claude process needs to act as this account: this
    /// process's environment, less another account's folder and the
    /// markers of a Claude Code session rush may have been started from.
    pub fn env(&self) -> Vec<String> {
        let mut env: Vec<String> = std::env::vars_os()
            .filter_map(|(k, v)| {
                let k = k.to_string_lossy();
                if k == "CLAUDE_CONFIG_DIR" || INHERITED.contains(&k.as_ref()) {
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

    /// Where Claude Code keeps this account's sign-in in the keychain: the
    /// default account unsuffixed, any other suffixed with its folder's
    /// hash.
    pub fn keychain_service(&self) -> String {
        if self.is_default() {
            return "Claude Code-credentials".into();
        }
        use sha2::Digest;
        let sum = sha2::Sha256::digest(self.config_dir.as_os_str().as_encoded_bytes());
        let hex: String = sum[..4].iter().map(|b| format!("{b:02x}")).collect();
        format!("Claude Code-credentials-{hex}")
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
        assert_eq!(h.keychain_service(), "Claude Code-credentials");
        let svc = a.keychain_service();
        assert!(svc.starts_with("Claude Code-credentials-"));
        assert_eq!(svc.len(), "Claude Code-credentials-".len() + 8);
    }

    // commands_test.go:69 (TestEnvDropsInheritedMarkers), without changing
    // this process's environment: what's dropped is what's listed.
    #[test]
    fn env_drops_inherited_markers() {
        let env = Account::at("x", "/c").env();
        for e in &env {
            let k = e.split('=').next().unwrap_or("");
            assert!(!INHERITED.contains(&k), "{k} passed on");
        }
        assert_eq!(
            env.last().map(String::as_str),
            Some("CLAUDE_CONFIG_DIR=/c")
        );
        assert!(
            !Account::home()
                .env()
                .iter()
                .any(|e| e.starts_with("CLAUDE_CONFIG_DIR="))
        );
    }
}
