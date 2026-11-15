//! What a GitHub address points at, read off the address alone (a pull
//! request, an issue, an Actions run, a commit, a repository), and an
//! Actions run as GitHub says it: what a link's popover shows and offers.
//! Rush-only: Go's linkinfo knew pull requests and issues alone
//! (internal/ghpr/view.go:15-36 is parse_url's shape, followed here).

use serde::Deserialize;

use super::{Error, Gh, gh_err, parse_issue_url, parse_url};
use crate::rfc3339::null_default;

/// What a GitHub address points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhLink {
    Pr {
        repo: String,
        n: u64,
    },
    Issue {
        repo: String,
        n: u64,
    },
    /// An Actions run, and the job the address names when it names one.
    Run {
        repo: String,
        id: u64,
        job: u64,
    },
    Commit {
        repo: String,
        sha: String,
    },
    Repo {
        repo: String,
    },
}

impl GhLink {
    /// The repository, owner/name.
    pub fn repo(&self) -> &str {
        match self {
            GhLink::Pr { repo, .. }
            | GhLink::Issue { repo, .. }
            | GhLink::Run { repo, .. }
            | GhLink::Commit { repo, .. }
            | GhLink::Repo { repo } => repo,
        }
    }
}

/// The first parts of github.com's own pages, never a repository's owner.
const NOT_OWNERS: &[&str] = &[
    "about",
    "apps",
    "collections",
    "customer-stories",
    "enterprise",
    "explore",
    "features",
    "login",
    "marketplace",
    "new",
    "notifications",
    "orgs",
    "organizations",
    "pricing",
    "pulls",
    "issues",
    "search",
    "settings",
    "sponsors",
    "topics",
    "trending",
];

/// The path parts of a github.com address, its query and anchor left off;
/// None for any other site.
fn github_parts(u: &str) -> Option<Vec<&str>> {
    let (scheme, rest) = u.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return None;
    }
    if NOT_OWNERS.contains(&parts[0].to_ascii_lowercase().as_str()) {
        return None;
    }
    Some(parts)
}

/// What a GitHub address points at; None for one rush doesn't know the
/// kind of (a file, a wiki page) or another site.
pub fn github_link(u: &str) -> Option<GhLink> {
    if let Some((repo, n)) = parse_url(u) {
        return Some(GhLink::Pr { repo, n });
    }
    if let Some((repo, n)) = parse_issue_url(u) {
        return Some(GhLink::Issue { repo, n });
    }
    let parts = github_parts(u)?;
    let repo = format!("{}/{}", parts[0], parts[1]);
    match &parts[2..] {
        [] => Some(GhLink::Repo { repo }),
        ["actions", "runs", id, rest @ ..] => {
            let id: u64 = id.parse().ok().filter(|&n| n > 0)?;
            let job = match rest {
                ["job", j, ..] => j.parse().unwrap_or(0),
                _ => 0,
            };
            Some(GhLink::Run { repo, id, job })
        }
        ["commit", sha, ..]
            if (7..=40).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            Some(GhLink::Commit {
                repo,
                sha: sha.to_ascii_lowercase(),
            })
        }
        _ => None,
    }
}

/// parse_url for an Actions run's address: .../actions/runs/7[/job/9].
pub fn parse_run_url(u: &str) -> Option<(String, u64)> {
    match github_link(u)? {
        GhLink::Run { repo, id, .. } => Some((repo, id)),
        _ => None,
    }
}

/// The canonical address of Actions run id in repo, owner/name.
pub fn run_url(repo: &str, id: u64) -> String {
    format!("https://github.com/{repo}/actions/runs/{id}")
}

/// One job of an Actions run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub name: String,
    /// queued, in_progress or completed.
    pub status: String,
    /// Once completed: success, failure, cancelled, skipped…
    pub conclusion: String,
    /// Its page, with its log.
    pub url: String,
}

/// An Actions run as GitHub says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub id: u64,
    /// Its repository, owner/name.
    pub repo: String,
    /// What it's named for: the commit's or the pull request's title.
    pub title: String,
    pub workflow: String,
    pub branch: String,
    /// queued, in_progress or completed.
    pub status: String,
    /// Once completed: success, failure, cancelled…
    pub conclusion: String,
    pub url: String,
    pub jobs: Vec<Job>,
}

impl Run {
    /// Its status in a word: passed, failed, cancelled, running, queued.
    pub fn word(&self) -> &'static str {
        if self.status != "completed" {
            return if self.status == "queued" || self.status == "waiting" {
                "queued"
            } else {
                "running"
            };
        }
        match self.conclusion.as_str() {
            "success" => "passed",
            "failure" | "startup_failure" | "timed_out" => "failed",
            "cancelled" => "cancelled",
            "skipped" => "skipped",
            "action_required" => "waiting",
            _ => "done",
        }
    }

    /// Its first job that failed.
    pub fn failed_job(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| {
            matches!(
                j.conclusion.as_str(),
                "failure" | "timed_out" | "startup_failure"
            )
        })
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawJob {
    #[serde(rename = "databaseId", deserialize_with = "null_default")]
    id: u64,
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(deserialize_with = "null_default")]
    conclusion: String,
    #[serde(deserialize_with = "null_default")]
    url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRun {
    #[serde(rename = "databaseId", deserialize_with = "null_default")]
    id: u64,
    #[serde(rename = "displayTitle", deserialize_with = "null_default")]
    title: String,
    #[serde(rename = "workflowName", deserialize_with = "null_default")]
    workflow: String,
    #[serde(rename = "headBranch", deserialize_with = "null_default")]
    branch: String,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(deserialize_with = "null_default")]
    conclusion: String,
    #[serde(deserialize_with = "null_default")]
    url: String,
    #[serde(deserialize_with = "null_default")]
    jobs: Vec<RawJob>,
}

/// Reads gh run view's JSON, for the run in repo.
pub fn parse_run(repo: &str, b: &[u8]) -> Result<Run, Error> {
    let r: RawRun = serde_json::from_slice(b)?;
    Ok(Run {
        id: r.id,
        repo: repo.to_string(),
        title: r.title,
        workflow: r.workflow,
        branch: r.branch,
        status: r.status.to_lowercase(),
        conclusion: r.conclusion.to_lowercase(),
        url: r.url,
        jobs: r
            .jobs
            .into_iter()
            .map(|j| Job {
                id: j.id,
                name: j.name,
                status: j.status.to_lowercase(),
                conclusion: j.conclusion.to_lowercase(),
                url: j.url,
            })
            .collect(),
    })
}

impl Gh {
    /// Asks GitHub for one Actions run by its address, from any folder,
    /// through the same gate as view.
    pub fn view_run(&self, u: &str) -> Result<Run, Error> {
        let Some((repo, id)) = parse_run_url(u) else {
            return Err(Error::Gh(format!("{u} isn't an Actions run")));
        };
        let id_s = id.to_string();
        let out = {
            let _pass = self.inner.gate.enter();
            self.gh(
                "",
                &[
                    "run",
                    "view",
                    &id_s,
                    "-R",
                    &repo,
                    "--json",
                    "databaseId,displayTitle,workflowName,headBranch,status,conclusion,url,jobs",
                ],
            )
        }
        .map_err(gh_err)?;
        parse_run(&repo, &out)
    }

    /// Runs an Actions run's jobs again: only those that failed, or all.
    pub fn rerun(&self, repo: &str, id: u64, failed: bool) -> Result<(), Error> {
        let id = id.to_string();
        let mut args = vec!["run", "rerun", &id, "-R", repo];
        if failed {
            args.push("--failed");
        }
        self.gh("", &args).map(|_| ()).map_err(gh_err)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn github_link_reads_each_kind() {
        let s = |x: &str| x.to_string();
        for (u, want) in [
            (
                "https://github.com/o/r/pull/7/files",
                Some(GhLink::Pr {
                    repo: s("o/r"),
                    n: 7,
                }),
            ),
            (
                "https://github.com/o/r/issues/4#issuecomment-1",
                Some(GhLink::Issue {
                    repo: s("o/r"),
                    n: 4,
                }),
            ),
            (
                "https://github.com/o/r/actions/runs/123",
                Some(GhLink::Run {
                    repo: s("o/r"),
                    id: 123,
                    job: 0,
                }),
            ),
            (
                "https://github.com/o/r/actions/runs/123/job/456",
                Some(GhLink::Run {
                    repo: s("o/r"),
                    id: 123,
                    job: 456,
                }),
            ),
            (
                "https://github.com/o/r/commit/ABCDEF1234",
                Some(GhLink::Commit {
                    repo: s("o/r"),
                    sha: s("abcdef1234"),
                }),
            ),
            (
                "https://github.com/o/r",
                Some(GhLink::Repo { repo: s("o/r") }),
            ),
            (
                "https://www.github.com/o/r/",
                Some(GhLink::Repo { repo: s("o/r") }),
            ),
            ("https://github.com/o/r/commit/xyz", None),
            ("https://github.com/o/r/blob/main/a.rs", None),
            ("https://github.com/o/r/actions/runs/0", None),
            ("https://github.com/settings/tokens", None),
            ("https://github.com/o", None),
            ("https://gitlab.com/o/r", None),
            ("file:///github.com/o/r", None),
        ] {
            assert_eq!(github_link(u), want, "github_link({u:?})");
        }
        assert_eq!(run_url("o/r", 9), "https://github.com/o/r/actions/runs/9");
    }

    #[test]
    fn view_run_asks_by_id_and_reads_its_jobs() {
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let a = asked.clone();
        let gh = Gh::new(move |_: &str, args: &[&str]| {
            *a.lock().unwrap() = args.iter().map(|s| s.to_string()).collect();
            Ok(br#"{"databaseId":123,"displayTitle":"Fix it","workflowName":"CI","headBranch":"fix","status":"COMPLETED","conclusion":"FAILURE",
            "url":"https://github.com/o/r/actions/runs/123",
            "jobs":[{"databaseId":1,"name":"lint","status":"completed","conclusion":"success","url":"u1"},
                    {"databaseId":2,"name":"test","status":"completed","conclusion":"failure","url":"u2"}]}"#
                .to_vec())
        });
        let r = gh
            .view_run("https://github.com/o/r/actions/runs/123/job/2")
            .unwrap();
        assert_eq!(
            asked.lock().unwrap()[..5],
            ["run", "view", "123", "-R", "o/r"]
        );
        assert_eq!((r.word(), r.repo.as_str()), ("failed", "o/r"));
        assert_eq!(r.failed_job().map(|j| j.name.as_str()), Some("test"));
    }

    #[test]
    fn a_runs_word() {
        let run = |status: &str, conclusion: &str| Run {
            status: status.into(),
            conclusion: conclusion.into(),
            ..Run::default()
        };
        for (r, want) in [
            (run("completed", "success"), "passed"),
            (run("completed", "failure"), "failed"),
            (run("completed", "cancelled"), "cancelled"),
            (run("in_progress", ""), "running"),
            (run("queued", ""), "queued"),
        ] {
            assert_eq!(r.word(), want);
        }
    }
}
