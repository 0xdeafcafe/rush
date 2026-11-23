//! A stopped rush session, as its host left it.

use rush_core::agent::State;

use super::host::Fake;
use super::tests::{KEY, agent, open, screen};

/// A stopped rush session, shown, as its host left it.
fn stopped(asleep: bool) -> (crate::model::Model, std::sync::Arc<Fake>) {
    let fake = Fake::new();
    fake.host(KEY);
    let info = super::host::Info {
        permission_mode: "auto".into(),
        sleeping: asleep,
        state: "idle".into(),
        began: Box::new(super::host::Began {
            cwd: "/src/checkout/.claude/worktrees/k1".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    fake.saved.lock().unwrap().push((KEY.into(), info));
    let mut a = agent("steer", State::Done);
    a.pid = 0;
    a.rush = true;
    let mut m = open(a, &fake, 120, 40);
    super::live::sync(&mut m);
    (m, fake)
}

/// The row with the session's name on it.
fn name_row(m: &mut crate::model::Model) -> String {
    let s = screen(m);
    s.iter()
        .find(|l| l.contains("tidy the parser"))
        .cloned()
        .unwrap_or_else(|| panic!("no name row:\n{}", s.join("\n")))
}

/// A stopped rush session's host info is read from disk once, with no
/// socket: asleep, its header says idle, else stopped (openSavedHost,
/// paneHeader's state, rushmode.go:2860-2874).
#[test]
fn a_stopped_rush_session_reads_as_its_host_left_it() {
    let (mut m, fake) = stopped(true);
    let row = name_row(&mut m);
    assert!(row.contains("·  idle"), "{row}");
    assert!(!row.contains("stopped"), "{row}");
    assert_eq!(*fake.dials.lock().unwrap(), 0, "never dialled");
    // Read once while it's shown, however often the list is read.
    super::live::sync(&mut m);
    super::live::sync(&mut m);
    assert_eq!(*fake.saved_reads.lock().unwrap(), 1);
    // Asleep, how it's reached goes unsaid (rushmode.go:2945); where it
    // is, is where its host last said (location.go:16).
    let f = screen(&mut m).join("\n");
    assert!(!f.contains("send to resume"), "{f}");
    assert!(f.contains("worktrees/k1"), "{f}");

    let (mut m, _) = stopped(false);
    let row = name_row(&mut m);
    assert!(row.contains("·  stopped"), "{row}");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("send to resume"), "{f}");
}

/// The box's top edge says its permission mode on the right, stopped or
/// not (rushmode.go:3725 b.topR).
#[test]
fn a_stopped_rush_sessions_box_says_its_mode() {
    let (mut m, _) = stopped(true);
    let s = screen(&mut m);
    assert!(
        s.iter().any(|l| l.contains("── auto ─")),
        "{}",
        s.join("\n")
    );
}
