//! What `rush serve`'s watch for notifications held while it slept, until
//! it let go of it before sleeping (watch.rs): the listing of every
//! session on this machine, read every 4 seconds. Counted by the allocator
//! over a home of 1,244 sessions, as many as the home measured for
//! DESIGN.md "Background processes' memory". Run without the test harness,
//! so the number prints: `cargo test -p rush-remote --test listing`.
//! RUSH_BENCH_OUT=<file> also writes it there, for CI's footprint job
//! (tools/footprint).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

use rush_host::Info;
use rush_remote::Session;

/// Bytes allocated and not yet freed.
static NOW: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
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
            NOW.fetch_sub(l.size(), Relaxed);
            NOW.fetch_add(new, Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

const SESSIONS: usize = 1_244;

/// Appends a number to $RUSH_BENCH_OUT as `name<TAB>kind<TAB>value`, for
/// CI's footprint job (tools/footprint/measure.py reads it).
fn record(name: &str, kind: &str, v: usize) {
    use std::io::Write;
    let Some(p) = std::env::var_os("RUSH_BENCH_OUT") else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{name}\t{kind}\t{v}");
    }
}

fn main() {
    rush_testenv::assert_sandboxed();
    for i in 0..SESSIONS {
        let id = format!("{i:08x}");
        let info = Info {
            id: id.clone(),
            session_id: format!("{id}-0000-4000-8000-000000000000"),
            account: "default".into(),
            cwd: "/Users/someone/Source/github.com/someone/project".into(),
            name: format!("fix the flaky upgrade test in the host module {i}"),
            state: "stopped".into(),
            model: "claude-opus-5-5".into(),
            ..Info::default()
        };
        let d = rush_host::paths::dir(&id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
    }
    // What the watch makes of a listing (Server::sessions).
    let base = NOW.load(Relaxed);
    let listing: Vec<Session> = rush_host::list()
        .into_iter()
        .map(|info| Session { info, alive: false })
        .collect();
    let held = NOW.load(Relaxed).saturating_sub(base);
    println!(
        "a listing of {} sessions: {held} B ({} B each)",
        listing.len(),
        held / listing.len().max(1)
    );
    record("serve: a listing, retained", "heap", held);
    assert_eq!(listing.len(), SESSIONS);
    assert!(held > 1 << 20, "{held} B");
}
