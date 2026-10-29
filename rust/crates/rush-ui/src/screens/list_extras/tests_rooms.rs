//! Rooms in the list, after Go's internal/ui/roomrows_test.go: a room is
//! one row with its members' sessions under it, a room opened from a chat
//! goes under that chat, and opening either opens the room
//! (roomrows.go:50 roomize, :125 roomMembers; model.go:2251, :2459;
//! view.go:1678; keys.go:392).

use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_room::room::{Member, Room, Summary};

use super::tests::{MIN, NOW, agent, model_of};
use crate::list::ListLine;
use crate::model::Model;
use crate::screens::social::rooms::RoomRows;

fn member(id: &str, name: &str) -> Agent {
    let mut a = agent(
        id,
        &format!("Room · {name} · tabs or spaces"),
        State::Stopped,
        0,
    );
    a.rush = true;
    a.job.updated_at = NOW - 2 * MIN;
    a
}

fn room(id: &str, from: &str, running: bool, sessions: &[(&str, &str)]) -> Summary {
    let mut s = Summary {
        room: Room {
            id: id.into(),
            topic: format!("topic {id}"),
            cwd: "/src/checkout".into(),
            from: from.into(),
            members: sessions
                .iter()
                .map(|(_, n)| Member {
                    name: n.to_string(),
                    ..Member::default()
                })
                .collect(),
            ..Room::default()
        },
        running,
        ..Summary::default()
    };
    s.updated = rush_state::Time::from_ms(NOW - MIN);
    for (sid, n) in sessions {
        s.sessions.insert(sid.to_string(), n.to_string());
    }
    s
}

fn shown(m: &Model) -> Vec<String> {
    m.lines
        .iter()
        .filter_map(|l| match l {
            ListLine::Agent(i) => Some(m.agents[*i].key.clone()),
            ListLine::Room { id, joined } => Some(format!(
                "room:{id}{}",
                if *joined { " (joined)" } else { "" }
            )),
            ListLine::Member { i, joined } => Some(format!(
                "  {}{}",
                m.agents[*i].key,
                if *joined { " (joined)" } else { "" }
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn a_room_is_one_row_with_its_members_under_it() {
    let chat = agent("chat", "plan the release", State::Stopped, 0);
    let mut m = model_of(
        vec![
            chat,
            member("s-opus", "Opus"),
            member("s-codex", "Codex"),
            member("s-gpt", "GPT"),
            // Not rush's own: never a member, however its id matches.
            agent("s-other", "a terminal session", State::Stopped, 0),
        ],
        120,
        40,
    );
    {
        let rr = m.kept::<RoomRows>();
        rr.list = vec![
            room("r1", "", true, &[("s-opus", "Opus"), ("s-codex", "Codex")]),
            room("r2", "default/chat", false, &[("s-gpt", "GPT")]),
            // Over, with no members left: no row.
            room("r3", "", false, &[("gone", "Gone")]),
            room("r4", "", false, &[("s-other", "Other")]),
        ];
    }
    m.rebuild();
    let rows = shown(&m);
    assert!(
        rows.iter().any(|r| r == "room:r1"),
        "the running room has a row: {rows:?}"
    );
    let at = rows.iter().position(|r| r == "room:r1").unwrap();
    assert_eq!(
        rows[at + 1..at + 3],
        ["  default/s-opus", "  default/s-codex"],
        "its members under it: {rows:?}"
    );
    let chat_at = rows.iter().position(|r| r == "default/chat").unwrap();
    assert_eq!(
        rows[chat_at + 1..chat_at + 3],
        ["room:r2 (joined)", "  default/s-gpt (joined)"],
        "the room opened from the chat goes under it: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|r| r.starts_with("room:r3") || r.starts_with("room:r4")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r == "default/s-other"), "{rows:?}");
    // Members aren't counted in their section: the room is.
    let metas: Vec<String> = m
        .lines
        .iter()
        .filter_map(|l| match l {
            ListLine::Section { title, meta, .. } => Some(format!("{title} {meta}")),
            _ => None,
        })
        .collect();
    let total: usize = metas
        .iter()
        .map(|s| {
            s.split_whitespace()
                .find_map(|w| w.parse::<usize>().ok())
                .unwrap_or(0)
        })
        .sum();
    assert_eq!(total, 3, "chat, other and the room r1: {metas:?}");

    // Drawn: the room's topic and how it stands, its members by name.
    let f: Vec<String> = m.frame().iter().map(Line::plain).collect();
    let f = f.join("\n");
    assert!(f.contains("◈ topic r1"), "{f}");
    assert!(f.contains("between turns · 2 agents"), "{f}");
    assert!(f.contains("└ Opus"), "{f}");
    assert!(
        f.contains("◈ topic r2") && f.contains("over · 1 agents"),
        "{f}"
    );

    // Enter on a member opens its room.
    m.sel = "default/s-codex".into();
    assert!(m.open_room_for("default/s-codex"));
    assert_eq!(m.sel, "room:r1");
    assert!(!m.open_room_for("default/chat"), "a chat isn't a room");
}

/// A hidden room has no row (model.go:2268: the overlay's hidden keys).
#[test]
fn a_hidden_room_has_no_row() {
    let mut m = model_of(vec![member("s-opus", "Opus")], 120, 40);
    {
        let rr = m.kept::<RoomRows>();
        rr.list = vec![room("r1", "", true, &[("s-opus", "Opus")])];
        rr.hidden.insert("r1".into());
    }
    m.rebuild();
    assert!(
        !shown(&m)
            .iter()
            .any(|r| r.contains("r1") || r.contains("s-opus")),
        "{:?}",
        shown(&m)
    );
}
