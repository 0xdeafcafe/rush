//! Rust-only: a conversation read back the one way, whatever agent wrote
//! it: its end alone first, read from the file's end backwards, then the
//! whole from its start, then what's added as it grows. rush-ui's worker
//! (spawn_read), its read-ahead and `--render` read every agent's
//! conversation through this, never one agent's own way.
//!
//! Two forms of conversation are read: a transcript in the form Claude
//! Code writes, which rush frames itself ([`Transcript`]: Claude Code's own
//! sessions and every agent whose adapter says it writes them,
//! `claude_transcripts`), and any other agent's history as its adapter
//! reads it back as events ([`History`]: its TailReader for the end, its
//! HistoryFollower or HistoryReader for the whole). An adapter that can't
//! read its end alone goes the same way, its end being all of it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rush_core::adapter::{self, AdapterRef, Follow, WaitOn};
use rush_core::event::Event;
use rush_sys::fswait::Grows;

use crate::model::Session;
use crate::transcript::{Decode, Fresh, Tail};

/// What one read of a conversation gave, to apply to it.
pub enum Got {
    /// A transcript's lines, parsed.
    Lines(Fresh),
    /// A history's events; reset: it was rewritten, start over.
    Events { events: Vec<Event>, reset: bool },
}

/// One read of a conversation as it grows.
pub struct Slice {
    pub got: Got,
    /// More is written than this slice took: read again at once.
    pub more: bool,
}

impl Slice {
    /// Whether it holds anything to apply.
    pub fn is_empty(&self) -> bool {
        match &self.got {
            Got::Lines(f) => f.is_empty(),
            Got::Events { events, reset } => events.is_empty() && !reset,
        }
    }

    /// Applies it to s.
    pub fn apply(self, s: &mut Session) {
        match self.got {
            Got::Lines(f) => {
                f.apply(s);
            }
            Got::Events { events, reset } => apply_events(s, &events, reset),
        }
    }
}

/// Applies events a history gave to sess: reset, the history was
/// rewritten and it starts over. History events carry no times, so each is
/// applied as of an unknown time (0).
pub fn apply_events(sess: &mut Session, events: &[Event], reset: bool) {
    if reset {
        // What its host saved (its model, "opus 5.5 · 1M") isn't the
        // history's to say: it stays, as take_whole keeps it.
        let info = std::mem::take(&mut sess.info);
        *sess = Session::new();
        sess.info = info;
    }
    for ev in events {
        sess.apply(ev, 0);
    }
}

/// One conversation being read.
pub trait Reading: Send {
    /// Its end alone: the whole lines of its last `most` bytes, found from
    /// the end backwards, as the conversation so far, marked partial
    /// ("reading earlier…") for the whole to take its place. None when
    /// that is all of it, or its agent can't read its end alone: the whole
    /// is read from its start then. Called before next, if at all.
    fn end(&mut self, most: u64) -> Option<Session>;
    /// Reads on from where it is, its start first: at most `most` bytes'
    /// worth. A line still being written waits for the next call. An
    /// error is one to show.
    fn next(&mut self, most: usize) -> adapter::Result<Slice>;
    /// What changes when there's more to read.
    fn wait_on(&self) -> WaitOn;
    /// Whether its own events say how its turns end and what its session
    /// is (model, effort): a history's do. A transcript's turn left open
    /// by a stopped agent is ended by rush, and the list's word on its
    /// session taken (Go's followTail).
    fn says_itself(&self) -> bool;
    /// A read under way gives up once stop is set.
    fn stop_on(&mut self, _: Arc<AtomicBool>) {}
}

impl Grows for dyn Reading + '_ {
    type Slice = Slice;
    fn next(&mut self, most: usize) -> adapter::Result<(Slice, bool)> {
        let s = Reading::next(self, most)?;
        let worth = !s.is_empty();
        Ok((s, worth))
    }
    fn more(s: &Slice) -> bool {
        s.more
    }
    fn wait_on(&self) -> WaitOn {
        Reading::wait_on(self)
    }
}

/// Reads a reading on from where it is until nothing more is written,
/// into sess; stops early (false) once stop is set.
pub fn read_on(
    r: &mut dyn Reading,
    sess: &mut Session,
    most: usize,
    stop: &AtomicBool,
) -> adapter::Result<bool> {
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let s = r.next(most)?;
        let more = s.more;
        s.apply(sess);
        if !more {
            return Ok(true);
        }
    }
}

/// A transcript in the form Claude Code writes, framed by Tail (its
/// prompts, notices, compactions and hooks) with its agent's decoder.
pub struct Transcript {
    path: PathBuf,
    decode: Decode,
    tail: Option<Tail>,
    stop: Option<Arc<AtomicBool>>,
}

impl Transcript {
    pub fn new(path: impl Into<PathBuf>, decode: Decode) -> Transcript {
        Transcript {
            path: path.into(),
            decode,
            tail: None,
            stop: None,
        }
    }

    fn stopped(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }
}

impl Reading for Transcript {
    fn end(&mut self, most: u64) -> Option<Session> {
        if self.tail.is_some() {
            return None;
        }
        let mut t = Tail::from_end(&self.path, most, self.decode.clone());
        if !t.partway() {
            return None;
        }
        t.stop = self.stop.clone();
        let mut s = Session::new();
        t.read(&mut s).ok()?;
        (!self.stopped()).then_some(s)
    }

    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        let (path, decode, stop) = (&self.path, &self.decode, &self.stop);
        let t = self.tail.get_or_insert_with(|| {
            let mut t = Tail::new(path, decode.clone());
            t.stop = stop.clone();
            t
        });
        let f = match t.fetch_most(most) {
            Ok(f) => f,
            // Only one that's gone is told: anything else is read again on
            // the next change.
            Err(_) if path.exists() => {
                return Ok(Slice {
                    got: Got::Events {
                        events: Vec::new(),
                        reset: false,
                    },
                    more: false,
                });
            }
            Err(e) => return Err(e.into()),
        };
        let more = f.more;
        Ok(Slice {
            got: Got::Lines(f),
            more,
        })
    }

    fn wait_on(&self) -> WaitOn {
        WaitOn {
            paths: vec![self.path.clone()],
            ..WaitOn::default()
        }
    }

    fn says_itself(&self) -> bool {
        false
    }

    fn stop_on(&mut self, stop: Arc<AtomicBool>) {
        if let Some(t) = &mut self.tail {
            t.stop = Some(stop.clone());
        }
        self.stop = Some(stop);
    }
}

/// Another agent's history, as its adapter reads it back.
pub struct History {
    ad: AdapterRef,
    s: adapter::Session,
    follow: Option<Box<dyn Follow>>,
    /// Read once through its HistoryReader (it has no follower).
    read: bool,
}

/// An adapter that can't read its sessions back.
#[derive(Debug)]
struct Unreadable(String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rush can't read {}'s conversations yet", self.0)
    }
}

impl std::error::Error for Unreadable {}

impl History {
    /// Session s's history through adapter ad: its follower started here
    /// (it may read the disk), so what it waits on is known before it
    /// reads.
    pub fn new(ad: AdapterRef, s: adapter::Session) -> adapter::Result<History> {
        let follow = match ad.history_follower() {
            Some(f) => Some(f.follow_history(&s)?),
            None if ad.history_reader().is_some() => None,
            None => return Err(Box::new(Unreadable(ad.name()))),
        };
        Ok(History {
            ad,
            s,
            follow,
            read: false,
        })
    }
}

impl Reading for History {
    fn end(&mut self, most: u64) -> Option<Session> {
        let t = self.ad.tail_reader()?;
        let (evs, cut) = t.history_tail(&self.s, most as i64).ok()?;
        if !cut {
            return None;
        }
        let mut sess = Session::new();
        apply_events(&mut sess, &evs, false);
        sess.partial = true;
        Some(sess)
    }

    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        if let Some(f) = &mut self.follow {
            let sl = f.next(most)?;
            return Ok(Slice {
                got: Got::Events {
                    events: sl.events,
                    reset: sl.reset,
                },
                more: sl.more,
            });
        }
        // Read once, whole: it can't be followed.
        let mut events = Vec::new();
        if !std::mem::replace(&mut self.read, true)
            && let Some(r) = self.ad.history_reader()
        {
            r.history(&self.s, 0, most, &mut |evs| {
                events.extend(evs);
                true
            })?;
        }
        Ok(Slice {
            got: Got::Events {
                events,
                reset: false,
            },
            more: false,
        })
    }

    fn wait_on(&self) -> WaitOn {
        self.follow
            .as_ref()
            .map(|f| f.wait_on())
            .unwrap_or_default()
    }

    fn says_itself(&self) -> bool {
        true
    }
}
