//! Before and after a moment, and with a saver against without. Port of
//! internal/efficiency/compare.go.

use rush_state::Time;

use crate::aggregate::{Query, quantile};
use crate::catalog::Saver;
use crate::scan::{Bucket, File, N_TOOLS};
use crate::store::Store;
use crate::when::{DAY, add_secs};

/// One side of a comparison, in figures that don't grow with how much you
/// worked: per request, per call and per session.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Side {
    pub sessions: i64,
    pub req: i64,
    pub ctx_per_req: f64,
    pub out_per_req: f64,
    /// Bytes of tool output per call.
    pub tool_per_call: f64,
    /// Percent.
    pub cache_hit: f64,
    pub cost_per_req: f64,
    /// The median session's.
    pub cost_session: f64,
    pub start_median: i64,
    pub cost: f64,
}

/// The figures before and after a moment, and, for a saver, sessions that
/// used it against those that didn't in the same days: the fairer test,
/// since the work is the same kind.
#[derive(Clone, Debug, Default)]
pub struct Compare {
    pub at: Time,
    pub days: i64,
    pub before: Side,
    pub after: Side,
    pub saver: Option<&'static Saver>,
    pub with: Side,
    pub without: Side,
}

/// Under how many sessions a side is too small to say much.
pub const FEW_SESSIONS: i64 = 15;

#[derive(Clone, Debug, Default)]
pub(crate) struct SideAcc {
    pub(crate) b: Bucket,
    pub(crate) costs: Vec<f64>,
    pub(crate) start: Vec<i64>,
    pub(crate) n: i64,
}

impl SideAcc {
    pub(crate) fn add(&mut self, f: &File, t: &Bucket) {
        self.b.add(t);
        self.n += 1;
        self.costs.push(t.cost());
        self.start.push(f.start());
    }

    pub(crate) fn side(&self) -> Side {
        let b = &self.b;
        let mut s = Side {
            sessions: self.n,
            req: b.req,
            cost: b.cost(),
            start_median: quantile(&self.start, 0.5),
            ..Side::default()
        };
        if b.req > 0 {
            s.ctx_per_req = b.context() as f64 / b.req as f64;
            s.out_per_req = b.out as f64 / b.req as f64;
            s.cost_per_req = b.cost() / b.req as f64;
        }
        let c = b.context();
        if c > 0 {
            s.cache_hit = 100.0 * b.cr as f64 / c as f64;
        }
        let calls: i64 = (0..N_TOOLS).map(|i| b.calls[i] as i64).sum();
        let bytes: i64 = b.bytes.iter().sum();
        if calls > 0 {
            s.tool_per_call = bytes as f64 / calls as f64;
        }
        if !self.costs.is_empty() {
            let mut costs = self.costs.clone();
            costs.sort_by(f64::total_cmp);
            s.cost_session = costs[costs.len() / 2];
        }
        s
    }
}

impl Store {
    /// The figures days either side of at, for q's accounts and folder.
    pub fn compare(
        &self,
        q: &Query,
        at: Time,
        days: i64,
        saver: Option<&'static Saver>,
    ) -> Compare {
        let inner = self.read();
        let span = days * DAY;
        let (from, to) = (add_secs(at, -span), add_secs(at, span));
        let mut before = SideAcc::default();
        let mut after = SideAcc::default();
        let mut with = SideAcc::default();
        let mut without = SideAcc::default();
        for f in inner.files.values() {
            if !q.has(f) || f.first > to || f.last < from {
                continue;
            }
            let is_after = if f.first < at && f.first >= from {
                false
            } else if f.first >= at && f.first < to {
                true
            } else {
                continue;
            };
            let side = if is_after { &mut after } else { &mut before };
            let t = f.totals();
            side.b.add(&t);
            if f.sub {
                continue;
            }
            side.n += 1;
            side.costs.push(t.cost());
            side.start.push(f.start());
            let Some(sv) = saver else { continue };
            if !is_after {
                continue;
            }
            if f.uses.keys().any(|k| sv.matches(k)) {
                with.add(f, &t);
            } else {
                without.add(f, &t);
            }
        }
        Compare {
            at,
            days,
            saver,
            before: before.side(),
            after: after.side(),
            with: with.side(),
            without: without.side(),
        }
    }
}
