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

pub mod account;
pub mod actions;
pub mod adapter;
pub mod conn;
pub mod daemon;
mod decode;
pub mod discover;
pub mod headless;
mod heard;
mod history;
pub mod jobs;
mod message;
mod native;
pub mod neutral;
pub mod past;
pub mod pricing;
mod query;
pub mod screen;
pub mod sessions;
mod spawn;
pub mod spend;
pub mod subruns;
pub mod tools;
pub mod transcript;

mod json;

pub use account::Account;
pub use adapter::Claude;
pub use neutral::messages;
