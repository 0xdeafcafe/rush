//! inotify: the kernel says when a file is written, as kqueue does on
//! macOS. One inotify descriptor holds every watch, and poll(2) waits on
//! it and on a [`Stop`]'s pipe together, so a quiet wait wakes for
//! nothing.
//!
//! inotify tells a folder of writes to the files in it, which kqueue
//! doesn't: those are passed by, so a folder is told only when an entry
//! is added, removed or renamed, or its own attributes change. A path
//! renamed over keeps its watch on the inode that was there, so each
//! watched path's inode is kept and looked at again on its events.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{File, Stop, changed, poll};

/// What every watch asks for: a file written or its attributes changed,
/// a folder's entries added, removed or renamed, the path itself removed
/// or renamed. The same for all, so a path watched twice (as asked, and
/// as a missing path's stand-in) keeps one mask.
const MASK: u32 = libc::IN_MODIFY
    | libc::IN_ATTRIB
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF;

/// A folder's events about its entries that kqueue would tell it of.
const ENTRIES: u32 = libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO;

/// The watched path itself is gone or elsewhere.
const GONE: u32 = libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED;

fn inotify() -> Option<OwnedFd> {
    // SAFETY: inotify_init1 takes flags; a non-negative result is ours.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    // SAFETY: as above.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The device and inode a path names now, to tell it replaced.
fn identity(p: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

/// Watches p on fd: its watch descriptor and what it names; None when
/// it isn't there (or the kernel won't).
fn add_watch(fd: RawFd, p: &Path, mask: u32) -> Option<(i32, (u64, u64))> {
    let c = CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: fd is an inotify descriptor; c is a NUL-terminated path.
    let wd = unsafe { libc::inotify_add_watch(fd, c.as_ptr(), mask) };
    if wd < 0 {
        return None;
    }
    let id = identity(p)?;
    Some((wd, id))
}

/// One event read: its watch, what happened, and whether it names an
/// entry of a watched folder (rather than the watched path itself).
#[derive(Clone, Copy)]
struct Event {
    wd: i32,
    mask: u32,
    entry: bool,
}

/// Reads every event waiting on fd, without blocking; an error but for
/// none waiting.
fn drain(fd: RawFd, out: &mut Vec<Event>) -> std::io::Result<()> {
    const HEAD: usize = std::mem::size_of::<libc::inotify_event>();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        // SAFETY: buf is valid for its length.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            return match e.kind() {
                std::io::ErrorKind::WouldBlock => Ok(()),
                std::io::ErrorKind::Interrupted => continue,
                _ => Err(e),
            };
        }
        if n == 0 {
            return Ok(());
        }
        let mut b = &buf[..n as usize];
        while b.len() >= HEAD {
            // SAFETY: the kernel wrote a whole inotify_event here; it may
            // sit unaligned in buf.
            let ev = unsafe { std::ptr::read_unaligned(b.as_ptr().cast::<libc::inotify_event>()) };
            out.push(Event {
                wd: ev.wd,
                mask: ev.mask,
                entry: ev.len > 0,
            });
            b = &b[(HEAD + ev.len as usize).min(b.len())..];
        }
    }
}

/// Waits for fd to turn readable or stop to close, up to timeout (forever
/// when None); whether fd is readable.
fn wait_readable(
    fd: RawFd,
    stop: Option<&Stop>,
    timeout: Option<Duration>,
) -> std::io::Result<bool> {
    let mut fds = vec![libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    if let Some(s) = stop {
        fds.push(libc::pollfd {
            fd: s.r.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
    }
    let ms = timeout.map_or(-1, |d| {
        // Rounded up: a wait never ends before its time.
        d.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32
    });
    // SAFETY: fds is valid for its length.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fds[0].revents != 0)
}

fn eintr(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Interrupted
}

/// Blocks until one of files differs in size from what was read, or stop
/// closes; it reports which. The kernel says when a file is written, so a
/// quiet transcript costs nothing while it's watched.
pub fn grown(stop: &Stop, files: &[File]) -> bool {
    let Some(ino) = inotify() else {
        return poll(stop, files, Duration::from_millis(25));
    };
    let mask = libc::IN_MODIFY | libc::IN_ATTRIB | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF;
    let mut ids = Vec::new();
    for f in files {
        // Not there yet: the checks below still see it appear.
        if let Some((_, id)) = add_watch(ino.as_raw_fd(), &f.path, mask) {
            ids.push((f.path.as_path(), id));
        }
    }
    if ids.is_empty() {
        return poll(stop, files, Duration::from_millis(250));
    }
    #[cfg(test)]
    super::watched::told(files);
    // Written between being read and being watched.
    if changed(files) {
        return true;
    }
    let mut events = Vec::new();
    loop {
        match wait_readable(ino.as_raw_fd(), Some(stop), None) {
            Ok(_) => {}
            Err(e) if eintr(&e) => continue,
            Err(_) => return poll(stop, files, Duration::from_millis(25)),
        }
        if stop.is_closed() {
            return false;
        }
        events.clear();
        if drain(ino.as_raw_fd(), &mut events).is_err() {
            return poll(stop, files, Duration::from_millis(25));
        }
        if events.is_empty() {
            continue;
        }
        if changed(files) {
            return true;
        }
        // Replaced: whoever reads it starts over.
        if events.iter().any(|e| e.mask & GONE != 0)
            || ids.iter().any(|(p, id)| identity(p) != Some(*id))
        {
            return true;
        }
    }
}

/// How long a wait the kernel can't serve lasts, when given no timeout.
const SLOW: Duration = Duration::from_secs(2);

/// One inotify watch: the paths watched through it as asked (one, unless
/// two spellings name the same file), and the folder it stands in as for
/// missing paths.
#[derive(Default)]
struct Wd {
    paths: Vec<PathBuf>,
    standin: bool,
}

/// Notices writes to a set of files and folders: a file written to, grown,
/// replaced or removed, or a folder gaining, losing or renaming an entry.
/// The kernel queues what happened, so asking costs one system call and
/// nothing is missed between asks.
pub struct Watcher {
    inner: Mutex<Inner>,
}

pub(crate) struct Inner {
    // Shared with a wait in progress, which runs without the lock.
    ino: Option<Arc<OwnedFd>>,
    /// Each path watched, its watch, and the file it named when watched.
    pub(crate) fds: HashMap<PathBuf, (i32, (u64, u64))>,
    wds: HashMap<i32, Wd>,
    /// Watched since the last ask: told as changed, once.
    fresh: HashSet<PathBuf>,
    /// It can't tell what changed since the last ask.
    dirty: bool,
    /// Paths asked for that aren't there yet: each waits on the nearest
    /// folder above it that is (a stand-in), and is watched once a change
    /// there shows it has appeared. Nothing is tried on a timer.
    pub(crate) missing: HashSet<PathBuf>,
    /// The stand-ins' watches by folder.
    standins: HashMap<PathBuf, i32>,
}

impl Inner {
    /// Notes what the kernel said: the paths changed, and those removed
    /// or replaced no longer watched; a stand-in's folder changed, the
    /// paths waiting on it tried again. Whether a path asked for changed
    /// (or appeared).
    fn took(&mut self, events: &[Event]) -> bool {
        let mut told = false;
        let mut retry = false;
        for ev in events {
            if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                // Events were lost: everything may have changed.
                self.dirty = true;
                told = true;
                retry = true;
                continue;
            }
            let Some(w) = self.wds.get(&ev.wd) else {
                continue;
            };
            if w.standin && (ev.mask & (ENTRIES | GONE) != 0) {
                retry = true;
            }
            // A file written in a watched folder: not the folder's news.
            if ev.entry && ev.mask & ENTRIES == 0 {
                continue;
            }
            let paths = w.paths.clone();
            for p in paths {
                let replaced = !ev.entry
                    && (ev.mask & GONE != 0
                        || self
                            .fds
                            .get(&p)
                            .is_some_and(|(_, id)| identity(&p) != Some(*id)));
                if replaced {
                    self.drop_path(&p); // gone or replaced: watch whatever is there now
                }
                self.fresh.insert(p);
                told = true;
            }
            if ev.mask & libc::IN_IGNORED != 0 {
                // The kernel ended the watch (its file is gone).
                if let Some(w) = self.wds.remove(&ev.wd)
                    && w.standin
                {
                    self.standins.retain(|_, wd| *wd != ev.wd);
                    retry = true;
                }
            }
        }
        if retry {
            let before = self.fresh.len();
            let missing: Vec<PathBuf> = self.missing.iter().cloned().collect();
            for p in missing {
                if self.open(&p) {
                    self.missing.remove(&p);
                }
            }
            told |= self.fresh.len() > before;
            self.stand_in();
        }
        told
    }

    /// Watches p, told as changed at the next ask; false when it isn't
    /// there (or the kernel won't).
    fn open(&mut self, p: &Path) -> bool {
        let Some(ino) = self.ino.as_ref().map(|i| i.as_raw_fd()) else {
            return false;
        };
        let Some((wd, id)) = add_watch(ino, p, MASK) else {
            return false;
        };
        self.wds.entry(wd).or_default().paths.push(p.to_path_buf());
        self.fds.insert(p.to_path_buf(), (wd, id));
        self.fresh.insert(p.to_path_buf());
        true
    }

    /// Ends wd's watch once nothing is watched through it.
    fn release(&mut self, wd: i32) {
        let Some(w) = self.wds.get(&wd) else { return };
        if !w.paths.is_empty() || w.standin {
            return;
        }
        self.wds.remove(&wd);
        if let Some(ino) = &self.ino {
            // SAFETY: plain integers; a watch the kernel ended already
            // only fails (EINVAL).
            unsafe { libc::inotify_rm_watch(ino.as_raw_fd(), wd) };
        }
    }

    /// Watches the nearest folder there is above each missing path, and
    /// no other.
    fn stand_in(&mut self) {
        let Some(ino) = self.ino.as_ref().map(|i| i.as_raw_fd()) else {
            return;
        };
        let mut want: HashSet<PathBuf> = HashSet::new();
        for p in &self.missing {
            let mut a = p.parent();
            while let Some(d) = a {
                if d.as_os_str().is_empty() {
                    break;
                }
                if std::fs::metadata(d).is_ok_and(|m| m.is_dir()) {
                    want.insert(d.to_path_buf());
                    break;
                }
                a = d.parent();
            }
        }
        let gone: Vec<(PathBuf, i32)> = self
            .standins
            .iter()
            .filter(|(d, _)| !want.contains(*d))
            .map(|(d, wd)| (d.clone(), *wd))
            .collect();
        for (d, wd) in gone {
            self.standins.remove(&d);
            if let Some(w) = self.wds.get_mut(&wd) {
                w.standin = false;
            }
            self.release(wd);
        }
        for d in want {
            if self.standins.contains_key(&d) {
                continue;
            }
            if let Some((wd, _)) = add_watch(ino, &d, MASK) {
                self.wds.entry(wd).or_default().standin = true;
                self.standins.insert(d, wd);
            }
        }
    }

    /// Stops watching p.
    fn drop_path(&mut self, p: &Path) {
        let Some((wd, _)) = self.fds.remove(p) else {
            return;
        };
        if let Some(w) = self.wds.get_mut(&wd) {
            w.paths.retain(|x| x != p);
        }
        self.release(wd);
    }

    /// Reads what the kernel queued; whether a path asked for changed.
    /// An error from the kernel counts as everything changed.
    fn read(&mut self) -> bool {
        let Some(ino) = self.ino.clone() else {
            return true;
        };
        let mut events = Vec::new();
        let failed = drain(ino.as_raw_fd(), &mut events).is_err();
        if failed {
            self.dirty = true;
        }
        self.took(&events) || failed
    }
}

impl Default for Watcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Watcher {
    /// A watcher of nothing yet; one that can't be made reports every ask
    /// as a change.
    pub fn new() -> Watcher {
        Watcher {
            inner: Mutex::new(Inner {
                ino: inotify().map(Arc::new),
                fds: HashMap::new(),
                wds: HashMap::new(),
                fresh: HashSet::new(),
                dirty: true,
                missing: HashSet::new(),
                standins: HashMap::new(),
            }),
        }
    }

    /// A watcher of many paths. inotify costs a watch per path and no
    /// descriptor, so this is a plain watcher.
    pub fn many() -> Watcher {
        Watcher::new()
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    /// Makes paths the set watched, keeping those already watched. One that
    /// isn't there yet waits on the nearest folder above it that is, and
    /// is watched (and told as changed) once that folder's change shows it
    /// has appeared. One newly watched is told as changed at the next ask:
    /// it may have changed between being read and being watched.
    pub fn watch<P: AsRef<Path>>(&self, paths: &[P]) {
        let mut w = self.inner.lock().unwrap();
        if w.ino.is_none() {
            return;
        }
        let want: HashSet<&Path> = paths.iter().map(|p| p.as_ref()).collect();
        let gone: Vec<PathBuf> = w
            .fds
            .keys()
            .filter(|p| !want.contains(p.as_path()))
            .cloned()
            .collect();
        for p in gone {
            w.drop_path(&p);
        }
        let mut missing = HashSet::new();
        for p in want {
            if !w.fds.contains_key(p) && !w.open(p) {
                missing.insert(p.to_path_buf());
            }
        }
        if missing != w.missing {
            w.missing = missing;
            w.stand_in();
        }
    }

    /// Whether path is watched: a change to it is told at an ask.
    pub fn watching(&self, path: impl AsRef<Path>) -> bool {
        self.inner.lock().unwrap().fds.contains_key(path.as_ref())
    }

    /// Whether anything watched changed since the last ask.
    pub fn changed(&self) -> bool {
        let (paths, all) = self.changes();
        all || !paths.is_empty()
    }

    /// The watched paths that changed since the last ask; all (true) when
    /// it can't tell which (the first ask, or the kernel failed it). A path
    /// removed or replaced is told, and no longer watched: watch it again
    /// to follow what's there now.
    pub fn changes(&self) -> (HashSet<PathBuf>, bool) {
        let mut w = self.inner.lock().unwrap();
        if w.ino.is_none() {
            return (HashSet::new(), true);
        }
        w.read();
        let all = std::mem::replace(&mut w.dirty, false);
        (std::mem::take(&mut w.fresh), all)
    }

    /// Blocks until something watched changes, stop (when given) closes,
    /// or timeout passes; false when stop closed. What changed is told at
    /// the next ask. Watch before reading what's watched: a change from
    /// then on wakes the wait, even one made before it starts. A path
    /// that wasn't there wakes it once it appears; a change to the folder
    /// it waits on that doesn't bring it is slept through.
    pub fn wait(&self, stop: Option<&Stop>, timeout: Option<Duration>) -> bool {
        let ino = {
            let mut w = self.inner.lock().unwrap();
            // What it couldn't tell, and paths newly watched: the caller
            // has read them since it watched.
            w.dirty = false;
            w.fresh.clear();
            w.ino.clone()
        };
        // Without the kernel's word: a slow poll.
        let slow = || match stop {
            Some(s) => !s.wait(timeout.unwrap_or(SLOW)),
            None => {
                std::thread::sleep(timeout.unwrap_or(SLOW));
                true
            }
        };
        let Some(ino) = ino else {
            return slow();
        };
        if stop.is_some_and(|s| s.is_closed()) {
            return false;
        }
        let until = timeout.map(|t| Instant::now() + t);
        loop {
            let left = until.map(|u| u.saturating_duration_since(Instant::now()));
            let ready = match wait_readable(ino.as_raw_fd(), stop, left) {
                Ok(r) => r,
                Err(e) if eintr(&e) => continue,
                Err(_) => return slow(),
            };
            if stop.is_some_and(|s| s.is_closed()) {
                return false;
            }
            // Only a stand-in's folder changed, or a file in a watched
            // folder was written, and nothing appeared: sleep on.
            let quiet = ready && !self.inner.lock().unwrap().read();
            if !quiet || left.is_some_and(|l| l.is_zero()) {
                return true;
            }
        }
    }

    /// Stops watching.
    pub fn close(&self) {
        let mut w = self.inner.lock().unwrap();
        w.fds.clear();
        w.wds.clear();
        w.missing.clear();
        w.standins.clear();
        w.ino = None;
    }
}
