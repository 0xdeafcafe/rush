//! Drawing a long conversation: what a frame costs once it's laid out
//! (warm), what laying all of it out costs (cold), and a delta streaming
//! in then a frame. Go's numbers are its BenchmarkRender
//! (internal/convo/bench_test.go:205) on benchSession(300), where a warm
//! frame still walks every turn's cache key; here a frame draws the turns
//! in view. Time per frame, bytes allocated per frame by a counting
//! allocator, and the rows kept in memory.
//!
//! `cargo bench -p rush-convo --bench render`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

#[path = "../tests/common/mod.rs"]
mod common;

use common::{Feed, at, bench_session, delta};
use rush_convo::{Options, Renderer, Session};

struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            TOTAL.fetch_add(l.size(), Relaxed);
            NOW.fetch_add(l.size(), Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        NOW.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            TOTAL.fetch_add(new, Relaxed);
            NOW.fetch_add(new, Relaxed);
            NOW.fetch_sub(l.size(), Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Runs f n times: microseconds and bytes allocated per run.
fn measure(n: usize, mut f: impl FnMut(usize)) -> (f64, usize) {
    let total = TOTAL.load(Relaxed);
    let t = Instant::now();
    for i in 0..n {
        f(i);
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
    (us, (TOTAL.load(Relaxed) - total) / n)
}

fn report(name: &str, (us, bytes): (f64, usize)) {
    println!(
        "{name:<34} {us:>10.1} µs {:>10.1} KiB/op",
        bytes as f64 / 1024.0
    );
}

fn opts(w: usize) -> Options {
    Options {
        width: w,
        now: at(100_000),
        ..Options::default()
    }
}

fn run(turns: usize, w: usize) {
    let s = bench_session(turns);
    let o = opts(w);
    println!("— {turns} turns, w{w}");
    // Cold: every turn laid out once, as a first scroll to the top needs.
    let mut n = 0;
    report(
        "cold layout (all turns)",
        measure(3, |_| {
            let mut r = Renderer::new();
            n = r.total_height(&s, &o);
        }),
    );
    // Cold open at the end: only the turns in view are drawn.
    report(
        "cold frame at the end",
        measure(20, |_| {
            let mut r = Renderer::new();
            r.render_tail(&s, &o, 0, 50);
        }),
    );
    // Warm: the same frame again, the live turn's clock ticking.
    let mut r = Renderer::new();
    r.render_tail(&s, &o, 0, 50);
    report(
        "warm frame at the end",
        measure(2000, |i| {
            let mut o = o.clone();
            o.tick = i;
            r.render_tail(&s, &o, 0, 50);
        }),
    );
    // Warm, scrolled to the middle by row.
    let mut r = Renderer::new();
    r.render_range(&s, &o, n / 2, 50);
    report(
        "warm frame mid-way (range)",
        measure(2000, |i| {
            let mut o = o.clone();
            o.tick = i;
            r.render_range(&s, &o, n / 2, 50);
        }),
    );
    let before = NOW.load(Relaxed);
    let kept = r.kept_rows();
    drop(r);
    println!(
        "{:<34} {kept:>10} rows {:>10.1} KiB",
        "kept by a renderer mid-way",
        (before - NOW.load(Relaxed)) as f64 / 1024.0
    );
    stream(turns, w);
}

/// A delta arrives and a frame is drawn: the streaming hot path.
fn stream(turns: usize, w: usize) {
    let mut s: Session = bench_session(turns);
    let o = opts(w);
    let mut r = Renderer::new();
    r.render_tail(&s, &o, 0, 50);
    report(
        "stream: delta, then a frame",
        measure(500, |i| {
            s.feed(delta("more words arrive "), at(100_000));
            let mut o = o.clone();
            o.tick = i;
            r.render_tail(&s, &o, 0, 50);
        }),
    );
}

fn main() {
    // RUSH_BENCH_COLD=<secs> lays a session out over and over, for a profiler.
    if let Some(secs) = std::env::var("RUSH_BENCH_COLD")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        let s = bench_session(300);
        let o = opts(120);
        let t = Instant::now();
        while t.elapsed().as_secs() < secs {
            Renderer::new().total_height(&s, &o);
        }
        return;
    }
    for (turns, w) in [(300, 120), (300, 250), (3000, 120)] {
        run(turns, w);
    }
}
