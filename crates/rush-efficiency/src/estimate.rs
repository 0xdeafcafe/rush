//! What a saver might save, from the view's own figures, and whether one
//! that's set up is doing anything. Port of internal/efficiency/estimate.go.

use std::fmt;

use rush_state::Time;

use crate::aggregate::{Metric, View};
use crate::catalog::Saver;
use crate::compare::{FEW_SESSIONS, Side};
use crate::detect::{Found, Status};
use crate::price::cache_read_price;
use crate::scan::{TOOL_BASH, TOOL_MCP, TOOL_READ, TOOL_SEARCH};

/// The part of the spend a saver acts on, worked out from the view's own
/// figures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// Shell output, carried in context.
    Shell,
    /// Searching and reading code: Read, Grep/Glob and shell searches,
    /// carried.
    Look,
    /// Every tool's output, carried.
    ToolOut,
    /// MCP tools' output, carried.
    Mcp,
    /// What Claude writes, thinking aside.
    Text,
    /// Thinking.
    Think,
    /// Subagents run on Opus or Fable.
    SubBig,
    /// What sessions start with, read again by every request.
    Start,
    /// Context past FAT_CONTEXT, read again by every request.
    Fat,
    /// All of it: savers only measured on the whole bill.
    All,
}

impl fmt::Display for Base {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Base::Shell => "shell output kept in context",
            Base::Look => "searching and reading code (Read, Grep, and grep/find/cat in the shell)",
            Base::ToolOut => "tool output kept in context",
            Base::Mcp => "MCP output kept in context",
            Base::Text => "what Claude writes, thinking aside",
            Base::Think => "thinking",
            Base::SubBig => "subagents on Opus or Fable",
            Base::Start => "the context sessions start with",
            Base::Fat => "context past 400k tokens",
            Base::All => "everything",
        })
    }
}

/// What a saver can be expected to take off its base: a share, low to
/// high, from what's been published about it (basis says what). A low
/// below zero is a saver that has been measured costing more.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub of: Base,
    pub low: f64,
    pub high: f64,
    pub basis: &'static str,
}

/// What a saver might have saved over a view had it been on throughout:
/// its cut of what the view spent on its base.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    /// Dollars the view spent on the base.
    pub base: f64,
    /// Dollars it might have saved.
    pub low: f64,
    pub high: f64,
}

impl Estimate {
    /// The middle of the range, for ordering.
    pub fn mid(&self) -> f64 {
        (self.low + self.high) / 2.0
    }
}

/// Sessions in the view that used a saver against those that didn't since
/// it was first seen: what it did here, as far as figures that aren't a
/// controlled test can say.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measured {
    pub with: Side,
    pub without: Side,
    pub since: Time,
}

/// Whether a saver that's set up is doing anything here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    /// A setting: acts on every session, nothing in a transcript shows it.
    #[default]
    Unseen,
    /// Found, but not finished: it does nothing.
    Half,
    /// Set up, but never seen in these sessions.
    Silent,
    /// Seen at work.
    Firing,
}

/// A saver's verdict over the view and, when compared, how the figure it
/// should move differed in sessions with it against those without, in
/// percent: negative is better, but for cache hits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Working {
    pub verdict: Verdict,
    pub metric: Metric,
    pub change: f64,
    pub compared: bool,
    /// Under FEW_SESSIONS on a side.
    pub few: bool,
}

impl Working {
    fn of(verdict: Verdict) -> Working {
        Working {
            verdict,
            metric: Metric::Ctx,
            change: 0.0,
            compared: false,
            few: false,
        }
    }
}

impl View {
    /// What the view spent on a base.
    pub fn spent(&self, b: Base) -> f64 {
        let t = &self.total;
        match b {
            Base::Shell => self.carry[TOOL_BASH],
            Base::Look => self.carry[TOOL_READ] + self.carry[TOOL_SEARCH] + self.look_carry,
            Base::ToolOut => self.carry.iter().sum(),
            Base::Mcp => self.carry[TOOL_MCP],
            Base::Text | Base::Think => {
                if t.out == 0 {
                    return 0.0;
                }
                let think = t.think.min(t.out) as f64 / t.out as f64;
                if b == Base::Think {
                    t.c_out * think
                } else {
                    t.c_out * (1.0 - think)
                }
            }
            Base::SubBig => self.sub_big_cost,
            Base::Start => self.start_median as f64 * t.req as f64 * cache_read_price("") / 1e6,
            Base::Fat => self.fat_carry,
            Base::All => t.cost(),
        }
    }

    /// The saver's estimate over the view, or None when it has no cut to
    /// go by or the view spent nothing on its base.
    pub fn estimate(&self, s: &Saver) -> Option<Estimate> {
        let c = s.cut?;
        let base = self.spent(c.of);
        if base <= 0.0 {
            return None;
        }
        Some(Estimate {
            base,
            low: base * c.low,
            high: base * c.high,
        })
    }

    /// Orders savers by what they might save over the view, most first;
    /// those without an estimate, or that skip says to leave out (being on
    /// already, their base has them in), keep their order after them.
    pub fn by_estimate(&self, list: &mut [&Saver], skip: Option<&dyn Fn(&Saver) -> bool>) {
        let mid = |s: &Saver| match self.estimate(s) {
            Some(e) if !skip.is_some_and(|f| f(s)) => e.mid(),
            _ => -1e18,
        };
        list.sort_by(|a, b| mid(b).total_cmp(&mid(a)));
    }

    /// The saver's split, or None when it was never seen.
    pub fn measured(&self, id: &str) -> Option<Measured> {
        let sp = self.split.get(id)?;
        Some(Measured {
            with: sp[0].side(),
            without: sp[1].side(),
            since: self.first_use.get(id).copied().unwrap_or_default(),
        })
    }

    /// Whether a saver found set up is doing anything in the view.
    pub fn working(&self, s: &Saver, f: &Found) -> Working {
        if f.status == Status::Partial {
            return Working::of(Verdict::Half);
        }
        if s.uses.is_empty() {
            return Working::of(Verdict::Unseen);
        }
        if !self.uses.contains_key(s.id) {
            return Working::of(Verdict::Silent);
        }
        let mut w = Working::of(Verdict::Firing);
        w.metric = s.moves.first().copied().unwrap_or(Metric::Cost);
        let Some(mz) = self.measured(s.id) else {
            return w;
        };
        if mz.with.sessions == 0 || mz.without.sessions == 0 {
            return w;
        }
        let (a, b) = (
            side_figure(&mz.with, w.metric),
            side_figure(&mz.without, w.metric),
        );
        if b <= 0.0 {
            return w;
        }
        w.change = (a - b) / b * 100.0;
        w.compared = true;
        w.few = mz.with.sessions < FEW_SESSIONS || mz.without.sessions < FEW_SESSIONS;
        w
    }
}

/// A side's figure for a metric, per request or per call.
fn side_figure(s: &Side, m: Metric) -> f64 {
    match m {
        Metric::ToolKb => s.tool_per_call,
        Metric::Ctx => s.ctx_per_req,
        Metric::Out => s.out_per_req,
        Metric::Cache => s.cache_hit,
        Metric::Start => s.start_median as f64,
        _ => s.cost_per_req,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{Query, SaverUse};
    use crate::compare::SideAcc;
    use crate::when::{DAY, add_secs};

    #[test]
    fn estimates() {
        let now = Time::now();
        let mut v = View {
            q: Query {
                from: add_secs(now, -30 * DAY),
                to: now,
                ..Query::default()
            },
            ..View::default()
        };
        v.carry[TOOL_BASH] = 100.0;
        v.carry[TOOL_READ] = 40.0;
        v.look_carry = 60.0;
        v.total.c_out = 50.0;
        v.total.out = 1000;
        v.total.think = 400;
        let mk = |id, c: Option<Cut>| Saver {
            id,
            cut: c,
            ..Saver::default()
        };
        let cut = |of, low, high| {
            Some(Cut {
                of,
                low,
                high,
                basis: "",
            })
        };
        let shell = mk("a", cut(Base::Shell, 0.1, 0.5));
        let look = mk("b", cut(Base::Look, -0.1, 0.3));
        let text = mk("c", cut(Base::Text, 0.5, 0.5));
        let none = mk("d", None);
        let e = v.estimate(&shell).unwrap();
        assert_eq!((e.base, e.low, e.high), (100.0, 10.0, 50.0));
        let e = v.estimate(&look).unwrap();
        assert_eq!(
            (e.base, e.low, e.high),
            (100.0, -10.0, 30.0),
            "searching and reading is Read and the shell's searches"
        );
        assert_eq!(
            v.estimate(&text).unwrap().base,
            30.0,
            "what Claude writes leaves out thinking"
        );
        assert!(
            v.estimate(&none).is_none(),
            "a saver with no cut has no estimate"
        );
        let mut list = vec![&none, &text, &look, &shell];
        v.by_estimate(&mut list, None);
        assert_eq!(list[0].id, "a");
        assert_eq!(list[3].id, "d");
    }

    #[test]
    fn working() {
        let sv = Saver {
            id: "x",
            uses: vec!["bash:x"],
            moves: vec![Metric::ToolKb],
            ..Saver::default()
        };
        let on = Found {
            status: Status::On,
            ..Found::default()
        };
        let mut v = View::default();
        let half = Found {
            status: Status::Partial,
            ..Found::default()
        };
        assert_eq!(v.working(&sv, &half).verdict, Verdict::Half);
        let setting = Saver {
            id: "s",
            ..Saver::default()
        };
        assert_eq!(
            v.working(&setting, &on).verdict,
            Verdict::Unseen,
            "a setting can't be seen"
        );
        assert_eq!(
            v.working(&sv, &on).verdict,
            Verdict::Silent,
            "on but never used"
        );
        v.uses.insert(
            "x".into(),
            SaverUse {
                n: 3,
                sessions: 2,
                ..SaverUse::default()
            },
        );
        let w = v.working(&sv, &on);
        assert!(
            w.verdict == Verdict::Firing && !w.compared,
            "used, nothing to compare"
        );
        let mut sp: [SideAcc; 2] = Default::default();
        sp[0].n = 1;
        sp[0].b.calls[TOOL_BASH] = 10;
        sp[0].b.bytes[TOOL_BASH] = 600;
        sp[1].n = 1;
        sp[1].b.calls[TOOL_BASH] = 10;
        sp[1].b.bytes[TOOL_BASH] = 1000;
        v.split.insert("x".into(), sp);
        let w = v.working(&sv, &on);
        assert!(
            w.compared && w.change == -40.0 && w.few && w.metric == Metric::ToolKb,
            "{w:?}"
        );
    }
}
