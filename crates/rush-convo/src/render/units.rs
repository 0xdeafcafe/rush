//! The unit memo: an open turn drawn part by part, each finished part (an
//! item, or a run of steps folded together) copied as it was last drawn
//! while nothing it reads has changed. A running turn redraws every frame;
//! only what still runs in it is drawn anew. Port of Go's drawer.memoized,
//! unitKey, unitDrawn and unitPrint (render.go:953-1110).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use rush_hl::Lang;

use super::RenderedLine;
use super::code::HlState;
use super::draw::Drawer;
use super::shell::OutSpan;
use crate::model::{Item, Status, StepId};

/// What of a drawer's own state carries from one part of a turn to the
/// next: a part's key holds it going in, its memo coming out.
#[derive(Clone, Default)]
pub(crate) struct Carry {
    pub named: bool,
    pub rule: bool,
    pub peek: bool,
    pub tool: i32,
    pub helper: i32,
    pub while_: bool,
    pub worked: bool,
    pub marks: HashSet<String>,
    pub lg: Option<&'static Lang>,
    pub by_path: bool,
    pub hs: HlState,
    pub hs_path: String,
    pub hs_n: usize,
    pub spans: Vec<OutSpan>,
    pub view: &'static str,
    pub nowrap: bool,
    pub fast: bool,
}

fn same_lang(a: Option<&'static Lang>, b: Option<&'static Lang>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        _ => false,
    }
}

impl PartialEq for Carry {
    fn eq(&self, o: &Carry) -> bool {
        self.named == o.named
            && self.rule == o.rule
            && self.peek == o.peek
            && self.tool == o.tool
            && self.helper == o.helper
            && self.while_ == o.while_
            && self.worked == o.worked
            && self.marks == o.marks
            && same_lang(self.lg, o.lg)
            && self.by_path == o.by_path
            && self.hs == o.hs
            && self.hs_path == o.hs_path
            && self.hs_n == o.hs_n
            && self.spans.len() == o.spans.len()
            && self.spans.iter().zip(&o.spans).all(|(a, b)| {
                same_lang(a.lg, b.lg)
                    && a.n == b.n
                    && a.by_path == b.by_path
                    && a.diff == b.diff
                    && a.git == b.git
            })
            && self.view == o.view
            && self.nowrap == o.nowrap
            && self.fast == o.fast
    }
}

/// What a part of an open turn was drawn from (unitKey).
#[derive(PartialEq)]
struct UnitKey {
    /// Items in it.
    n: usize,
    /// What they had (unit_print).
    print: u64,
    /// The turn's own: its folds, its selection, what it ends asking,
    /// the session's folders, the spine it's drawn on.
    ctx: u64,
    /// It holds the session's newest step.
    latest: bool,
    /// The row above: 0 none, else what of it a part reads.
    above: u8,
    carry: Carry,
}

/// A part as last drawn (unitDrawn): its rows and the drawer's state
/// coming out.
struct Unit {
    key: UnitKey,
    lines: Vec<RenderedLine>,
    out: Carry,
    /// The row picked when it was drawn, when one of its rows was that
    /// row (its ref or its owner); None when the pick was elsewhere.
    sel: Option<String>,
}

/// The pick, when one of lines is it (its ref, or the item it's part of).
fn picked_in(lines: &[RenderedLine], sel: &str) -> Option<String> {
    if sel.is_empty() {
        return None;
    }
    lines
        .iter()
        .any(|l| l.r#ref == sel || l.owner == sel)
        .then(|| sel.to_string())
}

/// An open turn's parts as last drawn, by the index of each one's first
/// item: one each, as Go keeps it on the item.
#[derive(Default)]
pub struct Units(HashMap<usize, Unit>);

impl Units {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// FNV-1a, as unitPrint mixes.
struct Fnv(u64);

impl Fnv {
    fn mix(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(1_099_511_628_211);
    }
    fn text(&mut self, s: &str) {
        // Streamed text only grows: its length and how it ends say how
        // far it's got.
        self.mix(s.len() as u64);
        for &b in &s.as_bytes()[s.len().saturating_sub(8)..] {
            self.mix(u64::from(b));
        }
    }
}

impl Drawer<'_> {
    pub(crate) fn carry_out(&self) -> Carry {
        Carry {
            named: self.named,
            rule: self.rule,
            peek: self.peek,
            tool: self.tool,
            helper: self.helper,
            while_: self.while_,
            worked: self.worked,
            marks: self.marks.clone(),
            lg: self.lg,
            by_path: self.by_path,
            hs: self.hs,
            hs_path: self.hs_path.clone(),
            hs_n: self.hs_n,
            spans: self.spans.clone(),
            view: self.view,
            nowrap: self.nowrap,
            fast: self.fast,
        }
    }

    fn take_carry(&mut self, c: &Carry) {
        self.named = c.named;
        self.rule = c.rule;
        self.peek = c.peek;
        self.tool = c.tool;
        self.helper = c.helper;
        self.while_ = c.while_;
        self.worked = c.worked;
        self.marks.clone_from(&c.marks);
        self.lg = c.lg;
        self.by_path = c.by_path;
        self.hs = c.hs;
        self.hs_path.clone_from(&c.hs_path);
        self.hs_n = c.hs_n;
        self.spans.clone_from(&c.spans);
        self.view = c.view;
        self.nowrap = c.nowrap;
        self.fast = c.fast;
    }

    /// What the row above reads as to what follows: none, a blank of the
    /// turn's, a row of only the spine, or anything else.
    fn above(&self) -> u8 {
        let n = self.lines.len();
        if n == 0 {
            return 0;
        }
        1 + u8::from(self.is_blank(n - 1)) + 2 * u8::from(self.spine_only(n - 1))
    }

    /// What the turn's parts all read besides their own items.
    pub(crate) fn unit_ctx(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut open: Vec<_> = self.o.open.iter().collect();
        open.sort();
        open.hash(&mut h);
        let mut view: Vec<_> = self.o.view.iter().collect();
        view.sort();
        view.hash(&mut h);
        // Not the row picked: a part is drawn again only when the pick
        // comes to or leaves one of its own rows (memoized), so moving it
        // through a turn of thousands of steps draws two parts, not all
        // of them (Rust-only: Go's ctx held the pick, render.go:1003).
        self.o.needs.key().hash(&mut h);
        (&self.s.info.cwd, &self.s.cwd).hash(&mut h);
        self.spine().plain().hash(&mut h);
        h.finish()
    }

    /// A fingerprint of what items lo..hi have, when they're done changing
    /// with the clock: none of their steps (nor theirs) runs or waits, and
    /// nothing drawn from beside the session (a script, a pause) is theirs
    /// (unitPrint).
    fn unit_print(&self, lo: usize, hi: usize) -> Option<u64> {
        let mut h = Fnv(14_695_981_039_346_656_037);
        for (j, it) in self.t.items[lo..hi].iter().enumerate() {
            match it {
                Item::Step(id) => {
                    h.mix(1);
                    self.step_print(*id, &mut h)?;
                }
                Item::Text { text, answer } => {
                    // Its shell blocks' runs change as they run: drawn
                    // every time while it has any.
                    if self.has_runs(lo + j) {
                        return None;
                    }
                    h.mix(2);
                    h.text(text);
                    h.mix(u64::from(*answer));
                }
                Item::Thinking(text) => {
                    h.mix(3);
                    h.text(text);
                }
                Item::Interject(x) => {
                    h.mix(4);
                    h.text(&x.text);
                    h.mix(x.images.len() as u64);
                    h.mix(x.pictures.len() as u64);
                }
                Item::Exchange(x) => {
                    h.mix(5);
                    h.text(&x.exchange.text);
                    h.text(&x.exchange.id);
                    h.mix(x.exchange.images.len() as u64);
                    h.mix(x.pictures.len() as u64);
                    // A run of PR news draws at its first: what follows
                    // it in the run is that card's too.
                    for next in self.t.items[lo + j + 1..]
                        .iter()
                        .take_while(|it| super::draw::pr_news(it))
                    {
                        if let Item::Exchange(y) = next {
                            h.text(&y.exchange.text);
                            h.text(&y.exchange.id);
                        }
                    }
                }
                // Cheap to draw.
                Item::Compact { .. } | Item::Notice { .. } => return None,
            }
        }
        Some(h.0)
    }

    fn step_print(&self, id: StepId, h: &mut Fnv) -> Option<()> {
        let st = self.s.step(id);
        if matches!(st.status, Status::Running | Status::Waiting)
            || st.approval.is_some()
            || st.kind == rush_core::tool::Kind::Subagent
            || self.o.paused.contains(&*st.id)
            // Returned, its command runs on: which part runs changes.
            || self.s.job_running(&st.id)
            || self.o.scripts.contains_key(&*st.id)
            || self.o.ran.contains_key(&*st.id)
        {
            return None;
        }
        h.mix(st.status as u64);
        h.mix(st.input_len() as u64);
        h.mix(st.output.len() as u64);
        h.mix(st.result_json().len() as u64);
        h.mix(st.exit as u64);
        h.mix(st.start as u64);
        h.mix(st.end as u64);
        h.mix(st.children.len() as u64);
        h.mix(st.images.len() as u64);
        if let Some(ks) = self.o.kids.get(&*st.id) {
            for c in ks {
                let (n, last, live, idn) = c.print();
                if live {
                    return None;
                }
                h.mix(n as u64);
                h.mix(last as u64);
                h.mix(idn as u64);
            }
        }
        for &c in &st.children {
            self.step_print(c, h)?;
        }
        Some(())
    }

    /// Draws items lo..hi with draw, or copies how they were last drawn
    /// when nothing they read has changed since. Only for what's done: a
    /// running or waiting step reads the clock, and is drawn every time.
    pub(crate) fn memoized(&mut self, lo: usize, hi: usize, draw: impl FnOnce(&mut Self)) {
        let (Some(ctx), Some(print)) = (self.ctx, self.unit_print(lo, hi)) else {
            draw(self);
            return;
        };
        if self.units.is_none() {
            draw(self);
            return;
        }
        let key = UnitKey {
            n: hi - lo,
            print,
            ctx,
            latest: self.latest.is_some_and(|l| {
                self.t.items[lo..hi]
                    .iter()
                    .any(|it| matches!(it, Item::Step(id) if *id == l))
            }),
            above: self.above(),
            carry: self.carry_out(),
        };
        let o = self.o;
        if let Some(u) = self.units.as_ref().and_then(|m| m.0.get(&lo))
            && u.key == key
            && picked_in(&u.lines, &o.selected) == u.sel
        {
            let (lines, out) = (u.lines.clone(), u.out.clone());
            self.lines.extend(lines);
            self.take_carry(&out);
            return;
        }
        let from = self.lines.len();
        let before = self.lines.last().cloned();
        draw(self);
        // Kept only when it drew rows of its own and left those before it
        // as they were: what it drew is then all it did.
        let kept = self.lines.len() >= from && self.lines[..from].last() == before.as_ref();
        let out = self.carry_out();
        let lines = if kept {
            Some(self.lines[from..].to_vec())
        } else {
            None
        };
        if let Some(m) = self.units.as_deref_mut() {
            match lines {
                Some(lines) => {
                    let sel = picked_in(&lines, &o.selected);
                    m.0.insert(
                        lo,
                        Unit {
                            key,
                            lines,
                            out,
                            sel,
                        },
                    );
                }
                None => {
                    m.0.remove(&lo);
                }
            }
        }
    }
}
