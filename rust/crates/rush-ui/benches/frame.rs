//! What a frame of the Agents list costs: building its rows (Model::frame),
//! drawing them on a screen in memory, and a key moving the pick then the
//! frame after it. Time per frame, and bytes and allocations per frame by
//! a counting allocator. 300 agents in 12 projects, 160x48, as a busy
//! rush has.
//!
//! `cargo bench -p rush-ui --bench frame` (run it through
//! rust/tools/sbtest.sh: the model is a real one, in a throwaway home);
//! with `-- profile` it draws for 20 s instead, for `sample`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use rush_core::agent::{Agent, State};
use rush_draw::{App, Headless, Key, KeyMods};
use rush_ui::{Fleet, Model, Note};

struct Counting;

static BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        BYTES.fetch_add(l.size(), Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        BYTES.fetch_add(new, Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

const NOW: i64 = 1_793_190_896_789;
const MIN: i64 = 60_000;

fn agents(n: usize) -> Vec<Agent> {
    let states = [
        State::Working,
        State::Blocked,
        State::Idle,
        State::Done,
        State::Stopped,
    ];
    (0..n)
        .map(|i| {
            let repo = format!("/src/project-{}", i % 12);
            let mut a = Agent {
                key: format!("default/a{i:04}"),
                display_name: format!("agent {i}: fix the flaky tax rounding test in checkout"),
                repo: repo.clone(),
                branch: format!("rush/task-{i}"),
                pid: if i % 5 < 3 { 1000 + i as i32 } else { 0 },
                interactive: true,
                cpu: (i % 7) as f32 * 3.5,
                mem: (i as u64 % 9) << 26,
                past: i % 5 >= 3,
                ..Agent::default()
            };
            a.job.id = format!("a{i:04}");
            a.job.state = states[i % states.len()];
            a.job.cwd = repo;
            a.job.detail = "running pnpm test".into();
            if a.job.state == State::Blocked {
                a.job.needs = "Round half up, or half to even?".into();
            }
            a.job.created_at = NOW - (20 + i as i64) * MIN;
            a.job.updated_at = NOW - (1 + i as i64) * MIN;
            a
        })
        .collect()
}

fn model(n: usize, w: usize, h: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = w;
    m.h = h;
    m.launch_dir = "/src/project-0".into();
    let agents = agents(n);
    let roots = agents.iter().map(|a| a.repo.clone()).collect();
    m.set_fleet(Fleet {
        agents,
        roots,
        previews: HashMap::new(),
        priced: true,
    });
    m
}

/// Instructions this process has retired (macOS), which a busy machine
/// doesn't change as it does the time taken; 0 elsewhere.
fn instructions() -> u64 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: an all-zero rusage_info_v4 is valid, and
        // proc_pid_rusage fills the one it is given.
        let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as i32,
                libc::RUSAGE_INFO_V4,
                (&mut ri as *mut libc::rusage_info_v4).cast(),
            )
        };
        if r == 0 {
            return ri.ri_instructions;
        }
    }
    0
}

/// What one run costs, on average.
struct Cost {
    us: f64,
    instr: u64,
    bytes: usize,
    allocs: usize,
}

/// Runs f n times: what each run cost.
fn measure(n: usize, mut f: impl FnMut(usize)) -> Cost {
    let (b, a, i0) = (BYTES.load(Relaxed), ALLOCS.load(Relaxed), instructions());
    let t = Instant::now();
    for i in 0..n {
        f(i);
    }
    Cost {
        us: t.elapsed().as_secs_f64() * 1e6 / n as f64,
        instr: (instructions() - i0) / n as u64,
        bytes: (BYTES.load(Relaxed) - b) / n,
        allocs: (ALLOCS.load(Relaxed) - a) / n,
    }
}

fn report(name: &str, c: Cost) {
    println!(
        "{name:<28} {:>8.1} µs {:>8.2} M instr {:>8.1} KiB {:>6} allocs",
        c.us,
        c.instr as f64 / 1e6,
        c.bytes as f64 / 1024.0,
        c.allocs
    );
}

fn main() {
    rush_testenv::assert_sandboxed();
    let (w, h) = (160u16, 48u16);
    let mut m = model(300, w as usize, h as usize);
    let mut hd = Headless::new(w, h);
    // Warm: the first frame reads what it reads once.
    hd.draw(|c| App::<Note>::draw(&mut m, c));
    // `frame profile`: drawn over and over for 20 s, for a profiler.
    if std::env::args().any(|a| a == "profile") {
        let t = Instant::now();
        let mut i = 0usize;
        while t.elapsed().as_secs() < 20 {
            let k = if (i / 20) % 2 == 0 {
                Key::Down
            } else {
                Key::Up
            };
            m.key(k, KeyMods::NONE);
            hd.draw(|c| App::<Note>::draw(&mut m, c));
            i += 1;
        }
        return;
    }
    println!("— Agents list, 300 agents, {w}x{h}");
    report(
        "rows (Model::frame)",
        measure(300, |_| {
            std::hint::black_box(m.frame());
        }),
    );
    report(
        "frame drawn (rows + cells)",
        measure(300, |_| hd.draw(|c| App::<Note>::draw(&mut m, c))),
    );
    report(
        "down or up, then the frame",
        measure(300, |i| {
            let k = if (i / 20) % 2 == 0 {
                Key::Down
            } else {
                Key::Up
            };
            m.key(k, KeyMods::NONE);
            hd.draw(|c| App::<Note>::draw(&mut m, c));
        }),
    );
}
