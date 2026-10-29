//! What adapters and rush hand each other, apart from any one agent. After
//! Go's internal/agent: adapter.go (Session, StartOptions, Input), agent.go
//! (PR, Convo, Command), actions.go (Move), choices.go, attribution.go,
//! auth.go, kinds.go (Once), query.go, media.go, memory.go (MemoryNote
//! :73, Dir and Reads :23-57, FrontMatter's Unquote :132), screen.go,
//! settings.go, settingspage.go, updater.go, rewind.go, plugins.go,
//! stats.go, subagents.go, timeline.go, summary.go, rows.go, spend.go.

use std::any::Any;
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{Error, Kind, Profile};
use crate::agent::{Job, PreviewLine, Task, Todo};
use crate::time::Ms;
use crate::wire::RawJson;

/// One conversation, running or past.
#[derive(Clone, Default)]
pub struct Session {
    pub kind: Kind,
    pub profile: Profile,
    pub id: String,
    pub name: String,
    /// working, blocked, idle, done, stopped
    pub state: String,
    /// What it's doing, in words.
    pub detail: String,
    /// What it's waiting on you for.
    pub needs: String,
    pub cwd: String,
    pub model: String,
    pub transcript: String,
    pub pid: i32,
    /// A one-shot run another program asked for (codex exec, claude -p): no
    /// one can reply to it, and one another agent's shell ran is listed
    /// with that agent.
    pub headless: bool,
    pub todos: Vec<Todo>,
    pub running: Vec<Task>,
    pub created_at: Ms,
    pub updated_at: Ms,
    /// A session running on the agent's own servers, not this machine: repo
    /// is the repository it works in, prs what it opened.
    pub remote: bool,
    pub repo: String,
    pub prs: Vec<Pr>,
    /// The adapter's own, for its own use: Claude's job file.
    pub extra: Option<Arc<dyn Any + Send + Sync>>,
    /// A background job's row, as its agent keeps it: set on the job, not
    /// on the session running it. job_id is the job a session runs, if it
    /// runs one.
    pub job: Option<Box<Job>>,
    pub job_id: String,
    /// A session opened in a terminal, not a job's.
    pub interactive: bool,
    /// What its process last said it was doing (busy, idle, shell), and
    /// status_at when; zero when it hasn't said.
    pub status: String,
    pub status_at: Ms,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("name", &self.name)
            .field("state", &self.state)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

/// Gets every line the agent writes, as it writes it, in its own words.
pub type Tap = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// How a Driver starts or resumes a session.
#[derive(Clone, Default)]
pub struct StartOptions {
    pub profile: Profile,
    /// When set, the login home (Homed::home) a session last ran in: it
    /// starts there again rather than in the one in use, where it still
    /// can, to keep its warm prompt cache.
    pub home: String,
    pub dir: String,
    /// Resumed when resume is set, else the new session's id if the agent
    /// takes one.
    pub session_id: String,
    pub resume: bool,
    pub fork: bool,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub env: Vec<String>,
    /// Passed to the agent as they are.
    pub flags: Vec<String>,
    /// The agent's program, when it isn't on PATH by its usual name.
    pub binary: String,
    /// What the agent goes without, in its own words: for Claude Code, tool
    /// rules taken out of its context.
    pub without: Vec<String>,
    /// A command to run after each tool call, the call's hook input on its
    /// stdin: what it prints is a message for the (sub)agent that made the
    /// call. An agent that can take one says so (takes_subagent_messages).
    pub inbox: String,
    /// A command to run before each shell call, the call's hook input on
    /// its stdin; what it prints may rewrite the call. For the gate: agents
    /// without hooks go without.
    pub bash_hook: String,
    /// When set, pays for the session per token with the provider's API key
    /// rather than out of the subscription signed in.
    pub api_key: String,
    /// The session's own scratch folder, for the agent's temporary files.
    pub temp_dir: String,
    /// Starts it without its non-essential network traffic, where it can.
    pub lean: bool,
    /// How many subagents it may run at once, where its harness caps them;
    /// 0 leaves the harness's own.
    pub subagents: i64,
    /// MCP servers rush serves in process for the session.
    pub tools: Vec<ToolServer>,
    /// Subagents to offer it, by name, each defined in its agent's own
    /// words; prompt is added to its system prompt.
    pub agents: BTreeMap<String, RawJson>,
    pub prompt: String,
    /// For a new session, another agent's conversation for it to take as
    /// its own history, where it can (FeaturePort).
    pub carry: Vec<PreviewLine>,
    /// Folders of <name>/SKILL.md skills Claude Code has, for an agent that
    /// isn't Claude Code to read as well.
    pub skill_roots: Vec<String>,
    /// When set, gets every line the agent writes, as it writes it, in its
    /// own words: what rushes that don't read events are sent.
    pub tap: Option<Tap>,
    /// Lets Events leave out what's only streamed text and tool results:
    /// the caller has them from tap.
    pub lightly: bool,
}

/// Answers one JSON-RPC message.
pub type Handle = Arc<dyn Fn(&RawValue) -> RawJson + Send + Sync>;

/// An MCP server rush serves in process for a session: its own drawing
/// tools, or an approved plugin's.
#[derive(Clone, Default)]
pub struct ToolServer {
    pub name: String,
    /// Its tools that never ask, by their own name: rush's own only draw.
    pub trusted: Vec<String>,
    /// Answers one JSON-RPC message. It may take a while; the session
    /// carries on meanwhile.
    pub handle: Option<Handle>,
    /// Run it as a process instead, for an agent that only runs its MCP
    /// servers so: rush's own executable with these.
    pub args: Vec<String>,
}

/// A message to a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    /// Paths.
    pub images: Vec<String>,
}

/// A pull request a session opened or pushed to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pr {
    #[serde(skip)]
    pub url: String,
    pub title: String,
    pub number: i64,
    pub state: String,
    pub review: String,
    pub checks: Checks,
}

/// A pull request's checks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checks {
    pub passed: i64,
    pub failed: i64,
    pub pending: i64,
}

/// A session's subagents: one type for the list and the adapters.
pub use crate::agent::SubagentStats;

/// A past conversation, known from its transcript.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Convo {
    pub session_id: String,
    pub cwd: String,
    /// Its rename, or else the title the agent gave it, or else its first
    /// prompt.
    pub title: String,
    pub started: Ms,
}

/// A slash command or skill a session can run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub skill: bool,
    /// Its SKILL.md or command file.
    pub path: String,
    /// yours, project, the agent's cloud, or the plugin's name
    pub source: String,
    /// Of the file: roughly what using it adds.
    pub size: i64,
}

/// What a new session can start with. Each list is offered as it is, after
/// the agent's own default, which is the empty ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
    pub modes: Vec<Choice>,
}

/// One value and what it means: {"sonnet", "faster and cheaper, good for
/// routine work"}.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub note: String,
    /// A model's context window in tokens, when its lister knows it; 0 when
    /// not.
    pub context: i64,
}

/// Where a session goes.
#[derive(Clone, Default)]
pub struct Move {
    pub from: Profile,
    pub to: Profile,
    /// The session as listed.
    pub job: Job,
    /// The adapter's own record of it, from its listing.
    pub extra: Option<Arc<dyn Any + Send + Sync>>,
    /// Its new folder; empty keeps the old one.
    pub dir: String,
    /// More folders to give it.
    pub add_dirs: Vec<String>,
    /// The first message once it's moved.
    pub note: String,
}

/// One profile apply_attribution changed, or failed to.
#[derive(Debug)]
pub struct AttributionChange {
    pub kind: Kind,
    pub profile: String,
    pub err: Option<Error>,
}

impl std::fmt::Display for AttributionChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.err {
            Some(e) => write!(f, "{} ({}): {}", self.kind, self.profile, e),
            None => write!(f, "{} ({})", self.kind, self.profile),
        }
    }
}

/// Safe account information for display. It never holds credentials.
/// Unknown identity and plan fields stay empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountSummary {
    pub signed_in: bool,
    /// Explicit evidence: credentials available, unknown, or another native
    /// state.
    pub status: String,
    pub method: String,
    pub name: String,
    pub email: String,
    pub org: String,
    pub plan: String,
    pub endpoint: String,
    pub note: String,
}

/// A one-shot run a shell asked of an agent's program: one task, answered
/// and printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Once {
    pub prompt: String,
    pub model: String,
    pub mode: String,
    pub cwd: String,
}

/// One question for a Querier.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub model: String,
    pub system: String,
    /// Given on its stdin.
    pub prompt: String,
    /// The JSON schema its answer must fit.
    pub schema: String,
    /// The folders it may read, and nothing else.
    pub dirs: Vec<String>,
    /// The most it may spend, in dollars.
    pub budget: f64,
}

/// What a Querier gave back.
#[derive(Debug, Default)]
pub struct Answer {
    /// The answer, in Query.schema's shape.
    pub out: Vec<u8>,
    /// What it cost, in dollars.
    pub cost: f64,
    /// Why it failed, when it did.
    pub err: Option<Error>,
}

/// What a model reads besides text, a bit for each kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Media(pub u8);

impl Media {
    pub const IMAGE: Media = Media(1);
    pub const PDF: Media = Media(2);
    pub const AUDIO: Media = Media(4);
    pub const VIDEO: Media = Media(8);

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn has(self, o: Media) -> bool {
        self.0 & o.0 != 0
    }
    /// What self needs that has doesn't read.
    pub fn without(self, has: Media) -> Media {
        Media(self.0 & !has.0)
    }

    /// Its first kind as a message says it: "images".
    pub fn as_str(self) -> &'static str {
        if self.has(Media::IMAGE) {
            "images"
        } else if self.has(Media::PDF) {
            "PDFs"
        } else if self.has(Media::AUDIO) {
            "audio"
        } else if self.has(Media::VIDEO) {
            "video"
        } else {
            "text"
        }
    }
}

impl std::ops::BitOr for Media {
    type Output = Media;
    fn bitor(self, o: Media) -> Media {
        Media(self.0 | o.0)
    }
}

impl std::ops::BitOrAssign for Media {
    fn bitor_assign(&mut self, o: Media) {
        self.0 |= o.0;
    }
}

impl std::fmt::Display for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of file path is by its name; empty for anything else.
pub fn media_of(path: &str) -> Media {
    let base = path.rsplit('/').next().unwrap_or(path);
    let Some(dot) = base.rfind('.') else {
        return Media::default();
    };
    match base[dot..].to_lowercase().as_str() {
        ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp" => Media::IMAGE,
        ".pdf" => Media::PDF,
        ".mp3" | ".wav" | ".m4a" | ".ogg" | ".flac" => Media::AUDIO,
        ".mp4" | ".mov" | ".webm" | ".mkv" | ".avi" => Media::VIDEO,
        _ => Media::default(),
    }
}

/// What an agent reads for a session, beyond the conversation: its
/// instruction files (CLAUDE.md, AGENTS.md), its own memory, settings and
/// the like, and what's untidy about them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// Grouped, in groups' order.
    pub files: Vec<MemoryFile>,
    pub groups: Vec<String>,
    /// When each group's files reach the model.
    pub when: BTreeMap<String, String>,
    pub problems: Vec<MemoryProblem>,
    /// The folder its own memory notes are kept in, "" when it keeps none
    /// (memory.go:23).
    pub dir: String,
}

/// One file an agent reads, or one it would read if it existed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryFile {
    pub group: String,
    pub path: String,
    pub name: String,
    pub about: String,
    /// A memory note's type: user, feedback, project, reference.
    pub kind: String,
    pub missing: bool,
    pub size: i64,
    pub mod_time: Ms,
    /// The tokens it costs every session, roughly; later, the tokens it
    /// costs when it's loaded, which when says.
    pub up: i64,
    pub later: i64,
    pub when: String,
    /// What's untidy about it.
    pub warn: String,
    /// Imported by the file above it.
    pub sub: bool,
    /// How many times sessions opened a memory note lately, and last_read
    /// when last; counted, whether they were counted at all. MEMORY.md's
    /// line for it reaches every session either way (memory.go:44).
    pub reads: i64,
    pub last_read: Ms,
    pub counted: bool,
    /// Why it's worth a look, as rush judges it when it reads it.
    pub review: String,
    /// The note it repeats, to merge it into.
    pub same: String,
}

/// A memory note to write (memory.go:73).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryNote {
    /// short-kebab-case, its file's name too
    pub name: String,
    /// Its line in the index.
    pub title: String,
    /// What recall picks it by.
    pub description: String,
    /// user, feedback, project or reference
    pub kind: String,
    pub body: String,
}

/// Something untidy, and what to do about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryProblem {
    pub kind: String,
    pub title: String,
    pub fix: String,
    /// The file to open for it.
    pub path: String,
    /// The files it's about.
    pub files: Vec<String>,
}

/// Reads name, description and type from a markdown note's frontmatter, at
/// any depth (a memory note keeps its type under metadata), and "paths"
/// when a rule has a paths: list.
pub fn front_matter(mut r: impl BufRead) -> BTreeMap<String, String> {
    // As Go's bufio.Scanner with its 1 MiB buffer: a longer line ends the
    // read, so a file with no newlines is never read whole.
    const MAX_LINE: u64 = 1 << 20;
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut block = String::new(); // the key whose value is a block (| or >) on the lines below
    let mut buf = Vec::new();
    for n in 0..60 {
        buf.clear();
        match (&mut r).take(MAX_LINE + 1).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        } else if buf.len() as u64 > MAX_LINE {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        if !block.is_empty() {
            let t = line.trim();
            if !t.is_empty() && (line.starts_with(' ') || line.starts_with('\t')) {
                let v = out.entry(block.clone()).or_default();
                *v = format!("{v} {t}").trim().to_string();
                continue;
            }
            block.clear();
        }
        if line.trim() == "---" {
            if n == 0 {
                continue;
            }
            break;
        }
        if n == 0 {
            return out; // no frontmatter
        }
        let Some((k, v)) = line.trim().split_once(':') else {
            continue;
        };
        match k.trim() {
            k @ ("name" | "description" | "type") => {
                if out.get(k).is_some_and(|s| !s.is_empty()) {
                    continue;
                }
                match v.trim() {
                    "|" | ">" | "|-" | ">-" | "|+" | ">+" => block = k.to_string(),
                    v => {
                        // A quoted value is read as Go's strconv.Unquote
                        // reads it, escapes and all (memory.go:132).
                        let v = match v.starts_with('"') && v.len() > 1 {
                            true => serde_json::from_str::<String>(v).ok(),
                            false => None,
                        }
                        .unwrap_or_else(|| v.trim_matches(['"', '\'']).to_string());
                        out.insert(k.to_string(), v);
                    }
                }
            }
            "paths" | "globs" => {
                out.insert("paths".into(), "yes".into());
            }
            _ => {}
        }
    }
    out
}

/// Writes what a mirrored screen draws to w, until it detaches.
pub type Relay = Box<dyn FnOnce(&mut dyn Write) -> std::io::Result<()> + Send>;

/// A session's screen attached for a pane to draw.
pub struct Mirror {
    /// The private modes its screen has on.
    pub modes: Vec<i32>,
    /// Takes the answers to its terminal's queries; dropping it detaches.
    pub conn: Box<dyn Write + Send>,
    /// Writes what the screen draws to w, until it detaches.
    pub relay: Relay,
}

/// One of the settings files an agent's session reads, and what it is to
/// you: "yours", "project".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsFile {
    pub label: String,
    pub path: String,
}

/// What an agent offers on its page in rush's Settings: keys of the first
/// of its SettingsFiles, each a row rush edits in place, and the
/// environment its sessions start with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsPage {
    /// Under the settings file's section, after its path.
    pub note: String,
    /// What a key not set means: "Claude Code's own default".
    pub unset: String,
    pub rows: Vec<SettingRow>,
    /// The variables offered as choices; anything else in env_key shows as
    /// a row of its own. Empty env_key: the agent has no env block.
    pub env_key: String,
    pub env: Vec<EnvSetting>,
}

/// One key of a settings file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingRow {
    pub label: String,
    pub key: String,
    pub what: String,
    pub kind: SettingType,
    /// Besides unset.
    pub choices: Vec<String>,
}

/// How a row's value is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingType {
    #[default]
    String,
    /// true is "on".
    Flag,
    /// true is "off": a key that turns a thing off.
    Off,
    Int,
}

/// A variable offered with choices, and what they mean.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvSetting {
    pub label: String,
    pub name: String,
    pub what: String,
    /// Besides unset.
    pub choices: Vec<String>,
    pub means: BTreeMap<String, String>,
}

/// A definition an agent's sessions can start as.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentDef {
    pub name: String,
    pub scope: String,
    pub model: String,
    pub desc: String,
    /// Its file; "" is the agent's built-in one.
    pub path: String,
}

/// Where an agent's program is published, so its newest version can be
/// looked up: the npm or PyPI package it is (either may be empty), and its
/// own update command, run as the program with these arguments, when it
/// has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Published {
    pub npm: String,
    pub pypi: String,
    pub update: Vec<String>,
}

/// What putting a conversation's files back did, or would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRewind {
    pub can_rewind: bool,
    /// Why it can't.
    pub error: String,
    pub files: Vec<String>,
    pub insertions: i64,
    pub deletions: i64,
}

/// An installed plugin, or one a marketplace offers. Its JSON is Claude
/// Code's plugin list's, the first agent that had them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plugin {
    /// name@marketplace
    pub id: String,
    pub name: String,
    #[serde(rename = "marketplaceName")]
    pub marketplace: String,
    pub description: String,
    pub version: String,
    #[serde(rename = "installCount")]
    pub installs: i64,

    // Installed ones only.
    #[serde(skip)]
    pub installed: bool,
    pub enabled: bool,
    /// user, project, local
    pub scope: String,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "installPath")]
    pub install_path: String,
    #[serde(rename = "lastUpdated")]
    pub last_updated: String,

    #[serde(skip)]
    pub parts: PluginParts,
}

/// What a plugin brings, read from its folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginParts {
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    pub commands: Vec<String>,
    pub hooks: Vec<String>,
    pub mcp: Vec<String>,
}

/// A catalogue plugins are installed from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
    pub repo: String,
    pub url: String,
    #[serde(rename = "installLocation")]
    pub location: String,
}

impl Marketplace {
    /// How a marketplace is found: its GitHub repo, URL or folder.
    pub fn place(&self) -> &str {
        if !self.repo.is_empty() {
            &self.repo
        } else if !self.url.is_empty() {
            &self.url
        } else {
            &self.location
        }
    }
}

/// A change to an agent's plugins or marketplaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginOp {
    #[default]
    Enable,
    Disable,
    Update,
    Remove,
    Install,
    /// ID is where it is: a repo, URL or folder.
    MarketAdd,
    /// An empty ID updates every one.
    MarketUpdate,
    MarketRemove,
}

/// One change: to the plugin or marketplace id, in scope (user, project,
/// local) where that matters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginChange {
    pub op: PluginOp,
    pub id: String,
    pub scope: String,
}

/// An agent's own record of a profile's use over time: activity and tokens
/// by day, and totals by model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// The last day it counted, 2006-01-02.
    pub computed: String,
    pub first: Ms,
    pub sessions: i64,
    pub messages: i64,
    /// Oldest first.
    pub days: Vec<StatsDay>,
    pub models: Vec<ModelTotal>,
    /// Sessions started in each hour of the day.
    pub hours: [i64; 24],
    pub longest: Duration,
    pub longest_msgs: i64,
    pub longest_id: String,
    pub longest_at: Ms,
}

/// One day's activity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatsDay {
    pub date: Ms,
    pub messages: i64,
    pub sessions: i64,
    pub tool_calls: i64,
    /// By model.
    pub tokens: BTreeMap<String, i64>,
}

impl StatsDay {
    /// The day's tokens across models.
    pub fn total_tokens(&self) -> i64 {
        self.tokens.values().sum()
    }
}

/// One model's tokens, all time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTotal {
    pub model: String,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost_usd: f64,
}

impl ModelTotal {
    /// Every token the model read or wrote.
    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// How a session's transcripts say a subagent run stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunState {
    /// No word of its call: judge by its writing.
    #[default]
    Unknown,
    Running,
    Done,
}

/// One subagent run a session started, as its transcripts say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubagentRun {
    pub id: String,
    pub tool_use_id: String,
    /// 1 for one the session started, more for one a run started.
    pub depth: i64,
    /// When its transcript was last written.
    pub mod_time: Ms,
    /// When it began, when that's known.
    pub born: Ms,
    /// The run's own words: which subagent it is, and what it was asked.
    pub kind: String,
    pub description: String,
    /// Its own transcript.
    pub path: String,
}

/// How many subagents a session may run at once, by the machine: they wait
/// on the API, and rush's gate queues their heavy tools.
pub fn subagent_cap() -> i64 {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as i64;
    20.max(cpus * 4)
}

/// What a timeline entry is (Go's EventKind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EventKind {
    /// You wrote to it.
    #[default]
    Prompt,
    /// A turn ended, with what it said.
    Turn,
    /// Tasks planned; n of them.
    Plan,
    /// A task ticked off.
    Tick,
    /// A subagent started.
    Start,
    /// A subagent, or a background command, ended: status says how.
    End,
    /// It asked you something.
    Ask,
    /// A commit.
    Commit,
    /// A PR opened.
    Pr,
    /// A turn died on an API error.
    Error,
    /// Its context was compacted.
    Compact,
}

/// One thing on a session's timeline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Happening {
    pub at: Ms,
    pub kind: EventKind,
    /// The subagent it's from, by agent id; "" is the session.
    pub run: String,
    /// For End: completed, failed or stopped.
    pub status: String,
    pub text: String,
    /// For Plan.
    pub n: i64,
}

/// text cut to about n tokens: its start and, most of it, its end, where
/// the work in hand is.
pub fn fit_tokens(text: &str, n: usize) -> String {
    const PER_TOKEN: usize = 3; // under-counting chars per token keeps it inside
    if n == 0 || text.len() <= n * PER_TOKEN {
        return text.to_string();
    }
    let floor = |mut i: usize| {
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let head = floor(n * PER_TOKEN / 5);
    let tail = floor(text.len() - n * PER_TOKEN * 4 / 5);
    format!(
        "{}\n\n[… {} characters of the middle left out …]\n\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

/// A folder of an agent's scratch work, and whether cleaning it empties it
/// (a folder the agent expects to find) or removes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TempDir {
    pub path: String,
    pub keep: bool,
}

/// One session's transcript, to price.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpendTarget {
    pub key: String,
    pub path: String,
    /// An agent that may be writing: its subagents' transcripts are checked
    /// every run. Others are only looked at when their own transcript or
    /// subagents folder changes.
    pub live: bool,
    /// A conversation nothing has open: its row is read again only every so
    /// often, and so are its files.
    pub past: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_tokens_keeps_the_start_and_the_end() {
        assert_eq!(fit_tokens("short", 100), "short");
        let long = format!("START{}END", "x".repeat(10_000));
        let got = fit_tokens(&long, 1000);
        assert!(got.len() <= 3200 && got.starts_with("START") && got.ends_with("END"));
        assert!(got.contains("left out"));
    }

    #[test]
    fn media_by_extension() {
        for (path, want) in [
            ("a.PNG", Media::IMAGE),
            ("/x/y.pdf", Media::PDF),
            ("s.flac", Media::AUDIO),
            ("v.webm", Media::VIDEO),
            ("notes.txt", Media::default()),
            ("dir.png/file", Media::default()),
        ] {
            assert_eq!(media_of(path), want, "{path}");
        }
        assert_eq!((Media::PDF | Media::VIDEO).to_string(), "PDFs");
        assert_eq!(Media::default().to_string(), "text");
    }

    #[test]
    fn front_matter_reads_its_keys() {
        let note = "---\nname: \"tidy\"\nmetadata:\n  type: feedback\ndescription: >\n  a long\n  one\npaths:\n  - a\n---\nbody: no\n";
        let got = front_matter(note.as_bytes());
        let want: BTreeMap<String, String> = [
            ("name", "tidy"),
            ("type", "feedback"),
            ("description", "a long one"),
            ("paths", "yes"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(got, want);
        assert!(front_matter("no frontmatter\nname: x".as_bytes()).is_empty());
        // CRLF lines, and a line too long to read, which ends it.
        let crlf = front_matter("---\r\nname: a\r\n---\r\n".as_bytes());
        assert_eq!(crlf.get("name").map(String::as_str), Some("a"));
        let long = format!("---\nname: b\n{}", "x".repeat(2 << 20));
        let got = front_matter(long.as_bytes());
        assert_eq!(got.get("name").map(String::as_str), Some("b"));
        // A quoted value is read with its escapes, as rush writes one.
        let q = front_matter("---\ndescription: \"? yes: \\\"10\\\"\"\n---\n".as_bytes());
        assert_eq!(
            q.get("description").map(String::as_str),
            Some("? yes: \"10\"")
        );
    }
}
