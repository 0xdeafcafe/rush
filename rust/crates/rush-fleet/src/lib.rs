//! The agent list's rows, whichever agents run them: each adapter's
//! sessions read through the registry, rush's own sessions through the
//! host module's Hosts, made into rows with what you've set on them, what
//! they cost, where they work and what their processes use; what each
//! row's state means; the machine's processes; each worktree and whether
//! binning it would lose anything; temp work and what agents left behind.
//! Port of internal/fleet. See DESIGN.md "Contracts".

mod folder;
mod git;
mod hosted;
mod hostfiles;
mod left;
mod loader;
mod logins;
mod machine;
mod pr;
mod predicates;
mod spawn;
mod temp;
mod verdict;
mod watch;
mod worktree;

pub use folder::{Commit, Folder, GitState, check_folder, folder_wants, git_state, short_remote};
pub use git::git_at;
pub use hosted::{
    Away, HostConfig, HostInfo, HostTask, Hosts, Limit, NoHosts, Retry, away_detail, clock,
    host_root, is_temp, is_temp_dir, needs_decisions, temp_dir, temp_root, use_hosts,
};
pub use hostfiles::InfoFiles;
pub use left::{
    LeftItem, SCRATCH_IDLE, Scratch, ScratchItem, clear_scratch, find_scratch, look_left,
    remove_left, stale_scratch,
};
pub use loader::{Handle, Loader, ProcSource, STUCK_NEEDS, Snapshot, System, is_print};
pub use logins::{
    AccountView, Link, LoginView, chain, freshest, left as window_left, next_login, reading_key,
    urgency, week,
};
pub use machine::{
    Leftover, Machine, ProcRow, Reaper, Role, machine_rows, orphan_label, shell_cmd,
};
pub use pr::{as_linked, find_pr, from_linked, from_status};
pub use predicates::*;
pub use temp::{
    TempSize, TempSizes, clean_stale_temp, clean_temp, disk_usage, disk_usage_background,
    remove_temp_entry, temp_dirs,
};
pub use verdict::{
    MADE_BY_CLAUDE, MADE_BY_OTHER, MADE_BY_RUSH, MADE_BY_SUBAGENT, Verdict, VerdictKind,
    fast_forward, is_own_worktree, landed_branches, made_by, settle,
};
pub use worktree::{
    DRY_RUN, Worktree, dry_run_err, find_worktrees, main_checkout, remove_worktree, sub_worktree,
    sub_worktrees, tidy_worktree, transcript_cwd,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_disk;
#[cfg(test)]
mod tests_git;
#[cfg(test)]
mod tests_logins;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
