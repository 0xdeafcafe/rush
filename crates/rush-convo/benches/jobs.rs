//! What the session's dock asks of its tasks each frame: for each run it
//! shows, the workflow task it's under (rush-ui-session-core workflow.rs
//! workflow_job), a scan of every task's kind. Session::job_kind looked
//! the task's call up (a hash of its id) for every task; now only for the
//! two kinds the call can change. "lookup each" is the old way, kept here
//! to set beside it. Time per frame and bytes allocated per frame.
//!
//! `cargo bench -p rush-convo --bench jobs`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

#[path = "../tests/common/mod.rs"]
mod common;

use common::{Feed, at, bench_session, tool_use};
use rush_convo::{Job, Session};
use rush_core::event::{Event, TaskKind, TaskStarted};
use serde_json::json;

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
    println!("{name:<34} {us:>10.2} µs {bytes:>8} B/frame");
}

/// A long session with tasks tasks of every kind, each started by a call.
fn session(tasks: usize) -> Session {
    let mut s = bench_session(100);
    for i in 0..tasks {
        let (tool, kind) = match i % 5 {
            0 => ("Bash", TaskKind::Shell),
            1 => ("Agent", TaskKind::Subagent),
            2 => ("Monitor", TaskKind::Shell),
            3 => ("Workflow", TaskKind::Workflow),
            _ => ("Bash", TaskKind::Other),
        };
        let call = format!("toolu_{i:024}");
        s.feed(
            tool_use(&call, tool, json!({"command": "true"})),
            at(10_000 + i as i64),
        );
        s.feed(
            Event::TaskStarted(TaskStarted {
                id: format!("task{i}"),
                call_id: call,
                kind,
                label: format!("task {i}"),
                ..TaskStarted::default()
            }),
            at(10_000 + i as i64),
        );
    }
    s
}

/// job_kind as it was: the call looked up for every task.
fn kind_looked_up(s: &Session, j: &Job) -> &'static str {
    let tool = s.step_by_call(&j.tool_use_id).map(|id| &*s.step(id).tool);
    match (j.kind_word.as_str(), tool) {
        ("local_bash", Some("Monitor")) => "monitor",
        ("", Some("Agent" | "Task")) => "subagent",
        _ => j.kind(),
    }
}

fn main() {
    for (tasks, runs) in [(50, 10), (300, 40)] {
        let s = session(tasks);
        println!("— {tasks} tasks, {runs} runs in the dock");
        // Both say the same of every task.
        for j in s.jobs() {
            assert_eq!(s.job_kind(j), kind_looked_up(&s, j), "{}", j.id);
        }
        let scan = |kind: &dyn Fn(&Job) -> &'static str| {
            let mut n = 0;
            for _ in 0..runs {
                n += s.jobs().iter().filter(|j| kind(j) == "workflow").count();
            }
            std::hint::black_box(n);
        };
        report(
            "dock's scan, lookup each (before)",
            measure(2000, || scan(&|j| kind_looked_up(&s, j))),
        );
        report(
            "dock's scan, job_kind (now)",
            measure(2000, || scan(&|j| s.job_kind(j))),
        );
    }
}
