//! `rush session`: rush-mode sessions without the view. start, send,
//! interrupt, stop, watch, info and list, with Go's words, flags and exit
//! codes; `--on MACHINE` is serve.rs's. send is how push-later's failed
//! push reaches its session; start is also how serve starts a session a
//! remote client asks for (start_requested).
//!
//! Ported from cmd/rush/session.go (sessionUsage :25, viewOf :52,
//! sessionCmd :64, pairs :137, uuidRe :152, sessionExists :156, waitInfo
//! :165, sessionStart :182, startSession :269, firstWordsOf :400,
//! sessionSend :419, sessionControl :571, sessionWatch :629, sessionInfo
//! :740, sessionList :767), serve.go:58 (serve's starts) and exchange.go:24
//! (outgoingExchange, here spawn.rs's).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use rush_core::adapter::Registry;
use rush_host::client::{Client, ClientError, await_line, ensure, read_info, resting};
use rush_host::events::log_event;
use rush_host::{Config, Info, Line, alive};
use rush_remote::{Session as View, StartRequest};
use serde_json::{Map, Value, json};

use crate::flags::{Flags, has_flag};

/// What `rush session help` says (sessionUsage).
pub const USAGE: &str = "rush session: run rush-mode sessions without the view

  rush session start --cwd DIR [--agent KIND] [--profile P] [--session-id UUID] [--resume] [--name N]
        [--prompt-file F] [--image PATH]... [--env K=V]... [--meta k=v]...
        [--binary PATH] [--model M] [--effort E] [--permission-mode M] [--json]

  --agent is the agent to run: claude, codex, copilot, gemini, kimi, opencode
  or vibe. Without it, the session's profile picks: --profile names one,
  else the folder's rule or the default profile says.
  rush session send <id> [--now] [--image PATH]...   message text on stdin; into the
        turn under way, or with --now stopping it
  rush session interrupt <id>
  rush session stop <id>
  rush session watch <id> [--json] [--until-idle]   what it says and does, as it
        happens, from the start of the turn under way; --until-idle stops
        when the turn ends
  rush session info <id> [--json]
  rush session list [--json] [--meta k=v]...
  rush session --on MACHINE <command> …   on another machine's rush serve
        (rush remote add); start takes --cwd --agent --profile --name
        --model --effort --prompt-file, or --resume SESSION to carry on one
        rush didn't start, which list --others lists
";

/// What the session commands run sessions with: the agents, how a host
/// is started, and whether one runs. A test hands a fake host's.
pub struct Env {
    pub reg: Registry,
    /// Writes a session's config and starts its host (host.Spawn).
    pub spawn: fn(&Registry, Config) -> Result<Config, String>,
    /// Whether a session's host runs (host.Alive of its pid).
    pub alive: fn(&Info) -> bool,
}

impl Env {
    /// This process's: hosts started as `rush host run`.
    pub fn process(reg: &Registry) -> Env {
        Env {
            reg: reg.clone(),
            spawn: |_, cfg| rush_host::spawn(cfg).map_err(|e| e.to_string()),
            alive: |i| alive(i.host_pid),
        }
    }

    /// A session as the session commands print it: its info, and whether
    /// its host runs (viewOf).
    fn view(&self, info: Info) -> View {
        let alive = (self.alive)(&info);
        View { info, alive }
    }
}

/// `rush session <sub>`: its exit code (sessionCmd). `--on` is the
/// caller's.
pub fn session_cmd(
    env: &Env,
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let Some(sub) = args.first().map(String::as_str) else {
        let _ = write!(stdout, "{USAGE}");
        return 0;
    };
    let rest = &args[1..];
    let (as_json, r) = match sub {
        "-h" | "--help" | "help" => {
            let _ = write!(stdout, "{USAGE}");
            return 0;
        }
        "start" => session_start(env, rest, stdin, stdout),
        "send" => (false, session_send(rest, stdin, stdout)),
        "interrupt" => (false, session_control(env, rest, stdout, false)),
        "stop" => (false, session_control(env, rest, stdout, true)),
        "watch" => session_watch(env, rest, stdout),
        "info" => session_info(env, rest, stdout),
        "list" => session_list(env, rest, stdout),
        other => (
            false,
            Err(format!("unknown session command {other:?}\n\n{USAGE}")),
        ),
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            if as_json {
                write_json(stdout, &json!({ "error": e }));
            } else {
                let _ = writeln!(stderr, "rush: {e}");
            }
            1
        }
    }
}

/// v as a line of JSON, as jsonx.Write writes it.
fn write_json<T: serde::Serialize + ?Sized>(w: &mut dyn Write, v: &T) {
    let mut b = rush_state::json::to_vec(v).unwrap_or_default();
    b.push(b'\n');
    let _ = w.write_all(&b);
}

/// key=value pairs, each given as a flag named what (pairs).
fn pairs(kvs: &[String], what: &str) -> Result<BTreeMap<String, String>, String> {
    let mut m = BTreeMap::new();
    for kv in kvs {
        match kv.split_once('=') {
            Some((k, v)) if !k.is_empty() => {
                m.insert(k.to_string(), v.to_string());
            }
            _ => return Err(format!("--{what} wants key=value, not {kv:?}")),
        }
    }
    Ok(m)
}

/// Whether s is a UUID, any case (uuidRe).
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Whether id has a session folder (sessionExists).
fn session_exists(id: &str) -> bool {
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        return false;
    }
    rush_host::root().join(id).is_dir()
}

/// The info of a session just started, once its host is up: at most 10s
/// (waitInfo).
fn wait_info(env: &Env, id: &str) -> Result<Info, String> {
    let end = Instant::now() + Duration::from_secs(10);
    let mut err = None;
    while Instant::now() < end {
        match read_info(id) {
            Ok(i) if (env.alive)(&i) => return Ok(i),
            Ok(_) => err = None,
            Err(e) => err = Some(e.to_string()),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(err.unwrap_or_else(|| format!("the host for {id} stopped right after starting")))
}

/// rush session start: a session started, or with --resume one stopped
/// brought back; one already running is shown as it is (sessionStart).
fn session_start(
    env: &Env,
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> (bool, Result<(), String>) {
    let mut fs = Flags::session(
        "start",
        &[
            "cwd",
            "agent",
            "profile",
            "session-id",
            "name",
            "prompt-file",
            "binary",
            "model",
            "effort",
            "permission-mode",
        ],
        &["resume", "json"],
        &["image", "env", "meta"],
    );
    if let Err(e) = fs.parse(args) {
        return (has_flag(args, "--json"), Err(e));
    }
    let as_json = fs.b("json");
    (as_json, start_flags(env, &fs, stdin, stdout, as_json))
}

fn start_flags(
    env: &Env,
    fs: &Flags,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    as_json: bool,
) -> Result<(), String> {
    if let Some(a) = fs.rest.first() {
        return Err(format!("unexpected argument {a:?}"));
    }
    let (resume, session_id) = (fs.b("resume"), fs.s("session-id"));
    if resume && session_id.is_empty() {
        return Err("--resume needs --session-id".into());
    }
    if !session_id.is_empty() && !is_uuid(&session_id) {
        return Err(format!("--session-id {session_id:?} is not a UUID"));
    }
    let env_vars = fs.m("env");
    for kv in &env_vars {
        if !kv.split_once('=').is_some_and(|(k, _)| !k.is_empty()) {
            return Err(format!("--env wants KEY=value, not {kv:?}"));
        }
    }
    let meta = fs.m("meta");
    let meta = if meta.is_empty() {
        None
    } else {
        Some(pairs(&meta, "meta")?)
    };
    let mut prompt = String::new();
    let pf = fs.s("prompt-file");
    if !pf.is_empty() {
        let b = if pf == "-" {
            let mut b = Vec::new();
            stdin.read_to_end(&mut b).map_err(|e| e.to_string())?;
            b
        } else {
            std::fs::read(&pf)
                .map_err(|e| format!("open {pf}: {}", rush_plugin::gopath::errno_text(&e)))?
        };
        prompt = String::from_utf8_lossy(&b)
            .trim_end_matches('\n')
            .to_string();
    }
    let images = fs
        .m("image")
        .into_iter()
        .map(|p| match std::path::absolute(&p) {
            Ok(a) => a.to_string_lossy().into_owned(),
            Err(_) => p,
        })
        .collect();
    let info = start_session(
        env,
        StartOpts {
            cwd: fs.s("cwd"),
            session_id,
            name: fs.s("name"),
            prompt,
            binary: fs.s("binary"),
            model: fs.s("model"),
            effort: fs.s("effort"),
            mode: fs.s("permission-mode"),
            kind: fs.s("agent"),
            profile: fs.s("profile"),
            resume,
            images,
            env: env_vars,
            meta,
        },
    )?;
    let v = env.view(info);
    if as_json {
        write_json(stdout, &v);
    } else {
        let _ = writeln!(stdout, "{} {} {}", v.info.id, v.info.state, v.info.name);
    }
    Ok(())
}

/// What a session is started with: rush session start's flags, or what a
/// remote client asked for (startOpts).
#[derive(Default)]
pub struct StartOpts {
    pub cwd: String,
    pub session_id: String,
    pub name: String,
    pub prompt: String,
    pub binary: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub kind: String,
    pub profile: String,
    pub resume: bool,
    pub images: Vec<String>,
    pub env: Vec<String>,
    pub meta: Option<BTreeMap<String, String>>,
}

/// Starts a session as a remote client asks (serve.go:58).
pub fn start_requested(reg: &Registry, r: &StartRequest) -> Result<Info, String> {
    start_session(
        &Env::process(reg),
        StartOpts {
            cwd: r.cwd.clone(),
            kind: r.agent.clone(),
            profile: r.profile.clone(),
            prompt: r.prompt.clone(),
            model: r.model.clone(),
            effort: r.effort.clone(),
            name: r.name.clone(),
            session_id: r.resume.clone(),
            resume: !r.resume.is_empty(),
            ..StartOpts::default()
        },
    )
}

/// a, or b when a is empty.
fn or(a: &str, b: &str) -> String {
    if a.is_empty() { b.into() } else { a.into() }
}

/// The first few words of a prompt, the name a session goes by until it
/// names itself (firstWordsOf).
fn first_words_of(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(6).collect();
    let n = words.join(" ");
    if n.chars().count() > 48 {
        return format!("{}…", n.chars().take(47).collect::<String>());
    }
    n
}

/// Starts a session, or with resume brings a stopped one back; one
/// already running is returned as it is (startSession).
pub fn start_session(env: &Env, o: StartOpts) -> Result<Info, String> {
    let reg = &env.reg;
    let st = rush_state::Store::load();
    let d = &st.config.dispatch;
    let mut cfg = Config::default();
    let mut kind = o.kind.clone();
    if !o.session_id.is_empty() {
        let id = rush_host::short_id(&o.session_id);
        if let Ok(info) = read_info(&id) {
            if rush_host::running(&info) || env_running(env, &info) {
                // Already running: nothing to start.
                return Ok(info);
            }
            if !o.resume {
                return Err(format!(
                    "session {id} exists and is stopped; pass --resume to bring it back"
                ));
            }
            // Back from its saved config, as the view's resume does.
            if let Ok(saved) = rush_host::read_config(&id) {
                cfg = saved;
            }
        }
        if cfg.id.is_empty() {
            cfg = Config {
                id,
                session_id: o.session_id.clone(),
                ..Config::default()
            };
        }
        cfg.resume = o.resume;
    }
    if !o.cwd.is_empty() {
        cfg.cwd = std::path::absolute(&o.cwd)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
    }
    if cfg.cwd.is_empty() {
        return Err("--cwd is required".into());
    }
    if !Path::new(&cfg.cwd).is_dir() {
        return Err(format!("--cwd {} is not a folder", cfg.cwd));
    }
    if !o.profile.is_empty() && st.config.profile_named(&o.profile).is_none() {
        return Err(format!("no profile named {:?}", o.profile));
    }
    if !cfg.resume || cfg.profile.is_empty() {
        let p = st.config.profile_for(&cfg.cwd, &o.profile);
        cfg.profile = p.name.clone();
        if kind.is_empty()
            && !cfg.resume
            && let Some(pick) = p.pick(&Default::default())
        {
            // The profile's first provider installed here: without the
            // view's readings of each account, it can't tell which are
            // nearly out.
            kind = pick.kind;
        }
    }
    let kind = or(&kind, &or(&cfg.kind, &st.config.default_agent()));
    if kind == rush_state::LOGINS_KIND && cfg.account.dir.is_empty() {
        let f = st.config.active_account();
        cfg.account = rush_core::adapter::Profile {
            kind: rush_state::LOGINS_KIND.into(),
            name: f.name,
            dir: f.config_dir,
        };
    }
    cfg.use_agent(reg, &kind)?;
    // Its own error says more, if it's still out.
    let _ = rush_host::kind::sign_in_if_out(reg, &kind, &cfg.account);
    // Each agent starts with what its own Settings page says, unless told.
    let start = d.resolved_for(&kind);
    cfg.prompt = o.prompt.clone();
    cfg.images = o.images.clone();
    cfg.lean = d.quick_start();
    if cfg.limit_mode.is_empty() && kind == rush_state::LOGINS_KIND {
        // Dispatch's own are that agent's.
        cfg.limit_mode = d.on_limit.clone();
    }
    cfg.model = or(&o.model, &or(&cfg.model, &start.model));
    cfg.effort = or(&o.effort, &or(&cfg.effort, &start.effort));
    cfg.permission_mode = or(&o.mode, &or(&cfg.permission_mode, &start.mode));
    cfg.binary = or(&o.binary, &cfg.binary);
    if !o.env.is_empty() {
        cfg.env = o.env.clone();
    }
    if let Some(m) = &o.meta {
        cfg.meta.extend(m.clone());
    }
    // Started from a rush session's shell, it's that session's subagent.
    if let Ok(by) = std::env::var("RUSH_SESSION")
        && !by.is_empty()
        && !cfg.resume
        && cfg.meta.get("spawnedBy").is_none_or(String::is_empty)
    {
        cfg.meta.insert("spawnedBy".into(), by);
    }
    cfg.name = or(&o.name, &cfg.name);
    if cfg.name.is_empty() {
        cfg.name = first_words_of(&o.prompt);
    }
    if cfg.name.is_empty()
        && let Some(first) = o.images.first()
    {
        cfg.name = format!("about {}", base(first));
    }
    if cfg.name.is_empty() {
        cfg.name = format!("fresh session in {}", base(&cfg.cwd));
    }
    // The spawned line, its host's cause.
    cfg.cause = rush_events::id();
    let started = (env.spawn)(reg, cfg.clone()).inspect_err(|e| {
        rush_host::events::log(
            "start-failed",
            json!({ "agent": kind, "cwd": cfg.cwd, "resume": cfg.resume, "err": e }),
        );
    })?;
    log_event(
        &started.id,
        "spawned",
        json!({ "ev": cfg.cause, "by": "rush session start",
            "spawnedBy": cfg.meta.get("spawnedBy").cloned().unwrap_or_default(),
            "agent": kind, "cwd": cfg.cwd, "resume": cfg.resume, "mode": cfg.permission_mode,
            "model": cfg.model, "prompt": crate::spawn::clip(&o.prompt) }),
    );
    wait_info(env, &started.id)
}

/// Whether info's host runs, as env sees it: a test's fake host runs in
/// this very process, which host.Running takes for a host gone.
fn env_running(env: &Env, info: &Info) -> bool {
    info.host_pid == std::process::id() as i64 && (env.alive)(info)
}

/// The last part of a path (filepath.Base).
fn base(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Sends stdin to session id, resuming it first if its host isn't
/// running (sessionSend).
fn session_send(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), String> {
    let mut fs = Flags::session("send", &[], &["now"], &["image"]);
    let id = fs.id_and_flags(args)?;
    let now = fs.b("now");
    let mut images = fs.m("image");
    if !session_exists(&id) {
        return Err(format!("session {id} not found"));
    }
    let mut b = Vec::new();
    stdin.read_to_end(&mut b).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&b)
        .trim_end_matches([' ', '\t', '\r', '\n'])
        .to_string();
    if text.is_empty() && images.is_empty() {
        return Err("nothing to send: write the message on stdin".into());
    }
    for p in &mut images {
        if let Ok(abs) = std::path::absolute(&*p) {
            *p = abs.to_string_lossy().into_owned();
        }
    }
    let mut exchange = crate::spawn::outgoing_exchange(&id, "message", &text, &images);
    // A host falling asleep as this comes turns it away: sent again, it
    // wakes.
    let mut r = send_once(&id, &text, &images, now, &mut exchange, stdout);
    for _ in 0..2 {
        match &r {
            Err(e) if resting(e) => {
                std::thread::sleep(Duration::from_millis(100));
                r = send_once(&id, &text, &images, now, &mut exchange, stdout);
            }
            _ => break,
        }
    }
    r.map_err(|e| e.to_string())
}

/// Interrupts a session's turn, or stops the session (sessionControl).
fn session_control(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
    stop: bool,
) -> Result<(), String> {
    let name = if stop { "stop" } else { "interrupt" };
    let id = Flags::session(name, &[], &[], &[]).id_and_flags(args)?;
    if !session_exists(&id) {
        return Err(format!("session {id} not found"));
    }
    let info = match read_info(&id) {
        Ok(i) if (env.alive)(&i) => i,
        _ => {
            if stop {
                // Asleep, it still has what it set going: a follower
                // would wake it.
                rush_host::client::stop_all(&id).map_err(|e| e.to_string())?;
                let _ = writeln!(stdout, "{id} is already stopped");
                return Ok(());
            }
            return Err(format!("session {id} is not running"));
        }
    };
    let c = Client::dial(&id).map_err(|e| e.to_string())?;
    if !stop {
        c.interrupt().map_err(|e| e.to_string())?;
        let _ = writeln!(stdout, "interrupted {id}");
        return Ok(());
    }
    let by = std::env::var("RUSH_SESSION").unwrap_or_default();
    log_event(
        &id,
        "stop-asked",
        json!({ "by": or(&by, "rush session stop") }),
    );
    rush_host::client::stop_all(&id).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while (env.alive)(&info) {
        if Instant::now() > deadline {
            return Err(format!("the host for {id} didn't stop"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(c);
    let _ = writeln!(stdout, "stopped {id}");
    Ok(())
}

/// The plugin broker's view of hosts, with each agent's own words for a
/// tool call: what watch reads a session through.
struct WatchHosts {
    files: rush_plugind::host::FileHosts,
    reg: Registry,
}

impl rush_plugind::host::Hosts for WatchHosts {
    fn lister(&self) -> rush_plugind::host::Lister {
        self.files.lister()
    }
    fn read_info(&self, id: &str) -> Result<rush_plugind::host::Info, String> {
        self.files.read_info(id)
    }
    fn dial(&self, id: &str) -> Result<rush_plugind::host::Dialed, String> {
        rush_plugind::host::dial_sock(&rush_host::sock_path(id))
    }
    fn spawn(&self, cfg: &rush_plugind::host::SpawnConfig) -> Result<String, String> {
        self.files.spawn(cfg)
    }
    fn log_event(&self, id: &str, kind: &str, fields: rush_events::Fields) {
        self.files.log_event(id, kind, fields)
    }
    fn views(&self, id: &str) -> Vec<rush_plugind::host::View> {
        self.files.views(id)
    }
    fn save_view(&self, id: &str, v: rush_plugind::host::View) -> Result<(), String> {
        self.files.save_view(id, v)
    }
    fn drop_view(&self, id: &str, name: &str) -> Result<(), String> {
        self.files.drop_view(id, name)
    }
    fn doing(&self, kind: &str, c: &rush_core::tool::Call) -> String {
        self.reg.doing(kind, c)
    }
}

/// Prints what a session says and does as it happens, as plugins see it
/// with sessions.subscribe: with --json a line of JSON an event,
/// {"event": type, "session": id, ...its fields} (sessionWatch).
fn session_watch(env: &Env, args: &[String], stdout: &mut dyn Write) -> (bool, Result<(), String>) {
    let mut fs = Flags::session("watch", &[], &["json", "until-idle"], &[]);
    let id = match fs.id_and_flags(args) {
        Ok(id) => id,
        Err(e) => return (fs.b("json") || has_flag(args, "--json"), Err(e)),
    };
    let (as_json, until_idle) = (fs.b("json"), fs.b("until-idle"));
    (as_json, watch(env, &id, as_json, until_idle, stdout))
}

fn watch(
    env: &Env,
    id: &str,
    as_json: bool,
    until_idle: bool,
    stdout: &mut dyn Write,
) -> Result<(), String> {
    if !session_exists(id) {
        return Err(format!("session {id} not found"));
    }
    match read_info(id) {
        Ok(i) if (env.alive)(&i) => {}
        _ => return Err(format!("session {id} is not running")),
    }
    let d = rush_plugind::host::dial_sock(&rush_host::sock_path(id))?;
    follow_watch(env, d, id, as_json, until_idle, stdout)
}

/// Prints what the host at the end of d says until it hangs up, or with
/// until_idle the turn under way ends.
fn follow_watch(
    env: &Env,
    d: rush_plugind::host::Dialed,
    id: &str,
    as_json: bool,
    until_idle: bool,
    stdout: &mut dyn Write,
) -> Result<(), String> {
    let hosts = WatchHosts {
        files: Default::default(),
        reg: env.reg.clone(),
    };
    let client = d.client.clone();
    let mut w = Watch {
        out: stdout,
        id,
        as_json,
        until_idle,
        done: false,
        busy: false,
        open: false,
        failed: None,
        streamed: String::new(),
    };
    rush_plugind::follow(d.lines.iter(), true, &hosts, |ev| {
        if w.event(ev) {
            // And follow returns once it has read what's buffered.
            client.close();
        }
    });
    client.close();
    if w.done {
        return w.failed.map_or(Ok(()), Err);
    }
    if as_json {
        write_json(w.out, &json!({ "event": "closed", "session": id }));
    } else {
        w.line("· closed");
    }
    if until_idle {
        return Err(format!("session {id} closed before its turn ended"));
    }
    Ok(())
}

/// What watch has printed so far.
struct Watch<'a> {
    out: &'a mut dyn Write,
    id: &'a str,
    as_json: bool,
    until_idle: bool,
    done: bool,
    busy: bool,
    /// A line of streamed words not yet ended.
    open: bool,
    failed: Option<String>,
    /// Words printed as written, not yet come whole.
    streamed: String,
}

impl Watch<'_> {
    fn line(&mut self, s: &str) {
        if self.open {
            let _ = writeln!(self.out);
            self.open = false;
        }
        let _ = writeln!(self.out, "{s}");
    }

    /// Prints one event; whether the watch is over.
    fn event(&mut self, mut ev: Map<String, Value>) -> bool {
        if self.done {
            return false;
        }
        let str_of = |ev: &Map<String, Value>, k: &str| {
            ev.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let typ = str_of(&ev, "type");
        let text = str_of(&ev, "text");
        let state = str_of(&ev, "state");
        let is_err = ev.get("isError").and_then(Value::as_bool).unwrap_or(false);
        if self.as_json {
            ev.remove("type");
            ev.insert("event".into(), json!(typ));
            ev.insert("session".into(), json!(self.id));
            write_json(self.out, &ev);
        } else {
            match typ.as_str() {
                "info" => {
                    let needs = str_of(&ev, "needs");
                    let detail = str_of(&ev, "detail");
                    let why = or(&needs, &detail);
                    let why = if why.is_empty() {
                        why
                    } else {
                        format!(": {why}")
                    };
                    self.line(&format!("· {state}{why}"));
                }
                "sent" => self.line(&format!("> {text}")),
                "delta" => {
                    let _ = write!(self.out, "{text}");
                    self.streamed.push_str(&text);
                    self.open = !text.ends_with('\n');
                }
                "text" => {
                    // Streamed already, the words aren't printed again.
                    if let Some(rest) = self.streamed.strip_prefix(text.as_str()) {
                        self.streamed = rest.to_string();
                        if self.streamed.is_empty() && self.open {
                            let _ = writeln!(self.out);
                            self.open = false;
                        }
                    } else {
                        self.streamed.clear();
                        self.line(&text);
                    }
                }
                "tool" => {
                    let doing = or(&str_of(&ev, "doing"), &str_of(&ev, "name"));
                    self.line(&format!("→ {doing}"));
                }
                "result" => {
                    if is_err {
                        self.line(&format!("· failed: {text}"));
                    } else {
                        let cost = ev.get("costUsd").and_then(Value::as_f64).unwrap_or(0.0);
                        self.line(&format!("· done, ${cost:.2}"));
                    }
                }
                _ => {}
            }
        }
        let idle = |s: &str| s == "idle" || s == "stopped";
        if self.until_idle {
            if typ == "result" && is_err {
                self.done = true;
                self.failed = Some(format!("the turn failed: {text}"));
            } else if typ == "result" || typ == "info" && self.busy && idle(&state) {
                // Idle when it started is waiting for a turn, not the end
                // of one.
                self.done = true;
            }
        }
        if typ == "info" {
            self.busy = !idle(&state);
        }
        self.done
    }
}

/// rush session info (sessionInfo).
fn session_info(env: &Env, args: &[String], stdout: &mut dyn Write) -> (bool, Result<(), String>) {
    let mut fs = Flags::session("info", &[], &["json"], &[]);
    let id = match fs.id_and_flags(args) {
        Ok(id) => id,
        Err(e) => return (fs.b("json") || has_flag(args, "--json"), Err(e)),
    };
    let as_json = fs.b("json");
    if !session_exists(&id) {
        return (as_json, Err("not found".into()));
    }
    let Ok(info) = read_info(&id) else {
        return (as_json, Err("not found".into()));
    };
    let v = env.view(info);
    if as_json {
        write_json(stdout, &v);
        return (true, Ok(()));
    }
    let i = &v.info;
    let _ = write!(
        stdout,
        "{} {} {}\n  cwd {}\n  session {}\n",
        i.id, i.state, i.name, i.cwd, i.session_id
    );
    if i.generation > 0 {
        let _ = writeln!(stdout, "  host started {} times", i.generation);
    }
    (false, Ok(()))
}

/// rush session list (sessionList).
fn session_list(env: &Env, args: &[String], stdout: &mut dyn Write) -> (bool, Result<(), String>) {
    let mut fs = Flags::session("list", &[], &["json"], &["meta"]);
    if let Err(e) = fs.parse(args) {
        return (has_flag(args, "--json"), Err(e));
    }
    let as_json = fs.b("json");
    let want = match pairs(&fs.m("meta"), "meta") {
        Ok(w) => w,
        Err(e) => return (as_json, Err(e)),
    };
    let out: Vec<View> = rush_host::list()
        .into_iter()
        .filter(|i| {
            want.iter()
                .all(|(k, v)| i.meta.get(k).is_some_and(|got| got == v))
        })
        .map(|i| env.view(i))
        .collect();
    if as_json {
        write_json(stdout, &out);
        return (true, Ok(()));
    }
    for v in &out {
        let _ = writeln!(stdout, "{}  {:<8} {}", v.info.id, v.info.state, v.info.name);
    }
    (false, Ok(()))
}

/// One try at sending: the host woken if it rests, the message given to
/// it, and its answer awaited (sessionSend's send).
fn send_once(
    id: &str,
    text: &str,
    images: &[String],
    now: bool,
    exchange: &mut Option<rush_core::event::Exchange>,
    stdout: &mut dyn Write,
) -> Result<(), ClientError> {
    let was = read_info(id);
    let resumed = was
        .as_ref()
        .map_or(true, |i| i.sleeping || !alive(i.host_pid));
    // A wake is the cause of the host's start and of the message's
    // delivery; without one they come of what ran this.
    let woke = rush_events::id();
    if let Err(e) = ensure(id, &woke) {
        if resumed {
            log_event(
                id,
                "wake-failed",
                json!({"ev": woke, "why": "a message was sent to it", "err": e.to_string()}),
            );
        }
        return Err(e);
    }
    if resumed {
        log_event(
            id,
            "woken",
            json!({"ev": woke, "why": "a message was sent to it",
                "by": std::env::var("RUSH_SESSION").unwrap_or_default()}),
        );
    }
    let mut c = Client::dial(id)?;
    if resumed {
        c.cause = woke;
    }
    // The host replays the conversation first and ends the replay with
    // its info: what comes after that answers this send.
    let mut peer_proto = 0;
    await_line(&c, Duration::from_secs(10), |l| match l {
        Line::Info(i) => {
            peer_proto = i.proto;
            true
        }
        _ => false,
    })?;
    if exchange.is_some() && peer_proto < 8 {
        let _ = writeln!(
            stdout,
            "warning: receiver runs an older host; agent attribution needs a host restart"
        );
        *exchange = None;
    }
    // Into the turn under way, not after it: another agent's message
    // shouldn't wait on the queue. Idle, or where the agent can't take
    // one mid-turn, the host sends it as any message.
    match exchange {
        Some(e) => c.send_exchange(e.clone(), now)?,
        None if now && !images.is_empty() => c.send_images(text, images, now)?,
        None if now => c.send_now(text)?,
        None => c.send_guide(text, images)?,
    }
    let mut failed = None;
    let want = exchange.as_ref().map(|e| e.id.clone());
    await_line(&c, Duration::from_secs(5), |l| match l {
        Line::Error(s) => {
            failed = Some(s.clone());
            true
        }
        Line::Sent(_) => true,
        Line::Info(i) => match &want {
            Some(w) => i
                .queue_exchanges
                .iter()
                .any(|q| q.as_ref().is_some_and(|q| q.id == *w)),
            None => i.queue.last().is_some_and(|q| q == text),
        },
        _ => false,
    })?;
    c.close();
    if let Some(f) = failed {
        return Err(ClientError::Other(f));
    }
    if let Some(e) = exchange
        && let Err(err) = crate::spawn::mirror_outgoing(e)
    {
        let _ = writeln!(
            stdout,
            "warning: delivered to {id}, but sender transcript update failed: {err}"
        );
    }
    if resumed {
        let _ = writeln!(stdout, "resumed {id} with the message");
    } else {
        let _ = writeln!(stdout, "sent to {id}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_host::fake;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// A rush folder for this thread under /tmp, where a socket's path
    /// fits.
    struct Home(tempfile::TempDir);

    impl Home {
        fn new() -> Home {
            let d = tempfile::Builder::new()
                .prefix("rs-")
                .tempdir_in("/tmp")
                .unwrap();
            let p = std::fs::canonicalize(d.path()).unwrap();
            rush_state::set_thread_dir(Some(p.clone()));
            rush_state::set_thread_cache_dir(Some(p.join("cache")));
            Home(d)
        }
        fn path(&self) -> String {
            std::fs::canonicalize(self.0.path())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            rush_state::set_thread_dir(None);
            rush_state::set_thread_cache_dir(None);
        }
    }

    /// The fake agent with an account of its own in this thread's rush
    /// folder, as a session needs one to start.
    struct Profiled(fake::Fake);

    impl rush_core::adapter::Adapter for Profiled {
        fn kind(&self) -> rush_core::adapter::Kind {
            self.0.kind()
        }
        fn name(&self) -> String {
            self.0.name()
        }
        fn features(&self) -> rush_core::adapter::Features {
            self.0.features()
        }
        fn level(&self) -> rush_core::adapter::Level {
            self.0.level()
        }
        fn profiles(&self) -> Vec<rush_core::adapter::Profile> {
            let dir = rush_state::dir().join("acct");
            let _ = std::fs::create_dir_all(&dir);
            vec![rush_core::adapter::Profile {
                kind: "fake".into(),
                name: "default".into(),
                dir: dir.to_string_lossy().into_owned(),
            }]
        }
        rush_core::has!(driver: Driver);
    }

    impl rush_core::adapter::Driver for Profiled {
        fn start(
            &self,
            o: rush_core::adapter::StartOptions,
            sink: std::sync::Arc<dyn rush_core::adapter::Sink>,
        ) -> rush_core::adapter::Result<Box<dyn rush_core::adapter::Conn>> {
            rush_core::adapter::Driver::start(&self.0, o, sink)
        }
    }

    /// Sessions hosted in this process by rush-host's fake host: never
    /// `rush host run`, never a real agent. One runs while its socket
    /// answers.
    fn fake_env() -> Env {
        let reg = Registry::new();
        reg.register(Profiled(fake::Fake::default()));
        Env {
            reg,
            spawn: |reg, cfg| fake::spawn(reg.clone(), cfg).map_err(|e| e.to_string()),
            alive: |i| Client::dial(&i.id).is_ok(),
        }
    }

    fn run(env: &Env, args: &[&str], stdin: &str) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = session_cmd(env, &s(args), &mut stdin.as_bytes(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// Starts a fake session in home; what start printed.
    fn start(env: &Env, home: &Home, extra: &[&str]) -> String {
        let cwd = home.path();
        let mut args = vec!["start", "--agent", "fake", "--cwd", cwd.as_str()];
        args.extend_from_slice(extra);
        let (code, out, err) = run(env, &args, "");
        assert_eq!((code, err.as_str()), (0, ""), "{out}");
        out
    }

    fn id_of(started: &str) -> String {
        started.split(' ').next().unwrap().to_string()
    }

    #[test]
    fn uuids_and_pairs() {
        assert!(is_uuid("0f8fad5b-d9cb-469f-a165-70867728950E"));
        for bad in [
            "",
            "0f8fad5b-d9cb-469f-a165",
            "0f8fad5bd9cb469fa16570867728950e",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
        ] {
            assert!(!is_uuid(bad), "{bad}");
        }
        assert_eq!(
            pairs(&s(&["a=1", "b=x=y"]), "meta").unwrap(),
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "x=y".into())])
        );
        assert_eq!(
            pairs(&s(&["=1"]), "meta").unwrap_err(),
            "--meta wants key=value, not \"=1\""
        );
    }

    #[test]
    fn first_words() {
        assert_eq!(
            first_words_of("fix the flaky test in the parser please"),
            "fix the flaky test in the"
        );
        assert_eq!(first_words_of(""), "");
    }

    // sessionStart's checks, before anything starts: each says why on
    // stderr, or as {"error"} on stdout with --json, and ends 1.
    #[test]
    fn start_refuses_what_go_refuses() {
        let home = Home::new();
        let env = fake_env();
        let dir = home.path();
        let file = format!("{dir}/f");
        std::fs::write(&file, "").unwrap();
        let not_folder = format!("--cwd {file} is not a folder");
        let unknown = format!("unknown session command \"bogus\"\n\n{USAGE}");
        let cases: &[(&[&str], &str)] = &[
            (&["start", "--resume"], "--resume needs --session-id"),
            (
                &["start", "--session-id", "nope"],
                "--session-id \"nope\" is not a UUID",
            ),
            (
                &["start", "--env", "=x"],
                "--env wants KEY=value, not \"=x\"",
            ),
            (
                &["start", "--meta", "k"],
                "--meta wants key=value, not \"k\"",
            ),
            (&["start", "extra"], "unexpected argument \"extra\""),
            (&["start", "--nope"], "flag provided but not defined: -nope"),
            (&["start"], "--cwd is required"),
            (
                &[
                    "start",
                    "--agent",
                    "fake",
                    "--profile",
                    "nobody",
                    "--cwd",
                    &dir,
                ],
                "no profile named \"nobody\"",
            ),
            (&["start", "--cwd", &file], &not_folder),
            (
                &["start", "--prompt-file", "/nonexistent/p", "--cwd", &dir],
                "open /nonexistent/p: no such file or directory",
            ),
            (&["bogus"], &unknown),
        ];
        for (args, want) in cases {
            let (code, out, err) = run(&env, args, "");
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert_eq!(err, format!("rush: {want}\n"), "{args:?}");
        }
        let (code, out, err) = run(&env, &["start", "--json", "--resume"], "");
        assert_eq!((code, err.as_str()), (1, ""));
        assert_eq!(out, "{\"error\":\"--resume needs --session-id\"}\n");
        // A flag error still answers in JSON when --json was among them.
        let (_, out, _) = run(&env, &["start", "--json", "--nope"], "");
        assert_eq!(
            out,
            "{\"error\":\"flag provided but not defined: -nope\"}\n"
        );
    }

    // start, info, list, interrupt, watch and stop against sessions the
    // fake host runs in this process.
    #[test]
    fn start_info_list_interrupt_watch_stop() {
        let home = Home::new();
        let env = fake_env();
        let id = id_of(&start(
            &env,
            &home,
            &["--name", "first", "--meta", "team=a"],
        ));
        let info = read_info(&id).unwrap();
        let (code, out, _) = run(&env, &["info", &id], "");
        assert_eq!(code, 0);
        assert_eq!(
            out,
            format!(
                "{id} {} first\n  cwd {}\n  session {}\n  host started 1 times\n",
                info.state,
                home.path(),
                info.session_id
            )
        );
        let (code, out, _) = run(&env, &["info", "--json", &id], "");
        assert_eq!(code, 0);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (v["id"].as_str(), v["alive"].as_bool()),
            (Some(id.as_str()), Some(true))
        );
        assert!(out.ends_with("\"alive\":true}\n"), "{out}");
        let (code, out, err) = run(&env, &["info", "nosuch"], "");
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (1, "", "rush: not found\n")
        );
        let (_, out, _) = run(&env, &["info", "nosuch", "--json"], "");
        assert_eq!(out, "{\"error\":\"not found\"}\n");

        // A prompt names a session by its first words.
        let pf = format!("{}/prompt", home.path());
        std::fs::write(&pf, "look at the failing build on main please now\n").unwrap();
        let other: Value =
            serde_json::from_str(&start(&env, &home, &["--prompt-file", &pf, "--json"])).unwrap();
        assert_eq!(other["name"], "look at the failing build on");
        assert_eq!(other["alive"], true);
        let other = other["id"].as_str().unwrap().to_string();
        let (_, out, _) = run(&env, &["list", "--meta", "team=a"], "");
        assert_eq!(out, format!("{id}  {:<8} first\n", info.state));
        let (_, out, _) = run(&env, &["list", "--json"], "");
        let all: Vec<Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(all.len(), 2);
        let (_, out, _) = run(&env, &["list", "--json", "--meta", "team=b"], "");
        assert_eq!(out, "[]\n");
        let (code, _, err) = run(&env, &["list", "--meta", "x"], "");
        assert_eq!(
            (code, err.as_str()),
            (1, "rush: --meta wants key=value, not \"x\"\n")
        );

        // Started again by its session id, a running one is as it is.
        let sid = info.session_id.clone();
        let cwd = home.path();
        let (code, out, _) = run(&env, &["start", "--session-id", &sid, "--cwd", &cwd], "");
        assert_eq!((code, id_of(&out)), (0, id.clone()));

        let (code, out, _) = run(&env, &["interrupt", &id], "");
        assert_eq!((code, out), (0, format!("interrupted {id}\n")));

        // watch follows until the host goes; --until-idle says it never
        // saw the turn end.
        let d = rush_plugind::host::dial_sock(&rush_host::sock_path(&id)).unwrap();
        let stopper = {
            let (id, dir) = (id.clone(), home.path());
            std::thread::spawn(move || {
                let dir = std::path::PathBuf::from(dir);
                rush_state::set_thread_dir(Some(dir.clone()));
                rush_state::set_thread_cache_dir(Some(dir.join("cache")));
                run(&fake_env(), &["stop", &id], "")
            })
        };
        let mut out = Vec::new();
        let r = follow_watch(&env, d, &id, false, true, &mut out);
        assert_eq!(
            r.unwrap_err(),
            format!("session {id} closed before its turn ended")
        );
        let out = String::from_utf8(out).unwrap();
        assert!(out.ends_with("· closed\n"), "{out}");
        let (code, out, _) = stopper.join().unwrap();
        assert_eq!((code, out), (0, format!("stopped {id}\n")));

        let (code, out, _) = run(&env, &["stop", &id], "");
        assert_eq!((code, out), (0, format!("{id} is already stopped\n")));
        for sub in ["interrupt", "watch"] {
            let (code, _, err) = run(&env, &[sub, &id], "");
            assert_eq!(
                (code, err),
                (1, format!("rush: session {id} is not running\n"))
            );
        }
        let (_, out, _) = run(&env, &["watch", &id, "--json"], "");
        assert_eq!(
            out,
            format!("{{\"error\":\"session {id} is not running\"}}\n")
        );
        let (code, _, err) = run(&env, &["stop"], "");
        assert_eq!(
            (code, err.as_str()),
            (1, "rush: usage: rush session stop <id>\n")
        );
        let (code, _, err) = run(&env, &["stop", "nosuch"], "");
        assert_eq!(
            (code, err.as_str()),
            (1, "rush: session nosuch not found\n")
        );

        // Stopped, it comes back only with --resume.
        let (code, _, err) = run(&env, &["start", "--session-id", &sid, "--cwd", &cwd], "");
        assert_eq!(code, 1);
        assert_eq!(
            err,
            format!("rush: session {id} exists and is stopped; pass --resume to bring it back\n")
        );
        let _ = Client::dial(&other).map(|c| c.stop());
    }

    fn watcher(out: &mut Vec<u8>, as_json: bool, busy: bool) -> Watch<'_> {
        Watch {
            out,
            id: "abc",
            as_json,
            until_idle: true,
            done: false,
            busy,
            open: false,
            failed: None,
            streamed: String::new(),
        }
    }

    fn ev(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    // What watch prints of each event, as Go's sessionWatch: streamed
    // words not printed twice, a turn's end with its cost, and with
    // --until-idle the end of the turn under way.
    #[test]
    fn watch_prints_as_go() {
        let mut out = Vec::new();
        let mut w = watcher(&mut out, false, false);
        let evs = [
            json!({"type": "info", "state": "idle", "detail": "", "needs": ""}),
            json!({"type": "sent", "text": "go"}),
            json!({"type": "info", "state": "working", "detail": "thinking", "needs": ""}),
            json!({"type": "delta", "text": "hel"}),
            json!({"type": "delta", "text": "lo"}),
            json!({"type": "text", "text": "hello"}),
            json!({"type": "tool", "name": "Bash", "doing": "running ls"}),
            json!({"type": "tool", "name": "Read", "doing": ""}),
            json!({"type": "text", "text": "fresh words"}),
            json!({"type": "info", "state": "asking", "detail": "x", "needs": "an answer"}),
        ];
        for e in evs {
            assert!(!w.event(ev(e)));
        }
        assert!(w.event(ev(
            json!({"type": "result", "text": "", "isError": false, "costUsd": 0.125})
        )));
        assert!(!w.event(ev(json!({"type": "sent", "text": "after"}))));
        assert_eq!(w.failed, None);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "· idle\n> go\n· working: thinking\nhello\n→ running ls\n→ Read\nfresh words\n\
             · asking: an answer\n· done, $0.12\n"
        );

        let mut out = Vec::new();
        let mut w = watcher(&mut out, true, true);
        assert!(w.event(ev(
            json!({"type": "result", "text": "boom", "isError": true, "costUsd": 0})
        )));
        assert_eq!(w.failed.as_deref(), Some("the turn failed: boom"));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"costUsd\":0,\"event\":\"result\",\"isError\":true,\"session\":\"abc\",\"text\":\"boom\"}\n"
        );
        // Busy, then idle, is a turn's end too.
        let mut out = Vec::new();
        let mut w = watcher(&mut out, false, false);
        assert!(!w.event(ev(json!({"type": "info", "state": "working"}))));
        assert!(w.event(ev(json!({"type": "info", "state": "stopped"}))));
    }

    // sessionSend against a host running in this process: what's on
    // stdin reaches the session, and what can't be sent says why.
    #[test]
    fn sends_stdin_to_a_session() {
        let home = Home::new();
        let env = fake_env();
        let id = id_of(&start(&env, &home, &[]));
        let (code, out, err) = run(&env, &["send", &id], "your push failed\n\n");
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out, format!("sent to {id}\n"));
        let (code, _, err) = run(&env, &["send", &id], " \n");
        assert_eq!(code, 1);
        assert_eq!(err, "rush: nothing to send: write the message on stdin\n");
        let (code, _, err) = run(&env, &["send", "nosuch"], "hi");
        assert_eq!(code, 1);
        assert_eq!(err, "rush: session nosuch not found\n");
        let (code, _, err) = run(&env, &["send", &id, "--image"], "hi");
        assert_eq!(
            (code, err.as_str()),
            (1, "rush: flag needs an argument: -image\n")
        );
        let (code, out, _) = run(&env, &["help"], "");
        assert_eq!(code, 0);
        assert!(out.starts_with("rush session: run rush-mode sessions without the view"));
        let _ = Client::dial(&id).map(|c| c.stop());
    }
}
