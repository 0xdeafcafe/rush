//! Searches hold the conversations they read for a while, so the next one
//! (the same words with a letter more, as you type) searches them in
//! memory rather than reading and decoding them again. Claude Code's are
//! held as a Tail, so one that has grown since reads only what's new.
//! Port of internal/convo/searchcache.go:1-114. As Go's, a timer sweeps
//! while any are held (sweepHeld), so one isn't held long after the last
//! search; it sweeps as it's used too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::find::{Library, Stat};
use super::pieces::PieceIndex;
use crate::model::Session;
use crate::transcript::Tail;

/// How much of the files on disk the held ones may come to, the first
/// read first: searched most recent first, the latest stay held.
const HELD_BYTES: u64 = 320 << 20;

/// How long one is held after a search last used it.
const HOLD_FOR: Duration = Duration::from_secs(120);

/// A conversation as held.
pub(crate) enum Body {
    /// Claude Code's, which reads on from where it got to.
    Tail(Tail, Session),
    /// Another agent's, read whole (None when it couldn't be).
    Read(Option<Session>),
    /// What's scored in it, from its index: held in place of the session
    /// when that was never read.
    Pieces(PieceIndex),
}

impl Body {
    /// The conversation as read; None when only its pieces are held.
    pub(crate) fn session(&self) -> Option<&Session> {
        match self {
            Body::Tail(_, s) => Some(s),
            Body::Read(s) => s.as_ref(),
            Body::Pieces(_) => None,
        }
    }

    pub(crate) fn is_pieces(&self) -> bool {
        matches!(self, Body::Pieces(_))
    }

    fn grows(&self) -> bool {
        matches!(self, Body::Tail(..))
    }
}

pub(crate) struct Entry {
    stat: Stat,
    used: Instant,
    grows: bool,
    body: Arc<Mutex<Body>>,
}

#[derive(Default)]
pub(crate) struct Held {
    m: HashMap<String, Entry>,
    bytes: u64,
    /// A sweep is due (Go's held.sweep timer).
    sweeping: bool,
}

impl Held {
    /// Lets go of those no search has used for HOLD_FOR; hands them back.
    fn sweep(&mut self, now: Instant) -> Vec<Entry> {
        let gone: Vec<String> = self
            .m
            .iter()
            .filter(|(_, e)| now.duration_since(e.used) >= HOLD_FOR)
            .map(|(k, _)| k.clone())
            .collect();
        let mut out = Vec::with_capacity(gone.len());
        for k in gone {
            if let Some(e) = self.m.remove(&k) {
                self.bytes -= e.stat.size;
                out.push(e);
            }
        }
        out
    }
}

/// Sweeps held HOLD_FOR from now, and again after while any are left
/// (searchcache.go:99 sweepHeld). What it lets go of is freed here, off
/// the thread that searched, and the memory given back.
fn sweep_later(held: Arc<Mutex<Held>>) {
    let h = held.clone();
    let spawned = std::thread::Builder::new()
        .name("rush-search-sweep".into())
        .spawn(move || {
            loop {
                std::thread::sleep(HOLD_FOR);
                let (gone, left) = {
                    let mut h = h.lock().unwrap();
                    let gone = h.sweep(Instant::now());
                    let left = !h.m.is_empty();
                    h.sweeping = left;
                    (gone, left)
                };
                if !gone.is_empty() {
                    drop(gone);
                    rush_sys::mem::give_back();
                }
                if !left {
                    return;
                }
            }
        });
    if spawned.is_err() {
        // No timer to be had: the next search sweeps, and tries again.
        held.lock().unwrap().sweeping = false;
    }
}

impl Library {
    /// The conversation at path as held, if it still stands as st says, or
    /// can be read on from where it got to (searchcache.go:51 heldFor).
    pub(crate) fn held_for(&self, path: &str, st: Stat) -> Option<Arc<Mutex<Body>>> {
        let mut held = self.held.lock().unwrap();
        let now = Instant::now();
        let _gone = held.sweep(now);
        let e = held.m.get_mut(path)?;
        let same = st == e.stat;
        let grown = e.grows && st.size > e.stat.size;
        if !same && !grown {
            let size = e.stat.size;
            held.m.remove(path);
            held.bytes -= size;
            return None;
        }
        e.used = now;
        Some(e.body.clone())
    }

    /// Keeps b for path, when there's room; hands it back to search.
    pub(crate) fn hold(&self, path: &str, st: Stat, b: Body) -> Arc<Mutex<Body>> {
        let grows = b.grows();
        let body = Arc::new(Mutex::new(b));
        let mut held = self.held.lock().unwrap();
        let now = Instant::now();
        let _gone = held.sweep(now);
        if let Some(old) = held.m.remove(path) {
            held.bytes -= old.stat.size;
        }
        if held.bytes + st.size > HELD_BYTES {
            return body;
        }
        held.bytes += st.size;
        let sweep = !held.sweeping;
        held.sweeping = true;
        held.m.insert(
            path.to_string(),
            Entry {
                stat: st,
                used: now,
                grows,
                body: body.clone(),
            },
        );
        drop(held);
        if sweep {
            sweep_later(self.held.clone());
        }
        body
    }

    /// Notes that the held conversation at path was read on to st.
    pub(crate) fn grew(&self, path: &str, st: Stat) {
        let mut held = self.held.lock().unwrap();
        let held = &mut *held;
        if let Some(e) = held.m.get_mut(path) {
            held.bytes = held.bytes + st.size - e.stat.size;
            e.stat = st;
        }
    }

    /// Whether a conversation is held for path, and as its pieces only.
    pub fn held(&self, path: &str) -> Option<bool> {
        let held = self.held.lock().unwrap();
        let e = held.m.get(path)?;
        let pieces = e.body.lock().unwrap().is_pieces();
        Some(pieces)
    }

    /// Lets go of what's held for path.
    pub fn forget(&self, path: &str) {
        let mut held = self.held.lock().unwrap();
        if let Some(e) = held.m.remove(path) {
            held.bytes -= e.stat.size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sweep_hands_back_what_it_let_go() {
        let lib = Library::new(None);
        let st = |size| Stat { size, modt: 1 };
        lib.hold("/a", st(10), Body::Read(None));
        lib.hold("/b", st(20), Body::Read(None));
        let mut h = lib.held.lock().unwrap();
        assert!(h.sweeping, "holding one starts the timer");
        h.m.get_mut("/a").unwrap().used -= HOLD_FOR;
        let gone = h.sweep(Instant::now());
        assert_eq!(gone.len(), 1);
        assert_eq!(h.bytes, 20);
        assert!(h.m.contains_key("/b"));
    }
}
