//! What a host keeps on disk and says about a session: its config.json
//! (Config), its info.json (Info) and the parts of them. Each reads and
//! writes byte for byte as Go rush's does (photon's jsonx: encoding/json/v2,
//! where omitempty leaves out only what encodes empty, so a false bool is
//! still written and omitzero leaves out zeros).
//!
//! Ported from internal/host/host.go:47-345 (Config, Branch, Info, Task,
//! Proto, Limit, Retry, Duration), config.go (the account's shape on disk),
//! stale.go:13-41 (BinStamp, Stale, Quiet, RestartDue), away.go:28-54
//! (Away, Held), left.go:17-21 (Left), workspace.go:12-28 (Workspace),
//! revive.go:23 (midTurn) and exchange.go (queueExchanges, trimExchanges).

use std::collections::BTreeMap;
use std::time::Duration;

use rush_core::adapter::{Profile, migrated};
use rush_core::agent::PreviewLine;
use rush_core::event::{Exchange, PermissionMode, Question};
use rush_core::usage::Change as UsageChange;
use rush_core::wire::{float, nullvec};
use rush_state::Time;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// This build's host protocol: 1 adds rewind, 2 context usage and control
/// requests passed through (the ask op), 3 moving a running tool to the
/// background (the background op) and Info.background, 4 other agents'
/// sessions as rush's own events, 5 the client's hello (hello.rs), 6
/// Claude Code's sessions as rush's own events too, to a client that says
/// it reads them, 7 steering with a queued message (queue_send's guide),
/// 8 exchanges, 9 guarded compaction, 10 idle host sleep, 11 away. A client
/// sends its build's in the hello.
pub const PROTO: i64 = 11;

/// How many left paths an agent remembers.
pub const MAX_BRANCHES: usize = 30;

pub(crate) fn zero<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

pub(crate) fn zero_time(t: &Time) -> bool {
    t.is_zero()
}

/// How a session is started. It is written next to the socket so the host
/// process reads it on launch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The ev of what started the host, for its events: spawn hands it over
    /// as RUSH_CAUSE, and it's never saved. "" is the starting process's
    /// own cause.
    #[serde(skip)]
    pub cause: String,
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// The conversation already exists.
    pub resume: bool,
    /// Continues a copy of the conversation (Claude Code's
    /// --fork-session), leaving the original to whoever has it open. Once
    /// the agent names the copy, session_id becomes that and fork is cleared.
    #[serde(skip_serializing_if = "zero")]
    pub fork: bool,
    /// How the session is paid for: "key" with its provider's API key, per
    /// token; "" as its agent is signed in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub billing: String,
    /// What the session's agent goes without, in its own words.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub without: Vec<String>,
    /// The conversation this one continues, for showing its history.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(rename = "permissionMode", skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub flags: Vec<String>,
    #[serde(rename = "promptExchange", skip_serializing_if = "Option::is_none")]
    pub prompt_exchange: Option<Box<Exchange>>,
    /// The first message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// Names the session from the first message sent to it, for one
    /// started without one (by /clear, say).
    #[serde(rename = "nameFirst", skip_serializing_if = "zero")]
    pub name_first: bool,
    /// Files attached to the first message.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub images: Vec<String>,
    /// What happens when a usage limit stops the session: "auto" continues
    /// at the reset, "off" waits for you, and "" (opt-in) asks once.
    #[serde(rename = "limitMode", skip_serializing_if = "String::is_empty")]
    pub limit_mode: String,
    /// The first wait before retrying an API error; each retry doubles
    /// it. retry_max caps the attempts. Zero means the defaults.
    #[serde(rename = "retryBase", skip_serializing_if = "zero", with = "godur")]
    pub retry_base: Duration,
    #[serde(rename = "retryMax", skip_serializing_if = "zero")]
    pub retry_max: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub binary: String,
    /// Starts the agent without its non-essential network traffic.
    #[serde(skip_serializing_if = "zero")]
    pub lean: bool,
    /// The paths /rewind left behind, newest last.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub branches: Vec<Branch>,
    /// The plugin that started the session, if one did.
    #[serde(rename = "startedBy", skip_serializing_if = "String::is_empty")]
    pub started_by: String,
    /// What whoever started it tagged it with.
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "nullmap"
    )]
    pub meta: BTreeMap<String, String>,
    /// Added to the agent's environment (KEY=value), on every start.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub env: Vec<String>,
    /// The agent the session runs, through its adapter. Always written; one
    /// read without it is Claude Code's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The profile the session was started under.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile: String,
    /// A run another program reads (rush spawn): no one watches it in rush.
    #[serde(skip_serializing_if = "zero")]
    pub headless: bool,
    /// Added to the agent's system prompt, after rush's own.
    #[serde(rename = "systemPrompt", skip_serializing_if = "String::is_empty")]
    pub system_prompt: String,
    /// Another agent's conversation, for the agent to take as its own
    /// history when it first starts. None when there's none to carry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carry: Option<Vec<CarryLine>>,
    /// The process (rush spawn) the session ends with, when it's the one
    /// that started the host.
    #[serde(skip_serializing_if = "zero")]
    pub owner: i64,
    /// Asks for the session's own git worktree, made before its agent first
    /// starts. Cleared once asked, made or not.
    #[serde(rename = "ownWorktree", skip_serializing_if = "zero")]
    pub own_worktree: bool,
    /// The worktree the session works in, and its branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// The agent's profile: on disk as {"name", "configDir"}, last, as Go's
    /// wireConfig has it (config.go).
    #[serde(with = "wire_account")]
    pub account: Profile,
}

impl Config {
    /// A config as config.json has it, brought up to date: an older
    /// rush's names no kind for Claude Code, and the account's kind is the
    /// session's.
    pub fn parse(b: &[u8]) -> serde_json::Result<Config> {
        let mut c: Config = serde_json::from_slice(b)?;
        c.kind = migrated(&c.kind).0; // migration: an older rush's config names no kind for Claude Code
        c.account.kind = migrated(&c.kind);
        Ok(c)
    }

    /// The config as Go's MarshalIndent writes it.
    pub fn to_json_indent(&self) -> Vec<u8> {
        crate::json::indent(self)
    }

    /// Points the config at session_id, keeping the conversation it leaves
    /// as a branch (when it has one, began) described by left (host.go:2250).
    pub fn rewind_to(
        &mut self,
        session_id: &str,
        resume: bool,
        left: Option<&Branch>,
        began: bool,
    ) {
        let cur = self.session_id.clone();
        self.branches
            .retain(|b| b.session_id != session_id && b.session_id != cur);
        if let Some(left) = left
            && began
            && !cur.is_empty()
            && !self.fork
        {
            let mut b = left.clone();
            b.session_id = cur;
            b.left = Time::now();
            self.branches.push(b);
            let n = self.branches.len();
            if n > MAX_BRANCHES {
                self.branches.drain(..n - MAX_BRANCHES);
            }
        }
        self.session_id = session_id.to_string();
        self.resume = resume;
        self.fork = false;
        self.from.clear();
        self.prompt.clear();
        self.images.clear();
        self.prompt_exchange = None;
        if !resume {
            // A conversation started afresh is named by its first message.
            self.name = crate::name::fresh_name(&self.cwd);
            self.name_first = true;
        }
    }
}

mod wire_account {
    use super::*;

    #[derive(Serialize, Deserialize, Default)]
    #[serde(default)]
    struct Wire {
        name: String,
        #[serde(rename = "configDir")]
        config_dir: String,
    }

    pub fn serialize<S: Serializer>(p: &Profile, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            name: p.name.clone(),
            config_dir: p.dir.clone(),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Profile, D::Error> {
        let w = Option::<Wire>::deserialize(d)?.unwrap_or_default();
        Ok(Profile {
            name: w.name,
            dir: w.config_dir,
            ..Profile::default()
        })
    }
}

fn nullmap<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    Ok(Option::<BTreeMap<String, String>>::deserialize(d)?.unwrap_or_default())
}

/// A time.Duration written as Go's String() ("15s", "1m30s"): host.go's
/// Duration.
pub mod godur {
    use super::*;
    use serde::de::Error as _;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(*d))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let s = Option::<String>::deserialize(d)?.unwrap_or_default();
        if s.is_empty() {
            return Ok(Duration::ZERO);
        }
        parse(&s).ok_or_else(|| D::Error::custom(format!("time: invalid duration \"{s}\"")))
    }

    /// d as Go's time.Duration.String writes it.
    pub fn format(d: Duration) -> String {
        rush_gate::godur::string(d)
    }

    /// A Go duration string ("1h2m3.5s", "300ms"); None when it isn't one.
    /// Negative durations read as zero.
    pub fn parse(s: &str) -> Option<Duration> {
        let mut rest = s;
        let neg = rest.starts_with('-');
        rest = rest.trim_start_matches(['-', '+']);
        if rest == "0" {
            return Some(Duration::ZERO);
        }
        if rest.is_empty() {
            return None;
        }
        let mut total = 0f64;
        while !rest.is_empty() {
            let n = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(rest.len());
            if n == 0 {
                return None;
            }
            let v: f64 = rest[..n].parse().ok()?;
            rest = &rest[n..];
            let u = rest
                .find(|c: char| c.is_ascii_digit() || c == '.')
                .unwrap_or(rest.len());
            let unit = match &rest[..u] {
                "ns" => 1.0,
                "us" | "µs" | "μs" => 1e3,
                "ms" => 1e6,
                "s" => 1e9,
                "m" => 60e9,
                "h" => 3600e9,
                _ => return None,
            };
            rest = &rest[u..];
            total += v * unit;
        }
        if neg {
            return Some(Duration::ZERO);
        }
        Some(Duration::from_nanos(total.round() as u64))
    }
}

/// One message of another agent's conversation carried into a new one
/// (Go's agent.Line), as config.json keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CarryLine {
    pub role: String,
    pub text: String,
    pub at: Time,
    pub doing: String,
}

impl From<&CarryLine> for PreviewLine {
    fn from(l: &CarryLine) -> PreviewLine {
        PreviewLine {
            role: l.role.clone(),
            text: l.text.clone(),
            at: l.at.to_ms(),
            doing: l.doing.clone(),
        }
    }
}

/// A path of the conversation that /rewind left: its own conversation,
/// sharing the turns before from with the one it left for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Branch {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub left: Time,
    /// The first turn of its own.
    #[serde(skip_serializing_if = "zero")]
    pub from: i64,
    /// Your messages on it.
    #[serde(skip_serializing_if = "zero")]
    pub turns: i64,
    /// The last of them.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last: String,
}

/// The git worktree a session works in, and its branch: one session, one
/// worktree, one branch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// The worktree's top.
    pub path: String,
    /// The main checkout it belongs to.
    pub repo: String,
    /// "" when it's detached.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    /// The commit the worktree started from, in full.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base: String,
    /// The branch the main checkout had checked out when the session
    /// started: where its work is meant to land.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// A worktree rush made for the session before its agent started.
    #[serde(skip_serializing_if = "zero")]
    pub own: bool,
}

/// What the list shows about a session; the host keeps it in info.json and
/// sends it to clients whenever it changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub account: String,
    pub cwd: String,
    /// When the host last moved it.
    #[serde(rename = "cwdAt", skip_serializing_if = "zero_time")]
    pub cwd_at: Time,
    /// Its scratch folder.
    #[serde(rename = "temp_dir", skip_serializing_if = "String::is_empty")]
    pub temp_dir: String,
    /// What it wrote outside its project and scratch.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub left: Vec<Left>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(rename = "hostPid")]
    pub host_pid: i64,
    #[serde(rename = "claudePid", skip_serializing_if = "zero")]
    pub claude_pid: i64,
    /// starting, working, blocked, idle, stopped
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub needs: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub without: Vec<String>,
    /// Whether its running subagents can be sent messages straight.
    #[serde(skip_serializing_if = "zero")]
    pub inbox: bool,
    #[serde(rename = "permissionMode", skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
    #[serde(
        rename = "permissionModes",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub permission_modes: Vec<PermissionMode>,
    #[serde(rename = "costUsd", skip_serializing_if = "zero", with = "float")]
    pub cost_usd: f64,
    /// plan, overage or metered; empty until the agent says.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub billing: String,
    /// Messages sent while the agent was busy; the host sends the first
    /// when the turn ends.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub queue: Vec<String>,
    /// The images attached to each queued message, by its place in queue;
    /// empty when none has any.
    #[serde(
        rename = "queueImages",
        skip_serializing_if = "Vec::is_empty",
        with = "images_list"
    )]
    pub queue_images: Vec<Vec<String>>,
    #[serde(
        rename = "queueExchanges",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub queue_exchanges: Vec<Option<Box<Exchange>>>,
    #[serde(rename = "queueHeld", skip_serializing_if = "zero")]
    pub queue_held: bool,
    #[serde(rename = "queueSeparate", skip_serializing_if = "zero")]
    pub queue_separate: bool,
    /// When the queue was found still waiting behind a turn its agent
    /// can't take a message in (Kimi and the other ACP agents): it waits on
    /// you to stop the turn and send it. Zero otherwise.
    #[serde(rename = "queueStuck", skip_serializing_if = "zero_time")]
    pub queue_stuck: Time,
    /// Set while a usage limit has stopped the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
    /// Set while an API error is being retried, or has given up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// When the prompt cache written by the last request expires.
    #[serde(rename = "cacheWarm")]
    pub cache_warm: Time,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(rename = "startedAt")]
    pub started_at: Time,
    #[serde(rename = "updatedAt")]
    pub updated_at: Time,
    /// When it last went idle: a turn's end. Zero until its first turn.
    #[serde(rename = "idleSince", skip_serializing_if = "zero_time")]
    pub idle_since: Time,
    /// When it last heard from its agent, set once a turn has heard
    /// nothing for a while with no tool running; zero otherwise.
    #[serde(skip_serializing_if = "zero_time")]
    pub stalled: Time,
    /// When /rewind last switched it to an earlier point.
    #[serde(rename = "rewoundAt")]
    pub rewound_at: Time,
    /// Set once the replay no longer reaches back to the start.
    #[serde(rename = "replayFrom")]
    pub replay_from: Time,
    /// What the host can do: see PROTO.
    #[serde(skip_serializing_if = "zero")]
    pub proto: i64,
    /// The hosts the session has had.
    #[serde(rename = "gen", skip_serializing_if = "zero")]
    pub generation: i64,
    /// The turns delivered to the session, all its hosts told.
    #[serde(skip_serializing_if = "zero")]
    pub turn: i64,
    /// A clean idle exit; explicit input wakes the saved session.
    pub sleeping: bool,
    /// The machine a session is really on, set only on rush remote
    /// attach's stand-in for it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    #[serde(rename = "remoteId", skip_serializing_if = "String::is_empty")]
    pub remote_id: String,
    /// Set on reading when the host went away without saying so (killed,
    /// or taken down with the app that ran it) mid-turn, or with background
    /// agents at work: see revive.rs.
    #[serde(skip)]
    pub lost: bool,
    /// On a reading that's lost, the background agents still at work when
    /// its host went after its turn had ended: see revive.rs.
    #[serde(skip)]
    pub cut_agents: Vec<String>,
    /// The agent it runs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile: String,
    #[serde(rename = "startedBy", skip_serializing_if = "String::is_empty")]
    pub started_by: String,
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "nullmap"
    )]
    pub meta: BTreeMap<String, String>,
    /// How much context the last request sent.
    #[serde(rename = "contextTokens", skip_serializing_if = "zero")]
    pub context_tokens: i64,
    /// What the agent was given at this start that it wasn't at the one
    /// before.
    #[serde(rename = "contextChange", skip_serializing_if = "Option::is_none")]
    pub context_change: Option<UsageChange>,
    /// What the agent has running in the background, in the order started.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub background: Vec<Task>,
    /// How many tasks the agent started are still running.
    #[serde(skip_serializing_if = "zero")]
    pub tasks: i64,
    /// When a finished task's notice reached the agent while it was idle.
    #[serde(skip_serializing_if = "zero_time")]
    pub waking: Time,
    /// An agent started before its profile was signed in as another account.
    pub relogin: bool,
    /// Its agent starts in the home of the login in use.
    #[serde(skip_serializing_if = "zero")]
    pub homes: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub home: String,
    #[serde(rename = "homeAt", skip_serializing_if = "zero_time")]
    pub home_at: Time,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login: String,
    /// The rush binary the host started from.
    #[serde(skip_serializing_if = "BinStamp::is_zero")]
    pub exe: BinStamp,
    /// Set while it's left to carry on by itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub away: Option<Away>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// The decisions block its last turn ended with, as a question card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Question>,
}

/// Each queued message's images: a message with none is null, as Go
/// writes a nil slice.
mod images_list {
    use super::*;

    pub fn serialize<S: Serializer>(v: &[Vec<String>], s: S) -> Result<S::Ok, S::Error> {
        let w: Vec<Option<&Vec<String>>> = v.iter().map(|i| (!i.is_empty()).then_some(i)).collect();
        w.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<String>>, D::Error> {
        let v = Option::<Vec<Option<Vec<String>>>>::deserialize(d)?.unwrap_or_default();
        Ok(v.into_iter().map(Option::unwrap_or_default).collect())
    }
}

impl Info {
    /// Whether the session is another machine's, seen through rush remote
    /// attach.
    pub fn is_remote(&self) -> bool {
        !self.remote.is_empty()
    }

    /// Brings info an older host wrote up to date (client.go:302).
    pub fn migrate(&mut self) {
        self.kind = migrated(&self.kind).0; // migration: older hosts wrote no kind for Claude Code
    }

    /// Whether a rush newer than the one this host started from is
    /// installed. A host from before it recorded one is never stale.
    pub fn stale(&self, installed: BinStamp) -> bool {
        !self.exe.mod_.is_zero() && installed.mod_.after(self.exe.mod_)
    }

    /// Whether the session is between turns: idle with nothing queued,
    /// asked, running in the background, about to take up a finished
    /// task's notice, stopped by a limit or being retried.
    pub fn quiet(&self) -> bool {
        self.state == "idle"
            && self.queue.is_empty()
            && self.needs.is_empty()
            && self.background.is_empty()
            && self.tasks == 0
            && self.limit.is_none()
            && self.retry.is_none()
            && !self.away.as_ref().is_some_and(Away::on)
            && (self.waking.is_zero()
                || Time::now().sub_nanos(self.waking) >= crate::UNANSWERED_FOR.as_nanos() as i128)
    }

    /// The info as one line of JSON, as info.json has it.
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Reads info.json's bytes, brought up to date.
    pub fn parse(b: &[u8]) -> serde_json::Result<Info> {
        let mut i: Info = serde_json::from_slice(b)?;
        i.migrate();
        Ok(i)
    }
}

/// Whether a host restart should happen now: a newer rush is installed
/// and the session is between turns.
pub fn restart_due(i: &Info, installed: BinStamp) -> bool {
    i.stale(installed) && i.quiet()
}

/// Whether the host went away in the middle of a turn (revive.go:23).
pub fn mid_turn(i: &Info) -> bool {
    !i.sleeping && (i.state == "working" || i.state == "blocked")
}

/// One thing running in the background.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    /// local_bash, local_agent, monitor_mcp, ...
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    #[serde(rename = "startedAt")]
    pub started_at: Time,
    /// The subagent's task that started it, if one did.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent: String,
}

/// A usage limit that stopped the session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limit {
    #[serde(rename = "resetsAt")]
    pub resets_at: Time,
    /// five_hour, seven_day, …
    #[serde(skip_serializing_if = "String::is_empty")]
    pub window: String,
    /// Whether the session carries on at the reset; ask is set while that's
    /// still yours to decide.
    #[serde(rename = "continue")]
    pub continue_: bool,
    #[serde(skip_serializing_if = "zero")]
    pub ask: bool,
}

/// An API error being retried.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retry {
    pub reason: String,
    pub attempt: i64,
    pub max: i64,
    pub next: Time,
    /// Set when retries ran out or the next would land after the cache
    /// expired; a message from you retries.
    #[serde(rename = "gaveUp", skip_serializing_if = "zero")]
    pub gave_up: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// The agent's last tool call before it gave up.
    #[serde(rename = "lastStep", skip_serializing_if = "String::is_empty")]
    pub last_step: String,
    /// Set while the API can't be reached.
    #[serde(skip_serializing_if = "zero")]
    pub offline: bool,
    /// Set while its prompt cache has expired and it waits for proof the
    /// connection holds before trying again.
    #[serde(skip_serializing_if = "zero")]
    pub proof: bool,
}

/// A binary as it was on disk: when it was written and how big it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BinStamp {
    #[serde(rename = "mod")]
    pub mod_: Time,
    pub size: i64,
}

impl BinStamp {
    pub fn is_zero(&self) -> bool {
        self.mod_.is_zero() && self.size == 0
    }

    /// The stamp of the file at path; zero when it can't be read.
    pub fn of(path: &std::path::Path) -> BinStamp {
        let Ok(m) = std::fs::metadata(path) else {
            return BinStamp::default();
        };
        let mt = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .unwrap_or_default();
        BinStamp {
            // Local, as Go's ModTime is.
            mod_: Time::from_unix(mt.as_secs() as i64, mt.subsec_nanos()).local(),
            size: m.len() as i64,
        }
    }

    /// The stamp of the rush that's running: a host's exe.
    pub fn exe() -> BinStamp {
        std::env::current_exe()
            .map(|p| BinStamp::of(&p))
            .unwrap_or_default()
    }
}

/// A session left to carry on by itself (away.go).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Away {
    pub from: Time,
    #[serde(skip_serializing_if = "zero_time")]
    pub until: Time,
    #[serde(with = "rush_core::wire::duration")]
    pub every: Duration,
    /// The next check-in.
    pub next: Time,
    /// Check-ins sent so far.
    pub nudges: i64,
    /// You being there: no "I'm away", and nothing held.
    #[serde(skip_serializing_if = "zero")]
    pub r#loop: bool,
    /// When an away's time ran out or it said it's all done.
    #[serde(skip_serializing_if = "zero_time")]
    pub ended: Time,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "nullvec::deserialize"
    )]
    pub held: Vec<Held>,
}

impl Away {
    /// Whether it is still checking in.
    pub fn on(&self) -> bool {
        self.ended.is_zero()
    }

    /// Whether its owner is away, not looping: nothing is answered till
    /// they're back.
    pub fn gone(&self) -> bool {
        self.on() && !self.r#loop
    }
}

/// Something the agent asked, or asked to do, while you were away.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Held {
    /// question, or permission
    pub kind: String,
    pub text: String,
    pub at: Time,
}

/// A place a session wrote outside its project and scratch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Left {
    pub path: String,
    /// Its last write there.
    pub at: Time,
    /// Read from a shell command, so a guess.
    #[serde(skip_serializing_if = "zero")]
    pub shell: bool,
}

/// Each queued message's exchange, one per message (exchange.go:10).
pub fn queue_exchanges(i: &Info) -> Vec<Option<Box<Exchange>>> {
    let mut q = i.queue_exchanges.clone();
    q.resize(i.queue.len(), None);
    q
}

/// Empty when no queued message has an exchange, so a queue that never
/// had one looks as it always did.
pub fn trim_exchanges(q: Vec<Option<Box<Exchange>>>) -> Vec<Option<Box<Exchange>>> {
    if q.iter().any(Option::is_some) {
        q
    } else {
        Vec::new()
    }
}

/// Each queued message's images, one list per message (host.go:1476).
pub fn queue_images(i: &Info) -> Vec<Vec<String>> {
    let mut q = i.queue_images.clone();
    q.resize(i.queue.len(), Vec::new());
    q
}

/// Empty when no queued message has images.
pub fn trim_images(q: Vec<Vec<String>>) -> Vec<Vec<String>> {
    if q.iter().any(|i| !i.is_empty()) {
        q
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // S81: session --json writes info as Go does: a cost to Go's last
    // digit, a context change's time and a binary's in the zone they were
    // written in.
    #[test]
    fn info_writes_back_as_go_wrote_it() {
        let go = r#"{"costUsd":1.8115424000000002,"contextChange":{"at":"2026-10-29T06:12:34.468226+01:00"},"exe":{"mod":"2026-10-30T09:28:47.19773404+01:00","size":7}}"#;
        let info: Info = serde_json::from_str(go).unwrap();
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["costUsd"].to_string(), "1.8115424000000002");
        assert_eq!(v["contextChange"]["at"], "2026-10-29T06:12:34.468226+01:00");
        assert_eq!(v["exe"]["mod"], "2026-10-30T09:28:47.19773404+01:00");
    }

    // Go's ModTime is local time: a stamp says its zone.
    #[test]
    fn a_binarys_stamp_is_local() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let s = BinStamp::of(f.path());
        assert_eq!(s.mod_.format(), s.mod_.local().format());
        assert!(!s.mod_.is_zero());
    }
}
