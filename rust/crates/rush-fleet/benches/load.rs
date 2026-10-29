//! The fleet read on this machine's real agents, as Go's
//! internal/fleet/real_bench_test.go measures it:
//!
//!   RUSH_REAL=1 cargo bench -p rush-fleet --bench load
//!   RUSH_REAL=1 go test -bench Real -benchmem ./internal/fleet
//!
//! Reads ~/.claude and rush's state read only. Without RUSH_REAL it does
//! nothing.

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use rush_core::adapter::Registry;
use rush_core::time::now;
use rush_fleet::Loader;

fn registry() -> Registry {
    let reg = Registry::new();
    reg.register(rush_claude::Claude::at(rush_claude::Account::home()));
    reg
}

fn real(c: &mut Criterion) {
    if std::env::var_os("RUSH_REAL").is_none() {
        eprintln!("set RUSH_REAL=1 to read this machine's agents");
        return;
    }
    raise_open_files();
    let mut g = c.benchmark_group("fleet");
    g.sample_size(20).measurement_time(Duration::from_secs(10));

    // BenchmarkLoadReal: a full reading, processes sampled.
    let mut l = Loader::from_state(registry());
    l.load(now());
    g.bench_function("load_real", |b| b.iter(|| l.load(now())));

    // BenchmarkScanReal: pricing every row's transcripts, none grown.
    let mut l = Loader::from_state(registry());
    let snap = l.load_unsampled(now());
    l.scan(&snap.agents);
    g.bench_function("scan_real", |b| b.iter(|| l.scan(&snap.agents)));

    // BenchmarkLoadRealOneChange: a load after one watched file changed,
    // as a refresh while agents write, against a sweep's, which trusts no
    // watch.
    let touch = rush_state::dir().join(".bench-touch");
    for sweep in [false, true] {
        let mut l = Loader::from_state(registry());
        l.watch(3_600_000);
        for _ in 0..3 {
            // watching, then told what's newly watched, then trusted
            l.load(now());
        }
        let name = if sweep {
            "load_real_sweep"
        } else {
            "load_real_one_change"
        };
        g.bench_function(name, |b| {
            b.iter(|| {
                let _ = std::fs::remove_file(&touch); // a new entry: its folder is told
                let _ = std::fs::write(&touch, now().to_string());
                if sweep {
                    l.force_sweep();
                }
                l.settle();
                l.load(now())
            })
        });
    }
    let _ = std::fs::remove_file(&touch);
    g.finish();
}

/// Raises the soft limit on open files as the binary does (and Go's
/// runtime): each watched folder is a descriptor, and a session's folder
/// not watched is read again every load.
fn raise_open_files() {
    // SAFETY: zeroed is a valid rlimit; getrlimit fills it.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out-pointer.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } != 0 {
        return;
    }
    r.rlim_cur = r.rlim_max.min(10240);
    // SAFETY: a valid rlimit.
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &r) };
}

criterion_group!(benches, real);
criterion_main!(benches);
