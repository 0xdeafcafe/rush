//! The workers beside the UI thread (DESIGN.md "Threads"): discovery and
//! spend, previews, and the open conversation's tail. Each sends only what
//! changed, and waits on the files (kqueue on macOS) rather than a tick;
//! where it must poll, it polls every 2s and only while something is
//! live. After Go's ui/model.go:835-869 (loadPreview, live previews at
//! 128k, the selected one at 384k), fleet/worktree.go:427 (mainCheckout)
//! and the refresh tick of ui/model.go:1081.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
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
            let mut w = Watch::new(d.watched(), None);
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
                w.wait(open.then_some(POLL));
                // New jobs or project folders: watch them too.
                w.set_paths(d.watched());
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
    // Closing the write end wakes the worker's wait.
    _wake: OwnedFd,
}

impl Drop for TailHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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
    let (rd, wr) = pipe()?;
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::Builder::new()
        .name("rush-tail".into())
        .spawn(move || {
            // Watched before the first read, so nothing written between a
            // read and the wait is missed.
            let mut w = Watch::new(vec![path.clone()], Some(rd.as_raw_fd()));
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
                if !w.wait(None) {
                    return;
                }
            }
        })
        .ok()?;
    Some(TailHandle { stop, _wake: wr })
}

fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: pipe fills two fds we then own.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: both are fresh fds owned by nobody else.
    unsafe { Some((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

/// A watch on some files and folders, kept between waits so a change
/// that lands while the caller reads is still seen by the next wait.
pub struct Watch {
    paths: Vec<PathBuf>,
    wake: Option<RawFd>,
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    kq: RawFd,
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    fds: Vec<RawFd>,
    /// Some path wasn't there or went away: open them all again.
    stale: bool,
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
impl Watch {
    pub fn new(paths: Vec<PathBuf>, wake: Option<RawFd>) -> Watch {
        // SAFETY: a fresh kqueue we own; closed in drop.
        let kq = unsafe { libc::kqueue() };
        let mut w = Watch {
            paths,
            wake,
            kq,
            fds: Vec::new(),
            stale: false,
        };
        if kq >= 0
            && let Some(f) = wake
        {
            w.register(f, libc::EVFILT_READ, libc::EV_ADD, 0);
        }
        w.arm();
        w
    }

    /// Watches these paths from now on; a no-op when they're the same.
    pub fn set_paths(&mut self, paths: Vec<PathBuf>) {
        if paths != self.paths {
            self.paths = paths;
            self.arm();
        }
    }

    fn register(&self, ident: RawFd, filter: i16, flags: u16, fflags: u32) {
        let ev = libc::kevent {
            ident: ident as usize,
            filter,
            flags,
            fflags,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one valid change, no events out.
        unsafe { libc::kevent(self.kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    }

    /// Opens and registers every path, dropping the old ones.
    fn arm(&mut self) {
        use std::os::unix::ffi::OsStrExt;
        for fd in self.fds.drain(..) {
            // SAFETY: an fd this watch opened; closing it drops its events.
            unsafe { libc::close(fd) };
        }
        self.stale = false;
        if self.kq < 0 {
            return;
        }
        for i in 0..self.paths.len() {
            let Ok(c) = std::ffi::CString::new(self.paths[i].as_os_str().as_bytes()) else {
                continue;
            };
            #[cfg(target_os = "macos")]
            let flags = libc::O_EVTONLY;
            #[cfg(not(target_os = "macos"))]
            let flags = libc::O_RDONLY;
            // SAFETY: a valid C path.
            let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_CLOEXEC) };
            if fd < 0 {
                self.stale = true;
                continue;
            }
            self.fds.push(fd);
            self.register(
                fd,
                libc::EVFILT_VNODE,
                libc::EV_ADD | libc::EV_CLEAR,
                libc::NOTE_WRITE
                    | libc::NOTE_EXTEND
                    | libc::NOTE_DELETE
                    | libc::NOTE_RENAME
                    | libc::NOTE_ATTRIB,
            );
        }
    }

    /// Blocks until a path changes, `wake` becomes readable (its writer
    /// closed), or `timeout` passes. False when woken by `wake`.
    pub fn wait(&mut self, timeout: Option<Duration>) -> bool {
        if self.kq < 0 {
            std::thread::sleep(POLL);
            return true;
        }
        // A path that couldn't be opened (not there yet): look again later.
        let timeout = match timeout {
            None if self.stale => Some(POLL),
            t => t,
        };
        let ts = timeout.map(|d| libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        });
        // SAFETY: zeroed kevents are valid out-buffers.
        let mut out: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
        // SAFETY: valid kqueue and buffer.
        let n = unsafe {
            libc::kevent(
                self.kq,
                std::ptr::null(),
                0,
                out.as_mut_ptr(),
                out.len() as i32,
                ts.as_ref().map_or(std::ptr::null(), |t| t as *const _),
            )
        };
        let got = &out[..n.max(0) as usize];
        let woken = self.wake.is_some_and(|w| {
            got.iter()
                .any(|e| e.filter == libc::EVFILT_READ && e.ident == w as usize)
        });
        // A path deleted or renamed: open it afresh before the caller reads.
        if self.stale
            || got.iter().any(|e| {
                e.filter == libc::EVFILT_VNODE
                    && e.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME) != 0
            })
        {
            self.arm();
        }
        !woken
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: fds this watch opened.
        unsafe {
            for fd in self.fds.drain(..) {
                libc::close(fd);
            }
            if self.kq >= 0 {
                libc::close(self.kq);
            }
        }
    }
}

/// Elsewhere: a slow poll, woken early only by `wake`.
#[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
impl Watch {
    pub fn new(paths: Vec<PathBuf>, wake: Option<RawFd>) -> Watch {
        Watch {
            paths,
            wake,
            stale: false,
        }
    }

    pub fn set_paths(&mut self, paths: Vec<PathBuf>) {
        self.paths = paths;
    }

    pub fn wait(&mut self, timeout: Option<Duration>) -> bool {
        let _ = self.stale;
        let ms = timeout.unwrap_or(POLL).max(POLL).as_millis() as i32;
        match self.wake {
            Some(w) => {
                let mut p = libc::pollfd {
                    fd: w,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: one valid pollfd.
                let n = unsafe { libc::poll(&mut p, 1, ms) };
                !(n > 0 && p.revents != 0)
            }
            None => {
                std::thread::sleep(Duration::from_millis(ms as u64));
                true
            }
        }
    }
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

    #[test]
    fn wait_wakes_on_a_write_and_on_close() {
        let t = std::env::temp_dir().join(format!("rush-ui-wait-{}", std::process::id()));
        std::fs::write(&t, b"a").unwrap();
        // A write made before the wait starts is still seen.
        let mut w = Watch::new(vec![t.clone()], None);
        {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&t)
                .unwrap()
                .write_all(b"b")
                .unwrap();
        }
        assert!(w.wait(Some(Duration::from_secs(30))));
        let (rd, wr) = pipe().unwrap();
        drop(wr);
        assert!(!Watch::new(vec![t.clone()], Some(rd.as_raw_fd())).wait(None));
        std::fs::remove_file(&t).unwrap();
    }
}
