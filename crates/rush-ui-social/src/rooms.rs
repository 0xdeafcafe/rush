//! Rooms as the screens show them: what the list knows of rooms, read off
//! the UI thread once each reading of the list lands; a room as one row
//! with its members' sessions grouped under it; how a room stands in a
//! few words; and a room's log folded into a conversation, so a room is
//! drawn as any agent's is. Port of Go's roomrows.go (roomRows :21,
//! loadRooms :33, roomize :50, roomMembers :125, roomOf :138, roomStatus
//! :170, roomLine :190, roomMemberLine :210, askHideRoom :229, isRoomKey
//! :272, roomConn :276), roomfeed.go (roomFeed :18, syncRoom :29, poll
//! :78, sendRoom :112, roomPaneKey :125, roomFold :178-286,
//! roomCall :280) and room.go:121 (roomChoice).

use std::collections::HashMap;

use rush_convo::Session;
use rush_core::agent::Agent;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_core::tool::{Call, Input, Output};
use rush_room::room::{self as r, Entry, Room, Rooms, Summary};
use rush_room::{Agents, Registered};

use crate::frame::Out;
use crate::model::Model;
use crate::session::{SessionView, Source};
use crate::style::*;

// What the list knows of rooms is the base's (rush_ui_base::rooms).
pub use rush_ui_base::rooms::{
    Grouped, ROOM_KEY_PREFIX, RoomRows, Under, grouped, is_room_key, row_agent,
};

/// Model's side of this module: Model is rush-ui-base's, so its
/// methods here are a trait's.
pub trait ModelRooms {
    fn load_rooms(&mut self);
}

impl ModelRooms for Model {
    /// Reads the rooms again, off the UI thread, once a reading of the
    /// list lands (loadRooms); the list is laid out again when they
    /// changed.
    fn load_rooms(&mut self) {
        let rr = self.kept::<RoomRows>();
        if rr.loading {
            return;
        }
        rr.loading = true;
        let lister = rr.lister.clone();
        self.later(
            move || {
                let mut l = lister.lock().unwrap_or_else(|e| e.into_inner());
                let list = l.list(&Rooms::new());
                let hidden = if list.is_empty() {
                    Default::default()
                } else {
                    rush_state::Store::load()
                        .overlay
                        .hidden
                        .keys()
                        .filter_map(|k| k.strip_prefix(ROOM_KEY_PREFIX))
                        .map(str::to_string)
                        .collect()
                };
                (list, hidden)
            },
            |m, (list, hidden)| {
                let rr = m.kept::<RoomRows>();
                rr.loading = false;
                // Each reading hands finished rooms on a chat their
                // verdict back, and the chat its running room
                // (roomrows.go:40 verdictsBack).
                m.ext.calls.verdicts_back()(m, list.clone());
                let rr = m.kept::<RoomRows>();
                let changed = rr.list != list || rr.hidden != hidden;
                rr.list = list;
                rr.hidden = hidden;
                if !changed {
                    return Out::Idle;
                }
                m.rebuild();
                Out::Draw
            },
        );
    }
}

/// The tick's want: a running rush whose last reading has a room running.
pub fn any_running(m: &Model) -> bool {
    m.sender().is_some() && m.rooms_running()
}

/// The tick's reading of the rooms (Go's loadRooms on each second's
/// refresh); a reading already under way is left to land.
pub fn tick_rooms(m: &mut Model) {
    m.load_rooms();
}

/// A room in a few words: who's speaking, paused, over (roomStatus). The
/// marker is None where a spinner goes.
pub fn room_status(s: &Summary) -> (Option<Span>, String) {
    let n = format!(" · {} agents", s.room.members.len());
    if !s.over.is_empty() || !s.running {
        if s.verdict {
            return (Some(sp("✓", GREEN)), format!("verdict in{n}"));
        }
        return (Some(sp("·", FAINT)), format!("over{n}"));
    }
    if s.paused {
        return (Some(sp("⏸", YELLOW)), format!("paused{n}"));
    }
    match s.speaking.len() {
        1 => (None, format!("{} speaking{n}", s.speaking[0])),
        0 => (None, format!("between turns{n}")),
        _ => (None, format!("openings{n}")),
    }
}

fn spin(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

/// A room's row: its icon, topic and how it stands (roomLine).
pub fn room_line(s: &Summary, w: usize, sel: bool, tick: usize) -> Line {
    let (marker, status) = room_status(s);
    let marker = marker.unwrap_or_else(|| sp(spin(tick + s.room.id.len()), ORANGE));
    let right = line([plain("  "), sp(status, DIM)]);
    let name = format!("◈ {}", s.room.topic);
    let name = if sel {
        line([spb(name, BRIGHT)])
    } else {
        line([sp(name, TEXT)])
    };
    let space = (w as isize - 3 - right.width() as isize).max(4) as usize;
    let mut l = line([plain(" "), marker, plain(" ")]);
    join(&mut l, fit(name, space));
    join(&mut l, right);
    l
}

/// A member's row under its room: its name, its agent and what it's
/// doing (roomMemberLine).
pub fn room_member_line(s: Option<&Summary>, a: &Agent, w: usize, sel: bool, tick: usize) -> Line {
    let (mut name, mut label) = (a.display_name.clone(), String::new());
    if let Some(s) = s {
        for mb in &s.room.members {
            if a.display_name.starts_with(&format!("Room · {} ·", mb.name)) {
                name = mb.name.clone();
                label = mb.label(&Registered);
            }
        }
    }
    let marker = if a.job.live() {
        sp(spin(tick + a.job.id.len()), ORANGE)
    } else if a.pid != 0 {
        sp("◦", DIM)
    } else {
        sp("·", FAINT)
    };
    let style = if sel { bold(BRIGHT) } else { st(SUB) };
    let body = line([
        Span::new(format!("└ {name}"), style),
        plain("  "),
        sp(label, FAINT),
        plain("  "),
        sp(a.job.detail.clone(), DIM),
    ]);
    let mut l = line([plain("   "), marker, plain(" ")]);
    join(&mut l, fit(body, w.saturating_sub(6).max(4)));
    l
}

/// What a room's pane header says where an agent's says how it's
/// connected: how the room stands, and the way to each member
/// (roomConn).
pub fn room_conn(s: &Summary) -> Line {
    let (marker, status) = room_status(s);
    let mut l = Line::new();
    if let Some(m) = marker {
        l.spans.push(m);
        l.spans.push(plain(" "));
    }
    if s.paused && s.running && s.over.is_empty() {
        l.spans.push(spb("PAUSED", YELLOW));
        l.spans.push(sp(" · /resume carries on", DIM));
    } else {
        l.spans.push(sp(status, DIM));
    }
    l.spans.push(sp(
        format!(" · alt+1…{} one's own session", s.room.members.len()),
        DIM,
    ));
    l
}

/// Closing a room's row asks to hide the room and its agents from the
/// list for good, stopping it if it runs; its log stays on disk
/// (askHideRoom).
pub fn ask_hide_room(m: &mut Model, a: &Agent) -> Out {
    let Some(s) = m.room_of(&a.key).cloned() else {
        return Out::Idle;
    };
    let rr = m.kept::<RoomRows>();
    let (list, hidden) = (rr.list.clone(), rr.hidden.clone());
    let members: Vec<String> = grouped(&list, &m.agents, &hidden)
        .members
        .get(&s.room.id)
        .map(|v| v.iter().map(|&i| m.agents[i].key.clone()).collect())
        .unwrap_or_default();
    let key = a.key.clone();
    let mut c = crate::frame::Confirm::new(format!("Hide room {}?", s.room.topic), move |m| {
        let next = if m.sel == key {
            m.ext.calls.neighbour()(m, &key)
        } else {
            String::new()
        };
        let mut hide = members.clone();
        hide.push(key.clone());
        let keys = hide.clone();
        crate::overlay::save(
            m,
            move |o| {
                for k in keys {
                    o.hidden.insert(k, rush_state::Time::now());
                }
                true
            },
            |m, r| {
                if let Err(e) = r {
                    m.fail(&format!("couldn't keep it hidden: {e}"));
                }
                Out::Draw
            },
        );
        // Gone from the list now, not on the next reading.
        for k in &hide {
            if let Some(i) = m.agents.iter().position(|x| x.key == *k) {
                m.agents.remove(i);
                if i < m.roots.len() {
                    m.roots.remove(i);
                }
            }
        }
        m.rebuild();
        if !next.is_empty() {
            m.sel = next;
        }
        m.flash(&format!("hidden: room {}", s.room.topic));
        if s.running {
            let id = s.room.id.clone();
            m.later(
                move || {
                    let e = Entry {
                        from: r::USER.into(),
                        kind: r::CONTROL.into(),
                        text: "stop".into(),
                        ..Entry::default()
                    };
                    Rooms::new().post(&id, e).map_err(|e| e.to_string())
                },
                |m, res| {
                    if let Err(e) = res {
                        m.fail(&e);
                    }
                    Out::Draw
                },
            );
        }
        Out::Draw
    });
    c.yes_text = "hide".into();
    c.detail = "y hides the room and its agents from the list for good, stopping it · its log kept on disk".into();
    m.ask(c);
    Out::Draw
}

/// What's in the box as said in the room: a message, @name, or /pause,
/// /resume, /verdict, /stop, posted off the UI thread (sendRoom).
pub fn send_room(m: &mut Model, room: &Room, running: bool, text: &str) -> Out {
    let text = text.trim();
    if text.is_empty() {
        return Out::Idle;
    }
    if !running {
        m.flash("this room is over · #room new opens another");
        return Out::Draw;
    }
    let (id, e) = (room.id.clone(), r::parse(text, &room.members));
    m.later(
        move || Rooms::new().post(&id, e).map_err(|e| e.to_string()),
        |m, res| {
            if let Err(e) = res {
                m.fail(&e);
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// A room's log as the events a conversation draws: you as your
/// messages, each agent's words and tool calls as a turn of its own
/// headed by its name, the verdict as the last turn (roomFold).
#[derive(Clone, Debug, Default)]
pub struct RoomFold {
    room: Room,
    speaker: String,
    calls: std::collections::HashSet<String>,
    /// A member: the model its session says it runs.
    uses: HashMap<String, String>,
    /// A member: the round of its turn under way.
    rounds: HashMap<String, i64>,
    pub paused: bool,
}

impl RoomFold {
    pub fn new(room: Room) -> RoomFold {
        RoomFold {
            room,
            ..RoomFold::default()
        }
    }

    /// Closes the turn being drawn.
    pub fn end(&mut self, s: &mut Session, now: Ms) {
        if s.live().is_some() {
            s.apply(
                &Event::TurnEnd(TurnEnd {
                    reason: "done".into(),
                    ..TurnEnd::default()
                }),
                now,
            );
        }
        self.speaker.clear();
    }

    /// Starts a turn that isn't yours: from, saying what about.
    fn open(&mut self, s: &mut Session, from: &str, about: &str, now: Ms) {
        self.end(s, now);
        s.sent(about.to_string(), Vec::new(), now);
        if let Some(t) = s.turns.last_mut() {
            t.from = from.to_string();
        }
    }

    fn say(s: &mut Session, text: String, now: Ms) {
        s.apply(
            &Event::Message(Message {
                role: "assistant".into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text,
                    ..Part::default()
                }],
                ..Message::default()
            }),
            now,
        );
    }

    pub fn apply(&mut self, s: &mut Session, e: &Entry) {
        let now = e.at.to_ms();
        if e.from == r::USER {
            let mut text = e.text.clone();
            if e.kind == r::CONTROL {
                text = format!("/{}", e.text);
                self.paused = e.text == "pause";
            }
            self.end(s, now);
            s.sent(text, Vec::new(), now);
            return;
        }
        // An agent's words: in a turn of its own, under its name.
        let speak = |f: &mut RoomFold, s: &mut Session| {
            if f.speaker != e.from {
                let about = f.header(e);
                f.open(s, &e.from, &about, now);
                f.speaker = e.from.clone();
            }
        };
        match e.kind.as_str() {
            r::USES => {
                self.uses.insert(e.from.clone(), e.text.clone());
            }
            r::TURN => {
                self.rounds.insert(e.from.clone(), e.round);
            }
            r::NOTE => {
                speak(self, s);
                Self::say(s, e.text.clone(), now);
            }
            r::TOOL => {
                speak(self, s);
                let c = room_call(e);
                if self.calls.contains(&e.id) {
                    s.apply(
                        &Event::CallUpdated(Box::new(rush_core::event::CallUpdated { call: c })),
                        now,
                    );
                    return;
                }
                self.calls.insert(e.id.clone());
                s.apply(
                    &Event::Message(Message {
                        role: "assistant".into(),
                        parts: vec![Part {
                            kind: PartKind::ToolCall,
                            call: Some(Box::new(c)),
                            ..Part::default()
                        }],
                        ..Message::default()
                    }),
                    now,
                );
            }
            r::DONE => s.apply(
                &Event::Message(Message {
                    role: "user".into(),
                    parts: vec![Part {
                        kind: PartKind::ToolResult,
                        output: Some(Box::new(Output {
                            call_id: e.id.clone(),
                            is_error: e.failed,
                            ..Output::default()
                        })),
                        ..Part::default()
                    }],
                    ..Message::default()
                }),
                now,
            ),
            r::SAY => {
                speak(self, s);
                let mut text = e.text.clone();
                match e.ready.as_str() {
                    "yes" => text.push_str("\n\n✓ ready to sign"),
                    "no" => text.push_str("\n\n… not settled"),
                    _ => {}
                }
                Self::say(s, text, now);
                self.end(s, now);
            }
            r::PAUSED => {
                speak(self, s);
                Self::say(s, format!("⏸ {}", e.text), now);
                self.end(s, now);
            }
            r::VERDICT => {
                let about = format!("each agent's final position · {}", e.detail);
                let about = about.strip_suffix(" · ").unwrap_or(&about).to_string();
                self.open(s, "Verdict", &about, now);
                let mut b = String::new();
                for line in e.text.split('\n') {
                    let (name, pos) = line.split_once(": ").unwrap_or((line, ""));
                    b.push_str(&format!("- **{name}:** {pos}\n"));
                }
                Self::say(s, b, now);
                self.end(s, now);
                self.paused = false;
            }
            r::INFO | r::END => {
                self.open(s, "room", &e.text, now);
                self.end(s, now);
                if e.kind == r::END {
                    self.paused = false;
                }
            }
            _ => {}
        }
    }

    /// What a member's turn is headed with, beside its name: its agent
    /// and the round.
    fn header(&self, e: &Entry) -> String {
        let mut h = Vec::new();
        for mb in self.room.members.iter().filter(|mb| mb.name == e.from) {
            let label = match self.uses.get(&mb.name) {
                Some(md) if !md.is_empty() => {
                    let kind = mb.kind();
                    format!(
                        "{} · {}",
                        Registered.model_name(&kind, md),
                        Registered.harness_label(&kind)
                    )
                }
                _ => mb.label(&Registered),
            };
            h.push(label);
        }
        if let Some(&n) = self.rounds.get(&e.from)
            && n > 0
        {
            h.push(format!("round {n}"));
        }
        h.join(" · ")
    }
}

/// The room a pane shows: its log, read off the UI thread and folded into
/// the pane's Session, so a room is drawn as any agent's conversation is
/// (roomFeed).
#[derive(Default)]
pub struct RoomFeed {
    /// The pane's key: room:<id>; empty for none.
    pub key: String,
    #[doc(hidden)]
    pub room: Room,
    #[doc(hidden)]
    pub fold: RoomFold,
    #[doc(hidden)]
    pub off: u64,
    pub running: bool,
    pub paused: bool,
    /// Counts the rooms opened, so a reading for one before is dropped.
    #[doc(hidden)]
    pub serial: u64,
    /// Its driver has been seen up: from then on, one not up is over.
    #[doc(hidden)]
    pub seen: bool,
}

/// How long a room just made counts as starting while its driver isn't
/// up yet: the driver writes driver.pid once it runs, which can be after
/// the pane's first looks (S19). Go looks once more after 300ms.
pub const STARTING: Ms = 10_000;

impl RoomFeed {
    /// Whether the room is starting rather than over: made moments ago,
    /// its driver not seen up yet.
    fn starting(&self, now: Ms) -> bool {
        !self.seen && starting(&self.room, now)
    }
}

/// Whether room was made under STARTING ago.
fn starting(room: &Room, now: Ms) -> bool {
    now - room.created.to_ms() < STARTING
}

/// A room's log from off, folded as far as it goes: the session, the fold,
/// where the log ended and whether the room runs.
struct Read {
    sess: Session,
    fold: RoomFold,
    off: u64,
    running: bool,
    err: String,
}

/// Reads room's log from the start into a Session (syncRoom's reading).
fn read_whole(room: Room, now: Ms) -> Read {
    let rooms = Rooms::new();
    let (es, off, err) = match rooms.read(&room.id, 0) {
        Ok((es, off)) => (es, off, String::new()),
        Err(e) => (Vec::new(), 0, e.to_string()),
    };
    let running = rooms.running(&room.id);
    // One just made whose driver isn't up yet isn't over.
    let ended = !running && !starting(&room, now);
    let mut sess = Session::new();
    let mut fold = RoomFold::new(room);
    for e in &es {
        fold.apply(&mut sess, e);
    }
    if ended {
        fold.end(&mut sess, now);
    }
    Read {
        sess,
        fold,
        off,
        running,
        err,
    }
}

/// The view of room row key, its log being read (syncRoom); None when
/// the list knows no such room. With no loop (tests, --render) it's read
/// here and now.
pub fn room_view(m: &mut Model, key: &str) -> Option<SessionView> {
    let room = m.room_of(key)?.room.clone();
    let mut v = SessionView::new(key.to_string(), Source::Room, 0);
    let feed = m.kept::<RoomFeed>();
    feed.serial += 1;
    let serial = feed.serial;
    feed.key = key.to_string();
    feed.room = room.clone();
    let now = m.now;
    if m.sender().is_none() {
        // No loop: the worker's part, run here.
        let r = rush_core::offui::off(|| read_whole(room, now));
        take_read(m.kept::<RoomFeed>(), &mut v, r, now);
        return Some(v);
    }
    m.later(
        move || read_whole(room, now),
        move |m, r| {
            let now = m.now;
            if !with_view(m, serial, |f, v| take_read(f, v, r, now)) {
                return Out::Idle; // the pane moved on
            }
            // Always: a room opened as #room makes it has no driver up
            // yet: it's starting until poll sees it up, or STARTING has
            // gone by (roomfeed.go:67-69).
            poll(m, serial);
            Out::Draw
        },
    );
    Some(v)
}

/// Runs f on the room's feed and its view, wherever it's shown (the
/// opened Session, or the pane beside the list); false when the feed has
/// moved on to another room or its view isn't shown.
fn with_view(m: &mut Model, serial: u64, f: impl FnOnce(&mut RoomFeed, &mut SessionView)) -> bool {
    let mut feed = std::mem::take(m.kept::<RoomFeed>());
    let shown = feed.serial == serial
        && match [m.session.as_mut(), m.pane.as_mut()]
            .into_iter()
            .flatten()
            .find(|v| v.key == feed.key)
        {
            Some(v) => {
                f(&mut feed, v);
                true
            }
            None => false,
        };
    *m.kept::<RoomFeed>() = feed;
    shown
}

fn take_read(f: &mut RoomFeed, v: &mut SessionView, r: Read, now: Ms) {
    v.sess = r.sess;
    v.r = rush_convo::Renderer::new();
    v.read = true;
    v.err = r.err;
    f.paused = r.fold.paused;
    f.fold = r.fold;
    f.off = r.off;
    f.seen = r.running;
    f.running = r.running || f.starting(now);
}

/// Folds what poll read into the room's view: entries from the log, and
/// whether its driver is up. A room starting counts as running; one whose
/// driver isn't up otherwise is over, its turn closed. Whether there's
/// anything new to draw, and whether to look again.
#[doc(hidden)]
pub fn take_poll(
    f: &mut RoomFeed,
    v: &mut SessionView,
    r: Result<(Vec<Entry>, u64), String>,
    up: bool,
    now: Ms,
) -> (bool, bool) {
    let (es, ok) = match r {
        Ok((es, off)) => {
            f.off = off;
            (es, true)
        }
        Err(_) => (Vec::new(), false),
    };
    for e in &es {
        f.fold.apply(&mut v.sess, e);
    }
    f.seen |= up;
    f.running = up || f.starting(now);
    if !f.running {
        f.fold.end(&mut v.sess, now);
    }
    f.paused = f.fold.paused;
    let news = !es.is_empty() || !f.running;
    // Over, and all of it read, it's done.
    let again = f.running || !es.is_empty() || !ok;
    (news, again)
}

/// Folds what's new in the room's log into its view, and comes back for
/// more while the room runs and its view is still shown (poll). It waits
/// on the log changing rather than every 300ms: a quiet room costs
/// nothing.
fn poll(m: &mut Model, serial: u64) {
    let feed = m.kept::<RoomFeed>();
    let (id, off) = (feed.room.id.clone(), feed.off);
    m.later(
        move || {
            let rooms = Rooms::new();
            let w = rush_sys::fswait::Watcher::new();
            w.watch(&[rooms.dir(&id), rooms.log_path(&id)]);
            let r = match rooms.read(&id, off) {
                Ok((es, _)) if es.is_empty() => {
                    // Nothing new yet: wait for the log to grow, or a
                    // while, as a room's driver may end without a word.
                    // Not running, it may be one just made whose driver
                    // isn't up yet: look again after Go's 300ms
                    // (roomfeed.go:87), while it's starting.
                    let ms = if rooms.running(&id) { 5000 } else { 300 };
                    w.wait(None, Some(std::time::Duration::from_millis(ms)));
                    rooms.read(&id, off)
                }
                r => r,
            };
            (r.map_err(|e| e.to_string()), rooms.running(&id))
        },
        move |m, (r, running)| {
            let now = m.now;
            let (mut news, mut again) = (false, false);
            let shown = with_view(m, serial, |f, v| {
                (news, again) = take_poll(f, v, r, running, now);
            });
            if !shown {
                return Out::Idle;
            }
            if again {
                poll(m, serial);
            }
            if news { Out::Draw } else { Out::Idle }
        },
    );
}

/// Opens room row key in the whole screen, as enter opens an agent: the
/// pane beside the list's view when it shows the room, else read anew.
pub fn open_room(m: &mut Model, key: &str) {
    if let Some(v) = m.pane.take_if(|v| v.key == key) {
        m.session = Some(v);
        return;
    }
    if let Some(v) = room_view(m, key) {
        m.session = Some(v);
    }
}

/// The room row an open view of room key is, and what its header says
/// where an agent's says how it's connected (roomConn).
pub fn pane_head(m: &Model, key: &str) -> Option<(Agent, Line)> {
    let id = key.strip_prefix(ROOM_KEY_PREFIX)?;
    let s = m.room_of(key)?;
    let a = m.rooms_grouped()?.row.get(id)?.clone();
    Some((a, room_conn(s)))
}

/// A room pane's own keys (roomPaneKey): ctrl+x pauses the room, as it
/// stops an agent's turn, and alt+1…9 opens a member's own session.
pub fn pane_key(m: &mut Model, key: &str, s: &str) -> Out {
    let feed = m.kept::<RoomFeed>();
    if feed.key != key || !is_room_key(key) {
        return Out::Pass;
    }
    if s == "ctrl+x" {
        if feed.paused || !feed.running {
            return Out::Idle;
        }
        let id = feed.room.id.clone();
        m.later(
            move || {
                let e = Entry {
                    from: r::USER.into(),
                    kind: r::CONTROL.into(),
                    text: "pause".into(),
                    ..Entry::default()
                };
                Rooms::new().post(&id, e).map_err(|e| e.to_string())
            },
            |m, res| {
                match res {
                    Ok(()) => m.flash("paused · /resume carries on"),
                    Err(e) => m.fail(&e),
                }
                Out::Draw
            },
        );
        return Out::Draw;
    }
    let b = s.as_bytes();
    if b.len() != 5 || !s.starts_with("alt+") || !(b'1'..=b'9').contains(&b[4]) {
        return Out::Pass;
    }
    let i = (b[4] - b'1') as usize;
    let Some(name) = feed.room.members.get(i).map(|mb| mb.name.clone()) else {
        return Out::Idle;
    };
    let id = feed.room.id.clone();
    let want = format!("Room · {name} ·");
    let member = m
        .rooms_grouped()
        .and_then(|g| g.members.get(&id))
        .into_iter()
        .flatten()
        .map(|&i| &m.agents[i])
        .find(|a| a.display_name.starts_with(&want))
        .map(|a| a.key.clone());
    match member {
        Some(k) => {
            // Its own session, in this view's place.
            m.session = None;
            if let Some(i) = m.agents.iter().position(|a| a.key == k) {
                m.select(k);
                m.open(i);
            }
        }
        None => m.flash(&format!("{name}'s session isn't in the list")),
    }
    Out::Draw
}

/// What's in a room view's box, said in the room (sendPane's room case,
/// sendRoom); None when key isn't a room's. The box is cleared unless
/// the room is over.
pub fn send_from(m: &mut Model, key: &str) -> Option<Out> {
    if !is_room_key(key) {
        return None;
    }
    let text = crate::sess::conn(m, key).input.string();
    let feed = m.kept::<RoomFeed>();
    if feed.key != key || text.trim().is_empty() {
        return Some(Out::Idle);
    }
    let (room, running) = (feed.room.clone(), feed.running);
    if running {
        crate::sess::conn(m, key).input.clear_sent();
    }
    Some(send_room(m, &room, running, &text))
}

/// The call a tool entry is: as made, or rebuilt from its words for a log
/// written before entries kept it (roomCall).
pub fn room_call(e: &Entry) -> Call {
    if let Some(c) = &e.call {
        return c.clone();
    }
    Call {
        id: e.id.clone(),
        name: e.text.clone(),
        title: e.text.clone(),
        input: Input {
            command: e.detail.clone(),
            ..Input::default()
        },
        ..Call::default()
    }
}
