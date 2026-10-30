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

use rush_convo::{Item, Session, Tail};

struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            grow(l.size());
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
}

fn measure(f: impl FnOnce() -> Session) -> (Run, Session) {
    let base = NOW.load(Relaxed);
    PEAK.store(base, Relaxed);
    let total0 = TOTAL.load(Relaxed);
    let t = Instant::now();
    let s = f();
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let run = Run {
        ms,
        total: TOTAL.load(Relaxed) - total0,
        peak: PEAK.load(Relaxed) - base,
        retained: NOW.load(Relaxed) - base,
    };
    (run, s)
}

fn report(name: &str, runs: &[Run]) {
    let mut ms: Vec<f64> = runs.iter().map(|r| r.ms).collect();
    ms.sort_by(f64::total_cmp);
    let r = &runs[0];
    println!(
        "{name:<26} {:>7.1} ms (min {:.1})  allocated {:>6.1} MiB  peak {:>5.1} MiB  retained {:>5.1} MiB",
        ms[ms.len() / 2],
        ms[0],
        r.total as f64 / MIB,
        r.peak as f64 / MIB,
        r.retained as f64 / MIB,
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
}
