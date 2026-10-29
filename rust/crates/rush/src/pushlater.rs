//! `rush push-later [git push args]`: the push the gate's hook held, sent
//! once the branch is quiet. `--now` sends this branch's held push at
//! once; `--run <key>` is the waiter, started by the first, in a session of
//! its own (rush's own binary found through selfexe, so an install writing
//! rush doesn't fail it). The holds and the waiter's loop are
//! rush_gh::pushlater's.
//!
//! Ported from cmd/rush/pushlater.go:1-163 (pushLater :22, gitWhere :60,
//! defaultBranch :73, gitPush :82, startWaiter :97, pushWait :117).

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use rush_gh::pushlater::{self as pl, Hold, Holds, Waiter};
use serde_json::json;

/// How long a held push may take before it's given up on (pushWait's
/// context).
const PUSH_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// `rush push-later args`: the exit code.
pub fn push_later(args: &[String]) -> i32 {
    if args.len() == 2 && args[0] == "--run" {
        return push_wait(&args[1]);
    }
    let now = args.first().is_some_and(|a| a == "--now");
    let args = if now { &args[1..] } else { args };
    let Ok((dir, branch)) = git_where() else {
        return git_push(".", args);
    };
    let key = pl::key(&dir, &branch);
    let holds = Holds::default();
    if now {
        return match holds.take(&key) {
            Ok(h) => git_push(&h.dir, &h.args),
            Err(_) => {
                eprintln!("rush push-later: {}", pl::ERR_NONE);
                1
            }
        };
    }
    if pl::off() || pl::protected(&branch) || branch == default_branch(&dir) {
        return git_push(&dir, args);
    }
    let h = Hold {
        dir: dir.clone(),
        branch: branch.clone(),
        args: args.to_vec(),
        session: std::env::var("RUSH_SESSION").unwrap_or_default(),
        at: rush_core::time::now(),
    };
    if holds.write(&h).is_err() {
        return git_push(&dir, args); // can't hold it: send it
    }
    if start_waiter(&holds, &key).is_err() {
        let _ = holds.take(&key);
        return git_push(&dir, args);
    }
    println!(
        "push held: rush sends {branch} once it has had no new push for {}; you'll get a message if it fails (rush push-later --now sends it now)",
        pl::QUIET_WORDS
    );
    0
}

/// The repository's top folder and the branch checked out.
fn git_where() -> Result<(String, String), String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(go_exit(out.status));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let f: Vec<&str> = s.split_whitespace().collect();
    if f.len() != 2 || f[1] == "HEAD" {
        return Err("not on a branch".into());
    }
    Ok((f[0].to_string(), f[1].to_string()))
}

/// The remote's default branch, "" when unknown.
fn default_branch(dir: &str) -> String {
    let Ok(out) = Command::new("git")
        .args([
            "-C",
            dir,
            "symbolic-ref",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .stderr(Stdio::null())
        .output()
    else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    s.strip_prefix("origin/").unwrap_or(s).to_string()
}

/// Runs git push args in dir, its output passed through.
fn git_push(dir: &str, args: &[String]) -> i32 {
    match Command::new("git")
        .args(["-C", dir, "push"])
        .args(args)
        .status()
    {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("rush push-later: {e}");
            1
        }
    }
}

/// Starts `rush push-later --run key`, in its own session so it outlives
/// the agent's shell.
fn start_waiter(holds: &Holds, key: &str) -> Result<(), String> {
    let exe = rush_sys::selfexe::path().map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(holds.dir().join(format!("{key}.log")))
        .map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(["push-later", "--run", key])
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    // SAFETY: setsid only, between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    // Released, as Go's Process.Release: the waiter outlives this rush,
    // which exits at once.
    cmd.spawn().map(drop).map_err(|e| e.to_string())
}

/// The waiter: one per held branch.
fn push_wait(key: &str) -> i32 {
    let holds = Holds::default();
    if !holds.lock(key, |pid| rush_host::alive(pid as i64)) {
        return 0;
    }
    Waiter {
        holds: holds.clone(),
        now: Box::new(rush_core::time::now),
        sleep: Box::new(std::thread::sleep),
        push: Box::new(|h: &Hold| {
            let r = push_held(h);
            let mut f = json!({"branch": h.branch, "args": h.args.join(" ")});
            if let Err((e, _)) = &r {
                f["err"] = json!(e);
            }
            if !h.session.is_empty() {
                rush_host::events::log_event(&h.session, "push-sent", f);
            }
            r.map_err(|(e, out)| format!("{e}\n{}", out.trim()))
        }),
        fail: Some(Box::new(|h: &Hold, err: &str| {
            if h.session.is_empty() {
                return;
            }
            let msg = format!(
                "rush held your `git push` of {} and sent it once the branch was quiet, but it failed:\n\n{err}\n\nFix it, then push again.",
                h.branch
            );
            // As Go runs os.Args[0]: the rush that's waiting.
            let me = std::env::args().next().unwrap_or_else(|| "rush".into());
            if let Ok(mut c) = Command::new(me)
                .args(["session", "send", &h.session])
                .stdin(Stdio::piped())
                .spawn()
            {
                if let Some(mut w) = c.stdin.take() {
                    let _ = std::io::Write::write_all(&mut w, msg.as_bytes());
                }
                let _ = c.wait();
            }
        })),
    }
    .run(key);
    holds.unlock(key);
    0
}

/// git push for h in its folder, given up on after PUSH_TIMEOUT: Err is
/// Go's words for how it ended and what git said (CombinedOutput).
fn push_held(h: &Hold) -> Result<(), (String, String)> {
    let (mut r, w) = std::io::pipe().map_err(|e| (e.to_string(), String::new()))?;
    let w2 = w.try_clone().map_err(|e| (e.to_string(), String::new()))?;
    let mut cmd = Command::new("git");
    cmd.args(["-C", &h.dir, "push"])
        .args(&h.args)
        .stdin(Stdio::null())
        .stdout(w)
        .stderr(w2);
    let mut child = cmd.spawn().map_err(|e| (e.to_string(), String::new()))?;
    drop(cmd); // its ends of the pipe, so git's exit ends the read
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = r.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let out = match rx.recv_timeout(PUSH_TIMEOUT) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            rx.recv().unwrap_or_default()
        }
    };
    let out = String::from_utf8_lossy(&out).into_owned();
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err((go_exit(s), out)),
        Err(e) => Err((e.to_string(), out)),
    }
}

/// How Go's exec.ExitError says s: "exit status N" or "signal: killed".
fn go_exit(s: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(c) = s.code() {
        return format!("exit status {c}");
    }
    match s.signal() {
        Some(libc::SIGKILL) => "signal: killed".into(),
        Some(libc::SIGTERM) => "signal: terminated".into(),
        Some(libc::SIGINT) => "signal: interrupt".into(),
        Some(n) => format!("signal: {n}"),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go's exec.ExitError words, which the session is told.
    #[test]
    fn exit_words_as_go_says_them() {
        use std::os::unix::process::ExitStatusExt;
        for (raw, want) in [
            (1 << 8, "exit status 1"),
            (128 << 8, "exit status 128"),
            (libc::SIGKILL, "signal: killed"),
            (libc::SIGTERM, "signal: terminated"),
        ] {
            assert_eq!(go_exit(ExitStatus::from_raw(raw)), want);
        }
    }

    // A push held then sent, in a repository of its own: the hold is
    // gone, the branch is on the remote, and git's words come back on a
    // failure.
    #[test]
    fn a_held_push_is_sent_and_a_failed_one_says_why() {
        let d = tempfile::tempdir().unwrap();
        let p = |s: &str| d.path().join(s).to_string_lossy().into_owned();
        let git = |dir: &str, a: &[&str]| {
            let ok = Command::new("git")
                .args(["-C", dir])
                .args(a)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {a:?}");
        };
        git(
            &d.path().to_string_lossy(),
            &["init", "-q", "--bare", &p("remote")],
        );
        git(&d.path().to_string_lossy(), &["init", "-q", &p("work")]);
        let w = p("work");
        for a in [
            &[
                "-c",
                "user.email=a@b",
                "-c",
                "user.name=a",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ][..],
            &["remote", "add", "origin", &p("remote")],
        ] {
            git(&w, a);
        }
        let h = Hold {
            dir: w.clone(),
            branch: "feat".into(),
            args: vec!["origin".into(), "HEAD:refs/heads/feat".into()],
            ..Hold::default()
        };
        assert_eq!(push_held(&h), Ok(()));
        let bad = Hold {
            args: vec!["nowhere".into(), "HEAD".into()],
            ..h
        };
        let (e, out) = push_held(&bad).unwrap_err();
        assert!(e.starts_with("exit status "), "{e}");
        assert!(out.contains("nowhere"), "{out}");
    }
}
