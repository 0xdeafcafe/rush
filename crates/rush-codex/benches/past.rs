//! Codex's past threads listed, as a hosted run's transcript lookup
//! (rush hosted.rs hosted_transcript) and the list ask for them every
//! couple of seconds: 3,000 rollouts in 100 day folders, made up in a temp
//! dir.
//!
//! - cold: a Codex new each time, every rollout's head read;
//! - again: the same Codex asked again, nothing changed on disk;
//! - changed: the same, told first that something under its home changed
//!   (Discoverer::changed), so it lists the folders again.
//!
//! Neither of the last two reads a rollout (Codex::past_reads).
//!
//! `cargo bench -p rush-codex --bench past`

use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use rush_codex::Codex;
use rush_core::adapter::{Adapter, Kind, Profile};

const DAYS: usize = 100;
const EACH: usize = 30;

fn rollout(path: &Path, n: usize) {
    let id = format!("01a0c9a2-0000-7000-8000-{n:012}");
    let at = "2026-09-20T10:00:00.000Z";
    let mut text = format!(
        r#"{{"timestamp":"{at}","type":"session_meta","payload":{{"id":"{id}","timestamp":"{at}","cwd":"/work/{n}","cli_version":"0.1.0","source":"cli"}}}}
{{"timestamp":"{at}","type":"turn_context","payload":{{"model":"gpt-5","cwd":"/work/{n}"}}}}
{{"timestamp":"{at}","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"fix the thing number {n}"}}]}}}}
"#
    );
    // What follows a head: a turn's output, never read for it.
    let filler = "x".repeat(400);
    for _ in 0..12 {
        text += &format!(
            r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"agent_message","message":"{filler}"}}}}
"#
        );
    }
    std::fs::write(path, text).unwrap();
}

fn tree() -> (tempfile::TempDir, Profile) {
    let dir = tempfile::tempdir().unwrap();
    for d in 0..DAYS {
        let day = dir
            .path()
            .join(format!("sessions/2026/{:02}/{:02}", 1 + d / 28, 1 + d % 28));
        std::fs::create_dir_all(&day).unwrap();
        for i in 0..EACH {
            let n = d * EACH + i;
            let name = format!("rollout-2026-09-20T10-00-00-01a0c9a2-0000-7000-8000-{n:012}.jsonl");
            rollout(&day.join(name), n);
        }
    }
    let p = Profile {
        kind: Kind::from(rush_codex::KIND),
        name: ".codex".into(),
        dir: dir.path().to_string_lossy().into_owned(),
    };
    (dir, p)
}

fn bench(c: &mut Criterion) {
    let (dir, p) = tree();
    let home = dir.path().to_string_lossy().into_owned();
    let codex = || Codex::with_env(&home, &home, || true);
    let mut g = c.benchmark_group("past");
    g.sample_size(10);
    g.bench_function("cold", |b| {
        b.iter(|| codex().discoverer().unwrap().past(&p).len())
    });
    let warm = codex();
    let d = warm.discoverer().unwrap();
    assert_eq!(d.past(&p).len(), DAYS * EACH);
    g.sample_size(50);
    let read = warm.past_reads();
    g.bench_function("again", |b| b.iter(|| d.past(&p).len()));
    g.bench_function("changed", |b| {
        b.iter(|| {
            d.changed(&p, None);
            d.past(&p).len()
        })
    });
    g.finish();
    assert_eq!(warm.past_reads(), read, "nothing changed, nothing read");
}

criterion_group!(benches, bench);
criterion_main!(benches);
