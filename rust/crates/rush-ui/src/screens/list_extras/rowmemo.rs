//! Rows kept between frames, and finding a row by what it's of. After
//! Go's internal/ui/rowmemo.go (rowMemo :7, turn :10, get :19) and
//! rowindex.go:9 (rowOf). rowOf's start, the row where the turn holding
//! the ref begins, is the caller's to give: the conversation in Rust
//! doesn't yet say which turn holds a ref (convo's TurnOf).

use std::collections::HashMap;
use std::hash::Hash;

/// Rows a frame builds from the same parts as the frame before: most rows
/// don't change frame to frame. What's kept is made only from the key, so
/// a hit is never stale (rowMemo).
pub struct RowMemo<K, V> {
    now: HashMap<K, V>,
    old: HashMap<K, V>,
    on: bool,
}

impl<K, V> Default for RowMemo<K, V> {
    fn default() -> Self {
        RowMemo {
            now: HashMap::new(),
            old: HashMap::new(),
            on: false,
        }
    }
}

impl<K: Eq + Hash, V: Clone> RowMemo<K, V> {
    /// Starts a frame: the last frame's rows stay at hand, older go
    /// (turn).
    pub fn turn(&mut self) {
        std::mem::swap(&mut self.old, &mut self.now);
        self.now.clear();
        self.on = true;
    }

    /// The row k makes, made by mk when neither frame had it; before the
    /// first turn nothing is kept (get).
    pub fn get(&mut self, k: K, mk: impl FnOnce() -> V) -> V {
        if !self.on {
            return mk();
        }
        if let Some(v) = self.now.get(&k) {
            return v.clone();
        }
        let v = self.old.remove(&k).unwrap_or_else(mk);
        self.now.insert(k, v.clone());
        v
    }

    /// How many rows the frame being drawn has kept.
    pub fn len(&self) -> usize {
        self.now.len()
    }

    pub fn is_empty(&self) -> bool {
        self.now.is_empty()
    }
}

/// The row of the first of refs with r, or None, where refs are rows'
/// refs from base on. It looks from `from` (where the turn holding r
/// begins, relative to base: rows put in after its steps only push it
/// further down), then above it, for a row drawn since another way
/// (rowOf).
pub fn row_of(refs: &[&str], base: usize, r: &str, from: usize) -> Option<usize> {
    let from = from.min(refs.len());
    refs[from..]
        .iter()
        .position(|x| *x == r)
        .map(|i| from + i)
        .or_else(|| refs[..from].iter().position(|x| *x == r))
        .map(|i| base + i)
}
