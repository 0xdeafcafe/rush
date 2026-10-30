//! A session's events as turns, items and steps. Port of
//! internal/convo/model.go (Status, Step, Item, Turn, Task, Request,
//! ToolStat, Session, Apply, endTurn, message, words, call, notice,
//! interrupted, results, result, request, tasks), neutral.go:14-202
//! (applyNeutral, partStart, delta, compacted, turnEnd, ensureStep),
//! usermessages.go:21-128 (sent, userMessage), hooks.go:12-53,
//! compactby.go:151-169 (compactAsk) and host/host.go:695 (TurnCost).
//!
//! What it takes in is rush_core::event::Event, whichever agent sent it:
//! a hosted session's stream, or a transcript read back (the Tail reads
//! Claude Code's through rush_claude::messages). Tool calls and results
//! arrive as rush_core::tool::Call and Output, made by the agent's adapter;
//! nothing here reads an agent's tool names.
//!
//! Steps live in one arena on the Session and are named by StepId; turns
//! name theirs by id, so a subagent's step can sit under its parent's call
//! and in its turn at once. Historic turns keep text, never drawn lines.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rush_core::agent::TokenUsage;
use rush_core::event::{self, Event, ImageData, OptionKind, PartKind};
use rush_core::time::Ms;
use rush_core::tool::{Call, Input, Kind, Output, TodoItem};
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::text::{
    between, exit_code_in, first_line, first_non_empty, first_plain, is_rejection, worktree_in,
};
use crate::words::{claude_result, claudes, result_text, step_tool};

mod chainrun;
mod exchange;
mod jobs;
pub mod runs;

pub use chainrun::{PartProc, RunningPart, Shell, ShellProc};
pub use exchange::{ExchangeItem, exchange_peer, peer_label};
pub use jobs::Job;

/// Where a step is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Running,
    Ok,
    Failed,
    /// An approval is pending.
    Waiting,
    /// Refused, by you or by a rule.
    Denied,
    /// Its turn ended or the agent died before it reported back.
    Lost,
}

/// A step's place in Session::steps.
pub type StepId = u32;

/// An image a message or a tool's result carried, as the conversation
/// keeps it: what it is and how big, never its bytes, which a long
/// session of screenshots would otherwise hold all of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picture {
    pub media_type: Box<str>,
    /// How many bytes the image is.
    pub bytes: usize,
    /// Where rush keeps a copy, when it does.
    pub path: Box<str>,
}

impl Picture {
    pub fn of(i: &ImageData) -> Picture {
        Picture {
            media_type: i.media_type.as_str().into(),
            bytes: i.data.len(),
            path: i.path.as_str().into(),
        }
    }
}

/// One tool call.
#[derive(Clone, Debug)]
pub struct Step {
    pub id: Box<str>,
    /// The tool, in Claude Code's words: "Bash", "Read", "mcp__x__y"; the
    /// Claude tool of its kind when another agent made it (words.rs).
    pub tool: Box<str>,
    /// What the call does.
    pub kind: Kind,
    /// The call as its agent's adapter made it, with its input as sent.
    call: Box<Call>,
    /// The input in Claude Code's words, when the call isn't Claude Code's
    /// own (whose words are the call's raw).
    claude: Option<Box<RawValue>>,
    pub status: Status,
    /// The tool result as text.
    pub output: Box<str>,
    /// How the call came out, as rush's own, once it's back: its text is
    /// `output`, and its raw is dropped.
    out: Option<Box<Output>>,
    /// Claude Code's structured result (patches, stdout/stderr) as JSON,
    /// with what nothing draws dropped (slim_result); another agent's
    /// result put as Claude's would be.
    pub result: Option<Box<str>>,
    /// Bash exit code, -1 when unknown.
    pub exit: i32,
    pub start: Ms,
    pub end: Ms,
    /// A subagent's own steps.
    pub children: Vec<StepId>,
    /// What the tool gave back as pictures: a Read of one, a screenshot.
    pub images: Vec<Picture>,
    /// The subagent call it was made under; None for the main agent's.
    pub parent: Option<StepId>,
    /// The turn whose steps hold it (an index into Session::turns).
    pub turn: Option<u32>,
    /// What it waits on you for, while it does.
    pub approval: Option<Box<Asking>>,
}

/// What a step waits on you for: leave to run, or answers to the questions
/// it asks (model.go:98).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asking {
    /// The request's id, which answering or cancelling it names.
    pub id: String,
    /// Why the agent asks, when it says.
    pub reason: String,
    /// The file outside the workspace that made it ask, if one did.
    pub path: String,
    /// It can be allowed for good, and what that says it does: "Always
    /// allow Bash(pnpm lint:*)".
    pub always: bool,
    pub always_as: String,
    /// Set when it asks you to choose.
    pub question: Option<Box<event::Question>>,
}

impl Step {
    /// The input JSON in Claude Code's words, "" when it had none.
    pub fn input_json(&self) -> &str {
        match &self.claude {
            Some(c) => c.get(),
            None => self.call.raw.as_ref().map_or("", RawJson::get),
        }
    }

    /// The structured result JSON, "" when there's none.
    pub fn result_json(&self) -> &str {
        self.result.as_deref().unwrap_or("")
    }

    /// The call as rush's own (path, command, pattern, edits…), as its
    /// agent made it.
    pub fn call(&self) -> &Call {
        &self.call
    }

    /// How the call came out, as rush's own: its streams, exit, the hunks it
    /// changed, the lines it read. Only its text and exit until it's back.
    pub fn out(&self) -> Output {
        if let Some(o) = &self.out {
            let mut o = Output::clone(o);
            o.call_id = self.id.to_string();
            o.text = self.output.to_string();
            return o;
        }
        let failed = self.status == Status::Failed;
        Output {
            call_id: self.id.to_string(),
            text: self.output.to_string(),
            is_error: failed,
            exit: (self.kind == Kind::Shell).then(|| {
                if failed {
                    exit_code_in(&self.output).map_or(-1, i64::from)
                } else {
                    0
                }
            }),
            ..Output::default()
        }
    }
}

/// What a compaction left: what set it off (manual, auto, "by …") and the
/// context's tokens before and after.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compacted {
    pub trigger: Box<str>,
    pub before: i64,
    pub after: i64,
}

/// You, sending mid-turn.
#[derive(Clone, Debug, Default)]
pub struct Interject {
    pub text: String,
    /// Files sent with it.
    pub images: Vec<Box<str>>,
    /// Embedded images.
    pub pictures: Vec<Picture>,
}

/// One thing in a turn, in order.
#[derive(Clone, Debug)]
pub enum Item {
    /// The agent's words. `answer` is set on the turn's final words when
    /// the turn ends.
    Text {
        text: String,
        answer: bool,
    },
    Thinking(String),
    Step(StepId),
    Interject(Box<Interject>),
    /// The conversation was compacted here; summary is what it left, once
    /// it's read.
    Compact {
        compact: Compacted,
        summary: String,
    },
    /// A message between agents (exchange.go): who to whom and what.
    Exchange(Box<ExchangeItem>),
    /// The agent's program telling you something; level is info, warning,
    /// error or ok.
    Notice {
        text: Box<str>,
        level: Box<str>,
    },
}

/// A turn runs from your message to the agent's last word.
#[derive(Clone, Debug, Default)]
pub struct Turn {
    /// 1-based.
    pub n: u32,
    pub prompt: String,
    pub items: Vec<Item>,
    pub start: Ms,
    pub end: Ms,
    pub live: bool,
    pub cost: f64,
    /// Why it ended badly, if it did.
    pub err: String,
    /// You stopped it.
    pub stopped: bool,
    /// The main agent's model for this turn.
    pub model: Arc<str>,
    pub effort: Box<str>,
    /// Paths or names of images sent with the prompt.
    pub images: Vec<Box<str>>,
    pub pictures: Vec<Picture>,
    /// Set when you didn't start it: a background task reporting back,
    /// another session's message.
    pub from: String,
    /// What started it when no message did ("/compact", "carried on from
    /// a summary").
    pub cause: String,
    /// The command of the shell task that woke it, when it runs over a line.
    pub command: String,
    /// What's streamed this turn (text, thinking, tool input), in bytes.
    pub streamed: usize,
    /// When the thinking now under way began; 0 when none is.
    pub thinking: Ms,
    /// When its agent last said anything.
    pub heard: Ms,
    /// The request to the model being tried again, from when it was said
    /// (retry_at), until the model starts answering.
    pub retry: Option<Box<event::Retry>>,
    pub retry_at: Ms,
    /// Every step of the turn, subagents' included, in the order made.
    pub steps: Vec<StepId>,
    ver: u32,
}

impl Turn {
    /// The first line of the turn's answer, without markdown.
    pub fn outcome(&self) -> String {
        self.answer().map(first_plain).unwrap_or_default()
    }

    /// The turn's final words as the agent wrote them, markdown and all.
    pub fn answer(&self) -> Option<&str> {
        self.items.iter().rev().find_map(|it| match it {
            Item::Text { text, answer: true } => Some(text.as_str()),
            _ => None,
        })
    }

    /// Counts every tool call in the turn, subagents' included.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Bumped whenever the turn changes: a renderer's cache key.
    pub fn ver(&self) -> u32 {
        self.ver
    }

    fn touch(&mut self) {
        self.ver = self.ver.wrapping_add(1);
    }
}

/// One entry in the agent's task list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub subject: String,
    /// Present-tense form, shown while in progress.
    pub active: String,
    /// pending, in_progress, completed
    pub status: String,
}

/// One call to the model, as its usage reports it.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub id: Box<str>,
    pub at: Ms,
    pub model: Arc<str>,
    /// "" for the main agent, else the subagent's type.
    pub agent: Box<str>,
    /// The tool call that started a subagent; "" for the main agent.
    pub run: Box<str>,
    pub usage: TokenUsage,
}

/// One tool's calls, totalled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolStat {
    pub calls: u32,
    pub failed: u32,
    pub time: Ms,
}

/// What a hook handed the agent as a session began: its event
/// (SessionStart, SubagentStart) and the text it added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hook {
    pub event: Box<str>,
    pub text: Box<str>,
}

/// Where a message you sent landed: its turn, and its item when it was
/// said mid-turn (usermessages.go's sentEcho).
#[derive(Clone, Debug)]
struct Echo {
    text: String,
    turn: usize,
    item: Option<usize>,
}

/// How many sends wait for the agent to say it got them, at most.
const ECHOES: usize = 64;

/// A number no other Session in this process has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Serial(u64);

impl Default for Serial {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Serial(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// What the agent list says of a session, kept beside what its own events
/// say (Go's convo Info, as followTail sets it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub cwd: String,
    pub cost: f64,
    pub model: String,
    /// What the session's host says of how it began, for the card the
    /// conversation opens on (opening.go; host.Info): its rush id, the
    /// agent it runs, its effort and permission mode, when it started,
    /// the plugin that started it, and the rush sessions that started it
    /// as their subagent (Meta spawnedBy) or beside them (Meta
    /// startedFrom), by id.
    pub id: String,
    pub kind: String,
    pub effort: String,
    pub permission_mode: String,
    pub started_at: Ms,
    pub started_by: String,
    pub spawned_by: String,
    pub started_from: String,
}

/// A subagent of the session, and its conversation: what it edits shows
/// in the session's Changes, marked as its (changes.go:41).
#[derive(Clone, Debug)]
pub struct Helper {
    pub name: String,
    pub sess: std::sync::Arc<Session>,
}

/// Everything known about one session.
#[derive(Debug, Default)]
pub struct Session {
    pub turns: Vec<Turn>,
    /// What start hooks told the agent, shown ahead of the first turn.
    pub hooks: Vec<Hook>,
    pub tasks: Vec<Task>,
    pub model: String,
    /// Where the agent says it's running.
    pub cwd: String,
    /// Its subagents' conversations, set by whoever reads them: what they
    /// edit shows in its Changes (model.go:275, Helper).
    pub helpers: Vec<Helper>,
    /// The slash commands and skills it came with, by name, once its
    /// host says (host.Commands).
    pub commands: Vec<String>,
    /// Those commands whole, each with what it does and its argument's
    /// hint, for the / picker (c.sess.Commands).
    pub command_list: Vec<event::Command>,
    /// The agent's version, its MCP servers and how many tools it has, as
    /// its init said.
    pub version: String,
    pub mcp: Vec<event::McpServer>,
    pub ntools: usize,
    /// Tokens in the context window after the last request.
    pub context: i64,
    /// The context window's size, when the agent says it.
    pub window: i64,
    /// "rejected" once a limit stopped the session.
    pub limit: String,
    pub requests: Vec<Request>,
    /// Each tool's calls, by its name.
    pub tools: HashMap<Box<str>, ToolStat>,
    /// What the agent last said about each background task (completed,
    /// stopped, …), by task id.
    pub task_status: HashMap<Box<str>, Box<str>>,
    /// The times of the first and latest activity.
    pub first: Ms,
    pub last: Ms,
    /// Read from part way through its transcript (Tail::from_end): what
    /// needs all of it says it's still counting.
    pub partial: bool,

    steps: Vec<Step>,
    by_id: HashMap<Box<str>, StepId>,
    /// The steps asked for approval and maybe still waiting.
    asked: Vec<StepId>,
    /// The text item streaming in: (turn, item).
    streaming: Option<(u32, u32)>,
    /// The process's cost total at its last result.
    spent: f64,
    req_idx: HashMap<Box<str>, u32>,
    step_ver: u64,
    req_ver: u64,
    applied: u64,
    worktree: String,
    /// When the compaction under way began; how long the last took per
    /// token it compacted, in nanoseconds.
    compacting: Ms,
    compact_rate_ns: i64,
    /// What you sent that the agent hasn't said it got yet, oldest first.
    echoes: Vec<Echo>,
    /// Tells this session from any other, a renderer's cache key.
    serial: Serial,
    /// What rush's list says of it, beside what its own events say (Go's
    /// Session.Info): where its agent works, what it has cost, its model.
    pub info: Info,
    /// The user messages already taken, by id.
    user_ids: HashSet<Box<str>>,
    /// Every task heard of, in the order they started (jobs.go).
    jobs: Vec<Job>,
    /// The task that last ended or fired, and when (or when the turn it
    /// came during ended): a turn opened soon after is put down to it.
    woke: Option<usize>,
    woke_at: Ms,
    /// Where each exchange is, by direction:id, as (turn, item).
    exchanges: HashMap<Box<str>, (usize, usize)>,
    /// What's known of each running chain's commands, by its step
    /// (chainrun.rs).
    chains: HashMap<StepId, chainrun::Chain>,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    // ---- reading it ----

    /// Every step, in the order made; a StepId indexes it.
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn step(&self, id: StepId) -> &Step {
        &self.steps[id as usize]
    }

    /// The step of tool call id, if it's one this session made.
    pub fn step_by_call(&self, call_id: &str) -> Option<StepId> {
        self.by_id.get(call_id).copied()
    }

    /// Whether the session made the call with this id.
    pub fn made_call(&self, call_id: &str) -> bool {
        self.by_id.contains_key(call_id)
    }

    /// The turn in progress, if any.
    pub fn live(&self) -> Option<&Turn> {
        self.live_idx().map(|i| &self.turns[i])
    }

    fn live_idx(&self) -> Option<usize> {
        let n = self.turns.len();
        (n > 0 && self.turns[n - 1].live).then(|| n - 1)
    }

    /// A number no other Session in this process has: a renderer tells a
    /// session started over from the one it drew by it.
    pub fn serial(&self) -> u64 {
        self.serial.0
    }

    /// Counts events applied and transcript lines taken: a caller's cache
    /// key for what it worked out from them.
    pub fn applied(&self) -> u64 {
        self.applied
    }

    /// Bumped whenever a step is added or changes.
    pub fn step_ver(&self) -> u64 {
        self.step_ver
    }

    /// Bumped by every request recorded.
    pub fn req_ver(&self) -> u64 {
        self.req_ver
    }

    /// The worktree the session's calls last reached into, by name.
    pub fn worktree(&self) -> &str {
        &self.worktree
    }

    /// The text item streaming in, as (turn, item) indexes: a renderer's
    /// "writing" line.
    pub fn streaming(&self) -> Option<(u32, u32)> {
        self.streaming
    }

    /// Whether a compaction is under way, and since when.
    pub fn compacting(&self) -> Option<Ms> {
        (self.compacting != 0).then_some(self.compacting)
    }

    /// How long the last compaction took per token it compacted, in
    /// nanoseconds; 0 when none was timed.
    pub fn compact_rate_ns(&self) -> i64 {
        self.compact_rate_ns
    }

    /// Says rush began compacting it at at, or with 0 that it's done.
    pub fn mark_compacting(&mut self, at: Ms) {
        self.compacting = at;
    }

    /// The effort it runs at: the one rush set or its init said, else the
    /// one the last turn ran at, else the latest an earlier one did
    /// (model.go:378, Effort).
    pub fn effort(&self) -> &str {
        if !self.info.effort.is_empty() {
            return &self.info.effort;
        }
        self.turns
            .iter()
            .rev()
            .map(|t| &*t.effort)
            .find(|e| !e.is_empty())
            .unwrap_or("")
    }

    /// The task in progress and how far through the list the agent is.
    pub fn current(&self) -> (Option<&Task>, usize, usize) {
        let mut now = None;
        let mut done = 0;
        for t in &self.tasks {
            match t.status.as_str() {
                "completed" => done += 1,
                "in_progress" if now.is_none() => now = Some(t),
                _ => {}
            }
        }
        (now, done, self.tasks.len())
    }

    // ---- taking events in ----

    /// Folds one event from any agent into the session at time now (0 when
    /// unknown): Go's Apply of an event.Event, and applyNeutral.
    pub fn apply(&mut self, ev: &Event, now: Ms) {
        self.applied += 1;
        self.saw(now);
        if let Some(i) = self.live_idx()
            && now > self.turns[i].heard
        {
            self.turns[i].heard = now;
        }
        match ev {
            Event::Exchange(e) => {
                self.exchange(e, now);
            }
            Event::Init(e) => {
                self.model = e.model.clone();
                self.cwd = e.cwd.clone();
                self.version = e.version.clone();
                self.mcp = e.mcp.clone();
                self.ntools = e.tools.len();
                if !e.effort.is_empty() {
                    self.info.effort = e.effort.clone();
                }
            }
            Event::Limited(_) => self.limit = "rejected".into(),
            Event::Context(e) => {
                self.context = e.tokens;
                if e.window > 0 {
                    self.window = e.window;
                }
            }
            Event::StartNotice(e) => self.add_hooks(vec![Hook {
                event: first_non_empty(&[&e.event, "SessionStart"]).into(),
                text: e.text.as_str().into(),
            }]),
            Event::PartStart(e) => self.part_start(e.kind, now),
            Event::Delta(e) => self.delta(e.kind, &e.text, now),
            Event::Compacted(e) => self.compacted(
                Compacted {
                    trigger: e.trigger.as_str().into(),
                    before: e.before,
                    after: e.after,
                },
                now,
            ),
            Event::Message(m) => {
                if self.user_message(m, now) {
                    return;
                }
                if m.role == "assistant"
                    && let Some(i) = self.live_idx()
                {
                    self.turns[i].retry = None; // the model answered
                }
                self.message(m, now);
            }
            Event::CallUpdated(e) => {
                if let Some(&sid) = self.by_id.get(e.call.id.as_str()) {
                    let (tool, claude) = step_words(&e.call);
                    let st = &mut self.steps[sid as usize];
                    st.tool = tool;
                    st.claude = claude;
                    st.kind = e.call.kind;
                    *st.call = e.call.clone();
                    self.touch_step(sid);
                }
            }
            Event::Approval(e) => {
                self.ensure_step(&e.call, now);
                let always = e.options.iter().find(|o| o.kind == OptionKind::AllowAlways);
                self.ask(
                    &e.call.id,
                    Asking {
                        id: e.id.clone(),
                        reason: e.reason.clone(),
                        path: e.path.clone(),
                        always: always.is_some(),
                        always_as: always.map(|o| o.label.clone()).unwrap_or_default(),
                        question: None,
                    },
                );
            }
            Event::Question(e) => {
                let id = first_non_empty(&[&e.call_id, &e.id]).to_owned();
                self.ensure_step(
                    &Call {
                        id: id.clone(),
                        name: "AskUserQuestion".into(),
                        kind: Kind::Question,
                        ..Call::default()
                    },
                    now,
                );
                self.ask(
                    &id,
                    Asking {
                        id: e.id.clone(),
                        question: Some(Box::new(e.clone())),
                        ..Asking::default()
                    },
                );
            }
            Event::ApprovalCancelled(e) => self.settle(&e.id),
            Event::Denied(e) => {
                if let Some(&sid) = self.by_id.get(e.call_id.as_str()) {
                    let st = &mut self.steps[sid as usize];
                    st.status = Status::Denied;
                    st.output = e.reason.as_str().into();
                    st.end = now;
                    self.touch_step(sid);
                }
            }
            Event::TaskStarted(_)
            | Event::TaskUpdated(_)
            | Event::TaskProgress(_)
            | Event::TaskDone(_)
            | Event::Background(_) => self.apply_job(ev, now),
            Event::Plan(e) => self.tasks_from_todos(&e.todos),
            Event::Status(e) => {
                if e.text == "compacting" {
                    if self.compacting == 0 {
                        self.compacting = now;
                        if let Some(i) = self.live_idx() {
                            self.turns[i].touch();
                        }
                    }
                } else {
                    self.compacting = 0;
                }
            }
            Event::Retry(e) => {
                if let Some(i) = self.live_idx() {
                    let t = &mut self.turns[i];
                    t.retry = Some(Box::new(e.clone()));
                    t.retry_at = now;
                    t.touch();
                }
            }
            Event::TurnEnd(e) => {
                if let Some(i) = self.live_idx() {
                    self.turns[i].retry = None;
                }
                self.turn_end(e, now);
            }
            // The commands it came with, for the card it opens on
            // (model.go:494).
            Event::Commands(c) => {
                self.commands = c.list.iter().map(|c| c.name.clone()).collect();
                self.command_list = c.list.clone();
            }
            // Only the host reads these, as Go's applyNeutral leaves them.
            Event::MessageStart(_) | Event::Quota(_) | Event::Billing(_) | Event::Other(_) => {}
        }
    }

    /// You sent text, as the host tells it: a new turn, or said mid-turn
    /// (Go's Apply of a host.Sent). The agent's own copy of it, when it
    /// sends one, is matched to it rather than said twice.
    pub fn sent(&mut self, text: String, images: Vec<Box<str>>, now: Ms) {
        self.applied += 1;
        self.saw(now);
        self.send(text, images, now, true);
    }

    /// The approval with this request id was answered or withdrawn (Go's
    /// Apply of a host.Answered): its step runs on (model.go:553).
    pub fn settle(&mut self, request: &str) {
        for i in 0..self.asked.len() {
            let sid = self.asked[i];
            let st = &mut self.steps[sid as usize];
            if st.approval.as_ref().is_some_and(|a| a.id == request) {
                st.approval = None;
                if st.status == Status::Waiting {
                    st.status = Status::Running;
                }
                self.touch_step(sid);
            }
        }
        let steps = &self.steps;
        self.asked
            .retain(|&sid| steps[sid as usize].approval.is_some());
    }

    /// The approvals waiting, oldest first (model.go:413).
    pub fn pending(&self) -> Vec<StepId> {
        self.asked
            .iter()
            .copied()
            .filter(|&sid| self.steps[sid as usize].approval.is_some())
            .collect()
    }

    /// Makes a step of a call an approval or question is about, when the
    /// agent asks before it says it's making the call (neutral.go:195).
    fn ensure_step(&mut self, c: &Call, now: Ms) {
        if c.id.is_empty() || self.by_id.contains_key(c.id.as_str()) {
            return;
        }
        self.message(
            &event::Message {
                role: "assistant".into(),
                parts: vec![event::Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(c.clone())),
                    ..event::Part::default()
                }],
                ..event::Message::default()
            },
            now,
        );
    }

    /// Marks the step of call id as waiting on you (model.go:541).
    fn ask(&mut self, call: &str, a: Asking) {
        let Some(&sid) = self.by_id.get(call) else {
            return;
        };
        if !self.asked.contains(&sid) {
            self.asked.push(sid);
        }
        let st = &mut self.steps[sid as usize];
        st.approval = Some(Box::new(a));
        st.status = Status::Waiting;
        self.touch_step(sid);
    }

    fn saw(&mut self, now: Ms) {
        if now != 0 {
            if self.first == 0 || now < self.first {
                self.first = now;
            }
            if now > self.last {
                self.last = now;
            }
        }
    }

    pub(crate) fn count_applied(&mut self) {
        self.applied += 1;
    }

    /// You sent text: a new turn, or said mid-turn. Says where it went: its
    /// turn, and its item when it was said mid-turn. echo keeps it to match
    /// the agent's own copy of it with (usermessages.go:21).
    pub(crate) fn send(
        &mut self,
        text: String,
        images: Vec<Box<str>>,
        now: Ms,
        echo: bool,
    ) -> (usize, Option<usize>) {
        self.saw(now);
        let echoed = echo.then(|| text.clone());
        let at = if let Some(i) = self.live_idx() {
            let t = &mut self.turns[i];
            t.items.push(Item::Interject(Box::new(Interject {
                text,
                images,
                pictures: Vec::new(),
            })));
            t.touch();
            (i, Some(t.items.len() - 1))
        } else {
            self.streaming = None;
            self.woke = None;
            let mut t = Turn {
                n: self.turns.len() as u32 + 1,
                prompt: text,
                start: now,
                live: true,
                images,
                ..Turn::default()
            };
            compact_ask(&mut t);
            self.turns.push(t);
            (self.turns.len() - 1, None)
        };
        if let Some(text) = echoed {
            // Some agents never echo: what waits for one is bounded.
            if self.echoes.len() == ECHOES {
                self.echoes.remove(0);
            }
            self.echoes.push(Echo {
                text,
                turn: at.0,
                item: at.1,
            });
        }
        at
    }

    /// A message of yours as the agent tells it, which a session it hosts
    /// sends back: matched to what the host said you sent, else a turn of
    /// its own. Says whether it was one (usermessages.go:67).
    fn user_message(&mut self, m: &event::Message, now: Ms) -> bool {
        if m.role != "user" || !m.parent.is_empty() || m.injected || m.parts.is_empty() {
            return false;
        }
        let mut texts = Vec::new();
        let mut images: Vec<Box<str>> = Vec::new();
        let mut pictures = Vec::new();
        for p in &m.parts {
            match p.kind {
                PartKind::Text => texts.push(p.text.as_str()),
                PartKind::Image => {
                    if let Some(img) = &p.image {
                        pictures.push(Picture::of(img));
                        images.push(first_non_empty(&[&img.path, "image"]).into());
                    }
                }
                _ => return false, // tool results belong to their calls
            }
        }
        let text = crate::transcript::clean_prompt(&crate::transcript::join_texts(&texts));
        if text.is_empty() && pictures.is_empty() {
            return true;
        }
        if !m.id.is_empty() && !self.user_ids.insert(m.id.as_str().into()) {
            return true;
        }
        let found = self
            .echoes
            .iter()
            .position(|e| crate::transcript::clean_prompt(&e.text) == text);
        let (turn, item) = match found {
            Some(i) => {
                let e = self.echoes.remove(i);
                (e.turn, e.item)
            }
            None => self.send(text, images.clone(), now, false),
        };
        if !pictures.is_empty() {
            let t = &mut self.turns[turn];
            match item.and_then(|i| t.items.get_mut(i)) {
                Some(Item::Interject(x)) => {
                    x.pictures = pictures;
                    if x.images.is_empty() {
                        x.images = images;
                    }
                }
                Some(Item::Exchange(x)) => {
                    x.pictures = pictures;
                    if x.exchange.images.is_empty() {
                        x.exchange.images = images.iter().map(|i| i.to_string()).collect();
                    }
                }
                _ => {
                    t.pictures = pictures;
                    if t.images.is_empty() {
                        t.images = images;
                    }
                }
            }
            t.touch();
        }
        true
    }

    /// The live turn, or one opened for output with no prompt to hold it:
    /// an agent waking for background work, a replay that starts mid-turn.
    fn turn_for(&mut self, now: Ms) -> usize {
        if let Some(i) = self.live_idx() {
            return i;
        }
        self.turns.push(Turn {
            n: self.turns.len() as u32 + 1,
            live: true,
            start: now,
            ..Turn::default()
        });
        let ti = self.turns.len() - 1;
        if let Some(j) = self.take_wake(now) {
            self.wake(ti, j);
        }
        ti
    }

    pub(crate) fn end_turn(&mut self, ti: usize, now: Ms) {
        self.step_ver += 1;
        self.streaming = None;
        let t = &mut self.turns[ti];
        t.live = false;
        t.end = now;
        for &sid in &t.steps {
            let st = &mut self.steps[sid as usize];
            // A background subagent's ask outlives the turn it landed in.
            if st.approval.is_some() && st.parent.is_some() {
                continue;
            }
            if matches!(st.status, Status::Running | Status::Waiting) {
                st.status = Status::Lost;
                st.approval = None;
            }
        }
        // The last words become the answer.
        for it in t.items.iter_mut().rev() {
            match it {
                Item::Text { text, answer } if !text.trim().is_empty() => {
                    *answer = true;
                    break;
                }
                Item::Step(_) => break,
                _ => {}
            }
        }
        t.touch();
    }

    fn touch_step(&mut self, sid: StepId) {
        self.step_ver += 1;
        if let Some(t) = self.steps[sid as usize].turn {
            self.turns[t as usize].touch();
        }
    }

    fn message(&mut self, m: &event::Message, now: Ms) {
        if m.role != "assistant" {
            // Tool results belong to the turn their call is in, however
            // late they arrive; they never open a turn of their own.
            self.results(m, now);
            return;
        }
        let parent = self.by_id.get(m.parent.as_str()).copied();
        // A subagent's message whose run we never saw start still isn't the
        // main agent's: it mustn't land in the turn or its numbers.
        let sub = !m.parent.is_empty();
        // Only the main agent opens a turn. A background subagent working
        // on after its turn ended stays with that turn.
        let mut t = self.live_idx();
        if !sub {
            t = Some(self.turn_for(now));
        } else if t.is_none()
            && let Some(p) = parent
        {
            t = self.steps[p as usize].turn.map(|x| x as usize);
        }
        if let Some(u) = &m.tokens {
            self.request(m, parent, now);
            if !sub && let Some(ti) = t {
                self.context =
                    u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h + u.output;
                if !m.model.is_empty() {
                    self.turns[ti].model = self.intern_model(&m.model);
                }
            }
        }
        for p in &m.parts {
            match p.kind {
                PartKind::Text => {
                    if !sub
                        && !p.text.trim().is_empty()
                        && let Some(ti) = t
                    {
                        self.words(ti, &p.text); // a subagent's words stay inside it
                    }
                }
                PartKind::Thinking => {
                    if !sub && let Some(ti) = t {
                        let items = &mut self.turns[ti].items;
                        if !matches!(items.last(), Some(Item::Thinking(_))) {
                            items.push(Item::Thinking(p.text.clone()));
                        }
                    }
                }
                PartKind::ToolCall => {
                    if let Some(c) = &p.call {
                        self.call(c, parent, t, sub, now);
                    }
                }
                PartKind::ToolResult | PartKind::Image => {}
            }
        }
        if let Some(ti) = t {
            self.turns[ti].touch();
        }
    }

    /// The main agent's words in turn ti: what streamed in, made whole.
    fn words(&mut self, ti: usize, text: &str) {
        if let Some((st, si)) = self.streaming.take()
            && let Some(Item::Text { text: t, .. }) = self
                .turns
                .get_mut(st as usize)
                .and_then(|t| t.items.get_mut(si as usize))
        {
            text.clone_into(t);
            return;
        }
        self.turns[ti].items.push(Item::Text {
            text: text.to_owned(),
            answer: false,
        });
    }

    /// Makes a step of a tool call, in turn t under parent.
    fn call(&mut self, c: &Call, parent: Option<StepId>, t: Option<usize>, sub: bool, now: Ms) {
        if !sub {
            // A background subagent's call lands mid-stream too: it doesn't
            // end the main agent's words, or they'd be drawn twice.
            self.streaming = None;
        }
        let (tool, claude) = step_words(c);
        self.tools.entry(tool.clone()).or_default().calls += 1;
        let call = Box::new(c.clone());
        let id: Box<str> = c.id.as_str().into();
        let sid = self.steps.len() as StepId;
        self.steps.push(Step {
            id: id.clone(),
            tool,
            kind: c.kind,
            call,
            claude,
            status: Status::Running,
            output: Box::default(),
            out: None,
            result: None,
            exit: -1,
            start: now,
            end: 0,
            children: Vec::new(),
            images: Vec::new(),
            parent,
            turn: t.map(|x| x as u32),
            approval: None,
        });
        if !sub {
            let wt = worktree_in(self.steps[sid as usize].input_json());
            if !wt.is_empty() && wt != self.worktree {
                self.worktree = wt.to_owned();
            }
        }
        self.by_id.insert(id, sid);
        if let Some(ti) = t {
            self.turns[ti].steps.push(sid);
        }
        self.step_ver += 1;
        match (parent, t) {
            (Some(p), _) => self.steps[p as usize].children.push(sid),
            (None, Some(ti)) if !sub => self.turns[ti].items.push(Item::Step(sid)),
            _ => {}
        }
        self.tasks_from_input(sid);
    }

    /// Adds something the agent's program said to you (not the model) to
    /// the running turn, or the last one.
    pub(crate) fn notice(&mut self, text: String, level: &str, now: Ms) {
        let ti = match self.live_idx() {
            Some(i) => i,
            None if !self.turns.is_empty() => self.turns.len() - 1,
            None => self.turn_for(now),
        };
        let t = &mut self.turns[ti];
        t.items.push(Item::Notice {
            text: text.into_boxed_str(),
            level: level.into(),
        });
        t.touch();
    }

    /// Ends the running turn (or marks the last one) as stopped by you.
    pub(crate) fn interrupted(&mut self, now: Ms) {
        let ti = match self.live_idx() {
            Some(i) => {
                self.end_turn(i, now);
                Some(i)
            }
            None => self.turns.len().checked_sub(1),
        };
        if let Some(i) = ti {
            let t = &mut self.turns[i];
            t.stopped = true;
            t.err.clear();
            t.touch();
        }
    }

    fn results(&mut self, m: &event::Message, now: Ms) {
        for p in &m.parts {
            if p.kind == PartKind::Text && p.text.trim().starts_with("[Request interrupted by user")
            {
                self.interrupted(now);
                return;
            }
        }
        // Text the agent injects as a user message (a background task
        // finishing, another session's message) starts a turn of its own.
        for p in &m.parts {
            if p.kind == PartKind::Text
                && p.text.trim().starts_with('<')
                && let Some((from, text)) = crate::transcript::injected(&p.text)
                && self.live_idx().is_none()
            {
                self.note_task(&p.text);
                self.sent(text, Vec::new(), now);
                if let Some(t) = self.turns.last_mut() {
                    t.from = from;
                }
            }
        }
        let mut last = None;
        for p in &m.parts {
            match p.kind {
                PartKind::ToolResult => {
                    if let Some(o) = &p.output {
                        self.result(o, now);
                        last = self.by_id.get(o.call_id.as_str()).copied();
                    }
                }
                PartKind::Image => {
                    // An image a tool gave back, drawn under its step.
                    if let (Some(img), Some(sid)) = (&p.image, last) {
                        self.steps[sid as usize].images.push(Picture::of(img));
                        self.touch_step(sid);
                    }
                }
                _ => {}
            }
        }
    }

    /// Takes in how a call came out.
    fn result(&mut self, o: &Output, now: Ms) {
        let Some(&sid) = self.by_id.get(o.call_id.as_str()) else {
            return;
        };
        let text = result_text(o);
        let st = &mut self.steps[sid as usize];
        let kind = st.kind;
        // Claude Code's structured result is kept as it sent it; another
        // agent's is put as Claude's would be.
        st.result = if claudes(&st.call) {
            slim_result(o.raw.as_ref().map_or("", RawJson::get))
        } else {
            slim_result(&claude_result(o))
        };
        // Its text is the step's output, and what's drawn is read from the
        // rest: neither its raw nor its id is kept twice.
        st.out = Some(Box::new(Output {
            call_id: String::new(),
            text: String::new(),
            is_error: o.is_error,
            stdout: o.stdout.clone(),
            stderr: o.stderr.clone(),
            exit: o.exit,
            patches: o.patches.clone(),
            lines: o.lines,
            created: o.created,
            raw: None,
        }));
        let rejected = o.is_error && is_rejection(&text);
        st.output = text.into_boxed_str();
        st.end = now;
        st.approval = None;
        st.status = match st.status {
            Status::Denied => Status::Denied,
            _ if rejected => Status::Denied,
            _ if o.is_error => Status::Failed,
            _ => Status::Ok,
        };
        if kind == Kind::Shell {
            st.exit = match st.status {
                Status::Ok => 0, // "Exit code N" in a success's output is just output
                _ => exit_code_in(&st.output).unwrap_or(-1),
            };
        }
        let (failed, took) = (
            st.status == Status::Failed,
            (st.start != 0).then(|| st.end - st.start),
        );
        let tool = st.tool.clone();
        self.touch_step(sid);
        let ts = self.tools.entry(tool).or_default();
        if failed {
            ts.failed += 1;
        }
        if let Some(d) = took {
            ts.time += d;
        }
        self.tasks_from_result(sid);
    }

    /// One model call's message: its id, model, usage. Claude Code sends
    /// one message per content block, all with the same id and usage, so a
    /// repeat replaces the last.
    fn request(&mut self, m: &event::Message, parent: Option<StepId>, now: Ms) {
        if m.model == "<synthetic>" {
            return; // Claude Code's own placeholder, not a model call
        }
        let agent: Box<str> =
            parent.map_or_else(Box::default, |p| agent_name(&self.steps[p as usize]).into());
        let mut r = Request {
            id: m.id.as_str().into(),
            at: now,
            model: self.intern_model(&m.model),
            agent,
            run: m.parent.as_str().into(),
            usage: m.tokens.unwrap_or_default(),
        };
        self.req_ver += 1;
        // The output count can grow between a call's messages: keep the
        // largest.
        if !m.id.is_empty()
            && let Some(&i) = self.req_idx.get(m.id.as_str())
        {
            let prev = &self.requests[i as usize];
            r.at = prev.at;
            r.usage.output = r.usage.output.max(prev.usage.output);
            self.requests[i as usize] = r;
            return;
        }
        if !m.id.is_empty() {
            self.req_idx
                .insert(r.id.clone(), self.requests.len() as u32);
        }
        self.requests.push(r);
    }

    /// One shared copy of a model's name: thousands of requests name a
    /// handful of models.
    fn intern_model(&self, model: &str) -> Arc<str> {
        if let Some(r) = self.requests.last()
            && &*r.model == model
        {
            return r.model.clone();
        }
        if let Some(t) = self.turns.iter().rev().take(4).find(|t| &*t.model == model) {
            return t.model.clone();
        }
        Arc::from(model)
    }

    // Tasks come from TodoWrite (the whole list each time) and from
    // TaskCreate and TaskUpdate (one at a time; TaskCreate's id is only in
    // its result).
    fn tasks_from_input(&mut self, sid: StepId) {
        let st = &self.steps[sid as usize];
        match &*st.tool {
            "TodoWrite" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Todo {
                    content: String,
                    #[serde(rename = "activeForm")]
                    active_form: String,
                    status: String,
                }
                #[derive(Deserialize)]
                struct In {
                    #[serde(default)]
                    todos: Vec<Todo>,
                }
                let Ok(inp) = serde_json::from_str::<In>(st.input_json()) else {
                    return;
                };
                self.tasks = inp
                    .todos
                    .into_iter()
                    .enumerate()
                    .map(|(i, td)| Task {
                        id: (i + 1).to_string(),
                        subject: td.content,
                        active: td.active_form,
                        status: td.status,
                    })
                    .collect();
            }
            "TaskUpdate" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct In {
                    #[serde(rename = "taskId")]
                    id: String,
                    status: String,
                    subject: String,
                    #[serde(rename = "activeForm")]
                    active_form: String,
                }
                let Ok(inp) = serde_json::from_str::<In>(st.input_json()) else {
                    return;
                };
                let Some(i) = self.tasks.iter().position(|t| t.id == inp.id) else {
                    return;
                };
                if inp.status == "deleted" {
                    self.tasks.remove(i);
                    return;
                }
                let t = &mut self.tasks[i];
                if !inp.status.is_empty() {
                    t.status = inp.status;
                }
                if !inp.subject.is_empty() {
                    t.subject = inp.subject;
                }
                if !inp.active_form.is_empty() {
                    t.active = inp.active_form;
                }
            }
            _ => {}
        }
    }

    /// The agent's whole plan, as a TodoWrite of it sets the tasks
    /// (neutral.go's Plan).
    fn tasks_from_todos(&mut self, todos: &[TodoItem]) {
        self.tasks = todos
            .iter()
            .enumerate()
            .map(|(i, td)| Task {
                id: (i + 1).to_string(),
                subject: td.label.clone(),
                active: td.active.clone(),
                status: td.status.clone(),
            })
            .collect();
    }

    fn tasks_from_result(&mut self, sid: StepId) {
        let st = &self.steps[sid as usize];
        if &*st.tool != "TaskCreate" || st.status != Status::Ok {
            return;
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct In {
            subject: String,
            #[serde(rename = "activeForm")]
            active_form: String,
        }
        let inp: In = serde_json::from_str(st.input_json()).unwrap_or_default();
        let id = created_task_id(&st.output)
            .map(str::to_owned)
            .unwrap_or_else(|| (self.tasks.len() + 1).to_string());
        self.tasks.push(Task {
            id,
            subject: inp.subject,
            active: inp.active_form,
            status: "pending".into(),
        });
    }

    /// Records a background task's reported status, from the notification
    /// Claude Code injects when one finishes.
    pub(crate) fn note_task(&mut self, text: &str) {
        let id = between(text, "<task-id>", "</task-id>");
        if id.is_empty() {
            return;
        }
        let st = task_status(between(text, "<status>", "</status>"));
        self.task_status
            .insert(id.into(), first_non_empty(&[st, "completed"]).into());
    }

    /// Keeps hooks the session hasn't got yet, and has the first turn,
    /// which draws them, drawn again.
    pub(crate) fn add_hooks(&mut self, hs: Vec<Hook>) {
        for h in hs {
            if !self.hooks.contains(&h) {
                self.hooks.push(h);
                if let Some(t) = self.turns.first_mut() {
                    t.touch();
                }
            }
        }
    }

    /// Opens a part of the message being written: thinking shows as it
    /// starts, since it often streams nothing readable.
    fn part_start(&mut self, p: PartKind, now: Ms) {
        let ti = self.turn_for(now);
        let t = &mut self.turns[ti];
        t.thinking = 0;
        t.retry = None; // the model is answering
        if p == PartKind::Thinking {
            t.thinking = now;
            if !matches!(t.items.last(), Some(Item::Thinking(_))) {
                t.items.push(Item::Thinking(String::new()));
            }
        }
        if p != PartKind::Text {
            self.streaming = None;
        }
        t.touch();
    }

    /// A piece of the message being written.
    fn delta(&mut self, p: PartKind, text: &str, now: Ms) {
        let ti = self.turn_for(now);
        let t = &mut self.turns[ti];
        t.streamed += text.len();
        match p {
            PartKind::ToolCall => {}
            PartKind::Thinking => {
                if !matches!(t.items.last(), Some(Item::Thinking(_))) {
                    t.items.push(Item::Thinking(String::new()));
                }
                if let Some(Item::Thinking(s)) = t.items.last_mut() {
                    s.push_str(text);
                }
            }
            _ => {
                let at = match self.streaming {
                    Some((st, si)) if st as usize == ti => si as usize,
                    _ => {
                        t.items.push(Item::Text {
                            text: String::new(),
                            answer: false,
                        });
                        let si = t.items.len() - 1;
                        self.streaming = Some((ti as u32, si as u32));
                        si
                    }
                };
                if let Some(Item::Text { text: s, .. }) = t.items.get_mut(at) {
                    s.push_str(text);
                }
            }
        }
        t.touch();
    }

    /// A divider in the turn it happened in (or the last one), and the
    /// context starts again from what the summary left.
    fn compacted(&mut self, c: Compacted, now: Ms) {
        let ti = match self.live_idx() {
            Some(i) => i,
            None if !self.turns.is_empty() => self.turns.len() - 1,
            None => self.turn_for(now),
        };
        if self.compacting != 0 && c.before > 0 {
            self.compact_rate_ns = (now - self.compacting) * 1_000_000 / c.before;
        }
        self.compacting = 0;
        if c.after > 0 {
            self.context = c.after;
        }
        let t = &mut self.turns[ti];
        t.items.push(Item::Compact {
            compact: c,
            summary: String::new(),
        });
        t.touch();
    }

    /// Ends the running turn: stopped by you, done, or gone wrong.
    fn turn_end(&mut self, e: &event::TurnEnd, now: Ms) {
        if e.reason == "interrupted" {
            self.interrupted(now);
            return;
        }
        self.end_jobs(now);
        if self.woke.is_some() {
            self.woke_at = now; // held until now: this is when it wakes the agent
        }
        let Some(ti) = self.live_idx() else { return };
        self.end_turn(ti, now);
        let cost = turn_cost(&mut self.spent, e.cost);
        let t = &mut self.turns[ti];
        t.cost = cost;
        if !e.reason.is_empty() && e.reason != "done" && e.reason != "error" {
            t.err = e.reason.replace('_', " ");
        } else if !e.err.is_empty() || e.reason == "error" {
            t.err = first_non_empty(&[first_line(&e.err), "error"]).to_owned();
        }
        // The agent's own copy of the error (a limit's, say) is the ✗ row's.
        if !t.err.is_empty()
            && let Some(Item::Text { text, .. }) = t.items.last()
            && first_line(text.trim()) == t.err
        {
            t.items.pop();
        }
    }

    // ---- for the transcript reader ----

    pub(crate) fn live_turn_mut(&mut self) -> Option<&mut Turn> {
        let i = self.live_idx()?;
        Some(&mut self.turns[i])
    }

    /// Makes a step of a command you ran with !, in the live turn.
    pub(crate) fn shell_start(&mut self, cmd: &str, at: Ms) {
        let Some(ti) = self.live_idx() else { return };
        let id: Box<str> = format!("you-{}", self.turns[ti].n).into();
        let raw = serde_json::to_string(&BTreeMap::from([
            ("command", cmd),
            ("description", "you ran"),
        ]))
        .ok()
        .and_then(|s| RawValue::from_string(s).ok())
        .map(RawJson);
        let call = Box::new(Call {
            id: id.to_string(),
            name: "Bash".into(),
            kind: Kind::Shell,
            input: Input {
                command: cmd.to_owned(),
                description: "you ran".into(),
                ..Input::default()
            },
            raw,
            ..Call::default()
        });
        let sid = self.steps.len() as StepId;
        self.steps.push(Step {
            id: id.clone(),
            tool: "Bash".into(),
            kind: Kind::Shell,
            call,
            claude: None,
            status: Status::Running,
            output: Box::default(),
            out: None,
            result: None,
            exit: -1,
            start: at,
            end: 0,
            children: Vec::new(),
            images: Vec::new(),
            parent: None,
            turn: Some(ti as u32),
            approval: None,
        });
        self.by_id.insert(id, sid);
        self.step_ver += 1;
        let t = &mut self.turns[ti];
        t.steps.push(sid);
        t.items.push(Item::Step(sid));
        t.touch();
    }

    /// What a command you ran with ! printed, on its step; the turn ends.
    pub(crate) fn shell_result(&mut self, ti: usize, stdout: &str, stderr: &str, at: Ms) {
        let id = format!("you-{}", self.turns[ti].n);
        let Some(&sid) = self.by_id.get(id.as_str()) else {
            return;
        };
        let stdout = if stdout == "(Bash completed with no output)" {
            ""
        } else {
            stdout
        };
        let st = &mut self.steps[sid as usize];
        st.output = format!("{stdout}\n{stderr}").trim().into();
        st.end = at;
        st.status = if stderr.trim().is_empty() {
            Status::Ok
        } else {
            Status::Failed
        };
        st.result =
            serde_json::to_string(&BTreeMap::from([("stdout", stdout), ("stderr", stderr)]))
                .ok()
                .map(String::into_boxed_str);
        let failed = st.status == Status::Failed;
        st.out = Some(Box::new(Output {
            is_error: failed,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            exit: Some(if failed {
                exit_code_in(&st.output).map_or(-1, i64::from)
            } else {
                0
            }),
            ..Output::default()
        }));
        self.apply(
            &Event::TurnEnd(event::TurnEnd {
                reason: "done".into(),
                ..event::TurnEnd::default()
            }),
            at,
        );
    }

    /// Puts the summary a compaction leaves on its divider. Says whether
    /// there was a divider to put it on.
    pub(crate) fn compact_summary(&mut self, text: &str) -> bool {
        let n = self.turns.len();
        for i in (n.saturating_sub(2)..n).rev() {
            let t = &mut self.turns[i];
            for it in t.items.iter_mut().rev() {
                if let Item::Compact { summary, .. } = it
                    && summary.is_empty()
                {
                    *summary = text.to_owned();
                    t.touch();
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn set_cwd(&mut self, cwd: &str) {
        if self.cwd.is_empty() {
            self.cwd = cwd.to_owned();
        }
    }
}

/// A call as a step keeps it in Claude Code's words: its tool, and its
/// input when that isn't the call's own raw.
fn step_words(c: &Call) -> (Box<str>, Option<Box<RawValue>>) {
    if claudes(c) {
        return (c.name.as_str().into(), None);
    }
    let (name, input) = step_tool(c);
    (name.into(), RawValue::from_string(input).ok())
}

/// What this turn cost, from the process's running total: a total lower
/// than the last means the process started again.
pub fn turn_cost(spent: &mut f64, total: f64) -> f64 {
    let mut d = total - *spent;
    if d < 0.0 {
        d = total;
    }
    *spent = total;
    d
}

/// Who a Task or Agent call ran: the agent Claude Code reports in its
/// result (it resolves an omitted or aliased subagent_type), else the one
/// asked for, else just "subagent". After render.go:2454 (agentName).
pub fn agent_name(st: &Step) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(rename = "agentType")]
        agent_type: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct In {
        subagent_type: String,
    }
    if let Some(r) = &st.result
        && let Ok(r) = serde_json::from_str::<R>(r)
        && !r.agent_type.is_empty()
    {
        return r.agent_type;
    }
    let inp: In = serde_json::from_str(st.input_json()).unwrap_or_default();
    if inp.subagent_type.is_empty() {
        "subagent".into()
    } else {
        inp.subagent_type
    }
}

/// A background task's status as rush says it: one you stopped is
/// stopped, though Claude Code calls it killed.
pub fn task_status(s: &str) -> &str {
    if s == "killed" { "stopped" } else { s }
}

/// The id in "Task #N created".
fn created_task_id(out: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(k) = out[from..].find("Task #") {
        let rest = &out[from + k + 6..];
        let n = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if n > 0 && rest[n..].starts_with(" created") {
            return Some(&rest[..n]);
        }
        from += k + 6;
    }
    None
}

pub(crate) const COMPACTED_HEAD: &str = "This conversation was compacted by ";
pub(crate) const COMPACTED_MID: &str =
    " to make room. Here is the summary of everything before this point:\n\n";
pub(crate) const COMPACTED_TAIL: &str = "\n\nTake this as what you know of the work so far. Reply only \"ready\" and wait for my next message.";

/// Draws a turn that asks for a compaction as rush's, not a message of
/// yours: /compact, or the summary a fresh conversation starts from, which
/// becomes its compaction divider.
fn compact_ask(t: &mut Turn) {
    let p = t.prompt.trim();
    if p == "/compact" || p.starts_with("/compact ") {
        t.cause = p.to_owned();
        t.prompt.clear();
        return;
    }
    let Some(rest) = p.strip_prefix(COMPACTED_HEAD) else {
        return;
    };
    let Some((by, rest)) = rest.split_once(COMPACTED_MID) else {
        return;
    };
    let summary = rest.split_once(COMPACTED_TAIL).map_or(rest, |(s, _)| s);
    t.items.push(Item::Compact {
        compact: Compacted {
            trigger: format!("by {by}").into(),
            before: 0,
            after: 0,
        },
        summary: summary.to_owned(),
    });
    t.cause = "carried on from a summary".into();
    t.prompt.clear();
}

/// The keys of a tool's structured result that anything reads: the
/// renderer's tool_summary (ToolSearch, Monitor, EnterWorktree), delivery
/// (SendMessage, PushNotification), stopped_command (TaskStop) and
/// agent_name (Agent, Task). Add a key here before reading it.
pub const RESULT_KEYS: [&str; 10] = [
    "agentType",
    "command",
    "localSent",
    "matches",
    "message",
    "persistent",
    "pushSent",
    "resumedAgentId",
    "timeoutMs",
    "worktreeBranch",
];

/// A result this long or shorter is kept as sent: reading it costs more
/// than it holds.
const SHORT_RESULT: usize = 128;

/// What's kept of a tool's structured result: a short one as sent, a long
/// one only for the keys anything reads (RESULT_KEYS). Else a long session
/// holds a copy of every file it read or edited, every shell's output its
/// text already has, and every screenshot an MCP tool gave back as base64.
/// One that isn't an object reads as no result at all, as it always did to
/// every reader; an object with no key read is kept as one with nothing
/// read in it (`{"":0}`), which delivery still tells from none.
pub fn slim_result(raw: &str) -> Option<Box<str>> {
    if raw.is_empty() {
        return None;
    }
    if raw.len() <= SHORT_RESULT {
        return Some(raw.into());
    }
    type Obj<'a> = BTreeMap<std::borrow::Cow<'a, str>, &'a RawValue>;
    let Ok(mut m) = serde_json::from_str::<Obj>(raw) else {
        return None;
    };
    m.retain(|k, _| RESULT_KEYS.contains(&k.as_ref()));
    if m.is_empty() {
        return Some(r#"{"":0}"#.into());
    }
    serde_json::to_string(&m).ok().map(String::into_boxed_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ids() {
        assert_eq!(
            created_task_id("Task #12 created successfully: x"),
            Some("12")
        );
        assert_eq!(created_task_id("Task #a_b created"), Some("a_b"));
        assert_eq!(created_task_id("Task # created"), None);
        assert_eq!(created_task_id("nothing"), None);
    }

    #[test]
    fn slims_results() {
        let big = "x".repeat(3000);
        let edit = format!(r#"{{"filePath":"/a","originalFile":"{big}","structuredPatch":[]}}"#);
        assert_eq!(slim_result(&edit).as_deref(), Some(r#"{"":0}"#));
        let read = format!(
            r#"{{"type":"text","file":{{"filePath":"/a","content":"{big}","numLines":3}}}}"#
        );
        assert_eq!(slim_result(&read).as_deref(), Some(r#"{"":0}"#));
        // Short: kept as sent.
        assert_eq!(
            slim_result(r#"{"originalFile":"x"}"#).as_deref(),
            Some(r#"{"originalFile":"x"}"#)
        );
        let bash = format!(r#"{{"stdout":"{big}"}}"#);
        assert_eq!(slim_result(&bash).as_deref(), Some(r#"{"":0}"#));
        assert_eq!(slim_result(""), None);
        // An MCP tool's blocks, a screenshot among them: nothing reads them.
        let shot = format!(r#"[{{"type":"image","source":{{"data":"{big}"}}}}]"#);
        assert_eq!(slim_result(&shot), None);
        // What's read stays, long or not.
        let sent = format!(r#"{{"message":"{big}","pushSent":true,"other":"{big}"}}"#);
        assert_eq!(
            slim_result(&sent).as_deref(),
            Some(format!(r#"{{"message":"{big}","pushSent":true}}"#).as_str())
        );
    }

    #[test]
    fn turn_costs() {
        let mut spent = 0.0;
        assert_eq!(turn_cost(&mut spent, 0.5), 0.5);
        assert_eq!(turn_cost(&mut spent, 0.75), 0.25);
        assert_eq!(turn_cost(&mut spent, 0.1), 0.1); // started again
    }
}
