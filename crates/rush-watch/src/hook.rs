//! What the watch plugin tells agents, and the hook that turns an agent's
//! polling of a pull request's checks into rush watch pr.
//! Ported from internal/watch/hook.go.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::go::{duration, truncate};
use crate::pr::Until;

/// What the watch plugin tells agents: wait on a pull request's checks
/// through rush, which asks GitHub for them, rather than poll it.
pub const PROMPT: &str = "Never wait by polling (sleep loops, re-running gh pr checks or gh run view, gh run watch, scheduled wakeups, a monitor on a polling script): to wait on a pull request's checks, run rush watch pr [<n>|owner/name#<n>|<url>] --until done|fail|first|each [--timeout 25m]; to wait on a GitHub Actions run with no pull request (a pushed branch or commit), run rush watch run [<branch>|<sha>|<run id>] [--workflow ci.yml] with the same flags (nothing: this checkout's commit). It waits for you at no cost and prints once, briefly, what happened: the counts, and each failed check with the log lines that say why. It exits 0 when all passed or it merged, 1 when a check failed, 2 when it timed out (run it again), 3 when closed, cancelled or GitHub couldn't be asked. Run it as a background command where you can, so it wakes you when it ends; --until each prints a line as each check finishes, for a monitor. rush watch help says more.";

/// What an agent's shell command that polls GitHub for a pull request's
/// checks becomes, so that rush waits for it instead: gh pr checks
/// --watch, which prints every check again every few seconds, is
/// rewritten to rush watch pr; a sleep that then asks GitHub is turned
/// back, with why. background and timeout are the call's own (a
/// foreground call is killed at its timeout, so the watch ends before
/// it); exe is rush. Both are "" for any other command.
pub fn redirect(command: &str, background: bool, timeout: Duration, exe: &str) -> (String, String) {
    let rewritten = rewrite_checks(command, background, timeout, exe);
    if !rewritten.is_empty() {
        return (rewritten, String::new());
    }
    let rewritten = rewrite_run_watch(command, background, timeout, exe);
    if !rewritten.is_empty() {
        return (rewritten, String::new());
    }
    // gh run watch asks GitHub every 3s for as long as the run takes:
    // turned back even when it isn't alone, as a few at once spend the
    // user's rate limit.
    if RUN_WATCH.is_match(command) || (polls(command) && ASKS_FOR_RUNS.is_match(command)) {
        return (String::new(), RUN_DENY.into());
    }
    if polls(command) {
        return (
            String::new(),
            "rush waits on a pull request's checks for you, so don't poll GitHub: run \
             rush watch pr <n> --until done (or fail, first, each) as a background command; \
             it prints once what happened and wakes you when it ends. rush watch help says more."
                .into(),
        );
    }
    (String::new(), String::new())
}

/// Why a poll of a GitHub Actions run is turned back.
const RUN_DENY: &str = "rush waits on GitHub Actions runs for you, so don't poll GitHub \
     (gh run watch asks every 3s, and a few at once spend the rate limit): run \
     rush watch run [<branch>|<sha>|<run id>] --until done (or fail, first, each) as a \
     background command; with nothing it watches this checkout's commit. It prints once \
     what happened and wakes you when it ends. rush watch help says more.";

static RUN_WATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)gh[\t\n\x0c\r ]+run[\t\n\x0c\r ]+watch(?-u:\b)").unwrap()
});
static ASKS_FOR_RUNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)gh[\t\n\x0c\r ]+(run[\t\n\x0c\r ]+(list|view|watch)|api[\t\n\x0c\r ]+[^\t\n\x0c\r ]*actions/(runs|jobs))").unwrap()
});

/// A watch's --timeout: short of the call's own timeout, so it says how
/// far it got before it's killed.
fn limit(background: bool, timeout: Duration) -> String {
    let mut limit = Duration::from_secs(45 * 60);
    if !background {
        let timeout = if timeout.is_zero() {
            Duration::from_secs(2 * 60)
        } else {
            timeout
        };
        limit = timeout
            .saturating_sub(Duration::from_secs(15))
            .max(Duration::from_secs(30));
    }
    duration(truncate(limit, Duration::from_secs(1)))
}

/// command as rush watch run, when it is gh run watch alone, perhaps after
/// a cd.
fn rewrite_run_watch(command: &str, background: bool, timeout: Duration, exe: &str) -> String {
    if exe.is_empty() {
        return String::new();
    }
    let (mut cd, mut rest) = (String::new(), command.trim());
    if let Some((first, after)) = rest.split_once("&&")
        && first.trim().starts_with("cd ")
    {
        cd = format!("{} && ", first.trim());
        rest = after.trim();
    }
    if rest.contains(|c| "|;&<>`$()".contains(c)) {
        return String::new();
    }
    let w: Vec<&str> = rest.split_whitespace().collect();
    if w.len() < 3 || w[0] != "gh" || w[1] != "run" || w[2] != "watch" {
        return String::new();
    }
    let (mut id, mut repo) = ("", "");
    let mut i = 3;
    while i < w.len() {
        let a = w[i];
        if a == "--exit-status" || a == "--compact" {
            // rush ends 1 when a job failed, and prints only what matters
        } else if a == "-i" || a == "--interval" {
            i += 1; // rush picks how often
        } else if a.starts_with("--interval=") {
        } else if a == "-R" || a == "--repo" {
            i += 1;
            repo = w.get(i).copied().unwrap_or("");
        } else if let Some(r) = a.strip_prefix("--repo=") {
            repo = r;
        } else if a.starts_with('-') {
            return String::new(); // a flag rush doesn't do
        } else if id.is_empty() && a.bytes().all(|b| b.is_ascii_digit()) {
            id = a;
        } else {
            return String::new();
        }
        i += 1;
    }
    let mut args = vec!["watch".to_string(), "run".to_string()];
    match (repo.is_empty(), id.is_empty()) {
        (true, true) => {}
        (true, false) => args.push(id.to_string()),
        (false, false) => args.push(format!("{repo}@{id}")),
        // Another repository's current branch isn't anything rush knows.
        (false, true) => return String::new(),
    }
    args.push("--until".into());
    args.push(Until::Done.as_str().into());
    args.push("--timeout".into());
    args.push(limit(background, timeout));
    let args: Vec<String> = args.iter().map(|a| quote(a)).collect();
    format!(
        "{cd}echo 'rush: watching through rush watch run, which waits for you and prints once'; {} {}",
        quote(exe),
        args.join(" ")
    )
}

/// command as rush watch pr, when it is gh pr checks --watch alone,
/// perhaps after a cd.
fn rewrite_checks(command: &str, background: bool, timeout: Duration, exe: &str) -> String {
    if exe.is_empty() {
        return String::new();
    }
    let (mut cd, mut rest) = (String::new(), command.trim());
    if let Some((first, after)) = rest.split_once("&&")
        && first.trim().starts_with("cd ")
    {
        cd = format!("{} && ", first.trim());
        rest = after.trim();
    }
    if rest.contains(|c| "|;&<>`$()".contains(c)) {
        return String::new();
    }
    let w: Vec<&str> = rest.split_whitespace().collect();
    if w.len() < 4 || w[0] != "gh" || w[1] != "pr" || w[2] != "checks" {
        return String::new();
    }
    let (mut until, mut pr, mut watching) = (Until::Done, "", false);
    let mut i = 3;
    while i < w.len() {
        let a = w[i];
        if a == "--watch" {
            watching = true;
        } else if a == "--fail-fast" {
            until = Until::Fail;
        } else if a == "--required" {
        } else if a == "-i" || a == "--interval" {
            i += 1; // rush picks how often
        } else if a.starts_with("--interval=") {
        } else if a.starts_with('-') {
            return String::new(); // a flag rush doesn't do: --json, --web…
        } else if pr.is_empty() {
            pr = a;
        } else {
            return String::new();
        }
        i += 1;
    }
    if !watching {
        return String::new();
    }
    let mut args = vec!["watch".to_string(), "pr".to_string()];
    if !pr.is_empty() {
        args.push(pr.to_string());
    }
    args.push("--until".into());
    args.push(until.as_str().into());
    args.push("--timeout".into());
    args.push(limit(background, timeout));
    let args: Vec<String> = args.iter().map(|a| quote(a)).collect();
    format!(
        "{cd}echo 'rush: watching through rush watch pr, which waits for you and prints once'; {} {}",
        quote(exe),
        args.join(" ")
    )
}

static SLEEPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[\t\n\x0c\r ;&|(])sleep[\t\n\x0c\r ]+([0-9]+)").unwrap());
static LOOPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[\t\n\x0c\r ;&|(])(while|until|for)[\t\n\x0c\r ]").unwrap());
static ASKS_FOR_CI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)gh[\t\n\x0c\r ]+(pr[\t\n\x0c\r ]+(checks|view|status)|run[\t\n\x0c\r ]+(list|view|watch)|api[\t\n\x0c\r ]+[^\t\n\x0c\r ]*(check-runs|/status|actions/runs))").unwrap()
});

/// Whether command waits a while, or in a loop, and asks GitHub how a pull
/// request's checks are going.
fn polls(command: &str) -> bool {
    if !ASKS_FOR_CI.is_match(command) {
        return false;
    }
    let long = SLEEPS
        .captures_iter(command)
        // Go's Atoi gives its largest on overflow: a long sleep still.
        .any(|m| m[2].parse::<u64>().unwrap_or(u64::MAX) >= 20);
    long || LOOPS.is_match(command) && SLEEPS.is_match(command)
}

/// s as one shell word.
fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains(|c| " \t\n'\"\\$`!*?[]{}()<>|&;#~".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_checks_watch() {
        for (cmd, bg, want) in [
            (
                "gh pr checks 42 --watch",
                false,
                "echo 'rush: watching through rush watch pr, which waits for you and prints once'; /r watch pr 42 --until done --timeout 1m45s",
            ),
            (
                "cd ~/x && gh pr checks --watch --fail-fast -i 5",
                true,
                "cd ~/x && echo 'rush: watching through rush watch pr, which waits for you and prints once'; /r watch pr --until fail --timeout 45m0s",
            ),
            ("gh pr checks 42", false, ""),
            ("gh pr checks 42 --watch --json name", false, ""),
            ("gh pr checks 42 --watch | tail -5", false, ""),
        ] {
            let (got, deny) = redirect(cmd, bg, Duration::ZERO, "/r");
            assert_eq!((got.as_str(), deny.as_str()), (want, ""), "{cmd}");
        }
    }

    #[test]
    fn turns_back_polling() {
        for (cmd, want) in [
            ("sleep 60 && gh pr checks 42", true),
            ("while true; do gh run list -L1; sleep 5; done", true),
            ("sleep 300; gh api repos/o/r/commits/abc/check-runs", true),
            ("sleep 3 && gh pr view 42 --json url", false), // a moment for GitHub to catch up
            ("sleep 99999999999999999999 && gh pr checks 42", true),
            ("sleep 60 && curl localhost:3000", false),
            ("for f in *.go; do gofmt -l $f; done; gh pr checks 4", false),
        ] {
            let (_, deny) = redirect(cmd, false, Duration::ZERO, "/r");
            assert_eq!(!deny.is_empty(), want, "{cmd}");
        }
    }

    #[test]
    fn rewrites_run_watch() {
        let say = "echo 'rush: watching through rush watch run, which waits for you and prints once'; /r watch run";
        for (cmd, bg, want) in [
            (
                "gh run watch 123 --exit-status",
                false,
                format!("{say} 123 --until done --timeout 1m45s"),
            ),
            (
                "cd ~/x && gh run watch -i 3 --compact",
                true,
                format!("cd ~/x && {say} --until done --timeout 45m0s"),
            ),
            (
                "gh run watch 9 -R o/r",
                true,
                format!("{say} o/r@9 --until done --timeout 45m0s"),
            ),
        ] {
            let (got, deny) = redirect(cmd, bg, Duration::ZERO, "/r");
            assert_eq!((got.as_str(), deny.as_str()), (want.as_str(), ""), "{cmd}");
        }
    }

    #[test]
    fn turns_back_run_polling_toward_watch_run() {
        for cmd in [
            "gh run watch 123 --exit-status && echo ok",
            "gh run watch 123 --json x",
            "gh run watch -R o/r",
            "while true; do gh run view 123; sleep 5; done",
            "sleep 60 && gh run list --branch main -L1",
            "sleep 120; gh api repos/o/r/actions/runs/1/jobs",
        ] {
            let (got, deny) = redirect(cmd, false, Duration::ZERO, "/r");
            assert!(got.is_empty(), "{cmd}: rewritten to {got}");
            assert!(deny.contains("rush watch run"), "{cmd}: {deny}");
        }
        // A pull request's checks still go to watch pr.
        let (_, deny) = redirect("sleep 60 && gh pr checks 42", false, Duration::ZERO, "/r");
        assert!(deny.contains("rush watch pr"), "{deny}");
        // A look at a run, once, is left alone.
        let (got, deny) = redirect("gh run view 123 --log-failed", false, Duration::ZERO, "/r");
        assert_eq!((got.as_str(), deny.as_str()), ("", ""));
    }
}
