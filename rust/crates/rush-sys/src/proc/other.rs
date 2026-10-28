//! Other systems have no process backend: the process columns stay empty.
//!
//! Ported from internal/proc/proc_other.go:1-27.

use std::time::SystemTime;

use super::{Pid, Proc};

pub(super) fn list() -> Vec<Proc> {
    Vec::new()
}
pub(super) fn fill_usage(_: &mut Proc) {}
pub(super) fn args(_: Pid) -> Vec<String> {
    Vec::new()
}
pub(super) fn env(_: Pid) -> Vec<String> {
    Vec::new()
}
pub(super) fn zombie(_: Pid) -> bool {
    false
}
/// When pid started: unknown here.
pub(super) fn started(_: Pid) -> Option<SystemTime> {
    None
}
