//! What every screen reads of an open session's connection: its host's
//! link and info, its message box, whether it sleeps, what fills its
//! context window ([`ConnCore`]), and the Session's own state the others
//! read (the row of keys, the session whose link is open). The rest of a
//! session's side (its views, cards, queue, runs) is session_core's
//! `Conn`, which holds a ConnCore and reads as one (Deref); the screens
//! reach a session's ConnCore through [`conn`] and [`peek_conn`], which
//! the Calls slots session_core fills answer. After Go's
//! internal/ui/rushmode.go hostConn (:1452), the parts the other screens
//! read.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rush_core::agent::{Agent, State};

use crate::frame::{Editor, InputBox};
use crate::host::{Info, Link};
use crate::model::Model;

/// What a pinned row's ref starts with: a click on it goes to its turn.
pub const PIN_REF: &str = "pin:";

/// The part of one session's side every screen reads (Go's hostConn's
/// link, info and box).
#[derive(Default)]
pub struct ConnCore {
    /// Its host, when rush runs it and it's connected.
    pub link: Option<Arc<dyn Link>>,
    pub link_gen: u64,
    /// The worker following the link stops once this is set.
    pub stop: Arc<AtomicBool>,
    /// Dialling the host is under way.
    pub dialing: bool,
    /// Its host sleeps, the session kept: the socket's let go and the
    /// conversation stays; a message wakes it (sleep_host.go parkHost).
    pub sleeping: bool,
    /// A wake is under way: the box keeps what was sent until it's known
    /// to have gone (c.waking).
    pub waking: bool,
    /// The back sheet has opened on this connection: an away that ended
    /// opens it by itself once (c.backShown).
    pub back_shown: bool,
    /// Plugins are being asked about what's in the box before it goes,
    /// and have just been: it goes past them once (c.intercepting,
    /// c.intercepted; plugin_ui::intercept).
    pub intercepting: bool,
    pub intercepted: bool,
    /// The process last dialled: a host gone is dialled again only once
    /// another runs it.
    pub dialed_pid: i32,
    /// The host's queue and state.
    pub info: Info,
    /// The message box.
    pub input: Editor,
    /// The box as last drawn.
    pub drawn: InputBox,
    /// The images its box holds, each [Image #N] in its text (c.imgs).
    pub imgs: crate::imagerefs::ImageRefs,
    /// The queued message being edited in the box, +1; 0 when none; its
    /// text before, and whether editing held the queue.
    pub edit_q: usize,
    pub edit_was: String,
    pub edit_held: bool,
    /// What fills its context window, as last counted (sess.Usage).
    pub context: Option<Box<rush_core::usage::Context>>,
}

impl ConnCore {
    /// rush hosts it and it's connected (Go's c.client != nil).
    pub fn hosted(&self) -> bool {
        self.link.is_some()
    }
}

/// The Session's own state the other screens read (Model::kept).
#[derive(Default)]
pub struct SessState {
    /// The bottom row's keys, as the dock last worked them out.
    pub hint: Option<rush_core::text::Line>,
    /// The session whose link is open.
    pub linked: String,
}

/// The Session's state the other screens read, to change.
pub fn state(m: &mut Model) -> &mut SessState {
    m.kept::<SessState>()
}

/// The Session's state, to read while drawing; None before it's made.
pub fn peek_state(m: &Model) -> Option<&SessState> {
    m.kept.peek::<SessState>()
}

/// Session key's side, made on first use (session_core's conn).
pub fn conn<'a>(m: &'a mut Model, key: &str) -> &'a mut ConnCore {
    let f = m.ext.calls.conn();
    f(m, key)
}

/// Session key's side, when it has one, to read.
pub fn peek_conn<'a>(m: &'a Model, key: &str) -> Option<&'a ConnCore> {
    let f = m.ext.calls.peek_conn();
    f(m, key)
}

/// Session key's side, when it has one, to change: none is made.
pub fn conn_if<'a>(m: &'a mut Model, key: &str) -> Option<&'a mut ConnCore> {
    let f = m.ext.calls.conn_if();
    f(m, key)
}

/// The session with this key has its host reached (Conn::hosted).
pub fn linked(m: &Model, key: &str) -> bool {
    peek_conn(m, key).is_some_and(ConnCore::hosted)
}

/// The message box of the session with this key, when it has one.
pub fn conn_box<'a>(m: &'a Model, key: &str) -> Option<&'a Editor> {
    peek_conn(m, key).map(|c| &c.input)
}

/// conn_box, to change.
pub fn conn_box_mut<'a>(m: &'a mut Model, key: &str) -> Option<&'a mut Editor> {
    conn_if(m, key).map(|c| &mut c.input)
}

/// The open session's key, if one is open.
pub fn open_key(m: &Model) -> Option<String> {
    m.session.as_ref().map(|v| v.key.clone())
}

/// The agent with key, as the list last read it.
pub fn agent<'a>(m: &'a Model, key: &str) -> Option<&'a Agent> {
    m.agents.iter().find(|a| a.key == key)
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
