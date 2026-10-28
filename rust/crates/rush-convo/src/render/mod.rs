//! A Session drawn as rows of styled text, for the conversation view. Port
//! of internal/convo/render.go:30-460 (Line, Options, Depth, the width
//! caps, Render, the per-turn cache and its key, latest, StepOpen).
//!
//! Unlike Go's Render, which walks every turn every frame, the Renderer
//! only draws what's in view: each turn's height is kept with the key it
//! was drawn under, and its rows only while it's near the viewport. A frame
//! of a 3,000-turn conversation draws the few turns on screen; scrolling to
//! the top lays the rest out once.

mod cards;
mod code;
mod decisions;
mod draw;
mod markdown;
mod shell;
mod style;
mod txt;
mod udiff;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use rush_core::text::Line;
use rush_core::time::Ms;

pub use cards::ENTITY_REF;
pub use decisions::{decisions_on_card, decisions_ref, decisions_sent};
pub use draw::{OWNER_SEP, image_label};
pub use markdown::markdown_lines;
pub use style::{SHOW_ALL_REF, dur, key_word, money};

use crate::model::{Item, Session, StepId};
use draw::{Drawer, hidden, squeezed, waiting};
use shell::Places;

/// Numbers never drift further right than this.
pub const CAP_ROW: usize = 124;
/// Prose wraps here however wide the pane.
pub const CAP_PROSE: usize = 120;
/// And here when nothing sits beside it.
pub const WIDE_PROSE: usize = 160;
/// Room for tool status and nested group rails.
pub const GUTTER: usize = 4;

/// How much of a turn's work shows: Prose is what's said and a row for
/// failed runs; Runs folds every run of steps; Default folds clean runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Depth {
    #[default]
    Default,
    Prose,
    Runs,
}

/// How to draw.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub width: usize,
    pub now: Ms,
    /// The spinner's frame.
    pub tick: usize,
    /// Fold overrides by ref; absent means the default.
    pub open: HashMap<String, bool>,
    /// The view a step's output was switched to, by ref: "text" or
    /// "pretty"; absent means the one it opens in.
    pub view: HashMap<String, String>,
    /// ctrl+o: open everything, trim nothing.
    pub verbose: bool,
    pub depth: Depth,
    pub selected: String,
    pub focused: bool,
    /// Rows may run the whole width, past CAP_ROW.
    pub wide: bool,
    /// Every turn drawn open, not only the latest.
    pub history_open: bool,
    /// The live line is drawn elsewhere.
    pub hide_activity: bool,
    /// No rule in the gutter.
    pub no_rule: bool,
    /// Who answers under your messages, and what it is (its heading).
    pub agent: String,
    pub speaker: String,
    /// Seconds east of UTC that clock times are shown in.
    pub zone: i64,
}

impl Options {
    /// How wide a row's numbers and rules may run.
    pub fn row_cap(&self) -> usize {
        if self.wide {
            self.width.max(CAP_ROW)
        } else {
            CAP_ROW
        }
    }

    /// Where prose wraps.
    pub fn prose_cap(&self) -> usize {
        if self.wide {
            CAP_PROSE.max(self.width.saturating_sub(GUTTER + 4).min(WIDE_PROSE))
        } else {
            CAP_PROSE
        }
    }

    /// What every turn's drawing reads, apart from its own folds,
    /// selection and clock.
    fn sig(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (
            self.width,
            self.wide,
            self.verbose,
            self.depth,
            self.history_open,
            self.hide_activity,
            self.no_rule,
            &self.agent,
            &self.speaker,
            self.zone,
        )
            .hash(&mut h);
        h.finish()
    }
}

/// One drawn row: its styled text, the ref it acts on ("t13" a turn's
/// heading, "t13:s:<tool id>" a step, "t13:run:4" a folded run; empty for
/// rows you can't act on), whether it carries on the row above, wrapped,
/// and the items it's part of, outermost first, joined by OWNER_SEP.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderedLine {
    pub line: Line,
    pub r#ref: String,
    pub wrap: bool,
    pub owner: String,
}

/// What a turn's drawing reads: when it's the same, its rows are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TurnKey {
    ver: u32,
    recent: bool,
    hooks: usize,
    latest: Option<StepId>,
    folds: String,
    sel: String,
    /// A running or waiting turn: the spinner's frame and the clock.
    clock: Option<(usize, i64)>,
}

#[derive(Default)]
struct Slot {
    key: Option<TurnKey>,
    height: usize,
    lines: Option<Vec<RenderedLine>>,
    fast: bool,
}

/// Draws a Session's turns, keeping each turn's height and, near the
/// view, its rows.
#[derive(Default)]
pub struct Renderer {
    slots: Vec<Slot>,
    sig: u64,
    places: Places,
    places_for: String,
    /// Each turn's first row, once every height before it is known.
    offsets: Vec<usize>,
    partial: Option<RenderedLine>,
    fast: bool,
    /// What the offsets were worked out for, and the turns on the clock.
    stamp: Option<Stamp>,
    clocked: Vec<usize>,
    /// How many turns either side of the view keep their rows.
    pub keep: usize,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            keep: 2,
            ..Renderer::default()
        }
    }

    /// Whether the last frame drew a timer showing tenths: the caller draws
    /// again in a tenth of a second rather than a second.
    pub fn fast(&self) -> bool {
        self.fast
    }

    /// Forgets every turn as drawn.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.offsets.clear();
        self.stamp = None;
    }

    /// Rows in memory, for measuring: those kept for turns near the view.
    pub fn kept_rows(&self) -> usize {
        self.slots
            .iter()
            .filter_map(|s| s.lines.as_ref())
            .map(Vec::len)
            .sum()
    }

    fn prepare(&mut self, s: &Session, o: &Options) {
        let sig = o.sig();
        if sig != self.sig {
            self.slots.clear();
            self.offsets.clear();
            self.sig = sig;
        }
        if self.places_for != s.cwd || self.places.bases.is_empty() && !s.cwd.is_empty() {
            self.places = Places::new(&s.cwd);
            self.places_for = s.cwd.clone();
            self.slots.clear();
            self.offsets.clear();
        }
        self.slots.resize_with(s.turns.len(), Slot::default);
        self.partial = s.partial.then(|| RenderedLine {
            line: style::dim("  reading earlier…").into_line(),
            ..RenderedLine::default()
        });
        self.fast = false;
    }

    /// The key turn i is drawn under now.
    fn key(
        &self,
        s: &Session,
        o: &Options,
        i: usize,
        latest: Option<(StepId, usize)>,
        folds: &Folds,
    ) -> TurnKey {
        let t = &s.turns[i];
        let mut k = TurnKey {
            ver: t.ver(),
            recent: o.history_open || i + 1 == s.turns.len(),
            hooks: if i == 0 { s.hooks.len() } else { 0 },
            latest: latest.filter(|l| l.1 == i).map(|l| l.0),
            ..TurnKey::default()
        };
        if !folds.0.is_empty() || !o.selected.is_empty() {
            let r = format!("t{}", t.n);
            k.folds = folds.of(&r);
            if o.selected == r
                || o.selected
                    .strip_prefix(&r)
                    .is_some_and(|x| x.starts_with(':'))
            {
                k.sel = o.selected.clone();
            }
        }
        if t.live || waiting(s, t) {
            let fast = self.slots.get(i).is_some_and(|x| x.fast);
            k.clock = Some((o.tick, if fast { o.now / 100 } else { o.now / 1000 }));
        }
        k
    }

    /// Turn i's rows, drawn afresh only when what it reads has changed.
    fn turn(
        &mut self,
        s: &Session,
        o: &Options,
        i: usize,
        latest: Option<(StepId, usize)>,
        folds: &Folds,
    ) -> &Vec<RenderedLine> {
        let k = self.key(s, o, i, latest, folds);
        let slot = &self.slots[i];
        if slot.key.as_ref() != Some(&k) || slot.lines.is_none() {
            let (lines, fast) = draw_turn(s, o, &self.places, i, k.latest, k.recent);
            let slot = &mut self.slots[i];
            slot.height = lines.len();
            slot.key = Some(k);
            slot.fast = fast;
            slot.lines = Some(lines);
        }
        self.fast |= self.slots[i].fast;
        self.slots[i].lines.as_ref().unwrap()
    }

    /// Turn i's height, drawing it only when it isn't known.
    fn height(
        &mut self,
        s: &Session,
        o: &Options,
        i: usize,
        latest: Option<(StepId, usize)>,
        folds: &Folds,
    ) -> usize {
        let k = self.key(s, o, i, latest, folds);
        if self.slots[i].key.as_ref() == Some(&k) {
            return self.slots[i].height;
        }
        self.turn(s, o, i, latest, folds).len()
    }

    /// Drops the rows of turns outside lo..hi, keeping their heights.
    fn forget_outside(&mut self, lo: usize, hi: usize) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if (i + self.keep < lo || i > hi + self.keep) && slot.lines.is_some() {
                slot.lines = None;
            }
        }
    }

    /// Every row, oldest first: for a dump, or a caller that wants all of
    /// it. Rows of every turn are kept until the next call.
    pub fn render(&mut self, s: &Session, o: &Options) -> Vec<RenderedLine> {
        self.prepare(s, o);
        self.stamp = None; // the offsets aren't kept here
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let mut out = Vec::new();
        if let Some(p) = &self.partial {
            out.push(p.clone());
        }
        for i in 0..s.turns.len() {
            out.extend(self.turn(s, o, i, latest, &folds).iter().cloned());
        }
        out
    }

    /// Rows top..top+rows of the whole conversation, counting from its first
    /// row. The first call lays every turn out once to learn its height;
    /// after that a frame only checks the turns that can have changed (the
    /// running one, unless the session or the folds moved on) and draws
    /// the turns in view.
    pub fn render_range(
        &mut self,
        s: &Session,
        o: &Options,
        top: usize,
        rows: usize,
    ) -> Vec<&RenderedLine> {
        self.prepare(s, o);
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let lead = usize::from(self.partial.is_some());
        let stamp = Stamp {
            applied: s.applied(),
            turns: s.turns.len(),
            latest,
            folds: folds.sig(),
            selected: o.selected.clone(),
            sig: self.sig,
        };
        if self.stamp.as_ref() != Some(&stamp) || self.offsets.len() != s.turns.len() + 1 {
            // Something moved: every turn's key is checked, a u32 or two
            // each, and the turns that changed are drawn again.
            self.offsets.clear();
            self.clocked.clear();
            let mut at = lead;
            for i in 0..s.turns.len() {
                self.offsets.push(at);
                at += self.height(s, o, i, latest, &folds);
                if self.slots[i]
                    .key
                    .as_ref()
                    .is_some_and(|k| k.clock.is_some())
                {
                    self.clocked.push(i);
                }
            }
            self.offsets.push(at);
            self.stamp = Some(stamp);
        } else {
            // Only a clock moved: the running turns are all that can redraw.
            let clocked = std::mem::take(&mut self.clocked);
            for &i in &clocked {
                let before = self.slots[i].height;
                let h = self.height(s, o, i, latest, &folds);
                if h != before {
                    for j in i + 1..self.offsets.len() {
                        self.offsets[j] = self.offsets[j] + h - before;
                    }
                }
            }
            self.clocked = clocked;
        }
        let n = s.turns.len();
        if n == 0 {
            return self
                .partial
                .iter()
                .filter(|_| top == 0 && rows > 0)
                .collect();
        }
        // The turns in view, by their first rows.
        let lo = self.offsets[..n]
            .partition_point(|&o| o <= top)
            .saturating_sub(1);
        let hi = (self.offsets[..n].partition_point(|&o| o < top + rows))
            .saturating_sub(1)
            .max(lo);
        self.forget_outside(lo, hi);
        for i in lo..=hi {
            self.turn(s, o, i, latest, &folds);
        }
        let mut out = Vec::with_capacity(rows);
        if lead == 1 && top == 0 {
            out.push(self.partial.as_ref().unwrap());
        }
        for i in lo..=hi {
            let from = self.offsets[i];
            for (j, l) in self.slots[i].lines.as_ref().unwrap().iter().enumerate() {
                let row = from + j;
                if row >= top && row < top + rows {
                    out.push(l);
                }
            }
        }
        out
    }

    /// The last rows of the conversation, from_bottom rows up from its
    /// end: what a view following the conversation shows. Only the turns
    /// at the end are drawn, however long it is.
    pub fn render_tail(
        &mut self,
        s: &Session,
        o: &Options,
        from_bottom: usize,
        rows: usize,
    ) -> Vec<&RenderedLine> {
        self.prepare(s, o);
        self.stamp = None; // the offsets aren't kept here
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let want = from_bottom + rows;
        let mut got = 0;
        let mut lo = s.turns.len();
        while lo > 0 && got < want {
            lo -= 1;
            got += self.height(s, o, lo, latest, &folds);
        }
        let hi = s.turns.len().saturating_sub(1);
        self.forget_outside(lo, hi);
        let mut all: Vec<&RenderedLine> = Vec::with_capacity(got + 1);
        for i in lo..s.turns.len() {
            self.turn(s, o, i, latest, &folds);
        }
        if lo == 0
            && let Some(p) = &self.partial
        {
            all.push(p);
        }
        for i in lo..s.turns.len() {
            all.extend(self.slots[i].lines.as_ref().unwrap().iter());
        }
        let end = all.len().saturating_sub(from_bottom);
        let start = end.saturating_sub(rows);
        all.drain(..start);
        all.truncate(end - start);
        all
    }

    /// The whole conversation's height in rows, laying out every turn not
    /// yet measured.
    pub fn total_height(&mut self, s: &Session, o: &Options) -> usize {
        self.prepare(s, o);
        self.stamp = None; // the offsets aren't kept here
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let mut n = usize::from(self.partial.is_some());
        for i in 0..s.turns.len() {
            n += self.height(s, o, i, latest, &folds);
        }
        // Measuring keeps no rows.
        for slot in &mut self.slots {
            slot.lines = None;
        }
        n
    }
}

/// What the whole layout reads, apart from clocks.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    applied: u64,
    turns: usize,
    latest: Option<(StepId, usize)>,
    folds: u64,
    selected: String,
    sig: u64,
}

/// The fold and view overrides grouped by the turn they're in.
struct Folds(HashMap<String, String>);

impl Folds {
    fn new(o: &Options) -> Folds {
        if o.open.is_empty() && o.view.is_empty() {
            return Folds(HashMap::new());
        }
        let mut by: HashMap<String, Vec<String>> = HashMap::new();
        for (k, v) in &o.open {
            let t = k.split(':').next().unwrap_or("");
            by.entry(t.to_string())
                .or_default()
                .push(format!("{k}={v}"));
        }
        for (k, v) in &o.view {
            let t = k.split(':').next().unwrap_or("");
            by.entry(t.to_string())
                .or_default()
                .push(format!("{k}~{v}"));
        }
        Folds(
            by.into_iter()
                .map(|(t, mut fs)| {
                    fs.sort();
                    (t, fs.join(","))
                })
                .collect(),
        )
    }

    fn sig(&self) -> u64 {
        let mut v: Vec<_> = self.0.iter().collect();
        v.sort();
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    fn of(&self, r: &str) -> String {
        self.0.get(r).cloned().unwrap_or_default()
    }
}

/// The newest step drawn at the top level, which shows opened until a
/// newer one takes its place, and the index of its turn.
fn latest_at(s: &Session) -> Option<(StepId, usize)> {
    for (i, t) in s.turns.iter().enumerate().rev() {
        for it in t.items.iter().rev() {
            if let Item::Step(id) = it
                && !hidden(s.step(*id))
            {
                return Some((*id, i));
            }
        }
    }
    None
}

/// Draws turn i: open, folded by hand, or as the story of a turn gone past.
fn draw_turn(
    s: &Session,
    o: &Options,
    p: &Places,
    i: usize,
    latest: Option<StepId>,
    recent: bool,
) -> (Vec<RenderedLine>, bool) {
    let t = &s.turns[i];
    let mut d = Drawer::new(s, t, o, p);
    d.latest = latest;
    d.first = i == 0;
    let r = d.r.clone();
    let mut open = recent || t.live || !t.err.is_empty() || waiting(s, t);
    if let Some(v) = o.open.get(&r) {
        open = *v;
    }
    if open {
        d.open();
    } else if o.open.contains_key(&r) {
        d.folded();
    } else {
        d.settled();
    }
    squeezed(&mut d.lines);
    (std::mem::take(&mut d.lines), d.fast)
}

/// Every row as plain text, a row a line as `ref|text`, the text's
/// trailing spaces trimmed: what the parity tests compare with Go's dump.
pub fn plain(lines: &[RenderedLine]) -> String {
    let mut out = String::new();
    for l in lines {
        out.push_str(&format!(
            "{:<14}|{}\n",
            l.r#ref,
            l.line.plain().trim_end_matches(' ')
        ));
    }
    out
}
