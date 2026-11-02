//! One pull request or issue by its address, from any folder: what a link
//! in a conversation says of where it stands. Ported from
//! internal/ghpr/view.go (ParseURL :15, ParseIssueURL :18, parseNumbered
//! :21, PRURL :38, IssueURL :43, View :50, ParseOne :65, Issue :74,
//! ViewIssue :84).

use serde::Deserialize;

use super::{Error, FIELDS, Gh, PR, RawPR, from_raw, gh_err};
use crate::rfc3339::null_default;

/// The repository, owner/name, and number of a pull request's address on
/// GitHub: https://github.com/o/r/pull/7, with or without a tab (/files,
/// /checks) or an anchor after it. None for anything else, an issue, a
/// commit or another site.
pub fn parse_url(u: &str) -> Option<(String, u64)> {
    parse_numbered(u, "pull")
}

/// parse_url for an issue's address: .../issues/7.
pub fn parse_issue_url(u: &str) -> Option<(String, u64)> {
    parse_numbered(u, "issues")
}

/// Reads github.com/<owner>/<name>/<kind>/<number>[/...].
fn parse_numbered(u: &str, kind: &str) -> Option<(String, u64)> {
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
    if parts.len() < 4 || parts[2] != kind || parts[0].is_empty() || parts[1].is_empty() {
        return None;
    }
    let n: u64 = parts[3].parse().ok()?;
    if n == 0 {
        return None;
    }
    Some((format!("{}/{}", parts[0], parts[1]), n))
}

/// The canonical address of pull request n in repo, owner/name.
pub fn pr_url(repo: &str, n: u64) -> String {
    format!("https://github.com/{repo}/pull/{n}")
}

/// The canonical address of issue n in repo, owner/name.
pub fn issue_url(repo: &str, n: u64) -> String {
    format!("https://github.com/{repo}/issues/{n}")
}

/// Reads gh pr view's JSON.
pub fn parse_one(b: &[u8]) -> Result<PR, Error> {
    let raw: RawPR = serde_json::from_slice(b)?;
    Ok(from_raw(raw))
}

/// An issue as GitHub says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// OPEN or CLOSED.
    pub state: String,
    /// Why it's closed: COMPLETED, NOT_PLANNED or DUPLICATE.
    pub reason: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawIssue {
    #[serde(deserialize_with = "null_default")]
    number: u64,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(deserialize_with = "null_default")]
    url: String,
    #[serde(deserialize_with = "null_default")]
    state: String,
    #[serde(rename = "stateReason", deserialize_with = "null_default")]
    state_reason: String,
}

impl Gh {
    /// Asks GitHub for one pull request by its address, from any folder,
    /// through the same gate as look so a page of links never asks too
    /// many at once.
    pub fn view(&self, u: &str) -> Result<PR, Error> {
        let out = {
            let _pass = self.inner.gate.enter();
            self.gh("", &["pr", "view", u, "--json", FIELDS])
        }
        .map_err(gh_err)?;
        parse_one(&out)
    }

    /// Asks GitHub for one issue by its address, from any folder, through
    /// the same gate as view.
    pub fn view_issue(&self, u: &str) -> Result<Issue, Error> {
        let out = {
            let _pass = self.inner.gate.enter();
            self.gh(
                "",
                &[
                    "issue",
                    "view",
                    u,
                    "--json",
                    "number,title,url,state,stateReason",
                ],
            )
        }
        .map_err(gh_err)?;
        let r: RawIssue = serde_json::from_slice(&out)?;
        Ok(Issue {
            number: r.number,
            title: r.title,
            url: r.url,
            state: r.state,
            reason: r.state_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::super::sel;
    use super::*;

    // view_test.go: TestParseURL.
    #[test]
    fn parse_url_reads_github_addresses() {
        let some = |r: &str, n| Some((r.to_string(), n));
        for (u, want) in [
            (
                "https://github.com/acme/terrafied/pull/33",
                some("acme/terrafied", 33),
            ),
            (
                "https://github.com/acme/terrafied/pull/33/files",
                some("acme/terrafied", 33),
            ),
            (
                "https://github.com/acme/terrafied/pull/33#issue-1",
                some("acme/terrafied", 33),
            ),
            (
                "https://www.github.com/acme/terrafied/pull/7",
                some("acme/terrafied", 7),
            ),
            ("https://github.com/acme/terrafied/issues/33", None),
            ("https://github.com/acme/terrafied/pull/x", None),
            ("https://github.com/acme/terrafied/pull/0", None),
            ("https://gitlab.com/acme/terrafied/pull/33", None),
            ("https://github.com/acme/terrafied", None),
            ("file:///Users/me/github.com/acme/terrafied/pull/33", None),
        ] {
            assert_eq!(parse_url(u), want, "parse_url({u:?})");
        }
        assert_eq!(
            parse_issue_url("https://github.com/acme/terrafied/issues/12"),
            some("acme/terrafied", 12)
        );
        assert_eq!(pr_url("a/b", 7), "https://github.com/a/b/pull/7");
        assert_eq!(issue_url("a/b", 7), "https://github.com/a/b/issues/7");
    }

    // view asks gh of the pull request by its address, from no folder in
    // particular, and reads its one answer (TestView).
    #[test]
    fn view_asks_by_address() {
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let a = asked.clone();
        let gh = Gh::new(move |dir: &str, args: &[&str]| {
            let mut v = vec![dir.to_string()];
            v.extend(args.iter().map(|s| s.to_string()));
            *a.lock().unwrap() = v;
            Ok(br#"{"number":33,"title":"Emoji fix","url":"https://github.com/acme/terrafied/pull/33","state":"OPEN",
            "statusCheckRollup":[{"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"}]}"#
                .to_vec())
        });
        let pr = gh
            .view("https://github.com/acme/terrafied/pull/33")
            .unwrap();
        assert!(
            pr.number == 33 && pr.state == "OPEN" && pr.checks.len() == 1,
            "{pr:?}"
        );
        let asked = asked.lock().unwrap();
        assert_eq!(
            asked[..4],
            [
                "",
                "pr",
                "view",
                "https://github.com/acme/terrafied/pull/33"
            ]
        );
    }

    #[test]
    fn view_issue_reads_why_closed() {
        let gh = Gh::new(|_: &str, _: &[&str]| {
            Ok(br#"{"number":4,"title":"t","url":"u","state":"CLOSED","stateReason":"NOT_PLANNED"}"#.to_vec())
        });
        let i = gh.view_issue("https://github.com/a/b/issues/4").unwrap();
        assert_eq!(
            (i.number, i.state.as_str(), i.reason.as_str()),
            (4, "CLOSED", "NOT_PLANNED")
        );
    }

    // TestSel.
    #[test]
    fn sel_names_by_number_or_address() {
        assert_eq!(sel(7, ""), "7");
        assert_eq!(
            sel(7, "https://github.com/a/b/pull/7"),
            "https://github.com/a/b/pull/7"
        );
    }
}
