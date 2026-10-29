//! Go's time.AfterFunc for the host: one thread waits on a condvar until
//! the next thing due, and sleeps for good when nothing is (idle costs
//! nothing).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Inner {
    due: BTreeMap<(Instant, u64), ()>,
    jobs: HashMap<u64, (Instant, Job)>,
    next: u64,
    done: bool,
}

/// Runs jobs once their time comes, each on the timers' own thread, one
/// at a time.
#[derive(Clone, Default)]
pub struct Timers(Arc<(Mutex<Inner>, Condvar)>);

/// A scheduled job, to cancel it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerId(u64);

impl Timers {
    /// Timers with their thread started.
    pub fn start() -> Timers {
        let t = Timers::default();
        let me = t.clone();
        let home = rush_state::dir();
        let cache = rush_state::cache_dir();
        std::thread::Builder::new()
            .name("host-timers".into())
            .spawn(move || {
                rush_state::set_thread_dir(Some(home));
                rush_state::set_thread_cache_dir(Some(cache));
                me.run();
            })
            .expect("a thread for the host's timers");
        t
    }

    fn run(&self) {
        let (m, cv) = &*self.0;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if g.done {
                return;
            }
            let now = Instant::now();
            let first = g.due.keys().next().copied();
            match first {
                Some((at, id)) if at <= now => {
                    g.due.remove(&(at, id));
                    let job = g.jobs.remove(&id).map(|(_, j)| j);
                    drop(g);
                    if let Some(j) = job {
                        // A job that panics ends only itself, not the timers.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(j));
                    }
                    g = m.lock().unwrap_or_else(|e| e.into_inner());
                }
                Some((at, _)) => {
                    g = cv
                        .wait_timeout(g, at - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
                None => {
                    g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                }
            }
        }
    }

    /// Runs f once d has passed.
    pub fn after(&self, d: Duration, f: impl FnOnce() + Send + 'static) -> TimerId {
        let (m, cv) = &*self.0;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.next += 1;
        let id = g.next;
        let at = Instant::now() + d;
        g.due.insert((at, id), ());
        g.jobs.insert(id, (at, Box::new(f)));
        cv.notify_all();
        TimerId(id)
    }

    /// Forgets a job not yet run.
    pub fn cancel(&self, id: TimerId) {
        let (m, cv) = &*self.0;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, _)) = g.jobs.remove(&id.0) {
            g.due.remove(&(at, id.0));
        }
        cv.notify_all();
    }

    /// Ends the timers' thread, dropping what's still to run.
    pub fn stop(&self) {
        let (m, cv) = &*self.0;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.done = true;
        g.due.clear();
        g.jobs.clear();
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn runs_in_order_and_cancels() {
        let t = Timers::start();
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        let tx3 = tx.clone();
        t.after(Duration::from_millis(20), move || tx.send(2).unwrap());
        t.after(Duration::ZERO, move || tx2.send(1).unwrap());
        let c = t.after(Duration::from_millis(5), move || tx3.send(3).unwrap());
        t.cancel(c);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        t.stop();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }
}
