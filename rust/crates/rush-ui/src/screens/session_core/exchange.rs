//! A Session's keys on what agents said to each other, and on what can be
//! copied whole: → on a picked exchange goes to the other agent's
//! Session; alt+c with nothing typed copies the picked exchange, else the
//! drawing picked, else the picked turn's answer as written. After Go's
//! internal/ui/rushmode.go (paneKey: alt+c :4038-4060, → on an exchange
//! :4222-4232) and cmdbar.go:1045 (goAgent).

use rush_convo::model::exchange_peer;

use super::{conn, open_key};
use crate::frame::{Out, Press};
use crate::model::Model;

/// The picked row of the open Session ("" for none).
fn picked(m: &Model) -> String {
    m.session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default()
}

/// Whether r names a turn: t and its number (isTurnRef).
fn is_turn_ref(r: &str) -> bool {
    r.len() >= 2 && r.starts_with('t') && r[1..].bytes().all(|c| c.is_ascii_digit())
}

/// → and alt+c on the open Session, with nothing typed in its box.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    if s != "right" && s != "alt+c" {
        return Out::Pass;
    }
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if !conn(m, &key).input.is_empty() {
        return Out::Pass;
    }
    let sel = picked(m);
    match s {
        "right" => match exchange_peer(&sel) {
            Some(peer) => {
                go_peer(m, peer.to_string());
                Out::Draw
            }
            None => Out::Pass,
        },
        _ => match copied(m, &sel) {
            Some(text) => {
                m.copy_text(text);
                Out::Draw
            }
            None => Out::Pass,
        },
    }
}

/// Opens the other agent of an exchange, found by its id, its session's
/// or its key; or says it isn't in the list.
fn go_peer(m: &mut Model, peer: String) {
    let found = (!peer.is_empty())
        .then(|| {
            m.agents
                .iter()
                .find(|a| a.job.id == peer || a.job.session_id == peer || a.key == peer)
                .map(|a| a.key.clone())
        })
        .flatten();
    match found {
        Some(k) => crate::screens::cmdbar::bar::go_agent(m, &k),
        None => m.flash("The peer session is not available in this agent list"),
    }
}

/// What alt+c copies for the row picked: the exchange whole, the drawing,
/// or the turn's answer, markdown and all.
fn copied(m: &Model, sel: &str) -> Option<String> {
    let s = &m.session.as_ref()?.sess;
    if let Some(text) = s.exchange_text(sel) {
        return Some(text);
    }
    if let Some(i) = sel.rfind(":s:") {
        let id = &sel[i + 3..];
        if let Some(rows) = s
            .step_by_call(id)
            .and_then(|st| rush_convo::render::drawing(s.step(st)))
        {
            return Some(rows.join("\n"));
        }
    }
    let t = sel.split(':').next().unwrap_or("");
    if is_turn_ref(t) {
        let turn = s.turns.iter().find(|turn| format!("t{}", turn.n) == t)?;
        if let Some(a) = turn.answer().filter(|a| !a.is_empty()) {
            return Some(a.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::host::Fake;
    use super::super::tests::{NOW, agent, flash, open, press, screen, sess};
    use super::*;
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
}
