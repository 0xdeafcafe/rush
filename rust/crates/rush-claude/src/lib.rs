//! The Claude Code adapter. See DESIGN.md.
//!
//! The read-only side of the agent list: where Claude Code keeps things
//! (`account`), its live sessions (`sessions`), background jobs (`jobs`),
//! past conversations (`past`), what transcripts cost and say
//! (`transcript`, `pricing`, `doing`, `spend`), finding them all
//! (`discover`), and the list rows made of them (`fleet`).

pub mod account;
pub mod decode;
pub mod discover;
pub mod doing;
pub mod fleet;
pub mod jobs;
pub mod past;
pub mod pricing;
pub mod proc;
pub mod sessions;
pub mod spend;
pub mod transcript;

mod json;

pub use account::Account;
pub use decode::decode_message;
pub use fleet::load;
