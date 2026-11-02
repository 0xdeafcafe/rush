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

use super::tree::{Group, Groups, Seen, Shared, Stream, is_wake};
use super::{File, Stop, changed, poll};

/// How many paths in one folder a [`Watcher::many`] watches through its
/// FSEvents stream rather than a descriptor each.
const MANY: usize = 8;
/// In such a folder, a path new or written in within this long keeps a
/// descriptor: a live session's folder is told at once, as before; the
/// hundreds quiet for longer share the stream.
const RECENT: Duration = Duration::from_secs(3600);

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

pub(super) fn kevent_of(ident: RawFd, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
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

/// How long a wait the kernel can't serve lasts, when given no timeout.
const SLOW: Duration = Duration::from_secs(2);

/// Adds fd's vnode events to kq; whether the kernel took it.
fn register(kq: RawFd, fd: &OwnedFd) -> bool {
    let ev = kevent_of(
        fd.as_raw_fd(),
        libc::EVFILT_VNODE,
        libc::EV_ADD | libc::EV_CLEAR,
        VNODE_FLAGS | libc::NOTE_ATTRIB,
    );
    // SAFETY: kq stays open while the caller holds the lock; ev is one
    // valid change and no events are read.
    unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) >= 0 }
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
    kq: Option<Arc<OwnedFd>>,
    pub(crate) fds: HashMap<PathBuf, OwnedFd>,
    /// fds' paths, to name what an event was on.
    paths: HashMap<RawFd, PathBuf>,
    /// Watched since the last ask: told as changed, once.
    fresh: HashSet<PathBuf>,
    /// It can't tell what changed since the last ask.
    dirty: bool,
    /// Paths asked for that aren't there yet: each waits on the nearest
    /// folder above it that is (a stand-in), and is watched once a change
    /// there shows it has appeared. Nothing is tried on a timer.
    pub(crate) missing: HashSet<PathBuf>,
    /// The stand-ins' fds by folder, and their folders by fd.
    standins: HashMap<PathBuf, OwnedFd>,
    standin_of: HashMap<RawFd, PathBuf>,
    /// Watches folders with many paths in them through FSEvents.
    many: bool,
    /// What the stream tells, the stream, and the real paths it's on.
    tree: Option<Arc<Shared>>,
    pub(crate) stream: Option<Stream>,
    pub(crate) streamed: Vec<PathBuf>,
    /// The folders of many, as given, and their real paths.
    reals: HashMap<PathBuf, PathBuf>,
    /// A path in such a folder new or written in this lately keeps a
    /// descriptor (kqueue tells at once; the stream a few ms later)...
    pub(crate) recent: Duration,
    /// ...from when each was last.
    hot: HashMap<PathBuf, Instant>,
    /// Moved to the stream, their descriptors kept till the next ask has
    /// read what they told: nothing between is missed.
    retire: HashSet<PathBuf>,
}

impl Inner {
    /// Notes what the kernel said: the paths changed, and those removed
    /// or replaced no longer watched; a stand-in's folder changed, the
    /// paths waiting on it tried again. Whether a path asked for changed
    /// (or appeared).
    fn took(&mut self, events: &[libc::kevent]) -> bool {
        let mut told = false;
        let mut retry = false;
        for ev in events {
            if is_wake(ev) {
                told |= self.take_tree();
                continue;
            }
            if ev.filter != libc::EVFILT_VNODE {
                continue;
            }
            let fd = ev.ident as RawFd;
            if self.standin_of.contains_key(&fd) {
                retry = true;
                continue;
            }
            let Some(p) = self.paths.get(&fd).cloned() else {
                continue;
            };
            if ev.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME) != 0 {
                self.drop_path(&p); // gone or replaced: watch whatever is there now
            }
            if p.parent().is_some_and(|d| self.reals.contains_key(d)) {
                self.hot.insert(p.clone(), Instant::now()); // still changing
            }
            self.fresh.insert(p);
            told = true;
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

    /// What the stream told since the last ask, kept as changed; whether
    /// there was anything (FSEvents losing events counts, as all). Those
    /// written in deep inside are given a descriptor and leave it.
    fn take_tree(&mut self) -> bool {
        let Some(t) = self.tree.clone() else {
            return false;
        };
        let (told, busy, lost) = t.take();
        if lost {
            self.dirty = true;
        }
        let any = lost || !told.is_empty();
        let now = Instant::now();
        for p in &busy {
            self.hot.insert(p.clone(), now);
        }
        self.fresh.extend(told);
        if !busy.is_empty() {
            // Watched by kqueue first, then let go by the stream: nothing
            // between is missed.
            for p in &busy {
                let told = self.fresh.contains(p);
                if !self.fds.contains_key(p) && self.open(p) && !told {
                    self.fresh.remove(p); // nothing it would tell happened
                }
            }
            let mut groups = t.groups();
            for g in groups.values_mut() {
                g.kids.retain(|name, _| !busy.contains(&g.given.join(name)));
            }
            if !self.restream(&t, groups) {
                self.give_up();
            }
        }
        any
    }

    /// Whether p is watched through the stream.
    fn in_tree(&self, p: &Path) -> bool {
        self.tree.as_ref().is_some_and(|t| t.watching(p))
    }

    /// Of want, the folders holding MANY or more, with the entries the
    /// stream is to watch in each: those there, neither new nor written in
    /// lately. The rest are left for kqueue.
    fn group(&mut self, want: &HashSet<&Path>) -> Groups {
        let mut by_dir: HashMap<&Path, Vec<&Path>> = HashMap::new();
        for p in want {
            if let (Some(d), Some(_)) = (p.parent(), p.file_name()) {
                by_dir.entry(d).or_default().push(p);
            }
        }
        by_dir.retain(|_, ps| ps.len() >= MANY);
        self.reals.retain(|d, _| by_dir.contains_key(d.as_path()));
        self.hot.retain(|_, t| t.elapsed() < self.recent);
        let mut groups = Groups::new();
        if by_dir.is_empty() {
            return groups;
        }
        if self.tree.is_none() {
            self.tree = self.kq.clone().and_then(Shared::new);
        }
        let Some(tree) = self.tree.clone() else {
            return groups;
        };
        let now = Instant::now();
        for (dir, ps) in by_dir {
            // A folder first watched: its entries all start on the stream.
            let first = !self.reals.contains_key(dir);
            if first {
                let Ok(real) = std::fs::canonicalize(dir) else {
                    continue;
                };
                self.reals.insert(dir.to_path_buf(), real);
            }
            let real = &self.reals[dir];
            let mut kids = HashMap::with_capacity(ps.len());
            for p in ps {
                let Some(name) = p.file_name() else { continue };
                if self.hot.contains_key(p) {
                    continue; // written in lately: kqueue's, told at once
                }
                if let Some(seen) = tree.seen(real, name) {
                    kids.insert(name.to_os_string(), seen);
                    continue;
                }
                if !first && !self.fds.contains_key(p) {
                    // New since (a session just made): likely live, so
                    // kqueue's for now, with no stream made again for it.
                    self.hot.insert(p.to_path_buf(), now);
                    continue;
                }
                // Not there: waits on its folder, as any missing path.
                let seen = Seen::of(p);
                if seen.there() {
                    kids.insert(name.to_os_string(), seen);
                }
            }
            groups.insert(
                real.clone(),
                Group {
                    given: dir.to_path_buf(),
                    kids,
                },
            );
        }
        groups
    }

    /// Has the stream watch groups' entries, started again only when they
    /// differ: a new stream starts before the old stops. False when
    /// FSEvents won't.
    fn restream(&mut self, tree: &Arc<Shared>, groups: Groups) -> bool {
        let mut paths: Vec<PathBuf> = groups
            .iter()
            .flat_map(|(real, g)| g.kids.keys().map(move |k| real.join(k)))
            .collect();
        paths.sort_unstable();
        if paths == self.streamed {
            tree.set(groups);
            return true;
        }
        let new = if paths.is_empty() {
            None
        } else {
            match Stream::start(tree, &paths) {
                Some(s) => Some(s),
                None => return false,
            }
        };
        tree.set(groups);
        self.stream = new; // the old stops here
        self.streamed = paths;
        true
    }

    /// FSEvents won't: every path gets a descriptor, as a plain watcher's.
    fn give_up(&mut self) {
        if let Some(t) = &self.tree {
            for g in t.groups().into_values() {
                for name in g.kids.keys() {
                    let p = g.given.join(name);
                    if !self.fds.contains_key(&p) {
                        self.open(&p);
                    }
                }
            }
        }
        self.many = false;
        self.stream = None;
        self.streamed.clear();
        self.tree = None;
        self.reals.clear();
        self.retire.clear();
        self.dirty = true;
    }

    /// Closes the descriptors of paths moved to the stream, once what
    /// they told is read.
    fn retired(&mut self) {
        for p in std::mem::take(&mut self.retire) {
            if self.in_tree(&p) {
                self.drop_path(&p);
            }
        }
    }

    /// Watches p, told as changed at the next ask; false when it isn't
    /// there (or the kernel won't).
    fn open(&mut self, p: &Path) -> bool {
        let Some(kq) = self.kq.as_ref().map(|k| k.as_raw_fd()) else {
            return false;
        };
        let Some(fd) = open_evtonly(p) else {
            return false;
        };
        if !register(kq, &fd) {
            return false; // not watched (fd closes as it drops)
        }
        self.paths.insert(fd.as_raw_fd(), p.to_path_buf());
        self.fds.insert(p.to_path_buf(), fd);
        self.fresh.insert(p.to_path_buf());
        true
    }

    /// Watches the nearest folder there is above each missing path, and
    /// no other.
    fn stand_in(&mut self) {
        let Some(kq) = self.kq.as_ref().map(|k| k.as_raw_fd()) else {
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
        self.standins.retain(|d, fd| {
            let keep = want.contains(d);
            if !keep {
                self.standin_of.remove(&fd.as_raw_fd());
            }
            keep
        });
        for d in want {
            if self.standins.contains_key(&d) {
                continue;
            }
            let Some(fd) = open_evtonly(&d) else { continue };
            if register(kq, &fd) {
                self.standin_of.insert(fd.as_raw_fd(), d.clone());
                self.standins.insert(d, fd);
            }
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
                missing: HashSet::new(),
                standins: HashMap::new(),
                standin_of: HashMap::new(),
                many: false,
                tree: None,
                stream: None,
                streamed: Vec::new(),
                reals: HashMap::new(),
                recent: RECENT,
                hot: HashMap::new(),
                retire: HashSet::new(),
            }),
        }
    }

    /// A watcher of many paths: where MANY or more share a folder, one
    /// FSEvents stream serves those of them quiet for a while, with no
    /// descriptor each. What the stream tells arrives a moment after the
    /// change (a wait wakes for it), and a path replaced stays watched.
    pub fn many() -> Watcher {
        let w = Watcher::new();
        w.inner.lock().unwrap().many = true;
        w
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
        if w.kq.is_none() {
            return;
        }
        let mut want: HashSet<&Path> = paths.iter().map(|p| p.as_ref()).collect();
        let groups = if w.many {
            w.group(&want)
        } else {
            Groups::new()
        };
        let streamed = |p: &Path| {
            let (Some(d), Some(name)) = (p.parent(), p.file_name()) else {
                return false;
            };
            w.reals
                .get(d)
                .and_then(|r| groups.get(r))
                .is_some_and(|g| g.kids.contains_key(name))
        };
        let gone: Vec<(PathBuf, bool)> = w
            .fds
            .keys()
            .filter(|p| !want.contains(p.as_path()) || streamed(p))
            .map(|p| (p.clone(), want.contains(p.as_path())))
            .collect();
        want.retain(|p| !streamed(p));
        for (p, to_stream) in gone {
            if to_stream {
                w.retire.insert(p); // closed once the next ask reads it
            } else {
                w.drop_path(&p);
            }
        }
        // Leaving the stream: watched by kqueue before the stream lets go.
        let mut missing = HashSet::new();
        for p in want {
            if !w.fds.contains_key(p) && !w.open(p) {
                missing.insert(p.to_path_buf());
            }
        }
        if let Some(t) = w.tree.clone() {
            // Moving in, a path is told changed at the next ask, as any
            // newly watched one.
            for g in groups.values() {
                for name in g.kids.keys() {
                    let p = g.given.join(name);
                    if !t.watching(&p) {
                        w.fresh.insert(p);
                    }
                }
            }
            if !w.restream(&t, groups) {
                w.give_up();
            }
        }
        if missing != w.missing {
            w.missing = missing;
            w.stand_in();
        }
    }

    /// Whether path is watched: a change to it is told at an ask.
    pub fn watching(&self, path: impl AsRef<Path>) -> bool {
        let w = self.inner.lock().unwrap();
        w.fds.contains_key(path.as_ref()) || w.in_tree(path.as_ref())
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
        // Told by the stream with its wake still queued, or lost.
        w.take_tree();
        let res = res || std::mem::replace(&mut w.dirty, false);
        w.retired();
        (std::mem::take(&mut w.fresh), res)
    }

    /// Blocks until something watched changes, stop (when given) closes,
    /// or timeout passes; false when stop closed. What changed is told at
    /// the next ask. Watch before reading what's watched: a change from
    /// then on wakes the wait, even one made before it starts. A path
    /// that wasn't there wakes it once it appears; a change to the folder
    /// it waits on that doesn't bring it is slept through.
    pub fn wait(&self, stop: Option<&Stop>, timeout: Option<Duration>) -> bool {
        let kq = {
            let mut w = self.inner.lock().unwrap();
            // What it couldn't tell, and paths newly watched: the caller
            // has read them since it watched.
            w.dirty = false;
            w.fresh.clear();
            w.kq.clone()
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
        let until = timeout.map(|t| Instant::now() + t);
        // SAFETY: kevent is plain data.
        let mut events = [unsafe { std::mem::zeroed::<libc::kevent>() }; 32];
        loop {
            let left = until.map(|u| u.saturating_duration_since(Instant::now()));
            let n = loop {
                match kevent(&kq, &[], &mut events, left) {
                    Ok(n) => break n,
                    Err(e) if eintr(&e) => continue,
                    Err(_) => return slow(),
                }
            };
            let told = self.inner.lock().unwrap().took(&events[..n]);
            let stopped = stop.is_some_and(|s| s.is_closed());
            // Only a stand-in's folder changed, and nothing appeared: sleep on.
            let quiet = n > 0
                && !told
                && events[..n]
                    .iter()
                    .all(|e| e.filter == libc::EVFILT_VNODE || is_wake(e));
            if stopped || !quiet || left.is_some_and(|l| l.is_zero()) {
                return !stopped;
            }
        }
    }

    /// Stops watching.
    pub fn close(&self) {
        let mut w = self.inner.lock().unwrap();
        w.fds.clear();
        w.paths.clear();
        w.missing.clear();
        w.standins.clear();
        w.standin_of.clear();
        w.stream = None;
        w.streamed.clear();
        w.tree = None;
        w.reals.clear();
        w.retire.clear();
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
