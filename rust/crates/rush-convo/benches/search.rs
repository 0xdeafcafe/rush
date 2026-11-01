//! An open conversation searched as the command bar searches it on each
//! key (Session::search): the time for each query typed letter by letter,
//! and a digest of what's found, to check a faster search finds the same.
//! RUSH_BENCH_TRANSCRIPT names the transcript (the largest under
//! ~/.claude/projects otherwise).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::Instant;

use rush_convo::search::SearchMemo;
use rush_convo::{Hit, Session, Tail};

/// What the bar shows of search's hits: the latest first, the closest
/// first when words are typed, cut to limit.
fn shown(s: &Session, q: &str, limit: usize) -> Vec<Hit> {
    let mut hits = s.search(q);
    hits.reverse();
    if !rush_convo::search::words(q).is_empty() {
        hits.sort_by_key(|h| std::cmp::Reverse(h.score));
    }
    hits.truncate(limit);
    hits
}

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
    // The bar's way: each key's top hits, a memo kept from key to key,
    // typed on and backspaced, checked against search's.
    for (limit, made) in [
        (50, false),
        (300, false),
        (4, false),
        (50, true),
        (300, true),
    ] {
        let mut memo = SearchMemo::default();
        if made {
            let i0 = instructions();
            let job = memo.prepare(&s).expect("long enough");
            let i1 = instructions();
            let v = job.make();
            let i2 = instructions();
            memo.take(v);
            println!(
                "vocabulary: copied in {:.1} M instructions, made in {:.1} M",
                (i1 - i0) as f64 / 1e6,
                (i2 - i1) as f64 / 1e6
            );
        }
        let mut tot = 0.0;
        let mut ins_all = Vec::new();
        for q in QUERIES {
            let chars: Vec<char> = q.chars().collect();
            let typed: Vec<String> = (1..=chars.len())
                .chain((1..chars.len()).rev())
                .map(|i| chars[..i].iter().collect())
                .collect();
            let mut mi = Vec::new();
            for t in &typed {
                let i0 = instructions();
                let c = Instant::now();
                let got = s.search_top(t, limit, &mut memo);
                tot += c.elapsed().as_secs_f64() * 1e3;
                mi.push((instructions() - i0) as f64 / 1e6);
                let want = shown(&s, t, limit);
                if got != want {
                    let brief = |v: &[Hit]| {
                        v.iter()
                            .map(|h| format!("{}={}", h.r#ref, h.score))
                            .collect::<Vec<_>>()
                    };
                    let i = got.iter().zip(&want).take_while(|(a, b)| a == b).count();
                    panic!(
                        "{t:?} limit {limit}: differs at {i} of {}/{}\n got {:?}\nwant {:?}",
                        got.len(),
                        want.len(),
                        &brief(&got)[i.min(got.len())..(i + 5).min(got.len())],
                        &brief(&want)[i.min(want.len())..(i + 5).min(want.len())],
                    );
                }
            }
            let avg = mi.iter().sum::<f64>() / mi.len() as f64;
            let max = mi.iter().cloned().fold(0.0, f64::max);
            println!(
                "top {limit:<3} {} {q:<18} per key {avg:>7.1} M instr avg, {max:>7.1} max",
                if made { "v" } else { "-" }
            );
            ins_all.extend(mi);
        }
        println!("top {limit}: {tot:.1} ms, all same as search's");
    }
}
