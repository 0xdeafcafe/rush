//! Opening a session whose host can't be reached (live.rs unreached): a
//! host that has gone is a stopped session, with nothing said; one that
//! fails to answer is said in plain words where the session is, never as
//! a raw error under another box, and logged for `rush why`.

use rush_core::agent::State;

use super::host::{Fake, Info};
use super::tests::{KEY, agent, flash, open, screen};
use super::{conn, live};
use crate::frame::messages::SayKind;
use crate::session::{SessionView, Source};

const REFUSED: &str = "Connection refused (os error 61)";

// A host that went, its socket left behind, opens as a stopped session:
// its info as it left it, nothing said, and not dialled again.
#[test]
fn a_gone_host_opens_as_a_stopped_session() {
    let fake = Fake::new();
    fake.host(KEY);
    *fake.stale.lock().unwrap() = true;
    let left = Info {
        state: "stopped".into(),
        ..Info::default()
    };
    fake.saved.lock().unwrap().push((KEY.into(), left));
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    live::sync(&mut m);
    let c = conn(&mut m, KEY);
    assert!(!c.hosted() && !c.dialing);
    assert!(c.unreached.is_empty(), "{}", c.unreached);
    assert_eq!(c.info.state, "stopped");
    assert_eq!(flash(&m), "", "nothing said");
    assert!(m.said.is_empty(), "{:?}", m.said);
    assert!(fake.open_failures.lock().unwrap().is_empty());
    live::sync(&mut m);
    assert_eq!(*fake.dials.lock().unwrap(), 1, "not dialled again");
}

// Open, a host that runs but doesn't answer is said under its own box in
// plain words, the raw error only in the log; said and logged once while
// it keeps failing, and gone once it's reached.
#[test]
fn a_failed_open_says_so_under_its_own_box() {
    let fake = Fake::new();
    fake.host(KEY);
    *fake.silent.lock().unwrap() = Some(REFUSED.into());
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    live::sync(&mut m);
    let want =
        "couldn't reach tidy the parser: its host is running but not answering. rush keeps trying";
    assert_eq!(flash(&m), want);
    assert_eq!(m.said.last().map(|l| l.kind), Some(SayKind::Failed));
    assert!(!flash(&m).contains("os error"));
    assert_eq!(
        fake.open_failures.lock().unwrap().clone(),
        vec![(
            KEY.to_string(),
            "not-answering".to_string(),
            REFUSED.to_string()
        )]
    );
    live::sync(&mut m);
    assert_eq!(*fake.dials.lock().unwrap(), 2, "tried again");
    assert_eq!(fake.open_failures.lock().unwrap().len(), 1, "logged once");
    let times = m.said.iter().filter(|l| l.text == want).count();
    assert_eq!(times, 1, "said once");
    *fake.silent.lock().unwrap() = None;
    live::sync(&mut m);
    let c = conn(&mut m, KEY);
    assert!(c.hosted() && c.unreached.is_empty());
}

// Beside the list (the new-agent box having the keys), a failure is said
// on the session beside it, not on the line under the box; #log has it.
#[test]
fn beside_the_list_it_says_so_on_the_session() {
    let fake = Fake::new();
    fake.host(KEY);
    *fake.down.lock().unwrap() = true;
    let mut a = agent("steer", State::Idle);
    a.rush = true;
    let mut m = open(a, &fake, 200, 40);
    m.full = false;
    m.session = None;
    m.rebuild();
    let mut v = SessionView::new(KEY.into(), Source::Hosted, 1);
    v.read = true;
    m.pane = Some(v);
    live::sync(&mut m);
    let say = "couldn't reach tidy the parser's host. rush why k1 has the error";
    assert_eq!(flash(&m), "", "nothing under the box");
    let l = m.said.last().expect("in #log");
    assert_eq!((l.kind, l.text.as_str()), (SayKind::News, say));
    assert_eq!(
        fake.open_failures.lock().unwrap().clone(),
        vec![(KEY.to_string(), "failed".to_string(), "no host".to_string())]
    );
    let f = screen(&mut m).join("\n");
    assert!(f.contains(say), "{f}");
    assert!(!f.contains("connecting to"), "{f}");
}
