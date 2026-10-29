//! What the UI does with links of a kind rush knows (rush_convo's
//! linkinfo): what a click on one opens in rush, and what its menu offers
//! before Open in browser and Copy link. A kind with no handler still
//! draws its badge; a click on it does what any link's does. Port of
//! internal/ui/linkkinds.go:1-75 (linkHandler :14, linkHandlers :24,
//! knownLink :33, openKnownLink :44, openPRLink :53, prLinkActs :61),
//! and the lookups turned on as the TUI starts, an answer landing drawing
//! it again (model.go:447-450 New, hosted.go:27 NewHosted).
//!
//! A pull request's link opens in its sheet; merging from its menu only
//! asks how (the sheet's question): nothing is merged without a key.

use std::any::Any;
use std::sync::{Arc, Mutex, PoisonError};

use rush_convo::linkinfo::{LINK_GITHUB_PR, link_info, look_links, on_lookup};
use rush_draw::Sender;
use rush_gh::ghpr::PR;

use crate::frame::Out;
use crate::model::Model;
use crate::screens::projects_machine::prsheet;
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

/// What a kind's lookup said of a link.
pub type Value = Arc<dyn Any + Send + Sync>;

/// One thing a kind's menu offers first.
pub struct KindAct {
    pub label: &'static str,
    pub run: Box<dyn Fn(&mut Model) -> Out>,
}

/// What the UI does with links of one kind (linkHandler).
#[derive(Clone, Copy)]
pub struct LinkHandler {
    /// Opens what u is to in rush, with what its kind said of it; None
    /// leaves the click to the terminal.
    pub open: Option<fn(&mut Model, &str, &Value) -> Out>,
    /// What the menu on u offers first.
    pub acts: Option<fn(&str, &Value) -> Vec<KindAct>>,
}

/// The handler for links of kind, by its name (linkHandlers).
fn handler(kind: &str) -> Option<LinkHandler> {
    match kind {
        LINK_GITHUB_PR => Some(LinkHandler {
            open: Some(|m, u, v| match v.downcast_ref::<PR>() {
                Some(pr) => open_pr_link(m, u, pr),
                None => Out::Pass,
            }),
            acts: Some(pr_link_acts),
        }),
        _ => None,
    }
}

/// The handler for u and what its kind said of it, when a kind knows u,
/// has answered, and the UI does something with it (knownLink).
pub fn known_link(u: &str) -> Option<(LinkHandler, Value)> {
    let (kind, _, v) = link_info(u)?;
    Some((handler(kind)?, v))
}

/// Opens a link of a kind rush knows in rush; None when it didn't
/// (openKnownLink).
pub fn open_known_link(m: &mut Model, u: &str) -> Option<Out> {
    let (h, v) = known_link(u)?;
    let open = h.open?;
    Some(open(m, u, &v))
}

/// Opens a linked pull request in its sheet, with its checks, reviews
/// and conversation, and merging, approving and commenting on it
/// (openPRLink).
pub fn open_pr_link(m: &mut Model, u: &str, pr: &PR) -> Out {
    prsheet::open_pr_link(m, u, pr);
    Out::Draw
}

/// What the menu on a pull request's link offers: opening it in rush, and
/// merging it from there when it's open (prLinkActs).
fn pr_link_acts(u: &str, v: &Value) -> Vec<KindAct> {
    let Some(pr) = v.downcast_ref::<PR>().cloned() else {
        return Vec::new();
    };
    let (u1, pr1) = (u.to_string(), pr.clone());
    let mut acts = vec![KindAct {
        label: "Open in rush",
        run: Box::new(move |m| open_pr_link(m, &u1, &pr1)),
    }];
    if pr.state == "OPEN" {
        let u = u.to_string();
        acts.push(KindAct {
            label: "Merge…",
            run: Box::new(move |m| {
                let out = open_pr_link(m, &u, &pr);
                prsheet::ask_merge_open(m, &pr);
                out
            }),
        });
    }
    acts
}
