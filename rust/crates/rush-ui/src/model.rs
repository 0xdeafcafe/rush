//! The screens' state, the Agents list's and conversation's keys and
//! mouse, and the frame. After Go's internal/ui/model.go (Update,
//! focused :2122, the tick :1081), view.go (title :44, render :604,
//! listView :1095, cardLines :1828, ctxBar :1919, keys :859) and keys.go
//! (listKey :273). The UI thread owns all of it; workers send it Notes
//! (worker.rs). Routing, places and what's drawn over the screen are the
//! frame's (frame/).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rush_core::adapter::{Registry, global};
use rush_core::agent::{Agent, Preview, State};
use rush_core::text::{Line, Span, width};
use rush_core::time::{Ms, now};
use rush_draw::{App, Button, Canvas, Event, Key, KeyMods, MouseKind, Msg, Sender, Step};
use rush_fleet::Predicates;

use crate::config::Config;
use crate::frame::edgepush::EdgePush;
use crate::frame::ext::Kept;
use crate::frame::framehold::Hold;
use crate::frame::overlayclick::OverlayHit;
use crate::frame::{Confirm, Editor, Ext, Mouse, Out, PlaceId, Press, Sheet, uistall};
use crate::list::ListLine;
use crate::screens::list_extras::peek;
use crate::session::{self, SessionView};
use crate::style::*;
use crate::worker::{self, Fleet, Note};

pub struct Model {
    /// The adapters rush knows: every agent's words and reading go through
    /// them (rush_core::adapter::global() unless set).
    pub reg: Registry,
    pub agents: Vec<Agent>,
    pub roots: Vec<String>,
    pub previews: HashMap<String, Preview>,
    pub cfg: Config,
    pub now: Ms,
    pub tick: usize,
    pub w: usize,
    pub h: usize,
    pub lines: Vec<ListLine>,
    pub order: Vec<usize>,
    pub group_of: HashMap<String, String>,
    pub sel: String,
    pub shown: String,
    pub hover: String,
    pub scroll: usize,
    pub row_keys: Vec<String>,
    pub list_top: usize,
    pub folds: HashMap<String, bool>,
    pub titles: HashMap<String, String>,
    pub run_since: HashMap<String, Ms>,
    pub run_init: bool,
    pub loaded: bool,
    pub account: String,
    pub launch_dir: String,
    pub version: String,
    /// A fixed clock, for tests; None reads the real one.
    pub clock: Option<Ms>,
    /// What rush last said on the line under the box, when, and whether
    /// it was a failure, which stays till a key (frame/messages.rs).
    pub(crate) flash: Option<(String, Ms, bool)>,
    /// The log of everything rush said, the last LOG_KEPT (#log).
    pub(crate) said: Vec<crate::frame::messages::LogLine>,
    quit_armed: Ms,
    pub(crate) help: bool,
    pub(crate) term_focused: bool,
    pub session: Option<SessionView>,
    /// The picked agent's Session beside the list, on a screen split
    /// between them (list_extras/split.rs); the list keeps the keys.
    pub pane: Option<SessionView>,
    /// The Session has the whole screen because you pushed it there
    /// (ctrl+6 from the split, shift+←, #view agent): Go's m.full.
    pub full: bool,
    /// The list is being drawn while the Session beside it has the keys:
    /// its box says how to come back (promptLines' sessionFocused).
    pub(crate) keys_beside: bool,
    /// The Session is being drawn beside the list without the keys: its
    /// box and keys say how to give them to it (paneDock's !paneFocus).
    pub(crate) pane_idle: bool,
    /// The opened Session is being drawn beside the list rather than on
    /// the whole screen (paneAlone false).
    pub(crate) in_pane: bool,
    /// The screen's width while `in_pane` narrows `w` to the pane's.
    pub(crate) screen_w: usize,
    pub(crate) tx: Option<Sender<Note>>,
    /// The place showing (Go's view), and Agents' page (0 its list).
    pub place: PlaceId,
    pub agents_page: usize,
    /// The extension points: places, pages, hooks (frame/ext.rs).
    pub ext: Ext,
    /// Screens' own state, by type (Model::kept).
    pub kept: Kept,
    /// A panel over the screen with every key (frame/sheet.rs).
    pub sheet: Option<Box<dyn Sheet>>,
    /// A question over everything (frame/confirm.rs).
    pub confirm: Option<Confirm>,
    /// The Agents box's text (Go's input, back, anchor).
    pub input: Editor,
    /// The box over the screen as last drawn, for the mouse.
    pub over: OverlayHit,
    /// Where the open sheet's body landed on screen.
    pub sheet_at: (usize, usize),
    /// The header tab under the pointer: (index, is a page).
    pub top_hover: Option<(usize, bool)>,
    /// The tabs' and pages' rows as last drawn, for the mouse.
    pub header_text: [String; 2],
    /// Arrows pushed into a box's edge.
    pub edge: EdgePush,
    /// Text for the clipboard with the next frame.
    pub pending_copy: Option<String>,
    /// Frames for background work, held a quarter second apart.
    hold: Hold,
    /// A wake-up a screen asked for (Model::wake_at).
    wake: Option<Instant>,
    r#gen: u64,
    title: String,
    /// A spinner or clock was drawn: the next frame is due in a second.
    spinning: bool,
    ticked: Instant,
    asked: HashMap<String, Ms>,
    /// Another rush leads: this one leaves rush's own housekeeping to it
    /// (lead.rs; RUSH_FOLLOW).
    pub follows: bool,
}

impl Model {
    /// A model with no workers: fed by hand (tests, rush --render).
    pub fn new(version: &str) -> Model {
        Model {
            reg: global().clone(),
            agents: Vec::new(),
            roots: Vec::new(),
            previews: HashMap::new(),
            cfg: Config::default(),
            now: now(),
            tick: 0,
            w: 80,
            h: 24,
            lines: Vec::new(),
            order: Vec::new(),
            group_of: HashMap::new(),
            sel: String::new(),
            shown: String::new(),
            hover: String::new(),
            scroll: 0,
            row_keys: Vec::new(),
            list_top: 0,
            folds: HashMap::new(),
            titles: HashMap::new(),
            run_since: HashMap::new(),
            run_init: false,
            loaded: false,
            account: "default".into(),
            // The shell's word for now, as Go's newModel (model.go:461):
            // the loop asks the kernel (bars::root_launch_dir).
            launch_dir: std::env::var("PWD").unwrap_or_default(),
            version: version.to_string(),
            clock: None,
            flash: None,
            said: Vec::new(),
            quit_armed: 0,
            help: false,
            term_focused: true,
            session: None,
            pane: None,
            full: false,
            keys_beside: false,
            pane_idle: false,
            in_pane: false,
            screen_w: 0,
            tx: None,
            place: PlaceId::Agents,
            agents_page: 0,
            ext: crate::frame::ext(),
            kept: Kept::default(),
            sheet: None,
            confirm: None,
            input: Editor::default(),
            over: OverlayHit::default(),
            sheet_at: (0, 0),
            top_hover: None,
            header_text: Default::default(),
            edge: EdgePush::default(),
            pending_copy: None,
            hold: Hold::default(),
            wake: None,
            r#gen: 0,
            title: String::new(),
            spinning: false,
            ticked: Instant::now(),
            asked: HashMap::new(),
            follows: crate::lead::follow_env(),
        }
    }

    /// A model whose workers send on tx.
    pub fn with_workers(version: &str, tx: Sender<Note>) -> Model {
        let mut m = Model::new(version);
        m.tx = Some(tx);
        crate::chores::start(&mut m);
        m
    }

    /// Takes a reading of the list.
    pub fn set_fleet(&mut self, f: Fleet) {
        self.now = self.clock.unwrap_or_else(now);
        self.agents = f.agents;
        crate::hosted::snap(self);
        self.roots = f.roots;
        // Forget agents that are gone.
        let keys: HashSet<&str> = self.agents.iter().map(|a| a.key.as_str()).collect();
        self.asked.retain(|k, _| keys.contains(k.as_str()));
        // Live previews come with the reading; a picked one (read wider)
        // stays while it's as new, else it's read again.
        self.previews.retain(|k, _| self.asked.contains_key(k));
        for (k, p) in f.previews {
            if let Some(old) = self.previews.get(&k)
                && old.at >= p.at
            {
                continue;
            }
            self.asked.remove(&k);
            self.previews.insert(k, p);
        }
        self.loaded = true;
        self.rebuild();
        self.ask_preview();
        self.follow_tail();
        for i in 0..self.ext.on_fleet.len() {
            let f = self.ext.on_fleet[i];
            f(self);
        }
        crate::hosted::pin(self);
    }

    /// State a screen keeps between calls, made on first use: one value
    /// per type.
    pub fn kept<T: Default + 'static>(&mut self) -> &mut T {
        self.kept.get::<T>()
    }

    /// Asks to be woken at `at` with nothing else happening (a spinner's
    /// next frame, a flash going); the earliest asked wins, once.
    pub fn ask_wake(&mut self, at: Instant) {
        self.wake = Some(self.wake.map_or(at, |w| w.min(at)));
    }

    /// The open conversation, told what the list says of its agent: a
    /// live turn ended once the agent has stopped (Go's followTail).
    fn follow_tail(&mut self) -> bool {
        let now = self.clock.unwrap_or_else(now);
        let mut moved = false;
        for v in [self.session.as_mut(), self.pane.as_mut()]
            .into_iter()
            .flatten()
        {
            if let Some(a) = self.agents.iter().find(|a| a.key == v.key) {
                moved |= v.follow(a, now);
            }
        }
        moved
    }

    /// The agent whose card is open: the selection, or on a heading the
    /// agent picked before it.
    pub fn focused(&self) -> Option<usize> {
        let key = if self.sel.starts_with('§') {
            &self.shown
        } else {
            &self.sel
        };
        self.order
            .iter()
            .copied()
            .find(|&i| self.agents[i].key == *key)
    }

    /// Picks the row with key (a heading's key starts with §).
    pub(crate) fn select(&mut self, key: String) {
        if !self.sel.is_empty() && !self.sel.starts_with('§') {
            self.shown = self.sel.clone();
        }
        self.sel = key;
        self.ask_preview();
    }

    /// The picked agent's preview, read on a worker: once, and again when
    /// its transcript moves on.
    fn ask_preview(&mut self) {
        let Some(tx) = &self.tx else { return };
        let Some(i) = self.selected() else { return };
        let a = &self.agents[i];
        if a.job.transcript_path.is_empty() {
            return;
        }
        if self.asked.contains_key(&a.key) {
            return;
        }
        self.asked.insert(a.key.clone(), now());
        worker::spawn_preview(
            tx.clone(),
            self.reg.clone(),
            a.kind.to_string(),
            a.key.clone(),
            a.job.transcript_path.clone(),
        );
    }

    /// Opens agent i's conversation: the one beside the list when it's
    /// that agent's, else read anew.
    pub fn open(&mut self, i: usize) {
        if let Some(v) = self.pane.take_if(|v| v.key == self.agents[i].key) {
            self.session = Some(v);
            self.follow_tail();
            return;
        }
        if let Some(v) = self.read_view(i, false) {
            self.session = Some(v);
            self.follow_tail();
        }
    }

    /// Agent i's conversation as a view, its reading started on a worker
    /// (at once with no loop); None when it can't be read, said why
    /// unless quiet.
    pub(crate) fn read_view(&mut self, i: usize, quiet: bool) -> Option<SessionView> {
        let a = &self.agents[i];
        self.r#gen += 1;
        // Read through the agent's adapter: Claude Code's transcript format
        // framed by convo's Tail, any other's history as it reads it back.
        // One rush hosts is read from its host once connected, even with
        // nothing on disk yet; a live one is its host's before its
        // transcript, as Go's rushPane is (syncHost's openHost; a stopped
        // one is read from its files, openSavedHost).
        let live_host = a.rush && a.pid != 0;
        let source = if live_host {
            Some(session::Source::Hosted)
        } else {
            None
        };
        let source = source
            .or_else(|| session::Source::of(&self.reg, a))
            .or_else(|| {
                let h = crate::screens::session_core::host::hosts_of(self);
                h.is_some_and(|h| h.hosts(a))
                    .then_some(session::Source::Hosted)
            });
        let Some(source) = source else {
            if quiet {
            } else if a.job.transcript_path.is_empty() {
                self.flash("no transcript to show for this one yet");
            } else {
                self.flash("rush can't read this agent's conversations yet");
            }
            return None;
        };
        let mut v = SessionView::new(a.key.clone(), source, self.r#gen);
        match &self.tx {
            Some(tx) => {
                v.tail = match &v.source {
                    session::Source::Transcript { path, decode } => {
                        worker::spawn_tail(path.clone(), self.r#gen, tx.clone(), decode.clone())
                    }
                    session::Source::History { adapter, session } => {
                        worker::spawn_history(adapter, session, self.r#gen, tx.clone())
                    }
                    session::Source::Hosted | session::Source::Room => None,
                };
            }
            None => v.read_now(),
        }
        Some(v)
    }

    /// The view a worker's reading of r#gen is for: the opened one's, or
    /// the one beside the list.
    fn view_of(&mut self, r#gen: u64) -> Option<&mut SessionView> {
        [self.session.as_mut(), self.pane.as_mut()]
            .into_iter()
            .flatten()
            .find(|v| v.r#gen == r#gen)
    }

    /// The opened Session gives up the keys (leavePane): beside the list
    /// it stays there; filling the screen, the list comes back.
    pub(crate) fn close(&mut self) {
        if crate::hosted::keeps_open(self) {
            // A hosted view's one session: no list to go back to, so esc
            // takes the keys off the box (paneKey's hostedAlone).
            crate::hosted::set_away(self);
            return;
        }
        crate::screens::list_extras::split::leave_pane(self);
    }

    /// Handles a key; Quit when it ends rush. A Mac's Option letters are
    /// alt, then the frame routes it (frame/keys.rs).
    pub fn key(&mut self, code: Key, mods: KeyMods) -> Step {
        let (code, mods) = crate::frame::keyname::mac_option(code, mods);
        self.press_key(&Press::new(code, mods)).step()
    }

    /// The Agents list's own keys (listKey). With the Agents box on screen
    /// (Ext::agents_box) typing goes into it, and keys that edit text are
    /// the box's while it has some.
    pub(crate) fn list_key(&mut self, k: &Press) -> Out {
        let s = k.s.as_str();
        let boxed = self.ext.agents_box;
        let empty = self.input.is_empty();
        let letters = !boxed; // j k g G h l q move and quit only without a box
        match s {
            "up" => self.step(-1),
            "down" => self.step(1),
            "k" if letters => self.step(-1),
            "j" if letters => self.step(1),
            "pgup" => self.step(-10),
            "pgdown" => self.step(10),
            "shift+up" => self.step(-(self.lines.len() as isize)),
            "shift+down" => self.step(self.lines.len() as isize),
            "home" if empty => self.step(-(self.lines.len() as isize)),
            "end" if empty => self.step(self.lines.len() as isize),
            "g" if letters => self.step(-(self.lines.len() as isize)),
            "G" if letters => self.step(self.lines.len() as isize),
            "ctrl+n" => {
                let order = self.order.clone();
                let at = order
                    .iter()
                    .position(|&i| self.agents[i].key == self.sel)
                    .map_or(0, |p| p + 1);
                let next = order[at.min(order.len())..]
                    .iter()
                    .chain(order.iter())
                    .find(|&&i| self.agents[i].needs_you())
                    .copied();
                match next {
                    Some(i) => {
                        let k = self.agents[i].key.clone();
                        self.select(k);
                    }
                    None => self.flash("nothing needs you"),
                }
            }
            "enter" | "right" if empty => {
                if let Some(t) = self.sel.strip_prefix('§').map(str::to_string) {
                    if s == "enter" || self.folded(&t) {
                        self.toggle_fold(&t);
                    }
                } else if let Some(i) = self.selected().filter(|_| peek::peeking(self)) {
                    // Peeking from a Session, enter opens the one picked,
                    // as it was (keys.go:386).
                    peek::open_peeked(self, i);
                } else if self.open_room_for(&self.sel.clone()) {
                    // A room, or one of its agents: the room opens
                    // (keys.go:392), its box saying what's typed in it.
                    let key = self.sel.clone();
                    crate::screens::social::rooms::open_room(self, &key);
                } else if let Some(i) = self.selected() {
                    self.open(i);
                }
            }
            "l" if letters => {
                if let Some(t) = self.sel.strip_prefix('§').map(str::to_string) {
                    if self.folded(&t) {
                        self.toggle_fold(&t);
                    }
                } else if let Some(i) = self.selected() {
                    self.open(i);
                }
            }
            "left" if empty => self.fold_here(),
            "h" if letters => self.fold_here(),
            "[" | "]" if empty => {
                self.step_page(if s == "[" { -1 } else { 1 });
            }
            "?" | "f1" if empty => self.help = true,
            "q" if letters => return Out::Quit,
            // With nothing typed, into the Session, as tab does
            // (keys.go:473).
            "{" | "}" | "tab" if empty => {
                if let Some(i) = self.selected() {
                    self.open(i);
                }
            }
            "esc" if empty => {
                if peek::back(self) {
                    return Out::Draw; // back to the Session you peeked from
                }
                if crate::screens::list_extras::split::preview(self) {
                    // The Session beside the list for now goes (leaveChat).
                    crate::screens::list_extras::split::leave_chat(self);
                    return Out::Draw;
                }
                if now() - self.quit_armed < 2000 {
                    return Out::Quit;
                }
                self.quit_armed = now();
                self.flash("esc again to quit");
            }
            _ => return self.box_key(k).or_idle(),
        }
        Out::Draw
    }

    /// ← on a heading folds it; on an agent with subagents running, tucks
    /// their rows away or brings them back (keys.go:345).
    fn fold_here(&mut self) {
        if let Some(t) = self.sel.strip_prefix('§').map(str::to_string) {
            if !self.folded(&t) {
                self.toggle_fold(&t);
            }
            return;
        }
        if let Some(i) = self.selected()
            && !self.agents[i].subagents.is_empty()
        {
            let k = self.agents[i].key.clone();
            crate::screens::list_extras::subs::toggle(self, &k);
        }
    }

    fn step(&mut self, d: isize) {
        self.move_sel(d);
        self.ask_preview();
    }

    fn session_rows(&self) -> usize {
        self.h
            .saturating_sub(self.top_h() + session::HEAD_H + 1)
            .max(1)
    }

    pub(crate) fn session_key(&mut self, s: &str) -> Step {
        let rows = self.session_rows();
        let (w, now, tick) = (self.w.saturating_sub(3), self.now, self.tick);
        let Some(v) = self.session.as_mut() else {
            return Step::Idle;
        };
        v.sync(w, now, tick);
        match s {
            "esc" | "q" => {
                if !v.sel.is_empty() && s == "esc" {
                    v.sel.clear();
                } else {
                    self.close();
                }
            }
            "up" | "k" => v.move_sel(-1, rows),
            "down" | "j" => v.move_sel(1, rows),
            "enter" | "space" | " " => v.toggle(rows),
            "right" | "l" => {
                if !v.sel.is_empty() {
                    v.o.open.insert(v.sel.clone(), true);
                }
            }
            "left" | "h" => {
                if !v.sel.is_empty() {
                    v.o.open.insert(v.sel.clone(), false);
                }
            }
            "pgup" => v.scroll(rows as isize - 1, rows),
            "pgdown" => v.scroll(-(rows as isize - 1), rows),
            "home" | "g" => v.home(rows),
            "end" | "G" => {
                v.pos = session::Pos::End;
                v.sel.clear();
            }
            "ctrl+o" => v.verbose = !v.verbose,
            _ => return Step::Idle,
        }
        Step::Draw
    }

    pub(crate) fn mouse(&mut self, kind: MouseKind, x: u16, y: u16) -> Step {
        let y = y as usize;
        let rows = self.session_rows();
        let (w, now, tick) = (self.w.saturating_sub(3), self.now, self.tick);
        if let Some(v) = self.session.as_mut() {
            v.sync(w, now, tick);
            match kind {
                MouseKind::ScrollUp => v.scroll(3, rows),
                MouseKind::ScrollDown => v.scroll(-3, rows),
                MouseKind::Down(Button::Left) => {
                    let Some(r) = v.ref_at(y).map(str::to_string) else {
                        return Step::Idle;
                    };
                    v.sel = r;
                    v.toggle(rows);
                }
                _ => return Step::Idle,
            }
            return Step::Draw;
        }
        // The Session beside the list: the wheel scrolls it, a click
        // gives it the keys, as enter does (model.go:1928 focusAt).
        let (lw, pw) = crate::screens::list_extras::split::widths(self);
        if pw > 0 && x as usize > lw {
            let rows = self.h.saturating_sub(self.list_top + 1);
            match kind {
                MouseKind::ScrollUp | MouseKind::ScrollDown => {
                    let up = kind == MouseKind::ScrollUp;
                    let (now, tick) = (self.now, self.tick);
                    if let Some(v) = self.pane.as_mut() {
                        v.sync(pw.saturating_sub(3), now, tick);
                        v.scroll(if up { 3 } else { -3 }, rows);
                    }
                }
                MouseKind::Down(Button::Left) => {
                    if let Some(i) = self.focused() {
                        self.open(i);
                    }
                }
                MouseKind::Moved if !self.hover.is_empty() => self.hover.clear(),
                _ => return Step::Idle,
            }
            return Step::Draw;
        }
        let at = y
            .checked_sub(self.list_top)
            .and_then(|i| self.row_keys.get(i))
            .cloned()
            .unwrap_or_default();
        match kind {
            MouseKind::ScrollUp => self.step(-1),
            MouseKind::ScrollDown => self.step(1),
            MouseKind::Moved => {
                if at == self.hover {
                    return Step::Idle;
                }
                self.hover = at;
            }
            MouseKind::Down(Button::Left) => {
                if at.is_empty() {
                    return Step::Idle;
                }
                if let Some(t) = at.strip_prefix('§') {
                    let t = t.to_string();
                    self.select(at);
                    self.toggle_fold(&t);
                } else if at == self.sel {
                    if let Some(i) = self.selected() {
                        self.open(i);
                    }
                } else {
                    self.select(at);
                }
            }
            _ => return Step::Idle,
        }
        Step::Draw
    }

    /// The window title: where you are, and how many wait on you.
    fn make_title(&self) -> String {
        let where_ = match &self.session {
            _ if self.place != PlaceId::Agents => self.place.name().to_string(),
            Some(v) => self
                .agents
                .iter()
                .find(|a| a.key == v.key)
                .map(|a| one_line(&a.display_name))
                .unwrap_or_else(|| "Agents".into()),
            None => "Agents".into(),
        };
        let n = self
            .agents
            .iter()
            .filter(|a| a.needs_you() || a.halted())
            .count();
        if n > 0 {
            format!("({n}) rush · {where_}")
        } else {
            format!("rush · {where_}")
        }
    }

    /// The frame as lines, top to bottom, h of them: the place showing,
    /// then what's over it (view.go:604 render).
    pub fn frame(&mut self) -> Vec<Line> {
        let h = self.h;
        self.now = self.clock.unwrap_or_else(now);
        crate::screens::list_extras::split::settle(self);
        let mut out = if self.on_place() {
            self.spinning = false;
            self.place_frame()
        } else {
            let mut out = self.header();
            let head = out.len();
            let body_h = h.saturating_sub(head + 1);
            let (lw, pw) = crate::screens::list_extras::split::widths(self);
            if lw > 0 && pw > 0 {
                self.split_frame(&mut out, body_h, lw, pw);
            } else {
                if self.session.is_some() {
                    self.session_frame(&mut out, body_h);
                } else {
                    self.list_frame(&mut out, body_h, lw, true);
                    self.pane = None;
                }
                out.truncate(h.saturating_sub(1));
                while out.len() < h.saturating_sub(1) {
                    out.push(Line::new());
                }
                out.push(self.status_line(self.w));
            }
            out
        };
        out.truncate(h);
        while out.len() < h {
            out.push(Line::new());
        }
        self.overlays(&mut out);
        out.truncate(h);
        out
    }

    /// The list and the Session side by side under the header: the list,
    /// its box and keys lw wide; the Session pw wide beside it, to the
    /// foot of the screen, past a divider; the side without the keys
    /// faded back (listView's split2, paneFrameRow).
    fn split_frame(&mut self, out: &mut Vec<Line>, body_h: usize, lw: usize, pw: usize) {
        use crate::screens::list_extras::split;
        let head = out.len();
        let rows = body_h + 1;
        // The list's side, drawn as the list is with no Session open,
        // its box saying how to come back while the Session has the keys.
        let focused = self.session.take();
        self.keys_beside = focused.is_some();
        // Under the header's rows, for where its parts say they are.
        let mut left = out.clone();
        self.list_frame(&mut left, body_h, lw, false);
        left.drain(..head.min(left.len()));
        left.truncate(body_h);
        while left.len() < body_h {
            left.push(Line::new());
        }
        left.push(self.status_line(lw));
        self.keys_beside = false;
        self.session = focused;
        // The Session's side: the opened one, or the picked agent's for
        // now, drawn as an opened one is, pw wide.
        let pane = if self.session.is_some() {
            self.pane = None;
            self.pane_frame(head, rows, pw)
        } else {
            split::follow(self);
            // An agent whose screen is mirrored, its summary picked: the
            // strip over it, so v brings the screen back (view.go:1225).
            let strip = |m: &mut Model| crate::screens::list_extras::live::summary_strip(m, pw);
            match split::mirror(self, pw, rows) {
                Some(p) => p,
                None => {
                    let strip = strip(self);
                    let top = usize::from(strip.is_some());
                    let mut p = match split::placeholder(self, pw, rows - top) {
                        Some(p) => p,
                        None => {
                            self.session = self.pane.take();
                            self.pane_idle = true;
                            let p = self.pane_frame(head + top, rows - top, pw);
                            self.pane_idle = false;
                            self.pane = self.session.take();
                            p
                        }
                    };
                    if let Some(s) = strip {
                        p.insert(0, s);
                    }
                    p
                }
            }
        };
        let fade_list = self.session.is_some();
        let kind = split::pane_kind(self);
        out.truncate(head);
        out.extend(split::join_rows(
            &left,
            lw,
            &pane,
            pw,
            rows,
            fade_list,
            kind.as_deref(),
            &self.reg,
        ));
    }

    /// The opened Session drawn w wide and rows tall, its row of keys the
    /// last: the screen narrowed to the pane while it's drawn, so every
    /// part of it lays itself out at the pane's width.
    /// head is the rows over it, for where its parts say they are.
    fn pane_frame(&mut self, head: usize, rows: usize, w: usize) -> Vec<Line> {
        crate::screens::list_extras::split::in_pane(self, w, |m| {
            let mut p = vec![Line::new(); head];
            m.session_frame(&mut p, rows.saturating_sub(1));
            p.drain(..head.min(p.len()));
            p.truncate(rows.saturating_sub(1));
            while p.len() < rows.saturating_sub(1) {
                p.push(Line::new());
            }
            p.push(m.status_line(w));
            p
        })
    }

    /// The rows the dock hooks draw under the list or the conversation.
    fn docks(&mut self, session: bool, w: usize) -> Vec<Line> {
        let n = if session {
            self.ext.session_dock.len()
        } else {
            self.ext.list_dock.len()
        };
        let mut out = Vec::new();
        for i in 0..n {
            let f = if session {
                self.ext.session_dock[i]
            } else {
                self.ext.list_dock[i]
            };
            out.extend(f(self, w));
        }
        out
    }

    /// The list w wide, body_h rows, its docks under it; the card of the
    /// picked agent too when card (no Session beside it).
    fn list_frame(&mut self, out: &mut Vec<Line>, body_h: usize, w: usize, card: bool) {
        let dl = self.cfg.dock_lines(self.h);
        let mut dock = Vec::new();
        if card && self.h >= 20 + dl {
            match self.focused() {
                Some(i) => {
                    for l in self.card_lines(i, w.saturating_sub(4), dl) {
                        let mut r = line([plain("  ")]);
                        join(&mut r, l);
                        dock.push(r);
                    }
                }
                None => {
                    dock.push(Line::new());
                    dock.push(line([sp(
                        "  select an agent to see what it is doing",
                        FAINT,
                    )]));
                }
            }
            while dock.len() < 4 + dl {
                dock.push(Line::new());
            }
        }
        let docks = self.docks(false, w);
        let body_h = body_h.saturating_sub(docks.len());
        if body_h < dock.len() + 5 {
            dock.clear();
        }
        let body_h = body_h.saturating_sub(dock.len()).max(3);
        self.list_top = out.len() + 1;
        out.push(self.column_header(w));
        let mut foot = Vec::new();
        for i in 0..self.ext.list_foot.len() {
            let f = self.ext.list_foot[i];
            foot = f(self, w, body_h);
            if !foot.is_empty() {
                break;
            }
        }
        foot.truncate((body_h - 1).saturating_sub(2));
        let rows = self.list_lines(w, body_h - 1 - foot.len());
        out.extend(rows);
        let target = self.list_top - 1 + body_h - foot.len();
        while out.len() < target {
            out.push(Line::new());
        }
        if !foot.is_empty() {
            self.row_keys.resize(body_h - 1 - foot.len(), String::new());
            for (l, k) in foot {
                out.push(l);
                self.row_keys.push(k);
            }
        }
        out.extend(dock);
        out.extend(docks);
        // Only a live agent on screen moves with the clock.
        let shown: HashSet<&str> = self.row_keys.iter().map(String::as_str).collect();
        let live = self
            .agents
            .iter()
            .any(|a| a.job.live() && !a.needs_you() && shown.contains(a.key.as_str()));
        self.spinning = live;
    }

    fn session_frame(&mut self, out: &mut Vec<Line>, body_h: usize) {
        let (w, now, tick) = (self.w, self.now, self.tick);
        let room = match &self.session {
            Some(v) => crate::screens::social::rooms::pane_head(self, &v.key),
            None => return,
        };
        let (beside, idle) = (self.in_pane, self.pane_idle);
        let Some(v) = self.session.as_mut() else {
            return;
        };
        (v.beside, v.idle) = (beside, idle);
        let (a, conn) = match &room {
            Some((a, conn)) => (Some(a), Some(conn.clone())),
            None => (self.agents.iter().find(|a| a.key == v.key), None),
        };
        let mut head = session::header(a, v, w, now, &self.reg, conn);
        let a_live = a.is_some_and(|a| a.job.live());
        // The header's wash is its agent's harness and provider's colours
        // (paneHeader's kind); a room's has none.
        let kind = a
            .filter(|_| room.is_none())
            .map(|a| a.kind.as_str().to_string());
        for i in 0..self.ext.session_head.len() {
            let f = self.ext.session_head[i];
            f(self, &mut head, w);
        }
        if let Some(kind) = kind {
            crate::frame::headerwash::wash_head(&self.reg, &kind, &mut head, w);
        }
        out.extend(head);
        let docks = self.docks(true, self.w);
        let rows = body_h.saturating_sub(session::HEAD_H + docks.len());
        if let Some(v) = self.session.as_mut() {
            v.top = out.len();
        }
        let mut drawn = None;
        for i in 0..self.ext.session_body.len() {
            let f = self.ext.session_body[i];
            if let Some(b) = f(self, w.saturating_sub(3), rows) {
                drawn = Some(b);
                break;
            }
        }
        let Some(v) = self.session.as_mut() else {
            return;
        };
        v.sync(w.saturating_sub(3), now, tick);
        let body = if let Some(b) = drawn {
            b
        } else if !v.read {
            vec![line([sp("reading the conversation…", DIM)])]
        } else if !v.err.is_empty() && v.sess.turns.is_empty() {
            vec![line([sp(format!("couldn't read it: {}", v.err), RED)])]
        } else {
            v.body(rows)
        };
        let n = body.len().min(rows);
        for l in body.into_iter().take(rows) {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            out.push(r);
        }
        self.spinning = v.sess.live().is_some() || a_live;
        if !docks.is_empty() {
            out.extend(std::iter::repeat_n(Line::new(), rows - n));
            out.extend(docks);
        }
    }

    /// The bottom row, w wide: a flash for a few seconds, else the keys.
    fn status_line(&self, w: usize) -> Line {
        if self.status_shown()
            && let Some((s, _, err)) = &self.flash
        {
            let c = if *err { RED } else { TEXT };
            return fit(line([plain("  "), sp(s.clone(), c)]), w);
        }
        if !self.on_place()
            && let Some(l) = self.ext.status.iter().find_map(|f| f(self, w))
        {
            return fit(l, w);
        }
        if self.session.is_none()
            && let Some(l) = self.ext.list_hint.iter().find_map(|f| f(self, w))
        {
            return fit(l, w);
        }
        let keys = if self.session.is_some() {
            keys_fit(
                w.saturating_sub(2),
                &[
                    ("↑↓", "pick a row"),
                    ("enter", "open or close it"),
                    ("ctrl+o", "more detail"),
                    ("esc", "back to the list"),
                ],
            )
        } else {
            keys_fit(
                w.saturating_sub(2),
                &[
                    ("enter", "open"),
                    ("↑↓", "choose"),
                    ("ctrl+n", "next that needs you"),
                    ("ctrl+c", "quit"),
                    ("?", "more"),
                ],
            )
        };
        let mut l = line([plain("  ")]);
        join(&mut l, keys);
        fit(l, w)
    }

    /// The keys, in a box over the middle of the screen.
    pub(crate) fn help_over(&self, out: &mut [Line]) {
        let pairs: &[(&str, &str)] = &[
            ("↑↓ j k", "move"),
            ("pgup pgdn", "a page"),
            ("home end", "first, last"),
            ("enter →", "open the agent, or a section"),
            ("←", "fold a section"),
            ("ctrl+n", "the next that needs you"),
            ("esc esc · q", "quit"),
            ("", ""),
            ("in a session", ""),
            ("↑↓", "pick a row"),
            ("enter space", "open or close it"),
            ("pgup pgdn", "scroll"),
            ("end", "follow the conversation"),
            ("ctrl+o", "more detail"),
            ("esc", "back to the list"),
            ("", ""),
            ("ctrl+c", "quit"),
        ];
        let bw = 52.min(self.w.saturating_sub(4));
        let inner = bw.saturating_sub(4);
        let mut rows = vec![line([sp(
            format!("╭─ keys {}╮", "─".repeat(bw.saturating_sub(9))),
            DIM,
        )])];
        for (k, what) in pairs {
            let body = if what.is_empty() {
                line([spb(*k, SUB)])
            } else {
                line([spb(fit_str(k, 14), TEXT), sp(*what, DIM)])
            };
            let mut l = line([sp("│ ", DIM)]);
            join(&mut l, on_ground(body, inner, PANEL_BG));
            l.spans.push(sp(" │", DIM));
            rows.push(l);
        }
        rows.push(line([sp(
            format!("╰{}╯", "─".repeat(bw.saturating_sub(2))),
            DIM,
        )]));
        let top = out.len().saturating_sub(rows.len()) / 2;
        let left = self.w.saturating_sub(bw) / 2;
        for (i, r) in rows.into_iter().enumerate() {
            let Some(base) = out.get_mut(top + i) else {
                break;
            };
            *base = overlay(base, r, left, self.w);
        }
    }

    /// The focused row's card: what it's doing in the title, its latest
    /// words inside, its numbers in a footer.
    fn card_lines(&self, i: usize, w: usize, body_lines: usize) -> Vec<Line> {
        let a = &self.agents[i];
        let empty = Preview::default();
        let p = self.previews.get(&a.key).unwrap_or(&empty);
        let inner = w.saturating_sub(6);
        let edge = |s: &str| sp(s, DIM);
        let row = |l: Line| {
            let mut out = line([edge("│")]);
            let mut body = line([plain("  ")]);
            join(&mut body, fit(l, inner));
            body.spans.push(plain("  "));
            join(&mut out, on_ground(body, inner + 4, PANEL_BG));
            out.spans.push(edge("│"));
            out
        };
        let live = a.job.live();
        let title = if live && !p.tool.is_empty() {
            let arg = one_line(&tildify(&p.tool_arg));
            line([
                sp("● ", ORANGE),
                spb(p.tool.clone(), TEXT),
                plain("  "),
                sp(
                    rush_draw::truncate(&arg, 10.max(inner.saturating_sub(p.tool.len() + 8)), "…")
                        .into_owned(),
                    SUB,
                ),
            ])
        } else if a.job.state == State::Blocked {
            line([spb("waiting on you", YELLOW)])
        } else if a.busy() {
            line([sp("◌ ", ORANGE), sp("background work still running", TEXT)])
        } else if live {
            line([sp("● ", ORANGE), sp("working…", TEXT)])
        } else if a.pid != 0 {
            line([sp("idle · still in memory", DIM)])
        } else {
            line([sp(format!("finished {} ago", age(a.age(self.now))), DIM)])
        };
        let tw = title.width();
        let mut top = line([edge("╭─ ")]);
        join(&mut top, title);
        top.spans
            .push(edge(&format!(" {}╮", "─".repeat(w.saturating_sub(tw + 5)))));
        let mut text = p.text.clone();
        if !live && !a.job.detail.trim().is_empty() && a.job.detail != "stopped" {
            text = a.job.detail.clone();
        }
        if a.job.state == State::Blocked && !a.job.needs.is_empty() {
            text = a.job.needs.clone();
        }
        if text.is_empty() {
            text = "…".into();
        }
        let text = unmark(&one_line(&text));
        // Its blank lines dropped (view.go:1873).
        let wrapped: Vec<String> = wrap(&text, inner)
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let mut body = Vec::new();
        for (n, l) in wrapped.iter().enumerate() {
            if n == body_lines - 1 && wrapped.len() > body_lines {
                body.push(line([
                    sp(
                        rush_draw::truncate(l, inner.saturating_sub(24), "…").into_owned(),
                        TEXT,
                    ),
                    sp("   tab for the full preview", FAINT),
                ]));
                break;
            }
            body.push(line([sp(l.clone(), TEXT)]));
        }
        let mut cells: Vec<Line> = Vec::new();
        if p.context > 0 {
            let model = if p.model.is_empty() {
                &a.spend.model
            } else {
                &p.model
            };
            // Against the window it compacts within, when that's smaller
            // than the model's (view.go:1888 ctxFill).
            let f = rush_core::adapter::fill_of(
                p.context,
                self.reg.context_window(a.kind.as_str(), model),
                a.compaction,
            );
            let pct = f.pct();
            let mut of = format!(" of {}", tokens(f.window));
            if f.compacts() {
                of.push_str(" auto-compact");
            }
            let mut c = line([sp("context ", DIM)]);
            join(&mut c, ctx_bar(pct));
            c.spans.push(plain(" "));
            c.spans.push(sp(format!("{pct:.0}%"), TEXT));
            c.spans.push(sp(of, DIM));
            cells.push(c);
        }
        let u = a.spend.usage;
        if a.spend.cost > 0.0 {
            cells.push(line([sp("spent ", DIM), sp(money(a.spend.cost), TEXT)]));
            cells.push(line([
                sp("out ", DIM),
                sp(tokens(u.output), SUB),
                sp("  cache ", DIM),
                sp(tokens(u.cache_read), SUB),
            ]));
        }
        // Its processes (view.go:1894).
        if a.pid != 0 && a.procs > 0 {
            cells.push(line([
                sp(format!("{} procs ", a.procs), DIM),
                sp(crate::style::mem(a.mem), SUB),
            ]));
        }
        if !live {
            let el = a.elapsed(self.now);
            if el > 0 {
                cells.push(line([sp("ran ", DIM), sp(dur(el), SUB)]));
            }
        }
        let c = crate::list::location_summary(a, &self.roots[i]);
        if !c.is_empty() {
            cells.push(line([sp(c, FAINT)]));
        }
        for f in &self.ext.card_cells {
            if let Some(c) = f(self, a) {
                cells.push(c);
            }
        }
        while body.len() < body_lines {
            body.push(Line::new());
        }
        let mut out = vec![top];
        for l in body.into_iter().take(body_lines) {
            out.push(row(l));
        }
        out.push(line([edge(&format!(
            "├{}┤",
            "─".repeat(w.saturating_sub(2))
        ))]));
        let mut foot = Line::new();
        for (n, c) in cells.into_iter().enumerate() {
            if n > 0 {
                foot.spans.push(edge("   │   "));
            }
            join(&mut foot, c);
        }
        out.push(row(foot));
        out.push(line([edge(&format!(
            "╰{}╯",
            "─".repeat(w.saturating_sub(2))
        ))]));
        out
    }

    /// Whether anything on screen moves with the clock.
    pub fn spinning(&self) -> bool {
        self.spinning
    }
}

fn ctx_bar(pct: f64) -> Line {
    let n = ((pct / 12.5 + 0.5) as usize).min(8);
    let c = if pct >= 80.0 {
        RED
    } else if pct >= 50.0 {
        YELLOW
    } else {
        SUB
    };
    line([sp("━".repeat(n), c), sp("━".repeat(8 - n), FAINT)])
}

/// Greedy word wrap to w cells; a word longer than w is cut.
fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(8);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ').filter(|x| !x.is_empty()) {
        let ww = width(word);
        if !cur.is_empty() && width(&cur) + 1 + ww > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        if ww > w {
            cur.push_str(&rush_draw::truncate(word, w, "…"));
        } else {
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `top` laid over `base` from column x.
fn overlay(base: &Line, top: Line, x: usize, w: usize) -> Line {
    let base = fit(base.clone(), w);
    let left = rush_draw::truncate_line(&base, x, "");
    let tw = top.width();
    let mut out = pad(left, x);
    join(&mut out, top);
    // What shows right of the box: the base's cells past it.
    let skip = x + tw;
    let mut col = 0;
    for s in base.spans {
        let sw = s.width();
        if col + sw <= skip {
            col += sw;
            continue;
        }
        if col >= skip {
            out.spans.push(s);
        } else {
            let mut text = String::new();
            let mut c = col;
            for ch in s.text.chars() {
                let cw = width(ch.encode_utf8(&mut [0; 4]));
                if c >= skip {
                    text.push(ch);
                } else if c + cw > skip {
                    text.push(' ');
                }
                c += cw;
            }
            out.spans.push(Span::new(text, s.style));
        }
        col += sw;
    }
    out
}

/// The local hour at ms.
pub fn local_hour(ms: Ms) -> i32 {
    local_tm(ms).map_or(12, |t| t.tm_hour)
}

/// Seconds east of UTC here, at ms.
pub fn local_zone(ms: Ms) -> i64 {
    local_tm(ms).map_or(0, |t| t.tm_gmtoff)
}

fn local_tm(ms: Ms) -> Option<libc::tm> {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes into the tm we own.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            None
        } else {
            Some(tm)
        }
    }
}

impl Model {
    /// A worker's note taken in: what it changed, and whether that's
    /// worth a frame (background work draws at most every 250 ms:
    /// frame/framehold.rs).
    fn note(&mut self, note: Note) -> Step {
        let step = match note {
            Note::Fleet(f) => {
                self.set_fleet(f);
                Step::Draw
            }
            Note::Preview(k, p) => {
                if self.previews.get(&k) == Some(&*p) {
                    return Step::Idle; // read again, the same
                }
                self.previews.insert(k, *p);
                Step::Draw
            }
            Note::Tail { r#gen, fresh } => match self.view_of(r#gen) {
                Some(v) => {
                    v.take(*fresh);
                    self.follow_tail();
                    Step::Draw
                }
                _ => Step::Idle,
            },
            Note::History {
                r#gen,
                events,
                reset,
            } => match self.view_of(r#gen) {
                Some(v) => {
                    v.take_events(events, reset);
                    self.follow_tail();
                    Step::Draw
                }
                _ => Step::Idle,
            },
            Note::Whole { r#gen, sess } => match self.view_of(r#gen) {
                Some(v) if v.sess.partial => {
                    v.take_whole(*sess);
                    self.follow_tail();
                    Step::Draw
                }
                _ => Step::Idle,
            },
            Note::TailFailed { r#gen, err } => match self.view_of(r#gen) {
                Some(v) => {
                    v.read = true;
                    v.err = err;
                    Step::Draw
                }
                _ => Step::Idle,
            },
            Note::Apply(f) => f(self).step(),
        };
        if step == Step::Draw && !self.hold.background(Instant::now()) {
            return Step::Idle; // drawn when the gap is up
        }
        step
    }
}

impl App<Note> for Model {
    fn update(&mut self, msg: Msg<Note>) -> Step {
        let _busy = uistall::busy(match &msg {
            Msg::Input(_) => "input",
            Msg::App(_) => "note",
        });
        let st = match msg {
            Msg::Input(ev) => match ev {
                Event::Key { code, mods } => self.key(code, mods),
                Event::Mouse { kind, x, y, mods } => {
                    self.mouse_event(&Mouse { kind, x, y, mods }).step()
                }
                Event::Resize(w, h) => {
                    self.w = w as usize;
                    self.h = h as usize;
                    Step::Draw
                }
                Event::FocusGained => {
                    self.term_focused = true;
                    Step::Draw
                }
                Event::FocusLost => {
                    self.term_focused = false;
                    Step::Idle
                }
                Event::Paste(t) => self.paste(&t).step(),
            },
            Msg::App(note) => self.note(note),
        };
        // A hosted view stays on its session (pinHosted).
        if crate::hosted::pin(self) && st == Step::Idle {
            return Step::Draw;
        }
        st
    }

    fn draw(&mut self, c: &mut Canvas) {
        let _busy = uistall::busy("frame");
        self.w = c.width() as usize;
        self.h = c.height() as usize;
        let rows = self.frame();
        for (y, l) in rows.iter().enumerate() {
            c.put_line(0, y as u16, l, self.w as u16);
        }
        self.title = self.make_title();
        self.hold.drew(Instant::now());
    }

    fn wake_at(&self) -> Option<Instant> {
        let at = self.draw_at();
        match crate::chores::due_at(self) {
            Some(c) => Some(at.map_or(c, |a: Instant| a.min(c))),
            None => at,
        }
    }

    fn wake(&mut self) -> Step {
        let now_i = Instant::now();
        // Chores alone draw nothing: what they do lands as its own note.
        crate::chores::run_due(self, now_i);
        if self.draw_at().is_none_or(|t| t > now_i) {
            return Step::Idle;
        }
        self.wake_draw(now_i)
    }

    fn title(&self) -> Option<&str> {
        Some(&self.title)
    }

    fn clipboard(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    fn outside(&mut self) -> Option<rush_draw::Outside<Note>> {
        self.take_outside()
    }
}

impl Model {
    /// When the screen next changes with the clock alone: a spinner's
    /// frame, a flash going, a place or sheet that moves with time.
    fn draw_at(&self) -> Option<Instant> {
        let mut at = None;
        let mut soonest = |t: Instant| at = Some(at.map_or(t, |a: Instant| a.min(t)));
        if self.spinning && self.term_focused {
            let fast = self.session.as_ref().is_some_and(|v| v.r.fast());
            let every = if fast {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(1)
            };
            soonest(self.ticked + every);
        }
        if let Some((_, t, false)) = &self.flash {
            let left = crate::frame::messages::FLASH_FOR - (now() - t);
            if left > 0 {
                soonest(Instant::now() + Duration::from_millis(left as u64));
            }
        }
        if let Some(t) = self.hold.due {
            soonest(t);
        }
        if let Some(t) = self.wake {
            soonest(t);
        }
        // A place, page or sheet showing that moves with the clock.
        if self.on_place() {
            let p = if self.place == PlaceId::Agents {
                self.agents_page
                    .checked_sub(1)
                    .and_then(|i| self.ext.agents_pages.get(i))
                    .and_then(|p| p.as_ref())
            } else {
                self.ext.places[self.place as usize].as_ref()
            };
            if let Some(t) = p.and_then(|p| p.wake_at(self)) {
                soonest(t);
            }
        }
        if let Some(t) = self.sheet.as_ref().and_then(|s| s.wake_at(self)) {
            soonest(t);
        }
        at
    }

    fn wake_draw(&mut self, now_i: Instant) -> Step {
        if self.ticked.elapsed() >= Duration::from_millis(950) {
            self.tick += 1;
            self.ticked = now_i;
            crate::clanker::beat(self);
        } else if self.session.as_ref().is_some_and(|v| v.r.fast()) {
            self.ticked = now_i;
        }
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, t, failed)| !failed && now() - t >= crate::frame::messages::FLASH_FOR)
        {
            self.flash = None;
        }
        if self.wake.is_some_and(|t| t <= now_i) {
            self.wake = None;
        }
        Step::Draw
    }
}
