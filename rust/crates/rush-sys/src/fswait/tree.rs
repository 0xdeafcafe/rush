//! FSEvents: one stream on the quiet entries of folders of many serves
//! them all, with no descriptor each. A [`Watcher::many`](super::Watcher::many)
//! watching a thousand session folders under one sessions folder holds
//! one stream, not a thousand kqueue descriptors.
//!
//! FSEvents tells of anything under a path, deep or not, so an entry
//! being written in (a live session's events, a project's transcripts)
//! would cost a delivery for every write: the first such write marks it
//! busy, and the watcher moves it to a kqueue descriptor of its own,
//! where writes deep inside cost nothing, and makes the stream again
//! without it. The stream is on the entries, not their folder, so that
//! leaves it quiet; the folder itself has a descriptor, as any.
//!
//! FSEvents' flags pile up for a path, so they don't say what happened:
//! each entry an event falls under is looked at (one stat) and told only
//! when it differs from what was last seen, which is what kqueue would
//! have told. A folder's status time moves when an entry is added,
//! removed or renamed, or its attributes change. A file written deep in
//! a watched folder doesn't move it, as kqueue wouldn't wake for it
//! either. Only folders are watched this way.
//!
//! The stream runs on a dispatch queue of its own; what it tells is kept
//! for the next ask and wakes a wait through the watcher's kqueue
//! (EVFILT_USER), so waiting costs nothing more than it did. It tells
//! 10-40 ms after the change (NoDefer: at once after a quiet spell).

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, OsStr, OsString, c_char, c_void};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

type CFRef = *const c_void;

#[repr(C)]
struct Context {
    version: isize,
    info: *const c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    describe: *const c_void,
}

type Callback = extern "C" fn(CFRef, *mut c_void, usize, *mut c_void, *const u32, *const u64);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        alloc: CFRef,
        cb: Callback,
        ctx: *const Context,
        paths: CFRef,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> CFRef;
    fn FSEventStreamSetDispatchQueue(s: CFRef, q: *mut c_void);
    fn FSEventStreamStart(s: CFRef) -> u8;
    fn FSEventStreamStop(s: CFRef);
    fn FSEventStreamInvalidate(s: CFRef);
    fn FSEventStreamRelease(s: CFRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithFileSystemRepresentation(alloc: CFRef, s: *const c_char) -> CFRef;
    fn CFArrayCreate(alloc: CFRef, v: *const CFRef, n: isize, cb: *const c_void) -> CFRef;
    fn CFRelease(r: CFRef);
    static kCFTypeArrayCallBacks: c_void;
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
}

const SINCE_NOW: u64 = u64::MAX;
const NO_DEFER: u32 = 0x2;
const FILE_EVENTS: u32 = 0x10;
/// What FSEvents lost or couldn't place: MustScanSubDirs, UserDropped,
/// KernelDropped, RootChanged.
const LOST: u32 = 0x1 | 0x2 | 0x4 | 0x20;
/// An entry made, removed or renamed (ItemCreated, ItemRemoved,
/// ItemRenamed): what moves its folder's status time.
const ENTRY: u32 = 0x100 | 0x200 | 0x800;
/// How long FSEvents gathers events after a delivery before the next:
/// the first after a quiet spell comes at once (NoDefer). An entry that
/// churns leaves the stream at its first write (busy), so this bounds
/// only what a burst costs.
const LATENCY: f64 = 0.02;

/// What a watched path was when last looked at; None when it isn't there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Seen(Option<(u64, i64, i64, u64, bool)>);

impl Seen {
    pub(crate) fn of(p: &Path) -> Seen {
        Seen(
            std::fs::metadata(p)
                .ok()
                .map(|m| (m.ino(), m.ctime(), m.ctime_nsec(), m.len(), m.is_dir())),
        )
    }
    pub(crate) fn there(&self) -> bool {
        self.0.is_some()
    }
}

/// A folder whose entries are watched through the stream.
#[derive(Clone)]
pub(crate) struct Group {
    /// The folder as the caller named it (FSEvents names its real path).
    pub(crate) given: PathBuf,
    pub(crate) kids: HashMap<OsString, Seen>,
}

/// The folders watched through the stream, by real path.
pub(crate) type Groups = HashMap<PathBuf, Group>;

#[derive(Default)]
struct State {
    groups: Groups,
    /// Told since the last ask.
    told: HashSet<PathBuf>,
    /// Written to deep inside since the last ask, which tells nothing
    /// but costs a delivery each time: the watcher gives them a kqueue
    /// descriptor, where it costs nothing.
    busy: HashSet<PathBuf>,
    /// FSEvents lost events: it can't tell what changed.
    lost: bool,
}

/// What the stream and the watcher share.
pub(crate) struct Shared {
    /// The watcher's kqueue: a change told wakes its waits.
    kq: Arc<OwnedFd>,
    st: Mutex<State>,
}

/// The EVFILT_USER ident the stream's news wakes the kqueue with.
const WAKE: usize = 0x7275_7368;

impl Shared {
    pub(crate) fn new(kq: Arc<OwnedFd>) -> Option<Arc<Shared>> {
        let ev = user(libc::EV_ADD | libc::EV_CLEAR, 0);
        // SAFETY: one valid change, no events read.
        let ok = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &ev,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } >= 0;
        ok.then(|| {
            Arc::new(Shared {
                kq,
                st: Mutex::new(State::default()),
            })
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wake(&self) {
        let ev = user(0, libc::NOTE_TRIGGER);
        // SAFETY: as in new.
        unsafe {
            libc::kevent(
                self.kq.as_raw_fd(),
                &ev,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
    }

    /// Takes what's been told since the last ask, what was busy, and
    /// whether anything was lost.
    pub(crate) fn take(&self) -> (HashSet<PathBuf>, HashSet<PathBuf>, bool) {
        let mut st = self.lock();
        let lost = std::mem::take(&mut st.lost);
        (
            std::mem::take(&mut st.told),
            std::mem::take(&mut st.busy),
            lost,
        )
    }

    /// Whether p is watched through the stream.
    pub(crate) fn watching(&self, p: &Path) -> bool {
        let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else {
            return false;
        };
        self.lock()
            .groups
            .values()
            .any(|g| g.given == dir && g.kids.contains_key(name))
    }

    /// The folders watched, as they are now.
    pub(crate) fn groups(&self) -> Groups {
        self.lock().groups.clone()
    }

    /// What was last seen of name in the folder real, if it's watched.
    pub(crate) fn seen(&self, real: &Path, name: &OsStr) -> Option<Seen> {
        self.lock().groups.get(real)?.kids.get(name).copied()
    }

    /// Makes groups those watched.
    pub(crate) fn set(&self, groups: Groups) {
        self.lock().groups = groups;
    }

    /// FSEvents' news: each watched entry an event falls under looked at,
    /// and told if it changed; one written deep inside, busy.
    fn took(&self, events: &[(&Path, u32)]) {
        let mut st = self.lock();
        let State {
            groups,
            told: out,
            busy,
            lost,
        } = &mut *st;
        let mut told = false;
        // Entries looked at in this batch: each once.
        let mut looked: Vec<(&[u8], &[u8])> = Vec::new();
        for &(p, flags) in events {
            if flags & LOST != 0 {
                *lost = true;
                told = true;
                continue;
            }
            let b = p.as_os_str().as_bytes();
            // The folder, as a slice of the event's own path.
            let Some(n) = groups.keys().find_map(|r| {
                let r = r.as_os_str().as_bytes();
                (b.len() > r.len() + 1 && b.starts_with(r) && b[r.len()] == b'/').then_some(r.len())
            }) else {
                continue;
            };
            let (root, rest) = (&b[..n], &b[n + 1..]);
            let (name, deep) = match rest.iter().position(|&c| c == b'/') {
                None => (rest, None),
                Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            };
            let kid = OsStr::from_bytes(name);
            let Some(g) = groups.get_mut(Path::new(OsStr::from_bytes(root))) else {
                continue;
            };
            if !g.kids.contains_key(kid) {
                continue;
            }
            // The entry itself, or an entry of it added, removed or
            // renamed, may change what kqueue would tell. Anything else
            // (a file written inside, or anything deeper) doesn't.
            let look = match deep {
                None => true,
                Some(d) => flags & ENTRY != 0 && !d.contains(&b'/'),
            };
            if look && looked.contains(&(root, name)) {
                continue;
            }
            let changed = look && {
                looked.push((root, name));
                let now = Seen::of(&Path::new(OsStr::from_bytes(root)).join(kid));
                let was = g.kids.get_mut(kid).expect("checked above");
                let changed = now != *was;
                *was = now;
                if !now.there() {
                    g.kids.remove(kid); // gone: no longer watched, as kqueue's
                }
                changed
            };
            if changed {
                out.insert(g.given.join(kid));
                told = true;
            } else if deep.is_some() && busy.insert(g.given.join(kid)) {
                // Written in, telling nothing: it costs a delivery a write
                // while here, and nothing on a descriptor.
                told = true;
            }
        }
        drop(st);
        if told {
            self.wake();
        }
    }
}

/// The stream's EVFILT_USER event: added, or triggered.
fn user(flags: u16, fflags: u32) -> libc::kevent {
    let mut ev = super::darwin::kevent_of(0, libc::EVFILT_USER, flags, fflags);
    ev.ident = WAKE;
    ev
}

/// Whether ev is the stream's wake.
pub(crate) fn is_wake(ev: &libc::kevent) -> bool {
    ev.filter == libc::EVFILT_USER && ev.ident == WAKE
}

extern "C" fn retain(info: *const c_void) -> *const c_void {
    // SAFETY: info is an Arc<Shared>'s pointer, made by into_raw.
    unsafe { Arc::increment_strong_count(info as *const Shared) };
    info
}

extern "C" fn release(info: *const c_void) {
    // SAFETY: as above; each release matches a retain.
    unsafe { Arc::decrement_strong_count(info as *const Shared) };
}

extern "C" fn callback(
    _s: CFRef,
    info: *mut c_void,
    n: usize,
    paths: *mut c_void,
    flags: *const u32,
    _ids: *const u64,
) {
    // SAFETY: the stream retains info (an Arc<Shared>) while it lives;
    // without UseCFTypes, paths is n C strings and flags n flags.
    let (sh, paths, flags) = unsafe {
        (
            &*(info as *const Shared),
            std::slice::from_raw_parts(paths as *const *const c_char, n),
            std::slice::from_raw_parts(flags, n),
        )
    };
    let events: Vec<(&Path, u32)> = paths
        .iter()
        // SAFETY: each is a NUL-terminated path FSEvents owns for the call.
        .map(|&p| Path::new(OsStr::from_bytes(unsafe { CStr::from_ptr(p) }.to_bytes())))
        .zip(flags.iter().copied())
        .collect();
    sh.took(&events);
}

/// The one queue every stream's news arrives on.
fn queue() -> *mut c_void {
    static Q: OnceLock<usize> = OnceLock::new();
    // SAFETY: a serial queue (no attributes), kept for the process's life.
    *Q.get_or_init(
        || unsafe { dispatch_queue_create(c"rush-fswait".as_ptr(), std::ptr::null()) } as usize,
    ) as *mut c_void
}

/// One folder's stream; it stops when dropped.
pub(crate) struct Stream(CFRef);

// SAFETY: FSEventStream calls are made under the watcher's lock, and
// FSEvents allows them from any thread.
unsafe impl Send for Stream {}

impl Stream {
    /// Starts telling sh of everything under paths (real paths).
    pub(crate) fn start(sh: &Arc<Shared>, paths: &[PathBuf]) -> Option<Stream> {
        let ctx = Context {
            version: 0,
            info: Arc::as_ptr(sh) as *const c_void,
            retain: Some(retain),
            release: Some(release),
            describe: std::ptr::null(),
        };
        // SAFETY: CF objects made here are released here; the stream
        // copies the context and retains info through it.
        unsafe {
            let mut strs: Vec<CFRef> = Vec::with_capacity(paths.len());
            for p in paths {
                let Ok(c) = CString::new(p.as_os_str().as_bytes()) else {
                    continue;
                };
                let s = CFStringCreateWithFileSystemRepresentation(std::ptr::null(), c.as_ptr());
                if !s.is_null() {
                    strs.push(s);
                }
            }
            let arr = (!strs.is_empty()).then(|| {
                CFArrayCreate(
                    std::ptr::null(),
                    strs.as_ptr(),
                    strs.len() as isize,
                    &kCFTypeArrayCallBacks as *const c_void,
                )
            });
            for s in strs {
                CFRelease(s);
            }
            let arr = arr.filter(|a| !a.is_null())?;
            let st = FSEventStreamCreate(
                std::ptr::null(),
                callback,
                &ctx,
                arr,
                SINCE_NOW,
                LATENCY,
                NO_DEFER | FILE_EVENTS,
            );
            CFRelease(arr);
            if st.is_null() {
                return None;
            }
            FSEventStreamSetDispatchQueue(st, queue());
            if FSEventStreamStart(st) == 0 {
                FSEventStreamInvalidate(st);
                FSEventStreamRelease(st);
                return None;
            }
            Some(Stream(st))
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: a started stream, stopped and let go once.
        unsafe {
            FSEventStreamStop(self.0);
            FSEventStreamInvalidate(self.0);
            FSEventStreamRelease(self.0);
        }
    }
}
