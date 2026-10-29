//! What rush asks of and does to the machine itself: the process table and
//! what each tree of processes costs (`proc`), the battery and free disk
//! (`sysinfo`), ending process trees and making git worktrees (`actions`),
//! ending a process group when its host goes (`pgguard`), waiting for files
//! to grow without polling (`fswait`), following an agent's longer shell
//! calls (`script`), and storing idle transcripts compressed (`squeeze`);
//! running a program with a time limit (`cmd`). Which agent programs have
//! a newer version out is rush-harnessup's, so that users of these don't
//! build a TLS stack.
//!
//! Ported from the Go packages of the same names under `internal/`.

pub mod actions;
pub mod cmd;
pub mod events;
pub mod fswait;
pub mod pgguard;
pub mod proc;
pub mod script;
pub mod shellwords;
pub mod squeeze;
pub mod sysinfo;
pub mod timefmt;

#[cfg(test)]
mod testutil;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
