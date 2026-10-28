//! What agents spend: tokens, and the quota windows an account has left.
//! It knows no provider; adapters fill it in. After Go's
//! internal/agent/usage: tokens.go, quota.go, reading.go, cache.go,
//! context.go and start.go.
//!
//! Times are Unix milliseconds (0 is unknown, Go's zero time), written as
//! Go writes them, so the quota file Go keeps reads and writes the same.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::time::{self, Ms};
use crate::wire;

/// Tokens by kind (Go's usage.TokenUsage). Providers without a prompt
/// cache leave the cache fields zero.
pub use crate::agent::TokenUsage;

/// TokenUsage as Go writes it: every field, by its Go name.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct TokenUsageWire {
    #[serde(rename = "Input")]
    input: i64,
    #[serde(rename = "Output")]
    output: i64,
    #[serde(rename = "CacheRead")]
    cache_read: i64,
    #[serde(rename = "CacheWrite5m")]
    cache_write_5m: i64,
    #[serde(rename = "CacheWrite1h")]
    cache_write_1h: i64,
    #[serde(rename = "Reasoning")]
    reasoning: i64,
}

impl Serialize for TokenUsage {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        TokenUsageWire {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write_5m: self.cache_write_5m,
            cache_write_1h: self.cache_write_1h,
            reasoning: self.reasoning,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for TokenUsage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<TokenUsage, D::Error> {
        let w = Option::<TokenUsageWire>::deserialize(d)?.unwrap_or_default();
        Ok(TokenUsage {
            input: w.input,
            output: w.output,
            cache_read: w.cache_read,
            cache_write_5m: w.cache_write_5m,
            cache_write_1h: w.cache_write_1h,
            reasoning: w.reasoning,
        })
    }
}

/// A reading's Problem when its sign-in has expired, one rush may be able
/// to refresh.
pub const EXPIRED: &str = "sign-in expired";

/// How old a reading may get before it's read again. Every rush process
/// and session shares them, so however many are open an account is asked
/// once in that time.
pub const EVERY: Duration = Duration::from_secs(5 * 60);

/// How far ahead rush looks for an account filling up: past the next
/// reading, and the turns already under way on it when it switches.
pub const LEAD: Duration = Duration::from_secs(2 * 5 * 60);

/// What reading other agents' limits is called where rush shows what
/// waits on the network.
pub const JOB: &str = "Other agents' limits";

const HOUR_MS: f64 = 3_600_000.0;

/// One limit an account's plan puts on it: Claude's five hours or week,
/// Codex's primary or secondary, Copilot's month of premium requests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Window {
    /// The provider's name for it: "five_hour", "seven_day_opus", "primary".
    #[serde(rename = "ID")]
    pub id: String,
    /// Short, for a meter: "5h", "7d", "Opus 7d", "month".
    pub label: String,
    /// In words: "5-hour", "weekly", "Opus weekly", "premium requests".
    pub name: String,
    /// How long it runs; zero when the provider doesn't say.
    #[serde(with = "wire::duration")]
    pub span: Duration,
    /// How much of it is used, 0–100. It is always set, from used and limit
    /// when the provider counts instead.
    #[serde(with = "wire::float")]
    pub percent: f64,
    /// When the provider counts: 212 of 300 requests.
    #[serde(with = "wire::float")]
    pub used: f64,
    #[serde(with = "wire::float")]
    pub limit: f64,
    #[serde(with = "wire::time")]
    pub resets_at: Ms,
    pub scope: Scope,
    /// How fast it's been filling lately, in percent an hour, from the
    /// reading before (follow); zero until there's one.
    #[serde(with = "wire::float", skip_serializing_if = "is_zero")]
    pub burn: f64,
}

fn is_zero(f: &f64) -> bool {
    *f == 0.0
}

impl Window {
    /// How fast it fills, in percent an hour: its recent burn, or its
    /// average since it began when that's faster.
    pub fn rate(&self, now: Ms) -> f64 {
        let mut r = self.burn;
        if !self.span.is_zero() && self.resets_at != 0 {
            let gone = self.span.as_millis() as i64 - (self.resets_at - now);
            if gone >= 10 * 60_000 {
                r = r.max(self.percent / (gone as f64 / HOUR_MS));
            }
        }
        r
    }

    /// How much of lead it can still fill in, in ms: none of it past its
    /// reset, when what's left is gone anyway.
    fn ahead(&self, lead: Duration, now: Ms) -> i64 {
        let lead = lead.as_millis() as i64;
        if self.resets_at == 0 {
            return lead;
        }
        0.max(lead.min(self.resets_at - now))
    }
}

/// Which models a window limits. The default Scope limits them all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Scope {
    /// Matched as substrings of a model's id, so "opus" covers every Opus.
    #[serde(with = "wire::nullvec")]
    pub models: Vec<String>,
}

impl Scope {
    /// Whether the window limits model. An empty model is any.
    pub fn covers(&self, model: &str) -> bool {
        if self.models.is_empty() || model.is_empty() {
            return true;
        }
        let model = model.to_lowercase();
        self.models
            .iter()
            .any(|m| model.contains(&m.to_lowercase()))
    }
}

/// Where a reading came from. On the wire it is its number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Source {
    /// The agent's own cache of it.
    #[default]
    Cached,
    /// Asked of the provider by rush.
    Fetched,
    /// Reported by a session as it ran.
    Live,
}

impl Serialize for Source {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Source, D::Error> {
        Ok(match Option::<i64>::deserialize(d)?.unwrap_or(0) {
            1 => Source::Fetched,
            2 => Source::Live,
            _ => Source::Cached,
        })
    }
}

/// How what a session spends is paid for: "plan", "overage" or "metered"
/// on the wire. Go keeps it a string, so any other word reads as Other.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Billing {
    /// None said.
    #[default]
    Unknown,
    /// Out of a subscription's allowance: its windows fill, and nothing
    /// more is charged.
    Plan,
    /// Past the allowance and paid for by use: Claude's extra usage,
    /// Codex's credits.
    Overage,
    /// An API key: every token is paid for.
    Metered,
    /// A word rush doesn't know, kept as it was.
    Other(String),
}

impl Billing {
    pub fn as_str(&self) -> &str {
        match self {
            Billing::Unknown => "",
            Billing::Plan => "plan",
            Billing::Overage => "overage",
            Billing::Metered => "metered",
            Billing::Other(s) => s,
        }
    }

    pub fn from_word(s: &str) -> Billing {
        match s {
            "" => Billing::Unknown,
            "plan" => Billing::Plan,
            "overage" => Billing::Overage,
            "metered" => Billing::Metered,
            s => Billing::Other(s.to_string()),
        }
    }
}

impl Serialize for Billing {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Billing {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Billing, D::Error> {
        Ok(Billing::from_word(
            &Option::<String>::deserialize(d)?.unwrap_or_default(),
        ))
    }
}

/// An account's limits as last read. An account belongs to one provider,
/// but any number of agents and sessions can spend it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Quota {
    /// The key it is kept under: "claude:login:<uuid>", "codex:chatgpt:<id>".
    pub account: String,
    /// Who the account is, when the reading says.
    pub email: String,
    pub plan: String,
    /// What's left on a pay-as-you-go account, in its own currency and
    /// words: "¥110.00 left". Such an account has no windows.
    pub balance: String,
    #[serde(with = "wire::nullvec")]
    pub windows: Vec<Window>,
    #[serde(with = "wire::time")]
    pub fetched_at: Ms,
    pub source: Source,
    /// Why there is no fresh reading: not signed in, expired, rate-limited.
    pub problem: String,
    /// The limit resets the account has earned; None when its provider has
    /// none, or didn't say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets: Option<Resets>,
}

/// An account's earned limit resets: each one, spent, resets the limits
/// it's for at once. Credits has the details the provider gave, which may
/// be fewer than Available.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Resets {
    pub available: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub credits: Vec<ResetCredit>,
}

/// One earned reset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ResetCredit {
    #[serde(rename = "ID")]
    pub id: String,
    pub title: String,
    pub about: String,
    #[serde(with = "wire::time")]
    pub granted: Ms,
    /// Zero when it doesn't.
    #[serde(with = "wire::time")]
    pub expires: Ms,
}

impl Quota {
    /// Whether a window that limits model fills within lead: 99% full
    /// already, or full before then, and before it resets, as fast as it
    /// fills.
    pub fn nearly_out(&self, model: &str, lead: Duration, now: Ms) -> bool {
        self.windows.iter().any(|w| {
            w.scope.covers(model)
                && (w.percent >= 99.0
                    || w.percent + w.rate(now) * (w.ahead(lead, now) as f64 / HOUR_MS) >= 100.0)
        })
    }

    /// How full the tightest window for model gets before nearly_out says
    /// so: 99%, or sooner the faster it fills.
    pub fn switch_point(&self, model: &str, lead: Duration, now: Ms) -> f64 {
        let w = self.tightest(model).unwrap_or_default();
        let fill = w.rate(now) * (w.ahead(lead, now) as f64 / HOUR_MS);
        (100.0 - fill).clamp(0.0, 99.0)
    }

    /// The fullest window that limits model: the one that stops it first.
    /// None when no window limits it.
    pub fn tightest(&self, model: &str) -> Option<Window> {
        let mut out: Option<&Window> = None;
        for c in &self.windows {
            if c.scope.covers(model) && out.is_none_or(|w| c.percent > w.percent) {
                out = Some(c);
            }
        }
        out.cloned()
    }

    /// How full the tightest window for model is, 0–100.
    pub fn used(&self, model: &str) -> f64 {
        self.tightest(model).map_or(0.0, |w| w.percent)
    }

    /// The window called id, if the reading has it.
    pub fn window(&self, id: &str) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Empties the windows that have reset since the reading was made:
    /// nothing has been used of them yet, as far as the reading knows.
    pub fn since(&self, now: Ms) -> Quota {
        let mut q = self.clone();
        for w in &mut q.windows {
            if w.resets_at != 0 && now > w.resets_at {
                w.percent = 0.0;
                w.used = 0.0;
                w.resets_at = 0;
            }
        }
        q
    }
}

/// `next` with each window's recent burn worked out from `prev`, an earlier
/// reading of the same account at least EVERY before it: percents are
/// whole, so one step a minute apart reads as 60% an hour.
pub fn follow(prev: &Quota, next: &Quota) -> Quota {
    let dt = next.fetched_at - prev.fetched_at;
    let mut next = next.clone();
    for w in &mut next.windows {
        match prev.window(&w.id) {
            None => {}                                                   // new
            Some(p) if w.percent < p.percent => {}                       // reset since
            Some(p) if dt < EVERY.as_millis() as i64 => w.burn = p.burn, // too close together to tell
            Some(p) => w.burn = (p.burn + (w.percent - p.percent) / (dt as f64 / HOUR_MS)) / 2.0,
        }
    }
    next
}

/// One reading of an account's plan: who the account is, and how much of
/// its limits it has used.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Reading {
    /// The signed-in account's own id.
    #[serde(rename = "AccountID")]
    pub account_id: String,
    pub email: String,
    pub org: String,
    pub plan: String,
    /// What else the provider says of the account.
    pub role: String,
    pub billing: String,
    pub org_type: String,
    /// Whether use past its limits is billed.
    pub extra: bool,
    #[serde(with = "wire::nullvec")]
    pub windows: Vec<Window>,
    #[serde(with = "wire::time")]
    pub fetched_at: Ms,
    /// Why no fresh reading: not signed in, expired, rate-limited.
    pub problem: String,
    /// Asked of the provider by rush, not the agent's own cache.
    pub fetched: bool,
}

impl Reading {
    /// The reading's limits, kept under key.
    pub fn quota(&self, key: &str) -> Quota {
        Quota {
            account: key.to_string(),
            plan: self.plan.clone(),
            windows: self.windows.clone(),
            fetched_at: self.fetched_at,
            problem: self.problem.clone(),
            source: if self.fetched {
                Source::Fetched
            } else {
                Source::Cached
            },
            ..Quota::default()
        }
    }

    /// Empties the windows that have reset since the reading was made.
    pub fn since(&self, now: Ms) -> Reading {
        let mut r = self.clone();
        if !r.windows.is_empty() {
            r.windows = Quota {
                windows: std::mem::take(&mut r.windows),
                ..Quota::default()
            }
            .since(now)
            .windows;
        }
        r
    }
}

// ---- The shared file of readings (Go's usage/cache.go) ----

/// Whether the network is up for a job, and what came of it: Go's
/// netwatch, which rush shows as what waits on the network. A refresh asks
/// before it reads, and says how it went.
pub trait Network {
    /// Whether job may go out now; false while the network is down.
    fn run(&self, job: &str) -> bool;
    /// How job went: None when it worked.
    fn done(&self, job: &str, err: Option<&str>);
}

/// A Network that is always up and keeps no record.
pub struct Online;

impl Network for Online {
    fn run(&self, _: &str) -> bool {
        true
    }
    fn done(&self, _: &str, _: Option<&str>) {}
}

/// The readings kept at path, by key.
pub fn load(path: &Path) -> BTreeMap<String, Quota> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Keeps q under key, unless the reading there is newer.
pub fn record(path: &Path, key: &str, q: &Quota) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    mkdir_private(dir)?;
    let _lock = lock(&with_suffix(path, ".lock"));
    let mut all = load(path);
    if let Some(old) = all.get(key)
        && q.fetched_at <= old.fetched_at
    {
        return Ok(());
    }
    all.insert(key.to_string(), q.clone());
    let b = serde_json::to_vec(&all).map_err(io::Error::other)?;
    let tmp = dir.join(format!(
        ".quotas-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let written = File::create(&tmp).and_then(|mut f| f.write_all(&b));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path)
}

/// The reading kept under key, read again with fetch when it's older than
/// EVERY; offline never reads. One process reads at a time, and the others
/// find its reading. A failed read keeps the last reading, with why in
/// problem.
pub fn refresh(
    path: &Path,
    key: &str,
    offline: bool,
    net: &dyn Network,
    fetch: impl FnOnce() -> Result<Quota, String>,
) -> Quota {
    let _lock = (!offline).then(|| {
        use sha2::Digest as _;
        let sum = sha2::Sha256::digest(key.as_bytes());
        let hex: String = sum[..8].iter().map(|b| format!("{b:02x}")).collect();
        lock(&with_suffix(path, ".locks").join(hex))
    });
    let mut q = load(path).remove(key).unwrap_or_default();
    if offline || time::now() - q.fetched_at < EVERY.as_millis() as i64 {
        return q;
    }
    if !net.run(JOB) {
        return q; // the network's down: the last reading, until it's back
    }
    let got = fetch();
    net.done(JOB, got.as_ref().err().map(String::as_str));
    match got {
        Err(e) => {
            q.problem = e;
            q
        }
        Ok(mut got) => {
            if got.fetched_at == 0 {
                got.fetched_at = time::now();
            }
            if got.account.is_empty() {
                got.account = key.to_string();
            }
            let _ = record(path, key, &got);
            got
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn mkdir_private(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

/// An exclusive lock on the file at path (flock, as Go's), held until
/// dropped; without one it holds nothing.
fn lock(path: &Path) -> Option<File> {
    mkdir_private(path.parent()?).ok()?;
    let mut o = OpenOptions::new();
    o.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let f = o.open(path).ok()?;
    let _ = f.lock();
    Some(f)
}

// ---- What fills a context window (Go's usage/context.go) ----

/// What fills a session's context window, part by part, as its agent
/// counts it. Its JSON is what clients have always read: the names are
/// Claude Code's /context answer's, the first agent that gave one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Context {
    #[serde(rename = "categories", with = "wire::nullvec")]
    pub parts: Vec<ContextPart>,
    #[serde(rename = "totalTokens")]
    pub total: i64,
    #[serde(rename = "maxTokens")]
    pub max: i64,
    #[serde(with = "wire::float")]
    pub percentage: f64,
    pub model: String,
    /// The fill at which the agent summarises the conversation, when
    /// auto_compact is on.
    #[serde(rename = "autoCompactThreshold")]
    pub auto_compact_at: i64,
    #[serde(rename = "isAutoCompactEnabled")]
    pub auto_compact: bool,
    #[serde(rename = "memoryFiles", with = "wire::nullvec")]
    pub memory_files: Vec<ContextFile>,
    #[serde(rename = "mcpTools", with = "wire::nullvec")]
    pub mcp_tools: Vec<ContextMcpTool>,
    #[serde(with = "wire::nullvec")]
    pub agents: Vec<ContextAgent>,
    pub skills: ContextSkills,
    #[serde(rename = "messageBreakdown")]
    pub messages: ContextMessages,
    /// When it was counted.
    #[serde(with = "wire::time")]
    pub at: Ms,
}

/// One part of the window. Kind is used, deferred (tools loaded on demand,
/// not in the window), buffer (kept free for auto-compact) or free.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextPart {
    pub name: String,
    pub tokens: i64,
    pub kind: String,
    #[serde(rename = "isDeferred")]
    pub deferred: bool,
}

/// A memory file the agent loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextFile {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub tokens: i64,
}

/// One MCP server's tool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextMcpTool {
    pub name: String,
    #[serde(rename = "serverName")]
    pub server: String,
    pub tokens: i64,
    #[serde(rename = "isLoaded")]
    pub is_loaded: bool,
}

/// A subagent the model is told it can start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextAgent {
    #[serde(rename = "agentType")]
    pub kind: String,
    pub source: String,
    pub tokens: i64,
}

/// The skills the model is told of.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextSkills {
    #[serde(rename = "totalSkills")]
    pub total: i64,
    #[serde(rename = "includedSkills")]
    pub included: i64,
    pub tokens: i64,
    #[serde(rename = "skillFrontmatter", with = "wire::nullvec")]
    pub each: Vec<ContextSkill>,
}

/// One of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextSkill {
    pub name: String,
    pub source: String,
    pub tokens: i64,
}

/// The conversation's share, by what it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextMessages {
    #[serde(rename = "toolCallTokens")]
    pub tool_calls: i64,
    #[serde(rename = "toolResultTokens")]
    pub tool_results: i64,
    #[serde(rename = "attachmentTokens")]
    pub attachments: i64,
    #[serde(rename = "assistantMessageTokens")]
    pub assistant: i64,
    #[serde(rename = "userMessageTokens")]
    pub user: i64,
    #[serde(rename = "toolCallsByType", with = "wire::nullvec")]
    pub by_tool: Vec<ContextToolUses>,
}

/// One tool's calls and results.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextToolUses {
    pub name: String,
    #[serde(rename = "callTokens")]
    pub calls: i64,
    #[serde(rename = "resultTokens")]
    pub results: i64,
}

// ---- What a session started with (Go's usage/start.go) ----

/// What a session was given when its agent last started: the tools and MCP
/// servers it said it had, the parts of the prompt rush added, and, once it
/// has counted, the agent's own totals by category. The agent's own base
/// prompt isn't in it: rush never sees that text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Start {
    #[serde(with = "wire::time")]
    pub at: Ms,
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub servers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub sections: Vec<Section>,
    /// The agent's totals by category at its first count, the conversation
    /// and free space left out; None until it has counted. (Go writes an
    /// empty count as none, and so does this.)
    #[serde(skip_serializing_if = "no_parts")]
    pub parts: Option<BTreeMap<String, i64>>,
}

fn no_parts(p: &Option<BTreeMap<String, i64>>) -> bool {
    p.as_ref().is_none_or(|m| m.is_empty())
}

/// One part of the prompt rush adds, by name, and its size estimated at
/// CHARS_PER_TOKEN.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Section {
    pub name: String,
    pub tokens: i64,
}

/// How many characters make a token, roughly, for text rush adds that no
/// agent has counted.
pub const CHARS_PER_TOKEN: f64 = 3.5;

/// text's size in tokens at CHARS_PER_TOKEN.
pub fn estimate(text: &str) -> i64 {
    (text.len() as f64 / CHARS_PER_TOKEN + 0.5) as i64
}

/// A count's categories as a Start keeps them: what's used, less the
/// conversation, which grows by itself between starts.
pub fn parts_of(u: &Context) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for p in &u.parts {
        if p.deferred
            || p.kind == "deferred"
            || p.kind == "buffer"
            || p.kind == "free"
            || p.name == "Messages"
        {
            continue;
        }
        *out.entry(p.name.clone()).or_insert(0) += p.tokens;
    }
    out
}

/// What differs between two starts of a session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Change {
    #[serde(with = "wire::time")]
    pub at: Ms,
    #[serde(
        rename = "toolsAdded",
        skip_serializing_if = "Vec::is_empty",
        with = "wire::nullvec"
    )]
    pub tools_added: Vec<String>,
    #[serde(
        rename = "toolsRemoved",
        skip_serializing_if = "Vec::is_empty",
        with = "wire::nullvec"
    )]
    pub tools_removed: Vec<String>,
    #[serde(
        rename = "serversAdded",
        skip_serializing_if = "Vec::is_empty",
        with = "wire::nullvec"
    )]
    pub servers_added: Vec<String>,
    #[serde(
        rename = "serversRemoved",
        skip_serializing_if = "Vec::is_empty",
        with = "wire::nullvec"
    )]
    pub servers_removed: Vec<String>,
    /// rush's prompt sections added (was 0, now set), removed (now 0) or
    /// changed in size, estimated.
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub sections: Vec<Delta>,
    /// The agent's own categories that changed, as it counted them; only
    /// when both starts were counted.
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub parts: Vec<Delta>,
}

/// One thing's size before and after, in tokens; gone or added say it
/// wasn't there on one side.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Delta {
    pub name: String,
    pub was: i64,
    pub now: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub added: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub gone: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Delta {
    /// now less was.
    pub fn change(&self) -> i64 {
        self.now - self.was
    }
}

impl Change {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.tools_added.is_empty()
            && self.tools_removed.is_empty()
            && self.servers_added.is_empty()
            && self.servers_removed.is_empty()
            && self.sections.is_empty()
            && self.parts.is_empty()
    }
}

/// What changed from was to now; None when nothing did. Tools are only
/// compared when both starts said theirs, and categories when both were
/// counted, so a start the agent hadn't yet described isn't read as
/// everything removed.
pub fn diff(was: Option<&Start>, now: Option<&Start>) -> Option<Change> {
    let (was, now) = (was?, now?);
    let mut c = Change {
        at: now.at,
        ..Change::default()
    };
    if !was.tools.is_empty() && !now.tools.is_empty() {
        (c.tools_added, c.tools_removed) = set_diff(&was.tools, &now.tools);
        (c.servers_added, c.servers_removed) = set_diff(&was.servers, &now.servers);
    }
    let sum = |ss: &[Section]| {
        let mut m = BTreeMap::new();
        for s in ss {
            *m.entry(s.name.clone()).or_insert(0) += s.tokens;
        }
        m
    };
    c.sections = deltas(&sum(&was.sections), &sum(&now.sections), true);
    if let (Some(w), Some(n)) = (&was.parts, &now.parts) {
        c.parts = deltas(w, n, false);
    }
    (!c.is_empty()).then_some(c)
}

/// What's in now and not was, and in was and not now, sorted.
fn set_diff(was: &[String], now: &[String]) -> (Vec<String>, Vec<String>) {
    let mut added: Vec<String> = Vec::new();
    for n in now {
        if !was.contains(n) && !added.contains(n) {
            added.push(n.clone());
        }
    }
    let mut removed: Vec<String> = Vec::new();
    for w in was {
        if !now.contains(w) && !removed.contains(w) {
            removed.push(w.clone());
        }
    }
    added.sort();
    removed.sort();
    (added, removed)
}

/// The names whose size changed, added or went, sorted by how much they
/// moved, most first. A part that's 0 on one side and absent on the other
/// hasn't changed; keep_zero keeps a section with no text as added or gone,
/// while a category at 0 counts as absent.
fn deltas(was: &BTreeMap<String, i64>, now: &BTreeMap<String, i64>, keep_zero: bool) -> Vec<Delta> {
    let mut out = Vec::new();
    for (n, &t) in now {
        match was.get(n) {
            None if t > 0 || keep_zero => out.push(Delta {
                name: n.clone(),
                now: t,
                added: true,
                ..Delta::default()
            }),
            Some(&w) if w != t => out.push(Delta {
                name: n.clone(),
                was: w,
                now: t,
                ..Delta::default()
            }),
            _ => {}
        }
    }
    for (n, &w) in was {
        if !now.contains_key(n) && (w > 0 || keep_zero) {
            out.push(Delta {
                name: n.clone(),
                was: w,
                gone: true,
                ..Delta::default()
            });
        }
    }
    out.sort_by(|a, b| {
        b.change()
            .abs()
            .cmp(&a.change().abs())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;
    const MINUTE: i64 = 60_000;

    #[test]
    fn tightest_follows_the_model() {
        let q = Quota {
            windows: vec![
                Window {
                    id: "five_hour".into(),
                    percent: 40.0,
                    ..Window::default()
                },
                Window {
                    id: "seven_day".into(),
                    percent: 55.0,
                    ..Window::default()
                },
                Window {
                    id: "seven_day_opus".into(),
                    percent: 90.0,
                    scope: Scope {
                        models: vec!["opus".into()],
                    },
                    ..Window::default()
                },
            ],
            ..Quota::default()
        };
        for (model, want) in [
            ("claude-opus-5-5[1m]", "seven_day_opus"),
            ("claude-sonnet-5", "seven_day"),
            ("", "seven_day_opus"), // any model: the fullest of all
        ] {
            assert_eq!(
                q.tightest(model).map(|w| w.id),
                Some(want.to_string()),
                "{model}"
            );
        }
        assert_eq!(q.used("claude-sonnet-5"), 55.0);
    }

    #[test]
    fn tightest_with_no_windows() {
        assert!(Quota::default().tightest("gpt-5").is_none());
    }

    #[test]
    fn since_empties_reset_windows() {
        let now = 1_790_510_400_000;
        let q = Quota {
            windows: vec![
                Window {
                    id: "five_hour".into(),
                    percent: 80.0,
                    resets_at: now - MINUTE,
                    ..Window::default()
                },
                Window {
                    id: "seven_day".into(),
                    percent: 30.0,
                    resets_at: now + HOUR,
                    ..Window::default()
                },
            ],
            ..Quota::default()
        };
        let got = q.since(now);
        let w = got.window("five_hour").unwrap();
        assert!(w.percent == 0.0 && w.resets_at == 0, "{w:?}");
        assert_eq!(got.window("seven_day").unwrap().percent, 30.0);
        assert_eq!(
            q.window("five_hour").unwrap().percent,
            80.0,
            "since changed the reading"
        );
    }

    fn week(now: Ms, pct: f64, ago: i64, resets_in: i64) -> Quota {
        Quota {
            fetched_at: now - ago,
            windows: vec![Window {
                id: "seven_day".into(),
                percent: pct,
                span: Duration::from_secs(7 * 24 * 3600),
                resets_at: now + resets_in,
                ..Window::default()
            }],
            ..Quota::default()
        }
    }

    #[test]
    fn switches_as_it_fills() {
        let now = time::now();
        // A week six days in: well under 1% an hour on average.
        let read = |pct, ago| week(now, pct, ago, 24 * HOUR);
        // Slow: it runs to 99%.
        let slow = follow(&read(96.9, 30 * MINUTE), &read(97.0, 0));
        assert!(!slow.nearly_out("", LEAD, now), "switched at 97%");
        let sp = slow.switch_point("", LEAD, now);
        assert!((97.0..=99.0).contains(&sp), "slow switch point {sp}");
        assert!(
            read(99.0, 0).nearly_out("", LEAD, now),
            "99% isn't nearly out"
        );
        // Fast: 30% in 10 minutes, so 85% won't last the lead.
        let fast = follow(&read(55.0, 10 * MINUTE), &read(85.0, 0));
        assert!(fast.nearly_out("", LEAD, now));
        // A reset since the last reading isn't a burn.
        let w = &follow(&read(90.0, 10 * MINUTE), &read(2.0, 0)).windows[0];
        assert_eq!(w.burn, 0.0, "burn across a reset");
    }

    #[test]
    fn a_week_about_to_reset_isnt_nearly_out() {
        let now = time::now();
        // One whole percent a minute apart is rounding, not 60% an hour.
        let q = follow(&week(now, 91.0, MINUTE, HOUR), &week(now, 92.0, 0, HOUR));
        assert!(
            !q.nearly_out("", LEAD, now),
            "a 1% step a minute apart switched"
        );
        // Filling fast, but it resets before it would fill.
        let mut five = Quota {
            fetched_at: now,
            windows: vec![Window {
                id: "five_hour".into(),
                percent: 85.0,
                span: Duration::from_secs(5 * 3600),
                resets_at: now + 3 * MINUTE,
                burn: 120.0,
                ..Window::default()
            }],
            ..Quota::default()
        };
        assert!(
            !five.nearly_out("", LEAD, now),
            "85% resetting in 3 minutes"
        );
        five.windows[0].resets_at = now + HOUR;
        assert!(
            five.nearly_out("", LEAD, now),
            "85% at 120%/h for the next hour"
        );
    }

    #[test]
    fn refresh_reads_once_in_a_while() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quotas.json");
        let reads = std::cell::Cell::new(0);
        let fetch = || {
            reads.set(reads.get() + 1);
            Ok(Quota {
                windows: vec![Window {
                    id: "primary".into(),
                    percent: 30.0,
                    ..Window::default()
                }],
                ..Quota::default()
            })
        };
        let q = refresh(&path, "codex:a", false, &Online, fetch);
        assert!(reads.get() == 1 && q.used("") == 30.0 && q.account == "codex:a");
        assert_ne!(q.fetched_at, 0);
        let q = refresh(&path, "codex:a", false, &Online, fetch);
        assert!(
            reads.get() == 1 && q.used("") == 30.0,
            "a fresh reading was read again"
        );
        let q = refresh(&path, "codex:b", true, &Online, fetch);
        assert!(reads.get() == 1 && q.windows.is_empty(), "offline read");

        // An older reading doesn't replace a newer one.
        let mut old = load(&path).remove("codex:a").unwrap();
        old.fetched_at = time::now() - HOUR;
        record(&path, "codex:a", &old).unwrap();
        assert_ne!(load(&path)["codex:a"].fetched_at, old.fetched_at);

        let failed = refresh(&dir.path().join("q.json"), "x", false, &Online, || {
            Err("signed out".into())
        });
        assert_eq!(failed.problem, "signed out");
    }

    #[test]
    fn refresh_waits_for_the_network() {
        struct Down;
        impl Network for Down {
            fn run(&self, job: &str) -> bool {
                assert_eq!(job, JOB);
                false
            }
            fn done(&self, _: &str, _: Option<&str>) {
                panic!("nothing went out");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let q = refresh(&dir.path().join("q.json"), "k", false, &Down, || {
            panic!("read while the network is down")
        });
        assert_eq!(q, Quota::default());
    }

    #[test]
    fn diff_says_what_changed_between_starts() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let sec = |n: &str, t| Section {
            name: n.into(),
            tokens: t,
        };
        let parts = |v: &[(&str, i64)]| Some(v.iter().map(|(k, n)| (k.to_string(), *n)).collect());
        let was = Start {
            tools: s(&["Bash", "Read", "mcp__old__a"]),
            servers: s(&["old"]),
            sections: vec![sec("rush's task list", 300), sec("plugin hex", 120)],
            parts: parts(&[("System tools", 12000), ("MCP tools", 800), ("Skills", 500)]),
            ..Start::default()
        };
        let now = Start {
            tools: s(&["Bash", "Read", "mcp__ts__def", "mcp__ts__refs"]),
            servers: s(&["ts"]),
            sections: vec![sec("rush's task list", 340), sec("plugin ts", 900)],
            parts: parts(&[("System tools", 12000), ("MCP tools", 2400), ("Skills", 0)]),
            ..Start::default()
        };
        let c = diff(Some(&was), Some(&now)).expect("no change found");
        assert_eq!(c.tools_added, s(&["mcp__ts__def", "mcp__ts__refs"]));
        assert_eq!(c.tools_removed, s(&["mcp__old__a"]));
        assert_eq!(c.servers_added, s(&["ts"]));
        assert_eq!(c.servers_removed, s(&["old"]));
        // Biggest move first: the new plugin, the old one, then the task list.
        let d = |n: &str, was, now, added, gone| Delta {
            name: n.into(),
            was,
            now,
            added,
            gone,
        };
        assert_eq!(
            c.sections,
            [
                d("plugin ts", 0, 900, true, false),
                d("plugin hex", 120, 0, false, true),
                d("rush's task list", 300, 340, false, false)
            ]
        );
        // System tools held; Skills went to 0, which is a change, not a removal.
        assert_eq!(
            c.parts,
            [
                d("MCP tools", 800, 2400, false, false),
                d("Skills", 500, 0, false, false)
            ]
        );
    }

    #[test]
    fn diff_compares_only_what_both_starts_knew() {
        let sec = vec![Section {
            name: "a".into(),
            tokens: 10,
        }];
        let memory = Some(BTreeMap::from([("Memory".to_string(), 100)]));
        let was = Start {
            tools: vec!["Bash".into()],
            sections: sec.clone(),
            parts: memory.clone(),
            ..Start::default()
        };
        // Not yet told its tools, not yet counted: neither reads as removed.
        let bare = Start {
            sections: sec.clone(),
            ..Start::default()
        };
        assert_eq!(diff(Some(&was), Some(&bare)), None);
        let first = Start {
            tools: vec!["Bash".into()],
            ..Start::default()
        };
        assert_eq!(diff(None, Some(&first)), None);
        assert_eq!(diff(Some(&was), Some(&was.clone())), None);
    }

    #[test]
    fn parts_of_leaves_out_the_conversation_and_free_space() {
        let p = |name: &str, tokens, kind: &str, deferred| ContextPart {
            name: name.into(),
            tokens,
            kind: kind.into(),
            deferred,
        };
        let u = Context {
            parts: vec![
                p("System prompt", 3000, "", false),
                p("Messages", 40000, "", false),
                p("MCP tools", 900, "", true),
                p("Autocompact buffer", 33000, "buffer", false),
                p("Free space", 100000, "free", false),
            ],
            ..Context::default()
        };
        assert_eq!(
            parts_of(&u),
            BTreeMap::from([("System prompt".to_string(), 3000)])
        );
        assert_eq!(estimate("1234567"), 2);
    }

    /// What Go's jsonx writes for these, byte for byte.
    #[test]
    fn writes_as_go_does() {
        let at = 1_790_510_400_000;
        let all = BTreeMap::from([(
            "k".to_string(),
            Quota {
                account: "k".into(),
                fetched_at: at,
                windows: vec![Window {
                    id: "w".into(),
                    burn: 1.5,
                    ..Window::default()
                }],
                resets: Some(Resets {
                    available: 1,
                    credits: vec![ResetCredit {
                        id: "r".into(),
                        granted: at,
                        ..ResetCredit::default()
                    }],
                }),
                ..Quota::default()
            },
        )]);
        let start = Start {
            at,
            parts: Some(BTreeMap::from([("b".to_string(), 1), ("a".to_string(), 2)])),
            ..Start::default()
        };
        let change = Change {
            parts: vec![Delta {
                name: "x".into(),
                gone: true,
                ..Delta::default()
            }],
            ..Change::default()
        };
        for (got, want) in [
            (
                serde_json::to_string(&all).unwrap(),
                r#"{"k":{"Account":"k","Email":"","Plan":"","Balance":"","Windows":[{"ID":"w","Label":"","Name":"","Span":0,"Percent":0,"Used":0,"Limit":0,"ResetsAt":"0001-01-01T00:00:00Z","Scope":{"Models":null},"Burn":1.5}],"FetchedAt":"2026-09-27T12:00:00Z","Source":0,"Problem":"","Resets":{"Available":1,"Credits":[{"ID":"r","Title":"","About":"","Granted":"2026-09-27T12:00:00Z","Expires":"0001-01-01T00:00:00Z"}]}}}"#,
            ),
            (
                serde_json::to_string(&start).unwrap(),
                r#"{"at":"2026-09-27T12:00:00Z","parts":{"a":2,"b":1}}"#,
            ),
            (
                serde_json::to_string(&Context::default()).unwrap(),
                r#"{"categories":null,"totalTokens":0,"maxTokens":0,"percentage":0,"model":"","autoCompactThreshold":0,"isAutoCompactEnabled":false,"memoryFiles":null,"mcpTools":null,"agents":null,"skills":{"totalSkills":0,"includedSkills":0,"tokens":0,"skillFrontmatter":null},"messageBreakdown":{"toolCallTokens":0,"toolResultTokens":0,"attachmentTokens":0,"assistantMessageTokens":0,"userMessageTokens":0,"toolCallsByType":null},"at":"0001-01-01T00:00:00Z"}"#,
            ),
            (
                serde_json::to_string(&change).unwrap(),
                r#"{"at":"0001-01-01T00:00:00Z","parts":[{"name":"x","was":0,"now":0,"gone":true}]}"#,
            ),
        ] {
            assert_eq!(got, want);
        }
        let back: BTreeMap<String, Quota> =
            serde_json::from_str(&serde_json::to_string(&all).unwrap()).unwrap();
        assert_eq!(back, all);
    }
}
