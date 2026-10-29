//! The seam a session's messages go through to the agent: rush's host for
//! a session rush runs, the agent's own daemon for one it doesn't. After
//! Go's internal/host/client.go (the ops: Send :483, SendNow :486,
//! SendGuide :490, SendImages :496, EditQueued … HoldQueue :504-530,
//! Allow/Deny :533, Interrupt :541, Tell :592, Stop :666; Lines, the
//! connection's stream; StopTask, Background), host.InfoEvent's queue fields, host/queue.go
//! (JoinQueue :11), host.go:1563 (IsCommand) and ui/localqueue.go
//! (replyOutside, moveToRushWith, host.Spawn on resume).
//!
//! The UI codes against [`Hosts`] and [`Link`]: the rush binary sets its
//! own, over rush-host's client, with [`set_hosts`] (which also lets the
//! screens start sessions, start.rs); without one [`NoHosts`] says rush
//! can't send or start. Tests use [`Fake`], or rush-host's fake host
//! through the binary's. Starting (host.Spawn, sleep.go's Ensure and
//! Dial on a wake) is [`Hosts::start`] and [`Hosts::wake`].

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
    /// Stops one subagent or background task by its task id
    /// (client.go StopTask).
    StopTask {
        id: String,
    },
    /// Moves the tool call the turn waits on into the background, by its
    /// call id; "" moves every one (client.go Background).
    Background {
        id: String,
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
    /// When the queue got stuck behind a turn that can't take a message
    /// (Kimi's), which may not end for hours; 0 when it isn't
    /// (Info.QueueStuck).
    pub queue_stuck: Ms,
    /// Queued messages go one per turn rather than all together.
    pub queue_separate: bool,
    /// Its permission mode, in the agent's words, drawn on the box.
    pub permission_mode: String,
    /// "working", "idle", …
    pub state: String,
    /// Its running subagents take messages straight (Info.Inbox).
    pub inbox: bool,
    /// The model it runs now: one switched mid-session comes only here.
    pub model: String,
    /// Its agent's process; 0 once it's gone (Info.ClaudePID).
    pub agent_pid: i64,
    /// What its host can do (Info.Proto): one before 3 can't move a task
    /// to the background.
    pub proto: i64,
    /// Why its agent last failed (Info.Error).
    pub error: String,
    /// Its host has gone to sleep, the session kept: a message wakes it
    /// (Info.Sleeping).
    pub sleeping: bool,
    /// How its requests are paid for: plan, overage or metered; "" until
    /// the agent says (Info.Billing).
    pub billing: String,
    /// When the rush its host runs was written, 0 for a host from before
    /// it said (Info.Exe.Mod): a rush installed since makes it older.
    pub exe_mod: Ms,
    /// The login its agent runs on, when it says (Info.Login).
    pub login: String,
    /// Set while a usage limit has stopped it (Info.Limit).
    pub limit: Option<Limit>,
    /// Set while an API error is being retried, or has given up
    /// (Info.Retry).
    pub retry: Option<Retry>,
    /// When the prompt cache its last request wrote expires; 0 unknown
    /// (Info.CacheWarm).
    pub cache_warm: Ms,
    /// Set while it's away or looping, or an away has just ended
    /// (Info.Away).
    pub away: Option<Box<crate::screens::list_extras::away::Away>>,
    /// How it began, for the card its conversation opens on.
    pub began: Box<Began>,
}

/// How a hosted session began, for the card its conversation opens on
/// (opening.go): its rush id, agent, effort and folder, when it started,
/// the plugin that started it, and the rush sessions that started it as
/// their subagent or beside them, by id (Meta spawnedBy, startedFrom).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Began {
    pub id: String,
    pub kind: String,
    pub effort: String,
    pub cwd: String,
    pub started_at: Ms,
    pub started_by: String,
    pub spawned_by: String,
    pub started_from: String,
}

impl Info {
    /// Whether a rush newer than the one its host started from is
    /// installed (installed: the installed rush's write time); a host from
    /// before it said is never older (stale.go:41 Stale).
    pub fn stale(&self, installed: Ms) -> bool {
        self.exe_mod != 0 && installed > self.exe_mod
    }
}

/// A usage limit that stopped a session (host.Limit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limit {
    /// When it resets; 0 unknown.
    pub resets_at: Ms,
    /// five_hour, seven_day, …
    pub window: String,
    /// It carries on at the reset.
    pub continue_: bool,
    /// Whether to carry on is still yours to decide.
    pub ask: bool,
}

impl Limit {
    /// The limit in words (rushmode.go:5548 limitText).
    pub fn text(&self) -> String {
        let mut t = match self.window.as_str() {
            "five_hour" => "5h limit",
            "seven_day" => "7d limit",
            "seven_day_opus" => "7d Opus limit",
            _ => "usage limit",
        }
        .to_string();
        if self.resets_at != 0 {
            t += &format!(
                " · resets {}",
                crate::screens::accounts::rows::clock(self.resets_at, false)
            );
        }
        if self.continue_ {
            t += " · continues then";
        } else if !self.ask {
            t += " · waiting for you";
        }
        t
    }
}

/// An API error being retried (host.Retry).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Retry {
    pub attempt: i64,
    pub max: i64,
    /// When it tries again.
    pub next: Ms,
    /// Retries ran out: a message from you retries.
    pub gave_up: bool,
    pub why: String,
    /// The API can't be reached.
    pub offline: bool,
    /// Its prompt cache expired and it waits for proof the connection
    /// holds.
    pub proof: bool,
}

/// A new session rush hosts, as the screens start it (the host.Config
/// startHosted and configAs make).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Start {
    /// The agent it runs.
    pub kind: String,
    pub cwd: String,
    /// The first message; "" starts it waiting.
    pub prompt: String,
    pub images: Vec<String>,
    /// Its name; "" names it from the first message, else afresh for its
    /// folder, to be named by the first message it's sent (host.NameFrom,
    /// FreshName, NameFirst).
    pub name: String,
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    /// "key" pays with its provider's API key.
    pub billing: String,
    /// The account it runs on, (name, folder): Claude Code's switched-in
    /// one; None is the agent's first.
    pub account: Option<(String, String)>,
    pub limit_mode: String,
    pub lean: bool,
    pub profile: String,
    /// In a worktree of its own, made by its host before its agent starts.
    pub own_worktree: bool,
    pub system_prompt: String,
    /// The plugin that started it, rush's own (Config.StartedBy): the
    /// list says so, and finds it again after a reload.
    pub started_by: String,
    /// Why, in a few words, for its row (Config.Meta's "why").
    pub why: String,
}

/// A session started (hostStartedMsg).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Started {
    /// Its rush id.
    pub id: String,
    pub name: String,
    /// The account it runs on, as its key has it.
    pub acct: String,
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

/// A session rush hosted for another's shell or spawn_agent call, as its
/// host's files say (spawns.go hostedRun): who it is, what it was asked,
/// where its agent writes, and whether it still works.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spawned {
    /// The rush session it runs as.
    pub id: String,
    /// The call that started it, when its harness said (Meta spawnStep).
    pub step: String,
    pub kind: String,
    /// Its agent's name: "Codex", "Claude Code".
    pub name: String,
    pub prompt: String,
    pub model: String,
    pub cwd: String,
    /// The session its agent writes, and when it began (what the
    /// spawn_agent call asked at, when it says).
    pub transcript: String,
    pub born: Ms,
    /// Idle or stopped, it doesn't (hostedLive).
    pub live: bool,
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
    /// Whether this rush can start sessions at all.
    fn can_start(&self) -> bool {
        false
    }
    /// Starts a new session (startHosted's host.Spawn): checks the agent
    /// can run here first (Config.UseAgent). Waits.
    fn start(&self, _: &Start) -> Result<Started, String> {
        Err(NOT_STARTING.into())
    }
    /// Whether a's host has gone to sleep, keeping the session
    /// (host.ReadInfo's Sleeping). Reads the disk.
    fn sleeping(&self, _: &Agent) -> bool {
        false
    }
    /// Wakes a's sleeping host and, unless text and images are both
    /// empty, sends them over the new connection, once (wakeHost:
    /// host.Ensure, Dial, SendImages). Waits.
    fn wake(
        &self,
        _: &Agent,
        _text: &str,
        _images: &[String],
        _now: bool,
    ) -> Result<Arc<dyn Link>, String> {
        Err(NOT_YET.into())
    }
    /// Sends text to the rush session id without a view on it, waking it
    /// if it sleeps: queued while it works, or now, stopping its turn
    /// (sendHostedID). Waits.
    fn send_to(&self, _id: &str, _text: &str, _now: bool) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    /// Whether an agent of kind takes a message mid-turn: one that can't
    /// (Kimi) leaves it queued until its turn ends (host.TakesMidTurn).
    fn takes_mid_turn(&self, _kind: &str) -> bool {
        true
    }
    /// The sessions rush hosted for rush session parent's shell or its
    /// spawn_agent calls (hostedRuns: Meta spawnedBy). Reads the disk.
    fn spawned(&self, _parent: &str) -> Vec<Spawned> {
        Vec::new()
    }
    /// Stops rush session id's host with the agents it spawned, as ctrl+x
    /// does: none works on unseen (host.StopAll). Waits.
    fn stop_all(&self, _id: &str) -> Result<(), String> {
        Err(NOT_STOPPING.into())
    }
    /// Stops rush session id's host, keeping its conversation; one not
    /// running is stopped already (host.Dial, Client.Stop). Waits.
    fn stop(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Hosts until rush-host lands: rush runs nothing, and says it can't send.
pub struct NoHosts;

const NOT_YET: &str = "rush can't send to agents from here yet";
/// What starting says with no hosts.
pub const NOT_STARTING: &str = "rush can't start sessions here yet";
/// What stopping a rush session says with no hosts.
pub const NOT_STOPPING: &str = "rush can't stop sessions from here yet";

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

/// Where the session's messages go from now on in m, and what starts
/// sessions: with hosts that can start them, the box, #new, #use, the
/// sheets and #ask start through them (start.rs).
pub fn set_hosts(m: &mut Model, h: Arc<dyn Hosts>) {
    let starts = h.can_start();
    m.kept::<super::Core>().hosts = Some(h);
    if starts {
        super::start::install(m);
    }
}

/// The hosts set in m, to read while drawing; None until set.
pub fn hosts_of(m: &Model) -> Option<Arc<dyn Hosts>> {
    m.kept.peek::<super::Core>().and_then(|c| c.hosts.clone())
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
    /// Sessions started, in order; each is started as fake-<n>.
    pub starts: Mutex<Vec<Start>>,
    /// Agents by key whose host sleeps.
    pub asleep: Mutex<Vec<String>>,
    /// Wakes, as (key, text, now).
    pub wakes: Mutex<Vec<(String, String, bool)>>,
    /// Sends without a view, as (id, text, now).
    pub sent_to: Mutex<Vec<(String, String, bool)>>,
    /// Kinds that take nothing mid-turn.
    pub not_mid_turn: Mutex<Vec<String>>,
    /// The sessions rush hosted for a parent's shell.
    pub spawned: Mutex<Vec<Spawned>>,
    /// Sessions stopped with what they spawned, then those stopped alone,
    /// by id.
    pub stopped_all: Mutex<Vec<String>>,
    pub stopped: Mutex<Vec<String>>,
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
    fn can_start(&self) -> bool {
        true
    }
    fn start(&self, s: &Start) -> Result<Started, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let mut starts = self.starts.lock().unwrap();
        starts.push(s.clone());
        let name = if s.name.is_empty() {
            s.prompt
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            s.name.clone()
        };
        Ok(Started {
            id: format!("fake-{}", starts.len()),
            name,
            acct: "default".into(),
        })
    }
    fn sleeping(&self, a: &Agent) -> bool {
        self.asleep.lock().unwrap().contains(&a.key)
    }
    fn wake(
        &self,
        a: &Agent,
        text: &str,
        _: &[String],
        now: bool,
    ) -> Result<Arc<dyn Link>, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        self.asleep.lock().unwrap().retain(|k| *k != a.key);
        let w = (a.key.clone(), text.to_string(), now);
        self.wakes.lock().unwrap().push(w);
        Ok(Arc::new(FakeLink(self.clone())))
    }
    fn send_to(&self, id: &str, text: &str, now: bool) -> Result<(), String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let s = (id.to_string(), text.to_string(), now);
        self.sent_to.lock().unwrap().push(s);
        Ok(())
    }
    fn spawned(&self, _parent: &str) -> Vec<Spawned> {
        self.spawned.lock().unwrap().clone()
    }
    fn takes_mid_turn(&self, kind: &str) -> bool {
        !self.not_mid_turn.lock().unwrap().iter().any(|k| k == kind)
    }
    fn stop_all(&self, id: &str) -> Result<(), String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        self.stopped_all.lock().unwrap().push(id.to_string());
        Ok(())
    }
    fn stop(&self, id: &str) -> Result<(), String> {
        self.stopped.lock().unwrap().push(id.to_string());
        Ok(())
    }
}
