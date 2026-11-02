//! The board in rush's agent list: a section per column, and each card's
//! agent under the card's name.

use std::time::{Duration, SystemTime};

use rush_plugin_sdk::rpc::CODE_DENIED;
use rush_plugin_sdk::sidebar::{self, Agent, MAX_AGENTS, MAX_NAME, Section, Sidebar};
use rush_plugin_sdk::{CallError, Conn};
use serde_json::Value;

use crate::cards::{COLUMNS, Card, links_path, read_cards, sort_column};

/// The board as rush's agent list shows it: a section per column, in board
/// order, and each card's agent under the card's name, in the card's place
/// in its column. Only cards with a conversation are there, as rush knows
/// agents by it.
pub fn payload(cards: &[Card]) -> Sidebar {
    let mut s = Sidebar {
        title: "Kanban".into(),
        ..Default::default()
    };
    for (id, title) in COLUMNS {
        let mut in_col: Vec<&Card> = cards
            .iter()
            .filter(|c| c.column == *id && !c.session_id().is_empty())
            .collect();
        if in_col.is_empty() {
            continue;
        }
        sort_column(&mut in_col);
        s.sections.push(Section {
            title: title.to_string(),
        });
        for (i, c) in in_col.iter().enumerate() {
            let sid = c.session_id();
            if s.agents.contains_key(sid)
                || !sidebar::is_session_id(sid)
                || s.agents.len() >= MAX_AGENTS
            {
                continue;
            }
            let mut name = c.title();
            if name == c.id {
                name.clear(); // no name of its own: rush keeps the agent's
            }
            s.agents.insert(
                sid.to_string(),
                Agent {
                    name: sidebar::clip(&name, MAX_NAME),
                    section: title.to_string(),
                    order: i as i64,
                },
            );
        }
    }
    s
}

/// Keeps rush's list arranged as the board: it looks at links.json every
/// two seconds, and sends the arrangement when it changed. It stops when
/// rush goes, or says the plugin wasn't approved for it.
pub fn show_board(conn: &Conn) {
    let mut seen: Option<SystemTime> = None;
    let mut sent: Option<Sidebar> = None;
    loop {
        let modified = std::fs::metadata(links_path())
            .and_then(|m| m.modified())
            .ok();
        if let Some(m) = modified
            && seen != Some(m)
            && let Ok(cards) = read_cards()
        {
            let p = payload(&cards);
            if sent.as_ref() == Some(&p) {
                seen = Some(m);
            } else {
                match conn.call::<_, Value>("sidebar.set", &p) {
                    Ok(_) => {
                        seen = Some(m);
                        sent = Some(p);
                    }
                    Err(e) => {
                        conn.log(format!("arranging rush's list: {e}"));
                        if denied(&e) {
                            return;
                        }
                    }
                }
            }
        }
        if conn.wait_timeout(Duration::from_secs(2)) {
            return;
        }
    }
}

fn denied(e: &CallError) -> bool {
    e.code() == Some(CODE_DENIED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::parse_cards;

    const BOARD: &str = r#"{"links": [
  {"id": "c1", "name": "Older", "column": "requires_attention", "sessionLink": {"sessionId": "11111111-aaaa"}, "lastActivity": "2026-09-01T00:00:00Z", "updatedAt": "x", "manuallyArchived": false},
  {"id": "c2", "name": "Newer", "column": "requires_attention", "sessionLink": {"sessionId": "22222222-bbbb"}, "lastActivity": "2026-09-03T00:00:00Z", "updatedAt": "x", "manuallyArchived": false},
  {"id": "c3", "name": "Placed", "column": "requires_attention", "sortOrder": 0, "sessionLink": {"sessionId": "33333333-cccc"}, "updatedAt": "2026-08-01T00:00:00Z", "manuallyArchived": false},
  {"id": "c4", "column": "in_progress", "promptBody": "Fix the flaky test\nin CI", "sessionLink": {"sessionId": "44444444-dddd"}, "updatedAt": "x", "manuallyArchived": false},
  {"id": "c5", "name": "No conversation yet", "column": "backlog", "updatedAt": "x", "manuallyArchived": false},
  {"id": "c6", "name": "Archived", "column": "done", "sessionLink": {"sessionId": "66666666-ffff"}, "updatedAt": "x", "manuallyArchived": true},
  {"id": "c7", "name": "Discovered", "column": "all_sessions", "sessionLink": {"sessionId": "77777777-0000"}, "updatedAt": "x", "manuallyArchived": false},
  {"id": "c9", "column": "done", "sessionLink": {"sessionId": "99999999-2222"}, "updatedAt": "y", "manuallyArchived": false},
  {"id": "c8", "name": "Shipped", "column": "done", "sessionLink": {"sessionId": "88888888-1111"}, "updatedAt": "x", "manuallyArchived": false}
]}"#;

    fn agent(name: &str, section: &str, order: i64) -> Agent {
        Agent {
            name: name.into(),
            section: section.into(),
            order,
        }
    }

    #[test]
    fn the_board_as_rush_takes_it() {
        let cards = parse_cards(BOARD.as_bytes()).unwrap();
        let wire = serde_json::to_value(payload(&cards)).unwrap();
        // It's what rush's broker takes.
        let s = Sidebar::parse(&wire).unwrap_or_else(|e| panic!("rush refuses {wire}: {e}"));
        let sections: Vec<&str> = s.sections.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(sections, ["In Progress", "Waiting", "Done"]);
        let want = [
            (
                "44444444-dddd",
                agent("Fix the flaky test", "In Progress", 0),
            ),
            ("33333333-cccc", agent("Placed", "Waiting", 0)),
            ("22222222-bbbb", agent("Newer", "Waiting", 1)),
            ("11111111-aaaa", agent("Older", "Waiting", 2)),
            ("99999999-2222", agent("", "Done", 0)),
            ("88888888-1111", agent("Shipped", "Done", 1)),
        ];
        assert_eq!(s.agents.len(), want.len(), "{:?}", s.agents);
        for (id, w) in want {
            assert_eq!(s.agents[id], w, "{id}");
        }
        assert_eq!(s.title, "Kanban");

        let empty = serde_json::to_value(payload(&[])).unwrap();
        let s = Sidebar::parse(&empty).unwrap();
        assert!(
            s.is_empty(),
            "an empty board should clear the arrangement: {empty}"
        );
    }
}
