//! What was read off the UI thread, kept by key, so a frame can ask for it
//! without waiting, and one result worked out off the UI thread for
//! something open. Port of Go's memo.go: the first ask for a key starts
//! the read on a thread of its own and has nothing yet; a frame after it
//! lands has it. One older than ttl is read again in the background, the
//! old one kept meanwhile. Nothing waits but the lock, which is never held
//! across a read. A read that lands wakes the loop (Note::Apply), so the
//! frame after draws it.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_draw::Sender;

use crate::frame::Out;
use crate::worker::Note;

struct Entry<V> {
    v: Option<V>,
    reading: bool,
    at: Option<Instant>,
    /// stale bumps it.
    r#gen: u64,
    /// The gen the kept value was read at.
    read_gen: u64,
}

impl<V> Default for Entry<V> {
    fn default() -> Self {
        Entry {
            v: None,
            reading: false,
            at: None,
            r#gen: 0,
            read_gen: 0,
        }
    }
}

type Read<K, V> = Arc<dyn Fn(&K) -> V + Send + Sync>;

/// What was read off the UI thread, by key.
pub struct Memo<K, V> {
    /// Zero keeps what was read until it's forgotten.
    ttl: Duration,
    m: Arc<Mutex<HashMap<K, Entry<V>>>>,
    read: Read<K, V>,
    /// Where a landed read wakes the loop; None reads at once (tests,
    /// --render).
    tx: Option<Sender<Note>>,
}

impl<K, V> Memo<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + 'static,
{
    pub fn new(
        ttl: Duration,
        tx: Option<Sender<Note>>,
        read: impl Fn(&K) -> V + Send + Sync + 'static,
    ) -> Memo<K, V> {
        Memo {
            ttl,
            m: Arc::new(Mutex::new(HashMap::new())),
            read: Arc::new(read),
            tx,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<K, Entry<V>>> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What's kept for k, if it has been read yet; the first ask, or one
    /// past its time, starts a read.
    pub fn get(&self, k: &K) -> Option<V> {
        let (start, r#gen) = {
            let mut m = self.lock();
            let e = m.entry(k.clone()).or_default();
            let old = !self.ttl.is_zero() && e.at.is_some_and(|t| t.elapsed() > self.ttl);
            let start = !e.reading && (e.v.is_none() || e.r#gen != e.read_gen || old);
            if start {
                e.reading = true;
            }
            (start, e.r#gen)
        };
        if start {
            self.fill(k.clone(), r#gen);
        }
        self.lock().get(k).and_then(|e| e.v.clone())
    }

    fn fill(&self, k: K, r#gen: u64) {
        let (m, read) = (self.m.clone(), self.read.clone());
        let land = move |k: K, v: V| {
            let mut m = m.lock().unwrap_or_else(|e| e.into_inner());
            let e = m.entry(k).or_default();
            e.reading = false;
            e.v = Some(v);
            e.at = Some(Instant::now());
            e.read_gen = r#gen;
        };
        let Some(tx) = self.tx.clone() else {
            let v = read(&k);
            land(k, v);
            return;
        };
        let back = k.clone();
        let spawned = std::thread::Builder::new()
            .name("rush-memo".into())
            .spawn(move || {
                let v = read(&k);
                land(k, v);
                tx.lane(1).send(Note::Apply(Box::new(|_| Out::Draw)));
            });
        if spawned.is_err()
            && let Some(e) = self.lock().get_mut(&back)
        {
            e.reading = false; // the next ask tries again
        }
    }

    /// Has k read again in the background on its next ask, keeping what
    /// it has until then.
    pub fn stale(&self, k: &K) {
        if let Some(e) = self.lock().get_mut(k) {
            e.r#gen += 1;
        }
    }

    /// Drops what's kept for k: the next ask reads it afresh.
    pub fn forget(&self, k: &K) {
        let mut m = self.lock();
        if m.get(k).is_some_and(|e| !e.reading) {
            m.remove(k);
        }
    }
}

/// One result worked out off the UI thread for something open, like a
/// sheet, which takes it in once it has landed (pending).
pub struct Pending<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Pending<T> {
    /// Starts f; a landed result wakes the loop when there's one to wake.
    pub fn start(tx: Option<Sender<Note>>, f: impl FnOnce() -> T + Send + 'static) -> Pending<T> {
        let (s, rx) = channel();
        match tx {
            None => {
                let _ = s.send(f());
            }
            Some(tx) => {
                let _ = std::thread::Builder::new()
                    .name("rush-pending".into())
                    .spawn(move || {
                        let _ = s.send(f());
                        tx.lane(1).send(Note::Apply(Box::new(|_| Out::Draw)));
                    });
            }
        }
        Pending { rx }
    }

    /// The result once it's landed, without waiting; only the first take
    /// after it lands has it.
    pub fn take(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn reads_once_until_stale() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let m: Memo<String, usize> = Memo::new(Duration::ZERO, None, move |k: &String| {
            c.fetch_add(1, Ordering::SeqCst);
            k.len()
        });
        assert_eq!(m.get(&"abc".to_string()), Some(3));
        assert_eq!(m.get(&"abc".to_string()), Some(3));
        assert_eq!(n.load(Ordering::SeqCst), 1, "read once");
        m.stale(&"abc".to_string());
        assert_eq!(m.get(&"abc".to_string()), Some(3));
        assert_eq!(n.load(Ordering::SeqCst), 2, "stale reads it again");
        m.forget(&"abc".to_string());
        m.get(&"abc".to_string());
        assert_eq!(n.load(Ordering::SeqCst), 3, "forgotten, read afresh");
    }

    #[test]
    fn pending_is_taken_once() {
        let p = Pending::start(None, || 42);
        assert_eq!(p.take(), Some(42));
        assert_eq!(p.take(), None);
    }
}
