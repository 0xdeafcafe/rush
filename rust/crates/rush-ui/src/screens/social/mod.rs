//! The social group's screens: the feed agents share and its graph,
//! rooms, broadcasts, zen, and the shared places and services the rest of
//! the screens lean on (the Overview's sections, the prompt pins, the
//! stack, the shelf, the stash's words, dictation's look, updates,
//! reloading, the API watch, transcript squeezing, a memo of reads off
//! the UI thread). After Go's internal/ui files of the same names; each
//! module cites its own.

use crate::frame::{Ext, Out};
use crate::model::Model;

pub mod feed;
pub mod feedgraph;
pub mod ink;
pub mod rooms;
pub mod stream;
pub mod zen;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    x.command("feed", "the feed agents share · #feed on|off", feed_command);
    x.keys.push(zen::key);
    x.overlays.push(zen::over);
    x.on_fleet.push(on_fleet);
}

/// #feed [on|off|new|post]: keys.go:1040's case "feed".
fn feed_command(m: &mut Model, arg: &str) -> Out {
    m.open_feed(arg)
}

/// After each reading of the list (model.go's snapMsg: loadStream once
/// the feed is on).
fn on_fleet(m: &mut Model) {
    m.zen_pick();
    // Reading the board and the rooms is for a running rush; one drawn
    // once (--render, tests) reads them when a screen asks.
    if m.sender().is_none() {
        return;
    }
    if m.kept::<stream::Stream>().on != Some(false) {
        m.load_stream();
    }
    m.load_rooms();
}
