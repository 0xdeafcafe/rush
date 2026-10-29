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
mod needs;
mod shell;
mod spawn;
pub(crate) mod style;
pub(crate) mod txt;
mod udiff;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use rush_core::text::Line;
use rush_core::time::Ms;

pub use cards::ENTITY_REF;
pub use decisions::{decisions_on_card, decisions_ref, decisions_sent};
pub use draw::fold_pastes;
pub use draw::{OWNER_SEP, each_paste, image_label, paste_chip};
pub use markdown::{answer_lines, markdown_lines};
pub use needs::{NEEDS_KINDS, Needs, NeedsItem, has_quote, needs_of, needs_ref};
pub use spawn::{Spawn, spawn_of, unquote_arg};
pub use style::{SHOW_ALL_REF, dur, key_word, money};

/// s with its inline markdown drawn (**bold**, `code`, links) over base,
/// as one line: a run with no colour of its own takes base's (Go's
/// convo.Inline, render.go:1881, for the UI's cards).
pub fn inline(s: &str, base: rush_core::text::Style) -> Line {
    let t = markdown::inline(s, base);
    Line::from_spans(
        t.0.into_iter()
            .map(|mut sp| {
                if sp.style.fg == rush_core::text::Color::Default {
                    sp.style.fg = base.fg;
                    sp.style.mods = sp.style.mods.with(base.mods);
                }
                sp
            })
            .collect(),
    )
}

use crate::model::{Item, Session, StepId};
use draw::{Drawer, message_tool, squeezed, waiting};
pub(crate) use draw::{glyph_for, hidden};
use shell::Places;
pub(crate) use shell::clean as clean_path;
pub(crate) use style::one_line;

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
    /// The session's own provider, which a subagent run in another
    /// harness is named against (render.go:123).
    pub kind: String,
    /// Seconds east of UTC that clock times are shown in.
    pub zone: i64,
    /// What the last message asks of you: the last turn ends with it.
    pub needs: Needs,
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
            &self.kind,
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
    /// What the last turn's answer is followed by: Needs::key.
    needs: String,
}

/// How many rows of drawn turns are kept for drawing again cheaply: several
/// screens either side of any window, never the whole of a long session
/// (Go's keepRows, window.go:17).
pub const KEEP_ROWS: usize = 20_000;

#[derive(Default)]
struct Slot {
    key: Option<TurnKey>,
    /// Its rows as last drawn at this layout; None until drawn.
    height: Option<usize>,
    /// Its rows estimated, open and gone past, for before it's drawn.
    est: (usize, usize),
    lines: Option<Vec<RenderedLine>>,
    fast: bool,
    /// The window that last drew or showed it, for letting go of the
    /// longest unused first.
    used: u64,
}

/// Draws a Session a window at a time (Go's RenderWindow, window.go): only
/// the turns holding the rows asked for are drawn; the rest are counted,
/// as last drawn or at an estimate until they are, in an index that finds
/// a turn's first row and the turn at a row by halving. At most KEEP_ROWS
/// rows of drawn turns are kept, the longest unused let go first.
#[derive(Default)]
pub struct Renderer {
    slots: Vec<Slot>,
    sig: u64,
    /// The links' lookups as last drawn (linkinfo::lookups_gen): an answer
    /// landing draws every turn again (Go's unitKey.gen).
    links: u64,
    serial: u64,
    places: Places,
    places_for: String,
    index: RowIndex,
    partial: Option<RenderedLine>,
    fast: bool,
    /// The turns the last window drew, [lo, hi).
    lo: usize,
    hi: usize,
    /// Windows drawn so far; rows kept.
    windows: u64,
    kept: usize,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// Whether the last frame drew a timer showing tenths: the caller draws
    /// again in a tenth of a second rather than a second.
    pub fn fast(&self) -> bool {
        self.fast
    }

    /// Forgets every turn as drawn.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.index = RowIndex::default();
        self.kept = 0;
        self.lo = 0;
        self.hi = 0;
    }

    /// Rows in memory, for measuring: those kept for turns drawn lately.
    pub fn kept_rows(&self) -> usize {
        self.kept
    }

    /// Counts the session's turns as they are: every turn again when the
    /// layout or the session changed, a new turn at its estimate.
    fn prepare(&mut self, s: &Session, o: &Options) {
        let sig = o.sig();
        let places = self.places_for != s.cwd || self.places.bases.is_empty() && !s.cwd.is_empty();
        if places {
            self.places = Places::new(&s.cwd);
            self.places_for = s.cwd.clone();
        }
        let links = crate::linkinfo::lookups_gen();
        if sig != self.sig
            || links != self.links
            || s.serial() != self.serial
            || s.turns.len() < self.slots.len()
            || places
        {
            self.clear();
            self.sig = sig;
            self.links = links;
            self.serial = s.serial();
        }
        let was = self.slots.len();
        for i in was..s.turns.len() {
            let est = estimate(s, o, i);
            self.slots.push(Slot {
                est,
                ..Slot::default()
            });
            let h = self.counted(s, o, i);
            self.index.push(h);
        }
        self.partial = s.partial.then(|| RenderedLine {
            line: style::dim("  reading earlier…").into_line(),
            ..RenderedLine::default()
        });
        self.fast = false;
    }

    /// Turn i's rows as counted before it's drawn: open at its estimate,
    /// folded by hand its prompt and a line of what it did, gone past as
    /// settled draws it.
    fn counted(&self, s: &Session, o: &Options, i: usize) -> usize {
        let slot = &self.slots[i];
        if let Some(h) = slot.height {
            return h;
        }
        let t = &s.turns[i];
        let r = format!("t{}", t.n);
        let mut open = o.history_open
            || i + 1 == s.turns.len()
            || t.live
            || !t.err.is_empty()
            || waiting(s, t);
        if let Some(v) = o.open.get(&r) {
            open = *v;
        }
        if open {
            slot.est.0
        } else if o.open.contains_key(&r) {
            3
        } else {
            slot.est.1
        }
    }

    /// The rows above the first turn: "reading earlier…".
    fn hdr(&self) -> usize {
        usize::from(self.partial.is_some())
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
        if i + 1 == s.turns.len() {
            k.needs = o.needs.key();
        }
        k
    }

    /// Turn i's rows, drawn afresh only when what it reads has changed;
    /// counted as drawn from then on.
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
            self.kept = self.kept + lines.len() - slot.lines.as_ref().map_or(0, Vec::len);
            if slot.height != Some(lines.len()) {
                self.index.set(i, lines.len());
            }
            slot.height = Some(lines.len());
            slot.key = Some(k);
            slot.fast = fast;
            slot.lines = Some(lines);
        }
        let slot = &mut self.slots[i];
        slot.used = self.windows;
        self.fast |= slot.fast;
        slot.lines.as_ref().unwrap()
    }

    /// Lets go of the rows of the turns used longest ago past KEEP_ROWS,
    /// never those of the last window.
    fn evict(&mut self) {
        if self.kept <= KEEP_ROWS {
            return;
        }
        let mut old: Vec<(u64, usize)> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(i, s)| s.lines.is_some() && (*i < self.lo || *i >= self.hi))
            .map(|(i, s)| (s.used, i))
            .collect();
        old.sort_unstable();
        for (_, i) in old {
            if self.kept <= KEEP_ROWS {
                break;
            }
            if let Some(ls) = self.slots[i].lines.take() {
                self.kept -= ls.len();
            }
        }
    }

    /// Every row, oldest first: for a dump, or a caller that wants all of
    /// it. Every turn is drawn; at most KEEP_ROWS rows are kept after.
    pub fn render(&mut self, s: &Session, o: &Options) -> Vec<RenderedLine> {
        self.prepare(s, o);
        self.windows += 1;
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let mut out = Vec::new();
        if let Some(p) = &self.partial {
            out.push(p.clone());
        }
        for i in 0..s.turns.len() {
            out.extend(self.turn(s, o, i, latest, &folds).iter().cloned());
        }
        (self.lo, self.hi) = (0, 0);
        self.evict();
        out
    }

    /// How many rows the whole session has, as it's counted now: each turn
    /// as last drawn, else at its estimate (Go's Count). Draws nothing.
    pub fn count(&mut self, s: &Session, o: &Options) -> usize {
        self.prepare(s, o);
        self.hdr() + self.index.total()
    }

    /// Draws the turns holding rows [from, to) as the session is counted
    /// now, whole, and only those: a long session costs what a screen of
    /// it does. A live or waiting last turn below them is drawn too, to
    /// count it as it grows. Returns the row the drawn turns start at and
    /// the session's rows counted after drawing them; [`Renderer::rows`]
    /// are those turns' rows. What was estimated is counted as drawn from
    /// then on, so a caller whose rows moved draws again (Go's ui does up
    /// to three passes).
    pub fn window(&mut self, s: &Session, o: &Options, from: usize, to: usize) -> (usize, usize) {
        self.prepare(s, o);
        self.windows += 1;
        let n = s.turns.len();
        let hdr = self.hdr();
        if n == 0 {
            (self.lo, self.hi) = (0, 0);
            return (0, hdr);
        }
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let lo = self.index.find(from.saturating_sub(hdr));
        let hi = self
            .index
            .find(from.max(to.saturating_sub(1)).saturating_sub(hdr))
            + 1;
        let last = n - 1;
        if last >= hi && (s.turns[last].live || waiting(s, &s.turns[last])) {
            self.turn(s, o, last, latest, &folds);
        }
        for i in lo..hi {
            self.turn(s, o, i, latest, &folds);
        }
        (self.lo, self.hi) = (lo, hi);
        self.evict();
        (self.turn_row(lo), hdr + self.index.total())
    }

    /// The rows of the turns the last window drew, from its base: "reading
    /// earlier…" first when they start at the top.
    pub fn rows(&self) -> Vec<&RenderedLine> {
        let mut out = Vec::new();
        if self.lo == 0
            && self.hi > 0
            && let Some(p) = &self.partial
        {
            out.push(p);
        }
        for s in &self.slots[self.lo..self.hi.min(self.slots.len())] {
            if let Some(ls) = &s.lines {
                out.extend(ls.iter());
            }
        }
        out
    }

    /// The turns the last window drew, [lo, hi).
    pub fn window_turns(&self) -> (usize, usize) {
        (self.lo, self.hi)
    }

    /// The row turn i begins at, as the session is counted now.
    pub fn turn_row(&self, i: usize) -> usize {
        if i == 0 {
            return 0;
        }
        self.hdr() + self.index.prefix(i)
    }

    /// The turn at row r, the nearest when r is off either end.
    pub fn turn_at(&self, r: usize) -> usize {
        self.index.find(r.saturating_sub(self.hdr()))
    }

    /// The session's last rows rows, as a view following it shows them.
    pub fn tail(&mut self, s: &Session, o: &Options, rows: usize) -> Vec<&RenderedLine> {
        let mut total = self.count(s, o);
        for _ in 0..3 {
            let (base, t) = self.window(s, o, total.saturating_sub(rows), total);
            let fits = base <= t.saturating_sub(rows);
            total = t;
            if fits {
                break;
            }
        }
        let all = self.rows();
        let start = all.len().saturating_sub(rows);
        all[start..].to_vec()
    }

    /// The whole conversation's height in rows, laying out every turn not
    /// yet drawn; it keeps their heights, not their rows.
    pub fn total_height(&mut self, s: &Session, o: &Options) -> usize {
        self.prepare(s, o);
        self.windows += 1;
        let latest = latest_at(s);
        let folds = Folds::new(o);
        for i in 0..s.turns.len() {
            let k = self.key(s, o, i, latest, &folds);
            if self.slots[i].key.as_ref() == Some(&k) && self.slots[i].height.is_some() {
                continue;
            }
            self.turn(s, o, i, latest, &folds);
            if i < self.lo || i >= self.hi {
                // Measuring keeps no rows.
                if let Some(ls) = self.slots[i].lines.take() {
                    self.kept -= ls.len();
                }
            }
        }
        self.hdr() + self.index.total()
    }
}

/// Each turn's rows, summed by halving (a Fenwick tree): a turn's first row
/// and the turn at a row in log time, however long the session.
#[derive(Default)]
struct RowIndex {
    h: Vec<usize>,
    /// 1-based: tree[i] sums h over (i - lowbit(i), i].
    tree: Vec<usize>,
}

impl RowIndex {
    fn len(&self) -> usize {
        self.h.len()
    }

    fn push(&mut self, v: usize) {
        let i = self.h.len() + 1;
        let low = i.isolate_lowest_one();
        // What tree[i] covers before i: h over (i - low, i - 1].
        let before = self.prefix(i - 1) - self.prefix(i - low);
        self.h.push(v);
        if self.tree.is_empty() {
            self.tree.push(0);
        }
        self.tree.push(before + v);
    }

    fn set(&mut self, i: usize, v: usize) {
        let old = self.h[i];
        if old == v {
            return;
        }
        self.h[i] = v;
        let mut j = i + 1;
        while j < self.tree.len() {
            self.tree[j] = self.tree[j] + v - old;
            j += j.isolate_lowest_one();
        }
    }

    /// The rows of turns [0, i).
    fn prefix(&self, i: usize) -> usize {
        let mut sum = 0;
        let mut j = i.min(self.h.len());
        while j > 0 {
            sum += self.tree[j];
            j -= j.isolate_lowest_one();
        }
        sum
    }

    fn total(&self) -> usize {
        self.prefix(self.h.len())
    }

    /// The turn holding row r: the last i with prefix(i) <= r, the last
    /// turn when r is past the end.
    fn find(&self, r: usize) -> usize {
        let n = self.len();
        if n == 0 {
            return 0;
        }
        let mut pos = 0;
        let mut left = r;
        let mut step = (n + 1).next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= left {
                pos = next;
                left -= self.tree[next];
            }
            step >>= 1;
        }
        // pos turns end at or before r; a zero-height run is skipped.
        pos.min(n - 1)
    }
}

/// How many rows turn i takes before it's drawn, open and gone past: its
/// heading, its words wrapped, and the gap after it; open, a row a step;
/// gone past, a row a run of plain steps and one for each step that stays
/// out of a run (Go's estimate, window.go).
fn estimate(s: &Session, o: &Options, i: usize) -> (usize, usize) {
    let t = &s.turns[i];
    let w = 20
        .max(o.width.min(o.prose_cap()).saturating_sub(GUTTER))
        .max(1);
    let lines = |x: &str| x.matches('\n').count() + x.len() / w;
    let n = 3 + lines(&t.prompt);
    let (mut open, mut past) = (n, n - 1);
    if !t.items.is_empty() {
        open += 2;
        past += 2;
    }
    if !t.steps.is_empty() {
        past += 1;
    }
    let mut run = false;
    for it in &t.items {
        match it {
            Item::Text { text, .. } => {
                let k = 2 + lines(text);
                open += k;
                past += k;
                run = false;
            }
            Item::Interject(x) => {
                let k = 2 + lines(&x.text);
                open += k;
                past += k;
                run = false;
            }
            Item::Thinking(_) => open += 1,
            Item::Step(id) => {
                open += 1;
                let st = s.step(*id);
                let g = glyph_for(st);
                if g == "⇉" || g == "◆" || g == "◇" || message_tool(&st.tool) {
                    past += 1;
                    run = false;
                } else if !run {
                    past += 1;
                    run = true;
                }
            }
            _ => {
                open += 1;
                past += 1;
            }
        }
    }
    (open, past)
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
