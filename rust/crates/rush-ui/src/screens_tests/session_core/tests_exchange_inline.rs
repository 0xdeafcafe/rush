//! rush-ui-session-core's exchange.rs tests: they make a whole UI model.

use super::exchange::*;
use super::host::Fake;
use super::tests::{NOW, agent, flash, open, press, screen, sess};
use crate::model::Model;
use rush_core::agent::State;
use rush_core::event::{Exchange, Peer};

/// A Session with a message another agent sent it, picked.
fn picked_exchange(peer_listed: bool) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("plain", State::Idle), &fake, 120, 40);
    if peer_listed {
        let mut p = agent("plain", State::Idle);
        p.key = "default/peer".into();
        p.job.id = "p1".into();
        p.display_name = "the reviewer".into();
        m.agents.push(p);
        m.roots.push(String::new());
    }
    let e = Exchange {
        at: NOW,
        id: "x1".into(),
        direction: "received".into(),
        phase: "message".into(),
        text: "please look at the parser's error paths".into(),
        sender: Peer {
            session_id: "p1".into(),
            kind: "plain".into(),
            name: "Reviewer".into(),
        },
        ..Exchange::default()
    };
    sess(&mut m).sent_exchange(&e, NOW);
    let _ = screen(&mut m);
    let v = m.session.as_mut().unwrap();
    let r = v
        .shown
        .iter()
        .find(|r| r.starts_with("exchange:"))
        .cloned()
        .expect("the exchange's row");
    v.sel = r;
    m
}

/// Picked, an exchange says what its keys do; alt+c copies it whole;
/// → goes to the agent that sent it (rushmode.go:3819, :4038, :4222).
#[test]
fn keys_on_an_exchange() {
    let mut m = picked_exchange(true);
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("space expand message") && f.contains("→ open peer session"),
        "{f}"
    );
    press(&mut m, "alt+c");
    assert!(
        m.pending_copy
            .as_deref()
            .is_some_and(|c| c.contains("please look at the parser's error paths")),
        "{:?}",
        m.pending_copy
    );
    press(&mut m, "right");
    // Picked in the list and opened there, as goAgent does (the test
    // agent has nothing to read, so its Session doesn't open).
    assert_eq!(m.sel, "default/peer");

    let mut m = picked_exchange(false);
    press(&mut m, "right");
    assert_eq!(
        flash(&m),
        "The peer session is not available in this agent list"
    );
}

/// A stopped rush session's past exchanges wait for its transcript,
/// and stay when the whole of it takes the end's place
/// (rushmode.go:2106, twopass.go:96).
#[test]
fn past_exchanges_come_back() {
    use crate::session::{SessionView, Source};
    let count = |v: &SessionView| {
        v.sess
            .turns
            .iter()
            .flat_map(|t| &t.items)
            .filter(|i| matches!(i, rush_convo::model::Item::Exchange(_)))
            .count()
    };
    let e = Exchange {
        at: NOW,
        id: "x1".into(),
        direction: "sent".into(),
        phase: "message".into(),
        text: "the parser's fixed".into(),
        ..Exchange::default()
    };
    let mut v = SessionView::new("default/k1".into(), Source::Hosted, 1);
    v.restore_exchanges(vec![e]);
    assert_eq!(count(&v), 0, "not before the transcript's read");
    v.read = true;
    v.restore_exchanges(Vec::new());
    assert_eq!(count(&v), 1);
    v.restore_exchanges(Vec::new());
    assert_eq!(count(&v), 1, "once");
    v.take_whole(rush_convo::Session::new());
    assert_eq!(count(&v), 1, "kept when the whole is read");
}

#[test]
fn turn_refs() {
    for (r, want) in [
        ("t1", true),
        ("t12", true),
        ("t", false),
        ("tx", false),
        ("s1", false),
    ] {
        assert_eq!(is_turn_ref(r), want, "{r}");
    }
}
