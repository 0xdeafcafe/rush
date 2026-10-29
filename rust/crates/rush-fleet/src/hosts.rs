//! The rush-hosted sessions the list reads: each `rush host` process's
//! session as its info says it, through a source the host crate gives
//! (rush_host's Lister), so the list needn't know the host's files.
//!
//! Go's fleet reads host.List() itself (internal/fleet/fleet.go:706); the
//! Rust crates can't, as the host depends on the fleet, so the fleet says
//! what it needs here and the host fills it in. The rows these make are
//! still to be ported (fleet.go:1036-1300, hostedAgent and hosted).

use std::path::PathBuf;

use rush_core::time::Ms;

/// One rush-hosted session, as the list needs it of its host's info.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hosted {
    pub id: String,
    pub kind: String,
    pub session_id: String,
    pub account: String,
    pub profile: String,
    pub cwd: String,
    pub name: String,
    /// idle, working, blocked, starting or stopped.
    pub state: String,
    pub detail: String,
    pub needs: String,
    pub error: String,
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    pub host_pid: i64,
    /// The agent's own process, 0 while it has none.
    pub agent_pid: i64,
    /// Its host rests, to be woken by a send.
    pub sleeping: bool,
    /// A session on another machine (Info.remote), and one whose machine
    /// has gone quiet.
    pub remote: String,
    pub lost: bool,
    pub started_at: Ms,
    pub updated_at: Ms,
    pub idle_since: Ms,
    /// When a working turn last heard its agent, set once it's stalled.
    pub stalled: Ms,
    pub cost_usd: f64,
    pub context_tokens: i64,
    pub started_by: String,
    /// The session that spawned it, and the call there that did.
    pub spawned_by: String,
    pub spawn_step: String,
    pub queued: usize,
    pub tasks: i64,
    pub background: usize,
    /// What its open decisions card needs of you, "" with none.
    pub decisions: String,
    /// The branch of its own worktree, "" outside one.
    pub branch: String,
    pub temp_dir: String,
}

/// Where the list reads the hosted sessions from.
pub trait Hosts: Send {
    /// Every hosted session as its info says it now.
    fn list(&mut self) -> Vec<Hosted>;
    /// The folders whose changes mean the listing changed: what to wait on
    /// rather than listing again on a timer.
    fn watched(&self) -> Vec<PathBuf>;
}
