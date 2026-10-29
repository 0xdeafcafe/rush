//! Messages waiting for a session: its host's queue when rush runs it,
//! else rush's own, which goes as one message once the agent is idle; and
//! the queue's keys once a message in it is picked. After Go's
//! internal/ui/localqueue.go (localQueue :18, queueOf :39, canQueue :55,
//! queueLocal :82, withImages :105, flushLocalQueues :121, sendLocal
//! :148, editLocal :178), model.go:1229 (localQueueFailed) and
//! sessionviews.go (queueHint :32, queueSel :58, queueKey :68, queueEdit
//! :152, holdQueue :210, sendQueueNow :228) and rushmode.go paneDock's
//! queue card :3532-3547 (when it sends; held, or stuck behind a turn
//! that can't take it).

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;

use super::host::{Op, SendErr, hosts, join_queue};
use super::{agent, busy, conn, core};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// Messages for a session rush doesn't run (or a watched subagent):
/// they wait here and go as one.
#[derive(Clone, Debug, Default)]
pub struct LocalQueue {
    pub items: Vec<String>,
    pub held: bool,
    pub sent_at: Ms,
    /// When the oldest waiting message was queued.
    pub since: Ms,
    /// After a failed send, when to try again.
    pub retry: Ms,
    pub fails: u32,
    /// A subagent's steps when its last message went.
    pub at: usize,
}

/// A session's queue as the dock shows it, whichever side keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Queued {
    pub items: Vec<String>,
    pub held: bool,
    pub separate: bool,
    /// rush keeps it, not a host.
    pub local: bool,
    /// Its turn can't take a message, and may not end for hours
    /// (Info.QueueStuck).
    pub stuck: bool,
}

/// Session key's queue.
pub fn queue_of(m: &mut Model, key: &str) -> Queued {
    let c = core(m);
    if let Some(cn) = c.conns.get(key)
        && cn.hosted()
    {
        let i = &cn.info;
        return Queued {
            items: i.queue.clone(),
            held: i.queue_held,
            separate: i.queue_separate,
            local: false,
            stuck: i.queue_stuck != 0,
        };
    }
    match c.local_q.get(key) {
        Some(q) => Queued {
            items: q.items.clone(),
            held: q.held,
            separate: false,
            local: true,
            stuck: false,
        },
        None => Queued {
            local: true,
            ..Queued::default()
        },
    }
}

/// Whether rush can hold messages for a: a session it doesn't run, not
/// open in a terminal, not past.
pub fn can_queue(m: &mut Model, a: &Agent) -> bool {
    !hosts(m).hosts(a) && !a.interactive && !a.past
}

/// Adds text to session key's own queue.
pub fn queue_local(m: &mut Model, key: &str, text: &str, now: Ms) -> usize {
    let q = core(m).local_q.entry(key.to_string()).or_default();
    if q.items.is_empty() {
        q.since = now;
    }
    q.items.push(text.to_string());
    q.items.len()
}

/// A message with image files added as paths; Claude Code opens them.
pub fn with_images(text: &str, images: &[String]) -> String {
    let mut b = text.to_string();
    for p in images {
        if !b.is_empty() {
            b.push('\n');
        }
        b.push_str(&format!("[image: {p}]"));
    }
    b
}

/// Sends each waiting queue whose agent has gone idle (or has waited
/// long enough while it works); asks to be woken when the next is due.
pub fn flush(m: &mut Model) {
    let now = m.now;
    let keys: Vec<String> = core(m)
        .local_q
        .iter()
        .filter(|(k, q)| {
            !k.contains('\x00')
                && !q.items.is_empty()
                && !q.held
                && now - q.sent_at >= 10_000
                && now >= q.retry
        })
        .map(|(k, _)| k.clone())
        .collect();
    let mut wake: Option<Ms> = None;
    for key in keys {
        let Some(a) = agent(m, &key).cloned() else {
            continue;
        };
        if !can_queue(m, &a) {
            continue;
        }
        // Idle (or stopped: the message resumes it), it goes now. Working,
        // after a short wait, so a burst goes as one; waiting on you, it
        // holds until you've answered.
        if a.job.state == State::Blocked {
            continue;
        }
        let since = core(m).local_q[&key].since;
        if busy(&a) && now - since < 15_000 {
            wake = Some(wake.map_or(since + 15_000, |w: Ms| w.min(since + 15_000)));
            continue;
        }
        send_local(m, &key, &a);
    }
    if let Some(w) = wake {
        let d = std::time::Duration::from_millis((w - now).max(0) as u64);
        m.ask_wake(std::time::Instant::now() + d);
    }
}

/// Sends session key's whole queue to a as one message.
pub fn send_local(m: &mut Model, key: &str, a: &Agent) {
    let now = m.now;
    let q = core(m).local_q.entry(key.to_string()).or_default();
    let items = std::mem::take(&mut q.items);
    q.sent_at = now;
    let text = join_queue(&items);
    let h = hosts(m);
    let (a, key) = (a.clone(), key.to_string());
    let name = a.display_name.clone();
    m.later(
        move || {
            let r = h.reply(&a, &text);
            (a, text, r)
        },
        move |m, (a, text, r)| {
            match r {
                Ok(()) => m.noted(&format!("queued messages sent: {name} has them")),
                Err(SendErr::Gone) => super::keys::adopt(m, &a, &text),
                Err(SendErr::Failed(e)) => failed(m, &key, &name, items, &e),
            }
            Out::Draw
        },
    );
}

/// A queue that didn't go: back at the front, tried again in 30s, and
/// held once it has failed three times running.
fn failed(m: &mut Model, key: &str, name: &str, items: Vec<String>, err: &str) {
    let now = m.now;
    let q = core(m).local_q.entry(key.to_string()).or_default();
    let mut all = items;
    all.append(&mut q.items);
    q.items = all;
    q.fails += 1;
    q.retry = now + 30_000;
    if q.fails >= 3 {
        q.held = true;
        q.fails = 0;
        m.news(
            &format!(
                "queued messages for {name} are held: sending failed three times, {err} · alt+h releases them"
            ),
            "",
        );
        return;
    }
    m.news(
        &format!("queued messages for {name} were not sent: {err} · trying again in 30s"),
        "",
    );
}

/// The queued message picked in the dock, from the conversation's pick.
pub fn sel(sel: &str, n: usize) -> Option<usize> {
    let i: usize = sel.strip_prefix("q:")?.parse().ok()?;
    (i < n).then_some(i)
}

/// The keys for a picked queued message (queueHint).
pub fn hint(q: &Queued, w: usize) -> rush_core::text::Line {
    let hold = if q.held { "let go" } else { "hold" };
    let mut pairs = vec![
        ("space", "edit"),
        ("shift+↑↓", "merge up/down"),
        ("[ ]", "move"),
        ("s", "send this now"),
        ("g", "steer with it"),
        ("G", "steer with all"),
        ("⌫", "drop"),
        ("h", hold),
    ];
    if !q.local {
        pairs.push((
            "o",
            if q.separate {
                "all together"
            } else {
                "one per turn"
            },
        ));
    }
    pairs.push(("esc", "done"));
    keys_fit(w, &pairs)
}

/// What a queue edit does (queueEdit's op).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Move(usize),
    Merge,
    Send,
    Drop,
}

/// A key on a picked queued message; Pass when it isn't one (queueKey).
pub fn key(m: &mut Model, key: &str, s: &str) -> Out {
    let q = queue_of(m, key);
    let Some(i) = m.session.as_ref().and_then(|v| sel(&v.sel, q.items.len())) else {
        return Out::Pass;
    };
    let set_sel = |m: &mut Model, s: String| {
        if let Some(v) = m.session.as_mut() {
            v.sel = s;
        }
    };
    match s {
        "space" | " " | "e" => {
            let c = conn(m, key);
            if !c.input.is_empty() {
                m.flash("Finish or stash your message before editing the queue");
                return Out::Draw;
            }
            // Edited in the box; enter there saves it back in place. The
            // queue holds meanwhile, so it doesn't go half edited.
            c.input.set(&q.items[i]);
            c.edit_q = i + 1;
            c.edit_was = q.items[i].clone();
            c.edit_held = !q.held;
            set_sel(m, String::new());
            if !q.held {
                hold(m, key, true);
            }
        }
        "shift+up" => {
            if i == 0 {
                m.flash_err("nothing before it to merge with");
                return Out::Draw;
            }
            set_sel(m, format!("q:{}", i - 1));
            edit(m, key, Edit::Merge, i - 1);
        }
        "shift+down" | "m" | "alt+m" => edit(m, key, Edit::Merge, i),
        "[" | "]" | "alt+up" | "alt+down" => {
            let down = s == "]" || s == "alt+down";
            let to = if down { i + 1 } else { i.wrapping_sub(1) };
            if to >= q.items.len() {
                return Out::Idle;
            }
            set_sel(m, format!("q:{to}"));
            edit(m, key, Edit::Move(to), i);
        }
        "s" => edit(m, key, Edit::Send, i),
        "backspace" | "delete" | "ctrl+x" => edit(m, key, Edit::Drop, i),
        "g" | "G" | "shift+g" => {
            // Steering: one message, or all, into the turn under way.
            steer(m, key, i, s != "g");
        }
        "h" | "alt+h" => {
            conn(m, key).edit_held = false; // yours now, not the edit's
            hold(m, key, !q.held);
        }
        "o" | "alt+o" => {
            if q.local {
                return Out::Idle;
            }
            let on = !q.separate;
            conn(m, key).info.queue_separate = on;
            run(m, key, Op::QueueSeparately(on));
        }
        _ => return Out::Pass,
    }
    Out::Draw
}

/// Changes the queue here at once, so the view and the next key see it,
/// and tells the host of a rush session, naming the message by its place
/// and the text it had (queueEdit).
pub fn edit(m: &mut Model, key: &str, op: Edit, i: usize) {
    let mut items = queue_of(m, key).items;
    let hosted = conn(m, key).hosted();
    let mut qi = conn(m, key).info.queue_images.clone();
    qi.resize(items.len(), Vec::new());
    let Some(was) = items.get(i).cloned() else {
        return;
    };
    match op {
        Edit::Move(to) => {
            let it = items.remove(i);
            items.insert(to, it);
            let im = qi.remove(i);
            qi.insert(to, im);
        }
        Edit::Merge => {
            if i + 1 >= items.len() {
                m.flash_err("nothing after it to merge with");
                return;
            }
            let next = items.remove(i + 1);
            items[i] = format!("{}\n\n{next}", items[i]);
            let im = qi.remove(i + 1);
            qi[i].extend(im);
        }
        Edit::Send | Edit::Drop => {
            items.remove(i);
            qi.remove(i);
            // The pick stays where it was, on the next message.
            let s = if items.is_empty() {
                String::new()
            } else {
                format!("q:{}", i.min(items.len() - 1))
            };
            if let Some(v) = m.session.as_mut() {
                v.sel = s;
            }
        }
    }
    if !hosted {
        core(m).local_q.entry(key.to_string()).or_default().items = items;
        if op == Edit::Send
            && let Some(a) = agent(m, key).cloned()
        {
            super::keys::reply(m, &a, was);
        }
        return;
    }
    let c = conn(m, key);
    c.info.queue = items;
    c.info.queue_images = qi;
    let op = match op {
        Edit::Move(to) => Op::MoveQueued { i, was, to },
        Edit::Merge => Op::MergeQueued { i, was },
        Edit::Send => Op::SendQueued { i, was },
        Edit::Drop => Op::RemoveQueued { i, was },
    };
    run(m, key, op);
}

/// Steers the turn under way with queued message i, or all of them,
/// taken out of the queue (steerQueue). Not hosted, they go as they would
/// anyway: they're read at the next step already.
fn steer(m: &mut Model, key: &str, i: usize, all: bool) {
    if !conn(m, key).hosted() {
        if all {
            send_now(m, key, "");
        } else {
            edit(m, key, Edit::Send, i);
        }
        return;
    }
    let queued = queue_of(m, key).items;
    let Some(mut was) = queued.get(i).cloned() else {
        return;
    };
    let mut i = i;
    let c = conn(m, key);
    let mut sel = String::new();
    if all {
        c.info.queue.clear();
        c.info.queue_images.clear();
        i = 0;
    } else {
        c.info.queue.remove(i);
        if i < c.info.queue_images.len() {
            c.info.queue_images.remove(i);
        }
        if !c.info.queue.is_empty() {
            sel = format!("q:{}", i.min(c.info.queue.len() - 1));
        }
    }
    if let Some(v) = m.session.as_mut() {
        v.sel = sel;
    }
    let what = if all && queued.len() > 1 {
        "the queue"
    } else {
        "it"
    };
    m.flash(&format!(
        "steering with {what} · it reads it at its next step, nothing stopped"
    ));
    let mut ops = Vec::new();
    if all {
        ops.extend((1..queued.len()).map(|k| Op::MergeQueued {
            i: 0,
            was: queued[..k].join("\n\n"),
        }));
        let text = join_queue(&queued);
        ops.push(Op::EditQueued {
            i: 0,
            was: queued.join("\n\n"),
            text: text.clone(),
        });
        was = text;
    }
    ops.push(Op::SteerQueued { i, was });
    run_all(m, key, ops);
}

/// Holds the queue or lets it go (holdQueue).
pub fn hold(m: &mut Model, key: &str, on: bool) {
    if !conn(m, key).hosted() {
        let q = core(m).local_q.entry(key.to_string()).or_default();
        q.held = on;
        q.fails = 0;
        q.retry = 0;
        return;
    }
    conn(m, key).info.queue_held = on;
    run(m, key, Op::HoldQueue(on));
}

/// Sends everything waiting straight away, and extra (what's in the box)
/// after it, as one message, in the order you wrote them (sendQueueNow).
pub fn send_now(m: &mut Model, key: &str, extra: &str) {
    let queued = queue_of(m, key).items;
    let mut items = queued.clone();
    if !extra.is_empty() {
        items.push(extra.to_string());
    }
    if items.is_empty() {
        m.flash("nothing queued");
        return;
    }
    if !conn(m, key).hosted() {
        let Some(a) = agent(m, key).cloned() else {
            return;
        };
        if !can_queue(m, &a) {
            return;
        }
        let q = core(m).local_q.entry(key.to_string()).or_default();
        q.items = items;
        q.fails = 0;
        q.retry = 0;
        send_local(m, key, &a);
        return;
    }
    let c = conn(m, key);
    c.info.queue.clear();
    c.info.queue_images.clear();
    if queued.is_empty() {
        run(
            m,
            key,
            Op::Send {
                text: extra.to_string(),
                images: Vec::new(),
                now: true,
            },
        );
        return;
    }
    // The queue merged into its first message, so their images go too,
    // then written out as items and sent as one (joinQueued).
    let mut ops: Vec<Op> = (1..queued.len())
        .map(|k| Op::MergeQueued {
            i: 0,
            was: queued[..k].join("\n\n"),
        })
        .collect();
    let text = join_queue(&items);
    ops.push(Op::EditQueued {
        i: 0,
        was: queued.join("\n\n"),
        text: text.clone(),
    });
    ops.push(Op::SendQueued { i: 0, was: text });
    run_all(m, key, ops);
}

/// Asks the host of session key to do op, off the UI thread; a failure
/// is flashed (hostCmd).
pub fn run(m: &mut Model, key: &str, op: Op) {
    run_all(m, key, vec![op]);
}

/// Asks for ops in order, stopping at the first that fails.
pub fn run_all(m: &mut Model, key: &str, ops: Vec<Op>) {
    let Some(link) = conn(m, key).link.clone() else {
        return;
    };
    m.later(
        move || ops.into_iter().try_for_each(|op| link.op(op)),
        |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                m.fail(&e);
                Out::Draw
            }
        },
    );
}

/// How the queue card says when it goes.
pub fn when(m: &mut Model, key: &str, q: &Queued) -> rush_core::text::Line {
    let now = m.now;
    // Yours if you moved it, not a fixed ctrl+enter (rushmode.go:3543-3546).
    let send_now = super::spawned::send_now_key(m);
    if q.held {
        // Never silently stuck: say it's held and how it goes.
        return line([
            sp(" · held", YELLOW),
            sp(format!(" · {send_now} sends it now"), DIM),
        ]);
    }
    if !q.local && q.stuck {
        // Its turn won't take it, and may not end for hours.
        return line([
            sp(" · its turn can't take it", YELLOW),
            sp(format!(" · {send_now} stops the turn and sends it"), DIM),
        ]);
    }
    if q.local
        && let Some(lq) = core(m).local_q.get(key)
        && now < lq.retry
    {
        let left = ((lq.retry - now) + 500) / 1000 * 1000;
        return line([sp(
            format!(" · send failed, trying again in {}", dur(left)),
            YELLOW,
        )]);
    }
    if q.local {
        return line([sp(" · sends within 15s, or when it's idle", DIM)]);
    }
    line([sp(" · sends when this turn ends", DIM)])
}
