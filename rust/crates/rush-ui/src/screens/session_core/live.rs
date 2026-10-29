//! A session rush hosts, live: one connection to its host, to the session
//! showing, opened as it's shown and let go as it goes; what the host says
//! taken into the conversation a batch at a time. After Go's
//! internal/ui/rushmode.go (syncHost :2003, dropHost :2078, onHostOpen
//! :2136, next :1958, onHostLines :2192, applyHostLine :1937), the host's
//! info as convo/model.go:474 takes it (a model switched mid-session),
//! and sleep_host.go (parkHost :21, reconnectSleeping :65,
//! resolveHostClose :149): a host that goes to sleep lets its socket go
//! and the conversation stays; a message wakes it.
//!
//! The connection's lines are read on a worker thread of its own and
//! come to the UI on a lane of one (DESIGN.md "Threads"): the next batch
//! is read while the UI applies the last, and a flood never holds up
//! typing. Go waits up to 3 s on connecting for the replay to come in
//! whole (takeReplay); here the replay's batches are drawn as they land,
//! at most every 250 ms (frame/framehold.rs).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rush_convo::Session;

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
    let Some(key) = open else {
        return;
    };
    let Some(a) = agent(m, &key).cloned() else {
        return;
    };
    let h = hosts(m);
    if a.pid == 0 || !h.hosts(&a) {
        return;
    }
    let c = conn(m, &key);
    if c.link.is_some() || c.dialing || c.dialed_pid == a.pid {
        return;
    }
    c.dialing = true;
    c.dialed_pid = a.pid;
    m.later(move || h.dial(&a), move |m, r| opened(m, &key, r));
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
    conn(m, key).dialing = false;
    let link = match r {
        Ok(l) => l,
        Err(e) => {
            m.fail(&format!("couldn't open the session: {e}"));
            // Zen moves past it for a while (rushmode.go:2149).
            crate::screens::social::zen::open_failed(m, key);
            return Out::Draw;
        }
    };
    conn(m, key).sleeping = false;
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
    if let Some(v) = view_of(m, key) {
        v.tail = None; // its worker stops with its handle
        v.sess = Session::new();
        v.read = false;
        v.err.clear();
        v.pos = Pos::End;
    }
    follow(m, key, link, r#gen, stop);
    Out::Draw
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
    for s in batch {
        match s {
            Said::Event(ev) => v.sess.apply(&ev, now),
            Said::Stamp(t) => now = t,
            Said::Sent { text, images } => {
                v.sess
                    .sent(text, images.into_iter().map(Into::into).collect(), now)
            }
            Said::Answered(id) => v.sess.settle(&id),
            Said::Info(i) => {
                // A model switched mid-session comes only as the host's
                // info: the agent's init said the one it started on.
                if !i.model.is_empty() && i.model != c.info.model {
                    v.sess.model = i.model.clone();
                }
                c.info = i;
            }
            Said::Error(e) => errs.push(e),
        }
    }
    v.read = true;
    c.out.settle(&v.sess, c.info.queue.len(), started);
    let asleep = closed && c.info.sleeping;
    if closed {
        c.link = None;
        c.sleeping = asleep;
        if core.linked == key {
            core.linked.clear();
        }
    }
    for e in errs {
        m.fail(&e); // the session on screen: its error is yours to see now
    }
    if closed && !asleep {
        resolve_close(m, key);
    }
    Out::Draw
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
