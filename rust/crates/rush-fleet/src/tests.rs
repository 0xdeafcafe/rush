//! Port of internal/fleet/stall_test.go and stuck_test.go, plus the list
//! read through the registry from a temp Claude Code folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rush_claude::{Account, Claude};
use rush_core::adapter::{Registry, Session};
use rush_core::agent::{Agent, Halt, State};
use rush_core::time::Ms;

use crate::loader::{apply_status, job_cwd};
use crate::*;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

/// A folder of its own under the temp dir, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "rush-fleet-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn finished(now: Ms) -> Agent {
    let mut a = Agent {
        pid: 1,
        ..Agent::default()
    };
    a.job.state = State::Done;
    a.job.updated_at = now - 10 * MINUTE;
    a
}

fn halt(kind: &str, text: &str, at: Ms) -> Option<Halt> {
    Some(Halt {
        kind: kind.into(),
        text: text.into(),
        at,
    })
}

#[test]
fn a_finished_turn_is_an_answer_or_an_error() {
    let now = rush_core::time::now();
    let a = finished(now);
    assert!(a.your_turn(now) && !a.halted() && a.continue_text() == "keep going");

    let mut a = finished(now);
    a.spend.halt = halt(
        "rate_limit",
        "You've hit your session limit · resets 5am (Europe/London)",
        0,
    );
    assert!(a.halted() && !a.your_turn(now) && a.continue_text() == "continue");
    assert_eq!(a.halt_reason(now), "session limit · resets 5am");
    a.spend.halt = halt(
        "server_error",
        "API Error: Can't reach the API server — check your connection",
        0,
    );
    assert_eq!(
        a.halt_reason(now),
        "Can't reach the API server — check your connection"
    );

    // Working again, or put away, it is neither.
    let mods: [&dyn Fn(&mut Agent); 3] = [
        &|a| a.job.state = State::Working,
        &|a| a.past = true,
        &|a| {
            a.job.in_flight = 1;
            a.job.background = vec!["agent\0lane".into()];
        },
    ];
    for m in mods {
        let mut a = finished(now);
        a.spend.halt = halt("server_error", "", 0);
        m(&mut a);
        let mut b = finished(now);
        m(&mut b);
        assert!(!a.halted() && !b.your_turn(now), "{:?}", a.job);
    }
    let mut old = finished(now);
    old.job.updated_at = now - 25 * HOUR;
    assert!(!old.your_turn(now), "a day-old finish is still your turn");
}

#[test]
fn api_errors_continue() {
    let now = rush_core::time::now();
    let halted = |text: &str, at: Ms| {
        let mut a = finished(now);
        a.job.updated_at = at;
        a.spend.halt = halt("server_error", text, at);
        a
    };
    let a = halted(
        "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)",
        now - MINUTE,
    );
    assert!(a.continues(now));
    assert_eq!(
        a.halt_reason(now),
        "offline · continues when the network is back"
    );
    for text in [
        "API Error: Connection dropped (ECONNRESET)",
        "API Error: Response stalled mid-stream. The response above may be incomplete.",
    ] {
        let a = halted(text, now);
        assert!(a.continues(now) && !a.offline(), "{text}");
        assert_eq!(
            a.halt_reason(now),
            format!(
                "{} · continues by itself",
                text.strip_prefix("API Error: ").unwrap()
            )
        );
    }
    let mut a = halted(
        "You've hit your session limit · resets 5am (Europe/London)",
        now,
    );
    a.spend.halt.as_mut().unwrap().kind = "rate_limit".into();
    assert!(!a.continues(now), "a limit waits for its reset");
    let a = halted("Not logged in · Please run /login", now);
    assert!(!a.continues(now), "logging in is yours to do");
    let a = halted(
        "API Error: Unable to connect to API (ENOTFOUND)",
        now - 48 * HOUR,
    );
    assert!(!a.continues(now), "two days ago is too long ago");
}

#[test]
fn stuck() {
    let now = rush_core::time::now();
    let mut a = Agent::default();
    a.job.state = State::Working;
    a.job.updated_at = now - 31 * MINUTE;
    assert!(a.stuck(now), "quiet 31m at work should be stuck");
    a.job.updated_at = now - 6 * MINUTE;
    assert!(!a.stuck(now));
    a.job.state = State::Done;
    a.job.updated_at = now - 31 * MINUTE;
    assert!(!a.stuck(now), "a finished agent is never stuck");
}

#[test]
fn status_overrides_the_job_file() {
    let ss = |status: &str, at: Ms| Session {
        status: status.into(),
        status_at: at,
        ..Session::default()
    };
    let mut a = Agent::default();
    a.job.needs = "x".into();
    assert_eq!(apply_status(&mut a, "running", &ss("busy", 0)), "working");
    assert_eq!(apply_status(&mut a, "blocked", &ss("busy", 0)), "working");
    assert!(a.job.needs.is_empty());
    a.job.background = vec!["shell\0x".into()];
    assert_eq!(apply_status(&mut a, "done", &ss("busy", 0)), "done");
    a.job.updated_at = 10;
    assert_eq!(apply_status(&mut a, "working", &ss("idle", 5)), "working");
    assert_eq!(apply_status(&mut a, "working", &ss("idle", 20)), "blocked");
    assert!(a.checking);
}

#[test]
fn job_cwd_prefers_the_worktree() {
    assert_eq!(job_cwd("/r", ""), "/r");
    assert_eq!(job_cwd("/r", "/r/.wt/a"), "/r/.wt/a");
    assert_eq!(job_cwd("/r/.wt/a/sub", "/r/.wt/a"), "/r/.wt/a/sub");
    assert_eq!(job_cwd("/r/.wt/ab", "/r/.wt/a"), "/r/.wt/a");
}

#[test]
fn git_at_finds_the_checkout_and_branch() {
    let d = TempDir::new("git");
    let repo = d.0.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("a/b")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let (r, b) = git_at(&path_str(&repo.join("a/b")));
    assert_eq!((r, b.as_str()), (path_str(&repo), "main"));
    // A linked worktree: .git is a file naming its git dir.
    let wt = d.0.join("wt");
    std::fs::create_dir_all(d.0.join("gd")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: ../gd\n").unwrap();
    std::fs::write(
        d.0.join("gd/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    assert_eq!(git_at(&path_str(&wt)).1, "01234567");
}

#[test]
fn loads_jobs_sessions_and_past() {
    let d = TempDir::new("fleet");
    // Claude's spend cache is kept in rush's cache folder: a temp one.
    rush_state::set_thread_cache_dir(Some(d.0.join("cache")));
    let acct = Account::at("default", &d.0);
    let now = rush_core::time::now();
    // A background job whose session says it's busy.
    let jd = acct.jobs_dir().join("job00001");
    std::fs::create_dir_all(&jd).unwrap();
    std::fs::write(
        jd.join("state.json"),
        r#"{"state":"running","name":"build it","cwd":"/r","sessionId":"jobsess1-0000","updatedAt":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    // The daemon runs the job, in a process named as Claude Code's program
    // is (a copy of sleep): a pid the roster names counts only if it's
    // the agent's program.
    let fake = d.0.join("claude");
    std::fs::copy("/bin/sleep", &fake).unwrap();
    let mut worker = std::process::Command::new(&fake).arg("30").spawn().unwrap();
    let wpid = worker.id();
    // The sessions began once their process had: a pid that started after
    // its session is another's.
    let began = rush_core::time::now() + 1000;
    // Its session, and one open in a terminal, both alive, run by a
    // process of the agent's program (a session whose process isn't is
    // left out, as Go's isProgramPID says).
    let sd = acct.sessions_dir();
    std::fs::create_dir_all(&sd).unwrap();
    let me = std::process::id();
    std::fs::write(
        sd.join("1.json"),
        format!(r#"{{"pid":{wpid},"sessionId":"jobsess1-0000","cwd":"/r","kind":"bg","status":"busy","jobId":"job00001","startedAt":{began}}}"#),
    )
    .unwrap();
    std::fs::write(
        sd.join("2.json"),
        format!(r#"{{"pid":{wpid},"sessionId":"termsess-0000","cwd":"/r","kind":"interactive","status":"idle","name":"term","startedAt":{began}}}"#),
    )
    .unwrap();
    std::fs::create_dir_all(d.0.join("daemon")).unwrap();
    let roster = |pid: u32| {
        std::fs::write(
            acct.roster_path(),
            format!(r#"{{"workers":[{{"pid":{pid},"dispatch":{{"short":"job00001"}}}}]}}"#),
        )
        .unwrap();
    };
    roster(wpid);
    // Past: one conversation, one the job's own (claimed), and one whose
    // id the terminal's row already stands for.
    let proj = d.0.join("projects").join("-r");
    std::fs::create_dir_all(&proj).unwrap();
    let convo = |sid: &str, text: &str| {
        let body = format!(
            r#"{{"type":"user","cwd":"/r","timestamp":"2026-09-29T10:00:00Z","message":{{"role":"user","content":"{text}"}}}}
{{"type":"assistant","cwd":"/r","timestamp":"2026-09-29T10:00:05Z","message":{{"id":"m1","model":"claude-opus-5-5","content":[{{"type":"text","text":"ok"}}],"usage":{{"output_tokens":1000000}}}}}}
"#
        );
        std::fs::write(proj.join(format!("{sid}.jsonl")), body).unwrap();
    };
    convo("pastsess-0000", "fix the build");
    convo("jobsess1-0000", "the job's");
    convo("termsess-0000", "the terminal's");

    let reg = Registry::new();
    reg.register_ref(Arc::new(Claude::at(acct.clone())));
    let mut l = Loader::new(reg);
    let rows = l.load(now).agents;
    let by = |k: &str| {
        rows.iter()
            .find(|a| a.key == k)
            .unwrap_or_else(|| panic!("no {k}: {rows:#?}"))
    };
    assert_eq!(rows.len(), 3, "{rows:#?}");
    let job = by("default/job00001");
    assert_eq!(job.job.state, State::Working);
    assert_eq!(job.pid, wpid as i32);
    let term = by("default/i:termsess");
    assert!(term.interactive);
    assert_eq!(term.job.state, State::Idle);
    assert_eq!(term.job.detail, "open in a terminal");
    assert!(
        term.job
            .transcript_path
            .ends_with("/projects/-r/termsess-0000.jsonl")
    );
    let past = by("default/i:pastsess");
    assert!(past.past);
    assert_eq!(past.display_name, "fix the build");
    assert_eq!(past.job.state, State::Stopped);
    // Priced once scanned.
    assert_eq!(l.scan(&rows), 3);
    let rows = l.load(now).agents;
    let past = rows.iter().find(|a| a.key == "default/i:pastsess").unwrap();
    assert!(past.spend.ready);
    assert!((past.spend.cost - 20.0).abs() < 1e-9, "{}", past.spend.cost);
    assert_eq!(past.spend.model, "claude-opus-5-5");
    assert!(past.elapsed(now) == 5000, "{}", past.elapsed(now));
    assert!(rows.iter().all(|a| a.kind.as_str() == "claude"));
    // What to watch: the agent's folders, the job's, and each project's.
    let w = l.watched();
    for p in [
        acct.sessions_dir(),
        acct.jobs_dir().join("job00001").join("state.json"),
        proj.clone(),
        d.0.join("projects"),
    ] {
        assert!(w.contains(&p), "{p:?} not in {w:#?}");
    }
    // A roster naming a pid another program has now: the job lost its
    // process.
    roster(me);
    let rows = l.load(now).agents;
    let job = rows.iter().find(|a| a.key == "default/job00001").unwrap();
    assert_eq!(job.pid, 0);
    assert_eq!(job.job.detail, "lost its process", "{job:#?}");
    worker.kill().unwrap();
    worker.wait().unwrap();
}

#[test]
fn print_mode() {
    assert_eq!(is_print(&["claude", "-p", "hi"]), (true, false));
    assert_eq!(
        is_print(&["claude", "--output-format=json", "--no-session-persistence"]),
        (true, true)
    );
    assert_eq!(is_print(&["claude"]), (false, false));
}
