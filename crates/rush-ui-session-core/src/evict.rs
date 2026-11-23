//! Rust-only (Go keeps every hostConn's state for the life of the
//! process): what a session out of sight holds of its runs'
//! conversations is let go once it hasn't been shown for EVICT_AFTER.
//!
//! A session's side (`Core::conns`, one a session ever touched) is never
//! dropped: it holds the box's draft, the outbox, the queue, the picks
//! and the views, none of which a transcript gives back. What it holds of
//! its runs is (subs.rs `Runs::evict`): the agents rush hosted for it, each
//! a whole conversation read while the session shows; every subagent run
//! read whole for Changes; the runs read on for their numbers. Each is read
//! again from its transcript once the session shows (a run due whole and
//! not held is due at once), so it looks as it did. Measured in DESIGN.md
//! ("A session out of sight lets go of its runs"): `cargo bench -p rush-ui
//! --bench sessmem`.
//!
//! Shown is opened, beside the list, a grid tile's, or the one the list's
//! read-ahead keeps to go back to (list_extras/predict.rs `left`).

use rush_core::time::Ms;

use crate::frame::Ext;
use crate::model::Model;

/// How long a session goes unshown before its runs' conversations are let
/// go. Going back and forth between a few sessions comes back within a
/// minute or two, and a session with dozens of hosted agents takes a
/// second or more of reading to show them again; five minutes out of
/// sight is a session left, and what it holds (tens of MiB for a few dozen
/// agents, DESIGN.md) is the memory a day of them grows by.
pub const EVICT_AFTER: Ms = 5 * 60 * 1000;

pub fn register(x: &mut Ext) {
    x.on_fleet.push(on_fleet);
    x.on_open.push(on_open);
}

fn on_fleet(m: &mut Model) {
    sweep(m);
}

/// A session opened is shown from now.
fn on_open(m: &mut Model) {
    let now = m.now;
    if let Some(key) = m.session.as_ref().map(|v| v.key.clone()) {
        super::conn(m, &key).shown_at = now;
    }
}

/// The sessions shown now: opened, beside the list, a tile's, and the one
/// the read-ahead keeps to go back to.
pub fn shown(m: &Model) -> Vec<String> {
    let left = m
        .kept
        .peek::<rush_ui_list_extras::predict::Predict>()
        .and_then(|p| p.left.as_ref());
    [m.session.as_ref(), m.pane.as_ref(), left]
        .into_iter()
        .flatten()
        .chain(m.tiles.values())
        .map(|v| v.key.clone())
        .collect()
}

/// Notes the sessions shown as of m.now, and lets go of what those unshown
/// for EVICT_AFTER hold of their runs: how many sessions let go of any.
pub fn sweep(m: &mut Model) -> usize {
    let now = m.now;
    let shown = shown(m);
    let mut gone = Vec::new();
    for (key, c) in &mut super::core(m).conns {
        if shown.contains(key) || c.shown_at == 0 {
            // A side made without being shown (a card, the list) counts
            // from when it's first seen here.
            c.shown_at = now;
            continue;
        }
        if now - c.shown_at < EVICT_AFTER || !c.runs.holds() {
            continue;
        }
        if let Some(e) = c.runs.evict() {
            gone.push(e);
        }
    }
    let n = gone.len();
    if n > 0 {
        rush_draw::perf_note(|| {
            let convs: usize = gone.iter().map(|e| e.conversations()).sum();
            format!("evict {n} sessions' runs, {convs} conversations")
        });
        // Off the UI thread, its pages given back; at once with no loop.
        if m.sender().is_some() {
            rush_sys::mem::let_go(gone);
        } else {
            drop(gone);
        }
    }
    n
}
