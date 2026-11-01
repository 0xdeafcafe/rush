//! Reads many small files at once: a list's first reading opens a
//! thousand transcripts' heads, each a wait on the disk, so they're read
//! side by side on a few threads that end with the call. Rust-only (Go
//! reads them one after another).

/// At most this many threads, the caller's included.
const MAX_THREADS: usize = 8;
/// Fewer items than this each aren't worth a thread.
const MIN_EACH: usize = 16;

/// `f` on each item, in the items' order, on up to MAX_THREADS threads,
/// each with a state of its own from `init` (a read buffer). Few items are
/// done on the caller's thread alone.
pub fn map_with<T, R, S>(
    mut items: Vec<T>,
    init: impl Fn() -> S + Sync,
    f: impl Fn(&mut S, T) -> R + Sync,
) -> Vec<R>
where
    T: Send,
    R: Send,
{
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = cpus.min(MAX_THREADS).min(items.len() / MIN_EACH).max(1);
    if threads == 1 {
        let mut s = init();
        return items.into_iter().map(|t| f(&mut s, t)).collect();
    }
    let each = items.len().div_ceil(threads);
    let mut chunks = Vec::with_capacity(threads);
    while items.len() > each {
        chunks.push(items.split_off(items.len() - each));
    }
    chunks.push(items);
    chunks.reverse();
    let (init, f) = (&init, &f);
    std::thread::scope(|sc| {
        let mut first = chunks.drain(..1);
        let mine = first.next().unwrap_or_default();
        drop(first);
        let others: Vec<_> = chunks
            .into_iter()
            .map(|c| {
                sc.spawn(move || {
                    let mut s = init();
                    c.into_iter().map(|t| f(&mut s, t)).collect::<Vec<R>>()
                })
            })
            .collect();
        let mut s = init();
        let mut out: Vec<R> = mine.into_iter().map(|t| f(&mut s, t)).collect();
        for h in others {
            match h.join() {
                Ok(r) => out.extend(r),
                Err(e) => std::panic::resume_unwind(e),
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::map_with;

    // Every item is done once, in order, whether on one thread or many.
    #[test]
    fn keeps_order() {
        for n in [0usize, 1, 15, 16, 17, 100, 1001] {
            let items: Vec<usize> = (0..n).collect();
            let out = map_with(
                items,
                || 0usize,
                |seen, i| {
                    *seen += 1;
                    i * 2
                },
            );
            assert_eq!(out, (0..n).map(|i| i * 2).collect::<Vec<_>>(), "n={n}");
        }
    }
}
