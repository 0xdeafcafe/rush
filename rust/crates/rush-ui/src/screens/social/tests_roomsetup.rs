//! A room's set-up, after Go's internal/ui room.go and discuss.go: its
//! rows, its keys, and the room it opens, alone or on a chat. A model
//! with no loop runs handed-off work at once; the hosts are a fake that
//! keeps the rooms it's asked to open.

use std::sync::Arc;

use rush_core::adapter::Registry;
use rush_core::text::Line;

use super::roomsetup::{RoomSetup, chat_room, room_choice};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::screens::accounts::setup::StartOver;
use crate::screens::session_cards::discuss::RoomAsk;
use crate::screens::session_core::host::{Fake, set_hosts};

fn reg() -> Registry {
    let r = Registry::new();
    r.register(rush_claude::Claude::new());
    r
}

fn claude(model: &str) -> StartOver {
    StartOver {
        kind: "claude".into(),
        model: model.into(),
        ..StartOver::default()
    }
}

/// A model in a rush folder of the test's own, for this thread: the
/// rooms it opens are kept there.
fn model() -> (Model, Arc<Fake>) {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.keep()));
    let mut m = Model::new("0.1.0");
    m.reg = reg();
    m.w = 120;
    m.h = 40;
    let fake = Fake::new();
    set_hosts(&mut m, Arc::new(fake.clone()));
    (m, fake)
}

fn setup(topic: &str) -> RoomSetup {
    RoomSetup {
        topic: topic.chars().collect(),
        pos: topic.chars().count(),
        options: vec![claude("")],
        rounds: 4,
        dir: "/src/checkout".into(),
        ..RoomSetup::default()
    }
}

fn press(s: &mut RoomSetup, m: &mut Model, k: &str) -> Out {
    s.key(m, &Press::of(k).unwrap_or_else(|| panic!("no key {k:?}")))
}

fn plain(ls: &[Line]) -> Vec<String> {
    ls.iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect()
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

/// The set-up's rows: the topic, where they work, the panel and its
/// choices, each agent at its default then at each of its models, and
/// the keys (draw, body).
#[test]
fn setup_rows() {
    let (m, _) = model();
    let mut s = setup("tabs or spaces");
    s.selected = vec![claude("")];
    let rows = plain(&s.body(&m, 100, 30));
    assert_eq!(rows[0], "Topic");
    assert!(rows[1].starts_with("  tabs or spaces"), "{:?}", rows[1]);
    assert_eq!(
        rows[3],
        "Folder  /src/checkout  where they read and run things: the selected agent's, as for a new session"
    );
    assert_eq!(
        rows[5],
        "Panel  1 chosen · two or more, mixed models argue best   rounds at most: 4"
    );
    assert!(
        rows[7].starts_with("  ● default model · Claude Code · Anthropic"),
        "{:?}",
        rows[7]
    );
    assert!(rows[8].starts_with("  ○ "), "{:?}", rows[8]);
    assert_eq!(
        rows.last().unwrap(),
        "tab topic ↔ panel · space choose · ←→ rounds · enter open the room · esc back"
    );
    assert!(rows.len() <= 30);
    // Keyed to the API, it says so.
    let o = StartOver {
        billing: rush_state::profiles::BILLING_KEY.into(),
        ..claude("opus")
    };
    let l = room_choice(&m.reg, &o).plain();
    assert!(l.contains(" · API key  opus"), "{l}");
}

/// Nothing runnable: it says where to add one.
#[test]
fn setup_with_no_agents() {
    let (m, _) = model();
    let mut s = setup("");
    s.options.clear();
    let rows = plain(&s.body(&m, 100, 30));
    assert!(
        rows.contains(&"No runnable agents. Add one in Settings first.".to_string()),
        "{rows:?}"
    );
}

/// tab goes between the topic and the panel; space chooses (six at
/// most), ←→ the rounds (2 to 12), and the topic is typed into.
#[test]
fn setup_keys() {
    let (mut m, _) = model();
    let mut s = setup("");
    press(&mut s, &mut m, "a");
    press(&mut s, &mut m, "b");
    assert_eq!(s.topic.iter().collect::<String>(), "ab");
    assert_eq!(s.paste(&mut m, "\tc"), Out::Draw);
    assert_eq!(s.topic.iter().collect::<String>(), "ab    c");
    press(&mut s, &mut m, "tab");
    assert!(s.on_list);
    press(&mut s, &mut m, "space");
    press(&mut s, &mut m, "down");
    press(&mut s, &mut m, "space");
    assert_eq!(s.selected.len(), 2);
    press(&mut s, &mut m, "space");
    assert_eq!(s.selected.len(), 1, "chosen again, it's let go");
    for _ in 0..5 {
        press(&mut s, &mut m, "left");
    }
    assert_eq!(s.rounds, 2);
    for _ in 0..20 {
        press(&mut s, &mut m, "right");
    }
    assert_eq!(s.rounds, 12);
    press(&mut s, &mut m, "shift+tab");
    assert!(!s.on_list);
    assert_eq!(press(&mut s, &mut m, "esc"), Out::Close);
}

/// enter wants a topic and two agents, and says so (create).
#[test]
fn create_needs_a_topic_and_two_agents() {
    let (mut m, fake) = model();
    let mut s = setup("  ");
    s.selected = vec![claude(""), claude("opus")];
    press(&mut s, &mut m, "enter");
    assert_eq!(flash(&m), "give the room a topic first");
    let mut s = setup("tabs or spaces");
    s.selected = vec![claude("")];
    press(&mut s, &mut m, "enter");
    assert_eq!(flash(&m), "choose at least two agents: tab, then space");
    assert!(fake.rooms.lock().unwrap().is_empty());
}

/// enter opens the room: each member's config from its start, named by
/// its model, numbered when taken; the panel kept for next time, the
/// sheet closed and the room's row picked and opened (create, showRoom).
#[test]
fn create_opens_the_room() {
    let (mut m, fake) = model();
    let mut s = setup("tabs or spaces");
    s.rounds = 6;
    s.selected = vec![claude("claude-opus-4-1"), claude("claude-opus-4-1")];
    m.open_sheet(Box::new(s));
    m.press("enter");
    let rooms = fake.rooms.lock().unwrap().clone();
    assert_eq!(rooms.len(), 1);
    let r = &rooms[0];
    assert_eq!(
        (r.topic.as_str(), r.cwd.as_str(), r.rounds, r.brief.as_str()),
        ("tabs or spaces", "/src/checkout", 6, "")
    );
    let names: Vec<&str> = r.members.iter().map(|mb| mb.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert_eq!(names[1], format!("{}2", names[0]));
    assert_eq!(r.members[0].kind(), "claude");
    assert_eq!(r.members[0].model(), "claude-opus-4-1");
    assert!(m.sheet.is_none(), "closed once the room is open");
    // Its row is picked until the rooms are read again; here its driver
    // never started, so the reading lets it go, and the room stays open.
    assert_eq!(
        m.session.as_ref().map(|v| v.key.as_str()),
        Some("room:room-1")
    );
    // The next set-up starts with the same panel.
    super::roomsetup::room_setup(&mut m, "");
    assert_eq!(m.sheet_as::<RoomSetup>().unwrap().selected.len(), 2);
}

/// A member's config that can't be made fails as it says; a room that
/// can't be opened says so; the sheet stays.
#[test]
fn create_says_why_it_failed() {
    let (mut m, fake) = model();
    let mut s = setup("tabs or spaces");
    s.selected = vec![claude(""), claude("opus")];
    m.open_sheet(Box::new(s));
    *fake.fail.lock().unwrap() = Some("rush can't run claude".into());
    m.press("enter");
    assert_eq!(flash(&m), "rush can't run claude");
    assert!(m.sheet_as::<RoomSetup>().is_some());
}

/// A room on a chat: the chat's exchanges as context, tab goes topic →
/// context → panel, ←→ the turns that go in; the room keeps where it came
/// from and the context chosen, and you stay in the chat (chatRoom,
/// brief, joinedOpened).
#[test]
fn a_room_on_a_chat() {
    let (mut m, fake) = model();
    chat_room(
        &mut m,
        RoomAsk {
            topic: "is this the right fix?".into(),
            from: "default/k".into(),
            turns: vec!["User: a\n".into(), "User: b\n".into(), "User: c\n".into()],
            ctx: 3,
            dir: "/src/chat".into(),
            head: "Status: idle".into(),
            fill: true,
        },
    );
    let mut s = std::mem::take(m.sheet_as::<RoomSetup>().expect("the set-up"));
    assert_eq!(
        (s.from.as_str(), s.ctx, s.dir.as_str()),
        ("default/k", 3, "/src/chat")
    );
    let rows = plain(&s.body(&m, 110, 30));
    assert!(
        rows.contains(&"Context  ← 3 →  the latest 3 of the chat's 3 exchanges".to_string()),
        "{rows:?}"
    );
    assert_eq!(
        rows.last().unwrap(),
        "tab topic → context → panel · ←→ turns of context, or rounds · space choose · enter open the room · esc back"
    );
    press(&mut s, &mut m, "tab");
    assert!(s.on_ctx);
    press(&mut s, &mut m, "left");
    press(&mut s, &mut m, "left");
    assert_eq!(s.ctx, 1);
    press(&mut s, &mut m, "right");
    press(&mut s, &mut m, "right");
    press(&mut s, &mut m, "right");
    assert_eq!(s.ctx, 3, "no more than the chat has");
    press(&mut s, &mut m, "left");
    press(&mut s, &mut m, "tab");
    assert!(s.on_list && !s.on_ctx);
    s.options = vec![claude("")];
    s.selected = vec![claude(""), claude("opus")];
    press(&mut s, &mut m, "enter");
    let r = fake.rooms.lock().unwrap()[0].clone();
    assert_eq!(r.from, "default/k");
    assert_eq!(r.brief, "Status: idle\n---\nUser: b\n\n---\nUser: c\n");
    assert_eq!(
        flash(&m),
        "room opened on this chat · it shows above the box · the verdict comes back here"
    );
}
