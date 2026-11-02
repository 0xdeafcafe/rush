//! A holder's watchdog over the run it holds: it lowers the run's
//! priority as it ages, notes what it holds, and stops or pauses it as
//! Judge says. Ported from internal/gate/watch.go:1-220.

use std::time::{Duration, Instant, SystemTime};

use rush_sys::proc::{self, Pid, Table};
use serde_json::{Value, json};

use crate::gate::{Cancel, Run};
use crate::godur;
use crate::share::{Act, Look, PRESSURE_CRITICAL, TICK, judge, nice_at};
use crate::sys::renice;

/// How long a run asked to stop has before it's made to.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Tells the agent something of its run.
pub type TellFn = Box<dyn Fn(&str) + Send + Sync>;
/// Records what was done and why: a kind and its fields.
pub type LogFn = Box<dyn Fn(&str, serde_json::Map<String, Value>) + Send + Sync>;

/// What a holder's watchdog needs of its run.
#[derive(Default)]
pub struct Watch {
    /// The run's top process: its tree, the holder left out, is the run.
    pub root: Pid,
    /// The agent's program that started it, 0 for none.
    pub agent_pid: Pid,
    /// Whether its rush session still runs; None for none.
    pub session_alive: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    /// Tells the agent why its run was stopped or paused.
    pub tell: Option<TellFn>,
    /// If set, records what the watchdog did to the run and why.
    pub log: Option<LogFn>,
    /// The share of memory, in percent, at which it's taken to be
    /// critical; 0 for the guard's default.
    pub floor: i32,
}

impl Watch {
    fn log(&self, kind: &str, f: Value) {
        if let (Some(log), Value::Object(m)) = (&self.log, f) {
            log(kind, m);
        }
    }

    fn tell(&self, s: &str) {
        if let Some(t) = &self.tell {
            t(s);
        }
    }
}

impl Run {
    /// Looks after the run until cancelled: every TICK it lowers the
    /// priority of the run's processes as it ages, new ones too, notes
    /// what it holds, and stops or pauses it as Judge says.
    pub fn watch(&self, cancel: &Cancel, w: Watch) {
        self.w.lock().unwrap_or_else(|e| e.into_inner()).root = w.root;
        let start = Instant::now();
        let mut next = start + TICK;
        let mut stopping: Option<Instant> = None;
        let mut killed = false;
        let me = std::process::id() as Pid;
        loop {
            if cancel.wait(next.saturating_duration_since(Instant::now())) {
                self.wake();
                return;
            }
            let now = Instant::now();
            next += TICK;
            if next < now {
                next = now + TICK; // a sleep of the machine's: no ticks to catch up
            }
            let mut tab = proc::snapshot();
            let tree = run_tree(&tab, w.root);
            if tree.is_empty() {
                continue;
            }
            if let Some(at) = stopping {
                if now.duration_since(at) >= STOP_GRACE {
                    signal_tree(&tree, libc::SIGKILL);
                    if !killed {
                        killed = true;
                        w.log(
                            "gate-killed",
                            json!({"name": self.ask.name, "pids": tree,
                                "why": format!("still running {} after it was asked to stop", godur::string(STOP_GRACE))}),
                        );
                    }
                }
                continue;
            }
            let (mut st, paused_total) = {
                let g = self.w.lock().unwrap_or_else(|e| e.into_inner());
                (g.state.clone(), g.paused)
            };
            let wall = SystemTime::now();
            let mut ran = now.duration_since(start).saturating_sub(paused_total);
            if let Some(at) = st.paused_at {
                ran = ran.saturating_sub(wall.duration_since(at).unwrap_or_default());
            }
            if !st.paused() {
                st.nice = nice_at(ran);
                for &pid in &tree {
                    renice(pid, st.nice);
                }
            }
            tab.fill(None, &[w.root]);
            st.footprint = tree
                .iter()
                .filter_map(|pid| tab.procs.get(pid))
                .map(|p| p.footprint)
                .fold(0u64, u64::saturating_add);
            let mut l = Look {
                ran,
                footprint: st.footprint,
                paused: st.paused(),
                pressure: self.gate.pressure(w.floor),
                has_agent: w.agent_pid > 0,
                ..Look::default()
            };
            if l.has_agent && tab.procs.contains_key(&w.agent_pid) {
                l.agent_alive = !proc::zombie(w.agent_pid);
            }
            let mut top = w.root;
            if tab.procs.get(&top).is_some_and(|p| p.ppid == me) {
                top = me;
            }
            if tab.procs.get(&top).is_some_and(|p| p.ppid == 1) {
                l.orphaned = true;
                l.session_alive = w.session_alive.as_ref().is_some_and(|f| f());
            }
            if l.pressure == PRESSURE_CRITICAL && !l.paused {
                (l.newest, l.others, l.since_pause) = self.standing(wall);
            }
            let (mut act, why) = judge(&self.ask.rule, &l);
            // A dry run writes down the stop or pause rather than make it.
            let dry = match act {
                Act::Stop => Some("gate-stop"),
                Act::Pause => Some("gate-pause"),
                _ => None,
            };
            if let Some(action) = dry
                && rush_core::dryrun::would(
                    action,
                    &format!("{} run, pid {}", self.ask.name, w.root),
                    &why,
                )
            {
                act = Act::Carry;
            }
            if act != Act::Carry {
                let kind = match act {
                    Act::Stop => "gate-stopped",
                    Act::Pause => "gate-paused",
                    _ => "gate-resumed",
                };
                w.log(
                    kind,
                    json!({"name": self.ask.name, "root": w.root, "why": why,
                        "ranFor": godur::string(godur::round_secs(ran)), "footprint": st.footprint,
                        "pressure": l.pressure}),
                );
            }
            let mut paused_more = Duration::ZERO;
            let mut tell = None;
            match act {
                Act::Stop => {
                    signal_tree(&tree, libc::SIGTERM);
                    if st.paused() {
                        signal_tree(&tree, libc::SIGCONT);
                    }
                    stopping = Some(now);
                    tell = Some(format!(
                        "rush stopped your `{}` run: {why}. Don't run it again as it was: run less of it at once, \
                         or change its limits in the gate's settings.",
                        self.ask.name
                    ));
                }
                Act::Pause => {
                    signal_tree(&tree, libc::SIGSTOP);
                    st.paused_at = Some(wall);
                    st.why = why.clone();
                    tell = Some(format!(
                        "rush paused your `{}` run: {why}. It carries on by itself once memory is back to normal; \
                         it's waiting, not hung, so don't stop it or run it again.",
                        self.ask.name
                    ));
                }
                Act::Resume => {
                    signal_tree(&tree, libc::SIGCONT);
                    if let Some(at) = st.paused_at {
                        paused_more = wall.duration_since(at).unwrap_or_default();
                    }
                    st.paused_at = None;
                    st.why.clear();
                }
                Act::Carry => {}
            }
            {
                let mut g = self.w.lock().unwrap_or_else(|e| e.into_inner());
                g.state = st.clone();
                g.paused += paused_more;
            }
            self.write_state(&st);
            // Told once the state says it, so what the agent then reads of
            // the gate agrees.
            if let Some(t) = tell {
                w.tell(&t);
            }
        }
    }

    /// Lets a paused run go on: one being asked to stop must run to hear
    /// it.
    pub fn wake(&self) {
        let g = self.w.lock().unwrap_or_else(|e| e.into_inner());
        if g.state.paused() && g.root > 0 {
            signal_tree(&run_tree(&proc::snapshot(), g.root), libc::SIGCONT);
        }
    }

    /// Of the runs going unpaused, whether this is the newest started and
    /// how many others there are; and how long since any run was last
    /// paused.
    fn standing(&self, now: SystemTime) -> (bool, usize, Duration) {
        let ts = self.gate.tickets();
        let mine = self.ticket_name();
        let mut newest = true;
        let mut others = 0;
        let mut since_pause = Duration::from_nanos(1 << 62);
        let me_since = ts.iter().find(|t| t.file == mine).and_then(|t| t.since);
        let after = |a: Option<SystemTime>, b: Option<SystemTime>| a > b;
        for t in &ts {
            if !t.running {
                continue;
            }
            if let Some(at) = t.state.paused_at {
                since_pause = since_pause.min(now.duration_since(at).unwrap_or_default());
                continue;
            }
            if t.file == mine {
                continue;
            }
            others += 1;
            if after(t.since, me_since) || t.since == me_since && t.file > mine {
                newest = false;
            }
        }
        (newest, others, since_pause)
    }
}

/// root and everything under it, deepest first, but this process: the
/// node preload's holder runs under the run it holds.
pub(crate) fn run_tree(tab: &Table, root: Pid) -> Vec<Pid> {
    if !tab.procs.contains_key(&root) {
        return Vec::new();
    }
    let me = std::process::id() as Pid;
    tab.descendants(root)
        .into_iter()
        .filter(|&p| p != me)
        .collect()
}

/// Sends sig to every process in tree, its root first, so a tree being
/// paused can't start more while it's walked.
pub(crate) fn signal_tree(tree: &[Pid], sig: i32) {
    for &pid in tree.iter().rev() {
        let _ = proc::kill(pid, sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::Gate;
    use crate::gate::tests::{Fake, ask};
    use crate::rules::GIB;
    use crate::share::PRESSURE_NORMAL;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, mpsc};

    fn proc_state(pid: i32) -> String {
        let out = std::process::Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // watch_test.go TestWatchPausesTheNewest: under critical memory
    // pressure the newest run going is paused, its processes stopped, and
    // goes on once pressure is back to normal; it counts as waiting
    // meanwhile, and its agent is told why. It takes the watchdog's ticks:
    // it waits on what it's told, never sleeps.
    #[test]
    fn watch_pauses_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(64 * GIB);
        fake.pressure.store(PRESSURE_CRITICAL, Ordering::SeqCst);
        let g = Gate::at(dir.path(), fake.clone());
        let first = g
            .acquire(&Cancel::new(), ask("tsc", GIB, "a"), None)
            .unwrap();
        // The newest by its start: tickets name it by the nanosecond.
        let newest = g
            .acquire(&Cancel::new(), ask("go", GIB, "b"), None)
            .unwrap();
        let mut sleep = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = sleep.id() as i32;
        let (told_tx, told) = mpsc::channel::<String>();
        let told_tx = std::sync::Mutex::new(told_tx);
        let (log_tx, logged) = mpsc::channel::<String>();
        let log_tx = std::sync::Mutex::new(log_tx);
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let run = Arc::clone(&newest);
        let h = std::thread::spawn(move || {
            run.watch(
                &c2,
                Watch {
                    root: pid,
                    tell: Some(Box::new(move |s| {
                        let _ = told_tx.lock().unwrap().send(s.to_string());
                    })),
                    log: Some(Box::new(move |kind, _| {
                        let _ = log_tx.lock().unwrap().send(kind.to_string());
                    })),
                    ..Watch::default()
                },
            )
        });
        let said = told.recv_timeout(4 * TICK).expect("never paused");
        assert!(said.contains("paused"), "{said}");
        assert_eq!(logged.recv().unwrap(), "gate-paused");
        let paused = |g: &Gate| g.tickets().iter().any(|t| t.name == "go" && t.paused());
        assert!(paused(&g));
        assert!(proc_state(pid).starts_with('T'), "{}", proc_state(pid));
        assert!(g.waiting(std::process::id() as i32));
        assert!(
            !g.tickets().iter().any(|t| t.name == "tsc" && t.paused()),
            "the older run was paused too"
        );
        fake.pressure.store(PRESSURE_NORMAL, Ordering::SeqCst);
        let kind = logged.recv_timeout(4 * TICK).expect("never resumed");
        assert_eq!(kind, "gate-resumed");
        // The state is written once the tick's acts are done: the next
        // tick's look sees it, or this one's write does.
        cancel.cancel();
        h.join().unwrap();
        assert!(!paused(&g));
        assert!(!proc_state(pid).starts_with('T'));
        let _ = sleep.kill();
        let _ = sleep.wait();
        first.release();
        newest.release();
    }

    // In a dry run the run that would be paused isn't: the pause is
    // written to the dry run's log instead, and its agent isn't told.
    #[test]
    fn a_dry_run_doesnt_pause() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(64 * GIB);
        fake.pressure.store(PRESSURE_CRITICAL, Ordering::SeqCst);
        let g = Gate::at(dir.path(), fake.clone());
        // An older run beside it: the newest is the one paused.
        let first = g
            .acquire(&Cancel::new(), ask("tsc", GIB, "a"), None)
            .unwrap();
        let run = g
            .acquire(&Cancel::new(), ask("dry", GIB, "b"), None)
            .unwrap();
        let mut sleep = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = sleep.id() as i32;
        let log = dir.path().join("dryrun.log");
        let (told_tx, told) = mpsc::channel::<String>();
        let told_tx = std::sync::Mutex::new(told_tx);
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let r2 = Arc::clone(&run);
        let l2 = log.clone();
        let h = std::thread::spawn(move || {
            rush_core::dryrun::set_thread(Some((true, Some(l2))));
            r2.watch(
                &c2,
                Watch {
                    root: pid,
                    tell: Some(Box::new(move |s| {
                        let _ = told_tx.lock().unwrap().send(s.to_string());
                    })),
                    ..Watch::default()
                },
            )
        });
        // Waits on the log's folder for the line, never sleeps.
        let w = rush_sys::fswait::Watcher::new();
        w.watch(&[dir.path()]);
        let deadline = Instant::now() + 4 * TICK;
        let line = loop {
            if let Ok(s) = std::fs::read_to_string(&log)
                && s.contains("gate-pause")
            {
                break s;
            }
            let now = Instant::now();
            assert!(now < deadline, "never written down");
            // A line added to a file there already may not wake it.
            w.wait(None, Some((deadline - now).min(Duration::from_millis(50))));
        };
        assert!(line.contains(&format!("dry run, pid {pid}")), "{line}");
        assert!(!g.tickets().iter().any(|t| t.paused()));
        cancel.cancel();
        h.join().unwrap();
        assert!(told.try_recv().is_err(), "its agent was told");
        let _ = sleep.kill();
        let _ = sleep.wait();
        run.release();
        first.release();
    }
}
