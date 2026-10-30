//! Agents rush speaks to over the Agent Client Protocol
//! (https://agentclientprotocol.com): JSON-RPC over the agent's stdin and
//! stdout. A [`Session`] starts or resumes one session and turns what the
//! agent says into rush's own events, pushed into the caller's Sink. Port
//! of internal/adapters/acp, vibe, gemini and glm.
//!
//! - `rpc`, `protocol`, `session`, `update`, `serve`, `tasks_ended`: the
//!   protocol and one live session.
//! - `agent`: an agent known only through ACP ([`Agent`]); OpenCode is one.
//! - `kimi*`: Kimi, with its models, saved sessions (Kimi Code 2's wire,
//!   the older CLI's context logs), sign-in, limits and subagents.
//! - `vibe`: Mistral Vibe over vibe-acp. `gemini`: Gemini CLI. `glm`: Z.ai's
//!   GLM through ZCode's ACP bridge.
//! - `history`: the message logs Kimi and Vibe keep, read back in slices.
//!
//! [`register`] makes each known to a registry, as Go's init functions do.

pub mod agent;
pub mod gemini;
pub mod glm;
pub mod history;
mod http;
pub mod kimi;
mod kimi_account;
mod kimi_children;
mod kimi_wire;
pub mod protocol;
pub mod rpc;
mod serve;
pub mod session;
#[cfg(test)]
mod session_tests;
mod tasks_ended;
mod update;
mod util;
pub mod vibe;

pub use agent::{Agent, kimi_agent, opencode};
pub use history::{
    content_parts, end_history_turns, read_messages, read_messages_before, read_messages_tail,
};
pub use kimi::{Kimi, kimi_home};
pub use protocol::{AuthMethod, Info, Mode, PROTOCOL_VERSION};
pub use rpc::{Error, ErrorKind, RpcError};
pub use session::{Options, Session};

use rush_core::adapter::Registry;

/// Registers the ACP agents rush runs: Kimi and OpenCode (Go's acp
/// Known), Vibe, Gemini CLI and GLM.
pub fn register(reg: &Registry) {
    reg.register(Kimi::new());
    reg.register(opencode());
    reg.register(vibe::Vibe::new());
    reg.register(gemini::Gemini::new());
    reg.register(glm::Glm::new());
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
