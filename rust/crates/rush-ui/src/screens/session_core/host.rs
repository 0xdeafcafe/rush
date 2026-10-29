//! The seam a session's messages go through to the agent: rush's host for
//! a session rush runs, the agent's own daemon for one it doesn't. After
//! Go's internal/host/client.go (the ops: Send :483, SendNow :486,
//! SendGuide :490, SendImages :496, EditQueued … HoldQueue :504-530,
//! Allow/Deny :533, Interrupt :541, Tell :592, Stop :666; Lines, the
//! connection's stream; StopTask, Background; DismissDecisions :558,
//! SetWithout :569, Rewind :575, CompactedIfUnchanged :597, Ask :702),
//! host.Reply and host.Context as the UI takes them, host.InfoEvent's
//! queue fields and what the cards read (Effort, Without,
//! PermissionModes, Decisions), host/queue.go (JoinQueue :11),
//! host.go:1563 (IsCommand) and ui/localqueue.go (replyOutside,
//! moveToRushWith, host.Spawn on resume). The cards' rewinds,
//! compactions, forks and hand-offs are [`Hosts`] calls (carry.rs).
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
use crate::screens::session_cards::fork::Fork;
use crate::screens::session_cards::rewind::Branch;

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
    /// Allows request id with input in place of what it asked (a
    /// question's answers, as JSON; client.go Allow's input).
    Answer {
        id: String,
        input: String,
        always: bool,
    },
    /// Switches its permission mode (client.go SetPermissionMode).
    Permission(String),
    /// Sets the decisions card id aside, unanswered: nothing is sent
    /// (client.go DismissDecisions).
    DismissDecisions(String),
    /// What it goes without from its agent's next start, at once when
    /// it's idle; empty puts everything back (client.go SetWithout).
    Without(Vec<String>),
    /// Passes a control request, JSON, to its agent; the answer comes back
    /// as [`Said::Reply`] with id (client.go Ask).
    Ask {
        id: String,
        req: String,
    },
    /// Switches its model from the next turn; "" its default
    /// (client.go SetModel, choicesheet.go:101 setArg).
    Model(String),
    /// Switches its effort from the agent's next start; "" its default
    /// (client.go SetEffort).
    Effort(String),
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
    /// The answer to a control request passed through with
    /// [`Op::Ask`] (host.Reply): its body, or the error its agent gave.
    Reply {
        id: String,
        body: Result<serde_json::Value, String>,
    },
    /// What fills its context window, as last counted (host.Context): its
    /// agent counts it at each turn's end.
    Context(Box<rush_core::usage::Context>),
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
    /// What it runs with now, as the cards read it.
    pub now: Box<Now>,
}

/// What a hosted session runs with now, beside its queue and state: its
/// effort, what it goes without, its modes, its open decisions, and the
/// conversation it's on (host.Info's Effort, Without, PermissionModes,
/// Decisions, SessionID, UpdatedAt).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Now {
    /// Its effort, and what it goes without from its agent's starts
    /// (#slim).
    pub effort: String,
    pub without: Vec<String>,
    /// The permission modes its agent says it has.
    pub permission_modes: Vec<crate::screens::session_cards::Mode>,
    /// The decisions block its last turn ended with, still open.
    pub decisions: Option<rush_core::event::Question>,
    /// The conversation it runs now, and when its host last changed it,
    /// to the nanosecond: a compaction is only for the conversation it
    /// was made from.
    pub session_id: String,
    pub updated_ns: i128,
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

/// A session's conversation carried on in a new one (handOverMessage):
/// the new one as configAs makes it, named; the one it carries on from,
/// by its rush id; and that conversation, told.
#[derive(Clone, Debug, Default)]
pub struct HandOver {
    pub start: Start,
    pub from: String,
    /// The one it carries on from is rush's own: it stops once the new
    /// one has its conversation, a switch rather than a copy.
    pub retire: bool,
    pub conv: rush_core::adapter::Conversation,
    /// Your next instruction, sent with it; "" none.
    pub message: String,
}

/// A hand-over made: the new session, and whether the one it carries on
/// from was stopped (hostStartedMsg's retire).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HandedOver {
    pub started: Started,
    pub retired: bool,
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
    /// message: one open in a terminal is copied, any other stopped
    /// where it runs and resumed under rush's host (moveToRushWith's
    /// work). Waits.
    fn adopt(&self, a: &Agent, text: &str) -> Result<Started, String>;
    /// Whether this rush can start sessions at all.
    fn can_start(&self) -> bool {
        false
    }
    /// Starts a new session (startHosted's host.Spawn): checks the agent
    /// can run here first (Config.UseAgent). Waits.
    fn start(&self, _: &Start) -> Result<Started, String> {
        Err(NOT_STARTING.into())
    }
    /// Starts a new session carrying another's conversation on, and stops
    /// the one it came from once the new one runs when that's rush's
    /// own and still quiet (handOverMessage's work). Waits.
    fn hand_over(&self, _: &HandOver) -> Result<HandedOver, String> {
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
    /// Moves the rush session id to dir, listed there from now on, and
    /// names it afresh for dir when retitle (cdHosted, retitleHosted):
    /// best effort, as an older host ignores both. Waits.
    fn cd(&self, _id: &str, _dir: &str, _retitle: bool) -> Result<(), String> {
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
    /// Starts rush session id's agent program again, resuming its
    /// conversation, signed in as whoever is in use now and with the
    /// settings as they are now; text, when there is any, is sent so it
    /// starts at once, else it starts with the next message. One not
    /// running says so (restart's run, restartClaude). Waits.
    fn restart(&self, _id: &str, _name: &str, _text: &str) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    /// Has rush session id carry on from conversation session_id, a copy
    /// cut before one of your messages (resume), a fresh one, or one of its
    /// branches, the one it leaves kept as left. Waits until its host has
    /// hung up, or says why it won't; true when its host was from an older
    /// rush and was restarted on this one already rewound (rewindHost,
    /// hangUp). Waits.
    fn rewind(
        &self,
        _id: &str,
        _session_id: &str,
        _resume: bool,
        _left: &Branch,
    ) -> Result<bool, String> {
        Err(NOT_YET.into())
    }
    /// Has rush session id carry on in a fresh conversation, session_id,
    /// that starts with prompt, only while it's still the idle
    /// conversation expected (compactBy's carry: CompactedIfUnchanged,
    /// hangUp). Waits.
    fn compacted(
        &self,
        _id: &str,
        _session_id: &str,
        _prompt: &str,
        _left: &Branch,
        _expected: &Info,
    ) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    /// The paths earlier rewinds left a's session, newest first, those
    /// whose transcript is still there (rewindsheet.go:100-105). Reads
    /// the disk.
    fn branches(&self, _a: &Agent) -> Vec<Branch> {
        Vec::new()
    }
    /// Starts a fork of a's session: f's model, effort and mode, or as it
    /// runs now, as its host was started, or the dispatch default for its
    /// agent (forksheet.go:350 start's host.Spawn). Waits.
    fn fork(&self, _a: &Agent, _f: &Fork) -> Result<Started, String> {
        Err(NOT_STARTING.into())
    }
    /// Starts a new session on agent kind in cwd, named name, with prompt
    /// (a conversation told for it) as its first message (handoffTo's
    /// host.Spawn). Waits.
    fn handoff(
        &self,
        _kind: &str,
        _cwd: &str,
        _name: &str,
        _prompt: &str,
    ) -> Result<Started, String> {
        Err(NOT_STARTING.into())
    }
    /// A decision model's score for each step, whether the work in goal
    /// still needs it, 0 to 1, in order (ollama.Keep). Waits.
    fn keep(&self, _model: &str, _goal: &str, _steps: &[String]) -> Result<Vec<f64>, String> {
        Err("rush can't ask a decision model from here yet".into())
    }
    /// The installed Ollama models that answer decisions, to prune with
    /// (ollama.DecisionModels). Waits.
    fn decision_models(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    /// A room member's session config, as s starts one: the session
    /// config written as Go's host.Config is (configAs, cfg.UseAgent).
    /// May look for the agent's program: off the UI thread.
    fn room_member(&self, _s: &Start) -> Result<rush_state::json::RawJson, String> {
        Err(NOT_STARTING.into())
    }
    /// Keeps room r and starts its driver, a process of its own
    /// (room.Create, room.Start). Waits.
    fn open_room(&self, _r: rush_room::room::Room) -> Result<rush_room::room::Room, String> {
        Err(NOT_STARTING.into())
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
    fn adopt(&self, _: &Agent, _: &str) -> Result<Started, String> {
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
    /// Moves to a folder, as (id, dir, retitle).
    pub cds: Mutex<Vec<(String, String, bool)>>,
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
    /// Restarts, as (id, text).
    pub restarted: Mutex<Vec<(String, String)>>,
    /// Rewinds, as (id, session id, resume, left); and whether a rewind
    /// finds its host from an older rush.
    pub rewound: Mutex<Vec<(String, String, bool, Branch)>>,
    pub old_host: Mutex<bool>,
    /// Compactions carried on, as (id, session id, prompt).
    pub compacted: Mutex<Vec<(String, String, String)>>,
    /// The paths each session's rewinds left, by its key.
    pub branches: Mutex<Vec<(String, Vec<Branch>)>>,
    /// Forks started, as (key, fork), and hand-offs, as (kind, cwd, name,
    /// prompt).
    pub forks: Mutex<Vec<(String, Fork)>>,
    pub handoffs: Mutex<Vec<(String, String, String, String)>>,
    /// What a decision model scores each step, in order.
    pub scores: Mutex<Vec<f64>>,
    /// Hand-overs, in order; each is started as fake-<n> of starts and
    /// these together, and retires what it carries on from.
    pub handed: Mutex<Vec<HandOver>>,
    /// The decision models Ollama has.
    pub deciders: Mutex<Vec<String>>,
    /// Rooms opened, in order; each is kept as room-<n> in rush's folder
    /// (a test's own).
    pub rooms: Mutex<Vec<rush_room::room::Room>>,
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
    fn adopt(&self, a: &Agent, text: &str) -> Result<Started, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let r = (a.key.clone(), text.to_string());
        let mut adopted = self.adopted.lock().unwrap();
        adopted.push(r);
        Ok(Started {
            id: format!("adopted-{}", adopted.len()),
            name: a.display_name.clone(),
            acct: a.acct.name.clone(),
        })
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
    fn hand_over(&self, h: &HandOver) -> Result<HandedOver, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let mut handed = self.handed.lock().unwrap();
        handed.push(h.clone());
        let n = handed.len() + self.starts.lock().unwrap().len();
        Ok(HandedOver {
            started: Started {
                id: format!("fake-{n}"),
                name: h.start.name.clone(),
                acct: "default".into(),
            },
            retired: h.retire,
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
    fn cd(&self, id: &str, dir: &str, retitle: bool) -> Result<(), String> {
        let c = (id.to_string(), dir.to_string(), retitle);
        self.cds.lock().unwrap().push(c);
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
    fn restart(&self, id: &str, _name: &str, text: &str) -> Result<(), String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        self.restarted
            .lock()
            .unwrap()
            .push((id.to_string(), text.to_string()));
        Ok(())
    }
    fn rewind(
        &self,
        id: &str,
        session_id: &str,
        resume: bool,
        left: &Branch,
    ) -> Result<bool, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let r = (id.to_string(), session_id.to_string(), resume, left.clone());
        self.rewound.lock().unwrap().push(r);
        Ok(*self.old_host.lock().unwrap())
    }
    fn compacted(
        &self,
        id: &str,
        session_id: &str,
        prompt: &str,
        _left: &Branch,
        _expected: &Info,
    ) -> Result<(), String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let c = (id.to_string(), session_id.to_string(), prompt.to_string());
        self.compacted.lock().unwrap().push(c);
        Ok(())
    }
    fn branches(&self, a: &Agent) -> Vec<Branch> {
        let b = self.branches.lock().unwrap();
        b.iter()
            .find(|(k, _)| *k == a.key)
            .map(|(_, b)| b.clone())
            .unwrap_or_default()
    }
    fn fork(&self, a: &Agent, f: &Fork) -> Result<Started, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let mut forks = self.forks.lock().unwrap();
        forks.push((a.key.clone(), f.clone()));
        Ok(Started {
            id: format!("fork-{}", forks.len()),
            name: f.name.clone(),
            acct: f.acct.name.clone(),
        })
    }
    fn handoff(&self, kind: &str, cwd: &str, name: &str, prompt: &str) -> Result<Started, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let mut h = self.handoffs.lock().unwrap();
        h.push((kind.into(), cwd.into(), name.into(), prompt.into()));
        Ok(Started {
            id: format!("handoff-{}", h.len()),
            name: name.to_string(),
            acct: "default".into(),
        })
    }
    fn keep(&self, _model: &str, _goal: &str, steps: &[String]) -> Result<Vec<f64>, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let s = self.scores.lock().unwrap();
        Ok(steps
            .iter()
            .enumerate()
            .map(|(i, _)| s.get(i).copied().unwrap_or(1.0))
            .collect())
    }
    fn decision_models(&self) -> Result<Vec<String>, String> {
        Ok(self.deciders.lock().unwrap().clone())
    }
    fn room_member(&self, s: &Start) -> Result<rush_state::json::RawJson, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        rush_state::json::RawJson::from_value(&serde_json::json!({
            "cwd": s.cwd, "kind": s.kind, "model": s.model,
        }))
        .map_err(|e| e.to_string())
    }
    fn open_room(&self, mut r: rush_room::room::Room) -> Result<rush_room::room::Room, String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        let mut rooms = self.rooms.lock().unwrap();
        let id = format!("room-{}", rooms.len() + 1);
        // Kept as rush keeps it (in the test's own rush folder), but no
        // driver started.
        r = rush_room::room::Rooms::new().create(r, || id)?;
        rooms.push(r.clone());
        Ok(r)
    }
}
