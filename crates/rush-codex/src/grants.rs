//! What rush keeps of Codex's approvals, past what Codex keeps itself.
//! Rust-only, no Go spec.
//!
//! - "Yes, for this session" lives in the app-server's memory, so a
//!   session rush put to sleep and woke forgets it. Rush keeps each
//!   thread's such answers in `<rush>/codex/granted/<thread>.json` and
//!   gives them again when the same ask comes back.
//! - Rush's own commands (`rush feed`, `rush watch`, …), which its prompt
//!   asks agents to use, need its folders and socket, which Codex's
//!   sandbox doesn't give: `<CODEX_HOME>/rules/rush.rules` allows them.

use std::path::{Path, PathBuf};

/// The rules file rush keeps in a Codex home.
pub(crate) const RULES: &str = "# Written by rush: its own commands (rush feed, rush watch, rush todo, …)\n\
# run without asking. Delete this file to be asked again.\n\
prefix_rule(pattern = [\"rush\"], decision = \"allow\")\n";

/// Writes rush's rules into the Codex home `home` ("" is CODEX_HOME, or
/// ~/.codex), unless they're there already.
pub(crate) fn write_rules(home: &str) {
    let Some(dir) = codex_home(home) else { return };
    let p = dir.join("rules").join("rush.rules");
    if std::fs::read_to_string(&p).is_ok_and(|b| b == RULES) {
        return;
    }
    if let Some(d) = p.parent()
        && std::fs::create_dir_all(d).is_ok()
    {
        let _ = std::fs::write(&p, RULES);
    }
}

fn codex_home(home: &str) -> Option<PathBuf> {
    if !home.is_empty() {
        return Some(PathBuf::from(home));
    }
    if let Some(h) = std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| Path::new(&h).join(".codex"))
}

/// The answers for a session one thread was given, kept under dir.
#[derive(Clone, Debug, Default)]
pub(crate) struct Granted {
    dir: PathBuf,
}

impl Granted {
    /// Kept under rush's folder as this thread sees it.
    pub(crate) fn here() -> Granted {
        Granted {
            dir: rush_state::dir().join("codex").join("granted"),
        }
    }

    fn file(&self, thread: &str) -> Option<PathBuf> {
        let ok = !thread.is_empty()
            && thread
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        ok.then(|| self.dir.join(format!("{thread}.json")))
    }

    fn read(&self, thread: &str) -> Vec<String> {
        self.file(thread)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Whether thread was told yes for the session to what key names.
    pub(crate) fn has(&self, thread: &str, key: &str) -> bool {
        !key.is_empty() && self.read(thread).iter().any(|k| k == key)
    }

    /// Keeps thread's yes for the session to what key names.
    pub(crate) fn add(&self, thread: &str, key: &str) {
        let Some(p) = self.file(thread) else { return };
        if key.is_empty() {
            return;
        }
        let mut all = self.read(thread);
        if all.iter().any(|k| k == key) {
            return;
        }
        all.push(key.to_string());
        let _ = rush_state::store::write_json(&p, &all);
    }
}

/// What an ask is, to know it again: the same command in the same
/// folder, the same folder to write to, or the same permissions.
pub(crate) fn key(method: &str, parts: &[&str]) -> String {
    let mut k = method.to_string();
    for p in parts {
        k.push('\0');
        k.push_str(p);
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_per_thread() {
        let d = tempfile::tempdir().unwrap();
        let g = Granted {
            dir: d.path().join("granted"),
        };
        let k = key("cmd", &["cargo test", "/w"]);
        assert!(!g.has("th1", &k));
        g.add("th1", &k);
        g.add("th1", &k);
        assert!(g.has("th1", &k));
        assert!(!g.has("th2", &k), "another session's");
        assert!(!g.has("th1", &key("cmd", &["cargo test", "/elsewhere"])));
        assert_eq!(g.read("th1").len(), 1, "kept once");
        g.add("../x", &k);
        assert!(!g.has("../x", &k), "a thread id that isn't one");
    }

    #[test]
    fn rules_written_once() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_string_lossy().into_owned();
        write_rules(&home);
        let p = d.path().join("rules").join("rush.rules");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), RULES);
        std::fs::write(&p, "old").unwrap();
        write_rules(&home);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), RULES, "put back");
    }
}
