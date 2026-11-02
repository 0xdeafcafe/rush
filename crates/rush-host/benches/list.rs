//! What the list holds in memory: the heap a reading of this machine's
//! real agents keeps (its rows, and the loader's own memo of them, rush's
//! sessions read through the host module as the binary reads them),
//! measured by a counting allocator, and the size of a row.
//!
//!   RUSH_REAL=1 cargo bench -p rush-host --bench list
//!
//! Reads ~/.claude and rush's state read only, as rush-fleet's load bench
//! does; it brings back no lost session (a lister that follows).
//! Without RUSH_REAL it prints the types' sizes and nothing more.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use rush_core::adapter::Registry;
use rush_core::agent::{Agent, Job, Preview, Spend};
use rush_core::time::now;
use rush_fleet::Loader;

struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
/// Allocations live, and made in all.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static MADE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            NOW.fetch_add(l.size(), Relaxed);
            LIVE.fetch_add(1, Relaxed);
            MADE.fetch_add(1, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        NOW.fetch_sub(l.size(), Relaxed);
        LIVE.fetch_sub(1, Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            NOW.fetch_add(new, Relaxed);
            NOW.fetch_sub(l.size(), Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

const KIB: f64 = 1024.0;
const MIB: f64 = (1 << 20) as f64;

/// The heap a copy of t holds: what t itself holds, near enough.
fn heap_of<T: Clone>(t: &T) -> usize {
    heap_allocs(t).0
}

/// The heap a copy of t holds, and in how many allocations.
fn heap_allocs<T: Clone>(t: &T) -> (usize, usize) {
    let (base, live) = (NOW.load(Relaxed), LIVE.load(Relaxed));
    let c = t.clone();
    let n = NOW.load(Relaxed).saturating_sub(base);
    let a = LIVE.load(Relaxed).saturating_sub(live);
    drop(c);
    (n, a)
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return; // cargo test --benches asks for the list
    }
    println!(
        "sizes: Agent {} B, Job {} B, Spend {} B, Preview {} B, Profile {} B, Workspace {} B, PrStatus {} B, Compaction {} B, HostInfo {} B, Totals {} B",
        size_of::<Agent>(),
        size_of::<Job>(),
        size_of::<Spend>(),
        size_of::<Preview>(),
        size_of::<rush_core::adapter::Profile>(),
        size_of::<rush_core::agent::Workspace>(),
        size_of::<rush_core::agent::PrStatus>(),
        size_of::<rush_core::adapter::Compaction>(),
        size_of::<rush_fleet::HostInfo>(),
        size_of::<rush_claude::transcript::Totals>(),
    );
    if std::env::var_os("RUSH_REAL").is_none() {
        println!("set RUSH_REAL=1 to read this machine's agents");
        return;
    }
    raise_open_files();
    rush_fleet::use_hosts(|| Box::new(rush_host::hosts::Sessions::following()));
    let reg = Registry::new();
    reg.register(rush_claude::Claude::at(rush_claude::Account::home()));
    let (base, live) = (NOW.load(Relaxed), LIVE.load(Relaxed));
    let mut l = Loader::from_state(reg);
    let first = l.load_unsampled(now());
    println!(
        "after a first reading: {:.1} MiB",
        (NOW.load(Relaxed) - base) as f64 / MIB
    );
    let t = std::time::Instant::now();
    l.scan(&first.agents);
    let first_scan = t.elapsed();
    println!(
        "after its scan: {:.1} MiB ({:.0} ms)",
        (NOW.load(Relaxed) - base) as f64 / MIB,
        first_scan.as_secs_f64() * 1e3,
    );
    // The next scan, as the list makes one each reading: most transcripts
    // haven't changed.
    let (held, kept) = (NOW.load(Relaxed), LIVE.load(Relaxed));
    let t = std::time::Instant::now();
    l.scan(&first.agents);
    println!(
        "a scan again: {:.1} ms, {:+.2} MiB in {:+} allocations kept",
        t.elapsed().as_secs_f64() * 1e3,
        (NOW.load(Relaxed) as f64 - held as f64) / MIB,
        LIVE.load(Relaxed) as i64 - kept as i64,
    );
    drop(first);
    // A second reading, priced, as the list shows after its first scan.
    let made = MADE.load(Relaxed);
    let snap = l.load_unsampled(now());
    let made = MADE.load(Relaxed) - made;
    let held = NOW.load(Relaxed) - base;
    let (rows, allocs) = heap_allocs(&snap.agents);
    let n = snap.agents.len().max(1);
    println!(
        "{} rows: loader and reading retain {:.1} MiB; the rows {:.1} MiB in {} allocations ({:.1} KiB, {} allocations a row)",
        snap.agents.len(),
        held as f64 / MIB,
        rows as f64 / MIB,
        allocs,
        rows as f64 / n as f64 / KIB,
        allocs / n,
    );
    println!("a reading made {made} allocations");
    let jobs: usize = snap.agents.iter().map(|a| heap_of(&a.job)).sum();
    let spend: usize = snap.agents.iter().map(|a| heap_of(&a.spend)).sum();
    println!(
        "of the rows: jobs {:.1} MiB, spend {:.1} MiB",
        jobs as f64 / MIB,
        spend as f64 / MIB,
    );
    drop(snap);
    println!(
        "the loader alone retains {:.1} MiB in {} allocations",
        (NOW.load(Relaxed) - base) as f64 / MIB,
        LIVE.load(Relaxed) - live,
    );
    // As the menu bar's feed reads: past conversations left out.
    let (base, live) = (NOW.load(Relaxed), LIVE.load(Relaxed));
    let reg = Registry::new();
    reg.register(rush_claude::Claude::at(rush_claude::Account::home()));
    let mut f = Loader::from_state(reg);
    f.handle().skip_past(true);
    let first = f.load_unsampled(now());
    f.scan(&first.agents);
    let rows = first.agents.len();
    drop(first);
    let snap = f.load_unsampled(now());
    f.scan(&snap.agents);
    drop(snap);
    println!(
        "past conversations left out (the feed's): {rows} rows; the loader alone retains {:.1} MiB in {} allocations",
        (NOW.load(Relaxed) - base) as f64 / MIB,
        LIVE.load(Relaxed) - live,
    );
    drop(f);
    if let Some(secs) = std::env::var("RUSH_BENCH_PAUSE")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        // Held for a look with heap(1) or malloc_history(1).
        println!("pid {}", std::process::id());
        std::thread::sleep(std::time::Duration::from_secs(secs));
        drop(l);
    }
}

/// Raises the soft limit on open files as the binary does.
fn raise_open_files() {
    // SAFETY: zeroed is a valid rlimit; getrlimit fills it.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out-pointer.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } != 0 {
        return;
    }
    r.rlim_cur = r.rlim_max.min(10240);
    // SAFETY: a valid rlimit.
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &r) };
}
