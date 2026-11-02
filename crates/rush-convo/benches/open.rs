//! Opens the largest real transcript (under 20 MB) under ~/.claude/projects
//! cold and applies all of it, read-only: time, bytes allocated, peak and
//! retained heap, by a counting allocator. Go's numbers for an 18–19 MB
//! transcript (docs/performance-review-2026-10-01.md): 55–76 MiB
//! allocated, about 22 MiB retained.
//!
//! `cargo bench -p rush-convo --bench open`; RUSH_BENCH_TRANSCRIPT=<file>
//! picks the transcript.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use rush_convo::{Item, Request, Session, Step, Tail, Turn};

struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Allocations made in all, and live.
static MADE: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            grow(l.size());
            MADE.fetch_add(1, Relaxed);
            LIVE.fetch_add(1, Relaxed);
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
            NOW.fetch_sub(l.size(), Relaxed);
            grow(new);
        }
        q
    }
}

fn grow(n: usize) {
    TOTAL.fetch_add(n, Relaxed);
    let now = NOW.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(now, Relaxed);
}

#[global_allocator]
static A: Counting = Counting;

fn largest(dir: &Path, best: &mut Option<(u64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() {
            if p.file_name().is_some_and(|n| n != "subagents") {
                largest(&p, best);
            }
        } else if p.extension().is_some_and(|x| x == "jsonl")
            && m.len() < 20 << 20
            && best.as_ref().is_none_or(|(n, _)| m.len() > *n)
        {
            *best = Some((m.len(), p));
        }
    }
}

const MIB: f64 = (1 << 20) as f64;

struct Run {
    ms: f64,
    total: usize,
    peak: usize,
    retained: usize,
    /// Allocations made, and those still live after.
    made: usize,
    live: usize,
}

fn measure(f: impl FnOnce() -> Session) -> (Run, Session) {
    let base = NOW.load(Relaxed);
    PEAK.store(base, Relaxed);
    let total0 = TOTAL.load(Relaxed);
    let (made0, live0) = (MADE.load(Relaxed), LIVE.load(Relaxed));
    let t = Instant::now();
    let s = f();
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let run = Run {
        ms,
        total: TOTAL.load(Relaxed) - total0,
        peak: PEAK.load(Relaxed) - base,
        retained: NOW.load(Relaxed) - base,
        made: MADE.load(Relaxed) - made0,
        live: LIVE.load(Relaxed).saturating_sub(live0),
    };
    (run, s)
}

/// The heap a copy of t holds: what t itself holds, near enough (a copy's
/// buffers are no bigger than they need be).
fn heap_of<T: Clone>(t: &T) -> usize {
    let base = NOW.load(Relaxed);
    let c = t.clone();
    let n = NOW.load(Relaxed).saturating_sub(base);
    drop(c);
    n
}

/// Where a session's retained heap is: each part copied, as a copy holds
/// what the original does.
#[derive(Default)]
struct Parts {
    steps: usize,
    calls: usize,
    raw: usize,
    output: usize,
    turns: usize,
    text: usize,
    requests: usize,
}

impl Parts {
    fn of(s: &Session) -> Parts {
        Parts {
            steps: heap_of(&s.steps().to_vec()),
            calls: s.steps().iter().map(|st| heap_of(st.call())).sum(),
            raw: s
                .steps()
                .iter()
                .map(|st| st.call().raw.as_ref().map_or(0, |r| r.get().len()))
                .sum(),
            output: s.steps().iter().map(|st| st.output.len()).sum(),
            turns: heap_of(&s.turns),
            text: s
                .turns
                .iter()
                .flat_map(|t| &t.items)
                .map(|it| match it {
                    Item::Text { text, .. } | Item::Thinking(text) => text.len(),
                    _ => 0,
                })
                .sum(),
            requests: heap_of(&s.requests),
        }
    }

    fn add(&mut self, o: &Parts) {
        self.steps += o.steps;
        self.calls += o.calls;
        self.raw += o.raw;
        self.output += o.output;
        self.turns += o.turns;
        self.text += o.text;
        self.requests += o.requests;
    }
}

impl std::fmt::Display for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = |n: usize| n as f64 / MIB;
        write!(
            f,
            "steps {:.1} MiB (calls {:.1}, their raw input {:.1}, output {:.1}), turns {:.1} MiB (text {:.1}), requests {:.1} MiB",
            m(self.steps),
            m(self.calls),
            m(self.raw),
            m(self.output),
            m(self.turns),
            m(self.text),
            m(self.requests),
        )
    }
}

fn report(name: &str, runs: &[Run]) {
    let mut ms: Vec<f64> = runs.iter().map(|r| r.ms).collect();
    ms.sort_by(f64::total_cmp);
    let r = &runs[0];
    println!(
        "{name:<26} {:>7.1} ms (min {:.1})  allocated {:>6.1} MiB in {} allocations  peak {:>5.1} MiB  retained {:>5.1} MiB in {} allocations",
        ms[ms.len() / 2],
        ms[0],
        r.total as f64 / MIB,
        r.made,
        r.peak as f64 / MIB,
        r.retained as f64 / MIB,
        r.live,
    );
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return; // cargo test --benches asks for the list
    }
    let path = match std::env::var_os("RUSH_BENCH_TRANSCRIPT") {
        Some(p) => PathBuf::from(p),
        None => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            let mut best = None;
            largest(&home.join(".claude/projects"), &mut best);
            match best {
                Some((_, p)) => p,
                None => {
                    println!("no transcript under ~/.claude/projects: nothing to measure");
                    return;
                }
            }
        }
    };
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("{} ({:.1} MiB)", path.display(), size as f64 / MIB);

    let mut whole = Vec::new();
    let mut end = Vec::new();
    let mut fetched = Vec::new();
    let mut last = None;
    for _ in 0..5 {
        let (r, s) = measure(|| {
            let mut s = Session::new();
            Tail::new(&path, rush_convo::decode(rush_claude::messages))
                .read(&mut s)
                .unwrap();
            s
        });
        whole.push(r);
        drop(last.replace(s));
        let (r, s) = measure(|| {
            let mut s = Session::new();
            Tail::from_end(&path, 2 << 20, rush_convo::decode(rush_claude::messages))
                .read(&mut s)
                .unwrap();
            s
        });
        end.push(r);
        drop(s);
        let (r, s) = measure(|| {
            let mut s = Session::new();
            let f = Tail::new(&path, rush_convo::decode(rush_claude::messages))
                .fetch()
                .unwrap();
            f.apply(&mut s);
            s
        });
        fetched.push(r);
        drop(s);
    }
    report("whole file, read", &whole);
    report("last 2 MB, read", &end);
    report("whole file, fetch+apply", &fetched);
    let s = last.unwrap();
    let items: usize = s.turns.iter().map(|t| t.items.len()).sum();
    let texts: usize = s
        .turns
        .iter()
        .flat_map(|t| &t.items)
        .map(|it| match it {
            Item::Text { text, .. } | Item::Thinking(text) => text.len(),
            _ => 0,
        })
        .sum();
    let out: usize = s.steps().iter().map(|st| st.output.len()).sum();
    let input: usize = s.steps().iter().map(|st| st.input_json().len()).sum();
    let result: usize = s.steps().iter().map(|st| st.result_json().len()).sum();
    let images: usize = s
        .steps()
        .iter()
        .flat_map(|st| &st.images)
        .map(|i| i.bytes)
        .sum::<usize>()
        + s.turns
            .iter()
            .flat_map(|t| &t.pictures)
            .map(|i| i.bytes)
            .sum::<usize>()
        + s.turns
            .iter()
            .flat_map(|t| &t.items)
            .map(|it| match it {
                Item::Interject(i) => i.pictures.iter().map(|p| p.bytes).sum(),
                _ => 0,
            })
            .sum::<usize>();
    println!(
        "{} turns, {} items, {} steps, {} requests · text {:.1} MiB, step output {:.1} MiB, input {:.1} MiB, result {:.1} MiB, images {:.1} MiB (not kept)",
        s.turns.len(),
        items,
        s.steps().len(),
        s.requests.len(),
        texts as f64 / MIB,
        out as f64 / MIB,
        input as f64 / MIB,
        result as f64 / MIB,
        images as f64 / MIB,
    );
    println!(
        "sizes: Step {} B, Turn {} B, Item {} B, Request {} B, Call {} B, Output {} B",
        size_of::<Step>(),
        size_of::<Turn>(),
        size_of::<Item>(),
        size_of::<Request>(),
        size_of::<rush_core::tool::Call>(),
        size_of::<rush_core::tool::Output>(),
    );
    println!("heap: {}", Parts::of(&s));
    // Its subagent runs, each read whole as the subagents view reads them
    // for their numbers: what the session's reader holds while that view
    // is open (and, before rush let go of runs no longer followed, after).
    let subs = path.with_extension("").join("subagents");
    let runs: Vec<PathBuf> = std::fs::read_dir(&subs)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
                .collect()
        })
        .unwrap_or_default();
    if !runs.is_empty() {
        let base = NOW.load(Relaxed);
        let read: Vec<(Tail, Session)> = runs
            .iter()
            .map(|p| {
                let mut t = Tail::subagent(p, rush_convo::decode(rush_claude::messages));
                let mut s = Session::new();
                let _ = t.read(&mut s);
                (t, s)
            })
            .collect();
        let held = NOW.load(Relaxed).saturating_sub(base);
        let mut parts = Parts::default();
        for (_, s) in &read {
            parts.add(&Parts::of(s));
        }
        println!(
            "its {} subagent runs read whole retain {:.1} MiB: {parts}",
            read.len(),
            held as f64 / MIB,
        );
        drop(read);
    }
    // A reader that keeps a session read on copies it when the UI still
    // holds the last one (Arc::make_mut), rather than reading it again.
    let (copied, c) = measure(|| s.clone());
    report("whole session, copied", &[copied]);
    drop(c);
    let t = Instant::now();
    drop(s);
    println!(
        "whole file, dropped        {:>7.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
}
