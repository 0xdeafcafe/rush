//! A plugin bundled with rush: `rush queue send|remove` sends now, or
//! drops, a message waiting in a session's queue, from a script or a
//! remote client, without the view.
//! Ported from internal/bundled/queue/plugin.go:1-101.

use std::os::unix::net::UnixStream;
use std::sync::Arc;

use rush_plugin::rpc::{from_raw, no_method};
use rush_plugin::{Bundle, CAP_QUEUED, CliResult, CliRun, CliSpec, Conn, Ctx, Manifest};

const USAGE: &str = "<session> <n> [--was TEXT]";

/// The queue plugin's.
pub fn manifest() -> Manifest {
    Manifest {
        sessions: vec![CAP_QUEUED.into()],
        workspaces: vec!["~".into()],
        cli: vec![
            CliSpec {
                name: "send".into(),
                usage: USAGE.into(),
                description: "send the message queued at place n (from 0, as info lists them) now".into(),
            },
            CliSpec {
                name: "remove".into(),
                usage: USAGE.into(),
                description: "drop the message queued at place n; --was names it by its text, if the queue moved".into(),
            },
        ],
        ..crate::manifest(
            "queue",
            "messages",
            "rush queue send and rush queue remove: send now, or drop, a message waiting in a rush-mode session's queue, \
             from a script or a remote client. It never sees what's queued.",
            32,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), false, run)
}

/// The plugin, on its connection to the broker.
fn run(s: UnixStream) -> Result<(), String> {
    crate::serve(
        s,
        Arc::new(|conn, ctx, method, params| match method {
            "initialize" => crate::empty(),
            "tools.list" => crate::no_tools(),
            "cli.run" => {
                let r: CliRun = from_raw(params).map_err(rush_plugin::Error::server)?;
                crate::answer(&cli(ctx, conn, &r))
            }
            _ => Err(no_method(method)),
        }),
    )
}

/// One CLI command.
fn cli(ctx: &Ctx, conn: &Conn, r: &CliRun) -> CliResult {
    let (id, n, was) = match parse(&r.args) {
        Ok(p) => p,
        Err(e) => {
            return CliResult {
                stderr: format!("{e}\nusage: rush queue {} {USAGE}\n", r.command),
                exit: 2,
                ..Default::default()
            };
        }
    };
    let params = serde_json::json!({ "id": id, "index": n, "was": was });
    if let Err(e) =
        conn.call::<_, serde_json::Value>(ctx, &format!("sessions.queued.{}", r.command), &params)
    {
        return CliResult {
            stderr: format!("{e}\n"),
            exit: 1,
            ..Default::default()
        };
    }
    let stdout = match r.command.as_str() {
        "send" => format!("sent queued message {n} to {id}\n"),
        "remove" => format!("removed queued message {n} from {id}\n"),
        // fmt.Sprintf of no format, as Go's would be.
        _ => format!("%!(EXTRA int={n}, string={id})"),
    };
    CliResult {
        stdout,
        ..Default::default()
    }
}

/// Reads <session> <n> [--was TEXT], the flag before or after, as Go's
/// flag package does.
fn parse(args: &[String]) -> Result<(String, i64, String), String> {
    let mut was = String::new();
    let mut pos: Vec<&str> = Vec::new();
    let mut args = args;
    while !args.is_empty() {
        args = parse_flags(args, &mut was)?;
        if let Some((first, rest)) = args.split_first() {
            pos.push(first);
            args = rest;
        }
    }
    if pos.len() != 2 {
        return Err("a session and a place in its queue, please".into());
    }
    let n = match pos[1].parse::<i64>() {
        Ok(n) if n >= 0 => n,
        _ => {
            return Err(format!(
                "queue place {} is not a number from 0",
                rush_plugin::text::go_q(pos[1])
            ));
        }
    };
    Ok((pos[0].trim().to_string(), n, was))
}

/// flag.FlagSet.Parse with one string flag, was: the args left once the
/// flags stop.
fn parse_flags<'a>(mut args: &'a [String], was: &mut String) -> Result<&'a [String], String> {
    while let Some(s) = args.first() {
        if s.len() < 2 || !s.starts_with('-') {
            return Ok(args);
        }
        let mut minuses = 1;
        if s.as_bytes()[1] == b'-' {
            minuses = 2;
            if s.len() == 2 {
                return Ok(&args[1..]); // "--" ends the flags
            }
        }
        let name = &s[minuses..];
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err(format!("bad flag syntax: {s}"));
        }
        args = &args[1..];
        let (name, value) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (name, None),
        };
        if name != "was" {
            if name == "help" || name == "h" {
                return Err("flag: help requested".into());
            }
            return Err(format!("flag provided but not defined: -{name}"));
        }
        *was = match value {
            Some(v) => v,
            None => match args.split_first() {
                Some((v, rest)) => {
                    args = rest;
                    v.clone()
                }
                None => return Err(format!("flag needs an argument: -{name}")),
            },
        };
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Broker;
    use rush_plugin::rpc::{CODE_INVALID_PARAMS, Error};

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    // plugin_test.go TestParse
    #[test]
    fn parses() {
        for args in [
            v(&["a1", "2", "--was", "hi there"]),
            v(&["--was", "hi there", "a1", "2"]),
            v(&["a1", "--was=hi there", "2"]),
        ] {
            assert_eq!(
                parse(&args),
                Ok(("a1".into(), 2, "hi there".into())),
                "{args:?}"
            );
        }
        for args in [
            v(&["a1"]),
            v(&["a1", "x"]),
            v(&["a1", "-1"]),
            v(&["a1", "1", "2"]),
            v(&["a1", "1", "--nope"]),
        ] {
            assert!(parse(&args).is_err(), "{args:?} should be refused");
        }
    }

    // plugin_test.go TestCLIRun: a CLI command goes to the broker as a
    // queued op, and says what it did.
    #[test]
    fn cli_run() {
        let b = Broker::start(
            run,
            Some(Arc::new(|_, p| {
                if p.to_string().contains(r#""gone""#) {
                    return Err(Error::new(CODE_INVALID_PARAMS, "no session gone"));
                }
                Ok(serde_json::json!({}))
            })),
        );
        let cli = |cmd: &str, args: &[&str]| -> CliResult {
            let out = b
                .call(
                    "cli.run",
                    serde_json::to_value(CliRun {
                        command: cmd.into(),
                        args: v(args),
                        ..Default::default()
                    })
                    .unwrap(),
                )
                .unwrap();
            serde_json::from_value(out).unwrap()
        };
        let out = cli("send", &["a1", "0"]);
        assert_eq!(
            (out.exit, out.stdout.as_str()),
            (0, "sent queued message 0 to a1\n")
        );
        let c = b.want("sessions.queued.send");
        assert_eq!(c["index"], 0);
        let out = cli("remove", &["a1", "1", "--was", "hi"]);
        assert!(
            out.exit == 0 && out.stdout.starts_with("removed queued message 1"),
            "{out:?}"
        );
        assert_eq!(b.want("sessions.queued.remove")["was"], "hi");
        let out = cli("send", &["gone", "0"]);
        assert!(
            out.exit == 1 && out.stderr.contains("no session gone"),
            "{out:?}"
        );
        let out = cli("send", &["a1"]);
        assert!(
            out.exit == 2 && out.stderr.contains("usage: rush queue send"),
            "{out:?}"
        );
        b.close().unwrap();
    }
}
