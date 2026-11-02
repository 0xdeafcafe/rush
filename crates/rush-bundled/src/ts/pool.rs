//! The servers this plugin runs: one per checkout, shared by every agent
//! that asks about it, started through the gate and stopped past their
//! memory cap or once idle.
//! Ported from internal/bundled/ts/pool.go:1-205.
//!
//! Go's watcher ticks every two seconds for the plugin's life. Here it
//! ticks only while a server runs, and sleeps on a condvar otherwise.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use rush_plugin::Ctx;

use super::engine::{resolve, workspace_root};
use super::server::Server;
use crate::gatecore::{self, Ask, HELD_ENV, Rule};
use crate::{Got, wait_on};

/// Records what the pool did to a server and why.
pub type Log = Arc<dyn Fn(&str, rush_events::Fields) + Send + Sync>;

/// The pool's options, from the plugin's settings.
#[derive(Clone, Default)]
pub struct Options {
    /// "auto", "tsrs" or "tsgo"
    pub engine: String,
    /// bytes one server may hold; 0 for no cap
    pub cap: u64,
    /// a server asked nothing this long is closed; zero never
    pub idle: Duration,
    pub log: Option<Log>,
}

#[derive(Default)]
struct State {
    opts: Options,
    servers: BTreeMap<String, Arc<Server>>,
    /// Roots a server is starting for, and who waits on each.
    starting: BTreeMap<String, Vec<mpsc::Sender<Got<()>>>>,
    done: bool,
}

/// The servers this plugin runs.
pub struct Pool {
    st: Mutex<State>,
    /// Wakes the watcher when a server comes or the pool ends.
    wake: Condvar,
}

/// How long a call waits for memory for a new server.
const START_WAIT: Duration = Duration::from_secs(45);
/// How often the watcher looks at each server while one runs.
const WATCH_EVERY: Duration = Duration::from_secs(2);

impl Pool {
    /// A pool that watches its servers until ctx ends.
    pub fn new(ctx: &Ctx, o: Options) -> Arc<Pool> {
        let p = Arc::new(Pool {
            st: Mutex::new(State {
                opts: o,
                ..Default::default()
            }),
            wake: Condvar::new(),
        });
        {
            let p = p.clone();
            let _reg = ctx.on_cancel({
                let p = p.clone();
                move || {
                    p.lock().done = true;
                    p.wake.notify_all();
                }
            });
            std::thread::spawn(move || {
                let _reg = _reg;
                p.watch();
            });
        }
        p
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Changes the options, for servers started from now on and the next
    /// look at those running.
    pub fn set_options(&self, o: Options) {
        self.lock().opts = o;
    }

    fn options(&self) -> Options {
        self.lock().opts.clone()
    }

    /// The server for the checkout file is in, started if need be.
    pub fn server_for(&self, ctx: &Ctx, file: &str) -> Result<Arc<Server>, String> {
        let root = workspace_root(file);
        if root.is_empty() {
            return Err(format!(
                "{file} is in no checkout and under no tsconfig.json"
            ));
        }
        loop {
            let rx = {
                let mut st = self.lock();
                if let Some(s) = st.servers.get(&root)
                    && s.why().is_none()
                {
                    return Ok(s.clone());
                }
                match st.starting.get_mut(&root) {
                    Some(waiters) => {
                        let (tx, rx) = mpsc::channel();
                        waiters.push(tx.clone());
                        Some((tx, rx))
                    }
                    None => {
                        st.starting.insert(root.clone(), Vec::new());
                        None
                    }
                }
            };
            if let Some((tx, rx)) = rx {
                let _reg = ctx.on_cancel(move || {
                    let _ = tx.send(Got::Stop);
                });
                if let Err(e) = wait_on(ctx, &rx) {
                    return Err(e.to_string());
                }
                continue;
            }
            let got = self.start(ctx, &root);
            let waiters = {
                let mut st = self.lock();
                let w = st.starting.remove(&root).unwrap_or_default();
                if let Ok(s) = &got {
                    st.servers.insert(root.clone(), s.clone());
                }
                w
            };
            for w in waiters {
                let _ = w.send(Got::Val(()));
            }
            self.wake.notify_all();
            return got;
        }
    }

    /// Runs a server for root once the gate has memory for it.
    fn start(&self, ctx: &Ctx, root: &str) -> Result<Arc<Server>, String> {
        let e = resolve(root, &self.options().engine)?;
        let mut run = None;
        // Inside a gated run (a test of this, say) the gate lets it
        // through, as it does everything else there.
        if let Some(rules) = crate::gate::rules()
            && std::env::var_os(HELD_ENV).is_none_or(|v| v.is_empty())
            && let Some(rt) = gatecore::runtime()
        {
            let mut rule = rules.get(&e.name).copied().unwrap_or(Rule {
                memory: gatecore::estimate("tsrs").unwrap_or_default(),
                ..Default::default()
            });
            rule.limit = Duration::ZERO; // a server lives as long as it's used
            let ask = Ask {
                name: e.name.clone(),
                rule,
                session: "rush ts".into(),
                reserve: crate::gate::reserve(),
            };
            run = Some(rt.acquire(&ctx.with_timeout(START_WAIT), ask).map_err(|_| {
                format!(
                    "no memory for a {} language server for {root} after {}: the machine is busy; try again \
                     once other runs finish (rush gate status)",
                    e.name,
                    crate::go_duration(START_WAIT.as_nanos() as i64)
                )
            })?);
        }
        let s = Server::start(ctx, root, e.clone(), run)?;
        self.log(
            "ts-started",
            fields(&[
                ("root", root.into()),
                ("engine", e.name.into()),
                ("bin", e.bin.into()),
                ("server", s.pid().into()),
            ]),
        );
        Ok(s)
    }

    /// The servers running, by checkout.
    pub fn servers(&self) -> BTreeMap<String, Arc<Server>> {
        self.lock().servers.clone()
    }

    /// Closes every server.
    pub fn close_all(&self) {
        for (root, s) in self.servers() {
            s.close();
            self.drop_server(&root, &s);
        }
    }

    fn drop_server(&self, root: &str, s: &Arc<Server>) {
        let mut st = self.lock();
        if st.servers.get(root).is_some_and(|x| Arc::ptr_eq(x, s)) {
            st.servers.remove(root);
        }
    }

    /// Looks at each server every two seconds while any runs: what it
    /// holds goes on its gate ticket, one past its cap is stopped, one idle
    /// too long closed, and one that died is dropped, so the next call
    /// starts another.
    fn watch(&self) {
        loop {
            {
                let mut st = self.lock();
                loop {
                    if st.done {
                        drop(st);
                        self.close_all();
                        return;
                    }
                    if st.servers.is_empty() {
                        st = self.wake.wait(st).unwrap_or_else(|e| e.into_inner());
                        continue;
                    }
                    let (g, t) = self
                        .wake
                        .wait_timeout(st, WATCH_EVERY)
                        .unwrap_or_else(|e| e.into_inner());
                    st = g;
                    if t.timed_out() {
                        break;
                    }
                }
            }
            self.look();
        }
    }

    /// One look at each server, as the watcher makes while any runs.
    pub fn look(&self) {
        let o = self.options();
        for (root, s) in self.servers() {
            if let Some(why) = s.why() {
                self.drop_server(&root, &s);
                self.log(
                    "ts-gone",
                    fields(&[("root", root.into()), ("why", why.into())]),
                );
                continue;
            }
            let held = s.footprint();
            if let Some(r) = &s.run {
                r.note(held);
            }
            if o.cap > 0 && held > o.cap {
                s.kill(&cap_error(&s, held, o.cap));
                self.drop_server(&root, &s);
                self.log(
                    "ts-capped",
                    fields(&[
                        ("root", root.into()),
                        ("engine", s.engine.name.clone().into()),
                        ("footprint", held.into()),
                        ("cap", o.cap.into()),
                    ]),
                );
            } else if !o.idle.is_zero() && s.idle() > o.idle {
                s.close();
                self.drop_server(&root, &s);
                self.log(
                    "ts-idle",
                    fields(&[
                        ("root", root.into()),
                        ("idle", crate::go_duration(o.idle.as_nanos() as i64).into()),
                    ]),
                );
            }
        }
    }

    fn log(&self, kind: &str, f: rush_events::Fields) {
        if let Some(log) = self.options().log {
            log(kind, f);
        }
    }
}

fn fields(kv: &[(&str, serde_json::Value)]) -> rush_events::Fields {
    kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// What a call in flight hears when its server passed its cap.
fn cap_error(s: &Server, held: u64, max: u64) -> String {
    format!(
        "the {} language server for {} held {}, past its {} cap, so rush stopped it. \
         Narrow the query: give file and line rather than a bare symbol search (that loads every project), \
         and work one package at a time. The next call starts a fresh server.",
        s.engine.name,
        s.root,
        gatecore::size(held),
        gatecore::size(max)
    )
}
