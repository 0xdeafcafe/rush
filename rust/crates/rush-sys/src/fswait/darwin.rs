//! kqueue: the kernel says when a file is written.
//!
//! Ported from internal/fswait/fswait_darwin.go:1-64 and
//! watch_darwin.go:1-160.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{File, Stop, changed, poll};

const VNODE_FLAGS: u32 =
    libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_DELETE | libc::NOTE_RENAME;

fn kqueue() -> Option<OwnedFd> {
    // SAFETY: kqueue takes nothing; a non-negative result is ours to own.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return None;
    }
    // SAFETY: kq is a descriptor we just made.
    unsafe { libc::fcntl(kq, libc::F_SETFD, libc::FD_CLOEXEC) };
    // SAFETY: as above.
    Some(unsafe { OwnedFd::from_raw_fd(kq) })
}

/// Opens path for events only (it doesn't keep a volume from unmounting).
fn open_evtonly(path: &Path) -> Option<OwnedFd> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: c is a NUL-terminated path.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
    // SAFETY: a non-negative fd is ours to own.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

fn kevent_of(ident: RawFd, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    // SAFETY: kevent is plain data.
    let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
    ev.ident = ident as usize;
    ev.filter = filter;
    ev.flags = flags;
    ev.fflags = fflags;
    ev
}

/// kevent(2): registers changes and/or reads up to events.len() events,
/// waiting up to timeout (forever when None).
fn kevent(
    kq: &OwnedFd,
    changes: &[libc::kevent],
    events: &mut [libc::kevent],
    timeout: Option<Duration>,
) -> std::io::Result<usize> {
    let ts = timeout.map(|d| libc::timespec {
        tv_sec: d.as_secs() as libc::time_t,
        tv_nsec: d.subsec_nanos() as libc::c_long,
    });
    // SAFETY: the slices are valid for their lengths; ts lives across the
    // call.
    let n = unsafe {
        libc::kevent(
            kq.as_raw_fd(),
            changes.as_ptr(),
            changes.len() as i32,
            events.as_mut_ptr(),
            events.len() as i32,
            ts.as_ref().map_or(std::ptr::null(), |t| t as *const _),
        )
    };
    if n < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn eintr(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::EINTR)
}

/// Blocks until one of files differs in size from what was read, or stop
/// closes; it reports which. The kernel says when a file is written, so a
/// quiet transcript costs nothing while it's watched.
pub fn grown(stop: &Stop, files: &[File]) -> bool {
    let Some(kq) = kqueue() else {
        return poll(stop, files, Duration::from_millis(25));
    };
    let mut fds = Vec::new();
    let mut changes = Vec::new();
    for f in files {
        let Some(fd) = open_evtonly(&f.path) else {
            continue; // not there yet: the checks below still see it appear
        };
        changes.push(kevent_of(
            fd.as_raw_fd(),
            libc::EVFILT_VNODE,
            libc::EV_ADD | libc::EV_CLEAR,
            VNODE_FLAGS,
        ));
        fds.push(fd);
    }
    if changes.is_empty() {
        return poll(stop, files, Duration::from_millis(250));
    }
    // Stop is in the same queue: its pipe reads as ended once it closes.
    let stop_fd = stop.r.as_raw_fd();
    changes.push(kevent_of(stop_fd, libc::EVFILT_READ, libc::EV_ADD, 0));
    if kevent(&kq, &changes, &mut [], None).is_err() {
        return poll(stop, files, Duration::from_millis(25));
    }
    #[cfg(test)]
    watched::told(files);
    // Written between being read and being watched.
    if changed(files) {
        return true;
    }
    // SAFETY: kevent is plain data.
    let mut events = vec![unsafe { std::mem::zeroed::<libc::kevent>() }; changes.len()];
    loop {
        let n = match kevent(&kq, &[], &mut events, None) {
            Ok(n) => n,
            Err(e) if eintr(&e) => 0,
            Err(_) => return poll(stop, files, Duration::from_millis(25)),
        };
        if stop.is_closed() {
            return false;
        }
        if n > 0 && changed(files) {
            return true;
        }
        for ev in &events[..n] {
            if ev.filter == libc::EVFILT_VNODE
                && ev.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME) != 0
            {
                return true; // replaced: whoever reads it starts over
            }
        }
    }
}

/// How long a path that wasn't there goes untried: unwatched, it's only
/// looked at as it would have been anyway.
pub(crate) const RETRY_MISSING: Duration = Duration::from_secs(5);

/// How long a wait the kernel can't serve lasts, when given no timeout.
const SLOW: Duration = Duration::from_secs(2);

/// Notices writes to a set of files and folders: a file written to, grown,
/// replaced or removed, or a folder gaining, losing or renaming an entry.
/// The kernel queues what happened, so asking costs one system call and
/// nothing is missed between asks.
pub struct Watcher {
    inner: Mutex<Inner>,
}

pub(crate) struct Inner {
    // Shared with a wait in progress, which runs without the lock.
    kq: Option<Arc<OwnedFd>>,
    fds: HashMap<PathBuf, OwnedFd>,
    /// fds' paths, to name what an event was on.
    paths: HashMap<RawFd, PathBuf>,
    /// Watched since the last ask: told as changed, once.
    fresh: HashSet<PathBuf>,
    /// It can't tell what changed since the last ask.
    dirty: bool,
    /// When each path that couldn't be opened was last tried: most are
    /// folders not made yet, tried again only after RETRY_MISSING.
    pub(crate) missed: HashMap<PathBuf, Instant>,
}

impl Inner {
    /// Notes what the kernel said: the paths changed, and those removed
    /// or replaced no longer watched.
    fn took(&mut self, events: &[libc::kevent]) {
        for ev in events {
            if ev.filter != libc::EVFILT_VNODE {
                continue;
            }
            let fd = ev.ident as RawFd;
            let Some(p) = self.paths.get(&fd).cloned() else {
                continue;
            };
            if ev.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME) != 0 {
                self.drop_path(&p); // gone or replaced: watch whatever is there now
            }
            self.fresh.insert(p);
        }
    }

    /// Stops watching p; closing its fd drops its registration.
    fn drop_path(&mut self, p: &Path) {
        if let Some(fd) = self.fds.remove(p) {
            self.paths.remove(&fd.as_raw_fd());
        }
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
                kq: kqueue().map(Arc::new),
                fds: HashMap::new(),
                paths: HashMap::new(),
                fresh: HashSet::new(),
                dirty: true,
                missed: HashMap::new(),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    /// Makes paths the set watched, keeping those already watched. One that
    /// isn't there yet is skipped: watch its folder to see it arrive. One
    /// newly watched is told as changed at the next ask: it may have
    /// changed between being read and being watched.
    pub fn watch<P: AsRef<Path>>(&self, paths: &[P]) {
        let mut w = self.inner.lock().unwrap();
        let Some(kq) = w.kq.as_ref().map(|k| k.as_raw_fd()) else {
            return;
        };
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
        w.missed.retain(|p, _| want.contains(p.as_path()));
        let now = Instant::now();
        for p in want {
            if w.fds.contains_key(p) {
                continue;
            }
            if w.missed
                .get(p)
                .is_some_and(|at| now.duration_since(*at) < RETRY_MISSING)
            {
                continue;
            }
            let Some(fd) = open_evtonly(p) else {
                w.missed.insert(p.to_path_buf(), now);
                continue;
            };
            w.missed.remove(p);
            let ev = kevent_of(
                fd.as_raw_fd(),
                libc::EVFILT_VNODE,
                libc::EV_ADD | libc::EV_CLEAR,
                VNODE_FLAGS | libc::NOTE_ATTRIB,
            );
            // SAFETY: kq stays open while the lock is held; ev is one valid
            // change and no events are read.
            let n = unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
            if n < 0 {
                continue; // not watched: never Watching (fd closes as it drops)
            }
            w.paths.insert(fd.as_raw_fd(), p.to_path_buf());
            w.fds.insert(p.to_path_buf(), fd);
            w.fresh.insert(p.to_path_buf());
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
        let Some(kq) = w.kq.clone() else {
            return (HashSet::new(), true);
        };
        let all = std::mem::replace(&mut w.dirty, false);
        // SAFETY: kevent is plain data.
        let mut events = [unsafe { std::mem::zeroed::<libc::kevent>() }; 32];
        let res = loop {
            let n = match kevent(&kq, &[], &mut events, Some(Duration::ZERO)) {
                Ok(n) => n,
                Err(e) if eintr(&e) => 0,
                Err(_) => break true,
            };
            w.took(&events[..n]);
            if n < events.len() {
                break all;
            }
        };
        (std::mem::take(&mut w.fresh), res)
    }

    /// Blocks until something watched changes, stop (when given) closes,
    /// or timeout passes; false when stop closed. What changed is told at
    /// the next ask. Watch before reading what's watched: a change from
    /// then on wakes the wait, even one made before it starts. Paths that
    /// weren't there are tried again only by watch: while there are some,
    /// a wait lasts at most RETRY_MISSING, so the caller watches again.
    pub fn wait(&self, stop: Option<&Stop>, timeout: Option<Duration>) -> bool {
        let (kq, timeout) = {
            let mut w = self.inner.lock().unwrap();
            // What it couldn't tell, and paths newly watched: the caller
            // has read them since it watched.
            w.dirty = false;
            w.fresh.clear();
            let t = match (timeout, w.missed.is_empty()) {
                (t, true) => t,
                (None, false) => Some(RETRY_MISSING),
                (Some(t), false) => Some(t.min(RETRY_MISSING)),
            };
            (w.kq.clone(), t)
        };
        // Without the kernel's word: a slow poll.
        let slow = || match stop {
            Some(s) => !s.wait(timeout.unwrap_or(SLOW)),
            None => {
                std::thread::sleep(timeout.unwrap_or(SLOW));
                true
            }
        };
        let Some(kq) = kq else {
            return slow();
        };
        if let Some(s) = stop {
            if s.is_closed() {
                return false;
            }
            // Its pipe reads as ended once it closes. Adding it again is a
            // no-op; a Stop dropped takes its registration with its fd.
            let ev = kevent_of(s.r.as_raw_fd(), libc::EVFILT_READ, libc::EV_ADD, 0);
            if kevent(&kq, &[ev], &mut [], None).is_err() {
                return slow();
            }
        }
        // SAFETY: kevent is plain data.
        let mut events = [unsafe { std::mem::zeroed::<libc::kevent>() }; 32];
        let n = loop {
            match kevent(&kq, &[], &mut events, timeout) {
                Ok(n) => break n,
                Err(e) if eintr(&e) => continue,
                Err(_) => return slow(),
            }
        };
        self.inner.lock().unwrap().took(&events[..n]);
        !stop.is_some_and(|s| s.is_closed())
    }

    /// Stops watching.
    pub fn close(&self) {
        let mut w = self.inner.lock().unwrap();
        w.fds.clear();
        w.paths.clear();
        w.kq = None;
    }
}

/// Tests wait here until a grown call watches their file, so what they do
/// next lands after the watch starts.
#[cfg(test)]
pub(crate) mod watched {
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex};

    static W: (Mutex<Vec<PathBuf>>, Condvar) = (Mutex::new(Vec::new()), Condvar::new());

    pub(crate) fn told(files: &[super::File]) {
        W.0.lock()
            .unwrap()
            .extend(files.iter().map(|f| f.path.clone()));
        W.1.notify_all();
    }

    /// Blocks until a grown call has watched p.
    pub(crate) fn wait(p: &Path) {
        let g = W.0.lock().unwrap();
        let _g = W.1.wait_while(g, |w| !w.iter().any(|x| x == p)).unwrap();
    }
}
