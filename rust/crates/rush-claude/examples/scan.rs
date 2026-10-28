//! Prints what a transcript cost and its preview, for checking against Go:
//! `cargo run -p rush-claude --example scan -- <transcript.jsonl>`.

fn main() {
    let path = std::env::args().nth(1).expect("a transcript");
    let p = std::path::Path::new(&path);
    let mut t = rush_claude::transcript::Totals::default();
    let t0 = std::time::Instant::now();
    rush_claude::transcript::scan(p, &mut t, &mut Vec::new()).expect("scan");
    let took = t0.elapsed();
    let (cost, u) = t.spend();
    println!(
        "cost={cost:.4} in={} out={} read={} w5m={} w1h={} model={} first={} last={} prs={} compacts={} context={} dir={} progress={:?} halt={:?} took={took:?}",
        u.input,
        u.output,
        u.cache_read,
        u.cache_write_5m,
        u.cache_write_1h,
        t.last_model,
        t.first,
        t.last,
        t.prs.len(),
        t.compacts,
        t.context(),
        t.dir,
        t.progress,
        t.halt.as_ref().map(|h| &h.kind)
    );
    let pv = rush_claude::transcript::read_preview(p, 384 << 10);
    println!(
        "preview tool={} doing={:?} model={} effort={} context={} at={} recent={} first={:?} text={:?}",
        pv.tool,
        pv.doing,
        pv.model,
        pv.effort,
        pv.context,
        pv.at,
        pv.recent.len(),
        pv.first.text.chars().take(40).collect::<String>(),
        pv.text.chars().take(60).collect::<String>()
    );
}
