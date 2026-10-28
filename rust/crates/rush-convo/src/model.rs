//! A session's events as turns, items and steps. Port of
//! internal/convo/model.go (Status, Step, Item, Turn, Task, Request,
//! ToolStat, Session, Apply, endTurn, message, words, call, notice,
//! interrupted, results, result, request, tasks), neutral.go:14-193
//! (applyNeutral, partStart, delta, compacted, turnEnd), usermessages.go:21
//! (sent), hooks.go:12-53, compactby.go:151-169 (compactAsk) and
//! host/host.go:695 (TurnCost).
//!
//! Steps live in one arena on the Session and are named by StepId; turns
//! name theirs by id, so a subagent's step can sit under its parent's call
//! and in its turn at once. Historic turns keep text, never drawn lines.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rush_core::agent::TokenUsage;
use rush_core::message::{Block, BlockKind, Image, Message, Role};
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::text::{
    between, exit_code_in, first_line, first_non_empty, first_plain, is_rejection, worktree_in,
};
use crate::tool::{Call, Kind, Output, claude_call, claude_kind_of, claude_output};

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

/// One tool call.
#[derive(Clone, Debug)]
pub struct Step {
    pub id: Box<str>,
    /// The tool, in Claude Code's words: "Bash", "Read", "mcp__x__y".
    pub tool: Box<str>,
    /// What the call does.
    pub kind: Kind,
    /// The input as Claude Code sent it, JSON; None when it had none.
    pub input: Option<Box<RawValue>>,
    pub status: Status,
    /// The tool result as text.
    pub output: Box<str>,
    /// Claude Code's structured result (patches, stdout/stderr) as JSON,
    /// with what nothing draws dropped (slim_result).
    pub result: Option<Box<str>>,
    /// Bash exit code, -1 when unknown.
    pub exit: i32,
    pub start: Ms,
    pub end: Ms,
    /// A subagent's own steps.
    pub children: Vec<StepId>,
    /// What the tool gave back as pictures: a Read of one, a screenshot.
    pub images: Vec<Image>,
    /// The subagent call it was made under; None for the main agent's.
    pub parent: Option<StepId>,
    /// The turn whose steps hold it (an index into Session::turns).
    pub turn: Option<u32>,
    /// What it waits on you for, while it does.
    pub approval: Option<Box<Asking>>,
}

/// What a step waits on you for: leave to run (model.go:98).
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
}

impl Step {
    /// The input JSON, "" when it had none.
    pub fn input_json(&self) -> &str {
        self.input.as_deref().map_or("", RawValue::get)
    }

    /// The structured result JSON, "" when there's none.
    pub fn result_json(&self) -> &str {
        self.result.as_deref().unwrap_or("")
    }

    /// The call as rush's own (path, command, pattern, edits…), read from
    /// its input each time it's asked: a renderer keeps what it needs.
    pub fn call(&self) -> Call {
        let mut c = claude_call(&self.id, &self.tool, self.input_json());
        if self.kind != Kind::Other {
            c.kind = self.kind;
        }
        c
    }

    /// How the call came out, as rush's own: its streams, exit, the hunks it
    /// changed, the lines it read. Empty until it's back.
    pub fn out(&self) -> Output {
        let c = Call {
            id: self.id.to_string(),
            name: self.tool.to_string(),
            kind: self.kind,
            ..Call::default()
        };
        claude_output(
            &c,
            &self.output,
            self.status == Status::Failed,
            self.result_json(),
        )
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
    pub pictures: Vec<Image>,
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
    pub pictures: Vec<Image>,
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

/// A part of a message being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Text,
    Thinking,
    ToolCall,
}

/// How a turn ended.
#[derive(Clone, Debug, Default)]
pub struct TurnEnd {
    /// done, interrupted, error, or what went wrong in the agent's words
    /// (max_turns).
    pub reason: String,
    /// The process's cost total so far.
    pub cost: f64,
    pub err: String,
}

/// What a Session takes in.
#[derive(Clone, Debug)]
pub enum Event {
    /// You sent a message.
    Sent {
        text: String,
        images: Vec<Box<str>>,
    },
    /// A whole message, in Claude Code's words (tool names, inputs and
    /// results as it writes them).
    Message(Message),
    /// A part of the message being written began.
    PartStart(Part),
    /// A piece of the message being written.
    Delta {
        part: Part,
        text: String,
    },
    Compacted(Compacted),
    /// The agent's busy word ("compacting", "requesting"); "" when idle.
    Status(String),
    TurnEnd(TurnEnd),
    /// The agent asks leave to make tool call `call` (name `tool`) before
    /// it runs it (neutral.go:58).
    Approval {
        call: String,
        tool: String,
        ask: Asking,
    },
    /// The approval with this request id was answered or withdrawn.
    Settled(String),
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
    /// Tokens in the context window after the last request.
    pub context: i64,
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

    /// The effort the last turn ran at, else the latest an earlier one did.
    pub fn effort(&self) -> &str {
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

    /// Folds one event into the session at time now (0 when unknown).
    pub fn apply(&mut self, ev: Event, now: Ms) {
        self.applied += 1;
        self.saw(now);
        if let Event::Sent { text, images } = ev {
            self.sent(text, images, now);
            return;
        }
        if let Some(i) = self.live_idx()
            && now > self.turns[i].heard
        {
            self.turns[i].heard = now;
        }
        match ev {
            Event::Sent { .. } => {}
            Event::Message(m) => self.message(m, now),
            Event::PartStart(p) => self.part_start(p, now),
            Event::Delta { part, text } => self.delta(part, &text, now),
            Event::Compacted(c) => self.compacted(c, now),
            Event::Status(text) => {
                if text == "compacting" {
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
            Event::TurnEnd(e) => self.turn_end(&e, now),
            Event::Approval { call, tool, ask } => {
                self.ensure_step(&call, &tool, now);
                self.ask(&call, ask);
            }
            Event::Settled(id) => self.settle(&id),
        }
    }

    /// The approvals waiting, oldest first (model.go:413).
    pub fn pending(&self) -> Vec<StepId> {
        self.asked
            .iter()
            .copied()
            .filter(|&sid| self.steps[sid as usize].approval.is_some())
            .collect()
    }

    /// Makes a step of a call an approval is about, when the agent asks
    /// before it says it's making the call (neutral.go:195).
    fn ensure_step(&mut self, call: &str, tool: &str, now: Ms) {
        if call.is_empty() || self.by_id.contains_key(call) {
            return;
        }
        let mut b = Block::new(BlockKind::ToolUse);
        b.id = call.to_string();
        b.name = tool.to_string();
        self.message(
            Message {
                role: Role::Assistant,
                id: String::new(),
                model: String::new(),
                uuid: String::new(),
                parent_tool_use_id: String::new(),
                blocks: vec![b],
                usage: None,
                tool_result: None,
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

    /// An approval answered or withdrawn: its step runs on (model.go:553).
    fn settle(&mut self, request: &str) {
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
    /// turn, and its item when it was said mid-turn.
    pub(crate) fn sent(
        &mut self,
        text: String,
        images: Vec<Box<str>>,
        now: Ms,
    ) -> (usize, Option<usize>) {
        self.saw(now);
        if let Some(i) = self.live_idx() {
            let t = &mut self.turns[i];
            t.items.push(Item::Interject(Box::new(Interject {
                text,
                images,
                pictures: Vec::new(),
            })));
            t.touch();
            return (i, Some(t.items.len() - 1));
        }
        self.streaming = None;
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
        self.turns.len() - 1
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

    fn message(&mut self, m: Message, now: Ms) {
        if m.role != Role::Assistant {
            // Tool results belong to the turn their call is in, however
            // late they arrive; they never open a turn of their own.
            self.results(m, now);
            return;
        }
        let parent = self.by_id.get(m.parent_tool_use_id.as_str()).copied();
        // A subagent's message whose run we never saw start still isn't the
        // main agent's: it mustn't land in the turn or its numbers.
        let sub = !m.parent_tool_use_id.is_empty();
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
        if let Some(u) = m.usage {
            self.request(&m, parent, now);
            if !sub && let Some(ti) = t {
                self.context =
                    u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h + u.output;
                if !m.model.is_empty() {
                    self.turns[ti].model = self.intern_model(&m.model);
                }
            }
        }
        for b in m.blocks {
            match b.kind {
                BlockKind::Text => {
                    if !sub
                        && !b.text.trim().is_empty()
                        && let Some(ti) = t
                    {
                        self.words(ti, b.text); // a subagent's words stay inside it
                    }
                }
                BlockKind::Thinking => {
                    if !sub && let Some(ti) = t {
                        let items = &mut self.turns[ti].items;
                        if !matches!(items.last(), Some(Item::Thinking(_))) {
                            items.push(Item::Thinking(b.text));
                        }
                    }
                }
                BlockKind::ToolUse => self.call(b, parent, t, sub, now),
                BlockKind::ToolResult => {}
            }
        }
        if let Some(ti) = t {
            self.turns[ti].touch();
        }
    }

    /// The main agent's words in turn ti: what streamed in, made whole.
    fn words(&mut self, ti: usize, text: String) {
        if let Some((st, si)) = self.streaming.take()
            && let Some(Item::Text { text: t, .. }) = self
                .turns
                .get_mut(st as usize)
                .and_then(|t| t.items.get_mut(si as usize))
        {
            *t = text;
            return;
        }
        self.turns[ti].items.push(Item::Text {
            text,
            answer: false,
        });
    }

    /// Makes a step of a tool call, in turn t under parent.
    fn call(&mut self, b: Block, parent: Option<StepId>, t: Option<usize>, sub: bool, now: Ms) {
        if !sub {
            // A background subagent's call lands mid-stream too: it doesn't
            // end the main agent's words, or they'd be drawn twice.
            self.streaming = None;
        }
        let tool: Box<str> = b.name.into_boxed_str();
        self.tools.entry(tool.clone()).or_default().calls += 1;
        let input = b.input;
        if !sub {
            let wt = worktree_in(input.as_deref().map_or("", RawValue::get));
            if !wt.is_empty() && wt != self.worktree {
                self.worktree = wt.to_owned();
            }
        }
        let kind = claude_kind_of(&tool);
        let id: Box<str> = b.id.into_boxed_str();
        let sid = self.steps.len() as StepId;
        self.steps.push(Step {
            id: id.clone(),
            tool,
            kind,
            input,
            status: Status::Running,
            output: Box::default(),
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

    fn results(&mut self, m: Message, now: Ms) {
        for b in &m.blocks {
            if b.kind == BlockKind::Text
                && b.text.trim().starts_with("[Request interrupted by user")
            {
                self.interrupted(now);
                return;
            }
        }
        // Text Claude Code injects as a user message (a background task
        // finishing, another session's message) starts a turn of its own.
        for b in &m.blocks {
            if b.kind == BlockKind::Text
                && b.text.trim().starts_with('<')
                && let Some((from, text)) = crate::transcript::injected(&b.text)
                && self.live_idx().is_none()
            {
                self.note_task(&b.text);
                self.apply(
                    Event::Sent {
                        text,
                        images: Vec::new(),
                    },
                    now,
                );
                if let Some(t) = self.turns.last_mut() {
                    t.from = from;
                }
            }
        }
        let raw = m.tool_result;
        for b in m.blocks {
            if b.kind != BlockKind::ToolResult {
                continue;
            }
            let id = self.by_id.get(b.tool_use_id.as_str()).copied();
            let images = self.result(b, raw.as_deref(), now);
            // The images it gave back are drawn under its step.
            if let Some(sid) = id
                && !images.is_empty()
            {
                self.steps[sid as usize].images.extend(images);
                self.touch_step(sid);
            }
        }
    }

    /// Takes in how a call came out; gives back the images it carried.
    fn result(&mut self, b: Block, raw: Option<&RawValue>, now: Ms) -> Vec<Image> {
        let Some(&sid) = self.by_id.get(b.tool_use_id.as_str()) else {
            return Vec::new();
        };
        let kind = self.steps[sid as usize].kind;
        let raw = raw.map(RawValue::get).unwrap_or("");
        let mut text = b.text;
        if text.is_empty() && kind == Kind::Shell && !raw.is_empty() {
            // Claude Code's text was empty: its streams, as resultText has it.
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Streams {
                stdout: String,
                stderr: String,
            }
            let s: Streams = serde_json::from_str(raw).unwrap_or_default();
            if !s.stdout.is_empty() || !s.stderr.is_empty() {
                text = format!("{}\n{}", s.stdout, s.stderr)
                    .trim_end_matches('\n')
                    .to_owned();
            }
        }
        let rejected = b.is_error && is_rejection(&text);
        let st = &mut self.steps[sid as usize];
        st.output = text.into_boxed_str();
        st.result = slim_result(kind, raw);
        st.end = now;
        st.status = match st.status {
            Status::Denied => Status::Denied,
            _ if rejected => Status::Denied,
            _ if b.is_error => Status::Failed,
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
        b.images
    }

    /// One model call's message: its id, model, usage. Claude Code sends
    /// one message per content block, all with the same id and usage, so a
    /// repeat replaces the last.
    fn request(&mut self, m: &Message, parent: Option<StepId>, now: Ms) {
        if m.model == "<synthetic>" {
            return; // Claude Code's own placeholder, not a model call
        }
        let agent: Box<str> =
            parent.map_or_else(Box::default, |p| agent_name(&self.steps[p as usize]).into());
        let u = m.usage.unwrap_or_default();
        let mut r = Request {
            id: m.id.as_str().into(),
            at: now,
            model: self.intern_model(&m.model),
            agent,
            run: m.parent_tool_use_id.as_str().into(),
            usage: TokenUsage {
                input: u.input,
                output: u.output,
                cache_read: u.cache_read,
                cache_write_5m: u.cache_write_5m,
                cache_write_1h: u.cache_write_1h,
                reasoning: 0,
            },
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
                #[derive(Deserialize)]
                struct In {
                    #[serde(default)]
                    todos: Vec<crate::tool::ClaudeTodo>,
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
    fn part_start(&mut self, p: Part, now: Ms) {
        let ti = self.turn_for(now);
        let t = &mut self.turns[ti];
        t.thinking = 0;
        if p == Part::Thinking {
            t.thinking = now;
            if !matches!(t.items.last(), Some(Item::Thinking(_))) {
                t.items.push(Item::Thinking(String::new()));
            }
        }
        if p != Part::Text {
            self.streaming = None;
        }
        t.touch();
    }

    /// A piece of the message being written.
    fn delta(&mut self, p: Part, text: &str, now: Ms) {
        let ti = self.turn_for(now);
        let t = &mut self.turns[ti];
        t.streamed += text.len();
        match p {
            Part::ToolCall => {}
            Part::Thinking => {
                if !matches!(t.items.last(), Some(Item::Thinking(_))) {
                    t.items.push(Item::Thinking(String::new()));
                }
                if let Some(Item::Thinking(s)) = t.items.last_mut() {
                    s.push_str(text);
                }
            }
            Part::Text => {
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
    fn turn_end(&mut self, e: &TurnEnd, now: Ms) {
        if e.reason == "interrupted" {
            self.interrupted(now);
            return;
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
        let input = serde_json::to_string(&BTreeMap::from([
            ("command", cmd),
            ("description", "you ran"),
        ]))
        .ok()
        .and_then(|s| RawValue::from_string(s).ok());
        let sid = self.steps.len() as StepId;
        self.steps.push(Step {
            id: id.clone(),
            tool: "Bash".into(),
            kind: Kind::Shell,
            input,
            status: Status::Running,
            output: Box::default(),
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
        self.apply(
            Event::TurnEnd(TurnEnd {
                reason: "done".into(),
                ..TurnEnd::default()
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

/// Drops from a tool's result what nothing draws: the whole file an edit
/// or write was made to, and a read file's text, which its output already
/// has. Else a long session holds a copy of every file it touched.
pub fn slim_result(k: Kind, raw: &str) -> Option<Box<str>> {
    if raw.is_empty() {
        return None;
    }
    if raw.len() < 2 << 10 || !matches!(k, Kind::Edit | Kind::Write | Kind::Read) {
        return Some(raw.into());
    }
    type Obj<'a> = BTreeMap<std::borrow::Cow<'a, str>, &'a RawValue>;
    let Ok(mut m) = serde_json::from_str::<Obj>(raw) else {
        return Some(raw.into());
    };
    let file: Box<RawValue>;
    match k {
        Kind::Edit | Kind::Write => {
            if m.remove("originalFile").is_none() {
                return Some(raw.into());
            }
        }
        _ => {
            let Some(f) = m.get("file") else {
                return Some(raw.into());
            };
            let Ok(mut f) = serde_json::from_str::<Obj>(f.get()) else {
                return Some(raw.into());
            };
            let text = f.remove("content").is_some();
            let image = f.remove("base64").is_some();
            if !text && !image {
                return Some(raw.into());
            }
            let Some(b) = serde_json::to_string(&f)
                .ok()
                .and_then(|s| RawValue::from_string(s).ok())
            else {
                return Some(raw.into());
            };
            file = b;
            m.insert("file".into(), &file);
        }
    }
    match serde_json::to_string(&m) {
        Ok(s) => Some(s.into_boxed_str()),
        Err(_) => Some(raw.into()),
    }
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
        assert_eq!(
            slim_result(Kind::Edit, &edit).as_deref(),
            Some(r#"{"filePath":"/a","structuredPatch":[]}"#)
        );
        let read = format!(
            r#"{{"type":"text","file":{{"filePath":"/a","content":"{big}","numLines":3}}}}"#
        );
        assert_eq!(
            slim_result(Kind::Read, &read).as_deref(),
            Some(r#"{"file":{"filePath":"/a","numLines":3},"type":"text"}"#)
        );
        // Small, or of a kind nothing drops from: kept as sent.
        assert_eq!(
            slim_result(Kind::Edit, r#"{"originalFile":"x"}"#).as_deref(),
            Some(r#"{"originalFile":"x"}"#)
        );
        let bash = format!(r#"{{"stdout":"{big}"}}"#);
        assert_eq!(
            slim_result(Kind::Shell, &bash).as_deref(),
            Some(bash.as_str())
        );
        assert_eq!(slim_result(Kind::Shell, ""), None);
    }

    #[test]
    fn turn_costs() {
        let mut spent = 0.0;
        assert_eq!(turn_cost(&mut spent, 0.5), 0.5);
        assert_eq!(turn_cost(&mut spent, 0.75), 0.25);
        assert_eq!(turn_cost(&mut spent, 0.1), 0.1); // started again
    }
}
