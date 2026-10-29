//! The agent list's rows, whichever agents run them: each adapter's
//! sessions, read through the registry, made into rows with what they cost,
//! where they work and what their processes use; and what each row's state
//! means; the repositories, worktrees, scratch and processes they leave
//! (folder, worktree, disk, machine). Port of internal/fleet (fleet.go, past.go, stall.go, stuck.go),
//! the parts the list uses so far. See DESIGN.md "Contracts".

pub mod disk;
pub mod folder;
mod git;
mod loader;
pub mod machine;
mod predicates;
pub mod reap;
pub mod worktree;

pub use disk::within;
pub use git::git_at;
pub use loader::{Loader, is_print};
pub use predicates::*;

#[cfg(test)]
mod tests;
