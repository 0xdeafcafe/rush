//! `rush serve`, `rush remote …` and `rush session --on NAME …`: this
//! machine's sessions to the web app and to rush elsewhere, the other
//! machines this one shows, and the session commands run on one of them.
//! After Go's cmd/rush/serve.go (remoteUsage :23, serveCmd :38,
//! remoteCmd :74, remoteSessionCmd :129, remoteWatch :287) and
//! session.go's startSession (:269) as serve starts sessions with it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_core::adapter::Registry;
use rush_host::{Config, Info};
use rush_remote::others::Other;
use rush_remote::{Ctx, Peer, Server, Session, StartRequest, load_config, save_config, token};
use serde_json::{Value, json};

pub const REMOTE_USAGE: &str = "rush serve [--listen ADDR]
        serve this machine's sessions to the web app and to rush elsewhere;
        127.0.0.1:7878 unless remote.json or --listen says. See docs/remote.md.
  rush remote token                    this machine's serve token
  rush remote add NAME URL TOKEN       show another machine's serve here
  rush remote attach NAME              keep NAME's sessions in this machine's rush (list, view, send,
                                       answer, stop) until it ends; start them with rush session --on NAME start
  rush remote remove NAME
  rush remote list
  rush session --on NAME list|info|start|send|interrupt|stop|watch …
        the session commands, run on machine NAME
";

/// Go's flag package, for these commands: -name or --name, its value
/// after = or as the next argument, a bool alone; flags end at the first
/// argument that isn't one.
struct Flags {
    name: String,
    strings: Vec<(&'static str, String)>,
    bools: Vec<(&'static str, bool)>,
    rest: Vec<String>,
}

impl Flags {
    fn new(name: &str, strings: &[&'static str], bools: &[&'static str]) -> Flags {
        Flags {
            name: name.into(),
            strings: strings.iter().map(|s| (*s, String::new())).collect(),
            bools: bools.iter().map(|b| (*b, false)).collect(),
            rest: Vec::new(),
        }
    }

    fn parse(&mut self, args: &[String]) -> Result<(), String> {
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--" {
                self.rest.extend(args[i + 1..].iter().cloned());
                return Ok(());
            }
            let Some(f) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
                self.rest.extend(args[i..].iter().cloned());
                return Ok(());
            };
            if f.is_empty() || f.starts_with('-') || f.starts_with('=') {
                return Err(format!("bad flag syntax: {a}"));
            }
            let (k, v) = match f.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (f, None),
            };
            if let Some(b) = self.bools.iter_mut().find(|(n, _)| *n == k) {
                b.1 = match v.as_deref() {
                    None | Some("true") | Some("1") | Some("t") | Some("T") | Some("TRUE")
                    | Some("True") => true,
                    Some("false") | Some("0") | Some("f") | Some("F") | Some("FALSE")
                    | Some("False") => false,
                    Some(x) => {
                        return Err(format!("invalid boolean value {x:?} for -{k}: parse error"));
                    }
                };
            } else if let Some(s) = self.strings.iter_mut().find(|(n, _)| *n == k) {
                s.1 = match v {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or_else(|| format!("flag needs an argument: -{k}"))?
                    }
                };
            } else if k == "help" || k == "h" {
                eprint!("{}", self.usage());
                return Err("flag: help requested".into());
            } else {
                return Err(format!("flag provided but not defined: -{k}"));
            }
            i += 1;
        }
        Ok(())
    }

    /// What -help prints, as Go's flag.PrintDefaults words it: each flag
    /// in name order, with no usage text of its own.
    fn usage(&self) -> String {
        let mut names: Vec<(&str, bool)> = self
            .strings
            .iter()
            .map(|(n, _)| (*n, true))
            .chain(self.bools.iter().map(|(n, _)| (*n, false)))
            .collect();
        names.sort();
        let mut out = format!("Usage of {}:\n", self.name);
        for (n, string) in names {
            let kind = if string { " string" } else { "" };
            out.push_str(&format!("  -{n}{kind}\n    \t\n"));
        }
        out
    }

    fn s(&self, k: &str) -> String {
        self.strings
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    fn b(&self, k: &str) -> bool {
        self.bools.iter().any(|(n, v)| *n == k && *v)
    }

    /// The session id, before or after the flags (idAndFlags).
    fn id_and_flags(&mut self, args: &[String]) -> Result<String, String> {
        let (mut id, rest) = match args.first() {
            Some(a) if !a.starts_with('-') => (a.clone(), &args[1..]),
            _ => (String::new(), args),
        };
        self.parse(rest)?;
        if id.is_empty()
            && let Some(a) = self.rest.first()
        {
            id = a.clone();
        }
        if id.is_empty() {
            return Err(format!("usage: rush session {} <id>", self.name));
        }
        Ok(id)
    }
}

/// Runs rush serve until it's interrupted.
pub fn serve_cmd(reg: &Registry, args: &[String]) -> Result<(), String> {
    let mut fs = Flags::new("serve", &["listen"], &[]);
    fs.parse(args)?;
    let mut cfg = load_config()?;
    if !fs.s("listen").is_empty() {
        cfg.listen = fs.s("listen");
    }
    let tok = token()?;
    let push = rush_remote::push::Push::load(&cfg.origin)?;
    let mut srv = Server::new(cfg.clone(), tok, reg.clone());
    srv.push = Some(Arc::new(push));
    let r = reg.clone();
    srv.start = Some(Arc::new(move |q: &StartRequest| start_session(&r, q)));
    let ln = std::net::TcpListener::bind(&cfg.listen).map_err(|e| {
        format!(
            "listen tcp {}: bind: {}",
            cfg.listen,
            rush_plugin::gopath::errno_text(&e)
        )
    })?;
    let ctx = Ctx::new();
    rush_watch::stop::on_signals();
    {
        let c = ctx.clone();
        std::thread::spawn(move || {
            while rush_watch::stop::sleep(Duration::from_secs(86_400)) {}
            c.cancel();
        });
    }
    let addr = ln.local_addr().map_err(|e| e.to_string())?;
    eprintln!(
        "rush serve: {} on http://{addr} (rush remote token shows the token)",
        cfg.name
    );
    let log: rush_remote::expose::Log = Arc::new(Mutex::new(std::io::stderr()));
    Arc::new(srv).run(&ctx, ln, log, None)
}

/// rush remote token|list|add|attach|remove.
pub fn remote_cmd(args: &[String], stdout: &mut dyn Write) -> Result<(), String> {
    let mut cfg = load_config()?;
    let sub = args.first().map_or("", String::as_str);
    match (sub, args.len()) {
        ("token", _) => {
            let t = token();
            let _ = writeln!(stdout, "{}", t.as_deref().unwrap_or(""));
            return t.map(|_| ());
        }
        ("list", _) => {
            for p in &cfg.peers {
                let _ = writeln!(stdout, "{}  {}", p.name, p.url);
            }
            return Ok(());
        }
        ("add", 4) => {
            let (name, u, tok) = (&args[1], args[2].trim_end_matches('/'), &args[3]);
            if !u.starts_with("https://") && !u.starts_with("http://") {
                return Err("the URL starts https:// (or http:// over a tailnet)".into());
            }
            if name.is_empty() || name.contains(['/', ' ']) {
                return Err("a machine's name has no spaces or slashes".into());
            }
            cfg.peers.retain(|p| &p.name != name);
            cfg.peers.push(Peer {
                name: name.clone(),
                url: u.into(),
                token: tok.clone(),
            });
            return save_config(&cfg);
        }
        ("attach", 2) => {
            let Some(p) = cfg.peer(&args[1]) else {
                return Err(format!(
                    "no machine {:?}: rush remote add it first",
                    args[1]
                ));
            };
            let ctx = Ctx::new();
            rush_watch::stop::on_signals();
            {
                let c = ctx.clone();
                std::thread::spawn(move || {
                    while rush_watch::stop::sleep(Duration::from_secs(86_400)) {}
                    c.cancel();
                });
            }
            eprintln!(
                "rush remote attach: {}'s sessions are in rush here until this ends",
                p.name
            );
            let log: rush_remote::expose::Log = Arc::new(Mutex::new(std::io::stderr()));
            return rush_remote::attach::attach(&ctx, &p, &log);
        }
        ("remove", 2) => {
            cfg.peers.retain(|p| p.name != args[1]);
            return save_config(&cfg);
        }
        _ => {}
    }
    let _ = write!(stdout, "{REMOTE_USAGE}");
    Ok(())
}

/// rush session --on NAME …: the session commands, run through that
/// machine's serve. Its exit status.
pub fn session_on(
    name: &str,
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let as_json = args
        .iter()
        .skip(1)
        .any(|a| a == "--json" || a == "--json=true");
    match remote_session(name, args, stdin, stdout) {
        Ok(()) => 0,
        Err(e) => {
            if as_json {
                let _ = writeln!(stdout, "{}", json!({ "error": e }));
            } else {
                let _ = writeln!(stderr, "rush: {e}");
            }
            1
        }
    }
}

/// A session as the session commands print it.
fn show(s: &Session, as_json: bool, stdout: &mut dyn Write) {
    if as_json {
        let _ = writeln!(stdout, "{}", serde_json::to_string(s).unwrap_or_default());
    } else {
        let _ = writeln!(stdout, "{}  {:<8} {}", s.info.id, s.info.state, s.info.name);
    }
}

fn remote_session(
    name: &str,
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), String> {
    let cfg = load_config()?;
    let Some(p) = cfg.peer(name) else {
        return Err(format!("no machine {name:?}: rush remote add it first"));
    };
    let Some(sub) = args.first() else {
        return Err(SESSION_USAGE.into());
    };
    let rest = &args[1..];
    let as_json = rest.iter().any(|a| a == "--json" || a == "--json=true");
    let call = |method: &str, path: &str, body: Option<Value>| -> Result<Vec<u8>, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let url = format!("{}{path}", p.url);
        let auth = format!("Bearer {}", p.token);
        let res = if method == "POST" {
            let b = serde_json::to_vec(&body.unwrap_or(Value::Null)).unwrap_or_default();
            agent
                .post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(&b[..])
        } else {
            agent
                .get(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .call()
        };
        let mut res = res.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let b = res
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_vec()
            .unwrap_or_default();
        if status >= 300 {
            let e: rush_remote::server::ErrorBody = serde_json::from_slice(&b).unwrap_or_default();
            if !e.error.is_empty() {
                return Err(format!("{name}: {}", e.error));
            }
            return Err(format!(
                "{name}: {}",
                rush_remote::client::status_line(status)
            ));
        }
        Ok(b)
    };
    match sub.as_str() {
        "list" => {
            if rest.iter().any(|a| a == "--others" || a == "--others=true") {
                let b = call("GET", "/api/others", None)?;
                let list: Vec<Other> = serde_json::from_slice::<Option<Vec<Other>>>(&b)
                    .map_err(|e| e.to_string())?
                    .unwrap_or_default();
                if as_json {
                    let _ = writeln!(
                        stdout,
                        "{}",
                        serde_json::to_string(&list).unwrap_or_default()
                    );
                    return Ok(());
                }
                for o in list {
                    let state = if o.live { "running" } else { "past" };
                    let _ = writeln!(
                        stdout,
                        "{}  {:<8} {:<7} {}  {}",
                        o.session_id, o.agent, state, o.name, o.cwd
                    );
                }
                return Ok(());
            }
            let b = call("GET", "/api/sessions", None)?;
            let list: Vec<Session> = serde_json::from_slice::<Option<Vec<Session>>>(&b)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            if as_json {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&list).unwrap_or_default()
                );
                return Ok(());
            }
            for s in &list {
                show(s, false, stdout);
            }
            Ok(())
        }
        "info" => {
            let mut fs = Flags::new("info", &[], &["json"]);
            let id = fs.id_and_flags(rest)?;
            let b = call("GET", "/api/sessions", None)?;
            let list: Vec<Session> = serde_json::from_slice::<Option<Vec<Session>>>(&b)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            match list.iter().find(|s| s.info.id == id) {
                Some(s) => {
                    show(s, as_json, stdout);
                    Ok(())
                }
                None => Err(format!("session {id} not found")),
            }
        }
        "start" => {
            let mut fs = Flags::new(
                "start",
                &[
                    "cwd",
                    "agent",
                    "profile",
                    "name",
                    "model",
                    "effort",
                    "resume",
                    "prompt-file",
                ],
                &["json"],
            );
            fs.parse(rest)?;
            let mut r = StartRequest {
                cwd: fs.s("cwd"),
                agent: fs.s("agent"),
                profile: fs.s("profile"),
                name: fs.s("name"),
                model: fs.s("model"),
                effort: fs.s("effort"),
                resume: fs.s("resume"),
                ..StartRequest::default()
            };
            let pf = fs.s("prompt-file");
            if !pf.is_empty() {
                let b = if pf == "-" {
                    let mut b = Vec::new();
                    stdin.read_to_end(&mut b).map_err(|e| e.to_string())?;
                    b
                } else {
                    std::fs::read(&pf).map_err(|e| e.to_string())?
                };
                r.prompt = String::from_utf8_lossy(&b)
                    .trim_end_matches('\n')
                    .to_string();
            }
            let b = call(
                "POST",
                "/api/sessions",
                Some(serde_json::to_value(&r).unwrap_or_default()),
            )?;
            let s: Session = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
            show(&s, as_json, stdout);
            Ok(())
        }
        "send" | "interrupt" | "stop" => {
            let mut fs = Flags::new(sub, &[], &["json", "now"]);
            let id = fs.id_and_flags(rest)?;
            let mut op = json!({ "op": sub });
            if sub == "send" {
                let mut b = Vec::new();
                stdin.read_to_end(&mut b).map_err(|e| e.to_string())?;
                op["text"] =
                    json!(String::from_utf8_lossy(&b).trim_end_matches([' ', '\t', '\r', '\n']));
                op["now"] = json!(fs.b("now"));
            }
            call("POST", &format!("/api/sessions/{id}/op"), Some(op)).map(|_| ())
        }
        "watch" => {
            let mut fs = Flags::new("watch", &[], &["json"]);
            let id = fs.id_and_flags(rest)?;
            remote_watch(&p, &id, as_json, stdout)
        }
        other => Err(format!("unknown session command {other:?}")),
    }
}

/// Prints a remote session's events as they come: each as a JSON line
/// with --json, else what's said.
fn remote_watch(p: &Peer, id: &str, as_json: bool, stdout: &mut dyn Write) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let res = agent
        .get(&format!("{}/api/sessions/{id}/events", p.url))
        .header("Authorization", &format!("Bearer {}", p.token))
        .call()
        .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(format!(
            "{}: {}",
            p.name,
            rush_remote::client::status_line(status)
        ));
    }
    let r = BufReader::new(res.into_body().into_reader());
    for l in r.lines() {
        let l = l.map_err(|e| e.to_string())?;
        let Some(data) = l.strip_prefix("data: ") else {
            continue;
        };
        if as_json {
            let _ = writeln!(stdout, "{data}");
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        // Go reads its fields in any case.
        let field = |v: &Value, k: &str| -> Value {
            v.as_object()
                .and_then(|m| {
                    m.iter()
                        .find(|(n, _)| n.eq_ignore_ascii_case(k))
                        .map(|(_, v)| v.clone())
                })
                .unwrap_or(Value::Null)
        };
        let e = field(&ev, "e");
        let text = |v: Value| v.as_str().unwrap_or_default().to_string();
        match text(field(&ev, "t")).as_str() {
            "message" => {
                let role = text(field(&e, "Role"));
                for pt in field(&e, "Parts").as_array().cloned().unwrap_or_default() {
                    let t = text(field(&pt, "Text"));
                    if field(&pt, "Kind").as_i64().unwrap_or(0) == 0 && !t.is_empty() {
                        let _ = writeln!(stdout, "{role}: {t}");
                    }
                }
            }
            "turn_end" => {
                let _ = writeln!(
                    stdout,
                    "— turn {} {}",
                    text(field(&e, "Reason")),
                    text(field(&e, "Err"))
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// What rush session's usage says (session.go sessionUsage).
const SESSION_USAGE: &str = "rush session: run rush-mode sessions without the view

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

/// Starts a session as a remote client asks, as rush session start does
/// (session.go startSession :269, with what serve passes it).
pub fn start_session(reg: &Registry, r: &StartRequest) -> Result<Info, String> {
    let st = rush_state::Store::load();
    let d = &st.config.dispatch;
    let mut cfg = Config::default();
    let resume = !r.resume.is_empty();
    let mut kind = r.agent.clone();
    if resume {
        let id = rush_host::short_id(&r.resume);
        if let Ok(info) = rush_host::read_info(&id) {
            if rush_host::running(&info) {
                // Already running: nothing to start.
                return Ok(info);
            }
            if let Ok(saved) = rush_host::read_config(&id) {
                cfg = saved;
            }
        }
        if cfg.id.is_empty() {
            cfg = Config {
                id,
                session_id: r.resume.clone(),
                ..Config::default()
            };
        }
        cfg.resume = true;
    }
    if !r.cwd.is_empty() {
        cfg.cwd = std::path::absolute(&r.cwd)
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
    if !r.profile.is_empty() && st.config.profile_named(&r.profile).is_none() {
        return Err(format!("no profile named {:?}", r.profile));
    }
    if !cfg.resume || cfg.profile.is_empty() {
        let p = st.config.profile_for(&cfg.cwd, &r.profile);
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
    cfg.prompt = r.prompt.clone();
    cfg.lean = d.quick_start();
    if cfg.limit_mode.is_empty() && kind == rush_state::LOGINS_KIND {
        cfg.limit_mode = d.on_limit.clone();
    }
    cfg.model = or(&r.model, &or(&cfg.model, &start.model));
    cfg.effort = or(&r.effort, &or(&cfg.effort, &start.effort));
    cfg.permission_mode = or(&cfg.permission_mode, &start.mode);
    cfg.name = or(&r.name, &cfg.name);
    if cfg.name.is_empty() {
        cfg.name = first_words_of(&r.prompt);
    }
    if cfg.name.is_empty() {
        let base = Path::new(&cfg.cwd)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        cfg.name = format!("fresh session in {base}");
    }
    // The spawned line, its host's cause.
    cfg.cause = rush_events::id();
    let started = rush_host::spawn(cfg.clone()).map_err(|e| {
        rush_host::events::log(
            "start-failed",
            json!({ "agent": kind, "cwd": cfg.cwd, "resume": cfg.resume, "err": e.to_string() }),
        );
        e.to_string()
    })?;
    let clipped = {
        let n = r.prompt.chars().count();
        if n > 300 {
            format!("{}…", r.prompt.chars().take(300).collect::<String>())
        } else {
            r.prompt.clone()
        }
    };
    rush_host::events::log_event(
        &started.id,
        "spawned",
        json!({ "ev": cfg.cause, "by": "rush session start", "spawnedBy": cfg.meta.get("spawnedBy").cloned().unwrap_or_default(),
            "agent": kind, "cwd": cfg.cwd, "resume": cfg.resume, "mode": cfg.permission_mode, "model": cfg.model, "prompt": clipped }),
    );
    wait_info(&started.id)
}

/// The info of a session just started, once its host is up: at most 10s
/// (waitInfo).
fn wait_info(id: &str) -> Result<Info, String> {
    let end = std::time::Instant::now() + Duration::from_secs(10);
    let mut err = None;
    while std::time::Instant::now() < end {
        match rush_host::read_info(id) {
            Ok(i) if rush_host::alive(i.host_pid) => return Ok(i),
            Ok(_) => err = None,
            Err(e) => err = Some(e.to_string()),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(err.unwrap_or_else(|| format!("the host for {id} stopped right after starting")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn flags_as_go_reads_them() {
        let mut f = Flags::new("start", &["cwd", "name"], &["json"]);
        f.parse(&s(&[
            "--cwd", "/x", "-name=n", "--json", "rest", "--cwd", "y",
        ]))
        .unwrap();
        assert_eq!(f.s("cwd"), "/x");
        assert_eq!(f.s("name"), "n");
        assert!(f.b("json"));
        assert_eq!(f.rest, s(&["rest", "--cwd", "y"]));
        let mut f = Flags::new("send", &[], &["now"]);
        assert_eq!(f.id_and_flags(&s(&["abc", "--now"])).unwrap(), "abc");
        assert!(f.b("now"));
        let mut f = Flags::new("send", &[], &["now"]);
        assert_eq!(f.id_and_flags(&s(&["--now", "abc"])).unwrap(), "abc");
        let mut f = Flags::new("send", &[], &[]);
        assert_eq!(
            f.id_and_flags(&[]).unwrap_err(),
            "usage: rush session send <id>"
        );
        let mut f = Flags::new("x", &[], &[]);
        assert_eq!(
            f.parse(&s(&["--nope"])).unwrap_err(),
            "flag provided but not defined: -nope"
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

    #[test]
    fn remote_list_add_remove() {
        let mut out = Vec::new();
        remote_cmd(&s(&["add", "box", "http://box:7878/", "tok"]), &mut out).unwrap();
        remote_cmd(&s(&["list"]), &mut out).unwrap();
        assert_eq!(String::from_utf8_lossy(&out), "box  http://box:7878\n");
        assert!(remote_cmd(&s(&["add", "b x", "http://x", "t"]), &mut out).is_err());
        assert_eq!(
            remote_cmd(&s(&["add", "x", "ftp://x", "t"]), &mut out).unwrap_err(),
            "the URL starts https:// (or http:// over a tailnet)"
        );
        remote_cmd(&s(&["remove", "box"]), &mut out).unwrap();
        out.clear();
        remote_cmd(&s(&["list"]), &mut out).unwrap();
        assert!(out.is_empty());
        remote_cmd(&s(&[]), &mut out).unwrap();
        assert_eq!(String::from_utf8_lossy(&out), REMOTE_USAGE);
        // The token is made once and kept.
        let mut a = Vec::new();
        remote_cmd(&s(&["token"]), &mut a).unwrap();
        let mut b = Vec::new();
        remote_cmd(&s(&["token"]), &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(String::from_utf8_lossy(&a).trim().len(), 64);
    }
}
