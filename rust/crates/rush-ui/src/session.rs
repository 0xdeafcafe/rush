//! A session opened read-only: its header, the conversation as rush-convo
//! draws it, and the keys that scroll it and fold its rows. After Go's
//! internal/ui/rushmode.go (paneHeader :2721, the conversation's keys
//! :3954-4120, isOpen :4661) and panegeometry.go:13 (paneFrameRow).
//! The conversation is read and parsed on a worker (worker::spawn_tail
//! for a Claude Code transcript, worker::spawn_history for any other
//! adapter's history); this side only applies what it sends.

use std::path::PathBuf;

use rush_convo::{Decode, Fresh, Options, Renderer, Session, Tail};
use rush_core::adapter::{self, AdapterRef, Kind, Registry, SLICE, migrated};
use rush_core::agent::Agent;
use rush_core::event::Event;
use rush_core::text::{Line, Span, Style, width};
use rush_core::time::Ms;
use rush_fleet::Predicates;

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
        Some(Source::History {
            adapter: ad,
            session: Box::new(adapter::Session {
                kind: a.kind.clone(),
                id: a.job.session_id.clone(),
                name: a.display_name.clone(),
                cwd: a.job.cwd.clone(),
                transcript: a.job.transcript_path.clone(),
                pid: a.pid,
                headless: a.headless,
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
    pub verbose: bool,
    pub r#gen: u64,
    pub tail: Option<TailHandle>,
    pub err: String,
    pub read: bool,
    /// The ref on each screen row last drawn, and where they start.
    pub shown: Vec<String>,
    pub top: usize,
    /// Columns kept at the right of the conversation for what's drawn
    /// beside it (the map): the conversation is drawn that much narrower.
    pub side: usize,
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
}

impl SessionView {
    pub fn new(key: String, source: Source, r#gen: u64) -> SessionView {
        SessionView {
            key,
            source,
            sess: Session::new(),
            r: Renderer::new(),
            o: Options::default(),
            sel: String::new(),
            pos: Pos::End,
            verbose: false,
            r#gen,
            tail: None,
            err: String::new(),
            read: false,
            shown: Vec::new(),
            top: 0,
            side: 0,
            start: 0,
            base: 0,
            panels: Vec::new(),
            panel_refs: Vec::new(),
        }
    }

    /// Reads it here and now, for a one-off frame (rush --render).
    pub fn read_now(&mut self) {
        self.read = true;
        match &self.source {
            Source::Transcript { path, decode } => {
                let mut t = Tail::from_end(path, 2 << 20, decode.clone());
                if let Err(e) = t.read(&mut self.sess) {
                    self.err = e.to_string();
                } else if t.partway() {
                    // Then the whole of it, as the worker does after.
                    let mut whole = Session::new();
                    if Tail::new(path, decode.clone()).read(&mut whole).is_ok() {
                        self.take_whole(whole);
                    }
                }
            }
            Source::History { adapter, session } => {
                let r = match (adapter.history_follower(), adapter.history_reader()) {
                    (Some(f), _) => f.follow_history(session).and_then(|mut f| {
                        loop {
                            let s = f.next(SLICE)?;
                            self.take_events(s.events, s.reset);
                            if !s.more {
                                break Ok(());
                            }
                        }
                    }),
                    (None, Some(r)) => {
                        adapter::read_history(r, session, 0).map(|evs| self.take_events(evs, false))
                    }
                    (None, None) => Ok(()),
                };
                if let Err(e) = r {
                    self.err = e.to_string();
                }
            }
            Source::Hosted | Source::Room => {}
        }
    }

    /// Takes what the worker read.
    pub fn take(&mut self, f: Fresh) {
        self.read = true;
        f.apply(&mut self.sess);
    }

    /// Puts the whole conversation where its end was. Refs name turns by
    /// number, and the end numbered its own from 1: what was opened,
    /// picked or scrolled to moves to the same turn, so nothing on screen
    /// jumps (Go's takeWhole, twopass.go:96).
    pub fn take_whole(&mut self, mut whole: Session) {
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
        whole.info = part.info.clone();
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
        self.sess = whole;
        self.r = Renderer::new();
    }

    /// Takes events an adapter's history gave: reset, the history was
    /// rewritten and the conversation starts over. History events carry
    /// no times, so each is applied as of an unknown time (0).
    pub fn take_events(&mut self, events: Vec<Event>, reset: bool) {
        self.read = true;
        if reset {
            self.sess = Session::new();
        }
        for ev in &events {
            self.sess.apply(ev, 0);
        }
    }

    /// Ends the conversation's live turn when its agent has stopped, and
    /// takes the list's word for where it works, what it cost and its
    /// model (Go's followTail, rushmode.go:2008): a transcript that ended
    /// mid-turn doesn't spin forever. Whether anything changed.
    pub fn follow(&mut self, a: &Agent, now: Ms) -> bool {
        let mut changed = false;
        if self.sess.live().is_some() && !a.job.live() {
            let end = rush_core::event::TurnEnd {
                reason: "done".into(),
                ..Default::default()
            };
            self.sess.apply(&rush_core::event::Event::TurnEnd(end), now);
            changed = true;
        }
        let i = &mut self.sess.info;
        let was = i.clone();
        i.cwd.clone_from(&a.job.cwd);
        i.cost = a.spend.cost;
        if i.model.is_empty() {
            i.model.clone_from(&a.spend.model);
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
        o.focused = true;
        o.zone = crate::model::local_zone(now);
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
        let paused = crate::screens::projects_machine::shells::paused_ids();
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
                Some(i) => (self.r.turn_row(i) + off).min(max),
                None => max,
            },
        }
    }

    /// Puts the view's top at row start: following the end once it's there.
    fn set_start(&mut self, start: usize, total: usize, rows: usize) {
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

    /// The conversation's rows in view, `rows` tall; notes their refs.
    pub fn body(&mut self, rows: usize) -> Vec<Line> {
        let (start, base) = self.layout(rows);
        (self.start, self.base) = (start, base);
        let all = self.r.rows();
        let from = start.saturating_sub(base).min(all.len());
        let to = (from + rows).min(all.len());
        let mut shown: Vec<String> = all[from..to].iter().map(|l| l.r#ref.clone()).collect();
        let mut out: Vec<Line> = all[from..to].iter().map(|l| l.line.clone()).collect();
        // Past the transcript's end, the panels.
        let t = self.r.count(&self.sess, &self.o);
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
        let at = refs[vs..ve]
            .iter()
            .position(|r| !self.sel.is_empty() && *r == self.sel)
            .map(|i| i + vs);
        let mut j: Option<usize> = None;
        match (at, d < 0) {
            (None, true) => j = (vs..ve).rev().find(|&i| !refs[i].is_empty()),
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
        if j < vs {
            self.set_start(base + j, total, rows);
        } else if j >= ve {
            self.set_start(base + j + 1 - rows.min(j + 1), total, rows);
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

    /// Opens or closes the row picked.
    pub fn toggle(&mut self, rows: usize) {
        if self.sel.is_empty() {
            return;
        }
        let v = !self.is_open(&self.sel.clone(), rows);
        self.o.open.insert(self.sel.clone(), v);
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
    let state = if a.needs_you() {
        spb("needs you", YELLOW)
    } else if a.halted() {
        spb("stopped mid-turn", RED)
    } else if a.job.live() {
        let mut s = spb("working", ORANGE);
        if let Some(t) = v.sess.live() {
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
    let model = if v.sess.model.is_empty() {
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

    /// The whole conversation, read after its end, takes the end's place:
    /// what was picked, opened and scrolled to moves to the same turn
    /// (takeWhole, renumber).
    #[test]
    fn the_whole_where_the_end_was() {
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
        let mut v = SessionView::new("k".into(), src, 1);
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
