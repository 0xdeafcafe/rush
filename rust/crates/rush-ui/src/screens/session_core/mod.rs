//! The Session: an open conversation's message box, sending, the queue,
//! interrupting and the live state of a session rush hosts. The
//! milestone-1 read-only view (session.rs) stays the conversation; this
//! group adds what's under it and the keys that type, send and stop.
//! After Go's internal/ui/rushmode.go (hostConn :1452, syncHost :1894,
//! onHostLines :2083, paneDock's sending/queue/box/hint :3209-3687,
//! paneKey :3792, sendPane :4440, sendOffline :5001), sending.go,
//! sendmode.go, localqueue.go, subqueue.go, sessionviews.go (the queue's
//! keys :23-280), hosted.go (canInterrupt :196, stopTurn :202) and
//! stash.go (wipeBox :173, boxVert :225).
//!
//! Its views (sessionviews.go's tabs, rushmode.go's paneView and
//! paneHeader tabs), the dragged-over text (textsel.go), the map
//! (minimap.go), the pointer over the pane and header (pane_pointer.go,
//! header_pointer.go, header_context.go) and #expand/#collapse
//! (history_folds.go) are views.rs and the modules it names. rail.go is
//! the list's (list.rs).
//!
//! What the session sends goes through [`host::Hosts`]: rush-host is being
//! ported on another branch, which registers its own with
//! [`host::set_hosts`]; until then [`host::NoHosts`] says rush can't send.

pub mod changes;
pub mod conversation;
pub mod dock;
pub mod history;
pub mod host;
pub mod keys;
pub mod live;
pub mod memory;
pub mod minimap;
pub mod overview;
pub mod paint;
pub mod queue;
pub mod sending;
pub mod sendmode;
pub mod side;
pub mod subqueue;
pub mod tasks;
pub mod textsel;
pub mod views;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;

use crate::frame::inputbox::InputBox;
use crate::frame::{Editor, Ext};
use crate::model::Model;
use host::{Hosts, Info, Link};
use queue::LocalQueue;
use sending::Outbox;
use sendmode::SendMode;
use subqueue::SubNote;

/// Adds the Session's keys, dock, paste, mouse and live state to x.
pub fn register(x: &mut Ext) {
    x.session_keys.push(textsel::key);
    x.session_keys.push(keys::key);
    x.session_keys.push(views::key);
    x.session_dock.push(dock::dock);
    x.session_body.push(views::body);
    x.session_head.push(views::head);
    x.paste.push(keys::paste);
    x.mouse.push(keys::mouse);
    x.mouse.push(conversation::mouse);
    x.mouse.push(textsel::mouse);
    x.mouse.push(views::mouse);
    history::register(x);
    x.overlays.push(dock::locate);
    x.overlays.push(conversation::over);
    x.on_fleet.push(live::on_fleet);
    x.status.push(dock::status);
}

/// What the Session keeps between frames, for every session opened.
#[derive(Default)]
pub struct Core {
    /// Where messages go (NoHosts until set).
    pub hosts: Option<Arc<dyn Hosts>>,
    /// Each session's box, outbox and connection, by its key.
    pub conns: HashMap<String, Conn>,
    /// Messages waiting for sessions rush doesn't run, by key, and for
    /// watched subagents, by subqueue::key.
    pub local_q: HashMap<String, LocalQueue>,
    /// How enter sends to each session while it works, once picked.
    pub send_modes: HashMap<String, SendMode>,
    /// Messages meant for subagents their conversation doesn't show.
    pub sub_notes: HashMap<String, Vec<SubNote>>,
    /// The bottom row's keys, as the dock last worked them out.
    pub hint: Option<rush_core::text::Line>,
    /// The session whose link is open.
    pub linked: String,
    /// No map beside the conversation, whatever the config says (tests
    /// drawn as Go's, with HideMinimap).
    pub hide_map: bool,
    /// Bumped for each link opened: what an older one says is dropped.
    pub link_gen: u64,
}

/// One session's side of the Session (Go's hostConn, the parts that
/// aren't the conversation's view).
#[derive(Default)]
pub struct Conn {
    /// Its host, when rush runs it and it's connected.
    pub link: Option<Arc<dyn Link>>,
    pub link_gen: u64,
    /// The worker following the link stops once this is set.
    pub stop: Arc<AtomicBool>,
    /// Dialling the host is under way.
    pub dialing: bool,
    /// The process last dialled: a host gone is dialled again only once
    /// another runs it.
    pub dialed_pid: i32,
    /// The host's queue and state.
    pub info: Info,
    /// The message box.
    pub input: Editor,
    /// The box as last drawn, and the screen row its top edge landed on.
    pub drawn: InputBox,
    pub box_y: Option<usize>,
    /// The box's top edge, and the queue card's first row, as rows into
    /// the dock.
    pub box_off: usize,
    pub q_off: usize,
    /// The queue card's rows as last drawn: the message on each, from
    /// the screen row q_top.
    pub q_rows: Vec<Option<usize>>,
    pub q_top: usize,
    /// The queue card is on the transcript's end, scrolling with it, and
    /// the screen rows [from, to) the conversation shows it in.
    pub q_panel: bool,
    /// The task rows drawn just above the box, how wide, and whether
    /// they're opened from their one.
    pub task_rows: usize,
    pub task_w: usize,
    pub tasks_open: bool,
    /// The column beside a wide conversation, as the dock last made it.
    pub side: side::Side,
    pub q_shown: (usize, usize),
    pub out: Outbox,
    /// When the box was last wiped: its hints say how to bring it back.
    pub cleared_at: Ms,
    /// When it was last asked to stop.
    pub stop_armed: Ms,
    pub last_send: Ms,
    /// The queued message being edited in the box, +1; 0 when none; its
    /// text before, and whether editing held the queue.
    pub edit_q: usize,
    pub edit_was: String,
    pub edit_held: bool,
    /// Its views: which shows, where each is, what the pointer is over.
    pub vs: views::ViewState,
    /// Text dragged over in its conversation.
    pub txt: textsel::TextSel,
    /// Its conversation's map.
    pub map: minimap::Minimap,
    /// How many rows the dock took as last drawn: the map runs on down
    /// beside them.
    pub dock_rows: usize,
}

impl Conn {
    /// rush hosts it and it's connected (Go's c.client != nil).
    pub fn hosted(&self) -> bool {
        self.link.is_some()
    }
}

/// The open session's key, if one is open.
pub fn open_key(m: &Model) -> Option<String> {
    m.session.as_ref().map(|v| v.key.clone())
}

/// The agent with key, as the list last read it.
pub fn agent<'a>(m: &'a Model, key: &str) -> Option<&'a Agent> {
    m.agents.iter().find(|a| a.key == key)
}

/// The Session's state.
pub fn core(m: &mut Model) -> &mut Core {
    m.kept::<Core>()
}

/// Session key's side, made on first use.
pub fn conn<'a>(m: &'a mut Model, key: &str) -> &'a mut Conn {
    core(m).conns.entry(key.to_string()).or_default()
}

/// Working or waiting on you (localqueue.go busy).
pub fn busy(a: &Agent) -> bool {
    matches!(a.job.state, State::Working | State::Blocked)
}

/// The agent's own words for where an interactive-kind session is driven
/// from (fleet.go:1551 Where).
pub fn where_(a: &Agent) -> &'static str {
    if a.headless && a.kind.as_str() == "claude" {
        return "run by another program (claude -p)";
    }
    if a.headless {
        return "run by another program";
    }
    "open in a terminal"
}
