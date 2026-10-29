//! The social group's screens: the feed agents share and its graph,
//! rooms, broadcasts, zen, and the shared places and services the rest of
//! the screens lean on (the Overview's sections, the
//! stack, the shelf, the stash's words, dictation's look, updates,
//! reloading, the API watch, transcript squeezing, a memo of reads off
//! the UI thread). After Go's internal/ui files of the same names; each
//! module cites its own.

use crate::frame::{Ext, Out};
use crate::model::Model;

pub mod broadcast;
pub mod dock;
pub mod feed;
pub mod feedgraph;
pub mod ink;
pub mod memo;
pub mod parts;
pub mod rooms;
pub mod roomsetup;
pub mod services;
#[cfg(test)]
mod sizes;
pub mod stream;
pub mod zen;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    x.command("feed", "the feed agents share · #feed on|off", feed_command);
    x.command(
        "room",
        "a full-screen group chat: fresh agents argue a topic to a verdict, and you're in it",
        roomsetup::open_room_cmd,
    );
    x.command(
        "broadcast",
        "one message to several agents: all, @a @b, or click to pick",
        broadcast::command,
    );
    x.command("update", "the newest rush over this one", |m, _| {
        m.install_update()
    });
    x.command(
        "reload",
        "run the installed rush in this one's place · all for every window",
        |m, arg| m.reload(arg.trim() == "all"),
    );
    x.keys.push(zen::key);
    // Zen's list is only the agents waiting on you.
    x.list_filter.push(zen::shows);
    x.list_keys.push(broadcast::list_key);
    x.mouse.push(broadcast::mouse);
    x.submit.push(broadcast::submit);
    x.overlays.push(zen::over);
    x.on_fleet.push(on_fleet);
    x.list_foot.push(dock::foot);
    x.mouse.push(dock::mouse);
}

/// #feed [on|off|new|post]: keys.go:1040's case "feed".
fn feed_command(m: &mut Model, arg: &str) -> Out {
    m.open_feed(arg)
}

/// After each reading of the list: zen keeps its agent, and a running
/// rush reads the board and the rooms, asks after updates, watches the
/// API for sessions to continue and squeezes idle transcripts (model.go's
/// snapMsg and tick: loadStream, loadRooms, checkUpdate, watchOnline,
/// squeezeTranscripts, applyRestore).
fn on_fleet(m: &mut Model) {
    m.take_restore(std::env::var(services::RESTORE_ENV).ok());
    m.apply_restore();
    m.zen_pick();
    // The rest is for a running rush; one drawn once (--render, tests)
    // reads them when a screen asks.
    if m.sender().is_none() {
        return;
    }
    if crate::hosted::quiet(m) {
        // Hosted reads only the rooms: no stream, update check or watch
        // on the API (model.go:556 Init, :1096 leads).
        m.load_rooms();
        return;
    }
    match m.kept::<stream::Stream>().on {
        Some(true) => m.load_stream(),
        Some(false) => {}
        // Whether the feed is on is read from the config off the UI
        // thread, once; the board after it.
        None => m.later(
            || rush_state::Store::load().config.feed,
            |m, on| {
                let s = m.kept::<stream::Stream>();
                if s.on.is_none() {
                    s.on = Some(on);
                }
                if s.on == Some(true) {
                    m.load_stream();
                }
                Out::Idle
            },
        ),
    }
    m.load_rooms();
    m.check_update();
    m.watch_online();
    // Only the rush that leads compresses transcripts (model.go:1100).
    if m.leads() {
        m.squeeze_transcripts();
    }
}

#[cfg(test)]
mod tests_roomsetup;

#[cfg(test)]
mod tests {
    use super::feed::{FeedSheet, tests::feed_model};
    use crate::frame::Out;

    // The group's # commands, as Go's keys.go:1040-1080 names them.
    #[test]
    fn commands() {
        let m = feed_model();
        for name in ["feed", "room", "broadcast", "update", "reload"] {
            assert!(
                m.ext.commands.iter().any(|c| c.name == name),
                "#{name} isn't registered"
            );
        }
    }

    // The feed sheet's keys: what each does, as community.go:523 has them.
    #[test]
    fn feed_keys() {
        let mut m = feed_model();
        m.open_feed("");
        type Check = fn(&FeedSheet) -> bool;
        let table: &[(&str, Check)] = &[
            ("g", |s| s.graph),
            ("g", |s| !s.graph),
            ("n", |s| s.composing && s.reply_to.is_empty()),
            ("esc", |s| !s.composing),
            ("enter", |s| s.composing && !s.reply_to.is_empty()),
            ("esc", |s| !s.composing),
            ("r", |s| s.composing),
            ("esc", |s| !s.composing),
            ("up", |s| !s.follow),
            ("down", |s| s.follow),
        ];
        for (k, ok) in table {
            let out = m.press(k);
            assert_ne!(out, Out::Pass, "{k} isn't the sheet's");
            let s = m.sheet_as::<FeedSheet>().expect("still open");
            assert!(ok(s), "after {k}: {s:?}");
        }
        assert_eq!(m.press("esc"), Out::Close);
        assert!(m.sheet.is_none());
        // Opened again, it's where it was.
        m.open_feed("");
        assert!(m.sheet_as::<FeedSheet>().is_some());
    }
}
