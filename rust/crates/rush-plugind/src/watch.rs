//! sessions.watch: the list as it is, then each change to it; and the
//! checkout a session's folder is in, read from .git rather than by
//! running git.
//! Ported from internal/plugind/watch.go.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_plugin::{Ctx, Error, Raw};
use serde_json::json;

use crate::runner::Runner;
use crate::sessions::{Session, session_of};
use crate::{server, to_raw};

/// How often a watching plugin's list is checked for changes.
const WATCH_EVERY: Duration = Duration::from_secs(2);

impl Runner {
    /// Answers sessions.watch: the list as it is now, then a
    /// session.changed notification whenever one appears or changes, and
    /// session.gone when one is removed. Watching again replaces the old
    /// watch.
    pub(crate) fn watch_list(self: &Arc<Self>) -> Result<Option<Raw>, Error> {
        // One lister for the watch, so each look reads only the info files
        // that changed.
        let mut l = self.hosts.lister();
        let mut now: HashMap<String, Session> = HashMap::new();
        let mut out = Vec::new();
        for i in l() {
            let s = session_of(&i);
            now.insert(s.id.clone(), s.clone());
            out.push(s);
        }
        let (ctx, cancel) = Ctx::background().with_cancel();
        let conn = {
            let mut st = self.lock();
            if let Some(old) = st.unwatch.replace(cancel.clone()) {
                old.cancel();
            }
            st.conn.clone()
        };
        let Some(conn) = conn else {
            // Asked for during initialize: the plugin asks again once it's up.
            cancel.cancel();
            return Err(server("not ready: watch once initialize has been answered"));
        };
        // It ends with the watch, or the connection.
        let reg = conn.ctx().on_cancel(move || cancel.cancel());
        self.scope.spawn("plugin-watch", move || {
            let _reg = reg;
            while ctx.sleep(WATCH_EVERY) {
                let mut seen = HashSet::new();
                for i in l() {
                    let s = session_of(&i);
                    seen.insert(s.id.clone());
                    if now.get(&s.id).is_none_or(|was| !same(was, &s)) {
                        now.insert(s.id.clone(), s.clone());
                        let _ = conn.notify("session.changed", &s);
                    }
                }
                now.retain(|id, _| {
                    let keep = seen.contains(id);
                    if !keep {
                        let _ = conn.notify("session.gone", &json!({"id": id}));
                    }
                    keep
                });
            }
        });
        Ok(Some(to_raw(&out)))
    }
}

/// Says two looks at a session found nothing worth telling a plugin: only
/// the time of the look differs.
pub(crate) fn same(a: &Session, b: &Session) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    a.updated_at = rush_state::Time::default();
    b.updated_at = rush_state::Time::default();
    a == b
}

/// The checkout a folder is in.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GitInfo {
    pub(crate) repo: String,
    pub(crate) branch: String,
    /// a linked worktree, not the main checkout
    pub(crate) worktree: bool,
    at: Option<Instant>,
}

static GIT_CACHE: Mutex<Option<HashMap<String, GitInfo>>> = Mutex::new(None);

/// The checkout dir is in, found by reading .git and HEAD, not by running
/// git: it's asked for every session every few seconds.
pub(crate) fn git_of(dir: &str) -> GitInfo {
    if dir.is_empty() {
        return GitInfo::default();
    }
    let now = Instant::now();
    {
        let c = GIT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(g) = c.as_ref().and_then(|m| m.get(dir))
            && g.at
                .is_some_and(|at| now.duration_since(at) < Duration::from_secs(30))
        {
            return g.clone();
        }
    }
    let mut g = GitInfo {
        at: Some(now),
        ..Default::default()
    };
    let mut d = dir.to_string();
    while d != "/" && d != "." && !d.is_empty() {
        let p = Path::new(&d).join(".git");
        if let Ok(st) = std::fs::metadata(&p) {
            let mut git_dir = p.to_string_lossy().into_owned();
            if !st.is_dir() {
                let b = std::fs::read_to_string(&p).unwrap_or_default();
                let s = b.trim();
                let mut s = s.strip_prefix("gitdir:").unwrap_or(s).trim().to_string();
                if !s.starts_with('/') {
                    s = clean_join(&d, &s);
                }
                g.worktree = s.contains("/.git/worktrees/");
                git_dir = s;
            }
            g.repo = d.clone();
            if let Ok(b) = std::fs::read_to_string(Path::new(&git_dir).join("HEAD")) {
                let h = b.trim();
                g.branch = h.strip_prefix("ref: refs/heads/").unwrap_or(h).to_string();
                if g.branch.len() == 40 {
                    g.branch.truncate(8);
                }
            }
            break;
        }
        d = parent(&d);
    }
    let mut c = GIT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let m = c.get_or_insert_with(HashMap::new);
    if m.len() > 512 {
        m.clear();
    }
    m.insert(dir.to_string(), g.clone());
    g
}

/// filepath.Dir.
fn parent(d: &str) -> String {
    match d.trim_end_matches('/').rfind('/') {
        Some(0) => "/".into(),
        Some(i) => d[..i].to_string(),
        None => ".".into(),
    }
}

/// filepath.Join(d, s), cleaned of . and ..
fn clean_join(d: &str, s: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in d.split('/').chain(s.split('/')) {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_ignores_only_the_look() {
        let a = Session {
            id: "a".into(),
            state: "idle".into(),
            meta: [("card".to_string(), "1".to_string())].into(),
            ..Default::default()
        };
        let mut b = a.clone();
        b.updated_at = b.updated_at.add_nanos(1);
        assert!(same(&a, &b), "a new look alone counted as a change");
        b.context_tokens = 5;
        assert!(!same(&a, &b), "a change was missed");
    }

    #[test]
    fn git_of_reads_the_checkout() {
        let Ok(o) = std::process::Command::new("git").arg("--version").output() else {
            return; // no git
        };
        assert!(o.status.success());
        let t = tempfile::tempdir().unwrap();
        let repo = t
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(o.status.success(), "git {args:?}: {o:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        let wt = format!("{repo}/.claude/worktrees/task");
        git(&["worktree", "add", "-q", "-b", "task-1", &wt]);
        std::fs::create_dir_all(format!("{wt}/sub")).unwrap();
        let g = git_of(&repo);
        assert!(
            g.repo == repo && g.branch == "main" && !g.worktree,
            "main checkout = {g:?}"
        );
        let g = git_of(&format!("{wt}/sub"));
        assert!(
            g.repo == wt && g.branch == "task-1" && g.worktree,
            "worktree = {g:?}"
        );
    }
}
