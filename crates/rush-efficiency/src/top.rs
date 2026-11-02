//! The costliest transcripts in a view: what the advisor is given to
//! decide which conversations are worth reading. Port of
//! internal/efficiency/top.go.

use rush_state::Time;

use crate::aggregate::Query;
use crate::scan::Bucket;
use crate::store::Store;
use crate::when::hour_of;

/// One transcript's figures inside a query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionCost {
    /// The transcript.
    pub path: String,
    pub project: String,
    pub model: String,
    pub sub: bool,
    pub first: Time,
    pub last: Time,
    pub b: Bucket,
    pub start_ctx: i64,
    pub peak_ctx: i64,
    pub compacts: i64,
    pub big: i64,
    /// The file read most often, and how many times.
    pub worst_read: String,
    pub worst_reads: i64,
}

impl Store {
    /// The n transcripts that cost most inside q, costliest first.
    pub fn top_sessions(&self, q: &Query, n: usize) -> Vec<SessionCost> {
        let inner = self.read();
        let (from_h, to_h) = (hour_of(q.from), hour_of(q.to));
        let mut out = Vec::new();
        for (p, f) in &inner.files {
            if !q.has(f) || f.last < q.from || f.first > q.to {
                continue;
            }
            let mut c = SessionCost {
                path: p.clone(),
                project: f.project.clone(),
                model: f.model.clone(),
                sub: f.sub,
                first: f.first,
                last: f.last,
                start_ctx: f.start_ctx,
                peak_ctx: f.peak_ctx,
                compacts: f.compacts.len() as i64,
                big: f.big,
                ..SessionCost::default()
            };
            f.each_hour(|h, b| {
                if h >= from_h && h <= to_h {
                    c.b.add(b);
                }
            });
            if c.b.req == 0 {
                continue;
            }
            for (file, &k) in &f.reads {
                if k > c.worst_reads || (k == c.worst_reads && *file < c.worst_read) {
                    c.worst_read = file.clone();
                    c.worst_reads = k;
                }
            }
            out.push(c);
        }
        out.sort_by(|a, b| {
            b.b.cost()
                .total_cmp(&a.b.cost())
                .then_with(|| a.path.cmp(&b.path))
        });
        out.truncate(n);
        out
    }
}
