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
use crate::frame::ext::{Act, Kept};
use crate::frame::framehold::Hold;
use crate::frame::overlayclick::OverlayHit;
use crate::frame::{Confirm, Editor, Ext, Mouse, Out, PlaceId, Press, Sheet, uistall};
use crate::list::ListLine;
use crate::session::{self, SessionView};
use crate::style::*;
use crate::worker::{self, Fleet, Note};

pub struct Model {
    /// The adapters rush knows: every agent's words and reading go through
    /// them (rush_core::adapter::global() unless set).
    pub reg: Registry,
    pub agents: Vec<Agent>,
    pub roots: Vec<String>,
    /// Each account's folder with its usage, what its sessions spent
    /// today and how many run, as the list's reading had them (Go's
    /// m.snap.Accounts).
    pub accounts: Vec<rush_fleet::AccountView>,
    pub previews: HashMap<String, Preview>,
    pub cfg: Config,
    pub now: Ms,
    pub tick: usize,
    /// Frames drawn so far: a screen not drawn in the last one wasn't on
    /// screen (the Catch-up view's "since you last looked").
    pub frames: u64,
    pub w: usize,
    pub h: usize,
    /// The rows the open Session is drawn in, its header's with them, as
    /// last drawn (rushPane's h); 0 before it's drawn.
    pub session_h: usize,
    /// The first row of the open Session's docks (its cards, the box and
    /// what's under it) on screen, as last drawn.
    pub session_dock_top: usize,
    pub lines: Vec<ListLine>,
    pub order: Vec<usize>,
    pub group_of: HashMap<String, String>,
    pub sel: String,
    pub shown: String,
    pub hover: String,
    pub scroll: usize,
    /// The pick the wheel scrolled the list away from: while it's still
    /// the pick, the list leaves its rows where the wheel put them (it may
    /// be out of sight) rather than bringing it back, as the conversation
    /// does; keys that move the pick bring it back.
    pub scrolled_from: Option<String>,
    /// The furthest the list's rows scroll, as last drawn.
    pub scroll_most: usize,
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
    /// The same for the loop's deadlines: when a frame is drawn, for a
    /// test stepping through time (Model::wake_on); None is now.
    pub instant: Option<Instant>,
    /// What rush last said on the line under the box, when, and whether
    /// it was a failure, which stays till a key (frame/messages.rs).
    pub flash: Option<(String, Ms, bool)>,
    /// The log of everything rush said, the last LOG_KEPT (#log).
    #[doc(hidden)]
    pub said: Vec<crate::frame::messages::LogLine>,
    pub(crate) quit_armed: Ms,
    pub help: bool,
    pub term_focused: bool,
    /// When you last pressed a key, clicked or pasted in rush: the
    /// runaway guard's away mode asks. 0: not since it started.
    pub last_input: Ms,
    pub session: Option<SessionView>,
    /// The picked agent's Session beside the list, on a screen split
    /// between them (list_extras/split.rs); the list keeps the keys.
    pub pane: Option<SessionView>,
    /// The grid's tiles' Sessions, kept open while their agents are
    /// pinned, by agent key: each tile drawn as its Session (Go's
    /// grid.conns; list_extras/grid.rs).
    pub tiles: HashMap<String, SessionView>,
    /// The Session has the whole screen because you pushed it there
    /// (ctrl+6 from the split, shift+←, #view agent): Go's m.full.
    pub full: bool,
    /// The list is being drawn while the Session beside it has the keys:
    /// its box says how to come back (promptLines' sessionFocused).
    pub keys_beside: bool,
    /// The Session is being drawn beside the list without the keys: its
    /// box and keys say how to give them to it (paneDock's !paneFocus).
    pub pane_idle: bool,
    /// The opened Session is being drawn beside the list rather than on
    /// the whole screen (paneAlone false).
    pub in_pane: bool,
    /// The screen's width while `in_pane` narrows `w` to the pane's.
    pub(crate) screen_w: usize,
    #[doc(hidden)]
    pub tx: Option<Sender<Note>>,
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
    /// The images the Agents box holds, each [Image #N] in its text
    /// (Go's m.imgs).
    pub imgs: crate::imagerefs::ImageRefs,
    /// The box over the screen as last drawn, for the mouse.
    pub over: OverlayHit,
    /// The terminal said it tells ctrl+enter from enter (Go's
    /// keysDisambiguated, model.go:384).
    pub keys_disambiguated: bool,
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
    /// Where the agent being typed to sat at the last rebuild: it stays
    /// there while the draft is there (Go's m.hold, model.go:216).
    pub(crate) row_hold: crate::list::RowHold,
    /// What a frame's parts each ask the same, asked once a frame.
    pub memo: std::rc::Rc<crate::frame::framememo::FrameMemo>,
    /// A wake-up a screen asked for (Model::wake_at).
    wake: Option<Instant>,
    /// When a label the last frame drew changes its text with the clock
    /// alone (style::turns_in).
    turn: Option<Instant>,
    /// When a list group's time runs out for an agent the list shows:
    /// it's laid out again then (rebuild).
    pub(crate) regroup: Option<Instant>,
    /// The last frame showed more than the list, whose times aren't all
    /// noted: drawn again when the chores wake the loop, as often as the
    /// hosts are looked at (chores.rs).
    clock_frame: bool,
    r#gen: u64,
    title: String,
    /// The rows of the frame drawn last (App::draw).
    drawn: Vec<Line>,
    /// A spinner or clock was drawn: the next frame is due in a second.
    spinning: bool,
    ticked: Instant,
    asked: HashMap<String, Ms>,
    /// Another rush leads: this one leaves rush's own housekeeping to it
    /// (lead.rs; RUSH_FOLLOW).
    pub follows: bool,
}

impl Model {
    /// A model with no workers: fed by hand (tests, rush --render), its
    /// extension points filled by register (rush-ui's screens::register,
    /// through rush_ui::new_model).
    pub fn bare(version: &str, register: fn(&mut Ext)) -> Model {
        Model {
            reg: global().clone(),
            agents: Vec::new(),
            roots: Vec::new(),
            accounts: Vec::new(),
            previews: HashMap::new(),
            cfg: Config::default(),
            now: now(),
            tick: 0,
            frames: 0,
            w: 80,
            h: 24,
            session_h: 0,
            session_dock_top: 0,
            lines: Vec::new(),
            order: Vec::new(),
            group_of: HashMap::new(),
            sel: String::new(),
            shown: String::new(),
            hover: String::new(),
            scroll: 0,
            scrolled_from: None,
            scroll_most: 0,
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
            instant: None,
            flash: None,
            said: Vec::new(),
            quit_armed: 0,
            help: false,
            term_focused: true,
            last_input: 0,
            session: None,
            pane: None,
            tiles: HashMap::new(),
            full: false,
            keys_beside: false,
            pane_idle: false,
            in_pane: false,
            screen_w: 0,
            tx: None,
            place: PlaceId::Agents,
            agents_page: 0,
            ext: crate::frame::ext(register),
            kept: Kept::default(),
            sheet: None,
            confirm: None,
            input: Editor::default(),
            imgs: Default::default(),
            over: OverlayHit::default(),
            keys_disambiguated: false,
            sheet_at: (0, 0),
            top_hover: None,
            header_text: Default::default(),
            edge: EdgePush::default(),
            pending_copy: None,
            hold: Hold::default(),
            row_hold: Default::default(),
            memo: Default::default(),
            wake: None,
            turn: None,
            regroup: None,
            clock_frame: false,
            r#gen: 0,
            title: String::new(),
            drawn: Vec::new(),
            spinning: false,
            ticked: Instant::now(),
            asked: HashMap::new(),
            follows: crate::lead::follow_env(),
        }
    }

    /// A model whose workers send on tx, its extension points filled by
    /// register (rush_ui::new_model_with_workers).
    pub fn bare_with_workers(version: &str, tx: Sender<Note>, register: fn(&mut Ext)) -> Model {
        let mut m = Model::bare(version, register);
        m.tx = Some(tx);
        crate::chores::start(&mut m);
        m
    }

    /// Takes a reading of the list.
    pub fn set_fleet(&mut self, f: Fleet) {
        self.now = self.clock.unwrap_or_else(now);
        self.agents = f.agents;
        self.ext.calls.hosted_snap()(self);
        self.roots = f.roots;
        self.accounts = f.accounts;
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
        crate::listcache::took_reading(self, f.priced);
        self.rebuild();
        self.ask_preview();
        self.follow_tail();
        for i in 0..self.ext.on_fleet.len() {
            let f = self.ext.on_fleet[i];
            f(self);
        }
        self.ext.calls.hosted_pin()(self);
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

    /// When the screens last asked to be woken, if they have and it's
    /// still to come.
    pub fn wake_asked(&self) -> Option<Instant> {
        self.wake
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
    pub fn select(&mut self, key: String) {
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
        let key = self.agents[i].key.clone();
        if let Some(v) = self.pane.take_if(|v| v.key == key) {
            self.set_session(v);
            return;
        }
        // The one beside the list, a pinned agent's: kept for its tile.
        let pinned = self.ext.calls.grid_keys()(self);
        if let Some(p) = self.pane.take_if(|v| pinned.contains(&v.key)) {
            self.ext.calls.grid_park()(self, p);
        }
        // A tile's Session, kept open: in at once (gridAdopt).
        if let Some(v) = self.ext.calls.grid_adopt()(self, &key) {
            self.set_session(v);
            return;
        }
        if let Some(v) = self.read_view(i, false) {
            self.set_session(v);
        }
    }

    /// v is the opened Session from now on; the one it replaces, another
    /// agent's, is kept for its tile when that agent is pinned (gridPark).
    fn set_session(&mut self, v: SessionView) {
        let key = v.key.clone();
        let old = self.session.replace(v);
        if old.as_ref().is_none_or(|o| o.key != key) {
            for i in 0..self.ext.on_open.len() {
                let f = self.ext.on_open[i];
                f(self);
            }
        }
        if let Some(old) = old.filter(|o| o.key != key) {
            self.ext.calls.grid_park()(self, old);
        }
        self.follow_tail();
    }

    /// Agent i's conversation as a view, its reading started on a worker
    /// (at once with no loop); None when it can't be read, said why
    /// unless quiet.
    pub fn read_view(&mut self, i: usize, quiet: bool) -> Option<SessionView> {
        // A copy: restoring exchanges below borrows the model.
        let a = &self.agents[i].clone();
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
                crate::host::hosts_of(self)
                    .is_some_and(|h| h.hosts(a))
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
        // A stopped rush session's past exchanges come back with it, read
        // off the UI thread (rushmode.go:2106 RestoreExchanges).
        let journal = (a.rush && !live_host).then(|| a.job.id.clone());
        let mut v = SessionView::new(a.key.clone(), source, self.r#gen);
        v.name_speaker(a);
        // Whether its agent has stopped, for the worker that lays out its
        // end before the view has it (follow says so from then on).
        v.stopped = !a.job.live();
        if let Some(id) = journal.filter(|id| !id.is_empty()) {
            let r#gen = self.r#gen;
            if self.tx.is_none() {
                v.exchanges = rush_host::exchange_journal::read_exchanges(&id).into();
            } else {
                self.later(
                    move || rush_host::exchange_journal::read_exchanges(&id),
                    move |m, recs| match m.view_of(r#gen) {
                        Some(v) if !recs.is_empty() => {
                            v.restore_exchanges(recs);
                            Out::Draw
                        }
                        _ => Out::Idle,
                    },
                );
            }
        }
        // Its end read ahead of time, as the list was moved through
        // (list_extras/predict.rs), whatever agent's: shown now if nothing
        // was written since, else read again (only its end was read) or
        // topped up first on the worker.
        let ahead = match &v.source {
            session::Source::Transcript { .. } | session::Source::History { .. }
                if self.tx.is_some() =>
            {
                self.ext.calls.predict_take()(self, &a.key)
            }
            _ => None,
        };
        let start = ahead.and_then(|h| {
            let resume = match (h.sess.partial, h.fresh) {
                (true, false) => return None,
                (true, true) => worker::Resume::Whole,
                (false, true) => worker::Resume::Follow,
                (false, false) => return Some((h.reading, worker::Resume::TopUp(h.sess))),
            };
            v.seed(*h.sess);
            rush_draw::perf_note(|| format!("tail end gen={} ahead", self.r#gen));
            Some((h.reading, resume))
        });
        match &self.tx {
            Some(tx) => {
                v.tail = match &v.source {
                    session::Source::Hosted | session::Source::Room => None,
                    src => {
                        let (open, resume) = match start {
                            Some((r, resume)) => (worker::Open::Reading(r), resume),
                            None => {
                                // A stopped rush session's folder is its
                                // host's to say, when the list doesn't
                                // (history.go:53).
                                let host = if a.rush {
                                    a.job.id.clone()
                                } else {
                                    String::new()
                                };
                                (
                                    worker::Open::Source(src.clone(), host),
                                    worker::Resume::Start,
                                )
                            }
                        };
                        let spawns = Some((self.reg.clone(), a.kind.as_str().to_string()));
                        worker::spawn_read(
                            open,
                            resume,
                            self.r#gen,
                            tx.clone(),
                            v.drawn.clone(),
                            spawns,
                        )
                    }
                };
            }
            None => {
                v.read_now();
                v.restore_exchanges(Vec::new());
            }
        }
        Some(v)
    }

    /// The view a worker's reading of r#gen is for: the opened one's, the
    /// one beside the list, or a tile's kept open.
    fn view_of(&mut self, r#gen: u64) -> Option<&mut SessionView> {
        let left = self.ext.calls.predict_left();
        [self.session.as_mut(), self.pane.as_mut()]
            .into_iter()
            .flatten()
            .chain(self.tiles.values_mut())
            .chain(left(&mut self.kept))
            .find(|v| v.r#gen == r#gen)
    }

    /// The opened Session gives up the keys (leavePane): beside the list
    /// it stays there; filling the screen, the list comes back.
    pub fn close(&mut self) {
        if self.ext.calls.hosted_keeps_open()(self) {
            // A hosted view's one session: no list to go back to, so esc
            // takes the keys off the box (paneKey's hostedAlone).
            self.ext.calls.hosted_set_away()(self);
            return;
        }
        self.ext.calls.leave_pane()(self);
    }

    /// Handles a key; Quit when it ends rush. A Mac's Option letters are
    /// alt, then the frame routes it (frame/keys.rs).
    pub fn key(&mut self, code: Key, mods: KeyMods) -> Step {
        let (code, mods) = crate::frame::keyname::mac_option(code, mods);
        self.guarded(|m| m.press_key(&Press::new(code, mods)))
            .step()
    }

    /// Picks the agent that has waited longest for you, so a stack of
    /// questions is cleared with ctrl+n and an answer each; "nothing needs
    /// you" when none does (keys.go:595 nextNeedingYou). The list's and
    /// the Session's ctrl+n.
    pub fn next_needing_you(&mut self) {
        let best = self
            .order
            .iter()
            .map(|&i| &self.agents[i])
            .filter(|a| a.needs_you())
            .min_by_key(|a| a.job.updated_at)
            .map(|a| a.key.clone());
        match best {
            Some(k) => {
                self.select(k);
                self.rebuild();
            }
            None => self.flash("nothing needs you"),
        }
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
            // The agent that has waited longest for you, so a stack of
            // questions clears with ctrl+n and an answer each
            // (keys.go:595 nextNeedingYou).
            "ctrl+n" => self.next_needing_you(),
            // → at the end of what's typed, a third time within a second,
            // asks to open the picked agent; a fourth says yes
            // (keys.go:314, edgepush.go:8).
            "right"
                if !empty
                    && self.input.pos == self.input.text.len()
                    && !self.sel.starts_with('§')
                    && self.selected().is_some()
                    && self.edge.push("list-right", Instant::now()) =>
            {
                let name = self.agents[self.selected().expect("picked")]
                    .display_name
                    .clone();
                let mut q = crate::frame::confirm::Confirm::new(format!("Open {name}?"), |m| {
                    m.focus_pane();
                    Out::Draw
                })
                .detail("what you typed stays in this box");
                q.again = "right".into();
                self.ask(q);
            }
            "enter" | "right" if empty => {
                if let Some(t) = self.sel.strip_prefix('§').map(str::to_string) {
                    if s == "enter" || self.folded(&t) {
                        self.toggle_fold(&t);
                    }
                } else if let Some(i) = self.selected().filter(|_| self.ext.calls.peeking()(self)) {
                    // Peeking from a Session, enter opens the one picked,
                    // as it was (keys.go:386).
                    self.ext.calls.open_peeked()(self, i);
                } else if self.open_room_for(&self.sel.clone()) {
                    // A room, or one of its agents: the room opens
                    // (keys.go:392), its box saying what's typed in it.
                    let key = self.sel.clone();
                    self.act(Act::OpenRoom(key));
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
            "?" | "f1" if empty => self.act(Act::OpenKeyHelp),
            "q" if letters => return Out::Quit,
            // With nothing typed, into the Session, as tab does
            // (keys.go:473); ctrl+o (reply) whatever's typed, and ⌘↓ with
            // nothing typed, as in the Finder (keys.go:421, :456 focusPane).
            "{" | "}" | "super+down" if empty => self.focus_pane(),
            // tab goes between the list and the Session whatever's typed,
            // unless it completes a # command, which the picker took first
            // (keys.go:253 switchFocus).
            "tab" => self.focus_pane(),
            "ctrl+o" => self.focus_pane(),
            "esc" if empty => {
                if self.ext.calls.peek_back()(self) {
                    return Out::Draw; // back to the Session you peeked from
                }
                if self.ext.calls.preview()(self) {
                    // The Session beside the list for now goes (leaveChat).
                    self.ext.calls.leave_chat()(self);
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

    /// Into the picked agent's Session, its box taking the keys; a room's
    /// row, or one of its agents, opens the room (focusPane,
    /// rushmode.go:5291).
    fn focus_pane(&mut self) {
        if self.open_room_for(&self.sel.clone()) {
            // A room's row, or one of its agents: the room opens
            // in the pane (switchFocus → focusPane → openRoomFor,
            // rushmode.go:5295), so what's typed next goes to it.
            let key = self.sel.clone();
            self.act(Act::OpenRoom(key));
        } else if let Some(i) = self.selected() {
            self.open(i);
        }
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
            self.ext.calls.sub_toggle()(self, &k);
        }
    }

    fn step(&mut self, d: isize) {
        self.move_sel(d);
        self.ask_preview();
        self.ext.calls.predict_moved()(self, d);
    }

    /// The wheel over the list moves its rows three a turn and leaves the
    /// pick where it is, out of sight if need be, as the conversation's
    /// wheel does. Whether anything moved: a trackpad's momentum at either
    /// end moves nothing, so draws nothing.
    pub fn wheel(&mut self, up: bool) -> bool {
        let was = self.scroll;
        self.scroll = if up {
            self.scroll.saturating_sub(3)
        } else {
            (self.scroll + 3).min(self.scroll_most.max(self.scroll))
        };
        if self.scroll == was {
            return false;
        }
        self.scrolled_from = Some(self.sel.clone());
        // The row under the pointer is another now; it lights again as
        // the pointer moves.
        self.hover.clear();
        true
    }

    fn session_rows(&self) -> usize {
        self.h
            .saturating_sub(self.top_h() + session::HEAD_H + 1)
            .max(1)
    }

    pub fn session_key(&mut self, s: &str) -> Step {
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

    #[doc(hidden)]
    pub fn mouse(&mut self, kind: MouseKind, x: u16, y: u16) -> Step {
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
        let (lw, pw) = crate::split::widths(self);
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
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                if !self.wheel(kind == MouseKind::ScrollUp) {
                    return Step::Idle;
                }
            }
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
        if let Some(q) = self.confirm.as_mut() {
            q.inlined = false; // the box asking it inline says so as it's drawn
        }
        let _drawing = crate::frame::framememo::FrameMemo::drawing(&self.memo);
        self.frames += 1;
        self.frame_rows()
    }

    fn frame_rows(&mut self) -> Vec<Line> {
        let h = self.h;
        self.now = self.clock.unwrap_or_else(now);
        crate::split::settle(self);
        let mut inexact = self.on_place();
        let mut out = if self.on_place() {
            self.spinning = false;
            self.place_frame()
        } else {
            let mut out = self.header();
            let head = out.len();
            let body_h = h.saturating_sub(head + 1);
            let (lw, pw) = crate::split::widths(self);
            if lw > 0 && pw > 0 {
                inexact = true;
                self.split_frame(&mut out, body_h, lw, pw);
            } else {
                if self.session.is_some() {
                    inexact = true;
                    // Tiles pinned to the grid take a column beside the
                    // Session, or a row under it (view.go:1216).
                    let w = self.w;
                    let pane = self.ext.calls.round_pane()(
                        self,
                        w,
                        body_h,
                        0,
                        head,
                        &mut |m, pw, rows| {
                            let full = m.w;
                            m.w = pw;
                            let mut p = vec![Line::new(); head];
                            m.session_frame(&mut p, rows);
                            m.w = full;
                            p.drain(..head.min(p.len()));
                            p
                        },
                    );
                    out.extend(pane);
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
        self.clock_frame = inexact
            || self.sheet.is_some()
            || self.help
            || self.confirm.is_some()
            || self.ext.calls.bar_open()(self)
            || self.ext.calls.key_help_open()(self);
        out
    }

    /// The list and the Session side by side under the header: the list,
    /// its box and keys lw wide; the Session pw wide beside it, to the
    /// foot of the screen, past a divider; the side without the keys
    /// faded back (listView's split2, paneFrameRow).
    fn split_frame(&mut self, out: &mut Vec<Line>, body_h: usize, lw: usize, pw: usize) {
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
        // The Session's side, with the grid's tiles round it.
        let pane = self.ext.calls.round_pane()(self, pw, rows, lw + 1, head, &mut |m, pw, rows| {
            m.split_pane(head, rows, pw)
        });
        let fade_list = self.session.is_some();
        out.truncate(head);
        out.extend(self.ext.calls.join_split()(
            self, &left, lw, &pane, pw, rows, fade_list,
        ));
    }

    /// The Session's side of the split: the opened one, or the picked
    /// agent's for now, drawn as an opened one is, pw wide.
    fn split_pane(&mut self, head: usize, rows: usize, pw: usize) -> Vec<Line> {
        if self.session.is_some() {
            self.pane = None;
            self.pane_frame(head, rows, pw)
        } else {
            self.ext.calls.pane_follow()(self);
            // An agent whose screen is mirrored, its summary picked: the
            // strip over it, so v brings the screen back (view.go:1225).
            let strip = |m: &mut Model| m.ext.calls.summary_strip()(m, pw);
            match self.ext.calls.pane_mirror()(self, pw, rows) {
                Some(p) => p,
                None => {
                    let strip = strip(self);
                    let top = usize::from(strip.is_some());
                    let mut p = match self.ext.calls.pane_placeholder()(self, pw, rows - top) {
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
        }
    }

    /// The opened Session drawn w wide and rows tall, its row of keys the
    /// last: the screen narrowed to the pane while it's drawn, so every
    /// part of it lays itself out at the pane's width.
    /// head is the rows over it, for where its parts say they are.
    fn pane_frame(&mut self, head: usize, rows: usize, w: usize) -> Vec<Line> {
        crate::split::in_pane(self, w, |m| {
            let mut p = vec![Line::new(); head];
            m.session_frame(&mut p, rows.saturating_sub(1));
            p.drain(..head.min(p.len()));
            p.truncate(rows.saturating_sub(1));
            while p.len() < rows.saturating_sub(1) {
                p.push(Line::new());
            }
            // Beside the list, the list's line says what rush said: the
            // Session keeps its keys (rushmode.go:3862-3866 shows it there
            // only with the Session on the whole screen).
            p.push(m.foot_line(w, false));
            p
        })
    }

    /// The open Session drawn w wide and rows tall as a grid's tile, its
    /// top on screen row top, with no row of keys of its own: the screen
    /// narrowed to the tile while it's drawn (gridSession's rushPane).
    pub fn tile_frame(&mut self, top: usize, rows: usize, w: usize) -> Vec<Line> {
        let in_pane = std::mem::replace(&mut self.in_pane, false);
        let out = crate::split::in_pane(self, w, |m| {
            let mut p = vec![Line::new(); top];
            m.session_frame(&mut p, rows);
            p.drain(..top.min(p.len()));
            p.truncate(rows);
            while p.len() < rows {
                p.push(Line::new());
            }
            p
        });
        self.in_pane = in_pane;
        out
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
        let live = self.agents.iter().any(|a| {
            a.job.live() && (a.at_work || !a.needs_you()) && shown.contains(a.key.as_str())
        });
        self.spinning = live;
    }

    fn session_frame(&mut self, out: &mut Vec<Line>, body_h: usize) {
        self.session_h = body_h;
        let (w, now, tick) = (self.w, self.now, self.tick);
        let room = match &self.session {
            Some(v) => self.ext.calls.pane_head()(self, &v.key),
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
            self.session_dock_top = out.len();
            out.extend(docks);
        }
    }

    /// The bottom row, w wide: a flash for a few seconds, else the keys.
    fn status_line(&self, w: usize) -> Line {
        self.foot_line(w, true)
    }

    /// The bottom row, w wide: the keys, or with flash a flash for a few
    /// seconds in their place.
    fn foot_line(&self, w: usize, flash: bool) -> Line {
        if flash
            && self.status_shown()
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
            line([sp(
                format!("finished {} ago", age_since(a.age(self.now))),
                DIM,
            )])
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
            let model: &str = if p.model.is_empty() {
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

    /// Keeps the spinner going if it was before something else was drawn
    /// (a grid tile drawn after the open Session sets it for itself).
    pub fn keep_spinning(&mut self, was: bool) {
        self.spinning |= was;
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

/// The local wall clock at ms, as 15:04, or Mon 15:04 with day.
pub fn clock(ms: Ms, day: bool) -> String {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only the tm it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    let hm = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    if !day {
        return hm;
    }
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!("{} {hm}", DAYS[tm.tm_wday.clamp(0, 6) as usize])
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
    /// A conversation's end as the worker that read it first made it
    /// (Note::End; Note::Seeded when read ahead), with what it worked out
    /// ahead of the frame that shows it.
    fn take_end(
        &mut self,
        r#gen: u64,
        sess: rush_convo::Session,
        warm: Option<Box<crate::worker::Warm>>,
        ahead: bool,
    ) -> Step {
        let Some(v) = self.view_of(r#gen).filter(|v| !v.read) else {
            return Step::Idle;
        };
        let (r, changes, spawns) = match warm {
            Some(w) => (Some((w.r, w.exchanges)), Some(w.changes), w.spawn_steps),
            None => (None, None, None),
        };
        v.take_end(sess, r);
        let key = v.key.clone();
        if let Some((ver, cwd, c)) = changes {
            self.ext.calls.prime_changes()(self, ver, &cwd, c);
        }
        if let Some(sp) = spawns {
            self.ext.calls.prime_spawns()(self, &key, sp);
        }
        let ahead = if ahead { " ahead" } else { "" };
        rush_draw::perf_note(|| format!("tail end gen={}{ahead}", r#gen));
        self.hold.answer(); // "reading the conversation…" goes now
        self.follow_tail();
        Step::Draw
    }

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
            Note::End { r#gen, sess, warm } => self.take_end(r#gen, *sess, warm, false),
            Note::Tail { r#gen, fresh } => match self.view_of(r#gen) {
                Some(v) => {
                    let first = !v.read;
                    let ended = v.take(*fresh);
                    if ended {
                        rush_draw::perf_note(|| format!("tail end gen={}", r#gen));
                    }
                    if first || ended {
                        // "reading the conversation…" goes now, and the
                        // end it was read for shows once all in, not a
                        // quarter second after its first slice.
                        self.hold.answer();
                    }
                    self.follow_tail();
                    Step::Draw
                }
                _ => Step::Idle,
            },
            Note::Seeded { r#gen, sess, warm } => self.take_end(r#gen, *sess, warm, true),
            Note::Whole { r#gen, sess, warm } => match self.view_of(r#gen) {
                Some(v) if v.sess.partial => {
                    let (r, changes, spawns) = match warm {
                        Some(w) => (Some((w.r, w.exchanges)), Some(w.changes), w.spawn_steps),
                        None => (None, None, None),
                    };
                    v.take_whole_drawn(*sess, r);
                    let key = v.key.clone();
                    if let Some((ver, cwd, c)) = changes {
                        self.ext.calls.prime_changes()(self, ver, &cwd, c);
                    }
                    if let Some(sp) = spawns {
                        self.ext.calls.prime_spawns()(self, &key, sp);
                    }
                    self.follow_tail();
                    Step::Draw
                }
                _ => Step::Idle,
            },
            Note::TailFailed { r#gen, err } => match self.view_of(r#gen) {
                Some(v) => {
                    let first = !std::mem::replace(&mut v.read, true);
                    v.err = err;
                    if first {
                        // Its placeholder goes now, for what went wrong.
                        self.hold.answer();
                    }
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
        // A key let go is only push-to-talk's: nothing else changes.
        if let Msg::Input(Event::KeyUp { code, mods }) = msg {
            return match self.ext.calls.dictation_released {
                Some(f) => f(self, code, mods),
                None => Step::Idle,
            };
        }
        let _busy = uistall::busy(match &msg {
            Msg::Input(_) => "input",
            Msg::App(_) => "note",
        });
        // Whatever changes a box, undo can take back (frame/keepundo.rs).
        let g = self.guard_boxes();
        if let Msg::Input(Event::Key { .. } | Event::Mouse { .. } | Event::Paste(_)) = &msg {
            self.last_input = self.clock.unwrap_or_else(rush_core::time::now);
        }
        let st = match msg {
            Msg::Input(ev) => match ev {
                Event::Key { code, mods } => self.key(code, mods),
                // A held key said again types again, as a press does.
                Event::KeyRepeat { code, mods } => match self.ext.calls.dictation_repeated {
                    Some(f) => f(self, code, mods),
                    None => self.key(code, mods),
                },
                Event::KeyUp { .. } => Step::Idle,
                Event::Mouse { kind, x, y, mods } => {
                    self.mouse_event(&Mouse { kind, x, y, mods }).step()
                }
                Event::Resize(w, h) => {
                    self.w = w as usize;
                    self.h = h as usize;
                    self.ext.calls.resized()(self);
                    Step::Draw
                }
                Event::FocusGained => {
                    self.term_focused = true;
                    self.ext.calls.pointer_back()(self);
                    for i in 0..self.ext.on_focus.len() {
                        let f = self.ext.on_focus[i];
                        f(self);
                    }
                    Step::Draw
                }
                Event::FocusLost => {
                    self.term_focused = false;
                    self.ext.calls.pointer_back()(self);
                    // No frame follows: yap is told now that the window
                    // no longer shows a dictation itself.
                    if let Some(f) = self.ext.calls.dictation_after_frame {
                        f(self);
                    }
                    Step::Idle
                }
                Event::Paste(t) => self.paste(&t).step(),
            },
            Msg::App(note) => self.note(note),
        };
        self.keep_undo(g);
        // An #intervene @agent waiting for its chat (model.go:933
        // interveneReady).
        let st = if self.ext.calls.intervene_ready()(self) {
            Step::Draw
        } else {
            st
        };
        // A hosted view stays on its session (pinHosted).
        if self.ext.calls.hosted_pin()(self) && st == Step::Idle {
            return Step::Draw;
        }
        st
    }

    fn draw(&mut self, c: &mut Canvas) {
        let _busy = uistall::busy("frame");
        self.w = c.width() as usize;
        self.h = c.height() as usize;
        crate::style::take_turn();
        rush_sys::sysinfo::unasked();
        let rows = self.frame();
        // The same rows as the frame on screen: it stands as it is.
        if c.can_keep() && rows == self.drawn {
            c.keep();
        } else {
            for (y, l) in rows.iter().enumerate() {
                c.put_line(0, y as u16, l, self.w as u16);
            }
            self.drawn = rows;
        }
        // yap is told the words on screen, read at most once a second.
        if let Some(f) = self.ext.calls.dictation_after_frame {
            f(self);
        }
        self.title = self.make_title();
        let drawn = self.instant.unwrap_or_else(Instant::now);
        self.hold.drew(drawn);
        // A few ms past the turn, so the clock has passed it.
        self.turn = crate::style::take_turn()
            .map(|d| drawn + Duration::from_millis(d as u64) + Duration::from_millis(5));
    }

    fn wake_at(&self) -> Option<Instant> {
        let at = [self.draw_at(), self.turn, self.regroup, self.timer_due()]
            .into_iter()
            .flatten()
            .min();
        match crate::chores::due_at(self) {
            Some(c) => Some(at.map_or(c, |a: Instant| a.min(c))),
            None => at,
        }
    }

    fn wake(&mut self) -> Step {
        self.wake_on(Instant::now())
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
    /// App::wake as at now_i: what the loop's deadline runs when it
    /// passes, for a test to step through an idle minute without waiting
    /// it (tests/threads.rs).
    pub fn wake_on(&mut self, now_i: Instant) -> Step {
        // Chores alone draw nothing: what they do lands as its own note.
        // A frame showing more than the list is drawn again with them.
        let chores = crate::chores::run_due(self, now_i);
        // What waited for its time (Model::after) draws when it says so.
        let timed = self.run_timers(now_i);
        if timed == Step::Quit {
            return Step::Quit;
        }
        let due = |t: Option<Instant>| t.is_some_and(|t| t <= now_i);
        // A label's or a group's time alone neither moves the spinners'
        // tick nor runs what moves with it.
        let clock = due(self.turn) || due(self.regroup) || chores && self.clock_frame;
        if self.turn.is_some_and(|t| t <= now_i) {
            self.turn = None;
        }
        // A group's time ran out: the list laid out again (Go lays it out
        // every second, rebuild on each reading).
        if self.regroup.is_some_and(|t| t <= now_i) {
            self.regroup = None;
            self.now = self.clock.unwrap_or_else(now);
            self.rebuild();
        }
        if !due(self.draw_at()) {
            return if clock || timed == Step::Draw {
                Step::Draw
            } else {
                Step::Idle
            };
        }
        self.wake_draw(now_i)
    }

    /// What each of the loop's deadlines is (App::wake_at's parts), by
    /// name, and whether a spinner turns: for a test to say what wakes.
    #[doc(hidden)]
    pub fn deadlines(&self) -> Vec<(&'static str, Option<Instant>)> {
        vec![
            ("draw", self.draw_at()),
            ("turn", self.turn),
            ("regroup", self.regroup),
            ("timer", self.timer_due()),
            ("chore", crate::chores::due_at(self)),
            ("spinner", self.spinning.then_some(self.ticked)),
            ("asked", self.wake),
        ]
    }

    /// What a key asked for is in, landed from the background: its frame
    /// is drawn at once, not held for the quarter second background work
    /// waits between frames (frame/framehold.rs).
    pub fn answered(&mut self) {
        self.hold.answer();
    }

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
        if now_i.saturating_duration_since(self.ticked) >= Duration::from_millis(950) {
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
