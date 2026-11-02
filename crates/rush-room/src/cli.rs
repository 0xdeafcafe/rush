//! `rush room`: open a room, follow it, speak in it.
//! Ported from cmd/rush/room.go:1-251, with Go's flag package's parsing of
//! rush room new's flags.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rush_state::json::RawJson;

use crate::Agents;
use crate::driver::{self, Sessions};
use crate::feed::clock;
use crate::room::{
    CONTROL, DEFAULT_ROUNDS, DONE, END, Entry, INFO, Member, NOTE, PAUSED, Room, Rooms, SAY, TOOL,
    TURN, USER, USES, VERDICT, name, parse,
};

/// rush room's help.
pub const ROOM_USAGE: &str = r#"rush room: a group chat of fresh agents arguing a topic to a verdict

  rush room new [--cwd DIR] [--rounds N] [--watch] --agent KIND[:MODEL[:EFFORT]]... <topic>
        opens a room with a fresh panel (two or more --agent), prints its id
  rush room show <id> [--follow]     the transcript; --follow until it ends
  rush room say <id> [text]          post as you (text or stdin): "@name ..." to
                                     one agent, "/verdict" to end on a verdict, "/stop"
  rush room pause <id>               stop the speaker now; no turn starts until resume
  rush room resume <id>              carry on: the paused agent picks its turn up again
  rush room list

In rush itself, #room opens the full-screen view.
"#;

/// What a new member's session starts with: a fresh session of kind as
/// Settings starts it, on model and effort when given (host.Config, as
/// JSON). The host module answers it.
pub trait MemberConfig {
    fn member_config(
        &self,
        kind: &str,
        model: &str,
        effort: &str,
        cwd: &str,
    ) -> Result<RawJson, String>;
    /// A new session id: a room's is one (host.NewSessionID).
    fn new_session_id(&self) -> String;
}

/// What `rush room` works with.
pub struct RoomCtx {
    pub rooms: Rooms,
    pub agents: Arc<dyn Agents>,
    /// The room driver's sessions; None where the host module isn't wired.
    pub sessions: Option<Arc<dyn Sessions>>,
    pub configs: Option<Arc<dyn MemberConfig>>,
    /// The rush that runs a room's driver, found as it starts: rush's own
    /// binary once an install has finished writing it (room.go:181,
    /// selfexe.Path).
    pub exe: fn() -> std::io::Result<PathBuf>,
    /// Where relative folders are from.
    pub cwd: PathBuf,
}

/// `rush room …`: its exit code.
pub fn room_cmd(
    ctx: &RoomCtx,
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "-h" | "--help") {
        let _ = stdout.write_all(ROOM_USAGE.as_bytes());
        return 0;
    }
    let r = match args[0].as_str() {
        "run" => {
            // the driver: rush room new starts it
            if args.len() != 2 {
                Err("usage: rush room run <id>".to_string())
            } else {
                match &ctx.sessions {
                    Some(s) => driver::run(&ctx.rooms, &args[1], &**s, &*ctx.agents),
                    None => Err("rooms need rush's host to run their sessions".into()),
                }
            }
        }
        "new" => room_new(ctx, &args[1..], stdout),
        "show" => room_show(ctx, &args[1..], stdout),
        "say" => room_say(ctx, &args[1..], stdin),
        "pause" | "resume" | "stop" | "verdict" => {
            if args.len() != 2 {
                Err(format!("usage: rush room {} <id>", args[0]))
            } else {
                room_say(ctx, &[args[1].clone(), format!("/{}", args[0])], stdin)
            }
        }
        "list" => {
            for r in ctx.rooms.list() {
                let status = if ctx.rooms.running(&r.id) {
                    "live"
                } else {
                    "over"
                };
                let names: Vec<&str> = r.members.iter().map(|m| m.name.as_str()).collect();
                let _ = writeln!(
                    stdout,
                    "{}  {status}  {}  ({})",
                    r.id,
                    r.topic,
                    names.join(", ")
                );
            }
            Ok(())
        }
        _ => Err(format!(
            "unknown room command {}; run rush room help",
            crate::room::go_quote(&args[0])
        )),
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(stderr, "rush: {e}");
            1
        }
    }
}

/// rush room new's flags, as Go's flag package reads them.
#[derive(Debug, PartialEq)]
struct NewFlags {
    cwd: String,
    rounds: i64,
    watch: bool,
    agents: Vec<String>,
    rest: Vec<String>,
}

fn new_flags(args: &[String]) -> Result<NewFlags, String> {
    let mut f = NewFlags {
        cwd: ".".into(),
        rounds: DEFAULT_ROUNDS,
        watch: false,
        agents: Vec::new(),
        rest: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.len() < 2 || !a.starts_with('-') {
            break;
        }
        let mut name = &a[1..];
        if let Some(n) = name.strip_prefix('-') {
            if n.is_empty() {
                i += 1; // "--" ends the flags
                break;
            }
            name = n;
        }
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err(format!("bad flag syntax: {a}"));
        }
        i += 1;
        let (name, value) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (name, None),
        };
        if name == "watch" {
            f.watch = match value.as_deref() {
                None => true,
                Some(v) => go_bool(v).ok_or(format!(
                    "invalid boolean value {} for -{name}: parse error",
                    crate::room::go_quote(v)
                ))?,
            };
            continue;
        }
        if !matches!(name, "cwd" | "rounds" | "agent") {
            return Err(format!("flag provided but not defined: -{name}"));
        }
        let value = match value {
            Some(v) => v,
            None if i < args.len() => {
                i += 1;
                args[i - 1].clone()
            }
            None => return Err(format!("flag needs an argument: -{name}")),
        };
        match name {
            "cwd" => f.cwd = value,
            "rounds" => {
                f.rounds = go_int(&value).ok_or(format!(
                    "invalid value {} for flag -{name}: parse error",
                    crate::room::go_quote(&value)
                ))?;
            }
            _ => f.agents.push(value),
        }
    }
    f.rest = args[i..].to_vec();
    Ok(f)
}

fn go_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// An int as Go's flag reads one: strconv.ParseInt(s, 0, 64).
fn go_int(s: &str) -> Option<i64> {
    let (neg, t) = match s.strip_prefix('-') {
        Some(t) => (true, t),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let t = t.replace('_', "");
    let (radix, digits) = if let Some(d) = t.strip_prefix("0x").or(t.strip_prefix("0X")) {
        (16, d.to_string())
    } else if let Some(d) = t.strip_prefix("0b").or(t.strip_prefix("0B")) {
        (2, d.to_string())
    } else if let Some(d) = t.strip_prefix("0o").or(t.strip_prefix("0O")) {
        (8, d.to_string())
    } else if t.len() > 1 && t.starts_with('0') {
        (8, t[1..].to_string())
    } else {
        (10, t.clone())
    };
    let n = i64::from_str_radix(&digits, radix).ok()?;
    Some(if neg { -n } else { n })
}

fn room_new(ctx: &RoomCtx, args: &[String], stdout: &mut dyn Write) -> Result<(), String> {
    let f = new_flags(args)?;
    let topic = f.rest.join(" ").trim().to_string();
    if topic.is_empty() || f.agents.len() < 2 {
        return Err(
            "usage: rush room new --agent KIND[:MODEL] --agent KIND[:MODEL]... <topic>".into(),
        );
    }
    let Some(configs) = &ctx.configs else {
        return Err("rooms need rush's host to start their sessions".into());
    };
    let abs = absolute(&ctx.cwd, &f.cwd);
    let mut r = Room {
        topic,
        cwd: abs.clone(),
        rounds: f.rounds,
        ..Room::default()
    };
    let mut taken: Vec<String> = Vec::new();
    for a in &f.agents {
        let mut parts: Vec<&str> = a.splitn(3, ':').collect();
        parts.resize(3, "");
        let cfg = configs.member_config(parts[0], parts[1], parts[2], &abs)?;
        let view: serde_json::Value = serde_json::from_str(cfg.get()).unwrap_or_default();
        let kind = view["kind"].as_str().unwrap_or("");
        let model = view["model"].as_str().unwrap_or("");
        let n = name(&*ctx.agents, kind, model, &taken);
        taken.push(n.clone());
        r.members.push(Member {
            name: n,
            config: cfg,
            context: 0,
        });
    }
    let r = ctx.rooms.create(r, || configs.new_session_id())?;
    let exe = (ctx.exe)().map_err(|e| e.to_string())?;
    ctx.rooms.start(&exe, &r.id)?;
    let _ = writeln!(stdout, "room {}: {}", r.id, taken.join(", "));
    if f.watch {
        return room_show(ctx, &[r.id.clone(), "--follow".into()], stdout);
    }
    Ok(())
}

/// dir made absolute from cwd, cleaned as Go's filepath.Abs does.
fn absolute(cwd: &Path, dir: &str) -> String {
    let p = if dir.starts_with('/') {
        PathBuf::from(dir)
    } else {
        cwd.join(dir)
    };
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        use std::path::Component::*;
        match c {
            RootDir | Prefix(_) | CurDir => {}
            ParentDir => {
                parts.pop();
            }
            Normal(s) => parts.push(s.to_string_lossy().into_owned()),
        }
    }
    format!("/{}", parts.join("/"))
}

/// How long a follow waits on the log before asking whether its driver
/// still runs.
const FOLLOW_LOOK: Duration = Duration::from_secs(2);

fn room_show(ctx: &RoomCtx, args: &[String], stdout: &mut dyn Write) -> Result<(), String> {
    if args.is_empty() {
        return Err("usage: rush room show <id> [--follow]".into());
    }
    let id = &args[0];
    let follow = args.len() > 1 && args[1] == "--follow";
    ctx.rooms.load(id)?;
    let mut off = 0u64;
    loop {
        let (es, next) = ctx.rooms.read(id, off).map_err(|e| e.to_string())?;
        off = next;
        for e in &es {
            let line = room_line(e);
            if !line.is_empty() {
                let _ = writeln!(stdout, "{line}");
            }
            if e.kind == END {
                return Ok(());
            }
        }
        if !follow {
            return Ok(());
        }
        if es.is_empty() && !ctx.rooms.running(id) {
            return Err("the room's driver isn't running".into());
        }
        // Waits for the log to grow, not a poll of it; a look a while to
        // see the driver is still there.
        wait_grown(&ctx.rooms.log_path(id), off, FOLLOW_LOOK);
    }
}

/// Waits until path grows past size, or for d.
fn wait_grown(path: &Path, size: u64, d: Duration) {
    let Ok(stop) = rush_sys::fswait::Stop::new() else {
        return;
    };
    let stop = Arc::new(stop);
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let s2 = stop.clone();
    let timer = std::thread::spawn(move || {
        let _ = done_rx.recv_timeout(d);
        s2.close();
    });
    let file = rush_sys::fswait::File {
        path: path.to_path_buf(),
        size,
    };
    rush_sys::fswait::grown(&stop, &[file]);
    drop(done_tx);
    let _ = timer.join();
}

/// An entry as rush room show prints it.
pub fn room_line(e: &Entry) -> String {
    let at = clock(e.at, 19);
    match e.kind.as_str() {
        SAY => {
            let mut who = e.from.clone();
            if who == USER {
                who = "you".into();
                if !e.to.is_empty() {
                    who += &format!(" → {}", e.to);
                }
            }
            let ready = if e.ready.is_empty() {
                String::new()
            } else {
                format!("  [READY: {}]", e.ready)
            };
            let round = if e.round > 0 {
                format!(" · round {}", e.round)
            } else {
                String::new()
            };
            format!("\n{at}  {who}{round}{ready}\n{}", e.text)
        }
        NOTE => format!("{at}  {} (on the way): {}", e.from, e.text),
        TOOL => format!("{at}  {} ⏺ {}  {}", e.from, e.text, e.detail),
        DONE => {
            if e.failed {
                format!("{at}  {} ✗ tool failed", e.from)
            } else {
                String::new()
            }
        }
        VERDICT => format!(
            "\n{at}  ══ VERDICT: each agent's final position ({}) ══\n{}",
            e.detail, e.text
        ),
        PAUSED => format!("{at}  ⏸ {}", e.text),
        USES => format!("{at}  — {} runs {}", e.from, e.text),
        CONTROL => format!("{at}  you: /{}", e.text),
        TURN => format!("{at}  … {} is speaking", e.from),
        INFO | END => format!("{at}  — {}", e.text),
        _ => String::new(),
    }
}

fn room_say(ctx: &RoomCtx, args: &[String], stdin: &mut dyn Read) -> Result<(), String> {
    if args.is_empty() {
        return Err("usage: rush room say <id> [text]".into());
    }
    let r = ctx.rooms.load(&args[0])?;
    let mut text = args[1..].join(" ");
    if text.is_empty() {
        let mut b = Vec::new();
        stdin
            .take(1 << 20)
            .read_to_end(&mut b)
            .map_err(|e| e.to_string())?;
        text = String::from_utf8_lossy(&b).into_owned();
    }
    if text.trim().is_empty() {
        return Err("nothing to say".into());
    }
    if !ctx.rooms.running(&r.id) {
        return Err("the room is over".into());
    }
    ctx.rooms
        .post(&r.id, parse(&text, &r.members))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::NoAgents;
    use rush_state::Time;

    fn ctx(home: &Path) -> RoomCtx {
        RoomCtx {
            rooms: Rooms::at(home),
            agents: Arc::new(NoAgents),
            sessions: None,
            configs: None,
            exe: || Ok(PathBuf::from("/nonexistent/rush")),
            cwd: PathBuf::from("/w"),
        }
    }

    fn cli(c: &RoomCtx, input: &str, args: &[&str]) -> (String, i32) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = room_cmd(c, &args, &mut input.as_bytes(), &mut out, &mut err);
        out.extend(err);
        (String::from_utf8(out).unwrap(), code)
    }

    #[test]
    fn flags_as_go_reads_them() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let f = new_flags(&s(&[
            "--cwd",
            "x",
            "-rounds=2",
            "--watch",
            "--agent",
            "claude",
            "-agent=codex:gpt-6-luna",
            "tabs",
            "--or",
            "spaces",
        ]))
        .unwrap();
        assert_eq!(
            f,
            NewFlags {
                cwd: "x".into(),
                rounds: 2,
                watch: true,
                agents: s(&["claude", "codex:gpt-6-luna"]),
                rest: s(&["tabs", "--or", "spaces"]),
            }
        );
        assert_eq!(
            new_flags(&s(&["--nope"])).unwrap_err(),
            "flag provided but not defined: -nope"
        );
        assert_eq!(
            new_flags(&s(&["--rounds", "x"])).unwrap_err(),
            "invalid value \"x\" for flag -rounds: parse error"
        );
        assert_eq!(
            new_flags(&s(&["--cwd"])).unwrap_err(),
            "flag needs an argument: -cwd"
        );
        assert_eq!(new_flags(&s(&["--", "-x"])).unwrap().rest, s(&["-x"]));
        assert_eq!(absolute(Path::new("/w/a"), "../b/./c"), "/w/b/c");
    }

    #[test]
    fn lines_as_show_prints_them() {
        let at = Time::parse("2026-10-29T14:04:05.5+01:00").unwrap();
        let e = |kind: &str, f: &dyn Fn(&mut Entry)| {
            let mut e = Entry {
                at,
                from: "Opus".into(),
                kind: kind.into(),
                ..Entry::default()
            };
            f(&mut e);
            room_line(&e)
        };
        assert_eq!(
            e(SAY, &|e| {
                e.text = "Tabs.".into();
                e.ready = "yes".into();
                e.round = 2
            }),
            "\n14:04:05  Opus · round 2  [READY: yes]\nTabs."
        );
        assert_eq!(
            e(SAY, &|e| {
                e.from = USER.into();
                e.to = "Codex".into();
                e.text = "why?".into()
            }),
            "\n14:04:05  you → Codex\nwhy?"
        );
        assert_eq!(
            e(TOOL, &|e| {
                e.text = "running go test".into();
                e.detail = "go test ./...".into()
            }),
            "14:04:05  Opus ⏺ running go test  go test ./..."
        );
        assert_eq!(e(DONE, &|_| {}), "");
        assert_eq!(
            e(DONE, &|e| e.failed = true),
            "14:04:05  Opus ✗ tool failed"
        );
        assert_eq!(
            e(VERDICT, &|e| {
                e.detail = "why".into();
                e.text = "Opus: tabs".into()
            }),
            "\n14:04:05  ══ VERDICT: each agent's final position (why) ══\nOpus: tabs"
        );
        assert_eq!(e(TURN, &|_| {}), "14:04:05  … Opus is speaking");
        assert_eq!(
            e(CONTROL, &|e| e.text = "pause".into()),
            "14:04:05  you: /pause"
        );
        assert_eq!(
            e(USES, &|e| e.text = "opus-5".into()),
            "14:04:05  — Opus runs opus-5"
        );
        assert_eq!(e(END, &|e| e.text = "over".into()), "14:04:05  — over");
        assert_eq!(
            e(NOTE, &|e| e.text = "hm".into()),
            "14:04:05  Opus (on the way): hm"
        );
        assert_eq!(e(PAUSED, &|e| e.text = "p".into()), "14:04:05  ⏸ p");
        assert_eq!(e("joined", &|_| {}), "");
    }

    #[test]
    fn show_say_and_list() {
        let home = tempfile::tempdir().unwrap();
        let c = ctx(home.path());
        assert_eq!(cli(&c, "", &[]).0, ROOM_USAGE);
        assert_eq!(
            cli(&c, "", &["show", "nope"]),
            ("rush: no room \"nope\"\n".into(), 1)
        );
        assert_eq!(
            cli(&c, "", &["frob"]),
            (
                "rush: unknown room command \"frob\"; run rush room help\n".into(),
                1
            )
        );
        assert_eq!(
            cli(&c, "", &["new", "--agent", "codex", "topic"]),
            (
                "rush: usage: rush room new --agent KIND[:MODEL] --agent KIND[:MODEL]... <topic>\n"
                    .into(),
                1
            )
        );
        let r = c
            .rooms
            .create(
                Room {
                    topic: "Tabs".into(),
                    members: vec![
                        Member {
                            name: "Opus".into(),
                            ..Member::default()
                        },
                        Member {
                            name: "Codex".into(),
                            ..Member::default()
                        },
                    ],
                    ..Room::default()
                },
                || "r1".into(),
            )
            .unwrap();
        assert_eq!(cli(&c, "", &["list"]).0, "r1  over  Tabs  (Opus, Codex)\n");
        assert_eq!(
            cli(&c, "", &["say", "r1", "hi"]),
            ("rush: the room is over\n".into(), 1)
        );
        assert_eq!(
            cli(&c, "", &["pause"]),
            ("rush: usage: rush room pause <id>\n".into(), 1)
        );
        // Its driver: this process, as far as the pid file says.
        std::fs::write(c.rooms.pid_path(&r.id), std::process::id().to_string()).unwrap();
        assert_eq!(cli(&c, "@codex why?", &["say", "r1"]).1, 0);
        assert_eq!(cli(&c, "", &["pause", "r1"]).1, 0);
        let (es, _) = c.rooms.read("r1", 0).unwrap();
        assert_eq!(es[0].to, "Codex");
        assert_eq!(
            (es[1].kind.as_str(), es[1].text.as_str()),
            (CONTROL, "pause")
        );
        c.rooms
            .post(
                "r1",
                Entry {
                    from: "room".into(),
                    kind: END.into(),
                    text: "stopped by you".into(),
                    ..Entry::default()
                },
            )
            .unwrap();
        let (out, code) = cli(&c, "", &["show", "r1", "--follow"]);
        assert_eq!(code, 0);
        assert!(out.ends_with("  — stopped by you\n"), "{out}");
        assert!(out.contains("you → Codex\n@codex why?\n"), "{out}");
    }
}
