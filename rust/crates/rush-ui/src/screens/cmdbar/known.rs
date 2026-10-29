//! What a frame shows but only the disk or the kernel knows is never read
//! while it's drawn: Known keeps what was read; a frame takes what it has
//! and asks for the rest, read off the UI thread through Model::later.
//! Port of internal/ui/known.go:1-90 (known, get, read, put). What each
//! of Go's uses reads (a process's words, worktrees, commands, paths) is
//! its screen's.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::frame::Out;
use crate::model::Model;

/// Values read off the UI thread, by key, and the keys asked for since.
pub struct Known<K, V> {
    got: HashMap<K, (V, Instant)>,
    want: HashSet<K>,
    reading: bool,
}

impl<K, V> Default for Known<K, V> {
    fn default() -> Self {
        Known {
            got: HashMap::new(),
            want: HashSet::new(),
            reading: false,
        }
    }
}

impl<K: Eq + Hash + Clone + Send + 'static, V: Clone + Send + 'static> Known<K, V> {
    /// k's value as last read, and whether it's fresh. One missing, or
    /// older than fresh (when fresh isn't zero), is asked for again (get).
    pub fn get(&mut self, k: &K, fresh: Duration) -> (Option<V>, bool) {
        let e = self.got.get(k);
        if let Some((v, at)) = e
            && (fresh.is_zero() || at.elapsed() < fresh)
        {
            return (Some(v.clone()), true);
        }
        self.want.insert(k.clone());
        (e.map(|(v, _)| v.clone()), false)
    }

    /// Whether anything was asked for and no read is out.
    pub fn wants(&self) -> bool {
        !self.want.is_empty() && !self.reading
    }

    /// Keeps values read at at (put).
    pub fn put(&mut self, vs: Vec<(K, V)>, at: Instant) {
        if self.got.len() > 4096 {
            self.got.clear();
        }
        for (k, v) in vs {
            self.got.insert(k, (v, at));
        }
    }

    /// Hands what was asked for to be read, once a read isn't out.
    fn take_wants(&mut self) -> Vec<K> {
        if self.reading {
            return Vec::new();
        }
        self.reading = !self.want.is_empty();
        self.want.drain().collect()
    }
}

/// Reads what the Known that `of` picks out of the model asked for with f,
/// off the UI thread, one read at a time; then runs then (read).
pub fn read_known<K, V>(
    m: &mut Model,
    of: fn(&mut Model) -> &mut Known<K, V>,
    f: fn(&K) -> V,
    then: Option<fn(&mut Model) -> Out>,
) where
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Send + 'static,
{
    let want = of(m).take_wants();
    if want.is_empty() {
        return;
    }
    m.later(
        move || want.into_iter().map(|k| (f(&k), k)).map(|(v, k)| (k, v)).collect::<Vec<_>>(),
        move |m, out| {
            let k = of(m);
            k.reading = false;
            k.put(out, Instant::now());
            then.map_or(Out::Draw, |t| t(m))
        },
    );
}
