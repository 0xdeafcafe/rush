//! Prints the Claude rows as Go's `rush --dump` does (no CPU or memory
//! yet), for diffing: `cargo run -p rush-claude --example dump`.

use rush_claude::fleet::Predicates;

fn main() {
    let t0 = std::time::Instant::now();
    let rows = rush_claude::load(rush_core::time::now());
    let took = t0.elapsed();
    let now = rush_core::time::now();
    for a in &rows {
        let name: String = a.display_name.chars().take(40).collect();
        println!(
            "{:<8} {:<8} {:<40} {:>8.2} {:>6}m {:>6}s",
            a.job.id,
            a.job.state.as_str(),
            name,
            a.spend.cost,
            (a.elapsed(now) + 30_000) / 60_000,
            (a.age(now) + 500) / 1000,
        );
    }
    eprintln!("{} rows in {took:?}", rows.len());
}
