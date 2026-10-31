//! `rush menubar feed`: writes the menu's state as it changes and carries
//! out the ops the app sends, one JSON object a line. Port of
//! internal/menubar/feed.go:131-520 (Feed, build, doing, answerable, do,
//! oneLine, windows).
//!
//! Go reads everything again every two seconds. This waits instead: on
//! the files the reading came from (kqueue, rush-sys's fswait), on the
//! app's ops and on the readings of limits it asked for. While an agent
//! is live (working, or waiting on you) it reads again every two seconds
//! as well, as rush's list does (a working agent's transcript grows
//! without its session files changing, and one waiting can be closed
//! without a file saying so), and asks the providers for limits once a
//! minute; idle, it wakes only when a limit shown resets, so the menu
//! never shows a used-up window past its reset.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_core::agent::State as Doing;
use rush_core::time::now;
use rush_core::usage::{Quota, Reading};
use rush_sys::fswait::{Stop, Watcher};
use serde_json::{Map, Value, json};

use crate::host::{self, Asking, Client, Info};
use crate::source::{Row, Snapshot, Source};
use crate::wire::{self, Account, Op, State, TOP, Waiting, Window, Working};

/// How often a reading is made again while an agent works.
const POLL: Duration = Duration::from_secs(2);
/// How often limits are asked for while an agent is live.
const LIMITS: Duration = Duration::from_secs(60);

/// What a waiting rush session is asking, as its host has it.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    /// Needs when it was read; a change means ask again.
    sig: String,
    host: String,
    req: Option<Asking>,
    info: Info,
}

/// What the feed knows each waiting session asks, by row key.
pub type Asked = HashMap<String, Pending>;

/// How the feed reaches sessions' hosts: the real ones, or a test's.
pub trait Hosts: Send + Sync {
    /// Connects to session id's host.
    fn dial(&self, id: &str) -> std::io::Result<Client>;
    /// Records what came of an op: in the session's own events when it
    /// names one (host ""), and in rush's.
    fn log(&self, host: &str, kind: &str, f: Map<String, Value>) {
        if host.is_empty() {
            rush_events::log(kind, f);
        } else {
            rush_events::session(host, &host::events_path(host), kind, f);
        }
    }
}

/// The hosts on this machine.
pub struct Local;

impl Hosts for Local {
    fn dial(&self, id: &str) -> std::io::Result<Client> {
        Client::dial(id)
    }
}

/// A quota's two windows for the feed: the first two of those that limit
/// every model.
pub fn windows(q: &Quota) -> (Window, Window) {
    let mut out = q
        .windows
        .iter()
        .filter(|w| w.scope.models.is_empty())
        .map(|w| Window {
            present: true,
            percent: w.percent,
            resets_at: w.resets_at,
            label: w.label.clone(),
            name: w.name.clone(),
        });
    (
        out.next().unwrap_or_default(),
        out.next().unwrap_or_default(),
    )
}

/// s's first line, trimmed, with " …" when there was more.
pub fn one_line(s: &str) -> String {
    let s = s.trim();
    match s.find('\n') {
        Some(i) => format!("{} …", s[..i].trim()),
        None => s.to_string(),
    }
}

fn first_non_empty<'a>(vs: &[&'a str]) -> &'a str {
    vs.iter().copied().find(|v| !v.is_empty()).unwrap_or("")
}

/// The menu's state from a reading. Sessions no longer waiting are
/// forgotten from asked.
pub fn build(snap: &Snapshot, asked: &mut Asked, hosts: &dyn Hosts) -> State {
    let mut s = State::default();
    if !snap.logins.is_empty() {
        // Accounts are the logins ~/.claude can be signed in as; the one in
        // use has the live agents.
        let live: i64 = snap.accounts.iter().map(|a| a.live).sum();
        for lv in &snap.logins {
            let (five_hour, seven_day) = windows(&lv.quota);
            s.accounts.push(Account {
                name: lv.name.clone(),
                email: lv.email.clone(),
                plan: lv.usage.plan.clone(),
                problem: lv.usage.problem.clone(),
                current: lv.current,
                live: if lv.current { live } else { 0 },
                five_hour,
                seven_day,
            });
        }
    } else {
        for av in &snap.accounts {
            let (five_hour, seven_day) = windows(&av.quota);
            s.accounts.push(Account {
                name: av.name.clone(),
                email: av.usage.email.clone(),
                plan: av.usage.plan.clone(),
                problem: av.usage.problem.clone(),
                live: av.live,
                current: av.current,
                five_hour,
                seven_day,
            });
        }
    }
    for o in &snap.others {
        let (five_hour, seven_day) = windows(&o.quota);
        let live = snap
            .agents
            .iter()
            .filter(|a| a.kind == o.kind && a.live())
            .count() as i64;
        s.accounts.push(Account {
            name: o.name.clone(),
            email: o.quota.email.clone(),
            plan: first_non_empty(&[&o.quota.plan, &o.quota.balance]).to_string(),
            problem: o.quota.problem.clone(),
            live,
            five_hour,
            seven_day,
            current: false,
        });
    }
    let mut live = std::collections::HashSet::new();
    let mut working: Vec<&Row> = Vec::new();
    for a in &snap.agents {
        if a.checking {
            continue;
        }
        if a.state == Doing::Blocked && (a.pid != 0 || a.decides) {
            live.insert(a.key.clone());
            let mut w = Waiting {
                key: a.key.clone(),
                name: a.display_name.clone(),
                needs: one_line(&a.needs),
                account: a.account.clone(),
                seen: a.seen,
                since: a.updated_at,
                ..Waiting::default()
            };
            if w.needs.is_empty() {
                w.needs = one_line(&a.detail);
            }
            if a.rush && a.pid != 0 {
                // Asleep, it has nothing to ask its host.
                answerable(&mut w, a, asked, hosts);
            }
            s.waiting.push(w);
        } else if a.state == Doing::Working && a.pid != 0 {
            working.push(a);
        }
    }
    asked.retain(|k, _| live.contains(k));
    // Longest-running first: the ones you started earliest and are waiting
    // on most.
    working.sort_by_key(|a| a.created_at);
    if working.len() > TOP {
        s.more = (working.len() - TOP) as i64;
    }
    for a in working.into_iter().take(TOP) {
        s.working.push(Working {
            key: a.key.clone(),
            name: a.display_name.clone(),
            doing: doing(a),
            repo: a.repo.clone(),
            account: a.account.clone(),
            since: a.created_at,
        });
    }
    s
}

/// What a working agent is doing now, in a few words.
fn doing(a: &Row) -> String {
    let d = one_line(&a.detail);
    if !d.is_empty() {
        return d;
    }
    if !a.transcript_path.is_empty() {
        let p = rush_core::adapter::global().read_preview(&a.kind, &a.transcript_path, 64 << 10);
        if !p.doing.is_empty() {
            return one_line(&p.doing);
        }
    }
    if a.interactive {
        return "working in a terminal".into();
    }
    "working…".into()
}

/// Fills in how a rush session's wait can be answered, asking its host
/// what it's waiting on when that changed.
fn answerable(w: &mut Waiting, a: &Row, asked: &mut Asked, hosts: &dyn Hosts) {
    let fresh = asked.get(&a.key).is_none_or(|p| p.sig != a.needs);
    if fresh {
        let (req, info) = hosts
            .dial(&a.id)
            .map(|c| host::read_wait(&c))
            .unwrap_or_default();
        asked.insert(
            a.key.clone(),
            Pending {
                sig: a.needs.clone(),
                host: a.id.clone(),
                req,
                info,
            },
        );
    }
    let p = &asked[&a.key];
    if p.info.limit.as_ref().is_some_and(|l| l.ask) {
        w.kind = "limit".into();
        return;
    }
    let Some(req) = &p.req else {
        return;
    };
    match &req.q {
        Some(q) => {
            w.req = req.id.clone();
            let Some(first) = q.asks.first() else {
                return;
            };
            w.header = first.header.clone();
            w.text = first.text.trim().to_string();
            // One single-choice question can be answered from a
            // notification; several, or ticking many, need rush.
            if q.asks.len() == 1 && !first.multi && !first.options.is_empty() {
                w.kind = "question".into();
                w.options = first.options.iter().map(|o| o.label.clone()).collect();
            }
        }
        None => {
            w.kind = "permission".into();
            w.req = req.id.clone();
            w.text = req.reason.trim().to_string();
        }
    }
}

/// The answer to an op whose question has gone.
pub const GONE: &str = "that question has already been answered";

/// Carries out an op on the session it names.
pub fn do_op(o: &Op, p: Option<&Pending>, hosts: &dyn Hosts) -> Result<(), String> {
    let Some(p) = p else {
        return Err(GONE.into());
    };
    let c = hosts.dial(&p.host).map_err(|e| e.to_string())?;
    // The replay isn't wanted: dropping the client ends its reader.
    if o.op == "limit" {
        return c.continue_at_reset(o.yes).map_err(|e| e.to_string());
    }
    let Some(req) = p.req.as_ref().filter(|r| r.id == o.req) else {
        return Err(GONE.into());
    };
    let res = match o.op.as_str() {
        "answer" => {
            let Some(q) = req.q.as_ref().filter(|q| !q.asks.is_empty()) else {
                return Err(GONE.into());
            };
            let input = host::answer_input(q, &[(q.asks[0].text.clone(), o.answer.clone())]);
            c.allow(&req.id, Some(&input))
        }
        "allow" => c.allow(&req.id, None),
        "deny" => c.deny(&req.id),
        _ => Ok(()),
    };
    res.map_err(|e| e.to_string())
}

/// How long until the soonest of the limits shown resets after at, a
/// second past it so the reading then has it reset; None with none to
/// come.
pub fn next_reset(s: &State, at: rush_core::time::Ms) -> Option<Duration> {
    s.accounts
        .iter()
        .flat_map(|a| [&a.five_hour, &a.seven_day])
        .filter(|w| w.present && w.resets_at > at)
        .map(|w| w.resets_at - at)
        .min()
        .map(|ms| Duration::from_millis(ms as u64) + Duration::from_secs(1))
}

/// What wakes the feed.
enum Wake {
    /// Files it read from changed; None when the watch can't tell which.
    Files(Option<Vec<PathBuf>>),
    /// A reading of an account's limits came back.
    Fetched(String, Box<Reading>),
    /// Something to tell the app of.
    Error(String),
    /// The app has gone: its ops ended.
    Done,
}

/// Writes the menu's state to out as it changes, and carries out ops read
/// from input until it ends. Takes the feed's lock while it runs.
pub fn feed(
    input: impl BufRead + Send + 'static,
    out: &mut dyn Write,
    source: Box<dyn Source>,
    hosts: Arc<dyn Hosts>,
) -> std::io::Result<()> {
    let _held = crate::bundle::hold();
    run(input, out, source, hosts)
}

/// Feed's loop, without the lock: what the tests run.
pub fn run(
    input: impl BufRead + Send + 'static,
    out: &mut dyn Write,
    mut source: Box<dyn Source>,
    hosts: Arc<dyn Hosts>,
) -> std::io::Result<()> {
    let asked: Arc<Mutex<Asked>> = Arc::default();
    let (tx, rx) = sync_channel::<Wake>(64);
    read_ops(input, asked.clone(), hosts.clone(), tx.clone());
    // Many: every session's folder, through one stream on their folder.
    let watcher = Arc::new(Watcher::many());
    let stop = Arc::new(Stop::new()?);
    watch_files(watcher.clone(), stop.clone(), tx.clone());
    let end = EndWatch(stop.clone());

    let mut last = String::new();
    let mut limits_at: Option<Instant> = None;
    loop {
        // Watched before reading: a change from here on wakes the wait.
        watcher.watch(&source.watched());
        let at = now();
        let snap = source.snapshot(at);
        let any_live = snap.agents.iter().any(Row::live);
        if limits_at.is_none_or(|t| any_live && t.elapsed() >= LIMITS) {
            limits_at = Some(Instant::now());
            for f in source.refresh() {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let (k, r) = f();
                    let _ = tx.send(Wake::Fetched(k, Box::new(r)));
                });
            }
        }
        let s = build(
            &snap,
            &mut asked.lock().unwrap_or_else(|e| e.into_inner()),
            &*hosts,
        );
        let b = serde_json::to_string(&s).map_err(std::io::Error::other)?;
        if b != last {
            out.write_all(b.as_bytes())?;
            out.write_all(b"\n")?;
            out.flush()?;
            last = b;
        }
        // Waits for a reason to read again: idle, only files, ops,
        // readings and the next reset of a limit shown.
        let mut wait = next_reset(&s, at);
        if any_live {
            let next = LIMITS.saturating_sub(limits_at.map_or(LIMITS, |t| t.elapsed()));
            let soonest = POLL.min(next);
            wait = Some(wait.map_or(soonest, |w| w.min(soonest)));
        }
        let read_at = Instant::now();
        let first = match wait {
            Some(d) => match rx.recv_timeout(d) {
                Ok(w) => Some(w),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => match rx.recv() {
                Ok(w) => Some(w),
                Err(_) => return Ok(()),
            },
        };
        // Takes everything that came; and, as Go reads every couple of
        // seconds at most, what else comes until two seconds have passed
        // since the last reading, so a flood of writes is one reading.
        // Errors are told as they come.
        let mut changed: Option<Vec<PathBuf>> = Some(Vec::new());
        let mut next = first;
        loop {
            let w = match next.take() {
                Some(w) => w,
                None => match rx.try_recv() {
                    Ok(w) => w,
                    Err(_) => {
                        let left = POLL.saturating_sub(read_at.elapsed());
                        if left.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(left) {
                            Ok(w) => w,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return Ok(()),
                        }
                    }
                },
            };
            match w {
                Wake::Done => {
                    drop(end);
                    return Ok(());
                }
                Wake::Error(e) => {
                    out.write_all(wire::error_line(&e).as_bytes())?;
                    out.write_all(b"\n")?;
                    out.flush()?;
                }
                Wake::Fetched(k, r) => source.fetched(k, *r),
                Wake::Files(None) => changed = None,
                Wake::Files(Some(p)) => {
                    if let Some(c) = changed.as_mut() {
                        c.extend(p);
                    }
                }
            }
        }
        match changed {
            None => source.changed(None),
            Some(c) if !c.is_empty() => source.changed(Some(&c)),
            Some(_) => {}
        }
    }
}

/// Closes stop when dropped, so the watching thread ends with the feed.
struct EndWatch(Arc<Stop>);

impl Drop for EndWatch {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Tells the feed what the watcher sees change, until stop closes.
fn watch_files(w: Arc<Watcher>, stop: Arc<Stop>, tx: SyncSender<Wake>) {
    std::thread::Builder::new()
        .name("menubar-watch".into())
        .spawn(move || {
            while w.wait(Some(&stop), None) {
                let (paths, all) = w.changes();
                let wake = if all {
                    Wake::Files(None)
                } else if paths.is_empty() {
                    continue;
                } else {
                    Wake::Files(Some(paths.into_iter().collect()))
                };
                if tx.send(wake).is_err() {
                    return;
                }
            }
        })
        .expect("a thread to watch files");
}

/// Reads the app's ops, one a line, and carries them out; Done once they
/// end.
fn read_ops(
    input: impl BufRead + Send + 'static,
    asked: Arc<Mutex<Asked>>,
    hosts: Arc<dyn Hosts>,
    tx: SyncSender<Wake>,
) {
    std::thread::Builder::new()
        .name("menubar-ops".into())
        .spawn(move || {
            for l in input.split(b'\n') {
                let Ok(l) = l else { break };
                // Lines past a megabyte are dropped, as Go's scanner's
                // buffer would end the feed.
                let Ok(o) = serde_json::from_slice::<Op>(&l) else {
                    continue;
                };
                if o.op == "show" {
                    if let Err(e) = crate::terminal::show(&o.key) {
                        let _ = tx.send(Wake::Error(format!("opening rush: {e}")));
                    }
                    continue;
                }
                let p = asked
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&o.key)
                    .cloned();
                let res = do_op(&o, p.as_ref(), &*hosts);
                let mut f = fields(json!({
                    "op": o.op, "key": o.key, "req": o.req, "why": "a click in the menu bar",
                }));
                if let Err(e) = &res {
                    f.insert("err".into(), Value::String(e.clone()));
                    let _ = tx.send(Wake::Error(e.clone()));
                }
                hosts.log(p.as_ref().map_or("", |p| &p.host), "menubar-op", f);
            }
            let _ = tx.send(Wake::Done);
        })
        .expect("a thread to read the app's ops");
}

fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// A reading that never changes, so a test's feed runs without the
/// machine's agents.
#[cfg(test)]
pub(crate) struct Fixed(pub Snapshot);

#[cfg(test)]
impl Source for Fixed {
    fn snapshot(&mut self, _: rush_core::time::Ms) -> Snapshot {
        self.0.clone()
    }
    fn watched(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn changed(&mut self, _: Option<&[PathBuf]>) {}
    fn refresh(&mut self) -> Vec<crate::source::Fetch> {
        vec![]
    }
    fn fetched(&mut self, _: String, _: Reading) {}
}

#[cfg(test)]
mod tests;
