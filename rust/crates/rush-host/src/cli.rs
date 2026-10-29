//! The host's commands, as the rush binary runs them: `rush host`, `rush
//! attach`, `rush view` / `rush views`, `rush inbox`, `rush reload` and
//! `rush remind`. Each returns its exit status; output and words are Go's.
//!
//! Ported from cmd/rush/main.go:99-200 (attach, host, view, inbox, reload),
//! cmd/rush/view.go (viewCmd), cmd/rush/reload.go (reloadAgents) and
//! cmd/rush/remind.go:1-136 (remindCmd).

use std::io::{Read, Write};

use rush_core::adapter::{Profile, Registry};

use crate::views::{View, drop_view, save_view, views};
use crate::viewwords::{VIEW_EXAMPLES, VIEW_GUIDE, VIEW_USAGE};

/// What `rush host` does: `run <id>`, the detached process a rush-mode
/// session lives in, and `preflight <id>`, a restart asking this rush,
/// newly installed, whether it can run the session before the old host
/// stops. It touches nothing.
pub fn host_cmd(
    reg: &Registry,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.len() == 2 && args[0] == "preflight" {
        if let Err(e) = crate::reload::preflight_here(reg, &args[1]) {
            let _ = writeln!(stderr, "rush: {e}");
            return 1;
        }
        let _ = writeln!(stdout, "{}", crate::reload::PREFLIGHT_OK);
        return 0;
    }
    if args.len() < 2 || args[0] != "run" {
        let _ = writeln!(stderr, "rush: usage: rush host run <id>");
        return 1;
    }
    if let Err(e) = crate::server::run(&args[1]) {
        let _ = writeln!(stderr, "rush: {e}");
        return 1;
    }
    0
}

/// `rush attach <id>`: a background session's own screen, in this
/// terminal, by the agent that opens them.
pub fn attach_cmd(reg: &Registry, args: &[String], stderr: &mut dyn Write) -> i32 {
    let Some(id) = args.first() else {
        let _ = writeln!(stderr, "rush: usage: rush attach <id>");
        return 1;
    };
    let Some(j) = reg
        .get(rush_state::LOGINS_KIND)
        .and_then(|a| a.joiner().map(|_| a.clone()))
    else {
        let _ = writeln!(stderr, "rush: no agent here opens a session's own screen");
        return 1;
    };
    let f = rush_state::Store::load().config.active_account();
    let p = Profile {
        kind: rush_core::adapter::Kind::from(rush_state::LOGINS_KIND),
        name: f.name,
        dir: f.config_dir,
    };
    let Some(joiner) = j.joiner() else { return 1 };
    if let Err(e) = joiner.join(&p, id).run() {
        let _ = writeln!(stderr, "rush: {e}");
        return 1;
    }
    0
}

/// `rush inbox <dir>`: a session's hook after each tool call, not meant to
/// be run by hand.
pub fn inbox_cmd(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let Some(d) = args.first() else { return 0 };
    if let Err(e) = crate::inbox::inbox(std::path::Path::new(d), stdin, stdout) {
        let _ = writeln!(stderr, "rush: {e}");
        return 1;
    }
    0
}

/// `rush reload`: every listed rush view reloads, and every session's host
/// on an older rush that's between turns is restarted on this one. One the
/// installed rush can't run stays on the one it has, and is counted
/// failed: exit status 1.
pub fn reload_cmd(stdout: &mut dyn Write) -> i32 {
    let n = rush_state::instances::reload(0);
    crate::events::log("reload-asked", serde_json::json!({"views": n}));
    let _ = writeln!(stdout, "reloaded {n} rush view(s)");
    let mut l = crate::client::Lister::default();
    let (mut stale, restarted, failed, gone) =
        crate::reload::reload_agents(&mut l, crate::wire::BinStamp::exe());
    let _ = write!(stdout, "restarted {restarted} agent(s) on this rush");
    if failed > 0 {
        let _ = write!(
            stdout,
            "; {failed} failed and carry on as they were: their events say why"
        );
        stale -= failed;
    }
    if stale > 0 {
        let _ = write!(
            stdout,
            "; {stale} mid-turn or waiting on you, restarted once their turn ends"
        );
    }
    if gone > 0 {
        let _ = write!(
            stdout,
            "; {gone} whose folder is gone stay as they were: close them"
        );
    }
    let _ = writeln!(stdout);
    if failed > 0 { 1 } else { 0 }
}

/// `rush view` / `rush views`: a panel of the agent's own, drawn by rush
/// beside the conversation. The session is $RUSH_SESSION's, or the one
/// --session names.
pub fn view_cmd(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    view_cmd_in(
        &std::env::var("RUSH_SESSION").unwrap_or_default(),
        args,
        stdin,
        stdout,
        stderr,
    )
}

/// view_cmd for session id unless --session names another.
pub fn view_cmd_in(
    id: &str,
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut id = id.to_string();
    let mut args = args.to_vec();
    if let Some(i) = (0..args.len().saturating_sub(1)).find(|&i| args[i] == "--session") {
        id = args[i + 1].clone();
        args.drain(i..i + 2);
    }
    let mut fail = |e: &str| {
        let _ = writeln!(stderr, "rush view: {e}");
        1
    };
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        let _ = writeln!(stdout, "{VIEW_USAGE}");
        return 0;
    }
    if args[0] == "guide" {
        let _ = writeln!(stdout, "{VIEW_GUIDE}");
        return 0;
    }
    if args[0] == "example" {
        if args.len() == 1 {
            let mut names: Vec<&str> = VIEW_EXAMPLES.iter().map(|(n, _)| *n).collect();
            names.sort();
            let _ = writeln!(stdout, "{}", names.join("\n"));
            return 0;
        }
        let Some((_, ex)) = VIEW_EXAMPLES.iter().find(|(n, _)| *n == args[1]) else {
            return fail(&format!(
                "no example {}: rush view example lists them",
                crate::reload::go_quote(&args[1])
            ));
        };
        let _ = writeln!(stdout, "{ex}");
        return 0;
    }
    if id.is_empty() {
        return fail("not in a rush session: --session <id> names one");
    }
    match (args[0].as_str(), args.len()) {
        ("set", 2) => {
            let mut b = Vec::new();
            if let Err(e) = stdin.take(1 << 20).read_to_end(&mut b) {
                return fail(&e.to_string());
            }
            let mut v: View = match serde_json::from_slice(&b) {
                Ok(v) => v,
                Err(e) => return fail(&format!("its JSON: {e} (rush view help shows the shape)")),
            };
            if args[1].contains("--") {
                return fail("a view's name has no -- in it: those are plugins'");
            }
            v.name = args[1].clone();
            v.by.clear(); // only rush says a plugin drew it
            let name = v.name.clone();
            if let Err(e) = save_view(&id, v) {
                return fail(&e);
            }
            let _ = writeln!(stdout, "shown: {name}");
            0
        }
        ("clear", 2) => match drop_view(&id, &args[1]) {
            Ok(()) => 0,
            Err(e) => fail(&e),
        },
        ("list", 1) => {
            for v in views(&id).0 {
                let _ = writeln!(stdout, "{}\t{}\t{}", v.name, v.title, clock(v.updated));
            }
            0
        }
        _ => {
            let _ = writeln!(stderr, "{VIEW_USAGE}");
            2
        }
    }
}

/// t's time of day here, as Go's Format("15:04:05") of its Local.
fn clock(t: rush_state::Time) -> String {
    let secs = t.unix_nanos().div_euclid(1_000_000_000) as i64;
    // SAFETY: localtime_r fills tm from a valid time_t.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = secs as libc::time_t;
    unsafe { libc::localtime_r(&tt, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// rush remind's help.
pub const REMIND_USAGE: &str = r#"rush remind — have rush send a session a message at a time, waking it if it sleeps

  rush remind <when> <text>   remind this session at when: "25m", "2h", "15:30",
                              "3pm", "tomorrow 9:00", "2026-11-02 09:00"
  rush remind list            this session's reminders (--all: every session's)
  rush remind cancel <id>     drop one; "all" drops every one this session has

Run it in a session rush runs, or name one with --session <id>.
"#;

/// `rush remind` / `rush reminders`: have rush send a session a message at
/// a time. The session is $RUSH_SESSION's, or the one --session names.
pub fn remind_cmd(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    remind_cmd_in(
        &std::env::var("RUSH_SESSION").unwrap_or_default(),
        args,
        stdout,
        stderr,
    )
}

/// remind_cmd for session unless --session names another.
pub fn remind_cmd_in(
    session: &str,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (mut session, mut args) = (session.to_string(), args);
    if args.len() >= 2 && args[0] == "--session" {
        session = args[1].clone();
        args = &args[2..];
    }
    if args.is_empty() || matches!(args[0].as_str(), "help" | "-h" | "--help") {
        let _ = write!(stdout, "{REMIND_USAGE}");
        return 0;
    }
    if args[0] == "run" && args.len() == 2 {
        return remind_run(&args[1], stderr);
    }
    let all = args[0] == "list" && args.len() > 1 && args[1] == "--all";
    if session.is_empty() && !all {
        let _ = writeln!(
            stderr,
            "rush remind: not in a session rush runs; name one with --session <id>"
        );
        return 1;
    }
    let now = rush_state::Time::now();
    match args[0].as_str() {
        "list" | "ls" => {
            remind_list(&session, all, now, stdout);
            return 0;
        }
        "cancel" | "rm" => return remind_cancel(&session, &args[1..], stdout, stderr),
        _ => {}
    }
    if args[0] == "add" {
        args = &args[1..];
    }
    if args.len() < 2 {
        let _ = write!(stderr, "{REMIND_USAGE}");
        return 1;
    }
    let r = rush_remind::parse(&args[0], now)
        .and_then(|due| rush_remind::add(&session, &args[1..].join(" "), due, now));
    match r {
        Ok(r) => {
            let _ = writeln!(stdout, "set {}", rush_remind::line(&r, now));
            0
        }
        Err(e) => {
            let _ = writeln!(stderr, "rush remind: {e}");
            1
        }
    }
}

/// Prints session's reminders; every session's when all.
fn remind_list(session: &str, all: bool, now: rush_state::Time, stdout: &mut dyn Write) {
    rush_remind::resume(); // a waiter lost to a restart is back before it's listed
    let list = rush_remind::list(if all { "" } else { session });
    if list.is_empty() {
        let _ = writeln!(stdout, "no reminders");
    }
    for r in &list {
        if all {
            let _ = writeln!(stdout, "{}  · {}", rush_remind::line(r, now), r.session);
        } else {
            let _ = writeln!(stdout, "{}", rush_remind::line(r, now));
        }
    }
}

/// Drops one of session's reminders, or all of them.
fn remind_cancel(
    session: &str,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.len() != 1 {
        let _ = writeln!(stderr, "usage: rush remind cancel <id>|all");
        return 1;
    }
    let r = if args[0] == "all" {
        rush_remind::cancel_all(session)
    } else {
        rush_remind::cancel(session, &args[0])
    };
    if let Err(e) = r {
        let _ = writeln!(stderr, "rush remind: {e}");
        return 1;
    }
    let _ = writeln!(stdout, "cancelled");
    0
}

/// The detached waiter rush_remind::start starts for one reminder.
fn remind_run(id: &str, stderr: &mut dyn Write) -> i32 {
    let Some(_held) = rush_remind::hold(id) else {
        return 0; // another waiter has it
    };
    let err = rush_remind::Waiter {
        now: &mut rush_state::Time::now,
        sleep: &mut std::thread::sleep,
        gone: &mut |session| crate::client::read_config(session).is_err(),
        deliver: &mut |r, text| {
            let e = rush_core::event::Exchange {
                id: rush_remind::random_hex(16),
                direction: "received".into(),
                phase: "message".into(),
                text: text.into(),
                sender: rush_core::event::Peer {
                    kind: "rush".into(),
                    name: "rush · reminder".into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            crate::client::deliver(&r.session, &e, false).map_err(|e| e.to_string())
        },
    }
    .run(id);
    if let Err(e) = err {
        let _ = writeln!(stderr, "rush remind: {e}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    fn run(id: &str, args: &[&str], stdin: &str) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = view_cmd_in(id, &args, &mut stdin.as_bytes(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn view_command_says_what_go_says() {
        let _h = Home::new();
        std::fs::create_dir_all(crate::paths::dir("s1")).unwrap();
        let (c, out, _) = run("", &[], "");
        assert_eq!(c, 0);
        assert!(out.starts_with(
            "rush view: a panel of your own, drawn by rush beside the conversation\n"
        ));
        let (_, out, _) = run("", &["example"], "");
        assert!(
            out.starts_with(
                "actions\nbars\nboard\ncallout\nchart\nchecklist\ncode\ncompare\ndeploy\n"
            ),
            "{out}"
        );
        let (c, _, err) = run("", &["example", "nope"], "");
        assert_eq!(
            (c, err.as_str()),
            (
                1,
                "rush view: no example \"nope\": rush view example lists them\n"
            )
        );
        let (c, _, err) = run("", &["list"], "");
        assert_eq!(
            (c, err.as_str()),
            (
                1,
                "rush view: not in a rush session: --session <id> names one\n"
            )
        );
        let tests = VIEW_EXAMPLES.iter().find(|(n, _)| *n == "tests").unwrap().1;
        let (c, out, err) = run("", &["--session", "s1", "set", "tests"], tests);
        assert_eq!((c, out.as_str(), err.as_str()), (0, "shown: tests\n", ""));
        let (c, out, _) = run("s1", &["list"], "");
        assert_eq!(c, 0);
        assert!(out.starts_with("tests\tTest run\t"), "{out}");
        let (c, _, err) = run("s1", &["set", "a--b"], "{\"title\":\"x\"}");
        assert_eq!(
            (c, err.as_str()),
            (
                1,
                "rush view: a view's name has no -- in it: those are plugins'\n"
            )
        );
        assert_eq!(run("s1", &["clear", "tests"], "").0, 0);
        assert_eq!(run("s1", &["list"], "").1, "");
        let (c, _, err) = run("s1", &["bogus"], "");
        assert_eq!(c, 2);
        assert!(err.starts_with("rush view: a panel"));
    }

    #[test]
    fn host_command_usage() {
        let reg = Registry::new();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(host_cmd(&reg, &["x".into()], &mut out, &mut err), 1);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "rush: usage: rush host run <id>\n"
        );
    }
}
