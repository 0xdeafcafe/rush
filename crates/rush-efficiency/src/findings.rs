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

impl Finding {
    /// A finding in so many words, of cost dollars, with no saver or file
    /// to it: as one is shown, for a fixture outside this crate, whose
    /// rank is the crate's own. Rust-only.
    pub fn new(title: &str, detail: &str, cost: f64) -> Finding {
        Finding {
            title: title.into(),
            detail: detail.into(),
            cost,
            ..Finding::default()
        }
    }
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
    images(v, &mut out);
    codex(v, &mut out);
    if v.start_median > 30_000 {
        // At Claude Code's prices, so over its requests alone.
        let req = if v.agents.is_empty() {
            v.total.req
        } else {
            v.agents.get(crate::AGENT).map_or(0, |a| a.b.req)
        };
        let carry = v.start_median as f64 * req as f64 * cache_read_price("") / 1e6;
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
    if v.sub_cheap > 0 && v.sub_cheap_saved > 0.0 {
        out.push(Finding {
            title: format!(
                "{} subagents ran on a cheaper model, saving about {}",
                v.sub_cheap,
                money(v.sub_cheap_saved)
            ),
            detail: format!(
                "they cost {} on Sonnet or Haiku; the same tokens on the default model cost more (Settings › Usage › Cheaper model for routine subagents)",
                money(v.sub_cheap_cost)
            ),
            rank: 9,
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

/// Image reads that were repeats worth a finding.
pub const IMAGE_REPEATS_AT: i64 = 10;

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Images read again, and sessions heavy on them (Rust-only). Their
/// costs are estimates: every request after an image re-reads it, at the
/// cache-read price, until a compaction drops it.
fn images(v: &View, out: &mut Vec<Finding>) {
    let t = &v.total;
    if t.img_rep >= IMAGE_REPEATS_AT {
        let mut detail = format!(
            "the requests after them re-read about {} tokens of them, about {} (an estimate)",
            tokens(t.img_rep_carry),
            money(v.img_rep_carry)
        );
        if v.worst_image_reads > 1 {
            detail += &format!(
                "; most: {}, read {} times",
                base_name(&v.worst_image),
                count(v.worst_image_reads)
            );
        }
        detail += ". Note what an image showed rather than reading it again, or put the ones to compare on one contact sheet";
        out.push(Finding {
            title: format!(
                "{} image reads were of an image the same session had read already",
                count(t.img_rep)
            ),
            detail,
            cost: v.img_rep_carry,
            rank: 1,
            ..Finding::default()
        });
    }
    if v.img_heavy > 0 {
        let h = &v.heaviest;
        let mut detail = format!(
            "{} images in all, about {} tokens; they stay in context, so every later request pays for them again: about {} here (an estimate)",
            count(t.img),
            tokens(t.img_tok),
            money(v.img_carry)
        );
        if h.held > 0 && h.peak > 0 {
            detail += &format!(
                ". The heaviest held {} of its {} peak context in images",
                tokens(h.held),
                tokens(h.peak)
            );
        }
        detail += ". Read them in batches, put them on a contact sheet, downscale them first, or hand the looking to a fresh reviewer that reads each once";
        out.push(Finding {
            title: format!("{} sessions were heavy on images", count(v.img_heavy)),
            detail,
            cost: v.img_carry,
            rank: 1,
            ..Finding::default()
        });
    }
}

/// Codex sessions near their window, and long ones on its frontier model
/// (Rust-only). rush has no prices for Codex, so they are in tokens.
fn codex(v: &View, out: &mut Vec<Finding>) {
    if v.near > 0 {
        let n = &v.nearest;
        out.push(Finding {
            title: format!("{} Codex sessions ran near their context window", count(v.near)),
            detail: format!(
                "the closest sent {} of {} tokens, {} requests running ({}): every request re-reads all of it, and one long output can run out of room. A fresh session with a short hand-off, or /compact, keeps each turn smaller",
                tokens(n.input),
                tokens(n.window),
                count(n.near),
                n.model
            ),
            rank: 1,
            ..Finding::default()
        });
    }
    if v.frontier > 0 {
        let l = &v.longest;
        let cheaper = crate::codex::cheaper(&l.model).unwrap_or_default();
        out.push(Finding {
            title: format!("{} long Codex sessions ran on {}", count(v.frontier), l.model),
            detail: format!(
                "the longest took {} requests and {} tokens of input. Routine work (a described edit, a lookup, running tests) does as well on {}, which costs less; keep {} for design and debugging. rush has no prices for Codex, so this is in tokens",
                count(l.req),
                tokens(l.input),
                cheaper,
                l.model
            ),
            rank: 2,
            ..Finding::default()
        });
    }
}

/// A count with its thousands marked: 12,345. Rust-only: the findings
/// ported from Go keep Go's plain numbers.
pub fn count(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
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
