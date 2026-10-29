//! A value read off the UI thread and taken in on it: start begins a read
//! unless one is out, take hands back what it read, once, when it's done;
//! until then the UI draws what it had. Port of internal/ui/offui.go:1-145
//! (offRead start :41, take :64, out :77, redraw :86; offReads :104).
//! Go's goOff and cmdOff are Model::later here (frame/work.rs).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct State<T> {
    busy: bool,
    done: Option<T>,
}

/// One read at a time, its value taken once.
pub struct OffRead<T> {
    s: Arc<(Mutex<State<T>>, Condvar)>,
}

impl<T> Default for OffRead<T> {
    fn default() -> Self {
        OffRead {
            s: Arc::new((
                Mutex::new(State {
                    busy: false,
                    done: None,
                }),
                Condvar::new(),
            )),
        }
    }
}

impl<T: Send + 'static> OffRead<T> {
    /// Runs f on a thread of its own unless a read is out; whether it
    /// started one.
    pub fn start(&self, f: impl FnOnce() -> T + Send + 'static) -> bool {
        {
            let mut st = self.s.0.lock().unwrap_or_else(|e| e.into_inner());
            if st.busy {
                return false;
            }
            st.busy = true;
        }
        let s = self.s.clone();
        let spawned = thread::Builder::new()
            .name("rush-offread".into())
            .spawn(move || {
                let v = f();
                let mut st = s.0.lock().unwrap_or_else(|e| e.into_inner());
                st.done = Some(v);
                s.1.notify_all();
            });
        if spawned.is_err() {
            self.s.0.lock().unwrap_or_else(|e| e.into_inner()).busy = false;
            return false;
        }
        true
    }

    /// The value read, once it's done; after it a new read can start.
    pub fn take(&self) -> Option<T> {
        let mut st = self.s.0.lock().unwrap_or_else(|e| e.into_inner());
        let v = st.done.take()?;
        st.busy = false;
        Some(v)
    }

    /// Whether a read is out and not yet taken.
    pub fn out(&self) -> bool {
        self.s.0.lock().unwrap_or_else(|e| e.into_inner()).busy
    }

    /// Blocks until the read out is done (Go's redraw waits so in a Cmd):
    /// for a worker that hands the frame what was read, never the UI
    /// thread. Returns at once with none out.
    pub fn wait(&self) {
        let mut st = self.s.0.lock().unwrap_or_else(|e| e.into_inner());
        while st.busy && st.done.is_none() {
            st = self.s.1.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// OffRead by key: one read out at a time for each (offReads).
pub struct OffReads<K, T> {
    m: Mutex<HashMap<K, Arc<OffRead<T>>>>,
}

impl<K, T> Default for OffReads<K, T> {
    fn default() -> Self {
        OffReads {
            m: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash + Clone, T: Send + 'static> OffReads<K, T> {
    fn get(&self, k: &K) -> Option<Arc<OffRead<T>>> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).get(k).cloned()
    }

    /// Reads for k in the background, unless a read for it is out.
    pub fn start(&self, k: K, f: impl FnOnce() -> T + Send + 'static) {
        let r = self
            .m
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(k)
            .or_default()
            .clone();
        r.start(f);
    }

    /// What was read for k, once it's in.
    pub fn take(&self, k: &K) -> Option<T> {
        let v = self.get(k)?.take()?;
        self.m.lock().unwrap_or_else(|e| e.into_inner()).remove(k);
        Some(v)
    }

    /// Whether a read for k is out.
    pub fn out(&self, k: &K) -> bool {
        self.get(k).is_some()
    }

    /// Blocks until k's read is done (tests, workers).
    pub fn wait(&self, k: &K) {
        if let Some(r) = self.get(k) {
            r.wait();
        }
    }
}
