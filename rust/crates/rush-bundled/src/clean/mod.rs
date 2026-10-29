//! rush's bundled clean-up plugins: go-clean, js-clean and git-clean. Each
//! is off until you turn it on, and runs only with its tools installed.
//! Its one command lists what that ecosystem keeps on your disk (caches,
//! stores, dependencies in agents' worktrees, merged branches) with how
//! much each takes, and what of it is running (gopls, node), with how much
//! memory; enter on a row cleans it up or ends it.
//!
//! Nothing goes by itself: sizes are measured in the background, and only
//! what you pick is cleaned.
//! Ported from internal/bundled/clean/clean.go:1-574.

pub mod fleet;
mod kits;
mod reap;

use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rush_plugin::rpc::{Error, from_raw, no_method};
use rush_plugin::{
    Bundle, CleanupReport, CommandSpec, Conn, Ctx, EV_SESSION_STOPPED, EV_TURN_ENDED,
    EV_TURN_STARTED, Manifest, Pick, PickAction, PickItem, Picked, Requires,
    SERVICE_CLEANUP_REPORT, SettingSpec, UI_EVENTS, UI_NOTIFY, UiEvent,
};
use rush_state::Time;
use rush_sys::proc;
use serde::Deserialize;
use serde_json::json;

pub use reap::{ReapFacts, refusals};

/// The bundled clean-up plugins.
pub(crate) fn bundles() -> Vec<Bundle> {
    vec![
        crate::bundle_of(kits::go_kit().manifest(), true, run_go),
        crate::bundle_of(kits::js_kit().manifest(), true, run_js),
        crate::bundle_of(kits::git_kit().manifest(), true, run_git),
    ]
}

fn run_go(s: UnixStream) -> Result<(), String> {
    run(kits::go_kit(), s)
}

fn run_js(s: UnixStream) -> Result<(), String> {
    run(kits::js_kit(), s)
}

fn run_git(s: UnixStream) -> Result<(), String> {
    run(kits::git_kit(), s)
}

/// Cleans a thing up, and says what it did beyond what it freed.
pub(crate) type Clean = Arc<dyn Fn(&Ctx) -> Result<String, String> + Send + Sync>;

/// One ecosystem's clean-up.
pub(crate) struct Kit {
    pub name: &'static str,
    pub about: &'static str,
    pub needs: &'static [&'static str],
    /// The process names it owns, as the kernel has them.
    pub procs: &'static [&'static str],
    /// Finds what it keeps on disk, by the folders sessions were seen in.
    /// It may run programs; sizes are measured after.
    pub things: fn(&Ctx, &[Session]) -> Vec<Thing>,
    /// What it offers under Settings, Plugins.
    pub settings: Vec<SettingSpec>,
}

/// One row of what a kit keeps.
#[derive(Clone)]
pub(crate) struct Thing {
    pub id: String,
    pub title: String,
    /// measured for its size, when set
    pub path: String,
    /// shown instead of a size, when set
    pub meta: String,
    /// what enter does, under its title
    pub does: String,
    /// A worktree it's in: it isn't cleaned while a session works there.
    pub in_dir: String,
    pub clean: Clean,
    pub size: i64,
}

impl Thing {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        does: impl Into<String>,
        clean: Clean,
    ) -> Thing {
        Thing {
            id: id.into(),
            title: title.into(),
            path: String::new(),
            meta: String::new(),
            does: does.into(),
            in_dir: String::new(),
            clean,
            size: 0,
        }
    }

    pub fn at(mut self, path: impl Into<String>) -> Thing {
        self.path = path.into();
        self
    }

    pub fn meta(mut self, meta: impl Into<String>) -> Thing {
        self.meta = meta.into();
        self
    }

    pub fn within(mut self, dir: impl Into<String>) -> Thing {
        self.in_dir = dir.into();
        self
    }
}

/// What a kit knows of a session seen in rush: where it works, and
/// whether it's working now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Session {
    pub id: String,
    pub cwd: String,
    pub state: String,
}

impl Kit {
    pub fn manifest(&self) -> Manifest {
        Manifest {
            ui: vec![UI_EVENTS.into(), UI_NOTIFY.into()],
            commands: vec![CommandSpec {
                name: "clean".into(),
                description: format!(
                    "what {} finds on disk and running, and cleaning it up",
                    self.name
                ),
                ..Default::default()
            }],
            // proc reads processes only there
            requires: Requires {
                os: vec!["darwin".into(), "linux".into()],
                bin: self.needs.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            settings: self.settings.clone(),
            provides: vec![SERVICE_CLEANUP_REPORT.into()],
            ..crate::manifest(self.name, "machine", self.about, 128)
        }
    }
}

/// How often sizes are measured again, while it runs.
const MEASURE_EVERY: Duration = Duration::from_secs(15 * 60);
/// How soon after it starts, or a folder comes in, it measures: Go's
/// first tick, for the sessions rush tells it of as it connects.
const MEASURE_SOON: Duration = Duration::from_secs(5);

#[derive(Default)]
struct State {
    seen: BTreeMap<String, Session>,
    things: Vec<Thing>,
    /// when it last measured; None before it has, or once it should again
    measured: Option<Time>,
    /// when it measures next
    due: Option<Instant>,
    measuring: bool,
    /// things being cleaned
    busy: std::collections::HashSet<String>,
    /// when it last said worktrees can be reaped
    said: Option<Time>,
    done: bool,
}

struct App {
    k: &'static Kit,
    st: Mutex<State>,
    wake: Condvar,
    /// Done when the plugin ends: cleans in flight stop with it.
    ctx: Ctx,
}

fn run(k: &'static Kit, s: UnixStream) -> Result<(), String> {
    let (ctx, cancel) = Ctx::background().with_cancel();
    let a = Arc::new(App {
        k,
        st: Mutex::new(State {
            due: Some(Instant::now() + MEASURE_SOON),
            ..Default::default()
        }),
        wake: Condvar::new(),
        ctx,
    });
    let h = a.clone();
    let conn = Conn::new(
        s,
        Some(Arc::new(move |conn, _, method, params| {
            h.handle(conn, method, params)
        })),
    )
    .map_err(|e| e.to_string())?;
    let looper = {
        let (a, c) = (a.clone(), conn.clone());
        std::thread::spawn(move || a.run_loop(&c))
    };
    conn.wait();
    a.lock().done = true;
    a.wake.notify_all();
    cancel.cancel();
    let _ = looper.join();
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ServiceCall {
    service: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UiCommand {
    command: String,
    ui: String,
}

impl App {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn handle(
        self: &Arc<Self>,
        conn: &Conn,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Option<rush_plugin::Raw>, Error> {
        match method {
            "initialize" | "ui.settings" => crate::empty(),
            "tools.list" => crate::no_tools(),
            "ui.event" => {
                if let Ok(ev) = from_raw::<UiEvent>(params) {
                    self.event(ev);
                }
                Ok(None)
            }
            "ui.command" => {
                let input: UiCommand = from_raw(params).map_err(Error::server)?;
                let _ = input.command;
                let (a, c) = (self.clone(), conn.clone());
                std::thread::spawn(move || a.show(&c, &input.ui));
                crate::empty()
            }
            "service.call" => {
                let input: ServiceCall = from_raw(params).unwrap_or_default();
                if input.service == SERVICE_CLEANUP_REPORT {
                    return crate::answer(&self.report());
                }
                Err(no_method(method))
            }
            "ui.picked" => {
                let input: Picked = from_raw(params).map_err(Error::server)?;
                if input.action == "enter" {
                    let (a, c) = (self.clone(), conn.clone());
                    std::thread::spawn(move || a.clean(&c, &input.ui, &input.item));
                }
                crate::empty()
            }
            _ => Err(no_method(method)),
        }
    }

    /// Keeps where sessions work and whether they're working.
    fn event(&self, ev: UiEvent) {
        let Some(sess) = ev.session.as_ref().filter(|s| !s.cwd.is_empty()) else {
            return;
        };
        let mut st = self.lock();
        let mut s = Session {
            id: sess.id.clone(),
            cwd: sess.cwd.clone(),
            state: sess.state.clone(),
        };
        match ev.kind.as_str() {
            EV_TURN_STARTED => s.state = "working".into(),
            EV_TURN_ENDED | EV_SESSION_STOPPED if s.state == "working" => s.state = "idle".into(),
            _ => {}
        }
        let fresh = st.seen.get(&s.id).is_none_or(|was| was.cwd != s.cwd);
        st.seen.insert(s.id.clone(), s);
        if fresh && !st.measuring {
            // A folder came in: what's in it is worth measuring soon.
            self.soon(&mut st);
        }
    }

    /// Measures again soon. Under the lock.
    fn soon(&self, st: &mut State) {
        st.measured = None;
        let soon = Instant::now() + MEASURE_SOON;
        st.due = Some(st.due.map_or(soon, |d| d.min(soon)));
        self.wake.notify_all();
    }

    fn sessions(&self) -> Vec<Session> {
        // By id, as a BTreeMap keeps them.
        self.lock().seen.values().cloned().collect()
    }

    /// Measures what it keeps now and then, and soon after sessions are
    /// first seen. It sleeps until a measure is due, as Go's ticker would
    /// find it, without waking in between.
    fn run_loop(&self, conn: &Conn) {
        let mut st = self.lock();
        loop {
            if st.done {
                return;
            }
            let due = st.due.unwrap_or_else(|| Instant::now() + MEASURE_EVERY);
            let now = Instant::now();
            if now < due {
                st = self
                    .wake
                    .wait_timeout(st, due - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
                continue;
            }
            drop(st);
            self.measure(conn);
            st = self.lock();
        }
    }

    fn measure(&self, conn: &Conn) {
        {
            let mut st = self.lock();
            if st.measuring {
                return;
            }
            st.measuring = true;
            st.due = None;
        }
        let mut things = (self.k.things)(&self.ctx, &self.sessions());
        for t in &mut things {
            if !t.path.is_empty() {
                t.size = fleet::disk_usage(&t.path);
            }
        }
        let text = {
            let mut st = self.lock();
            let now = Time::now();
            st.things = things;
            st.measured = Some(now);
            st.measuring = false;
            st.due = Some(Instant::now() + MEASURE_EVERY);
            let text = reap_notice(&st.things, st.said, now);
            if !text.is_empty() {
                st.said = Some(now);
            }
            text
        };
        if !text.is_empty() {
            self.notify(conn, "", &text); // every window: it's nobody's in particular
        }
    }

    /// What it keeps, as last measured, for rush's cleanup agent.
    fn report(&self) -> CleanupReport {
        let (things, measured) = {
            let st = self.lock();
            (st.things.clone(), st.measured)
        };
        let mut r = CleanupReport {
            title: self.k.name.into(),
            rules: format!(
                "- Each line is something {} can clean up, and what cleaning it does; it's safe to do that for one nobody works in.",
                self.k.name
            ),
            ..Default::default()
        };
        let Some(measured) = measured else {
            r.report = "It hasn't measured yet.".into();
            return r;
        };
        let mut lines = vec![format!(
            "As measured {} (what · size · what cleaning it does):",
            ago(measured, Time::now())
        )];
        for t in &things {
            let size = if t.meta.is_empty() {
                format!("{:.1}G", t.size as f64 / (1u64 << 30) as f64)
            } else {
                t.meta.clone()
            };
            let mut l = format!("- {} · {size} · {}", t.title, t.does);
            if !t.path.is_empty() {
                l.push_str(&format!(" ({})", t.path));
            }
            lines.push(l);
        }
        r.report = lines.join("\n");
        r
    }

    /// Opens the list in the window that asked.
    fn show(&self, conn: &Conn, ui: &str) {
        let (things, measured) = {
            let st = self.lock();
            (st.things.clone(), st.measured)
        };
        let p = pick(self.k, &things, measured, &running(self.k), Time::now());
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let _ = conn.call::<_, serde_json::Value>(&ctx, "ui.pick", &json!({ "ui": ui, "pick": p }));
    }

    /// Cleans up the thing, or ends the process, picked.
    fn clean(&self, conn: &Conn, ui: &str, id: &str) {
        if id.starts_with("pid:") {
            self.notify(conn, ui, &end(self.k, id));
            return;
        }
        let t = {
            let mut st = self.lock();
            let Some(t) = st.things.iter().find(|t| t.id == id).cloned() else {
                return;
            };
            if st.busy.contains(id) {
                return;
            }
            if !t.in_dir.is_empty() && working_in(&st.seen, &t.in_dir) {
                drop(st);
                self.notify(
                    conn,
                    ui,
                    &format!(
                        "{}: an agent is working there now, so it's left alone",
                        t.title
                    ),
                );
                return;
            }
            st.busy.insert(id.to_string());
            t
        };
        self.notify(conn, ui, &format!("cleaning up {}…", t.title));
        let ctx = self.ctx.with_timeout(Duration::from_secs(10 * 60));
        let got = (t.clean)(&ctx);
        let freed = if t.path.is_empty() {
            0
        } else {
            t.size - fleet::disk_usage(&t.path)
        };
        {
            let mut st = self.lock();
            st.busy.remove(id);
            self.soon(&mut st); // measure again, soon
        }
        let said = match got {
            Err(e) => format!("{}: {e}", t.title),
            Ok(said) if freed > 0 => format!("{}: freed {}. {said}", t.title, bytes(freed))
                .trim()
                .to_string(),
            Ok(said) => format!("{}: done. {said}", t.title).trim().to_string(),
        };
        self.notify(conn, ui, &said);
    }

    fn notify(&self, conn: &Conn, ui: &str, text: &str) {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let _ = conn.call::<_, serde_json::Value>(
            &ctx,
            "ui.notify",
            &json!({ "ui": ui, "text": text }),
        );
    }
}

/// Whether a session seen is working in dir.
fn working_in(seen: &BTreeMap<String, Session>, dir: &str) -> bool {
    seen.values()
        .any(|s| s.state == "working" && (s.cwd == dir || s.cwd.starts_with(&format!("{dir}/"))))
}

/// How often, at most, it says worktrees can be reaped.
const REAP_NOTICE_EVERY_NS: i128 = 4 * 3600 * 1_000_000_000;

/// The quiet word that worktrees can be reaped, "" when there are none or
/// it said so within REAP_NOTICE_EVERY. It only points at the list:
/// nothing goes until you pick it there.
fn reap_notice(things: &[Thing], said: Option<Time>, now: Time) -> String {
    let n = things.iter().filter(|t| t.id.starts_with("reap:")).count();
    if n == 0 || said.is_some_and(|s| now.sub_nanos(s) < REAP_NOTICE_EVERY_NS) {
        return String::new();
    }
    format!(
        "{} can be reaped: #git-clean.clean to review",
        plural(n, "worktree", "worktrees")
    )
}

fn pick(k: &Kit, things: &[Thing], measured: Option<Time>, running: &[Running], now: Time) -> Pick {
    let mut p = Pick {
        id: "clean".into(),
        title: k.name.into(),
        tabs: vec!["On disk".into()],
        actions: vec![PickAction {
            key: "enter".into(),
            name: "clean up".into(),
            stay: true,
        }],
        empty: vec!["Nothing found yet.".into()],
        ..Default::default()
    };
    match measured {
        None => {
            p.about = "Measuring what's on disk; look again in a moment.".into();
            p.empty[0] = "Measuring…".into();
        }
        Some(m) => p.about = format!("Measured {}. Enter on a row cleans it up.", ago(m, now)),
    }
    let mut total = 0;
    for t in things {
        let mut meta = t.meta.clone();
        if meta.is_empty() {
            meta = bytes(t.size);
            total += t.size;
        }
        let mut lines = vec![t.title.clone(), format!("enter {}", t.does)];
        if !t.path.is_empty() {
            lines.insert(1, t.path.clone());
        }
        p.items.push(PickItem {
            id: t.id.clone(),
            text: lines.join("\n"),
            meta,
            ..Default::default()
        });
    }
    if total > 0 {
        p.tabs[0] = format!("On disk · {}", bytes(total));
    }
    if !k.procs.is_empty() {
        p.tabs.push("Running".into());
        p.empty
            .push(format!("None of {} is running.", k.procs.join(", ")));
        let mut mem = 0u64;
        for r in running {
            mem += r.p.footprint;
            let up = Time::from_unix(
                r.p.start
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64),
                0,
            );
            p.items.push(PickItem {
                id: r.id(),
                tab: 1,
                text: format!(
                    "{} · pid {} · up {}\n{}\nenter ends it",
                    r.p.comm,
                    r.p.pid,
                    ago(up, now),
                    r.cmd
                ),
                meta: bytes(r.p.footprint as i64),
            });
        }
        if mem > 0 {
            p.tabs[1] = format!("Running · {}", bytes(mem as i64));
        }
    }
    p
}

/// A process a kit owns.
#[derive(Clone, Debug)]
pub(crate) struct Running {
    pub p: proc::Proc,
    pub cmd: String,
}

impl Running {
    fn id(&self) -> String {
        format!("pid:{}:{}", self.p.pid, start_nanos(&self.p))
    }
}

fn start_nanos(p: &proc::Proc) -> u128 {
    p.start
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// The kit's processes, biggest first.
fn running(k: &Kit) -> Vec<Running> {
    if k.procs.is_empty() {
        return Vec::new();
    }
    let mut tab = proc::snapshot();
    let me = std::process::id() as proc::Pid;
    let pids: Vec<proc::Pid> = tab
        .procs
        .values()
        .filter(|p| k.procs.contains(&&*p.comm) && p.pid != me)
        .map(|p| p.pid)
        .collect();
    tab.fill(None, &pids);
    let mut out: Vec<Running> = pids
        .into_iter()
        .filter_map(|pid| {
            let p = tab.procs.get(&pid)?.clone();
            let cmd = proc::command_line(pid);
            // gone, or not ours to look at
            (!(cmd.is_empty() && p.footprint == 0)).then_some(Running { p, cmd })
        })
        .collect();
    out.sort_by(|a, b| b.p.footprint.cmp(&a.p.footprint));
    out
}

/// Ends the process id names, if it's still the one listed.
fn end(k: &Kit, id: &str) -> String {
    let parts: Vec<&str> = id.split(':').collect();
    if parts.len() != 3 {
        return "no such process".into();
    }
    let pid: proc::Pid = parts[1].parse().unwrap_or(0);
    let start: u128 = parts[2].parse().unwrap_or(0);
    for r in running(k) {
        if r.p.pid == pid && start_nanos(&r.p) == start {
            if let Err(e) = proc::kill(pid, libc::SIGTERM) {
                return format!("{} (pid {pid}): {e}", r.p.comm);
            }
            return format!(
                "ended {} (pid {pid}), which held {}",
                r.p.comm,
                bytes(r.p.footprint as i64)
            );
        }
    }
    format!("pid {pid} has already gone")
}

fn ago(t: Time, now: Time) -> String {
    const MIN: i128 = 60 * 1_000_000_000;
    let d = now.sub_nanos(t);
    if d < MIN {
        "just now".into()
    } else if d < 60 * MIN {
        format!("{}m ago", d / MIN)
    } else if d < 48 * 60 * MIN {
        format!("{}h ago", d / (60 * MIN))
    } else {
        format!("{}d ago", d / (24 * 60 * MIN))
    }
}

/// n bytes for people, as the efficiency module says them: 3.0 GB, 512 MB.
pub(crate) fn bytes(n: i64) -> String {
    let f = n as f64;
    if n >= 1 << 30 {
        format!("{:.1} GB", f / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.0} MB", f / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KB", f / (1u64 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

/// Runs a tool it needs, found as rush finds agents, and gives back what
/// it printed.
pub(crate) fn command(ctx: &Ctx, dir: &str, name: &str, args: &[&str]) -> Result<String, String> {
    let Some(bin) = rush_core::adapter::find(name, &[]) else {
        return Err(format!("{name} isn't installed"));
    };
    let mut cmd = Command::new(bin);
    cmd.args(args);
    if !dir.is_empty() {
        cmd.current_dir(dir);
    }
    // A look must not block an agent's own git.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    let out = crate::exec::run(cmd, ctx, None, false).map_err(|e| e.to_string())?;
    if !out.success() {
        if !out.stderr.is_empty() {
            return Err(format!(
                "{name}: {}",
                last_line(&String::from_utf8_lossy(&out.stderr)).trim()
            ));
        }
        return Err(match out.status.and_then(|s| s.code()) {
            Some(c) => format!("exit status {c}"),
            None => "signal: killed".into(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn last_line(s: &str) -> &str {
    let s = s.trim();
    match s.rfind('\n') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// A Clean that runs a command.
pub(crate) fn ran(dir: &str, name: &'static str, args: &[&str]) -> Clean {
    let dir = dir.to_string();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    Arc::new(move |ctx| {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        command(ctx, &dir, name, &a).map(|_| String::new())
    })
}

/// A Clean that deletes a folder.
pub(crate) fn removed(path: &str) -> Clean {
    let path = path.to_string();
    Arc::new(move |_| match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(String::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(rush_plugin::gopath::os_err(
            "unlinkat",
            std::path::Path::new(&path),
            &e,
        )),
    })
}

/// Whether path is a folder.
pub(crate) fn is_dir(path: &str) -> bool {
    !path.is_empty() && std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// A folder under the user's cache folder (os.UserCacheDir).
pub(crate) fn cache_dir(name: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return String::new();
    }
    let base = if cfg!(target_os = "macos") {
        format!("{home}/Library/Caches")
    } else {
        std::env::var("XDG_CACHE_HOME")
            .ok()
            .filter(|d| d.starts_with('/'))
            .unwrap_or_else(|| format!("{home}/.cache"))
    };
    format!("{base}/{name}")
}

/// The linked worktrees of every repo sessions were seen in, and which a
/// session is working in now.
pub(crate) fn worktrees(seen: &[Session]) -> (Vec<fleet::Worktree>, HashMap<String, bool>) {
    let agents: Vec<fleet::Agent> = seen
        .iter()
        .map(|s| fleet::Agent {
            key: s.id.clone(),
            cwd: s.cwd.clone(),
        })
        .collect();
    let state: HashMap<&str, &str> = seen
        .iter()
        .map(|s| (s.id.as_str(), s.state.as_str()))
        .collect();
    let wts = fleet::find_worktrees(&agents);
    let mut busy = HashMap::new();
    for w in &wts {
        if w.agents
            .iter()
            .any(|k| state.get(k.as_str()) == Some(&"working"))
        {
            busy.insert(w.path.clone(), true);
        }
    }
    (wts, busy)
}

/// The main checkouts sessions were seen in.
pub(crate) fn repos(ctx: &Ctx, seen: &[Session]) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    let mut cwds = std::collections::HashSet::new();
    for s in seen {
        if !cwds.insert(s.cwd.clone()) {
            continue;
        }
        let Ok(common) = command(
            ctx,
            &s.cwd,
            "git",
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ) else {
            continue;
        };
        if let Some(d) = std::path::Path::new(&common).parent() {
            set.insert(d.to_string_lossy().into_owned());
        }
    }
    set.into_iter().collect()
}

pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        return format!("1 {one}");
    }
    format!("{n} {many}")
}

#[cfg(test)]
mod tests;
