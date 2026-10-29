//! rush watch, the command: its words, its flags as Go's flag package
//! reads them, and its exit codes.
//! Ported from cmd/rush/watch.go, with internal/bundled/prwatch/plugin.go:34
//! (On), internal/plugin/bundled.go:99-140 (whether a bundled plugin is on)
//! and internal/host/events.go:18-31 (LogEvent).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::go::quote;
use crate::goflag::{FlagSet, Kind};
use crate::pr::{EXIT_ERROR, Options, Until, resolve, watch_pr};

pub const WATCH_USAGE: &str =
    "rush watch: wait on something without polling it yourself. rush asks for
you, cheaply, and says once, briefly, what happened.

  rush watch pr [<n> | owner/name#<n> | <url>] [--until U] [--timeout D]
                    a pull request's checks; with no PR, this branch's

  --until done      every check finished, or it merged or closed (the default)
          fail      the first check to fail, else as done
          first     the first check to finish, or a new commit
          each      a line as each check finishes, then as done: for a monitor
  --timeout 25m     give up then, saying how far it got (0: never)

It ends with: 0 all passed (or merged), 1 a check failed, 2 timed out
(run it again), 3 closed, or GitHub couldn't be asked.

It asks GitHub through gh, signed in as the user, every 15s at first and
every minute after 15m; every agent watching the same PR shares one answer,
and an answer that hasn't changed costs none of the user's rate limit.
Run it as a background command where your harness has them, so it wakes
you when it ends, and spend nothing while it waits.
";

/// Whether you have the bundled plugin name on. Optional bundled plugins
/// (drive, watch) are on when plugins/bundled-on.json lists them.
pub fn bundled_on(name: &str) -> bool {
    std::fs::read(rush_state::dir().join("plugins").join("bundled-on.json"))
        .ok()
        .and_then(|b| rush_state::json::from_slice::<Vec<String>>(&b).ok())
        .is_some_and(|l| l.iter().any(|n| n == name))
}

/// A session's own log: host.EventsPath.
pub fn session_events_path(id: &str) -> PathBuf {
    rush_state::dir()
        .join("sessions")
        .join(id)
        .join("events.jsonl")
}

/// Records an event of a session's, in its own log and rush's: Go's
/// host.LogEvent.
pub fn log_session_event(id: &str, kind: &str, fields: Map<String, Value>) {
    if id.is_empty() {
        return;
    }
    rush_events::session(id, &session_events_path(id), kind, fields);
}

/// Sets a watch's options before it runs: GitHub, the clock and the log,
/// for tests.
pub type SetOptions = Box<dyn Fn(&mut Options<'_>)>;

/// Where rush watch runs: its folder, the session it's run for, and the
/// task it runs as. [`Env::from_process`] reads them as Go does.
pub struct Env {
    pub dir: PathBuf,
    /// RUSH_SESSION: "" when rush doesn't run it.
    pub session: String,
    /// The background task it runs as, if it is one.
    pub task: String,
    /// GitHub, the clock and the log, for tests; None, the real ones.
    pub options: Option<SetOptions>,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            dir: std::env::current_dir().unwrap_or_default(),
            session: std::env::var("RUSH_SESSION").unwrap_or_default(),
            task: crate::live::task_id(),
            options: None,
        }
    }
}

/// rush watch with args, as the process it runs in: its exit code.
pub fn watch_cmd(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let code = watch_cmd_in(&Env::from_process(), args, stdout, stderr);
    // What it logged is written before the process ends.
    rush_events::flush();
    code
}

/// rush watch with args, run in env.
pub fn watch_cmd_in(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        let _ = stdout.write_all(WATCH_USAGE.as_bytes());
        return 0;
    }
    if !bundled_on("watch") {
        let _ = writeln!(
            stderr,
            "rush watch: the watch plugin is off: \"rush plugin on watch\" turns it on"
        );
        return EXIT_ERROR;
    }
    if args[0] != "pr" {
        let _ = write!(
            stderr,
            "rush watch: no {} to watch\n\n{WATCH_USAGE}",
            quote(&args[0])
        );
        return EXIT_ERROR;
    }
    // The PR may come before or after the flags.
    let mut what = String::new();
    let mut rest = &args[1..];
    if let Some(first) = rest.first()
        && !first.is_empty()
        && !first.starts_with('-')
    {
        what = first.clone();
        rest = &rest[1..];
    }
    let mut fs = FlagSet::new("rush watch pr");
    fs.add("until", Kind::String, Until::Done.as_str());
    fs.add("timeout", Kind::Duration, "25m0s");
    let Ok(left) = fs.parse(rest, stderr) else {
        return EXIT_ERROR;
    };
    if !left.is_empty() && what.is_empty() {
        what = left[0].clone();
    }
    let until_word = fs.get("until").to_string();
    let Some(until) = Until::parse(&until_word) else {
        let _ = writeln!(
            stderr,
            "rush watch: --until is done, fail, first or each, not {}",
            quote(&until_word)
        );
        return EXIT_ERROR;
    };
    crate::stop::on_signals();
    let session = env.session.clone();
    let log = move |kind: &str, f: Map<String, Value>| {
        if session.is_empty() {
            rush_events::log(kind, f)
        } else {
            log_session_event(&session, kind, f)
        }
    };
    let mut o = Options {
        until,
        // A negative timeout is never, as Go's is.
        timeout: Duration::from_nanos(fs.duration("timeout").max(0) as u64),
        task: env.task.clone(),
        ..Default::default()
    };
    if let Some(set) = &env.options {
        set(&mut o);
    }
    let log: crate::pr::LogFn<'_> = match o.log.take() {
        Some(l) => l,
        None => Box::new(log),
    };
    let (repo, n) = match resolve(Path::new(&env.dir), &what) {
        Ok(v) => v,
        Err(e) => {
            let mut f = Map::new();
            f.insert("what".into(), what.clone().into());
            f.insert("err".into(), e.clone().into());
            log("watch-pr-failed", f);
            let _ = writeln!(stderr, "rush watch: {e}");
            return EXIT_ERROR;
        }
    };
    o.repo = repo;
    o.number = n;
    o.log = Some(log);
    // Say writes each line as it's said, as Go's Fprintln does.
    let out = std::cell::RefCell::new(stdout);
    o.say = Some(Box::new(move |s: &str| {
        let mut w = out.borrow_mut();
        let _ = writeln!(w, "{s}");
        let _ = w.flush();
    }));
    watch_pr(&o)
}
