//! rush watch, the command: its words, its flags as Go's flag package
//! reads them, and its exit codes.
//! Ported from cmd/rush/watch.go, with internal/bundled/prwatch/plugin.go:34
//! (On), internal/plugin/bundled.go:99-140 (whether a bundled plugin is on)
//! and internal/host/events.go:18-31 (LogEvent).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::go::{duration_ns, parse_duration, quote};
use crate::pr::{EXIT_ERROR, Options, Until, resolve, watch_pr};

pub const WATCH_USAGE: &str = "rush watch: wait on something without polling it yourself. rush asks for
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

/// Where rush watch runs: its folder, the session it's run for, and the
/// task it runs as. [`Env::from_process`] reads them as Go does.
pub struct Env {
    pub dir: PathBuf,
    /// RUSH_SESSION: "" when rush doesn't run it.
    pub session: String,
    /// The background task it runs as, if it is one.
    pub task: String,
    /// GitHub, the clock and the log, for tests; None, the real ones.
    pub options: Option<Box<dyn Fn(&mut Options<'_>)>>,
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
    watch_cmd_in(&Env::from_process(), args, stdout, stderr)
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
    let mut flags = Flags {
        until: "done".into(),
        timeout: 25 * 60 * 1_000_000_000,
    };
    let left = match flags.parse(rest, stderr) {
        Ok(left) => left,
        Err(()) => return EXIT_ERROR,
    };
    if !left.is_empty() && what.is_empty() {
        what = left[0].clone();
    }
    let Some(until) = Until::parse(&flags.until) else {
        let _ = writeln!(
            stderr,
            "rush watch: --until is done, fail, first or each, not {}",
            quote(&flags.until)
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
        timeout: Duration::from_nanos(flags.timeout.max(0) as u64),
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

/// rush watch pr's flags, read as Go's flag package reads them.
struct Flags {
    until: String,
    /// Nanoseconds, as Go's time.Duration.
    timeout: i64,
}

impl Flags {
    /// Reads args up to the first that isn't a flag, and gives the rest.
    /// What's wrong goes to stderr with the usage, as Go's flag says it.
    fn parse(&mut self, args: &[String], stderr: &mut dyn Write) -> Result<Vec<String>, ()> {
        let mut i = 0;
        while i < args.len() {
            let s = &args[i];
            if s.len() < 2 || !s.starts_with('-') {
                break;
            }
            let mut name = &s[1..];
            if let Some(n) = name.strip_prefix('-') {
                if n.is_empty() {
                    // "--" ends the flags
                    i += 1;
                    break;
                }
                name = n;
            }
            if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
                return self.fail(stderr, &format!("bad flag syntax: {s}"));
            }
            i += 1;
            let (name, value) = match name[1..].find('=') {
                Some(eq) => (&name[..eq + 1], Some(name[eq + 2..].to_string())),
                None => (name, None),
            };
            if name != "until" && name != "timeout" {
                if name == "help" || name == "h" {
                    self.usage(stderr);
                    return Err(());
                }
                return self.fail(stderr, &format!("flag provided but not defined: -{name}"));
            }
            let value = match value {
                Some(v) => v,
                None if i < args.len() => {
                    i += 1;
                    args[i - 1].clone()
                }
                None => {
                    return self.fail(stderr, &format!("flag needs an argument: -{name}"));
                }
            };
            if name == "until" {
                self.until = value;
            } else {
                match parse_duration(&value) {
                    Some(d) => self.timeout = d,
                    None => {
                        self.timeout = 0;
                        return self.fail(
                            stderr,
                            &format!(
                                "invalid value {} for flag -{name}: parse error",
                                quote(&value)
                            ),
                        );
                    }
                }
            }
        }
        Ok(args[i..].to_vec())
    }

    fn fail<T>(&self, stderr: &mut dyn Write, msg: &str) -> Result<T, ()> {
        let _ = writeln!(stderr, "{msg}");
        self.usage(stderr);
        Err(())
    }

    /// Go's defaultUsage: the flags in name order, each with its default.
    fn usage(&self, stderr: &mut dyn Write) {
        let _ = write!(
            stderr,
            "Usage of rush watch pr:\n  -timeout duration\n    \t (default {})\n  -until string\n    \t (default \"done\")\n",
            duration_ns(25 * 60 * 1_000_000_000)
        );
    }
}
