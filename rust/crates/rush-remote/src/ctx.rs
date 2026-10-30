//! What Go's context.Context is to serve: a stop that ends loops, waits
//! and requests, passed down to children that end with it. A wait on one
//! blocks on a condition variable, never polls.

use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

struct Inner {
    done: Mutex<bool>,
    cv: Condvar,
    children: Mutex<Vec<Weak<Inner>>>,
}

impl Inner {
    fn cancel(&self) {
        {
            let mut d = self.done.lock().unwrap_or_else(|e| e.into_inner());
            if *d {
                return;
            }
            *d = true;
        }
        self.cv.notify_all();
        let kids = std::mem::take(&mut *self.children.lock().unwrap_or_else(|e| e.into_inner()));
        for k in kids {
            if let Some(k) = k.upgrade() {
                k.cancel();
            }
        }
    }
}

/// A stop, and what ends with it.
#[derive(Clone)]
pub struct Ctx(Arc<Inner>);

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx::new()
    }
}

impl Ctx {
    /// One that ends only when cancelled.
    pub fn new() -> Ctx {
        Ctx(Arc::new(Inner {
            done: Mutex::new(false),
            cv: Condvar::new(),
            children: Mutex::new(Vec::new()),
        }))
    }

    /// One that ends with self, or on its own cancel.
    pub fn child(&self) -> Ctx {
        let c = Ctx::new();
        if self.done() {
            c.cancel();
            return c;
        }
        let mut kids = self.0.children.lock().unwrap_or_else(|e| e.into_inner());
        kids.retain(|k| k.strong_count() > 0);
        kids.push(Arc::downgrade(&c.0));
        c
    }

    /// Ends it and every child.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Whether it has ended.
    pub fn done(&self) -> bool {
        *self.0.done.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits for d or the end, whichever is first: true when d passed.
    pub fn sleep(&self, d: Duration) -> bool {
        let until = Instant::now() + d;
        let mut g = self.0.done.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if *g {
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            g = self
                .0
                .cv
                .wait_timeout(g, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Waits for the end.
    pub fn wait(&self) {
        let mut g = self.0.done.lock().unwrap_or_else(|e| e.into_inner());
        while !*g {
            g = self.0.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_ends_with_its_parent() {
        let p = Ctx::new();
        let c = p.child();
        let g = c.child();
        assert!(!g.done());
        p.cancel();
        assert!(c.done() && g.done());
        assert!(!g.sleep(Duration::from_secs(60)));
        g.wait();
        // Made after the end, it's ended.
        assert!(p.child().done());
    }
}
