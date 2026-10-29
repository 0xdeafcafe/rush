//! The Claude Code adapter. See DESIGN.md.
//!
//! The read-only side of the agent list: where Claude Code keeps things
//! (`account`), its live sessions (`sessions`), background jobs (`jobs`),
//! past conversations (`past`), what transcripts cost and say
//! (`transcript`, `pricing`, `spend`), and finding them all (`discover`).
//! Its tools read as rush's tool::Call and Output (`tools`), its messages
//! as rush's events (`neutral`, and `history` for a whole transcript), so
//! nothing outside reads Claude Code's words. `adapter` is all of it as
//! rush_core's Adapter: the list rows are rush-fleet's, read through it.
//!
//! Driving it live: `claude -p` speaking stream-json (`headless`), as
//! rush's Conn (`conn`, with `heard`), its lines for a host (`native`),
//! its daemon's control socket (`daemon`) and the screens and actions on
//! its background sessions (`screen`, `actions`), a session another
//! agent's shell began (`spawn`) and one-off questions (`query`).
//!
//! Beyond the list: the logins rush keeps and switches between
//! (`accounts`, `login`, `home`), their plans and limits (`usage`,
//! `oauth`), settings files and what Settings offers of them (`settings`),
//! commands and skills (`commands`), plugins (`plugins`), what a session
//! reads and what's untidy in it (`memory`), Claude Code's own stats
//! (`stats`), a session's subagent runs and timeline (`subruns`,
//! `timeline`), fork copies (`forkcopy`, `files`), efficiency figures
//! (`efficiency`), summaries (`summary`), and the parts that wire them in
//! (`parts`).

pub mod account;
pub mod accounts;
pub mod actions;
pub mod adapter;
pub mod commands;
pub mod conn;
pub mod daemon;
mod decode;
pub mod discover;
pub mod efficiency;
pub mod files;
pub mod forkcopy;
pub mod headless;
mod heard;
mod history;
pub mod home;
pub mod jobs;
mod lean;
pub mod login;
pub mod memory;
mod message;
pub mod modelname;
mod native;
pub mod neutral;
pub mod oauth;
pub mod parts;
pub mod past;
pub mod plugins;
pub mod pricing;
mod query;
pub mod screen;
pub mod sessions;
pub mod settings;
mod spawn;
pub mod spend;
pub mod stats;
pub mod subruns;
pub mod summary;
pub mod timeline;
pub mod tools;
pub mod transcript;
pub mod usage;

mod json;

pub use account::Account;
pub use adapter::Claude;
pub use neutral::messages;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
