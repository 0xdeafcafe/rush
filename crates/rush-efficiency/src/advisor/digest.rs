//! What a pass is given, in words compact enough for a cheap model to read
//! every pass. Port of internal/advisor/digest.go.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::aggregate::{FAT_CONTEXT, REREAD_AT, View};
use crate::catalog::CATALOG;
use crate::detect::{Found, Status};
use crate::findings::{self, bytes, money, tokens};
use crate::scan::{BIG_RESULT, TOOL_NAMES};
use crate::top::SessionCost;
use crate::when::{local, since};

use super::Finding;

/// What a pass is given: the figures, worked out by rush, so the model
/// reads a few kilobytes rather than the transcripts themselves.
#[derive(Default)]
pub struct Input {
    pub view: View,
    pub found: HashMap<String, Found>,
    /// What the Efficiency place already says.
    pub findings: Vec<findings::Finding>,
    pub top: Vec<SessionCost>,
    /// A brief of each of top, by index; "" where there's none.
    pub briefs: Option<Vec<String>>,
    /// The advisor's own findings so far.
    pub known: Vec<String>,
    /// The IDs Opus decided on or you put away: never checked again.
    pub settled: Vec<String>,
    /// Counts each review against the day's cap before it runs; an error
    /// stops the reviews.
    pub reserve: Option<Box<dyn FnMut() -> Result<(), String> + Send>>,
    /// Earlier candidates Opus hasn't checked yet: checked before new ones
    /// worth less.
    pub pending: Vec<Finding>,
}

/// The input in words.
pub fn digest(input: &Input) -> String {
    let v = &input.view;
    let t = &v.total;
    let mut b = String::new();
    macro_rules! p {
        ($($a:tt)*) => {{
            let _ = write!(b, $($a)*);
            b.push('\n');
        }};
    }
    p!(
        "## Figures, {} to {}",
        local(v.q.from, "2 Jan"),
        local(v.q.to, "2 Jan 15:04")
    );
    if t.req == 0 {
        p!("No requests.");
        return b;
    }
    let cr = match t.context() {
        0 => 0.0,
        ctx => t.cr as f64 / ctx as f64 * 100.0,
    };
    p!(
        "Spend {} over {} requests, {} sessions and {} subagent runs (list prices).",
        money(t.cost()),
        t.req,
        v.sessions,
        v.subagents
    );
    p!(
        "Cost by kind: input {}, output {}, cache reads {}, cache writes {}. Cache hits {:.0}% of input.",
        money(t.c_in),
        money(t.c_out),
        money(t.c_cr),
        money(t.c_cw),
        cr
    );
    p!(
        "Per request: {} tokens sent, {} out ({} of it thinking).",
        tokens(t.context() / t.req),
        tokens(t.out / t.req),
        tokens(t.think / t.req)
    );
    p!(
        "Context: sessions start at {} (median; p90 {}) and peak at {} (median). {} passed {}; carrying that cost about {}. {} compactions, {} of them automatic.",
        tokens(v.start_median),
        tokens(v.start_p90),
        tokens(v.peak_median),
        v.fat,
        tokens(FAT_CONTEXT),
        money(v.fat_carry),
        v.compacts,
        v.autos
    );
    let (all, share) = v.tool_bytes();
    let tools: Vec<String> = TOOL_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| t.calls[*i] != 0)
        .map(|(i, name)| {
            format!(
                "{name} {} calls, {} ({:.0}%), carried ≈{}",
                t.calls[i],
                bytes(t.bytes[i]),
                share[i] * 100.0,
                money(v.carry[i])
            )
        })
        .collect();
    p!("Tool output, {} in all: {}.", bytes(all), tools.join("; "));
    if t.look_calls > 0 {
        p!(
            "Shell commands that searched or read code: {}, {}, carried ≈{}.",
            t.look_calls,
            bytes(t.look),
            money(v.look_carry)
        );
    }
    p!("Tool results over {}: {}.", bytes(BIG_RESULT), v.big);
    if v.rereads > 0 {
        p!(
            "{} sessions read one file {}+ times; worst {} ×{}.",
            v.rereads,
            REREAD_AT,
            v.worst_read,
            v.worst_reads
        );
    }
    if v.sub_cost > 0.0 {
        p!(
            "Subagents cost {}, {} of it on Opus or Fable.",
            money(v.sub_cost),
            money(v.sub_big_cost)
        );
    }

    let status = |id: &str| input.found.get(id).map_or(Status::Off, |f| f.status);
    let mut on = Vec::new();
    let mut half = Vec::new();
    for s in CATALOG.iter() {
        match status(s.id) {
            Status::On => on.push(s.id),
            Status::Partial => half.push(s.id),
            Status::Off => {}
        }
    }
    p!("\n## Savers (fix ids)");
    p!("On: {}.", list(&on));
    if !half.is_empty() {
        p!("Half set up: {}.", list(&half));
    }
    for s in CATALOG.iter().filter(|s| status(s.id) == Status::Off) {
        p!("- {}: {}", s.id, s.about);
    }

    if !input.findings.is_empty() || !input.known.is_empty() {
        p!("\n## Already said (don't repeat these)");
        for f in &input.findings {
            p!("- {}", f.title);
        }
        for k in &input.known {
            p!("- {k}");
        }
    }

    p!("\n## Costliest sessions");
    for (i, s) in input.top.iter().enumerate() {
        let kind = if s.sub { " subagent" } else { "" };
        p!(
            "{}. {}, {} req, {}, {}{} in {}, start {}, peak {}, {} compactions, {} big results",
            i + 1,
            money(s.b.cost()),
            s.b.req,
            span(since(s.last, s.first)),
            s.model,
            kind,
            s.project,
            tokens(s.start_ctx),
            tokens(s.peak_ctx),
            s.compacts,
            s.big
        );
        if s.worst_reads >= REREAD_AT {
            p!("   read {} ×{}", s.worst_read, s.worst_reads);
        }
        if let Some(br) = input.briefs.as_ref().and_then(|b| b.get(i))
            && !br.is_empty()
        {
            p!("   brief: {br}");
        }
        p!("   transcript: {}", s.path);
    }
    b
}

fn list(s: &[&str]) -> String {
    if s.is_empty() {
        "none".into()
    } else {
        s.join(", ")
    }
}

/// A session's length, short: 98h15m, 40m.
fn span(secs: i64) -> String {
    let m = (secs + 30).div_euclid(60);
    if m < 60 {
        return format!("{m}m");
    }
    format!("{}h{:02}m", m / 60, m % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::Query;
    use crate::scan::{Bucket, TOOL_BASH};
    use crate::when::{DAY, add_secs};
    use rush_state::Time;

    #[test]
    fn digests() {
        let now = Time::now();
        let mut input = Input {
            view: View {
                q: Query {
                    from: add_secs(now, -7 * DAY),
                    to: now,
                    ..Query::default()
                },
                ..View::default()
            },
            ..Input::default()
        };
        assert!(digest(&input).contains("No requests"));
        input.view.total = Bucket {
            req: 100,
            input: 1000,
            cr: 9000,
            out: 500,
            c_in: 1.0,
            c_cr: 2.0,
            ..Bucket::default()
        };
        input.view.total.calls[TOOL_BASH] = 40;
        input.view.total.bytes[TOOL_BASH] = 2 << 20;
        input.top = vec![SessionCost {
            path: "/p/s.jsonl".into(),
            project: "/src/x".into(),
            b: Bucket {
                req: 50,
                c_in: 2.0,
                ..Bucket::default()
            },
            worst_read: "a.go".into(),
            worst_reads: 9,
            ..SessionCost::default()
        }];
        input.known = vec!["Trim CLAUDE.md".into()];
        let d = digest(&input);
        for want in [
            "100 requests",
            "Cache hits 90%",
            "Bash 40 calls",
            "/p/s.jsonl",
            "a.go ×9",
            "Trim CLAUDE.md",
            "Savers",
        ] {
            assert!(d.contains(want), "digest lacks {want:?}:\n{d}");
        }
        assert!(d.len() <= 8 << 10, "digest is {} bytes", d.len());
        assert_eq!(span(40 * 60), "40m");
        assert_eq!(span(98 * 3600 + 15 * 60), "98h15m");
    }
}
