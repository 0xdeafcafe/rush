//! A session rush hosts, live: one connection to its host, to the session
//! showing, opened as it's shown and let go as it goes; what the host says
//! taken into the conversation a batch at a time. After Go's
//! internal/ui/rushmode.go (syncHost :2003, dropHost :2078, onHostOpen
//! :2136, next :1958, onHostLines :2192, applyHostLine :1937), the host's
//! info as convo/model.go:474 takes it (a model switched mid-session),
//! and sleep_host.go (parkHost :21, reconnectSleeping :65,
//! resolveHostClose :149, openSavedHost :172): a host that goes to sleep
//! lets its socket go and the conversation stays; a message wakes it. A
//! stopped one shown has its host's info read from disk, with no socket.
//!
//! The connection's lines are read on a worker thread of its own and
//! come to the UI on a lane of one (DESIGN.md "Threads"): the next batch
//! is read while the UI applies the last, and a flood never holds up
//! typing. Go waits up to 3 s on connecting for the replay to come in
//! whole (takeReplay); here the replay's batches are drawn as they land,
//! at most every 250 ms (frame/framehold.rs).
//!
//! Once reached, the host's info is read again (a host that just
//! restarted has written its own start by then), and the conversation is
//! the transcript up to where the host's replay begins, then the replay:
//! a session that took over an existing conversation shows it, and a
//! long one whose replay was trimmed shows all of it. Its end is read
//! first; the whole of it after, off the UI thread, with the host's lines
//! that came meanwhile put on top (rushmode.go:1832-1858 openHost,
//! twopass.go:187-260 hostPre, preFor, base, tail; :78-120 onWhole,
//! takeWhole; :366-397 readWholeHost; rushmode.go:2208 the lines kept).
//! A split pane's session that reports an error is news, not a failure
//! (rushmode.go:2096-2104). A control request's answer goes to whoever
//! asked (asksheets.go:41 onReply); a count of the context is kept, with
//! what it holds the session never used (convo/model.go:496, slim.go);
//! a rewind's message goes back in the box once its host is reached
//! again (rushmode.go:2156).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rush_convo::{Decode, Session, history_from, history_start};
use rush_core::adapter::{self, AdapterRef, Profile, Registry, migrated};
use rush_core::time::Ms;

use super::host::{Link, Said, hosts};
use super::{Core, agent, conn, core, open_key, queue};
use crate::frame::Out;
use crate::model::Model;
use crate::session::Pos;
use crate::worker::Note;

/// After each reading of the list: the link follows the session showing,
/// and queues whose agent went idle go.
pub fn on_fleet(m: &mut Model) {
    sync(m);
    queue::flush(m);
}

/// Keeps one connection open, to the hosted session showing, and lets it
/// go when the Session moves on (syncHost). A host that closed is dialled
/// again only once another process runs the session.
pub fn sync(m: &mut Model) {
    let open = showing(m);
    let linked = core(m).linked.clone();
    if !linked.is_empty() && open.as_deref() != Some(linked.as_str()) {
        drop_link(m, &linked);
    }
    if open.as_deref() != Some(core(m).saved.as_str()) {
        core(m).saved.clear();
    }
    let Some(key) = open else {
        return;
    };
    let Some(a) = agent(m, &key).cloned() else {
        return;
    };
    let h = hosts(m);
    if !h.hosts(&a) {
        return;
    }
    if a.pid == 0 {
        read_saved(m, &key, a, h);
        return;
    }
    let c = conn(m, &key);
    if c.link.is_some() || c.dialing || c.dialed_pid == a.pid {
        return;
    }
    c.dialing = true;
    c.dialed_pid = a.pid;
    let reg = m.reg.clone();
    early(m, &key, &a);
    rush_draw::perf_note(|| format!("live dial {key}"));
    m.later(
        move || {
            // A host asleep has let its socket go: its saved info, not a
            // dial that can only fail (openHost's info.Sleeping,
            // rushmode.go:1870).
            if let Some(i) = h.saved(&a).filter(|i| i.sleeping) {
                return Err(Dial::Asleep(Box::new(i)));
            }
            let link = h.dial(&a).map_err(Dial::Failed)?;
            rush_draw::perf_note(|| "live dialled".into());
            // Read once it's reached: what came before its replay.
            let mut pre = read_pre(&reg, &a, rush_core::time::now());
            rush_draw::perf_note(|| format!("live pre found {}", pre.path));
            let sess = pre.tail();
            rush_draw::perf_note(|| {
                format!("live pre tail turns={} at={:?}", sess.turns.len(), pre.at)
            });
            Ok((link, Some((pre, sess))))
        },
        move |m, r: Result<Dialed, Dial>| match r {
            Ok((l, pre)) => opened_with(m, &key, Ok(l), pre),
            Err(Dial::Failed(e)) => opened_with(m, &key, Err(e), None),
            Err(Dial::Asleep(info)) => {
                let c = conn(m, &key);
                c.dialing = false;
                if c.link.is_some() {
                    return Out::Idle;
                }
                c.sleeping = true;
                c.info = *info;
                Out::Draw
            }
        },
    );
}

/// Rust-only: a live session's transcript end, read beside the dial so
/// what's there shows at once rather than "connecting…" while a busy
/// host answers and what came before its replay is read. Only taken
/// while the view has nothing and the host isn't reached yet; the host's
/// own conversation replaces it once its replay is in (take).
fn early(m: &mut Model, key: &str, a: &rush_core::agent::Agent) {
    // With no loop (tests, --render) the dial is done at once: nothing to
    // wait on.
    if m.sender().is_none() || view_of(m, key).is_none_or(|v| !v.sess.turns.is_empty()) {
        return;
    }
    let reg = m.reg.clone();
    let a = a.clone();
    let key = key.to_string();
    m.later(
        move || {
            let now = rush_core::time::now();
            let mut p = read_pre(&reg, &a, now);
            if p.path.is_empty() || p.decode.is_none() || p.other.is_some() {
                // Not resumed: what the transcript holds is all the replay's.
                let id = &a.job.id;
                let info = rush_host::read_info(id).ok();
                let mut cfg = rush_host::read_config(id).ok().unwrap_or_default();
                cfg.resume = true;
                let path = if a.job.transcript_path.is_empty() {
                    a.history.clone()
                } else {
                    a.job.transcript_path.clone()
                };
                p = pre_for(&reg, info.as_ref(), Some(&cfg), path, a.acct.clone(), now);
            }
            if p.path.is_empty() || p.decode.is_none() || p.other.is_some() {
                return None;
            }
            // Its end up to now, not just what came before the replay.
            p.before = now + 60_000;
            p.at = None;
            let s = p.tail();
            rush_draw::perf_note(|| format!("live early turns={}", s.turns.len()));
            (!s.turns.is_empty()).then_some(s)
        },
        move |m, s: Option<Session>| {
            let Some(s) = s else { return Out::Idle };
            let linked = conn(m, &key).link.is_some();
            let Some(v) = view_of(m, &key) else {
                return Out::Idle;
            };
            if !matches!(v.source, crate::session::Source::Hosted) {
                return Out::Idle;
            }
            if linked {
                // Reached first, but nothing from it yet: its conversation
                // goes behind the end until its replay is in.
                if v.read {
                    return Out::Idle;
                }
                let behind = std::mem::replace(&mut v.sess, s);
                v.read = true;
                v.pos = Pos::End;
                let c = conn(m, &key);
                c.shadow = Some(behind);
                c.shadow_until = Some(std::time::Instant::now() + REPLAY_MOST);
                return Out::Draw;
            }
            if !v.sess.turns.is_empty() {
                return Out::Idle;
            }
            v.sess = s;
            v.read = true;
            v.pos = Pos::End;
            conn(m, &key).early = true;
            Out::Draw
        },
    );
}

/// A stopped rush session shown: its host's info as it was left, read
/// once off the UI thread, with no socket and no wake (openSavedHost,
/// sleep_host.go:172-190). The header's state word (idle while its host
/// sleeps, rushmode.go:2861) and the box's mode (rushmode.go:3725) read
/// it.
fn read_saved(
    m: &mut Model,
    key: &str,
    a: rush_core::agent::Agent,
    h: Arc<dyn super::host::Hosts>,
) {
    if core(m).saved == key {
        return;
    }
    let c = conn(m, key);
    if c.link.is_some() || c.dialing {
        return;
    }
    core(m).saved = key.to_string();
    let key = key.to_string();
    m.later(
        move || h.saved(&a),
        move |m, info: Option<super::host::Info>| {
            let Some(info) = info else {
                return Out::Idle;
            };
            let c = conn(m, &key);
            if c.link.is_some() {
                return Out::Idle; // reached since: its host's word is newer
            }
            c.sleeping = info.sleeping;
            // The conversation keeps it too, as Go's c.sess.Info = info:
            // its permission mode on the Overview.
            if let Some(v) = view_of(m, &key) {
                take_info(&mut v.sess.info, &info);
            }
            conn(m, &key).info = info;
            Out::Draw
        },
    );
}

/// The session whose host the screen shows: the one opened, else the
/// one beside the list (Go's m.host is either).
pub fn showing(m: &Model) -> Option<String> {
    open_key(m).or_else(|| m.pane.as_ref().map(|v| v.key.clone()))
}

/// The view of session key on screen: opened, or beside the list.
fn view_of<'a>(m: &'a mut Model, key: &str) -> Option<&'a mut crate::session::SessionView> {
    [m.session.as_mut(), m.pane.as_mut()]
        .into_iter()
        .flatten()
        .find(|v| v.key == key)
}

/// Lets go of session key's connection (dropHost): its worker stops.
pub fn drop_link(m: &mut Model, key: &str) {
    let c = conn(m, key);
    c.stop.store(true, Ordering::Relaxed);
    c.dialed_pid = 0;
    c.early = false;
    c.shadow = None;
    c.shadow_until = None;
    if let Some(l) = c.link.take() {
        l.close();
    }
    let core = core(m);
    if core.linked == key {
        core.linked.clear();
    }
}

/// A connection made, or not (onHostOpen): the conversation is the
/// host's from now on, its transcript no longer followed.
pub(super) fn opened(m: &mut Model, key: &str, r: Result<Arc<dyn Link>, String>) -> Out {
    opened_with(m, key, r, None)
}

/// opened, the conversation starting from pre's: what its transcript
/// held before the host's replay, read up to where it begins.
pub(super) fn opened_with(
    m: &mut Model,
    key: &str,
    r: Result<Arc<dyn Link>, String>,
    pre: Option<(Pre, Session)>,
) -> Out {
    conn(m, key).dialing = false;
    let link = match r {
        Ok(l) => l,
        Err(e) => {
            // Dialled again at the next reading, as Go's syncHost does
            // once onHostOpen has cleared hostOpening.
            conn(m, key).dialed_pid = 0;
            m.fail(&format!("couldn't open the session: {e}"));
            return Out::Draw;
        }
    };
    // Woken, it's the same connection (carryHostView keeps backShown);
    // dialled afresh, an away that ended opens its back sheet again.
    let c = conn(m, key);
    if !c.sleeping {
        c.back_shown = false;
    }
    c.sleeping = false;
    if showing(m).as_deref() != Some(key) {
        link.close(); // opened too late: the Session has moved on
        return Out::Idle;
    }
    let core = core(m);
    core.link_gen += 1;
    let r#gen = core.link_gen;
    core.linked = key.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let c = conn(m, key);
    c.link = Some(link.clone());
    c.link_gen = r#gen;
    c.stop = stop.clone();
    c.since.clear();
    let (pre, sess) = pre.map_or((None, Session::new()), |(p, s)| (Some(p), s));
    // Its end only: the whole of it is read next, the host's lines kept
    // meanwhile to put on top (onReplay's keep).
    let whole = pre.filter(|p| sess.partial && p.any());
    c.keep = whole.is_some();
    let early =
        std::mem::take(&mut c.early) && view_of(m, key).is_some_and(|v| !v.sess.turns.is_empty());
    let c = conn(m, key);
    c.shadow = None;
    c.shadow_until = None;
    if early {
        // The transcript's end stays on screen while the host's own
        // conversation comes in behind it.
        c.shadow = Some(sess);
        c.shadow_until = Some(std::time::Instant::now() + REPLAY_MOST);
        if let Some(v) = view_of(m, key) {
            v.tail = None;
            v.err.clear();
        }
    } else if let Some(v) = view_of(m, key) {
        v.tail = None; // its worker stops with its handle
        v.sess = sess;
        v.read = false;
        v.err.clear();
        v.pos = Pos::End;
    }
    follow(m, key, link, r#gen, stop.clone());
    // A rewind's message back in the box, and the box given the keys
    // (onHostOpen's m.rewound, paneFocus = true): one beside the list is
    // opened, as focusing the pane is here.
    if let Some(d) = super::core(m).rewound.remove(key) {
        conn(m, key).input.set(&d);
        if m.session.as_ref().is_none_or(|v| v.key != key)
            && m.pane.as_ref().is_some_and(|v| v.key == key)
            && let Some(i) = m.agents.iter().position(|a| a.key == key)
        {
            m.open(i);
        }
    }
    branches(m, key);
    rush_draw::perf_note(|| format!("live opened whole={}", whole.is_some()));
    if let Some(pre) = whole {
        let key = key.to_string();
        m.later(
            move || {
                let w = pre.whole(Some(stop));
                rush_draw::perf_note(|| format!("live whole read turns={}", w.turns.len()));
                w
            },
            move |m, whole| took_whole(m, &key, r#gen, whole),
        );
    }
    Out::Draw
}

/// Reads the paths session key's rewinds left, off the UI thread, for
/// /rewind to offer (rewindsheet.go:100-105).
fn branches(m: &mut Model, key: &str) {
    let Some(a) = agent(m, key).cloned() else {
        return;
    };
    let h = hosts(m);
    let key = key.to_string();
    m.later(
        move || h.branches(&a),
        move |m, b| {
            conn(m, &key).branches = b;
            Out::Idle
        },
    );
}

/// The whole transcript before the replay, read: the host's lines that
/// came while it was read go on top, and it takes the end's place, what
/// was picked or scrolled to staying put (onWhole, takeWhole).
pub(super) fn took_whole(m: &mut Model, key: &str, r#gen: u64, whole: Session) -> Out {
    let t0 = std::time::Instant::now();
    let out = took_whole_now(m, key, r#gen, whole);
    rush_draw::perf_note(|| format!("live took whole us={}", t0.elapsed().as_micros()));
    out
}

fn took_whole_now(m: &mut Model, key: &str, r#gen: u64, mut whole: Session) -> Out {
    let started = m.now;
    let core = m.kept.get::<Core>();
    let Some(c) = core
        .conns
        .get_mut(key)
        .filter(|c| c.link_gen == r#gen && c.keep)
    else {
        return Out::Idle;
    };
    if c.stop.load(Ordering::Relaxed) {
        return Out::Idle; // let go while it read: it gave up part way
    }
    c.keep = false;
    let since = std::mem::take(&mut c.since);
    let mut model = String::new();
    let mut now = started;
    for s in since {
        apply_said(&mut whole, s, &mut now, &mut model);
    }
    if let Some(sh) = c.shadow.as_mut() {
        // Still behind the transcript's end: the whole takes the shadow's
        // place, and shows when the replay is in.
        whole.info = std::mem::take(&mut sh.info);
        *sh = whole;
        return Out::Idle;
    }
    let Some(v) = view_of(m, key) else {
        return Out::Idle;
    };
    if !v.sess.partial {
        return Out::Idle;
    }
    v.take_whole(whole);
    Out::Draw
}

/// One thing the host said, into a conversation (applyHostLine); the
/// model as the host's info last said it, for a switch mid-session. What
/// isn't the conversation's (the info's queue, errors) the caller takes.
fn apply_said(s: &mut Session, said: Said, now: &mut Ms, model: &mut String) {
    match said {
        Said::Event(ev) => s.apply(&ev, *now),
        Said::Stamp(t) => *now = t,
        Said::Sent { text, images } => {
            s.sent(text, images.into_iter().map(Into::into).collect(), *now)
        }
        Said::Answered(id) => s.settle(&id),
        Said::Info(i) => {
            if !i.model.is_empty() && i.model != *model {
                s.model = i.model.clone();
            }
            take_info(&mut s.info, &i);
            model.clone_from(&i.model);
        }
        Said::Error(_) | Said::Reply { .. } | Said::Context(_) => {}
    }
}

/// Why a host wasn't reached: asleep (its saved info), or the dial failed.
enum Dial {
    Asleep(Box<super::host::Info>),
    Failed(String),
}

/// A host reached, and what its conversation held before its replay.
type Dialed = (Arc<dyn Link>, Option<(Pre, Session)>);

/// How much of a transcript's end a hosted session opens on: a few turns,
/// read in milliseconds; the rest after (tailBytes).
pub const PRE_TAIL: u64 = 2 << 20;

/// The longest the transcript's end shown early stays in front of the
/// host's conversation coming in (Go's replayMost, rushmode.go:1899).
const REPLAY_MOST: std::time::Duration = std::time::Duration::from_secs(3);

/// What a hosted session's conversation held before its host's replay,
/// and where to read it (hostPre): its transcript up to `before`, or
/// another agent's history through its adapter.
#[derive(Clone, Default)]
pub struct Pre {
    pub path: String,
    pub before: Ms,
    /// The transcript it forked from, read when its own has nothing yet.
    pub fork: String,
    decode: Option<Decode>,
    /// Another agent's: its adapter, its session, and up to when.
    other: Option<(AdapterRef, Box<adapter::Session>, Ms)>,
    /// Where its end was read from; None until it has been.
    at: Option<u64>,
}

/// What session a's host's files say came before its replay, read now:
/// its info again, as it is once reached, and its config.
fn read_pre(reg: &Registry, a: &rush_core::agent::Agent, now: Ms) -> Pre {
    let id = &a.job.id;
    let info = rush_host::read_info(id).ok();
    let cfg = rush_host::read_config(id).ok();
    let mut path = if a.job.transcript_path.is_empty() {
        a.history.clone()
    } else {
        a.job.transcript_path.clone()
    };
    if info
        .as_ref()
        .is_some_and(|i| i.session_id != a.job.session_id)
    {
        path.clear(); // the list may still point at a branch left by rewind
    }
    pre_for(reg, info.as_ref(), cfg.as_ref(), path, a.acct.clone(), now)
}

/// How a hosted session with this info and config reads what came before
/// its replay (preFor): nothing unless it resumed a conversation or its
/// replay was trimmed.
pub fn pre_for(
    reg: &Registry,
    info: Option<&rush_host::Info>,
    cfg: Option<&rush_host::Config>,
    mut path: String,
    mut acct: Profile,
    now: Ms,
) -> Pre {
    let mut p = Pre::default();
    if acct.dir.is_empty()
        && let Some(c) = cfg
    {
        // Live rows for other harnesses may only carry an account name;
        // their saved config owns the folder their history is in.
        acct.dir.clone_from(&c.account.dir);
    }
    if let Some(i) = info
        && !i.session_id.is_empty()
        && !i.cwd.is_empty()
    {
        // The list may not have caught up with a rewind yet, and a
        // session that moved to a worktree keeps its transcript where it
        // started: it's looked for, not just named from its cwd.
        let found = reg.find_transcript(migrated(&i.kind).as_str(), &acct, &i.cwd, &i.session_id);
        if !found.is_empty() {
            path = found;
        }
    }
    let trimmed = info.is_some_and(|i| !i.replay_from.is_zero());
    let resume = cfg.is_some_and(|c| c.resume || trimmed);
    if let Some(i) = info {
        let kind = migrated(&i.kind);
        if !reg.reads_as_claude(kind.as_str()) {
            if resume && let Some(ad) = reg.get(kind.as_str()) {
                let started = if trimmed { i.replay_from } else { i.started_at };
                let s = adapter::Session {
                    kind: kind.clone(),
                    id: i.session_id.clone(),
                    transcript: path,
                    profile: Profile {
                        kind,
                        dir: acct.dir.clone(),
                        ..Profile::default()
                    },
                    ..adapter::Session::default()
                };
                p.other = Some((ad, Box::new(s), started.to_ms()));
            }
            return p;
        }
    }
    if !resume || path.is_empty() {
        return p;
    }
    let kind = info.map_or_else(|| migrated(""), |i| migrated(&i.kind));
    p.decode = crate::session::decoder(reg, kind.as_str());
    p.before = now;
    if let Some(i) = info
        && !i.started_at.is_zero()
    {
        p.before = [i.started_at, i.rewound_at, i.replay_from]
            .into_iter()
            .filter(|t| !t.is_zero())
            .map(|t| t.to_ms())
            .max()
            .unwrap_or(now);
    }
    if let Some(c) = cfg.filter(|c| !c.from.is_empty())
        && let Some(dir) = Path::new(&path).parent()
    {
        p.fork = dir
            .join(format!("{}.jsonl", c.from))
            .to_string_lossy()
            .into_owned();
    }
    p.path = path;
    p
}

impl Pre {
    /// Whether anything came before the replay to read.
    pub fn any(&self) -> bool {
        self.other.is_some() || !self.path.is_empty() && self.decode.is_some()
    }

    /// The conversation's end before the replay: the last PRE_TAIL bytes
    /// of its transcript before `before`, Partial when that left some out
    /// (tail). Reads the disk.
    pub fn tail(&mut self) -> Session {
        if let Some((ad, s, before)) = &self.other {
            if let Some(r) = ad.history_tail_before_reader()
                && let Ok((evs, cut)) = r.history_tail_before(s, PRE_TAIL as i64, *before)
            {
                let mut out = Session::new();
                for ev in &evs {
                    out.apply(ev, 0);
                }
                out.partial = cut;
                return out;
            }
            return self.whole(None);
        }
        let Some(decode) = self.decode.clone().filter(|_| !self.path.is_empty()) else {
            return Session::new();
        };
        let at = *self
            .at
            .get_or_insert_with(|| history_start(Path::new(&self.path), self.before, PRE_TAIL));
        let s = history_from(&self.path, self.before, at, None, decode);
        if s.turns.is_empty() && at == 0 && !self.fork.is_empty() {
            self.path = std::mem::take(&mut self.fork);
            self.at = None;
            return self.tail();
        }
        s
    }

    /// All of it before the replay, read off the UI thread; it gives up
    /// once stop is set (readWholeHost).
    pub fn whole(&self, stop: Option<Arc<AtomicBool>>) -> Session {
        if let Some((ad, s, before)) = &self.other {
            let mut out = Session::new();
            if let Some(r) = ad.history_reader()
                && let Ok(evs) = adapter::read_history(r, s, *before)
            {
                for ev in &evs {
                    out.apply(ev, 0);
                }
            }
            return out;
        }
        let Some(decode) = self.decode.clone().filter(|_| !self.path.is_empty()) else {
            return Session::new();
        };
        history_from(&self.path, self.before, 0, stop, decode)
    }
}

/// Reads the host's lines on a worker of their own, a batch at a time,
/// on a lane of one. With no loop (tests, `rush --render`) what the link
/// has is taken here and now.
fn follow(m: &mut Model, key: &str, link: Arc<dyn Link>, r#gen: u64, stop: Arc<AtomicBool>) {
    let Some(tx) = m.sender() else {
        link.follow(&mut |batch| {
            take(m, key, r#gen, batch, false);
            true
        });
        return;
    };
    let tx = tx.lane(1);
    let key = key.to_string();
    let spawned = std::thread::Builder::new()
        .name("rush-host-lines".into())
        .spawn(move || {
            link.follow(&mut |batch| {
                if stop.load(Ordering::Relaxed) {
                    return false;
                }
                let key = key.clone();
                tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                    take(m, &key, r#gen, batch, false)
                })))
            });
            if !stop.load(Ordering::Relaxed) {
                tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                    take(m, &key, r#gen, Vec::new(), true)
                })));
            }
        });
    if spawned.is_err() {
        m.fail("couldn't start a thread for the session");
    }
}

/// Takes a batch of the host's lines into the conversation (onHostLines);
/// closed, the host has gone. A batch from a connection let go is
/// dropped.
pub fn take(m: &mut Model, key: &str, r#gen: u64, batch: Vec<Said>, closed: bool) -> Out {
    let t0 = std::time::Instant::now();
    let n = batch.len();
    let out = take_batch(m, key, r#gen, batch, closed);
    rush_draw::perf_note(|| format!("live batch n={n} us={}", t0.elapsed().as_micros()));
    out
}

fn take_batch(m: &mut Model, key: &str, r#gen: u64, batch: Vec<Said>, closed: bool) -> Out {
    let started = m.now;
    let Some(v) = [m.session.as_mut(), m.pane.as_mut()]
        .into_iter()
        .flatten()
        .find(|v| v.key == key)
    else {
        return Out::Idle;
    };
    let core = m.kept.get::<Core>();
    let Some(c) = core.conns.get_mut(key).filter(|c| c.link_gen == r#gen) else {
        return Out::Idle;
    };
    let mut now = started;
    let mut errs = Vec::new();
    let mut replies = Vec::new();
    let mut counted = false;
    let mut replayed = false;
    if c.keep {
        // For the whole transcript being read to take in too.
        c.since.extend(batch.iter().cloned());
    }
    {
        // Behind the transcript's end shown early, or the view's own.
        let sess = c.shadow.as_mut().unwrap_or(&mut v.sess);
        for s in batch {
            match s {
                // A model switched mid-session comes only as the host's info:
                // the agent's init said the one it started on.
                Said::Info(i) => {
                    let mut model = c.info.model.clone();
                    apply_said(sess, Said::Info(i.clone()), &mut now, &mut model);
                    c.info = i;
                    replayed = true; // the info that ends the replay
                }
                Said::Error(e) => errs.push(e),
                Said::Reply { id, body } => replies.push((id, body)),
                Said::Context(u) => {
                    c.context = Some(u);
                    counted = true;
                }
                s => apply_said(sess, s, &mut now, &mut String::new()),
            }
        }
    }
    if c.shadow.is_some()
        && (replayed
            || closed
            || c.shadow_until
                .is_some_and(|t| std::time::Instant::now() >= t))
        && let Some(sh) = c.shadow.take()
    {
        // The replay's in: the host's conversation takes the early end's
        // place, where it was scrolled to staying put.
        c.shadow_until = None;
        let info = sh.info.clone();
        v.take_whole(sh);
        v.sess.info = info;
    }
    v.read = true;
    let sess = c.shadow.as_ref().unwrap_or(&v.sess);
    c.out.settle(sess, c.info.queue.len(), started);
    if counted && let Some(u) = &c.context {
        // What it carries and has never used, as the count stands now
        // (Unused: worked out once a count, not each frame).
        c.unused = Some(super::unused::unused(sess, u));
    }
    let asleep = closed && c.info.sleeping;
    if closed {
        c.link = None;
        c.sleeping = asleep;
        if core.linked == key {
            core.linked.clear();
        }
    }
    if !errs.is_empty() {
        let shown = showing(m).as_deref() == Some(key);
        let name = agent(m, key).map_or("a session".to_string(), |a| a.display_name.clone());
        for e in errs {
            if shown {
                m.fail(&e); // the session on screen: its error is yours to see now
            } else {
                // A split pane's session: news, as it isn't the one you're on.
                m.news(&format!("{name} reported an error: {e}"), "");
            }
        }
    }
    if closed && !asleep {
        resolve_close(m, key);
    }
    // Each answer to whoever asked; another rush's isn't ours (onReply).
    for (id, body) in replies {
        if let Some(done) = conn(m, key).asks.remove(&id) {
            done(m, body);
        }
    }
    crate::screens::cmdbar::awaycmd::back_from_away(m, key);
    Out::Draw
}

/// What the conversation keeps of the host's info: how it began, for
/// the card it opens on (convo/model.go:483 s.Info = ev.Info).
pub fn take_info(to: &mut rush_convo::Info, info: &super::host::Info) {
    let i = &*info.began;
    to.id.clone_from(&i.id);
    to.kind.clone_from(&i.kind);
    to.effort.clone_from(&i.effort);
    to.permission_mode.clone_from(&info.permission_mode);
    to.started_at = i.started_at;
    to.started_by.clone_from(&i.started_by);
    to.spawned_by.clone_from(&i.spawned_by);
    to.started_from.clone_from(&i.started_from);
    if !i.cwd.is_empty() {
        to.cwd.clone_from(&i.cwd);
    }
    if !info.model.is_empty() {
        to.model.clone_from(&info.model);
    }
}

/// The connection closed with no word of sleep: the info may have lost
/// the race with the end of the socket, so the host's info on disk says
/// (resolveHostClose). Asleep, the conversation stays as it is (parkHost);
/// else the session is read from disk again, as any other is.
fn resolve_close(m: &mut Model, key: &str) {
    let Some(a) = agent(m, key).cloned() else {
        return;
    };
    let h = hosts(m);
    conn(m, key).waking = true; // the box keeps what's typed till it's known
    let key = key.to_string();
    m.later(
        move || h.sleeping(&a),
        move |m, asleep| {
            let c = conn(m, &key);
            c.waking = false;
            if asleep {
                c.sleeping = true;
                return Out::Idle;
            }
            // dropHost: read as any other session is, from its transcript.
            if m.pane.as_ref().is_some_and(|v| v.key == key) && open_key(m).is_none() {
                m.pane = None; // read again as the list now has it
                return Out::Draw;
            }
            if open_key(m).as_deref() == Some(key.as_str())
                && let Some(i) = m.agents.iter().position(|a| a.key == key)
            {
                let pos = m.session.as_ref().map(|v| v.pos);
                m.open(i);
                if let (Some(v), Some(p)) = (m.session.as_mut(), pos) {
                    v.pos = p;
                }
            }
            Out::Draw
        },
    );
}

/// What a message to a sleeping host does once it's tried to wake it
/// (reconnectSleeping's result): connected, the link is the session's
/// from now on and what was sent went over it; not, what was sent is put
/// back in the box when it's empty.
pub(super) fn woke(
    m: &mut Model,
    key: &str,
    r: Result<std::sync::Arc<dyn Link>, String>,
    text: String,
) -> Out {
    conn(m, key).waking = false;
    match r {
        Ok(link) => {
            if open_key(m).as_deref() != Some(key) {
                link.close(); // the Session moved on while it woke
                return Out::Idle;
            }
            if let Some(a) = agent(m, key) {
                conn(m, key).dialed_pid = a.pid;
            }
            opened(m, key, Ok(link))
        }
        Err(e) => {
            let c = conn(m, key);
            c.out.failed();
            if c.input.is_empty() {
                c.input.insert_str(&text);
            }
            m.fail(&e);
            Out::Draw
        }
    }
}
