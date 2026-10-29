//! The UI's side of a host: starting one (spawn), reading what sessions
//! there are (read_info, Lister), and the Client, a connection to one
//! session's host that sends ops and receives its lines.
//!
//! Ported from internal/host/client.go (all of it but StopAll's pull
//! request followers, which are prdrive's), sleep.go:196-260 (Ensure) and
//! stale.go:59-90 (Sweep). A gone host's reading is revive.go:52 (lose).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rush_core::event::Exchange;
use rush_core::wire::RawJson;
use rush_state::Time;
use serde_json::json;

use crate::events::log_event;
use crate::lines::{Line, decode};
use crate::op::Op;
use crate::paths::{dir, new_session_id, root, short_id, sock_path};
use crate::wire::{Away, BinStamp, Branch, Config, Info};

/// What a client's calls fail with: its text is Go's.
#[derive(Debug)]
pub enum ClientError {
    /// The host ended the connection before it answered.
    Closed,
    /// The socket couldn't be reached, or a write failed.
    Io(std::io::Error),
    Other(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Closed => f.write_str("the host closed the connection"),
            ClientError::Io(e) => write!(f, "{e}"),
            ClientError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> ClientError {
        ClientError::Io(e)
    }
}

impl From<String> for ClientError {
    fn from(s: String) -> ClientError {
        ClientError::Other(s)
    }
}

pub(crate) type Result<T> = std::result::Result<T, ClientError>;

impl Config {
    /// Names a new session, and gives a fork an id of its own.
    pub fn fill_ids(&mut self) {
        if self.kind.is_empty() {
            // migration: callers from before rush ran other agents name
            // none, meaning Claude Code; from here on the kind is written.
            self.kind = rush_core::adapter::migrated("").0;
            self.account.kind = rush_core::adapter::Kind::from(self.kind.as_str());
        }
        if self.session_id.is_empty() {
            (self.session_id, self.id) = new_session_id();
        }
        if self.id.is_empty() && self.fork {
            self.id = new_session_id().1; // the copy is a session of its own
        }
        if self.id.is_empty() {
            self.id = short_id(&self.session_id);
        }
    }
}

/// Whether info's host runs, for a process about to start that session.
/// Pids start over when a machine restarts, so a saved host pid can name
/// another process: the very one asking, or any that started after the
/// host did, as a host writes its info once it runs. Either is a host
/// gone with the restart.
pub fn running(info: &Info) -> bool {
    if info.host_pid == std::process::id() as i64 || !crate::alive(info.host_pid) {
        return false;
    }
    // Linux counts a start in hundredths from a boot time in whole seconds.
    let Some(at) = rush_sys::proc::started(info.host_pid as i32) else {
        return true;
    };
    if info.started_at.is_zero() {
        return true;
    }
    let at = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let at = Time::from_unix(at.as_secs() as i64, at.subsec_nanos());
    !at.after(info.started_at.add_nanos(2_000_000_000))
}

/// Writes cfg and starts its host as a detached process, returning once
/// the host is accepting connections. cfg.id and cfg.session_id are
/// filled in for a new conversation when empty.
pub fn spawn(cfg: Config) -> Result<Config> {
    spawn_on(cfg, None)
}

/// spawn on the rush at exe, this one when None. Given one, it also makes
/// sure the host that answers was started from it, and ends a host that
/// never came up rather than leave it: restart starts another.
pub(crate) fn spawn_on(mut cfg: Config, exe: Option<&Path>) -> Result<Config> {
    cfg.fill_ids();
    let d = dir(&cfg.id);
    mkdir(&d)?;
    if let Ok(info) = read_info(&cfg.id) {
        if info.is_remote() {
            return Err(format!(
                "{} is a session on {}, not one this machine can start",
                cfg.id, info.remote
            )
            .into());
        }
        if running(&info) {
            return Err(format!("{} is already running", cfg.id).into());
        }
    }
    crate::json::write_private(&d.join("config.json"), &cfg.to_json_indent())?;
    let checked = exe.is_some();
    let exe: PathBuf = match exe {
        Some(e) => e.to_path_buf(),
        None => std::env::current_exe()?,
    };
    let want = if checked {
        BinStamp::of(&exe)
    } else {
        BinStamp::default()
    };
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.join("host.log"))?;
    let _ = std::fs::remove_file(sock_path(&cfg.id));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["host", "run", &cfg.id])
        .current_dir(if cfg.cwd.is_empty() { "/" } else { &cfg.cwd })
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    let cause = if cfg.cause.is_empty() {
        rush_events::cause()
    } else {
        cfg.cause.clone()
    };
    if !cause.is_empty() {
        cmd.env("RUSH_CAUSE", &cause);
    }
    // Its own session, so closing rush or its terminal leaves it running.
    // SAFETY: setsid is async-signal-safe and touches nothing of ours.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id() as i64;
    // Reap it when it ends, or a stopped host lingers as a zombie that
    // still looks alive for as long as this process runs.
    let stop = Arc::new(rush_sys::fswait::Stop::new()?);
    let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (stop, exited) = (stop.clone(), exited.clone());
        std::thread::spawn(move || {
            let _ = child.wait();
            exited.store(true, std::sync::atomic::Ordering::SeqCst);
            stop.close();
        });
    }
    let end = |err: String| -> Result<Config> {
        if checked {
            // its own process group: see setsid.
            let _ = rush_sys::proc::kill_group(pid as i32, libc::SIGKILL);
            crate::sys::wait_exit(pid, Duration::from_secs(5));
        }
        Err(err.into())
    };
    // It's up once its socket answers and its info names it: the folder
    // changes then, and the watcher says so, with no polling.
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[&d]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if UnixStream::connect(sock_path(&cfg.id)).is_ok()
            && let Ok(info) = read_info(&cfg.id)
            && info.host_pid == pid
        {
            if checked && info.exe != want {
                return end(format!(
                    "host for {} isn't running the rush at {}",
                    cfg.id,
                    exe.display()
                ));
            }
            return Ok(cfg);
        }
        if exited.load(std::sync::atomic::Ordering::SeqCst) {
            let tail = std::fs::read_to_string(d.join("host.log")).unwrap_or_default();
            return Err(format!("host for {} exited: {}", cfg.id, last_line(&tail)).into());
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        w.wait(Some(&stop), Some(deadline - now));
    }
    end(format!(
        "host for {} (pid {pid}) did not start listening",
        cfg.id
    ))
}

fn mkdir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(p)
}

/// The last line of s, its trailing newlines and spaces left off.
pub fn last_line(s: &str) -> &str {
    let s = s.trim_end_matches(['\n', ' ']);
    match s.rfind('\n') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// A session's last published info. A session whose host has gone is
/// reported stopped, unless it deliberately went to sleep.
pub fn read_info(id: &str) -> std::io::Result<Info> {
    let mut info = read_info_file(id)?;
    if !crate::alive(info.host_pid) {
        crate::revive::lose(&mut info);
    }
    Ok(info)
}

/// info.json as written, brought up to date.
pub fn read_info_file(id: &str) -> std::io::Result<Info> {
    let b = std::fs::read(dir(id).join("info.json"))?;
    let mut info = Info::parse(&b).map_err(std::io::Error::other)?;
    drop_dismissed(id, &mut info);
    Ok(info)
}

/// The file whose contents name the decisions card set aside while the
/// session slept.
pub const DECISIONS_GONE: &str = "decisions-dismissed";

/// Takes away a decisions card set aside while its host was gone
/// (decisions.go:431).
fn drop_dismissed(id: &str, info: &mut Info) {
    if let Some(d) = &info.decisions
        && let Ok(b) = std::fs::read_to_string(dir(id).join(DECISIONS_GONE))
        && b.trim() == d.id
    {
        info.decisions = None;
    }
}

/// The config a session was started with, to restart it.
pub fn read_config(id: &str) -> std::io::Result<Config> {
    let b = std::fs::read(dir(id).join("config.json"))?;
    Config::parse(&b).map_err(std::io::Error::other)
}

/// Every rush-mode session, newest first.
pub fn list() -> Vec<Info> {
    Lister::default().list()
}

struct Listed {
    mod_: Option<SystemTime>,
    size: u64,
    info: Option<Info>,
    /// The listing that last looked at it (see Lister::trust).
    at: u64,
    /// Its host was found gone, as last listed.
    dead: bool,
}

/// Says whether a path is unchanged since a listing numbered at looked
/// at it.
pub type Trust = Box<dyn Fn(&Path, u64) -> bool + Send>;

/// Lists sessions again and again, parsing only the info files that
/// changed; every session ever started stays listed, so most are
/// unchanged. A listing with nothing changed is the last one again.
#[derive(Default)]
pub struct Lister {
    infos: HashMap<String, Listed>,
    /// When set, says whether path is unchanged since a listing numbered
    /// at looked at it (a watcher's word): a session's folder unchanged,
    /// its info is as it was without a look. at numbers this listing.
    pub trust: Option<Trust>,
    pub at: u64,
    ids: Option<Vec<String>>,
    ids_at: u64,
    last: Option<Arc<Vec<Info>>>,
}

impl Lister {
    pub fn list(&mut self) -> Vec<Info> {
        self.list_shared().as_ref().clone()
    }

    /// The listing, shared with the last when nothing changed.
    pub fn list_shared(&mut self) -> Arc<Vec<Info>> {
        let t = self.trust.take();
        let at = self.at;
        let out = self.list_trusting(
            &|p: &Path, at: u64| t.as_ref().is_some_and(|f| f(p, at)),
            at,
        );
        self.trust = t;
        out
    }

    /// The listing numbered at, trusting what fresh says is unchanged
    /// since the listing that last looked at it; shared with the last
    /// when nothing changed.
    pub fn list_trusting(&mut self, fresh: &dyn Fn(&Path, u64) -> bool, at: u64) -> Arc<Vec<Info>> {
        self.at = at;
        let trust = |p: &Path, at: u64| fresh(p, at);
        let mut same = self.last.is_some();
        if self.ids.is_none() || !trust(&root(), self.ids_at) {
            let mut ids: Vec<String> = std::fs::read_dir(root())
                .map(|es| {
                    es.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            ids.sort();
            same = same && self.ids.as_ref() == Some(&ids);
            self.ids = Some(ids);
            self.ids_at = self.at;
        }
        let mut stand = Vec::new();
        let mut n = 0;
        let ids = self.ids.clone().unwrap_or_default();
        for id in &ids {
            let known = self.infos.contains_key(id);
            let trusted = self.infos.get(id).is_some_and(|c| trust(&dir(id), c.at));
            if !trusted {
                let Ok(st) = std::fs::metadata(dir(id).join("info.json")) else {
                    if known {
                        self.infos.remove(id);
                        same = false;
                    }
                    continue;
                };
                let (m, size) = (st.modified().ok(), st.len());
                let changed = self
                    .infos
                    .get(id)
                    .is_none_or(|c| c.mod_ != m || c.size != size);
                if changed {
                    let dead = self.infos.get(id).is_some_and(|c| c.dead);
                    self.infos.insert(
                        id.clone(),
                        Listed {
                            mod_: m,
                            size,
                            info: read_info_file(id).ok(),
                            at: 0,
                            dead,
                        },
                    );
                    same = false;
                }
                if let Some(c) = self.infos.get_mut(id) {
                    c.at = self.at;
                }
            }
            let Some(c) = self.infos.get_mut(id) else {
                continue;
            };
            let Some(info) = &c.info else { continue };
            let dead = info.state != "stopped" && !crate::alive(info.host_pid);
            same = same && dead == c.dead;
            c.dead = dead;
            if info.is_remote() {
                stand.push(info.host_pid as i32);
            }
            n += 1;
        }
        rush_sys::proc::protect(&stand); // the attach process behind them is no agent's to signal
        if same
            && let Some(last) = &self.last
            && n == last.len()
        {
            return last.clone();
        }
        let mut out = Vec::with_capacity(n);
        for id in &ids {
            let Some(c) = self.infos.get(id) else {
                continue;
            };
            let Some(info) = &c.info else { continue };
            let mut info = info.clone();
            if c.dead {
                crate::revive::lose(&mut info);
            }
            out.push(info);
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        let out = Arc::new(out);
        self.last = Some(out.clone());
        out
    }

    /// How many live sessions' hosts are on an older rush than the one
    /// installed, how many of those it restarted (when restart is set)
    /// because they were between turns, and how many of those restarts
    /// failed. It reads every session's info and restarts hosts, so never
    /// on the UI thread (stale.go:59).
    pub fn sweep(&mut self, installed: BinStamp, restart_due: bool) -> (usize, usize, usize) {
        let (mut stale, mut restarted, mut failed) = (0, 0, 0);
        for i in self.list() {
            // A sleeping one wakes on the new binary anyway.
            if i.state == "stopped" || i.sleeping || !i.stale(installed) {
                continue;
            }
            stale += 1;
            if !restart_due || !crate::wire::restart_due(&i, installed) {
                continue;
            }
            match read_config(&i.id) {
                Ok(c) if c.owner == 0 => {}
                _ => continue, // a stand-in's host ends with the program that started it
            }
            // Read again: a message may have come since the list was read.
            match read_info(&i.id) {
                Ok(now) if crate::wire::restart_due(&now, installed) => {}
                _ => continue,
            }
            if let Err(e) = restart(&i.id, |_| {}) {
                failed += 1;
                log_event(&i.id, "restart-failed", json!({"err": e.to_string()}));
                continue;
            }
            restarted += 1;
            stale -= 1;
        }
        (stale, restarted, failed)
    }
}

/// A connection to one session's host.
pub struct Client {
    /// Every line from the host, replay first; it ends when the connection
    /// does.
    pub lines: Receiver<Vec<u8>>,
    /// The ev of what this client's ops come of, for the host's events;
    /// "" is this process's own cause.
    pub cause: String,
    w: Mutex<UnixStream>,
}

impl Client {
    /// Connects to a running session.
    pub fn dial(id: &str) -> Result<Client> {
        Client::dial_proto(id, crate::wire::PROTO)
    }

    /// Connects as a rush from before the hello: a Claude Code session's
    /// lines come as Claude Code wrote them.
    pub fn dial_raw(id: &str) -> Result<Client> {
        Client::dial_proto(id, 0)
    }

    /// Connects saying hello as a client of protocol proto; 0 says none.
    pub fn dial_proto(id: &str, proto: i64) -> Result<Client> {
        let sock = sock_path(id);
        rush_core::sandbox::guard("a host's socket", &sock);
        let c = UnixStream::connect(sock)?;
        let r = c.try_clone()?;
        let (tx, rx) = sync_channel(1024);
        std::thread::Builder::new()
            .name("host-client".into())
            .spawn(move || {
                let mut r = BufReader::with_capacity(64 << 10, r);
                let mut l = Vec::new();
                loop {
                    l.clear();
                    match r.read_until(b'\n', &mut l) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let line = l.strip_suffix(b"\n").unwrap_or(&l).to_vec();
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            })?;
        let cl = Client {
            lines: rx,
            cause: String::new(),
            w: Mutex::new(c),
        };
        if proto > 0 {
            let mut o = Op::named("hello");
            o.proto = proto;
            cl.send_op(o)?;
        }
        Ok(cl)
    }

    /// Sends one op, with this client's cause.
    pub fn send_op(&self, mut o: Op) -> Result<()> {
        o.cause = if self.cause.is_empty() {
            rush_events::cause()
        } else {
            self.cause.clone()
        };
        let mut b = crate::json::line(&o);
        b.push(b'\n');
        let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(&b)?;
        Ok(())
    }

    fn op(&self, f: impl FnOnce(&mut Op), name: &str) -> Result<()> {
        let mut o = Op::named(name);
        f(&mut o);
        self.send_op(o)
    }

    /// Delivers a message, or queues it if the agent is busy.
    pub fn send(&self, text: &str) -> Result<()> {
        self.op(|o| o.text = text.into(), "send")
    }

    /// Delivers a message mid-turn: the agent reads it at its next step.
    pub fn send_now(&self, text: &str) -> Result<()> {
        self.op(
            |o| {
                o.text = text.into();
                o.now = true;
            },
            "send",
        )
    }

    /// Hands a message to the turn under way without stopping it, where
    /// the agent can take one; else it's a send.
    pub fn send_guide(&self, text: &str, paths: &[String]) -> Result<()> {
        self.op(
            |o| {
                o.text = text.into();
                o.images = paths.to_vec();
                o.guide = true;
            },
            "send",
        )
    }

    /// Delivers a message with image files attached, or queues it with
    /// them; now sends it mid-turn instead.
    pub fn send_images(&self, text: &str, paths: &[String], now: bool) -> Result<()> {
        self.op(
            |o| {
                o.text = text.into();
                o.images = paths.to_vec();
                o.now = now;
            },
            "send",
        )
    }

    // Queue edits name a message by its place and its text as you saw it,
    // so they still find it if the queue moved in between.

    pub fn edit_queued(&self, i: i64, was: &str, text: &str) -> Result<()> {
        self.op(
            |o| {
                o.index = i;
                o.was = was.into();
                o.text = text.into();
            },
            "queue_edit",
        )
    }
    pub fn remove_queued(&self, i: i64, was: &str) -> Result<()> {
        self.op(
            |o| {
                o.index = i;
                o.was = was.into();
            },
            "queue_remove",
        )
    }
    pub fn move_queued(&self, i: i64, was: &str, to: i64) -> Result<()> {
        self.op(
            |o| {
                o.index = i;
                o.was = was.into();
                o.to = to;
            },
            "queue_move",
        )
    }
    pub fn merge_queued(&self, i: i64, was: &str) -> Result<()> {
        self.op(
            |o| {
                o.index = i;
                o.was = was.into();
            },
            "queue_merge",
        )
    }
    pub fn send_queued(&self, i: i64, was: &str) -> Result<()> {
        self.op(
            |o| {
                o.index = i;
                o.was = was.into();
            },
            "queue_send",
        )
    }
    /// Hands queued message i to the turn under way without stopping it.
    pub fn steer_queued(&self, i: i64, was: &str) -> Result<()> {
        self.op(
            |o| {
                o.index = i;
                o.was = was.into();
                o.guide = true;
            },
            "queue_send",
        )
    }
    /// Pauses or resumes sending the queue.
    pub fn hold_queue(&self, on: bool) -> Result<()> {
        self.op(|o| o.now = on, "queue_hold")
    }
    /// Sends queued messages one per turn instead of together.
    pub fn queue_separately(&self, on: bool) -> Result<()> {
        self.op(|o| o.now = on, "queue_separate")
    }

    /// Lets a pending tool call run; input None keeps the requested input.
    pub fn allow(&self, id: &str, input: Option<RawJson>, always: bool) -> Result<()> {
        self.op(
            |o| {
                o.id = id.into();
                o.input = input;
                o.always = always;
            },
            "allow",
        )
    }
    pub fn deny(&self, id: &str, message: &str, interrupt: bool) -> Result<()> {
        self.op(
            |o| {
                o.id = id.into();
                o.message = message.into();
                o.interrupt = interrupt;
            },
            "deny",
        )
    }
    pub fn interrupt(&self) -> Result<()> {
        self.op(|_| {}, "interrupt")
    }
    /// Stops one subagent or background shell by its task id.
    pub fn stop_task(&self, id: &str) -> Result<()> {
        self.op(|o| o.id = id.into(), "stop_task")
    }
    pub fn set_permission_mode(&self, m: &str) -> Result<()> {
        self.op(|o| o.mode = m.into(), "mode")
    }
    /// Moves a tool call the turn is waiting on into the background by its
    /// id; empty moves them all.
    pub fn background(&self, tool_use_id: &str) -> Result<()> {
        self.op(|o| o.id = tool_use_id.into(), "background")
    }
    pub fn set_model(&self, m: &str) -> Result<()> {
        self.op(|o| o.model = m.into(), "model")
    }
    /// Tells the session its profile is now signed in as another account.
    pub fn relogin(&self) -> Result<()> {
        self.op(|_| {}, "relogin")
    }
    /// Whether a session stopped by a usage limit carries on at the reset.
    pub fn continue_at_reset(&self, yes: bool) -> Result<()> {
        self.op(|o| o.now = yes, "limit")
    }
    /// Sets the decisions card id aside, unanswered: nothing is sent.
    pub fn dismiss_decisions(&self, id: &str) -> Result<()> {
        self.op(|o| o.id = id.into(), "decisions_dismiss")
    }
    /// Leaves the session to carry on by itself; None comes back.
    pub fn set_away(&self, a: Option<Away>) -> Result<()> {
        self.op(|o| o.away = a, "away")
    }
    /// Changes effort; it applies from the agent's next start.
    pub fn set_effort(&self, e: &str) -> Result<()> {
        self.op(|o| o.effort = e.into(), "effort")
    }
    /// What the session goes without from its agent's next start: at once
    /// when it's idle.
    pub fn set_without(&self, w: &[String]) -> Result<()> {
        self.op(|o| o.without = w.to_vec(), "without")
    }
    /// Carries on from another conversation, session_id: a copy cut before
    /// one of your messages (resume), a fresh one, or one of its branches.
    /// The connection closes once it has; dial again to see it.
    pub fn rewind(&self, session_id: &str, resume: bool, left: Branch) -> Result<()> {
        self.op(
            |o| {
                o.text = session_id.into();
                o.now = resume;
                o.branch = Some(left);
            },
            "rewind",
        )
    }
    /// Names the session afresh now it works in dir.
    pub fn retitle(&self, dir: &str) -> Result<()> {
        self.op(|o| o.text = dir.into(), "retitle")
    }
    /// Moves the session to dir, where you told its agent to work.
    pub fn cd(&self, dir: &str) -> Result<()> {
        self.op(|o| o.text = dir.into(), "cd")
    }
    /// Leaves text for running subagent sub, given at its next tool call.
    pub fn tell(&self, sub: &str, text: &str) -> Result<()> {
        self.op(
            |o| {
                o.id = sub.into();
                o.text = text.into();
            },
            "tell",
        )
    }
    /// Carries the session on in a fresh conversation, session_id, that
    /// starts with prompt: a summary another model wrote of this one.
    pub fn compacted(&self, session_id: &str, prompt: &str, left: Branch) -> Result<()> {
        self.op(
            |o| {
                o.text = session_id.into();
                o.message = prompt.into();
                o.branch = Some(left);
            },
            "compacted",
        )
    }
    /// compacted, only for the idle conversation the summary was made from
    /// (protocol 9 checks it under the host's lock).
    pub fn compacted_if_unchanged(
        &self,
        session_id: &str,
        prompt: &str,
        left: Branch,
        expected: &Info,
    ) -> Result<()> {
        if expected.proto < 9 {
            return Err(
                "restart this session's host to use external compaction safely"
                    .to_string()
                    .into(),
            );
        }
        self.op(
            |o| {
                o.text = session_id.into();
                o.message = prompt.into();
                o.branch = Some(left);
                o.expected_session = expected.session_id.clone();
                o.expected_updated_at = expected.updated_at;
            },
            "compacted_checked",
        )
    }
    /// Ends the session and its host; the conversation is kept.
    pub fn stop(&self) -> Result<()> {
        self.op(|_| {}, "stop")
    }
    /// Passes a control request to the agent (req carries its subtype),
    /// waking it if it's asleep. The answer arrives on lines as a Reply
    /// with this id.
    pub fn ask(&self, id: &str, req: RawJson) -> Result<()> {
        self.op(
            |o| {
                o.id = id.into();
                o.request = Some(req);
            },
            "ask",
        )
    }
    /// Asks for a fresh count of what fills the context window; it arrives
    /// on lines as a Context.
    pub fn ask_context(&self) -> Result<()> {
        self.op(|_| {}, "context")
    }
    /// Sends an agent's message, keeping where it came from through queues.
    pub fn send_exchange(&self, e: Exchange, now: bool) -> Result<()> {
        self.op(
            |o| {
                o.text = e.text.clone();
                o.images = e.images.clone();
                o.now = now;
                o.guide = !now;
                o.exchange = Some(Box::new(e));
            },
            "send",
        )
    }
    /// Records the other side of an exchange without prompting the agent.
    pub fn record_exchange(&self, e: Exchange) -> Result<()> {
        self.op(|o| o.exchange = Some(Box::new(e)), "exchange")
    }
    /// Ends the connection.
    pub fn close(&self) {
        let w = self.w.lock().unwrap_or_else(|e| e.into_inner());
        let _ = w.shutdown(std::net::Shutdown::Both);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads c's lines until want takes one, the host closes the connection,
/// or d passes.
pub fn await_line(c: &Client, d: Duration, mut want: impl FnMut(&Line) -> bool) -> Result<()> {
    let end = Instant::now() + d;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match c.lines.recv_timeout(left) {
            Ok(l) => {
                if let Ok(ev) = decode(&l)
                    && want(&ev)
                {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Err(ClientError::Closed),
            Err(RecvTimeoutError::Timeout) => {
                return Err("the host did not answer in time".to_string().into());
            }
        }
    }
}

/// Whether a send was turned away by a host falling asleep as it came: it
/// refused it, cut it off or was already gone. Sent again, it wakes the
/// host.
pub fn resting(err: &ClientError) -> bool {
    match err {
        ClientError::Closed => true,
        ClientError::Io(e) => matches!(
            e.kind(),
            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
        ),
        ClientError::Other(s) => s.contains("session is sleeping"),
    }
}

/// Gives session id the exchange, waking it if it rests, and returns once
/// its host has taken it: sent on, or queued.
pub fn deliver(id: &str, e: &Exchange, now: bool) -> Result<()> {
    let mut last = Ok(());
    for i in 0..3 {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
        last = deliver_once(id, e, now);
        match &last {
            Err(err) if resting(err) => continue,
            _ => return last,
        }
    }
    last
}

fn deliver_once(id: &str, e: &Exchange, now: bool) -> Result<()> {
    ensure(id, "")?;
    let c = Client::dial(id)?;
    // The replay ends with the host's info: what follows answers this.
    await_line(&c, Duration::from_secs(10), |l| matches!(l, Line::Info(_)))?;
    c.send_exchange(e.clone(), now)?;
    let mut failed = None;
    await_line(&c, Duration::from_secs(5), |l| match l {
        Line::Error(s) => {
            failed = Some(s.clone());
            true
        }
        Line::Sent(s) => s.exchange.as_ref().is_some_and(|x| x.id == e.id),
        Line::Info(i) => i
            .queue_exchanges
            .iter()
            .any(|q| q.as_ref().is_some_and(|q| q.id == e.id)),
        _ => false,
    })?;
    match failed {
        Some(f) => Err(ClientError::Other(f)),
        None => Ok(()),
    }
}

/// Wakes a saved host for an explicit action, starting it with prompt as
/// its first message when one is given. Concurrent wakes share a startup
/// lock, so only one process is spawned (sleep.go:202).
pub fn ensure(id: &str, cause: &str) -> Result<()> {
    ensure_with(id, "", cause)
}

pub(crate) fn ensure_with(id: &str, prompt: &str, cause: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        return Err("invalid session ID".to_string().into());
    }
    if let Ok(info) = read_info_file(id)
        && info.is_remote()
    {
        if crate::alive(info.host_pid) {
            return Ok(()); // the machine it's on wakes it, on a send
        }
        return Err(format!(
            "{id} is on {}, and rush remote attach {} isn't running",
            info.remote, info.remote
        )
        .into());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir(id).join("wake.lock"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    flock_until(&lock, deadline)?;
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[dir(id)]);
    while let Ok(info) = read_info(id) {
        if !crate::alive(info.host_pid) {
            break;
        }
        if !info.sleeping && UnixStream::connect(sock_path(id)).is_ok() {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err("session host did not become ready or finish exiting"
                .to_string()
                .into());
        }
        // Its folder changes as it comes up or goes; its exit isn't seen
        // there, so a look a quarter-second on at most.
        w.wait(None, Some((deadline - now).min(Duration::from_millis(250))));
    }
    // Read again under the lock: another start or model change may have
    // updated it.
    let mut cfg = read_config(id)?;
    cfg.prompt = prompt.to_string();
    cfg.resume = cfg.resume || !prompt.is_empty();
    cfg.images.clear();
    cfg.prompt_exchange = None;
    cfg.owner = 0;
    cfg.cause = cause.to_string();
    spawn(cfg).map(|_| ())
}

/// Takes f's exclusive lock, waiting until deadline; the lock is let go
/// when f closes.
fn flock_until(f: &std::fs::File, deadline: Instant) -> Result<()> {
    use std::os::fd::AsRawFd;
    let fd = f.as_raw_fd();
    loop {
        // SAFETY: fd is f's, open.
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(e.into());
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for session startup".to_string().into());
        }
        // Another process is starting it; its start takes a moment.
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Ends a session's host and starts it again on the installed rush, with
/// change applied to its config first: how a host from an older rush gets
/// what's new. The installed rush is asked first whether it can run the
/// session (preflight), and the old host is left running when it can't; a
/// new host that then doesn't come up is replaced by one on the rush
/// installed before it (roll_back).
pub fn restart(id: &str, change: impl FnOnce(&mut Config)) -> Result<()> {
    let mut cfg = read_config(id)?;
    let exe = crate::reload::installed_exe()?;
    if let Err(e) = crate::reload::preflight(&exe, id) {
        log_event(
            id,
            "reload-refused",
            json!({"exe": exe.to_string_lossy(), "err": e}),
        );
        return Err(
            format!("the installed rush can't run {id}, so its host carries on: {e}").into(),
        );
    }
    let info = read_info(id).unwrap_or_default();
    // Its stop and its start again come of this.
    let ev = rush_events::id();
    log_event(id, "restart", json!({"ev": ev, "host": info.host_pid}));
    if let Ok(mut c) = Client::dial(id) {
        c.cause = ev.clone();
        let _ = c.stop();
        c.close();
    }
    if !crate::sys::wait_exit(info.host_pid, Duration::from_secs(15)) {
        return Err(format!("the host for {id} didn't stop").into());
    }
    change(&mut cfg);
    cfg.prompt.clear();
    cfg.images.clear();
    cfg.cause = ev;
    if let Err(e) = spawn_on(cfg.clone(), Some(&exe)) {
        return Err(crate::reload::roll_back(&cfg, &exe, &e.to_string()).into());
    }
    Ok(())
}

/// Rewind for a host from before it could (Proto 0): the host is
/// restarted on this rush, already carrying on from session_id, with the
/// path it leaves kept as a branch.
pub fn rewind_by_restart(id: &str, session_id: &str, resume: bool, left: Branch) -> Result<()> {
    restart(id, |cfg| {
        let reg = rush_core::adapter::global();
        let kept = reg
            .get(&cfg.kind)
            .and_then(|a| {
                a.brancher()
                    .map(|b| b.transcript_path(&cfg.account, &cfg.cwd, &cfg.session_id))
            })
            .is_some_and(|p| std::fs::metadata(p).is_ok());
        cfg.rewind_to(session_id, resume, Some(&left), kept);
    })
}

/// Stops session id and every session it started with spawn_agent, theirs
/// in turn: nothing it set going works on unseen. A spawned session's
/// stops even while it sleeps. (The pull requests it follows are
/// prdrive's to stop, not yet ported.)
pub fn stop_all(id: &str) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    stop_all_seen(id, &mut seen)
}

fn stop_all_seen(id: &str, seen: &mut std::collections::HashSet<String>) -> Result<()> {
    if !seen.insert(id.to_string()) {
        return Ok(());
    }
    let mut errs = Vec::new();
    if let Ok(c) = Client::dial(id) {
        if let Err(e) = c.stop() {
            errs.push(e.to_string());
        }
        c.close();
    }
    for i in list() {
        if i.is_remote() {
            continue;
        }
        // One asleep or stopped is still looked through: what it spawned
        // may be at work.
        if let Ok(cfg) = read_config(&i.id)
            && cfg.meta.get("spawnedBy").is_some_and(|s| s == id)
            && let Err(e) = stop_all_seen(&i.id, seen)
        {
            errs.push(e.to_string());
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n").into())
    }
}
