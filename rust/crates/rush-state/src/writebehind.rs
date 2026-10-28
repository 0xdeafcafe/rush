//! The view saves its config and overlay from the UI thread, where nothing
//! may wait on the disk. With write-behind on, a save encodes there
//! (memory only) and one writer thread puts the bytes on disk, in order; a
//! file saved again before its last save was written is written once,
//! with the newest. flush waits for everything handed over. Hosts and the
//! CLI leave it off and write as they save.
//!
//! The writer runs only while there's something to write: it's started by
//! the save that finds it idle and ends when the queue is empty.
//!
//! Ported from internal/state/writebehind.go.

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;
use std::sync::{Condvar, LazyLock, Mutex};

pub(crate) type Job = Box<dyn FnOnce() -> io::Result<()> + Send>;

#[derive(Default)]
struct Behind {
    on: bool,
    /// By what it writes: a newer job of the same replaces it.
    pending: HashMap<String, Job>,
    order: VecDeque<String>,
    busy: bool,
    last_err: Option<io::Error>,
}

static BEHIND: LazyLock<(Mutex<Behind>, Condvar)> =
    LazyLock::new(|| (Mutex::new(Behind::default()), Condvar::new()));

fn lock() -> std::sync::MutexGuard<'static, Behind> {
    BEHIND.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Has saves written by a thread of their own from now on.
pub fn write_behind() {
    lock().on = true;
}

/// Waits until every save handed over is on disk, and says how the last
/// one that failed went.
pub fn flush() -> io::Result<()> {
    let mut b = lock();
    while b.busy {
        b = BEHIND.1.wait(b).unwrap_or_else(|e| e.into_inner());
    }
    match b.last_err.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Whether saves are handed to the writer.
pub(crate) fn on() -> bool {
    lock().on
}

/// Writes v to path now, or hands it to the writer when write-behind is
/// on; it only fails there when v can't be encoded.
pub(crate) fn save<T: Serialize + ?Sized>(path: &Path, v: &T) -> io::Result<()> {
    if !on() {
        return crate::store::write_json(path, v);
    }
    let b = crate::json::to_vec_indent(v).map_err(io::Error::other)?;
    let p = path.to_path_buf();
    queue(
        path.to_string_lossy().into_owned(),
        Box::new(move || crate::store::write_bytes(&p, &b)),
    );
    Ok(())
}

/// Hands job to the writer, under key: a job queued under a key already
/// waiting takes its place in the order.
pub(crate) fn queue(key: String, job: Job) {
    let mut b = lock();
    if !b.pending.contains_key(&key) {
        b.order.push_back(key.clone());
    }
    b.pending.insert(key, job);
    if !b.busy {
        b.busy = true;
        std::thread::Builder::new()
            .name("rush-write-behind".into())
            .spawn(write)
            .expect("start the write-behind thread");
    }
}

fn write() {
    let mut b = lock();
    while let Some(key) = b.order.pop_front() {
        let Some(job) = b.pending.remove(&key) else {
            continue;
        };
        drop(b);
        let res = job();
        b = lock();
        if let Err(e) = res {
            b.last_err = Some(e);
        }
    }
    b.busy = false;
    BEHIND.1.notify_all();
}
