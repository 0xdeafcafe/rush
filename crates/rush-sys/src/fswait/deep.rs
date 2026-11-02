//! Rust-only: says which of some folders had anything changed anywhere
//! under them, through one FSEvents stream with no descriptor each, so
//! what's asked of them (git's word on a checkout) is asked again only
//! when something in them changed, rather than on a timer. Paths under
//! `skip` (a checkout's own index and objects, which asking git itself
//! writes) don't count.
//!
//! macOS only; elsewhere [`Deep::new`] is None and callers keep their
//! timer.

use std::ffi::{CStr, CString, OsStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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
/// Folder-level events, gathered over a second: what's asked again is
/// asked every few seconds at most anyway.
const LATENCY: f64 = 1.0;
/// What FSEvents lost or couldn't place (MustScanSubDirs, UserDropped,
/// KernelDropped, RootChanged): every folder counts as changed.
const LOST: u32 = 0x1 | 0x2 | 0x4 | 0x20;

/// What the stream calls with the folders (as given) that changed.
pub type Tell = Box<dyn Fn(Vec<PathBuf>) + Send + Sync>;

struct Shared {
    /// The folders as given, with their real paths.
    roots: Vec<(PathBuf, PathBuf)>,
    /// Real paths whose changes don't count.
    skip: Vec<PathBuf>,
    tell: Tell,
}

/// The stream; it stops when dropped.
pub struct Deep {
    st: CFRef,
}

// SAFETY: FSEvents allows its calls from any thread; the stream is only
// stopped once, on drop.
unsafe impl Send for Deep {}
unsafe impl Sync for Deep {}

impl Deep {
    /// Tells `tell` which of `roots` had anything under them change, but
    /// what's under `skip`. None where it can't start.
    pub fn new(roots: &[PathBuf], skip: &[PathBuf], tell: Tell) -> Option<Deep> {
        let real = |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let sh = Arc::new(Shared {
            roots: roots.iter().map(|p| (p.clone(), real(p))).collect(),
            skip: skip.iter().map(real).collect(),
            tell,
        });
        let ctx = Context {
            version: 0,
            info: Arc::as_ptr(&sh) as *const c_void,
            retain: Some(retain),
            release: Some(release),
            describe: std::ptr::null(),
        };
        // SAFETY: CF objects made here are released here; the stream
        // copies the context and retains info through it.
        unsafe {
            let mut strs: Vec<CFRef> = Vec::with_capacity(sh.roots.len());
            for (_, p) in &sh.roots {
                let Ok(c) = CString::new(p.as_os_str().as_bytes()) else {
                    continue;
                };
                let s = CFStringCreateWithFileSystemRepresentation(std::ptr::null(), c.as_ptr());
                if !s.is_null() {
                    strs.push(s);
                }
            }
            if strs.is_empty() {
                return None;
            }
            let arr = CFArrayCreate(
                std::ptr::null(),
                strs.as_ptr(),
                strs.len() as isize,
                &kCFTypeArrayCallBacks as *const c_void,
            );
            for s in strs {
                CFRelease(s);
            }
            if arr.is_null() {
                return None;
            }
            let st =
                FSEventStreamCreate(std::ptr::null(), callback, &ctx, arr, SINCE_NOW, LATENCY, 0);
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
            Some(Deep { st })
        }
    }
}

impl Drop for Deep {
    fn drop(&mut self) {
        // SAFETY: a started stream, stopped and let go once.
        unsafe {
            FSEventStreamStop(self.st);
            FSEventStreamInvalidate(self.st);
            FSEventStreamRelease(self.st);
        }
    }
}

impl Shared {
    fn took(&self, events: &[(&Path, u32)]) {
        let mut out: Vec<PathBuf> = Vec::new();
        for &(p, flags) in events {
            if flags & LOST != 0 {
                out = self.roots.iter().map(|(g, _)| g.clone()).collect();
                break;
            }
            if self.skip.iter().any(|s| p.starts_with(s)) {
                continue;
            }
            for (given, real) in &self.roots {
                if p.starts_with(real) && !out.contains(given) {
                    out.push(given.clone());
                }
            }
        }
        if !out.is_empty() {
            (self.tell)(out);
        }
    }
}

extern "C" fn retain(info: *const c_void) -> *const c_void {
    // SAFETY: info is an Arc<Shared>'s pointer.
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

fn queue() -> *mut c_void {
    static Q: OnceLock<usize> = OnceLock::new();
    // SAFETY: a serial queue, kept for the process's life.
    *Q.get_or_init(
        || unsafe { dispatch_queue_create(c"rush-deepwatch".as_ptr(), std::ptr::null()) } as usize,
    ) as *mut c_void
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    /// A write deep in a folder tells it; one under skip doesn't.
    #[test]
    fn tells_deep_changes_not_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(a.join("x/y")).unwrap();
        std::fs::create_dir_all(b.join(".git/objects")).unwrap();
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let d = Deep::new(
            &[a.clone(), b.clone()],
            &[b.join(".git/objects")],
            Box::new(move |v| {
                let _ = tx.lock().unwrap().send(v);
            }),
        )
        .expect("a stream");
        // What making the folders told, if anything, is let go first.
        std::thread::sleep(Duration::from_millis(1500));
        while rx.try_recv().is_ok() {}
        std::fs::write(b.join(".git/objects/o"), "x").unwrap();
        std::fs::write(a.join("x/y/f"), "x").unwrap();
        // FSEvents is slow on a busy machine: wait for a's news, noting
        // everything told meanwhile.
        let mut told = Vec::new();
        while !told.contains(&a) {
            told.extend(rx.recv_timeout(Duration::from_secs(30)).expect("told"));
        }
        // b's objects may be told along with it when FSEvents gathers a
        // busy spell into "look again" (LOST): that's took's to judge.
        drop(d);
    }

    /// What's under skip isn't news; a lost spell tells every folder.
    #[test]
    fn took_skips_and_tells_all_when_lost() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let sh = Shared {
            roots: vec![
                (PathBuf::from("/a"), PathBuf::from("/r/a")),
                (PathBuf::from("/b"), PathBuf::from("/r/b")),
            ],
            skip: vec![PathBuf::from("/r/b/.git/objects")],
            tell: Box::new(move |v| g.lock().unwrap().push(v)),
        };
        sh.took(&[(Path::new("/r/b/.git/objects/ab"), 0)]);
        assert!(got.lock().unwrap().is_empty());
        sh.took(&[
            (Path::new("/r/a/x/y"), 0),
            (Path::new("/r/b/.git/objects"), 0),
        ]);
        assert_eq!(got.lock().unwrap().pop(), Some(vec![PathBuf::from("/a")]));
        sh.took(&[(Path::new("/r/b/src"), 0x1)]);
        assert_eq!(
            got.lock().unwrap().pop(),
            Some(vec![PathBuf::from("/a"), PathBuf::from("/b")])
        );
    }
}
