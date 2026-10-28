//! `rush plugin …` and `rush <plugin> <command>`, as library functions the
//! binary calls with its stdout and whether stdin is a terminal: the same
//! words, output and exit codes as Go rush.
//! Ported from cmd/rush/plugins.go and cmd/rush/plugincli.go.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::bundled::{bundle_named, bundled_on, bundles, run_bundled, set_bundled};
use crate::cli::{CLI_TIMEOUT, CliRun, cli_plugins, run_cli};
use crate::client::{Status, ask_status, broker_log, reload};
use crate::ctx::Ctx;
use crate::describe::describe;
use crate::manifest::{CATEGORIES, Manifest};
use crate::origin::{origins, set_dev};
use crate::plugin::{Plugin, approvals, approve, digest, installed, load, revoke};
use crate::sandbox::{Launch, supported};
use crate::text::go_q;

/// What `rush plugin` needs of the terminal it runs at.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    /// Whether stdin is a terminal: approving needs you at one.
    pub stdin_terminal: bool,
    /// Reads a line you type, newline and all ("" at the end of input).
    pub read_line: &'a mut dyn FnMut() -> String,
}

const PLUGIN_USAGE: &str = r#"rush plugin — plugins, bundled and sandboxed

  rush plugin list            what's bundled, installed, approved and running
  rush plugin check <name>    check a plugin and show what approving it allows
  rush plugin approve <name>  read what a plugin may do, and let it run
  rush plugin revoke <name>   stop it running
  rush plugin off <name>      turn a bundled plugin off
  rush plugin on <name>       and on again
  rush plugin dev <name> on   development mode: rebuild it from its
                              repository whenever a file there changes
  rush plugin dev <name> off  keep it at the commit it was installed at
  rush plugin logs <name>     where its output goes
  rush plugin dir             where installed plugins live

A repository can ship plugins in .rush/plugins/<name>, each with an
install.sh that builds it into $RUSH_PLUGIN_DIR: rush offers to install
them when you work in it. Each stays at the commit it was installed at;
rush offers an update when a newer commit changes its files.

Plugins you install live in %s/<name>, each with a plugin.json.
Bundled plugins come with rush. Most are on until you turn them off; some,
like the clean-up ones, are off until you turn them on.
"#;

fn usage() -> String {
    PLUGIN_USAGE.replace("%s", &crate::root().display().to_string())
}

macro_rules! out {
    ($io:expr, $($a:tt)*) => {{
        let _ = write!($io.out, $($a)*);
    }};
}

/// `rush plugin <args>`.
pub fn plugin_cmd(args: &[String], io: &mut Io<'_>) -> Result<(), String> {
    let Some(sub) = args.first() else {
        out!(io, "{}", usage());
        return Ok(());
    };
    let n = args.len();
    match sub.as_str() {
        "list" | "ls" => plugin_list(io),
        "run" => {
            // How the broker starts a bundled plugin; not meant to be run by
            // hand.
            if n != 2 {
                return Err("usage: rush plugin run <name>".into());
            }
            // Its HOME is its data folder, <state>/plugin-data/<name>: rush's
            // state is two up, for a broker too old to say so in RUSH_HOME.
            if let Ok(d) = std::env::var("RUSH_PLUGIN_DATA")
                && !d.is_empty()
                && std::env::var("RUSH_HOME").unwrap_or_default().is_empty()
            {
                let parent = crate::gopath::dir(&d);
                if crate::gopath::base(&parent) == "plugin-data" {
                    // SAFETY: set before the plugin starts any thread.
                    unsafe { std::env::set_var("RUSH_HOME", crate::gopath::dir(&parent)) };
                }
            }
            run_bundled(&args[1])
        }
        "on" | "off" => {
            if n != 2 {
                return Err(format!("usage: rush plugin {sub} <name>"));
            }
            set_bundled(&args[1], sub == "on")?;
            out!(io, "{} is {sub}.\n", args[1]);
            reload()
        }
        "check" => {
            if n != 2 {
                return Err("usage: rush plugin check <name>".into());
            }
            let p = load(&crate::root().join(&args[1]))?;
            out!(io, "{}", describe(&p));
            supported()?;
            if let Err(e) = (Launch {
                plugin: p.clone(),
                proxy_port: 0,
            })
            .program()
            {
                return Err(format!("its program: {e}"));
            }
            let why = p.unmet();
            if !why.is_empty() {
                out!(io, "\nIt won't run here: it {why}.\n");
            }
            let d = digest(Path::new(&p.dir)).unwrap_or_default();
            match approvals().get(&p.name) {
                None => out!(
                    io,
                    "\nValid, not approved. To run it: rush plugin approve {}\n",
                    p.name
                ),
                Some(a) if d != a.digest => {
                    out!(
                        io,
                        "\nValid, changed since approval. To run it: rush plugin approve {}\n",
                        p.name
                    )
                }
                Some(_) => out!(io, "\nValid and approved.\n"),
            }
            Ok(())
        }
        "approve" => {
            if n != 2 {
                return Err("usage: rush plugin approve <name>".into());
            }
            plugin_approve(&args[1], io)
        }
        "revoke" => {
            if n != 2 {
                return Err("usage: rush plugin revoke <name>".into());
            }
            revoke(&args[1])?;
            out!(
                io,
                "{} revoked; it stops now and won't start again.\n",
                args[1]
            );
            reload()
        }
        "dir" => {
            out!(io, "{}\n", crate::root().display());
            Ok(())
        }
        "dev" => {
            if n != 3 || (args[2] != "on" && args[2] != "off") {
                return Err("usage: rush plugin dev <name> on|off".into());
            }
            plugin_dev(&args[1], args[2] == "on", io)
        }
        "logs" => {
            if n != 2 {
                return Err("usage: rush plugin logs <name>".into());
            }
            out!(io, "{}\n", crate::log_path(&args[1]).display());
            out!(io, "{}\n", broker_log().display());
            Ok(())
        }
        _ => {
            out!(io, "{}", usage());
            Err(format!("unknown command {}", go_q(sub)))
        }
    }
}

struct Row {
    name: String,
    status: String,
    about: String,
    from: String,
}

fn plugin_list(io: &mut Io<'_>) -> Result<(), String> {
    let (installed, bad) = installed();
    let approvals = approvals();
    let running: BTreeMap<String, Status> = ask_status()
        .map(|st| st.into_iter().map(|s| (s.name.clone(), s)).collect())
        .unwrap_or_default();
    let mut by_category: BTreeMap<&'static str, Vec<Row>> = BTreeMap::new();
    let mut add = |p: &Plugin, status: String| {
        let mut from = String::new();
        if !p.bundled {
            from = p.from();
            if let Ok(d) = digest(Path::new(&p.dir))
                && p.origin.changed_since_install(&d)
            {
                from += ", and changed since";
            }
        }
        by_category
            .entry(p.category_of().id)
            .or_default()
            .push(Row {
                name: p.name.clone(),
                status,
                about: p.description.clone(),
                from,
            });
    };
    for b in bundles() {
        let name = &b.manifest.name;
        let unmet = b.manifest.unmet();
        let mut status = format!("bundled, off: rush plugin on {name}");
        if !unmet.is_empty() {
            status = format!("bundled, {unmet}");
        } else if bundled_on(name) {
            status = "bundled, on".into();
            if let Some(s) = running.get(name) {
                status += &format!(", {}", s.state);
                if !s.error.is_empty() && s.state != "running" {
                    status += &format!(": {}", s.error);
                }
            }
        }
        add(
            &Plugin {
                manifest: b.manifest.clone(),
                bundled: true,
                ..Default::default()
            },
            status,
        );
    }
    for p in &installed {
        let bundled_name = bundle_named(&p.name).is_some();
        let unmet = p.unmet();
        let mut status = "not approved".to_string();
        if !unmet.is_empty() {
            status = unmet;
        } else if bundled_name {
            status = "not run: a plugin bundled with rush has this name".into();
        } else if let Some(a) = approvals.get(&p.name) {
            status = "approved".into();
            match digest(Path::new(&p.dir)) {
                Ok(d) if d == a.digest => {
                    if let Some(s) = running.get(&p.name) {
                        status = s.state.clone();
                        if s.pid > 0 {
                            status += &format!(" (pid {})", s.pid);
                        }
                        if s.restarts > 0 {
                            status += &format!(", {} restarts", s.restarts);
                        }
                        if !s.error.is_empty() && s.state != "running" {
                            status += &format!(": {}", s.error);
                        }
                    }
                }
                _ => status = "changed since approval: approve again to run it".into(),
            }
        }
        add(p, status);
    }
    // Each kind of plugin under its own heading, by what they do.
    for c in &CATEGORIES {
        let Some(rows) = by_category.get(c.id).filter(|r| !r.is_empty()) else {
            continue;
        };
        out!(io, "{} · {}\n", c.title, c.about);
        for l in rows {
            out!(io, "  {:<16} {}\n", l.name, l.status);
            if !l.about.is_empty() {
                out!(io, "  {:<16} {}\n", "", l.about);
            }
            if !l.from.is_empty() {
                out!(io, "  {:<16} {}\n", "", l.from);
            }
        }
        out!(io, "\n");
    }
    if installed.is_empty() && bad.is_empty() {
        out!(
            io,
            "No plugins installed. Put one in {}/<name>.\n",
            crate::root().display()
        );
    }
    if let Err(e) = supported() {
        out!(io, "{e}\n");
    }
    for (n, e) in &bad {
        out!(io, "{n:<16} broken: {e}\n");
    }
    Ok(())
}

fn yes(line: &str) -> bool {
    let a = line.trim().to_lowercase();
    a == "y" || a == "yes"
}

fn plugin_approve(name: &str, io: &mut Io<'_>) -> Result<(), String> {
    supported()?;
    let p = load(&crate::root().join(name))?;
    out!(io, "{}", describe(&p));
    if !io.stdin_terminal {
        return Err("approving needs you at a terminal".into());
    }
    out!(io, "\nLet it run? [y/N] ");
    let _ = io.out.flush();
    if !yes(&(io.read_line)()) {
        out!(io, "Not approved.\n");
        return Ok(());
    }
    approve(&p)?;
    out!(
        io,
        "Approved. If any file in {} changes, it stops until you approve it again.\n",
        p.dir
    );
    out!(
        io,
        "New sessions get its agents, prompt and tools; running ones get them when Claude Code next starts.\n"
    );
    reload()
}

fn plugin_dev(name: &str, on: bool, io: &mut Io<'_>) -> Result<(), String> {
    if on {
        let Some(o) = origins().remove(name).filter(|o| !o.project.is_empty()) else {
            return Err(format!(
                "{name} wasn't installed from a repository: development mode rebuilds it from one"
            ));
        };
        out!(
            io,
            "In development mode, whenever a file changes in {name}'s sources in {},\n",
            o.project
        );
        out!(
            io,
            "rush runs its install.sh again, as you and outside the sandbox, and runs what it\n"
        );
        out!(
            io,
            "built without asking you, as long as it asks for no more than you approved.\n"
        );
        if !io.stdin_terminal {
            return Err("turning development mode on needs you at a terminal".into());
        }
        out!(io, "\nTurn it on? [y/N] ");
        let _ = io.out.flush();
        if !yes(&(io.read_line)()) {
            out!(io, "Left off.\n");
            return Ok(());
        }
    }
    set_dev(name, on)?;
    if on {
        out!(
            io,
            "{name} is in development mode: it's built again now, and whenever its files change.\n"
        );
    } else {
        out!(
            io,
            "{name} stays as it is now; rush offers an update when a newer commit changes it.\n"
        );
    }
    reload()
}

/// Runs `rush <plugin> <command> [args]`, one of the commands a plugin
/// adds, through the broker; it returns the exit code. A terminal on
/// stdout or stderr gets the plugin's output without control characters.
pub fn plugin_cli(
    name: &str,
    m: &Manifest,
    args: &[String],
    stdout: &mut dyn Write,
    stdout_terminal: bool,
    stderr: &mut dyn Write,
    stderr_terminal: bool,
) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        let _ = write!(stdout, "{}", plugin_cli_usage(name, m));
        return 0;
    }
    if m.cli_command(&args[0]).is_none() {
        let _ = write!(
            stderr,
            "rush: {name} has no command {}\n\n{}",
            go_q(&args[0]),
            plugin_cli_usage(name, m)
        );
        return 2;
    }
    let cwd = std::env::current_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let ctx = Ctx::background().with_timeout(CLI_TIMEOUT + std::time::Duration::from_secs(5));
    let run = CliRun {
        command: args[0].clone(),
        args: args[1..].to_vec(),
        cwd,
    };
    let res = match run_cli(&ctx, name, &run) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(stderr, "rush: {e}");
            return 1;
        }
    };
    let for_terminal = |s: &str, term: bool| {
        if term {
            strip_control(s)
        } else {
            s.to_string()
        }
    };
    let _ = write!(stdout, "{}", for_terminal(&res.stdout, stdout_terminal));
    let _ = write!(stderr, "{}", for_terminal(&res.stderr, stderr_terminal));
    res.exit as i32
}

/// A plugin's output as it may reach a terminal: no control characters but
/// newlines and tabs, so it can't set the clipboard, the title or the
/// screen with escape sequences.
pub fn strip_control(s: &str) -> String {
    s.chars()
        .filter(|&r| {
            let u = r as u32;
            r == '\n' || r == '\t' || (u >= 0x20 && u != 0x7f && !(0x80..=0x9f).contains(&u))
        })
        .collect()
}

fn plugin_cli_usage(name: &str, m: &Manifest) -> String {
    let mut b = format!("rush {name}: from the plugin {name}\n\n");
    for c in &m.cli {
        b += &format!("  rush {name} {}", c.name);
        if !c.usage.is_empty() {
            b += &format!(" {}", c.usage);
        }
        b += &format!("\n        {}\n", c.description);
    }
    b
}

/// The help's lines for the commands plugins add.
pub fn plugins_usage() -> String {
    let ps = cli_plugins();
    if ps.is_empty() {
        return String::new();
    }
    let mut b = String::from("\nFrom plugins (\"rush <plugin> help\" for their commands):\n\n");
    for (n, m) in &ps {
        let cs: Vec<&str> = m.cli.iter().map(|c| c.name.as_str()).collect();
        b += &format!("  rush {n:<11} {}\n", cs.join(", "));
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CliSpec;
    use crate::plugin::tests::{Home, install_plugin, manifest};

    /// A plugin's output can't drive the terminal: escape sequences and
    /// other control characters go, newlines and tabs stay.
    #[test]
    fn strip_control_keeps_lines() {
        let input = "ok\t1\n\x1b]52;c;aGk=\x07\x1b[2Jdone\x7f\u{9b}\r";
        assert_eq!(strip_control(input), "ok\t1\n]52;c;aGk=[2Jdone");
    }

    fn run(args: &[&str], terminal: bool, answer: &str) -> (Result<(), String>, String) {
        let mut out = Vec::new();
        let mut answer = Some(answer.to_string());
        let mut read = || answer.take().unwrap_or_default();
        let mut io = Io {
            out: &mut out,
            stdin_terminal: terminal,
            read_line: &mut read,
        };
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let r = plugin_cmd(&args, &mut io);
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn plugin_commands() {
        let _h = Home::new();
        let (r, out) = run(&[], false, "");
        assert!(
            r.is_ok()
                && out.starts_with("rush plugin — plugins")
                && out.contains(&format!("{}/<name>", crate::root().display()))
        );
        let (r, _) = run(&["frob"], false, "");
        assert_eq!(r, Err("unknown command \"frob\"".into()));
        let (r, out) = run(&["dir"], false, "");
        assert_eq!((r, out), (Ok(()), format!("{}\n", crate::root().display())));
        let (r, out) = run(&["list"], false, "");
        assert!(r.is_ok() && out.contains("No plugins installed."), "{out}");

        let mut m = manifest("p", "bin");
        m.description = "Does a thing.".into();
        install_plugin(&m, &[("bin", "#!/bin/sh\n")]);
        let (r, _) = run(&["approve", "p"], false, "");
        if cfg!(target_os = "macos") {
            assert_eq!(r, Err("approving needs you at a terminal".into()));
            let (r, out) = run(&["approve", "p"], true, "n\n");
            assert!(
                r.is_ok() && out.ends_with("Let it run? [y/N] Not approved.\n"),
                "{out}"
            );
            let (_, out) = run(&["check", "p"], false, "");
            assert!(
                out.ends_with("\nValid, not approved. To run it: rush plugin approve p\n"),
                "{out}"
            );
        }
        crate::plugin::approve(&load(&crate::root().join("p")).unwrap()).unwrap();
        let (_, out) = run(&["list"], false, "");
        assert!(out.starts_with("Other · plugins that name no category\n  p                approved\n                   Does a thing.\n"), "{out}");
        let (r, _) = run(&["revoke", "nobody"], false, "");
        assert_eq!(r, Err("nobody is not approved".into()));
        let (r, _) = run(&["dev", "p", "on"], true, "y\n");
        assert_eq!(
            r,
            Err(
                "p wasn't installed from a repository: development mode rebuilds it from one"
                    .into()
            )
        );
    }

    #[test]
    fn plugin_cli_help() {
        let m = Manifest {
            name: "queue".into(),
            cli: vec![CliSpec {
                name: "send".into(),
                usage: "<id>".into(),
                description: "send it now".into(),
            }],
            ..Default::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            plugin_cli("queue", &m, &[], &mut out, false, &mut err, false),
            0
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "rush queue: from the plugin queue\n\n  rush queue send <id>\n        send it now\n"
        );
        let mut out = Vec::new();
        assert_eq!(
            plugin_cli(
                "queue",
                &m,
                &["nope".into()],
                &mut out,
                false,
                &mut err,
                false
            ),
            2
        );
        assert!(
            String::from_utf8(err)
                .unwrap()
                .starts_with("rush: queue has no command \"nope\"\n\n")
        );
    }
}
