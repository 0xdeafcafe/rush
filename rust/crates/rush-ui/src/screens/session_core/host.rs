//! The seam a session's messages go through to the agent: rush's host for
//! a session rush runs, the agent's own daemon for one it doesn't. After
//! Go's internal/host/client.go (the ops: Send :483, SendNow :486,
//! SendGuide :490, SendImages :496, EditQueued … HoldQueue :504-530,
//! Allow/Deny :533, Interrupt :541, Tell :592, Stop :666; Lines, the
//! connection's stream), host.InfoEvent's queue fields, host/queue.go
//! (JoinQueue :11), host.go:1563 (IsCommand) and ui/localqueue.go
//! (replyOutside, moveToRushWith, host.Spawn on resume).
//!
//! rush-host is being ported on another branch: until it lands, the UI
//! codes against [`Hosts`] and [`Link`], and [`NoHosts`] says rush can't
//! send yet. The host branch registers its own with [`set_hosts`]; tests
//! use [`Fake`].

use std::sync::{Arc, Mutex};

use rush_core::agent::Agent;
use rush_core::event::Event;
use rush_core::time::Ms;

use crate::model::Model;

/// One thing asked of a session's host (Go's op).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// A message: queued while it works, or (now) into the turn under way,
    /// stopping it first.
    Send {
        text: String,
        images: Vec<String>,
        now: bool,
    },
    /// Into the turn under way at its next step, without stopping it.
    Guide {
        text: String,
        images: Vec<String>,
    },
    Interrupt,
    Allow {
        id: String,
        always: bool,
    },
    Deny {
        id: String,
        message: String,
    },
    EditQueued {
        i: usize,
        was: String,
        text: String,
    },
    RemoveQueued {
        i: usize,
        was: String,
    },
    MoveQueued {
        i: usize,
        was: String,
        to: usize,
    },
    MergeQueued {
        i: usize,
        was: String,
    },
    SendQueued {
        i: usize,
        was: String,
    },
    /// Queued message i into the turn under way, without stopping it.
    SteerQueued {
        i: usize,
        was: String,
    },
    HoldQueue(bool),
    QueueSeparately(bool),
    /// Into a running subagent's inbox, for the end of its step.
    Tell {
        sub: String,
        text: String,
    },
}

/// What the host says to the UI, in order (Go's host.Decode of each
/// line).
#[derive(Clone, Debug)]
pub enum Said {
    /// One of the conversation's events.
    Event(Event),
    /// The time the events after it happened (host.Stamp).
    Stamp(Ms),
    /// You sent this (host.Sent): the agent's copy of it is matched.
    Sent { text: String, images: Vec<String> },
    /// An approval was answered or withdrawn (host.Answered).
    Answered(String),
    /// What the host keeps beside the conversation (host.InfoEvent): the
    /// end of a replay, and each change to its queue or state after.
    Info(Info),
    /// Something went wrong on the host's side (host.ErrorEvent).
    Error(String),
}

/// The host's own state of a session, beside its conversation (the
/// queue fields of Go's host.Info).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    /// Messages waiting for the turn to end, oldest first.
    pub queue: Vec<String>,
    /// Each queued message's images.
    pub queue_images: Vec<Vec<String>>,
    pub queue_held: bool,
    /// Queued messages go one per turn rather than all together.
    pub queue_separate: bool,
    /// Its permission mode, in the agent's words, drawn on the box.
    pub permission_mode: String,
    /// "working", "idle", …
    pub state: String,
    /// Its running subagents take messages straight (Info.Inbox).
    pub inbox: bool,
}

/// Why a reply to a session rush doesn't run didn't go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendErr {
    /// The agent's daemon has let it go (agent.ErrGone): its conversation
    /// carries on in rush mode, with the message as its first turn.
    Gone,
    Failed(String),
}

/// A connection to one hosted session (Go's host.Client).
pub trait Link: Send + Sync {
    /// Asks the host to do op; an error says why it didn't.
    fn op(&self, op: Op) -> Result<(), String>;
    /// Hands what the host says to each, a batch at a time (what's come
    /// in by then), until each returns false or the connection closes.
    /// Blocks: it's run on a worker thread of its own, never the UI's.
    fn follow(&self, each: &mut dyn FnMut(Vec<Said>) -> bool);
    /// Lets go: follow returns.
    fn close(&self);
}

/// Who the session's messages go to: rush's hosts, and the daemons of
/// agents rush doesn't run.
pub trait Hosts: Send + Sync {
    /// Whether rush runs a (Go's Agent.Rush).
    fn hosts(&self, a: &Agent) -> bool;
    /// A connection to a's host. Waits: run off the UI thread.
    fn dial(&self, a: &Agent) -> Result<Arc<dyn Link>, String>;
    /// Sends text to a session rush doesn't run, through its agent's own
    /// daemon (replyOutside). Waits.
    fn reply(&self, a: &Agent, text: &str) -> Result<(), SendErr>;
    /// Starts a stopped rush session again with text as its next message
    /// (host.Spawn with Resume). Waits.
    fn resume(&self, a: &Agent, text: &str, images: &[String]) -> Result<(), String>;
    /// Moves a session rush doesn't run into rush mode, text its first
    /// message (moveToRushWith). Waits.
    fn adopt(&self, a: &Agent, text: &str) -> Result<(), String>;
}

/// Hosts until rush-host lands: rush runs nothing, and says it can't send.
pub struct NoHosts;

const NOT_YET: &str = "rush can't send to agents from here yet";

impl Hosts for NoHosts {
    fn hosts(&self, _: &Agent) -> bool {
        false
    }
    fn dial(&self, _: &Agent) -> Result<Arc<dyn Link>, String> {
        Err(NOT_YET.into())
    }
    fn reply(&self, _: &Agent, _: &str) -> Result<(), SendErr> {
        Err(SendErr::Failed(NOT_YET.into()))
    }
    fn resume(&self, _: &Agent, _: &str, _: &[String]) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    fn adopt(&self, _: &Agent, _: &str) -> Result<(), String> {
        Err(NOT_YET.into())
    }
}

/// Where the session's messages go from now on in m.
pub fn set_hosts(m: &mut Model, h: Arc<dyn Hosts>) {
    m.kept::<super::Core>().hosts = Some(h);
}

/// The hosts m sends through.
pub fn hosts(m: &mut Model) -> Arc<dyn Hosts> {
    m.kept::<super::Core>()
        .hosts
        .get_or_insert_with(|| Arc::new(NoHosts))
        .clone()
}

/// The queue as one message, each part its own (host/queue.go:11).
pub fn join_queue(items: &[String]) -> String {
    if items.len() == 1 {
        return items[0].clone();
    }
    let n = items.len();
    let mut b = format!(
        "{n} messages, queued while you worked. Each is its own message; take them in order."
    );
    for (i, it) in items.iter().enumerate() {
        b.push_str(&format!("\n\n[Message {} of {n}]\n{it}", i + 1));
    }
    b
}

/// Whether t is a slash command, not words (host.go:1563): "/compact",
/// not "/usr/bin" or "/etc/x.conf".
pub fn is_command(t: &str) -> bool {
    let t = t.trim();
    let Some(rest) = t.strip_prefix('/') else {
        return false;
    };
    let first = rest.split_whitespace().next().unwrap_or("");
    !first.contains(['/', '.'])
}

/// Hosts and a link for tests: every op and reply is written down, and
/// the session's lines are handed over by the test.
#[derive(Default)]
pub struct Fake {
    /// Agents by key that rush runs.
    pub hosted: Mutex<Vec<String>>,
    /// Ops asked of the link, in order.
    pub ops: Mutex<Vec<Op>>,
    /// Replies sent outside rush, as (key, text).
    pub replies: Mutex<Vec<(String, String)>>,
    pub resumed: Mutex<Vec<(String, String)>>,
    pub adopted: Mutex<Vec<(String, String)>>,
    /// The next op or reply fails with this.
    pub fail: Mutex<Option<String>>,
    /// The next reply finds the job gone.
    pub gone: Mutex<bool>,
    /// What follow hands over, once.
    pub lines: Mutex<Vec<Said>>,
    pub dials: Mutex<usize>,
}

impl Fake {
    pub fn new() -> Arc<Fake> {
        Arc::new(Fake::default())
    }

    pub fn host(&self, key: &str) {
        self.hosted.lock().unwrap().push(key.to_string());
    }

    pub fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }

    fn failed(&self) -> Option<String> {
        self.fail.lock().unwrap().take()
    }
}

/// The fake's link: ops go to the fake's list.
struct FakeLink(Arc<Fake>);

impl Link for FakeLink {
    fn op(&self, op: Op) -> Result<(), String> {
        if let Some(e) = self.0.failed() {
            return Err(e);
        }
        self.0.ops.lock().unwrap().push(op);
        Ok(())
    }
    fn follow(&self, each: &mut dyn FnMut(Vec<Said>) -> bool) {
        let lines = std::mem::take(&mut *self.0.lines.lock().unwrap());
        if !lines.is_empty() {
            each(lines);
        }
    }
    fn close(&self) {}
}

impl Hosts for Arc<Fake> {
    fn hosts(&self, a: &Agent) -> bool {
        self.hosted.lock().unwrap().contains(&a.key)
    }
    fn dial(&self, _: &Agent) -> Result<Arc<dyn Link>, String> {
        *self.dials.lock().unwrap() += 1;
        if let Some(e) = self.failed() {
            return Err(e);
        }
        Ok(Arc::new(FakeLink(self.clone())))
    }
    fn reply(&self, a: &Agent, text: &str) -> Result<(), SendErr> {
        if std::mem::take(&mut *self.gone.lock().unwrap()) {
            return Err(SendErr::Gone);
        }
        if let Some(e) = self.failed() {
            return Err(SendErr::Failed(e));
        }
        let r = (a.key.clone(), text.to_string());
        self.replies.lock().unwrap().push(r);
        Ok(())
    }
    fn resume(&self, a: &Agent, text: &str, _: &[String]) -> Result<(), String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let r = (a.key.clone(), text.to_string());
        self.resumed.lock().unwrap().push(r);
        Ok(())
    }
    fn adopt(&self, a: &Agent, text: &str) -> Result<(), String> {
        let r = (a.key.clone(), text.to_string());
        self.adopted.lock().unwrap().push(r);
        Ok(())
    }
}
