//! The host keeps a rush-mode session alive outside the rush view.
//!
//! Each session gets one small detached `rush host run <id>` process. It
//! owns the session's agent, run headless through its adapter
//! (rush_core's Driver and Conn: the adapter's reader thread pushes what
//! the session says into one sink, the host's queue), and serves a unix
//! socket: a client that connects is sent what the session has said so
//! far, then everything live, and can send messages, answer permission
//! prompts, interrupt or stop.
//!
//! The socket's protocol (ops in, lines out), config.json, info.json,
//! events.jsonl, exchanges.jsonl, the inbox and the views are Go rush's,
//! byte for byte: a Go UI talks to a Rust host and back
//! (tests/fixtures are what Go writes; gofixtures/gen.sh writes them
//! again).
//!
//! Ported from internal/host (host.go, client.go, hello.go, kind.go,
//! pack.go, reload.go, events.go, views.go, viewacts.go, workspace.go,
//! tmp.go, trim.go, shim.go, exchange_journal.go, inbox.go, title.go and
//! the parts of the rest they need) and the host's commands in cmd/rush
//! (`rush host`, `rush attach`, `rush view`, `rush inbox`, `rush reload`).

pub mod agentmatch;
pub mod agents;
pub mod agtools;
pub mod away;
pub mod cli;
pub mod client;
pub mod decisions;
pub mod events;
pub mod exchange_journal;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod hello;
pub mod hosts;
pub mod inbox;
pub mod json;
pub mod kind;
pub mod left;
pub mod lines;
pub mod longtask;
pub mod name;
pub mod op;
pub mod pack;
pub mod paths;
pub mod reload;
pub mod remind;
pub mod replace;
pub mod revive;
pub mod server;
pub mod shim;
pub mod stuck;
pub mod subtool;
pub mod sys;
pub mod timer;
pub mod title;
pub mod tmp;
pub mod trim;
pub mod viewacts;
pub mod views;
pub mod viewwords;
pub mod wire;
pub mod workspace;

#[cfg(test)]
mod gofiles;
#[cfg(test)]
pub(crate) mod testing;

use std::time::Duration;

pub use client::{
    Client, ClientError, Lister, deliver, ensure, list, read_config, read_info, restart, running,
    spawn,
};
pub use lines::{Decoder, Line, Sent, decode};
pub use op::Op;
pub use paths::{dir, events_path, new_session_id, root, short_id, sock_path};
pub use server::run;
pub use wire::{
    Away, BinStamp, Branch, Config, Held, Info, Left, Limit, PROTO, Retry, Task, Workspace,
};

/// How long a sent message keeps an idle agent up (host.go:1145).
pub const UNANSWERED_FOR: Duration = Duration::from_secs(60);

/// Whether a session's host process (Info.host_pid) is running. A host
/// publishes "stopped" just before it exits, so the state alone can say
/// stopped while the process is still there.
pub fn alive(pid: i64) -> bool {
    if pid <= 0 || pid > i32::MAX as i64 {
        return false;
    }
    // SAFETY: kill with signal 0 only asks whether pid can be signalled.
    let r = unsafe { libc::kill(pid as i32, 0) };
    let ok = r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    // A host rush started before a #reload exec'd it is never reaped, so
    // once it ends it lingers as a zombie that kill(pid, 0) still finds.
    ok && !rush_sys::proc::zombie(pid as i32)
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
