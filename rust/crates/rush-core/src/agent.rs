//! An agent's session as rush's list shows it, and what it spent. After
//! Go's internal/agent (job.go, spend.go, agent.go, usage/tokens.go).

use crate::time::Ms;

/// What a session is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    Working,
    /// Waiting on you: a question or a permission.
    Blocked,
    Done,
    #[default]
    Stopped,
    /// A live process with nothing to do (an open terminal).
    Idle,
}

impl State {
    /// A state as an agent's records say it; "running" (a turn the
    /// agent hasn't classed yet) and anything unknown read as Done and
    /// Stopped.
    pub fn from_word(s: &str) -> State {
        match s {
            "working" => State::Working,
            "blocked" => State::Blocked,
            "done" | "running" => State::Done,
            "idle" => State::Idle,
            _ => State::Stopped,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "blocked",
            State::Done => "done",
            State::Stopped => "stopped",
            State::Idle => "idle",
        }
    }
}

/// One item of an agent's todo list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Todo {
    pub label: String,
    pub done: bool,
    pub started: bool,
}

/// Something an agent has running beside its turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// agent, shell, monitor
    pub kind: String,
    pub label: String,
    pub started_at: Ms,
}

/// A session as a list row shows it, whichever agent runs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job {
    pub id: String,
    pub account: String,
    pub name: String,
    pub state: State,
    pub detail: String,
    pub needs: String,
    pub intent: String,
    pub cwd: String,
    pub session_id: String,
    pub transcript_path: String,
    pub worktree_path: String,
    pub worktree_branch: String,
    /// Background tasks running or queued.
    pub in_flight: u32,
    /// What they are: shell commands, subagent names.
    pub background: Vec<String>,
    pub subagents: u32,
    pub running: Vec<Task>,
    pub todos: Vec<Todo>,
    pub created_at: Ms,
    pub updated_at: Ms,
    pub mod_time: Ms,
}

impl Job {
    /// Working, or waiting on you.
    pub fn live(&self) -> bool {
        matches!(self.state, State::Working | State::Blocked)
    }
    /// A finished turn whose background work is still running.
    pub fn busy(&self) -> bool {
        !self.live() && self.in_flight > 0 && !self.background.is_empty()
    }
    /// Anything with a live process, idle terminals included.
    pub fn open(&self) -> bool {
        self.live() || self.state == State::Idle
    }
}

/// Tokens by kind. `reasoning` is the part of `output` spent thinking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub reasoning: i64,
}

impl TokenUsage {
    pub fn add(&mut self, o: TokenUsage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write_5m += o.cache_write_5m;
        self.cache_write_1h += o.cache_write_1h;
        self.reasoning += o.reasoning;
    }
    /// Every token counted.
    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }
}

/// The error a session's last turn ended on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Halt {
    /// The agent's word for the error: rate_limit, server_error…
    pub kind: String,
    /// What it told the user, first line.
    pub text: String,
    pub at: Ms,
}

/// What a session's transcripts say it cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spend {
    pub cost: f64,
    pub usage: TokenUsage,
    pub model: String,
    pub first: Ms,
    pub last: Ms,
    pub today: f64,
    pub halt: Option<Halt>,
    /// Tokens its newest request sent: how full the context window is.
    pub context: i64,
    pub compacts: u32,
    /// Where its own transcript last worked.
    pub dir: String,
    /// When `dir` last changed.
    pub dir_at: Ms,
    /// Folders it worked in, subagents included.
    pub dirs: Vec<String>,
    /// Pull request URLs its transcripts mention.
    pub prs: Vec<String>,
    /// The last count it reported moving ("lint 11,065 → 9,052"), and when.
    pub progress: String,
    pub progress_at: Ms,
    /// Its transcripts have been priced.
    pub ready: bool,
}

/// A session's subagents: how many it ever spawned, and how many are
/// running directly and at any depth (agent.go:43).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubagentStats {
    pub spawned: u32,
    pub direct: u32,
    pub nested: u32,
    /// When the agents of a workflow still running last wrote: they work
    /// for the session as its subagents do, though none is counted here.
    pub workflow: Ms,
}

/// One message of a Preview.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewLine {
    /// user, assistant, tool
    pub role: String,
    pub text: String,
    pub at: Ms,
    pub doing: String,
}

/// What a session is doing and has lately said, read cheaply from the end
/// of its transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub text: String,
    pub tool: String,
    pub tool_arg: String,
    /// The tool call in words: "running pnpm test".
    pub doing: String,
    pub at: Ms,
    pub model: String,
    pub effort: String,
    pub last_user: String,
    pub context: i64,
    pub recent: Vec<PreviewLine>,
    pub first: PreviewLine,
}

/// A row of rush's agent list: the job plus what rush knows around it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agent {
    /// The agent it is: which adapter found it.
    pub kind: crate::adapter::Kind,
    pub job: Job,
    /// Stable key: "<harness>:<account>:<session id>".
    pub key: String,
    pub display_name: String,
    pub repo: String,
    pub branch: String,
    pub pid: i32,
    /// CPU percent and memory footprint of its process tree, and how many
    /// processes are in it.
    pub cpu: f32,
    pub mem: u64,
    pub procs: u32,
    pub interactive: bool,
    /// An interactive-kind session that is really `claude -p` driven by
    /// another program.
    pub headless: bool,
    /// Its turn just ended; the agent hasn't classified it yet.
    pub checking: bool,
    /// A conversation nothing has open, read from its transcript.
    pub past: bool,
    pub spend: Spend,
    pub subs: SubagentStats,

    // What rush knows around the session, as Go's fleet.Agent has it
    // (internal/fleet/fleet.go:29-113).
    /// The profile it runs in: the agent's config folder.
    pub acct: crate::adapter::Profile,
    /// Pinned in its agent's own list.
    pub pinned: bool,
    /// A rush session whose turn ended with decisions for you: it waits on
    /// you with its host asleep too. `decisions_key` is what that card
    /// asks, by its headings.
    pub decides: bool,
    pub decisions_key: String,
    /// When a hosted agent at work last said anything, set once its turn
    /// has heard nothing for a while with no tool running; 0 otherwise.
    pub stalled: Ms,
    /// Put away by you.
    pub done: bool,
    pub group: String,
    /// The process a background job runs in, under its agent's service.
    pub worker: Option<i32>,
    /// The main checkout of `repo`'s repository: `repo` itself, or for a
    /// linked worktree the checkout it was made from.
    pub root: String,
    /// The pull requests its agent linked to it.
    pub prs: Vec<crate::adapter::Pr>,
    /// The worktree a rush session works in, and its branch.
    pub workspace: Option<Workspace>,
    /// The pull request for its branch, as GitHub last said, else the
    /// first its agent linked; None when none is known.
    pub pr: Option<PrStatus>,
    /// rush's own advisor at work, not yours.
    pub advisor: bool,
    /// Its subagent runs still working, each for a tile of its own.
    pub subagents: Vec<SubagentTile>,
    /// The user has opened or answered this question already.
    pub seen: bool,
    /// A session rush runs itself, through a host process.
    pub rush: bool,
    /// The machine a rush session is really on, when this row stands in
    /// for it: nothing may signal, sample or read folders for it.
    pub peer: String,
    /// How much disk its temp work takes, as last measured.
    pub temp: i64,
    /// Its temp folders, for a row with temp work to show.
    pub scratch: Vec<crate::adapter::TempDir>,
    /// What a rush session wrote outside its project and scratch.
    pub left: Vec<Left>,
    /// The profile a rush session was started under.
    pub profile: String,
    /// Another agent's transcript, read through its adapter.
    pub history: String,
    /// A session running on its agent's own servers.
    pub remote: bool,
    /// Where its agent compacts its context, when set smaller than the
    /// model's window.
    pub compaction: crate::adapter::Compaction,
}

/// The worktree a rush session works in (Go's host.Workspace).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workspace {
    /// The worktree's top.
    pub path: String,
    /// The main checkout it belongs to.
    pub repo: String,
    /// "" when it's detached.
    pub branch: String,
    /// The commit the worktree started from, in full.
    pub base: String,
    /// The branch its work is meant to land on.
    pub target: String,
    /// rush made it for the session before its agent started.
    pub own: bool,
}

/// A place a session wrote outside its project and scratch (Go's
/// host.Left).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Left {
    pub path: String,
    /// Its last write there.
    pub at: Ms,
    /// Read from a shell command, so a guess.
    pub shell: bool,
}

/// A session's pull request, found by its branch (Go's ghpr.Status): its
/// state is open, draft, merged or closed; checks pass, fail or pending,
/// with the first failing one's name; review approved or changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrStatus {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub branch: String,
    pub state: String,
    pub checks: String,
    pub failing: String,
    pub review: String,
    pub head_oid: String,
    pub merged_at: Ms,
}

impl PrStatus {
    pub fn merged(&self) -> bool {
        self.state == "merged"
    }
    pub fn open(&self) -> bool {
        self.state == "open" || self.state == "draft"
    }
}

/// One of a session's subagent runs still working, as its own tile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubagentTile {
    pub id: String,
    pub kind: String,
    pub description: String,
    /// Its own transcript.
    pub path: String,
    /// Its checkout, when not its session's.
    pub worktree: String,
    /// The parent's call waiting on it.
    pub tool_use_id: String,
    /// When its transcript was last written.
    pub mod_time: Ms,
    /// When it began.
    pub born: Ms,
}
