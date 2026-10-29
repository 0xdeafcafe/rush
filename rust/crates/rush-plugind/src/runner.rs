//! Keeping one plugin running: starting it (sandboxed, or as rush itself
//! for a bundled one), saying hello, starting it again when it ends
//! (waiting longer each time), and answering the MCP messages sessions
//! send its tools.
//! Ported from internal/plugind/runner.go.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use rush_plugin::rpc::{CODE_NO_METHOD, CODE_PARSE, Handler};
use rush_plugin::{Cancel, Conn, Ctx, CtxErr, Error, Launch, Plugin, Proxy, Raw};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::RawValue;

use crate::broker::{Broker, Inner, Status, VERSION};
use crate::host::Hosts;
use crate::sessions::id_ok;
use crate::ui::{Outbox, PLUGIN_QUEUE};
use crate::{Log, Scope, fields, object, raw, to_raw};

/// One plugin's runner.
pub(crate) struct Runner {
    pub(crate) name: String,
    /// as approved
    pub(crate) digest: String,
    pub(crate) log: Log,
    pub(crate) scope: Scope,
    pub(crate) hosts: Arc<dyn Hosts>,
    b: Weak<Inner>,
    st: Mutex<RunState>,
    /// Told of every change to st that wait looks for.
    cv: Condvar,
    /// Programs it's running, see exec.
    pub(crate) execs: AtomicUsize,
    quit: Ctx,
    stop: Cancel,
}

/// What changes as it runs.
pub(crate) struct RunState {
    pub(crate) p: Plugin,
    pub(crate) conn: Option<Conn>,
    pid: i32,
    /// Done when its last process exited.
    exited: Option<Ctx>,
    /// An MCP plugin's initialize result.
    mcp_init: Option<Raw>,
    state: String,
    since: rush_state::Time,
    restarts: i64,
    last_err: String,
    /// Followed sessions, by id.
    pub(crate) subs: HashMap<String, Arc<Sub>>,
    /// Ends sessions.watch.
    pub(crate) unwatch: Option<Cancel>,
    /// Notifications to it, while it runs.
    pub(crate) out: Option<Arc<Outbox>>,
    /// When it last started sessions, to cap how fast it can.
    pub(crate) starts: Vec<Instant>,
}

/// One followed session: cancel ends the following.
pub(crate) struct Sub {
    pub(crate) cancel: Cancel,
}

impl Runner {
    /// A runner of plugin name, as approved with digest, for broker b (None
    /// in tests that need none).
    pub(crate) fn new(b: Option<&Broker>, name: &str, digest: &str) -> Arc<Runner> {
        let (log, scope, hosts, weak) = match b {
            Some(b) => (
                b.0.log.named(&format!("plugind {name} ")),
                b.0.scope.clone(),
                b.0.hosts.clone(),
                Arc::downgrade(&b.0),
            ),
            None => (
                Log::stderr(&format!("plugind {name} ")),
                Scope::here(),
                Arc::new(crate::host::FileHosts) as Arc<dyn Hosts>,
                Weak::new(),
            ),
        };
        Runner::with(name, digest, log, scope, hosts, weak)
    }

    pub(crate) fn with(
        name: &str,
        digest: &str,
        log: Log,
        scope: Scope,
        hosts: Arc<dyn Hosts>,
        b: Weak<Inner>,
    ) -> Arc<Runner> {
        let (quit, stop) = Ctx::background().with_cancel();
        Arc::new(Runner {
            name: name.into(),
            digest: digest.into(),
            log,
            scope,
            hosts,
            b,
            st: Mutex::new(RunState {
                p: Plugin::default(),
                conn: None,
                pid: 0,
                exited: None,
                mcp_init: None,
                state: "starting".into(),
                since: rush_state::Time::now(),
                restarts: 0,
                last_err: String::new(),
                subs: HashMap::new(),
                unwatch: None,
                out: None,
                starts: Vec::new(),
            }),
            cv: Condvar::new(),
            execs: AtomicUsize::new(0),
            quit,
            stop,
        })
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, RunState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The broker it runs under, while there is one.
    pub(crate) fn broker(&self) -> Option<Broker> {
        self.b.upgrade().map(Broker)
    }

    pub(crate) fn status(&self) -> Status {
        let s = self.lock();
        Status {
            name: self.name.clone(),
            state: s.state.clone(),
            pid: s.pid as i64,
            since: s.since,
            restarts: s.restarts,
            error: s.last_err.clone(),
        }
    }

    pub(crate) fn pid_now(&self) -> i32 {
        self.lock().pid
    }

    fn set_state(&self, state: &str, err: &str) {
        let mut s = self.lock();
        s.state = state.into();
        s.since = rush_state::Time::now();
        if !err.is_empty() {
            s.last_err = err.into();
        }
        drop(s);
        self.cv.notify_all();
    }

    /// Puts p, conn and its outbox in place as a running plugin's, without
    /// starting anything: for tests that stand in for the plugin.
    #[cfg(test)]
    pub(crate) fn set_running(&self, p: Plugin, conn: Conn) {
        let mut s = self.lock();
        s.p = p;
        if let Some(o) = s.out.take() {
            o.close();
        }
        s.out = Some(Outbox::new(conn.clone(), PLUGIN_QUEUE, &self.scope));
        s.conn = Some(conn);
        s.state = "running".into();
    }

    /// Sets the manifest it answers by, without starting it: for tests.
    #[cfg(test)]
    pub(crate) fn set_plugin(&self, p: Plugin) {
        self.lock().p = p;
    }

    /// Starts the plugin and starts it again whenever it ends, until it is
    /// shut down or refused.
    pub(crate) fn supervise(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            if self.quit.err().is_some() {
                return;
            }
            let p = rush_plugin::verify(&self.name).and_then(|p| {
                if !p.bundled {
                    rush_plugin::supported()?;
                }
                Ok(p)
            });
            let p = match p {
                Ok(p) => p,
                Err(e) => {
                    // Not as approved: it waits for you, not for a timer.
                    self.log.print(&format!("refused: {e}"));
                    self.scope.log(
                        "plugin-refused",
                        fields(&[("plugin", json!(self.name)), ("err", json!(e))]),
                    );
                    self.set_state("refused", &e);
                    return;
                }
            };
            let began = Instant::now();
            let res = self.run(p);
            if self.quit.err().is_some() {
                return;
            }
            if began.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            let msg = match res {
                Ok(()) => "exited".to_string(),
                Err(e) => e,
            };
            let wait = crate::go_secs(backoff);
            self.log.print(&format!("{msg}; starting again in {wait}"));
            self.scope.log(
                "plugin-restart",
                fields(&[
                    ("plugin", json!(self.name)),
                    ("why", json!(msg)),
                    ("wait", json!(wait)),
                ]),
            );
            self.lock().restarts += 1;
            self.set_state("waiting", &msg);
            if !self.quit.sleep(backoff) {
                return;
            }
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    }

    /// Starts the plugin, and returns when it has exited: Ok when it exited
    /// as it should, else why not.
    fn run(self: &Arc<Self>, p: Plugin) -> Result<(), String> {
        self.lock().p = p.clone();
        self.set_state("starting", "");

        let proxy = if p.network.is_empty() {
            None
        } else {
            Some(Proxy::listen(&p.network, Some(self.log.as_proxy_log()))?)
        };
        let res = self.run_with(&p, proxy.as_ref().map_or(0, Proxy::port));
        if let Some(px) = proxy {
            px.close();
        }
        res
    }

    fn run_with(self: &Arc<Self>, p: &Plugin, proxy_port: u16) -> Result<(), String> {
        let mut cmd = Launch {
            plugin: p.clone(),
            proxy_port,
        }
        .process()?;
        let logp = rush_plugin::log_path(&p.name);
        if let Some(d) = logp.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let logf = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&logp)
            .map_err(|e| format!("open {}: {e}", logp.display()))?;
        cmd.stderr(Stdio::from(logf.try_clone().map_err(|e| e.to_string())?));

        let (child, conn) = if p.proto() == rush_plugin::PROTO_MCP {
            cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| spawn_err(&cmd, &e))?;
            let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
            let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
                return Err("no pipes to the plugin".into());
            };
            let (r, closer) =
                rush_plugin::sys::wakeable(OwnedFd::from(stdout)).map_err(|e| e.to_string())?;
            let h: Handler = Arc::new(from_mcp_server);
            let conn = Conn::lines(r, stdin, move || closer.close(), Some(h));
            (child, conn)
        } else {
            cmd.stdin(Stdio::null()).stdout(Stdio::from(logf));
            let (mine, theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
            rush_plugin::sandbox::with_ipc_fd(&mut cmd, theirs.as_raw_fd());
            let child = cmd.spawn();
            drop(theirs);
            let child = child.map_err(|e| spawn_err(&cmd, &e))?;
            let me = Arc::downgrade(self);
            let h: Handler = Arc::new(move |_, ctx, method, params| match me.upgrade() {
                Some(r) => {
                    r.scope.enter();
                    r.answer_plugin(ctx, method, params)
                }
                None => Err(crate::server("the plugin has stopped")),
            });
            match Conn::new(mine, Some(h)) {
                Ok(c) => (child, c),
                Err(e) => {
                    let mut child = child;
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.to_string());
                }
            }
        };

        let pid = child.id() as i32;
        let (exited, wait_err) = wait_for(child, &self.scope);
        {
            let mut s = self.lock();
            s.pid = pid;
            s.exited = Some(exited.clone());
        }
        let res = self.serve(p, &conn, pid, &exited, &wait_err);
        self.down(&conn);
        res
    }

    /// Says hello and waits for the plugin to end, once it's running.
    fn serve(
        &self,
        p: &Plugin,
        conn: &Conn,
        pid: i32,
        exited: &Ctx,
        wait_err: &Arc<Mutex<String>>,
    ) -> Result<(), String> {
        if let Err(e) = self.handshake(p, conn) {
            kill(pid, exited);
            return Err(format!("did not start: {e}"));
        }
        {
            let mut s = self.lock();
            s.conn = Some(conn.clone());
            if p.proto() == rush_plugin::PROTO_RUSH {
                s.out = Some(Outbox::new(conn.clone(), PLUGIN_QUEUE, &self.scope));
            }
        }
        self.cv.notify_all();
        self.set_state("running", "");
        if let Some(b) = self.broker() {
            b.0.ui.changed();
        }
        self.log.print(&format!("running, pid {pid}"));

        // Whichever comes first: it exits, it closes its end of the
        // channel (it's no use without it), or it's shut down.
        let (any, wake) = Ctx::background().with_cancel();
        let regs = [
            exited.on_cancel({
                let w = wake.clone();
                move || w.cancel()
            }),
            conn.ctx().on_cancel({
                let w = wake.clone();
                move || w.cancel()
            }),
            self.quit.on_cancel(move || wake.cancel()),
        ];
        any.wait();
        drop(regs);
        if exited.err().is_none() {
            kill(pid, exited);
            if self.quit.err().is_some() {
                return Err("stopped".into());
            }
        }
        let e = wait_err.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if e.is_empty() { Ok(()) } else { Err(e) }
    }

    /// Forgets a run that ended: its connection, what it followed, and
    /// what it added to rush's screen.
    pub(crate) fn down(&self, conn: &Conn) {
        conn.close();
        let (subs, unwatch, out) = {
            let mut s = self.lock();
            s.conn = None;
            s.pid = 0;
            s.mcp_init = None;
            (std::mem::take(&mut s.subs), s.unwatch.take(), s.out.take())
        };
        self.cv.notify_all();
        if let Some(o) = out {
            o.close();
        }
        for (_, sub) in subs {
            sub.cancel.cancel();
        }
        if let Some(u) = unwatch {
            u.cancel();
        }
        // What it added to rush's screen goes with it.
        if let Some(b) = self.broker() {
            b.0.ui.gone(&self.name);
        }
    }

    /// Says hello. A rush plugin is told who it is and what it may do; an
    /// MCP server is initialized as any MCP client would.
    pub(crate) fn handshake(&self, p: &Plugin, conn: &Conn) -> Result<(), String> {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(15));
        if p.proto() == rush_plugin::PROTO_MCP {
            let q = object(vec![
                ("protocolVersion", to_raw("2025-06-18")),
                ("capabilities", raw("{}")),
                (
                    "clientInfo",
                    object(vec![
                        ("name", to_raw("rush")),
                        ("version", to_raw(&VERSION.to_string())),
                    ]),
                ),
            ]);
            let res = conn
                .call_raw(&ctx, "initialize", Some(q))
                .map_err(|e| e.to_string())?;
            self.lock().mcp_init = res;
            return conn.notify_raw("notifications/initialized", None);
        }
        let mut exec: Vec<&String> = p.exec.keys().collect();
        exec.sort();
        let commands: Vec<&String> = p.commands.iter().map(|c| &c.name).collect();
        let q = object(vec![
            ("protocol", to_raw(&VERSION)),
            ("name", to_raw(&p.name)),
            ("dataDir", to_raw(&rush_plugin::data_dir(&p.name))),
            ("sessions", nil_or(&p.sessions)),
            ("workspaces", nil_or(&p.workspace_dirs())),
            ("network", nil_or(&p.network)),
            ("exec", nil_or(&exec)),
            ("ui", nil_or(&p.ui)),
            ("commands", to_raw(&commands)),
            (
                "settings",
                to_raw(&rush_plugin::setting_values_of(&p.manifest)),
            ),
        ]);
        conn.call_raw(&ctx, "initialize", Some(q))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The plugin's manifest, and its connection if it's up.
    pub(crate) fn live(&self) -> (Plugin, Option<Conn>) {
        let s = self.lock();
        (s.p.clone(), s.conn.clone())
    }

    /// Queues a notification to the plugin, without waiting: when the
    /// plugin falls behind, the oldest waiting is dropped, and one with a
    /// key replaces the one waiting with the same key.
    pub(crate) fn notify<T: Serialize + ?Sized>(&self, method: &str, params: &T, key: &str) {
        let out = self.lock().out.clone();
        if let Some(o) = out {
            o.put(method, params, key);
        }
    }

    /// Stops it for good.
    pub(crate) fn shutdown(&self) {
        self.stop.cancel();
        let exited = self.lock().exited.clone();
        if let Some(e) = exited {
            e.wait();
        }
    }

    /// Ends the running process for a reason; supervise starts it again.
    pub(crate) fn fail(&self, why: &str) {
        let (pid, exited) = {
            let mut s = self.lock();
            s.last_err = why.into();
            (s.pid, s.exited.clone())
        };
        let Some(exited) = exited.filter(|_| pid > 0) else {
            return;
        };
        self.log.print(&format!("ending it: {why}"));
        self.scope.log(
            "plugin-kill",
            fields(&[("plugin", json!(self.name)), ("why", json!(why))]),
        );
        kill(pid, &exited);
    }

    /// The connection once the plugin is up. A plugin that isn't up within
    /// a few seconds is failing to start, and the call fails rather than
    /// wait out the backoff.
    pub(crate) fn wait(&self, ctx: &Ctx) -> Result<Conn, String> {
        let ctx = ctx.with_timeout(Duration::from_secs(10));
        let mut s = self.lock();
        loop {
            if let Some(c) = &s.conn {
                return Ok(c.clone());
            }
            if s.state == "refused" {
                return Err(format!("{} is not running: {}", self.name, s.last_err));
            }
            if ctx.err().is_some() {
                let mut msg = format!("{} is not running", self.name);
                if !s.last_err.is_empty() {
                    msg += &format!(": {} (see rush plugin logs {})", s.last_err, self.name);
                }
                return Err(msg);
            }
            // Woken by any change; a second at most, for whoever asked
            // giving up.
            let left = ctx
                .deadline()
                .map_or(Duration::from_secs(1), |d| {
                    d.saturating_duration_since(Instant::now())
                })
                .min(Duration::from_secs(1));
            s = self
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Answers one MCP message from a session for this plugin's server,
    /// with the whole JSON-RPC reply.
    pub(crate) fn mcp(&self, ctx: &Ctx, session: &str, msg: &RawValue) -> Raw {
        #[derive(Deserialize)]
        struct M {
            #[serde(default, deserialize_with = "present")]
            id: Option<Raw>,
            #[serde(default)]
            method: String,
            #[serde(default, deserialize_with = "present")]
            params: Option<Raw>,
        }
        let Ok(m) = serde_json::from_str::<M>(msg.get()) else {
            return rpc_reply(None, None, Some(Error::new(CODE_PARSE, "parse error")));
        };
        let id = m.id.as_deref();
        let p = self.lock().p.clone();
        if !p.has_tools() {
            return rpc_reply(
                id,
                None,
                Some(Error::new(
                    CODE_NO_METHOD,
                    format!("{} has no tools", self.name),
                )),
            );
        }
        let mcp = p.proto() == rush_plugin::PROTO_MCP;
        let res: Result<Option<Raw>, Error> = match m.method.as_str() {
            "initialize" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct In {
                    #[serde(rename = "protocolVersion")]
                    protocol_version: String,
                }
                let i: In = m
                    .params
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p.get()).ok())
                    .unwrap_or_default();
                let mut out = vec![
                    ("protocolVersion", to_raw(&i.protocol_version)),
                    ("capabilities", raw(r#"{"tools":{}}"#)),
                    (
                        "serverInfo",
                        object(vec![
                            ("name", to_raw(&p.server())),
                            ("version", to_raw(&p.version)),
                        ]),
                    ),
                ];
                // An MCP server's own guidance for using it goes along.
                if mcp && self.wait(ctx).is_ok() {
                    #[derive(Deserialize, Default)]
                    #[serde(default)]
                    struct Init {
                        instructions: String,
                    }
                    let init: Init = self
                        .lock()
                        .mcp_init
                        .as_deref()
                        .and_then(|r| serde_json::from_str(r.get()).ok())
                        .unwrap_or_default();
                    if !init.instructions.is_empty() {
                        out.push(("instructions", to_raw(&init.instructions)));
                    }
                }
                Ok(Some(object(out)))
            }
            "ping" => Ok(Some(raw("{}"))),
            "tools/list" => self.wait(ctx).map_err(crate::server).and_then(|conn| {
                if mcp {
                    conn.call_raw(ctx, "tools/list", m.params)
                } else {
                    conn.call_raw(ctx, "tools.list", None)
                }
                .map_err(crate::call_err)
            }),
            "tools/call" => self.wait(ctx).map_err(crate::server).and_then(|conn| {
                if mcp {
                    return conn
                        .call_raw(ctx, "tools/call", m.params)
                        .map_err(crate::call_err);
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Call {
                    name: String,
                    #[serde(deserialize_with = "present")]
                    arguments: Option<Raw>,
                }
                let call: Call = m
                    .params
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p.get()).ok())
                    .unwrap_or_default();
                let mut args = vec![
                    ("session", to_raw(session)),
                    ("name", to_raw(&call.name)),
                    ("arguments", call.arguments.unwrap_or_else(|| raw("null"))),
                ];
                // Who's calling, so a plugin can tie the call to what it
                // knows the session by.
                if id_ok(session)
                    && let Ok(i) = self.hosts.read_info(session)
                {
                    args.push(("sessionId", to_raw(&i.session_id)));
                    args.push(("cwd", to_raw(&i.cwd)));
                    args.push((
                        "meta",
                        if i.meta.is_empty() {
                            raw("null")
                        } else {
                            to_raw(&i.meta)
                        },
                    ));
                }
                conn.call_raw(ctx, "tools.call", Some(object(args)))
                    .map_err(crate::call_err)
            }),
            other => Err(Error::new(
                CODE_NO_METHOD,
                format!("method not found: {other}"),
            )),
        };
        match res {
            Ok(r) => rpc_reply(id, Some(r.unwrap_or_else(|| raw("null"))), None),
            Err(e) => rpc_reply(id, None, Some(e)),
        }
    }
}

/// A member that's there, even as null, is Some: as a jsontext.Value is.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Raw>, D::Error> {
    Raw::deserialize(d).map(Some)
}

/// v, or null when it's empty: a nil slice as Go writes it.
fn nil_or<T: Serialize>(v: &[T]) -> Raw {
    if v.is_empty() { raw("null") } else { to_raw(v) }
}

/// A whole JSON-RPC reply.
fn rpc_reply(id: Option<&RawValue>, result: Option<Raw>, e: Option<Error>) -> Raw {
    let mut out = vec![("jsonrpc", to_raw("2.0"))];
    if let Some(id) = id {
        out.push(("id", id.to_owned()));
    }
    match e {
        Some(e) => out.push(("error", to_raw(&e))),
        None => out.push(("result", result.unwrap_or_else(|| raw("null")))),
    }
    object(out)
}

/// Answers what an MCP server asks of its client: nothing it asks for
/// (sampling, roots, elicitation) is offered.
fn from_mcp_server(
    _: &Conn,
    _: &Ctx,
    method: &str,
    _: Option<&RawValue>,
) -> Result<Option<Raw>, Error> {
    Err(Error::new(
        CODE_NO_METHOD,
        format!("rush does not offer {method}"),
    ))
}

fn spawn_err(cmd: &std::process::Command, e: &std::io::Error) -> String {
    format!(
        "fork/exec {}: {}",
        cmd.get_program().to_string_lossy(),
        rush_plugin::gopath::errno_text(e)
    )
}

/// Waits for child on a thread of its own: the Ctx returned is done when
/// it has exited, and the string then says how, as Go's cmd.Wait does
/// ("" when it exited 0).
fn wait_for(mut child: Child, scope: &Scope) -> (Ctx, Arc<Mutex<String>>) {
    let (exited, done) = Ctx::background().with_cancel();
    let err = Arc::new(Mutex::new(String::new()));
    let e = err.clone();
    scope.spawn("plugin-wait", move || {
        let msg = match child.wait() {
            Ok(st) => exit_text(st),
            Err(err) => err.to_string(),
        };
        *e.lock().unwrap_or_else(|e| e.into_inner()) = msg;
        done.cancel();
    });
    (exited, err)
}

/// An exit as Go's *exec.ExitError says it: "exit status 1", "signal:
/// killed"; "" for a clean exit.
fn exit_text(st: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(sig) = st.signal() {
        let name = match sig {
            libc::SIGKILL => "killed".to_string(),
            libc::SIGTERM => "terminated".to_string(),
            libc::SIGINT => "interrupt".to_string(),
            libc::SIGHUP => "hangup".to_string(),
            libc::SIGSEGV => "segmentation fault".to_string(),
            libc::SIGABRT => "abort trap".to_string(),
            libc::SIGBUS => "bus error".to_string(),
            libc::SIGPIPE => "broken pipe".to_string(),
            n => format!("signal {n}"),
        };
        return format!("signal: {name}");
    }
    match st.code() {
        Some(0) | None => String::new(),
        Some(c) => format!("exit status {c}"),
    }
}

/// Ends a plugin's process group: politely, then not.
fn kill(pid: i32, exited: &Ctx) {
    // SAFETY: signals the plugin's own process group, which it leads.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    if exited.with_timeout(Duration::from_secs(2)).wait() == CtxErr::DeadlineExceeded {
        // SAFETY: as above.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        exited.wait();
    }
}
