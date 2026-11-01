//! The social group's screens: the feed agents share and its graph,
//! rooms, broadcasts, and the shared places and services the rest of
//! the screens lean on (the Overview's sections, the
//! stack, the shelf, the stash's words, dictation's look, updates,
//! reloading, the API watch, transcript squeezing, a memo of reads off
//! the UI thread). After Go's internal/ui files of the same names; each
//! module cites its own.

// The base's modules, so crate::frame::.. and the like read as they did
// in rush-ui. Each is the one module, not a copy.
use rush_ui_base::{
    accounts, config, finish, frame, host, list, look, mention, model, overlay, sess, session,
    startdir, style, worker,
};

use crate::feed::ModelFeed;
use crate::frame::{Ext, Out};
use crate::model::Model;
use crate::rooms::ModelRooms;
use crate::services::ModelOnline;
use crate::services::ModelReload;
use crate::services::ModelSqueeze;
use crate::services::ModelUpdates;
use crate::stream::ModelStream;

pub mod broadcast;
pub mod dictation;
pub mod dock;
pub mod feed;
pub mod feedgraph;
pub mod ink;
pub mod memo;
pub mod parts;
pub mod pushtalk;
pub mod rooms;
pub mod roomsetup;
pub mod screenwords;
pub mod services;
pub mod stream;

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
    // Push-to-talk's key and esc before anything else has them, and yap's
    // paste before a box folds it (dictation.rs).
    x.keys.insert(0, dictation::key);
    x.paste.insert(0, dictation::paste);
    x.list_keys.push(broadcast::list_key);
    x.mouse.push(broadcast::mouse);
    x.submit.push(broadcast::submit);
    x.on_fleet.push(on_fleet);
    x.list_foot.push(dock::foot);
    x.mouse.push(dock::mouse);
}

/// #feed [on|off|new|post]: keys.go:1040's case "feed".
fn feed_command(m: &mut Model, arg: &str) -> Out {
    m.open_feed(arg)
}

/// After each reading of the list: a running rush reads the board and the
/// rooms, asks after updates, watches the API for sessions to continue and
/// squeezes idle transcripts (model.go's snapMsg and tick: loadStream, loadRooms, checkUpdate, watchOnline,
/// squeezeTranscripts, applyRestore).
fn on_fleet(m: &mut Model) {
    m.take_restore(std::env::var(services::RESTORE_ENV).ok());
    m.apply_restore();
    // The rest is for a running rush; one drawn once (--render, tests)
    // reads them when a screen asks.
    if m.sender().is_none() {
        return;
    }
    if m.ext.calls.hosted_id()(m).is_some() {
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

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
