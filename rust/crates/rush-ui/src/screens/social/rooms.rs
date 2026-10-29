//! Rooms as the screens show them: what the list knows of rooms, read off
//! the UI thread once each reading of the list lands; a room as one row
//! with its members' sessions grouped under it; how a room stands in a
//! few words; and a room's log folded into a conversation, so a room is
//! drawn as any agent's is. Port of Go's roomrows.go (roomRows :21,
//! loadRooms :33, roomize :50, roomMembers :125, roomOf :138, roomStatus
//! :170, roomLine :190, roomMemberLine :210, askHideRoom :229, isRoomKey
//! :272, roomConn :276), roomfeed.go (roomFold :178-286, sendRoom :112,
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
use crate::style::*;

/// A room's row in the list is keyed by its id after this.
pub const ROOM_KEY_PREFIX: &str = "room:";

/// What the list knows of rooms.
#[derive(Default)]
pub struct RoomRows {
    pub list: Vec<Summary>,
    loading: bool,
    lister: Arc<Mutex<Lister>>,
}

impl Model {
    /// Reads the rooms again, off the UI thread (loadRooms).
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
                l.list(&Rooms::new())
            },
            |m, list| {
                let rr = m.kept::<RoomRows>();
                rr.loading = false;
                let changed = rr.list != list;
                rr.list = list;
                if changed { Out::Draw } else { Out::Idle }
            },
        );
    }

    /// The room an agent's row is, or is a member of (roomOf).
    pub fn room_of(&self, key: &str) -> Option<&Summary> {
        let list = &self.kept.peek::<RoomRows>()?.list;
        let id = match key.strip_prefix(ROOM_KEY_PREFIX) {
            Some(id) => id.to_string(),
            None => grouped(list, &self.agents).of.get(key)?.clone(),
        };
        list.iter().find(|s| s.room.id == id)
    }
}

/// Rooms laid over the list's agents: which room each member is in, each
/// room's members, and the rooms opened from each chat (roomize).
#[derive(Debug, Default, PartialEq)]
pub struct Grouped {
    /// A member's agent key: its room's id.
    pub of: HashMap<String, String>,
    /// A room's id: its members, by index into the agents.
    pub members: HashMap<String, Vec<usize>>,
    /// The agents that are rows of their own (not a room's member), by
    /// index.
    pub rows: Vec<usize>,
    /// Rooms shown as rows: their ids, those opened from a chat that's
    /// listed going under it (joined) rather than in rows.
    pub rooms: Vec<String>,
    /// A chat's agent key: the rooms opened from it.
    pub joined: HashMap<String, Vec<String>>,
}

/// The agents the list lays out with each room as one row in place of its
/// members (roomize). A room with no members left that isn't running
/// goes; hidden rooms are the caller's to drop.
pub fn grouped(list: &[Summary], agents: &[Agent]) -> Grouped {
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
        if let Some(s) = by_id.get(a.job.id.as_str()) {
            g.of.insert(a.key.clone(), s.room.id.clone());
            g.members.entry(s.room.id.clone()).or_default().push(i);
            continue;
        }
        g.rows.push(i);
        chats.insert(a.key.clone());
    }
    for s in list {
        if !s.running && !g.members.contains_key(&s.room.id) {
            continue;
        }
        if chats.contains(&s.room.from) {
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
    let list = m.kept::<RoomRows>().list.clone();
    let members: Vec<String> = grouped(&list, &m.agents)
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
        m.later(
            move || {
                let mut st = rush_state::Store::load();
                for k in &keys {
                    st.hide(k);
                }
                st.save_overlay().map_err(|e| e.to_string())
            },
            |m, r: Result<(), String>| {
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
                job: rush_core::agent::Job {
                    id: "sid-opus".into(),
                    ..Default::default()
                },
                ..Agent::default()
            },
        ];
        let g = grouped(&[s, gone, joined], &agents);
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
        let row = Agent {
            key: "room:r1".into(),
            ..Agent::default()
        };
        crate::screens::cmdbar::stops::ask_close(&mut m, Some(row));
        let c = m.confirm.as_ref().expect("it asks");
        assert_eq!(c.question, "Hide room tabs or spaces?");
        assert!(c.more.is_empty(), "only to hide it");
        m.press("y");
        assert_eq!(m.agents.len(), 1, "its member left the list");
        assert_eq!(m.agents[0].key, "default/k2");
        assert_eq!(m.flash.as_ref().unwrap().0, "hidden: room tabs or spaces");
    }
}
