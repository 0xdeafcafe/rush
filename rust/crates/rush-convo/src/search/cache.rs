//! Searches hold the conversations they read for a while, so the next one
//! (the same words with a letter more, as you type) searches them in
//! memory rather than reading and decoding them again. Claude Code's are
//! held as a Tail, so one that has grown since reads only what's new.
//! Port of internal/convo/searchcache.go:1-114; Go sweeps on a timer, this
//! sweeps as it's used, so nothing wakes while nobody searches.

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
}

impl Held {
    /// Lets go of those no search has used for HOLD_FOR.
    fn sweep(&mut self, now: Instant) {
        let mut gone = 0;
        self.m.retain(|_, e| {
            let keep = now.duration_since(e.used) < HOLD_FOR;
            if !keep {
                gone += e.stat.size;
            }
            keep
        });
        self.bytes -= gone;
    }
}

impl Library {
    /// The conversation at path as held, if it still stands as st says, or
    /// can be read on from where it got to (searchcache.go:51 heldFor).
    pub(crate) fn held_for(&self, path: &str, st: Stat) -> Option<Arc<Mutex<Body>>> {
        let mut held = self.held.lock().unwrap();
        let now = Instant::now();
        held.sweep(now);
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
        held.sweep(now);
        if let Some(old) = held.m.remove(path) {
            held.bytes -= old.stat.size;
        }
        if held.bytes + st.size > HELD_BYTES {
            return body;
        }
        held.bytes += st.size;
        held.m.insert(
            path.to_string(),
            Entry {
                stat: st,
                used: now,
                grows,
                body: body.clone(),
            },
        );
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
