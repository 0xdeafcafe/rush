//! A run followed while it writes: its transcript grows a line at a time,
//! each write a wake, and on each wake the reader reads what it gained.
//! What that costs in bytes read, allocations made and bytes allocated,
//! by a counting allocator: read on from where it got to (a run's
//! numbers), and read on into a conversation the UI holds too (a run read
//! whole, rush-ui-session-core subs.rs `full`), which copies it first,
//! once each write and as rush reads it now (FULL_EVERY: at most once a
//! second; writes ten a second here).
//!
//! `cargo bench -p rush-convo --bench follow`; RUSH_BENCH_TRANSCRIPT=<file>
//! picks the run (else a made-up one of 2,000 lines).

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use rush_convo::{Session, Tail};

struct Counting;

static TOTAL: AtomicUsize = AtomicUsize::new(0);
static MADE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            TOTAL.fetch_add(l.size(), Relaxed);
            MADE.fetch_add(1, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            TOTAL.fetch_add(new, Relaxed);
            MADE.fetch_add(1, Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

const MIB: f64 = (1 << 20) as f64;
/// Writes a second, and the wakes between two readings of a run read
/// whole as rush reads it now (FULL_EVERY, a second).
const PER_SEC: usize = 10;

fn made_up() -> Vec<String> {
    (0..2000)
        .map(|i| {
            let id = format!("t{i}");
            if i % 2 == 0 {
                format!(
                    r#"{{"type":"assistant","isSidechain":true,"timestamp":"2026-11-02T10:00:00Z","message":{{"id":"m{i}","role":"assistant","model":"claude-opus-5-5","content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"cd /work/rush && cargo test -p rush-convo --lib model::tests::case_{i}","description":"Run the tests"}}}}],"usage":{{"input_tokens":10,"output_tokens":200,"cache_read_input_tokens":50000}}}}}}"#
                )
            } else {
                format!(
                    r#"{{"type":"user","isSidechain":true,"timestamp":"2026-11-02T10:00:01Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{}","content":"{}"}}]}}}}"#,
                    i - 1,
                    "test result: ok. 107 passed; 0 failed\\n".repeat(20)
                )
            }
        })
        .collect()
}

struct Cost {
    read: u64,
    made: usize,
    bytes: usize,
}

/// Writes lines to path one at a time, calling wake after each with the
/// write's number; what the wakes cost.
fn grow(path: &Path, lines: &[String], mut wake: impl FnMut(usize)) -> Cost {
    std::fs::write(path, "").unwrap();
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    let (made, bytes) = (MADE.load(Relaxed), TOTAL.load(Relaxed));
    let mut cost = Cost {
        read: 0,
        made: 0,
        bytes: 0,
    };
    for (i, l) in lines.iter().enumerate() {
        f.write_all(l.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
        wake(i);
    }
    cost.made = MADE.load(Relaxed) - made;
    cost.bytes = TOTAL.load(Relaxed) - bytes;
    cost.read = std::fs::metadata(path).unwrap().len();
    cost
}

fn report(name: &str, c: &Cost, reads: usize) {
    println!(
        "{name:<44} {reads:>5} readings  {:>8.1} MiB read  {:>10} allocations  {:>8.1} MiB allocated",
        c.read as f64 / MIB,
        c.made,
        c.bytes as f64 / MIB,
    );
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let lines: Vec<String> = match std::env::var_os("RUSH_BENCH_TRANSCRIPT") {
        Some(p) => std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect(),
        None => made_up(),
    };
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("agent-a1.jsonl");
    println!("a run of {} lines, written {PER_SEC} a second", lines.len());
    let decode = || rush_convo::decode(rush_claude::messages);

    // Its numbers: read on from where it got to, each wake.
    let mut t = Tail::subagent(&path, decode());
    let mut s = Session::new();
    let c = grow(&path, &lines, |_| {
        let _ = t.read(&mut s);
    });
    report("read on, each write", &c, lines.len());
    drop((t, s));

    // Read whole and shared with the UI, which holds the last reading:
    // each reading copies it first.
    for (name, every) in [
        ("read whole, shared, each write", 1),
        ("read whole, shared, once a second", PER_SEC),
    ] {
        let mut t = Tail::subagent(&path, decode());
        let mut s = Arc::new(Session::new());
        let mut ui = s.clone();
        let mut reads = 0;
        let c = grow(&path, &lines, |i| {
            if i % every != every - 1 && i + 1 != lines.len() {
                return;
            }
            reads += 1;
            let _ = t.read(Arc::make_mut(&mut s));
            ui = s.clone();
        });
        report(name, &c, reads);
        drop(ui);
    }
}
