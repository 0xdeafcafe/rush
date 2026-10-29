//! rush pr, the command: state, follow, unfollow, following, reply,
//! resolve, help, and the detached follower (follow-run) rush pr follow
//! starts. Its words and exit codes are Go's.
//! Ported from cmd/rush/pr.go and prfollow.go, with
//! internal/bundled/drive/drive.go:33 (On) and the parts of
//! internal/host/client.go:179-315 (ReadInfo, ReadConfig) the follower
//! asks.

use std::io::{Read, Write};
use std::path::PathBuf;

use rush_core::event::{Exchange, Peer};
use rush_state::Time;
use rush_watch::go::quote;
use rush_watch::goflag::{FlagSet, Kind};
use serde::Deserialize;

use crate::follow::{self, Entry, Options};
use crate::playbook::PLAYBOOK;
use crate::state::render;
use crate::trust::Trust;

/// rush pr's usage.
pub const PR_USAGE: &str = r#"rush pr: drive a pull request to mergeable. rush works out what's left, in
the order to do it, and follows the PR for you, so you never poll it.

  rush pr state [<pr>...]          what's left: the verdict and the to-do list
  rush pr follow [<pr>...]         state, then rush follows each PR and sends
                                   this session a message only with news to act on
  rush pr unfollow [<pr>...]       stop following (all of this session's, with none)
  rush pr following                what this session follows
  rush pr reply <pr> <thread> <text>   one reply on a review thread, refused when
                                   it would start or carry on an argument
  rush pr resolve <pr> <thread>... resolve review threads
  rush pr help                     this, and how to drive a PR

A PR is a number (in this repository), owner/name#number or its URL; with
none, this branch's.
"#;

/// What rush pr needs of rush's host, the part of rush that runs
/// sessions: whether a session is there, when its last turn ended, and
/// telling it something.
pub trait Host {
    /// Whether the session is still there (its config can be read).
    fn alive(&self, session: &str) -> bool {
        session_config_readable(session)
    }
    /// When its last turn ended; zero while one runs, or when it can't say.
    fn last_turn(&self, session: &str) -> Time {
        last_turn_from_info(session)
    }
    /// Sends the session e, waking it if it rests (Go's host.Deliver with
    /// now false).
    fn deliver(&self, session: &str, e: &Exchange) -> Result<(), String>;
}

/// The host as the files say, with nothing to deliver through: the binary
/// gives its own once rush's host is ported.
pub struct FilesHost;

impl Host for FilesHost {
    fn deliver(&self, _: &str, _: &Exchange) -> Result<(), String> {
        Err("rush's host can't be reached from this build".into())
    }
}

fn session_dir(id: &str) -> PathBuf {
    rush_state::dir().join("sessions").join(id)
}

/// Whether the session's config.json reads: host.ReadConfig succeeding.
pub fn session_config_readable(id: &str) -> bool {
    std::fs::read(session_dir(id).join("config.json"))
        .ok()
        .is_some_and(|b| rush_state::json::from_slice::<serde_json::Value>(&b).is_ok())
}

/// When the session's last turn ended, as its info says and the list ages
/// it: zero while it works or when it can't be read.
pub fn last_turn_from_info(id: &str) -> Time {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Info {
        #[serde(default, deserialize_with = "rush_watch::gh::nd")]
        host_pid: i64,
        #[serde(default, deserialize_with = "rush_watch::gh::nd")]
        state: String,
        #[serde(default, deserialize_with = "rush_watch::gh::nd")]
        sleeping: bool,
        #[serde(default)]
        updated_at: Option<String>,
        #[serde(default)]
        idle_since: Option<String>,
    }
    let Some(mut info) = std::fs::read(session_dir(id).join("info.json"))
        .ok()
        .and_then(|b| rush_state::json::from_slice::<Info>(&b).ok())
    else {
        return Time::default();
    };
    // A host that's gone isn't working, whatever it last wrote.
    if !host_alive(info.host_pid) && !info.sleeping {
        info.state = "stopped".into();
    }
    if info.state == "working" {
        return Time::default();
    }
    let read = |s: &Option<String>| s.as_deref().and_then(Time::parse).unwrap_or_default();
    let idle = read(&info.idle_since);
    if !idle.is_zero() {
        return idle;
    }
    read(&info.updated_at)
}

/// Whether the host process pid runs: Go's host.alive.
fn host_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only asks whether the process is there.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    let ok = r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    ok && !rush_sys::proc::zombie(pid as _)
}

/// Starts a follower for an entry: whether it started one.
pub type StartFn = Box<dyn Fn(Entry) -> Result<bool, String>>;

/// Where rush pr runs: its folder, the session it's run for, and the host
/// it tells. [`Env::from_process`] reads them as Go does.
pub struct Env {
    pub dir: PathBuf,
    /// RUSH_SESSION: "" when rush doesn't run it.
    pub session: String,
    pub host: Box<dyn Host>,
    /// Starts a follower; None, this program as rush pr follow-run.
    pub start: Option<StartFn>,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            dir: std::env::current_dir().unwrap_or_default(),
            session: std::env::var("RUSH_SESSION").unwrap_or_default(),
            host: Box::new(FilesHost),
            start: None,
        }
    }
}

/// rush pr with args, as the process it runs in: its exit code.
pub fn pr_cmd(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    pr_cmd_in(&Env::from_process(), args, stdout, stderr)
}

/// rush pr with args, run in env.
pub fn pr_cmd_in(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        let _ = write!(stdout, "{PR_USAGE}\n{PLAYBOOK}");
        return 0;
    }
    if !rush_watch::bundled_on("drive") {
        let _ = writeln!(
            stderr,
            "rush pr: the drive plugin is off: \"rush plugin on drive\" turns it on"
        );
        return 1;
    }
    rush_watch::stop::on_signals();
    let mut fail = |err: &str| {
        let _ = writeln!(stderr, "rush pr: {err}");
        1
    };
    let dir = env.dir.clone();
    let resolve = |what: &str| rush_watch::resolve(&dir, what);
    match args[0].as_str() {
        "state" | "status" => {
            let prs: Vec<String> = if args.len() > 1 {
                args[1..].to_vec()
            } else {
                vec![String::new()]
            };
            let mut out = Vec::new();
            for what in &prs {
                let (repo, n) = match resolve(what) {
                    Ok(v) => v,
                    Err(e) => return fail(&e),
                };
                match follow::look(&repo, n) {
                    Ok(s) => out.push(render(&s)),
                    Err(e) => return fail(&e),
                }
            }
            let _ = writeln!(stdout, "{}", out.join("\n\n"));
            0
        }
        "reply" => {
            if args.len() != 4 {
                return fail("usage: rush pr reply <pr> <thread> <text>");
            }
            let (repo, n) = match resolve(&args[1]) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            if let Err(e) = crate::reply::reply(&repo, n, &args[2], &args[3], &Trust::new(&repo)) {
                return fail(&e);
            }
            let _ = writeln!(stdout, "replied");
            0
        }
        "resolve" => {
            if args.len() < 3 {
                return fail("usage: rush pr resolve <pr> <thread>...");
            }
            if let Err(e) = crate::reply::resolve(&args[2..]) {
                return fail(&e);
            }
            let _ = writeln!(stdout, "resolved {}", args.len() - 2);
            0
        }
        "follow" | "unfollow" | "following" => pr_follow_cmd(env, args, stdout, stderr),
        // The detached follower rush pr follow starts: not meant to be run by hand.
        "follow-run" => pr_follow_run(env, &args[1..], stderr),
        other => {
            let _ = write!(stderr, "rush pr: no {}\n\n{PR_USAGE}", quote(other));
            1
        }
    }
}

/// rush pr follow, unfollow and following, for the session rush runs this
/// in.
fn pr_follow_cmd(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let session = env.session.as_str();
    if session.is_empty() {
        let _ = writeln!(
            stderr,
            "rush pr: only a session rush runs can be followed for: wait with rush watch pr instead"
        );
        return 1;
    }
    let resolve = |what: &str| rush_watch::resolve(&env.dir, what);
    match args[0].as_str() {
        "following" => {
            for e in follow::following(session) {
                let _ = writeln!(
                    stdout,
                    "{}#{} since {}",
                    e.repo,
                    e.number,
                    clock_and_day(e.started)
                );
            }
            return 0;
        }
        "unfollow" => {
            let mut list = follow::following(session);
            if args.len() > 1 {
                list.clear();
                for what in &args[1..] {
                    match resolve(what) {
                        Ok((repo, n)) => list.push(Entry::new(session, &repo, n)),
                        Err(e) => {
                            let _ = writeln!(stderr, "rush pr: {e}");
                            return 1;
                        }
                    }
                }
            }
            for e in &list {
                if let Err(err) = follow::stop(e) {
                    let _ = writeln!(stderr, "rush pr: {err}");
                    return 1;
                }
                let _ = writeln!(stdout, "stopped following {}#{}", e.repo, e.number);
            }
            return 0;
        }
        _ => {}
    }
    let prs: Vec<String> = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        vec![String::new()]
    };
    let start = |e: Entry| match &env.start {
        Some(f) => f(e),
        None => follow::start(e),
    };
    let (out, res) =
        follow::follow_all_with(session, &prs, &resolve, &start, &|r, n| follow::look(r, n));
    let _ = writeln!(stdout, "{out}");
    if let Err(e) = res {
        let _ = writeln!(stderr, "rush pr: {e}");
        return 1;
    }
    0
}

/// The detached follower follow::start starts.
fn pr_follow_run(env: &Env, args: &[String], stderr: &mut dyn Write) -> i32 {
    let mut fs = FlagSet::new("rush pr follow-run");
    fs.add("session", Kind::String, "");
    fs.add("repo", Kind::String, "");
    fs.add("pr", Kind::Int, "0");
    // Go's flag writes its own complaints to the process's stderr.
    let parsed = fs.parse(args, &mut std::io::stderr());
    let (session, repo, n) = (
        fs.get("session").to_string(),
        fs.get("repo").to_string(),
        fs.int("pr"),
    );
    if parsed.is_err() || session.is_empty() || repo.is_empty() || n == 0 {
        let _ = writeln!(
            stderr,
            "usage: rush pr follow-run --session ID --repo OWNER/NAME --pr N"
        );
        return 1;
    }
    let Some(e) = follow::hold(Entry::new(&session, &repo, n)) else {
        return 0; // another follower has it
    };
    let host = &env.host;
    let o = Options {
        entry: e,
        // Off ends it.
        alive: Some(Box::new(|| {
            host.alive(&session) && rush_watch::bundled_on("drive")
        })),
        // As the list ages it.
        last_turn: Some(Box::new(|| host.last_turn(&session))),
        deliver: Some(Box::new(|text: &str| {
            let ex = Exchange {
                at: 0,
                archived: false,
                id: random_id(),
                direction: "received".into(),
                phase: "message".into(),
                text: text.into(),
                images: Vec::new(),
                sender: Peer {
                    session_id: String::new(),
                    kind: "rush".into(),
                    name: format!("rush · {repo}#{n}"),
                },
                receiver: Peer::default(),
            };
            host.deliver(&session, &ex)
        })),
        ..Default::default()
    };
    match follow::follow(&o) {
        Err(err) if !err.contains("context canceled") => {
            let _ = writeln!(stderr, "rush pr follow: {err}");
            1
        }
        _ => 0,
    }
}

/// 16 random bytes as hex: an exchange's id.
fn random_id() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// t as Go's Format("15:04 Jan 2") writes it, in the zone it was written
/// in.
fn clock_and_day(t: Time) -> String {
    let s = t.format(); // 2026-10-21T12:00:00…
    let month: usize = s.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    let day: u32 = s.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(1);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {day}",
        s.get(11..16).unwrap_or("00:00"),
        MONTHS[month.clamp(1, 12) - 1]
    )
}

/// What rush pr help prints: the usage, then how to drive a PR.
pub fn help() -> String {
    format!("{PR_USAGE}\n{PLAYBOOK}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_when_as_go_s_format_does() {
        for (t, want) in [
            ("2026-10-21T12:04:00.5+01:00", "12:04 Oct 21"),
            ("2026-01-02T03:04:05Z", "03:04 Jan 2"),
        ] {
            assert_eq!(clock_and_day(Time::parse(t).unwrap()), want);
        }
    }

    #[test]
    fn a_session_rests_from_when_its_last_turn_ended() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let dir = home.path().join("sessions").join("s1");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(last_turn_from_info("s1").is_zero(), "no info");
        assert!(!session_config_readable("s1"));
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        assert!(session_config_readable("s1"));
        let me = std::process::id();
        let info = |state: &str| {
            format!(
                r#"{{"hostPid":{me},"state":"{state}","updatedAt":"2026-10-21T12:00:00Z","idleSince":"2026-10-21T11:00:00Z"}}"#
            )
        };
        std::fs::write(dir.join("info.json"), info("working")).unwrap();
        assert!(last_turn_from_info("s1").is_zero(), "working");
        std::fs::write(dir.join("info.json"), info("idle")).unwrap();
        assert_eq!(last_turn_from_info("s1").format(), "2026-10-21T11:00:00Z");
        // A host that's gone isn't working, whatever it last wrote.
        std::fs::write(
            dir.join("info.json"),
            r#"{"hostPid":0,"state":"working","updatedAt":"2026-10-21T12:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(last_turn_from_info("s1").format(), "2026-10-21T12:00:00Z");
        rush_state::set_thread_dir(None);
    }
}
