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

pub mod account;
pub mod adapter;
mod decode;
pub mod discover;
pub mod files;
pub mod forkcopy;
mod history;
pub mod jobs;
mod message;
pub mod modelname;
pub mod neutral;
pub mod past;
pub mod pricing;
pub mod sessions;
pub mod settings;
pub mod spend;
pub mod stats;
pub mod tools;
pub mod transcript;

mod json;

pub use account::Account;
pub use adapter::Claude;
pub use neutral::messages;
