//! A session opened read-only: its header, the conversation as rush-convo
//! draws it, and the keys that scroll it and fold its rows. After Go's
//! internal/ui/rushmode.go (paneHeader :2721, the conversation's keys
//! :3954-4120, isOpen :4661, followTail :2117, the speaker :2400),
//! history.go:45 (openHistory) and panegeometry.go:13 (paneFrameRow).
//! The conversation is read and parsed on a worker (worker::spawn_read,
//! the same for every agent: its end first, then the whole); this side
//! only applies what it sends.

use std::path::PathBuf;
use std::sync::Arc;

use rush_convo::reading::{Reading, Slice};
use rush_convo::{Decode, Options, Renderer, Session};
use rush_core::adapter::{self, AdapterRef, Kind, Registry, migrated};
use rush_core::agent::Agent;
use rush_core::event::{Event, Exchange};
use rush_core::text::{Line, Span, Style, width};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use crate::look::kind_name;
use crate::mention::{ROOM_KEY_PREFIX, drawn_handle};
use crate::style::*;
use crate::worker::TailHandle;

/// How agent k's transcript lines read as rush's events: the native
/// agent's decoder, for an agent whose sessions write its transcripts
/// (Claude Code's own, and those that run in it). None for one rush can't
/// read yet.
pub fn decoder(reg: &Registry, k: &str) -> Option<Decode> {
    let a = reg.get(migrated(k).as_str())?;
    if !a.claude_transcripts() {
        return None;
    }
    let n = reg.native_agent()?;
    n.native()?;
    Some(std::sync::Arc::new(move || match n.native() {
        Some(n) => n.messages(),
        None => unreachable!("the native agent was checked to read messages"),
    }))
}

pub use rush_convo::reading::apply_events;

/// How conversation src is read (rush_convo::reading), whatever agent
/// wrote it: a transcript in the form Claude Code writes, or another
/// agent's history as its adapter reads it back. host: a stopped rush
/// session's, whose folder says where its history is. Reads the disk.
pub fn reading(src: &Source, host: &str) -> adapter::Result<Box<dyn Reading>> {
    match src {
        Source::Transcript { path, decode } => Ok(Box::new(rush_convo::reading::Transcript::new(
            path.clone(),
            decode.clone(),
        ))),
        Source::History { adapter, session } => {
            let s = crate::worker::history_session((**session).clone(), host);
            Ok(Box::new(rush_convo::reading::History::new(
                adapter.clone(),
                s,
            )?))
        }
        Source::Hosted | Source::Room => Err("nothing to read on disk".into()),
    }
}

/// What a view knows of its session once it takes a reading whose own
/// events said `read`, over `had`: a transcript's is the list's word alone
/// (had); a history's events say it themselves (says), as when applied to
/// the view's session one by one, with had where they said nothing.
pub fn info_over(says: bool, read: rush_convo::Info, had: rush_convo::Info) -> rush_convo::Info {
    if !says {
        return had;
    }
    fn or(a: String, b: String) -> String {
        if a.is_empty() { b } else { a }
    }
    rush_convo::Info {
        cwd: or(read.cwd, had.cwd),
        cost: if read.cost != 0.0 {
            read.cost
        } else {
            had.cost
        },
        model: or(read.model, had.model),
        id: or(read.id, had.id),
        kind: or(read.kind, had.kind),
        effort: or(read.effort, had.effort),
        permission_mode: or(read.permission_mode, had.permission_mode),
        started_at: if read.started_at != 0 {
            read.started_at
        } else {
            had.started_at
        },
        started_by: or(read.started_by, had.started_by),
        spawned_by: or(read.spawned_by, had.spawned_by),
        started_from: or(read.started_from, had.started_from),
        temp_dir: or(read.temp_dir, had.temp_dir),
    }
}

/// Where an open conversation is read from.
#[derive(Clone)]
pub enum Source {
    /// A transcript in Claude Code's format, framed by convo::Tail (its
    /// prompts, notices, compactions and hooks) with the native agent's
    /// decoder: Claude Code's own sessions, and agents that run in it.
    Transcript { path: PathBuf, decode: Decode },
    /// Any other agent's history, as its adapter reads it back: followed
    /// through its HistoryFollower, else read once through its
    /// HistoryReader.
    History {
        adapter: AdapterRef,
        session: Box<adapter::Session>,
    },
    /// A session rush hosts with nothing to read on disk yet (one just
    /// started): its host's connection fills it (session_core/live.rs).
    Hosted,
    /// A room: its log folded into a conversation (social/rooms.rs
    /// RoomFeed, Go's roomFeed).
    Room,
}

impl Source {
    /// Where agent a's conversation can be read from, through the
    /// registry; None when its adapter can't read it back.
    pub fn of(reg: &Registry, a: &Agent) -> Option<Source> {
        if !a.job.transcript_path.is_empty()
            && let Some(decode) = decoder(reg, a.kind.as_str())
        {
            return Some(Source::Transcript {
                path: PathBuf::from(&a.job.transcript_path),
                decode,
            });
        }
        let ad = reg.get(migrated(a.kind.as_str()).as_str())?;
        if ad.history_follower().is_none() && ad.history_reader().is_none() {
            return None;
        }
        // As Go's openHistory (internal/ui/history.go:45): the history its
        // adapter listed, in the profile it was found in, else where the
        // adapter says the session keeps it. A past Codex session is found
        // by its thread in that profile's sessions folder.
        let profile = adapter::Profile {
            kind: a.kind.clone(),
            ..a.acct.clone()
        };
        let mut transcript = if a.history.is_empty() {
            a.job.transcript_path.clone()
        } else {
            a.history.clone()
        };
        if transcript.is_empty() {
            transcript = reg.transcript_path(
                migrated(a.kind.as_str()).as_str(),
                &profile,
                &a.job.cwd,
                &a.job.session_id,
            );
        }
        Some(Source::History {
            adapter: ad,
            session: Box::new(adapter::Session {
                kind: a.kind.clone(),
                profile,
                id: a.job.session_id.clone(),
                name: a.display_name.clone(),
                cwd: a.job.cwd.clone(),
                transcript,
                pid: a.pid,
                headless: a.headless,
                remote: a.remote,
                ..adapter::Session::default()
            }),
        })
    }
}

/// The program agent k runs in, as people call it: "Claude Code".
pub fn harness_name(reg: &Registry, k: &Kind) -> String {
    reg.harness_label(migrated(k.as_str()).as_str())
}

/// The pane's chrome: a blank row, the name, where it works, its tabs,
/// and a rule.
pub const HEAD_H: usize = 5;

/// Where the view is: following the conversation's end, or scrolled up to
/// a row of a turn, kept there whatever is added below or counted again
/// above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pos {
    End,
    /// The view's top row is `off` rows into turn number n.
    At {
        n: u32,
        off: usize,
    },
}

/// How many prompts the conversation pins over the top of the body with
/// it starting at row start, rows tall: the conversation's to say, so a
/// jump lands just below them (set by rush-ui-session-core).
pub static TOP_PINS: std::sync::OnceLock<fn(&SessionView, usize, usize) -> usize> =
    std::sync::OnceLock::new();

pub struct SessionView {
    pub key: String,
    /// Where its conversation is read from.
    pub source: Source,
    pub sess: Session,
    pub r: Renderer,
    /// How it's drawn, kept between frames: the folds you've set
    /// (`o.open`), the width, the clock.
    pub o: Options,
    /// The row picked, by ref; "" for none.
    pub sel: String,
    pub pos: Pos,
    /// `pos` is a jump to a pinned prompt: the turn lands just below the
    /// top pins that show there, not under them. Cleared once placed.
    pub land: bool,
    pub verbose: bool,
    pub r#gen: u64,
    pub tail: Option<TailHandle>,
    pub err: String,
    pub read: bool,
    /// Its reading has reached what's written so far: a transcript or
    /// history taken in a slice at a time isn't, between slices, and its
    /// turn under way then isn't one its stopped agent left open.
    pub caught_up: bool,
    /// Its first reading has reached what was written then.
    pub ended: bool,
    /// The ref on each screen row last drawn, and where they start.
    pub shown: Vec<String>,
    pub top: usize,
    /// Rows at the body's top that aren't the header's nor the transcript's
    /// (none drawn now): top is that far below the header.
    pub top_off: usize,
    /// Columns kept at the right of the conversation for what's drawn
    /// beside it (the map): the conversation is drawn that much narrower.
    pub side: usize,
    /// Drawn beside the list: the turns' rule stays out of the gutter
    /// (Options.NoRule, besideList).
    pub beside: bool,
    /// Drawn without the keys (Options.Focused, paneFocus).
    pub idle: bool,
    /// The conversation's row at the view's top, and the first row the
    /// renderer's window holds, as last drawn.
    pub start: usize,
    pub base: usize,
    /// Rows after the transcript that scroll with it: what the dock puts
    /// on its end in a conversation (Go's panel rows: messages on their
    /// way, the queue, a row of room).
    pub panels: Vec<Line>,
    /// The ref on each panel row ("" for none): the dock's runs, picked
    /// and clicked there.
    pub panel_refs: Vec<String>,
    /// Where the panels' first row sat, in rows below the view's top, as
    /// last drawn (above it when negative): for clicks on what's there.
    pub panels_at: isize,
    /// How it last drew while it showed only a long conversation's end:
    /// the worker reading the whole lays out its end the same way.
    pub drawn: crate::worker::Drawn,
    /// A stopped rush session's past exchanges, from its journal: merged
    /// into what's read of its transcript once it's there, and again
    /// when the whole of it takes the end's place (rushmode.go:2106).
    pub exchanges: Arc<[Exchange]>,
    /// Its agent has stopped, as follow last found it.
    pub stopped: bool,
}

/// A conversation with this many steps is let go off the UI thread, its
/// memory given back: dropping one is thousands of frees.
const LET_GO_STEPS: usize = 256;

impl Drop for SessionView {
    fn drop(&mut self) {
        if self.sess.steps().len() >= LET_GO_STEPS {
            rush_sys::mem::let_go((std::mem::take(&mut self.sess), std::mem::take(&mut self.r)));
        }
    }
}

impl SessionView {
    pub fn new(key: String, source: Source, r#gen: u64) -> SessionView {
        SessionView {
            key,
            caught_up: matches!(source, Source::Hosted | Source::Room),
            ended: false,
            source,
            sess: Session::new(),
            r: Renderer::new(),
            o: Options::default(),
            sel: String::new(),
            pos: Pos::End,
            land: false,
            verbose: false,
            r#gen,
            tail: None,
            err: String::new(),
            read: false,
            shown: Vec::new(),
            top: 0,
            top_off: 0,
            side: 0,
            beside: false,
            idle: false,
            start: 0,
            base: 0,
            panels: Vec::new(),
            panel_refs: Vec::new(),
            panels_at: 0,
            drawn: crate::worker::Drawn::default(),
            exchanges: Arc::default(),
            stopped: false,
        }
    }

    /// Merges the session's past exchanges into its conversation
    /// (RestoreExchanges); merging one again changes nothing.
    pub fn restore_exchanges(&mut self, records: Vec<rush_core::event::Exchange>) {
        if !records.is_empty() {
            self.exchanges = records.into();
        }
        if self.read && !self.exchanges.is_empty() {
            self.sess.restore_exchanges(&self.exchanges);
        }
    }

    /// Reads it here and now, for a one-off frame (rush --render), as the
    /// worker does (spawn_read): its end first, then the whole in its
    /// place.
    pub fn read_now(&mut self) {
        self.read = true;
        self.caught_up = true;
        if matches!(self.source, Source::Hosted | Source::Room) {
            return;
        }
        let mut r = match reading(&self.source, "") {
            Ok(r) => r,
            Err(e) => {
                self.err = e.to_string();
                return;
            }
        };
        let end = r.end(crate::worker::TAIL_END);
        if let Some(mut end) = end {
            let had = std::mem::take(&mut self.sess.info);
            end.info = self.info_after(std::mem::take(&mut end.info), had);
            self.sess = end;
        }
        let mut whole = Session::new();
        let never = std::sync::atomic::AtomicBool::new(false);
        match rush_convo::reading::read_on(
            r.as_mut(),
            &mut whole,
            crate::worker::TAIL_SLICE,
            &never,
        ) {
            Ok(_) if self.sess.partial => self.take_whole(whole),
            Ok(_) => {
                let had = std::mem::take(&mut self.sess.info);
                whole.info = self.info_after(std::mem::take(&mut whole.info), had);
                self.sess = whole;
            }
            Err(e) => self.err = e.to_string(),
        }
    }

    /// Takes a conversation's end as the worker that read it first made
    /// it: laid out (warm) as this view draws, with the exchanges it
    /// merged. What the list says of it stays the view's.
    pub fn take_end(&mut self, mut sess: Session, warm: Option<(Renderer, Arc<[Exchange]>)>) {
        self.read = true;
        self.caught_up = true;
        self.ended = true;
        let had = std::mem::take(&mut self.sess.info);
        sess.info = self.info_after(std::mem::take(&mut sess.info), had);
        let (r, merged) = match warm {
            Some((r, x)) => (r, Arc::ptr_eq(&x, &self.exchanges)),
            None => (Renderer::new(), false),
        };
        self.sess = sess;
        self.r = r;
        if !merged {
            self.restore_exchanges(Vec::new());
        }
    }

    /// What the view knows of its session once it takes a reading whose
    /// own events said `read`, over `had`: a transcript's is the list's
    /// word alone (had); a history's events say it themselves, as when
    /// applied to the view's session one by one (take_events), with had
    /// where they said nothing.
    fn info_after(&self, read: rush_convo::Info, had: rush_convo::Info) -> rush_convo::Info {
        info_over(matches!(self.source, Source::History { .. }), read, had)
    }

    /// Takes what the worker read; true when it's the last slice of its
    /// first reading, the conversation's end now all in.
    pub fn take(&mut self, f: Slice) -> bool {
        let first = !self.read;
        self.read = true;
        self.caught_up = !f.more;
        let ended = self.caught_up && !std::mem::replace(&mut self.ended, true);
        f.apply(&mut self.sess);
        if first {
            self.restore_exchanges(Vec::new());
        }
        ended
    }

    /// Takes its end read ahead of time (list_extras/predict.rs), as the
    /// worker's slices of it would have been taken.
    pub fn seed(&mut self, sess: Session) {
        self.sess = sess;
        self.read = true;
        self.caught_up = true;
        self.ended = true;
        self.restore_exchanges(Vec::new());
    }

    /// Puts the whole conversation where its end was. Refs name turns by
    /// number, and the end numbered its own from 1: what was opened,
    /// picked or scrolled to moves to the same turn, so nothing on screen
    /// jumps (Go's takeWhole, twopass.go:96).
    pub fn take_whole(&mut self, whole: Session) {
        self.take_whole_drawn(whole, None);
    }

    /// take_whole, with its end laid out already by the worker that read
    /// it (r), as this view last drew and with the exchanges it merged:
    /// the frame that shows it draws only what changed since (Go's warmed,
    /// rushmode.go:2064).
    pub fn take_whole_drawn(
        &mut self,
        mut whole: Session,
        warm: Option<(Renderer, Arc<[Exchange]>)>,
    ) {
        let part = &self.sess;
        let mut shift = whole.turns.len() as i64 - part.turns.len() as i64;
        if let Some(last) = part.turns.last() {
            let n = part.turns.len() as i64;
            if let Some(i) = whole
                .turns
                .iter()
                .rposition(|t| t.start == last.start && t.prompt == last.prompt)
            {
                shift = i as i64 - (n - 1);
            }
        }
        whole.info = self.info_after(std::mem::take(&mut whole.info), part.info.clone());
        self.sel = renumber(&self.sel, shift);
        self.o.open = std::mem::take(&mut self.o.open)
            .into_iter()
            .map(|(k, v)| (renumber(&k, shift), v))
            .collect();
        if let Pos::At { n, off } = self.pos {
            self.pos = Pos::At {
                n: (i64::from(n) + shift).max(1) as u32,
                off,
            };
        }
        let (r, merged) = match warm {
            Some((r, x)) => (r, Arc::ptr_eq(&x, &self.exchanges)),
            None => (Renderer::new(), false),
        };
        let end = std::mem::replace(&mut self.sess, whole);
        let drawn = std::mem::replace(&mut self.r, r);
        // The end it showed, and the reading of the whole before it, go
        // back to the system rather than sitting in the allocator.
        rush_sys::mem::let_go((end, drawn));
        if let Ok(mut d) = self.drawn.lock() {
            *d = None;
        }
        if !merged {
            self.restore_exchanges(Vec::new());
        }
    }

    /// Takes events an adapter's history gave: reset, the history was
    /// rewritten and the conversation starts over. History events carry
    /// no times, so each is applied as of an unknown time (0).
    pub fn take_events(&mut self, events: Vec<Event>, reset: bool) {
        self.read = true;
        apply_events(&mut self.sess, &events, reset);
    }

    /// Who answers under your messages: its @handle, and what it is
    /// (rushmode.go:2400, agentHandle and kindName); a room names each
    /// speaker itself. Set as the view opens, so its first frame says it,
    /// and kept up as the list renames it. Whether it changed.
    pub fn name_speaker(&mut self, a: &Agent) -> bool {
        if a.key.starts_with(ROOM_KEY_PREFIX) {
            return false;
        }
        let handle = drawn_handle(a);
        let kind = migrated(a.kind.as_str());
        let speaker = kind_name(rush_core::adapter::global(), kind.as_str());
        if self.o.agent == handle && self.o.speaker == speaker {
            return false;
        }
        self.o.agent = handle;
        self.o.speaker = speaker;
        true
    }

    /// Ends the conversation's live turn when its agent has stopped, and
    /// takes the list's word for where it works, what it cost and its
    /// model (Go's followTail, rushmode.go:2008): a transcript that ended
    /// mid-turn doesn't spin forever. Whether anything changed.
    pub fn follow(&mut self, a: &Agent, now: Ms) -> bool {
        let mut changed = false;
        // Only a transcript's (followTail returns without a tail: an
        // adapter's history, a host's or a room's says when its turn ends),
        // and only once read to the end: a turn open between two slices of
        // the reading is still being read, not left open by the agent (Go
        // reads the whole before following it, rushmode.go:2117).
        let tail = matches!(self.source, Source::Transcript { .. });
        self.stopped = !a.job.live();
        if tail && self.caught_up && self.sess.live().is_some() && self.stopped {
            let end = crate::worker::end_left_open();
            self.sess.apply(&rush_core::event::Event::TurnEnd(end), now);
            changed = true;
        }
        changed |= self.name_speaker(a);
        let i = &mut self.sess.info;
        let was = i.clone();
        // A host's session says where it works itself; only a
        // transcript's takes the list's word (followTail returns without
        // a tail).
        if tail || i.cwd.is_empty() {
            i.cwd.clone_from(&a.job.cwd);
        }
        i.cost = a.spend.cost;
        if i.model.is_empty() {
            a.spend.model.as_str().clone_into(&mut i.model);
        }
        changed || *i != was
    }

    /// Brings the drawing options up to date: w wide, at now.
    pub fn sync(&mut self, w: usize, now: Ms, tick: usize) {
        let o = &mut self.o;
        o.width = w.saturating_sub(self.side);
        o.now = now;
        o.tick = tick;
        o.verbose = self.verbose;
        if o.selected != self.sel {
            o.selected.clone_from(&self.sel);
        }
        o.focused = !self.idle;
        o.no_rule = self.beside;
        // The pane's live line is docked: it names a running or waiting
        // step and the turn's time (rushmode.go:2336 paneActivity).
        o.dock_activity = true;
        o.zone = crate::model::local_zone(now);
        // Each time in the zone's offset then, as Go's time.Local: a
        // message from before the clocks changed keeps its hour.
        o.zone_at = Some(crate::model::local_zone);
        // The session's own harness, which a run in another is named
        // against (rushmode.go:2402, providers.go:127 kindOf).
        let kind = match self.sess.info.kind.as_str() {
            "" => rush_state::LOGINS_KIND,
            k => k,
        };
        if o.kind != kind {
            o.kind = kind.to_string();
        }
        // The calls whose running command rush paused (rushmode.go:2403).
        let paused = crate::shells::paused_ids();
        if o.paused != paused {
            o.paused = paused;
        }
    }

    /// The row the view starts at for a session total rows long, rows tall.
    fn want_start(&self, total: usize, rows: usize) -> usize {
        let max = total.saturating_sub(rows);
        match self.pos {
            Pos::End => max,
            Pos::At { n, off } => match turn_index(&self.sess, n) {
                Some(i) if self.land => {
                    // Just below the top pins that will show with the view
                    // there: the fewest rows up that hold that many.
                    let row = self.r.turn_row(i);
                    let tops = |s: usize| TOP_PINS.get().map_or(0, |f| f(self, s, rows));
                    let k = (0..=2)
                        .find(|&k| tops(row.saturating_sub(k)) <= k)
                        .unwrap_or(2);
                    (row.saturating_sub(k) + off).min(max)
                }
                Some(i) => (self.r.turn_row(i) + off).min(max),
                None => max,
            },
        }
    }

    /// Puts the view's top at row start: following the end once it's there.
    fn set_start(&mut self, start: usize, total: usize, rows: usize) {
        self.land = false;
        if start >= total.saturating_sub(rows) || self.sess.turns.is_empty() {
            self.pos = Pos::End;
            return;
        }
        let i = self.r.turn_at(start);
        self.pos = Pos::At {
            n: self.sess.turns[i].n,
            off: start - self.r.turn_row(i).min(start),
        };
    }

    /// How many rows the conversation has from the view's top to its end,
    /// as last laid out: what keep_rows_below puts back once more of what
    /// came before its end is read (Rust-only).
    pub fn rows_below_top(&mut self) -> usize {
        let total = self.r.count(&self.sess, &self.o) + self.panels.len();
        total.saturating_sub(self.start)
    }

    /// Puts the view's top below rows above the conversation's end, rows
    /// tall: where it was before more was read above it, so nothing on
    /// screen jumps (Rust-only).
    pub fn keep_rows_below(&mut self, below: usize, rows: usize) {
        let total = self.r.count(&self.sess, &self.o) + self.panels.len();
        self.set_start(total.saturating_sub(below), total, rows);
    }

    /// Draws the turns around the view, rows tall, and only those (Go's
    /// drawWindow, up to three passes as what was estimated is drawn).
    /// Returns where the view starts and where the drawn rows do.
    fn layout(&mut self, rows: usize) -> (usize, usize) {
        let p = self.panels.len();
        let mut total = self.r.count(&self.sess, &self.o) + p;
        let mut start = self.want_start(total, rows);
        let mut base = 0;
        for _ in 0..3 {
            let (b, t) = self.r.window(
                &self.sess,
                &self.o,
                start.saturating_sub(rows),
                start + 2 * rows,
            );
            (base, total) = (b, t + p);
            start = self.want_start(total, rows);
            let (_, hi) = self.r.window_turns();
            let end = if hi >= self.sess.turns.len() {
                total
            } else {
                self.r.turn_row(hi)
            };
            if start >= base && (start + rows).min(total) <= end {
                break;
            }
        }
        self.set_start(start, total, rows);
        (start, base)
    }

    /// Notes how it's drawn now, for the worker reading it to lay out what
    /// it reads the same way: while it's yet to be read, and while it
    /// shows only a long conversation's end.
    pub fn note_drawn(&self) {
        if let Ok(mut d) = self.drawn.lock() {
            *d = Some(crate::worker::DrawnAs {
                o: self.o.clone(),
                info: self.sess.info.clone(),
                exchanges: self.exchanges.clone(),
                stopped: self.stopped,
            });
        }
    }

    /// The conversation's rows in view, `rows` tall; notes their refs.
    pub fn body(&mut self, rows: usize) -> Vec<Line> {
        if self.sess.partial {
            self.note_drawn();
        }
        let (start, base) = self.layout(rows);
        (self.start, self.base) = (start, base);
        let all = self.r.rows();
        let from = start.saturating_sub(base).min(all.len());
        let to = (from + rows).min(all.len());
        let mut shown: Vec<String> = all[from..to].iter().map(|l| l.r#ref.clone()).collect();
        let mut out: Vec<Line> = all[from..to].iter().map(|l| l.line.clone()).collect();
        // Past the transcript's end, the panels.
        let t = self.r.count(&self.sess, &self.o);
        self.panels_at = t as isize - start as isize;
        let mut r = start + out.len();
        while out.len() < rows && r >= t && r - t < self.panels.len() {
            out.push(self.panels[r - t].clone());
            shown.push(self.panel_refs.get(r - t).cloned().unwrap_or_default());
            r += 1;
        }
        self.shown = shown;
        out
    }

    /// The conversation's height in rows, its panels' included.
    pub fn total(&mut self) -> usize {
        self.r.count(&self.sess, &self.o) + self.panels.len()
    }

    /// Picks the next row up (d < 0) or down with a ref of its own, and
    /// scrolls it into view.
    pub fn move_sel(&mut self, d: isize, rows: usize) {
        let (start, base) = self.layout(rows);
        let refs: Vec<String> = self.r.rows().iter().map(|l| l.r#ref.clone()).collect();
        let n = refs.len();
        if n == 0 {
            return;
        }
        let total = self.total();
        // The view's rows within the window.
        let vs = start.saturating_sub(base).min(n);
        let ve = (vs + rows).min(n);
        // The rows of them drawn as they are, [lo, hi): pinned prompts
        // cover the top rows and the bottom one, and scrolled up the
        // "more below" pill takes the last (conversation.rs); a row under
        // them isn't in view, to pick or to scroll to. As last drawn, when
        // that was at this start; else all but the pill's.
        let (lo, hi) = if self.shown.is_empty() || self.start != start {
            let pill = usize::from(self.pos != Pos::End && rows > 1);
            (vs, ve.saturating_sub(pill).max(vs))
        } else {
            let m = self.shown.len().min(ve - vs);
            let same = |k: usize| self.shown[k] == refs[vs + k];
            let top = (0..m).take_while(|&k| !same(k)).count();
            let bot = (top..m).rev().take_while(|&k| !same(k)).count();
            (vs + top, vs + m - bot)
        };
        let at = refs[vs..ve]
            .iter()
            .position(|r| !self.sel.is_empty() && *r == self.sel)
            .map(|i| i + vs);
        let mut j: Option<usize> = None;
        match (at, d < 0) {
            (None, true) => j = (lo..hi).rev().find(|&i| !refs[i].is_empty()),
            // ↓ with nothing picked stays in the box (Go's moveSel).
            (None, false) => return,
            (Some(i), true) => {
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    if !refs[k].is_empty() && refs[k] != self.sel {
                        // The top row of that ref.
                        let r = &refs[k];
                        while k > 0 && refs[k - 1] == *r {
                            k -= 1;
                        }
                        j = Some(k);
                        break;
                    }
                }
            }
            (Some(i), false) => {
                j = (i + 1..n).find(|&k| !refs[k].is_empty() && refs[k] != self.sel);
            }
        }
        let Some(j) = j else {
            if d > 0 && at.is_some() && base + n >= total - self.panels.len() {
                // ↓ past the last row: back to typing, nothing picked.
                self.sel.clear();
                return;
            }
            // Nothing more that way in the rows drawn: scroll instead.
            let half = rows as isize / 2;
            self.scroll(if d < 0 { half } else { -half }, rows);
            return;
        };
        self.sel = refs[j].clone();
        if j < lo {
            // Its first row just under the pins over the top.
            self.set_start((base + j).saturating_sub(lo - vs), total, rows);
        } else if j >= hi {
            // Its last row just over what covers the bottom.
            let h = (hi - vs).max(1);
            self.set_start((base + j + 1).saturating_sub(h), total, rows);
        }
    }

    /// Up by d rows (down when negative), rows tall.
    pub fn scroll(&mut self, d: isize, rows: usize) {
        let total = self.total();
        let start = (self.want_start(total, rows) as isize - d).max(0) as usize;
        self.set_start(start, total, rows);
    }

    /// To the conversation's first row, however far that is: estimated
    /// until it's drawn, so the jump draws only the top.
    pub fn home(&mut self, rows: usize) {
        let total = self.total();
        self.set_start(0, total, rows);
    }

    /// Whether the row picked shows open: folded rows show ▸.
    fn is_open(&mut self, r: &str, rows: usize) -> bool {
        if let Some(&v) = self.o.open.get(r) {
            return v;
        }
        self.layout(rows);
        let mut seen = false;
        // The rows near the view say it; a ref out of view is closed.
        for l in self.r.rows() {
            if l.r#ref == r {
                seen = true;
                if l.line.plain().contains('▸') {
                    return false;
                }
            } else if seen {
                break;
            }
        }
        seen
    }

    /// Whether the row picked can open and is not open: it shows ▸, or
    /// opening it would add rows (a folded step has no glyph). Tries it,
    /// then puts it back as it was, so only for a key, not every frame.
    pub fn can_open(&mut self, rows: usize) -> bool {
        if self.sel.is_empty() {
            return false;
        }
        let sel = self.sel.clone();
        let was = self.o.open.get(&sel).copied();
        if was == Some(true) {
            return false;
        }
        self.layout(rows);
        if self
            .r
            .rows()
            .iter()
            .any(|l| l.r#ref == sel && l.line.plain().contains('▸'))
        {
            return true;
        }
        let before = self.r.rows().len();
        self.o.open.insert(sel.clone(), true);
        self.layout(rows);
        let after = self.r.rows().len();
        match was {
            Some(v) => self.o.open.insert(sel, v),
            None => self.o.open.remove(&sel),
        };
        self.layout(rows);
        after > before
    }

    /// Opens or closes the row picked.
    pub fn toggle(&mut self, rows: usize) {
        if self.sel.is_empty() {
            return;
        }
        let v = !self.is_open(&self.sel.clone(), rows);
        self.o.open.insert(self.sel.clone(), v);
    }

    /// Closes what's open round the row picked, one level: the row itself
    /// when it's open (a step, a group of steps, a turn), else the step or
    /// group of steps it is in, innermost first; the pick moves to what closed (its header row).
    /// Whether anything was open to close.
    pub fn fold_in(&mut self, rows: usize) -> bool {
        if self.sel.is_empty() {
            return false;
        }
        self.layout(rows);
        let sel = self.sel.clone();
        let drawn = self.r.rows();
        let mut chain = vec![sel.clone()];
        if let Some(row) = drawn.iter().find(|l| l.r#ref == sel) {
            chain.extend(
                row.owner
                    .rsplit(rush_convo::render::OWNER_SEP)
                    // A turn round it stays: it closes only when picked.
                    .filter(|o| !o.is_empty() && o.contains(':'))
                    .map(str::to_string),
            );
        }
        // Opened or closed by hand, else the header row says by its glyph.
        let open = |r: &str| match self.o.open.get(r) {
            Some(&v) => v,
            None => drawn
                .iter()
                .find(|l| l.r#ref == r)
                .is_some_and(|l| l.line.plain().contains('▾')),
        };
        let Some(hit) = chain.into_iter().find(|r| open(r)) else {
            return false;
        };
        self.o.open.insert(hit.clone(), false);
        self.sel = hit;
        true
    }

    /// Scrolls row r (a turn's or a step's ref) into view, a third of the
    /// way down, rows tall: what's there after the conversation is drawn
    /// another way. A ref not drawn leaves its turn at the top; one of no
    /// turn leaves the view where it is. Whether r is drawn.
    pub fn reveal(&mut self, r: &str, rows: usize) -> bool {
        let turn = r.split(':').next().unwrap_or("");
        let Some(n) = turn.strip_prefix('t').and_then(|n| n.parse::<u32>().ok()) else {
            return false;
        };
        if turn_index(&self.sess, n).is_none() {
            return false;
        }
        self.pos = Pos::At { n, off: 0 };
        let (_, base) = self.layout(rows);
        let at = self.r.rows().iter().position(|l| l.r#ref == r);
        if let Some(j) = at {
            let total = self.total();
            let row = base + j;
            self.set_start(row.saturating_sub(rows / 3), total, rows);
        }
        at.is_some()
    }

    /// The ref on screen row y, if any.
    pub fn ref_at(&self, y: usize) -> Option<&str> {
        let i = y.checked_sub(self.top)?;
        self.shown
            .get(i)
            .map(String::as_str)
            .filter(|r| !r.is_empty())
    }
}

/// A ref of turn N ("t12", "t12:s:…") moved to turn N+shift (renumber).
fn renumber(r: &str, shift: i64) -> String {
    if shift == 0 || !r.starts_with('t') {
        return r.to_string();
    }
    let (turn, rest) = match r.split_once(':') {
        Some((t, rest)) => (t, Some(rest)),
        None => (r, None),
    };
    let Ok(n) = turn[1..].parse::<i64>() else {
        return r.to_string();
    };
    match rest {
        Some(rest) => format!("t{}:{rest}", n + shift),
        None => format!("t{}", n + shift),
    }
}

/// The index of turn number n, or None (Go's turnIndex).
fn turn_index(s: &Session, n: u32) -> Option<usize> {
    let i = s.turns.partition_point(|t| t.n < n);
    if s.turns.get(i).is_some_and(|t| t.n == n) {
        return Some(i);
    }
    s.turns.iter().position(|t| t.n == n)
}

/// The pane's chrome, w wide: blank, name and state, where it works and
/// what it is, the tab, a rule.
/// conn, when given, is said where an agent's header says how it's
/// connected: a room's says how it stands (paneHeader's roomConn).
pub fn header(
    a: Option<&Agent>,
    v: &SessionView,
    w: usize,
    now: Ms,
    reg: &Registry,
    conn: Option<Line>,
) -> Vec<Line> {
    let iw = w.saturating_sub(3);
    let chrome = |l: Line| {
        let mut out = line([Span::new("  ", Style::default().on(CHROME_BG))]);
        join(&mut out, on_ground(l, iw, CHROME_BG));
        out.spans
            .push(Span::new(" ", Style::default().on(CHROME_BG)));
        out
    };
    let Some(a) = a else {
        return vec![
            chrome(Line::new()),
            chrome(line([sp("this session has gone", DIM)])),
            chrome(Line::new()),
            chrome(Line::new()),
            line([sp("─".repeat(w), FAINT)]),
        ];
    };
    let state = if a.needs_you() && !a.at_work {
        spb("needs you", YELLOW)
    } else if a.halted() {
        spb("stopped mid-turn", RED)
    } else if a.job.live() {
        let mut s = spb("working", ORANGE);
        if let Some(t) = v.sess.live()
            && !t.left_open(now, v.o.between_turns)
        {
            s.text.push_str(&format!(" · {}", dur(now - t.start)));
        }
        s
    } else if a.pid != 0 {
        sp("idle", DIM)
    } else {
        sp("stopped", DIM)
    };
    let mut name = one_line(&a.display_name);
    // paneHeader's lead: the name, the row under it and the tabs' names
    // all start two cells in.
    let room = 12.max(56.min(iw.saturating_sub(2 + 3 + width(&state.text)) / 2));
    if width(&name) > room {
        name = rush_draw::truncate(&name, room, "…").into_owned();
    }
    let row1 = line([
        plain("  "),
        spb(name, BRIGHT),
        plain("   "),
        sp("·", DIM),
        plain("  "),
        state,
    ]);
    let mut meta = Vec::new();
    let cwd = if a.job.cwd.is_empty() {
        &a.repo
    } else {
        &a.job.cwd
    };
    if !cwd.is_empty() {
        meta.push(tildify(cwd));
    }
    if !a.branch.is_empty() {
        meta.push(a.branch.clone());
    }
    let model: &str = if v.sess.model.is_empty() {
        &a.spend.model
    } else {
        &v.sess.model
    };
    if !model.is_empty() {
        meta.push(model.trim_start_matches("claude-").to_string());
    }
    let mut row2 = line([plain("  ")]);
    for (i, m) in meta.into_iter().enumerate() {
        if i > 0 {
            row2.spans.push(sp(" · ", FAINT));
        }
        row2.spans.push(sp(m, DIM));
    }
    let harness = harness_name(reg, &a.kind);
    let said = if conn.is_some() {
        String::new()
    } else if a.past {
        "past conversation".to_string()
    } else if a.headless {
        // Agent.Where (fleet.go:1551).
        format!("{harness} · run by another program (claude -p)")
    } else if a.interactive {
        format!("{harness} · open in a terminal")
    } else {
        harness
    };
    let mut conn = conn.unwrap_or_else(|| line([sp(said, DIM)]));
    conn.spans.push(plain(" "));
    let row2 = crate::header::spread(row2, conn, iw);
    // tabLabel (rushmode.go:1418): the view's name, capitalised.
    let tab = Span::new(" Conversation ", Style::fg(BRIGHT).bold().on(TAB_ON_BG));
    let tab_w = tab.width();
    let mut chips = Line::new();
    if v.verbose {
        chips.spans.push(sp("m3 all shown", ORANGE));
        chips.spans.push(plain("  "));
    }
    chips.spans.push(sp("esc", TEXT));
    chips.spans.push(sp(" back to the list", DIM));
    chips.spans.push(plain(" "));
    let row3 = crate::header::spread(line([tab]), chips, iw);
    let rule = line([
        sp("──", FAINT),
        sp("─", FAINT),
        sp("━".repeat(tab_w.min(iw)), SUB),
        sp("─".repeat(w.saturating_sub(3 + tab_w)), FAINT),
    ]);
    vec![
        chrome(Line::new()),
        chrome(row1),
        chrome(row2),
        chrome(row3),
        fit(rule, w),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(s: &mut Session, prompts: &[&str], from: Ms) {
        for (i, p) in prompts.iter().enumerate() {
            let at = from + i as Ms * 1000;
            s.sent(p.to_string(), Vec::new(), at);
            let end = rush_core::event::TurnEnd::default();
            s.apply(&rush_core::event::Event::TurnEnd(end), at + 500);
        }
    }

    /// A History source over a registry with one bare adapter.
    fn history_view() -> SessionView {
        struct Any;
        impl adapter::Adapter for Any {
            fn kind(&self) -> Kind {
                Kind::from("any")
            }
            fn name(&self) -> String {
                "Any".into()
            }
            fn features(&self) -> adapter::Features {
                adapter::Features::default()
            }
            fn level(&self) -> adapter::Level {
                adapter::Level::default()
            }
            fn profiles(&self) -> Vec<adapter::Profile> {
                Vec::new()
            }
        }
        let reg = Registry::new();
        reg.register(Any);
        let src = Source::History {
            adapter: reg.get("any").expect("registered"),
            session: Box::default(),
        };
        SessionView::new("k".into(), src, 1)
    }

    /// A stopped agent's row.
    fn stopped() -> Agent {
        let mut a = Agent {
            key: "k".into(),
            ..Agent::default()
        };
        a.job.state = rush_core::agent::State::Idle;
        a
    }

    /// A history's end, then its whole, taken as read on a worker: what
    /// their events said of it is kept, what the view knew fills the
    /// rest, as when its events are taken one by one.
    #[test]
    fn a_history_taken_whole_keeps_what_its_events_said() {
        let mut v = history_view();
        v.sess.info.cost = 1.5;
        v.sess.info.model = "from the list".into();
        let mut end = Session::new();
        end.partial = true;
        end.info.model = "gpt-5".into();
        end.info.effort = "high".into();
        v.take_end(end, None);
        assert_eq!(
            (v.sess.info.model.as_str(), v.sess.info.effort.as_str()),
            ("gpt-5", "high")
        );
        assert_eq!(v.sess.info.cost, 1.5);
        let mut whole = Session::new();
        whole.info.effort = "low".into();
        v.take_whole(whole);
        assert_eq!(
            (v.sess.info.model.as_str(), v.sess.info.effort.as_str()),
            ("gpt-5", "low")
        );
        assert!(!v.sess.partial);
    }

    /// The history read again from its start keeps what the host saved:
    /// the model with its window ("opus 5.5 · 1M"), which the history
    /// doesn't say.
    #[test]
    fn history_read_again_keeps_the_saved_info() {
        let mut v = history_view();
        v.sess.info.model = "claude-opus-5-5[1m]".into();
        v.sess.info.effort = "medium".into();
        v.take_events(Vec::new(), true);
        assert_eq!(v.sess.info.model, "claude-opus-5-5[1m]");
        assert_eq!(v.sess.info.effort, "medium");
    }

    /// The folder a hosted session reports is its own: the list's word
    /// only fills it when it has none; a transcript's always takes it
    /// (followTail; rulesheet.go:71 reads c.sess.Info.Cwd first).
    #[test]
    fn a_hosted_session_keeps_the_folder_it_reports() {
        let mut a = stopped();
        a.job.cwd = "/list/says".into();
        let mut h = SessionView::new("k".into(), Source::Hosted, 1);
        h.sess.info.cwd = "/host/says".into();
        h.follow(&a, 5000);
        assert_eq!(h.sess.info.cwd, "/host/says");
        h.sess.info.cwd.clear();
        h.follow(&a, 5000);
        assert_eq!(h.sess.info.cwd, "/list/says");
        let tail = Source::Transcript {
            path: PathBuf::from("/nowhere.jsonl"),
            decode: rush_convo::decode(rush_claude::messages),
        };
        let mut v = SessionView::new("k".into(), tail, 1);
        v.sess.info.cwd = "/old".into();
        v.follow(&a, 5000);
        assert_eq!(v.sess.info.cwd, "/list/says");
    }

    /// A transcript's turn left open by a stopped agent is ended, once
    /// it's read to the end: between two slices it's still being read
    /// (S32's made-up "woke up" turn). An adapter's history isn't: its
    /// own reading says when a turn ends (followTail needs a tail).
    #[test]
    fn only_a_transcript_read_to_its_end_has_its_turn_ended() {
        let a = stopped();
        let tail = Source::Transcript {
            path: PathBuf::from("/nowhere.jsonl"),
            decode: rush_convo::decode(rush_claude::messages),
        };
        let mut v = SessionView::new("k".into(), tail, 1);
        v.sess.sent("go".into(), Vec::new(), 1000);
        v.caught_up = false;
        v.follow(&a, 5000);
        assert!(v.sess.live().is_some(), "ended between two slices");
        v.caught_up = true;
        v.follow(&a, 5000);
        assert!(v.sess.live().is_none(), "left open once read whole");

        let mut h = history_view();
        h.sess.sent("go".into(), Vec::new(), 1000);
        h.caught_up = true;
        h.follow(&a, 5000);
        assert!(
            h.sess.live().is_some(),
            "a history's turn ended by the list"
        );
    }

    /// Who answers is named as the view opens: the harness and the
    /// @handle, never "Agent" (S44).
    #[test]
    fn the_speaker_is_named_as_it_opens() {
        let mut v = history_view();
        let mut a = stopped();
        a.kind = Kind::from("claude");
        a.display_name = "Fix the parser".into();
        assert!(v.name_speaker(&a));
        assert!(v.o.agent.starts_with('@'), "{:?}", v.o.agent);
        assert!(!v.o.speaker.is_empty());
        assert!(!v.name_speaker(&a), "the same names again change nothing");
    }

    /// The whole conversation, read after its end, takes the end's place:
    /// what was picked, opened and scrolled to moves to the same turn
    /// (takeWhole, renumber).
    #[test]
    fn the_whole_where_the_end_was() {
        let mut v = history_view();
        said(&mut v.sess, &["four", "five"], 4000);
        v.sess.partial = true;
        v.sel = "t2:s:c1".into();
        v.o.open.insert("t1".into(), true);
        v.pos = Pos::At { n: 1, off: 3 };
        let mut whole = Session::new();
        said(&mut whole, &["one", "two", "three"], 1000);
        said(&mut whole, &["four", "five"], 4000);
        v.take_whole(whole);
        assert_eq!(v.sess.turns.len(), 5);
        assert!(!v.sess.partial);
        assert_eq!(v.sel, "t5:s:c1");
        assert_eq!(v.o.open.get("t4"), Some(&true));
        assert_eq!(v.pos, Pos::At { n: 4, off: 3 });
    }

    /// The whole as the worker that read it made it: the past exchanges
    /// merged and its end laid out. The view keeps both rather than
    /// merging them again, which would change what was laid out.
    #[test]
    fn the_whole_as_the_worker_merged_it() {
        let mut v = history_view();
        said(&mut v.sess, &["four", "five"], 4000);
        v.sess.partial = true;
        v.read = true;
        v.restore_exchanges(vec![Exchange {
            at: 4500,
            direction: "sent".into(),
            text: "over to you".into(),
            ..Exchange::default()
        }]);
        let mut whole = Session::new();
        said(&mut whole, &["one", "two", "three"], 1000);
        said(&mut whole, &["four", "five"], 4000);
        whole.restore_exchanges(&v.exchanges);
        let o = Options {
            width: 80,
            ..v.o.clone()
        };
        let mut r = Renderer::new();
        r.tail(&whole, &o, 400);
        let kept = r.kept_rows();
        assert!(kept > 0);
        v.take_whole_drawn(whole, Some((r, v.exchanges.clone())));
        let rows = |s: &Session| {
            s.turns
                .iter()
                .flat_map(|t| &t.items)
                .filter(|i| matches!(i, rush_convo::Item::Exchange(_)))
                .count()
        };
        assert_eq!(rows(&v.sess), 1, "merged once");
        assert_eq!(v.r.kept_rows(), kept, "its end as the worker laid it out");
    }

    #[test]
    fn renumbers_turn_refs_only() {
        for (r, want) in [
            ("t3", "t5"),
            ("t3:s:abc", "t5:s:abc"),
            ("chg:/a", "chg:/a"),
            ("tree:/x", "tree:/x"),
            ("", ""),
        ] {
            assert_eq!(renumber(r, 2), want);
        }
    }
}
