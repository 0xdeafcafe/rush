//! `rush spawn <program> <args…>`: what a session's stand-in for an
//! agent's program runs (rush-host's shims). A run rush can host (codex
//! exec, claude -p, a one-shot another adapter reads) runs as a headless
//! rush session, so you can send to it while it works, printing what the
//! program would have. Anything else, or a flag rush can't print
//! faithfully, runs the real program as it was asked.
//!
//! Port of Go's cmd/rush/spawn.go (spawnCmd :48, spawnHosted :87,
//! riderOf :124, withStdin :143, parseRun :168, flagArgs :198, codexRun
//! :250, claudeRun :315, readStdin :350, realProgram :366, runReal :398,
//! harness :429, hostRun :441 with Headless (Go 26d89b4b), parentMode
//! :548, stopHost :566, inRepo :584, follow :608, claudeOut :649,
//! onceOut :702, codexOut :735, execItem :931) and cmd/rush/exchange.go
//! (outgoingExchange :24, recordExchangeAt :39, mirrorOutgoing :69,
//! exchangePrinter :79, recordReturn :111). parseRun (:168) reads the
//! command as a shell would through rush-convo's spawn_of (convo.SpawnOf).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::{Duration, Instant};

use rush_core::adapter::Registry;
use rush_core::event::{Event, Exchange, PartKind};
use rush_host::client::{Client, await_line, read_config, read_info};
use rush_host::events::log_event;
use rush_host::{Config, Line};
use serde_json::{Value, json};

/// A run rush can host (workRun).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkRun {
    pub kind: String,
    pub prompt: String,
    pub cwd: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    /// codex --json; claude --include-partial-messages.
    pub json: bool,
    pub partial: bool,
    /// claude's --output-format: text, json, stream-json.
    pub format: String,
    /// codex -o.
    pub last_msg: String,
    pub skip_git: bool,
    /// What the program says as it reads stdin.
    pub stdin_note: String,
}

/// The most read_stdin takes in; a prompt that long runs the real
/// program, which reads the rest.
const MAX_STDIN: usize = 1 << 20;

/// Runs rush spawn; its exit status.
pub fn spawn_cmd(reg: &Registry, args: &[String]) -> i32 {
    let Some((prog, rest)) = args.split_first() else {
        eprintln!("usage: rush spawn <program> [args...]");
        return 2;
    };
    let path = std::env::var("PATH").unwrap_or_default();
    // What this runs (a host, the real program's lookups) finds the real
    // programs; the real one run in its place keeps PATH as it was.
    // SAFETY: before any thread of this command starts.
    unsafe { std::env::set_var("PATH", rush_host::shim::without_shims(&path)) };
    let r = parse_run(reg, prog, rest);
    // RUSH_AGENT=ollama claude -p … runs on another provider through the
    // same harness; its own spawns choose again.
    let rider = rider_of(reg, &std::env::var("RUSH_AGENT").unwrap_or_default(), prog);
    if r.is_some() || !rider.is_empty() {
        // SAFETY: as above.
        unsafe { std::env::remove_var("RUSH_AGENT") };
    }
    // Run as itself too, the program finds the sign-in rush keeps for it.
    if rider.is_empty()
        && let Some(a) = reg.get(prog)
        && let Some(p) = reg.profiles_of(&a).into_iter().next()
        && let Err(e) = rush_host::kind::sign_in_if_out(reg, prog, &p)
    {
        log_ev(
            "sign-in-failed",
            json!({"agent": prog, "profile": p.dir, "err": e}),
        );
        eprintln!("rush: {e}");
    }
    let can = r.is_some() && std::env::var("RUSH_NO_SHIM").unwrap_or_default().is_empty();
    if let Some(code) = spawn_hosted(reg, prog, rest, &path, r.clone(), can, &rider) {
        return code;
    }
    log_ev(
        "spawn-not-hosted",
        json!({"program": prog, "args": clip(&rest.join(" ")), "rider": rider, "parsed": r.is_some()}),
    );
    if !rider.is_empty() {
        // The real program would run it on its own provider instead.
        eprintln!(
            "rush: couldn't run this on {} (RUSH_AGENT={rider}): only a run rush can host goes to it",
            agent_name(reg, &rider)
        );
        return 1;
    }
    run_real(reg, prog, rest, &path, None)
}

/// A line in the session's log when this runs in one, else in rush's
/// (logEvent).
fn log_ev(kind: &str, fields: Value) {
    match std::env::var("RUSH_SESSION") {
        Ok(id) if !id.is_empty() => log_event(&id, kind, fields),
        _ => rush_events::log(
            kind,
            match fields {
                Value::Object(m) => m,
                _ => Default::default(),
            },
        ),
    }
}

/// s in at most 300 characters (clipText).
fn clip(s: &str) -> String {
    let r: Vec<char> = s.chars().collect();
    if r.len() > 300 {
        return r[..300].iter().collect::<String>() + "…";
    }
    s.to_string()
}

/// Hosts r when it can, with any prompt it's fed on stdin; None when the
/// real program should run instead (spawnHosted).
fn spawn_hosted(
    reg: &Registry,
    prog: &str,
    rest: &[String],
    path: &str,
    r: Option<WorkRun>,
    can: bool,
    rider: &str,
) -> Option<i32> {
    let mut r = r.filter(|_| can)?;
    if !rider.is_empty() {
        r.kind = rider.to_string();
    }
    let a = reg.get(&r.kind)?;
    if !reg.installed(a.kind().as_str()) {
        return None;
    }
    let (input, fed, waited) = read_stdin();
    if !input.is_empty() {
        match with_stdin(reg, &r, &input) {
            Some(p) => r.prompt = p,
            None if !rider.is_empty() => return None,
            // The program adds what it's fed to the prompt: it does that
            // itself.
            None => return Some(run_real(reg, prog, rest, path, Some(input))),
        }
    }
    if r.prompt.is_empty() {
        return None; // the program says what it wants
    }
    if fed && !r.stdin_note.is_empty() {
        eprintln!("{}", r.stdin_note);
    }
    if waited && harness(reg, &r.kind) == "claude" {
        eprintln!(
            "Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer."
        );
    }
    host_run(reg, &r, &mut std::io::stdout(), &mut std::io::stderr())
}

/// k when it's a provider riding harness_kind's program, else ""
/// (riderOf).
pub fn rider_of(reg: &Registry, k: &str, harness_kind: &str) -> String {
    if k.is_empty() || k == harness_kind {
        return String::new();
    }
    match reg.get(k) {
        Some(a) if a.rider().is_some() && harness(reg, k) == harness_kind => k.to_string(),
        _ => String::new(),
    }
}

fn agent_name(reg: &Registry, k: &str) -> String {
    reg.get(k).map_or_else(|| k.to_string(), |a| a.name())
}

/// The agent whose program k runs: its own, or the one a provider riding
/// another's (Ollama in Claude Code) runs (harness).
pub fn harness(reg: &Registry, k: &str) -> String {
    match reg.get(k).and_then(|a| a.rider().map(|r| r.rides())) {
        Some(h) => h.as_str().to_string(),
        None => k.to_string(),
    }
}

/// r's prompt with what it was fed on stdin, as its program puts them
/// together; None when rush can't say how it would (withStdin).
pub fn with_stdin(reg: &Registry, r: &WorkRun, input: &[u8]) -> Option<String> {
    let mut s = String::from_utf8_lossy(input).into_owned();
    if input.len() >= MAX_STDIN || s.trim().is_empty() && r.prompt.is_empty() {
        return None;
    }
    match harness(reg, &r.kind).as_str() {
        "claude" => Some(if r.prompt.is_empty() {
            s
        } else {
            format!("{}\n{s}", r.prompt)
        }),
        "codex" => {
            if r.prompt.is_empty() {
                return Some(s);
            }
            if !s.ends_with('\n') {
                s.push('\n');
            }
            Some(format!("{}\n\n<stdin>\n{s}</stdin>", r.prompt))
        }
        _ => None,
    }
}

/// The run args ask prog for, when rush can host it and print it as prog
/// would (parseRun).
pub fn parse_run(reg: &Registry, prog: &str, args: &[String]) -> Option<WorkRun> {
    // As a shell command: a word the shell would take apart, quoted.
    let mut words = vec![prog.to_string()];
    for a in args {
        if a.is_empty() || a.chars().any(|c| !SHELL_SAFE.contains(c)) {
            words.push(format!("'{}'", a.replace('\'', r"'\''")));
        } else {
            words.push(a.clone());
        }
    }
    let sp = rush_convo::render::spawn_of(reg, &words.join(" "))?;
    match sp.kind.as_str() {
        "codex" => return codex_run(args),
        // migration: per-agent CLI parsing and output move behind the adapters
        "claude" => return claude_run(args),
        _ => {}
    }
    let a = reg.get(&sp.kind)?;
    let o = a.once_reader()?.read_once(args)?;
    Some(WorkRun {
        kind: sp.kind,
        prompt: o.prompt,
        model: o.model,
        mode: o.mode,
        cwd: o.cwd,
        format: "text".into(),
        ..WorkRun::default()
    })
}

/// The characters a word needs no quotes for (shellSafe).
const SHELL_SAFE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./=:@,+%";

/// Walks args: each flag with its value (for those that take one, as
/// --x=v or --x v), and each bare word. Any flag it isn't told about
/// makes it false (flagArgs).
fn flag_args(
    args: &[String],
    valued: &[&str],
    bare: &[&str],
    mut each: impl FnMut(&str, &str) -> bool,
    mut word: impl FnMut(&str) -> bool,
) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            // The rest are words, a prompt starting with - too.
            return args[i + 1..].iter().all(|w| word(w));
        }
        if !a.starts_with('-') || a == "-" {
            if !word(a) {
                return false;
            }
            i += 1;
            continue;
        }
        let (name, mut val, eq) = match a.split_once('=') {
            Some((n, v)) => (n, v.to_string(), true),
            None => (a.as_str(), String::new(), false),
        };
        if valued.contains(&name) && !eq {
            if i + 1 >= args.len() {
                return false;
            }
            i += 1;
            val = args[i].clone();
        } else if !(bare.contains(&name) && !eq || valued.contains(&name)) {
            return false;
        }
        if !each(name, &val) {
            return false;
        }
        i += 1;
    }
    true
}

const CODEX_VALUED: &[&str] = &[
    "-m",
    "--model",
    "-s",
    "--sandbox",
    "-C",
    "--cd",
    "-o",
    "--output-last-message",
    "--color",
    "-c",
    "--config",
];
const CODEX_BARE: &[&str] = &[
    "--full-auto",
    "--dangerously-bypass-approvals-and-sandbox",
    "--yolo",
    "--skip-git-repo-check",
    "--json",
    "--experimental-json",
];

/// codex's sandboxes as the modes its adapter takes (codexModes).
fn codex_mode(sandbox: &str) -> &'static str {
    match sandbox {
        "read-only" => "read-only",
        "workspace-write" => "auto",
        "danger-full-access" => "full-access",
        _ => "",
    }
}

/// Reads codex [flags] exec [flags] PROMPT (codexRun).
pub fn codex_run(args: &[String]) -> Option<WorkRun> {
    let mut r = WorkRun {
        kind: "codex".into(),
        format: "text".into(),
        ..WorkRun::default()
    };
    let mut sub = false;
    let mut words: Vec<String> = Vec::new();
    let ok = flag_args(
        args,
        CODEX_VALUED,
        CODEX_BARE,
        |name, val| {
            match name {
                "-m" | "--model" => r.model = val.into(),
                "-s" | "--sandbox" => {
                    r.mode = codex_mode(val).into();
                    return !r.mode.is_empty();
                }
                "--full-auto" => r.mode = "auto".into(),
                "--dangerously-bypass-approvals-and-sandbox" | "--yolo" => {
                    r.mode = "full-access".into()
                }
                "-C" | "--cd" => r.cwd = val.into(),
                "--skip-git-repo-check" => r.skip_git = true,
                "--json" | "--experimental-json" => r.json = true,
                "-o" | "--output-last-message" => r.last_msg = val.into(),
                "-c" | "--config" => {
                    // Only the effort maps onto a session's own settings.
                    let (k, v) = val.split_once('=').unwrap_or((val, ""));
                    if k.trim() != "model_reasoning_effort" {
                        return false;
                    }
                    r.effort = v.trim().trim_matches(['"', '\'']).into();
                }
                _ => {}
            }
            true
        },
        |w| {
            if !sub {
                sub = w == "exec" || w == "e";
                return sub;
            }
            words.push(w.into());
            true
        },
    );
    // exec resume and exec review are runs of another kind.
    if !ok
        || !sub
        || words.len() > 1
        || words.len() == 1 && matches!(words[0].as_str(), "resume" | "review" | "help")
    {
        return None;
    }
    // With no prompt, or -, it's all on stdin.
    match words.first() {
        None => r.stdin_note = "Reading prompt from stdin...".into(),
        Some(w) if w != "-" => {
            r.prompt = w.clone();
            r.stdin_note = "Reading additional input from stdin...".into();
        }
        _ => {}
    }
    Some(r)
}

const CLAUDE_VALUED: &[&str] = &[
    "--model",
    "--output-format",
    "--permission-mode",
    "--effort",
];
const CLAUDE_BARE: &[&str] = &[
    "-p",
    "--print",
    "--verbose",
    "--include-partial-messages",
    "--dangerously-skip-permissions",
];

/// Reads claude -p [flags] PROMPT (claudeRun).
pub fn claude_run(args: &[String]) -> Option<WorkRun> {
    let mut r = WorkRun {
        kind: "claude".into(),
        format: "text".into(),
        ..WorkRun::default()
    };
    let (mut print, mut verbose) = (false, false);
    let mut words: Vec<String> = Vec::new();
    let ok = flag_args(
        args,
        CLAUDE_VALUED,
        CLAUDE_BARE,
        |name, val| {
            match name {
                "-p" | "--print" => print = true,
                "--model" => r.model = val.into(),
                "--output-format" => {
                    r.format = val.into();
                    return matches!(val, "text" | "json" | "stream-json");
                }
                "--verbose" => verbose = true,
                "--include-partial-messages" => r.partial = true,
                "--permission-mode" => r.mode = val.into(),
                "--dangerously-skip-permissions" => r.mode = "bypassPermissions".into(),
                "--effort" => r.effort = val.into(),
                _ => {}
            }
            true
        },
        |w| {
            words.push(w.into());
            true
        },
    );
    // stream-json wants --verbose (claude says so itself), and json with
    // it prints every message rather than the result.
    if !ok
        || !print
        || words.len() > 1
        || r.format == "stream-json" && !verbose
        || r.format == "json" && verbose
        || r.partial && r.format != "stream-json"
    {
        return None;
    }
    if let Some(w) = words.pop() {
        r.prompt = w; // else it's on stdin
    }
    Some(r)
}

/// What stdin holds when it isn't a terminal, as the programs read it;
/// fed is whether it isn't. Claude Code gives up on it after a few
/// seconds, and so does this: waited is whether it did (readStdin).
fn read_stdin() -> (Vec<u8>, bool, bool) {
    // SAFETY: isatty only asks.
    if unsafe { libc::isatty(0) } == 1 {
        return (Vec::new(), false, false);
    }
    let (tx, rx) = sync_channel(1);
    let _ = std::thread::Builder::new()
        .name("spawn-stdin".into())
        .spawn(move || {
            let mut b = Vec::new();
            let _ = std::io::stdin()
                .lock()
                .take(MAX_STDIN as u64)
                .read_to_end(&mut b);
            let _ = tx.send(b);
        }); // the rest is read by whoever runs
    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(b) => (b, true, false),
        Err(_) => (Vec::new(), true, true),
    }
}

/// Where prog is on path, past the stand-ins (realProgram).
fn real_program(reg: &Registry, prog: &str, path: &str) -> Option<PathBuf> {
    let shims = rush_host::shim::shim_dir();
    let shims = std::fs::canonicalize(&shims).unwrap_or(shims);
    let in_shims = |p: &Path| {
        std::fs::canonicalize(p)
            .ok()
            .is_some_and(|r| r.parent() == Some(shims.as_path()))
    };
    for d in std::env::split_paths(&rush_host::shim::without_shims(path)) {
        let p = d.join(prog);
        let Ok(md) = std::fs::metadata(&p) else {
            continue;
        };
        use std::os::unix::fs::PermissionsExt;
        if md.is_dir() || md.permissions().mode() & 0o111 == 0 || in_shims(&p) {
            continue;
        }
        return Some(p);
    }
    let k = reg.program_kind(prog)?;
    let p = reg.path(k.as_str());
    (!p.is_empty() && !in_shims(Path::new(&p))).then(|| PathBuf::from(p))
}

/// Runs the real prog in this process's place, with PATH as it was; fed,
/// when set, is the start of its stdin, already read (runReal).
fn run_real(reg: &Registry, prog: &str, args: &[String], path: &str, fed: Option<Vec<u8>>) -> i32 {
    use std::os::unix::process::CommandExt;
    let Some(bin) = real_program(reg, prog, path) else {
        eprintln!("rush: can't find {prog} on PATH");
        return 127;
    };
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg0(prog).args(args).env("PATH", path);
    let Some(fed) = fed else {
        rush_events::flush();
        let err = cmd.exec(); // only returns if it failed
        eprintln!("rush: {err}");
        return 126;
    };
    cmd.stdin(std::process::Stdio::piped());
    // SAFETY: SIG_IGN only; they reach it too, as its group's.
    unsafe {
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(s, libc::SIG_IGN);
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rush: {e}");
            return 126;
        }
    };
    if let Some(mut w) = child.stdin.take() {
        let _ = std::thread::Builder::new().spawn(move || {
            let _ = w.write_all(&fed);
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut w);
        });
    }
    match child.wait() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("rush: {e}");
            126
        }
    }
}

/// Where each agent's program is told its config folder (configDirEnv).
fn config_dir_env(k: &str) -> &'static str {
    match k {
        "claude" => "CLAUDE_CONFIG_DIR",
        "codex" => "CODEX_HOME",
        _ => "",
    }
}

/// Whether dir is in a git repository, which codex exec wants (inRepo).
fn in_repo(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// The permission mode of the rush session whose shell this runs in, when
/// it's the same agent's: a run it starts works for it, with its trust,
/// not stuck on prompts no one sees. "" otherwise (parentMode).
fn parent_mode(k: &str) -> String {
    let id = std::env::var("RUSH_SESSION").unwrap_or_default();
    if id.is_empty() {
        return String::new();
    }
    let Ok(cfg) = read_config(&id) else {
        return String::new();
    };
    if rush_core::adapter::migrated(&cfg.kind).as_str() != k {
        return String::new();
    }
    read_info(&id)
        .map(|i| i.permission_mode)
        .unwrap_or_default()
}

/// What follow returns when the session's host went away.
const HOST_GONE: i32 = -1;

/// Runs r as a headless rush session and prints it as its program would;
/// None when it couldn't start one, and nothing was printed (hostRun).
fn host_run(
    reg: &Registry,
    r: &WorkRun,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Option<i32> {
    let here = std::env::current_dir().ok()?;
    let cwd = if r.cwd.is_empty() {
        here
    } else {
        here.join(&r.cwd)
    };
    let cwd_s = cwd.to_string_lossy().into_owned();
    let base = harness(reg, &r.kind);
    let mut r = r.clone();
    if base == "codex" && !r.skip_git && !in_repo(&cwd) {
        if r.kind != base {
            // The real codex would run it on OpenAI.
            let _ = writeln!(
                stderr,
                "Not inside a trusted directory and --skip-git-repo-check was not specified."
            );
            return Some(1);
        }
        return None; // codex says why it won't run there
    }
    if r.mode.is_empty() && r.kind == base {
        r.mode = parent_mode(&r.kind);
    }
    let parent = std::env::var("RUSH_SESSION").unwrap_or_default();
    let mut cfg = Config {
        cwd: cwd_s.clone(),
        model: r.model.clone(),
        effort: r.effort.clone(),
        permission_mode: r.mode.clone(),
        prompt: r.prompt.clone(),
        name: first_words_of(&r.prompt),
        owner: std::process::id() as i64,
        // No one watches it in rush: it isn't told of rush's task list,
        // decisions cards or views, nor given view and show.
        headless: true,
        ..Config::default()
    };
    cfg.meta.insert(
        "spawnedBy".into(),
        if parent.is_empty() {
            "shell".into()
        } else {
            parent.clone()
        },
    );
    if r.kind == rush_state::LOGINS_KIND {
        let f = rush_state::Store::load().config.active_account();
        cfg.account = rush_core::adapter::Profile {
            kind: rush_state::LOGINS_KIND.into(),
            name: f.name,
            dir: f.config_dir,
        };
    }
    cfg.use_agent(reg, &r.kind).ok()?;
    // In the config folder the program would have used.
    let env = config_dir_env(&r.kind);
    if let Ok(d) = std::env::var(env)
        && !env.is_empty()
        && r.kind == base
        && !d.is_empty()
        && Path::new(&d) != Path::new(&cfg.account.dir)
    {
        cfg.account.dir = d.clone();
        if let Some(a) = reg.get(&r.kind) {
            for p in a.profiles() {
                if Path::new(&p.dir) == Path::new(&d) {
                    cfg.account = p;
                }
            }
        }
    }
    let _ = rush_host::kind::sign_in_if_out(reg, &r.kind, &cfg.account); // said once already
    let mut request = outgoing_exchange("", "request", &r.prompt, &[]);
    cfg.prompt_exchange = request.clone().map(Box::new);
    cfg.cause = rush_events::id(); // the spawned line, its host's cause
    let started = match rush_host::client::spawn(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            log_ev(
                "spawn-failed",
                json!({"agent": r.kind, "cwd": cwd_s, "err": e.to_string()}),
            );
            return None;
        }
    };
    log_event(
        &started.id,
        "spawned",
        json!({"ev": cfg.cause, "by": "rush spawn", "spawnedBy": cfg.meta["spawnedBy"],
            "agent": r.kind, "cwd": cwd_s, "mode": r.mode, "model": r.model, "prompt": clip(&r.prompt)}),
    );
    if let Some(req) = request.as_mut() {
        req.receiver = rush_host::subtool::peer(&started.id);
        if let Err(e) = mirror_outgoing(req) {
            let _ = writeln!(stderr, "rush: could not record delegated task: {e}");
        }
    }
    // Claude Code's own lines, as claude -p prints them.
    let dial = if base == "claude" {
        Client::dial_raw
    } else {
        Client::dial
    };
    let mut c = match dial(&started.id) {
        Ok(c) => c,
        Err(_) => {
            if let Ok(info) = read_info(&started.id)
                && info.host_pid > 0
            {
                // SAFETY: a signal to the host this started.
                unsafe { libc::kill(info.host_pid as i32, libc::SIGTERM) };
            }
            return None;
        }
    };
    let mut out: Box<dyn Printer> = match base.as_str() {
        "codex" => Box::new(CodexOut::new(r.clone(), cwd_s.clone())),
        "claude" => Box::new(ClaudeOut::new(r.clone())),
        _ => Box::new(OnceOut::default()),
    };
    if request.is_some() {
        out = Box::new(ExchangePrinter {
            inner: out,
            decoder: rush_host::Decoder::default(),
            text: String::new(),
        });
    }
    // Stopped, it stops the session too, as the program would have ended.
    let msgs = merged(&mut c);
    let mut code = follow(&c, &msgs, &mut *out, stdout, stderr);
    if code == HOST_GONE {
        // Why it went, as the agent that couldn't start said.
        let why = read_info(&started.id).map(|i| i.error).unwrap_or_default();
        let why = if why.is_empty() {
            "the session's host went away".to_string()
        } else {
            why
        };
        let _ = writeln!(stderr, "rush: {why}");
        code = 1;
    }
    if let Some(req) = &request {
        record_return(req, &started.id, &out.answer(), code);
    }
    stop_host(&c, &started.id);
    Some(code)
}

/// The first few words of a prompt, as a session's name (firstWordsOf).
fn first_words_of(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(6).collect();
    let n = words.join(" ");
    let r: Vec<char> = n.chars().collect();
    if r.len() > 48 {
        return r[..47].iter().collect::<String>() + "…";
    }
    n
}

/// Ends the session's host, as the program would have ended: by asking,
/// and if that doesn't take, by signal (stopHost).
fn stop_host(c: &Client, id: &str) {
    let _ = c.stop();
    let Ok(info) = read_info(id) else {
        return;
    };
    if info.host_pid <= 0 {
        return;
    }
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if !rush_host::alive(info.host_pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    log_event(
        id,
        "host-stop-forced",
        json!({"pid": info.host_pid, "why": "it didn't stop within 5s of being asked"}),
    );
    // SAFETY: a signal to the host this started.
    unsafe { libc::kill(info.host_pid as i32, libc::SIGTERM) };
}

// ---- Signals ----

/// What follow waits on: a line from the host, its end, or a signal.
enum Msg {
    Line(Vec<u8>),
    Gone,
    Signal(i32),
}

static SIG_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(s: libc::c_int) {
    let fd = SIG_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let b = s as u8;
        // SAFETY: write is async-signal-safe; b is on this stack.
        unsafe { libc::write(fd, (&b as *const u8).cast(), 1) };
    }
}

/// The host's lines and this process's SIGINT, SIGTERM and SIGHUP, as
/// one stream: two threads, each blocked until there's something.
fn merged(c: &mut Client) -> Receiver<Msg> {
    let (tx, rx) = sync_channel::<Msg>(256);
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe writes two descriptors into fds.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == 0 {
        SIG_FD.store(fds[1], Ordering::SeqCst);
        // SAFETY: on_signal only loads an atomic and writes.
        unsafe {
            let h = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                libc::signal(s, h);
            }
        }
        let tx = tx.clone();
        let fd = fds[0];
        let _ = std::thread::Builder::new()
            .name("spawn-signals".into())
            .spawn(move || {
                let mut b = 0u8;
                // SAFETY: one byte into b.
                if unsafe { libc::read(fd, (&mut b as *mut u8).cast(), 1) } == 1 {
                    let _ = tx.send(Msg::Signal(b as i32));
                }
            });
    }
    let lines = std::mem::replace(&mut c.lines, sync_channel(0).1);
    let _ = std::thread::Builder::new()
        .name("spawn-lines".into())
        .spawn(move || {
            for l in lines {
                if tx.send(Msg::Line(l)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Gone);
        });
    rx
}

// ---- Following and printing ----

/// Prints a hosted run as its program would.
pub trait Printer {
    fn line(&mut self, raw: &[u8], l: Option<&Line>, out: &mut dyn Write, err: &mut dyn Write);
    fn finish(&mut self, out: &mut dyn Write, err: &mut dyn Write) -> i32;
    /// The run's answer, for the exchange recorded with whoever started
    /// it.
    fn answer(&self) -> String {
        String::new()
    }
}

/// Reads the session until it has answered and has nothing more to do:
/// its turn over, and nothing you sent it while it ran left. Anything it
/// asks is refused, as the program run this way would (follow).
fn follow(
    c: &Client,
    msgs: &Receiver<Msg>,
    out: &mut dyn Printer,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut ended = false;
    loop {
        let raw = match msgs.recv() {
            Ok(Msg::Line(l)) => l,
            Ok(Msg::Signal(s)) => return if s == libc::SIGINT { 130 } else { 143 },
            Ok(Msg::Gone) | Err(_) => return HOST_GONE,
        };
        let l = rush_host::decode(&raw).ok();
        out.line(&raw, l.as_ref(), stdout, stderr);
        match &l {
            Some(Line::Event(Event::TurnEnd(_))) => ended = true,
            Some(Line::Event(Event::Approval(a))) => {
                let _ = c.deny(&a.id, "not allowed in a run nobody is watching", false);
            }
            Some(Line::Event(Event::Question(q))) => {
                let _ = c.deny(&q.id, "no one is there to answer", false);
            }
            Some(Line::Native(n)) => {
                use rush_claude::headless::event::Event as H;
                match n.downcast_ref::<H>() {
                    Some(H::Result(_)) => ended = true,
                    Some(H::PermissionRequest(p)) => {
                        let why = format!(
                            "Claude requested permissions to use {}, but you haven't granted it yet.",
                            p.tool
                        );
                        let _ = c.deny(&p.id, &why, false);
                    }
                    _ => {}
                }
            }
            Some(Line::Info(i)) => {
                if i.state == "working" {
                    ended = false;
                }
                // Work it left in the background would be cut off: it
                // picks up again when that ends.
                let busy = i.retry.as_ref().is_some_and(|r| !r.gave_up) || !i.background.is_empty();
                if ended && (i.state == "idle" && i.queue.is_empty() && !busy || i.limit.is_some())
                    || i.state == "stopped"
                {
                    return out.finish(stdout, stderr);
                }
            }
            _ => {}
        }
    }
}

/// Prints a hosted Claude Code run as claude -p does: the result's text,
/// the result, or every line (claudeOut).
pub struct ClaudeOut {
    r: WorkRun,
    result: Option<Vec<u8>>,
    failed: bool,
}

impl ClaudeOut {
    pub fn new(r: WorkRun) -> ClaudeOut {
        ClaudeOut {
            r,
            result: None,
            failed: false,
        }
    }
}

/// The lines of Claude Code's a hosted session has and claude -p doesn't
/// print (claudeOnly).
const CLAUDE_ONLY: &[&str] = &[
    "control_request",
    "control_response",
    "control_cancel_request",
    "keep_alive",
];

impl Printer for ClaudeOut {
    fn line(&mut self, raw: &[u8], _: Option<&Line>, out: &mut dyn Write, _: &mut dyn Write) {
        let Ok(v) = serde_json::from_slice::<Value>(raw) else {
            return;
        };
        let typ = v["type"].as_str().unwrap_or("");
        let subtype = v["subtype"].as_str().unwrap_or("");
        let agtop_sent = v["agtop_sent"].as_bool().unwrap_or(false);
        let is_error = v["is_error"].as_bool().unwrap_or(false);
        if agtop_sent || typ.starts_with("agtop_") || CLAUDE_ONLY.contains(&typ) {
            return;
        }
        // Streamed deltas, and the status that comes with them, are only
        // printed when asked for.
        if (typ == "stream_event" || typ == "system" && subtype == "status") && !self.r.partial {
            return;
        }
        if typ == "result" {
            self.result = Some(raw.to_vec());
            self.failed = is_error;
        }
        if self.r.format == "stream-json" {
            let _ = out.write_all(raw);
            let _ = out.write_all(b"\n");
        }
    }

    fn finish(&mut self, out: &mut dyn Write, _: &mut dyn Write) -> i32 {
        let Some(res) = &self.result else {
            return 1;
        };
        match self.r.format.as_str() {
            "json" => {
                let _ = out.write_all(res);
                let _ = out.write_all(b"\n");
            }
            "text" => {
                let v: Value = serde_json::from_slice(res).unwrap_or_default();
                let _ = writeln!(out, "{}", v["result"].as_str().unwrap_or(""));
            }
            _ => {}
        }
        i32::from(self.failed)
    }
}

/// Prints a hosted one-shot run as the programs rush reads with
/// OnceReader do: the last answer on stdout, or why it failed on stderr
/// (onceOut).
#[derive(Default)]
pub struct OnceOut {
    last: String,
    err: String,
}

impl Printer for OnceOut {
    fn line(&mut self, _: &[u8], l: Option<&Line>, _: &mut dyn Write, _: &mut dyn Write) {
        match l {
            Some(Line::Event(Event::Message(m))) => {
                let text: String = m
                    .parts
                    .iter()
                    .filter(|p| p.kind == PartKind::Text)
                    .map(|p| p.text.as_str())
                    .collect();
                if m.role == "assistant" && !text.is_empty() {
                    self.last = text;
                }
            }
            Some(Line::Event(Event::TurnEnd(e))) => {
                self.err.clear();
                if e.reason == "error" {
                    self.err = or(&e.err, "turn failed");
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        if !self.err.is_empty() {
            let _ = writeln!(err, "Error: {}", self.err);
            return 1;
        }
        if !self.last.is_empty() {
            let _ = writeln!(out, "{}", self.last);
        }
        0
    }
}

fn or(a: &str, b: &str) -> String {
    if a.is_empty() { b } else { a }.to_string()
}

/// Prints a hosted Codex run as codex exec does: progress on stderr and
/// the last message on stdout, or its events as JSON lines (codexOut).
pub struct CodexOut {
    r: WorkRun,
    cwd: String,
    /// Codex's item ids as exec numbers them.
    ids: HashMap<String, String>,
    /// The turn's last message, and why the last turn failed.
    last: String,
    err: String,
    turn: bool,
    /// The todo list's item this turn, and what it last said.
    plan: String,
    todo: Option<rush_core::event::Plan>,
    tokens: i64,
}

impl CodexOut {
    pub fn new(r: WorkRun, cwd: String) -> CodexOut {
        CodexOut {
            r,
            cwd,
            ids: HashMap::new(),
            last: String::new(),
            err: String::new(),
            turn: false,
            plan: String::new(),
            todo: None,
            tokens: 0,
        }
    }

    fn id(&mut self, of: &str) -> String {
        if let Some(id) = self.ids.get(of) {
            return id.clone();
        }
        let id = format!("item_{}", self.ids.len());
        self.ids.insert(of.to_string(), id.clone());
        id
    }

    fn emit(out: &mut dyn Write, v: Value) {
        if let Ok(b) = serde_json::to_vec(&v) {
            let _ = out.write_all(&b);
            let _ = out.write_all(b"\n");
        }
    }

    fn part(
        &mut self,
        m: &rush_core::event::Message,
        p: &rush_core::event::Part,
        out: &mut dyn Write,
        err: &mut dyn Write,
    ) {
        let json = self.r.json;
        if m.role == "assistant" && p.kind == PartKind::Text {
            self.last = p.text.clone();
            if json {
                let id = self.id(&m.id);
                Self::emit(
                    out,
                    json!({"type": "item.completed", "item": {"id": id, "type": "agent_message", "text": p.text}}),
                );
                return;
            }
            let _ = writeln!(err, "codex\n{}", p.text);
        } else if m.role == "assistant" && p.kind == PartKind::Thinking {
            if json {
                let id = self.id(&m.id);
                Self::emit(
                    out,
                    json!({"type": "item.completed", "item": {"id": id, "type": "reasoning", "text": p.text}}),
                );
                return;
            }
            let _ = writeln!(err, "thinking\n{}", p.text);
        } else if p.kind == PartKind::ToolCall
            && let Some(c) = &p.call
        {
            if json {
                let id = self.id(&c.id);
                if let Some(it) = exec_item(&id, c.raw.as_ref().map(|r| r.get())) {
                    Self::emit(out, json!({"type": "item.started", "item": it}));
                }
                return;
            }
            if !c.input.command.is_empty() {
                let _ = writeln!(
                    err,
                    "exec\n{} in {}",
                    c.input.command,
                    or(&c.input.cwd, &self.cwd)
                );
            }
        } else if p.kind == PartKind::ToolResult
            && let Some(o) = &p.output
        {
            if json {
                let id = self.id(&o.call_id);
                if let Some(it) = exec_item(&id, o.raw.as_ref().map(|r| r.get())) {
                    Self::emit(out, json!({"type": "item.completed", "item": it}));
                }
                return;
            }
            match o.exit {
                Some(0) => {
                    let _ = writeln!(err, " succeeded:\n{}", o.text);
                }
                Some(n) => {
                    let _ = writeln!(err, " exited {n}:\n{}", o.text);
                }
                None => {}
            }
        }
    }

    /// The turn's plan as exec's todo_list item: started, then updated,
    /// and completed as the turn ends (how, then).
    fn todos(&mut self, how: &str, out: &mut dyn Write) {
        if !self.r.json {
            return;
        }
        let Some(todo) = self.todo.clone() else {
            return;
        };
        let mut kind = "item.updated";
        if self.plan.is_empty() {
            let n = self.ids.len();
            self.plan = self.id(&format!("plan:{n}"));
            kind = "item.started";
        }
        if !how.is_empty() {
            kind = how;
        }
        let items: Vec<Value> = todo
            .todos
            .iter()
            .map(|t| json!({"text": t.label, "completed": t.status == "completed"}))
            .collect();
        Self::emit(
            out,
            json!({"type": kind, "item": {"id": self.plan, "type": "todo_list", "items": items}}),
        );
    }

    fn turn_end(
        &mut self,
        e: &rush_core::event::TurnEnd,
        out: &mut dyn Write,
        err: &mut dyn Write,
    ) {
        self.todos("item.completed", out);
        self.turn = false;
        self.todo = None;
        self.tokens += e.tokens.input + e.tokens.output;
        if e.reason == "error" {
            self.err = or(&e.err, "turn failed");
        }
        if !self.r.json {
            if !self.err.is_empty() {
                let _ = writeln!(err, "ERROR: {}", self.err);
            }
            return;
        }
        if !self.err.is_empty() {
            Self::emit(
                out,
                json!({"type": "turn.failed", "error": {"message": self.err}}),
            );
            return;
        }
        let t = &e.tokens;
        Self::emit(
            out,
            json!({"type": "turn.completed", "usage": {
                "input_tokens": t.input + t.cache_read,
                "cached_input_tokens": t.cache_read,
                "cache_write_input_tokens": t.cache_write_5m + t.cache_write_1h,
                "output_tokens": t.output,
                "reasoning_output_tokens": t.reasoning,
            }}),
        );
    }
}

impl Printer for CodexOut {
    fn line(&mut self, _: &[u8], l: Option<&Line>, out: &mut dyn Write, err: &mut dyn Write) {
        match l {
            Some(Line::Event(Event::Init(e))) => {
                if self.r.json {
                    Self::emit(
                        out,
                        json!({"type": "thread.started", "thread_id": e.session_id}),
                    );
                    return;
                }
                let _ = write!(
                    err,
                    "OpenAI Codex v{}\n--------\nworkdir: {}\nmodel: {}\n",
                    e.version.trim_start_matches('v'),
                    or(&e.cwd, &self.cwd),
                    e.model
                );
                if !self.r.effort.is_empty() {
                    let _ = writeln!(err, "reasoning effort: {}", self.r.effort);
                }
                let _ = write!(
                    err,
                    "session id: {}\n--------\nuser\n{}\n",
                    e.session_id, self.r.prompt
                );
            }
            Some(Line::Sent(s)) => {
                if !self.r.json && s.text != self.r.prompt {
                    let _ = writeln!(err, "user\n{}", s.text);
                }
            }
            Some(Line::Event(Event::Status(s))) => {
                if s.busy && s.text.is_empty() && !self.turn {
                    self.turn = true;
                    self.last.clear();
                    self.err.clear();
                    self.plan.clear();
                    self.todo = None;
                    if self.r.json {
                        Self::emit(out, json!({"type": "turn.started"}));
                    }
                }
            }
            Some(Line::Event(Event::Message(m))) => {
                for p in &m.parts {
                    self.part(m, p, out, err);
                }
            }
            Some(Line::Event(Event::Plan(p))) => {
                self.todo = Some(p.clone());
                self.todos("", out);
            }
            Some(Line::Event(Event::TurnEnd(e))) => self.turn_end(e, out, err),
            _ => {}
        }
    }

    fn finish(&mut self, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        if !self.r.json {
            let _ = writeln!(err, "tokens used\n{}", self.tokens);
            if !self.last.is_empty() {
                let _ = writeln!(out, "{}", self.last);
            }
        }
        if !self.err.is_empty() {
            return 1; // and -o is left as it was, as exec leaves it
        }
        if !self.r.last_msg.is_empty() {
            let _ = std::fs::write(&self.r.last_msg, &self.last);
        }
        0
    }
}

/// One of Codex's app-server items as exec prints it, by the id exec
/// gives it; None for the kinds exec doesn't print (execItem).
pub fn exec_item(id: &str, raw: Option<&str>) -> Option<Value> {
    let it: Value = serde_json::from_str(raw?).ok()?;
    let s = |k: &str| it[k].as_str().unwrap_or("").to_string();
    let status = s("status")
        .replace("inProgress", "in_progress")
        .to_lowercase();
    let or_null = |v: &Value| if v.is_null() { Value::Null } else { v.clone() };
    match it["type"].as_str()? {
        "commandExecution" => Some(json!({
            "id": id, "type": "command_execution", "command": s("command"),
            "aggregated_output": s("aggregatedOutput"), "exit_code": or_null(&it["exitCode"]),
            "status": status,
        })),
        "fileChange" => {
            let changes: Vec<Value> = it["changes"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| json!({"path": c["path"].as_str().unwrap_or(""), "kind": c["kind"]["type"].as_str().unwrap_or("")}))
                        .collect()
                })
                .unwrap_or_default();
            Some(json!({"id": id, "type": "file_change", "changes": changes, "status": status}))
        }
        "mcpToolCall" => {
            let res = if it["result"].is_object() {
                json!({"content": or_null(&it["result"]["content"]), "structured_content": or_null(&it["result"]["structuredContent"])})
            } else {
                Value::Null
            };
            let err = if it["error"].is_object() {
                json!({"message": s_of(&it["error"], "message")})
            } else {
                Value::Null
            };
            Some(json!({
                "id": id, "type": "mcp_tool_call", "server": s("server"), "tool": s("tool"),
                "arguments": or_null(&it["arguments"]), "result": res, "error": err, "status": status,
            }))
        }
        "webSearch" => Some(json!({
            "id": id, "type": "web_search", "query": s("query"), "action": or_null(&it["action"]),
        })),
        _ => None,
    }
}

fn s_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

// ---- The exchange with whoever started it ----

/// Keeps the provider's normal output while collecting the final answer
/// in neutral form for the return rows on both sessions (exchangePrinter).
struct ExchangePrinter {
    inner: Box<dyn Printer>,
    decoder: rush_host::Decoder,
    text: String,
}

impl Printer for ExchangePrinter {
    fn line(&mut self, raw: &[u8], l: Option<&Line>, out: &mut dyn Write, err: &mut dyn Write) {
        self.inner.line(raw, l, out, err);
        for l in self.decoder.decode(raw).unwrap_or_default() {
            match l {
                Line::Event(Event::Message(m)) if m.role == "assistant" && m.parent.is_empty() => {
                    let texts: Vec<&str> = m
                        .parts
                        .iter()
                        .filter(|p| p.kind == PartKind::Text)
                        .map(|p| p.text.as_str())
                        .collect();
                    if !texts.is_empty() {
                        self.text = texts.join("\n");
                    }
                }
                Line::Event(Event::TurnEnd(e)) if !e.text.is_empty() => self.text = e.text,
                _ => {}
            }
        }
    }

    fn finish(&mut self, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        self.inner.finish(out, err)
    }

    fn answer(&self) -> String {
        self.text.clone()
    }
}

/// The task this run is, as the rush session in RUSH_SESSION hands it to
/// the session `to`; None outside one (outgoingExchange).
pub(crate) fn outgoing_exchange(
    to: &str,
    phase: &str,
    text: &str,
    images: &[String],
) -> Option<Exchange> {
    let from = std::env::var("RUSH_SESSION").unwrap_or_default();
    if from.is_empty() || from == to {
        return None;
    }
    // A stale or unverifiable environment value is not attributed to an
    // agent.
    read_config(&from).ok()?;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .ok()?;
    let id: String = b.iter().map(|b| format!("{b:02x}")).collect();
    Some(Exchange {
        id,
        direction: "received".into(),
        phase: phase.into(),
        text: text.into(),
        images: images.to_vec(),
        sender: rush_host::subtool::peer(&from),
        receiver: rush_host::subtool::peer(to),
        ..Exchange::default()
    })
}

/// Records e in session id's conversation, once its host says it has it
/// (recordExchangeAt).
fn record_exchange_at(id: &str, e: Exchange) -> Result<(), String> {
    if id.is_empty() {
        return Ok(());
    }
    let c = Client::dial(id).map_err(|e| e.to_string())?;
    let mut proto = 0;
    await_line(&c, Duration::from_secs(5), |l| match l {
        Line::Info(i) => {
            proto = i.proto;
            true
        }
        _ => false,
    })
    .map_err(|e| e.to_string())?;
    if proto < 8 {
        return Err(format!(
            "session {id} runs an older host; restart it to show agent exchanges"
        ));
    }
    let (want_id, want_dir) = (e.id.clone(), e.direction.clone());
    c.record_exchange(e).map_err(|e| e.to_string())?;
    await_line(&c, Duration::from_secs(5), |l| {
        matches!(l, Line::Event(Event::Exchange(x)) if x.id == want_id && x.direction == want_dir)
    })
    .map_err(|e| e.to_string())
}

/// The task as sent, in the starting session's conversation
/// (mirrorOutgoing).
pub(crate) fn mirror_outgoing(e: &Exchange) -> Result<(), String> {
    let mut copy = e.clone();
    copy.direction = "sent".into();
    let from = copy.sender.session_id.clone();
    record_exchange_at(&from, copy)
}

/// The run's answer, sent from it and received by whoever started it
/// (recordReturn).
fn record_return(request: &Exchange, child: &str, text: &str, code: i32) {
    let mut result = Exchange {
        id: format!("{}-result", request.id),
        phase: "result".into(),
        text: text.into(),
        sender: rush_host::subtool::peer(child),
        receiver: request.sender.clone(),
        ..Exchange::default()
    };
    if result.text.is_empty() {
        result.text = format!("Agent finished with exit code {code} and no text response.");
    }
    result.direction = "sent".into();
    if let Err(e) = record_exchange_at(child, result.clone()) {
        eprintln!("rush: couldn't record outgoing agent result: {e}");
    }
    result.direction = "received".into();
    let to = result.receiver.session_id.clone();
    if let Err(e) = record_exchange_at(&to, result) {
        eprintln!("rush: couldn't record returned agent result: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    /// An agent known only by its program, as spawn_of reads one.
    struct Prog(&'static str);

    impl rush_core::adapter::Adapter for Prog {
        fn kind(&self) -> rush_core::adapter::Kind {
            rush_core::adapter::Kind::from(self.0)
        }
        fn name(&self) -> String {
            self.0.to_string()
        }
        fn features(&self) -> rush_core::adapter::Features {
            rush_core::adapter::Features::new()
        }
        fn level(&self) -> rush_core::adapter::Level {
            rush_core::adapter::Level::Preview
        }
        fn profiles(&self) -> Vec<rush_core::adapter::Profile> {
            Vec::new()
        }
        fn programmer(&self) -> Option<&dyn rush_core::adapter::Programmer> {
            Some(self)
        }
    }

    impl rush_core::adapter::Programmer for Prog {
        fn program(&self) -> (String, Vec<String>) {
            (self.0.to_string(), Vec::new())
        }
    }

    // spawn_test.go:51: the run is read as the shell would read the
    // command (convo.SpawnOf), each argument quoted as it needs.
    #[test]
    fn parse_run_reads_the_command_as_spawn_of_does() {
        let reg = Registry::new();
        reg.register(Prog("claude"));
        reg.register(Prog("codex"));
        let r = parse_run(&reg, "claude", &a(&["-p", "it's a; test | here"])).unwrap();
        assert_eq!(
            (r.kind.as_str(), r.prompt.as_str()),
            ("claude", "it's a; test | here")
        );
        let r = parse_run(&reg, "codex", &a(&["exec", "say hi"])).unwrap();
        assert_eq!((r.kind.as_str(), r.prompt.as_str()), ("codex", "say hi"));
        for (prog, args) in [
            ("codex", &["login"][..]),
            ("codex", &["--version"]),
            ("claude", &["mcp", "list"]),
            ("ls", &["-la"]),
        ] {
            assert!(parse_run(&reg, prog, &a(args)).is_none(), "{prog} {args:?}");
        }
    }

    #[test]
    fn claude_runs_rush_can_print() {
        let r = claude_run(&a(&["-p", "--model", "opus", "fix it"])).unwrap();
        assert_eq!(
            (
                r.kind.as_str(),
                r.model.as_str(),
                r.prompt.as_str(),
                r.format.as_str()
            ),
            ("claude", "opus", "fix it", "text")
        );
        let r = claude_run(&a(&[
            "-p",
            "--output-format=stream-json",
            "--verbose",
            "--include-partial-messages",
            "x",
        ]))
        .unwrap();
        assert!(r.partial && r.format == "stream-json");
        let r = claude_run(&a(&["--print", "--dangerously-skip-permissions"])).unwrap();
        assert_eq!(
            (r.mode.as_str(), r.prompt.as_str()),
            ("bypassPermissions", "")
        );
        for no in [
            &["fix it"][..],                                // not -p
            &["-p", "--output-format", "stream-json", "x"], // stream-json wants --verbose
            &["-p", "--output-format", "json", "--verbose", "x"],
            &["-p", "--include-partial-messages", "x"],
            &["-p", "--resume", "abc", "x"], // a flag it isn't told of
            &["-p", "a", "b"],               // two words
            &["-p", "--output-format", "yaml", "x"],
        ] {
            assert!(claude_run(&a(no)).is_none(), "{no:?}");
        }
        let r = claude_run(&a(&["-p", "--", "-starts with a dash"])).unwrap();
        assert_eq!(r.prompt, "-starts with a dash");
    }

    #[test]
    fn codex_runs_rush_can_print() {
        let r = codex_run(&a(&[
            "exec",
            "-s",
            "workspace-write",
            "-m",
            "gpt-5",
            "--json",
            "-o",
            "/tmp/o",
            "do it",
        ]))
        .unwrap();
        assert_eq!(
            (
                r.mode.as_str(),
                r.model.as_str(),
                r.prompt.as_str(),
                r.json,
                r.last_msg.as_str()
            ),
            ("auto", "gpt-5", "do it", true, "/tmp/o")
        );
        assert_eq!(r.stdin_note, "Reading additional input from stdin...");
        let r = codex_run(&a(&[
            "--yolo",
            "e",
            "-c",
            "model_reasoning_effort=\"high\"",
        ]))
        .unwrap();
        assert_eq!(
            (r.mode.as_str(), r.effort.as_str(), r.prompt.as_str()),
            ("full-access", "high", "")
        );
        assert_eq!(r.stdin_note, "Reading prompt from stdin...");
        let r = codex_run(&a(&["exec", "-"])).unwrap();
        assert_eq!((r.prompt.as_str(), r.stdin_note.as_str()), ("", ""));
        for no in [
            &["do it"][..],
            &["exec", "resume", "x"][..],
            &["exec", "review"][..],
            &["exec", "-s", "bogus", "x"][..],
            &["exec", "-c", "model=x", "x"][..],
        ] {
            assert!(codex_run(&a(no)).is_none(), "{no:?}");
        }
    }

    #[test]
    fn stdin_joins_the_prompt_as_the_program_would() {
        let reg = Registry::new();
        let claude = WorkRun {
            kind: "claude".into(),
            prompt: "sum up".into(),
            ..WorkRun::default()
        };
        assert_eq!(
            with_stdin(&reg, &claude, b"the text").unwrap(),
            "sum up\nthe text"
        );
        let codex = WorkRun {
            kind: "codex".into(),
            prompt: "sum up".into(),
            ..WorkRun::default()
        };
        assert_eq!(
            with_stdin(&reg, &codex, b"the text").unwrap(),
            "sum up\n\n<stdin>\nthe text\n</stdin>"
        );
        assert!(
            with_stdin(
                &reg,
                &WorkRun {
                    kind: "x".into(),
                    ..WorkRun::default()
                },
                b"t"
            )
            .is_none()
        );
        let bare = WorkRun {
            kind: "claude".into(),
            ..WorkRun::default()
        };
        assert!(with_stdin(&reg, &bare, b"  \n").is_none());
    }

    #[test]
    fn first_words_name_it() {
        assert_eq!(
            first_words_of("fix the flaky test in the parser please"),
            "fix the flaky test in the"
        );
    }

    fn print(p: &mut dyn Printer, lines: &[&str]) -> (String, String, i32) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        for l in lines {
            let decoded = rush_host::decode(l.as_bytes()).ok();
            p.line(l.as_bytes(), decoded.as_ref(), &mut out, &mut err);
        }
        let code = p.finish(&mut out, &mut err);
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
            code,
        )
    }

    /// claude -p prints the result's text, the result, or every line but
    /// rush's own and the control traffic.
    #[test]
    fn claude_prints_as_claude_p() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            r#"{"type":"control_request","request_id":"1"}"#,
            r#"{"type":"stream_event","event":{}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done it"}"#,
        ];
        let text = WorkRun {
            format: "text".into(),
            ..WorkRun::default()
        };
        assert_eq!(
            print(&mut ClaudeOut::new(text), &lines),
            ("done it\n".into(), String::new(), 0)
        );
        let json = WorkRun {
            format: "json".into(),
            ..WorkRun::default()
        };
        assert_eq!(
            print(&mut ClaudeOut::new(json), &lines).0,
            format!("{}\n", lines[3])
        );
        let stream = WorkRun {
            format: "stream-json".into(),
            ..WorkRun::default()
        };
        assert_eq!(
            print(&mut ClaudeOut::new(stream), &lines).0,
            format!("{}\n{}\n", lines[0], lines[3])
        );
        assert_eq!(
            print(&mut ClaudeOut::new(WorkRun::default()), &[]).2,
            1,
            "no result"
        );
    }

    #[test]
    fn exec_items_as_codex_exec_prints_them() {
        let it = exec_item(
            "item_1",
            Some(r#"{"type":"commandExecution","command":"ls","aggregatedOutput":"a\n","exitCode":0,"status":"completed"}"#),
        )
        .unwrap();
        assert_eq!(
            it,
            json!({"id":"item_1","type":"command_execution","command":"ls","aggregated_output":"a\n","exit_code":0,"status":"completed"})
        );
        let it = exec_item(
            "item_2",
            Some(r#"{"type":"fileChange","changes":[{"path":"a.rs","kind":{"type":"update"}}],"status":"inProgress"}"#),
        )
        .unwrap();
        assert_eq!(
            it,
            json!({"id":"item_2","type":"file_change","changes":[{"path":"a.rs","kind":"update"}],"status":"in_progress"})
        );
        assert!(exec_item("x", Some(r#"{"type":"agentMessage"}"#)).is_none());
        assert!(exec_item("x", None).is_none());
    }

    /// codex exec's words on stderr and its last message on stdout; the
    /// turn's tokens counted.
    #[test]
    fn codex_prints_as_exec() {
        let r = WorkRun {
            kind: "codex".into(),
            prompt: "do it".into(),
            ..WorkRun::default()
        };
        let mut o = CodexOut::new(r, "/w".into());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let evs = [
            Event::Init(Box::new(rush_core::event::Init {
                version: "v0.50.0".into(),
                model: "gpt-5".into(),
                session_id: "t1".into(),
                ..Default::default()
            })),
            Event::Status(rush_core::event::Status {
                busy: true,
                ..Default::default()
            }),
            Event::Message(rush_core::event::Message {
                role: "assistant".into(),
                id: "m".into(),
                parts: vec![rush_core::event::Part {
                    kind: PartKind::Text,
                    text: "All done.".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            Event::TurnEnd(rush_core::event::TurnEnd {
                tokens: rush_core::agent::TokenUsage {
                    input: 10,
                    output: 5,
                    ..Default::default()
                },
                ..Default::default()
            }),
        ];
        for e in evs {
            o.line(b"", Some(&Line::Event(e)), &mut out, &mut err);
        }
        assert_eq!(o.finish(&mut out, &mut err), 0);
        assert_eq!(String::from_utf8(out).unwrap(), "All done.\n");
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "OpenAI Codex v0.50.0\n--------\nworkdir: /w\nmodel: gpt-5\nsession id: t1\n--------\nuser\ndo it\ncodex\nAll done.\ntokens used\n15\n"
        );
    }

    #[test]
    fn once_prints_the_last_answer_or_why_not() {
        let mut o = OnceOut::default();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        o.line(
            b"",
            Some(&Line::Event(Event::TurnEnd(rush_core::event::TurnEnd {
                reason: "error".into(),
                err: "no key".into(),
                ..Default::default()
            }))),
            &mut out,
            &mut err,
        );
        assert_eq!(o.finish(&mut out, &mut err), 1);
        assert_eq!(String::from_utf8(err).unwrap(), "Error: no key\n");
    }
}
