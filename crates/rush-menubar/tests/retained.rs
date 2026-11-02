//! What the feed keeps of a reading while it waits, counted by the
//! allocator: every row, as it was, against only those working or waiting
//! on you (`source::live_rows`). A home like the one measured for DESIGN.md
//! "Background processes' memory": 900 rows, 720 past conversations, 175
//! finished, 5 working. Run without the test harness, so the numbers
//! print: `cargo test -p rush-menubar --test retained`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

use rush_core::agent::{Agent, State};
use rush_menubar::source::{Row, live_rows};

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

fn agent(i: usize, state: State) -> Agent {
    let mut a = Agent {
        kind: "claude".into(),
        key: format!("claude:default:i:{i:08x}"),
        display_name: format!("fix the flaky upgrade test in the host module {i}"),
        repo: "/Users/someone/Source/github.com/someone/project".into(),
        pid: if state == State::Working {
            4000 + i as i32
        } else {
            0
        },
        ..Agent::default()
    };
    a.job.id = format!("{i:08x}");
    a.job.state = state;
    a.job.account = "default".into();
    a.job.detail = "reading crates/rush-host/src/server.rs".into();
    a.job.transcript_path = format!(
        "/Users/someone/.claude/projects/-Users-someone-Source-github-com-someone-project/{i:08x}-0000-4000-8000-000000000000.jsonl"
    );
    a
}

/// The bytes rows hold, counted from before f makes them.
fn retained(f: impl FnOnce() -> Vec<Row>) -> (usize, Vec<Row>) {
    let base = NOW.load(Relaxed);
    let rows = f();
    (NOW.load(Relaxed).saturating_sub(base), rows)
}

fn main() {
    rush_testenv::assert_sandboxed();
    let agents: Vec<Agent> = (0..900)
        .map(|i| {
            let st = match i {
                0..5 => State::Working,
                5..725 => State::Stopped,
                _ => State::Done,
            };
            agent(i, st)
        })
        .collect();
    let (every, all) = retained(|| agents.iter().map(Row::from).collect());
    let (live, rows) = retained(|| live_rows(&agents));
    println!(
        "a reading of {} rows: every row {every} B, those working or waiting {live} B ({} rows)",
        all.len(),
        rows.len()
    );
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(Row::live));
    // Only the live rows were left out: what's kept is what was.
    let kept: Vec<&Row> = all.iter().filter(|r| r.live()).collect();
    assert_eq!(kept, rows.iter().collect::<Vec<_>>());
    assert!(live * 100 < every, "{live} B against {every} B");
}
