//! The process table read, as the machine view and fleet do it once a
//! second: a snapshot of every process, memory and CPU for one tree, and
//! for every tree. Go's twin: `go test ./internal/proc -bench . -benchmem`
//! (internal/proc/bench_test.go).

use criterion::{Criterion, criterion_group, criterion_main};
use rush_sys::proc;

fn bench(c: &mut Criterion) {
    c.bench_function("snapshot", |b| b.iter(proc::snapshot));
    let me = std::process::id() as proc::Pid;
    let prev = proc::snapshot();
    c.bench_function("snapshot+fill(self)", |b| {
        b.iter(|| {
            let mut t = proc::snapshot();
            t.fill(Some(&prev), &[me]);
            t
        })
    });
    let roots: Vec<proc::Pid> = proc::snapshot()
        .children
        .get(&1)
        .cloned()
        .unwrap_or_default();
    c.bench_function("snapshot+fill(every)", |b| {
        b.iter(|| {
            let mut t = proc::snapshot();
            t.fill(Some(&prev), &roots);
            t
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
