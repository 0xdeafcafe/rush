//! Reading a pull request's conversation, and acting on it: approving,
//! asking for changes, commenting, merging.
//!
//! Ported from internal/ghpr/act.go.

use rush_core::time::Ms;
use serde::Deserialize;

use super::{Error, Gh, RunError, Who, gh_err, merge_err};
use crate::rfc3339::{de_time, null_default};

/// Comment is something said on a pull request's conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comment {
    pub who: String,
    pub body: String,
    pub at: Ms,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawComments {
    #[serde(deserialize_with = "null_default")]
    comments: Vec<RawComment>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawComment {
    #[serde(deserialize_with = "null_default")]
    author: Who,
    #[serde(deserialize_with = "null_default")]
    body: String,
    #[serde(rename = "createdAt", deserialize_with = "de_time")]
    created_at: Ms,
}

pub(crate) fn parse_comments(b: &[u8]) -> Result<Vec<Comment>, Error> {
    let raw: RawComments = serde_json::from_slice(b)?;
    Ok(raw
        .comments
        .into_iter()
        .map(|c| Comment {
            who: c.author.words(),
            body: c.body,
            at: c.created_at,
        })
        .collect())
}

impl Gh {
    /// comments are a pull request's conversation, oldest first.
    pub fn comments(&self, root: &str, n: u64) -> Result<Vec<Comment>, Error> {
        let n = n.to_string();
        let out = self
            .gh(root, &["pr", "view", &n, "--json", "comments"])
            .map_err(gh_err)?;
        parse_comments(&out)
    }

    /// approve approves pull request n, saying body with it when there is
    /// one.
    pub fn approve(&self, root: &str, n: u64, body: &str) -> Result<(), Error> {
        let n = n.to_string();
        let mut args = vec!["pr", "review", &n, "--approve"];
        if !body.is_empty() {
            args.extend(["--body", body]);
        }
        self.act(root, &args)
    }

    /// request_changes asks for changes to pull request n, saying why.
    pub fn request_changes(&self, root: &str, n: u64, body: &str) -> Result<(), Error> {
        let n = n.to_string();
        self.act(
            root,
            &["pr", "review", &n, "--request-changes", "--body", body],
        )
    }

    /// say comments body on pull request n.
    pub fn say(&self, root: &str, n: u64, body: &str) -> Result<(), Error> {
        let n = n.to_string();
        self.act(root, &["pr", "comment", &n, "--body", body])
    }

    /// merge merges pull request n by how: squash, merge or rebase; admin
    /// merges past its base branch's rules, as gh pr merge --admin does.
    pub fn merge(&self, root: &str, n: u64, how: &str, admin: bool) -> Result<(), Error> {
        let n = n.to_string();
        let how = format!("--{how}");
        let mut args = vec!["pr", "merge", &n, &how];
        if admin {
            args.push("--admin");
        }
        self.gh(root, &args).map(|_| ()).map_err(|e| match &e {
            RunError::Exit { stderr, .. } => merge_err(&stderr.clone(), e),
            _ => gh_err(e),
        })
    }

    fn act(&self, root: &str, args: &[&str]) -> Result<(), Error> {
        self.gh(root, args).map(|_| ()).map_err(gh_err)
    }
}
