//! What the list knows of rooms, and a room as one row with its members'
//! sessions grouped under it: the frame lays the list out by these; the
//! rooms are read, drawn and opened by screens/social/rooms.rs. Port of
//! Go's roomrows.go (roomRows :21, roomize :50, roomMembers :125, roomOf
//! :138, isRoomKey :272).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rush_core::agent::Agent;
use rush_room::room::{Lister, Summary};

use crate::model::Model;

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
    pub loading: bool,
    pub lister: Arc<Mutex<Lister>>,
}

impl Model {
    /// Whether a room the list last read is running, so the rooms are
    /// read again each second (chores.rs).
    pub fn rooms_running(&self) -> bool {
        self.kept
            .peek::<RoomRows>()
            .is_some_and(|rr| rr.list.iter().any(|s| s.running))
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
    /// (See row_agent for an agent or a room's row by key.)
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

/// Whether key is a room's own row, not an agent (isRoomKey).
pub fn is_room_key(key: &str) -> bool {
    key.starts_with(ROOM_KEY_PREFIX)
}

/// The agent row key is: an agent's, or a room's (Go's room rows are
/// agents in the list, so a room's pane has an agent's header).
pub fn row_agent<'a>(m: &'a Model, key: &str) -> Option<&'a Agent> {
    if let Some(a) = m.agents.iter().find(|a| a.key == key) {
        return Some(a);
    }
    let id = key.strip_prefix(ROOM_KEY_PREFIX)?;
    m.rooms_grouped()?.row.get(id)
}
