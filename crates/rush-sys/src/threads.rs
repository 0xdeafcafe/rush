//! The threads rush starts: how many this process has, and helpers to start
//! one with a stack the size of what it does (Rust-only).
//!
//! A thread std starts gets a 2 MB stack (the main thread 8 MB). Most of
//! rush's helpers block in one wait (a read, a kevent, a condvar) and call
//! little: [`SMALL`] is plenty for them. The stack is only reserved, not
//! touched, so the saving is in address space and in the pages a deep
//! call once dirtied, not in a fresh thread's footprint; what costs is the
//! thread itself (its kernel side, its wake-ups), so the first aim is
//! fewer of them.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

/// For a helper that waits on one thing and hands what it gets on: a
/// reader of a pipe or socket, a waiter on a child, a timer. Big enough for
/// serde_json on a message and a log line.
pub const SMALL: usize = 256 << 10;

/// For a worker that parses, formats or calls into other crates' deep
/// code (a JSON-RPC handler, a plugin's main).
pub const MEDIUM: usize = 1 << 20;

/// Starts f on a thread called name with a stack of stack bytes.
pub fn spawn_sized<T: Send + 'static>(
    name: &str,
    stack: usize,
    f: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(stack)
        .spawn(f)
}

/// Starts f on a thread called name with a [`SMALL`] stack.
pub fn spawn_small<T: Send + 'static>(
    name: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    spawn_sized(name, SMALL, f)
}

/// Threads for work handed off one piece at a time (the UI's `later`):
/// a piece goes to a thread waiting idle, else to a new one, so none ever
/// queues behind another, and a thread idle for `keep` ends. While an
/// agent works the UI hands off a few pieces a second; each started a
/// thread of its own.
pub struct Pool {
    inner: Arc<PoolInner>,
}

type Job = Box<dyn FnOnce() + Send>;

struct PoolInner {
    name: String,
    stack: usize,
    keep: Duration,
    q: Mutex<PoolQ>,
    cv: Condvar,
}

#[derive(Default)]
struct PoolQ {
    jobs: VecDeque<Job>,
    /// Threads waiting for a piece.
    idle: usize,
    /// Threads started, for tests.
    started: usize,
}

impl Pool {
    /// A pool whose threads are called name, with stacks of stack bytes,
    /// each ending once idle for keep.
    pub fn new(name: &str, stack: usize, keep: Duration) -> Pool {
        Pool {
            inner: Arc::new(PoolInner {
                name: name.into(),
                stack,
                keep,
                q: Mutex::new(PoolQ::default()),
                cv: Condvar::new(),
            }),
        }
    }

    /// Runs f on an idle thread, or a new one when none is idle.
    pub fn run(&self, f: impl FnOnce() + Send + 'static) -> io::Result<()> {
        let mut q = self.inner.lock();
        if q.idle > q.jobs.len() {
            q.jobs.push_back(Box::new(f));
            self.inner.cv.notify_one();
            return Ok(());
        }
        q.started += 1;
        drop(q);
        let inner = self.inner.clone();
        let started = spawn_sized(&self.inner.name, self.inner.stack, move || {
            f();
            inner.serve();
        });
        if started.is_err() {
            self.inner.lock().started -= 1;
        }
        started.map(|_| ())
    }

    /// How many threads it has started so far.
    pub fn started(&self) -> usize {
        self.inner.lock().started
    }

    /// How many of its threads wait idle now.
    pub fn idle(&self) -> usize {
        self.inner.lock().idle
    }
}

impl PoolInner {
    fn lock(&self) -> MutexGuard<'_, PoolQ> {
        self.q.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes the pieces handed to it until it has been idle for keep.
    fn serve(&self) {
        let mut q = self.lock();
        loop {
            if let Some(job) = q.jobs.pop_front() {
                drop(q);
                job();
                q = self.lock();
                continue;
            }
            q.idle += 1;
            q = self
                .cv
                .wait_timeout_while(q, self.keep, |q| q.jobs.is_empty())
                .unwrap_or_else(|e| e.into_inner())
                .0;
            q.idle -= 1;
            if q.jobs.is_empty() {
                return;
            }
        }
    }
}

/// How many threads this process runs now; 0 when it can't be told.
pub fn count() -> usize {
    count_of(std::process::id() as i32)
}

/// How many threads process pid runs now; 0 when it can't be told.
pub fn count_of(pid: i32) -> usize {
    // SAFETY: zeroed is a valid proc_taskinfo; proc_pidinfo fills it.
    let mut ti: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: an out-buffer of the size given.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut ti as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    if n == size {
        ti.pti_threadnum as usize
    } else {
        0
    }
}

/// proc_pidinfo's list of a process's threads (sys/proc_info.h), which
/// libc doesn't name.
const PROC_PIDLISTTHREADS: libc::c_int = 6;

/// The names of this process's threads now, an unnamed one as ""; empty
/// when they can't be told. For a test to say which it didn't expect.
pub fn names() -> Vec<String> {
    let pid = std::process::id() as i32;
    let mut ids = vec![0u64; count() + 16];
    // SAFETY: an out-buffer of the size given.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDLISTTHREADS,
            0,
            ids.as_mut_ptr().cast(),
            (ids.len() * 8) as libc::c_int,
        )
    };
    if n <= 0 {
        return Vec::new();
    }
    ids.truncate(n as usize / 8);
    let size = std::mem::size_of::<libc::proc_threadinfo>() as libc::c_int;
    ids.iter()
        .filter_map(|&t| {
            // SAFETY: zeroed is a valid proc_threadinfo; proc_pidinfo fills it.
            let mut ti: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
            // SAFETY: an out-buffer of the size given.
            let r = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTHREADINFO,
                    t,
                    (&mut ti as *mut libc::proc_threadinfo).cast(),
                    size,
                )
            };
            // SAFETY: pth_name is NUL-terminated within its array.
            (r == size).then(|| {
                unsafe { std::ffi::CStr::from_ptr(ti.pth_name.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn counts_a_thread_started_and_ended() {
        // Other tests start threads of their own meanwhile, so only that a
        // count is had, and that a small stack runs what a helper does.
        assert!(count() >= 1);
        let (tx, rx) = mpsc::channel::<()>();
        let t = spawn_small("rush-test", move || {
            let v: serde_json::Value = serde_json::from_str(r#"{"a":[1,2,{"b":"c"}]}"#).unwrap();
            let _ = rx.recv();
            v.to_string()
        })
        .unwrap();
        assert!(count() >= 2);
        assert!(names().iter().any(|n| n == "rush-test"), "{:?}", names());
        drop(tx);
        assert_eq!(t.join().unwrap(), r#"{"a":[1,2,{"b":"c"}]}"#);
        assert_eq!(count_of(-1), 0);
    }

    /// A piece goes to the thread idle from the last; two at once each
    /// get a thread, neither waiting for the other.
    #[test]
    fn a_pool_reuses_an_idle_thread() {
        let p = Pool::new("rush-test-pool", SMALL, Duration::from_secs(600));
        let idle = |p: &Pool| {
            while p.idle() == 0 {
                std::thread::yield_now();
            }
        };
        let (tx, rx) = mpsc::channel();
        let t = tx.clone();
        p.run(move || t.send(std::thread::current().id()).unwrap())
            .unwrap();
        let first = rx.recv().unwrap();
        idle(&p);
        p.run(move || tx.send(std::thread::current().id()).unwrap())
            .unwrap();
        assert_eq!(rx.recv().unwrap(), first);
        assert_eq!(p.started(), 1);
        idle(&p);
        let (go_tx, go) = mpsc::channel::<()>();
        let (ran_tx, ran) = mpsc::channel();
        let r = ran_tx.clone();
        p.run(move || {
            r.send(()).unwrap();
            let _ = go.recv();
        })
        .unwrap();
        ran.recv().unwrap();
        p.run(move || ran_tx.send(()).unwrap()).unwrap();
        ran.recv().unwrap();
        assert_eq!(p.started(), 2);
        drop(go_tx);
    }

    /// Its threads end once idle for their keep; pieces run all the same.
    #[test]
    fn a_pool_thread_ends_once_idle_for_its_keep() {
        let p = Pool::new("rush-test-pool", SMALL, Duration::ZERO);
        let (tx, rx) = mpsc::channel();
        for _ in 0..3 {
            let t = tx.clone();
            p.run(move || t.send(()).unwrap()).unwrap();
            rx.recv().unwrap();
        }
        assert!(p.started() >= 1);
    }
}
