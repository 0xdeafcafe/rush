//! The tasks beside a turn: shells, monitors, subagents and workflows the
//! agent runs, in the foreground or the background, and the turn one wakes.
//! Port of internal/convo/jobs.go:17-397 (Job, applyJob, reopenJob,
//! backgroundNow, jobCalls, noteWake, wake, wakeFrom, endJobs, JobCommand,
//! JobKind) and words.go:201 (taskType). Go's wake also names the agents a
//! found subagent run ran (spawn.go); the Rust port doesn't link runs yet.

use std::collections::{BTreeMap, HashSet};

use rush_core::event::{self, Event, TaskKind};
use rush_core::time::Ms;
use rush_core::tool::{Call, Input, Kind};
use rush_core::wire::RawJson;
use serde_json::value::RawValue;

use super::{Session, Status, Step, StepId};
use crate::text::{first_line, first_non_empty};

/// How soon after a task ends or fires a turn with no message is put down
/// to it.
const WAKE_WINDOW: Ms = 2 * 60 * 1000;

/// One of the agent's tasks: a shell command, a monitor, a subagent or a
/// workflow, whether the turn waits on it or it runs in the background.
/// Only a session rush runs hears of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    /// The tool call that started it.
    pub tool_use_id: String,
    /// local_bash, local_agent, monitor_mcp, local_workflow, ...
    pub kind_word: String,
    /// What the agent calls it: a description, or the command.
    pub label: String,
    /// A subagent's type.
    pub agent: String,
    /// Runs beside the turn rather than holding it.
    pub background: bool,
    /// Empty while it runs; then completed, failed, stopped, or ended when
    /// it went without a word (its process did).
    pub status: String,
    pub error: String,
    pub start: Ms,
    pub end: Ms,
    /// Where its output is, once it's done.
    pub output_file: String,
    /// Its latest progress, or how it ended.
    pub summary: String,
    pub last_tool: String,
    /// The last reported change in progress, not a heartbeat.
    pub progress_at: Ms,
    pub tokens: i64,
    pub tool_uses: i64,
    /// A workflow's phases, and its agents in the order it started them.
    pub phases: Vec<String>,
    pub agents: Vec<event::WorkflowAgent>,
}

impl Job {
    pub fn running(&self) -> bool {
        self.status.is_empty()
    }

    /// What it is, in a word: shell, monitor, subagent, workflow, task.
    pub fn kind(&self) -> &'static str {
        match self.kind_word.as_str() {
            "local_bash" => "shell",
            "monitor_mcp" | "monitor_ws" => "monitor",
            "local_agent" | "remote_agent" | "in_process_teammate" => "subagent",
            "local_workflow" => "workflow",
            _ => "task",
        }
    }
}

/// The agent's own word for a kind of task (words.go:201).
fn task_type(k: TaskKind) -> &'static str {
    match k {
        TaskKind::Shell => "local_bash",
        TaskKind::Subagent => "local_agent",
        TaskKind::Monitor => "monitor_mcp",
        TaskKind::Workflow => "local_workflow",
        TaskKind::Other => "task",
    }
}

/// The agent's word for how a task ended, as rush says it.
fn job_status(st: &str) -> &str {
    match st {
        "killed" | "stopped" => "stopped",
        "" | "running" | "pending" => "",
        _ => st,
    }
}

/// Whether a task's name is a shell command and not a description of one:
/// a description is words, a command has a shell's punctuation, a path or
/// a flag in it. A guess, as Go's: a one-word command (make) reads as words.
fn looks_like_command(s: &str) -> bool {
    ["&&", "||", ";", "|", ">", "$(", "/", " -", "=", "\n"]
        .iter()
        .any(|m| s.contains(m))
}

impl Session {
    /// Every task heard of, in the order they started.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    /// The task with this id.
    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// The tasks but subagents' runs: the shells, monitors and workflows
    /// the background view lists.
    pub fn work_jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|j| self.job_kind(j) != "subagent")
    }

    /// The tasks still running, in the order they started.
    pub fn running_jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|j| j.running())
    }

    /// The task of the subagent run with this agent id or started by this
    /// tool call.
    pub fn subagent_job(&self, id: &str, tool_use_id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| {
            self.job_kind(j) == "subagent"
                && (j.id == id || !tool_use_id.is_empty() && j.tool_use_id == tool_use_id)
        })
    }

    /// The subagent runs' tasks by agent id and by "call:" and the tool
    /// call that started each: subagent_job for many runs at once, as a
    /// view asks for every run's on every frame (jobs.go:89 SubagentJobs).
    pub fn subagent_jobs(&self) -> std::collections::HashMap<String, &Job> {
        let mut out = std::collections::HashMap::new();
        for j in &self.jobs {
            if self.job_kind(j) != "subagent" {
                continue;
            }
            out.entry(j.id.clone()).or_insert(j);
            if !j.tool_use_id.is_empty() {
                out.entry(format!("call:{}", j.tool_use_id)).or_insert(j);
            }
        }
        out
    }

    /// Whether a task tool call id started is still running: a command sent
    /// to the background, long after its step returned.
    pub fn job_running(&self, call_id: &str) -> bool {
        self.jobs
            .iter()
            .any(|j| j.tool_use_id == call_id && j.running())
    }

    /// What a shell or monitor task runs, from the tool call that started
    /// it; empty when that call isn't in the conversation.
    pub fn job_command(&self, j: &Job) -> String {
        self.by_id
            .get(j.tool_use_id.as_str())
            .map(|&sid| {
                self.steps[sid as usize]
                    .call
                    .input
                    .command
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default()
    }

    /// Job::kind, knowing a Monitor tool's command from a Bash one: both are
    /// shell tasks to Claude Code. A task heard of only from its end is
    /// known by the tool call that started it.
    pub fn job_kind(&self, j: &Job) -> &'static str {
        let tool = self
            .by_id
            .get(j.tool_use_id.as_str())
            .map(|&sid| &*self.steps[sid as usize].tool);
        match (j.kind_word.as_str(), tool) {
            ("local_bash", Some("Monitor")) => "monitor",
            ("", Some("Agent" | "Task")) => "subagent",
            _ => j.kind(),
        }
    }

    /// The task with this id, made when it's new.
    fn job_at(&mut self, id: &str, now: Ms) -> usize {
        if let Some(i) = self.jobs.iter().position(|j| j.id == id) {
            return i;
        }
        self.jobs.push(Job {
            id: id.to_owned(),
            start: now,
            ..Job::default()
        });
        self.jobs.len() - 1
    }

    /// Folds in what the agent says of its tasks (jobs.go:166).
    pub(super) fn apply_job(&mut self, ev: &Event, now: Ms) {
        match ev {
            Event::TaskStarted(e) => {
                let i = self.job_at(&e.id, now);
                // Listed in the background a moment before (its list can
                // come first), it's there still.
                let listed = self.jobs[i].running() && self.jobs[i].background;
                self.reopen_job(i, now);
                let j = &mut self.jobs[i];
                // A subagent a message woke runs again under the message's
                // call: the call that started it stays its own.
                if e.kind != TaskKind::Subagent
                    || j.tool_use_id.is_empty()
                    || j.tool_use_id.starts_with("job:")
                {
                    j.tool_use_id = e.call_id.clone();
                }
                j.kind_word = task_type(e.kind).into();
                j.background = e.background || listed;
                j.label = first_non_empty(&[&e.label, &j.label]).to_owned();
                j.agent = first_non_empty(&[&e.agent, &j.agent]).to_owned();
                self.job_calls(now);
            }
            Event::TaskUpdated(e) => {
                let i = self.job_at(&e.id, now);
                let j = &mut self.jobs[i];
                if let Some(b) = e.background {
                    j.background = b;
                }
                if !e.label.is_empty() {
                    j.label = e.label.clone();
                }
                if !e.err.is_empty() {
                    j.error = e.err.clone();
                }
                let st = job_status(&e.status);
                if !st.is_empty() && (j.running() || j.status == "ended") {
                    j.status = st.to_owned();
                    j.end = now;
                }
            }
            Event::TaskProgress(e) => {
                let i = self.job_at(&e.id, now);
                let j = &mut self.jobs[i];
                if !e.summary.is_empty() && e.summary != j.summary
                    || !e.last_tool.is_empty() && e.last_tool != j.last_tool
                    || e.tokens > j.tokens
                    || e.tool_uses > j.tool_uses
                {
                    j.progress_at = now;
                }
                j.summary = first_non_empty(&[&e.summary, &j.summary]).to_owned();
                j.last_tool = first_non_empty(&[&e.last_tool, &j.last_tool]).to_owned();
                j.tokens = j.tokens.max(e.tokens);
                j.tool_uses = j.tool_uses.max(e.tool_uses);
                if !e.phases.is_empty() {
                    j.phases = e.phases.clone();
                }
                if !e.agents.is_empty() {
                    j.agents = event::merge_agents(std::mem::take(&mut j.agents), e.agents.clone());
                    j.progress_at = now;
                }
                if self.job_kind(&self.jobs[i]) == "monitor" {
                    self.note_wake(i, now); // each thing a monitor sees can wake the agent
                }
            }
            Event::TaskDone(e) => {
                let i = self.job_at(&e.id, now);
                let j = &mut self.jobs[i];
                j.tool_use_id = first_non_empty(&[&j.tool_use_id, &e.call_id]).to_owned();
                j.output_file = first_non_empty(&[&e.output_file, &j.output_file]).to_owned();
                if j.running() || j.status == "ended" {
                    j.status = first_non_empty(&[job_status(&e.status), "completed"]).to_owned();
                }
                if j.end == 0 {
                    j.end = now;
                }
                if !e.summary.is_empty() && e.summary != j.label {
                    j.summary = e.summary.clone();
                }
                let st = first_non_empty(&[&j.status, "completed"]).into();
                self.task_status.insert(e.id.as_str().into(), st);
                self.note_wake(i, now);
            }
            Event::Background(e) => {
                let list: Vec<event::BackgroundTask> = e
                    .tasks
                    .iter()
                    .map(|t| {
                        let mut t = t.clone();
                        if t.kind != TaskKind::Other {
                            t.kind_word = task_type(t.kind).into();
                        }
                        t
                    })
                    .collect();
                self.background_now(&list, now);
                self.job_calls(now);
            }
            _ => {}
        }
    }

    /// Starts again a task that had ended: a subagent a message was sent to
    /// after it finished runs again under the same id.
    fn reopen_job(&mut self, i: usize, now: Ms) {
        let j = &mut self.jobs[i];
        if j.running() {
            return;
        }
        j.status.clear();
        j.error.clear();
        j.output_file.clear();
        j.summary.clear();
        j.last_tool.clear();
        j.end = 0;
        j.start = now;
        j.progress_at = 0;
        j.tokens = 0;
        j.tool_uses = 0;
        j.phases.clear();
        j.agents.clear();
        self.task_status.remove(j.id.as_str());
    }

    /// Takes the agent's list of what runs in the background: each one
    /// there is running and backgrounded, and a backgrounded one no longer
    /// there has ended (how comes after, if the agent says).
    fn background_now(&mut self, list: &[event::BackgroundTask], now: Ms) {
        let mut on = HashSet::new();
        for t in list {
            on.insert(t.id.as_str());
            let i = self.job_at(&t.id, now);
            self.reopen_job(i, now);
            let j = &mut self.jobs[i];
            j.background = true;
            j.kind_word = first_non_empty(&[&j.kind_word, &t.kind_word]).to_owned();
            j.label = first_non_empty(&[&j.label, &t.label]).to_owned();
        }
        for j in &mut self.jobs {
            if j.running() && j.background && !on.contains(j.id.as_str()) {
                j.status = "ended".into();
                j.end = now;
            }
        }
    }

    /// Stands a call in for each shell task whose own isn't in the
    /// conversation, when what it's called is its command: so it's drawn,
    /// timed and its files found as any other. Under its call's id when it
    /// has one, so the call itself, should it come after all, takes its
    /// place.
    fn job_calls(&mut self, now: Ms) {
        for i in 0..self.jobs.len() {
            let j = &self.jobs[i];
            if j.kind_word != "local_bash"
                || self.by_id.contains_key(j.tool_use_id.as_str())
                || !looks_like_command(&j.label)
            {
                continue;
            }
            if j.tool_use_id.is_empty() {
                self.jobs[i].tool_use_id = format!("job:{}", self.jobs[i].id);
            }
            let j = &self.jobs[i];
            let raw = serde_json::to_string(&BTreeMap::from([("command", j.label.as_str())]))
                .ok()
                .and_then(|s| RawValue::from_string(s).ok())
                .map(RawJson);
            let id: Box<str> = j.tool_use_id.as_str().into();
            let call = Box::new(Call {
                id: j.tool_use_id.clone(),
                name: "Bash".into(),
                kind: Kind::Shell,
                input: Input {
                    command: j.label.clone(),
                    ..Input::default()
                },
                raw,
                ..Call::default()
            });
            let start = if j.start != 0 { j.start } else { now };
            let sid = self.steps.len() as StepId;
            self.steps.push(Step {
                id: id.clone(),
                tool: "Bash".into(),
                kind: Kind::Shell,
                call,
                claude: None,
                status: Status::Ok,
                output: Box::default(),
                out: None,
                result: None,
                exit: -1,
                start,
                end: 0,
                children: Vec::new(),
                images: Vec::new(),
                parent: None,
                turn: None,
                approval: None,
            });
            self.by_id.insert(id, sid);
            self.step_ver += 1;
        }
    }

    /// Remembers a task that ended or fired: the agent is woken for it
    /// without a message, and the turn it starts says so. One that comes
    /// while a turn runs is held until the turn ends, and wakes it then.
    fn note_wake(&mut self, i: usize, now: Ms) {
        self.woke = Some(i);
        self.woke_at = now;
    }

    /// The task that woke a turn opened at now, if one did; forgotten
    /// either way (model.go:442).
    pub(super) fn take_wake(&mut self, now: Ms) -> Option<usize> {
        let w = self.woke.take()?;
        (now - self.woke_at < WAKE_WINDOW).then_some(w)
    }

    /// Says who started turn ti, a turn task i woke (jobs.go:331).
    pub(super) fn wake(&mut self, ti: usize, i: usize) {
        let j = &self.jobs[i];
        let from = self.wake_from(j);
        let mut cause =
            first_non_empty(&[&j.label, &j.summary, &self.job_command(j), &j.id]).to_owned();
        let mut command = String::new();
        let k = self.job_kind(j);
        if (k == "shell" || k == "monitor") && cause.trim().contains('\n') {
            // A shell task's description is its command, heredoc and all.
            command = cause.trim().to_owned();
            cause = first_line(&cause).to_owned();
        }
        let t = &mut self.turns[ti];
        t.from = from;
        t.cause = cause;
        t.command = command;
        t.touch();
    }

    /// Who started a turn a task woke: "background shell · completed",
    /// "monitor · fired".
    fn wake_from(&self, j: &Job) -> String {
        let kind = self.job_kind(j);
        if kind == "monitor" && j.running() {
            return "monitor · fired".into();
        }
        let mut from = match kind {
            "shell" | "subagent" | "workflow" => format!("background {kind}"),
            _ => kind.to_owned(),
        };
        if !j.status.is_empty() {
            from.push_str(" · ");
            from.push_str(&j.status);
        }
        from
    }

    /// Ends the tasks a finished turn was waiting on: they ran in the
    /// foreground, so they were done by the time it was.
    pub(super) fn end_jobs(&mut self, now: Ms) {
        for j in &mut self.jobs {
            if j.running() && !j.background {
                j.status = "completed".into();
                j.end = now;
                if let Some(&sid) = self.by_id.get(j.tool_use_id.as_str())
                    && self.steps[sid as usize].status == Status::Failed
                {
                    j.status = "failed".into();
                }
            }
        }
    }
}
