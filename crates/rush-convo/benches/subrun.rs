//! A subagent's run in the Subagents pane: one turn of 5,000 steps (what
//! it was asked, then its calls), drawn 72 wide and 30 rows tall, as the
//! pane beside the list draws it. The renderer lays out a turn at a time,
//! so a run is laid out whole however little of it shows; what matters is
//! what the UI thread pays for it:
//!
//! - its first frame, laid out on the UI thread (as the pane did), and
//!   laid out ahead by the reader off the UI thread (as it does now:
//!   subs.rs read_detail's warm), the frame then drawing nothing new;
//! - a key moving the pick a row, every part of the turn drawn again (as
//!   it was: the pick was in each part's memo key) and now, two parts
//!   (units.rs memoized: the pick only in the parts it comes to or leaves);
//! - a frame of a run still working, its clock ticking.
//!
//! `cargo bench -p rush-convo --bench subrun`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

#[path = "../tests/common/mod.rs"]
mod common;

use common::{Feed, at, done, say, sent, tool_result, tool_use};
use rush_convo::{Options, Renderer, Session};
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

const STEPS: usize = 5000;
const W: usize = 72;
const ROWS: usize = 30;

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
        "{name:<46} {us:>11.1} µs {:>10.1} KiB/op",
        bytes as f64 / 1024.0
    );
}

/// A run of steps calls, a word now and then between them; ended, or
/// still at work.
fn run(steps: usize, ended: bool) -> Session {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    let mut sec = 0;
    let mut add = |s: &mut Session, ev: common::Ev| {
        s.feed(ev, at(sec));
        sec += 1;
    };
    add(&mut s, sent("port the subagents pane and measure it"));
    for i in 0..steps {
        if i % 50 == 0 {
            add(&mut s, say(&format!("Reading the next part, {i} so far.")));
        }
        let id = format!("toolu_{i:06}");
        add(
            &mut s,
            tool_use(
                &id,
                "Read",
                json!({"file_path": format!("/work/rush/src/part{i}.rs")}),
            ),
        );
        add(
            &mut s,
            tool_result(&id, "fn part() {}\nfn more() {}\n", false, None),
        );
    }
    if ended {
        add(&mut s, say("Done: every part read."));
        add(&mut s, done(0.0));
    }
    s
}

fn opts() -> Options {
    Options {
        width: W,
        now: at(100_000),
        ..Options::default()
    }
}

fn main() {
    let s = run(STEPS, true);
    println!("— a run of {STEPS} steps in one turn, {W} wide, {ROWS} rows");
    let o = opts();
    report(
        "first frame, laid out on the UI thread (before)",
        measure(5, |_| {
            let mut r = Renderer::new();
            std::hint::black_box(r.tail(&s, &o, ROWS).len());
        }),
    );
    // Laid out by the reader as the pane last drew it (warm): the frame
    // that shows it finds the turn drawn.
    let mut r = Renderer::new();
    r.tail(&s, &o, ROWS);
    report(
        "first frame, laid out off the UI thread (now)",
        measure(200, |_| {
            std::hint::black_box(r.tail(&s, &o, ROWS).len());
        }),
    );
    // The refs of the rows at the end: a key moves the pick between two.
    let refs: Vec<String> = r
        .tail(&s, &o, ROWS)
        .iter()
        .map(|l| l.r#ref.clone())
        .filter(|r| r.contains(":s:"))
        .collect();
    assert!(refs.len() >= 2, "no steps in sight");
    let pick = |i: usize| {
        let mut o = opts();
        o.selected = refs[i % 2].clone();
        o
    };
    report(
        "↑: the pick moves, every part drawn (before)",
        measure(5, |i| {
            // What it cost then: the pick in each part's key, every part
            // missed, the turn drawn as if new.
            let mut r = Renderer::new();
            std::hint::black_box(r.tail(&s, &pick(i), ROWS).len());
        }),
    );
    let mut r = Renderer::new();
    r.tail(&s, &pick(1), ROWS);
    report(
        "↑: the pick moves, two parts drawn (now)",
        measure(50, |i| {
            std::hint::black_box(r.tail(&s, &pick(i), ROWS).len());
        }),
    );
    // Still at work: its turn is drawn again as its clock ticks, the
    // parts that are done copied.
    let live = run(STEPS, false);
    let mut r = Renderer::new();
    r.tail(&live, &o, ROWS);
    report(
        "a working run's frame, its clock ticking",
        measure(50, |i| {
            let mut o = opts();
            o.tick = i + 1;
            std::hint::black_box(r.tail(&live, &o, ROWS).len());
        }),
    );
}
