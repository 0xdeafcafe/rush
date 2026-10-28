//! Running gh, and its failures in a few words.
//! Ported from internal/ghpr/branches.go:48 (RunGH), ghpr.go:247 (ghErr)
//! and act.go:84-114 (MergeError, mergeErr).

use std::fmt;
use std::io::ErrorKind;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How running gh went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// gh isn't on the PATH.
    NotFound,
    /// gh ran and failed: `status` as Go words it ("exit status 1",
    /// "signal: killed"), and what it said on stderr.
    Exit { status: String, stderr: String },
    /// Anything else, in words.
    Other(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NotFound => f.write_str("exec: \"gh\": executable file not found in $PATH"),
            RunError::Exit { status, .. } => f.write_str(status),
            RunError::Other(s) => f.write_str(s),
        }
    }
}

/// Runs gh in a folder and gives its output. The real one is [`SystemGh`];
/// tests pass a closure.
pub trait Runner: Send + Sync {
    fn run(&self, dir: &str, args: &[&str]) -> Result<Vec<u8>, RunError>;
}

impl<F> Runner for F
where
    F: Fn(&str, &[&str]) -> Result<Vec<u8>, RunError> + Send + Sync,
{
    fn run(&self, dir: &str, args: &[&str]) -> Result<Vec<u8>, RunError> {
        self(dir, args)
    }
}

/// The gh command on the PATH, killed when it takes longer than `timeout`
/// (Go asks in the background with 30 seconds: branches.go:146).
pub struct SystemGh {
    pub timeout: Duration,
}

impl Default for SystemGh {
    fn default() -> Self {
        SystemGh {
            timeout: Duration::from_secs(30),
        }
    }
}

impl Runner for SystemGh {
    fn run(&self, dir: &str, args: &[&str]) -> Result<Vec<u8>, RunError> {
        let mut cmd = Command::new("gh");
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !dir.is_empty() {
            cmd.current_dir(dir);
        }
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(RunError::NotFound),
            Err(e) => return Err(RunError::Other(e.to_string())),
        };
        // One thread waits out the timeout, blocked on a channel; it kills
        // gh only if the answer hasn't come by then.
        let pid = child.id() as libc::pid_t;
        let (done, wait) = mpsc::channel::<()>();
        let timeout = self.timeout;
        let killer = std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(timeout) {
                // SAFETY: kill only sends a signal to a pid we started and
                // have not yet reaped.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        });
        let out = child.wait_with_output();
        let _ = done.send(());
        let _ = killer.join();
        let out = out.map_err(|e| RunError::Other(e.to_string()))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        let status = match out.status.code() {
            Some(n) => format!("exit status {n}"),
            None => "signal: killed".to_string(),
        };
        Err(RunError::Exit {
            status,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// What asking GitHub can fail with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// gh failed, in a few words (see [`gh_err`]).
    Gh(String),
    /// GitHub refused a merge, said plainly.
    Merge(MergeError),
    /// gh's answer wasn't the JSON expected.
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Gh(s) | Error::Parse(s) => f.write_str(s),
            Error::Merge(m) => f.write_str(&m.why),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Parse(e.to_string())
    }
}

/// A MergeError is GitHub refusing a merge, said plainly. Admin is whether
/// merging as an admin gets past it: the base branch's rules stand in its
/// way, a review or checks it wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeError {
    pub why: String,
    pub admin: bool,
}

/// gh's failure in a few words.
pub fn gh_err(err: RunError) -> Error {
    match &err {
        RunError::NotFound => return Error::Gh("gh isn't installed".into()),
        RunError::Exit { stderr, .. } => {
            let msg = stderr.trim();
            if msg.contains("no git remotes") || msg.contains("none of the git remotes") {
                return Error::Gh("not on GitHub".into());
            }
            if msg.contains("gh auth login") || msg.contains("authentication") {
                return Error::Gh("gh isn't signed in · gh auth login".into());
            }
            if !msg.is_empty() {
                let line = match msg.find('\n') {
                    Some(i) if i > 0 => &msg[..i],
                    _ => msg,
                };
                return Error::Gh(line.to_string());
            }
        }
        RunError::Other(_) => {}
    }
    Error::Gh(err.to_string())
}

/// merge_err reads what gh pr merge said when it failed, all of it: gh puts
/// the way past on the lines after the first.
pub fn merge_err(stderr: &str, err: RunError) -> Error {
    let low = stderr.to_lowercase();
    let refused = |why: &str, admin: bool| {
        Error::Merge(MergeError {
            why: why.to_string(),
            admin,
        })
    };
    if low.contains("rate limit") {
        return refused(
            "GitHub's rate limit is used up: try again once it resets",
            false,
        );
    }
    if low.contains("--admin") || low.contains("base branch policy") {
        return refused(
            "the base branch's rules block it (a review or checks): an admin can merge past them",
            true,
        );
    }
    if low.contains("merge conflict") || (low.contains("not mergeable") && low.contains("conflict"))
    {
        return refused("it conflicts with its base: bring the base in first", false);
    }
    gh_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(stderr: &str) -> RunError {
        RunError::Exit {
            status: "exit status 1".into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn gh_err_words() {
        for (err, want) in [
            (RunError::NotFound, "gh isn't installed"),
            (exit("no git remotes found\n"), "not on GitHub"),
            (
                exit("none of the git remotes configured point to a known GitHub host"),
                "not on GitHub",
            ),
            (
                exit("To get started with GitHub CLI, please run:  gh auth login"),
                "gh isn't signed in · gh auth login",
            ),
            (exit("HTTP 504\nmore\n"), "HTTP 504"),
            (exit(""), "exit status 1"),
            (RunError::Other("exec: not found".into()), "exec: not found"),
        ] {
            assert_eq!(gh_err(err).to_string(), want);
        }
    }

    // A refused merge says why in plain words, all of gh's lines read: the
    // way past its base branch's rules is on the last.
    #[test]
    fn merge_err_words() {
        let policy = "X Pull request o/r#9 is not mergeable: the base branch policy prohibits the merge.\nTo have the pull request merged after all the requirements have been met, add the `--auto` flag.\nTo use administrator privileges to immediately merge the pull request, add the `--admin` flag.\n";
        match merge_err(policy, exit(policy)) {
            Error::Merge(m) => assert!(m.admin, "policy: {m:?}"),
            e => panic!("policy: {e:?}"),
        }
        let rate = "GraphQL: API rate limit exceeded for user ID 1.";
        match merge_err(rate, exit(rate)) {
            Error::Merge(m) => assert!(!m.admin && m.why.contains("rate limit"), "{m:?}"),
            e => panic!("rate limit: {e:?}"),
        }
        let conflict =
            "Pull request is not mergeable: the merge commit cannot be cleanly created (conflict)";
        match merge_err(conflict, exit(conflict)) {
            Error::Merge(m) => assert!(m.why.contains("conflicts"), "{m:?}"),
            e => panic!("conflict: {e:?}"),
        }
        assert_eq!(
            merge_err("boom", exit("boom")),
            Error::Gh("boom".into()),
            "anything else is gh's own words"
        );
    }
}
