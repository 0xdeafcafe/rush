//! Claude Code's own ways to change a session outside rush mode: its
//! background daemon's control socket first, the claude CLI as the
//! fallback. Port of internal/adapters/claude/actions.go.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use rush_core::adapter::{
    self, Attacher, Dispatcher, Loginer, Move, Mover, Profile, Remover, Replier, Screener, Stopper,
};

use crate::account::{Account, path_str};
use crate::adapter::{Claude, account};
use crate::daemon::{self, Client};
use crate::jobs::{ClaudeJob, project_slug};
use crate::screen::Refusal;
use crate::sessions::read_workers;

/// Claude Code's program, run by name from PATH.
pub const PROGRAM: &str = "claude";

fn err(msg: impl Into<String>) -> adapter::Error {
    msg.into().into()
}

/// The session id claude --bg prints in its "claude attach <id>" hint, or
/// else the first short id it prints.
fn new_id(out: &str) -> String {
    static ATTACH: OnceLock<Regex> = OnceLock::new();
    static SHORT: OnceLock<Regex> = OnceLock::new();
    let attach = ATTACH.get_or_init(|| Regex::new(r"claude attach ([0-9a-f]{8})").unwrap());
    if let Some(m) = attach.captures(out) {
        return m[1].to_string();
    }
    let short = SHORT.get_or_init(|| Regex::new(r"\b[0-9a-f]{8}\b").unwrap());
    short
        .find(out)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// The new session's id in a resume's "claude attach <id>" hint.
fn attach_id(out: &str) -> Option<String> {
    Regex::new(r"claude attach ([0-9a-f]{8})")
        .ok()?
        .captures(out)
        .map(|m| m[1].to_string())
}

/// claude, run as acct in dir.
pub(crate) fn claude_cmd(acct: &Account, dir: &str, args: &[&str]) -> Command {
    let mut c = Command::new(PROGRAM);
    c.args(args).env_clear();
    for e in acct.env() {
        if let Some((k, v)) = e.split_once('=') {
            c.env(k, v);
        }
    }
    if !dir.is_empty() {
        c.current_dir(dir);
    }
    c
}

/// Runs c for what it says, stdout and stderr together; failing, its last
/// line is the error.
fn run(c: &mut Command) -> adapter::Result<String> {
    let (mut r, w) = std::io::pipe()?;
    c.stdin(Stdio::null()).stdout(w.try_clone()?).stderr(w);
    let child = c.spawn();
    // c keeps the write ends it was given: swap them out so the read end
    // sees the end once the program exits.
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = child?;
    let mut out = Vec::new();
    let _ = r.read_to_end(&mut out);
    let st = child.wait()?;
    let s = String::from_utf8_lossy(&out).trim().to_string();
    if !st.success() {
        let s = if s.is_empty() {
            match st.code() {
                Some(c) => format!("exit status {c}"),
                None => "signal: killed".into(),
            }
        } else {
            s
        };
        return Err(err(last_line(&s)));
    }
    Ok(s)
}

fn last_line(s: &str) -> String {
    let s = s.trim();
    s.rsplit_once('\n').map_or(s, |(_, l)| l).to_string()
}

impl Dispatcher for Claude {
    /// Starts a new background session and returns its short id.
    fn dispatch(
        &self,
        p: &Profile,
        dir: &str,
        prompt: &str,
        flags: &[String],
    ) -> adapter::Result<String> {
        let mut args = vec!["--bg"];
        args.extend(flags.iter().map(String::as_str));
        args.extend(["--", prompt]); // a prompt starting with - is still a prompt
        let out = run(&mut claude_cmd(&account(p), dir, &args))?;
        Ok(new_id(&out))
    }
}

impl Stopper for Claude {
    /// Ends a session's process and keeps its conversation. When the
    /// daemon and the CLI both fail, or the process outlives them, pid gets
    /// a SIGTERM.
    fn stop(&self, p: &Profile, short: &str, pid: i32) -> adapter::Result<()> {
        stop(&account(p), short, pid)
    }
}

fn stop(acct: &Account, short: &str, pid: i32) -> adapter::Result<()> {
    let mut res = Client::new(acct.clone()).kill(short);
    if res.is_err() {
        res = run(&mut claude_cmd(acct, "", &["stop", short])).map(drop);
    }
    if pid == 0 || wait_exit(pid, Duration::from_secs(5)) {
        return res;
    }
    rush_sys::proc::kill(pid, libc::SIGTERM).map_err(Into::into)
}

/// Whether pid has gone within max. Another's process says nothing when
/// it goes, so, as Go does, it's looked at every 200ms.
fn wait_exit(pid: i32, max: Duration) -> bool {
    let deadline = Instant::now() + max;
    while Instant::now() < deadline {
        // SAFETY: kill(2) with signal 0 only asks.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

impl Remover for Claude {
    /// Deletes a session, and its worktree when Claude Code deems it safe.
    fn remove(&self, p: &Profile, short: &str) -> adapter::Result<()> {
        run(&mut claude_cmd(&account(p), "", &["rm", short])).map(drop)
    }
}

impl Replier for Claude {
    /// Sends a message to a background session through the daemon. One the
    /// daemon has let go of (ENOJOB) is Gone.
    fn reply(&self, p: &Profile, short: &str, text: &str) -> adapter::Result<()> {
        let c = Client::new(account(p));
        if !c.running() {
            return Err(err(
                "Claude Code's background agents service isn't running, so it can't take a message; open the agent with enter instead",
            ));
        }
        match c.reply(short, text) {
            Err(e) if daemon::is_refusal(&*e, "ENOJOB") => Err(Box::new(Refusal::gone(e, true))),
            r => r,
        }
    }
}

impl Attacher for Claude {
    /// The claude CLI's own attach, used when the socket refuses.
    fn attach(&self, p: &Profile, short: &str) -> Command {
        claude_cmd(&account(p), "", &["attach", short])
    }
}

impl Loginer for Claude {
    /// Signs the config folder p is in to an account.
    fn login(&self, p: &Profile) -> Command {
        claude_cmd(&account(p), "", &["auth", "login"])
    }
}

impl Screener for Claude {
    /// Claude Code's own screen for a slash command (/plugin, /hooks, …)
    /// in dir. Nothing is sent to a model, and no conversation is kept:
    /// it's a fresh Claude Code that starts on the command. hint is
    /// printed above it (how to get back).
    fn screen(&self, p: &Profile, dir: &str, command: &str, hint: &str) -> Command {
        let mut c = Command::new("sh");
        c.args([
            "-c",
            r#"printf '\033[2J\033[H%s\n\n' "$1"; exec claude "/$2""#,
            "sh",
            hint,
            command,
        ])
        .env_clear();
        for e in account(p).env() {
            if let Some((k, v)) = e.split_once('=') {
                c.env(k, v);
            }
        }
        if !dir.is_empty() {
            c.current_dir(dir);
        }
        c
    }
}

impl Mover for Claude {
    /// Moves a conversation: stop it, make its transcript visible to the
    /// target account and folder, and resume it there with its original
    /// flags (from its job file, the listing's extra).
    fn move_session(&self, mv: &Move) -> adapter::Result<String> {
        let cj = mv
            .extra
            .as_ref()
            .and_then(|x| x.downcast_ref::<ClaudeJob>());
        let flags = cj.map(|c| c.respawn_flags.clone()).unwrap_or_default();
        let j = &mv.job;
        let (from, to) = (account(&mv.from), account(&mv.to));
        if j.session_id.is_empty() || j.transcript_path.is_empty() {
            return Err(err("this agent has no saved conversation to move"));
        }
        let dir = if mv.dir.is_empty() { &j.cwd } else { &mv.dir };
        if !std::path::Path::new(dir).is_dir() {
            return Err(err(format!("folder not found: {dir}")));
        }
        if running(&from, &j.id) {
            stop(&from, &j.id, 0)
                .map_err(|e| err(format!("could not stop the agent first: {e}")))?;
            if !wait_stopped(&from, &j.id, Duration::from_secs(10)) {
                return Err(err("the agent is still stopping; try again in a moment"));
            }
        }
        let dst = to
            .projects_dir()
            .join(project_slug(dir))
            .join(format!("{}.jsonl", j.session_id));
        if path_str(&dst) != j.transcript_path {
            copy_file(&j.transcript_path, &dst)
                .map_err(|e| err(format!("could not copy the conversation: {e}")))?;
        }
        let mut args: Vec<String> = vec!["--bg".into(), "--resume".into(), j.session_id.clone()];
        args.extend(respawn_flags(&flags));
        for d in &mv.add_dirs {
            args.extend(["--add-dir".into(), d.clone()]);
        }
        if !mv.note.is_empty() {
            // --add-dir takes many values; end them first.
            args.extend(["--".into(), mv.note.clone()]);
        }
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run(&mut claude_cmd(&to, dir, &a))?;
        // Resuming in the background forks the conversation into a new
        // session.
        attach_id(&out).ok_or_else(|| {
            err("resumed, but Claude Code didn't say the new session's id; it will appear in the list")
        })
    }
}

fn running(a: &Account, short: &str) -> bool {
    read_workers(a).iter().any(|(id, _)| id == short)
}

/// The flags that describe the agent, not the old session.
fn respawn_flags(flags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i].as_str();
        if matches!(f, "--resume" | "--session-id" | "-r") {
            i += 2;
            continue;
        }
        if matches!(f, "--fork-session" | "-c" | "--continue")
            || f.starts_with("--resume=")
            || f.starts_with("--session-id=")
        {
            i += 1;
            continue;
        }
        out.push(flags[i].clone());
        i += 1;
    }
    out
}

/// Whether the daemon has let go of short within max; it says so only in
/// its roster, which is looked at every 200ms, as Go does.
fn wait_stopped(a: &Account, short: &str, max: Duration) -> bool {
    let deadline = Instant::now() + max;
    while Instant::now() < deadline {
        if !running(a, short) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn copy_file(src: &str, dst: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut input = std::fs::File::open(src)?;
    let mut tmp = dst.as_os_str().to_owned();
    tmp.push(".rush.tmp");
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    std::io::copy(&mut input, &mut out)?;
    out.sync_all().ok();
    drop(out);
    std::fs::rename(&tmp, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respawn_flags_drop_session_identity() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            respawn_flags(&s(&[
                "--agent",
                "claude",
                "--resume",
                "abc",
                "--model",
                "opus[1m]",
                "--session-id",
                "x",
                "--fork-session"
            ])),
            s(&["--agent", "claude", "--model", "opus[1m]"])
        );
        assert_eq!(
            respawn_flags(&s(&[
                "--resume=abc",
                "-c",
                "--model",
                "opus",
                "--continue",
                "--session-id=x"
            ])),
            s(&["--model", "opus"])
        );
    }

    #[test]
    fn new_id_prefers_the_attach_hint() {
        let out = "Resumed bc572d22-5b3b\n  claude attach 5606143a    open in this terminal\n";
        assert_eq!(new_id(out), "5606143a");
        assert_eq!(new_id("started deadbeef"), "deadbeef");
        assert_eq!(attach_id("nothing"), None);
    }

    #[test]
    fn runs_and_says_its_last_line_on_failure() {
        let ok = run(Command::new("sh").args(["-c", "echo out; echo err >&2"])).unwrap();
        assert_eq!(ok, "out\nerr");
        let e = run(Command::new("sh").args(["-c", "echo one; echo two >&2; exit 2"])).unwrap_err();
        assert_eq!(e.to_string(), "two");
        let e = run(Command::new("sh").args(["-c", "exit 3"])).unwrap_err();
        assert_eq!(e.to_string(), "exit status 3");
    }

    #[test]
    fn a_reply_needs_the_service() {
        let c = Claude::new();
        let p = crate::adapter::profile(&Account::at("x", "/nonexistent/rush-test"));
        let e = c.reply(&p, "abcd1234", "hi").unwrap_err();
        assert!(
            e.to_string()
                .contains("background agents service isn't running")
        );
    }

    #[test]
    fn a_move_needs_a_conversation_and_a_folder() {
        let c = Claude::new();
        let mut mv = Move::default();
        assert_eq!(
            c.move_session(&mv).unwrap_err().to_string(),
            "this agent has no saved conversation to move"
        );
        mv.job.session_id = "s".into();
        mv.job.transcript_path = "/t.jsonl".into();
        mv.dir = "/nonexistent/rush-test".into();
        assert_eq!(
            c.move_session(&mv).unwrap_err().to_string(),
            "folder not found: /nonexistent/rush-test"
        );
    }

    #[test]
    fn copies_a_transcript_whole() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.jsonl");
        std::fs::write(&src, "line\n").unwrap();
        let dst = d.path().join("p/q/a.jsonl");
        copy_file(&path_str(&src), &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst).unwrap(), "line\n");
    }
}
