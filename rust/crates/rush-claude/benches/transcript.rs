//! Scan and ReadPreview on a real transcript: RUSH_BENCH_TRANSCRIPT, else
//! the largest under ~/.claude/projects (read only), else a made-up one.

use std::path::{Path, PathBuf};

use criterion::{Criterion, criterion_group, criterion_main};
use rush_claude::transcript::{Totals, read_preview, scan};

fn largest(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for pd in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(ents) = std::fs::read_dir(pd.path()) else {
            continue;
        };
        for e in ents.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let n = e.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(b, _)| n > *b) {
                best = Some((n, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn made_up() -> PathBuf {
    let p = std::env::temp_dir().join(format!("rush-claude-bench-{}.jsonl", std::process::id()));
    let mut s = String::new();
    for i in 0..20_000 {
        s.push_str(&format!(
            r#"{{"type":"user","timestamp":"2026-09-23T10:00:00Z","message":{{"role":"user","content":"step {i}"}}}}
{{"type":"assistant","cwd":"/r","timestamp":"2026-09-23T10:00:01Z","message":{{"id":"m{i}","model":"claude-opus-5-5","content":[{{"type":"text","text":"working on it, lint 120 -> 80"}},{{"type":"tool_use","name":"Bash","input":{{"command":"cd /r && go test ./..."}}}}],"usage":{{"input_tokens":10,"output_tokens":50,"cache_read_input_tokens":1000}}}}}}
"#
        ));
    }
    std::fs::write(&p, s).unwrap();
    p
}

fn transcript() -> PathBuf {
    if let Some(p) = std::env::var_os("RUSH_BENCH_TRANSCRIPT") {
        return p.into();
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    largest(&home.join(".claude").join("projects")).unwrap_or_else(made_up)
}

fn benches(c: &mut Criterion) {
    let path = transcript();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "transcript: {} ({:.1} MB)",
        path.display(),
        size as f64 / 1e6
    );
    let mut g = c.benchmark_group("transcript");
    g.sample_size(10);
    let mut buf = Vec::new();
    g.bench_function("scan_cold", |b| {
        b.iter(|| {
            let mut t = Totals::default();
            scan(&path, &mut t, &mut buf).unwrap();
            t.cost
        })
    });
    let mut warm = Totals::default();
    scan(&path, &mut warm, &mut buf).unwrap();
    g.bench_function("scan_unchanged", |b| {
        b.iter(|| {
            scan(&path, &mut warm, &mut buf).unwrap();
            warm.offset
        })
    });
    for window in [128u64 << 10, 384 << 10] {
        g.bench_function(format!("preview_{}k", window >> 10), |b| {
            b.iter(|| read_preview(&path, window).context)
        });
    }
    g.finish();
}

criterion_group!(group, benches);
criterion_main!(group);
