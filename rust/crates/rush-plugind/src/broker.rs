//! The broker itself: `rush plugind`. One per user, held by a lock; it
//! listens on broker.sock for rush (session hosts, the CLI and rush's
//! windows), keeps a runner for every plugin that's enabled, and exits once
//! none is.
//! Ported from internal/plugind/plugind.go.

use std::collections::{BTreeMap, HashMap};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_plugin::rpc::{CODE_NO_METHOD, Handler};
use rush_plugin::{Answer, CliResult, CliRun, Conn, Ctx, Error, Raw};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;

use crate::host::{FileHosts, Hosts};
use crate::runner::Runner;
use crate::ui::UiHub;
use crate::{Log, Scope, call_err, empty, fields, params, server, to_raw};

/// The plugin protocol this broker speaks, sent in initialize.
pub const VERSION: i64 = 1;

/// What the broker says about one plugin it runs.
pub use rush_plugin::client::Status;

/// How often each plugin's memory is looked at; its files every sixth look.
const WATCH_EVERY: Duration = Duration::from_secs(5);

/// How often plugins in development mode are looked at.
const DEVELOP_EVERY: Duration = Duration::from_secs(2);

/// The broker: what it runs, and what it holds of rush's screens.
#[derive(Clone)]
pub struct Broker(pub(crate) Arc<Inner>);

pub(crate) struct Inner {
    pub(crate) log: Log,
    pub(crate) scope: Scope,
    pub(crate) hosts: Arc<dyn Hosts>,
    plugins: Mutex<HashMap<String, Arc<Runner>>>,
    pub(crate) ui: UiHub,
    /// Done when the broker stops.
    pub(crate) quit: Ctx,
    stop: rush_plugin::Cancel,
    /// Numbers each connection on broker.sock, for the UI it may be.
    conns: AtomicU64,
}

impl Broker {
    /// A broker that runs nothing yet, reaching sessions through hosts.
    pub fn new(hosts: Arc<dyn Hosts>, log: Log, scope: Scope) -> Broker {
        let (quit, stop) = Ctx::background().with_cancel();
        Broker(Arc::new_cyclic(|me| Inner {
            log,
            scope,
            hosts,
            plugins: Mutex::new(HashMap::new()),
            ui: UiHub::new(me.clone()),
            quit,
            stop,
            conns: AtomicU64::new(0),
        }))
    }

    /// Answers one connection on broker.sock. A UI's calls need its
    /// connection, to send it what changes.
    pub fn accept(&self, s: UnixStream) -> std::io::Result<Conn> {
        let id = self.0.conns.fetch_add(1, Ordering::Relaxed) + 1;
        let b = Arc::downgrade(&self.0);
        let h: Handler = Arc::new(move |conn, ctx, method, p| {
            let Some(b) = b.upgrade() else {
                return Err(server("the broker has stopped"));
            };
            b.scope.enter();
            if method.starts_with("ui.") {
                return b.ui.answer_ui(ctx, id, conn, method, p);
            }
            Broker(b).answer_rush(ctx, method, p)
        });
        Conn::new(s, Some(h))
    }

    /// Ends the broker: Run returns once its plugins have stopped.
    pub fn stop(&self) {
        self.0.stop.cancel();
    }

    /// Done once the broker stops.
    pub fn done(&self) -> &Ctx {
        &self.0.quit
    }

    /// Brings the running plugins in line with what's enabled: new ones
    /// start, revoked or re-approved ones stop (and the re-approved start
    /// again).
    pub fn reload(&self) {
        let approved = rush_plugin::enabled();
        let mut stopping = Vec::new();
        let empty = {
            let mut ps = lock(&self.0.plugins);
            ps.retain(|name, r| {
                let keep = approved.get(name).is_some_and(|a| a.digest == r.digest);
                if !keep {
                    stopping.push(r.clone());
                }
                keep
            });
            for (name, a) in &approved {
                if ps.contains_key(name) {
                    continue;
                }
                let r = Runner::new(Some(self), name, &a.digest);
                ps.insert(name.clone(), r.clone());
                self.0.scope.log(
                    "plugin-start",
                    fields(&[("plugin", json!(name)), ("why", json!("approved"))]),
                );
                self.0
                    .scope
                    .spawn("plugin-supervise", move || r.supervise());
            }
            ps.is_empty()
        };
        // A revoked plugin's arrangement of the list goes with it.
        rush_plugin::sidebar::prune_sidebars(&approved);
        for r in stopping {
            self.0
                .log
                .print(&format!("{}: stopping (approval changed)", r.name));
            self.0.scope.log(
                "plugin-stop",
                fields(&[
                    ("plugin", json!(r.name)),
                    ("why", json!("approval changed")),
                ]),
            );
            r.shutdown();
        }
        if empty {
            self.0.log.print("no plugins to run");
            self.0.scope.log(
                "plugind-exit",
                fields(&[("why", json!("no plugin is approved"))]),
            );
            self.stop();
        }
    }

    /// The runner of plugin name, if it's enabled.
    pub(crate) fn runner(&self, name: &str) -> Option<Arc<Runner>> {
        self.0.runner(name)
    }

    /// What the broker runs now.
    pub fn status(&self) -> Vec<Status> {
        let mut out: Vec<Status> = self.0.runners().iter().map(|r| r.status()).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Checks every few seconds that each plugin is within its memory
    /// limit and its files are as approved, until the broker stops.
    fn watch(&self) {
        let mut tick = 0u64;
        while self.0.quit.sleep(WATCH_EVERY) {
            let rs = self.0.runners();
            let pids: Vec<i32> = rs.iter().map(|r| r.pid_now()).filter(|&p| p > 0).collect();
            if !pids.is_empty() {
                let mut table = rush_sys::proc::snapshot();
                table.fill(None, &pids);
                for r in &rs {
                    let Some(p) = table.procs.get(&r.pid_now()) else {
                        continue;
                    };
                    let limit = r.live().0.memory();
                    if p.footprint > limit {
                        r.fail(&format!(
                            "using {} MB, over its {} MB limit",
                            p.footprint >> 20,
                            limit >> 20
                        ));
                    }
                }
            }
            // Files changed under a running plugin: it is no longer the
            // one you approved. Checked less often, as it reads every file.
            if tick % 6 == 5 {
                for r in &rs {
                    let p = r.live().0;
                    if r.pid_now() > 0 && !p.bundled {
                        let same = rush_plugin::digest(std::path::Path::new(&p.dir))
                            .is_ok_and(|d| d == r.digest);
                        if !same {
                            r.fail("its files changed since it was approved");
                        }
                    }
                }
            }
            tick += 1;
        }
    }

    /// Installs again, from its repository as it is, each plugin in
    /// development mode whose files changed, once they've stopped changing
    /// for a look, and runs what it built. One that now asks for more
    /// waits on your approval instead. A build that fails isn't tried
    /// again until the files change again.
    fn develop(&self) {
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        let mut failed: BTreeMap<String, String> = BTreeMap::new();
        while self.0.quit.sleep(DEVELOP_EVERY) {
            let changed = rush_plugin::dev_changed();
            for (name, h) in &changed {
                if seen.get(name) != Some(h) || failed.get(name) == Some(h) {
                    seen.insert(name.clone(), h.clone()); // still changing, or already failed as it is
                    continue;
                }
                let log = &self.0.log;
                log.print(&format!(
                    "{name}: its files changed, building it again (development mode)"
                ));
                match rush_plugin::rebuild(name) {
                    Err(e) => {
                        failed.insert(name.clone(), h.clone());
                        log.print(&format!("{name}: building it again: {e}"));
                        self.0.scope.log(
                            "plugin-rebuild",
                            fields(&[("plugin", json!(name)), ("error", json!(e))]),
                        );
                        continue;
                    }
                    Ok(false) => {
                        log.print(&format!(
                            "{name}: built, and it asks for more than was approved: it waits on you"
                        ));
                        self.0.scope.log(
                            "plugin-rebuild",
                            fields(&[
                                ("plugin", json!(name)),
                                ("why", json!("asks for more: waits on approval")),
                            ]),
                        );
                    }
                    Ok(true) => self.0.scope.log(
                        "plugin-rebuild",
                        fields(&[("plugin", json!(name)), ("why", json!("files changed"))]),
                    ),
                }
                failed.remove(name);
                self.reload();
            }
            seen.retain(|name, _| changed.contains_key(name));
        }
    }

    /// Answers rush itself: session hosts and the CLI.
    fn answer_rush(
        &self,
        ctx: &Ctx,
        method: &str,
        p: Option<&RawValue>,
    ) -> Result<Option<Raw>, Error> {
        match method {
            "mcp" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    plugin: String,
                    session: String,
                    message: Option<Raw>,
                }
                let p: P = params(p)?;
                let r = self
                    .runner(&p.plugin)
                    .ok_or_else(|| server(format!("{} is not running", p.plugin)))?;
                let msg = p.message.unwrap_or_else(|| crate::raw("null"));
                Ok(Some(r.mcp(ctx, &p.session, &msg)))
            }
            "cli" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    plugin: String,
                    run: CliRun,
                }
                let p: P = params(p)?;
                let out = self.cli(ctx, &p.plugin, p.run)?;
                Ok(Some(to_raw(&out)))
            }
            "services.ask" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    service: String,
                }
                let p: P = params(p)?;
                Ok(Some(to_raw(&self.ask(ctx, &p.service, ""))))
            }
            "reload" => {
                let b = self.clone();
                self.0.scope.spawn("plugind-reload", move || b.reload());
                Ok(empty())
            }
            "status" => Ok(Some(to_raw(&self.status()))),
            _ => Err(Error::new(
                CODE_NO_METHOD,
                format!("method not found: {method}"),
            )),
        }
    }

    /// Calls service.call on every running plugin that provides service,
    /// at once, but the one asking (from), and gathers their answers by
    /// plugin. One that can't answer in time says so in its error.
    pub(crate) fn ask(&self, ctx: &Ctx, service: &str, from: &str) -> Vec<Answer> {
        let rs = self.0.runners();
        let out: Mutex<Vec<Answer>> = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for r in &rs {
                let (p, conn) = r.live();
                let Some(conn) = conn else { continue };
                if r.name == from || !p.provides.iter().any(|x| x == service) {
                    continue;
                }
                let out = &out;
                let scope = &self.0.scope;
                s.spawn(move || {
                    scope.enter();
                    let ctx = ctx.with_timeout(rush_plugin::SERVICE_TIMEOUT);
                    let mut a = Answer {
                        plugin: r.name.clone(),
                        ..Default::default()
                    };
                    let q = crate::object(vec![("service", to_raw(service))]);
                    match conn.call_raw(&ctx, "service.call", Some(q)) {
                        Ok(res) => a.result = res,
                        Err(e) => a.error = e.to_string(),
                    }
                    lock(out).push(a);
                });
            }
        });
        let mut out = out.into_inner().unwrap_or_else(|e| e.into_inner());
        out.sort_by(|a, b| a.plugin.cmp(&b.plugin));
        out
    }

    /// Runs one of a plugin's CLI commands, for `rush <plugin> <command>`.
    pub(crate) fn cli(&self, ctx: &Ctx, name: &str, run: CliRun) -> Result<CliResult, Error> {
        let r = match self.runner(name) {
            Some(r) => r,
            None => {
                self.reload(); // a broker the CLI just started
                self.runner(name)
                    .ok_or_else(|| server(format!("{name} is not running")))?
            }
        };
        let conn = r.wait(ctx).map_err(server)?;
        let (p, _) = r.live();
        p.check_cli_run(&run).map_err(crate::bad_params)?;
        let ctx = ctx.with_timeout(rush_plugin::CLI_TIMEOUT);
        let out: CliResult = conn.call(&ctx, "cli.run", &run).map_err(call_err)?;
        Ok(out.clip())
    }
}

impl Inner {
    pub(crate) fn runner(&self, name: &str) -> Option<Arc<Runner>> {
        lock(&self.plugins).get(name).cloned()
    }

    pub(crate) fn runners(&self) -> Vec<Arc<Runner>> {
        lock(&self.plugins).values().cloned().collect()
    }

    /// Takes on r as plugin name's runner, as reload would: for tests.
    #[cfg(test)]
    pub(crate) fn put_runner(&self, r: Arc<Runner>) {
        lock(&self.plugins).insert(r.name.clone(), r);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Takes the broker's lock without waiting: None when another broker
/// holds it.
fn take_lock() -> Result<Option<std::fs::File>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = rush_plugin::broker_lock();
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    // SAFETY: flock on a descriptor we hold open.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(None);
    }
    Ok(Some(f))
}

/// `rush plugind`: runs every enabled plugin until none is, reaching
/// sessions through rush's files and logging to stderr (broker.log, as
/// rush starts it). It returns at once if another broker holds the lock.
pub fn run() -> Result<(), String> {
    run_with(
        Arc::new(FileHosts::default()),
        Log::stderr("plugind "),
        Scope::here(),
        true,
    )
}

/// Run, with the hosts, log and scope given; signals says whether SIGHUP,
/// SIGTERM and SIGINT are taken, as `rush plugind` takes them.
pub fn run_with(
    hosts: Arc<dyn Hosts>,
    log: Log,
    scope: Scope,
    signals: bool,
) -> Result<(), String> {
    scope.enter();
    let root = rush_plugin::root();
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .map_err(|e| format!("mkdir {}: {e}", root.display()))?;
    }
    let Some(_lock) = take_lock()? else {
        return Ok(()); // another broker is running
    };
    let sock = rush_plugin::broker_sock();
    let _ = std::fs::remove_file(&sock);
    // Only you can connect: the folder is 0700 and the socket 0600.
    // SAFETY: umask only sets this process's file mode mask.
    let old = unsafe { libc::umask(0o177) };
    let ln = UnixListener::bind(&sock);
    // SAFETY: as above, putting it back.
    unsafe { libc::umask(old) };
    let ln = ln.map_err(|e| format!("listen unix {}: {e}", sock.display()))?;

    let b = Broker::new(hosts, log, scope.clone());
    b.0.log
        .print(&format!("started, pid {}", std::process::id()));
    scope.log("plugind-start", serde_json::Map::new());
    b.reload();
    {
        let b = b.clone();
        scope.spawn("plugind-watch", move || b.watch());
    }
    {
        let b = b.clone();
        scope.spawn("plugind-develop", move || b.develop());
    }
    if signals {
        let w = Arc::downgrade(&b.0);
        crate::signals::notify(&scope, move |sig| {
            let Some(b) = w.upgrade() else { return };
            let b = Broker(b);
            if sig == libc::SIGHUP {
                b.reload();
            } else {
                b.stop();
            }
        });
    }
    {
        let b = b.clone();
        let quit = b.0.quit.clone();
        scope.spawn("plugind-accept", move || {
            for c in ln.incoming() {
                if quit.err().is_some() {
                    return;
                }
                if let Ok(c) = c {
                    let _ = b.accept(c);
                }
            }
        });
    }
    b.0.quit.wait();
    // Wakes the accept, which then sees the broker has stopped.
    drop(UnixStream::connect(&sock));
    if signals {
        crate::signals::forget();
    }
    let rs = b.0.runners();
    std::thread::scope(|s| {
        for r in &rs {
            s.spawn(|| r.shutdown());
        }
    });
    let _ = std::fs::remove_file(&sock);
    b.0.log.print("stopped");
    scope.log("plugind-stop", serde_json::Map::new());
    Ok(())
}

/// Asks the running broker for its plugins' status.
pub fn ask() -> Result<Vec<Status>, String> {
    rush_plugin::client::ask_status()
}

/// Tells the running broker the approvals changed, starting it if it isn't
/// running and a plugin is approved: Go's plugind.Reload and hooks.Reload.
pub fn reload() -> Result<(), String> {
    rush_plugin::client::reload()
}
