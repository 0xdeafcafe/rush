//! Emeritus in the Session (docs/emeritus.md): a session that retired is
//! read-only, its box saying whom it handed over to and enter opening
//! that one; a session that took over names the one it came from, alt+o
//! opening it. A retired session open here stays a row in the list while
//! it's open, though the list folds retired ones into their successor's.
//! Rust-only: Go rush has no Emeritus.

use rush_core::text::Line;

use super::{agent, conn, open_key};
use crate::frame::{InputBox, Out, Press};
use crate::model::Model;
use crate::style::*;

/// A session in a chain of handovers, by rush id and its name in the
/// list ("session <id>" when it isn't listed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Other {
    pub id: String,
    pub name: String,
}

fn named(m: &Model, id: &str, known: &str) -> Other {
    let name = m
        .agents
        .iter()
        .find(|a| a.rush && a.job.id == id)
        .map(|a| a.display_name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| (!known.is_empty()).then(|| known.to_string()))
        .unwrap_or_else(|| format!("session {id}"));
    Other {
        id: id.to_string(),
        name,
    }
}

/// The session that took over from session key's, when it's retired.
pub fn successor(m: &Model, key: &str) -> Option<Other> {
    let a = agent(m, key).filter(|a| a.rush && !a.successor().is_empty())?;
    Some(named(m, a.successor(), ""))
}

/// The session key's took over from, when it did.
pub fn predecessor(m: &Model, key: &str) -> Option<Other> {
    let l = agent(m, key)?.lineage.as_deref()?;
    if l.predecessor.is_empty() {
        return None;
    }
    Some(named(m, &l.predecessor, &l.name))
}

/// The Session's keys for Emeritus, ahead of the box's: on a retired
/// session enter opens its successor and typing is refused, saying so;
/// alt+o on one that took over opens the one it came from. Pass for the
/// rest.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let s = k.s.as_str();
    if let Some(to) = successor(m, &key) {
        if s == "enter" || s == "alt+o" {
            open(m, &to);
            return Out::Draw;
        }
        if k.text().is_some() || s == "ctrl+enter" {
            m.flash(&format!(
                "retired: it takes no messages · enter opens {}, which carries on its work",
                to.name
            ));
            return Out::Draw;
        }
        return Out::Pass;
    }
    if s == "alt+o"
        && conn(m, &key).input.is_empty()
        && let Some(from) = predecessor(m, &key)
    {
        open(m, &from);
        return Out::Draw;
    }
    Out::Pass
}

/// Opens session o, a row in the list or folded into another's.
fn open(m: &mut Model, o: &Other) {
    super::spawned::open_session(m, &o.id);
}

/// The box of a retired session: read-only, saying whom it handed over
/// to and that enter opens that one.
pub fn retired_box(m: &Model, key: &str, b: &mut InputBox) {
    let Some(to) = successor(m, key) else {
        return;
    };
    b.top_l = line([
        sp("retired", ORANGE),
        sp(" · handed over to ", DIM),
        sp(rush_draw::truncate(&to.name, 32, "…").into_owned(), TEXT),
        sp(" · enter opens it", ORANGE),
    ]);
    b.top_r = Line::new();
    b.text.clear();
    b.cursor = 0;
    b.holder = "read-only: what's typed here goes nowhere".into();
    b.edge = Some(FAINT);
}

/// After each reading of the list: a retired session open here is kept a
/// row of its own while it's open, so the list doesn't fold it away from
/// under you.
pub fn keep_open(m: &mut Model) {
    let Some(key) = open_key(m) else {
        return;
    };
    let Some(id) = agent(m, &key)
        .filter(|a| a.rush && !a.successor().is_empty())
        .map(|a| a.job.id.clone())
    else {
        return;
    };
    // Once each: an unfold reads the list again.
    if m.kept::<KeptOpen>().0.insert(id.clone()) {
        let _ = m.kept::<crate::worker::FleetIn>().unfold(&id);
    }
}

/// The retired sessions kept a row of their own while open.
#[derive(Default)]
struct KeptOpen(std::collections::HashSet<String>);
