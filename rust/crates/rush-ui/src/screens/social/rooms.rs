//! Rooms as the screens show them: what the list knows of rooms, read off
//! the UI thread once each reading of the list lands; a room as one row
//! with its members' sessions grouped under it; how a room stands in a
//! few words; and a room's log folded into a conversation, so a room is
//! drawn as any agent's is. Port of Go's roomrows.go (roomRows :21,
//! loadRooms :33, roomize :50, roomMembers :125, roomOf :138, roomStatus
//! :170, roomLine :190, roomMemberLine :210, isRoomKey :272, roomConn
//! :276), roomfeed.go (roomFold :178-286, sendRoom :112, roomCall :280)
//! and room.go:121 (roomChoice).

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
}
