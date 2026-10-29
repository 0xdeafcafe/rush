//! The sessions rush runs itself, as the list needs them: what a host
//! says of its session (Go's host.Info, the fields the fleet reads), the
//! source the host module implements to list them, where their folders
//! are, and each made into a row. Port of internal/fleet/fleet.go:1036
//! (hostedAgent), 1138 (hosted), 1245 (awayDetail), idle_rows.go, and the
//! path helpers of host/host.go:359 (Root), host/tmp.go (TempRoot,
//! TempDir, IsTempDir, IsTemp) and host/decisions.go:458 (NeedsDecisions).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rush_core::agent::{Left, Workspace};
use rush_core::time::Ms;

/// One thing a session runs in the background (host.Task).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostTask {
    pub id: String,
    /// local_bash, local_agent, monitor_mcp, …
    pub kind: String,
    pub label: String,
    pub started_at: Ms,
}

/// A usage limit that stopped the session (host.Limit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limit {
    pub resets_at: Ms,
    /// Whether to carry on at the reset is still yours to decide.
    pub ask: bool,
}

/// An API error being retried (host.Retry).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Retry {
    pub attempt: i64,
    pub max: i64,
    /// Retries ran out; a message from you retries.
    pub gave_up: bool,
    pub why: String,
    /// The API can't be reached: it continues once it can.
    pub offline: bool,
    /// Its prompt cache expired and it waits for proof the connection
    /// holds before trying again.
    pub proof: bool,
}

/// A session left to carry on by itself (host.Away).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Away {
    pub until: Ms,
    /// The next check-in.
    pub next: Ms,
    /// You being there: no "I'm away", and nothing held.
    pub looping: bool,
    /// When its time ran out or it said it's all done; 0 while on.
    pub ended: Ms,
    /// How many things it asked, or asked to do, while you were away.
    pub held: usize,
}

impl Away {
    /// Still checking in.
    pub fn on(&self) -> bool {
        self.ended == 0
    }
}

/// What a host says of its session: the fields of Go's host.Info the
/// fleet reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostInfo {
    pub id: String,
    pub session_id: String,
    pub account: String,
    pub cwd: String,
    /// When the host last moved cwd.
    pub cwd_at: Ms,
    pub left: Vec<Left>,
    pub name: String,
    pub host_pid: i32,
    pub claude_pid: i32,
    /// starting, working, blocked, idle, stopped
    pub state: String,
    pub detail: String,
    pub needs: String,
    pub cost_usd: f64,
    pub limit: Option<Limit>,
    pub retry: Option<Retry>,
    pub error: String,
    pub started_at: Ms,
    pub updated_at: Ms,
    /// When it last went idle: a turn's end. 0 until its first turn.
    pub idle_since: Ms,
    /// When it last heard from its agent, once a turn has heard nothing
    /// for a while with no tool running; 0 otherwise.
    pub stalled: Ms,
    /// What the host can do (host.Proto).
    pub proto: i64,
    /// A clean idle exit: explicit input wakes the saved session.
    pub sleeping: bool,
    /// The machine a session is really on, on rush remote attach's
    /// stand-in for it.
    pub remote: String,
    /// The host went away mid-turn without saying so.
    pub lost: bool,
    /// The agent it runs; Claude Code's kind when the host wrote none.
    pub kind: String,
    /// The profile it was started under.
    pub profile: String,
    /// The config's, as it started with: spawnedBy and the like.
    pub meta: HashMap<String, String>,
    pub background: Vec<HostTask>,
    pub away: Option<Away>,
    pub workspace: Option<Workspace>,
    /// The headings of the decisions its last turn ended with, as a
    /// question card for you; None when it ended with none.
    pub decisions: Option<Vec<String>>,
}

impl HostInfo {
    /// Another machine's session: no folder or process here is its.
    pub fn is_remote(&self) -> bool {
        !self.remote.is_empty()
    }
}

/// What a session's config says that the list reads (host.Config).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostConfig {
    /// The paths /rewind left behind: their conversations are the
    /// session's, not past rows of their own.
    pub branches: Vec<String>,
    /// The profile its agent runs on, when it says.
    pub account: rush_core::adapter::Profile,
    /// Added to its agent's environment, KEY=value.
    pub env: Vec<String>,
}

/// Lists the sessions rush runs: the host module's, which reads their
/// folders and asks their hosts. The fleet only reads what it says.
pub trait Hosts: Send {
    /// Every rush session's info. fresh(path, at) says whether path is
    /// watched and unchanged since listing number at looked at it: a
    /// session's folder unchanged, its info is as it was without a look.
    /// A listing with nothing changed may be the last one again, shared.
    fn list(&mut self, fresh: &dyn Fn(&Path, u64) -> bool, at: u64) -> Arc<Vec<HostInfo>>;
    /// The config session id was started with; None once it's gone.
    fn config(&self, id: &str) -> Option<HostConfig>;
    /// Brings back a session whose host went away mid-turn, in the
    /// background: its next reading shows it working again.
    fn revive(&self, _info: &HostInfo) {}
}

/// No rush sessions: until the host module is wired in.
pub struct NoHosts;

impl Hosts for NoHosts {
    fn list(&mut self, _: &dyn Fn(&Path, u64) -> bool, _: u64) -> Arc<Vec<HostInfo>> {
        Arc::default()
    }
    fn config(&self, _: &str) -> Option<HostConfig> {
        None
    }
}

/// The folder holding one folder per session.
pub fn host_root() -> PathBuf {
    rush_state::dir().join("sessions")
}

/// Where sessions' scratch folders are, one per project.
pub fn temp_root() -> PathBuf {
    rush_state::dir().join("tmp")
}

/// Where a session's agent and everything it runs keep their scratch
/// files. The session's own folder links to it (tmp); one from before the
/// per-project layout has the folder itself there.
pub fn temp_dir(id: &str) -> String {
    let p = host_root().join(id).join("tmp");
    match std::fs::read_link(&p) {
        Ok(r) => r.to_string_lossy().into_owned(),
        Err(_) => p.to_string_lossy().into_owned(),
    }
}

/// Whether p is a session's scratch folder as rush lays them out: the
/// only folders its tidy-ups empty.
pub fn is_temp_dir(p: &str) -> bool {
    let p = crate::worktree::clean(p);
    let path = Path::new(&p);
    path.file_name().is_some_and(|n| n == "tmp")
        || path.parent().and_then(Path::parent) == Some(temp_root().as_path())
}

/// Whether p is in a temp folder, a session's or the system's: somewhere
/// work is scratch, never a project.
pub fn is_temp(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let p = crate::worktree::clean(p);
    let tmp = temp_root();
    if inside(&tmp.to_string_lossy(), &p) || system_temp(&p) {
        return true;
    }
    let root = host_root();
    let root = root.to_string_lossy();
    if !inside(&root, &p) || p == root {
        return false;
    }
    let rest = &p[root.len() + 1..];
    let rest = rest.split_once('/').map_or("", |(_, r)| r);
    rest == "tmp" || rest.starts_with("tmp/")
}

/// Whether clean path p is root or under it.
fn inside(root: &str, p: &str) -> bool {
    p.starts_with(root) && (p.len() == root.len() || p.as_bytes()[root.len()] == b'/')
}

/// Whether p is in the system's temp folders.
fn system_temp(p: &str) -> bool {
    p.contains("/var/folders/") || p.starts_with("/tmp/") || p.starts_with("/private/tmp/")
}

/// What a decisions card needs of you, said briefly.
pub fn needs_decisions(headers: &[String]) -> String {
    if headers.len() == 1 {
        return format!("decide: {}", headers[0]);
    }
    format!("{} decisions", headers.len())
}

/// An away or looping row's say: time left and the next check-in, or
/// once away's over, what's waiting for you and its last words.
pub fn away_detail(a: &Away, last: &str, now: Ms) -> String {
    let held = if a.held > 0 {
        format!(" · {} saved for you", a.held)
    } else {
        String::new()
    };
    if !a.on() {
        let s = format!("✈ back from away{held} · {last}");
        return s.strip_suffix(" · ").map(str::to_string).unwrap_or(s);
    }
    let mut d = if a.looping { "⟳ loop" } else { "✈ away" }.to_string();
    if a.until != 0 {
        let left = a.until - now;
        let mins = left / 60_000;
        d += &format!(" {}h{:02}m more", left / 3_600_000, mins % 60);
    }
    d + " · next check-in " + &clock(a.next) + &held
}

/// A time as Go's Local().Format("15:04") writes it.
pub fn clock(ms: Ms) -> String {
    let t = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r writes only into the tm we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return "00:00".into();
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}
