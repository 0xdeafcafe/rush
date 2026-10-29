//! `rush session`: rush-mode sessions without the view. `send` is here:
//! stdin to a session, waking its host first if it rests, into the turn
//! under way, or with --now stopping it; another rush session sending is
//! attributed as an agent's exchange. It's how push-later's failed push
//! reaches its session. `--on MACHINE` is serve.rs's; start, interrupt,
//! stop, watch, info and list are still to come.
//!
//! Ported from cmd/rush/session.go (sessionUsage :25, sessionCmd :64,
//! idAndFlags :115, sessionExists :156, sessionSend :419) and
//! exchange.go:24 (outgoingExchange, here spawn.rs's).

use std::io::{Read, Write};
use std::time::Duration;

use rush_host::client::{Client, ClientError, await_line, ensure, read_info, resting};
use rush_host::events::log_event;
use rush_host::{Line, alive};
use serde_json::json;

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

/// `rush session <sub>`: its exit code (sessionCmd). `--on` is the
/// caller's.
pub fn session_cmd(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let Some(sub) = args.first().map(String::as_str) else {
        let _ = write!(stdout, "{USAGE}");
        return 0;
    };
    let r = match sub {
        "-h" | "--help" | "help" => {
            let _ = write!(stdout, "{USAGE}");
            return 0;
        }
        "send" => session_send(&args[1..], stdin, stdout),
        "start" | "interrupt" | "stop" | "watch" | "info" | "list" => Err(format!(
            "rush session {sub} isn't in this rush yet: the Go rush has it"
        )),
        other => Err(format!("unknown session command {other:?}\n\n{USAGE}")),
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(stderr, "rush: {e}");
            1
        }
    }
}

/// send's flags: the session's id, before or after them, --now, and any
/// number of --image (idAndFlags with Go's flag package).
#[derive(Debug, Default, PartialEq)]
struct SendArgs {
    id: String,
    now: bool,
    images: Vec<String>,
}

fn send_args(args: &[String]) -> Result<SendArgs, String> {
    let mut out = SendArgs::default();
    let mut args = args;
    if let Some(first) = args.first()
        && !first.starts_with('-')
    {
        out.id = first.clone();
        args = &args[1..];
    }
    let mut i = 0;
    let mut rest: &[String] = &[];
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            rest = &args[i + 1..];
            break;
        }
        let Some(f) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
            rest = &args[i..];
            break;
        };
        if f.is_empty() || f.starts_with('-') || f.starts_with('=') {
            return Err(format!("bad flag syntax: {a}"));
        }
        let (k, v) = match f.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (f, None),
        };
        match k {
            "now" => {
                out.now = match v.as_deref() {
                    None | Some("true" | "1" | "t" | "T" | "TRUE" | "True") => true,
                    Some("false" | "0" | "f" | "F" | "FALSE" | "False") => false,
                    Some(x) => {
                        return Err(format!("invalid boolean value {x:?} for -now: parse error"));
                    }
                }
            }
            "image" => {
                let v = match v {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or_else(|| "flag needs an argument: -image".to_string())?
                    }
                };
                out.images.push(v);
            }
            _ => return Err(format!("flag provided but not defined: -{k}")),
        }
        i += 1;
    }
    if out.id.is_empty()
        && let Some(id) = rest.first()
    {
        out.id = id.clone();
    }
    if out.id.is_empty() {
        return Err("usage: rush session send <id>".into());
    }
    Ok(out)
}

/// Whether id has a session folder (sessionExists).
fn session_exists(id: &str) -> bool {
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        return false;
    }
    rush_host::root().join(id).is_dir()
}

/// Sends stdin to session id, resuming it first if its host isn't
/// running (sessionSend).
fn session_send(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), String> {
    let SendArgs {
        id,
        now,
        mut images,
    } = send_args(args)?;
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
    use crate::hosted::{RushHosts, Starts};
    use rush_core::adapter::Registry;
    use rush_host::{Config, fake};
    use rush_ui::screens::session_core::host::{Hosts, Start};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // idAndFlags: the id before or after the flags, --image any number
    // of times, as Go's flag package reads them.
    #[test]
    fn send_flags() {
        let got = send_args(&s(&["abc", "--now", "--image", "a.png", "-image=b.png"])).unwrap();
        assert_eq!(
            got,
            SendArgs {
                id: "abc".into(),
                now: true,
                images: s(&["a.png", "b.png"]),
            }
        );
        let got = send_args(&s(&["--now=false", "abc"])).unwrap();
        assert_eq!((got.id.as_str(), got.now), ("abc", false));
        assert_eq!(send_args(&[]).unwrap_err(), "usage: rush session send <id>");
        assert_eq!(
            send_args(&s(&["abc", "--nope"])).unwrap_err(),
            "flag provided but not defined: -nope"
        );
        assert_eq!(
            send_args(&s(&["abc", "--image"])).unwrap_err(),
            "flag needs an argument: -image"
        );
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
        fn path(&self) -> std::path::PathBuf {
            std::fs::canonicalize(self.0.path()).unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            rush_state::set_thread_dir(None);
            rush_state::set_thread_cache_dir(None);
        }
    }

    fn spawn_fake(reg: &Registry, cfg: Config) -> Result<Config, String> {
        fake::spawn(reg.clone(), cfg).map_err(|e| e.to_string())
    }

    fn ensure_fake(_: &Registry, id: &str) -> Result<(), String> {
        rush_host::read_info(id)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn run(args: &[&str], stdin: &str) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = session_cmd(&s(args), &mut stdin.as_bytes(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    // sessionSend against a host running in this process (never `rush
    // host run`, never a real agent): what's on stdin reaches the
    // session, and what can't be sent says why.
    #[test]
    fn sends_stdin_to_a_session() {
        let home = Home::new();
        let h = RushHosts::with(
            fake::registry(),
            Starts {
                spawn: spawn_fake,
                ensure: ensure_fake,
            },
        );
        let acct = home.path().join("acct");
        std::fs::create_dir_all(&acct).unwrap();
        let st = h
            .start(&Start {
                kind: "fake".into(),
                cwd: home.path().to_string_lossy().into_owned(),
                account: Some(("default".into(), acct.to_string_lossy().into_owned())),
                ..Start::default()
            })
            .expect("started");
        let (code, out, err) = run(&["send", &st.id], "your push failed\n\n");
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out, format!("sent to {}\n", st.id));
        let (code, _, err) = run(&["send", &st.id], " \n");
        assert_eq!(code, 1);
        assert_eq!(err, "rush: nothing to send: write the message on stdin\n");
        let (code, _, err) = run(&["send", "nosuch"], "hi");
        assert_eq!(code, 1);
        assert_eq!(err, "rush: session nosuch not found\n");
        let (code, out, _) = run(&["help"], "");
        assert_eq!(code, 0);
        assert!(out.starts_with("rush session: run rush-mode sessions without the view"));
        let _ = rush_host::client::Client::dial(&st.id).map(|c| c.stop());
    }
}
