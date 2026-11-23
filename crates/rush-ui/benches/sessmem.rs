//! What a day of sessions leaves held (rush-ui-session-core evict.rs): 30
//! sessions opened one after another, 5 of them 5,000 turns long with 30
//! agents rush hosted for each (spawned, each read whole while its
//! session shows) and 40 subagents (read whole while Changes shows, which
//! two of them open), 25 of them 40 turns with 2 hosted agents. Each is
//! opened, its runs read as its dock does, a frame drawn, then the next
//! opened; then back to the list. Live heap by a counting allocator
//! (bytes allocated less those freed): what each kind of thing holds
//! read alone, what's held once all 30 were opened, and what's held once
//! the sweep runs EVICT_AFTER later. Then the first heavy session opened
//! again, its hosted agents read again.
//!
//! `cargo bench -p rush-ui --bench sessmem` (in a throwaway home: the
//! bench links rush-testenv; its transcripts are made in a temp dir).

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

use rush_core::agent::{Agent, State};
use rush_ui::{Fleet, Model};
use rush_ui_session_core::{conn, core, evict, subs};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size() as isize, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as isize, Relaxed);
        unsafe { System.dealloc(p, l) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        LIVE.fetch_add(new as isize - l.size() as isize, Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn live() -> isize {
    LIVE.load(Relaxed)
}

fn mib(b: isize) -> String {
    format!("{:>8.1} MiB", b as f64 / (1 << 20) as f64)
}

const NOW: i64 = 1_793_190_896_789;
const HEAVY: usize = 5;
const SESSIONS: usize = 30;

/// An ISO time s seconds into 2026-09-23.
fn ts(s: u64) -> String {
    let (d, s) = (s / 86_400, s % 86_400);
    format!(
        "2026-09-{:02}T{:02}:{:02}:{:02}Z",
        23 + d,
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Text n bytes long, words that differ by seed.
fn words(seed: usize, n: usize) -> String {
    let mut s = String::with_capacity(n + 16);
    let mut i = seed;
    while s.len() < n {
        s.push_str(
            [
                "checkout ",
                "rounding ",
                "the test ",
                "flaky ",
                "tax ",
                "fixed ",
            ][i % 6],
        );
        i = i.wrapping_mul(31).wrapping_add(7);
    }
    s
}

fn j(v: serde_json::Value) -> String {
    v.to_string()
}

/// A parent session: turns turns, each a prompt, a shell call and its
/// output, and a reply. Turn k*step's call is the one hosted agent k ran.
fn parent(turns: usize) -> String {
    let mut out = String::new();
    let mut t = 0u64;
    for i in 0..turns {
        let mut line = |mut v: serde_json::Value| {
            v["timestamp"] = ts(t).into();
            out.push_str(&j(v));
            out.push('\n');
            t += 2;
        };
        line(serde_json::json!({"type":"user",
            "message":{"role":"user","content":format!("turn {i}: {}", words(i, 120))}}));
        line(serde_json::json!({"type":"assistant",
            "message":{"id":format!("m{i}a"),"role":"assistant","model":"claude-opus-4-1",
            "content":[{"type":"text","text":words(i + 1, 300)},
                {"type":"tool_use","id":format!("toolu_p{i}"),"name":"Bash",
                 "input":{"command":format!("cargo test -p mod{i} -- --nocapture")}}],
            "usage":{"input_tokens":1200,"output_tokens":90}}}));
        line(serde_json::json!({"type":"user",
            "message":{"role":"user","content":[{"type":"tool_result",
            "tool_use_id":format!("toolu_p{i}"),"content":words(i + 2, 800)}]}}));
        line(serde_json::json!({"type":"assistant",
            "message":{"id":format!("m{i}b"),"role":"assistant","model":"claude-opus-4-1",
            "content":[{"type":"text","text":words(i + 3, 400)}],
            "usage":{"input_tokens":1300,"output_tokens":120}}}));
    }
    out
}

/// A run's conversation: a prompt, then steps reads, each with 2 KB of
/// what it read, and its reply; side marks a subagent's (sidechain).
fn run(steps: usize, side: bool) -> String {
    let mut out = String::new();
    let mut t = 0u64;
    let mut line = |mut v: serde_json::Value| {
        v["isSidechain"] = side.into();
        v["timestamp"] = ts(t).into();
        out.push_str(&j(v));
        out.push('\n');
        t += 1;
    };
    line(serde_json::json!({"type":"user","message":{"role":"user","content":words(1, 400)}}));
    for i in 0..steps {
        line(
            serde_json::json!({"type":"assistant","message":{"id":format!("r{i}"),
            "role":"assistant","model":"claude-sonnet-4-5","content":[
            {"type":"text","text":words(i, 160)},
            {"type":"tool_use","id":format!("toolu_r{i}"),"name":"Read",
             "input":{"file_path":format!("/src/project/mod{i}.rs")}}],
            "usage":{"input_tokens":900,"output_tokens":60}}}),
        );
        line(
            serde_json::json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":format!("toolu_r{i}"),
             "content":words(i + 9, 2048)}]}}),
        );
    }
    line(
        serde_json::json!({"type":"assistant","message":{"id":"rz","role":"assistant",
        "content":[{"type":"text","text":words(5, 600)}]}}),
    );
    out
}

struct Plan {
    key: String,
    path: String,
    /// The hosted agents' steps and transcripts.
    spawns: Vec<(String, String)>,
    changes: bool,
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

/// The 30 sessions' transcripts under dir: what each is.
fn make(dir: &Path) -> Vec<Plan> {
    let (big, small) = (parent(5_000), parent(40));
    let child = run(150, false);
    let sub = run(150, true);
    (0..SESSIONS)
        .map(|i| {
            let heavy = i < HEAVY;
            let path = dir.join(format!("proj/s{i:02}.jsonl"));
            write(&path, if heavy { &big } else { &small });
            let (n_spawn, n_sub, every) = if heavy { (30, 40, 150) } else { (2, 0, 20) };
            let spawns = (0..n_spawn)
                .map(|k| {
                    let p = dir.join(format!("proj/kids/s{i:02}-k{k:02}.jsonl"));
                    write(&p, &child);
                    (
                        format!("toolu_p{}", k * every),
                        p.to_string_lossy().into_owned(),
                    )
                })
                .collect();
            let subdir = dir.join(format!("proj/s{i:02}/subagents"));
            for k in 0..n_sub {
                write(
                    &subdir.join(format!("agent-a{k:02}.meta.json")),
                    &format!(r#"{{"agentType":"Explore","description":"look {k}"}}"#),
                );
                write(&subdir.join(format!("agent-a{k:02}.jsonl")), &sub);
            }
            Plan {
                key: format!("default/s{i:02}"),
                path: path.to_string_lossy().into_owned(),
                spawns,
                changes: heavy && i < 2,
            }
        })
        .collect()
}

fn model(plans: &[Plan]) -> Model {
    let reg = rush_core::adapter::Registry::new();
    reg.register(rush_claude::Claude::new());
    let mut m = rush_ui::new_model("0.1.0");
    m.reg = reg;
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = 160;
    m.h = 48;
    let agents: Vec<Agent> = plans
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut a = Agent {
                key: p.key.clone(),
                kind: "claude".into(),
                display_name: format!("session {i}"),
                repo: "/src/project".into(),
                ..Agent::default()
            };
            a.job.id = format!("s{i:02}");
            a.job.session_id = format!("s{i:02}");
            a.job.state = State::Done;
            a.job.cwd = "/src/project".into();
            a.job.transcript_path = p.path.clone();
            a.job.updated_at = NOW - i as i64 * 60_000;
            a
        })
        .collect();
    m.set_fleet(Fleet {
        agents,
        roots: vec!["/src/project".into(); plans.len()],
        previews: HashMap::new(),
        priced: true,
        accounts: Vec::new(),
    });
    m
}

/// Opens p's session as the list would, its runs read as its dock reads
/// them (Changes open for those that open it), a frame drawn.
fn visit(m: &mut Model, p: &Plan) {
    let i = m.agents.iter().position(|a| a.key == p.key).unwrap();
    m.open(i);
    m.full = true;
    // Its runs read once first: that read takes the transcript's path,
    // starting its runs afresh.
    subs::refresh(m, &p.key);
    for (step, path) in &p.spawns {
        let r = &mut conn(m, &p.key).runs;
        r.spawns
            .entry(step.clone())
            .or_insert_with(|| rush_ui_base::spawns::SpawnRun {
                step: step.clone(),
                kind: "claude".into(),
                name: "worker".into(),
                path: path.clone(),
                born: NOW - 3_600_000,
                ..Default::default()
            });
    }
    conn(m, &p.key).runs.full_want = p.changes;
    for _ in 0..4 {
        subs::kick(m, &p.key);
        subs::refresh(m, &p.key);
    }
    std::hint::black_box(m.frame());
    // Changes closed again, as the view moves back to the conversation.
    conn(m, &p.key).runs.full_want = false;
}

/// Live bytes of what f makes and keeps.
fn held<T>(f: impl FnOnce() -> T) -> (T, isize) {
    let b = live();
    let v = f();
    (v, live() - b)
}

fn read_whole(path: &str, side: bool) -> rush_convo::Session {
    let d = rush_convo::decode(rush_claude::messages);
    let mut t = if side {
        rush_convo::Tail::subagent(path, d)
    } else {
        rush_convo::Tail::new(path, d)
    };
    let mut s = rush_convo::Session::new();
    t.read(&mut s).unwrap();
    s
}

fn main() {
    rush_testenv::assert_sandboxed();
    let dir = tempfile::tempdir().unwrap();
    let plans = make(dir.path());
    let sub0 = dir
        .path()
        .join("proj/s00/subagents/agent-a00.jsonl")
        .to_string_lossy()
        .into_owned();
    println!("— what each holds, read alone");
    let (s, b) = held(|| read_whole(&plans[0].path, false));
    println!("5,000-turn session read whole     {}", mib(b));
    drop(s);
    let (s, b) = held(|| read_whole(&plans[5].path, false));
    println!("40-turn session read whole        {}", mib(b));
    drop(s);
    let (s, b) = held(|| read_whole(&plans[0].spawns[0].1, false));
    println!("hosted agent (150 reads) whole    {}", mib(b));
    drop(s);
    let (s, b) = held(|| read_whole(&sub0, true));
    println!("subagent (150 reads) whole        {}", mib(b));
    drop(s);

    let base = live();
    let mut m = model(&plans);
    let after_model = live();
    println!("— 30 sessions opened in turn");
    println!(
        "model with 30 agents              {}",
        mib(after_model - base)
    );
    let mut t = NOW;
    for p in &plans {
        t += 60_000;
        m.now = t;
        m.clock = Some(t);
        visit(&mut m, p);
        let kids = m.session.as_ref().map_or(0, |v| {
            v.o.kids
                .values()
                .flatten()
                .filter(|c| c.sess.is_some())
                .count()
        });
        let full = conn(&mut m, &p.key).runs.full.len();
        print!("{}:{full}/{kids} ", &p.key[9..]);
    }
    println!("(session: runs held whole / hosted agents drawn whole)");
    let open_last = live();
    // Back to the list: the last one's view goes.
    m.session = None;
    m.full = false;
    let on_list = live();
    println!(
        "all opened, the last still open   {}",
        mib(open_last - after_model)
    );
    println!(
        "back on the list (before)         {}",
        mib(on_list - after_model)
    );
    let holding = core(&mut m)
        .conns
        .values()
        .filter(|c| c.runs.holds())
        .count();
    println!("sessions holding runs: {holding}");

    // Who held what: the UI's copies of the conversations read whole are
    // taken out first, so the sweeps free what the reader alone held
    // (runs read on for their numbers), and dropping them after what
    // they did.
    let ui_full: Vec<_> = core(&mut m)
        .conns
        .values_mut()
        .map(|c| std::mem::take(&mut c.runs.full))
        .collect();
    let whole: usize = ui_full.iter().map(|f| f.len()).sum();
    // A minute after the last was opened, the last few aren't let go
    // (out of sight under EVICT_AFTER); EVICT_AFTER on, they are.
    m.now = t + 60_000;
    let (n1, f1) = held(|| evict::sweep(&mut m));
    m.now = t + evict::EVICT_AFTER;
    let (n2, f2) = held(|| evict::sweep(&mut m));
    println!("swept a minute after the last: {n1} of 30 let go; EVICT_AFTER on, {n2} more");
    println!("runs read on for their numbers    {} freed", mib(-f1 - f2));
    let ((), freed) = held(|| drop(ui_full));
    println!("{whole} runs read whole              {} freed", mib(-freed));
    println!(
        "back on the list (after)          {}",
        mib(live() - after_model)
    );
    let rest = live();
    let conns = std::mem::take(&mut core(&mut m).conns);
    drop(conns);
    println!(
        "what every session's side still holds {}",
        mib(rest - live())
    );

    // The first heavy one opened again: its hosted agents read again.
    let mut m = model(&plans);
    visit(&mut m, &plans[0]);
    let kids = |m: &Model| {
        m.session.as_ref().map_or(0, |v| {
            v.o.kids
                .values()
                .map(|k| k.iter().filter(|c| c.sess.is_some()).count())
                .sum::<usize>()
        })
    };
    let before = kids(&m);
    m.session = None;
    m.now += evict::EVICT_AFTER;
    evict::sweep(&mut m);
    visit(&mut m, &plans[0]);
    println!(
        "opened again: {} of {before} hosted agents drawn whole",
        kids(&m)
    );
}
