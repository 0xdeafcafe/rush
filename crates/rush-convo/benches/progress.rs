//! What the dock asks of each background command it shows, every frame:
//! how far its output says it has got (render::progress_any over the
//! last 30 lines the reader kept: cargo's crates and bar, its tests, or
//! a last line's n/m or n%). Time per call and bytes allocated per call,
//! for a cargo build, cargo's tests and a command that says nothing of
//! its progress (the worst case: every pattern tried).
//!
//! `cargo bench -p rush-convo --bench progress`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct Counting;

static TOTAL: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        TOTAL.fetch_add(l.size(), Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Runs f n times: microseconds and bytes allocated per run.
fn measure(n: usize, mut f: impl FnMut()) -> (f64, usize) {
    let total = TOTAL.load(Relaxed);
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
    (us, (TOTAL.load(Relaxed) - total) / n)
}

fn report(name: &str, (us, bytes): (f64, usize)) {
    println!("{name:<28} {us:>8.2} µs {bytes:>8} B/call");
}

fn main() {
    let build: Vec<String> = (0..29)
        .map(|i| format!("   Compiling crate-{i} v0.1.0 (/Users/me/src/rush/crates/crate-{i})"))
        .chain(["    Building [=====>      ] 212/430: rush-convo".to_string()])
        .collect();
    let tests: Vec<String> = ["running 430 tests".to_string()]
        .into_iter()
        .chain((0..29).map(|i| format!("test render::presets::case_{i} ... ok")))
        .collect();
    let quiet: Vec<String> = (0..30)
        .map(|i| format!("GET /api/items/{i} 200 in 12ms"))
        .collect();
    for (name, cmd, lines) in [
        ("cargo build", "cargo build --release", &build),
        ("cargo test", "cargo test -p rush-convo", &tests),
        ("a dev server", "npm run dev", &quiet),
    ] {
        let ls: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(
            (name == "a dev server") == rush_convo::render::progress_any(cmd, &ls).is_none(),
            "{name}"
        );
        report(
            name,
            measure(20_000, || {
                std::hint::black_box(rush_convo::render::progress_any(cmd, &ls));
            }),
        );
    }
}
