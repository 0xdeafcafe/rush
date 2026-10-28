//! The workers beside the UI thread (DESIGN.md "Threads"): discovery and
//! spend, previews, and the open conversation's tail. Each sends only what
//! changed, and waits on the files (kqueue on macOS) rather than a tick;
//! where it must poll, it polls every 2s and only while something is
//! live. After Go's ui/model.go:835-869 (loadPreview, live previews at
//! 128k, the selected one at 384k), fleet/worktree.go:427 (mainCheckout)
//! and the refresh tick of ui/model.go:1081.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rush_claude::Account;
use rush_claude::fleet::Loader;
use rush_convo::{Fresh, Tail};
use rush_core::agent::{Agent, Preview};
use rush_core::time::{Ms, now};
use rush_draw::Sender;
use rush_sys::fswait::{Stop, Watcher};

/// What a worker tells the UI thread.
pub enum Note {
    Fleet(Fleet),
    Preview(String, Box<Preview>),
    Tail { r#gen: u64, fresh: Box<Fresh> },
    TailFailed { r#gen: u64, err: String },
}

/// The list's rows as discovery last read them.
pub struct Fleet {
    pub agents: Vec<Agent>,
    /// Each agent's repository's main checkout, or "".
    pub roots: Vec<String>,
    pub previews: HashMap<String, Preview>,
    /// Their transcripts have been priced.
    pub priced: bool,
}

/// While something is live, how often discovery reads again.
pub const POLL: Duration = Duration::from_secs(2);
const LIVE_WINDOW: u64 = 128 << 10;
pub const PICKED_WINDOW: u64 = 384 << 10;

/// Reads the list once, as `rush --dump` and `--render` do: rows, a spend
/// scan, rows again.
pub fn load_once(now: Ms) -> Fleet {
    let mut d = Discovery::new();
    d.load(now, true)
}

/// Discovery's state between readings.
pub struct Discovery {
    loader: Loader,
    roots: HashMap<String, String>,
    acct: Account,
}

impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Discovery {
    pub fn new() -> Discovery {
        let acct = Account::from_env();
        Discovery {
            loader: Loader::new(acct.clone()),
            roots: HashMap::new(),
            acct,
        }
    }

    /// The rows; `price` scans spend first.
    pub fn load(&mut self, now: Ms, price: bool) -> Fleet {
        let mut agents = self.loader.load(now);
        if price {
            self.loader.scan(&agents, now);
            agents = self.loader.load(now);
        }
        let roots = agents
            .iter()
            .map(|a| {
                if a.repo.is_empty() {
                    return String::new();
                }
                self.roots
                    .entry(a.repo.clone())
                    .or_insert_with(|| main_checkout(&a.repo).unwrap_or_else(|| a.repo.clone()))
                    .clone()
            })
            .collect();
        let previews = agents
            .iter()
            .filter(|a| a.job.live() && !a.job.transcript_path.is_empty())
            .map(|a| {
                let p = rush_claude::transcript::read_preview(
                    Path::new(&a.job.transcript_path),
                    LIVE_WINDOW,
                );
                (a.key.clone(), p)
            })
            .collect();
        Fleet {
            agents,
            roots,
            previews,
            priced: price,
        }
    }

    /// The folders and files whose changes mean a new reading: the top
    /// folders, and each job's and project's folder below them (a folder
    /// watch doesn't see what happens in its subfolders).
    fn watched(&self) -> Vec<PathBuf> {
        let mut out = vec![
            self.acct.sessions_dir(),
            self.acct.jobs_dir(),
            self.acct.projects_dir(),
        ];
        for (top, file) in [
            (self.acct.jobs_dir(), Some("state.json")),
            (self.acct.projects_dir(), None),
        ] {
            let Ok(rd) = std::fs::read_dir(&top) else {
                continue;
            };
            for e in rd.flatten() {
                if !e.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let d = e.path();
                if let Some(f) = file {
                    out.push(d.join(f));
                }
                out.push(d);
            }
        }
        out.sort();
        out
    }
}

/// The main checkout of the repository dir is in: dir itself, or for a
/// linked worktree the checkout it came from.
pub fn main_checkout(dir: &str) -> Option<String> {
    let mut d = Path::new(dir);
    loop {
        let p = d.join(".git");
        if let Ok(m) = std::fs::metadata(&p) {
            if m.is_dir() {
                return Some(d.to_string_lossy().into_owned());
            }
            let b = std::fs::read_to_string(&p).ok()?;
            let g = b.trim().trim_start_matches("gitdir:").trim();
            let g = if g.starts_with('/') {
                g.to_string()
            } else {
                d.join(g).to_string_lossy().into_owned()
            };
            return g.find("/.git/worktrees/").map(|i| g[..i].to_string());
        }
        d = d.parent()?;
        if d.as_os_str() == "/" {
            return None;
        }
    }
}

/// Starts discovery: a reading now, priced at once, then again whenever
/// Claude Code's folders change, or every POLL while an agent is open.
pub fn spawn_discovery(tx: Sender<Note>) {
    let _ = std::thread::Builder::new()
        .name("rush-discovery".into())
        .spawn(move || {
            let mut d = Discovery::new();
            // Watched before each reading, so a change during it still wakes.
            let w = Watcher::new();
            w.watch(&d.watched());
            let first = d.load(now(), false);
            if !tx.send(Note::Fleet(first)) {
                return;
            }
            loop {
                let f = d.load(now(), true);
                let open = f.agents.iter().any(|a| a.pid != 0 || a.job.live());
                if !tx.send(Note::Fleet(f)) {
                    return;
                }
                w.wait(None, open.then_some(POLL));
                // New jobs or project folders: watch them too.
                w.watch(&d.watched());
            }
        });
}

/// Reads one agent's preview off the UI thread.
pub fn spawn_preview(tx: Sender<Note>, key: String, path: String) {
    let _ = std::thread::Builder::new()
        .name("rush-preview".into())
        .spawn(move || {
            let p = rush_claude::transcript::read_preview(Path::new(&path), PICKED_WINDOW);
            tx.send(Note::Preview(key, Box::new(p)));
        });
}

/// The tail of the conversation open: stops when dropped.
pub struct TailHandle {
    stop: Arc<AtomicBool>,
    // Closing it wakes the worker's wait.
    wake: Arc<Stop>,
}

impl Drop for TailHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.close();
    }
}

/// Reads a transcript's last 2 MB, then what's added as it grows; each
/// piece parsed here and sent for the UI thread to apply.
pub fn spawn_tail(
    path: PathBuf,
    r#gen: u64,
    tx: Sender<Note>,
    decode: rush_convo::Decode,
) -> Option<TailHandle> {
    let wake = Arc::new(Stop::new().ok()?);
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let wk = wake.clone();
    std::thread::Builder::new()
        .name("rush-tail".into())
        .spawn(move || {
            // Watched before the first read, so nothing written between a
            // read and the wait is missed.
            let w = Watcher::new();
            w.watch(&[&path]);
            let mut tail = Tail::from_end(&path, 2 << 20, decode);
            tail.stop = Some(st.clone());
            loop {
                if st.load(Ordering::Relaxed) {
                    return;
                }
                match tail.fetch() {
                    Ok(f) => {
                        if (!f.is_empty() || f.reset)
                            && !tx.send(Note::Tail {
                                r#gen,
                                fresh: Box::new(f),
                            })
                        {
                            return;
                        }
                    }
                    Err(e) => {
                        if !path.exists() {
                            tx.send(Note::TailFailed {
                                r#gen,
                                err: e.to_string(),
                            });
                        }
                    }
                }
                if !w.wait(Some(&wk), None) {
                    return;
                }
                // Replaced or removed: watch what's there now.
                w.watch(&[&path]);
            }
        })
        .ok()?;
    Some(TailHandle { stop, wake })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_checkout_of_a_worktree() {
        let t = std::env::temp_dir().join(format!("rush-ui-mc-{}", std::process::id()));
        let main = t.join("main");
        let tree = t.join("trees").join("x");
        std::fs::create_dir_all(main.join(".git").join("worktrees").join("x")).unwrap();
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(
            tree.join(".git"),
            format!("gitdir: {}/.git/worktrees/x\n", main.display()),
        )
        .unwrap();
        let m = main.to_string_lossy().into_owned();
        assert_eq!(main_checkout(&m).as_deref(), Some(m.as_str()));
        assert_eq!(
            main_checkout(&tree.join("sub").to_string_lossy()).as_deref(),
            Some(m.as_str())
        );
        std::fs::remove_dir_all(&t).unwrap();
    }
}
