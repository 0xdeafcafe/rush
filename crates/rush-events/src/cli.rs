//! The read side of `rush events` and `rush why`, as the binary will call
//! them: the same flags, words, output and exit codes as Go. Ported from
//! Go `cmd/rush/events.go` and `idAndFlags` (`cmd/rush/session.go:115`).

use std::io::Write;
use std::path::Path;

use rush_core::time::Ms;

use crate::flags::{Kind, Parsed, parse};
use crate::query::{
    BUDGET, Filter, Line, chain, flat, format, query, query_why, sessions_starting, tree,
};
use crate::since::parse_since;

/// rush events -h (events.go:13), with what rush took on since: a
/// session by the start of its id, -n 0 for all, why's --all.
pub const EVENTS_USAGE: &str = "rush events: what rush did and why, newest first, a line each

  rush events [-s SESSION] [-k kind,kind] [-p proc] [--since 10m] [--grep TEXT] [-n 20] [--json]
  rush events --chain EV    one line's chain: what led to it and what came of that
  rush why SESSION [--last 1h] [--all]
                            that session's lines, its parent's, children's and
                            senders', and rush's own failures, each under its cause

  A row is: how long ago, kind, session, process:pid, #ev ←cause, then its fields.
  -s takes a session's id or the start of it. -k 'hook*' is every kind hook
  starts (quote the * from your shell). --since takes 90s, 10m, 2h, 3d, 1d12h
  or a time (2026-10-23T09:00). -n 0 prints them all. --json prints the lines
  as they're kept. why leaves out state and task lines; --all keeps them.
";

/// rush why's usage, said when it's asked for or given nothing.
const WHY_USAGE: &str = "usage: rush why SESSION [--last 1h] [--all] [--json]\n";

/// The kinds that say how a session stands rather than what happened to
/// it: a session's state and its background tasks starting and ending.
/// rush why leaves them out unless --all, or one led to a line it keeps;
/// they were most of a busy session's story.
const WHY_QUIET: [&str; 3] = ["state", "task-started", "task-done"];

/// What a cut-short answer says to narrow it with.
pub const NARROW_EVENTS: &str = "-s, -k, -p, --since or --grep";

const EVENTS_FLAGS: [(&str, Kind); 9] = [
    ("s", Kind::Str),
    ("k", Kind::Str),
    ("p", Kind::Str),
    ("since", Kind::Str),
    ("grep", Kind::Str),
    ("chain", Kind::Str),
    ("n", Kind::Int),
    ("json", Kind::Bool),
    ("h", Kind::Bool),
];

/// rush events, reading the log at log (events.go:29). args are those after
/// "events"; now is the time rows are aged from. Returns the exit code.
pub fn events_cmd(
    log: &Path,
    args: &[String],
    now: Ms,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    // What's wrong is said above the usage; Go said only the usage.
    let (p, wrong) = match parse(&EVENTS_FLAGS, args) {
        Ok(p) => (p, None),
        Err((p, e)) if e == "flag: help requested" => (p, Some(String::new())),
        Err((p, e)) => (p, Some(e)),
    };
    let help = p.bool("h") || wrong.as_deref() == Some("");
    let wrong = match (wrong, p.args.first()) {
        (Some(e), _) if !e.is_empty() => Some(e),
        (_, Some(a)) if !help => Some(format!("{a:?} isn't a flag; rush events takes only flags")),
        _ if p.int("n", 20) < 0 => Some("-n takes how many lines, or 0 for all of them".into()),
        _ => None,
    };
    if help || wrong.is_some() {
        if let Some(e) = &wrong {
            let _ = writeln!(stderr, "rush events: {e}\n");
        }
        let _ = stderr.write_all(EVENTS_USAGE.as_bytes());
        return if wrong.is_none() { 0 } else { 2 };
    }
    let raw = p.bool("json");
    let ch = p.str("chain", "");
    if !ch.is_empty() {
        return match chain(log, ch.strip_prefix('#').unwrap_or(&ch)) {
            Ok(lines) => print_lines(stdout, &lines, raw, now, "--json for all of it"),
            Err(e) => {
                let _ = writeln!(stderr, "rush events: {e}");
                1
            }
        };
    }
    let split = |name: &str| -> Vec<String> {
        let v = p.str(name, "");
        if v.is_empty() {
            Vec::new()
        } else {
            v.split(',').map(str::to_string).collect()
        }
    };
    let mut f = Filter {
        sessions: split("s"),
        kinds: split("k"),
        proc: p.str("p", ""),
        grep: p.str("grep", ""),
        limit: p.int("n", 20) as usize,
        since: 0,
    };
    f.since = match parse_since(&p.str("since", ""), now) {
        Ok(t) => t,
        Err(e) => {
            let _ = writeln!(stderr, "rush events: {e}");
            return 2;
        }
    };
    let (lines, more) = match query(log, &f) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(stderr, "rush events: {e}");
            return 1;
        }
    };
    if lines.is_empty() {
        let _ = writeln!(stderr, "rush events: none match");
        return 1;
    }
    if raw {
        print_raw(stdout, &lines);
        return 0;
    }
    let narrow = format!("{NARROW_EVENTS}, or -n to raise the limit");
    let _ = stdout.write_all(format(&flat(&lines), now, BUDGET, more, &narrow).as_bytes());
    0
}

const WHY_FLAGS: [(&str, Kind); 3] = [
    ("last", Kind::Str),
    ("json", Kind::Bool),
    ("all", Kind::Bool),
];

/// rush why: a session's story, each line under its cause (events.go:96).
/// linked is the sessions a session's config and the others' say it's
/// linked to: the one that started it and the ones it started (Go
/// `linkedSessions`, which reads the host's configs).
pub fn why_cmd(
    log: &Path,
    args: &[String],
    now: Ms,
    linked: &dyn Fn(&str) -> Vec<String>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-h" | "--h" | "-help" | "--help"))
    {
        let _ = stderr.write_all(WHY_USAGE.as_bytes());
        return 0;
    }
    let Some((mut id, p)) = id_and_flags(&WHY_FLAGS, args) else {
        let _ = stderr.write_all(WHY_USAGE.as_bytes());
        return 2;
    };
    let last = p.str("last", "1h");
    let since = match parse_since(&last, now) {
        Ok(t) => t,
        Err(e) => {
            let _ = writeln!(stderr, "rush why: {e}");
            return 2;
        }
    };
    // A session by the start of its id, as rush's lists show it, when
    // that start is one session's alone.
    match sessions_starting(log, &id, since) {
        Ok(s) if s.len() == 1 => id = s[0].clone(),
        Ok(s) if s.len() > 1 && !s.contains(&id) => {
            let _ = writeln!(
                stderr,
                "rush why: {id} is the start of {}; say more of it",
                s.join(", ")
            );
            return 2;
        }
        Ok(_) => {}
        Err(e) => {
            let _ = writeln!(stderr, "rush why: {e}");
            return 1;
        }
    }
    let mut lines = match query_why(log, &id, &linked(&id), since) {
        Ok(l) => l,
        Err(e) => {
            let _ = writeln!(stderr, "rush why: {e}");
            return 1;
        }
    };
    // "in the last 1h", or "since 2026-10-23T09:00" for a time.
    let span = if last.len() >= 10 && last.as_bytes()[4] == b'-' {
        format!("since {last}")
    } else {
        format!("in the last {last}")
    };
    if lines.is_empty() {
        let _ = writeln!(stderr, "rush why: nothing of {id}'s {span}");
        return 1;
    }
    let mut quiet = 0;
    if !p.bool("all") && !p.bool("json") {
        let causes: std::collections::HashSet<String> = lines
            .iter()
            .filter(|l| !l.cause.is_empty())
            .map(|l| l.cause.clone())
            .collect();
        let before = lines.len();
        lines.retain(|l| !WHY_QUIET.contains(&l.kind.as_str()) || causes.contains(&l.ev));
        quiet = before - lines.len();
    }
    if lines.is_empty() {
        let _ = writeln!(
            stderr,
            "rush why: only state and task lines of {id}'s {span}; --all shows them"
        );
        return 1;
    }
    let narrow = format!("--last, or rush events -s {id} -k KIND");
    let code = print_lines(stdout, &lines, p.bool("json"), now, &narrow);
    if quiet > 0 {
        let _ = writeln!(
            stdout,
            "… {quiet} state and task lines left out; --all shows them"
        );
    }
    code
}

/// A command's id, first or after its flags, and its flags (session.go:115).
fn id_and_flags(flags: &[(&'static str, Kind)], args: &[String]) -> Option<(String, Parsed)> {
    let (mut id, rest) = match args.first() {
        Some(a) if !a.starts_with('-') => (a.clone(), &args[1..]),
        _ => (String::new(), args),
    };
    let p = parse(flags, rest).ok()?;
    if id.is_empty()
        && let Some(a) = p.args.first()
    {
        id = a.clone();
    }
    (!id.is_empty()).then_some((id, p))
}

/// Prints lines (oldest first) under their causes, or as kept.
fn print_lines(w: &mut dyn Write, lines: &[Line], raw: bool, now: Ms, narrow: &str) -> i32 {
    if raw {
        print_raw(w, lines);
        return 0;
    }
    let _ = w.write_all(format(&tree(lines), now, BUDGET, false, narrow).as_bytes());
    0
}

fn print_raw(w: &mut dyn Write, lines: &[Line]) {
    for l in lines {
        let _ = w.write_all(&l.raw);
        let _ = w.write_all(b"\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TempDir;

    const MIN: i64 = 60_000;

    fn t(ms: Ms) -> String {
        crate::log::rfc3339_nano(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64),
        )
    }

    fn log(now: Ms) -> (TempDir, std::path::PathBuf) {
        let home = TempDir::new();
        let p = home.0.join("events.jsonl");
        let lines = [
            format!(
                r#"{{"ev":"aS","kind":"agent-started","pid":9,"proc":"host","session":"A","t":"{}"}}"#,
                t(now - 50 * MIN)
            ),
            format!(
                r#"{{"cause":"aS","ev":"wk","kind":"woken","proc":"session","session":"B","t":"{}"}}"#,
                t(now - 40 * MIN)
            ),
            format!(
                r#"{{"cause":"wk","ev":"dl","from":"A","kind":"delivered","pid":10,"proc":"host","session":"B","t":"{}","text":"hi there","turn":1}}"#,
                t(now - 39 * MIN)
            ),
            format!(
                r#"{{"ev":"ok","kind":"saved","file":"config.json","proc":"ui","t":"{}"}}"#,
                t(now - 2 * MIN)
            ),
        ];
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        (home, p)
    }

    fn run(f: impl FnOnce(&mut Vec<u8>, &mut Vec<u8>) -> i32) -> (i32, String, String) {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let code = f(&mut o, &mut e);
        (
            code,
            String::from_utf8(o).unwrap(),
            String::from_utf8(e).unwrap(),
        )
    }

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn events_prints_as_go_does() {
        let now = 1_793_190_896_000;
        let (_home, p) = log(now);
        let extra = format!(
            "rush events: \"extra\" isn't a flag; rush events takes only flags\n\n{EVENTS_USAGE}"
        );
        let bogus = format!("rush events: flag provided but not defined: -bogus\n\n{EVENTS_USAGE}");
        let nabc = format!("rush events: invalid value \"abc\" for flag -n\n\n{EVENTS_USAGE}");
        let nneg =
            format!("rush events: -n takes how many lines, or 0 for all of them\n\n{EVENTS_USAGE}");
        let snone = format!("rush events: flag needs an argument: -s\n\n{EVENTS_USAGE}");
        for (a, code, out, err) in [
            (
                &["-n", "2"][..],
                0,
                "2m  saved  ui  #ok  file=config.json\n\
                 39m  delivered  s=B  host:10  #dl ←wk  from=A  turn=1  text=\"hi there\"\n\
                 … more before these; narrow with -s, -k, -p, --since or --grep, or -n to raise the limit\n",
                "",
            ),
            (
                &["-s", "A"],
                0,
                "50m  agent-started  s=A  host:9  #aS\n",
                "",
            ),
            (&["-k", "sav*", "--json"], 0, "", ""),
            (&["-p", "nobody"], 1, "", "rush events: none match\n"),
            (
                &["--since", "soon"],
                2,
                "",
                "rush events: \"soon\" is neither a while back (10m, 2h, 3d) nor a time (2026-10-23T09:00)\n",
            ),
            (&["-h"], 0, "", EVENTS_USAGE),
            (&["--help"], 0, "", EVENTS_USAGE),
            (&["extra"], 2, "", extra.as_str()),
            (&["-bogus"], 2, "", bogus.as_str()),
            (&["-n", "abc"], 2, "", nabc.as_str()),
            (&["-n", "-1"], 2, "", nneg.as_str()),
            (&["-s"], 2, "", snone.as_str()),
            (
                &["--chain", "#nope"],
                1,
                "",
                "rush events: no line in rush's log is nope\n",
            ),
            (
                &["--chain", "dl"],
                0,
                "50m  agent-started  s=A  host:9  #aS\n  \
                 40m  woken  s=B  session  #wk ←aS\n    \
                 39m  delivered  s=B  host:10  #dl ←wk  from=A  turn=1  text=\"hi there\"\n",
                "",
            ),
        ] {
            let (c, o, e) = run(|o, e| events_cmd(&p, &args(a), now, o, e));
            assert_eq!((c, e.as_str()), (code, err), "{a:?}");
            if a.contains(&"--json") {
                assert!(o.starts_with(r#"{"ev":"ok","kind":"saved""#), "{o}");
            } else {
                assert_eq!(o, out, "{a:?}");
            }
        }
    }

    #[test]
    fn why_leaves_out_state_and_takes_an_ids_start() {
        let now = 1_793_190_896_000;
        let home = TempDir::new();
        let p = home.0.join("events.jsonl");
        let lines = [
            format!(
                r#"{{"ev":"st","kind":"state","proc":"host","session":"abc1","t":"{}"}}"#,
                t(now - 5 * MIN)
            ),
            format!(
                r#"{{"ev":"tk","kind":"task-started","proc":"host","session":"abc1","t":"{}"}}"#,
                t(now - 4 * MIN)
            ),
            format!(
                r#"{{"cause":"tk","ev":"dl","kind":"delivered","proc":"host","session":"abc1","t":"{}"}}"#,
                t(now - 3 * MIN)
            ),
            format!(
                r#"{{"ev":"td","kind":"task-done","proc":"host","session":"abc1","t":"{}"}}"#,
                t(now - 2 * MIN)
            ),
            format!(
                r#"{{"ev":"o2","kind":"turn-end","proc":"host","session":"abc2","t":"{}"}}"#,
                t(now - MIN)
            ),
        ];
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        let none = |_: &str| Vec::new();
        let (c, o, e) = run(|o, e| why_cmd(&p, &args(&["abc1"]), now, &none, o, e));
        assert_eq!((c, e.as_str()), (0, ""));
        assert_eq!(
            o,
            "4m  task-started  s=abc1  host  #tk\n  3m  delivered  s=abc1  host  #dl ←tk\n\
             … 2 state and task lines left out; --all shows them\n",
            "a task line that led to one kept stays"
        );
        let (c, o, _) = run(|o, e| why_cmd(&p, &args(&["abc1", "--all"]), now, &none, o, e));
        assert!(
            c == 0 && o.contains("#st") && o.contains("#td") && !o.contains("left out"),
            "{o}"
        );
        let (c, _, e) = run(|o, e| why_cmd(&p, &args(&["abc"]), now, &none, o, e));
        assert_eq!(
            (c, e.as_str()),
            (
                2,
                "rush why: abc is the start of abc2, abc1; say more of it\n"
            )
        );
        let (c, o, _) = run(|o, e| why_cmd(&p, &args(&["abc2"]), now, &none, o, e));
        assert_eq!((c, o.as_str()), (0, "1m  turn-end  s=abc2  host  #o2\n"));
        let (c, _, e) = run(|o, e| {
            why_cmd(
                &p,
                &args(&["abc1", "--last", "2030-01-01T00:00"]),
                now,
                &none,
                o,
                e,
            )
        });
        assert_eq!(
            (c, e.as_str()),
            (1, "rush why: nothing of abc1's since 2030-01-01T00:00\n")
        );
    }

    #[test]
    fn why_prints_as_go_does() {
        let now = 1_793_190_896_000;
        let (_home, p) = log(now);
        let linked = |id: &str| {
            if id == "C" {
                vec!["A".to_string()]
            } else {
                Vec::new()
            }
        };
        for (a, code, out, err) in [
            (
                &["B"][..],
                0,
                "50m  agent-started  s=A  host:9  #aS\n  \
                 40m  woken  s=B  session  #wk ←aS\n    \
                 39m  delivered  s=B  host:10  #dl ←wk  from=A  turn=1  text=\"hi there\"\n",
                "",
            ),
            (
                &["--last", "45m", "B"],
                0,
                "40m  woken  s=B  session  #wk ←aS\n  39m  delivered  s=B  host:10  #dl ←wk  from=A  turn=1  text=\"hi there\"\n",
                "",
            ),
            (&["C"], 0, "50m  agent-started  s=A  host:9  #aS\n", ""),
            (
                &["B", "--last", "10m"],
                1,
                "",
                "rush why: nothing of B's in the last 10m\n",
            ),
            (
                &["B", "--last", "soon"],
                2,
                "",
                "rush why: \"soon\" is neither a while back (10m, 2h, 3d) nor a time (2026-10-23T09:00)\n",
            ),
            (&[], 2, "", WHY_USAGE),
            (&["-h"], 0, "", WHY_USAGE),
            (&["B", "--help"], 0, "", WHY_USAGE),
        ] {
            let (c, o, e) = run(|o, e| why_cmd(&p, &args(a), now, &linked, o, e));
            assert_eq!((c, o.as_str(), e.as_str()), (code, out, err), "{a:?}");
        }
    }
}
