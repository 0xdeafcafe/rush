//! The tests of rush-ui-social's rooms.rs that need the whole UI.

use crate::session::{SessionView, Source};
use crate::style::*;
use rush_convo::Session;
use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_room::room::{self as r, Entry, Room, Rooms, Summary};
use rush_ui_social::rooms::*;

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

// roomrows.go:40: each reading of the rooms hands a room opened on a
// chat back to it: running, it's the chat's room; with a verdict, the
// verdict waits on the chat.
#[test]
fn reading_the_rooms_hands_verdicts_back() {
    use crate::screens::session_cards::discuss::{Verdicts, chat_room_on};
    let mut m = crate::kit::model(200, 30);
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let rooms = Rooms::new();
    let mut room = summary("r1", true).room;
    room.from = "default/a:k".into();
    rooms.create(room, || "r1".into()).unwrap();
    rooms
        .post("r1", entry(r::USER, r::SAY, "is the plan ok"))
        .unwrap();
    std::fs::write(rooms.pid_path("r1"), std::process::id().to_string()).unwrap();
    m.load_rooms();
    assert_eq!(
        chat_room_on(&m, "default/a:k").map(|s| s.room.id.as_str()),
        Some("r1"),
        "the running room isn't on its chat"
    );
    assert!(m.kept::<Verdicts>().waiting.is_empty());

    rooms
        .post("r1", entry("Opus", r::VERDICT, "Opus: it's ok"))
        .unwrap();
    m.load_rooms();
    assert!(chat_room_on(&m, "default/a:k").is_none());
    let v = m.kept::<Verdicts>();
    assert_eq!(
        v.waiting.get("default/a:k").map(|s| s.room.id.as_str()),
        Some("r1"),
        "the verdict didn't come back to its chat"
    );
    rush_state::set_thread_dir(None);
}

// roomrows.go:107: a room's row is an agent of kind room, so its pane
// has an agent's header (paneHeader): its @handle, "◇ room" where an
// agent says what it runs as, how the room stands, and the tabs.
#[test]
fn a_rooms_pane_has_an_agents_header() {
    let mut m = crate::kit::model(200, 30);
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let rooms = Rooms::new();
    let room = rooms
        .create(summary("r1", true).room, || "r1".into())
        .unwrap();
    rooms
        .post("r1", entry(r::USER, r::SAY, "settle tabs or spaces"))
        .unwrap();
    // Its driver: this process stands in, so the room runs.
    std::fs::write(rooms.pid_path("r1"), std::process::id().to_string()).unwrap();
    let mut s = summary("r1", true);
    s.room = room;
    m.kept::<RoomRows>().list = vec![s];
    m.rebuild();
    m.select("room:r1".to_string());
    m.press("enter");
    let f = m
        .frame()
        .iter()
        .map(|l| l.plain())
        .collect::<Vec<_>>()
        .join("\n");
    for want in [
        "tabs or spaces   @tabs-spaces  ·  working",
        "◇ room",
        "between turns · 2 agents",
        "Conversation   Overview   Changes",
        // The box says what enter does, as a busy agent's
        // (rushmode.go:3680).
        "queuing",
    ] {
        assert!(f.contains(want), "no {want:?} in:\n{f}");
    }
    // Overview is the room's, not its log again.
    m.press("]");
    let f = m
        .frame()
        .iter()
        .map(|l| l.plain())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(f.contains("tabs or spaces @tabs-spaces"), "{f}");
    assert!(
        !f.contains("settle tabs or spaces"),
        "the log under Overview:\n{f}"
    );
    // Over, the box's edge says enter replies (rushmode.go:3684). A
    // member keeps the over room in the list.
    let mut s = m.kept::<RoomRows>().list[0].clone();
    s.running = false;
    s.sessions.insert("sid-opus".into(), "Opus".into());
    m.kept::<RoomRows>().list = vec![s];
    m.agents.push(Agent {
        key: "rush:opus".into(),
        rush: true,
        job: rush_core::agent::Job {
            id: "sid-opus".into(),
            ..Default::default()
        },
        ..Agent::default()
    });
    m.roots.push(String::new());
    m.rebuild();
    m.press("[");
    let f = m
        .frame()
        .iter()
        .map(|l| l.plain())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(f.contains("enter replies through Claude Code"), "{f}");
    rush_state::set_thread_dir(None);
}

// roomfeed.go: a room's row shows its log beside the list as a
// conversation; enter opens it, what's typed is said in the room,
// ctrl+x pauses it, and alt+n names a member not in the list.
#[test]
fn a_rooms_pane_shows_its_log() {
    let mut m = crate::kit::model(200, 30);
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
    let mut m = crate::kit::model(120, 30);
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
