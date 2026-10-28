//! The Claude Code adapter. See DESIGN.md.
//!
//! The read-only side of the agent list: where Claude Code keeps things
//! (`account`), its live sessions (`sessions`), background jobs (`jobs`),
//! past conversations (`past`), what transcripts cost and say
//! (`transcript`, `pricing`, `spend`), finding them all (`discover`), and
//! the list rows made of them (`fleet`). Its tools read as rush's
//! tool::Call and Output (`tools`), and its messages as rush's events
//! (`neutral`), so nothing outside reads Claude Code's words.

pub mod account;
mod decode;
pub mod discover;
pub mod fleet;
pub mod jobs;
mod message;
pub mod neutral;
pub mod past;
pub mod pricing;
pub mod sessions;
pub mod spend;
pub mod tools;
pub mod transcript;

mod json;

pub use account::Account;
pub use fleet::load;
pub use neutral::messages;
