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

use rush_convo::{Fresh, Tail};
use rush_core::adapter::{Registry, global};
use rush_core::agent::{Agent, Preview};
use rush_core::time::{Ms, now};
use rush_draw::Sender;
use rush_fleet::Loader;
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
const LIVE_WINDOW: i64 = 128 << 10;
pub const PICKED_WINDOW: i64 = 384 << 10;

/// Reads the list once, as `rush --dump` and `--render` do: rows, a spend
/// scan, rows again.
pub fn load_once(now: Ms) -> Fleet {
    let mut d = Discovery::new();
    d.load(now, true)
}

/// Discovery's state between readings.
pub struct Discovery {
    reg: Registry,
    loader: Loader,
    roots: HashMap<String, String>,
}

impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Discovery {
    /// Discovery of every agent in rush's registry.
    pub fn new() -> Discovery {
        Discovery::with(global().clone())
    }

    pub fn with(reg: Registry) -> Discovery {
        Discovery {
            loader: Loader::new(reg.clone()),
            reg,
            roots: HashMap::new(),
        }
    }

    /// The rows; `price` scans spend first.
    pub fn load(&mut self, now: Ms, price: bool) -> Fleet {
        let mut agents = self.loader.load(now);
        if price {
            self.loader.scan(&agents);
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
                let p = self
                    .reg
                    .read_preview(a.kind.as_str(), &a.job.transcript_path, LIVE_WINDOW);
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

    /// The folders and files whose changes mean a new reading (what the
    /// last one read).
    fn watched(&self) -> Vec<PathBuf> {
        self.loader.watched()
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
/// the agents' folders change, or every POLL while an agent is open.
pub fn spawn_discovery(tx: Sender<Note>) {
    // One reading waits at most: the next is read once it's taken.
    let tx = tx.lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-discovery".into())
        .spawn(move || {
            let mut d = Discovery::new();
            let first = d.load(now(), false);
            // Watched before each reading after, so a change during it
            // still wakes; the first says what there is to watch.
            let w = Watcher::new();
            w.watch(&d.watched());
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
pub fn spawn_preview(tx: Sender<Note>, reg: Registry, kind: String, key: String, path: String) {
    let tx = tx.lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-preview".into())
        .spawn(move || {
            let p = reg.read_preview(&kind, &path, PICKED_WINDOW);
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
    // One slice waits at most: the next is read while the UI applies it.
    let tx = tx.lane(1);
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

    /// The open conversation's tail reads what's there, then wakes for
    /// what's appended (fswait's Watcher), and stops when dropped.
    #[test]
    fn the_tail_follows_appends() {
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        let d = std::env::temp_dir().join(format!("rush-ui-tail-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.jsonl");
        let line = |t: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-09-29T10:00:00Z","message":{{"id":"{t}","role":"assistant","content":[{{"type":"text","text":"{t}"}}]}}}}"#
            ) + "\n"
        };
        std::fs::write(&p, line("one")).unwrap();
        struct Notes {
            n: usize,
            path: PathBuf,
            more: String,
        }
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                if let Msg::App(Note::Tail { .. }) = m {
                    self.n += 1;
                    if self.n == 1 {
                        use std::io::Write;
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&self.path)
                            .unwrap()
                            .write_all(self.more.as_bytes())
                            .unwrap();
                    } else {
                        return Step::Quit;
                    }
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let lp: Loop<Note> = Loop::new();
        let decode = rush_convo::decode(rush_claude::messages);
        let h = spawn_tail(p.clone(), 1, lp.sender(), decode).unwrap();
        let mut app = Notes {
            n: 0,
            path: p.clone(),
            more: line("two"),
        };
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.n, 2, "the append wasn't read");
        drop(h);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
