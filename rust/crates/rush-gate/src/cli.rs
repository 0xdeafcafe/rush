//! `rush gate`: what agents and their shims call. Same words, output and
//! exit codes as Go, since agents run it.
//! Ported from cmd/rush/gate.go:1-578.

use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};

use crate::gate::{AcquireError, Ask, Cancel, Gate, HELD_ENV, Run, Wait, bin_dir};
use crate::godur;
use crate::heavy::{heavy, worker_env_here};
use crate::hook;
use crate::host::{Fields, Host};
use crate::node::{base, node_name};
use crate::plugin::{self, Plugin};
use crate::rules::size;
use crate::share::{PRESSURE_CRITICAL, PRESSURE_NORMAL, PRESSURE_WARN};
use crate::watch::Watch;

/// rush gate's help.
pub const GATE_USAGE: &str = r#"rush gate — share the machine between intensive programs your agents run (the gate plugin)

  rush gate run [--name N] [--dir D] -- cmd args…
                    run cmd once there's memory for it, and watch it
  rush gate shim <name> args…
                    what the gate's shims run: <name> off PATH, gated
  rush gate hold --ready F --pid P [--dir D] -- script args…
                    what the node preload runs: gates node process P
                    running script, saying so in F, and watches it
  rush gate status  what runs and what waits, and why, and the shims and
                    node preload brought up to date
  rush gate hook    Claude Code's PreToolUse hook: a gated Bash call runs
                    under rush gate run, and with search-guard on a
                    wasteful search is refused

Turn it on with "rush plugin on gate"; its settings are in Settings, Plugins.
"#;

/// Everything the gate's commands work with.
pub struct Ctx {
    pub gate: Gate,
    pub plugin: Plugin,
    pub host: Arc<dyn Host>,
    /// The running rush, links resolved (plugin::exe).
    pub exe: String,
}

impl Ctx {
    /// The gate as this process sees it, over the sessions' own files.
    pub fn new(host: Arc<dyn Host>) -> Ctx {
        Ctx {
            gate: Gate::new(),
            plugin: Plugin::new(),
            host,
            exe: plugin::exe(),
        }
    }
}

/// `rush gate …`: its exit code. stdin, stdout and stderr are the
/// process's own: `run` hands them to the program it runs.
pub fn gate_cmd(ctx: &Ctx, args: &[String]) -> i32 {
    let mut out = io::stdout();
    let mut err = io::stderr();
    if args.is_empty() {
        let _ = out.write_all(GATE_USAGE.as_bytes());
        return 0;
    }
    // The real program is found past the shims, so a shim never runs itself.
    let path = without_dir(
        &std::env::var("PATH").unwrap_or_default(),
        &bin_dir(ctx.gate.root()),
    );
    match args[0].as_str() {
        "run" => match gate_run_args(&args[1..]) {
            // memory is the machine's: where it runs doesn't matter
            Ok((name, _, argv)) => gate_run(ctx, &path, &name, &argv),
            Err(e) => {
                let _ = write!(
                    err,
                    "rush gate: {e}\nusage: rush gate run [--name N] [--dir D] -- cmd args…\n"
                );
                2
            }
        },
        "shim" => {
            if args.len() < 2 {
                let _ = writeln!(err, "usage: rush gate shim <name> args…");
                return 2;
            }
            gate_run(ctx, &path, &args[1], &args[1..])
        }
        "hold" => gate_hold(ctx, &args[1..]),
        "status" => {
            // The plugin's own process runs with its data folder for a
            // home, so its shims land there: the ones sessions find are
            // written here, as a session's start writes them.
            ctx.plugin.write_shims(ctx.gate.root());
            gate_status(ctx, &mut out);
            0
        }
        "hook" => {
            gate_hook(ctx, &mut io::stdin().lock(), &mut out);
            0
        }
        _ => {
            let _ = err.write_all(GATE_USAGE.as_bytes());
            2
        }
    }
}

/// rush gate run's flags: its name, folder and command.
pub fn gate_run_args(args: &[String]) -> Result<(String, String, Vec<String>), String> {
    let (mut name, mut dir) = (String::new(), String::new());
    let mut args = args;
    while let Some(a) = args.first() {
        if a == "--" {
            if args.len() < 2 {
                return Err("no command".into());
            }
            return Ok((name, dir, args[1..].to_vec()));
        }
        if (a == "--name" || a == "--dir") && args.len() > 1 {
            if a == "--name" {
                name = args[1].clone();
            } else {
                dir = args[1].clone();
            }
            args = &args[2..];
            continue;
        }
        if a.starts_with('-') {
            return Err(format!("unknown flag {a}"));
        }
        return Ok((name, dir, args.to_vec()));
    }
    Err("no command".into())
}

/// Runs argv once the gate lets it, and is its exit code. Without a rule
/// for it, with the gate off, or inside a gated program, it just runs.
fn gate_run(ctx: &Ctx, path_env: &str, name: &str, argv: &[String]) -> i32 {
    let mut err = io::stderr();
    let path = match look_path(&argv[0], path_env) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(err, "rush gate: {e}");
            return 127;
        }
    };
    let name = if name.is_empty() {
        base(&argv[0]).to_string()
    } else {
        name.to_string()
    };
    let mut rule = None;
    let held = std::env::var(HELD_ENV).is_ok_and(|v| !v.is_empty());
    if !held && !ctx.gate.held_for(std::process::id() as i32) {
        rule = ctx.plugin.rules().and_then(|r| r.get(&name).copied());
    }
    // A shim's run is judged by its words; the hook judged a wrapped one.
    if rule.is_some() && base(&argv[0]) == name && !heavy(&name, &argv[1..]) {
        rule = None;
    }
    let Some(rule) = rule else {
        rush_events::flush();
        let e = std::process::Command::new(&path)
            .arg0(&argv[0])
            .args(&argv[1..])
            .env("PATH", path_env)
            .exec();
        let _ = writeln!(err, "rush gate: {}", go_errno(&e));
        return 126;
    };
    let own = owner(ctx);
    let signals = Signals::new();
    let run = own.acquire(
        ctx,
        &signals.cancel,
        Ask {
            name: name.clone(),
            rule,
            session: own.key.clone(),
            reserve: ctx.plugin.reserve(),
        },
        true,
    );
    let run = match run {
        Ok(r) => r,
        Err(AcquireError::Cancelled) => return 130,
        Err(e) => {
            let _ = writeln!(err, "rush gate: {e}");
            return 1;
        }
    };
    let mut env: Vec<(String, String)> = vec![
        ("PATH".into(), path_env.into()),
        (HELD_ENV.into(), "system".into()),
    ];
    for kv in worker_env_here(&name, argv) {
        if let Some((k, v)) = kv.split_once('=') {
            env.push((k.into(), v.into()));
        }
    }
    let child = std::process::Command::new(&path)
        .arg0(&argv[0])
        .args(&argv[1..])
        .envs(env)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(err, "rush gate: {}", go_errno(&e));
            run.release();
            return 126;
        }
    };
    signals.forward(child.id() as i32, run.clone());
    let watching = Cancel::new();
    let watcher = {
        let run = run.clone();
        let w = own.watch(ctx, child.id() as i32, true);
        let watching = watching.clone();
        std::thread::spawn(move || run.watch(&watching, w))
    };
    let status = child.wait();
    watching.cancel();
    let _ = watcher.join();
    signals.close();
    run.release();
    match status {
        Ok(s) => match s.signal() {
            Some(sig) => 128 + sig,
            None => s.code().unwrap_or(1),
        },
        Err(_) => 1,
    }
}

/// SIGINT and SIGTERM while a run waits or runs: waiting, they stop the
/// wait; running, they go to the program, and wake it if it's paused.
/// Where signals go once the program runs: its pid, and the run to wake.
type Forward = Arc<Mutex<Option<(i32, Arc<Run>)>>>;

struct Signals {
    cancel: Cancel,
    to: Forward,
    handle: Option<signal_hook::iterator::Handle>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Signals {
    fn new() -> Signals {
        let cancel = Cancel::new();
        let to: Forward = Arc::default();
        let Ok(mut sigs) = signal_hook::iterator::Signals::new([libc::SIGINT, libc::SIGTERM])
        else {
            return Signals {
                cancel,
                to,
                handle: None,
                thread: None,
            };
        };
        let handle = sigs.handle();
        let (c, t) = (cancel.clone(), to.clone());
        let thread = std::thread::spawn(move || {
            for sig in sigs.forever() {
                match &*t.lock().unwrap_or_else(|e| e.into_inner()) {
                    Some((pid, run)) => {
                        let _ = rush_sys::proc::kill(*pid, sig);
                        run.wake();
                    }
                    None => c.cancel(),
                }
            }
        });
        Signals {
            cancel,
            to,
            handle: Some(handle),
            thread: Some(thread),
        }
    }

    /// From now on, signals go to pid.
    fn forward(&self, pid: i32, run: Arc<Run>) {
        *self.to.lock().unwrap_or_else(|e| e.into_inner()) = Some((pid, run));
    }

    fn close(mut self) {
        if let Some(h) = self.handle.take() {
            h.close();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Who a gated run is for: its rush session, if any, and the agent's
/// program above it, if any.
struct Owner {
    session: String,
    agent_pid: i32,
    /// The agent, for the fair order.
    key: String,
    host: Arc<dyn Host>,
}

/// Who the run this process holds is for.
fn owner(ctx: &Ctx) -> Owner {
    let session = std::env::var("RUSH_SESSION").unwrap_or_default();
    let tab = rush_sys::proc::snapshot();
    // SAFETY: getppid takes nothing.
    let ppid = unsafe { libc::getppid() };
    let mut agent_pid = 0;
    let mut pid = ppid;
    for _ in 0..64 {
        if pid <= 1 {
            break;
        }
        let Some(p) = tab.procs.get(&pid) else {
            break;
        };
        if ctx.host.is_agent_program(&p.comm) {
            agent_pid = pid;
            break;
        }
        pid = p.ppid;
    }
    let key = if !session.is_empty() {
        session.clone()
    } else if agent_pid > 0 {
        format!("pid {agent_pid}")
    } else {
        format!("pid {ppid}")
    };
    Owner {
        session,
        agent_pid,
        key,
        host: ctx.host.clone(),
    }
}

fn fields(v: Value) -> Fields {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl Owner {
    /// Records an event of the run's: in its session's log, else rush's
    /// with the agent it's for.
    fn log(&self, kind: &str, mut f: Fields) {
        if !self.session.is_empty() {
            self.host.log_event(&self.session, kind, f);
            return;
        }
        f.insert("agent".into(), Value::String(self.key.clone()));
        self.host.log(kind, f);
    }

    /// Gate::acquire, logging each change while it waits, the run let in,
    /// and a failure; tell says why it waits on stderr too.
    fn acquire(
        &self,
        ctx: &Ctx,
        cancel: &Cancel,
        a: Ask,
        tell: bool,
    ) -> Result<Arc<Run>, AcquireError> {
        let start = Instant::now();
        let mut waited = false;
        let name = a.name.clone();
        let memory = a.rule.memory;
        let mut on_wait = |w: Wait| {
            waited = true;
            self.log(
                "gate-waiting",
                fields(json!({"name": name, "ahead": w.ahead, "running": w.running, "need": w.need, "room": w.room})),
            );
            if tell {
                tell_wait(&mut io::stderr(), &name, &w);
            }
        };
        let r = ctx.gate.acquire(cancel, a, Some(&mut on_wait));
        let took = godur::string(godur::round_secs(start.elapsed()));
        match &r {
            Ok(_) => {
                let mut f = fields(json!({"name": name, "memory": memory}));
                if waited {
                    f.insert("waited".into(), Value::String(took));
                }
                self.log("gate-admitted", f);
            }
            Err(AcquireError::Cancelled) => {
                if waited {
                    self.log(
                        "gate-gave-up",
                        fields(
                            json!({"name": name, "why": "its waiting was stopped", "waited": took}),
                        ),
                    );
                }
            }
            Err(e) => self.log(
                "gate-failed",
                fields(json!({"name": name, "err": e.to_string()})),
            ),
        }
        r
    }

    /// What the watchdog needs to look after root's run: with echo, what
    /// the agent is told goes to stderr too, as well as its inbox.
    fn watch(&self, ctx: &Ctx, root: i32, echo: bool) -> Watch {
        let me = Arc::new(Owner {
            session: self.session.clone(),
            agent_pid: self.agent_pid,
            key: self.key.clone(),
            host: self.host.clone(),
        });
        let mut w = Watch {
            root,
            agent_pid: self.agent_pid,
            floor: ctx.plugin.floor(),
            ..Watch::default()
        };
        let o = me.clone();
        w.log = Some(Box::new(move |kind, f| o.log(kind, f)));
        if !self.session.is_empty() {
            let o = me.clone();
            w.session_alive = Some(Box::new(move || o.host.session_alive(&o.session)));
        }
        let o = me;
        w.tell = Some(Box::new(move |s| {
            if echo {
                let _ = writeln!(io::stderr(), "{s}");
            }
            if !o.session.is_empty() {
                let r = o.host.notice(&o.session, s);
                let mut f = fields(json!({"text": clip_text(s)}));
                if let Err(e) = r {
                    f.insert("err".into(), Value::String(e));
                }
                o.log("gate-told", f);
            }
        }));
        w
    }
}

/// Says on w why name waits.
fn tell_wait(w: &mut dyn Write, name: &str, wait: &Wait) {
    if wait.need > 0 {
        let _ = writeln!(
            w,
            "rush gate: {name} waiting for memory: it needs {}, there's {} past what's kept free ({} running)…",
            size(wait.need),
            size(wait.room),
            wait.running
        );
        return;
    }
    let _ = writeln!(
        w,
        "rush gate: {name} waiting its turn: {} ahead, the agents running least first ({} running)…",
        wait.ahead, wait.running
    );
}

/// The node preload's holder: it says in --ready whether node process
/// --pid, running a script, is gated, then holds and watches its run until
/// that process is gone. It answers "free" when the script isn't gated, or
/// can't be judged; the preload runs it unheld then, as without a gate.
pub fn gate_hold(ctx: &Ctx, args: &[String]) -> i32 {
    let mut ready = String::new();
    let mut pid = 0i32;
    let mut args = args;
    while !args.is_empty() && args[0] != "--" {
        if args.len() < 2 {
            return 2;
        }
        match args[0].as_str() {
            "--ready" => ready = args[1].clone(),
            "--dir" => {} // memory is the machine's: where it runs doesn't matter now
            "--pid" => pid = args[1].parse().unwrap_or(0),
            _ => return 2,
        }
        args = &args[2..];
    }
    if !args.is_empty() {
        args = &args[1..]; // the "--"
    }
    if ready.is_empty() || pid <= 0 || args.len() < 2 {
        if !ready.is_empty() {
            let _ = crate::gate::write_file(Path::new(&ready), b"free\n", 0o600);
        }
        return 2;
    }
    let say = Say::new(ready);
    let rules = ctx.plugin.rules().unwrap_or_default();
    let name = node_name(&args[1], &rules);
    let Some(rule) = rules
        .get(&name)
        .copied()
        .filter(|_| heavy(&name, &args[2..]))
    else {
        say.say(&["free"]);
        return 0; // not gated: nothing done, nothing logged
    };
    // Waiting ends with the node process: Ctrl-C there, or its agent stopping it.
    let cancel = Cancel::new();
    {
        let c = cancel.clone();
        std::thread::spawn(move || {
            crate::exit::wait_gone(pid);
            c.cancel();
        });
    }
    let own = owner(ctx);
    let ask = Ask {
        name: name.clone(),
        rule,
        session: own.key.clone(),
        reserve: ctx.plugin.reserve(),
    };
    let run = match own.acquire(ctx, &cancel, ask, true) {
        Ok(r) => r,
        Err(e) => {
            own.log(
                "gate-hold",
                fields(
                    json!({"name": name, "pid": pid, "script": clip_text(&args[1]), "held": false,
                    "why": format!("its waiting ended: {e}")}),
                ),
            );
            say.say(&["free"]);
            return 0;
        }
    };
    own.log(
        "gate-hold",
        fields(json!({"name": name, "pid": pid, "script": clip_text(&args[1]), "held": true})),
    );
    let mut lines = vec!["held".to_string(), format!("{HELD_ENV}=system")];
    lines.extend(worker_env_here(&name, &args[2..]));
    say.say(&lines.iter().map(String::as_str).collect::<Vec<_>>());
    // The node process's stderr is no longer ours to keep open.
    // SAFETY: closing our own stderr; nothing writes it after.
    unsafe {
        libc::close(2);
    }
    run.watch(&cancel, own.watch(ctx, pid, false));
    run.release();
    0
}

/// The holder's answer in --ready, said once; until it's said, it beats
/// on ready.beat each second so the preload knows it's alive.
struct Say {
    ready: String,
    said: Mutex<bool>,
    beating: Cancel,
    beat: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Say {
    fn new(ready: String) -> Say {
        let beating = Cancel::new();
        let beat_path = format!("{ready}.beat");
        let b = beating.clone();
        let beat = std::thread::spawn(move || {
            loop {
                let now = SystemTime::now();
                let touched = std::fs::File::options()
                    .write(true)
                    .open(&beat_path)
                    .and_then(|f| {
                        f.set_times(
                            std::fs::FileTimes::new()
                                .set_accessed(now)
                                .set_modified(now),
                        )
                    });
                if touched.is_err() {
                    let _ = crate::gate::write_file(Path::new(&beat_path), b"", 0o600);
                }
                if b.wait(Duration::from_secs(1)) {
                    return;
                }
            }
        });
        Say {
            ready,
            said: Mutex::new(false),
            beating,
            beat: Mutex::new(Some(beat)),
        }
    }

    fn say(&self, lines: &[&str]) {
        let mut said = self.said.lock().unwrap_or_else(|e| e.into_inner());
        if *said {
            return;
        }
        *said = true;
        self.beating.cancel();
        if let Some(t) = self.beat.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(format!("{}.beat", self.ready));
        let tmp = format!("{}.tmp", self.ready);
        let _ = crate::gate::write_file(
            Path::new(&tmp),
            format!("{}\n", lines.join("\n")).as_bytes(),
            0o600,
        );
        let _ = std::fs::rename(&tmp, &self.ready);
    }
}

impl Drop for Say {
    fn drop(&mut self) {
        self.say(&["free"]);
    }
}

/// Answers Claude Code's PreToolUse hook: a Bash call that runs a gated
/// program comes back with its command under rush gate run, and one of
/// more than a few commands runs as a script rush can follow and stop; the
/// rest of its input as it was. Anything else, or anything it can't read,
/// gets nothing: the call goes ahead as it is.
pub fn gate_hook(ctx: &Ctx, r: &mut dyn Read, w: &mut dyn Write) {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct In {
        tool_name: String,
        tool_input: Option<std::collections::BTreeMap<String, rush_state::json::RawJson>>,
        cwd: String,
        tool_use_id: String,
    }
    let mut b = Vec::new();
    if r.take(4 << 20).read_to_end(&mut b).is_err() {
        return;
    }
    let Ok(mut inp) = rush_state::json::from_slice::<In>(&b) else {
        return;
    };
    if inp.tool_name != "Bash" {
        return;
    }
    let Some(input) = inp.tool_input.as_mut() else {
        return;
    };
    let Some(command) = input
        .get("command")
        .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
    else {
        return;
    };
    let exe = ctx.exe.clone();
    let timeout = input
        .get("timeout")
        .and_then(|v| serde_json::from_str::<f64>(v.get()).ok())
        .unwrap_or(0.0);
    let background = input
        .get("run_in_background")
        .and_then(|v| serde_json::from_str::<bool>(v.get()).ok())
        .unwrap_or(false);
    // A wasteful search is refused while search-guard is on. Waiting on a
    // pull request's checks is rush's to do, not the agent's, while the
    // watch plugin is on.
    let mut deny = ctx.host.search_refusal(&hook::unwrap(&command));
    let mut watched = String::new();
    if deny.is_empty() {
        // As Go: the milliseconds made whole first, then a duration.
        let timeout = Duration::from_millis((timeout as i64).max(0) as u64);
        (watched, deny) = ctx.host.watch_redirect(&command, background, timeout, &exe);
    }
    let hook_log = |kind: &str, mut f: Fields| {
        f.insert("tool_use_id".into(), Value::String(inp.tool_use_id.clone()));
        f.insert("command".into(), Value::String(clip_text(&command)));
        match std::env::var("RUSH_SESSION") {
            Ok(id) if !id.is_empty() => ctx.host.log_event(&id, kind, f),
            _ => ctx.host.log(kind, f),
        }
    };
    if !deny.is_empty() {
        hook_log("hook-denied", fields(json!({"why": clip_text(&deny)})));
        let out = json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": deny}});
        if let Ok(mut b) = rush_state::json::to_vec(&out) {
            b.push(b'\n');
            let _ = w.write_all(&b);
        }
        return;
    }
    let name = hook::gated(&command, ctx.plugin.rules().as_ref());
    let mut held = None;
    if !exe.is_empty() && !rush_gh::pushlater::off() {
        held = rush_gh::pushlater::rewrite(&command, &exe);
    }
    let (mut wrapped, mut why) = (String::new(), String::new());
    if let Some(h) = held {
        wrapped = h;
        why = "a push waits until the branch is quiet, so a burst reaches CI once".into();
    } else if !watched.is_empty() {
        wrapped = watched;
        why = "it waits on a pull request's checks: rush watch pr does that without polling".into();
    } else if !name.is_empty() && !exe.is_empty() {
        if inp.cwd.is_empty() {
            inp.cwd = std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        wrapped = hook::wrap(&exe, &name, &inp.cwd, &command);
        why = format!("it runs {name}, which the gate shares out");
    } else if rush_sys::script::long(&command) && !command.contains("rush-script") {
        let scripts = rush_sys::script::Scripts::new(rush_state::dir());
        let key = rush_sys::script::key(&inp.tool_use_id, &command);
        wrapped = scripts
            .wrap(&key, &command)
            .ok()
            .flatten()
            .unwrap_or_default();
        why = "it's long: run as a script rush can follow and stop".into();
    }
    if wrapped.is_empty() {
        return;
    }
    hook_log(
        "hook-rewrote",
        fields(json!({"to": clip_text(&wrapped), "why": why})),
    );
    let Ok(v) = rush_state::json::RawJson::from_value(&wrapped) else {
        return;
    };
    let mut input = inp.tool_input.take().unwrap_or_default();
    input.insert("command".into(), v);
    #[derive(serde::Serialize)]
    struct Out<'a> {
        #[serde(rename = "hookSpecificOutput")]
        hso: Hso<'a>,
    }
    #[derive(serde::Serialize)]
    struct Hso<'a> {
        #[serde(rename = "hookEventName")]
        event: &'a str,
        #[serde(rename = "updatedInput")]
        input: &'a std::collections::BTreeMap<String, rush_state::json::RawJson>,
    }
    let out = Out {
        hso: Hso {
            event: "PreToolUse",
            input: &input,
        },
    };
    if let Ok(mut b) = rush_state::json::to_vec(&out) {
        b.push(b'\n');
        let _ = w.write_all(&b);
    }
}

/// Writes the gate's rules, then each run going and each waiter in the
/// order they go, with why.
pub fn gate_status(ctx: &Ctx, w: &mut dyn Write) {
    match ctx.plugin.rules() {
        None => {
            let _ = writeln!(w, r#"The gate is off: "rush plugin on gate" turns it on."#);
        }
        Some(rules) => {
            for (n, r) in &rules {
                let _ = writeln!(
                    w,
                    "{n:<14} expects {}; stopped past {}, or holding {}",
                    size(r.memory),
                    godur::string(r.limit),
                    size(r.cap())
                );
            }
        }
    }
    let reserve = ctx.plugin.reserve();
    let m = ctx.gate.machine();
    let (avail, total) = m.memory();
    if total > 0 {
        let _ = writeln!(
            w,
            "\nMemory: {} available of {}, {} kept free; pressure {}.",
            size(avail),
            size(total),
            size((total as f64 * reserve) as u64),
            pressure_word(m.pressure_level())
        );
    }
    let (running, waiting) = ctx.gate.status(reserve);
    if running.is_empty() && waiting.is_empty() {
        let _ = writeln!(w, "\nNothing running or waiting.");
        return;
    }
    let mut names: std::collections::HashMap<String, String> = Default::default();
    let mut agent_name = |key: &str| -> String {
        names
            .entry(key.to_string())
            .or_insert_with(|| {
                ctx.host
                    .session_name(key)
                    .unwrap_or_else(|| key.to_string())
            })
            .clone()
    };
    let now = SystemTime::now();
    let age = |since: Option<SystemTime>| {
        godur::string(godur::round_secs(
            since
                .and_then(|s| now.duration_since(s).ok())
                .unwrap_or_default(),
        ))
    };
    if !running.is_empty() {
        let _ = writeln!(w, "\nRunning");
    }
    for t in &running {
        let why = if t.paused() {
            format!("  paused: {}", t.state.why)
        } else {
            String::new()
        };
        let _ = writeln!(
            w,
            "  {:<12} {:<24} {:>8}  nice {:<2}  {:>9}{why}",
            t.name,
            short(&agent_name(&t.session), 24),
            age(t.since),
            t.state.nice,
            size(t.state.footprint)
        );
    }
    if !waiting.is_empty() {
        let _ = writeln!(w, "\nWaiting, in the order they go");
    }
    for t in &waiting {
        let _ = writeln!(
            w,
            "  {} {:<12} {:<24} {:>8}  {}",
            t.place,
            t.ticket.name,
            short(&agent_name(&t.ticket.session), 24),
            age(t.ticket.since),
            t.why
        );
    }
}

/// A memory pressure level in words.
pub fn pressure_word(l: i32) -> &'static str {
    match l {
        PRESSURE_NORMAL => "normal",
        PRESSURE_WARN => "raised",
        PRESSURE_CRITICAL => "critical",
        _ => "unknown",
    }
}

/// The start of s, enough to know it by, for the event log.
pub fn clip_text(s: &str) -> String {
    if s.chars().count() > 300 {
        return s.chars().take(300).collect::<String>() + "…";
    }
    s.to_string()
}

/// s in at most n characters.
pub fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        return s.chars().take(n - 1).collect::<String>() + "…";
    }
    s.to_string()
}

/// path without d.
pub fn without_dir(path: &str, d: &Path) -> String {
    let d = clean(&d.to_string_lossy());
    path.split(':')
        .filter(|p| !p.is_empty() && clean(p) != d)
        .collect::<Vec<_>>()
        .join(":")
}

/// p as Go's filepath.Clean has it, as far as comparing folders goes: no
/// trailing slash, no doubled ones, no "." parts.
fn clean(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." if parts.last().is_some_and(|l| *l != "..") => {
                parts.pop();
            }
            ".." if abs => {}
            s => parts.push(s),
        }
    }
    let j = parts.join("/");
    match (abs, j.is_empty()) {
        (true, _) => format!("/{j}"),
        (false, true) => ".".into(),
        _ => j,
    }
}

/// file looked for as Go's exec.LookPath does, in path: a name with a
/// slash as it is; the error as Go words it.
pub fn look_path(file: &str, path: &str) -> Result<PathBuf, String> {
    if file.contains('/') {
        return match executable(Path::new(file)) {
            Ok(()) => Ok(PathBuf::from(file)),
            Err(e) => Err(format!("exec: {}: {e}", go_quote(file))),
        };
    }
    look_path_in(file, path).ok_or_else(|| {
        format!(
            "exec: {}: executable file not found in $PATH",
            go_quote(file)
        )
    })
}

/// file found in path's folders, as LookPath finds it; None when it isn't.
pub fn look_path_in(file: &str, path: &str) -> Option<PathBuf> {
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = Path::new(dir).join(file);
        if executable(&p).is_ok() {
            return Some(p);
        }
    }
    None
}

/// Whether p is a file this process may run; why not, as Go words it.
fn executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let m = std::fs::metadata(p).map_err(|e| format!("stat {}: {}", p.display(), go_errno(&e)))?;
    if !m.is_dir() && m.permissions().mode() & 0o111 != 0 {
        return Ok(());
    }
    Err("permission denied".into())
}

/// s quoted as Go's %q.
fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// An OS error as Go's syscall.Errno says it: "permission denied".
pub fn go_errno(e: &io::Error) -> String {
    let Some(n) = e.raw_os_error() else {
        return e.to_string();
    };
    // SAFETY: strerror returns a static string for any errno.
    let s = unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }
        .to_string_lossy()
        .into_owned();
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::tests::Fake;
    use crate::rules::GIB;

    /// A host that remembers what it was asked.
    #[derive(Default)]
    pub(crate) struct Fake2 {
        pub logged: Mutex<Vec<(String, String, Fields)>>,
        pub refuse: String,
    }

    impl Host for Fake2 {
        fn log_event(&self, id: &str, kind: &str, f: Fields) {
            self.logged
                .lock()
                .unwrap()
                .push((id.into(), kind.into(), f));
        }
        fn log(&self, kind: &str, f: Fields) {
            self.logged
                .lock()
                .unwrap()
                .push((String::new(), kind.into(), f));
        }
        fn notice(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn session_alive(&self, _: &str) -> bool {
            true
        }
        fn session_name(&self, id: &str) -> Option<String> {
            (id == "s1").then(|| "Fix the build".into())
        }
        fn is_agent_program(&self, comm: &str) -> bool {
            base(comm) == "claude"
        }
        fn search_refusal(&self, _: &str) -> String {
            self.refuse.clone()
        }
    }

    fn ctx(home: &Path, rules: Option<&str>, host: Arc<Fake2>) -> Ctx {
        Ctx {
            gate: Gate::at(home.join("gate"), Fake::new(64 * GIB)),
            plugin: Plugin {
                home: home.to_path_buf(),
                rules_env: rules.map(String::from),
            },
            host,
            exe: "/opt/rush".into(),
        }
    }

    fn hook(c: &Ctx, input: &str) -> String {
        let mut out = Vec::new();
        gate_hook(c, &mut input.as_bytes(), &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn run_args() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            gate_run_args(&s(&[
                "--name", "tsc", "--dir", "/x", "--", "bash", "-c", "tsc"
            ])),
            Ok(("tsc".into(), "/x".into(), s(&["bash", "-c", "tsc"])))
        );
        assert_eq!(
            gate_run_args(&s(&["go", "test"])),
            Ok((String::new(), String::new(), s(&["go", "test"])))
        );
        assert_eq!(gate_run_args(&s(&["--"])), Err("no command".into()));
        assert_eq!(gate_run_args(&s(&[])), Err("no command".into()));
        assert_eq!(
            gate_run_args(&s(&["-x", "go"])),
            Err("unknown flag -x".into())
        );
    }

    // A Bash call running a gated program comes back under rush gate run,
    // the rest of its input as it was; others get nothing.
    #[test]
    fn hook_wraps_gated_calls() {
        let home = tempfile::tempdir().unwrap();
        let host = Arc::new(Fake2::default());
        let c = ctx(home.path(), Some("tsc"), host.clone());
        let out = hook(
            &c,
            r#"{"tool_name":"Bash","tool_use_id":"toolu_1","cwd":"/w","tool_input":{"command":"tsc -b","description":"a <b> & c","timeout":120000}}"#,
        );
        assert_eq!(
            out,
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"'/opt/rush' gate run --name 'tsc' --dir '/w' -- bash -c 'tsc -b'","description":"a <b> & c","timeout":120000}}}"#
                .to_string()
                + "\n"
        );
        let logged = host.logged.lock().unwrap();
        assert_eq!(logged[0].1, "hook-rewrote");
        assert_eq!(logged[0].2["why"], "it runs tsc, which the gate shares out");
        drop(logged);
        for input in [
            r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#,
            r#"{"tool_name":"Read","tool_input":{"command":"tsc"}}"#,
            r#"{"tool_name":"Bash"}"#,
            "not json",
            r#"{"tool_name":"Bash","tool_input":{"command":"tsc --watch"}}"#,
        ] {
            assert_eq!(hook(&c, input), "", "{input}");
        }
        let off = ctx(home.path(), None, host.clone());
        assert_eq!(
            hook(
                &off,
                r#"{"tool_name":"Bash","tool_input":{"command":"tsc -b"}}"#
            ),
            ""
        );
    }

    // searchguard_test.go: a refused search is denied, with why.
    #[test]
    fn hook_denies_what_search_guard_refuses() {
        let home = tempfile::tempdir().unwrap();
        let host = Arc::new(Fake2 {
            refuse: "search the project, not the disk".into(),
            ..Fake2::default()
        });
        let c = ctx(home.path(), None, host);
        assert_eq!(
            hook(&c, r#"{"tool_name":"Bash","tool_input":{"command":"grep -r x /"}}"#),
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"search the project, not the disk"}}"#
                .to_string()
                + "\n"
        );
    }

    // scripthook_test.go TestScriptHook: a Bash call of more than three
    // commands over more than one line comes back as a script run, its
    // command kept as the script; a short one goes as it is.
    #[test]
    fn hook_runs_long_calls_as_scripts() {
        if rush_sys::script::bash().is_none() {
            return; // no bash 4 or later
        }
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let c = ctx(home.path(), None, Arc::new(Fake2::default()));
        let out = hook(
            &c,
            r#"{"tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{"command":"a && b\nc && d","description":"x"}}"#,
        );
        assert!(
            out.contains(r#""updatedInput""#)
                && out.contains("rush-script")
                && out.contains(r#""description":"x""#),
            "{out}"
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("scripts/toolu_1.sh")).unwrap(),
            "a && b\nc && d\n"
        );
        for cmd in ["go test ./...", "a && b && c && d"] {
            let input = format!(
                r#"{{"tool_name":"Bash","tool_use_id":"toolu_2","tool_input":{{"command":"{cmd}"}}}}"#
            );
            assert_eq!(hook(&c, &input), "", "{cmd}");
        }
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn status_says_whats_going() {
        let home = tempfile::tempdir().unwrap();
        let host = Arc::new(Fake2::default());
        let c = ctx(home.path(), None, host.clone());
        let mut out = Vec::new();
        gate_status(&c, &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "The gate is off: \"rush plugin on gate\" turns it on.\n\n\
             Memory: 64.0 GiB available of 64.0 GiB, 9.6 GiB kept free; pressure unknown.\n\n\
             Nothing running or waiting.\n"
        );
        let c = ctx(home.path(), Some("tsc=6G/45m"), host);
        let run = c
            .gate
            .acquire(
                &Cancel::new(),
                Ask {
                    name: "tsc".into(),
                    rule: crate::rules::Rule {
                        memory: 6 * GIB,
                        limit: Duration::from_secs(2700),
                    },
                    session: "s1".into(),
                    reserve: 0.15,
                },
                None,
            )
            .unwrap();
        let mut out = Vec::new();
        gate_status(&c, &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.starts_with(
                "tsc            expects 6.0 GiB; stopped past 45m0s, or holding 18.0 GiB\n"
            ),
            "{s}"
        );
        assert!(
            s.contains(
                "\nRunning\n  tsc          Fix the build                  0s  nice 0       0 MiB\n"
            ),
            "{s}"
        );
        run.release();
    }

    #[test]
    fn helpers_as_go_has_them() {
        assert_eq!(
            without_dir("/a/bin:/g/bin/:/usr/bin::/g//bin", Path::new("/g/bin")),
            "/a/bin:/usr/bin"
        );
        assert_eq!(short("abcdef", 4), "abc…");
        assert_eq!(short("abc", 4), "abc");
        assert_eq!(clip_text(&"x".repeat(301)).chars().count(), 301);
        assert_eq!(
            look_path("no-such-program-here", "/nonexistent").unwrap_err(),
            r#"exec: "no-such-program-here": executable file not found in $PATH"#
        );
        assert!(look_path("sh", "/nonexistent:/bin").is_ok());
        assert_eq!(
            go_errno(&io::Error::from_raw_os_error(libc::EACCES)),
            "permission denied"
        );
        let mut w = Vec::new();
        tell_wait(
            &mut w,
            "tsc",
            &Wait {
                ahead: 2,
                running: 3,
                need: 0,
                room: 0,
            },
        );
        tell_wait(
            &mut w,
            "go",
            &Wait {
                ahead: 0,
                running: 1,
                need: 2 * GIB,
                room: GIB,
            },
        );
        assert_eq!(
            String::from_utf8(w).unwrap(),
            "rush gate: tsc waiting its turn: 2 ahead, the agents running least first (3 running)…\n\
             rush gate: go waiting for memory: it needs 2.0 GiB, there's 1.0 GiB past what's kept free (1 running)…\n"
        );
    }
}
