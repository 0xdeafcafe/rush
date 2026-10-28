//! How long a call may wait, and whether whoever asked has gone: what Go's
//! context.Context does for rush's plugin calls. A Ctx has an optional
//! deadline and any number of cancels; waiting on one blocks on a channel
//! that its cancel or its deadline ends, never a loop that looks.

use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Why a Ctx is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtxErr {
    Canceled,
    DeadlineExceeded,
}

impl fmt::Display for CtxErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CtxErr::Canceled => "context canceled",
            CtxErr::DeadlineExceeded => "context deadline exceeded",
        })
    }
}

type Waiter = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct CancelState {
    done: bool,
    next: u64,
    waiters: HashMap<u64, Waiter>,
}

#[derive(Default)]
struct CancelInner {
    state: Mutex<CancelState>,
}

impl CancelInner {
    fn cancel(&self) {
        let waiters = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.done {
                return;
            }
            s.done = true;
            std::mem::take(&mut s.waiters)
        };
        for (_, w) in waiters {
            w();
        }
    }

    fn is_done(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).done
    }
}

/// Ends the Ctx it was made with, and every Ctx made from that one.
#[derive(Clone)]
pub struct Cancel(Arc<CancelInner>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// A deadline and the cancels a wait ends on.
#[derive(Clone, Default)]
pub struct Ctx {
    deadline: Option<Instant>,
    cancels: Vec<Arc<CancelInner>>,
}

/// A waiter on a Ctx's cancels, taken off them when dropped.
pub struct Registration {
    on: Vec<(Arc<CancelInner>, u64)>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        for (c, id) in self.on.drain(..) {
            c.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .waiters
                .remove(&id);
        }
    }
}

impl Ctx {
    /// Never done: context.Background.
    pub fn background() -> Ctx {
        Ctx::default()
    }

    /// This Ctx, done by d from now at the latest.
    pub fn with_timeout(&self, d: Duration) -> Ctx {
        self.with_deadline(Instant::now() + d)
    }

    /// This Ctx, done by at at the latest.
    pub fn with_deadline(&self, at: Instant) -> Ctx {
        let mut c = self.clone();
        c.deadline = Some(c.deadline.map_or(at, |d| d.min(at)));
        c
    }

    /// This Ctx, also done when the Cancel returned is.
    pub fn with_cancel(&self) -> (Ctx, Cancel) {
        let inner = Arc::new(CancelInner::default());
        let mut c = self.clone();
        c.cancels.push(inner.clone());
        (c, Cancel(inner))
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Why it's done, None while it isn't.
    pub fn err(&self) -> Option<CtxErr> {
        if self.cancels.iter().any(|c| c.is_done()) {
            return Some(CtxErr::Canceled);
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Some(CtxErr::DeadlineExceeded);
        }
        None
    }

    /// Runs f once, when one of its cancels is (at once if one is already):
    /// not on its deadline, which a waiter keeps itself. Dropping what it
    /// returns forgets f.
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) -> Registration {
        let mut reg = Registration { on: Vec::new() };
        if self.cancels.is_empty() {
            return reg;
        }
        // f runs once, whichever cancel comes first.
        let f: Arc<Mutex<Option<Waiter>>> = Arc::new(Mutex::new(Some(Box::new(f))));
        let fire = {
            let f = f.clone();
            move || {
                if let Some(f) = f.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    f();
                }
            }
        };
        for c in &self.cancels {
            let mut s = c.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.done {
                drop(s);
                fire();
                return reg;
            }
            s.next += 1;
            let id = s.next;
            let fire = fire.clone();
            s.waiters.insert(id, Box::new(fire));
            reg.on.push((c.clone(), id));
        }
        reg
    }

    /// Blocks until it's done, and says why.
    pub fn wait(&self) -> CtxErr {
        // tx is kept here, so only a cancel or the deadline ends the wait.
        let (tx, rx) = mpsc::channel::<()>();
        let fire = tx.clone();
        let _reg = self.on_cancel(move || {
            let _ = fire.send(());
        });
        let got = self.recv(&rx).err().unwrap_or(CtxErr::Canceled);
        drop(tx);
        got
    }

    /// Waits d, or less if it's done first: false when it is.
    pub fn sleep(&self, d: Duration) -> bool {
        let short = self.with_timeout(d);
        let (tx, rx) = mpsc::channel::<()>();
        let fire = tx.clone();
        let _reg = short.on_cancel(move || {
            let _ = fire.send(());
        });
        let _ = short.recv(&rx);
        drop(tx);
        self.err().is_none()
    }

    /// Receives from rx until this Ctx's deadline: what came, or why
    /// nothing did. A sender gone is taken as canceled.
    pub(crate) fn recv<T>(&self, rx: &mpsc::Receiver<T>) -> Result<T, CtxErr> {
        let got = match self.deadline {
            None => rx.recv().map_err(|_| CtxErr::Canceled),
            Some(d) => rx
                .recv_timeout(d.saturating_duration_since(Instant::now()))
                .map_err(|e| match e {
                    mpsc::RecvTimeoutError::Timeout => CtxErr::DeadlineExceeded,
                    mpsc::RecvTimeoutError::Disconnected => CtxErr::Canceled,
                }),
        };
        match got {
            Ok(v) => Ok(v),
            Err(e) => Err(self.err().unwrap_or(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_and_deadline() {
        let (c, cancel) = Ctx::background().with_cancel();
        let child = c.with_timeout(Duration::from_secs(60));
        assert_eq!(child.err(), None);
        let t = std::thread::spawn(move || child.wait());
        cancel.cancel();
        assert_eq!(t.join().unwrap(), CtxErr::Canceled);
        assert_eq!(c.err(), Some(CtxErr::Canceled));
        let short = Ctx::background().with_timeout(Duration::ZERO);
        assert_eq!(short.wait(), CtxErr::DeadlineExceeded);
        assert!(!short.sleep(Duration::from_secs(5)));
        assert!(Ctx::background().sleep(Duration::ZERO));
    }
}
