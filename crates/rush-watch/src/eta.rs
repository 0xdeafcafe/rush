//! When a watch's runs should be done, from how long their workflows
//! usually take: the median of their last successful runs on GitHub. A
//! watch asks once for what it waits on (the answers are shared and kept
//! an hour, so watchers of one repository ask once between them), and
//! keeps the end it expects in its Live, for rush to say "~4m left" on
//! its card. Nothing of it goes to the agent. New in the Rust port.

use std::time::Duration;

use rush_state::Time;
use serde::Deserialize;

use crate::gh::{self, nd, time_or_zero};

/// How many of a workflow's last successful runs its usual time is taken
/// from.
pub const USUAL_OF: usize = 10;

/// How long an answer about past runs is used before it's asked again.
const USUAL_FRESH: Duration = Duration::from_secs(60 * 60);

/// A run being waited on: its workflow, when it started, and whether it
/// has finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Started {
    pub workflow: i64,
    pub at: Time,
    pub done: bool,
}

/// The middle of ds (the lower of the two middles for an even count);
/// None for none.
pub fn median(mut ds: Vec<Duration>) -> Option<Duration> {
    if ds.is_empty() {
        return None;
    }
    ds.sort();
    Some(ds[(ds.len() - 1) / 2])
}

/// When the last of runs still going should end: each one's start and its
/// workflow's usual time, as usual says (None when unknown, and that run
/// is left out). None when none still going has a usual time.
pub fn expected_end(runs: &[Started], usual: impl Fn(i64) -> Option<Duration>) -> Option<Time> {
    runs.iter()
        .filter(|r| !r.done && !r.at.is_zero())
        .filter_map(|r| {
            let d = usual(r.workflow)?;
            Some(r.at.add_nanos(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)))
        })
        .max()
}

#[derive(Deserialize)]
struct RunTimes {
    #[serde(default, deserialize_with = "nd")]
    workflow_id: i64,
    #[serde(default, deserialize_with = "nd")]
    status: String,
    #[serde(default, deserialize_with = "time_or_zero")]
    run_started_at: Time,
    #[serde(default, deserialize_with = "time_or_zero")]
    updated_at: Time,
}

#[derive(Deserialize)]
struct RunList {
    #[serde(default, deserialize_with = "nd")]
    workflow_runs: Vec<RunTimes>,
}

/// How long a run took, from its start to its last update; None when
/// either is missing or they're out of order.
fn took(r: &RunTimes) -> Option<Duration> {
    if r.run_started_at.is_zero() || r.updated_at.is_zero() {
        return None;
    }
    let n = r.updated_at.sub_nanos(r.run_started_at);
    (n > 0).then(|| Duration::from_nanos(u64::try_from(n).unwrap_or(u64::MAX)))
}

/// How long workflow (by id) in repo usually takes: the median of its
/// last USUAL_OF successful runs, as a JSON list of runs from GitHub.
pub fn usual_of(body: &[u8]) -> Option<Duration> {
    let l: RunList = rush_state::json::from_slice(body).ok()?;
    median(l.workflow_runs.iter().filter_map(took).collect())
}

/// How long workflow (by id) in repo usually takes, asked of GitHub.
pub fn usual(repo: &str, workflow: i64) -> Option<Duration> {
    let path = format!(
        "repos/{repo}/actions/workflows/{workflow}/runs?status=success&per_page={USUAL_OF}"
    );
    let (b, _) = gh::get(&path, USUAL_FRESH).ok()?;
    usual_of(&b)
}

/// The newest run of each workflow, from a JSON list of runs newest
/// first, as GitHub gives them.
pub fn started_of(body: &[u8]) -> Vec<Started> {
    let Ok(l) = rush_state::json::from_slice::<RunList>(body) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    l.workflow_runs
        .into_iter()
        .filter(|r| seen.insert(r.workflow_id))
        .map(|r| Started {
            workflow: r.workflow_id,
            at: r.run_started_at,
            done: r.status == "completed",
        })
        .collect()
}

/// The runs on commit sha in repo, the newest of each workflow, asked of
/// GitHub (shared with other watchers for a minute).
pub fn started_on(repo: &str, sha: &str) -> Vec<Started> {
    let path = format!("repos/{repo}/actions/runs?head_sha={sha}&per_page=100");
    match gh::get(&path, Duration::from_secs(60)) {
        Ok((b, _)) => started_of(&b),
        Err(_) => Vec::new(),
    }
}
