//! What in the figures is worth doing something about. Port of
//! internal/efficiency/findings.go.

use std::collections::HashMap;
use std::path::Path;

use crate::aggregate::{FAT_CONTEXT, REREAD_AT, View};
use crate::catalog::CATALOG;
use crate::detect::{Found, Status};
use crate::price::cache_read_price;
use crate::scan::{BIG_RESULT, TOOL_BASH, TOOL_READ};
use crate::when::{HOUR, since};

/// Something in the figures worth doing something about, with the saver
/// that addresses it when there is one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Finding {
    pub title: String,
    pub detail: String,
    /// Roughly the dollars involved in the view, when it can be worked
    /// out; findings with one come first, biggest first.
    pub cost: f64,
    /// The saver that addresses it: enter sets it up.
    pub fix: String,
    /// The file to change for it, when there's no saver: enter opens it.
    pub open: String,
    /// The advisor finding's ID, on the advisor's.
    pub advice: String,
    /// Lower first among findings without a cost; below 0 goes first of
    /// all.
    pub(crate) rank: i32,
}

/// The view's findings, most at stake first.
pub fn findings(v: &View, found: &HashMap<String, Found>) -> Vec<Finding> {
    let mut out = Vec::new();
    let status = |id: &str| found.get(id).map_or(Status::Off, |f| f.status);
    let cost = v.total.cost();
    if v.total.req == 0 {
        return out;
    }
    for s in CATALOG.iter() {
        if let Some(f) = found.get(s.id)
            && f.status == Status::Partial
        {
            out.push(Finding {
                title: format!("{} is half set up: {}", s.name, f.wants),
                detail: "it does nothing until it's finished".into(),
                fix: s.id.into(),
                rank: -1,
                ..Finding::default()
            });
        }
    }

    let (all, share) = v.tool_bytes();
    if share[TOOL_BASH] > 0.3 && all > 1 << 20 {
        let mut f = Finding {
            title: format!(
                "Shell output is {:.0}% of what tools sent back ({})",
                share[TOOL_BASH] * 100.0,
                bytes(v.total.bytes[TOOL_BASH])
            ),
            detail: format!(
                "every token of it is read again by the requests after it: carrying it cost about {}",
                money(v.carry[TOOL_BASH])
            ),
            cost: v.carry[TOOL_BASH],
            ..Finding::default()
        };
        if status("bash-output") == Status::Off {
            f.fix = "bash-output".into();
        } else if status("rtk") != Status::On {
            f.fix = "rtk".into();
        }
        out.push(f);
    }
    if share[TOOL_READ] > 0.25 && all > 1 << 20 {
        let mut f = Finding {
            title: format!(
                "File reads are {:.0}% of what tools sent back ({})",
                share[TOOL_READ] * 100.0,
                bytes(v.total.bytes[TOOL_READ])
            ),
            detail: format!("carrying them cost about {}", money(v.carry[TOOL_READ])),
            cost: v.carry[TOOL_READ],
            ..Finding::default()
        };
        if v.rereads > 0 {
            let base = Path::new(&v.worst_read)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            f.detail += &format!(
                "; {} sessions read one file {}+ times (most: {}, {} times)",
                v.rereads, REREAD_AT, base, v.worst_reads
            );
        }
        out.push(f);
    }
    if v.start_median > 30_000 {
        let carry = v.start_median as f64 * v.total.req as f64 * cache_read_price("") / 1e6;
        out.push(Finding {
            title: format!(
                "Sessions start with {} tokens of context (p90 {})",
                tokens(v.start_median),
                tokens(v.start_p90)
            ),
            detail: format!(
                "the system prompt, CLAUDE.md, skills, plugins and MCP tools, read again by every request: about {} here. /context in a session shows what's in it",
                money(carry)
            ),
            cost: carry,
            ..Finding::default()
        });
    }
    if v.fat > 0 {
        let mut f = Finding {
            title: format!(
                "{} sessions went past {} tokens of context",
                v.fat,
                tokens(FAT_CONTEXT)
            ),
            detail: "every request re-reads all of it; compacting sooner keeps each turn cheaper"
                .into(),
            rank: 1,
            ..Finding::default()
        };
        if status("autocompact") == Status::Off {
            f.fix = "autocompact".into();
        }
        out.push(f);
    }
    if v.sub_cost > 5.0
        && v.sub_big_cost > 0.3 * v.sub_cost
        && status("subagent-model") == Status::Off
    {
        out.push(Finding {
            title: format!(
                "Subagents cost {}, {} of it on Opus or Fable",
                money(v.sub_cost),
                money(v.sub_big_cost)
            ),
            detail:
                "most subagent work is searching and reading; on Sonnet it would cost about half"
                    .into(),
            cost: v.sub_big_cost / 2.0,
            fix: "subagent-model".into(),
            ..Finding::default()
        });
    }
    if v.big > 10 && status("bash-output") == Status::Off {
        out.push(Finding {
            title: format!(
                "{} tool results were over {} tokens each",
                v.big,
                tokens(BIG_RESULT / 4)
            ),
            detail: "a lower cap keeps the start and end and saves the rest to a file".into(),
            fix: "bash-output".into(),
            rank: 2,
            ..Finding::default()
        });
    }
    let hit = v.cache_hit();
    if hit < 90.0 && v.total.req > 200 {
        out.push(Finding {
            title: format!("Only {hit:.0}% of input came from the cache"),
            detail: "sessions left idle past the cache's life, or switching model, write the whole context again".into(),
            rank: 2,
            ..Finding::default()
        });
    }
    if cost > 0.0 {
        let o = v.total.c_out / cost;
        if o < 0.12 {
            out.push(Finding {
                title: format!("What Claude writes is only {:.0}% of the spend", o * 100.0),
                detail: "savers that shorten replies can save at most that; context is where the money goes".into(),
                rank: 3,
                ..Finding::default()
            });
        }
    }
    if v.total.out > 0
        && v.total.think as f64 / v.total.out as f64 > 0.7
        && v.total.c_out > 0.15 * cost
    {
        out.push(Finding {
            title: format!(
                "Thinking is {:.0}% of output",
                100.0 * v.total.think as f64 / v.total.out as f64
            ),
            detail: "a lower effort (/effort) thinks less on routine work".into(),
            rank: 3,
            ..Finding::default()
        });
    }
    for s in CATALOG.iter() {
        if status(s.id) == Status::On
            && !s.uses.is_empty()
            && !v.uses.contains_key(s.id)
            && since(v.q.to, v.q.from) >= 72 * HOUR
        {
            out.push(Finding {
                title: format!("{} is on but wasn't seen in these sessions", s.name),
                detail: "it may not be loading, or not apply to this work".into(),
                rank: 3,
                ..Finding::default()
            });
        }
    }
    if status("history") == Status::Off {
        out.push(Finding {
            title: "Claude Code deletes transcripts after 30 days".into(),
            detail: "rush keeps totals of deleted ones, but not their detail".into(),
            fix: "history".into(),
            rank: 4,
            ..Finding::default()
        });
    }
    out.sort_by(|a, b| {
        use std::cmp::Ordering;
        if a.rank < 0 || b.rank < 0 {
            return a.rank.cmp(&b.rank);
        }
        if (a.cost > 0.0) != (b.cost > 0.0) {
            return if a.cost > 0.0 {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        if a.cost != b.cost {
            return b.cost.total_cmp(&a.cost);
        }
        a.rank.cmp(&b.rank)
    });
    out
}

/// Dollars in words, short.
pub fn money(c: f64) -> String {
    if c >= 10_000.0 {
        format!("${:.1}k", c / 1000.0)
    } else if c >= 100.0 {
        format!("${c:.0}")
    } else if c >= 1.0 {
        format!("${c:.2}")
    } else {
        format!("${c:.3}")
    }
}

/// Tokens in words, short.
pub fn tokens(n: i64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// Bytes in words, short.
pub fn bytes(n: i64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.0} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        for (c, want) in [
            (12_345.0, "$12.3k"),
            (150.0, "$150"),
            (2.5, "$2.50"),
            (0.0123, "$0.012"),
        ] {
            assert_eq!(money(c), want);
        }
        for (n, want) in [
            (1_500_000_000, "1.5B"),
            (2_500_000, "2.5M"),
            (45_000, "45k"),
            (1_500, "1.5k"),
            (12, "12"),
        ] {
            assert_eq!(tokens(n), want);
        }
        for (n, want) in [
            (3 << 30, "3.0 GB"),
            (5 << 20, "5 MB"),
            (2048, "2 KB"),
            (12, "12 B"),
        ] {
            assert_eq!(bytes(n), want);
        }
    }
}
