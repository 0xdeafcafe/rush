//! rush serve: an HTTP server in front of this machine's session hosts,
//! for the web app and for rush on other machines. A serve with peers is a
//! hub, proxying to theirs.
//!
//! After Go's internal/remote/server.go (Server :35, StartRequest :52,
//! Session :67, Machine :73, Handler :81, secure :108, webOn :120, noCache
//! :132, tokenOK :139, auth :147, sameOrigin :190, login :203, signIn
//! :222, logout :234, isHTTPS :241, machines :245, list :253, start :262,
//! allowedCwd :309, validID :336, remoteOps :348, remoteOp :354, Picture
//! :380, savePictures :398, op :428, proxy :489, events :524, sse :587,
//! pump :602, wireLine :631, sendHistory :677, readJSONMost :719, clip
//! :731, writeJSON :738, fail :744, Run :754), pair.go's handler and
//! reachedAt (:100, :120), devices.go's handlers (:200, :209) and watch.go's
//! push handlers (:15, :23).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use rush_core::adapter::{Profile, Registry, Session as AgentSession, migrated};
use rush_core::event::{self, Event};
use rush_core::wire::RawJson;
use rush_host::{Away, Info};
use rush_state::Time;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{Config, current, hex, rand_bytes};
use crate::ctx::Ctx;
use crate::devices::{Device, Devices, LiveConns, device_name, same};
use crate::expose::{Log, Up, expose, fields, say};
use crate::http::{self, Request, Response};
use crate::pair::{PairCodes, Pairing};
use crate::push::{Notice, Push, Subscription};

/// A new session as a remote client asks for it: what the web app's form
/// offers, and nothing that reaches past the agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartRequest {
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The session id of a past session rush didn't start: it carries on
    /// in rush, with its own agent and folder.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resume: String,
}

/// A session as serve lists it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(flatten)]
    pub info: Info,
    #[serde(default)]
    pub alive: bool,
}

/// One machine the web app shows: this one, or a peer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    #[serde(rename = "self", default, skip_serializing_if = "std::ops::Not::not")]
    pub me: bool,
}

/// An error as serve answers it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorBody {
    #[serde(alias = "Error")]
    pub error: String,
}

/// Starts a session as rush session start does.
pub type StartFn = Arc<dyn Fn(&StartRequest) -> Result<Info, String> + Send + Sync>;

const COOKIE: &str = "rush_token";

/// One machine's rush serve.
pub struct Server {
    pub config: Config,
    pub token: String,
    /// Starts a session as rush session start does.
    pub start: Option<StartFn>,
    /// Sends notifications, when set up.
    pub push: Option<Arc<Push>>,
    /// The agents, for reading a session's history and the sessions rush
    /// didn't start.
    pub reg: Registry,
    /// The pause after a bad sign-in (tests: none).
    pub bad_guess: Duration,
    pub(crate) pairs: PairCodes,
    pub(crate) devs: Devices,
    pub(crate) live: LiveConns,
    /// Each SSH device's rush.
    pub(crate) tuis: crate::sshd::Tuis,
}

impl Server {
    pub fn new(config: Config, token: String, reg: Registry) -> Server {
        Server {
            config,
            token,
            start: None,
            push: None,
            reg,
            bad_guess: Duration::from_secs(1),
            pairs: PairCodes::default(),
            devs: Devices::default(),
            live: LiveConns::default(),
            tuis: crate::sshd::Tuis::default(),
        }
    }

    /// Serve's whole HTTP surface, as one handler.
    pub fn handler(self: &Arc<Self>) -> http::Handler {
        let s = self.clone();
        Arc::new(move |r: &Request| {
            let mut resp = s.route(r);
            secure(&mut resp);
            resp
        })
    }

    fn route(&self, r: &Request) -> Response {
        let p = r.path.as_str();
        if p == "/api/login" && r.method == "POST" {
            return self.login(r);
        }
        if p == "/api/logout" && r.method == "POST" {
            return logout(r);
        }
        if p.starts_with("/api/") || p.starts_with("/m/") {
            return self.auth(r);
        }
        if !current().web {
            return text(
                404,
                "The web app is off on this machine: Settings › Remote in rush turns it on.\n",
            );
        }
        let mut resp = crate::web::serve(r);
        // FileServer drops it from an error.
        if resp.status < 400 {
            resp.set("Cache-Control", "no-cache");
        }
        resp
    }

    fn token_ok(&self, t: &str) -> bool {
        !t.is_empty() && same(t.as_bytes(), self.token.as_bytes())
    }

    /// Lets through a request with the token, as a bearer (rush on another
    /// machine, a hub) or the browser's cookie. A browser's request that
    /// changes anything must be JSON from this origin.
    fn auth(&self, r: &Request) -> Response {
        if let Some(b) = r.header("authorization").strip_prefix("Bearer ") {
            if !self.token_ok(b) {
                return fail(401, "bad token");
            }
            return self.api(r, None);
        }
        let Some(c) = r.cookie(COOKIE) else {
            return fail(401, "sign in");
        };
        if r.method != "GET"
            && r.method != "HEAD"
            && (!self.same_origin(r) || !r.header("content-type").starts_with("application/json"))
        {
            return fail(403, "cross-origin request refused");
        }
        let mut cookie = None;
        let mut dev = self.devs.web(&c);
        if dev.is_none() && self.token_ok(&c) {
            // A cookie from before devices becomes one, so it can be
            // revoked: at the app's first request, which it makes alone.
            dev = Some(Device::default());
            if r.path == "/api/machines" {
                match self.sign_in(r) {
                    Ok((d, set)) => {
                        dev = Some(d);
                        cookie = Some(set);
                    }
                    Err(e) => return fail(500, &e),
                }
            }
        }
        let Some(dev) = dev else {
            return fail(401, "sign in");
        };
        let _live = self.live.add(&dev.id, r.ctx.clone());
        let mut resp = self.api(r, Some(c));
        if let Some(set) = cookie {
            resp.add("Set-Cookie", &set);
        }
        resp
    }

    /// The API behind auth.
    fn api(&self, r: &Request, cookie: Option<String>) -> Response {
        let segs: Vec<&str> = r.path.trim_start_matches('/').split('/').collect();
        let m = r.method.as_str();
        let get = m == "GET" || m == "HEAD";
        let allow = |ok: bool, a: &str| -> Option<Response> {
            if ok {
                None
            } else {
                let mut x = text(405, "Method Not Allowed\n");
                x.set("Allow", a);
                Some(x)
            }
        };
        if segs.first() == Some(&"m") && segs.len() >= 3 {
            return self.proxy(r, segs[1]);
        }
        match segs.as_slice() {
            ["api", "machines"] => allow(get, "GET, HEAD").unwrap_or_else(|| self.machines()),
            ["api", "sessions"] => match m {
                "GET" | "HEAD" => self.list(),
                "POST" => self.start_session(r),
                _ => allow(false, "GET, HEAD, POST").unwrap_or_else(|| fail(405, "")),
            },
            ["api", "sessions", "stream"] => {
                allow(get, "GET, HEAD").unwrap_or_else(|| self.sessions_stream(r, cookie))
            }
            ["api", "sessions", id, "events"] if !id.is_empty() => {
                allow(get, "GET, HEAD").unwrap_or_else(|| self.events(r, id))
            }
            ["api", "sessions", id, "op"] if !id.is_empty() => {
                allow(m == "POST", "POST").unwrap_or_else(|| self.op(r, id))
            }
            ["api", "git"] => allow(m == "POST", "POST").unwrap_or_else(|| self.git(r)),
            ["api", "others"] => allow(get, "GET, HEAD").unwrap_or_else(|| self.others()),
            ["api", "pair"] => allow(m == "POST", "POST").unwrap_or_else(|| self.pair(r)),
            ["api", "devices"] => allow(get, "GET, HEAD").unwrap_or_else(|| self.list_devices()),
            ["api", "devices", id, "revoke"] if !id.is_empty() => {
                allow(m == "POST", "POST").unwrap_or_else(|| self.revoke_device(id))
            }
            ["api", "push"] => match m {
                "GET" | "HEAD" => self.push_key(),
                "POST" => self.push_subscribe(r),
                _ => allow(false, "GET, HEAD, POST").unwrap_or_else(|| fail(405, "")),
            },
            _ => text(404, "404 page not found\n"),
        }
    }

    /// Whether a browser's request came from this app: its Origin is the
    /// configured one, or the host it asked for.
    fn same_origin(&self, r: &Request) -> bool {
        let o = r.header("origin");
        if o.is_empty() {
            let site = r.header("sec-fetch-site");
            return site.is_empty() || site == "same-origin";
        }
        if !self.config.origin.is_empty() && o == self.config.origin.trim_end_matches('/') {
            return true;
        }
        url_host(o).is_some_and(|h| h == r.host())
    }

    fn login(&self, r: &Request) -> Response {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct In {
            #[serde(alias = "Token")]
            token: String,
            /// A phone pairing: see pair.rs.
            #[serde(alias = "Code")]
            code: String,
        }
        let parsed = read_json::<In>(r, 1 << 20);
        let ok = self.same_origin(r)
            && parsed
                .as_ref()
                .is_ok_and(|i| self.token_ok(i.token.trim()) || self.pairs.take(i.code.trim()));
        if !ok {
            // A pause per bad guess, not a rate limiter; the token is 256 bits.
            std::thread::sleep(self.bad_guess);
            return fail(401, "that isn't this machine's token");
        }
        match self.sign_in(r) {
            Ok((_, set)) => {
                let mut resp = write_json(&json!({ "name": self.config.name }));
                resp.add("Set-Cookie", &set);
                resp
            }
            Err(e) => fail(500, &e),
        }
    }

    /// Makes this browser a device of its own; the cookie that says so.
    fn sign_in(&self, r: &Request) -> Result<(Device, String), String> {
        let (tok, dev) = self.devs.add_web(&device_name(r.header("user-agent")))?;
        let mut c = format!(
            "{COOKIE}={tok}; Path=/; Max-Age={}; HttpOnly",
            400 * 24 * 3600
        );
        if is_https(r) {
            c.push_str("; Secure");
        }
        c.push_str("; SameSite=Strict");
        Ok((dev, c))
    }

    fn machines(&self) -> Response {
        let mut out = vec![Machine {
            name: self.config.name.clone(),
            me: true,
        }];
        out.extend(self.config.peers.iter().map(|p| Machine {
            name: p.name.clone(),
            me: false,
        }));
        write_json(&out)
    }

    /// This machine's sessions, each with whether its host is running.
    pub fn sessions(&self) -> Vec<Session> {
        rush_host::list()
            .into_iter()
            .map(|info| {
                let alive = rush_host::alive(info.host_pid);
                Session { info, alive }
            })
            .collect()
    }

    fn list(&self) -> Response {
        write_json(&self.sessions())
    }

    fn start_session(&self, r: &Request) -> Response {
        let mut req = match read_json::<StartRequest>(r, 1 << 20) {
            Ok(q) => q,
            Err(e) => return fail(400, &e),
        };
        if !req.resume.is_empty() {
            let Some(o) = crate::others::past_other(&self.reg, &req.resume) else {
                return fail(
                    404,
                    &format!("no past session {} on this machine", req.resume),
                );
            };
            req.agent = o.agent;
            req.cwd = o.cwd;
            req.profile.clear();
        }
        if let Some(rest) = req.cwd.strip_prefix('~')
            && (rest.is_empty() || rest.starts_with('/'))
        {
            let home = std::env::var("HOME").unwrap_or_default();
            req.cwd = format!("{home}{rest}");
        }
        if !Path::new(&req.cwd).is_absolute() {
            return fail(400, "the folder must be a full path");
        }
        if let Err(e) = self.allowed_cwd(&req.cwd) {
            return fail(403, &e);
        }
        let Some(start) = &self.start else {
            return fail(501, "this serve can't start sessions");
        };
        let info = match start(&req) {
            Ok(i) => i,
            Err(e) => {
                rush_events::log(
                    "remote-start-failed",
                    fields(
                        json!({ "agent": req.agent, "cwd": req.cwd, "resume": req.resume, "err": e }),
                    ),
                );
                return fail(400, &e);
            }
        };
        rush_host::events::log_event(
            &info.id,
            "remote-started",
            json!({ "agent": req.agent, "cwd": req.cwd, "resume": req.resume }),
        );
        if !req.resume.is_empty() {
            // It's rush's now.
            crate::others::forget();
        }
        let alive = rush_host::alive(info.host_pid);
        write_json(&Session { info, alive })
    }

    /// Whether a remote client may start a session in dir: never in rush's
    /// own folder (the tokens and keys serve holds), and where remote.json's
    /// workspaces list some, only in them.
    pub fn allowed_cwd(&self, dir: &str) -> Result<(), String> {
        if under(&real(dir), &real(&crate::config::dir().to_string_lossy())) {
            return Err("that folder is rush's own".into());
        }
        if self.config.workspaces.is_empty() {
            return Ok(());
        }
        if self
            .config
            .workspaces
            .iter()
            .any(|w| under(&real(dir), &real(w)))
        {
            return Ok(());
        }
        Err("that folder isn't one of this machine's workspaces".into())
    }

    fn op(&self, r: &Request, id: &str) -> Response {
        if !valid_id(id) {
            return fail(404, "no such session");
        }
        let o = match read_json::<RemoteOp>(r, OP_MOST) {
            Ok(o) => o,
            Err(e) => return fail(400, &e),
        };
        if !REMOTE_OPS.contains(&o.op.as_str()) {
            return fail(403, &format!("{:?} can't be sent remotely", o.op));
        }
        if o.op == "send" && o.text.trim().is_empty() && o.pictures.is_empty() {
            return fail(400, "nothing to send");
        }
        if let Some(a) = &o.away
            && !a.every.is_zero()
            && a.every < Duration::from_secs(60)
        {
            return fail(400, "check in at most once a minute");
        }
        if o.op != "send" && !o.pictures.is_empty() {
            return fail(400, "only a message takes images");
        }
        let info = rush_host::read_info(id);
        let mut woke = false;
        let mut err: Option<String> = None;
        let running = info.as_ref().is_ok_and(|i| rush_host::alive(i.host_pid));
        if o.op == "send" && !running {
            woke = true;
            err = rush_host::ensure(id, "").err().map(|e| e.to_string());
        } else if let Err(e) = &info {
            err = Some(e.to_string());
        } else if !running {
            err = Some("the session isn't running".into());
        }
        let mut f = json!({ "op": o.op, "woke": woke });
        if !o.id.is_empty() {
            f["id"] = json!(o.id);
        }
        if !o.text.is_empty() {
            f["text"] = json!(clip(&o.text));
        }
        if let Some(e) = &err {
            f["err"] = json!(e);
        }
        rush_host::events::log_event(id, "remote-op", f);
        if let Some(e) = err {
            return fail(409, &e);
        }
        let c = match rush_host::Client::dial(id) {
            Ok(c) => c,
            Err(e) => return fail(409, &e.to_string()),
        };
        let images = match save_pictures(id, &o.pictures) {
            Ok(p) => p,
            Err(e) => return fail(400, &e),
        };
        let mut hop = o.to_op();
        hop.images = images;
        if let Err(e) = c.send_op(hop) {
            return fail(502, &e.to_string());
        }
        c.close();
        Response::new(202, Vec::new())
    }

    /// Passes a request for /m/<peer>/api/… on to that peer's serve, with
    /// the peer's token in place of the browser's cookie. Peers are
    /// remote.json's: a request can't name somewhere else to go.
    fn proxy(&self, r: &Request, name: &str) -> Response {
        let rest = r.path.strip_prefix(&format!("/m/{name}")).unwrap_or("");
        let Some(p) = self.config.peer(name).filter(|_| rest.starts_with("/api/")) else {
            return fail(404, "no such machine");
        };
        let mut url = format!("{}{rest}", p.url.trim_end_matches('/'));
        if !r.query.is_empty() {
            url.push('?');
            url.push_str(&r.query);
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into();
        let mut b = ureq::http::Request::builder()
            .method(r.method.as_str())
            .uri(&url);
        for (k, v) in &r.headers {
            if matches!(
                k.as_str(),
                "cookie"
                    | "origin"
                    | "authorization"
                    | "host"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "keep-alive"
                    | "upgrade"
                    | "te"
                    | "trailer"
            ) {
                continue;
            }
            b = b.header(k.as_str(), v.as_str());
        }
        b = b.header("Authorization", format!("Bearer {}", p.token));
        let sent = match b.body(r.body.clone()) {
            Ok(req) => agent.run(req),
            Err(e) => return fail(502, &format!("{name} can't be reached: {e}")),
        };
        let res = match sent {
            Ok(res) => res,
            Err(e) => {
                return fail(
                    502,
                    &format!("{name} can't be reached: {}", go_dial_err(&e, &p.url)),
                );
            }
        };
        let status = res.status().as_u16();
        let mut headers = Vec::new();
        for (k, v) in res.headers() {
            let k = k.as_str();
            if matches!(
                k,
                "connection" | "content-length" | "transfer-encoding" | "keep-alive"
            ) {
                continue;
            }
            if let Ok(v) = v.to_str() {
                headers.push((k.to_string(), v.to_string()));
            }
        }
        let mut body = res.into_body().into_reader();
        let mut out = Response::stream(
            status,
            Box::new(move |w: &mut dyn Write| {
                let mut buf = [0u8; 16 << 10];
                while let Ok(n) = body.read(&mut buf) {
                    if n == 0 || w.write_all(&buf[..n]).is_err() {
                        return;
                    }
                }
            }),
        );
        out.headers = headers;
        out
    }

    /// A code that pairs a device, and where it opens it.
    fn pair(&self, _r: &Request) -> Response {
        let (code, until) = self.pairs.issue();
        let base = self.reached_at();
        let mut out = Pairing {
            code: code.clone(),
            base: base.clone(),
            expires: until,
            ..Pairing::default()
        };
        if !base.is_empty() {
            out.url = format!("{base}/#pair={code}");
        }
        if current().ssh {
            let listen = self.config.ssh_listen_or();
            let (mut host, mut port) = listen
                .rsplit_once(':')
                .map(|(h, p)| (h.to_string(), p.to_string()))
                .unwrap_or_default();
            if let Some(h) = url_hostname(&base).filter(|h| !h.is_empty()) {
                host = h;
                port = self.config.ssh_port_or().to_string();
            }
            out.ssh = format!("ssh -p {port} {host}");
            out.host_key = crate::sshkey::fingerprint();
        }
        write_json(&out)
    }

    /// The address phones reach this serve at: remote.json's origin, else
    /// the tailnet's name for it while it's reached over Tailscale.
    fn reached_at(&self) -> String {
        if !self.config.origin.is_empty() {
            return self.config.origin.trim_end_matches('/').to_string();
        }
        if current().reach != crate::config::REACH_TAILSCALE {
            return String::new();
        }
        crate::expose::tailnet_url(&self.config)
    }

    fn list_devices(&self) -> Response {
        let mut list = self.devs.all();
        for d in &mut list {
            d.token_hash.clear();
            d.live = self.live.count(&d.id);
        }
        // Go's nil list: null.
        write_json(&(!list.is_empty()).then_some(list))
    }

    fn revoke_device(&self, id: &str) -> Response {
        if !self.devs.revoke(id) {
            return fail(404, "no such device");
        }
        self.live.drop_device(id);
        write_json(&json!({}))
    }

    fn push_key(&self) -> Response {
        match &self.push {
            None => fail(404, "notifications aren't set up here"),
            Some(p) => write_json(&json!({ "key": p.public_key() })),
        }
    }

    fn push_subscribe(&self, r: &Request) -> Response {
        let Some(p) = &self.push else {
            return fail(404, "notifications aren't set up here");
        };
        let sub = match read_json::<Subscription>(r, 1 << 20) {
            Ok(s) => s,
            Err(e) => return fail(400, &e),
        };
        if let Err(e) = p.add(sub) {
            return fail(400, &e);
        }
        let p = p.clone();
        std::thread::spawn(move || {
            p.send(&Notice {
                title: "rush".into(),
                body: "Notifications are on.".into(),
                url: "/".into(),
                tag: "rush-on".into(),
            })
        });
        Response::new(204, Vec::new())
    }

    /// Streams a session as Server-Sent Events, each {"t":…,"e":…}: rush's
    /// neutral events, and info, answered, sent, error and reset of its
    /// own. A running host gives its replay then what happens; one that
    /// isn't running gives its saved transcript, and the stream picks the
    /// host up when something wakes it. Reading never wakes it. History
    /// starts with the transcript's last HISTORY_MOST events, or with
    /// ?full=1 all of it.
    fn events(&self, r: &Request, id: &str) -> Response {
        if !valid_id(id) {
            return fail(404, "no such session");
        }
        let full = r.query_get("full").as_deref() == Some("1");
        let (id, ctx, reg) = (id.to_string(), r.ctx.clone(), self.reg.clone());
        let mut resp = Response::stream(
            200,
            Box::new(move |w: &mut dyn Write| {
                let send =
                    |w: &mut dyn Write, t: &str, e: &Value| -> bool { sse(w, Some(&tagged(t, e))) };
                // The info the transcript was shown with.
                let mut shown: Option<Time> = None;
                let mut n = 0u64;
                loop {
                    let info = match rush_host::read_info(&id) {
                        Ok(i) => i,
                        Err(e) => {
                            send(w, "error", &json!({ "Error": e.to_string() }));
                            return;
                        }
                    };
                    if rush_host::alive(info.host_pid) {
                        if let Ok(c) = rush_host::Client::dial(&id) {
                            send(w, "reset", &Value::Null);
                            if !info.replay_from.is_zero() {
                                send_history(&reg, &info, info.replay_from, full, &mut |b| {
                                    sse(w, Some(&b));
                                });
                            }
                            let ok = pump(&ctx, &c, w);
                            c.close();
                            if !ok || ctx.done() {
                                return;
                            }
                            shown = None;
                        }
                    } else if shown.is_none_or(|s| s != info.updated_at) {
                        // Polls the info file each second while the host is down.
                        send(w, "reset", &Value::Null);
                        send(
                            w,
                            "info",
                            &serde_json::to_value(&info).unwrap_or(Value::Null),
                        );
                        send_history(&reg, &info, Time::default(), full, &mut |b| {
                            sse(w, Some(&b));
                        });
                        shown = Some(if info.updated_at.is_zero() {
                            Time::from_unix(1, 0)
                        } else {
                            info.updated_at
                        });
                    } else if n.is_multiple_of(15) && !sse(w, None) {
                        return;
                    }
                    n += 1;
                    if !ctx.sleep(Duration::from_secs(1)) {
                        return;
                    }
                }
            }),
        );
        sse_headers(&mut resp);
        resp
    }

    /// Serves on ln until ctx ends or serving fails, with notifications
    /// and what exposes it running beside it. Whichever way it ends, ctx
    /// is cancelled before waiting on them, so a failed server can't leave
    /// them up or hang its own shutdown.
    pub fn run(
        self: &Arc<Self>,
        ctx: &Ctx,
        ln: TcpListener,
        log: Log,
        sshd: Option<Up>,
    ) -> Result<(), String> {
        let ctx = ctx.child();
        let addr = ln.local_addr().map_err(|e| e.to_string())?;
        rush_events::log(
            "serve-started",
            fields(json!({ "addr": addr.to_string(), "machine": self.config.name })),
        );
        {
            let (s, c) = (self.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("serve-watch".into())
                .spawn(move || s.watch(&c))
                .map_err(|e| e.to_string())?;
        }
        {
            let (cfg, c, l) = (self.config.clone(), ctx.clone(), log.clone());
            std::thread::Builder::new()
                .name("serve-attach".into())
                .spawn(move || crate::attach::attach_peers(&c, &cfg, &l))
                .map_err(|e| e.to_string())?;
        }
        let exposed = expose(&ctx, self.config.clone(), addr.port(), log.clone(), sshd);
        let r = http::serve(&ctx, ln, self.handler());
        ctx.cancel();
        let _ = exposed.join();
        rush_events::log("serve-stopped", serde_json::Map::new());
        if let Err(e) = &r {
            say(&log, &format!("rush serve: {e}\n"));
        }
        r.map_err(|e| e.to_string())
    }
}

/// A failed request to url as Go's net/http says it, for the commonest:
/// "dial tcp 127.0.0.1:9: connect: connection refused".
pub(crate) fn go_dial_err(e: &ureq::Error, url: &str) -> String {
    let refused =
        matches!(e, ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused);
    if !refused {
        return e.to_string();
    }
    let mut host = url_host(url).unwrap_or_default();
    if !host.contains(':') || host.ends_with(']') {
        host.push_str(if url.starts_with("https:") {
            ":443"
        } else {
            ":80"
        });
    }
    format!("dial tcp {host}: connect: connection refused")
}

/// Headers every response carries: no framing, no sniffing, and scripts
/// only from serve itself.
fn secure(r: &mut Response) {
    r.set(
        "Content-Security-Policy",
        "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
    );
    r.set("X-Content-Type-Options", "nosniff");
    r.set("Referrer-Policy", "no-referrer");
}

fn logout(r: &Request) -> Response {
    let mut c = format!("{COOKIE}=; Path=/; Max-Age=0; HttpOnly");
    if is_https(r) {
        c.push_str("; Secure");
    }
    let mut resp = Response::new(204, Vec::new());
    resp.add("Set-Cookie", &c);
    resp
}

/// Whether the browser reached serve over HTTPS, itself or through what's
/// in front of it.
fn is_https(r: &Request) -> bool {
    r.tls || r.header("x-forwarded-proto") == "https"
}

/// The host:port of a URL.
pub(crate) fn url_host(u: &str) -> Option<String> {
    let (_, rest) = u.split_once("://")?;
    let h = rest.split(['/', '?', '#']).next()?;
    Some(h.rsplit_once('@').map_or(h, |(_, h)| h).to_string())
}

/// The host of a URL, without its port.
fn url_hostname(u: &str) -> Option<String> {
    let h = url_host(u)?;
    if let Some(v6) = h.strip_prefix('[') {
        return v6.split_once(']').map(|(a, _)| a.to_string());
    }
    Some(
        h.rsplit_once(':')
            .map_or(h.as_str(), |(a, _)| a)
            .to_string(),
    )
}

/// p with its links resolved, as git and Go's EvalSymlinks name folders.
pub(crate) fn real(p: &str) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| clean(Path::new(p)))
}

/// p made lexically clean, as Go's filepath.Clean.
pub(crate) fn clean(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<String> = Vec::new();
    let abs = p.is_absolute();
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if out.last().is_some_and(|l| l != "..") {
                    out.pop();
                } else if !abs {
                    out.push("..".into());
                }
            }
            _ => {}
        }
    }
    let s = out.join("/");
    PathBuf::from(if abs {
        format!("/{s}")
    } else if s.is_empty() {
        ".".into()
    } else {
        s
    })
}

/// Whether p is root or under it.
pub(crate) fn under(p: &Path, root: &Path) -> bool {
    let (p, root) = (clean(p), clean(root));
    p == root || p.starts_with(&root)
}

/// A session id with a session folder behind it.
fn valid_id(id: &str) -> bool {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return false;
    }
    rush_host::root().join(id).is_dir()
}

/// The ops a remote client may send a host: answering, steering and
/// stopping its agent, its queue, going away and coming back, and what it
/// waits on at a limit. Rewinds, compacting and relogins work on files or
/// accounts the client has only on its own machine.
pub const REMOTE_OPS: [&str; 23] = [
    "send",
    "allow",
    "deny",
    "interrupt",
    "stop",
    "stop_task",
    "background",
    "model",
    "effort",
    "mode",
    "without",
    "tell",
    "context",
    "limit",
    "away",
    "decisions_dismiss",
    "queue_edit",
    "queue_remove",
    "queue_move",
    "queue_merge",
    "queue_send",
    "queue_hold",
    "queue_separate",
];

/// The part of a host op a remote client may set: never a path on this
/// machine (images, exchanges) the agent would read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteOp {
    pub op: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub always: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<RawJson>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub interrupt: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub now: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub guide: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub index: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub was: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub to: i64,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "rush_core::wire::nullvec::deserialize"
    )]
    pub without: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub away: Option<Away>,
    /// A send's images as bytes: serve writes them to the session's
    /// scratch folder here, so no path ever crosses.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "rush_core::wire::nullvec::deserialize"
    )]
    pub pictures: Vec<Picture>,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl RemoteOp {
    fn to_op(&self) -> rush_host::Op {
        let mut o = rush_host::Op::named(&self.op);
        o.text = self.text.clone();
        o.id = self.id.clone();
        o.always = self.always;
        o.input = self.input.clone().filter(|r| r.get() != "null");
        o.message = self.message.clone();
        o.interrupt = self.interrupt;
        o.mode = self.mode.clone();
        o.model = self.model.clone();
        o.effort = self.effort.clone();
        o.now = self.now;
        o.guide = self.guide;
        o.index = self.index;
        o.was = self.was.clone();
        o.to = self.to;
        o.without = self.without.clone();
        o.away = self.away.clone();
        o
    }
}

/// One image sent with a message.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Picture {
    /// image/png, image/jpeg, image/gif or image/webp
    #[serde(rename = "type")]
    pub kind: String,
    /// base64 in JSON
    #[serde(serialize_with = "to_b64", deserialize_with = "b64")]
    pub data: Vec<u8>,
}

fn to_b64<S: serde::Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b))
}

fn b64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(Vec::new()),
        Some(s) => base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom),
    }
}

const PICTURES_MOST: usize = 8;
/// Before the host's own 5 MB check, which a png may pass as webp.
const PICTURE_MOST: usize = 20 << 20;
/// An op's whole body, its images base64 in it.
const OP_MOST: usize = 64 << 20;

/// Writes a send's pictures into session id's scratch folder; their paths.
fn save_pictures(id: &str, ps: &[Picture]) -> Result<Vec<String>, String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if ps.len() > PICTURES_MOST {
        return Err(format!("at most {PICTURES_MOST} images at once"));
    }
    let mut paths = Vec::new();
    for p in ps {
        let ext = match p.kind.as_str() {
            "image/png" => ".png",
            "image/jpeg" => ".jpg",
            "image/gif" => ".gif",
            "image/webp" => ".webp",
            _ => return Err(format!("{:?} isn't a png, jpeg, gif or webp image", p.kind)),
        };
        if p.data.is_empty() || p.data.len() > PICTURE_MOST {
            return Err(format!("an image must be under {} MB", PICTURE_MOST >> 20));
        }
        let dir = rush_host::tmp::temp_dir(id);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| e.to_string())?;
        let path = dir.join(format!("remote-{}{ext}", hex(&rand_bytes(8))));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        f.write_all(&p.data).map_err(|e| e.to_string())?;
        paths.push(path.to_string_lossy().into_owned());
    }
    Ok(paths)
}

/// {"e":e,"t":t}, as Go writes the map.
fn tagged(t: &str, e: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "t": t, "e": e })).unwrap_or_default()
}

/// The stream's headers.
pub(crate) fn sse_headers(r: &mut Response) {
    r.set("Content-Type", "text/event-stream");
    r.set("Cache-Control", "no-store");
    r.set("X-Accel-Buffering", "no");
}

/// Writes one event, or with None a keepalive comment; false once the
/// client is gone.
pub(crate) fn sse(w: &mut dyn Write, data: Option<&[u8]>) -> bool {
    match data {
        None => w.write_all(b": \n\n").is_ok(),
        Some(d) => {
            let mut b = Vec::with_capacity(d.len() + 8);
            b.extend_from_slice(b"data: ");
            b.extend_from_slice(d);
            b.extend_from_slice(b"\n\n");
            w.write_all(&b).is_ok()
        }
    }
}

/// Passes a host's lines on as events until the host or the client goes;
/// false is the client gone.
fn pump(ctx: &Ctx, c: &rush_host::Client, w: &mut dyn Write) -> bool {
    use std::sync::mpsc::RecvTimeoutError;
    let keep = Duration::from_secs(15);
    let mut last = std::time::Instant::now();
    loop {
        if ctx.done() {
            return false;
        }
        // Waits a second at a time, to see the end.
        match c.lines.recv_timeout(Duration::from_secs(1)) {
            Ok(l) => {
                if let Some(b) = wire_line(&l)
                    && !sse(w, Some(&b))
                {
                    return false;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return true,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if last.elapsed() >= keep {
            last = std::time::Instant::now();
            if !sse(w, None) {
                return false;
            }
        }
    }
}

/// A host's line as the web app reads it, None for what it has no use for.
pub fn wire_line(l: &[u8]) -> Option<Vec<u8>> {
    use rush_host::lines::Line;
    let (t, e) = match rush_host::decode(l).ok()? {
        Line::Event(ev) => return event::marshal(&ev).ok().map(String::into_bytes),
        Line::Info(i) => ("info", serde_json::to_value(&*i).ok()?),
        Line::Answered(id) => ("answered", json!({ "ID": id })),
        Line::Error(e) => ("error", json!({ "Error": e })),
        Line::Sent(s) => (
            "sent",
            json!({
                "Exchange": s.exchange,
                "Text": s.text,
                "Images": if s.images.is_empty() { Value::Null } else { json!(s.images) },
            }),
        ),
        Line::Stamp(t) => ("stamp", json!({ "At": t })),
        _ => return None,
    };
    Some(tagged(t, &e))
}

/// How many of a transcript's last events a stream starts with, and how
/// many when asked for all of it.
pub const HISTORY_MOST: usize = 600;
pub const FULL_MOST: usize = 50_000;

/// Sends what a session's transcript holds from before before (all of it
/// when zero): its last HISTORY_MOST events, or with full the whole
/// transcript up to FULL_MOST. A "cut" says there's more before.
pub fn send_history(
    reg: &Registry,
    info: &Info,
    before: Time,
    full: bool,
    emit: &mut dyn FnMut(Vec<u8>),
) {
    let kind = migrated(&info.kind);
    let Some(a) = reg.get(kind.as_str()) else {
        return;
    };
    let mut p = Profile {
        kind: kind.clone(),
        ..Profile::default()
    };
    if let Ok(cfg) = rush_host::read_config(&info.id) {
        p.dir = cfg.account.dir;
    }
    let transcript = reg.find_transcript(kind.as_str(), &p, &info.cwd, &info.session_id);
    let s = AgentSession {
        id: info.session_id.clone(),
        name: info.name.clone(),
        cwd: info.cwd.clone(),
        profile: p,
        transcript,
        ..AgentSession::default()
    };
    const TAIL_BYTES: i64 = 4 << 20;
    let most = if full { FULL_MOST } else { HISTORY_MOST };
    let (evs, mut cut): (Vec<Event>, bool) = if let Some(tb) = a.history_tail_before_reader()
        && !before.is_zero()
        && !full
    {
        match tb.history_tail_before(&s, TAIL_BYTES, before.to_ms()) {
            Ok(x) => x,
            Err(_) => return,
        }
    } else if let Some(tr) = a.tail_reader()
        && before.is_zero()
        && !full
    {
        match tr.history_tail(&s, TAIL_BYTES) {
            Ok(x) => x,
            Err(_) => return,
        }
    } else if let Some(hr) = a.history_reader() {
        // Keeps only the end, a slice at a time.
        let mut keep: VecDeque<Event> = VecDeque::new();
        let mut dropped = false;
        let r = hr.history(
            &s,
            before.to_ms(),
            rush_core::adapter::SLICE,
            &mut |slice| {
                for ev in slice {
                    keep.push_back(ev);
                    if keep.len() > most {
                        keep.pop_front();
                        dropped = true;
                    }
                }
                true
            },
        );
        if r.is_err() {
            return;
        }
        (keep.into_iter().collect(), dropped)
    } else {
        return;
    };
    if evs.is_empty() {
        return;
    }
    let evs = if evs.len() > most {
        cut = true;
        evs[evs.len() - most..].to_vec()
    } else {
        evs
    };
    if cut {
        emit(br#"{"t":"cut"}"#.to_vec());
    }
    for ev in &evs {
        if let Ok(b) = event::marshal(ev) {
            emit(b.into_bytes());
        }
    }
}

/// Reads the body as JSON, at most most bytes.
pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(
    r: &Request,
    most: usize,
) -> Result<T, String> {
    if r.body.len() > most {
        return Err("that's too big to send".into());
    }
    serde_json::from_slice(&r.body).map_err(|e| e.to_string())
}

/// The start of s, enough to know it by, for the event log.
fn clip(s: &str) -> String {
    let n = s.chars().count();
    if n > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s.to_string()
    }
}

/// v as the JSON answer, a line as Go's encoder writes it.
pub(crate) fn write_json<T: Serialize + ?Sized>(v: &T) -> Response {
    let mut b = serde_json::to_vec(v).unwrap_or_default();
    b.push(b'\n');
    let mut r = Response::new(200, b);
    r.set("Content-Type", "application/json");
    r.set("Cache-Control", "no-store");
    r
}

/// An error answer.
pub(crate) fn fail(code: u16, msg: &str) -> Response {
    let mut b = serde_json::to_vec(&json!({ "error": msg })).unwrap_or_default();
    b.push(b'\n');
    let mut r = Response::new(code, b);
    r.set("Content-Type", "application/json");
    r
}

/// A plain text answer, as Go's http.Error writes it.
fn text(code: u16, msg: &str) -> Response {
    let mut r = Response::new(code, msg.as_bytes().to_vec());
    r.set("Content-Type", "text/plain; charset=utf-8");
    r.set("X-Content-Type-Options", "nosniff");
    r
}
