//! The read side of `rush events` and `rush why`, as the binary will call
//! them: the same flags, words, output and exit codes as Go. Ported from
//! Go `cmd/rush/events.go` and `idAndFlags` (`cmd/rush/session.go:115`).

use std::io::Write;
use std::path::Path;

use rush_core::time::Ms;

use crate::flags::{Kind, Parsed, parse};
use crate::query::{BUDGET, Filter, Line, chain, flat, format, query, query_why, tree};
use crate::since::parse_since;

/// rush events -h (events.go:13).
pub const EVENTS_USAGE: &str = "rush events: what rush did and why, newest first, a line each

  rush events [-s SESSION] [-k kind,kind] [-p proc] [--since 10m] [--grep TEXT] [-n 20] [--json]
  rush events --chain EV    one line's chain: what led to it and what came of that
  rush why SESSION [--last 1h]
                            that session's lines, its parent's, children's and
                            senders', and rush's own failures, each under its cause

  A row is: how long ago, kind, session, process:pid, #ev ←cause, then its fields.
  -k takes kind* for every kind it starts. --since takes 90s, 10m, 2h, 3d or a
  time (2026-10-23T09:00). --json prints the lines as they're kept.
";

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
    let (p, failed) = match parse(&EVENTS_FLAGS, args) {
        Ok(p) => (p, false),
        Err((p, _)) => (p, true),
    };
    let help = p.bool("h");
    if failed || help || !p.args.is_empty() {
        let _ = stderr.write_all(EVENTS_USAGE.as_bytes());
        return if help { 0 } else { 2 };
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
        limit: p.int("n", 20).max(0) as usize,
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

const WHY_FLAGS: [(&str, Kind); 2] = [("last", Kind::Str), ("json", Kind::Bool)];

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
    let Some((id, p)) = id_and_flags(&WHY_FLAGS, args) else {
        let _ = writeln!(stderr, "usage: rush why SESSION [--last 1h] [--json]");
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
    let lines = match query_why(log, &id, &linked(&id), since) {
        Ok(l) => l,
        Err(e) => {
            let _ = writeln!(stderr, "rush why: {e}");
            return 1;
        }
    };
    if lines.is_empty() {
        let _ = writeln!(stderr, "rush why: nothing of {id}'s in the last {last}");
        return 1;
    }
    let narrow = format!("--last, or rush events -s {id} -k");
    print_lines(stdout, &lines, p.bool("json"), now, &narrow)
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
            (&["extra"], 2, "", EVENTS_USAGE),
            (&["-bogus"], 2, "", EVENTS_USAGE),
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
            (&[], 2, "", "usage: rush why SESSION [--last 1h] [--json]\n"),
            (
                &["-h"],
                2,
                "",
                "usage: rush why SESSION [--last 1h] [--json]\n",
            ),
        ] {
            let (c, o, e) = run(|o, e| why_cmd(&p, &args(a), now, &linked, o, e));
            assert_eq!((c, o.as_str(), e.as_str()), (code, out, err), "{a:?}");
        }
    }
}
