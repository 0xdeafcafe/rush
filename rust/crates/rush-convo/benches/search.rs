//! An open conversation searched as the command bar searches it on each
//! key (Session::search): the time for each query typed letter by letter,
//! and a digest of what's found, to check a faster search finds the same.
//! RUSH_BENCH_TRANSCRIPT names the transcript (the largest under
//! ~/.claude/projects otherwise).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::Instant;

use rush_convo::{Session, Tail};

const QUERIES: [&str; 8] = [
    "rust port",
    "sharing trace",
    "langwatch",
    "xyzzyq plover",
    "is:failed cargo",
    "file:src rename",
    "naïve über",
    "#12",
];

/// Instructions this process has retired (macOS), else 0.
fn instructions() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        // SAFETY: ri is a rusage_info_v4 to write into.
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

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let Some(path) = std::env::var_os("RUSH_BENCH_TRANSCRIPT").map(PathBuf::from) else {
        println!("set RUSH_BENCH_TRANSCRIPT to a transcript: nothing to measure");
        return;
    };
    let mut s = Session::new();
    Tail::new(&path, rush_convo::decode(rush_claude::messages))
        .read(&mut s)
        .unwrap();
    println!("{} turns, {} steps", s.turns.len(), s.steps().len());
    let cap = |n: usize| n.min(64 << 10);
    let out: usize = s.steps().iter().map(|st| st.output.len()).sum();
    let outc: usize = s.steps().iter().map(|st| cap(st.output.len())).sum();
    let texts: usize = s
        .turns
        .iter()
        .flat_map(|t| &t.items)
        .map(|it| match it {
            rush_convo::Item::Text { text, .. } => cap(text.len()),
            _ => 0,
        })
        .sum();
    println!("outputs {out} bytes ({outc} within 64K each), texts {texts}");
    let mut total = 0.0;
    // RUSH_BENCH_ROUNDS=n searches it all n times (to profile).
    let rounds: usize = std::env::var("RUSH_BENCH_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(1);
    for q in QUERIES.iter().cycle().take(QUERIES.len() * rounds) {
        let mut ms = Vec::new();
        let mut mi = Vec::new();
        let mut h = DefaultHasher::new();
        let mut n = 0;
        let chars: Vec<char> = q.chars().collect();
        for i in 1..=chars.len() {
            let typed: String = chars[..i].iter().collect();
            let mut best = f64::MAX;
            let mut ins = u64::MAX;
            let mut hits = Vec::new();
            for _ in 0..3 {
                let i0 = instructions();
                let t = Instant::now();
                hits = s.search(&typed);
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
                ins = ins.min(instructions() - i0);
            }
            ms.push(best);
            mi.push(ins as f64 / 1e6);
            n += hits.len();
            format!("{hits:?}").hash(&mut h);
        }
        let sum: f64 = ms.iter().sum();
        total += sum;
        let max = ms.iter().cloned().fold(0.0, f64::max);
        let isum: f64 = mi.iter().sum();
        let imax = mi.iter().cloned().fold(0.0, f64::max);
        println!(
            "{q:<18} per key {:>7.2} ms avg, {:>7.2} max, {:>7.1} M instr avg, {:>7.1} max  hits {n:>6}  digest {:016x}",
            sum / ms.len() as f64,
            max,
            isum / mi.len() as f64,
            imax,
            h.finish()
        );
    }
    println!("total {total:.1} ms");
}
