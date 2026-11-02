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
#[doc(hidden)]
pub fn is_turn_ref(r: &str) -> bool {
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
        Some(k) => rush_ui_cmdbar::bar::go_agent(m, &k),
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
