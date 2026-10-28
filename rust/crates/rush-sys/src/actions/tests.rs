//! Ported from internal/actions/endtree_test.go, signal_test.go and
//! worktree_test.go. Waits are on the kernel (waitpid, kqueue), never
//! sleeps.

use super::*;
use crate::testutil::{gone_within, wait_stopped};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn end_tree_ends_a_pipeline_that_ignores_term() {
    // A shell with a pipeline under it, one part of which ignores SIGTERM.
    // That part says when it's there; the left part was started before it.
    let mut c = Command::new("/bin/sh")
        .args([
            "-c",
            r#"sleep 60 | (trap "" TERM; echo ready >&2; exec sleep 61)"#,
        ])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(c.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "ready\n");
    let root_pid = c.id() as Pid;
    std::thread::spawn(move || c.wait());
    let tab = proc::snapshot();
    let root = &tab.procs[&root_pid];
    let pids = tab.descendants(root_pid);
    assert!(pids.len() >= 3, "want a tree, got {pids:?}");
    let n = end_tree(root_pid, root.start, Duration::from_millis(500)).unwrap();
    assert_eq!(n, pids.len());
    for pid in pids {
        assert!(
            gone_within(pid, Duration::from_secs(3)),
            "{pid} still running"
        );
    }
}

#[test]
fn kill_tree_kills_everything_under() {
    let mut c = Command::new("/bin/sh")
        .args(["-c", "sleep 60 & echo ready; wait"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let root_pid = c.id() as Pid;
    std::thread::spawn(move || c.wait());
    let tab = proc::snapshot();
    let pids = tab.descendants(root_pid);
    assert_eq!(pids.len(), 2, "{pids:?}");
    assert_eq!(kill_tree(root_pid, tab.procs[&root_pid].start), Ok(2));
    for pid in pids {
        assert!(
            gone_within(pid, Duration::from_secs(3)),
            "{pid} still running"
        );
    }
}

#[test]
fn refuses_rush_and_the_wrong_process() {
    let me = std::process::id() as Pid;
    let start = proc::started(me).unwrap();
    assert_eq!(signal_tree(me, start, 0), Err(TreeError::IsRush));
    assert_eq!(TreeError::IsRush.to_string(), "that would kill rush itself");
    assert_eq!(
        kill_tree(999_999, start).unwrap_err().to_string(),
        "process 999999 has already exited"
    );
}

// A paused tree stops (ps says T) and goes on again when let go.
#[test]
fn signal_tree_pauses_and_resumes() {
    let mut c = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = c.id() as Pid;
    let start = proc::snapshot().procs[&pid].start;
    let state = || {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    signal_tree(pid, start, libc::SIGSTOP).unwrap();
    wait_stopped(pid, false);
    assert!(state().starts_with('T'), "paused, ps says {:?}", state());
    signal_tree(pid, start, libc::SIGCONT).unwrap();
    wait_stopped(pid, true);
    assert!(
        !state().starts_with('T'),
        "let go, ps still says {:?}",
        state()
    );
    assert!(
        signal_tree(pid, start + Duration::from_secs(1), libc::SIGSTOP).is_err(),
        "a process started at another time isn't the one picked"
    );
    let _ = c.kill();
    let _ = c.wait();
}

#[test]
fn go_quote_matches_go() {
    for (s, want) in [
        ("plain", r#""plain""#),
        (r#"say "hi"\now"#, r#""say \"hi\"\\now""#),
        ("a\nb\tc\x01", r#""a\nb\tc\x01""#),
        ("héllo ✓", "\"héllo ✓\""),
    ] {
        assert_eq!(go_quote(s), want);
    }
}

/// A git repository with one commit on main, and its folder, symlinks
/// resolved as git gives them.
fn repo() -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(t.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    run(&d, &["init", "-q", "-b", "main"]);
    run(
        &d,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    );
    (t, d)
}

fn run(dir: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// A worktree made from inside another goes under the main checkout, not
// inside that worktree, and starts from the main checkout's HEAD, not the
// worktree's.
#[test]
fn make_worktree_from_a_worktree_goes_under_main() {
    let (_t, main) = repo();
    let first = make_worktree(&main, "one", "rush/one", "").unwrap();
    let under = |n: &str| format!("{main}/.claude/worktrees/{n}");
    assert_eq!(first.path, under("one"));
    assert_eq!(first.repo, main);
    assert_eq!(first.target, "main");
    // Work in the first that main hasn't got.
    run(
        &first.path,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "wip",
        ],
    );
    let main_head = run(&main, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(format!("{}/sub", first.path)).unwrap();
    let second = make_worktree(&format!("{}/sub", first.path), "two", "rush/two", "").unwrap();
    assert_eq!(
        second.path,
        under("two"),
        "nested in the worktree it was made from"
    );
    assert_eq!(second.base, main_head);
    assert_eq!(run(&second.path, &["rev-parse", "HEAD"]), main_head);
    assert_eq!(main_root(&second.path), main);
    // new_worktree_on, as the fork sheet and plugins use it, the same.
    assert_eq!(
        new_worktree_on(&first.path, "three", "worktree-three", "").unwrap(),
        under("three")
    );
    // worktree_at knows the first for what it is; the main checkout isn't one.
    let m = worktree_at(&first.path).expect("worktree_at");
    assert_eq!(
        (m.branch.as_str(), m.repo.as_str(), m.target.as_str()),
        ("rush/one", main.as_str(), "main")
    );
    assert_eq!(m.base, main_head);
    assert_eq!(
        worktree_at(&main),
        None,
        "the main checkout taken for a worktree"
    );
}

#[test]
fn worktree_errors_and_the_matching_folder() {
    let (_t, main) = repo();
    std::fs::create_dir_all(format!("{main}/a/b")).unwrap();
    // The folder inside only matches when the new worktree has it too
    // (empty folders aren't committed): it falls back to the top.
    let m = make_worktree(&format!("{main}/a/b"), "w", "w", "").unwrap();
    assert_eq!(m.dir, m.path);
    assert_eq!(
        make_worktree(&main, "w", "w2", "").unwrap_err(),
        "a worktree named w already exists"
    );
    let name = std::path::Path::new(&main)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        make_worktree(&main, "x", "x", "nope").unwrap_err(),
        format!("{name} has no commit nope to start a worktree from")
    );
    let none = tempfile::tempdir().unwrap();
    let p = none.path().to_string_lossy().into_owned();
    assert_eq!(
        make_worktree(&p, "x", "x", "").unwrap_err(),
        format!("{p} isn't in a git repository")
    );
    assert_eq!(repo_root(&p), "");
    assert_eq!(
        new_worktree(&main, "n").unwrap(),
        format!("{main}/.claude/worktrees/n")
    );
    assert_eq!(
        worktree_at(&format!("{main}/.claude/worktrees/n"))
            .unwrap()
            .branch,
        "worktree-n"
    );
}
