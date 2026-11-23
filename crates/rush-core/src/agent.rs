//! An agent's session as rush's list shows it, and what it spent. After
//! Go's internal/agent (job.go, spend.go, agent.go, usage/tokens.go).

use crate::time::Ms;

/// What a session is doing.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Todo {
    pub label: String,
    pub done: bool,
    pub started: bool,
}

/// Something an agent has running beside its turn.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Task {
    /// Its agent's id for it, when its host says (host.Task's ID): what
    /// a background command's result names it by.
    pub id: String,
    /// agent, shell, monitor
    pub kind: String,
    pub label: String,
    pub started_at: Ms,
}

/// A session as a list row shows it, whichever agent runs it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Job {
    pub id: String,
    /// When the rush its host runs was written; 0 for a session rush
    /// doesn't host, or a host that didn't say. Rust-only.
    #[serde(skip_serializing_if = "is_zero_ms")]
    pub host_exe: Ms,
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Halt {
    /// The agent's word for the error: rate_limit, server_error…
    pub kind: String,
    /// What it told the user, first line.
    pub text: String,
    pub at: Ms,
}

/// What a session's transcripts say it cost.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Spend {
    pub cost: f64,
    pub usage: TokenUsage,
    /// The model it last used, and where its own transcript last worked:
    /// each shared with every other row saying the same (rush_core::name).
    pub model: crate::name::Name,
    pub first: Ms,
    pub last: Ms,
    pub today: f64,
    pub halt: Option<Halt>,
    /// Tokens its newest request sent: how full the context window is.
    pub context: i64,
    pub compacts: u32,
    /// Where its own transcript last worked.
    pub dir: crate::name::Name,
    /// When `dir` last changed.
    pub dir_at: Ms,
    /// Folders it worked in, subagents included: shared with the spend
    /// scanner's own and every copy of the row, as most are the same few.
    pub dirs: Vec<std::sync::Arc<str>>,
    /// Pull request URLs its transcripts mention.
    pub prs: Vec<String>,
    /// The last count it reported moving ("lint 11,065 → 9,052"), and when.
    pub progress: String,
    pub progress_at: Ms,
    /// What its subagents' prompt cache lapsing cost (Rust-only).
    pub sub_cache: crate::subcache::SubCache,
    /// Its transcripts have been priced.
    pub ready: bool,
}

/// A session's subagents: how many it ever spawned, and how many are
/// running directly and at any depth (agent.go:43).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SubagentStats {
    pub spawned: u32,
    pub direct: u32,
    pub nested: u32,
    /// How many of the runs it started, folded under it, ask you something
    /// live to answer: it needs you with its own host asleep too
    /// (Agent::sub_asks). Rust-only.
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub asking: u32,
    /// When the agents of a workflow still running last wrote: they work
    /// for the session as its subagents do, though none is counted here.
    pub workflow: Ms,
}

/// One message of a Preview.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
    /// Waiting on you with that card still open, but at work too: something
    /// that isn't your answer woke it. It's listed as asking you and drawn
    /// as working.
    pub at_work: bool,
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
    /// The worktree a rush session works in, and its branch. Boxed, as
    /// the pull request below: most rows have neither, and every row is
    /// kept several times over (the list, the loader's memo of it).
    pub workspace: Option<Box<Workspace>>,
    /// The pull request for its branch, as GitHub last said, else the
    /// first its agent linked; None when none is known.
    pub pr: Option<Box<PrStatus>>,
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
    /// The plugin that started a rush session, if one did, and why what
    /// it said it started it for (fleet.go:104).
    pub started_by: String,
    pub why: String,
    /// Another agent's transcript, read through its adapter.
    pub history: String,
    /// A session running on its agent's own servers.
    pub remote: bool,
    /// Where its agent compacts its context, when set smaller than the
    /// model's window.
    pub compaction: crate::adapter::Compaction,
    /// Emeritus (Rust-only, docs/emeritus.md): where a rush session
    /// stands in a chain of handovers, when it's in one. Boxed: most rows
    /// aren't.
    pub lineage: Option<Box<Lineage>>,
    /// What a rush session has done, as its host counted it (Rust-only):
    /// the list's WORK cell and the weight by its name. Empty for every
    /// other row.
    pub work: Work,
}

/// What a session has done, as its host counts it while it works
/// (Rust-only, nothing of Go's): its task list's tasks done and in all,
/// and how many files its edits changed. All 0 when nothing counted it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Work {
    pub done: u16,
    pub total: u16,
    pub files: u16,
}

impl Work {
    /// Neither a task list nor a file changed: its cell is blank.
    pub fn is_empty(&self) -> bool {
        self.total == 0 && self.files == 0
    }

    /// How much there is to it: the files changed and the tasks still
    /// open. What the list sorts by work on.
    pub fn score(&self) -> u32 {
        u32::from(self.files) + u32::from(self.total.saturating_sub(self.done))
    }

    /// How many of the weight's three cells are filled: none for nothing
    /// counted, one for a task list or up to 5 (score), two up to 20,
    /// three past that.
    pub fn weight(&self) -> usize {
        if self.is_empty() {
            return 0;
        }
        match self.score() {
            0..=5 => 1,
            6..=20 => 2,
            _ => 3,
        }
    }

    /// The WORK cell: "13/18 · 42f", the tasks and the files each in
    /// place so the column lines up ("  2/2 ·  1f", "13/18      ",
    /// "         7f"); "" when empty.
    pub fn cell(&self) -> String {
        let tasks = if self.total > 0 {
            format!("{}/{}", self.done, self.total)
        } else {
            String::new()
        };
        let files = if self.files > 0 {
            format!("{}f", self.files)
        } else {
            String::new()
        };
        match (tasks.is_empty(), files.is_empty()) {
            (true, true) => String::new(),
            (false, true) => format!("{tasks:>5}      "),
            (true, false) => format!("{:>5}   {files:>3}", ""),
            (false, false) => format!("{tasks:>5} · {files:>3}"),
        }
    }
}

impl Agent {
    /// The rush session that took over from this one as it retired
    /// (Emeritus); "" for one that hasn't.
    pub fn successor(&self) -> &str {
        self.lineage.as_deref().map_or("", |l| l.successor.as_str())
    }
}

/// Where a rush session that took over from another stands in its chain
/// of handovers (Emeritus, Rust-only).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Lineage {
    /// The session that took over from it as it retired, by rush id; ""
    /// while it hasn't retired.
    pub successor: String,
    /// The session it took over from, by rush id; "" for the first.
    pub predecessor: String,
    /// Its name, as the list had it; "" while it isn't known.
    pub name: String,
    /// How many handovers led to it: 0 for the first, 1 for its successor.
    pub handovers: u32,
    /// How many retired sessions of its chain the list folded into its
    /// row, their cost counted with its.
    pub folded: u32,
    /// Past its threshold with the cap on handovers in a row reached: it
    /// isn't asked to hand over again.
    pub capped: bool,
}

/// The worktree a rush session works in (Go's host.Workspace).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
    /// 1 for a run the session started, more for one another run started
    /// (its meta's spawnDepth): only the session waits on a run of 1.
    pub depth: i64,
    /// When its transcript was last written.
    pub mod_time: Ms,
    /// When it began.
    pub born: Ms,
    /// When it began to the nanosecond, when known (0 when not), for the
    /// order runs begun in the same millisecond go in.
    pub born_ns: i64,
    /// It's the run its row says asks you, and rush hosts it as session
    /// id: its ask is answered on that host (Agent::sub_asker). Rust-only.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub asking: bool,
    /// It, or an agent it started, waits on you: its row says so rather
    /// than spinning. Rust-only.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub waits: bool,
    /// How it ended, a run just done kept a moment so its row says so:
    /// "done", "failed" or "stopped"; "" while it works. Rust-only.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ended: String,
}

/// What a subagent is called wherever rush names one that asks you
/// something (its parent's row in the list, its card in the parent's
/// session, the notes under its box): its agent's name and what it was
/// asked, the first line of each, "Codex · fix the flaky test"; either
/// alone when the other isn't known, "a subagent" when neither is.
/// Rust-only.
pub fn sub_name(agent: &str, task: &str) -> String {
    let (a, t) = (crate::tool::one_line(agent), crate::tool::one_line(task));
    match (a.is_empty(), t.is_empty()) {
        (false, false) => format!("{a} · {t}"),
        (false, true) => a.to_string(),
        (true, false) => t.to_string(),
        (true, true) => "a subagent".into(),
    }
}

/// The rush session a hosted one is the subagent of, from its config's
/// Meta spawnedBy and startedFrom: None for one of its own. One started
/// beside another (startedFrom: start_agent, or `rush session start
/// --meta startedFrom=…` from a session's shell, which also says
/// spawnedBy) is a session of its own, with its own row, whatever else
/// it says. Rust-only.
pub fn subagent_of<'a>(spawned_by: Option<&'a str>, started_from: Option<&str>) -> Option<&'a str> {
    if started_from.is_some_and(|s| !s.is_empty()) {
        return None;
    }
    spawned_by.filter(|s| !s.is_empty())
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

impl Agent {
    /// A live run it started asks you something through it (subs.asking).
    pub fn sub_asks(&self) -> bool {
        self.subs.asking > 0
    }

    /// The rush session of the run its row says asks, where that ask is
    /// answered: its tile's id; "" for none, or one rush doesn't host.
    pub fn sub_asker(&self) -> &str {
        self.subagents
            .iter()
            .find(|t| t.asking)
            .map_or("", |t| t.id.as_str())
    }
}

fn is_zero_ms(v: &Ms) -> bool {
    *v == 0
}

#[cfg(test)]
mod sizes {
    use super::*;

    /// A row is kept once per row in the list and again in the loader's
    /// memo of it: what makes one bigger is a choice. (type, its size, the
    /// most it may be)
    #[test]
    fn rows_stay_small() {
        let cases = [
            // 1288: Emeritus's lineage, boxed (8 B); 1296: its Work, three
            // u16s (Rust-only).
            ("Agent", size_of::<Agent>(), 1296),
            ("Job", size_of::<Job>(), 384),
            ("Spend", size_of::<Spend>(), 304),
            (
                "Option<Box<PrStatus>>",
                size_of::<Option<Box<PrStatus>>>(),
                8,
            ),
        ];
        for (name, size, most) in cases {
            assert!(size <= most, "{name} is {size} B, more than {most}");
        }
    }
}

#[cfg(test)]
mod work {
    use super::Work;

    #[test]
    fn a_sessions_work_its_cell_and_its_weight() {
        // (done, total, files, the cell, the weight)
        let cases = [
            (0, 0, 0, "", 0),
            (2, 2, 1, "  2/2 ·  1f", 1),
            (13, 18, 42, "13/18 · 42f", 3),
            (0, 3, 0, "  0/3      ", 1),
            (3, 3, 0, "  3/3      ", 1),
            (0, 0, 7, "         7f", 2),
            (0, 0, 5, "         5f", 1),
            (1, 10, 12, " 1/10 · 12f", 3),
            (0, 0, 21, "        21f", 3),
        ];
        for (done, total, files, cell, weight) in cases {
            let w = Work { done, total, files };
            assert_eq!(w.cell(), cell, "{w:?}");
            assert_eq!(w.weight(), weight, "{w:?}");
            assert_eq!(w.is_empty(), weight == 0, "{w:?}");
        }
    }
}
