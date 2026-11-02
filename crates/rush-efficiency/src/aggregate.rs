//! A view of the figures: one query's points, totals and what the savers
//! did inside it. Port of internal/efficiency/aggregate.go.

use std::collections::HashMap;

use rush_state::Time;

use crate::catalog::CATALOG;
use crate::compare::SideAcc;
use crate::price::cache_read_price;
use crate::scan::{Bucket, File, N_TOOLS, TOOL_NAMES, Use};
use crate::store::Store;
use crate::when::{DAY, HOUR, add_secs, at_secs, hour_of, local_day_start, next_local_day, secs};

/// A figure the Timeline can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    /// Tokens sent per request: what each call carries.
    Ctx,
    /// Dollars, by what they paid for.
    Cost,
    /// Tokens, by kind.
    Tokens,
    /// Output per request, thinking and text.
    Out,
    /// Share of input read from cache.
    Cache,
    /// Tool output per call, by tool.
    ToolKb,
    /// Context sessions start with.
    Start,
    /// Requests.
    Req,
}

pub const METRICS: [Metric; 8] = [
    Metric::Ctx,
    Metric::Cost,
    Metric::Tokens,
    Metric::Out,
    Metric::Cache,
    Metric::ToolKb,
    Metric::Start,
    Metric::Req,
];

pub const METRIC_NAMES: [&str; 8] = [
    "context per request",
    "cost",
    "tokens",
    "output per request",
    "cache hits",
    "tool output per call",
    "context at start",
    "requests",
];

impl Metric {
    pub fn name(self) -> &'static str {
        METRIC_NAMES[self as usize]
    }
}

/// What a view covers: whose transcripts, which folder, which time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Account folders; none is every account.
    pub accounts: Vec<String>,
    /// A folder: sessions that worked in it or below.
    pub project: String,
    pub from: Time,
    pub to: Time,
    /// Buckets of a local day rather than an hour.
    pub daily: bool,
}

/// One of the spans the place offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub name: &'static str,
    /// Seconds.
    pub span: i64,
    pub daily: bool,
}

pub const RANGES: [Range; 4] = [
    Range {
        name: "24 hours",
        span: DAY,
        daily: false,
    },
    Range {
        name: "7 days",
        span: 7 * DAY,
        daily: true,
    },
    Range {
        name: "30 days",
        span: 30 * DAY,
        daily: true,
    },
    Range {
        name: "90 days",
        span: 90 * DAY,
        daily: true,
    },
];

/// One bucket of the view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub at: Time,
    pub b: Bucket,
    /// Contexts sessions that began here started with.
    pub starts: Vec<i64>,
}

/// How much a saver was used in a view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SaverUse {
    pub n: i64,
    pub sessions: i64,
    pub first: Time,
    pub last: Time,
}

/// Everything the place shows for one query.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub q: Query,
    pub points: Vec<Point>,
    pub total: Bucket,

    pub sessions: i64,
    pub subagents: i64,
    pub start_median: i64,
    pub start_p90: i64,
    pub peak_median: i64,
    /// Sessions whose context passed FAT_CONTEXT.
    pub fat: i64,
    pub compacts: i64,
    pub autos: i64,
    /// Tool results over BIG_RESULT.
    pub big: i64,

    /// Sessions that read one file REREAD_AT times or more; the worst is
    /// the most reads of one file.
    pub rereads: i64,
    pub worst_read: String,
    pub worst_reads: i64,

    /// Subagents' spend, and how much of it ran on Opus or Fable.
    pub sub_cost: f64,
    pub sub_big_cost: f64,
    /// What each tool's output cost to keep in context: every token of it
    /// is read again by the requests after it.
    pub carry: [f64; N_TOOLS],
    /// The same for shell output that searched or read code, and for
    /// context past FAT_CONTEXT.
    pub look_carry: f64,
    pub fat_carry: f64,

    /// By saver ID, inside the view.
    pub uses: HashMap<String, SaverUse>,
    /// When each saver was first seen at all, in any transcript of these
    /// accounts: the markers on the Timeline.
    pub first_use: HashMap<String, Time>,
    /// How many transcripts the store knows.
    pub scanned: usize,

    /// By saver ID: sessions with it, without.
    pub(crate) split: HashMap<String, [SideAcc; 2]>,
}

pub const FAT_CONTEXT: i64 = 400_000;
pub const REREAD_AT: i64 = 4;

impl Query {
    /// Whether a file is inside the query's accounts and folder.
    pub fn has(&self, f: &File) -> bool {
        self.has_account(&f.account) && self.has_project(&f.project)
    }

    fn has_account(&self, acct: &str) -> bool {
        self.accounts.is_empty() || self.accounts.iter().any(|a| a == acct)
    }

    fn has_project(&self, p: &str) -> bool {
        self.project.is_empty()
            || p == self.project
            || p.starts_with(&format!("{}/", self.project.trim_end_matches('/')))
    }

    /// The start of the bucket t falls in.
    pub fn bucket_start(&self, t: Time) -> Time {
        if self.daily {
            return local_day_start(t);
        }
        at_secs(secs(t).div_euclid(HOUR) * HOUR)
    }

    pub fn next(&self, t: Time) -> Time {
        if self.daily {
            return next_local_day(t);
        }
        add_secs(t, HOUR)
    }

    /// A range ending now.
    pub fn new(r: Range, now: Time) -> Query {
        let mut q = Query {
            to: now,
            daily: r.daily,
            ..Query::default()
        };
        q.from = q.bucket_start(add_secs(now, -r.span));
        if r.daily {
            q.from = q.next(q.from);
        }
        q
    }
}

impl Store {
    /// Works out a query's figures.
    pub fn view(&self, q: &Query) -> View {
        let inner = self.read();
        let mut v = View {
            q: q.clone(),
            scanned: inner.files.len(),
            ..View::default()
        };

        let mut index: HashMap<i64, usize> = HashMap::new();
        let mut t = q.bucket_start(q.from);
        while t <= q.to {
            index.insert(secs(t), v.points.len());
            v.points.push(Point {
                at: t,
                ..Point::default()
            });
            t = q.next(t);
        }
        let slot = |t: Time| index.get(&secs(q.bucket_start(t))).copied();
        let (from_h, to_h) = (hour_of(q.from), hour_of(q.to));
        let mut starts = Vec::new();
        let mut peaks = Vec::new();
        let mut sessions: Vec<(&File, Bucket)> = Vec::new();

        let first_use = |v: &mut View, key: &str, u: &Use| {
            for sv in CATALOG.iter().filter(|s| s.matches(key)) {
                match v.first_use.get(sv.id) {
                    Some(t) if u.first >= *t => {}
                    _ => {
                        v.first_use.insert(sv.id.to_string(), u.first);
                    }
                }
            }
        };
        for f in inner.files.values() {
            if !q.has(f) {
                continue;
            }
            for (k, u) in &f.uses {
                first_use(&mut v, k, u);
            }
            if f.last < q.from || f.first > q.to {
                continue;
            }
            let mut file_b = Bucket::default();
            f.each_hour(|h, b| {
                if h < from_h || h > to_h {
                    return;
                }
                if let Some(i) = slot(at_secs(h * HOUR)) {
                    v.points[i].b.add(b);
                    file_b.add(b);
                }
            });
            v.total.add(&file_b);
            if f.sub {
                v.subagents += 1;
                v.sub_cost += file_b.cost();
                let m = &f.model;
                if m.contains("opus") || m.contains("fable") || m.contains("mythos") {
                    v.sub_big_cost += file_b.cost();
                }
            } else if f.first >= q.from && file_b.req > 0 {
                v.sessions += 1;
                starts.push(f.start());
                peaks.push(f.peak());
                if let Some(i) = slot(f.first)
                    && f.start() > 0
                {
                    v.points[i].starts.push(f.start());
                }
                sessions.push((f, file_b));
                let peak = f.peak();
                if peak > FAT_CONTEXT {
                    v.fat += 1;
                    // Context grows roughly steadily, so the requests after
                    // it passed FAT_CONTEXT carried half the excess on
                    // average.
                    let over = (peak - FAT_CONTEXT) as f64;
                    v.fat_carry += over / 2.0 * file_b.req as f64 * over / peak as f64
                        * cache_read_price(&f.model)
                        / 1e6;
                }
                let (mut worst, mut worst_n) = ("", 0);
                for (path, &n) in &f.reads {
                    if n > worst_n || (n == worst_n && n > 0 && path.as_str() < worst) {
                        (worst, worst_n) = (path, n);
                    }
                }
                if worst_n >= REREAD_AT {
                    v.rereads += 1;
                    if worst_n > v.worst_reads {
                        v.worst_read = worst.to_string();
                        v.worst_reads = worst_n;
                    }
                }
            }
            for c in &f.compacts {
                if c.at >= q.from && c.at <= q.to {
                    v.compacts += 1;
                    if c.auto {
                        v.autos += 1;
                    }
                }
            }
            v.big += f.big;
            // A token of tool output is read again by roughly half the
            // session's later requests, on average, at the cache-read
            // price.
            if file_b.req > 0 {
                let read = cache_read_price(&f.model);
                let per = file_b.req as f64 / 2.0 * read / 1e6;
                for c in 0..N_TOOLS {
                    v.carry[c] += file_b.bytes[c] as f64 / 4.0 * per;
                }
                v.look_carry += file_b.look as f64 / 4.0 * per;
            }
            for (k, u) in &f.uses {
                if u.last < q.from || u.first > q.to {
                    continue;
                }
                for sv in CATALOG.iter().filter(|s| s.matches(k)) {
                    let su = v.uses.entry(sv.id.to_string()).or_default();
                    su.n += u.n;
                    su.sessions += 1;
                    if su.first.is_zero() || u.first < su.first {
                        su.first = u.first;
                    }
                    if u.last > su.last {
                        su.last = u.last;
                    }
                }
            }
        }
        for (acct, r) in &inner.retired {
            if !q.has_account(acct) || !q.project.is_empty() {
                continue;
            }
            for (k, u) in &r.uses {
                first_use(&mut v, k, u);
            }
            for (&h, b) in &r.hours {
                if h < from_h || h > to_h {
                    continue;
                }
                if let Some(i) = slot(at_secs(h * HOUR)) {
                    v.points[i].b.add(b);
                    v.total.add(b);
                }
            }
        }
        for sv in CATALOG.iter() {
            let Some(&first) = v.first_use.get(sv.id) else {
                continue;
            };
            if sv.uses.is_empty() {
                continue;
            }
            let mut sp: [SideAcc; 2] = Default::default();
            for (f, b) in &sessions {
                if f.first < first {
                    continue;
                }
                let used = f.uses.keys().any(|k| sv.matches(k));
                sp[if used { 0 } else { 1 }].add(f, b);
            }
            v.split.insert(sv.id.to_string(), sp);
        }
        v.start_median = quantile(&starts, 0.5);
        v.start_p90 = quantile(&starts, 0.9);
        v.peak_median = quantile(&peaks, 0.5);
        v
    }
}

/// The q-th quantile of the non-zero xs.
pub fn quantile(xs: &[i64], q: f64) -> i64 {
    let mut nz: Vec<i64> = xs.iter().copied().filter(|&x| x > 0).collect();
    if nz.is_empty() {
        return 0;
    }
    nz.sort_unstable();
    nz[((nz.len() as f64 * q) as usize).min(nz.len() - 1)]
}

/// One metric's values for each point, split into the parts the Timeline
/// stacks, bottom first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub parts: Vec<&'static str>,
    /// [point][part]
    pub values: Vec<Vec<f64>>,
    /// "tok", "$", "%", "B"
    pub unit: &'static str,
}

impl View {
    /// A metric over the view's points.
    pub fn series(&self, m: Metric) -> Series {
        let per = |n: i64, d: i64| if d == 0 { 0.0 } else { n as f64 / d as f64 };
        let (parts, unit): (Vec<&'static str>, &'static str) = match m {
            Metric::Ctx => (vec!["cache read", "cache write", "input"], "tok"),
            Metric::Cost => (vec!["cache read", "cache write", "input", "output"], "$"),
            Metric::Tokens => (vec!["cache read", "cache write", "input", "output"], "tok"),
            Metric::Out => (vec!["thinking", "text & tools"], "tok"),
            Metric::Cache => (vec!["from cache"], "%"),
            Metric::ToolKb => (TOOL_NAMES.to_vec(), "B"),
            Metric::Start => (vec!["median start"], "tok"),
            Metric::Req => (vec!["requests"], ""),
        };
        let values = self
            .points
            .iter()
            .map(|p| {
                let b = &p.b;
                match m {
                    Metric::Ctx => vec![per(b.cr, b.req), per(b.cw, b.req), per(b.input, b.req)],
                    Metric::Cost => vec![b.c_cr, b.c_cw, b.c_in, b.c_out],
                    Metric::Tokens => {
                        vec![b.cr as f64, b.cw as f64, b.input as f64, b.out as f64]
                    }
                    Metric::Out => {
                        let think = b.think.min(b.out);
                        vec![per(think, b.req), per(b.out - think, b.req)]
                    }
                    Metric::Cache => vec![100.0 * per(b.cr, b.context())],
                    Metric::ToolKb => {
                        let calls: i64 = b.calls.iter().map(|&c| c as i64).sum();
                        (0..N_TOOLS).map(|c| per(b.bytes[c], calls)).collect()
                    }
                    Metric::Start => vec![quantile(&p.starts, 0.5) as f64],
                    Metric::Req => vec![b.req as f64],
                }
            })
            .collect();
        Series {
            parts,
            values,
            unit,
        }
    }

    /// The share of input read from the cache, in percent.
    pub fn cache_hit(&self) -> f64 {
        let c = self.total.context();
        if c > 0 {
            100.0 * self.total.cr as f64 / c as f64
        } else {
            0.0
        }
    }

    pub fn ctx_per_req(&self) -> i64 {
        if self.total.req == 0 {
            0
        } else {
            self.total.context() / self.total.req
        }
    }

    pub fn out_per_req(&self) -> i64 {
        if self.total.req == 0 {
            0
        } else {
            self.total.out / self.total.req
        }
    }

    /// All tool output in the view, and each tool's share of it.
    pub fn tool_bytes(&self) -> (i64, [f64; N_TOOLS]) {
        let all: i64 = self.total.bytes.iter().sum();
        let mut share = [0.0; N_TOOLS];
        if all > 0 {
            for (s, &b) in share.iter_mut().zip(&self.total.bytes) {
                *s = b as f64 / all as f64;
            }
        }
        (all, share)
    }
}
