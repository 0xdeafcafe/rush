//! `rush serve`, `rush remote …` and `rush session --on NAME …`: this
//! machine's sessions to the web app and to rush elsewhere, the other
//! machines this one shows, and the session commands run on one of them.
//! After Go's cmd/rush/serve.go (remoteUsage :23, serveCmd :38,
//! remoteCmd :74, remoteSessionCmd :129, remoteWatch :287) and
//! session.go's startSession (:269) as serve starts sessions with it.

use std::io::{BufRead, BufReader, Read, Write};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_core::adapter::Registry;
use rush_remote::others::Other;
use rush_remote::{Ctx, Peer, Server, Session, StartRequest, load_config, save_config, token};

use crate::flags::Flags;
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
    srv.start = Some(Arc::new(move |q: &StartRequest| {
        crate::session::start_requested(&r, q)
    }));
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
    let srv = Arc::new(srv);
    let sshd: rush_remote::expose::Up = {
        let (s, log) = (srv.clone(), log.clone());
        Arc::new(move |c: &Ctx| s.serve_ssh(c, &log))
    };
    srv.run(&ctx, ln, log, Some(sshd))
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
        return Err(crate::session::USAGE.into());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
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
