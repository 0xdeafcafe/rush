//! Driving a pull request to mergeable for an agent: State is everything
//! between it and merge, in the order to do it; follow keeps asking GitHub
//! (cheaply, through rush-watch's shared answers) and wakes the agent's
//! session only with news it can act on. Who may give the agent
//! instructions is decided here, live, never by the agent.
//!
//! Ported from internal/prdrive and cmd/rush/pr.go, prfollow.go.

pub mod cli;
pub mod follow;
pub mod github;
pub mod playbook;
pub mod reply;
pub mod state;
pub mod trust;

pub use cli::{Env, FilesHost, Host, PR_USAGE, help, pr_cmd, pr_cmd_in};
pub use follow::{
    Entry, Options, digest, follow, follow_all, following, hold, holding, look, message, start,
    stop, whats_new,
};
pub use github::{Actor, Check, Comment, Raw, Review, Thread, fetch};
pub use playbook::{PLAYBOOK, PROMPT};
pub use reply::{may_reply, reply, resolve};
pub use state::{
    CLOSED, FORK, Item, MERGEABLE, MERGED, READY, State, WAITING, WORK, analyse, analyse_at, gist,
    keys, render,
};
pub use trust::{REVIEWERS, Reviewer, Trust};

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
