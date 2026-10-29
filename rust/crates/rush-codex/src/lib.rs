//! The Codex adapter: OpenAI's Codex CLI as rush's, driving `codex
//! app-server`, Codex's own JSON-RPC protocol, rather than ACP, because
//! only it reports the account's rate-limit windows. Port of
//! internal/adapters/codex.
//!
//! Its threads' rollouts are read back as rush's events (`rollout`,
//! `history`), its threads found from them (`discover`, `children`); a
//! live thread runs in an app-server of its own (`rpc`, `conn`), whose
//! items read as rush's tool calls (`items`, `diff`). `quota` reads the
//! account's limits, `accounts` switches its sign-ins, `models` says what
//! a session can start with. `adapter` is all of it as rush_core's
//! Adapter.

mod accounts;
pub mod adapter;
mod children;
mod conn;
mod diff;
mod discover;
mod history;
mod items;
mod json;
mod models;
mod quota;
mod rollout;
mod rpc;

pub use accounts::keep;
pub use adapter::{Codex, key_flags};
pub use conn::{Conn, start};
pub use quota::{quota, use_reset};
pub use rpc::{Closed, RpcError};

/// Codex's kind.
pub const KIND: &str = "codex";

/// Registers Codex with rush's registry, and Codex's own words for a
/// sign-in it can no longer refresh with the failures rush knows (Go's
/// init in codex.go).
pub fn register() {
    rush_core::failure::add(
        rush_core::failure::Kind::Auth,
        &["refresh_token_reused", "refresh token", "sign in again"],
    );
    rush_core::adapter::global().register(Codex::new());
}
