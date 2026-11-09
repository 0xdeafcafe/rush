//! What the session's Subagents dock works out of its runs each frame
//! (rush-ui-session-core subs.rs): the runs again from those listed, the
//! agents found and those only a task says (Runs::rebuild, job_subs),
//! then the ones still working (Runs::running), as refresh, the dock's
//! sync and the tab strip each ask. 300 runs and 300 subagent tasks, as
//! a session with hundreds of subagents has. job_subs looked every task
//! up against every run (tasks × runs compares); now by a set. Time and
//! bytes allocated per frame.
//!
//! `cargo bench -p rush-ui-session-core --bench subs`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use rush_convo::{Session, Subagent};
use rush_core::event::{Event, TaskKind, TaskStarted};
use rush_ui_session_core::subs::{Runs, job_subs};

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
    println!("{name:<40} {us:>10.2} µs {bytes:>9} B/frame");
}

/// A session with tasks subagent tasks, and runs runs listed, every
/// other one started by one of the tasks' calls.
fn session(tasks: usize, runs: usize) -> (Session, Runs) {
    let mut s = Session::new();
    for i in 0..tasks {
        s.apply(
            &Event::TaskStarted(TaskStarted {
                id: format!("task{i}"),
                call_id: format!("toolu_{i:024}"),
                kind: TaskKind::Subagent,
                label: format!("task {i}"),
                ..TaskStarted::default()
            }),
            10_000 + i as i64,
        );
    }
    let mut r = Runs::default();
    r.listed.extend((0..runs).map(|i| Subagent {
        id: format!("agent-{i:016x}"),
        typ: "general-purpose".into(),
        description: format!("run {i}"),
        tool_use_id: if i % 2 == 0 {
            format!("toolu_{i:024}")
        } else {
            String::new()
        },
        depth: 1,
        path: format!("/tmp/subagents/agent-{i:016x}.jsonl"),
        born: 10_000 + i as i64,
        modified: 20_000 + i as i64,
        ..Subagent::default()
    }));
    (s, r)
}

/// job_subs as it was: every run scanned for each task.
fn job_subs_scanned(s: &Session, have: &[Subagent], claude: bool) -> usize {
    s.jobs()
        .iter()
        .filter(|j| s.job_kind(j) == "subagent" && (!claude || j.running()))
        .filter(|j| {
            !have.iter().any(|sa| {
                !j.tool_use_id.is_empty() && sa.tool_use_id == j.tool_use_id
                    || claude && sa.id == j.id
            })
        })
        .count()
}

fn main() {
    for (tasks, runs) in [(50, 50), (300, 300)] {
        let (s, mut r) = session(tasks, runs);
        println!("— {runs} runs, {tasks} subagent tasks");
        let listed = r.listed.clone();
        assert_eq!(
            job_subs(&s, &listed, true).len(),
            job_subs_scanned(&s, &listed, true),
            "both find the same tasks"
        );
        report(
            "job_subs, every run scanned (before)",
            measure(200, || {
                std::hint::black_box(job_subs_scanned(&s, &listed, true));
            }),
        );
        report(
            "job_subs, looked up (now)",
            measure(200, || {
                std::hint::black_box(job_subs(&s, &listed, true));
            }),
        );
        report(
            "rebuild",
            measure(200, || {
                r.rebuild(&s, true);
                std::hint::black_box(r.subs.len());
            }),
        );
        report(
            "running, asked three times a frame",
            measure(200, || {
                for _ in 0..3 {
                    std::hint::black_box(r.running(&s, 30_000));
                }
            }),
        );
    }
}
