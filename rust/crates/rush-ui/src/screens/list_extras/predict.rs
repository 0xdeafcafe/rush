//! Rust-only: the conversations the list is likely to show next, their
//! ends read ahead off the UI thread, so the Session beside the list
//! (or one opened) shows at once rather than "reading the conversation…",
//! or a live session rush hosts its transcript's end rather than
//! "connecting…" (session_core/live.rs early). Go reads a picked agent's
//! conversation only once it's picked.
//!
//! Only after a key moves the pick (idle reads nothing): the next two
//! rows the way it moved, the one just left, the one waiting longest on
//! you (ctrl+n's), and with no Session beside the list the picked one
//! (enter's). One read at
//! a time, the end's 2 MB as the tail's worker reads it (worker.rs
//! TAIL_END), parsed and taken into a conversation there. At most KEEP
//! are kept, those the list has moved away from dropped, all of them
//! when the machine's memory is short.
//!
//! Never stale: one is shown as read only while its file is as it was
//! read (length, time, the same file); else the tail's worker first takes
//! in what was written since, from where the read stopped, as it would
//! have (worker::TailStart::From). A live session's is used only as read,
//! its host the same process; else it's read again as before.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use rush_convo::{Decode, Session, Tail};
use rush_core::adapter::{self, AdapterRef, Follow};
use rush_core::agent::Agent;

use crate::frame::Out;
use crate::list::ListLine;
use crate::model::Model;
use crate::session::{SessionView, Source};
use crate::worker::{TAIL_END, TAIL_SLICE};

/// The most conversations' ends kept read ahead.
pub const KEEP: usize = 3;
/// How far ahead the way the pick moves.
const AHEAD: usize = 2;

/// What's read ahead for an agent.
#[derive(Clone)]
enum What {
    /// Its transcript's end, as the tail's worker reads it first.
    Tail(PathBuf, Decode),
    /// A live session rush hosts: its transcript's end as live.rs's early
    /// reads it while its host is dialled.
    Early(Box<Agent>),
    /// Another agent's history, as its follower reads it (host: a stopped
    /// rush session's, whose folder says where it is).
    History(AdapterRef, Box<adapter::Session>, String),
}

/// Where a read ahead stopped, for its reading to go on from there.
enum From {
    Tail(Box<Tail>),
    /// A live session's end: read again rather than followed.
    Early,
    /// A history's follower, and the files it waits on as they were
    /// before it read: None when it waits on something else, so it's
    /// always topped up first.
    Follow(Box<dyn Follow>, Option<Vec<(PathBuf, Option<Stat>)>>),
}

/// An agent whose conversation is likely shown next.
#[derive(Clone)]
struct Want {
    key: String,
    /// Its process: a live session's host started again is read again.
    pid: i32,
    what: What,
}

/// A conversation's end, read ahead.
pub struct Ahead {
    key: String,
    pid: i32,
    /// Where it was read from, and the file as it was when the read
    /// ended: (length, modified, device, inode).
    path: PathBuf,
    seen: Option<Stat>,
    /// Where its read stopped.
    from: From,
    sess: Box<Session>,
}

impl Want {
    /// Whether a is what's wanted, read.
    fn is(&self, a: &Ahead) -> bool {
        self.key == a.key
            && self.pid == a.pid
            && match &self.what {
                What::Tail(p, _) => matches!(a.from, From::Tail(_)) && *p == a.path,
                What::Early(_) => matches!(a.from, From::Early),
                What::History(..) => matches!(a.from, From::Follow(..)),
            }
    }
}

/// One taken for a view: fresh when nothing was written since it was read.
pub struct Hit {
    pub fresh: bool,
    pub tail: Box<Tail>,
    pub sess: Box<Session>,
}

/// A history read ahead, taken for a view.
pub struct HistHit {
    pub fresh: bool,
    pub follow: Box<dyn Follow>,
    pub sess: Box<Session>,
}

/// What's read ahead and what's wanted (Model::kept).
#[derive(Default)]
pub struct Predict {
    /// Read, newest first.
    ready: VecDeque<Ahead>,
    /// Wanted, most likely first.
    want: Vec<Want>,
    /// The one being read, and what stops it.
    busy: Option<(String, Arc<AtomicBool>)>,
    /// The Session beside the list until the pick moved, kept as it was
    /// (its reading still following its file), for going back to it.
    pub(crate) left: Option<SessionView>,
    /// Reads made and taken, for the timing log.
    made: u64,
    used: u64,
}

/// The longest transcript whose Session is kept once the list moves off
/// it: a longer one's conversation is read whole, and too big to keep.
const LEFT_MOST: u64 = 4 << 20;

/// Whether view v is kept once the list moves off it: a conversation read
/// from its own file, not too long (a live session rush hosts is its
/// host's, a room its log's).
fn keeps(v: &SessionView) -> bool {
    let path = match &v.source {
        Source::Transcript { path, .. } => path.clone(),
        Source::History { session, .. } if !session.transcript.is_empty() => {
            PathBuf::from(&session.transcript)
        }
        _ => return false,
    };
    std::fs::metadata(path).is_ok_and(|m| m.len() <= LEFT_MOST)
}

/// The Session beside the list as the pick moves off it: kept if it can
/// be, the one kept before let go.
pub fn leave(m: &mut Model, v: SessionView) {
    let keep = m.sender().is_some() && keeps(&v) && !memory_short();
    rush_draw::perf_note(|| format!("predict left {} kept={keep}", v.key));
    m.kept.get::<Predict>().left = keep.then_some(v);
}

/// The Session kept for agent key as the list moved off it, taken back.
pub fn back(m: &mut Model, key: &str) -> Option<SessionView> {
    let p = m.kept.get::<Predict>();
    let v = p.left.take_if(|v| v.key == key)?;
    rush_draw::perf_note(|| format!("predict back {key}"));
    Some(v)
}

type Stat = (u64, SystemTime, u64, u64);

/// The file at path now: (length, modified, device, inode).
fn stat(path: &Path) -> Option<Stat> {
    stat_of(&std::fs::metadata(path).ok()?)
}

fn stat_of(m: &std::fs::Metadata) -> Option<Stat> {
    use std::os::unix::fs::MetadataExt;
    Some((m.len(), m.modified().ok()?, m.dev(), m.ino()))
}

/// Whether the machine is short of memory (the kernel's pressure level
/// past normal): nothing is read ahead then.
fn memory_short() -> bool {
    #[cfg(target_os = "macos")]
    {
        let mut v: u32 = 0;
        let mut len = std::mem::size_of::<u32>();
        let name = c"kern.memorystatus_vm_pressure_level";
        // SAFETY: name is NUL-terminated; v and len are valid for len.
        let r = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&raw mut v).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        r == 0 && v > 1
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// The conversation's end read ahead for agent key at path, taken out:
/// None when it wasn't, or was of another file.
pub fn take(m: &mut Model, key: &str, path: &Path) -> Option<Hit> {
    let p = m.kept.get::<Predict>();
    let i = p
        .ready
        .iter()
        .position(|a| a.key == key && matches!(a.from, From::Tail(_)))?;
    let a = p.ready.remove(i)?;
    let From::Tail(tail) = a.from else {
        return None;
    };
    if a.path != path {
        return None;
    }
    let fresh = a.seen.is_some() && stat(path) == a.seen;
    used(p, key, fresh);
    Some(Hit {
        fresh,
        tail,
        sess: a.sess,
    })
}

/// Live session key's transcript end read ahead, taken out, if its host
/// is still process pid and nothing was written since; None otherwise.
pub fn take_early(m: &mut Model, key: &str, pid: i32) -> Option<Session> {
    let p = m.kept.get::<Predict>();
    let i = p
        .ready
        .iter()
        .position(|a| a.key == key && matches!(a.from, From::Early))?;
    let a = p.ready.remove(i)?;
    let fresh = a.pid == pid && a.seen.is_some() && stat(&a.path) == a.seen;
    used(p, key, fresh);
    fresh.then_some(*a.sess)
}

/// Agent key's history read ahead, taken out: fresh when none of the
/// files its follower waits on changed since before it read.
pub fn take_history(m: &mut Model, key: &str) -> Option<HistHit> {
    let p = m.kept.get::<Predict>();
    let i = p
        .ready
        .iter()
        .position(|a| a.key == key && matches!(a.from, From::Follow(..)))?;
    let a = p.ready.remove(i)?;
    let From::Follow(follow, seen) = a.from else {
        return None;
    };
    let fresh = seen.is_some_and(|ps| ps.iter().all(|(p, s)| s.is_some() && stat(p) == *s));
    used(p, key, fresh);
    Some(HistHit {
        fresh,
        follow,
        sess: a.sess,
    })
}

/// Notes one read ahead taken, in the timing log.
fn used(p: &mut Predict, key: &str, fresh: bool) {
    p.used += 1;
    let (made, used) = (p.made, p.used);
    rush_draw::perf_note(|| format!("predict hit {key} fresh={fresh} made={made} used={used}"));
}

/// What agent i's conversation would be read ahead as: its transcript, or
/// for a live session rush hosts its transcript's end; None for one read
/// any other way.
fn what(m: &mut Model, i: usize) -> Option<What> {
    let a = &m.agents[i];
    if a.rush && a.pid != 0 {
        let a = a.clone();
        let h = crate::screens::session_core::host::hosts(m);
        return h.hosts(&a).then(|| What::Early(Box::new(a)));
    }
    let host = if a.rush {
        a.job.id.clone()
    } else {
        String::new()
    };
    match Source::of(&m.reg, a)? {
        Source::Transcript { path, decode } => Some(What::Tail(path, decode)),
        Source::History { adapter, session } if adapter.history_follower().is_some() => {
            Some(What::History(adapter, session, host))
        }
        _ => None,
    }
}

/// The pick moved by d (a key in the list): what's likely shown next is
/// read ahead.
pub fn moved(m: &mut Model, d: isize) {
    if m.sender().is_none() || d == 0 {
        return; // no loop (tests, --render): nothing waits to be shown
    }
    // The list's agents in order, and where the pick is among its rows.
    let rows: Vec<Option<usize>> = m
        .lines
        .iter()
        .filter_map(|l| match l {
            ListLine::Section { .. } | ListLine::Room { .. } => Some(None),
            ListLine::Agent(i) | ListLine::Member { i, .. } => Some(Some(*i)),
            _ => None,
        })
        .collect();
    let at = rows
        .iter()
        .position(|r| r.is_some_and(|i| m.agents[i].key == m.sel));
    let mut picks: Vec<usize> = Vec::new();
    // The one picked: what enter opens, unless it's beside the list now
    // or the split is about to read it.
    let beside = crate::screens::list_extras::split::widths(m).1 > 0;
    if let Some(i) = at.and_then(|a| rows[a])
        && !beside
    {
        picks.push(i);
    }
    // The next rows the way it moved.
    if let Some(a) = at.or_else(|| rows.iter().position(|r| r.is_some())) {
        let mut j = a as isize;
        let mut n = 0;
        while n < AHEAD {
            j += d.signum();
            let Some(r) = usize::try_from(j).ok().and_then(|j| rows.get(j)) else {
                break;
            };
            if let Some(i) = r {
                picks.push(*i);
                n += 1;
            }
        }
    }
    // The one just left, gone back to as often.
    if let Some(i) = at
        .and_then(|a| a.checked_add_signed(-d.signum()))
        .and_then(|j| rows.get(j).copied().flatten())
    {
        picks.push(i);
    }
    // The one that has waited longest on you: ctrl+n picks it.
    if let Some(i) = m
        .order
        .iter()
        .copied()
        .filter(|&i| rush_fleet::Predicates::needs_you(&m.agents[i]))
        .min_by_key(|&i| m.agents[i].job.updated_at)
    {
        picks.push(i);
    }
    let keys: Vec<String> = picks.into_iter().map(|i| m.agents[i].key.clone()).collect();
    toward(m, &keys);
}

/// What's likely shown next is the agents with keys, most likely first:
/// those not yet read ahead are, one at a time; the rest read ahead go,
/// but for the one picked, which the split is about to take.
pub fn toward(m: &mut Model, keys: &[String]) {
    if m.sender().is_none() {
        return;
    }
    if memory_short() {
        let p = m.kept.get::<Predict>();
        p.ready.clear();
        p.want.clear();
        p.left = None;
        if let Some((_, s)) = p.busy.take() {
            s.store(true, Ordering::Relaxed);
        }
        return;
    }
    let mut want: Vec<Want> = Vec::new();
    for k in keys {
        let Some(i) = m.agents.iter().position(|a| a.key == *k) else {
            continue;
        };
        // On screen after this key: the one opened, or beside the list
        // the one picked (the one just left is still there till drawn).
        let showing = m.session.as_ref().is_some_and(|v| v.key == *k)
            || *k == m.sel && m.pane.as_ref().is_some_and(|v| v.key == *k);
        // Kept as the list moved off it, or about to be.
        let kept = m
            .kept
            .get::<Predict>()
            .left
            .as_ref()
            .is_some_and(|v| v.key == *k)
            || *k != m.sel && m.pane.as_ref().is_some_and(|v| v.key == *k && keeps(v));
        if showing || kept || want.iter().any(|w| w.key == *k) {
            continue;
        }
        if let Some(what) = what(m, i) {
            let pid = m.agents[i].pid;
            want.push(Want {
                key: k.clone(),
                pid,
                what,
            });
        }
        if want.len() >= KEEP {
            break;
        }
    }
    let sel = m.sel.clone();
    let p = m.kept.get::<Predict>();
    p.ready
        .retain(|a| a.key == sel || want.iter().any(|w| w.is(a)));
    if let Some((k, s)) = &p.busy
        && !want.iter().any(|w| w.key == *k)
    {
        s.store(true, Ordering::Relaxed);
    }
    p.want = want;
    next(m);
}

/// Reads the most likely one wanted not yet read, when none is being.
fn next(m: &mut Model) {
    let reg = m.reg.clone();
    let p = m.kept.get::<Predict>();
    if p.busy.is_some() {
        return;
    }
    let Some(w) = p
        .want
        .iter()
        .find(|w| !p.ready.iter().any(|a| w.is(a)))
        .cloned()
    else {
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    p.busy = Some((w.key.clone(), stop.clone()));
    p.made += 1;
    rush_draw::perf_note(|| format!("predict read {}", w.key));
    let (key, pid) = (w.key.clone(), w.pid);
    m.later(
        move || match w.what {
            What::Tail(path, decode) => read(&path, decode, stop)
                .map(|(t, s, seen)| (path, From::Tail(Box::new(t)), s, seen)),
            What::History(ad, s, host) => {
                let (f, sess, seen) = read_history(&ad, *s, &host, &stop)?;
                Some((PathBuf::new(), From::Follow(f, seen), sess, None))
            }
            What::Early(a) => {
                let (s, path, was) = crate::screens::session_core::live::read_early(
                    &reg,
                    &a,
                    rush_core::time::now(),
                )?;
                // As it was before the read: one written to during it
                // isn't taken as read.
                let seen = was.as_ref().and_then(stat_of);
                Some((PathBuf::from(path), From::Early, s, seen))
            }
        },
        move |m, r| {
            let p = m.kept.get::<Predict>();
            if p.busy.as_ref().is_some_and(|(b, _)| *b == key) {
                p.busy = None;
            }
            let Some((path, from, sess, seen)) = r else {
                // It can't be read (its transcript gone, a Scratch row's):
                // not wanted again until the list asks anew, or next would
                // read it over and over.
                p.want.retain(|w| w.key != key);
                next(m);
                return Out::Idle;
            };
            {
                let a = Ahead {
                    key,
                    pid,
                    path,
                    seen,
                    from,
                    sess: Box::new(sess),
                };
                if p.want.iter().any(|w| w.is(&a)) {
                    p.ready.retain(|r| r.key != a.key);
                    p.ready.push_front(a);
                    p.ready.truncate(KEEP);
                }
            }
            next(m);
            Out::Idle
        },
    );
}

type Read = (Tail, Session, Option<Stat>);

type Followed = (
    Box<dyn Follow>,
    Session,
    Option<Vec<(PathBuf, Option<Stat>)>>,
);

/// A history read from its start as spawn_history's worker reads it, its
/// slices applied in order, and its follower, to go on from there; with
/// the files it waits on as they were before (None when it waits on
/// something else). None when stopped or it can't be read.
fn read_history(
    ad: &AdapterRef,
    s: adapter::Session,
    host: &str,
    stop: &AtomicBool,
) -> Option<Followed> {
    let s = crate::worker::history_session(s, host);
    let mut f = ad.history_follower()?.follow_history(&s).ok()?;
    let on = f.wait_on();
    let seen = (on.fd.is_none() && on.every.is_none() && !on.paths.is_empty())
        .then(|| on.paths.iter().map(|p| (p.clone(), stat(p))).collect());
    let mut sess = Session::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let sl = f.next(adapter::SLICE).ok()?;
        crate::session::apply_events(&mut sess, &sl.events, sl.reset);
        if !sl.more {
            break;
        }
    }
    Some((f, sess, seen))
}

/// A transcript's end read as the tail's worker reads it first, slices
/// applied in order; None when stopped or it can't be read.
fn read(path: &Path, decode: Decode, stop: Arc<AtomicBool>) -> Option<Read> {
    let mut tail = Tail::from_end(path, TAIL_END, decode);
    tail.stop = Some(stop.clone());
    let mut sess = Session::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let f = tail.fetch_most(TAIL_SLICE).ok()?;
        let more = f.more;
        f.apply(&mut sess);
        if !more {
            break;
        }
    }
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    tail.stop = None;
    // As it is now: one written to since shows only once topped up.
    let seen = stat(path).filter(|s| s.0 == tail.size());
    Some((tail, sess, seen))
}

/// The agents whose conversations are read ahead now, newest first.
#[doc(hidden)]
pub fn ready(m: &mut Model) -> Vec<String> {
    m.kept
        .get::<Predict>()
        .ready
        .iter()
        .map(|a| a.key.clone())
        .collect()
}

/// Reads made so far, and whether one is under way.
#[doc(hidden)]
pub fn reads(m: &mut Model) -> (u64, bool) {
    let p = m.kept.get::<Predict>();
    (p.made, p.busy.is_some())
}
