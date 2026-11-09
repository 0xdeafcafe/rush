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
mod codedoc;
mod decisions;
mod diffbox;
mod draw;
mod markdown;
mod needs;
mod opening;
pub(crate) mod outcome;
mod scriptdoc;
mod shell;
mod spawn;
mod stepout;
pub(crate) mod style;
mod tools;
pub(crate) mod txt;
mod udiff;
mod units;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use rush_core::text::Line;
use rush_core::time::Ms;
use rush_core::tool::Kind;

pub use cards::ENTITY_REF;
pub use code::{set_show_whitespace, show_whitespace};
pub use codedoc::{code_rows, is_code};
pub use decisions::{
    decisions_gone, decisions_on_card, decisions_ref, decisions_sent, open_decisions,
};
pub use draw::presets::check_names;
pub use draw::{OWNER_SEP, each_paste, image_label, paste_chip};
pub use draw::{drawing, fold_pastes, live_head};
pub use markdown::{answer_lines, link_target, markdown_lines};
pub use needs::{
    NEEDS_ALL, NEEDS_KINDS, Needs, NeedsItem, has_quote, needs_of, needs_ref, without_chrome,
};
pub use scriptdoc::{ScriptView, script_line, script_line_ref};
pub use spawn::{Child, Spawn, agent_run, asked, spawn_of, tool_run, unquote_arg};
pub use style::{SHOW_ALL_REF, dur, key_word, money};
pub use udiff::clean_output;

// The Needs-you section's replies as choices.
pub use needs::{NEEDS_OWN, needs_choices};

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

/// One line of code in the language of file name over base, going on from
/// st and leaving st for the next line: a diff's lines in the UI's views
/// (highlight.go:56 langFor, :68 highlight).
pub fn code_line(
    name: &str,
    st: &mut rush_hl::State,
    s: &str,
    base: rush_core::text::Style,
) -> Line {
    code::highlight(code::lang_for(name), st, s, base, None).into_line()
}

/// A line of a diff in the language of file name over base, its spaces
/// and tabs marked as · and →, cut to w cells with a › (render.go:4115
/// diffText, when showSpace is on).
pub fn marked_code_line(
    name: &str,
    st: &mut rush_hl::State,
    s: &str,
    base: rush_core::text::Style,
    w: usize,
) -> Line {
    let t = code::paint_code(code::lang_for(name), st, s, base, None, true);
    let w = w.max(4);
    if t.width() <= w {
        return t.into_line();
    }
    t.take(w - 1).and(txt::Txt::styled("›", base)).into_line()
}

use crate::model::{Item, Session, StepId};
pub use draw::diffbox::SIDE_BY_SIDE;
use draw::{Drawer, message_tool, squeezed, waiting};
pub(crate) use draw::{glyph_for, hidden};
pub use shell::Places;
pub(crate) use shell::clean as clean_path;
pub(crate) use shell::{Segment, fields_of, run_text, segments, shell_lines, unquote};
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

/// Which way the conversation is shown, one for every session (the
/// config's convoView), each answering one question: Normal as it always
/// was; Chat "what did we say?" (bubbles, yours right, the agent's left);
/// Code "what did it change?" (each turn's diffs and checks); CatchUp
/// "what happened while I was away?" (one card since you last looked);
/// Trail "where did the time go?" (a line a step on a time axis). Each
/// draws the same parsed turns, a turn at a time, so a long conversation
/// costs what a screen of it does whichever is picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Preset {
    #[default]
    Normal,
    Chat,
    Code,
    CatchUp,
    Trail,
}

impl Preset {
    /// Every one, in the order the key goes through them.
    pub const ALL: [Preset; 5] = [
        Preset::Normal,
        Preset::Chat,
        Preset::Code,
        Preset::CatchUp,
        Preset::Trail,
    ];

    /// Its name as the config keeps it.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Normal => "normal",
            Preset::Chat => "chat",
            Preset::Code => "code",
            Preset::CatchUp => "catch-up",
            Preset::Trail => "trail",
        }
    }

    /// Its name as the screen shows it.
    pub fn title(self) -> &'static str {
        match self {
            Preset::Normal => "Normal",
            Preset::Chat => "Chat",
            Preset::Code => "Code",
            Preset::CatchUp => "Catch-up",
            Preset::Trail => "Trail",
        }
    }

    /// What it shows, in a few words.
    pub fn about(self) -> &'static str {
        match self {
            Preset::Normal => "everything, as usual",
            Preset::Chat => "what did we say?",
            Preset::Code => "what did it change?",
            Preset::CatchUp => "what happened while I was away?",
            Preset::Trail => "where did the time go?",
        }
    }

    /// The one named s; Normal for "" or a name it doesn't know.
    pub fn from_name(s: &str) -> Preset {
        Preset::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or_default()
    }

    /// The one after it, back to Normal after the last.
    pub fn next(self) -> Preset {
        let i = Preset::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Preset::ALL[(i + 1) % Preset::ALL.len()]
    }
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
    /// Which way the conversation is shown.
    pub preset: Preset,
    /// When you last looked at this session before now (0: never, as far
    /// as rush knows): the Catch-up view's card covers what came after.
    pub since: Ms,
    pub selected: String,
    pub focused: bool,
    /// Rows may run the whole width, past CAP_ROW.
    pub wide: bool,
    /// Every turn drawn open, not only the latest.
    pub history_open: bool,
    /// The live line is drawn elsewhere.
    pub hide_activity: bool,
    /// The live line is the pane's docked one (activity.go:13): a running
    /// or waiting step is named on it, with the turn's time, as its own
    /// row may be folded or scrolled away.
    pub dock_activity: bool,
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
    /// The local zone's offset at an instant, when it has one: a time
    /// from before the clocks changed is shown as it was then, as Go's
    /// time.Local does. None: always `zone`.
    pub zone_at: Option<fn(Ms) -> i64>,
    /// What the last message asks of you: the last turn ends with it.
    pub needs: Needs,
    /// The agent each shell step ran, by the step's id, once it's found:
    /// the step is drawn as that agent's row (SetChildren's st.run,
    /// spawn.go:574). Set by whoever finds them.
    pub ran: HashMap<String, Spawn>,
    /// The agents each shell step ran, found, by the step's id: one is the
    /// step's own row with its steps and reply under it, several a row
    /// each under it (SetChildren, spawn.go:573). Set by whoever finds
    /// them.
    pub kids: HashMap<String, Vec<Child>>,
    /// The session this one works for as a subagent, and the one that
    /// started it beside it (start_agent), by name, for the card it opens
    /// on (render.go:112-117).
    pub parent: String,
    pub started_from: String,
    /// Emeritus: the session this one took over from as it retired, by
    /// name, for the card it opens on; "" when it took over from none.
    pub predecessor: String,
    /// How many handovers led to it, and whether that's the cap on them
    /// in a row (it isn't asked to hand over again).
    pub handovers: u32,
    pub capped: bool,
    /// rush runs the session, and so added its own prompt to the agent's;
    /// brief is what the session's start added after it.
    pub hosted: bool,
    pub brief: String,
    /// The Bash calls, by tool call id, whose running command rush has
    /// paused: their rows say so (render.go:129 Paused).
    pub paused: std::collections::HashSet<String>,
    /// The Bash calls, by tool call id, rush ran as scripts: drawn as the
    /// file they ran from (render.go:132 Scripts).
    pub scripts: HashMap<String, ScriptView>,
    /// What a running Bash call is doing now, by tool call id, found from
    /// its processes and the files it writes: one dim line under its
    /// command while it runs. Set by whoever finds it.
    pub live: HashMap<String, StepLive>,
    /// The commands rush ran itself, by their box's ref: a shell block of
    /// a reply run (`t3:sh:<item>:<k>`) or a command typed after `!`
    /// (`t3:bang:<start>`, drawn after the items its turn had then). Set
    /// by whoever runs them (shellbox.rs).
    pub shells: HashMap<String, std::sync::Arc<crate::shellbox::ShellRun>>,
    /// Where those commands run: the session's folder as the agent list
    /// has it ("" for the conversation's own).
    pub shell_cwd: String,
    /// A box whose commands are being edited before they run, and the
    /// text as it stands (shellbox.rs).
    pub shell_edit: Option<crate::shellbox::ShellEdit>,
    /// Background tasks rush can say more of than that they run, by task
    /// id: a row drawn in place of the call's "running in the background"
    /// note while the task runs (a rush watch's card: what it waits on,
    /// how far it's got, the time left). Set by whoever reads them.
    /// Rust-only.
    pub tasks: HashMap<String, Line>,
    /// The runs a harness's own subagent calls started in the background,
    /// by the call's id: a row (its mark, what it was asked, how it
    /// stands, "open ›") drawn in place of the boilerplate the call
    /// returned ("Async agent launched… output_file… Do NOT Read…").
    /// Set by whoever reads the runs. Rust-only.
    pub runs: HashMap<String, Line>,
}

/// What a running shell step is doing now: a line from its processes, and
/// the last lines of a file its command writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepLive {
    pub now: String,
    pub tail: Vec<String>,
    /// What it has printed so far, read from the file its shell writes
    /// to, for a harness that doesn't stream it (Claude Code's): its last
    /// lines are drawn under it (liveout.rs).
    pub out: Option<std::sync::Arc<crate::model::LiveOut>>,
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

    /// Seconds east of UTC at t: what a clock time of t is shown in.
    pub fn offset(&self, t: Ms) -> i64 {
        self.zone_at.map_or(self.zone, |f| f(t))
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
            self.zone_at,
        )
            .hash(&mut h);
        (self.dock_activity, code::show_whitespace(), self.preset).hash(&mut h);
        self.shell_cwd.hash(&mut h);
        if self.preset == Preset::CatchUp {
            self.since.hash(&mut h);
        }
        let mut ran: Vec<_> = self.ran.iter().collect();
        ran.sort();
        ran.hash(&mut h);
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
    /// What the first turn's opening card reads (openingPrint).
    opening: String,
    /// What the agents its steps ran have done.
    kids: u64,
    /// How many calls are paused, while it runs (render.go:407).
    paused: usize,
    /// What of its scripts shows (scriptsSig, render.go:392).
    scripts: u64,
    /// What another view draws it from besides the turn itself: in
    /// Catch-up, whether the card covers it, and the background tasks the
    /// last turn's card says are still going.
    view: u64,
    /// The commands rush ran itself in it, as they stand (shellbox.rs).
    shells: u64,
    /// The rows of its calls' background tasks, as they stand (tasks).
    tasks: u64,
    /// The rows of its subagent calls' runs, as they stand (runs).
    runs: u64,
}

/// What turn t's subagent calls' runs draw as now: their rows in o.runs,
/// for the turn's key.
fn runs_sig(s: &Session, t: &crate::model::Turn, o: &Options) -> u64 {
    let mut h = DefaultHasher::new();
    for it in &t.items {
        let Item::Step(id) = it else { continue };
        let st = s.step(*id);
        let Some(row) = o.runs.get(&*st.id) else {
            continue;
        };
        st.id.hash(&mut h);
        for sp in &row.spans {
            (sp.text.as_str(), sp.style).hash(&mut h);
        }
    }
    h.finish()
}

/// What turn t's calls' background tasks draw as now: their rows in
/// o.tasks, for the turn's key.
fn tasks_sig(s: &Session, t: &crate::model::Turn, o: &Options) -> u64 {
    let mut h = DefaultHasher::new();
    for j in s.running_jobs() {
        let Some(row) = o.tasks.get(&j.id) else {
            continue;
        };
        let ours = s.step_by_call(&j.tool_use_id).is_some_and(|id| {
            t.items
                .iter()
                .any(|it| matches!(it, Item::Step(x) if *x == id))
        });
        if ours {
            j.id.hash(&mut h);
            for sp in &row.spans {
                (sp.text.as_str(), sp.style).hash(&mut h);
            }
        }
    }
    h.finish()
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
    /// Its parts as last drawn, while it's drawn open (the unit memo).
    units: units::Units,
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
    /// The commands you ran after `!` before the session had a turn, as
    /// their boxes, above the first turn (or alone); and what they were
    /// drawn from.
    pre: Vec<RenderedLine>,
    pre_key: Option<u64>,
    fast: bool,
    /// The turns the last window drew, [lo, hi).
    lo: usize,
    hi: usize,
    /// Windows drawn so far; rows kept.
    windows: u64,
    kept: usize,
    /// The first turn the Catch-up card covers, as last counted.
    catch: usize,
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
    /// Only what draws (draws) starts over for a link's lookup landing:
    /// counting keeps the rows last drawn, so a caller that draws a window
    /// and then counts reads the rows it drew, whatever lands between.
    fn prepare(&mut self, s: &Session, o: &Options, draws: bool) {
        let sig = o.sig();
        let places = self.places_for != s.cwd || self.places.bases.is_empty() && !s.cwd.is_empty();
        if places {
            self.places = Places::new(&s.cwd);
            self.places_for = s.cwd.clone();
        }
        let links = if draws {
            crate::linkinfo::lookups_gen()
        } else {
            self.links
        };
        // The turns the catch-up card covers draw nothing; when that
        // changes, every turn is counted again.
        let catch = if o.preset == Preset::CatchUp {
            draw::presets::catch_from(s, o.since).0
        } else {
            0
        };
        if sig != self.sig
            || links != self.links
            || s.serial() != self.serial
            || s.turns.len() < self.slots.len()
            || places
            || catch != self.catch
        {
            self.catch = catch;
            self.clear();
            self.sig = sig;
            self.links = links;
            self.serial = s.serial();
        }
        let was = self.slots.len();
        for i in was..s.turns.len() {
            let est = estimate(s, o, i, self.catch);
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
        let pre_key = unturned_key(s, o, sig);
        if pre_key != self.pre_key {
            self.pre = match pre_key {
                Some(_) => draw_unturned(s, o, &self.places),
                None => Vec::new(),
            };
            self.pre_key = pre_key;
        }
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
        // Another view draws each turn one way, whatever its folds.
        if o.preset != Preset::Normal {
            return slot.est.0;
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

    /// The rows above the first turn: "reading earlier…", and the
    /// commands you ran before there was one.
    fn hdr(&self) -> usize {
        usize::from(self.partial.is_some()) + self.pre.len()
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
        // A call of a turn gone past whose command runs on in the
        // background says which of its commands runs, and for how long.
        let runs_on = || {
            s.running_jobs().any(|j| {
                s.step_by_call(&j.tool_use_id).is_some_and(|id| {
                    !s.running_label(id).is_empty()
                        && t.items
                            .iter()
                            .any(|it| matches!(it, Item::Step(x) if *x == id))
                })
            })
        };
        if t.live || waiting(s, t) || runs_on() {
            let fast = self.slots.get(i).is_some_and(|x| x.fast);
            k.clock = Some((o.tick, if fast { o.now / 100 } else { o.now / 1000 }));
            k.paused = o.paused.len();
        }
        if i + 1 == s.turns.len() {
            k.needs = o.needs.key();
            // A row of the Needs-you section picked lights its key.
            if o.selected.starts_with("needs:") {
                k.sel = o.selected.clone();
            }
        }
        if i == 0 {
            k.opening = opening::opening_print(s, o);
        }
        if !o.scripts.is_empty() {
            let ids = t.items.iter().filter_map(|it| match it {
                Item::Step(id) => Some(&*s.step(*id).id),
                _ => None,
            });
            k.scripts = scriptdoc::scripts_sig(&o.scripts, ids);
        }
        if !o.shells.is_empty() || o.shell_edit.is_some() {
            k.shells = draw::shellbox::turn_sig(o, &format!("t{}:", t.n));
        }
        if !o.tasks.is_empty() {
            k.tasks = tasks_sig(s, t, o);
        }
        if !o.runs.is_empty() {
            k.runs = runs_sig(s, t, o);
        }
        if o.preset == Preset::CatchUp {
            let last = i + 1 == s.turns.len();
            let mut h = DefaultHasher::new();
            (i >= self.catch, last).hash(&mut h);
            if last {
                let mut going = false;
                for j in s.running_jobs().filter(|j| j.background) {
                    (&j.id, &j.label).hash(&mut h);
                    going = true;
                }
                if going {
                    (o.now / 1000).hash(&mut h);
                }
                // The commands you ran, any turn's: the card names them,
                // and opens the one picked.
                if !o.shells.is_empty() {
                    draw::shellbox::turn_sig(o, "t").hash(&mut h);
                    if crate::shellbox::BoxRef::parse(&o.selected).is_some() {
                        o.selected.hash(&mut h);
                    }
                    let mut opened: Vec<_> = o
                        .open
                        .iter()
                        .filter(|(k, _)| crate::shellbox::BoxRef::parse(k).is_some())
                        .collect();
                    opened.sort();
                    opened.hash(&mut h);
                }
            }
            k.view = h.finish();
        }
        // The agents its steps ran, as they work.
        if !o.kids.is_empty() {
            let mut h = DefaultHasher::new();
            for it in &t.items {
                if let Item::Step(id) = it
                    && let Some(ks) = o.kids.get(&*s.step(*id).id)
                {
                    for c in ks {
                        c.print().hash(&mut h);
                    }
                }
            }
            // An agent_result or agent_send call names the agent it's
            // about, and says what it's doing, wherever it was started.
            let about: Vec<String> = t
                .items
                .iter()
                .filter_map(|it| match it {
                    Item::Step(id) => crate::model::runs::agent_id(s.step(*id)),
                    _ => None,
                })
                .collect();
            if !about.is_empty() {
                let mut all: Vec<&Child> = o
                    .kids
                    .values()
                    .flatten()
                    .filter(|c| about.contains(&c.id))
                    .collect();
                all.sort_by(|a, b| a.id.cmp(&b.id));
                for c in all {
                    c.print().hash(&mut h);
                }
            }
            k.kids = h.finish();
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
            let (lines, fast) = draw_turn(
                s,
                o,
                &self.places,
                i,
                k.latest,
                k.recent,
                &mut self.slots[i].units,
            );
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
                self.slots[i].units.clear();
                self.kept -= ls.len();
            }
        }
    }

    /// Every row, oldest first: for a dump, or a caller that wants all of
    /// it. Every turn is drawn; at most KEEP_ROWS rows are kept after.
    pub fn render(&mut self, s: &Session, o: &Options) -> Vec<RenderedLine> {
        self.prepare(s, o, true);
        self.windows += 1;
        let latest = latest_at(s);
        let folds = Folds::new(o);
        let mut out = Vec::new();
        if let Some(p) = &self.partial {
            out.push(p.clone());
        }
        out.extend(self.pre.iter().cloned());
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
        self.prepare(s, o, false);
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
        self.prepare(s, o, true);
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
        // At the top: the window starts there, or there are no turns.
        if self.lo == 0 && (self.hi > 0 || self.slots.is_empty()) {
            if let Some(p) = &self.partial {
                out.push(p);
            }
            out.extend(self.pre.iter());
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

    /// The row turn i's own rows begin at: unlike turn_row, the first
    /// turn's come after the rows above it ("reading earlier…", what
    /// comes before the first turn), which a view at row 0 shows with it.
    pub fn turn_top(&self, i: usize) -> usize {
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
        self.prepare(s, o, false);
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
                    self.slots[i].units.clear();
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
fn estimate(s: &Session, o: &Options, i: usize, catch: usize) -> (usize, usize) {
    if o.preset != Preset::Normal {
        let n = presets_estimate(s, o, i, catch);
        return (n, n);
    }
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

/// How many rows turn i takes in another view, before it's drawn: Chat
/// its bubbles, Code a heading and each file's few rows, Catch-up a row
/// for a turn before the card and the card on the last, Trail a row a
/// step.
fn presets_estimate(s: &Session, o: &Options, i: usize, catch: usize) -> usize {
    let t = &s.turns[i];
    let w = 20.max(o.width.min(o.prose_cap()) * 3 / 5).max(1);
    let lines = |x: &str| 1 + x.matches('\n').count() + x.len() / w;
    let steps = || {
        t.items
            .iter()
            .filter(|it| matches!(it, Item::Step(_)))
            .count()
    };
    match o.preset {
        Preset::CatchUp if i + 1 == s.turns.len() => 9,
        Preset::CatchUp if i >= catch => 0,
        Preset::CatchUp => 1,
        Preset::Trail => 4 + steps(),
        Preset::Code => {
            let edits = t
                .steps
                .iter()
                .filter(|id| {
                    matches!(
                        s.step(**id).kind,
                        Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move
                    )
                })
                .count();
            let shells = t
                .items
                .iter()
                .filter(|it| matches!(it, Item::Step(id) if s.step(*id).kind == Kind::Shell))
                .count();
            2 + edits * 8 + shells / 2
        }
        _ => {
            let mut n = 4 + lines(&t.prompt);
            let mut said = 0;
            for it in &t.items {
                if let Item::Text { text, .. } = it {
                    said += lines(text) + 1;
                }
            }
            if said > 0 || !t.items.is_empty() {
                n += 2 + said;
            }
            n
        }
    }
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

/// A task's launch script, laid out as an opened shell step's command is,
/// indent cells in; none when its call isn't in the conversation
/// (chainrun.go:368, JobLines).
pub fn job_lines(s: &Session, j: &crate::Job, o: &Options, indent: usize) -> Vec<RenderedLine> {
    let cmd = s.job_command(j);
    let Some(id) = s.step_by_call(&j.tool_use_id).filter(|_| !cmd.is_empty()) else {
        return Vec::new();
    };
    let t = crate::model::Turn::default();
    let p = Places::new(&s.cwd);
    let mut d = Drawer::new(s, &t, o, &p);
    d.shell_body(Some(id), &cmd, indent);
    d.lines
}

/// The view after cur among those step ref's output can be seen in, or ""
/// when it has no other; cur "" is the one it opened in: v on a picked
/// step (views.go:33 NextView).
pub fn next_view(s: &Session, r: &str, cur: &str) -> String {
    let Some(id) = r.split_once(":s:").and_then(|(_, id)| s.step_by_call(id)) else {
        return String::new();
    };
    let (t, o) = (crate::model::Turn::default(), Options::default());
    let p = Places::new(&s.cwd);
    let (vs, auto) = Drawer::new(s, &t, &o, &p).views(id);
    if vs.len() < 2 {
        return String::new();
    }
    let cur = if cur.is_empty() { auto } else { cur };
    match vs.iter().position(|v| *v == cur) {
        Some(i) => vs[(i + 1) % vs.len()].to_string(),
        None => vs[0].to_string(),
    }
}

/// The Options.open key that keeps each line of step ref's output to one
/// row, cut with ›, instead of wrapping it (views.go:104 NoWrapRef).
pub fn no_wrap_ref(r: &str) -> String {
    format!("{r}:nowrap")
}

/// Draws turn i: open, folded by hand, or as the story of a turn gone past.
fn draw_turn(
    s: &Session,
    o: &Options,
    p: &Places,
    i: usize,
    latest: Option<StepId>,
    recent: bool,
    units: &mut units::Units,
) -> (Vec<RenderedLine>, bool) {
    let t = &s.turns[i];
    let mut d = Drawer::new(s, t, o, p);
    d.latest = latest;
    d.first = i == 0;
    let r = d.r.clone();
    if o.preset != Preset::Normal {
        d.ti = i;
        d.preset(o.preset);
        squeezed(&mut d.lines);
        units.clear();
        return (std::mem::take(&mut d.lines), d.fast);
    }
    let mut open = recent || t.live || !t.err.is_empty() || waiting(s, t);
    if let Some(v) = o.open.get(&r) {
        open = *v;
    }
    if open {
        // Drawn part by part through the unit memo: a turn on the clock
        // redraws every frame, and only what runs in it is drawn anew
        // (render.go:345).
        d.ctx = Some(d.unit_ctx());
        d.units = Some(units);
        d.open();
    } else if o.open.contains_key(&r) {
        units.clear();
        d.folded();
    } else {
        units.clear();
        d.settled();
    }
    squeezed(&mut d.lines);
    (std::mem::take(&mut d.lines), d.fast)
}

/// What the commands you ran before the session had a turn are drawn
/// from (`t0:bang:<start>`); None when there are none.
fn unturned_key(s: &Session, o: &Options, sig: u64) -> Option<u64> {
    if !crate::shellbox::unturned(&o.shells) {
        return None;
    }
    let mut h = DefaultHasher::new();
    (sig, draw::shellbox::turn_sig(o, "t0:"), &s.cwd).hash(&mut h);
    if o.selected.starts_with("t0:") {
        o.selected.hash(&mut h);
    }
    let mut opened: Vec<_> = o
        .open
        .iter()
        .filter(|(k, _)| k.starts_with("t0:"))
        .collect();
    opened.sort();
    opened.hash(&mut h);
    Some(h.finish())
}

/// The boxes of the commands you ran before the session had a turn, in
/// the order they ran: drawn alike in every view.
fn draw_unturned(s: &Session, o: &Options, p: &Places) -> Vec<RenderedLine> {
    let t = crate::model::Turn::default();
    let mut d = Drawer::new(s, &t, o, p);
    for (k, run) in d.take_bangs(usize::MAX) {
        d.gap();
        d.run_box(&k, run);
        d.blank();
    }
    std::mem::take(&mut d.lines)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A link's lookup landing between drawing a window and counting the
    /// rows (a frame's body does both) leaves the rows drawn: only the
    /// next window draws again. Counting used to let them go, so a click
    /// or a drag read no rows until the next frame.
    #[test]
    fn counting_keeps_the_rows_drawn() {
        let mut s = Session::new();
        for i in 0..3 {
            s.sent(format!("question {i}"), Vec::new(), 1_000 * i);
        }
        let o = Options {
            width: 80,
            now: 10_000,
            ..Options::default()
        };
        let mut r = Renderer::new();
        let total = r.count(&s, &o);
        r.window(&s, &o, 0, total);
        let drawn: Vec<String> = r.rows().iter().map(|l| l.line.plain()).collect();
        assert!(!drawn.is_empty());
        let counted = r.count(&s, &o);
        crate::linkinfo::redraw();
        assert_eq!(r.count(&s, &o), counted);
        let after: Vec<String> = r.rows().iter().map(|l| l.line.plain()).collect();
        assert_eq!(after, drawn, "the rows drawn stay till the next window");
        r.total_height(&s, &o);
        assert!(!r.rows().is_empty());
        // The next window draws them again.
        r.window(&s, &o, 0, total);
        assert_eq!(r.links, crate::linkinfo::lookups_gen());
    }
}
