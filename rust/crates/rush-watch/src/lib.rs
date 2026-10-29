//! Waiting on things for agents, so they don't poll: rush asks on their
//! behalf, cheaply, and says once what happened. A pull request's checks
//! are asked of GitHub through the host's gh, signed in as the user. It's
//! files and flock under rush's folder/watch, no daemon: every agent
//! watching the same pull request shares one answer, and asking again
//! sends the last ETag, so an answer that hasn't changed (304) costs none
//! of the user's rate limit.
//!
//! `rush watch pr … --until done|fail|first|each` ends 0 when everything
//! passed or it merged, 1 when a check failed, 2 when it timed out, 3 when
//! closed or GitHub couldn't be asked, as Go's does.
//!
//! Ported from internal/watch and cmd/rush/watch.go.

pub mod cli;
pub mod gh;
pub mod go;
pub mod goflag;
pub mod hook;
pub mod live;
pub mod pr;
pub mod stop;

pub use cli::{Env, WATCH_USAGE, bundled_on, watch_cmd, watch_cmd_in};
pub use gh::{Error, get, raw, remaining};
pub use hook::{PROMPT, redirect};
pub use live::{Live, live_for, task_id};
pub use pr::{
    Check, EXIT_ERROR, EXIT_FAILED, EXIT_PASSED, EXIT_TIMED_OUT, Options, PR, Until, fetch,
    interval, log_errors, resolve, summary, watch_pr,
};
