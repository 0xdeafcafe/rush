//! The gate: sharing the machine between the intensive programs (tsc, go,
//! cargo…) rush's agents run, as docs/gate-sharing.md has it. A run waits
//! only for memory, fairly by agent, and its holder lowers its priority as
//! it ages and watches it; past the gate, the guard stops the largest
//! program any agent started when the machine is all but out of memory.
//! It's files and flock under rush's folder's gate, no daemon, the same
//! files as Go rush's: either rush's runs wait on the other's.
//!
//! `rush gate run` is what agents and their shims call, so [`cli`] keeps
//! Go's words, output and exit codes. What the gate asks of sessions,
//! plugins and the agent registry goes through [`host::Host`].
//!
//! Ported from internal/gate, internal/bundled/gate (the settings and
//! shims) and cmd/rush/gate.go.

pub mod cli;
mod exit;
pub mod gate;
pub mod godur;
pub mod guard;
pub mod heavy;
pub mod hook;
pub mod host;
pub mod node;
pub mod plugin;
pub mod rules;
pub mod share;
pub mod sys;
pub mod watch;

pub use cli::{Ctx, GATE_USAGE, gate_cmd, gate_hold, gate_hook, gate_status};
pub use gate::{AcquireError, Ask, Cancel, Gate, HELD_ENV, Run, Wait, Waiter, bin_dir, root};
pub use guard::{DEFAULT_FLOOR, Guard, Victim};
pub use heavy::{heavy, worker_env};
pub use hook::{gated, here_doc, unwrap, wrap};
pub use host::{FileHost, Host};
pub use node::node_name;
pub use plugin::Plugin;
pub use rules::{Rule, Rules, parse_floor, parse_reserve, parse_rules, parse_size, size};
pub use share::{Act, Look, State, Ticket, judge, nice_at};
pub use sys::{Machine, Real};
pub use watch::Watch;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
