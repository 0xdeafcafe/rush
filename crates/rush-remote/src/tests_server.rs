//! serve's HTTP surface, through its handler and on loopback only: who
//! gets in, what reaches a session's host, the hub's proxy, pairing,
//! devices and the streams. After Go's server_test.go, devices_test.go,
//! stream_test.go and fakehost_test.go.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_host::Info;
use rush_state::Time;
use serde_json::Value;

use crate::config::{Config, Peer, update_config};
use crate::ctx::Ctx;
use crate::http::{Body, Request, Response};
use crate::server::{Server, StartRequest};
use crate::testutil::home;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn server(peers: Vec<Peer>) -> Arc<Server> {
    update_config(|c| c.web = true).unwrap();
    let mut s = Server::new(
        Config {
            name: "hub".into(),
            peers,
            ..Config::default()
        },
        TOKEN.into(),
        rush_core::adapter::Registry::new(),
    );
    s.bad_guess = Duration::ZERO;
    Arc::new(s)
}

fn req(method: &str, path: &str, body: &str, hdr: &[(&str, &str)]) -> Request {
    let mut r = Request::new(method, path)
        .with_body(body)
        .with_header("Host", "rush.test");
    for (k, v) in hdr {
        r = r.with_header(k, v);
    }
    r
}

fn call(s: &Arc<Server>, method: &str, path: &str, body: &str, hdr: &[(&str, &str)]) -> Response {
    s.handler()(&req(method, path, body, hdr))
}

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

fn cookie(t: &str) -> String {
    format!("rush_token={t}")
}

#[test]
fn auth_origin_and_ops() {
    let _h = home();
    let s = server(vec![]);
    let b = bearer(TOKEN);
    let c = cookie(TOKEN);
    let json = ("Content-Type", "application/json");
    let login = format!(r#"{{"token":"{TOKEN}"}}"#);
    type Case<'a> = (
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        Vec<(&'a str, &'a str)>,
        u16,
    );
    let cases: Vec<Case> = vec![
        ("shell is public", "GET", "/", "", vec![], 200),
        ("api needs a token", "GET", "/api/sessions", "", vec![], 401),
        (
            "wrong bearer",
            "GET",
            "/api/sessions",
            "",
            vec![("Authorization", "Bearer nope")],
            401,
        ),
        (
            "bearer",
            "GET",
            "/api/sessions",
            "",
            vec![("Authorization", &b)],
            200,
        ),
        (
            "cookie",
            "GET",
            "/api/sessions",
            "",
            vec![("Cookie", &c), json],
            200,
        ),
        (
            "cookie post, same host",
            "POST",
            "/api/sessions/zz/op",
            r#"{"op":"stop"}"#,
            vec![("Cookie", &c), json, ("Origin", "http://rush.test")],
            404,
        ),
        (
            "cookie post, other origin",
            "POST",
            "/api/sessions/zz/op",
            r#"{"op":"stop"}"#,
            vec![("Cookie", &c), json, ("Origin", "https://evil.example")],
            403,
        ),
        (
            "cookie post, form",
            "POST",
            "/api/sessions/zz/op",
            "op=stop",
            vec![
                ("Cookie", &c),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            403,
        ),
        (
            "cross-site fetch without origin",
            "POST",
            "/api/sessions/zz/op",
            r#"{"op":"stop"}"#,
            vec![("Cookie", &c), json, ("Sec-Fetch-Site", "cross-site")],
            403,
        ),
        (
            "login from another origin",
            "POST",
            "/api/login",
            &login,
            vec![("Origin", "https://evil.example")],
            401,
        ),
        (
            "login",
            "POST",
            "/api/login",
            &login,
            vec![("Origin", "http://rush.test")],
            200,
        ),
        (
            "no such session",
            "GET",
            "/api/sessions/nope/events",
            "",
            vec![("Authorization", &b)],
            404,
        ),
        (
            "path trick in id",
            "GET",
            "/api/sessions/..%2f..%2fetc/events",
            "",
            vec![("Authorization", &b)],
            404,
        ),
        (
            "no such machine",
            "GET",
            "/m/ghost/api/sessions",
            "",
            vec![("Authorization", &b)],
            404,
        ),
        (
            "machine needs token",
            "GET",
            "/m/ghost/api/sessions",
            "",
            vec![],
            401,
        ),
    ];
    for (name, m, p, body, hdr, want) in cases {
        let r = call(&s, m, p, body, &hdr);
        assert_eq!(
            r.status,
            want,
            "{name}: {}",
            String::from_utf8_lossy(r.bytes())
        );
        assert_eq!(r.get("X-Content-Type-Options"), "nosniff", "{name}");
        assert!(
            r.get("Content-Security-Policy")
                .contains("frame-ancestors 'none'")
        );
    }
}

/// Serves h on loopback; its address.
fn on_loopback(h: crate::http::Handler) -> (String, Ctx) {
    let ln = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", ln.local_addr().unwrap());
    let ctx = Ctx::new();
    let c = ctx.clone();
    std::thread::spawn(move || crate::http::serve(&c, ln, h));
    (url, ctx)
}

#[test]
fn hub_proxies_with_the_peers_token() {
    let _h = home();
    let peer_tok = "b".repeat(64);
    let mut peer = Server::new(
        Config::default(),
        peer_tok.clone(),
        rush_core::adapter::Registry::new(),
    );
    peer.bad_guess = Duration::ZERO;
    let ph = Arc::new(peer).handler();
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let s2 = seen.clone();
    let (url, stop) = on_loopback(Arc::new(move |r: &Request| {
        s2.lock()
            .unwrap()
            .push((r.header("authorization").into(), r.header("cookie").into()));
        ph(r)
    }));
    let hub = server(vec![Peer {
        name: "box".into(),
        url,
        token: peer_tok.clone(),
    }]);
    let r = call(
        &hub,
        "GET",
        "/m/box/api/sessions",
        "",
        &[("Cookie", &cookie(TOKEN))],
    );
    let mut body = Vec::new();
    if let Body::Stream(f) = r.body {
        f(&mut body);
    }
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(String::from_utf8_lossy(&body), "[]\n");
    let got = seen.lock().unwrap()[0].clone();
    assert_eq!(got, (bearer(&peer_tok), String::new()));
    // Only /api/ goes through, and only to a named peer.
    assert_eq!(
        call(
            &hub,
            "GET",
            "/m/box/",
            "",
            &[("Authorization", &bearer(TOKEN))]
        )
        .status,
        404
    );
    // The peer's token is no use at the hub.
    assert_eq!(
        call(
            &hub,
            "GET",
            "/api/sessions",
            "",
            &[("Authorization", &bearer(&peer_tok))]
        )
        .status,
        401
    );
    stop.cancel();
}

#[test]
fn remote_ops_allowlist() {
    let _h = home();
    let s = server(vec![]);
    std::fs::create_dir_all(rush_host::root().join("zz")).unwrap();
    for op in [
        "rewind",
        "restart",
        "ask",
        "relogin",
        "retitle",
        "compacted",
    ] {
        let r = call(
            &s,
            "POST",
            "/api/sessions/zz/op",
            &format!(r#"{{"op":"{op}"}}"#),
            &[
                ("Authorization", &bearer(TOKEN)),
                ("Content-Type", "application/json"),
            ],
        );
        assert_eq!(r.status, 403, "{op}");
    }
}

#[test]
fn allowed_cwd() {
    let _h = home();
    let s = server(vec![]);
    let ws = tempfile::tempdir().unwrap();
    let ws_s = ws.path().to_string_lossy().into_owned();
    assert!(
        s.allowed_cwd(&crate::config::dir().join("logins").to_string_lossy())
            .is_err()
    );
    assert!(s.allowed_cwd(&ws_s).is_ok());
    let mut c = s.config.clone();
    c.workspaces = vec![ws_s.clone()];
    let s = Server::new(c, TOKEN.into(), rush_core::adapter::Registry::new());
    let sub = ws.path().join("p");
    std::fs::create_dir_all(&sub).unwrap();
    assert!(s.allowed_cwd(&sub.to_string_lossy()).is_ok());
    assert!(s.allowed_cwd(&format!("{ws_s}/..")).is_err());
    let out = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(out.path(), ws.path().join("link")).unwrap();
    assert!(
        s.allowed_cwd(&ws.path().join("link").to_string_lossy())
            .is_err()
    );
}

/// A session's host stands in: alive (this process), answering on the
/// session's socket with its info, and handing over every op it's sent.
struct FakeHost {
    info: Info,
    ops: Receiver<Vec<u8>>,
}

fn fake_host(id: &str, name: &str, cwd: &str) -> FakeHost {
    let dir = rush_host::dir(id);
    std::fs::create_dir_all(&dir).unwrap();
    let info = Info {
        id: id.into(),
        session_id: format!("{id}-0000"),
        name: name.into(),
        cwd: cwd.into(),
        host_pid: std::process::id() as i64,
        state: "idle".into(),
        proto: rush_host::PROTO,
        started_at: Time::now(),
        updated_at: Time::now(),
        ..Info::default()
    };
    std::fs::write(dir.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
    let ln = UnixListener::bind(rush_host::sock_path(id)).unwrap();
    let (tx, rx) = channel();
    let i2 = info.clone();
    std::thread::spawn(move || {
        for c in ln.incoming() {
            let Ok(mut c) = c else { return };
            let mut l = br#"{"info":"#.to_vec();
            l.extend(serde_json::to_vec(&i2).unwrap());
            l.extend_from_slice(b",\"type\":\"agtop_info\"}\n");
            let _ = c.write_all(&l);
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(c).split(b'\n').map_while(Result::ok) {
                    let v: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
                    if v["op"] != "hello" {
                        let _ = tx.send(line);
                    }
                }
            });
        }
    });
    FakeHost { info, ops: rx }
}

impl FakeHost {
    fn op(&self) -> Value {
        let l = self
            .ops
            .recv_timeout(Duration::from_secs(5))
            .expect("no op reached the host");
        serde_json::from_slice(&l).unwrap()
    }
}

/// Every control the web app has, sent as the browser would: what reaches
/// the host is what the app asked for, and nothing carries a path.
#[test]
fn web_controls_reach_the_host() {
    let _h = home();
    let f = fake_host("feedbeef", "job", "/work");
    let started: Arc<Mutex<Option<StartRequest>>> = Arc::default();
    let st = started.clone();
    let info = f.info.clone();
    let mut s = Server::new(
        Config {
            name: "box".into(),
            ..Config::default()
        },
        TOKEN.into(),
        rush_core::adapter::Registry::new(),
    );
    s.start = Some(Arc::new(move |r: &StartRequest| {
        *st.lock().unwrap() = Some(r.clone());
        Ok(info.clone())
    }));
    let s = Arc::new(s);
    let c = cookie(TOKEN);
    let hdr = [
        ("Cookie", c.as_str()),
        ("Content-Type", "application/json"),
        ("Origin", "http://rush.test"),
    ];
    let post = |p: &str, b: &str| call(&s, "POST", p, b, &hdr);
    let r = post(
        "/api/sessions",
        r#"{"cwd":"/work","agent":"claude","prompt":"go","name":"n"}"#,
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(r.bytes()));
    let got = started.lock().unwrap().clone().unwrap();
    assert_eq!(
        (
            got.cwd.as_str(),
            got.agent.as_str(),
            got.prompt.as_str(),
            got.name.as_str()
        ),
        ("/work", "claude", "go", "n")
    );
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "send",
            r#"{"op":"send","text":"hello"}"#,
            serde_json::json!({"op":"send","text":"hello"}),
        ),
        (
            "steer",
            r#"{"op":"send","text":"turn left","guide":true}"#,
            serde_json::json!({"op":"send","text":"turn left","guide":true}),
        ),
        (
            "stop the turn",
            r#"{"op":"interrupt"}"#,
            serde_json::json!({"op":"interrupt"}),
        ),
        (
            "approve",
            r#"{"op":"allow","id":"ap1"}"#,
            serde_json::json!({"op":"allow","id":"ap1"}),
        ),
        (
            "approve always",
            r#"{"op":"allow","id":"ap1","always":true}"#,
            serde_json::json!({"op":"allow","id":"ap1","always":true}),
        ),
        (
            "deny",
            r#"{"op":"deny","id":"ap1","message":"no"}"#,
            serde_json::json!({"op":"deny","id":"ap1","message":"no"}),
        ),
        (
            "stop a task",
            r#"{"op":"stop_task","id":"t1"}"#,
            serde_json::json!({"op":"stop_task","id":"t1"}),
        ),
        (
            "queue send",
            r#"{"op":"queue_send","index":1,"was":"next"}"#,
            serde_json::json!({"op":"queue_send","index":1,"was":"next"}),
        ),
        (
            "continue at the reset",
            r#"{"op":"limit","now":true}"#,
            serde_json::json!({"op":"limit","now":true}),
        ),
        (
            "tell a subagent",
            r#"{"op":"tell","id":"sub1","text":"hurry"}"#,
            serde_json::json!({"op":"tell","id":"sub1","text":"hurry"}),
        ),
    ];
    for (name, body, want) in cases {
        let r = post("/api/sessions/feedbeef/op", body);
        assert_eq!(
            r.status,
            202,
            "{name}: {}",
            String::from_utf8_lossy(r.bytes())
        );
        let got = f.op();
        for (k, v) in want.as_object().unwrap() {
            assert_eq!(&got[k], v, "{name}: {got}");
        }
    }
    // The answer's input arrives whole.
    post(
        "/api/sessions/feedbeef/op",
        r#"{"op":"allow","id":"q1","input":{"answers":{"Pick one":"B"}}}"#,
    );
    assert_eq!(f.op()["input"]["answers"]["Pick one"], "B");
    // A browser can't attach files of this machine, or sneak an exchange.
    post(
        "/api/sessions/feedbeef/op",
        r#"{"op":"send","text":"x","images":["/etc/passwd"],"exchange":{"id":"z"}}"#,
    );
    let got = f.op();
    assert!(
        got.get("images").is_none() && got.get("exchange").is_none(),
        "{got}"
    );
    assert_eq!(
        post(
            "/api/sessions/feedbeef/op",
            r#"{"op":"away","away":{"every":1000}}"#
        )
        .status,
        400
    );
    // An image arrives as bytes and reaches the host as a file in the
    // session's scratch folder.
    post(
        "/api/sessions/feedbeef/op",
        r#"{"op":"send","text":"see","pictures":[{"type":"image/png","data":"aGk="}]}"#,
    );
    let got = f.op();
    let p = got["images"][0].as_str().unwrap().to_string();
    assert!(
        p.starts_with(&*rush_host::tmp::temp_dir("feedbeef").to_string_lossy())
            && p.ends_with(".png"),
        "{p}"
    );
    assert_eq!(std::fs::read(&p).unwrap(), b"hi");
    for bad in [
        r#"{"op":"send","text":"x","pictures":[{"type":"text/html","data":"aGk="}]}"#,
        r#"{"op":"send","text":"x","pictures":[{"type":"image/png","data":""}]}"#,
        r#"{"op":"allow","id":"ap1","pictures":[{"type":"image/png","data":"aGk="}]}"#,
        &format!(
            r#"{{"op":"send","pictures":[{}{{"type":"image/png","data":"aGk="}}]}}"#,
            r#"{"type":"image/png","data":"aGk="},"#.repeat(8)
        ),
    ] {
        assert_eq!(post("/api/sessions/feedbeef/op", bad).status, 400, "{bad}");
    }
    assert_eq!(
        post("/api/sessions/feedbeef/op", r#"{"op":"rewind"}"#).status,
        403
    );
    assert!(f.ops.try_recv().is_err(), "an op reached the host");
}

#[test]
fn a_pairing_code_signs_in_once() {
    let _h = home();
    let mut c = Config {
        name: "hub".into(),
        origin: "https://rush.test".into(),
        ..Config::default()
    };
    c.web = true;
    let mut s = Server::new(c, TOKEN.into(), rush_core::adapter::Registry::new());
    s.bad_guess = Duration::ZERO;
    let s = Arc::new(s);
    let json = ("Content-Type", "application/json");
    assert_eq!(call(&s, "POST", "/api/pair", "{}", &[json]).status, 401);
    let r = call(
        &s,
        "POST",
        "/api/pair",
        "{}",
        &[("Authorization", &bearer(TOKEN)), json],
    );
    let p: crate::pair::Pairing = serde_json::from_slice(r.bytes()).unwrap();
    assert_eq!(p.url, format!("https://rush.test/#pair={}", p.code));
    assert!(!String::from_utf8_lossy(r.bytes()).contains(TOKEN));
    let login = |code: &str| {
        call(
            &s,
            "POST",
            "/api/login",
            &format!(r#"{{"code":"{code}"}}"#),
            &[("Origin", "https://rush.test"), json],
        )
    };
    let r = login(&p.code);
    assert_eq!(r.status, 200);
    assert!(r.get("Set-Cookie").starts_with("rush_token="));
    assert!(
        r.get("Set-Cookie")
            .contains("; Path=/; Max-Age=34560000; HttpOnly; SameSite=Strict")
    );
    assert_eq!(login(&p.code).status, 401, "the code worked twice");
}

#[test]
fn the_web_app_follows_its_switch() {
    let _h = home();
    let s = server(vec![]);
    update_config(|c| c.web = false).unwrap();
    let r = call(&s, "GET", "/", "", &[]);
    assert_eq!(r.status, 404);
    assert_eq!(
        String::from_utf8_lossy(r.bytes()),
        "The web app is off on this machine: Settings › Remote in rush turns it on.\n"
    );
    assert_eq!(
        call(
            &s,
            "GET",
            "/api/sessions",
            "",
            &[("Authorization", &bearer(TOKEN))]
        )
        .status,
        200
    );
    update_config(|c| c.web = true).unwrap();
    let r = call(&s, "GET", "/", "", &[]);
    assert_eq!(r.get("Content-Type"), "text/html; charset=utf-8");
    assert_eq!(r.get("Cache-Control"), "no-cache");
    assert!(String::from_utf8_lossy(r.bytes()).contains("<html"));
    assert_eq!(call(&s, "GET", "/nope.js", "", &[]).status, 404);
}

/// Each sign-in is a device of its own; revoking one locks only it out. A
/// cookie from before devices becomes one at the app's first request.
#[test]
fn devices_sign_in_and_revoke() {
    let _h = home();
    let s = server(vec![]);
    let sign_in = |ua: &str| -> String {
        let r = call(
            &s,
            "POST",
            "/api/login",
            &format!(r#"{{"token":"{TOKEN}"}}"#),
            &[
                ("Origin", "http://rush.test"),
                ("Content-Type", "application/json"),
                ("User-Agent", ua),
            ],
        );
        assert_eq!(r.status, 200);
        let set = r.get("Set-Cookie");
        let v = set
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("rush_token=")
            .unwrap()
            .to_string();
        assert_ne!(v, TOKEN);
        v
    };
    let phone = sign_in(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) Version/19.0 Mobile/15E148 Safari/604.1",
    );
    let laptop =
        sign_in("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/140.0 Safari/537.36");
    assert_eq!(
        call(
            &s,
            "GET",
            "/api/sessions",
            "",
            &[("Cookie", &cookie(&phone))]
        )
        .status,
        200
    );
    let b = bearer(TOKEN);
    let r = call(&s, "GET", "/api/devices", "", &[("Authorization", &b)]);
    let body = String::from_utf8_lossy(r.bytes()).into_owned();
    assert!(!body.contains("tokenHash"), "{body}");
    let list: Vec<crate::devices::Device> = serde_json::from_str(&body).unwrap();
    assert_eq!(list.len(), 2);
    let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names.contains(&"iPhone · Safari") && names.contains(&"Mac · Chrome"),
        "{names:?}"
    );
    let id = list
        .iter()
        .find(|d| d.name.starts_with("iPhone"))
        .unwrap()
        .id
        .clone();
    let r = call(
        &s,
        "POST",
        &format!("/api/devices/{id}/revoke"),
        "{}",
        &[("Authorization", &b), ("Content-Type", "application/json")],
    );
    assert_eq!(r.status, 200);
    assert_eq!(
        call(
            &s,
            "GET",
            "/api/sessions",
            "",
            &[("Cookie", &cookie(&phone))]
        )
        .status,
        401
    );
    assert_eq!(
        call(
            &s,
            "GET",
            "/api/sessions",
            "",
            &[("Cookie", &cookie(&laptop))]
        )
        .status,
        200
    );
    let r = call(
        &s,
        "GET",
        "/api/machines",
        "",
        &[("Cookie", &cookie(TOKEN))],
    );
    assert_eq!(r.status, 200);
    assert!(r.get("Set-Cookie").starts_with("rush_token="));
    assert!(!r.get("Set-Cookie").contains(TOKEN));
    assert_eq!(s.devs.all().len(), 2);
    assert_eq!(
        String::from_utf8_lossy(r.bytes()),
        "[{\"name\":\"hub\",\"self\":true}]\n"
    );
}

/// A writer that ends its stream once it holds want events.
struct Until {
    out: Arc<Mutex<Vec<u8>>>,
    want: usize,
}

impl Write for Until {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let mut o = self.out.lock().unwrap();
        if String::from_utf8_lossy(&o).matches("data: ").count() >= self.want {
            return Err(std::io::Error::other("enough"));
        }
        o.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn stream(r: Response, want: usize) -> String {
    let out: Arc<Mutex<Vec<u8>>> = Arc::default();
    if let Body::Stream(f) = r.body {
        f(&mut Until {
            out: out.clone(),
            want,
        });
    }
    String::from_utf8_lossy(&out.lock().unwrap()).into_owned()
}

/// A session whose host isn't running gives its info, and the stream ends
/// when the client goes.
#[test]
fn events_of_a_stopped_session() {
    let _h = home();
    let s = server(vec![]);
    let dir = rush_host::dir("d4d4d4d4");
    std::fs::create_dir_all(&dir).unwrap();
    let info = Info {
        id: "d4d4d4d4".into(),
        name: "past".into(),
        state: "stopped".into(),
        host_pid: 0,
        updated_at: Time::now(),
        ..Info::default()
    };
    std::fs::write(dir.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
    let r = call(
        &s,
        "GET",
        "/api/sessions/d4d4d4d4/events",
        "",
        &[("Authorization", &bearer(TOKEN))],
    );
    assert_eq!(r.get("Content-Type"), "text/event-stream");
    // The writer refuses the keepalive after these two, which ends it.
    let got = stream(r, 2);
    assert!(
        got.starts_with("data: {\"e\":null,\"t\":\"reset\"}\n\n"),
        "{got}"
    );
    assert!(got.contains("\"t\":\"info\""), "{got}");
    assert!(got.contains("\"name\":\"past\""), "{got}");
}

#[test]
fn the_sessions_stream_starts_with_the_list() {
    let _h = home();
    let s = server(vec![]);
    let dir = rush_host::dir("c3c3c3c3");
    std::fs::create_dir_all(&dir).unwrap();
    let info = Info {
        id: "c3c3c3c3".into(),
        name: "first".into(),
        state: "idle".into(),
        updated_at: Time::now(),
        ..Info::default()
    };
    std::fs::write(dir.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
    let r = call(
        &s,
        "GET",
        "/api/sessions/stream",
        "",
        &[("Authorization", &bearer(TOKEN))],
    );
    assert_eq!(r.get("Content-Type"), "text/event-stream");
    let got = stream(r, 1);
    assert!(got.starts_with("data: {\"sessions\":["), "{got}");
    assert!(got.contains("\"name\":\"first\""), "{got}");
    assert!(got.contains("\"alive\":false"), "{got}");
}

#[test]
fn a_hosts_lines_as_the_web_app_reads_them() {
    use crate::server::wire_line;
    assert_eq!(
        wire_line(br#"{"type":"agtop_answered","request_id":"r1"}"#).unwrap(),
        br#"{"e":{"ID":"r1"},"t":"answered"}"#
    );
    assert_eq!(
        wire_line(br#"{"type":"agtop_error","error":"no"}"#).unwrap(),
        br#"{"e":{"Error":"no"},"t":"error"}"#
    );
    assert!(wire_line(b"rubbish").is_none());
}

/// attachPeers on attaches each peer, with no terminal; off ends it.
#[test]
fn attach_peers_follows_its_switch() {
    let _h = home();
    let (url, stop) = on_loopback(Arc::new(|_: &Request| Response::new(200, "[]")));
    update_config(|c| c.attach_peers = true).unwrap();
    let ctx = Ctx::new();
    let c2 = ctx.clone();
    let cfg = Config {
        peers: vec![Peer {
            name: "box".into(),
            url,
            token: "t".into(),
        }],
        ..Config::default()
    };
    let log: crate::expose::Log = Arc::new(Mutex::new(Vec::<u8>::new()));
    let t = std::thread::spawn(move || crate::attach::attach_peers(&c2, &cfg, &log));
    let there = || crate::git::unreachable("box").exists();
    until("box attached", there);
    update_config(|c| c.attach_peers = false).unwrap();
    until("box let go", || !there());
    ctx.cancel();
    t.join().unwrap();
    stop.cancel();
}

/// Waits for ok, at most 15s: what it waits on is a thread of its own.
fn until(what: &str, ok: impl Fn() -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < end {
        if ok() {
            return;
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
    panic!("timed out: {what}");
}

// ── history pages ──
use rush_core::adapter::{
    Adapter, Features, HistoryReader, HistoryWindow, Kind, Level, Profile, Session,
    TranscriptLocator,
};
use rush_core::event::{Event, Message, Part, PartKind};

/// An agent whose history pages: a transcript of one-word lines, each
/// line one message event at the byte it starts (a page's cursor), its
/// transcript where its profile's dir keeps <session>.log.
struct Paged;

impl Adapter for Paged {
    fn kind(&self) -> Kind {
        Kind::from("paged")
    }
    fn name(&self) -> String {
        "Paged".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(history_window: HistoryWindow, transcript_locator: TranscriptLocator);
}

impl TranscriptLocator for Paged {
    fn transcript_path(&self, p: &Profile, _: &str, sid: &str) -> String {
        std::path::Path::new(&p.dir)
            .join(format!("{sid}.log"))
            .to_string_lossy()
            .into_owned()
    }
}

fn line_message(line: &[u8]) -> Event {
    Event::Message(Message {
        role: "user".into(),
        parts: vec![Part {
            kind: PartKind::Text,
            text: String::from_utf8_lossy(line.trim_ascii()).into_owned(),
            ..Part::default()
        }],
        ..Message::default()
    })
}

impl HistoryWindow for Paged {
    fn history_window(
        &self,
        s: &Session,
        from: u64,
        end: u64,
    ) -> rush_core::adapter::Result<Vec<(u64, Vec<Event>)>> {
        let b = std::fs::read(&s.transcript).unwrap_or_default();
        let mut out = Vec::new();
        let mut at = 0u64;
        for line in b.split_inclusive(|&c| c == b'\n') {
            let next = at + line.len() as u64;
            if at >= from && next <= end && !line.trim_ascii().is_empty() {
                out.push((at, vec![line_message(line)]));
            }
            at = next;
        }
        Ok(out)
    }
}

/// The same agent without windows, paging by count through its whole
/// history.
struct Plain;

impl Adapter for Plain {
    fn kind(&self) -> Kind {
        Kind::from("plain")
    }
    fn name(&self) -> String {
        "Plain".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(history_reader: HistoryReader, transcript_locator: TranscriptLocator);
}

impl TranscriptLocator for Plain {
    fn transcript_path(&self, p: &Profile, _: &str, sid: &str) -> String {
        std::path::Path::new(&p.dir)
            .join(format!("{sid}.log"))
            .to_string_lossy()
            .into_owned()
    }
}

impl HistoryReader for Plain {
    fn history(
        &self,
        s: &Session,
        _: i64,
        _: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> rush_core::adapter::Result<()> {
        let b = std::fs::read(&s.transcript).unwrap_or_default();
        let evs: Vec<Event> = b
            .split_inclusive(|&c| c == b'\n')
            .filter(|l| !l.trim_ascii().is_empty())
            .map(line_message)
            .collect();
        if !evs.is_empty() {
            each(evs);
        }
        Ok(())
    }
}

/// One stopped session of kind with a transcript of n lines in its
/// profile's dir, and the server that knows the agents.
fn paged_server(id: &str, kind: &str, lines: usize) -> Arc<Server> {
    let reg = rush_core::adapter::Registry::new();
    reg.register(Paged);
    reg.register(Plain);
    let mut s = Server::new(Config::default(), TOKEN.into(), reg);
    s.bad_guess = Duration::ZERO;
    let s = Arc::new(s);
    let dir = rush_host::dir(id);
    let pro = dir.join("pro");
    std::fs::create_dir_all(&pro).unwrap();
    let body: String = (0..lines).map(|i| format!("line {i:03}\n")).collect();
    std::fs::write(pro.join("s1.log"), &body).unwrap();
    let cfg = format!(
        r#"{{"id":"{id}","kind":"{kind}","sessionId":"s1","account":{{"name":"","configDir":{}}}}}"#,
        serde_json::to_string(pro.to_str().unwrap()).unwrap()
    );
    std::fs::write(dir.join("config.json"), cfg).unwrap();
    let info = Info {
        id: id.into(),
        kind: kind.into(),
        session_id: "s1".into(),
        state: "stopped".into(),
        host_pid: 0,
        updated_at: Time::now(),
        ..Info::default()
    };
    std::fs::write(dir.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
    s
}

/// The answer of one page as JSON.
fn page_json(r: Response) -> Value {
    assert_eq!(r.status, 200, "{}", r.status);
    match r.body {
        crate::http::Body::Bytes(b) => serde_json::from_slice(&b).unwrap(),
        _ => panic!("not bytes"),
    }
}

fn texts(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["e"]["Parts"][0]["Text"].as_str().unwrap().to_string())
        .collect()
}

/// History pages back: the newest page first, each cursor naming the
/// next, until the start — never the whole history at once — with
/// appends leaving a page put and a rewrite answering reset.
#[test]
fn history_pages_back_before_a_cursor() {
    let _h = home();
    let s = paged_server("e5e5e5e5", "paged", 90);
    let get = |q: &str| {
        call(
            &s,
            "GET",
            &format!("/api/sessions/e5e5e5e5/events/before{q}"),
            "",
            &[("Authorization", &bearer(TOKEN))],
        )
    };

    // the token still counts
    let r = call(&s, "GET", "/api/sessions/e5e5e5e5/events/before", "", &[]);
    assert_eq!(r.status, 401);

    // paging to the start, thirty a page
    let (mut cur, mut seen, mut pages) = (None::<String>, Vec::new(), 0);
    loop {
        let q = match &cur {
            Some(c) => format!("?limit=30&cursor={c}"),
            None => "?limit=30".into(),
        };
        let v = page_json(get(&q));
        let got = texts(&v);
        assert!(!got.is_empty(), "an empty page before the start");
        assert!(got.len() <= 30, "a page over its limit");
        pages += 1;
        seen.extend(got);
        let more = v["more"].as_bool().unwrap();
        cur = v["cursor"].as_str().map(String::from);
        if !more {
            break;
        }
        assert!(cur.is_some(), "more without a cursor");
    }
    assert_eq!(pages, 3, "{pages} pages for 90 lines");
    let want: Vec<String> = (0..90).map(|i| format!("line {i:03}")).collect();
    let mut all = seen.clone();
    all.reverse();
    assert_eq!(all, want, "the pages in order");

    // a page asked for again after the transcript grew: the same answer
    let first = {
        let v = page_json(get("?limit=30"));
        v["cursor"].as_str().unwrap().to_string()
    };
    let pro = rush_host::dir("e5e5e5e5").join("pro");
    let mut body = std::fs::read_to_string(pro.join("s1.log")).unwrap();
    let before = page_json(get(&format!("?limit=30&cursor={first}")));
    body.push_str("line 090\nline 091\n");
    std::fs::write(pro.join("s1.log"), &body).unwrap();
    let after = page_json(get(&format!("?limit=30&cursor={first}")));
    assert_eq!(texts(&before), texts(&after), "appends moved a page");

    // a rewrite shorter than where paging began: reset, for the app to
    // start its stream over
    std::fs::write(pro.join("s1.log"), "only one\n").unwrap();
    let v = page_json(get(&format!("?limit=30&cursor={first}")));
    assert_eq!(v["reset"], true, "{v}");

    // a bad cursor is refused
    for bad in ["nonsense", "9-10", "-1", "10-"] {
        let r = get(&format!("?cursor={bad}"));
        assert_eq!(r.status, 400, "{bad}");
    }

    // a session that isn't: 404
    let r = call(
        &s,
        "GET",
        "/api/sessions/nope/events/before",
        "",
        &[("Authorization", &bearer(TOKEN))],
    );
    assert_eq!(r.status, 404);
}

/// An agent without windows pages by count: the same walk, a number for
/// a cursor.
#[test]
fn an_agent_without_windows_pages_by_count() {
    let _h = home();
    let s = paged_server("f6f6f6f6", "plain", 20);
    let get = |q: &str| {
        call(
            &s,
            "GET",
            &format!("/api/sessions/f6f6f6f6/events/before{q}"),
            "",
            &[("Authorization", &bearer(TOKEN))],
        )
    };
    let v = page_json(get("?limit=6"));
    let got = texts(&v);
    assert_eq!(
        (got.first().unwrap(), got.last().unwrap(), got.len()),
        (&"line 014".to_string(), &"line 019".to_string(), 6)
    );
    assert_eq!(v["more"], true);
    let cur = v["cursor"].as_str().unwrap().to_string();
    assert!(cur.chars().all(|c| c.is_ascii_digit()), "{cur}");
    let v = page_json(get(&format!("?limit=6&cursor={cur}")));
    let got = texts(&v);
    assert_eq!(
        (got.first().unwrap(), got.last().unwrap(), got.len()),
        (&"line 008".to_string(), &"line 013".to_string(), 6)
    );
    let cur = v["cursor"].as_str().unwrap().to_string();
    let v = page_json(get(&format!("?limit=100&cursor={cur}")));
    assert_eq!((texts(&v).len(), v["more"].as_bool().unwrap()), (8, false));
    assert_eq!(texts(&v).first().unwrap(), "line 000");
}
