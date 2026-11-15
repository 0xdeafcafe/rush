//! What the UI does with links of a kind rush knows (rush_convo's
//! linkinfo): a click on a GitHub link of any kind it knows (a pull
//! request, an issue, an Actions run, a commit, a repository) opens its
//! popover (linkpop.rs), which offers its links out first and what can be
//! done with it folded in groups; a link of a kind with no handler still
//! draws its badge, and a click on it does what any link's does. Port of
//! internal/ui/linkkinds.go:1-75 (openKnownLink :44), with the lookups
//! turned on as the TUI starts, an answer landing drawing it again
//! (model.go:447-450 New, hosted.go:27 NewHosted).
//!
//! Rush-only: Go opened a pull request's whole sheet from its link and put
//! Open in rush and Merge… first in its menu (linkHandler :14,
//! openPRLink :53, prLinkActs :61); here the sheet is one of the
//! popover's actions, and merging from it still only asks.

use std::sync::{Mutex, PoisonError};

use rush_convo::linkinfo::{look_links, on_lookup};
use rush_draw::Sender;

use crate::frame::Out;
use crate::model::Model;
use crate::worker::Note;

/// Where a lookup's answer wakes the UI, on a lane of its own.
static WAKE: Mutex<Option<Sender<Note>>> = Mutex::new(None);

/// Turns link lookups on, so links rush knows the kind of say where they
/// stand, and has each answer that lands draw the screen again (New's
/// convo.LookLinks(true)). Only the TUI does: tests and --render draw
/// links bare and ask nothing.
pub fn start(m: &Model) {
    let Some(tx) = m.sender() else {
        return;
    };
    *WAKE.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx.lane(1));
    on_lookup(wake);
    look_links(true);
}

/// An answer landed: a frame, unless one is already on its way. It runs
/// on the lookup's thread, so it never waits.
fn wake() {
    if let Some(tx) = WAKE.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        let _ = tx.try_send(Note::Apply(Box::new(|_| Out::Draw)));
    }
}

/// Opens a link of a kind rush knows in rush: a GitHub link's popover,
/// by the cell clicked; None when it isn't one (openKnownLink).
pub fn open_known_link(m: &mut Model, u: &str) -> Option<Out> {
    super::linkpop::open_link_pop(m, u).then_some(Out::Draw)
}
