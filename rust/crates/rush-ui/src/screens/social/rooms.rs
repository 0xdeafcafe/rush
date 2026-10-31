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
use std::sync::{Arc, Mutex};

use rush_convo::Session;
use rush_core::agent::Agent;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_core::tool::{Call, Input, Output};
use rush_room::room::{self as r, Entry, Lister, Room, Rooms, Summary};
use rush_room::{Agents, Registered};

use crate::frame::Out;
use crate::model::Model;
use crate::session::{SessionView, Source};
use crate::style::*;

/// A room's row in the list is keyed by its id after this.
pub const ROOM_KEY_PREFIX: &str = "room:";

/// What the list knows of rooms.
#[derive(Default)]
pub struct RoomRows {
    pub list: Vec<Summary>,
    /// Rooms hidden for good: their ids, from the overlay's hidden keys.
    pub hidden: std::collections::HashSet<String>,
    /// The rooms laid over the list as it was last laid out (roomize).
    pub grouped: Grouped,
    loading: bool,
    lister: Arc<Mutex<Lister>>,
}

impl Model {
    /// Reads the rooms again, off the UI thread, once a reading of the
    /// list lands (loadRooms); the list is laid out again when they
    /// changed.
    pub(crate) fn load_rooms(&mut self) {
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

    /// The room an agent's row is, or is a member of (roomOf).
    pub fn room_of(&self, key: &str) -> Option<&Summary> {
        let rr = self.kept.peek::<RoomRows>()?;
        let id = match key.strip_prefix(ROOM_KEY_PREFIX) {
            Some(id) => id,
            None => rr.grouped.of.get(key)?.as_str(),
        };
        rr.list.iter().find(|s| s.room.id == id)
    }

    /// The rooms as the list was last laid out with them.
    pub fn rooms_grouped(&self) -> Option<&Grouped> {
        self.kept.peek::<RoomRows>().map(|rr| &rr.grouped)
    }

    /// The row picked as an agent, a room's row included (selected over
    /// m.order, which holds the rooms' rows).
    pub fn selected_row(&self) -> Option<Agent> {
        match self.sel.strip_prefix(ROOM_KEY_PREFIX) {
            Some(id) => self.rooms_grouped()?.row.get(id).cloned(),
            None => self.selected().map(|i| self.agents[i].clone()),
        }
    }

    /// Picks the room key is, or is a member of, as opening an agent
    /// does; false when key has nothing to do with a room (openRoomFor).
    pub fn open_room_for(&mut self, key: &str) -> bool {
        let Some(id) = self.room_of(key).map(|s| s.room.id.clone()) else {
            return false;
        };
        self.select(format!("{ROOM_KEY_PREFIX}{id}"));
        true
    }
}

/// Rooms laid over the list's agents: which room each member is in, each
/// room's members, and the rooms opened from each chat (roomize).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grouped {
    /// A member's agent key: its room's id.
    pub of: HashMap<String, String>,
    /// A room's id: its members, by index into the agents.
    pub members: HashMap<String, Vec<usize>>,
    /// The agents that are rows of their own (not a room's member), by
    /// index.
    pub rows: Vec<usize>,
    /// Rooms shown as rows of their own: their ids. Those opened from a
    /// chat that's listed go under it (joined) instead.
    pub rooms: Vec<String>,
    /// A chat's agent key: the rooms opened from it.
    pub joined: HashMap<String, Vec<String>>,
    /// Each shown room's row as an agent the list sorts and places
    /// (roomize's rr.rows): its topic, folder, times, state and its
    /// members' spend.
    pub row: HashMap<String, Agent>,
}

impl Grouped {
    /// The rows that go under a row: a room's members, or a chat's rooms,
    /// each with its members (roomMembers).
    pub fn under(&self, key: &str) -> Vec<Under> {
        if let Some(id) = key.strip_prefix(ROOM_KEY_PREFIX) {
            return self
                .members
                .get(id)
                .map(|v| v.iter().map(|&i| Under::Member(i)).collect())
                .unwrap_or_default();
        }
        let mut out = Vec::new();
        for id in self.joined.get(key).into_iter().flatten() {
            out.push(Under::Room(id.clone()));
            for &i in self.members.get(id).into_iter().flatten() {
                out.push(Under::Member(i));
            }
        }
        out
    }
}

/// A row under another (Grouped::under): a room's, or an agent's by its
/// index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Under {
    Room(String),
    Member(usize),
}

/// The agents the list lays out with each room as one row in place of its
/// members (roomize): only rush's own sessions are members; a hidden
/// room, or one with no members left that isn't running, has no row.
pub fn grouped(
    list: &[Summary],
    agents: &[Agent],
    hidden: &std::collections::HashSet<String>,
) -> Grouped {
    let mut g = Grouped::default();
    if list.is_empty() {
        g.rows = (0..agents.len()).collect();
        return g;
    }
    let mut by_id: HashMap<&str, &Summary> = HashMap::new();
    for s in list {
        for sid in s.sessions.keys() {
            by_id.insert(sid, s);
        }
    }
    let mut chats = std::collections::HashSet::new();
    for (i, a) in agents.iter().enumerate() {
        if a.rush
            && let Some(s) = by_id.get(a.job.id.as_str())
        {
            g.of.insert(a.key.clone(), s.room.id.clone());
            g.members.entry(s.room.id.clone()).or_default().push(i);
            continue;
        }
        g.rows.push(i);
        chats.insert(a.key.as_str());
    }
    for s in list {
        if hidden.contains(&s.room.id) || !s.running && !g.members.contains_key(&s.room.id) {
            continue;
        }
        let mut a = Agent {
            key: format!("{ROOM_KEY_PREFIX}{}", s.room.id),
            display_name: s.room.topic.clone(),
            kind: "room".into(),
            ..Agent::default()
        };
        a.job.name = s.room.topic.clone();
        a.job.cwd = s.room.cwd.clone();
        a.job.created_at = s.room.created.to_ms();
        a.job.updated_at = s.updated.to_ms();
        a.job.state = if s.running {
            rush_core::agent::State::Working
        } else {
            rush_core::agent::State::Done
        };
        for &i in g.members.get(&s.room.id).into_iter().flatten() {
            a.spend.cost += agents[i].spend.cost;
            a.spend.today += agents[i].spend.today;
        }
        g.row.insert(s.room.id.clone(), a);
        if chats.contains(s.room.from.as_str()) {
            g.joined
                .entry(s.room.from.clone())
                .or_default()
                .push(s.room.id.clone());
            continue;
        }
        g.rooms.push(s.room.id.clone());
    }
    g
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

/// Whether key is a room's own row, not an agent (isRoomKey).
pub fn is_room_key(key: &str) -> bool {
    key.starts_with(ROOM_KEY_PREFIX)
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
            crate::screens::list_extras::neighbour(m, &key)
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
    room: Room,
    fold: RoomFold,
    off: u64,
    pub running: bool,
    pub paused: bool,
    /// Counts the rooms opened, so a reading for one before is dropped.
    serial: u64,
    /// Its driver has been seen up: from then on, one not up is over.
    seen: bool,
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
        let r = read_whole(room, now);
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
fn take_poll(
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
    let text = crate::screens::session_core::conn(m, key).input.string();
    let feed = m.kept::<RoomFeed>();
    if feed.key != key || text.trim().is_empty() {
        return Some(Out::Idle);
    }
    let (room, running) = (feed.room.clone(), feed.running);
    if running {
        crate::screens::session_core::conn(m, key)
            .input
            .clear_sent();
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

#[cfg(test)]
mod tests {
    use super::*;
    use rush_room::room::Member;
    use rush_state::Time;

    /// A room just made reads as starting, not over, until its driver is
    /// seen up or STARTING has gone by: poll keeps looking, and the box
    /// doesn't say "this room is over" (S19). Once seen up, a driver gone
    /// is the room over; an old room not up is over at once.
    #[test]
    fn a_room_just_made_is_starting_not_over() {
        const NOW: Ms = 1_790_200_800_000;
        let feed = |created: Ms| RoomFeed {
            room: Room {
                created: Time::from_ms(created),
                ..summary("r9", false).room
            },
            ..RoomFeed::default()
        };
        let mut v = SessionView::new("room:r9".into(), Source::Room, 0);
        let said = |text: &str| Entry {
            from: "Opus".into(),
            kind: r::SAY.into(),
            text: text.into(),
            ..Entry::default()
        };
        // Made a second ago, its driver not up: starting.
        let mut f = feed(NOW - 1_000);
        let (_, again) = take_poll(&mut f, &mut v, Ok((Vec::new(), 0)), false, NOW);
        assert!(f.running && again, "a new room's polling stopped");
        // The driver comes up and speaks; then it's gone: over.
        let (news, again) = take_poll(&mut f, &mut v, Ok((vec![said("tabs")], 40)), true, NOW);
        assert!(news && again && f.running);
        let (news, again) = take_poll(&mut f, &mut v, Ok((Vec::new(), 40)), false, NOW + 500);
        assert!(news && !again && !f.running, "seen up, then gone: over");
        // Never seen up, STARTING on: over.
        let mut f = feed(NOW - 1_000);
        let (_, again) = take_poll(&mut f, &mut v, Ok((Vec::new(), 0)), false, NOW + STARTING);
        assert!(!again && !f.running);
        // An old room opened, its driver not up: over at once.
        let mut f = feed(NOW - 86_400_000);
        let (_, again) = take_poll(&mut f, &mut v, Ok((Vec::new(), 0)), false, NOW);
        assert!(!again && !f.running);
    }

    fn summary(id: &str, running: bool) -> Summary {
        Summary {
            room: Room {
                id: id.into(),
                topic: "tabs or spaces".into(),
                members: vec![
                    Member {
                        name: "Opus".into(),
                        ..Member::default()
                    },
                    Member {
                        name: "Codex".into(),
                        ..Member::default()
                    },
                ],
                ..Room::default()
            },
            running,
            ..Summary::default()
        }
    }

    #[test]
    fn statuses() {
        let mut s = summary("r1", true);
        assert_eq!(room_status(&s).1, "between turns · 2 agents");
        s.speaking = vec!["Opus".into()];
        assert_eq!(room_status(&s).1, "Opus speaking · 2 agents");
        assert!(room_status(&s).0.is_none(), "a spinner where it's live");
        s.speaking.push("Codex".into());
        assert_eq!(room_status(&s).1, "openings · 2 agents");
        s.paused = true;
        assert_eq!(
            room_status(&s),
            (Some(sp("⏸", YELLOW)), "paused · 2 agents".into())
        );
        s.running = false;
        assert_eq!(room_status(&s).1, "over · 2 agents");
        s.verdict = true;
        assert_eq!(room_status(&s).1, "verdict in · 2 agents");
        let l = room_line(&s, 60, false, 0);
        assert!(
            l.plain().starts_with(" ✓ ◈ tabs or spaces")
                && l.plain().ends_with("verdict in · 2 agents")
        );
        assert_eq!(l.width(), 60);
        let c = room_conn(&s).plain();
        assert!(
            c.contains("verdict in") && c.ends_with("alt+1…2 one's own session"),
            "{c}"
        );
    }

    #[test]
    fn groups_members_under_their_room() {
        let mut s = summary("r1", true);
        s.sessions.insert("sid-opus".into(), "Opus".into());
        let mut gone = summary("r2", false);
        gone.room.from = "chat".into();
        let mut joined = summary("r3", true);
        joined.room.from = "claude:x:chat".into();
        let agents = vec![
            Agent {
                key: "claude:x:chat".into(),
                ..Agent::default()
            },
            Agent {
                key: "rush:opus".into(),
                rush: true,
                job: rush_core::agent::Job {
                    id: "sid-opus".into(),
                    ..Default::default()
                },
                ..Agent::default()
            },
        ];
        let g = grouped(&[s, gone, joined], &agents, &Default::default());
        assert_eq!(g.rows, vec![0]);
        assert_eq!(g.of["rush:opus"], "r1");
        assert_eq!(g.members["r1"], vec![1]);
        assert_eq!(
            g.rooms,
            vec!["r1".to_string()],
            "an over room with no members goes"
        );
        assert_eq!(g.joined["claude:x:chat"], vec!["r3".to_string()]);
    }

    fn entry(from: &str, kind: &str, text: &str) -> Entry {
        Entry {
            at: Time::from_ms(1_000),
            from: from.into(),
            kind: kind.into(),
            text: text.into(),
            ..Entry::default()
        }
    }

    // A room's log reads as a conversation: you as yours, each agent a turn
    // of its own under its name, the verdict last.
    #[test]
    fn folds_a_log() {
        let room = summary("r1", true).room;
        let mut f = RoomFold::new(room);
        let mut s = Session::new();
        let mut turn = entry("Opus", r::TURN, "");
        turn.round = 2;
        let mut say = entry("Opus", r::SAY, "tabs, obviously");
        say.ready = "yes".into();
        let mut tool = entry("Codex", r::TOOL, "Bash");
        tool.id = "t1".into();
        tool.detail = "rg tabs".into();
        let mut done = entry("Codex", r::DONE, "");
        done.id = "t1".into();
        let mut verdict = entry("room", r::VERDICT, "Opus: tabs\nCodex: spaces");
        verdict.detail = "every member signed".into();
        for e in [
            entry(r::USER, r::SAY, "settle it"),
            turn,
            say,
            tool,
            done,
            entry("Codex", r::SAY, "spaces"),
            entry(r::USER, r::CONTROL, "pause"),
            verdict,
        ] {
            f.apply(&mut s, &e);
        }
        f.end(&mut s, 2_000);
        let froms: Vec<&str> = s.turns.iter().map(|t| t.from.as_str()).collect();
        assert_eq!(froms, ["", "Opus", "Codex", "", "Verdict"], "{froms:?}");
        assert!(
            s.turns[1].prompt.contains("round 2"),
            "{:?}",
            s.turns[1].prompt
        );
        assert_eq!(s.turns[3].prompt, "/pause");
        assert!(!f.paused, "the verdict ends the pause");
        assert!(s.live().is_none(), "every turn closed");
        assert_eq!(
            room_call(&{
                let mut e = entry("Codex", r::TOOL, "Bash");
                e.detail = "ls".into();
                e
            })
            .input
            .command,
            "ls"
        );
    }

    // roomfeed.go: a room's row shows its log beside the list as a
    // conversation; enter opens it, what's typed is said in the room,
    // ctrl+x pauses it, and alt+n names a member not in the list.
    #[test]
    fn a_rooms_pane_shows_its_log() {
        let mut m = crate::screens::social::feed::tests::model(200, 30);
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let rooms = Rooms::new();
        let room = rooms
            .create(summary("r1", true).room, || "r1".into())
            .unwrap();
        for e in [
            entry(r::USER, r::SAY, "settle tabs or spaces"),
            entry("Opus", r::SAY, "tabs, obviously"),
            entry("Codex", r::SAY, "spaces, for alignment"),
        ] {
            rooms.post("r1", e).unwrap();
        }
        // Its driver: this process stands in, so the room runs.
        std::fs::write(rooms.pid_path("r1"), std::process::id().to_string()).unwrap();
        let mut s = summary("r1", true);
        s.room = room;
        m.kept::<RoomRows>().list = vec![s];
        m.rebuild();
        m.select("room:r1".to_string());
        let text: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
        let text = text.join("\n");
        assert!(text.contains("tabs, obviously"), "{text}");
        assert!(text.contains("spaces, for alignment"), "{text}");
        assert!(text.contains("alt+1…2 one's own session"), "{text}");
        assert!(m.kept::<RoomFeed>().running);

        m.press("enter");
        assert_eq!(m.session.as_ref().map(|v| v.key.as_str()), Some("room:r1"));
        for c in "@Opus why".chars() {
            m.press(&c.to_string());
        }
        m.press("enter");
        m.press("ctrl+x");
        let (es, _) = rooms.read("r1", 0).unwrap();
        let last: Vec<(&str, &str, &str)> = es[3..]
            .iter()
            .map(|e| (e.from.as_str(), e.kind.as_str(), e.text.as_str()))
            .collect();
        assert_eq!(last.len(), 2, "{last:?}");
        assert_eq!(last[1], (r::USER, r::CONTROL, "pause"));
        assert_eq!(m.flash.as_ref().unwrap().0, "paused · /resume carries on");
        m.press("alt+2");
        assert_eq!(
            m.flash.as_ref().unwrap().0,
            "Codex's session isn't in the list"
        );

        // Over, the box says so and keeps what's typed.
        std::fs::remove_file(rooms.pid_path("r1")).unwrap();
        m.kept::<RoomFeed>().running = false;
        m.press("x");
        m.press("enter");
        assert_eq!(
            m.flash.as_ref().unwrap().0,
            "this room is over · #room new opens another"
        );
        rush_state::set_thread_dir(None);
    }

    // roomrows_test.go: closing a room's row asks only to hide it; yes
    // hides the room and its members for good.
    #[test]
    fn closing_a_room_hides_it() {
        let mut m = crate::screens::social::feed::tests::model(120, 30);
        let mut member = Agent {
            key: "default/k1".into(),
            display_name: "Opus".into(),
            ..Agent::default()
        };
        member.job.id = "s1".into();
        member.rush = true;
        let other = Agent {
            key: "default/k2".into(),
            display_name: "other".into(),
            ..Agent::default()
        };
        m.set_fleet(crate::worker::Fleet {
            agents: vec![member, other],
            roots: vec![String::new(); 2],
            ..Default::default()
        });
        let mut s = summary("r1", false);
        s.sessions.insert("s1".into(), "Opus".into());
        m.kept::<RoomRows>().list = vec![s];
        m.rebuild();
        // ctrl+x on the room's own row in the list (keys.go:462, over
        // m.order, which holds the rooms' rows).
        m.select("room:r1".to_string());
        m.press("ctrl+x");
        let c = m.confirm.as_ref().expect("it asks");
        assert_eq!(c.question, "Hide room tabs or spaces?");
        assert!(c.more.is_empty(), "only to hide it");
        m.press("y");
        assert_eq!(m.agents.len(), 1, "its member left the list");
        assert_eq!(m.agents[0].key, "default/k2");
        assert_eq!(m.flash.as_ref().unwrap().0, "hidden: room tabs or spaces");
    }
}
